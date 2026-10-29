use super::*;
use cas_store::wait_budget;

fn assert_unknown_grace_expires(mode: &str) {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&root);
    let queue = crate::store::open_prompt_queue_store(&root).unwrap();
    let id = queue.enqueue("supervisor", "worker-a", "decision").unwrap();
    let row = queue.queued_prompt(id).unwrap().unwrap();
    if mode == "failed" {
        let agent = cas_types::Agent::new("worker-id".into(), "worker-a".into());
        crate::store::open_agent_store(&root)
            .unwrap()
            .register(&agent)
            .unwrap();
        let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
        conn.execute("UPDATE agents SET name=x'00' WHERE id='worker-id'", [])
            .unwrap();
    }
    let mut preparation = super::super::queue_control_jobs::PromptPreparation::default();
    let mut reads = super::super::prompt_queue_jobs::QueueJobs::inline();
    let first_seen = chrono::Utc::now();
    let expired = first_seen
        + chrono::Duration::from_std(CLAUDE_FIRST_PROMPT_GRACE).unwrap()
        + chrono::Duration::seconds(1);
    let start = Instant::now();
    daemon.queue_jobs = super::super::prompt_queue_jobs::QueueJobs::start(root.clone(), None);
    for (phase, now) in [first_seen, expired].into_iter().enumerate() {
        preparation.wake_clock = Some(now);
        if mode != "missing" {
            let mut raw = Some(super::super::prompt_queue_jobs::QueueSelection {
                rows: vec![row.clone()],
                ..Default::default()
            });
            let mut released = None;
            for attempt in 0..3 {
                released = preparation.advance(
                    &mut reads,
                    raw.take(),
                    root.clone(),
                    "lead".into(),
                    vec![],
                    start + Duration::from_millis(phase as u64 * 1000 + attempt * 101),
                );
                if released.is_some() {
                    break;
                }
            }
            assert!(released.is_some(), "batch released in {mode} phase {phase}");
        }
        daemon.queue_jobs.prompt_preparation = preparation;
        {
            let _lane = super::super::loop_watchdog::enforce_test_lane(
                cas_store::lane_guard::Lane::Control,
            );
            let evidence = daemon.prepared_wake_evidence("worker-a");
            assert_eq!(
                evidence.awaiting_first_prompt,
                phase == 0,
                "{mode}: repeated reads must not renew the first-seen boot grace"
            );
            assert_eq!(
                claude_turn_aware_retry_with_evidence(
                    1,
                    None,
                    true,
                    Some(first_seen),
                    now,
                    evidence
                ),
                if phase == 0 {
                    TurnAwareRetry::Wait
                } else {
                    TurnAwareRetry::UseCadence
                }
            );
            assert_eq!(
                daemon.park_wake_starved_row(&row, "worker-a", 3, true),
                phase == 1,
                "{mode}: exhausted wake budget must escalate after the fixed grace"
            );
        }
        preparation = std::mem::take(&mut daemon.queue_jobs.prompt_preparation);
    }
    // Verify the real asynchronous escalation, not only its boolean policy result.
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let notices = queue.peek_all(10).unwrap();
        if notices.iter().any(|notice| {
            notice.target == daemon.app.supervisor_name()
                && notice.prompt.contains("notification_id=")
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "expired grace must enqueue the supervisor notice"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        daemon
            .queue_jobs
            .drain_for_shutdown(&root, Duration::from_secs(3))
    );
}

