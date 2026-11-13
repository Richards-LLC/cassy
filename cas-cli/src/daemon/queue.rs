//! Priority-based task queue for daemon maintenance operations
//!
//! This module provides a priority queue for scheduling maintenance tasks
//! with coordination between different types of background work.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Types of maintenance tasks the daemon can perform
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskType {
    /// Generate embeddings for pending entries (highest priority for responsiveness)
    GenerateEmbeddings,
    /// Process pending observations into memories
    ProcessObservations,
    /// Consolidate related memories
    ConsolidateMemories,
    /// Apply memory decay (tier changes)
    ApplyDecay,
    /// Prune stale/archived entries
    PruneEntries,
    /// Rebuild search indexes
    RebuildIndex,
    /// Sync to cloud (if enabled)
    CloudSync,
}

impl TaskType {
    /// Get default priority for this task type (lower = higher priority)
    pub fn default_priority(&self) -> u8 {
        match self {
            TaskType::GenerateEmbeddings => 1, // Highest - needed for search
            TaskType::ProcessObservations => 2,
            TaskType::ConsolidateMemories => 3,
            TaskType::ApplyDecay => 4,
            TaskType::CloudSync => 5,
            TaskType::RebuildIndex => 6,
            TaskType::PruneEntries => 7, // Lowest - can wait
        }
    }

    /// Human-readable name
    pub fn name(&self) -> &'static str {
        match self {
            TaskType::GenerateEmbeddings => "Generate Embeddings",
            TaskType::ProcessObservations => "Process Observations",
            TaskType::ConsolidateMemories => "Consolidate Memories",
            TaskType::ApplyDecay => "Apply Memory Decay",
            TaskType::PruneEntries => "Prune Entries",
            TaskType::RebuildIndex => "Rebuild Index",
            TaskType::CloudSync => "Cloud Sync",
        }
    }
}

/// A maintenance task with priority and metadata
#[derive(Debug, Clone)]
pub struct MaintenanceTask {
    /// Type of task
    pub task_type: TaskType,
    /// Priority (lower = higher priority)
    pub priority: u8,
    /// When this task was queued
    pub queued_at: Instant,
    /// Optional trigger source (e.g., "add command", "scheduled")
    pub trigger: Option<String>,
    /// Maximum items to process (if applicable)
    pub batch_size: Option<usize>,
}

impl MaintenanceTask {
    /// Create a new task with default priority
    pub fn new(task_type: TaskType) -> Self {
        Self {
            task_type,
            priority: task_type.default_priority(),
            queued_at: Instant::now(),
            trigger: None,
            batch_size: None,
        }
    }

    /// Create with custom priority
    pub fn with_priority(task_type: TaskType, priority: u8) -> Self {
        Self {
            task_type,
            priority,
            queued_at: Instant::now(),
            trigger: None,
            batch_size: None,
        }
    }

    /// Set the trigger source
    pub fn triggered_by(mut self, trigger: impl Into<String>) -> Self {
        self.trigger = Some(trigger.into());
        self
    }

    /// Set batch size limit
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = Some(size);
        self
    }

    /// How long this task has been waiting
    pub fn wait_time(&self) -> Duration {
        self.queued_at.elapsed()
    }
}

// Implement ordering for priority queue (BinaryHeap is max-heap, so invert)
impl PartialEq for MaintenanceTask {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.queued_at == other.queued_at
    }
}

impl Eq for MaintenanceTask {}

impl PartialOrd for MaintenanceTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MaintenanceTask {
    fn cmp(&self, other: &Self) -> Ordering {
        // Lower priority number = higher priority (comes first)
        // If same priority, older tasks come first (FIFO within priority)
        match other.priority.cmp(&self.priority) {
            Ordering::Equal => other.queued_at.cmp(&self.queued_at),
            ord => ord,
        }
    }
}

