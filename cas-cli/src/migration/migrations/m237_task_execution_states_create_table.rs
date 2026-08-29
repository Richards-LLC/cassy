//! Migration: create the compact structured task resume-state table.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 237,
    name: "task_execution_states_create_table",
    subsystem: Subsystem::Tasks,
    description: "Add sparse structured execution state for task resume (cas-4adb)",
    up: &["CREATE TABLE IF NOT EXISTS task_execution_states (
        task_id TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
        state TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )"],
    detect: Some(
        "SELECT CASE WHEN (SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'task_execution_states') = 1 THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_task_execution_states() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE tasks (id TEXT PRIMARY KEY)", [])
            .unwrap();
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
