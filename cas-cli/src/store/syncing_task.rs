//! Syncing task store wrapper
//!
//! Automatically queues task changes for cloud sync on add/update/delete.
//! When a team is configured and the task passes the T1 filter policy,
//! the write is dual-enqueued to both the personal queue and the team queue.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::cloud::{
    CloudConfig, EntityType, SyncOperation, SyncQueue, TaskSyncFulfillMode, TaskSyncFulfillResult,
    TaskSyncIntent, TaskSyncPayload,
};
use crate::error::CasError;
use crate::store::share_policy::{eligible_for_team_task, resolve_team_id};
use crate::store::{Result, StoreError, TaskStore};
use crate::types::{Dependency, DependencyType, Scope, Task, TaskStatus};
use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(serde::Serialize)]
struct TaskDependencyPayload<'a> {
    from_id: &'a str,
    to_id: &'a str,
    dep_type: String,
    created_at: DateTime<Utc>,
    origin_project: Option<&'a str>,
}

/// After a clean scheduled reconcile, the next one waits this long. It is the
/// safety net for intents a crashed process left behind; a mutator fulfills
/// its own intent.
const RECONCILE_PERIOD: Duration = Duration::from_secs(60);
/// After a deferred or failed scheduled reconcile, the next one waits this long.
const RECONCILE_RETRY: Duration = Duration::from_secs(5);

/// Next due time of the scheduled reconcile, per process and `.cas` directory.
fn reconcile_schedule() -> std::sync::MutexGuard<'static, HashMap<PathBuf, Instant>> {
    static SCHEDULE: OnceLock<Mutex<HashMap<PathBuf, Instant>>> = OnceLock::new();
    SCHEDULE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Outcome of one reconcile pass over the pending task sync intents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskSyncReconcileOutcome {
    /// No intent was left for a later pass.
    Clean,
    /// A live mutation owned an entity, or the database stayed write-busy:
    /// the remaining intents stay durable for a later pass.
    Deferred,
}

/// A task store wrapper that queues changes for cloud sync
pub struct SyncingTaskStore {
    inner: Arc<dyn TaskStore>,
    queue: Arc<SyncQueue>,
    /// Pre-resolved team UUID for dual-enqueue; see
    /// `SyncingEntryStore::team_id` for the protocol. `None` preserves
    /// personal-only behaviour.
    team_id: Option<Arc<str>>,
    team_only: bool,
}

impl SyncingTaskStore {
    /// Create a new syncing task store (personal queue only).
    pub fn new(inner: Arc<dyn TaskStore>, queue: Arc<SyncQueue>) -> Self {
        Self {
            inner,
            queue,
            team_id: None,
            team_only: false,
        }
    }

    /// Attach a cloud config for team auto-promotion. See
    /// `SyncingEntryStore::with_cloud_config` for the protocol.
    #[must_use]
    pub fn with_cloud_config(mut self, cloud_config: Arc<CloudConfig>) -> Self {
        self.team_id = resolve_team_id(&cloud_config);
        self.team_only = cloud_config.team_only && self.team_id.is_some();
        self
    }

    fn stage_upsert(
        &self,
        task: &Task,
        operation: &str,
        previous_updated_at: Option<&str>,
        previous: Option<&Task>,
    ) -> Result<TaskSyncIntent> {
        let team_id = self
            .team_id
            .as_deref()
            .filter(|_| eligible_for_team_task(task));
        let previous_team_id = previous
            .filter(|task| eligible_for_team_task(task))
            .and(self.team_id.as_deref());
        let previous_project_id = previous
            .and_then(|task| task.origin_project.as_deref())
            .filter(|project_id| !project_id.trim().is_empty());
        self.queue
            .stage_task_sync_intent(
                &task.id,
                operation,
                previous_updated_at,
                team_id,
                previous_team_id,
                previous_project_id,
                task.scope == Scope::Global,
            )
            .map_err(queue_error_before_local_commit)
    }

    fn load_canonical_sync_payload(&self, intent: &TaskSyncIntent) -> Result<TaskSyncPayload> {
        // This is the only callback run while SyncQueue holds its queue mutex
        // and BEGIN IMMEDIATE transaction. It is intentionally read-only: one
        // inner.get plus in-memory policy/serialization. It cannot reopen a
        // syncing store, acquire the mutation flock, or call back into queue.
        let mut task = self.inner.get(&intent.entity_id)?;
        task.scope = if intent.global_scope {
            Scope::Global
        } else {
            Scope::Project
        };
        if task.scope == Scope::Global {
            task.origin_project = None;
        }
        let current_project_id = task
            .origin_project
            .as_deref()
            .filter(|project_id| !project_id.trim().is_empty())
            .map(ToOwned::to_owned);
        let current_team_id = self
            .team_id
            .as_deref()
            .filter(|_| eligible_for_team_task(&task))
            .map(ToOwned::to_owned);
        let payload = serde_json::to_string(&task)?;
        Ok(TaskSyncPayload {
            payload,
            current_project_id,
            current_team_id,
            personal: !self.team_only || task.scope == Scope::Global,
        })
    }

    fn fulfill_upsert(&self, intent: &TaskSyncIntent) -> Result<TaskSyncFulfillResult> {
        self.fulfill_upsert_after_validation(intent, TaskSyncFulfillMode::Mutation, || {})
    }

    fn fulfill_upsert_after_validation<H>(
        &self,
        intent: &TaskSyncIntent,
        mode: TaskSyncFulfillMode,
        after_validation: H,
    ) -> Result<TaskSyncFulfillResult>
    where
        H: FnOnce(),
    {
        self.queue
            .fulfill_task_sync_intent(intent, mode, after_validation, || {
                self.load_canonical_sync_payload(intent)
                    .map_err(|error| CasError::Other(error.to_string()))
            })
            .map_err(|error| degraded_sync_error(intent, error.to_string()))
    }

    fn cancel_staged_after_local_failure(&self, intent: &TaskSyncIntent) {
        // The local error remains authoritative. If cancellation itself fails,
        // restart reconciliation reloads the canonical row (or discards the
        // marker when the add never created one), so retaining the marker is
        // safe and preferable to masking the real local failure.
        let _ = self.queue.cancel_task_sync_intent(intent.id);
    }

    /// Fulfill or retire every pending task sync intent that no live mutation
    /// owns. Never waits on a lock and bounds each SQLite write wait
    /// (GH #1165): an owned entity or a write-busy database defers the rest of
    /// the pass, leaving the intents durable.
    pub(crate) fn reconcile_pending_task_sync(&self) -> Result<TaskSyncReconcileOutcome> {
        // Lock-free first read: the common case is nothing to reconcile.
        let intents = self
            .queue
            .pending_task_sync_intents()
            .map_err(queue_error_before_local_commit)?;
        if intents.is_empty() {
            return Ok(TaskSyncReconcileOutcome::Clean);
        }
        let mut by_entity = std::collections::BTreeMap::<String, Vec<TaskSyncIntent>>::new();
        for intent in intents {
            by_entity
                .entry(intent.entity_id.clone())
                .or_default()
                .push(intent);
        }
        for (entity_id, mut intents) in by_entity {
            match self.inner.get(&entity_id) {
                Ok(_) => {}
                Err(StoreError::TaskNotFound(_)) | Err(StoreError::NotFound(_)) => {
                    // Revision/receipt evidence, not row absence alone,
                    // decides whether this is pre-commit or superseded.
                }
                Err(error) => return Err(error),
            }
            while let Some(intent) = intents.pop() {
                match self.fulfill_upsert_after_validation(
                    &intent,
                    TaskSyncFulfillMode::Reconcile,
                    || {},
                )? {
                    TaskSyncFulfillResult::ProvenPreCommit => continue,
                    TaskSyncFulfillResult::Fulfilled | TaskSyncFulfillResult::Superseded => break,
                    TaskSyncFulfillResult::Deferred => {
                        return Ok(TaskSyncReconcileOutcome::Deferred);
                    }
                }
            }
        }
        Ok(TaskSyncReconcileOutcome::Clean)
    }

    /// The task-store open path's reconcile: once per process and `.cas`
    /// directory, then again only when the schedule is due
    /// ([`RECONCILE_PERIOD`] after a clean pass, [`RECONCILE_RETRY`] after a
    /// deferred or failed one). Opens are read paths, so a failure is logged
    /// and the intents stay durable for the next pass; explicit `init`
    /// still reports it.
    pub(crate) fn reconcile_if_due(&self) {
        let cas_dir = self.queue.cas_dir().to_path_buf();
        let now = Instant::now();
        {
            let mut schedule = reconcile_schedule();
            if schedule.get(&cas_dir).is_some_and(|due| now < *due) {
                return;
            }
            // Claim this pass so concurrent opens do not repeat it.
            schedule.insert(cas_dir.clone(), now + RECONCILE_RETRY);
        }
        match self.reconcile_pending_task_sync() {
            Ok(outcome) => record_reconcile(cas_dir, outcome),
            Err(error) => {
                record_reconcile(cas_dir, TaskSyncReconcileOutcome::Deferred);
                tracing::warn!(%error, "task sync reconcile failed; pending intents retained for retry");
            }
        }
    }

    fn persisted_for_queue(&self, task: &Task) -> Result<Task> {
        let mut persisted = self.inner.get(&task.id)?;

        // The task table is project-scoped storage and therefore does not
        // persist the wire-level scope field. Keep the caller's scope for the
        // queue decision: a Global task must remain personal-only after the
        // round trip through the inner store. Global tasks also have no
        // project identity, so do not attach the inner store's identity to
        // their queued payload.
        persisted.scope = task.scope;
        if task.scope == Scope::Global {
            persisted.origin_project = None;
        }

        Ok(persisted)
    }

