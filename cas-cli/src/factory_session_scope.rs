//! cas-bebc (GH #1023 finding 9): fleet mutations stay inside the factory
//! session that owns the target.
//!
//! GH #699 made a second live supervisor on one clone visible, and cas-9771
//! (GH #734) routed messages and lifecycle relays to the owning session. The
//! mutating actions still resolved their targets against the whole clone:
//! a `task reset` could force-release another session's worker's task,
//! `worktree_merge` could integrate another session's branch, and
//! `spawn_workers worker_names=<name>` could register over another session's
//! live worker, which is how same-name supersession reaps it. Most other
//! factory actions already filter their roster with
//! [`Agent::visible_to_factory_session`]; this module is the shared ownership
//! check for the paths that name a target directly.
//!
//! A target is **foreign** only when all of these hold:
//! - the caller runs inside a factory session (`CAS_FACTORY_SESSION`);
//! - the target resolves to a live worker whose factory session differs, and
//!   no live worker of the caller's own session answers to the same name;
//! - that other session still has a live supervisor. A session whose
//!   supervisor is gone leaves orphans, and recovering them from here stays
//!   allowed.
//!
//! Reads (`worker_status`, `epic_status`, `qa_status`) are never checked.

use std::path::Path;

use chrono::{DateTime, Utc};

use crate::factory_supervisor_overlap::live_supervisor_sessions;
use crate::types::{Agent, AgentRole, Task};

/// The session that owns a target the caller may not mutate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignOwner {
    /// The worker the target resolved to.
    pub worker: String,
    /// Its factory session.
    pub session: String,
    /// That session's live supervisor.
    pub supervisor: String,
}

impl ForeignOwner {
    /// The refusal every guarded action returns. It names both sessions so
    /// the operator knows which pane can act.
    pub fn refusal(&self, action: &str, caller_session: &str) -> String {
        format!(
            "{action} refused: worker {worker} belongs to factory session {session} \
             (supervisor {supervisor}), not to this session ({caller_session}). Two supervisors \
             share this clone, and one session must not mutate or reap the other's workers. \
             Act on it from {session}, or ask {supervisor} to. Nothing was changed.",
            worker = self.worker,
            session = self.session,
            supervisor = self.supervisor,
        )
    }
}

/// The caller's factory session, from its environment.
pub fn caller_factory_session() -> Option<String> {
    std::env::var("CAS_FACTORY_SESSION")
        .ok()
        .map(|session| session.trim().to_string())
        .filter(|session| !session.is_empty())
}

fn session_of(agent: &Agent) -> Option<&str> {
    agent
        .factory_session
        .as_deref()
        .map(str::trim)
        .filter(|session| !session.is_empty())
}

fn answers_to(agent: &Agent, target: &str) -> bool {
    agent.name.eq_ignore_ascii_case(target) || agent.id == target
}

/// The foreign owner of the worker named (or identified) by `target`.
pub fn foreign_owner_of_worker(
    agents: &[Agent],
    caller_session: Option<&str>,
    target: &str,
    now: DateTime<Utc>,
) -> Option<ForeignOwner> {
    let caller = caller_session?;
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    let live_workers: Vec<&Agent> = agents
        .iter()
        .filter(|agent| agent.role == AgentRole::Worker && agent.is_alive())
        .filter(|agent| answers_to(agent, target))
        .collect();
    // The caller's own worker wins a name shared across sessions.
    if live_workers
        .iter()
        .any(|agent| session_of(agent) == Some(caller))
    {
        return None;
    }
    let supervisors = live_supervisor_sessions(agents, now);
    live_workers
        .iter()
        .filter_map(|agent| {
            let session = session_of(agent)?;
            if session == caller {
                return None;
            }
            let supervisor = supervisors
                .iter()
                .find(|live| live.session.as_deref() == Some(session))?;
            Some(ForeignOwner {
                worker: agent.name.clone(),
                session: session.to_string(),
                supervisor: supervisor.name.clone(),
            })
        })
        .max_by(|a, b| a.worker.cmp(&b.worker))
}

