#[path = "support/hub_fixture.rs"]
mod hub_fixture;

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::process::Command as ProcessCommand;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;
use std::time::Instant;

use assert_cmd::Command;
use serde_json::Value;

const CHILD_PROGRESS_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(unix)]
fn install_tailscale_mock(_home: &Path, path: &Path, script: &str) {
    cas::test_paths::warm_stub(path, script);
}

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

fn cas_process_command(home: &Path, path: &OsStr) -> std::process::Command {
    let mut command = std::process::Command::new(cas::test_paths::cas_binary());
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", path)
        .env("TAILSCALE", fixture_tailscale(home, path))
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    command
}

fn system_path() -> OsString {
    std::env::var_os("PATH").unwrap_or_default()
}

fn private_home() -> hub_fixture::PrivateHubTempDir {
    hub_fixture::private_hub_tempdir()
}

/// Reserve an established client's local port, which cannot accept new
/// connections. A merely bound, non-listening socket times out on macOS;
/// this connected socket instead rejects unrelated connections on both OSes.
/// Keeping both ends open excludes the port from automatic ephemeral allocation
/// without any close/rebind window. This does not exclude an explicit bind with
/// SO_REUSEADDR, which macOS permits over an established connection.
#[cfg(unix)]
fn reserve_refused_loopback_port() -> (u16, (TcpStream, TcpStream)) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (peer, _) = listener.accept().unwrap();
    let port = client.local_addr().unwrap().port();
    let endpoint = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    assert!(
        TcpStream::connect_timeout(&endpoint, Duration::from_millis(100))
            .is_err_and(|error| error.kind() == std::io::ErrorKind::ConnectionRefused),
        "reserved loopback target must be connection-refused"
    );
    (port, (client, peer))
}

#[cfg(unix)]
struct LegacyHubCleanup(Vec<u32>);

#[cfg(unix)]
impl Drop for LegacyHubCleanup {
    fn drop(&mut self) {
        for pid in &self.0 {
            // Isolated test hubs only. A failed assertion must not leak an old
            // SIGSTOP'ed process or a newly launched replacement.
            unsafe { libc::kill(*pid as i32, libc::SIGKILL) };
        }
    }
}

#[cfg(unix)]
fn legacy_hub_command(binary: &Path, home: &Path, bin: &Path) -> ProcessCommand {
    let mut command = ProcessCommand::new(binary);
    let path = std::env::join_paths([bin, Path::new("/usr/bin"), Path::new("/bin")]).unwrap();
    command.env_clear().env("HOME", home).env("PATH", path).env("CAS_SKIP_FACTORY_TOOLING", "1");
    command
}

/// Exercise default publication through the actual CLI and post-swap receipt.
#[cfg(unix)]
#[test]
fn flagless_hub_lifecycle_publishes_and_host_opt_out_removes_serve() {
    let home = private_home();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    install_tailscale_mock(
        home.path(),
        &bin.join("tailscale"),
        include_str!("fixtures/hub_update_mock_tailscale.sh"),
    );
    fs::write(home.path().join("mock-port"), "10035").unwrap();
    fs::write(
        home.path().join("mock-dns-name"),
        "no-such-default-hub.invalid.",
    )
    .unwrap();
    let start = cas_command(home.path(), bin.as_os_str())
        .args([
            "--json",
            "hub",
            "start",
            "--port",
            "0",
            "--tailscale-serve-port",
            "10035",
        ])
        .output()
        .unwrap();
    assert!(
        start.status.success(),
        "{}",
        String::from_utf8_lossy(&start.stderr)
    );
    let initial: Value = serde_json::from_slice(&start.stdout).unwrap();
    assert!(
        initial["public_url"].is_string(),
        "flagless start: {initial}"
    );
    let restart = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "restart", "--port", "0"])
        .output()
        .unwrap();
    assert!(
        restart.status.success(),
        "{}",
        String::from_utf8_lossy(&restart.stderr)
    );
    let paths = cas::hub::HubRuntimePaths::new(home.path().join(".cas/hub"));
    assert_eq!(
        paths.read_process_record().unwrap().tailscale_serve_port,
        Some(10035)
    );
    // Model binary replacement: the real loopback process remains the old hub.
    let mut old = paths.read_process_record().unwrap();
    old.version = "old-default-policy".to_owned();
    paths.write_process_record(&old).unwrap();
    let receipt_path = home.path().join("default-update.json");
    let update = cas_command(home.path(), bin.as_os_str())
        .args([
            "--json",
            "update",
            "--post-swap",
            "--from",
            "old-default-policy",
            "--refresh-receipt",
            receipt_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        update.status.success(),
        "{}",
        String::from_utf8_lossy(&update.stderr)
    );
    let receipt: Value = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    assert_eq!(receipt["hub_restart"]["verified"], true, "{receipt}");
    assert!(
        receipt["hub_restart"]["public_url"].is_string(),
        "{receipt}"
    );
    fs::write(
        home.path().join(".cas/config.toml"),
        "[hub]\ntailscale_serve = false\n",
    )
    .unwrap();
    let disabled = cas_command(home.path(), bin.as_os_str())
        .args([
            "--json",
            "update",
            "--post-swap",
            "--from",
            "old-default-policy",
            "--refresh-receipt",
            receipt_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        disabled.status.success(),
        "{}",
        String::from_utf8_lossy(&disabled.stderr)
    );
    let receipt: Value = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    assert_eq!(receipt["hub_restart"]["verified"], true, "{receipt}");
    assert!(receipt["hub_restart"]["public_url"].is_null(), "{receipt}");
    assert!(!home.path().join("mock-route").exists());
    let enabled = cas_command(home.path(), bin.as_os_str())
        .args([
            "--json",
            "hub",
            "start",
            "--port",
            "0",
            "--tailscale-serve",
            "--tailscale-serve-port",
            "10035",
        ])
        .output()
        .unwrap();
    assert!(
        enabled.status.success(),
        "{}",
        String::from_utf8_lossy(&enabled.stderr)
    );
    assert!(paths.read_process_record().unwrap().public_url.is_some());
    let explicit_off = cas_command(home.path(), bin.as_os_str())
        .args([
            "--json",
            "hub",
            "start",
            "--port",
            "0",
            "--loopback",
        ])
        .output()
        .unwrap();
    assert!(
        explicit_off.status.success(),
        "{}",
        String::from_utf8_lossy(&explicit_off.stderr)
    );
    assert!(paths.read_process_record().unwrap().public_url.is_none());
    assert!(!home.path().join("mock-route").exists());
}

#[cfg(unix)]
#[test]
fn signed_out_hub_start_requires_explicit_loopback() {
    let home = private_home();
    // This refusal is the explicit required policy; auto may retain loopback.
    fs::create_dir_all(home.path().join(".cas")).unwrap();
    fs::write(home.path().join(".cas/config.toml"), "[hub]\ntransport_mode = 'required'\n").unwrap();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    install_tailscale_mock(
        home.path(),
        &bin.join("tailscale"),
        "#!/bin/sh\nprintf '%s' '{\"BackendState\":\"NeedsLogin\",\"Self\":{}}'\n",
    );
    let start = cas_command(home.path(), bin.as_os_str())
        .args(["hub", "start", "--port", "0"])
        .output()
        .unwrap();
    assert!(!start.status.success(), "signed-out start must fail");
    assert!(String::from_utf8_lossy(&start.stderr).contains("--loopback"));
    let restart = cas_command(home.path(), bin.as_os_str())
        .args(["hub", "restart", "--port", "0"])
        .output().unwrap();
    assert!(!restart.status.success(), "signed-out restart must fail");
    for output in [&start, &restart] {
        let text = String::from_utf8_lossy(&output.stderr);
        let remedies: Vec<_> = text.lines().filter(|line|
            line.starts_with("Retry:") || line.starts_with("Or run ")
        ).collect();
        assert!(!remedies.is_empty(), "missing refusal remedy: {text}");
        assert!(remedies.iter().all(|line| line.chars().count() <= 80),
            "required refusal remedy exceeds 80 columns: {text}");
    }

    let json_start = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "start", "--port", "0"])
        .output()
        .unwrap();
    assert!(!json_start.status.success(), "signed-out JSON start must fail");
    let error: Value = serde_json::from_slice(&json_start.stdout)
        .expect("--json hub refusal must emit one JSON document");
    assert_eq!(error["status"], "error", "{error}");
    assert!(error["message"].as_str().unwrap_or_default().contains("--loopback"), "{error}");

    let paths = cas::hub::HubRuntimePaths::new(home.path().join(".cas/hub"));
    assert!(paths.read_process_record().is_err(), "refused start must not leave a hub");
    let loopback = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "start", "--port", "0", "--loopback"])
        .output()
        .unwrap();
    assert!(loopback.status.success(), "{}", String::from_utf8_lossy(&loopback.stderr));
    assert!(paths.read_process_record().unwrap().public_url.is_none());
}

