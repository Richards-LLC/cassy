use crate::Result;
use crate::agent_store::{AgentStore, LeaseHistoryEntry, SqliteAgentStore};
use cas_types::{Agent, AgentStatus, ClaimResult, TaskLease, WorktreeClaimResult, WorktreeLease};

impl AgentStore for SqliteAgentStore {
    fn init(&self) -> Result<()> {
        self.agent_init()
    }
    fn register(&self, agent: &Agent) -> Result<()> {
        self.agent_register(agent)
    }
    fn get(&self, id: &str) -> Result<Agent> {
        self.agent_get(id)
    }
    fn update(&self, agent: &Agent) -> Result<()> {
        self.agent_update(agent)
    }
    fn unregister(&self, id: &str) -> Result<()> {
        self.agent_unregister(id)
    }
    fn list(&self, status: Option<AgentStatus>) -> Result<Vec<Agent>> {
        self.agent_list(status)
    }
    fn list_stale(&self, timeout_secs: i64) -> Result<Vec<Agent>> {
        self.agent_list_stale(timeout_secs)
    }
    fn list_failed_startup(&self, timeout_secs: i64) -> Result<Vec<Agent>> {
        self.agent_list_failed_startup(timeout_secs)
    }
    fn heartbeat(&self, id: &str) -> Result<()> {
        self.agent_heartbeat(id)
    }
    fn mark_stale(&self, id: &str) -> Result<()> {
        self.agent_mark_stale(id)
    }
    fn revive(&self, id: &str) -> Result<()> {
        self.agent_revive(id)
    }
    fn get_by_cc_pid(&self, cc_pid: u32) -> Result<Option<Agent>> {
        self.agent_get_by_cc_pid(cc_pid)
    }
    fn get_by_pid(&self, pid: u32) -> Result<Option<Agent>> {
        self.agent_get_by_pid(pid)
    }

    fn try_claim(
        &self,
        task_id: &str,
        agent_id: &str,
        duration_secs: i64,
        reason: Option<&str>,
    ) -> Result<ClaimResult> {
        self.lease_try_claim(task_id, agent_id, duration_secs, reason)
    }
    fn release_lease(&self, task_id: &str, agent_id: &str) -> Result<()> {
        self.lease_release_lease(task_id, agent_id)
    }
    fn release_lease_for_task(&self, task_id: &str, reason: &str) -> Result<bool> {
        self.lease_release_lease_for_task(task_id, reason)
    }
    fn release_lease_if_owner_epoch(
        &self,
        task_id: &str,
        agent_id: &str,
        epoch: u64,
        reason: &str,
    ) -> Result<bool> {
        self.lease_release_lease_if_owner_epoch(task_id, agent_id, epoch, reason)
    }
    fn renew_lease(&self, task_id: &str, agent_id: &str, duration_secs: i64) -> Result<()> {
        self.lease_renew_lease(task_id, agent_id, duration_secs)
    }
    fn get_lease(&self, task_id: &str) -> Result<Option<TaskLease>> {
        self.lease_get_lease(task_id)
    }
    fn list_agent_leases(&self, agent_id: &str) -> Result<Vec<TaskLease>> {
        self.lease_list_agent_leases(agent_id)
    }
    fn list_active_leases(&self) -> Result<Vec<TaskLease>> {
        self.lease_list_active_leases()
    }
    fn reclaim_expired_leases(&self) -> Result<usize> {
        self.lease_reclaim_expired_leases()
    }
    fn cleanup_lease_history(&self, older_than_days: i64) -> Result<usize> {
        self.lease_cleanup_lease_history(older_than_days)
    }
    fn get_lease_history(
        &self,
        task_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<LeaseHistoryEntry>> {
        self.lease_get_lease_history(task_id, limit)
    }
    fn get_agent_worked_tasks(
        &self,
        agent_id: &str,
        since: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Vec<String>> {
        self.lease_get_agent_worked_tasks(agent_id, since)
    }

    fn get_active_children(&self, agent_id: &str) -> Result<Vec<Agent>> {
        self.coord_get_active_children(agent_id)
    }
    fn graceful_shutdown(&self, agent_id: &str) -> Result<Vec<String>> {
        self.coord_graceful_shutdown(agent_id)
    }
    fn add_working_epic(&self, agent_id: &str, epic_id: &str) -> Result<()> {
        self.coord_add_working_epic(agent_id, epic_id)
    }
    fn get_working_epics(&self, agent_id: &str) -> Result<Vec<String>> {
        self.coord_get_working_epics(agent_id)
    }
    fn list_all_working_epics(&self) -> Result<Vec<String>> {
        self.coord_list_all_working_epics()
    }
    fn list_orphaned_working_epics(&self) -> Result<Vec<String>> {
        self.coord_list_orphaned_working_epics()
    }
    fn remove_working_epic(&self, agent_id: &str, epic_id: &str) -> Result<()> {
        self.coord_remove_working_epic(agent_id, epic_id)
    }
    fn clear_working_epics(&self, agent_id: &str) -> Result<()> {
        self.coord_clear_working_epics(agent_id)
    }
    fn register_daemon(&self, daemon_id: &str, daemon_type: &str) -> Result<()> {
        self.coord_register_daemon(daemon_id, daemon_type)
    }
    fn daemon_heartbeat(&self, daemon_id: &str) -> Result<()> {
        self.coord_daemon_heartbeat(daemon_id)
    }
    fn unregister_daemon(&self, daemon_id: &str) -> Result<()> {
        self.coord_unregister_daemon(daemon_id)
    }
    fn is_daemon_active(&self, threshold_secs: i64) -> Result<bool> {
        self.coord_is_daemon_active(threshold_secs)
    }

    fn try_claim_worktree(
        &self,
        worktree_id: &str,
        agent_id: &str,
        duration_secs: i64,
    ) -> Result<WorktreeClaimResult> {
        self.worktree_try_claim_worktree(worktree_id, agent_id, duration_secs)
    }
    fn release_worktree_lease(&self, worktree_id: &str, agent_id: &str) -> Result<()> {
        self.worktree_release_worktree_lease(worktree_id, agent_id)
    }
    fn renew_worktree_lease(
        &self,
        worktree_id: &str,
        agent_id: &str,
        duration_secs: i64,
    ) -> Result<()> {
        self.worktree_renew_worktree_lease(worktree_id, agent_id, duration_secs)
    }
    fn get_worktree_lease(&self, worktree_id: &str) -> Result<Option<WorktreeLease>> {
        self.worktree_get_worktree_lease(worktree_id)
    }
    fn get_worktree_lease_for_epic(&self, epic_id: &str) -> Result<Option<WorktreeLease>> {
        self.worktree_get_worktree_lease_for_epic(epic_id)
    }
    fn list_agent_worktree_leases(&self, agent_id: &str) -> Result<Vec<WorktreeLease>> {
        self.worktree_list_agent_worktree_leases(agent_id)
    }
    fn list_active_worktree_leases(&self) -> Result<Vec<WorktreeLease>> {
        self.worktree_list_active_worktree_leases()
    }
    fn reclaim_expired_worktree_leases(&self) -> Result<usize> {
        self.worktree_reclaim_expired_worktree_leases()
    }
    fn can_agent_work_on_epic(&self, agent_id: &str, epic_id: &str) -> Result<bool> {
        self.worktree_can_agent_work_on_epic(agent_id, epic_id)
    }

    fn close(&self) -> Result<()> {
        self.worktree_close()
    }
}
