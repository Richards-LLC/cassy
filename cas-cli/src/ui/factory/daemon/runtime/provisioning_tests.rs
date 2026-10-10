//! Exercise the real pending-spawn consumer/reset with an injected stalled child.
use super::super::imports::*;
use crate::ui::factory::app::provisioning::{ProvisioningCancellation, launch_stalled_for_test};
use std::path::Path;

pub(super) fn daemon(cas_dir: &Path) -> FactoryDaemon {
    let app = FactoryApp::for_test_at(cas_dir.to_path_buf());
    let listener = UnixListener::bind(cas_dir.join("test.sock")).unwrap();
    let gui_listener = UnixListener::bind(cas_dir.join("gui.sock")).unwrap();
    let session_manager = SessionManager::new();
    let merge_sweep = super::merge_sweep::MergeSweepCoordinator::new(cas_dir, "provision-test");
    FactoryDaemon {
        session_name: "provision-test".into(),
        app,
        listener,
        clients: HashMap::new(),
        next_client_id: 0,
        owner_client_id: None,
        owner_last_activity: Instant::now(),
        session_manager,
        shutdown: Arc::new(AtomicBool::new(false)),
        cols: 80,
        rows: 24,
        pending_resize: None,
        pending_resize_at: Instant::now(),
        compact_terminal: None,
        compact_cols: 0,
        compact_rows: 0,
        pending_spawns: VecDeque::new(),
        spawn_task: None,
        spawn_verifications: HashMap::new(),
        cloud_handle: None,
        phone_home: false,
        relay_clients: HashMap::new(),
        pane_watchers: HashMap::new(),
        pane_buffers: HashMap::new(),
        session_summarizer: super::session_summarizer::SessionSummarizer::new(Default::default()),
        gui_listener,
        gui_clients: HashMap::new(),
        next_gui_client_id: 0,
        ws_listener: None,
        ws_clients: HashMap::new(),
        terminal_exchange: Default::default(),
        next_ws_client_id: 0,
        web_pane_sizes: HashMap::new(),
        teams: None,
        notify_rx: None,
        dead_workers: std::collections::HashSet::new(),
        recent_worker_exits: Vec::new(),
        reported_unavailable_workers: std::collections::HashMap::new(),
        last_usage_limit_scan: None,
        open_operator_notices: std::collections::HashMap::new(),
        last_permission_request_scan: None,
        reported_permission_requests: std::collections::HashSet::new(),
        last_commander_mirror_scan: None,
        reported_auth_failed_workers: std::collections::HashMap::new(),
        last_auth_failure_scan: None,
        cancelled_spawns: std::collections::HashSet::new(),
        merge_sweep,
        last_idle_message_times: HashMap::new(),
        lifecycle_redelivery_attempts: HashMap::new(),
        lifecycle_redelivery_counts: HashMap::new(),
        inbox_deferred_writes: std::collections::HashMap::new(),
        urgent_wake_probes: HashMap::new(),
        send_receipts: crate::ui::factory::daemon::runtime::send_dedupe::SendReceipts::default(),
        normal_delivery_probes: HashMap::new(),
        last_pane_output_bytes: HashMap::new(),
        pane_silent_since: HashMap::new(),
        last_prompt_poison_sweep: Some(Instant::now()),
        resumed_epic_ids: std::collections::HashSet::new(),
        spawn_started_at: None,
        spawn_cancellation: None,
        last_spawn_queue_stall_scan: None,
        last_external_wake_scan: None,
        reported_stalled_spawn_requests: std::collections::HashSet::new(),
    }
}

