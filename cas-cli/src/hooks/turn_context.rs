//! Redundant factory context delivery when Claude skips UserPromptSubmit.
//!
//! A bounded transcript tail identifies turns; a per-session file lock shares
//! receipts between hook processes and the MCP server. Prompt bodies are never
//! persisted here or sent to attribution/memory ingestion.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::HookInput;

const TAIL_BYTES: u64 = 256 * 1024;
const RECENT_TURNS: usize = 32;

#[derive(Default, Serialize, Deserialize)]
struct TurnState {
    delivered: Vec<String>,
    silent_prompts: u64,
    updated_at: i64,
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
        if !self.state.delivered.iter().any(|seen| seen == key) {
            self.state.delivered.push(key.to_string());
            if self.state.delivered.len() > RECENT_TURNS {
                self.state.delivered.remove(0);
            }
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
fn latest_turn(path: &Path, session: &str) -> Option<(String, String)> {
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let start = size.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    for line in text.lines().rev() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value["type"] != "user" || value["isMeta"] == true || value["isSidechain"] == true {
            continue;
        }
        if value
            .get("sessionId")
            .and_then(|v| v.as_str())
            .is_some_and(|id| id != session)
        {
            continue;
        }
        // Claude emits operator prompts as strings. Arrays include tool_result
        // and synthetic user messages; do not guess their provenance.
        let Some(prompt) = value["message"]["content"].as_str() else {
            continue;
        };
        let Some(key) = value["promptId"]
            .as_str()
            .or_else(|| value["uuid"].as_str())
        else {
            continue;
        };
        return Some((key.to_string(), prompt.to_string()));
    }
    None
}

/// Called by PostToolUse and by successful MCP responses. Inbox receipts are
/// already atomic and shared with UserPromptSubmit. Recall runs once per turn,
/// including read tools, independent of observation capture filters.
pub(crate) fn fallback_context(root: &Path, input: &HookInput) -> Option<String> {
    if crate::internal_llm::is_internal_invocation()
        || !crate::harness_policy::is_factory_agent(input)
    {
        return None;
    }
    let (key, prompt) = latest_turn(
        Path::new(input.transcript_path.as_deref()?),
        &input.session_id,
    )?;
    let mut receipt = Receipt::open(root, &input.session_id)?;
    if receipt.state.delivered.contains(&key) {
        return None;
    }
    // Both recovery channels share this claim before any inbox/store work.
    receipt.remember(&key);
    receipt.state.silent_prompts = receipt.state.silent_prompts.saturating_add(1);
    receipt.save()?;
    let mut parts = Vec::new();
    if let Some(mail) =
        super::handlers::handlers_middle::factory_inbox::surface_factory_inbox(Some(root), input)
    {
        parts.push(mail);
    }
    if let Some(packet) =
        crate::ambient_recall::build_local_ambient_recall_context(input, root, &prompt)
    {
        parts.push(packet.full);
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
