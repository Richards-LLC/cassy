//! cas-ef94 (GH #1267): the running daemon's own Violet consumer receipt.
//!
//! Each factory daemon writes one receipt for its session when push-wake
//! starts or is skipped. The receipt records the v1 claim consumer id and the
//! capabilities compiled into that process. `cas factory status` and
//! `cas doctor` trust a receipt only when its PID and start time match the
//! session's live daemon. Without a matching receipt, a running daemon is
//! reported as unknown: an old daemon writes none, and writing can fail. The CLI on PATH never
//! stands in for the daemon's build or capability.
//!
//! This is local evidence. It is not a Cloud `capable_v1_consumers`
//! attestation and grants no consumer exemption.
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ui::factory::protocol::SessionMetadata;

const RECEIPT_VERSION: u32 = 1;
/// Managed bindings are read through the v2 reader with legacy claims off.
pub(crate) const MANAGED_READER_V2: &str = "managed_reader_v2";
pub(crate) const AUTHORITY: &str = "local_evidence_not_cloud_attestation";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ConsumerReceipt {
    pub format_version: u32,
    pub factory_session: String,
    pub pid: u32,
    pub pid_starttime: Option<u64>,
    pub build: String,
    pub started_at: DateTime<Utc>,
    /// `slack.wake_enabled` as read at start; the daemon never re-reads it.
    pub wake_enabled_at_start: bool,
    /// The v1 claim consumer id, when push-wake started.
    pub consumer_id: Option<String>,
    pub project_id: Option<String>,
    /// Why push-wake did not start, when it did not.
    pub not_started: Option<String>,
    pub capabilities: Vec<String>,
}

fn receipt_path(cas_dir: &Path, factory_session: &str) -> Option<PathBuf> {
    let valid = !factory_session.is_empty()
        && factory_session != "."
        && factory_session != ".."
        && !factory_session.contains(['/', '\\']);
    valid.then(|| {
        cas_dir
            .join("violet")
            .join("consumers")
            .join(format!("{factory_session}.json"))
    })
}

fn compiled_capabilities() -> Vec<String> {
    let mut capabilities = vec![MANAGED_READER_V2.to_string()];
    capabilities.extend(
        crate::ui::factory::protocol::local_gui_capabilities()
            .into_iter()
            .filter(|capability| {
                *capability
                    == crate::ui::factory::protocol::ProtocolCapability::BindingIntakeControlV1
            })
            .filter_map(|capability| serde_json::to_value(capability).ok())
            .filter_map(|value| value.as_str().map(str::to_owned)),
    );
    capabilities
}

// Keep the lock inode: deleting it could give simultaneous callers different
// locks for the same session. Publication and retirement use this same lock.
fn lock_receipt(path: &Path) -> Result<std::fs::File, String> {
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))
        .map_err(|e| format!("open consumer receipt lock: {e}"))?;
    fs2::FileExt::lock_exclusive(&lock).map_err(|e| format!("lock consumer receipt: {e}"))?;
    Ok(lock)
}

fn failure_path(path: &Path, pid: u32) -> PathBuf {
    path.with_extension(format!("json.{pid}.failed"))
}

