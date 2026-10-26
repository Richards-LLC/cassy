use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_viktor(registry: &mut ConfigRegistry) {
    registry.register(ConfigMeta {
        key: "viktor.enabled",
        section: "viktor",
        name: "Managed Viktor Upstream",
        description: "Enable Cassy's machine-managed Viktor MCP server. Read and written in ~/.cas/config.toml; disabling removes the managed user proxy server and allowlist entries and suppresses the session-start warning.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["viktor", "mcp", "upstream", "opt-out", "server"],
        use_cases: &["Disable the managed Viktor upstream", "Restore the managed Viktor upstream"],
    });
}
