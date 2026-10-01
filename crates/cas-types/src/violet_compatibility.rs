//! Shared production-hub retirement and credential fallback contract.
//! The adjacent manifest is the only source of legacy identifiers and URL.
use serde::Deserialize;
use std::sync::LazyLock;

#[derive(Deserialize)]
pub struct VioletCompatibility {
    pub hub_url: String,
    pub retired_server: String,
    pub retired_tools: [String; 2],
    pub retired_post_skill: String,
    pub retired_issue_key: String,
    pub legacy_token_prefix: String,
    pub legacy_bypass_env: String,
    pub legacy_token_selector_env: String,
    pub legacy_train_selector_env: String,
}

pub fn violet_compatibility() -> &'static VioletCompatibility {
    static CONTRACT: LazyLock<VioletCompatibility> = LazyLock::new(|| {
        serde_json::from_str(include_str!("violet-compatibility.json"))
            .expect("embedded Violet compatibility manifest must parse")
    });
    &CONTRACT
}

pub fn violet_hub_url() -> &'static str {
    &violet_compatibility().hub_url
}

/// Canonical credential first, corresponding installed-machine fallback second.
/// Custom variable names retain their original semantics.
pub fn violet_credential_names(name: &str) -> Vec<String> {
    let contract = violet_compatibility();
    let suffix = name
        .strip_prefix("VIOLET_SLACK_TOKEN")
        .or_else(|| name.strip_prefix(contract.legacy_token_prefix.as_str()))
        .filter(|suffix| suffix.is_empty() || suffix.starts_with('_'));
    if let Some(suffix) = suffix {
        return vec![
            format!("VIOLET_SLACK_TOKEN{suffix}"),
            format!("{}{suffix}", contract.legacy_token_prefix),
        ];
    }
    if name == "VIOLET_VERCEL_BYPASS" || name == contract.legacy_bypass_env {
        return vec![
            "VIOLET_VERCEL_BYPASS".into(),
            contract.legacy_bypass_env.clone(),
        ];
    }
    vec![name.to_owned()]
}
