use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_skills(registry: &mut ConfigRegistry) {
    registry.register(ConfigMeta {
        key: "skills.optional",
        section: "skills",
        name: "Optional Builtin Skills",
        description: "Comma-separated optional builtin skill ids to enable for this project even when stack detection does not select them.",
        value_type: ConfigType::StringList,
        default: "",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["skills", "optional", "fallow", "nuxt", "stack", "project"],
        use_cases: &[
            "Enable fallow in a JavaScript or TypeScript repository without package metadata",
            "Enable cas-nuxt-playwright in a project whose Nuxt dependency is indirect",
        ],
    });
}
