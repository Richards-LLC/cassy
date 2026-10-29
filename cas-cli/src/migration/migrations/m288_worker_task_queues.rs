//! Ordered worker backlogs and atomic spawn payloads.
use crate::migration::{Migration, Subsystem};
pub const MIGRATION: Migration = Migration {
    id: 288,
    name: "worker_task_queues",
    subsystem: Subsystem::Agents,
    description: "Persist ordered worker task queues independently of registrations",
    up: &[cas_store::worker_task_queue_store::WORKER_TASK_QUEUE_SCHEMA],
    detect: Some(
        "SELECT COUNT(*)=2 FROM sqlite_master WHERE type='table' AND name IN ('worker_task_queues','spawn_task_queues')",
    ),
};