#[cfg(unix)]
#[test]
fn stopped_tailscale_diagnostic_names_cli_and_socket() {
    let home = private_home();
    // Restart refuses a stopped daemon only under the required policy (cas-3f03e).
    fs::create_dir_all(home.path().join(".cas")).unwrap();
    fs::write(home.path().join(".cas/config.toml"), "[hub]\ntransport_mode = 'required'\n").unwrap();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let tailscale = bin.join("tailscale");
    install_tailscale_mock(home.path(), &tailscale,
        "#!/bin/sh\ncase \"$1\" in *missing.socket) exit 1 ;; esac\nprintf '%s' '{\"BackendState\":\"Stopped\",\"Self\":{\"DNSName\":\"stopped.tail.example.\"}}'\n");
    let socket = home.path().join("selected.socket");
    let start = cas_command(home.path(), bin.as_os_str())
        .args(["hub", "start", "--port", "0", "--no-tailscale-serve"])
        .output().unwrap();
    assert!(start.status.success());
    let paths = cas::hub::HubRuntimePaths::new(home.path().join(".cas/hub"));
    let mut record = paths.read_process_record().unwrap();
    record.launched_by = Some("service".into());
    record.tailscale_cli = Some(tailscale.display().to_string());
    record.transport_warning = Some("tailscale is not logged in or has no MagicDNS name".into());
    paths.write_process_record(&record).unwrap();
    let mut receipt: Value = serde_json::from_str(include_str!("../src/hub/fixtures/tailscale-serve-v2.60.0.json")).unwrap();
    receipt["socket"] = Value::String(socket.display().to_string());
    fs::write(paths.root().join("tailscale-serve.json"), serde_json::to_vec(&receipt).unwrap()).unwrap();
    for explicit in [None, Some(home.path().join("override.socket")), Some(home.path().join("missing.socket"))] {
        let selected = explicit.as_ref().unwrap_or(&socket);
        for args in [vec!["hub", "status"], vec!["hub", "restart", "--port", "0"]] {
            let is_restart = args.contains(&"restart");
            let mut command = cas_command(home.path(), bin.as_os_str());
            if let Some(explicit) = &explicit { command.env("TAILSCALE_SOCKET", explicit); }
            let output = command.args(args).output().unwrap();
            assert!(!output.status.success());
            let text = format!("{}{}", String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr));
            let cause = if selected.ends_with("missing.socket") { "probe failed" } else { "Stopped" };
            for expected in [cause, "TAILSCALE_SOCKET", tailscale.to_str().unwrap(), selected.to_str().unwrap()] {
                assert!(text.contains(expected), "missing {expected}: {text}");
            }
            if is_restart {
                for expected in [
                    "Hub transport required: Tailscale is unavailable.",
                    "Restart refused; existing hub (pid ",
                    ") remains running.",
                    "Retry: TAILSCALE_SOCKET=<signed-in-socket> cas hub restart.",
                ] {
                    assert!(text.contains(expected), "missing {expected}: {text}");
                }
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn hub_auto_fallback_warning_names_retry_and_fits_80_columns() {
    let home = private_home();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    install_tailscale_mock(home.path(), &bin.join("tailscale"), "#!/bin/sh\nexit 1\n");
    let mut path_entries = vec![bin.clone()];
    path_entries.extend(std::env::split_paths(&system_path()));
    let path = std::env::join_paths(path_entries).unwrap();

    let start = cas_command(home.path(), &path)
        .args(["hub", "start", "--port", "0"])
        .output()
        .unwrap();
    assert!(start.status.success(), "{}", String::from_utf8_lossy(&start.stderr));
    let paths = cas::hub::HubRuntimePaths::new(home.path().join(".cas/hub"));
    let mut cleanup = LegacyHubCleanup(vec![paths.read_process_record().unwrap().pid]);

    let restart = cas_command(home.path(), &path)
        .args(["hub", "restart", "--port", "0"])
        .output()
        .unwrap();
    assert!(restart.status.success(), "{}", String::from_utf8_lossy(&restart.stderr));
    let record = paths.read_process_record().unwrap();
    cleanup.0.clear();
    cleanup.0.push(record.pid);

    let warning = String::from_utf8_lossy(&restart.stderr);
    for expected in [
        "Tailscale Serve inactive; hub is loopback-only.",
        "Cause:",
        "Retry: tailscale status && cas hub restart",
    ] {
        assert!(warning.contains(expected), "missing {expected}: {warning}");
    }
    assert!(
        warning.lines().all(|line| line.chars().count() <= 80),
        "warning exceeds 80 columns: {warning}"
    );

    let stop = cas_command(home.path(), &path)
        .args(["hub", "stop", "--force"])
        .output()
        .unwrap();
    assert!(stop.status.success(), "{}", String::from_utf8_lossy(&stop.stderr));
    cleanup.0.clear();
}

#[cfg(unix)]
#[test]
fn post_swap_starts_known_stopped_hub_and_serve_failure_keeps_update_successful() {
    let home = private_home();
    let path = system_path();
    let start = cas_command(home.path(), &path)
        .args([
            "--json",
            "hub",
            "start",
            "--port",
            "0",
            "--no-tailscale-serve",
        ])
        .output()
        .unwrap();
    assert!(
        start.status.success(),
        "{}",
        String::from_utf8_lossy(&start.stderr)
    );
    let stop = cas_command(home.path(), &path)
        .args(["--json", "hub", "stop"])
        .output()
        .unwrap();
    assert!(
        stop.status.success(),
        "{}",
        String::from_utf8_lossy(&stop.stderr)
    );
    assert!(home.path().join(".cas/hub/machine-id").is_file());
    let receipt_path = home.path().join("loopback-update.json");
    let update = cas_command(home.path(), &path)
        .args([
            "--json",
            "update",
            "--post-swap",
            "--from",
            "old",
            "--refresh-receipt",
            receipt_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        update.status.success(),
        "{}",
        String::from_utf8_lossy(&update.stderr)
    );
    let receipt: Value = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    let hub = &receipt["hub_restart"];
    assert_eq!(hub["action"], "started", "{receipt}");
    assert_eq!(hub["verified"], true, "{receipt}");
    assert_eq!(hub["loopback_verified"], true, "{receipt}");
    assert_eq!(hub["transport_verified"], false, "{receipt}");
    assert_eq!(hub["recovery_attempted"], false, "{receipt}");
    assert!(hub["failure"].is_null(), "{receipt}");
    let warning = hub["transport_warning"].as_str().unwrap();
    assert!(warning.contains("Tailscale Serve inactive"), "{warning}");
    assert!(hub["remedy"].as_str().unwrap().contains("cas hub restart"));
}

#[cfg(target_os = "linux")]
#[test]
fn post_swap_starts_an_installed_stopped_service_with_default_serve_policy() {
    let home = private_home();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let binary = cas::test_paths::cas_binary();
    let unit = home.path().join(".config/systemd/user/cas-hub.service");
    fs::create_dir_all(unit.parent().unwrap()).unwrap();
    fs::write(&unit, format!("[Service]\nExecStart={} hub serve --bind 127.0.0.1 --port {port} --launched-by service\nRestart=on-failure\n", binary.display())).unwrap();
    let manager = bin.join("systemctl");
    cas::test_paths::warm_stub(
        &manager,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$HOME/manager-calls"
case "$*" in
  '--user --version'|'--user daemon-reload') exit 0 ;;
  '--user is-active --quiet cas-hub.service') exit 1 ;;
  '--user restart cas-hub.service')
    if [ "$TEST_START_FAIL" = 1 ]; then exit 9; fi
    if /usr/bin/grep -q -- '--tailscale-serve ' "$HOME/.config/systemd/user/cas-hub.service"; then
      publication=--tailscale-serve
    else
      publication=--no-tailscale-serve
    fi
    /usr/bin/setsid "$TEST_CAS_BINARY" hub serve --bind 127.0.0.1 --port "$CAS_HUB_SERVICE_PORT" --launched-by service "$publication" >> "$HOME/service.log" 2>&1 < /dev/null &
    exit 0 ;;
  *) exit 9 ;;
esac
"#,
    );
    let receipt_path = home.path().join("service-update.json");
    let update = cas_command(home.path(), bin.as_os_str())
        .env("CAS_HUB_SYSTEMCTL", &manager)
        .env("TEST_CAS_BINARY", &binary)
        .env("CAS_HUB_LAUNCHD_LABEL", "test.cas.d8e7")
        .env("CAS_HUB_SERVICE_PORT", port.to_string())
        .args([
            "--json",
            "update",
            "--post-swap",
            "--from",
            "old",
            "--refresh-receipt",
            receipt_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        update.status.success(),
        "{}",
        String::from_utf8_lossy(&update.stderr)
    );
    let receipt: Value = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    let hub = &receipt["hub_restart"];
    assert_eq!(hub["action"], "started", "{receipt}");
    assert_eq!(hub["via"], "service", "{receipt}");
    assert_eq!(hub["verified"], true, "{receipt}");
    assert_eq!(hub["loopback_verified"], true, "{receipt}");
    assert_eq!(hub["transport_verified"], false, "{receipt}");
    assert!(
        hub["transport_warning"]
            .as_str()
            .unwrap()
            .contains("Tailscale Serve inactive")
    );
    let definition = fs::read_to_string(unit).unwrap();
    assert!(definition.contains("--tailscale-serve --tailscale-serve-port 443"));
    assert!(
        definition.contains(&format!("--port {port}")),
        "listener preserved"
    );
    assert!(
        fs::read_to_string(home.path().join("manager-calls"))
            .unwrap()
            .contains("--user restart cas-hub.service")
    );
    assert_eq!(
        cas::hub::HubRuntimePaths::new(home.path().join(".cas/hub"))
            .read_process_record()
            .unwrap()
            .launched_by
            .as_deref(),
        Some("service")
    );
    let stop = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "stop"])
        .output()
        .unwrap();
    assert!(
        stop.status.success(),
        "{}",
        String::from_utf8_lossy(&stop.stderr)
    );
    let failed_start = cas_command(home.path(), bin.as_os_str())
        .env("CAS_HUB_SYSTEMCTL", &manager)
        .env("TEST_START_FAIL", "1")
        .env("CAS_HUB_LAUNCHD_LABEL", "test.cas.d8e7")
        .env("CAS_HUB_SERVICE_PORT", port.to_string())
        .args([
            "--json",
            "update",
            "--post-swap",
            "--from",
            "old",
            "--refresh-receipt",
            receipt_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        failed_start.status.success(),
        "best-effort start must not fail update: {}",
        String::from_utf8_lossy(&failed_start.stderr)
    );
    let receipt: Value = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    assert_eq!(
        receipt["hub_restart"]["action"], "start_failed",
        "{receipt}"
    );
    assert_eq!(receipt["hub_restart"]["verified"], false, "{receipt}");
    assert!(receipt["hub_restart"]["failure"].is_string(), "{receipt}");
}

