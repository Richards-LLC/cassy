use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_jev(registry: &mut ConfigRegistry) {
    registry
        .section_descriptions
        .insert("jev", "Calibrated Jev decision evaluations");
    for (key, name, description, value_type, default, constraint) in [
        (
            "jev.gate.shadow",
            "Jev gate shadow",
            "Observe Bash/Write/Edit risk without changing hook decisions; default off.",
            ConfigType::Bool,
            "false",
            Constraint::None,
        ),
        (
            "jev.model",
            "Jev model",
            "Pinned TypeSafe decision model.",
            ConfigType::String,
            "jev-1.13.0",
            Constraint::NotEmpty,
        ),
        (
            "jev.key_file",
            "Jev key file",
            "Optional path to a development TypeSafe key; uses direct transport.",
            ConfigType::String,
            "",
            Constraint::None,
        ),
        (
            "jev.enabled",
            "Jev enabled",
            "Enable Jev evaluations; disabled advisory calls return unavailable.",
            ConfigType::Bool,
            "true",
            Constraint::None,
        ),
    ] {
        registry.register(ConfigMeta {
            key,
            section: "jev",
            name,
            description,
            value_type,
            default,
            constraint,
            advanced: false,
            requires_feature: None,
            keywords: &["jev", "typesafe", "decisions"],
            use_cases: &["Configure calibrated decision evaluation"],
        });
    }
}