/// Write this process's receipt for `factory_session`, replacing any older one.
pub(crate) fn record(
    cas_dir: &Path,
    factory_session: &str,
    wake_enabled_at_start: bool,
    consumer_id: Option<&str>,
    project_id: Option<&str>,
    not_started: Option<&str>,
) -> Result<(), String> {
    use std::io::Write;
    let path = receipt_path(cas_dir, factory_session).ok_or("invalid factory session name")?;
    let pid = std::process::id();
    let receipt = ConsumerReceipt {
        format_version: RECEIPT_VERSION,
        factory_session: factory_session.to_string(),
        pid,
        pid_starttime: crate::mcp::daemon::read_pid_starttime(pid),
        build: option_env!("CAS_GIT_HASH").unwrap_or("unknown").to_string(),
        started_at: Utc::now(),
        wake_enabled_at_start,
        consumer_id: consumer_id.map(str::to_owned),
        project_id: project_id.map(str::to_owned),
        not_started: not_started.map(str::to_owned),
        capabilities: compiled_capabilities(),
    };
    let parent = path.parent().ok_or("receipt path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    let _lock = lock_receipt(&path)?;
    let tmp = path.with_extension(format!("json.{pid}.tmp"));
    let bytes = serde_json::to_vec_pretty(&receipt).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("rename {}: {e}", path.display()))
    })();
    let failure = failure_path(&path, pid);
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
        // Best effort: a completely unwritable directory may prevent even this
        // evidence. Only this PID + start time can use it as a known failure.
        let _ = std::fs::write(failure, bytes);
    } else {
        let _ = std::fs::remove_file(failure);
    }
    result
}

/// Remove this process's receipt at clean shutdown. Another process's
/// receipt for the same session name is left alone.
pub(crate) fn retire(cas_dir: &Path, factory_session: &str) {
    retire_after_read(cas_dir, factory_session, || {});
}

// Deterministic boundary for testing concurrent publication, without sleeps.
fn retire_after_read(cas_dir: &Path, factory_session: &str, after_read: impl FnOnce()) {
    let Some(path) = receipt_path(cas_dir, factory_session) else {
        return;
    };
    let Ok(_lock) = lock_receipt(&path) else {
        return;
    };
    if read(&path).is_some_and(|receipt| receipt.pid == std::process::id()) {
        after_read();
        let _ = std::fs::remove_file(&path);
    }
    let _ = std::fs::remove_file(failure_path(&path, std::process::id()));
}

fn read(path: &Path) -> Option<ConsumerReceipt> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConsumerState {
    /// The live daemon's receipt: push-wake claims with `consumer_id`.
    Running,
    /// The live daemon's receipt: push-wake did not start.
    PushWakeOff,
    /// The live daemon wrote no matching receipt (older build or write failure).
    Unknown,
    /// The daemon is gone; a receipt from it remains.
    Stale,
}

/// What `cas factory status` and `cas doctor` report for one session.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ConsumerStatus {
    pub factory_session: String,
    pub state: ConsumerState,
    pub consumer_id: Option<String>,
    pub capabilities: Vec<String>,
    /// The build the daemon itself recorded, never the CLI on PATH.
    pub daemon_build: Option<String>,
    pub wake_enabled_at_start: Option<bool>,
    pub wake_enabled_now: bool,
    pub detail: String,
    pub authority: &'static str,
}

