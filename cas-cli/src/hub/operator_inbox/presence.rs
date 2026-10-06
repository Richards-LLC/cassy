//! Opted-in machine lease reporter (cloud contract §17).
//!
//! The server owns deadlines and outage transitions. This process owns one
//! activation identity and one unanswered body: retries never resample it or
//! reuse its sequence for another observation. A fenced or disabled process
//! cannot acquire another epoch. No viewer, event drain or hub credential is
//! involved in this lease.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::machine::{HttpClient, MachinePrincipal, MachineTransport, PrincipalStore, UreqHttp};

const ACTIVATE: &str = "/api/operator/machine/presence/activate";
const HEARTBEAT: &str = "/api/operator/machine/presence";
const EPOCH_FILE: &str = "presence-epoch.json";
const MAX_POSITION: u64 = 9_223_372_036_854_775_807;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ComponentName {
    Hub,
    Serve,
    Factory,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ComponentState {
    Up,
    Degraded,
    Down,
    Unknown,
}

/// Closed fields only. Observations never contain addresses, errors or labels.
#[derive(Clone, Serialize)]
pub(crate) struct Component {
    component: ComponentName,
    state: ComponentState,
    age_s: u16,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SilenceKind {
    Sleep,
    Reboot,
    Maintenance,
}

#[derive(Clone, Serialize)]
pub(crate) struct Silence {
    kind: SilenceKind,
    duration_s: u32,
}

impl Silence {
    pub(crate) fn new(kind: SilenceKind, duration_s: u32) -> Option<Self> {
        (1..=86_400)
            .contains(&duration_s)
            .then_some(Self { kind, duration_s })
    }
}

struct Prepared {
    path: &'static str,
    body: Vec<u8>,
}

enum Phase {
    Activate,
    Reporting { epoch: String, generation: String },
    Stopped(&'static str),
}

struct Reporter {
    machine_id: String,
    boot_id: String,
    instance_id: String,
    previous_epoch: String,
    phase: Phase,
    activation_conflict_retried: bool,
    seq: u64,
    pending: Option<Prepared>,
}

fn position(value: &Value) -> Option<String> {
    let s = value.as_str()?;
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    s.parse::<u64>()
        .ok()
        .filter(|n| *n <= MAX_POSITION)
        .map(|_| s.to_owned())
}

impl Reporter {
    fn new(
        machine_id: String,
        boot_id: String,
        instance_id: String,
        previous_epoch: String,
    ) -> Self {
        Self {
            machine_id,
            boot_id,
            instance_id,
            previous_epoch,
            phase: Phase::Activate,
            activation_conflict_retried: false,
            seq: 0,
            pending: None,
        }
    }

    fn stop(&mut self, reason: &'static str) {
        self.pending = None;
        self.phase = Phase::Stopped(reason);
    }

    fn prepare(
        &mut self,
        components: Vec<Component>,
        silence: Option<Silence>,
    ) -> Option<&Prepared> {
        if self.pending.is_none() {
            let value = match &self.phase {
                Phase::Stopped(_) => return None,
                Phase::Activate => json!({
                    "wire_version": 1, "boot_id": self.boot_id, "instance_id": self.instance_id,
                    "previous_reporter_epoch": self.previous_epoch,
                }),
                Phase::Reporting { epoch, generation } => {
                    if self.seq == MAX_POSITION {
                        self.stop("sequence_exhausted");
                        return None;
                    }
                    self.seq += 1;
                    let mut report = json!({
                        "wire_version": 1, "reporter_epoch": epoch,
                        "monitoring_generation": generation, "seq": self.seq.to_string(),
                        "components": components,
                    });
                    if let Some(silence) = silence {
                        report["silence"] = json!(silence);
                    }
                    report
                }
            };
            self.pending = Some(Prepared {
                path: if matches!(self.phase, Phase::Activate) {
                    ACTIVATE
                } else {
                    HEARTBEAT
                },
                body: serde_json::to_vec(&value).expect("closed presence fields serialize"),
            });
        }
        self.pending.as_ref()
    }

    /// Successful activation returns the epoch that must be durably saved
    /// before the caller sends a heartbeat. Transient HTTP failures retain the
    /// exact pending bytes; a fresh PoP proof still binds those bytes.
    fn receive(&mut self, status: u16, body: &[u8]) -> Option<String> {
        let Some(pending) = &self.pending else {
            return None;
        };
        if status == 429 || status >= 500 {
            return None;
        }
        let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let activation = pending.path == ACTIVATE;
        if status == 200 {
            if value["wire_version"] != 1
                || (activation && value["machine_id"].as_str() != Some(self.machine_id.as_str()))
            {
                self.stop("invalid_receipt");
                return None;
            }
            if activation {
                let epoch = position(&value["reporter_epoch"]).filter(|v| v != "0");
                let generation = position(&value["monitoring_generation"]).filter(|v| v != "0");
                let policy = value["heartbeat_interval_s"] == 60
                    && value["heartbeat_jitter_s"] == 10
                    && value["lease_s"] == 180
                    && value["grace_s"] == 60
                    && value["max_silence_s"] == 86_400;
                if let (Some(epoch), Some(generation), true) = (epoch, generation, policy)
                    && matches!(value["outcome"].as_str(), Some("activated" | "existing"))
                {
                    if epoch != self.previous_epoch {
                        self.seq = 0;
                    }
                    self.previous_epoch = epoch.clone();
                    self.phase = Phase::Reporting {
                        epoch: epoch.clone(),
                        generation,
                    };
                    self.pending = None;
                    return Some(epoch);
                }
            } else if let Phase::Reporting { epoch, .. } = &self.phase {
                let receipt = &value["receipt"];
                if receipt["reporter_epoch"].as_str() == Some(epoch.as_str())
                    && receipt["seq"].as_str() == Some(self.seq.to_string().as_str())
                    && matches!(receipt["outcome"].as_str(), Some("accepted" | "duplicate"))
                {
                    self.pending = None;
                    return None;
                }
            }
            self.stop("invalid_receipt");
            return None;
        }
        match (status, value["error"].as_str()) {
            (409, Some("reporter_epoch_conflict"))
                if activation && !self.activation_conflict_retried =>
            {
                if let Some(epoch) = position(&value["current_reporter_epoch"]) {
                    self.previous_epoch = epoch;
                    self.activation_conflict_retried = true;
                    self.pending = None;
                } else {
                    self.stop("invalid_conflict");
                }
            }
            (409, Some("monitoring_generation_conflict")) if !activation => {
                self.phase = Phase::Activate;
                self.activation_conflict_retried = false;
                self.pending = None;
            }
            (409, Some("presence_report_stale")) if !activation => {
                // The old seq was not consumed. Advancing still guarantees a
                // fresh body and PoP iat rather than replaying stale recovery.
                self.pending = None;
            }
            (409, Some("monitoring_disabled")) => self.stop("monitoring_disabled"),
            (409, Some("reporter_epoch_conflict")) => self.stop("reporter_fenced"),
            (401 | 403, _) => self.stop("grant_refused"),
            _ => self.stop("presence_refused"),
        }
        None
    }
}

#[derive(Serialize, Deserialize)]
struct SavedEpoch {
    wire_version: u8,
    cloud_origin: String,
    account_id: String,
    machine_id: String,
    reporter_epoch: String,
}

fn load_epoch(root: &Path, principal: &MachinePrincipal) -> anyhow::Result<String> {
    let path = root.join("operator-inbox").join(EPOCH_FILE);
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok("0".into()),
        Err(e) => return Err(e.into()),
    };
    anyhow::ensure!(meta.is_file(), "presence epoch is not a regular file");
    let saved: SavedEpoch = serde_json::from_slice(&fs::read(path)?)?;
    anyhow::ensure!(saved.wire_version == 1, "unsupported presence epoch");
    let epoch = position(&json!(saved.reporter_epoch))
        .ok_or_else(|| anyhow::anyhow!("invalid presence epoch"))?;
    if saved.cloud_origin != principal.cloud_origin
        || saved.account_id != principal.account_id
        || saved.machine_id != principal.machine_id
    {
        return Ok("0".into());
    }
    Ok(epoch)
}

fn save_epoch(root: &Path, principal: &MachinePrincipal, epoch: String) -> anyhow::Result<()> {
    let dir = root.join("operator-inbox");
    crate::hub::ensure_private_dir(&dir)?;
    let temp = dir.join(format!(".{EPOCH_FILE}.{}", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> anyhow::Result<()> {
        let mut file = options.open(&temp)?;
        let saved = SavedEpoch {
            wire_version: 1,
            cloud_origin: principal.cloud_origin.clone(),
            account_id: principal.account_id.clone(),
            machine_id: principal.machine_id.clone(),
            reporter_epoch: epoch,
        };
        file.write_all(&serde_json::to_vec(&saved)?)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, dir.join(EPOCH_FILE))?;
        fs::File::open(&dir)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn sample(root: &Path) -> (Option<String>, Vec<Component>) {
    use crate::hub::observation::{Observation, ObservationState, collect_runtime_receipt};
    let paths = crate::hub::HubRuntimePaths::new(root);
    let sampled_at = std::time::Instant::now();
    let started = chrono::Utc::now();
    let record = paths
        .read_process_record()
        .ok()
        .filter(|record| record.pid == std::process::id());
    // Verify the current process's owned Serve route without changing it.
    // An external HTTPS reachability claim needs an independent observer and
    // is never inferred from this local control-plane probe.
    let manager = crate::hub::tailscale::TailscaleServeManager::new(root);
    let publication = match record.as_ref() {
        Some(record) if record.transport_warning.is_some() => Observation::new(
            ObservationState::Failed,
            "serve_publication_unavailable",
            "current_runtime_record",
        ),
        Some(record) => match manager.owned_receipt() {
            Ok(Some(owned))
                if record.tailscale_serve_target.as_deref()
                    == Some(owned.local_target.as_str()) =>
            {
                match manager.serve_handlers(owned.https_port) {
                    Ok(handlers)
                        if handlers
                            .iter()
                            .any(|(path, target)| path == "/" && target == &owned.local_target) =>
                    {
                        Observation::new(
                            ObservationState::Healthy,
                            "owned_route_present",
                            "current_serve_probe",
                        )
                    }
                    Ok(_) => Observation::new(
                        ObservationState::Failed,
                        "owned_route_absent",
                        "current_serve_probe",
                    ),
                    Err(_) => Observation::new(
                        ObservationState::Unknown,
                        "serve_probe_unavailable",
                        "current_serve_probe",
                    ),
                }
            }
            _ => Observation::new(
                ObservationState::Unknown,
                "no_owned_publication_observed",
                "current_serve_probe",
            ),
        },
        None => Observation::new(
            ObservationState::Unknown,
            "runtime_record_unavailable",
            "current_runtime_record",
        ),
    };
    let receipt = collect_runtime_receipt(
        &paths,
        record.as_ref(),
        Observation::new(ObservationState::Healthy, "reporter_running", "hub_process"),
        publication,
        started,
    );
    let age = sampled_at.elapsed().as_secs().saturating_add(1).min(300) as u16;
    let map = |name, state| Component {
        component: name,
        state: match state {
            ObservationState::Healthy => ComponentState::Up,
            ObservationState::Failed => ComponentState::Down,
            ObservationState::Unknown
            | ObservationState::Disabled
            | ObservationState::Unsupported => ComponentState::Unknown,
        },
        age_s: age,
    };
    (
        receipt.current_os_boot_id,
        vec![
            map(ComponentName::Hub, receipt.hub.state),
            map(ComponentName::Serve, receipt.serve_publication.state),
            map(ComponentName::Factory, receipt.factory.health.state),
        ],
    )
}

/// A single hub-owned loop. The synchronous signed transport has a bounded
/// per-request 10s HTTP deadline. Monotonic scheduling skips missed ticks and never
/// queues a backlog after suspend. No enrollment or implicit opt-in occurs.
pub fn spawn_presence_loop(root: PathBuf) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let store = PrincipalStore::new(&root);
        let http: Arc<dyn HttpClient> = Arc::new(UreqHttp::default());
        let instance = uuid::Uuid::new_v4().to_string();
        let mut active: Option<(MachinePrincipal, MachineTransport, Reporter)> = None;
        loop {
            let tick = tokio::time::Instant::now();
            let work_root = root.clone();
            let principal_store = store.clone();
            let loaded = tokio::task::spawn_blocking(move || {
                let principal = principal_store.load()?;
                Ok::<_, anyhow::Error>(principal.map(|principal| (principal, sample(&work_root))))
            })
            .await;
            let Ok(Ok(Some((principal, (boot, components))))) = loaded else {
                if active.is_some() {
                    tracing::info!("presence reporter stopped: enrollment unavailable");
                    return;
                }
                tokio::time::sleep(Duration::from_secs(60)).await;
                continue;
            };
            if !principal
                .capabilities
                .iter()
                .any(|v| v == "presence:report")
            {
                if active.is_some() {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(60)).await;
                continue;
            }
            if active.is_none() {
                let Ok(epoch) = load_epoch(&root, &principal) else {
                    tracing::warn!("presence reporter stopped: epoch storage unavailable");
                    return;
                };
                let Some(boot) = boot else {
                    tracing::warn!("presence reporter stopped: boot identity unavailable");
                    return;
                };
                let Ok(transport) = MachineTransport::new(
                    Arc::clone(&http),
                    principal.clone(),
                    Some(store.clone()),
                ) else {
                    return;
                };
                active = Some((
                    principal.clone(),
                    transport,
                    Reporter::new(principal.machine_id.clone(), boot, instance.clone(), epoch),
                ));
            }
            let (bound, transport, reporter) = active.as_mut().expect("active reporter");
            if bound.machine_id != principal.machine_id
                || bound.account_id != principal.account_id
                || bound.cloud_origin != principal.cloud_origin
            {
                tracing::warn!("presence reporter stopped: enrollment replaced");
                return;
            }
            let Some(prepared) = reporter.prepare(components, None) else {
                return;
            };
            let activation = prepared.path == ACTIVATE;
            let transport = transport.clone();
            let path = prepared.path;
            let body = prepared.body.clone();
            let response = tokio::task::spawn_blocking(move || {
                transport.exchange_blocking("POST", path, &body)
            })
            .await;
            match response {
                Ok(Ok(response)) => {
                    if let Some(epoch) = reporter.receive(response.status, &response.body)
                        && save_epoch(&root, bound, epoch).is_err()
                    {
                        tracing::warn!("presence reporter stopped: epoch could not be persisted");
                        return;
                    }
                }
                Ok(Err(super::Failure::Unavailable | super::Failure::Deadline)) => {}
                Ok(Err(super::Failure::Http {
                    status: 429 | 500..=599,
                    ..
                })) => {}
                Ok(Err(_)) => reporter.stop("transport_refused"),
                Err(_) => reporter.stop("reporter_worker_failed"),
            }
            if let Phase::Stopped(reason) = &reporter.phase {
                tracing::info!(reason, "presence reporter stopped");
                return;
            }
            if activation && matches!(reporter.phase, Phase::Reporting { .. }) {
                continue; // activation does not renew the lease; report now
            }
            let jitter = (uuid::Uuid::new_v4().as_u128() % 21) as u64;
            tokio::time::sleep_until(tick + Duration::from_secs(50 + jitter)).await;
        }
    })
}

#[cfg(test)]
#[path = "presence_tests.rs"]
mod tests;
