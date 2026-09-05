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
    last_error TEXT,
    last_outcome TEXT,
    last_reason TEXT,
    failed_client_version TEXT
);

CREATE INDEX IF NOT EXISTS idx_sync_queue_created ON sync_queue(created_at);
CREATE INDEX IF NOT EXISTS idx_sync_queue_retry ON sync_queue(retry_count);

-- A task mutation stages one row here before touching the canonical task.
-- Successful outbox insertion deletes it in the same transaction as the
-- sync_queue rows; a crash or enqueue failure leaves restart-visible repair
-- evidence instead of silently losing the mutation's sync intent.
CREATE TABLE IF NOT EXISTS task_sync_intents (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    previous_updated_at TEXT,
    team_id TEXT,
    previous_team_id TEXT,
    previous_project_id TEXT,
    global_scope INTEGER NOT NULL DEFAULT 0 CHECK (global_scope IN (0, 1)),
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_task_sync_intents_entity
    ON task_sync_intents(entity_id);

CREATE TABLE IF NOT EXISTS task_sync_routes (
    entity_id TEXT PRIMARY KEY,
    team_id TEXT,
    project_id TEXT,
    updated_at TEXT NOT NULL
);

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
    resolved_at TEXT NOT NULL,
    -- Nullable: a conflict settled on the timestamp path (either side lacking
    -- a server revision) legitimately has no revisions to record.
    local_revision INTEGER,
    remote_revision INTEGER
);

CREATE INDEX IF NOT EXISTS idx_sync_conflicts_resolved_at ON sync_conflicts(resolved_at DESC);
"#;

impl SyncQueue {
    /// Add the conflict-journal revision columns to an existing database.
    ///
    /// `CREATE TABLE IF NOT EXISTS` cannot widen a table that already exists,
    /// so a store created before cas-c32f would otherwise fail every conflict
    /// insert until migration 252 happened to run. The queue owns these writes,
    /// so it repairs its own schema rather than depending on migration order.
    pub(super) fn migrate_conflict_revisions(&self, conn: &Connection) -> Result<(), CasError> {
        for column in ["local_revision", "remote_revision"] {
            let present: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM pragma_table_info('sync_conflicts') WHERE name = ?1",
                    [column],
                    |row| row.get(0),
                )
                .unwrap_or(false);
            if !present {
                conn.execute_batch(&format!(
                    "ALTER TABLE sync_conflicts ADD COLUMN {column} INTEGER;"
                ))?;
            }
        }
        Ok(())
    }

    /// Add the per-row cloud verdict columns to existing sync_queue tables.
    ///
    /// `last_error` alone cannot separate a benign last-writer-wins skip from a
    /// refused write, and it cannot say which client build parked a row. The
    /// structured columns keep the cloud's own verdict (`last_outcome`,
    /// `last_reason`) and the build that recorded it so `cas update`/`cas
    /// doctor` can name rejections by reason and so a client upgrade can
    /// requeue rows that only an older build treated as terminal.
    pub(super) fn migrate_row_outcomes(&self, conn: &Connection) -> Result<(), CasError> {
        for column in ["last_outcome", "last_reason", "failed_client_version"] {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM pragma_table_info('sync_queue') WHERE name = ?1",
                    [column],
                    |row| row.get(0),
                )
                .unwrap_or(false);
            if !exists {
                conn.execute_batch(&format!("ALTER TABLE sync_queue ADD COLUMN {column} TEXT;"))?;
            }
        }
        Ok(())
    }

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
