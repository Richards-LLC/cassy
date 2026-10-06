//! Bounded, content-free connection evidence. No origins, paths or credentials.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use serde::Serialize;

#[derive(Clone, Default)]
pub(super) struct RecoveryTelemetry(Arc<Mutex<Report>>);
#[derive(Default, Serialize)]
pub(super) struct Report {
    counts: Vec<Count>,
    refusals: BTreeMap<String, u64>,
}
#[derive(Serialize)]
struct Count {
    category: &'static str,
    preflight: bool,
    status: u16,
    count: u64,
    request_id: String,
}
pub(super) fn category(path: &str) -> &'static str {
    match path {
        "/v1/health" => "health",
        "/v1/machine" => "machine",
        "/v1/sessions" => "sessions",
        "/v1/events" => "events",
        "/v1/diagnostics" => "diagnostics",
        value if value.starts_with("/v1/auth/") => "auth",
        _ => "other",
    }
}
pub(super) fn refusal(value: &str) -> &'static str {
    match value {
        "expired" => "expired", "revoked" => "revoked",
        "unknown_credential" => "unknown_credential", "scope_mismatch" => "scope_mismatch",
        "stale_proof" => "stale_proof", "proof_replay" => "proof_replay",
        "invalid_proof" => "invalid_proof", "viewer_lagged" => "viewer_lagged",
        _ => "other",
    }
}
impl RecoveryTelemetry {
    /// Return a sampled audit count: first occurrence and powers of two only.
    pub(super) fn record(&self, category: &'static str, preflight: bool, status: u16, request_id: &str, reason: Option<&str>) -> Option<u64> {
        let mut report = self.0.lock().expect("connection evidence lock poisoned");
        if let Some(reason) = reason {
            let count = report.refusals.entry(refusal(reason).to_owned()).or_default();
            *count = count.saturating_add(1);
        }
        let index = report.counts.iter().position(|row| row.category == category && row.preflight == preflight && row.status == status);
        let index = match index {
            Some(index) => index,
            None => {
                if report.counts.len() == 64 { report.counts.remove(0); }
                report.counts.push(Count { category, preflight, status, count: 0, request_id: String::new() });
                report.counts.len() - 1
            }
        };
        let row = &mut report.counts[index];
        row.count = row.count.saturating_add(1);
        row.request_id = request_id.to_owned();
        row.count.is_power_of_two().then_some(row.count)
    }
    pub(super) fn snapshot(&self) -> serde_json::Value {
        serde_json::to_value(&*self.0.lock().expect("connection evidence lock poisoned")).expect("fixed evidence schema")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connection_evidence_is_bounded_and_classifies_secret_paths_cas_2b3a5() {
        let evidence = RecoveryTelemetry::default();
        for status in 100..200 { evidence.record(category("/v1/sessions/secret-prompt"), true, status, "test-request", Some("secret-token")); }
        let value = evidence.snapshot();
        assert_eq!(value["counts"].as_array().unwrap().len(), 64);
        assert_eq!(value["refusals"]["other"], 100);
        assert!(!value.to_string().contains("secret"));
    }
}
