//! Migration: per-recipient Claude Teams body-fallback marker on
//! `prompt_queue_recipient_seen` (cas-1f0c).
//!
//! A Claude Teams inbox carries a pointer to a queued message; the CAS hooks
//! show the body. When no hook or `inbox_poll` has shown it within minutes,
//! the daemon writes the full body to that recipient's Teams inbox once. This
//! column records that claim per recipient (one per broadcast peer), so the
//! fallback is one-shot across daemon restarts and no hook renders the row
//! for that recipient again.
//!
//! `SqlitePromptQueueStore::init` also installs the column with
//! `ensure_column`, as it does for `source` (m220), so a store opened before
//! the ledger runs has it; this entry records it in the ledger and keeps the
//! baseline and migrated shapes equal (m208's shape test).

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 272,
    name: "prompt_queue_recipient_seen_teams_body_fallback",
    subsystem: Subsystem::Agents,
    description: "Record the one-shot Claude Teams body fallback per recipient (cas-1f0c)",
    up: &["ALTER TABLE prompt_queue_recipient_seen ADD COLUMN teams_body_fallback_at TEXT"],
    detect: Some(
        "SELECT CASE WHEN EXISTS (
            SELECT 1 FROM pragma_table_info('prompt_queue_recipient_seen')
             WHERE name = 'teams_body_fallback_at'
         ) THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_adds_and_detects_the_teams_body_fallback_column() {
        let conn = Connection::open_in_memory().unwrap();
        for sql in super::super::m208_prompt_queue_recipient_seen_create_table::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }
        let detected = |conn: &Connection| -> i64 {
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
                .unwrap()
        };
        assert_eq!(detected(&conn), 0);
        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }
        assert_eq!(detected(&conn), 1);
    }
}
