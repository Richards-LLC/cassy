//! Explicit resource denials at the factory worker boundary.

use serde::{Deserialize, Serialize};

/// Project grants can narrow or authorize worker resources; these operator
/// denials always take precedence. Empty lists preserve the existing policy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FactoryWorkerPolicy {
    /// Explicit names authorized for worker CLI chores. Supervisor denials win.
    #[serde(default)]
    pub worker_credential_env: Vec<String>,
    #[serde(default)]
    pub supervisor_only_mcp: Vec<String>,
    #[serde(default)]
    pub supervisor_only_env: Vec<String>,
}

impl FactoryWorkerPolicy {
    pub fn denies_server(&self, name: &str) -> bool {
        self.supervisor_only_mcp
            .iter()
            .any(|candidate| candidate == name)
    }

    pub fn denies_env(&self, name: &str) -> bool {
        self.supervisor_only_env
            .iter()
            .any(|candidate| candidate == name)
    }

    pub fn is_empty(&self) -> bool {
        // Grants alone do not require native MCP scope isolation.
        self.supervisor_only_mcp.is_empty() && self.supervisor_only_env.is_empty()
    }
}
