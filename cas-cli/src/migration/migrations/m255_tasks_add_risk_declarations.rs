//! Migration: persist task risk declarations and their scoped proof targets.
//!
//! Both columns remain nullable for legacy rows. New task/bug/feature creates
//! must declare a risk through the MCP layer; NULL reads as an empty legacy
//! declaration and is never silently upgraded.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 255,
    name: "tasks_add_risk_declarations",
    subsystem: Subsystem::Tasks,
    description: "Add nullable task risk and proof_targets columns",
    up: &[
        "ALTER TABLE tasks ADD COLUMN risk TEXT",
        "ALTER TABLE tasks ADD COLUMN proof_targets TEXT",
    ],
    detect: Some(
        "SELECT CASE WHEN EXISTS (SELECT 1 FROM pragma_table_info('tasks') WHERE name = 'risk') AND EXISTS (SELECT 1 FROM pragma_table_info('tasks') WHERE name = 'proof_targets') THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_adds_both_risk_columns() {
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
