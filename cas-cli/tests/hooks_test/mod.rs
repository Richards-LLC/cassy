//! Integration tests for CAS hooks system
//!
//! Tests the full flow of hook events from Claude Code through to storage.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

// =============================================================================
// Test Utilities
// =============================================================================

/// Create cas command for temp directory
pub(crate) fn cas_cmd(dir: &TempDir) -> Command {
    let mut cmd = Command::new(cas::test_paths::cas_binary());
    let home = dir.path().join(".test-home");
    let xdg = dir.path().join(".test-xdg-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    if let Some(host_home) = std::env::var_os("HOME") {
        cmd.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    cmd.env("HOME", home).env("XDG_CONFIG_HOME", xdg);
    cmd.current_dir(dir.path());
    // Clear CAS_ROOT to prevent env pollution from parent shell
    cmd.env_remove("CAS_ROOT");
    cmd.env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd
}

/// Initialize CAS in temp directory
pub(crate) fn init_cas(dir: &TempDir) {
    cas_cmd(dir).args(["init", "--yes"]).assert().success();
}

/// Initialize CAS with dev mode enabled
#[allow(dead_code)]
pub(crate) fn init_cas_dev_mode(dir: &TempDir) {
    init_cas(dir);
    // Config is now saved as TOML (auto-migrated from YAML if it existed)
    let config_path = dir.path().join(".cas/config.toml");
    let mut config = std::fs::read_to_string(&config_path).unwrap();
    config.push_str("\n[dev]\ndev_mode = true\n");
    std::fs::write(&config_path, config).unwrap();
}

/// Send hook event via stdin and return stdout
pub(crate) fn send_hook(dir: &TempDir, event: &str, input: &serde_json::Value) -> String {
    let output = cas_cmd(dir)
        .args(["hook", event])
        .write_stdin(serde_json::to_string(input).unwrap())
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Create PostToolUse input for Write tool
pub(crate) fn write_tool_input(session_id: &str, file_path: &str) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "cwd": "/test",
        "hook_event_name": "PostToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": file_path, "content": "test content\nline 2\nline 3"},
        "tool_response": {}
    })
}

/// Create PostToolUse input for Edit tool
pub(crate) fn edit_tool_input(
    session_id: &str,
    file_path: &str,
    old_lines: usize,
    new_lines: usize,
) -> serde_json::Value {
    let old_string = (0..old_lines)
        .map(|i| format!("old line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    let new_string = (0..new_lines)
        .map(|i| format!("new line {}", i))
        .collect::<Vec<_>>()
        .join("\n");

    serde_json::json!({
        "session_id": session_id,
        "cwd": "/test",
        "hook_event_name": "PostToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": file_path,
            "old_string": old_string,
            "new_string": new_string
        },
        "tool_response": {}
    })
}

/// Create PostToolUse input for Bash with exit code
pub(crate) fn bash_tool_input(
    session_id: &str,
    command: &str,
    exit_code: i32,
) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "cwd": "/test",
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": command},
        "tool_response": {
            "exitCode": exit_code,
            "stderr": if exit_code != 0 { "error: command failed" } else { "" }
        }
    })
}

/// Create SessionStart hook input
pub(crate) fn session_start_input(session_id: &str) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "cwd": "/test",
        "hook_event_name": "SessionStart"
    })
}

/// Create Stop hook input
pub(crate) fn stop_input(session_id: &str) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "cwd": "/test",
        "hook_event_name": "Stop"
    })
}

/// Count entries in CAS using list command
pub(crate) fn count_entries(dir: &TempDir) -> usize {
    let output = cas_cmd(dir).args(["list", "--json"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Handle empty case
    if stdout.trim().is_empty() || stdout.contains("[]") {
        return 0;
    }

    // Parse JSON array and count
    serde_json::from_str::<Vec<serde_json::Value>>(&stdout)
        .map(|v| v.len())
        .unwrap_or(0)
}

// Check if entries contain a specific substring
// =============================================================================
// Part A: PostToolUse Handler Tests
// =============================================================================

/// Core regression test: observations should be stored even without dev_mode
pub(crate) fn register_agent(dir: &TempDir, session_id: &str, name: &str) -> String {
    let output = cas_cmd(dir)
        .args([
            "agent",
            "register",
            "--name",
            name,
            "--session-id",
            session_id,
            "--json",
        ])
        .output()
        .expect("Failed to run agent register");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("Failed to parse agent register output");
    json["id"].as_str().unwrap_or("").to_string()
}

/// Helper to create a task via CLI, returns task ID
pub(crate) fn create_task(dir: &TempDir, title: &str) -> String {
    let output = cas_cmd(dir)
        .args(["task", "create", title, "--json"])
        .output()
        .expect("Failed to run task create");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("Failed to parse task create output");
    json["id"].as_str().unwrap_or("").to_string()
}

/// Helper to claim a task via CLI
pub(crate) fn claim_task(dir: &TempDir, task_id: &str, agent_id: &str) {
    cas_cmd(dir)
        .args(["task", "claim", task_id, "--agent-id", agent_id])
        .assert()
        .success();
}

/// Test that Stop is blocked when agent has claimed tasks
pub(crate) fn add_learning(dir: &TempDir, content: &str) {
    cas_cmd(dir)
        .args(["add", "--entry-type", "learning", content])
        .assert()
        .success();
    // Delay to avoid ID collision (timestamp-based ID generation)
    std::thread::sleep(std::time::Duration::from_millis(20));
}

/// Test that Stop blocks when learning_review is enabled and threshold is exceeded
pub(crate) fn add_draft_rule(dir: &TempDir, content: &str) {
    cas_cmd(dir)
        .args(["rules", "add", content])
        .assert()
        .success();
    // Delay to avoid ID collision (timestamp-based)
    std::thread::sleep(std::time::Duration::from_millis(20));
}

/// Test that Stop blocks when rule_review is enabled and threshold is exceeded
pub(crate) fn add_entry(dir: &TempDir, content: &str) {
    cas_cmd(dir).args(["add", content]).assert().success();
    // Delay to avoid ID collision (timestamp-based)
    std::thread::sleep(std::time::Duration::from_millis(20));
}

mod exit_and_summary;
mod learning_rule_duplicate;
/// Test that Stop blocks when duplicate_detection is enabled and threshold is exceeded
mod post_tool_and_flow;
