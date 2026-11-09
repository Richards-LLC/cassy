//! Queue policy and dispatch. Task start remains the owner of execution policy.
use std::{fs::File, path::Path};

use cas_store::worker_task_queue_store::{SqliteWorkerTaskQueueStore, WorkerTaskQueue};
use cas_types::{Agent, AgentRole, TaskStatus, TaskType};
use fs2::FileExt;
use rmcp::{ErrorData as McpError, model::ErrorCode};

use crate::{mcp::CasCore, mcp::tools::TaskStartRequest};

pub(crate) fn conversation_generation(worker: &Agent) -> String {
    format!("{}|{}", worker.id, worker.cc_session_id.as_deref().unwrap_or_default())
}

fn error(message: impl std::fmt::Display) -> McpError {
    CasCore::error(ErrorCode::INVALID_PARAMS, message.to_string())
}

fn worker_lock_file(root: &Path, worker: &Agent, operation: &str) -> Result<File, McpError> {
    let session = worker.factory_session.as_deref().unwrap_or_default();
    let key = format!("{session}-{}-{operation}", worker.name)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    let dir = root.join("locks");
    std::fs::create_dir_all(&dir).map_err(error)?;
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(format!("worker-{key}.lock")))
        .map_err(error)
}

/// File locks fence Git and queue mutations across separate MCP processes.
pub(crate) fn worker_lock(root: &Path, worker: &Agent, operation: &str) -> Result<File, McpError> {
    let file = worker_lock_file(root, worker, operation)?;
    file.try_lock_exclusive().map_err(|_| {
        error(format!(
            "Worker {} has a {operation} operation in progress; retry after it finishes",
            worker.name
        ))
    })?;
    Ok(file)
}

/// Plain starts serialize without a queue-specific contention refusal. Keep
/// the runtime free while waiting for another process's start to finish.
pub(crate) async fn wait_for_worker_start(root: &Path, worker: &Agent) -> Result<File, McpError> {
    wait_for_worker_lock(root, worker, "start").await
}

async fn wait_for_worker_lock(root: &Path, worker: &Agent, operation: &str) -> Result<File, McpError> {
    let file = worker_lock_file(root, worker, operation)?;
    tokio::task::spawn_blocking(move || {
        file.lock_exclusive().map_err(error)?;
        Ok(file)
    }).await.map_err(error)?
}

/// A queue may switch branches only inside this worker's own checkout.
pub(crate) fn ensure_owned_queue_checkout(worker: &Agent) -> Result<(), McpError> {
    let checkout = worker.metadata.get("clone_path").ok_or_else(|| {
        error(format!(
            "Worker {} needs an isolated checkout for task queues; respawn with isolate=true",
            worker.name
        ))
    })?;
    crate::factory_isolation::verify_worker_worktree_binding(
        &worker.name,
        Path::new(checkout),
        None,
    )
    .map_err(error)
}

/// Only this module can construct a start principal on behalf of a worker.
pub(crate) struct QueueStartPrincipal(Agent, File);
impl QueueStartPrincipal {
    pub(crate) fn agent(&self) -> &Agent {
        &self.0
    }
}

/// Resolve booting names through their durable, session-scoped spawn receipt.
/// A later failed spawn must not fall back to an older launch of the same name.
fn assignment_worker(
    root: &Path, assignee: &str, live: Option<&Agent>,
) -> crate::Result<Option<(Agent, Option<cas_store::SpawnLifecycle>)>> {
    let Some(session) = (match live {
        Some(worker) => worker.factory_session.clone(),
        None => std::env::var("CAS_FACTORY_SESSION").ok().filter(|s| !s.is_empty()),
    })
    else { return Ok(None); };
    let name = live.map_or(assignee, |worker| worker.name.as_str());
    let receipt = crate::store::open_spawn_queue_store(root)?
        .recent_spawn_lifecycle(&session, 100)?
        .into_iter()
        .find(|r| r.worker_name.as_deref().is_some_and(|worker| worker.eq_ignore_ascii_case(name))
            || (r.worker_name.is_none() && r.requested_names.len() == 1
                && r.requested_names[0].eq_ignore_ascii_case(name)));
    let worker = match live {
        Some(worker) => worker.clone(),
        None => {
            let Some(receipt) = receipt.as_ref().filter(|r| !r.state.is_terminal()) else {
                return Ok(None);
            };
            let name = receipt.worker_name.clone().unwrap_or_else(|| receipt.requested_names[0].clone());
            let mut worker = Agent::new_with_role(String::new(), name, AgentRole::Worker);
            worker.factory_session = Some(session);
            worker
        }
    };
    Ok(Some((worker, receipt)))
}

