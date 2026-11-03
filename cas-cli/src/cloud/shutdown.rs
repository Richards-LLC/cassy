//! Durable SessionEnd cleanup. Hooks only enqueue; daemon presence work drains.
//! Intents belong to a registration incarnation, including same-session resumes.
//! Neither tokens nor agent metadata are copied into the intent.
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{CloudCoordinator, rate_limit};
use crate::types::Agent;

const DIR: &str = "cloud_shutdown";
const TTL_HOURS: i64 = 24;

#[derive(Serialize, Deserialize)]
struct Intent {
    agent_id: String,
    session_id: String,
    #[serde(default)]
    registered_at: Option<DateTime<Utc>>,
    endpoint: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    next_retry_at: DateTime<Utc>,
    attempts: u32,
    cause: String,
    #[serde(default)]
    completed_at: Option<DateTime<Utc>>,
}

fn path(root: &Path, agent: &str, session: &str) -> PathBuf {
    root.join(DIR).join(format!(
        "{:x}.json",
        Sha256::digest(serde_json::to_vec(&(agent, session)).expect("string tuple serializes"))
    ))
}

fn incarnation_path(root: &Path, agent: &Agent, session: &str) -> PathBuf {
    root.join(DIR).join(format!(
        "{:x}.json",
        Sha256::digest(
            serde_json::to_vec(&(&agent.id, session, agent.registered_at))
                .expect("incarnation serializes")
        )
    ))
}

fn write(path: &Path, intent: &Intent, create: bool) -> std::io::Result<()> {
    let parent = path.parent().expect("intent directory");
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(&serde_json::to_vec(intent)?)?;
    tmp.as_file().sync_all()?;
    if create {
        match tmp.persist_noclobber(path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
            Err(error) => return Err(error.error),
        }
    } else {
        tmp.persist(path).map_err(|error| error.error)?;
    }
    File::open(parent)?.sync_all()
}

/// Local, atomic and idempotent. A repeated SessionEnd does not reset the TTL.
pub(crate) fn enqueue(
    root: &Path,
    agent: &Agent,
    session: &str,
    endpoint: &str,
) -> std::io::Result<()> {
    let now = Utc::now();
    write(
        &incarnation_path(root, agent, session),
        &Intent {
            agent_id: agent.id.clone(),
            session_id: session.into(),
            registered_at: Some(agent.registered_at),
            endpoint: endpoint.into(),
            created_at: now,
            expires_at: now + chrono::Duration::hours(TTL_HOURS),
            next_retry_at: now,
            attempts: 0,
            cause: "awaiting daemon cleanup".into(),
            completed_at: None,
        },
        true,
    )
}

fn supersedes(intent: &Intent, agent: &Agent) -> bool {
    let session = agent.cc_session_id.as_deref();
    session.is_some_and(|session| session != intent.session_id)
        || (session == Some(intent.session_id.as_str())
            && agent.registered_at > intent.registered_at.unwrap_or(intent.created_at))
}

/// Parsed once per presence pass, including failures scoped to their file.
/// A failed directory listing does not take every unrelated agent offline.
pub(crate) struct Snapshot {
    files: std::collections::BTreeMap<PathBuf, Result<Intent, String>>,
    listing_failed: bool,
    generation: Option<DirectoryGeneration>,
}

/// Exact sorted intent names detect changes even when timestamps and entry
/// counts coincide. No hash collision or writer-side counter is involved.
#[derive(PartialEq, Eq)]
struct DirectoryGeneration {
    modified: Option<std::time::SystemTime>,
    files: Vec<PathBuf>,
}

impl Snapshot {
    pub(crate) fn load(root: &Path) -> Self {
        Self::from_observation(directory_generation(root))
    }

    fn from_observation(observation: std::io::Result<DirectoryGeneration>) -> Self {
        match observation {
            Ok(generation) => {
                let files = generation
                    .files
                    .iter()
                    .cloned()
                    .filter_map(|file| match read(&file) {
                        Ok(intent) => Some((file, Ok(intent))),
                        Err(error) if missing(&error) => None,
                        Err(error) => Some((file, Err(error.to_string()))),
                    })
                    .collect();
                Self {
                    files,
                    listing_failed: false,
                    generation: Some(generation),
                }
            }
            Err(error) => {
                tracing::warn!(%error, "cloud shutdown intent listing unavailable; checking agent fences directly");
                Self {
                    files: Default::default(),
                    listing_failed: true,
                    generation: None,
                }
            }
        }
    }

