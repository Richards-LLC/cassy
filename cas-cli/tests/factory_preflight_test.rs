use std::time::{Duration, Instant};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn git(repo: &std::path::Path, args: &[&str]) {
    assert!(
        std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .unwrap()
            .success()
    );
}

fn project(with_mcp: bool) -> TempDir {
    let dir = TempDir::new().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(
        dir.path(),
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:org/preflight-cli.git",
        ],
    );
    std::fs::create_dir(dir.path().join(".cas")).unwrap();
    std::fs::write(
        dir.path().join(".cas/config.toml"),
        "[project]\ncanonical_id = \"factory-preflight-cli-test\"\n",
    )
    .unwrap();
    if with_mcp {
        std::fs::write(
            dir.path().join(".mcp.json"),
            r#"{"mcpServers":{"cas":{"command":"cas","args":["serve"]}}}"#,
        )
        .unwrap();
    }
    dir
}

#[allow(deprecated)]
fn command(project: &TempDir, home: &TempDir) -> Command {
    let mut command = Command::cargo_bin("cas").unwrap();
    command
        .current_dir(project.path())
        .env("HOME", home.path())
        .env_remove("CAS_ROOT")
        .env_remove("CAS_SOURCE_DIR")
        .env_remove("CAS_EXPECTED_DEPLOYMENT_SHA")
        .args(["--json", "factory", "preflight"]);
    command
}

#[test]
fn json_cli_is_bounded_deterministic_and_warnings_exit_zero() {
    let project = project(true);
    let home = TempDir::new().unwrap();
    let started = Instant::now();
    let output = command(&project, &home)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        started.elapsed() < Duration::from_secs(7),
        "preflight exceeded bound: {:?}",
        started.elapsed()
    );
    let report: Value = serde_json::from_slice(&output).expect("stable JSON report");
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["factory_blocked"], false);
    assert_eq!(report["runtime_bound_ms"], 6500);
    assert_eq!(report["repository"]["state"], "ready");
    assert_eq!(report["cas_mcp"]["configured"], true);
    assert_eq!(report["cas_mcp"]["observed_via_mcp"], false);
    assert_eq!(report["cas_mcp"]["state"], "ready");
    assert_eq!(report["optional_upstreams"]["state"], "ready");
    assert_eq!(
        report["harnesses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|harness| harness["harness"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["claude", "codex", "grok"]
    );

    let json = String::from_utf8(output).unwrap();
    for forbidden in [
        project.path().to_string_lossy().as_ref(),
        "https://",
        "Bearer ",
        "token=",
    ] {
        assert!(!json.contains(forbidden), "{forbidden} leaked: {json}");
    }
}

#[test]
fn cli_missing_cas_mcp_registration_prints_json_then_exits_nonzero() {
    let project = project(false);
    let home = TempDir::new().unwrap();
    let assertion = command(&project, &home).assert().failure();
    let report: Value =
        serde_json::from_slice(&assertion.get_output().stdout).expect("critical JSON report");
    assert_eq!(report["overall"], "critical");
    assert_eq!(report["factory_blocked"], true);
    assert_eq!(report["cas_mcp"]["configured"], false);
    assert_eq!(report["cas_mcp"]["observed_via_mcp"], false);
    assert!(
        report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| {
                finding["code"] == "cas_mcp.registration_missing"
                    && finding["severity"] == "critical"
            })
    );
}
