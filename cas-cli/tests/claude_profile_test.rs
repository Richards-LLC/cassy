//! CLI wiring tests for `cas claude` — factory launch on a chosen Claude account.

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

/// A home dir with `~/.claude-alt` logged in and `~/.claude-work` not.
fn home_with_profiles() -> TempDir {
    let home = TempDir::new().unwrap();
    let alt = home.path().join(".claude-alt");
    std::fs::create_dir_all(&alt).unwrap();
    std::fs::write(alt.join(".credentials.json"), "{}").unwrap();
    std::fs::create_dir_all(home.path().join(".claude-work")).unwrap();
    home
}

#[test]
fn list_profiles_shows_detected_accounts_and_login_state() {
    let home = home_with_profiles();
    let alt = home.path().join(".claude-alt");

    cas_cmd(home.path())
        .env("CLAUDE_CONFIG_DIR", &alt)
        .args(["claude", "--list-profiles"])
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

/// The headline behavior: `cas claude alt` selects the alt account and then
/// hands off to the factory launcher (which bails here only because the test
/// harness has no TTY).
#[test]
fn named_profile_selects_account_then_launches_factory() {
    let home = home_with_profiles();

    cas_cmd(home.path())
        .args(["claude", "alt"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Using Claude account config:"))
        .stderr(predicate::str::contains(".claude-alt"))
        .stderr(predicate::str::contains(
            "Factory mode requires an interactive terminal",
        ));
}

/// `main` resolves to `~/.claude`, not `~/.claude-main`.
#[test]
fn main_profile_resolves_to_default_config_dir() {
    let home = home_with_profiles();

    cas_cmd(home.path())
        .args(["claude", "main"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Using Claude account config:"))
        .stderr(predicate::str::contains(".claude\n").or(predicate::str::contains(".claude ")))
        .stderr(predicate::str::contains(
            "Factory mode requires an interactive terminal",
        ));
}

/// Factory flags pass through after the profile positional.
#[test]
fn factory_flags_pass_through_after_profile() {
    let home = home_with_profiles();

    cas_cmd(home.path())
        .args(["claude", "alt", "--workers", "2", "--new"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Factory mode requires an interactive terminal",
        ));
}

/// An unknown factory flag is rejected by the factory parser, not silently eaten.
#[test]
fn unknown_trailing_flag_is_rejected() {
    let home = home_with_profiles();

    cas_cmd(home.path())
        .args(["claude", "alt", "--definitely-not-a-flag"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument"));
}

/// Omitting the profile leaves the ambient account untouched and still launches
/// the factory — symmetric with `cas codex` / `cas grok`.
#[test]
fn bare_claude_launches_factory_without_touching_account() {
    let home = home_with_profiles();

    cas_cmd(home.path())
        .args(["claude"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Factory mode requires an interactive terminal",
        ))
        .stderr(predicate::str::contains("Using Claude account config:").not());
}

#[test]
fn help_documents_profile_and_factory_passthrough() {
    let home = home_with_profiles();

    cas_cmd(home.path())
        .args(["claude", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("supervisor"))
        .stdout(predicate::str::contains("PROFILE"))
        .stdout(predicate::str::contains("--list-profiles"))
        .stdout(predicate::str::contains("--bare"));
}
