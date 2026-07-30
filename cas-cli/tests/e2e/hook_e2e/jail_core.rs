use super::*;

/// A pending close transition must not block unrelated hook tool use.
#[test]
#[ignore]
fn test_pending_verification_allows_unrelated_tools() {
    let env = HookTestEnv::new();

    // Create a task
    let task_id = env.create_task("Test task for verification jail");
    println!("Created task: {}", task_id);

    // Set pending_verification to true
    env.set_pending_verification(&task_id, true);
    println!("Set pending_verification=true for {}", task_id);

    // Run hook directly - unrelated Read remains available.
    let (_, stdout) = env.run_pre_tool_use_read("test.txt");
    println!("Verification jail hook output: {}", stdout);

    assert!(
        !stdout.contains("deny"),
        "Pending verification must not block unrelated tool use: {}",
        stdout
    );
}

/// A verifier spawn claims authority but never clears a pending transition.
#[test]
#[ignore]
fn test_subagent_start_does_not_clear_pending_tasks() {
    let env = HookTestEnv::new();

    let task_owned = env.create_task("Owned jailed task");
    let task_other = env.create_task("Other jailed task");

    env.set_task_assignee(&task_owned, HOOK_TEST_SESSION_ID);
    env.set_task_assignee(&task_other, "other-session");

    env.set_pending_verification(&task_owned, true);
    env.set_pending_verification(&task_other, true);

    // Run SubagentStart (task-verifier)
    let (success, output) = env.run_subagent_start_task_verifier();
    assert!(success, "SubagentStart should succeed: {}", output);

    assert!(
        env.get_pending_verification(&task_owned),
        "spawn alone must not clear the owned task"
    );
    assert!(
        env.get_pending_verification(&task_other),
        "Other agent task should remain jailed"
    );
}

/// Test that tools work normally when NOT in verification jail
#[test]
#[ignore]
fn test_no_jail_allows_tools() {
    let env = HookTestEnv::new();

    // Create a task but DON'T set pending_verification
    let task_id = env.create_task("Normal task without jail");
    println!("Created task (no jail): {}", task_id);

    // Run hook directly - should NOT be blocked
    let (_, stdout) = env.run_pre_tool_use_read("test.txt");
    println!("Non-jailed hook output: {}", stdout);

    let is_blocked = stdout.contains("deny");
    assert!(
        !is_blocked,
        "Tools should not be blocked when not jailed: {}",
        stdout
    );
}

/// Pending state does not alter unrelated hook permissions before or after clear.
#[test]
#[ignore]
fn test_pending_verification_does_not_change_unrelated_tool_permission() {
    let env = HookTestEnv::new();

    // Create a task and set it to jailed
    let task_id = env.create_task("Task for jail release test");
    env.set_pending_verification(&task_id, true);
    println!("Created jailed task: {}", task_id);

    // Unrelated tools remain available while the exact close waits.
    let (_, stdout1) = env.run_pre_tool_use_read("test.txt");
    let was_blocked = stdout1.contains("deny");
    println!("First attempt blocked: {}", was_blocked);
    assert!(
        !was_blocked,
        "Pending verification must not block Read: {}",
        stdout1
    );

    // Now clear the jail
    env.set_pending_verification(&task_id, false);
    println!("Cleared jail for {}", task_id);

    // Tools should work now
    let (_, stdout2) = env.run_pre_tool_use_read("test.txt");
    let still_blocked = stdout2.contains("deny");
    println!("After jail release: blocked={}", still_blocked);
    assert!(
        !still_blocked,
        "Should not be blocked after jail release: {}",
        stdout2
    );
}

/// Regression for cas-c496: `Agent(task-verifier)` remains available, but
/// spawning it cannot itself resolve the task transition.
#[test]
#[ignore]
fn test_agent_tool_spawns_task_verifier_without_clearing_pending() {
    let env = HookTestEnv::new();

    let task_id = env.create_task("Task for Agent-tool unjail test");
    env.set_task_assignee(&task_id, HOOK_TEST_SESSION_ID);
    env.set_pending_verification(&task_id, true);
    assert!(
        env.get_pending_verification(&task_id),
        "precondition: task should be jailed"
    );

    // A plain Read remains available.
    let (_, stdout_read) = env.run_pre_tool_use_read("test.txt");
    assert!(
        !stdout_read.contains("deny"),
        "pending verification should not block unrelated tools: {}",
        stdout_read
    );

    // Agent(task-verifier) must be ALLOWED (same allowance as Task(task-verifier))
    // without clearing pending_verification as a side effect.
    let (_, stdout_agent) = env.run_pre_tool_use_agent_verifier();
    assert!(
        !stdout_agent.contains("deny"),
        "Agent tool spawning task-verifier must bypass jail (got: {})",
        stdout_agent
    );
    assert!(
        env.get_pending_verification(&task_id),
        "spawning Agent(task-verifier) must not clear pending_verification"
    );
}

// =============================================================================
// Worktree Merge Jail Tests
// =============================================================================

