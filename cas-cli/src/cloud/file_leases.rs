//! cas-8dbe: cross-session file leases on Cassy Cloud claims.
//!
//! Workers in two factory sessions of one repository (often on two machines)
//! could not see that both were about to edit the same file: worker peer
//! messages and task execution state stay in one Cassy database. A worker's
//! edit now claims its repository-relative path through the same Cloud claim
//! model as tasks ([`PeerClaims`]), so a second session's worker is told who
//! holds the file before it edits too. The claim is advisory, like task
//! claims: an unreachable Cloud only means the peer check did not run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::peer_claims::{Acquire, ClaimKind, PeerClaims};

/// One edit's allowance for the Cloud round trip, so a slow Cloud costs an
/// edit at most this long.
const CLAIM_BUDGET: Duration = Duration::from_secs(2);
/// A path this agent already claimed is not re-asked inside this window.
const OWN_CLAIM_REUSE_SECS: i64 = 120;
/// After the Cloud could not be asked, edits skip the check for this long.
const UNAVAILABLE_BACKOFF_SECS: i64 = 60;

/// What a worker learns before it edits `path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileLease {
    /// This worker holds (or now holds) the file's lease.
    Mine,
    /// Another agent holds it: a live peer elsewhere, or another local session.
    HeldBy(String),
    /// No peer check ran (no Cloud login, Cloud unreachable, or backing off).
    Unchecked,
}

/// The Cloud claim id for a repository-relative path. The repository scope is
/// added by the claim key itself.
pub fn lease_id(relative_path: &str) -> String {
    let digest = Sha256::digest(relative_path.as_bytes());
    let hex: String = digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("file-{hex}")
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Ledger {
    #[serde(default)]
    leases: BTreeMap<String, Entry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    path: String,
    mine: bool,
    at_unix_secs: i64,
}

fn ledger_path(cas_root: &Path, agent_id: &str) -> PathBuf {
    let safe: String = agent_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    cas_root.join("file-leases").join(format!("{safe}.json"))
}

fn load(cas_root: &Path, agent_id: &str) -> Ledger {
    std::fs::read_to_string(ledger_path(cas_root, agent_id))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(cas_root: &Path, agent_id: &str, ledger: &Ledger) {
    let path = ledger_path(cas_root, agent_id);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(text) = serde_json::to_string(ledger) else {
        return;
    };
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    if std::fs::write(&temporary, text).is_ok() && std::fs::rename(&temporary, &path).is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
}

/// Claim `relative_path` for `agent` working `task_id` before it edits.
pub fn claim(
    cas_root: &Path,
    agent_id: &str,
    agent_name: &str,
    task_id: &str,
    relative_path: &str,
) -> FileLease {
    let Some(claims) = PeerClaims::for_project(cas_root) else {
        return FileLease::Unchecked;
    };
    claim_with(
        &claims,
        cas_root,
        agent_id,
        agent_name,
        task_id,
        relative_path,
    )
}

pub(crate) fn claim_with(
    claims: &PeerClaims,
    cas_root: &Path,
    agent_id: &str,
    agent_name: &str,
    task_id: &str,
    relative_path: &str,
) -> FileLease {
    let id = lease_id(relative_path);
    let now = chrono::Utc::now().timestamp();
    let mut ledger = load(cas_root, agent_id);
    if let Some(entry) = ledger.leases.get(&id) {
        let age = now - entry.at_unix_secs;
        if entry.mine && age < OWN_CLAIM_REUSE_SECS {
            return FileLease::Mine;
        }
        if !entry.mine && age < UNAVAILABLE_BACKOFF_SECS {
            return FileLease::Unchecked;
        }
    }
    // The holder's task rides in its display name, so a refused peer reads
    // "worker (task cas-x) on machine" without a wire-format change.
    let holder = format!("{agent_name} (task {task_id})");
    let duration = super::peer_claims::lease_duration_secs(cas_root);
    let agents = crate::store::open_agent_store(cas_root).ok();
    let local_name = |id: &str| -> Option<String> {
        let agent = agents.as_ref()?.get(id).ok()?;
        Some(match agent.factory_session.as_deref() {
            Some(session) => format!("{} (factory session {session}, this machine)", agent.name),
            None => format!("{} (this machine)", agent.name),
        })
    };
    let is_local = |id: &str| id != agent_id && local_name(id).is_some();
    let answer = claims.acquire_within(
        &id,
        ClaimKind::File,
        agent_id,
        &holder,
        duration,
        &is_local,
        CLAIM_BUDGET,
    );
    let (lease, record) = match answer {
        Acquire::Claimed => (FileLease::Mine, Some(true)),
        Acquire::LocalHolder(other) => {
            (FileLease::HeldBy(local_name(&other).unwrap_or(other)), None)
        }
        Acquire::PeerHolds(hold) | Acquire::StalePeer(hold) => {
            // A live holder is named from the agent list; its claim reason
            // also carries the task it is editing for.
            let who = claims
                .recorded_holder(&id, agent_id, CLAIM_BUDGET)
                .map(|recorded| format!("{} on {}", recorded.name, recorded.machine))
                .unwrap_or_else(|| hold.describe());
            (FileLease::HeldBy(who), None)
        }
        Acquire::Unavailable(_) => (FileLease::Unchecked, Some(false)),
    };
    match record {
        Some(mine) => {
            ledger.leases.insert(
                id,
                Entry {
                    path: relative_path.to_string(),
                    mine,
                    at_unix_secs: now,
                },
            );
        }
        None => {
            ledger.leases.remove(&id);
        }
    }
    save(cas_root, agent_id, &ledger);
    lease
}

/// The advisory a worker sees before editing a file another agent holds.
pub fn collision_note(relative_path: &str, holder: &str) -> String {
    format!(
        "⚠️ SHARED FILE (cas-8dbe): {relative_path} is being edited by {holder}. Before you change it, \
         coordinate through your supervisor (`coordination action=message target=supervisor summary=\"shared file {relative_path}\" message=\"<what you need to change>\"`), which can \
         reach the other session's supervisor, so both edits do not collide."
    )
}

/// Release every file lease `agent_id` recorded, when its task ends.
pub fn release_all(cas_root: &Path, agent_id: &str) {
    let ledger = load(cas_root, agent_id);
    if ledger.leases.is_empty() {
        return;
    }
    if let Some(claims) = PeerClaims::for_project(cas_root) {
        for (id, entry) in &ledger.leases {
            if entry.mine {
                let _ = claims.release_own(id, agent_id);
            }
        }
    }
    let _ = std::fs::remove_file(ledger_path(cas_root, agent_id));
}
