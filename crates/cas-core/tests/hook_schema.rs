//! Per-event golden-JSON schema tests for hook output.
//!
//! Context: Claude Code's runtime validator rejects the entire hook output
//! (silently) if it violates the schema. Two bugs shipped in baa540b emitted
//! `hookSpecificOutput.additionalContext` from Stop-family hooks, causing the
//! hooks to become no-ops. This test file establishes a behavior baseline for
//! every HookEvent variant so regressions are caught at `cargo test` time, not
//! in production.
//!
//! Schema summary (from the cloud-side rejection log):
//! - Top-level allowed keys: `continue`, `suppressOutput`, `stopReason`,
//!   `decision` ("approve"|"block"), `reason`, `systemMessage`,
//!   `permissionDecision` ("allow"|"deny"|"ask"), `hookSpecificOutput`.
//! - `hookSpecificOutput` is ONLY permitted for: PreToolUse, PostToolUse,
//!   UserPromptSubmit, SessionStart, SubagentStart, Notification,
//!   PermissionRequest.
//! - `hookSpecificOutput` MUST NOT appear for: Stop, SubagentStop, PreCompact,
//!   SessionEnd — these route any context through `systemMessage` instead.
//! - Per-variant shape rules for `hookSpecificOutput`:
//!   - PreToolUse:        { hookEventName, permissionDecision?, permissionDecisionReason?, updatedInput? }
//!   - UserPromptSubmit:  { hookEventName, additionalContext } — additionalContext REQUIRED
//!   - PostToolUse:       { hookEventName, additionalContext? }
//!
//! Each test below asserts BOTH the presence of required fields AND the
//! absence of forbidden fields so that either direction of regression fails
//! the test.

use cas_core::hooks::types::{HookOutput, PreToolUseHarness};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Schema assertion helpers
// ---------------------------------------------------------------------------

/// Set of top-level keys allowed anywhere in a HookOutput. Anything outside
/// this set is a schema violation.
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

/// Events for which `hookSpecificOutput` MUST NOT appear in emitted JSON.
/// Emitting it on these events is what caused the baa540b silent-drop bug.
const EVENTS_FORBIDDING_HOOK_SPECIFIC: &[&str] =
    &["Stop", "SubagentStop", "PreCompact", "SessionEnd"];

/// Serialize + parse as a generic `Value` for structural assertions.
fn to_value(output: &HookOutput) -> Value {
    let json = serde_json::to_string(output).expect("HookOutput serializes cleanly");
    serde_json::from_str(&json).expect("emitted JSON round-trips")
}

/// Assert no unknown top-level keys.
fn assert_only_allowed_top_level(value: &Value) {
    let obj = value.as_object().expect("output is a JSON object");
    for key in obj.keys() {
        assert!(
            ALLOWED_TOP_LEVEL.contains(&key.as_str()),
            "unknown top-level key in hook output: {key:?} (value = {value})"
        );
    }
}

/// Assert `hookSpecificOutput` is absent from the output — the Stop-family
/// rule. This is the primary regression guard for baa540b.
fn assert_no_hook_specific_output(value: &Value, ctx: &str) {
    // Keyed check is the authoritative assertion — raw substring scans would
    // false-positive whenever a user-supplied `systemMessage` legitimately
    // contains the string "additionalContext" (e.g., a rule-review snippet
    // quoting the schema). Walk the top-level object + any hookSpecificOutput
    // sub-object explicitly.
    assert!(
        value.get("hookSpecificOutput").is_none(),
        "{ctx}: hookSpecificOutput must be absent, got {value}"
    );
    if let Some(obj) = value.as_object() {
        for key in obj.keys() {
            assert!(
                !key.eq_ignore_ascii_case("hookSpecificOutput"),
                "{ctx}: variant-cased hookSpecificOutput key {key:?} must be absent, got {value}"
            );
        }
    }
}

/// Extract `hookSpecificOutput` as an object, panicking if absent.
fn hook_specific<'a>(value: &'a Value, ctx: &str) -> &'a serde_json::Map<String, Value> {
    value
        .get("hookSpecificOutput")
        .unwrap_or_else(|| panic!("{ctx}: expected hookSpecificOutput, got {value}"))
        .as_object()
        .unwrap_or_else(|| panic!("{ctx}: hookSpecificOutput must be an object, got {value}"))
}

