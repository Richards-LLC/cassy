use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;
use std::time::Instant;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn cas_command(home: &Path, path: &OsStr) -> Command {
    let mut command = Command::new(assert_cmd::cargo::cargo_bin!("cas"));
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", path)
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    command
}

fn cas_process_command(home: &Path, path: &OsStr) -> std::process::Command {
    let mut command = std::process::Command::new(assert_cmd::cargo::cargo_bin!("cas"));
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", path)
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    command
}

fn system_path() -> OsString {
    std::env::var_os("PATH").unwrap_or_default()
}

fn private_home() -> TempDir {
    let parent = std::env::temp_dir().canonicalize().unwrap();
    let home = tempfile::tempdir_in(parent).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(home.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    home
}

fn start_hub(home: &Path, path: &OsStr, tailscale: bool) -> Value {
    assert!(
        !home.join(".cas").exists(),
        "process proof must start with an actually absent CAS home"
    );
    let mut command = cas_command(home, path);
    command.args(["--json", "hub", "start", "--port", "0"]);
    if tailscale {
        command.arg("--tailscale-serve");
    }
    let output = command.output().expect("start clean-home hub process");
    assert!(
        output.status.success(),
        "clean-home hub start failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("start output is JSON")
}

fn assert_health_status_and_stop(home: &Path, path: &OsStr, record: &Value) -> Value {
    let port = record["port"].as_u64().expect("hub port");
    let health_response = ureq::get(&format!("http://127.0.0.1:{port}/v1/health"))
        .set("Host", "spoof.tail.example")
        .set("Forwarded", "proto=https;host=spoof.tail.example")
        .set("X-Forwarded-Proto", "https")
        .set("X-Forwarded-Host", "spoof.tail.example")
        .set("Tailscale-User-Login", "spoof@example.com")
        .timeout(Duration::from_secs(2))
        .call()
        .expect("clean-home health request");
    assert_eq!(
        health_response.header("strict-transport-security"),
        None,
        "the documented plaintext listener ignores client-spoofed TLS headers"
    );
    let health: Value = health_response.into_json().expect("health JSON");
    assert_eq!(health, serde_json::json!({"schema_version":1,"ready":true}));

    let status = cas_command(home, path)
        .args(["--json", "hub", "status"])
        .output()
        .expect("clean-home hub status");
    assert!(
        status.status.success(),
        "clean-home hub status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: Value = serde_json::from_slice(&status.stdout).expect("status output is JSON");
    assert_eq!(status["running"], true);
    assert_eq!(status["record"]["pid"], record["pid"]);

    let stop = cas_command(home, path)
        .args(["--json", "hub", "stop"])
        .output()
        .expect("clean-home hub stop");
    assert!(
        stop.status.success(),
        "clean-home hub stop failed: {}",
        String::from_utf8_lossy(&stop.stderr)
    );
    let stop: Value = serde_json::from_slice(&stop.stdout).expect("stop output is JSON");
    assert_eq!(stop["stopped"], true);
    assert!(!home.join(".cas/hub/process.json").exists());
    stop
}

fn response_even_for_error(request: ureq::Request) -> ureq::Response {
    match request.timeout(Duration::from_secs(2)).call() {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(error) => panic!("Commander request failed before HTTP response: {error}"),
    }
}

fn assert_hsts(response: &ureq::Response) {
    assert_eq!(
        response.header("strict-transport-security"),
        Some("max-age=31536000")
    );
    assert_eq!(response.all("strict-transport-security").len(), 1);
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::MetadataExt;
    fs::symlink_metadata(path).unwrap().mode() & 0o777
}

#[cfg(unix)]
fn assert_private_state_modes(home: &Path) {
    assert_eq!(mode(&home.join(".cas")), 0o700);
    assert_eq!(mode(&home.join(".cas/hub")), 0o700);
    for name in ["machine-id", "auth.lock", "auth.json"] {
        assert_eq!(
            mode(&home.join(".cas/hub").join(name)),
            0o600,
            "{name} must remain private"
        );
    }
}

#[test]
fn clean_home_process_start_health_status_stop_needs_no_init() {
    let home = private_home();
    let path = system_path();
    let record = start_hub(home.path(), &path, false);
    #[cfg(unix)]
    assert_private_state_modes(home.path());

    let stop = assert_health_status_and_stop(home.path(), &path, &record);
    assert_eq!(stop["tailscale_serve_removed"], false);
    assert!(home.path().join(".cas/hub/machine-id").exists());
    assert!(home.path().join(".cas/hub/auth.json").exists());
}

#[cfg(unix)]
#[test]
fn clean_home_tailscale_stop_reports_removal_after_serve_exit_teardown() {
    use std::os::unix::fs::PermissionsExt;

    let home = private_home();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let tailscale = bin.join("tailscale");
    fs::write(
        &tailscale,
        r#"#!/bin/sh
case "$*" in
  'status --json') printf '%s' '{"Self":{"DNSName":"clean-host.tail.example."}}' ;;
  'serve status --json')
    if [ -f "$HOME/mock-serve" ]; then
      printf '%s' '{"Web":{"clean-host.tail.example:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:'"$(/bin/cat "$HOME/mock-port")"'"}}}}}'
    else
      printf '%s' '{}'
    fi ;;
  'serve --bg --yes --https=443 '*)
    printf '%s' "${5##*:}" > "$HOME/mock-port"
    : > "$HOME/mock-serve" ;;
  'serve --https=443 off') /bin/rm -f "$HOME/mock-serve" ;;
  *) exit 9 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&tailscale, fs::Permissions::from_mode(0o700)).unwrap();

    let record = start_hub(home.path(), bin.as_os_str(), true);
    assert_eq!(record["public_url"], "https://clean-host.tail.example/");
    assert_eq!(record["transport_warning"], Value::Null);
    assert_private_state_modes(home.path());
    assert_eq!(
        mode(&home.path().join(".cas/hub/tailscale-serve.json")),
        0o600
    );

    let plaintext_port = record["port"].as_u64().unwrap() as u16;
    let trusted_backend_port = fs::read_to_string(home.path().join("mock-port"))
        .unwrap()
        .parse::<u16>()
        .unwrap();
    assert_ne!(trusted_backend_port, plaintext_port);
    for (path, status) in [
        ("/", 200),
        ("/v1/health", 200),
        ("/v1/sessions", 401),
        ("/missing", 405),
    ] {
        let response = response_even_for_error(ureq::get(&format!(
            "http://127.0.0.1:{trusted_backend_port}{path}"
        )));
        assert_eq!(response.status(), status, "unexpected status for {path}");
        assert_hsts(&response);
        assert_eq!(response.header("referrer-policy"), Some("no-referrer"));
        assert_eq!(response.header("x-content-type-options"), Some("nosniff"));
        assert_eq!(response.header("x-frame-options"), Some("DENY"));
        assert!(response.header("content-security-policy").is_some());
    }
    let preflight = response_even_for_error(
        ureq::request(
            "OPTIONS",
            &format!("http://127.0.0.1:{trusted_backend_port}/v1/auth/pairing/exchange"),
        )
        .set("Origin", "http://127.0.0.1:4173")
        .set("Access-Control-Request-Method", "POST")
        .set("Access-Control-Request-Headers", "content-type"),
    );
    assert_eq!(preflight.status(), 204);
    assert_eq!(
        preflight.header("access-control-allow-origin"),
        Some("http://127.0.0.1:4173")
    );
    assert_hsts(&preflight);

    let restart = cas_command(home.path(), bin.as_os_str())
        .args([
            "--json",
            "hub",
            "restart",
            "--port",
            "0",
            "--tailscale-serve",
        ])
        .output()
        .expect("restart trusted proxy hub");
    assert!(
        restart.status.success(),
        "trusted proxy restart failed: {}",
        String::from_utf8_lossy(&restart.stderr)
    );
    let status = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "status"])
        .output()
        .expect("status after restart");
    assert!(status.status.success());
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    let restarted = &status["record"];
    assert_eq!(restarted["public_url"], "https://clean-host.tail.example/");
    let restarted_plaintext = restarted["port"].as_u64().unwrap() as u16;
    let restarted_backend = fs::read_to_string(home.path().join("mock-port"))
        .unwrap()
        .parse::<u16>()
        .unwrap();
    assert_ne!(restarted_backend, restarted_plaintext);
    assert_hsts(&response_even_for_error(ureq::get(&format!(
        "http://127.0.0.1:{restarted_backend}/v1/health"
    ))));

    let killed_pid = restarted["pid"].as_i64().unwrap() as i32;
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(killed_pid),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    let recovered_output = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "start", "--port", "0", "--tailscale-serve"])
        .output()
        .expect("recover killed trusted proxy hub");
    assert!(
        recovered_output.status.success(),
        "stale owned mapping recovery failed: {}",
        String::from_utf8_lossy(&recovered_output.stderr)
    );
    let recovered: Value = serde_json::from_slice(&recovered_output.stdout).unwrap();
    assert_eq!(recovered["public_url"], "https://clean-host.tail.example/");
    let recovered_plaintext = recovered["port"].as_u64().unwrap() as u16;
    let recovered_backend = fs::read_to_string(home.path().join("mock-port"))
        .unwrap()
        .parse::<u16>()
        .unwrap();
    assert_ne!(recovered_backend, recovered_plaintext);
    assert_hsts(&response_even_for_error(ureq::get(&format!(
        "http://127.0.0.1:{recovered_backend}/v1/health"
    ))));
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", restarted_backend)).is_err(),
        "killed trusted backend listener survived recovery"
    );

    // `hub stop` waits for the foreground process to exit. That process now
    // performs the owned teardown itself, so the receipt must report the
    // mapping's final absence rather than require stop to be its remover.
    let stop = assert_health_status_and_stop(home.path(), bin.as_os_str(), &recovered);
    assert_eq!(stop["tailscale_serve_removed"], true);
    assert!(!home.path().join(".cas/hub/tailscale-serve.json").exists());
    assert_eq!(
        mode(&home.path().join(".cas/hub/tailscale-serve-teardown.json")),
        0o600
    );
    assert!(!home.path().join("mock-serve").exists());
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", recovered_plaintext)).is_err(),
        "plaintext listener survived owned teardown"
    );
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", recovered_backend)).is_err(),
        "trusted proxy backend survived owned teardown"
    );
}

