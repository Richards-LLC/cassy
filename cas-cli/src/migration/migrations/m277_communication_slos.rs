//! Additive surface observations and end-of-session communication summaries.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 277,
    name: "communication_slos",
    subsystem: Subsystem::Events,
    description: "Measure communication SLOs without changing recipient seen receipts",
    up: cas_store::communication_slo::SCHEMA_STATEMENTS,
    detect: Some("SELECT CASE WHEN
        (SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN
         ('communication_body_observations','communication_observation_epoch','communication_slo_summaries'))=3
        THEN 1 ELSE 0 END"),
};

#[cfg(test)]
mod tests {
    #[test]
    fn cas_7e24_migration_bootstrap_reconciliation_preserves_receipts() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let project = home.join("project");
            std::fs::create_dir_all(&project).unwrap();
            let root = crate::store::init_cas_dir(&project).unwrap();
            let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
            let detected = || {
                conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap()
            };
            assert_eq!(detected(), 1);
            use cas_store::PromptQueueStore;
            cas_store::SqlitePromptQueueStore::open(&root)
                .unwrap()
                .init()
                .unwrap();
            conn.execute("INSERT INTO prompt_queue_recipient_seen(prompt_id,recipient,seen_at,source) VALUES(91,'worker','2026-01-01','inbox_poll')", []).unwrap();
            conn.execute_batch("DROP TABLE communication_body_observations")
                .unwrap();
            assert_eq!(detected(), 0);
            let result = crate::migration::run_migrations(&root, false).unwrap();
            assert!(result.errors.is_empty(), "{result:?}");
            assert_eq!(detected(), 1);
            assert_eq!(
                crate::migration::run_migrations(&root, false)
                    .unwrap()
                    .applied_count,
                0
            );
            conn.execute("DELETE FROM cas_migrations", []).unwrap();
            crate::migration::bootstrap_migrations(&root).unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM cas_migrations WHERE id=277",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
            assert_eq!(
                conn.query_row(
                    "SELECT source FROM prompt_queue_recipient_seen WHERE prompt_id=91",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                "inbox_poll"
            );
        });
    }
}
