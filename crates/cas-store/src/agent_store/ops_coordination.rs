use crate::Result;
use crate::agent_store::SqliteAgentStore;
use crate::shared_db::ImmediateTx;
use cas_types::Agent;
use chrono::Utc;
use rusqlite::params;

impl SqliteAgentStore {
    pub(crate) fn coord_get_active_children(&self, agent_id: &str) -> Result<Vec<Agent>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, name, agent_type, role, status, pid, ppid, cc_session_id, parent_id,
             machine_id, registered_at, last_heartbeat, active_tasks, metadata, pid_starttime, factory_session
             FROM agents WHERE parent_id = ? AND status = 'active'
             ORDER BY registered_at DESC",
        )?;

        let agents = stmt
            .query_map(params![agent_id], Self::agent_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(agents)
    }
    pub(crate) fn coord_graceful_shutdown(&self, agent_id: &str) -> Result<Vec<String>> {
        let conn = self.lock_conn()?;
        let tx = ImmediateTx::new(&conn)?;

        // Get all active task leases for this agent
        let mut stmt = tx.prepare_cached(
            "SELECT task_id, epoch FROM task_leases WHERE agent_id = ? AND status = 'active'",
        )?;
        let leases: Vec<(String, i64)> = stmt
            .query_map(params![agent_id], |row| {
                Ok((row.get(0)?, row.get::<_, i64>(1).unwrap_or(1)))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);

        let task_ids: Vec<String> = leases.iter().map(|(id, _)| id.clone()).collect();

        // Release all active task leases
        tx.execute(
            "UPDATE task_leases SET status = 'released' WHERE agent_id = ? AND status = 'active'",
            params![agent_id],
        )?;

        // Release all active worktree leases
        tx.execute(
            "UPDATE worktree_leases SET status = 'released' WHERE agent_id = ? AND status = 'active'",
            params![agent_id],
        )?;

        // Log released events for each task lease
        for (task_id, epoch) in &leases {
            Self::log_lease_event(
                &tx,
                task_id,
                agent_id,
                "released",
                *epoch as u64,
                None,
                None,
                Some("graceful_shutdown"),
            )?;
        }

        // Mark agent as shutdown
        tx.execute(
            "UPDATE agents SET status = 'shutdown', active_tasks = 0 WHERE id = ?",
            params![agent_id],
        )?;

        tx.commit()?;
        Ok(task_ids)
    }
    pub(crate) fn coord_add_working_epic(&self, agent_id: &str, epic_id: &str) -> Result<()> {
        let conn = self.lock_conn()?;
        let now = Utc::now().to_rfc3339();

        // INSERT OR IGNORE - don't fail if already exists
        conn.execute(
            "INSERT OR IGNORE INTO working_epics (agent_id, epic_id, started_at) VALUES (?, ?, ?)",
            params![agent_id, epic_id, now],
        )?;

        Ok(())
    }
    pub(crate) fn coord_get_working_epics(&self, agent_id: &str) -> Result<Vec<String>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT epic_id FROM working_epics WHERE agent_id = ? ORDER BY started_at DESC",
        )?;

        let epic_ids = stmt
            .query_map(params![agent_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(epic_ids)
    }
    pub(crate) fn coord_list_all_working_epics(&self) -> Result<Vec<String>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT epic_id FROM working_epics ORDER BY started_at DESC",
        )?;

        let epic_ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(epic_ids)
    }
    pub(crate) fn coord_list_orphaned_working_epics(&self) -> Result<Vec<String>> {
        let conn = self.lock_conn()?;
        // Only return epics from agents that are NOT active
        // This prevents blocking Agent B when Agent A has an active epic
        let mut stmt = conn.prepare_cached(
            "SELECT DISTINCT w.epic_id
             FROM working_epics w
             LEFT JOIN agents a ON w.agent_id = a.id
             WHERE a.id IS NULL OR a.status != 'active'
             ORDER BY w.started_at DESC",
        )?;

        let epic_ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(epic_ids)
    }
    pub(crate) fn coord_remove_working_epic(&self, agent_id: &str, epic_id: &str) -> Result<()> {
        let conn = self.lock_conn()?;
        conn.execute(
            "DELETE FROM working_epics WHERE agent_id = ? AND epic_id = ?",
            params![agent_id, epic_id],
        )?;
        Ok(())
    }
    pub(crate) fn coord_clear_working_epics(&self, agent_id: &str) -> Result<()> {
        let conn = self.lock_conn()?;
        conn.execute(
            "DELETE FROM working_epics WHERE agent_id = ?",
            params![agent_id],
        )?;
        Ok(())
    }
    pub(crate) fn coord_register_daemon(&self, daemon_id: &str, daemon_type: &str) -> Result<()> {
        let conn = self.lock_conn()?;
        let now = Utc::now().to_rfc3339();
        let pid = std::process::id() as i64;

        conn.execute(
            "INSERT OR REPLACE INTO daemon_instances (id, pid, daemon_type, started_at, last_heartbeat, status)
             VALUES (?, ?, ?, ?, ?, 'running')",
            params![daemon_id, pid, daemon_type, now, now],
        )?;

        Ok(())
    }
    pub(crate) fn coord_daemon_heartbeat(&self, daemon_id: &str) -> Result<()> {
        let conn = self.lock_conn()?;
        let now = Utc::now().to_rfc3339();

        conn.execute(
            "UPDATE daemon_instances SET last_heartbeat = ? WHERE id = ?",
            params![now, daemon_id],
        )?;

        Ok(())
    }
    pub(crate) fn coord_unregister_daemon(&self, daemon_id: &str) -> Result<()> {
        let conn = self.lock_conn()?;

        conn.execute(
            "DELETE FROM daemon_instances WHERE id = ?",
            params![daemon_id],
        )?;

        Ok(())
    }
    pub(crate) fn coord_is_daemon_active(&self, threshold_secs: i64) -> Result<bool> {
        let conn = self.lock_conn()?;
        let cutoff = (Utc::now() - chrono::Duration::seconds(threshold_secs)).to_rfc3339();

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM daemon_instances WHERE last_heartbeat > ? AND status = 'running'",
            params![cutoff],
            |row| row.get(0),
        )?;

        Ok(count > 0)
    }

    // Worktree lease operations
}