/// The worker a factory branch belongs to: `factory/<worker>` or the
/// per-task form `factory/<worker>-<task>`. The longest matching registered
/// worker name wins, so `factory/ab-cd-cas-1` resolves to `ab-cd`, not `ab`.
pub fn worker_for_branch<'a>(agents: &'a [Agent], branch: &str) -> Option<&'a str> {
    let branch = branch
        .trim()
        .trim_start_matches("refs/heads/")
        .trim_start_matches("refs/remotes/")
        .trim_start_matches("origin/");
    let rest = branch.strip_prefix("factory/").unwrap_or(branch);
    agents
        .iter()
        .filter(|agent| agent.role == AgentRole::Worker)
        .map(|agent| agent.name.as_str())
        .filter(|name| {
            rest.eq_ignore_ascii_case(name)
                || rest
                    .get(..name.len() + 1)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(&format!("{name}-")))
        })
        .max_by_key(|name| name.len())
}

/// The foreign owner of a merge target: the worker or branch `id` names, or
/// the task's assignee or parked branch.
pub fn foreign_owner_of_merge(
    agents: &[Agent],
    caller_session: Option<&str>,
    id: &str,
    task: Option<&Task>,
    now: DateTime<Utc>,
) -> Option<ForeignOwner> {
    caller_session?;
    let mut targets: Vec<String> = Vec::new();
    match worker_for_branch(agents, id) {
        Some(worker) => targets.push(worker.to_string()),
        None => targets.push(id.trim_start_matches("factory/").to_string()),
    }
    if let Some(task) = task {
        targets.extend(task.assignee.clone());
        if let Some(worker) = task
            .deliverables
            .parked_branch
            .as_deref()
            .and_then(|branch| worker_for_branch(agents, branch))
        {
            targets.push(worker.to_string());
        }
    }
    targets
        .iter()
        .find_map(|target| foreign_owner_of_worker(agents, caller_session, target, now))
}

