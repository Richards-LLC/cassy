//! Repair existing lazy queues without creating a queue or rewriting receipts.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 279,
    name: "prompt_queue_health_schema",
    subsystem: Subsystem::Agents,
    description: "Restore required legacy prompt queue columns for read-only health checks (cas-24d3)",
    // Shared canonical DDL runs on the migration-owned connection below;
    // raw ALTER statements would fail on stores whose queue is still lazy.
    up: &[],
    detect: Some(cas_store::PROMPT_QUEUE_HEALTH_SCHEMA_DETECT),
};

pub(crate) fn repair(conn: &rusqlite::Connection) -> crate::migration::Result<()> {
    cas_store::repair_legacy_prompt_queue_schema(conn)?;
    Ok(())
}
