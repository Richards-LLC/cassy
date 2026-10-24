use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_slack(registry: &mut ConfigRegistry) {
    registry.register(ConfigMeta {
        key: "slack.channels", section: "slack", name: "Project Slack Channels",
        description: "JSON list of project channel IDs (C... or G...). cas violet channels compares Violet discovery with Cloud verification; cas update reconciles configured channels through ordinary authorized repair. Declaring a channel grants no mapping or work authority.",
        value_type: ConfigType::String, default: "[]", constraint: Constraint::None,
        advanced: false, requires_feature: None,
        keywords: &["slack", "channel", "mapping", "binding"],
        use_cases: &["List every desired channel with cas violet channels", "Detect configured channels missing from Violet"],
    });
    registry.register(ConfigMeta {
        key: "slack.stakeholder_contacts", section: "slack", name: "Stakeholder Contact Policies",
        description: "JSON list of workspace_id, user_id, minimum_interval_secs and mode (refuse or warn). Cloud owns the shared policy and last-contact record. Until Cloud and Violet advertise stakeholder-contacts/v1, posts carry a local-only warning with the last locally confirmed contact; cross-machine exclusion is unavailable.",
        value_type: ConfigType::String, default: "[]", constraint: Constraint::None,
        advanced: false, requires_feature: None,
        keywords: &["stakeholder", "contact", "rate", "mention", "interval"],
        use_cases: &["Limit top-level stakeholder mentions across supervisors"],
    });
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
    registry.register(ConfigMeta {
        key: "slack.wake_enabled",
        section: "slack",
        name: "Violet Push-Wake",
        description: "When a person @-mentions Violet, or replies in a thread Violet started, in a Slack channel mapped to this project, the factory daemon claims that activity from Cassy Cloud, wakes the supervisor, and checks the channel every 5 minutes until 1 hour passes without human activity. Requires a Cloud login. Set false to stop claiming.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["slack", "violet", "wake", "mention", "supervisor", "watch"],
        use_cases: &["Wake the supervisor when someone mentions Violet", "Stop Slack activity from waking the supervisor"],
    });
    registry.register(ConfigMeta {
        key: "slack.violet_bot_user_ids",
        section: "slack",
        name: "Violet Bot User IDs",
        description: "Comma-separated Slack user ids of the Violet bot. Channel sweeps never count these authors as human activity. The daemon also learns the id from threads Violet started, so this is needed only before that happens.",
        value_type: ConfigType::String,
        default: "",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["slack", "violet", "bot", "user", "sweep"],
        use_cases: &["Keep Violet's own replies from re-waking the supervisor"],
    });
}
