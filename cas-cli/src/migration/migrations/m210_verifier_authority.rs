//! Migration: persist typed verifier provenance and one-time capabilities.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 210,
    name: "verifier_authority",
    subsystem: Subsystem::Verification,
    description: "Add fail-closed verifier provenance and task-scoped capability storage (cas-941b)",
    up: &[
        "ALTER TABLE verifications ADD COLUMN provenance TEXT NOT NULL DEFAULT 'legacy'",
        "ALTER TABLE verifications ADD COLUMN capability_id TEXT",
        "ALTER TABLE verifications ADD COLUMN issuer_agent_id TEXT",
        "CREATE TABLE verification_capabilities (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL,
            issuer_agent_id TEXT NOT NULL,
            verifier_agent_id TEXT,
            token_hash TEXT NOT NULL,
            issued_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            bound_at TEXT,
            consumed_at TEXT
        )",
        "CREATE INDEX idx_verification_capabilities_task
            ON verification_capabilities(task_id, consumed_at, expires_at)",
    ],
    detect: Some(
        "SELECT CASE WHEN
            EXISTS (
                SELECT 1 FROM pragma_table_info('verifications')
                WHERE name = 'provenance'
            )
            AND EXISTS (
                SELECT 1 FROM pragma_table_info('verifications')
                WHERE name = 'capability_id'
            )
            AND EXISTS (
                SELECT 1 FROM pragma_table_info('verifications')
                WHERE name = 'issuer_agent_id'
            )
            AND EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'verification_capabilities'
            )
         THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    fn legacy_verification_schema(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE verifications (
                id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL,
                agent_id TEXT,
                verification_type TEXT NOT NULL DEFAULT 'task',
                status TEXT NOT NULL DEFAULT 'approved',
                confidence REAL,
                summary TEXT NOT NULL DEFAULT '',
                files_reviewed TEXT NOT NULL DEFAULT '[]',
                duration_ms INTEGER,
                created_at TEXT NOT NULL
            );",
        )
        .unwrap();
    }

    #[test]
    fn migration_adds_authority_schema_and_preserves_legacy_default() {
        let conn = Connection::open_in_memory().unwrap();
        legacy_verification_schema(&conn);
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
            "INSERT INTO verifications
             (id, task_id, verification_type, status, summary, files_reviewed, created_at)
             VALUES ('ver-old', 'cas-old', 'task', 'approved', 'old', '[]',
                     '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT provenance FROM verifications WHERE id = 'ver-old'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "legacy"
        );
        assert_eq!(
            conn.query_row(detect, [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
