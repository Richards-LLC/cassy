use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use crate::store::TaskStore;
use crate::store::mock::id_counter::IdCounter;
use crate::types::{Dependency, DependencyType, Task, TaskStatus, TaskType};
use cas_store::{Result, StoreError};
use chrono::{DateTime, Utc};

/// In-memory mock implementation of the TaskStore trait.
#[derive(Debug)]
pub struct MockTaskStore {
    tasks: RwLock<HashMap<String, Task>>,
    dependencies: RwLock<Vec<Dependency>>,
    id_counter: IdCounter,
    error_on_next: RwLock<Option<StoreError>>,
}

impl Default for MockTaskStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MockTaskStore {
    /// Create a new empty mock task store.
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            dependencies: RwLock::new(Vec::new()),
            id_counter: IdCounter::default(),
            error_on_next: RwLock::new(None),
        }
    }

    /// Create with pre-populated tasks.
    pub fn with_tasks(tasks: Vec<Task>) -> Self {
        let store = Self::new();
        {
            let mut map = store.tasks.write().unwrap();
            for task in tasks {
                map.insert(task.id.clone(), task);
            }
        }
        store
    }

    /// Inject an error.
    pub fn inject_error(&self, error: StoreError) {
        *self.error_on_next.write().unwrap() = Some(error);
    }

    fn check_error(&self) -> Result<()> {
        let mut error = self.error_on_next.write().unwrap();
        if let Some(value) = error.take() {
            return Err(value);
        }
        Ok(())
    }

    /// Get count (for testing).
    pub fn len(&self) -> usize {
        self.tasks.read().unwrap().len()
    }

    /// Check if empty (for testing).
    pub fn is_empty(&self) -> bool {
        self.tasks.read().unwrap().is_empty()
    }
}

impl TaskStore for MockTaskStore {
    fn init(&self) -> Result<()> {
        self.check_error()
    }

    fn generate_id(&self) -> Result<String> {
        self.check_error()?;
        let counter = self.id_counter.next();
        Ok(format!("cas-{counter:04x}"))
    }