/// Enforce that only `allowed` keys appear inside `hookSpecificOutput`.
fn assert_hook_specific_keys_subset(
    hso: &serde_json::Map<String, Value>,
    allowed: &[&str],
    ctx: &str,
) {
    for key in hso.keys() {
        assert!(
            allowed.contains(&key.as_str()),
            "{ctx}: unexpected key {key:?} in hookSpecificOutput (allowed: {allowed:?})"
        );
    }
}

// ---------------------------------------------------------------------------
// PreToolUse
// ---------------------------------------------------------------------------

#[test]
fn pretooluse_permission_decision_output_schema() {
    // Shape: { hookEventName, permissionDecision, permissionDecisionReason }
    let output = HookOutput::with_pre_tool_permission("allow", "auto-approved");
    let value = to_value(&output);

    assert_only_allowed_top_level(&value);
    let hso = hook_specific(&value, "PreToolUse.permission");
    assert_eq!(
        hso.get("hookEventName").and_then(Value::as_str),
        Some("PreToolUse"),
        "hookEventName must round-trip"
    );
    assert_eq!(
        hso.get("permissionDecision").and_then(Value::as_str),
        Some("allow")
    );
    assert_eq!(
        hso.get("permissionDecisionReason").and_then(Value::as_str),
        Some("auto-approved")
    );
    // Default permission builder omits additionalContext (optional field).
    assert!(
        hso.get("additionalContext").is_none(),
        "default with_pre_tool_permission must not emit additionalContext"
    );
    assert_hook_specific_keys_subset(
        hso,
        &[
            "hookEventName",
            "permissionDecision",
            "permissionDecisionReason",
            "updatedInput",
            "additionalContext",
        ],
        "PreToolUse.permission",
    );
}

#[test]
fn pretooluse_permission_with_context_emits_additional_context() {
    // cas-73c8: successful SendMessage auto-route uses allow + additionalContext
    // so Claude sees a success receipt instead of a deny/`<error>` envelope.
    let output = HookOutput::with_pre_tool_permission_and_context(
        "allow",
        "auto-routed",
        "✅ AUTO-ROUTED via CAS coordination",
    );
    let value = to_value(&output);
    let hso = hook_specific(&value, "PreToolUse.permission+context");
    assert_eq!(
        hso.get("permissionDecision").and_then(Value::as_str),
        Some("allow")
    );
    assert_eq!(
        hso.get("additionalContext").and_then(Value::as_str),
        Some("✅ AUTO-ROUTED via CAS coordination")
    );
}

#[test]
fn pretooluse_claude_code_2_1_227_captured_decision_contract() {
    // Captured with real Claude Code 2.1.227 PreToolUse invocations on
    // 2026-08-11. `allow` executed the tool, `deny` blocked it, and `ask`
    // reached the harness permission path. The accepted shape is nested and
    // event-tagged; the legacy top-level `decision: "approve"` is inert and
    // must not become CAS's auto-approval fallback.
    //
    // Durable raw transcripts: artifacts/cas-a53c/live-contract/.
    for decision in ["allow", "deny", "ask"] {
        let output = HookOutput::with_pre_tool_permission_for_harness(
            PreToolUseHarness::ClaudeCode,
            decision,
            "captured-contract",
            None,
        );
        let value = to_value(&output);
        assert_eq!(
            value,
            serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": decision,
                    "permissionDecisionReason": "captured-contract",
                }
            }),
            "Claude Code 2.1.227 captured PreToolUse contract regressed for {decision}"
        );
        assert!(
            value.get("decision").is_none(),
            "legacy top-level decision must remain absent for {decision}: {value}"
        );
    }

    // The SendMessage receipt path formerly constructed this separately. Its
    // additionalContext is accepted by the same 2.1.227 live contract, while
    // the decision envelope must remain byte-for-byte identical to the normal
    // allow path above.
    assert_eq!(
        to_value(&HookOutput::with_pre_tool_permission_for_harness(
            PreToolUseHarness::ClaudeCode,
            "allow",
            "captured-contract",
            Some("CAS receipt"),
        )),
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason": "captured-contract",
                "additionalContext": "CAS receipt",
            }
        }),
        "Claude Code 2.1.227 captured context-bearing allow contract regressed"
    );
}

