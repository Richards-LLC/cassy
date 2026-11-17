use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AccountSelection {
    SoonestReset,
    /// The default (cas-2820): the operator never has to manage accounts.
    #[default]
    MostHeadroom,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountsConfig {
    /// Absolute account folders excluded from automatic selection.
    pub disabled: Vec<String>,
    pub selection: AccountSelection,
    /// Accounts at or above this percentage in either window are excluded.
    pub threshold: u8,
}
impl Default for AccountsConfig {
    fn default() -> Self {
        Self {
            disabled: Vec::new(),
            selection: AccountSelection::MostHeadroom,
            threshold: 90,
        }
    }
}
