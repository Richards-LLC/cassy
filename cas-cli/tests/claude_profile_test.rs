//! CLI wiring tests for `cas claude` account-profile launching.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn cas_cmd(home: &std::path::Path) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cas"));
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".xdg"))
        .env_remove("CAS_ROOT")
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

#[test]
fn bare_claude_lists_usage_and_detected_profiles() {
    let home = TempDir::new().unwrap();
    let alt = home.path().join(".claude-alt");
    std::fs::create_dir_all(&alt).unwrap();
    std::fs::write(alt.join(".credentials.json"), "{}").unwrap();
    std::fs::create_dir_all(home.path().join(".claude-work")).unwrap();

    cas_cmd(home.path())
        .env("CLAUDE_CONFIG_DIR", &alt)
        .arg("claude")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: cas claude <profile>"))
        .stdout(predicate::str::contains("main"))
        .stdout(predicate::str::contains("alt"))
        .stdout(predicate::str::contains("logged in"))
        .stdout(predicate::str::contains("(active)"))
        .stdout(predicate::str::contains("work"))
        .stdout(predicate::str::contains("not logged in"));
}
