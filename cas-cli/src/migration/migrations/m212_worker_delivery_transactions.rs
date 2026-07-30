//! Migration: transactional worker completion receipts through epic merge.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 212,
    name: "worker_delivery_transactions",
    subsystem: Subsystem::Tasks,
    description: "Add immutable worker completion receipts and resumable delivery state (cas-60a6)",
    up: &[
        "CREATE TABLE worker_completion_receipts (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL,
            worker_agent_id TEXT NOT NULL,
            worker_name TEXT NOT NULL,
            repo_selector TEXT NOT NULL,
            source_branch TEXT NOT NULL,
            commit_sha TEXT NOT NULL,
            merge_base_sha TEXT NOT NULL,
            target_branch TEXT NOT NULL,
            target_sha TEXT NOT NULL,
            proof_reference TEXT NOT NULL,
            scope_summary TEXT NOT NULL,
            created_at TEXT NOT NULL
        )",
        "CREATE INDEX idx_worker_completion_receipts_task
            ON worker_completion_receipts(task_id, created_at DESC)",
        "CREATE TRIGGER worker_completion_receipts_immutable_update
            BEFORE UPDATE ON worker_completion_receipts
            BEGIN SELECT RAISE(ABORT, 'worker completion receipts are immutable'); END",
        "CREATE TRIGGER worker_completion_receipts_immutable_delete
            BEFORE DELETE ON worker_completion_receipts
            BEGIN SELECT RAISE(ABORT, 'worker completion receipts are immutable'); END",
        "CREATE TABLE worker_delivery_transactions (
            id TEXT PRIMARY KEY,
            receipt_id TEXT NOT NULL UNIQUE,
            task_id TEXT NOT NULL,
            state TEXT NOT NULL,
            supervisor_agent_id TEXT,
            verification_id TEXT,
            merge_commit_sha TEXT,
            last_error_code TEXT,
            last_error_detail TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY(receipt_id) REFERENCES worker_completion_receipts(id)
        )",
        "CREATE INDEX idx_worker_delivery_transactions_task
            ON worker_delivery_transactions(task_id, created_at DESC)",
        "CREATE TABLE worker_delivery_events (
            id TEXT PRIMARY KEY,
            transaction_id TEXT NOT NULL,
            state TEXT NOT NULL,
            actor_agent_id TEXT NOT NULL,
            detail TEXT,
            created_at TEXT NOT NULL,
            UNIQUE(transaction_id, state),
            FOREIGN KEY(transaction_id) REFERENCES worker_delivery_transactions(id)
        )",
        "CREATE TRIGGER worker_delivery_events_append_only_update
            BEFORE UPDATE ON worker_delivery_events
            BEGIN SELECT RAISE(ABORT, 'worker delivery events are append-only'); END",
        "CREATE TRIGGER worker_delivery_events_append_only_delete
            BEFORE DELETE ON worker_delivery_events
            BEGIN SELECT RAISE(ABORT, 'worker delivery events are append-only'); END",
    ],
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='worker_completion_receipts')
            AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='worker_delivery_transactions')
            AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='worker_delivery_events')
            AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='trigger' AND name='worker_completion_receipts_immutable_update')
            AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='trigger' AND name='worker_delivery_events_append_only_update')
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_adds_immutable_receipt_and_delivery_state() {
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
}
