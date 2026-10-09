//! Shared production-hub retirement and credential fallback contract.
//! The adjacent manifest is the only source of legacy identifiers and URLs.
use serde::Deserialize;
use std::sync::LazyLock;

#[derive(Deserialize)]
pub struct VioletCompatibility {
    /// Canonical Violet hub MCP endpoint every managed registration names.
    pub hub_url: String,
    /// Former hostname of the same deployment. Still served during the switch;
    /// recognized only so installed registrations migrate to [`Self::hub_url`].
    pub legacy_hub_url: String,
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

/// True for the production hub endpoint under either hostname. A staging or
/// other custom endpoint is operator-owned and never matches.
pub fn is_violet_hub_url(url: &str) -> bool {
    let contract = violet_compatibility();
    url == contract.hub_url || url == contract.legacy_hub_url
}

/// The canonical spelling of a credential variable name: an installed-machine
/// legacy name maps to its `VIOLET_*` counterpart; any other name is returned
/// unchanged.
pub fn canonical_violet_credential_name(name: &str) -> String {
    violet_credential_names(name).swap_remove(0)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_hub_url_is_violet_hub_and_legacy_host_is_only_recognized() {
        let contract = violet_compatibility();
        assert_eq!(violet_hub_url(), "https://violet-hub.vercel.app/mcp/slack");
        assert_ne!(contract.legacy_hub_url, contract.hub_url);
        assert!(is_violet_hub_url(violet_hub_url()));
        assert!(is_violet_hub_url(&contract.legacy_hub_url));
        assert!(!is_violet_hub_url("https://staging.example/mcp/slack"));
    }

    #[test]
    fn legacy_credential_names_canonicalize_and_custom_names_survive() {
        let contract = violet_compatibility();
        let legacy_token = format!("{}_LAPTOP", contract.legacy_token_prefix);
        assert_eq!(
            canonical_violet_credential_name(&legacy_token),
            "VIOLET_SLACK_TOKEN_LAPTOP"
        );
        assert_eq!(
            canonical_violet_credential_name(&contract.legacy_bypass_env),
            "VIOLET_VERCEL_BYPASS"
        );
        assert_eq!(
            canonical_violet_credential_name("VIOLET_SLACK_TOKEN_LAPTOP"),
            "VIOLET_SLACK_TOKEN_LAPTOP"
        );
        assert_eq!(
            canonical_violet_credential_name("CUSTOM_TOKEN"),
            "CUSTOM_TOKEN"
        );
        // A prefix that is not followed by `_` is a different variable.
        let lookalike = format!("{}X", contract.legacy_token_prefix);
        assert_eq!(canonical_violet_credential_name(&lookalike), lookalike);
    }
}
