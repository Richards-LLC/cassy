//! Integration tests for CAS iteration loops
//!
//! Tests the full flow of loop creation, iteration, and completion.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

// =============================================================================
// Test Utilities
// =============================================================================

/// Create cas command for temp directory
fn cas_cmd(dir: &TempDir) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("cas"));
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
fn init_cas(dir: &TempDir) {
    cas_cmd(dir).args(["init", "--yes"]).assert().success();
}

/// Create Stop hook input
fn stop_input(session_id: &str) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "cwd": "/test",
        "hook_event_name": "Stop"
    })
}

/// Create Stop hook input with transcript path
fn stop_input_with_transcript(session_id: &str, transcript_path: &str) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "cwd": "/test",
        "hook_event_name": "Stop",
        "transcript_path": transcript_path
    })
}

/// Send hook event via stdin and return stdout
fn send_hook(dir: &TempDir, event: &str, input: &serde_json::Value) -> String {
    let output = cas_cmd(dir)
        .args(["hook", event])
        .write_stdin(serde_json::to_string(input).unwrap())
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Get loop status JSON
fn get_loop_status(dir: &TempDir, session_id: &str) -> serde_json::Value {
    let output = cas_cmd(dir)
        .args(["loop", "status", "--session", session_id, "--json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or(serde_json::json!({"active": false}))
}

/// List loops and return JSON
fn list_loops(dir: &TempDir) -> Vec<serde_json::Value> {
    let output = cas_cmd(dir)
        .args(["loop", "list", "--json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or_default()
}

// =============================================================================
// Loop CLI Tests
// =============================================================================

#[test]
#[ignore = "loop CLI command removed - tests need MCP fixtures"]
fn test_loop_start_creates_active_loop() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Start a loop
    cas_cmd(&temp)
        .args([
            "loop",
            "start",
            "Implement the feature",
            "--session",
            "test-session-001",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"id\""))
        .stdout(predicate::str::contains("\"session\""));

    // Verify loop is active
    let status = get_loop_status(&temp, "test-session-001");
    assert!(status.get("id").is_some(), "Loop should have an ID");
    assert_eq!(
        status.get("status").and_then(|v| v.as_str()),
        Some("active")
    );
    assert_eq!(status.get("iteration").and_then(|v| v.as_i64()), Some(1));
}

#[test]
#[ignore = "loop CLI command removed - tests need MCP fixtures"]
fn test_loop_start_with_promise_and_max_iterations() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Start a loop with options
    cas_cmd(&temp)
        .args([
            "loop",
            "start",
            "Build the thing",
            "--session",
            "test-session-002",
            "--promise",
            "DONE",
            "--max-iterations",
            "5",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"max_iterations\": 5"));

    // Verify options are set
    let status = get_loop_status(&temp, "test-session-002");
    assert_eq!(
        status.get("max_iterations").and_then(|v| v.as_i64()),
        Some(5)
    );
    assert_eq!(status.get("promise").and_then(|v| v.as_str()), Some("DONE"));
}

#[test]
#[ignore = "loop CLI command removed - tests need MCP fixtures"]
fn test_loop_cancel_stops_active_loop() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Start a loop
    cas_cmd(&temp)
        .args([
            "loop",
            "start",
            "Test prompt",
            "--session",
            "test-session-003",
        ])
        .assert()
        .success();

    // Cancel it
    cas_cmd(&temp)
        .args([
            "loop",
            "cancel",
            "--session",
            "test-session-003",
            "--reason",
            "User cancelled",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"status\": \"cancelled\""));

    // Verify no longer active
    let status = get_loop_status(&temp, "test-session-003");
    assert_eq!(status.get("active").and_then(|v| v.as_bool()), Some(false));
}

#[test]
#[ignore = "loop CLI command removed - tests need MCP fixtures"]
fn test_loop_list_shows_recent_loops() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Create multiple loops
    for i in 1..=3 {
        cas_cmd(&temp)
            .args([
                "loop",
                "start",
                &format!("Task {i}"),
                "--session",
                &format!("session-{i}"),
            ])
            .assert()
            .success();

        // Cancel to allow creating another
        cas_cmd(&temp)
            .args(["loop", "cancel", "--session", &format!("session-{i}")])
            .assert()
            .success();
    }

    // List loops
    let loops = list_loops(&temp);
    assert_eq!(loops.len(), 3, "Should have 3 loops");
}

#[test]
#[ignore = "loop CLI command removed - tests need MCP fixtures"]
fn test_loop_prevents_duplicate_for_session() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Start first loop
    cas_cmd(&temp)
        .args(["loop", "start", "First task", "--session", "same-session"])
        .assert()
        .success();

    // Try to start another for same session (should fail)
    cas_cmd(&temp)
        .args([
            "loop",
            "start",
            "Second task",
            "--session",
            "same-session",
            "--json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"error\""));
}

// =============================================================================
// Stop Hook Integration Tests
// =============================================================================

