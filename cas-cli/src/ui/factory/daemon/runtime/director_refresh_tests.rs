//! cas-ee9ab: a slow store never extends a daemon loop pass.
use super::super::super::imports::*;
use super::DirectorRefresh;

/// A test daemon on real job lanes (the provisioning fixture runs queue jobs
/// inline, which would put every job back on the loop).
fn daemon_on_lanes(cas_dir: &std::path::Path) -> FactoryDaemon {
    let mut daemon = super::super::provisioning_tests::daemon(cas_dir);
    daemon.queue_jobs = super::super::prompt_queue_jobs::QueueJobs::start(
        cas_dir.to_path_buf(),
        daemon.app.project_scope(),
    );
    daemon
}

/// The pass latency #1165 asks for.
const PASS_LATENCY_LIMIT: Duration = Duration::from_millis(100);
/// The injected store delay.
const SLOW_READ: Duration = Duration::from_millis(500);

#[tokio::test]
async fn a_slow_director_read_never_extends_a_loop_pass_cas_ee9ab() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let progress = super::super::loop_watchdog::LoopProgress::new();
    let store_worker = super::super::store_worker::global();
    let mut refresh = DirectorRefresh::new().with_read_delay(SLOW_READ);

    // Written before the refresh is due: the first snapshot must carry it.
    let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
    store
        .add(&cas_types::Task::new(
            "cas-ee9a".into(),
            "visible after one refresh".into(),
        ))
        .unwrap();

    let started = Instant::now();
    let mut slowest = Duration::ZERO;
    let mut due = true;
    let mut visible_after = None;
    while started.elapsed() < Duration::from_secs(10) {
        let pass = Instant::now();
        daemon
            .director_refresh_pass(&mut refresh, due, store_worker, &progress)
            .await;
        slowest = slowest.max(pass.elapsed());
        due = false;
        if daemon
            .app
            .director_data()
            .ready_tasks
            .iter()
            .any(|task| task.id == "cas-ee9a")
        {
            visible_after = Some(started.elapsed());
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let visible_after = visible_after.expect("the refresh never reached the panel data");
    assert!(
        visible_after >= SLOW_READ,
        "the snapshot appeared before the slow read finished ({visible_after:?})"
    );
    assert!(
        slowest < PASS_LATENCY_LIMIT,
        "a pass waited for the {SLOW_READ:?} store read: slowest pass {slowest:?}"
    );
}

/// spec-4c5b §7 gate test (C2, cas-223f): a 2 s `DirectorSnapshot` job leaves
/// every control pass under 100 ms. The snapshot runs as a job on the jobs
/// runtime's read lane, where the watchdog sees it, and the panels pick it up
/// on a later pass.
#[tokio::test]
async fn flv2_director_snapshot_off_lane() {
    const SNAPSHOT: Duration = Duration::from_secs(2);
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let progress = super::super::loop_watchdog::LoopProgress::new();
    let store_worker = super::super::store_worker::global();
    let mut refresh = DirectorRefresh::new().with_read_delay(SNAPSHOT);
    let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
    store
        .add(&cas_types::Task::new(
            "cas-f1v2".into(),
            "after one snapshot".into(),
        ))
        .unwrap();

    let started = Instant::now();
    let mut slowest = Duration::ZERO;
    let mut due = true;
    let mut snapshot_seen_running = false;
    let mut visible_after = None;
    while started.elapsed() < Duration::from_secs(15) {
        let pass = Instant::now();
        daemon
            .director_refresh_pass(&mut refresh, due, store_worker, &progress)
            .await;
        slowest = slowest.max(pass.elapsed());
        due = false;
        let lanes = daemon.queue_jobs.job_lanes().expect("real job lanes");
        if lanes.lane_stats().into_iter().any(|stats| {
            stats.running.is_some_and(|job| {
                job.lane == super::super::jobs::Lane::Read
                    && job.kind == super::super::factory_jobs::DIRECTOR_SNAPSHOT
            })
        }) {
            snapshot_seen_running = true;
        }
        if daemon
            .app
            .director_data()
            .ready_tasks
            .iter()
            .any(|task| task.id == "cas-f1v2")
        {
            visible_after = Some(started.elapsed());
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let visible_after = visible_after.expect("the snapshot never reached the panel data");
    assert!(
        snapshot_seen_running,
        "the snapshot did not run as a DirectorSnapshot job on the read lane"
    );
    assert!(
        visible_after >= SNAPSHOT,
        "the snapshot appeared before the slow job finished ({visible_after:?})"
    );
    assert!(
        slowest < PASS_LATENCY_LIMIT,
        "a pass waited for the {SNAPSHOT:?} snapshot: slowest pass {slowest:?}"
    );
}

/// spec-4c5b §7 gate test (C2/C3, cas-223f): the inject loop's retraction
/// decision is a fresh read, run as a `RevalidateFresh` job, never the cached
/// snapshot and never on the loop. The cached snapshot here is stale both
/// ways: it still shows `worker-a` assigned (which would retract a
/// legitimate idle alert) and does not yet show `worker-b` assigned.
#[tokio::test]
async fn flv2_retraction_reads_fresh() {
    use cas_store::wait_budget;
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
    let mut task_a = cas_types::Task::new("cas-aaaa".into(), "a's work".into());
    task_a.status = cas_types::TaskStatus::InProgress;
    task_a.assignee = Some("worker-a".into());
    store.add(&task_a).unwrap();
    let mut task_b = cas_types::Task::new("cas-bbbb".into(), "b's work".into());
    store.add(&task_b).unwrap();

    let mut daemon = daemon_on_lanes(&cas_dir);
    // The cached snapshot: worker-a assigned, worker-b not.
    daemon.app.refresh_data().unwrap();
    // Then the store moves on: worker-a finished, worker-b was assigned.
    task_a.status = cas_types::TaskStatus::Closed;
    store.update(&task_a).unwrap();
    task_b.status = cas_types::TaskStatus::InProgress;
    task_b.assignee = Some("worker-b".into());
    store.update(&task_b).unwrap();

    let idle_alert = |worker: &str| crate::ui::factory::director::Prompt {
        target: "supervisor".into(),
        text: format!("{worker} is idle"),
        stall_nudge_task: None,
        retract_worker: None,
        retract_task: None,
        retract_epic: None,
        drop_if_worker_assigned: Some(worker.into()),
        durable_retry: false,
    };
    let mut refresh = DirectorRefresh::new();
    let violations = wait_budget::store_access_violations();
    {
        let _forbid = wait_budget::forbid_store_access();
        refresh
            .start_deliverability_check(
                &daemon.app,
                &mut daemon.queue_jobs,
                vec![idle_alert("worker-a"), idle_alert("worker-b")],
                Vec::new(),
                Instant::now(),
            )
            .expect("the re-check runs as a job");
    }
    assert_eq!(
        wait_budget::store_access_violations(),
        violations,
        "the retraction decision read the store on the loop: {:?}",
        wait_budget::last_store_access_violation()
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    let batch = loop {
        if let Some(batch) = refresh.take_deliverability(&mut daemon.queue_jobs).pop() {
            break batch;
        }
        assert!(Instant::now() < deadline, "the re-check never finished");
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    let verdicts: Vec<(String, bool)> = batch
        .prompts
        .into_iter()
        .map(|(prompt, deliverable)| (prompt.drop_if_worker_assigned.unwrap(), deliverable))
        .collect();
    assert_eq!(
        verdicts,
        vec![
            ("worker-a".to_string(), true),
            ("worker-b".to_string(), false)
        ],
        "decided from the fresh read, not the cached snapshot"
    );
}

/// Review round 1, M2: a snapshot job that dies without a result frees its
/// slot, so the next pass starts a new one instead of waiting forever.
#[tokio::test]
async fn c2_failed_snapshot_frees_its_slot() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
    let mut refresh = DirectorRefresh::new().with_panicking_reads();
    refresh.start_director_read(&daemon.app, &mut daemon.queue_jobs);
    assert!(refresh.take_director_read(&mut daemon.queue_jobs).is_none());
    assert!(
        !refresh.director.busy(),
        "a dead snapshot job must not hold the slot"
    );
    // The next pass starts a fresh one, which now succeeds.
    refresh.panic_reads = false;
    refresh.start_director_read(&daemon.app, &mut daemon.queue_jobs);
    assert!(refresh.take_director_read(&mut daemon.queue_jobs).is_some());
}

/// Review round 1, M2: a delivery read that dies keeps its events: they
/// wait for the next read instead of being lost.
#[tokio::test]
async fn c2_failed_delivery_read_requeues_its_events() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
    let mut refresh = DirectorRefresh::new().with_panicking_reads();
    let event = crate::ui::factory::director::DirectorEvent::WorkerIdle {
        worker: "idle-worker".into(),
        active_task: None,
    };
    refresh
        .start_delivery_read(&daemon.app, &mut daemon.queue_jobs, vec![event])
        .unwrap();
    assert!(refresh.take_delivery_read(&mut daemon.queue_jobs).is_none());
    assert!(!refresh.delivery.busy());
    assert_eq!(refresh.delivery_backlog.len(), 1, "the event waits for the next read");
    refresh.panic_reads = false;
    refresh
        .start_delivery_backlog(&daemon.app, &mut daemon.queue_jobs)
        .unwrap();
    let (events, _) = refresh
        .take_delivery_read(&mut daemon.queue_jobs)
        .expect("the retried read reports");
    assert_eq!(events.len(), 1);
}

/// Review round 1, M4: shutdown never drops prompts waiting for their
/// re-check. A re-check still running past the drain is handed back for the
/// caller to re-check itself.
#[tokio::test]
async fn c2_shutdown_hands_back_pending_rechecks() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let mut refresh = DirectorRefresh::new().with_read_delay(Duration::from_millis(500));
    let alert = crate::ui::factory::director::Prompt {
        target: "supervisor".into(),
        text: "worker-a is idle".into(),
        stall_nudge_task: None,
        retract_worker: None,
        retract_task: None,
        retract_epic: None,
        drop_if_worker_assigned: Some("worker-a".into()),
        durable_retry: false,
    };
    refresh
        .start_deliverability_check(
            &daemon.app,
            &mut daemon.queue_jobs,
            vec![alert.clone()],
            Vec::new(),
            Instant::now(),
        )
        .unwrap();
    let (done, pending) = refresh.drain_deliverability(&mut daemon.queue_jobs, Duration::ZERO);
    assert!(done.is_empty());
    assert_eq!(pending.len(), 1, "the unfinished re-check is handed back");
    assert_eq!(pending[0].prompts[0].text, alert.text);

    refresh
        .start_deliverability_check(
            &daemon.app,
            &mut daemon.queue_jobs,
            vec![alert],
            Vec::new(),
            Instant::now(),
        )
        .unwrap();
    let (done, pending) =
        refresh.drain_deliverability(&mut daemon.queue_jobs, Duration::from_secs(10));
    assert!(pending.is_empty());
    assert_eq!(done.len(), 1, "a re-check that finishes during the drain is delivered");
}

/// Review round 1, M1: the git half of the stall evidence is its own job on
/// the external lane, never part of a read-lane job.
#[test]
fn c2_ci_status_runs_on_the_external_lane() {
    let spec = super::super::factory_jobs::ci_status_spec(1);
    assert_eq!(spec.lane, super::super::jobs::Lane::External);
    assert_eq!(spec.kind, super::super::factory_jobs::CI_STATUS);
    for read in [
        super::super::factory_jobs::director_snapshot_spec(1),
        super::super::factory_jobs::delivery_read_spec(1),
        super::super::factory_jobs::deliverability_spec(1),
    ] {
        assert_eq!(read.lane, super::super::jobs::Lane::Read, "{}", read.key);
    }
}

/// C3 round-3 review: a lane can drop a queued snapshot and then still run
/// the key's coalesced rerun. C2 frees its slot on Dropped and applies a
/// later Done only when its generation is the one in flight.
#[test]
fn c2_dropped_then_done_applies_only_the_current_generation() {
    use super::super::jobs::JobResult;
    use super::super::prompt_queue_jobs::ExternalOutcome;
    let result = |generation, outcome| JobResult {
        kind: super::super::factory_jobs::DIRECTOR_SNAPSHOT,
        key: super::super::factory_jobs::DIRECTOR_SNAPSHOT_KEY.to_string(),
        generation,
        outcome: Ok(outcome),
        elapsed: Duration::ZERO,
        writer_wait: Duration::ZERO,
        over_budget: false,
    };
    let mut refresh = DirectorRefresh::new();
    refresh.director.start(1);
    refresh.accept(result(1, ExternalOutcome::Dropped));
    assert!(!refresh.director.busy(), "Dropped frees the slot");

    // A later rerun for generation 1 reports Done while generation 2 runs.
    refresh.director.start(2);
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let daemon = super::super::provisioning_tests::daemon(&cas_dir);
    let load = || {
        Box::new(super::super::factory_jobs::FactoryJobOutput::DirectorSnapshot(
            daemon.app.director_refresh_request().read(None),
        )) as Box<dyn std::any::Any + Send>
    };
    refresh.accept(result(1, ExternalOutcome::Done(load())));
    assert!(refresh.director.ready.is_none(), "a stale generation is ignored");
    refresh.accept(result(2, ExternalOutcome::Done(load())));
    assert!(refresh.director.ready.is_some(), "the current generation applies");
}

/// Review round 2, M2: events re-queued after a failed delivery read start
/// their next read on the next pass, even when nothing else happens (no
/// refresh due, no other read finished).
#[tokio::test]
async fn c2_requeued_events_start_on_a_quiet_pass() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
    let progress = super::super::loop_watchdog::LoopProgress::new();
    let store_worker = super::super::store_worker::global();
    let mut refresh = DirectorRefresh::new().with_panicking_reads();
    let event = crate::ui::factory::director::DirectorEvent::WorkerIdle {
        worker: "quiet-worker".into(),
        active_task: None,
    };
    refresh
        .start_delivery_read(&daemon.app, &mut daemon.queue_jobs, vec![event])
        .unwrap();
    assert!(refresh.take_delivery_read(&mut daemon.queue_jobs).is_none());
    assert_eq!(refresh.delivery_backlog.len(), 1);
    refresh.panic_reads = false;
    assert!(
        refresh.has_finished(&mut daemon.queue_jobs),
        "waiting events wake the refresh pass"
    );
    // Two quiet passes: the first starts the read, the second delivers it.
    for _ in 0..2 {
        daemon
            .director_refresh_pass(&mut refresh, false, store_worker, &progress)
            .await;
    }
    assert!(refresh.delivery_backlog.is_empty(), "the backlog started");
    assert!(!refresh.delivery.busy(), "and its read was delivered");
}

/// Review round 2, M3: a delivery read without fresh data is retried, and
/// only after repeated failures are its events delivered as detected.
#[test]
fn c2_stale_delivery_read_retries_before_delivering() {
    let mut refresh = DirectorRefresh::new();
    let event = || crate::ui::factory::director::DirectorEvent::WorkerIdle {
        worker: "w".into(),
        active_task: None,
    };
    for attempt in 1..=3 {
        assert!(refresh.retry_stale_delivery(vec![event()]).is_none(), "attempt {attempt}");
    }
    assert_eq!(refresh.delivery_backlog.len(), 3);
    assert_eq!(refresh.retry_stale_delivery(vec![event()]).map(|e| e.len()), Some(1));
    refresh.note_fresh_delivery_read();
    assert!(refresh.retry_stale_delivery(vec![event()]).is_none(), "a fresh read resets the count");
}

/// Review round 2, M3: when the fresh read fails, revalidation never
/// filters an event against the cache; every event is delivered as detected.
#[tokio::test]
async fn c2_failed_fresh_read_never_revalidates_from_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
    let events = vec![
        crate::ui::factory::director::DirectorEvent::WorkerIdle {
            worker: "unknown-worker".into(),
            active_task: None,
        },
        crate::ui::factory::director::DirectorEvent::WorkerIdle {
            worker: "another-unknown".into(),
            active_task: None,
        },
    ];
    let (delivered, _prompts, sweep) = daemon.app.revalidate_and_prompt_for_delivery_from(
        &events,
        crate::ui::factory::app::DeliveryInputs::failed_for_test(),
    );
    assert_eq!(delivered.len(), events.len(), "nothing is filtered from the cache");
    assert!(sweep.is_none(), "no retraction without a fresh read");
}