    fn add(&self, task: &Task) -> Result<()> {
        self.check_error()?;
        let mut tasks = self.tasks.write().unwrap();
        if tasks.contains_key(&task.id) {
            return Err(StoreError::EntryExists(task.id.clone()));
        }
        tasks.insert(task.id.clone(), task.clone());
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Task> {
        self.check_error()?;
        let tasks = self.tasks.read().unwrap();
        tasks
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    fn update(&self, task: &Task) -> Result<DateTime<Utc>> {
        self.check_error()?;
        let mut tasks = self.tasks.write().unwrap();
        if !tasks.contains_key(&task.id) {
            return Err(StoreError::NotFound(task.id.clone()));
        }
        // cas-ec74: mirror SqliteTaskStore exactly — `updated_at` is store-owned,
        // so re-stamp from our own clock instead of honouring the caller. The
        // mock used to persist `task.updated_at` verbatim, which meant no test
        // written against it could ever observe (or catch) the production
        // producer/consumer timestamp mismatch.
        let now = Utc::now();
        let mut stored = task.clone();
        stored.updated_at = now;
        tasks.insert(task.id.clone(), stored);
        Ok(now)
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.check_error()?;
        let mut tasks = self.tasks.write().unwrap();
        tasks
            .remove(id)
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        let mut deps = self.dependencies.write().unwrap();
        deps.retain(|dependency| dependency.from_id != id && dependency.to_id != id);
        Ok(())
    }

    fn list(&self, status: Option<TaskStatus>) -> Result<Vec<Task>> {
        self.check_error()?;
        let tasks = self.tasks.read().unwrap();
        let mut list: Vec<Task> = tasks
            .values()
            .filter(|task| status.is_none() || Some(task.status) == status)
            .cloned()
            .collect();
        list.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| b.created_at.cmp(&a.created_at))
        });
        Ok(list)
    }

    fn list_ready(&self) -> Result<Vec<Task>> {
        self.check_error()?;
        let tasks = self.tasks.read().unwrap();
        let deps = self.dependencies.read().unwrap();

        let blocked_ids: HashSet<_> = deps
            .iter()
            .filter(|dependency| dependency.dep_type == DependencyType::Blocks)
            .filter(|dependency| {
                tasks
                    .get(&dependency.to_id)
                    .map(|task| task.status != TaskStatus::Closed)
                    .unwrap_or(false)
            })
            .map(|dependency| dependency.from_id.clone())
            .collect();

        let mut list: Vec<Task> = tasks
            .values()
            .filter(|task| {
                (task.status == TaskStatus::Open || task.status == TaskStatus::InProgress)
                    && !blocked_ids.contains(&task.id)
            })
            .cloned()
            .collect();

        list.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| b.created_at.cmp(&a.created_at))
        });
        Ok(list)
    }

    fn list_blocked(&self) -> Result<Vec<(Task, Vec<Task>)>> {
        self.check_error()?;
        let tasks = self.tasks.read().unwrap();
        let deps = self.dependencies.read().unwrap();

        let mut result = Vec::new();
        for task in tasks.values() {
            if task.status == TaskStatus::Closed {
                continue;
            }

            let blockers: Vec<Task> = deps
                .iter()
                .filter(|dependency| {
                    dependency.from_id == task.id && dependency.dep_type == DependencyType::Blocks
                })
                .filter_map(|dependency| {
                    tasks
                        .get(&dependency.to_id)
                        .filter(|candidate| candidate.status != TaskStatus::Closed)
                        .cloned()
                })
                .collect();

            if !blockers.is_empty() {
                result.push((task.clone(), blockers));
            }
        }

        result.sort_by(|a, b| a.0.priority.cmp(&b.0.priority));
        Ok(result)
    }

    fn list_pending_verification(&self) -> Result<Vec<Task>> {
        self.check_error()?;
        let tasks = self.tasks.read().unwrap();
        Ok(tasks.values().filter(|t| t.pending_verification).cloned().collect())
    }

    fn list_pending_worktree_merge(&self) -> Result<Vec<Task>> {
        self.check_error()?;
        let tasks = self.tasks.read().unwrap();
        Ok(tasks.values().filter(|t| t.pending_worktree_merge).cloned().collect())
    }

    fn close(&self) -> Result<()> {
        self.check_error()
    }

    fn add_dependency(&self, dep: &Dependency) -> Result<()> {
        self.check_error()?;

        let tasks = self.tasks.read().unwrap();
        if !tasks.contains_key(&dep.from_id) {
            return Err(StoreError::NotFound(dep.from_id.clone()));
        }
        if !tasks.contains_key(&dep.to_id) {
            return Err(StoreError::NotFound(dep.to_id.clone()));
        }
        drop(tasks);

        if self.would_create_cycle(&dep.from_id, &dep.to_id)? {
            return Err(StoreError::CyclicDependency(
                dep.from_id.clone(),
                dep.to_id.clone(),
            ));
        }

        let mut deps = self.dependencies.write().unwrap();
        deps.push(dep.clone());
        Ok(())
    }

    fn remove_dependency(&self, from_id: &str, to_id: &str) -> Result<()> {
        self.check_error()?;
        let mut deps = self.dependencies.write().unwrap();
        let len_before = deps.len();
        deps.retain(|dependency| !(dependency.from_id == from_id && dependency.to_id == to_id));
        if deps.len() == len_before {
            return Err(StoreError::NotFound(format!("{from_id} -> {to_id}")));
        }
        Ok(())
    }

    fn remove_dependency_of_type(
        &self,
        from_id: &str,
        to_id: &str,
        dep_type: DependencyType,
    ) -> Result<bool> {
        self.check_error()?;
        let mut deps = self.dependencies.write().unwrap();
        let len_before = deps.len();
        deps.retain(|dependency| {
            !(dependency.from_id == from_id
                && dependency.to_id == to_id
                && dependency.dep_type == dep_type)
        });
        Ok(deps.len() < len_before)
    }

    fn get_dependencies(&self, task_id: &str) -> Result<Vec<Dependency>> {
        self.check_error()?;
        let deps = self.dependencies.read().unwrap();
        Ok(deps
            .iter()
            .filter(|dependency| dependency.from_id == task_id)
            .cloned()
            .collect())
    }

    fn get_dependents(&self, task_id: &str) -> Result<Vec<Dependency>> {
        self.check_error()?;
        let deps = self.dependencies.read().unwrap();
        Ok(deps
            .iter()
            .filter(|dependency| dependency.to_id == task_id)
            .cloned()
            .collect())
    }

    fn get_blockers(&self, task_id: &str) -> Result<Vec<Task>> {
        self.check_error()?;
        let tasks = self.tasks.read().unwrap();
        let deps = self.dependencies.read().unwrap();

        Ok(deps
            .iter()
            .filter(|dependency| {
                dependency.from_id == task_id && dependency.dep_type == DependencyType::Blocks
            })
            .filter_map(|dependency| tasks.get(&dependency.to_id).cloned())
            .collect())
    }

    fn would_create_cycle(&self, from_id: &str, to_id: &str) -> Result<bool> {
        self.check_error()?;

        let deps = self.dependencies.read().unwrap();

        let mut visited = HashSet::new();
        let mut stack = vec![to_id.to_string()];

        while let Some(current) = stack.pop() {
            if current == from_id {
                return Ok(true);
            }
            if visited.insert(current.clone()) {
                for dependency in deps.iter() {
                    if dependency.from_id == current
                        && dependency.dep_type == DependencyType::Blocks
                    {
                        stack.push(dependency.to_id.clone());
                    }
                }
            }
        }

        Ok(false)
    }

    fn list_dependencies(&self, dep_type: Option<DependencyType>) -> Result<Vec<Dependency>> {
        self.check_error()?;
        let deps = self.dependencies.read().unwrap();
        Ok(deps
            .iter()
            .filter(|dependency| dep_type.is_none() || Some(dependency.dep_type) == dep_type)
            .cloned()
            .collect())
    }

    fn get_subtasks(&self, parent_id: &str) -> Result<Vec<Task>> {
        self.check_error()?;
        let tasks = self.tasks.read().unwrap();
        let deps = self.dependencies.read().unwrap();

        let subtask_ids: Vec<String> = deps
            .iter()
            .filter(|dependency| {
                dependency.to_id == parent_id && dependency.dep_type == DependencyType::ParentChild
            })
            .map(|dependency| dependency.from_id.clone())
            .collect();

        Ok(tasks
            .values()
            .filter(|task| subtask_ids.contains(&task.id))
            .cloned()
            .collect())
    }

    fn get_sibling_notes(
        &self,
        epic_id: &str,
        exclude_task_id: &str,
    ) -> Result<Vec<(String, String, String)>> {
        self.check_error()?;
        let tasks = self.tasks.read().unwrap();
        let deps = self.dependencies.read().unwrap();

        let subtask_ids: Vec<String> = deps
            .iter()
            .filter(|dependency| {
                dependency.to_id == epic_id && dependency.dep_type == DependencyType::ParentChild
            })
            .map(|dependency| dependency.from_id.clone())
            .collect();

        Ok(tasks
            .values()
            .filter(|task| {
                subtask_ids.contains(&task.id)
                    && task.id != exclude_task_id
                    && !task.notes.is_empty()
            })
            .map(|task| (task.id.clone(), task.title.clone(), task.notes.clone()))
            .collect())
    }

    fn get_parent_epic(&self, task_id: &str) -> Result<Option<Task>> {
        self.check_error()?;
        let tasks = self.tasks.read().unwrap();
        let deps = self.dependencies.read().unwrap();

        for dependency in deps.iter() {
            if dependency.from_id == task_id && dependency.dep_type == DependencyType::ParentChild {
                if let Some(parent) = tasks.get(&dependency.to_id) {
                    if parent.task_type == TaskType::Epic {
                        return Ok(Some(parent.clone()));
                    }
                }
            }
        }
        Ok(None)
    }
}
