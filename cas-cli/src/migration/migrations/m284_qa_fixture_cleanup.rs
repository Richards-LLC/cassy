//! Durable cleanup handles for staging fixture recipes.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 284,
    name: "qa_fixture_cleanup",
    subsystem: Subsystem::Verification,
    description: "Persist release fixture cleanup handles and orphan debt",
    up: cas_store::QA_FIXTURE_SCHEMA_STATEMENTS,
    detect: Some("SELECT CASE WHEN
        EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='qa_fixture_cleanup')
        AND EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_qa_fixture_cleanup_expiry')
        THEN 1 ELSE 0 END"),
};

#[cfg(test)]
mod tests {
    #[test]
    fn fixture_cleanup_migration_bootstrap_reconciliation() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let detected = || {
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |r| r.get::<_, i64>(0))
                .unwrap()
        };
        assert_eq!(detected(), 0);
        for _ in 0..2 {
            for sql in super::MIGRATION.up {
                conn.execute(sql, []).unwrap();
            }
            assert_eq!(detected(), 1);
        }
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            crate::migration::check_migrations(&root)
                .unwrap()
                .pending_count(),
            0
        );
        conn.execute("DROP TABLE qa_fixture_cleanup", []).unwrap();
        crate::migration::run_migrations(&root, false).unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
