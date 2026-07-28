//! SQLite-based agent and lease storage for multi-agent coordination
//!
//! This module provides storage for agent registration and task leasing,
//! enabling multiple Claude Code instances to coordinate work without conflicts.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, params, types::ValueRef};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::Result;
use crate::error::StoreError;
use cas_types::{
    Agent, AgentRole, AgentStatus, AgentType, ClaimResult, LeaseStatus, TaskLease,
    WorktreeClaimResult, WorktreeLease,
};
use serde::{Deserialize, Serialize};

/// A single entry in the lease history audit log
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseHistoryEntry {
    /// Unique ID of the history entry
    pub id: i64,
    /// Task ID the lease was for
    pub task_id: String,
    /// Agent ID that performed the action
    pub agent_id: String,
    /// Type of event: 'claimed', 'released', 'expired', 'transferred', 'renewed', 'revoked'
    pub event_type: String,
    /// Epoch number at time of event
    pub epoch: u64,
    /// When the event occurred
    pub timestamp: DateTime<Utc>,
    /// Optional JSON details about the event
    pub details: Option<String>,
    /// For transfers, the previous agent that held the lease
    pub previous_agent_id: Option<String>,
    /// Human-readable reason for the event
    pub reason: Option<String>,
}

/// Schema for agents and task leases tables.
///
/// Re-exported via `cas_store::AGENT_SCHEMA` so the migration runner in
/// `cas-cli` can bootstrap the base table before applying ALTER migrations
/// against subsystems whose tables were historically created lazily.
/// See cas-bdb9 / EPIC cas-9fdb.
pub const AGENT_SCHEMA: &str = r#"
-- Agents table: tracks registered Claude Code instances
-- Agent ID is now PPID-based (cc-{ppid}-{machine_hash}) for stability
CREATE TABLE IF NOT EXISTS agents (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    agent_type TEXT NOT NULL DEFAULT 'primary',
    role TEXT NOT NULL DEFAULT 'standard',
    status TEXT NOT NULL DEFAULT 'active',
    pid INTEGER,
    ppid INTEGER,
    cc_session_id TEXT,
    parent_id TEXT,
    machine_id TEXT,
    factory_session TEXT,
    registered_at TEXT NOT NULL,
    last_heartbeat TEXT NOT NULL,
    active_tasks INTEGER NOT NULL DEFAULT 0,
    metadata TEXT NOT NULL DEFAULT '{}',
    startup_confirmed INTEGER NOT NULL DEFAULT 0,
    -- PID-reuse fingerprint (Linux /proc/<pid>/stat field 22). Typed
    -- counterpart to metadata[PID_STARTTIME_KEY]; see cas-b157 +
    -- migration m200. Nullable because non-Linux hosts and pre-migration
    -- legacy rows have no value.
    pid_starttime INTEGER
);

CREATE INDEX IF NOT EXISTS idx_agents_status ON agents(status);
CREATE INDEX IF NOT EXISTS idx_agents_machine ON agents(machine_id);
CREATE INDEX IF NOT EXISTS idx_agents_heartbeat ON agents(last_heartbeat);
CREATE INDEX IF NOT EXISTS idx_agents_parent ON agents(parent_id);
CREATE INDEX IF NOT EXISTS idx_agents_ppid ON agents(ppid);
-- NOTE: idx_agents_factory_session is deliberately NOT here. This baseline
-- runs at every store open, BEFORE migrations. On a pre-m204 database the
-- CREATE TABLE above no-ops (old table, no factory_session column) and an
-- index on that column would abort store init — which bricked `cas serve`
-- on every existing store (2026-07-06 regression). The index lives in
-- migration m205, which runs after the column exists on every path.
-- General rule: never index a migration-added column from a baseline schema.

-- Task leases table: tracks exclusive task claims
CREATE TABLE IF NOT EXISTS task_leases (
    task_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    acquired_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    renewed_at TEXT NOT NULL,
    renewal_count INTEGER NOT NULL DEFAULT 0,
    epoch INTEGER NOT NULL DEFAULT 1,
    claim_reason TEXT,
    FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_leases_agent ON task_leases(agent_id);
CREATE INDEX IF NOT EXISTS idx_leases_status ON task_leases(status);
CREATE INDEX IF NOT EXISTS idx_leases_expires ON task_leases(expires_at);

-- Lease history table: audit log of all lease operations
CREATE TABLE IF NOT EXISTS task_lease_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    event_type TEXT NOT NULL,  -- 'claimed', 'released', 'expired', 'transferred', 'renewed'
    epoch INTEGER NOT NULL DEFAULT 1,
    timestamp TEXT NOT NULL,
    details TEXT,  -- JSON with additional context
    previous_agent_id TEXT,  -- For transfers, who held it before
    reason TEXT  -- Human-readable reason for the event
);

