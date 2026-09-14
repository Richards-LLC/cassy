use crate::hooks_test::*;
use tempfile::TempDir;

#[test]
fn test_post_tool_use_stores_attribution_without_dev_mode() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp); // dev_mode=false (default)

    // Write attribution is durable independently of the optional dev tracer.
    let session_id = "test-session-001";
    let file_path = "/project/src/main.rs";
    let input = write_tool_input(session_id, file_path);
    send_hook(&temp, "PostToolUse", &input);

    let changes = file_changes(&temp);
    assert_eq!(
        changes.len(),
        1,
        "Write should create one attribution record"
    );
    assert_eq!(changes[0].session_id, session_id);
    assert_eq!(changes[0].tool_name, "Write");
    assert_eq!(changes[0].file_path, file_path);
    assert!(
        buffered_observations(&temp).is_empty(),
        "dev_mode=false must not persist raw tracer observations"
    );
}

#[test]
fn test_post_tool_use_without_session_does_not_create_orphan_observation() {
    let temp = TempDir::new().unwrap();
    init_cas_dev_mode(&temp);

    send_hook(&temp, "PostToolUse", &bash_tool_input("", "cargo test", 1));

    assert_eq!(
        count_buffered_observations(&temp),
        0,
        "a missing harness session must not create an unconsumable buffer row"
    );
}

#[test]
fn test_post_tool_use_filters_simple_commands() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Send simple commands that should be filtered
    send_hook(
        &temp,
        "PostToolUse",
        &bash_tool_input("test-session", "ls -la", 0),
    );
    send_hook(
        &temp,
        "PostToolUse",
        &bash_tool_input("test-session", "cd /tmp", 0),
    );
    send_hook(
        &temp,
        "PostToolUse",
        &bash_tool_input("test-session", "pwd", 0),
    );

    // These should be filtered out
    let count = count_entries(&temp);
    assert_eq!(
        count, 0,
        "Simple commands (ls, cd, pwd) should be filtered out. Got {} entries",
        count
    );
}

#[test]
fn test_post_tool_use_captures_errors() {
    let temp = TempDir::new().unwrap();
    init_cas_dev_mode(&temp);

    // Send failed Bash command
    let input = bash_tool_input("test-session-err", "cargo test", 1);
    send_hook(&temp, "PostToolUse", &input);

    // Raw observations are buffered by the enabled dev tracer. The error
    // flag, exit code, and useful command content must survive the hook.
    let observations = buffered_observations(&temp);
    assert_eq!(observations.len(), 1, "failed Bash should be buffered once");
    let (tool, content, is_error, exit_code) = &observations[0];
    assert_eq!(tool, "Bash");
    assert!(
        content.contains("cargo test"),
        "buffered content: {content}"
    );
    assert!(*is_error, "failed Bash must retain error metadata");
    assert_eq!(*exit_code, Some(1));
}

#[test]
fn test_post_tool_use_captures_significant_edits() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Send significant edit (15 lines changed)
    let input = edit_tool_input("test-session-edit", "/project/big_change.rs", 5, 20);
    send_hook(&temp, "PostToolUse", &input);

    let changes = file_changes(&temp);
    assert_eq!(
        changes.len(),
        1,
        "Edit should create one attribution record"
    );
    assert_eq!(changes[0].session_id, "test-session-edit");
    assert_eq!(changes[0].tool_name, "Edit");
    assert_eq!(changes[0].file_path, "/project/big_change.rs");
}

#[test]
fn test_post_tool_use_ignores_small_edits() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Send small edit (2 lines changed)
    let input = edit_tool_input("test-session-small", "/project/small.rs", 3, 5);
    send_hook(&temp, "PostToolUse", &input);

    // Small edits should be ignored (less than 10 line diff, less than 50 total)
    let count = count_entries(&temp);
    assert_eq!(
        count, 0,
        "Small edits should be ignored. Got {} entries",
        count
    );
}

#[test]
fn test_post_tool_use_captures_writes() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Send Write tool event (new file creation)
    let input = write_tool_input("test-session-write", "/project/new_file.rs");
    send_hook(&temp, "PostToolUse", &input);

    let changes = file_changes(&temp);
    assert_eq!(
        changes.len(),
        1,
        "Write should create one attribution record"
    );
    assert_eq!(changes[0].session_id, "test-session-write");
    assert_eq!(changes[0].tool_name, "Write");
    assert_eq!(changes[0].file_path, "/project/new_file.rs");
}