    fn queue_delete(&self, id: &str, project_id: Option<&str>, scope: Scope) {
        if !self.team_only || scope == Scope::Global {
            let _ = self
                .queue
                .enqueue(EntityType::Task, id, SyncOperation::Delete, None);
        } else {
            let _ = self
                .queue
                .drop_personal_queued_push_for(EntityType::Task, id);
        }

        // See `share_policy` module docs: delete fans out unconditionally
        // when a team is configured.
        if let Some(team_id) = self.team_id.as_deref()
            && (!self.team_only || scope == Scope::Project)
        {
            let _ = self.queue.enqueue_for_team_project(
                EntityType::Task,
                id,
                SyncOperation::Delete,
                None,
                team_id,
                project_id,
            );
        }
    }

    fn queue_dependency_upsert(&self, dep: &Dependency, from_task: &Task) {
        if from_task.scope == Scope::Project {
            let local = self.inner.project_id();
            let to = self.inner.get(&dep.to_id).ok();
            let authored_here = local.is_none_or(|local| {
                [Some(from_task), to.as_ref()].into_iter().all(|task| {
                    task.is_some_and(|task| {
                        task.origin_project.as_deref().is_none_or(|origin| {
                            origin != "unknown" && crate::cloud::project_ids_match(origin, local)
                        })
                    })
                })
            });
            if !authored_here {
                return;
            }
        }
        let origin_project = match from_task.scope {
            Scope::Global => None,
            Scope::Project => from_task.origin_project.as_deref(),
        };
        let payload = TaskDependencyPayload {
            from_id: &dep.from_id,
            to_id: &dep.to_id,
            dep_type: dep.dep_type.to_string(),
            created_at: dep.created_at,
            origin_project,
        };
        let Ok(payload) = serde_json::to_string(&payload) else {
            return;
        };
        let entity_id = dependency_entity_id(dep);
        if !self.team_only || from_task.scope == Scope::Global {
            let _ = self.queue.enqueue(
                EntityType::TaskDependency,
                &entity_id,
                SyncOperation::Upsert,
                Some(&payload),
            );
        } else {
            let _ = self
                .queue
                .drop_personal_queued_push_for(EntityType::TaskDependency, &entity_id);
        }

        if let Some(team_id) = self.team_id.as_deref()
            && eligible_for_team_task(from_task)
        {
            let _ = self.queue.enqueue_for_team(
                EntityType::TaskDependency,
                &entity_id,
                SyncOperation::Upsert,
                Some(&payload),
                team_id,
            );
        }
    }

    fn queue_dependency_delete(&self, dep: &Dependency, scope: Scope) {
        let entity_id = dependency_entity_id(dep);
        if !self.team_only || scope == Scope::Global {
            let _ = self.queue.enqueue(
                EntityType::TaskDependency,
                &entity_id,
                SyncOperation::Delete,
                None,
            );
        } else {
            let _ = self
                .queue
                .drop_personal_queued_push_for(EntityType::TaskDependency, &entity_id);
        }

        // A delete must fan out when a team is configured, even when the
        // source task is no longer available to evaluate the promotion
        // predicate. This prevents stale cloud edges from surviving local
        // task/dependency deletion.
        if let Some(team_id) = self.team_id.as_deref()
            && (!self.team_only || scope == Scope::Project)
        {
            let _ = self.queue.enqueue_for_team(
                EntityType::TaskDependency,
                &entity_id,
                SyncOperation::Delete,
                None,
                team_id,
            );
        }
    }
}

fn record_reconcile(cas_dir: PathBuf, outcome: TaskSyncReconcileOutcome) {
    let delay = match outcome {
        TaskSyncReconcileOutcome::Clean => RECONCILE_PERIOD,
        TaskSyncReconcileOutcome::Deferred => RECONCILE_RETRY,
    };
    reconcile_schedule().insert(cas_dir, Instant::now() + delay);
}

fn dependency_entity_id(dep: &Dependency) -> String {
    format!("{}:{}:{}", dep.from_id, dep.to_id, dep.dep_type)
}

fn queue_error_before_local_commit(error: CasError) -> StoreError {
    match error {
        CasError::Database(error) => StoreError::Database(error),
        CasError::Io(error) => StoreError::Io(error),
        CasError::Json(error) => StoreError::Json(error),
        error => StoreError::Other(error.to_string()),
    }
}

fn degraded_sync_error(intent: &TaskSyncIntent, reason: String) -> StoreError {
    StoreError::SyncDegradedAfterCommit {
        entity_type: "task".to_string(),
        entity_id: intent.entity_id.clone(),
        operation: intent.operation.clone(),
        reason,
    }
}

impl TaskStore for SyncingTaskStore {
    fn init(&self) -> Result<()> {
        self.inner.init()?;
        self.queue.init().map_err(queue_error_before_local_commit)?;
        let cas_dir = self.queue.cas_dir().to_path_buf();
        match self.reconcile_pending_task_sync() {
            Ok(outcome) => {
                record_reconcile(cas_dir, outcome);
                Ok(())
            }
            Err(error) => {
                record_reconcile(cas_dir, TaskSyncReconcileOutcome::Deferred);
                Err(error)
            }
        }
    }

    fn generate_id(&self) -> Result<String> {
        self.inner.generate_id()
    }

    fn project_id(&self) -> Option<&str> {
        self.inner.project_id()
    }

    fn add(&self, task: &Task) -> Result<()> {
        let _sync_guard = self
            .queue
            .lock_task_sync_mutations(&[&task.id])
            .map_err(queue_error_before_local_commit)?;
        let intent = self.stage_upsert(task, "add", None, None)?;
        if let Err(error) = self
            .inner
            .add_with_mutation_receipt(task, &intent.mutation_id)
        {
            self.cancel_staged_after_local_failure(&intent);
            return Err(error);
        }
        crate::mcp::tools::service::mutation_receipt::task_committed(&task.id);
        self.fulfill_upsert(&intent)?;
        Ok(())
    }

