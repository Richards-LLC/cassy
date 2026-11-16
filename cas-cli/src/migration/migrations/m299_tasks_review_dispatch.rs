//! Persist the exact dispatch a review assignment targets, without granting authority.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 299,
    name: "tasks_review_dispatch",
    subsystem: Subsystem::Tasks,
    description: "Add optional dispatch target for independent review assignments",
    up: &["ALTER TABLE tasks ADD COLUMN review_dispatch_id TEXT"],
    detect: Some("SELECT EXISTS (SELECT 1 FROM pragma_table_info('tasks') WHERE name = 'review_dispatch_id')"),
};
