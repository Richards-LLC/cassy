//! Migration: quarantine state for refused embedding units (GH #695).
//!
//! Adds `embedding_error` to `history_commits` and `history_docs`.
//!
//! # Why a column and not another `pending_embedding` value
//!
//! `pending_embedding` answers "is this unit waiting for a vector". A unit the
//! provider *refuses* is not waiting — retrying the identical payload gets the
//! identical 400 — but it is also not done. Before this column the drain had
//! only two moves for such a unit: leave it pending, where the deterministic
//! queue order re-sent it on every tick and pinned the whole corpus behind it
//! (GH #695: one 138k-char commit body held 7,885 units for three days), or
//! clear it silently and lose the only evidence that the corpus is incomplete.
//! Storing the provider's message alongside the retirement gives `cas doctor` a
//! count *and* a reason, and gives `cas history embed --retry-quarantined`
//! something to re-arm.
//!
//! # Why a separate numbered step
//!
//! Same reasoning m224 records for `symbol_mapping`: m221/m222 already ran on
//! every store that has a history index and their `detect` predicates return 1
//! there, so editing their statements would only reach databases that never
//! needed the upgrade. `SqliteHistoryStore::init` performs the same idempotent
//! ALTER for any path that opens the store before the runner has had a turn;
//! the two must stay in lockstep or m224's shape-drift guard fails.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 253,
    name: "history_embedding_error",
    subsystem: Subsystem::Code,
    description:
        "Add history_commits/history_docs.embedding_error so a provider-refused unit can leave the queue with its reason (GH #695)",
    up: &[
        "ALTER TABLE history_commits ADD COLUMN embedding_error TEXT",
        "ALTER TABLE history_docs ADD COLUMN embedding_error TEXT",
    ],
    // Both tables must have it. Checking one would let a half-upgraded store
    // report "done" and then fail at query time on the other.
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (
                SELECT 1 FROM pragma_table_info('history_commits')
                WHERE name = 'embedding_error'
            )
            AND EXISTS (
                SELECT 1 FROM pragma_table_info('history_docs')
                WHERE name = 'embedding_error'
            )
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    /// A store that has been through the runner must end up with the same
    /// shape as one created fresh from the declarative schema. This is the
    /// guard that catches a column added in only one of the two places.
    #[test]
    fn upgraded_history_tables_match_a_fresh_store() {
        fn columns(conn: &Connection, table: &str) -> Vec<String> {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT name FROM pragma_table_info('{table}') ORDER BY name"
                ))
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        }

        // The pre-GH#695 shape: today's schema minus the column this migration
        // adds. Written as an explicit DROP rather than a literal copy of the
        // old schema so the fixture cannot drift away from the real tables.
        let upgraded = Connection::open_in_memory().unwrap();
        upgraded
            .execute_batch(cas_store::HISTORY_SCHEMA)
            .unwrap();
        upgraded
            .execute_batch(cas_store::HISTORY_DOCS_SCHEMA)
            .unwrap();
        for table in ["history_commits", "history_docs"] {
            upgraded
                .execute_batch(&format!("ALTER TABLE {table} DROP COLUMN embedding_error"))
                .unwrap();
        }
        for statement in super::MIGRATION.up {
            upgraded.execute_batch(statement).unwrap();
        }

        let fresh = Connection::open_in_memory().unwrap();
        fresh.execute_batch(cas_store::HISTORY_SCHEMA).unwrap();
        fresh.execute_batch(cas_store::HISTORY_DOCS_SCHEMA).unwrap();

        for table in ["history_commits", "history_docs"] {
            assert_eq!(
                columns(&upgraded, table),
                columns(&fresh, table),
                "shape drift for {table} between the upgrade path and a fresh store"
            );
        }
    }

    /// `detect` must be false before the migration and true after, or the
    /// runner either skips a store that needs it or re-runs an ALTER that
    /// fails on the second pass.
    #[test]
    fn detect_flips_only_once_both_tables_have_the_column() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(cas_store::HISTORY_SCHEMA).unwrap();
        conn.execute_batch(cas_store::HISTORY_DOCS_SCHEMA).unwrap();
        let detect = super::MIGRATION.detect.unwrap();

        let done: i64 = conn.query_row(detect, [], |row| row.get(0)).unwrap();
        assert_eq!(done, 1, "a fresh store already satisfies this migration");

        conn.execute_batch("ALTER TABLE history_docs DROP COLUMN embedding_error")
            .unwrap();
        let done: i64 = conn.query_row(detect, [], |row| row.get(0)).unwrap();
        assert_eq!(done, 0, "one table missing the column is not done");
    }
}
