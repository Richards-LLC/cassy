use super::*;
use cas_types::{
    Agent, AgentRole, AgentStatus, Event, EventEntityType, EventType, Task, TaskStatus,
};
use std::path::PathBuf;

const SESSION: &str = "provision-test";
const SUPERVISOR: &str = "test-supervisor";

fn env() -> crate::test_support::TestEnvGuard {
    crate::test_support::TestEnvGuard::with_vars(&[
        ("CAS_FACTORY_SESSION", SESSION),
        ("CAS_AGENT_NAME", SUPERVISOR),
        ("CAS_AGENT_ROLE", "supervisor"),
    ])
}

struct Fixture {
    root: tempfile::TempDir,
    cas_dir: PathBuf,
    daemon: FactoryDaemon,
    queue: Arc<dyn cas_store::PromptQueueStore>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let cas_dir = crate::store::init_cas_dir(root.path()).unwrap();
        let agents = open_agent_store(&cas_dir).unwrap();
        for (name, role) in [
            (SUPERVISOR, AgentRole::Supervisor),
            ("worker", AgentRole::Worker),
        ] {
            let mut agent = Agent::new_with_role(format!("{name}-id"), name.into(), role);
            agent.factory_session = Some(SESSION.into());
            agents.register(&agent).unwrap();
        }
        Self {
            daemon: super::super::provisioning_tests::daemon(&cas_dir),
            queue: open_prompt_queue_store(&cas_dir).unwrap(),
            cas_dir,
            root,
        }
    }

    fn aged(&self, source: &str, target: &str, body: &str) -> i64 {
        let target = if target == "supervisor" {
            SUPERVISOR
        } else {
            target
        };
        let id = self
            .queue
            .enqueue_urgent_with_outcome(
                source,
                target,
                body,
                Some(SESSION),
                Some(body),
                Some(cas_store::NotificationPriority::High),
                false,
                None,
            )
            .unwrap()
            .id();
        rusqlite::Connection::open(self.cas_dir.join("cas.db"))
            .unwrap()
            .execute(
                "UPDATE prompt_queue SET created_at=? WHERE id=?",
                rusqlite::params![
                    (chrono::Utc::now() - chrono::Duration::seconds(900)).to_rfc3339(),
                    id
                ],
            )
            .unwrap();
        id
    }

    fn scan(&mut self) -> Vec<cas_store::QueuedPrompt> {
        self.daemon
            .pane_silent_since
            .insert("worker".into(), Instant::now() - Duration::from_secs(900));
        self.daemon
            .enqueue_delivery_stalled_bounces(self.queue.as_ref());
        self.daemon.relay_idle_unacked_messages(self.queue.as_ref());
        self.queue
            .peek_all(100)
            .unwrap()
            .into_iter()
            .filter(|row| {
                row.source == "delivery-watchdog"
                    || row
                        .summary
                        .as_deref()
                        .is_some_and(|s| s.starts_with("Unread on idle worker"))
                    || row.prompt.contains("kind=\"supervisor_unread\"")
            })
            .collect()
    }
}

#[test]
fn cas_07ee_terminated_recipient_never_raises_unread_or_stalled() {
    let _env = env();
    for state in ["shutdown", "dead-pane"] {
        let mut f = Fixture::new();
        f.aged("supervisor", "worker", "Please run the task check");
        if state == "shutdown" {
            let store = open_agent_store(&f.cas_dir).unwrap();
            let mut worker = store.get("worker-id").unwrap();
            worker.status = AgentStatus::Shutdown;
            store.register(&worker).unwrap();
            assert_eq!(
                store.get("worker-id").unwrap().status,
                AgentStatus::Shutdown
            );
        } else {
            f.daemon.dead_workers.insert("worker".into());
        }
        assert!(
            f.scan().is_empty(),
            "a terminated recipient needs no alarm: {state}"
        );
    }
}

