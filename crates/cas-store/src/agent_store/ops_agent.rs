use crate::Result;
use crate::agent_store::{AGENT_SCHEMA, SqliteAgentStore};
use crate::error::StoreError;
use crate::event_store::record_event_with_conn;
use crate::recording_store::capture_agent_event;
use crate::shared_db::ImmediateTx;
use cas_types::{
    Agent, AgentStatus, DEFAULT_LEASE_DURATION_SECS, Event, EventEntityType, EventType,
    RecordingEventType,
};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};

/// cas-85d9: renewal window applied to a live agent's active task leases on
/// every heartbeat (see the long comment in `agent_heartbeat` for why this
/// doesn't weaken dead-holder recovery). Reuses `DEFAULT_LEASE_DURATION_SECS`
/// (600s) — an agent heartbeating every 5-30s (observed daemon tick cadence)
/// keeps its leases continuously fresh well inside this window, while a
/// worker that stops heartbeating still lets the lease expire naturally.
const TASK_LEASE_HEARTBEAT_RENEWAL_SECS: i64 = DEFAULT_LEASE_DURATION_SECS;

const FACTORY_SESSION_REHOMED_FROM_KEY: &str = "factory_session_rehomed_from";
const FACTORY_SESSION_REHOMED_AT_KEY: &str = "factory_session_rehomed_at";