async fn assert_replacement_gets_fresh_grace(action: Option<PendingSpawn>, expect_fresh: bool) {
    use super::super::prompt_queue_jobs::{QueueJobs, QueueSelection};
    use super::super::queue_control_jobs::PromptPreparation;
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let queue = crate::store::open_prompt_queue_store(&root).unwrap();
    let id = queue.enqueue("supervisor", "worker-a", "decision").unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&root);
    // Respawn refuses an already-active roster entry before any PTY launch.
    daemon.app.add_worker_name_for_test("worker-a");
    let mut jobs = QueueJobs::inline();
    let mut preparation = PromptPreparation::default();
    let first = chrono::Utc::now();
    preparation.wake_clock = Some(first);
    // Preserve the old prepared input as well as its first-seen map entry.
    assert!(
        preparation
            .advance(
                &mut jobs,
                Some(QueueSelection {
                    rows: vec![queue.queued_prompt(id).unwrap().unwrap()],
                    ..Default::default()
                }),
                root.clone(),
                "lead".into(),
                vec![],
                Instant::now()
            )
            .is_some()
    );
    daemon.queue_jobs.prompt_preparation = preparation;
    {
        let _lane =
            super::super::loop_watchdog::enforce_test_lane(cas_store::lane_guard::Lane::Control);
        assert!(
            daemon
                .prepared_wake_evidence("worker-a")
                .awaiting_first_prompt
        );
        assert!(
            daemon
                .prepared_wake_evidence("worker-b")
                .awaiting_first_prompt
        );
        daemon.queue_jobs.prompt_preparation.wake_clock = Some(
            first
                + chrono::Duration::from_std(CLAUDE_FIRST_PROMPT_GRACE).unwrap()
                + chrono::Duration::seconds(1),
        );
        assert!(
            !daemon
                .prepared_wake_evidence("worker-a")
                .awaiting_first_prompt
        );
    }
    if let Some(action) = action {
        // Active worker / invalid recipe prevent a real harness launch.
        // Refused recycle must not cross the pane replacement boundary.
        daemon.pending_spawns.push_back(action);
        daemon.process_pending_spawns().await;
    } else {
        let _lane =
            super::super::loop_watchdog::enforce_test_lane(cas_store::lane_guard::Lane::Control);
        daemon.queue_jobs.prompt_preparation.forget_pane("worker-a");
    }
    let _lane =
        super::super::loop_watchdog::enforce_test_lane(cas_store::lane_guard::Lane::Control);
    assert_eq!(
        daemon.prepared_wake_evidence("worker-a").awaiting_first_prompt,
        expect_fresh,
        "replacement renews boot grace; refused recycle retains the existing session's grace"
    );
    assert!(
        !daemon
            .prepared_wake_evidence("worker-b")
            .awaiting_first_prompt,
        "forgetting one pane must not renew another pane's expired grace"
    );
}

#[tokio::test]
async fn cas_7b6b_round4_forget_pane_restarts_expired_grace_under_c6() {
    assert_replacement_gets_fresh_grace(None, true).await;
}
#[tokio::test]
async fn cas_7b6b_round4_respawn_forgets_expired_pane_grace() {
    assert_replacement_gets_fresh_grace(Some(PendingSpawn::Respawn("worker-a".into())), true).await;
}
#[tokio::test]
async fn cas_08e6_refused_recycle_preserves_existing_pane_grace() {
    assert_replacement_gets_fresh_grace(Some(PendingSpawn::Recycle {
        request_id: 1,
        name: "worker-a".into(),
        spec: None,
    }), false)
    .await;
}

fn assert_unknown_sender_unresolvable(
    root: &std::path::Path,
    preparation: super::super::queue_control_jobs::PromptPreparation,
) {
    let mut daemon = super::super::provisioning_tests::daemon(root);
    daemon.queue_jobs.prompt_preparation = preparation;
    let _lane =
        super::super::loop_watchdog::enforce_test_lane(cas_store::lane_guard::Lane::Control);
    assert!(
        matches!(
            daemon.wake_sender_for(Some(&cas_store::QueueOrigin::RegisteredAgent {
                agent_id: "worker-id".into()
            })),
            WakeSender::Unresolvable
        ),
        "Unknown fallback must grant no registered sender authority"
    );
}

#[test]
fn cas_7b6b_round3_missing_inputs_grace_expires_and_escalates_under_c6() {
    assert_unknown_grace_expires("missing");
}
#[test]
fn cas_7b6b_round3_unregistered_grace_expires_across_reads_and_escalates_under_c6() {
    assert_unknown_grace_expires("unregistered");
}
#[test]
fn cas_7b6b_round3_failed_read_grace_expires_across_reads_and_escalates_under_c6() {
    assert_unknown_grace_expires("failed");
}