/// Run only through scripts/run-hub-update-legacy-matrix.sh, which builds the
/// three real tagged binaries and points this process at them.
#[cfg(unix)]
#[test]
#[ignore = "requires real tagged hub binaries; use scripts/run-hub-update-legacy-matrix.sh"]
fn real_legacy_hubs_recover_through_new_post_swap_step() {
    let binary_root = PathBuf::from(std::env::var("CAS_TEST_OLD_HUB_BIN_DIR").expect("old binary directory"));
    let new_binary = cas::test_paths::cas_binary();
    if std::env::var("CAS_TEST_LEGACY_TAG").is_err() && std::env::var("CAS_TEST_LEGACY_STATE").is_err() {
        let empty_home = private_home();
        let receipt_path = empty_home.path().join("none-update.json");
        let update = legacy_hub_command(&new_binary, empty_home.path(), Path::new("/usr/bin"))
            .args(["--json", "update", "--post-swap", "--from", "3.27.8", "--refresh-receipt", receipt_path.to_str().unwrap()])
            .output().unwrap();
        assert!(update.status.success(), "no prior hub should remain stopped: {}", String::from_utf8_lossy(&update.stderr));
        let receipt: Value = serde_json::from_slice(&fs::read(receipt_path).unwrap()).unwrap();
        assert_eq!(receipt["hub_restart"]["prior_state"], "none");
        assert_eq!(receipt["hub_restart"]["action"], "skipped");
        assert!(!empty_home.path().join(".cas/hub/process.json").exists());
    }
    for tag in ["v3.27.8", "v3.28.1", "v3.28.2"] {
        if std::env::var("CAS_TEST_LEGACY_TAG").ok().is_some_and(|only| only != tag) {
            continue;
        }
        let old_binary = binary_root.join(tag).join("cas");
        assert!(old_binary.is_file(), "missing real {tag} binary at {}", old_binary.display());
        for state in ["healthy", "unresponsive", "dead_route", "stuck_starting"] {
            if std::env::var("CAS_TEST_LEGACY_STATE").ok().is_some_and(|only| only != state) {
                continue;
            }
            eprintln!("legacy update matrix: {tag}/{state}");
            let home = private_home();
            let bin = home.path().join("bin");
            fs::create_dir(&bin).unwrap();
            install_tailscale_mock(home.path(), &bin.join("tailscale"), include_str!("fixtures/hub_update_mock_tailscale.sh"));
            fs::write(home.path().join("mock-port"), "10031").unwrap();
            let hub_port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
            let start = legacy_hub_command(&old_binary, home.path(), &bin)
                .args(["hub", "--tailscale-serve", "--tailscale-serve-port", "10031", "start", "--port", &hub_port.to_string()])
                .output().unwrap();
            assert!(start.status.success(), "{tag}/{state} old start: {}", String::from_utf8_lossy(&start.stderr));
            let paths = cas::hub::HubRuntimePaths::new(home.path().join(".cas/hub"));
            let old = paths.read_process_record().unwrap();
            let mut cleanup = LegacyHubCleanup(vec![old.pid]);
            assert!(home.path().join("mock-route").exists(), "old hub did not publish route");

            if tag == "v3.27.8" && state == "healthy" {
                // Pre-transport-field process.json and pre-phase hub.lock.
                let record_path = home.path().join(".cas/hub/process.json");
                let mut record: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
                for key in ["tailscale_cli", "tailscale_serve_port", "tailscale_serve_target", "public_url"] {
                    record.as_object_mut().unwrap().remove(key);
                }
                fs::write(record_path, serde_json::to_vec(&record).unwrap()).unwrap();
                let lock_path = home.path().join(".cas/hub/hub.lock");
                let mut lock: Value = serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
                lock.as_object_mut().unwrap().remove("phase");
                fs::write(lock_path, serde_json::to_vec(&lock).unwrap()).unwrap();
            }
            match state {
                "unresponsive" => { unsafe { libc::kill(old.pid as i32, libc::SIGSTOP) }; }
                "dead_route" => {
                    unsafe { libc::kill(old.pid as i32, libc::SIGKILL) };
                    let dead_deadline = Instant::now() + CHILD_PROGRESS_TIMEOUT;
                    while unsafe { libc::kill(old.pid as i32, 0) } == 0 && Instant::now() < dead_deadline {
                        thread::sleep(Duration::from_millis(20));
                    }
                    assert_ne!(unsafe { libc::kill(old.pid as i32, 0) }, 0, "old hub must be dead before recovery");
                    if tag == "v3.27.8" {
                        // Receipt after publication, but no process record.
                        fs::remove_file(home.path().join(".cas/hub/process.json")).unwrap();
                    } else if tag == "v3.28.1" {
                        let receipt_path = home.path().join(".cas/hub/tailscale-serve.json");
                        let mut receipt: Value = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
                        receipt.as_object_mut().unwrap().remove("executable");
                        fs::write(receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
                    } else {
                        fs::remove_file(home.path().join(".cas/hub/tailscale-serve.json")).unwrap();
                    }
                }
                "stuck_starting" => {
                    let lock_path = home.path().join(".cas/hub/hub.lock");
                    let mut lock: Value = serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
                    lock["phase"] = Value::String("starting".to_owned());
                    lock["acquired_at"] = Value::String("2025-01-01T00:00:00Z".to_owned());
                    fs::write(lock_path, serde_json::to_vec(&lock).unwrap()).unwrap();
                    unsafe { libc::kill(old.pid as i32, libc::SIGSTOP) };
                }
                _ => {}
            }
            let mut updated_pid = None;
            for iteration in 0..2 {
                let receipt_path = home.path().join(format!("update-{iteration}.json"));
                let update = legacy_hub_command(&new_binary, home.path(), &bin)
                    .args(["--json", "update", "--post-swap", "--from", tag.trim_start_matches('v'), "--refresh-receipt", receipt_path.to_str().unwrap()])
                    .output().unwrap();
                assert!(update.status.success(), "{tag}/{state}/{iteration} update: {}", String::from_utf8_lossy(&update.stderr));
                let receipt: Value = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
                let expected_state = if iteration != 0 || state == "healthy" { "running" } else if state == "dead_route" { "exited" } else if state == "stuck_starting" { "startup_wedged" } else { state };
                assert_eq!(receipt["hub_restart"]["prior_state"], expected_state, "{tag}/{state}/{iteration}: {receipt}");
                let expected_action = if iteration == 0 {
                    if state == "dead_route" {
                        "started"
                    } else {
                        "restarted"
                    }
                } else {
                    "verified"
                };
                assert_eq!(receipt["hub_restart"]["action"], expected_action, "{receipt}");
                if receipt["hub_restart"]["loopback_verified"] != true {
                    eprintln!("hub.log: {}", fs::read_to_string(home.path().join(".cas/hub/hub.log")).unwrap_or_default());
                    eprintln!("lock: {}", fs::read_to_string(home.path().join(".cas/hub/hub.lock")).unwrap_or_default());
                    eprintln!("old pid ps: {}", String::from_utf8_lossy(&ProcessCommand::new("ps").args(["-p", &old.pid.to_string(), "-o", "pid,ppid,state,command"]).output().unwrap().stdout));
                }
                assert_eq!(receipt["hub_restart"]["verified"], true, "{receipt}");
                assert_eq!(receipt["hub_restart"]["recovery_attempted"], false, "{receipt}");
                assert_eq!(receipt["hub_restart"]["loopback_verified"], true, "{receipt}");
                assert_eq!(receipt["hub_restart"]["transport_verified"], false, "{receipt}");
                assert!(receipt["hub_restart"]["transport_warning"].as_str().unwrap().contains("public Tailscale"), "{receipt}");
                assert!(receipt["hub_restart"]["remedy"].as_str().unwrap().contains("MagicDNS"), "{receipt}");
                assert!(receipt["hub_restart"]["failure"].is_null(), "{receipt}");
                let current = paths.read_process_record().unwrap();
                if let Some(previous_pid) = updated_pid {
                    assert_eq!(current.pid, previous_pid, "a healthy current hub must keep its PID");
                }
                updated_pid = Some(current.pid);
                cleanup.0.push(current.pid);
                assert_eq!(current.version, env!("CARGO_PKG_VERSION"));
                assert!(home.path().join("mock-route").exists(), "{tag}/{state}/{iteration} route lost");
                assert_eq!(fs::read_to_string(home.path().join("mock-route")).unwrap(), current.tailscale_serve_target.unwrap());
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn healthy_current_hub_with_unresolvable_public_url_keeps_its_pid() {
    let home = private_home();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    install_tailscale_mock(
        home.path(),
        &bin.join("tailscale"),
        include_str!("fixtures/hub_update_mock_tailscale.sh"),
    );
    fs::write(home.path().join("mock-port"), "10031").unwrap();
    fs::write(home.path().join("mock-dns-name"), "no-such-cas-hub.invalid.").unwrap();
    let hub_port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let binary = cas::test_paths::cas_binary();
    let start = legacy_hub_command(&binary, home.path(), &bin)
        .args([
            "hub",
            "--tailscale-serve",
            "--tailscale-serve-port",
            "10031",
            "start",
            "--port",
            &hub_port.to_string(),
        ])
        .output()
        .unwrap();
    assert!(start.status.success(), "{}", String::from_utf8_lossy(&start.stderr));
    let paths = cas::hub::HubRuntimePaths::new(home.path().join(".cas/hub"));
    let pid = paths.read_process_record().unwrap().pid;
    let _cleanup = LegacyHubCleanup(vec![pid]);
    let original_route = fs::read_to_string(home.path().join("mock-route")).unwrap();
    for iteration in 0..2 {
        let receipt_path = home.path().join(format!("current-update-{iteration}.json"));
        let update = legacy_hub_command(&binary, home.path(), &bin)
            .args([
                "--json",
                "update",
                "--post-swap",
                "--from",
                env!("CARGO_PKG_VERSION"),
                "--refresh-receipt",
                receipt_path.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(update.status.success(), "{}", String::from_utf8_lossy(&update.stderr));
        let receipt: Value = serde_json::from_slice(&fs::read(receipt_path).unwrap()).unwrap();
        assert_eq!(receipt["hub_restart"]["action"], "verified", "{receipt}");
        assert_eq!(receipt["hub_restart"]["verified"], true, "{receipt}");
        assert_eq!(receipt["hub_restart"]["loopback_verified"], true, "{receipt}");
        assert_eq!(receipt["hub_restart"]["transport_verified"], false, "{receipt}");
        assert_eq!(receipt["hub_restart"]["recovery_attempted"], false, "{receipt}");
        assert!(receipt["hub_restart"]["transport_warning"]
            .as_str()
            .unwrap()
            .contains("no-such-cas-hub.invalid"), "{receipt}");
        assert!(receipt["hub_restart"]["remedy"]
            .as_str()
            .unwrap()
            .contains("MagicDNS"), "{receipt}");
        assert_eq!(paths.read_process_record().unwrap().pid, pid);
        assert_eq!(fs::read_to_string(home.path().join("mock-route")).unwrap(), original_route);
    }
}

/// Exercise the shared update verifier through a real, isolated launchd
/// service. This is opt-in because it needs a signed-in local Tailscale node.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires real launchd and Tailscale; set CAS_TEST_REAL_TAILSCALE_SERVICE_PORT"]
fn real_launchd_service_update_restarts_stale_then_preserves_current_pid() {
    let serve_port: u16 = std::env::var("CAS_TEST_REAL_TAILSCALE_SERVICE_PORT")
        .expect("non-443 Tailscale Serve port")
        .parse()
        .unwrap();
    assert_ne!(serve_port, 443);
    let home = private_home();
    let bin = home.path().join("cas");
    fs::copy(cas::test_paths::cas_binary(), &bin).unwrap();
    let hub_port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let label = format!("dev.cas.update-proof-{}", std::process::id());
    let app_cli = std::env::var_os("CAS_TEST_TAILSCALE_APP_CLI");
    let command = |args: &[&str]| {
        let mut command = ProcessCommand::new(&bin);
        command
            .args(args)
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("CAS_SKIP_FACTORY_TOOLING", "1")
            .env("CAS_HUB_LAUNCHD_LABEL", &label)
            .env("CAS_HUB_SERVICE_PORT", hub_port.to_string());
        if let Some(app_cli) = &app_cli {
            command.env("TAILSCALE", app_cli);
        }
        command.output().unwrap()
    };
    struct Cleanup<'a>(&'a dyn Fn(&[&str]) -> std::process::Output);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = (self.0)(&["hub", "service", "uninstall"]);
            let _ = (self.0)(&["hub", "stop", "--force"]);
        }
    }
    let _cleanup = Cleanup(&command);
    let install = command(&["hub", "service", "install"]);
    assert!(
        install.status.success(),
        "service install: {}",
        String::from_utf8_lossy(&install.stderr)
    );
    let publish = command(&[
        "hub",
        "--tailscale-serve",
        "--tailscale-serve-port",
        &serve_port.to_string(),
        "restart",
    ]);
    assert!(
        publish.status.success(),
        "service publication: {}",
        String::from_utf8_lossy(&publish.stderr)
    );
    if let Some(app_cli) = &app_cli {
        let receipt: Value = serde_json::from_slice(
            &fs::read(home.path().join(".cas/hub/tailscale-serve.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt["executable"], app_cli.to_string_lossy().as_ref());
    }
    let paths = cas::hub::HubRuntimePaths::new(home.path().join(".cas/hub"));
    let first_pid = paths.read_process_record().unwrap().pid;
    // The service process is real launchd; only its version metadata is made
    // stale so the first update must enter restart_supervised and its verifier.
    let mut stale_record = paths.read_process_record().unwrap();
    stale_record.version = "3.27.8".to_owned();
    paths.write_process_record(&stale_record).unwrap();
    let mut restarted_pid = None;
    for iteration in 0..3 {
        let receipt_path = home.path().join(format!("service-update-{iteration}.json"));
        let update = command(&[
            "--json",
            "update",
            "--post-swap",
            "--from",
            "3.27.8",
            "--refresh-receipt",
            receipt_path.to_str().unwrap(),
        ]);
        assert!(
            update.status.success(),
            "service update {iteration}: {}",
            String::from_utf8_lossy(&update.stderr)
        );
        let receipt: Value = serde_json::from_slice(&fs::read(receipt_path).unwrap()).unwrap();
        assert_eq!(receipt["hub_restart"]["via"], "service");
        assert_eq!(receipt["hub_restart"]["verified"], true);
        assert_eq!(receipt["hub_restart"]["transport_verified"], true);
        assert_eq!(receipt["hub_restart"]["recovery_attempted"], false);
        let record = paths.read_process_record().unwrap();
        if iteration == 0 {
            assert_ne!(record.pid, first_pid, "stale service must restart");
            assert_eq!(receipt["hub_restart"]["action"], "restarted");
            restarted_pid = Some(record.pid);
        } else {
            assert_eq!(record.pid, restarted_pid.unwrap(), "healthy current service must keep its PID");
            assert_eq!(receipt["hub_restart"]["action"], "verified");
        }
        assert_eq!(record.port, hub_port);
        assert_eq!(record.tailscale_serve_port, Some(serve_port));
    }
}

/// No real Tailscale state or operator launchd label is used by this journey.
#[cfg(target_os = "macos")]
#[test]
fn isolated_launchd_lifecycle_stops_without_respawn_and_restarts() {
    let home = private_home();
    let binary = home.path().join("cas");
    fs::copy(cas::test_paths::cas_binary(), &binary).unwrap();
    let tailscale = home.path().join("tailscale");
    let script = format!("#!/bin/sh\n[ \"${{TAILSCALE_BE_CLI:-}}\" = 1 ] || exit 9\ncase \"$1\" in \"--socket=$HOME/fixture.socket\"|\"--socket=$HOME/updated.socket\") ;; *) exit 9 ;; esac\nprintf '%s' \"$1\" > \"$HOME/socket-used\"\nshift\n{}",
        include_str!("fixtures/hub_update_mock_tailscale.sh").trim_start_matches("#!/bin/sh\n"));
    install_tailscale_mock(home.path(), &tailscale, &script);
    fs::write(home.path().join("mock-port"), "10046").unwrap();
    fs::write(home.path().join("mock-dns-name"), "launchd-proof.invalid.").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let label = format!("dev.cas.lifecycle-proof-{}", std::process::id());
    let target = format!("gui/{}/{label}", unsafe { libc::geteuid() });
    let socket = std::cell::Cell::new(Some("fixture.socket"));
    let command = |args: &[&str]| {
        let mut command = ProcessCommand::new(&binary);
        command.env_clear()
            .env("HOME", home.path()).env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("TAILSCALE", &tailscale).env("CAS_SKIP_FACTORY_TOOLING", "1")
            .env("CAS_HUB_LAUNCHD_LABEL", &label)
            .env("CAS_HUB_SERVICE_PORT", port.to_string());
        if let Some(socket) = socket.get() {
            command.env("TAILSCALE_SOCKET", home.path().join(socket));
        }
        command.args(args).output().unwrap()
    };
    struct Cleanup<'a>(&'a dyn Fn(&[&str]) -> std::process::Output);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) { let _ = (self.0)(&["hub", "service", "uninstall"]); }
    }
    let _cleanup = Cleanup(&command);
    assert!(command(&["--version"]).status.success());
    let install = command(&["hub", "service", "install", "--tailscale-serve-port", "10046"]);
    assert!(install.status.success(), "{}", String::from_utf8_lossy(&install.stderr));
    let paths = cas::hub::HubRuntimePaths::new(home.path().join(".cas/hub"));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if paths.read_process_record().is_ok_and(|record| record.public_url.is_some())
            && ureq::get(&format!("http://127.0.0.1:{port}/v1/health"))
                .timeout(Duration::from_millis(100)).call().is_ok() { break; }
        assert!(Instant::now() < deadline, "isolated launchd hub did not become ready");
        thread::sleep(Duration::from_millis(20));
    }
    let initial_pid = paths.read_process_record().unwrap().pid;
    let keep = command(&["hub", "start", "--port", &port.to_string()]);
    assert!(keep.status.success(), "{}", String::from_utf8_lossy(&keep.stderr));
    assert_eq!(paths.read_process_record().unwrap().pid, initial_pid);
    let loopback = command(&["hub", "start", "--loopback", "--port", &port.to_string()]);
    assert!(loopback.status.success(), "{}", String::from_utf8_lossy(&loopback.stderr));
    let local = paths.read_process_record().unwrap();
    assert!(local.public_url.is_none());
    assert_eq!(local.launched_by.as_deref(), Some("service"));
    assert!(!home.path().join("mock-route").exists());
    let publish = command(&["hub", "start"]);
    assert!(publish.status.success(), "{}", String::from_utf8_lossy(&publish.stderr));
    let first_pid = paths.read_process_record().unwrap().pid;
    assert_eq!(paths.read_process_record().unwrap().launched_by.as_deref(), Some("service"));
    assert!(paths.read_process_record().unwrap().public_url.is_some());
    socket.set(None);
    let stopped_at = Instant::now();
    let stop = command(&["hub", "stop"]);
    assert!(stop.status.success(), "{}", String::from_utf8_lossy(&stop.stderr));
    assert!(stopped_at.elapsed() < Duration::from_secs(15));
    // An absent bootstrap job cannot respawn, regardless of launchd throttle timing.
    assert!(!ProcessCommand::new("/bin/launchctl").args(["print", &target])
        .stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success());
    assert!(paths.read_process_record().is_err());
    assert!(!home.path().join("mock-route").exists());
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    let plist = home.path().join(format!("Library/LaunchAgents/{label}.plist"));
    assert!(plist.is_file(), "stop must keep the installed service definition");
    assert!(command(&["hub", "stop"]).status.success(), "stop is idempotent");
    socket.set(Some("updated.socket"));
    let restarted_at = Instant::now();
    let restart = command(&["hub", "restart"]);
    assert!(restart.status.success(), "{}", String::from_utf8_lossy(&restart.stderr));
    assert!(restarted_at.elapsed() < Duration::from_secs(15));
    let record = paths.read_process_record().unwrap();
    assert_ne!(record.pid, first_pid);
    assert_eq!(record.public_url.as_deref(), Some("https://launchd-proof.invalid:10046/"));
    let selected_socket = home.path().join("updated.socket");
    let definition_socket = ProcessCommand::new("/usr/bin/plutil")
        .args(["-extract", "EnvironmentVariables.TAILSCALE_SOCKET", "raw", "-o", "-"])
        .arg(&plist).output().unwrap();
    assert!(definition_socket.status.success());
    assert_eq!(String::from_utf8_lossy(&definition_socket.stdout).trim(),
        selected_socket.to_str().unwrap());
    assert_eq!(fs::read_to_string(home.path().join("socket-used")).unwrap(),
        format!("--socket={}", selected_socket.display()));
    socket.set(None);
    assert!(command(&["hub", "status"]).status.success(), "plain status must use the recorded socket");
    let mut stale = record;
    stale.version = "old-launchd-proof".into();
    paths.write_process_record(&stale).unwrap();
    let receipt_path = home.path().join("update.json");
    let updated_at = Instant::now();
    let update = command(&["--json", "update", "--post-swap", "--from", "old-launchd-proof",
        "--refresh-receipt", receipt_path.to_str().unwrap()]);
    assert!(update.status.success(), "{}", String::from_utf8_lossy(&update.stderr));
    assert!(updated_at.elapsed() < Duration::from_secs(15));
    let receipt: Value = serde_json::from_slice(&fs::read(receipt_path).unwrap()).unwrap();
    assert_eq!(receipt["hub_restart"]["verified"], true);
    assert_eq!(paths.read_process_record().unwrap().public_url.as_deref(),
        Some("https://launchd-proof.invalid:10046/"));
}

#[cfg(target_os = "macos")]
#[test]
fn service_start_retries_tailscale_before_publication() {
    let home = private_home();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let script = format!("#!/bin/sh\nif [ \"$*\" = 'status --json' ]; then\n count=0\n if [ -f \"$HOME/probes\" ]; then count=$(/bin/cat \"$HOME/probes\"); fi\n count=$((count + 1))\n printf '%s' \"$count\" > \"$HOME/probes\"\n if [ \"$count\" -lt 3 ]; then printf '%s' '{{\"BackendState\":\"Stopped\",\"Self\":{{}}}}'; exit 0; fi\nfi\n{}",
        include_str!("fixtures/hub_update_mock_tailscale.sh").trim_start_matches("#!/bin/sh\n"));
    install_tailscale_mock(home.path(), &bin.join("tailscale"), &script);
    fs::write(home.path().join("mock-port"), "10047").unwrap();
    let mut child = cas_process_command(home.path(), bin.as_os_str())
        .args(["hub", "serve", "--port", "0", "--launched-by", "service",
            "--tailscale-serve-port", "10047"])
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let _cleanup = LegacyHubCleanup(vec![child.id()]);
    let paths = cas::hub::HubRuntimePaths::new(home.path().join(".cas/hub"));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if paths.read_lock_owner().is_some_and(|owner| owner.phase == "running") { break; }
        assert!(Instant::now() < deadline && child.try_wait().unwrap().is_none(),
            "service never completed publication");
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(paths.read_process_record().unwrap().public_url.as_deref(),
        Some("https://localhost:10047/"));
}

#[cfg(unix)]
#[test]
fn foreground_start_carries_legacy_target_across_process_record_write() {
    let home = private_home();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let tailscale = bin.join("tailscale");
    install_tailscale_mock(
        home.path(),
        &tailscale,
        r##"#!/bin/sh
case "$*" in
  'status --json') printf '%s' '{"Self":{"DNSName":"legacy.tail.example."}}' ;;
  'serve status --json')
    if [ -f "$HOME/mock-serve" ]; then
      target=$(/bin/cat "$HOME/mock-serve")
      printf '{"Web":{"legacy.tail.example:443":{"Handlers":{"/":{"Proxy":"%s"}}}}}' "$target"
    else printf '%s' '{}'; fi ;;
  'serve --bg --yes --https=443 '*) printf '%s' "$5" > "$HOME/mock-serve" ;;
  'serve --https=443 off') /bin/rm -f "$HOME/mock-serve"; printf '%s' off >> "$HOME/mock-calls" ;;
  *) exit 9 ;;