/// Admit mutation/director assignments once, including the pre-registration
/// gap. Spawn binding happens first so it cannot overwrite an admitted tail.
pub(crate) fn admit_assignment(root: &Path, task: &cas_types::Task, live: Option<&Agent>) -> crate::Result<bool> {
    if task.status != TaskStatus::Open { return Ok(false); }
    let Some(assignee) = task.assignee.as_deref() else { return Ok(false); };
    let Some((worker, receipt)) = assignment_worker(root, assignee, live)? else { return Ok(false); };
    let session = worker.factory_session.as_deref().expect("assignment session");
    let queues = SqliteWorkerTaskQueueStore::open(root)?;
    if let Some(receipt) = &receipt {
        queues.bind_spawn(receipt.id, session, &worker.name)?;
    }
    let tasks = crate::store::open_task_store(root)?;
    // A director snapshot may predate a cancellation or reassignment.
    let current = tasks.get(&task.id)?;
    if current.status != TaskStatus::Open || current.assignee != task.assignee {
        return Ok(false);
    }
    let queue = queues.get(session, &worker.name)?;
    let head = if queue.revision == 0 {
        let active = if live.is_some() && worker.metadata.contains_key("clone_path") {
            tasks.list(Some(TaskStatus::InProgress))?.into_iter().find(|t|
                t.assignee.as_deref().is_some_and(|a| a == worker.name || a == worker.id))
        } else { None };
        let head = active.or_else(|| receipt.as_ref()
            .filter(|r| !matches!(r.state, cas_store::SpawnLifecycleState::Failed | cas_store::SpawnLifecycleState::Stopped))
            .and_then(|r| r.task_id.as_ref()).and_then(|id| tasks.get(id).ok()));
        // A single-task spawn keeps its existing startup path until a tail is
        // actually assigned. Ordinary independent assignments also stay plain.
        head
    } else { None };
    let admitted = queues.admit_assignment(session, &worker.name, &task.id,
        head.as_ref().map(|head| (head.id.as_str(), head.status == TaskStatus::InProgress)))?;
    if admitted {
        let _ = cas_factory::notify_daemon(root);
    }
    Ok(admitted)
}

