//! Migration: bind legacy task verification to an immutable repository state.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 230,
    name: "verification_repository_proof",
    subsystem: Subsystem::Verification,
    description: "Persist an optional immutable repository proof on verification dispatches (cas-05ee)",
    up: &["ALTER TABLE verification_dispatches ADD COLUMN repository_proof TEXT"],
    detect: Some(
        "SELECT EXISTS (
            SELECT 1 FROM pragma_table_info('verification_dispatches')
            WHERE name = 'repository_proof'
         )",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_is_nullable_for_existing_dispatches() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE verification_dispatches (
                id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL
             );
             INSERT INTO verification_dispatches (id, task_id)
             VALUES ('vdispatch-legacy', 'cas-legacy');",
        )
        .unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        for statement in super::MIGRATION.up {
            conn.execute(statement, []).unwrap();
        }
        assert!(
            conn.query_row(
                "SELECT repository_proof FROM verification_dispatches WHERE id = 'vdispatch-legacy'",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn current_bootstrap_schema_detects_migration_as_applied() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(cas_store::VERIFICATION_SCHEMA).unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
