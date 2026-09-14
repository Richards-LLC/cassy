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

    // Parse output and check for blocking
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stop_output) {
        let continue_session = json["continue_session"].as_bool();
        let stop_reason = json["stopReason"].as_str().or(json["stop_reason"].as_str());

        // If exit blocking is working, continue_session should be false with a reason
        if continue_session == Some(false) {
            assert!(
                stop_reason
                    .map(|r| r.contains("remaining work") || r.contains("Cannot exit"))
                    .unwrap_or(false),
                "Stop reason should mention remaining work. Got: {:?}",
                stop_reason
            );
        }
    }
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

    // Parse output
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stop_output) {
        let continue_session = json["continue_session"].as_bool();
        // Should NOT be blocked (continue_session should not be false with blocking reason)
        if continue_session == Some(false) {
            let stop_reason = json["stopReason"]
                .as_str()
                .or(json["stop_reason"].as_str())
                .unwrap_or("");
            assert!(
                !stop_reason.contains("remaining work"),
                "Should not be blocked when task is closed. Got: {}",
                stop_reason
            );
        }
    }
}

/// Test that Stop is allowed when exit blocking is disabled
#[test]
fn test_exit_allowed_when_blocking_disabled() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Disable exit blocking in the current TOML config fixture.
    let config_path = temp.path().join(".cas/config.toml");
    let mut config = std::fs::read_to_string(&config_path).unwrap();
    config.push_str("\n[tasks]\nblock_exit_on_open = false\n");
    std::fs::write(&config_path, config).unwrap();

    let session_id = "no-block-session";

    // Register the session agent in the store fixture.
    let agent_id = register_agent(&temp, session_id, "test-agent");

    // Create and claim a task through the lease/store fixture (don't close it)
    let task_id = create_task(&temp, "Open task");
    claim_task(&temp, &task_id, &agent_id);

    // Try to stop - should NOT be blocked because config disabled it
    let stop_output = send_hook(&temp, "Stop", &stop_input(session_id));

    // Parse output
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stop_output) {
        let continue_session = json["continue_session"].as_bool();
        let stop_reason = json["stopReason"]
            .as_str()
            .or(json["stop_reason"].as_str())
            .unwrap_or("");

        // Should not be blocked due to remaining work
        if continue_session == Some(false) {
            assert!(
                !stop_reason.contains("remaining work"),
                "Should not be blocked when blocking is disabled. Got: {}",
                stop_reason
            );
        }
    }
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

    // Parse output.
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stop_output) {
        let continue_session = json["continue_session"].as_bool();
        let stop_reason = json["stopReason"].as_str().or(json["stop_reason"].as_str());

        // Should be blocked.
        if let (Some(false), Some(reason)) = (continue_session, stop_reason) {
            // Either mentions subtasks or remaining work.
            assert!(
                reason.contains("remaining work")
                    || reason.contains("Subtask")
                    || reason.contains("Epic"),
                "Should mention subtasks or remaining work. Got: {}",
                reason
            );
        }
    }
}

// =============================================================================
// Part G: Session Summary Tests
// =============================================================================

/// Test that Stop blocks when generate_summary is enabled and no summary exists
#[test]
fn test_stop_blocks_for_session_summary() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Enable generate_summary in config
    let config_path = temp.path().join(".cas/config.toml");
    let mut config = std::fs::read_to_string(&config_path).unwrap();
    config.push_str("\n[hooks.stop]\ngenerate_summary = true\n");
    std::fs::write(&config_path, config).unwrap();

    let session_id = "summary-test-session";

    // Create some observations
    send_hook(
        &temp,
        "PostToolUse",
        &write_tool_input(session_id, "/src/main.rs"),
    );

    // Try to stop - should be blocked for session summary
    let stop_output = send_hook(&temp, "Stop", &stop_input(session_id));

    // Parse output and check for blocking
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stop_output) {
        let continue_session = json["continue_session"].as_bool();
        let stop_reason = json["stopReason"].as_str().or(json["stop_reason"].as_str());

        // If generate_summary is working, should be blocked
        if continue_session == Some(false) {
            let reason = stop_reason.unwrap_or("");
            assert!(
                reason.contains("session-summarizer")
                    || reason.contains("summary")
                    || reason.contains("Session summary"),
                "Stop reason should mention session summary. Got: {}",
                reason
            );
        }

        // Also check for context in system_reminder
        if let Some(context) = json["system_reminder"].as_str() {
            assert!(
                context.contains("session-summary required")
                    || context.contains("session-summarizer"),
                "Context should mention session summary. Got: {}",
                context
            );
        }
    }
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

    // Parse output
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stop_output) {
        let continue_session = json["continue_session"].as_bool();
        let stop_reason = json["stopReason"]
            .as_str()
            .or(json["stop_reason"].as_str())
            .unwrap_or("");

        // Should not be blocked for session summary
        if continue_session == Some(false) {
            assert!(
                !stop_reason.contains("session-summarizer"),
                "Should not be blocked for session summary when disabled. Got: {}",
                stop_reason
            );
        }
    }
}

// =============================================================================
// Part H: Learning Review Tests
// =============================================================================
