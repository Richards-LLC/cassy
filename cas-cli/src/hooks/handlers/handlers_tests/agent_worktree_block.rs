//! Tests for supervisor Agent(isolation="worktree") block (cas-483b / EPIC cas-7c88).
//!
//! The gate fires only when ALL of:
//!   - `input.agent_role == Some("supervisor")` (or, as a last resort, the
//!     process env var `CAS_AGENT_ROLE=supervisor`), AND
//!   - `tool_name == "Agent"`, AND
//!   - `tool_input.isolation == "worktree"`, AND
//!   - `tool_input.subagent_type != "task-verifier"` (verification-jail exemption).
//!
//! Any other combination must pass through untouched so that workers, solo users,
//! non-worktree Agent calls (Explore, code review personas, task-verifier),
//! and task-verifier-with-isolation continue to work.
//!
//! cas-18fe: tests drive the role through `HookInput.agent_role` so the prod
//! path is exercised without racing on the process-global `CAS_AGENT_ROLE` —
//! no env mutation, no cross-test locking needed. One dedicated fallback test
//! still covers the env-var legacy path.

use crate::hooks::handlers::handle_pre_tool_use;
use cas_core::hooks::types::HookInput;

fn agent_input_with_role(role: Option<&str>, isolation: Option<&str>) -> HookInput {
    let tool_input = match isolation {
        Some(iso) => serde_json::json!({
            "subagent_type": "general-purpose",
            "prompt": "do work",
            "isolation": iso,
        }),
        None => serde_json::json!({
            "subagent_type": "Explore",
            "prompt": "explore",
        }),
    };
    HookInput {
        session_id: "test-session".into(),
        cwd: "/test".into(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some("Agent".into()),
        tool_input: Some(tool_input),
        agent_role: role.map(str::to_string),
        ..HookInput::default()
    }
}

/// Extract the pre-tool deny reason from a HookOutput, or None if not a deny.
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
        .map(|s| s.to_string())
}

// ============================================================================
// Positive: the one case that must be blocked.
// ============================================================================

#[test]
fn supervisor_agent_worktree_is_denied() {
    // EPIC cas-8888 (cas-fd9f): the deny message's tool-prefix now comes from
    // `harness_policy::own_tool_prefix()`, which is env-based (matching
    // every other harness-alias helper in this codebase). In real usage
    // `HookInput.agent_role` is always just a snapshot of `CAS_AGENT_ROLE`
    // taken at dispatch time (see `cli/hook.rs`), so the two signals are
    // never actually independent — but this test drives role purely via
    // the HookInput field (by original design, to avoid env locking), so it
    // must now also pin the env explicitly to keep both signals consistent
    // the way production always does, rather than leaving the prefix to
    // whatever CAS_AGENT_ROLE/CAS_FACTORY_SUPERVISOR_CLI the ambient process
    // happens to have.
    let _g = env_lock();
    let _role = set_role_env(Some("supervisor"));
    let _cli = set_supervisor_cli_env(Some("claude"));

    let input = agent_input_with_role(Some("supervisor"), Some("worktree"));
    let out = handle_pre_tool_use(&input, None).expect("handler ok");
    let reason = deny_reason(&out).expect("expected deny");
    // Message must point at the factory escape hatch (cas-8563b: spawn_workers
    // moved from coordination to factory) and explain the leak.
    assert!(
        reason.contains("mcp__cas__factory action=spawn_workers"),
        "deny message should direct to factory: {reason}"
    );
    assert!(
        reason.contains("spawn_workers"),
        "deny message should name spawn_workers action: {reason}"
    );
    assert!(
        reason.contains("leak") || reason.contains("cleaned up"),
        "deny message should explain the leak reason: {reason}"
    );
}

