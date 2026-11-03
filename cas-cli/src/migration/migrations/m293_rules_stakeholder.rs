//! Preserve verbatim stakeholder provenance; old rules remain unattributed.
use crate::migration::{Migration, Subsystem};
pub const MIGRATION: Migration = Migration {
    id: 293,
    name: "rules_stakeholder",
    subsystem: Subsystem::Rules,
    description: "Add nullable stakeholder attribution to rules without backfilling old rows",
    up: &["ALTER TABLE rules ADD COLUMN stakeholder TEXT"],
    detect: Some(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('rules') WHERE name = 'stakeholder')",
    ),
};
