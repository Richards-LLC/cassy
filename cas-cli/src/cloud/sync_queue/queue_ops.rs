use chrono::{DateTime, Utc};
use rusqlite::params;

use crate::cloud::sync_queue::{EntityType, QueuedSync, SyncOperation, SyncQueue};
use crate::error::CasError;

impl SyncQueue {
    /// Drop a queued task/entry tombstone when its target still exists locally.
    ///
    /// Pull/apply paths intentionally write through non-syncing stores, so a
    /// remote restore can recreate a row without replacing an older queued
    /// delete. Checking the co-located source-of-truth table before any HTTP
    /// request prevents that stale tombstone from deleting the live cloud row.
    /// The check and queue removal share one SQLite transaction so the exact
    /// queue item is neutralized atomically.
    pub(crate) fn neutralize_delete_if_local_entity_exists(
        &self,
        item: &QueuedSync,
    ) -> Result<bool, CasError> {
        let table = match item.entity_type {
            EntityType::Entry => "entries",
            EntityType::Task => "tasks",
            _ => return Ok(false),
        };

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let exists: bool = tx.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id = ?1)"),
            params![item.entity_id],
            |row| row.get(0),
        )?;
        if exists {
            tx.execute(
                "DELETE FROM sync_queue WHERE id = ?1 AND operation = 'delete'",
                params![item.id],
            )?;
        }
        tx.commit()?;
        Ok(exists)
    }

    /// Queue a sync operation.
    ///
    /// Uses upsert semantics - if an item with the same entity_type,
    /// entity_id, and team_id exists, it is replaced with the new operation.
    pub fn enqueue(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        operation: SyncOperation,
        payload: Option<&str>,
    ) -> Result<(), CasError> {
        self.enqueue_with_team(entity_type, entity_id, operation, payload, "")
    }

    /// Queue a sync operation for a specific team.
    pub fn enqueue_for_team(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        operation: SyncOperation,
        payload: Option<&str>,
        team_id: &str,
    ) -> Result<(), CasError> {
        self.enqueue_with_team(entity_type, entity_id, operation, payload, team_id)
    }

    fn enqueue_with_team(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        operation: SyncOperation,
        payload: Option<&str>,
        team_id: &str,
    ) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();

        conn.execute(
            r#"
            INSERT INTO sync_queue (entity_type, entity_id, operation, payload, team_id, created_at, retry_count)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)
            ON CONFLICT(entity_type, entity_id, team_id) DO UPDATE SET
                operation = excluded.operation,
                payload = excluded.payload,
                created_at = excluded.created_at,
                retry_count = 0,
                last_error = NULL
            "#,
            params![
                entity_type.as_str(),
                entity_id,
                operation.as_str(),
                payload,
                team_id,
                now
            ],
        )?;

        Ok(())
    }

    /// Get pending items for sync (personal items only, team_id = '').
    pub fn pending(&self, limit: usize, max_retries: i32) -> Result<Vec<QueuedSync>, CasError> {
        self.pending_for_entity_type(None, limit, max_retries)
    }

    /// Get pending personal items for one entity type.
    ///
    /// The entity predicate is applied before `LIMIT`, so a scoped push cannot
    /// be starved by older rows of another type at the head of the queue.
    /// `None` preserves the normal all-entity FIFO ordering. Knowledge pages
    /// are excluded from this generic queue path because they use their own
    /// watermark protocol in `syncer::knowledge`.
    pub fn pending_for_entity_type(
        &self,
        entity_type: Option<EntityType>,
        limit: usize,
        max_retries: i32,
    ) -> Result<Vec<QueuedSync>, CasError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, entity_type, entity_id, operation, payload, team_id, created_at, retry_count, last_error
            FROM sync_queue
            WHERE retry_count < ?1 AND (team_id IS NULL OR team_id = '')
              AND entity_type != 'knowledge_page'
              AND (?3 IS NULL OR entity_type = ?3)
            ORDER BY created_at ASC
            LIMIT ?2
            "#,
        )?;

        let items = stmt
            .query_map(
                params![
                    max_retries,
                    limit as i64,
                    entity_type.map(|kind| kind.as_str())
                ],
                Self::map_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(items)
    }

    /// Get retained failed personal rows for operator-facing diagnostics.
    pub fn failed_for_entity_type(
        &self,
        entity_type: Option<EntityType>,
        max_retries: i32,
        limit: usize,
    ) -> Result<Vec<QueuedSync>, CasError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, entity_type, entity_id, operation, payload, team_id, created_at, retry_count, last_error
            FROM sync_queue
            WHERE retry_count >= ?1 AND (team_id IS NULL OR team_id = '')
              AND entity_type != 'knowledge_page'
              AND (?3 IS NULL OR entity_type = ?3)
            ORDER BY id DESC
            LIMIT ?2
            "#,
        )?;

        let items = stmt
            .query_map(
                params![
                    max_retries,
                    limit as i64,
                    entity_type.map(|kind| kind.as_str())
                ],
                Self::map_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(items)
    }

    /// Get pending items for a specific team.
    pub fn pending_for_team(
        &self,
        team_id: &str,
        limit: usize,
        max_retries: i32,
    ) -> Result<Vec<QueuedSync>, CasError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, entity_type, entity_id, operation, payload, team_id, created_at, retry_count, last_error
            FROM sync_queue
            WHERE retry_count < ?1 AND team_id = ?2
            ORDER BY created_at ASC
            LIMIT ?3
            "#,
        )?;

        let items = stmt
            .query_map(params![max_retries, team_id, limit as i64], Self::map_row)?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(items)
    }

    /// Drain (remove and return) all pending items for a specific team.
    pub fn drain_by_team(
        &self,
        team_id: &str,
        max_retries: i32,
    ) -> Result<Vec<QueuedSync>, CasError> {
        let items = self.pending_for_team(team_id, usize::MAX, max_retries)?;
        let conn = self.conn.lock().unwrap();

        for item in &items {
            conn.execute("DELETE FROM sync_queue WHERE id = ?1", params![item.id])?;
        }

        Ok(items)
    }

    /// List all items in the queue (for display).
    pub fn list_all(&self, limit: usize) -> Result<Vec<QueuedSync>, CasError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, entity_type, entity_id, operation, payload, team_id, created_at, retry_count, last_error
            FROM sync_queue
            ORDER BY created_at DESC
            LIMIT ?1
            "#,
        )?;

        let items = stmt
            .query_map(params![limit as i64], Self::map_row)?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(items)
    }

    pub(super) fn map_row(row: &rusqlite::Row) -> Result<QueuedSync, rusqlite::Error> {
        let entity_type_str: String = row.get(1)?;
        let operation_str: String = row.get(3)?;
        let created_str: String = row.get(6)?;
        let team_id: Option<String> = row
            .get::<_, Option<String>>(5)?
            .filter(|value| !value.is_empty());

        Ok(QueuedSync {
            id: row.get(0)?,
            entity_type: EntityType::parse(&entity_type_str).unwrap_or(EntityType::Entry),
            entity_id: row.get(2)?,
            operation: SyncOperation::parse(&operation_str).unwrap_or(SyncOperation::Upsert),
            payload: row.get(4)?,
            team_id,
            created_at: DateTime::parse_from_rfc3339(&created_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            retry_count: row.get(7)?,
            last_error: row.get(8)?,
        })
    }
}
