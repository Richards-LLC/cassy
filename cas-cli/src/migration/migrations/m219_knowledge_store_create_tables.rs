//! Migration: create the project-knowledge store (EPIC cas-7d31 / cas-cbf1).
//!
//! Adds `knowledge_pages` (index rows; bodies stay on disk under
//! `.cas/knowledge/`), `knowledge_sources` (content-hash ingest ledger) and the
//! contentless `knowledge_pages_fts` FTS5 index over title + snippet + body.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 219,
    name: "knowledge_store_create_tables",
    subsystem: Subsystem::Knowledge,
    description: "Create knowledge page index, content-hash source ledger, and FTS5 index (cas-cbf1)",
    up: cas_store::KNOWLEDGE_SCHEMA_STATEMENTS,
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'knowledge_pages'
            )
            AND EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'knowledge_sources'
            )
            AND EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'knowledge_pages_fts'
            )
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_and_detects_knowledge_schema() {
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

    /// Applying the migration twice must be a no-op (it runs on existing DBs).
    #[test]
    fn migration_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        for _ in 0..2 {
            for sql in super::MIGRATION.up {
                conn.execute(sql, []).unwrap();
            }
        }
    }

    #[test]
    fn migration_schema_matches_store_baseline() {
        // Compares the FULL column definition (type, NOT NULL, default, pk) and
        // the normalized DDL text — not just column names. Name-only comparison
        // would let a lost CHECK constraint, a dropped NOT NULL, or (worst) a
        // missing `content=''` on the FTS table pass as identical, leaving
        // migrated databases structurally weaker than freshly created ones.
        fn shape(conn: &Connection, table: &str) -> (Vec<String>, Vec<String>, String) {
            let columns = {
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
            let ddl: String = conn
                .query_row(
                    "SELECT COALESCE(sql, '') FROM sqlite_master WHERE name = ?1",
                    [table],
                    |row| row.get::<_, String>(0),
                )
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            (columns, indexes, ddl)
        }

        let baseline = Connection::open_in_memory().unwrap();
        baseline.execute_batch(cas_store::KNOWLEDGE_SCHEMA).unwrap();
        let migrated = Connection::open_in_memory().unwrap();
        for sql in super::MIGRATION.up {
            migrated.execute(sql, []).unwrap();
        }

        for table in [
            "knowledge_pages",
            "knowledge_sources",
            "knowledge_pages_fts",
        ] {
            assert_eq!(
                shape(&baseline, table),
                shape(&migrated, table),
                "shape drift for {table}"
            );
        }

        // The invariant the whole design rests on: the FTS table must stay
        // contentless, or every distilled body starts living in cas.db.
        let fts_ddl = shape(&migrated, "knowledge_pages_fts").2;
        assert!(
            fts_ddl.contains("content=''"),
            "knowledge_pages_fts must remain contentless: {fts_ddl}"
        );
        assert!(
            fts_ddl.contains("contentless_delete=1"),
            "knowledge_pages_fts needs contentless_delete for reindexing: {fts_ddl}"
        );
    }
}
