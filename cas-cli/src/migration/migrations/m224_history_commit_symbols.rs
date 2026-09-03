//! Migration: symbol mapping for the git-history index (EPIC cas-6212 / cas-0562).
//!
//! Adds `history_commit_symbols` — which symbols a commit's changed line ranges
//! actually intersect (spec §4.1) — and the `history_commits.symbol_mapping`
//! column that records *why* a commit has no symbol rows.
//!
//! # Why a second migration rather than editing m221
//!
//! m221 already ran on every store that has a history index, and its `detect`
//! predicate now returns 1 there, so the runner will never revisit it. Editing
//! its statements only affects databases that have never seen m221 — which is
//! exactly the population that does *not* need the upgrade. The ALTER has to
//! live in its own numbered step or existing installs silently keep the old
//! shape and fail at query time on a missing column.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 224,
    name: "history_commit_symbols",
    subsystem: Subsystem::Code,
    description:
        "Add history_commit_symbols and history_commits.symbol_mapping for commit↔symbol overlap (cas-0562)",
    up: &[
        "ALTER TABLE history_commits ADD COLUMN symbol_mapping TEXT NOT NULL DEFAULT 'pending'",
        "CREATE TABLE IF NOT EXISTS history_commit_symbols (
            sha TEXT NOT NULL REFERENCES history_commits(sha) ON DELETE CASCADE,
            symbol_id TEXT NOT NULL,
            qualified_name TEXT NOT NULL,
            file_path TEXT NOT NULL,
            PRIMARY KEY (sha, symbol_id)
        )",
        "CREATE INDEX IF NOT EXISTS idx_history_commit_symbols_qualified_name
            ON history_commit_symbols(qualified_name)",
    ],
    // Both halves must be present. Checking only the table would let a store
    // that somehow gained the table without the column report "done" and then
    // fail on every stamp; checking only the column has the mirror problem.
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (
                SELECT 1 FROM pragma_table_info('history_commits')
                WHERE name = 'symbol_mapping'
            )
            AND EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'history_commit_symbols'
            )
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    /// The exact shape m221 produced *before* M3 — the state every already
    /// migrated store is sitting in. Kept as a literal on purpose: deriving it
    /// from today's `HISTORY_SCHEMA` would make the upgrade-path test tautological.
    const PRE_M3_HISTORY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS history_commits (
    sha TEXT PRIMARY KEY,
    short_sha TEXT NOT NULL,
    parent_shas TEXT NOT NULL DEFAULT '[]',
    is_merge INTEGER NOT NULL DEFAULT 0,
    author_name TEXT,
    author_email TEXT,
    authored_at TEXT,
    committed_at TEXT NOT NULL,
    subject TEXT NOT NULL,
    body TEXT,
    branch_hint TEXT,
    repository TEXT NOT NULL,
    pending_embedding INTEGER NOT NULL DEFAULT 1,
    indexed_at TEXT NOT NULL,
    scope TEXT NOT NULL DEFAULT 'project'
);
CREATE INDEX IF NOT EXISTS idx_history_commits_committed_at
    ON history_commits(committed_at DESC);
CREATE INDEX IF NOT EXISTS idx_history_commits_short_sha
    ON history_commits(short_sha);
CREATE INDEX IF NOT EXISTS idx_history_commits_pending_embedding
    ON history_commits(committed_at) WHERE pending_embedding = 1;
CREATE TABLE IF NOT EXISTS history_commit_files (
    sha TEXT NOT NULL REFERENCES history_commits(sha) ON DELETE CASCADE,
    file_path TEXT NOT NULL,
    change_type TEXT NOT NULL,
    old_path TEXT,
    insertions INTEGER,
    deletions INTEGER,
    PRIMARY KEY (sha, file_path)
);
CREATE INDEX IF NOT EXISTS idx_history_commit_files_path
    ON history_commit_files(file_path);
