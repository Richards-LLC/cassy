use crate::hooks_test::*;
use tempfile::TempDir;

#[test]
fn test_stop_blocks_for_learning_review() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Enable learning_review in the existing hooks.stop table.
    set_config_value(&temp, "hooks.stop.learning_review_enabled", "true");
    set_config_value(&temp, "hooks.stop.learning_review_threshold", "3");

    let session_id = "learning-review-test-session";

    // Add 5 learnings (above threshold of 3)
    for i in 0..5 {
        add_learning(&temp, &format!("Test learning number {}", i));
    }

    // Create some observations first
    send_hook(
        &temp,
        "PostToolUse",
        &write_tool_input(session_id, "/src/main.rs"),
    );

    // Try to stop - should be blocked for learning review
    let stop_output = send_hook(&temp, "Stop", &stop_input(session_id));

    assert_stop_blocked(
        &stop_output,
        &["learning-reviewer", "learning", "Learning review"],
        Some(&[
            "learning-review required",
            "learning-reviewer",
            "Unreviewed Learnings",
        ]),
    );
}

/// Test that Stop is NOT blocked when learnings are below threshold
#[test]
fn test_stop_not_blocked_below_learning_threshold() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Enable learning_review in the existing hooks.stop table.
    set_config_value(&temp, "hooks.stop.learning_review_enabled", "true");
    set_config_value(&temp, "hooks.stop.learning_review_threshold", "10");

    let session_id = "below-threshold-session";

    // Add only 3 learnings (below threshold of 10)
    for i in 0..3 {
        add_learning(&temp, &format!("Test learning below threshold {}", i));
    }

    // Try to stop - should NOT be blocked
    let stop_output = send_hook(&temp, "Stop", &stop_input(session_id));

    assert_stop_allowed(&stop_output, Some("learning-review"));
}

/// Test that Stop is not blocked when learning_review is disabled (default)
#[test]
fn test_stop_not_blocked_without_learning_review_config() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Don't enable learning_review (use default config)
    let session_id = "no-learning-review-session";

    // Add several learnings
    for i in 0..10 {
        add_learning(&temp, &format!("Test learning {}", i));
    }

    // Try to stop - should NOT be blocked for learning review
    let stop_output = send_hook(&temp, "Stop", &stop_input(session_id));

    assert_stop_allowed(&stop_output, Some("learning-review"));
}

// =============================================================================
// Part I: Rule Review Tests
// =============================================================================

/// Test that Stop blocks when rule_review is enabled and threshold is exceeded
#[test]
fn test_stop_blocks_for_rule_review() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Enable rule_review in the existing hooks.stop table.
    set_config_value(&temp, "hooks.stop.rule_review_enabled", "true");
    set_config_value(&temp, "hooks.stop.rule_review_threshold", "3");

    let session_id = "rule-review-test-session";

    // Add 5 draft rules (above threshold of 3)
    for i in 0..5 {
        add_draft_rule(&temp, &format!("Test draft rule number {}", i));
    }

    // Create some observations first
    send_hook(
        &temp,
        "PostToolUse",
        &write_tool_input(session_id, "/src/main.rs"),
    );

    // Try to stop - should be blocked for rule review
    let stop_output = send_hook(&temp, "Stop", &stop_input(session_id));

    assert_stop_blocked(
        &stop_output,
        &["rule-reviewer", "rule", "Rule review"],
        Some(&["rule-review required", "rule-reviewer", "Draft Rules"]),
    );
}

/// Test that Stop is NOT blocked when draft rules are below threshold
#[test]
fn test_stop_not_blocked_below_rule_threshold() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Enable rule_review in the existing hooks.stop table.
    set_config_value(&temp, "hooks.stop.rule_review_enabled", "true");
    set_config_value(&temp, "hooks.stop.rule_review_threshold", "10");

    let session_id = "below-rule-threshold-session";

    // Add only 3 draft rules (below threshold of 10)
    for i in 0..3 {
        add_draft_rule(&temp, &format!("Test draft rule below threshold {}", i));
    }

    // Try to stop - should NOT be blocked
    let stop_output = send_hook(&temp, "Stop", &stop_input(session_id));

    assert_stop_allowed(&stop_output, Some("rule-review"));
}

/// Test that Stop is not blocked when rule_review is explicitly disabled.
#[test]
fn test_stop_not_blocked_with_rule_review_disabled() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Preserve the explicit opt-out contract while the default is enabled.
    set_config_value(&temp, "hooks.stop.rule_review_enabled", "false");

    let session_id = "no-rule-review-session";

    // Add several draft rules
    for i in 0..10 {
        add_draft_rule(&temp, &format!("Test draft rule {}", i));
    }

    // Try to stop - should NOT be blocked for rule review
    let stop_output = send_hook(&temp, "Stop", &stop_input(session_id));

    assert_stop_allowed(&stop_output, Some("rule-review"));
}

// =============================================================================
// Part J: Duplicate Detection Tests
// =============================================================================

/// Test that Stop blocks when duplicate_detection is enabled and threshold is exceeded
#[test]
fn test_stop_blocks_for_duplicate_detection() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Enable duplicate_detection in the existing hooks.stop table.
    set_config_value(&temp, "hooks.stop.duplicate_detection_enabled", "true");
    set_config_value(&temp, "hooks.stop.duplicate_detection_threshold", "5");

    let session_id = "duplicate-detection-test-session";

    // Add 10 entries (above threshold of 5)
    for i in 0..10 {
        add_entry(&temp, &format!("Test entry number {}", i));
    }

    // Create some observations first
    send_hook(
        &temp,
        "PostToolUse",
        &write_tool_input(session_id, "/src/main.rs"),
    );

    // Try to stop - should be blocked for duplicate detection
    let stop_output = send_hook(&temp, "Stop", &stop_input(session_id));

    assert_stop_blocked(
        &stop_output,
        &["duplicate-detector", "duplicate", "Duplicate detection"],
        Some(&[
            "duplicate-detection required",
            "duplicate-detector",
            "Memory Cleanup",
        ]),
    );
}

/// Test that Stop is NOT blocked when entries are below threshold
#[test]
fn test_stop_not_blocked_below_duplicate_threshold() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Enable duplicate_detection in the existing hooks.stop table.
    set_config_value(&temp, "hooks.stop.duplicate_detection_enabled", "true");
    set_config_value(&temp, "hooks.stop.duplicate_detection_threshold", "20");

    let session_id = "below-duplicate-threshold-session";

    // Add only 5 entries (below threshold of 20)
    for i in 0..5 {
        add_entry(&temp, &format!("Test entry below threshold {}", i));
    }

    // Try to stop - should NOT be blocked
    let stop_output = send_hook(&temp, "Stop", &stop_input(session_id));

    assert_stop_allowed(&stop_output, Some("duplicate-detection"));
}

/// Test that Stop is not blocked when duplicate_detection is disabled (default)
#[test]
fn test_stop_not_blocked_without_duplicate_detection_config() {
    let temp = TempDir::new().unwrap();
    init_cas(&temp);

    // Don't enable duplicate_detection (use default config)
    let session_id = "no-duplicate-detection-session";

    // Add several entries
    for i in 0..25 {
        add_entry(&temp, &format!("Test entry {}", i));
    }

    // Try to stop - should NOT be blocked for duplicate detection
    let stop_output = send_hook(&temp, "Stop", &stop_input(session_id));

    assert_stop_allowed(&stop_output, Some("duplicate-detection"));
}
