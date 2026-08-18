//! CLI wiring tests for `cas claude` — factory launch on a chosen Claude account.

use assert_cmd::Command;
use predicates::prelude::*;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

fn cas_cmd(home: &std::path::Path) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cas"));
    let path = std::env::join_paths(std::iter::once(home.join("bin")).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".xdg"))
        .env("PATH", path)
        .env_remove("CAS_ROOT")
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

/// A home dir with `~/.claude-alt` logged in and `~/.claude-work` not.
fn home_with_profiles() -> TempDir {
    let home = TempDir::new().unwrap();
    let alt = home.path().join(".claude-alt");
    std::fs::create_dir_all(&alt).unwrap();
    std::fs::create_dir_all(home.path().join(".claude-work")).unwrap();
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let claude = bin.join("claude");
    std::fs::write(
        &claude,
        r#"#!/bin/sh
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  case "$CLAUDE_SECURESTORAGE_CONFIG_DIR" in
    *".claude-alt") printf '%s\n' '{"loggedIn":true}' ;;
    *) printf '%s\n' '{"loggedIn":false}' ;;
  esac
  exit 0
fi
if [ "$1" = "auth" ] && [ "$2" = "login" ]; then
  printf 'LOGIN_CONFIG=%s\n' "$CLAUDE_CONFIG_DIR"
  printf 'LOGIN_SECURE_STORAGE=%s\n' "$CLAUDE_SECURESTORAGE_CONFIG_DIR"
  printf 'LOGIN_ARGS=%s\n' "$*"
  exit 0
fi
exit 0
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&claude).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&claude, permissions).unwrap();
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
        .stdout(predicate::str::contains("login"))
        .stdout(predicate::str::contains("--list-profiles"))
        .stdout(predicate::str::contains("--bare"));
}

#[test]
fn login_subcommand_binds_auth_flow_to_named_profile() {
    let home = home_with_profiles();

    cas_cmd(home.path())
        .args(["claude", "login", "alt", "--email", "alt@example.com"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("LOGIN_CONFIG=").and(predicate::str::contains(".claude-alt")),
        )
        .stdout(
            predicate::str::contains("LOGIN_SECURE_STORAGE=")
                .and(predicate::str::contains(".claude-alt")),
        )
        .stdout(predicate::str::contains(
            "LOGIN_ARGS=auth login --email alt@example.com",
        ));
}

#[test]
fn login_subcommand_keeps_main_on_legacy_default_credential_store() {
    let home = home_with_profiles();

    cas_cmd(home.path())
        .args(["claude", "login", "main"])
        .assert()
        .success()
        .stdout(predicate::str::contains("LOGIN_CONFIG=\n"))
        .stdout(predicate::str::contains("LOGIN_SECURE_STORAGE=\n"));
}

/// `cas claude --workers 0` errored with "unexpected argument" until cas-6dad:
/// a dedicated `profile` positional made clap reject a leading factory flag.
/// Both spellings must reach the factory parser.
#[test]
fn factory_flags_pass_through_with_and_without_a_profile() {
    let home = home_with_profiles();

    cas_cmd(home.path())
        .args(["claude", "--workers", "0", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--default"));

    cas_cmd(home.path())
        .args(["claude", "main", "--workers", "0", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--default"));
}