async fn stall(daemon: &mut FactoryDaemon, timeout: Duration) -> Arc<ProvisioningCancellation> {
    let entered = daemon.app.cas_dir().join("entered");
    let late_write = daemon.app.cas_dir().join("late-write");
    let mut command = Command::new("sh");
    command
        .args([
            "-c",
            "touch \"$1\"; (sleep 1; touch \"$2\") & sleep 30 & wait",
            "fixture",
        ])
        .arg(&entered)
        .arg(&late_write);
    let queue = crate::store::open_spawn_queue_store(daemon.app.cas_dir()).unwrap();
    let request_id = queue
        .enqueue_spawn(
            1,
            &["stalled-worker".into()],
            true,
            None,
            Some("provision-test"),
            Some("cas-stalled"),
        )
        .unwrap();
    let accepted = queue.poll("provision-test", 1).unwrap();
    assert_eq!(accepted[0].id, request_id);
    let (handle, stop) = launch_stalled_for_test(command, timeout);
    daemon.app.spawning_count += 1;
    daemon.app.add_pending_worker("stalled-worker".into(), true);
    daemon.spawn_task = Some((
        "stalled-worker".into(),
        Some(request_id),
        None,
        Some("cas-stalled".into()),
        handle,
    ));
    daemon.spawn_started_at = Some(Instant::now());
    daemon.spawn_cancellation = Some(stop.clone());
    let started = Instant::now();
    while !entered.exists() && started.elapsed() < Duration::from_secs(2) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        entered.exists(),
        "fixture must enter provisioning before the action"
    );
    stop
}

fn setup() -> (tempfile::TempDir, FactoryDaemon) {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
    let mut task = cas_types::Task::new("cas-stalled".into(), "stalled task".into());
    task.assignee = Some("stalled-worker".into());
    store.add(&task).unwrap();
    let daemon = daemon(&cas_dir);
    (dir, daemon)
}

async fn drain_failure(daemon: &mut FactoryDaemon) {
    let started = Instant::now();
    while daemon.spawn_task.is_some() && started.elapsed() < Duration::from_secs(3) {
        daemon.process_pending_spawns().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        daemon.spawn_task.is_none(),
        "a single stalled spawn must retire"
    );
    assert_eq!(daemon.app.spawning_count, 0);
    assert!(!daemon.app.is_pending_worker("stalled-worker"));
    assert_eq!(
        crate::store::open_task_store_cached(daemon.app.cas_dir())
            .unwrap()
            .get("cas-stalled")
            .unwrap()
            .assignee,
        None
    );
}

