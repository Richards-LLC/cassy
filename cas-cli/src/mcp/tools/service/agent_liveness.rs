//! Authoritative factory agent liveness (cas-e98e).
//!
//! Supervisors previously saw **four disagreeing answers** to "who is alive?":
//! `worker_status`, `agent_list`, the FACTORY pane, and the OS process table.
//! cas-3e56 fixed the high-severity Grok false-stale path on `worker_status`.
//! This module is the **single source of truth** those surfaces should share:
//!
//! **Authoritative formula:** an agent is *supervision-live* if either
//! (a) heartbeat is fresher than [`WORKER_STALE_SECS`] while Active/Idle, or
//! (b) the OS still has a live harness process for that agent
//!     (even when heartbeat lagged or the registry row is Stale).
//!
//! Shutdown decisions must use this dual signal — never `worker_status`
//! "None active" alone (see cas-supervisor skill note).

use cas_types::{Agent, AgentRole, AgentStatus};

/// Heartbeat age at which a worker is considered **stale** for supervision
/// prune / dual-signal (same constant as `factory_ops::WORKER_STALE_SECS`).
pub const WORKER_STALE_SECS: i64 = 30;

/// Effective liveness for supervisor tooling (cas-e98e).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisionLiveness {
    /// Registry Active/Idle and heartbeat within [`WORKER_STALE_SECS`].
    Live,
    /// Process proves mid-turn despite lagged heartbeat or Stale registry.
    AliveHeartbeatStale,
    /// Not live for supervision (no fresh heartbeat, no process).
    NotLive,
}

impl SupervisionLiveness {
    pub fn is_live(self) -> bool {
        !matches!(self, Self::NotLive)
    }
}

/// Seconds since last heartbeat.
pub fn agent_heartbeat_age_secs(agent: &Agent) -> i64 {
    (chrono::Utc::now() - agent.last_heartbeat)
        .num_seconds()
        .max(0)
}

/// cas-3e56/cas-e98e: whether this agent still has a live harness process.
pub fn agent_process_is_alive(agent: &Agent) -> bool {
    if agent_process_is_alive_with(
        agent,
        crate::mcp::daemon::pid_alive,
        crate::mcp::daemon::pid_matches_fingerprint,
    ) {
        return true;
    }
    crate::cli::factory::wedged::find_worker_pid(
        &crate::cli::factory::wedged::RealProcessTable,
        &agent.name,
    )
    .is_some()
}

/// Injected-probe registered-pid check (unit tests).
pub fn agent_process_is_alive_with(
    agent: &Agent,
    pid_alive_fn: impl Fn(u32) -> bool,
    fingerprint_matches_fn: impl Fn(u32, u64) -> bool,
) -> bool {
    let Some(pid) = agent.pid else {
        return false;
    };
    let expected_starttime = agent.pid_starttime.or_else(|| {
        agent
            .metadata
            .get(crate::mcp::daemon::PID_STARTTIME_KEY)
            .and_then(|s| s.parse::<u64>().ok())
    });
    match expected_starttime {
        Some(st) => fingerprint_matches_fn(pid, st),
        None => pid_alive_fn(pid),
    }
}

/// Evaluate authoritative supervision liveness (`process_alive` injected).
pub fn evaluate_supervision_liveness_with(
    agent: &Agent,
    process_alive: bool,
    stale_secs: i64,
) -> SupervisionLiveness {
    match agent.status {
        AgentStatus::Shutdown => {
            if process_alive {
                SupervisionLiveness::AliveHeartbeatStale
            } else {
                SupervisionLiveness::NotLive
            }
        }
        AgentStatus::Stale => {
            if process_alive {
                SupervisionLiveness::AliveHeartbeatStale
            } else {
                SupervisionLiveness::NotLive
            }
        }
        AgentStatus::Active | AgentStatus::Idle => {
            let age = agent_heartbeat_age_secs(agent);
            if age < stale_secs {
                SupervisionLiveness::Live
            } else if process_alive {
                SupervisionLiveness::AliveHeartbeatStale
            } else {
                SupervisionLiveness::NotLive
            }
        }
    }
}

/// Production path.
pub fn evaluate_supervision_liveness(agent: &Agent) -> SupervisionLiveness {
    evaluate_supervision_liveness_with(agent, agent_process_is_alive(agent), WORKER_STALE_SECS)
}

/// Whether any registry row for a visible worker name is supervision-live.
///
/// Task assignees persist the worker's display name, while `AgentStore::get`
/// is keyed by the opaque agent ID.  Keep this name-based resolution beside
/// the authoritative liveness formula so task-ownership paths cannot confuse
/// the two identities (cas-2327).
pub fn has_live_agent_named<'a>(agents: impl IntoIterator<Item = &'a Agent>, name: &str) -> bool {
    agents
        .into_iter()
        .any(|agent| agent.name == name && evaluate_supervision_liveness(agent).is_live())
}

