use crate::Result;
use crate::error::StoreError;
use crate::sqlite::SqliteStore;
use cas_types::Entry;
use chrono::Utc;
use rusqlite::params;
use std::path::Path;

impl SqliteStore {
    pub(crate) fn store_list_pending_index(&self, limit: usize) -> Result<Vec<Entry>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, type, tags, created, content, title, helpful_count,
             harmful_count, last_accessed, archived, session_id, source_tool,
             pending_extraction, observation_type, stability, access_count,
             raw_content, compressed, memory_tier, importance, valid_from, valid_until,
             review_after, last_reviewed, pending_embedding, belief_type, confidence, domain, branch, scope, team_id, share, source_ids
             FROM entries
             WHERE archived = 0 AND (indexed_at IS NULL OR updated_at > indexed_at)
             ORDER BY updated_at DESC
             LIMIT ?",
        )?;

        let entries = stmt
            .query_map(params![limit as i64], Self::row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }
    pub(crate) fn store_mark_indexed(&self, id: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let now = Utc::now().to_rfc3339();
        let rows = conn.execute(
            "UPDATE entries SET indexed_at = ? WHERE id = ?",
            params![now, id],
        )?;
        if rows == 0 {
            return Err(StoreError::EntryNotFound(id.to_string()));
        }
        Ok(())
    }
    pub(crate) fn store_mark_indexed_batch(&self, ids: &[&str]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        const CHUNK_SIZE: usize = 500;
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let now = Utc::now().to_rfc3339();

        for chunk in ids.chunks(CHUNK_SIZE) {
            let placeholders: Vec<String> =
                (0..chunk.len()).map(|i| format!("?{}", i + 2)).collect();
            let query = format!(
                "UPDATE entries SET indexed_at = ?1 WHERE id IN ({})",
                placeholders.join(", ")
            );

            let mut params_vec: Vec<&dyn rusqlite::ToSql> = vec![&now];
            for id in chunk {
                params_vec.push(id);
            }

            conn.execute(&query, params_vec.as_slice())?;
        }
        Ok(())
    }
    pub(crate) fn store_cas_dir(&self) -> &Path {
        &self.cas_dir
    }
    pub(crate) fn store_close(&self) -> Result<()> {
        Ok(())
    }
}