#[test]
fn cas_7b6b_round3_timed_out_wake_reads_release_batch_under_c6() {
    use super::super::jobs::{Criticality, JobSpec, Lane};
    let (_temp, root, batch) = unusable_read_fixture();
    let mut jobs = super::super::prompt_queue_jobs::QueueJobs::start(root.clone(), None);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    jobs.submit_external(
        JobSpec {
            kind: "HeldRead",
            key: "held-read".into(),
            lane: Lane::Blocking,
            criticality: Criticality::Critical,
            generation: 1,
            budget: Duration::from_millis(100),
        },
        move || {
            entered_tx.send(()).unwrap();
            let _ = release_rx.recv();
            Box::new(())
        },
    );
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let mut preparation = super::super::queue_control_jobs::PromptPreparation::default();
    let start = Instant::now();
    let timeout = super::super::factory_jobs::in_flight_timeout(Duration::from_millis(100));
    let mut raw = Some(batch);
    let mut released = None;
    for attempt in 0..=3 {
        let _lane =
            super::super::loop_watchdog::enforce_test_lane(cas_store::lane_guard::Lane::Control);
        released = preparation.advance(
            &mut jobs,
            raw.take(),
            root.clone(),
            "lead".into(),
            vec![],
            start + (timeout + Duration::from_millis(1)) * attempt,
        );
        assert_eq!(
            released.is_some(),
            attempt == 3,
            "release only after exactly three unusable read timeouts"
        );
    }
    release_tx.send(()).unwrap();
    assert!(released.is_some());
    assert!(!preparation.has_pending());
    assert!(preparation.allows_delivery(Instant::now()));
    assert!(jobs.drain_for_shutdown(&root, Duration::from_secs(3)));
}

#[test]
fn cas_7b6b_round2_spawn_request_is_built_only_when_submission_is_possible() {
    use super::super::queue_control_jobs::{SpawnIngress, SpawnPollRequest};
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let writer = super::super::prompt_queue_jobs::tests::TestWriter::start(true);
    let mut jobs = super::super::prompt_queue_jobs::tests::jobs_on(&writer);
    let mut ingress = SpawnIngress::default();
    let mut builds = 0;
    for _ in 0..8 {
        ingress.advance(
            &mut jobs,
            || {
                builds += 1;
                SpawnPollRequest {
                    cas_dir: root.clone(),
                    session: "s".into(),
                    supervisor: "lead".into(),
                    scan_stalls: false,
                    reported: Default::default(),
                }
            },
            Instant::now(),
        );
    }
    assert_eq!(
        builds, 1,
        "held poll must not clone inputs on every control pass"
    );
    ingress.cancel(&mut jobs, "test shutdown", true);
    writer.open();
    assert!(jobs.drain_for_shutdown(&root, Duration::from_secs(5)));
}

#[test]
fn cas_7b6b_round2_spawn_poll_failure_keeps_cas2702_error_diagnostic() {
    #[derive(Clone)]
    struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let queue = crate::store::open_spawn_queue_store(&root).unwrap();
    let id = queue
        .enqueue_spawn(1, &[], false, None, Some("provision-test"), None)
        .unwrap();
    let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
    conn.execute("UPDATE spawn_queue SET action=x'00' WHERE id=?", [id])
        .unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&root);
    let output = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::ERROR)
        .with_writer(move || Capture(sink.clone()))
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        assert!(daemon.enqueue_spawn_requests().is_err())
    });
    let log = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert!(
        log.contains("ERROR") && log.contains("cas-2702"),
        "poll failures remain visible: {log}"
    );
}

