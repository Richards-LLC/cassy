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
}
