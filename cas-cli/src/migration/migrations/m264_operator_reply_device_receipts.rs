//! Application storage receipts, independent of m263's cloud outbox.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 264,
    name: "operator_reply_device_receipts",
    subsystem: Subsystem::Tasks,
    description: "Record per-device Commander reply persistence without marking operator read",
    up: cas_store::OPERATOR_REPLY_RECEIPTS_SCHEMA_STATEMENTS,
    detect: Some(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='operator_reply_device_receipts') AND EXISTS (SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_operator_reply_receipt_device')",
    ),
};

#[cfg(test)]
mod tests {
    use cas_store::{
        PromptQueueStore, SqlitePromptQueueStore, SqliteStore, SqliteTaskStore, Store, TaskStore,
    };
    use rusqlite::Connection;
    use std::path::Path;
    use tempfile::TempDir;

    fn initialize_core_stores(path: &Path) {
        SqliteStore::open(path).unwrap().init().unwrap();
        SqliteTaskStore::open(path).unwrap().init().unwrap();
    }

    fn assert_receipt_migration(conn: &Connection) {
        assert!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, bool>(0))
                .unwrap()
        );
        assert!(
            conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM cas_migrations WHERE id=?1 AND name=?2)",
                rusqlite::params![super::MIGRATION.id, super::MIGRATION.name],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
        );
    }

    fn receipt_time(conn: &Connection, id: i64) -> String {
        conn.query_row("SELECT persisted_at FROM operator_reply_device_receipts WHERE prompt_id=?1 AND factory_session='session-a' AND device_id='phone'", [id], |row| row.get(0)).unwrap()
    }

    #[test]
    fn real_migrations_before_lazy_prompt_store_init_are_idempotent() {
        let root = TempDir::new().unwrap();
        initialize_core_stores(root.path());
        let conn = Connection::open(root.path().join("cas.db")).unwrap();
        let has_prompts = || {
            conn.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name='prompt_queue')", [], |row| row.get::<_, bool>(0)).unwrap()
        };
        assert!(!has_prompts());

        crate::migration::run_migrations(root.path(), false).unwrap();
        assert_receipt_migration(&conn);
        assert!(
            !has_prompts(),
            "receipt migration must leave lazy prompt-store ownership intact"
        );

        let prompts = SqlitePromptQueueStore::open(root.path()).unwrap();
        prompts.init().unwrap();
        let id = prompts
            .enqueue_with_session(
                "supervisor",
                "operator",
                "reply after migration",
                "session-a",
            )
            .unwrap();
        prompts
            .record_operator_reply_persisted(id, "session-a", "phone")
            .unwrap();
        let before = receipt_time(&conn, id);
        let repeated = crate::migration::run_migrations(root.path(), false).unwrap();
        assert_eq!(repeated.applied_count, 0);
        assert_eq!(receipt_time(&conn, id), before);
    }

    #[test]
    fn real_migrations_preserve_legacy_prompt_rows() {
        let root = TempDir::new().unwrap();
        initialize_core_stores(root.path());
        let prompts = SqlitePromptQueueStore::open(root.path()).unwrap();
        prompts.init().unwrap();
        let id = prompts
            .enqueue_with_session("supervisor", "operator", "legacy reply", "session-a")
            .unwrap();
        let before = prompts.queued_prompt(id).unwrap().unwrap();
        let conn = Connection::open(root.path().join("cas.db")).unwrap();
        // A pre-m264 prompt store has real queued turns, but no device receipts.
        conn.execute("DROP TABLE operator_reply_device_receipts", [])
            .unwrap();

        crate::migration::run_migrations(root.path(), false).unwrap();
        assert_receipt_migration(&conn);
        let after = prompts.queued_prompt(id).unwrap().unwrap();
        assert_eq!(after.prompt, before.prompt);
        assert_eq!(after.processed_at, before.processed_at);
        prompts
            .record_operator_reply_persisted(id, "session-a", "phone")
            .unwrap();
        assert!(
            prompts
                .queued_prompt(id)
                .unwrap()
                .unwrap()
                .acked_at
                .is_none()
        );
        assert!(!receipt_time(&conn, id).is_empty());
        assert_eq!(
            crate::migration::run_migrations(root.path(), false)
                .unwrap()
                .applied_count,
            0
        );
    }

    #[test]
    fn real_migrations_detect_store_init_and_repair_a_recorded_missing_index() {
        let root = TempDir::new().unwrap();
        initialize_core_stores(root.path());
        let prompts = SqlitePromptQueueStore::open(root.path()).unwrap();
        prompts.init().unwrap();
        let id = prompts
            .enqueue_with_session("supervisor", "operator", "already stored", "session-a")
            .unwrap();
        prompts
            .record_operator_reply_persisted(id, "session-a", "phone")
            .unwrap();
        let conn = Connection::open(root.path().join("cas.db")).unwrap();
        let before = receipt_time(&conn, id);
        crate::migration::run_migrations(root.path(), false).unwrap();
        assert_receipt_migration(&conn);
        assert_eq!(receipt_time(&conn, id), before);

        conn.execute("DROP INDEX idx_operator_reply_receipt_device", [])
            .unwrap();
        assert!(
            !conn
                .query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                    .get::<_, bool>(0))
                .unwrap()
        );
        crate::migration::run_migrations(root.path(), false).unwrap();
        assert_receipt_migration(&conn);
        prompts
            .record_operator_reply_persisted(id, "session-a", "phone")
            .unwrap();
        assert_eq!(
            receipt_time(&conn, id),
            before,
            "repair must retain the original application receipt"
        );
        assert_eq!(
            crate::migration::run_migrations(root.path(), false)
                .unwrap()
                .applied_count,
            0
        );
    }

    #[test]
    fn receipt_migration_is_idempotent_without_cloud_outbox_tables() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE prompt_queue(id INTEGER PRIMARY KEY);")
            .unwrap();
        for _ in 0..2 {
            for sql in super::MIGRATION.up {
                conn.execute(sql, []).unwrap();
            }
            assert_eq!(
                conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
    }
}
