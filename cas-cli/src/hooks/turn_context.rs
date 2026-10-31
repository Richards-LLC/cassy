//! Redundant factory context delivery when Claude skips UserPromptSubmit.
//!
//! A bounded transcript tail identifies turns; a per-session file lock shares
//! receipts between hook processes and the MCP server. Prompt bodies are never
//! persisted here or sent to attribution/memory ingestion.
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::Path;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::HookInput;

const TAIL_BYTES: u64 = 256 * 1024;
/// cas-a1ed: how far back a tool-boundary mail check looks for the start of
/// the current turn. A long working turn writes far more than [`TAIL_BYTES`]
/// of tool output after its first line (golden-stork-66 had none of 23 tool
/// boundaries' turn starts inside the tail), and a check that cannot place the
/// turn cannot tell a held message from the turn's own prompt.
const TURN_START_SCAN_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
const RECENT_TURNS: usize = 32;

#[derive(Default, Serialize, Deserialize)]
struct TurnState {
    delivered: Vec<String>,
    silent_prompts: u64,
    updated_at: i64,
    /// cas-b5e4: epoch milliseconds of the last tool-boundary inbox check.
    #[serde(default)]
    mail_checked_at: i64,
    /// cas-1f0c: turns whose `CAS wake: message N` pointers are resolved, so
    /// their later tool boundaries stop re-checking the named rows.
    #[serde(default)]
    pointers_resolved: Vec<String>,
    /// cas-6ccb: turns whose polled-but-unacknowledged rows were checked.
    #[serde(default)]
    polled_checked: Vec<String>,
    /// cas-a1ed: resume point of the deep turn-start scan.
    #[serde(default)]
    turn_scan: Option<TurnScanCache>,
}

struct Receipt {
    file: File,
    state: TurnState,
}

impl Receipt {
    fn open(root: &Path, session: &str) -> Option<Self> {
        if session.trim().is_empty() {
            return None;
        }
        let directory = root.join("turn-context");
        fs::create_dir_all(&directory).ok()?;
        let key = format!("{:x}", Sha256::digest(session.as_bytes()));
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join(format!("{key}.json")))
            .ok()?;
        file.try_lock_exclusive().ok()?;
        let mut data = String::new();
        (&mut file).take(16 * 1024).read_to_string(&mut data).ok()?;
        let state = if data.is_empty() {
            TurnState::default()
        } else {
            serde_json::from_str(&data).ok()?
        };
        Some(Self { file, state })
    }

    fn save(&mut self) -> Option<()> {
        self.state.updated_at = chrono::Utc::now().timestamp();
        let data = serde_json::to_vec(&self.state).ok()?;
        self.file.seek(SeekFrom::Start(0)).ok()?;
        self.file.write_all(&data).ok()?;
        self.file.set_len(data.len() as u64).ok()?;
        Some(())
    }

    fn remember(&mut self, key: &str) {
        remember_recent(&mut self.state.delivered, key);
    }
}

fn remember_recent(keys: &mut Vec<String>, key: &str) {
    if !keys.iter().any(|seen| seen == key) {
        keys.push(key.to_string());
        if keys.len() > RECENT_TURNS {
            keys.remove(0);
        }
    }
}

/// Successful normal hooks suppress fallback recall for that same prompt.
pub(crate) fn record_prompt_hook(root: &Path, input: &HookInput) {
    if crate::internal_llm::is_internal_invocation() {
        return;
    }
    let Some(key) = input.prompt_id.as_deref() else {
        return;
    };
    let Some(mut receipt) = Receipt::open(root, &input.session_id) else {
        return;
    };
    receipt.remember(key);
    receipt.state.silent_prompts = 0;
    let _ = receipt.save();
}

/// Read only real external user messages, never tool results, sidechains or
/// meta reminders. `promptId` aligns with the normal hook's `prompt_id`.
#[cfg(test)]
fn latest_turn(path: &Path, session: &str) -> Option<(String, String)> {
    latest_turn_with_start(path, session).map(|(key, prompt, _)| (key, prompt))
}

type TranscriptTurn = (String, String, Option<chrono::DateTime<chrono::Utc>>);

/// [`latest_turn`] plus the turn's start time from the transcript entry's
/// `timestamp`, when present. Reads only the transcript's last [`TAIL_BYTES`].
fn latest_turn_with_start(path: &Path, session: &str) -> Option<TranscriptTurn> {
    latest_turn_within(path, session, TAIL_BYTES)
}

/// The newest turn start in the transcript's last `limit` bytes (cas-a1ed).
fn latest_turn_within(path: &Path, session: &str, limit: u64) -> Option<TranscriptTurn> {
    scan_turn_starts(path, session, |size| size.saturating_sub(limit))
        .and_then(|scan| scan.hit)
        .map(|(turn, _)| turn)
}

