use crate::hooks_test::*;
use tempfile::TempDir;

#[test]
fn test_exit_blocked_with_claimed_task() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    let session_id = "blocked-session-001";

    // Register the session agent in the store fixture.
    let agent_id = register_agent(&temp, session_id, "test-agent");
    assert!(!agent_id.is_empty(), "Agent should be registered");

    // Create and claim a task through the lease/store fixture.
    let task_id = create_task(&temp, "Blocking task");
    claim_task(&temp, &task_id, &agent_id);

    // Try to stop - should be blocked
    let stop_output = send_hook(&temp, "Stop", &stop_input(session_id));

    assert_stop_blocked(&stop_output, &["remaining work", "Cannot exit"], None);
}

/// Test that Stop is allowed when all tasks are closed
#[test]
fn test_exit_allowed_when_tasks_closed() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    let session_id = "allowed-session-001";

    // Register the session agent in the store fixture.
    let agent_id = register_agent(&temp, session_id, "test-agent");

    // Create a task through the store fixture.
    let task_id = create_task(&temp, "Quick task");

    // Claim the task through the lease/store fixture.
    claim_task(&temp, &task_id, &agent_id);

    close_task(&temp, &task_id);

    // Try to stop - should be allowed
    let stop_output = send_hook(&temp, "Stop", &stop_input(session_id));

    assert_stop_allowed(&stop_output, Some("remaining work"));
}

/// Test that Stop is allowed when exit blocking is disabled
#[test]
fn test_exit_allowed_when_blocking_disabled() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Disable exit blocking in the existing tasks table.
    set_config_value(&temp, "tasks.block_exit_on_open", "false");

    let session_id = "no-block-session";

    // Register the session agent in the store fixture.
    let agent_id = register_agent(&temp, session_id, "test-agent");

    // Create and claim a task through the lease/store fixture (don't close it)
    let task_id = create_task(&temp, "Open task");
    claim_task(&temp, &task_id, &agent_id);

    // Try to stop - should NOT be blocked because config disabled it
    let stop_output = send_hook(&temp, "Stop", &stop_input(session_id));

    assert_stop_allowed(&stop_output, Some("remaining work"));
}

/// Test that Stop is blocked with open epic subtasks
#[test]
fn test_exit_blocked_with_epic_subtasks() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    let session_id = "epic-session-001";

    // Register the session agent in the store fixture.
    let agent_id = register_agent(&temp, session_id, "test-agent");

    // Create an epic and subtask through the task-store fixture.
    let epic_id = create_epic(&temp, "Epic task");
    let subtask_id = create_task(&temp, "Subtask 1");
    add_epic_subtask(&temp, &subtask_id, &epic_id);

    // Claim the epic (but not the subtask) through the lease/store fixture.
    claim_task(&temp, &epic_id, &agent_id);

    // Try to stop - should be blocked because of open subtask.
    let stop_output = send_hook(&temp, "Stop", &stop_input(session_id));

    assert_stop_blocked(&stop_output, &["remaining work", "Subtask", "Epic"], None);
}

// =============================================================================
// Part G: Session Summary Tests
// =============================================================================

/// Test that Stop blocks when generate_summary is enabled and no summary exists
#[test]
fn test_stop_blocks_for_session_summary() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Enable generate_summary in the existing hooks.stop table.
    set_config_value(&temp, "hooks.stop.generate_summary", "true");

    let session_id = "summary-test-session";

    // Create some observations
    send_hook(
        &temp,
        "PostToolUse",
        &write_tool_input(session_id, "/src/main.rs"),
    );

    // Try to stop - should be blocked for session summary
    let stop_output = send_hook(&temp, "Stop", &stop_input(session_id));

    assert_stop_blocked(
        &stop_output,
        &["session-summarizer", "summary", "Session summary"],
        Some(&["session-summary required", "session-summarizer"]),
    );
}

/// Test that Stop is not blocked when generate_summary is disabled (default)
#[test]
fn test_stop_not_blocked_without_summary_config() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Don't enable generate_summary (use default config)
    let session_id = "no-summary-session";

    // Create some observations
    send_hook(
        &temp,
        "PostToolUse",
        &write_tool_input(session_id, "/src/main.rs"),
    );

    // Try to stop - should NOT be blocked for session summary
    let stop_output = send_hook(&temp, "Stop", &stop_input(session_id));

    assert_stop_allowed(&stop_output, Some("session-summarizer"));
}

// =============================================================================
// Part H: Learning Review Tests
// =============================================================================