impl CasCore {
    fn queue_worker(&self, name: &str, supervisor_only: bool) -> Result<Agent, McpError> {
        let agents = self.open_agent_store()?;
        let caller_id = self.get_registered_agent_id_read_only()?;
        let caller = agents.get(&caller_id).map_err(error)?;
        let worker = agents
            .list(None)
            .map_err(error)?
            .into_iter()
            .filter(|a| a.name.eq_ignore_ascii_case(name) || a.id == name)
            .max_by_key(|a| a.registered_at)
            .filter(|a| {
                a.role == AgentRole::Worker
                    && a.is_alive()
                    && (chrono::Utc::now() - a.last_heartbeat).num_seconds()
                        <= cas_types::DEFAULT_HEARTBEAT_TIMEOUT_SECS
            })
            .ok_or_else(|| error(format!("No live registered worker '{name}'")))?;
        let session = worker
            .factory_session
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| error("Worker task queues require a factory session"))?;
        if !supervisor_only && caller.id == worker.id {
            return Ok(worker);
        }
        let supervisor = self
            .resolve_live_supervisor_authority()
            .map_err(|_| error("Worker queues require a live owning supervisor"))?;
        let owner =
            crate::mcp::tools::core::task::lifecycle::supervisor_push::resolve_worker_supervisor(
                agents.as_ref(),
                Some(session),
            );
        if supervisor.factory_session.as_deref() != Some(session)
            || owner
                .as_ref()
                .is_none_or(|owner| owner.agent_id != supervisor.id)
        {
            return Err(error(
                "Worker task queue belongs to another factory supervisor",
            ));
        }
        Ok(worker)
    }

    /// Inspect or replace the pending list. The current execution is preserved.
    pub(crate) async fn configure_worker_task_queue(
        &self,
        name: &str,
        task_ids: Option<Vec<String>>,
        expected: Option<u64>,
    ) -> Result<String, McpError> {
        let worker = self.queue_worker(name, true)?;
        let session = worker
            .factory_session
            .as_deref()
            .expect("validated session");
        let queues = SqliteWorkerTaskQueueStore::open(&self.cas_root).map_err(error)?;
        let lock = worker_lock(&self.cas_root, &worker, "queue")?;
        let start_fence = task_ids
            .as_ref()
            .map(|_| worker_lock(&self.cas_root, &worker, "start"))
            .transpose()?;
        let mut queue = queues.get(session, &worker.name).map_err(error)?;
        let changed = task_ids.is_some();
        let mut source_notices = String::new();
        let mut source_admissions = Vec::new();
        let mut checked_sources = std::collections::HashSet::new();
        if task_ids.as_ref().is_some_and(|ids| !ids.is_empty()) {
            ensure_owned_queue_checkout(&worker)?;
        }
        if let Some(ids) = task_ids {
            if expected.is_some_and(|expected| expected != queue.revision) {
                return Err(error(
                    "Worker task queue revision changed; inspect it before replacing",
                ));
            }
            let mut unique = std::collections::HashSet::new();
            let tasks = self.open_task_store()?;
            for id in &ids {
                if id.trim().is_empty() || !unique.insert(id.clone()) {
                    return Err(error(
                        "Worker task queue requires nonempty, distinct task IDs",
                    ));
                }
                let task = tasks.get(id).map_err(error)?;
                if let Some(key) = task.external_ref.as_deref().and_then(crate::cloud::peer_claims::github_source_key)
                    && !checked_sources.insert(key) { continue; }
                let admission = self.task_source_admission_for(&task, None).await?;
                source_notices.push_str(&admission.notice);
                source_admissions.push(admission);
            }
            if queue.current_task.is_none() {
                queue.current_task = tasks
                    .list(Some(TaskStatus::InProgress))
                    .map_err(error)?
                    .into_iter()
                    .find(|task| {
                        task.assignee
                            .as_deref()
                            .is_some_and(|a| a == worker.name || a == worker.id)
                    })
                    .map(|task| task.id);
            }
            if queue.current_task.as_deref().is_some_and(|id| {
                tasks
                    .get(id)
                    .is_ok_and(|task| task.status == TaskStatus::InProgress)
            }) {
                queue.current_started = true;
            }
            if ids.iter().any(|id| Some(id) == queue.current_task.as_ref()) {
                return Err(error(
                    "The current task is already running; supply only pending task IDs",
                ));
            }
            queue.pending = ids;
            queues
                .save(session, &worker.name, &mut queue)
                .map_err(error)?;
        }
        let predecessor = queue.current_task.clone();
        let idle = predecessor.as_deref().is_none_or(|id| {
            self.open_task_store()
                .ok()
                .and_then(|tasks| tasks.get(id).ok())
                .is_some_and(|task| {
                    matches!(task.status, TaskStatus::Closed | TaskStatus::AwaitingMerge)
                })
        });
        drop(source_admissions);
        drop(start_fence);
        drop(lock);
        if changed && idle {
            self.advance_worker_task_queue(&worker.name, predecessor.as_deref())
                .await?;
            queue = queues.get(session, &worker.name).map_err(error)?;
        }
        let tasks = self.open_task_store()?;
        Ok(format!("{}{source_notices}", render(&worker, &queue, tasks.as_ref()).map_err(error)?))
    }

    /// Called after either close or park commits, including immutable receipts.
    pub(crate) async fn advance_queue_after_delivery(
        &self,
        id: &str,
    ) -> Result<Option<String>, McpError> {
        let tasks = self.open_task_store()?;
        let task = tasks.get(id).map_err(error)?;
        if !matches!(task.status, TaskStatus::Closed | TaskStatus::AwaitingMerge)
            || task.deliverables.merge_conflicted
        {
            return Ok(None);
        }
        let Some(worker) = task.assignee.as_deref() else {
            return Ok(None);
        };
        let agents = self.open_agent_store()?;
        let Some(registration) = agents
            .list(None)
            .map_err(error)?
            .into_iter()
            .filter(|a| a.role == AgentRole::Worker && (a.name == worker || a.id == worker))
            .max_by_key(|a| a.registered_at)
        else {
            return Ok(None);
        };
        let Some(session) = registration.factory_session.as_deref() else {
            return Ok(None);
        };
        if SqliteWorkerTaskQueueStore::open(&self.cas_root)
            .map_err(error)?
            .get(session, &registration.name)
            .map_err(error)?
            .revision
            == 0
        {
            return Ok(None);
        }
        if agents
            .get_lease(id)
            .map_err(error)?
            .is_some_and(|lease| lease.status == cas_types::LeaseStatus::Active)
        {
            return Ok(None);
        }
        self.advance_worker_task_queue(worker, Some(id)).await
    }

    /// cas-526e: a release returns work to the pool, so it also leaves the
    /// worker's queue. Left as the cursor, queue repair restarted it at once.
    /// A released cursor advances the queue to its next pending task.
    pub(crate) async fn drop_released_from_queue(
        &self,
        id: &str,
        worker: &str,
    ) -> Result<Option<String>, McpError> {
        let Some(registration) = self
            .open_agent_store()?
            .list(None)
            .map_err(error)?
            .into_iter()
            .filter(|a| a.role == AgentRole::Worker && (a.name == worker || a.id == worker))
            .max_by_key(|a| a.registered_at)
        else {
            return Ok(None);
        };
        let Some(session) = registration.factory_session.clone() else {
            return Ok(None);
        };
        let queues = SqliteWorkerTaskQueueStore::open(&self.cas_root).map_err(error)?;
        let lock = wait_for_worker_lock(&self.cas_root, &registration, "queue").await?;
        let mut queue = queues.get(&session, &registration.name).map_err(error)?;
        let current = queue.current_task.as_deref() == Some(id);
        if queue.revision == 0 || (!current && !queue.pending.iter().any(|p| p == id)) {
            return Ok(None);
        }
        queue.pending.retain(|p| p != id);
        if current {
            queue.current_task = None;
            queue.current_started = false;
            queue.starting = false;
            queue.dispatch_from = None;
        }
        let note = format!("Task queue dropped {id}: released");
        queue.notes.push(note.clone());
        queues.save(&session, &registration.name, &mut queue).map_err(error)?;
        drop(lock);
        if !current {
            return Ok(Some(note));
        }
        let advanced = self.advance_worker_task_queue(&registration.name, None).await?;
        Ok(Some(advanced.map_or(note.clone(), |advanced| format!("{note}\n{advanced}"))))
    }

    async fn advance_worker_task_queue(
        &self,
        name: &str,
        predecessor: Option<&str>,
    ) -> Result<Option<String>, McpError> {
        let worker = self.queue_worker(name, false)?;
        self.advance_queue_for_worker(worker, predecessor).await
    }

    // Only authenticated RPCs or the private, session-validated daemon repair
    // below can reach this seam. Never rebind an MCP caller to a worker.
    async fn advance_queue_for_worker(
        &self,
        worker: Agent,
        predecessor: Option<&str>,
    ) -> Result<Option<String>, McpError> {
        let session = worker
            .factory_session
            .as_deref()
            .expect("validated session");
        let _lock = worker_lock(&self.cas_root, &worker, "queue")?;
        let queues = SqliteWorkerTaskQueueStore::open(&self.cas_root).map_err(error)?;
        let mut queue = queues.get(session, &worker.name).map_err(error)?;
        if queue.revision == 0 {
            return Ok(None);
        }
        ensure_owned_queue_checkout(&worker)?;
        let retrying = queue.starting && queue.dispatch_from.as_deref() == predecessor;
        let tasks = self.open_task_store()?;
        let agents = self.open_agent_store()?;
        // A returned predecessor can close again while its successor is held.
        // Even an interrupted brief dispatch must keep that execution's slot.
        if queue.current_started
            && queue.current_task.as_deref().is_some_and(|id| {
                tasks.get(id).is_ok_and(|task| {
                    task.status == TaskStatus::Blocked
                        && task
                            .assignee
                            .as_deref()
                            .is_some_and(|a| a == worker.name || a == worker.id)
                })
            })
        {
            return Ok(None);
        }
        let completed_cursor = queue.current_task.as_deref().is_some_and(|id| {
            tasks.get(id).ok().is_some_and(|task| {
                matches!(task.status, TaskStatus::Closed | TaskStatus::AwaitingMerge)
                    && !task.deliverables.merge_conflicted
            }) && agents
                .get_lease(id)
                .ok()
                .flatten()
                .is_none_or(|lease| lease.status != cas_types::LeaseStatus::Active)
        });
        if !retrying && queue.current_task.as_deref() != predecessor && !completed_cursor {
            return Ok(None);
        }
        let start_fence = worker_lock(&self.cas_root, &worker, "start")?;
        // A process can die after the terminal write and before close releases
        // its lease. Repair only the exact owning generation of terminal work.
        if let Some(previous) = predecessor
            && tasks.get(previous).is_ok_and(|task| {
                matches!(task.status, TaskStatus::Closed | TaskStatus::AwaitingMerge)
                    && !task.deliverables.merge_conflicted
            })
            && let Some(lease) = agents.get_lease(previous).map_err(error)?
            && lease.agent_id == worker.id
            && lease.status == cas_types::LeaseStatus::Active
        {
            agents
                .release_lease_if_owner_epoch(
                    previous,
                    &worker.id,
                    lease.epoch,
                    "Queue repair after completed delivery",
                )
                .map_err(error)?;
        }
        if tasks
            .list(Some(TaskStatus::InProgress))
            .map_err(error)?
            .iter()
            .any(|task| {
                Some(task.id.as_str()) != queue.current_task.as_deref()
                    && task
                        .assignee
                        .as_deref()
                        .is_some_and(|a| a == worker.name || a == worker.id)
            })
            || agents
                .list_active_leases()
                .map_err(error)?
                .iter()
                .any(|lease| {
                    lease.agent_id == worker.id
                        && Some(lease.task_id.as_str()) != queue.current_task.as_deref()
                })
        {
            return Ok(Some(format!(
                "Task queue waiting: {} still has active work",
                worker.name
            )));
        }

        let selected = if retrying {
            queue.current_task.clone()
        } else {
            let ready = tasks.list_ready().map_err(error)?;
            let mut selected = None;
            while !queue.pending.is_empty() {
                let id = queue.pending.remove(0);
                let reason = match tasks.get(&id) {
                    Err(
                        cas_store::StoreError::NotFound(_) | cas_store::StoreError::TaskNotFound(_),
                    ) => Some("task missing".to_string()),
                    Err(e) => return Err(error(e)),
                    Ok(task) if task.status != TaskStatus::Open => {
                        Some(format!("status {}", task.status))
                    }
                    Ok(task)
                        if task
                            .assignee
                            .as_deref()
                            .is_some_and(|a| a != worker.name && a != worker.id) =>
                    {
                        Some("assigned elsewhere".into())
                    }
                    Ok(_)
                        if agents.get_lease(&id).map_err(error)?.is_some_and(|lease| {
                            lease.status == cas_types::LeaseStatus::Active
                                && lease.expires_at > chrono::Utc::now()
                                && lease.agent_id != worker.id
                        }) =>
                    {
                        Some("leased elsewhere".into())
                    }
                    Ok(task) if matches!(task.task_type, TaskType::Epic | TaskType::Gate) => {
                        Some("not worker execution".into())
                    }
                    Ok(_) if !ready.iter().any(|task| task.id == id) => {
                        Some("start blocked".into())
                    }
                    Ok(_) => None,
                };
                if let Some(reason) = reason {
                    let note = format!("Task queue skipped {id}: {reason}");
                    queue.notes.push(note.clone());
                    if let Some(previous) = predecessor {
                        tasks.append_note(previous, &note).map_err(error)?;
                    }
                } else {
                    selected = Some(id);
                    break;
                }
            }
            queue.current_task = selected.clone();
            queue.current_started = false;
            queue.dispatch_from = predecessor.map(str::to_string);
            queue.starting = selected.is_some();
            queue.dispatch_revision = queue.revision + 1;
            // cas-2d41: a worker whose MCP server predates the installed
            // build takes the same boundary restart, so a release reaches it
            // between tasks without an operator recycle.
            queue.context_reset_from = if self.load_config().factory().restart_worker_between_tasks
                || crate::factory_binary_skew::agent_mcp_is_older(&self.cas_root, &worker.name)
            {
                predecessor.map(|_| conversation_generation(&worker))
            } else {
                None
            };
            queue.context_reset_request = None;
            queues
                .save(session, &worker.name, &mut queue)
                .map_err(error)?;
            selected
        };
        let Some(id) = selected else { return Ok(None) };
        let selected_task = match tasks.get(&id) {
            Ok(task) => Some(task),
            Err(cas_store::StoreError::NotFound(_) | cas_store::StoreError::TaskNotFound(_)) => {
                None
            }
            Err(cause) => return Err(error(cause)),
        };
        let foreign_lease = agents.get_lease(&id).map_err(error)?.is_some_and(|lease| {
            lease.status == cas_types::LeaseStatus::Active
                && lease.expires_at > chrono::Utc::now()
                && lease.agent_id != worker.id
        });
        if foreign_lease
            || selected_task.as_ref().is_none_or(|task| {
                !matches!(task.status, TaskStatus::Open | TaskStatus::InProgress)
                    || task
                        .assignee
                        .as_deref()
                        .is_some_and(|a| a != worker.name && a != worker.id)
            })
        {
            let note = format!("Task queue skipped {id}: changed ownership or status before start");
            queue.notes.push(note.clone());
            if let Some(previous) = predecessor {
                tasks.append_note(previous, &note).map_err(error)?;
            }
            queue.current_task = predecessor.map(str::to_string);
            queue.starting = false;
            queues
                .save(session, &worker.name, &mut queue)
                .map_err(error)?;
            drop(start_fence);
            drop(_lock);
            return Box::pin(self.advance_queue_for_worker(worker, predecessor)).await;
        }
        // Opted-in task boundaries are conversation boundaries. Persist the old
        // generation before requesting the existing daemon recycle lifecycle;
        // never enqueue B's assignment or switch its branch in A's context.
        if queue.context_reset_from.as_deref() == Some(conversation_generation(&worker).as_str()) {
            let spawns = crate::store::open_spawn_queue_store(&self.cas_root).map_err(error)?;
            // An operator's pending shutdown wins over automatic queue work.
            if spawns.peek(100).map_err(error)?.iter().any(|request| {
                request.action == cas_store::SpawnAction::Shutdown
                    && request.factory_session.as_deref().is_none_or(|owner| owner == session)
                    && (request.worker_names.is_empty() || request.worker_names.contains(&worker.name))
            }) {
                return Ok(Some(format!("Task queue waiting for worker lifecycle before {id}")));
            }
            if queue.context_reset_request.is_none() && !spawns.has_pending_spawn(&worker.name, Some(session)).map_err(error)? {
                let spec = crate::mcp::tools::service::factory_ops::worker_spec_from_agent(&worker);
                cas_factory::validate_explicit(&spec, &cas_factory::CapabilitySnapshot::default()).map_err(error)?;
                let recipe = serde_json::to_string(&spec).map_err(error)?;
                queue.context_reset_request = Some(spawns.enqueue_recycle(&worker.name, Some(&recipe), Some(session)).map_err(error)?);
                queues.save(session, &worker.name, &mut queue).map_err(error)?;
                let _ = cas_factory::notify_daemon(&self.cas_root);
            }
            return Ok(Some(format!("Task queue waiting for fresh context before {id} for {}", worker.name)));
        }
        if queue.context_reset_from.is_some() {
            let spawns = crate::store::open_spawn_queue_store(&self.cas_root).map_err(error)?;
            if spawns.has_pending_spawn(&worker.name, Some(session)).map_err(error)? {
                return Ok(Some(format!("Task queue waiting for context recycle completion before {id}")));
            }
        }
        let mut reserved_epoch = None;
        let result = async {
            let mut task = tasks.get(&id).map_err(error)?;
            if task
                .assignee
                .as_deref()
                .is_some_and(|a| a != worker.name && a != worker.id)
                || !matches!(task.status, TaskStatus::Open | TaskStatus::InProgress)
            {
                return Err(error(
                    "Selected task changed ownership or status; no start was applied",
                ));
            }
            // Reserve ownership atomically before assignment or Git. A
            // concurrent claimant must never leave us stealing its assignee.
            match agents
                .try_claim(
                    &id,
                    &worker.id,
                    i64::from(self.load_config().lease().default_duration_mins) * 60,
                    Some("Queued task start reservation"),
                )
                .map_err(error)?
            {
                cas_types::ClaimResult::Success(lease) => reserved_epoch = Some(lease.epoch),
                cas_types::ClaimResult::AlreadyClaimed { held_by, .. } if held_by == worker.id => {}
                other => return Err(error(format!("Queue reservation refused: {other:?}"))),
            }
            if task.status == TaskStatus::Open
                && task.assignee.as_deref() != Some(worker.name.as_str())
            {
                let expected = task.clone();
                let assignee = self
                    .validate_task_assignee(
                        tasks.as_ref(),
                        &task,
                        &worker.name,
                        TaskStatus::Open,
                        false,
                        None,
                        &mut None,
                        &mut Vec::new(),
                    )
                    .map_err(error)?;
                task.assignee = Some(assignee);
                tasks.update_if_current(&task, &expected).map_err(error)?;
            }
            let started = task.status == TaskStatus::InProgress
                && task.assignee.as_deref() == Some(worker.name.as_str())
                && agents.get_lease(&id).map_err(error)?.is_some_and(|lease| {
                    lease.agent_id == worker.id && lease.status == cas_types::LeaseStatus::Active
                });
            if started {
                return Ok(());
            }
            let output = self
                .task_start_for_queue(
                    TaskStartRequest {
                        id: id.clone(),
                        brief: Some(true),
                        force: None,
                    },
                    QueueStartPrincipal(worker.clone(), start_fence),
                )
                .await?;
            let committed = tasks.get(&id).map_err(error)?;
            if output.is_error.unwrap_or(false)
                || committed.status != TaskStatus::InProgress
                || committed.assignee.as_deref() != Some(worker.name.as_str())
            {
                return Err(error(
                    "Queued start did not commit; inspect the task before retrying",
                ));
            }
            Ok(())
        }
        .await;
        if result.is_err()
            && let Some(epoch) = reserved_epoch
            && tasks
                .get(&id)
                .is_ok_and(|task| task.status != TaskStatus::InProgress)
        {
            agents
                .release_lease_if_owner_epoch(&id, &worker.id, epoch, "Queued start did not commit")
                .map_err(error)?;
        }
        match result {
            Ok(()) => {
                queue.current_started = true;
                // Persist the executed cursor even if its brief cannot be queued.
                queues
                    .save(session, &worker.name, &mut queue)
                    .map_err(error)?;
                let notification = crate::task_assignment::enqueue_queued(
                    &self.cas_root,
                    &tasks.get(&id).map_err(error)?,
                    queue.dispatch_revision,
                    queue.dispatch_from.is_none(),
                )
                .map_err(error)?;
                if notification.is_none() {
                    let note = format!(
                        "Task queue deferred {id}: worker brief not queued; registration repair will retry"
                    );
                    if queue.notes.last() != Some(&note) {
                        queue.notes.push(note.clone());
                        queues
                            .save(session, &worker.name, &mut queue)
                            .map_err(error)?;
                    }
                    return Ok(Some(note));
                }
                queue.starting = false;
                queues
                    .save(session, &worker.name, &mut queue)
                    .map_err(error)?;
                Ok(Some(format!("Task queue started {id} for {}", worker.name)))
            }
            Err(cause) => {
                let note = format!("Task queue deferred {id}: {}", cause.message);
                if queue.notes.last() != Some(&note) {
                    queue.notes.push(note.clone());
                    queues
                        .save(session, &worker.name, &mut queue)
                        .map_err(error)?;
                    if let Some(previous) = predecessor {
                        tasks.append_note(previous, &note).map_err(error)?;
                    }
                }
                Ok(Some(note))
            }
        }
    }
}

