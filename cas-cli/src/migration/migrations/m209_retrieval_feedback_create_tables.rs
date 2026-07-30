//! Migration: add versioned retrieval identity and explicit outcome feedback.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 209,
    name: "retrieval_feedback_create_tables",
    subsystem: Subsystem::Events,
    description: "Create privacy-safe retrieval query, result identity, and explicit outcome tables (cas-aeac)",
    up: cas_store::RETRIEVAL_SCHEMA_STATEMENTS,
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'retrieval_queries'
            )
            AND EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'retrieval_query_results'
            )
            AND EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'retrieval_outcomes'
            )
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_and_detects_retrieval_schema() {
        let conn = Connection::open_in_memory().unwrap();
        let detect = super::MIGRATION.detect.unwrap();
        assert_eq!(
            conn.query_row(detect, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );

        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }

        assert_eq!(
            conn.query_row(detect, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn migration_schema_matches_store_baseline() {
        fn shape(conn: &Connection, table: &str) -> (Vec<String>, Vec<String>) {
            let columns = {
                let mut stmt = conn
                    .prepare(&format!(
                        "SELECT name FROM pragma_table_info('{table}') ORDER BY cid"
                    ))
                    .unwrap();
                stmt.query_map([], |row| row.get::<_, String>(0))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
            };
            let indexes = {
                let mut stmt = conn
                    .prepare(&format!(
                        "SELECT name FROM sqlite_master
                     WHERE type = 'index' AND tbl_name = '{table}' AND sql IS NOT NULL
                     ORDER BY name"
                    ))
                    .unwrap();
                stmt.query_map([], |row| row.get::<_, String>(0))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
            };
            (columns, indexes)
        }

        let baseline = Connection::open_in_memory().unwrap();
        baseline.execute_batch(cas_store::RETRIEVAL_SCHEMA).unwrap();
        let migrated = Connection::open_in_memory().unwrap();
        for sql in super::MIGRATION.up {
            migrated.execute(sql, []).unwrap();
        }

        for table in [
            "retrieval_queries",
            "retrieval_query_results",
            "retrieval_outcomes",
        ] {
            assert_eq!(shape(&baseline, table), shape(&migrated, table));
        }
    }
}
