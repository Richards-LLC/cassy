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
}