/// Move the live fleet owned by a restarted logical supervisor to its new
/// factory session and retire the supervisor identities it replaces.
///
/// Factory session ids identify one supervisor *process lifetime*, not the
/// durable supervisor.  A restart therefore changes `factory_session` while
/// preserving `(project database, supervisor name, role)`.  Leaving the old
/// rows active makes session-scoped status, activity, and prompt delivery lose
/// workers that never stopped running; later maintenance also mistakes the old
/// supervisor rows for dead workers.  Reconcile while registration still owns
/// the write transaction so readers can observe only the pre- or post-restart
/// registry, never a split fleet.
fn reconcile_restarted_factory_supervisor(
    conn: &Connection,
    agent: &Agent,
    factory_session: Option<&str>,
) -> Result<()> {
    if !matches!(agent.role, cas_types::AgentRole::Supervisor)
        || factory_session.is_none_or(str::is_empty)
    {
        return Ok(());
    }
    let factory_session = factory_session.expect("checked above");
    let role = agent.role.to_string();

    let mut prior_stmt = conn.prepare_cached(
        "SELECT DISTINCT factory_session
         FROM agents
         WHERE id <> ?1
           AND name = ?2
           AND role = ?3
           AND status IN ('active', 'idle')
           AND factory_session IS NOT NULL",
    )?;
    let prior_sessions = prior_stmt
        .query_map(params![agent.id, agent.name, role], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(prior_stmt);

    if prior_sessions.is_empty() {
        return Ok(());
    }

    let rehomed_at = Utc::now().to_rfc3339();
    for prior_session in prior_sessions
        .iter()
        .filter(|session| session.as_str() != factory_session)
    {
        let mut worker_stmt = conn.prepare_cached(
            "SELECT id, metadata
             FROM agents
             WHERE role = 'worker'
               AND status IN ('active', 'idle')
               AND factory_session = ?1",
        )?;
        let workers = worker_stmt
            .query_map(params![prior_session], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(worker_stmt);

        for (worker_id, metadata_json) in workers {
            let mut metadata =
                serde_json::from_str::<std::collections::HashMap<String, String>>(&metadata_json)
                    .unwrap_or_default();
            metadata.insert(
                FACTORY_SESSION_REHOMED_FROM_KEY.to_string(),
                prior_session.clone(),
            );
            metadata.insert(
                FACTORY_SESSION_REHOMED_AT_KEY.to_string(),
                rehomed_at.clone(),
            );
            let metadata_json = serde_json::to_string(&metadata).unwrap_or(metadata_json);
            conn.execute(
                "UPDATE agents
                 SET factory_session = ?1, metadata = ?2
                 WHERE id = ?3",
                params![factory_session, metadata_json, worker_id],
            )?;
        }
    }

    // `shutdown` is intentionally terminal for heartbeat and stale scans.  It
    // retains the forensic row without allowing maintenance to emit a
    // worker_died relay about the supervisor itself.
    conn.execute(
        "UPDATE agents
         SET status = 'shutdown'
         WHERE id <> ?1
           AND name = ?2
           AND role = ?3
           AND status IN ('active', 'idle')",
        params![agent.id, agent.name, role],
    )?;

    Ok(())
}

pub(crate) fn register_agent_with_conn(conn: &Connection, agent: &Agent) -> Result<()> {
    let metadata_json = serde_json::to_string(&agent.metadata).unwrap_or_else(|_| "{}".to_string());
    let env_factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
    let factory_session = agent
        .factory_session
        .as_deref()
        .or(env_factory_session.as_deref());
    let existed = conn
        .query_row(
            "SELECT 1 FROM agents WHERE id = ?1",
            params![agent.id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();

    // Use INSERT ... ON CONFLICT for idempotent registration.
    // This allows SessionStart hook and MCP to both register without conflict.
    // On conflict (re-registration), we preserve startup_confirmed so that a
    // live agent that re-registers doesn't get falsely detected as failed-startup.
    conn.execute(
        "INSERT INTO agents (id, name, agent_type, role, status, pid, ppid, cc_session_id, parent_id,
         machine_id, registered_at, last_heartbeat, active_tasks, metadata, pid_starttime, factory_session, startup_confirmed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 0)
         ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            agent_type = agents.agent_type,
            role = agents.role,
            status = excluded.status,
            pid = excluded.pid,
            ppid = excluded.ppid,
            -- cas-dffe: a registration that does not know its harness session
            -- id must not ERASE one that is already recorded. `clear_context`
            -- writes the post-reset Claude session id here so transcript
            -- resolution follows the live conversation; the MCP server
            -- re-registers with cc_session_id = NULL, which used to silently
            -- undo that. Mirrors the factory_session rule below.
            cc_session_id = COALESCE(excluded.cc_session_id, agents.cc_session_id),
            parent_id = excluded.parent_id,
            machine_id = excluded.machine_id,
            last_heartbeat = excluded.last_heartbeat,
            active_tasks = excluded.active_tasks,
            metadata = excluded.metadata,
            pid_starttime = excluded.pid_starttime,
            factory_session = COALESCE(excluded.factory_session, factory_session)",
        params![
            agent.id,
            agent.name,
            agent.agent_type.to_string(),
            agent.role.to_string(),
            agent.status.to_string(),
            agent.pid,
            agent.ppid,
            agent.cc_session_id,
            agent.parent_id,
            agent.machine_id,
            agent.registered_at.to_rfc3339(),
            agent.last_heartbeat.to_rfc3339(),
            agent.active_tasks,
            metadata_json,
            agent.pid_starttime.map(|v| v as i64),
            factory_session,
        ],
    )?;

    reconcile_restarted_factory_supervisor(conn, agent, factory_session)?;

    if !existed {
        // Record event for sidecar activity feed
        let event = Event::new(
            EventType::AgentRegistered,
            EventEntityType::Agent,
            &agent.id,
            format!("Agent registered: {}", agent.name),
        )
        .with_session(agent.cc_session_id.as_deref().unwrap_or(&agent.id));
        let _ = record_event_with_conn(conn, &event); // Best-effort, don't fail on event recording

        // Capture event for recording playback
        let _ = capture_agent_event(conn, RecordingEventType::AgentJoined, &agent.id, None);
    }

    Ok(())
}

impl SqliteAgentStore {
    pub(crate) fn agent_init(&self) -> Result<()> {
        let conn = self.lock_conn()?;
        conn.execute_batch(AGENT_SCHEMA)?;
        Ok(())
    }
    pub(crate) fn agent_register(&self, agent: &Agent) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.lock_conn()?;
            let tx = ImmediateTx::new(&conn)?;
            register_agent_with_conn(&tx, agent)?;
            tx.commit()?;
            Ok(())
        }) // with_write_retry
    }
    pub(crate) fn agent_get(&self, id: &str) -> Result<Agent> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT id, name, agent_type, role, status, pid, ppid, cc_session_id, parent_id,
             machine_id, registered_at, last_heartbeat, active_tasks, metadata, pid_starttime, factory_session
             FROM agents WHERE id = ?",
            params![id],
            Self::agent_from_row,
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound(format!("Agent not found: {id}")))
    }
    pub(crate) fn agent_update(&self, agent: &Agent) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.lock_conn()?;
            let metadata_json =
                serde_json::to_string(&agent.metadata).unwrap_or_else(|_| "{}".to_string());

            let rows = conn.execute(
                "UPDATE agents SET name = ?1, agent_type = ?2, role = ?3, status = ?4, pid = ?5,
             ppid = ?6, cc_session_id = ?7, parent_id = ?8, machine_id = ?9, last_heartbeat = ?10,
             active_tasks = ?11, metadata = ?12, pid_starttime = ?13, factory_session = COALESCE(?14, factory_session)
             WHERE id = ?15",
                params![
                    agent.name,
                    agent.agent_type.to_string(),
                    agent.role.to_string(),
                    agent.status.to_string(),
                    agent.pid,
                    agent.ppid,
                    agent.cc_session_id,
                    agent.parent_id,
                    agent.machine_id,
                    agent.last_heartbeat.to_rfc3339(),
                    agent.active_tasks,
                    metadata_json,
                    agent.pid_starttime.map(|v| v as i64),
                    agent.factory_session.as_deref(),
                    agent.id,
                ],
            )?;

            if rows == 0 {
                return Err(StoreError::NotFound(format!(
                    "Agent not found: {}",
                    agent.id
                )));
            }
            Ok(())
        }) // with_write_retry
    }
    pub(crate) fn agent_unregister(&self, id: &str) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.lock_conn()?;
            let tx = ImmediateTx::new(&conn)?;

            // Get agent name before deleting (for event summary)
            let agent_name: Option<String> = tx
                .query_row("SELECT name FROM agents WHERE id = ?", params![id], |row| {
                    row.get(0)
                })
                .optional()?;

            // Release all leases first (due to foreign key)
            tx.execute(
                "UPDATE task_leases SET status = 'released' WHERE agent_id = ?",
                params![id],
            )?;

            let rows = tx.execute("DELETE FROM agents WHERE id = ?", params![id])?;
            if rows == 0 {
                // SessionEnd, MCP shutdown, and factory teardown can all race
                // to retire the same identity. Repeating unregister is a
                // successful no-op and must not emit another activity event.
                tx.commit()?;
                return Ok(());
            }

            // Record event for sidecar activity feed (use name if available, else id)
            let display_name = agent_name
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| id.to_string());
            let event = Event::new(
                EventType::AgentShutdown,
                EventEntityType::Agent,
                id,
                format!("Agent unregistered: {display_name}"),
            );
            let _ = record_event_with_conn(&tx, &event);

            // Capture event for recording playback
            let _ = capture_agent_event(&tx, RecordingEventType::AgentLeft, id, None);

            tx.commit()?;
            Ok(())
        }) // with_write_retry
    }
    pub(crate) fn agent_list(&self, status: Option<AgentStatus>) -> Result<Vec<Agent>> {
        let conn = self.lock_conn()?;

        let (sql, params): (&str, Vec<String>) = match status {
            Some(s) => (
                "SELECT id, name, agent_type, role, status, pid, ppid, cc_session_id, parent_id,
                 machine_id, registered_at, last_heartbeat, active_tasks, metadata, pid_starttime, factory_session
                 FROM agents WHERE status = ? ORDER BY registered_at DESC",
                vec![s.to_string()],
            ),
            None => (
                "SELECT id, name, agent_type, role, status, pid, ppid, cc_session_id, parent_id,
                 machine_id, registered_at, last_heartbeat, active_tasks, metadata, pid_starttime, factory_session
                 FROM agents ORDER BY registered_at DESC",
                vec![],
            ),
        };

        let mut stmt = conn.prepare_cached(sql)?;
        let agents = if params.is_empty() {
            stmt.query_map([], Self::agent_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![params[0]], Self::agent_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };

        Ok(agents)
    }
    pub(crate) fn agent_list_stale(&self, timeout_secs: i64) -> Result<Vec<Agent>> {
        let conn = self.lock_conn()?;
        let cutoff = (Utc::now() - chrono::Duration::seconds(timeout_secs)).to_rfc3339();

        let mut stmt = conn.prepare_cached(
            "SELECT id, name, agent_type, role, status, pid, ppid, cc_session_id, parent_id,
             machine_id, registered_at, last_heartbeat, active_tasks, metadata, pid_starttime, factory_session
             FROM agents
             WHERE status IN ('active', 'idle') AND last_heartbeat < ?
             ORDER BY last_heartbeat ASC",
        )?;

        let agents = stmt
            .query_map(params![cutoff], Self::agent_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(agents)
    }
    pub(crate) fn agent_heartbeat(&self, id: &str) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.lock_conn()?;
            let now_dt = Utc::now();
            let now = now_dt.to_rfc3339();

            // Only heartbeat agents in live states (active/idle). Agents that have been
            // explicitly shut down or marked stale should not be revived by a heartbeat —
            // their daemon may still be running briefly after the process was killed.
            // Also confirm startup on first heartbeat (startup_confirmed = 1).
            let rows = conn.execute(
            "UPDATE agents SET last_heartbeat = ?, status = 'active', startup_confirmed = 1 WHERE id = ? AND status IN ('active', 'idle')",
            params![now, id],
        )?;

            if rows == 0 {
                // Use a single query to check existence and get status,
                // providing a specific error message without a second round-trip.
                // We already know the UPDATE didn't match, so the agent either
                // doesn't exist or is in a non-live state.
                let status: Option<String> = conn
                    .query_row(
                        "SELECT status FROM agents WHERE id = ?",
                        params![id],
                        |row| row.get(0),
                    )
                    .optional()?;
                match status {
                    Some(s) if s == "shutdown" || s == "stale" => {
                        return Err(StoreError::Other(format!(
                            "Agent {id} is {s} — heartbeat ignored"
                        )));
                    }
                    Some(s) => {
                        return Err(StoreError::Other(format!(
                            "Agent {id} has unexpected status '{s}'"
                        )));
                    }
                    None => {
                        return Err(StoreError::NotFound(format!("Agent not found: {id}")));
                    }
                }
            }

            // cas-85d9: renew this agent's active task AND worktree leases
            // in the SAME transaction as the heartbeat itself. Root-cause
            // fix for the "leases are never renewed" gap (cas-d165
            // finding).
            //
            // Task leases: nothing in production ever called `renew_lease`
            // for them at all, so any task held past its ~30min claim
            // duration lost its lease under a perfectly healthy,
            // heartbeating worker.
            //
            // Worktree leases: DID have a renewal call site —
            // `cas_agent_heartbeat` (the `mcp__cas__coordination
            // action=heartbeat` MCP handler,
            // agent_coordination/agent_management.rs) renews them via
            // `renew_worktree_lease`. But that handler is a *client-invoked
            // MCP tool call*; the actual high-frequency (~5-30s observed)
            // production heartbeat is a separate, purely internal loop —
            // the daemon's per-agent PID-liveness monitor
            // (`cas-cli/src/mcp/daemon.rs`) — which calls this very
            // `store.heartbeat()` method DIRECTLY and never goes anywhere
            // near the MCP handler layer. So worktree leases had the exact
            // same hole as task leases in practice: whether they ever got
            // renewed depended on whether an agent happened to also call
            // the `heartbeat` MCP action explicitly (uncertain/inconsistent
            // in the field), not on the daemon keeping the process alive.
            // Fixing renewal here, at the one method both paths actually
            // call, closes the gap for both lease types uniformly instead
            // of leaving worktree leases with a second, harder-to-notice
            // instance of the same bug right next to the one just fixed.
            //
            // Why heartbeat-renewal does NOT weaken dead-holder recovery
            // (the property the lease exists for): `agent_mark_stale`
            // (ops_agent.rs, a few lines below) already revokes ALL of an
            // agent's active task leases immediately and unconditionally
            // the moment heartbeat staleness is detected (~30-75s via
            // WORKER_STALE_SECS/WORKER_DEAD_SECS, well before this renewal
            // window could matter) — dead-holder recovery has never
            // actually depended on a lease's own `expires_at` timer
            // reaching zero; it is driven entirely by heartbeat staleness.
            // A worker whose process is truly dead stops heartbeating and
            // therefore stops renewing within seconds, same as today.
            //
            // A worker whose PROCESS is alive but whose WORK LOOP is
            // wedged (heartbeating, doing nothing) is a different failure
            // mode the lease's `expires_at` timer was never actually
            // catching either way — `mark_stale` only fires on heartbeat
            // *staleness*, and a wedged-but-heartbeating worker's
            // heartbeat stays fresh. That failure mode is the job of
            // `WorkerStalled` / `cas factory is-wedged` (checkpoint-age +
            // in-flight-tool-call evidence, cas-9829/cas-7e85/cas-d165),
            // which does not consult lease state at all. Renewing the
            // lease here does not make a wedged worker any harder to
            // detect or recover than it already is/was.
            //
            // Renewal window: extends `expires_at` to `now +
            // TASK_LEASE_HEARTBEAT_RENEWAL_SECS` on every heartbeat.
            // Heartbeats observed every 5-30s in practice (daemon tick),
            // so this keeps an actively-heartbeating worker's lease
            // continuously fresh; a worker that stops heartbeating for
            // longer than the window still lets the lease expire
            // naturally (defense in depth alongside `mark_stale`).
            let renewed_until =
                (now_dt + chrono::Duration::seconds(TASK_LEASE_HEARTBEAT_RENEWAL_SECS))
                    .to_rfc3339();
            conn.execute(
                "UPDATE task_leases SET expires_at = ?, renewed_at = ?, renewal_count = renewal_count + 1
                 WHERE agent_id = ? AND status = 'active'",
                params![renewed_until, now, id],
            )?;
            conn.execute(
                "UPDATE worktree_leases SET expires_at = ?, renewed_at = ?, renewal_count = renewal_count + 1
                 WHERE agent_id = ? AND status = 'active'",
                params![renewed_until, now, id],
            )?;

            Ok(())
        }) // with_write_retry
    }
    pub(crate) fn agent_mark_stale(&self, id: &str) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.lock_conn()?;
            let tx = ImmediateTx::new(&conn)?;

            // Get all active leases for this agent before revoking
            let mut stmt = tx.prepare_cached(
                "SELECT task_id, epoch FROM task_leases WHERE agent_id = ? AND status = 'active'",
            )?;
            let leases_to_revoke: Vec<(String, i64)> = stmt
                .query_map(params![id], |row| {
                    Ok((row.get(0)?, row.get::<_, i64>(1).unwrap_or(1)))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(stmt);

            // Mark agent as stale (was: dead)
            tx.execute(
                "UPDATE agents SET status = 'stale' WHERE id = ?",
                params![id],
            )?;

            // Revoke all active task leases
            tx.execute(
            "UPDATE task_leases SET status = 'revoked' WHERE agent_id = ? AND status = 'active'",
            params![id],
        )?;

            // Revoke all active worktree leases
            tx.execute(
            "UPDATE worktree_leases SET status = 'revoked' WHERE agent_id = ? AND status = 'active'",
            params![id],
        )?;

            // Log revoked events for each lease
            for (task_id, epoch) in &leases_to_revoke {
                Self::log_lease_event(
                    &tx,
                    task_id,
                    id,
                    "revoked",
                    *epoch as u64,
                    None,
                    None,
                    Some("agent_stale"),
                )?;
            }

            tx.commit()?;
            Ok(())
        }) // with_write_retry
    }
    pub(crate) fn agent_revive(&self, id: &str) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.lock_conn()?;
            let now = Utc::now().to_rfc3339();

            // Revive agent: set status to active, update heartbeat, and confirm startup.
            // Only works if agent exists and is in stale/shutdown/dead state.
            // Setting startup_confirmed = 1 prevents the agent from being immediately
            // re-detected as failed-startup after revival.
            let rows = conn.execute(
                "UPDATE agents SET status = 'active', last_heartbeat = ?, startup_confirmed = 1
             WHERE id = ? AND status IN ('dead', 'shutdown', 'stale')",
                params![now, id],
            )?;

            if rows == 0 {
                return Err(StoreError::NotFound(format!(
                    "Agent not found or already active: {id}"
                )));
            }

            Ok(())
        }) // with_write_retry
    }
    pub(crate) fn agent_list_failed_startup(&self, timeout_secs: i64) -> Result<Vec<Agent>> {
        let conn = self.lock_conn()?;
        let cutoff = (Utc::now() - chrono::Duration::seconds(timeout_secs)).to_rfc3339();

        let mut stmt = conn.prepare_cached(
            "SELECT id, name, agent_type, role, status, pid, ppid, cc_session_id, parent_id,
             machine_id, registered_at, last_heartbeat, active_tasks, metadata, pid_starttime, factory_session
             FROM agents
             WHERE status IN ('active', 'idle') AND startup_confirmed = 0 AND registered_at < ?
             ORDER BY registered_at ASC",
        )?;

        let agents = stmt
            .query_map(params![cutoff], Self::agent_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(agents)
    }

    pub(crate) fn agent_get_by_cc_pid(&self, cc_pid: u32) -> Result<Option<Agent>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, name, agent_type, role, status, pid, ppid, cc_session_id, parent_id,
             machine_id, registered_at, last_heartbeat, active_tasks, metadata, pid_starttime, factory_session
             FROM agents WHERE ppid = ? AND status IN ('active', 'idle', 'stale', 'dead', 'shutdown')
             ORDER BY last_heartbeat DESC LIMIT 1",
        )?;

        stmt.query_row(params![cc_pid], Self::agent_from_row)
            .optional()
            .map_err(Into::into)
    }

    pub(crate) fn agent_get_by_pid(&self, pid: u32) -> Result<Option<Agent>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, name, agent_type, role, status, pid, ppid, cc_session_id, parent_id,
             machine_id, registered_at, last_heartbeat, active_tasks, metadata, pid_starttime, factory_session
             FROM agents WHERE pid = ? AND status IN ('active', 'idle', 'stale', 'dead', 'shutdown')
             ORDER BY last_heartbeat DESC LIMIT 1",
        )?;

        stmt.query_row(params![pid], Self::agent_from_row)
            .optional()
            .map_err(Into::into)
    }
}