/// Test that worktree merge jail blocks tool use when task has pending_worktree_merge
#[test]
#[ignore]
fn test_worktree_merge_jail_blocks_tools() {
    let env = HookTestEnv::new();

    // Enable worktrees (required for worktree merge jail)
    env.enable_worktrees();

    // Create a task (simulating an epic that would have a worktree)
    let task_id = env.create_task("Epic for worktree merge jail");
    println!("Created task: {}", task_id);

    // Set pending_worktree_merge to true
    env.set_pending_worktree_merge(&task_id, true);
    println!("Set pending_worktree_merge=true for {}", task_id);

    // Verify the flag was set
    let (_, pending_merge) = env.get_task_jail_state(&task_id);
    assert!(
        pending_merge,
        "pending_worktree_merge should be true after setting"
    );

    // Run hook directly - should be blocked by worktree merge jail
    let (_, stdout) = env.run_pre_tool_use_read("README.md");
    println!("Worktree merge jail hook output: {}", stdout);

    let is_blocked = stdout.contains("deny");
    assert!(
        is_blocked,
        "Expected worktree merge jail to block tool use: {}",
        stdout
    );

    // Verify message mentions worktree
    assert!(
        stdout.to_lowercase().contains("worktree"),
        "Worktree jail message should mention 'worktree': {}",
        stdout
    );
}

/// Test that worktree merge jail is released when pending_worktree_merge is cleared
#[test]
#[ignore]
fn test_worktree_merge_jail_release() {
    let env = HookTestEnv::new();

    env.enable_worktrees();

    // Create a task and set it to worktree merge jail
    let task_id = env.create_task("Task for worktree merge release test");
    env.set_pending_worktree_merge(&task_id, true);
    println!("Created worktree-jailed task: {}", task_id);

    // Verify blocked
    let (_, stdout1) = env.run_pre_tool_use_read("test.txt");
    let was_blocked = stdout1.contains("deny");
    println!("Before release: blocked={}", was_blocked);
    assert!(
        was_blocked,
        "Should be blocked while worktree-jailed: {}",
        stdout1
    );

    // Clear the jail
    env.set_pending_worktree_merge(&task_id, false);
    println!("Cleared worktree merge jail for {}", task_id);

    // Tools should work now
    let (_, stdout2) = env.run_pre_tool_use_read("test.txt");
    let still_blocked = stdout2.contains("deny");
    println!("After worktree jail release: blocked={}", still_blocked);
    assert!(
        !still_blocked,
        "Should not be blocked after worktree jail release: {}",
        stdout2
    );
}

// =============================================================================
// Agent Isolation Tests
// =============================================================================

/// Test that verification jail for one agent doesn't affect other agents
#[test]
#[ignore]
fn test_jail_agent_isolation() {
    let env = HookTestEnv::new();

    // Create a task jailed for a DIFFERENT agent
    let task_id = env.create_task("Task jailed for other agent");
    env.set_task_assignee(&task_id, "other-agent-session-id");
    env.set_pending_verification(&task_id, true);
    println!("Created task jailed for other agent: {}", task_id);

    // Our agent (HOOK_TEST_SESSION_ID) should NOT be affected
    let (_, stdout) = env.run_pre_tool_use_read("test.txt");
    println!("Agent isolation hook output: {}", stdout);

    let is_blocked = stdout.contains("deny");
    assert!(
        !is_blocked,
        "Our agent should not be blocked by another agent's jailed task: {}",
        stdout
    );
}

// =============================================================================
// Combined Jail Tests
// =============================================================================

/// Test both verification and worktree jails together
#[test]
#[ignore]
fn test_both_jails_block_tools() {
    let env = HookTestEnv::new();

    env.enable_worktrees();

    // Create two tasks with different jail types
    let task1 = env.create_task("Task with verification jail");
    let task2 = env.create_task("Task with worktree jail");

    env.set_pending_verification(&task1, true);
    env.set_pending_worktree_merge(&task2, true);
    println!("Created tasks with both jail types: {}, {}", task1, task2);

    // Should be blocked
    let (_, stdout) = env.run_pre_tool_use_read("test.txt");
    println!("Both jails hook output: {}", stdout);

    let is_blocked = stdout.contains("deny");
    assert!(
        is_blocked,
        "Expected jail to block tool use, got: {}",
        stdout
    );

    // Clear verification jail only - should still be blocked by worktree jail
    env.set_pending_verification(&task1, false);
    let (_, stdout2) = env.run_pre_tool_use_read("test.txt");
    let still_blocked = stdout2.contains("deny");
    println!(
        "After clearing verification jail: blocked={}",
        still_blocked
    );
    assert!(
        still_blocked,
        "Should still be blocked by worktree jail: {}",
        stdout2
    );

    // Clear worktree jail too - should work now
    env.set_pending_worktree_merge(&task2, false);
    let (_, stdout3) = env.run_pre_tool_use_read("test.txt");
    let final_blocked = stdout3.contains("deny");
    println!("After clearing both jails: blocked={}", final_blocked);
    assert!(
        !final_blocked,
        "Should not be blocked after clearing both jails: {}",
        stdout3
    );
}

// =============================================================================
// Bug-Finding Tests: Edge Cases and Race Conditions
// =============================================================================

// BUG TEST: Multiple tasks jailed - clearing ONE should NOT release jail.
// This tests that jail is global across ALL tasks with pending_verification.
