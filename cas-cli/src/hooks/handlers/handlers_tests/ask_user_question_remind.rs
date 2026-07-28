//! Tests for blocking PreToolUse `AskUserQuestion` in factory mode.
//!
//! `AskUserQuestion` has no human UI surface for factory agents. It surfaces as
//! a self-directed permission prompt and pauses the caller, so the hook denies
//! it for both supervisors and workers with role-tailored guidance.

use crate::hooks::handlers::handle_pre_tool_use;
use crate::test_support::TestEnvGuard;
use cas_core::hooks::types::{HookInput, HookOutput};

// ============================================================================
// Input builder
// ============================================================================

fn hook_input(tool_name: &str) -> HookInput {
    HookInput {
        session_id: "test-session".into(),
        cwd: "/test".into(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some(tool_name.into()),
        tool_input: Some(serde_json::json!({
            "question": "Should I proceed?",
            "options": [{"label": "Yes"}, {"label": "No"}]
        })),
        ..HookInput::default()
    }
}

fn deny_reason(out: &HookOutput) -> Option<String> {
    let specific = out.hook_specific_output.as_ref()?;
    let value = serde_json::to_value(specific).ok()?;
    let decision = value.get("permissionDecision")?.as_str()?;
    if decision != "deny" {
        return None;
    }
    value
        .get("permissionDecisionReason")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

// ============================================================================
// Factory agents + AskUserQuestion -> deny with actionable guidance.
// ============================================================================

#[test]
fn factory_supervisor_ask_user_question_is_denied_with_plain_text_guidance() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("supervisor")),
        ("CAS_FACTORY_SUPERVISOR_CLI", Some("claude")),
    ]);

    let tmp = tempfile::tempdir().expect("tempdir");
    let out =
        handle_pre_tool_use(&hook_input("AskUserQuestion"), Some(tmp.path())).expect("handler ok");

    let reason = deny_reason(&out).expect("supervisor AskUserQuestion must be denied");
    assert!(
        reason.contains("AskUserQuestion cannot reach the human in factory mode"),
        "deny reason must explain the factory-mode failure: {reason}"
    );
    assert!(
        reason.contains("plain text") && reason.contains("END YOUR TURN"),
        "supervisor guidance must say to ask in plain text and end turn: {reason}"
    );
    assert!(
        reason.contains("director relays"),
        "supervisor guidance must mention director relay: {reason}"
    );
    assert!(
        reason.contains("mcp__cas__coordination action=message"),
        "supervisor guidance must include its own coordination prefix: {reason}"
    );
}

#[test]
fn factory_supervisor_ask_user_question_is_denied_from_hook_input_role() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", None),
        ("CAS_FACTORY_SUPERVISOR_CLI", Some("claude")),
    ]);

    let mut input = hook_input("AskUserQuestion");
    input.agent_role = Some("supervisor".into());
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");

    let reason = deny_reason(&out).expect("HookInput supervisor role must deny AskUserQuestion");
    assert!(
        reason.contains("plain text") && reason.contains("END YOUR TURN"),
        "field-path supervisor guidance must say to ask in plain text and end turn: {reason}"
    );
}

#[test]
fn grok_factory_supervisor_ask_user_question_uses_grok_prefix() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("supervisor")),
        ("CAS_FACTORY_SUPERVISOR_CLI", Some("grok")),
    ]);

    let tmp = tempfile::tempdir().expect("tempdir");
    let out =
        handle_pre_tool_use(&hook_input("AskUserQuestion"), Some(tmp.path())).expect("handler ok");

    let reason = deny_reason(&out).expect("grok supervisor AskUserQuestion must be denied");
    assert!(
        reason.contains("cas__coordination action=message"),
        "grok supervisor guidance must use its own cas__ prefix: {reason}"
    );
    assert!(
        !reason.contains("mcp__cas__coordination") && !reason.contains("mcp__cs__coordination"),
        "grok supervisor guidance must not carry another harness prefix: {reason}"
    );
}

#[test]
fn factory_worker_ask_user_question_is_denied_with_supervisor_message_guidance() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("worker")),
        ("CAS_FACTORY_WORKER_CLI", Some("codex")),
    ]);

    let tmp = tempfile::tempdir().expect("tempdir");
    let out =
        handle_pre_tool_use(&hook_input("AskUserQuestion"), Some(tmp.path())).expect("handler ok");

    let reason = deny_reason(&out).expect("worker AskUserQuestion must be denied");
    assert!(
        reason.contains("AskUserQuestion is blocked in factory mode"),
        "worker deny reason must explain the block: {reason}"
    );
    assert!(
        reason.contains("mcp__cs__coordination action=message target=<supervisor>"),
        "worker guidance must point at supervisor coordination message: {reason}"
    );
}

// ============================================================================
// Non-target calls are untouched.
// ============================================================================

#[test]
fn non_factory_agent_ask_user_question_is_untouched() {
    let _env = TestEnvGuard::with_optional_vars(&[("CAS_AGENT_ROLE", None)]);

    let tmp = tempfile::tempdir().expect("tempdir");
    let out =
        handle_pre_tool_use(&hook_input("AskUserQuestion"), Some(tmp.path())).expect("handler ok");

    assert!(
        deny_reason(&out).is_none(),
        "non-factory AskUserQuestion must not be denied"
    );
    assert!(
        out.hook_specific_output.is_none(),
        "non-factory AskUserQuestion must be untouched"
    );
}

#[test]
fn factory_supervisor_other_tool_is_untouched() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("supervisor")),
        ("CAS_FACTORY_SUPERVISOR_CLI", Some("claude")),
    ]);

    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_pre_tool_use(&hook_input("AskFollowupQuestions"), Some(tmp.path()))
        .expect("handler ok");

    assert!(
        deny_reason(&out).is_none(),
        "supervisor on a non-AskUserQuestion tool must not be denied"
    );
}

#[test]
fn factory_supervisor_ask_user_question_is_denied_without_cas_root() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("supervisor")),
        ("CAS_FACTORY_SUPERVISOR_CLI", Some("claude")),
    ]);

    let out = handle_pre_tool_use(&hook_input("AskUserQuestion"), None).expect("handler ok");

    let reason = deny_reason(&out).expect("cas_root=None AskUserQuestion must still be denied");
    assert!(
        reason.contains("plain text") && reason.contains("director relays"),
        "cas_root=None deny must keep actionable supervisor guidance: {reason}"
    );
}
