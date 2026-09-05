//! Syncing task store wrapper
//!
//! Automatically queues task changes for cloud sync on add/update/delete.
//! When a team is configured and the task passes the T1 filter policy,
//! the write is dual-enqueued to both the personal queue and the team queue.

use std::sync::Arc;

use crate::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue, TaskSyncIntent};
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

/// A task store wrapper that queues changes for cloud sync
pub struct SyncingTaskStore {
    inner: Arc<dyn TaskStore>,
    queue: Arc<SyncQueue>,
    /// Pre-resolved team UUID for dual-enqueue; see
    /// `SyncingEntryStore::team_id` for the protocol. `None` preserves
    /// personal-only behaviour.
    team_id: Option<Arc<str>>,
}

impl SyncingTaskStore {
    /// Create a new syncing task store (personal queue only).
    pub fn new(inner: Arc<dyn TaskStore>, queue: Arc<SyncQueue>) -> Self {
        Self {
            inner,
            queue,
            team_id: None,
        }
    }

    /// Attach a cloud config for team auto-promotion. See
    /// `SyncingEntryStore::with_cloud_config` for the protocol.
    #[must_use]
    pub fn with_cloud_config(mut self, cloud_config: Arc<CloudConfig>) -> Self {
        self.team_id = resolve_team_id(&cloud_config);
        self
    }

    fn stage_upsert(
        &self,
        task: &Task,
        operation: &str,
        previous_updated_at: Option<&str>,
        old_project_id: Option<&str>,
    ) -> Result<TaskSyncIntent> {
        let team_id = self
            .team_id
            .as_deref()
            .filter(|_| eligible_for_team_task(task));
        self.queue
            .stage_task_sync_intent(
                &task.id,
                operation,
                previous_updated_at,
                team_id,
                old_project_id,
                task.scope == Scope::Global,
            )
            .map_err(queue_error_before_local_commit)
    }

    fn fulfill_upsert(&self, intent: &TaskSyncIntent, task: &Task) -> Result<()> {
        let payload = serde_json::to_string(task)
            .map_err(|error| degraded_sync_error(intent, error.to_string()))?;
        let current_project_id = task
            .origin_project
            .as_deref()
            .filter(|project_id| !project_id.trim().is_empty());
        self.queue
            .fulfill_task_sync_intent(intent, &payload, current_project_id)
            .map_err(|error| degraded_sync_error(intent, error.to_string()))
    }

    fn cancel_staged_after_local_failure(&self, intent: &TaskSyncIntent) {
        // The local error remains authoritative. If cancellation itself fails,
        // restart reconciliation reloads the canonical row (or discards the
        // marker when the add never created one), so retaining the marker is
        // safe and preferable to masking the real local failure.
        let _ = self.queue.cancel_task_sync_intent(intent.id);
    }

