use std::time::{Duration, Instant};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn file_snapshot(root: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    fn visit(
        root: &std::path::Path,
        current: &std::path::Path,
        files: &mut Vec<(std::path::PathBuf, Vec<u8>)>,
    ) {
        let mut entries = std::fs::read_dir(current)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                files.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                ));
            }
        }
    }

    let mut files = Vec::new();
    visit(root, root, &mut files);
    files
}

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
    // This is a real registered project fixture, so keep it beneath the
    // runtime test cwd rather than a system temp root that discovery treats as
    // disposable.
    let dir = TempDir::new_in(cas::test_paths::runtime_fixture_parent()).unwrap();
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

fn command(project: &TempDir, home: &TempDir) -> Command {
    let mut command = command_at(project.path(), home);
    command.args([
        "--cas-root",
        project.path().join(".cas").to_str().unwrap(),
    ]);
    command
}

fn command_at(cwd: &std::path::Path, home: &TempDir) -> Command {
    let mut command = Command::new(cas::test_paths::binary(
        "cas",
        option_env!("CARGO_BIN_EXE_cas").map(Into::into),
    ));
    command
        .current_dir(cwd)
        .env("HOME", home.path())
        .env("GROK_HOME", cwd.join(".test-grok-home"))
        .env_remove("CAS_ROOT")
        .env_remove("CAS_SOURCE_DIR")
        .env_remove("CAS_EXPECTED_DEPLOYMENT_SHA")
        .args(["--json", "factory", "preflight"]);
    command
}

fn human_command_at(cwd: &std::path::Path, home: &TempDir) -> Command {
    let mut command = Command::new(cas::test_paths::binary(
        "cas",
        option_env!("CARGO_BIN_EXE_cas").map(Into::into),
    ));
    command
        .current_dir(cwd)
        .env("HOME", home.path())
        .env("GROK_HOME", cwd.join(".test-grok-home"))
        .env_remove("CAS_ROOT")
        .env_remove("CAS_SOURCE_DIR")
        .env_remove("CAS_EXPECTED_DEPLOYMENT_SHA")
        .args(["factory", "preflight"]);
    command
}

#[test]
fn explicit_cas_root_for_another_repo_still_fails_critical() {
    let active = project(true);
    let other = project(true);
    let home = TempDir::new().unwrap();
    let assertion = command_at(active.path(), &home)
        .args(["--cas-root", other.path().join(".cas").to_str().unwrap()])
        .assert()
        .failure();
    let report: Value =
        serde_json::from_slice(&assertion.get_output().stdout).expect("critical JSON report");
    assert_eq!(report["factory_blocked"], true);
    assert_eq!(report["repository"]["state"], "critical");
    assert!(
        report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["code"] == "repository.wrong")
    );
}

#[test]
fn nested_cwd_uses_the_resolved_cas_project_root_for_all_evidence() {
    let project = project(true);
    let nested = project.path().join("nested/deeper");
    std::fs::create_dir_all(&nested).unwrap();
    let home = TempDir::new().unwrap();
    let output = command_at(&nested, &home)
        .args(["--cas-root", project.path().join(".cas").to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&output).expect("stable JSON report");
    assert_eq!(report["repository"]["state"], "ready");
    assert_eq!(report["cas_mcp"]["configured"], true);
    assert_eq!(report["factory_blocked"], false);
}

#[test]
fn human_cli_reports_ready_and_leaves_home_cas_state_unchanged() {
    let project = project(true);
    let nested = project.path().join("nested/deeper");
    std::fs::create_dir_all(&nested).unwrap();
    let home = TempDir::new().unwrap();
    std::fs::create_dir_all(home.path().join(".cas")).unwrap();
    std::fs::create_dir_all(home.path().join(".config/cas")).unwrap();
    std::fs::write(
        home.path().join(".cas/live-task-ledger.sqlite"),
        b"live-ledger-sentinel",
    )
    .unwrap();
    std::fs::write(
        home.path().join(".config/cas/known_repos.db"),
        b"live-registry-sentinel",
    )
    .unwrap();
    let before = file_snapshot(home.path());

    let output = human_command_at(&nested, &home)
        .args(["--cas-root", project.path().join(".cas").to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let human = String::from_utf8(output).unwrap();
    assert!(human.starts_with("Factory preflight:"));
    assert!(!human.contains("(factory blocked)"), "{human}");
    assert!(human.contains("repository: ready"), "{human}");
    assert!(human.contains("cas mcp: ready configured=true"), "{human}");
    assert_eq!(
        file_snapshot(home.path()),
        before,
        "preflight must not mutate ambient HOME/CAS state"
    );
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
    assert_eq!(report["schema_version"], 2);
    assert_eq!(report["factory_blocked"], false);
    assert_eq!(report["runtime_bound_ms"], 6500);
    assert!(report["runtime_elapsed_ms"].as_u64().unwrap() < 6500);
    assert!(report["timed_out_components"].is_array());
    assert_eq!(report["repository"]["state"], "ready");
    assert_eq!(report["cas_mcp"]["configured"], true);
    assert_eq!(report["cas_mcp"]["observed_via_mcp"], false);
    assert_eq!(report["cas_mcp"]["state"], "ready");
    assert_eq!(report["optional_upstreams"]["state"], "degraded");
    assert!(report["findings"].as_array().unwrap().iter().any(|finding| {
        finding["code"] == "optional_upstreams.health_missing"
    }));
    assert_eq!(
        report["harnesses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|harness| harness["harness"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["claude", "codex", "grok", "opencode"]
    );
    let harnesses = report["harnesses"].as_array().unwrap();
    assert_eq!(harnesses[0]["required"], true);
    assert_eq!(harnesses[1]["required"], true);
    assert_eq!(harnesses[2]["required"], false);
    assert_eq!(harnesses[3]["required"], false);
    for harness in harnesses {
        assert!(matches!(
            harness["default_probe"].as_str(),
            Some("observed" | "unavailable" | "timed_out")
        ));
    }

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
