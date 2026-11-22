//! Durable ownership of a late close, including its outcome writer.
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{CasService, ToolWorkPermit};

#[derive(Serialize, Deserialize)]
struct Journal {
    id: String,
    task_id: String,
    owner: Option<String>,
    session: Option<String>,
    note: Option<String>,
    failed: bool,
    #[serde(default)]
    retry_peer_park: bool,
    #[serde(default)]
    peer_park_delivery: Option<(u32, Option<String>, Option<String>)>,
}

struct Pending {
    root: PathBuf,
    path: PathBuf,
    _lock: File,
    journal: Journal,
}

impl Pending {
    fn save(&self) -> Result<(), String> {
        let mut file = tempfile::NamedTempFile::new_in(self.path.parent().unwrap())
            .map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec(&self.journal).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        file.persist(&self.path).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn publish(&mut self) -> Result<(), String> {
        if self.journal.note.is_none() {
            self.journal.failed = true;
            self.journal.note = Some("Close continuation was lost before its outcome was recorded; mutation outcome is unknown. Re-query task state before retrying. code=continuation_lost".into());
            self.save()?;
        }
        let store = crate::store::open_task_store_cached(&self.root).map_err(|e| e.to_string())?;
        // Stable per-request identity makes recovery after append/notice crashes
        // idempotent. The file lock serializes writers across MCP backends.
        let marker = format!("CLOSE_OUTCOME: continuation={}", self.journal.id);
        let task = store
            .get(&self.journal.task_id)
            .map_err(|e| e.to_string())?;
        let noted = if task.notes.contains(&marker) {
            Ok(())
        } else {
            store
                .append_note(
                    &self.journal.task_id,
                    &format!(
                        "[{}] 📝 PROGRESS {marker}; task status now `{}`. {}",
                        chrono::Utc::now().format("%Y-%m-%d %H:%M"),
                        task.status,
                        self.journal.note.as_deref().unwrap_or_default(),
                    ),
                )
                .map(|_| ())
                .map_err(|e| e.to_string())
        };
        // A note-store failure must still wake the supervisor and retain the
        // journal for the next recovery pass.
        if self.journal.failed || noted.is_err() {
            let agents = crate::store::open_agent_store(&self.root)
                .map_err(|e| e.to_string())?
                .list(None)
                .map_err(|e| e.to_string())?;
            let recipient = crate::supervisor_notices::resolve(
                &agents,
                self.journal.owner.as_deref(),
                self.journal.session.as_deref(),
            );
            let detail = format!(
                "Background close for {} needs attention (continuation {}). {}{}",
                self.journal.task_id,
                self.journal.id,
                self.journal.note.as_deref().unwrap_or_default(),
                noted
                    .as_ref()
                    .err()
                    .map(|e| format!(
                        " Task note unavailable: {e}. Recovery journal: {}",
                        self.path.display()
                    ))
                    .unwrap_or_default()
            );
            crate::supervisor_notices::enqueue_with_context(
                &self.root,
                &recipient,
                "blocker",
                &detail,
                &self.journal.id,
                Some(serde_json::json!({"task_id": self.journal.task_id,
                    "continuation_id": self.journal.id, "code": "close_continuation_failed"})),
            )
            .map_err(|e| e.to_string())?;
        }
        noted?;
        if self.journal.retry_peer_park {
            let current_delivery = (task.deliverables.delivery_cycle,
                task.deliverables.parked_branch.clone(), task.deliverables.factory_branch_anchor.clone());
            let current = task.status == cas_types::TaskStatus::AwaitingMerge
                && self.journal.peer_park_delivery.as_ref() == Some(&current_delivery);
            // This journal now owns publication, not a second close. The
            // publisher's immutable outbox key makes crash replay idempotent.
            if current {
                let worker = crate::cloud::peer_mailbox::train_parks::host_worker(&self.root, &task)
                    .ok_or("Park publication still needs its registered host worker")?;
                crate::cloud::peer_mailbox::train_parks::publish_ready(&self.root, &task, &worker)?
                    .ok_or("Park publication has no owner notification")?;
            }
            let recovered = format!("CLOSE_RECOVERY_OUTCOME: continuation={}", self.journal.id);
            if !task.notes.contains(&recovered) {
                store.append_note(&task.id, &format!("{recovered}; code={}",
                    if current { "park_publication_recovered" } else { "park_publication_superseded" }))
                    .map_err(|e| e.to_string())?;
            }
        }
        std::fs::remove_file(&self.path).map_err(|e| e.to_string())?;
        // The lock file may remain if the process exits here; only JSON entries
        // are scanned, so it cannot resurrect a completed continuation.
        let _ = std::fs::remove_file(self.path.with_extension("lock"));
        Ok(())
    }
}

pub(super) struct LateClose {
    pending: Option<Pending>,
    // Drop order matters: release worker ownership before signaling idle.
    _worker_guard: Option<std::sync::Arc<File>>,
    work: Option<ToolWorkPermit>,
}

impl LateClose {
    pub(super) fn begin(service: &CasService, task_id: &str) -> Result<Self, String> {
        let worker_guard = worker_close_guard(service)?;
        let directory = service.inner.cas_root.join("close-continuations");
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let id = uuid::Uuid::new_v4().to_string();
        let path = directory.join(format!("{id}.json"));
        let lock = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))
            .map_err(|e| e.to_string())?;
        fs2::FileExt::lock_exclusive(&lock).map_err(|e| e.to_string())?;
        let agent = service
            .inner
            .get_registered_agent_id_read_only()
            .ok()
            .and_then(|id| service.inner.open_agent_store().ok()?.get(&id).ok());
        let pending = Pending {
            root: service.inner.cas_root.clone(),
            path,
            _lock: lock,
            journal: Journal {
                id,
                task_id: task_id.into(),
                owner: agent.as_ref().and_then(|a| a.parent_id.clone()),
                session: agent.and_then(|a| a.factory_session),
                note: None,
                failed: false,
                retry_peer_park: false,
                peer_park_delivery: None,
            },
        };
        pending.save()?;
        Ok(Self {
            pending: Some(pending),
            work: Some(service.tool_work.permit()),
            _worker_guard: worker_guard,
        })
    }

    pub(super) fn id(&self) -> &str {
        &self.pending.as_ref().expect("pending close continuation").journal.id
    }

    pub(super) fn finish(mut self, note: String, failed: bool, retry_peer_park: bool) -> Result<(), String> {
        let mut pending = self.pending.take().unwrap();
        pending.journal.note = Some(note);
        pending.journal.failed = failed;
        pending.journal.retry_peer_park = retry_peer_park;
        if retry_peer_park {
            let task = crate::store::open_task_store_cached(&pending.root).map_err(|e| e.to_string())?
                .get(&pending.journal.task_id).map_err(|e| e.to_string())?;
            pending.journal.peer_park_delivery = Some((task.deliverables.delivery_cycle,
                task.deliverables.parked_branch, task.deliverables.factory_branch_anchor));
        }
        pending.save()?;
        pending.publish()
    }
}

