//! Migration: persist the canonical project that owns each task.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 241,
    name: "tasks_add_origin_project",
    subsystem: Subsystem::Tasks,
    description: "Add nullable canonical origin-project identity to tasks (cas-e0c5)",
    up: &[
        "ALTER TABLE tasks ADD COLUMN origin_project TEXT",
        "CREATE INDEX IF NOT EXISTS idx_tasks_origin_project ON tasks(origin_project)",
    ],
    detect: Some(
        "SELECT CASE WHEN (SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tasks') = 0 THEN 1 ELSE (SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name = 'origin_project') * (SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_tasks_origin_project') END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_adds_nullable_column_without_guessing_legacy_identity() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (id TEXT PRIMARY KEY, status TEXT NOT NULL);
             INSERT INTO tasks (id, status) VALUES
               ('legacy-closed', 'closed'),
               ('legacy-open', 'open');",
        )
        .unwrap();

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
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM tasks WHERE origin_project IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0,
        );
    }
}
