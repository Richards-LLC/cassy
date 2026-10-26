//! Preserve supervisor succession independently of disposable registry rows.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 283,
    name: "agent_session_ends",
    subsystem: Subsystem::Agents,
    description: "Retain session-end causes and atomically claimed supervisor lane snapshots",
    // Canonical additive baseline: safe on both lazily initialized and migrated stores.
    up: cas_store::AGENT_SESSION_END_SCHEMA,
    detect: Some(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='agent_session_ends'",
    ),
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_end_schema_bootstrap_and_recorded_drop_reconcile_cas_790e() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let project = home.join("project");
            std::fs::create_dir_all(&project).unwrap();
            let root = crate::store::init_cas_dir(&project).unwrap();
            let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
            let detected = || {
                conn.query_row(MIGRATION.detect.unwrap(), [], |r| r.get::<_, i64>(0))
                    .unwrap()
            };
            assert_eq!(detected(), 1);
            assert!(
                crate::migration::detector::detect_applied_migrations(&root)
                    .unwrap()
                    .contains(&283)
            );
            conn.execute_batch("DROP TABLE agent_session_ends").unwrap();
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
        });
    }
}