#[test]
#[ignore = "loop CLI command removed - tests need MCP fixtures"]
fn test_stop_hook_blocks_exit_with_active_loop() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Start a loop
    cas_cmd(&temp)
        .args([
            "loop",
            "start",
            "Keep working on this",
            "--session",
            "stop-test-session",
        ])
        .assert()
        .success();

    // Send Stop hook
    let output = send_hook(&temp, "Stop", &stop_input("stop-test-session"));

    // Debug: print raw output
    eprintln!("Stop hook output: {output}");

    // Should block exit (decision: "block" means don't exit, Claude continues)
    let response: serde_json::Value = serde_json::from_str(&output).unwrap_or_default();
    assert_eq!(
        response.get("decision").and_then(|v| v.as_str()),
        Some("block"),
        "Stop hook should block exit when loop is active"
    );

    // Should include the prompt in systemMessage or additionalContext (camelCase from serde)
    let system_message = response
        .get("systemMessage")
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let additional_context = response
        .get("hookSpecificOutput")
        .and_then(|h| h.get("additionalContext"))
        .and_then(|c| c.as_str())
        .unwrap_or("");

    assert!(
        system_message.contains("Keep working on this")
            || additional_context.contains("Keep working on this"),
        "Should include loop prompt in systemMessage or additionalContext. systemMessage='{system_message}', additionalContext='{additional_context}'"
    );
}

#[test]
#[ignore = "loop CLI command removed - tests need MCP fixtures"]
fn test_stop_hook_increments_iteration() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Start a loop
    cas_cmd(&temp)
        .args(["loop", "start", "Iterate", "--session", "iter-test-session"])
        .assert()
        .success();

    // Initial iteration should be 1
    let status1 = get_loop_status(&temp, "iter-test-session");
    assert_eq!(status1.get("iteration").and_then(|v| v.as_i64()), Some(1));

    // Send Stop hook
    send_hook(&temp, "Stop", &stop_input("iter-test-session"));

    // Iteration should now be 2
    let status2 = get_loop_status(&temp, "iter-test-session");
    assert_eq!(status2.get("iteration").and_then(|v| v.as_i64()), Some(2));

    // Send another Stop hook
    send_hook(&temp, "Stop", &stop_input("iter-test-session"));

    // Iteration should now be 3
    let status3 = get_loop_status(&temp, "iter-test-session");
    assert_eq!(status3.get("iteration").and_then(|v| v.as_i64()), Some(3));
}

#[test]
#[ignore = "loop CLI command removed - tests need MCP fixtures"]
fn test_stop_hook_respects_max_iterations() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Start a loop with max 2 iterations
    cas_cmd(&temp)
        .args([
            "loop",
            "start",
            "Limited iterations",
            "--session",
            "max-iter-session",
            "--max-iterations",
            "2",
        ])
        .assert()
        .success();

    // First Stop - should block (iteration 1 -> 2)
    let output1 = send_hook(&temp, "Stop", &stop_input("max-iter-session"));
    let response1: serde_json::Value = serde_json::from_str(&output1).unwrap_or_default();
    assert_eq!(
        response1.get("decision").and_then(|v| v.as_str()),
        Some("block"),
        "First Stop should block exit"
    );

    // Second Stop - should allow exit (max reached)
    let output2 = send_hook(&temp, "Stop", &stop_input("max-iter-session"));
    let response2: serde_json::Value = serde_json::from_str(&output2).unwrap_or_default();
    // When max is reached, we return empty output (allow exit)
    assert!(
        response2.get("decision").is_none()
            || response2.get("decision").and_then(|v| v.as_str()) != Some("block"),
        "Second Stop should allow exit after max iterations"
    );

    // Verify loop is no longer active
    let status = get_loop_status(&temp, "max-iter-session");
    assert_eq!(status.get("active").and_then(|v| v.as_bool()), Some(false));
}

#[test]
#[ignore = "loop CLI command removed - tests need MCP fixtures"]
fn test_stop_hook_without_active_loop_allows_exit() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Send Stop hook without any active loop
    let output = send_hook(&temp, "Stop", &stop_input("no-loop-session"));

    // Should not block exit
    let response: serde_json::Value = serde_json::from_str(&output).unwrap_or_default();
    assert!(
        response.get("decision").is_none()
            || response.get("decision").and_then(|v| v.as_str()) != Some("block"),
        "Stop hook without active loop should not block exit"
    );
}

// =============================================================================
// Completion Promise Tests
// =============================================================================

#[test]
#[ignore = "loop CLI command removed - tests need MCP fixtures"]
fn test_completion_promise_in_transcript() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Create a mock transcript file with the promise
    let transcript_path = temp.path().join("test_transcript.jsonl");
    let transcript_content = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"I have completed the task. <promise>DONE</promise>"}]}}