fn unusable_read_fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    super::super::prompt_queue_jobs::QueueSelection,
) {
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    crate::store::open_agent_store(&root)
        .unwrap()
        .register(&cas_types::Agent::new(
            "worker-id".into(),
            "worker-a".into(),
        ))
        .unwrap();
    let queue = crate::store::open_prompt_queue_store(&root).unwrap();
    let id = queue
        .enqueue("supervisor", "worker-a", "retained decision")
        .unwrap();
    let batch = super::super::prompt_queue_jobs::QueueSelection {
        rows: vec![queue.queued_prompt(id).unwrap().unwrap()],
        ..Default::default()
    };
    (temp, root, batch)
}

#[test]
fn cas_7b6b_round2_failed_reads_eventually_release_batch_with_unknown_evidence() {
    let (_temp, root, batch) = unusable_read_fixture();
    let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
    conn.execute("UPDATE agents SET name=x'00' WHERE id='worker-id'", [])
        .unwrap();
    let mut jobs = super::super::prompt_queue_jobs::QueueJobs::inline();
    let mut preparation = super::super::queue_control_jobs::PromptPreparation::default();
    let mut raw = Some(batch);
    let mut released = None;
    let mut attempts = 0;
    for n in 0..8 {
        attempts += 1;
        released = preparation.advance(
            &mut jobs,
            raw.take(),
            root.clone(),
            "lead".into(),
            vec![],
            Instant::now() + Duration::from_millis(n * 101),
        );
        if released.is_some() {
            break;
        }
    }
    assert!(
        released.is_some(),
        "persistent read errors must not retain a batch forever"
    );
    assert_eq!(
        preparation.recipient("worker-a").unwrap().tool_call,
        ToolCallEvidence::Unknown
    );
    assert_eq!(attempts, 3, "release after three unusable read attempts");
    assert!(preparation.allows_delivery(Instant::now() + Duration::from_secs(1)));
    assert!(!preparation.has_pending());
    assert_unknown_sender_unresolvable(&root, preparation);
}

#[test]
fn cas_7b6b_round2_slow_reads_eventually_release_batch_off_the_loop() {
    let (_temp, root, batch) = unusable_read_fixture();
    let mut jobs = super::super::prompt_queue_jobs::QueueJobs::start(root.clone(), None);
    let mut preparation = super::super::queue_control_jobs::PromptPreparation::default();
    // Delay the actual background reader, including its real SQLite read.
    preparation.read_delay = Duration::from_millis(275);
    let mut raw = Some(batch);
    let mut released = None;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        {
            let _lane = super::super::loop_watchdog::enforce_test_lane(
                cas_store::lane_guard::Lane::Control,
            );
            released = preparation.advance(
                &mut jobs,
                raw.take(),
                root.clone(),
                "lead".into(),
                vec![],
                Instant::now(),
            );
        }
        if released.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        released.is_some(),
        "consistently >250ms reads must not retain a batch forever"
    );
    assert_eq!(
        preparation.recipient("worker-a").unwrap().tool_call,
        ToolCallEvidence::Unknown
    );
    assert!(
        preparation.allows_delivery(Instant::now() + Duration::from_secs(1)),
        "degraded observations must pass the delivery guard rather than carry the batch again"
    );
    assert!(!preparation.has_pending());
    assert_unknown_sender_unresolvable(&root, preparation);
}

