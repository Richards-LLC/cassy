//! cassy#1157: `cas violet post|thread|read` from the real binary.
//!
//! The hub double lives in the unit tests (`cli::violet_cmd::tests`); these
//! cases pin what only the binary shows: the command is routed, local
//! mistakes fail before any network, and a failure exits non-zero with its
//! named code on stderr, or as one JSON document under `--json`.
#![cfg(feature = "mcp-proxy")]

use assert_cmd::Command;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

/// A `cas` with an empty home: no proxy registration, no credentials.
fn violet(root: &Path) -> Command {
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut command = Command::new(cas::test_paths::cas_binary());
    // Legacy credential names come from the compatibility manifest, never
    // spelled here.
    let legacy = cas_types::violet_compatibility::violet_compatibility();
    for (key, _) in std::env::vars_os() {
        let key_text = key.to_string_lossy();
        if key_text.starts_with("CAS_")
            || key_text.starts_with("VIOLET_")
            || key_text.starts_with("SLACK_")
            || key_text.starts_with(legacy.legacy_token_prefix.as_str())
            || key_text == legacy.legacy_bypass_env
        {
            command.env_remove(key);
        }
    }
    if let Some(host_home) = std::env::var_os("HOME") {
        command.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    command
        .current_dir(root)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("CODEX_HOME", home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", &home)
        .env("CAS_SKIP_FACTORY_TOOLING", "1")
        .timeout(Duration::from_secs(20));
    command.arg("violet");
    command
}

#[test]
fn violet_help_lists_post_thread_and_read() {
    let temp = TempDir::new().unwrap();
    let output = violet(temp.path())
        .arg("--help")
        .assert()
        .success()
        .get_output()
        .clone();
    let help = String::from_utf8(output.stdout).unwrap();
    for subcommand in ["post", "thread", "read"] {
        assert!(help.contains(subcommand), "{help}");
    }
}

#[test]
fn an_unregistered_machine_fails_with_not_configured() {
    let temp = TempDir::new().unwrap();
    let output = violet(temp.path())
        .args(["post", "--channel", "cas-internal", "--text", "hi"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("not_configured"), "{stderr}");
    assert!(stderr.contains("cas integrate violet"), "{stderr}");
    assert!(output.stdout.is_empty());
}

#[test]
fn json_failure_is_one_error_document_and_a_non_zero_exit() {
    let temp = TempDir::new().unwrap();
    let output = violet(temp.path())
        .args([
            "post",
            "--channel",
            "cas-internal",
            "--text",
            "hi",
            "--json",
        ])
        .assert()
        .failure()
        .get_output()
        .clone();
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout must be one complete JSON document");
    assert_eq!(report["ok"], false);
    assert_eq!(report["error"]["code"], "not_configured");
    assert_eq!(report["error"]["retryable"], false);
}

#[test]
fn a_missing_file_fails_locally_before_the_hub() {
    let temp = TempDir::new().unwrap();
    let output = violet(temp.path())
        .args(["post", "--channel", "cas-internal", "--file", "absent.png"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    // A path problem is reported as such, not masked by the missing
    // registration the hub connection would hit next.
    assert!(stderr.contains("local_request_failed"), "{stderr}");
    assert!(!stderr.contains("not_configured"), "{stderr}");
}

#[test]
fn a_channel_read_without_since_is_refused() {
    let temp = TempDir::new().unwrap();
    let output = violet(temp.path())
        .args(["read", "--channel", "cas-internal"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("invalid_input"), "{stderr}");
    assert!(stderr.contains("--since"), "{stderr}");
}