    fn create_atomic(
        &self,
        task: &Task,
        blocked_by: &[String],
        epic_id: Option<&str>,
        created_by: Option<&str>,
    ) -> Result<()> {
        let _sync_guard = self
            .queue
            .lock_task_sync_mutations(&[&task.id])
            .map_err(queue_error_before_local_commit)?;
        let intent = self.stage_upsert(task, "create_atomic", None, None)?;
        if let Err(error) = self.inner.create_atomic_with_mutation_receipt(
            task,
            blocked_by,
            epic_id,
            created_by,
            &intent.mutation_id,
        ) {
            self.cancel_staged_after_local_failure(&intent);
            return Err(error);
        }
        crate::mcp::tools::service::mutation_receipt::task_committed(&task.id);
        let task_sync_result = self.fulfill_upsert(&intent);
        let persisted = self
            .persisted_for_queue(task)
            .map_err(|error| degraded_sync_error(&intent, error.to_string()))?;
        for dep in self.inner.get_dependencies(&task.id)? {
            self.queue_dependency_upsert(&dep, &persisted);
        }
        task_sync_result?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Task> {
        self.inner.get(id)
    }

    fn get_execution_state(&self, task_id: &str) -> Result<Option<Value>> {
        self.inner.get_execution_state(task_id)
    }

    fn patch_execution_state(&self, task_id: &str, patch: &Value) -> Result<Value> {
        self.inner.patch_execution_state(task_id, patch)
    }

    fn update(&self, task: &Task) -> Result<DateTime<Utc>> {
        let _sync_guard = self
            .queue
            .lock_task_sync_mutations(&[&task.id])
            .map_err(queue_error_before_local_commit)?;
        let previous = self.inner.get(&task.id)?;
        let previous_updated_at = previous.updated_at.to_rfc3339();
        let intent =
            self.stage_upsert(task, "update", Some(&previous_updated_at), Some(&previous))?;
        let persisted_at = match self
            .inner
            .update_with_mutation_receipt(task, &intent.mutation_id)
        {
            Ok(persisted_at) => persisted_at,
            Err(error) => {
                self.cancel_staged_after_local_failure(&intent);
                return Err(error);
            }
        };
        crate::mcp::tools::service::mutation_receipt::task_committed(&task.id);
        self.fulfill_upsert(&intent)?;
        Ok(persisted_at)
    }

    fn update_from_sync(&self, task: &Task, expected: &Task) -> Result<Option<DateTime<Utc>>> {
        self.inner.update_from_sync(task, expected)
    }

    fn append_note(&self, task_id: &str, formatted_note: &str) -> Result<DateTime<Utc>> {
        let _sync_guard = self
            .queue
            .lock_task_sync_mutations(&[task_id])
            .map_err(queue_error_before_local_commit)?;
        let previous = self.inner.get(task_id)?;
        let previous_updated_at = previous.updated_at.to_rfc3339();
        let intent = self.stage_upsert(
            &previous,
            "append_note",
            Some(&previous_updated_at),
            Some(&previous),
        )?;
        let persisted_at = match self.inner.append_note_with_mutation_receipt(
            task_id,
            formatted_note,
            &intent.mutation_id,
        ) {
            Ok(persisted_at) => persisted_at,
            Err(error) => {
                self.cancel_staged_after_local_failure(&intent);
                return Err(error);
            }
        };
        crate::mcp::tools::service::mutation_receipt::task_committed(task_id);
        self.fulfill_upsert(&intent)?;
        Ok(persisted_at)
    }

    fn delete(&self, id: &str) -> Result<()> {
        let task = self.inner.get(id)?;
        let mut dependencies = self.inner.get_dependencies(id)?;
        for dep in self.inner.get_dependents(id)? {
            if !dependencies.iter().any(|existing| {
                existing.from_id == dep.from_id
                    && existing.to_id == dep.to_id
                    && existing.dep_type == dep.dep_type
            }) {
                dependencies.push(dep);
            }
        }
        self.inner.delete(id)?;
        let project_id = task
            .origin_project
            .as_deref()
            .filter(|project_id| !project_id.trim().is_empty());
        if task.scope == Scope::Project
            && project_id.is_some_and(|origin| {
                origin == "unknown"
                    || self
                        .inner
                        .project_id()
                        .is_some_and(|local| !crate::cloud::project_ids_match(origin, local))
            })
        {
            // A move away may have left an old-owner tombstone in the queue.
            // The sync-intent move path removed stale upserts; deleting this
            // foreign row must leave that tombstone available for push.
            return Ok(());
        }
        self.queue_delete(id, project_id, task.scope);
        for dep in &dependencies {
            self.queue_dependency_delete(dep, task.scope);
        }
        Ok(())
    }

    fn list(&self, status: Option<TaskStatus>) -> Result<Vec<Task>> {
        self.inner.list(status)
    }

    fn list_with_suppressed(&self, status: Option<TaskStatus>) -> Result<(Vec<Task>, Vec<Task>)> {
        self.inner.list_with_suppressed(status)
    }

    fn list_ready(&self) -> Result<Vec<Task>> {
        self.inner.list_ready()
    }

    fn list_blocked(&self) -> Result<Vec<(Task, Vec<Task>)>> {
        self.inner.list_blocked()
    }

    fn list_pending_verification(&self) -> Result<Vec<Task>> {
        self.inner.list_pending_verification()
    }

    fn list_pending_worktree_merge(&self) -> Result<Vec<Task>> {
        self.inner.list_pending_worktree_merge()
    }

    fn close(&self) -> Result<()> {
        self.inner.close()
    }

    // Dependency operations are first-class cloud entities. The local
    // dependency table remains authoritative; queue writes mirror successful
    // local mutations without routing pulled rows back through this wrapper.
    fn add_dependency(&self, dep: &Dependency) -> Result<()> {
        let scope = self.inner.get(&dep.from_id)?.scope;
        let previous = self
            .inner
            .get_dependencies(&dep.from_id)?
            .into_iter()
            .find(|existing| existing.to_id == dep.to_id);
        self.inner.add_dependency(dep)?;
        if let Some(previous) = previous.filter(|previous| previous.dep_type != dep.dep_type) {
            self.queue_dependency_delete(&previous, scope);
        }
        let from_task = self.inner.get(&dep.from_id)?;
        self.queue_dependency_upsert(dep, &from_task);
        Ok(())
    }

    fn remove_dependency(&self, from_id: &str, to_id: &str) -> Result<()> {
        let scope = self
            .inner
            .get(from_id)
            .map(|task| task.scope)
            .unwrap_or(Scope::Project);
        let dependencies: Vec<Dependency> = self
            .inner
            .get_dependencies(from_id)?
            .into_iter()
            .filter(|dep| dep.to_id == to_id)
            .collect();
        self.inner.remove_dependency(from_id, to_id)?;
        for dep in &dependencies {
            self.queue_dependency_delete(dep, scope);
        }
        Ok(())
    }

    fn remove_dependency_of_type(
        &self,
        from_id: &str,
        to_id: &str,
        dep_type: DependencyType,
    ) -> Result<bool> {
        let scope = self
            .inner
            .get(from_id)
            .map(|task| task.scope)
            .unwrap_or(Scope::Project);
        let removed = self
            .inner
            .remove_dependency_of_type(from_id, to_id, dep_type)?;
        if removed {
            self.queue_dependency_delete(
                &Dependency {
                    from_id: from_id.to_string(),
                    to_id: to_id.to_string(),
                    dep_type,
                    created_at: Utc::now(),
                    created_by: None,
                },
                scope,
            );
        }
        Ok(removed)
    }

    fn get_dependencies(&self, task_id: &str) -> Result<Vec<Dependency>> {
        self.inner.get_dependencies(task_id)
    }

    fn get_dependents(&self, task_id: &str) -> Result<Vec<Dependency>> {
        self.inner.get_dependents(task_id)
    }

    fn get_blockers(&self, task_id: &str) -> Result<Vec<Task>> {
        self.inner.get_blockers(task_id)
    }

    fn would_create_cycle(&self, from_id: &str, to_id: &str) -> Result<bool> {
        self.inner.would_create_cycle(from_id, to_id)
    }

    fn list_dependencies(&self, dep_type: Option<DependencyType>) -> Result<Vec<Dependency>> {
        self.inner.list_dependencies(dep_type)
    }

    fn get_subtasks(&self, parent_id: &str) -> Result<Vec<Task>> {
        self.inner.get_subtasks(parent_id)
    }

    fn get_sibling_notes(
        &self,
        epic_id: &str,
        exclude_task_id: &str,
    ) -> Result<Vec<(String, String, String)>> {
        self.inner.get_sibling_notes(epic_id, exclude_task_id)
    }

    fn get_parent_epic(&self, task_id: &str) -> Result<Option<Task>> {
        self.inner.get_parent_epic(task_id)
    }
}

#[cfg(test)]
mod tests {
    use crate::store::mock::MockTaskStore;
    use crate::store::syncing_task::*;
    use crate::store::{SqliteTaskStore, StoreError};
    use fs2::FileExt;
    use std::fs::OpenOptions;
    use std::path::Path;
    use std::sync::{Barrier, mpsc};
    use std::time::Duration;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, SyncingTaskStore) {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path();

        let inner = SqliteTaskStore::open(cas_dir).unwrap();
        inner.init().unwrap();

        let queue = SyncQueue::open(cas_dir).unwrap();
        queue.init().unwrap();

        let store = SyncingTaskStore::new(Arc::new(inner), Arc::new(queue));
        (temp, store)
    }

    /// cas-d5c8 (GH #921): the MCP task write paths (create, update, note,
    /// close, cancel) complete while other agents keep committing writes, the
    /// shape of a 4-6 worker factory session. Before the fix, staging the sync
    /// intent failed with "database is locked" on the first collision.
    #[test]
    fn task_writes_wait_out_a_fleet_of_concurrent_writers() {
        let (temp, store) = create_test_store();
        let db = temp.path().join("cas.db");
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let writers: Vec<_> = (0..5)
            .map(|agent| {
                let db = db.clone();
                let stop = stop.clone();
                let started = started.clone();
                std::thread::spawn(move || {
                    let conn = rusqlite::Connection::open(db).unwrap();
                    let mut commits = 0usize;
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        // A write held for a moment, as a fleet's store writes are.
                        // (A zero-think-time hammer starves any SQLite waiter,
                        // because the busy handler polls; that is not a fleet.)
                        if conn.execute_batch("BEGIN IMMEDIATE").is_err() {
                            continue;
                        }
                        conn.execute(
                            "INSERT INTO task_mutation_revisions (entity_id, revision, present) VALUES (?1, 1, 1)
                             ON CONFLICT(entity_id) DO UPDATE SET revision = revision + 1",
                            rusqlite::params![format!("fleet-agent-{agent}")],
                        )
                        .unwrap();
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        conn.execute_batch("COMMIT").unwrap();
                        commits += 1;
                        started.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        // Then some think time, different per agent so their
                        // writes interleave: together the five keep the
                        // database write-locked roughly 60% of the time.
                        std::thread::sleep(std::time::Duration::from_millis(80 + 40 * agent as u64));
                    }
                    commits
                })
            })
            .collect();

        // Every agent is writing before the task writes begin.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while started.load(std::sync::atomic::Ordering::Relaxed) < 5
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let mut failures = Vec::new();
        for round in 0..6 {
            let id = format!("cas-fleet-{round}");
            let mut task = Task::new(id.clone(), format!("fleet round {round}"));
            // Spread the task writes across the agents' write cycles.
            std::thread::sleep(std::time::Duration::from_millis(25));
            if let Err(error) = store.add(&task) {
                failures.push(format!("create {id}: {error}"));
                continue;
            }
            task.priority = crate::types::Priority::HIGH;
            if let Err(error) = store.update(&task) {
                failures.push(format!("update {id}: {error}"));
            }
            if let Err(error) = store.append_note(&id, "progress under contention") {
                failures.push(format!("note {id}: {error}"));
            }
            task.status = if round % 2 == 0 {
                TaskStatus::Closed
            } else {
                TaskStatus::Cancelled
            };
            if let Err(error) = store.update(&task) {
                failures.push(format!("close/cancel {id}: {error}"));
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let commits: usize = writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .sum();
        assert!(
            commits > 0,
            "the concurrent writers must actually have written"
        );
        assert!(
            failures.is_empty(),
            "task writes failed under contention: {failures:#?}"
        );
    }
    fn reopen_test_store(cas_dir: &Path, with_team: bool) -> SyncingTaskStore {
        let inner = SqliteTaskStore::open(cas_dir).unwrap();
        let queue = SyncQueue::open(cas_dir).unwrap();
        let store = SyncingTaskStore::new(Arc::new(inner), Arc::new(queue));
        if with_team {
            let mut cfg = CloudConfig::default();
            cfg.set_team(TEST_TEAM, "test-team");
            store.with_cloud_config(Arc::new(cfg))
        } else {
            store
        }
    }

    fn reopen_test_store_for_team(cas_dir: &Path, team_id: &str) -> SyncingTaskStore {
        let inner = SqliteTaskStore::open(cas_dir).unwrap();
        let queue = SyncQueue::open(cas_dir).unwrap();
        let mut cfg = CloudConfig::default();
        cfg.set_team(team_id, "test-team");
        SyncingTaskStore::new(Arc::new(inner), Arc::new(queue)).with_cloud_config(Arc::new(cfg))
    }

    fn install_task_enqueue_failure(cas_dir: &Path, team_id: &str) {
        let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
        conn.execute_batch(&format!(
            r#"
            CREATE TRIGGER fail_task_enqueue
            BEFORE INSERT ON sync_queue
            WHEN NEW.entity_type = 'task' AND NEW.team_id = '{team_id}'
            BEGIN
                SELECT RAISE(FAIL, 'injected task enqueue failure');
            END;
            "#,
        ))
        .unwrap();
    }

    fn task_mutation_revision(cas_dir: &Path, task_id: &str) -> (i64, bool) {
        let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
        conn.query_row(
            "SELECT revision, present FROM task_mutation_revisions WHERE entity_id = ?1",
            [task_id],
            |row| Ok((row.get(0)?, row.get::<_, i64>(1)? == 1)),
        )
        .unwrap()
    }

    fn remove_task_enqueue_failure(cas_dir: &Path) {
        let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
        conn.execute_batch("DROP TRIGGER fail_task_enqueue;")
            .unwrap();
    }

    fn assert_degraded(error: StoreError, operation: &str, entity_id: &str) {
        match error {
            StoreError::SyncDegradedAfterCommit {
                entity_type,
                entity_id: actual_entity_id,
                operation: actual_operation,
                reason,
            } => {
                assert_eq!(entity_type, "task");
                assert_eq!(actual_entity_id, entity_id);
                assert_eq!(actual_operation, operation);
                assert!(reason.contains("injected task enqueue failure"), "{reason}");
            }
            other => panic!("expected structured degraded-sync error, got {other}"),
        }
    }

    #[test]
    fn test_add_queues_sync() {
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();

        let task = Task::new("task-001".to_string(), "Test task".to_string());
        store.add(&task).unwrap();

        let pending = queue.pending(10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].entity_type, EntityType::Task);
        assert_eq!(pending[0].entity_id, task.id);
        assert_eq!(pending[0].operation, SyncOperation::Upsert);
    }

