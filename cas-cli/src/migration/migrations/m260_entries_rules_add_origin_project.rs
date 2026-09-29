//! Persist authoring project on entries and rules. Legacy attribution runs once
//! with this migration, in the same transaction as its ledger receipt.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 260,
    name: "entries_rules_add_origin_project",
    subsystem: Subsystem::Entries,
    description: "Add nullable origin_project to entries and rules",
    up: &[
        "ALTER TABLE entries ADD COLUMN origin_project TEXT",
        "ALTER TABLE rules ADD COLUMN origin_project TEXT",
    ],
    detect: Some(
        "SELECT (SELECT COUNT(*) FROM pragma_table_info('entries') WHERE name = 'origin_project')
              * (SELECT COUNT(*) FROM pragma_table_info('rules') WHERE name = 'origin_project')",
    ),
};
