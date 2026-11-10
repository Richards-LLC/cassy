//! Audited artifact tombstones are independent of upload phase metadata.
use crate::migration::{Migration, Subsystem};
pub const MIGRATION: Migration = Migration {
    id: 298,
    name: "artifact_revocations",
    subsystem: Subsystem::Tasks,
    description: "Retain artifact revocation audit and Cloud deletion acknowledgement",
    up: cas_store::ARTIFACT_REVOCATION_SCHEMA_STATEMENTS,
    detect: Some("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='artifact_revocations')"),
};

#[cfg(test)]
mod tests {
    #[test]
    fn artifact_revocation_migration_is_idempotent_cas_6d35() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(cas_store::ARTIFACT_SCHEMA).unwrap();
        let detected = || conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get::<_, i64>(0)).unwrap();
        assert_eq!(detected(), 0);
        for _ in 0..2 {
            for statement in super::MIGRATION.up { conn.execute(statement, []).unwrap(); }
            assert_eq!(detected(), 1);
        }
        assert_eq!(conn.query_row("SELECT count(*) FROM artifacts", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    }
}
