use crate::Result;
use crate::error::StoreError;
use crate::sqlite::SqliteStore;
use cas_types::Entry;
use chrono::Utc;
use rusqlite::params;

impl SqliteStore {
    pub(crate) fn store_list_pending(&self, limit: usize) -> Result<Vec<Entry>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, type, tags, created, content, title, helpful_count,
             harmful_count, last_accessed, archived, session_id, source_tool,
             pending_extraction, observation_type, stability, access_count,
             raw_content, compressed, memory_tier, importance, valid_from, valid_until, review_after, last_reviewed, pending_embedding,
             belief_type, confidence, domain, branch, scope, team_id, share
             FROM entries WHERE pending_extraction = 1 AND archived = 0
             ORDER BY created DESC LIMIT ?",
        )?;

        let entries = stmt
            .query_map(params![limit as i64], Self::row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }
    pub(crate) fn store_mark_extracted(&self, id: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let rows = conn.execute(
            "UPDATE entries SET pending_extraction = 0 WHERE id = ?",
            params![id],
        )?;
        if rows == 0 {
            return Err(StoreError::EntryNotFound(id.to_string()));
        }
        Ok(())
    }
    pub(crate) fn store_list_pinned(&self) -> Result<Vec<Entry>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, type, tags, created, content, title, helpful_count,
             harmful_count, last_accessed, archived, session_id, source_tool,
             pending_extraction, observation_type, stability, access_count,
             raw_content, compressed, memory_tier, importance, valid_from, valid_until, review_after, last_reviewed, pending_embedding,
             belief_type, confidence, domain, branch, scope, team_id, share
             FROM entries WHERE memory_tier = 'in-context' AND archived = 0
             ORDER BY created DESC",
        )?;

        let entries = stmt
            .query_map([], Self::row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }
    pub(crate) fn store_list_helpful(&self, limit: usize) -> Result<Vec<Entry>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, type, tags, created, content, title, helpful_count,
             harmful_count, last_accessed, archived, session_id, source_tool,
             pending_extraction, observation_type, stability, access_count,
             raw_content, compressed, memory_tier, importance, valid_from, valid_until, review_after, last_reviewed, pending_embedding,
             belief_type, confidence, domain, branch, scope, team_id, share
             FROM entries
             WHERE archived = 0 AND (helpful_count - harmful_count) > 0
             ORDER BY (helpful_count - harmful_count) DESC, last_accessed DESC
             LIMIT ?",
        )?;

        let entries = stmt
            .query_map([limit as i64], Self::row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }
    pub(crate) fn store_list_by_session(&self, session_id: &str) -> Result<Vec<Entry>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, type, tags, created, content, title, helpful_count,
             harmful_count, last_accessed, archived, session_id, source_tool,
             pending_extraction, observation_type, stability, access_count,
             raw_content, compressed, memory_tier, importance, valid_from, valid_until, review_after, last_reviewed, pending_embedding,
             belief_type, confidence, domain, branch, scope, team_id, share
             FROM entries
             WHERE session_id = ? AND archived = 0
             ORDER BY created DESC LIMIT 10000",
        )?;

        let entries = stmt
            .query_map([session_id], Self::row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }
    pub(crate) fn store_list_unreviewed_learnings(&self, limit: usize) -> Result<Vec<Entry>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, type, tags, created, content, title, helpful_count,
             harmful_count, last_accessed, archived, session_id, source_tool,
             pending_extraction, observation_type, stability, access_count,
             raw_content, compressed, memory_tier, importance, valid_from, valid_until, review_after, last_reviewed, pending_embedding,
             belief_type, confidence, domain, branch, scope, team_id, share
             FROM entries
             WHERE type = 'learning' AND archived = 0 AND last_reviewed IS NULL
             ORDER BY created DESC
             LIMIT ?",
        )?;

        let entries = stmt
            .query_map([limit as i64], Self::row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }
    pub(crate) fn store_mark_reviewed(&self, id: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let now = Utc::now().to_rfc3339();
        let rows = conn.execute(
            "UPDATE entries SET last_reviewed = ? WHERE id = ?",
            params![now, id],
        )?;
        if rows == 0 {
            return Err(StoreError::EntryNotFound(id.to_string()));
        }
        Ok(())
    }
}
