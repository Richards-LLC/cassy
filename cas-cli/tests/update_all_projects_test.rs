//! End-to-end coverage for the native multi-project update sweep (cas-4ee9).

use assert_cmd::Command;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn cas_cmd(root: &Path) -> Command {
    let mut cmd = Command::new(cas::test_paths::cas_binary());
    let home = root.join(".test-home");
    let xdg = root.join(".test-xdg-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    if let Some(host_home) = std::env::var_os("HOME") {
        cmd.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", xdg)
        .env_remove("CAS_ROOT")
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

fn init_project(root: &Path, project: &Path) {
    std::fs::create_dir_all(project).unwrap();
    cas_cmd(root)
        .current_dir(project)
        .args(["init", "--yes"])
        .assert()
        .success();
}

fn worker_skill(project: &Path) -> PathBuf {
    project.join(".claude/skills/cas-worker/SKILL.md")
}

#[test]
fn all_projects_dry_run_is_non_mutating_then_syncs_every_discovered_project() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let projects = root.join("projects");
    let first = projects.join("first");
    let second = projects.join("nested/second");
    init_project(root, &first);
    init_project(root, &second);

    let missing = worker_skill(&first);
    std::fs::remove_file(&missing).unwrap();

    let dry = cas_cmd(root)
        .current_dir(root)
        .env("CAS_PROJECT_ROOTS", &projects)
        .args(["update", "--all-projects", "--dry-run"])
        .assert()
        .success();
    let dry_out = String::from_utf8_lossy(&dry.get_output().stdout);
    assert!(
        dry_out.contains("2 local Cassy project(s)"),
        "output was:\n{dry_out}"
    );
    assert!(dry_out.contains("DRY RUN"), "output was:\n{dry_out}");
    assert!(!missing.exists(), "dry run must not restore deleted skills");

    let synced = cas_cmd(root)
        .current_dir(root)
        .env("CAS_PROJECT_ROOTS", &projects)
        .args(["update", "--all-projects"])
        .assert()
        .success();
    let synced_out = String::from_utf8_lossy(&synced.get_output().stdout);
    assert!(
        synced_out.contains("2 succeeded, 0 failed"),
        "output was:\n{synced_out}"
    );
    assert!(
        synced_out.contains("membership: skipped: not cloud-linked"),
        "offline/unlinked team phase must be an advisory skip; output was:\n{synced_out}"
    );
    assert!(
        synced_out.contains("cloud sync: skipped: not cloud-linked"),
        "offline/unlinked cloud phase must be an advisory skip; output was:\n{synced_out}"
    );
    assert!(
        missing.is_file(),
        "native sweep must restore the stale builtin"
    );
}
