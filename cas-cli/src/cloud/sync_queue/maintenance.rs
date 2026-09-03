use chrono::{Duration, Utc};
use rusqlite::params;

use crate::cloud::sync_queue::SyncQueue;
use crate::error::CasError;

fn parse_numeric_version(value: &str) -> Option<(u64, u64, u64)> {
    let core = value.trim().strip_prefix('v').unwrap_or(value.trim());
    let core = core.split(['-', '+']).next()?;
    let mut components = core.split('.');
    let major = components.next()?.parse().ok()?;
    let minor = components.next()?.parse().ok()?;
    let patch = components.next()?.parse().ok()?;
    if components.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn minimum_version_from_gate_error(error: &str) -> Option<(u64, u64, u64)> {
    let after_client = error.strip_prefix("Client version ").or_else(|| {
        error
            .find("Client version ")
            .map(|index| &error[index..])
            .and_then(|value| value.strip_prefix("Client version "))
    })?;
    let (client, minimum) = after_client.split_once(" is below minimum ")?;
    parse_numeric_version(client)?;
    parse_numeric_version(minimum.split_whitespace().next()?)
}

/// The build that is recording a queue verdict right now.
///
/// Stamping every failure with it is what makes "requeue once after an
/// upgrade" decidable: a row parked by an older build (or by a build old
/// enough to predate the column, leaving NULL) gets exactly one fresh attempt
/// under the new client, while a row this build already parked stays parked.
pub(crate) fn recording_client_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

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
            SET retry_count = retry_count + 1,
                last_error = ?2,
                failed_client_version = ?3
            WHERE id = ?1
            "#,
            params![id, error, recording_client_version()],
        )?;
        Ok(())
    }

    /// Persist the cloud's own per-row verdict for a queue row.
    ///
    /// The verdict is kept separately from `last_error` so reporting can group
    /// terminal rows by the reason the cloud gave (`project_mismatch`,
    /// `revision_conflict`, …) instead of string-matching a human diagnostic.
    /// Acknowledged rows are deleted by [`Self::mark_synced`] and never reach
    /// this path.
    pub fn record_row_outcome(
        &self,
        id: i64,
        outcome: &str,
        reason: Option<&str>,
    ) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sync_queue SET last_outcome = ?2, last_reason = ?3 WHERE id = ?1",
            params![id, outcome, reason],
        )?;
        Ok(())
    }

    /// Park a row that the cloud has conclusively rejected.
    ///
    /// Permanent ownership rejections cannot be repaired by retrying the
    /// exact same request.  Set the retry count directly to the configured
    /// terminal threshold so the next sync excludes it, while retaining the
    /// row and its operator-facing diagnostic for `cas cloud queue --verbose`.
    pub fn park_failed(&self, id: i64, error: &str, max_retries: i32) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sync_queue
             SET retry_count = MAX(retry_count, ?2),
                 last_error = ?3,
                 failed_client_version = ?4
             WHERE id = ?1",
            params![id, max_retries, error, recording_client_version()],
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

    /// Get the number of pending personal rows for one entity scope.
    pub fn pending_count_for_entity_type(
        &self,
        entity_type: Option<crate::cloud::EntityType>,
        max_retries: i32,
    ) -> Result<usize, CasError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            r#"
            SELECT COUNT(*) FROM sync_queue
            WHERE retry_count < ?1 AND (team_id IS NULL OR team_id = '')
              AND entity_type != 'knowledge_page'
              AND (?2 IS NULL OR entity_type = ?2)
            "#,
            params![max_retries, entity_type.map(|kind| kind.as_str())],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Get the number of failed personal rows for one entity scope.
    pub fn failed_count_for_entity_type(
        &self,
        entity_type: Option<crate::cloud::EntityType>,
        max_retries: i32,
    ) -> Result<usize, CasError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            r#"
            SELECT COUNT(*) FROM sync_queue
            WHERE retry_count >= ?1 AND (team_id IS NULL OR team_id = '')
              AND entity_type != 'knowledge_page'
              AND (?2 IS NULL OR entity_type = ?2)
            "#,
            params![max_retries, entity_type.map(|kind| kind.as_str())],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Group terminal personal rows by the reason the cloud rejected them.
    ///
    /// Rows without a per-row verdict (transport failures, payload errors, or
    /// rows parked by a client that predates the verdict columns) are absent:
    /// callers report those under the plain failure count so a rejection
    /// breakdown never overstates what the cloud actually said.
    pub fn rejected_reason_counts_for_entity_type(
        &self,
        entity_type: Option<crate::cloud::EntityType>,
        max_retries: i32,
    ) -> Result<std::collections::BTreeMap<String, usize>, CasError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT COALESCE(NULLIF(TRIM(last_reason), ''), 'unspecified') AS reason, COUNT(*)
            FROM sync_queue
            WHERE retry_count >= ?1 AND (team_id IS NULL OR team_id = '')
              AND entity_type != 'knowledge_page'
              AND (?2 IS NULL OR entity_type = ?2)
              AND last_outcome = 'rejected'
            GROUP BY reason
            "#,
        )?;
        let rows = stmt.query_map(
            params![max_retries, entity_type.map(|kind| kind.as_str())],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize)),
        )?;
        rows.collect::<Result<std::collections::BTreeMap<_, _>, _>>()
            .map_err(CasError::from)
    }

    /// Get the number of failed rows for one active team.
    pub fn failed_count_for_team(
        &self,
        team_id: &str,
        max_retries: i32,
    ) -> Result<usize, CasError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sync_queue WHERE retry_count >= ?1 AND team_id = ?2",
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

    /// Requeue terminal rows whose last diagnostic contains a repaired reason.
    /// Matching is case-insensitive and substring-based because push errors
    /// retain structured reason text inside a user-readable diagnostic.
    pub fn retry_failed_for_reason(
        &self,
        reason: &str,
        max_retries: i32,
    ) -> Result<usize, CasError> {
        let reason = reason.trim();
        if reason.is_empty() {
            return Ok(0);
        }

        let conn = self.conn.lock().unwrap();
        let reset = conn.execute(
            "UPDATE sync_queue
             SET retry_count = 0
             WHERE retry_count >= ?1
               AND last_error IS NOT NULL
               AND instr(lower(last_error), lower(?2)) > 0",
            params![max_retries, reason],
        )?;
        Ok(reset)
    }

    /// Requeue terminal failures caused by an older client once this build
    /// meets the server's recorded minimum version. The diagnostic is cleared
    /// with the retry counter so the operation is idempotent: a second push
    /// sees no matching gate error after the first reset.
    pub fn requeue_version_gated_failures(
        &self,
        current_version: &str,
        max_retries: i32,
    ) -> Result<usize, CasError> {
        let Some(current) = parse_numeric_version(current_version) else {
            return Ok(0);
        };

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let candidates = {
            let mut stmt = tx.prepare(
                r#"
                SELECT id, last_error
                FROM sync_queue
                WHERE retry_count >= ?1
                  AND last_error LIKE '%Client version % is below minimum %'
                "#,
            )?;
            stmt.query_map(params![max_retries], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?
        };

        let mut requeued = 0;
        for (id, error) in candidates {
            let Some(minimum) = minimum_version_from_gate_error(&error) else {
                continue;
            };
            if current < minimum {
                continue;
            }
            requeued += tx.execute(
                "UPDATE sync_queue SET retry_count = 0, last_error = NULL WHERE id = ?1 AND last_error = ?2",
                params![id, error],
            )?;
        }
        tx.commit()?;
        Ok(requeued)
    }

    /// Give terminal rows parked by an older client build exactly one fresh
    /// attempt after an upgrade.
    ///
    /// A row that only this build parked is left alone, and a row the cloud
    /// refused for a permanent reason (an ownership collision that no client
    /// version can repair) stays parked with its reason intact. Every requeued
    /// row is stamped with the current build, so the operation is idempotent
    /// within a version even if the row is never attempted again.
    pub fn requeue_stale_client_failures(
        &self,
        current_version: &str,
        max_retries: i32,
    ) -> Result<usize, CasError> {
        let Some(current) = parse_numeric_version(current_version) else {
            return Ok(0);
        };

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let candidates = {
            let mut stmt = tx.prepare(
                r#"
                SELECT id, failed_client_version, last_outcome, last_reason
                FROM sync_queue
                WHERE retry_count >= ?1
                "#,
            )?;
            stmt.query_map(params![max_retries], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
        };

        let mut requeued = 0;
        for (id, parked_by, outcome, reason) in candidates {
            if outcome.as_deref() == Some("rejected")
                && crate::cloud::syncer::push_reason_is_permanent(reason.as_deref().unwrap_or(""))
            {
                continue;
            }
            // A NULL stamp is a row parked before this client learned to record
            // the build, which is by definition older than the current one.
            if let Some(parked_by) = parked_by.as_deref() {
                match parse_numeric_version(parked_by) {
                    Some(parked) if parked >= current => continue,
                    // An unparseable stamp is not evidence of a newer build;
                    // treat it like an unknown older client and retry once.
                    _ => {}
                }
            }
            requeued += tx.execute(
                "UPDATE sync_queue SET retry_count = 0, failed_client_version = ?2 WHERE id = ?1",
                params![id, current_version],
            )?;
        }
        tx.commit()?;
        Ok(requeued)
    }

    /// Clear all items from the queue.
    pub fn clear(&self) -> Result<(), CasError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM sync_queue", [])?;
        Ok(())
    }
}
