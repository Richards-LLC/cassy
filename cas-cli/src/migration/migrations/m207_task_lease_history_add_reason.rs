//! Migration: Add `reason` column to `task_lease_history`.
//!
//! Release reasons were historically stored in `previous_agent_id`, even
//! though that column is reserved for transfer attribution. The nullable
//! column keeps the migration backfill-free; the store reader provides the
//! compatibility fallback for legacy released rows.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 207,
    name: "task_lease_history_add_reason",
    subsystem: Subsystem::Agents,
    description: "Add reason TEXT column to task_lease_history for human-readable lease event reasons (cas-7aef)",
    up: &["ALTER TABLE task_lease_history ADD COLUMN reason TEXT"],
    detect: Some(
        "SELECT CASE WHEN EXISTS (SELECT 1 FROM pragma_table_info('task_lease_history') WHERE name = 'reason') THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    fn lease_history_columns(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('task_lease_history') ORDER BY cid")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn create_legacy_table(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE task_lease_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                epoch INTEGER NOT NULL DEFAULT 1,
                timestamp TEXT NOT NULL,
                details TEXT,
                previous_agent_id TEXT
            );",
        )
        .unwrap();
    }

    #[test]
    fn migration_adds_reason_column_without_backfill() {
        let conn = Connection::open_in_memory().unwrap();
        create_legacy_table(&conn);
        conn.execute(
            "INSERT INTO task_lease_history
             (task_id, agent_id, event_type, timestamp, previous_agent_id)
             VALUES ('cas-old', 'agent-old', 'released', '2026-07-22T00:00:00Z', 'Task closed')",
            [],
        )
        .unwrap();

        let result: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(result, 0, "detect should return 0 on pre-migration schema");

        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }

        let cols = lease_history_columns(&conn);
        assert!(cols.contains(&"reason".to_string()));
        let legacy_reason: Option<String> = conn
            .query_row(
                "SELECT reason FROM task_lease_history WHERE task_id = 'cas-old'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            legacy_reason, None,
            "migration must remain backfill-free; compatibility belongs in the reader"
        );
    }

    #[test]
    fn baseline_schema_and_post_migration_schema_produce_identical_lease_history_shape() {
        let fresh = Connection::open_in_memory().unwrap();
        fresh.execute_batch(cas_store::AGENT_SCHEMA).unwrap();
        let fresh_cols = lease_history_columns(&fresh);

        let upgraded = Connection::open_in_memory().unwrap();
        create_legacy_table(&upgraded);
        for sql in super::MIGRATION.up {
            upgraded.execute(sql, []).unwrap();
        }
        let upgraded_cols = lease_history_columns(&upgraded);

        assert_eq!(fresh_cols, upgraded_cols);
        assert!(fresh_cols.contains(&"reason".to_string()));
    }
}
