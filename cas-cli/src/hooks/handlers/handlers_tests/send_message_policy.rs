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

/// GH #1887: replay the audited time and stale-clipboard incidents through
/// the sending hook, using real transcript records rather than a policy mock.
#[test]
fn operator_claims_require_same_turn_evidence_gh1887() {
    let _env = TestEnvGuard::with_optional_vars(&[("CAS_AGENT_ROLE", Some("supervisor"))]);
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("transcript.jsonl");
    let user = serde_json::json!({"type":"user","message":{"role":"user","content":"status?"}});
    let call = serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[
        {"type":"tool_use","id":"tool-clock","name":"Bash","input":{"command":"date"}}
    ]}});
    let result = serde_json::json!({"type":"user","message":{"role":"user","content":[
        {"type":"tool_result","tool_use_id":"tool-clock","content":"11:20 AM ET","is_error":false}
    ]}});
    let failed = serde_json::json!({"type":"user","message":{"role":"user","content":[
        {"type":"tool_result","tool_use_id":"tool-clock","content":"command failed","is_error":true}
    ]}});
    for (records, message, denied) in [
        (vec![user.clone()], "12:20 → 12:38 PM ET", true),
        (vec![user.clone()], "Still in your clipboard", true),
        (vec![user.clone()], "Deleted 133 branches, SHAs saved", true),
        (vec![user.clone()], "[unverified] 12:20 → 12:38 PM ET", false),
        (vec![user.clone(), call.clone(), result.clone()], "11:20 AM ET [evidence:tool-clock]", false),
        (vec![user.clone(), call.clone(), result.clone()], "11:20 AM ET (`date`)", false),
        (vec![user.clone(), call.clone(), result.clone(), user.clone()], "11:20 AM ET (`date`)", true),
        (vec![user.clone(), call.clone(), result.clone()], "12:20 PM ET (`date`)", true),
        (vec![user.clone(), call.clone(), result.clone(), user.clone()], "11:20 AM ET [evidence:tool-clock]", true),
        (vec![user.clone(), call.clone(), failed.clone()], "SHAs saved [evidence:tool-clock]", true),
        (vec![user.clone(), call.clone(), failed], "SHAs saved", true),
        (vec![user.clone(), call.clone(), result.clone()], "11:20 AM ET [evidence:invented]", true),
        (vec![user.clone(), call.clone(), result.clone()], "12:20 PM ET [evidence:tool-clock]", true),
        (vec![user.clone(), call.clone(), result.clone()], "Still in your clipboard [evidence:tool-clock]", true),
        (vec![user.clone(), call, result], "11:20 AM ET [evidence:tool-clock]\n\nDeleted 133 branches", true),
        (vec![user], "Investigating the queue", false),
    ] {
        std::fs::write(&transcript, records.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")).unwrap();
        let input: HookInput = serde_json::from_value(serde_json::json!({
            "session_id":"evidence-session", "hook_event_name":"PreToolUse",
            "transcript_path":transcript, "cwd":tmp.path(),
            "tool_name":"mcp__cs__coordination", "tool_input":{
                "action":"message", "target":"operator", "summary":"status", "message":message
            }
        })).unwrap();
        let out = handle_pre_tool_use(&input, Some(tmp.path())).unwrap();
        assert_eq!(deny_reason(&out).is_some(), denied, "{message}");
        if denied {
            assert!(crate::gates::codes_in(&deny_reason(&out).unwrap()).contains(&"H-47"));
        } else {
            // No rewrite of a proven caller payload.
            assert!(serde_json::to_value(out).unwrap()["hookSpecificOutput"]["updatedInput"].is_null());
        }
    }
}

