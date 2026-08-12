//! Migration: persist typed terminal task outcomes without backfilling legacy rows.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 233,
    name: "tasks_add_terminal_outcome",
    subsystem: Subsystem::Tasks,
    description: "Add nullable typed terminal outcome metadata to tasks (cas-5092)",
    up: &["ALTER TABLE tasks ADD COLUMN terminal_outcome TEXT"],
    detect: Some(
        "SELECT CASE WHEN (SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tasks') = 0 THEN 1 ELSE (SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name = 'terminal_outcome') END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_adds_nullable_column_without_backfilling_legacy_rows() {
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
                "SELECT COUNT(*) FROM tasks WHERE terminal_outcome IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0,
            "m233 must never backfill a guessed outcome"
        );
    }
}
