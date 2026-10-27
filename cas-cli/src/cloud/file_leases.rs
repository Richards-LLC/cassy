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
    /// Another agent holds it, in this session or elsewhere.
    HeldBy(FileHolder),
    /// No peer check ran (no Cloud login, Cloud unreachable, or backing off).
    Unchecked,
}

/// A holder's identity and its known relationship to the editing worker.
/// Unknown session identity must not imply a different supervisor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileHolder {
    SameSession(String),
    OtherSession(String),
    UnknownSession(String),
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
    /// cas-c55f: leases whose task ended, by lease id, with when the release
    /// was asked. Each stays until Cassy Cloud confirms its release, so a
    /// release that could not reach the Cloud is retried later.
    #[serde(default)]
    releasing: BTreeMap<String, i64>,
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
    load_at(&ledger_path(cas_root, agent_id))
}

fn load_at(path: &Path) -> Ledger {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_at(path: &Path, ledger: &Ledger) {
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

/// cas-c55f: read-modify-write the ledger at `path` under an exclusive lock
/// on its sibling `.lock` file, so a claim and a release by the same worker
/// (an edit hook and a park, in two processes) never overwrite each other.
/// No Cloud call runs inside `change`. An emptied ledger is removed.
fn update_at<R>(path: &Path, change: impl FnOnce(&mut Ledger) -> R) -> R {
    use fs2::FileExt;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"));
    // An unlockable ledger is still updated: the lease is advisory.
    let _guard = lock.as_ref().ok().filter(|file| file.lock_exclusive().is_ok());
    let mut ledger = load_at(path);
    let result = change(&mut ledger);
    if ledger.leases.is_empty() && ledger.releasing.is_empty() && ledger.notified.is_empty() {
        let _ = std::fs::remove_file(path);
    } else {
        save_at(path, &ledger);
    }
    result
}

fn update<R>(cas_root: &Path, agent_id: &str, change: impl FnOnce(&mut Ledger) -> R) -> R {
    update_at(&ledger_path(cas_root, agent_id), |ledger| {
        ledger.agent_id = agent_id.to_string();
        change(ledger)
    })
}

fn ledger_files(cas_root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(cas_root.join("file-leases"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
                .collect()
        })
        .unwrap_or_default()
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
    let ledger = load(cas_root, agent_id);
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
    let session = agents
        .as_ref()
        .and_then(|store| store.get(agent_id).ok())
        .and_then(|agent| agent.factory_session)
        .filter(|session| !session.is_empty());
    let local_holder = |other_id: &str| -> Option<FileHolder> {
        let agent = agents.as_ref()?.get(other_id).ok()?;
        // The file ledger identifies the task this particular edit belongs to.
        let other_ledger = load(cas_root, other_id);
        let worker = match other_ledger.leases.get(&id).and_then(|entry| entry.task_id.as_deref()) {
            Some(task) => format!("{}, task {task}", agent.name),
            None => agent.name,
        };
        let other_session = agent.factory_session.as_deref().filter(|s| !s.is_empty());
        Some(match (session.as_deref(), other_session) {
            (Some(ours), Some(theirs)) if ours == theirs => FileHolder::SameSession(worker),
            (Some(_), Some(theirs)) => FileHolder::OtherSession(format!(
                "{worker} (factory session {theirs}, this machine)"
            )),
            _ => FileHolder::UnknownSession(format!("{worker} (this machine)")),
        })
    };
    let is_local = |id: &str| {
        id != agent_id && agents.as_ref().is_some_and(|store| store.get(id).is_ok())
    };
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
        Acquire::LocalHolder(other) => (
            FileLease::HeldBy(local_holder(&other).unwrap_or(FileHolder::UnknownSession(other))),
            None,
        ),
        Acquire::PeerHolds(hold) | Acquire::StalePeer(hold) => {
            // A live holder is named from the agent list; its claim reason
            // also carries the task it is editing for.
            let who = claims
                .recorded_holder(&id, agent_id, CLAIM_BUDGET)
                .map(|recorded| format!("{} on {}", recorded.name, recorded.machine))
                .unwrap_or_else(|| hold.describe());
            (FileLease::HeldBy(FileHolder::UnknownSession(who)), None)
        }
        Acquire::Unavailable(_) => (FileLease::Unchecked, Some(false)),
    };
    // The Cloud was asked outside the lock; record its answer under it.
    update(cas_root, agent_id, |ledger| match record {
        Some(mine) => {
            if mine {
                // Claimed afresh, so an older release must not drop it.
                ledger.releasing.remove(&id);
            }
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
    });
    lease
}

/// The advisory a worker sees before editing a file another agent holds.
pub fn collision_note(relative_path: &str, holder: &FileHolder) -> String {
    let (who, handoff) = match holder {
        FileHolder::SameSession(worker) => (
            format!("another worker in your session ({worker})"),
            "",
        ),
        FileHolder::OtherSession(worker) => (
            worker.clone(),
            ", which can reach the other session's supervisor",
        ),
        FileHolder::UnknownSession(worker) => (worker.clone(), ""),
    };
    format!(
        "⚠️ SHARED FILE: {relative_path} is being edited by {who}. Before you change it, \
         coordinate through your supervisor (`coordination action=message target=supervisor summary=\"shared file {relative_path}\" message=\"<what you need to change>\"`){handoff}, \
         so both edits do not collide."
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
    holder: &FileHolder,
) -> bool {
    let id = lease_id(relative_path);
    let now = chrono::Utc::now().timestamp();
    let ledger = load(cas_root, agent_id);
    if ledger
        .notified
        .get(&id)
        .is_some_and(|at| now - at < NOTICE_WINDOW_SECS)
    {
        return false;
    }
    let Ok(queue) = crate::store::open_prompt_queue_store(cas_root) else {
        return false;
    };
    let note = collision_note(relative_path, holder);
    if queue
        .enqueue_with_summary("cas", agent_name, &note, factory_session, Some("shared file"))
        .is_err()
    {
        return false;
    }
    update(cas_root, agent_id, |ledger| ledger.notified.insert(id, now));
    true
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
        let holder = FileHolder::UnknownSession("fierce-koala-22 (task cas-ac47) on soundwave".into());
        let path = "hub-web/src/source-admission.ts";
        let first = notify_worker_of_holder(&cas_root, "worker-b", "bright-cobra-44", Some("sess-b"), path, &holder);
        let repeat = notify_worker_of_holder(&cas_root, "worker-b", "bright-cobra-44", Some("sess-b"), path, &holder);
        assert!(first && !repeat, "first={first} repeat={repeat}");
        let queue = crate::store::open_prompt_queue_store(&cas_root).unwrap();
        let queued = queue.peek_all(10).unwrap();
        assert_eq!(queued.len(), 1, "{queued:?}");
        assert_eq!(queued[0].target, "bright-cobra-44");
        // The same advisory a Claude worker gets as PreToolUse context.
        assert_eq!(queued[0].prompt, collision_note(path, &holder));
        // pin: missing session evidence must not invent another supervisor.
        assert!(!queued[0].prompt.contains("other session's supervisor"));
    }
}

/// Release the file leases taken for `task_id` by whichever agent took them,
/// when the task parks, closes, is cancelled, released or reset. On the
/// factory path the worker's close only parks the task and the supervisor
/// closes it, so the release runs by task, not by the calling agent.
///
/// cas-c55f: the local ledgers change at once; the Cloud releases run on a
/// background thread, so a slow Cloud never delays the caller. A release
/// that does not reach the Cloud stays recorded and is retried by the next
/// release sweep or the daemon's claim renewal.
pub fn release_for_task(cas_root: &Path, task_id: &str) {
    let claims = PeerClaims::for_project(cas_root)
        .map(|claims| claims.with_budget(super::peer_claims::PeerBudget::new(RELEASE_BUDGET)));
    let _ = release_for_task_with(claims, cas_root, task_id);
}

/// [`release_for_task`] with the given claims; returns the background
/// thread running the Cloud releases, when there are claims to ask.
pub(crate) fn release_for_task_with(
    claims: Option<PeerClaims>,
    cas_root: &Path,
    task_id: &str,
) -> Option<std::thread::JoinHandle<()>> {
    let now = chrono::Utc::now().timestamp();
    for ledger_file in ledger_files(cas_root) {
        update_at(&ledger_file, |ledger| {
            let ended: Vec<String> = ledger
                .leases
                .iter()
                .filter(|(_, lease)| lease.task_id.as_deref() == Some(task_id))
                .map(|(id, _)| id.clone())
                .collect();
            for id in ended {
                if ledger.leases.remove(&id).is_some_and(|lease| lease.mine) {
                    ledger.releasing.insert(id, now);
                }
            }
        });
    }
    let claims = claims?;
    let cas_root = cas_root.to_path_buf();
    std::thread::Builder::new()
        .name("cas-file-lease-release".into())
        .spawn(move || release_pending(&claims, &cas_root))
        .ok()
}

/// Ask Cassy Cloud to release every recorded pending file lease of this
/// database, as the agent that took it. Released or no-longer-held leases
/// leave the record; failures stay for the next attempt. A pending release
/// older than a lease's lifetime is dropped: the Cloud claim has run out.
pub(crate) fn release_pending(claims: &PeerClaims, cas_root: &Path) {
    let now = chrono::Utc::now().timestamp();
    let lifetime = i64::from(super::peer_claims::lease_duration_secs(cas_root));
    for ledger_file in ledger_files(cas_root) {
        let ledger = load_at(&ledger_file);
        if ledger.releasing.is_empty() || ledger.agent_id.is_empty() {
            continue;
        }
        let mut released = Vec::new();
        for (id, asked) in &ledger.releasing {
            if now - asked >= lifetime || claims.release_own(id, &ledger.agent_id).is_ok() {
                released.push((id.clone(), *asked));
            }
        }
        if released.is_empty() {
            continue;
        }
        update_at(&ledger_file, |ledger| {
            for (id, asked) in released {
                // A release asked again meanwhile keeps its newer record.
                if ledger.releasing.get(&id) == Some(&asked) {
                    ledger.releasing.remove(&id);
                }
            }
        });
    }
}
