//! Migration: surfacing-receipt provenance on `prompt_queue_recipient_seen`.
//!
//! cas-7a01 (GH #155). The receipt table recorded THAT a row was surfaced but
//! not BY WHICH path, so an `inbox_poll` drain (the recipient chose to look)
//! and a turn-start hook injection (CAS put the message in front of a
//! recipient that did not know to look) were indistinguishable. Only the
//! second is evidence that the delivery bug this task fixes was actually
//! repaired for a given message, which is why `message_status` now reports an
//! observed wake from it.
//!
//! # Why only one of this change's four columns is here
//!
//! The same change also adds `wake_attempt`, `wake_attempt_at` and
//! `wake_attempt_detail` to `prompt_queue`. Those are deliberately NOT in the
//! ledger, because `prompt_queue` columns are not ledger-managed: all
//! seventeen of its existing added columns (`acked_via`, `urgent`,
//! `dedupe_key`, `highest_stage`, …) are installed by `ensure_column` inside
//! `SqlitePromptQueueStore::init`, which is idempotent and runs on every open.
//! Nothing in the ledger creates `prompt_queue` at all — `ensure_base_schemas`
//! covers only the subsystems whose canonical DDL lives in a store
//! constructor, and the prompt queue is not one of them. An `ALTER TABLE
//! prompt_queue` here therefore runs against a database where that table may
//! not exist yet and fails outright (`no such table: prompt_queue`) — which is
//! exactly what the daemon's delivery tests caught when this migration was
//! first written to include them.
//!
//! `prompt_queue_recipient_seen` is different: it IS ledger-managed (m208
//! creates it, m218 created its transport sibling), and migrations apply in id
//! order, so the table is guaranteed to exist by the time this runs.
//!
//! `source` is deliberately NOT back-filled to `'inbox_poll'`. Every existing
//! receipt did come from that path, but a NULL meaning "provenance unknown" is
//! safer than a value asserting provenance CAS never actually recorded.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 220,
    name: "prompt_queue_recipient_seen_add_source",
    subsystem: Subsystem::Agents,
    description: "Record which surfacing path wrote each recipient receipt (cas-7a01)",
    up: &["ALTER TABLE prompt_queue_recipient_seen ADD COLUMN source TEXT"],
    detect: Some(
        "SELECT CASE WHEN EXISTS (
            SELECT 1 FROM pragma_table_info('prompt_queue_recipient_seen') WHERE name = 'source'
         ) THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use cas_store::{PromptQueueStore, SqlitePromptQueueStore};
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn columns(conn: &Connection, table: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT name FROM pragma_table_info('{table}') ORDER BY cid"
            ))
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    /// Applied against the table shape m208 installs — the only shape this
    /// migration can encounter, since migrations run in id order.
    #[test]
    fn migration_adds_and_detects_the_source_column() {
        let conn = Connection::open_in_memory().unwrap();
        for sql in super::super::m208_prompt_queue_recipient_seen_create_table::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }

        let before: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(before, 0, "a pre-migration store must not report applied");

        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }

        let after: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(after, 1);
    }

    /// The ledger and the baseline schema must agree, or a migrated store and
    /// a freshly-initialised one disagree about the shape of the evidence
    /// `message_status` now reports wake state from.
    #[test]
    fn baseline_store_carries_the_source_column() {
        let temp = TempDir::new().unwrap();
        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        let baseline = Connection::open(temp.path().join("cas.db")).unwrap();

        assert!(
            columns(&baseline, "prompt_queue_recipient_seen")
                .iter()
                .any(|c| c == "source"),
            "baseline receipt table is missing the surfacing-source column"
        );
    }

    /// The wake columns are store-managed, not ledger-managed. This pins that
    /// contract: `init()` must install them, because no migration will.
    #[test]
    fn store_init_installs_the_wake_attempt_columns() {
        let temp = TempDir::new().unwrap();
        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        let baseline = Connection::open(temp.path().join("cas.db")).unwrap();

        let queue_columns = columns(&baseline, "prompt_queue");
        for expected in ["wake_attempt", "wake_attempt_at", "wake_attempt_detail"] {
            assert!(
                queue_columns.iter().any(|c| c == expected),
                "baseline prompt_queue is missing {expected}: {queue_columns:?}"
            );
        }
    }
}
