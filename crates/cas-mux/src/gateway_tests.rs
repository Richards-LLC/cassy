use super::*;
use crate::{Pane, SupervisorCli};
use std::io::{Read, Write};

const KEY: &str = "fixture-client-key-never-in-argv";

fn build(cli: SupervisorCli, cas_root: &Path, account: &Path) -> PtyConfig {
    let mut config = Pane::build_worker_config(
        "gw-probe",
        cas_root.parent().unwrap().to_path_buf(),
        Some(&cas_root.to_path_buf()),
        "supervisor",
        SupervisorCli::Claude,
        cli,
        None,
        None,
        Some(account.to_str().unwrap()),
        Some("explicit"),
        None,
        None,
        None,
    );
    // Each build mints a fresh session id; everything else must match.
    let session = config.env.iter().find(|(k, _)| k == "CAS_SESSION_ID").map(|(_, v)| v.clone());
    if let Some(session) = session {
        for value in config.env.iter_mut().map(|(_, v)| v).chain(config.args.iter_mut()) {
            *value = value.replace(&session, "SESSION");
        }
    }
    config
}

fn snapshot(config: &PtyConfig) -> (Vec<(String, String)>, Vec<String>, Vec<String>) {
    (config.env.clone(), config.args.clone(), config.env_remove.clone())
}

fn without_transport(config: &PtyConfig) -> (Vec<(String, String)>, Vec<String>, Vec<String>) {
    let mut config = config.clone();
    config.env.retain(|(k, _)| k != TRANSPORT_ENV);
    let forwarded = format!("mcp_servers.cs.env.{TRANSPORT_ENV}=");
    if let Some(index) = config.args.iter().position(|arg| arg.starts_with(&forwarded)) {
        config.args.drain(index - 1..=index);
    }
    (config.env, config.args, config.env_remove)
}

fn transport(config: &PtyConfig) -> Option<&str> {
    config.env.iter().rev().find_map(|(k, v)| (k == TRANSPORT_ENV).then_some(v.as_str()))
}

fn write_config(cas_root: &Path, gateway: &str) {
    std::fs::write(cas_root.join("config.toml"), format!("[factory.gateway]\n{gateway}")).unwrap();
}

/// A loopback hub that answers its model listing 200 only for `KEY`.
fn hub() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut request = Vec::new();
            let mut buf = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
            let ok = request.starts_with("get /v1/models ")
                && request.contains(&format!("authorization: bearer {}", KEY.to_ascii_lowercase()));
            let status = if ok { "200 OK" } else { "401 Unauthorized" };
            let _ = stream.write_all(
                format!("HTTP/1.1 {status}\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{{\"data\":[]}}").as_bytes(),
            );
        }
    });
    endpoint
}

fn key_file(dir: &Path, name: &str, key: &str, mode: u32) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("{key}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    path
}

#[test]
fn absent_or_disabled_gateway_config_leaves_worker_launch_byte_identical() {
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().join(".cas");
    let account = temp.path().join("account");
    std::fs::create_dir_all(&cas_root).unwrap();
    std::fs::create_dir_all(&account).unwrap();
    let key = key_file(temp.path(), "client.key", KEY, 0o600);
    for cli in [SupervisorCli::Claude, SupervisorCli::Codex] {
        let _ = std::fs::remove_file(cas_root.join("config.toml"));
        let absent = snapshot(&build(cli, &cas_root, &account));
        write_config(
            &cas_root,
            &format!("enabled = false\nendpoint = \"{}\"\nkey_file = \"{}\"\n", hub(), key.display()),
        );
        let disabled = snapshot(&build(cli, &cas_root, &account));
        assert_eq!(absent, disabled, "{cli:?}");
        assert!(!disabled.0.iter().any(|(k, _)| k == TRANSPORT_ENV || k == GATEWAY_KEY_FILE_ENV), "{cli:?}");
    }
}

