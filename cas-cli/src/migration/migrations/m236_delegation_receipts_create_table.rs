//! Migration: durable, local receipts and reservations for external delegation.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 236,
    name: "delegation_receipts_create_table",
    subsystem: Subsystem::Verification,
    description: "Persist gateway idempotency receipts and local budget reservations (cas-869c)",
    // The migration runner executes each entry independently. Keep this
    // split even though the store's lazy bootstrap uses one batch string.
    up: &[
        "CREATE TABLE IF NOT EXISTS delegation_receipts (id TEXT PRIMARY KEY, factory_session_id TEXT NOT NULL, epic_id TEXT NOT NULL, task_id TEXT NOT NULL, gate_kind TEXT NOT NULL, request_digest TEXT NOT NULL, attempt INTEGER NOT NULL, idempotency_key TEXT NOT NULL UNIQUE, reserved_amount INTEGER NOT NULL, settled_amount INTEGER, state TEXT NOT NULL, run_id TEXT, terminal_verdict TEXT, evidence_reference TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, completed_at TEXT, UNIQUE(factory_session_id, task_id, gate_kind, request_digest))",
        "CREATE INDEX IF NOT EXISTS idx_delegation_receipts_session_active ON delegation_receipts(factory_session_id, state)",
        "CREATE INDEX IF NOT EXISTS idx_delegation_receipts_epic_active ON delegation_receipts(epic_id, state)",
        "CREATE INDEX IF NOT EXISTS idx_delegation_receipts_task ON delegation_receipts(task_id, created_at DESC)",
    ],
    detect: Some(
        "SELECT CASE WHEN (SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='delegation_receipts') = 1 THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_delegation_receipts() {
        let conn = Connection::open_in_memory().unwrap();
        for statement in super::MIGRATION.up {
            conn.execute(statement, []).unwrap();
        }
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
