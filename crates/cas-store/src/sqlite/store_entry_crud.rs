use crate::Result;
use crate::error::StoreError;
use crate::event_store::record_event_with_conn;
use crate::recording_store::capture_memory_event;
use crate::sqlite::{ENTRIES_RULES_SCHEMA, SqliteStore};
use crate::tracing::{DevTracer, TraceTimer};
use cas_types::{Entry, Event, EventEntityType, EventType, Scope};
use chrono::Utc;
use rusqlite::{OptionalExtension, params};

impl SqliteStore {
    pub(crate) fn store_init(&self) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        // ENTRIES_RULES_SCHEMA covers entries, rules, metadata, sessions
        // (+ all indexes including the helpful-score expression index).
        // No additional inline DDL needed here — single source of truth.
        // NOTE: Column migrations are now handled by `cas update --schema-only`
        // See cas-cli/src/migration/migrations.rs for the migration definitions.
        conn.execute_batch(ENTRIES_RULES_SCHEMA)?;
        Ok(())
    }
    pub(crate) fn store_generate_id(&self) -> Result<String> {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        // Use a per-day sequence key so IDs reset daily (e.g., "entry:2026-03-30")
        let seq_name = format!("entry:{today}");
        let id_pattern = format!("{today}-[0-9]*");
        let suffix_start = (today.len() + 2) as i64; // SQLite SUBSTR positions are 1-indexed.

        loop {
            let next_num = crate::shared_db::next_sequence_val(&conn, &seq_name)?;
            let max_existing: Option<i64> = conn.query_row(
                "SELECT MAX(CASE
                    WHEN id GLOB ?1 THEN CAST(SUBSTR(id, ?2) AS INTEGER)
                 END)
                 FROM entries",
                params![&id_pattern, suffix_start],
                |row| row.get(0),
            )?;

            // Synced or restored rows do not advance the local sequence. Keep
            // the counter above every same-day entry before handing an ID to a
            // caller, otherwise a deterministic retry can return the same
            // colliding slug again.
            if max_existing.is_none_or(|max| next_num > max) {
                return Ok(format!("{today}-{next_num}"));
            }

            conn.execute(
                "INSERT INTO id_sequences (name, next_val) VALUES (?1, ?2)
                 ON CONFLICT(name) DO UPDATE SET next_val =
                    CASE
                        WHEN next_val < excluded.next_val THEN excluded.next_val
                        ELSE next_val
                    END",
                params![
                    &seq_name,
                    max_existing.expect("max existing ID is present after collision check")
                ],
            )?;
        }
    }
    pub(crate) fn store_add(&self, entry: &Entry) -> Result<()> {
        let timer = TraceTimer::new();
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = crate::shared_db::ImmediateTx::new(&conn)?;
            let now = Utc::now().to_rfc3339();
            let result = tx.execute(
            "INSERT INTO entries (id, type, tags, created, content, title,
             helpful_count, harmful_count, last_accessed, archived,
             session_id, source_tool, pending_extraction, observation_type,
             stability, access_count, raw_content, compressed, memory_tier, importance,
             valid_from, valid_until, review_after, last_reviewed, pending_embedding, belief_type, confidence, domain, branch, scope, team_id, share, updated_at, source_ids)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34)",
            params![
                entry.id,
                entry.entry_type.to_string(),
                Self::tags_to_string(&entry.tags),
                entry.created.to_rfc3339(),
                entry.content,
                entry.title,
                entry.helpful_count,
                entry.harmful_count,
                entry.last_accessed.map(|t| t.to_rfc3339()),
                entry.archived as i32,
                entry.session_id,
                entry.source_tool,
                entry.pending_extraction as i32,
                entry.observation_type.map(|t| t.to_string()),
                entry.stability,
                entry.access_count,
                entry.raw_content,
                entry.compressed as i32,
                entry.memory_tier.to_string(),
                entry.importance,
                entry.valid_from.map(|t| t.to_rfc3339()),
                entry.valid_until.map(|t| t.to_rfc3339()),
                entry.review_after.map(|t| t.to_rfc3339()),
                entry.last_reviewed.map(|t| t.to_rfc3339()),
                entry.pending_embedding as i32,
                entry.belief_type.to_string(),
                entry.confidence,
                entry.domain,
                entry.branch,
                entry.scope.to_string(),
                entry.team_id,
                entry.share.as_ref().map(|s| s.to_string()),
                now, // updated_at = created time for new entries
                Self::source_ids_to_string(&entry.source_ids),
            ],
        );

            // Record trace (only if store ops tracing is enabled)
            if let Some(tracer) = DevTracer::get() {
                if tracer.should_trace_store_ops() {
                    let (success, error) = match &result {
                        Ok(_) => (true, None),
                        Err(e) => (false, Some(e.to_string())),
                    };
                    let _ = tracer.record_store_op(
                        "add",
                        "sqlite",
                        &[entry.id.as_str()],
                        if success { 1 } else { 0 },
                        timer.elapsed_ms(),
                        success,
                        error.as_deref(),
                    );
                }
            }

            match result {
                Ok(_) => {}
                Err(rusqlite::Error::SqliteFailure(sqlite_error, message))
                    if sqlite_error.code == rusqlite::ffi::ErrorCode::ConstraintViolation
                        && message.as_deref().is_some_and(|message| {
                            message.contains("UNIQUE constraint failed: entries.id")
                        }) =>
                {
                    return Err(StoreError::EntryExists(entry.id.clone()));
                }
                Err(error) => return Err(StoreError::Database(error)),
            }

            // Record event for sidecar activity feed (within same transaction)
            let summary = entry.title.as_deref().unwrap_or_else(|| {
                // Truncate content for summary
                if entry.content.len() > 50 {
                    &entry.content[..50]
                } else {
                    &entry.content
                }
            });
            let event = Event::new(
                EventType::MemoryStored,
                EventEntityType::Entry,
                &entry.id,
                format!("Memory stored: {summary}"),
            )
            .with_session(entry.session_id.as_deref().unwrap_or(""));
            let _ = record_event_with_conn(&tx, &event);

            // Capture event for recording playback (within same transaction)
            let _ = capture_memory_event(&tx, &entry.id, None);

            tx.commit()?;
            Ok(())
        }) // with_write_retry
    }
    pub(crate) fn store_get(&self, id: &str) -> Result<Entry> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let entry = conn
            .query_row(
                "SELECT id, type, tags, created, content, title, helpful_count,
                 harmful_count, last_accessed, archived, session_id, source_tool,
                 pending_extraction, observation_type, stability, access_count,
                 raw_content, compressed, memory_tier, importance, valid_from, valid_until, review_after, last_reviewed, pending_embedding,
                 belief_type, confidence, domain, branch, scope, team_id, share, source_ids
                 FROM entries WHERE id = ? AND archived = 0",
                params![id],
                Self::row_to_entry,
            )
            .optional()?
            .ok_or_else(|| StoreError::EntryNotFound(id.to_string()))?;
        Ok(entry)
    }
    pub(crate) fn store_get_archived(&self, id: &str) -> Result<Entry> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let entry = conn
            .query_row(
                "SELECT id, type, tags, created, content, title, helpful_count,
                 harmful_count, last_accessed, archived, session_id, source_tool,
                 pending_extraction, observation_type, stability, access_count,
                 raw_content, compressed, memory_tier, importance, valid_from, valid_until, review_after, last_reviewed, pending_embedding,
                 belief_type, confidence, domain, branch, scope, team_id, share, source_ids
                 FROM entries WHERE id = ? AND archived = 1",
                params![id],
                Self::row_to_entry,
            )
            .optional()?
            .ok_or_else(|| StoreError::EntryNotFound(id.to_string()))?;
        Ok(entry)
    }
    pub(crate) fn store_update(&self, entry: &Entry) -> Result<()> {
        let timer = TraceTimer::new();
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = crate::shared_db::ImmediateTx::new(&conn)?;
            let now = Utc::now().to_rfc3339();
            let result = tx.execute(
            "UPDATE entries SET type = ?1, tags = ?2, content = ?3, title = ?4,
             helpful_count = ?5, harmful_count = ?6, last_accessed = ?7, archived = ?8,
             session_id = ?9, source_tool = ?10, pending_extraction = ?11, observation_type = ?12,
             stability = ?13, access_count = ?14, raw_content = ?15, compressed = ?16,
             memory_tier = ?17, importance = ?18, valid_from = ?19, valid_until = ?20, review_after = ?21,
             last_reviewed = ?22, pending_embedding = ?23, belief_type = ?24, confidence = ?25, domain = ?26, branch = ?27,
             updated_at = ?28, scope = ?29, share = ?30, source_ids = ?31
             WHERE id = ?32",
            params![
                entry.entry_type.to_string(),
                Self::tags_to_string(&entry.tags),
                entry.content,
                entry.title,
                entry.helpful_count,
                entry.harmful_count,
                entry.last_accessed.map(|t| t.to_rfc3339()),
                entry.archived as i32,
                entry.session_id,
                entry.source_tool,
                entry.pending_extraction as i32,
                entry.observation_type.map(|t| t.to_string()),
                entry.stability,
                entry.access_count,
                entry.raw_content,
                entry.compressed as i32,
                entry.memory_tier.to_string(),
                entry.importance,
                entry.valid_from.map(|t| t.to_rfc3339()),
                entry.valid_until.map(|t| t.to_rfc3339()),
                entry.review_after.map(|t| t.to_rfc3339()),
                entry.last_reviewed.map(|t| t.to_rfc3339()),
                entry.pending_embedding as i32,
                entry.belief_type.to_string(),
                entry.confidence,
                entry.domain,
                entry.branch,
                now, // updated_at = current time on update
                entry.scope.to_string(),
                entry.share.as_ref().map(|s| s.to_string()),
                Self::source_ids_to_string(&entry.source_ids),
                entry.id,
            ],
        );

            // Record trace (only if store ops tracing is enabled)
            if let Some(tracer) = DevTracer::get() {
                if tracer.should_trace_store_ops() {
                    let (success, error) = match &result {
                        Ok(rows) => (*rows > 0, None),
                        Err(e) => (false, Some(e.to_string())),
                    };
                    let _ = tracer.record_store_op(
                        "update",
                        "sqlite",
                        &[entry.id.as_str()],
                        result.as_ref().copied().unwrap_or(0),
                        timer.elapsed_ms(),
                        success,
                        error.as_deref(),
                    );
                }
            }

            let rows = result?;
            if rows == 0 {
                return Err(StoreError::EntryNotFound(entry.id.clone()));
            }
            tx.commit()?;
            Ok(())
        }) // with_write_retry
    }

    /// Atomically apply the fields used by a memory merge only while the
    /// lifecycle timestamp observed by the caller still matches. Returning
    /// `Ok(false)` is a normal optimistic-concurrency conflict.
    pub(crate) fn store_update_if_unmodified(
        &self,
        entry: &Entry,
        expected_updated_at: chrono::DateTime<Utc>,
    ) -> Result<bool> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = crate::shared_db::ImmediateTx::new(&conn)?;
            let rows = tx.execute(
                "UPDATE entries SET type = ?1, tags = ?2, content = ?3, title = ?4,
                 importance = ?5, valid_from = ?6, valid_until = ?7,
                 pending_embedding = 1, updated_at = ?8
                 WHERE id = ?9
                   AND MAX(created, COALESCE(updated_at, created)) = ?10",
                params![
                    entry.entry_type.to_string(),
                    Self::tags_to_string(&entry.tags),
                    entry.content,
                    entry.title,
                    entry.importance,
                    entry.valid_from.map(|t| t.to_rfc3339()),
                    entry.valid_until.map(|t| t.to_rfc3339()),
                    Utc::now().to_rfc3339(),
                    entry.id,
                    expected_updated_at.to_rfc3339(),
                ],
            )?;
            tx.commit()?;
            Ok(rows == 1)
        })
    }
    pub(crate) fn store_delete(&self, id: &str) -> Result<()> {
        let timer = TraceTimer::new();
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let result = conn.execute("DELETE FROM entries WHERE id = ?", params![id]);

        // Record trace (only if store ops tracing is enabled)
        if let Some(tracer) = DevTracer::get() {
            if tracer.should_trace_store_ops() {
                let (success, error) = match &result {
                    Ok(rows) => (*rows > 0, None),
                    Err(e) => (false, Some(e.to_string())),
                };
                let _ = tracer.record_store_op(
                    "delete",
                    "sqlite",
                    &[id],
                    result.as_ref().copied().unwrap_or(0),
                    timer.elapsed_ms(),
                    success,
                    error.as_deref(),
                );
            }
        }

        let rows = result?;
        if rows == 0 {
            return Err(StoreError::EntryNotFound(id.to_string()));
        }
        Ok(())
    }
    pub(crate) fn store_list(&self) -> Result<Vec<Entry>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, type, tags, created, content, title, helpful_count,
             harmful_count, last_accessed, archived, session_id, source_tool,
             pending_extraction, observation_type, stability, access_count,
             raw_content, compressed, memory_tier, importance, valid_from, valid_until, review_after, last_reviewed, pending_embedding,
             belief_type, confidence, domain, branch, scope, team_id, share, source_ids
             FROM entries WHERE archived = 0 ORDER BY created DESC LIMIT 10000",
        )?;

        let entries = stmt
            .query_map([], Self::row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    pub(crate) fn store_list_by_scope_and_tag(
        &self,
        scope: Scope,
        tag: &str,
    ) -> Result<Vec<Entry>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, type, tags, created, content, title, helpful_count,
             harmful_count, last_accessed, archived, session_id, source_tool,
             pending_extraction, observation_type, stability, access_count,
             raw_content, compressed, memory_tier, importance, valid_from, valid_until, review_after, last_reviewed, pending_embedding,
             belief_type, confidence, domain, branch, scope, team_id, share, source_ids
             FROM entries
             WHERE archived = 0
               AND scope = ?
               AND tags IS NOT NULL
               AND instr(lower(tags), lower(?)) > 0
             ORDER BY created DESC LIMIT 10000",
        )?;

        let entries = stmt
            .query_map(params![scope.to_string(), tag], Self::row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    pub(crate) fn store_list_decayable(&self) -> Result<Vec<Entry>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, type, tags, created, content, title, helpful_count,
             harmful_count, last_accessed, archived, session_id, source_tool,
             pending_extraction, observation_type, stability, access_count,
             raw_content, compressed, memory_tier, importance, valid_from, valid_until, review_after, last_reviewed, pending_embedding,
             belief_type, confidence, domain, branch, scope, team_id, share, source_ids
             FROM entries WHERE archived = 0 AND memory_tier NOT IN ('in_context', 'archive')
             ORDER BY created DESC LIMIT 10000",
        )?;

        let entries = stmt
            .query_map([], Self::row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    pub(crate) fn store_list_prunable(&self, stability_threshold: f32) -> Result<Vec<Entry>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, type, tags, created, content, title, helpful_count,
             harmful_count, last_accessed, archived, session_id, source_tool,
             pending_extraction, observation_type, stability, access_count,
             raw_content, compressed, memory_tier, importance, valid_from, valid_until, review_after, last_reviewed, pending_embedding,
             belief_type, confidence, domain, branch, scope, team_id, share, source_ids
             FROM entries WHERE archived = 0 AND stability < ?
             ORDER BY stability ASC LIMIT 10000",
        )?;

        let entries = stmt
            .query_map(rusqlite::params![stability_threshold], Self::row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    pub(crate) fn store_recent(&self, n: usize) -> Result<Vec<Entry>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, type, tags, created, content, title, helpful_count,
             harmful_count, last_accessed, archived, session_id, source_tool,
             pending_extraction, observation_type, stability, access_count,
             raw_content, compressed, memory_tier, importance, valid_from, valid_until, review_after, last_reviewed, pending_embedding,
             belief_type, confidence, domain, branch, scope, team_id, share, source_ids
             FROM entries
             WHERE archived = 0
             ORDER BY MAX(created, COALESCE(updated_at, created)) DESC, id DESC
             LIMIT ?",
        )?;

        let entries = stmt
            .query_map(params![n as i64], Self::row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    pub(crate) fn store_recent_timestamp(&self, entry: &Entry) -> Result<chrono::DateTime<Utc>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let timestamp = conn.query_row(
            "SELECT MAX(created, COALESCE(updated_at, created)) FROM entries WHERE id = ?",
            params![entry.id],
            |row| row.get::<_, String>(0),
        )?;
        Ok(Self::parse_datetime(&timestamp).unwrap_or(entry.created))
    }

    pub(crate) fn store_archive(&self, id: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let rows = conn.execute(
            "UPDATE entries SET archived = 1 WHERE id = ? AND archived = 0",
            params![id],
        )?;
        if rows == 0 {
            return Err(StoreError::EntryNotFound(id.to_string()));
        }
        Ok(())
    }
    pub(crate) fn store_unarchive(&self, id: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let rows = conn.execute(
            "UPDATE entries SET archived = 0 WHERE id = ? AND archived = 1",
            params![id],
        )?;
        if rows == 0 {
            return Err(StoreError::EntryNotFound(id.to_string()));
        }
        Ok(())
    }
    pub(crate) fn store_list_archived(&self) -> Result<Vec<Entry>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, type, tags, created, content, title, helpful_count,
             harmful_count, last_accessed, archived, session_id, source_tool,
             pending_extraction, observation_type, stability, access_count,
             raw_content, compressed, memory_tier, importance, valid_from, valid_until, review_after, last_reviewed, pending_embedding,
             belief_type, confidence, domain, branch, scope, team_id, share, source_ids
             FROM entries WHERE archived = 1 ORDER BY created DESC LIMIT 10000",
        )?;

        let entries = stmt
            .query_map([], Self::row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    pub(crate) fn store_list_by_branch(&self, branch: &str) -> Result<Vec<Entry>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, type, tags, created, content, title, helpful_count,
             harmful_count, last_accessed, archived, session_id, source_tool,
             pending_extraction, observation_type, stability, access_count,
             raw_content, compressed, memory_tier, importance, valid_from, valid_until, review_after, last_reviewed, pending_embedding,
             belief_type, confidence, domain, branch, scope, team_id, share, source_ids
             FROM entries WHERE branch = ? AND archived = 0 ORDER BY created DESC",
        )?;

        let entries = stmt
            .query_map(params![branch], Self::row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }
}