#[tokio::test(flavor = "current_thread")]
async fn stalled_provisioner_keeps_loop_passes_and_reports_timeout() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_dir, mut daemon) = setup();
    stall(&mut daemon, Duration::from_secs(1)).await;
    // The real consumer must return while the preparation is still blocked.
    for _ in 0..3 {
        let started = Instant::now();
        daemon.process_pending_spawns().await;
        assert!(started.elapsed() < Duration::from_millis(100));
    }
    assert!(daemon.spawn_task.is_some());
    drain_failure(&mut daemon).await;
    let queue = crate::store::open_prompt_queue_store(daemon.app.cas_dir()).unwrap();
    assert!(
        queue
            .peek_all(20)
            .unwrap()
            .iter()
            .any(|row| row.prompt.contains("provision_timeout"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn restart_spawn_queue_cancels_stalled_provisioner_and_recovers() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_dir, mut daemon) = setup();
    stall(&mut daemon, Duration::from_secs(30)).await;
    let next_request = crate::store::open_spawn_queue_store(daemon.app.cas_dir())
        .unwrap()
        .enqueue_spawn(
            1,
            &["next-worker".into()],
            false,
            None,
            Some("provision-test"),
            None,
        )
        .unwrap();
    crate::factory_daemon_health::request_reset(
        daemon.app.cas_dir(),
        "provision-test",
        "supervisor",
    )
    .unwrap();
    let reset =
        crate::factory_daemon_health::take_reset(daemon.app.cas_dir(), "provision-test").unwrap();
    let started = Instant::now();
    let report = daemon.apply_spawn_queue_reset(&reset);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(report.contains("stalled-worker"));
    assert!(daemon.spawn_task.is_none());
    assert!(daemon.spawn_cancellation.is_none());
    assert_eq!(daemon.app.spawning_count, 0);
    assert_eq!(
        crate::store::open_task_store_cached(daemon.app.cas_dir())
            .unwrap()
            .get("cas-stalled")
            .unwrap()
            .assignee,
        None
    );
    // A persistent request survives reset and becomes the next provisioner.
    daemon.enqueue_spawn_requests().unwrap();
    daemon.process_pending_spawns().await;
    assert_eq!(daemon.spawn_task.as_ref().unwrap().1, Some(next_request));
    assert_eq!(daemon.spawn_task.as_ref().unwrap().0, "next-worker");
    daemon.cancel_provisioning();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(
        !daemon.app.cas_dir().join("late-write").exists(),
        "reset must kill descendant writers"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn targeted_shutdown_kills_stalled_provisioner_without_retiring_other_names() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_dir, mut daemon) = setup();
    stall(&mut daemon, Duration::from_secs(30)).await;
    let survivor = daemon.app.cas_dir().join("independent-survivor");
    let mut independent = Command::new("sh")
        .args(["-c", "sleep 1; touch \"$1\"", "fixture"])
        .arg(&survivor)
        .spawn()
        .unwrap();
    daemon.pending_spawns.push_back(PendingSpawn::Shutdown {
        request_id: Some(2),
        count: None,
        names: vec!["other-worker".into()],
        force: true,
    });
    daemon.process_pending_spawns().await;
    assert!(
        daemon.spawn_task.is_some(),
        "unrelated shutdown must preserve this generation"
    );
    daemon.pending_spawns.push_back(PendingSpawn::Shutdown {
        request_id: Some(3),
        count: None,
        names: vec!["stalled-worker".into()],
        force: true,
    });
    let started = Instant::now();
    daemon.process_pending_spawns().await;
    assert!(started.elapsed() < Duration::from_millis(500));
    drain_failure(&mut daemon).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(
        !daemon.app.cas_dir().join("late-write").exists(),
        "shutdown must kill descendant writers"
    );
    assert!(independent.wait().unwrap().success());
    assert!(
        survivor.exists(),
        "cancelling a provisioner must preserve other process groups"
    );
    let notices = crate::store::open_prompt_queue_store(daemon.app.cas_dir())
        .unwrap()
        .peek_all(20)
        .unwrap();
    assert!(
        notices
            .iter()
            .any(|row| row.prompt.contains("Spawn cancelled by targeted shutdown"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn daemon_drop_cancels_stalled_provisioner() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_dir, mut daemon) = setup();
    stall(&mut daemon, Duration::from_secs(30)).await;
    // Keep the running handle to prove cancellation rather than its abort.
    let (_, _, _, _, handle) = daemon.spawn_task.take().unwrap();
    drop(daemon);
    let error = tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .unwrap()
        .unwrap()
        .err()
        .unwrap();
    assert!(error.to_string().contains("provision_cancelled"));
}

#[tokio::test(flavor = "current_thread")]
async fn daemon_provisioning_deadline_retires_only_stalled_generation() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (_dir, mut daemon) = setup();
    stall(&mut daemon, Duration::from_secs(30)).await;
    daemon.spawn_started_at = Some(Instant::now() - Duration::from_secs(301));
    daemon.pending_spawns.push_back(PendingSpawn::Named {
        request_id: Some(2),
        name: "next-worker".into(),
        isolate: false,
        spec: None,
        task_id: None,
    });
    daemon.app.spawning_count += 1;
    let started = Instant::now();
    daemon.process_pending_spawns().await;
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(daemon.spawn_task.is_none());
    assert!(daemon.spawn_cancellation.is_none());
    assert_eq!(daemon.app.spawning_count, 1);
    daemon.process_pending_spawns().await;
    assert_eq!(daemon.spawn_task.as_ref().unwrap().0, "next-worker");
    daemon.cancel_provisioning();
}
