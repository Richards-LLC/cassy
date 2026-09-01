//! Regression coverage for hub launches from short-lived CLI shells.
//!
//! The hub must own a fresh session/process group and identify the launcher in
//! its durable record, so a worker shell disappearing cannot take it with it.

#![cfg(unix)]

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn cas_command(home: &Path, path: &OsStr) -> Command {
    let mut command = Command::new(cas::test_paths::cas_binary());
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", path)
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    command
}
fn private_home() -> TempDir {
    let parent = std::env::temp_dir().canonicalize().unwrap();
    let home = tempfile::tempdir_in(parent).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(home.path(), fs::Permissions::from_mode(0o700)).unwrap();
    home
}

#[cfg(target_os = "linux")]
fn own_cgroup_dir() -> PathBuf {
    let path = fs::read_to_string("/proc/self/cgroup")
        .unwrap()
        .lines()
        .find_map(|line| {
            let (hierarchy, rest) = line.split_once(":")?;
            let (controllers, path) = rest.split_once(":")?;
            (hierarchy == "0" && controllers.is_empty())
                .then(|| PathBuf::from("/sys/fs/cgroup").join(path.trim_start_matches('/')))
        })
        .expect("test runner cgroup v2 path");
    assert!(path.is_dir(), "test runner cgroup is a directory: {path:?}");
    path
}

