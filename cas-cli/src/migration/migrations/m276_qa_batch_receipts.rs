//! Project-scoped staging QA receipts and durable deferred-evidence obligations.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 276, name: "qa_batch_receipts", subsystem: Subsystem::Verification,
    description: "Record immutable batch QA receipts and evidence deferred at park (GH #1203)",
    up: cas_store::QA_BATCH_SCHEMA_STATEMENTS,
    detect: Some("SELECT CASE WHEN
        EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='qa_batch_receipts')
        AND EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='qa_batch_deferrals')
        AND EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_qa_batch_receipts_project')
        THEN 1 ELSE 0 END"),
};

#[cfg(test)]
mod tests {
    #[test]
    fn batch_qa_bootstrap_reconciliation_cas_d697() {
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
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
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
    }
}
