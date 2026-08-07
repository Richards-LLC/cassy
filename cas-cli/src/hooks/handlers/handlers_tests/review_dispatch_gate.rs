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

// ============================================================================
// cas-62b0 / GH #152 — the persona fan-out is an entry path of its own.
//
// Every test above refuses the ORCHESTRATOR. The orchestrator is cheap; the
// personas are the ~500k subagent tokens. A worker that reads the skill body
// and spawns the personas itself reaches the pipeline without touching
// `Skill` or `Workflow` — and the close gate's own remediation text used to
// point workers at exactly that route ("via the Skill or Task tool").
// ============================================================================

/// PATH 5 — worker spawns the pipeline as a subagent (`Task`, legacy spelling).
#[test]
fn worker_task_tool_dispatch_of_the_review_is_refused_cas_62b0() {
    let _env = worker_env();
    let tmp = tempfile::tempdir().expect("tempdir");

    let out = handle_pre_tool_use(
        &hook_input(
            "Task",
            serde_json::json!({
                "subagent_type": "general-purpose",
                "description": "run cas-code-review",
                "prompt": "Run cas-code-review over the current diff and return the envelope."
            }),
        ),
        Some(tmp.path()),
    )
    .expect("handler ok");

    assert_is_the_cas_4fef_refusal(
        &deny_reason(&out).expect("Task-tool review dispatch must be refused"),
        "Task-tool review dispatch",
    );
}

/// PATH 6 — same thing under the current `Agent` spelling.
///
/// This is the spelling that had no seam at all: neither generated matcher
/// listed `Agent` before cas-62b0, so the hook was never even invoked for it.
#[test]
fn worker_agent_tool_dispatch_of_the_review_is_refused_cas_62b0() {
    let _env = worker_env();
    let tmp = tempfile::tempdir().expect("tempdir");

    let out = handle_pre_tool_use(
        &hook_input(
            "Agent",
            serde_json::json!({
                "subagent_type": "cas-code-review",
                "prompt": "You are the correctness persona. Review the diff."
            }),
        ),
        Some(tmp.path()),
    )
    .expect("handler ok");

    assert_is_the_cas_4fef_refusal(
        &deny_reason(&out).expect("Agent-tool review dispatch must be refused"),
        "Agent-tool review dispatch",
    );
}

/// The refusal must name the config key AND how to read it back.
///
/// GH #152's first symptom was `cas config get code_review.owner` answering
/// "Unknown config key" for a setting that was in force. A refusal that cites
/// a setting nobody can verify is how "the gate is advisory" gets believed.
#[test]
fn the_refusal_names_the_config_and_how_to_verify_it_cas_62b0() {
    let _env = worker_env();
    let tmp = tempfile::tempdir().expect("tempdir");

    let out = handle_pre_tool_use(
        &hook_input("Agent", serde_json::json!({"prompt": "cas-code-review personas"})),
        Some(tmp.path()),
    )
    .expect("handler ok");

    let reason = deny_reason(&out).expect("must be refused");
    assert!(
        reason.contains("[code_review] owner = \"supervisor\""),
        "refusal must name the config: {reason}"
    );
    assert!(
        reason.contains("cas config get code_review.owner"),
        "refusal must name the readback command: {reason}"
    );
    assert!(
        reason.contains("Task/Agent"),
        "refusal must close the hand-spawned-persona route by name: {reason}"
    );
}

/// The sealed task-verifier handoff must be untouched.
///
/// `task-verifier` spawns carry server-side authority minted in this same
/// handler and their prompts legitimately quote review findings. Refusing one
/// would break close verification in the name of saving review tokens.
#[test]
fn task_verifier_spawns_are_never_refused_by_the_review_gate_cas_62b0() {
    let _env = worker_env();
    let tmp = tempfile::tempdir().expect("tempdir");

    for tool in ["Task", "Agent"] {
        let out = handle_pre_tool_use(
            &hook_input(
                tool,
                serde_json::json!({
                    "subagent_type": "task-verifier",
                    "prompt": "Verify task cas-1234. Prior cas-code-review envelope: {...}"
                }),
            ),
            Some(tmp.path()),
        )
        .expect("handler ok");

        assert_ne!(
            deny_reason(&out).as_deref(),
            Some(crate::code_review_dispatch::supervisor_owned_review_refusal().as_str()),
            "{tool}(task-verifier) must not be refused as a review dispatch"
        );
    }
}

/// Ordinary subagent work must pass through untouched.
///
/// Over-refusal here would be worse than the bug: a worker that cannot spawn
/// an Explore agent cannot work at all.
#[test]
fn unrelated_subagent_spawns_are_untouched_cas_62b0() {
    let _env = worker_env();
    let tmp = tempfile::tempdir().expect("tempdir");

    for tool in ["Task", "Agent"] {
        let out = handle_pre_tool_use(
            &hook_input(
                tool,
                serde_json::json!({
                    "subagent_type": "Explore",
                    "description": "locate the close gate",
                    "prompt": "Find where the close gate rejects a missing envelope."
                }),
            ),
            Some(tmp.path()),
        )
        .expect("handler ok");

        assert!(
            deny_reason(&out).is_none(),
            "an unrelated {tool} spawn must not be refused"
        );
    }
}

/// `owner = "worker"` keeps the fan-out legal too — the escape hatch is not
/// silently narrowed by covering more entry paths.
#[test]
fn worker_owned_projects_may_still_spawn_personas_cas_62b0() {
    let _env = worker_env();
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        tmp.path().join("config.toml"),
        "[code_review]\nowner = \"worker\"\n",
    )
    .expect("write config");

    let out = handle_pre_tool_use(
        &hook_input(
            "Agent",
            serde_json::json!({"prompt": "Run cas-code-review persona: security"}),
        ),
        Some(tmp.path()),
    )
    .expect("handler ok");

    assert!(
        deny_reason(&out).is_none(),
        "`owner = \"worker\"` must still permit the worker-run fan-out"
    );
}

/// Every recognized entry tool must be reachable by the hook.
///
/// The gate is only as real as the matcher that invokes it: cas-bcfb's gate
/// was compiled into the binary that ran 8 personas because the tool it
/// needed to see was not in any matcher. This test fails the moment
/// `REVIEW_ENTRY_TOOLS` grows an entry the generated settings do not route.
#[test]
fn every_review_entry_tool_is_routed_to_the_hook_cas_62b0() {
    let factory_matcher =
        crate::ui::factory::daemon::runtime::teams::TeamsManager::factory_pre_tool_intercept_list();
    let generated_matcher = crate::config::hooks::default_pre_tool_use_matcher();

    for tool in crate::code_review_dispatch::REVIEW_ENTRY_TOOLS {
        // `Task` is the legacy spelling of `Agent`; the factory settings route
        // the current one. Both are recognized by the gate so a harness that
        // still emits `Task` (and whose config-dir settings match it) is
        // covered, but only the current spelling is required in the factory
        // matcher — adding `Task` there would newly activate the sealed
        // verifier handoff for factory agents, which is a separate change.
        if *tool == "Task" {
            assert!(
                generated_matcher.iter().any(|t| t == tool),
                "generated settings must route {tool} to PreToolUse"
            );
            continue;
        }
        assert!(
            factory_matcher.contains(tool),
            "factory settings must route {tool} to PreToolUse, else the gate cannot fire"
        );
        assert!(
            generated_matcher.iter().any(|t| t == tool),
            "generated settings must route {tool} to PreToolUse"
        );
    }
}