    /// Call only after acquiring the shared presence lease. Compare intent
    /// names as well as mtime, so coarse timestamps cannot hide a late fence.
    /// Reuse the observed listing on reload instead of walking it twice.
    pub(crate) fn refresh_after_lease(&mut self, root: &Path) {
        let current = directory_generation(root);
        if self.listing_failed || self.generation.as_ref() != current.as_ref().ok() {
            *self = Self::from_observation(current);
        }
    }

    pub(crate) fn is_ended(
        &mut self,
        root: &Path,
        agent: &Agent,
        now: DateTime<Utc>,
    ) -> Result<bool, String> {
        let session = agent.cc_session_id.as_deref().unwrap_or(&agent.id);
        let direct = [
            incarnation_path(root, agent, session),
            path(root, &agent.id, session),
        ];
        if self.listing_failed {
            if agent.cc_session_id.is_none() {
                return Err("shutdown intent unavailable: cannot check an unknown harness session without a directory listing".into());
            }
            for file in &direct {
                if !self.files.contains_key(file) {
                    match read(file) {
                        Ok(intent) => {
                            self.files.insert(file.clone(), Ok(intent));
                        }
                        Err(error) if missing(&error) => {}
                        Err(error) => {
                            self.files.insert(file.clone(), Err(error.to_string()));
                        }
                    }
                }
            }
        }
        for (file, result) in &self.files {
            match result {
                Ok(intent)
                    if intent.agent_id == agent.id
                        && !supersedes(intent, agent)
                        && now < intent.expires_at =>
                {
                    return Ok(true);
                }
                Err(error) if direct.contains(file) => {
                    return Err(format!("shutdown intent unavailable: {error}"));
                }
                _ => {}
            }
        }
        Ok(false)
    }
}

fn directory_generation(root: &Path) -> std::io::Result<DirectoryGeneration> {
    // Capture mtime BEFORE listing. A change during snapshot loading must
    // remain observable at the post-lease check too. Intent paths are sorted.
    let modified = std::fs::metadata(root.join(DIR))
        .and_then(|metadata| metadata.modified())
        .ok();
    Ok(DirectoryGeneration {
        modified,
        files: paths(root)?,
    })
}