/// One backwards scan for the newest turn start at or after a floor.
struct TurnStartScan {
    /// The newest turn start found, with the byte offset of its line.
    hit: Option<(TranscriptTurn, u64)>,
    /// Where a later scan may resume: the start of the last line, which may
    /// still have been mid-write.
    resume_from: u64,
}

/// Read backwards from the end of the transcript to `floor(size)`, a
/// [`TAIL_BYTES`] block at a time, so a turn near the end costs one block.
fn scan_turn_starts(
    path: &Path,
    session: &str,
    floor: impl FnOnce(u64) -> u64,
) -> Option<TurnStartScan> {
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let floor = floor(size).min(size);
    let mut end = size;
    let mut resume_from = None;
    // The start of a line that began in an earlier, not yet read, block.
    let mut carry: Vec<u8> = Vec::new();
    while end > floor {
        let start = end.saturating_sub(TAIL_BYTES).max(floor);
        // One byte more, when there is one, says whether `start` begins a line.
        let read_from = start.saturating_sub(1);
        let mut block = vec![0; (end - read_from) as usize];
        file.seek(SeekFrom::Start(read_from)).ok()?;
        file.read_exact(&mut block).ok()?;
        let starts_line = start == 0 || block[0] == b'\n';
        if start > 0 {
            block.remove(0);
        }
        block.extend_from_slice(&carry);
        // `block` now holds the bytes from `start` on.
        let complete_from = if starts_line {
            0
        } else {
            match block.iter().position(|byte| *byte == b'\n') {
                Some(newline) => newline + 1,
                None => {
                    carry = block;
                    end = start;
                    continue;
                }
            }
        };
        let mut line_end = block.len();
        loop {
            let line_start = block[complete_from..line_end]
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(complete_from, |newline| complete_from + newline + 1);
            let at = start + line_start as u64;
            resume_from.get_or_insert(at);
            if let Some(turn) = turn_start_line(&block[line_start..line_end], session) {
                return Some(TurnStartScan {
                    hit: Some((turn, at)),
                    resume_from: resume_from.unwrap_or(at),
                });
            }
            if line_start == complete_from {
                break;
            }
            line_end = line_start - 1;
        }
        carry = block[..complete_from.saturating_sub(1)].to_vec();
        end = start;
    }
    Some(TurnStartScan {
        hit: None,
        resume_from: resume_from.unwrap_or(floor),
    })
}

/// cas-a1ed review M1: where the last deep turn-start scan of this session's
/// transcript stopped, so the next one reads only the bytes added since.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct TurnScanCache {
    path: String,
    /// Bytes before this offset have been scanned.
    scanned_to: u64,
    /// Offset of the newest turn-start line found so far.
    turn_at: Option<u64>,
}

/// The current turn when its first line is outside the tail: scan back up to
/// [`TURN_START_SCAN_LIMIT_BYTES`] once, then only what was appended since.
fn deep_turn_start(
    path: &Path,
    session: &str,
    cache: &mut Option<TurnScanCache>,
) -> Option<TranscriptTurn> {
    let key = path.to_string_lossy().into_owned();
    let size = fs::metadata(path).ok()?.len();
    let previous = cache
        .as_ref()
        .filter(|scan| scan.path == key && scan.scanned_to <= size)
        .cloned();
    let scan = scan_turn_starts(path, session, |size| match &previous {
        Some(scan) => scan.scanned_to,
        None => size.saturating_sub(TURN_START_SCAN_LIMIT_BYTES),
    })?;
    let (turn, turn_at) = match scan.hit {
        Some((turn, at)) => (Some(turn), Some(at)),
        None => {
            let at = previous.as_ref().and_then(|scan| scan.turn_at);
            (at.and_then(|at| turn_line_at(path, session, at)), at)
        }
    };
    *cache = Some(TurnScanCache {
        path: key,
        scanned_to: scan.resume_from,
        turn_at,
    });
    turn
}

/// Re-read the turn-start line at `at`; prompt text is never cached.
fn turn_line_at(path: &Path, session: &str, at: u64) -> Option<TranscriptTurn> {
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(at)).ok()?;
    let mut line = Vec::new();
    std::io::BufReader::new(file.take(TURN_START_SCAN_LIMIT_BYTES))
        .read_until(b'\n', &mut line)
        .ok()?;
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    turn_start_line(&line, session)
}

/// Whether a line can be a turn start: its message content is a string. Tool
/// results are user entries too, but their content is an array, so they are
/// skipped without being parsed.
fn may_be_turn_start(line: &str) -> bool {
    const CONTENT: &str = "\"content\":";
    line.find("\"message\":")
        .and_then(|message| {
            let rest = &line[message..];
            rest.find(CONTENT).map(|content| &rest[content + CONTENT.len()..])
        })
        .is_some_and(|value| value.trim_start().starts_with('"'))
}

