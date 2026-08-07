//! cas-4fef: dispatch-layer ownership gate for `cas-code-review`.
//!
//! The prohibition "factory workers must not run the persona pipeline when
//! `[code_review] owner = \"supervisor\"`" previously existed only as prose —
//! in the skill description's tail and in per-assignment supervisor messages.
//! Prose lost twice in one session, and the reason it lost is documented in
//! [`supervisor_owned_review_refusal`]: the close path's own tool schema was
//! *instructing* workers to produce a review envelope. A worker following its
//! close instructions and a worker obeying the prohibition were being asked
//! for opposite things, and the instruction was the one it read at the moment
//! it mattered.
//!
//! This module is the enforcement seam: a pure decision the dispatch sites can
//! call without a store, a config file, or an environment probe of their own.

/// Canonical skill name of the multi-persona review orchestrator.
pub const CAS_CODE_REVIEW_SKILL: &str = "cas-code-review";

/// Whether an id/name refers to the multi-persona review skill.
///
/// Accepts the bare name and the `/`-prefixed slash-command spelling, because
/// Claude Code v2.1.146 renamed its built-in `/simplify` to `/code-review`
/// (memory 2026-05-26-5) and an agent reaching for the built-in reviewer can
/// arrive here with either spelling.
pub fn is_cas_code_review_skill(id: &str) -> bool {
    let normalized = id.trim().trim_start_matches('/').to_ascii_lowercase();
    normalized == CAS_CODE_REVIEW_SKILL || normalized == "cas_code_review"
}

/// Outcome of an attempted `cas-code-review` dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewDispatchDecision {
    /// The caller may run the pipeline.
    Allowed,
    /// Refused: a factory worker under supervisor-owned review.
    Refused { message: String },
}

impl ReviewDispatchDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }
}

/// The refusal text a blocked worker sees.
///
/// It names the alternative explicitly, because a refusal that only says "no"
/// leaves a worker mid-close with no legal next move — which is how the
/// original prohibition got rationalized away.
pub fn supervisor_owned_review_refusal() -> String {
    "⛔ cas-code-review is supervisor-owned in this project \
     (`[code_review] owner = \"supervisor\"`, the default since cas-865b).\n\n\
     A factory worker must NOT run the persona pipeline. Do this instead:\n\
     1. Attempt the close normally — do NOT pass `code_review_findings`. \
     Under supervisor-owned review the close transitions the task to \
     PendingSupervisorReview; you are not expected to produce a review envelope.\n\
     2. Tell your supervisor the branch is ready; the supervisor runs \
     cas-code-review on their own schedule.\n\n\
     To opt this project back into worker-run review, set \
     `[code_review] owner = \"worker\"` in .cas/config.toml."
        .to_string()
}

/// Decide whether this caller may dispatch `cas-code-review`.
///
/// Pure on purpose: both dispatch sites (skill use, and any headless entry
/// that grows later) resolve `is_factory_worker` / `supervisor_owned` from
/// their own context and get the identical verdict, so the two cannot drift.
///
/// The escape hatch is deliberate and tested: `supervisor_owned == false`
/// (i.e. `[code_review] owner = "worker"`) restores the legacy inline flow,
/// so a project that wants worker-run review keeps it.
pub fn review_dispatch_decision(
    is_factory_worker: bool,
    supervisor_owned: bool,
) -> ReviewDispatchDecision {
    if is_factory_worker && supervisor_owned {
        ReviewDispatchDecision::Refused {
            message: supervisor_owned_review_refusal(),
        }
    } else {
        ReviewDispatchDecision::Allowed
    }
}

/// Is this session a factory worker? (`CAS_AGENT_ROLE=worker` inside a factory.)
///
/// Mirrors the `is_factory_worker` probe the close gate already uses
/// (close_ops.rs, cas-8edb) so the gate and the close path cannot disagree
/// about who is a worker.
pub fn is_factory_worker_from_env() -> bool {
    std::env::var("CAS_AGENT_ROLE")
        .map(|r| r.eq_ignore_ascii_case("worker"))
        .unwrap_or(false)
        && std::env::var("CAS_FACTORY_MODE").is_ok()
}

