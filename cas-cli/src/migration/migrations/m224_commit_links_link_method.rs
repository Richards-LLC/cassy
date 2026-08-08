//! Migration: add `commit_links.link_method` (EPIC cas-6212 / cas-519f, spec §5.3).
//!
//! Until M5 the PostToolUse Bash hook was the only writer of `commit_links`, so
//! every row was by construction a direct observation and the table had no need
//! to say so. M5 makes the history indexer a second writer, one that
//! *reconstructs* links from the `worker_git_commit` edge — and spec §5.3's
//! requirement is explicit: "writes a `commit_links` row with an explicit
//! `link_method` so a reconstructed link is never confused with an observed
//! one".
//!
//! A `NULL` therefore has a precise meaning rather than being missing data: it
//! is a row that predates the indexer, and is read as `hook_observed`.
//!
//! Registered under [`Subsystem::Code`] beside the rest of the history index.
//! Separate from `m143` (which created the table) because m143 already reports
//! "applied" on every live database — an appended statement would reach only
//! fresh installs, i.e. exactly the ones with no commit links to describe.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 224,
    name: "commit_links_link_method",
    subsystem: Subsystem::Code,
    description:
        "Add commit_links.link_method so a reconstructed provenance link is never mistaken for an observed one (cas-519f)",
    up: cas_store::COMMIT_LINK_LINK_METHOD_STATEMENTS,
    // Also satisfied when `commit_links` does not exist at all: m143 creates it
    // earlier in the same run, and a database that somehow lacks it must not be
    // handed an `ALTER TABLE` against a missing table.
    detect: Some(
        "SELECT CASE WHEN
             NOT EXISTS (
                 SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'commit_links'
             )
             OR EXISTS (
                 SELECT 1 FROM pragma_table_info('commit_links') WHERE name = 'link_method'
             )
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    /// The pre-M5 shape of the table, as `m143` created it.
    fn legacy_table(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE commit_links (
                 commit_hash TEXT PRIMARY KEY,
                 session_id TEXT NOT NULL,
                 agent_id TEXT NOT NULL,
                 branch TEXT NOT NULL,
                 message TEXT NOT NULL,
                 files_changed TEXT NOT NULL,
                 prompt_ids TEXT NOT NULL,
                 committed_at TEXT NOT NULL,
                 author TEXT NOT NULL,
                 scope TEXT NOT NULL DEFAULT 'project'
             )",
        )
        .unwrap();
    }

    fn has_link_method(conn: &Connection) -> bool {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('commit_links') WHERE name = 'link_method')",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    }

    #[test]
    fn migration_adds_the_column_to_a_legacy_table() {
        let conn = Connection::open_in_memory().unwrap();
        legacy_table(&conn);
        let detect = super::MIGRATION.detect.unwrap();
        assert_eq!(
            conn.query_row(detect, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0,
            "a legacy table must be detected as needing the migration"
        );

        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }

        assert!(has_link_method(&conn));
        assert_eq!(
            conn.query_row(detect, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    /// `ALTER TABLE ADD COLUMN` is not idempotent on its own — it errors on the
    /// second run. The `detect` predicate is what makes the migration safe to
    /// re-attempt, so it is the thing under test here.
    #[test]
    fn detect_short_circuits_a_second_run() {
        let conn = Connection::open_in_memory().unwrap();
        legacy_table(&conn);
        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
            1,
            "a second run must be short-circuited, since ALTER TABLE would error"
        );
    }

    /// A database with no `commit_links` at all must be reported as "nothing to
    /// do" rather than sent into an ALTER against a missing table.
    #[test]
    fn detect_is_satisfied_when_the_table_is_absent() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
            1
        );
    }

    /// The migrated table and a freshly created one must agree column-for-column
    /// — otherwise an upgraded install is structurally different from a new one
    /// and only one of them is covered by tests. Mirrors `m221`/`m222`.
    #[test]
    fn migrated_shape_matches_the_store_baseline() {
        fn columns(conn: &Connection) -> Vec<String> {
            let mut stmt = conn
                .prepare(
                    "SELECT name, type, \"notnull\", COALESCE(dflt_value, ''), pk
                       FROM pragma_table_info('commit_links') ORDER BY cid",
                )
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
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
        }

        let migrated = Connection::open_in_memory().unwrap();
        legacy_table(&migrated);
        for sql in super::MIGRATION.up {
            migrated.execute(sql, []).unwrap();
        }

        let fresh = Connection::open_in_memory().unwrap();
        fresh.execute_batch(cas_store::COMMIT_LINK_SCHEMA).unwrap();

        assert_eq!(columns(&migrated), columns(&fresh));
    }
}
