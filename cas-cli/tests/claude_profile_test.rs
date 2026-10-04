//! CLI wiring tests for `cas claude` — factory launch on a chosen Claude account.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

const CREDENTIAL_OVERRIDES: [&str; 5] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
];

fn cas_cmd(home: &std::path::Path) -> Command {
    let mut cmd = Command::new(cas::test_paths::cas_binary());
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
    cas::test_paths::warm_stub(
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
  for key in ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN CLAUDE_CODE_OAUTH_TOKEN CLAUDE_CODE_OAUTH_REFRESH_TOKEN CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR; do
    eval "present=\${$key+set}"
    printf 'LOGIN_%s=%s\n' "$key" "$present"
  done
  printf 'LOGIN_ARGS=%s\n' "$*"
  exit 0
fi
printf 'BARE_CONFIG=%s\n' "$CLAUDE_CONFIG_DIR"
printf 'BARE_SECURE_STORAGE=%s\n' "$CLAUDE_SECURESTORAGE_CONFIG_DIR"
exit 0
"#,
    );
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
        .stdout(predicate::str::contains(format!(
            "  alt — {} (logged in) (active)\n",
            alt.display()
        )))
        .stdout(predicate::str::contains(format!(
            "  main — {} (not logged in)\n",
            home.path().join(".claude").display()
        )))
        .stdout(predicate::str::contains(format!(
            "  work — {} (not logged in)\n",
            home.path().join(".claude-work").display()
        )));
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
        .stderr(predicate::str::contains("Choose Claude account").not())
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

    let mut cmd = cas_cmd(home.path());
    for key in CREDENTIAL_OVERRIDES {
        cmd.env(key, "fixture-override");
    }
    let assertion = cmd
        .args(["claude", "login", "alt", "--email", "alt@example.com"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "LOGIN_CONFIG={}\n",
            home.path().join(".claude-alt").display()
        )))
        .stdout(predicate::str::contains(format!(
            "LOGIN_SECURE_STORAGE={}\n",
            home.path().join(".claude-alt").display()
        )))
        .stdout(predicate::str::contains(
            "LOGIN_ARGS=auth login --email alt@example.com",
        ));
    for key in CREDENTIAL_OVERRIDES {
        assert!(
            String::from_utf8_lossy(&assertion.get_output().stdout)
                .lines()
                .any(|line| line == format!("LOGIN_{key}=")),
            "{key} must be unset in the login child"
        );
    }
}

#[test]
fn login_subcommand_keeps_main_on_legacy_default_credential_store() {
    let home = home_with_profiles();

    let mut cmd = cas_cmd(home.path());
    cmd.env("CLAUDE_CONFIG_DIR", home.path().join(".claude-alt"))
        .env(
            "CLAUDE_SECURESTORAGE_CONFIG_DIR",
            home.path().join(".claude-alt"),
        );
    for key in CREDENTIAL_OVERRIDES {
        cmd.env(key, "fixture-override");
    }
    let assertion = cmd
        .args(["claude", "login", "main"])
        .assert()
        .success()
        .stdout(predicate::str::contains("LOGIN_CONFIG=\n"))
        .stdout(predicate::str::contains("LOGIN_SECURE_STORAGE=\n"));
    for key in CREDENTIAL_OVERRIDES {
        assert!(
            String::from_utf8_lossy(&assertion.get_output().stdout)
                .lines()
                .any(|line| line == format!("LOGIN_{key}=")),
            "{key} must be unset for main too"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn bare_picker_on_a_real_tty_forwards_the_selected_profile_cas_fa64() {
    use cas_pty::{Pty, PtyConfig, PtyEvent};
    let home = home_with_profiles();
    let path = std::env::join_paths(std::iter::once(home.path().join("bin")).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    let mut pty = Pty::spawn(
        "claude-picker-probe",
        PtyConfig {
            command: cas::test_paths::cas_binary().to_string_lossy().into_owned(),
            args: vec!["claude".into(), "--bare".into()],
            cwd: Some(home.path().to_path_buf()),
            env: vec![
                ("HOME".into(), home.path().to_string_lossy().into_owned()),
                (
                    "XDG_CONFIG_HOME".into(),
                    home.path().join(".xdg").to_string_lossy().into_owned(),
                ),
                ("PATH".into(), path.to_string_lossy().into_owned()),
                ("TERM".into(), "xterm-256color".into()),
                ("CAS_SKIP_FACTORY_TOOLING".into(), "1".into()),
            ],
            env_remove: vec![
                "CAS_ROOT".into(),
                "CLAUDE_CONFIG_DIR".into(),
                "CLAUDE_SECURESTORAGE_CONFIG_DIR".into(),
            ],
            ..PtyConfig::default()
        },
    )
    .expect("spawn Cassy with terminal stdin/stdout");
    let mut output = Vec::new();
    let mut selected = false;
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            match pty.recv().await {
                Some(PtyEvent::Output(bytes)) => {
                    output.extend(bytes);
                    if !selected
                        && String::from_utf8_lossy(&output).contains("Choose Claude account")
                    {
                        // main starts selected; the next sorted account is alt.
                        pty.write(b"\x1b[B\r").await.unwrap();
                        selected = true;
                    }
                }
                Some(PtyEvent::Exited(code)) => return code,
                Some(PtyEvent::Error(error)) => panic!("picker PTY: {error}"),
                None => panic!("picker exited without status"),
            }
        }
    })
    .await;
    pty.kill_tree_force();
    let output = String::from_utf8_lossy(&output).replace('\r', "");
    assert_eq!(
        result.expect("picker did not finish after selection"),
        Some(0),
        "{output}"
    );
    assert!(selected, "bare launch never prompted: {output}");
    for label in ["BARE_CONFIG", "BARE_SECURE_STORAGE"] {
        assert!(
            output.contains(&format!(
                "{label}={}\n",
                home.path().join(".claude-alt").display()
            )),
            "wrong selected account for {label}: {output}"
        );
    }
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
