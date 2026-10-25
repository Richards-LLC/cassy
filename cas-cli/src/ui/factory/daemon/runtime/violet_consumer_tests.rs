use super::*;
use crate::ui::factory::daemon::runtime::violet_activity::VioletWakeRuntime;

fn live_session(name: &str) -> SessionMetadata {
    let pid = std::process::id();
    let mut metadata = crate::ui::factory::session::create_metadata(
        name,
        pid,
        "supervisor",
        &[],
        None,
        None,
        None,
    );
    metadata.daemon_pid_starttime = crate::mcp::daemon::read_pid_starttime(pid);
    metadata.daemon_build_hash = Some("abc1234".into());
    metadata
}

fn set_wake_enabled(root: &Path, enabled: bool) {
    std::fs::write(
        root.join("config.toml"),
        format!("[slack]\nwake_enabled = {enabled}\n"),
    )
    .unwrap();
}

/// The started daemon's receipt is the running consumer; a live daemon
/// without its own receipt (an old build) is unknown, never inferred.
#[test]
fn cas_ef94_running_consumer_comes_from_the_live_daemon_receipt() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    let runtime = VioletWakeRuntime::start(root, "s1", "supervisor").unwrap();

    let running = observe(root, &live_session("s1"), true).unwrap();
    assert_eq!(running.state, ConsumerState::Running, "{running:?}");
    let expected = format!("{}:s1", cas_types::Agent::get_or_generate_machine_id());
    assert_eq!(running.consumer_id.as_deref(), Some(expected.as_str()));
    assert_eq!(
        running.capabilities,
        ["managed_reader_v2", "binding_intake_control_v1"]
    );
    assert_eq!(running.authority, "local_evidence_not_cloud_attestation");

    // Another live daemon process for the session: the old receipt is not its evidence.
    let mut restarted = live_session("s1");
    restarted.daemon_pid = 1;
    restarted.daemon_pid_starttime = None;
    let unknown = observe(root, &restarted, true).unwrap();
    assert_eq!(unknown.state, ConsumerState::Unknown, "{unknown:?}");
    assert_eq!(
        (unknown.consumer_id, unknown.daemon_build.as_deref()),
        (None, Some("abc1234"))
    );
    assert!(
        unknown.detail.contains("ignored a receipt from pid"),
        "{}",
        unknown.detail
    );
    assert_eq!(
        observe(root, &restarted, false).unwrap().state,
        ConsumerState::Stale
    );

    let old = observe(root, &live_session("old"), true).unwrap();
    assert_eq!(old.state, ConsumerState::Unknown);
    assert!(
        old.detail
            .contains("build abc1234) wrote no consumer receipt"),
        "{}",
        old.detail
    );
    assert!(!old.detail.contains("it predates"), "{}", old.detail);
    assert!(observe(root, &live_session("old"), false).is_none());

    // Clean shutdown retires only this process's receipt.
    drop(runtime);
    retire(root, "s1");
    assert_eq!(
        observe(root, &live_session("s1"), true).unwrap().state,
        ConsumerState::Unknown
    );
}

/// `slack.wake_enabled` is read only at start; a later change is reported.
#[test]
fn cas_ef94_push_wake_off_at_start_reports_config_drift() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    set_wake_enabled(root, false);
    assert!(VioletWakeRuntime::start(root, "off", "supervisor").is_none());

    let off = observe(root, &live_session("off"), true).unwrap();
    assert_eq!(off.state, ConsumerState::PushWakeOff, "{off:?}");
    assert_eq!(off.consumer_id, None);
    assert_eq!(
        off.detail,
        "push-wake not started (slack.wake_enabled=false)"
    );

    set_wake_enabled(root, true);
    let drift = observe(root, &live_session("off"), true).unwrap();
    assert_eq!(
        (drift.wake_enabled_at_start, drift.wake_enabled_now),
        (Some(false), true)
    );
    assert!(
        drift
            .detail
            .ends_with("the daemon reads it only at start: restart the factory to apply"),
        "{}",
        drift.detail
    );
}

/// A successor publishing in the read/unlink window must retain its receipt.
#[test]
fn cas_a6f2_retirement_serializes_with_successor_publication() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    record(root, "s1", true, Some("incumbent"), None, None).unwrap();
    let path = receipt_path(root, "s1").unwrap();
    std::thread::scope(|scope| {
        let (attempted, attempt) = std::sync::mpsc::channel();
        let (published, publication) = std::sync::mpsc::channel();
        let (start, started) = std::sync::mpsc::channel();
        let writer = scope.spawn(move || {
            started.recv().unwrap();
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path.with_extension("lock"))
                .unwrap();
            let blocked = match fs2::FileExt::try_lock_exclusive(&lock) {
                Ok(()) => false,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => true,
                Err(error) => panic!("unexpected lock error: {error}"),
            };
            drop(lock);
            attempted.send(blocked).unwrap();
            record(root, "s1", true, Some("successor"), None, None).unwrap();
            let _ = published.send(());
        });
        retire_after_read(root, "s1", || {
            start.send(()).unwrap();
            if !attempt.recv().unwrap() {
                // On the buggy path the writer publishes before unlink.
                publication.recv().unwrap();
            }
        });
        writer.join().unwrap();
    });
    let path = receipt_path(root, "s1").unwrap();
    assert_eq!(
        read(&path)
            .and_then(|receipt| receipt.consumer_id)
            .as_deref(),
        Some("successor")
    );
    let mut foreign = read(&path).unwrap();
    foreign.pid = 1;
    std::fs::write(&path, serde_json::to_vec(&foreign).unwrap()).unwrap();
    retire(root, "s1");
    assert_eq!(read(&path), Some(foreign));
}

/// A failed rename cleans its temp file and reports this daemon's write failure.
#[test]
fn cas_a6f2_failed_publication_cleans_temp_and_reports_write_failure() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    let path = receipt_path(root, "s1").unwrap();
    std::fs::create_dir_all(&path).unwrap(); // Force rename failure, even as root.
    assert!(
        record(root, "s1", true, Some("consumer"), None, None)
            .unwrap_err()
            .contains("rename")
    );
    let status = observe(root, &live_session("s1"), true).unwrap();
    let files: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !files.iter().any(|name| name.ends_with(".tmp")),
        "{files:?}"
    );
    assert_eq!(status.state, ConsumerState::Unknown);
    assert!(
        status.detail.contains("receipt write failed"),
        "{}",
        status.detail
    );
    let mut other = live_session("s1");
    other.daemon_pid = 1;
    let other = observe(root, &other, true).unwrap();
    assert!(
        !other.detail.contains("receipt write failed"),
        "{}",
        other.detail
    );
    std::fs::remove_dir(&path).unwrap();
    record(root, "s1", true, Some("consumer"), None, None).unwrap();
    retire(root, "s1");
    let retired = observe(root, &live_session("s1"), true).unwrap();
    assert!(
        !retired.detail.contains("receipt write failed"),
        "{}",
        retired.detail
    );
}
