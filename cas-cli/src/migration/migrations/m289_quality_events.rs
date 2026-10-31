//! Friction ledger: the append-only quality_events table.
use crate::migration::{Migration, Subsystem};
pub const MIGRATION: Migration = Migration {
    id: 289,
    name: "quality_events",
    subsystem: Subsystem::Agents,
    description: "Persist the append-only friction ledger of checks, interventions, escapes, releases and feature verdicts",
    up: cas_store::QUALITY_EVENT_SCHEMA_STATEMENTS,
    detect: Some(
        "SELECT COUNT(*)=3 FROM sqlite_master WHERE name IN ('quality_events','quality_events_no_update','quality_events_no_delete')",
    ),
};