#[test]
fn cas_07ee_closed_task_no_longer_needs_attention() {
    let _env = env();
    let mut f = Fixture::new();
    let mut task = Task::new("cas-07ee".into(), "delivery".into());
    task.assignee = Some("worker".into());
    task.status = TaskStatus::Closed;
    crate::store::open_task_store_local(&f.cas_dir)
        .unwrap()
        .add(&task)
        .unwrap();
    f.aged("supervisor", "worker", "cas-07ee merged; re-close now");
    assert!(
        f.scan().is_empty(),
        "re-close is already satisfied by the closed task"
    );
}

#[test]
fn cas_07ee_merged_delivery_no_longer_needs_attention() {
    let _env = env();
    let mut f = Fixture::new();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(f.root.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@example.invalid"]);
    git(&["config", "user.name", "Cassy Test"]);
    std::fs::write(f.root.path().join("base.txt"), "base\n").unwrap();
    git(&["add", "base.txt"]);
    git(&["commit", "-qm", "base"]);
    git(&["checkout", "-qb", "factory/worker"]);
    std::fs::write(f.root.path().join("delivery.txt"), "already merged\n").unwrap();
    git(&["add", "delivery.txt"]);
    git(&["commit", "-qm", "delivery"]);
    let tip = git(&["rev-parse", "HEAD"]);
    git(&["checkout", "-q", "main"]);
    git(&["merge", "--no-ff", "--no-edit", "factory/worker"]);
    let mut task = Task::new("cas-07ee".into(), "delivery".into());
    task.assignee = Some("worker".into());
    task.status = TaskStatus::AwaitingMerge;
    task.deliverables.parked_branch = Some("factory/worker".into());
    crate::store::open_task_store_local(&f.cas_dir)
        .unwrap()
        .add(&task)
        .unwrap();
    let envelope = crate::prompt_revalidation::MergeRequestEnvelope {
        task_id: task.id.clone(),
        branch_tip: tip,
        target_branch: "main".into(),
        target_branch_tip: "old-target".into(),
        anchor_tip: None,
        commits_not_on_target_base: 1,
        pr_number: None,
        source_freshness_unavailable: None,
    };
    assert!(matches!(
        crate::prompt_revalidation::queued_merge_request_decision(
            f.root.path(),
            Some(&task),
            &envelope
        ),
        crate::prompt_revalidation::MergeRequestDecision::AlreadyIntegrated { .. }
    ));
    let body =
        crate::prompt_revalidation::attach_merge_request_envelope("status: ready", &envelope);
    f.aged("worker", "supervisor", &body);
    assert!(
        f.scan().is_empty(),
        "the requested commit is already on the real Git target"
    );
}

#[test]
fn cas_07ee_later_recipient_reply_suppresses_both_alarm_lanes() {
    let _env = env();
    for (sender, recipient) in [("supervisor", "worker"), ("worker", "supervisor")] {
        let mut f = Fixture::new();
        let id = f.aged(sender, recipient, "Please perform request 2574");
        f.queue
            .enqueue_with_session(recipient, sender, "Your request 2574 is complete", SESSION)
            .unwrap();
        assert!(f.scan().is_empty(), "recipient acted on #{id}");
        assert!(
            f.queue
                .message_delivery_report(id)
                .unwrap()
                .unwrap()
                .confirmed_at
                .is_none(),
            "action evidence suppresses alarms without fabricating explicit read receipts"
        );
    }
}

#[test]
fn cas_07ee_attributed_task_note_suppresses_stalled_attention() {
    let _env = env();
    let mut f = Fixture::new();
    f.aged(
        "supervisor",
        "worker",
        "Supervisor clarified narrow drift rule: one canonical candidate",
    );
    let event = Event::new(
        EventType::TaskNoteAdded,
        EventEntityType::Task,
        "cas-07ee",
        "Supervisor clarified narrow drift rule: one canonical candidate",
    )
    .with_session("worker-id");
    crate::store::open_event_store(&f.cas_dir)
        .unwrap()
        .record(&event)
        .unwrap();
    assert!(
        f.scan().is_empty(),
        "the recipient's task note proves it acted"
    );
}

#[test]
fn cas_07ee_later_codex_turn_suppresses_stalled_attention() {
    let _env = env();
    let mut f = Fixture::new();
    let id = f.aged("supervisor", "worker", "Please apply the scope boundary");
    assert!(f.queue.claim_recipient_transport(id, "worker").unwrap());
    let account = f.root.path().join("codex");
    let sessions = account.join("sessions/2026/10/15");
    std::fs::create_dir_all(&sessions).unwrap();
    let session_id = "019f84af-3121-7950-ba14-b01db2da1153";
    let path = sessions.join(format!("rollout-2026-10-15T12-00-00-{session_id}.jsonl"));
    let now = chrono::Utc::now();
    let lines = [
        serde_json::json!({"timestamp":now.to_rfc3339(),"type":"session_meta","payload":{"id":session_id,"session_id":session_id,"cwd":f.root.path(),"originator":"codex-tui","source":"cli","thread_source":"user"}}),
        serde_json::json!({"timestamp":now.to_rfc3339(),"type":"event_msg","payload":{"type":"task_started","turn_id":"acted"}}),
        serde_json::json!({"timestamp":(now+chrono::Duration::seconds(1)).to_rfc3339(),"type":"event_msg","payload":{"type":"task_complete","turn_id":"acted"}}),
    ];
    std::fs::write(
        &path,
        lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let store = open_agent_store(&f.cas_dir).unwrap();
    let mut worker = store.get("worker-id").unwrap();
    worker.cc_session_id = Some(session_id.into());
    worker.metadata.insert("worker_cli".into(), "codex".into());
    worker.metadata.insert(
        "worker_account_dir".into(),
        account.to_string_lossy().into(),
    );
    worker
        .metadata
        .insert("clone_path".into(), f.root.path().to_string_lossy().into());
    store.register(&worker).unwrap();
    let created = f.queue.queued_prompt(id).unwrap().unwrap().created_at;
    assert!(
        f.daemon
            .recipient_turn_completion_after("worker", created)
            .is_some(),
        "fixture must resolve the real Codex turn"
    );
    assert!(
        f.scan().is_empty(),
        "a later turn makes unread/stalled attention obsolete"
    );
}

#[test]
fn cas_07ee_merge_envelope_never_claims_it_is_ordinary_unread_traffic() {
    let _env = env();
    let mut f = Fixture::new();
    let envelope = crate::prompt_revalidation::MergeRequestEnvelope {
        task_id: "cas-07ee".into(),
        branch_tip: "delivery-tip".into(),
        target_branch: "main".into(),
        target_branch_tip: "old-target".into(),
        anchor_tip: None,
        commits_not_on_target_base: 1,
        pr_number: None,
        source_freshness_unavailable: None,
    };
    let body =
        crate::prompt_revalidation::attach_merge_request_envelope("status: ready", &envelope);
    f.aged("worker", "supervisor", &body);
    assert!(
        !f.scan()
            .iter()
            .any(|row| row.prompt.contains("kind=\"supervisor_unread\"")),
        "a typed merge request must not be described as carrying no merge envelope"
    );
}

#[test]
fn cas_07ee_informational_approval_does_not_raise_idle_unread() {
    let _env = env();
    for body in [
        "Review PASS. Holding the approved branch until CI is green.",
        "Approved; I will merge on green CI, then tell you to re-close.",
    ] {
        let mut f = Fixture::new();
        f.aged("supervisor", "worker", body);
        f.daemon
            .pane_silent_since
            .insert("worker".into(), Instant::now() - Duration::from_secs(900));
        f.daemon.relay_idle_unacked_messages(f.queue.as_ref());
        assert!(
            !f.queue.peek_all(20).unwrap().iter().any(|row| row
                .summary
                .as_deref()
                .is_some_and(|s| s.starts_with("Unread on idle worker"))),
            "informational message needs no idle-unread alarm: {body}"
        );
    }
}

#[test]
fn cas_07ee_teammate_surface_confirms_supervisor_and_never_alerts() {
    let _env = env();
    let mut f = Fixture::new();
    let body = "ready: request 2574 complete";
    let id = f.aged("worker", "supervisor", body);
    assert!(f.queue.claim_recipient_transport(id, SUPERVISOR).unwrap());
    let input = crate::hooks::HookInput {
        session_id: "sup-session".into(),
        agent_role: Some("supervisor".into()),
        agent_id: Some(format!("supervisor@{SESSION}")),
        agent_type: Some("team-lead".into()),
        ..Default::default()
    };
    let prompt = format!(
        "<teammate-message teammate_id=\"worker\">\n[cas #{id} worker-authored 0s first]\n\n{body}\n</teammate-message>"
    );
    crate::hooks::handlers::handlers_middle::factory_inbox::record_turn_prompt_envelopes(
        &f.cas_dir, &input, &prompt,
    );
    let report = f.queue.message_delivery_report(id).unwrap().unwrap();
    assert_eq!(report.stage, cas_store::DeliveryStage::Confirmed);
    assert!(f.scan().is_empty());
}

#[test]
fn cas_07ee_never_surfaced_supervisor_message_still_alerts_once() {
    let _env = env();
    let mut f = Fixture::new();
    let id = f.aged(
        "worker",
        "supervisor",
        "Please review this unsurfaced delivery",
    );
    let first = f.scan();
    assert_eq!(
        first
            .iter()
            .filter(|row| row.prompt.contains("kind=\"supervisor_unread\""))
            .count(),
        1
    );
    assert!(
        first
            .iter()
            .any(|row| row.prompt.contains(&format!("message {id}")))
    );
    let again = f.scan();
    assert_eq!(again.len(), first.len(), "one durable alarm per unread row");
}

#[test]
fn cas_07ee_guard_approval_with_a_new_instruction_still_alerts() {
    let _env = env();
    let mut f = Fixture::new();
    f.aged(
        "supervisor",
        "worker",
        "Approved. I will merge the branch; rebase your worker now.",
    );
    assert!(
        !f.scan().is_empty(),
        "an approval prefix must not hide a real instruction"
    );
}

#[test]
fn cas_07ee_guard_unrelated_author_activity_does_not_hide_unread() {
    let _env = env();
    let mut f = Fixture::new();
    f.aged("supervisor", "worker", "Please run the targeted checks");
    let event = Event::new(
        EventType::TaskNoteAdded,
        EventEntityType::Task,
        "cas-07ee",
        "Another worker acted",
    )
    .with_session("other-worker-id");
    crate::store::open_event_store(&f.cas_dir)
        .unwrap()
        .record(&event)
        .unwrap();
    f.queue
        .enqueue_with_session(
            "other-worker",
            "supervisor",
            "I finished my unrelated task",
            SESSION,
        )
        .unwrap();
    assert!(
        !f.scan().is_empty(),
        "only this recipient's activity can quiet its alarm"
    );
}

#[test]
fn cas_07ee_guard_closed_task_mention_is_not_a_satisfied_instruction() {
    let _env = env();
    let mut f = Fixture::new();
    let mut task = Task::new("cas-07ee".into(), "closed".into());
    task.status = TaskStatus::Closed;
    crate::store::open_task_store_local(&f.cas_dir)
        .unwrap()
        .add(&task)
        .unwrap();
    f.aged(
        "supervisor",
        "worker",
        "Please explain why cas-07ee closed without the required evidence",
    );
    assert!(
        !f.scan().is_empty(),
        "a closed-task mention can still ask new work"
    );
}
