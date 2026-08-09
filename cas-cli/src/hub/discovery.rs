//! Optional read-only CAS Cloud device hints for the Commander catalog.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cloud::CloudConfig;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudDeviceSuggestion {
    pub id: String,
    pub name: String,
    pub status: Option<String>,
    pub hub_url: Option<String>,
    pub ssh_host: Option<String>,
}

/// Best-effort by design: cloud discovery never affects hub readiness.
pub fn load_cloud_device_suggestions() -> Vec<CloudDeviceSuggestion> {
    let config = CloudConfig::load_user()
        .ok()
        .filter(CloudConfig::is_logged_in)
        .or_else(|| CloudConfig::load().ok().filter(CloudConfig::is_logged_in));
    config
        .as_ref()
        .and_then(fetch_cloud_device_suggestions)
        .unwrap_or_default()
}

fn fetch_cloud_device_suggestions(config: &CloudConfig) -> Option<Vec<CloudDeviceSuggestion>> {
    let token = config.token.as_deref()?;
    let url = format!("{}/api/devices", config.endpoint.trim_end_matches('/'));
    let response = ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_millis(1_500))
        .call()
        .ok()?;
    let body: Value = response.into_json().ok()?;
    Some(parse_cloud_device_suggestions(&body))
}

fn parse_cloud_device_suggestions(body: &Value) -> Vec<CloudDeviceSuggestion> {
    body.get("devices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|device| {
            let id = device.get("id")?.as_str()?.to_owned();
            let name = device.get("name")?.as_str()?.to_owned();
            let hub_url = ["hub_url", "commander_url", "tailscale_url"]
                .into_iter()
                .find_map(|field| device.get(field).and_then(Value::as_str))
                .filter(|url| url.starts_with("https://"))
                .map(str::to_owned);
            Some(CloudDeviceSuggestion {
                id,
                name,
                status: device
                    .get("status")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                hub_url,
                ssh_host: device
                    .pointer("/ssh_config/host")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_roster_is_suggestions_only_and_omits_unusable_rows() {
        let parsed = parse_cloud_device_suggestions(&serde_json::json!({"devices": [
            {"id":"one","name":"Laptop","status":"online","tailscale_url":"https://laptop.tail.ts.net/","ssh_config":{"host":"me@laptop"},"token":"never-copy"},
            {"id":"two","name":"Desktop","hub_url":"http://lan-host:4173"},
            {"id":"missing-name"}
        ]}));
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[0].hub_url.as_deref(),
            Some("https://laptop.tail.ts.net/")
        );
        assert_eq!(parsed[0].ssh_host.as_deref(), Some("me@laptop"));
        assert_eq!(
            parsed[1].hub_url, None,
            "plaintext remote hints are discarded"
        );
        assert!(
            !serde_json::to_string(&parsed)
                .unwrap()
                .contains("never-copy")
        );
    }
}
