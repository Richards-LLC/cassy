//! Integration tests for CLI command output.
//!
//! Tests that commands produce correct output in piped mode (no TTY),
//! respect NO_COLOR, and produce complete machine reports.
//!
//! Includes PtyRunner-based tests that verify output in a real terminal.

use assert_cmd::Command;
use cas_tui_test::{PtyRunner, PtyRunnerConfig, WaitExt, screen_with_size};
use predicates::prelude::*;
use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::TempDir;

#[derive(Clone, Copy, Debug)]
enum ReportKind {
    Doctor,
    Status,
    Version,
    Help,
    Plain,
}

// External report fixtures: doctor ends in its summary/verbose hint, while
// status reports the entry/rule/high-value counts on one complete row.
const COMPLETE_DOCTOR_REPORT: &str = r#"[{"name":"database","status":"ok","group":"store"},{"name":"schema","status":"ok","group":"store"}]"#;
const COMPLETE_STATUS_REPORT: &str = r#"{"entries":2,"rules":0,"proven_rules":0,"high_value":0}"#;
// Independently authored 80-column rendering of the joined receipt. The split
// through "full" is the shape observed in the assembly failure (cas-bca0).
const WRAPPED_DOCTOR_REPORT_80COL: &str = "Store  [OK] database  [OK] schema\n29 ok · 3 warnings · 0 errors · 498ms · cas doctor --verbose for timings and ful\nl messages\n";

// Verbatim captured output, kept alongside the independently authored fixtures.
// Doctor PTY: daemon nextest log 215ea2978962d1e4516bc8a8918d211fc8af3f3e7a7dcb2af54dc73ccbb7a408,
// pty_doctor_output (80x24 terminal; 200-row rendered screen), exit Success.
// Piped doctor/status/help/version: installed cas build ee98022 (2026-09-30),
// isolated init --yes project, COLUMNS=4000, NO_COLOR=1, all exit 0.
// Only version tokens use the package version; all other captured bytes,
// including temporary paths, timings and terminal wraps, remain verbatim.
const CAPTURED_DOCTOR_PTY_REPORT: &str = concat!(
    "doctor found safe automatic fixes; apply now? [y/N]\n[WARN] 3 warnings · 29 ok · .tmp1sqccl · ",
    env!("CARGO_PKG_VERSION"),
    "\n────────────────────────────────────────────────────────────────────────────────\nHost          [OK] registered project roots\n  [WARN] host  host: 3 findings — see `cas doctor --host`\nStore         [OK] cas directory  [OK] prompt hook  [OK] database  [OK] schema\n[OK] tables  [OK] entry store  [OK] memory stats  [OK] memory decay  [OK] rules\n [OK] tasks  [OK] cloud team-only\nIndexes       [OK] legacy search index  [OK] symbol index  [OK] embedding drain\n [OK] embeddings\n  [WARN] search index        Index not found at /tmp/.tmp1SQCcL/.cas/index/tanti\nvy-v15. Will be created on first search; Run a search to build it\n  [WARN] code history index  cannot check code history index: not a git reposito\nry: /tmp/.tmp1SQCcL (fatal: not a git repository (or any parent up to mount poin\nt /) Stopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).)\nCloud         [OK] supervisor relay  [OK] delivery retries  [OK] canonical id  [\nOK] cloud sync queue  [OK] cross-project rows\nConfig        [OK] SessionStart budget  [OK] configuration  [OK] issue repositor\nies  [OK] MCP stdio upstreams  [OK] MCP upstream reachability  [OK] sync target\n [OK] mcp config\nIntegrations  [OK] integrations\n\n29 ok · 3 warnings · 0 errors · 498ms · cas doctor --verbose for timings and ful\nl messages\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n"
);
const CAPTURED_DOCTOR_PIPED_REPORT: &str = concat!(
    "[WARN] 3 warnings · 29 ok · cas-bca0-capture-trsxxe38 · ",
    env!("CARGO_PKG_VERSION"),
    "\n────────────────────────────────────────────────────────────────────────────────\nHost          [OK] registered project roots\n  [WARN] host  host: 3 findings — see `cas doctor --host`\nStore         [OK] cas directory  [OK] prompt hook  [OK] database  [OK] schema  [OK] tables  [OK] entry store  [OK] memory stats  [OK] memory decay  [OK] rules  [OK] tasks  [OK] cloud team-only\nIndexes       [OK] legacy search index  [OK] symbol index  [OK] embedding drain  [OK] embeddings\n  [WARN] search index        Index not found at /tmp/cas-bca0-capture-trsxxe38/.cas/index/tantivy-v15. Will be created on first search; Run a search to build it\n  [WARN] code history index  cannot check code history index: not a git repository: /tmp/cas-bca0-capture-trsxxe38 (fatal: not a git repository (or any parent up to mount point /) Stopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).)\nCloud         [OK] supervisor relay  [OK] delivery retries  [OK] canonical id  [OK] cloud sync queue  [OK] cross-project rows\nConfig        [OK] SessionStart budget  [OK] configuration  [OK] issue repositories  [OK] MCP stdio upstreams  [OK] MCP upstream reachability  [OK] sync target  [OK] mcp config\nIntegrations  [OK] integrations\n\n29 ok · 3 warnings · 0 errors · 400ms · cas doctor --verbose for timings and full messages\n"
);
const CAPTURED_VERSION_REPORT: &str =
    concat!("cas ", env!("CARGO_PKG_VERSION"), " (ee98022 2026-09-30)\n");
