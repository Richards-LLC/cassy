//! Migration: map the removed pending-supervisor-review task status.
//!
//! `pending_supervisor_review` was a local-only queue state. Existing rows
//! represent work waiting for the supervisor to merge, so they continue as
//! `awaiting_merge` and no longer advertise a verification request.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 248,
    name: "tasks_retire_pending_supervisor_review",
    subsystem: Subsystem::Tasks,
    description: "Map legacy pending_supervisor_review task rows to awaiting_merge",
    up: &["UPDATE tasks SET status = 'awaiting_merge', pending_verification = 0, updated_at = CURRENT_TIMESTAMP WHERE status = 'pending_supervisor_review'"],
    detect: Some(
        "SELECT CASE WHEN NOT EXISTS (SELECT 1 FROM tasks WHERE status = 'pending_supervisor_review') THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_maps_legacy_rows_and_clears_verification_pending() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (
                id TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                pending_verification INTEGER NOT NULL,
                updated_at TEXT NOT NULL
            );
            INSERT INTO tasks (id, status, pending_verification, updated_at)
            VALUES ('cas-legacy', 'pending_supervisor_review', 1, '2026-08-01T00:00:00Z');
            INSERT INTO tasks (id, status, pending_verification, updated_at)
            VALUES ('cas-open', 'open', 0, '2026-08-01T00:00:00Z');",
        )
        .unwrap();

        let before: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(before, 0);

        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }

        let mapped: (String, i64) = conn
            .query_row(
                "SELECT status, pending_verification FROM tasks WHERE id = 'cas-legacy'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(mapped, ("awaiting_merge".to_string(), 0));
        let untouched: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE id = 'cas-open'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(untouched, "open");

        let after: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(after, 1);
    }
}
