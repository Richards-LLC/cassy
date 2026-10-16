//! Migration: when the daemon's transport handed a recipient its copy
//! (cas-786a6, factory loop v2 C4a review M2).
//!
//! The daemon takes a `transport_claimed` receipt before it injects a row,
//! and since C4a the claim and the inject are separate passes. A daemon that
//! dies between them leaves a claim nobody handed off. This column marks the
//! claims that were handed off, so a restarted daemon releases only the
//! others, and a broadcast member counts as delivered only when it was.
//!
//! `SqlitePromptQueueStore::init` also installs the column with
//! `ensure_column`, as for `teams_body_fallback_at` (m272) and
//! `poll_settled` (m273); m208's shape test chains this migration.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 274,
    name: "prompt_queue_recipient_seen_delivered_at",
    subsystem: Subsystem::Agents,
    description: "Record when the daemon's transport handed a recipient its copy (cas-786a6)",
    up: &["ALTER TABLE prompt_queue_recipient_seen ADD COLUMN delivered_at TEXT"],
    detect: Some(
        "SELECT CASE WHEN EXISTS (
            SELECT 1 FROM pragma_table_info('prompt_queue_recipient_seen')
             WHERE name = 'delivered_at'
         ) THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_adds_and_detects_the_delivered_at_column() {
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
