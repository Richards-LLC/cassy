//! Migration: persist dependency-edge tombstones received from the cloud (cas-cf1f).
//!
//! The `dependencies` row is gone once an edge is deleted, so a received
//! deletion needs its own ledger. Without it, the next reconciliation sees a
//! local-only edge on a machine that has not yet pulled the delete and pushes
//! it straight back — the resurrection GH #640 exists to prevent.

use crate::cloud::TASK_DEPENDENCY_TOMBSTONE_STATEMENTS;
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 249,
    name: "task_dependency_tombstones",
    subsystem: Subsystem::Tasks,
    description: "Add the local dependency-edge tombstone ledger for cloud delete propagation (cas-cf1f)",
    up: TASK_DEPENDENCY_TOMBSTONE_STATEMENTS,
    detect: Some(
        "SELECT EXISTS (
             SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'task_dependency_tombstones'
         )",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_the_tombstone_ledger_for_existing_task_stores() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE dependencies (
                 from_id TEXT NOT NULL,
                 to_id TEXT NOT NULL,
                 dep_type TEXT NOT NULL
             )",
        )
        .unwrap();
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
        let deleted_at: String = conn
            .query_row(
                "INSERT INTO task_dependency_tombstones
                     (id, from_id, to_id, dep_type, deleted_at, recorded_at)
                 VALUES ('cas-a:cas-b:blocks', 'cas-a', 'cas-b', 'blocks',
                         '2026-09-03T12:00:00+00:00', '2026-09-03T12:00:01+00:00')
                 RETURNING deleted_at",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(deleted_at, "2026-09-03T12:00:00+00:00");
    }
}
