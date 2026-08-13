//! Migration: durable operator suppressions for replayed external handoffs.

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 235,
    name: "external_task_dependency_suppressions",
    subsystem: Subsystem::Tasks,
    description: "Preserve operator removal of replayed cloud external dependencies",
    up: &[
        "ALTER TABLE external_task_dependencies ADD COLUMN suppressed_at TEXT",
        "CREATE TABLE IF NOT EXISTS task_proposal_request_keys (request_fingerprint TEXT PRIMARY KEY, client_request_id TEXT NOT NULL, created_at TEXT NOT NULL)",
    ],
    detect: Some(
        "SELECT CASE WHEN EXISTS (SELECT 1 FROM pragma_table_info('external_task_dependencies') WHERE name = 'suppressed_at') AND EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'task_proposal_request_keys') THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    fn table_shape(
        conn: &Connection,
        table: &str,
    ) -> Vec<(String, String, i64, Option<String>, i64)> {
        let mut statement = conn
            .prepare(&format!("PRAGMA table_info('{table}')"))
            .unwrap();
        let mut shape = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        shape.sort_by(|left, right| left.0.cmp(&right.0));
        shape
    }

    #[test]
    fn migration_adds_durable_suppression_marker() {
        let conn = Connection::open_in_memory().unwrap();
        for statement in super::super::m234_external_task_dependencies::MIGRATION.up {
            conn.execute(statement, []).unwrap();
        }
        let before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('external_task_dependencies') WHERE name = 'suppressed_at'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(before, 0, "m234 must model the real pre-m235 schema");
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        for statement in super::MIGRATION.up {
            conn.execute(statement, []).unwrap();
        }
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn m234_then_m235_matches_fresh_task_bootstrap_shape() {
        let migrated = Connection::open_in_memory().unwrap();
        for statement in super::super::m234_external_task_dependencies::MIGRATION.up {
            migrated.execute(statement, []).unwrap();
        }
        for statement in super::MIGRATION.up {
            migrated.execute(statement, []).unwrap();
        }

        let fresh = Connection::open_in_memory().unwrap();
        fresh.execute_batch(cas_store::TASK_SCHEMA).unwrap();

        for table in [
            "external_task_dependencies",
            "external_task_dependency_sync_state",
            "task_proposal_request_keys",
        ] {
            assert_eq!(
                table_shape(&migrated, table),
                table_shape(&fresh, table),
                "migrated and fresh bootstrap schemas diverged for {table}"
            );
        }
    }

    #[test]
    fn bootstrap_suppression_column_can_converge_when_request_table_is_missing() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE external_task_dependencies (
                origin_task_id TEXT, proposal_id TEXT, suppressed_at TEXT
            )",
        )
        .unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        conn.execute(super::MIGRATION.up[1], []).unwrap();
        assert_eq!(
            conn.query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
