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

use crate::types::{Agent, AgentRole, Task, TaskStatus, TaskType};

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

/// Whether `source` names a **registered supervisor agent** on this clone
/// (cas-15f2).
///
/// Deliberately a roster lookup and not a string test: `prompt_queue.source` is
/// caller-settable (`cas factory message --from …`, bridge `POST /message`), so
/// a name that merely *looks* like a supervisor's proves nothing. Resolving it
/// to a row whose role is `Supervisor` is what makes the peer-wake allowance
/// safe — an arbitrary client can spell any string into `source`, but it cannot
/// register itself as a supervisor.
///
/// Deliberately unscoped by session: the whole point is the OTHER session's
/// supervisor, which is why `agents` must come from an unscoped roster read.
pub fn names_a_registered_supervisor(agents: &[Agent], source: &str) -> bool {
    agents
        .iter()
        .any(|agent| agent.role == AgentRole::Supervisor && agent.name.eq_ignore_ascii_case(source))
}

/// Whether a queued row addressed to `supervisor_name`'s pane is a *peer
/// supervisor's* message, the one class cas-15f2 made wake-eligible.
///
/// Two supervisors sharing a clone have no other channel to each other, and an
/// inbox-only row is discovered by polling — which is how a release gate went
/// uncoordinated on 2026-09-04, both messages dying at
/// `abandoned_unknown_target` with `delivery_attempts=0`. A supervisor's own
/// outbound rows are excluded: nothing should wake a pane with its own echo.
///
/// `source_is_supervisor` must come from [`names_a_registered_supervisor`], not
/// from the caller-settable source string.
pub fn is_peer_supervisor_message(
    source: &str,
    supervisor_name: &str,
    source_is_supervisor: bool,
) -> bool {
    source_is_supervisor && !source.eq_ignore_ascii_case(supervisor_name)
}

/// What `worker_status` can say about the epic a live supervisor is running
/// (cas-5087).
///
/// "Which supervisor is live on this clone" was only half the answer an
/// operator needs before a gate: the other half is *what each of them is in the
/// middle of*, because that is what decides whether a merge, a reset or a
/// shutdown collides with somebody's release. The three states are kept
/// distinct on purpose — a supervisor that has declared no epic and a task
/// store that could not be read this pass are different facts, and collapsing
/// them into one blank field is how a status page starts lying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorEpic {
    /// This supervisor's current epic.
    Running {
        /// Epic task id (`cas-cfd3`).
        id: String,
        /// Epic title when the task store could resolve it.
        title: Option<String>,
    },
    /// No epic is pinned for this supervisor's session and none names it as
    /// verification owner. Advisory, never an error.
    NoEpic,
    /// The task store could not be read on this pass. `worker_status` is what
    /// an operator checks BEFORE a gate, so it renders the gap instead of
    /// failing or — worse — rendering it as "no epic".
    Unavailable,
}

/// How much of an epic title `worker_status` prints before eliding. Long enough
/// to recognize a release epic, short enough that a five-supervisor clone stays
/// one line per supervisor.
pub const SUPERVISOR_EPIC_TITLE_MAX: usize = 60;

impl SupervisorEpic {
    /// The suffix appended to a supervisor's `worker_status` row. Always
    /// non-empty: a supervisor with nothing to report says so.
    pub fn render(&self) -> String {
        match self {
            Self::Running { id, title } => match title {
                Some(title) => format!(" — epic {id}: {}", elide(title, SUPERVISOR_EPIC_TITLE_MAX)),
                None => format!(" — epic {id}"),
            },
            Self::NoEpic => " — no epic".to_string(),
            Self::Unavailable => " — epic unknown (task store unreadable)".to_string(),
        }
    }
}

