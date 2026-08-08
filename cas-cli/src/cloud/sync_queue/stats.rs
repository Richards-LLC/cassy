use rusqlite::{OptionalExtension, params};
use std::collections::HashMap;

use crate::cloud::sync_queue::{EntityType, PendingByType, QueueStats, QueuedSync, SyncQueue};
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
                EntityType::KnowledgePage => grouped.knowledge_pages.push(item),
            }
        }
        grouped
    }
}