#[cfg(unix)]
#[test]
fn process_start_rejects_state_collisions_with_sanitized_diagnostics() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    for case in ["symlink", "file", "loose", "unwritable"] {
        let home = private_home();
        let path = system_path();
        let cas = home.path().join(".cas");
        match case {
            "symlink" => {
                let target = home.path().join("elsewhere");
                fs::create_dir(&target).unwrap();
                symlink(&target, &cas).unwrap();
            }
            "file" => fs::write(&cas, "collision").unwrap(),
            "loose" => {
                fs::create_dir(&cas).unwrap();
                let hub = cas.join("hub");
                fs::create_dir(&hub).unwrap();
                fs::set_permissions(&hub, fs::Permissions::from_mode(0o755)).unwrap();
            }
            "unwritable" => {
                fs::create_dir(&cas).unwrap();
                fs::set_permissions(&cas, fs::Permissions::from_mode(0o500)).unwrap();
            }
            _ => unreachable!(),
        }

        let output = cas_command(home.path(), &path)
            .args(["--json", "hub", "start", "--port", "0"])
            .output()
            .expect("collision start process");
        assert!(
            !output.status.success(),
            "{case} collision unexpectedly started"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("hub state hierarchy") || stderr.contains("mode 0700"),
            "{case} collision lacked a stable diagnostic: {stderr}"
        );
        assert!(
            !stderr.contains(home.path().to_string_lossy().as_ref()),
            "{case} collision leaked its filesystem path: {stderr}"
        );
        assert!(!cas.join("hub/process.json").exists());

        if case == "unwritable" {
            fs::set_permissions(&cas, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
}

#[cfg(unix)]
#[test]
fn restart_waits_for_record_absent_instance_lock_release_before_replacement() {
    use cas::hub::HubRuntimePaths;
    use std::os::unix::fs::PermissionsExt;

    let home = private_home();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let tailscale = bin.join("tailscale");
    fs::write(
        &tailscale,
        r#"#!/bin/sh
case "$*" in
  'status --json') printf '%s' '{"Self":{"DNSName":"restart-lock.tail.example."}}' ;;
  'serve status --json')
    if [ -f "$HOME/mock-serve" ]; then
      printf '%s' '{"Web":{"restart-lock.tail.example:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:'"$(/bin/cat "$HOME/mock-port")"'"}}}}}'
    else
      printf '%s' '{}'
    fi ;;
  'serve --bg --yes --https=443 '*)
    printf '%s' "${5##*:}" > "$HOME/mock-port"
    : > "$HOME/mock-serve" ;;
  'serve --https=443 off') /bin/rm -f "$HOME/mock-serve" ;;
  *) exit 9 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&tailscale, fs::Permissions::from_mode(0o700)).unwrap();

    let barrier = home.path().join("restart-lock-barrier");
    fs::create_dir(&barrier).unwrap();
    let initial = cas_command(home.path(), bin.as_os_str())
        .env("CAS_TEST_HUB_LOCK_RELEASE_BARRIER", &barrier)
        .args(["--json", "hub", "start", "--port", "0", "--tailscale-serve"])
        .output()
        .expect("start hub for restart-lock race");
    assert!(
        initial.status.success(),
        "initial hub failed: {}",
        String::from_utf8_lossy(&initial.stderr)
    );

    let mut restart = cas_process_command(home.path(), bin.as_os_str());
    restart
        .env("CAS_TEST_HUB_LOCK_RELEASE_BARRIER", &barrier)
        .args([
            "--json",
            "hub",
            "restart",
            "--port",
            "0",
            "--tailscale-serve",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = restart.spawn().expect("spawn exact restart command");

    let marker = barrier.join("record-removed-lock-held");
    let marker_deadline = Instant::now() + Duration::from_secs(3);
    while !marker.exists() && Instant::now() < marker_deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        marker.exists(),
        "old hub never reached the widened lock window"
    );
    assert!(
        !home.path().join(".cas/hub/process.json").exists(),
        "the deterministic seam must expose record-absent state"
    );
    let paths = HubRuntimePaths::new(home.path().join(".cas/hub"));
    assert!(
        paths.acquire_instance_lock().is_err(),
        "old hub must still authoritatively own the lock after record removal"
    );

    // Exceed the public implementation's five-second process-only stop wait.
    // The old restart path discards that timeout, starts too early, and its
    // one-shot child loses the still-held lock before this release arrives.
    thread::sleep(Duration::from_millis(5_200));
    fs::write(barrier.join("release"), b"release\n").unwrap();
    let output = child
        .wait_with_output()
        .expect("wait exact restart command");
    assert!(
        output.status.success(),
        "restart raced the old instance lock: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let status = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "status"])
        .output()
        .expect("status after restart-lock handoff");
    assert!(status.status.success());
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["running"], true);
    let replacement_pid = status["record"]["pid"].as_u64().unwrap();
    let initial: Value = serde_json::from_slice(&initial.stdout).unwrap();
    assert_ne!(replacement_pid, initial["pid"].as_u64().unwrap());

    let stop = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "stop"])
        .output()
        .expect("stop replacement hub");
    assert!(stop.status.success());
    assert!(paths.acquire_instance_lock().is_ok());
    assert!(!home.path().join(".cas/hub/process.json").exists());
    assert!(!home.path().join("mock-serve").exists());
}

