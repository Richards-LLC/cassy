//! cas-4362: Violet CLI output is one JSON report or the human summary.
#![cfg(feature = "mcp-proxy")]

use assert_cmd::Command;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

fn violet_command(root: &Path) -> Command {
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut command = Command::new(cas::test_paths::cas_binary());
    // Factory and host credential overrides must not reach the fixture.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CAS_")
            || key.to_string_lossy().starts_with("VIOLET_")
            || key.to_string_lossy().starts_with("SLACK_")
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
        .timeout(Duration::from_secs(20));
    command
}

#[test]
fn violet_json_stdout_is_one_report_for_both_global_flag_positions() {
    for leading_json in [true, false] {
        let temp = TempDir::new().unwrap();
        let mut command = violet_command(temp.path());
        if leading_json {
            command.arg("--json");
        }
        command.args([
            "integrate",
            "violet",
            "--dry-run",
            "--skip-verify",
            "--no-harness",
            "--label",
            "CAS_4362",
            "--url",
            "https://violet.example.test/mcp/slack",
        ]);
        if !leading_json {
            command.arg("--json");
        }
        let output = command.assert().success().get_output().clone();
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)
            .expect("stdout must contain one complete JSON value, without trailing human text");
        assert_eq!(report["url"], "https://violet.example.test/mcp/slack");
        assert_eq!(report["token_env"], "VIOLET_SLACK_TOKEN_CAS_4362");
        assert_eq!(report["registration"], "planned");
        assert_eq!(report["probe"]["result"], "skipped");
        assert!(
            !temp
                .path()
                .join("home/.config/code-mode-mcp/config.toml")
                .exists()
        );
    }
}

#[test]
fn violet_without_json_keeps_the_human_summary() {
    let temp = TempDir::new().unwrap();
    let output = violet_command(temp.path())
        .args([
            "integrate",
            "violet",
            "--dry-run",
            "--skip-verify",
            "--no-harness",
            "--label",
            "CAS_4362",
            "--url",
            "https://violet.example.test/mcp/slack",
        ])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("violet init: "), "{stdout}");
    assert!(
        stdout.contains("https://violet.example.test/mcp/slack"),
        "{stdout}"
    );
    assert!(
        stdout.contains("authenticated tools/list: skipped (--skip-verify)"),
        "{stdout}"
    );
    assert!(serde_json::from_str::<serde_json::Value>(&stdout).is_err());
}

#[tokio::test]
async fn violet_rejected_probe_keeps_json_clean_and_human_diagnostics() {
    use wiremock::matchers::any;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let hub = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(401))
        .mount(&hub)
        .await;
    for json in [true, false] {
        let temp = TempDir::new().unwrap();
        let mut command = violet_command(temp.path());
        command
            .env("VIOLET_SLACK_TOKEN_CAS_4362", "fixture-bearer")
            .env("VIOLET_VERCEL_BYPASS", "fixture-bypass")
            .args([
                "integrate",
                "violet",
                "--no-harness",
                "--label",
                "CAS_4362",
                "--url",
                &hub.uri(),
            ]);
        if json {
            command.arg("--json");
        }
        let output =
            tokio::task::spawn_blocking(move || command.assert().failure().get_output().clone())
                .await
                .unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("Violet refused this machine's credential (HTTP 401)"),
            "{stderr}"
        );
        if json {
            let report: serde_json::Value = serde_json::from_slice(&output.stdout)
                .expect("even a rejected probe must not append human text to its JSON report");
            assert_eq!(report["probe"]["result"], "unauthorized");
        } else {
            let stdout = String::from_utf8(output.stdout.clone()).unwrap();
            assert!(
                stdout.contains("authenticated tools/list: refused (HTTP 401"),
                "{stdout}"
            );
        }
        for secret in ["fixture-bearer", "fixture-bypass"] {
            assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
            assert!(!stderr.contains(secret));
        }
    }
    assert!(!hub.received_requests().await.unwrap().is_empty());
}
