//! Migration: local projection for cloud-owned cross-project blockers (cas-e0c9).

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 234,
    name: "external_task_dependencies",
    subsystem: Subsystem::Tasks,
    description: "Project cloud-owned cross-project blockers without foreign task replicas (cas-e0c9)",
    up: &[
        "CREATE TABLE IF NOT EXISTS external_task_dependencies (origin_task_id TEXT NOT NULL, proposal_id TEXT NOT NULL, target_project_canonical_id TEXT NOT NULL DEFAULT '', target_task_id TEXT NOT NULL, proposal_state TEXT NOT NULL, target_task_status TEXT, resolution_state TEXT NOT NULL, resolved_at TEXT, updated_at TEXT NOT NULL, PRIMARY KEY (origin_task_id, proposal_id))",
        "CREATE INDEX IF NOT EXISTS idx_external_task_dependencies_origin_state ON external_task_dependencies(origin_task_id, resolution_state)",
        "CREATE TABLE IF NOT EXISTS external_task_dependency_sync_state (origin_project_canonical_id TEXT PRIMARY KEY, cursor TEXT, updated_at TEXT NOT NULL)",
    ],
    detect: Some(
        "SELECT CASE WHEN (SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='external_task_dependencies') = 0 THEN 0 WHEN (SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='external_task_dependency_sync_state') = 0 THEN 0 ELSE 1 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_projection_and_cursor_tables() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        for statement in super::MIGRATION.up {
            conn.execute(statement, []).unwrap();
        }
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
