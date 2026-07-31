//! Integration tests for CLI command output.
//!
//! Tests that commands produce correct output in piped mode (no TTY),
//! respect NO_COLOR, and produce clean snapshots.
//!
//! Includes PtyRunner-based tests that verify output in a real terminal.

use assert_cmd::Command;
use cas_tui_test::{PtyRunner, PtyRunnerConfig, WaitExt, screen};
use predicates::prelude::*;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

fn cas_cmd(dir: &Path) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cas"));
    let home = dir.join(".test-home");
    let xdg = dir.join(".test-xdg-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    if let Some(host_home) = std::env::var_os("HOME") {
        cmd.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    cmd.env("HOME", home).env("XDG_CONFIG_HOME", xdg);
    cmd.env_remove("CAS_ROOT");
    cmd.env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

fn cas_in_dir(dir: &TempDir) -> Command {
    let mut cmd = cas_cmd(dir.path());
    cmd.current_dir(dir);
    cmd
}

fn init_cas(dir: &TempDir) {
    cas_cmd(dir.path())
        .current_dir(dir)
        .args(["init", "--yes"])
        .assert()
        .success();
}

// ============================================================================
// Piped output tests — stdout is not a TTY (assert_cmd captures it)
// ============================================================================

#[test]
fn doctor_piped_no_ansi() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    let output = cas_in_dir(&temp)
        .arg("doctor")
        .output()
        .expect("failed to run cas doctor");

    let stdout = String::from_utf8(output.stdout).unwrap();
    // Piped output should contain no ANSI escape sequences
    assert!(
        !stdout.contains('\x1b'),
        "Piped output contains ANSI escape codes:\n{stdout}"
    );
    // Should contain key doctor output
    assert!(
        stdout.contains("Doctor") || stdout.contains("doctor") || stdout.contains("Store"),
        "Doctor output missing expected content:\n{stdout}"
    );
}

#[test]
fn doctor_no_color_env() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    let output = cas_in_dir(&temp)
        .arg("doctor")
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run cas doctor");

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains('\x1b'),
        "NO_COLOR=1 output contains ANSI escape codes:\n{stdout}"
    );
}

#[test]
fn status_piped_no_ansi() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    let output = cas_in_dir(&temp)
        .arg("status")
        .output()
        .expect("failed to run cas status");

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains('\x1b'),
        "Piped status output contains ANSI escape codes:\n{stdout}"
    );
}

#[test]
fn status_no_color_env() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    let output = cas_in_dir(&temp)
        .arg("status")
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run cas status");

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains('\x1b'),
        "NO_COLOR=1 status output contains ANSI escape codes:\n{stdout}"
    );
}

// ============================================================================
// Content assertions for piped output
// ============================================================================

#[test]
fn doctor_piped_content() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    cas_in_dir(&temp)
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("Store").or(predicate::str::contains("store")));
}

#[test]
fn version_piped_no_ansi() {
    let temp = TempDir::new().unwrap();
    let output = cas_cmd(temp.path())
        .arg("--version")
        .output()
        .expect("failed to run cas --version");

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains('\x1b'),
        "Version output contains ANSI escape codes:\n{stdout}"
    );
    assert!(
        stdout.contains("cas"),
        "Version output missing 'cas': {stdout}"
    );
}

#[test]
fn help_piped_no_ansi() {
    let temp = TempDir::new().unwrap();
    let output = cas_cmd(temp.path())
        .arg("--help")
        .output()
        .expect("failed to run cas --help");

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains('\x1b'),
        "Help output contains ANSI escape codes:\n{stdout}"
    );
}

// ============================================================================
// Snapshot tests for CLI output
// ============================================================================

#[test]
fn doctor_snapshot() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    let output = cas_in_dir(&temp)
        .arg("doctor")
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run cas doctor");

    let stdout = String::from_utf8(output.stdout).unwrap();
    // Redact dynamic values (paths, timestamps, sizes)
    let redacted = redact_dynamic_values(&stdout);
    insta::assert_snapshot!(redacted);
}

#[test]
fn status_empty_snapshot() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    let output = cas_in_dir(&temp)
        .arg("status")
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run cas status");

    let stdout = String::from_utf8(output.stdout).unwrap();
    let redacted = redact_dynamic_values(&stdout);
    insta::assert_snapshot!(redacted);
}

// ============================================================================
// PtyRunner integration tests — real terminal (TTY) output
// ============================================================================

