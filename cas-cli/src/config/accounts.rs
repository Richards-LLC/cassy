use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AccountSelection {
    #[default]
    SoonestReset,
    MostHeadroom,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountsConfig {
    pub selection: AccountSelection,
    /// Accounts at or above this percentage in either window are excluded.
    pub threshold: u8,
}
impl Default for AccountsConfig {
    fn default() -> Self {
        Self { selection: AccountSelection::SoonestReset, threshold: 90 }
    }
}
