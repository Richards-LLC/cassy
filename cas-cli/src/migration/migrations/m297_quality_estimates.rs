//! Extend the append-only friction ledger with calibrated promises and outcomes.
use crate::migration::{Migration, Subsystem};
pub const MIGRATION: Migration = Migration {
    id: 297,
    name: "quality_estimates",
    subsystem: Subsystem::Agents,
    description: "Preserve ledger history while admitting estimates and their outcomes",
    up: cas_store::QUALITY_EVENT_ESTIMATE_UPGRADE_STATEMENTS,
    detect: Some(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name='quality_events' AND instr(sql, char(39)||'estimate_outcome'||char(39)) > 0)",
    ),
};