fn elide(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// Resolve the epic one live supervisor is running (cas-5087).
///
/// `focused_epic_id` is that supervisor's session focus pin — the explicit
/// "this is what I am running" declaration, so it wins. A pin that no longer
/// resolves in `candidate_epics` is still reported by id: the supervisor said
/// it, and dropping the row because the title lookup missed would hide the
/// exact case an operator is trying to see.
///
/// The fallback is ownership: the most recently updated non-closed epic whose
/// `epic_verification_owner` is this agent. Matching accepts the agent id or
/// its name because both are written into that field by different paths.
///
/// `candidate_epics` may legitimately be empty (no epics on this clone); an
/// UNREADABLE task store is a different fact and is the caller's to report via
/// [`SupervisorEpic::Unavailable`].
pub fn resolve_supervisor_epic(
    agent_id: &str,
    agent_name: &str,
    focused_epic_id: Option<&str>,
    candidate_epics: &[Task],
) -> SupervisorEpic {
    let focused = focused_epic_id.map(str::trim).filter(|id| !id.is_empty());
    if let Some(id) = focused {
        let title = candidate_epics
            .iter()
            .find(|epic| epic.id == id)
            .map(|epic| epic.title.clone())
            .filter(|title| !title.trim().is_empty());
        return SupervisorEpic::Running {
            id: id.to_string(),
            title,
        };
    }

    let owned = candidate_epics
        .iter()
        .filter(|epic| epic.task_type == TaskType::Epic && epic.status != TaskStatus::Closed)
        .filter(|epic| {
            epic.epic_verification_owner
                .as_deref()
                .map(str::trim)
                .filter(|owner| !owner.is_empty())
                .is_some_and(|owner| {
                    owner.eq_ignore_ascii_case(agent_id) || owner.eq_ignore_ascii_case(agent_name)
                })
        })
        .max_by_key(|epic| epic.updated_at);

    match owned {
        Some(epic) => SupervisorEpic::Running {
            id: epic.id.clone(),
            title: Some(epic.title.clone()).filter(|title| !title.trim().is_empty()),
        },
        None => SupervisorEpic::NoEpic,
    }
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

    fn epic(id: &str, title: &str, owner: Option<&str>) -> Task {
        let mut task = Task::new(id.to_string(), title.to_string());
        task.task_type = TaskType::Epic;
        task.status = TaskStatus::InProgress;
        task.epic_verification_owner = owner.map(ToOwned::to_owned);
        task
    }

    /// The pin is the supervisor's own declaration of what it is running, so it
    /// outranks ownership inference.
    #[test]
    fn a_session_focus_pin_names_the_epic_with_its_title() {
        let epics = vec![
            epic("cas-cfd3", "EPIC: update follow-ups (fixture)", None),
            epic("cas-other", "EPIC: something else", Some("id-noble-koala-5")),
        ];

        let resolved =
            resolve_supervisor_epic("id-noble-koala-5", "noble-koala-5", Some("cas-cfd3"), &epics);

        assert_eq!(
            resolved,
            SupervisorEpic::Running {
                id: "cas-cfd3".to_string(),
                title: Some("EPIC: update follow-ups (fixture)".to_string()),
            },
            "the pin must beat the owned epic"
        );
        assert!(resolved.render().contains("epic cas-cfd3: EPIC: update follow-ups (fixture)"));
    }

    /// A pin whose epic is not in the candidate list still names the epic. The
    /// title is a nicety; the id is the answer.
    #[test]
    fn a_pin_the_task_store_cannot_resolve_still_names_the_epic_id() {
        let resolved = resolve_supervisor_epic("id-a", "a", Some("cas-gone"), &[]);
        assert_eq!(
            resolved,
            SupervisorEpic::Running {
                id: "cas-gone".to_string(),
                title: None
            }
        );
        assert_eq!(resolved.render(), " — epic cas-gone");
    }

    /// Without a pin, the epic this supervisor is verification owner of is the
    /// answer — and the most recently touched one when it owns several.
    #[test]
    fn ownership_is_the_fallback_and_the_freshest_owned_epic_wins() {
        let mut old = epic("cas-old", "EPIC: last week", Some("id-noble-koala-5"));
        old.updated_at = Utc::now() - Duration::days(7);
        let mut current = epic("cas-now", "EPIC: this week", Some("noble-koala-5"));
        current.updated_at = Utc::now();
        let unrelated = epic("cas-theirs", "EPIC: another lane", Some("id-gentle-falcon-66"));

        let resolved = resolve_supervisor_epic(
            "id-noble-koala-5",
            "noble-koala-5",
            None,
            &[old, current, unrelated],
        );

        assert_eq!(
            resolved,
            SupervisorEpic::Running {
                id: "cas-now".to_string(),
                title: Some("EPIC: this week".to_string()),
            },
            "owner match must accept the agent name as well as its id"
        );
    }

    /// A closed epic is not what anyone is running, and a supervisor with
    /// nothing pinned or owned must render cleanly rather than as a blank field.
    #[test]
    fn a_supervisor_with_no_live_epic_says_so() {
        let mut closed = epic("cas-done", "EPIC: shipped", Some("id-a"));
        closed.status = TaskStatus::Closed;

        let resolved = resolve_supervisor_epic("id-a", "a", None, &[closed]);

        assert_eq!(resolved, SupervisorEpic::NoEpic);
        assert_eq!(resolved.render(), " — no epic");
    }

    /// An unreadable task store must not be reported as "no epic" — that is a
    /// claim, and worker_status is read immediately before gates.
    #[test]
    fn an_unreadable_task_store_is_distinguishable_from_no_epic() {
        assert_ne!(
            SupervisorEpic::Unavailable.render(),
            SupervisorEpic::NoEpic.render()
        );
        assert!(
            SupervisorEpic::Unavailable
                .render()
                .contains("task store unreadable")
        );
    }

    /// A five-supervisor clone must stay one line per supervisor.
    #[test]
    fn a_long_epic_title_is_elided() {
        let long = "EPIC: ".to_string() + &"x".repeat(200);
        let rendered = SupervisorEpic::Running {
            id: "cas-long".to_string(),
            title: Some(long),
        }
        .render();
        assert!(rendered.ends_with('…'), "{rendered}");
        assert!(
            rendered.chars().count() <= SUPERVISOR_EPIC_TITLE_MAX + 24,
            "row must stay short: {rendered}"
        );
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