CREATE TABLE IF NOT EXISTS history_index_state (
    repository TEXT NOT NULL,
    source TEXT NOT NULL,
    last_indexed_sha TEXT,
    last_indexed_at TEXT,
    last_attempt_at TEXT,
    last_error TEXT,
    backfill_complete INTEGER NOT NULL DEFAULT 0,
    items_indexed INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (repository, source)
);
"#;

    fn pre_m3(conn: &Connection) {
        conn.execute_batch(PRE_M3_HISTORY_SCHEMA).unwrap();
    }

    fn apply(conn: &Connection) {
        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }
    }

    fn detect(conn: &Connection) -> i64 {
        conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn migration_upgrades_a_pre_m3_store_and_detects() {
        let conn = Connection::open_in_memory().unwrap();
        pre_m3(&conn);
        assert_eq!(detect(&conn), 0, "pre-M3 store must not read as migrated");
        apply(&conn);
        assert_eq!(detect(&conn), 1);
    }

    /// A store already carrying M1 rows must keep them, and every existing
    /// commit must land on `pending` rather than on NULL or on a value that
    /// would read as "we looked and found nothing".
    #[test]
    fn existing_commits_default_to_pending_not_a_verdict() {
        let conn = Connection::open_in_memory().unwrap();
        pre_m3(&conn);
        conn.execute(
            "INSERT INTO history_commits (sha, short_sha, committed_at, subject, repository, indexed_at)
             VALUES ('abc', 'abc', 'now', 's', '/repo', 'now')",
            [],
        )
        .unwrap();

        apply(&conn);

        let mapping: String = conn
            .query_row(
                "SELECT symbol_mapping FROM history_commits WHERE sha = 'abc'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            mapping, "pending",
            "a commit indexed before M3 must say 'never attempted', not a verdict"
        );
    }

    /// The upgraded shape must be indistinguishable from a store created fresh
    /// from `HISTORY_SCHEMA`, or migrated installs quietly diverge from new ones.
    #[test]
    fn upgraded_shape_matches_a_fresh_store() {
        fn shape(conn: &Connection, table: &str) -> (Vec<String>, Vec<String>) {
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
            (columns, indexes)
        }

        let upgraded = Connection::open_in_memory().unwrap();
        pre_m3(&upgraded);
        apply(&upgraded);
        // Every later migration that touches these tables belongs in the
        // upgrade path too, or this guard compares a partially-migrated store
        // against a fresh one and fails for the wrong reason.
        for statement in super::super::m253_history_embedding_error::MIGRATION.up {
            if statement.contains("history_commits") {
                upgraded.execute_batch(statement).unwrap();
            }
        }

        let fresh = Connection::open_in_memory().unwrap();
        fresh.execute_batch(cas_store::HISTORY_SCHEMA).unwrap();

        for table in ["history_commits", "history_commit_symbols"] {
            assert_eq!(
                shape(&upgraded, table),
                shape(&fresh, table),
                "shape drift for {table} between the upgrade path and a fresh store"
            );
        }
    }

    /// Deleting a commit must not strand its symbol rows — the same cascade
    /// contract m221 established for `history_commit_files`.
    #[test]
    fn commit_symbols_cascade_on_commit_delete() {
        let conn = Connection::open_in_memory().unwrap();
        pre_m3(&conn);
        apply(&conn);
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute(
            "INSERT INTO history_commits (sha, short_sha, committed_at, subject, repository, indexed_at)
             VALUES ('abc', 'abc', 'now', 's', '/repo', 'now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO history_commit_symbols (sha, symbol_id, qualified_name, file_path)
             VALUES ('abc', 'sym1', 'foo::bar', 'a.rs')",
            [],
        )
        .unwrap();

        conn.execute("DELETE FROM history_commits WHERE sha = 'abc'", [])
            .unwrap();
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM history_commit_symbols", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(left, 0, "commit delete must cascade to its symbol rows");
    }

    /// A fresh store created from `HISTORY_SCHEMA` already has both halves, so
    /// the runner must skip this migration rather than attempting a duplicate
    /// `ADD COLUMN`, which SQLite rejects outright.
    #[test]
    fn fresh_store_reads_as_already_migrated() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(cas_store::HISTORY_SCHEMA).unwrap();
        assert_eq!(detect(&conn), 1);
    }
}
