//! Integration tests: fake HookInput → each `handle_*` handler → schema-valid
//! output.
//!
//! Sibling to `crates/cas-core/tests/hook_schema.rs`, which covers the
//! `HookOutput` constructor level. This file catches handler-level
//! wrong-builder mistakes — a handler that assembles an otherwise-valid
//! constructor but attaches it to a forbidden event (e.g., Stop emitting
//! `hookSpecificOutput`) would slip past constructor-only tests.
//!
//! Strategy: call every public handler with `cas_root: None`. In this shape,
//! handlers take their early-return path and produce `HookOutput::empty()` or
//! an equivalent schema-valid shape without touching any storage. That is
//! sufficient to prove:
//!   1. The routing dispatch in `handle_hook` is wired up correctly per event.
//!   2. Every handler returns valid JSON (no panics, no unknown keys, no
//!      hookSpecificOutput for Stop-family events).
//!
//! Richer integration coverage — handlers with a real `cas_root` exercising
//! the non-early-return paths — is intentionally out of scope here. Those
//! paths already go through the same tested `HookOutput` constructors and are
//! covered indirectly by the constructor tests and the existing
//! `hooks_test/` integration tests.

use cas::hooks::{
    HookInput, HookOutput, handle_notification, handle_permission_request, handle_post_tool_use,
    handle_pre_compact, handle_pre_tool_use, handle_session_end, handle_session_start, handle_stop,
    handle_subagent_start, handle_subagent_stop, handle_user_prompt_submit,
};
use serde_json::Value;

/// Events for which `hookSpecificOutput` MUST NOT appear (Stop-family).
const STOP_FAMILY: &[&str] = &["Stop", "SubagentStop", "PreCompact", "SessionEnd"];

/// Top-level keys allowed in any HookOutput.
const ALLOWED_TOP_LEVEL: &[&str] = &[
    "continue",
    "suppressOutput",
    "stopReason",
    "decision",
    "reason",
    "systemMessage",
    "permissionDecision",
    "hookSpecificOutput",
];

fn to_value(output: &HookOutput) -> Value {
    serde_json::from_str(&serde_json::to_string(output).expect("serializable")).expect("round-trip")
}

fn assert_schema_valid(value: &Value, event: &str) {
    let obj = value
        .as_object()
        .unwrap_or_else(|| panic!("{event}: output must be a JSON object, got {value}"));

    for key in obj.keys() {
        assert!(
            ALLOWED_TOP_LEVEL.contains(&key.as_str()),
            "{event}: unknown top-level key {key:?} in {value}"
        );
    }

    if STOP_FAMILY.contains(&event) {
        assert!(
            value.get("hookSpecificOutput").is_none(),
            "{event}: Stop-family event must not emit hookSpecificOutput, got {value}"
        );
    }

    // If hookSpecificOutput is present, it must carry hookEventName matching
    // the event being handled — otherwise Claude Code's validator rejects it.
    if let Some(hso) = value.get("hookSpecificOutput") {
        let name = hso
            .get("hookEventName")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{event}: hookSpecificOutput missing hookEventName"));
        assert_eq!(
            name, event,
            "{event}: hookEventName inside hookSpecificOutput must match the event"
        );
    }

    // decision must be approve|block when present.
    if let Some(decision) = value.get("decision").and_then(Value::as_str) {
        assert!(
            ["approve", "block"].contains(&decision),
            "{event}: decision must be approve|block, got {decision:?}"
        );
    }

    // permissionDecision must be allow|deny|ask when present at the top level.
    if let Some(pd) = value.get("permissionDecision").and_then(Value::as_str) {
        assert!(
            ["allow", "deny", "ask"].contains(&pd),
            "{event}: top-level permissionDecision must be allow|deny|ask, got {pd:?}"
        );
    }
}

/// cas-f3e3: built by PARSING the common wire envelope, not by struct literal.
///
/// Every field below appears verbatim on real Claude Code 2.1.224 payloads,
/// including `prompt_id`, which CAS deliberately does not declare — so this
/// helper also pins that an undeclared harness key can never make a hook fail
/// to deserialize.
fn base_input(event: &str) -> HookInput {
    serde_json::from_value(serde_json::json!({
        "session_id": "schema-test-session",
        "transcript_path": "/tmp/cas-schema-test/transcript.jsonl",
        "cwd": "/tmp/cas-schema-test",
        "prompt_id": "d323a56b-73b1-4afc-8b3f-32605be51b91",
        "permission_mode": "default",
        "hook_event_name": event,
    }))
    .expect("common hook envelope must deserialize")
}

#[test]
fn session_start_handler_emits_schema_valid_output() {
    let out = handle_session_start(&base_input("SessionStart"), None).expect("ok");
    assert_schema_valid(&to_value(&out), "SessionStart");
}

