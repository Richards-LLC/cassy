use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_hub(registry: &mut ConfigRegistry) {
    registry
        .section_descriptions
        .insert("hub", "Machine hub publication");
    registry.register(ConfigMeta {
        key: "hub.transport_mode",
        section: "hub",
        name: "Transport mode",
        description: "Persistent machine hub policy from ~/.cas/config.toml. auto tries Tailscale Serve with an interactive loopback warning (macOS service failures exit so launchd retries); required refuses loopback fallback; loopback never publishes. Restart and update honor this setting. CLI publication flags override a single launch.",
        value_type: ConfigType::String,
        default: "auto",
        constraint: Constraint::OneOf(vec!["auto".into(), "required".into(), "loopback".into()]),
        advanced: false,
        requires_feature: None,
        keywords: &["hub", "transport", "tailscale", "loopback", "required"],
        use_cases: &["Set the host transport once and preserve it through updates"],
    });
    registry.register(ConfigMeta {
        key: "hub.tailscale_serve",
        section: "hub",
        name: "Tailscale Serve",
        description: "Legacy fallback when hub.transport_mode is unset: true selects auto and false selects loopback. Prefer hub.transport_mode for persistent host policy.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["hub", "tailscale", "serve", "https", "loopback"],
        use_cases: &["Keep the hub reachable from Commander after updates", "Opt out of Tailscale Serve in the host configuration"],
    });
}
