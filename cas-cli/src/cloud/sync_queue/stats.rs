use rusqlite::{OptionalExtension, params};
use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::cloud::sync_queue::{
    EntityType, PendingByType, QueueHealth, QueueStats, QueuedSync, SyncConflictRecord, SyncQueue,
};
use crate::error::CasError;

impl SyncQueue {
    /// Get items grouped by entity type for batched sync.
    pub fn pending_by_type(
        &self,
        limit: usize,
        max_retries: i32,
    ) -> Result<PendingByType, CasError> {
        let items = self.pending(limit, max_retries)?;
        Ok(Self::group_pending_items(items))
    }

    /// Get personal pending items for one entity type, grouped for the syncer.
    pub fn pending_by_type_for_entity(
        &self,
        entity_type: EntityType,
        limit: usize,
        max_retries: i32,
    ) -> Result<PendingByType, CasError> {
        let items = self.pending_for_entity_type(Some(entity_type), limit, max_retries)?;
        Ok(Self::group_pending_items(items))
    }

    /// Get items grouped by entity type for a specific team.
    pub fn pending_by_type_for_team(
        &self,
        team_id: &str,
        limit: usize,
        max_retries: i32,
    ) -> Result<PendingByType, CasError> {
        let items = self.pending_for_team(team_id, limit, max_retries)?;
        Ok(Self::group_pending_items(items))
    }

    /// Get queue statistics.
    pub fn stats(&self, max_retries: i32) -> Result<QueueStats, CasError> {
        let conn = self.conn.lock().unwrap();

        let total: i64 = conn.query_row("SELECT COUNT(*) FROM sync_queue", [], |row| row.get(0))?;

        let pending: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sync_queue WHERE retry_count < ?1",
            params![max_retries],
            |row| row.get(0),
        )?;

