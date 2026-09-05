use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};
use crate::config::settings::{
    DEFAULT_CASSY_ISSUES_REPO, DEFAULT_CLOUD_ISSUES_REPO, DEFAULT_MECHA_CASSY_ISSUES_REPO,
};

pub(super) fn register_issues(registry: &mut ConfigRegistry) {
    registry.register(ConfigMeta {
        key: "issues.repo",
        section: "issues",
        name: "Issue Intake Repository",
        description: "GitHub repository in owner/repo form for Cassy-system bug reports. This is project-local and intentionally has no inferred default: a downstream project's origin may not be the Cassy upstream.",
        value_type: ConfigType::String,
        default: "",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["issues", "github", "bugs", "intake", "repository", "upstream"],
        use_cases: &[
            "Route Cassy-system bugs from a downstream project to its configured Cassy upstream",
            "Leave empty to preserve reports locally until an explicit target is configured",
        ],
    });
    registry.register(ConfigMeta {
        key: "issues.components.cassy",
        section: "issues.components",
        name: "Cassy Issue Repository",
        description: "GitHub repository for Cassy runtime, hooks, MCP, factory, and skill bugs.",
        value_type: ConfigType::String,
        default: DEFAULT_CASSY_ISSUES_REPO,
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["issues", "github", "bugs", "repository", "component"],
        use_cases: &["Override only when operating a fork or alternate Cassy distribution"],
    });
    registry.register(ConfigMeta {
        key: "issues.components.mecha_cassy",
        section: "issues.components",
        name: "MechaCassy Issue Repository",
        description: "GitHub repository for MechaCassy Slack hub and message delivery bugs.",
        value_type: ConfigType::String,
        default: DEFAULT_MECHA_CASSY_ISSUES_REPO,
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["issues", "github", "bugs", "repository", "component"],
        use_cases: &["Route bugs in the MechaCassy hub to its component repository"],
    });
    registry.register(ConfigMeta {
        key: "issues.components.cloud",
        section: "issues.components",
        name: "Cassy Cloud Issue Repository",
        description: "GitHub repository for Cassy Cloud sync, hub relay, and pairing bugs.",
        value_type: ConfigType::String,
        default: DEFAULT_CLOUD_ISSUES_REPO,
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["issues", "github", "bugs", "repository", "component"],
        use_cases: &["Route bugs in Cassy Cloud services to their component repository"],
    });
}
