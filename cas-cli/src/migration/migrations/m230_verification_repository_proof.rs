//! Migration: bind legacy task verification to an immutable repository state.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 230,
    name: "verification_repository_proof",
    subsystem: Subsystem::Verification,
    description: "Persist an optional immutable repository proof on verification dispatches (cas-05ee)",
    up: &[cas_store::VERIFICATION_DISPATCH_REPOSITORY_PROOF_STATEMENT],
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

    #[test]
    fn store_open_repair_leaves_m230_for_the_migration_ledger_to_record() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let project = home.join("project");
            std::fs::create_dir_all(&project).unwrap();
            crate::store::init_cas_dir(&project).expect("initialize current CAS store");
            let cas_dir = project.join(".cas");

            let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
            conn.execute_batch(
                "ALTER TABLE verification_dispatches DROP COLUMN repository_proof;
                 DELETE FROM cas_migrations WHERE id = 230;",
            )
            .expect("restore a pre-m230 dispatch table");
            drop(conn);

            let store = cas_store::SqliteVerificationStore::open(&cas_dir)
                .expect("store open self-heals the missing m230 column");
            drop(store);

            crate::migration::check_migrations(&cas_dir)
                .expect("the numbered migration remains detectable after repair");
            let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
            let applied_at: String = conn
                .query_row(
                    "SELECT applied_at FROM cas_migrations WHERE id = 230",
                    [],
                    |row| row.get(0),
                )
                .expect("m230 must retain its durable ledger record");
            assert_eq!(applied_at, "DETECTED");
        });
    }
}
