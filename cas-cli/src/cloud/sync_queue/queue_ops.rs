use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::cloud::sync_queue::{EntityType, QueuedSync, SyncOperation, SyncQueue};
use crate::error::CasError;

pub(super) fn upsert_queue_row(
    conn: &Connection,
    entity_type: EntityType,
    entity_id: &str,
    operation: SyncOperation,
    payload: Option<&str>,
    team_id: &str,
    project_id: Option<&str>,
) -> Result<(), CasError> {
    conn.execute(
        r#"
        INSERT INTO sync_queue
            (entity_type, entity_id, operation, payload, team_id, project_id, created_at, retry_count)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)
        ON CONFLICT DO UPDATE SET
            operation = excluded.operation,
            payload = excluded.payload,
            created_at = excluded.created_at,
            project_id = excluded.project_id,
            retry_count = 0,
            last_error = NULL
        "#,
        params![
            entity_type.as_str(),
            entity_id,
            operation.as_str(),
            payload,
            team_id,
            project_id,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub(super) fn enqueue_team_move_rows(
    conn: &Connection,
    entity_type: EntityType,
    entity_id: &str,
    old_project_id: &str,
    new_project_id: &str,
    payload: &str,
    team_id: &str,
) -> Result<(), CasError> {
    // Legacy team upserts predate project-keyed queue identities and use a
    // NULL project_id. Remove any before writing the move pair so an
    // edit-then-move sequence cannot leave a third row that would replay the
    // task under the pusher's project after the old key is deleted.
    remove_legacy_team_upsert_row(conn, entity_type, entity_id, team_id)?;
    upsert_queue_row(
        conn,
        entity_type,
        entity_id,
        SyncOperation::Delete,
        None,
        team_id,
        Some(old_project_id),
    )?;
    upsert_queue_row(
        conn,
        entity_type,
        entity_id,
        SyncOperation::Upsert,
        Some(payload),
        team_id,
        Some(new_project_id),
    )
}

pub(super) fn remove_legacy_team_upsert_row(
    conn: &Connection,
    entity_type: EntityType,
    entity_id: &str,
    team_id: &str,
) -> Result<(), CasError> {
    conn.execute(
        r#"
        DELETE FROM sync_queue
        WHERE entity_type = ?1
          AND entity_id = ?2
          AND operation = 'upsert'
          AND team_id = ?3
          AND project_id IS NULL
        "#,
        params![entity_type.as_str(), entity_id, team_id],
    )?;
    Ok(())
}

impl SyncQueue {
    /// Provenance for a queued project entity. The stored row wins over a
    /// potentially stale payload; older payloads can still use their own
    /// stamp when the row was subsequently deleted.
    pub(super) fn queued_origin(
        conn: &Connection,
        entity_type: EntityType,
        entity_id: &str,
        payload: Option<&str>,
    ) -> Option<String> {
        let table = match entity_type {
            EntityType::Entry => "entries",
            EntityType::Rule => "rules",
            EntityType::Task => "tasks",
            _ => "",
        };
        if !table.is_empty() {
            let stored = conn
                .query_row(
                    &format!("SELECT origin_project FROM {table} WHERE id = ?1"),
                    params![entity_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()
                .ok()
                .flatten()
                .flatten();
            if stored.is_some() {
                return stored;
            }
        }
        payload
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .and_then(|value| {
                value
                    .get("origin_project")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            })
    }

    pub(super) fn record_unauthored_skip(conn: &Connection) -> Result<(), CasError> {
        conn.execute(
            "INSERT INTO sync_metadata (key, value) VALUES ('unauthored_skipped', '1')
             ON CONFLICT(key) DO UPDATE SET value = CAST(value AS INTEGER) + 1",
            [],
        )?;
        Ok(())
    }

    pub fn unauthored_skipped_count(&self) -> Result<usize, CasError> {
        Ok(self
            .get_metadata("unauthored_skipped")?
            .and_then(|value| value.parse().ok())
            .unwrap_or(0))
    }

    /// Purge queued copies whose persisted provenance or payload identifies
    /// another authoring project. Both personal and team queues are covered.
    pub fn drop_queued_rows_with_foreign_origin(&self, local: &str) -> Result<usize, CasError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let rows = {
            let mut stmt = tx.prepare(
                "SELECT id, entity_type, entity_id, operation, payload, project_id FROM sync_queue
                 WHERE entity_type IN ('entry', 'rule', 'task', 'task_dependency')",
            )?;
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        let mut dropped = 0;
        for (id, kind, entity_id, operation, payload, project_id) in rows {
            let value = payload
                .as_deref()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
            if value
                .as_ref()
                .is_some_and(|v| v.get("scope").and_then(|x| x.as_str()) == Some("global"))
            {
                continue;
            }
            let Some(kind) = EntityType::parse(&kind) else {
                continue;
            };
            let origin = Self::queued_origin(&tx, kind, &entity_id, payload.as_deref());
            let foreign_target = project_id
                .as_deref()
                .is_some_and(|id| !crate::cloud::project_ids_match(id, local));
            // Deleting this project's old cloud key remains valid after a
            // local task move gives the stored row a different origin.
            let foreign_origin = operation != SyncOperation::Delete.as_str()
                && origin.as_deref().is_some_and(|origin| {
                    origin == "unknown" || !crate::cloud::project_ids_match(origin, local)
                });
            if foreign_target || foreign_origin {
                dropped += tx.execute("DELETE FROM sync_queue WHERE id = ?1", params![id])?;
            }
        }
        if dropped > 0 {
            tx.execute(
                "INSERT INTO sync_metadata (key, value) VALUES ('unauthored_skipped', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = CAST(value AS INTEGER) + ?1",
                params![dropped.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(dropped)
    }

    /// Remove legacy prompt capture upserts before either personal or team
    /// push reads the outbox. The queued payload retains its entry tags.
    pub fn drop_queued_user_prompts(&self) -> Result<usize, CasError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let ids = {
            let mut stmt = tx.prepare(
                "SELECT id, payload FROM sync_queue WHERE entity_type = 'entry' AND payload IS NOT NULL",
            )?;
            stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter_map(|(id, payload)| {
                let value = serde_json::from_str::<serde_json::Value>(&payload).ok()?;
                value
                    .get("tags")
                    .and_then(|tags| tags.as_array())
                    .is_some_and(|tags| tags.iter().any(|tag| tag.as_str() == Some("user-prompt")))
                    .then_some(id)
            })
            .collect::<Vec<_>>()
        };
        for id in &ids {
            tx.execute("DELETE FROM sync_queue WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
        Ok(ids.len())
    }

    /// Remove one stale personal outbox row when a project write is now
    /// routed exclusively to its team. Team rows and cloud data are untouched.
    pub fn drop_personal_queued_push_for(
        &self,
        entity_type: EntityType,
        entity_id: &str,
    ) -> Result<usize, CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM sync_queue WHERE entity_type = ?1 AND entity_id = ?2 AND team_id = ''",
            params![entity_type.as_str(), entity_id],
        )
        .map_err(CasError::from)
    }

    /// Drop queued personal copies of team-visible project rows after a
    /// project opts into team-only sync. This changes only the local outbox;
    /// no cloud delete is emitted. Personal memories and global rows remain.
    pub fn neutralize_team_only_personal(&self) -> Result<usize, CasError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut ids = Vec::new();
        {
            let mut stmt = tx.prepare(
                "SELECT id, entity_type, entity_id, operation, payload FROM sync_queue WHERE team_id = ''",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?;
            for row in rows {
                let (id, kind, entity_id, operation, payload) = row?;
                let value = payload
                    .as_deref()
                    .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok());
                let is_project = match kind.as_str() {
                    "entry" => value.as_ref().is_some_and(|v| {
                        let share = v.get("share").and_then(|x| x.as_str());
                        share == Some("team")
                            || (share != Some("private")
                                && v.get("scope").and_then(|x| x.as_str()) == Some("project")
                                && v.get("entry_type").and_then(|x| x.as_str())
                                    != Some("preference"))
                    }),
                    "task" | "rule" | "skill" => value
                        .as_ref()
                        .is_some_and(|v| v.get("scope").and_then(|x| x.as_str()) != Some("global")),
                    "task_dependency" => value.as_ref().is_some_and(|v| {
                        v.get("origin_project").and_then(|x| x.as_str()).is_some()
                    }),
                    _ => false,
                };
                let paired_team_delete = operation == "delete"
                    && matches!(kind.as_str(), "entry" | "task" | "rule" | "skill" | "task_dependency")
                    && tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM sync_queue WHERE entity_type = ?1 AND entity_id = ?2 AND team_id != '')",
                        params![kind, entity_id],
                        |row| row.get::<_, bool>(0),
                    )?;
                if is_project || paired_team_delete {
                    ids.push(id);
                }
            }
        }
        for id in &ids {
            tx.execute("DELETE FROM sync_queue WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
        tracing::info!(
            count = ids.len(),
            "neutralized queued personal copies of team-only project rows"
        );
        Ok(ids.len())
    }

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
    /// Uses upsert semantics - if an item with the same entity/project/team
    /// identity exists, it is replaced with the new operation.
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
        self.enqueue_for_team_project(entity_type, entity_id, operation, payload, team_id, None)
    }

    /// Enqueue a team operation targeted at a specific project identity.
    ///
    /// Ordinary writes leave `project_id` unset so the pusher's project is
    /// used. An explicit foreign target is refused by the provenance guard.
    pub fn enqueue_for_team_project(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        operation: SyncOperation,
        payload: Option<&str>,
        team_id: &str,
        project_id: Option<&str>,
    ) -> Result<(), CasError> {
        self.enqueue_with_team_project(
            entity_type,
            entity_id,
            operation,
            payload,
            team_id,
            project_id,
        )
    }

    fn enqueue_with_team(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        operation: SyncOperation,
        payload: Option<&str>,
        team_id: &str,
    ) -> Result<(), CasError> {
        self.enqueue_with_team_project(entity_type, entity_id, operation, payload, team_id, None)
    }

    fn enqueue_with_team_project(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        operation: SyncOperation,
        payload: Option<&str>,
        team_id: &str,
        project_id: Option<&str>,
    ) -> Result<(), CasError> {
        let local = crate::cloud::resolve_canonical_id(&self.cas_dir);
        let conn = self.conn.lock().unwrap();
        if let Some(local) = local.as_deref() {
            let scope_is_global = payload
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                .is_some_and(|value| value.get("scope").and_then(|v| v.as_str()) == Some("global"));
            if !scope_is_global {
                let origin = Self::queued_origin(&conn, entity_type, entity_id, payload);
                let foreign_target =
                    project_id.is_some_and(|id| !crate::cloud::project_ids_match(id, local));
                if foreign_target
                    || origin.as_deref().is_some_and(|origin| {
                        origin == "unknown" || !crate::cloud::project_ids_match(origin, local)
                    })
                {
                    Self::record_unauthored_skip(&conn)?;
                    return Ok(());
                }
            }
        }
        upsert_queue_row(
            &conn,
            entity_type,
            entity_id,
            operation,
            payload,
            team_id,
            project_id,
        )
    }

    /// Queue the two team operations needed to move a task between project
    /// identities. Both rows are inserted in one transaction so a pending
    /// move cannot expose only one side of the cloud-key rewrite.
    pub fn enqueue_team_move(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        old_project_id: &str,
        new_project_id: &str,
        payload: &str,
        team_id: &str,
    ) -> Result<(), CasError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        enqueue_team_move_rows(
            &tx,
            entity_type,
            entity_id,
            old_project_id,
            new_project_id,
            payload,
            team_id,
        )?;
        tx.commit()?;
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
            SELECT id, entity_type, entity_id, operation, payload, team_id, project_id, created_at, retry_count, last_error, last_outcome, last_reason, failed_client_version
            FROM sync_queue
            WHERE retry_count < ?1 AND (team_id IS NULL OR team_id = '')
              AND entity_type != 'knowledge_page'
              AND (?3 IS NULL OR entity_type = ?3)
            ORDER BY created_at ASC, id ASC
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
            SELECT id, entity_type, entity_id, operation, payload, team_id, project_id, created_at, retry_count, last_error, last_outcome, last_reason, failed_client_version
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
            SELECT id, entity_type, entity_id, operation, payload, team_id, project_id, created_at, retry_count, last_error, last_outcome, last_reason, failed_client_version
            FROM sync_queue
            WHERE retry_count < ?1 AND team_id = ?2
            ORDER BY created_at ASC, id ASC
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
            SELECT id, entity_type, entity_id, operation, payload, team_id, project_id, created_at, retry_count, last_error, last_outcome, last_reason, failed_client_version
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
        let created_str: String = row.get(7)?;
        let team_id: Option<String> = row
            .get::<_, Option<String>>(5)?
            .filter(|value| !value.is_empty());
        let project_id: Option<String> = row
            .get::<_, Option<String>>(6)?
            .filter(|value| !value.trim().is_empty());

        Ok(QueuedSync {
            id: row.get(0)?,
            entity_type: EntityType::parse(&entity_type_str).unwrap_or(EntityType::Entry),
            entity_id: row.get(2)?,
            operation: SyncOperation::parse(&operation_str).unwrap_or(SyncOperation::Upsert),
            payload: row.get(4)?,
            team_id,
            project_id,
            created_at: DateTime::parse_from_rfc3339(&created_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            retry_count: row.get(8)?,
            last_error: row.get(9)?,
            last_outcome: row.get(10)?,
            last_reason: row.get(11)?,
            failed_client_version: row.get(12)?,
        })
    }
}
