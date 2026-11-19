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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standby {
    pub session: String,
    pub since: DateTime<Utc>,
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

pub fn clear(cas_root: &Path, session: &str) -> std::io::Result<()> {
    match std::fs::remove_file(path(cas_root, session)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}
