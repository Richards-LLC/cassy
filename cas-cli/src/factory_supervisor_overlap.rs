//! Detect two or more live supervisor sessions sharing one clone.
//!
//! # Why this exists (GH #699)
//!
//! Everything a supervisor does to a fleet — `reset`, `worktree_merge`,
//! `shutdown_workers`, `clear_context`, spawning over a name — reaches through
//! one clone's `.cas/` state. When a second supervisor session starts on the
//! same checkout, both operate on that shared state, and same-name
//! registration supersession (cas-ef8b) lets either one adopt or reap the
//! other's workers. Nothing said so: `cas doctor` checked each factory session
//! *in isolation* ("this session has exactly 1 supervisor: ok"), and
//! `worker_status` filters agents to the caller's own session, so the second
//! supervisor was shown only itself and told its worker roster was empty.
//!
//! # The key is the clone, not a recorded path
//!
//! Agent rows carry no clone path, and they do not need one: every surface
//! here reads the agent store of one `.cas/` directory, so *every* live
//! supervisor row it returns is by construction sharing this clone's state.
//! The path is used for the message, never for matching.
//!
//! Sessions, not rows, are the unit. Two supervisor rows inside one factory
//! session are a different fault (a duplicate registration) already reported by
//! doctor's per-session check, so grouping by session keeps the two signals
//! from double-reporting each other. A live supervisor with no factory session
//! is still its own process on this clone, so each such row forms its own
//! group.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};

use crate::types::{Agent, AgentRole};

/// How fresh a supervisor's heartbeat must be to count as live here.
///
/// Supervisors heartbeat on the daemon's 30s tick. Five minutes tolerates a
/// pane blocked on a long turn without keeping a supervisor that exited hours
/// ago in the report — a false "two supervisors" warning would train operators
/// to ignore the real one.
pub const SUPERVISOR_LIVE_SECS: i64 = 300;

/// One live supervisor session observed on this clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSupervisorSession {
    /// `CAS_FACTORY_SESSION` value, or `None` for a supervisor registered
    /// outside a factory session.
    pub session: Option<String>,
    /// Supervisor agent name (`noble-koala-5`).
    pub name: String,
    /// Supervisor agent id.
    pub id: String,
    /// When this supervisor registered — the "start time" an operator needs to
    /// tell the incumbent from the newcomer.
    pub started_at: DateTime<Utc>,
    /// Last heartbeat, for the same judgement call.
    pub last_heartbeat: DateTime<Utc>,
}

impl LiveSupervisorSession {
    /// `session/name` when the supervisor belongs to a factory session, bare
    /// name otherwise. This is the label the operator sees in every surface.
    pub fn label(&self) -> String {
        match self.session.as_deref() {
            Some(session) => format!("{session}/{}", self.name),
            None => format!("{} (no factory session)", self.name),
        }
    }

    fn describe(&self) -> String {
        format!(
            "{} (started {})",
            self.label(),
            self.started_at.format("%Y-%m-%dT%H:%M:%SZ")
        )
    }
}

/// Live supervisor sessions on this clone, oldest first.
///
/// One entry per factory session: when a session somehow holds several live
/// supervisor rows, the earliest-registered one represents it, because the
/// duplicate-registration fault is reported separately and repeating it here
/// would bury the cross-session hazard.
pub fn live_supervisor_sessions(agents: &[Agent], now: DateTime<Utc>) -> Vec<LiveSupervisorSession> {
    let mut by_session: BTreeMap<String, LiveSupervisorSession> = BTreeMap::new();

    for agent in agents {
        if agent.role != AgentRole::Supervisor || !agent.is_alive() {
            continue;
        }
        if (now - agent.last_heartbeat).num_seconds() > SUPERVISOR_LIVE_SECS {
            continue;
        }

        let session = agent
            .factory_session
            .as_deref()
            .map(str::trim)
            .filter(|session| !session.is_empty())
            .map(ToOwned::to_owned);
        // A sessionless supervisor is its own process; key it by agent id so
        // two of them are two groups rather than one collapsed row.
        let key = match session.as_deref() {
            Some(session) => format!("session:{session}"),
            None => format!("agent:{}", agent.id),
        };
        let candidate = LiveSupervisorSession {
            session,
            name: agent.name.clone(),
            id: agent.id.clone(),
            started_at: agent.registered_at,
            last_heartbeat: agent.last_heartbeat,
        };
        by_session
            .entry(key)
            .and_modify(|existing| {
                if candidate.started_at < existing.started_at {
                    *existing = candidate.clone();
                }
            })
            .or_insert(candidate);
    }

    let mut sessions: Vec<_> = by_session.into_values().collect();
    sessions.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.name.cmp(&b.name)));
    sessions
}

/// The operator-facing warning when more than one live supervisor session
/// shares this clone, or `None` for the ordinary single-supervisor case.
///
/// `clone_root` is the `.cas` directory; its parent is shown when resolvable
/// so the message names the checkout an operator recognizes.
pub fn shared_clone_warning(
    agents: &[Agent],
    clone_root: &Path,
    now: DateTime<Utc>,
) -> Option<String> {
    let sessions = live_supervisor_sessions(agents, now);
    if sessions.len() < 2 {
        return None;
    }
    Some(render_shared_clone_warning(&sessions, clone_root))
}