/// Review round 2, L3: a delivery read that outlives its timeout gives its
/// events to the next read, and its late result is ignored.
#[tokio::test]
async fn c2_expired_delivery_read_requeues_and_ignores_the_late_result() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let mut refresh = DirectorRefresh::new()
        .with_read_delay(Duration::from_millis(400))
        .with_timeout(Duration::from_millis(50));
    let event = crate::ui::factory::director::DirectorEvent::WorkerIdle {
        worker: "slow-worker".into(),
        active_task: None,
    };
    refresh
        .start_delivery_read(&daemon.app, &mut daemon.queue_jobs, vec![event])
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(refresh.take_delivery_read(&mut daemon.queue_jobs).is_none());
    assert!(!refresh.delivery.busy(), "the timed-out read freed its slot");
    assert_eq!(refresh.delivery_backlog.len(), 1, "its event waits for the next read");
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        refresh.take_delivery_read(&mut daemon.queue_jobs).is_none(),
        "the late result is ignored"
    );
}

/// Review round 2, L3: a prompt re-check that outlives its timeout delivers
/// its prompts (uncertainty delivers), and its late verdicts are ignored.
#[tokio::test]
async fn c2_expired_recheck_delivers_and_ignores_the_late_result() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let mut refresh = DirectorRefresh::new()
        .with_read_delay(Duration::from_millis(400))
        .with_timeout(Duration::from_millis(50));
    let alert = crate::ui::factory::director::Prompt {
        target: "supervisor".into(),
        text: "worker-a is idle".into(),
        stall_nudge_task: None,
        retract_worker: None,
        retract_task: None,
        retract_epic: None,
        drop_if_worker_assigned: Some("worker-a".into()),
        durable_retry: false,
    };
    refresh
        .start_deliverability_check(&daemon.app, &mut daemon.queue_jobs, vec![alert], Vec::new(), Instant::now())
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let batches = refresh.take_deliverability(&mut daemon.queue_jobs);
    assert_eq!(batches.len(), 1);
    assert!(batches[0].prompts.iter().all(|(_, deliverable)| *deliverable));
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        refresh.take_deliverability(&mut daemon.queue_jobs).is_empty(),
        "the late verdicts are ignored"
    );
}

