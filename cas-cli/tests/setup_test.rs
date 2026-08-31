//! Acceptance coverage for the guided machine setup command.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn cas_setup_cmd(root: &TempDir) -> Command {
    let home = root.path().join("home");
    let xdg = root.path().join("xdg-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();

    let mut command = Command::new(cas::test_paths::cas_binary());
    command
        .current_dir(root.path())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", xdg)
        .env("SHELL", "/bin/bash")
        .env("PATH", "/usr/bin")
        .env_remove("CAS_ROOT")
        .env_remove("CAS_CLOUD_TOKEN")
        .env_remove("CAS_CLOUD_ENDPOINT")
        .env_remove("VIKTOR_API_KEY")
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    command
}

#[test]
fn dry_run_prints_the_complete_fresh_machine_plan_without_writes() {
    let root = TempDir::new().unwrap();

    cas_setup_cmd(&root)
        .args(["setup", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Cassy setup (dry-run; no changes)",
        ))
        .stdout(predicate::str::contains("1    skipped"))
        .stdout(predicate::str::contains("cas login"))
        .stdout(predicate::str::contains("cas device register"))
        .stdout(predicate::str::contains("cas hub service install"))
        .stdout(predicate::str::contains("cas viktor key"))
        .stdout(predicate::str::contains("cas setup --project <DIR>"))
        .stdout(predicate::str::contains("7    action-needed"));

    assert!(!root.path().join("home/.profile").exists());
    assert!(!root.path().join("home/.cas").exists());
    assert!(!root.path().join("xdg-config/cas/device.json").exists());
}

#[test]
fn setup_help_exposes_machine_options() {
    let root = TempDir::new().unwrap();

    cas_setup_cmd(&root)
        .args(["setup", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--yes"))
        .stdout(predicate::str::contains("--dry-run"))
        .stdout(predicate::str::contains("--project <DIR>"))
        .stdout(predicate::str::contains("--token <TOKEN>"));
}

#[test]
fn json_dry_run_has_exactly_seven_status_steps() {
    let root = TempDir::new().unwrap();

    let output = cas_setup_cmd(&root)
        .args(["--json", "setup", "--dry-run"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let steps = report["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 7);
    assert_eq!(steps[0]["name"], "Environment");
    assert_eq!(steps[1]["name"], "Cloud login + team");
    assert_eq!(steps[2]["name"], "Machine pairing");
    assert_eq!(steps[3]["name"], "Hub service");
    assert_eq!(steps[4]["name"], "Viktor key");
    assert_eq!(steps[5]["name"], "First project");
    assert_eq!(steps[6]["name"], "Final status");
    assert!(report["dry_run"].as_bool().unwrap());
}
