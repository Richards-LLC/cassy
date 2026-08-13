use crate::Result;
use crate::error::StoreError;
use crate::recording_store::{RecordingStore, SqliteRecordingStore};
use cas_types::{Recording, RecordingAgent, RecordingEvent, RecordingQuery};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};

impl RecordingStore for SqliteRecordingStore {
    fn init(&self) -> Result<()> {
        // Schema is created via migrations
        Ok(())
    }

    fn generate_id(&self) -> Result<String> {
        Ok(Recording::generate_id())
    }

    fn add(&self, recording: &Recording) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "INSERT INTO recordings (id, session_id, started_at, ended_at, duration_ms,
             file_path, file_size, title, description, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                recording.id,
                recording.session_id,
                recording.started_at.to_rfc3339(),
                recording.ended_at.map(|t| t.to_rfc3339()),
                recording.duration_ms,
                recording.file_path,
                recording.file_size,
                recording.title,
                recording.description,
                recording.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Recording> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.query_row(
            "SELECT id, session_id, started_at, ended_at, duration_ms,
             file_path, file_size, title, description, created_at
             FROM recordings WHERE id = ?",
            params![id],
            Self::recording_from_row,
        )
        .optional()?
        .ok_or_else(|| StoreError::Other(format!("Recording not found: {id}")))
    }

    fn update(&self, recording: &Recording) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let rows = conn.execute(
            "UPDATE recordings SET session_id = ?1, started_at = ?2, ended_at = ?3,
             duration_ms = ?4, file_path = ?5, file_size = ?6, title = ?7, description = ?8
             WHERE id = ?9",
            params![
                recording.session_id,
                recording.started_at.to_rfc3339(),
                recording.ended_at.map(|t| t.to_rfc3339()),
                recording.duration_ms,
                recording.file_path,
                recording.file_size,
                recording.title,
                recording.description,
                recording.id,
            ],
        )?;
        if rows == 0 {
            return Err(StoreError::Other(format!(
                "Recording not found: {}",
                recording.id
            )));
        }
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let rows = conn.execute("DELETE FROM recordings WHERE id = ?", params![id])?;
        if rows == 0 {
            return Err(StoreError::Other(format!("Recording not found: {id}")));
        }
        Ok(())
    }

    fn list(&self) -> Result<Vec<Recording>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, session_id, started_at, ended_at, duration_ms,
             file_path, file_size, title, description, created_at
             FROM recordings ORDER BY started_at DESC",
        )?;

        let recordings = stmt
            .query_map([], Self::recording_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(recordings)
    }

    fn query(&self, query: &RecordingQuery) -> Result<Vec<Recording>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let mut sql = String::from(
            "SELECT r.id, r.session_id, r.started_at, r.ended_at, r.duration_ms,
             r.file_path, r.file_size, r.title, r.description, r.created_at
             FROM recordings r",
        );
        let mut conditions = Vec::new();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if query.agent_name.is_some() {
            sql.push_str(" JOIN recording_agents ra ON r.id = ra.recording_id");
        }

        if let Some(ref session_id) = query.session_id {
            conditions.push("r.session_id = ?");
            params_vec.push(Box::new(session_id.clone()));
        }

        if let Some(ref from_date) = query.from_date {
            conditions.push("r.started_at >= ?");
            params_vec.push(Box::new(from_date.to_rfc3339()));
        }

        if let Some(ref to_date) = query.to_date {
            conditions.push("r.started_at <= ?");
            params_vec.push(Box::new(to_date.to_rfc3339()));
        }

        if let Some(ref agent_name) = query.agent_name {
            conditions.push("ra.agent_name = ?");
            params_vec.push(Box::new(agent_name.clone()));
        }

        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }

        sql.push_str(" ORDER BY r.started_at DESC");

        if let Some(limit) = query.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }

        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare_cached(&sql)?;
        let recordings = stmt
            .query_map(params_refs.as_slice(), Self::recording_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(recordings)
    }

    fn list_by_session(&self, session_id: &str) -> Result<Vec<Recording>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, session_id, started_at, ended_at, duration_ms,
             file_path, file_size, title, description, created_at
             FROM recordings WHERE session_id = ? ORDER BY started_at DESC",
        )?;

        let recordings = stmt
            .query_map(params![session_id], Self::recording_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(recordings)
    }

    fn list_by_date_range(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<Recording>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, session_id, started_at, ended_at, duration_ms,
             file_path, file_size, title, description, created_at
             FROM recordings WHERE started_at >= ? AND started_at <= ?
             ORDER BY started_at DESC",
        )?;

        let recordings = stmt
            .query_map(
                params![from.to_rfc3339(), to.to_rfc3339()],
                Self::recording_from_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(recordings)
    }

    fn list_by_agent(&self, agent_name: &str) -> Result<Vec<Recording>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT r.id, r.session_id, r.started_at, r.ended_at, r.duration_ms,
             r.file_path, r.file_size, r.title, r.description, r.created_at
             FROM recordings r
             JOIN recording_agents ra ON r.id = ra.recording_id
             WHERE ra.agent_name = ?
             ORDER BY r.started_at DESC",
        )?;

        let recordings = stmt
            .query_map(params![agent_name], Self::recording_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(recordings)
    }

    fn add_agent(&self, agent: &RecordingAgent) -> Result<i64> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "INSERT INTO recording_agents (recording_id, agent_name, agent_type, file_path, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                agent.recording_id,
                agent.agent_name,
                agent.agent_type,
                agent.file_path,
                agent.created_at.to_rfc3339(),
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    fn get_agents(&self, recording_id: &str) -> Result<Vec<RecordingAgent>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, recording_id, agent_name, agent_type, file_path, created_at
             FROM recording_agents WHERE recording_id = ?",
        )?;

        let agents = stmt
            .query_map(params![recording_id], Self::recording_agent_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(agents)
    }

    fn delete_agents(&self, recording_id: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "DELETE FROM recording_agents WHERE recording_id = ?",
            params![recording_id],
        )?;
        Ok(())
    }

    fn add_event(&self, event: &RecordingEvent) -> Result<i64> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "INSERT INTO recording_events (recording_id, timestamp_ms, event_type,
             entity_type, entity_id, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                event.recording_id,
                event.timestamp_ms,
                event.event_type.to_string(),
                event.entity_type,
                event.entity_id,
                event.metadata,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    fn get_events(&self, recording_id: &str) -> Result<Vec<RecordingEvent>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, recording_id, timestamp_ms, event_type, entity_type, entity_id, metadata
             FROM recording_events WHERE recording_id = ? ORDER BY timestamp_ms",
        )?;

        let events = stmt
            .query_map(params![recording_id], Self::recording_event_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(events)
    }

    fn get_events_in_range(
        &self,
        recording_id: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<RecordingEvent>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, recording_id, timestamp_ms, event_type, entity_type, entity_id, metadata
             FROM recording_events
             WHERE recording_id = ? AND timestamp_ms >= ? AND timestamp_ms <= ?
             ORDER BY timestamp_ms",
        )?;

        let events = stmt
            .query_map(
                params![recording_id, from_ms, to_ms],
                Self::recording_event_from_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(events)
    }

    fn get_events_for_entity(
        &self,
        recording_id: &str,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<Vec<RecordingEvent>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, recording_id, timestamp_ms, event_type, entity_type, entity_id, metadata
             FROM recording_events
             WHERE recording_id = ? AND entity_type = ? AND entity_id = ?
             ORDER BY timestamp_ms",
        )?;

        let events = stmt
            .query_map(
                params![recording_id, entity_type, entity_id],
                Self::recording_event_from_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(events)
    }

    fn delete_events(&self, recording_id: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "DELETE FROM recording_events WHERE recording_id = ?",
            params![recording_id],
        )?;
        Ok(())
    }

    fn add_fts_content(
        &self,
        recording_id: &str,
        content: &str,
        content_type: &str,
        timestamp_ms: i64,
    ) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "INSERT INTO recordings_fts (recording_id, content, content_type, timestamp_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                recording_id,
                content,
                content_type,
                timestamp_ms.to_string()
            ],
        )?;
        Ok(())
    }

    fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<(String, i64)>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT recording_id, CAST(timestamp_ms AS INTEGER)
             FROM recordings_fts
             WHERE recordings_fts MATCH ?
             LIMIT ?",
        )?;

        let results = stmt
            .query_map(params![query, limit as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(results)
    }

    fn delete_fts_content(&self, recording_id: &str) -> Result<()> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.execute(
            "DELETE FROM recordings_fts WHERE recording_id = ?",
            params![recording_id],
        )?;
        Ok(())
    }

    fn prune(&self, older_than_days: i64) -> Result<usize> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        // Check if recordings table exists (schema comes from migrations)
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='recordings'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !table_exists {
            return Ok(0);
        }

        let cutoff = (Utc::now() - chrono::Duration::days(older_than_days)).to_rfc3339();

        // Delete FTS content for old recordings first (no CASCADE on virtual tables).
        // Ignore errors — FTS table may not exist in all environments.
        let _ = conn.execute(
            "DELETE FROM recordings_fts WHERE recording_id IN (
                SELECT id FROM recordings WHERE created_at < ?1
            )",
            params![cutoff],
        );

        // Delete recordings — CASCADE handles recording_agents and recording_events
        let deleted = conn.execute(
            "DELETE FROM recordings WHERE created_at < ?",
            params![cutoff],
        )?;

        Ok(deleted)
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}