    #[tokio::test]
    async fn mcp_commit_receipt_survives_projection_failure_but_not_rollback_cas_e4a8() {
        use crate::mcp::tools::service::mutation_receipt::{Receipt, scope};
        let (temp, store) = create_test_store();
        install_task_enqueue_failure(temp.path(), "");
        let task = Task::new("task-committed-receipt".into(), "committed".into());
        let receipt = Receipt::new("task", "create", None);
        scope(receipt.clone(), async {
            assert_degraded(store.add(&task).unwrap_err(), "add", &task.id);
        }).await;
        assert!(receipt.commit.get().unwrap().description.contains(&task.id));
        assert_eq!(store.get(&task.id).unwrap().title, "committed");

        remove_task_enqueue_failure(temp.path());
        let conn = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON task_mutation_receipts BEGIN SELECT RAISE(ABORT, 'rollback'); END;").unwrap();
        let task = Task::new("task-rollback-receipt".into(), "rolled back".into());
        let receipt = Receipt::new("task", "create", None);
        scope(receipt.clone(), async { assert!(store.add(&task).is_err()); }).await;
        assert!(receipt.commit.get().is_none(), "a rollback must remain unconfirmed");
        assert!(matches!(store.get(&task.id), Err(StoreError::TaskNotFound(_))));
    }