esac
"##,
    );

    let (dead_port, _dead_port_reservation) = reserve_refused_loopback_port();
    let old_target = format!("http://127.0.0.1:{dead_port}");
    let paths = cas::hub::HubRuntimePaths::new(home.path().join(".cas/hub"));
    let old_record: cas::hub::HubProcessRecord = serde_json::from_value(serde_json::json!({
        "pid": 999999, "bind": "127.0.0.1", "port": 4173, "version": "3.27.8",
        "started_at": "2026-01-01T00:00:00Z", "tailscale_serve_port": 443,
        "tailscale_serve_target": old_target
    }))
    .unwrap();
    paths.write_process_record(&old_record).unwrap();
    fs::write(home.path().join("mock-serve"), &old_target).unwrap();

    let mut child = cas_process_command(home.path(), bin.as_os_str())
        .args(["hub", "serve", "--port", "0", "--tailscale-serve"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut replacement = None;
    while Instant::now() < deadline {
        if let Ok(record) = paths.read_process_record() {
            if record.pid == child.id() && record.tailscale_serve_target.is_some() {
                replacement = record.tailscale_serve_target;
                break;
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
    let replacement = replacement.unwrap_or_else(|| {
        let _ = child.kill();
        let _ = child.wait();
        let mut stderr = String::new();
        let _ = child.stderr.take().unwrap().read_to_string(&mut stderr);
        panic!("foreground start must publish the replacement shim: {stderr}");
    });
    assert_ne!(replacement, old_target);
    assert_eq!(
        fs::read_to_string(home.path().join("mock-serve")).unwrap(),
        replacement
    );
    assert!(
        fs::read_to_string(home.path().join("mock-calls"))
            .unwrap()
            .contains("off")
    );
    let published_port = paths.read_process_record().unwrap().port;
    let health_url = format!("http://127.0.0.1:{published_port}/v1/health");
    let deadline = Instant::now() + CHILD_PROGRESS_TIMEOUT;
    while Instant::now() < deadline {
        if ureq::get(&health_url)
            .timeout(Duration::from_millis(200))
            .call()
            .is_ok()
        {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    assert!(
        ureq::get(&health_url)
            .timeout(Duration::from_secs(1))
            .call()
            .is_ok()
    );
    // Simulate a legacy hub whose publish completed but receipt write did not.
    // Its foreground teardown must still see process.json before deleting it.
    fs::remove_file(home.path().join(".cas/hub/tailscale-serve.json")).unwrap();
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(child.id() as i32),
        nix::sys::signal::Signal::SIGTERM,
    )
    .unwrap();
    let _ = child.wait().unwrap();
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(!home.path().join("mock-serve").exists(), "{stderr}");
}

#[cfg(unix)]
#[test]
fn detached_start_forwards_prior_target_after_failed_launcher_cleanup() {
    let home = private_home();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let tailscale = bin.join("tailscale");
    install_tailscale_mock(
        home.path(),
        &tailscale,
        r##"#!/bin/sh
case "$*" in
  'status --json') printf '%s' '{"Self":{"DNSName":"legacy.tail.example."}}' ;;
  'serve status --json')
    if [ -f "$HOME/mock-serve" ]; then
      target=$(/bin/cat "$HOME/mock-serve")
      printf '{"Web":{"legacy.tail.example:443":{"Handlers":{"/":{"Proxy":"%s"}}}}}' "$target"
    else printf '%s' '{}'; fi ;;
  'serve --bg --yes --https=443 '*) printf '%s' "$5" > "$HOME/mock-serve" ;;
  'serve --https=443 off')
    if [ ! -f "$HOME/first-off-failed" ]; then
      : > "$HOME/first-off-failed"
      exit 9
    fi
    /bin/rm -f "$HOME/mock-serve" ;;
  *) exit 9 ;;
esac
"##,
    );
    let (dead_port, _dead_port_reservation) = reserve_refused_loopback_port();
    let old_target = format!("http://127.0.0.1:{dead_port}");
    let paths = cas::hub::HubRuntimePaths::new(home.path().join(".cas/hub"));
    let old_record: cas::hub::HubProcessRecord = serde_json::from_value(serde_json::json!({
        "pid": 999999, "bind": "127.0.0.1", "port": 4173, "version": "3.27.8",
        "started_at": "2026-01-01T00:00:00Z", "tailscale_serve_port": 443,
        "tailscale_serve_target": old_target
    }))
    .unwrap();
    paths.write_process_record(&old_record).unwrap();
    fs::write(home.path().join("mock-serve"), &old_target).unwrap();

    let start = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "start", "--port", "0", "--tailscale-serve"])
        .output()
        .unwrap();
    assert!(
        start.status.success(),
        "{}",
        String::from_utf8_lossy(&start.stderr)
    );
    let record: Value = serde_json::from_slice(&start.stdout).unwrap();
    let new_target = record["tailscale_serve_target"].as_str().unwrap();
    assert_ne!(new_target, old_target);
    assert_eq!(
        fs::read_to_string(home.path().join("mock-serve")).unwrap(),
        new_target
    );
    assert!(home.path().join("first-off-failed").exists());
    let stop = cas_command(home.path(), bin.as_os_str())
        .args(["hub", "stop"])
        .output()
        .unwrap();
    assert!(
        stop.status.success(),
        "{}",
        String::from_utf8_lossy(&stop.stderr)
    );
    assert!(!home.path().join("mock-serve").exists());
}

