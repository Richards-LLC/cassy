//! cas-5f67: the automatic supervisor handoff and clear, through the daemon's
//! real turn scanner.
use std::io::Write;
use std::path::{Path, PathBuf};

use cas_store::{AgentStore, PromptQueueStore, SqliteAgentStore, SqlitePromptQueueStore};
use cas_types::{Agent, AgentRole, Task, TaskStatus};
use serde_json::json;

use super::commander_mirror::{MirrorScanState, scan_supervisor_replies};
use crate::factory_context_reset::{CLEAR_COMMAND_MARKER, CONTEXT_RESET_CONTROL};
use crate::supervisor_auto_clear::SOURCE;

const SESSION: &str = "factory-5f67";
const APPROVAL: &str = "[2026-11-07 18:17] ✅ DECISION OPERATOR APPROVED: cut v9.99.1 unattended once green";

struct Lane {
    _temp: tempfile::TempDir,
    root: PathBuf,
    project: PathBuf,
    transcript: PathBuf,
    queue: SqlitePromptQueueStore,
    agents: Vec<Agent>,
    state: MirrorScanState,
    turn: u32,
}

impl Lane {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().to_path_buf();
        let root = project.join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("config.toml"),
            "[factory]\ncommander_mirror_enabled = false\nsupervisor_wake_input_budget = 1000000\n",
        )
        .unwrap();
        let queue = SqlitePromptQueueStore::open(&root).unwrap();
        queue.init().unwrap();
        let account = project.join("account");
        let mut supervisor = Agent::new_with_role("sup-5f67".into(), "calm-koala".into(), AgentRole::Supervisor);
        supervisor.cc_session_id = Some("native-5f67".into());
        supervisor.factory_session = Some(SESSION.into());
        supervisor.metadata.insert("supervisor_cli".into(), "claude".into());
        supervisor.metadata.insert("supervisor_account_dir".into(), account.to_string_lossy().into_owned());
        let store = SqliteAgentStore::open(&root).unwrap();
        store.init().unwrap();
        store.register(&supervisor).unwrap();
        let tasks = crate::store::open_task_store(&root).unwrap();
        let mut epic = Task::new("cas-train".into(), "Release train v9.99.1".into());
        epic.epic_verification_owner = Some(supervisor.id.clone());
        epic.status = TaskStatus::InProgress;
        tasks.add(&epic).unwrap();
        tasks.append_note("cas-train", APPROVAL).unwrap();
        let transcript = account
            .join("projects")
            .join(project.to_string_lossy().replace(['/', '.'], "-"))
            .join("native-5f67.jsonl");
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(&transcript, "").unwrap();
        Self { _temp: temp, root, project, transcript, queue, agents: vec![supervisor], state: MirrorScanState::default(), turn: 0 }
    }

    /// Append one supervisor turn; `end=false` leaves it running.
    fn turn(&mut self, input_tokens: u64, end: bool) {
        self.turn += 1;
        let at = (chrono::Utc::now() + chrono::Duration::seconds(i64::from(self.turn))).to_rfc3339();
        let id = format!("turn-{}", self.turn);
        let mut lines = vec![json!({"type":"user","uuid":id,"timestamp":at,"message":{"content":"status?"}})];
        if end {
            lines.push(json!({"type":"assistant","timestamp":at,"message":{"id":format!("msg-{id}"),"role":"assistant",
                "usage":{"input_tokens":input_tokens,"output_tokens":5},"content":[{"type":"text","text":"Done."}],"stop_reason":"end_turn"}}));
        }
        let mut file = std::fs::OpenOptions::new().append(true).open(&self.transcript).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn scan(&mut self) {
        scan_supervisor_replies(&self.root, &self.project, SESSION, &self.queue, &self.agents, &mut self.state);
    }

    fn rows(&self, body: &str) -> Vec<cas_store::QueuedPrompt> {
        self.queue
            .peek_all(500)
            .unwrap()
            .into_iter()
            .filter(|row| row.source == SOURCE && row.prompt.contains(body))
            .collect()
    }

    fn handoffs(&self) -> Vec<cas_types::Entry> {
        crate::store::open_store(&self.root)
            .unwrap()
            .list()
            .unwrap()
            .into_iter()
            .filter(cas_core::memory::handoff::is_handoff)
            .collect()
    }

    fn current_handoff(&self) -> cas_types::Entry {
        let handoffs = self.handoffs();
        cas_core::memory::handoff::current_handoff(&handoffs, "supervisor").unwrap().clone()
    }

    fn deliver_notice(&self) {
        for row in self.rows("Context budget") {
            self.queue.mark_processed(row.id).unwrap();
        }
    }
}

