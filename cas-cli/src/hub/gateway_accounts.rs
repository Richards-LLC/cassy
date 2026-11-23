//! Read-only hub account inventory for Commander's Accounts sheet.
//!
//! The machine's home gateway is described by its user-level
//! `[factory.gateway]` settings: readiness uses the worker probe, and the
//! upstream accounts come from the hub's existing dashboard. Neither key nor
//! address leaves this machine, and Codex Team accounts are never listed.
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

/// The only dashboard paths Cassy may request. Every request is a GET.
const READ_PATHS: &[&str] = &["/v0/management/auth-files"];

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DashboardError {
    /// A path outside [`READ_PATHS`] was asked for; nothing was sent.
    NotReadOnly,
    Failed(String),
}

pub(crate) struct Dashboard {
    endpoint: String,
    key: String,
}

impl Dashboard {
    /// The single request method: GET of an allowlisted read path.
    pub(crate) fn get(&self, path: &str) -> Result<serde_json::Value, DashboardError> {
        if !READ_PATHS.contains(&path) {
            return Err(DashboardError::NotReadOnly);
        }
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(2))
            .timeout(Duration::from_secs(5))
            .redirects(0)
            .build();
        agent
            .get(&format!("{}{path}", self.endpoint))
            .set("Authorization", &format!("Bearer {}", self.key))
            .call()
            .map_err(|error| match error {
                ureq::Error::Status(code, _) => DashboardError::Failed(format!("dashboard answered {code}")),
                ureq::Error::Transport(error) => DashboardError::Failed(format!("dashboard unreachable ({})", error.kind())),
            })?
            .into_json()
            .map_err(|_| DashboardError::Failed("dashboard answered with unreadable JSON".into()))
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct HubAccount {
    provider: String,
    /// Masked as the dashboard masks it: `pip***@example.com`.
    label: String,
    /// `active`, `disabled` or `unavailable`.
    state: &'static str,
    recent_requests: u64,
    recent_failed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    recent_minutes: Option<u32>,
}

#[derive(Debug, Serialize, Default)]
pub(crate) struct GatewayInventory {
    /// `off` (no hub configured), `ready` or `unavailable`.
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    host: Option<&'static str>,
    /// `ok`, `off` (no dashboard configured) or `unavailable`.
    inventory: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    inventory_reason: Option<String>,
    accounts: Vec<HubAccount>,
    /// Codex Team accounts present on the hub. Cassy does not use Team; they are counted, never listed.
    team_accounts: usize,
}

pub(crate) fn inventory() -> GatewayInventory {
    let settings = cas_mux::gateway::user_settings();
    let Some(endpoint) = settings.endpoint.as_deref().filter(|endpoint| !endpoint.trim().is_empty()) else {
        return GatewayInventory { status: "off", inventory: "off", ..Default::default() };
    };
    let readiness = cas_mux::gateway::readiness(&settings);
    let mut inventory = GatewayInventory {
        status: if readiness.is_ok() { "ready" } else { "unavailable" },
        reason: readiness.err().map(|reason| home_relative(&reason)),
        host: cas_mux::gateway::host_kind(endpoint),
        inventory: "off",
        ..Default::default()
    };
    let Some(dashboard) = settings.dashboard.as_deref().filter(|url| !url.trim().is_empty()) else {
        return inventory;
    };
    let read = dashboard_client(dashboard, settings.dashboard_key_file.as_deref())
        .and_then(|client| client.get(READ_PATHS[0]).map_err(|error| match error {
            DashboardError::NotReadOnly => "refused a non-read dashboard path".to_string(),
            DashboardError::Failed(reason) => reason,
        }));
    match read {
        Ok(files) => {
            let (accounts, team) = project(&files);
            inventory.inventory = "ok";
            inventory.accounts = accounts;
            inventory.team_accounts = team;
        }
        Err(reason) => {
            inventory.inventory = "unavailable";
            inventory.inventory_reason = Some(home_relative(&reason));
        }
    }
    inventory
}

fn dashboard_client(url: &str, key_file: Option<&str>) -> Result<Dashboard, String> {
    let endpoint = cas_mux::gateway::validate_endpoint(url).map_err(|_| "dashboard must be loopback or tailnet".to_string())?;
    let key_file = key_file
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(cas_pty::expand_codex_home(path)))
        .ok_or("factory.gateway.dashboard_key_file is not set")?;
    let key = cas_pty::gateway_key::read_gateway_key(&key_file)?;
    Ok(Dashboard { endpoint, key })
}

