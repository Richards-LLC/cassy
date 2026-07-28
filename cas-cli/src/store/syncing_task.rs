//! Syncing task store wrapper
//!
//! Automatically queues task changes for cloud sync on add/update/delete.
//! When a team is configured and the task passes the T1 filter policy,
//! the write is dual-enqueued to both the personal queue and the team queue.

use std::sync::Arc;

use crate::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
use crate::store::share_policy::{eligible_for_team_task, resolve_team_id};
use crate::store::{Result, TaskStore};
use crate::types::{Dependency, DependencyType, Task, TaskStatus};

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

    fn queue_upsert(&self, task: &Task) {
        let payload = match serde_json::to_string(task) {
            Ok(p) => p,
            Err(_) => return,
        };

        let _ = self.queue.enqueue(
            EntityType::Task,
            &task.id,
            SyncOperation::Upsert,
            Some(&payload),
        );

        if let Some(team_id) = self.team_id.as_deref()
            && eligible_for_team_task(task)
        {
            let _ = self.queue.enqueue_for_team(
                EntityType::Task,
                &task.id,
                SyncOperation::Upsert,
                Some(&payload),
                team_id,
            );
        }
    }

    fn queue_delete(&self, id: &str) {
        let _ = self
            .queue
            .enqueue(EntityType::Task, id, SyncOperation::Delete, None);

        // See `share_policy` module docs: delete fans out unconditionally
        // when a team is configured.
        if let Some(team_id) = self.team_id.as_deref() {
            let _ = self.queue.enqueue_for_team(
                EntityType::Task,
                id,
                SyncOperation::Delete,
                None,
                team_id,
            );
        }
    }
}

impl TaskStore for SyncingTaskStore {
    fn init(&self) -> Result<()> {
        self.inner.init()
    }

    fn generate_id(&self) -> Result<String> {
        self.inner.generate_id()
    }

    fn add(&self, task: &Task) -> Result<()> {
        self.inner.add(task)?;
        self.queue_upsert(task);
        Ok(())
    }

    fn create_atomic(
        &self,
        task: &Task,
        blocked_by: &[String],
        epic_id: Option<&str>,
        created_by: Option<&str>,
    ) -> Result<()> {
        self.inner
            .create_atomic(task, blocked_by, epic_id, created_by)?;
        self.queue_upsert(task);
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Task> {
        self.inner.get(id)
    }

    fn update(&self, task: &Task) -> Result<()> {
        self.inner.update(task)?;
        // The inner store may enforce transition invariants while persisting
        // (for example, clearing a prior close-cycle branch anchor when a
        // Closed task returns to work). Queue the canonical stored row so a
        // stale caller-owned value cannot be synced back over that invariant.
        let persisted = self.inner.get(&task.id)?;
        self.queue_upsert(&persisted);
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.inner.delete(id)?;
        self.queue_delete(id);
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

    // Dependency operations - don't sync these as they're derived from task relationships
    fn add_dependency(&self, dep: &Dependency) -> Result<()> {
        self.inner.add_dependency(dep)
    }

    fn remove_dependency(&self, from_id: &str, to_id: &str) -> Result<()> {
        self.inner.remove_dependency(from_id, to_id)
    }

    fn remove_dependency_of_type(
        &self,
        from_id: &str,
        to_id: &str,
        dep_type: DependencyType,
    ) -> Result<bool> {
        self.inner.remove_dependency_of_type(from_id, to_id, dep_type)
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
    use crate::store::SqliteTaskStore;
    use crate::store::syncing_task::*;
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
        assert_eq!(personal, 1, "personal project must enqueue to personal queue");
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
