//! Migration: store the server's per-row revision for conflict resolution (cas-c32f).
//!
//! The cloud increments a monotonic `revision` on every accepted write. Keeping
//! the last observed value lets the client compare revisions instead of clocks,
//! and lets a push declare the base revision it is updating from.

use crate::cloud::SYNC_REVISION_STATEMENTS;
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 251,
    name: "sync_revisions",
    subsystem: Subsystem::Tasks,
    description: "Track server-owned per-row sync revisions for conflict resolution (cas-c32f)",
    up: SYNC_REVISION_STATEMENTS,
    detect: Some(
        "SELECT EXISTS (
             SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'sync_revisions'
         )",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_the_revision_ledger_keyed_by_entity() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );

        for statement in super::MIGRATION.up {
            conn.execute_batch(statement).unwrap();
        }
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );

        // One row per (entity_type, entity_id): the same id in two entity
        // types is two independent revisions, and a repeat write replaces.
        conn.execute_batch(
            "INSERT INTO sync_revisions (entity_type, entity_id, revision, updated_at)
                 VALUES ('task', 'cas-a', 3, '2026-09-03T12:00:00+00:00');
             INSERT INTO sync_revisions (entity_type, entity_id, revision, updated_at)
                 VALUES ('entry', 'cas-a', 9, '2026-09-03T12:00:00+00:00');",
        )
        .unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT revision FROM sync_revisions WHERE entity_type = 'task' AND entity_id = 'cas-a'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            3
        );
        assert!(
            conn.execute(
                "INSERT INTO sync_revisions (entity_type, entity_id, revision, updated_at)
                 VALUES ('task', 'cas-a', 4, '2026-09-03T12:00:01+00:00')",
                [],
            )
            .is_err(),
            "the primary key must reject a duplicate (entity_type, entity_id)"
        );
    }
}
