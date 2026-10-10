//! Migration: index code_symbols(parent_id) and code_relationships(target_id)
//! (cas-ba66).
//!
//! `code_symbols.parent_id` references `code_symbols(id)` with ON DELETE SET
//! NULL. Without an index on the child column, every deleted symbol makes
//! SQLite scan the whole table for children to null out, so the canonical
//! indexer's `delete_symbols_in_file` cost grew with the table, not the file.
//! `code_relationships.target_id` drives caller lookups the same way.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 266,
    name: "code_parent_target_indexes",
    subsystem: Subsystem::Code,
    description: "Index code_symbols(parent_id) and code_relationships(target_id) so symbol deletes and caller lookups do not scan (cas-ba66)",
    up: &[
        "CREATE INDEX IF NOT EXISTS idx_code_symbols_parent ON code_symbols(parent_id)",
        "CREATE INDEX IF NOT EXISTS idx_code_relationships_target ON code_relationships(target_id)",
    ],
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_code_symbols_parent')
            AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_code_relationships_target')
            THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn code_tables(conn: &Connection) {
        for migration in [
            super::super::m131_code_files_create_table::MIGRATION,
            super::super::m132_code_symbols_create_table::MIGRATION,
            super::super::m133_code_relationships_create_table::MIGRATION,
        ] {
            for statement in migration.up {
                conn.execute_batch(statement).unwrap();
            }
        }
    }

    fn apply(conn: &Connection) {
        for statement in MIGRATION.up {
            conn.execute_batch(statement).unwrap();
        }
    }

    fn detected(conn: &Connection) -> i64 {
        conn.query_row(MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap()
    }

    fn plan(conn: &Connection, sql: &str) -> String {
        let mut statement = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>()
            .join("; ")
    }

    #[test]
    fn creates_both_indexes_idempotently_and_detects_them() {
        let conn = Connection::open_in_memory().unwrap();
        code_tables(&conn);
        assert_eq!(detected(&conn), 0);
        apply(&conn);
        assert_eq!(detected(&conn), 1);
        apply(&conn);
        assert_eq!(detected(&conn), 1, "re-running the migration is a no-op");
    }

    #[test]
    fn parent_and_target_lookups_use_the_indexes() {
        let conn = Connection::open_in_memory().unwrap();
        code_tables(&conn);
        let child_scan = "SELECT id FROM code_symbols WHERE parent_id = 'sym-1'";
        let callers = "SELECT source_id FROM code_relationships WHERE target_id = 'sym-1'";
        assert!(!plan(&conn, child_scan).contains("idx_code_symbols_parent"));
        apply(&conn);
        assert!(
            plan(&conn, child_scan).contains("idx_code_symbols_parent"),
            "{}",
            plan(&conn, child_scan)
        );
        assert!(
            plan(&conn, callers).contains("idx_code_relationships_target"),
            "{}",
            plan(&conn, callers)
        );
    }
}
