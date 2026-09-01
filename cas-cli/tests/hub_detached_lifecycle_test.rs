//! Regression coverage for hub launches from short-lived CLI shells.
//!
//! The hub must own a fresh session/process group and identify the launcher in
//! its durable record, so a worker shell disappearing cannot take it with it.

#![cfg(unix)]

use std::ffi::OsStr;
use std::fs;
use std::path::Path;
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
