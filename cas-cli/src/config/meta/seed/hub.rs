use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_hub(registry: &mut ConfigRegistry) {
    registry
        .section_descriptions
        .insert("hub", "Machine hub publication");
    registry.register(ConfigMeta {
        key: "hub.tailscale_serve",
        section: "hub",
        name: "Tailscale Serve",
        description: "Publish the machine hub through tailnet-only HTTPS by default. Read from ~/.cas/config.toml. Set false for loopback-only startup and updates; --tailscale-serve explicitly enables a launch and --no-tailscale-serve explicitly disables it. Unavailable Tailscale leaves a healthy loopback hub with a warning.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["hub", "tailscale", "serve", "https", "loopback"],
        use_cases: &["Keep the hub reachable from Commander after updates", "Opt out of Tailscale Serve in the host configuration"],
    });
}
