//! Record each running binary's build hash beside its version, so a rebuild at
//! the same version is told apart from the build already running (cas-724e).
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 301,
    name: "history_epochs_build",
    subsystem: Subsystem::Code,
    description: "Add the running binary's build hash to history_epochs",
    up: &["ALTER TABLE history_epochs ADD COLUMN build TEXT"],
    detect: Some("SELECT EXISTS (SELECT 1 FROM pragma_table_info('history_epochs') WHERE name = 'build')"),
};
