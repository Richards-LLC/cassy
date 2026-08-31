//! Historical migration: rules_add_hook_command.
//!
//! The live Rule model intentionally no longer exposes this column. Keep this
//! migration immutable so existing migration ledgers remain stable; the
//! column is ignored by current SQLite/Markdown stores.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 51,
    name: "rules_add_hook_command",
    subsystem: Subsystem::Rules,
    description: "Add hook_command column for rule automation",
    up: &["ALTER TABLE rules ADD COLUMN hook_command TEXT"],
    detect: Some("SELECT COUNT(*) FROM pragma_table_info('rules') WHERE name = 'hook_command'"),
};