/// Resolve `[code_review] owner` for a `.cas` root, defaulting to
/// supervisor-owned (cas-865b) when the config is absent or unreadable.
///
/// One function so every dispatch site answers "who owns review here?"
/// identically; a site that rolled its own `Config::load` chain could drift
/// into allowing what another site refuses.
pub fn supervisor_owned_at(cas_root: Option<&std::path::Path>) -> bool {
    cas_root
        .and_then(|root| crate::config::Config::load(root).ok())
        .and_then(|config| config.code_review)
        .map(|cr| cr.supervisor_owned())
        .unwrap_or_else(|| crate::config::CodeReviewConfig::default().supervisor_owned())
}

/// Harness-native tools that reach the review pipeline WITHOUT touching CAS
/// MCP (cas-bcfb / GH #125).
///
/// This is the gap that made the cas-4fef gate a no-op in practice: it was
/// installed only on `cas_skill_use`, i.e. `mcp__cas__skill action=use`. The
/// paths an agent actually takes are Claude Code's own `Skill` tool (which
/// reads `.claude/skills/cas-code-review/SKILL.md` off disk) and the `Workflow`
/// tool (which runs `.claude/workflows/cas-code-review.js` directly — the
/// Phase C cas-b667 workflow the skill is a thin wrapper around). Neither ever
/// calls into the MCP server, so neither could ever be refused.
pub const REVIEW_ENTRY_TOOLS: &[&str] = &["Skill", "Workflow"];

/// Tool-input fields that carry the identity of what is being dispatched.
///
/// `Workflow` gets `script` as well as `name`/`scriptPath` because an inline
/// script is a first-class way to invoke the pipeline (the skill body itself
/// documents pasting the workflow inline). Matching the inline body is
/// deliberately over-inclusive: a worker whose workflow merely mentions
/// `cas-code-review` is refused, and the refusal tells it the legal next move
/// — the failure direction we want under supervisor-owned review.
fn review_entry_identity_fields(tool_name: &str) -> &'static [&'static str] {
    match tool_name {
        "Skill" => &["skill", "name", "id"],
        "Workflow" => &["name", "scriptPath", "script"],
        _ => &[],
    }
}

/// Does this string name the multi-persona review orchestrator?
///
/// Superset of [`is_cas_code_review_skill`]: also true for values that merely
/// *contain* the name, so `.claude/workflows/cas-code-review.js` and an inline
/// `meta = { name: 'cas-code-review' }` are both recognized.
pub fn value_names_cas_code_review(value: &str) -> bool {
    if is_cas_code_review_skill(value) {
        return true;
    }
    let normalized = value.to_ascii_lowercase();
    normalized.contains(CAS_CODE_REVIEW_SKILL) || normalized.contains("cas_code_review")
}

