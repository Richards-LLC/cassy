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