#[test]
fn test_post_tool_use_captures_significant_bash() {
    let temp = TempDir::new().unwrap();
    init_cas_dev_mode(&temp);

    // Send significant bash commands
    send_hook(
        &temp,
        "PostToolUse",
        &bash_tool_input("test-session", "cargo build", 0),
    );
    send_hook(
        &temp,
        "PostToolUse",
        &bash_tool_input("test-session", "cargo test", 0),
    );
    send_hook(
        &temp,
        "PostToolUse",
        &bash_tool_input("test-session", "git commit -m 'test'", 0),
    );

    let observations = buffered_observations(&temp);
    assert_eq!(
        observations.len(),
        3,
        "each significant Bash should be buffered"
    );
    for expected_command in ["cargo build", "cargo test", "git commit"] {
        let observation = observations
            .iter()
            .find(|(_, content, _, _)| content.contains(expected_command));
        let (tool, content, is_error, exit_code) =
            observation.unwrap_or_else(|| panic!("missing buffered command {expected_command:?}"));
        assert_eq!(tool, "Bash");
        assert!(
            !content.is_empty(),
            "buffered Bash content must be retained"
        );
        assert!(
            !is_error,
            "successful Bash should not be marked as an error"
        );
        assert_eq!(*exit_code, Some(0));
    }
}

// =============================================================================
// Part B: Stop Hook Synthesis Tests
// =============================================================================

#[test]
fn test_stop_handles_empty_session() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Send stop without any prior observations
    let input = stop_input("empty-session");
    let output = send_hook(&temp, "Stop", &input);

    // Should succeed without error
    assert!(
        !output.contains("error"),
        "Stop should handle empty sessions gracefully"
    );
}

#[test]
fn test_stop_synthesizes_matching_buffered_observations_component() {
    let temp = TempDir::new().unwrap();
    init_cas_dev_mode(&temp);

    let session_id = "summary-session";

    // Seed matching-session observations in the current tracer store. Stop's
    // production path reads these rows, synthesizes learnings, and clears the
    // buffer after processing it.
    seed_buffered_observation(
        &temp,
        session_id,
        "Write",
        "Write: /src/main.rs",
        None,
        false,
    );
    seed_buffered_observation(
        &temp,
        session_id,
        "Bash",
        "Bash: cargo build",
        Some(0),
        false,
    );
    let before_stop = count_entries(&temp);

    // End session
    send_hook(&temp, "Stop", &stop_input(session_id));

    // Stop should synthesize a durable learning from matching rows and clear
    // only the rows it consumed.
    let count = count_entries(&temp);
    assert!(
        count > before_stop,
        "Stop should create a session learning. Before: {before_stop}, after: {count}"
    );
    assert_eq!(
        count_buffered_observations(&temp),
        0,
        "Stop should clear the matching observation buffer"
    );
    let entries = open_entries(&temp);
    assert!(
        entries.iter().any(|entry| {
            entry.session_id.as_deref() == Some(session_id)
                && entry.tags.iter().any(|tag| tag == "build-success")
        }),
        "Stop synthesis should retain the source session"
    );
}

// =============================================================================
// Part C: SessionStart Context Injection Tests
// =============================================================================

#[test]
fn test_session_start_returns_json() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Send SessionStart
    let input = session_start_input("context-session");
    let output = send_hook(&temp, "SessionStart", &input);

    // Should return valid JSON
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&output);
    assert!(
        parsed.is_ok(),
        "SessionStart should return valid JSON. Got: {}",
        output
    );
}

#[test]
fn test_session_start_includes_tasks() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Create a task through the local task-store fixture.
    create_task(&temp, "Test task for context");

    // Send SessionStart
    let input = session_start_input("task-context-session");
    let output = send_hook(&temp, "SessionStart", &input);

    // Should include task info (may be in context or systemReminder)
    // The context should at least be non-empty if tasks exist
    assert!(
        !output.is_empty(),
        "SessionStart should return context with tasks"
    );
}

#[test]
fn test_session_start_plan_mode() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Send SessionStart with plan mode
    let input = serde_json::json!({
        "session_id": "plan-mode-session",
        "cwd": "/test",
        "hook_event_name": "SessionStart",
        "permission_mode": "plan"
    });
    let output = send_hook(&temp, "SessionStart", &input);

    // Should return valid JSON (plan mode may have different context)
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&output);
    assert!(
        parsed.is_ok(),
        "SessionStart plan mode should return valid JSON. Got: {}",
        output
    );
}

// =============================================================================
// Part D: End-to-End Flow Tests
// =============================================================================

