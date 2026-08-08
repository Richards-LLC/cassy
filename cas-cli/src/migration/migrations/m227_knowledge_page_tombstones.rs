//! Migration: persist knowledge-page tombstones for cloud sync (cas-e6aa).
//!
//! The page row is intentionally gone after a delete, so this is a separate
//! ledger rather than a column on `knowledge_pages`. It distinguishes locally
//! authored, not-yet-pushed deletes from received tombstones that only guard
//! against stale-page resurrection.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 227,
    name: "knowledge_page_tombstones",
    subsystem: Subsystem::Knowledge,
    description: "Add durable knowledge-page tombstones for cloud delete propagation (cas-e6aa)",
    up: cas_store::KNOWLEDGE_PAGE_TOMBSTONE_STATEMENTS,
    detect: Some(
        "SELECT CASE WHEN
             NOT EXISTS (
                 SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'knowledge_pages'
             )
             OR EXISTS (
                 SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'knowledge_page_tombstones'
             )
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_the_tombstone_ledger_for_existing_knowledge_stores() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE knowledge_pages (id TEXT PRIMARY KEY);
             INSERT INTO knowledge_pages VALUES ('cas-kn001')",
        )
        .unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );

        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }
        let (id, locally_authored, pushed_at): (String, i64, Option<String>) = conn
            .query_row(
                "INSERT INTO knowledge_page_tombstones (id, deleted_at)
                 VALUES ('cas-kn001', '2026-08-08T00:00:00Z')
                 RETURNING id, locally_authored, pushed_at",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(id, "cas-kn001");
        assert_eq!(locally_authored, 0);
        assert_eq!(pushed_at, None);
    }
}