#[test]
fn ready_hub_routes_the_worker_and_failed_readiness_starts_direct_on_the_same_account() {
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().join(".cas");
    let account = temp.path().join("account");
    std::fs::create_dir_all(&cas_root).unwrap();
    std::fs::create_dir_all(&account).unwrap();
    let endpoint = hub();
    let good = key_file(temp.path(), "client.key", KEY, 0o600);
    let wrong = key_file(temp.path(), "wrong.key", "not-the-key", 0o600);
    let shared = key_file(temp.path(), "shared.key", KEY, 0o644);
    for cli in [SupervisorCli::Claude, SupervisorCli::Codex] {
        let _ = std::fs::remove_file(cas_root.join("config.toml"));
        let direct = snapshot(&build(cli, &cas_root, &account));

        write_config(&cas_root, &format!("enabled = true\nendpoint = \"{endpoint}/\"\nkey_file = \"{}\"\n", good.display()));
        let routed = build(cli, &cas_root, &account);
        assert_eq!(transport(&routed), Some("gateway"), "{cli:?}");
        // The recorded route (worker_transport, shown by worker_status) and the Codex MCP
        // forward never carry the hub's host or port; only the request config does.
        let address = endpoint.trim_start_matches("http://");
        let forwarded = routed.args.iter().find(|arg| arg.contains(&format!("mcp_servers.cs.env.{TRANSPORT_ENV}=")));
        assert!(transport(&routed).into_iter().chain(forwarded.map(String::as_str)).all(|value| !value.contains(address)), "{cli:?}");
        assert!(
            !routed.env.iter().any(|(_, v)| v.contains(KEY)) && !routed.args.iter().any(|a| a.contains(KEY)),
            "{cli:?}: the key must stay in its file until Pty::spawn"
        );
        let env = |key: &str| routed.env.iter().rev().find_map(|(k, v)| (k == key).then_some(v.as_str()));
        assert_eq!(env(GATEWAY_KEY_FILE_ENV), Some(good.to_str().unwrap()));
        // The native account still supplies hooks, MCP and sessions.
        for entry in &direct.0 {
            assert!(routed.env.contains(entry), "{cli:?} lost {entry:?}");
        }
        if cli == SupervisorCli::Codex {
            assert!(routed.args.contains(&"model_provider=\"cas_gateway\"".to_string()));
            let provider = routed.args.iter().find_map(|a| a.strip_prefix("model_providers.cas_gateway=")).unwrap();
            let provider: toml::Value = toml::from_str(&format!("p = {provider}")).unwrap();
            assert_eq!(provider["p"]["base_url"].as_str(), Some(format!("{endpoint}/v1").as_str()));
            assert_eq!(provider["p"]["env_key"].as_str(), Some(CODEX_GATEWAY_KEY_ENV));
            assert_eq!(env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE"), Some("cassy/gw-probe"));
        } else {
            assert_eq!(env("ANTHROPIC_BASE_URL"), Some(endpoint.as_str()));
            assert_eq!(env("CLAUDE_AGENT_SDK_CLIENT_APP"), Some("cassy/gw-probe"));
        }

        for (gateway, status) in [
            (format!("endpoint = \"{endpoint}\"\nkey_file = \"{}\"\n", wrong.display()), Some("401")),
            (format!("endpoint = \"{endpoint}\"\nkey_file = \"{}\"\n", shared.display()), None),
            (format!("endpoint = \"http://127.0.0.1:9\"\nkey_file = \"{}\"\n", good.display()), None),
        ] {
            write_config(&cas_root, &format!("enabled = true\n{gateway}"));
            let fallback = build(cli, &cas_root, &account);
            let shown = transport(&fallback).unwrap_or_default();
            assert!(shown.starts_with("direct"), "{cli:?}: {shown}");
            if let Some(status) = status {
                assert!(shown.contains(status), "{cli:?}: {shown}");
            }
            assert_eq!(without_transport(&fallback), direct, "{cli:?}: fallback must keep the native launch");
        }
    }
}

#[test]
fn only_loopback_and_tailnet_endpoints_are_accepted() {
    for accepted in [
        "http://127.0.0.1:8317",
        "http://localhost:8317/",
        "http://[::1]:8317",
        "http://100.66.32.70:8317",
        "https://soundwave-linux.tailf5a734.ts.net:8317",
    ] {
        assert!(validate_endpoint(accepted).is_ok(), "{accepted}");
    }
    for refused in [
        "",
        "https://api.anthropic.com",
        "http://192.168.1.5:8317",
        "http://100.128.0.1:8317",
        "https://evil.example/x.ts.net",
        "https://ts.net.evil.example",
        "http://user:pass@127.0.0.1:8317",
        "ftp://127.0.0.1:8317",
    ] {
        assert!(validate_endpoint(refused).is_err(), "{refused}");
    }
}

/// Commander pins a session's route on its daemon, so the project setting
/// read at each spawn can neither turn a direct session on nor a gateway one off.
#[test]
fn session_pin_overrides_the_project_setting_in_both_directions() {
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().join(".cas");
    std::fs::create_dir_all(&cas_root).unwrap();
    let hub = "endpoint = \"http://127.0.0.1:8317\"\nkey_file = \"/k\"\n";
    for (project, pin, routed) in [
        (true, Some("direct"), false),
        (false, Some("gateway"), true),
        (true, None, true),
        (false, None, false),
    ] {
        write_config(&cas_root, &format!("enabled = {project}\n{hub}"));
        let settings = load(Some(&cas_root), pin);
        assert_eq!(settings.enabled, routed, "project={project} pin={pin:?}");
        assert_eq!(settings.endpoint.is_some(), routed, "project={project} pin={pin:?}");
    }
}

/// A CLI-started daemon pins the project's route at startup; flipping the
/// project setting afterwards never changes what its next worker gets. Runs
/// in a child process because the pin is process environment.
#[test]
fn a_cli_started_daemon_keeps_its_startup_route_after_the_project_setting_flips() {
    const CHILD: &str = "CAS_TEST_GATEWAY_PIN_CHILD";
    const RAN: &str = "CAS_TEST_GATEWAY_PIN_RAN";
    let Ok(start) = std::env::var(CHILD) else {
        for start in ["true", "false"] {
            let marker = tempfile::tempdir().unwrap();
            let ran = marker.path().join("ran");
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "gateway::tests::a_cli_started_daemon_keeps_its_startup_route_after_the_project_setting_flips", "--nocapture"])
                .env(CHILD, start)
                .env(RAN, &ran)
                .env_remove(ROUTE_ENV)
                .output()
                .unwrap();
            assert!(out.status.success(), "start={start}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            assert!(ran.exists(), "start={start}: the child must run the test body");
        }
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().join(".cas");
    let account = temp.path().join("account");
    std::fs::create_dir_all(&cas_root).unwrap();
    std::fs::create_dir_all(&account).unwrap();
    let endpoint = hub();
    let key = key_file(temp.path(), "client.key", KEY, 0o600);
    let hub_settings = format!("endpoint = \"{endpoint}/\"\nkey_file = \"{}\"\n", key.display());
    write_config(&cas_root, &format!("enabled = {start}\n{hub_settings}"));
    // The daemon starts (the same hook fork_first and app init call), then the project flips.
    pin_at_start(&cas_root);
    let flipped = if start == "true" { "false" } else { "true" };
    write_config(&cas_root, &format!("enabled = {flipped}\n{hub_settings}"));
    for cli in [SupervisorCli::Claude, SupervisorCli::Codex] {
        let worker = build(cli, &cas_root, &account);
        let routed = transport(&worker) == Some("gateway");
        assert_eq!(routed, start == "true", "{cli:?}: startup route must hold after the flip");
    }
    std::fs::write(std::env::var(RAN).unwrap(), "").unwrap();
}