/// Classify `session`'s consumer from its receipt and the live daemon identity.
/// `None` when the daemon is not running and left no receipt.
pub(crate) fn observe(
    cas_dir: &Path,
    session: &SessionMetadata,
    daemon_running: bool,
) -> Option<ConsumerStatus> {
    let receipt = receipt_path(cas_dir, &session.name).and_then(|path| read(&path));
    let wake_enabled_now = crate::config::Config::load(cas_dir)
        .unwrap_or_default()
        .slack
        .unwrap_or_default()
        .wake_enabled();
    let matches_daemon = |receipt: &&ConsumerReceipt| {
        daemon_running
            && receipt.pid == session.daemon_pid
            && receipt.pid_starttime.is_some()
            && crate::mcp::daemon::read_pid_starttime(receipt.pid) == receipt.pid_starttime
            && session
                .daemon_pid_starttime
                .is_none_or(|expected| Some(expected) == receipt.pid_starttime)
    };
    let verified = receipt.as_ref().filter(matches_daemon);
    let write_failed = receipt_path(cas_dir, &session.name)
        .and_then(|path| read(&failure_path(&path, session.daemon_pid)))
        .as_ref()
        .filter(matches_daemon)
        .is_some();
    let mut status = ConsumerStatus {
        factory_session: session.name.clone(),
        state: ConsumerState::Unknown,
        consumer_id: None,
        capabilities: Vec::new(),
        daemon_build: session.daemon_build_hash.clone(),
        wake_enabled_at_start: None,
        wake_enabled_now,
        detail: String::new(),
        authority: AUTHORITY,
    };
    match (verified, receipt.as_ref()) {
        (Some(receipt), _) => {
            status.consumer_id = receipt.consumer_id.clone();
            status.capabilities = receipt.capabilities.clone();
            status.daemon_build = Some(receipt.build.clone());
            status.wake_enabled_at_start = Some(receipt.wake_enabled_at_start);
            status.state = if receipt.consumer_id.is_some() {
                ConsumerState::Running
            } else {
                ConsumerState::PushWakeOff
            };
            status.detail = match &receipt.not_started {
                None => format!(
                    "v1 consumer {}",
                    receipt.consumer_id.as_deref().unwrap_or("?")
                ),
                Some(reason) => format!("push-wake not started ({reason})"),
            };
            if receipt.wake_enabled_at_start != wake_enabled_now {
                status.detail.push_str(&format!(
                    "; slack.wake_enabled is now {wake_enabled_now}, but the daemon reads it only at start: restart the factory to apply"
                ));
            }
        }
        (None, Some(receipt)) if !daemon_running => {
            status.state = ConsumerState::Stale;
            status.daemon_build = Some(receipt.build.clone());
            status.detail = format!(
                "daemon pid {} is not running; its receipt is stale",
                receipt.pid
            );
        }
        (None, None) if !daemon_running => return None,
        (None, stale) => {
            let ignored = stale
                .map(|receipt| format!(" (ignored a receipt from pid {})", receipt.pid))
                .unwrap_or_default();
            let reason = if write_failed {
                "consumer receipt write failed; see the daemon log"
            } else {
                "reporting unavailable (older build or receipt write failure); see the daemon log"
            };
            status.detail = format!(
                "daemon pid {} (build {}) wrote no consumer receipt{ignored}: {reason}; restart the factory to report it",
                session.daemon_pid,
                session.daemon_build_hash.as_deref().unwrap_or("unrecorded"),
            );
        }
    }
    Some(status)
}

impl ConsumerStatus {
    /// Status-screen rows: state first, then evidence.
    pub(crate) fn rows(&self) -> Vec<String> {
        let state = match self.state {
            ConsumerState::Running => "running",
            ConsumerState::PushWakeOff => "push-wake off",
            ConsumerState::Unknown => "unknown",
            ConsumerState::Stale => "stale",
        };
        let mut rows = vec![
            format!("{state} · local evidence, not a Cloud attestation"),
            self.detail.clone(),
        ];
        if !self.capabilities.is_empty() {
            rows.push(format!("capabilities: {}", self.capabilities.join(", ")));
        }
        if let Some(build) = self
            .daemon_build
            .as_deref()
            .filter(|_| self.state != ConsumerState::Unknown)
        {
            rows.push(format!("daemon build {build}"));
        }
        rows
    }
}

/// Running factory sessions whose daemon uses `cas_dir`, for `cas doctor`.
pub(crate) fn project_statuses(cas_dir: &Path) -> Vec<ConsumerStatus> {
    let Ok(sessions) = crate::ui::factory::session::SessionManager::new().list_sessions_read_only()
    else {
        return Vec::new();
    };
    let ours = cas_dir.canonicalize().ok();
    sessions
        .into_iter()
        .filter(|session| session.is_running)
        .filter(|session| {
            let root = session
                .metadata
                .cas_root
                .as_ref()
                .map(PathBuf::from)
                .or_else(|| {
                    session
                        .metadata
                        .project_dir
                        .as_ref()
                        .map(|dir| Path::new(dir).join(".cas"))
                });
            ours.is_some() && root.and_then(|root| root.canonicalize().ok()) == ours
        })
        .filter_map(|session| observe(cas_dir, &session.metadata, true))
        .collect()
}

#[cfg(test)]
#[path = "violet_consumer_tests.rs"]
mod tests;
