//! cas-c423: logical-reader admission identity and its atomic queue cursor.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 279,
    name: "violet_reader_admissions",
    subsystem: Subsystem::Agents,
    description: "Persist occurrence/reader admissions with queue and cursor state (cas-c423)",
    up: &[cas_store::VIOLET_ADMISSION_SCHEMA],
    detect: Some("SELECT CASE WHEN
        (SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN
         ('violet_admissions','violet_admission_cursors'))=2
        AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='trigger' AND name='violet_admission_identity_immutable')
        THEN 1 ELSE 0 END"),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cas_c423_m279_bootstrap_and_reconciliation_keep_queue_evidence() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let project = home.join("project");
            std::fs::create_dir_all(&project).unwrap();
            let root = crate::store::init_cas_dir(&project).unwrap();
            let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
            assert!(crate::migration::migration_is_detected(&conn, &MIGRATION));
            use cas_store::PromptQueueStore;
            let queue = cas_store::SqlitePromptQueueStore::open(&root).unwrap();
            queue.init().unwrap();
            let id = queue
                .enqueue_with_session("sender", "recipient", "keep receipt", "factory")
                .unwrap();
            conn.execute(
                "INSERT INTO prompt_queue_recipient_seen(prompt_id,recipient,seen_at,source)
                VALUES (?1,'recipient','2026-01-01','hook_surfaced')",
                [id],
            )
            .unwrap();
            conn.execute_batch("DROP TABLE violet_admission_cursors")
                .unwrap();
            assert!(!crate::migration::migration_is_detected(&conn, &MIGRATION));
            let result = crate::migration::run_migrations(&root, false).unwrap();
            assert!(result.errors.is_empty(), "{result:?}");
            assert!(crate::migration::migration_is_detected(&conn, &MIGRATION));
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
                    "SELECT COUNT(*) FROM cas_migrations WHERE id=279",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
            assert_eq!(
                conn.query_row(
                    "SELECT source FROM prompt_queue_recipient_seen WHERE prompt_id=?1",
                    [id],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                "hook_surfaced"
            );
        });
    }
}
