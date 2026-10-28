//! One durable account-switch decision per harness limit episode.
use crate::migration::{Migration, Subsystem};
pub const MIGRATION: Migration = Migration {
    id: 287,
    name: "usage_limit_episodes",
    subsystem: Subsystem::Agents,
    description: "Persist limit-switch and no-eligible decisions beyond spawn cleanup",
    up: &[cas_store::USAGE_LIMIT_EPISODE_SCHEMA],
    detect: Some(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='usage_limit_episodes'",
    ),
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_database_upgrade_is_forward_only_and_idempotent() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let project = home.join("project");
            std::fs::create_dir_all(&project).unwrap();
            let root = crate::store::init_cas_dir(&project).unwrap();
            let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
            conn.execute_batch("DROP TABLE IF EXISTS usage_limit_episodes; DELETE FROM cas_migrations WHERE id=287").unwrap();
            // An already-populated pre-287 queue must remain byte-for-byte intact.
            conn.execute("INSERT INTO spawn_queue(action, worker_names, worker_spec) VALUES ('recycle','manual-worker','{}')", []).unwrap();
            let result = crate::migration::run_migrations(&root, false).unwrap();
            assert!(result.errors.is_empty(), "{result:?}");
            assert!(
                result
                    .applied_names
                    .iter()
                    .any(|name| name.contains("usage_limit_episodes"))
            );
            assert_eq!(
                conn.query_row::<String, _, _>(
                    "SELECT worker_spec FROM spawn_queue WHERE worker_names='manual-worker'",
                    [],
                    |r| r.get(0)
                )
                .unwrap(),
                "{}"
            );
            conn.execute("INSERT INTO usage_limit_episodes(episode,detail) VALUES ('old-refusal','no eligible account until reset')", []).unwrap();
            let result = crate::migration::run_migrations(&root, false).unwrap();
            assert!(result.errors.is_empty(), "{result:?}");
            assert_eq!(result.applied_count, 0);
            assert_eq!(
                conn.query_row::<String, _, _>(
                    "SELECT detail FROM usage_limit_episodes WHERE episode='old-refusal'",
                    [],
                    |r| r.get(0)
                )
                .unwrap(),
                "no eligible account until reset"
            );
        });
    }
}
