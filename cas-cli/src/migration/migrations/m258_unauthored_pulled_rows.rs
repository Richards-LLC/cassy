//! Migration: local ledger of pulled rows this project did not author (cas-3a90).
//!
//! Entries, rules and skills pulled without an `origin_project` cannot be
//! attributed. The ledger records the ones a pull created, so every push drops
//! queued writes for them instead of re-publishing them under this project
//! (GH #909).

use crate::cloud::UNAUTHORED_PULL_STATEMENTS;
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 258,
    name: "unauthored_pulled_rows",
    subsystem: Subsystem::Entries,
    description: "Record pulled entries, rules and skills this project did not author so they are never re-pushed (cas-3a90)",
    up: UNAUTHORED_PULL_STATEMENTS,
    detect: Some(
        "SELECT EXISTS (
             SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'unauthored_pulled_rows'
         )",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_and_detects_the_unauthored_pull_ledger() {
        let conn = Connection::open_in_memory().unwrap();
        let detected = |conn: &Connection| {
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
        };
        assert_eq!(detected(&conn), 0);
        for statement in super::MIGRATION.up {
            conn.execute_batch(statement).unwrap();
        }
        assert_eq!(detected(&conn), 1);
        // Idempotent: re-running the DDL on an existing ledger is harmless.
        for statement in super::MIGRATION.up {
            conn.execute_batch(statement).unwrap();
        }
    }
}