const CAPTURED_STATUS_REPORT: &str = "cas: 0 entries, 0 rules (0 proven), 0 high-value\n";
const CAPTURED_HELP_REPORT: &str = "Cassy\n\nCassy — a multi-agent coding factory with persistent memory and task coordination\n\nUsage: cas [OPTIONS] [COMMAND]\n\nCommands:\n  open              Interactive project picker — scan ~/projects/, select, launch or attach\n  init              Initialize Cassy in current directory\n  setup             Guide a complete machine through installation, login, pairing, and a first project\n  attach            Attach to a running factory session\n  list              List running factory sessions\n  kill              Terminate a factory session\n  kill-all          Terminate all factory sessions\n  factory           Launch factory session (bare `cas` runs factory with defaults)\n  claude            Launch factory with Claude as the supervisor on a chosen account profile\n  codex             Launch factory with Codex as the supervisor on a chosen account profile\n  grok              Launch factory with Grok as the supervisor (shortcut for `cas factory --supervisor-cli=grok`)\n  default           Set the default supervisor provider without launching (persist only)\n  bridge            Local helper server for external orchestration tools\n  hub               Stable machine-local Commander hub\n  serve             Run the CAS MCP server\n  doctor            Run diagnostics\n  viktor            Show credential-safe provisioning status for the managed Viktor gateway\n  config            Manage configuration\n  status            Show session status\n  limits            Show local provider rate-limit and credit availability\n  status-line       Output a status line for agent integrations\n  hook              Handle Claude Code hook events\n  auth              Authentication commands (login, logout, whoami)\n  login             Log in to Cassy Cloud (shortcut for 'auth login')\n  logout            Log out (shortcut for 'auth logout')\n  whoami            Show current user (shortcut for 'auth whoami')\n  update            Update Cassy to the latest version\n  changelog         Show release notes and changelog from GitHub releases\n  release           Release lifecycle helpers\n  mcp               Manage upstream MCP servers\n  queue             Prompt queue operations (poll/ack for native extensions)\n  cloud             Sync data with Cassy Cloud\n  device            Manage registered devices\n  sync              Synchronize generated project files\n  claude-md         Evaluate and optimize CLAUDE.md files for token efficiency\n  codemap           Codemap staleness info and pending changes\n  history           Structural git-history index (backfill/status)\n  index             Build local search indexes on demand (`cas index code`)\n  artifact          Publish and inspect durable task artifacts (publish/show/list)\n  knowledge         Distilled project knowledge wiki (build/status/list)\n  memory-migrate    Migrate the legacy memory store into knowledge pages\n  project-overview  PRODUCT_OVERVIEW.md staleness info and pending changes\n  integrate         Auto-integrate the project with Vercel/Neon/GitHub (writes SKILL files)\n  memory            Share or unshare personal memories with your team (retroactive)\n  known-repos       Inspect and bootstrap the host-scoped known_repos registry\n  worktree          Worktree-scoped diagnostics and maintenance (sweep, ...)\n  sweep-all         Shortcut for `cas worktree sweep --all-repos`\n  help              Print this message or the help of the given subcommand(s)\n\nOptions:\n      --json     Output in JSON format\n      --full     Include full content in JSON output\n  -v, --verbose  Verbose output\n  -h, --help     Print help\n  -V, --version  Print version\n";

