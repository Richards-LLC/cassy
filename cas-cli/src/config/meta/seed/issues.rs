use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};
use crate::config::settings::{
    DEFAULT_CASSY_ISSUES_REPO, DEFAULT_CLOUD_ISSUES_REPO, DEFAULT_VIOLET_ISSUES_REPO,
};

pub(super) fn register_issues(registry: &mut ConfigRegistry) {
    registry.register(ConfigMeta {
        key: "issues.repo",
        section: "issues",
        name: "Issue Intake Repository",
        description: "This project's GitHub issue repository in owner/repo form. CLI reads resolve an unset value from the local GitHub origin without saving it; doctor flags the unset setting. Cassy-system bug reports use effective issues.components.cassy, including its compiled default; only a raw explicitly blank component opts into this stored legacy override. They never use the inferred origin. Code-history indexing uses history.github_repo.",
        value_type: ConfigType::String,
        default: "",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["issues", "github", "bugs", "intake", "repository", "upstream"],
        use_cases: &[
            "Override the origin-derived project issue tracker",
            "Leave unset to resolve CLI reads from the local GitHub origin without writing config",
        ],
    });
    registry.register(ConfigMeta {
        key: "issues.components.cassy",
        section: "issues.components",
        name: "Cassy Issue Repository",
        description: "GitHub repository for Cassy runtime, hooks, MCP, factory, and skill bugs. report_cas_bug honors this effective destination, including the compiled default when unset. A raw explicitly blank component permits a stored issues.repo legacy fallback; clearing with config set restores the default. Invalid selected repositories stage the report without filing.",
        value_type: ConfigType::String,
        default: DEFAULT_CASSY_ISSUES_REPO,
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["issues", "github", "bugs", "repository", "component"],
        use_cases: &["Override only when operating a fork or alternate Cassy distribution"],
    });
    registry.register(ConfigMeta {
        key: "issues.components.violet",
        section: "issues.components",
        name: "Violet Issue Repository",
        description: "GitHub repository for Violet Slack hub and message delivery bugs.",
        value_type: ConfigType::String,
        default: DEFAULT_VIOLET_ISSUES_REPO,
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &[
            "issues",
            "github",
            "bugs",
            "repository",
            "component",
            "violet",
        ],
        use_cases: &["Route bugs in the Violet hub to its component repository"],
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
