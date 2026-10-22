//! Persist per-child split releases independently of historical split rows.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 286,
    name: "epic_split_releases",
    subsystem: Subsystem::Agents,
    description: "Remember released split children so obsolete shares stop projecting",
    up: cas_store::EPIC_SPLIT_RELEASE_SCHEMA_STATEMENTS,
    detect: Some(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='epic_split_releases'",
    ),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_release_schema_bootstrap_and_recorded_drop_reconcile_cas_8503() {
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
                    .contains(&286)
            );

            // An already-recorded migration must repair a dropped table.
            conn.execute_batch("DROP TABLE epic_split_releases")
                .unwrap();
            assert_eq!(detected(), 0);
            let result = crate::migration::run_migrations(&root, false).unwrap();
            assert!(result.errors.is_empty(), "{result:?}");
            assert_eq!(detected(), 1);

            // A lazily repaired legacy store must bootstrap its numbered
            // migration without losing a previously released child.
            conn.execute_batch(
                "DROP TABLE epic_split_releases; DELETE FROM cas_migrations WHERE id=286",
            )
            .unwrap();
            cas_store::mark_epic_split_child_released(
                &root,
                "split-old",
                "cas-child",
                chrono::Utc::now(),
            )
            .unwrap();
            assert_eq!(detected(), 1);
            let result = crate::migration::run_migrations(&root, false).unwrap();
            assert!(result.errors.is_empty(), "{result:?}");
            assert!(
                crate::migration::check_migrations(&root)
                    .unwrap()
                    .applied
                    .iter()
                    .any(|row| row.id == 286)
            );
            assert_eq!(
                cas_store::list_epic_split_child_releases(&root).unwrap(),
                vec![("split-old".into(), "cas-child".into())]
            );
            assert_eq!(
                crate::migration::run_migrations(&root, false)
                    .unwrap()
                    .applied_count,
                0
            );
        });
    }
}
