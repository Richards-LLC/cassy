//! Migration: create `history_epochs` (EPIC cas-6212 / cas-8d2a, spec §9).
//!
//! One row per observed binary epoch, so "is symptom X fixed" is answered
//! against the timeline of binaries that were actually *running* rather than
//! against a tag date. Registered under [`Subsystem::Code`] beside `m221`'s
//! structural git tables and `m222`'s docs table.
//!
//! Separate from `m221` for the same reason `m223` is: `m221` has already run
//! on live databases and its `detect` predicate would short-circuit, so a table
//! folded into it would never reach those installs.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 224,
    name: "history_epochs_create_table",
    subsystem: Subsystem::Code,
    description:
        "Create history_epochs: running-binary timeline for is-it-fixed verdicts (cas-8d2a)",
    up: cas_store::HISTORY_EPOCHS_SCHEMA_STATEMENTS,
    detect: Some(
        "SELECT CASE WHEN EXISTS (
             SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'history_epochs'
         ) THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_and_detects_history_epochs() {
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

    /// The identity index is the whole idempotency story for epoch recording:
    /// without it a re-registering daemon grows a second window for the same
    /// process, widening MIXED and suppressing CLEAN-POST evidence.
    #[test]
    fn identity_index_rejects_a_duplicate_epoch() {
        let conn = Connection::open_in_memory().unwrap();
        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }
        let insert = "INSERT INTO history_epochs (epoch_kind, started_at, pid, recorded_at)
                      VALUES ('daemon_start', '2026-08-07T21:02:26Z', 42, 'now')";
        conn.execute(insert, []).unwrap();
        assert!(
            conn.execute(insert, []).is_err(),
            "a second row with the same (kind, pid, started_at) must be rejected"
        );
    }

    /// Compares the FULL column definition (type, NOT NULL, default, pk), the
    /// index set and the normalized DDL — mirrors `m221`/`m222`'s test, so a
    /// dropped NOT NULL or a lost default cannot pass as identical.
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
            .execute_batch(cas_store::HISTORY_EPOCHS_SCHEMA)
            .unwrap();
        let migrated = Connection::open_in_memory().unwrap();
        for sql in super::MIGRATION.up {
            migrated.execute(sql, []).unwrap();
        }

        assert_eq!(
            shape(&baseline, "history_epochs"),
            shape(&migrated, "history_epochs"),
            "shape drift for history_epochs"
        );
    }

    /// Additive over an `m221` database: the M1 rows must survive.
    #[test]
    fn m224_is_additive_over_an_m221_database() {
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
            "an m221-only database must not report history_epochs present"
        );

        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }
        let commits: i64 = conn
            .query_row("SELECT COUNT(*) FROM history_commits", [], |r| r.get(0))
            .unwrap();
        assert_eq!(commits, 1, "M1 rows must survive the M8 migration");
    }
}
