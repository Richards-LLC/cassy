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