// false omits the explicit flag and exercises the default host policy (Serve ON).
fn start_hub(home: &Path, path: &OsStr, explicit_tailscale_flag: bool) -> Value {
    assert!(
        !home.join(".cas").exists(),
        "process proof must start with an actually absent CAS home"
    );
    let mut command = cas_command(home, path);
    command.args(["--json", "hub", "start", "--port", "0"]);
    if explicit_tailscale_flag {
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

#[cfg(target_os = "macos")]
#[test]
fn detached_hub_uses_app_bundle_override_with_no_parent_term() {
    let home = private_home();
    let app_cli = home.path().join("Applications/Tailscale.app/Contents/MacOS/Tailscale");
    fs::create_dir_all(app_cli.parent().unwrap()).unwrap();
    let script = format!(
        "#!/bin/sh\nif [ \"${{TAILSCALE_BE_CLI:-}}\" != 1 ]; then echo 'The Tailscale GUI failed to start'; exit 0; fi\n[ \"$1\" = \"--socket=$HOME/fixture.socket\" ] || exit 9\nshift\n{}",
        include_str!("fixtures/hub_update_mock_tailscale.sh").trim_start_matches("#!/bin/sh\n")
    );
    install_tailscale_mock(home.path(), &app_cli, &script);
    fs::write(home.path().join("mock-port"), "10034").unwrap();
    fs::write(home.path().join("mock-dns-name"), "app-cli.tail.example.").unwrap();
    let command = |args: &[&str]| {
        cas_command(home.path(), OsStr::new("/usr/bin:/bin"))
            .env("TAILSCALE", &app_cli)
            .env("TAILSCALE_SOCKET", home.path().join("fixture.socket"))
            .args(args)
            .output()
            .unwrap()
    };
    let start = command(&["--json", "hub", "--tailscale-serve", "--tailscale-serve-port", "10034", "start", "--port", "0"]);
    assert!(start.status.success(), "{}", String::from_utf8_lossy(&start.stderr));
    let record: Value = serde_json::from_slice(&start.stdout).unwrap();
    assert_eq!(record["public_url"], "https://app-cli.tail.example:10034/");
    assert!(home.path().join("mock-route").exists());
    let stop = command(&["hub", "stop"]);
    assert!(stop.status.success(), "{}", String::from_utf8_lossy(&stop.stderr));
}

#[cfg(unix)]
fn hub_serve_command_is_live(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains(" hub serve"))
}

#[cfg(unix)]
#[test]
fn panic_mid_fixture_reaps_detached_hub_and_group() {
    let mut pid = 0;
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let home = private_home();
        let record = start_hub(home.path(), &system_path(), false);
        pid = record["pid"].as_u64().expect("detached hub PID") as u32;
        assert!(hub_serve_command_is_live(pid));
        panic!("forced panic after detached hub startup");
    }));
    assert!(panic.is_err());
    // cas-b86b: the fixture's Drop waits for the hub to exit (a pidfd on
    // Linux, with load-tolerant ceilings), so by now it must be gone or a
    // zombie. No grace period here: a hub still serving is a real leak.
    if hub_serve_command_is_live(pid) {
        // The PID came from this fixture's successful start; prevent this
        // regression test itself from leaving a leaked hub on assertion failure.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        panic!("detached fixture hub PID {pid} survived panic cleanup");
    }
}

