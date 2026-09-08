use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_qa(registry: &mut ConfigRegistry) {
    registry.register(ConfigMeta {
        key: "qa.user_facing_labels",
        section: "qa",
        name: "User-Facing Task Labels",
        description: "Comma-separated labels that require a non-empty demo_statement when creating a task. Epics and supervisor overrides are exempt.",
        value_type: ConfigType::StringList,
        default: "ui,hub,cli-ux,commander,frontend",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["qa", "user-facing", "labels", "demo", "statement", "task"],
        use_cases: &[
            "Add project-specific labels such as mobile or public-api",
            "Clear the list when no labels should opt into the creation gate",
        ],
    });
}