    pub(crate) fn reconcile_pending_task_sync(&self) -> Result<()> {
        let intents = self
            .queue
            .pending_task_sync_intents()
            .map_err(queue_error_before_local_commit)?;
        for intent in intents {
            let mut task = match self.inner.get(&intent.entity_id) {
                Ok(task) => task,
                Err(StoreError::TaskNotFound(_)) | Err(StoreError::NotFound(_)) => {
                    self.queue
                        .cancel_task_sync_intent(intent.id)
                        .map_err(queue_error_before_local_commit)?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if intent.operation == "update"
                && intent.previous_updated_at.as_deref()
                    == Some(task.updated_at.to_rfc3339().as_str())
            {
                // The process exited after staging but before the local
                // update changed the store-owned timestamp. There was no
                // committed local mutation to repair.
                self.queue
                    .cancel_task_sync_intent(intent.id)
                    .map_err(queue_error_before_local_commit)?;
                continue;
            }
            if intent.global_scope {
                task.scope = Scope::Global;
                task.origin_project = None;
            }
            self.fulfill_upsert(&intent, &task)?;
        }
        Ok(())
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

    fn queue_delete(&self, id: &str, project_id: Option<&str>) {
        let _ = self
            .queue
            .enqueue(EntityType::Task, id, SyncOperation::Delete, None);

        // See `share_policy` module docs: delete fans out unconditionally
        // when a team is configured.
        if let Some(team_id) = self.team_id.as_deref() {
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
        let origin_project = match from_task.scope {
            Scope::Global => None,
            Scope::Project => from_task
                .origin_project
                .as_deref()
                .or(self.inner.project_id()),
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
        let _ = self.queue.enqueue(
            EntityType::TaskDependency,
            &entity_id,
            SyncOperation::Upsert,
            Some(&payload),
        );

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

    fn queue_dependency_delete(&self, dep: &Dependency) {
        let entity_id = dependency_entity_id(dep);
        let _ = self.queue.enqueue(
            EntityType::TaskDependency,
            &entity_id,
            SyncOperation::Delete,
            None,
        );

        // A delete must fan out when a team is configured, even when the
        // source task is no longer available to evaluate the promotion
        // predicate. This prevents stale cloud edges from surviving local
        // task/dependency deletion.
        if let Some(team_id) = self.team_id.as_deref() {
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
        self.reconcile_pending_task_sync()
    }

    fn generate_id(&self) -> Result<String> {
        self.inner.generate_id()
    }

    fn project_id(&self) -> Option<&str> {
        self.inner.project_id()
    }

    fn add(&self, task: &Task) -> Result<()> {
        let intent = self.stage_upsert(task, "add", None, None)?;
        if let Err(error) = self.inner.add(task) {
            self.cancel_staged_after_local_failure(&intent);
            return Err(error);
        }
        let persisted = self
            .persisted_for_queue(task)
            .map_err(|error| degraded_sync_error(&intent, error.to_string()))?;
        self.fulfill_upsert(&intent, &persisted)?;
        Ok(())
    }

    fn create_atomic(
        &self,
        task: &Task,
        blocked_by: &[String],
        epic_id: Option<&str>,
        created_by: Option<&str>,
    ) -> Result<()> {
        let intent = self.stage_upsert(task, "create_atomic", None, None)?;
        if let Err(error) = self
            .inner
            .create_atomic(task, blocked_by, epic_id, created_by)
        {
            self.cancel_staged_after_local_failure(&intent);
            return Err(error);
        }
        let persisted = self
            .persisted_for_queue(task)
            .map_err(|error| degraded_sync_error(&intent, error.to_string()))?;
        let task_sync_result = self.fulfill_upsert(&intent, &persisted);
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
        let previous = self.inner.get(&task.id)?;
        let old_project_id = previous
            .origin_project
            .as_deref()
            .filter(|project_id| !project_id.trim().is_empty());
        let previous_updated_at = previous.updated_at.to_rfc3339();
        let intent =
            self.stage_upsert(task, "update", Some(&previous_updated_at), old_project_id)?;
        let persisted_at = match self.inner.update(task) {
            Ok(persisted_at) => persisted_at,
            Err(error) => {
                self.cancel_staged_after_local_failure(&intent);
                return Err(error);
            }
        };
        // The inner store may enforce transition invariants while persisting
        // (for example, clearing a prior close-cycle branch anchor when a
        // Closed task returns to work). Queue the canonical stored row so a
        // stale caller-owned value cannot be synced back over that invariant.
        let mut persisted = self
            .inner
            .get(&task.id)
            .map_err(|error| degraded_sync_error(&intent, error.to_string()))?;
        persisted.scope = task.scope;
        if task.scope == Scope::Global {
            persisted.origin_project = None;
        }
        self.fulfill_upsert(&intent, &persisted)?;
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
        self.queue_delete(id, project_id);
        for dep in &dependencies {
            self.queue_dependency_delete(dep);
        }
        Ok(())
    }

    fn list(&self, status: Option<TaskStatus>) -> Result<Vec<Task>> {
        self.inner.list(status)
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
        let previous = self
            .inner
            .get_dependencies(&dep.from_id)?
            .into_iter()
            .find(|existing| existing.to_id == dep.to_id);
        self.inner.add_dependency(dep)?;
        if let Some(previous) = previous.filter(|previous| previous.dep_type != dep.dep_type) {
            self.queue_dependency_delete(&previous);
        }
        let from_task = self.inner.get(&dep.from_id)?;
        self.queue_dependency_upsert(dep, &from_task);
        Ok(())
    }

    fn remove_dependency(&self, from_id: &str, to_id: &str) -> Result<()> {
        let dependencies: Vec<Dependency> = self
            .inner
            .get_dependencies(from_id)?
            .into_iter()
            .filter(|dep| dep.to_id == to_id)
            .collect();
        self.inner.remove_dependency(from_id, to_id)?;
        for dep in &dependencies {
            self.queue_dependency_delete(dep);
        }
        Ok(())
    }

    fn remove_dependency_of_type(
        &self,
        from_id: &str,
        to_id: &str,
        dep_type: DependencyType,
    ) -> Result<bool> {
        let removed = self
            .inner
            .remove_dependency_of_type(from_id, to_id, dep_type)?;
        if removed {
            self.queue_dependency_delete(&Dependency {
                from_id: from_id.to_string(),
                to_id: to_id.to_string(),
                dep_type,
                created_at: Utc::now(),
                created_by: None,
            });
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
    use crate::store::syncing_task::*;
    use crate::store::{SqliteTaskStore, StoreError};
    use std::path::Path;
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

        let pending = queue.pending_for_entity_type(Some(EntityType::TaskDependency), 10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].entity_id, "task-dep-from:task-dep-to:blocks");
        assert_eq!(pending[0].operation, SyncOperation::Upsert);
        let payload: serde_json::Value = serde_json::from_str(pending[0].payload.as_deref().unwrap()).unwrap();
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

        assert!(store
            .remove_dependency_of_type(&from.id, &to.id, DependencyType::Related)
            .unwrap());

        let pending = queue.pending_for_entity_type(Some(EntityType::TaskDependency), 10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].entity_id, "task-remove-from:task-remove-to:related");
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
    fn task_origin_project_move_queues_old_delete_before_new_upsert() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut task = Task::new("p-task-move-001".to_string(), "move me".to_string());
        task.origin_project = Some("project-a".to_string());
        store.add(&task).unwrap();
        queue.clear().unwrap();

        task.origin_project = Some("project-b".to_string());
        store.update(&task).unwrap();

        let pending = queue.pending_for_team(TEST_TEAM, 10, 5).unwrap();
        assert_eq!(pending.len(), 2, "a move must retain both team operations");
        assert_eq!(pending[0].operation, SyncOperation::Delete);
        assert_eq!(pending[0].entity_id, task.id);
        assert_eq!(pending[0].project_id.as_deref(), Some("project-a"));
        assert_eq!(pending[1].operation, SyncOperation::Upsert);
        assert_eq!(pending[1].entity_id, task.id);
        assert_eq!(pending[1].project_id.as_deref(), Some("project-b"));
        assert!(
            pending[1]
                .payload
                .as_deref()
                .is_some_and(|payload| payload.contains("\"origin_project\":\"project-b\""))
        );
    }

    #[test]
    fn task_origin_project_move_removes_legacy_unkeyed_team_upsert() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut task = Task::new("p-task-move-legacy".to_string(), "move me".to_string());
        task.origin_project = Some("project-a".to_string());
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
        assert_eq!(pending.len(), 2, "a move must replace stale generic upserts");
        assert_eq!(pending[0].operation, SyncOperation::Delete);
        assert_eq!(pending[0].project_id.as_deref(), Some("project-a"));
        assert_eq!(pending[1].operation, SyncOperation::Upsert);
        assert_eq!(pending[1].project_id.as_deref(), Some("project-b"));
    }

    #[test]
    fn task_origin_project_move_later_edit_stays_on_new_owner_key() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut task = Task::new("p-task-move-002".to_string(), "move me".to_string());
        task.origin_project = Some("project-a".to_string());
        store.add(&task).unwrap();
        queue.clear().unwrap();

        task.origin_project = Some("project-b".to_string());
        store.update(&task).unwrap();
        queue.clear().unwrap();

        task.title = "edited after move".to_string();
        store.update(&task).unwrap();

        let pending = queue.pending_for_team(TEST_TEAM, 10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].operation, SyncOperation::Upsert);
        assert_eq!(pending[0].project_id.as_deref(), Some("project-b"));
    }

    #[test]
    fn task_delete_after_origin_project_move_targets_current_owner_key() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut task = Task::new(
            "p-task-move-delete".to_string(),
            "move then delete".to_string(),
        );
        task.origin_project = Some("project-a".to_string());
        store.add(&task).unwrap();
        queue.clear().unwrap();

        task.origin_project = Some("project-b".to_string());
        store.update(&task).unwrap();
        queue.clear().unwrap();

        store.delete(&task.id).unwrap();

        let pending = queue.pending_for_team(TEST_TEAM, 10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].operation, SyncOperation::Delete);
        assert_eq!(pending[0].project_id.as_deref(), Some("project-b"));
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