#[test]
fn pretooluse_codex_0_147_captured_decision_contract() {
    // Captured from the Codex 0.147 runner on 2026-08-11. It rejects the
    // Claude-only `permissionDecision:"allow"` response, but accepts the
    // nested deny response used by workspace and formatter safety gates. An
    // empty object is its accepted allow/no-op response.
    assert_eq!(
        to_value(&HookOutput::with_pre_tool_permission_for_harness(
            PreToolUseHarness::Codex,
            "allow",
            "captured-contract",
            None,
        )),
        serde_json::json!({}),
        "Codex allow must be a no-op rather than Claude's rejected allow vocabulary"
    );
    assert_eq!(
        to_value(&HookOutput::with_pre_tool_permission_for_harness(
            PreToolUseHarness::Codex,
            "deny",
            "captured-contract",
            None,
        )),
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "captured-contract",
            }
        }),
        "Codex deny must preserve the accepted protection-gate response"
    );
}

#[test]
fn pretooluse_updated_input_output_schema() {
    // PreToolUse with_updated_input must carry updatedInput and nothing else
    // beyond hookEventName.
    let output = HookOutput::with_pre_tool_updated_input(serde_json::json!({ "command": "ls -la" }));
    let value = to_value(&output);
    assert_only_allowed_top_level(&value);
    let hso = hook_specific(&value, "PreToolUse.updatedInput");
    assert_eq!(
        hso.get("hookEventName").and_then(Value::as_str),
        Some("PreToolUse")
    );
    assert!(hso.get("updatedInput").is_some());
    assert!(hso.get("permissionDecision").is_none());
    assert!(hso.get("additionalContext").is_none());
}

// ---------------------------------------------------------------------------
// UserPromptSubmit
// ---------------------------------------------------------------------------

#[test]
fn userpromptsubmit_requires_additional_context() {
    // additionalContext is REQUIRED (per Claude Code schema) when emitting
    // hookSpecificOutput for UserPromptSubmit. with_context is the correct
    // builder; it must produce a non-null string.
    let output = HookOutput::with_user_prompt_context("recall: user prefers X".into());
    let value = to_value(&output);

    assert_only_allowed_top_level(&value);
    let hso = hook_specific(&value, "UserPromptSubmit");
    assert_eq!(
        hso.get("hookEventName").and_then(Value::as_str),
        Some("UserPromptSubmit")
    );
    let ctx = hso
        .get("additionalContext")
        .expect("UserPromptSubmit hookSpecificOutput must carry additionalContext");
    assert!(
        ctx.is_string(),
        "additionalContext must be a string, got {ctx}"
    );
    assert!(
        !ctx.as_str().unwrap().is_empty(),
        "additionalContext must not be empty for UserPromptSubmit"
    );
    // permissionDecision / updatedInput have no place here.
    assert!(hso.get("permissionDecision").is_none());
    assert!(hso.get("updatedInput").is_none());
}

// ---------------------------------------------------------------------------
// PostToolUse
// ---------------------------------------------------------------------------

#[test]
fn posttooluse_additional_context_optional_both_shapes_valid() {
    // Case A: empty PostToolUse output — no hookSpecificOutput at all.
    let empty = HookOutput::empty();
    let value = to_value(&empty);
    assert_only_allowed_top_level(&value);
    // empty() never emits hookSpecificOutput — verify.
    assert!(value.get("hookSpecificOutput").is_none());

    // Case B: PostToolUse with additionalContext — must be a valid shape.
    let with_ctx = HookOutput::with_post_tool_context("tool observation".into());
    let value = to_value(&with_ctx);
    assert_only_allowed_top_level(&value);
    let hso = hook_specific(&value, "PostToolUse.withContext");
    assert_eq!(
        hso.get("hookEventName").and_then(Value::as_str),
        Some("PostToolUse")
    );
    assert_eq!(
        hso.get("additionalContext").and_then(Value::as_str),
        Some("tool observation")
    );
    // Per schema: PostToolUse hookSpecificOutput only carries
    // { hookEventName, additionalContext? }. Permission fields are PreToolUse-
    // only — if a future builder leaks them here, the test must fail.
    assert_hook_specific_keys_subset(
        hso,
        &["hookEventName", "additionalContext"],
        "PostToolUse.withContext",
    );
}

// ---------------------------------------------------------------------------
// Stop family — MUST NOT emit hookSpecificOutput
// ---------------------------------------------------------------------------