    #[test]
    fn add_reports_degraded_sync_when_the_personal_outbox_write_fails() {
        let (temp, store) = create_test_store();
        install_task_enqueue_failure(temp.path(), "");

        let task = Task::new(
            "task-degraded-add".to_string(),
            "locally committed".to_string(),
        );
        let error = store.add(&task).unwrap_err();

        assert_degraded(error, "add", &task.id);
        assert_eq!(store.get(&task.id).unwrap().title, "locally committed");
        let queue = SyncQueue::open(temp.path()).unwrap();
        assert!(queue.pending(10, 5).unwrap().is_empty());
        let intents = queue.pending_task_sync_intents().unwrap();
        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].entity_id, task.id);

        let still_broken = reopen_test_store(temp.path(), false);
        assert_degraded(still_broken.init().unwrap_err(), "add", &task.id);
        assert_eq!(queue.pending_task_sync_intents().unwrap().len(), 1);

        remove_task_enqueue_failure(temp.path());
        let restarted = reopen_test_store(temp.path(), false);
        restarted.init().unwrap();
        assert!(queue.pending_task_sync_intents().unwrap().is_empty());
        let pending = queue.pending(10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].entity_id, task.id);
        assert!(
            pending[0]
                .payload
                .as_deref()
                .is_some_and(|payload| payload.contains("locally committed"))
        );
    }

    #[test]
    fn intent_staging_failure_is_pre_commit_and_releases_the_mutation_lease() {
        let (temp, store) = create_test_store();
        let conn = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
        conn.execute_batch("DROP TABLE task_sync_intents;").unwrap();
        let task = Task::new(
            "task-pre-commit-queue-failure".to_string(),
            "must not commit".to_string(),
        );

        let error = store.add(&task).unwrap_err();

        assert!(matches!(error, StoreError::Database(_)));
        assert!(matches!(
            store.get(&task.id),
            Err(StoreError::TaskNotFound(_))
        ));
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(temp.path().join("task-sync-intents.lock"))
            .unwrap();
        contender
            .try_lock_exclusive()
            .expect("every pre-commit error path must release the mutation lease");
        FileExt::unlock(&contender).unwrap();
    }

    #[test]
    fn atomic_receipt_failure_rolls_back_the_task_mutation() {
        let (temp, store) = create_test_store();
        let conn = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
        conn.execute_batch(
            r#"
            CREATE TRIGGER fail_task_mutation_receipt
            BEFORE INSERT ON task_mutation_receipts
            BEGIN
                SELECT RAISE(ABORT, 'injected receipt failure');
            END;
            "#,
        )
        .unwrap();
        let task = Task::new(
            "task-atomic-receipt-failure".to_string(),
            "must roll back".to_string(),
        );

        let error = store.add(&task).unwrap_err();

        assert!(!matches!(error, StoreError::SyncDegradedAfterCommit { .. }));
        assert!(matches!(
            store.get(&task.id),
            Err(StoreError::TaskNotFound(_))
        ));
        assert!(store.queue.pending_task_sync_intents().unwrap().is_empty());
    }

    #[test]
    fn store_without_atomic_receipt_support_fails_before_mutation() {
        let inner = MockTaskStore::new();
        let task = Task::new(
            "task-unsupported-receipt".to_string(),
            "unchanged".to_string(),
        );

        let error = inner
            .add_with_mutation_receipt(&task, "receipt-id")
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("atomic task mutation receipts are unsupported"),
            "{error}"
        );
        assert!(inner.is_empty());
    }

    #[test]
    fn task_mutation_revision_seeds_and_stays_monotonic_across_delete_replace_and_upsert() {
        let (temp, store) = create_test_store();
        let task = Task::new("task-revision-lifecycle".to_string(), "one".to_string());
        store.inner.add(&task).unwrap();
        let (insert_revision, present) = task_mutation_revision(temp.path(), &task.id);
        assert!(present);

        let conn = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
        conn.execute(
            "DELETE FROM task_mutation_revisions WHERE entity_id = ?1",
            [&task.id],
        )
        .unwrap();
        store.inner.init().unwrap();
        assert_eq!(task_mutation_revision(temp.path(), &task.id), (1, true));

        store.inner.delete(&task.id).unwrap();
        let (deleted_revision, present) = task_mutation_revision(temp.path(), &task.id);
        assert!(!present);
        assert!(deleted_revision > insert_revision);
        store.inner.add(&task).unwrap();
        let (recreated_revision, present) = task_mutation_revision(temp.path(), &task.id);
        assert!(present);
        assert!(recreated_revision > deleted_revision);

        conn.execute(
            "INSERT OR REPLACE INTO tasks (id, title, created_at, updated_at) VALUES (?1, 'replace', ?2, ?2)",
            rusqlite::params![task.id, Utc::now().to_rfc3339()],
        )
        .unwrap();
        let (replace_revision, present) = task_mutation_revision(temp.path(), &task.id);
        assert!(present);
        assert!(replace_revision > recreated_revision);

        conn.execute(
            "INSERT INTO tasks (id, title, created_at, updated_at) VALUES (?1, 'upsert', ?2, ?2)
             ON CONFLICT(id) DO UPDATE SET title = excluded.title",
            rusqlite::params![task.id, Utc::now().to_rfc3339()],
        )
        .unwrap();
        let (upsert_revision, present) = task_mutation_revision(temp.path(), &task.id);
        assert!(present);
        assert!(upsert_revision > replace_revision);
    }

    #[test]
    fn update_reports_degraded_personal_sync_and_restart_queues_the_committed_row() {
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();
        let mut task = Task::new("task-degraded-update".to_string(), "before".to_string());
        store.add(&task).unwrap();
        queue.clear().unwrap();
        install_task_enqueue_failure(temp.path(), "");

        task.title = "after local commit".to_string();
        assert_degraded(store.update(&task).unwrap_err(), "update", &task.id);
        assert_eq!(store.get(&task.id).unwrap().title, "after local commit");
        assert!(queue.pending(10, 5).unwrap().is_empty());
        assert_eq!(queue.pending_task_sync_intents().unwrap().len(), 1);

        remove_task_enqueue_failure(temp.path());
        reopen_test_store(temp.path(), false).init().unwrap();
        assert!(queue.pending_task_sync_intents().unwrap().is_empty());
        let pending = queue.pending(10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert!(
            pending[0]
                .payload
                .as_deref()
                .is_some_and(|payload| payload.contains("after local commit"))
        );
    }

    #[test]
    fn restart_discards_an_update_intent_staged_before_an_uncommitted_write() {
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();
        let task = Task::new(
            "task-uncommitted-update".to_string(),
            "unchanged".to_string(),
        );
        store.add(&task).unwrap();
        queue.clear().unwrap();
        let previous_updated_at = store.get(&task.id).unwrap().updated_at.to_rfc3339();
        queue
            .stage_task_sync_intent(
                &task.id,
                "update",
                Some(&previous_updated_at),
                None,
                None,
                None,
                false,
            )
            .unwrap();

        reopen_test_store(temp.path(), false).init().unwrap();

        assert!(queue.pending_task_sync_intents().unwrap().is_empty());
        assert!(
            queue.pending(10, 5).unwrap().is_empty(),
            "an update that never committed must not be synthesized on restart"
        );
    }

    /// A live mutation's staged-but-uncommitted intent looks exactly like a
    /// crashed pre-commit one. Reconcile must defer it, without waiting,
    /// whether the writer is a current mutator (entity stripe) or a pre-GH
    /// #1165 binary (exclusive process lease), and consume it once the writer
    /// has committed and released.
    #[test]
    fn independent_reconciler_defers_a_live_pre_write_intent() {
        for legacy_writer in [false, true] {
            let (temp, _) = create_test_store();
            let cas_dir = temp.path().to_path_buf();
            let writer_queue = SyncQueue::open(&cas_dir).unwrap();
            writer_queue.init().unwrap();
            let task = Task::new(
                format!("task-live-pre-write-intent-{legacy_writer}"),
                "commit after reconcile".to_string(),
            );
            let legacy_lock = OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(false)
                .open(cas_dir.join("task-sync-intents.lock"))
                .unwrap();
            let mutation = if legacy_writer {
                legacy_lock.lock_exclusive().unwrap();
                None
            } else {
                Some(writer_queue.lock_task_sync_mutations(&[&task.id]).unwrap())
            };
            let intent = writer_queue
                .stage_task_sync_intent(&task.id, "add", None, None, None, None, false)
                .unwrap();

            let reconciler = reopen_test_store(&cas_dir, false);
            let started = std::time::Instant::now();
            assert_eq!(
                reconciler.reconcile_pending_task_sync().unwrap(),
                TaskSyncReconcileOutcome::Deferred
            );
            assert!(started.elapsed() < Duration::from_millis(100));
            reconciler.init().unwrap();
            assert_eq!(
                writer_queue.pending_task_sync_intents().unwrap(),
                vec![intent.clone()],
                "a live writer's intent must survive reconciliation"
            );

            let local = SqliteTaskStore::open(&cas_dir).unwrap();
            local.init().unwrap();
            local
                .add_with_mutation_receipt(&task, &intent.mutation_id)
                .unwrap();
            drop(mutation);
            if legacy_writer {
                FileExt::unlock(&legacy_lock).unwrap();
            }

            assert_eq!(
                reconciler.reconcile_pending_task_sync().unwrap(),
                TaskSyncReconcileOutcome::Clean
            );
            assert!(writer_queue.pending_task_sync_intents().unwrap().is_empty());
            assert_eq!(writer_queue.pending(10, 5).unwrap().len(), 1);
        }
    }

    /// GH #1165 (2): a task-store open reconciles once per process, then only
    /// on the bounded schedule; a deferred pass is retried and the intent
    /// still reaches the queue.
    #[test]
    fn open_path_reconciles_once_then_on_schedule_and_retries_deferred_work() {
        let (temp, store) = create_test_store();
        let task = leave_degraded_update_intent(&temp, &store, "task-scheduled-reconcile");
        let cas_dir = temp.path().to_path_buf();

        // First pass in this process: a live mutation owns the task, so the
        // pass defers without waiting and schedules a retry.
        let held = store.queue.lock_task_sync_mutations(&[&task.id]).unwrap();
        let started = std::time::Instant::now();
        reopen_test_store(&cas_dir, false).reconcile_if_due();
        assert!(started.elapsed() < Duration::from_millis(100));
        drop(held);
        assert_eq!(store.queue.pending_task_sync_intents().unwrap().len(), 1);

        // Not yet due: further opens do no reconcile work at all.
        reopen_test_store(&cas_dir, false).reconcile_if_due();
        assert_eq!(store.queue.pending_task_sync_intents().unwrap().len(), 1);
        let retry_due = *reconcile_schedule().get(&cas_dir).unwrap();
        assert!(retry_due <= Instant::now() + RECONCILE_RETRY);

        // Once the retry is due, the deferred intent is fulfilled.
        reconcile_schedule().insert(cas_dir.clone(), Instant::now());
        reopen_test_store(&cas_dir, false).reconcile_if_due();
        assert!(store.queue.pending_task_sync_intents().unwrap().is_empty());
        let pending = store.queue.pending(10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert!(
            pending[0]
                .payload
                .as_deref()
                .is_some_and(|payload| payload.contains("committed, sync pending"))
        );
        let clean_due = *reconcile_schedule().get(&cas_dir).unwrap();
        assert!(clean_due > Instant::now() + RECONCILE_RETRY);
    }

    /// GH #1165 (3): reconcile bounds its SQLite write wait and defers rather
    /// than spending the ~31 s retry budget on an open path.
    #[test]
    fn reconcile_defers_when_the_database_stays_write_busy() {
        let (temp, store) = create_test_store();
        leave_degraded_update_intent(&temp, &store, "task-busy-reconcile");
        let blocker = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let started = std::time::Instant::now();
        let outcome = store.reconcile_pending_task_sync().unwrap();
        let elapsed = started.elapsed();
        blocker.execute_batch("ROLLBACK").unwrap();
        assert_eq!(outcome, TaskSyncReconcileOutcome::Deferred);
        assert!(
            elapsed < Duration::from_secs(1),
            "reconcile waited {elapsed:?}"
        );
        assert_eq!(store.queue.pending_task_sync_intents().unwrap().len(), 1);
        assert_eq!(
            store.reconcile_pending_task_sync().unwrap(),
            TaskSyncReconcileOutcome::Clean
        );
        assert!(store.queue.pending_task_sync_intents().unwrap().is_empty());
    }

    /// Leave one committed task whose sync intent is still pending, the state
    /// a degraded post-commit enqueue leaves behind.
    fn leave_degraded_update_intent(temp: &TempDir, store: &SyncingTaskStore, id: &str) -> Task {
        let mut task = Task::new(id.to_string(), "before".to_string());
        store.add(&task).unwrap();
        store.queue.clear().unwrap();
        install_task_enqueue_failure(temp.path(), "");
        task.title = "committed, sync pending".to_string();
        assert_degraded(store.update(&task).unwrap_err(), "update", &task.id);
        remove_task_enqueue_failure(temp.path());
        assert_eq!(store.queue.pending_task_sync_intents().unwrap().len(), 1);
        task
    }

    /// GH #1165: every task-store open reconciled under a blocking exclusive
    /// flock on task-sync-intents.lock, so a read (the factory daemon's main
    /// loop, an MCP `task show`) queued behind any process holding it, for up
    /// to 42.8 s in the field. While another process holds the lock (here
    /// for up to 30 s), open + reconcile + get + list must finish within
    /// 100 ms, and the pending intent must stay for a later reconcile.
    #[test]
    fn reads_never_wait_for_a_held_task_sync_lock_gh_1165() {
        let (temp, store) = create_test_store();
        let task = leave_degraded_update_intent(&temp, &store, "task-held-lock-read");
        let holder = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(temp.path().join("task-sync-intents.lock"))
            .unwrap();
        holder.lock_exclusive().unwrap();

        let (done_tx, done_rx) = mpsc::channel();
        let cas_dir = temp.path().to_path_buf();
        let task_id = task.id.clone();
        let reader = std::thread::spawn(move || {
            let reopened = reopen_test_store(&cas_dir, false);
            let started = std::time::Instant::now();
            let init = reopened.init();
            let got = reopened.get(&task_id).map(|task| task.title);
            let listed = reopened.list(None).map(|tasks| tasks.len());
            done_tx
                .send((started.elapsed(), init.is_ok(), got, listed))
                .unwrap();
        });
        let outcome = done_rx.recv_timeout(Duration::from_secs(30));
        FileExt::unlock(&holder).unwrap();
        reader.join().unwrap();
        let (elapsed, init_ok, got, listed) =
            outcome.expect("a read must not wait for the held task-sync lock");
        assert!(
            elapsed < Duration::from_millis(100),
            "open/reconcile/get/list took {elapsed:?} while the lock was held"
        );
        assert!(init_ok, "a deferred reconcile is not an open failure");
        assert_eq!(got.unwrap(), "committed, sync pending");
        assert_eq!(listed.unwrap(), 1);
        assert_eq!(
            store.queue.pending_task_sync_intents().unwrap().len(),
            1,
            "the deferred intent stays durable for a later reconcile"
        );
    }

    #[test]
    fn unbound_advanced_intent_is_retained_without_queue_output() {
        let (_temp, store) = create_test_store();
        let mut task = Task::new("task-ambiguous-intent".to_string(), "before".to_string());
        store.inner.add(&task).unwrap();
        let previous = store.inner.get(&task.id).unwrap();
        let intent = store
            .stage_upsert(
                &task,
                "update",
                Some(&previous.updated_at.to_rfc3339()),
                Some(&previous),
            )
            .unwrap();
        task.title = "advanced outside the receipt protocol".to_string();
        store.inner.update(&task).unwrap();

        let error = store.reconcile_pending_task_sync().unwrap_err();

        assert!(error.to_string().contains("unclassified"), "{error}");
        assert!(error.to_string().contains("evidence retained"), "{error}");
        assert!(store.queue.pending(10, 5).unwrap().is_empty());
        assert_eq!(
            store.queue.pending_task_sync_intents().unwrap(),
            vec![intent]
        );
    }

    #[test]
    fn canonical_read_failure_rolls_back_without_retiring_bound_evidence() {
        let (temp, store) = create_test_store();
        install_task_enqueue_failure(temp.path(), "");
        let task = Task::new("task-read-failure".to_string(), "committed".to_string());
        assert_degraded(store.add(&task).unwrap_err(), "add", &task.id);
        remove_task_enqueue_failure(temp.path());
        let conn = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
        conn.execute_batch("DROP TABLE tasks;").unwrap();

        assert!(store.reconcile_pending_task_sync().is_err());

        assert!(store.queue.pending(10, 5).unwrap().is_empty());
        assert_eq!(store.queue.pending_task_sync_intents().unwrap().len(), 1);
    }

    #[test]
    fn direct_writer_cannot_interleave_between_revision_validation_and_production_payload_load() {
        let (temp, store) = create_test_store();
        let mut task = Task::new("task-fulfill-snapshot".to_string(), "before".to_string());
        store.add(&task).unwrap();
        store.queue.clear().unwrap();
        install_task_enqueue_failure(temp.path(), "");
        task.title = "validated canonical body".to_string();
        assert_degraded(store.update(&task).unwrap_err(), "update", &task.id);
        remove_task_enqueue_failure(temp.path());
        let intent = store
            .queue
            .pending_task_sync_intents()
            .unwrap()
            .pop()
            .unwrap();

        let (start_tx, start_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let direct_dir = temp.path().to_path_buf();
        let task_id = task.id.clone();
        let writer = std::thread::spawn(move || {
            start_rx.recv().unwrap();
            let direct = SqliteTaskStore::open(&direct_dir).unwrap();
            let mut newer = direct.get(&task_id).unwrap();
            newer.title = "interleaving direct body".to_string();
            let result = direct.update(&newer);
            finished_tx.send(result).unwrap();
        });

        let outcome = store
            .fulfill_upsert_after_validation(&intent, TaskSyncFulfillMode::Mutation, || {
                start_tx.send(()).unwrap();
                assert!(
                    finished_rx
                        .recv_timeout(Duration::from_millis(100))
                        .is_err(),
                    "BEGIN IMMEDIATE must exclude the direct writer through payload enqueue"
                );
            })
            .unwrap();
        assert_eq!(outcome, TaskSyncFulfillResult::Fulfilled);
        finished_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("direct writer should complete after fulfillment commits")
            .unwrap();
        writer.join().unwrap();

        let pending = store.queue.pending(10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        let payload = pending[0].payload.as_deref().unwrap();
        assert!(payload.contains("validated canonical body"));
        assert!(!payload.contains("interleaving direct body"));
    }

    /// cas-0e57: fulfillment must not hold the SQLite write lock while it
    /// waits for the task store's in-process connection mutex. Another thread
    /// holding that mutex while it waited for the write lock made a lock-order
    /// inversion that stalled every writer for a full retry budget (31.6s)
    /// during a fleet boot. Here a thread holds the mutex; while fulfillment
    /// waits for it, a foreign connection must still be able to write.
    #[test]
    fn fulfillment_waits_for_the_store_mutex_without_holding_the_write_lock_cas_0e57() {
        let (temp, store) = create_test_store();
        let mut task = Task::new("task-0e57-inversion".to_string(), "before".to_string());
        store.add(&task).unwrap();
        store.queue.clear().unwrap();
        install_task_enqueue_failure(temp.path(), "");
        task.title = "canonical body".to_string();
        assert_degraded(store.update(&task).unwrap_err(), "update", &task.id);
        remove_task_enqueue_failure(temp.path());
        let intent = store
            .queue
            .pending_task_sync_intents()
            .unwrap()
            .pop()
            .unwrap();

        let db = temp.path().join("cas.db");
        let pooled = cas_store::shared_db::shared_connection(&db).unwrap();
        let (held_tx, held_rx) = mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _guard = pooled.lock().unwrap();
            held_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(1500));
        });
        held_rx.recv().unwrap();

        let store = Arc::new(store);
        let fulfilling = Arc::clone(&store);
        let fulfiller = std::thread::spawn(move || fulfilling.fulfill_upsert(&intent));
        std::thread::sleep(Duration::from_millis(300));
        let probe = rusqlite::Connection::open(&db).unwrap();
        let write = probe.execute_batch("BEGIN IMMEDIATE; ROLLBACK;");
        holder.join().unwrap();
        let outcome = fulfiller.join().unwrap().unwrap();
        assert!(
            write.is_ok(),
            "fulfillment held the write lock while waiting for the store mutex: {write:?}"
        );
        assert_eq!(outcome, TaskSyncFulfillResult::Fulfilled);
        let pending = store.queue.pending(10, 5).unwrap();
        assert!(
            pending
                .iter()
                .any(|row| row.payload.as_deref().is_some_and(|p| p.contains("canonical body"))),
            "the canonical body must be queued ({} rows)",
            pending.len()
        );
    }

    #[test]
    fn conditional_sync_update_never_queues_an_echo_or_overwrites_a_later_edit() {
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();
        let task = Task::new("cas-86eb-wrapper".into(), "local".into());
        store.add(&task).unwrap();
        queue.clear().unwrap();
        let expected = store.get(&task.id).unwrap();
        let mut remote = expected.clone();
        remote.description = "remote context".into();
        assert!(
            store
                .update_from_sync(&remote, &expected)
                .unwrap()
                .is_some()
        );
        assert_eq!(store.get(&task.id).unwrap().description, remote.description);
        assert!(queue.pending(10, 5).unwrap().is_empty());
        let mut edited = store.get(&task.id).unwrap();
        edited.description = "later local edit".into();
        store.update(&edited).unwrap();
        assert_eq!(store.update_from_sync(&remote, &expected).unwrap(), None);
        assert_eq!(store.get(&task.id).unwrap().description, edited.description);
        assert_eq!(queue.pending(10, 5).unwrap().len(), 1);
    }

    #[test]
    fn test_update_queues_sync() {
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut task = Task::new("task-002".to_string(), "Test task".to_string());
        store.add(&task).unwrap();

        // Clear queue
        queue.clear().unwrap();

        task.title = "Updated title".to_string();
        store.update(&task).unwrap();

        let pending = queue.pending(10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert!(
            pending[0]
                .payload
                .as_ref()
                .unwrap()
                .contains("Updated title")
        );
    }

    #[test]
    fn test_append_note_queues_the_canonical_task_once() {
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();
        let task = Task::new("task-note-sync".to_string(), "Task notes".to_string());
        store.add(&task).unwrap();
        queue.clear().unwrap();

        store
            .append_note(&task.id, "synced task note")
            .expect("append note should commit locally and stage sync");

        let pending = queue.pending(10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].entity_id, task.id);
        assert_eq!(pending[0].operation, SyncOperation::Upsert);
        assert_eq!(
            pending[0]
                .payload
                .as_ref()
                .unwrap()
                .matches("synced task note")
                .count(),
            1
        );
        assert_eq!(
            store
                .get(&task.id)
                .unwrap()
                .notes
                .matches("synced task note")
                .count(),
            1
        );
    }

    #[test]
    fn work_target_roundtrips_in_sync_payload_without_host_paths() {
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();
        let mut task = Task::new("task-target".to_string(), "Cross repo".to_string());
        task.deliverables.work_target = Some(crate::types::WorkTarget {
            repo_selector: "remote:github.com/org/repo".to_string(),
            target_branch: "master".to_string(),
        });
        task.deliverables.pre_close_hook = Some(crate::types::PreCloseHookEvidence {
            repo_selector: "remote:github.com/org/repo".to_string(),
            target_branch: "master".to_string(),
            worktree_branch: Some("factory/worker".to_string()),
            task_tip: Some("0123456789abcdef".to_string()),
        });
        store.add(&task).unwrap();

        let pending = queue.pending(10, 5).unwrap();
        let payload = pending[0].payload.as_deref().unwrap();
        assert!(payload.contains("remote:github.com/org/repo"));
        assert!(!payload.contains(temp.path().to_string_lossy().as_ref()));
        let roundtrip: Task = serde_json::from_str(payload).unwrap();
        assert_eq!(
            roundtrip
                .deliverables
                .work_target
                .as_ref()
                .unwrap()
                .target_branch,
            "master"
        );
        let evidence = roundtrip.deliverables.pre_close_hook.as_ref().unwrap();
        assert_eq!(evidence.worktree_branch.as_deref(), Some("factory/worker"));
        assert_eq!(evidence.task_tip.as_deref(), Some("0123456789abcdef"));
    }

    #[test]
    fn test_delete_queues_sync() {
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();

        let task = Task::new("task-003".to_string(), "Test task".to_string());
        store.add(&task).unwrap();

        // Clear queue
        queue.clear().unwrap();

        store.delete(&task.id).unwrap();

        let pending = queue.pending(10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].operation, SyncOperation::Delete);
    }

    #[test]
    fn dependency_add_queues_task_dependency_upsert() {
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();
        let from = Task::new("task-dep-from".to_string(), "from".to_string());
        let to = Task::new("task-dep-to".to_string(), "to".to_string());
        store.add(&from).unwrap();
        store.add(&to).unwrap();
        queue.clear().unwrap();

        let dep = Dependency::new(from.id.clone(), to.id.clone(), DependencyType::Blocks);
        store.add_dependency(&dep).unwrap();

        let pending = queue
            .pending_for_entity_type(Some(EntityType::TaskDependency), 10, 5)
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].entity_id, "task-dep-from:task-dep-to:blocks");
        assert_eq!(pending[0].operation, SyncOperation::Upsert);
        let payload: serde_json::Value =
            serde_json::from_str(pending[0].payload.as_deref().unwrap()).unwrap();
        assert_eq!(payload["from_id"], "task-dep-from");
        assert_eq!(payload["to_id"], "task-dep-to");
        assert_eq!(payload["dep_type"], "blocks");
        assert!(payload["created_at"].is_string());
        assert!(payload.get("origin_project").is_some());
    }

    #[test]
    fn dependency_remove_queues_task_dependency_delete() {
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();
        let from = Task::new("task-remove-from".to_string(), "from".to_string());
        let to = Task::new("task-remove-to".to_string(), "to".to_string());
        store.add(&from).unwrap();
        store.add(&to).unwrap();
        let dep = Dependency::new(from.id.clone(), to.id.clone(), DependencyType::Related);
        store.add_dependency(&dep).unwrap();
        queue.clear().unwrap();

        assert!(
            store
                .remove_dependency_of_type(&from.id, &to.id, DependencyType::Related)
                .unwrap()
        );

        let pending = queue
            .pending_for_entity_type(Some(EntityType::TaskDependency), 10, 5)
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].entity_id,
            "task-remove-from:task-remove-to:related"
        );
        assert_eq!(pending[0].operation, SyncOperation::Delete);
        assert!(pending[0].payload.is_none());
    }

    #[test]
    fn create_atomic_queues_all_created_task_dependencies() {
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut epic = Task::new("task-dep-epic".to_string(), "epic".to_string());
        epic.task_type = crate::types::TaskType::Epic;
        store.add(&epic).unwrap();
        let blocker = Task::new("task-dep-blocker".to_string(), "blocker".to_string());
        store.add(&blocker).unwrap();
        queue.clear().unwrap();

        let child = Task::new("task-dep-child".to_string(), "child".to_string());
        store
            .create_atomic(&child, &[blocker.id.clone()], Some(&epic.id), Some("test"))
            .unwrap();

        let pending = queue
            .pending_for_entity_type(Some(EntityType::TaskDependency), 10, 5)
            .unwrap();
        let ids: std::collections::HashSet<_> =
            pending.iter().map(|item| item.entity_id.as_str()).collect();
        assert_eq!(pending.len(), 2);
        assert!(ids.contains("task-dep-child:task-dep-blocker:blocks"));
        assert!(ids.contains("task-dep-child:task-dep-epic:parent-child"));
    }

    // ── Dual-enqueue behaviour (cas-82a1) ────────────────────────────────

    use cas_types::Scope;

    use crate::store::share_policy::TEST_TEAM_UUID as TEST_TEAM;
    const SECOND_TEAM: &str = "650e8400-e29b-41d4-a716-446655440001";

    fn create_team_store(team_auto_promote: Option<bool>) -> (TempDir, SyncingTaskStore) {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path();
        let inner = SqliteTaskStore::open(cas_dir).unwrap();
        inner.init().unwrap();
        let queue = SyncQueue::open(cas_dir).unwrap();
        queue.init().unwrap();
        let mut cfg = CloudConfig::default();
        cfg.set_team(TEST_TEAM, "test-team");
        cfg.team_auto_promote = team_auto_promote;
        let store = SyncingTaskStore::new(Arc::new(inner), Arc::new(queue))
            .with_cloud_config(Arc::new(cfg));
        (temp, store)
    }

    fn queue_counts(queue: &SyncQueue) -> (usize, usize) {
        let personal = queue.pending(100, 5).unwrap().len();
        let team = queue.pending_for_team(TEST_TEAM, 100, 5).unwrap().len();
        (personal, team)
    }

    #[test]
    fn task_dual_enqueue_when_team_configured_and_project_scope() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        // Default task is Project scope — passes T1 filter.
        let task = Task::new("p-task-001".to_string(), "team task".to_string());
        store.add(&task).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 1, "team queue should have the task");
    }

    #[test]
    fn team_only_task_routes_project_to_team_and_global_to_personal() {
        let (temp, mut store) = create_team_store(None);
        store.team_only = true;
        let queue = SyncQueue::open(temp.path()).unwrap();
        store
            .add(&Task::new("team-only-task".to_string(), "team".to_string()))
            .unwrap();
        assert_eq!(queue_counts(&queue), (0, 1));

        let mut global = Task::new("global-task".to_string(), "global".to_string());
        global.scope = Scope::Global;
        store.add(&global).unwrap();
        assert_eq!(queue_counts(&queue), (1, 1));

        queue.clear().unwrap();
        let child = Task::new("team-only-child".to_string(), "child".to_string());
        store
            .create_atomic(&child, &["team-only-task".to_string()], None, None)
            .unwrap();
        assert_eq!(queue.pending(10, 5).unwrap().len(), 0);
        assert_eq!(queue.pending_for_team(TEST_TEAM, 10, 5).unwrap().len(), 2);
    }

    #[test]
    fn team_only_task_update_removes_the_older_personal_copy() {
        let (temp, mut store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();
        let mut task = Task::new("switching-task".to_string(), "before".to_string());
        store.add(&task).unwrap();
        assert_eq!(queue_counts(&queue), (1, 1));

        store.team_only = true;
        task.title = "after".to_string();
        store.update(&task).unwrap();
        assert_eq!(queue_counts(&queue), (0, 1));
    }

    #[test]
    fn team_only_without_team_keeps_project_task_personal() {
        let (temp, store) = create_test_store();
        let mut config = CloudConfig::default();
        config.team_only = true;
        let store = store.with_cloud_config(Arc::new(config));
        let queue = SyncQueue::open(temp.path()).unwrap();
        store
            .add(&Task::new("unlinked-task".to_string(), "local".to_string()))
            .unwrap();
        assert_eq!(queue.pending(10, 5).unwrap().len(), 1);
    }

    #[test]
    fn add_reports_degraded_team_sync_and_restart_atomically_queues_both_paths() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();
        install_task_enqueue_failure(temp.path(), TEST_TEAM);

        let task = Task::new("task-team-degraded-add".to_string(), "team add".to_string());
        assert_degraded(store.add(&task).unwrap_err(), "add", &task.id);
        assert_eq!(store.get(&task.id).unwrap().title, "team add");
        assert_eq!(
            queue_counts(&queue),
            (0, 0),
            "the failed team row must roll back the personal row"
        );
        assert_eq!(queue.pending_task_sync_intents().unwrap().len(), 1);

        remove_task_enqueue_failure(temp.path());
        reopen_test_store(temp.path(), true).init().unwrap();
        assert!(queue.pending_task_sync_intents().unwrap().is_empty());
        assert_eq!(queue_counts(&queue), (1, 1));
    }

    #[test]
    fn update_reports_degraded_team_sync_and_restart_queues_committed_state_to_both_paths() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();
        let mut task = Task::new(
            "task-team-degraded-update".to_string(),
            "before".to_string(),
        );
        store.add(&task).unwrap();
        queue.clear().unwrap();
        install_task_enqueue_failure(temp.path(), TEST_TEAM);

        task.title = "team update committed".to_string();
        assert_degraded(store.update(&task).unwrap_err(), "update", &task.id);
        assert_eq!(store.get(&task.id).unwrap().title, "team update committed");
        assert_eq!(queue_counts(&queue), (0, 0));
        assert_eq!(queue.pending_task_sync_intents().unwrap().len(), 1);

        remove_task_enqueue_failure(temp.path());
        reopen_test_store(temp.path(), true).init().unwrap();
        assert!(queue.pending_task_sync_intents().unwrap().is_empty());
        assert_eq!(queue_counts(&queue), (1, 1));
        for queued in queue
            .pending(10, 5)
            .unwrap()
            .into_iter()
            .chain(queue.pending_for_team(TEST_TEAM, 10, 5).unwrap())
        {
            assert!(
                queued
                    .payload
                    .as_deref()
                    .is_some_and(|payload| payload.contains("team update committed"))
            );
        }
    }

    #[test]
    fn restart_does_not_replay_a_stale_team_intent_after_team_promotion_is_disabled() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();
        let mut task = Task::new("task-stale-team-disabled".to_string(), "before".to_string());
        store.add(&task).unwrap();
        queue.clear().unwrap();
        install_task_enqueue_failure(temp.path(), TEST_TEAM);
        task.title = "committed while team A was active".to_string();
        assert_degraded(store.update(&task).unwrap_err(), "update", &task.id);
        remove_task_enqueue_failure(temp.path());

        reopen_test_store(temp.path(), false).init().unwrap();

        assert!(queue.pending_task_sync_intents().unwrap().is_empty());
        assert_eq!(queue.pending(10, 5).unwrap().len(), 1);
        let obsolete_team = queue.pending_for_team(TEST_TEAM, 10, 5).unwrap();
        assert_eq!(obsolete_team.len(), 1);
        assert_eq!(obsolete_team[0].operation, SyncOperation::Delete);
        assert!(obsolete_team[0].payload.is_none());
    }

    #[test]
    fn restart_routes_a_stale_team_intent_only_to_the_current_team() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();
        let mut task = Task::new("task-stale-team-change".to_string(), "before".to_string());
        store.add(&task).unwrap();
        queue.clear().unwrap();
        install_task_enqueue_failure(temp.path(), TEST_TEAM);
        task.title = "current team only".to_string();
        assert_degraded(store.update(&task).unwrap_err(), "update", &task.id);
        remove_task_enqueue_failure(temp.path());

        reopen_test_store_for_team(temp.path(), SECOND_TEAM)
            .init()
            .unwrap();

        let obsolete_team = queue.pending_for_team(TEST_TEAM, 10, 5).unwrap();
        assert_eq!(obsolete_team.len(), 1);
        assert_eq!(obsolete_team[0].operation, SyncOperation::Delete);
        assert!(obsolete_team[0].payload.is_none());
        let current_team = queue.pending_for_team(SECOND_TEAM, 10, 5).unwrap();
        assert_eq!(current_team.len(), 1);
        assert!(
            current_team[0]
                .payload
                .as_deref()
                .is_some_and(|payload| payload.contains("current team only"))
        );
    }

    #[test]
    fn newer_direct_local_global_mutation_supersedes_a_retained_project_intent() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();
        let mut task = Task::new(
            "task-stale-project-direct-local".to_string(),
            "before".to_string(),
        );
        store.add(&task).unwrap();
        queue.clear().unwrap();
        install_task_enqueue_failure(temp.path(), TEST_TEAM);
        task.title = "older failed project mutation".to_string();
        assert_degraded(store.update(&task).unwrap_err(), "update", &task.id);
        remove_task_enqueue_failure(temp.path());

        let direct = SqliteTaskStore::open(temp.path()).unwrap();
        let mut newer = direct.get(&task.id).unwrap();
        newer.scope = Scope::Global;
        newer.origin_project = None;
        newer.title = "newer direct-local private body".to_string();
        direct.update(&newer).unwrap();

        reopen_test_store(temp.path(), true).init().unwrap();

        assert!(queue.pending(10, 5).unwrap().is_empty());
        assert!(queue.pending_for_team(TEST_TEAM, 10, 5).unwrap().is_empty());
        assert!(queue.pending_task_sync_intents().unwrap().is_empty());
    }

    #[test]
    fn newer_global_mutation_supersedes_an_older_failed_team_intent() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();
        let mut task = Task::new("task-stale-team-global".to_string(), "before".to_string());
        store.add(&task).unwrap();
        queue.clear().unwrap();
        install_task_enqueue_failure(temp.path(), TEST_TEAM);
        task.title = "failed public update".to_string();
        assert_degraded(store.update(&task).unwrap_err(), "update", &task.id);
        remove_task_enqueue_failure(temp.path());

        task.scope = Scope::Global;
        task.title = "newest private payload".to_string();
        store.update(&task).unwrap();
        reopen_test_store(temp.path(), true).init().unwrap();

        assert!(queue.pending_task_sync_intents().unwrap().is_empty());
        let obsolete_team = queue.pending_for_team(TEST_TEAM, 10, 5).unwrap();
        assert_eq!(obsolete_team.len(), 1);
        assert_eq!(obsolete_team[0].operation, SyncOperation::Delete);
        assert!(obsolete_team[0].payload.is_none());
        let personal = queue.pending(10, 5).unwrap();
        assert_eq!(personal.len(), 1);
        assert!(
            personal[0]
                .payload
                .as_deref()
                .is_some_and(|payload| payload.contains("\"scope\":\"global\""))
        );
    }

    #[test]
    fn task_personal_only_when_global_scope() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut task = Task::new("g-task-001".to_string(), "global task".to_string());
        task.scope = Scope::Global;
        store.add(&task).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 0, "Global scope does not auto-promote");
    }

    #[test]
    fn task_personal_only_when_kill_switch_engaged() {
        let (temp, store) = create_team_store(Some(false));
        let queue = SyncQueue::open(temp.path()).unwrap();

        let task = Task::new("p-task-002".to_string(), "kill-switched".to_string());
        store.add(&task).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 0, "team_auto_promote=false disables dual-enqueue");
    }

    #[test]
    fn task_delete_dual_enqueues_when_team_configured() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let task = Task::new("p-task-003".to_string(), "to-delete".to_string());
        store.add(&task).unwrap();
        queue.clear().unwrap();

        store.delete(&task.id).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 1);
    }

    #[test]
    fn task_origin_project_move_queues_only_old_owner_delete() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut task = Task::new("p-task-move-001".to_string(), "move me".to_string());
        let local = crate::cloud::resolve_canonical_id(temp.path()).unwrap();
        task.origin_project = Some(local.clone());
        store.add(&task).unwrap();
        queue.clear().unwrap();

        task.origin_project = Some("project-b".to_string());
        store.update(&task).unwrap();

        let pending = queue.pending_for_team(TEST_TEAM, 10, 5).unwrap();
        assert_eq!(pending.len(), 1, "a move must only delete the local copy");
        assert_eq!(pending[0].operation, SyncOperation::Delete);
        assert_eq!(pending[0].entity_id, task.id);
        assert_eq!(pending[0].project_id.as_deref(), Some(local.as_str()));
        assert!(queue.pending(10, 5).unwrap().is_empty());
    }

    #[test]
    fn task_origin_project_move_removes_legacy_unkeyed_team_upsert() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut task = Task::new("p-task-move-legacy".to_string(), "move me".to_string());
        let local = crate::cloud::resolve_canonical_id(temp.path()).unwrap();
        task.origin_project = Some(local.clone());
        store.add(&task).unwrap();
        queue.clear().unwrap();

        // Rows written before project-keyed team queue identities were added
        // have a NULL project_id. Preserve one here to reproduce a live move
        // where the stale generic upsert survived beside the move pair.
        let legacy_payload =
            serde_json::to_string(&task).expect("legacy task payload should serialize");
        queue
            .enqueue_for_team(
                EntityType::Task,
                &task.id,
                SyncOperation::Upsert,
                Some(&legacy_payload),
                TEST_TEAM,
            )
            .unwrap();

        task.origin_project = Some("project-b".to_string());
        store.update(&task).unwrap();

        let pending = queue.pending_for_team(TEST_TEAM, 10, 5).unwrap();
        assert_eq!(pending.len(), 1, "a move must remove stale generic upserts");
        assert_eq!(pending[0].operation, SyncOperation::Delete);
        assert_eq!(pending[0].project_id.as_deref(), Some(local.as_str()));
    }

    #[test]
    fn task_origin_project_move_later_edit_queues_no_foreign_upsert() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut task = Task::new("p-task-move-002".to_string(), "move me".to_string());
        let local = crate::cloud::resolve_canonical_id(temp.path()).unwrap();
        task.origin_project = Some(local.clone());
        store.add(&task).unwrap();
        queue.clear().unwrap();

        task.origin_project = Some("project-b".to_string());
        store.update(&task).unwrap();

        task.title = "edited after move".to_string();
        store.update(&task).unwrap();

        let pending = queue.pending_for_team(TEST_TEAM, 10, 5).unwrap();
        assert_eq!(pending.len(), 1, "later edits must preserve the old delete");
        assert_eq!(pending[0].operation, SyncOperation::Delete);
        assert_eq!(pending[0].project_id.as_deref(), Some(local.as_str()));
        assert!(queue.pending(10, 5).unwrap().is_empty());
    }

    #[test]
    fn task_delete_after_origin_project_move_queues_no_foreign_delete() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut task = Task::new(
            "p-task-move-delete".to_string(),
            "move then delete".to_string(),
        );
        task.origin_project = crate::cloud::resolve_canonical_id(temp.path());
        store.add(&task).unwrap();
        queue.clear().unwrap();

        task.origin_project = Some("project-b".to_string());
        store.update(&task).unwrap();

        store.delete(&task.id).unwrap();

        let pending = queue.pending_for_team(TEST_TEAM, 10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].operation, SyncOperation::Delete);
        assert_eq!(
            pending[0].project_id,
            crate::cloud::resolve_canonical_id(temp.path())
        );
    }

    #[test]
    fn task_delete_personal_only_when_kill_switch_engaged() {
        let (temp, store) = create_team_store(Some(false));
        let queue = SyncQueue::open(temp.path()).unwrap();

        let task = Task::new("p-task-004".to_string(), "to-delete".to_string());
        store.add(&task).unwrap();
        queue.clear().unwrap();

        store.delete(&task.id).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 0, "kill-switch also silences delete fan-out");
    }

    // ── cas-f8e3: personal-project guard ─────────────────────────────────────

    /// Regression: a project with NO project-level `team_id` and NO
    /// `team_auto_promote = Some(true)` must never enqueue to the team queue,
    /// even when the user has a team configured at the user level.
    ///
    /// This covers the openclaw / penguinz promotion path: the user's
    /// `~/.cas/cloud.json` had `default_team_id` set, which caused
    /// `active_team_id()` to return a team UUID for ALL projects via the
    /// user-level fallback.  After cas-f8e3 the fallback only fires when
    /// `team_auto_promote = Some(true)` is present in the project config.
    #[test]
    fn f8e3_personal_project_no_team_id_never_enqueues_for_team() {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path();
        let inner = SqliteTaskStore::open(cas_dir).unwrap();
        inner.init().unwrap();
        let queue = SyncQueue::open(cas_dir).unwrap();
        queue.init().unwrap();

        // Personal project: no team_id, no team_auto_promote=Some(true).
        // Even if the user-level cloud.json has default_team_id, active_team_id()
        // returns None for this project (Step 1.5 guard fires).
        let cfg = CloudConfig::default(); // team_id=None, team_auto_promote=None
        let store = SyncingTaskStore::new(Arc::new(inner), Arc::new(queue))
            .with_cloud_config(Arc::new(cfg));

        let task = Task::new("f8e3-personal-001".to_string(), "personal task".to_string());
        store.add(&task).unwrap();

        let queue = SyncQueue::open(temp.path()).unwrap();
        let (personal, team) = queue_counts(&queue);
        assert_eq!(
            personal, 1,
            "personal project must enqueue to personal queue"
        );
        assert_eq!(
            team, 0,
            "cas-f8e3: personal project (no team_id) must NOT enqueue to team queue \
             — was the openclaw/penguinz promotion path"
        );
    }

    #[test]
    fn f8e3_personal_project_delete_never_fans_out_to_team() {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path();
        let inner = SqliteTaskStore::open(cas_dir).unwrap();
        inner.init().unwrap();
        let queue = SyncQueue::open(cas_dir).unwrap();
        queue.init().unwrap();

        let cfg = CloudConfig::default(); // personal: no team_id
        let store = SyncingTaskStore::new(Arc::new(inner), Arc::new(queue))
            .with_cloud_config(Arc::new(cfg));

        let task = Task::new("f8e3-personal-del".to_string(), "to-delete".to_string());
        store.add(&task).unwrap();

        // Clear upserts, then delete.
        let queue = SyncQueue::open(temp.path()).unwrap();
        queue.clear().unwrap();
        store.delete(&task.id).unwrap();

        let queue = SyncQueue::open(temp.path()).unwrap();
        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(
            team, 0,
            "cas-f8e3: personal project delete must NOT fan out to team queue"
        );
    }
}
