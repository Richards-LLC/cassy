use super::*;
use cas_types::{
    Agent, AgentRole, AgentStatus, Event, EventEntityType, EventType, Task, TaskStatus,
};

const SESSION: &str = "provision-test";

fn env() -> crate::test_support::TestEnvGuard {
    crate::test_support::TestEnvGuard::with_vars(&[
        ("CAS_FACTORY_SESSION", SESSION),
        ("CAS_AGENT_NAME", "supervisor"),
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
            ("supervisor", AgentRole::Supervisor),
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
fn cas_07ee_closed_or_merged_task_no_longer_needs_attention() {
    let _env = env();
    for merged in [false, true] {
        let mut f = Fixture::new();
        let mut task = Task::new("cas-07ee".into(), "delivery".into());
        task.assignee = Some("worker".into());
        task.status = if merged {
            TaskStatus::AwaitingMerge
        } else {
            TaskStatus::Closed
        };
        if merged {
            task.deliverables.merge_commit = Some("merged-tip".into());
        }
        crate::store::open_task_store_local(&f.cas_dir)
            .unwrap()
            .add(&task)
            .unwrap();
        if merged {
            let envelope = crate::prompt_revalidation::MergeRequestEnvelope {
                task_id: task.id.clone(),
                branch_tip: "delivery-tip".into(),
                target_branch: "main".into(),
                target_branch_tip: "old-target".into(),
                anchor_tip: None,
                commits_not_on_target_base: 1,
                pr_number: None,
                source_freshness_unavailable: None,
            };
            let body = crate::prompt_revalidation::attach_merge_request_envelope(
                "status: ready",
                &envelope,
            );
            f.aged("worker", "supervisor", &body);
        } else {
            f.aged("supervisor", "worker", "cas-07ee merged; re-close now");
        }
        assert!(
            f.scan().is_empty(),
            "the task has already satisfied this message: merged={merged}"
        );
    }
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
fn cas_07ee_informational_approval_does_not_raise_idle_unread() {
    let _env = env();
    for body in [
        "Review PASS. Holding the approved branch until CI is green.",
        "Approved; I will merge on green CI, then tell you to re-close.",
    ] {
        let mut f = Fixture::new();
        f.aged("supervisor", "worker", body);
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
    assert!(f.queue.claim_recipient_transport(id, "supervisor").unwrap());
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
