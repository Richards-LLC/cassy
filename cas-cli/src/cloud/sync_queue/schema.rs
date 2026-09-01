use rusqlite::Connection;

use crate::cloud::sync_queue::SyncQueue;
use crate::error::CasError;

pub(super) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS sync_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    payload TEXT,
    team_id TEXT,
    project_id TEXT,
    created_at TEXT NOT NULL,
    retry_count INTEGER NOT NULL DEFAULT 0,
    last_error TEXT
);

CREATE INDEX IF NOT EXISTS idx_sync_queue_created ON sync_queue(created_at);
CREATE INDEX IF NOT EXISTS idx_sync_queue_retry ON sync_queue(retry_count);

CREATE TABLE IF NOT EXISTS sync_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sync_conflicts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    discarded_row_json TEXT NOT NULL,
    winner_side TEXT NOT NULL,
    strategy TEXT NOT NULL,
    resolved_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sync_conflicts_resolved_at ON sync_conflicts(resolved_at DESC);
"#;

impl SyncQueue {
    /// Add team/project identity columns to existing sync_queue tables.
    pub(super) fn migrate_team_id(&self, conn: &Connection) -> Result<(), CasError> {
        let has_team_id: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('sync_queue') WHERE name = 'team_id'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        let has_project_id: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('sync_queue') WHERE name = 'project_id'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !has_team_id {
            conn.execute_batch("ALTER TABLE sync_queue ADD COLUMN team_id TEXT;")?;
        }
        if !has_project_id {
            conn.execute_batch("ALTER TABLE sync_queue ADD COLUMN project_id TEXT;")?;
        }

        // The original table had an inline UNIQUE(entity_type, entity_id,
        // team_id). SQLite cannot replace that constraint with an expression
        // index in place, so rebuild once when either identity column was
        // added. Existing rows retain NULL project_id and therefore continue
        // to coalesce under COALESCE(project_id, ''). Do not create the new
        // unique index inside this rebuild: old databases may already contain
        // duplicate rows, and cleanup below must run before index creation.
        if !has_team_id || !has_project_id {
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS sync_queue_new (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    entity_type TEXT NOT NULL,
                    entity_id TEXT NOT NULL,
                    operation TEXT NOT NULL,
                    payload TEXT,
                    team_id TEXT,
                    project_id TEXT,
                    created_at TEXT NOT NULL,
                    retry_count INTEGER NOT NULL DEFAULT 0,
                    last_error TEXT
                );
                INSERT INTO sync_queue_new (id, entity_type, entity_id, operation, payload, team_id, project_id, created_at, retry_count, last_error)
                    SELECT id, entity_type, entity_id, operation, payload, COALESCE(team_id, ''), project_id, created_at, retry_count, last_error FROM sync_queue;
                DROP TABLE sync_queue;
                ALTER TABLE sync_queue_new RENAME TO sync_queue;
                CREATE INDEX IF NOT EXISTS idx_sync_queue_created ON sync_queue(created_at);
                CREATE INDEX IF NOT EXISTS idx_sync_queue_retry ON sync_queue(retry_count);
                CREATE INDEX IF NOT EXISTS idx_sync_queue_team ON sync_queue(team_id);
                "#,
            )?;
        }

        // Normalize any pre-migration NULL team_ids to '' before creating the
        // unique index. In SQLite, NULL != '' under UNIQUE, so a row with
        // team_id=NULL and a subsequent enqueue with team_id='' would create
        // duplicates (defect C / cas-8dd8).
        conn.execute_batch("UPDATE sync_queue SET team_id = '' WHERE team_id IS NULL;")?;

        // Databases created before the identity index could contain several
        // copies of one queue key. Retain the newest row (highest AUTOINCREMENT
        // id, which is the latest enqueue) so its operation and payload are the
        // values that the next push observes. This also makes index creation
        // safe for a partially migrated database.
        conn.execute_batch(
            r#"
            DELETE FROM sync_queue
            WHERE id NOT IN (
                SELECT MAX(id)
                FROM sync_queue
                GROUP BY entity_type, entity_id, team_id, COALESCE(project_id, '')
            );
            "#,
        )?;

        conn.execute_batch(
            r#"
            CREATE INDEX IF NOT EXISTS idx_sync_queue_team ON sync_queue(team_id);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_sync_queue_entity_team_project
                ON sync_queue(entity_type, entity_id, team_id, COALESCE(project_id, ''));
            "#,
        )?;

        Ok(())
    }
}
