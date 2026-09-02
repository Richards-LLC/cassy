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
}
