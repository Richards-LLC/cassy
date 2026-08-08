//! Migration: create `history_docs` (EPIC cas-6212 / cas-9a38, spec §4.1 + §8).
//!
//! One row per embeddable text unit from GitHub (issues, pull requests, their
//! comments) and from `CHANGELOG.md` (one row per release section). Registered
//! under [`Subsystem::Code`] beside `m221`'s structural git tables.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 222,
    name: "history_docs_create_table",
    subsystem: Subsystem::Code,
    description:
        "Create history_docs: GitHub issue/PR/comment bodies and CHANGELOG release sections (cas-9a38)",
    up: cas_store::HISTORY_DOCS_SCHEMA_STATEMENTS,
    detect: Some(
        "SELECT CASE WHEN EXISTS (
             SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'history_docs'
         ) THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_and_detects_history_docs() {
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
    fn migration_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        for _ in 0..2 {
            for sql in super::MIGRATION.up {
                conn.execute(sql, []).unwrap();
            }
        }
    }

    /// Compares the FULL column definition (type, NOT NULL, default, pk), the
    /// index set and the normalized DDL — not just column names, which would
    /// let a dropped NOT NULL or a lost default pass as identical and leave a
    /// migrated database structurally weaker than a freshly created one.
    /// Mirrors `m221`'s test.
    #[test]
    fn migration_schema_matches_store_baseline() {
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
        baseline
            .execute_batch(cas_store::HISTORY_DOCS_SCHEMA)
            .unwrap();
        let migrated = Connection::open_in_memory().unwrap();
        for sql in super::MIGRATION.up {
            migrated.execute(sql, []).unwrap();
        }

        assert_eq!(
            shape(&baseline, "history_docs"),
            shape(&migrated, "history_docs"),
            "shape drift for history_docs"
        );
    }

    /// `m222` must be additive over an `m221` database: a store that stopped at
    /// M1 has the three git tables and no `history_docs`, and applying M6 must
    /// leave the M1 rows exactly where they were.
    #[test]
    fn m222_is_additive_over_an_m221_database() {
        let conn = Connection::open_in_memory().unwrap();
        for sql in super::super::m221_history_index_create_tables::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }
        conn.execute(
            "INSERT INTO history_commits (sha, short_sha, committed_at, subject, repository, indexed_at)
             VALUES ('abc', 'abc', 'now', 's', '/repo', 'now')",
            [],
        )
        .unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0,
            "an m221-only database must not report history_docs present"
        );

        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }
        let commits: i64 = conn
            .query_row("SELECT COUNT(*) FROM history_commits", [], |r| r.get(0))
            .unwrap();
        assert_eq!(commits, 1, "M1 rows must survive the M6 migration");
    }
}
