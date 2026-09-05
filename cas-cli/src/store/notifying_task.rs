//! Notifying task store wrapper
//!
//! Emits notification events on task add/update/close for TUI display.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use crate::config::NotificationConfig;
use crate::notifications::{NotificationEvent, get_global_notifier};
use crate::store::{Result, TaskStore};
use crate::types::{Dependency, DependencyType, Task, TaskStatus};
use serde_json::Value;

/// A task store wrapper that emits notification events
pub struct NotifyingTaskStore {
    inner: Arc<dyn TaskStore>,
    config: NotificationConfig,
}

impl NotifyingTaskStore {
    /// Create a new notifying task store
    pub fn new(inner: Arc<dyn TaskStore>, config: NotificationConfig) -> Self {
        Self { inner, config }
    }

    fn notify_created(&self, task: &Task) {
        if self.config.enabled && self.config.tasks.on_created {
            if let Some(notifier) = get_global_notifier() {
                notifier.notify(NotificationEvent::task_created(&task.id, &task.title));
            }
        }
    }

    fn notify_updated(&self, task: &Task, old_status: Option<TaskStatus>) {
        // Check for status transitions
        if let Some(old) = old_status {
            // Started: was not in_progress, now is in_progress
            if old != TaskStatus::InProgress
                && task.status == TaskStatus::InProgress
                && self.config.tasks.on_started
            {
                if let Some(notifier) = get_global_notifier() {
                    notifier.notify(NotificationEvent::task_started(&task.id, &task.title));
                }
                return;
            }

            // Closed: was not closed, now is closed
            if old != TaskStatus::Closed
                && task.status == TaskStatus::Closed
                && self.config.tasks.on_closed
            {
                if let Some(notifier) = get_global_notifier() {
                    notifier.notify(NotificationEvent::task_closed(&task.id, &task.title));
                }
                return;
            }
        }

        // Generic update notification
        if self.config.enabled && self.config.tasks.on_updated {
            if let Some(notifier) = get_global_notifier() {
                notifier.notify(NotificationEvent::task_updated(&task.id, &task.title));
            }
        }
    }
}

impl TaskStore for NotifyingTaskStore {
    fn init(&self) -> Result<()> {
        self.inner.init()
    }

    fn generate_id(&self) -> Result<String> {
        self.inner.generate_id()
    }

    fn project_id(&self) -> Option<&str> {
        self.inner.project_id()
    }

    fn add(&self, task: &Task) -> Result<()> {
        self.inner.add(task)?;
        self.notify_created(task);
        Ok(())
    }

    fn add_with_mutation_receipt(&self, task: &Task, receipt_id: &str) -> Result<()> {
        self.inner.add_with_mutation_receipt(task, receipt_id)?;
        self.notify_created(task);
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
        self.notify_created(task);
        Ok(())
    }

    fn create_atomic_with_mutation_receipt(
        &self,
        task: &Task,
        blocked_by: &[String],
        epic_id: Option<&str>,
        created_by: Option<&str>,
        receipt_id: &str,
    ) -> Result<()> {
        self.inner.create_atomic_with_mutation_receipt(
            task, blocked_by, epic_id, created_by, receipt_id,
        )?;
        self.notify_created(task);
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
        // Get old status for transition detection
        let old_status = self.inner.get(&task.id).ok().map(|t| t.status);

        let persisted_at = self.inner.update(task)?;
        self.notify_updated(task, old_status);
        Ok(persisted_at)
    }

    fn update_with_mutation_receipt(&self, task: &Task, receipt_id: &str) -> Result<DateTime<Utc>> {
        let old_status = self.inner.get(&task.id).ok().map(|t| t.status);
        let persisted_at = self.inner.update_with_mutation_receipt(task, receipt_id)?;
        self.notify_updated(task, old_status);
        Ok(persisted_at)
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.inner.delete(id)
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

    // Dependency operations - delegate without notifications
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
    use crate::store::notifying_task::*;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, NotifyingTaskStore) {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path();

        let inner = SqliteTaskStore::open(cas_dir).unwrap();
        inner.init().unwrap();

        let config = NotificationConfig::default();
        let store = NotifyingTaskStore::new(Arc::new(inner), config);
        (temp, store)
    }

    #[test]
    fn test_store_operations_work() {
        let (_temp, store) = create_test_store();

        // Test add
        let task = Task::new("task-001".to_string(), "Test task".to_string());
        store.add(&task).unwrap();

        // Test get
        let fetched = store.get("task-001").unwrap();
        assert_eq!(fetched.title, "Test task");

        // Test update (status change)
        let mut updated_task = task.clone();
        updated_task.status = TaskStatus::InProgress;
        store.update(&updated_task).unwrap();

        let fetched = store.get("task-001").unwrap();
        assert_eq!(fetched.status, TaskStatus::InProgress);

        // Test close
        updated_task.status = TaskStatus::Closed;
        store.update(&updated_task).unwrap();

        let fetched = store.get("task-001").unwrap();
        assert_eq!(fetched.status, TaskStatus::Closed);

        // Test delete
        store.delete("task-001").unwrap();
        assert!(store.get("task-001").is_err());
    }
}