#[cfg(unix)]
#[test]
fn cleanup_refuses_hub_home_outside_test_temp_root() {
    struct OwnedChild(std::process::Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let outside = tempfile::tempdir_in(cas::test_paths::workspace_root()).unwrap();
    let test_root = std::env::temp_dir().canonicalize().unwrap();
    assert!(
        !outside
            .path()
            .canonicalize()
            .unwrap()
            .starts_with(&test_root)
    );
    fs::write(
        outside.path().join(".cas-test-hub-home"),
        std::process::id().to_string(),
    )
    .unwrap();
    let mut hub = OwnedChild(
        cas_process_command(outside.path(), &system_path())
            .args(["hub", "serve", "--port", "0"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let paths = cas::hub::HubRuntimePaths::new(outside.path().join(".cas/hub"));
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut ready = false;
    while Instant::now() < deadline {
        if paths
            .read_process_record()
            .is_ok_and(|record| record.pid == hub.0.id())
        {
            ready = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(ready, "outside hub never wrote its process record");
    assert!(hub_serve_command_is_live(hub.0.id()));
    hub_fixture::cleanup_detached_hub_test_home(outside.path());
    assert!(
        hub.0.try_wait().unwrap().is_none(),
        "outside hub was killed"
    );
}

#[cfg(unix)]
#[test]
fn atexit_backstop_reaps_forgotten_fixture_home() {
    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt_path = receipt_dir.path().join("forgotten-hub.json");
    let child_test = module_path!()
        .split_once("::")
        .map(|(_, module)| format!("{module}::atexit_backstop_helper"))
        .unwrap_or_else(|| "atexit_backstop_helper".to_string());
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        // One test thread makes libtest name the test before running it; the
        // helper exits before the result line, so this is its only trace.
        .args(["--exact", &child_test, "--ignored", "--test-threads=1"])
        .env("CAS_HUB_ATEXIT_RECEIPT", &receipt_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "atexit child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The helper exits before libtest's final summary so atexit can run. A
    // zero-match invocation also exits successfully but never writes a receipt.
    child_test_evidence::assert_started(&String::from_utf8_lossy(&output.stdout), &child_test);
    let receipt: Value = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    let pid = receipt["pid"].as_u64().unwrap() as u32;
    for _ in 0..50 {
        if !hub_serve_command_is_live(pid) {
            let home = receipt["home"].as_str().unwrap();
            fs::remove_dir_all(home).unwrap();
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    panic!("atexit backstop left detached hub PID {pid} alive");
}

#[cfg(unix)]
#[test]
#[ignore = "subprocess helper for atexit fixture backstop"]
fn atexit_backstop_helper() {
    let receipt_path = std::env::var_os("CAS_HUB_ATEXIT_RECEIPT")
        .expect("atexit helper must run through its parent test");
    let home = private_home();
    let record = start_hub(home.path(), &system_path(), false);
    fs::write(
        receipt_path,
        serde_json::json!({"home":home.path(), "pid":record["pid"]}).to_string(),
    )
    .unwrap();
    std::mem::forget(home);
    std::process::exit(0);
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
    let startup_log = fs::read_to_string(home.path().join(".cas/hub/hub.log"))
        .expect("hub startup must create a diagnostic log before optional probes");
    assert!(
        startup_log.contains("cas hub serve starting (tailscale_serve=true"),
        "startup log: {startup_log}"
    );
    #[cfg(unix)]
    assert_private_state_modes(home.path());

    let stop = assert_health_status_and_stop(home.path(), &path, &record);
    assert_eq!(stop["tailscale_serve_removed"], false);
    assert!(home.path().join(".cas/hub/machine-id").exists());
    assert!(home.path().join(".cas/hub/auth.json").exists());
}

#[cfg(unix)]
#[test]
fn current_hub_with_incomplete_lock_metadata_is_not_reported_ready() {
    let home = private_home();
    let path = system_path();
    let record = start_hub(home.path(), &path, false);
    fs::write(home.path().join(".cas/hub/hub.lock"), b"").unwrap();

    let status = cas_command(home.path(), &path)
        .args(["--json", "hub", "status"])
        .output()
        .expect("status with incomplete lock metadata");
    assert!(!status.status.success());
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["running"], false);
    assert_eq!(status["record"]["pid"], record["pid"]);

    let stop = cas_command(home.path(), &path)
        .args(["--json", "hub", "stop"])
        .output()
        .expect("cleanup hub with incomplete lock metadata");
    assert!(
        stop.status.success(),
        "cleanup failed: {}",
        String::from_utf8_lossy(&stop.stderr)
    );
}

#[cfg(unix)]
#[test]
fn hub_serve_does_not_hold_instance_lock_while_auth_lock_is_contended() {
    use fs2::FileExt;
    use std::os::unix::fs::PermissionsExt;

    let home = private_home();
    let path = system_path();
    let hub = home.path().join(".cas/hub");
    fs::create_dir_all(&hub).unwrap();
    fs::set_permissions(home.path().join(".cas"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&hub, fs::Permissions::from_mode(0o700)).unwrap();
    let auth_lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(hub.join("auth.lock"))
        .unwrap();
    fs::set_permissions(hub.join("auth.lock"), fs::Permissions::from_mode(0o600)).unwrap();
    auth_lock.lock_exclusive().unwrap();

    let mut serve = cas_process_command(home.path(), &path);
    serve
        .args(["hub", "serve", "--bind", "127.0.0.1", "--port", "0"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = serve.spawn().expect("spawn auth-lock contention fixture");
    thread::sleep(Duration::from_millis(300));

    let paths = cas::hub::HubRuntimePaths::new(&hub);
    let contender = paths
        .acquire_instance_lock()
        .expect("hub serve must not hold hub.lock while auth.lock blocks startup");
    drop(contender);
    assert!(!hub.join("process.json").exists());

    auth_lock.unlock().unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
}

#[cfg(unix)]
#[test]
fn clean_home_tailscale_stop_reports_removal_after_serve_exit_teardown() {
    let home = private_home();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let tailscale = bin.join("tailscale");
    install_tailscale_mock(
        home.path(),
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
    );

    let record = start_hub(home.path(), bin.as_os_str(), true);
    assert_eq!(record["public_url"], "https://clean-host.tail.example/");
    assert_eq!(record["transport_warning"], Value::Null);
    assert_private_state_modes(home.path());
    assert_eq!(
        mode(&home.path().join(".cas/hub/tailscale-serve.json")),
        0o600
    );

    let plaintext_port = record["port"].as_u64().unwrap() as u16;
    let first_backend_port = fs::read_to_string(home.path().join("mock-port"))
        .unwrap()
        .parse::<u16>()
        .unwrap();
    assert_ne!(first_backend_port, plaintext_port);

    // A prior explicit Serve publication is durable operator intent. A plain
    // restart must carry it forward even when the flag is omitted.
    let plain_restart = cas_command(home.path(), bin.as_os_str())
        .args(["hub", "restart", "--port", "0"])
        .output()
        .expect("restart trusted proxy hub without the Serve flag");
    assert!(
        plain_restart.status.success(),
        "flagless trusted-proxy restart failed: {}",
        String::from_utf8_lossy(&plain_restart.stderr)
    );
    let plain_status = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "status"])
        .output()
        .expect("status after flagless restart");
    assert!(plain_status.status.success());
    let plain_status: Value = serde_json::from_slice(&plain_status.stdout).unwrap();
    assert_eq!(plain_status["record"]["public_url"], "https://clean-host.tail.example/");
    let trusted_backend_port = fs::read_to_string(home.path().join("mock-port"))
        .unwrap()
        .parse::<u16>()
        .unwrap();
    assert_ne!(trusted_backend_port, plaintext_port);
    assert_ne!(trusted_backend_port, first_backend_port);
    assert!(
        TcpStream::connect(("127.0.0.1", first_backend_port)).is_err(),
        "the pre-restart Serve shim listener survived the flagless restart"
    );

    // A route that no longer targets the live shim is a hard diagnostic
    // failure, with the same finding surfaced by both hub status and doctor.
    fs::write(home.path().join("mock-port"), "33427\n").unwrap();
    let mismatch_status = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "status"])
        .output()
        .expect("inspect mismatched Serve route");
    assert!(!mismatch_status.status.success());
    let mismatch_status: Value = serde_json::from_slice(&mismatch_status.stdout).unwrap();
    assert_eq!(mismatch_status["tailscale_serve"]["status"], "fail");
    assert!(mismatch_status["tailscale_serve"]["message"]
        .as_str()
        .unwrap()
        .contains("does not match the live hub shim"));

    let doctor = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "doctor", "--host"])
        .output()
        .expect("doctor mismatched Serve route");
    assert!(doctor.status.success());
    let doctor: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let transport_check = doctor
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "hub transport")
        .expect("doctor hub transport row");
    assert_eq!(transport_check["status"], "error");
    assert!(transport_check["message"]
        .as_str()
        .unwrap()
        .contains("does not match the live hub shim"));
    fs::write(
        home.path().join("mock-port"),
        format!("{trusted_backend_port}\n"),
    )
    .unwrap();

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

    let mut held_proxy_client = TcpStream::connect(("127.0.0.1", trusted_backend_port)).unwrap();
    held_proxy_client
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(
        held_proxy_client,
        "POST /v1/auth/pairing/exchange HTTP/1.1\r\nHost: 127.0.0.1:{trusted_backend_port}\r\nOrigin: https://clean-host.tail.example\r\nContent-Type: application/json\r\nContent-Length: 1024\r\n\r\n{{"
    )
    .unwrap();
    held_proxy_client.flush().unwrap();
    thread::sleep(Duration::from_millis(100));

    let restart_started = Instant::now();
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
    assert!(
        restart_started.elapsed() < Duration::from_secs(8),
        "trusted-proxy restart exceeded its bounded drain window"
    );
    let mut closed = [0_u8; 1];
    assert!(
        matches!(held_proxy_client.read(&mut closed), Ok(0) | Err(_)),
        "old trusted proxy left the held client silently live"
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
fn slow_tailscale_serve_start_waits_for_bounded_publication() {
    let home = private_home();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let tailscale = bin.join("tailscale");
    install_tailscale_mock(
        home.path(),
        &tailscale,
        include_str!("fixtures/hub_mock_tailscale_slow.sh"),
    );
    fs::write(home.path().join("slow-start"), b"").unwrap();

    let started = Instant::now();
    let record = start_hub(home.path(), bin.as_os_str(), true);
    let elapsed = started.elapsed();
    fs::remove_file(home.path().join("slow-start")).unwrap();
    let startup_log = fs::read_to_string(home.path().join(".cas/hub/hub.log")).unwrap();

    let stop = assert_health_status_and_stop(home.path(), bin.as_os_str(), &record);
    assert!(
        elapsed >= Duration::from_secs(8),
        "{elapsed:?}; {startup_log}"
    );
    assert_eq!(
        record["public_url"], "https://slow.tail.example/",
        "{record}; {startup_log}"
    );
    assert_eq!(
        record["transport_warning"],
        Value::Null,
        "{record}; {startup_log}"
    );
    assert_eq!(stop["tailscale_serve_removed"], true);
    assert!(!home.path().join("mock-serve").exists());
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
    let home = private_home();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let tailscale = bin.join("tailscale");
    install_tailscale_mock(
        home.path(),
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
    );

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
    let marker_deadline = Instant::now() + CHILD_PROGRESS_TIMEOUT;
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
fn restart_force_closes_a_held_client_and_starts_the_replacement() {
    let home = private_home();
    let path = system_path();
    let initial = start_hub(home.path(), &path, false);
    let old_pid = initial["pid"].as_u64().unwrap();
    let old_pid_starttime = cas::test_paths::pid_starttime(old_pid as u32);
    let port = initial["port"].as_u64().unwrap() as u16;
    let machine_id = fs::read_to_string(home.path().join(".cas/hub/machine-id")).unwrap();

    // Keep a real accepted HTTP request active by declaring a body and withholding
    // most of it. This has the same server-lifecycle shape as an upgraded viewer:
    // graceful shutdown cannot finish until the client goes away.
    let mut held = TcpStream::connect(("127.0.0.1", port)).unwrap();
    held.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    write!(
        held,
        "POST /v1/auth/pairing/exchange HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: 1024\r\n\r\n{{"
    )
    .unwrap();
    held.flush().unwrap();
    thread::sleep(Duration::from_millis(100));

    let started = Instant::now();
    let restart = cas_command(home.path(), &path)
        .args(["--json", "hub", "restart", "--port", &port.to_string()])
        .output()
        .expect("restart hub with held client");
    let elapsed = started.elapsed();

    if !restart.status.success() {
        let stderr = String::from_utf8_lossy(&restart.stderr).into_owned();
        drop(held);
        let _ = cas_command(home.path(), &path)
            .args(["--json", "hub", "stop"])
            .output();
        panic!("held client blocked restart for {elapsed:?}: {stderr}");
    }
    assert!(
        elapsed < Duration::from_secs(8),
        "restart exceeded its bounded drain window: {elapsed:?}"
    );

    let mut closed = [0_u8; 1];
    assert!(
        matches!(held.read(&mut closed), Ok(0) | Err(_)),
        "old hub left the held connection silently live"
    );
    let status = cas_command(home.path(), &path)
        .args(["--json", "hub", "status"])
        .output()
        .unwrap();
    assert!(status.status.success());
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    let old_process_gone = match old_pid_starttime {
        Some(expected_starttime) => cas::test_paths::pid_starttime(old_pid as u32)
            .is_none_or(|actual_starttime| actual_starttime != expected_starttime),
        None => nix::sys::signal::kill(nix::unistd::Pid::from_raw(old_pid as i32), None).is_err(),
    };
    assert!(
        old_process_gone,
        "restart left the original hub process live (pid={old_pid})"
    );
    assert_eq!(status["record"]["port"].as_u64().unwrap(), u64::from(port));
    assert_eq!(
        fs::read_to_string(home.path().join(".cas/hub/machine-id")).unwrap(),
        machine_id,
        "restart must preserve the stable machine identity"
    );

    let stop = cas_command(home.path(), &path)
        .args(["--json", "hub", "stop"])
        .output()
        .unwrap();
    assert!(stop.status.success());
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
    let marker_deadline = Instant::now() + CHILD_PROGRESS_TIMEOUT;
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
    let release_deadline = Instant::now() + CHILD_PROGRESS_TIMEOUT;
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
fn restart_force_terminates_a_recordless_lock_holder_and_starts_replacement() {
    use cas::hub::HubRuntimePaths;

    let home = private_home();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let barrier = home.path().join("restart-lock-force");
    fs::create_dir(&barrier).unwrap();

    let initial = cas_command(home.path(), bin.as_os_str())
        .env("CAS_TEST_HUB_LOCK_RELEASE_BARRIER", &barrier)
        .args(["--json", "hub", "start", "--port", "0"])
        .output()
        .expect("start force-recovery fixture");
    assert!(initial.status.success());
    let initial: Value = serde_json::from_slice(&initial.stdout).unwrap();

    // --force may terminate the running holder before its SIGTERM handler
    // removes the record. Establish the recordless stopping state this test
    // exercises before asking the restart command to recover it.
    let old_pid = initial["pid"].as_u64().unwrap() as u32;
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(old_pid as i32),
        nix::sys::signal::Signal::SIGTERM,
    )
    .expect("signal the old hub into its stopping barrier");
    let marker = barrier.join("record-removed-lock-held");
    let marker_deadline = Instant::now() + CHILD_PROGRESS_TIMEOUT;
    while !marker.exists() && Instant::now() < marker_deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists(), "old hub did not reach its stopping barrier");
    assert!(
        !home.path().join(".cas/hub/process.json").exists(),
        "force recovery must start from the recordless state"
    );
    let paths = HubRuntimePaths::new(home.path().join(".cas/hub"));
    assert!(
        paths.acquire_instance_lock().is_err(),
        "old hub must still hold its machine lock"
    );
    let owner = paths.read_lock_owner().unwrap();
    assert_eq!(owner.pid, old_pid);
    assert_eq!(owner.phase, "stopping");

    let restart = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "restart", "--force", "--port", "0"])
        .output()
        .expect("force-restart recordless holder");
    assert!(
        restart.status.success(),
        "force restart failed: stdout={} stderr={}",
        String::from_utf8_lossy(&restart.stdout),
        String::from_utf8_lossy(&restart.stderr)
    );
    let stderr = String::from_utf8_lossy(&restart.stderr);
    assert!(
        stderr.contains(&format!("cas hub pid {} is stopping;", initial["pid"])),
        "force recovery must identify the recordless stopping holder: {stderr}"
    );
    assert!(stderr.contains("terminating lock holder"), "{stderr}");

    let status = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "status"])
        .output()
        .expect("status after force recovery");
    assert!(status.status.success());
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["running"], true);
    assert_ne!(status["record"]["pid"], initial["pid"]);
    assert!(home.path().join(".cas/hub/process.json").exists());

    let stop = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "stop"])
        .output()
        .expect("stop force-recovery replacement");
    assert!(
        stop.status.success(),
        "replacement stop failed: {}",
        String::from_utf8_lossy(&stop.stderr)
    );
}

