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
    let id = f.aged(source, target, "Review passed; wait for CI and merge notification");
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
            "worker", SUPERVISOR, "Recorded the decision; waiting for the merge", Some(SESSION),
            Some("status"), Some(cas_store::NotificationPriority::Normal), false, Some(origin),
        )
        .unwrap()
        .id()
}

#[test]
fn cas_0cdaa_recipient_authored_reply_retires_both_watchdog_alarms() {
    let _env = env();
    let mut f = Fixture::new();
    let id = handed_off_row(&f, "supervisor", "worker");
    authored_outbound(&f, &cas_store::QueueOrigin::RegisteredAgent { agent_id: "worker-id".into() });
    assert!(f.scan().is_empty(), "later recipient-authored reply must retire both unread alarms");
    let report = f.queue.message_delivery_report(id).unwrap().unwrap();
    assert!(report.confirmed_at.is_none(), "alarm retirement must not invent message acknowledgement");
    let slos = cas_store::communication_slo::read_session(&f.cas_dir, SESSION, chrono::Utc::now()).unwrap();
    assert_eq!(slos.false_abandon, 0);
    assert_eq!(slos.duplicate_body_reads, 0);
}

#[test]
fn cas_0cdaa_recipient_task_note_retires_both_watchdog_alarms() {
    let _env = env();
    let mut f = Fixture::new();
    handed_off_row(&f, "supervisor", "worker");
    let event = Event::new(
        EventType::TaskNoteAdded, EventEntityType::Task, "cas-0cdaa",
        "Decision recorded; waiting for CI before re-closing",
    ).with_session("worker-id");
    crate::store::open_event_store(&f.cas_dir).unwrap().record(&event).unwrap();
    assert!(f.scan().is_empty(), "recipient's later task note is implicit action evidence");
}

#[test]
fn cas_0cdaa_self_reminder_never_raises_sender_or_supervisor_alarm() {
    let _env = env();
    let mut f = Fixture::new();
    handed_off_row(&f, "worker", "worker");
    assert!(f.scan().is_empty(), "self-addressed reminder is not a stalled inter-agent delivery");
}

#[test]
fn cas_0cdaa_transport_without_recipient_action_still_alerts_once() {
    let _env = env();
    let mut f = Fixture::new();
    handed_off_row(&f, "supervisor", "worker");
    let first = f.scan();
    assert!(!first.is_empty(), "transport alone must not disarm the watchdog");
    let again = f.scan();
    assert_eq!(again.len(), first.len(), "the genuine unread episode alerts once");
}

#[test]
fn cas_0cdaa_daemon_outbound_is_not_recipient_action() {
    let _env = env();
    let mut f = Fixture::new();
    handed_off_row(&f, "supervisor", "worker");
    authored_outbound(&f, &cas_store::QueueOrigin::Daemon);
    assert!(!f.scan().is_empty(), "daemon reminder carrying a worker source is not authored activity");
}

#[test]
fn cas_0cdaa_pre_handoff_outbound_does_not_retire_unread_alarm() {
    let _env = env();
    let mut f = Fixture::new();
    let id = handed_off_row(&f, "supervisor", "worker");
    let outbound = authored_outbound(&f, &cas_store::QueueOrigin::RegisteredAgent { agent_id: "worker-id".into() });
    rusqlite::Connection::open(f.cas_dir.join("cas.db")).unwrap().execute(
        "UPDATE prompt_queue SET created_at=? WHERE id=?",
        rusqlite::params![(chrono::Utc::now() - chrono::Duration::seconds(700)).to_rfc3339(), outbound],
    ).unwrap();
    assert!(!f.scan().is_empty(), "outbound before handoff cannot have reacted to the message");
    assert!(f.queue.message_delivery_report(id).unwrap().unwrap().confirmed_at.is_none());
}
