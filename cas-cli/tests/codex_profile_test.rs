//! CLI wiring tests for `cas codex` — factory launch on a chosen ChatGPT
//! account (cas-9cc3), the Codex sibling of `claude_profile_test.rs`.
//!
//! The fake `codex` on PATH reproduces the real CLI's contract as verified
//! against codex-cli 0.147.0: `codex login status` prints `Logged in using …`
//! and exits 0, or prints `Not logged in` and exits 1, scoped entirely by
//! `CODEX_HOME`.

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
        .env_remove("CODEX_HOME")
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

/// A home with `~/.codex-alt` logged in, `~/.codex-work` not, plus a lock dir
/// that is not an account.
fn home_with_profiles() -> TempDir {
    let home = TempDir::new().unwrap();
    for dir in [
        ".codex",
        ".codex-alt",
        ".codex-work",
        ".codex-support@example.com.lock",
    ] {
        std::fs::create_dir_all(home.path().join(dir)).unwrap();
    }
    std::fs::write(home.path().join(".codex/config.toml"), "model = \"gpt-5\"\n").unwrap();
    std::fs::write(home.path().join(".codex/auth.json"), "{\"main\":true}").unwrap();

    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let codex = bin.join("codex");
    std::fs::write(
        &codex,
        r#"#!/bin/sh
if [ "$1" = "login" ] && [ "$2" = "status" ]; then
  case "$CODEX_HOME" in
    *".codex-alt") printf '%s\n' 'Logged in using ChatGPT'; exit 0 ;;
    *) printf '%s\n' 'Not logged in'; exit 1 ;;
  esac
fi
if [ "$1" = "login" ]; then
  printf 'LOGIN_CODEX_HOME=%s\n' "$CODEX_HOME"
  printf 'LOGIN_OPENAI_API_KEY=%s\n' "${OPENAI_API_KEY:-<scrubbed>}"
  printf 'LOGIN_ARGS=%s\n' "$*"
  exit 0
fi
exit 0
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&codex).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&codex, permissions).unwrap();
    home
}

#[test]
fn list_profiles_shows_detected_accounts_login_state_and_excludes_lock_dirs() {
    let home = home_with_profiles();
    let alt = home.path().join(".codex-alt");

    cas_cmd(home.path())
        .env("CODEX_HOME", &alt)
        .args(["codex", "--list-profiles"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: cas codex <profile>"))
        .stdout(predicate::str::contains("main"))
        .stdout(predicate::str::contains("alt"))
        .stdout(predicate::str::contains("logged in"))
        .stdout(predicate::str::contains("(active)"))
        .stdout(predicate::str::contains("work"))
        .stdout(predicate::str::contains("not logged in"))
        // a `.lock` directory is not an account and must not be selectable
        .stdout(predicate::str::contains("support@example.com").not());
}

/// The headline behavior: `cas codex alt` selects that account and hands off to
/// the factory launcher (which bails here only because the harness has no TTY).
#[test]
fn named_profile_selects_account_then_launches_factory() {
    let home = home_with_profiles();

    cas_cmd(home.path())
        .args(["codex", "alt"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Using Codex account home:"))
        .stderr(predicate::str::contains(".codex-alt"))
        .stderr(predicate::str::contains(
            "Factory mode requires an interactive terminal",
        ));
}

/// `main` resolves to `~/.codex`, not `~/.codex-main`.
#[test]
fn main_profile_resolves_to_default_codex_home() {
    let home = home_with_profiles();

    cas_cmd(home.path())
        .args(["codex", "main"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Using Codex account home:"))
        .stderr(predicate::str::contains(".codex-main").not())
        .stderr(predicate::str::contains(
            "Factory mode requires an interactive terminal",
        ));
}

/// A profile that is not logged in is called out rather than failing silently.
#[test]
fn logged_out_profile_is_announced_before_launch() {
    let home = home_with_profiles();

    cas_cmd(home.path())
        .args(["codex", "work"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not logged in yet"))
        .stderr(predicate::str::contains("cas codex login work"));
}

/// An explicitly named missing account preserves the non-TTY contract: it is
/// called out, but it never asks a script to answer the interactive login
/// prompt. The factory error proves launch preparation proceeded normally.
#[test]
fn non_tty_missing_named_profile_warns_then_reaches_factory() {
    let home = home_with_profiles();

    cas_cmd(home.path())
        .args(["codex", "support"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(".codex-support does not exist yet"))
        .stderr(predicate::str::contains("Using Codex account home:"))
        .stderr(predicate::str::contains(
            "Factory mode requires an interactive terminal",
        ));
}

/// Factory flags pass through both with and without a profile — `cas codex
/// --workers 2` predates the picker and must keep working.
#[test]
fn factory_flags_pass_through_with_and_without_a_profile() {
    let home = home_with_profiles();

    cas_cmd(home.path())
        .args(["codex", "alt", "--workers", "2", "--new"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Factory mode requires an interactive terminal",
        ));

    cas_cmd(home.path())
        .args(["codex", "--workers", "2"])
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
        .args(["codex", "alt", "--definitely-not-a-flag"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument"));
}

/// Non-interactive launches never prompt: the harness has no TTY, so this must
/// reach the factory rather than block on a picker.
#[test]
fn non_tty_launch_does_not_prompt() {
    let home = home_with_profiles();

    cas_cmd(home.path())
        .args(["codex"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Choose Codex account").not())
        .stderr(predicate::str::contains(
            "Factory mode requires an interactive terminal",
        ));
}

/// `cas codex login <profile>` creates the profile home, seeds the shared
/// configuration surface by symlink, and runs `codex login` scoped to it with
/// inherited API keys scrubbed. `auth.json` is never seeded.
#[test]
fn login_creates_seeds_and_scopes_the_profile() {
    let home = home_with_profiles();

    let output = cas_cmd(home.path())
        .env("OPENAI_API_KEY", "sk-inherited-must-not-win")
        .args(["codex", "login", "work@example.com"])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let profile = home.path().join(".codex-work@example.com");

    assert!(
        stdout.contains(&format!("LOGIN_CODEX_HOME={}", profile.display())),
        "codex login was not scoped to the profile home: {stdout}"
    );
    assert!(
        stdout.contains("LOGIN_OPENAI_API_KEY=<scrubbed>"),
        "an inherited OPENAI_API_KEY leaked into the login: {stdout}"
    );
    assert!(
        stderr.contains("Seeded shared config from ~/.codex: config.toml"),
        "seeding was not reported: {stderr}"
    );
    assert!(
        stderr.contains("Credentials and account identity remain private to this profile."),
        "the private-by-default guarantee was not stated: {stderr}"
    );

    // config.toml arrived as a symlink to the main home; auth.json did not arrive at all.
    let seeded = profile.join("config.toml");
    assert!(
        seeded.symlink_metadata().unwrap().file_type().is_symlink(),
        "config.toml should be a symlink into ~/.codex"
    );
    assert_eq!(
        std::fs::read_to_string(&seeded).unwrap(),
        "model = \"gpt-5\"\n"
    );
    assert!(
        !profile.join("auth.json").exists(),
        "credentials must never be seeded into another profile"
    );
}
