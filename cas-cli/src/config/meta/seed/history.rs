use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_history(registry: &mut ConfigRegistry) {
    registry.register(ConfigMeta {
        key: "history.github_repo",
        section: "history",
        name: "History GitHub Repository",
        description: "GitHub history repo; unset uses checkout origin; separate from issues.repo.",
        value_type: ConfigType::String,
        // An empty override is the config default; the resolver supplies the
        // checkout's GitHub origin when this remains unset.
        default: "",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["history", "github", "issues", "pull requests", "repository"],
        use_cases: &[
            "Override the GitHub repository used by the code-history document index",
            "Leave empty to use the checkout's GitHub origin",
        ],
    });
}
