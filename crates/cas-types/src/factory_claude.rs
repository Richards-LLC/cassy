//! Operator-selected Claude factory launch behavior.
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FactoryClaudeConfig {
    /// Opt in to interactive Remote Control for the supervisor only.
    pub remote_control: bool,
    /// Explicit overload fallback; no model availability is inferred by Cassy.
    #[serde(
        deserialize_with = "deserialize_fallback",
        skip_serializing_if = "Option::is_none"
    )]
    pub fallback_model: Option<String>,
}

impl FactoryClaudeConfig {
    pub fn parse_fallback(value: &str) -> Result<Option<String>, String> {
        if value.chars().any(char::is_control) {
            return Err("factory.claude.fallback_model must not contain control characters".into());
        }
        let value = value.trim();
        Ok((!value.is_empty()).then(|| value.to_owned()))
    }
}

fn deserialize_fallback<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    value
        .map(|value| FactoryClaudeConfig::parse_fallback(&value))
        .transpose()
        .map(Option::flatten)
        .map_err(serde::de::Error::custom)
}
