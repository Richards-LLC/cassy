//! Integration + unit tests for the `cas codex` / `cas grok` /
//! `cas default` provider-shortcut feature (cas-7f2c, extended by EPIC cas-8888
//! Phase 3 / cas-964a for Grok).
//!
//! Coverage:
//! - CLI parse: `cas codex --help`, `cas grok --help`, `cas default --help`
//! - `cas default <provider>` round-trip: persists to config, confirmation printed
//! - `cas default <invalid>` → non-zero exit + useful error

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn cas_cmd() -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cas"));
    cmd.env_remove("CAS_ROOT");
    cmd.env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

// ── Help / parse-level tests ──────────────────────────────────────────────────

#[test]
fn test_codex_help_shows_shortcut_description() {
    cas_cmd()
        .args(["codex", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("supervisor"))
        .stdout(predicate::str::contains("--default"));
}

#[test]
fn test_grok_help_shows_shortcut_description() {
    cas_cmd()
        .args(["grok", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("supervisor"))
        .stdout(predicate::str::contains("--default"));
}

#[test]
fn test_default_help() {
    cas_cmd()
        .args(["default", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PROVIDER"))
        .stdout(predicate::str::contains("claude"))
        .stdout(predicate::str::contains("codex"))
        .stdout(predicate::str::contains("grok"));
}

#[test]
fn test_codex_accepts_factory_flags() {
    cas_cmd()
        .args(["codex", "--workers", "0", "--help"])
        .assert()
        .success();
}

#[test]
fn test_grok_accepts_factory_flags() {
    cas_cmd()
        .args(["grok", "--workers", "0", "--help"])
        .assert()
        .success();
}

// ── `cas default` round-trip ──────────────────────────────────────────────────

/// Helper: run `cas default <provider>` with HOME pointed at a TempDir
/// (so we don't mutate the real `~/.cas/config.toml`).  Returns the path
/// to the temp config file so callers can assert on its contents.
fn run_default_in_temp(provider: &str) -> (TempDir, std::path::PathBuf, String) {
    let temp = TempDir::new().unwrap();
    let cas_dir = temp.path().join(".cas");
    std::fs::create_dir_all(&cas_dir).unwrap();

    let output = cas_cmd()
        .env("HOME", temp.path())
        .args(["default", provider])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "cas default {provider} failed: {}\n{}",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );

    let config_path = cas_dir.join("config.toml");
    (temp, config_path, stdout)
}

#[test]
fn test_default_codex_persists_to_config() {
    let (_temp, config_path, stdout) = run_default_in_temp("codex");

    assert!(
        stdout.contains("supervisor default set to codex"),
        "Expected confirmation line, got: {stdout}"
    );

    assert!(config_path.exists(), "config.toml was not created");
    let content = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        content.contains(r#"harness = "codex""#),
        "harness = \"codex\" not found in config:\n{content}"
    );
}

#[test]
fn test_default_claude_persists_to_config() {
    let (_temp, config_path, stdout) = run_default_in_temp("claude");

    assert!(
        stdout.contains("supervisor default set to claude"),
        "Expected confirmation line, got: {stdout}"
    );

    assert!(config_path.exists(), "config.toml was not created");
    let content = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        content.contains(r#"harness = "claude""#),
        "harness = \"claude\" not found in config:\n{content}"
    );
}

#[test]
fn test_default_grok_persists_to_config() {
    let (_temp, config_path, stdout) = run_default_in_temp("grok");

    assert!(
        stdout.contains("supervisor default set to grok"),
        "Expected confirmation line, got: {stdout}"
    );

    assert!(config_path.exists(), "config.toml was not created");
    let content = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        content.contains(r#"harness = "grok""#),
        "harness = \"grok\" not found in config:\n{content}"
    );
}

#[test]
fn test_default_invalid_provider_fails() {
    let temp = TempDir::new().unwrap();
    std::fs::create_dir_all(temp.path().join(".cas")).unwrap();

    cas_cmd()
        .env("HOME", temp.path())
        .args(["default", "openai"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Unknown provider"));
}

#[test]
fn test_default_preserves_existing_config_keys() {
    let temp = TempDir::new().unwrap();
    let cas_dir = temp.path().join(".cas");
    std::fs::create_dir_all(&cas_dir).unwrap();

    // Pre-seed a config with some other settings.
    let seed = r#"
[sync]
min_helpful = 7

[llm.worker]
model = "claude-sonnet-4-6"
"#;
    std::fs::write(cas_dir.join("config.toml"), seed).unwrap();

    cas_cmd()
        .env("HOME", temp.path())
        .args(["default", "codex"])
        .assert()
        .success();

    let content = std::fs::read_to_string(cas_dir.join("config.toml")).unwrap();
    assert!(
        content.contains("min_helpful = 7"),
        "sync.min_helpful was clobbered:\n{content}"
    );
    assert!(
        content.contains(r#"model = "claude-sonnet-4-6""#),
        "llm.worker.model was clobbered:\n{content}"
    );
    assert!(
        content.contains(r#"harness = "codex""#),
        "harness = \"codex\" not written:\n{content}"
    );
}

/// Mirror: a plain `cas factory` (no explicit flag) SHOULD pick up the
/// persisted codex default from config.
#[test]
fn test_plain_factory_picks_up_persisted_default() {
    use cas::config::{Config, LlmConfig, LlmRoleConfig};
    use cas::cli::FactoryArgs;
    use tempfile::TempDir;

    let cas_dir = TempDir::new().unwrap();
    let mut project_cfg = Config::default();
    project_cfg.llm = Some(LlmConfig {
        harness: Some("claude".into()),
        model: None,
        reasoning_effort: None,
        supervisor: Some(LlmRoleConfig {
            harness: Some("codex".into()),
            model: None,
            reasoning_effort: None,
        }),
        worker: None,
    });
    project_cfg.save(cas_dir.path()).unwrap();

    // Default FactoryArgs — no explicit flag.
    let args = FactoryArgs::default(); // supervisor_cli_explicit = false

    let mut effective = args.clone();
    if let Ok(cfg) = Config::load(cas_dir.path()) {
        let llm = cfg.llm();
        if !effective.supervisor_cli_explicit {
            effective.supervisor_cli = llm.harness_for_role("supervisor").to_string();
        }
    }

    assert_eq!(
        effective.supervisor_cli, "codex",
        "plain `cas factory` should pick up persisted codex default; got {}",
        effective.supervisor_cli
    );
}
