//! Migration: durable semantic-code vector queue and coverage ledger (cas-733e).

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 229,
    name: "code_vector_state",
    subsystem: Subsystem::Code,
    description: "Create isolated source-code vector queue and full-scan coverage ledger (cas-733e)",
    up: cas_store::CODE_VECTOR_SCHEMA_STATEMENTS,
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (SELECT 1 FROM sqlite_master
                    WHERE type = 'table' AND name = 'code_vector_queue')
            AND EXISTS (SELECT 1 FROM sqlite_master
                        WHERE type = 'table' AND name = 'code_index_state')
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_detects_and_matches_store_schema() {
        fn shape(conn: &Connection, table: &str) -> Vec<String> {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT name, type, \"notnull\", COALESCE(dflt_value, ''), pk
                     FROM pragma_table_info('{table}') ORDER BY cid"
                ))
                .unwrap();
            stmt.query_map([], |row| {
                Ok(format!(
                    "{}|{}|{}|{}|{}",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
        }

        let migrated = Connection::open_in_memory().unwrap();
        assert_eq!(
            migrated
                .query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        for sql in super::MIGRATION.up {
            migrated.execute(sql, []).unwrap();
        }
        assert_eq!(
            migrated
                .query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );

        let baseline = Connection::open_in_memory().unwrap();
        baseline
            .execute_batch(cas_store::CODE_VECTOR_SCHEMA)
            .unwrap();
        for table in ["code_vector_queue", "code_index_state"] {
            assert_eq!(shape(&baseline, table), shape(&migrated, table));
        }
    }

    #[test]
    fn migration_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        for _ in 0..2 {
            for sql in super::MIGRATION.up {
                conn.execute(sql, []).unwrap();
            }
        }
    }
}