#[tokio::test]
async fn cas_7b6b_round2_deliver_and_park_use_prepared_inputs_under_c6() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("claude-config");
    std::fs::create_dir_all(config.join("teams/provision-test/inboxes")).unwrap();
    let _env = crate::test_support::TestEnvGuard::with_optional_vars(&[(
        "CLAUDE_CONFIG_DIR",
        config.to_str(),
    )]);
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let mut worker = cas_types::Agent::new_with_role(
        "worker-id".into(),
        "worker-a".into(),
        cas_types::AgentRole::Worker,
    );
    worker.factory_session = Some("provision-test".into());
    worker.registered_at = chrono::Utc::now() - chrono::Duration::hours(1);
    crate::store::open_agent_store(&root)
        .unwrap()
        .register(&worker)
        .unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&root);
    super::stall_nudge_rework_tests::make_worker_ready_for(
        &mut daemon,
        cas_mux::SupervisorCli::Claude,
    )
    .await;
    daemon
        .pane_silent_since
        .insert("worker-a".into(), Instant::now());
    daemon.teams = Some(super::super::teams::TeamsManager::new("provision-test"));
    let queue = crate::store::open_prompt_queue_store(&root).unwrap();
    let id = queue
        .enqueue_with_session(
            "supervisor",
            "worker-a",
            "decision to deliver and park",
            "provision-test",
        )
        .unwrap();
    for _ in 0..3 {
        queue.record_wake_gate_decline(id, "busy").unwrap();
    }
    daemon.queue_jobs = super::super::prompt_queue_jobs::QueueJobs::start(root, None);
    daemon.queue_jobs.store(daemon.app.cas_dir()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        {
            let progress = super::super::loop_watchdog::LoopProgress::new();
            let _lane = super::super::loop_watchdog::enforced_daemon_step(
                &progress,
                super::super::loop_watchdog::LoopPhase::PromptQueue,
            );
            daemon.process_prompt_queue().await.unwrap();
        }
        let conn = rusqlite::Connection::open(daemon.app.cas_dir().join("cas.db")).unwrap();
        let parked: Option<String> = conn
            .query_row(
                "SELECT next_attempt_at FROM prompt_queue WHERE id=?",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        if parked.is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "actual delivery path must reach the exhausted-budget park"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        queue
            .queued_prompt(id)
            .unwrap()
            .unwrap()
            .processed_at
            .is_none()
    );
}

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
    // cas-44528: gated, so the poll cannot finish inside
    // enqueue_spawn_requests; its trailing collect would hand the batch to
    // control, and these tests are about a completion control has not taken.
    let writer = super::super::prompt_queue_jobs::tests::TestWriter::start(true);
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
    writer.open();
    // cas-c4b3: ran() records job entry before poll commits processed_at.
    // Wait for the durable dequeue, without collecting its result on control.
    settle_until(|| queue.peek(10).unwrap().is_empty());
    assert!(
        queue.peek(10).unwrap().is_empty(),
        "fixture must cross the destructive poll"
    );
    assert!(
        daemon.pending_spawns.is_empty(),
        "fixture must leave the dequeued batch unconsumed by control"
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
    settle_until(|| {
        crate::store::open_prompt_queue_store(daemon.app.cas_dir())
            .unwrap()
            .peek_all(20)
            .unwrap()
            .iter()
            .any(|row| {
                row.target == daemon.app.supervisor_name()
                    && row.prompt.contains(&id.to_string())
                    && row.prompt.contains("re-issue")
            })
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
        let progress = super::super::loop_watchdog::LoopProgress::new();
        let _lane = super::super::loop_watchdog::enforced_daemon_step(
            &progress,
            super::super::loop_watchdog::LoopPhase::SpawnQueue,
        );
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
        let _lane =
            super::super::loop_watchdog::enforce_test_lane(cas_store::lane_guard::Lane::Control);
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
        let _lane =
            super::super::loop_watchdog::enforce_test_lane(cas_store::lane_guard::Lane::Control);
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
        let _lane =
            super::super::loop_watchdog::enforce_test_lane(cas_store::lane_guard::Lane::Control);
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
        let _lane =
            super::super::loop_watchdog::enforce_test_lane(cas_store::lane_guard::Lane::Control);
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
        let _lane =
            super::super::loop_watchdog::enforce_test_lane(cas_store::lane_guard::Lane::Control);
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
        let _lane =
            super::super::loop_watchdog::enforce_test_lane(cas_store::lane_guard::Lane::Control);
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
        !preparation.allows_delivery(later),
        "expired evidence cannot authorize transport"
    );
    std::thread::sleep(Duration::from_millis(110));
    assert!(
        preparation
            .advance(
                &mut jobs,
                batch(),
                root,
                "lead".into(),
                vec![],
                Instant::now()
            )
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
    let mut director = super::super::director_refresh::DirectorRefresh::new();
    director.has_finished(&mut jobs);
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

#[test]
fn cas_7b6b_failed_wake_read_retains_raw_batch_until_recovery() {
    use super::super::prompt_queue_jobs::{QueueJobs, QueueSelection};
    use super::super::queue_control_jobs::PromptPreparation;
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    crate::store::open_agent_store(&root)
        .unwrap()
        .register(&cas_types::Agent::new(
            "worker-id".into(),
            "worker-a".into(),
        ))
        .unwrap();
    let queue = crate::store::open_prompt_queue_store(&root).unwrap();
    let id = queue
        .enqueue("supervisor", "worker-a", "retained decision")
        .unwrap();
    let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
    conn.execute("UPDATE agents SET name=x'00' WHERE id='worker-id'", [])
        .unwrap();
    let mut jobs = QueueJobs::inline();
    let mut preparation = PromptPreparation::default();
    let batch = QueueSelection {
        rows: vec![queue.queued_prompt(id).unwrap().unwrap()],
        ..Default::default()
    };
    assert!(
        preparation
            .advance(
                &mut jobs,
                Some(batch),
                root.clone(),
                "lead".into(),
                vec![],
                Instant::now()
            )
            .is_none()
    );
    assert!(
        preparation.has_pending(),
        "failed evidence read must retain selected work"
    );
    assert!(
        queue
            .queued_prompt(id)
            .unwrap()
            .unwrap()
            .processed_at
            .is_none()
    );
    conn.execute("UPDATE agents SET name='worker-a' WHERE id='worker-id'", [])
        .unwrap();
    let ready = preparation
        .advance(
            &mut jobs,
            None,
            root,
            "lead".into(),
            vec![],
            Instant::now() + Duration::from_secs(1),
        )
        .expect("recovered read returns the held batch");
    assert_eq!(ready.rows[0].id, id);
    assert!(!preparation.has_pending());
}

#[test]
fn cas_7b6b_wake_inputs_prefer_active_registration_over_newer_stale_namesake() {
    use super::super::prompt_queue_jobs::{QueueJobs, QueueSelection};
    use super::super::queue_control_jobs::PromptPreparation;
    let temp = tempfile::tempdir().unwrap();
    let root = crate::store::init_cas_dir(temp.path()).unwrap();
    let agents = crate::store::open_agent_store(&root).unwrap();
    let now = chrono::Utc::now();
    for (id, status, cli, age) in [
        ("active-id", cas_types::AgentStatus::Active, "claude", 2),
        ("stale-id", cas_types::AgentStatus::Stale, "codex", 1),
    ] {
        let mut agent = cas_types::Agent::new_with_role(
            id.into(),
            "worker-a".into(),
            cas_types::AgentRole::Worker,
        );
        agent.status = status;
        agent.registered_at = now - chrono::Duration::seconds(age);
        agent.metadata.insert("worker_cli".into(), cli.into());
        agent.metadata.insert(
            "clone_path".into(),
            temp.path().to_string_lossy().into_owned(),
        );
        agent.metadata.insert(
            "worker_account_dir".into(),
            temp.path().join(id).to_string_lossy().into_owned(),
        );
        agents.register(&agent).unwrap();
    }
    let queue = crate::store::open_prompt_queue_store(&root).unwrap();
    let id = queue.enqueue("supervisor", "worker-a", "decision").unwrap();
    let mut jobs = QueueJobs::inline();
    let mut preparation = PromptPreparation::default();
    assert!(
        preparation
            .advance(
                &mut jobs,
                Some(QueueSelection {
                    rows: vec![queue.queued_prompt(id).unwrap().unwrap()],
                    ..Default::default()
                }),
                root,
                "lead".into(),
                vec![],
                Instant::now()
            )
            .is_some()
    );
    assert!(
        preparation
            .recipient("worker-a")
            .unwrap()
            .wake_evidence(now)
            .awaiting_first_prompt,
        "GH #160: use the Active Claude session rather than a newer Stale Codex namesake"
    );
}
