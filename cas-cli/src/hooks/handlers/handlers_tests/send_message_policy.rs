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

#[test]
fn factory_worker_native_messages_follow_child_ownership() {
    use crate::hooks::handlers::{handle_subagent_start, handle_subagent_stop};
    use cas_types::{Agent, AgentRole};
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("worker")), ("CAS_FACTORY_SESSION", None),
    ]);
    let root = tempfile::tempdir().unwrap();
    let agents = crate::store::open_agent_store(root.path()).unwrap();
    agents.init().unwrap();
    agents.register(&Agent::new_with_role("test-session".into(), "parent-worker".into(), AgentRole::Worker)).unwrap();
    agents.register(&Agent::new_with_role("peer-session".into(), "registered-peer".into(), AgentRole::Worker)).unwrap();
    let mut start = HookInput {
        session_id: "test-session".into(), agent_id: Some("a-owned".into()),
        agent_type: Some("general-purpose".into()), hook_event_name: "SubagentStart".into(),
        ..HookInput::default()
    };
    handle_subagent_start(&start, Some(root.path())).unwrap();
    start.session_id = "other-parent".into();
    start.agent_id = Some("a-foreign".into());
    handle_subagent_start(&start, Some(root.path())).unwrap();
    // Even a claimed child cannot become a native route to a factory peer.
    start.session_id = "test-session".into();
    start.agent_id = Some("registered-peer".into());
    handle_subagent_start(&start, Some(root.path())).unwrap();
    for (args, allowed) in [
        (serde_json::json!({"recipient":"a-owned", "type":"message"}), true),
        (serde_json::json!({"to":"a-owned"}), true),
        (serde_json::json!({"to":"a-foreign"}), false),
        (serde_json::json!({"to":"registered-peer"}), false),
        (serde_json::json!({"to":"supervisor"}), false),
        (serde_json::json!({"to":"all_workers"}), false),
        (serde_json::json!({"to":"a-owned", "type":"broadcast"}), false),
        (serde_json::json!({"recipient":"a-owned", "to":"a-foreign"}), false),
        (serde_json::json!({}), false),
    ] {
        let mut input = send_message_input(Some(args));
        input.agent_role = Some("worker".into());
        let output = handle_pre_tool_use(&input, Some(root.path())).unwrap();
        assert_eq!(deny_reason(&output).is_none(), allowed, "{:?}", input.tool_input);
        if !allowed {
            assert!(crate::gates::codes_in(&deny_reason(&output).unwrap()).contains(&"H-21"));
        }
    }
    start.agent_id = Some("a-owned".into());
    start.hook_event_name = "SubagentStop".into();
    handle_subagent_stop(&start, Some(root.path())).unwrap();
    let mut input = send_message_input(Some(serde_json::json!({"to":"a-owned"})));
    input.agent_role = Some("worker".into());
    // Completion preserves ownership: SendMessage resumes the same native child.
    assert!(deny_reason(&handle_pre_tool_use(&input, Some(root.path())).unwrap()).is_none());
    input.tool_input = Some(serde_json::json!({"to":"a-foreign"}));
    assert!(deny_reason(&handle_pre_tool_use(&input, Some(root.path())).unwrap()).is_some());
    assert!(agents.get("a-owned").is_err());
    assert_eq!(agents.get("test-session").unwrap().role, AgentRole::Worker);
}
