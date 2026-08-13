//! Migration: durable operator suppressions for replayed external handoffs.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 235,
    name: "external_task_dependency_suppressions",
    subsystem: Subsystem::Tasks,
    description: "Preserve operator removal of replayed cloud external dependencies",
    up: &[
        "ALTER TABLE external_task_dependencies ADD COLUMN suppressed_at TEXT",
        "CREATE TABLE IF NOT EXISTS task_proposal_request_keys (request_fingerprint TEXT PRIMARY KEY, client_request_id TEXT NOT NULL, created_at TEXT NOT NULL)",
    ],
    detect: Some(
        "SELECT CASE WHEN EXISTS (SELECT 1 FROM pragma_table_info('external_task_dependencies') WHERE name = 'suppressed_at') AND EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'task_proposal_request_keys') THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_adds_durable_suppression_marker() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE external_task_dependencies (origin_task_id TEXT, proposal_id TEXT)",
        )
        .unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        conn.execute(super::MIGRATION.up[0], []).unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
