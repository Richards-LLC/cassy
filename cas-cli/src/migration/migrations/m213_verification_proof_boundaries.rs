//! Migration: bind verifier authority and verdicts to one exact proof cycle.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 213,
    name: "verification_proof_boundaries",
    subsystem: Subsystem::Verification,
    description: "Bind dispatches, capabilities, verdicts, and delivery to one exact proof boundary (cas-66b6)",
    up: &[
        "ALTER TABLE verification_dispatches ADD COLUMN receipt_id TEXT",
        "ALTER TABLE verification_dispatches ADD COLUMN delivery_transaction_id TEXT",
        "ALTER TABLE verification_capabilities ADD COLUMN dispatch_id TEXT",
        "ALTER TABLE verifications ADD COLUMN dispatch_id TEXT",
        "CREATE INDEX idx_verification_capabilities_dispatch
            ON verification_capabilities(dispatch_id)",
        "CREATE INDEX idx_verifications_dispatch
            ON verifications(dispatch_id, created_at DESC)",
    ],
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (
                SELECT 1 FROM pragma_table_info('verification_dispatches')
                WHERE name = 'receipt_id'
            )
            AND EXISTS (
                SELECT 1 FROM pragma_table_info('verification_dispatches')
                WHERE name = 'delivery_transaction_id'
            )
            AND EXISTS (
                SELECT 1 FROM pragma_table_info('verification_capabilities')
                WHERE name = 'dispatch_id'
            )
            AND EXISTS (
                SELECT 1 FROM pragma_table_info('verifications')
                WHERE name = 'dispatch_id'
            )
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_adds_nullable_legacy_safe_exact_boundary_links() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE verification_dispatches (
                id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL
             );
             CREATE TABLE verification_capabilities (
                id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL
             );
             CREATE TABLE verifications (
                id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL,
                verification_type TEXT NOT NULL,
                provenance TEXT NOT NULL,
                status TEXT NOT NULL,
                summary TEXT NOT NULL,
                files_reviewed TEXT NOT NULL,
                created_at TEXT NOT NULL
             );
             INSERT INTO verifications
             (id, task_id, verification_type, provenance, status, summary,
              files_reviewed, created_at)
             VALUES ('ver-legacy', 'cas-legacy', 'task', 'legacy', 'approved',
                     'readable only', '[]', '2026-01-01T00:00:00Z');",
        )
        .unwrap();
        let detect = super::MIGRATION.detect.unwrap();
        assert_eq!(
            conn.query_row(detect, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        for statement in super::MIGRATION.up {
            conn.execute_batch(statement).unwrap();
        }
        assert_eq!(
            conn.query_row(detect, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(
            conn.query_row(
                "SELECT dispatch_id FROM verifications WHERE id = 'ver-legacy'",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn current_bootstrap_schema_detects_migration_as_already_applied() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(cas_store::VERIFICATION_SCHEMA).unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );
    }
}