CREATE INDEX IF NOT EXISTS idx_lease_history_task ON task_lease_history(task_id);
CREATE INDEX IF NOT EXISTS idx_lease_history_agent ON task_lease_history(agent_id);
CREATE INDEX IF NOT EXISTS idx_lease_history_timestamp ON task_lease_history(timestamp);

-- Daemon instances table: tracks active embedded daemons
CREATE TABLE IF NOT EXISTS daemon_instances (
    id TEXT PRIMARY KEY,
    pid INTEGER NOT NULL,
    daemon_type TEXT NOT NULL DEFAULT 'mcp_embedded',
    started_at TEXT NOT NULL,
    last_heartbeat TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'running'
);

CREATE INDEX IF NOT EXISTS idx_daemon_heartbeat ON daemon_instances(last_heartbeat);
CREATE INDEX IF NOT EXISTS idx_daemon_status ON daemon_instances(status);

-- Working epics table: tracks which epics an agent is actively working on
-- When an agent claims a subtask of an epic, the epic is recorded here
-- Used by exit blocker to check if epic has remaining open subtasks
CREATE TABLE IF NOT EXISTS working_epics (
    agent_id TEXT NOT NULL,
    epic_id TEXT NOT NULL,
    started_at TEXT NOT NULL,
    PRIMARY KEY (agent_id, epic_id)
);

CREATE INDEX IF NOT EXISTS idx_working_epics_agent ON working_epics(agent_id);

