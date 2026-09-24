use crate::Result;
use crate::agent_store::SqliteAgentStore;
use crate::error::StoreError;
use cas_types::{LeaseStatus, WorktreeClaimResult, WorktreeLease};
use chrono::Utc;
use rusqlite::{OptionalExtension, params};

impl SqliteAgentStore {
    pub(crate) fn worktree_try_claim_worktree(
        &self,
        worktree_id: &str,
        agent_id: &str,
        duration_secs: i64,
    ) -> Result<WorktreeClaimResult> {
        let conn = self.lock_conn()?;
        let tx = crate::shared_db::begin_immediate_with_retry(&conn)?;
        let now = Utc::now();
        let expires_at = now + chrono::Duration::seconds(duration_secs);

        // Check if worktree is already claimed with an active, non-expired lease
        let existing: Option<(String, String, String)> = tx
            .query_row(
                "SELECT agent_id, status, expires_at FROM worktree_leases WHERE worktree_id = ?",
                params![worktree_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;

        if let Some((held_by, status, expires_str)) = existing {
            if status == "active" {
                if let Some(expires) = Self::parse_datetime(&expires_str) {
                    if expires > now {
                        if held_by == agent_id {
                            // Same agent - renew the lease
                            tx.execute(
                                "UPDATE worktree_leases SET expires_at = ?, renewed_at = ?, renewal_count = renewal_count + 1
                                 WHERE worktree_id = ?",
                                params![expires_at.to_rfc3339(), now.to_rfc3339(), worktree_id],
                            )?;

                            let lease = WorktreeLease {
                                worktree_id: worktree_id.to_string(),
                                agent_id: agent_id.to_string(),
                                status: LeaseStatus::Active,
                                acquired_at: now, // Note: should ideally preserve original acquired_at
                                expires_at,
                                renewed_at: now,
                                renewal_count: 1, // Incremented
                            };
                            tx.commit()?;
                            return Ok(WorktreeClaimResult::Success(lease));
                        }
                        // Different agent holds active lease
                        // No commit needed - tx rolls back (read-only path)
                        return Ok(WorktreeClaimResult::AlreadyClaimed {
                            worktree_id: worktree_id.to_string(),
                            held_by,
                            expires_at: expires,
                        });
                    }
                }
            }
            // Lease exists but is expired/released/revoked - update it
            tx.execute(
                "UPDATE worktree_leases SET agent_id = ?, status = 'active', acquired_at = ?,
                 expires_at = ?, renewed_at = ?, renewal_count = 0
                 WHERE worktree_id = ?",
                params![
                    agent_id,
                    now.to_rfc3339(),
                    expires_at.to_rfc3339(),
                    now.to_rfc3339(),
                    worktree_id,
                ],
            )?;
        } else {
            // No existing lease - create new one
            tx.execute(
                "INSERT INTO worktree_leases (worktree_id, agent_id, status, acquired_at, expires_at, renewed_at, renewal_count)
                 VALUES (?, ?, 'active', ?, ?, ?, 0)",
                params![
                    worktree_id,
                    agent_id,
                    now.to_rfc3339(),
                    expires_at.to_rfc3339(),
                    now.to_rfc3339(),
                ],
            )?;
        }

        let lease = WorktreeLease {
            worktree_id: worktree_id.to_string(),
            agent_id: agent_id.to_string(),
            status: LeaseStatus::Active,
            acquired_at: now,
            expires_at,
            renewed_at: now,
            renewal_count: 0,
        };

        tx.commit()?;
        Ok(WorktreeClaimResult::Success(lease))
    }
    pub(crate) fn worktree_release_worktree_lease(
        &self,
        worktree_id: &str,
        agent_id: &str,
    ) -> Result<()> {
        let conn = self.lock_conn()?;

        // Verify the agent owns this lease
        let owner: Option<String> = conn
            .query_row(
                "SELECT agent_id FROM worktree_leases WHERE worktree_id = ? AND status = 'active'",
                params![worktree_id],
                |row| row.get(0),
            )
            .optional()?;

        match owner {
            Some(owner_id) if owner_id == agent_id => {
                conn.execute(
                    "UPDATE worktree_leases SET status = 'released' WHERE worktree_id = ?",
                    params![worktree_id],
                )?;
                Ok(())
            }
            Some(_) => Err(StoreError::Parse(format!(
                "Worktree {worktree_id} is not owned by agent {agent_id}"
            ))),
            None => Err(StoreError::NotFound(format!(
                "No active lease found for worktree {worktree_id}"
            ))),
        }
    }
    pub(crate) fn worktree_renew_worktree_lease(
        &self,
        worktree_id: &str,
        agent_id: &str,
        duration_secs: i64,
    ) -> Result<()> {
        let conn = self.lock_conn()?;
        let now = Utc::now();
        let expires_at = now + chrono::Duration::seconds(duration_secs);

        let rows = conn.execute(
            "UPDATE worktree_leases SET expires_at = ?, renewed_at = ?, renewal_count = renewal_count + 1
             WHERE worktree_id = ? AND agent_id = ? AND status = 'active'",
            params![expires_at.to_rfc3339(), now.to_rfc3339(), worktree_id, agent_id],
        )?;

        if rows == 0 {
            return Err(StoreError::NotFound(format!(
                "No active lease found for worktree {worktree_id} owned by {agent_id}"
            )));
        }
        Ok(())
    }
    pub(crate) fn worktree_get_worktree_lease(
        &self,
        worktree_id: &str,
    ) -> Result<Option<WorktreeLease>> {
        let conn = self.lock_conn()?;
        let result = conn.query_row(
            "SELECT worktree_id, agent_id, status, acquired_at, expires_at, renewed_at, renewal_count
             FROM worktree_leases WHERE worktree_id = ? AND status = 'active'",
            params![worktree_id],
            Self::worktree_lease_from_row,
        )
        .optional()?;
        Ok(result)
    }
    pub(crate) fn worktree_get_worktree_lease_for_epic(
        &self,
        epic_id: &str,
    ) -> Result<Option<WorktreeLease>> {
        let conn = self.lock_conn()?;
        // Join with worktrees table to find the worktree for this epic, then get its lease
        let result = conn.query_row(
            "SELECT wl.worktree_id, wl.agent_id, wl.status, wl.acquired_at, wl.expires_at, wl.renewed_at, wl.renewal_count
             FROM worktree_leases wl
             JOIN worktrees w ON wl.worktree_id = w.id
             WHERE w.epic_id = ? AND wl.status = 'active' AND w.status = 'active'",
            params![epic_id],
            Self::worktree_lease_from_row,
        )
        .optional()?;
        Ok(result)
    }
    pub(crate) fn worktree_list_agent_worktree_leases(
        &self,
        agent_id: &str,
    ) -> Result<Vec<WorktreeLease>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT worktree_id, agent_id, status, acquired_at, expires_at, renewed_at, renewal_count
             FROM worktree_leases WHERE agent_id = ? AND status = 'active'
             ORDER BY acquired_at DESC",
        )?;

        let leases = stmt
            .query_map(params![agent_id], Self::worktree_lease_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(leases)
    }
    pub(crate) fn worktree_list_active_worktree_leases(&self) -> Result<Vec<WorktreeLease>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT worktree_id, agent_id, status, acquired_at, expires_at, renewed_at, renewal_count
             FROM worktree_leases WHERE status = 'active'
             ORDER BY expires_at ASC",
        )?;