#[test]
fn stop_output_never_has_hook_specific_output() {
    // Iterate every Stop-producing constructor. These are the builders used
    // by `handle_stop` and its helpers. If a future change wires `with_context`
    // (or adds a new builder that sets hook_specific_output) into the Stop
    // handler, one of these assertions fails.
    let constructors: Vec<(&str, HookOutput)> = vec![
        ("empty", HookOutput::empty()),
        (
            "with_system_context",
            HookOutput::with_system_context("codemap stale".into()),
        ),
        (
            "block_stop",
            HookOutput::block_stop("keep working".into()),
        ),
        (
            "block_stop_with_context",
            HookOutput::block_stop_with_context(
                "keep working".into(),
                "pending review context".into(),
            ),
        ),
        (
            "blocking_error",
            HookOutput::blocking_error("error msg".into()),
        ),
    ];

    for (label, out) in constructors {
        let value = to_value(&out);
        let ctx = format!("Stop::{label}");
        assert_only_allowed_top_level(&value);
        assert_no_hook_specific_output(&value, &ctx);
    }
}

#[test]
fn stop_block_stop_with_context_routes_via_system_message() {
    // Regression for baa540b: block_stop_with_context used to emit
    // hookSpecificOutput.additionalContext — the entire output was rejected.
    // The correct path is `systemMessage` + `decision: block` + `reason`.
    let out = HookOutput::block_stop_with_context(
        "continue working on reviews".into(),
        "<system-reminder>2 learnings pending</system-reminder>".into(),
    );
    let value = to_value(&out);

    assert_no_hook_specific_output(&value, "Stop::block_stop_with_context");
    assert_eq!(
        value.get("decision").and_then(Value::as_str),
        Some("block"),
        "block_stop_with_context must set decision=block, got {value}"
    );
    assert_eq!(
        value.get("reason").and_then(Value::as_str),
        Some("continue working on reviews")
    );
    assert!(
        value
            .get("systemMessage")
            .and_then(Value::as_str)
            .is_some_and(|s| s.contains("pending")),
        "context must be routed via systemMessage, got {value}"
    );
}

#[test]
fn subagentstop_output_never_has_hook_specific_output() {
    // SubagentStop shares the Stop-family constraint. handle_subagent_stop
    // currently uses empty() and with_system_context() shapes. Cover both.
    for (label, out) in [
        ("empty", HookOutput::empty()),
        (
            "with_system_context",
            HookOutput::with_system_context("subagent wrap-up".into()),
        ),
    ] {
        let value = to_value(&out);
        assert_only_allowed_top_level(&value);
        assert_no_hook_specific_output(&value, &format!("SubagentStop::{label}"));
    }
}

#[test]
fn precompact_output_never_has_hook_specific_output() {
    // PreCompact is Stop-family for schema purposes.
    for (label, out) in [
        ("empty", HookOutput::empty()),
        (
            "with_system_context",
            HookOutput::with_system_context("compaction notice".into()),
        ),
    ] {
        let value = to_value(&out);
        assert_only_allowed_top_level(&value);
        assert_no_hook_specific_output(&value, &format!("PreCompact::{label}"));
    }
}

#[test]
fn sessionend_output_never_has_hook_specific_output() {
    // SessionEnd is Stop-family for schema purposes.
    for (label, out) in [
        ("empty", HookOutput::empty()),
        (
            "with_system_context",
            HookOutput::with_system_context("session end summary".into()),
        ),
    ] {
        let value = to_value(&out);
        assert_only_allowed_top_level(&value);
        assert_no_hook_specific_output(&value, &format!("SessionEnd::{label}"));
    }
}

/// Meta-assertion: the Stop-family list stays in sync with any new `with_*`
/// builder we add — if someone adds a Stop-family constructor that emits
/// hookSpecificOutput by accident, the list-driven loop below catches it.
#[test]
fn stop_family_constructors_never_emit_hook_specific_output() {
    let stop_family_outputs: Vec<(&str, HookOutput)> = vec![
        ("empty", HookOutput::empty()),
        (
            "with_system_context",
            HookOutput::with_system_context("x".into()),
        ),
        ("block_stop", HookOutput::block_stop("x".into())),
        (
            "block_stop_with_context",
            HookOutput::block_stop_with_context("x".into(), "y".into()),
        ),
        ("blocking_error", HookOutput::blocking_error("x".into())),
    ];

    for event in EVENTS_FORBIDDING_HOOK_SPECIFIC {
        for (label, out) in &stop_family_outputs {
            let value = to_value(out);
            assert_no_hook_specific_output(&value, &format!("{event}::{label}"));
        }
    }
}

// ---------------------------------------------------------------------------
// SessionStart — may carry additionalContext via hookSpecificOutput
// ---------------------------------------------------------------------------

