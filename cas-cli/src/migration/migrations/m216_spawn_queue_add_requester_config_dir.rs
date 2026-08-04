//! Migration: preserve the requesting supervisor's Claude account directory
//! on queued worker spawns.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 216,
    name: "spawn_queue_add_requester_config_dir",
    subsystem: Subsystem::Agents,
    description: "Add requester_config_dir TEXT to spawn_queue so daemon worker spawns retain the requesting supervisor's Claude account",
    up: &["ALTER TABLE spawn_queue ADD COLUMN requester_config_dir TEXT"],
    detect: Some(
        "SELECT CASE WHEN EXISTS (SELECT 1 FROM pragma_table_info('spawn_queue') WHERE name = 'requester_config_dir') THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migration_adds_requester_config_dir_column() {
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
    }
}