/// EPIC cas-8888 (cas-fd9f): the load-bearing regression test — before the
/// fix, this deny message was hardcoded to Claude's `mcp__cas__` prefix
/// unconditionally.
#[test]
fn grok_supervisor_agent_worktree_is_denied_with_cas_prefix() {
    let _g = env_lock();
    let _role = set_role_env(Some("supervisor"));
    let _cli = set_supervisor_cli_env(Some("grok"));

    let input = agent_input_with_role(Some("supervisor"), Some("worktree"));
    let out = handle_pre_tool_use(&input, None).expect("handler ok");
    let reason = deny_reason(&out).expect("expected deny");
    assert!(
        reason.contains("cas__factory"),
        "grok supervisor deny message must use its own cas__ prefix: {reason}"
    );
    assert!(
        !reason.contains("mcp__cas__factory") && !reason.contains("mcp__cs__factory"),
        "grok supervisor deny message must NOT carry another harness's prefix: {reason}"
    );
}

// ============================================================================
// Negatives: must NOT fire.
// ============================================================================

#[test]
fn worker_agent_worktree_is_allowed() {
    let input = agent_input_with_role(Some("worker"), Some("worktree"));
    let out = handle_pre_tool_use(&input, None).expect("handler ok");
    assert!(
        deny_reason(&out).is_none(),
        "worker should not hit the supervisor gate"
    );
}

#[test]
fn supervisor_agent_without_isolation_is_allowed() {
    let input = agent_input_with_role(Some("supervisor"), None);
    let out = handle_pre_tool_use(&input, None).expect("handler ok");
    assert!(
        deny_reason(&out).is_none(),
        "supervisor Agent() without isolation must still work (Explore, reviews, verifier)"
    );
}

#[test]
fn supervisor_agent_isolation_other_value_is_allowed() {
    // Only the literal string "worktree" triggers the block. Other values
    // (future isolation modes, bogus strings) pass through.
    let input = agent_input_with_role(Some("supervisor"), Some("sandbox"));
    let out = handle_pre_tool_use(&input, None).expect("handler ok");
    assert!(
        deny_reason(&out).is_none(),
        "non-worktree isolation values are out of scope for this gate"
    );
}

#[test]
fn solo_user_agent_worktree_is_allowed() {
    // No role at all — plain `claude` session outside factory. This test
    // exercises the fallback path (agent_role=None → read env), so it must
    // serialize against the only other env-touching test in this module.
    let _g = env_lock();
    let _role = set_role_env(None);
    let input = agent_input_with_role(None, Some("worktree"));
    let out = handle_pre_tool_use(&input, None).expect("handler ok");
    assert!(
        deny_reason(&out).is_none(),
        "solo users have no supervisor discipline to enforce"
    );
}