/// Thread-safe priority queue for maintenance tasks
pub struct TaskQueue {
    queue: Mutex<BinaryHeap<MaintenanceTask>>,
    /// Maximum queue size to prevent unbounded growth
    max_size: usize,
}

impl TaskQueue {
    /// Create a new task queue with default max size
    pub fn new() -> Self {
        Self::with_max_size(100)
    }

    /// Create with custom max size
    pub fn with_max_size(max_size: usize) -> Self {
        Self {
            queue: Mutex::new(BinaryHeap::new()),
            max_size,
        }
    }

    /// Add a task to the queue
    /// Returns false if queue is full
    pub fn push(&self, task: MaintenanceTask) -> bool {
        let mut queue = self.queue.lock().unwrap();
        if queue.len() >= self.max_size {
            return false;
        }
        queue.push(task);
        true
    }

    /// Add a task, replacing lowest priority task if full
    pub fn push_or_replace(&self, task: MaintenanceTask) {
        let mut queue = self.queue.lock().unwrap();
        if queue.len() >= self.max_size {
            // Drain into a sorted vec (descending priority), pop the lowest, rebuild
            let mut tasks = queue.drain().collect::<Vec<_>>();
            // Find the index of the lowest priority (highest priority number) task
            if let Some((worst_idx, _)) = tasks
                .iter()
                .enumerate()
                .max_by_key(|(_, t)| t.priority)
            {
                tasks.swap_remove(worst_idx);
            }
            for t in tasks {
                queue.push(t);
            }
        }
        queue.push(task);
    }

    /// Get the next task (highest priority)
    pub fn pop(&self) -> Option<MaintenanceTask> {
        let mut queue = self.queue.lock().unwrap();
        queue.pop()
    }

    /// Peek at the next task without removing it
    pub fn peek(&self) -> Option<MaintenanceTask> {
        let queue = self.queue.lock().unwrap();
        queue.peek().cloned()
    }

    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        let queue = self.queue.lock().unwrap();
        queue.is_empty()
    }

    /// Get queue length
    pub fn len(&self) -> usize {
        let queue = self.queue.lock().unwrap();
        queue.len()
    }

    /// Clear all tasks
    pub fn clear(&self) {
        let mut queue = self.queue.lock().unwrap();
        queue.clear();
    }

    /// Check if a task type is already queued
    pub fn contains(&self, task_type: TaskType) -> bool {
        let queue = self.queue.lock().unwrap();
        queue.iter().any(|t| t.task_type == task_type)
    }

    /// Remove all tasks of a specific type
    pub fn remove_type(&self, task_type: TaskType) {
        let mut queue = self.queue.lock().unwrap();
        let tasks: Vec<_> = queue.drain().filter(|t| t.task_type != task_type).collect();
        for t in tasks {
            queue.push(t);
        }
    }

    /// Get all pending tasks (for status display)
    pub fn pending_tasks(&self) -> Vec<MaintenanceTask> {
        let queue = self.queue.lock().unwrap();
        let mut tasks: Vec<_> = queue.iter().cloned().collect();
        tasks.sort_by(|a, b| a.cmp(b).reverse()); // Highest priority first
        tasks
    }
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Global task queue for daemon coordination
pub fn global_queue() -> Arc<TaskQueue> {
    use std::sync::OnceLock;
    static QUEUE: OnceLock<Arc<TaskQueue>> = OnceLock::new();
    QUEUE.get_or_init(|| Arc::new(TaskQueue::new())).clone()
}

/// Queue an embedding task (called from add command)
pub fn queue_embedding_task() {
    let queue = global_queue();
    if !queue.contains(TaskType::GenerateEmbeddings) {
        queue.push(MaintenanceTask::new(TaskType::GenerateEmbeddings).triggered_by("add command"));
    }
}

/// Queue observation processing (called from hooks)
pub fn queue_observation_task() {
    let queue = global_queue();
    if !queue.contains(TaskType::ProcessObservations) {
        queue
            .push(MaintenanceTask::new(TaskType::ProcessObservations).triggered_by("session hook"));
    }
}