/// Is this harness tool call an entry into the review pipeline?
///
/// Pure over the hook payload so every entry path shares one recognizer.
pub fn tool_call_enters_review(tool_name: &str, tool_input: Option<&serde_json::Value>) -> bool {
    if !REVIEW_ENTRY_TOOLS.contains(&tool_name) {
        return false;
    }
    let Some(input) = tool_input else {
        return false;
    };
    review_entry_identity_fields(tool_name)
        .iter()
        .filter_map(|field| input.get(field).and_then(|v| v.as_str()))
        .any(value_names_cas_code_review)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reported incident: worker role + default supervisor ownership.
    #[test]
    fn worker_under_supervisor_owned_review_is_refused_at_dispatch() {
        let decision = review_dispatch_decision(true, true);
        let ReviewDispatchDecision::Refused { message } = decision else {
            panic!("a factory worker must not reach the persona pipeline under owner=supervisor");
        };
        assert!(
            message.contains("supervisor-owned"),
            "refusal must name the reason"
        );
        assert!(
            message.contains("do NOT pass `code_review_findings`"),
            "refusal must correct the close-path instruction that caused the violation"
        );
        assert!(
            message.contains("owner = \"worker\""),
            "refusal must name the config that re-enables worker review"
        );
    }

    /// The documented escape hatch — owner=worker keeps the legacy flow.
    #[test]
    fn worker_under_worker_owned_review_is_allowed() {
        assert!(
            review_dispatch_decision(true, false).is_allowed(),
            "`[code_review] owner = \"worker\"` must still permit worker invocation"
        );
    }

    /// Supervisors own the pipeline under both configurations.
    #[test]
    fn supervisors_are_never_refused() {
        for supervisor_owned in [true, false] {
            assert!(
                review_dispatch_decision(false, supervisor_owned).is_allowed(),
                "non-worker callers must never be gated (supervisor_owned={supervisor_owned})"
            );
        }
    }

    /// The gate must catch the spellings an agent can actually arrive with,
    /// including the `/code-review` slash-command collision with Claude Code's
    /// renamed built-in.
    #[test]
    fn review_skill_is_recognized_by_every_spelling_agents_use() {
        for id in [
            "cas-code-review",
            "/cas-code-review",
            "CAS-Code-Review",
            "  cas-code-review  ",
            "cas_code_review",
        ] {
            assert!(is_cas_code_review_skill(id), "must gate {id:?}");
        }
        for id in ["cas-worker", "cas-supervisor", "code-review-queue"] {
            assert!(
                !is_cas_code_review_skill(id),
                "must not gate unrelated skill {id:?}"
            );
        }
    }

    /// cas-bcfb / GH #125: the harness-native entry paths the original gate
    /// never saw. Each shape below is one way a worker actually reached the
    /// persona fan-out on a binary that already contained the gate.
    #[test]
    fn every_harness_entry_path_into_the_review_is_recognized_cas_bcfb() {
        // 1. Claude Code `Skill` tool — the path loyal-heron-7 had available.
        assert!(tool_call_enters_review(
            "Skill",
            Some(&serde_json::json!({"skill": "cas-code-review", "args": "mode=interactive"}))
        ));
        assert!(tool_call_enters_review(
            "Skill",
            Some(&serde_json::json!({"skill": "/cas-code-review"}))
        ));
        // 2. Direct Workflow invocation by name.
        assert!(tool_call_enters_review(
            "Workflow",
            Some(&serde_json::json!({"name": "cas-code-review"}))
        ));
        // 3. Direct Workflow invocation by script path (the Phase C workflow).
        assert!(tool_call_enters_review(
            "Workflow",
            Some(&serde_json::json!({"scriptPath": ".claude/workflows/cas-code-review.js"}))
        ));
        // 4. Headless skill-to-skill — an inline script carrying the pipeline.
        assert!(tool_call_enters_review(
            "Workflow",
            Some(&serde_json::json!({
                "script": "export const meta = { name: 'cas-code-review', description: 'personas' }"
            }))
        ));
    }

    /// The gate must not swallow unrelated harness traffic.
    #[test]
    fn unrelated_tool_calls_are_not_treated_as_review_entries_cas_bcfb() {
        assert!(!tool_call_enters_review(
            "Skill",
            Some(&serde_json::json!({"skill": "cas-worker"}))
        ));
        assert!(!tool_call_enters_review(
            "Workflow",
            Some(&serde_json::json!({"name": "find-flaky-tests"}))
        ));
        assert!(!tool_call_enters_review("Skill", None));
        // Tools outside the entry list are never inspected, even if their
        // payload happens to mention the skill (e.g. a Bash grep for it).
        assert!(!tool_call_enters_review(
            "Bash",
            Some(&serde_json::json!({"command": "grep -r cas-code-review ."}))
        ));
    }

    /// Absent/unreadable config must resolve to supervisor-owned (cas-865b
    /// default), and an explicit `owner = "worker"` must still opt out.
    #[test]
    fn supervisor_ownership_defaults_on_and_respects_the_opt_out_cas_bcfb() {
        assert!(
            supervisor_owned_at(None),
            "no cas root must fall back to the supervisor-owned default"
        );

        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(
            supervisor_owned_at(Some(tmp.path())),
            "missing config.toml must fall back to the supervisor-owned default"
        );

        std::fs::write(
            tmp.path().join("config.toml"),
            "[code_review]\nowner = \"worker\"\n",
        )
        .expect("write config");
        assert!(
            !supervisor_owned_at(Some(tmp.path())),
            "`owner = \"worker\"` must opt the project out of the gate"
        );
    }
}