#[test]
fn sessionstart_output_schema() {
    // SessionStart is one of the events that DOES accept hookSpecificOutput.
    // Shape: { hookEventName, additionalContext }.
    let out = HookOutput::with_session_start_context("CAS active: 3 open tasks".into());
    let value = to_value(&out);
    assert_only_allowed_top_level(&value);

    let hso = hook_specific(&value, "SessionStart");
    assert_eq!(
        hso.get("hookEventName").and_then(Value::as_str),
        Some("SessionStart")
    );
    assert!(
        hso.get("additionalContext")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty()),
        "SessionStart additionalContext must be a non-empty string"
    );
    assert!(hso.get("permissionDecision").is_none());
    assert!(hso.get("updatedInput").is_none());
}

#[test]
fn sessionstart_empty_output_also_valid() {
    // handlers_session::handle_session_start falls through to empty() when
    // there's nothing to inject. Verify that path emits `{}` — no stray
    // fields, no hookSpecificOutput.
    let out = HookOutput::empty();
    let value = to_value(&out);
    let obj = value.as_object().expect("object");
    assert!(
        obj.is_empty(),
        "empty() must serialize to `{{}}`, got {value}"
    );
}

// ---------------------------------------------------------------------------
// SubagentStart
// ---------------------------------------------------------------------------

#[test]
fn subagentstart_output_schema() {
    // handle_subagent_start currently returns empty(); cover the context path
    // too in case future logic attaches context. The key invariant is the
    // event name matches.
    // SubagentStart has NO HookSpecificOutput variant — the typed enum makes
    // it unrepresentable, mirroring the Stop-family rule. Any context for a
    // SubagentStart hook routes through systemMessage instead.
    let out = HookOutput::empty();
    let value = to_value(&out);
    assert_only_allowed_top_level(&value);
    assert!(value.as_object().unwrap().is_empty());

    let with_msg = HookOutput::with_system_context("subagent role: reviewer".into());
    let value = to_value(&with_msg);
    assert_only_allowed_top_level(&value);
    assert!(value.get("hookSpecificOutput").is_none());
    assert_eq!(
        value.get("systemMessage").and_then(Value::as_str),
        Some("subagent role: reviewer")
    );
}

// ---------------------------------------------------------------------------
// Notification
// ---------------------------------------------------------------------------

#[test]
fn notification_output_schema() {
    // Notification handlers currently return empty(). Cover the common valid
    // shapes so a future change can't introduce forbidden fields unnoticed.
    let out = HookOutput::empty();
    let value = to_value(&out);
    assert_only_allowed_top_level(&value);
    assert!(value.as_object().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// PermissionRequest
// ---------------------------------------------------------------------------

#[test]
fn permissionrequest_output_schema() {
    // PermissionRequest shares the PreToolUse shape for permission decisions.
    let out = HookOutput::with_permission_request("deny", "blocked by policy");
    let value = to_value(&out);
    assert_only_allowed_top_level(&value);
    let hso = hook_specific(&value, "PermissionRequest");
    assert_eq!(
        hso.get("hookEventName").and_then(Value::as_str),
        Some("PermissionRequest")
    );
    assert_eq!(
        hso.get("permissionDecision").and_then(Value::as_str),
        Some("deny")
    );
    assert_eq!(
        hso.get("permissionDecisionReason").and_then(Value::as_str),
        Some("blocked by policy")
    );
}

// ---------------------------------------------------------------------------
// Universal invariants
// ---------------------------------------------------------------------------

#[test]
fn decision_field_accepts_only_known_values() {
    // Schema allows decision ∈ {approve, block}. block_stop uses "block".
    // Approve isn't currently emitted by any builder, but the universe of
    // string values we do emit must stay inside the enum.
    let emitted = [HookOutput::block_stop("x".into())];
    for out in &emitted {
        let value = to_value(out);
        if let Some(d) = value.get("decision").and_then(Value::as_str) {
            assert!(
                ["approve", "block"].contains(&d),
                "decision must be approve|block, got {d:?}"
            );
        }
    }
}

#[test]
fn empty_output_serializes_to_empty_object() {
    // Baseline: a default HookOutput must never contain serialization noise
    // like `"continue": null` or `"hookSpecificOutput": null`. Those would
    // also be rejected by the validator.
    let out = HookOutput::empty();
    let json = serde_json::to_string(&out).unwrap();
    assert_eq!(json, "{}");
}
