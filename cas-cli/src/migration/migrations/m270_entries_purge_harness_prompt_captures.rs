//! Migration: purge harness envelopes captured as `User request:` memories (cas-9734).
//!
//! Claude submits its own background-task completions (`<task-notification>`),
//! teammate relays and wakes through UserPromptSubmit. Until cas-9734 the
//! prompt-capture hook stored them as `user-prompt` context memories: 936 of
//! the cas-src store's 2,148 entries were task notifications alone, and
//! ambient recall injected them instead of real lessons. The capture source is
//! fixed in `prompt_capture::is_context_memory_worthy_prompt`; this migration
//! removes what it already wrote.
//!
//! Every purged row is first copied verbatim into `entries_purge_backup_m270`,
//! so a wrong match is recoverable with
//! `INSERT INTO entries SELECT * FROM entries_purge_backup_m270 WHERE id = ?`.
//! Only `user-prompt` captures are eligible. Two classes go:
//! - the prompt body carries a harness envelope on any line (the same
//!   prefixes `prompt_capture::is_harness_prompt` refuses), or is one of three
//!   machine-generated prompts recorded before the 2026-08 capture gate;
//! - exact duplicate captures, keeping the oldest copy.

use crate::migration::{Migration, Subsystem};

/// A capture is the 14-byte `User request: ` prefix followed by the prompt.
macro_rules! junk_capture_predicate {
    () => {
        "content LIKE 'User request: %'
         AND tags LIKE '%\"user-prompt\"%'
         AND (
            substr(content, 15, 18) = '<task-notification'
            OR substr(content, 15, 17) = '<teammate-message'
            OR substr(content, 15, 16) = '<system-reminder'
            OR substr(content, 15, 9) = 'CAS wake:'
            OR substr(content, 15, 15) = 'CAS provenance:'
            OR substr(content, 15, 6) = '[cas #'
            OR substr(content, 15, 21) = '[supervisor reminder]'
            OR substr(content, 15, 10) = '[lifecycle'
            OR substr(content, 15, 17) = '[system-reminder]'
            OR substr(content, 15, 39) = 'Another Claude session sent a message:'
            OR instr(content, char(10) || '<task-notification') > 0
            OR instr(content, char(10) || '<teammate-message') > 0
            OR instr(content, char(10) || '<system-reminder') > 0
            OR instr(content, char(10) || 'CAS wake:') > 0
            OR instr(content, char(10) || 'CAS provenance:') > 0
            OR instr(content, char(10) || '[cas #') > 0
            OR instr(content, char(10) || '[supervisor reminder]') > 0
            OR instr(content, char(10) || '[lifecycle') > 0
            OR instr(content, char(10) || '[system-reminder]') > 0
            OR instr(content, char(10) || 'Another Claude session sent a message:') > 0
            OR substr(content, 15, 33) = 'You are a documentation distiller'
            OR lower(substr(content, 15, 47)) = 'analyze this user prompt and determine if it ex'
            OR substr(content, 15, 27) = 'Generate a 5-8 word title s'
            OR EXISTS (
                SELECT 1 FROM entries older
                WHERE older.content = entries.content
                  AND older.tags LIKE '%\"user-prompt\"%'
                  AND (older.created < entries.created
                       OR (older.created = entries.created AND older.id < entries.id))
            )
         )"
    };
}

