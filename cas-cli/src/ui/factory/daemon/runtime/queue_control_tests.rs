use super::*;
use cas_store::wait_budget;

fn settle_until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(
            Instant::now() < deadline,
            "asynchronous queue operation did not finish"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn dequeued_fixture() -> (tempfile::TempDir, FactoryDaemon, i64) {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&root);
    let writer = super::super::prompt_queue_jobs::tests::TestWriter::start(false);
    daemon.queue_jobs = super::super::prompt_queue_jobs::tests::jobs_on(&writer);
    let queue = crate::store::open_spawn_queue_store(&root).unwrap();
    let id = queue
        .enqueue_spawn(
            1,
            &["late-worker".into()],
            false,
            None,
            Some(&daemon.session_name),
            None,
        )
        .unwrap();
    daemon.enqueue_spawn_requests().unwrap();
    settle_until(|| !writer.ran().is_empty());
    assert!(
        queue.peek(10).unwrap().is_empty(),
        "fixture must cross the destructive poll"
    );
    (temp, daemon, id)
}

#[test]
fn cas_7b6b_reset_accounts_for_dequeued_completion_before_control_applies_it() {
    let (_temp, mut daemon, id) = dequeued_fixture();
    crate::factory_daemon_health::request_reset(
        daemon.app.cas_dir(),
        &daemon.session_name,
        "supervisor",
    )
    .unwrap();
    let reset =
        crate::factory_daemon_health::take_reset(daemon.app.cas_dir(), &daemon.session_name)
            .unwrap();
    daemon.apply_spawn_queue_reset(&reset);
    daemon.enqueue_spawn_requests().unwrap();
    assert!(
        daemon.pending_spawns.is_empty(),
        "a pre-reset dequeue must not launch after reset"
    );
    let queue = crate::store::open_spawn_queue_store(daemon.app.cas_dir()).unwrap();
    settle_until(|| {
        queue
            .recent_spawn_lifecycle(&daemon.session_name, 10)
            .unwrap()
            .iter()
            .any(|row| row.id == id && row.state == cas_store::SpawnLifecycleState::Failed)
    });
}

#[test]
fn cas_7b6b_shutdown_accounts_for_dequeued_completion_before_draining_results() {
    let (_temp, mut daemon, id) = dequeued_fixture();
    assert!(
        daemon
            .queue_jobs
            .drain_for_shutdown(daemon.app.cas_dir(), Duration::from_secs(5))
    );
    let queue = crate::store::open_spawn_queue_store(daemon.app.cas_dir()).unwrap();
    assert!(
        queue
            .recent_spawn_lifecycle(&daemon.session_name, 10)
            .unwrap()
            .iter()
            .any(|row| row.id == id && row.state == cas_store::SpawnLifecycleState::Failed),
        "shutdown must account for a dequeued row whose completion control did not consume"
    );
}

#[test]
fn cas_7b6b_spawn_poll_touches_no_store_on_control() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&root);
    // The daemon's actual asynchronous job lanes; fixture defaults are inline.
    daemon.queue_jobs =
        super::super::prompt_queue_jobs::QueueJobs::start(root, daemon.app.project_scope());
    let before = wait_budget::store_access_violations();
    {
        let _forbid = wait_budget::forbid_store_access();
        for _ in 0..8 {
            daemon.enqueue_spawn_requests().unwrap();
        }
    }
    assert_eq!(
        wait_budget::store_access_violations(),
        before,
        "spawn polling touched the loop store: {:?}",
        wait_budget::last_store_access_violation()
    );
}

#[test]
fn cas_7b6b_prompt_wake_gate_touches_no_store_on_control() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let tasks = crate::store::open_task_store_cached(&root).unwrap();
    let mut task = cas_types::Task::new("cas-gate".into(), "Waiting on supervisor".into());
    task.status = cas_types::TaskStatus::Blocked;
    task.assignee = Some("worker-a".into());
    tasks.add(&task).unwrap();
    let daemon = super::super::provisioning_tests::daemon(&root);
    let sender = WakeSender::Registered {
        role: cas_types::AgentRole::Supervisor,
        name: daemon.app.supervisor_name().into(),
    };
    let before = wait_budget::store_access_violations();
    {
        let _forbid = wait_budget::forbid_store_access();
        let _ = daemon.worker_wait_state_for(&sender, "worker-a");
    }
    assert_eq!(
        wait_budget::store_access_violations(),
        before,
        "prompt wake gate touched the loop store: {:?}",
        wait_budget::last_store_access_violation()
    );
}