fn merge_lock(root: &Path) -> crate::worktree::target_lock::DeliveryTargetLock {
    crate::worktree::target_lock::lock_delivery_target(root, Path::new("/repo/.git"), "main").unwrap()
}

/// AC (a) and (c): one over-budget wake at an idle boundary saves exactly one
/// handoff and queues exactly one clear; the new session gets the handoff with
/// the operator approval and the supervisor's judgement intact.
#[test]
fn over_budget_wake_at_idle_boundary_hands_off_and_clears_once() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let mut lane = Lane::new();
    lane.turn(1_300_000, true);
    lane.scan();
    lane.scan();
    assert_eq!(lane.handoffs().len(), 1, "one handoff from the lane facts");
    let facts = lane.current_handoff();
    assert!(facts.content.contains(APPROVAL), "operator approval copied from the decision note: {}", facts.content);
    assert_eq!(lane.rows("Context budget").len(), 1, "one judgement notice");
    assert!(lane.rows(CONTEXT_RESET_CONTROL).is_empty(), "no clear before the judgement turn");

    lane.deliver_notice();
    crate::store::open_store(&lane.root).unwrap().add(&{
        let mut judgement = cas_types::Entry::new("mem-judgement".into(), "Next: merge the no-op wake fix first.".into());
        judgement.tags = vec!["handoff".into(), "role:supervisor".into()];
        judgement
    }).unwrap();
    lane.turn(40_000, true);
    lane.scan();
    lane.scan();
    assert_eq!(lane.rows(CONTEXT_RESET_CONTROL).len(), 1, "exactly one clear after the judgement turn");

    let dir = lane.transcript.parent().unwrap().to_path_buf();
    let cleared = format!("{}\n", json!({"type":"user","message":{"content":CLEAR_COMMAND_MARKER}}));
    std::fs::write(dir.join("new-session.jsonl"), cleared).unwrap();
    lane.scan();
    lane.scan();
    let resumed = lane.rows("Cassy cleared your context");
    assert_eq!(resumed.len(), 1, "one resume brief for the new session");
    for kept in [APPROVAL, "Next: merge the no-op wake fix first."] {
        assert!(resumed[0].prompt.contains(kept), "resumed session lost {kept}: {}", resumed[0].prompt);
        assert!(lane.current_handoff().content.contains(kept), "SessionStart handoff lost {kept}");
    }
    assert_eq!(lane.rows(CONTEXT_RESET_CONTROL).len(), 1);
}

/// AC (b): an over-budget wake that ends during an in-flight merge, or while a
/// newer turn is already running, defers; each step fires only once idle.
#[test]
fn over_budget_wake_waits_for_idle_and_never_clears_mid_turn() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let mut lane = Lane::new();
    let lock = merge_lock(&lane.root);
    lane.turn(1_300_000, true);
    lane.scan();
    assert!(lane.handoffs().is_empty(), "no handoff while a merge holds the target lock");
    drop(lock);

    lane.turn(20_000, true);
    lane.turn(0, false);
    lane.scan();
    assert!(lane.handoffs().is_empty(), "no handoff while a newer turn is running");
    let mut file = std::fs::OpenOptions::new().append(true).open(&lane.transcript).unwrap();
    writeln!(file, "{}", json!({"type":"assistant","timestamp":chrono::Utc::now().to_rfc3339(),"message":{"id":"msg-late","role":"assistant",
        "usage":{"input_tokens":20_000,"output_tokens":5},"content":[{"type":"text","text":"Done."}],"stop_reason":"end_turn"}})).unwrap();
    lane.scan();
    assert_eq!(lane.handoffs().len(), 1, "handoff saved at the first idle turn end");

    lane.deliver_notice();
    lane.turn(20_000, true);
    lane.turn(0, false);
    lane.scan();
    assert!(lane.rows(CONTEXT_RESET_CONTROL).is_empty(), "no clear while the supervisor is mid-turn");
    let lock = merge_lock(&lane.root);
    let mut file = std::fs::OpenOptions::new().append(true).open(&lane.transcript).unwrap();
    writeln!(file, "{}", json!({"type":"assistant","timestamp":chrono::Utc::now().to_rfc3339(),"message":{"id":"msg-later","role":"assistant",
        "usage":{"input_tokens":20_000,"output_tokens":5},"content":[{"type":"text","text":"Done."}],"stop_reason":"end_turn"}})).unwrap();
    lane.scan();
    assert!(lane.rows(CONTEXT_RESET_CONTROL).is_empty(), "no clear during an in-flight merge");
    drop(lock);
    lane.turn(20_000, true);
    lane.scan();
    assert_eq!(lane.rows(CONTEXT_RESET_CONTROL).len(), 1, "clear queued at the idle boundary");
}

