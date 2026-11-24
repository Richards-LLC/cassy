//! Opt-in home-gateway transport for factory workers.
use serde::{Deserialize, Serialize};

/// `[factory.gateway]`. The user-level `~/.cas/config.toml` names the hub;
/// only a project's own `enabled = true` routes its workers through it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FactoryGatewayConfig {
    pub enabled: bool,
    /// Hub base URL without `/v1`, e.g. `http://127.0.0.1:8317`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Private client key file (mode 0600). Only the path is configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_file: Option<String>,
    /// Worker harnesses routed through the hub. Unset means claude and codex.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clients: Option<Vec<String>>,
    /// User-level only: the hub's existing usage dashboard (CPA Manager Plus),
    /// read for Commander's hub account inventory. Same private-host rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboard: Option<String>,
    /// User-level only: the dashboard's admin key file (mode 0600). Cassy uses
    /// it for read-only GETs and never sends it or the URL to Commander.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboard_key_file: Option<String>,
}

impl FactoryGatewayConfig {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An untouched config saves nothing, so projects that never opt in
    /// keep byte-identical config files.
    #[test]
    fn default_gateway_config_serializes_to_nothing() {
        #[derive(Serialize)]
        struct Factory {
            #[serde(skip_serializing_if = "FactoryGatewayConfig::is_default")]
            gateway: FactoryGatewayConfig,
        }
        let off = serde_json::to_string(&Factory { gateway: FactoryGatewayConfig::default() }).unwrap();
        assert_eq!(off, "{}");
        let on = FactoryGatewayConfig { enabled: true, ..Default::default() };
        assert!(!on.is_default());
    }
}
