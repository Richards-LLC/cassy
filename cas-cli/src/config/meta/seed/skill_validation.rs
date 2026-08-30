use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_skill_validation(registry: &mut ConfigRegistry) {
    registry.register(ConfigMeta {
        key: "skill_validation.require_sandbox",
        section: "skill_validation",
        name: "Require Network Sandbox",
        description: "Require bubblewrap-backed network isolation for validation scripts. When false (the default), systems without bubblewrap use an env-scrubbed temporary-directory shell fallback and report that network isolation is unavailable.",
        value_type: ConfigType::Bool,
        default: "false",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["skills", "validation", "sandbox", "bubblewrap", "network", "security"],
        use_cases: &[
            "Fail closed when validation must not run without network isolation",
            "Leave disabled for CI and hosts where bubblewrap is unavailable",
        ],
    });
}