/// Store-reading form for the MCP handlers: the caller's session comes from
/// the environment and the roster from `cas_root`. Unreadable state yields
/// `None`: the existing guards on each path still apply.
pub fn foreign_owner_in_store(
    cas_root: &Path,
    resolve: impl FnOnce(&[Agent], Option<&str>, DateTime<Utc>) -> Option<ForeignOwner>,
) -> Option<(ForeignOwner, String)> {
    let caller = caller_factory_session()?;
    let agents = crate::store::open_agent_store(cas_root)
        .ok()?
        .list(None)
        .ok()?;
    resolve(&agents, Some(&caller), Utc::now()).map(|owner| (owner, caller))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AgentStatus;

    const A: &str = "gabber-studio-lively-jaguar-83";
    const B: &str = "gabber-studio-loyal-koala-48";

    fn agent(name: &str, role: AgentRole, session: Option<&str>) -> Agent {
        let mut agent = Agent::new_with_role(format!("id-{name}"), name.to_string(), role);
        agent.factory_session = session.map(str::to_string);
        agent.status = AgentStatus::Active;
        agent
    }

    /// The GH #734 shape: two live sessions on one clone.
    fn clone_with_two_sessions() -> Vec<Agent> {
        vec![
            agent("clever-lion-53", AgentRole::Supervisor, Some(A)),
            agent("path-delivery", AgentRole::Worker, Some(A)),
            agent("sharp-koala-25", AgentRole::Supervisor, Some(B)),
            agent("mib-hotfix", AgentRole::Worker, Some(B)),
        ]
    }

    #[test]
    fn another_sessions_live_worker_is_foreign_and_named() {
        let agents = clone_with_two_sessions();
        let owner = foreign_owner_of_worker(&agents, Some(A), "mib-hotfix", Utc::now())
            .expect("session B's worker is foreign to session A");
        assert_eq!(
            owner,
            ForeignOwner {
                worker: "mib-hotfix".to_string(),
                session: B.to_string(),
                supervisor: "sharp-koala-25".to_string(),
            }
        );
        let refusal = owner.refusal("task reset of cas-7fc5", A);
        assert!(refusal.contains(B) && refusal.contains(A), "{refusal}");
        assert!(refusal.contains("sharp-koala-25"), "{refusal}");
        // By agent id too, and symmetric from session B.
        assert!(foreign_owner_of_worker(&agents, Some(A), "id-mib-hotfix", Utc::now()).is_some());
        let reverse =
            foreign_owner_of_worker(&agents, Some(B), "path-delivery", Utc::now()).unwrap();
        assert_eq!(reverse.session, A);
        assert_eq!(reverse.supervisor, "clever-lion-53");
    }

    #[test]
    fn own_unknown_orphaned_and_sessionless_targets_are_not_foreign() {
        let mut agents = clone_with_two_sessions();
        let now = Utc::now();
        assert_eq!(
            foreign_owner_of_worker(&agents, Some(A), "path-delivery", now),
            None
        );
        assert_eq!(
            foreign_owner_of_worker(&agents, Some(A), "nobody", now),
            None
        );
        // No caller session: operator CLI, not a factory supervisor.
        assert_eq!(
            foreign_owner_of_worker(&agents, None, "mib-hotfix", now),
            None
        );
        // A shut-down worker is no longer anyone's live worker.
        agents[3].status = AgentStatus::Shutdown;
        assert_eq!(
            foreign_owner_of_worker(&agents, Some(A), "mib-hotfix", now),
            None
        );
        agents[3].status = AgentStatus::Active;
        // Session B's supervisor is gone: its workers are orphans to recover.
        agents[2].last_heartbeat = now
            - chrono::Duration::seconds(
                crate::factory_supervisor_overlap::SUPERVISOR_LIVE_SECS + 60,
            );
        assert_eq!(
            foreign_owner_of_worker(&agents, Some(A), "mib-hotfix", now),
            None
        );
        // A legacy worker with no session is not claimed by anyone.
        let legacy = vec![
            agent("sharp-koala-25", AgentRole::Supervisor, Some(B)),
            agent("old-worker", AgentRole::Worker, None),
        ];
        assert_eq!(
            foreign_owner_of_worker(&legacy, Some(A), "old-worker", now),
            None
        );
    }

    #[test]
    fn a_name_shared_across_sessions_resolves_to_the_callers_own_worker() {
        let mut agents = clone_with_two_sessions();
        agents.push(agent("mib-hotfix", AgentRole::Worker, Some(A)));
        assert_eq!(
            foreign_owner_of_worker(&agents, Some(A), "mib-hotfix", Utc::now()),
            None
        );
    }

    #[test]
    fn branches_and_tasks_resolve_to_their_worker() {
        let mut agents = clone_with_two_sessions();
        agents.push(agent("mib", AgentRole::Worker, Some(A)));
        assert_eq!(
            worker_for_branch(&agents, "factory/mib-hotfix"),
            Some("mib-hotfix")
        );
        assert_eq!(
            worker_for_branch(&agents, "origin/factory/mib-hotfix-cas-7fc5"),
            Some("mib-hotfix")
        );
        assert_eq!(worker_for_branch(&agents, "factory/mib-cas-1"), Some("mib"));
        assert_eq!(worker_for_branch(&agents, "epic/x"), None);

        let now = Utc::now();
        let by_branch =
            foreign_owner_of_merge(&agents, Some(A), "factory/mib-hotfix-cas-7fc5", None, now)
                .expect("session B's per-task branch");
        assert_eq!(by_branch.worker, "mib-hotfix");
        assert_eq!(
            foreign_owner_of_merge(&agents, Some(A), "path-delivery", None, now),
            None
        );

        let mut task = Task::new("cas-7fc5".to_string(), "MIB hotfix".to_string());
        task.assignee = Some("mib-hotfix".to_string());
        let by_task = foreign_owner_of_merge(&agents, Some(A), "epic-lane", Some(&task), now)
            .expect("the task's assignee belongs to session B");
        assert_eq!(by_task.session, B);
    }
}