#[test]
fn clipboard_evidence_expires_after_overwrite_gh1887() {
    let _env = TestEnvGuard::with_optional_vars(&[("CAS_AGENT_ROLE", Some("supervisor"))]);
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("codex.jsonl");
    let user = serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"status?"}});
    let call = |id: &str, command: &str| serde_json::json!({"type":"response_item","payload":{
        "type":"function_call","call_id":id,"name":"exec_command","arguments":serde_json::json!({"cmd":command}).to_string()
    }});
    let result = |id: &str| serde_json::json!({"type":"response_item","payload":{
        "type":"function_call_output","call_id":id,"output":"Process exited with code 0\nOutput:\nrun command"
    }});
    for (records, denied) in [
        (vec![user.clone(), call("paste", "wl-paste"), result("paste")], false),
        (vec![user.clone(), call("paste", "wl-paste"), result("paste"), call("copy", "printf replacement | wl-copy"), result("copy")], true),
        (vec![user, call("copy", "printf replacement | wl-copy"), result("copy"), call("paste", "wl-paste"), result("paste")], false),
    ] {
        std::fs::write(&transcript, records.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")).unwrap();
        for target in ["operator", "commander:phone"] {
            for message in ["Still in your clipboard [evidence:paste]", "Still in your clipboard"] {
                let input: HookInput = serde_json::from_value(serde_json::json!({
                    "session_id":"evidence-session", "hook_event_name":"PreToolUse",
                    "transcript_path":transcript, "cwd":tmp.path(),
                    "tool_name":"mcp__cs__coordination", "tool_input":{
                        "action":"message", "target":target, "summary":"status", "message":message
                    }
                })).unwrap();
                assert_eq!(deny_reason(&handle_pre_tool_use(&input, Some(tmp.path())).unwrap()).is_some(), denied);
            }
        }
    }
}


#[test]
fn pane_claims_are_labelled_before_display_gh1887() {
    use crate::hooks::handlers::handle_message_display;
    let _env = TestEnvGuard::with_optional_vars(&[("CAS_AGENT_ROLE", Some("supervisor"))]);
    let tmp = tempfile::tempdir().unwrap();
    let input: HookInput = serde_json::from_value(serde_json::json!({
        "session_id":"pane-evidence", "hook_event_name":"MessageDisplay",
        "delta":"12:20 → 12:38 PM ET", "final":true
    })).unwrap();
    let out = handle_message_display(&input, Some(tmp.path())).unwrap();
    // MessageDisplay's machine protocol carries the labelled payload; no prose pin.
    assert!(serde_json::to_value(&out).unwrap()["hookSpecificOutput"]["updatedMessage"].is_string());
    assert!(crate::gates::codes_in(out.system_message.as_deref().unwrap_or_default()).contains(&"H-47"));
    let value = serde_json::to_value(&out).unwrap();
    let transformed = value["hookSpecificOutput"]["updatedMessage"].as_str().unwrap();
    // The caller's payload survives; the emitted label is understood by the
    // same real guard on a second display, without pinning its human wording.
    assert!(transformed.ends_with(input.display_text().unwrap()));
    let mut labelled = input;
    labelled.message = Some(transformed.to_owned());
    assert!(handle_message_display(&labelled, Some(tmp.path())).unwrap().hook_specific_output.is_none());
}


#[test]
fn pane_stop_refuses_uncited_claim_without_reentrant_loop_gh1887() {
    use crate::hooks::handlers::handle_stop;
    let _env = TestEnvGuard::with_optional_vars(&[("CAS_AGENT_ROLE", Some("supervisor"))]);
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("codex.jsonl");
    let rows = [
        serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"status?"}}),
        serde_json::json!({"type":"response_item","payload":{"type":"message","role":"assistant","channel":"final","content":[{"type":"output_text","text":"12:20 → 12:38 PM ET"}]}}),
    ];
    std::fs::write(&transcript, rows.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")).unwrap();
    for reentrant in [false, true] {
        if reentrant { std::fs::remove_file(&transcript).unwrap(); }
        let input: HookInput = serde_json::from_value(serde_json::json!({
            "session_id":"pane-evidence", "hook_event_name":"Stop", "transcript_path":transcript,
            "stop_hook_active":reentrant, "last_assistant_message":"12:20 → 12:38 PM ET"
        })).unwrap();
        let out = handle_stop(&input, Some(tmp.path())).unwrap();
        assert_eq!(out.decision.as_deref(), if reentrant { None } else { Some("block") });
        let notice = out.reason.as_deref().or(out.system_message.as_deref()).unwrap();
        assert!(crate::gates::codes_in(notice).contains(&"H-47"));
    }
}