#[test]
fn session_end_handler_emits_schema_valid_output() {
    let out = handle_session_end(&base_input("SessionEnd"), None).expect("ok");
    assert_schema_valid(&to_value(&out), "SessionEnd");
}

#[test]
fn stop_handler_emits_schema_valid_output() {
    // Regression guard for baa540b: handle_stop must never emit
    // hookSpecificOutput, even when it wants to inject a codemap reminder.
    let out = handle_stop(&base_input("Stop"), None).expect("ok");
    assert_schema_valid(&to_value(&out), "Stop");
}

#[test]
fn subagent_start_handler_emits_schema_valid_output() {
    let out = handle_subagent_start(&base_input("SubagentStart"), None).expect("ok");
    assert_schema_valid(&to_value(&out), "SubagentStart");
}

#[test]
fn subagent_stop_handler_emits_schema_valid_output() {
    let out = handle_subagent_stop(&base_input("SubagentStop"), None).expect("ok");
    assert_schema_valid(&to_value(&out), "SubagentStop");
}

#[test]
fn post_tool_use_handler_emits_schema_valid_output() {
    let mut input = base_input("PostToolUse");
    input.tool_name = Some("Write".into());
    input.tool_input = Some(serde_json::json!({ "file_path": "/tmp/x" }));
    input.tool_response = Some(serde_json::json!({}));
    let out = handle_post_tool_use(&input, None).expect("ok");
    assert_schema_valid(&to_value(&out), "PostToolUse");
}

#[test]
fn pre_tool_use_handler_emits_schema_valid_output() {
    let mut input = base_input("PreToolUse");
    input.tool_name = Some("Bash".into());
    input.tool_input = Some(serde_json::json!({ "command": "ls" }));
    let out = handle_pre_tool_use(&input, None).expect("ok");
    assert_schema_valid(&to_value(&out), "PreToolUse");
}

#[test]
fn user_prompt_submit_handler_emits_schema_valid_output() {
    let mut input = base_input("UserPromptSubmit");
    input.user_prompt = Some("what should I do next?".into());
    let out = handle_user_prompt_submit(&input, None).expect("ok");
    assert_schema_valid(&to_value(&out), "UserPromptSubmit");
}

#[test]
fn permission_request_handler_emits_schema_valid_output() {
    let mut input = base_input("PermissionRequest");
    input.tool_name = Some("Bash".into());
    let out = handle_permission_request(&input, None).expect("ok");
    assert_schema_valid(&to_value(&out), "PermissionRequest");
}

#[test]
fn notification_handler_emits_schema_valid_output() {
    let out = handle_notification(&base_input("Notification"), None).expect("ok");
    assert_schema_valid(&to_value(&out), "Notification");
}

#[test]
fn pre_compact_handler_emits_schema_valid_output() {
    let out = handle_pre_compact(&base_input("PreCompact"), None).expect("ok");
    assert_schema_valid(&to_value(&out), "PreCompact");
}

/// Aggregate sanity test: all 11 handlers return an empty-ish, schema-valid
/// output when no cas_root is available. Written as a single loop so the
/// per-handler tests above can be removed in the future without losing the
/// comprehensive check.
#[test]
fn all_handlers_emit_schema_valid_output_for_empty_cas_root() {
    macro_rules! check {
        ($event:expr, $fn:ident, $input:expr) => {{
            let out = $fn(&$input, None).expect(concat!($event, " handler returned Err"));
            assert_schema_valid(&to_value(&out), $event);
        }};
    }

    check!("SessionStart", handle_session_start, base_input("SessionStart"));
    check!("SessionEnd", handle_session_end, base_input("SessionEnd"));
    check!("Stop", handle_stop, base_input("Stop"));
    check!(
        "SubagentStart",
        handle_subagent_start,
        base_input("SubagentStart")
    );
    check!(
        "SubagentStop",
        handle_subagent_stop,
        base_input("SubagentStop")
    );
    check!("PostToolUse", handle_post_tool_use, {
        let mut i = base_input("PostToolUse");
        i.tool_name = Some("Write".into());
        i.tool_input = Some(serde_json::json!({}));
        i.tool_response = Some(serde_json::json!({}));
        i
    });
    check!("PreToolUse", handle_pre_tool_use, {
        let mut i = base_input("PreToolUse");
        i.tool_name = Some("Bash".into());
        i.tool_input = Some(serde_json::json!({}));
        i
    });
    check!(
        "UserPromptSubmit",
        handle_user_prompt_submit,
        {
            let mut i = base_input("UserPromptSubmit");
            i.user_prompt = Some("hi".into());
            i
        }
    );
    check!(
        "PermissionRequest",
        handle_permission_request,
        {
            let mut i = base_input("PermissionRequest");
            i.tool_name = Some("Bash".into());
            i
        }
    );
    check!(
        "Notification",
        handle_notification,
        base_input("Notification")
    );
    check!("PreCompact", handle_pre_compact, base_input("PreCompact"));
}
