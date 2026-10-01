use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_slack(registry: &mut ConfigRegistry) {
    registry
        .section_descriptions
        .insert("slack", "Slack transport policy");
    registry.register(ConfigMeta {
        key: "slack.transport",
        section: "slack",
        name: "Slack Transport",
        description: "Violet is the canonical transport for Slack reads and posts (violet.violet_read / violet.violet_post, violet skill). By default, pre-tool hooks deny writes through other Slack integrations in every session. Set any to explicitly disable this write guard. Codex uses generated agent guidance and doctor diagnostics where MCP pre-tool hooks are unavailable.",
        value_type: ConfigType::String,
        default: "violet",
        constraint: Constraint::OneOf(vec!["violet".into(), "any".into()]),
        advanced: false,
        requires_feature: None,
        keywords: &["slack", "violet", "transport", "connector", "policy"],
        use_cases: &["Require Violet for Slack writes", "Explicitly permit another Slack integration with cas config set slack.transport any"],
    });
}
