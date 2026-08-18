//! Tests for the cas-7f33 factory auto-approve belt (#3) in
//! `handle_permission_request`.
//!
//! The PreToolUse auto-approve covers the primary path. This belt covers
//! Claude Code 2.1.x builds where PreToolUse `permissionDecision:"allow"`
//! doesn't pre-empt team-mode escalation cleanly and the decision flow
//! falls through to a PermissionRequest. Without the belt, the UG9 bug
//! still self-deadlocks the supervisor on Write/Edit/Bash.
//!
//! These tests lock in:
//!   - Positive: factory supervisor/worker + allowlisted tool → allow,
//!     reason mentions "Factory agent auto-approve".
//!   - Negative: CAS_AGENT_ROLE unset → no bypass, falls through to
//!     Claude Code's normal flow.
//!   - Negative: factory agent + tool outside the allowlist → no bypass.
//!   - Fires on both `cas_root=None` (no Cassy initialized) and
//!     `cas_root=Some` — intentionally asymmetric with the PreToolUse
//!     hoist; see the block comment in notifications.rs for rationale.
//!
//! Like `factory_auto_approve.rs`, these tests mutate process env
//! (`CAS_AGENT_ROLE`) and therefore serialize on a local mutex.

use crate::hooks::handlers::handle_permission_request;
use cas_core::hooks::types::HookInput;

fn input_for(tool: &str, file_path: Option<&str>) -> HookInput {
    let tool_input = match file_path {
        Some(p) => serde_json::json!({"file_path": p, "content": "x"}),
        None => serde_json::json!({"command": "echo hi"}),
    };
    HookInput {
        session_id: "test-session".into(),
        cwd: "/test".into(),
        hook_event_name: "permission_prompt".into(),
        tool_name: Some(tool.into()),
        tool_input: Some(tool_input),
        ..HookInput::default()
    }
}

fn allow_reason(out: &cas_core::hooks::types::HookOutput) -> Option<String> {
    let specific = out.hook_specific_output.as_ref()?;
    let value = serde_json::to_value(specific).ok()?;
    let decision = value.get("permissionDecision")?.as_str()?;
    if decision != "allow" {
        return None;
    }
    value
        .get("permissionDecisionReason")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

// ============================================================================
// Positives: factory belt fires on both cas_root states for allowlisted tools.
// ============================================================================

#[test]
fn supervisor_write_permission_request_is_auto_approved_without_cas_root() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("supervisor"));
    let input = input_for("Write", Some("/tmp/foo.txt"));
    let out = handle_permission_request(&input, None).expect("handler ok");
    let reason = allow_reason(&out).expect("expected allow");
    assert!(
        reason.contains("Factory agent auto-approve"),
        "allow reason should identify the factory bypass: {reason}"
    );
}

#[test]
fn worker_edit_permission_request_is_auto_approved_without_cas_root() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("worker"));
    let input = input_for("Edit", Some("/tmp/foo.txt"));
    let out = handle_permission_request(&input, None).expect("handler ok");
    assert!(
        allow_reason(&out).is_some(),
        "worker Edit must auto-approve at PermissionRequest (belt #3)"
    );
}

#[test]
fn supervisor_bash_permission_request_is_auto_approved_without_cas_root() {
    let _g = super::env_lock();
    let _role = set_role_env(Some("supervisor"));
    let input = input_for("Bash", None);
    let out = handle_permission_request(&input, None).expect("handler ok");
    assert!(
        allow_reason(&out).is_some(),
        "supervisor Bash must auto-approve at PermissionRequest (belt #3)"
    );
}

#[test]
fn supervisor_write_permission_request_is_auto_approved_with_cas_root() {
    // Asymmetric-by-design with the PreToolUse hoist: belt #3 fires even
    // when cas_root=Some, because PermissionRequest has no protection
    // gate invariant to preserve. See comment in notifications.rs.
    let _g = super::env_lock();
    let _role = set_role_env(Some("supervisor"));
    let input = input_for("Write", Some("/tmp/foo.txt"));
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = handle_permission_request(&input, Some(tmp.path())).expect("handler ok");
    let reason = allow_reason(&out).expect("expected allow");
    assert!(
        reason.contains("Factory agent auto-approve"),
        "allow reason should identify the factory bypass: {reason}"
    );
}

// ============================================================================
// Negatives: bypass must stay scoped to factory agents and allowlisted tools.
// ============================================================================

#[test]
fn solo_user_permission_request_is_not_auto_approved() {
    // CAS_AGENT_ROLE unset — the handler must fall through. With no
    // agent store present, the cas_root=None early return (or the
    // agent-lookup failure) yields an empty output.
    let _g = super::env_lock();
    let _role = set_role_env(None);
    let input = input_for("Write", Some("/tmp/foo.txt"));
    let out = handle_permission_request(&input, None).expect("handler ok");
    assert!(
        allow_reason(&out).is_none(),
        "standalone sessions must not get the factory PermissionRequest bypass"
    );
}

#[test]
fn factory_agent_unknown_tool_permission_request_is_not_auto_approved() {
    // Tools outside FACTORY_AUTO_APPROVE_TOOLS must not get the bypass —
    // regression guard against widening the belt.
    let _g = super::env_lock();
    let _role = set_role_env(Some("worker"));
    let input = input_for("WebFetch", None);
    let out = handle_permission_request(&input, None).expect("handler ok");
    assert!(
        allow_reason(&out).is_none(),
        "WebFetch is not in the factory auto-approve list"
    );
}

// ----------------------------------------------------------------------------
// Env helpers — use the shared process-wide mutex from mod.rs so that
// concurrent tests across sibling modules don't race on CAS_AGENT_ROLE.
// ----------------------------------------------------------------------------

struct RoleGuard(Option<String>);

impl Drop for RoleGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.0 {
                Some(v) => std::env::set_var("CAS_AGENT_ROLE", v),
                None => std::env::remove_var("CAS_AGENT_ROLE"),
            }
        }
    }
}

fn set_role_env(role: Option<&str>) -> RoleGuard {
    let prev = std::env::var("CAS_AGENT_ROLE").ok();
    unsafe {
        match role {
            Some(v) => std::env::set_var("CAS_AGENT_ROLE", v),
            None => std::env::remove_var("CAS_AGENT_ROLE"),
        }
    }
    RoleGuard(prev)
}
