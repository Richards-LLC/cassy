use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn cas_cmd(root: &std::path::Path) -> Command {
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

#[test]
fn agents_md_write_and_check_have_expected_staleness_behavior() {
    let project = TempDir::new().unwrap();
    std::fs::write(
        project.path().join("CLAUDE.md"),
        "mcp__cas__task\n<!-- claude-only:start -->\nsecret\n<!-- claude-only:end -->\n<!-- codex-only:start -->\ncodex note\n<!-- codex-only:end -->\n",
    )
    .unwrap();

    cas_cmd(project.path())
        .current_dir(&project)
        .args(["sync", "agents-md", "--write"])
        .assert()
        .success();

    let generated = std::fs::read_to_string(project.path().join("AGENTS.md")).unwrap();
    assert!(generated.contains("mcp__cs__task"));
    assert!(generated.contains("codex note"));
    assert!(!generated.contains("secret"));

    cas_cmd(project.path())
        .current_dir(&project)
        .args(["sync", "agents-md", "--check"])
        .assert()
        .success()
        .stdout(predicate::str::contains("current"));

    std::fs::write(project.path().join("CLAUDE.md"), "new content\n").unwrap();
    cas_cmd(project.path())
        .current_dir(&project)
        .args(["sync", "agents-md", "--check"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("stale"));
}

/// Skills audit L6 F7: the tracked AGENTS.md went stale for a week because
/// nothing checked this repository's own generated file, only the command.
/// Code and mixed diffs run this; docs-only diffs hit the same check in the
/// Docs Lint CI job.
#[test]
fn repository_agents_md_is_current() {
    // Resolve the checkout at runtime: archive-mode tests run from a checkout
    // at a different path than the producer's CARGO_MANIFEST_DIR.
    let repo_root = cas::test_paths::workspace_root();
    if !repo_root.join("AGENTS.md").is_file() || !repo_root.join("CLAUDE.md").is_file() {
        eprintln!(
            "SKIP repository_agents_md_is_current: source checkout is absent at {}",
            repo_root.display()
        );
        return;
    }
    let scratch = TempDir::new().unwrap();
    let mut cmd = Command::new(cas::test_paths::cas_binary());
    let home = scratch.path().join("home");
    let xdg = scratch.path().join("xdg");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    if let Some(host_home) = std::env::var_os("HOME") {
        cmd.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", xdg)
        .env_remove("CAS_ROOT")
        .env("CAS_SKIP_FACTORY_TOOLING", "1")
        .current_dir(&repo_root)
        .args(["sync", "agents-md", "--check"])
        .assert()
        .success()
        .stdout(predicate::str::contains("current"));
}