/// Whether one transcript line is a real external user message of `session`.
fn turn_start_line(line: &[u8], session: &str) -> Option<TranscriptTurn> {
    let line = String::from_utf8_lossy(line);
    if !may_be_turn_start(&line) {
        return None;
    }
    let line = line.trim_end_matches('\r');
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return None;
    };
    if value["type"] != "user" || value["isMeta"] == true || value["isSidechain"] == true {
        return None;
    }
    if value
        .get("sessionId")
        .and_then(|v| v.as_str())
        .is_some_and(|id| id != session)
    {
        return None;
    }
    // Claude emits operator prompts as strings. Arrays include tool_result
    // and synthetic user messages; do not guess their provenance.
    let prompt = value["message"]["content"].as_str()?;
    let key = value["promptId"]
        .as_str()
        .or_else(|| value["uuid"].as_str())?;
    let started_at = value["timestamp"]
        .as_str()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .map(|at| at.with_timezone(&chrono::Utc));
    Some((key.to_string(), prompt.to_string(), started_at))
}

/// Called by PostToolUse and by successful MCP responses.
///
/// cas-b5e4 (GH #989): mail is checked at every tool boundary, so a message
/// that reaches a busy agent mid-turn is in its context within one tool call.
/// The check reads only a small per-recipient stamp until something new was
/// enqueued; inbox receipts are atomic and shared with UserPromptSubmit, so a
/// row surfaces once. Recall still runs once per turn, and only when the
/// normal prompt hook did not serve the turn.
pub(crate) fn fallback_context(root: &Path, input: &HookInput) -> Option<String> {
    fallback_context_within(root, input, 0)
}