impl Drop for LateClose {
    fn drop(&mut self) {
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        let work = self.work.take();
        let worker_guard = self._worker_guard.take();
        // Even cancellation before the watcher's first poll has an owner. If
        // runtime shutdown cancels this writer too, its unlocked journal is
        // recovered by a surviving or replacement daemon.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn_blocking(move || {
                let _work = work;
                let _worker_guard = worker_guard;
                if let Err(error) = pending.publish() {
                    tracing::warn!(%error, "Lost close continuation retained for recovery");
                }
            });
        }
    }
}

/// A lock proves execution/recording still owns the promise. No PID reuse or
/// elapsed-time guess can mistake a slow close for a dead one.
pub(crate) fn recover(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root.join("close-continuations")) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let recover = || -> Result<(), String> {
            let lock = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path.with_extension("lock"))
                .map_err(|e| e.to_string())?;
            match fs2::FileExt::try_lock_exclusive(&lock) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error.to_string()),
            }
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.to_string()),
            };
            let journal = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            Pending {
                root: root.into(),
                path: path.clone(),
                _lock: lock,
                journal,
            }
            .publish()
        };
        if let Err(error) = recover() {
            tracing::warn!(path = %path.display(), %error, "Close continuation recovery will retry");
        }
    }
}

/// These lock files are never unlinked: replacing an inode could admit a close
/// concurrently with teardown. File locks end with their owning process.
fn worker_lock(root: &Path, id: &str) -> Result<File, String> {
    use sha2::{Digest, Sha256};
    let directory = root.join("worker-close-locks");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    OpenOptions::new().create(true).truncate(false).read(true).write(true)
        .open(directory.join(format!("{}.lock", hex::encode(Sha256::digest(id.as_bytes())))))
        .map_err(|e| e.to_string())
}

/// Acquire before the handler starts, including the interval before its late
/// journal exists. Cloned ownership bridges the early response to its writer.
pub(super) fn worker_close_guard(service: &CasService) -> Result<Option<std::sync::Arc<File>>, String> {
    let Ok(id) = service.inner.get_registered_agent_id_read_only() else { return Ok(None) };
    let agents = service.inner.open_agent_store().map_err(|e| e.to_string())?;
    let agent = agents.get(&id).map_err(|e| e.to_string())?;
    if agent.role != cas_types::AgentRole::Worker { return Ok(None); }
    let lock = worker_lock(&service.inner.cas_root, &id)?;
    fs2::FileExt::try_lock_shared(&lock).map_err(|e| format!(
        "Worker shutdown owns this process; no close started. code=worker_shutdown_pending: {e}"))?;
    if agents.get(&id).map_err(|e| e.to_string())?.status == cas_types::AgentStatus::Shutdown {
        return Err("Worker is shut down; no close started. code=worker_shutdown_pending".into());
    }
    Ok(Some(std::sync::Arc::new(lock)))
}

/// Hold through signalling/retirement, not just admission. Force and recycle
/// cannot discard publication that has not reached its durable outcome yet.
pub(crate) fn shutdown_guards<'a>(root: &Path, workers: impl IntoIterator<Item = &'a str>) -> Result<Vec<File>, String> {
    workers.into_iter().map(|id| {
        let lock = worker_lock(root, id)?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|e| format!(
            "Worker {id} still owns a close or its outcome writer. Wait for CLOSE_OUTCOME, then retry shutdown. code=worker_close_pending: {e}"))?;
        Ok(lock)
    }).collect()
}
