use chrono::{Duration, Utc};
use rusqlite::params;

use crate::cloud::sync_queue::SyncQueue;
use crate::error::CasError;

impl SyncQueue {
    /// Mark an item as successfully synced (removes from queue).
    pub fn mark_synced(&self, id: i64) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM sync_queue WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Mark an item as failed (increments retry count, stores error).
    pub fn mark_failed(&self, id: i64, error: &str) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            UPDATE sync_queue
            SET retry_count = retry_count + 1, last_error = ?2
            WHERE id = ?1
            "#,
            params![id, error],
        )?;
        Ok(())
    }

    /// Preserve a server-side refusal on a still-retryable queue row.
    ///
    /// Unlike [`Self::mark_failed`], this deliberately does not increment the
    /// retry counter. A structured cloud `skipped` response is evidence that
    /// the server accepted the request but declined one or more rows; callers
    /// must retain those rows until the server can explain or resolve the
    /// conflict. Recording the diagnostic makes `cas cloud queue --verbose`
    /// useful without incorrectly parking the row as a transport failure.
    pub fn record_diagnostic(&self, id: i64, diagnostic: &str) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sync_queue SET last_error = ?2 WHERE id = ?1",
            params![id, diagnostic],
        )?;
        Ok(())
    }

    /// Get the number of items in the queue.
    pub fn queue_depth(&self) -> Result<usize, CasError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM sync_queue", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Get the number of pending items (under max retries).
    pub fn pending_count(&self, max_retries: i32) -> Result<usize, CasError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sync_queue WHERE retry_count < ?1",
            params![max_retries],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Get pending count for a specific team.
    pub fn pending_count_for_team(
        &self,
        team_id: &str,
        max_retries: i32,
    ) -> Result<usize, CasError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sync_queue WHERE retry_count < ?1 AND team_id = ?2",
            params![max_retries, team_id],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Clear failed items older than the specified number of days.
    pub fn prune_failed(&self, older_than_days: i64, max_retries: i32) -> Result<usize, CasError> {
        let conn = self.conn.lock().unwrap();
        let cutoff = Utc::now() - Duration::days(older_than_days);

        let deleted = conn.execute(
            r#"
            DELETE FROM sync_queue
            WHERE retry_count >= ?1 AND created_at < ?2
            "#,
            params![max_retries, cutoff.to_rfc3339()],
        )?;

        Ok(deleted)
    }

    /// Requeue every terminally failed row without discarding its last server
    /// diagnostic. This is the recovery path after an operator resolves a
    /// remote-side rejection (for example a project identity collision).
    pub fn retry_failed(&self, max_retries: i32) -> Result<usize, CasError> {
        let conn = self.conn.lock().unwrap();
        let reset = conn.execute(
            "UPDATE sync_queue SET retry_count = 0 WHERE retry_count >= ?1",
            params![max_retries],
        )?;
        Ok(reset)
    }

    /// Clear all items from the queue.
    pub fn clear(&self) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM sync_queue", [])?;
        Ok(())
    }
}
