//! Regression coverage for hub launches from short-lived CLI shells.
//!
//! The hub must own a fresh session/process group and identify the launcher in
//! its durable record, so a worker shell disappearing cannot take it with it.

#![cfg(unix)]

#[path = "support/hub_fixture.rs"]
mod hub_fixture;

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

fn fixture_tailscale(home: &Path, path: &OsStr) -> PathBuf {
    std::env::split_paths(path)
        .map(|dir| dir.join("tailscale"))
        .find(|cli| cli.starts_with(home) && cli.is_file())
        .unwrap_or_else(|| home.join("missing-tailscale"))
}

fn cas_command(home: &Path, path: &OsStr) -> Command {
    let mut command = Command::new(cas::test_paths::cas_binary());
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", path)
        .env("TAILSCALE", fixture_tailscale(home, path))
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    command
}
fn private_home() -> hub_fixture::PrivateHubTempDir {
    hub_fixture::private_hub_tempdir()
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
            tailscale_serve_target: None,
            transport_warning: None,
            nofile_limit: None,
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

#[cfg(unix)]
#[test]
fn hub_restart_json_emits_one_value_with_stop_and_start_outcomes() {
    let home = private_home();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let occupied_listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).expect("reserve port for failed replacement");
    let occupied_port = occupied_listener
        .local_addr()
        .expect("occupied listener address")
        .port();
    let initial = cas_command(home.path(), &path)
        .args([
            "--json",
            "hub",
            "--no-tailscale-serve",
            "start",
            "--port",
            "0",
        ])
        .output()
        .expect("start hub before restart");
    assert!(
        initial.status.success(),
        "hub start failed: {}",
        String::from_utf8_lossy(&initial.stderr)
    );
    let initial: Value = serde_json::from_slice(&initial.stdout).expect("initial hub JSON");

    let restart = cas_command(home.path(), &path)
        .args([
            "--json",
            "hub",
            "--no-tailscale-serve",
            "restart",
            "--port",
            "0",
        ])
        .output()
        .expect("restart detached hub");
    assert!(
        restart.status.success(),
        "hub restart failed: {}",
        String::from_utf8_lossy(&restart.stderr)
    );

    // serde_json::from_slice rejects trailing JSON, so this also proves that
    // restart writes exactly one document to stdout.
    let receipt: Value =
        serde_json::from_slice(&restart.stdout).expect("one hub restart JSON value");
    assert_eq!(receipt["stop"]["stopped"], true, "{receipt}");
    assert_eq!(receipt["stop"]["pid"], initial["pid"], "{receipt}");
    assert_eq!(receipt["start"]["version"], initial["version"], "{receipt}");
    assert_ne!(receipt["start"]["pid"], initial["pid"], "{receipt}");

    let failed_restart = cas_command(home.path(), &path)
        .args([
            "--json",
            "hub",
            "--no-tailscale-serve",
            "restart",
            "--port",
            &occupied_port.to_string(),
        ])
        .output()
        .expect("restart into an occupied port");
    assert!(
        !failed_restart.status.success(),
        "restart unexpectedly succeeded on occupied port {}",
        occupied_port
    );
    let failure: Value = serde_json::from_slice(&failed_restart.stdout)
        .expect("one JSON error value when replacement start fails");
    assert_eq!(failure["status"], "error", "{failure}");
    assert_eq!(failure["stop"]["stopped"], true, "{failure}");
    assert_eq!(failure["stop"]["pid"], receipt["start"]["pid"], "{failure}");
    assert_eq!(failure["stop"]["already_satisfied"], false, "{failure}");
    let message = failure["message"].as_str().expect("failure message");
    assert!(
        String::from_utf8_lossy(&failed_restart.stderr).contains(message),
        "human diagnostic must remain on stderr: stdout={} stderr={}",
        String::from_utf8_lossy(&failed_restart.stdout),
        String::from_utf8_lossy(&failed_restart.stderr),
    );

    let expected_stop_keys = [
        "already_satisfied",
        "pid",
        "stopped",
        "tailscale_serve_removed",
        "tailscale_warning",
    ]
    .map(str::to_owned)
    .into_iter()
    .collect::<std::collections::BTreeSet<_>>();
    for stop in [&receipt["stop"], &failure["stop"]] {
        let keys = stop
            .as_object()
            .expect("restart stop object")
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(keys, expected_stop_keys, "{stop}");
    }

    let help = cas_command(home.path(), &path)
        .args(["hub", "restart", "--help"])
        .output()
        .expect("restart help");
    assert!(help.status.success());
    let help_text = String::from_utf8_lossy(&help.stdout);
    let normalized_help = help_text.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        normalized_help.contains("Keep this launch loopback-only (overrides hub.transport_mode for this launch)"),
        "loopback flag help should describe its transport policy override: {help_text}"
    );
    assert!(
        help_text.to_ascii_lowercase().contains("detached restart"),
        "restart JSON help must scope its response shape to detached hubs: {help_text}"
    );
    let restart_help_lines = help_text
        .lines()
        .filter(|line| {
            line.starts_with("Detached restart:")
                || line.starts_with("Stop keys:")
                || line.starts_with("Stop also has")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        restart_help_lines.len(),
        3,
        "restart help should render its three concise contract lines: {help_text}"
    );
    for line in restart_help_lines {
        assert!(
            line.chars().count() <= 80,
            "restart help line exceeds 80 cells: {} ({})",
            line.chars().count(),
            line
        );
    }

    drop(occupied_listener);
    let stop = cas_command(home.path(), &path)
        .args(["--json", "hub", "stop"])
        .output()
        .expect("stop restarted hub");
    assert!(
        stop.status.success(),
        "hub stop failed: {}",
        String::from_utf8_lossy(&stop.stderr)
    );

    // The existing debug-only lock-release barrier keeps the old hub's lock
    // held after its record disappears. Race two actual restart commands in
    // that widened window until one observes the concurrent winner.
    let max_race_attempts = 12;
    let mut satisfied_attempt = None;
    for attempt in 1..=max_race_attempts {
        let race_home = private_home();
        let barrier = race_home.path().join("restart-race-barrier");
        std::fs::create_dir_all(&barrier).expect("create restart race barrier");
        let initial = cas_command(race_home.path(), &path)
            .env("CAS_TEST_HUB_LOCK_RELEASE_BARRIER", &barrier)
            .args([
                "--json",
                "hub",
                "--no-tailscale-serve",
                "start",
                "--port",
                "0",
            ])
            .output()
            .expect("start hub for concurrent restart race");
        assert!(
            initial.status.success(),
            "race attempt {attempt} hub start failed: {}",
            String::from_utf8_lossy(&initial.stderr)
        );

        let mut first_restart = cas_command(race_home.path(), &path);
        first_restart
            .env("CAS_TEST_HUB_LOCK_RELEASE_BARRIER", &barrier)
            .args([
                "--json",
                "hub",
                "--no-tailscale-serve",
                "restart",
                "--port",
                "0",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let first = first_restart.spawn().expect("spawn first restart racer");

        let marker = barrier.join("record-removed-lock-held");
        let marker_deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() && Instant::now() < marker_deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !marker.exists() {
            let _ = std::fs::write(barrier.join("release"), b"release\n");
            let _ = first.wait_with_output();
            panic!("race attempt {attempt} old hub did not reach the release barrier");
        }

        let mut second_restart = cas_command(race_home.path(), &path);
        second_restart
            .env("CAS_TEST_HUB_LOCK_RELEASE_BARRIER", &barrier)
            .args([
                "--json",
                "hub",
                "--no-tailscale-serve",
                "restart",
                "--port",
                "0",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let second = second_restart.spawn().expect("spawn second restart racer");
        std::thread::sleep(Duration::from_millis(250));
        std::fs::write(barrier.join("release"), b"release\n")
            .expect("release old hub lock barrier");
        let outputs = [
            first
                .wait_with_output()
                .expect("wait for first restart racer"),
            second
                .wait_with_output()
                .expect("wait for second restart racer"),
        ];
        let mut observed = false;
        for output in &outputs {
            let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "race attempt {attempt} did not emit one JSON value: {error}; stdout={} stderr={}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                )
            });
            let stop = value["stop"].as_object().expect("restart stop receipt");
            let keys = stop
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(keys, expected_stop_keys, "race attempt {attempt}: {value}");
            if output.status.success() {
                assert!(
                    value["start"].is_object(),
                    "race attempt {attempt}: {value}"
                );
            } else {
                assert_eq!(value["status"], "error", "race attempt {attempt}: {value}");
            }
            if stop["already_satisfied"] == true {
                assert_eq!(stop["stopped"], false, "race attempt {attempt}: {value}");
                assert!(stop["pid"].is_null(), "race attempt {attempt}: {value}");
                assert!(
                    stop["tailscale_serve_removed"].is_null(),
                    "race attempt {attempt}: {value}"
                );
                assert!(
                    stop["tailscale_warning"].is_null(),
                    "race attempt {attempt}: {value}"
                );
                observed = true;
            }
        }
        let cleanup = cas_command(race_home.path(), &path)
            .args(["--json", "hub", "stop"])
            .output()
            .expect("stop concurrent restart fixture");
        assert!(
            cleanup.status.success(),
            "race attempt {attempt} cleanup failed: {}",
            String::from_utf8_lossy(&cleanup.stderr)
        );
        if observed {
            satisfied_attempt = Some(attempt);
            break;
        }
    }
    assert!(
        satisfied_attempt.is_some(),
        "two concurrent restarts did not exercise already_satisfied in {max_race_attempts} attempts"
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