"#;
    std::fs::write(&transcript_path, transcript_content).unwrap();

    // Start a loop with promise
    cas_cmd(&temp)
        .args([
            "loop",
            "start",
            "Work until done",
            "--session",
            "promise-test-session",
            "--promise",
            "DONE",
        ])
        .assert()
        .success();

    // Send Stop hook with transcript path
    let input =
        stop_input_with_transcript("promise-test-session", transcript_path.to_str().unwrap());
    let output = send_hook(&temp, "Stop", &input);

    // Should allow exit (promise detected)
    let response: serde_json::Value = serde_json::from_str(&output).unwrap_or_default();
    assert!(
        response.get("decision").is_none()
            || response.get("decision").and_then(|v| v.as_str()) != Some("block"),
        "Stop hook should allow exit when promise is detected"
    );

    // Verify loop is completed
    let status = get_loop_status(&temp, "promise-test-session");
    assert_eq!(status.get("active").and_then(|v| v.as_bool()), Some(false));
}

#[test]
#[ignore = "loop CLI command removed - tests need MCP fixtures"]
fn test_loop_continues_without_promise() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Create a transcript without the promise
    let transcript_path = temp.path().join("test_transcript2.jsonl");
    let transcript_content = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Still working on it..."}]}}
"#;
    std::fs::write(&transcript_path, transcript_content).unwrap();

    // Start a loop with promise
    cas_cmd(&temp)
        .args([
            "loop",
            "start",
            "Work until DONE",
            "--session",
            "no-promise-session",
            "--promise",
            "DONE",
        ])
        .assert()
        .success();

    // Send Stop hook with transcript path
    let input = stop_input_with_transcript("no-promise-session", transcript_path.to_str().unwrap());
    let output = send_hook(&temp, "Stop", &input);

    // Should block exit (promise not found)
    let response: serde_json::Value = serde_json::from_str(&output).unwrap_or_default();
    assert_eq!(
        response.get("decision").and_then(|v| v.as_str()),
        Some("block"),
        "Stop hook should block exit when promise is not found"
    );

    // Verify loop is still active
    let status = get_loop_status(&temp, "no-promise-session");
    assert_eq!(
        status.get("status").and_then(|v| v.as_str()),
        Some("active")
    );
}

// =============================================================================
// Task Integration Tests
// =============================================================================

#[test]
#[ignore = "loop CLI command removed - tests need MCP fixtures"]
fn test_loop_with_task_linking() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Create a task and capture the ID from output
    let output = cas_cmd(&temp)
        .args(["task", "create", "Implement feature X", "--json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    eprintln!("Task create output: {stdout}");

    // Parse task ID from create output
    let task_response: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_default();
    let task_id = task_response
        .get("id")
        .and_then(|id| id.as_str())
        .expect("Task create should return an id");

    // Start a loop linked to the task
    cas_cmd(&temp)
        .args([
            "loop",
            "start",
            "Work on feature X",
            "--session",
            "task-link-session",
            "--task",
            task_id,
            "--json",
        ])
        .assert()
        .success();

    // Verify loop has task_id
    let status = get_loop_status(&temp, "task-link-session");
    assert_eq!(
        status.get("task_id").and_then(|v| v.as_str()),
        Some(task_id),
        "Loop should be linked to task"
    );

    // Trigger a Stop hook to add iteration note
    send_hook(&temp, "Stop", &stop_input("task-link-session"));

    // Verify task has notes (would need to check task show output)
    let task_output = cas_cmd(&temp)
        .args(["task", "show", task_id])
        .output()
        .unwrap();
    let task_stdout = String::from_utf8_lossy(&task_output.stdout);
    eprintln!("Task show output: {task_stdout}");
    assert!(
        task_stdout.contains("Loop iteration") || task_stdout.contains("loop-"),
        "Task should have loop iteration notes. Got: {task_stdout}"
    );
}

// =============================================================================
// Edge Cases
// =============================================================================

#[test]
#[ignore = "loop CLI command removed - tests need MCP fixtures"]
fn test_loop_status_for_nonexistent_session() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Get status for nonexistent session
    let status = get_loop_status(&temp, "nonexistent-session");
    assert_eq!(status.get("active").and_then(|v| v.as_bool()), Some(false));
}

#[test]
#[ignore = "loop CLI command removed - tests need MCP fixtures"]
fn test_loop_cancel_for_nonexistent_session() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Cancel should not error, just report no loop
    cas_cmd(&temp)
        .args(["loop", "cancel", "--session", "nonexistent-session"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No active loop"));
}

#[test]
#[ignore = "loop CLI command removed - tests need MCP fixtures"]
fn test_loop_with_empty_prompt() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Start a loop with minimal prompt
    cas_cmd(&temp)
        .args(["loop", "start", "x", "--session", "minimal-session"])
        .assert()
        .success();

    // Should work
    let status = get_loop_status(&temp, "minimal-session");
    assert_eq!(
        status.get("status").and_then(|v| v.as_str()),
        Some("active")
    );
}