/// Queue all scheduled maintenance tasks
pub fn queue_scheduled_maintenance() {
    let queue = global_queue();

    // Add all maintenance task types if not already queued
    let tasks = [
        TaskType::GenerateEmbeddings,
        TaskType::ProcessObservations,
        TaskType::ConsolidateMemories,
        TaskType::ApplyDecay,
        TaskType::PruneEntries,
    ];

    for task_type in tasks {
        if !queue.contains(task_type) {
            queue.push(MaintenanceTask::new(task_type).triggered_by("scheduled"));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::daemon::queue::*;

    #[test]
    fn test_task_type_priority() {
        assert!(
            TaskType::GenerateEmbeddings.default_priority()
                < TaskType::PruneEntries.default_priority()
        );
    }

    #[test]
    fn test_task_ordering() {
        let high = MaintenanceTask::new(TaskType::GenerateEmbeddings);
        let low = MaintenanceTask::new(TaskType::PruneEntries);

        // High priority should come first
        assert!(high > low);
    }

    #[test]
    fn test_same_priority_fifo() {
        let first = MaintenanceTask::new(TaskType::GenerateEmbeddings);
        let mut second = MaintenanceTask::new(TaskType::GenerateEmbeddings);
        second.queued_at = first.queued_at + std::time::Duration::from_secs(1);

        // First queued should come first (FIFO)
        assert!(first > second);
    }

    #[test]
    fn test_queue_push_pop() {
        let queue = TaskQueue::new();

        queue.push(MaintenanceTask::new(TaskType::PruneEntries));
        queue.push(MaintenanceTask::new(TaskType::GenerateEmbeddings));
        queue.push(MaintenanceTask::new(TaskType::ApplyDecay));

        // Should pop in priority order
        assert_eq!(queue.pop().unwrap().task_type, TaskType::GenerateEmbeddings);
        assert_eq!(queue.pop().unwrap().task_type, TaskType::ApplyDecay);
        assert_eq!(queue.pop().unwrap().task_type, TaskType::PruneEntries);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn test_queue_max_size() {
        let queue = TaskQueue::with_max_size(2);

        assert!(queue.push(MaintenanceTask::new(TaskType::GenerateEmbeddings)));
        assert!(queue.push(MaintenanceTask::new(TaskType::ApplyDecay)));
        assert!(!queue.push(MaintenanceTask::new(TaskType::PruneEntries))); // Should fail

        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn test_queue_contains() {
        let queue = TaskQueue::new();

        assert!(!queue.contains(TaskType::GenerateEmbeddings));
        queue.push(MaintenanceTask::new(TaskType::GenerateEmbeddings));
        assert!(queue.contains(TaskType::GenerateEmbeddings));
    }

    #[test]
    fn test_queue_remove_type() {
        let queue = TaskQueue::new();

        queue.push(MaintenanceTask::new(TaskType::GenerateEmbeddings));
        queue.push(MaintenanceTask::new(TaskType::ApplyDecay));
        queue.push(MaintenanceTask::new(TaskType::GenerateEmbeddings));

        queue.remove_type(TaskType::GenerateEmbeddings);

        assert_eq!(queue.len(), 1);
        assert!(!queue.contains(TaskType::GenerateEmbeddings));
        assert!(queue.contains(TaskType::ApplyDecay));
    }

    #[test]
    fn test_triggered_by() {
        let task = MaintenanceTask::new(TaskType::GenerateEmbeddings).triggered_by("test");
        assert_eq!(task.trigger, Some("test".to_string()));
    }

    #[test]
    fn test_with_batch_size() {
        let task = MaintenanceTask::new(TaskType::GenerateEmbeddings).with_batch_size(50);
        assert_eq!(task.batch_size, Some(50));
    }
}
