//! A supervisor session parked on standby (cas-94a0).
//!
//! `factory action=standby` stops every worker of the session, then records
//! this marker next to the daemon's other session files
//! (`.cas/factory-standby/<session>.json`). While it exists the daemon wakes
//! the supervisor pane only for the operator and CAS lifecycle events; every
//! other row stays in the inbox. `status=resume` respawns the workers that held
//! tasks and removes the marker. The marker is the whole state: standby adds no
//! second lifecycle beside the existing worker queues and handoff.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A worker that held tasks when the session went on standby.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldWorker {
    pub name: String,
    pub tasks: Vec<String>,
    /// The harness the stopped worker ran, so its replacement runs the same one.
    #[serde(default)]
    pub cli: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standby {
    pub session: String,
    pub since: DateTime<Utc>,
    /// The supervisor's current handoff when the session went on standby.
    #[serde(default)]
    pub handoff: Option<String>,
    pub workers: Vec<HeldWorker>,
}

fn path(cas_root: &Path, session: &str) -> PathBuf {
    let safe: String = session
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    cas_root.join("factory-standby").join(format!("{safe}.json"))
}

pub fn enter(cas_root: &Path, standby: &Standby) -> std::io::Result<()> {
    let path = path(cas_root, &standby.session);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::to_vec_pretty(standby).map_err(std::io::Error::other)?;
    std::fs::write(path, body)
}

/// The recorded standby, if this session is on standby.
pub fn read(cas_root: &Path, session: &str) -> Option<Standby> {
    serde_json::from_slice(&std::fs::read(path(cas_root, session)).ok()?).ok()
}

pub fn active(cas_root: &Path, session: &str) -> bool {
    path(cas_root, session).is_file()
}

/// Whether any standby marker lists `worker` as parked with tasks.
pub fn holds(cas_root: &Path, worker: &str) -> bool {
    let Ok(markers) = std::fs::read_dir(cas_root.join("factory-standby")) else { return false };
    markers.flatten().any(|marker| {
        std::fs::read(marker.path()).ok()
            .and_then(|bytes| serde_json::from_slice::<Standby>(&bytes).ok())
            .is_some_and(|standby| standby.workers.iter().any(|held| held.name == worker))
    })
}

pub fn clear(cas_root: &Path, session: &str) -> std::io::Result<()> {
    match std::fs::remove_file(path(cas_root, session)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The supervisor's current handoff, the one resume tells it to reload.
pub fn current_handoff_id(cas_root: &Path) -> Option<String> {
    let entries = crate::store::open_store(cas_root).ok()?.list().ok()?;
    use cas_core::memory::handoff::{HandoffLineage, current_handoff};
    current_handoff(&entries, "supervisor", Some(&HandoffLineage::for_local_machine())).map(|entry| entry.id.clone())
}

/// The supervisor's first operator message after standby: bring the workers back
/// and name the handoff to reload. Runs the same resume as `status=resume`, on its
/// own runtime because hooks are synchronous. `None` when the session is not on
/// standby or the resume failed (the marker then stays, so `status=resume` still works).
pub fn resume_for_operator_message(cas_root: &Path, session: &str) -> Option<String> {
    if !active(cas_root, session) {
        return None;
    }
    let (root, session) = (cas_root.to_path_buf(), session.to_string());
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
        runtime.block_on(async {
            let core = crate::mcp::CasCore::with_daemon(root, None, None);
            crate::mcp::CasService::new(core, None).standby_resume_from_env(&session).await.ok()
        })
    })
    .join()
    .ok()
    .flatten()
}
