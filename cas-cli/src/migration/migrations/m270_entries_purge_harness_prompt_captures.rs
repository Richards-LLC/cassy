//! Migration: purge task-notification envelopes captured as memories (cas-9734).
//!
//! Claude submits its own background-task completions (`<task-notification>`)
//! through UserPromptSubmit. Until cas-9734 the prompt-capture hook stored each
//! one as a `user-prompt` context memory: 936 of the cas-src store's 2,148
//! entries, and ambient recall injected them instead of real lessons. The
//! source is fixed in `prompt_capture::is_context_memory_worthy_prompt`.
//!
//! Each purged row is first copied verbatim into `entries_purge_backup_m270`;
//! restore one with
//! `INSERT INTO entries SELECT * FROM entries_purge_backup_m270 WHERE id = ?`.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 270,
    name: "entries_purge_harness_prompt_captures",
    subsystem: Subsystem::Entries,
    description: "Back up, then delete, task-notification envelopes captured as user-prompt memories (cas-9734)",
    up: &[
        // Same columns as entries, no rows; a fresh store gets an empty backup.
        "CREATE TABLE IF NOT EXISTS entries_purge_backup_m270 AS SELECT * FROM entries WHERE 0",
        "INSERT INTO entries_purge_backup_m270 SELECT * FROM entries
         WHERE content LIKE 'User request: <task-notification>%'
           AND tags LIKE '%\"user-prompt\"%'
           AND id NOT IN (SELECT id FROM entries_purge_backup_m270)",
        "DELETE FROM entries WHERE id IN (SELECT id FROM entries_purge_backup_m270)",
    ],
    detect: Some(
        "SELECT EXISTS (
             SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'entries_purge_backup_m270'
         )",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};

    fn apply(conn: &Connection) {
        for statement in super::MIGRATION.up {
            conn.execute_batch(statement).unwrap();
        }
    }

    fn ids(conn: &Connection, table: &str) -> Vec<String> {
        let mut statement = conn
            .prepare(&format!("SELECT id FROM {table} ORDER BY id"))
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// Captured notifications move to the backup; real memories, including a
    /// genuine user request and a lesson quoting the envelope, survive.
    #[test]
    fn purges_task_notification_captures_into_a_restorable_backup() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE entries (id TEXT PRIMARY KEY, tags TEXT, created TEXT NOT NULL, content TEXT NOT NULL)",
        )
        .unwrap();
        let prompt = r#"["user-prompt"]"#;
        for (id, tags, content) in [
            ("junk-1", prompt, "User request: <task-notification>\n<task-id>b3fh45va2</task-id>\n<status>completed</status>\n</task-notification>"),
            ("junk-2", prompt, "User request: <task-notification>\n<task-id>btfyt710h</task-id>\n</task-notification>"),
            ("user", prompt, "User request: Please make lessons stick across releases"),
            ("lesson", r#"["release","operation:release-cut"]"#, "User request: <task-notification> blobs crowd recall; purge them"),
            ("scratch", r#"["release"]"#, "Scratch base must live on the checkout's filesystem"),
        ] {
            conn.execute(
                "INSERT INTO entries (id, tags, created, content) VALUES (?1, ?2, '2026-10-14T17:23:32Z', ?3)",
                params![id, tags, content],
            )
            .unwrap();
        }
        assert!(!crate::migration::migration_is_detected(&conn, &super::MIGRATION));
        apply(&conn);
        apply(&conn);
        assert!(crate::migration::migration_is_detected(&conn, &super::MIGRATION));
        assert_eq!(ids(&conn, "entries"), ["lesson", "scratch", "user"]);
        assert_eq!(ids(&conn, "entries_purge_backup_m270"), ["junk-1", "junk-2"]);

        conn.execute(
            "INSERT INTO entries SELECT * FROM entries_purge_backup_m270 WHERE id = 'junk-1'",
            [],
        )
        .unwrap();
        let restored: String = conn
            .query_row("SELECT content FROM entries WHERE id = 'junk-1'", [], |row| row.get(0))
            .unwrap();
        assert!(restored.contains("<task-id>b3fh45va2</task-id>"));
    }
}
