//! The release gate's cross-process resource ledger (rate and quota buckets).
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 284,
    name: "qa_resource_use",
    subsystem: Subsystem::Verification,
    description: "Persist release-gate rate and quota use per environment",
    up: cas_store::QA_RESOURCE_SCHEMA_STATEMENTS,
    detect: Some("SELECT CASE WHEN
        EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='qa_resource_use')
        AND EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_qa_resource_use_bucket')
        THEN 1 ELSE 0 END"),
};

#[cfg(test)]
mod tests {
    #[test]
    fn resource_use_migration_bootstrap_reconciliation() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
        let detected = || {
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |r| r.get::<_, i64>(0))
                .unwrap()
        };
        assert_eq!(detected(), 1, "a new store has the ledger");
        assert_eq!(
            crate::migration::check_migrations(&root)
                .unwrap()
                .pending_count(),
            0
        );
        conn.execute("DROP TABLE qa_resource_use", []).unwrap();
        assert_eq!(detected(), 0);
        crate::migration::run_migrations(&root, false).unwrap();
        assert_eq!(detected(), 1, "an existing store gains it on update");
    }
}