/// Review round 2, L3: a real lane drop. With the blocking lane busy and
/// full, the oldest queued health probe is dropped and handed back to the
/// detector to ask again, once.
#[tokio::test]
async fn c2_lane_drop_hands_the_probe_back_once() {
    use std::sync::{Arc, Barrier};
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let started = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let (s, r) = (Arc::clone(&started), Arc::clone(&release));
    daemon.queue_jobs.submit_external(
        super::super::jobs::JobSpec {
            kind: "TestBlocker",
            key: "TestBlocker:c2".into(),
            lane: super::super::jobs::Lane::Blocking,
            criticality: super::super::jobs::Criticality::Critical,
            generation: 1,
            budget: Duration::from_secs(60),
        },
        move || {
            s.wait();
            r.wait();
            Box::new(())
        },
    );
    started.wait();
    let mut refresh = DirectorRefresh::new();
    let requests: Vec<_> = (0..=256)
        .map(|n| crate::ui::factory::director::WorkerHealthRequest {
            worker: format!("w{n}"),
            task: "cas-t".into(),
            threshold_secs: 300,
            delay_secs: 5,
            requested_at: chrono::Utc::now(),
        })
        .collect();
    let refused = refresh.submit_health_probes(&daemon.app, &mut daemon.queue_jobs, requests);
    assert_eq!(
        refused,
        vec![("w0".to_string(), "cas-t".to_string())],
        "the oldest queued probe was dropped and handed back"
    );
    release.wait();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (reports, failed) = refresh.take_health_reports(&mut daemon.queue_jobs);
        assert!(failed.is_empty(), "a dropped probe is handed back once: {failed:?}");
        let _ = reports;
        if refresh.health_in_flight.is_empty() || Instant::now() > deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(refresh.health_in_flight.is_empty(), "every other probe reported");
}