#[test]
fn cas_7b6b_held_writer_does_not_block_control_or_duplicate_spawn_requests() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&root);
    let writer = super::super::prompt_queue_jobs::tests::TestWriter::start(true);
    daemon.queue_jobs = super::super::prompt_queue_jobs::tests::jobs_on(&writer);
    let queue = crate::store::open_spawn_queue_store(&root).unwrap();
    for name in ["first", "second"] {
        queue
            .enqueue_spawn(
                1,
                &[name.into()],
                false,
                None,
                Some(&daemon.session_name),
                None,
            )
            .unwrap();
    }
    let before = wait_budget::store_access_violations();
    {
        let _forbid = wait_budget::forbid_store_access();
        for _ in 0..8 {
            daemon.enqueue_spawn_requests().unwrap();
        }
    }
    assert!(daemon.pending_spawns.is_empty());
    assert_eq!(
        queue.peek(10).unwrap().len(),
        2,
        "held writer must leave rows durable"
    );
    writer.open();
    settle_until(|| {
        let _forbid = wait_budget::forbid_store_access();
        daemon.enqueue_spawn_requests().unwrap();
        daemon.pending_spawns.len() == 2
    });
    let names: Vec<_> = daemon
        .pending_spawns
        .iter()
        .map(|pending| match pending {
            PendingSpawn::Named { name, .. } => name.as_str(),
            _ => panic!("named requests must keep their shape"),
        })
        .collect();
    assert_eq!(names, ["first", "second"]);
    assert_eq!(
        writer.ran().len(),
        1,
        "one outstanding destructive poll despite repeated passes"
    );
    assert_eq!(wait_budget::store_access_violations(), before);
}

#[test]
fn cas_7b6b_reset_before_writer_runs_preserves_durable_rows() {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&root);
    let writer = super::super::prompt_queue_jobs::tests::TestWriter::start(true);
    daemon.queue_jobs = super::super::prompt_queue_jobs::tests::jobs_on(&writer);
    let queue = crate::store::open_spawn_queue_store(&root).unwrap();
    let id = queue
        .enqueue_spawn(
            1,
            &["still-queued".into()],
            false,
            None,
            Some(&daemon.session_name),
            None,
        )
        .unwrap();
    daemon.enqueue_spawn_requests().unwrap();
    crate::factory_daemon_health::request_reset(&root, &daemon.session_name, "supervisor").unwrap();
    let reset = crate::factory_daemon_health::take_reset(&root, &daemon.session_name).unwrap();
    daemon.apply_spawn_queue_reset(&reset);
    writer.open();
    settle_until(|| !writer.ran().is_empty());
    assert_eq!(
        queue.peek(10).unwrap()[0].id,
        id,
        "cancelled queued poll must not consume rows"
    );
    settle_until(|| {
        daemon.enqueue_spawn_requests().unwrap();
        daemon.pending_spawns.len() == 1
    });
    assert!(
        queue.peek(10).unwrap().is_empty(),
        "the next poll still drains preserved rows"
    );
}

#[test]
fn cas_7b6b_carried_batch_gets_fresh_authority_and_wait_inputs_off_control() {
    use super::super::prompt_queue_jobs::{QueueJobs, QueueSelection};
    use super::super::queue_control_jobs::PromptPreparation;
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&root);
    let agents = crate::store::open_agent_store(&root).unwrap();
    let lead = cas_types::Agent::new_with_role(
        "lead-id".into(),
        daemon.app.supervisor_name().into(),
        cas_types::AgentRole::Supervisor,
    );
    agents.register(&lead).unwrap();
    agents
        .register(&cas_types::Agent::new_with_role(
            "worker-id".into(),
            "worker-a".into(),
            cas_types::AgentRole::Worker,
        ))
        .unwrap();
    let tasks = crate::store::open_task_store_cached(&root).unwrap();
    let mut task = cas_types::Task::new("cas-gate".into(), "Awaiting decision".into());
    task.status = cas_types::TaskStatus::Blocked;
    task.assignee = Some("worker-a".into());
    tasks.add(&task).unwrap();
    let queue = crate::store::open_prompt_queue_store(&root).unwrap();
    let origin = cas_store::QueueOrigin::RegisteredAgent {
        agent_id: lead.id.clone(),
    };
    let id = queue
        .enqueue_urgent_with_outcome(
            "spoofed display source",
            "worker-a",
            "decision",
            Some(&daemon.session_name),
            None,
            None,
            false,
            Some(&origin),
        )
        .unwrap()
        .id();
    // Claim-only carried batches may have an old/empty registry snapshot.
    let mut raw = Some(QueueSelection {
        rows: vec![queue.queued_prompt(id).unwrap().unwrap()],
        ..Default::default()
    });
    daemon.queue_jobs = QueueJobs::start(root.clone(), daemon.app.project_scope());
    let mut preparation = PromptPreparation::default();
    let before = wait_budget::store_access_violations();
    let mut prepared = None;
    settle_until(|| {
        let _forbid = wait_budget::forbid_store_access();
        prepared = preparation.advance(
            &mut daemon.queue_jobs,
            raw.take(),
            root.clone(),
            daemon.app.supervisor_name().into(),
            vec!["worker-a".into()],
            Instant::now(),
        );
        prepared.is_some()
    });
    assert_eq!(prepared.unwrap().rows[0].id, id);
    daemon.queue_jobs.prompt_preparation = preparation;
    {
        let _forbid = wait_budget::forbid_store_access();
        let sender = daemon.wake_sender_for(Some(&origin));
        assert!(matches!(
            sender,
            WakeSender::Registered {
                role: cas_types::AgentRole::Supervisor,
                ..
            }
        ));
        assert!(
            daemon
                .worker_wait_state_for(&sender, "worker-a")
                .unwrap()
                .task_blocked
        );
        assert_eq!(
            daemon.prepared_tool_call_evidence("worker-a"),
            ToolCallEvidence::Unknown
        );
        assert_eq!(daemon.prepared_mid_turn("worker-a"), None);
        assert!(matches!(
            daemon.wake_sender_for(Some(&cas_store::QueueOrigin::RegisteredAgent {
                agent_id: "missing-id".into()
            })),
            WakeSender::Unresolvable
        ));
    }
    assert_eq!(wait_budget::store_access_violations(), before);
}

