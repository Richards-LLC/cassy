//! Migration: persist explicit task-scoped verification dispatch state.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 211,
    name: "verification_dispatches",
    subsystem: Subsystem::Verification,
    description: "Add explicit verifier owner, deadline, state, and recovery metadata (cas-08ca)",
    up: &[
        "CREATE TABLE verification_dispatches (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL,
            requester_agent_id TEXT NOT NULL,
            owner_agent_id TEXT NOT NULL,
            verifier_agent_id TEXT,
            capability_id TEXT,
            state TEXT NOT NULL DEFAULT 'pending',
            requested_at TEXT NOT NULL,
            deadline_at TEXT NOT NULL,
            resolved_at TEXT,
            recovery_action TEXT NOT NULL DEFAULT 'supervisor_redispatch_or_direct'
        )",
        "CREATE INDEX idx_verification_dispatches_task
            ON verification_dispatches(task_id, requested_at DESC)",
        "CREATE UNIQUE INDEX idx_verification_dispatches_active_task
            ON verification_dispatches(task_id)
            WHERE state IN ('pending', 'claimed')",
    ],
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'verification_dispatches'
            )
            AND EXISTS (
                SELECT 1 FROM pragma_table_info('verification_dispatches')
                WHERE name = 'owner_agent_id'
            )
            AND EXISTS (
                SELECT 1 FROM pragma_table_info('verification_dispatches')
                WHERE name = 'deadline_at'
            )
            AND EXISTS (
                SELECT 1 FROM pragma_table_info('verification_dispatches')
                WHERE name = 'recovery_action'
            )
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_adds_explicit_dispatch_lifecycle() {
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

        conn.execute(
            "INSERT INTO verification_dispatches
             (id, task_id, requester_agent_id, owner_agent_id, requested_at, deadline_at)
             VALUES ('vdispatch-test', 'cas-test', 'worker', 'supervisor',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:10:00Z')",
            [],
        )
        .unwrap();
        let (state, recovery): (String, String) = conn
            .query_row(
                "SELECT state, recovery_action
                 FROM verification_dispatches WHERE id = 'vdispatch-test'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, "pending");
        assert_eq!(recovery, "supervisor_redispatch_or_direct");
        assert_eq!(
            conn.query_row(detect, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
