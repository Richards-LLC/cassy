//! Migration: why an `inbox_poll` settled a recipient receipt (cas-6ccb,
//! GH #1215).
//!
//! An `inbox_poll` claims rows before its response is read, so a direct row
//! polled but never acknowledged is shown once more at the recipient's next
//! turn. Rows the poll withheld as already done, did not replay, or showed as
//! a redelivery are settled instead: this column records why, without an ack
//! or a stage change, so the repeat skips them and `message_status` can say so.
//!
//! `SqlitePromptQueueStore::init` also installs the column with
//! `ensure_column`, as for `source` (m220) and `teams_body_fallback_at`
//! (m272); m208's shape test chains this migration.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 273,
    name: "prompt_queue_recipient_seen_poll_settled",
    subsystem: Subsystem::Agents,
    description: "Record why an inbox_poll settled a recipient receipt (cas-6ccb)",
    up: &["ALTER TABLE prompt_queue_recipient_seen ADD COLUMN poll_settled TEXT"],
    detect: Some(
        "SELECT CASE WHEN EXISTS (
            SELECT 1 FROM pragma_table_info('prompt_queue_recipient_seen')
             WHERE name = 'poll_settled'
         ) THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_adds_and_detects_the_poll_settled_column() {
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
