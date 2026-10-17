//! Cloud's binding evidence, read only by explicit status requests. Never a claim gate.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

/// Preserve the original Cloud receipt, including explicit refusals. No cache.
pub(crate) fn observe(cas_root: &Path, registration: Option<&str>) -> Option<Value> {
    let registration = registration.filter(|id| !id.trim().is_empty())?;
    let unavailable = |code: &str| json!({"code": code});
    let Ok(config) = super::CloudConfig::load_from_cas_dir(cas_root) else {
        return Some(unavailable("activation_unavailable"));
    };
    let Some(token) = config.token.as_deref().filter(|token| !token.is_empty()) else {
        return Some(unavailable("not_logged_in"));
    };
    let response = ureq::get(&format!(
        "{}/api/coordination/bindings/activation",
        config.endpoint.trim_end_matches('/')
    ))
    .query("registration_id", registration)
    .set("Authorization", &format!("Bearer {token}"))
    .timeout(super::peers::DISCOVERY_TIMEOUT)
    .call();
    Some(match response {
        Ok(response) | Err(ureq::Error::Status(_, response)) => match response.into_json::<Value>() {
            Ok(payload) if payload["schema"] == "cassy.coordination/v1" => payload,
            _ => unavailable("invalid_response"),
        },
        Err(ureq::Error::Transport(_)) => unavailable("authority_unavailable"),
    })
}

pub(crate) fn line(payload: &Value) -> String {
    if payload["code"] != "completed" {
        return format!("Slack activation: {}", payload["code"].as_str().unwrap_or("unknown"));
    }
    let Some(bindings) = payload["bindings"].as_array() else {
        return "Slack activation: invalid_response".into();
    };
    let mut counts = BTreeMap::new();
    for binding in bindings {
        *counts.entry(binding["strict_activation"].as_str().unwrap_or("unknown")).or_insert(0) += 1;
    }
    if counts.is_empty() {
        "Slack activation: no bindings".into()
    } else {
        format!("Slack activation: {}", counts.into_iter().map(|(state, count)| format!("{state} {count}")).collect::<Vec<_>>().join(", "))
    }
}
