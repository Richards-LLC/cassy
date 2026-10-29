//! Queue policy and dispatch. Task start remains the owner of execution policy.
use std::{fs::File, path::Path};

use cas_store::worker_task_queue_store::{SqliteWorkerTaskQueueStore, WorkerTaskQueue};
use cas_types::{Agent, AgentRole, TaskStatus, TaskType};
use fs2::FileExt;
use rmcp::{ErrorData as McpError, model::ErrorCode};

use crate::{mcp::CasCore, mcp::tools::TaskStartRequest};

fn error(message: impl std::fmt::Display) -> McpError {
    CasCore::error(ErrorCode::INVALID_PARAMS, message.to_string())
}

/// File locks fence Git and queue mutations across separate MCP processes.
pub(crate) fn worker_lock(root: &Path, worker: &Agent, operation: &str) -> Result<File, McpError> {
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
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(format!("worker-{key}.lock")))
        .map_err(error)?;
    file.try_lock_exclusive().map_err(|_| {
        error(format!(
            "Worker {} has a {operation} operation in progress; retry after it finishes",
            worker.name
        ))
    })?;
    Ok(file)
}

/// Only this module can construct a start principal on behalf of a worker.
pub(crate) struct QueueStartPrincipal(Agent, File);
impl QueueStartPrincipal {
    pub(crate) fn agent(&self) -> &Agent {
        &self.0
    }
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
                tasks.get(id).map_err(error)?;
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
        drop(start_fence);
        drop(lock);
        if changed && idle {
            self.advance_worker_task_queue(&worker.name, predecessor.as_deref())
                .await?;
            queue = queues.get(session, &worker.name).map_err(error)?;
        }
        Ok(render(&worker.name, &queue))
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

    async fn advance_worker_task_queue(
        &self,
        name: &str,
        predecessor: Option<&str>,
    ) -> Result<Option<String>, McpError> {
        let worker = self.queue_worker(name, false)?;
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
        let retrying = queue.starting && queue.dispatch_from.as_deref() == predecessor;
        let tasks = self.open_task_store()?;
        let agents = self.open_agent_store()?;
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
            queue.dispatch_from = predecessor.map(str::to_string);
            queue.starting = selected.is_some();
            queue.dispatch_revision = queue.revision + 1;
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
        if selected_task.as_ref().is_none_or(|task| {
            !matches!(task.status, TaskStatus::Open | TaskStatus::InProgress)
                || task
                    .assignee
                    .as_deref()
                    .is_some_and(|a| a != worker.name && a != worker.id)
        }) {
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
            return Box::pin(self.advance_worker_task_queue(&worker.name, predecessor)).await;
        }
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
            if task.status == TaskStatus::Open {
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
        match result {
            Ok(()) => {
                crate::task_assignment::enqueue_queued(
                    &self.cas_root,
                    &tasks.get(&id).map_err(error)?,
                    queue.dispatch_revision,
                )
                .map_err(error)?;
                queue.starting = false;
                queues
                    .save(session, &worker.name, &mut queue)
                    .map_err(error)?;
                Ok(Some(format!("Task queue started {id} for {}", worker.name)))
            }
            Err(cause) => {
                let note = format!("Task queue deferred {id}: {}", cause.message);
                queue.notes.push(note.clone());
                queues
                    .save(session, &worker.name, &mut queue)
                    .map_err(error)?;
                if let Some(previous) = predecessor {
                    tasks.append_note(previous, &note).map_err(error)?;
                }
                Ok(Some(note))
            }
        }
    }
}

pub(crate) fn render(worker: &str, queue: &WorkerTaskQueue) -> String {
    let current = queue.current_task.as_deref().unwrap_or("none");
    let pending = if queue.pending.is_empty() {
        "none".into()
    } else {
        queue.pending.join(" -> ")
    };
    let mut output = format!(
        "Task queue for {worker} (revision {})\nCurrent: {current}\nPending: {pending}",
        queue.revision
    );
    if let Some(note) = queue.notes.last() {
        output.push_str(&format!("\n{note}"));
    }
    output
}