/// cas-bf90 guard on the shared stop path.
///
/// `hub restart` and a flags-differing `hub start` now pass a relaunch intent,
/// which lets their wait resolve as success when a concurrent command already
/// produced a satisfying live hub. Plain `hub stop` passes no intent, because
/// for it "a hub is alive" is the opposite of success — reporting success while
/// leaving a hub running would be a far worse bug than the stall being fixed.
/// This pins that separation: with the lock held past the deadline, plain stop
/// must still fail, and must say why.
#[cfg(unix)]
#[test]
fn plain_stop_never_reports_success_while_a_hub_still_holds_the_lock() {
    use cas::hub::HubRuntimePaths;

    let home = private_home();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let barrier = home.path().join("stop-lock-barrier");
    fs::create_dir(&barrier).unwrap();

    let initial = cas_command(home.path(), bin.as_os_str())
        .env("CAS_TEST_HUB_LOCK_RELEASE_BARRIER", &barrier)
        .args(["--json", "hub", "start", "--port", "0"])
        .output()
        .expect("start stop-barrier fixture");
    assert!(initial.status.success());

    // The hub removes its record and then holds the machine lock, so the stop
    // wait cannot observe a quiescent machine before its deadline.
    let stop = cas_command(home.path(), bin.as_os_str())
        .env("CAS_TEST_HUB_LOCK_RELEASE_BARRIER", &barrier)
        .args(["--json", "hub", "stop"])
        .output()
        .expect("run plain stop");
    let stderr = String::from_utf8_lossy(&stop.stderr);
    assert!(
        !stop.status.success(),
        "plain `hub stop` claimed success while a hub still held the machine lock — \
         the relaunch-intent early return has leaked into stop: stdout={} stderr={stderr}",
        String::from_utf8_lossy(&stop.stdout)
    );
    assert!(
        stderr.contains("remained live after 10.0s") || stderr.contains("remained held after 10.0s"),
        "plain stop must name why it could not finish: {stderr}"
    );

    fs::write(barrier.join("release"), b"release\n").unwrap();
    let paths = HubRuntimePaths::new(home.path().join(".cas/hub"));
    let release_deadline = Instant::now() + CHILD_PROGRESS_TIMEOUT;
    while paths.acquire_instance_lock().is_err() && Instant::now() < release_deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let _ = cas_command(home.path(), bin.as_os_str())
        .args(["--json", "hub", "stop"])
        .output();
}