        let failed: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sync_queue WHERE retry_count >= ?1",
            params![max_retries],
            |row| row.get(0),
        )?;

        let mut by_type = HashMap::new();
        let mut stmt =
            conn.prepare("SELECT entity_type, COUNT(*) FROM sync_queue GROUP BY entity_type")?;
        let rows = stmt.query_map([], |row| {
            let entity_type: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((entity_type, count as usize))
        })?;

        for row in rows {
            let (entity_type, count) = row?;
            by_type.insert(entity_type, count);
        }

        // oldest_item reports the head of the *pending* queue (retry_count <
        // max_retries) so that parked/failed items don't hold oldest_item
        // frozen after the poison-head fix (defect B / cas-8dd8).
        let oldest_item: Option<String> = conn
            .query_row(
                "SELECT created_at FROM sync_queue WHERE retry_count < ?1 ORDER BY created_at ASC LIMIT 1",
                params![max_retries],
                |row| row.get(0),
            )
            .optional()?;

        Ok(QueueStats {
            total: total as usize,
            pending: pending as usize,
            failed: failed as usize,
            by_type,
            oldest_item,
        })
    }

    /// Capture the queue facts needed to detect a stalled cloud push path.
    pub fn health(&self, max_retries: i32, now: DateTime<Utc>) -> Result<QueueHealth, CasError> {
        let conn = self.conn.lock().unwrap();
        let pending: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sync_queue WHERE retry_count < ?1",
            params![max_retries],
            |row| row.get(0),
        )?;
        let oldest_item_text: Option<String> = conn
            .query_row(
                "SELECT created_at FROM sync_queue WHERE retry_count < ?1 ORDER BY created_at ASC LIMIT 1",
                params![max_retries],
                |row| row.get(0),
            )
            .optional()?;
        let oldest_item = oldest_item_text
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc));
        let last_error: Option<String> = conn
            .query_row(
                "SELECT last_error FROM sync_queue WHERE last_error IS NOT NULL ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let has_conflict_table: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'sync_conflicts')",
            [],
            |row| row.get(0),
        )?;
        let unreviewed_conflicts = if has_conflict_table {
            conn.query_row("SELECT COUNT(*) FROM sync_conflicts", [], |row| {
                row.get::<_, i64>(0)
            })? as usize
        } else {
            0
        };

        Ok(QueueHealth {
            pending: pending as usize,
            oldest_age_secs: oldest_item.map(|created_at| (now - created_at).num_seconds().max(0)),
            oldest_item,
            last_error,
            unreviewed_conflicts,
        })
    }

    /// Persist a full local row before a pull replaces or merges it.
    ///
    /// `local_revision`/`remote_revision` are the server revisions the decision
    /// was made on. Both are `None` when the conflict was settled on the
    /// timestamp path, which is a real and legitimate state — an operator
    /// auditing the journal has to be able to tell the two regimes apart.
    pub fn record_conflict(
        &self,
        entity_type: &str,
        entity_id: &str,
        discarded_row_json: &str,
        winner_side: &str,
        strategy: &str,
        local_revision: Option<i64>,
        remote_revision: Option<i64>,
    ) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sync_conflicts (entity_type, entity_id, discarded_row_json, winner_side, strategy, resolved_at, local_revision, remote_revision) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![entity_type, entity_id, discarded_row_json, winner_side, strategy, Utc::now().to_rfc3339(), local_revision, remote_revision],
        )?;
        Ok(())
    }

    /// Whether this machine still has a local queued mutation for the row.
    pub fn has_pending_entity_change(
        &self,
        entity_type: EntityType,
        entity_id: &str,
    ) -> Result<bool, CasError> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sync_queue WHERE entity_type = ?1 AND entity_id = ?2)",
            params![entity_type.as_str(), entity_id],
            |row| row.get(0),
        )?)
    }

    pub fn list_conflicts(&self, limit: usize) -> Result<Vec<SyncConflictRecord>, CasError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, entity_type, entity_id, discarded_row_json, winner_side, strategy, resolved_at, local_revision, remote_revision FROM sync_conflicts ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(SyncConflictRecord {
                id: row.get(0)?,
                entity_type: row.get(1)?,
                entity_id: row.get(2)?,
                discarded_row_json: row.get(3)?,
                winner_side: row.get(4)?,
                strategy: row.get(5)?,
                resolved_at: row.get(6)?,
                local_revision: row.get(7)?,
                remote_revision: row.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn unreviewed_conflict_count(&self) -> Result<usize, CasError> {
        let conn = self.conn.lock().unwrap();
        Ok(
            conn.query_row("SELECT COUNT(*) FROM sync_conflicts", [], |row| {
                row.get::<_, i64>(0)
            })? as usize,
        )
    }

    pub fn prune_conflicts(&self, older_than_days: i64) -> Result<usize, CasError> {
        let conn = self.conn.lock().unwrap();
        let cutoff = Utc::now() - chrono::Duration::days(older_than_days);
        Ok(conn.execute(
            "DELETE FROM sync_conflicts WHERE resolved_at <= ?1",
            params![cutoff.to_rfc3339()],
        )?)
    }

    fn group_pending_items(items: Vec<QueuedSync>) -> PendingByType {
        let mut grouped = PendingByType::default();
        for item in items {
            match item.entity_type {
                EntityType::Entry => grouped.entries.push(item),
                EntityType::Task => grouped.tasks.push(item),
                EntityType::Rule => grouped.rules.push(item),
                EntityType::Skill => grouped.skills.push(item),
                EntityType::Session => grouped.sessions.push(item),
                EntityType::Verification => grouped.verifications.push(item),
                EntityType::Event => grouped.events.push(item),
                EntityType::Prompt => grouped.prompts.push(item),
                EntityType::FileChange => grouped.file_changes.push(item),
                EntityType::CommitLink => grouped.commit_links.push(item),
                EntityType::Agent => grouped.agents.push(item),
                EntityType::Worktree => grouped.worktrees.push(item),
                EntityType::TaskDependency => grouped.task_dependencies.push(item),
                EntityType::KnowledgePage => grouped.knowledge_pages.push(item),
            }
        }
        grouped
    }
}