#[cfg(unix)]
#[test]
fn restart_deadline_keeps_old_lock_owner_and_launches_no_replacement() {
    use cas::hub::HubRuntimePaths;

    let home = private_home();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let barrier = home.path().join("restart-lock-deadline");
    fs::create_dir(&barrier).unwrap();

    let initial = cas_command(home.path(), bin.as_os_str())
        .env("CAS_TEST_HUB_LOCK_RELEASE_BARRIER", &barrier)
        .args(["--json", "hub", "start", "--port", "0"])
        .output()
        .expect("start deadline fixture");
    assert!(initial.status.success());
    let initial: Value = serde_json::from_slice(&initial.stdout).unwrap();
    let old_pid = initial["pid"].as_u64().unwrap();

    let mut restart = cas_process_command(home.path(), bin.as_os_str());
    restart
        .env("CAS_TEST_HUB_LOCK_RELEASE_BARRIER", &barrier)
        .args(["--json", "hub", "restart", "--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = restart.spawn().expect("spawn deadline restart");
    let marker = barrier.join("record-removed-lock-held");
    let marker_deadline = Instant::now() + Duration::from_secs(3);
    while !marker.exists() && Instant::now() < marker_deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists());

    let output = child.wait_with_output().expect("wait deadline restart");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("machine lock remained live after 10.0s")
            && stderr.contains("no replacement was started"),
        "deadline must be truthful: {stderr}"
    );
    assert!(!home.path().join(".cas/hub/process.json").exists());
    let paths = HubRuntimePaths::new(home.path().join(".cas/hub"));
    assert!(paths.acquire_instance_lock().is_err());
    assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(old_pid as i32), None).is_ok());

    fs::write(barrier.join("release"), b"release\n").unwrap();
    let release_deadline = Instant::now() + Duration::from_secs(3);
    while paths.acquire_instance_lock().is_err() && Instant::now() < release_deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(paths.acquire_instance_lock().is_ok());
    let cleanup = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "stop"])
        .output()
        .expect("clean deadline fixture");
    assert!(cleanup.status.success());
    assert!(!home.path().join(".cas/hub/process.json").exists());
}

