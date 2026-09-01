//! Migration: persist the factory branch delivery route on tasks.
//!
//! Fresh databases include this nullable column in `TASK_SCHEMA`. Existing
//! databases receive it through this migration; NULL reads as `push_branch`
//! so legacy tasks retain the normal delivery behavior.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 247,
    name: "tasks_add_delivery_mode",
    subsystem: Subsystem::Tasks,
    description: "Add nullable factory delivery_mode TEXT column to tasks",
    up: &["ALTER TABLE tasks ADD COLUMN delivery_mode TEXT"],
    detect: Some(
        "SELECT CASE WHEN EXISTS (SELECT 1 FROM pragma_table_info('tasks') WHERE name = 'delivery_mode') THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_adds_delivery_mode_column() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE tasks (id TEXT PRIMARY KEY, status TEXT NOT NULL);")
            .unwrap();

        let before: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(before, 0);

        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }

        let after: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(after, 1);
    }
}
