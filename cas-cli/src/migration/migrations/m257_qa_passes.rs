//! Migration: independent QA pass rounds (cas-619f).

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 257,
    name: "qa_passes",
    subsystem: Subsystem::Verification,
    description: "Record independent QA and polish pass rounds bound to a delivered branch tip (cas-619f)",
    up: cas_store::QA_PASS_SCHEMA_STATEMENTS,
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'qa_passes')
            AND EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'idx_qa_passes_active_task')
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_and_detects_qa_passes() {
        let conn = Connection::open_in_memory().unwrap();
        let detect = super::MIGRATION.detect.unwrap();
        let detected = |conn: &Connection| conn.query_row(detect, [], |row| row.get::<_, i64>(0)).unwrap();
        assert_eq!(detected(&conn), 0);
        for statement in super::MIGRATION.up {
            conn.execute(statement, []).unwrap();
        }
        assert_eq!(detected(&conn), 1);
        // Idempotent: the store-open repair runs the same DDL again.
        for statement in super::MIGRATION.up {
            conn.execute(statement, []).unwrap();
        }
    }
}
