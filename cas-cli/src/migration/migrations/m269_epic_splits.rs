//! Migration: epic splits between peer supervisors (cas-4736).

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 269,
    name: "epic_splits",
    subsystem: Subsystem::Agents,
    description: "Record epic splits agreed between peer supervisors of one repository (cas-4736)",
    up: cas_store::EPIC_SPLIT_SCHEMA_STATEMENTS,
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'epic_splits')
            AND EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'idx_epic_splits_state')
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_and_detects_epic_splits() {
        let conn = Connection::open_in_memory().unwrap();
        let detect = super::MIGRATION.detect.unwrap();
        let detected =
            |conn: &Connection| conn.query_row(detect, [], |row| row.get::<_, i64>(0)).unwrap();
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

    #[test]
    fn a_store_migrated_from_scratch_has_the_table() {
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
        let detected: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(detected, 1);
    }
}
