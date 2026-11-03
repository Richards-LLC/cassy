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
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

/// Configure a home dir with `~/.claude-alt` logged in and `~/.claude-work` not.
fn setup_home_with_profiles(home: &std::path::Path) {
    std::fs::create_dir_all(home.join(".claude")).unwrap();
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
fn generated_statusline_records_usage_even_for_a_factory_supervisor() {
    let home = home_with_profiles();
    let project = TempDir::new().unwrap();
    cas_cmd(home.path()).current_dir(project.path())
        .env_remove("CAS_AGENT_ROLE")
        .args(["init", "--yes", "--no-integrations", "--json"])
        .assert().success();
    let settings: serde_json::Value = serde_json::from_slice(
        &std::fs::read(project.path().join(".claude/settings.json")).unwrap()).unwrap();
    let command = settings["statusLine"]["command"].as_str().unwrap();
    let args = command.strip_prefix("cas ").expect("generated Cassy command");
    for role in ["supervisor", "worker"] {
        let dir = home.path().join(format!(".claude-{role}"));
        std::fs::create_dir(&dir).unwrap();
        cas_cmd(home.path()).current_dir(project.path())
            .env("CLAUDE_CONFIG_DIR", &dir).env("CAS_AGENT_ROLE", role)
            .args(args.split_whitespace())
            .write_stdin(serde_json::json!({"rate_limits": {
                "seven_day": {"used_percentage": 38, "resets_at": chrono::Utc::now().timestamp() + 86400}
            }}).to_string())
            .assert().success();
        let snapshot: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.join(".cas-claude-usage.json")).expect("generated command must record the native snapshot")).unwrap();
        assert_eq!(snapshot["windows"][0]["used_percent"].as_f64(), Some(38.0));
        // Idle profiles retain their observation until the window resets.
        let mut old = snapshot;
        old["observed_at"] = (chrono::Utc::now() - chrono::Duration::hours(3)).to_rfc3339().into();
        std::fs::write(dir.join(".cas-claude-usage.json"), old.to_string()).unwrap();
    }
    cas_cmd(home.path()).args(["claude", "--list-profiles"])
        .assert().success()
        .stdout(predicate::str::contains("weekly 38%").count(2))
        .stdout(predicate::str::contains("as of ").count(2))
        .stdout(predicate::str::contains(" ET"));
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
            "  operator@example.com (alt) (logged in) (active)\n    ~/.claude-alt\n",
        ))
        .stdout(predicate::str::contains(
            "  main (not logged in)\n    ~/.claude\n",
        ))
        .stdout(predicate::str::contains(
            "  work (not logged in)\n    ~/.claude-work\n",
        ))
        .stdout(predicate::str::contains("Full profile names and folders:").not());

    for alias in [
        format!("{}@example.com", "long-profile-".repeat(5)),
        "界".repeat(50),
        "🚀".repeat(40),
    ] {
        let directory = home.path().join(format!(".claude-{alias}"));
        std::fs::create_dir(&directory).unwrap();
        let email = format!("{alias}@example.org");
        std::fs::write(directory.join(".claude.json"), serde_json::json!({
            "oauthAccount": {"emailAddress": email, "accountUuid": alias, "organizationUuid": "long-profile"}
        }).to_string()).unwrap();
        let output = cas_cmd(home.path())
            .env("COLUMNS", "80")
            .env("CLAUDE_CONFIG_DIR", &directory)
            .args(["claude", "--list-profiles"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let output = String::from_utf8(output).unwrap();
        for line in output.lines() {
            let width = ratatui::text::Line::from(line).width();
            assert!(width <= 80, "profile list overflows 80 cells: {line}");
        }
        let header = output.lines().find(|line| line.contains("(active)")).unwrap();
        assert!(ratatui::text::Line::from(header).width() >= 79, "unused display cells: {header}");
        assert!(header.contains("(not logged in) (active)"), "{output}");
        assert!(output.contains("--full"), "truncated identity needs an escape: {output}");
        for flag in ["--full", "--json"] {
            cas_cmd(home.path())
                .env("CLAUDE_CONFIG_DIR", &directory)
                .args(["claude", "--list-profiles", flag])
                .assert()
                .success()
                .stdout(predicate::str::contains(&email))
                .stdout(predicate::str::contains(&alias))
                .stdout(predicate::str::contains(format!("~/.claude-{alias}")))
                .stdout(predicate::str::contains("Full profile names and folders:").not());
        }
        std::fs::remove_dir_all(&directory).unwrap();
    }
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
    let past = home.path().join(".codex-past");
    std::fs::create_dir_all(past.join("sessions")).unwrap();
    let mut past_auth: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(codex.join("auth.json")).unwrap()).unwrap();
    past_auth["tokens"]["account_id"] = "past".into();
    std::fs::write(past.join("auth.json"), past_auth.to_string()).unwrap();
    std::fs::write(past.join("sessions/rollout.jsonl"), serde_json::json!({"rate_limits": {
        "primary": {"window_minutes": 300, "used_percent": 99, "resets_at": future_reset - 172800}
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
    assert_eq!(rows.len(), 7);
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
    assert!(row("codex", "past")["windows"][0]["used_percent"].is_null());
    assert_eq!(team["email"], serde_json::Value::Null);
    assert_eq!(team["identity"], serde_json::Value::Null);
    assert_eq!(team["flags"]["duplicate_of"], serde_json::Value::Null);
    assert_eq!(team["windows"][0]["used_percent"], 12.0);
    assert_eq!(row("codex", "team@example.com")["windows"][0]["used_percent"], 45.0);
    assert!(
        rows.iter()
            .position(|a| a["name"] == "team@example.com")
            .unwrap()
            > rows.iter().position(|a| a["name"] == "team").unwrap()
    );
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
    assert!(human.contains("reset, usage unknown until next session"));
    assert!(human.lines().all(|line| line.len() <= 80));
}

#[test]
fn account_launch_resolves_display_emails_and_refuses_ambiguous_or_unknown_names() {
    use base64::Engine as _;
    let home = home_with_profiles();
    let bin = home.path().join("bin");
    for provider in ["claude", "codex"] {
        cas::test_paths::warm_stub(
            &bin.join(provider),
            r#"#!/bin/sh
if [ "$1" = auth ] && [ "$2" = status ]; then echo '{"loggedIn":true}'; exit 0; fi
if [ "$1" = login ] && [ "$2" = status ]; then echo 'Logged in using ChatGPT'; exit 0; fi
printf 'SELECTED=%s\n' "${CLAUDE_CONFIG_DIR:-${CODEX_HOME:-main}}"
"#,
        );
        for (alias, email) in [
            ("main", "main@example.com"),
            ("alt", "shared@example.com"),
            ("work", "shared@example.com"),
        ] {
            let dir = home.path().join(if alias == "main" {
                format!(".{provider}")
            } else {
                format!(".{provider}-{alias}")
            });
            std::fs::create_dir_all(&dir).unwrap();
            if provider == "claude" {
                let json = serde_json::json!({"oauthAccount": {"emailAddress": email}}).to_string();
                std::fs::write(
                    if alias == "main" {
                        home.path().join(".claude.json")
                    } else {
                        dir.join(".claude.json")
                    },
                    json,
                )
                .unwrap();
            } else {
                let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(serde_json::json!({"email": email}).to_string());
                std::fs::write(
                    dir.join("auth.json"),
                    serde_json::json!({"tokens": {"id_token": format!("h.{claims}.s")}})
                        .to_string(),
                )
                .unwrap();
            }
        }
        let command = |name: &str| {
            let mut cmd = cas_cmd(home.path());
            cmd.env_remove("CODEX_HOME")
                .env_remove("CLAUDE_CONFIG_DIR")
                .env_remove("CLAUDE_SECURESTORAGE_CONFIG_DIR")
                .args([provider, "--bare", name, "--version"]);
            cmd
        };
        let main_key = if provider == "claude" { "main@example.com" } else { "main" };
        command(main_key)
            .assert()
            .success()
            .stdout(predicate::str::contains("SELECTED=main"));
        cas_cmd(home.path())
            .env_remove("CODEX_HOME")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CLAUDE_SECURESTORAGE_CONFIG_DIR")
            .args([provider, main_key])
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "Factory mode requires an interactive terminal",
            ));
        assert!(
            !home
                .path()
                .join(format!(".{provider}-main@example.com"))
                .exists()
        );
        if provider == "claude" {
            command("shared@example.com")
                .assert()
                .failure()
                .stderr(predicate::str::contains("ambiguous"))
                .stderr(predicate::str::contains("alt"))
                .stderr(predicate::str::contains("work"));
        } else {
            for email in ["main@example.com", "shared@example.com"] {
                command(email).assert().failure()
                    .stderr(predicate::str::contains("unknown"));
            }
            command("alt").assert().success()
                .stdout(predicate::str::contains(".codex-alt"));
        }
        command("altt")
            .assert()
            .failure()
            .stderr(predicate::str::contains("alt"))
            .stderr(predicate::str::contains(format!(
                "cas {provider} login altt"
            )));
        assert!(!home.path().join(format!(".{provider}-altt")).exists());
        // An exact existing folder alias wins even when its text is another login's email.
        let exact = home.path().join(format!(".{provider}-main@example.com"));
        std::fs::create_dir(&exact).unwrap();
        command("main@example.com")
            .assert()
            .success()
            .stdout(predicate::str::contains(exact.to_string_lossy().as_ref()));
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

/// cas-05cee: a launch links the chosen account's empty shared slots before
/// the provider starts, and only warns about a real item in a slot.
#[test]
fn launch_links_empty_shared_slots_first_and_still_launches_on_a_real_item() {
    for provider in ["claude", "codex"] {
        let home = TempDir::new().unwrap();
        let h = home.path();
        std::fs::create_dir(h.join("bin")).unwrap();
        cas::test_paths::warm_stub(
            &h.join("bin").join(provider),
            r#"#!/bin/sh
if [ "$1" = auth ] && [ "$2" = status ]; then echo '{"loggedIn":true}'; exit 0; fi
if [ "$1" = login ] && [ "$2" = status ]; then echo 'Logged in using ChatGPT'; exit 0; fi
dir="${CLAUDE_CONFIG_DIR:-$CODEX_HOME}"
[ -L "$dir/skills" ] && echo "LAUNCHED_WITH_SKILLS $dir"
"#,
        );
        let base = h.join(format!(".cas/{provider}-base"));
        for item in ["skills", "agents"] {
            std::fs::create_dir_all(base.join(item)).unwrap();
        }
        let account = h.join(format!(".{provider}-w@x.com"));
        std::fs::create_dir_all(account.join("agents")).unwrap();
        let login = if provider == "claude" { ".credentials.json" } else { "auth.json" };
        let secret = b"{\"secret\":1}";
        std::fs::write(account.join(login), secret).unwrap();

        let output = cas_cmd(h)
            .env_remove("CODEX_HOME")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CLAUDE_SECURESTORAGE_CONFIG_DIR")
            .args([provider, "--bare", "w@x.com"])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stdout}{stderr}");
        assert!(stdout.contains("LAUNCHED_WITH_SKILLS"), "{provider}: linked before launch: {stdout}{stderr}");
        assert!(stderr.contains(&format!(
            "Linked 1 shared item in ~/.{provider}-w@x.com from ~/.cas/{provider}-base\n"
        )), "{stderr}");
        // pin: the warning names the item and the exact fix command (task AC).
        let warning = format!("Warning: 1 shared item in ~/.{provider}-w@x.com not linked to ~/.cas/{provider}-base: agents. Fix: cas doctor --fix --yes\n");
        assert!(stderr.contains(&warning), "{stderr}");
        assert!(account.join("agents").is_dir() && !account.join("agents").is_symlink());
        assert_eq!(std::fs::read(account.join(login)).unwrap(), secret, "credentials untouched");
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

/// Picker new-login on a real terminal: keys inquire read ahead of the
/// duplicate question are handed over without reading the terminal again, so
/// finished answers, EOF and Ctrl-C keep their order and meaning; everything
/// newer is the terminal's own input.
#[cfg(unix)]
#[tokio::test]
async fn picker_email_type_ahead_answers_the_duplicate_question_for_claude() {
    use cas_pty::{Pty, PtyConfig, PtyEvent};
    // Only Claude publishes a non-secret identity for the duplicate question.
    {
        let provider = "claude";
        let home = TempDir::new().unwrap();
        let bin = home.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        // Official-login stub: every login is the same real@example.com identity.
        cas::test_paths::warm_stub(
            &bin.join(provider),
            r#"#!/bin/sh
if [ "$1" = auth ] && [ "$2" = status ]; then echo '{"loggedIn":true}'; exit 0; fi
if [ "$1" = login ] && [ "$2" = status ]; then echo 'Logged in using ChatGPT'; exit 0; fi
if [ "$1" = auth ] || [ "$1" = login ]; then
  # The login child must inherit the terminal's native (canonical) mode.
  stty -a 2>/dev/null | grep -q -- -icanon && echo CHILD_RAW
  echo LOGIN_STARTED; [ -f "$HOME/pause-login" ] && sleep 1
fi
if [ "$1" = auth ] && [ -n "$CLAUDE_CONFIG_DIR" ]; then
  printf '{"oauthAccount":{"emailAddress":"real@example.com","accountUuid":"one"}}' > "$CLAUDE_CONFIG_DIR/.claude.json"
fi
if [ "$1" = login ] && [ -n "$CODEX_HOME" ]; then
  printf '{"tokens":{"id_token":"h.eyJlbWFpbCI6InJlYWxAZXhhbXBsZS5jb20ifQ.s","account_id":"one"}}' > "$CODEX_HOME/auth.json"
fi
exit 0
"#,
        );
        cas_cmd(home.path())
            .current_dir(home.path())
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .args([provider, "login", "first@example.com"])
            .assert()
            .success();
        let path = std::env::join_paths(std::iter::once(bin.clone()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        // (keys written with the email, output that triggers the next write,
        // keys written then, succeeds). Everything typed after the email's
        // Enter is the terminal's own input, in order, until the question.
        let cases: [(&[u8], &str, &[u8], bool); 10] = [
            (b"typed@example.com\ry\r", "", b"", true),
            // Chooser, email and answer in one write share one reader queue.
            (b"\x1b[F\rtyped@example.com\ry\r", "", b"", true),
            // Answers are consumed in order: the first one, `n`, wins.
            (b"typed@example.com\rn\ry\r", "", b"", false),
            (b"typed@example.com\ry\x15\r", "", b"", false),
            // An unfinished, never-displayed `n` is dropped; later keys are
            // the terminal's own, so Backspace then `y` answers yes and a
            // lone Enter during the login is an empty answer.
            (b"typed@example.com\rn", "[y/N] ", b"\x7fy\r", true),
            (b"typed@example.com\ry", "LOGIN_STARTED", b"\r", false),
            // EOF between the two readers, in every position, cancels.
            (b"typed@example.com\r\x04", "", b"", false),
            (b"typed@example.com\ry", "LOGIN_STARTED", b"\x04", false),
            (b"typed@example.com\ry", "[y/N] ", b"\x04", false),
            (b"typed@example.com\r\x03", "", b"", false),
        ];
        for (with_email, trigger, then, succeeds) in cases {
            let pause = home.path().join("pause-login");
            if trigger == "LOGIN_STARTED" {
                std::fs::write(&pause, "").unwrap();
            } else {
                let _ = std::fs::remove_file(&pause);
            }
            let mut pty = Pty::spawn(
                "picker-type-ahead",
                PtyConfig {
                    command: cas::test_paths::cas_binary().to_string_lossy().into_owned(),
                    args: vec![provider.into(), "--bare".into()],
                    cwd: Some(home.path().to_path_buf()),
                    env: vec![
                        ("HOME".into(), home.path().to_string_lossy().into_owned()),
                        ("XDG_CONFIG_HOME".into(), home.path().join(".xdg").to_string_lossy().into_owned()),
                        ("PATH".into(), path.to_string_lossy().into_owned()),
                        ("TERM".into(), "xterm-256color".into()),
                        ("CAS_SKIP_FACTORY_TOOLING".into(), "1".into()),
                    ],
                    env_remove: vec![
                        "CAS_ROOT".into(),
                        "CLAUDE_CONFIG_DIR".into(),
                        "CLAUDE_SECURESTORAGE_CONFIG_DIR".into(),
                        "CODEX_HOME".into(),
                    ],
                    ..PtyConfig::default()
                },
            )
            .expect("spawn Cassy with terminal stdin/stdout");
            let (mut output, mut step) = (Vec::new(), 0);
            let result = tokio::time::timeout(std::time::Duration::from_secs(15), async {
                loop {
                    match pty.recv().await {
                        Some(PtyEvent::Output(bytes)) => {
                            output.extend(bytes);
                            let text = String::from_utf8_lossy(&output).into_owned();
                            if step == 0 && text.contains("list-profiles") {
                                // End selects the last row: the new-login entry.
                                let whole = with_email.starts_with(b"\x1b[F");
                                pty.write(if whole { with_email } else { b"\x1b[F\r" }).await.unwrap();
                                step = if whole { 2 } else { 1 };
                            } else if step == 1 && text.contains("Email for the new") {
                                pty.write(with_email).await.unwrap();
                                step = 2;
                            } else if step == 2 && !trigger.is_empty() && text.contains(trigger) {
                                pty.write(then).await.unwrap();
                                step = 3;
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
            let case = format!("{} then {:?} at {trigger:?}", String::from_utf8_lossy(with_email), String::from_utf8_lossy(then));
            let Ok(code) = result else { panic!("{provider} {case:?}: still waiting for input: {output}") };
            assert!(!output.contains("CHILD_RAW"), "{provider} {case:?}: {output}");
            if succeeds {
                assert_eq!(code, Some(0), "{provider} {case:?}: {output}");
                assert!(output.contains("Keep the existing account? [y/N] "), "{provider} {case:?}: {output}");
            } else {
                assert_ne!(code, Some(0), "{provider} {case:?}: {output}");
            }
            let left: Vec<_> = std::fs::read_dir(home.path())
                .unwrap()
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with(&format!(".{provider}-")))
                .collect();
            assert_eq!(left.len(), 1, "{provider} {case:?}: one account, no staging: {left:?}");
        }
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
fn account_mutation_help_explains_macos_shell_visibility() {
    let home = TempDir::new().unwrap();
    for command in ["rename", "remove"] {
        cas_cmd(home.path())
            .args(["accounts", command, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("macOS"))
            .stdout(predicate::str::contains("/bin/zsh, /bin/bash and /bin/sh"))
            .stdout(predicate::str::contains("CLAUDE_CONFIG_DIR"))
            .stdout(predicate::str::contains("CODEX_HOME"))
            .stdout(predicate::str::contains("Close shells"));
    }
}

#[test]
fn accounts_lifecycle_preserves_shared_targets_and_guards_mutations() {
    const HOLDER: &str = "CAS_TEST_ACCOUNT_HOLDER";
    if std::env::var_os(HOLDER).is_some() {
        // Use stderr so libtest's stdout banner cannot race the ready signal.
        eprintln!("ready");
        std::io::stdin().read_line(&mut String::new()).unwrap();
        return;
    }
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
        let can_rename = !(cfg!(target_os = "macos") && provider == "claude");
        let mut rename = cas_cmd(home.path());
        rename.current_dir(home.path()).args(["accounts", "rename", &key, "fixed@example.com"]);
        let name = if can_rename {
            rename.assert().success();
            assert!(!folder.exists());
            "fixed@example.com"
        } else {
            rename.assert().failure().stderr(predicate::str::contains("cas claude login fixed@example.com"));
            assert!(folder.exists());
            "typo@example.com"
        };
        let output = cas_cmd(home.path())
            .current_dir(home.path())
            .args(["accounts", "list", "--json"])
            .output().unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(value["accounts"].as_array().unwrap().iter()
            .any(|a| a["name"] == name && a["flags"]["disabled"] == true));
        let key = format!("{provider}:{name}");
        let moved = home.path().join(format!(".{provider}-{name}"));
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
            let test_name = "accounts_lifecycle_preserves_shared_targets_and_guards_mutations";
            let child_test = module_path!()
                .split_once("::")
                .map(|(_, module)| format!("{module}::{test_name}"))
                .unwrap_or_else(|| test_name.to_string());
            // Keep an Apple shell alive around a readable child. Even when
            // macOS hides the shell's env, its descendant must protect the account.
            let mut holder = std::process::Command::new("/bin/sh")
                .args(["-c", "\"$@\"; exit $?", "account-holder"])
                .arg(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    &child_test,
                    "--nocapture",
                ])
                .env(HOLDER, "1")
                .env(selector, &moved)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut ready = String::new();
            std::io::BufReader::new(holder.stderr.take().unwrap())
                .read_line(&mut ready)
                .unwrap();
            assert_eq!(ready.trim(), "ready");
            if can_rename {
                cas_cmd(home.path())
                    .current_dir(home.path())
                    .env_remove(selector)
                    .args(["accounts", "rename", &key, "busy-rename"])
                    .assert()
                    .failure()
                    .stderr(predicate::str::contains("is in use"));
            }
            cas_cmd(home.path())
                .current_dir(home.path())
                .env_remove(selector)
                .args(["accounts", "remove", &key, "--yes"])
                .assert()
                .failure()
                .stderr(predicate::str::contains("is in use"));
            assert!(moved.exists(), "busy account must remain in place");
            holder.stdin.take().unwrap().write_all(b"\n").unwrap();
            assert!(holder.wait().unwrap().success());
        }
        if can_rename {
            for (from, to) in [(name, "busy-rename"), ("busy-rename", name)] {
                cas_cmd(home.path())
                    .current_dir(home.path())
                    .env_remove(selector)
                    .args(["accounts", "rename", &format!("{provider}:{from}"), to])
                    .assert()
                    .success();
            }
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
                .any(|a| a["name"] == name && a["flags"]["disabled"] == false)
        );
        std::fs::create_dir(home.path().join(format!(".{provider}"))).unwrap();
        for name in ["main", "unknown"] {
            cas_cmd(home.path())
                .current_dir(home.path())
                .args(["accounts", "remove", &format!("{provider}:{name}"), "--yes"])
                .assert()
                .failure();
        }
        // Lifecycle lookup must retain both aliases even though write scans
        // collapse their common physical target (legacy alt sorts first).
        let physical_home = TempDir::new().unwrap();
        let aliases_home = TempDir::new().unwrap();
        std::fs::remove_dir(aliases_home.path()).unwrap();
        std::os::unix::fs::symlink(physical_home.path(), aliases_home.path()).unwrap();
        let real = aliases_home.path().join(format!(".{provider}-work"));
        let link = aliases_home.path().join(format!(".{provider}-alt"));
        std::fs::create_dir(&real).unwrap();
        let private_bytes = b"private target";
        std::fs::write(real.join("keep"), private_bytes).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        for alias in ["alt", "work"] {
            let key = format!("{provider}:{alias}");
            for action in ["disable", "enable"] {
                cas_cmd(aliases_home.path())
                    .current_dir(aliases_home.path())
                    .args(["accounts", action, &key])
                    .assert()
                    .success();
                let output = cas_cmd(aliases_home.path())
                    .current_dir(aliases_home.path())
                    .args(["accounts", "list", "--json"])
                    .output()
                    .unwrap();
                let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                let rows = rows["accounts"].as_array().unwrap();
                assert_eq!(rows.len(), 2);
                for row in rows {
                    assert_eq!(
                        row["flags"]["disabled"],
                        action == "disable",
                        "{provider}/{alias}: {row}"
                    );
                }
            }
        }
        cas_cmd(aliases_home.path())
            .current_dir(aliases_home.path())
            .args(["accounts", "disable", &format!("{provider}:work")])
            .assert()
            .success();
        if can_rename {
            cas_cmd(aliases_home.path())
                .current_dir(aliases_home.path())
                .args(["accounts", "rename", &format!("{provider}:work"), "renamed"])
                .assert()
                .success();
            let renamed = aliases_home.path().join(format!(".{provider}-renamed"));
            assert!(renamed.join("keep").is_file());
            let output = cas_cmd(aliases_home.path())
                .current_dir(aliases_home.path())
                .args(["accounts", "list", "--json"])
                .output()
                .unwrap();
            let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            let moved = rows["accounts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["name"] == "renamed")
                .unwrap();
            assert_eq!(
                moved["flags"]["disabled"], true,
                "renaming through a symlinked HOME retains disabled state"
            );
            cas_cmd(aliases_home.path())
                .current_dir(aliases_home.path())
                .args(["accounts", "rename", &format!("{provider}:renamed"), "work"])
                .assert()
                .success();
        }
        cas_cmd(aliases_home.path())
            .current_dir(aliases_home.path())
            .args(["accounts", "disable", &format!("{provider}:alt")])
            .assert()
            .success();
        cas_cmd(aliases_home.path())
            .current_dir(aliases_home.path())
            .args(["accounts", "remove", &format!("{provider}:alt"), "--yes"])
            .assert()
            .success();
        assert!(!link.is_symlink());
        assert_eq!(std::fs::read(real.join("keep")).unwrap(), private_bytes);
        let output = cas_cmd(aliases_home.path())
            .current_dir(aliases_home.path())
            .args(["accounts", "list", "--json"])
            .output()
            .unwrap();
        let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(rows["accounts"][0]["flags"]["disabled"], true);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let second = aliases_home.path().join(format!(".{provider}-copy"));
        std::os::unix::fs::symlink(&real, &second).unwrap();
        cas_cmd(aliases_home.path())
            .current_dir(aliases_home.path())
            .args(["accounts", "disable", &format!("{provider}:alt")])
            .assert()
            .success();
        cas_cmd(aliases_home.path())
            .current_dir(aliases_home.path())
            .args(["accounts", "remove", &format!("{provider}:work")])
            .write_stdin("n\n")
            .assert()
            .success()
            .stderr(predicate::str::contains(link.to_str().unwrap()))
            .stderr(predicate::str::contains(second.to_str().unwrap()));
        assert_eq!(std::fs::read(real.join("keep")).unwrap(), private_bytes);
        assert!(link.is_symlink());
        cas_cmd(aliases_home.path())
            .current_dir(aliases_home.path())
            .args(["accounts", "remove", &format!("{provider}:work"), "--yes"])
            .assert()
            .success();
        assert!(!real.exists());
        assert!(
            !link.is_symlink(),
            "removing the account removes its aliases"
        );
        assert!(!second.is_symlink());
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let output = cas_cmd(aliases_home.path())
            .current_dir(aliases_home.path())
            .args(["accounts", "list", "--json"])
            .output()
            .unwrap();
        let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            rows["accounts"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["flags"]["disabled"] == false)
        );
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
[ "$FAIL_LOGIN" = 1 ] && {{ echo AUTH_RAN; exit 1; }}
dir="${{CLAUDE_CONFIG_DIR:-$CODEX_HOME}}"
if [ "{provider}" = claude ]; then
  printf '{{"oauthAccount":{{"emailAddress":"real@example.com","accountUuid":"%s","organizationUuid":"org"}}}}' "${{LOGIN_ACCOUNT_ID:-one}}" > "$dir/.claude.json"
else
  printf '{{"tokens":{{"id_token":"h.{token}.s","account_id":"%s"}}}}' "${{LOGIN_ACCOUNT_ID:-one}}" > "$dir/auth.json"
fi
"#
        );
        cas::test_paths::warm_stub(&bin.join(provider), &script);
        let stale = home.path().join(format!(".{provider}-login.tmp.stale"));
        let fresh = home.path().join(format!(".{provider}-login.tmp.fresh"));
        let linked = home.path().join(format!(".{provider}-login.tmp.link"));
        let outside = home.path().join("keep-target");
        for path in [&stale, &fresh, &outside] {
            std::fs::create_dir(path).unwrap();
        }
        std::fs::File::open(&stale)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new().set_modified(
                    std::time::SystemTime::now() - std::time::Duration::from_secs(7200),
                ),
            )
            .unwrap();
        std::os::unix::fs::symlink(&outside, &linked).unwrap();
        let active = home.path().join(format!(".{provider}-login.tmp.active"));
        std::fs::create_dir(&active).unwrap();
        let lease = std::fs::File::open(&active).unwrap();
        fs2::FileExt::lock_exclusive(&lease).unwrap();
        lease
            .set_times(
                std::fs::FileTimes::new().set_modified(
                    std::time::SystemTime::now() - std::time::Duration::from_secs(7200),
                ),
            )
            .unwrap();
        // Real filesystem refusal: the folder can be inspected and locked, but
        // its contents cannot be deleted. This fixture runs as the test user.
        use std::os::unix::fs::PermissionsExt;
        let blocked = home.path().join(format!(".{provider}-login.tmp.blocked"));
        std::fs::create_dir(&blocked).unwrap();
        std::fs::write(blocked.join("keep"), "fixture").unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o555)).unwrap();
        std::fs::File::open(&blocked)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new().set_modified(
                    std::time::SystemTime::now() - std::time::Duration::from_secs(7200),
                ),
            )
            .unwrap();
        let output = cas_cmd(home.path())
            .current_dir(home.path())
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .env("FAIL_LOGIN", "1")
            .args([provider, "login", "mistyped@example.com"])
            .output()
            .unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(blocked.join("keep").is_file());
        std::fs::remove_dir_all(&blocked).unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("AUTH_RAN"),
            "{provider}: sweep must not block auth: {stderr}"
        );
        assert!(
            stderr.contains(&blocked.display().to_string()) && stderr.contains("Permission denied"),
            "{stderr}"
        );
        assert_eq!(
            stderr.matches("Warning:").count(),
            1,
            "one warning per failed folder: {stderr}"
        );
        assert!(
            !stale.exists(),
            "next login must sweep old temporary folders"
        );
        assert!(fresh.is_dir(), "fresh temporary folders must be kept");
        assert!(
            linked.is_symlink() && outside.is_dir(),
            "never traverse sweep symlinks"
        );
        assert!(active.is_dir(), "old active auth folders must be kept");
        drop(lease);
        std::fs::remove_dir(&active).unwrap();
        std::fs::remove_dir(&fresh).unwrap();
        std::fs::remove_file(&linked).unwrap();
        assert!(
            !home
                .path()
                .join(format!(".{provider}-mistyped@example.com"))
                .exists()
        );
        let stable_path = cfg!(target_os = "macos") && provider == "claude";
        cas_cmd(home.path())
            .current_dir(home.path())
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .args([provider, "login", "mistyped@example.com"])
            .assert().success();
        let unknown_identity = provider == "codex";
        let expected_name = if stable_path || unknown_identity { "mistyped@example.com" } else { "real@example.com" };
        assert!(home.path().join(format!(".{provider}-{expected_name}")).is_dir());
        if !stable_path && !unknown_identity {
            assert!(!home.path().join(format!(".{provider}-mistyped@example.com")).exists());
        }
        let duplicate = cas_cmd(home.path())
            .current_dir(home.path())
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .args([provider, "login", "another-typo"])
            .write_stdin("y\n")
            .assert()
            .success();
        if unknown_identity {
            duplicate.stderr(predicate::str::contains("already exists").not());
            assert!(home.path().join(".codex-another-typo").is_dir());
        } else {
            duplicate.stderr(predicate::str::contains("already exists"));
        }
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
            if unknown_identity { 2 } else { 1 },
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
        let second_name = if stable_path || unknown_identity { "another-subscription" } else { "real@example.com-2" };
        assert!(home.path().join(format!(".{provider}-{second_name}")).is_dir());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn interrupted_login_cleans_temporary_account_for_both_providers() {
    use cas_pty::{Pty, PtyConfig, PtyEvent};
    for provider in ["claude", "codex"] {
        // Ctrl-C goes to the foreground group; service shutdown may target only cas.
        for (signal, group) in [(libc::SIGINT, true), (libc::SIGTERM, false)] {
            let home = TempDir::new().unwrap();
            let bin = home.path().join("bin");
            std::fs::create_dir(&bin).unwrap();
            cas::test_paths::warm_stub(
                &bin.join(provider),
                "#!/bin/sh\nprintf 'AUTH_READY\\n'\nexec /bin/cat\n",
            );
            let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            )))
            .unwrap();
            let mut pty = Pty::spawn(
                "interrupted-account-login",
                PtyConfig {
                    command: cas::test_paths::cas_binary().to_string_lossy().into_owned(),
                    args: vec![
                        provider.into(),
                        "login".into(),
                        "cancelled@example.com".into(),
                    ],
                    cwd: Some(home.path().into()),
                    env: vec![
                        ("HOME".into(), home.path().to_string_lossy().into_owned()),
                        (
                            "XDG_CONFIG_HOME".into(),
                            home.path().join(".xdg").to_string_lossy().into_owned(),
                        ),
                        ("PATH".into(), path.to_string_lossy().into_owned()),
                        ("CAS_SKIP_FACTORY_TOOLING".into(), "1".into()),
                    ],
                    env_remove: vec![
                        "CAS_ROOT".into(),
                        "CAS_CLONE_PATH".into(),
                        "CLAUDE_CONFIG_DIR".into(),
                        "CLAUDE_SECURESTORAGE_CONFIG_DIR".into(),
                        "CODEX_HOME".into(),
                    ],
                    ..PtyConfig::default()
                },
            )
            .unwrap();
            let pid = pty.process_group_id().unwrap() as i32;
            let mut output = Vec::new();
            let mut interrupted = false;
            let result = tokio::time::timeout(std::time::Duration::from_secs(15), async {
                loop {
                    match pty.recv().await {
                        Some(PtyEvent::Output(bytes)) => {
                            output.extend(bytes);
                            if !interrupted
                                && String::from_utf8_lossy(&output).contains("AUTH_READY")
                            {
                                assert_eq!(
                                    unsafe { libc::kill(if group { -pid } else { pid }, signal) },
                                    0
                                );
                                interrupted = true;
                            }
                        }
                        Some(PtyEvent::Exited(code)) => return code,
                        Some(PtyEvent::Error(e)) => panic!("login PTY: {e}"),
                        None => panic!("login exited without status"),
                    }
                }
            })
            .await;
            pty.kill_tree_force();
            assert!(
                result.is_ok() && interrupted,
                "{provider} {signal}: {}",
                String::from_utf8_lossy(&output)
            );
            let leftovers: Vec<_> = std::fs::read_dir(home.path())
                .unwrap()
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with(&format!(".{provider}-"))
                })
                .map(|e| e.path())
                .collect();
            assert!(
                leftovers.is_empty(),
                "{provider} {signal}: interrupted login left {leftovers:?}"
            );
        }
    }
}