/// [`fallback_context`] for an output that already carries `reserved`
/// characters of other context (cas-1f0c: PostToolUse lessons and guardrail
/// warnings), so the whole stays within Claude Code's hook cap.
pub(crate) fn fallback_context_within(
    root: &Path,
    input: &HookInput,
    reserved: usize,
) -> Option<String> {
    if crate::internal_llm::is_internal_invocation()
        || !super::handlers::handlers_middle::factory_inbox::owns_factory_inbox(input)
    {
        return None;
    }
    use super::handlers::handlers_middle::factory_inbox::{
        CurrentTurn, inbox_signal_stamp, names_wake_pointer, surface_factory_inbox_after_tool_result,
        surface_polled_unacked, surface_turn_prompt_rows,
    };
    let turn = input
        .transcript_path
        .as_deref()
        .and_then(|path| latest_turn_with_start(Path::new(path), &input.session_id));
    let signal = inbox_signal_stamp(root, input);
    let (turn_unserved, pointers_pending, polled_pending) = turn
        .as_ref()
        .and_then(|(key, prompt, started_at)| {
            let receipt = Receipt::open(root, &input.session_id)?;
            Some((
                !receipt.state.delivered.contains(key),
                names_wake_pointer(prompt) && !receipt.state.pointers_resolved.contains(key),
                started_at.is_some() && !receipt.state.polled_checked.contains(key),
            ))
        })
        .unwrap_or((false, false, false));
    if signal.is_none() && !turn_unserved && !pointers_pending && !polled_pending {
        return None;
    }
    let mut receipt = Receipt::open(root, &input.session_id)?;
    let mut parts = Vec::new();
    let mut changed = false;
    // cas-1f0c: one render budget for every block this output carries.
    let mut budget = super::handlers::handlers_middle::factory_inbox::hook_body_budget(reserved);

    // cas-1f0c: the rows this turn's own prompt names come first, and before
    // the turn is recorded below, so only an earlier surfacing reads as
    // "already shown". A Claude Teams copy is a pointer, and its teammate turn
    // fires no UserPromptSubmit; a pointer turn whose turn-start hook failed
    // or ran out of room is retried here until its rows resolve.
    if let Some((key, prompt, started_at)) = &turn {
        let first_boundary = !receipt.state.delivered.contains(key);
        let pending = names_wake_pointer(prompt) && !receipt.state.pointers_resolved.contains(key);
        // cas-6ccb: this turn's polled-but-unacknowledged rows go in the
        // same block, so a row gets one note.
        let polled_before = (*started_at).filter(|_| !receipt.state.polled_checked.contains(key));
        if (first_boundary || pending)
            && let Some(outcome) =
                surface_turn_prompt_rows(root, input, prompt, first_boundary, polled_before, &mut budget)
        {
            parts.extend(outcome.text);
            if polled_before.is_some() {
                remember_recent(&mut receipt.state.polled_checked, key);
                changed = true;
            }
            if outcome.complete && names_wake_pointer(prompt) {
                remember_recent(&mut receipt.state.pointers_resolved, key);
                changed = true;
            }
        }
    }

    // cas-6ccb (GH #1215): once per turn, independent of new mail, a row
    // read with inbox_poll before this turn and never acknowledged is shown
    // once more (a teammate-message turn fires no turn-start hook).
    if let Some((key, _, Some(started_at))) = &turn
        && !receipt.state.polled_checked.contains(key)
        && let Some(outcome) = surface_polled_unacked(root, input, *started_at, &mut budget)
    {
        parts.extend(outcome.text);
        remember_recent(&mut receipt.state.polled_checked, key);
        changed = true;
    }

    if let Some(signal) = signal
        && signal > receipt.state.mail_checked_at
    {
        // Taken before the query: later mail stamps a newer signal. Commit
        // this cursor only after every selected body fits; otherwise the next
        // tool boundary must retry the overflow even without another enqueue.
        let checked_at = chrono::Utc::now().timestamp_millis();
        // cas-a1ed: deep in a long turn its first line is outside the tail;
        // look further back before checking mail without a turn.
        let deep;
        let turn_for_mail = if turn.is_some() {
            turn.as_ref()
        } else {
            deep = input.transcript_path.as_deref().and_then(|path| {
                deep_turn_start(Path::new(path), &input.session_id, &mut receipt.state.turn_scan)
            });
            deep.as_ref()
        };
        let current = turn_for_mail.and_then(|(_, prompt, started_at)| {
            started_at.map(|started_at| CurrentTurn {
                started_at,
                prompt: prompt.as_str(),
            })
        });
        if let Some(outcome) =
            surface_factory_inbox_after_tool_result(Some(root), input, current, &mut budget)
        {
            parts.extend(outcome.text);
            if outcome.complete {
                receipt.state.mail_checked_at = checked_at;
                changed = true;
            }
        }
    }

    if let Some((key, prompt, _)) = &turn
        && !receipt.state.delivered.contains(key)
    {
        // cas-7bfa: a teammate-message turn is a delivery; record it once.
        super::handlers::handlers_middle::factory_inbox::record_turn_prompt_envelopes(
            root, input, prompt,
        );
        receipt.remember(key);
        receipt.state.silent_prompts = receipt.state.silent_prompts.saturating_add(1);
        changed = true;
        // cas-1f0c: mail above is already receipted; recall may not take it
        // down (panic) or push the output past Claude Code's cap.
        if let Some(packet) = super::handlers::handlers_middle::factory_inbox::after_mail(|| {
            crate::ambient_recall::build_local_ambient_recall_context(input, root, prompt)
        }) {
            let mut joined = parts.join("\n\n");
            if super::handlers::handlers_middle::factory_inbox::push_within_hook_cap(
                &mut joined,
                &packet.full,
                reserved,
            ) {
                parts.push(packet.full);
            }
        }
    }
    // cas-1f0c: a failed save must not drop mail that is already receipted;
    // the turn is then simply re-examined at the next tool call.
    if changed && receipt.save().is_none() {
        tracing::warn!(
            target: "cas::coordination",
            session = %input.session_id,
            "cas-1f0c: turn-context receipt not saved; returning the rendered context anyway"
        );
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// Recent observed misses only: never infer silence from absent attribution
/// rows (supervisors intentionally omit them). Expire stopped sessions after a day.
pub(crate) fn silent_prompt_count(root: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(root.join("turn-context")) else {
        return 0;
    };
    let cutoff = chrono::Utc::now().timestamp() - 24 * 60 * 60;
    entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|e| e == "json"))
        .filter_map(|entry| fs::read(entry.path()).ok())
        .filter_map(|data| serde_json::from_slice::<TurnState>(&data).ok())
        .filter(|state| state.updated_at >= cutoff)
        .map(|state| state.silent_prompts)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_turn_excludes_tool_results_and_other_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        fs::write(&path, concat!(
            "{\"type\":\"user\",\"sessionId\":\"s\",\"promptId\":\"p\",\"message\":{\"content\":\"question\"}}\n",
            "{\"type\":\"user\",\"uuid\":\"tool\",\"message\":{\"content\":[{\"type\":\"tool_result\"}]}}\n",
            "{\"type\":\"user\",\"isMeta\":true,\"uuid\":\"meta\",\"message\":{\"content\":\"ignore\"}}\n",
            "{\"type\":\"user\",\"sessionId\":\"other\",\"uuid\":\"other\",\"message\":{\"content\":\"foreign\"}}\n",
        )).unwrap();
        assert_eq!(
            latest_turn(&path, "s"),
            Some(("p".into(), "question".into()))
        );
    }

    /// cas-a1ed: the turn's first line may sit anywhere behind a long run of
    /// tool output, including across a block boundary of the backward read.
    #[test]
    fn a_turn_start_far_behind_the_tail_is_found_across_block_boundaries_cas_a1ed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let turn = "{\"type\":\"user\",\"sessionId\":\"s\",\"promptId\":\"deep\",\"timestamp\":\"2026-10-15T22:00:00Z\",\"message\":{\"content\":\"start cas-223f\"}}\n";
        let tool = |bytes: usize| {
            format!(
                "{{\"type\":\"user\",\"sessionId\":\"s\",\"message\":{{\"content\":[{{\"type\":\"tool_result\",\"content\":\"{}\"}}]}}}}\n",
                "x".repeat(bytes.saturating_sub(90).max(1))
            )
        };
        let block = TAIL_BYTES as usize;
        for after in [block - 40, block - 1, block, block + 1, block + 40, 3 * block + 7] {
            let mut body = tool(5000).repeat(3);
            body.push_str(turn);
            let mut rest = String::new();
            while rest.len() < after {
                rest.push_str(&tool((after - rest.len()).min(7000)));
            }
            body.push_str(&rest);
            fs::write(&path, &body).unwrap();
            let found = latest_turn_within(&path, "s", TURN_START_SCAN_LIMIT_BYTES)
                .unwrap_or_else(|| panic!("turn not found with {after} bytes after it"));
            assert_eq!(found.0, "deep");
            assert_eq!(found.1, "start cas-223f");
            assert!(found.2.is_some());
            if rest.len() > block {
                assert!(latest_turn_with_start(&path, "s").is_none(), "the tail alone misses it");
            }
            assert!(latest_turn_within(&path, "s", (rest.len() - 1) as u64).is_none(), "bounded by the limit");
        }
    }

    /// cas-a1ed review M1: the deep scan reads the transcript back once and
    /// afterwards only the bytes appended since; tool results are skipped
    /// without being parsed.
    #[test]
    fn the_deep_turn_scan_resumes_where_it_stopped_cas_a1ed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let line = |key: &str, content: &str| {
            format!(
                "{{\"type\":\"user\",\"sessionId\":\"s\",\"promptId\":\"{key}\",\"timestamp\":\"2026-10-15T22:00:00Z\",\"message\":{{\"content\":{content}}}}}\n"
            )
        };
        let tool = line("tool", &format!("[{{\"type\":\"tool_result\",\"content\":\"{}\"}}]", "x".repeat(4000)));
        // A turn start padded to exactly one tool line, to overwrite one in place.
        let stale_prefix = line("stale", "\"\"");
        let stale = line("stale", &format!("\"{}\"", " ".repeat(tool.len() - stale_prefix.len())));
        assert_eq!(stale.len(), tool.len());
        let mut body = line("deep", "\"start cas-223f\"");
        let first_tool = body.len();
        body.push_str(&tool.repeat(200));
        fs::write(&path, &body).unwrap();

        let mut cache = None;
        let found = deep_turn_start(&path, "s", &mut cache).unwrap();
        assert_eq!(found.0, "deep");
        let scan = cache.clone().unwrap();
        assert_eq!(scan.scanned_to, body.len() as u64);
        assert_eq!(scan.turn_at, Some(0));

        // Rewrite an already-scanned line into a newer turn start, then append.
        {
            use std::io::Write as _;
            let mut file = OpenOptions::new().write(true).open(&path).unwrap();
            file.seek(SeekFrom::Start(first_tool as u64)).unwrap();
            file.write_all(stale.as_bytes()).unwrap();
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            file.write_all(tool.repeat(10).as_bytes()).unwrap();
        }
        let resumed = deep_turn_start(&path, "s", &mut cache).unwrap();
        assert_eq!(resumed.0, "deep", "only appended bytes are read again");
        assert_eq!(resumed.1, "start cas-223f", "the prompt is re-read from its line");
        assert!(cache.as_ref().unwrap().scanned_to > scan.scanned_to);
        let mut fresh = None;
        assert_eq!(deep_turn_start(&path, "s", &mut fresh).unwrap().0, "stale");

        // A turn appended later is found in the new bytes.
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(line("newer", "\"next\"").as_bytes())
            .unwrap();
        assert_eq!(deep_turn_start(&path, "s", &mut cache).unwrap().0, "newer");
        assert!(!may_be_turn_start(&tool), "a tool result is not parsed");
    }

    #[test]
    fn prompt_hook_receipt_recovers_silence_and_deduplicates() {
        let _guard = crate::hooks::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let mut receipt = Receipt::open(dir.path(), "s").unwrap();
        receipt.state.silent_prompts = 2;
        receipt.save().unwrap();
        drop(receipt);
        assert_eq!(silent_prompt_count(dir.path()), 2);
        record_prompt_hook(
            dir.path(),
            &HookInput {
                session_id: "s".into(),
                prompt_id: Some("p".into()),
                ..Default::default()
            },
        );
        assert_eq!(silent_prompt_count(dir.path()), 0);
        let receipt = Receipt::open(dir.path(), "s").unwrap();
        assert_eq!(receipt.state.delivered, ["p"]);
        assert!(
            Receipt::open(dir.path(), "s").is_none(),
            "concurrent readers must not duplicate a turn"
        );
    }
    fn teams_env() -> crate::test_support::TestEnvGuard {
        crate::test_support::TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_AGENT_NAME", Some("worker-7bfa")),
            ("CAS_FACTORY_SESSION", Some("sess-7bfa")),
            (crate::internal_llm::INTERNAL_LLM_ENV, None),
        ])
    }

    fn teams_input(transcript: &Path, agent_id: &str) -> HookInput {
        HookInput {
            session_id: "s-7bfa".into(),
            transcript_path: Some(transcript.to_string_lossy().into_owned()),
            hook_event_name: "PostToolUse".into(),
            tool_name: Some("Bash".into()),
            agent_id: Some(agent_id.into()),
            agent_type: Some("general-purpose".into()),
            ..Default::default()
        }
    }

    fn turn_line(uuid: &str, at: chrono::DateTime<chrono::Utc>, prompt: &str) -> String {
        format!(
            "{}\n",
            serde_json::json!({
                "type": "user",
                "uuid": uuid,
                "sessionId": "s-7bfa",
                "timestamp": at.to_rfc3339(),
                "message": {"content": prompt},
            })
        )
    }

    /// Communication contract v2, P1.2/P2.3 (cas-7bfa, GH #1209): a Claude
    /// Agent Teams member's hooks carry its own `agent_id` (`<name>@<team>`).
    /// A row the daemon claimed mid-turn, while the wake gate deferred it,
    /// surfaces at the member's next tool boundary. A real subagent (hex
    /// agent_id) still does not drain the parent's inbox.
    #[test]
    fn ct2_p1_teams_member_gated_row_surfaces_at_tool_boundary() {
        let _env = teams_env();
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("session.jsonl");
        fs::write(
            &transcript,
            turn_line("t1", chrono::Utc::now() - chrono::Duration::seconds(60), "work on cas-x"),
        )
        .unwrap();
        let queue = crate::store::open_prompt_queue_store(dir.path()).unwrap();
        let id = queue
            .enqueue_with_session("supervisor", "worker-7bfa", "P0: rebase now", "sess-7bfa")
            .unwrap();
        assert!(queue.claim_recipient_transport(id, "worker-7bfa").unwrap());
        queue.record_deferred_inbox(id, 0).unwrap();
        queue
            .record_pending_reason(
                id,
                cas_store::PendingReason::GatedNotReady,
                Some("pane has not been silent long enough"),
            )
            .unwrap();

        assert!(
            fallback_context(dir.path(), &teams_input(&transcript, "a24c4ba99866d62e3")).is_none(),
            "a subagent never drains its parent's inbox"
        );
        assert!(
            fallback_context(dir.path(), &teams_input(&transcript, "worker-7bfa@other-team"))
                .is_none(),
            "a same-named member of another team is not this session"
        );
        assert_eq!(
            queue.message_delivery_report(id).unwrap().unwrap().recipient_receipt,
            Some(cas_store::SurfacingSource::TransportClaimed),
            "neither foreign identity touched the pending row"
        );
        let mail = fallback_context(dir.path(), &teams_input(&transcript, "worker-7bfa@sess-7bfa"))
            .expect("the Teams member's own tool boundary surfaces its mail");
        assert!(mail.contains("P0: rebase now"), "{mail}");
        let report = queue.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(report.recipient_receipt, Some(cas_store::SurfacingSource::HookSurfaced));
        assert_eq!(report.stage, cas_store::DeliveryStage::Confirmed, "hook surfacing acks the row");

        // The main thread may also arrive with only its --agent-type.
        let second = queue
            .enqueue_with_session("supervisor", "worker-7bfa", "P1: and re-run", "sess-7bfa")
            .unwrap();
        assert!(queue.claim_recipient_transport(second, "worker-7bfa").unwrap());
        queue.record_deferred_inbox(second, 0).unwrap();
        let mut type_only = teams_input(&transcript, "unused");
        type_only.agent_id = None;
        let mail = fallback_context(dir.path(), &type_only)
            .expect("agent_type alone is the main thread, not a child");
        assert!(mail.contains("P1: and re-run"), "{mail}");
    }

    /// Communication contract v2, P1.2/P1.7 (cas-7bfa, GH #1209): Claude Code
    /// delivers a held Teams copy as a teammate-message turn, and that turn
    /// fires no UserPromptSubmit. The turn's first tool boundary records each
    /// `[cas #N]` row in its prompt as surfaced, so the row leaves `gated` and
    /// can no longer raise a false "delivery stalled" notice.
    #[test]
    fn ct2_p1_teammate_turn_prompt_records_its_envelopes() {
        let _env = teams_env();
        let dir = tempfile::tempdir().unwrap();
        let queue = crate::store::open_prompt_queue_store(dir.path()).unwrap();
        let id = queue
            .enqueue_with_session("supervisor", "worker-7bfa", "next: cas-y", "sess-7bfa")
            .unwrap();
        assert!(queue.claim_recipient_transport(id, "worker-7bfa").unwrap());
        let other = queue
            .enqueue_with_session("supervisor", "worker-other", "not yours", "sess-7bfa")
            .unwrap();
        // Quoted but not carried: the envelope is in the prompt, the body is not.
        let quoted = queue
            .enqueue_with_session("supervisor", "worker-7bfa", "body never shown", "sess-7bfa")
            .unwrap();
        assert!(queue.claim_recipient_transport(quoted, "worker-7bfa").unwrap());
        // Never handed to a transport: envelope and body present, no receipt.
        let undelivered = queue
            .enqueue_with_session("worker-peer", "worker-7bfa", "still queued", "sess-7bfa")
            .unwrap();
        let transcript = dir.path().join("session.jsonl");
        fs::write(
            &transcript,
            turn_line(
                "t2",
                chrono::Utc::now() + chrono::Duration::seconds(1),
                &format!(
                    "<teammate-message teammate_id=\"supervisor\">\n[cas #{id} supervisor-authored 0s first]\n\nnext: cas-y\n[cas #{other} supervisor-authored 0s first]\nquoting [cas #{quoted} supervisor-authored 0s first] and [cas #{undelivered} worker-authored 0s first] still queued\n</teammate-message>"
                ),
            ),
        )
        .unwrap();
        // Isolate the turn-prompt path: the mail drain has already run for
        // every signal, so only the once-per-turn envelope record can act.
        {
            let mut receipt = Receipt::open(dir.path(), "s-7bfa").unwrap();
            receipt.state.mail_checked_at = i64::MAX / 2;
            receipt.save().unwrap();
        }

        let _ = fallback_context(dir.path(), &teams_input(&transcript, "worker-7bfa@sess-7bfa"));

        let report = queue.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(report.recipient_receipt, Some(cas_store::SurfacingSource::HookSurfaced));
        assert_eq!(report.stage, cas_store::DeliveryStage::Confirmed, "hook surfacing acks the row");
        assert!(report.confirmed_at.is_some(), "a body that entered a turn is confirmed");
        let foreign = queue.message_delivery_report(other).unwrap().unwrap();
        assert_eq!(foreign.recipient_receipt, None, "only rows addressed to this agent");
        let quoted = queue.message_delivery_report(quoted).unwrap().unwrap();
        assert_eq!(
            quoted.recipient_receipt,
            Some(cas_store::SurfacingSource::TransportClaimed),
            "an envelope without its body confirms nothing"
        );
        assert!(quoted.confirmed_at.is_none());
        let undelivered = queue.message_delivery_report(undelivered).unwrap().unwrap();
        assert_eq!(
            undelivered.recipient_receipt, None,
            "a never-transported row is not confirmed by a quoted envelope"
        );
        assert!(undelivered.confirmed_at.is_none());
    }

    /// cas-7bfa review: the supervisor's own Teams identity is
    /// `supervisor@<session>`. It owns its inbox under both aliases, while a
    /// hex subagent of the supervisor stays a child.
    #[test]
    fn ct2_p1_teams_supervisor_owns_inbox_but_its_subagent_does_not() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_AGENT_ROLE", "supervisor");
        env.set("CAS_AGENT_NAME", "lead-sup");
        env.set("CAS_FACTORY_SESSION", "sess-7bfa");
        env.remove(crate::internal_llm::INTERNAL_LLM_ENV);
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("session.jsonl");
        fs::write(
            &transcript,
            turn_line("t3", chrono::Utc::now() - chrono::Duration::seconds(60), "review cas-x"),
        )
        .unwrap();
        let queue = crate::store::open_prompt_queue_store(dir.path()).unwrap();
        // cas-ca02: two status messages arrive at one busy supervisor tool
        // boundary. Both fit an empty block; only one fits this block.
        let bodies = [
            format!("Candidate already prepared: {}", "a".repeat(3_000)),
            format!("Final green tip ready for candidate; Cargo released: {}", "b".repeat(3_000)),
        ];
        let ids: Vec<_> = bodies.iter().map(|body| {
            let id = queue.enqueue_with_session("worker-7bfa", "supervisor", body, "sess-7bfa").unwrap();
            assert!(queue.claim_recipient_transport(id, "supervisor").unwrap());
            queue.record_deferred_inbox(id, 0).unwrap();
            id
        }).collect();

        assert!(
            fallback_context(dir.path(), &teams_input(&transcript, "b81f0c2e9d4a7735e")).is_none(),
            "the supervisor's subagent never drains the supervisor's inbox"
        );
        assert_eq!(
            queue.message_delivery_report(ids[0]).unwrap().unwrap().recipient_receipt,
            Some(cas_store::SurfacingSource::TransportClaimed)
        );
        let hook = teams_input(&transcript, "supervisor@sess-7bfa");
        let mail = fallback_context(dir.path(), &hook)
            .expect("supervisor@<session> is the supervisor itself");
        assert!(mail.contains(&bodies[0]), "{mail}");
        assert!(!mail.contains(&bodies[1]), "second row exceeds this block's body budget");
        assert_eq!(
            queue.message_delivery_report(ids[1]).unwrap().unwrap().recipient_receipt,
            Some(cas_store::SurfacingSource::TransportClaimed),
            "an overflow row is not rendered or confirmed"
        );
        let next = fallback_context(dir.path(), &hook)
            .expect("the busy supervisor's next tool boundary must retry overflow without new mail");
        assert!(next.contains(&bodies[1]), "{next}");
        assert!(!next.contains(&bodies[0]), "a rendered row is never repeated");
        assert_eq!(
            queue.message_delivery_report(ids[1]).unwrap().unwrap().recipient_receipt,
            Some(cas_store::SurfacingSource::HookSurfaced)
        );
        assert!(fallback_context(dir.path(), &hook).is_none(), "both rows were rendered once");
    }

    /// cas-7bfa review: a Teams member now reaches the inbox-signal check, so
    /// its served-turn no-op must stay cheap with a large transcript.
    #[test]
    fn teams_served_turn_noop_with_large_transcript_is_under_50ms() {
        let _env = teams_env();
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("session.jsonl");
        let filler = format!(
            "{}\n",
            serde_json::json!({
                "type": "user",
                "uuid": "tool",
                "message": {"content": [{"type": "tool_result", "content": "x".repeat(4096)}]},
            })
        );
        let mut body = turn_line("t-big", chrono::Utc::now(), "continue")
            .replace("\"uuid\":\"t-big\"", "\"uuid\":\"t-big\",\"promptId\":\"p-big\"");
        for _ in 0..512 {
            body.push_str(&filler);
        }
        fs::write(&transcript, body).unwrap();
        let mut input = teams_input(&transcript, "worker-7bfa@sess-7bfa");
        input.prompt_id = Some("p-big".into());
        record_prompt_hook(dir.path(), &input);
        let started = std::time::Instant::now();
        for _ in 0..100 {
            assert!(fallback_context(dir.path(), &input).is_none());
        }
        let average = started.elapsed() / 100;
        eprintln!("teams served-turn average with a 2 MiB transcript: {average:?}");
        assert!(average < std::time::Duration::from_millis(50));
    }

    #[test]
    fn served_turn_noop_is_under_50ms_and_does_not_open_stores() {
        let _env = crate::test_support::TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_ROLE", Some("worker")),
            (crate::internal_llm::INTERNAL_LLM_ENV, None),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("session.jsonl");
        fs::write(
            &transcript,
            r#"{"type":"user","promptId":"p","message":{"content":"continue"}}"#,
        )
        .unwrap();
        let input = HookInput {
            session_id: "s".into(),
            prompt_id: Some("p".into()),
            transcript_path: Some(transcript.to_string_lossy().into_owned()),
            ..Default::default()
        };
        record_prompt_hook(dir.path(), &input);
        let state_file = fs::read_dir(dir.path().join("turn-context"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let modified = fs::metadata(&state_file).unwrap().modified().unwrap();
        let started = std::time::Instant::now();
        for _ in 0..100 {
            assert!(fallback_context(dir.path(), &input).is_none());
        }
        let average = started.elapsed() / 100;
        eprintln!("served-turn recovery average: {average:?} (100 calls)");
        assert!(average < std::time::Duration::from_millis(50));
        assert_eq!(
            fs::metadata(state_file).unwrap().modified().unwrap(),
            modified
        );
        assert!(
            !dir.path().join("cas.db").exists(),
            "no-op must never open a store"
        );
    }
}
