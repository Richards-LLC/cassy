//! Admit identified Cassy problems (cas-87a2) while preserving the append-only ledger.
use crate::migration::{Migration, Subsystem};
pub const MIGRATION: Migration = Migration {
    id: 295,
    name: "quality_friction",
    subsystem: Subsystem::Agents,
    description: "Extend the friction ledger with filed Cassy problems",
    up: cas_store::QUALITY_EVENT_FRICTION_UPGRADE_STATEMENTS,
    detect: Some(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name='quality_events' AND instr(sql, char(39)||'friction'||char(39)) > 0)",
    ),
};
