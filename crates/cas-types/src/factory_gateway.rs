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
}

impl FactoryGatewayConfig {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}
