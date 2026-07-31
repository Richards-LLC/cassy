//! Migration: distinguish sealed server-side verifier handoffs from legacy bearers.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 215,
    name: "verifier_server_handoffs",
    subsystem: Subsystem::Tasks,
    description: "Add server-side verifier handoff transport and hook correlation (cas-6939)",
    up: &[
        "CREATE TABLE IF NOT EXISTS verification_handoffs (
            capability_id TEXT PRIMARY KEY,
            issuer_agent_id TEXT NOT NULL,
            verifier_agent_id TEXT,
            tool_use_id_hash TEXT NOT NULL,
            state TEXT NOT NULL DEFAULT 'pending',
            created_at TEXT NOT NULL,
            bound_at TEXT,
            consumed_at TEXT,
            FOREIGN KEY (capability_id)
                REFERENCES verification_capabilities(id) ON DELETE CASCADE
        )",
        "CREATE INDEX IF NOT EXISTS idx_verification_handoffs_child
            ON verification_handoffs(verifier_agent_id, state)",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_verification_handoffs_pending_parent
            ON verification_handoffs(issuer_agent_id)
            WHERE state = 'pending'",
    ],
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'verification_handoffs'
            )
            AND EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'index'
                  AND name = 'idx_verification_handoffs_child'
            )
            AND EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'index'
                  AND name = 'idx_verification_handoffs_pending_parent'
            )
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_preserves_legacy_bearers_and_enforces_one_unbound_handoff_per_parent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE verification_capabilities (
                id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL,
                dispatch_id TEXT,
                issuer_agent_id TEXT NOT NULL,
                verifier_agent_id TEXT,
                token_hash TEXT NOT NULL,
                issued_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                bound_at TEXT,
                consumed_at TEXT
            );
            INSERT INTO verification_capabilities
                (id, task_id, dispatch_id, issuer_agent_id, verifier_agent_id,
                 token_hash, issued_at, expires_at, bound_at, consumed_at)
            VALUES
                ('vcap-legacy', 'cas-legacy', NULL, 'parent', NULL,
                 'digest', '2026-07-30T00:00:00Z', '2026-07-31T00:00:00Z',
                 NULL, NULL);",
        )
        .unwrap();
        for _ in 0..2 {
            for sql in super::MIGRATION.up {
                conn.execute(sql, []).unwrap();
            }
        }
        let legacy_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM verification_capabilities
                 WHERE id = 'vcap-legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy_count, 1);

        let insert = |id: &str| {
            conn.execute(
                "INSERT INTO verification_capabilities
                    (id, task_id, dispatch_id, issuer_agent_id, verifier_agent_id,
                     token_hash, issued_at, expires_at, bound_at, consumed_at)
                 VALUES (?1, 'cas-a', 'dispatch-a', 'parent', NULL, 'digest',
                         '2026-07-30T00:00:00Z', '2026-07-31T00:00:00Z',
                         NULL, NULL)",
                [id],
            )?;
            conn.execute(
                "INSERT INTO verification_handoffs
                    (capability_id, issuer_agent_id, verifier_agent_id,
                     tool_use_id_hash, state, created_at, bound_at, consumed_at)
                 VALUES (?1, 'parent', NULL, 'tool-hash', 'pending',
                         '2026-07-30T00:00:00Z', NULL, NULL)",
                [id],
            )
        };
        insert("vhnd-one").unwrap();
        assert!(
            insert("vhnd-two").is_err(),
            "database must reject a second live unbound handoff for one parent"
        );
    }
}
