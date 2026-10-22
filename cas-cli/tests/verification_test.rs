//! Verification contracts through the current MCP surface and live config CLI.

use assert_cmd::Command;
use cas::store::init_cas_dir;
use predicates::prelude::*;

use crate::test_env_guard::TestEnvGuard;

#[path = "verification_test_cases/current.rs"]
mod current;

/// The config CLI remains supported. `verification.enabled` is operator-only
/// (cas-0d4f0): the operator path (here, the library as a test fixture) sets
/// it, `cas config get` reads it, and a non-interactive agent `cas config set`
/// is refused without changing it.
#[test]
fn test_verification_config_toggle() {
    let mut env = TestEnvGuard::temp_home();
    let project = env.home().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let root = init_cas_dir(&project).unwrap();
    env.set("CAS_ROOT", &root);
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    let command = || {
        let mut command = Command::new(cas::test_paths::cas_binary());
        command.env("HOME", env.home());
        command.current_dir(&project);
        command
    };
    for expected in ["true", "false"] {
        let mut config = cas::config::Config::load(&root).unwrap();
        config.set("verification.enabled", expected).unwrap();
        config.save(&root).unwrap();
        command()
            .args(["config", "get", "verification.enabled"])
            .assert()
            .success()
            .stdout(predicate::str::contains(expected));
    }
    let human_refusal = command()
        .env("CLAUDECODE", "1")
        .args(["config", "set", "verification.enabled", "true"])
        .output()
        .expect("cas config set must run");
    assert!(!human_refusal.status.success());
    let stderr = String::from_utf8_lossy(&human_refusal.stderr);
    assert!(
        stderr.contains("Operator-only configuration change refused."),
        "the CLI should show the short refusal headline; stderr: {stderr}"
    );
    assert!(
        stderr.lines().skip(1).all(|line| line.starts_with("  ")),
        "refusal details should be indented; stderr: {stderr}"
    );
    assert!(
        stderr.lines().all(|line| line.chars().count() <= 80),
        "refusal should fit 80 columns; stderr: {stderr}"
    );
    let json_refusal = command()
        .env("CLAUDECODE", "1")
        .args(["--json", "config", "set", "verification.enabled", "true"])
        .output()
        .expect("cas --json config set must run");
    assert!(!json_refusal.status.success());
    let json: serde_json::Value = serde_json::from_slice(&json_refusal.stdout)
        .expect("--json refusal should still be one JSON document");
    assert_eq!(json["status"], "error");
    assert_eq!(
        json["message"],
        "refused: CLAUDECODE is set, so this runs inside an agent session. verification.enabled is operator-only because it relaxes a Cassy guard; run `cas config set verification.enabled <value>` yourself from your own terminal. If this is your own terminal, it inherited CLAUDECODE from an agent pane: open a fresh terminal and run it there."
    );
    command()
        .args(["config", "get", "verification.enabled"])
        .assert()
        .success()
        .stdout(predicate::str::contains("false"));
}
