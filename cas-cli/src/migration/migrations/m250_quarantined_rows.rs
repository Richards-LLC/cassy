//! Migration: local quarantine ledger for unattributed cloud rows (cas-4342).
//!
//! The rows this ledger hides are already resident in every affected database,
//! so the remediation has to reach existing stores, not only fresh ones. The
//! ledger is deliberately a side table: the task row stays byte-for-byte as it
//! was, which is what makes the quarantine reversible and what keeps a re-pull
//! from resurfacing the row.

use crate::cloud::QUARANTINED_ROW_STATEMENTS;
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 250,
    name: "quarantined_rows",
    subsystem: Subsystem::Tasks,
    description: "Add the local quarantine ledger for rows whose home project cannot be established (cas-4342)",
    up: QUARANTINED_ROW_STATEMENTS,
    detect: Some(
        "SELECT EXISTS (
             SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'quarantined_rows'
         )",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_creates_the_quarantine_ledger_for_existing_task_stores() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE tasks (id TEXT PRIMARY KEY, title TEXT NOT NULL)")
            .unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );

        for statement in super::MIGRATION.up {
            conn.execute_batch(statement).unwrap();
        }

        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );

        conn.execute(
            "INSERT INTO quarantined_rows (entity_type, entity_id, reason, quarantined_at)
             VALUES ('task', 'cas-a1b2', 'unattributed cloud row', '2026-09-03T21:00:00+00:00')",
            [],
        )
        .unwrap();
        // Idempotent by primary key: a repeated quarantine cannot duplicate a
        // row or restamp when the decision was taken.
        let repeated = conn
            .execute(
                "INSERT OR IGNORE INTO quarantined_rows
                     (entity_type, entity_id, reason, quarantined_at)
                 VALUES ('task', 'cas-a1b2', 'second run', '2026-09-04T09:00:00+00:00')",
                [],
            )
            .unwrap();
        assert_eq!(repeated, 0);
        let (reason, at): (String, String) = conn
            .query_row(
                "SELECT reason, quarantined_at FROM quarantined_rows WHERE entity_id = 'cas-a1b2'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(reason, "unattributed cloud row");
        assert_eq!(at, "2026-09-03T21:00:00+00:00");
    }
}