/// Live factory worker for roster agreement (`worker_status` ↔ `agent_list`).
pub fn is_live_factory_worker(agent: &Agent) -> bool {
    agent.role == AgentRole::Worker && evaluate_supervision_liveness(agent).is_live()
}

/// Whether a worker pane named `name` should remain given registry agents.
///
/// cas-e98e AC3 phantom-pane rule: keep when still registering (no rows) or
/// any matching row is supervision-live; drop when every matching row is
/// non-live (dead process + stale/shutdown registry).
pub fn should_keep_worker_pane<'a>(
    name: &str,
    agents: impl IntoIterator<Item = &'a Agent>,
) -> bool {
    let matching: Vec<&Agent> = agents.into_iter().filter(|a| a.name == name).collect();
    if matching.is_empty() {
        return true;
    }
    matching
        .iter()
        .any(|a| evaluate_supervision_liveness(a).is_live())
}

/// `agent_list` status token using authoritative liveness.
pub fn agent_list_status_label(agent: &Agent) -> String {
    match evaluate_supervision_liveness(agent) {
        SupervisionLiveness::Live => match agent.status {
            AgentStatus::Idle => "idle".to_string(),
            _ => "active".to_string(),
        },
        SupervisionLiveness::AliveHeartbeatStale => "active,alive-heartbeat-stale".to_string(),
        SupervisionLiveness::NotLive => format!("{}", agent.status).to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(status: AgentStatus, hb_age_secs: i64) -> Agent {
        let mut a = Agent::new("id-1".into(), "w1".into());
        a.role = AgentRole::Worker;
        a.status = status;
        a.last_heartbeat = chrono::Utc::now() - chrono::Duration::seconds(hb_age_secs);
        a
    }

    #[test]
    fn live_fresh_heartbeat_is_live() {
        let a = worker(AgentStatus::Active, 5);
        assert_eq!(
            evaluate_supervision_liveness_with(&a, false, WORKER_STALE_SECS),
            SupervisionLiveness::Live
        );
    }

    #[test]
    fn stale_heartbeat_without_process_is_not_live() {
        let a = worker(AgentStatus::Active, 60);
        assert_eq!(
            evaluate_supervision_liveness_with(&a, false, WORKER_STALE_SECS),
            SupervisionLiveness::NotLive
        );
    }

    #[test]
    fn stale_heartbeat_with_process_is_alive_stale() {
        let a = worker(AgentStatus::Active, 60);
        assert_eq!(
            evaluate_supervision_liveness_with(&a, true, WORKER_STALE_SECS),
            SupervisionLiveness::AliveHeartbeatStale
        );
    }

    #[test]
    fn registry_stale_with_process_is_alive_stale() {
        let a = worker(AgentStatus::Stale, 120);
        assert_eq!(
            evaluate_supervision_liveness_with(&a, true, WORKER_STALE_SECS),
            SupervisionLiveness::AliveHeartbeatStale
        );
    }

    #[test]
    fn shutdown_without_process_is_not_live() {
        let a = worker(AgentStatus::Shutdown, 0);
        assert_eq!(
            evaluate_supervision_liveness_with(&a, false, WORKER_STALE_SECS),
            SupervisionLiveness::NotLive
        );
    }

    #[test]
    fn agent_process_is_alive_with_no_pid_is_false() {
        let a = Agent::new("id".into(), "n".into());
        assert!(!agent_process_is_alive_with(&a, |_| true, |_, _| true));
    }

    #[test]
    fn agent_process_is_alive_with_pid_only() {
        let mut a = Agent::new("id".into(), "n".into());
        a.pid = Some(9);
        assert!(agent_process_is_alive_with(&a, |p| p == 9, |_, _| false));
        assert!(!agent_process_is_alive_with(&a, |_| false, |_, _| true));
    }

    #[test]
    fn pane_kept_when_unregistered_or_live() {
        assert!(should_keep_worker_pane("spawning", std::iter::empty()));
        let live = worker(AgentStatus::Active, 5);
        assert!(should_keep_worker_pane("w1", std::iter::once(&live)));
    }

    #[test]
    fn pane_dropped_when_all_matching_not_live() {
        let dead = worker(AgentStatus::Stale, 120);
        assert!(!should_keep_worker_pane("w1", std::iter::once(&dead)));
        let shutdown = worker(AgentStatus::Shutdown, 0);
        assert!(!should_keep_worker_pane("w1", std::iter::once(&shutdown)));
    }
}
