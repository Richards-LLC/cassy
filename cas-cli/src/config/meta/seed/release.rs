use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_release(registry: &mut ConfigRegistry) {
    registry.register(ConfigMeta {
        key: "release.claude_account_allowlist",
        section: "release",
        name: "Approved Claude Accounts",
        description: "E-mail addresses approved for the one-shot `claude -p` route used by release-note posting, compared case-insensitively against `claude auth status --json`. Empty by default and the gate fails closed, so an unconfigured project approves no Claude account.",
        value_type: ConfigType::StringList,
        default: "",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &[
            "release",
            "claude",
            "account",
            "allowlist",
            "routing",
            "notes",
        ],
        use_cases: &[
            "Approve the accounts that may post release notes through the one-shot Claude route",
            "Leave empty to keep the Claude fallback closed for this project",
        ],
    });
    for (key, name, description, default, constraint) in [
        ("release.auto_cut_max_per_day", "Auto-Cut Releases Per Day",
         "Automatic cuts of a green release train allowed in any 24 hours. Cassy cuts a train whose exact tip has a green full-suite receipt and changelog fragments once the supervisor's notes draft exists, with no queue run in flight and no Commander pause. 0 turns auto-cut off.",
         "4", Constraint::Range(0, 24)),
        ("release.auto_cut_min_interval_hours", "Auto-Cut Minimum Interval",
         "Minimum hours between one release freeze and the next automatic cut.",
         "2", Constraint::Range(1, 168)),
    ] {
        registry.register(ConfigMeta {
            key,
            section: "release",
            name,
            description,
            value_type: ConfigType::Int,
            default,
            constraint,
            advanced: false,
            requires_feature: None,
            keywords: &["release", "auto-cut", "cadence", "train"],
            use_cases: &["Bound how often Cassy cuts a green release train on its own"],
        });
    }
}
