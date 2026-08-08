//! Migration: add the FTS5 index over commit prose (EPIC cas-6212 / cas-7f40).
//!
//! M1 (m221) landed `history_commits` without a lexical index because M1 had no
//! query surface. M4 adds one, and it needs BM25 over `subject + body`.
//!
//! This is a **separate** migration rather than an extra statement on m221 for
//! a reason worth stating: m221 has already run on live databases, and its
//! `detect` predicate reports "applied". Appending to its statement list would
//! be invisible to exactly the installs that have history rows to index.
//!
//! The second statement backfills the index from rows m221 already wrote, so a
//! database that has been indexing commits for days becomes searchable without
//! a re-backfill. It is guarded by `WHERE NOT EXISTS (SELECT 1 FROM
//! history_commits_fts)`: running it twice would double every commit's term
//! frequencies and silently skew `bm25()` rather than fail loudly.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 223,
    name: "history_commits_fts",
    subsystem: Subsystem::Code,
    description: "Add FTS5 lexical index over git-history commit subject/body, backfilled from existing rows (cas-7f40)",
    up: cas_store::HISTORY_FTS_STATEMENTS,
    detect: Some(
        "SELECT CASE WHEN EXISTS (
            SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'history_commits_fts'
         ) THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    /// A database in the state m221 leaves behind: commits, no FTS index.
    fn m221_database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        for sql in super::super::m221_history_index_create_tables::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }
        conn
    }

    #[test]
    fn migration_creates_and_detects_the_fts_index() {
        let conn = m221_database();
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

    /// The whole reason this is its own migration: rows written before it
    /// existed must become searchable without a re-backfill.
    #[test]
    fn existing_commits_are_backfilled_into_the_index() {
        let conn = m221_database();
        conn.execute(
            "INSERT INTO history_commits (sha, short_sha, committed_at, subject, body, repository, indexed_at)
             VALUES ('abc', 'abc', '2026-08-01T00:00:00Z', 'fix the verifier gate', 'closes the hole', '/repo', 'now')",
            [],
        )
        .unwrap();

        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }

        let found: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM history_commits_fts WHERE history_commits_fts MATCH 'verifier'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(found, 1, "pre-existing commit was not backfilled");

        // Body prose is indexed too, not just the subject.
        let found: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM history_commits_fts WHERE history_commits_fts MATCH 'hole'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(found, 1);
    }

    /// Re-running must not duplicate term frequencies. A doubled index does not
    /// error — it quietly changes every ranking, which is why this is a test
    /// rather than a comment.
    #[test]
    fn rerunning_the_backfill_does_not_duplicate_rows() {
        let conn = m221_database();
        conn.execute(
            "INSERT INTO history_commits (sha, short_sha, committed_at, subject, repository, indexed_at)
             VALUES ('abc', 'abc', '2026-08-01T00:00:00Z', 'subject', '/repo', 'now')",
            [],
        )
        .unwrap();

        for _ in 0..3 {
            for sql in super::MIGRATION.up {
                conn.execute(sql, []).unwrap();
            }
        }

        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM history_commits_fts", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 1);
    }

    /// The store's own DDL and the migration must produce the same object, or a
    /// migrated database and a freshly created one diverge.
    #[test]
    fn migration_matches_the_store_baseline() {
        fn ddl(conn: &Connection) -> String {
            conn.query_row(
                "SELECT COALESCE(sql, '') FROM sqlite_master WHERE name = 'history_commits_fts'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
        }

        let baseline = Connection::open_in_memory().unwrap();
        baseline.execute_batch(cas_store::HISTORY_SCHEMA).unwrap();

        let migrated = m221_database();
        for sql in super::MIGRATION.up {
            migrated.execute(sql, []).unwrap();
        }

        assert_eq!(ddl(&baseline), ddl(&migrated), "FTS shape drift");
    }
}