/// Revalidate before replacing an existing intent. A stale pass must not
/// recreate retired work or overwrite a completion recorded during HTTP.
fn persist_pending(file: &Path, candidate: Intent) -> std::io::Result<Option<Intent>> {
    match read(file) {
        Ok(current) if current.completed_at.is_some() => Ok(Some(current)),
        Ok(_) => {
            write(file, &candidate, false)?;
            Ok(Some(candidate))
        }
        Err(error) if missing(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn complete_superseded(file: &Path, now: DateTime<Utc>) -> std::io::Result<Option<Intent>> {
    let mut current = match read(file) {
        Ok(current) => current,
        Err(error) if missing(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    if current.completed_at.is_none() {
        current.completed_at = Some(now);
        current.cause = "superseded by a new local registration".into();
        persist_pending(file, current)
    } else {
        Ok(Some(current))
    }
}

fn missing(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

/// NULL-session automatic reinsertion stays fenced regardless of PID ancestry.
pub(crate) fn is_ended(root: &Path, agent: &Agent) -> Result<bool, String> {
    Snapshot::load(root).is_ended(root, agent, Utc::now())
}

fn json_paths(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|ext| ext == "json") {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

fn paths(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    json_paths(&root.join(DIR))
}

fn read(path: &Path) -> std::io::Result<Intent> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

#[derive(Serialize, Deserialize)]
struct ExpiredHealth {
    agent_id: String,
    cause: String,
}

/// Retain the diagnostic before removing the expired work item. The exclusive
/// drain lease serializes this with other daemons; rename makes crash replay safe.
fn retain_expired(root: &Path, file: &Path, intent: &Intent) -> std::io::Result<()> {
    let dir = root.join(DIR).join("expired");
    std::fs::create_dir_all(&dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
    tmp.write_all(&serde_json::to_vec(&ExpiredHealth {
        agent_id: intent.agent_id.clone(),
        cause: intent.cause.clone(),
    })?)?;
    tmp.as_file().sync_all()?;
    tmp.persist(dir.join(file.file_name().expect("intent filename")))
        .map_err(|error| error.error)?;
    File::open(&dir)?.sync_all()?;
    std::fs::remove_file(file)
}

/// Local status includes failed and expired intents even after the ended
/// agent row is gone. Expired failures remain visible and are never retried.
pub(crate) fn health(root: &Path, now: DateTime<Utc>) -> String {
    let status = || -> std::io::Result<String> {
        let mut pending = 0;
        let mut expired = 0;
        let mut causes = Vec::new();
        let agents = if root.join("cas.db").is_file() {
            Some(crate::store::open_agent_store(root).map_err(std::io::Error::other)?)
        } else {
            None
        };
        for path in paths(root)? {
            let intent = match read(&path) {
                Ok(intent) => intent,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if intent.completed_at.is_some() {
                continue;
            }
            if agents
                .as_ref()
                .and_then(|store| store.get(&intent.agent_id).ok())
                .is_some_and(|agent| supersedes(&intent, &agent))
            {
                continue;
            }
            if now >= intent.expires_at {
                expired += 1;
            } else {
                pending += 1;
            }
            if causes.len() < 3 {
                causes.push(format!("{}: {}", intent.agent_id, intent.cause));
            }
        }
        for file in json_paths(&root.join(DIR).join("expired"))? {
            // A crash between durable health and unlink must not double-count.
            if root
                .join(DIR)
                .join(file.file_name().expect("health filename"))
                .exists()
            {
                continue;
            }
            let record: ExpiredHealth = serde_json::from_slice(&std::fs::read(file)?)?;
            expired += 1;
            if causes.len() < 3 {
                causes.push(format!("{}: {}", record.agent_id, record.cause));
            }
        }
        if pending + expired == 0 {
            return Ok(String::new());
        }
        let action = if expired > 0 {
            "Inspect expired cleanup: ls .cas/cloud_shutdown"
        } else {
            "Retry: automatic when a daemon runs, for up to 24h."
        };
        let mut out = super::presence::wrap_line(&format!(
            "Cloud presence: shutdown pending {pending}, expired {expired}."
        ));
        out.push_str(&super::presence::wrap_line(action));
        let causes = causes.join("; ");
        let excerpt: String = causes.chars().take(140).collect();
        out.push_str(&super::presence::wrap_line(&format!("Cause: {excerpt}")));
        Ok(out)
    };
    status().unwrap_or_else(|error| {
        super::presence::wrap_line(&format!(
            "Cloud presence: shutdown health unavailable: {error}"
        ))
    })
}

/// Readers can register/heartbeat concurrently; cleanup excludes all readers.
/// Hooks do not acquire this network-work lease.
fn lease(root: &Path, exclusive: bool) -> Result<Option<File>, String> {
    std::fs::create_dir_all(root.join(DIR)).map_err(|error| error.to_string())?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(DIR).join("drain.lock"))
        .map_err(|error| error.to_string())?;
    let result = if exclusive {
        FileExt::try_lock_exclusive(&lock)
    } else {
        FileExt::try_lock_shared(&lock)
    };
    match result {
        Ok(()) => Ok(Some(lock)),
        Err(error) if error.kind() == fs2::lock_contended_error().kind() => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn presence_lease(root: &Path) -> Result<Option<File>, String> {
    lease(root, false)
}

/// One nonblocking lease per project prevents concurrent daemons from sending
/// the same cleanup. The entire off-loop pass, including retries, gets 5s.
#[cfg(test)]
pub(crate) fn drain(
    root: &Path,
    coordinator: &CloudCoordinator,
    now: DateTime<Utc>,
) -> Result<(), String> {
    drain_snapshot(root, coordinator, now, &mut Snapshot::load(root))
}

pub(crate) fn drain_snapshot(
    root: &Path,
    coordinator: &CloudCoordinator,
    now: DateTime<Utc>,
    snapshot: &mut Snapshot,
) -> Result<(), String> {
    // Supersession and completed-fence pruning need no network-work lease.
    // Only cleanup that is actually due competes with presence readers.
    let mut due = Vec::new();
    let mut retired = Vec::new();
    let mut agents = None;
    for (file, result) in &mut snapshot.files {
        let intent = match result {
            Ok(intent) => intent,
            Err(error) => {
                tracing::warn!(path=%file.display(), %error, "unreadable cloud shutdown intent");
                continue;
            }
        };
        if intent.completed_at.is_some() {
            if now >= intent.expires_at {
                if let Err(error) = std::fs::remove_file(file) {
                    if !missing(&error) {
                        tracing::warn!(%error, "could not prune completed shutdown intent");
                    }
                }
            }
            continue;
        }
        if root.join("cas.db").is_file() {
            if agents.is_none() {
                agents =
                    Some(crate::store::open_agent_store(root).map_err(|error| error.to_string())?);
            }
            match agents
                .as_ref()
                .expect("opened agent store")
                .get(&intent.agent_id)
            {
                Ok(agent) if supersedes(intent, &agent) => {
                    match complete_superseded(file, now).map_err(|error| error.to_string())? {
                        Some(current) => *intent = current,
                        None => retired.push(file.clone()),
                    }
                    continue;
                }
                Ok(_) | Err(cas_store::StoreError::NotFound(_)) => {}
                Err(error) => {
                    tracing::warn!(%error, "local session check unavailable before shutdown lease");
                    continue;
                }
            }
        }
        if now >= intent.next_retry_at || now >= intent.expires_at {
            due.push(file.clone());
        }
    }
    for file in retired {
        snapshot.files.remove(&file);
    }
    if due.is_empty() {
        return Ok(());
    }
    let Some(_lease) = lease(root, true)? else {
        return Ok(());
    };
    let deadline = Instant::now() + rate_limit::CLEANUP_BUDGET;
    let agents = crate::store::open_agent_store(root).map_err(|error| error.to_string())?;
    for file in due {
        // Revalidate after taking the lease: another daemon may have drained
        // this due item since the snapshot. Future items are never re-read.
        let mut intent = match read(&file) {
            Ok(intent) => intent,
            Err(error) => {
                tracing::warn!(path = %file.display(), %error, "unreadable cloud shutdown intent");
                continue;
            }
        };
        if intent.completed_at.is_some() {
            continue;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        // Never tear down a row that has been rebound to a newer harness
        // session (clear_context). Check even though normal SessionEnd already
        // guards this before enqueueing.
        let current = match agents.get(&intent.agent_id) {
            Ok(agent) => Some(agent),
            Err(cas_store::StoreError::NotFound(_)) => None,
            Err(error) => {
                intent.cause = format!("local session check failed: {error}");
                intent.next_retry_at = now + chrono::Duration::seconds(30);
                match persist_pending(&file, intent).map_err(|error| error.to_string())? {
                    Some(current) => {
                        snapshot.files.insert(file, Ok(current));
                    }
                    None => {
                        snapshot.files.remove(&file);
                    }
                }
                continue;
            }
        };
        if current
            .as_ref()
            .is_some_and(|agent| supersedes(&intent, agent))
        {
            intent.completed_at = Some(now);
            intent.cause = "superseded by a new local registration".into();
        } else if now >= intent.expires_at {
            retain_expired(root, &file, &intent).map_err(|error| error.to_string())?;
            snapshot.files.remove(&file);
            continue;
        } else if now < intent.next_retry_at {
            continue;
        } else if coordinator.cloud_config().endpoint != intent.endpoint {
            intent.cause = "cloud endpoint changed; awaiting original endpoint".into();
            intent.next_retry_at = now + chrono::Duration::minutes(5);
        } else {
            let result = coordinator
                .clone()
                .with_agent_id(&intent.agent_id)
                .with_timeout(left)
                .shutdown_with_retry();
            intent.attempts = intent.attempts.saturating_add(1);
            match result {
                Ok(_) => intent.completed_at = Some(Utc::now()),
                Err(error) if error.status == Some(404) => intent.completed_at = Some(Utc::now()),
                Err(error) => {
                    intent.cause = error.cause;
                    intent.next_retry_at = match error.retry_after {
                        Some(delay) => chrono::Duration::from_std(delay)
                            .ok()
                            .and_then(|delay| Utc::now().checked_add_signed(delay))
                            .unwrap_or(intent.expires_at),
                        None => {
                            Utc::now()
                                + chrono::Duration::seconds(
                                    (30_i64 << intent.attempts.saturating_sub(1).min(4)).min(300),
                                )
                        }
                    }
                    .min(intent.expires_at);
                }
            }
        }
        match persist_pending(&file, intent).map_err(|error| error.to_string())? {
            Some(current) => {
                snapshot.files.insert(file, Ok(current));
            }
            None => {
                snapshot.files.remove(&file);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "shutdown_tests.rs"]
mod tests;