#[test]
fn cas_7b6b_wake_snapshot_reuses_fresh_inputs_and_refreshes_after_cadence() {
    use super::super::prompt_queue_jobs::{QueueJobs, QueueSelection};
    use super::super::queue_control_jobs::PromptPreparation;
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let agents = crate::store::open_agent_store(&root).unwrap();
    let lead = cas_types::Agent::new_with_role(
        "lead-id".into(),
        "lead".into(),
        cas_types::AgentRole::Supervisor,
    );
    agents.register(&lead).unwrap();
    let tasks = crate::store::open_task_store_cached(&root).unwrap();
    let mut task = cas_types::Task::new("cas-wait".into(), "waiting".into());
    task.status = cas_types::TaskStatus::Blocked;
    task.assignee = Some("worker-a".into());
    tasks.add(&task).unwrap();
    let queue = crate::store::open_prompt_queue_store(&root).unwrap();
    let origin = cas_store::QueueOrigin::RegisteredAgent {
        agent_id: lead.id.clone(),
    };
    let id = queue
        .enqueue_urgent_with_outcome(
            "lead",
            "worker-a",
            "decision",
            Some("s"),
            None,
            None,
            false,
            Some(&origin),
        )
        .unwrap()
        .id();
    let row = queue.queued_prompt(id).unwrap().unwrap();
    let batch = || {
        Some(QueueSelection {
            rows: vec![row.clone()],
            ..Default::default()
        })
    };
    let mut jobs = QueueJobs::inline();
    let mut preparation = PromptPreparation::default();
    let started = Instant::now();
    assert!(
        preparation
            .advance(
                &mut jobs,
                batch(),
                root.clone(),
                "lead".into(),
                vec![],
                started
            )
            .is_some()
    );
    assert!(preparation.recipient("worker-a").unwrap().wait.task_blocked);
    let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
    conn.execute("UPDATE tasks SET status='open' WHERE id='cas-wait'", [])
        .unwrap();
    // A second batch needs no new store read inside the bounded cadence.
    let before = wait_budget::store_access_violations();
    {
        let _forbid = wait_budget::forbid_store_access();
        assert!(
            preparation
                .advance(
                    &mut jobs,
                    batch(),
                    root.clone(),
                    "lead".into(),
                    vec![],
                    Instant::now()
                )
                .is_some()
        );
    }
    assert_eq!(wait_budget::store_access_violations(), before);
    assert!(preparation.recipient("worker-a").unwrap().wait.task_blocked);
    let later = Instant::now() + Duration::from_secs(1);
    assert!(
        !preparation.is_fresh(later),
        "expired evidence cannot authorize transport"
    );
    assert!(
        preparation
            .advance(&mut jobs, batch(), root, "lead".into(), vec![], later)
            .is_some()
    );
    assert!(!preparation.recipient("worker-a").unwrap().wait.task_blocked);
}

#[test]
fn cas_7b6b_wake_completion_remains_ready_after_another_owner_applies_results() {
    use super::super::jobs::{Criticality, JobSpec, Lane};
    use super::super::prompt_queue_jobs::QueueJobs;
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let mut jobs = QueueJobs::start(root, None);
    jobs.submit_external(
        JobSpec {
            kind: super::super::queue_control_jobs::WAKE_GATE_READ,
            key: "WakeGateRead:delivery".into(),
            lane: Lane::Blocking,
            criticality: Criticality::Refresh,
            generation: 1,
            budget: Duration::from_millis(100),
        },
        || Box::new(()),
    );
    settle_until(|| {
        jobs.apply_completions();
        jobs.has_ready()
    });
    jobs.apply_completions();
    assert!(
        jobs.has_ready(),
        "spawn/director collection must not erase a pending wake completion"
    );
    assert_eq!(
        jobs.take_external_completions_where(
            |kind| kind == super::super::queue_control_jobs::WAKE_GATE_READ
        )
        .len(),
        1
    );
    assert!(!jobs.has_ready());
}