#[test]
fn operator_evidence_scope_and_missing_transcript_gh1887() {
    let _env = TestEnvGuard::with_optional_vars(&[("CAS_AGENT_ROLE", None)]);
    for (role, target, denied) in [
        ("supervisor", "operator", true),
        ("supervisor", "commander:phone", true),
        ("supervisor", "worker-peer", false),
        ("worker", "operator", false),
    ] {
        let input: HookInput = serde_json::from_value(serde_json::json!({
            "agent_role":role, "hook_event_name":"PreToolUse", "tool_name":"mcp__cs__coordination",
            "transcript_path":"/nonexistent/gh1887.jsonl", "tool_input":{
                "action":"message", "target":target, "summary":"status", "message":"12:20 PM ET [evidence:missing]"
            }
        })).unwrap();
        assert_eq!(deny_reason(&handle_pre_tool_use(&input, None).unwrap()).is_some(), denied);
    }
}

/// GH #1915: Claude Code exposes commands and results to the model, but not
/// tool-use IDs. Replay that transcript shape through the operator's Stop hook.
#[test]
fn pane_stop_matches_visible_result_without_tool_id_gh1915() {
    use crate::hooks::handlers::handle_stop;
    let _env = TestEnvGuard::with_optional_vars(&[("CAS_AGENT_ROLE", Some("supervisor"))]);
    let tmp = tempfile::tempdir().unwrap();
    let transcript = tmp.path().join("claude.jsonl");
    for (message, denied) in [
        ("`git log -1 --oneline` and `git rev-list --count HEAD`: d39a718, 133 commits.", false),
        ("`git rev-list --count HEAD`: 134 commits.", true),
        ("`git log -2 --oneline`: 133 commits.", true),
        ("`git log -1 --oneline`: abcdefa.", true),
        // Same digit fragments as the real SHA must not prove a different SHA.
        ("`git log -1 --oneline`: e39b718.", true),
    ] {
        let rows = [
            serde_json::json!({"type":"user","sessionId":"claude-evidence","message":{"role":"user","content":[{"type":"text","text":"check the current commit and count"}]}}),
            serde_json::json!({"type":"assistant","sessionId":"claude-evidence","message":{"role":"assistant","content":[
                {"type":"tool_use","id":"toolu_hidden_log","name":"Bash","input":{"command":"git log -1 --oneline"}},
                {"type":"tool_use","id":"toolu_hidden_count","name":"Bash","input":{"command":"git rev-list --count HEAD"}}
            ]}}),
            serde_json::json!({"type":"user","sessionId":"claude-evidence","message":{"role":"user","content":[
                {"type":"tool_result","tool_use_id":"toolu_hidden_log","content":"d39a718 release hotfix","is_error":false},
                {"type":"tool_result","tool_use_id":"toolu_hidden_count","content":"133","is_error":false}
            ]}}),
            serde_json::json!({"type":"assistant","sessionId":"claude-evidence","message":{"role":"assistant","content":[{"type":"text","text":message}]}}),
        ];
        std::fs::write(&transcript, rows.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")).unwrap();
        let input: HookInput = serde_json::from_value(serde_json::json!({
            "session_id":"claude-evidence", "hook_event_name":"Stop", "transcript_path":transcript,
            "last_assistant_message":message
        })).unwrap();
        let out = handle_stop(&input, Some(tmp.path())).unwrap();
        assert_eq!(out.decision.as_deref() == Some("block"), denied, "{message}");
        if denied {
            assert!(crate::gates::codes_in(out.reason.as_deref().unwrap()).contains(&"H-47"));
        }
    }
}