fn cas_bin_path() -> String {
    assert_cmd::cargo::cargo_bin!("cas")
        .to_string_lossy()
        .to_string()
}

fn pty_cas_in_dir(dir: &TempDir, args: &[&str]) -> PtyRunner {
    let bin = cas_bin_path();
    let home = dir.path().join(".test-home");
    let xdg = dir.path().join(".test-xdg-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    let mut config = PtyRunnerConfig::with_size(80, 24)
        .env("HOME", home.to_string_lossy())
        .env("XDG_CONFIG_HOME", xdg.to_string_lossy())
        .env("CAS_SKIP_FACTORY_TOOLING", "1")
        .env_remove("CAS_ROOT")
        .cwd(dir.path());
    if let Some(host_home) = std::env::var_os("HOME") {
        config = config.env("CAS_TEST_PROTECTED_HOME", host_home.to_string_lossy());
    }
    let mut runner = PtyRunner::with_config(config);
    runner.spawn(&bin, args).unwrap();
    runner
}

#[test]
fn pty_doctor_output() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    let mut runner = pty_cas_in_dir(&temp, &["doctor"]);

    // Wait for doctor output to appear
    let result = runner.wait_for_text_timeout("doctor", Duration::from_secs(10));
    assert!(result.is_ok(), "Should find 'doctor' in PTY output");

    let output = runner.get_output().as_str();
    let scr = screen(&output);
    scr.assert_contains("doctor").unwrap();
}

#[test]
fn pty_status_output() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    let mut runner = pty_cas_in_dir(&temp, &["status"]);

    // Wait for status output
    let result = runner.wait_for_text_timeout("cas", Duration::from_secs(10));
    assert!(result.is_ok(), "Should find 'cas' in PTY status output");

    let output = runner.get_output().as_str();
    let scr = screen(&output);
    scr.assert_contains("cas").unwrap();
}

#[test]
fn pty_doctor_has_expected_sections() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    let mut runner = pty_cas_in_dir(&temp, &["doctor"]);

    // Wait for the output to stabilize
    runner
        .wait_for_text_timeout("checks", Duration::from_secs(10))
        .unwrap();

    let output = runner.get_output().as_str();
    let scr = screen(&output);

    // Verify key sections are present
    scr.assert_contains("database").unwrap();
    scr.assert_contains("schema").unwrap();
}

// ============================================================================
// Redaction helpers for snapshot stability
// ============================================================================

/// Redact dynamic values from CLI output for stable snapshots.
///
/// Replaces file paths, timestamps, byte sizes, and other
/// machine-specific values with placeholders.
fn redact_dynamic_values(s: &str) -> String {
    let mut result = s.to_string();

    // Redact absolute paths (Unix-style)
    let path_re = regex::Regex::new(r"/[^\s:]+/\.cas/[^\s]+").unwrap();
    result = path_re.replace_all(&result, "[CAS_PATH]").to_string();

    // Redact absolute paths to temp dirs
    let tmp_re = regex::Regex::new(r"/(?:tmp|var/folders|private/var/folders)[^\s]+").unwrap();
    result = tmp_re.replace_all(&result, "[TEMP_PATH]").to_string();

    // Redact file sizes (e.g., "2.4 MB", "512 KB", "1234 bytes")
    let size_re = regex::Regex::new(r"\d+(?:\.\d+)?\s*(?:MB|KB|GB|bytes|B)\b").unwrap();
    result = size_re.replace_all(&result, "[SIZE]").to_string();

    // Redact ISO timestamps
    let ts_re = regex::Regex::new(r"\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}[^\s]*").unwrap();
    result = ts_re.replace_all(&result, "[TIMESTAMP]").to_string();

    // Redact durations (e.g., "15ms", "2.3s")
    let dur_re = regex::Regex::new(r"\d+(?:\.\d+)?(?:ms|µs|ns|s)\b").unwrap();
    result = dur_re.replace_all(&result, "[DURATION]").to_string();

    // Redact version numbers (e.g., "0.7.0")
    let ver_re = regex::Regex::new(r"\d+\.\d+\.\d+").unwrap();
    result = ver_re.replace_all(&result, "[VERSION]").to_string();

    // Redact counts that follow "entries:", "tasks:", etc.
    let count_re = regex::Regex::new(r":\s+\d+\b").unwrap();
    result = count_re.replace_all(&result, ": [N]").to_string();

    result
}
