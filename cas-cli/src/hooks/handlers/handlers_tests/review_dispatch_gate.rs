//! cas-bcfb / GH #125: the `cas-code-review` ownership gate must hold on
//! EVERY entry path into the review, not just the CAS MCP one.
//!
//! cas-4fef installed the gate on `cas_skill_use` (`mcp__cas__skill
//! action=use`). Agents do not go that way: the skill is on disk and is
//! invoked with Claude Code's native `Skill` tool, and the Phase C workflow
//! (`.claude/workflows/cas-code-review.js`) is callable straight from the
//! `Workflow` tool. Both bypass the MCP server, so a worker ran the full
//! persona fan-out under `owner = "supervisor"` on a binary that provably
//! contained the gate. PreToolUse is the seam that covers those paths; these
//! tests pin one refusal per path.

use crate::hooks::handlers::handle_pre_tool_use;
use crate::test_support::TestEnvGuard;
use cas_core::hooks::types::{HookInput, HookOutput};

fn hook_input(tool_name: &str, tool_input: serde_json::Value) -> HookInput {
    HookInput {
        session_id: "test-session".into(),
        cwd: "/test".into(),
        hook_event_name: "PreToolUse".into(),
        tool_name: Some(tool_name.into()),
        tool_input: Some(tool_input),
        ..HookInput::default()
    }
}

fn deny_reason(out: &HookOutput) -> Option<String> {
    let specific = out.hook_specific_output.as_ref()?;
    let value = serde_json::to_value(specific).ok()?;
    if value.get("permissionDecision")?.as_str()? != "deny" {
        return None;
    }
    value
        .get("permissionDecisionReason")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// The refusal must be the cas-4fef text verbatim — reason + legal next move.
fn assert_is_the_cas_4fef_refusal(reason: &str, path: &str) {
    assert_eq!(
        reason,
        crate::code_review_dispatch::supervisor_owned_review_refusal(),
        "{path} must be refused with the same message as every other entry path"
    );
    assert!(
        reason.contains("supervisor-owned"),
        "{path} refusal must name the reason: {reason}"
    );
    assert!(
        reason.contains("do NOT pass `code_review_findings`"),
        "{path} refusal must name the legal next move: {reason}"
    );
}

fn worker_env() -> TestEnvGuard {
    TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("worker")),
        ("CAS_FACTORY_MODE", Some("1")),
    ])
}

// ============================================================================
// One test per entry path a worker can actually take.
// ============================================================================

/// PATH 1 — Claude Code's native `Skill` tool (the GH #125 incident path).
#[test]
fn worker_skill_tool_dispatch_of_cas_code_review_is_refused_cas_bcfb() {
    let _env = worker_env();
    let tmp = tempfile::tempdir().expect("tempdir");

    let out = handle_pre_tool_use(
        &hook_input(
            "Skill",
            serde_json::json!({"skill": "cas-code-review", "args": "mode=interactive"}),
        ),
        Some(tmp.path()),
    )
    .expect("handler ok");

    let reason = deny_reason(&out).expect("Skill-tool dispatch must be refused");
    assert_is_the_cas_4fef_refusal(&reason, "Skill dispatch");
}

/// PATH 1b — the `/cas-code-review` slash spelling of the same tool.
#[test]
fn worker_slash_spelling_of_the_review_skill_is_refused_cas_bcfb() {
    let _env = worker_env();
    let tmp = tempfile::tempdir().expect("tempdir");

    let out = handle_pre_tool_use(
        &hook_input("Skill", serde_json::json!({"skill": "/cas-code-review"})),
        Some(tmp.path()),
    )
    .expect("handler ok");

    assert_is_the_cas_4fef_refusal(
        &deny_reason(&out).expect("slash spelling must be refused"),
        "slash-spelled Skill dispatch",
    );
}

/// PATH 2 — direct `Workflow` invocation by name.
#[test]
fn worker_workflow_invocation_by_name_is_refused_cas_bcfb() {
    let _env = worker_env();
    let tmp = tempfile::tempdir().expect("tempdir");

    let out = handle_pre_tool_use(
        &hook_input("Workflow", serde_json::json!({"name": "cas-code-review"})),
        Some(tmp.path()),
    )
    .expect("handler ok");

    assert_is_the_cas_4fef_refusal(
        &deny_reason(&out).expect("Workflow-by-name must be refused"),
        "Workflow by name",
    );
}

/// PATH 3 — direct `Workflow` invocation by script path (Phase C workflow).
#[test]
fn worker_workflow_invocation_by_script_path_is_refused_cas_bcfb() {
    let _env = worker_env();
    let tmp = tempfile::tempdir().expect("tempdir");

    let out = handle_pre_tool_use(
        &hook_input(
            "Workflow",
            serde_json::json!({"scriptPath": ".claude/workflows/cas-code-review.js"}),
        ),
        Some(tmp.path()),
    )
    .expect("handler ok");

    assert_is_the_cas_4fef_refusal(
        &deny_reason(&out).expect("Workflow-by-scriptPath must be refused"),
        "Workflow by scriptPath",
    );
}

