use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_memory(registry: &mut ConfigRegistry) {
    registry.register(ConfigMeta {
        key: "memory.session_learn_auto",
        section: "memory",
        name: "Automatic Session Learning",
        description: "Run the session-learn classifier automatically at Stop. Defaults to false.",
        value_type: ConfigType::Bool,
        default: "false",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["memory", "session", "learn", "automatic"],
        use_cases: &["Opt into automatic session learning at Stop"],
    });

    registry.register(ConfigMeta {
        key: "memory.decay.curated_importance_floor",
        section: "memory.decay",
        name: "Curated Importance Floor",
        description: "Importance at or above this value protects memories from stability decay below the working tier. The measured retrieval knee defaults to 0.9.",
        value_type: ConfigType::Float,
        default: "0.9",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["memory", "decay", "curated", "importance", "floor", "working"],
        use_cases: &["Tune the curated-memory decay protection threshold"],
    });

    registry.register(ConfigMeta {
        key: "memory.decay.promote_on_access",
        section: "memory.decay",
        name: "Promote Memories on Access",
        description: "Promote cold and archive-tier memories to working when accessed. Defaults to true.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["memory", "decay", "access", "promotion", "working"],
        use_cases: &["Disable automatic tier promotion for controlled experiments"],
    });
}
