//! Migration: record the lifecycle operation for every rule and skill version.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 242,
    name: "rule_skill_versions_operations",
    subsystem: Subsystem::Rules,
    description: "Record create, update, delete, and restore operations in knowledge history (cas-ef20)",
    up: &[
        "ALTER TABLE rule_versions ADD COLUMN operation TEXT NOT NULL DEFAULT 'update'",
        "ALTER TABLE skill_versions ADD COLUMN operation TEXT NOT NULL DEFAULT 'update'",
    ],
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'rule_versions')
            AND EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'skill_versions')
            AND EXISTS (SELECT 1 FROM pragma_table_info('rule_versions') WHERE name = 'operation')
            AND EXISTS (SELECT 1 FROM pragma_table_info('skill_versions') WHERE name = 'operation')
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_adds_operation_to_existing_history_tables() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE rule_versions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                rule_id TEXT NOT NULL,
                version INTEGER NOT NULL,
                snapshot_json TEXT NOT NULL,
                content TEXT NOT NULL,
                status TEXT NOT NULL,
                changed_by TEXT,
                changed_at TEXT NOT NULL,
                change_note TEXT NOT NULL,
                UNIQUE(rule_id, version)
             );
             CREATE TABLE skill_versions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                skill_id TEXT NOT NULL,
                version INTEGER NOT NULL,
                snapshot_json TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT NOT NULL,
                status TEXT NOT NULL,
                changed_by TEXT,
                changed_at TEXT NOT NULL,
                change_note TEXT NOT NULL,
                UNIQUE(skill_id, version)
             );",
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
                "SELECT dflt_value FROM pragma_table_info('rule_versions') WHERE name = 'operation'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "'update'"
        );
        assert_eq!(
            conn.query_row(
                "SELECT dflt_value FROM pragma_table_info('skill_versions') WHERE name = 'operation'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "'update'"
        );
    }
}