/// Auth-file rows → displayable accounts, dropping Codex Team plans.
fn project(files: &serde_json::Value) -> (Vec<HubAccount>, usize) {
    let mut team = 0;
    let mut accounts = Vec::new();
    for file in files["files"].as_array().into_iter().flatten() {
        let text = |key: &str| file[key].as_str().unwrap_or_default().trim().to_string();
        let provider = text("provider");
        let plan = file["id_token"]["plan_type"].as_str().unwrap_or_default();
        if provider == "codex" && plan.eq_ignore_ascii_case("team") {
            team += 1;
            continue;
        }
        let label = [text("email"), text("label"), text("account")]
            .into_iter()
            .find(|value| !value.is_empty())
            .unwrap_or_else(|| "unnamed account".into());
        let buckets = file["recent_requests"].as_array().cloned().unwrap_or_default();
        let count = |key: &str| buckets.iter().map(|bucket| bucket[key].as_u64().unwrap_or(0)).sum::<u64>();
        let state = if file["disabled"].as_bool() == Some(true) {
            "disabled"
        } else if file["unavailable"].as_bool() == Some(true) {
            "unavailable"
        } else {
            "active"
        };
        accounts.push(HubAccount {
            provider,
            label: mask(&label),
            state,
            recent_requests: count("success") + count("failed"),
            recent_failed: count("failed"),
            recent_minutes: bucket_minutes(&buckets),
        });
    }
    (accounts, team)
}

/// `"14:40-14:50"` buckets → the minutes they cover together.
fn bucket_minutes(buckets: &[serde_json::Value]) -> Option<u32> {
    let span = buckets.first()?["time"].as_str()?;
    let (start, end) = span.split_once('-')?;
    let minutes = |clock: &str| -> Option<i64> {
        let (hours, minutes) = clock.trim().split_once(':')?;
        Some(hours.parse::<i64>().ok()? * 60 + minutes.parse::<i64>().ok()?)
    };
    let width = (minutes(end)? - minutes(start)?).rem_euclid(24 * 60);
    u32::try_from(width * buckets.len() as i64).ok().filter(|total| *total > 0)
}

fn mask(label: &str) -> String {
    match label.split_once('@') {
        Some((local, domain)) => format!("{}***@{domain}", local.chars().take(3).collect::<String>()),
        None => label.to_string(),
    }
}

/// Readiness reasons may name the key file; keep only a home-relative form.
fn home_relative(reason: &str) -> String {
    match dirs::home_dir() {
        Some(home) => reason.replace(&home.display().to_string(), "~"),
        None => reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dashboard key is an admin key, so the client can only GET an
    /// allowlisted read path; anything else is refused before any request.
    #[test]
    fn dashboard_client_refuses_every_path_but_the_read_allowlist() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let client = Dashboard { endpoint: format!("http://{}", listener.local_addr().unwrap()), key: "k".into() };
        for path in ["/v0/management/auth-files/delete", "/v0/management/config", "/v0/management/api-keys", "/v0/management/auth-files?x=1"] {
            assert_eq!(client.get(path).unwrap_err(), DashboardError::NotReadOnly, "{path}");
        }
        assert!(listener.accept().is_err(), "a refused path must not reach the dashboard");
    }

    /// Team plans are counted, never listed; labels are masked.
    #[test]
    fn inventory_drops_codex_team_and_masks_labels() {
        let files = serde_json::json!({"files": [
            {"provider": "claude", "email": "hello@example.com", "disabled": false, "unavailable": false,
             "recent_requests": [{"time": "14:40-14:50", "success": 3, "failed": 1}, {"time": "14:50-15:00", "success": 2, "failed": 0}]},
            {"provider": "codex", "email": "team@example.com", "id_token": {"plan_type": "team"}},
            {"provider": "codex", "email": "pro@example.com", "id_token": {"plan_type": "pro"}, "disabled": true},
        ]});
        let (accounts, team) = project(&files);
        assert_eq!(team, 1);
        assert_eq!(
            accounts,
            vec![
                HubAccount { provider: "claude".into(), label: "hel***@example.com".into(), state: "active", recent_requests: 6, recent_failed: 1, recent_minutes: Some(20) },
                HubAccount { provider: "codex".into(), label: "pro***@example.com".into(), state: "disabled", recent_requests: 0, recent_failed: 0, recent_minutes: None },
            ]
        );
    }
}