-- Worktree leases table: tracks exclusive worktree claims for multi-agent coordination
CREATE TABLE IF NOT EXISTS worktree_leases (
    worktree_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    acquired_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    renewed_at TEXT NOT NULL,
    renewal_count INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_worktree_leases_agent ON worktree_leases(agent_id);
CREATE INDEX IF NOT EXISTS idx_worktree_leases_status ON worktree_leases(status);
CREATE INDEX IF NOT EXISTS idx_worktree_leases_expires ON worktree_leases(expires_at);
"#;

/// Trait for agent storage operations
pub trait AgentStore: Send + Sync {
    /// Initialize the store (create tables)
    fn init(&self) -> Result<()>;

    /// Register a new agent
    fn register(&self, agent: &Agent) -> Result<()>;

    /// Get an agent by ID
    fn get(&self, id: &str) -> Result<Agent>;

    /// Update an agent
    fn update(&self, agent: &Agent) -> Result<()>;

    /// Remove an agent (and release all its leases)
    fn unregister(&self, id: &str) -> Result<()>;

    /// List all agents with optional status filter
    fn list(&self, status: Option<AgentStatus>) -> Result<Vec<Agent>>;

    /// List agents that need heartbeat check (idle or potentially dead)
    fn list_stale(&self, timeout_secs: i64) -> Result<Vec<Agent>>;

    /// List agents that registered but never confirmed startup (no heartbeat received)
    /// Returns agents where startup_confirmed = 0 and registered more than timeout_secs ago
    fn list_failed_startup(&self, timeout_secs: i64) -> Result<Vec<Agent>>;

    /// Update agent heartbeat
    fn heartbeat(&self, id: &str) -> Result<()>;

    /// Mark an agent as stale and revoke its leases
    /// Called when agent hasn't sent heartbeat for stale threshold (crash detection)
    fn mark_stale(&self, id: &str) -> Result<()>;

    /// Revive a stale/shutdown agent back to active status
    /// Called when MCP tool is used and agent exists but is not active
    fn revive(&self, id: &str) -> Result<()>;

    /// Get agent by Claude Code parent PID (fallback when session file missing)
    fn get_by_cc_pid(&self, cc_pid: u32) -> Result<Option<Agent>>;

    /// Get agent by its own PID (for daemon PID-based adoption)
    fn get_by_pid(&self, pid: u32) -> Result<Option<Agent>>;

    /// Try to claim a task for an agent (atomic operation)
    fn try_claim(
        &self,
        task_id: &str,
        agent_id: &str,
        duration_secs: i64,
        reason: Option<&str>,
    ) -> Result<ClaimResult>;

    /// Release a task lease
    fn release_lease(&self, task_id: &str, agent_id: &str) -> Result<()>;

    /// Release any lease on a task (regardless of owner), recording why it was released.
    fn release_lease_for_task(&self, task_id: &str, reason: &str) -> Result<bool>;

    /// Renew a task lease
    fn renew_lease(&self, task_id: &str, agent_id: &str, duration_secs: i64) -> Result<()>;

    /// Get lease for a task
    fn get_lease(&self, task_id: &str) -> Result<Option<TaskLease>>;

    /// List all leases for an agent
    fn list_agent_leases(&self, agent_id: &str) -> Result<Vec<TaskLease>>;

    /// List all active leases
    fn list_active_leases(&self) -> Result<Vec<TaskLease>>;

    /// Reclaim expired leases (returns number reclaimed)
    fn reclaim_expired_leases(&self) -> Result<usize>;

    /// Delete lease history entries older than the given number of days
    fn cleanup_lease_history(&self, older_than_days: i64) -> Result<usize>;

    /// Get lease history for a task (audit log)
    fn get_lease_history(
        &self,
        task_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<LeaseHistoryEntry>>;

    /// Get lease history for an agent (all tasks they worked on)
    /// Returns unique task IDs from the agent's lease history
    /// If `since` is provided, only returns tasks claimed after that timestamp
    fn get_agent_worked_tasks(
        &self,
        agent_id: &str,
        since: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Vec<String>>;

    /// Get active child agents (for exit blocking)
    fn get_active_children(&self, agent_id: &str) -> Result<Vec<Agent>>;

    /// Gracefully shutdown an agent: release all leases and mark as shutdown
    /// Returns the list of task IDs that had leases released
    fn graceful_shutdown(&self, agent_id: &str) -> Result<Vec<String>>;

    // Working epics tracking (for exit blocker)

    /// Record that an agent is working on an epic
    /// Called when agent claims a subtask that belongs to an epic
    fn add_working_epic(&self, agent_id: &str, epic_id: &str) -> Result<()>;

    /// Get all epics an agent is working on
    fn get_working_epics(&self, agent_id: &str) -> Result<Vec<String>>;

    /// Get ALL unique working epics across all agents (for exit blocker check)
    fn list_all_working_epics(&self) -> Result<Vec<String>>;

    /// Get working epics only from non-active (stale/dead) agents
    /// Used by exit blocker to inherit orphaned epics without blocking on other active agents' work
    fn list_orphaned_working_epics(&self) -> Result<Vec<String>>;

    /// Remove a working epic (when epic is completed or agent stops working on it)
    fn remove_working_epic(&self, agent_id: &str, epic_id: &str) -> Result<()>;

    /// Clear all working epics for an agent (on session end)
    fn clear_working_epics(&self, agent_id: &str) -> Result<()>;

    // Daemon instance tracking

    /// Register a daemon instance
    fn register_daemon(&self, daemon_id: &str, daemon_type: &str) -> Result<()>;

    /// Update daemon heartbeat
    fn daemon_heartbeat(&self, daemon_id: &str) -> Result<()>;

    /// Unregister a daemon instance
    fn unregister_daemon(&self, daemon_id: &str) -> Result<()>;

    /// Check if any daemon is active (heartbeat within threshold)
    fn is_daemon_active(&self, threshold_secs: i64) -> Result<bool>;

    // Worktree lease operations (exclusive worktree locking)

    /// Try to claim a worktree for an agent (atomic operation)
    /// Returns Success if claimed, AlreadyClaimed if another agent holds it
    fn try_claim_worktree(
        &self,
        worktree_id: &str,
        agent_id: &str,
        duration_secs: i64,
    ) -> Result<WorktreeClaimResult>;

    /// Release a worktree lease
    fn release_worktree_lease(&self, worktree_id: &str, agent_id: &str) -> Result<()>;

    /// Renew a worktree lease
    fn renew_worktree_lease(
        &self,
        worktree_id: &str,
        agent_id: &str,
        duration_secs: i64,
    ) -> Result<()>;

    /// Get lease for a worktree (active only)
    fn get_worktree_lease(&self, worktree_id: &str) -> Result<Option<WorktreeLease>>;

    /// Get worktree lease by epic ID (looks up worktree for epic, then gets lease)
    fn get_worktree_lease_for_epic(&self, epic_id: &str) -> Result<Option<WorktreeLease>>;

    /// List all worktree leases for an agent
    fn list_agent_worktree_leases(&self, agent_id: &str) -> Result<Vec<WorktreeLease>>;

    /// List all active worktree leases
    fn list_active_worktree_leases(&self) -> Result<Vec<WorktreeLease>>;

    /// Reclaim expired worktree leases (returns number reclaimed)
    fn reclaim_expired_worktree_leases(&self) -> Result<usize>;

    /// Check if an agent can work on an epic (no conflicting worktree lease)
    fn can_agent_work_on_epic(&self, agent_id: &str, epic_id: &str) -> Result<bool>;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// SQLite-based agent store
pub struct SqliteAgentStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteAgentStore {
    /// Open or create a SQLite agent store
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let db_path = cas_dir.join("cas.db");
        let conn = crate::shared_db::shared_connection(&db_path)?;

        Ok(Self { conn })
    }

    fn parse_datetime(s: &str) -> Option<DateTime<Utc>> {
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(dt.with_timezone(&Utc));
        }
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
            return Some(Utc.from_utc_datetime(&dt));
        }
        None
    }

    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|e| StoreError::Other(format!("agent store lock poisoned: {e}")))
    }

    fn agent_from_row(row: &rusqlite::Row) -> rusqlite::Result<Agent> {
        let metadata_str: String = row.get(13)?;
        let metadata: std::collections::HashMap<String, String> =
            serde_json::from_str(&metadata_str).unwrap_or_default();
        let active_tasks = row
            .get_ref(12)
            .map(Self::parse_active_tasks)
            .unwrap_or_default();
        // cas-b157: typed pid_starttime at column 14. Nullable, so any
        // SELECT that forgot to project the column (or pre-migration
        // rows) falls back to None and the liveness gate picks up the
        // legacy metadata fallback in the daemon.
        let pid_starttime = row
            .get_ref(14)
            .ok()
            .and_then(Self::parse_optional_u64_from_value);

        Ok(Agent {
            id: row.get(0)?,
            name: row.get(1)?,
            agent_type: row
                .get::<_, String>(2)?
                .parse()
                .unwrap_or(AgentType::Primary),
            role: row
                .get::<_, String>(3)?
                .parse()
                .unwrap_or(AgentRole::Standard),
            status: row
                .get::<_, String>(4)?
                .parse()
                .unwrap_or(AgentStatus::Active),
            pid: row
                .get_ref(5)
                .ok()
                .and_then(Self::parse_optional_u32_from_value),
            ppid: row
                .get_ref(6)
                .ok()
                .and_then(Self::parse_optional_u32_from_value),
            cc_session_id: row.get(7)?,
            factory_session: row.get(15).ok(),
            parent_id: row.get(8)?,
            machine_id: row.get(9)?,
            registered_at: Self::parse_datetime(&row.get::<_, String>(10)?)
                .unwrap_or_else(Utc::now),
            last_heartbeat: Self::parse_datetime(&row.get::<_, String>(11)?)
                .unwrap_or_else(Utc::now),
            active_tasks,
            pid_starttime,
            metadata,
        })
    }

    fn parse_active_tasks(value: ValueRef<'_>) -> u32 {
        match Self::parse_non_negative_i64_from_value(value) {
            Some(n) => n as u32,
            None => match value {
                // Legacy/dirty rows may contain JSON arrays (e.g., '["task-1"]').
                ValueRef::Text(bytes) => {
                    let trimmed = String::from_utf8_lossy(bytes);
                    if let Ok(serde_json::Value::Array(items)) =
                        serde_json::from_str::<serde_json::Value>(trimmed.trim())
                    {
                        items.len() as u32
                    } else {
                        0
                    }
                }
                _ => 0,
            },
        }
    }

    fn lease_from_row(row: &rusqlite::Row) -> rusqlite::Result<TaskLease> {
        Ok(TaskLease {
            task_id: row.get(0)?,
            agent_id: row.get(1)?,
            status: row
                .get::<_, String>(2)?
                .parse()
                .unwrap_or(LeaseStatus::Active),
            acquired_at: Self::parse_datetime(&row.get::<_, String>(3)?).unwrap_or_else(Utc::now),
            expires_at: Self::parse_datetime(&row.get::<_, String>(4)?).unwrap_or_else(Utc::now),
            renewed_at: Self::parse_datetime(&row.get::<_, String>(5)?).unwrap_or_else(Utc::now),
            renewal_count: row
                .get_ref(6)
                .map(Self::parse_non_negative_i64_from_value)
                .ok()
                .flatten()
                .unwrap_or_default() as u32,
            epoch: row
                .get_ref(7)
                .map(Self::parse_non_negative_i64_from_value)
                .ok()
                .flatten()
                .unwrap_or(1) as u64,
            claim_reason: row.get(8)?,
        })
    }

    fn worktree_lease_from_row(row: &rusqlite::Row) -> rusqlite::Result<WorktreeLease> {
        Ok(WorktreeLease {
            worktree_id: row.get(0)?,
            agent_id: row.get(1)?,
            status: row
                .get::<_, String>(2)?
                .parse()
                .unwrap_or(LeaseStatus::Active),
            acquired_at: Self::parse_datetime(&row.get::<_, String>(3)?).unwrap_or_else(Utc::now),
            expires_at: Self::parse_datetime(&row.get::<_, String>(4)?).unwrap_or_else(Utc::now),
            renewed_at: Self::parse_datetime(&row.get::<_, String>(5)?).unwrap_or_else(Utc::now),
            renewal_count: row
                .get_ref(6)
                .map(Self::parse_non_negative_i64_from_value)
                .ok()
                .flatten()
                .unwrap_or_default() as u32,
        })
    }

    fn parse_optional_u32_from_value(value: ValueRef<'_>) -> Option<u32> {
        Self::parse_non_negative_i64_from_value(value).map(|n| n as u32)
    }

    /// Parse a nullable non-negative integer column into `Option<u64>`.
    ///
    /// Used for the typed `pid_starttime` column (cas-b157) — Linux
    /// `/proc/<pid>/stat` field 22 is a `u64` in clock ticks since boot.
    /// Storing it as SQLite `INTEGER` (i64) loses the top bit, but real
    /// boot uptimes never approach 2^63 ticks, so the round-trip is
    /// safe in practice. We normalise via
    /// [`parse_non_negative_i64_from_value`] to share the malformed-row
    /// defensive handling with the `u32` path.
    fn parse_optional_u64_from_value(value: ValueRef<'_>) -> Option<u64> {
        Self::parse_non_negative_i64_from_value(value).map(|n| n as u64)
    }

    fn parse_non_negative_i64_from_value(value: ValueRef<'_>) -> Option<i64> {
        match value {
            ValueRef::Null => None,
            ValueRef::Integer(n) => Some(n.max(0)),
            ValueRef::Real(n) => Some(n.max(0.0) as i64),
            ValueRef::Text(bytes) => {
                let trimmed = String::from_utf8_lossy(bytes);
                if let Ok(n) = trimmed.trim().parse::<i64>() {
                    return Some(n.max(0));
                }
                if let Ok(n) = trimmed.trim().parse::<f64>() {
                    return Some(n.max(0.0) as i64);
                }
                None
            }
            ValueRef::Blob(_) => None,
        }
    }

    /// Log a lease event to the history table for audit purposes
    fn log_lease_event(
        conn: &rusqlite::Connection,
        task_id: &str,
        agent_id: &str,
        event_type: &str,
        epoch: u64,
        details: Option<&str>,
        previous_agent_id: Option<&str>,
        reason: Option<&str>,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO task_lease_history (task_id, agent_id, event_type, epoch, timestamp, details, previous_agent_id, reason)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                task_id,
                agent_id,
                event_type,
                epoch as i64,
                Utc::now().to_rfc3339(),
                details,
                previous_agent_id,
                reason,
            ],
        )?;
        Ok(())
    }
}

mod ops_agent;
mod ops_coordination;
mod ops_task_leases;
mod ops_worktree;
mod trait_impl;

#[cfg(test)]
mod tests;