#[cfg(target_os = "linux")]
fn cgroup_delegation_available() -> bool {
    let own = own_cgroup_dir();
    let parent = own
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| name.starts_with("cas-worker-"))
        .and_then(|_| own.parent())
        .map(Path::to_path_buf)
        .unwrap_or(own);
    if !parent.join("cgroup.controllers").exists() {
        return false;
    }

    // Match production's writable_scope_parent probe: creating and removing
    // a child is the proof that this cgroup tree is delegated to the runner.
    let probe = parent.join(format!(
        ".cas-b2c4-containment-probe-{}",
        std::process::id()
    ));
    match fs::create_dir(&probe) {
        Ok(()) => {
            let _ = fs::remove_dir(&probe);
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_dir(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(target_os = "linux")]
fn exited_pid() -> u32 {
    let mut child = Command::new("true")
        .spawn()
        .expect("spawn exited-pid helper");
    let pid = child.id();
    child.wait().expect("wait exited-pid helper");
    pid
}

#[cfg(target_os = "linux")]
fn write_stale_record(home: &Path, cgroup: PathBuf) {
    use cas::hub::{HubProcessRecord, HubRuntimePaths};

    let paths = HubRuntimePaths::new(home.join(".cas/hub"));
    paths
        .write_process_record(&HubProcessRecord {
            pid: exited_pid(),
            sid: None,
            pgid: None,
            bind: "127.0.0.1".to_owned(),
            port: 1,
            version: "stale-test".to_owned(),
            started_at: "2026-09-01T00:00:00Z".to_owned(),
            cgroup: Some(cgroup),
            launched_by: Some("test".to_owned()),
            launched_at: None,
            public_url: None,
            tailscale_serve_port: None,
            tailscale_cli: None,
            transport_warning: None,
        })
        .expect("write stale hub record");
}

#[cfg(target_os = "linux")]
fn mark_live_record_stale(home: &Path, cgroup: PathBuf) {
    use cas::hub::HubRuntimePaths;

    let paths = HubRuntimePaths::new(home.join(".cas/hub"));
    let mut record = paths.read_process_record().expect("read live hub record");
    record.version = "stale-test".to_owned();
    record.cgroup = Some(cgroup);
    paths
        .write_process_record(&record)
        .expect("rewrite live hub record");
}

#[test]
fn hub_record_proves_its_own_session_and_cli_launcher() {
    let home = private_home();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let output = cas_command(home.path(), &path)
        .args(["--json", "hub", "start", "--port", "0"])
        .output()
        .expect("start hub from a short-lived shell");
    assert!(
        output.status.success(),
        "hub start failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record: Value = serde_json::from_slice(&output.stdout).expect("hub start JSON");
    let pid = record["pid"].as_u64().expect("hub pid") as libc::pid_t;
    let sid = record["sid"].as_u64().expect("hub session id") as libc::pid_t;
    let pgid = record["pgid"].as_u64().expect("hub process group id") as libc::pid_t;

    // A setsid child is both a session and process-group leader. This proves
    // the record identifies the hub's own detached session, not its launcher.
    assert_eq!(sid, pid, "hub must be its own session leader");
    assert_eq!(pgid, pid, "hub must be its own process-group leader");
    assert_eq!(record["launched_by"], "cli");
    assert!(
        record["cgroup"].is_null(),
        "a plain-shell hub must not record its inherited cgroup: {record}"
    );

    let stop = cas_command(home.path(), &path)
        .args(["--json", "hub", "stop"])
        .output()
        .expect("stop detached hub");
    assert!(
        stop.status.success(),
        "hub stop failed: {}",
        String::from_utf8_lossy(&stop.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn factory_worker_hub_record_uses_its_joined_server_scope() {
    let home = private_home();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let session = format!("cas-1187-worker-{}", std::process::id());
    let start = cas_command(home.path(), &path)
        .env("CAS_AGENT_ROLE", "worker")
        .env("CAS_FACTORY_SESSION", &session)
        .args(["--json", "hub", "start", "--port", "0"])
        .output()
        .expect("start hub from a factory worker shell");
    assert!(
        start.status.success(),
        "worker hub start failed: {}",
        String::from_utf8_lossy(&start.stderr)
    );
    let record: Value = serde_json::from_slice(&start.stdout).expect("worker hub start JSON");
    if cgroup_delegation_available() {
        let cgroup = record["cgroup"].as_str().expect("worker hub record cgroup");
        assert!(
            cgroup.contains("/cas-server-") && cgroup.ends_with("-hub"),
            "worker hub must record its joined cas-server scope: {cgroup}"
        );
    } else {
        assert!(
            record["cgroup"].is_null(),
            "worker hub must omit cgroup when delegation is unavailable: {record}"
        );
    }

    let stop = cas_command(home.path(), &path)
        .env("CAS_AGENT_ROLE", "worker")
        .env("CAS_FACTORY_SESSION", &session)
        .args(["--json", "hub", "stop"])
        .output()
        .expect("stop worker hub");
    assert!(
        stop.status.success(),
        "worker hub stop failed: {}",
        String::from_utf8_lossy(&stop.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn hub_stop_refuses_a_recorded_cgroup_owned_by_the_caller() {
    let home = private_home();
    write_stale_record(home.path(), own_cgroup_dir());
    let path = std::env::var_os("PATH").unwrap_or_default();

    let stop = cas_command(home.path(), &path)
        .args(["--json", "hub", "stop"])
        .output()
        .expect("stop stale hub record with caller cgroup");
    assert!(
        stop.status.success(),
        "hub stop failed: {}",
        String::from_utf8_lossy(&stop.stderr)
    );
    let output: Value = serde_json::from_slice(&stop.stdout).expect("hub stop JSON");
    assert_eq!(output["stopped"], true);
    assert!(
        std::fs::metadata("/proc/self").is_ok(),
        "the test process must survive refused cgroup teardown"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn hub_start_refuses_a_stale_record_cgroup_owned_by_the_caller() {
    let home = private_home();
    write_stale_record(home.path(), own_cgroup_dir());
    let path = std::env::var_os("PATH").unwrap_or_default();

    let start = cas_command(home.path(), &path)
        .args(["--json", "hub", "start", "--port", "0"])
        .output()
        .expect("start after stale hub record with caller cgroup");
    assert!(
        start.status.success(),
        "hub start failed: {}",
        String::from_utf8_lossy(&start.stderr)
    );
    let record: Value = serde_json::from_slice(&start.stdout).expect("hub start JSON");
    assert!(
        record["cgroup"].is_null(),
        "plain-shell replacement must not record its inherited cgroup: {record}"
    );

    let stop = cas_command(home.path(), &path)
        .args(["--json", "hub", "stop"])
        .output()
        .expect("stop replacement hub");
    assert!(stop.status.success());
}

#[cfg(target_os = "linux")]
#[test]
fn post_swap_restart_refuses_a_recorded_cgroup_owned_by_the_caller() {
    let home = private_home();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let initial = cas_command(home.path(), &path)
        .args(["--json", "hub", "start", "--port", "0"])
        .output()
        .expect("start hub for stale post-swap restart");
    assert!(
        initial.status.success(),
        "initial hub start failed: {}",
        String::from_utf8_lossy(&initial.stderr)
    );
    let initial_record: Value = serde_json::from_slice(&initial.stdout).expect("initial JSON");
    mark_live_record_stale(home.path(), own_cgroup_dir());

    let restart = cas_command(home.path(), &path)
        .args(["--json", "update", "--post-swap", "--from", "3.8.0"])
        .output()
        .expect("run post-swap stale hub restart");
    assert!(
        restart.status.success(),
        "post-swap restart failed: {}",
        String::from_utf8_lossy(&restart.stderr)
    );
    let replacement = cas::hub::HubRuntimePaths::new(home.path().join(".cas/hub"))
        .read_process_record()
        .expect("read replacement hub record");
    assert_ne!(
        replacement.pid,
        initial_record["pid"].as_u64().unwrap() as u32
    );
    assert!(replacement.cgroup.is_none());
    assert!(std::fs::metadata("/proc/self").is_ok());

    let stop = cas_command(home.path(), &path)
        .args(["--json", "hub", "stop"])
        .output()
        .expect("stop replacement hub");
    assert!(stop.status.success());
}
