//! Read-only hub account inventory for Commander's Accounts sheet.
//!
//! The machine's home gateway is described by its user-level
//! `[factory.gateway]` settings: readiness uses the worker probe, and the
//! upstream accounts come from the hub's existing dashboard. Neither key nor
//! address leaves this machine, and Codex Team accounts are never listed.
use crate::account_registry::{Account, UsageWindow};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// The only dashboard paths Cassy may request. Every request is a GET.
const READ_PATHS: &[&str] = &["/v0/management/auth-files", INSPECTION_RUNS];
/// The hub's latest codex-inspection runs: its timer reads each account's usage
/// through the provider's zero-cost usage endpoint. Cassy never starts a run.
const INSPECTION_RUNS: &str = "/v0/management/codex-inspection/runs?limit=5";
const INSPECTION_RUN: &str = "/v0/management/codex-inspection/runs/";

/// Allowlisted reads only: the fixed paths, or one inspection run by its numeric id.
fn is_read_path(path: &str) -> bool {
    READ_PATHS.contains(&path)
        || path.strip_prefix(INSPECTION_RUN).is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
}

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
        if !is_read_path(path) {
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
    inventory_with(&cas_mux::gateway::user_settings())
}

/// Routing readiness and the hub's accounts are independent: a machine with
/// only the dashboard keys (no endpoint) routes nothing, yet still lists the
/// hub's accounts read-only.
fn inventory_with(settings: &cas_types::factory_gateway::FactoryGatewayConfig) -> GatewayInventory {
    let mut inventory = match settings.endpoint.as_deref().filter(|endpoint| !endpoint.trim().is_empty()) {
        None => GatewayInventory { status: "off", inventory: "off", ..Default::default() },
        Some(endpoint) => {
            let readiness = cas_mux::gateway::readiness(settings);
            GatewayInventory {
                status: if readiness.is_ok() { "ready" } else { "unavailable" },
                reason: readiness.err().map(|reason| home_relative(&reason)),
                host: cas_mux::gateway::host_kind(endpoint),
                inventory: "off",
                ..Default::default()
            }
        }
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

/// The quota the hub holds for each login: one number for one account on
/// every machine that reads this hub. Reading it needs only the user-level
/// dashboard keys, never the gateway endpoint, so pointing a machine's
/// Accounts here never routes its workers through the hub.
#[derive(Debug)]
pub(crate) enum SharedQuota {
    /// No dashboard configured: each machine shows its own read.
    Off,
    /// Configured but unreadable: no number is shown as the shared one.
    Unreachable(String),
    Read(Vec<HubQuota>),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HubQuota {
    provider: String,
    /// Lowercased login email; matched server-side, never sent to Commander.
    email: String,
    /// The provider account id (Codex `id_token.chatgpt_account_id`); matched
    /// server-side against the login's own rollout identity, never rendered or logged.
    account_id: Option<String>,
    windows: Vec<UsageWindow>,
    observed_at: DateTime<Utc>,
}

/// Where a row's capacity came from: `hub` (shared), `local` (this machine's
/// own read; the hub has none for it) or `hub_unreachable` (none shown).
pub(crate) type UsageSource = &'static str;

pub(crate) fn shared_quota() -> SharedQuota {
    shared_quota_with(&cas_mux::gateway::user_settings())
}

fn shared_quota_with(settings: &cas_types::factory_gateway::FactoryGatewayConfig) -> SharedQuota {
    let Some(dashboard) = settings.dashboard.as_deref().filter(|url| !url.trim().is_empty()) else {
        return SharedQuota::Off;
    };
    let client = match dashboard_client(dashboard, settings.dashboard_key_file.as_deref()) {
        Ok(client) => client,
        Err(reason) => return SharedQuota::Unreachable(home_relative(&reason)),
    };
    match client.get(READ_PATHS[0]) {
        // The inspection is a freshness top-up: without it, traffic signals still count.
        Ok(files) => SharedQuota::Read(quotas(&files, latest_inspection(&client).as_ref())),
        Err(DashboardError::NotReadOnly) => SharedQuota::Unreachable("refused a non-read dashboard path".into()),
        Err(DashboardError::Failed(reason)) => SharedQuota::Unreachable(home_relative(&reason)),
    }
}

/// The newest completed codex-inspection run, with its per-account results.
fn latest_inspection(client: &Dashboard) -> Option<serde_json::Value> {
    let runs = client.get(INSPECTION_RUNS).ok()?;
    let run = runs["items"].as_array()?.iter().find(|run| run["status"] == "completed")?;
    client.get(&format!("{INSPECTION_RUN}{}", run["id"].as_u64()?)).ok()
}

/// An inspection run's quota windows per auth index, with when they were read.
fn inspection_windows(run: &serde_json::Value) -> HashMap<String, (Vec<UsageWindow>, DateTime<Utc>)> {
    let finished = run["finishedAtMs"].as_i64().or_else(|| run["run"]["finishedAtMs"].as_i64());
    let mut out = HashMap::new();
    for result in run["results"].as_array().into_iter().flatten() {
        let Some(index) = result["authIndex"].as_str().filter(|index| !index.is_empty()) else { continue };
        let Some(read) = result["createdAtMs"].as_i64().or(finished).and_then(DateTime::from_timestamp_millis) else { continue };
        let windows: Vec<UsageWindow> = result["quotaWindows"].as_array().into_iter().flatten().filter_map(|window| {
            let name = match window["limitWindowSeconds"].as_i64()? {
                18_000 => "5h",
                604_800 => "weekly",
                _ => return None,
            };
            Some(UsageWindow {
                name: name.into(),
                used_percent: window["usedPercent"].as_f64(),
                resets_at: window["resetAtMs"].as_i64().and_then(DateTime::from_timestamp_millis),
            })
        }).collect();
        if !windows.is_empty() {
            out.insert(index.to_string(), (windows, read));
        }
    }
    out
}

impl SharedQuota {
    /// `off`, `hub` or `unreachable`, with the reason for the last.
    pub(crate) fn state(&self) -> (&'static str, Option<&str>) {
        match self {
            SharedQuota::Off => ("off", None),
            SharedQuota::Unreachable(reason) => ("unreachable", Some(reason)),
            SharedQuota::Read(_) => ("hub", None),
        }
    }

    /// Replace each matched login's local read with the hub's. An unreachable
    /// hub clears every read: a local number is never shown as the shared one.
    /// Returns each account's source, by folder.
    pub(crate) fn apply(&self, accounts: &mut [Account]) -> HashMap<PathBuf, UsageSource> {
        let mut sources = HashMap::new();
        for account in accounts.iter_mut() {
            let source = match self {
                SharedQuota::Off => continue,
                SharedQuota::Unreachable(_) => {
                    account.windows.clear();
                    account.usage_observed_at = None;
                    "hub_unreachable"
                }
                SharedQuota::Read(quotas) => {
                    // Codex has no local email source: it matches on the account id from its own
                    // newest rollout. Providers with a login email match on that.
                    let email = account.email.as_deref().map(|email| email.trim().to_lowercase());
                    let account_id = account.identity.as_ref().and_then(|identity| identity.account_id.as_deref());
                    let matches = |quota: &&HubQuota| quota.provider == account.provider.name() && match account.provider {
                        crate::accounts::Provider::Codex => account_id.is_some() && quota.account_id.as_deref() == account_id,
                        _ => Some(&quota.email) == email.as_ref(),
                    };
                    match quotas.iter().find(matches) {
                        Some(quota) => {
                            account.windows = quota.windows.clone();
                            account.usage_observed_at = Some(quota.observed_at);
                            "hub"
                        }
                        None => "local",
                    }
                }
            };
            sources.insert(account.folder.clone(), source);
        }
        sources
    }
}

/// Auth-file `quota.signals` → usage windows, per provider's header family,
/// replaced by the hub's inspection read of the same account when that is newer.
fn quotas(files: &serde_json::Value, inspection: Option<&serde_json::Value>) -> Vec<HubQuota> {
    let inspected = inspection.map(inspection_windows).unwrap_or_default();
    let mut out = Vec::new();
    for file in files["files"].as_array().into_iter().flatten() {
        let (Some(provider), Some(email)) = (file["provider"].as_str(), file["email"].as_str()) else { continue };
        let quota = &file["quota"];
        let traffic_at = quota["observed_at"].as_str().and_then(|at| DateTime::parse_from_rfc3339(at).ok()).map(|at| at.with_timezone(&Utc));
        let signal = |key: &str| -> Option<f64> {
            let value = &quota["signals"][key];
            value.as_f64().or_else(|| value.as_str()?.trim().parse().ok())
        };
        let at = |key: &str| signal(key).and_then(|secs| DateTime::from_timestamp(secs as i64, 0));
        let windows: Vec<UsageWindow> = match provider {
            "claude" => [("5h", "5h"), ("7d", "weekly")]
                .into_iter()
                .filter_map(|(header, name)| {
                    let used = signal(&format!("Anthropic-Ratelimit-Unified-{header}-Utilization"))?;
                    Some(UsageWindow { name: name.into(), used_percent: Some(used * 100.0), resets_at: at(&format!("Anthropic-Ratelimit-Unified-{header}-Reset")) })
                })
                .collect(),
            "codex" => ["Primary", "Secondary"]
                .into_iter()
                .filter_map(|which| {
                    let name = match signal(&format!("X-Codex-{which}-Window-Minutes"))? as i64 {
                        300 => "5h",
                        10080 => "weekly",
                        _ => return None,
                    };
                    Some(UsageWindow { name: name.into(), used_percent: signal(&format!("X-Codex-{which}-Used-Percent")), resets_at: at(&format!("X-Codex-{which}-Reset-At")) })
                })
                .collect(),
            _ => continue,
        };
        let traffic = traffic_at.filter(|_| !windows.is_empty()).map(|at| (windows, at));
        let inspection = file["auth_index"].as_str().and_then(|index| inspected.get(index)).cloned();
        let newest = match (traffic, inspection) {
            (Some(traffic), Some(inspection)) => Some(if inspection.1 > traffic.1 { inspection } else { traffic }),
            (traffic, inspection) => traffic.or(inspection),
        };
        let Some((windows, observed_at)) = newest else { continue };
        let account_id = file["id_token"]["chatgpt_account_id"].as_str().map(str::trim).filter(|id| !id.is_empty()).map(str::to_string);
        out.push(HubQuota { provider: provider.into(), email: email.trim().to_lowercase(), account_id, windows, observed_at });
    }
    out
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

    /// Two machines (two homes) each hold the same Claude and Codex logins;
    /// both build the real Accounts catalog (`GET /v1/accounts`) from one hub
    /// answer and show one number per login. Codex is matched through its
    /// newest rollout's account id, so an older rollout as another account
    /// never decides. An unreachable hub shows none. A dashboard-only machine
    /// can't route.
    #[test]
    fn two_machines_reading_one_hub_show_one_number_for_one_login() {
        let hub = SharedQuota::Read(quotas(&serde_json::json!({"files": [
            {"provider": "claude", "email": "Daniel@Example.com", "quota": {"observed_at": "2026-11-23T18:44:57-05:00", "signals": {
                "Anthropic-Ratelimit-Unified-7d-Utilization": "0.2", "Anthropic-Ratelimit-Unified-7d-Reset": "1795986000"}}},
            {"provider": "codex", "email": "daniel@example.com", "id_token": {"chatgpt_account_id": "acct-hub"}, "quota": {"observed_at": "2026-11-23T18:12:55-05:00", "signals": {
                "X-Codex-Primary-Window-Minutes": "10080", "X-Codex-Primary-Used-Percent": "86", "X-Codex-Primary-Reset-At": "1795702350",
                "X-Codex-Secondary-Window-Minutes": "0", "X-Codex-Secondary-Used-Percent": "0"}}},
        ]}), None));
        // Codex rollouts, oldest first: only the newest one's session_meta names the login.
        let machine = |codex_rollouts: &[&str]| {
            let home = tempfile::tempdir().unwrap();
            let dir = home.path().join(".claude-daniel");
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(dir.join(".claude.json"), serde_json::json!({"oauthAccount": {"emailAddress": "daniel@example.com", "accountUuid": "u1"}}).to_string()).unwrap();
            std::fs::write(dir.join(crate::accounts::Provider::Claude.credentials()), "{}").unwrap();
            let sessions = home.path().join(".codex-daniel/sessions/2026/11/23");
            std::fs::create_dir_all(&sessions).unwrap();
            let start = std::time::SystemTime::now() - Duration::from_secs(3600);
            for (age, account) in codex_rollouts.iter().enumerate() {
                let path = sessions.join(format!("rollout-{age}.jsonl"));
                std::fs::write(&path, format!("{}\n", serde_json::json!({"type": "session_meta", "payload": {"creator_account_id": account}}))).unwrap();
                std::fs::File::options().write(true).open(&path).unwrap().set_modified(start + Duration::from_secs(60 * age as u64)).unwrap();
            }
            home
        };
        let row = |home: &tempfile::TempDir, quota: &SharedQuota, key: &str| {
            let catalog = serde_json::to_value(crate::hub::accounts::catalog_with(home.path(), quota).unwrap()).unwrap();
            let row = catalog["accounts"].as_array().unwrap().iter().find(|row| row["key"] == key).unwrap().clone();
            (catalog["quota_source"]["state"].clone(), row)
        };
        // soundwave last ran Codex as the hub's account; prowl's newest rollout is another account.
        let (soundwave, prowl) = (machine(&["acct-old", "acct-hub"]), machine(&["acct-hub"]));
        let switched = machine(&["acct-hub", "acct-other"]);
        let (state, on_soundwave) = row(&soundwave, &hub, "claude:daniel");
        let (_, on_prowl) = row(&prowl, &hub, "claude:daniel");
        assert_eq!(state, "hub");
        assert_eq!(on_soundwave["usage_source"], "hub");
        assert_eq!(on_soundwave["windows"], on_prowl["windows"]);
        assert_eq!(on_soundwave["usage_observed_at"], on_prowl["usage_observed_at"]);
        assert_eq!(on_soundwave["windows"][0]["used_percent"].as_f64().map(f64::round), Some(20.0));

        // Codex: matched by the newest rollout's account id, one number on both machines.
        let (_, codex_soundwave) = row(&soundwave, &hub, "codex:daniel");
        let (_, codex_prowl) = row(&prowl, &hub, "codex:daniel");
        assert_eq!(codex_soundwave["usage_source"], "hub");
        assert_eq!(codex_soundwave["windows"], codex_prowl["windows"]);
        assert_eq!(codex_soundwave["windows"][0]["used_percent"].as_f64().map(f64::round), Some(86.0));
        // A newer rollout as another account wins over an older matching one: no hub number.
        let (_, codex_switched) = row(&switched, &hub, "codex:daniel");
        assert_eq!(codex_switched["usage_source"], "local");

        // An unreachable hub shows no number rather than the local one.
        let (state, offline) = row(&prowl, &SharedQuota::Unreachable("dashboard unreachable".into()), "claude:daniel");
        assert_eq!(state, "unreachable");
        assert_eq!(offline["usage_source"], "hub_unreachable");
        assert!(offline["windows"].as_array().unwrap().is_empty() && offline["usage_observed_at"].is_null());

        // Reading quota needs only the dashboard keys; without an endpoint the machine can't route.
        let dashboard_only = cas_types::factory_gateway::FactoryGatewayConfig {
            dashboard: Some("http://100.64.0.1:18317".into()),
            dashboard_key_file: Some("~/.config/hub/admin.key".into()),
            ..Default::default()
        };
        assert!(cas_mux::gateway::readiness(&dashboard_only).is_err());
    }

    /// prowl's approved setup: dashboard keys, no routing endpoint. The real
    /// `/v1/accounts/gateway` body reports routing off and still lists the
    /// hub's accounts from the dashboard.
    #[test]
    fn a_dashboard_only_machine_lists_hub_accounts_with_routing_off() {
        use std::io::{Read, Write};
        use std::os::unix::fs::PermissionsExt;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dashboard = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut request = [0u8; 2048];
                let read = stream.read(&mut request).unwrap_or(0);
                let authorized = String::from_utf8_lossy(&request[..read]).contains("Bearer admin-key");
                let body = serde_json::json!({"files": [{"provider": "claude", "email": "hello@example.com", "recent_requests": []}]}).to_string();
                let status = if authorized { "200 OK" } else { "401 Unauthorized" };
                let _ = stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            }
        });
        let temp = tempfile::tempdir().unwrap();
        let key = temp.path().join("admin.key");
        std::fs::write(&key, "admin-key\n").unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        let settings = cas_types::factory_gateway::FactoryGatewayConfig {
            dashboard: Some(dashboard),
            dashboard_key_file: Some(key.display().to_string()),
            ..Default::default()
        };
        let inventory = inventory_with(&settings);
        assert_eq!((inventory.status, inventory.inventory), ("off", "ok"));
        assert_eq!(inventory.accounts.len(), 1);
        assert!(cas_mux::gateway::readiness(&settings).is_err(), "no endpoint: this machine's workers stay direct");
    }

    /// cas-966b: with no Codex traffic for 9 h, the hub's own inspection read
    /// (its timer, zero-cost usage endpoint) refreshes the login through the
    /// real quota source; Cassy only GETs the runs and never starts one.
    #[test]
    fn a_newer_inspection_run_refreshes_a_login_with_no_traffic() {
        use std::io::{Read, Write};
        use std::os::unix::fs::PermissionsExt;
        let now = Utc::now();
        let stale = (now - chrono::Duration::hours(9)).to_rfc3339();
        let fresh_ms = (now - chrono::Duration::minutes(4)).timestamp_millis();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dashboard = format!("http://{}", listener.local_addr().unwrap());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut request = [0u8; 2048];
                let read = stream.read(&mut request).unwrap_or(0);
                let line = String::from_utf8_lossy(&request[..read]).lines().next().unwrap_or_default().to_string();
                log.lock().unwrap().push(line.clone());
                let body = if line.starts_with("GET /v0/management/auth-files ") {
                    serde_json::json!({"files": [{"provider": "codex", "email": "pip@example.com", "auth_index": "fb45", "id_token": {"chatgpt_account_id": "acct-1"},
                        "quota": {"observed_at": stale, "signals": {"X-Codex-Primary-Window-Minutes": "10080", "X-Codex-Primary-Used-Percent": "88"}}}]})
                } else if line.starts_with("GET /v0/management/codex-inspection/runs?limit=5 ") {
                    serde_json::json!({"items": [{"id": 7, "status": "running"}, {"id": 6, "status": "completed"}]})
                } else if line.starts_with("GET /v0/management/codex-inspection/runs/6 ") {
                    serde_json::json!({"finishedAtMs": fresh_ms, "results": [{"authIndex": "fb45", "quotaWindows": [
                        {"limitWindowSeconds": 604800, "usedPercent": 90.0, "resetAtMs": 1795702350000i64},
                        {"limitWindowSeconds": 18000, "usedPercent": 12.0}]}]})
                } else {
                    serde_json::json!({"error": "unexpected"})
                }.to_string();
                let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            }
        });
        let temp = tempfile::tempdir().unwrap();
        let key = temp.path().join("admin.key");
        std::fs::write(&key, "admin-key\n").unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        let settings = cas_types::factory_gateway::FactoryGatewayConfig {
            dashboard: Some(dashboard.clone()),
            dashboard_key_file: Some(key.display().to_string()),
            ..Default::default()
        };
        let SharedQuota::Read(quotas) = shared_quota_with(&settings) else { panic!("hub not read") };
        let login = &quotas[0];
        assert_eq!(login.observed_at.timestamp_millis(), fresh_ms, "the newer inspection read wins over 9 h old traffic");
        let window = |name: &str| login.windows.iter().find(|window| window.name == name).and_then(|window| window.used_percent);
        assert_eq!((window("weekly"), window("5h")), (Some(90.0), Some(12.0)));
        assert!(seen.lock().unwrap().iter().all(|line| line.starts_with("GET ")), "{:?}", seen.lock().unwrap());
        // Starting a run is the hub's timer's job: the client refuses that path outright.
        let client = Dashboard { endpoint: dashboard, key: "k".into() };
        for path in ["/v0/management/codex-inspection/run", "/v0/management/codex-inspection/runs/6/cancel"] {
            assert_eq!(client.get(path).unwrap_err(), DashboardError::NotReadOnly, "{path}");
        }
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