#[cfg(unix)]
#[test]
fn concurrent_start_and_restart_leave_exactly_one_lock_owner() {
    use cas::hub::HubRuntimePaths;

    let home = private_home();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let initial = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "start", "--port", "0"])
        .output()
        .expect("start concurrency fixture");
    assert!(initial.status.success());

    for _ in 0..5 {
        let gate = Arc::new(Barrier::new(3));
        let run = |action: &'static str, gate: Arc<Barrier>| {
            let home = home.path().to_path_buf();
            let bin = bin.clone();
            thread::spawn(move || {
                let mut command = cas_process_command(&home, bin.as_os_str());
                command.args(["--json", "hub", action, "--port", "0", "--tailscale-serve"]);
                gate.wait();
                command.output().unwrap()
            })
        };
        let start = run("start", gate.clone());
        let restart = run("restart", gate.clone());
        gate.wait();
        let outputs = [start.join().unwrap(), restart.join().unwrap()];
        assert!(
            outputs.iter().any(|output| output.status.success()),
            "at least one concurrent lifecycle command must complete: {:?}",
            outputs
                .iter()
                .map(|output| String::from_utf8_lossy(&output.stderr))
                .collect::<Vec<_>>()
        );

        let status = cas_command(home.path(), bin.as_os_str())
            .args(["--json", "hub", "status"])
            .output()
            .unwrap();
        assert!(status.status.success());
        let _status: Value = serde_json::from_slice(&status.stdout).unwrap();
        #[cfg(target_os = "linux")]
        {
            let pid = _status["record"]["pid"].as_u64().unwrap();
            let lock_path = home.path().join(".cas/hub/hub.lock");
            let lock_fds = fs::read_dir(format!("/proc/{pid}/fd"))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| fs::read_link(entry.path()).ok().as_deref() == Some(&lock_path))
                .count();
            assert_eq!(lock_fds, 1, "the recorded hub must own exactly one lock FD");
        }
        assert!(
            HubRuntimePaths::new(home.path().join(".cas/hub"))
                .acquire_instance_lock()
                .is_err(),
            "an independent contender must be excluded"
        );
    }

    let stop = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "stop"])
        .output()
        .unwrap();
    assert!(stop.status.success());
    assert!(!home.path().join(".cas/hub/process.json").exists());
    assert!(
        HubRuntimePaths::new(home.path().join(".cas/hub"))
            .acquire_instance_lock()
            .is_ok()
    );
}