pub(crate) fn render(
    worker: &Agent,
    queue: &WorkerTaskQueue,
    tasks: &dyn cas_store::TaskStore,
) -> cas_store::Result<String> {
    // The cursor retains held work for dispatch recovery; execution and holds
    // come from current task state, without rewriting that durable position.
    let owns = |task: &cas_types::Task| task.assignee.as_deref()
        .is_some_and(|assignee| assignee == worker.name || assignee == worker.id);
    let current = tasks.list(Some(TaskStatus::InProgress))?.into_iter()
        .find(|task| owns(task)).map(|task| task.id).unwrap_or_else(|| "none".into());
    let mut held = tasks.list(Some(TaskStatus::Blocked))?.into_iter()
        .filter(|task| owns(task)).map(|task| task.id).collect::<Vec<_>>();
    held.sort();
    let held = if held.is_empty() { "none".into() } else { held.join(", ") };
    let pending = if queue.pending.is_empty() {
        "none".into()
    } else {
        queue.pending.join(" -> ")
    };
    let mut output = format!(
        "Task queue for {} (revision {})\nCurrent: {current}\nHeld: {held}\nPending: {pending}",
        worker.name, queue.revision
    );
    if queue.starting && queue.context_reset_from.is_some() {
        output.push_str(&format!("\nContext reset: waiting for fresh conversation (request {})",
            queue.context_reset_request.map(|id| id.to_string()).unwrap_or_else(|| "pending".into())));
    }
    if let Some(note) = queue.notes.last() {
        output.push_str(&format!("\n{note}"));
    }
    Ok(output)
}