/// PATH 4 — headless skill-to-skill: the pipeline pasted in as an inline script.
#[test]
fn worker_headless_inline_review_workflow_is_refused_cas_bcfb() {
    let _env = worker_env();
    let tmp = tempfile::tempdir().expect("tempdir");

    let out = handle_pre_tool_use(
        &hook_input(
            "Workflow",
            serde_json::json!({
                "script": "export const meta = { name: 'cas-code-review', description: 'personas' }\nphase('Review')"
            }),
        ),
        Some(tmp.path()),
    )
    .expect("handler ok");

    assert_is_the_cas_4fef_refusal(
        &deny_reason(&out).expect("inline review workflow must be refused"),
        "headless inline Workflow",
    );
}

/// The gate must still fire when hook dispatch cannot resolve a CAS root —
/// the cas-865b default is supervisor-owned, so "no config" means refuse.
#[test]
fn the_gate_still_fires_without_a_cas_root_cas_bcfb() {
    let _env = worker_env();

    let out = handle_pre_tool_use(
        &hook_input("Skill", serde_json::json!({"skill": "cas-code-review"})),
        None,
    )
    .expect("handler ok");

    assert_is_the_cas_4fef_refusal(
        &deny_reason(&out).expect("cas_root=None must still refuse"),
        "cas_root=None Skill dispatch",
    );
}

/// Role comes from the hook payload too, not only the process env.
#[test]
fn the_gate_reads_the_worker_role_from_the_hook_payload_cas_bcfb() {
    let _env = TestEnvGuard::with_optional_vars(&[("CAS_AGENT_ROLE", None)]);
    let tmp = tempfile::tempdir().expect("tempdir");

    let mut input = hook_input("Skill", serde_json::json!({"skill": "cas-code-review"}));
    input.agent_role = Some("worker".into());

    assert_is_the_cas_4fef_refusal(
        &deny_reason(&handle_pre_tool_use(&input, Some(tmp.path())).expect("handler ok"))
            .expect("payload-role worker must be refused"),
        "payload-role Skill dispatch",
    );
}

// ============================================================================
// Who must NOT be gated.
// ============================================================================

/// Supervisors own the pipeline — they must never be refused.
#[test]
fn supervisors_are_never_refused_on_any_entry_path_cas_bcfb() {
    let _env = TestEnvGuard::with_optional_vars(&[
        ("CAS_AGENT_ROLE", Some("supervisor")),
        ("CAS_FACTORY_SUPERVISOR_CLI", Some("claude")),
    ]);
    let tmp = tempfile::tempdir().expect("tempdir");

    for (tool, payload) in [
        ("Skill", serde_json::json!({"skill": "cas-code-review"})),
        ("Workflow", serde_json::json!({"name": "cas-code-review"})),
    ] {
        let out = handle_pre_tool_use(&hook_input(tool, payload), Some(tmp.path()))
            .expect("handler ok");
        assert!(
            deny_reason(&out).is_none(),
            "supervisor must not be refused on the {tool} path"
        );
    }
}

/// The documented escape hatch: `[code_review] owner = "worker"` restores the
/// legacy worker-run flow on the harness paths too.
#[test]
fn worker_owned_projects_keep_the_legacy_flow_cas_bcfb() {
    let _env = worker_env();
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        tmp.path().join("config.toml"),
        "[code_review]\nowner = \"worker\"\n",
    )
    .expect("write config");

    let out = handle_pre_tool_use(
        &hook_input("Skill", serde_json::json!({"skill": "cas-code-review"})),
        Some(tmp.path()),
    )
    .expect("handler ok");

    assert!(
        deny_reason(&out).is_none(),
        "`owner = \"worker\"` must still permit worker-run review"
    );
}

/// Unrelated skills and workflows must pass through untouched.
#[test]
fn unrelated_skill_and_workflow_calls_are_untouched_cas_bcfb() {
    let _env = worker_env();
    let tmp = tempfile::tempdir().expect("tempdir");

    for (tool, payload) in [
        ("Skill", serde_json::json!({"skill": "cas-worker"})),
        ("Workflow", serde_json::json!({"name": "find-flaky-tests"})),
    ] {
        let out = handle_pre_tool_use(&hook_input(tool, payload), Some(tmp.path()))
            .expect("handler ok");
        assert!(
            deny_reason(&out).is_none(),
            "unrelated {tool} call must not be refused"
        );
    }
}

/// Non-factory sessions (no role) are outside the factory policy entirely.
#[test]
fn non_factory_sessions_are_not_gated_cas_bcfb() {
    let _env = TestEnvGuard::with_optional_vars(&[("CAS_AGENT_ROLE", None)]);
    let tmp = tempfile::tempdir().expect("tempdir");

    let out = handle_pre_tool_use(
        &hook_input("Skill", serde_json::json!({"skill": "cas-code-review"})),
        Some(tmp.path()),
    )
    .expect("handler ok");

    assert!(
        deny_reason(&out).is_none(),
        "a non-factory session must not be gated"
    );
}
