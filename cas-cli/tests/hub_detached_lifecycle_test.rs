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

use cas::hub::HubRuntimePaths;
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
    assert_eq!(receipt["start"]["already_satisfied"], false, "{receipt}");
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
                || line.starts_with("Start also has")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        restart_help_lines.len(),
        4,
        "restart help should render its four concise contract lines: {help_text}"
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

    // cas-bcef: hold an old stop after quiescence but before cleanup. A peer
    // launch must wait for that cleanup, rather than publish a record which
    // the old stop can subsequently unlink.
    {
        let race_home = private_home();
        let paths = HubRuntimePaths::new(race_home.path().join(".cas/hub"));
        let barrier = race_home.path().join("stop-cleanup-barrier");
        fs::create_dir_all(&barrier).unwrap();
        let initial = cas_command(race_home.path(), &path)
            .args(["--json", "hub", "--no-tailscale-serve", "start", "--port", "0"])
            .output()
            .expect("start old generation before cleanup race");
        assert!(initial.status.success(), "{initial:?}");
        let initial: Value = serde_json::from_slice(&initial.stdout).unwrap();
        let stopper = cas_command(race_home.path(), &path)
            .env("CAS_TEST_HUB_STOP_CLEANUP_BARRIER", &barrier)
            .args(["--json", "hub", "stop"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("pause old stop before cleanup");
        let cleanup_marker = barrier.join(format!("stop-cleanup-{}", stopper.id()));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !cleanup_marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !cleanup_marker.exists() {
            fs::write(barrier.join("release-stop-cleanup"), b"release\n").unwrap();
            let output = stopper.wait_with_output().unwrap();
            panic!("old stop did not reach cleanup: {output:?}");
        }
        let mut starter = cas_command(race_home.path(), &path)
            .env("CAS_TEST_HUB_LAUNCH_WAIT_BARRIER", &barrier)
            .args(["--json", "hub", "--no-tailscale-serve", "start", "--port", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start replacement during old cleanup");
        let launch_marker = barrier.join(format!("launch-waiter-{}", starter.id()));
        let deadline = Instant::now() + Duration::from_secs(10);
        // Either the peer contends on cleanup (correct), or it completes its
        // launch before cleanup (the original record-loss interleaving).
        while !launch_marker.exists()
            && starter.try_wait().unwrap().is_none()
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        fs::write(barrier.join("release-stop-cleanup"), b"release\n").unwrap();
        let stopped = stopper.wait_with_output().unwrap();
        fs::write(barrier.join("release-launch-waiter"), b"release\n").unwrap();
        let started = starter.wait_with_output().unwrap();
        assert!(stopped.status.success(), "{stopped:?}");
        assert!(started.status.success(), "{started:?}");
        let replacement: Value = serde_json::from_slice(&started.stdout).unwrap();
        assert_ne!(replacement["pid"], initial["pid"]);
        let status = cas_command(race_home.path(), &path)
            .args(["--json", "hub", "status"])
            .output()
            .expect("read replacement after old cleanup");
        let status: Value = serde_json::from_slice(&status.stdout).unwrap();
        assert_eq!(
            status["running"], true,
            "old stop erased the successful replacement's process record: {status}"
        );
        assert_eq!(status["record"]["pid"], replacement["pid"], "{status}");
        assert!(paths.lock_holders().iter().any(|holder| {
            Some(u64::from(holder.pid)) == replacement["pid"].as_u64()
        }), "replacement record must identify the actual lock owner: {status}");
        let stopped = cas_command(race_home.path(), &path)
            .args(["--json", "hub", "stop"])
            .output()
            .expect("stop cleanup-race replacement");
        assert!(stopped.status.success(), "{stopped:?}");
    }

    // Force both restart parents past their old-hub stop and into launch before
    // releasing either new hub. Holding their result observers then makes one
    // parent deterministically see the peer's ready record. Other passes hold
    // the winner in startup and before record publication to exercise both
    // early-exit paths.
    for (hold_startup, hold_publication) in [(false, false), (true, false), (false, true)] {
        let race_home = private_home();
        let barrier = race_home.path().join("restart-race-barrier");
        std::fs::create_dir_all(&barrier).expect("create restart race barrier");
        let session = format!(
            "cas-3dc8-test-{}-{hold_startup}-{hold_publication}",
            std::process::id()
        );
        let initial = cas_command(race_home.path(), &path)
            .env("CAS_AGENT_ROLE", "worker")
            .env("CAS_FACTORY_SESSION", &session)
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
            "race hub start failed: {}",
            String::from_utf8_lossy(&initial.stderr)
        );
        let initial: Value =
            serde_json::from_slice(&initial.stdout).expect("race hub start JSON");

        let spawn_restart = |name: &str| {
            let mut command = cas_command(race_home.path(), &path);
            command
                .env("CAS_AGENT_ROLE", "worker")
                .env("CAS_FACTORY_SESSION", &session)
                .env("CAS_TEST_HUB_LOCK_RELEASE_BARRIER", &barrier)
                .env("CAS_TEST_HUB_LAUNCH_BARRIER", &barrier)
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
            if hold_startup {
                command.env("CAS_TEST_HUB_STARTUP_BARRIER", &barrier);
            } else if hold_publication {
                command.env("CAS_TEST_HUB_RECORD_PUBLICATION_BARRIER", &barrier);
                command.env("CAS_TEST_HUB_PEER_RECORD_WAIT_BARRIER", &barrier);
            } else {
                command.env("CAS_TEST_HUB_READY_BARRIER", &barrier);
            }
            command
                .spawn()
                .unwrap_or_else(|error| panic!("spawn {name} concurrent restart: {error}"))
        };
        let mut first = Some(spawn_restart("first"));

        let marker = barrier.join("record-removed-lock-held");
        let marker_deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() && Instant::now() < marker_deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !marker.exists() {
            let _ = std::fs::write(barrier.join("release"), b"release\n");
            let _ = std::fs::write(barrier.join("release-launchers"), b"release\n");
            let _ = std::fs::write(barrier.join("release-monitors"), b"release\n");
            let _ = std::fs::write(barrier.join("release-hub-record"), b"release\n");
            let _ = std::fs::write(barrier.join("release-peer-record-wait"), b"release\n");
            let _ = first.take().unwrap().wait_with_output();
            panic!("old hub did not reach the deterministic release barrier");
        }

        let waiter_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let waiters = std::fs::read_dir(&barrier)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("restart-stop-waiter-")
                })
                .count();
            if waiters >= 1 || Instant::now() >= waiter_deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut second = Some(spawn_restart("second"));
        let waiter_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let waiters = std::fs::read_dir(&barrier)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("restart-stop-waiter-")
                })
                .count();
            if waiters >= 2 || Instant::now() >= waiter_deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let waiters = std::fs::read_dir(&barrier)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("restart-stop-waiter-")
            })
            .count();
        std::fs::write(barrier.join("release"), b"release\n").unwrap();

        let wait_for_markers = |prefix: &str, expected: usize| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let markers = std::fs::read_dir(&barrier)
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
                    .map(|entry| entry.path())
                    .collect::<Vec<_>>();
                if markers.len() >= expected || Instant::now() >= deadline {
                    return markers;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        let launchers = wait_for_markers("launcher-", 2);
        if launchers.len() < 2 {
            let _ = std::fs::write(barrier.join("release-launchers"), b"release\n");
            let _ = std::fs::write(barrier.join("release-monitors"), b"release\n");
            let _ = std::fs::write(barrier.join("release-hub-startup"), b"release\n");
            let _ = std::fs::write(barrier.join("release-hub-record"), b"release\n");
            let _ = std::fs::write(barrier.join("release-peer-record-wait"), b"release\n");
            let _ = first.take().unwrap().wait_with_output();
            let _ = second.take().unwrap().wait_with_output();
            panic!("both restarts did not launch before the barrier: {launchers:?}");
        }
        std::fs::write(barrier.join("release-launchers"), b"release\n").unwrap();
        let monitors = wait_for_markers("monitor-", 2);
        if monitors.len() < 2 {
            let _ = std::fs::write(barrier.join("release-monitors"), b"release\n");
            let _ = std::fs::write(barrier.join("release-hub-startup"), b"release\n");
            let _ = first.take().unwrap().wait_with_output();
            let _ = second.take().unwrap().wait_with_output();
            panic!("both restart result observers did not reach the barrier: {monitors:?}");
        }

        let paths = HubRuntimePaths::new(race_home.path().join(".cas/hub"));
        let winner_pid = if hold_startup {
            let startup_markers = wait_for_markers("hub-starting-", 1);
            if startup_markers.len() != 1 {
                let _ = std::fs::write(barrier.join("release-monitors"), b"release\n");
                let _ = std::fs::write(barrier.join("release-hub-startup"), b"release\n");
                let _ = first.take().unwrap().wait_with_output();
                let _ = second.take().unwrap().wait_with_output();
                panic!("expected one lock-owning startup marker: {startup_markers:?}");
            }
            let record = paths
                .read_process_record()
                .expect("starting hub process record");
            assert_eq!(
                record.pid,
                startup_markers[0]
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .trim_start_matches("hub-starting-")
                    .parse::<u32>()
                    .unwrap(),
                "startup barrier belongs to the recorded lock owner"
            );
            assert!(
                paths
                    .lock_holders()
                    .iter()
                    .any(|holder| holder.pid == record.pid),
                "starting record must own the machine lock"
            );
            std::fs::write(barrier.join("release-monitors"), b"release\n").unwrap();
            let loser_deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if first
                    .as_mut()
                    .is_some_and(|child| child.try_wait().unwrap().is_some())
                    || second
                        .as_mut()
                        .is_some_and(|child| child.try_wait().unwrap().is_some())
                {
                    break;
                }
                if Instant::now() >= loser_deadline {
                    let _ = std::fs::write(barrier.join("release-hub-startup"), b"release\n");
                    let _ = first.take().unwrap().wait_with_output();
                    let _ = second.take().unwrap().wait_with_output();
                    panic!("losing launch did not exit while the winner was held starting");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            record.pid
        } else if hold_publication {
            let pending = wait_for_markers("hub-record-pending-", 1);
            if pending.len() != 1 {
                let _ = std::fs::write(barrier.join("release-monitors"), b"release\n");
                let _ = std::fs::write(barrier.join("release-hub-record"), b"release\n");
                let _ = std::fs::write(barrier.join("release-peer-record-wait"), b"release\n");
                let _ = first.take().unwrap().wait_with_output();
                let _ = second.take().unwrap().wait_with_output();
                panic!("expected one hub held before publishing its process record: {pending:?}");
            }
            let winner_pid = pending[0]
                .file_name()
                .unwrap()
                .to_string_lossy()
                .trim_start_matches("hub-record-pending-")
                .parse::<u32>()
                .unwrap();
            assert!(
                paths.read_process_record().is_err(),
                "the publication barrier must precede the winning process record"
            );
            std::fs::write(barrier.join("release-monitors"), b"release\n").unwrap();
            let peer_waiters = wait_for_markers("peer-record-wait-", 1);
            if peer_waiters.len() != 1 {
                let _ = std::fs::write(barrier.join("release-monitors"), b"release\n");
                let _ = std::fs::write(barrier.join("release-hub-record"), b"release\n");
                let _ = std::fs::write(barrier.join("release-peer-record-wait"), b"release\n");
                let _ = first.take().unwrap().wait_with_output();
                let _ = second.take().unwrap().wait_with_output();
                panic!("losing restart did not observe the unpublished peer record");
            }
            std::fs::write(barrier.join("release-hub-record"), b"release\n").unwrap();
            let record_deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if paths.read_process_record().is_ok_and(|record| record.pid == winner_pid) {
                    break;
                }
                if Instant::now() >= record_deadline {
                    let _ = std::fs::write(barrier.join("release-peer-record-wait"), b"release\n");
                    let _ = first.take().unwrap().wait_with_output();
                    let _ = second.take().unwrap().wait_with_output();
                    panic!("winning hub did not publish its process record");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            std::fs::write(barrier.join("release-peer-record-wait"), b"release\n").unwrap();
            winner_pid
        } else {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut running_pid = None;
            while running_pid.is_none() && Instant::now() < deadline {
                let status = cas_command(race_home.path(), &path)
                    .args(["--json", "hub", "status"])
                    .output()
                    .expect("read ready winner while observers are held");
                running_pid = serde_json::from_slice::<Value>(&status.stdout)
                    .ok()
                    .filter(|value| value["running"] == true)
                    .and_then(|value| value["record"]["pid"].as_u64())
                    .map(|pid| pid as u32);
                if running_pid.is_none() {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            assert!(
                running_pid.is_some(),
                "no ready hub reached the observer barrier"
            );
            running_pid.unwrap()
        };
        std::fs::write(barrier.join("release-monitors"), b"release\n").unwrap();
        if !hold_startup && !hold_publication {
            let ready_observers = wait_for_markers("ready-observer-", 2);
            std::fs::write(barrier.join("release-ready-observers"), b"release\n").unwrap();
            if ready_observers.len() < 2 {
                let _ = first.take().unwrap().wait_with_output();
                let _ = second.take().unwrap().wait_with_output();
                panic!(
                    "both restarts must observe the peer's ready record: {ready_observers:?}"
                );
            }
        }
        let mut first_output = None;
        let mut second_output = None;
        if hold_startup || hold_publication {
            // Capture the loser only after its peer record has been published;
            // the publication-gap case must attribute this already-exited child.
            let loser_deadline = Instant::now() + Duration::from_secs(10);
            let (loser_index, output) = loop {
                if first
                    .as_mut()
                    .is_some_and(|child| child.try_wait().unwrap().is_some())
                {
                    break (0, first.take().unwrap().wait_with_output().unwrap());
                }
                if second
                    .as_mut()
                    .is_some_and(|child| child.try_wait().unwrap().is_some())
                {
                    break (1, second.take().unwrap().wait_with_output().unwrap());
                }
                if Instant::now() >= loser_deadline {
                    let _ = std::fs::write(barrier.join("release-hub-startup"), b"release\n");
                    let _ = std::fs::write(barrier.join("release-hub-record"), b"release\n");
                    let _ = std::fs::write(barrier.join("release-peer-record-wait"), b"release\n");
                    let _ = first.take().map(|child| child.wait_with_output());
                    let _ = second.take().map(|child| child.wait_with_output());
                    panic!("losing restart did not report while its peer owned the machine lock");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            std::fs::write(barrier.join("release-hub-startup"), b"release\n").unwrap();
            if loser_index == 0 {
                first_output = Some(output);
            } else {
                second_output = Some(output);
            }
        }

        if let Some(child) = first.take() {
            first_output = Some(child.wait_with_output().expect("wait first restart"));
        }
        if let Some(child) = second.take() {
            second_output = Some(child.wait_with_output().expect("wait second restart"));
        }
        let mut outputs = Vec::new();
        if let Some(output) = first_output {
            outputs.push(output);
        }
        if let Some(output) = second_output {
            outputs.push(output);
        }
        assert_eq!(outputs.len(), 2, "both restart outcomes must be captured");
        assert_eq!(
            waiters, 2,
            "both restarts must reach the stop wait, startup-held={hold_startup}"
        );

        let status = cas_command(race_home.path(), &path)
            .args(["--json", "hub", "status"])
            .output()
            .expect("read hub after concurrent restarts");
        let running: Value = serde_json::from_slice(&status.stdout).expect("hub status JSON");
        assert_eq!(running["running"], true, "{running}");
        assert_eq!(running["record"]["pid"], winner_pid, "{running}");
        let cleanup = cas_command(race_home.path(), &path)
            .env("CAS_AGENT_ROLE", "worker")
            .env("CAS_FACTORY_SESSION", &session)
            .args(["--json", "hub", "stop"])
            .output()
            .expect("stop concurrent restart fixture");
        assert!(
            cleanup.status.success(),
            "race cleanup failed: {}",
            String::from_utf8_lossy(&cleanup.stderr)
        );

        let mut launched_starts = 0;
        let mut satisfied_starts = 0;
        let mut errors = 0;
        for output in &outputs {
            let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "restart did not emit one JSON value: {error}; stdout={} stderr={}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                )
            });
            let stop = value["stop"].as_object().expect("restart stop receipt");
            let keys = stop
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(keys, expected_stop_keys, "{value}");
            if output.status.success() {
                assert!(value["start"].is_object(), "{value}");
                assert_eq!(value["start"]["pid"], winner_pid, "{value}");
                match value["start"]["already_satisfied"].as_bool() {
                    Some(false) => launched_starts += 1,
                    Some(true) => satisfied_starts += 1,
                    None => panic!("restart has no start disposition: {value}"),
                }
            } else {
                errors += 1;
                assert_eq!(value["status"], "error", "{value}");
                assert!(
                    value["message"]
                        .as_str()
                        .unwrap_or_default()
                        .contains(&format!(
                            "another cas hub instance won the machine lock (pid {winner_pid})"
                        )),
                    "losing restart error must name the running winner: {value}"
                );
            }
        }
        if hold_startup || hold_publication {
            assert_eq!(
                launched_starts, 1,
                "exactly one restart reports launching the held winner"
            );
            assert_eq!(errors, 1, "the losing launch must report its winner");
        } else {
            assert_eq!(
                launched_starts, 1,
                "exactly one restart launched the ready hub"
            );
            assert_eq!(
                satisfied_starts, 1,
                "the peer adopter must be flagged satisfied"
            );
            assert_eq!(errors, 0, "ready peer adoption must be a success");
        }

        if let Some(cgroup) = initial["cgroup"].as_str().map(Path::new)
            && let Some(parent) = cgroup.parent()
        {
            let prefix = format!("cas-server-{session}-hub-");
            let leftovers = std::fs::read_dir(parent)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
                .map(|entry| entry.path())
                .collect::<Vec<_>>();
            assert!(
                leftovers.is_empty(),
                "restart launch scopes were left behind: {leftovers:?}"
            );
        }
    }
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
        let pid = record["pid"].as_u64().expect("worker hub pid");
        assert!(
            cgroup.contains("/cas-server-") && cgroup.ends_with(&format!("-hub-{pid}")),
            "worker hub must record its generation-specific cas-server scope: {cgroup}"
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
