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

    fn gate(&self, id: i64) {
        self.queue
            .record_pending_reason(
                id,
                cas_store::PendingReason::GatedNotReady,
                Some("wake_declined_by_policy: pane has not been silent long enough"),
            )
            .unwrap();
        self.queue
            .record_wake_attempt(
                id,
                cas_store::WakeAttempt::NotAttempted,
                Some("wake declined for busy recipient"),
            )
            .unwrap();
        assert_eq!(
            self.queue
                .message_delivery_report(id)
                .unwrap()
                .unwrap()
                .stage,
            cas_store::DeliveryStage::Gated
        );
    }

    fn codex_turn(&self, text: &str) {
        let account = self.root.path().join("codex");
        let sessions = account.join("sessions/2026/10/15");
        std::fs::create_dir_all(&sessions).unwrap();
        let session_id = "019f84af-3121-7950-ba14-b01db2da1153";
        let path = sessions.join(format!("rollout-2026-10-15T12-00-00-{session_id}.jsonl"));
        let now = chrono::Utc::now() - chrono::Duration::seconds(400);
        let lines = [
            serde_json::json!({"timestamp":now.to_rfc3339(),"type":"session_meta","payload":{"id":session_id,"session_id":session_id,"cwd":self.root.path(),"originator":"codex-tui","source":"cli","thread_source":"user"}}),
            serde_json::json!({"timestamp":now.to_rfc3339(),"type":"event_msg","payload":{"type":"task_started","turn_id":"acted"}}),
            serde_json::json!({"timestamp":now.to_rfc3339(),"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}],"internal_chat_message_metadata_passthrough":{"turn_id":"acted"}}}),
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
        let store = open_agent_store(&self.cas_dir).unwrap();
        let mut worker = store.get("worker-id").unwrap();
        worker.cc_session_id = Some(session_id.into());
        worker.metadata.insert("worker_cli".into(), "codex".into());
        worker.metadata.insert(
            "worker_account_dir".into(),
            account.to_string_lossy().into(),
        );
        worker.metadata.insert(
            "clone_path".into(),
            self.root.path().to_string_lossy().into(),
        );
        store.register(&worker).unwrap();
    }

    fn supervisor_turn(&self, cli: &str, text: &str) {
        let account = self.root.path().join(cli);
        let session_id = "019f84af-3121-7950-ba14-b01db2da1154";
        let now = chrono::Utc::now() - chrono::Duration::seconds(400);
        let (path, records) = if cli == "codex" {
            (
                account.join(format!("sessions/2026/10/15/rollout-{session_id}.jsonl")),
                vec![
                    serde_json::json!({"timestamp":now.to_rfc3339(),"type":"session_meta","payload":{"id":session_id,"cwd":self.root.path(),"originator":"codex-tui","source":"cli"}}),
                    serde_json::json!({"timestamp":now.to_rfc3339(),"type":"event_msg","payload":{"type":"task_started","turn_id":"consumed"}}),
                    serde_json::json!({"timestamp":now.to_rfc3339(),"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}],"internal_chat_message_metadata_passthrough":{"turn_id":"consumed"}}}),
                ],
            )
        } else {
            let slug = self.root.path().to_string_lossy().replace(['/', '.'], "-");
            (
                account.join(format!("projects/{slug}/{session_id}.jsonl")),
                vec![
                    serde_json::json!({"timestamp":now.to_rfc3339(),"type":"user","message":{"role":"user","content":text}}),
                ],
            )
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            records.iter().map(|r| format!("{r}\n")).collect::<String>(),
        )
        .unwrap();
        let store = open_agent_store(&self.cas_dir).unwrap();
        let mut agent = store.get(&format!("{SUPERVISOR}-id")).unwrap();
        agent.cc_session_id = Some(session_id.into());
        agent.metadata.insert("supervisor_cli".into(), cli.into());
        agent.metadata.insert(
            "worker_account_dir".into(),
            account.to_string_lossy().into(),
        );
        agent.metadata.insert(
            "clone_path".into(),
            self.root.path().to_string_lossy().into(),
        );
        store.register(&agent).unwrap();
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
    task.closed_at = Some(chrono::Utc::now());
    task.deliverables.merge_commit = Some("merged-delivery".into());
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
fn cas_521d_merge_request_stops_alerting_after_its_task_changes_state() {
    let _env = env();
    // Each case starts with its own queue and notification history.
    for handled in [false, true] {
        let mut f = Fixture::new();
        let before_message = chrono::Utc::now() - chrono::Duration::seconds(1200);
        let mut task = Task::new("cas-521d-transition".into(), "Parked delivery".into());
        task.status = TaskStatus::AwaitingMerge;
        task.assignee = Some("worker".into());
        task.created_at = before_message;
        task.updated_at = before_message;
        crate::store::open_task_store_local(&f.cas_dir)
            .unwrap()
            .add(&task)
            .unwrap();

        let envelope = crate::prompt_revalidation::MergeRequestEnvelope {
            task_id: task.id.clone(),
            branch_tip: "a".repeat(40),
            target_branch: "epic/test".into(),
            target_branch_tip: "b".repeat(40),
            anchor_tip: None,
            commits_not_on_target_base: 1,
            pr_number: None,
            source_freshness_unavailable: None,
        };
        let body = crate::prompt_revalidation::attach_merge_request_envelope(
            "Please merge this parked delivery",
            &envelope,
        );
        let id = f.aged("worker", "supervisor", &body);

        // The supervisor's decision reopens the task for corrective work. The
        // queued merge request stays unread, but its task lifecycle proves it was
        // handled; it must no longer feed a supervisor_unread alert.
        if handled {
            task.status = TaskStatus::InProgress;
            crate::store::open_task_store_local(&f.cas_dir)
                .unwrap()
                .update(&task)
                .unwrap();
        }
        assert!(
            f.queue
                .queued_prompt(id)
                .unwrap()
                .is_some_and(|row| row.acked_at.is_none()),
            "the lifecycle action must not fake an explicit message acknowledgement"
        );
        assert_eq!(f.scan().is_empty(), handled, "only a handled merge request loses its alert");
    }
}

#[test]
fn cas_07ee_same_recipient_unbound_reply_still_alarms() {
    let _env = env();
    for (sender, recipient) in [("supervisor", "worker"), ("worker", "supervisor")] {
        let mut f = Fixture::new();
        let id = f.aged(sender, recipient, "Please perform request 2574");
        f.queue
            .enqueue_with_session(recipient, sender, "Your request 2574 is complete", SESSION)
            .unwrap();
        assert!(
            !f.scan().is_empty(),
            "an unbound reply is not evidence for #{id}"
        );
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
fn cas_07ee_same_recipient_unrelated_note_still_alarms() {
    let _env = env();
    let mut f = Fixture::new();
    let id = f.aged(
        "supervisor",
        "worker",
        "Supervisor clarified narrow drift rule: one canonical candidate",
    );
    f.gate(id);
    let event = Event::new(
        EventType::TaskNoteAdded,
        EventEntityType::Task,
        "cas-unrelated",
        "Supervisor clarified narrow drift rule: one canonical candidate",
    )
    .with_session("worker-id");
    crate::store::open_event_store(&f.cas_dir)
        .unwrap()
        .record(&event)
        .unwrap();
    assert!(
        !f.scan().is_empty(),
        "an unrelated note must not hide this message"
    );
}

#[test]
fn cas_07ee_same_recipient_unrelated_codex_turn_still_alarms() {
    let _env = env();
    let mut f = Fixture::new();
    let id = f.aged("supervisor", "worker", "Please apply the scope boundary");
    f.gate(id);
    assert!(f.queue.claim_recipient_transport(id, "worker").unwrap());
    f.codex_turn("A separate task that never carried this queued body");
    let created = f.queue.queued_prompt(id).unwrap().unwrap().created_at;
    assert!(
        super::super::delivery_probe_jobs::recipient_turn_completion_after_io(
            f.daemon.app.cas_dir(), "worker", created)
            .is_some(),
        "fixture must resolve the real Codex turn"
    );
    f.daemon
        .pane_silent_since
        .insert("worker".into(), Instant::now() - Duration::from_secs(900));
    f.daemon.relay_idle_unacked_messages(f.queue.as_ref());
    assert!(
        f.queue.peek_all(100).unwrap().iter().any(|row| row
            .summary
            .as_deref()
            .is_some_and(|s| s.starts_with("Unread on idle worker"))),
        "a completed unrelated turn followed by idle must still raise idle-unread"
    );
    assert!(
        !f.scan().is_empty(),
        "an unrelated completed turn must not hide this message"
    );
}

#[test]
fn cas_07ee_unsurfaced_merge_envelope_still_alarms() {
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
        f.scan()
            .iter()
            // pin: machine-readable worker-attention kind routes the unread alarm.
            .any(|row| row.prompt.contains("kind=\"supervisor_unread\"")),
        "an envelope without a confirmed wake still needs attention"
    );
}

#[test]
fn cas_07ee_untyped_approvals_still_raise_idle_unread() {
    let _env = env();
    for body in [
        "Review PASS. Holding the approved branch until CI is green.",
        "Approved; I will merge on green CI, then tell you to re-close.",
    ] {
        let mut f = Fixture::new();
        let id = f.aged("supervisor", "worker", body);
        f.gate(id);
        f.daemon
            .pane_silent_since
            .insert("worker".into(), Instant::now() - Duration::from_secs(900));
        f.daemon.relay_idle_unacked_messages(f.queue.as_ref());
        assert!(
            f.queue.peek_all(20).unwrap().iter().any(|row| row
                .summary
                .as_deref()
                .is_some_and(|s| s.starts_with("Unread on idle worker"))),
            "untyped text cannot establish informational intent: {body}"
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
            // pin: count this typed alarm kind to prove one durable episode.
            .filter(|row| row.prompt.contains("kind=\"supervisor_unread\""))
            .count(),
        1
    );
    assert!(
        first
            .iter()
            // pin: the diagnostic must identify the exact unread row for investigation.
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

#[test]
fn cas_07ee_missing_recipient_evidence_still_alarms() {
    let _env = env();
    let mut f = Fixture::new();
    let id = f.aged("supervisor", "worker", "Please fix the red CI");
    rusqlite::Connection::open(f.cas_dir.join("cas.db"))
        .unwrap()
        .execute("DELETE FROM agents WHERE id = 'worker-id'", [])
        .unwrap();
    assert!(
        !f.scan().is_empty(),
        "missing registry evidence must not hide #{id}"
    );
}

#[test]
fn cas_07ee_unreadable_recipient_registry_still_needs_attention() {
    let _env = env();
    let f = Fixture::new();
    let id = f.aged("supervisor", "worker", "Please fix the red CI");
    let row = f.queue.queued_prompt(id).unwrap().unwrap();
    rusqlite::Connection::open(f.cas_dir.join("cas.db"))
        .unwrap()
        .execute("DROP TABLE agents", [])
        .unwrap();
    assert_eq!(
        f.daemon
            .message_attention_decisions(f.queue.as_ref(), &[row])
            .get(&id),
        Some(&MessageAttention::Needed)
    );
}

#[test]
fn cas_07ee_respawned_worker_name_still_alarms() {
    let _env = env();
    let mut f = Fixture::new();
    f.aged(
        "supervisor",
        "worker",
        "Please handle this course correction",
    );
    f.daemon.dead_workers.insert("worker".into());
    let pty = cas_mux::Pty::spawn(
        "worker",
        cas_mux::PtyConfig {
            command: "sh".into(),
            args: vec!["-c".into(), "sleep 30".into()],
            cwd: Some(f.root.path().into()),
            env: vec![],
            env_remove: vec![],
            rows: 24,
            cols: 80,
        },
    )
    .unwrap();
    let pane = cas_mux::Pane::with_pty(
        "worker",
        cas_mux::PaneKind::Worker,
        pty,
        24,
        80,
        cas_mux::SupervisorCli::Codex,
    )
    .unwrap();
    f.daemon.app.adopt_worker_pane_for_test(pane);
    assert!(
        !f.scan().is_empty(),
        "a name in dead_workers can belong to a live respawn"
    );
}

#[test]
fn cas_07ee_closed_before_a_new_instruction_still_alarms() {
    let _env = env();
    let mut f = Fixture::new();
    let mut task = Task::new("cas-07ee".into(), "delivery".into());
    task.status = TaskStatus::Closed;
    task.closed_at = Some(chrono::Utc::now() - chrono::Duration::hours(1));
    task.updated_at = task.closed_at.unwrap();
    crate::store::open_task_store_local(&f.cas_dir)
        .unwrap()
        .add(&task)
        .unwrap();
    f.aged(
        "supervisor",
        "worker",
        "cas-07ee needs investigation; re-close after repair",
    );
    assert!(
        !f.scan().is_empty(),
        "closure before the instruction cannot satisfy it"
    );
}

#[test]
fn cas_07ee_exact_body_in_codex_turn_satisfies_only_that_row() {
    let _env = env();
    let mut f = Fixture::new();
    let body = "Please apply the scope boundary";
    let id = f.aged("supervisor", "worker", body);
    let other = f.aged("supervisor", "worker", "A separate course correction");
    f.codex_turn(body);
    let rows = [
        f.queue.queued_prompt(id).unwrap().unwrap(),
        f.queue.queued_prompt(other).unwrap().unwrap(),
    ];
    let got = f
        .daemon
        .message_attention_decisions(f.queue.as_ref(), &rows);
    assert_eq!(got.get(&id), Some(&MessageAttention::Satisfied));
    assert_eq!(got.get(&other), Some(&MessageAttention::Needed));
    assert!(
        f.queue
            .message_delivery_report(id)
            .unwrap()
            .unwrap()
            .confirmed_at
            .is_none()
    );
}

#[tokio::test]
async fn cas_07ee_explicit_reply_confirms_the_referenced_row() {
    let _env = env();
    let mut f = Fixture::new();
    let id = f.aged("supervisor", "worker", "Please perform request 2574");
    let core = crate::mcp::CasCore::with_daemon(f.cas_dir.clone(), None, None);
    core.set_agent_id_for_testing("worker-id".into());
    #[cfg(feature = "mcp-proxy")]
    let service = crate::mcp::CasService::new(core, None);
    #[cfg(not(feature = "mcp-proxy"))]
    let service = crate::mcp::CasService::new(core);
    let request = serde_json::from_value(serde_json::json!({
        "action":"message", "target":"supervisor", "in_reply_to":id,
        "summary":"request complete", "message":"Your request 2574 is complete"
    }))
    .unwrap();
    service
        .coordination(rmcp::handler::server::wrapper::Parameters(request))
        .await
        .unwrap();
    assert!(
        f.queue
            .message_delivery_report(id)
            .unwrap()
            .unwrap()
            .confirmed_at
            .is_some()
    );
    assert!(f.scan().is_empty());
}

#[tokio::test]
async fn cas_07ee_note_reference_is_row_bound_even_after_summary_truncation() {
    let _env = env();
    for spelling in ["#", "notification_id="] {
        let mut f = Fixture::new();
        let id = f.aged(
            "supervisor",
            "worker",
            "Please implement the narrow drift rule",
        );
        crate::store::open_task_store_local(&f.cas_dir)
            .unwrap()
            .add(&Task::new("cas-other".into(), "implementation".into()))
            .unwrap();
        let core = crate::mcp::CasCore::with_daemon(f.cas_dir.clone(), None, None);
        core.set_agent_id_for_testing("worker-id".into());
        let request = serde_json::from_value(serde_json::json!({
            "id":"cas-other", "note_type":"progress",
            "note":format!("{} Applied the instruction in {spelling}{id}.", "implementation detail ".repeat(8))
        })).unwrap();
        core.cas_task_notes(rmcp::handler::server::wrapper::Parameters(request))
            .await
            .unwrap();
        let events = crate::store::open_event_store(&f.cas_dir)
            .unwrap()
            .list_by_type(EventType::TaskNoteAdded, 10)
            .unwrap();
        assert!(
            !events[0].summary.contains(&format!("{spelling}{id}")),
            "exercise summary truncation"
        );
        assert!(
            f.scan().is_empty(),
            "the attributed full note cites exactly this row"
        );
        assert!(
            f.queue
                .message_delivery_report(id)
                .unwrap()
                .unwrap()
                .confirmed_at
                .is_none()
        );
    }
}

#[test]
fn cas_07ee_note_reference_prefix_does_not_satisfy_another_row() {
    let _env = env();
    let mut f = Fixture::new();
    let id = f.aged("supervisor", "worker", "Please fix the red CI");
    let event = Event::new(
        EventType::TaskNoteAdded,
        EventEntityType::Task,
        "cas-other",
        format!("Applied #{id}0 and notification_id={id}1"),
    )
    .with_session("worker-id");
    crate::store::open_event_store(&f.cas_dir)
        .unwrap()
        .record(&event)
        .unwrap();
    assert!(
        !f.scan().is_empty(),
        "numeric prefixes are not exact row references"
    );
}

#[test]
fn cas_07ee_closure_without_merge_evidence_still_alarms() {
    let _env = env();
    let mut f = Fixture::new();
    let id = f.aged("supervisor", "worker", "cas-07ee merged; re-close now");
    let mut task = Task::new("cas-07ee".into(), "delivery".into());
    task.status = TaskStatus::Closed;
    task.closed_at = Some(chrono::Utc::now());
    crate::store::open_task_store_local(&f.cas_dir)
        .unwrap()
        .add(&task)
        .unwrap();
    assert!(
        !f.scan().is_empty(),
        "closure alone does not prove the requested merge for #{id}"
    );
}

/// GH #1196: a message about a task that was cancelled after it was sent
/// asks nothing more of anyone, so it raises no unread or stalled notice.
#[test]
fn gh_1196_cancelled_task_no_longer_needs_attention() {
    let _env = env();
    let mut f = Fixture::new();
    let id = f.aged("supervisor", "worker", "cas-1be0 merged; re-close now");
    let mut task = Task::new("cas-1be0".into(), "delivery".into());
    task.status = TaskStatus::Cancelled;
    task.closed_at = Some(chrono::Utc::now());
    crate::store::open_task_store_local(&f.cas_dir)
        .unwrap()
        .add(&task)
        .unwrap();
    assert!(
        f.scan().is_empty(),
        "a task cancelled after #{id} retires its alarm"
    );
}

// A handed-off Codex row can lack a usable transcript binding. The watchdog
// must distinguish later authored action from transport alone (cas-0cdaa).
fn handed_off_row(f: &Fixture, source: &str, target: &str) -> i64 {
    let agents = open_agent_store(&f.cas_dir).unwrap();
    let mut worker = agents.get("worker-id").unwrap();
    worker.cc_session_id = Some("codex-worker-synthetic-session".into());
    worker.metadata.insert("worker_cli".into(), "codex".into());
    let account = f.root.path().join("unbound-codex");
    std::fs::create_dir_all(&account).unwrap();
    worker.metadata.insert(
        "worker_account_dir".into(),
        account.to_string_lossy().into(),
    );
    worker
        .metadata
        .insert("clone_path".into(), f.root.path().to_string_lossy().into());
    agents.register(&worker).unwrap();
    let id = f.aged(
        source,
        target,
        "cas-0cdaa: Review passed; wait for CI and merge notification",
    );
    assert!(f.queue.claim_recipient_transport(id, target).unwrap());
    f.queue.mark_transport_delivered(id).unwrap();
    // Give the fixture a stable ordering without sleeping or depending on
    // SQLite's sub-millisecond timestamp precision.
    rusqlite::Connection::open(f.cas_dir.join("cas.db"))
        .unwrap()
        .execute(
            "UPDATE prompt_queue SET transport_delivered_at=? WHERE id=?",
            rusqlite::params![
                (chrono::Utc::now() - chrono::Duration::seconds(600)).to_rfc3339(),
                id
            ],
        )
        .unwrap();
    id
}

fn authored_outbound(f: &Fixture, origin: &cas_store::QueueOrigin) -> i64 {
    f.queue
        .enqueue_urgent_with_outcome(
            "worker",
            SUPERVISOR,
            "Recorded the decision; waiting for the merge",
            Some(SESSION),
            Some("status"),
            Some(cas_store::NotificationPriority::Normal),
            false,
            Some(origin),
        )
        .unwrap()
        .id()
}

fn actor(id: &str) -> cas_store::QueueOrigin {
    cas_store::QueueOrigin::RegisteredAgent { agent_id: id.into() }
}

fn task_note(f: &Fixture, session: &str) {
    let event = Event::new(EventType::TaskNoteAdded, EventEntityType::Task, "cas-0cdaa", "Decision recorded")
        .with_session(session);
    crate::store::open_event_store(&f.cas_dir).unwrap().record(&event).unwrap();
}

#[test]
fn cas_0cdaa_only_the_recipients_later_authored_action_retires_watchdog_alarms() {
    let _env = env();
    // (case, row source, row target, later activity, alarm still raised)
    let cases: [(&str, &str, &str, fn(&Fixture), bool); 9] = [
        ("recipient reply", "supervisor", "worker", |f| { authored_outbound(f, &actor("worker-id")); }, false),
        ("recipient task note", "supervisor", "worker", |f| task_note(f, "worker-id"), false),
        ("self reminder", "worker", "worker", |_| {}, false),
        ("transport alone", "supervisor", "worker", |_| {}, true),
        ("daemon outbound with worker source", "supervisor", "worker", |f| { authored_outbound(f, &cas_store::QueueOrigin::Daemon); }, true),
        ("forged outbound actor", "supervisor", "worker", |f| { authored_outbound(f, &actor("different-worker-id")); }, true),
        ("another author's note", "supervisor", "worker", |f| task_note(f, "another-worker-id"), true),
        (
            "outbound before handoff",
            "supervisor",
            "worker",
            |f| {
                let outbound = authored_outbound(f, &actor("worker-id"));
                let at = (chrono::Utc::now() - chrono::Duration::seconds(700)).to_rfc3339();
                rusqlite::Connection::open(f.cas_dir.join("cas.db"))
                    .unwrap()
                    .execute("UPDATE prompt_queue SET created_at=? WHERE id=?", rusqlite::params![at, outbound])
                    .unwrap();
            },
            true,
        ),
        (
            // Worker action fallback must not hide unread deliveries to the supervisor.
            "unrelated supervisor activity",
            "worker",
            SUPERVISOR,
            |f| {
                f.queue
                    .enqueue_urgent_with_outcome(
                        "supervisor",
                        "worker",
                        "A different task update",
                        Some(SESSION),
                        Some("status"),
                        Some(cas_store::NotificationPriority::Normal),
                        false,
                        Some(&actor(&format!("{SUPERVISOR}-id"))),
                    )
                    .unwrap();
            },
            true,
        ),
    ];
    for (case, source, target, activity, alarms) in cases {
        let mut f = Fixture::new();
        let id = handed_off_row(&f, source, target);
        activity(&f);
        let first = f.scan();
        assert_eq!(!first.is_empty(), alarms, "{case}: {first:?}");
        let report = f.queue.message_delivery_report(id).unwrap().unwrap();
        assert!(report.confirmed_at.is_none(), "{case}: alarm retirement must not invent acknowledgement");
        if case == "transport alone" {
            assert_eq!(f.scan().len(), first.len(), "the genuine unread episode alerts once");
        }
        if case == "recipient reply" {
            let slos = cas_store::communication_slo::read_session(&f.cas_dir, SESSION, chrono::Utc::now()).unwrap();
            assert_eq!((slos.false_abandon, slos.duplicate_body_reads), (0, 0));
        }
    }
}

#[test]
fn cas_0cdaa_action_after_scan_cancels_durable_sender_bounce() {
    let _env = env();
    let mut f = Fixture::new();
    let id = handed_off_row(&f, "supervisor", "worker");
    let context = message_attention::AttentionInputs::from_daemon(&f.daemon);
    let candidates = super::super::poison_sweep_jobs::read_bounces(
        &context,
        &HashMap::new(),
        cas_mux::SupervisorCli::Codex,
    );
    assert!(candidates.iter().any(|candidate| candidate.row.id == id));
    authored_outbound(&f, &actor("worker-id"));
    let bounce = f
        .queue
        .enqueue_delivery_stalled_bounce(id, SESSION, "stale scan", "stale alarm")
        .unwrap();
    assert!(
        bounce.is_none(),
        "atomic bounce write must revalidate recipient action after the scan"
    );
    assert!(f.scan().is_empty());
}

#[test]
fn cas_0cdaa_forged_outbound_source_is_not_recipient_action() {
    let _env = env();
    let mut f = Fixture::new();
    handed_off_row(&f, "supervisor", "worker");
    authored_outbound(
        &f,
        &cas_store::QueueOrigin::RegisteredAgent {
            agent_id: "different-worker-id".into(),
        },
    );
    assert!(
        !f.scan().is_empty(),
        "worker source text cannot replace authenticated actor identity"
    );
}

#[test]
fn cas_0cdaa_real_task_claim_and_release_retire_watchdog_alarms() {
    let _env = env();
    for release in [false, true] {
        let mut f = Fixture::new();
        crate::store::open_task_store_local(&f.cas_dir)
            .unwrap()
            .add(&Task::new("cas-0cdaa".into(), "Message decision work".into()))
            .unwrap();
        let agents = open_agent_store(&f.cas_dir).unwrap();
        if release {
            agents.try_claim("cas-0cdaa", "worker-id", 1800, None).unwrap();
            // The existing lease predates the message; only its later release
            // may satisfy this case, not the setup claim.
            rusqlite::Connection::open(f.cas_dir.join("cas.db"))
                .unwrap()
                .execute(
                    "UPDATE task_lease_history SET timestamp=? WHERE task_id='cas-0cdaa' AND event_type='claimed'",
                    [(chrono::Utc::now() - chrono::Duration::seconds(1200)).to_rfc3339()],
                )
                .unwrap();
        }
        handed_off_row(&f, "supervisor", "worker");
        if release {
            agents.release_lease("cas-0cdaa", "worker-id").unwrap();
        } else {
            agents.try_claim("cas-0cdaa", "worker-id", 1800, None).unwrap();
        }
        assert!(
            f.scan().is_empty(),
            "real recipient task transition satisfies the alarm; release={release}"
        );
    }
}

#[test]
fn cas_6d5c_task_lifecycle_retires_only_the_related_workers_alarms() {
    use crate::mcp::tools::core::task::lifecycle::supervisor_push::{
        LifecycleTransition, emit_task_lifecycle_transition,
    };
    let _env = env();
    for (transition, status, case, actor) in [
        (
            LifecycleTransition::Started,
            TaskStatus::InProgress,
            "current",
            "worker",
        ),
        (
            LifecycleTransition::AwaitingMerge,
            TaskStatus::AwaitingMerge,
            "current",
            "worker",
        ),
        (
            LifecycleTransition::Closed,
            TaskStatus::Closed,
            "current",
            "worker",
        ),
        (
            LifecycleTransition::Closed,
            TaskStatus::Closed,
            "other actor",
            SUPERVISOR,
        ),
        (
            LifecycleTransition::Closed,
            TaskStatus::Closed,
            "before handoff",
            "worker",
        ),
        (
            LifecycleTransition::Closed,
            TaskStatus::Closed,
            "other factory",
            "worker",
        ),
    ] {
        let mut f = Fixture::new();
        let acted = handed_off_row(&f, "supervisor", "worker");
        let unrelated = handed_off_row(&f, "supervisor", "worker");
        rusqlite::Connection::open(f.cas_dir.join("cas.db"))
            .unwrap()
            .execute(
                "UPDATE prompt_queue SET prompt='Continue cas-0cdaa-other' WHERE id=?",
                [unrelated],
            )
            .unwrap();
        let tasks = crate::store::open_task_store_local(&f.cas_dir).unwrap();
        let mut task = Task::new("cas-0cdaa".into(), "Message decision work".into());
        tasks.add(&task).unwrap();
        task.status = status;
        tasks.update(&task).unwrap();
        let agents = open_agent_store(&f.cas_dir).unwrap();
        emit_task_lifecycle_transition(
            crate::store::open_supervisor_queue_store(&f.cas_dir)
                .unwrap()
                .as_ref(),
            Some(f.queue.as_ref()),
            agents.as_ref(),
            &task.id,
            &task.title,
            TaskStatus::Open,
            status,
            actor,
            None,
            transition,
            "lifecycle-after-handoff",
        )
        .unwrap();
        let conn = rusqlite::Connection::open(f.cas_dir.join("cas.db")).unwrap();
        if case == "before handoff" {
            conn.execute(
                "UPDATE supervisor_queue SET created_at=? WHERE event_type='task_lifecycle'",
                [(chrono::Utc::now() - chrono::Duration::seconds(700)).to_rfc3339()],
            )
            .unwrap();
        } else if case == "other factory" {
            conn.execute(
                "UPDATE supervisor_queue SET payload=json_set(payload, '$.factory_session', 'other-factory') WHERE event_type='task_lifecycle'",
                [],
            ).unwrap();
        }
        let retires = case == "current";
        let context = message_attention::AttentionInputs::from_daemon(&f.daemon);
        let bounces = super::super::poison_sweep_jobs::read_bounces(
            &context,
            &HashMap::new(),
            cas_mux::SupervisorCli::Codex,
        );
        assert_eq!(
            !bounces.iter().any(|row| row.row.id == acted),
            retires,
            "{transition:?}/{case}: only related recipient lifecycle retires the alarm"
        );
        assert!(
            bounces.iter().any(|row| row.row.id == unrelated),
            "unrelated work, including a task-id prefix, must still alert"
        );
        let idle = super::super::delivery_probe_jobs::read_idle_unread(&context, SUPERVISOR);
        assert_eq!(
            !idle.iter().any(|row| row.row.id == acted),
            retires,
            "{case}"
        );
        assert!(idle.iter().any(|row| row.row.id == unrelated));
        assert_eq!(
            f.queue
                .enqueue_delivery_stalled_bounce(acted, SESSION, "stale scan", "stale alarm",)
                .unwrap()
                .is_none(),
            retires,
            "atomic bounce write must recheck the lifecycle: {case}"
        );
        assert!(
            !f.scan().is_empty(),
            "genuinely unread unrelated task still alerts"
        );
        assert!(
            f.queue
                .message_delivery_report(acted)
                .unwrap()
                .unwrap()
                .confirmed_at
                .is_none(),
            "alarm suppression must not manufacture a message read receipt"
        );
    }
}

/// cas-badb (GH #1302): an unread message, even one typed into the pane with no
/// turn observed afterwards, reaches its sender and the operator, and the
/// operator is paged once per episode.
#[test]
fn cas_badb_unread_message_escalates_to_sender_and_operator_once() {
    let _env = env();
    let mut f = Fixture::new();
    let id = handed_off_row(&f, "supervisor", "worker");
    f.queue
        .record_recipient_surfaced(id, "worker", cas_store::SurfacingSource::TransportDelivered)
        .unwrap();
    let pages = |f: &Fixture| -> i64 {
        rusqlite::Connection::open(f.cas_dir.join("cas.db"))
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM prompt_queue WHERE source = 'unread-message-watchdog' AND target = 'operator'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    let first = f.scan();
    assert!(
        first.iter().any(|row| row.source == "delivery-watchdog" && row.target == "supervisor"),
        "the sender hears that its message is unread: {first:?}"
    );
    assert_eq!(pages(&f), 1, "the operator is paged with the sender");
    f.scan();
    assert_eq!(pages(&f), 1, "one operator page per unread episode");
}


fn age_all(f: &Fixture) {
    rusqlite::Connection::open(f.cas_dir.join("cas.db"))
        .unwrap()
        .execute(
            "UPDATE prompt_queue SET created_at=?",
            rusqlite::params![(chrono::Utc::now() - chrono::Duration::seconds(900)).to_rfc3339()],
        )
        .unwrap();
}

/// cas-9c34: the supervisor's worktree_merge naming the parked task (even
/// one refused while CI is pending) reads that task's merge request; another
/// task's request still alarms.
#[test]
fn cas_9c34_acting_on_a_merge_request_retires_both_alarms_for_that_task_only() {
    let _env = env();
    let mut f = Fixture::new();
    let request = |task: &str| {
        let envelope = crate::prompt_revalidation::MergeRequestEnvelope {
            task_id: task.into(),
            branch_tip: "delivery-tip".into(),
            target_branch: "epic/v50".into(),
            target_branch_tip: "old-target".into(),
            anchor_tip: None,
            commits_not_on_target_base: 1,
            pr_number: None,
            source_freshness_unavailable: None,
        };
        crate::prompt_revalidation::attach_merge_request_envelope("status: ready", &envelope)
    };
    let acted = f.aged("worker", "supervisor", &request("cas-b51b"));
    let waiting = f.aged("worker", "supervisor", &request("cas-other"));
    let supervisor = open_agent_store(&f.cas_dir).unwrap().get(&format!("{SUPERVISOR}-id")).unwrap();
    assert_eq!(
        crate::prompt_revalidation::ack_merge_requests_acted_on(f.queue.as_ref(), &supervisor, "cas-b51b"),
        1
    );
    let alarms = f.scan();
    assert!(
        // pin: alarms name the exact unread row id; the acted-on one is gone.
        alarms.iter().all(|row| !row.prompt.contains(&format!("message {acted}"))),
        "{alarms:?}"
    );
    assert!(
        // pin: another task's request still names its row in the alarm.
        alarms.iter().any(|row| row.prompt.contains(&format!("message {waiting}"))),
        "{alarms:?}"
    );
}

/// cas-37b8: a store-stamped peer copy and a daemon non-waking lifecycle
/// relay ask nothing; left unread they raise no alarm and stay unacked.
#[test]
fn cas_37b8_fyi_rows_left_unread_raise_no_alarm_and_stay_unacked() {
    let _env = env();
    let mut f = Fixture::new();
    let lifecycle = f
        .queue
        .enqueue_urgent_with_outcome(
            "lifecycle:19821",
            SUPERVISOR,
            "<task-lifecycle transition=\"task_started\" task_id=\"cas-f9b7\" old=\"open\" \
             new=\"in_progress\" actor=\"worker\" notification_id=\"19821\" \
             occurrence=\"2026-10-17T15:17:19.585694327+00:00\">\nTask cas-f9b7 — sync\n\
             Session: provision-test\n</task-lifecycle>",
            Some(SESSION),
            Some("cas-f9b7 started"),
            Some(cas_store::NotificationPriority::Normal),
            false,
            Some(&cas_store::QueueOrigin::Daemon),
        )
        .unwrap()
        .id();
    let body = "cas-f9b7 touches the same store; I will stay read-only.";
    let copy = crate::mcp::tools::service::agent_search_system::message::peer_copy_text(
        "worker", "other-worker", body,
    );
    let peer = f
        .queue
        .enqueue_worker_peer_with_supervisor_copy(
            "worker",
            "other-worker",
            SUPERVISOR,
            body,
            &copy,
            SESSION,
            Some("collision warning"),
            None,
            false,
            Some(&cas_store::QueueOrigin::RegisteredAgent { agent_id: "worker-id".into() }),
        )
        .unwrap();
    // The peer read its own copy; only the supervisor's FYI copy is unread.
    f.queue.ack(peer.recipient_id).unwrap();
    let peer = peer.supervisor_copy_id;
    age_all(&f);
    let alarms = f.scan();
    assert!(
        // pin: the typed worker-attention kind routes the unread alarm.
        alarms.iter().all(|row| !row.prompt.contains("kind=\"supervisor_unread\"")
            && row.source != "delivery-watchdog"),
        "{alarms:?}"
    );
    for id in [lifecycle, peer] {
        assert!(f.queue.queued_prompt(id).unwrap().unwrap().acked_at.is_none(), "row {id}");
    }
}

#[test]
fn cas_afe0_reference_wire_never_becomes_a_body_receipt() {
    let _env = env();
    for cli in ["codex", "claude"] {
        for terminal in [false, true] {
            let mut f = Fixture::new();
            let id = f.aged("worker", "supervisor", "Review the queued delivery details");
            f.queue.record_recipient_transport_delivered(id, SUPERVISOR).unwrap();
            f.queue.mark_transport_delivered(id).unwrap();
            if terminal {
                f.queue.acknowledge_for_recipients(id, &[SUPERVISOR.into()], None).unwrap();
            } else {
                f.queue.poll_unseen_for_recipient(SUPERVISOR, Some(SESSION), 1).unwrap();
            }
            let receipt = f.queue.recipient_receipt(id, SUPERVISOR).unwrap();
            let projected = f.queue.recipient_delivery(id, SUPERVISOR).unwrap().unwrap();
            assert!(!projected.is_full());
            let projected = projected.into_prompt();
            let wire = super::teams_body_fallback_text(&projected);
            // References carry no body provenance or reply hint.
            assert_eq!(wire, projected.prompt);
            let wire = if cli == "codex" {
                super::super::delivery::frame_pty_payload(cas_mux::SupervisorCli::Codex, "worker", &wire)
            } else {
                format!("<teammate-message teammate_id=\"worker\">\n{wire}\n</teammate-message>")
            };
            f.supervisor_turn(cli, &wire);
            f.scan();
            assert_eq!(f.queue.recipient_receipt(id, SUPERVISOR).unwrap(), receipt,
                "{cli}/terminal={terminal}: consuming a notice is not consuming the body");
        }
    }
}

#[test]
fn cas_ad3d_consumed_injection_is_durable_and_transport_only_stays_unread() {
    let _env = env();
    for (cli, claimed, pasted) in [
        ("codex", false, false),
        ("codex", true, false),
        ("claude", false, false),
        ("claude", true, false),
        ("codex", false, true),
        ("codex", true, true),
        ("claude", false, true),
        ("claude", true, true),
    ] {
        let mut f = Fixture::new();
        let id = f.aged(
            "worker",
            "supervisor",
            "Please review the completed delivery",
        );
        // The same body with another id must stay unread: body equality or a
        // later turn alone cannot correlate the delivery.
        let unread = f.aged(
            "worker",
            "supervisor",
            "Please review the completed delivery",
        );
        for row in [id, unread] {
            if claimed {
                f.queue
                    .record_recipient_transport_delivered(row, SUPERVISOR)
                    .unwrap();
            }
            f.queue.mark_transport_delivered(row).unwrap();
        }
        if claimed {
            // Exercise the existing weak probe and its actual stamps. Pane
            // growth without this message's body must never grant a read.
            assert_eq!(
                super::classify_urgent_wake(10, Some(11), Duration::ZERO, Duration::from_secs(1)),
                super::UrgentWakeOutcome::Observed
            );
            FactoryDaemon::consume_urgent_wake_row(f.queue.as_ref(), unread, SUPERVISOR).unwrap();
            assert_eq!(
                f.queue.recipient_receipt(unread, SUPERVISOR).unwrap(),
                Some(cas_store::SurfacingSource::TransportClaimed)
            );
        }
        let row = f.queue.queued_prompt(id).unwrap().unwrap();
        let wire = super::teams_body_fallback_text(&row);
        let wire = if cli == "codex" {
            super::super::delivery::frame_pty_payload(
                cas_mux::SupervisorCli::Codex,
                "worker",
                &wire,
            )
        } else {
            format!("<teammate-message teammate_id=\"worker\">\n{wire}\n</teammate-message>")
        };
        // Captured native paste wrapper, including its attributed closing tag:
        // tests/data/ambient-supervisor-replay.json (UserPromptSubmit).
        let wire = if pasted {
            format!("\n\n<pasted_content id=\"d36a\">\n{wire}\n</pasted_content id=\"d36a\">\n")
        } else {
            wire
        };
        f.supervisor_turn(cli, &wire);
        let alarms = f.scan();
        let report = f.queue.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(
            report.recipient_receipt,
            Some(cas_store::SurfacingSource::ObservedWake),
            "{cli}/claimed={claimed}/pasted={pasted}: consumed wire must get a durable receipt"
        );
        assert!(
            report.confirmed_at.is_none(),
            "consumption is not explicit acknowledgement"
        );
        assert!(
            alarms
                .iter()
                // pin: supervisor_unread names the oldest unread notification here.
                .any(|r| r.prompt.contains(&format!("message {unread},"))),
            "{cli}: the other transport-only message must still alarm"
        );
        assert_eq!(
            f.queue
                .message_delivery_report(unread)
                .unwrap()
                .unwrap()
                .recipient_receipt,
            claimed.then_some(cas_store::SurfacingSource::TransportClaimed)
        );
        assert!(
            f.queue
                .poll_unseen_for_recipient(SUPERVISOR, Some(SESSION), 10)
                .unwrap()
                .iter()
                .all(|r| r.id != id)
        );
    }
}
