//! Migration: supervisor_queue.prompt_delivered_at (cas-194c).
//!
//! `prompt_delivered_at` marks a durable supervisor notification whose prompt
//! was delivered (the cas-17e4 outbox). Until now only
//! `SupervisorQueueStore::init` added it, so a store migrated from scratch
//! (m153 creates the table without it) lacked the column until something
//! opened the supervisor queue. The prompt-queue retention sweep (cas-f207)
//! reads it to release outbox keys, so on such a store every sweep failed with
//! "no such column: s.prompt_delivered_at" and pruned nothing.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 267,
    name: "supervisor_queue_add_prompt_delivered_at",
    subsystem: Subsystem::Agents,
    description: "Add supervisor_queue.prompt_delivered_at so retention can release delivered outbox keys on a migrated store (cas-194c)",
    up: &[
        // m153's table, so the ALTER cannot fail on a store that skipped it.
        r#"CREATE TABLE IF NOT EXISTS supervisor_queue (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            supervisor_id TEXT NOT NULL,
            event_type TEXT NOT NULL,
            payload TEXT NOT NULL,
            priority INTEGER NOT NULL DEFAULT 2,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            processed_at TEXT
        )"#,
        "ALTER TABLE supervisor_queue ADD COLUMN prompt_delivered_at TEXT",
    ],
    detect: Some(
        "SELECT COUNT(*) FROM pragma_table_info('supervisor_queue') WHERE name = 'prompt_delivered_at'",
    ),
};

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn supervisor_queue_as_migrated(conn: &Connection) {
        for statement in super::super::m153_supervisor_queue_create_table::MIGRATION.up {
            conn.execute_batch(statement).unwrap();
        }
    }

    fn detected(conn: &Connection) -> i64 {
        conn.query_row(MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn adds_the_column_to_the_migrated_table_and_detects_it() {
        let conn = Connection::open_in_memory().unwrap();
        supervisor_queue_as_migrated(&conn);
        assert_eq!(
            detected(&conn),
            0,
            "m153 creates the table without the column"
        );
        for statement in MIGRATION.up {
            conn.execute_batch(statement).unwrap();
        }
        assert_eq!(detected(&conn), 1);
    }

    #[test]
    fn applies_on_a_store_without_the_table() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(detected(&conn), 0);
        for statement in MIGRATION.up {
            conn.execute_batch(statement).unwrap();
        }
        assert_eq!(detected(&conn), 1);
    }

    #[test]
    fn a_store_that_already_added_the_column_detects_as_applied() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = cas_store::SqliteSupervisorQueueStore::open(dir.path()).unwrap();
        cas_store::SupervisorQueueStore::init(&store).unwrap();
        let conn = Connection::open(dir.path().join("cas.db")).unwrap();
        assert_eq!(
            detected(&conn),
            1,
            "the migration must be skipped, not re-run"
        );
    }

    /// The acceptance path: a store created by `cas init` (every migration
    /// from scratch) has the column, and the prompt-queue retention sweep
    /// prunes on it.
    #[test]
    fn retention_prunes_on_a_store_migrated_from_scratch() {
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();

        let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
        assert_eq!(
            detected(&conn),
            1,
            "a migrated store has prompt_delivered_at"
        );
        drop(conn);

        let queue = cas_store::SqlitePromptQueueStore::open(&cas_dir).unwrap();
        cas_store::PromptQueueStore::init(&queue).unwrap();
        let id =
            cas_store::PromptQueueStore::enqueue(&queue, "supervisor", "worker", "old").unwrap();
        let conn = Connection::open(cas_dir.join("cas.db")).unwrap();
        let aged = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        conn.execute(
            "UPDATE prompt_queue SET processed_at = ?1 WHERE id = ?2",
            rusqlite::params![aged, id],
        )
        .unwrap();
        drop(conn);

        let sweep =
            cas_store::PromptQueueStore::prune_terminal_older_than(&queue, 7 * 24 * 60 * 60)
                .expect("retention must not fail on a migrated store");
        assert_eq!(sweep.pruned, 1);
    }
}