/// Formatting half of [`shared_clone_warning`], separated so every surface
/// renders the same sentence and tests can pin it without building a store.
pub fn render_shared_clone_warning(sessions: &[LiveSupervisorSession], clone_root: &Path) -> String {
    let checkout = clone_root
        .file_name()
        .and_then(|name| (name == ".cas").then(|| clone_root.parent()))
        .flatten()
        .unwrap_or(clone_root);
    let described: Vec<String> = sessions.iter().map(LiveSupervisorSession::describe).collect();
    format!(
        "{} live supervisors share this clone ({}): {} — reset, worktree_merge, shutdown_workers or a spawn from either session can reap the other's workers",
        sessions.len(),
        checkout.display(),
        described.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentRole, AgentStatus};
    use chrono::Duration;

    fn supervisor(name: &str, session: Option<&str>, age_secs: i64, heartbeat_secs: i64) -> Agent {
        let now = Utc::now();
        let mut agent = Agent::new(format!("id-{name}"), name.to_string());
        agent.role = AgentRole::Supervisor;
        agent.status = AgentStatus::Active;
        agent.factory_session = session.map(ToOwned::to_owned);
        agent.registered_at = now - Duration::seconds(age_secs);
        agent.last_heartbeat = now - Duration::seconds(heartbeat_secs);
        agent
    }

    #[test]
    fn one_live_supervisor_is_not_a_hazard() {
        let agents = vec![
            supervisor("noble-koala-5", Some("gabber-gentle-hawk-71"), 600, 5),
            {
                let mut worker = Agent::new("id-w".to_string(), "zen-newt-93".to_string());
                worker.role = AgentRole::Worker;
                worker.factory_session = Some("gabber-gentle-hawk-71".to_string());
                worker
            },
        ];
        assert_eq!(live_supervisor_sessions(&agents, Utc::now()).len(), 1);
        assert!(shared_clone_warning(&agents, Path::new("/repo/.cas"), Utc::now()).is_none());
    }

    #[test]
    fn two_live_sessions_warn_and_name_both_with_start_times() {
        let agents = vec![
            supervisor("gentle-falcon-66", Some("gabber-witty-panda-98"), 3600, 24),
            supervisor("noble-koala-5", Some("gabber-gentle-hawk-71"), 120, 5),
        ];
        let warning = shared_clone_warning(
            &agents,
            Path::new("/home/pippenz/Petrastella/gabber-studio/.cas"),
            Utc::now(),
        )
        .expect("two live supervisor sessions must warn");

        assert!(warning.contains("2 live supervisors share this clone"));
        assert!(warning.contains("/home/pippenz/Petrastella/gabber-studio"));
        assert!(warning.contains("gabber-witty-panda-98/gentle-falcon-66"));
        assert!(warning.contains("gabber-gentle-hawk-71/noble-koala-5"));
        assert!(warning.contains("started "));
        assert!(warning.contains("reap the other's workers"));
        // Oldest first: the incumbent is named before the newcomer.
        let incumbent = warning.find("gentle-falcon-66").expect("incumbent listed");
        let newcomer = warning.find("noble-koala-5").expect("newcomer listed");
        assert!(incumbent < newcomer, "sessions must read oldest first");
    }

    #[test]
    fn a_supervisor_whose_heartbeat_lapsed_is_not_counted() {
        let agents = vec![
            supervisor("gentle-falcon-66", Some("gabber-witty-panda-98"), 7200, 3600),
            supervisor("noble-koala-5", Some("gabber-gentle-hawk-71"), 120, 5),
        ];
        let live = live_supervisor_sessions(&agents, Utc::now());
        assert_eq!(live.len(), 1, "stale supervisor must not raise the warning");
        assert_eq!(live[0].name, "noble-koala-5");
        assert!(shared_clone_warning(&agents, Path::new("/repo/.cas"), Utc::now()).is_none());
    }

    #[test]
    fn duplicate_rows_in_one_session_are_a_single_session() {
        let mut duplicate = supervisor("noble-koala-5", Some("gabber-gentle-hawk-71"), 60, 5);
        duplicate.id = "id-duplicate".to_string();
        let agents = vec![
            supervisor("noble-koala-5", Some("gabber-gentle-hawk-71"), 600, 5),
            duplicate,
        ];
        assert_eq!(
            live_supervisor_sessions(&agents, Utc::now()).len(),
            1,
            "the duplicate-registration fault is doctor's per-session check, not this one"
        );
        assert!(shared_clone_warning(&agents, Path::new("/repo/.cas"), Utc::now()).is_none());
    }

    #[test]
    fn sessionless_supervisors_each_count_as_their_own_process() {
        let agents = vec![
            supervisor("solo-one", None, 600, 5),
            supervisor("solo-two", None, 60, 5),
        ];
        let warning = shared_clone_warning(&agents, Path::new("/repo/.cas"), Utc::now())
            .expect("two sessionless supervisors still share the clone");
        assert!(warning.contains("solo-one (no factory session)"));
        assert!(warning.contains("solo-two (no factory session)"));
    }

    #[test]
    fn shutdown_and_stale_supervisor_rows_are_ignored() {
        let mut gone = supervisor("gentle-falcon-66", Some("gabber-witty-panda-98"), 600, 5);
        gone.status = AgentStatus::Shutdown;
        let mut stale = supervisor("brave-otter-7", Some("gabber-brave-otter"), 600, 5);
        stale.status = AgentStatus::Stale;
        let agents = vec![
            gone,
            stale,
            supervisor("noble-koala-5", Some("gabber-gentle-hawk-71"), 120, 5),
        ];
        assert!(shared_clone_warning(&agents, Path::new("/repo/.cas"), Utc::now()).is_none());
    }
}
