use crate::Result;
use crate::agent_store::{LeaseHistoryEntry, SqliteAgentStore};
use crate::error::StoreError;
use crate::shared_db::ImmediateTx;
use cas_types::{ClaimResult, LeaseStatus, TaskLease};
use chrono::Utc;
use rusqlite::{OptionalExtension, params};

impl SqliteAgentStore {
    pub(crate) fn lease_try_claim(
        &self,
        task_id: &str,
        agent_id: &str,
        duration_secs: i64,
        reason: Option<&str>,
    ) -> Result<ClaimResult> {
        crate::shared_db::with_write_retry(|| {
        let conn = self.lock_conn()?;
        let tx = ImmediateTx::new(&conn)?;
        let now = Utc::now();
        let expires_at = now + chrono::Duration::seconds(duration_secs);

        // Check if task is already claimed with an active, non-expired lease
        let existing: Option<(String, String, String, i64)> = tx
            .query_row(
                "SELECT agent_id, status, expires_at, epoch FROM task_leases WHERE task_id = ?",
                params![task_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get::<_, i64>(3).unwrap_or(1),
                    ))
                },
            )
            .optional()?;

        let new_epoch: u64;
        if let Some((held_by, status, expires_str, current_epoch)) = existing {
            if status == "active" {
                if let Some(expires) = Self::parse_datetime(&expires_str) {
                    if expires > now {
                        // Lease is still valid - check if worker can take over from supervisor
                        let can_takeover = if held_by != agent_id {
                            // Check if requesting agent's parent is the current holder
                            let parent: Option<Option<String>> = tx
                                .query_row(
                                    "SELECT parent_id FROM agents WHERE id = ?",
                                    params![agent_id],
                                    |row| row.get(0),
                                )
                                .optional()?;
                            matches!(parent, Some(Some(ref pid)) if pid == &held_by)
                        } else {
                            false
                        };

                        if can_takeover {
                            // Worker taking over from supervisor - release supervisor's lease
                            tx.execute(
                                "UPDATE agents SET active_tasks = MAX(0, active_tasks - 1) WHERE id = ?",
                                params![&held_by],
                            )?;
                            // Log transfer event
                            Self::log_lease_event(
                                &tx,
                                task_id,
                                &held_by,
                                "transferred",
                                current_epoch as u64,
                                Some(&format!(
                                    r#"{{"to_agent":"{agent_id}","reason":"worker_takeover"}}"#
                                )),
                                Some(agent_id),
                                None,
                            )?;
                            // Fall through to update lease for worker
                        } else {
                            // Not a supervisor takeover - cannot claim
                            return Ok(ClaimResult::AlreadyClaimed {
                                task_id: task_id.to_string(),
                                held_by,
                                expires_at: expires,
                            });
                        }
                    }
                }
            }
            // Lease exists but is expired/released/revoked - update it with incremented epoch
            new_epoch = (current_epoch as u64) + 1;
            tx.execute(
                "UPDATE task_leases SET agent_id = ?, status = 'active', acquired_at = ?,
                 expires_at = ?, renewed_at = ?, renewal_count = 0, epoch = ?, claim_reason = ?
                 WHERE task_id = ?",
                params![
                    agent_id,
                    now.to_rfc3339(),
                    expires_at.to_rfc3339(),
                    now.to_rfc3339(),
                    new_epoch as i64,
                    reason,
                    task_id,
                ],
            )?;
        } else {
            // No existing lease - create new one with epoch 1
            new_epoch = 1;
            tx.execute(
                "INSERT INTO task_leases (task_id, agent_id, status, acquired_at, expires_at,
                 renewed_at, renewal_count, epoch, claim_reason)
                 VALUES (?, ?, 'active', ?, ?, ?, 0, 1, ?)",
                params![
                    task_id,
                    agent_id,
                    now.to_rfc3339(),
                    expires_at.to_rfc3339(),
                    now.to_rfc3339(),
                    reason,
                ],
            )?;
        }

        // Update agent's active task count
        tx.execute(
            "UPDATE agents SET active_tasks = active_tasks + 1 WHERE id = ?",
            params![agent_id],
        )?;

        let lease = TaskLease {
            task_id: task_id.to_string(),
            agent_id: agent_id.to_string(),
            status: LeaseStatus::Active,
            acquired_at: now,
            expires_at,
            renewed_at: now,
            renewal_count: 0,
            epoch: new_epoch,
            claim_reason: reason.map(|s| s.to_string()),
        };

