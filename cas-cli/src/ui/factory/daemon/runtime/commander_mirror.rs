//! Capture supervisor terminal input and completed answers in the durable Commander lane.
//! The daemon observes both Claude and Codex transcripts; Codex has no Stop
//! hook, so a hook-only mirror would silently omit its supervisor turns.

use std::cell::RefCell;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use cas_store::PromptQueueStore;

const TRANSCRIPT_TAIL_BYTES: u64 = 4 * 1024 * 1024;
const MAX_REPLY_CHARS: usize = 4_000;
const UNBOUND_CODEX_RETRY: Duration = Duration::from_secs(1);
const MAX_UNBOUND_CODEX_RETRY: Duration = Duration::from_secs(30);
const FINALIZED_RECEIPT_POLL: Duration = Duration::from_secs(30);
/// How far from a turn's start a recorded terminal question may be and still
/// be the question that turn answers (cas-5c89).
const TERMINAL_TURN_MATCH_SECS: i64 = 120;

#[derive(Debug)]
struct CompletedTurn {
    key: String,
    started_at: DateTime<Utc>,
    completed_at: DateTime<Utc>,
    text: String,
    commander_id: Option<i64>,
    /// All consumed envelope hints; the newest independently verified row wins.
    commander_ids: Vec<i64>,
    /// The operator-visible prompt that started the turn, when the transcript
    /// carries it (cas-5c89: matched against recorded terminal turns).
    prompt: Option<String>,
}

struct ClaudeInput {
    key: String,
    at: DateTime<Utc>,
    commander_id: Option<i64>,
    prompt: String,
}

fn timestamp(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.get("timestamp")?.as_str()?)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

fn text_blocks(value: &Value, path: &str, block_type: &str) -> Option<String> {
    let text = value
        .pointer(path)?
        .as_array()?
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some(block_type))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    (!text.trim().is_empty()).then_some(text)
}

/// The provenance marker is only a hint. The queue row is independently
/// checked for a hub-stamped paired-device origin before using its reply_to.
fn commander_marker(text: &str) -> Option<i64> {
    commander_markers(text).into_iter().next()
}

fn commander_markers(text: &str) -> Vec<i64> {
    let mut seen = HashSet::new();
    text.lines().filter_map(|line| {
        let marker = line.strip_prefix("[cas #")?;
        let digits: String = marker.chars().take_while(char::is_ascii_digit).collect();
        marker[digits.len()..].starts_with(" operator ")
            .then(|| digits.parse::<i64>().ok()).flatten()
    }).filter(|id| seen.insert(*id)).collect()
}

fn bounded_reply(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= MAX_REPLY_CHARS {
        return text.to_owned();
    }
    let cut: String = text.chars().take(MAX_REPLY_CHARS - 2).collect();
    let cut = cut.rsplit_once('\n').map(|(head, _)| head).unwrap_or(&cut);
    format!("{cut}\n…")
}

fn transcript_tail(path: &Path) -> std::io::Result<Vec<Value>> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let start = size.saturating_sub(TRANSCRIPT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(TRANSCRIPT_TAIL_BYTES).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(text
        .lines()
        .skip(usize::from(start > 0))
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

/// Read consumed text/image input without treating queue bookkeeping, file
/// attachments or tool results as operator input. Claude can absorb a startup
/// or mid-turn send as queued_command instead of emitting a user record.
fn claude_prompt(value: &Value) -> Option<String> {
    let path = match value.get("type").and_then(Value::as_str) {
        Some("user") => "/message/content",
        Some("attachment")
            if value.pointer("/attachment/type").and_then(Value::as_str) == Some("queued_command") => "/attachment/prompt",
        _ => return None,
    };
    if value.pointer(path).and_then(Value::as_array)
        .is_some_and(|blocks| blocks.iter().any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))) {
        return None;
    }
    value.pointer(path).and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| text_blocks(value, path, "text"))
}

/// A send waiting during startup can arrive through our prompt hook instead
/// of Claude's input queue. Only its incoming Commander envelope starts a
/// reply binding; ordinary hook context never becomes a terminal question.
fn claude_commander_hook(value: &Value) -> Option<String> {
    if value.get("type").and_then(Value::as_str) != Some("attachment")
        || value.pointer("/attachment/type").and_then(Value::as_str) != Some("hook_additional_context")
        || value.pointer("/attachment/hookEvent").and_then(Value::as_str) != Some("UserPromptSubmit") {
        return None;
    }
    let context = value.pointer("/attachment/content")?.as_array()?.iter()
        .filter_map(Value::as_str).collect::<Vec<_>>().join("\n");
    // This header is emitted by factory_inbox::render_rows_with_header.
    // The marker remains a hint: scan_supervisor_replies verifies the row's
    // paired-device origin, factory and recipient before using its reply_to.
    (context.lines().any(|line| line.trim() == "[incoming messages]")
        && commander_marker(&context).is_some()).then_some(context)
}

fn human_provenance(value: &Value) -> bool {
    if ["isMeta", "isSidechain", "isCompactSummary"].iter()
        .any(|key| value.get(key).and_then(Value::as_bool) == Some(true)) {
        return false;
    }
    // Old harness records omit provenance. Explicit machine provenance always
    // wins, including a perfectly ordinary-looking injected message body.
    for (path, allowed) in [
        ("/origin/kind", &["human", "operator", "user"][..]),
        ("/promptSource", &["typed", "queued"][..]),
        ("/turnOrigin", &["human", "operator", "user"][..]),
        ("/userType", &["external"][..]),
    ] {
        if value.pointer(path).and_then(Value::as_str)
            .is_some_and(|kind| !allowed.contains(&kind)) {
            return false;
        }
    }
    ["machine_prompt_provenance", "machinePromptProvenance"].iter()
        .all(|key| value.get(key).is_none_or(Value::is_null))
}

struct TerminalInput {
    key: String,
    at: DateTime<Utc>,
    prompt: String,
}

/// User input is owed while the assistant is still working. Keep this reader
/// independent of completed_turns, which only settles the reply half.
fn terminal_inputs(values: &[Value], cli: cas_mux::SupervisorCli) -> Vec<TerminalInput> {
    let mut inputs = Vec::new();
    let mut codex_turn: Option<String> = None;
    let mut seen = HashSet::new();
    for value in values {
        let Some(at) = timestamp(value) else { continue };
        let (key, prompt) = match cli {
            cas_mux::SupervisorCli::Claude => {
                let Some(key) = value.get("promptId").or_else(|| value.get("uuid"))
                    .and_then(Value::as_str).filter(|id| !id.is_empty()) else { continue };
                let Some(prompt) = claude_prompt(value) else { continue };
                (key.to_owned(), prompt)
            }
            cas_mux::SupervisorCli::Codex => {
                if value.get("type").and_then(Value::as_str) == Some("event_msg")
                    && matches!(value.pointer("/payload/type").and_then(Value::as_str), Some("task_started" | "turn_started")) {
                    codex_turn = value.pointer("/payload/turn_id").and_then(Value::as_str).map(str::to_owned);
                }
                if value.get("type").and_then(Value::as_str) != Some("response_item")
                    || value.pointer("/payload/type").and_then(Value::as_str) != Some("message")
                    || value.pointer("/payload/role").and_then(Value::as_str) != Some("user") {
                    continue;
                }
                let Some(turn) = codex_turn.as_deref() else { continue };
                let Some(prompt) = text_blocks(value, "/payload/content", "input_text") else { continue };
                (format!("{turn}:{}", at.to_rfc3339()), prompt)
            }
            _ => continue,
        };
        let prompt = prompt.trim().to_owned();
        if prompt.is_empty() || !human_provenance(value)
            || value.get("payload").is_some_and(|payload| !human_provenance(payload))
            || value.get("attachment").is_some_and(|attachment| !human_provenance(attachment))
            || crate::hooks::handlers::handlers_middle::is_harness_prompt(&prompt) {
            continue;
        }
        // Claude can append a second text-only projection of a text/image
        // record, with another UUID but the identical input timestamp.
        if seen.insert((at, prompt.clone())) {
            inputs.push(TerminalInput { key, at, prompt });
        }
    }
    inputs
}