#[cfg(unix)]
#[test]
fn concurrent_start_and_restart_leave_exactly_one_lock_owner() {
    use cas::hub::HubRuntimePaths;

    // cas-036b: a stop can acquire the machine lock between the launcher
    // releasing it and the new foreground hub acquiring it. Force a real
    // failed first acquisition, then either finish cleanup or hold it through
    // the bounded startup wait so the caller receives the specific failure.
    for cleanup_finishes in [true, false] {
        let home = private_home();
        let bin = home.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let barrier = home.path().join("startup-lock-barrier");
        fs::create_dir(&barrier).unwrap();
        let paths = HubRuntimePaths::new(home.path().join(".cas/hub"));
        let mut command = cas_process_command(home.path(), bin.as_os_str());
        let mut start = command
            .env("CAS_TEST_HUB_LAUNCH_BARRIER", &barrier)
            .env("CAS_TEST_HUB_STARTUP_LOCK_BARRIER", &barrier)
            .args([
                "--json", "hub", "--no-tailscale-serve", "start", "--port", "0",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start hub at lock handoff barrier");
        let deadline = Instant::now() + CHILD_PROGRESS_TIMEOUT;
        let launcher = loop {
            let launcher = fs::read_dir(&barrier).unwrap().filter_map(Result::ok)
                .find_map(|entry| entry.file_name().to_string_lossy()
                    .strip_prefix("launcher-").and_then(|pid| pid.parse::<u32>().ok()));
            if launcher.is_some() || Instant::now() >= deadline {
                break launcher;
            }
            thread::sleep(Duration::from_millis(10));
        };
        if launcher.is_none() {
            let _ = fs::write(barrier.join("release-launchers"), b"release\n");
            let _ = fs::write(barrier.join("release-monitors"), b"release\n");
            let _ = fs::write(barrier.join("release-startup-lock"), b"release\n");
            let output = start.wait_with_output().unwrap();
            panic!("launcher did not reach lock handoff: {output:?}");
        }
        let launcher = launcher.unwrap();
        let mut cleanup_lock = paths.acquire_instance_lock().unwrap();
        cleanup_lock.set_phase("stopping").unwrap();
        let unrelated_error = paths.root().join(".launcher-unrelated.error");
        fs::write(&unrelated_error, b"another launch's diagnostic").unwrap();
        fs::write(barrier.join("release-launchers"), b"release\n").unwrap();
        let contention = barrier.join(format!("startup-lock-waiter-{launcher}"));
        let deadline = Instant::now() + CHILD_PROGRESS_TIMEOUT;
        while !contention.exists() && start.try_wait().unwrap().is_none()
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        let cleanup_lock = Some(cleanup_lock);
        let held_lock = if cleanup_finishes {
            drop(cleanup_lock);
            None
        } else {
            cleanup_lock
        };
        fs::write(barrier.join("release-startup-lock"), b"release\n").unwrap();
        fs::write(barrier.join("release-monitors"), b"release\n").unwrap();
        let output = start.wait_with_output().unwrap();
        assert!(contention.exists(), "hub did not attempt the contended lock: {output:?}");
        let errors = fs::read_dir(paths.root()).unwrap().filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".error"))
            .map(|entry| entry.path()).collect::<Vec<_>>();
        assert_eq!(errors, vec![unrelated_error.clone()], "launch leaked its diagnostic file");
        assert_eq!(fs::read(&unrelated_error).unwrap(), b"another launch's diagnostic");
        if !cleanup_finishes {
            assert!(!output.status.success(), "hub ignored the held machine lock: {output:?}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("machine lock remained held"),
                "startup timeout lost its specific diagnostic: {stderr}");
            assert!(!paths.root().join("process.json").exists(), "timed-out hub published a record");
            drop(held_lock);
            continue;
        }
        assert!(output.status.success(), "hub abandoned a transient cleanup lock: {output:?}");
        let started: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(started["pid"], launcher);
        let status = cas_command(home.path(), bin.as_os_str())
            .args(["--json", "hub", "status"]).output().unwrap();
        let running: Value = serde_json::from_slice(&status.stdout).unwrap();
        assert_eq!(running["running"], true, "{running}");
        assert_eq!(running["record"]["pid"], launcher, "{running}");
        let stop = cas_command(home.path(), bin.as_os_str())
            .args(["--json", "hub", "stop"]).output().unwrap();
        assert!(stop.status.success());
    }

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
                // The two CLI commands can launch detached hubs while the
                // other command still owns a captured pipe. File-backed
                // output does not wait for an unrelated descendant to close
                // an inherited pipe after its CLI parent has exited.
                let stdout = home.join(format!("concurrent-{action}.stdout"));
                let stderr = home.join(format!("concurrent-{action}.stderr"));
                command
                    .stdout(Stdio::from(fs::File::create(&stdout).unwrap()))
                    .stderr(Stdio::from(fs::File::create(&stderr).unwrap()));
                gate.wait();
                let status = command.status().unwrap();
                std::process::Output {
                    status,
                    stdout: fs::read(stdout).unwrap(),
                    stderr: fs::read(stderr).unwrap(),
                }
            })
        };
        let start = run("start", gate.clone());
        let restart = run("restart", gate.clone());
        gate.wait();
        let outputs = [start.join().unwrap(), restart.join().unwrap()];
        let labelled = [("start", &outputs[0]), ("restart", &outputs[1])];
        let stderrs = || {
            labelled
                .iter()
                .map(|(name, output)| {
                    format!("{name}: {}", String::from_utf8_lossy(&output.stderr).trim())
                })
                .collect::<Vec<_>>()
                .join(" | ")
        };

        // cas-bf90. The old assertion was `any(success)`, which passed whenever
        // either command happened to win. That hid the real behaviour: the
        // losing command failed in ~80% of concurrent iterations, always after
        // stalling the full 10s machine-lock timeout, because it waited for a
        // lock the winner's healthy hub legitimately holds. It also meant the
        // gate's one observed both-failed run was the *only* signal this test
        // could ever give.
        //
        // Now every command must either succeed or fail with a named reason —
        // never a bare non-zero exit, and never a lock-wait timeout, which is
        // the specific defect that was fixed.
        for (name, output) in labelled {
            if output.status.success() {
                continue;
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                !stderr.contains("machine lock remained held"),
                "`hub {name}` timed out waiting for a lock a healthy hub already holds — \
                 the cas-bf90 defect has regressed: {}",
                stderrs()
            );
            assert!(
                !stderr.trim().is_empty(),
                "`hub {name}` failed without naming a reason: {}",
                stderrs()
            );
        }

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

    // The two `hub start`/`hub restart` races above cover competing launchers.
    // Also pin two restarts that both began stopping the same generation:
    // hold the old hub after it removes its record, wait until both restart
    // processes have entered the stop waiter, then release the old lock.
    // Worker mode gives each detached hub its shared-server cgroup, so stale
    // cleanup must not kill the replacement that joined a later generation.
    let race_home = private_home();
    let barrier = race_home.path().join("concurrent-restart-barrier");
    fs::create_dir(&barrier).unwrap();
    let session = format!("cas-3dc8-test-{}", std::process::id());
    let initial = cas_command(race_home.path(), bin.as_os_str())
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
        .expect("start worker hub for concurrent restart race");
    assert!(
        initial.status.success(),
        "worker hub start failed: {}",
        String::from_utf8_lossy(&initial.stderr)
    );
    let initial: Value = serde_json::from_slice(&initial.stdout).unwrap();

    let spawn_restart = |name: &str| {
        let stdout_path = race_home.path().join(format!("{name}.stdout"));
        let stderr_path = race_home.path().join(format!("{name}.stderr"));
        let mut command = cas_process_command(race_home.path(), bin.as_os_str());
        command
            .env("CAS_AGENT_ROLE", "worker")
            .env("CAS_FACTORY_SESSION", &session)
            .env("CAS_TEST_HUB_LOCK_RELEASE_BARRIER", &barrier)
            .args([
                "--json",
                "hub",
                "--no-tailscale-serve",
                "restart",
                "--port",
                "0",
            ])
            .stdout(Stdio::from(fs::File::create(&stdout_path).unwrap()))
            .stderr(Stdio::from(fs::File::create(&stderr_path).unwrap()));
        let child = command.spawn().expect("spawn concurrent restart");
        (child, stdout_path, stderr_path)
    };
    let (mut first, first_stdout, first_stderr) = spawn_restart("restart-a");
    let old_hub_waiting = barrier.join("record-removed-lock-held");
    let deadline = Instant::now() + CHILD_PROGRESS_TIMEOUT;
    while !old_hub_waiting.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if !old_hub_waiting.exists() {
        let _ = fs::write(barrier.join("release"), b"release\n");
        let _ = first.wait_with_output();
        panic!("old hub did not reach the deterministic lock-release barrier");
    }
    let (mut second, second_stdout, second_stderr) = spawn_restart("restart-b");
    let waiter_deadline = Instant::now() + CHILD_PROGRESS_TIMEOUT;
    loop {
        let waiters = fs::read_dir(&barrier)
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
        thread::sleep(Duration::from_millis(10));
    }
    let waiter_count = fs::read_dir(&barrier)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("restart-stop-waiter-")
        })
        .count();
    let _ = fs::write(barrier.join("release"), b"release\n");
    let first_status = first.wait().expect("wait first concurrent restart");
    let second_status = second.wait().expect("wait second concurrent restart");
    let first = std::process::Output {
        status: first_status,
        stdout: fs::read(first_stdout).unwrap(),
        stderr: fs::read(first_stderr).unwrap(),
    };
    let second = std::process::Output {
        status: second_status,
        stdout: fs::read(second_stdout).unwrap(),
        stderr: fs::read(second_stderr).unwrap(),
    };

    let status = cas_command(race_home.path(), bin.as_os_str())
        .args(["--json", "hub", "status"])
        .output()
        .expect("read hub after concurrent restarts");
    let running = serde_json::from_slice::<Value>(&status.stdout).ok();
    let running_pid = running
        .as_ref()
        .and_then(|status| status["record"]["pid"].as_u64());
    let cleanup = cas_command(race_home.path(), bin.as_os_str())
        .env("CAS_AGENT_ROLE", "worker")
        .env("CAS_FACTORY_SESSION", &session)
        .args(["--json", "hub", "stop"])
        .output()
        .expect("stop concurrent restart fixture");

    let labelled = [("restart-a", &first), ("restart-b", &second)];
    let detail = labelled
        .iter()
        .map(|(name, output)| {
            format!(
                "{name}: exit={}, stdout={}, stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stdout).trim(),
                String::from_utf8_lossy(&output.stderr).trim()
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");
    assert_eq!(waiter_count, 2, "both restarts must reach the stop wait: {detail}");
    assert!(status.status.success(), "hub status failed: {detail}");
    assert_eq!(running.as_ref().unwrap()["running"], true, "{detail}");
    let running_pid = running_pid.expect("running hub PID");
    assert_ne!(running_pid, initial["pid"].as_u64().unwrap(), "{detail}");
    assert!(
        cleanup.status.success(),
        "fixture cleanup failed: {}",
        String::from_utf8_lossy(&cleanup.stderr)
    );

    let mut launched_pids = Vec::new();
    let mut satisfied_pids = Vec::new();
    for (name, output) in labelled {
        let value: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("{name} did not emit one JSON value ({error}): {detail}"));
        if output.status.success() {
            let pid = value["start"]["pid"]
                .as_u64()
                .unwrap_or_else(|| panic!("{name} success has no hub PID: {value}"));
            match value["start"]["already_satisfied"].as_bool() {
                Some(false) => launched_pids.push(pid),
                Some(true) => satisfied_pids.push(pid),
                None => panic!("{name} success has no start disposition: {value}; {detail}"),
            }
            if value["start"]["already_satisfied"] == true
                || value["stop"]["already_satisfied"] == true
            {
                assert_eq!(
                    pid, running_pid,
                    "{name} already-satisfied stop must return the running hub: {value}; {detail}"
                );
            }
            assert_eq!(pid, running_pid, "{name} must report the running hub: {value}; {detail}");
        } else {
            let message = value["message"].as_str().unwrap_or_default();
            assert!(
                message.contains("another cas hub instance won the machine lock")
                    && message.contains(&format!("pid {running_pid}")),
                "{name} reported an error inconsistent with the running winner: {value}; {detail}"
            );
        }
    }
    assert_eq!(
        launched_pids,
        vec![running_pid],
        "exactly one restart may report that it launched the running hub: {detail}"
    );
    assert!(
        satisfied_pids.iter().all(|pid| *pid == running_pid),
        "a peer-satisfied restart must identify the running hub: {detail}"
    );
    if let Some(old_cgroup) = initial["cgroup"].as_str() {
        assert_ne!(
            running.as_ref().unwrap()["record"]["cgroup"].as_str(),
            Some(old_cgroup),
            "a replacement must not share a cgroup with the generation its stop cleans up"
        );
    }
}

#[path = "../../crates/cas-core/src/test_child.rs"]
mod child_test_evidence;

#[cfg(target_os = "linux")]
#[test]
fn hub_status_bounds_never_finishing_manager_and_reports_unknown_cas_8ee8() {
    let home = private_home();
    let manager = home.path().join("slow-systemctl");
    let pid_log = home.path().join("manager-pids");
    cas::test_paths::warm_stub(
        &manager,
        "#!/bin/sh\nprintf '%s\\n' \"$$\" >> \"$CAS_MANAGER_PID_LOG\"\nexec /usr/bin/tail -f /dev/null\n",
    );
    // The manager's answer matters only for an installed unit: with none,
    // status rightly ignores the manager (cas-6033), so this fixture installs
    // one for the hung diagnostic to block on.
    let unit = home.path().join(".config/systemd/user/cas-hub.service");
    fs::create_dir_all(unit.parent().unwrap()).unwrap();
    fs::write(&unit, "[Service]\nExecStart=/nonexistent/cas hub serve --launched-by service\n").unwrap();
    let started = Instant::now();
    let output = cas_command(home.path(), &system_path())
        .env("CAS_HUB_SYSTEMCTL", &manager)
        .env("CAS_MANAGER_PID_LOG", &pid_log)
        .args(["hub", "status", "--json"])
        .timeout(Duration::from_secs(4))
        .output();
    let elapsed = started.elapsed();
    // Reap only this fixture's diagnostic children if the old unbounded path
    // outlives the CLI's independent assertion timeout. No live manager used.
    if let Ok(pids) = fs::read_to_string(&pid_log) {
        for pid in pids.lines().filter_map(|line| line.parse::<i32>().ok()) {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
    let output = output.expect("run bounded status");
    assert!(
        elapsed < Duration::from_secs(3),
        "manager pinned status: {elapsed:?}"
    );
    let status: Value = serde_json::from_slice(&output.stdout).expect("one status document");
    assert_eq!(status["service_status"], "unknown", "{status}");
    assert!(
        status["service_warning"]
            .as_str()
            .unwrap()
            .contains("timed out")
    );
    assert_eq!(
        status["runtime_receipt"]["boot_prerequisites"]["service_active"],
        Value::Null
    );
    assert_eq!(status["runtime_receipt"]["hub"]["state"], "unknown");
    assert!(!home.path().join(".cas/hub").exists());
}

#[cfg(unix)]
#[test]
fn hub_service_status_readiness_fits_80_columns_cas_0252() {
    let home = private_home();
    let bin = home.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    for cli in ["claude", "codex"] {
        cas::test_paths::warm_stub(&bin.join(cli), "#!/bin/sh\nexit 0\n");
    }
    let systemctl = bin.join("systemctl");
    cas::test_paths::warm_stub(
        &systemctl,
        "#!/bin/sh\n[ \"$2\" = \"--version\" ] && exit 0\nexit 1\n",
    );

    let path = bin.as_os_str();
    let mut human_command = cas_command(home.path(), path);
    #[cfg(target_os = "linux")]
    human_command.env("CAS_HUB_SYSTEMCTL", &systemctl);
    let output = human_command
        .args(["hub", "service", "status"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let human = String::from_utf8(output.stdout).unwrap();
    assert!(
        human.lines().all(|line| line.chars().count() <= 80),
        "service status exceeded 80 columns:\n{human}"
    );
    // pin: service status puts the first copyable repair command under its verdict.
    assert_eq!(
        human.lines().nth(1),
        Some("  Fix: cas claude login main"),
        "the first repair command should follow the service summary"
    );
    assert!(human.contains("path: ~/.claude"), "{human}");
    assert!(human.contains("Fix: cas claude login main"), "{human}");
    assert!(human.contains("path: ~/.codex"), "{human}");
    assert!(human.contains("Fix: cas codex login main"), "{human}");
    assert!(!human.contains("Fix: cas hub service install"), "{human}");
    assert_eq!(
        human.matches("Retry: cas hub status").count(),
        usize::from(human.contains("service manager unknown")),
        "{human}"
    );

    let mut json_command = cas_command(home.path(), path);
    #[cfg(target_os = "linux")]
    json_command.env("CAS_HUB_SYSTEMCTL", &systemctl);
    let json = json_command
        .args(["hub", "service", "status", "--json"])
        .output()
        .unwrap();
    assert!(json.status.success());
    let report: Value = serde_json::from_slice(&json.stdout).unwrap();
    let readiness = report["launch_readiness"].as_array().unwrap();
    for (cli, profile) in [("claude", ".claude"), ("codex", ".codex")] {
        let detail = readiness
            .iter()
            .find(|row| row["cli"] == cli)
            .unwrap()["detail"]
            .as_str()
            .unwrap();
        assert!(detail.contains(&home.path().join(profile).display().to_string()));
        assert!(!home.path().join(profile).exists(), "readiness must not create {profile}");
    }
    assert_eq!(
        readiness
            .iter()
            .find(|row| row["cli"] == "grok")
            .unwrap()["detail"],
        "grok CLI is not installed or executable"
    );

    fs::create_dir_all(home.path().join(".cas")).unwrap();
    fs::write(home.path().join(".cas/config.toml"), "[hub\n").unwrap();
    let mut failed_probe = cas_command(home.path(), path);
    #[cfg(target_os = "linux")]
    failed_probe.env("CAS_HUB_SYSTEMCTL", &systemctl);
    let failed_probe = failed_probe.args(["hub", "service", "status"]).output().unwrap();
    assert!(failed_probe.status.success());
    let failed_probe = String::from_utf8(failed_probe.stdout).unwrap();
    assert!(failed_probe.contains("launch issue"), "{failed_probe}");
    assert!(!failed_probe.contains("Launch readiness: healthy"), "{failed_probe}");
    assert!(failed_probe.contains("Cause:"), "{failed_probe}");
    assert!(failed_probe.contains("--json"), "{failed_probe}");
    assert!(failed_probe.lines().all(|line| line.chars().count() <= 80), "{failed_probe}");
    #[cfg(target_os = "linux")]
    {
        let manual = cas_command(home.path(), path)
            .env("CAS_HUB_SYSTEMCTL", home.path().join("missing-systemctl"))
            .args(["hub", "service", "status"])
            .output()
            .unwrap();
        assert!(manual.status.success());
        let manual = String::from_utf8(manual.stdout).unwrap();
        assert!(manual.contains("rc script after networking and Tailscale"), "{manual}");
        assert!(manual.contains("cas hub start --tailscale-serve"), "{manual}");
        assert!(manual.contains("cas hub status"), "{manual}");
        assert!(manual.lines().all(|line| line.chars().count() <= 80), "{manual}");
    }
    assert!(
        human.contains("Fix: curl -fsSL https://x.ai/cli/install.sh | bash"),
        "{human}"
    );
}

#[cfg(unix)]
#[test]
fn hub_status_prefers_live_lock_holder_over_dead_record_cas_ebbb() {
    let home = private_home();
    let paths = cas::hub::HubRuntimePaths::for_home(home.path());
    let mut lock = paths.acquire_instance_lock().unwrap();
    let mut exited = ProcessCommand::new("/usr/bin/true").spawn().unwrap();
    let dead_pid = exited.id();
    assert!(exited.wait().unwrap().success());
    let record = serde_json::from_value(serde_json::json!({
        "pid": dead_pid, "bind": "127.0.0.1", "port": 0,
        "version": "old-record", "started_at": "2020-01-01T00:00:00Z",
    })).unwrap();
    paths.write_process_record(&record).unwrap();

    for (phase, kinds) in [
        ("starting", &["starting", "startup_wedged"][..]),
        ("stopping", &["stopping"][..]),
        ("running", &["unresponsive"][..]),
    ] {
        lock.set_phase(phase).unwrap();
        let output = cas_command(home.path(), &system_path())
            .args(["hub", "status", "--json"])
            .output().unwrap();
        assert!(!output.status.success(), "a lock holder alone is not ready");
        let json: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(kinds.contains(&json["state"]["kind"].as_str().unwrap()), "{json}");
        assert_eq!(json["state"]["pid"], std::process::id(), "{json}");
        assert_eq!(json["record"]["pid"], dead_pid, "keep the historical record");
        assert_eq!(json["running"], false);
        assert!(!json["state"]["remedy"].as_str().unwrap().contains("`cas hub start`"));
        let output = cas_command(home.path(), &system_path())
            .args(["hub", "status"])
            .output().unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.starts_with(&format!("Cassy hub: pid {} is ", std::process::id())), "{text}");
        assert!(!text.contains(&format!("last pid {dead_pid} exited")), "{text}");
        assert!(!text.contains("`cas hub start`"), "{text}");
    }

    drop(lock);
    // The test process is still alive; remove its released lock metadata.
    fs::remove_file(paths.lock_path()).unwrap();
    let output = cas_command(home.path(), &system_path())
        .args(["hub", "status", "--json"])
        .output().unwrap();
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["state"]["kind"], "exited");
    assert_eq!(json["state"]["pid"], dead_pid);
    assert_eq!(json["state"]["remedy"], "Run `cas hub start`.");
}
