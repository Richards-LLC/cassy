//! Migration: make the spawn lifecycle queryable instead of log-only.
//!
//! GH #60: `spawn_workers` returns `Queued spawn request ... (request ID: N)`,
//! a receipt with the same shape whether a worker later registers or nothing
//! ever happens. The daemon knew the truth — it emits per-stage audit lines and
//! supervisor notices — but that state lived only in log prose and a free-text
//! inbox message, so the supervisor could not ask "what became of request N?"
//! and had to correlate by reading messages. When two spawns are in flight
//! (anonymous spawns get their worker name only at provisioning time), that
//! correlation is guesswork — the live failure mode where four requests were
//! attributed to the wrong workers.
//!
//! These columns record the terminal-so-far state of each queued request, so
//! request → worker → state → reason is a lookup rather than a reconstruction.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 217,
    name: "spawn_queue_add_lifecycle_state",
    subsystem: Subsystem::Agents,
    description: "Add spawn_state/spawn_worker/spawn_detail/spawn_state_at to spawn_queue so spawn lifecycle (queued → provisioning → launched → registered/FAILED) is queryable per request",
    up: &[
        "ALTER TABLE spawn_queue ADD COLUMN spawn_state TEXT",
        "ALTER TABLE spawn_queue ADD COLUMN spawn_worker TEXT",
        "ALTER TABLE spawn_queue ADD COLUMN spawn_detail TEXT",
        "ALTER TABLE spawn_queue ADD COLUMN spawn_state_at TEXT",
    ],
    detect: Some(
        "SELECT CASE WHEN EXISTS (SELECT 1 FROM pragma_table_info('spawn_queue') WHERE name = 'spawn_state') THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_adds_lifecycle_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE spawn_queue (id INTEGER PRIMARY KEY, action TEXT, task_id TEXT);",
        )
        .unwrap();

        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }

        let detected: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(detected, 1);

        // Every column the store writes must exist, not just the detect probe.
        for column in [
            "spawn_state",
            "spawn_worker",
            "spawn_detail",
            "spawn_state_at",
        ] {
            let present: i64 = conn
                .query_row(
                    "SELECT CASE WHEN EXISTS (SELECT 1 FROM pragma_table_info('spawn_queue') WHERE name = ?) THEN 1 ELSE 0 END",
                    [column],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(present, 1, "spawn_queue is missing column {column}");
        }
    }
}