fn completed_turns(values: &[Value], cli: cas_mux::SupervisorCli) -> Vec<CompletedTurn> {
    let mut turns = Vec::new();
    let mut started: Option<(String, DateTime<Utc>, Option<i64>)> = None;
    let mut started_prompt: Option<String> = None;
    let mut final_text: Option<String> = None;
    let mut claude_inputs: Vec<ClaudeInput> = Vec::new();
    let mut claude_status: Option<ClaudeInput> = None;
    for value in values {
        let Some(at) = timestamp(value) else { continue };
        match cli {
            cas_mux::SupervisorCli::Claude => {
                if let Some(prompt) = claude_prompt(value).or_else(|| claude_commander_hook(value))
                {
                    let key = value
                        .get("promptId")
                        .or_else(|| value.get("uuid"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    if !key.is_empty() {
                        let human = human_provenance(value)
                            && value.get("attachment").is_none_or(human_provenance);
                        let ids = if human { commander_markers(&prompt) } else { Vec::new() };
                        if ids.is_empty() {
                            let input = ClaudeInput { key, at, commander_id: None, prompt };
                            if !human || crate::hooks::handlers::handlers_middle::is_harness_prompt(&input.prompt) {
                                // A standalone worker wake still mirrors as status,
                                // but a notification cannot take an operator's turn.
                                if claude_inputs.is_empty() {
                                    claude_status = Some(input);
                                    final_text = None;
                                }
                            } else if !claude_inputs.iter().any(|prior| prior.key == input.key
                                || (prior.at == input.at && prior.prompt == input.prompt)) {
                                claude_inputs.push(input);
                                final_text = None;
                            }
                        } else {
                            for id in &ids {
                                if !claude_inputs.iter().any(|prior| prior.commander_id == Some(*id)) {
                                    claude_inputs.push(ClaudeInput {
                                        key: if ids.len() == 1 { key.clone() } else { format!("{key}:operator:{id}") },
                                        at, commander_id: Some(*id), prompt: prompt.clone(),
                                    });
                                    final_text = None;
                                }
                            }
                        }
                    }
                }
                if value.get("type").and_then(Value::as_str) == Some("assistant")
                    && value.pointer("/message/role").and_then(Value::as_str) == Some("assistant")
                    && value.get("isSidechain").and_then(Value::as_bool) != Some(true)
                {
                    if let Some(text) = text_blocks(value, "/message/content", "text") {
                        final_text = Some(text);
                    }
                }
                let finished = (value.get("type").and_then(Value::as_str) == Some("assistant")
                    && value.pointer("/message/stop_reason").and_then(Value::as_str) == Some("end_turn")
                    && value.get("isSidechain").and_then(Value::as_bool) != Some(true))
                    || (value.get("type").and_then(Value::as_str) == Some("system")
                        && value.get("subtype").and_then(Value::as_str) == Some("turn_duration"));
                if finished && let Some(text) = final_text.take()
                {
                    let status_only = claude_inputs.is_empty();
                    if status_only {
                        claude_inputs.extend(claude_status.take());
                    }
                    // A terminal turn produces one visible answer. The latest
                    // verified Commander send supplies its durable thread;
                    // receipt scanning still marks every consumed send delivered.
                    if let Some(input) = claude_inputs.first() {
                        turns.push(CompletedTurn {
                            key: input.key.clone(), started_at: input.at, completed_at: at,
                            text, commander_id: claude_inputs.iter().rev().find_map(|input| input.commander_id),
                            commander_ids: claude_inputs.iter().filter_map(|input| input.commander_id).collect(),
                            prompt: (!status_only).then(|| input.prompt.clone()),
                        });
                    }
                    claude_inputs.clear();
                    claude_status = None;
                }
            }
            cas_mux::SupervisorCli::Codex => {
                let outer = value.get("type").and_then(Value::as_str);
                let payload = value.get("payload");
                let kind = payload.and_then(|p| p.get("type")).and_then(Value::as_str);
                if outer == Some("event_msg")
                    && matches!(kind, Some("task_started" | "turn_started"))
                {
                    if let Some(id) = payload
                        .and_then(|p| p.get("turn_id"))
                        .and_then(Value::as_str)
                    {
                        started = Some((id.to_owned(), at, None));
                        started_prompt = None;
                        final_text = None;
                    }
                }
                if outer == Some("response_item") && kind == Some("message") {
                    let role = payload.and_then(|p| p.get("role")).and_then(Value::as_str);
                    if role == Some("user") {
                        if let Some(prompt) = text_blocks(value, "/payload/content", "input_text") {
                            if let Some((_, _, commander_id)) = started.as_mut() {
                                *commander_id = commander_marker(&prompt).or(*commander_id);
                                if started_prompt.is_none() {
                                    started_prompt = Some(prompt);
                                }
                            }
                        }
                    } else if role == Some("assistant")
                        && payload.and_then(|p| p.get("phase")).and_then(Value::as_str)
                            == Some("final_answer")
                    {
                        final_text = text_blocks(value, "/payload/content", "output_text");
                    }
                }
                if outer == Some("event_msg")
                    && kind == Some("item_completed")
                    && value.pointer("/payload/item/type").and_then(Value::as_str)
                        == Some("AgentMessage")
                    && value.pointer("/payload/item/phase").and_then(Value::as_str)
                        == Some("final_answer")
                {
                    final_text = text_blocks(value, "/payload/item/content", "Text");
                }
                if outer == Some("event_msg")
                    && matches!(kind, Some("task_complete" | "turn_completed"))
                    && final_text.is_some()
                {
                    if let (Some((key, begun, commander_id)), Some(text)) =
                        (started.take(), final_text.take())
                    {
                        let completed_id = payload
                            .and_then(|p| p.get("turn_id"))
                            .and_then(Value::as_str);
                        if completed_id.is_none_or(|id| id == key) {
                            turns.push(CompletedTurn {
                                key,
                                started_at: begun,
                                completed_at: at,
                                text,
                                commander_id,
                                commander_ids: commander_id.into_iter().collect(),
                                prompt: started_prompt.take(),
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }
    turns
}

fn activation_time(
    root: &Path,
    session: &str,
    first_pair_at: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let directory = root.join("commander-mirror");
    std::fs::create_dir_all(&directory).ok()?;
    let hash = format!("{:x}", Sha256::digest(session.as_bytes()));
    let marker = directory.join(format!("{hash}.started"));
    // First rollout should pick up a Commander turn that completed just
    // before this poll, without flooding the phone with an old session's
    // transcript. Later daemon restarts reuse the durable start watermark.
    let initial = first_pair_at.max(Utc::now() - chrono::Duration::minutes(2));
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        let _ = file.write_all(initial.to_rfc3339().as_bytes());
    }
    std::fs::read_to_string(marker)
        .ok()
        .and_then(|at| DateTime::parse_from_rfc3339(&at).ok())
        .map(|at| at.with_timezone(&Utc))
        .or(Some(initial))
}

/// An explicit opt-out ends the capture window. Persist the resume floor so
/// neither daemon restarts nor terminal-answer backfill expose disabled turns.
fn resume_floor(
    root: &Path,
    session: &str,
    enabled: bool,
    policy_floor: Option<DateTime<Utc>>,
) -> std::io::Result<Option<DateTime<Utc>>> {
    let directory = root.join("commander-mirror");
    std::fs::create_dir_all(&directory)?;
    let hash = format!("{:x}", Sha256::digest(session.as_bytes()));
    let disabled = directory.join(format!("{hash}.disabled"));
    let resumed = directory.join(format!("{hash}.resumed"));
    if !enabled {
        std::fs::write(disabled, b"disabled\n")?;
        return Ok(None);
    }
    if disabled.exists() {
        let disabled_at = DateTime::<Utc>::from(std::fs::metadata(&disabled)?.modified()?);
        // A CLI resume already opened the window. Don't move it forward to
        // this poll and discard replies completed since that config save.
        // A later raw-file opt-out still needs the observed-poll fallback.
        let floor = policy_floor
            .filter(|at| *at >= disabled_at)
            .unwrap_or_else(Utc::now);
        std::fs::write(&resumed, floor.to_rfc3339())?;
        std::fs::remove_file(disabled)?;
    }
    match std::fs::read_to_string(resumed) {
        Ok(at) => DateTime::parse_from_rfc3339(&at)
            .map(|at| Some(at.with_timezone(&Utc)))
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

type TranscriptSource = (String, cas_mux::SupervisorCli, Option<String>);

#[derive(Default)]
pub(crate) struct MirrorScanState {
    cursors: HashMap<String, TranscriptCursor>,
    resolution_misses: HashMap<String, (TranscriptSource, Option<i64>, Instant, Duration)>,
    codex_ownership: RefCell<HashMap<PathBuf, RolloutOwnership>>,
    receipt_next_poll: HashMap<i64, Instant>,
    published_inputs: HashSet<i64>,
    pub(super) operator_messages: Vec<crate::ui::factory::ConversationHistoryMessage>,
    pub(super) pending_operator_receipts: HashSet<i64>,
    pub(super) finished_operator_receipts: Vec<i64>,
    #[cfg(test)]
    transcript_reads: usize,
    #[cfg(test)]
    resolution_scans: usize,
    #[cfg(test)]
    receipt_reads: usize,
    #[cfg(test)]
    poll_clock: Option<std::time::Instant>,
}

struct TranscriptCursor {
    source: TranscriptSource,
    path: PathBuf,
    length: u64,
    modified: Option<SystemTime>,
}

#[derive(Default)]
struct RolloutOwnership {
    offset: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    ids: HashSet<i64>,
    #[cfg(test)]
    bytes_read: u64,
}

/// Incrementally index envelope IDs, then verify against the current recipient
/// and startup boundary. A miss never rereads unchanged tool output; ownership
/// can still precede the bounded reply tail by hours.
fn codex_rollout_owns_recipient(
    path: &Path,
    queue: &dyn PromptQueueStore,
    session: &str,
    recipient: &str,
    startup_id: Option<i64>,
    cache: &RefCell<HashMap<PathBuf, RolloutOwnership>>,
) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else { return false };
    let Ok(metadata) = file.metadata() else { return false };
    let mut cache = cache.borrow_mut();
    // The resolver considers at most 200 recent rollouts. Bound historical
    // indexes as well when long-lived daemons move through many sessions.
    if cache.len() > 512 { cache.clear(); }
    let cursor = cache.entry(path.to_owned()).or_default();
    let created = metadata.created().ok();
    let modified = metadata.modified().ok();
    if cursor.created != created || metadata.len() < cursor.offset
        || (metadata.len() == cursor.offset && cursor.modified != modified) {
        *cursor = RolloutOwnership::default();
    }
    if file.seek(SeekFrom::Start(cursor.offset)).is_err() { return false; }
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    loop {
        line.clear();
        let Ok(bytes) = reader.read_line(&mut line) else { break };
        if bytes == 0 || !line.ends_with('\n') { break; }
        cursor.offset += bytes as u64;
        #[cfg(test)] { cursor.bytes_read += bytes as u64; }
        if !line.contains("[cas #") { continue; }
        let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
        if value.get("type").and_then(Value::as_str) != Some("response_item")
            || value.pointer("/payload/type").and_then(Value::as_str) != Some("message")
            || value.pointer("/payload/role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let Some(text) = text_blocks(&value, "/payload/content", "input_text") else { continue };
        let first = text.lines().next().unwrap_or("");
        let marker = first.strip_prefix("[cas #").or_else(|| {
            first.strip_prefix("Message from ")?.split_once(": [cas #").map(|(_, marker)| marker)
        });
        if let Some(id) = marker.and_then(|marker| marker.chars().take_while(char::is_ascii_digit).collect::<String>().parse::<i64>().ok()) {
            cursor.ids.insert(id);
        }
    }
    cursor.created = created;
    cursor.modified = modified;
    cursor.ids.iter().any(|id| {
        if startup_id.is_some_and(|floor| *id < floor) { return false; }
        if startup_id == Some(*id) { return true; }
        queue.queued_prompt(*id).ok().flatten().is_some_and(|row| {
            row.factory_session.as_deref() == Some(session)
                && (row.target == recipient || row.target == "supervisor")
        })
    })
}

pub(super) fn scan_supervisor_replies(
    root: &Path,
    project_path: &Path,
    session: &str,
    queue: &dyn PromptQueueStore,
    agents: &[cas_types::Agent],
    state: &mut MirrorScanState,
) {
    let now = Instant::now();
    #[cfg(test)]
    let now = state.poll_clock.unwrap_or(now);
    // Inbox reads and explicit answers can settle a message in another
    // process. Publish those durable changes to connected devices as well as
    // history pages, independently of optional assistant-reply mirroring.
    for id in std::mem::take(&mut state.pending_operator_receipts) {
        if state.receipt_next_poll.get(&id).is_some_and(|next| now < *next) {
            state.pending_operator_receipts.insert(id);
            continue;
        }
        state.receipt_next_poll.remove(&id);
        #[cfg(test)]
        { state.receipt_reads += 1; }
        let row = match queue.queued_prompt(id) {
            Ok(row) => row,
            Err(_) => {
                state.pending_operator_receipts.insert(id);
                continue;
            }
        };
        let finalized = row.as_ref().is_some_and(|row| row.processed_at.is_some());
        let message = row.filter(|row| row.factory_session.as_deref() == Some(session))
            .and_then(|row| super::queue_and_events::commander_history_send(queue, row, session));
        match message {
            Some(message) if message.state == "acknowledged" => {
                // Old cleanup-only rows have no active transport left to
                // watch every second. Keep watching slowly: a later genuine
                // recipient receipt must still reach connected devices.
                if finalized {
                    state.receipt_next_poll.insert(id, now + FINALIZED_RECEIPT_POLL);
                }
                state.pending_operator_receipts.insert(id);
            }
            Some(message) => {
                state.operator_messages.push(message);
                state.finished_operator_receipts.push(id);
            }
            None => state.finished_operator_receipts.push(id),
        }
    }
    let Ok(config) = crate::config::Config::load(root) else {
        return;
    };
    if !config.factory().commander_mirror_enabled {
        state.cursors.clear();
        state.resolution_misses.clear();
        let _ = resume_floor(root, session, false, None);
        return;
    }
    let supervisors = agents
        .iter()
        .filter(|agent| {
            agent.role == cas_types::AgentRole::Supervisor
                && agent.factory_session.as_deref() == Some(session)
        })
        .collect::<Vec<_>>();
    if supervisors.is_empty() {
        return;
    }
    // P3: the local operator lane exists before its reader first sends a
    // message. Enrollment gates remote delivery, not local capture. Final
    // replies remain owed even if registration became Idle/Stale/Shutdown.
    let first_operator_at = queue
        .first_operator_activity_at(session)
        .ok()
        .flatten()
        .unwrap_or_else(|| Utc::now() - chrono::Duration::minutes(2));
    // Config saves persist this project-wide boundary even when the daemon
    // never observed false. A pending disable fails closed during the write.
    let Ok(policy_floor) = crate::config::commander_mirror_resume_floor(root) else {
        return;
    };
    let Ok(local_floor) = resume_floor(root, session, true, policy_floor) else {
        return;
    };
    let resumed_since = local_floor.into_iter().chain(policy_floor).max();
    let Some(active_since) = activation_time(root, session, first_operator_at) else {
        return;
    };
    state
        .cursors
        .retain(|id, _| supervisors.iter().any(|agent| &agent.id == id));
    state.resolution_misses.retain(|id, _| supervisors.iter().any(|agent| &agent.id == id));
    for agent in supervisors {
        // Supervisor registration persists its own harness under
        // `supervisor_cli`; `worker_cli` names the default child harness.
        let cli = agent
            .metadata
            .get("supervisor_cli")
            .and_then(|value| value.parse::<cas_mux::SupervisorCli>().ok())
            .unwrap_or(cas_mux::SupervisorCli::Claude);
        let session_id = agent.cc_session_id.as_deref().unwrap_or(&agent.id);
        let account = agent.metadata.get("supervisor_account_dir").cloned();
        let source = (session_id.to_owned(), cli, account.clone());
        // Keep resolution out of the steady-state poll as well: scan-based
        // harness resolvers can themselves read transcript metadata. /clear,
        // account/harness changes, and a removed path invalidate this cache.
        let cached = state
            .cursors
            .get(&agent.id)
            .filter(|cursor| cursor.source == source && cursor.path.is_file());
        let unbound_codex = cli == cas_mux::SupervisorCli::Codex && session_id.starts_with("codex-");
        let startup_id = if unbound_codex && cached.is_none() {
            queue.latest_from_source_to_targets(
                super::queue_and_events::SUPERVISOR_STARTUP_SOURCE,
                &[&agent.name], Some(session),
            ).ok().flatten().filter(|row| matches!(row.origin, Some(cas_store::QueueOrigin::Daemon))).map(|row| row.id)
        } else { None };
        let miss_held = state.resolution_misses.get(&agent.id).is_some_and(|(prior, floor, next, _)| {
            prior == &source && *floor == startup_id && now < *next
        });
        let path = cached
            .map(|cursor| cursor.path.clone())
            .or_else(|| {
                if miss_held { return None; }
                #[cfg(test)]
                { state.resolution_scans += 1; }
                // cas-4735: eagerly registered factory Codex ids are synthetic.
                // Never cache a same-cwd guess before this pane's rollout exists.
                if unbound_codex {
                    return crate::mcp::tools::service::factory_ops::resolve_codex_mirror_transcript(
                        project_path.to_str()?, account.as_deref(),
                        &|path| codex_rollout_owns_recipient(path, queue, session, &agent.name, startup_id, &state.codex_ownership),
                    );
                }
                crate::mcp::tools::service::factory_ops::resolve_worker_transcript_path_for_account(
                    project_path.to_str(),
                    session_id,
                    cli,
                    account.as_deref(),
                )
            });
        let Some(path) = path else {
            state.cursors.remove(&agent.id);
            if unbound_codex && !miss_held {
                let delay = state.resolution_misses.get(&agent.id)
                    .filter(|(prior, floor, _, _)| prior == &source && *floor == startup_id)
                    .map(|(_, _, _, delay)| (*delay * 2).min(MAX_UNBOUND_CODEX_RETRY))
                    .unwrap_or(UNBOUND_CODEX_RETRY);
                state.resolution_misses.insert(agent.id.clone(), (source, startup_id, now + delay, delay));
            }
            continue;
        };
        state.resolution_misses.remove(&agent.id);
        let Ok(metadata) = std::fs::metadata(&path) else {
            state.cursors.remove(&agent.id);
            continue;
        };
        let length = metadata.len();
        let modified = metadata.modified().ok();
        if cached.is_some_and(|cursor| {
            cursor.path == path
                && cursor.length == length
                && cursor.modified == modified
                && modified.is_some()
        }) {
            continue;
        }
        #[cfg(test)]
        {
            state.transcript_reads += 1;
        }
        let Ok(values) = transcript_tail(&path) else {
            state.cursors.remove(&agent.id);
            continue;
        };
        let mut written = true;
        for input in terminal_inputs(&values, cli) {
            if input.at < active_since || resumed_since.is_some_and(|floor| input.at < floor) {
                continue;
            }
            match queue.record_terminal_transcript_turn(session, session_id, &input.key, &input.prompt, input.at) {
                Ok(id) if !state.published_inputs.contains(&id) => {
                    // Hook-created rows also need their first live broadcast.
                    // Read on this job's store lane, just like QueueStamp.
                    match queue.queued_prompt(id).map(|row| row.and_then(|row|
                        super::queue_and_events::commander_history_send(queue, row, session))) {
                        Ok(Some(message)) => {
                            state.operator_messages.push(message);
                            state.published_inputs.insert(id);
                        }
                        _ => written = false,
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    written = false;
                    tracing::warn!(%error, "terminal Commander input capture failed");
                }
            }
        }
        for turn in completed_turns(&values, cli) {
            if resumed_since.is_some_and(|floor| turn.completed_at < floor) {
                continue;
            }
            // cas-5c89: the answer to a question the operator typed in the
            // terminal threads under that question, and is mirrored even if it
            // completed before mirroring started; other turns are not
            // backfilled, so first activation never floods an old transcript.
            let terminal_question = turn.prompt.as_deref()
                // cas-8737: harness envelopes are never operator questions,
                // and skipping them keeps the per-poll lookups to real ones.
                .filter(|prompt| {
                    !crate::hooks::handlers::handlers_middle::is_harness_prompt(prompt)
                })
                .and_then(|prompt| {
                    queue
                        .terminal_operator_turn_near(
                            session,
                            prompt,
                            turn.started_at,
                            TERMINAL_TURN_MATCH_SECS,
                        )
                        .ok()
                        .flatten()
                });
            if turn.completed_at < active_since && terminal_question.is_none() {
                continue;
            }
            let prior = turn
                .commander_ids.iter().rev()
                .filter_map(|id| queue.queued_prompt(*id).ok().flatten())
                .find(|row| {
                    row.factory_session.as_deref() == Some(session)
                        && row
                            .origin
                            .as_ref()
                            .and_then(cas_store::QueueOrigin::verified_device_id)
                            .is_some()
                        && row.source.starts_with("commander:")
                        && (row.target == "supervisor" || row.target == agent.name)
                });
            let device = prior
                .as_ref()
                .and_then(|row| {
                    row.origin
                        .as_ref()
                        .and_then(cas_store::QueueOrigin::verified_device_id)
                })
                .unwrap_or("*");
            let text = bounded_reply(
                &crate::hooks::handlers::handlers_events::message_display::redact_secrets(
                    turn.text,
                )
                .0,
            );
            if text.is_empty() {
                continue;
            }
            let summary: String = text
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(120)
                .collect();
            let kind = if prior.is_some() || terminal_question.is_some() {
                crate::ui::factory::OperatorTurnKind::Answer
            } else {
                crate::ui::factory::OperatorTurnKind::Status
            };
            let payload = crate::ui::factory::OperatorReplyPayload {
                schema_version: 2,
                reply_to: prior.as_ref().map(|row| row.id).or(terminal_question),
                message: text,
                summary: summary.clone(),
                device_id: device.to_owned(),
                operator_label: None,
                kind,
                options: Vec::new(),
                attachments: Vec::new(),
                reply_to_session: None,
            };
            let Ok(payload) = serde_json::to_string(&payload) else {
                continue;
            };
            let key = format!("commander-mirror:{}:{}:{}", session, agent.id, turn.key);
            if let Err(error) = queue.mirror_supervisor_turn(
                session,
                &key,
                turn.started_at,
                turn.completed_at,
                &payload,
                &summary,
                device,
                kind.as_str(),
            ) {
                written = false;
                tracing::warn!(%error, "supervisor Commander mirror queue write failed");
            }
        }
        // Only acknowledge a snapshot once its rows are durable. A failed
        // queue write retries on the next poll, with store dedupe intact.
        if written {
            state.cursors.insert(
                agent.id.clone(),
                TranscriptCursor {
                    source,
                    path,
                    length,
                    modified,
                },
            );
        } else {
            state.cursors.remove(&agent.id);
        }
    }
}

#[cfg(test)]
fn mirror_supervisor_replies(
    root: &Path,
    project_path: &Path,
    session: &str,
    queue: &dyn PromptQueueStore,
) {
    let agents = crate::store::open_agent_store(root)
        .ok()
        .and_then(|store| store.list(None).ok())
        .unwrap_or_default();
    scan_supervisor_replies(
        root,
        project_path,
        session,
        queue,
        &agents,
        &mut MirrorScanState::default(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::{AgentStore, OperatorStamp, SqliteAgentStore, SqlitePromptQueueStore};
    use cas_types::{Agent, AgentRole};

    fn codex_turn(prompt: &str) -> Vec<Value> {
        vec![
            serde_json::json!({"timestamp":"2026-09-23T12:00:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-1"}}),
            serde_json::json!({"timestamp":"2026-09-23T12:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":prompt}]}}),
            serde_json::json!({"timestamp":"2026-09-23T12:00:02Z","type":"response_item","payload":{"type":"function_call_output","output":"secret tool output"}}),
            serde_json::json!({"timestamp":"2026-09-23T12:00:03Z","type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"**Ready**\n- Next step is clear."}]}}),
            serde_json::json!({"timestamp":"2026-09-23T12:00:04Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"turn-1"}}),
        ]
    }

    #[test]
    fn codex_final_excludes_tool_output_and_keeps_markdown_and_reply_id() {
        let turns = completed_turns(
            &codex_turn("[cas #42 operator Alice@phone verified 0s first] Hello"),
            cas_mux::SupervisorCli::Codex,
        );
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].text, "**Ready**\n- Next step is clear.");
        assert_eq!(turns[0].commander_id, Some(42));
    }

    #[test]
    fn claude_only_mirrors_completed_final_text() {
        let values = vec![
            serde_json::json!({"timestamp":"2026-09-23T12:00:00Z","type":"user","uuid":"prompt-1","message":{"content":"status"}}),
            serde_json::json!({"timestamp":"2026-09-23T12:00:00Z","type":"user","uuid":"text-projection","message":{"content":"status"}}),
            serde_json::json!({"timestamp":"2026-09-23T12:00:01Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Working"},{"type":"tool_use","input":{"secret":"hidden"}}],"stop_reason":"tool_use"}}),
            serde_json::json!({"timestamp":"2026-09-23T12:00:01Z","type":"attachment","uuid":"second-send","attachment":{"type":"queued_command","origin":{"kind":"human"},"prompt":"[cas #42 operator Alice@phone verified 0s first] second question"}}),
            serde_json::json!({"timestamp":"2026-09-23T12:00:01Z","type":"attachment","uuid":"hook-batch","attachment":{"type":"hook_additional_context","hookEvent":"UserPromptSubmit","content":["[incoming messages]\n[cas #42 operator Alice@phone verified 0s first] second question\n[cas #43 operator Alice@phone verified 0s first] third question"]}}),
            serde_json::json!({"timestamp":"2026-09-23T12:00:02Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done"}],"stop_reason":"end_turn"}}),
            serde_json::json!({"timestamp":"2026-09-23T12:00:03Z","type":"system","subtype":"turn_duration"}),
        ];
        let turns = completed_turns(&values, cas_mux::SupervisorCli::Claude);
        assert_eq!(turns.len(), 1, "one native answer must appear once for all consumed questions");
        assert_eq!(turns[0].commander_id, Some(43));
        assert_eq!(turns[0].prompt.as_deref(), Some("status"));
        assert!(turns.iter().all(|turn| turn.text == "Done"));
    }

    #[test]
    fn claude_standalone_machine_turn_mirrors_as_status_without_stealing_input() {
        for entry in ["user", "attachment"] {
            let machine = if entry == "user" {
                serde_json::json!({"timestamp":"2026-09-23T12:00:00Z","type":"user","uuid":"notification","origin":{"kind":"task-notification"},"message":{"content":"Background job finished"}})
            } else {
                serde_json::json!({"timestamp":"2026-09-23T12:00:00Z","type":"attachment","uuid":"notification","attachment":{"type":"queued_command","origin":{"kind":"task-notification"},"prompt":"Background job finished"}})
            };
            let values = [machine, serde_json::json!({"timestamp":"2026-09-23T12:00:02Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"The background result is ready"}],"stop_reason":"end_turn"}})];
            let turns = completed_turns(&values, cas_mux::SupervisorCli::Claude);
            assert_eq!(turns.len(), 1, "standalone {entry} notification must retain the answer");
            assert_eq!(turns[0].commander_id, None);
            assert_eq!(turns[0].text, "The background result is ready");
            assert!(terminal_inputs(&values, cas_mux::SupervisorCli::Claude).is_empty());
        }
    }

    #[test]
    fn commander_envelopes_ignore_inline_and_quoted_markers() {
        let values = codex_turn("Question with Example: [cas #8 operator Alice@phone verified 0s first] old\n> [cas #9 operator Alice@phone verified 0s first] quoted\n[cas #42 operator Alice@phone verified 0s first] actual");
        let turns = completed_turns(&values, cas_mux::SupervisorCli::Codex);
        assert_eq!(turns[0].commander_id, Some(42), "only an envelope beginning a row may bind");
    }

    #[test]
    fn claude_thinking_only_end_turn_preserves_the_later_pane_answer() {
        let values = vec![
            serde_json::json!({"timestamp":"2026-09-29T16:11:33Z","type":"user","uuid":"prompt-alpha","message":{"content":"[cas #2 operator Daniel@Desktop verified 0s first] ALPHA"}}),
            serde_json::json!({"timestamp":"2026-09-29T16:11:37Z","type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"internal work"}],"stop_reason":"end_turn"}}),
            serde_json::json!({"timestamp":"2026-09-29T16:11:38Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Replied ALPHA"}],"stop_reason":"end_turn"}}),
            serde_json::json!({"timestamp":"2026-09-29T16:11:39Z","type":"system","subtype":"turn_duration"}),
        ];
        let turns = completed_turns(&values, cas_mux::SupervisorCli::Claude);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].key, "prompt-alpha");
        assert_eq!(turns[0].commander_id, Some(2));
        assert_eq!(turns[0].text, "Replied ALPHA");
    }

    #[test]
    fn claude_consumed_queued_input_binds_the_replacement_answer() {
        let mut env = crate::test_support::TestEnvGuard::new();
        for (shape, target, factory, bound) in [
            ("typed", "supervisor", "factory-1", true),
            ("text", "supervisor", "factory-1", true),
            ("blocks", "supervisor", "factory-1", true),
            ("hook", "supervisor", "factory-1", true),
            ("hook", "another-supervisor", "factory-1", false),
            ("hook", "supervisor", "another-factory", false),
            ("text", "another-supervisor", "factory-1", false),
            ("text", "supervisor", "another-factory", false),
        ] {
            let (temp, root, queue) = terminal_session(vec![]);
            let account = temp.path().join("account");
            env.set("CLAUDE_CONFIG_DIR", &account);
            let agents = SqliteAgentStore::open(&root).unwrap();
            let mut supervisor = agents.get("supervisor-id").unwrap();
            supervisor.cc_session_id = Some("replacement".into());
            supervisor.metadata.insert("supervisor_cli".into(), "claude".into());
            agents.update(&supervisor).unwrap();
            let message = queue.enqueue_operator_message(
                "commander:Alice@phone", target, "reply after replacement", Some(factory),
                None, None, false, None,
                &OperatorStamp {
                    operator: "Alice".into(), device_id: "phone".into(), device_label: "phone".into(),
                    scopes: vec!["message:send".into()], verified: true,
                },
            ).unwrap();
            let prompt = format!("<pasted_content id=\"4582\">\n[cas #{} operator Alice@phone verified 2s first]\nreply after replacement\n</pasted_content id=\"4582\">", message.id());
            let now = Utc::now() + chrono::Duration::seconds(1);
            let at = |offset| (now + chrono::Duration::seconds(offset)).to_rfc3339();
            let directory = account.join("projects").join(temp.path().to_string_lossy().replace(['/', '.'], "-"));
            std::fs::create_dir_all(&directory).unwrap();
            let path = directory.join("replacement.jsonl");
            let values = [
                serde_json::json!({"timestamp":at(0),"type":"user","uuid":"startup","message":{"content":"[cas #0 agent-authored 0s first] startup context"}}),
                serde_json::json!({"timestamp":at(1),"type":"queue-operation","operation":"enqueue","content":prompt}),
                serde_json::json!({"timestamp":at(2),"type":"queue-operation","operation":"dequeue","content":prompt,"commandUuid":"queued","deliveryId":"delivery"}),
                serde_json::json!({"timestamp":at(3),"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Startup ready"}],"stop_reason":"end_turn"}}),
            ];
            std::fs::write(&path, values.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
            let scan = |state: &mut MirrorScanState| scan_supervisor_replies(
                &root, temp.path(), "factory-1", &queue, &agents.list(None).unwrap(), state,
            );
            let mut state = MirrorScanState::default();
            scan(&mut state);
            assert_eq!(operator_replies(&queue)[0].reply_to, None, "queue bookkeeping must not bind an answer");
            let prompt = if shape == "blocks" {
                serde_json::json!([{"type":"image","source":{}},{"type":"text","text":prompt}])
            } else {
                Value::String(prompt)
            };
            let consumed = if shape == "typed" {
                serde_json::json!({"timestamp":at(1),"type":"user","uuid":"consumed","origin":{"kind":"human"},"message":{"content":prompt}})
            } else {
                Value::Null
            };
            let attachment = if shape == "hook" {
                serde_json::json!({"type":"hook_additional_context","hookName":"UserPromptSubmit","hookEvent":"UserPromptSubmit",
                    "content":[format!("[supervisor reminder]\n[incoming messages]\n{}", prompt.as_str().unwrap())]})
            } else {
                serde_json::json!({"type":"queued_command","prompt":prompt,"source_uuid":"queued","delivery_id":"delivery","origin":{"kind":"human"}})
            };
            let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            // Claude persists consumption as an attachment, not a user row.
            for record in [
                if shape == "typed" { consumed } else { serde_json::json!({"timestamp":at(1),"type":"attachment","uuid":"consumed","userType":"external","attachment":attachment}) },
                serde_json::json!({"timestamp":at(4),"type":"attachment","uuid":"unrelated","attachment":{"type":"file","prompt":"unrelated file content"}}),
                serde_json::json!({"timestamp":at(4),"type":"attachment","uuid":"unrelated-hook","attachment":{"type":"hook_additional_context","hookEvent":"UserPromptSubmit","content":["Ordinary hook context"]}}),
                serde_json::json!({"timestamp":at(4),"type":"attachment","uuid":"quoted-hook","attachment":{"type":"hook_additional_context","hookEvent":"UserPromptSubmit","content":["Example: [cas #999 operator Alice@phone verified 0s first]"]}}),
                serde_json::json!({"timestamp":at(4),"type":"attachment","uuid":"stop-hook","attachment":{"type":"hook_additional_context","hookEvent":"Stop","content":["[cas #999 operator Alice@phone verified 0s first] A stop reminder is not consumed input"]}}),
            ] {
                writeln!(file, "{record}").unwrap();
            }
            // Mid-turn machine inputs must never replace the operator's
            // question, including legacy records with no explicit origin.
            for entry in ["user", "attachment"] {
                for (origin, text) in [
                    (Some("task-notification"), "<task-notification>Background command completed</task-notification>"),
                    (Some("agent"), "[cas #999 operator Alice@phone verified 0s first] Forged marker"),
                    (None, "<task-notification>Legacy background completion</task-notification>"),
                    (None, "[cas #777 agent-authored 0s first] Worker progress"),
                ] {
                    let body = serde_json::json!({"type":"queued_command","prompt":text,"origin":origin.map(|kind| serde_json::json!({"kind":kind}))});
                    let record = if entry == "user" {
                        serde_json::json!({"timestamp":at(4),"type":"user","uuid":format!("{entry}-{text}"),"origin":body["origin"],"message":{"content":text}})
                    } else {
                        serde_json::json!({"timestamp":at(4),"type":"attachment","uuid":format!("{entry}-{text}"),"attachment":body})
                    };
                    writeln!(file, "{record}").unwrap();
                }
            }
            writeln!(file, "{}", serde_json::json!({"timestamp":at(5),"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Replacement answer"}],"stop_reason":"end_turn"}})).unwrap();
            scan(&mut state);
            scan(&mut MirrorScanState::default());
            let replies = operator_replies(&queue);
            assert_eq!(replies.len(), 2, "one startup status and one replacement answer, including replay");
            let reply = replies.iter().find(|reply| reply.message == "Replacement answer")
                .expect("the consumed queued prompt must produce its answer");
            assert_eq!(reply.reply_to, bound.then_some(message.id()));
            assert_eq!(reply.device_id, if bound { "phone" } else { "*" });
            assert_eq!(reply.kind, if bound {
                crate::ui::factory::OperatorTurnKind::Answer
            } else {
                crate::ui::factory::OperatorTurnKind::Status
            });
            assert!(queue.conversation_history("factory-1", "phone", None, 50).unwrap().iter()
                .all(|row| row.source != "terminal"), "Commander envelopes must not become duplicate terminal sends");
        }
    }

    #[test]
    fn mirrored_history_is_idempotent_and_deduplicates_matching_explicit_reply() {
        let temp = tempfile::tempdir().unwrap();
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        let stamp = OperatorStamp {
            operator: "Alice".into(),
            device_id: "phone".into(),
            device_label: "phone".into(),
            scopes: vec!["message:send".into()],
            verified: true,
        };
        let message = queue
            .enqueue_operator_message(
                "commander:Alice@phone",
                "supervisor",
                "Hello",
                Some("factory-1"),
                None,
                None,
                false,
                None,
                &stamp,
            )
            .unwrap();
        let turn = completed_turns(
            &codex_turn(&format!(
                "[cas #{} operator Alice@phone verified 0s first] Hello",
                message.id()
            )),
            cas_mux::SupervisorCli::Codex,
        )
        .pop()
        .unwrap();
        let payload = serde_json::to_string(&crate::ui::factory::OperatorReplyPayload {
            schema_version: 2,
            reply_to: Some(message.id()),
            message: turn.text.clone(),
            summary: "Ready".into(),
            device_id: "phone".into(),
            operator_label: None,
            kind: crate::ui::factory::OperatorTurnKind::Answer,
            options: Vec::new(),
            attachments: Vec::new(),
            reply_to_session: None,
        })
        .unwrap();
        assert!(
            queue
                .mirror_supervisor_turn(
                    "factory-1",
                    "turn-1",
                    turn.started_at,
                    turn.completed_at,
                    &payload,
                    "Ready",
                    "phone",
                    "answer"
                )
                .unwrap()
                .is_some()
        );
        assert!(
            queue
                .mirror_supervisor_turn(
                    "factory-1",
                    "turn-1",
                    turn.started_at,
                    turn.completed_at,
                    &payload,
                    "Ready",
                    "phone",
                    "answer"
                )
                .unwrap()
                .is_none()
        );
        let history = queue
            .conversation_history("factory-1", "phone", None, 20)
            .unwrap();
        assert_eq!(
            history
                .iter()
                .filter(|row| row.target == "operator")
                .count(),
            1
        );
        assert_eq!(
            history
                .iter()
                .find(|row| row.target == "operator")
                .unwrap()
                .prompt,
            payload
        );

        // A later turn already sent an explicit operator row while composing.
        let explicit_at = Utc::now();
        queue
            .enqueue_with_session("supervisor", "operator", &payload, "factory-1")
            .unwrap();
        let end = explicit_at + chrono::Duration::seconds(2);
        assert!(
            queue
                .mirror_supervisor_turn(
                    "factory-1",
                    "turn-2",
                    explicit_at - chrono::Duration::seconds(1),
                    end,
                    &payload,
                    "Ready",
                    "phone",
                    "answer"
                )
                .unwrap()
                .is_none()
        );
        let different = payload.replace(
            "**Ready**\\n- Next step is clear.",
            "A separate final answer",
        );
        assert!(
            queue
                .mirror_supervisor_turn(
                    "factory-1",
                    "turn-3",
                    explicit_at - chrono::Duration::seconds(1),
                    end,
                    &different,
                    "separate",
                    "phone",
                    "answer"
                )
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn no_registered_supervisor_does_not_create_a_mirror_marker_or_reply() {
        let temp = tempfile::tempdir().unwrap();
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        mirror_supervisor_replies(temp.path(), temp.path(), "factory-1", &queue);
        assert!(!temp.path().join("commander-mirror").exists());
        assert!(
            queue
                .conversation_history("factory-1", "phone", None, 20)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn daemon_scan_mirrors_supervisor_only_into_commander_history() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        let queue = SqlitePromptQueueStore::open(&root).unwrap();
        queue.init().unwrap();
        let stamp = OperatorStamp {
            operator: "Alice".into(),
            device_id: "phone".into(),
            device_label: "phone".into(),
            scopes: vec!["message:send".into()],
            verified: true,
        };
        queue
            .enqueue_operator_message(
                "commander:Alice@phone",
                "supervisor",
                "status",
                Some("factory-1"),
                None,
                None,
                false,
                None,
                &stamp,
            )
            .unwrap();
        // Pairing activates mirroring. A worker transcript with a final answer
        // must still produce no operator row.
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        let agents = SqliteAgentStore::open(&root).unwrap();
        agents.init().unwrap();
        let account = temp.path().join("account");
        let sessions = account.join("sessions/2026/09/23");
        std::fs::create_dir_all(&sessions).unwrap();
        let rollout = sessions.join("rollout-2026-09-23T12-00-00-test.jsonl");
        let mut lines = vec![
            serde_json::json!({"type":"session_meta","payload":{"cwd":temp.path().to_str().unwrap(),"source":"cli"}}),
        ];
        let now = Utc::now() + chrono::Duration::seconds(1);
        for (index, mut value) in codex_turn("status").into_iter().enumerate() {
            value["timestamp"] = serde_json::Value::String(
                (now + chrono::Duration::seconds(index as i64)).to_rfc3339(),
            );
            lines.push(value);
        }
        std::fs::write(
            &rollout,
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
        let mut worker =
            Agent::new_with_role("worker-id".into(), "worker".into(), AgentRole::Worker);
        worker.factory_session = Some("factory-1".into());
        worker.metadata.insert("worker_cli".into(), "codex".into());
        worker.metadata.insert(
            "worker_account_dir".into(),
            account.to_string_lossy().into_owned(),
        );
        agents.register(&worker).unwrap();
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        assert_eq!(
            queue
                .conversation_history("factory-1", "phone", None, 20)
                .unwrap()
                .iter()
                .filter(|row| row.target == "operator")
                .count(),
            0
        );

        let mut supervisor = Agent::new_with_role(
            "supervisor-id".into(),
            "supervisor".into(),
            AgentRole::Supervisor,
        );
        supervisor.factory_session = Some("factory-1".into());
        supervisor
            .metadata
            .insert("supervisor_cli".into(), "codex".into());
        supervisor.metadata.insert(
            "supervisor_account_dir".into(),
            account.to_string_lossy().into_owned(),
        );
        agents.register(&supervisor).unwrap();
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        let history = queue
            .conversation_history("factory-1", "phone", None, 20)
            .unwrap();
        let replies: Vec<_> = history
            .iter()
            .filter(|row| row.target == "operator")
            .collect();
        assert_eq!(replies.len(), 1);
        let payload: crate::ui::factory::OperatorReplyPayload =
            serde_json::from_str(&replies[0].prompt).unwrap();
        assert_eq!(payload.kind, crate::ui::factory::OperatorTurnKind::Answer);
        assert_eq!(payload.reply_to, history.iter().find(|row| row.source == "terminal").map(|row| row.id));
        assert_eq!(payload.message, "**Ready**\n- Next step is clear.");
        assert_eq!(payload.device_id, "*");
    }

    #[test]
    fn concurrent_daemon_polls_create_one_reply() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let queue = SqlitePromptQueueStore::open(&root).unwrap();
        queue.init().unwrap();
        let now = Utc::now();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let joins: Vec<_> = (0..8)
            .map(|_| {
                let root = root.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let queue = SqlitePromptQueueStore::open(&root).unwrap();
                    barrier.wait();
                    queue.mirror_supervisor_turn(
                        "factory-1", "same-turn", now - chrono::Duration::seconds(2), now,
                        r#"{"schema_version":2,"reply_to":null,"message":"done","summary":"done","device_id":"*","kind":"status","attachments":[]}"#,
                        "done", "*", "status",
                    ).unwrap()
                })
            })
            .collect();
        let created = joins
            .into_iter()
            .map(|join| join.join().unwrap())
            .filter(Option::is_some)
            .count();
        assert_eq!(created, 1);
        assert_eq!(
            queue
                .conversation_history("factory-1", "phone", None, 20)
                .unwrap()
                .len(),
            1
        );
    }

    // -------------------------------------------------------------------
    // cas-5c89: terminal-only operators see the supervisor's answers
    // -------------------------------------------------------------------

    /// One Codex turn with its own id, at `start` + 0..4 s.
    fn codex_turn_at(id: &str, prompt: &str, answer: &str, start: DateTime<Utc>) -> Vec<Value> {
        let at = |offset: i64| (start + chrono::Duration::seconds(offset)).to_rfc3339();
        vec![
            serde_json::json!({"timestamp":at(0),"type":"event_msg","payload":{"type":"task_started","turn_id":id}}),
            serde_json::json!({"timestamp":at(1),"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":prompt}]}}),
            serde_json::json!({"timestamp":at(3),"type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":answer}]}}),
            serde_json::json!({"timestamp":at(4),"type":"event_msg","payload":{"type":"task_complete","turn_id":id}}),
        ]
    }

    /// A Codex supervisor registered in `factory-1` whose rollout holds `turns`.
    fn terminal_session(
        turns: Vec<Value>,
    ) -> (
        tempfile::TempDir,
        std::path::PathBuf,
        SqlitePromptQueueStore,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        let queue = SqlitePromptQueueStore::open(&root).unwrap();
        queue.init().unwrap();
        let account = temp.path().join("account");
        let sessions = account.join("sessions/2026/10/10");
        std::fs::create_dir_all(&sessions).unwrap();
        let mut lines = vec![
            serde_json::json!({"type":"session_meta","payload":{"cwd":temp.path().to_str().unwrap(),"source":"cli"}}),
        ];
        lines.extend(turns);
        std::fs::write(
            sessions.join("rollout-2026-10-10T12-00-00-terminal.jsonl"),
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
        let agents = SqliteAgentStore::open(&root).unwrap();
        agents.init().unwrap();
        let mut supervisor = Agent::new_with_role(
            "supervisor-id".into(),
            "supervisor".into(),
            AgentRole::Supervisor,
        );
        supervisor.factory_session = Some("factory-1".into());
        supervisor
            .metadata
            .insert("supervisor_cli".into(), "codex".into());
        supervisor.metadata.insert(
            "supervisor_account_dir".into(),
            account.to_string_lossy().into_owned(),
        );
        agents.register(&supervisor).unwrap();
        (temp, root, queue)
    }

    fn operator_replies(
        queue: &SqlitePromptQueueStore,
    ) -> Vec<crate::ui::factory::OperatorReplyPayload> {
        queue
            .conversation_history("factory-1", "phone", None, 50)
            .unwrap()
            .iter()
            .filter(|row| row.target == "operator")
            .map(|row| serde_json::from_str(&row.prompt).unwrap())
            .collect()
    }

    #[test]
    fn codex_boot_mirror_waits_for_its_own_rollout_in_a_reused_project() {
        let (temp, root, queue) = terminal_session(vec![]);
        let store = SqliteAgentStore::open(&root).unwrap();
        let mut agent = store.get("supervisor-id").unwrap();
        agent.cc_session_id = Some("codex-supervisor-synthetic".into());
        store.update(&agent).unwrap();
        let stamp = OperatorStamp {
            operator: "Alice".into(), device_id: "phone".into(),
            device_label: "phone".into(), scopes: vec!["message:send".into()], verified: true,
        };
        let enqueue = |session| queue.enqueue_operator_message(
            "commander:Alice@phone", "supervisor", "Boot words", Some(session),
            None, None, false, None, &stamp,
        ).unwrap().id();
        let old = enqueue("factory-1");
        let first = temp.path().join("account/sessions/2026/10/10/rollout-2026-10-10T12-00-00-terminal.jsonl");
        let meta: Value = serde_json::from_str(std::fs::read_to_string(&first).unwrap().lines().next().unwrap()).unwrap();
        let write = |path: &Path, id, key, answer| {
            let mut values = vec![meta.clone()];
            values.extend(codex_turn_at(key, &format!("[cas #{id} operator Alice@phone verified 0s first] Boot words"), answer, Utc::now()));
            std::fs::write(path, values.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
        };
        write(&first, old, "previous", "Previous session answer");
        queue.enqueue_idempotent(
            super::super::queue_and_events::SUPERVISOR_STARTUP_SOURCE,
            "supervisor", "Replacement startup", None, None, None,
            "replacement-startup", Some(&cas_store::QueueOrigin::Daemon),
        ).unwrap();
        let mut state = MirrorScanState::default();
        let scan = |state: &mut MirrorScanState| scan_supervisor_replies(
            &root, temp.path(), "factory-1", &queue, &store.list(None).unwrap(), state,
        );
        // The eager MCP registration exists before this native rollout does.
        scan(&mut state);
        let own = enqueue("factory-1");
        let second = first.with_file_name("rollout-2026-10-10T12-00-01-native.jsonl");
        write(&second, own, "boot", "Boot words preserved");
        let mut file = std::fs::OpenOptions::new().append(true).open(&second).unwrap();
        writeln!(file, "{}", serde_json::json!({"type":"event_msg", "padding":"x".repeat(TRANSCRIPT_TAIL_BYTES as usize)})).unwrap();
        let question = "Terminal question after a long rollout";
        let terminal_id = queue.record_terminal_operator_turn("factory-1", question).unwrap();
        for value in codex_turn_at("later-terminal", question, "Latest terminal answer", Utc::now()) {
            writeln!(file, "{value}").unwrap();
        }
        // Another live pane can keep the prior cwd match newest. Freshness
        // alone must not bind either supervisor to the other pane's rollout.
        std::fs::File::open(&first).unwrap().set_modified(SystemTime::now() + std::time::Duration::from_secs(2)).unwrap();
        let peer = queue.enqueue_operator_message(
            "commander:Alice@phone", "peer-supervisor", "Peer words", Some("factory-1"),
            None, None, false, None, &stamp,
        ).unwrap().id();
        let third = first.with_file_name("rollout-2026-10-10T12-00-02-peer.jsonl");
        write(&third, peer, "peer", "Peer session answer");
        std::fs::File::open(&third).unwrap().set_modified(SystemTime::now() + std::time::Duration::from_secs(3)).unwrap();
        state.poll_clock = Some(std::time::Instant::now() + std::time::Duration::from_secs(1));
        scan(&mut state);
        let replies = operator_replies(&queue);
        assert!(replies.iter().any(|reply| reply.message == "Latest terminal answer"),
            "ownership beyond the tail must still mirror later terminal answers after daemon restart");
        assert_eq!(replies.len(), 1, "another session's answer must never enter this thread");
        assert_eq!(replies[0].reply_to, Some(terminal_id));
        let reads = state.transcript_reads;
        scan(&mut state);
        assert_eq!(state.transcript_reads, reads, "owned unchanged rollout remains cached");
    }

    #[test]
    fn unresolved_codex_resolution_does_not_poll_every_tick() {
        let (temp, root, queue) = terminal_session(vec![]);
        let store = SqliteAgentStore::open(&root).unwrap();
        let mut agent = store.get("supervisor-id").unwrap();
        agent.cc_session_id = Some("codex-unbound".into());
        store.update(&agent).unwrap();
        let now = std::time::Instant::now();
        let mut state = MirrorScanState { poll_clock: Some(now), ..Default::default() };
        let scan = |state: &mut MirrorScanState| scan_supervisor_replies(
            &root, temp.path(), "factory-1", &queue, &store.list(None).unwrap(), state);
        scan(&mut state);
        let bytes = || state.codex_ownership.borrow().values().map(|cursor| cursor.bytes_read).sum::<u64>();
        let initial_bytes = bytes();
        assert!(initial_bytes > 0, "fixture must exercise ownership reads");
        state.poll_clock = Some(now + std::time::Duration::from_secs(1));
        scan(&mut state);
        assert_eq!(state.resolution_scans, 2, "the first retry must bind a new rollout promptly");
        state.poll_clock = Some(now + std::time::Duration::from_secs(2));
        scan(&mut state);
        assert_eq!(state.resolution_scans, 2, "repeated misses must back off");
        state.poll_clock = Some(now + std::time::Duration::from_secs(3));
        scan(&mut state);
        assert_eq!(state.resolution_scans, 3, "misses keep retrying");
        assert_eq!(state.codex_ownership.borrow().values().map(|cursor| cursor.bytes_read).sum::<u64>(), initial_bytes,
            "retries must not reread unchanged rollout bodies");
        let send = queue.enqueue_operator_message(
            "commander:Alice@phone", "supervisor", "New words", Some("factory-1"),
            None, None, false, None, &OperatorStamp {
                operator: "Alice".into(), device_id: "phone".into(), device_label: "phone".into(),
                scopes: vec!["message:send".into()], verified: true,
            }).unwrap();
        let path = temp.path().join("account/sessions/2026/10/10/rollout-2026-10-10T12-00-00-terminal.jsonl");
        let append = codex_turn_at("new-turn", &format!("[cas #{} operator Alice@phone verified 0s first] New words", send.id()), "Incremental answer", Utc::now())
            .iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n";
        std::fs::OpenOptions::new().append(true).open(path).unwrap().write_all(append.as_bytes()).unwrap();
        state.poll_clock = Some(now + std::time::Duration::from_secs(7));
        scan(&mut state);
        assert_eq!(operator_replies(&queue)[0].message, "Incremental answer");
        assert_eq!(state.codex_ownership.borrow().values().map(|cursor| cursor.bytes_read).sum::<u64>(), initial_bytes + append.len() as u64,
            "a growing rollout must read only its appended records");
    }

    #[test]
    fn finalized_receipts_back_off_without_losing_later_delivery() {
        let (temp, root, queue) = terminal_session(vec![]);
        let stamp = OperatorStamp { operator: "Alice".into(), device_id: "phone".into(),
            device_label: "phone".into(), scopes: vec!["message:send".into()], verified: true };
        let id = queue.enqueue_operator_message("commander:Alice@phone", "supervisor", "Unproven old send",
            Some("factory-1"), None, None, false, None, &stamp).unwrap().id();
        queue.mark_processed(id).unwrap();
        let now = std::time::Instant::now();
        let mut state = MirrorScanState { poll_clock: Some(now), ..Default::default() };
        state.pending_operator_receipts.insert(id);
        let scan = |state: &mut MirrorScanState| scan_supervisor_replies(
            &root, temp.path(), "factory-1", &queue, &[], state);
        scan(&mut state);
        state.poll_clock = Some(now + std::time::Duration::from_secs(1));
        scan(&mut state);
        assert_eq!(state.receipt_reads, 1, "finalized unproven sends must use a slower receipt watch");
        assert!(state.operator_messages.is_empty(), "cleanup is never delivery evidence");
        queue.mark_transport_delivered(id).unwrap();
        state.poll_clock = Some(now + std::time::Duration::from_secs(31));
        scan(&mut state);
        assert_eq!(state.receipt_reads, 2);
        assert_eq!(state.operator_messages.len(), 1, "a later real receipt still publishes");
        assert_eq!(state.operator_messages[0].state, "delivered");
        assert_eq!(state.finished_operator_receipts, vec![id]);
    }

    #[test]
    fn ct2_p3_operator_mirror_unchanged_transcript_is_not_read() {
        let (temp, root, queue) = terminal_session(codex_turn_at(
            "status",
            "[cas #77 agent-authored] update",
            "Ready.",
            Utc::now() - chrono::Duration::seconds(10),
        ));
        let store = SqliteAgentStore::open(&root).unwrap();
        let mut state = MirrorScanState::default();
        let agents = store.list(None).unwrap();
        scan_supervisor_replies(&root, temp.path(), "factory-1", &queue, &agents, &mut state);
        assert_eq!(state.transcript_reads, 1);
        for status in [
            cas_types::AgentStatus::Idle,
            cas_types::AgentStatus::Shutdown,
        ] {
            let mut agent = store.get("supervisor-id").unwrap();
            agent.status = status;
            store.update(&agent).unwrap();
            scan_supervisor_replies(
                &root,
                temp.path(),
                "factory-1",
                &queue,
                &store.list(None).unwrap(),
                &mut state,
            );
        }
        assert_eq!(
            state.transcript_reads, 1,
            "unchanged polls must not read transcript tails"
        );
        assert_eq!(operator_replies(&queue).len(), 1);
    }

    #[test]
    fn ct2_p3_operator_mirror_toggle_between_polls_excludes_private_turn() {
        let (temp, root, queue) = terminal_session(vec![]);
        let mut config = crate::config::Config::default();
        config
            .set("factory.commander_mirror_enabled", "false")
            .unwrap();
        config.save(&root).unwrap();
        // CLI saving false must record the floor without a daemon poll.
        assert!(
            root.join("commander-mirror/policy.disabled").exists(),
            "disable boundary must be durable"
        );
        let path = temp
            .path()
            .join("account/sessions/2026/10/10/rollout-2026-10-10T12-00-00-terminal.jsonl");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        let now = Utc::now();
        for mut value in codex_turn_at("private", "Terminal question", "Private reply", now) {
            value["timestamp"] = serde_json::json!(now.to_rfc3339());
            writeln!(file, "{value}").unwrap();
        }
        queue
            .record_terminal_operator_turn("factory-1", "Terminal question")
            .unwrap();
        config
            .set("factory.commander_mirror_enabled", "true")
            .unwrap();
        config.save(&root).unwrap();
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        assert!(
            operator_replies(&queue).is_empty(),
            "no poll observed false; disabled replies must still stay private"
        );
        for value in codex_turn_at(
            "public",
            "[cas #78 agent-authored] update",
            "Public reply",
            Utc::now(),
        ) {
            writeln!(file, "{value}").unwrap();
        }
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        let replies = operator_replies(&queue);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].message, "Public reply");
    }

    #[test]
    fn ct2_p3_operator_mirror_resume_starts_at_config_save() {
        let (temp, root, queue) = terminal_session(vec![]);
        let mut config = crate::config::Config::default();
        config
            .set("factory.commander_mirror_enabled", "false")
            .unwrap();
        config.save(&root).unwrap();
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        config
            .set("factory.commander_mirror_enabled", "true")
            .unwrap();
        config.save(&root).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(
                temp.path()
                    .join("account/sessions/2026/10/10/rollout-2026-10-10T12-00-00-terminal.jsonl"),
            )
            .unwrap();
        let at = Utc::now();
        for mut value in codex_turn_at(
            "public-after-resume",
            "[cas #78 agent-authored] update",
            "Reply before first enabled poll.",
            at,
        ) {
            value["timestamp"] = serde_json::json!(at.to_rfc3339());
            writeln!(file, "{value}").unwrap();
        }
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        let replies = operator_replies(&queue);
        assert_eq!(
            replies.len(),
            1,
            "resume starts at CLI save, not the later daemon poll"
        );
        assert_eq!(replies[0].message, "Reply before first enabled poll.");
    }

    #[test]
    fn ct2_p3_operator_mirror_excludes_workers_and_other_sessions() {
        for (role, status, session) in [
            (AgentRole::Worker, cas_types::AgentStatus::Idle, "factory-1"),
            (
                AgentRole::Worker,
                cas_types::AgentStatus::Shutdown,
                "factory-1",
            ),
            (
                AgentRole::Supervisor,
                cas_types::AgentStatus::Active,
                "factory-other",
            ),
        ] {
            let (temp, root, queue) = terminal_session(codex_turn_at(
                "foreign",
                "[cas #77 agent-authored] update",
                "Foreign reply.",
                Utc::now(),
            ));
            let store = SqliteAgentStore::open(&root).unwrap();
            let mut agent = store.get("supervisor-id").unwrap();
            agent.role = role;
            agent.status = status;
            agent.factory_session = Some(session.into());
            store.update(&agent).unwrap();
            let mut state = MirrorScanState::default();
            scan_supervisor_replies(
                &root,
                temp.path(),
                "factory-1",
                &queue,
                &store.list(None).unwrap(),
                &mut state,
            );
            assert_eq!(state.transcript_reads, 0);
            assert!(
                operator_replies(&queue).is_empty(),
                "excluded {role:?}/{status}/{session}"
            );
        }
    }

    #[test]
    fn ct2_p3_operator_mirror_defaults_on_when_unset() {
        let config: crate::config::Config = toml::from_str("[factory]\n").unwrap();
        assert_eq!(
            config.get("factory.commander_mirror_enabled").as_deref(),
            Some("true")
        );
        assert!(
            crate::config::Config::default()
                .factory()
                .commander_mirror_enabled
        );
    }

    #[test]
    fn ct2_p3_operator_mirror_cursor_invalidates_on_source_and_file_changes() {
        let (temp, root, queue) = terminal_session(codex_turn_at(
            "status",
            "[cas #77 agent-authored] update",
            "Ready.",
            Utc::now() - chrono::Duration::seconds(10),
        ));
        let store = SqliteAgentStore::open(&root).unwrap();
        let mut state = MirrorScanState::default();
        let scan = |state: &mut MirrorScanState| {
            scan_supervisor_replies(
                &root,
                temp.path(),
                "factory-1",
                &queue,
                &store.list(None).unwrap(),
                state,
            )
        };
        scan(&mut state);
        let first = temp
            .path()
            .join("account/sessions/2026/10/10/rollout-2026-10-10T12-00-00-terminal.jsonl");
        let second = temp
            .path()
            .join("account2/sessions/2026/10/10/rollout-2026-10-10T12-00-00-terminal.jsonl");
        std::fs::create_dir_all(second.parent().unwrap()).unwrap();
        std::fs::copy(&first, &second).unwrap();
        let mut agent = store.get("supervisor-id").unwrap();
        agent.metadata.insert(
            "supervisor_account_dir".into(),
            temp.path().join("account2").to_string_lossy().into(),
        );
        store.update(&agent).unwrap();
        scan(&mut state);
        assert_eq!(
            state.transcript_reads, 2,
            "account/path change must invalidate"
        );
        agent.cc_session_id = Some("new-harness-session".into());
        store.update(&agent).unwrap();
        scan(&mut state);
        assert_eq!(
            state.transcript_reads, 3,
            "harness session change must invalidate"
        );
        // Same length, changed mtime: edits/replacements are not only appends.
        std::fs::File::open(&second)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
            .unwrap();
        scan(&mut state);
        assert_eq!(state.transcript_reads, 4, "mtime change must invalidate");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&second)
            .unwrap();
        for value in codex_turn_at(
            "new-status",
            "[cas #78 agent-authored] update",
            "New reply.",
            Utc::now(),
        ) {
            writeln!(file, "{value}").unwrap();
        }
        scan(&mut state);
        assert_eq!(state.transcript_reads, 5, "length change must invalidate");
        scan(&mut state);
        assert_eq!(state.transcript_reads, 5);
        assert_eq!(operator_replies(&queue).len(), 2);
        let rotated = second.with_file_name("rollout-2026-10-10T12-00-01-terminal.jsonl");
        std::fs::rename(&second, &rotated).unwrap();
        scan(&mut state);
        assert_eq!(
            state.transcript_reads, 6,
            "removed path must be resolved again"
        );
        agent
            .metadata
            .insert("supervisor_cli".into(), "claude".into());
        store.update(&agent).unwrap();
        scan(&mut state);
        assert_eq!(
            state.transcript_reads, 6,
            "must not reuse Codex path for Claude"
        );
        agent
            .metadata
            .insert("supervisor_cli".into(), "codex".into());
        store.update(&agent).unwrap();
        scan(&mut state);
        assert_eq!(
            state.transcript_reads, 7,
            "harness change invalidates source"
        );
        assert_eq!(operator_replies(&queue).len(), 2);
    }

    #[test]
    fn terminal_operator_prompt_is_captured_before_the_supervisor_finishes() {
        let mut env = crate::test_support::TestEnvGuard::new();
        for (cli, queued) in [
            (cas_mux::SupervisorCli::Codex, false),
            (cas_mux::SupervisorCli::Claude, false),
            (cas_mux::SupervisorCli::Claude, true),
        ] {
            let now = Utc::now();
            // cas-7cd1: a real typed message that pastes harness-shaped text
            // after its own words is still the operator's.
            let question = "Message from lifecycle team: please check this capture\n<environment_context>\n  <shell>bash</shell>\n</environment_context>";
            let input = |id: &str, text: &str, at: DateTime<Utc>| match cli {
                cas_mux::SupervisorCli::Claude if queued => vec![serde_json::json!({
                    "timestamp":at.to_rfc3339(),"type":"attachment","uuid":id,"userType":"external",
                    "attachment":{"type":"queued_command","origin":{"kind":"human"},
                        "prompt":[{"type":"image","source":{}},{"type":"text","text":text}]}
                })],
                cas_mux::SupervisorCli::Claude => vec![serde_json::json!({
                    "timestamp":at.to_rfc3339(),"type":"user","uuid":format!("uuid-{id}"),"promptId":id,
                    "origin":{"kind":"human"},"promptSource":"typed","turnOrigin":"human","userType":"external",
                    "message":{"role":"user","content":[{"type":"image","source":{}},{"type":"text","text":text}]}
                })],
                _ => vec![
                    serde_json::json!({"timestamp":at.to_rfc3339(),"type":"event_msg","payload":{"type":"task_started","turn_id":id}}),
                    serde_json::json!({"timestamp":at.to_rfc3339(),"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}}),
                ],
            };
            let mut values = input("pending", question, now);
            // Reject explicit machine origins even with ordinary message text,
            // as well as legacy unmarked CAS/Commander envelopes.
            for (index, (field, provenance)) in [
                ("origin", serde_json::json!({"kind":"task-notification"})),
                ("promptSource", serde_json::json!("system")),
                ("turnOrigin", serde_json::json!("task_notification")),
                ("userType", serde_json::json!("internal")),
                ("machine_prompt_provenance", serde_json::json!({"origin":"agent_authored"})),
                ("isMeta", serde_json::json!(true)),
                ("isSidechain", serde_json::json!(true)),
            ].into_iter().enumerate() {
                let mut records = input(&format!("injected-{index}"), "Ordinary looking machine text", now);
                records.last_mut().unwrap()[field] = provenance;
                values.extend(records);
            }
            if queued {
                for (field, provenance) in [
                    ("origin", serde_json::json!({"kind":"task-notification"})),
                    ("isMeta", serde_json::json!(true)),
                ] {
                    let mut records = input(field, "Ordinary looking queued machine text", now);
                    records[0]["attachment"][field] = provenance;
                    values.extend(records);
                }
            }
            for (index, text) in [
                "[cas #77 operator Daniel@Phone verified 0s first] Commander send",
                "<task-notification>completed</task-notification>",
                "CAS wake: message 7 from supervisor is in your inbox",
                "Plain prefix\n<teammate-message teammate_id=\"director\">agent text</teammate-message>",
                // cas-7cd1: Codex's own context and the daemon's pane deliveries.
                "# AGENTS.md instructions for /repo\n\n<INSTRUCTIONS>\nrules\n</INSTRUCTIONS>\n<environment_context>\n  <shell>bash</shell>\n</environment_context>",
                "<environment_context>\n  <current_date>2026-10-30</current_date>\n</environment_context>",
                "Message from director: [cas #2 agent-authored 5s first]\n\nCodex supervisor startup: 0 current workers.",
                "Message from lifecycle-wake:delivery-watchdog: Cassy delivery watchdog: a normal supervisor message may be waiting",
            ].into_iter().enumerate() {
                values.extend(input(&format!("envelope-{index}"), text, now));
            }
            if cli == cas_mux::SupervisorCli::Claude {
                // Same-turn text-only projection has a different UUID and no
                // provenance. It must not become another operator bubble.
                values.push(serde_json::json!({"timestamp":now.to_rfc3339(),"type":"user","uuid":"projection",
                    "message":{"role":"user","content":[{"type":"text","text":question}]}}));
                values.push(serde_json::json!({"timestamp":now.to_rfc3339(),"type":"user","uuid":"tool-result",
                    "message":{"role":"user","content":[{"type":"tool_result","content":"private output"},{"type":"text","text":"Generated tool context"}]}}));
            }
            let (temp, root, queue) = terminal_session(values.clone());
            env.set("CLAUDE_CONFIG_DIR", temp.path().join("account").to_str().unwrap());
            let agents = SqliteAgentStore::open(&root).unwrap();
            let mut supervisor = agents.get("supervisor-id").unwrap();
            supervisor.metadata.insert("supervisor_cli".into(), cli.backend().name().to_owned());
            agents.update(&supervisor).unwrap();
            let path = if cli == cas_mux::SupervisorCli::Claude {
                let directory = temp.path().join("account/projects").join(temp.path().to_string_lossy().replace(['/', '.'], "-"));
                std::fs::create_dir_all(&directory).unwrap();
                let path = directory.join("supervisor-id.jsonl");
                std::fs::write(&path, values.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
                path
            } else {
                temp.path().join("account/sessions/2026/10/10/rollout-2026-10-10T12-00-00-terminal.jsonl")
            };
            let mut state = MirrorScanState::default();
            let scan = |state: &mut MirrorScanState| scan_supervisor_replies(
                &root, temp.path(), "factory-1", &queue, &agents.list(None).unwrap(), state,
            );
            scan(&mut state);
            scan(&mut state);
            assert_eq!(state.operator_messages.len(), 1, "{cli:?}: live input before final answer");
            assert_eq!(state.operator_messages[0].text, question);
            assert_eq!(state.operator_messages[0].operator_label.as_deref(), Some("Terminal"));
            assert_eq!(state.operator_messages[0].device_id, "terminal");
            assert_eq!(state.operator_messages[0].state, "delivered");
            assert!(!state.operator_messages[0].stamped);
            for device in ["phone", "desktop"] {
                let rows = queue.conversation_history("factory-1", device, None, 50).unwrap();
                assert_eq!(rows.len(), 1, "{cli:?}: terminal input is owed before any assistant answer");
                assert_eq!(rows[0].source, "terminal");
                assert_eq!(rows[0].prompt, question);
                assert_eq!(rows[0].created_at, now);
                assert!(rows[0].processed_at.is_some(), "history must never re-inject the prompt");
            }
            // A new turn with identical text is a distinct question; replaying
            // the transcript after a daemon restart still retains exactly two.
            let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
            for record in input("pending-again", question, now + chrono::Duration::seconds(1)) {
                writeln!(file, "{record}").unwrap();
            }
            scan(&mut state);
            assert_eq!(state.operator_messages.len(), 2);
            scan(&mut MirrorScanState::default());
            assert_eq!(queue.conversation_history("factory-1", "phone", None, 50).unwrap().len(), 2);
        }
    }

    #[test]
    fn ct2_p3_operator_mirror_without_prior_send() {
        let (temp, root, queue) = terminal_session(codex_turn_at(
            "unsolicited-status",
            "[cas #77 agent-authored] worker update",
            "The release is ready for review.",
            Utc::now() - chrono::Duration::seconds(10),
        ));
        assert_eq!(queue.first_operator_activity_at("factory-1").unwrap(), None);

        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);

        for device in ["phone", "desktop"] {
            let rows = queue
                .conversation_history("factory-1", device, None, 50)
                .unwrap();
            assert_eq!(rows.len(), 1, "operator lane must not need a prior send");
            assert_eq!(rows[0].target, "operator");
            let reply: crate::ui::factory::OperatorReplyPayload =
                serde_json::from_str(&rows[0].prompt).unwrap();
            assert_eq!(reply.message, "The release is ready for review.");
            assert_eq!(reply.kind, crate::ui::factory::OperatorTurnKind::Status);
            assert_eq!(reply.reply_to, None);
            assert_eq!(reply.device_id, "*");
        }
    }

    #[test]
    fn ct2_p3_operator_mirror_survives_supervisor_state_changes() {
        for status in [
            cas_types::AgentStatus::Idle,
            cas_types::AgentStatus::Stale,
            cas_types::AgentStatus::Shutdown,
        ] {
            let question = "Is the release ready?";
            let (temp, root, queue) = terminal_session(codex_turn_at(
                "final-before-idle",
                question,
                "Yes, it is ready.",
                Utc::now(),
            ));
            let terminal = queue
                .record_terminal_operator_turn("factory-1", question)
                .unwrap();
            let agents = SqliteAgentStore::open(&root).unwrap();
            let mut supervisor = agents.get("supervisor-id").unwrap();
            supervisor.status = status;
            agents.update(&supervisor).unwrap();

            mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
            mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);

            let replies = operator_replies(&queue);
            assert_eq!(replies.len(), 1, "completed answer lost in {status}");
            assert_eq!(replies[0].message, "Yes, it is ready.");
            assert_eq!(replies[0].reply_to, Some(terminal));
        }
    }

    #[test]
    fn ct2_p3_operator_mirror_opt_out_does_not_backfill_disabled_turns() {
        let question = "Keep this terminal turn private.";
        let (temp, root, queue) = terminal_session(codex_turn_at(
            "private-turn",
            question,
            "Private answer.",
            Utc::now() - chrono::Duration::seconds(10),
        ));
        queue
            .record_terminal_operator_turn("factory-1", question)
            .unwrap();
        std::fs::write(
            root.join("config.toml"),
            "[factory]\ncommander_mirror_enabled = false\n",
        )
        .unwrap();
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        assert!(
            operator_replies(&queue).is_empty(),
            "explicit opt-out must stop mirroring"
        );

        std::fs::write(
            root.join("config.toml"),
            "[factory]\ncommander_mirror_enabled = true\n",
        )
        .unwrap();
        // The first enabled poll starts a new window. Even terminal-linked
        // answers from the disabled interval must not backfill into it.
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        assert!(operator_replies(&queue).is_empty());
        let mut transcript = std::fs::OpenOptions::new()
            .append(true)
            .open(
                temp.path()
                    .join("account/sessions/2026/10/10/rollout-2026-10-10T12-00-00-terminal.jsonl"),
            )
            .unwrap();
        for value in codex_turn_at(
            "public-turn",
            "[cas #78 agent-authored] new update",
            "Public status.",
            Utc::now() + chrono::Duration::seconds(1),
        ) {
            writeln!(transcript, "{value}").unwrap();
        }
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        let replies = operator_replies(&queue);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].message, "Public status.");
    }

    /// The violet_ps shape: the operator never paired a device and only typed
    /// in the terminal. Their question and the supervisor's answer must both
    /// reach the conversation, the answer threaded under the question, once.
    #[test]
    fn enrolled_terminal_conversation_reaches_history_and_cloud_once_cas_5c89() {
        let question = "What changed in Violet today?";
        let (temp, root, queue) = terminal_session(codex_turn_at(
            "turn-enrolled",
            question,
            "**Two fixes landed.**",
            Utc::now(),
        ));
        queue
            .bind_operator_feed(&cas_store::OperatorFeedBinding {
                account_id: "acct-1".into(),
                machine_id: "machine-1".into(),
                hub_id: "hub-1".into(),
                project_id: "violet_ps".into(),
                bound_at: Utc::now().to_rfc3339(),
            })
            .unwrap();
        let terminal = queue
            .record_terminal_operator_turn("factory-1", question)
            .unwrap();

        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);

        let claims = queue
            .claim_operator_cloud("machine-1", Utc::now(), 100, 30)
            .unwrap();
        assert_eq!(claims.len(), 2, "cloud gets the question and answer once");
        let snapshots: Vec<Value> = claims
            .iter()
            .map(|claim| serde_json::from_str(&claim.payload_snapshot).unwrap())
            .collect();
        assert_eq!(snapshots[0]["prompt_id"], terminal);
        assert_eq!(snapshots[0]["source"], "terminal");
        assert_eq!(snapshots[0]["target"], "terminal-history");
        assert_eq!(snapshots[0]["prompt"], question);
        assert!(
            snapshots[0]["operator"].is_null(),
            "no paired-device stamp is invented"
        );
        let payload: Value =
            serde_json::from_str(snapshots[1]["prompt"].as_str().unwrap()).unwrap();
        assert_eq!(snapshots[1]["target"], "operator");
        assert_eq!(payload["reply_to"], terminal);
        assert_eq!(payload["message"], "**Two fixes landed.**");
        assert_eq!(payload["kind"], "answer");

        for device in ["phone", "laptop"] {
            let mut history = queue
                .conversation_history("factory-1", device, None, 50)
                .unwrap();
            history.reverse();
            assert_eq!(history.len(), 2);
            for (row, snapshot) in history.iter().zip(&snapshots) {
                assert_eq!(row.id, snapshot["prompt_id"].as_i64().unwrap());
                assert_eq!(row.prompt, snapshot["prompt"].as_str().unwrap());
            }
        }
        assert!(
            queue
                .peek_for_targets(&["terminal-history"], Some("factory-1"), 50)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_terminal_only_operator_gets_the_supervisors_answers_cas_5c89() {
        let question = "What changed in Violet today?";
        let now = Utc::now();
        let mut turns = codex_turn_at(
            "turn-q",
            question,
            "**Two fixes landed.**",
            now + chrono::Duration::seconds(1),
        );
        turns.extend(codex_turn_at(
            "turn-m",
            "[cas #77 agent-authored 0s first] worker update",
            "Noted the worker update.",
            now + chrono::Duration::seconds(10),
        ));
        let (temp, root, queue) = terminal_session(turns);
        let terminal = queue
            .record_terminal_operator_turn("factory-1", question)
            .unwrap();

        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);

        let replies = operator_replies(&queue);
        assert_eq!(
            replies.len(),
            2,
            "one row per completed turn, no duplicates: {replies:?}"
        );
        let answer = replies
            .iter()
            .find(|reply| reply.message == "**Two fixes landed.**")
            .expect("the answer to the terminal question is mirrored");
        assert_eq!(answer.kind, crate::ui::factory::OperatorTurnKind::Answer);
        assert_eq!(answer.reply_to, Some(terminal));
        assert_eq!(answer.device_id, "*");
        let status = replies
            .iter()
            .find(|reply| reply.message == "Noted the worker update.")
            .unwrap();
        assert_eq!(status.kind, crate::ui::factory::OperatorTurnKind::Status);
        assert_eq!(status.reply_to, None);

        // The conversation reads question, then answer (pages are newest
        // first; clients order by time), and never carries the
        // machine-injected prompt itself.
        let history = queue
            .conversation_history("factory-1", "phone", None, 50)
            .unwrap();
        let question_row = history.iter().find(|row| row.id == terminal).unwrap();
        let answer_row = history
            .iter()
            .find(|row| row.target == "operator" && row.prompt.contains("Two fixes landed"))
            .unwrap();
        assert!(
            question_row.created_at <= answer_row.created_at && question_row.id < answer_row.id,
            "{history:?}"
        );
        assert!(
            !history
                .iter()
                .any(|row| row.prompt.contains("worker update") && row.target != "operator")
        );
    }

    /// Answers to the operator's own terminal questions are backfilled even
    /// when they completed before mirroring started; other old turns are not,
    /// so first activation never floods the phone with an old transcript.
    #[test]
    fn answers_to_terminal_questions_backfill_but_other_old_turns_do_not_cas_5c89() {
        let question = "Is the release branch green?";
        let now = Utc::now();
        let mut turns = codex_turn_at("turn-q", question, "Yes, every check passed.", now);
        turns.extend(codex_turn_at(
            "turn-old",
            "[cas #9 agent-authored] relay",
            "Old status.",
            now + chrono::Duration::seconds(6),
        ));
        let (temp, root, queue) = terminal_session(turns);
        let terminal = queue
            .record_terminal_operator_turn("factory-1", question)
            .unwrap();
        // Mirroring started after both turns completed.
        let marker_dir = root.join("commander-mirror");
        std::fs::create_dir_all(&marker_dir).unwrap();
        let hash = format!("{:x}", Sha256::digest("factory-1".as_bytes()));
        std::fs::write(
            marker_dir.join(format!("{hash}.started")),
            (now + chrono::Duration::minutes(10)).to_rfc3339(),
        )
        .unwrap();

        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);

        let replies = operator_replies(&queue);
        assert_eq!(replies.len(), 1, "{replies:?}");
        assert_eq!(replies[0].message, "Yes, every check passed.");
        assert_eq!(replies[0].reply_to, Some(terminal));
    }

    /// cas-8737 and cas-5c89: a harness-started turn never threads under a
    /// terminal row, even one with identical text, and is not backfilled.
    #[test]
    fn harness_prompts_never_become_operator_questions_cas_5c89() {
        let envelope = "[cas #77 agent-authored 0s first] worker update";
        let now = Utc::now();
        let (temp, root, queue) = terminal_session(codex_turn_at(
            "turn-h",
            envelope,
            "Noted.",
            now + chrono::Duration::seconds(1),
        ));
        queue
            .record_terminal_operator_turn("factory-1", envelope)
            .unwrap();
        mirror_supervisor_replies(&root, temp.path(), "factory-1", &queue);
        let replies = operator_replies(&queue);
        assert_eq!(replies.len(), 1, "{replies:?}");
        assert_eq!(
            replies[0].kind,
            crate::ui::factory::OperatorTurnKind::Status
        );
        assert_eq!(replies[0].reply_to, None);
    }
}
