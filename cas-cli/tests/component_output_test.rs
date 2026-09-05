//! Integration tests for CLI command output.
//!
//! Tests that commands produce correct output in piped mode (no TTY),
//! respect NO_COLOR, and produce clean snapshots.
//!
//! Includes PtyRunner-based tests that verify output in a real terminal.

use assert_cmd::Command;
use cas_tui_test::{PtyRunner, PtyRunnerConfig, WaitExt, screen, screen_with_size};
use predicates::prelude::*;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

fn cas_cmd(dir: &Path) -> Command {
    let mut cmd = Command::new(cas::test_paths::cas_binary());
    let home = dir.join(".test-home");
    let xdg = dir.join(".test-xdg-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    if let Some(host_home) = std::env::var_os("HOME") {
        cmd.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    cmd.env("HOME", home).env("XDG_CONFIG_HOME", xdg);
    // Pin the wrap width so snapshots do not depend on the host terminal or on
    // how long a redacted path happens to be before redaction.
    cmd.env("COLUMNS", "4000");
    cmd.env_remove("CAS_ROOT");
    // HOME is redirected above, but a harness account directory is selected by
    // its own variable and would otherwise point back at the host, making any
    // check that inspects user-level harness state (doctor's "user skills"
    // row) depend on whose machine ran the test.
    cmd.env_remove("CLAUDE_CONFIG_DIR");
    cmd.env_remove("CODEX_HOME");
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
    assert!(
        stdout.starts_with("Cassy\n"),
        "Piped help must fall back to the plain Cassy wordmark:\n{stdout}"
    );
    assert!(
        stdout.contains("Usage: cas"),
        "The command usage must remain `cas`:\n{stdout}"
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
    cas::test_paths::cas_binary().to_string_lossy().to_string()
}

fn pty_cas_in_dir(dir: &TempDir, args: &[&str]) -> PtyRunner {
    let bin = cas_bin_path();
    let home = dir.path().join(".test-home");
    let xdg = dir.path().join(".test-xdg-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    let mut config = PtyRunnerConfig::with_size(80, 24)
        .env("HOME", home.to_string_lossy())
        .env("COLUMNS", "4000")
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
    // Render tall enough to hold the whole report. The default 24-row screen
    // made this assertion depend on the report's LENGTH, not on whether doctor
    // rendered: the command echo sits on line 3, so the first check added to
    // doctor after the report reached 24 lines scrolls the echo off the top and
    // fails a test that is supposed to be about PTY rendering.
    let scr = screen_with_size(&output, 80, 200);
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
        .wait_for_text_timeout("Store", Duration::from_secs(10))
        .unwrap();

    let output = runner.get_output().as_str();
    // New migration-accounted doctor checks may extend the report beyond the
    // default 24-row terminal. This test verifies sections, not scrollback.
    let scr = screen_with_size(&output, 80, 200);

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
fn redact_temp_roots(s: &str) -> String {
    // Replace any configured temp root by value so hosts whose TMPDIR is not
    // under /tmp (or is very long) still redact to [TEMP_PATH].
    let mut result = s.to_string();
    for root in [std::env::temp_dir()]
        .into_iter()
        .chain(std::env::var_os("TMPDIR").map(std::path::PathBuf::from))
    {
        let root = root.to_string_lossy().trim_end_matches('/').to_string();
        if root.len() > 1 && !root.starts_with("/tmp") {
            result = result.replace(&root, "/tmp");
        }
    }
    result
}

fn redact_dynamic_values(s: &str) -> String {
    let s = &redact_temp_roots(s);
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

    // Git versions vary the no-repository diagnostic: newer releases add a
    // mount-boundary explanation on a second line. Doctor only needs to show
    // that this check could not inspect a non-repository temp fixture, so keep
    // the snapshot independent of the installed Git wording (cas-58be).
    let git_not_repo_re = regex::Regex::new(
        r"fatal:\s+not\s+a\s+git\s+repository\s+\((?:or\s+any\s+of\s+the\s+parent\s+directories\):\s+\.git|or\s+any\s+parent\s+up\s+to\s+mount\s+point\s+/[^)\s]*\)\s+Stopping\s+at\s+filesystem\s+boundary\s+\(GIT_DISCOVERY_ACROSS_FILESYSTEM\s+not\s+set\)\.)",
    )
    .unwrap();
    // The grouped doctor renderer wraps non-OK messages at terminal width
    // with a hanging indent, so the git wording may span lines; the pattern
    // above is whitespace-tolerant for that reason.
    result = git_not_repo_re
        .replace_all(&result, "[GIT_NOT_REPOSITORY]")
        .to_string();

    // Wrap positions of non-OK message continuation lines depend on the
    // redacted values' original lengths (temp paths differ per host), so join
    // hanging-indent continuations back onto their row before comparing.
    let continuation_re = regex::Regex::new(r"\n {20,}").unwrap();
    result = continuation_re.replace_all(&result, " ").to_string();

    // Redact the cloud canonical-id bucket name. When a project has no git
    // remote, `cas doctor` derives the bucket from the folder name — under test
    // that is the randomly-generated TempDir basename, so it must be redacted or
    // the snapshot is flaky (cas-f699 / GH #134 added this row).
    let bucket_re = regex::Regex::new(r"Cloud bucket `[^`]*`").unwrap();
    result = bucket_re
        .replace_all(&result, "Cloud bucket `[BUCKET]`")
        .to_string();

    // The header includes the canonical project id. Temp fixtures have a
    // generated folder id, so normalize that value independently of the cloud
    // bucket row.
    let project_re = regex::Regex::new(r"cas doctor · [^·\n]+ ·").unwrap();
    result = project_re
        .replace_all(&result, "cas doctor · [PROJECT] ·")
        .to_string();

    // Redact counts that follow "entries:", "tasks:", etc.
    let count_re = regex::Regex::new(r":\s+\d+\b").unwrap();
    result = count_re.replace_all(&result, ": [N]").to_string();

    result
}

#[test]
fn doctor_snapshot_redaction_normalizes_git_not_repository_diagnostics() {
    let prefix = "[WARN] code history index: cannot check code history index: not a git repository: [TEMP_PATH] (";
    let expected = format!("{prefix}[GIT_NOT_REPOSITORY])");
    for diagnostic in [
        "fatal: not a git repository (or any of the parent directories): .git",
        "fatal: not a git repository (or any parent up to mount point /)\nStopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).",
        "fatal: not a git repository (or any parent up to mount point /mnt)\nStopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).",
        "fatal: not a git repository (or any parent up to mount point /mnt/shockwave)\nStopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).",
    ] {
        assert_eq!(
            redact_dynamic_values(&format!("{prefix}{diagnostic})")),
            expected,
            "diagnostic must not make the doctor snapshot depend on Git version"
        );
    }

    let unrelated = "fatal: not a git repository (permission denied while reading mount metadata)";
    assert_eq!(
        redact_dynamic_values(&format!("{prefix}{unrelated})")),
        format!("{prefix}{unrelated})"),
        "unrelated git diagnostics must remain visible"
    );
}