#[test]
fn test_e2e_tool_use_to_entry() {
    let temp = TempDir::new().unwrap();
    init_cas_dev_mode(&temp);

    let session_id = "e2e-session";
    let other_session_id = "other-e2e-session";

    // Simulate session with tool uses
    send_hook(
        &temp,
        "PostToolUse",
        &write_tool_input(session_id, "/src/main.rs"),
    );
    send_hook(
        &temp,
        "PostToolUse",
        &bash_tool_input(session_id, "cargo build", 0),
    );
    send_hook(
        &temp,
        "PostToolUse",
        &bash_tool_input(session_id, "cargo test", 1),
    ); // error
    send_hook(
        &temp,
        "PostToolUse",
        &bash_tool_input(other_session_id, "cargo check", 1),
    ); // must survive the first session's Stop

    // End session
    send_hook(&temp, "Stop", &stop_input(session_id));

    // Write attribution and dev-tracer buffering are the current durable
    // capture paths exercised by this full hook flow.
    let changes = file_changes(&temp);
    assert!(
        changes.iter().any(|change| {
            change.session_id == session_id
                && change.tool_name == "Write"
                && change.file_path == "/src/main.rs"
        }),
        "E2E flow should retain Write attribution"
    );
    let observations = buffered_observations_with_sessions(&temp);
    assert!(
        observations
            .iter()
            .all(|(session, ..)| session == other_session_id),
        "E2E Stop should clear only the matching buffered observations: {observations:?}"
    );
    assert_eq!(
        observations.len(),
        1,
        "E2E Stop should retain one unrelated session row"
    );
    assert!(
        observations
            .iter()
            .any(|(session, tool, content, is_error, exit_code)| {
                session == other_session_id
                    && tool == "Bash"
                    && content.contains("cargo check")
                    && *is_error
                    && *exit_code == Some(1)
            }),
        "unrelated session observation should retain its error metadata"
    );
    let entries = open_entries(&temp);
    assert!(
        entries.iter().any(|entry| {
            entry.session_id.as_deref() == Some(session_id)
                && entry.tags.iter().any(|tag| tag == "session-errors")
        }),
        "E2E Stop should synthesize the failed Bash observation"
    );
}

#[test]
fn test_e2e_multiple_sessions() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Session 1
    send_hook(
        &temp,
        "PostToolUse",
        &write_tool_input("session-1", "/src/a.rs"),
    );
    send_hook(&temp, "Stop", &stop_input("session-1"));

    let session_1_changes = file_changes(&temp);

    // Session 2
    send_hook(
        &temp,
        "PostToolUse",
        &write_tool_input("session-2", "/src/b.rs"),
    );
    send_hook(&temp, "Stop", &stop_input("session-2"));

    let all_changes = file_changes(&temp);

    // File attribution is session-scoped; the second session must add its own
    // row without changing the first session's identity.
    assert!(
        session_1_changes
            .iter()
            .any(|change| change.session_id == "session-1" && change.file_path == "/src/a.rs"),
        "Session 1 should produce its own attribution record"
    );
    assert!(
        all_changes
            .iter()
            .any(|change| change.session_id == "session-2" && change.file_path == "/src/b.rs"),
        "Session 2 should produce its own attribution record"
    );
    assert_eq!(
        all_changes
            .iter()
            .filter(|change| change.session_id == "session-1")
            .count(),
        1,
        "Session 1 records must not be duplicated by Session 2"
    );
    assert_eq!(
        all_changes
            .iter()
            .filter(|change| change.session_id == "session-2")
            .count(),
        1,
        "Session 2 records must remain isolated"
    );
}

#[test]
fn test_e2e_error_observation_captured() {
    let temp = TempDir::new().unwrap();
    init_cas_dev_mode(&temp);

    let session_id = "error-session";

    // Send only an error
    send_hook(
        &temp,
        "PostToolUse",
        &bash_tool_input(session_id, "cargo test", 1),
    );
    send_hook(&temp, "Stop", &stop_input(session_id));

    // Stop should consume the matching error row and synthesize a durable
    // learning with the failed command content.
    let observations = buffered_observations_with_sessions(&temp);
    assert!(
        observations.is_empty(),
        "Stop should consume the matching error observation"
    );
    assert!(
        open_entries(&temp).iter().any(|entry| {
            entry.session_id.as_deref() == Some(session_id)
                && entry.tags.iter().any(|tag| tag == "session-errors")
                && entry.content.contains("Bash: cargo test")
        }),
        "Stop should synthesize the failed Bash observation"
    );
}

// =============================================================================
// Part E: Hook Configuration Tests
// =============================================================================

#[test]
fn test_hook_status_command() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Configure hooks
    cas_cmd(&temp)
        .args(["hook", "configure"])
        .assert()
        .success();

    // Check status
    cas_cmd(&temp)
        .args(["hook", "status"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("SessionStart").or(predicate::str::contains("configured")),
        );
}

#[test]
fn test_hook_configure_creates_settings() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Configure hooks
    cas_cmd(&temp)
        .args(["hook", "configure"])
        .assert()
        .success();

    // Verify settings file exists
    let settings_path = temp.path().join(".claude/settings.json");
    assert!(
        settings_path.exists(),
        "hook configure should create .claude/settings.json"
    );
}

// =============================================================================
// Part F: Exit Blocking Tests
// =============================================================================