        // Log the claim event
        let details = reason.map(|r| format!(r#"{{"reason":"{r}"}}"#));
        Self::log_lease_event(
            &tx,
            task_id,
            agent_id,
            "claimed",
            new_epoch,
            details.as_deref(),
            None,
            None,
        )?;

        tx.commit()?;
        Ok(ClaimResult::Success(lease))
        }) // with_write_retry
    }
    pub(crate) fn lease_release_lease(&self, task_id: &str, agent_id: &str) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
        let conn = self.lock_conn()?;
        let tx = ImmediateTx::new(&conn)?;

        // Verify the agent owns this lease and get epoch
        let lease_info: Option<(String, i64)> = tx
            .query_row(
                "SELECT agent_id, epoch FROM task_leases WHERE task_id = ? AND status = 'active'",
                params![task_id],
                |row| Ok((row.get(0)?, row.get::<_, i64>(1).unwrap_or(1))),
            )
            .optional()?;

        match lease_info {
            Some((owner_id, epoch)) if owner_id == agent_id => {
                // Release the lease
                tx.execute(
                    "UPDATE task_leases SET status = 'released' WHERE task_id = ?",
                    params![task_id],
                )?;

                // Decrement agent's active task count
                tx.execute(
                    "UPDATE agents SET active_tasks = MAX(0, active_tasks - 1) WHERE id = ?",
                    params![agent_id],
                )?;

                // Log the release event
                Self::log_lease_event(
                    &tx,
                    task_id,
                    agent_id,
                    "released",
                    epoch as u64,
                    None,
                    None,
                    None,
                )?;

                tx.commit()?;
                Ok(())
            }
            Some(_) => Err(StoreError::Parse(format!(
                "Task {task_id} is not owned by agent {agent_id}"
            ))),
            None => Err(StoreError::NotFound(format!(
                "No active lease found for task {task_id}"
            ))),
        }
        }) // with_write_retry
    }
    pub(crate) fn lease_release_lease_for_task(&self, task_id: &str, reason: &str) -> Result<bool> {
        crate::shared_db::with_write_retry(|| {
        let conn = self.lock_conn()?;
        let tx = ImmediateTx::new(&conn)?;

        // Get lease info if it exists
        let lease_info: Option<(String, i64)> = tx
            .query_row(
                "SELECT agent_id, epoch FROM task_leases WHERE task_id = ? AND status = 'active'",
                params![task_id],
                |row| Ok((row.get(0)?, row.get::<_, i64>(1).unwrap_or(1))),
            )
            .optional()?;

        if let Some((agent_id, epoch)) = lease_info {
            // Release the lease
            tx.execute(
                "UPDATE task_leases SET status = 'released' WHERE task_id = ?",
                params![task_id],
            )?;

            // Decrement agent's active task count
            tx.execute(
                "UPDATE agents SET active_tasks = MAX(0, active_tasks - 1) WHERE id = ?",
                params![agent_id],
            )?;

            // Log the release event
            Self::log_lease_event(
                &tx,
                task_id,
                &agent_id,
                "released",
                epoch as u64,
                None,
                None,
                Some(reason),
            )?;

            tx.commit()?;
            Ok(true)
        } else {
            // No active lease to release
            Ok(false)
        }
        }) // with_write_retry
    }
    pub(crate) fn lease_renew_lease(
        &self,
        task_id: &str,
        agent_id: &str,
        duration_secs: i64,
    ) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
        let conn = self.lock_conn()?;
        let now = Utc::now();
        let expires_at = now + chrono::Duration::seconds(duration_secs);

        // Get current epoch before renewing
        let epoch: Option<i64> = conn
            .query_row(
                "SELECT epoch FROM task_leases WHERE task_id = ? AND agent_id = ? AND status = 'active'",
                params![task_id, agent_id],
                |row| row.get(0),
            )
            .optional()?;

        let rows = conn.execute(
            "UPDATE task_leases SET expires_at = ?, renewed_at = ?, renewal_count = renewal_count + 1
             WHERE task_id = ? AND agent_id = ? AND status = 'active'",
            params![expires_at.to_rfc3339(), now.to_rfc3339(), task_id, agent_id],
        )?;

        if rows == 0 {
            return Err(StoreError::NotFound(format!(
                "No active lease found for task {task_id} owned by {agent_id}"
            )));
        }

        // Log the renewal event
        if let Some(ep) = epoch {
            let details = format!(r#"{{"duration_secs":{duration_secs}}}"#);
            Self::log_lease_event(
                &conn,
                task_id,
                agent_id,
                "renewed",
                ep as u64,
                Some(&details),
                None,
                None,
            )?;
        }

        Ok(())
        }) // with_write_retry
    }
    pub(crate) fn lease_get_lease(&self, task_id: &str) -> Result<Option<TaskLease>> {
        let conn = self.lock_conn()?;
        let result = conn
            .query_row(
                "SELECT task_id, agent_id, status, acquired_at, expires_at, renewed_at,
             renewal_count, epoch, claim_reason
             FROM task_leases WHERE task_id = ? AND status = 'active'",
                params![task_id],
                Self::lease_from_row,
            )
            .optional()?;
        Ok(result)
    }
    pub(crate) fn lease_list_agent_leases(&self, agent_id: &str) -> Result<Vec<TaskLease>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT task_id, agent_id, status, acquired_at, expires_at, renewed_at,
             renewal_count, epoch, claim_reason
             FROM task_leases WHERE agent_id = ? AND status = 'active'
             ORDER BY acquired_at DESC",
        )?;

        let leases = stmt
            .query_map(params![agent_id], Self::lease_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(leases)
    }
    pub(crate) fn lease_list_active_leases(&self) -> Result<Vec<TaskLease>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT task_id, agent_id, status, acquired_at, expires_at, renewed_at,
             renewal_count, epoch, claim_reason
             FROM task_leases WHERE status = 'active'
             ORDER BY expires_at ASC",
        )?;

        let leases = stmt
            .query_map([], Self::lease_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(leases)
    }
    pub(crate) fn lease_reclaim_expired_leases(&self) -> Result<usize> {
        crate::shared_db::with_write_retry(|| {
        let conn = self.lock_conn()?;
        let now = Utc::now().to_rfc3339();

        // Find expired leases with their agents and epochs
        let mut stmt = conn.prepare_cached(
            "SELECT task_id, agent_id, epoch FROM task_leases
             WHERE status = 'active' AND expires_at < ?",
        )?;
        let expired: Vec<(String, String, i64)> = stmt
            .query_map(params![now], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2).unwrap_or(1)))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);

        let count = expired.len();

        // Mark leases as expired
        conn.execute(
            "UPDATE task_leases SET status = 'expired'
             WHERE status = 'active' AND expires_at < ?",
            params![now],
        )?;

        // Batch decrement active task counts: count expirations per agent
        // and apply in a single UPDATE instead of N separate UPDATEs.
        {
            let mut agent_counts: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
            for (_, agent_id, _) in &expired {
                *agent_counts.entry(agent_id.as_str()).or_insert(0) += 1;
            }
            for (agent_id, decrement) in &agent_counts {
                conn.execute(
                    "UPDATE agents SET active_tasks = MAX(0, active_tasks - ?1) WHERE id = ?2",
                    params![decrement, agent_id],
                )?;
            }
        }

        // Log expired events (still per-lease for accurate history)
        for (task_id, agent_id, epoch) in &expired {
            Self::log_lease_event(
                &conn,
                task_id,
                agent_id,
                "expired",
                *epoch as u64,
                None,
                None,
                None,
            )?;
        }

        Ok(count)
        }) // with_write_retry
    }
    pub(crate) fn lease_get_lease_history(
        &self,
        task_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<LeaseHistoryEntry>> {
        let conn = self.lock_conn()?;
        let limit_val = limit.unwrap_or(100) as i64;

        let mut stmt = conn.prepare_cached(
            "SELECT id, task_id, agent_id, event_type, epoch, timestamp, details, previous_agent_id, reason
             FROM task_lease_history
             WHERE task_id = ?
             ORDER BY timestamp DESC
             LIMIT ?",
        )?;

        let entries = stmt
            .query_map(params![task_id, limit_val], |row| {
                let timestamp_str: String = row.get(5)?;
                let timestamp = Self::parse_datetime(&timestamp_str).unwrap_or_else(Utc::now);
                let event_type: String = row.get(3)?;
                let previous_agent_id: Option<String> = row.get(7)?;
                let reason: Option<String> = row.get(8)?;
                let reason = reason.or_else(|| {
                    (event_type == "released")
                        .then(|| previous_agent_id.clone())
                        .flatten()
                });
                Ok(LeaseHistoryEntry {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    agent_id: row.get(2)?,
                    event_type,
                    epoch: row.get::<_, i64>(4).unwrap_or(1) as u64,
                    timestamp,
                    details: row.get(6)?,
                    previous_agent_id,
                    reason,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }
    pub(crate) fn lease_get_agent_worked_tasks(
        &self,
        agent_id: &str,
        since: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Vec<String>> {
        let conn = self.lock_conn()?;

        // Get unique task_ids from lease history for this agent
        // Only include tasks where the agent 'claimed' them (not just received via transfer)
        let task_ids: Vec<String> = if let Some(since_time) = since {
            let since_str = since_time.to_rfc3339();
            let mut stmt = conn.prepare_cached(
                "SELECT DISTINCT task_id FROM task_lease_history
                 WHERE agent_id = ? AND event_type = 'claimed' AND timestamp >= ?
                 ORDER BY timestamp DESC",
            )?;
            let rows =
                stmt.query_map(params![agent_id, since_str], |row| row.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            let mut stmt = conn.prepare_cached(
                "SELECT DISTINCT task_id FROM task_lease_history
                 WHERE agent_id = ? AND event_type = 'claimed'
                 ORDER BY timestamp DESC",
            )?;
            let rows = stmt.query_map(params![agent_id], |row| row.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        Ok(task_ids)
    }

    pub(crate) fn lease_cleanup_lease_history(&self, older_than_days: i64) -> Result<usize> {
        crate::shared_db::with_write_retry(|| {
        let conn = self.lock_conn()?;
        let cutoff = (Utc::now() - chrono::Duration::days(older_than_days)).to_rfc3339();

        let rows = conn.execute(
            "DELETE FROM task_lease_history WHERE timestamp < ?",
            params![cutoff],
        )?;

        Ok(rows)
        }) // with_write_retry
    }
}
