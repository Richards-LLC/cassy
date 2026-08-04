//! Migration: Add `task_id` column to `spawn_queue`.
//!
//! Lets `spawn_workers` pre-assign a task to the spawned worker at
//! spawn-request-consumption time, eliminating the "first-claim stall" race
//! where a spawn-time assignment message could sit in `prompt_queue` before
//! the worker was registered (cas-6913). The nullable column preserves
//! legacy rows: `NULL` means "no pre-assignment requested" (the pre-cas-6913
//! behavior).

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 206,
    name: "spawn_queue_add_task_id",
    subsystem: Subsystem::Agents,
    description: "Add task_id TEXT column to spawn_queue for spawn-time task pre-assignment (cas-6913)",
    up: &["ALTER TABLE spawn_queue ADD COLUMN task_id TEXT"],
    detect: Some(
        "SELECT CASE WHEN EXISTS (SELECT 1 FROM pragma_table_info('spawn_queue') WHERE name = 'task_id') THEN 1 ELSE 0 END",
    ),
};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    fn spawn_queue_columns(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('spawn_queue') ORDER BY cid")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn migration_adds_task_id_column() {
        let conn = Connection::open_in_memory().unwrap();

        conn.execute_batch(
            "CREATE TABLE spawn_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                action TEXT NOT NULL,
                count INTEGER,
                worker_names TEXT,
                force INTEGER NOT NULL DEFAULT 0,
                isolate INTEGER NOT NULL DEFAULT 0,
                worker_spec TEXT,
                factory_session TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                processed_at TEXT
            );",
        )
        .unwrap();

        let result: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(result, 0, "detect should return 0 on pre-migration schema");

        for sql in super::MIGRATION.up {
            conn.execute(sql, []).unwrap();
        }

        let cols = spawn_queue_columns(&conn);
        assert!(
            cols.contains(&"task_id".to_string()),
            "task_id column should exist after migration"
        );

        let result: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(result, 1, "detect should return 1 after migration");
    }

    #[test]
    fn idempotent_detect_on_fresh_schema() {
        let conn = Connection::open_in_memory().unwrap();

        conn.execute_batch(
            "CREATE TABLE spawn_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                action TEXT NOT NULL,
                count INTEGER,
                worker_names TEXT,
                force INTEGER NOT NULL DEFAULT 0,
                isolate INTEGER NOT NULL DEFAULT 0,
                worker_spec TEXT,
                factory_session TEXT,
                task_id TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                processed_at TEXT
            );",
        )
        .unwrap();

        let result: i64 = conn
            .query_row(super::MIGRATION.detect.unwrap(), [], |row| row.get(0))
            .unwrap();
        assert_eq!(result, 1, "detect must return 1 when column already exists");
    }

    /// Class-guard (m205 pattern, per the 4efed95 hotfix scar): the baseline
    /// `SPAWN_QUEUE_SCHEMA` (applied on every fresh DB, before migrations
    /// run) and a pre-m206 table with this migration's `up` SQL applied
    /// (the existing-DB upgrade path) must produce IDENTICAL column sets.
    /// If `task_id` only existed on one of the two paths, every test that
    /// only ever spins up a fresh DB would be blind to it — the exact
    /// fresh-vs-existing split that caused the baseline-schema hotfix.
    #[test]
    fn baseline_schema_and_post_migration_schema_produce_identical_spawn_queue_shape() {
        let fresh = Connection::open_in_memory().unwrap();
        fresh.execute_batch(cas_store::SPAWN_QUEUE_SCHEMA).unwrap();
        let fresh_cols = spawn_queue_columns(&fresh);
        assert!(
            fresh_cols.contains(&"task_id".to_string()),
            "baseline SPAWN_QUEUE_SCHEMA must include task_id (fresh-DB path): {fresh_cols:?}"
        );

        let upgraded = Connection::open_in_memory().unwrap();
        upgraded
            .execute_batch(
                "CREATE TABLE spawn_queue (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    action TEXT NOT NULL,
                    count INTEGER,
                    worker_names TEXT,
                    force INTEGER NOT NULL DEFAULT 0,
                    isolate INTEGER NOT NULL DEFAULT 0,
                    worker_spec TEXT,
                    factory_session TEXT,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    processed_at TEXT
                );",
            )
            .unwrap();
        for sql in super::MIGRATION.up {
            upgraded.execute(sql, []).unwrap();
        }
        // Newer spawn-queue migrations must be applied on the upgraded-db
        // path too: the fresh baseline already includes their columns.
        for sql in
            crate::migration::migrations::m216_spawn_queue_add_requester_config_dir::MIGRATION.up
        {
            upgraded.execute(sql, []).unwrap();
        }
        for sql in crate::migration::migrations::m217_spawn_queue_add_lifecycle_state::MIGRATION.up
        {
            upgraded.execute(sql, []).unwrap();
        }
        let upgraded_cols = spawn_queue_columns(&upgraded);
        assert!(
            upgraded_cols.contains(&"task_id".to_string()),
            "post-migration schema (existing-DB path) must include task_id: {upgraded_cols:?}"
        );
        assert!(
            upgraded_cols.contains(&"requester_config_dir".to_string()),
            "post-migration schema (existing-DB path) must include requester_config_dir: {upgraded_cols:?}"
        );

        // Column ORDER legitimately differs — ALTER TABLE ADD COLUMN always
        // appends at the end, while the baseline declares task_id inline
        // between factory_session and created_at. That's harmless: every
        // query in this store selects columns by explicit name (see
        // `poll`/`peek`'s SELECT lists), never `SELECT *`, so physical
        // column position is never load-bearing. What actually matters —
        // and what the 4efed95 class of bug breaks — is that the SET of
        // column names is identical on both paths.
        let mut fresh_sorted = fresh_cols.clone();
        fresh_sorted.sort();
        let mut upgraded_sorted = upgraded_cols.clone();
        upgraded_sorted.sort();
        assert_eq!(
            fresh_sorted, upgraded_sorted,
            "baseline (fresh-DB, {fresh_cols:?}) and post-migration (upgraded-DB, {upgraded_cols:?}) \
             spawn_queue shapes must have the identical set of columns — a mismatch here is \
             invisible to any test that only creates fresh DBs (see hotfix 4efed95)"
        );
    }
}
