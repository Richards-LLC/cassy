//! Tests for cas-55ac: Per-turn UserPromptSubmit reminder for factory supervisors.
//!
//! The `UserPromptSubmit` hook now emits a ≤512B role reminder when
//! `is_factory_agent && role==supervisor`. The reminder refreshes Hard Rules
//! every turn to counter mid-session drift. Non-supervisor and non-factory
//! sessions remain unchanged (returns empty).

use crate::test_support::TestEnvGuard;
use cas_core::hooks::types::{HookInput, HookSpecificOutput};

use crate::hooks::handlers::handle_user_prompt_submit;

fn supervisor_input() -> HookInput {
    HookInput {
        session_id: "test-supervisor-session".to_string(),
        cwd: "/test".to_string(),
        hook_event_name: "UserPromptSubmit".to_string(),
        user_prompt: Some("What tasks are ready?".to_string()),
        agent_role: Some("supervisor".to_string()),
        ..HookInput::default()
    }
}

fn worker_input() -> HookInput {
    HookInput {
        session_id: "test-worker-session".to_string(),
        cwd: "/test".to_string(),
        hook_event_name: "UserPromptSubmit".to_string(),
        user_prompt: Some("What tasks are ready?".to_string()),
        agent_role: Some("worker".to_string()),
        ..HookInput::default()
    }
}

fn non_factory_input() -> HookInput {
    HookInput {
        session_id: "test-solo-session".to_string(),
        cwd: "/test".to_string(),
        hook_event_name: "UserPromptSubmit".to_string(),
        user_prompt: Some("What tasks are ready?".to_string()),
        agent_role: None,
        ..HookInput::default()
    }
}

/// AC1: Supervisor receives a non-empty UserPromptSubmit additionalContext.
/// AC4: Output byte length ≤ 512.
/// Both checked together since they require the same setup.
#[test]
fn supervisor_gets_reminder_within_512_bytes() {
    let mut env = TestEnvGuard::new();
    super::set_role_env(&mut env, Some("supervisor"));
    let input = supervisor_input();
    let output = handle_user_prompt_submit(&input, None).unwrap();

    let context = match &output.hook_specific_output {
        Some(HookSpecificOutput::UserPromptSubmit { additional_context }) => additional_context,
        other => panic!("Expected UserPromptSubmit hookSpecificOutput, got: {other:?}"),
    };

    let byte_len = context.as_bytes().len();
    assert!(
        byte_len <= 512,
        "Supervisor reminder exceeds 512 bytes: {byte_len} bytes\n---\n{context}\n---"
    );
}

/// AC1: The reminder contains all 6 Hard Rule keywords.
#[test]
fn supervisor_reminder_contains_all_6_hard_rule_keywords() {
    let mut env = TestEnvGuard::new();
    super::set_role_env(&mut env, Some("supervisor"));
    super::set_supervisor_cli_env(&mut env, Some("claude"));
    let input = supervisor_input();
    let output = handle_user_prompt_submit(&input, None).unwrap();

    let context = match &output.hook_specific_output {
        Some(HookSpecificOutput::UserPromptSubmit { additional_context }) => additional_context,
        other => panic!("Expected UserPromptSubmit hookSpecificOutput, got: {other:?}"),
    };

    // All 6 Hard Rule keywords must appear in the reminder.
    let keywords = [
        "AskUserQuestion",
        "BLOCKED",
        "end your turn",
        "mcp__cas__coordination",
        "SendMessage",
        "close",
        "implement",
        "poll",
    ];
    for kw in keywords {
        assert!(
            context.contains(kw),
            "Supervisor reminder missing keyword '{kw}':\n---\n{context}\n---"
        );
    }
}

/// EPIC cas-8888 (cas-fd9f): the load-bearing regression test — this
/// every-turn reminder was hardcoded to Claude's `mcp__cas__` prefix, so a
/// Grok supervisor was told a tool call it cannot make on EVERY turn.
#[test]
fn grok_supervisor_reminder_uses_cas_prefix() {
    let mut env = TestEnvGuard::new();
    super::set_role_env(&mut env, Some("supervisor"));
    super::set_supervisor_cli_env(&mut env, Some("grok"));
    let input = supervisor_input();
    let output = handle_user_prompt_submit(&input, None).unwrap();

    let context = match &output.hook_specific_output {
        Some(HookSpecificOutput::UserPromptSubmit { additional_context }) => additional_context,
        other => panic!("Expected UserPromptSubmit hookSpecificOutput, got: {other:?}"),
    };

    assert!(
        context.contains("cas__coordination"),
        "grok supervisor reminder must use its own cas__ prefix:\n---\n{context}\n---"
    );
    assert!(
        !context.contains("mcp__cas__coordination") && !context.contains("mcp__cs__coordination"),
        "grok supervisor reminder must NOT carry another harness's prefix:\n---\n{context}\n---"
    );
}

/// AC2: Worker sessions return empty (no reminder injected).
#[test]
fn worker_does_not_get_supervisor_reminder() {
    let mut env = TestEnvGuard::new();
    super::set_role_env(&mut env, Some("worker"));
    let input = worker_input();
    let output = handle_user_prompt_submit(&input, None).unwrap();

    let json = serde_json::to_string(&output).unwrap();
    assert_eq!(
        json, "{}",
        "Worker session must return empty HookOutput, got: {json}"
    );
}

/// AC2: Non-factory (generic Claude) sessions return empty.
#[test]
fn non_factory_does_not_get_supervisor_reminder() {
    let mut env = TestEnvGuard::new();
    super::set_role_env(&mut env, None);
    let input = non_factory_input();
    let output = handle_user_prompt_submit(&input, None).unwrap();

    let json = serde_json::to_string(&output).unwrap();
    assert_eq!(
        json, "{}",
        "Non-factory session must return empty HookOutput, got: {json}"
    );
}

/// AC1: Supervisor reminder includes identity placeholders (name, team).
/// The reminder text must contain the identity section so the supervisor
/// knows which role it's been assigned.
#[test]
fn supervisor_reminder_contains_identity_context() {
    let mut env = TestEnvGuard::new();
    super::set_role_env(&mut env, Some("supervisor"));
    let input = supervisor_input();
    let output = handle_user_prompt_submit(&input, None).unwrap();

    let context = match &output.hook_specific_output {
        Some(HookSpecificOutput::UserPromptSubmit { additional_context }) => additional_context,
        other => panic!("Expected UserPromptSubmit hookSpecificOutput, got: {other:?}"),
    };

    // The reminder must include an identity line (supervisor of team ...)
    assert!(
        context.contains("supervisor"),
        "Reminder must identify the role as 'supervisor':\n---\n{context}\n---"
    );
}