#[test]
fn supervisor_task_verifier_with_worktree_is_allowed() {
    // Task-verifier is the only supervisor-initiated Agent subagent that may
    // need isolation without being caught by the leak gate. Blocking it would
    // strand the supervisor in pending_verification.
    let input = HookInput {
        session_id: "test-session".into(),
        cwd: "/test".into(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some("Agent".into()),
        tool_input: Some(serde_json::json!({
            "subagent_type": "task-verifier",
            "prompt": "Verify task cas-xyz",
            "isolation": "worktree",
        })),
        agent_role: Some("supervisor".into()),
        ..HookInput::default()
    };
    let out = handle_pre_tool_use(&input, None).expect("handler ok");
    assert!(
        deny_reason(&out).is_none(),
        "task-verifier must be exempt from the worktree gate so verification-jail unjail paths work"
    );
}

#[test]
fn supervisor_agent_isolation_non_string_is_allowed() {
    // A JSON number, bool, or object in `isolation` must not collapse into
    // `Some("worktree")`. Documents that the match is string-literal only.
    let input = HookInput {
        session_id: "test".into(),
        cwd: "/test".into(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some("Agent".into()),
        tool_input: Some(serde_json::json!({
            "subagent_type": "general-purpose",
            "prompt": "do work",
            "isolation": 42,
        })),
        agent_role: Some("supervisor".into()),
        ..HookInput::default()
    };
    let out = handle_pre_tool_use(&input, None).expect("handler ok");
    assert!(
        deny_reason(&out).is_none(),
        "non-string isolation values fall through via and_then(as_str)"
    );
}

#[test]
fn supervisor_agent_missing_tool_input_is_allowed() {
    // `tool_input: None` must not panic or deny — the `and_then` chain
    // gracefully yields `None` for isolation.
    let input = HookInput {
        session_id: "test".into(),
        cwd: "/test".into(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some("Agent".into()),
        tool_input: None,
        agent_role: Some("supervisor".into()),
        ..HookInput::default()
    };
    let out = handle_pre_tool_use(&input, None).expect("handler ok");
    assert!(
        deny_reason(&out).is_none(),
        "missing tool_input must pass through without denying"
    );
}

#[test]
fn supervisor_non_agent_tool_is_allowed() {
    // Even with isolation="worktree" in tool_input, non-Agent tools must pass.
    let mut input = agent_input_with_role(Some("supervisor"), Some("worktree"));
    input.tool_name = Some("Bash".into());
    let out = handle_pre_tool_use(&input, None).expect("handler ok");
    assert!(
        deny_reason(&out).is_none(),
        "gate is Agent-scoped; Bash etc. must not be affected"
    );
}

#[test]
fn supervisor_role_case_insensitive() {
    // harness_policy::is_supervisor uses eq_ignore_ascii_case.
    for role in ["SUPERVISOR", "Supervisor", "sUpErViSoR"] {
        let input = agent_input_with_role(Some(role), Some("worktree"));
        let out = handle_pre_tool_use(&input, None).expect("handler ok");
        assert!(
            deny_reason(&out).is_some(),
            "role '{role}' should be recognized case-insensitively and trigger deny"
        );
    }
}

// ============================================================================
// Env-var fallback: when HookInput.agent_role is absent, the helpers fall
// back to reading CAS_AGENT_ROLE. Verified with a single dedicated test that
// guards the legacy path.
// ============================================================================

/// Use the shared process-wide mutex from mod.rs so that concurrent tests
/// across sibling modules don't race on CAS_AGENT_ROLE.
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    super::env_lock()
}

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

/// EPIC cas-8888 (cas-fd9f): pins `CAS_FACTORY_SUPERVISOR_CLI` for tests that
/// assert a specific tool-prefix in the deny/reminder text (see
/// `harness_policy::own_tool_prefix`).
struct SupervisorCliGuard(Option<String>);

impl Drop for SupervisorCliGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.0 {
                Some(v) => std::env::set_var("CAS_FACTORY_SUPERVISOR_CLI", v),
                None => std::env::remove_var("CAS_FACTORY_SUPERVISOR_CLI"),
            }
        }
    }
}

fn set_supervisor_cli_env(cli: Option<&str>) -> SupervisorCliGuard {
    let prev = std::env::var("CAS_FACTORY_SUPERVISOR_CLI").ok();
    unsafe {
        match cli {
            Some(v) => std::env::set_var("CAS_FACTORY_SUPERVISOR_CLI", v),
            None => std::env::remove_var("CAS_FACTORY_SUPERVISOR_CLI"),
        }
    }
    SupervisorCliGuard(prev)
}

#[test]
fn env_fallback_triggers_when_hook_input_role_absent() {
    // agent_role: None — handlers should fall through to CAS_AGENT_ROLE.
    let _g = env_lock();
    let _role = set_role_env(Some("supervisor"));

    let input = HookInput {
        session_id: "test".into(),
        cwd: "/test".into(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some("Agent".into()),
        tool_input: Some(serde_json::json!({
            "subagent_type": "general-purpose",
            "prompt": "fallback",
            "isolation": "worktree",
        })),
        agent_role: None, // force env-fallback path
        ..HookInput::default()
    };
    let out = handle_pre_tool_use(&input, None).expect("handler ok");
    assert!(
        deny_reason(&out).is_some(),
        "env-var fallback should still trigger the gate when HookInput.agent_role is None"
    );
}
