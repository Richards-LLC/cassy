//! Task store wrapper that hides quarantined rows from every list surface.
//!
//! # Why a wrapper (GH #701 / cas-4342)
//!
//! Quarantine has to hold on the board — `task ready`, `task list`, the MCP
//! surfaces, the TUI — and there is no single call site for those; they all
//! reach the store. Wrapping the store once means a new list surface is
//! covered on the day it is written, instead of being one more place somebody
//! has to remember to filter.
//!
//! Only the *list* methods filter. `get` deliberately does not: a quarantined
//! row must stay inspectable by id, or the operator cannot review what was
//! quarantined and the decision would not be reversible in any practical
//! sense. Writes pass straight through — the point of the ledger is that the
//! task row itself is never touched.
//!
//! The scoped list variants (`list_ready_scoped`, `list_blocked_scoped`) are
//! trait defaults built on `list_ready` / `list_blocked`, so filtering the
//! base methods covers them too.

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::cloud::{QUARANTINE_TASK, SyncQueue};
use crate::store::{Result, TaskStore};
use crate::types::{Dependency, DependencyType, Task, TaskStatus};

/// Hides quarantined task rows from list surfaces.
pub struct QuarantineFilteringTaskStore {
    inner: Arc<dyn TaskStore>,
    queue: Arc<SyncQueue>,
}

impl QuarantineFilteringTaskStore {
    pub fn new(inner: Arc<dyn TaskStore>, queue: Arc<SyncQueue>) -> Self {
        Self { inner, queue }
    }

    /// Currently quarantined task ids.
    ///
    /// A read failure yields an empty set on purpose: the ledger is a
    /// *suppression*, and failing it open shows the operator more rows than
    /// they asked for, never fewer. Failing closed would hide the whole board
    /// behind one unreadable side table.
    fn quarantined(&self) -> BTreeSet<String> {
        self.queue
            .quarantined_ids(QUARANTINE_TASK)
            .unwrap_or_default()
    }
}

impl TaskStore for QuarantineFilteringTaskStore {
    fn init(&self) -> Result<()> {
        self.inner.init()
    }

    fn project_id(&self) -> Option<&str> {
        self.inner.project_id()
    }

    fn generate_id(&self) -> Result<String> {
        self.inner.generate_id()
    }

    fn add(&self, task: &Task) -> Result<()> {
        self.inner.add(task)
    }

    fn create_atomic(
        &self,
        task: &Task,
        blocked_by: &[String],
        epic_id: Option<&str>,
        created_by: Option<&str>,
    ) -> Result<()> {
        self.inner
            .create_atomic(task, blocked_by, epic_id, created_by)
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
        self.inner.update(task)
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.inner.delete(id)
    }

    fn list(&self, status: Option<TaskStatus>) -> Result<Vec<Task>> {
        let hidden = self.quarantined();
        Ok(self
            .inner
            .list(status)?
            .into_iter()
            .filter(|task| !hidden.contains(&task.id))
            .collect())
    }

    fn list_ready(&self) -> Result<Vec<Task>> {
        let hidden = self.quarantined();
        Ok(self
            .inner
            .list_ready()?
            .into_iter()
            .filter(|task| !hidden.contains(&task.id))
            .collect())
    }

    fn list_blocked(&self) -> Result<Vec<(Task, Vec<Task>)>> {
        let hidden = self.quarantined();
        Ok(self
            .inner
            .list_blocked()?
            .into_iter()
            .filter(|(task, _)| !hidden.contains(&task.id))
            .collect())
    }

    fn list_pending_verification(&self) -> Result<Vec<Task>> {
        let hidden = self.quarantined();
        Ok(self
            .inner
            .list_pending_verification()?
            .into_iter()
            .filter(|task| !hidden.contains(&task.id))
            .collect())
    }

    fn list_pending_worktree_merge(&self) -> Result<Vec<Task>> {
        let hidden = self.quarantined();
        Ok(self
            .inner
            .list_pending_worktree_merge()?
            .into_iter()
            .filter(|task| !hidden.contains(&task.id))
            .collect())
    }

    fn close(&self) -> Result<()> {
        self.inner.close()
    }

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
        self.inner
            .remove_dependency_of_type(from_id, to_id, dep_type)
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
    use super::*;
    use crate::store::SqliteTaskStore;
    use tempfile::TempDir;

    fn store() -> (TempDir, Arc<SyncQueue>, QuarantineFilteringTaskStore) {
        let temp = TempDir::new().unwrap();
        let inner = SqliteTaskStore::open(temp.path()).unwrap();
        inner.init().unwrap();
        let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());
        queue.init().unwrap();
        let filtered = QuarantineFilteringTaskStore::new(Arc::new(inner), Arc::clone(&queue));
        (temp, queue, filtered)
    }

    #[test]
    fn quarantined_rows_leave_the_board_but_stay_readable_and_reversible() {
        let (_temp, queue, store) = store();
        store
            .add(&Task::new("cas-keep".to_string(), "Native work".to_string()))
            .unwrap();
        store
            .add(&Task::new(
                "cas-hide".to_string(),
                "Unattributed replica".to_string(),
            ))
            .unwrap();

        assert_eq!(store.list(None).unwrap().len(), 2);
        assert_eq!(store.list_ready().unwrap().len(), 2);

        assert!(
            queue
                .quarantine_row(QUARANTINE_TASK, "cas-hide", "unattributed cloud row")
                .unwrap()
        );

        let listed: Vec<String> = store.list(None).unwrap().into_iter().map(|t| t.id).collect();
        assert_eq!(listed, vec!["cas-keep".to_string()]);
        let ready: Vec<String> = store
            .list_ready()
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ready, vec!["cas-keep".to_string()]);

        // Readable by id: quarantine is a suppression, not a deletion, and an
        // operator must be able to inspect what was hidden.
        assert_eq!(store.get("cas-hide").unwrap().title, "Unattributed replica");

        // Reversible.
        assert!(queue.release_quarantined_row(QUARANTINE_TASK, "cas-hide").unwrap());
        assert_eq!(store.list(None).unwrap().len(), 2);
    }

    #[test]
    fn re_quarantining_is_idempotent_and_release_reports_honestly() {
        let (_temp, queue, _store) = store();
        assert!(
            queue
                .quarantine_row(QUARANTINE_TASK, "cas-hide", "unattributed cloud row")
                .unwrap()
        );
        assert!(
            !queue
                .quarantine_row(QUARANTINE_TASK, "cas-hide", "second run")
                .unwrap(),
            "a repeated fix run must be a no-op, not a re-stamp"
        );
        assert_eq!(queue.quarantined_count(QUARANTINE_TASK).unwrap(), 1);
        assert_eq!(
            queue.quarantined_rows(QUARANTINE_TASK).unwrap()[0].reason,
            "unattributed cloud row",
            "the original decision's reason must survive a repeat run"
        );

        assert!(queue.release_quarantined_row(QUARANTINE_TASK, "cas-hide").unwrap());
        assert!(
            !queue.release_quarantined_row(QUARANTINE_TASK, "cas-hide").unwrap(),
            "releasing nothing must not claim to have released something"
        );
        assert_eq!(queue.quarantined_count(QUARANTINE_TASK).unwrap(), 0);
    }
}
