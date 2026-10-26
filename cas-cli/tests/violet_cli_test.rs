//! cassy#1157: `cas violet post|thread|read` from the real binary.
//!
//! The hub double lives in the unit tests (`cli::violet_cmd::tests`); these
//! cases pin what only the binary shows: the command is routed, local
//! mistakes fail before any network, and a failure exits non-zero with its
//! named code on stderr, or as one JSON document under `--json`.
#![cfg(feature = "mcp-proxy")]

use assert_cmd::Command;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

/// A `cas` with an empty home: no proxy registration, no credentials.
fn violet(root: &Path) -> Command {
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut command = Command::new(cas::test_paths::cas_binary());
    // Legacy credential names come from the compatibility manifest, never
    // spelled here.
    let legacy = cas_types::violet_compatibility::violet_compatibility();
    for (key, _) in std::env::vars_os() {
        let key_text = key.to_string_lossy();
        if key_text.starts_with("CAS_")
            || key_text.starts_with("VIOLET_")
            || key_text.starts_with("SLACK_")
            || key_text.starts_with(legacy.legacy_token_prefix.as_str())
            || key_text == legacy.legacy_bypass_env
        {
            command.env_remove(key);
        }
    }
    if let Some(host_home) = std::env::var_os("HOME") {
        command.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    command
        .current_dir(root)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("CODEX_HOME", home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", &home)
        .env("CAS_SKIP_FACTORY_TOOLING", "1")
        .env_remove("COLUMNS")
        .timeout(Duration::from_secs(20));
    command.arg("violet");
    command
}

#[test]
fn violet_help_lists_post_thread_and_read() {
    let temp = TempDir::new().unwrap();
    let output = violet(temp.path())
        .arg("--help")
        .assert()
        .success()
        .get_output()
        .clone();
    let help = String::from_utf8(output.stdout).unwrap();
    for subcommand in ["post", "thread", "read"] {
        assert!(help.contains(subcommand), "{help}");
    }
}

#[test]
fn an_unregistered_machine_fails_with_not_configured() {
    let temp = TempDir::new().unwrap();
    let output = violet(temp.path())
        .args(["post", "--channel", "cas-internal", "--text", "hi"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("not_configured"), "{stderr}");
    assert!(stderr.contains("cas integrate violet"), "{stderr}");
    assert!(output.stdout.is_empty());
}

#[test]
fn json_failure_is_one_error_document_and_a_non_zero_exit() {
    let temp = TempDir::new().unwrap();
    let output = violet(temp.path())
        .args([
            "post",
            "--channel",
            "cas-internal",
            "--text",
            "hi",
            "--json",
        ])
        .assert()
        .failure()
        .get_output()
        .clone();
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout must be one complete JSON document");
    assert_eq!(report["ok"], false);
    assert_eq!(report["error"]["code"], "not_configured");
    assert_eq!(report["error"]["retryable"], false);
}

#[test]
fn a_missing_file_fails_locally_before_the_hub() {
    let temp = TempDir::new().unwrap();
    let output = violet(temp.path())
        .args(["post", "--channel", "cas-internal", "--file", "absent.png"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    // A path problem is reported as such, not masked by the missing
    // registration the hub connection would hit next.
    assert!(stderr.contains("local_request_failed"), "{stderr}");
    assert!(!stderr.contains("not_configured"), "{stderr}");
}

#[test]
fn a_channel_read_without_since_is_refused() {
    let temp = TempDir::new().unwrap();
    let output = violet(temp.path())
        .args(["read", "--channel", "cas-internal"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("invalid_input"), "{stderr}");
    assert!(stderr.contains("--since"), "{stderr}");
}

/// The real binary reaches the credential boundary without contacting a hub:
/// an intentionally missing bypass stops it after bearer selection.
#[test]
fn cas_0a4c_thread_resolves_registered_private_bearer_before_missing_bypass() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new().unwrap();
    let mut command = violet(temp.path());
    let home = temp.path().join("home");
    let config = home.join(".config/code-mode-mcp/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(config, format!("# managed\n[servers.violet]\ntransport='http'\nurl={:?}\nauth='env:VIOLET_SLACK_TOKEN_CASSY_PROXY'\n[servers.violet.headers]\nx-vercel-protection-bypass='env:VIOLET_VERCEL_BYPASS'\n", cmcp_core::config::violet_hub_url())).unwrap();
    std::fs::write(home.join(".claude.json"), r#"{"mcpServers":{"violet":{"headers":{"Authorization":"Bearer ${VIOLET_SLACK_TOKEN_ACCOUNT}"}}}}"#).unwrap();
    let credentials = home.join("credentials.env");
    std::fs::write(
        &credentials,
        "VIOLET_SLACK_TOKEN_ACCOUNT='binary-private-fixture'\n",
    )
    .unwrap();
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600)).unwrap();
    let output = command
        .env("CAS_CREDENTIALS_FILE", &credentials)
        .args([
            "thread",
            "--channel",
            "fixture",
            "--text",
            "fixture",
            "--reply",
            "fixture",
            "--idempotency-key",
            "cas-0a4c-fixture",
            "--json",
        ])
        .assert()
        .failure()
        .get_output()
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["error"]["code"], "missing_credential");
    let message = report["error"]["message"].as_str().unwrap();
    assert!(message.contains("VIOLET_VERCEL_BYPASS"));
    assert!(!message.contains("VIOLET_SLACK_TOKEN_CASSY_PROXY"));
    assert!(!message.contains("binary-private-fixture"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("binary-private-fixture"));
}

#[test]
fn cas_0a4c_real_binary_missing_selector_is_one_secret_free_json_error() {
    let temp = TempDir::new().unwrap();
    let mut command = violet(temp.path());
    let config = temp.path().join("home/.config/code-mode-mcp/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(config, format!("# managed\n[servers.violet]\ntransport='http'\nurl={:?}\nauth='env:VIOLET_SLACK_TOKEN_CASSY_PROXY'\n", cmcp_core::config::violet_hub_url())).unwrap();
    let output = command
        .env("VIOLET_SLACK_TOKEN_ENV", "VIOLET_SLACK_TOKEN_MISSING")
        .env("VIOLET_SLACK_TOKEN_CASSY_PROXY", "binary-default-fixture")
        .args([
            "read",
            "--channel",
            "fixture",
            "--since",
            "2026-10-15T00:00:00Z",
            "--json",
        ])
        .assert()
        .failure()
        .get_output()
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["error"]["code"], "missing_credential");
    assert_eq!(report["error"]["retryable"], false);
    assert!(
        report["error"]["message"]
            .as_str()
            .unwrap()
            .contains("VIOLET_SLACK_TOKEN_MISSING")
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("binary-default-fixture"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("binary-default-fixture"));
}

/// Read the real CLI's report against isolated ordinary-auth REST services.
/// No registration, mapping mutation, Slack body read or activation is allowed.
fn channels_report(mappings: serde_json::Value, inventory: Vec<&str>, authorities: Vec<(&str, serde_json::Value)>, registration: bool, expected_exit: i32) -> serde_json::Value {
    use serde_json::json;
    let temp = TempDir::new().unwrap();
    let root = temp.path().join(".cas");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.toml"), "[project]\ncanonical_id = 'qa.invalid/channels'\n[slack]\nchannels = ['C03CONFIG']\n").unwrap();
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", server.server_addr());
    cas::cloud::CloudConfig {
        endpoint: endpoint.clone(), token: Some("ordinary-fixture-user".into()),
        team_id: Some("qa-team".into()), ..Default::default()
    }.save_to_cas_dir(&root).unwrap();
    let inventory = inventory.into_iter().map(|channel| json!({"binding":{"channel_id":channel,"project_id":"qa.invalid/channels"}})).collect::<Vec<_>>();
    let mut responses = vec![("/api/channels".to_owned(), json!({"channels": mappings}))];
    if registration {
        responses.push(("/api/coordination/bindings/activation?registration_id=qa-supervisor".to_owned(), json!({"schema":"cassy.coordination/v1","code":"completed","bindings":inventory})));
    }
    responses.extend(authorities.into_iter().map(|(channel, body)| (
        format!("/api/coordination/bindings/authority?team_id=qa-team&channel_id={channel}"), body
    )));
    let remote = std::thread::spawn(move || {
        for (path, body) in responses {
            let request = server.recv_timeout(Duration::from_secs(10)).unwrap().expect("report request");
            assert_eq!(request.method(), &tiny_http::Method::Get, "report is read only");
            assert_eq!(request.url(), path);
            assert!(request.headers().iter().any(|h| h.field.equiv("Authorization") && h.value.as_str() == "Bearer ordinary-fixture-user"));
            // Deployed activation inventory is discovery, with Vercel's default
            // caching header; Violet and binding authority explicitly use no-store.
            let cache_control = if path.starts_with("/api/coordination/bindings/activation?") {
                "public, max-age=0, must-revalidate"
            } else {
                "no-store"
            };
            request.respond(tiny_http::Response::from_string(body.to_string()).with_header(
                tiny_http::Header::from_bytes("Cache-Control", cache_control).unwrap()
            )).unwrap();
        }
        assert!(server.try_recv().unwrap().is_none(), "no activation or registration");
    });
    let mut command = violet(temp.path());
    command.env_remove("CLAUDE_CODE_SESSION_ID").args(["channels", "--url", &endpoint, "--json"]);
    if registration { command.args(["--registration-id", "qa-supervisor"]); }
    let output = command.output().unwrap();
    // Join even when the CLI fails, so no fixture server outlives this test.
    let served = remote.join();
    assert_eq!(output.status.code(), Some(expected_exit), "unexpected report status: {}", String::from_utf8_lossy(&output.stderr));
    served.unwrap();
    assert!(!root.join("cas.db").exists(), "listing must not create a local agent database");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let snapshot: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("violet-channel-check.json")).expect("channel inspection saves local evidence")
    ).unwrap();
    assert_eq!(snapshot["project_id"], "qa.invalid/channels");
    assert_eq!(snapshot["team_id"], "qa-team");
    assert_eq!(snapshot["channels"][0]["channel_id"], "C03CONFIG");
    let configured = report["channels"].as_array().unwrap().iter().find(|row| row["configured"] == true).unwrap();
    assert_eq!(snapshot["channels"][0]["flags"], configured["flags"]);
    assert!(!snapshot.to_string().contains("ordinary-fixture-user"));
    report
}

fn channel_authority(channel: &str, project: &str, verified: bool) -> serde_json::Value {
    serde_json::json!({
        "schema":"cassy.binding-reconciliation/v1", "code":"completed",
        "binding": {"id":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","provider":"slack","team_id":"qa-team",
            "workspace_id":"T0QACHANNELS","channel_id":channel,"project_id":project,"mode":"managed",
            "verified":verified,"fence_generation":1,"activated_at":"2026-10-24T16:00:00.000Z","legacy_fenced_at":"2026-10-24T16:00:00.000Z"},
        "strict_activation":if verified { "evidenced" } else { "unevidenced" }
    })
}

#[test]
fn channels_report_unions_both_stores_and_configured_unmapped_channels() {
    use serde_json::json;
    let absent = json!({"schema":"cassy.binding-reconciliation/v1","code":"completed","binding":null,"strict_activation":null});
    let report = channels_report(json!([
        {"channel_id":"C01BOTH","channel_name":"both","project_id":"qa.invalid/channels"},
        {"channel_id":"C04VIOLET","channel_name":"violet-only","project_id":"qa.invalid/channels"},
        {"channel_id":"C09OTHER","channel_name":"other","project_id":"another/project"}
    ]), vec!["C01BOTH","C02CLOUD"], vec![
        ("C01BOTH",channel_authority("C01BOTH","qa.invalid/channels",true)),
        ("C02CLOUD",channel_authority("C02CLOUD","qa.invalid/channels",false)),
        ("C03CONFIG",absent.clone()), ("C04VIOLET",absent),
    ], true, 1);
    assert_eq!(report["complete"], true);
    let rows = report["channels"].as_array().unwrap();
    assert_eq!(rows.iter().map(|r| r["channel_id"].as_str().unwrap()).collect::<Vec<_>>(), ["C01BOTH","C02CLOUD","C03CONFIG","C04VIOLET"]);
    assert_eq!(rows[0]["cloud_verified"],true);
    assert_eq!(rows[1]["cloud_verified"],false);
    assert_eq!(rows[2]["configured"],true);
    assert_eq!(rows[2]["flags"],json!(["violet_unmapped","cloud_unmapped"]));
    assert_eq!(rows[3]["flags"],json!(["cloud_unmapped"]));
    assert!(report.to_string().contains("cas update"));

    let without_registration = channels_report(json!([
        {"channel_id":"C01BOTH","channel_name":"both","project_id":"qa.invalid/channels"}
    ]), vec![], vec![
        ("C01BOTH",channel_authority("C01BOTH","qa.invalid/channels",true)),
        ("C03CONFIG",json!({"schema":"cassy.binding-reconciliation/v1","code":"completed","binding":null,"strict_activation":null})),
    ], false, 1);
    assert_eq!(without_registration["complete"],false);
    assert_eq!(without_registration["channels"].as_array().unwrap().len(),2);
    assert!(without_registration["errors"].to_string().contains("Cloud-only discovery unavailable"));
}

#[test]
fn channels_flags_project_mismatches_and_never_calls_them_verified_matches() {
    use serde_json::json;
    let report = channels_report(json!([
        {"channel_id":"C01BOTH","channel_name":"wrong-cloud","project_id":"qa.invalid/channels"},
        {"channel_id":"C02CLOUD","channel_name":"wrong-violet","project_id":"another/project"}
    ]), vec!["C02CLOUD"], vec![
        ("C01BOTH",channel_authority("C01BOTH","another/project",true)),
        ("C02CLOUD",channel_authority("C02CLOUD","qa.invalid/channels",true)),
        ("C03CONFIG",json!({"schema":"cassy.binding-reconciliation/v1","code":"completed","binding":null,"strict_activation":null})),
    ], true, 1);
    let rows=report["channels"].as_array().unwrap();
    for row in &rows[..2] {
        assert_eq!(row["flags"],json!(["project_mismatch"]),"{row}");
        assert_eq!(row["cloud_verified"],true,"stored verification is distinct from matching mappings");
        assert!(row["remedy"].as_str().unwrap().contains("inspect"),"a mismatch must not silently remap a channel");
    }
    assert_eq!(report["ok"],false);
}

#[test]
fn channels_registered_inventory_accepts_deployed_discovery_cache_headers() {
    let report = channels_report(serde_json::json!([
        {"channel_id":"C03CONFIG","channel_name":"configured","project_id":"qa.invalid/channels"}
    ]), vec!["C03CONFIG"], vec![
        ("C03CONFIG",channel_authority("C03CONFIG","qa.invalid/channels",true)),
    ], true, 0);
    assert_eq!(report["complete"],true);
    assert_eq!(report["ok"],true);
    assert_eq!(report["channels"].as_array().unwrap().len(),1);
    assert_eq!(report["channels"][0]["cloud_verified"],true);
}
