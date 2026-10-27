use crate::config::meta::{ConfigMeta, ConfigRegistry, ConfigType, Constraint};

pub(super) fn register_accounts(registry: &mut ConfigRegistry) {
    registry.register(ConfigMeta {
        key: "accounts.disabled", section: "accounts", name: "Disabled accounts",
        description: "Absolute account folders excluded from automatic selection. Managed by cas accounts disable/enable.",
        value_type: ConfigType::StringList, default: "", constraint: Constraint::None,
        advanced: true, requires_feature: None, keywords: &["account", "disabled", "enable"],
        use_cases: &["Keep a subscription available for manual selection only"],
    });
    registry.register(ConfigMeta {
        key: "accounts.selection", section: "accounts", name: "Account selection",
        description: "Recommend the soonest weekly reset (within 24h prefer headroom), or the most weekly then 5h headroom. Manual choices and explicit directories always win.",
        value_type: ConfigType::String, default: "soonest-reset",
        constraint: Constraint::OneOf(vec!["soonest-reset".into(), "most-headroom".into()]),
        advanced: false, requires_feature: None, keywords: &["account", "recommend", "reset", "headroom", "claude", "codex"],
        use_cases: &["Choose a subscription for interactive launches and workers"],
    });
    registry.register(ConfigMeta {
        key: "accounts.threshold", section: "accounts", name: "Account usage threshold",
        description: "Exclude accounts at or above this usage percentage in either the 5h or weekly window from recommendations.",
        value_type: ConfigType::Int, default: "90", constraint: Constraint::Range(1, 100),
        advanced: false, requires_feature: None, keywords: &["account", "usage", "threshold", "capacity"],
        use_cases: &["Leave capacity in each subscription's rate-limit windows"],
    });
}
