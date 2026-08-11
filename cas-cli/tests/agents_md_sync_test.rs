use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn cas_cmd(root: &std::path::Path) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cas"));
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