pub const MIGRATION: Migration = Migration {
    id: 270,
    name: "entries_purge_harness_prompt_captures",
    subsystem: Subsystem::Entries,
    description: "Back up, then delete, harness envelopes and duplicates captured as user-prompt memories (cas-9734)",
    up: &[
        // Same columns as entries, no rows; a fresh store gets an empty backup.
        "CREATE TABLE IF NOT EXISTS entries_purge_backup_m270 AS SELECT * FROM entries WHERE 0",
        concat!(
            "INSERT INTO entries_purge_backup_m270 SELECT * FROM entries WHERE ",
            junk_capture_predicate!(),
            " AND id NOT IN (SELECT id FROM entries_purge_backup_m270)"
        ),
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

    fn store() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE entries (
                id TEXT PRIMARY KEY,
                type TEXT NOT NULL DEFAULT 'learning',
                tags TEXT,
                created TEXT NOT NULL,
                content TEXT NOT NULL,
                archived INTEGER NOT NULL DEFAULT 0
            )",
        )
        .unwrap();
        conn
    }

    fn add(conn: &Connection, id: &str, tags: &str, created: &str, content: &str) {
        conn.execute(
            "INSERT INTO entries (id, type, tags, created, content) VALUES (?1, 'context', ?2, ?3, ?4)",
            params![id, tags, created, content],
        )
        .unwrap();
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

    fn apply(conn: &Connection) {
        for statement in super::MIGRATION.up {
            conn.execute_batch(statement).unwrap();
        }
    }

    #[test]
    fn purges_harness_captures_and_duplicates_into_a_restorable_backup() {
        let conn = store();
        let prompt = r#"["user-prompt"]"#;
        add(&conn, "n1", prompt, "2026-10-14T17:23:32Z",
            "User request: <task-notification>\n<task-id>b3fh45va2</task-id>\n<status>completed</status>\n</task-notification>");
        add(&conn, "n2", prompt, "2026-10-11T06:03:54Z",
            "User request: CAS wake: message 4184645 from director is in your inbox");
        add(&conn, "n3", prompt, "2026-10-10T01:00:00Z",
            "User request: please look\n<teammate-message teammate_id=\"director\">fix it</teammate-message>");
        add(&conn, "n4", prompt, "2026-08-08T04:17:08Z",
            "User request: Analyze this user prompt and determine if it expresses a coding preference");
        add(&conn, "d-old", prompt, "2026-09-01T00:00:00Z",
            "User request: Please implement the release report recurrence row");
        add(&conn, "d-new", prompt, "2026-09-02T00:00:00Z",
            "User request: Please implement the release report recurrence row");
        add(&conn, "keep-user", prompt, "2026-09-03T00:00:00Z",
            "User request: Please make lessons stick across releases");
        // Not a capture: a curated learning that quotes the envelope is kept.
        add(&conn, "keep-learning", r#"["release"]"#, "2026-09-04T00:00:00Z",
            "User request: <task-notification> blobs crowd recall; purge them");
        add(&conn, "keep-lesson", r#"["release","op:release-cut"]"#, "2026-10-10T00:00:00Z",
            "Scratch base must live on the checkout's filesystem");

        let detected = |conn: &Connection| {
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get::<_, i64>(0))
                .unwrap()
        };
        assert_eq!(detected(&conn), 0);
        apply(&conn);
        assert_eq!(detected(&conn), 1);

        assert_eq!(
            ids(&conn, "entries"),
            ["d-old", "keep-learning", "keep-lesson", "keep-user"]
        );
        assert_eq!(
            ids(&conn, "entries_purge_backup_m270"),
            ["d-new", "n1", "n2", "n3", "n4"]
        );

        // Idempotent: a second run neither re-copies nor deletes more.
        apply(&conn);
        assert_eq!(ids(&conn, "entries").len(), 4);
        assert_eq!(ids(&conn, "entries_purge_backup_m270").len(), 5);

        // The backup is a verbatim copy: a wrong match restores in one statement.
        conn.execute(
            "INSERT INTO entries SELECT * FROM entries_purge_backup_m270 WHERE id = 'n1'",
            [],
        )
        .unwrap();
        let restored: String = conn
            .query_row("SELECT content FROM entries WHERE id = 'n1'", [], |row| row.get(0))
            .unwrap();
        assert!(restored.contains("<task-id>b3fh45va2</task-id>"));
    }

    #[test]
    fn fresh_store_gets_an_empty_backup_and_detects_applied() {
        let conn = store();
        apply(&conn);
        assert!(ids(&conn, "entries_purge_backup_m270").is_empty());
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