/// GH #1665: repeated cache reads measure billing, not live context pressure.
#[test]
fn cached_wake_replay_uses_latest_call_and_reported_window() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let mut lane = Lane::new();
    lane.turn(0, false);
    let at = chrono::Utc::now().to_rfc3339();
    let mut file = std::fs::OpenOptions::new().append(true).open(&lane.transcript).unwrap();
    // 78 calls, first 62,490, peak 116,844, cumulative 7,627,334.
    for call in 0..78 {
        let input = match call { 0 => 62_490, 77 => 116_844, _ => 98_000 };
        writeln!(file, "{}", json!({"type":"assistant","timestamp":at,"message":{
            "id":format!("cached-{call}"),"role":"assistant",
            "usage":{"input_tokens":100,"cache_read_input_tokens":input - 200,
                "cache_creation_input_tokens":100,"output_tokens":5},
            "content":[{"type":"text","text":"ok"}],
            "stop_reason":if call == 77 { "end_turn" } else { "tool_use" }
        }})).unwrap();
    }
    lane.scan();
    assert!(lane.handoffs().is_empty());
    assert!(lane.rows(CONTEXT_RESET_CONTROL).is_empty());

    // A Codex prompt at 85% of its reported 200K window must trigger even
    // though the lifetime counter is unrelated and the absolute cap is 1M.
    std::fs::write(&lane.transcript, format!("{}\n", json!({"type":"event_msg","payload":{
        "type":"token_count","info":{"last_token_usage":{"input_tokens":170_000,
            "cached_input_tokens":160_000},"total_token_usage":{"input_tokens":7_627_334},
            "model_context_window":200_000}
    }}))).unwrap();
    let step = crate::supervisor_auto_clear::on_turn_end(&lane.root, &lane.queue,
        &crate::supervisor_auto_clear::TurnEnd {
            agent:&lane.agents[0], cli:cas_mux::SupervisorCli::Codex, factory_session:SESSION,
            harness_session:"codex-session", transcript:&lane.transcript,
            input_tokens:Some(170_000), at:chrono::Utc::now(), mid_turn:false,
        }).unwrap();
    assert!(matches!(step, crate::supervisor_auto_clear::Step::HandoffSaved(_)));
}

#[test]
fn handoff_receipt_and_injection_keep_only_live_work_and_readable_pending_rows() {
    use cas_store::{SqliteSupervisorQueueStore, SupervisorQueueStore};
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let mut lane = Lane::new();
    let tasks = crate::store::open_task_store(&lane.root).unwrap();
    for index in 0..150 {
        let mut task = Task::new(format!("cas-stale-{index}"), "old task".into());
        task.assignee = Some(lane.agents[0].id.clone());
        task.notes = "old history ".repeat(1000);
        tasks.add(&task).unwrap();
    }
    let mut parked = Task::new("cas-parked".into(), "delivery".into());
    parked.assignee = Some(lane.agents[0].id.clone());
    parked.status = TaskStatus::AwaitingMerge;
    tasks.add(&parked).unwrap();
    let pending = SqliteSupervisorQueueStore::open(&lane.root).unwrap();
    pending.init().unwrap();
    for _ in 0..89 {
        pending.notify(&lane.agents[0].id, "task_blocked", &json!({
            "actor":"nimble-badger-37", "task_id":"cas-parked",
            "message":"unneeded payload ".repeat(100)
        }).to_string(), cas_store::NotificationPriority::Normal).unwrap();
    }
    lane.turn(1_300_000, true);
    lane.scan();
    let receipt_path = std::fs::read_dir(lane.root.join("supervisor-handoffs")).unwrap()
        .next().unwrap().unwrap().path();
    let bytes = std::fs::read(receipt_path).unwrap();
    let snapshot: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(snapshot["tasks"].as_array().unwrap().len(), 2);
    assert!(bytes.len() < 30_000, "receipt bytes: {}", bytes.len());
    let content = lane.current_handoff().content;
    assert!(content.len() < 8_000, "injection bytes: {}", content.len());
    assert!(!content.contains("cas-stale-"));
    let row = content.lines().find(|line| line.contains("nimble-badger-37")).unwrap();
    for data in ["task_blocked", "cas-parked"] { assert!(row.contains(data)); }
    assert!(!row.contains('{'));
    assert!(!row.contains('\n'));
    assert!(content.contains(APPROVAL));
}
