//! Admit completed model turns while preserving the append-only ledger.
use crate::migration::{Migration, Subsystem};
pub const MIGRATION: Migration = Migration {
    id: 292,
    name: "quality_turns",
    subsystem: Subsystem::Agents,
    description: "Extend the friction ledger with typed native model turns",
    up: cas_store::QUALITY_EVENT_TURN_UPGRADE_STATEMENTS,
    detect: Some(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name='quality_events' AND instr(sql, char(39)||'turn'||char(39)) > 0)",
    ),
};