#[test]
fn pty_report_validation_accepts_captured_wrapped_doctor() {
    validate_report_text(CAPTURED_DOCTOR_PTY_REPORT, true, ReportKind::Plain, false)
        .expect("verbatim successful Doctor output from the 80-column PTY");
    let error = validate_report_text(CAPTURED_DOCTOR_PTY_REPORT, false, ReportKind::Plain, false)
        .expect_err("a real report cannot hide a failed child");

}

#[test]
#[cfg(unix)]
fn report_validation_accepts_captured_cli_reports() {
    for (kind, report) in [
        (ReportKind::Plain, CAPTURED_DOCTOR_PIPED_REPORT),
        (ReportKind::Plain, CAPTURED_STATUS_REPORT),
        (ReportKind::Version, CAPTURED_VERSION_REPORT),
        (ReportKind::Help, CAPTURED_HELP_REPORT),
    ] {
        let output = report_fixture(report, "0");
        let accepted = validate_piped_report(&output, kind).expect("verbatim real CLI report");
        assert_eq!(accepted, report);
    }
}

#[test]
#[cfg(unix)]
fn report_validation_rejects_failed_children_with_captured_reports() {
    for (kind, report) in [
        (ReportKind::Plain, CAPTURED_DOCTOR_PIPED_REPORT),
        (ReportKind::Plain, CAPTURED_STATUS_REPORT),
        (ReportKind::Version, CAPTURED_VERSION_REPORT),
        (ReportKind::Help, CAPTURED_HELP_REPORT),
    ] {
        let output = report_fixture(report, "23");
        let error = validate_piped_report(&output, kind).expect_err("failed real-report child");

    }
}

