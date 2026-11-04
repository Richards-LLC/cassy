//! Attribute verification duration without backfilling unknown legacy timings.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 293,
    name: "verification_duration_source",
    subsystem: Subsystem::Verification,
    description: "Label explicit, child-runtime and dispatch-fallback verification durations",
    up: &[cas_store::VERIFICATION_DURATION_SOURCE_STATEMENT],
    detect: Some(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('verifications') WHERE name = 'duration_source')",
    ),
};

#[cfg(test)]
mod tests {
    use cas_store::VerificationStore;

    #[test]
    fn legacy_timings_stay_unknown_after_bootstrap_and_migration() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let project = home.join("project");
            std::fs::create_dir_all(&project).unwrap();
            let root = crate::store::init_cas_dir(&project).unwrap();
            let store = cas_store::SqliteVerificationStore::open(&root).unwrap();
            let mut legacy = cas_types::Verification::new("ver-legacy".into(), "cas-legacy".into());
            legacy.set_duration(1500);
            store.add(&legacy).unwrap();
            let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
            assert_eq!(
                conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
            conn.execute_batch("ALTER TABLE verifications DROP COLUMN duration_source; DELETE FROM cas_migrations WHERE id = 293;").unwrap();
            assert_eq!(
                conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            let repaired = cas_store::SqliteVerificationStore::open(&root).unwrap();
            let restored = repaired.get(&legacy.id).unwrap();
            assert_eq!(restored.duration_ms, Some(1500));
            assert!(restored.duration_source.is_none());
            crate::migration::check_migrations(&root).unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT applied_at FROM cas_migrations WHERE id = 293",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
                "DETECTED"
            );
            cas_store::SqliteVerificationStore::open(&root).unwrap();
        });
    }
}
