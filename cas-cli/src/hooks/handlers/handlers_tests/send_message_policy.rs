//! Factory hooks deny native `SendMessage` and direct agents to CAS coordination.

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

#[test]
fn factory_worker_send_message_is_denied_with_coordination_guidance() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("worker")),
        ("CAS_FACTORY_SESSION", None),
    ]);

    let tmp = tempfile::tempdir().expect("tempdir");
    let mut input = send_message_input(Some(serde_json::json!({
        "to": "all_workers",
        "message": "broadcast outside the durable coordination route",
        "summary": "broadcast",
    })));
    input.agent_role = Some("worker".into());

    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    let reason = deny_reason(&out).expect("factory worker SendMessage must be denied");
    assert!(
        reason.contains("mcp__cas__coordination action=message"),
        "deny reason must point at CAS coordination: {reason}"
    );
    assert!(
        reason.contains("summary=") && reason.contains("message="),
        "deny reason must explain the required coordination fields: {reason}"
    );

    let queue = crate::store::open_prompt_queue_store(tmp.path()).expect("queue opens");
    assert!(
        queue
            .peek_for_targets(&["all_workers"], None, 10)
            .expect("peek pending")
            .is_empty(),
        "denied worker SendMessage must not enqueue a parallel transport"
    );
}

// Supervisors must not return `allow` from this hook: Claude Code then runs
// its native SendMessage transport after the hook, which bypasses Cassy's
// durable queue and can strand an assignment. The CAS coordination command is
// the only supported supervisor-to-worker transport.
#[test]
fn factory_supervisor_send_message_to_worker_is_denied_with_cas_guidance() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", None),
        ("CAS_FACTORY_SUPERVISOR_CLI", Some("claude")),
        ("CAS_FACTORY_SESSION", None),
    ]);

    let tmp = tempfile::tempdir().expect("tempdir");
    let mut input = send_message_input(Some(serde_json::json!({
        "to": "test-worker-7",
        "message": "Task cas-f42e is assigned; start it now.",
        "summary": "assign cas-f42e",
    })));
    input.agent_role = Some("supervisor".into());

    let out = handle_pre_tool_use(&input, None).expect("handler ok");
    let reason = deny_reason(&out).expect("factory supervisor SendMessage must be denied");
    assert!(
        reason.contains("mcp__cas__coordination action=message target=<worker>")
            && reason.contains("summary=")
            && reason.contains("message="),
        "deny reason must provide the exact CAS message replacement: {reason}"
    );
    assert!(
        reason.contains("task action=update id=<task-id> assignee=<worker>"),
        "assignment guidance must point at task update assignee: {reason}"
    );
    assert!(
        !reason.contains("AUTO-ROUTED"),
        "supervisor SendMessage must not enqueue or claim a successful native transport"
    );

    let queue = crate::store::open_prompt_queue_store(tmp.path()).expect("queue opens");
    assert!(
        queue
            .peek_for_targets(&["test-worker-7"], None, 10)
            .expect("peek pending")
            .is_empty(),
        "denied supervisor SendMessage must not enqueue a parallel transport"
    );
}

#[test]
fn send_message_outside_factory_falls_through() {
    let _env =
        TestEnvGuard::with_optional_vars(&[("CAS_AGENT_ROLE", None), ("CAS_AGENT_NAME", None)]);

    let tmp = tempfile::tempdir().expect("tempdir");
    let input = send_message_input(Some(serde_json::json!({
        "to": "some-teammate",
        "message": "hi from solo session",
    })));

    let out = handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok");
    assert_eq!(
        permission_decision(&out),
        None,
        "SendMessage outside factory mode must pass through to the host harness"
    );
}
