//! Durable SessionEnd cleanup. Hooks only enqueue; daemon presence work drains.
//! Completed intents fence late local heartbeats until their 24-hour retention
//! ends. Neither tokens nor agent metadata are copied into the intent.
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
    agent: &str,
    session: &str,
    endpoint: &str,
) -> std::io::Result<()> {
    let now = Utc::now();
    write(
        &path(root, agent, session),
        &Intent {
            agent_id: agent.into(),
            session_id: session.into(),
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

/// Do not register/heartbeat a cloned agent after its SessionEnd has queued
/// cleanup. A new session using the same durable id has a different key.
pub(crate) fn is_ended(root: &Path, agent: &Agent) -> bool {
    path(
        root,
        &agent.id,
        agent.cc_session_id.as_deref().unwrap_or(&agent.id),
    )
    .exists()
}

fn paths(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let dir = root.join(DIR);
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

fn read(path: &Path) -> std::io::Result<Intent> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

/// Local status includes failed and expired intents even after the ended
/// agent row is gone. Expired failures remain visible and are never retried.
pub(crate) fn health(root: &Path, now: DateTime<Utc>) -> String {
    let status = || -> std::io::Result<String> {
        let mut pending = 0;
        let mut expired = 0;
        let mut causes = Vec::new();
        for path in paths(root)? {
            let intent = read(&path)?;
            if intent.completed_at.is_some() {
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
        if pending + expired == 0 {
            return Ok(String::new());
        }
        Ok(super::presence::wrap_line(&format!(
            "Cloud presence: shutdown pending {pending}, expired {expired}: {}. Expired cleanup requires operator attention.",
            causes.join("; ")
        )))
    };
    status().unwrap_or_else(|error| {
        super::presence::wrap_line(&format!(
            "Cloud presence: shutdown health unavailable: {error}"
        ))
    })
}

/// One nonblocking lease per project prevents concurrent daemons from sending
/// the same cleanup. The entire off-loop pass, including retries, gets 5s.
pub(crate) fn drain(
    root: &Path,
    coordinator: &CloudCoordinator,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let files = paths(root).map_err(|error| error.to_string())?;
    if files.is_empty() {
        return Ok(());
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(DIR).join("drain.lock"))
        .map_err(|error| error.to_string())?;
    match lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if error.kind() == fs2::lock_contended_error().kind() => return Ok(()),
        Err(error) => return Err(error.to_string()),
    }
    let deadline = Instant::now() + rate_limit::CLEANUP_BUDGET;
    let agents = crate::store::open_agent_store(root).map_err(|error| error.to_string())?;
    for file in files {
        let mut intent = read(&file).map_err(|error| error.to_string())?;
        if intent.completed_at.is_some() {
            if now >= intent.expires_at {
                std::fs::remove_file(&file).map_err(|error| error.to_string())?;
            }
            continue;
        }
        if now >= intent.expires_at || now < intent.next_retry_at {
            continue;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        // Never tear down a row that has been rebound to a newer harness
        // session (clear_context). Check even though normal SessionEnd already
        // guards this before enqueueing.
        if agents.get(&intent.agent_id).is_ok_and(|agent| {
            agent
                .cc_session_id
                .as_deref()
                .is_some_and(|session| session != intent.session_id)
        }) {
            intent.completed_at = Some(now);
            intent.cause = "superseded by a new local session".into();
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
                    let delay = error
                        .retry_after
                        .and_then(|delay| chrono::Duration::from_std(delay).ok())
                        .unwrap_or_else(|| {
                            chrono::Duration::seconds(
                                (30_i64 << intent.attempts.saturating_sub(1).min(4)).min(300),
                            )
                        });
                    intent.next_retry_at = Utc::now()
                        .checked_add_signed(delay)
                        .unwrap_or(intent.expires_at)
                        .min(intent.expires_at);
                }
            }
        }
        write(&file, &intent, false).map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "shutdown_tests.rs"]
mod tests;