#[test]
fn report_validation_rejects_incomplete_machine_reports() {
    for (kind, report) in [
        (ReportKind::Version, "cas 9.9.9 (ee98022)\n"),
        (ReportKind::Version, "cas 9.9.9 (ee98022 2026-09-30\n"),
        (ReportKind::Version, "cas 9.9.9 (ee98022 2026-09-30) extra\n"),
        (ReportKind::Version, "$ cas --version (ee98022 2026-09-30)\n"),
        (ReportKind::Doctor, r#"[{"name":"database","status":"ok","group":"store"}"#),
        (ReportKind::Status, r#"{"entries":2,"rules":0,"proven_rules":0,"high_value":0"#),
    ] {
        validate_report_text(report, true, kind, true)
            .expect_err("incomplete machine report or command echo is not a successful report");
    }
}



#[test]
#[cfg(unix)]
fn pty_report_validation_accepts_real_80_column_wrap() {
    // Print one logical 90-column receipt into an actual 80-column PTY. The
    // screen parser must render the same split as the independent fixture.
    let report = "Store  [OK] database  [OK] schema\n29 ok · 3 warnings · 0 errors · 498ms · cas doctor --verbose for timings and full messages\n";
    let mut runner = PtyRunner::with_config(PtyRunnerConfig::with_size(80, 24));
    runner
        .spawn(
            "sh",
            &["-c", "printf '%s' \"$1\"", "report-fixture", report],
        )
        .unwrap();
    let captured = completed_pty_report(&mut runner, ReportKind::Plain)
        .expect("a successful report wrapped by the real PTY");
    assert_eq!(
        screen_with_size(&captured, 80, 200).text().trim_end(),
        WRAPPED_DOCTOR_REPORT_80COL.trim_end()
    );
}

#[cfg(unix)]
fn report_fixture(stdout: &str, exit_code: &str) -> std::process::Output {
    std::process::Command::new("sh")
        .args([
            "-c",
            "printf '%s' \"$1\"; exit \"$2\"",
            "report-fixture",
            stdout,
            exit_code,
        ])
        .output()
        .expect("run report fixture child")
}

#[test]
#[cfg(unix)]
fn report_validation_rejects_command_echo() {
    for (kind, echo) in [
        (ReportKind::Doctor, "$ cas doctor\n"),
        (ReportKind::Status, "$ cas status\n"),
        (ReportKind::Version, "$ cas --version\n"),
        (ReportKind::Help, "$ cas --help\n"),
    ] {
        let output = report_fixture(echo, "0");
        let error = validate_piped_report(&output, kind).expect_err("echo is not a report");

    }
}

#[test]
#[cfg(unix)]
fn report_validation_rejects_empty_success() {
    let output = report_fixture("", "0");
    for kind in [
        ReportKind::Doctor,
        ReportKind::Status,
        ReportKind::Version,
        ReportKind::Help,
    ] {
        let error =
            validate_piped_report(&output, kind).expect_err("empty success is not a report");

    }
}

#[test]
#[cfg(unix)]
fn report_validation_rejects_failed_children_with_completed_output() {
    for (kind, report) in [
        (ReportKind::Doctor, COMPLETE_DOCTOR_REPORT),
        (ReportKind::Status, COMPLETE_STATUS_REPORT),
        (ReportKind::Version, "cas 1.2.3\n"),
        (ReportKind::Help, "Cassy\nUsage: cas [OPTIONS] <COMMAND>\n"),
    ] {
        let output = report_fixture(report, "23");
        let error = validate_piped_report(&output, kind).expect_err("failed child is not success");

    }
}

#[test]
#[cfg(unix)]
fn report_validation_accepts_completed_successful_reports() {
    for (kind, report) in [
        (ReportKind::Doctor, COMPLETE_DOCTOR_REPORT),
        (ReportKind::Status, COMPLETE_STATUS_REPORT),
        (ReportKind::Version, "cas 1.2.3\n"),
        (ReportKind::Help, "Cassy\nUsage: cas [OPTIONS] <COMMAND>\n"),
    ] {
        let output = report_fixture(report, "0");
        let accepted = validate_piped_report(&output, kind).expect("completed successful report");
        assert_eq!(accepted, report);
    }
}

#[test]
#[cfg(unix)]
fn plain_report_validation_rejects_ansi_in_completed_output() {
    let output = report_fixture(&format!("\x1b[32m{COMPLETE_STATUS_REPORT}\x1b[0m"), "0");
    let error = validate_piped_report(&output, ReportKind::Status).expect_err("ANSI is forbidden");
    assert!(error.contains("ANSI"), "{error}");
}

#[test]
#[cfg(unix)]
fn pty_report_validation_rejects_echo_empty_and_failed_children() {
    for (kind, stdout, exit_code, _cause) in [
        (
            ReportKind::Doctor,
            "$ cas doctor\n",
            "0",
            "missing completed",
        ),
        (
            ReportKind::Doctor,
            "$ cas doctor --verbose for timings and full messages\n",
            "0",
            "missing completed",
        ),
        (
            ReportKind::Status,
            "$ cas status\n",
            "0",
            "missing completed",
        ),
        (ReportKind::Doctor, "", "0", "missing completed"),
        (ReportKind::Status, "", "0", "missing completed"),
        (
            ReportKind::Doctor,
            COMPLETE_DOCTOR_REPORT,
            "23",
            "child failed",
        ),
        (
            ReportKind::Doctor,
            WRAPPED_DOCTOR_REPORT_80COL,
            "23",
            "child failed",
        ),
        (
            ReportKind::Status,
            COMPLETE_STATUS_REPORT,
            "23",
            "child failed",
        ),
    ] {
        let mut runner = PtyRunner::new();
        runner
            .spawn(
                "sh",
                &[
                    "-c",
                    "printf '%s' \"$1\"; exit \"$2\"",
                    "report-fixture",
                    stdout,
                    exit_code,
                ],
            )
            .unwrap();
        let error = completed_pty_report(&mut runner, kind).expect_err("invalid PTY report");

    }
}

#[test]
#[cfg(unix)]
fn pty_report_validation_accepts_completed_successful_children() {
    for (kind, report) in [
        (ReportKind::Doctor, COMPLETE_DOCTOR_REPORT),
        (ReportKind::Status, COMPLETE_STATUS_REPORT),
    ] {
        let mut runner = PtyRunner::new();
        runner
            .spawn(
                "sh",
                &["-c", "printf '%s' \"$1\"", "report-fixture", report],
            )
            .unwrap();
        let captured = completed_pty_report(&mut runner, kind).expect("completed PTY report");
        assert_eq!(captured.replace("\r\n", "\n").trim_end(), report.trim_end());
    }
}

#[test]
#[cfg(unix)]
fn pty_report_rejects_completed_output_from_a_running_child() {
    let mut runner = PtyRunner::new();
    runner
        .spawn(
            "sh",
            &[
                "-c",
                "printf '%s' \"$1\"; exec sleep 30",
                "report-fixture",
                COMPLETE_STATUS_REPORT,
            ],
        )
        .unwrap();
    runner
        .wait_for_text_timeout(COMPLETE_STATUS_REPORT, Duration::from_secs(2))
        .unwrap();
    let error =
        completed_pty_report_timeout(&mut runner, ReportKind::Status, Duration::from_millis(100))
            .expect_err("a printed report does not prove child completion");

    assert!(
        !runner.is_running(),
        "timed-out fixture child must be reaped"
    );
}

#[test]
fn snapshot_redaction_preserves_stable_counts() {
    let report = "host: 7 findings — see `cas doctor --host`\ncas: 2 entries, 3 rules (1 proven), 4 high-value\nEntries: 2\nTasks: 5\nSchema: 42\n";
    assert_eq!(
        redact_dynamic_values(report),
        "host: [N] finding(s) — see `cas doctor --host`\ncas: 2 entries, 3 rules (1 proven), 4 high-value\nEntries: 2\nTasks: 5\nSchema: 42\n"
    );
}




/// Shared by real CLI/PTY tests and the negative child fixtures above. A clean
/// buffer alone cannot prove a successful render: both completion and exit
/// success are required before the no-ANSI assertion can pass.
fn validate_report_text(
    stdout: &str,
    child_succeeded: bool,
    report: ReportKind,
    require_plain: bool,
) -> Result<(), String> {
    if !child_succeeded {
        return Err(format!("child failed while rendering {report:?}:\n{stdout}"));
    }
    if require_plain && stdout.contains('\x1b') {
        return Err(format!("unexpected ANSI in {report:?} report:\n{stdout}"));
    }
    let text = stdout.trim();
    if text.is_empty() || text.starts_with("$ ") {
        return Err(format!("empty output or command echo for {report:?}"));
    }
    match report {
        ReportKind::Doctor => {
            let value: serde_json::Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
            let checks = value.as_array().filter(|checks| !checks.is_empty())
                .ok_or_else(|| "doctor JSON must contain check records".to_string())?;
            if checks.iter().any(|check| !check["name"].is_string()
                || !check["group"].is_string()
                || !matches!(check["status"].as_str(), Some("ok" | "info" | "warning" | "error"))) {
                return Err("doctor JSON check schema is incomplete".into());
            }
        }
        ReportKind::Status => {
            let value: serde_json::Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
            for key in ["entries", "rules", "proven_rules", "high_value"] {
                if value[key].as_u64().is_none() {
                    return Err(format!("status JSON lacks numeric {key}"));
                }
            }
        }
        ReportKind::Version => {
            if !regex::Regex::new(r"(?m)^cas \d+\.\d+\.\d+(?:[-+][^\s]+)?(?: \((?:[0-9a-f]+(?:-dirty)?|unknown(?:-dirty)?) (?:\d{4}-\d{2}-\d{2}|unknown)\))?\r?$")
                .unwrap().is_match(stdout) {
                return Err("version machine format is incomplete".into());
            }
        }
        ReportKind::Help | ReportKind::Plain => {}
    }
    Ok(())
}

fn validate_piped_report(
    output: &std::process::Output,
    report: ReportKind,
) -> Result<String, String> {
    let stdout = String::from_utf8(output.stdout.clone()).map_err(|error| error.to_string())?;
    validate_report_text(&stdout, output.status.success(), report, true).map_err(|error| {
        format!(
            "{error}\nexit: {}\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
    })?;
    Ok(stdout)
}

fn completed_pty_report(runner: &mut PtyRunner, report: ReportKind) -> Result<String, String> {
    completed_pty_report_timeout(runner, report, Duration::from_secs(10))
}

fn completed_pty_report_timeout(
    runner: &mut PtyRunner,
    report: ReportKind,
    timeout: Duration,
) -> Result<String, String> {
    let deadline = Instant::now() + timeout;
    while runner.is_running() {
        if Instant::now() >= deadline {
            let output = runner.get_output().as_str();
            runner.kill().map_err(|error| error.to_string())?;
            runner
                .wait()
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "killed PTY child returned no exit status".to_string())?;
            return Err(format!(
                "timed out waiting for {report:?} child completion:\n{output}"
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let status = runner
        .wait()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "PTY child returned no exit status".to_string())?;
    // Child exit and the reader thread draining the last PTY bytes are separate
    // events. Collect stable output only after exit, then decode the complete machine report.
    let output = runner.wait_stable().map_err(|error| error.to_string())?;
    let rendered = screen_with_size(&output, 80, 200).text();
    let validated = if matches!(report, ReportKind::Doctor | ReportKind::Status) { output.as_str() } else { rendered.as_str() };
    validate_report_text(validated, status.success(), report, false)
        .map_err(|error| format!("{error}\nexit: {status}"))?;
    Ok(output)
}



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

    validate_piped_report(&output, ReportKind::Plain).expect("successful complete plain report");
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

    validate_piped_report(&output, ReportKind::Plain).expect("successful complete plain report");
}

#[test]
fn status_piped_no_ansi() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    let output = cas_in_dir(&temp)
        .arg("status")
        .output()
        .expect("failed to run cas status");

    validate_piped_report(&output, ReportKind::Plain).expect("successful complete plain report");
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

    validate_piped_report(&output, ReportKind::Plain).expect("successful complete plain report");
}

// ============================================================================
// Content assertions for piped output
// ============================================================================



#[test]
fn version_piped_no_ansi() {
    let temp = TempDir::new().unwrap();
    let output = cas_cmd(temp.path())
        .arg("--version")
        .output()
        .expect("failed to run cas --version");

    validate_piped_report(&output, ReportKind::Version).expect("successful complete plain report");
}

#[test]
fn help_piped_no_ansi() {
    let temp = TempDir::new().unwrap();
    let output = cas_cmd(temp.path())
        .arg("--help")
        .output()
        .expect("failed to run cas --help");

    validate_piped_report(&output, ReportKind::Help).expect("successful complete plain report");
}

// ============================================================================
// Snapshot tests for CLI output
// ============================================================================





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
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
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
    let mut runner = pty_cas_in_dir(&temp, &["doctor", "--json"]);
    completed_pty_report(&mut runner, ReportKind::Doctor)
        .expect("doctor must exit successfully and drain a complete JSON report");
}

#[test]
fn pty_status_output() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);
    let mut runner = pty_cas_in_dir(&temp, &["status", "--json"]);
    let output = completed_pty_report(&mut runner, ReportKind::Status)
        .expect("status must exit successfully and drain a complete JSON report");
    let status: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(status["entries"], 0);
    assert_eq!(status["rules"], 0);
    assert_eq!(status["proven_rules"], 0);
    assert_eq!(status["high_value"], 0);
}

