//! Migration: create the structural git-history index (EPIC cas-6212 / cas-7a21).
//!
//! Adds `history_commits` (one row per commit), `history_commit_files` (the
//! `(commit, file)` structural diff mapping) and `history_index_state` (the
//! walker watermark plus its honesty ledger). Registered under
//! [`Subsystem::Code`] alongside the existing code-index tables.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 221,
    name: "history_index_create_tables",
    subsystem: Subsystem::Code,
    description: "Create structural git-history index: commits, commit files, walker watermark (cas-7a21)",
    up: cas_store::HISTORY_SCHEMA_STATEMENTS,
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'history_commits'
            )
            AND EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'history_commit_files'
            )
            AND EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'history_index_state'
            )
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    const TABLES: [&str; 3] = [
        "history_commits",
        "history_commit_files",
        "history_index_state",
    ];

    #[test]
    fn migration_creates_and_detects_history_schema() {
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

    /// Compares the FULL column definition (type, NOT NULL, default, pk), the
    /// index set and the normalized DDL text — not just column names. A
    /// name-only comparison would let a dropped NOT NULL, a lost default or a
    /// missing FK pass as identical, leaving migrated databases structurally
    /// weaker than freshly created ones. Mirrors m219's test.
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
        baseline.execute_batch(cas_store::HISTORY_SCHEMA).unwrap();
        let migrated = Connection::open_in_memory().unwrap();
        for sql in super::MIGRATION.up {
            migrated.execute(sql, []).unwrap();
        }

        for table in TABLES {
            assert_eq!(
                shape(&baseline, table),
                shape(&migrated, table),
                "shape drift for {table}"
            );
        }
    }

    /// The cascade is what keeps a re-backfill from stranding orphan file rows.
    #[test]
    fn commit_files_cascade_on_commit_delete() {
        let conn = Connection::open_in_memory().unwrap();
        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute(
            "INSERT INTO history_commits (sha, short_sha, committed_at, subject, repository, indexed_at)
             VALUES ('abc', 'abc', 'now', 's', '/repo', 'now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO history_commit_files (sha, file_path, change_type)
             VALUES ('abc', 'a.rs', 'M')",
            [],
        )
        .unwrap();

        conn.execute("DELETE FROM history_commits WHERE sha = 'abc'", [])
            .unwrap();
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM history_commit_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "commit delete must cascade to its file rows");
    }
}
