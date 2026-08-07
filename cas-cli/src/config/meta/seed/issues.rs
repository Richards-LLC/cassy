use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_issues(registry: &mut ConfigRegistry) {
    registry.register(ConfigMeta {
        key: "issues.repo",
        section: "issues",
        name: "Issue Intake Repository",
        description: "GitHub repository in owner/repo form for CAS-system bug reports. This is project-local and intentionally has no inferred default: a downstream project's origin may not be the CAS upstream.",
        value_type: ConfigType::String,
        default: "",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["issues", "github", "bugs", "intake", "repository", "upstream"],
        use_cases: &[
            "Route CAS-system bugs from a downstream project to its configured CAS upstream",
            "Leave empty to preserve reports locally until an explicit target is configured",
        ],
    });
    register_code_review(registry);
}

/// `[code_review]` — who owns the multi-persona review pipeline (cas-62b0).
///
/// The struct and every runtime gate have honoured this key since cas-b51a,
/// but it was never registered here, so `cas config get/set/describe
/// code_review.owner` answered "Unknown config key" for a setting that was
/// in force. A policy nobody can read back is a policy nobody can audit —
/// GH #152 is what that costs.
fn register_code_review(registry: &mut ConfigRegistry) {
    registry.register(ConfigMeta {
        key: "code_review.owner",
        section: "code_review",
        name: "Code Review Owner",
        description: "Who dispatches the multi-persona cas-code-review pipeline. \"supervisor\" (default since cas-865b): factory workers must NOT run it — their close runs a lightweight structural lint and hands the branch to the supervisor's review queue. \"worker\": legacy inline dispatch, where each worker runs the full pipeline before its own close.",
        value_type: ConfigType::String,
        default: "supervisor",
        constraint: Constraint::OneOf(vec!["supervisor".to_string(), "worker".to_string()]),
        advanced: false,
        requires_feature: None,
        keywords: &[
            "code review",
            "cas-code-review",
            "personas",
            "owner",
            "supervisor",
            "worker",
            "review queue",
            "dispatch",
        ],
        use_cases: &[
            "Keep workers out of the persona pipeline so review cost is paid once, by the supervisor",
            "Set to \"worker\" to restore the legacy per-close inline review",
            "Read back with `cas config get code_review.owner` to confirm which gate a project is under",
        ],
    });
}