        let leases = stmt
            .query_map([], Self::worktree_lease_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(leases)
    }
    pub(crate) fn worktree_reclaim_expired_worktree_leases(&self) -> Result<usize> {
        let conn = self.lock_conn()?;
        let now = Utc::now().to_rfc3339();

        let count = conn.execute(
            "UPDATE worktree_leases SET status = 'expired'
             WHERE status = 'active' AND expires_at < ?",
            params![now],
        )?;

        Ok(count)
    }
    pub(crate) fn worktree_can_agent_work_on_epic(
        &self,
        agent_id: &str,
        epic_id: &str,
    ) -> Result<bool> {
        // Check if there's a worktree for this epic
        let conn = self.lock_conn()?;

        let worktree_id: Option<String> = conn
            .query_row(
                "SELECT id FROM worktrees WHERE epic_id = ? AND status = 'active'",
                params![epic_id],
                |row| row.get(0),
            )
            .optional()?;

        let Some(wt_id) = worktree_id else {
            // No worktree for this epic - anyone can work on it
            return Ok(true);
        };

        // Check if there's an active lease on this worktree
        let lease_info: Option<(String, String)> = conn
            .query_row(
                "SELECT agent_id, expires_at FROM worktree_leases WHERE worktree_id = ? AND status = 'active'",
                params![wt_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        match lease_info {
            None => Ok(true), // No lease - anyone can work
            Some((owner, expires_str)) => {
                // Check if expired
                if let Some(expires) = Self::parse_datetime(&expires_str) {
                    if expires <= Utc::now() {
                        return Ok(true); // Expired - anyone can work
                    }
                }
                // Active lease exists - only the owner can work
                Ok(owner == agent_id)
            }
        }
    }
    pub(crate) fn worktree_close(&self) -> Result<()> {
        Ok(())
    }
}