#[test]
fn pty_doctor_has_expected_sections() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);
    let mut runner = pty_cas_in_dir(&temp, &["doctor", "--json"]);
    let output = completed_pty_report(&mut runner, ReportKind::Doctor)
        .expect("doctor groups must belong to a completed successful JSON report");
    let checks: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();
    assert!(checks.iter().any(|check| check["group"] == "store"));
    assert!(checks.iter().any(|check| check["group"] == "config"));
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
        // macOS exposes /var and /tmp symlinks through their /private paths
        // in subprocess diagnostics. Replace the physical spelling first so
        // a later lexical replacement cannot leave a stray /private prefix.
        for spelling in root
            .canonicalize()
            .ok()
            .into_iter()
            .chain(std::iter::once(root))
        {
            let spelling = spelling.to_string_lossy().trim_end_matches('/').to_string();
            if spelling.len() > 1 && spelling != "/tmp" {
                result = result.replace(&spelling, "/tmp");
            }
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

    // Some Git callers expose only the short diagnostic. Match its closing
    // wrapper parenthesis as a delimiter so a similarly-prefixed diagnostic
    // with additional details is not over-redacted; put that delimiter back
    // in the replacement.
    let git_not_repo_plain_re =
        regex::Regex::new(r"fatal:\s+not\s+a\s+git\s+repository\)").unwrap();
    result = git_not_repo_plain_re
        .replace_all(&result, "[GIT_NOT_REPOSITORY])")
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

    // The verdict line (cas-4df0) ends `· <canonical id> · <version>`. Temp
    // fixtures have a generated folder id, so normalize that value
    // independently of the cloud bucket row. An info count (cas-dd56's
    // shipped-issues row reports Info when no GitHub repo resolves) sits
    // between the ok count and the id.
    let project_re =
        regex::Regex::new(r"(?m)^(\[(?:OK|WARN|ERROR)\] [^·\n]+ · \d+ ok(?: · \d+ info)?) · [^·\n]+ ·")
            .unwrap();
    result = project_re
        .replace_all(&result, "$1 · [PROJECT] ·")
        .to_string();

    // Host findings depend on which provider CLIs are installed. Entry/task/
    // rule/schema counts come from the fixture and must remain visible.
    let findings_re = regex::Regex::new(r"(\bhost:\s+)\d+ findings?\b").unwrap();
    result = findings_re
        .replace_all(&result, "${1}[N] finding(s)")
        .to_string();

    result
}

#[test]
fn doctor_snapshot_redaction_normalizes_git_not_repository_diagnostics() {
    let prefix = "[WARN] code history index: cannot check code history index: not a git repository: [TEMP_PATH] (";
    let expected = format!("{prefix}[GIT_NOT_REPOSITORY])");
    for diagnostic in [
        "fatal: not a git repository",
        "fatal: not a git repository (or any of the parent directories): .git",
        "fatal: not a git repository (or any parent up to mount point /)\nStopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).",
        "fatal: not a git repository (or any parent up to mount point /mnt) Stopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).",
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
