use super::*;
use crate::ui::factory::daemon::runtime::violet_activity::VioletWakeRuntime;

fn live_session(name: &str) -> SessionMetadata {
    let pid = std::process::id();
    let mut metadata =
        crate::ui::factory::session::create_metadata(name, pid, "supervisor", &[], None, None, None);
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
    assert_eq!(running.capabilities, ["managed_reader_v2", "binding_intake_control_v1"]);
    assert_eq!(running.authority, "local_evidence_not_cloud_attestation");

    // Another live daemon process for the session: the old receipt is not its evidence.
    let mut restarted = live_session("s1");
    restarted.daemon_pid = 1;
    restarted.daemon_pid_starttime = None;
    let unknown = observe(root, &restarted, true).unwrap();
    assert_eq!(unknown.state, ConsumerState::Unknown, "{unknown:?}");
    assert_eq!((unknown.consumer_id, unknown.daemon_build.as_deref()), (None, Some("abc1234")));
    assert!(unknown.detail.contains("ignored a receipt from pid"), "{}", unknown.detail);
    assert_eq!(observe(root, &restarted, false).unwrap().state, ConsumerState::Stale);

    let old = observe(root, &live_session("old"), true).unwrap();
    assert_eq!(old.state, ConsumerState::Unknown);
    assert!(old.detail.contains("build abc1234) wrote no consumer receipt"), "{}", old.detail);
    assert!(observe(root, &live_session("old"), false).is_none());

    // Clean shutdown retires only this process's receipt.
    drop(runtime);
    retire(root, "s1");
    assert_eq!(observe(root, &live_session("s1"), true).unwrap().state, ConsumerState::Unknown);
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
    assert_eq!(off.detail, "push-wake not started (slack.wake_enabled=false)");

    set_wake_enabled(root, true);
    let drift = observe(root, &live_session("off"), true).unwrap();
    assert_eq!((drift.wake_enabled_at_start, drift.wake_enabled_now), (Some(false), true));
    assert!(
        drift.detail.ends_with("the daemon reads it only at start: restart the factory to apply"),
        "{}",
        drift.detail
    );
}
