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
        .env_remove("CAS_CLONE_PATH")
        .env_remove("COLUMNS")
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

/// Configure a home dir with `~/.claude-alt` logged in and `~/.claude-work` not.
fn setup_home_with_profiles(home: &std::path::Path) {
    let alt = home.join(".claude-alt");
    std::fs::create_dir_all(&alt).unwrap();
    std::fs::create_dir_all(home.join(".claude-work")).unwrap();
    let bin = home.join("bin");
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
}

/// A temporary home dir with the normal Claude profile fixtures.
fn home_with_profiles() -> TempDir {
    let home = TempDir::new().unwrap();
    setup_home_with_profiles(home.path());
    home
}

#[test]
fn list_profiles_shows_detected_accounts_and_login_state() {
    let home = home_with_profiles();
    let alt = home.path().join(".claude-alt");

    std::fs::write(alt.join(".claude.json"), serde_json::json!({
        "oauthAccount": {"emailAddress": "operator@example.com", "accountUuid": "one", "organizationUuid": "org"}
    }).to_string()).unwrap();
    cas_cmd(home.path())
        .env("CLAUDE_CONFIG_DIR", &alt)
        .args(["claude", "--list-profiles"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: cas claude <profile>"))
        // pin: cas-e1f4 two-line rows that fit 80 columns.
        .stdout(predicate::str::contains(
            "  operator@example.com (logged in) (active)\n    ~/.claude-alt\n",
        ))
        .stdout(predicate::str::contains(
            "  main (not logged in)\n    ~/.claude\n",
        ))
        .stdout(predicate::str::contains(
            "  work (not logged in)\n    ~/.claude-work\n",
        ));
}

#[test]
fn accounts_list_json_includes_both_providers_flags_and_absolute_resets() {
    use base64::Engine as _;
    let home = home_with_profiles();
    let identity = serde_json::json!({"oauthAccount": {
        "emailAddress": "operator@example.com", "accountUuid": "one", "organizationUuid": "org"
    }})
    .to_string();
    for name in ["alt", "operator@example.com"] {
        let dir = home.path().join(format!(".claude-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".claude.json"), &identity).unwrap();
        // No OAuth token: this test never makes a network request.
        std::fs::write(dir.join(".credentials.json"), "{}").unwrap();
    }
    let future_reset = chrono::Utc::now().timestamp() + 86400;
    let codex = home.path().join(".codex-team");
    std::fs::create_dir_all(codex.join("sessions")).unwrap();
    let claims =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"email":"team@example.com"}"#);
    std::fs::write(
        codex.join("auth.json"),
        serde_json::json!({"tokens": {
            "id_token": format!("h.{claims}.s"), "account_id": "team"
        }})
        .to_string(),
    )
    .unwrap();
    std::fs::write(codex.join("sessions/rollout.jsonl"), serde_json::json!({"rate_limits": {
        "plan_type": "pro", "primary": {"window_minutes": 300, "used_percent": 12, "resets_at": future_reset},
        "secondary": {"window_minutes": 10080, "used_percent": 34, "resets_at": future_reset + 86400}
    }}).to_string()).unwrap();
    let canonical = home.path().join(".codex-team@example.com");
    std::fs::create_dir_all(canonical.join("sessions")).unwrap();
    std::fs::copy(codex.join("auth.json"), canonical.join("auth.json")).unwrap();
    std::fs::write(codex.join("sessions/rollout.jsonl"), serde_json::json!({"timestamp": "2026-01-01T00:00:00Z", "rate_limits": {
        "plan_type": "pro", "primary": {"window_minutes": 300, "used_percent": 12, "resets_at": future_reset}
    }}).to_string()).unwrap();
    std::fs::write(canonical.join("sessions/rollout.jsonl"), serde_json::json!({"timestamp": "2026-01-02T00:00:00Z", "rate_limits": {
        "plan_type": "pro", "primary": {"window_minutes": 300, "used_percent": 45, "resets_at": future_reset}
    }}).to_string()).unwrap();
    // Keep the login probe deterministic and local for this fixture home.
    cas::test_paths::warm_stub(
        &home.path().join("bin/codex"),
        "#!/bin/sh\necho 'Logged in using ChatGPT'\n",
    );
    let output = cas_cmd(home.path())
        .args(["accounts", "list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let rows = json["accounts"].as_array().unwrap();
    assert_eq!(rows.len(), 5);
    let row = |provider, name| {
        rows.iter()
            .find(|a| a["provider"] == provider && a["name"] == name)
            .unwrap()
    };
    let alt = row("claude", "alt");
    assert_eq!(alt["display_name"], "operator@example.com");
    assert!(
        alt["flags"]["duplicate_of"]
            .as_str()
            .unwrap()
            .ends_with(".claude-operator@example.com")
    );
    assert_eq!(row("claude", "work")["flags"]["never_logged_in"], true);
    assert_eq!(
        row("claude", "operator@example.com")["login_state"],
        "stale_login"
    );
    let team = row("codex", "team");
    assert_eq!(team["plan"], "pro");
    assert_eq!(team["windows"][0]["used_percent"], 45.0);
    assert_eq!(row("codex", "team@example.com")["windows"], team["windows"]);
    assert!(rows.iter().position(|a| a["name"] == "team@example.com").unwrap()
        < rows.iter().position(|a| a["name"] == "team").unwrap());
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(team["windows"][0]["resets_at"].as_str().unwrap())
            .unwrap()
            .timestamp(),
        future_reset
    );
    assert!(alt["windows"].as_array().unwrap().is_empty());
    assert!(!String::from_utf8(output).unwrap().contains("id_token"));
    let human = cas_cmd(home.path())
        .args(["accounts", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let human = String::from_utf8(human).unwrap();
    assert!(human.contains("operator@example.com"));
    assert!(human.contains("usage unknown"));
    assert!(human.contains("stale login"));
    assert!(human.lines().all(|line| line.len() <= 80));
}

#[test]
fn account_launch_resolves_display_emails_and_refuses_ambiguous_or_unknown_names() {
    use base64::Engine as _;
    let home = home_with_profiles();
    let bin = home.path().join("bin");
    for provider in ["claude", "codex"] {
        cas::test_paths::warm_stub(&bin.join(provider), r#"#!/bin/sh
if [ "$1" = auth ] && [ "$2" = status ]; then echo '{"loggedIn":true}'; exit 0; fi
if [ "$1" = login ] && [ "$2" = status ]; then echo 'Logged in using ChatGPT'; exit 0; fi
printf 'SELECTED=%s\n' "${CLAUDE_CONFIG_DIR:-${CODEX_HOME:-main}}"
"#);
        for (alias, email) in [("main", "main@example.com"), ("alt", "shared@example.com"), ("work", "shared@example.com")] {
            let dir = home.path().join(if alias == "main" { format!(".{provider}") } else { format!(".{provider}-{alias}") });
            std::fs::create_dir_all(&dir).unwrap();
            if provider == "claude" {
                let json = serde_json::json!({"oauthAccount": {"emailAddress": email}}).to_string();
                std::fs::write(if alias == "main" { home.path().join(".claude.json") } else { dir.join(".claude.json") }, json).unwrap();
            } else {
                let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::json!({"email": email}).to_string());
                std::fs::write(dir.join("auth.json"), serde_json::json!({"tokens": {"id_token": format!("h.{claims}.s")}}).to_string()).unwrap();
            }
        }
        let command = |name: &str| {
            let mut cmd = cas_cmd(home.path());
            cmd.env_remove("CODEX_HOME").env_remove("CLAUDE_CONFIG_DIR").env_remove("CLAUDE_SECURESTORAGE_CONFIG_DIR")
                .args([provider, "--bare", name, "--version"]);
            cmd
        };
        command("main@example.com").assert().success().stdout(predicate::str::contains("SELECTED=main"));
        assert!(!home.path().join(format!(".{provider}-main@example.com")).exists());
        command("shared@example.com").assert().failure()
            .stderr(predicate::str::contains("ambiguous")).stderr(predicate::str::contains("alt"))
            .stderr(predicate::str::contains("work"));
        command("altt").assert().failure().stderr(predicate::str::contains("alt"))
            .stderr(predicate::str::contains(format!("cas {provider} login altt")));
        assert!(!home.path().join(format!(".{provider}-altt")).exists());
        // An exact existing folder alias wins even when its text is another login's email.
        let exact = home.path().join(format!(".{provider}-main@example.com"));
        std::fs::create_dir(&exact).unwrap();
        command("main@example.com").assert().success().stdout(predicate::str::contains(exact.to_string_lossy().as_ref()));
    }
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
    let temp = TempDir::new().unwrap();
    let home = temp.path().join(format!("login-home-{}", "x".repeat(48)));
    setup_home_with_profiles(&home);

    let mut cmd = cas_cmd(&home);
    for key in CREDENTIAL_OVERRIDES {
        cmd.env(key, "fixture-override");
    }
    let assertion = cmd
        .args(["claude", "login", "alt", "--email", "alt@example.com"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "LOGIN_CONFIG={}\n",
            home.join(".claude-alt").display()
        )))
        .stdout(predicate::str::contains(format!(
            "LOGIN_SECURE_STORAGE={}\n",
            home.join(".claude-alt").display()
        )))
        .stdout(predicate::str::contains(
            "LOGIN_ARGS=auth login --email alt@example.com",
        ));
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("Logging in Claude account config: ~/.claude-alt\n"),
        "login announcement should use a recoverable HOME-relative path; got {stderr:?}"
    );
    assert!(
        stderr.lines().all(|line| line.chars().count() <= 80),
        "login announcement should fit 80 columns; got {stderr:?}"
    );
    for key in CREDENTIAL_OVERRIDES {
        assert!(
            String::from_utf8_lossy(&assertion.get_output().stdout)
                .lines()
                .any(|line| line == format!("LOGIN_{key}=")),
            "{key} must be unset in the login child"
        );
    }
}

/// cas-e1f4 QA F07: once accounts share a base folder, `cas claude login
/// <new>` links the new account dir to it, as the picker does; its login
/// stays its own.
#[test]
fn login_subcommand_links_a_new_account_to_the_shared_base() {
    let home = home_with_profiles();
    let base = home.path().join(".cas/claude-base");
    std::fs::create_dir_all(base.join("commands")).unwrap();
    std::fs::write(base.join("commands/finish.md"), "F").unwrap();
    std::fs::write(base.join("settings.json"), "{}").unwrap();

    cas_cmd(home.path())
        .args(["claude", "login", "newbie@x.com"])
        .assert()
        .success();
    let account = home.path().join(".claude-newbie@x.com");
    assert_eq!(
        std::fs::read_link(account.join("commands")).unwrap(),
        base.join("commands")
    );
    assert_eq!(
        std::fs::read_to_string(account.join("commands/finish.md")).unwrap(),
        "F"
    );
    assert_eq!(
        std::fs::read_link(account.join("settings.json")).unwrap(),
        base.join("settings.json")
    );
    assert!(
        !account.join(".credentials.json").exists(),
        "no login is shared"
    );
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

#[test]
fn accounts_lifecycle_preserves_shared_targets_and_guards_mutations() {
    for provider in ["claude", "codex"] {
        let home = TempDir::new().unwrap();
        let folder = home.path().join(format!(".{provider}-typo@example.com"));
        let base = home.path().join(format!(".cas/{provider}-base"));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("keep"), "shared").unwrap();
        std::os::unix::fs::symlink(&base, folder.join("shared")).unwrap();
        let key = format!("{provider}:typo@example.com");
        cas_cmd(home.path())
            .current_dir(home.path())
            .args(["accounts", "disable", &key])
            .assert()
            .success();
        let output = cas_cmd(home.path())
            .current_dir(home.path())
            .args(["accounts", "list", "--json"])
            .output()
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["accounts"][0]["flags"]["disabled"], true);
        cas_cmd(home.path())
            .current_dir(home.path())
            .args(["accounts", "enable", &key])
            .assert()
            .success();
        let output = cas_cmd(home.path())
            .current_dir(home.path())
            .args(["accounts", "list", "--json"])
            .output()
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["accounts"][0]["flags"]["disabled"], false);
        cas_cmd(home.path())
            .current_dir(home.path())
            .args(["accounts", "disable", &key])
            .assert()
            .success();
        let collision = home.path().join(format!(".{provider}-taken"));
        std::fs::create_dir(&collision).unwrap();
        cas_cmd(home.path())
            .current_dir(home.path())
            .args(["accounts", "rename", &key, "taken"])
            .assert()
            .failure();
        cas_cmd(home.path())
            .current_dir(home.path())
            .args(["accounts", "rename", &key, "fixed@example.com"])
            .assert()
            .success();
        assert!(!folder.exists());
        let output = cas_cmd(home.path())
            .current_dir(home.path())
            .args(["accounts", "list", "--json"])
            .output()
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            value["accounts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["name"] == "fixed@example.com" && a["flags"]["disabled"] == true)
        );
        let key = format!("{provider}:fixed@example.com");
        let moved = home.path().join(format!(".{provider}-fixed@example.com"));
        let selector = if provider == "claude" {
            "CLAUDE_CONFIG_DIR"
        } else {
            "CODEX_HOME"
        };
        cas_cmd(home.path())
            .current_dir(home.path())
            .env(selector, &moved)
            .args(["accounts", "remove", &key, "--yes"])
            .assert()
            .failure();
        assert!(moved.exists());
        {
            use std::io::{BufRead, Write};
            use std::process::Stdio;
            let mut holder = std::process::Command::new("/bin/sh")
                .args(["-c", "printf 'ready\\n'; read answer"])
                .env(selector, &moved)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut ready = String::new();
            std::io::BufReader::new(holder.stdout.take().unwrap())
                .read_line(&mut ready)
                .unwrap();
            assert_eq!(ready.trim(), "ready");
            cas_cmd(home.path())
                .current_dir(home.path())
                .env_remove(selector)
                .args(["accounts", "rename", &key, "busy-rename"])
                .assert()
                .failure();
            cas_cmd(home.path())
                .current_dir(home.path())
                .env_remove(selector)
                .args(["accounts", "remove", &key, "--yes"])
                .assert()
                .failure();
            holder.stdin.take().unwrap().write_all(b"\n").unwrap();
            assert!(holder.wait().unwrap().success());
        }
        cas_cmd(home.path())
            .current_dir(home.path())
            .env_remove(selector)
            .args(["accounts", "remove", &key])
            .write_stdin("n\n")
            .assert()
            .success();
        assert!(moved.exists());
        cas_cmd(home.path())
            .current_dir(home.path())
            .env_remove(selector)
            .args(["accounts", "remove", &key, "--yes"])
            .assert()
            .success();
        assert!(!moved.exists());
        assert_eq!(
            std::fs::read_to_string(base.join("keep")).unwrap(),
            "shared"
        );
        std::fs::create_dir(&moved).unwrap();
        let output = cas_cmd(home.path())
            .current_dir(home.path())
            .args(["accounts", "list", "--json"])
            .output()
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            value["accounts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["name"] == "fixed@example.com" && a["flags"]["disabled"] == false)
        );
        std::fs::create_dir(home.path().join(format!(".{provider}"))).unwrap();
        for name in ["main", "unknown"] {
            cas_cmd(home.path())
                .current_dir(home.path())
                .args(["accounts", "remove", &format!("{provider}:{name}"), "--yes"])
                .assert()
                .failure();
        }
    }
}

#[test]
fn new_login_is_atomic_for_both_providers_and_reports_existing_email() {
    use base64::Engine as _;
    for provider in ["claude", "codex"] {
        let home = TempDir::new().unwrap();
        let bin = home.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"email":"real@example.com"}"#);
        let script = format!(
            r#"#!/bin/sh
if [ "$1" = auth ] && [ "$2" = status ]; then echo '{{"loggedIn":true}}'; exit 0; fi
if [ "$1" = login ] && [ "$2" = status ]; then echo 'Logged in using ChatGPT'; exit 0; fi
[ "$FAIL_LOGIN" = 1 ] && exit 1
dir="${{CLAUDE_CONFIG_DIR:-$CODEX_HOME}}"
if [ "{provider}" = claude ]; then
  printf '{{"oauthAccount":{{"emailAddress":"real@example.com","accountUuid":"%s","organizationUuid":"org"}}}}' "${{LOGIN_ACCOUNT_ID:-one}}" > "$dir/.claude.json"
else
  printf '{{"tokens":{{"id_token":"h.{token}.s","account_id":"%s"}}}}' "${{LOGIN_ACCOUNT_ID:-one}}" > "$dir/auth.json"
fi
"#
        );
        cas::test_paths::warm_stub(&bin.join(provider), &script);
        cas_cmd(home.path())
            .current_dir(home.path())
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .env("FAIL_LOGIN", "1")
            .args([provider, "login", "mistyped@example.com"])
            .assert()
            .failure();
        assert!(
            !home
                .path()
                .join(format!(".{provider}-mistyped@example.com"))
                .exists()
        );
        cas_cmd(home.path())
            .current_dir(home.path())
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .args([provider, "login", "mistyped@example.com"])
            .assert()
            .success();
        assert!(
            home.path()
                .join(format!(".{provider}-real@example.com"))
                .is_dir()
        );
        assert!(
            !home
                .path()
                .join(format!(".{provider}-mistyped@example.com"))
                .exists()
        );
        cas_cmd(home.path())
            .current_dir(home.path())
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .args([provider, "login", "another-typo"])
            .write_stdin("y\n")
            .assert()
            .success()
            .stderr(predicate::str::contains("already exists"));
        let names: Vec<_> = std::fs::read_dir(home.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(&format!(".{provider}-"))
            })
            .collect();
        assert_eq!(
            names.len(),
            1,
            "failed/duplicate logins must leave no scratch accounts"
        );
        cas_cmd(home.path())
            .current_dir(home.path())
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .env("LOGIN_ACCOUNT_ID", "two")
            .args([provider, "login", "another-subscription"])
            .write_stdin("y\n")
            .assert()
            .success()
            .stderr(predicate::str::contains("already exists").not());
        assert!(
            home.path()
                .join(format!(".{provider}-real@example.com-2"))
                .is_dir()
        );
    }
}
