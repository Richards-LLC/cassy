//! Migration: durable records for published artifacts (cassy#910).

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 256,
    name: "artifacts_create_table",
    subsystem: Subsystem::Tasks,
    description: "Record published artifacts and their Cloud upload state (cas-b72a)",
    up: cas_store::ARTIFACT_SCHEMA_STATEMENTS,
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'artifacts')
            AND EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'idx_artifacts_task')
            AND EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'idx_artifacts_sha256')
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_and_detects_the_artifact_ledger() {
        let conn = Connection::open_in_memory().unwrap();
        let detect = super::MIGRATION.detect.unwrap();
        assert_eq!(
            conn.query_row(detect, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );

        for statement in super::MIGRATION.up {
            conn.execute(statement, []).unwrap();
        }
        assert_eq!(
            conn.query_row(detect, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('artifacts')",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            11
        );
    }

    #[test]
    fn the_migration_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        for _ in 0..2 {
            for statement in super::MIGRATION.up {
                conn.execute(statement, []).unwrap();
            }
        }
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
