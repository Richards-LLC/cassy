//! Tests for cas-f32b / cas-73c8: PreToolUse `SendMessage` auto-route onto
//! the CAS prompt queue in factory mode.
//!
//! Before cas-f32b, the hook denied `SendMessage` with guidance telling the
//! agent to call `mcp__cas__coordination action=message` instead. Claude
//! Code's Team Coordination system-reminder points agents at `SendMessage`,
//! so they often retried the denied call instead of switching tools —
//! wedging workers on a deny loop (observed 2026-04-23, gabber-studio).
//!
//! cas-f32b: parse the call, enqueue onto the CAS prompt queue, notify the
//! daemon. cas-73c8: return `allow` + `additionalContext` with an
//! "AUTO-ROUTED — do not retry" receipt so success is not wrapped in
//! Claude Code's `<error>` envelope (deny always surfaces as tool error).
//! On any failure we fall back to deny-with-guidance so the agent's
//! message is never silently dropped.

use crate::hooks::handlers::handle_pre_tool_use;
use cas_core::hooks::types::HookInput;

use crate::test_support::TestEnvGuard;

fn send_message_input(tool_input: Option<serde_json::Value>) -> HookInput {
    HookInput {
        session_id: "test-session".into(),
        cwd: "/test".into(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some("SendMessage".into()),
        tool_input,
        ..HookInput::default()
    }
}

fn permission_decision(out: &cas_core::hooks::types::HookOutput) -> Option<String> {
    let specific = out.hook_specific_output.as_ref()?;
    let value = serde_json::to_value(specific).ok()?;
    value
        .get("permissionDecision")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn deny_reason(out: &cas_core::hooks::types::HookOutput) -> Option<String> {
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

fn additional_context(out: &cas_core::hooks::types::HookOutput) -> Option<String> {
    let specific = out.hook_specific_output.as_ref()?;
    let value = serde_json::to_value(specific).ok()?;
    value
        .get("additionalContext")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

// ============================================================================
// Happy path — SendMessage with full args enqueues and returns AUTO-ROUTED
// as tool success (allow + additionalContext), not deny/`<error>`.
// ============================================================================

#[test]
fn send_message_auto_routes_onto_prompt_queue() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("worker")),
        ("CAS_AGENT_NAME", Some("test-worker-99")),
        ("CAS_FACTORY_SESSION", None),
    ]);

    let tmp = tempfile::tempdir().expect("tempdir");
    let input = send_message_input(Some(serde_json::json!({
        "to": "supervisor",
        "message": "task cas-abcd assigned, starting work",
        "summary": "task started",
    })));

    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    // cas-73c8: success must not use deny (deny → `<error>` envelope in CC).
    assert_eq!(
        permission_decision(&out).as_deref(),
        Some("allow"),
        "successful auto-route must be allow, not deny/error"
    );
    assert!(
        deny_reason(&out).is_none(),
        "successful auto-route must not emit a deny reason"
    );
    let ctx = additional_context(&out).expect("expected additionalContext receipt");
    assert!(
        ctx.contains("AUTO-ROUTED"),
        "context should signal auto-route success, got: {ctx}"
    );
    assert!(
        ctx.contains("supervisor"),
        "receipt should name the target: {ctx}"
    );
    assert!(
        ctx.contains("DO NOT"),
        "receipt must tell the agent not to retry: {ctx}"
    );

    // Verify a row actually landed on the prompt queue.
    let queue = crate::store::open_prompt_queue_store(tmp.path()).expect("queue opens");
    let pending = queue
        .peek_for_targets(&["supervisor"], None, 10)
        .expect("peek pending");
    assert_eq!(
        pending.len(),
        1,
        "expected one queued message for supervisor, got {}",
        pending.len()
    );
    assert_eq!(pending[0].prompt, "task cas-abcd assigned, starting work");
    assert_eq!(pending[0].source, "test-worker-99");
}

// ============================================================================
// Structured-payload happy path — {type:"shutdown_response",...} serializes.
// ============================================================================

#[test]
fn send_message_serializes_structured_payload() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("worker")),
        ("CAS_AGENT_NAME", Some("test-worker-22")),
        ("CAS_FACTORY_SESSION", None),
    ]);

    let tmp = tempfile::tempdir().expect("tempdir");
    let input = send_message_input(Some(serde_json::json!({
        "to": "supervisor",
        "message": {
            "type": "plan_approval_response",
            "request_id": "req-7",
            "approve": true,
        },
    })));

    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    assert_eq!(
        permission_decision(&out).as_deref(),
        Some("allow"),
        "structured messages must auto-route as success"
    );
    assert!(
        additional_context(&out)
            .expect("context")
            .contains("AUTO-ROUTED"),
        "structured messages must auto-route too"
    );

    let queue = crate::store::open_prompt_queue_store(tmp.path()).expect("queue opens");
    let pending = queue
        .peek_for_targets(&["supervisor"], None, 10)
        .expect("peek pending");
    assert_eq!(pending.len(), 1);
    assert!(
        pending[0].prompt.contains("plan_approval_response")
            && pending[0].prompt.contains("req-7"),
        "structured payload must serialize to JSON, got: {}",
        pending[0].prompt
    );
}

// ============================================================================
// Negative: missing required fields fall back to deny-with-guidance.
// ============================================================================

#[test]
fn send_message_missing_target_falls_back_to_guidance() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("worker")),
        ("CAS_AGENT_NAME", Some("test-worker-3")),
    ]);

    let tmp = tempfile::tempdir().expect("tempdir");
    let input = send_message_input(Some(serde_json::json!({
        "message": "no recipient specified",
    })));

    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    let reason = deny_reason(&out).expect("deny");
    assert!(
        reason.contains("SendMessage is disabled"),
        "missing `to` must return the fallback guidance, got: {reason}"
    );
    assert!(
        !reason.contains("AUTO-ROUTED"),
        "fallback must not claim success"
    );
}

#[test]
fn send_message_missing_body_falls_back_to_guidance() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("worker")),
        ("CAS_AGENT_NAME", Some("test-worker-4")),
    ]);

    let tmp = tempfile::tempdir().expect("tempdir");
    let input = send_message_input(Some(serde_json::json!({
        "to": "supervisor",
    })));

    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    let reason = deny_reason(&out).expect("deny");
    assert!(
        reason.contains("SendMessage is disabled"),
        "missing `message` must return the fallback guidance, got: {reason}"
    );
}

#[test]
fn send_message_no_tool_input_falls_back_to_guidance() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("supervisor")),
        ("CAS_AGENT_NAME", Some("test-super-1")),
    ]);

    let tmp = tempfile::tempdir().expect("tempdir");
    let input = send_message_input(None);

    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    let reason = deny_reason(&out).expect("deny");
    assert!(reason.contains("SendMessage is disabled"));
}

// ============================================================================
// Scope: non-factory sessions must not auto-route — they fall through to
// Claude Code's normal flow (SendMessage works there).
// ============================================================================

#[test]
fn send_message_outside_factory_falls_through() {
    let _env = TestEnvGuard::with_optional_vars(&[("CAS_AGENT_ROLE", None), ("CAS_AGENT_NAME", None)]);

    let tmp = tempfile::tempdir().expect("tempdir");
    let input = send_message_input(Some(serde_json::json!({
        "to": "some-teammate",
        "message": "hi from solo session",
    })));

    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    // Outside factory mode we must not emit any permission decision for
    // SendMessage — it passes through to Claude Code's normal handling.
    assert!(
        deny_reason(&out).is_none(),
        "SendMessage outside factory mode must not be denied by our hook"
    );
}