/// Recover durable cursors after lock contention, process loss or an invalid
/// spawn head. Called only by the factory daemon on its external job lane.
/// This core has no model identity: the sealed principal supplies only start's
/// worker identity after validating the live session and its owning supervisor.
pub(crate) async fn reconcile_worker_task_queues(
    root: &Path,
    session: &str,
) -> Result<usize, McpError> {
    let core = CasCore::with_daemon(root.to_path_buf(), None, None);
    let agents = core.open_agent_store()?;
    let Some(owner) =
        crate::mcp::tools::core::task::lifecycle::supervisor_push::resolve_worker_supervisor(
            agents.as_ref(),
            Some(session),
        )
    else {
        return Ok(0);
    };
    let supervisor = agents.get(&owner.agent_id).map_err(error)?;
    if supervisor.factory_session.as_deref() != Some(session)
        || !supervisor.is_alive()
        || (chrono::Utc::now() - supervisor.last_heartbeat).num_seconds()
            > cas_types::DEFAULT_HEARTBEAT_TIMEOUT_SECS
    {
        return Ok(0);
    }
    let tasks = core.open_task_store()?;
    let queues = SqliteWorkerTaskQueueStore::open(root).map_err(error)?;
    let mut repaired = 0;
    let mut registrations = std::collections::HashMap::<String, Agent>::new();
    for agent in agents.list(None).map_err(error)? {
        if agent.factory_session.as_deref() == Some(session) {
            let entry = registrations
                .entry(agent.name.clone())
                .or_insert_with(|| agent.clone());
            if agent.registered_at > entry.registered_at {
                *entry = agent;
            }
        }
    }
    for worker in registrations.into_values().filter(|a| {
        a.role == AgentRole::Worker
            && a.factory_session.as_deref() == Some(session)
            && a.is_alive()
            && (chrono::Utc::now() - a.last_heartbeat).num_seconds()
                <= cas_types::DEFAULT_HEARTBEAT_TIMEOUT_SECS
    }) {
        let lock = match worker_lock(root, &worker, "queue") {
            Ok(lock) => lock,
            Err(_) => continue, // The durable cursor stays due on the next pass.
        };
        let mut queue = queues.get(session, &worker.name).map_err(error)?;
        if queue.revision == 0 {
            continue;
        }
        // Started work can yield its lease while blocked. Preserve that
        // execution; only an unstarted spawn head is eligible for skip repair.
        if queue.current_started
            && queue.current_task.as_deref().is_some_and(|id| {
                tasks.get(id).is_ok_and(|task| {
                    task.assignee
                        .as_deref()
                        .is_some_and(|a| a == worker.name || a == worker.id)
                        && !task.is_terminal()
                        && !(matches!(task.status, TaskStatus::Closed | TaskStatus::AwaitingMerge)
                            && !task.deliverables.merge_conflicted)
                        && !(queue.starting && task.status == TaskStatus::InProgress)
                })
            })
        {
            continue;
        }
        let predecessor = if queue.starting {
            queue.dispatch_from.clone()
        } else if let Some(id) = queue.current_task.clone() {
            let task = tasks.get(&id).ok();
            if task.as_ref().is_some_and(|task| {
                task.status == TaskStatus::InProgress
                    && task
                        .assignee
                        .as_deref()
                        .is_some_and(|a| a == worker.name || a == worker.id)
            }) {
                if !queue.current_started {
                    queue.current_started = true;
                    queues
                        .save(session, &worker.name, &mut queue)
                        .map_err(error)?;
                }
                continue;
            }
            if task.as_ref().is_some_and(|task| {
                matches!(task.status, TaskStatus::Closed | TaskStatus::AwaitingMerge)
                    && !task.deliverables.merge_conflicted
            }) {
                Some(id)
            } else {
                // A spawn head may have become blocked/reassigned before
                // registration. Re-select it with the ordinary skip policy.
                queue.pending.insert(0, id);
                queue.current_task = None;
                queue.current_started = false;
                queues
                    .save(session, &worker.name, &mut queue)
                    .map_err(error)?;
                None
            }
        } else if queue.pending.is_empty() {
            continue;
        } else {
            None
        };
        drop(lock);
        match core
            .advance_queue_for_worker(worker, predecessor.as_deref())
            .await
        {
            Ok(Some(_)) => repaired += 1,
            Ok(None) => {}
            Err(cause) => {
                tracing::debug!(%cause, "worker queue repair deferred; durable cursor remains due")
            }
        }
    }
    Ok(repaired)
}
