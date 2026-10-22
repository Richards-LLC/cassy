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
/// cas-a30e9: a worker is sent the contended-file notice at most once per
/// file in this window (Codex workers, which get it as a message).
const NOTICE_WINDOW_SECS: i64 = OWN_CLAIM_REUSE_SECS;

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
    /// The agent that holds this ledger's leases, so a release run by another
    /// agent (a supervisor's park, close or cancel) releases them as it.
    #[serde(default)]
    agent_id: String,
    #[serde(default)]
    leases: BTreeMap<String, Entry>,
    /// When this agent was last sent the contended-file notice, by lease id.
    #[serde(default)]
    notified: BTreeMap<String, i64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    path: String,
    mine: bool,
    at_unix_secs: i64,
    /// The task the lease was taken for (cas-8dbe review: release by task).
    #[serde(default)]
    task_id: Option<String>,
}

/// The whole release of one task's leases waits at most this long.
const RELEASE_BUDGET: Duration = Duration::from_secs(5);

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
                    task_id: Some(task_id.to_string()),
                },
            );
        }
        None => {
            ledger.leases.remove(&id);
        }
    }
    ledger.agent_id = agent_id.to_string();
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

/// cas-a30e9: Codex rejects PreToolUse context, so a Codex worker gets the
/// contended-file advisory as a factory message to itself, the channel its
/// coordination messages already use. Sent at most once per file per
/// [`NOTICE_WINDOW_SECS`]; returns whether a message was queued.
pub fn notify_worker_of_holder(
    cas_root: &Path,
    agent_id: &str,
    agent_name: &str,
    factory_session: Option<&str>,
    relative_path: &str,
    holder: &str,
) -> bool {
    // Red: the pre-a30e9 behaviour, where a Codex worker was never told.
    let _ = (cas_root, agent_id, agent_name, factory_session, relative_path, holder);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cas-a30e9: a Codex worker cannot be given PreToolUse context, so it is
    /// messaged who holds a contended file, once per file per window.
    #[test]
    fn codex_worker_is_messaged_the_holder_once_per_window_cas_a30e9() {
        let dir = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(dir.path()).unwrap();
        let holder = "fierce-koala-22 (task cas-ac47) on soundwave";
        let path = "hub-web/src/source-admission.ts";
        let first = notify_worker_of_holder(&cas_root, "worker-b", "bright-cobra-44", Some("sess-b"), path, holder);
        let repeat = notify_worker_of_holder(&cas_root, "worker-b", "bright-cobra-44", Some("sess-b"), path, holder);
        assert!(first && !repeat, "first={first} repeat={repeat}");
        let queue = crate::store::open_prompt_queue_store(&cas_root).unwrap();
        let queued = queue.peek_all(10).unwrap();
        assert_eq!(queued.len(), 1, "{queued:?}");
        assert_eq!(queued[0].target, "bright-cobra-44");
        // The same advisory a Claude worker gets as PreToolUse context.
        assert_eq!(queued[0].prompt, collision_note(path, holder));
    }
}

/// Release the file leases taken for `task_id` by whichever agent took them,
/// when the task parks, closes or is cancelled. On the factory path the
/// worker's close only parks the task and the supervisor closes it, so the
/// release runs by task, not by the calling agent.
pub fn release_for_task(cas_root: &Path, task_id: &str) {
    let claims = PeerClaims::for_project(cas_root)
        .map(|claims| claims.with_budget(super::peer_claims::PeerBudget::new(RELEASE_BUDGET)));
    release_for_task_with(claims.as_ref(), cas_root, task_id);
}

pub(crate) fn release_for_task_with(claims: Option<&PeerClaims>, cas_root: &Path, task_id: &str) {
    let Ok(entries) = std::fs::read_dir(cas_root.join("file-leases")) else {
        return;
    };
    for entry in entries.flatten() {
        let ledger_file = entry.path();
        if ledger_file.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(mut ledger) = std::fs::read_to_string(&ledger_file)
            .ok()
            .and_then(|text| serde_json::from_str::<Ledger>(&text).ok())
        else {
            continue;
        };
        let held: Vec<String> = ledger
            .leases
            .iter()
            .filter(|(_, lease)| lease.task_id.as_deref() == Some(task_id))
            .map(|(id, _)| id.clone())
            .collect();
        if held.is_empty() {
            continue;
        }
        for id in &held {
            if let (Some(claims), Some(lease)) = (claims, ledger.leases.get(id))
                && lease.mine
                && !ledger.agent_id.is_empty()
            {
                let _ = claims.release_own(id, &ledger.agent_id);
            }
            ledger.leases.remove(id);
        }
        if ledger.leases.is_empty() {
            let _ = std::fs::remove_file(&ledger_file);
        } else {
            let agent = ledger.agent_id.clone();
            save(cas_root, &agent, &ledger);
        }
    }
}
