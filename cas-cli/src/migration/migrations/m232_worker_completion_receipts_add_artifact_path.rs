//! Migration: retain structured durable completion artifacts (cas-96f9).

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 232,
    name: "worker_completion_receipts_add_artifact_path",
    subsystem: Subsystem::Tasks,
    description: "Add immutable durable artifact paths to worker completion receipts (cas-96f9)",
    up: &["ALTER TABLE worker_completion_receipts ADD COLUMN artifact_path TEXT"],
    detect: Some(
        "SELECT CASE WHEN (SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='worker_completion_receipts') = 0 THEN 1 ELSE (SELECT COUNT(*) FROM pragma_table_info('worker_completion_receipts') WHERE name = 'artifact_path') END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_adds_nullable_artifact_path_to_existing_receipts() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE worker_completion_receipts (
                id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL
            )",
        )
        .unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        for statement in super::MIGRATION.up {
            conn.execute(statement, []).unwrap();
        }
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        conn.execute(
            "INSERT INTO worker_completion_receipts (id, task_id, artifact_path) VALUES ('legacy', 'cas-legacy', NULL)",
            [],
        )
        .unwrap();
    }
}
