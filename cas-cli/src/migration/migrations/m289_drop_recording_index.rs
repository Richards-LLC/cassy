//! Drop the recording index tables. No production path ever inserted a
//! `recordings` row: factory recordings are files written by the panes, and
//! the store that indexed them was removed. Children and FTS tables go first;
//! each table's indexes and triggers drop with it.
use crate::migration::{Migration, Subsystem};

/// Detection for m167 and m171-m180, which created these tables. Their
/// table/index predicates turn false once this migration runs, and the runner
/// would then replay their recorded DDL on every start and recreate the tables.
pub(super) const RETIRED_DETECT: &str = "SELECT 1";

pub const MIGRATION: Migration = Migration {
    id: 289,
    name: "drop_recording_index",
    subsystem: Subsystem::Recordings,
    description: "Drop the never-written recording index tables",
    up: &[
        "DROP TABLE IF EXISTS recording_events",
        "DROP TABLE IF EXISTS recording_agents",
        "DROP TABLE IF EXISTS recordings_fts",
        "DROP TABLE IF EXISTS recordings",
        "DROP TABLE IF EXISTS recording_text_fts",
        "DROP TABLE IF EXISTS recording_text",
    ],
    detect: Some(
        "SELECT COUNT(*)=0 FROM sqlite_master WHERE type='table' AND name IN ('recording_events','recording_agents','recordings_fts','recordings','recording_text_fts','recording_text')",
    ),
};
