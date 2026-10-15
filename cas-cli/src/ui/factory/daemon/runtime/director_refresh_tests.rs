//! cas-ee9ab: a slow store never extends a daemon loop pass.
use super::super::super::imports::*;
use super::DirectorRefresh;

/// The pass latency #1165 asks for.
const PASS_LATENCY_LIMIT: Duration = Duration::from_millis(100);
/// The injected store delay.
const SLOW_READ: Duration = Duration::from_millis(500);

#[tokio::test]
async fn a_slow_director_read_never_extends_a_loop_pass_cas_ee9ab() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
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
    let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
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
        let running = refresh
            .jobs()
            .and_then(|jobs| jobs.lane_stats(super::super::jobs::Lane::Read).running);
        if running.is_some_and(|job| job.kind == super::super::factory_jobs::DIRECTOR_SNAPSHOT) {
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

    let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
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
        if let Some(batch) = refresh.take_deliverability().pop() {
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
