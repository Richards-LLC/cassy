use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_notifications(registry: &mut ConfigRegistry) {
    // NOTIFICATIONS SECTION
    // ============================================================
    registry.register(ConfigMeta {
            key: "notifications.enabled",
            section: "notifications",
            name: "Enable Notifications",
            description: "Master switch for hook notifications: permission, idle and authentication prompts and the notification webhook.",
            value_type: ConfigType::Bool,
            default: "true",
            constraint: Constraint::None,
            advanced: false,
            requires_feature: None,
            keywords: &["notifications", "alerts", "hooks", "webhook"],
            use_cases: &[
                "Disable to silence every hook notification at once",
            ],
        });
}
