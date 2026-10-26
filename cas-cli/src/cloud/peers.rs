//! Peer supervisor discovery (cas-e477).
//!
//! Supervisors working the same GitHub repo register with Cassy Cloud under
//! the repo's canonical id, on one machine or many. This module reads that
//! registry back: the live supervisors of this repo other than the caller,
//! with machine, factory session, epic focus and heartbeat age.
//!
//! The list is asked for this project (`GET /api/agents?project=<id>`,
//! cloud PR #153, which also returns teammates' agents on a team project).
//! The repo filter is still applied here, on the identity each supervisor
//! registered in its metadata, so an older server that ignores `project`
//! gives the same answer. Liveness needs a recent `last_heartbeat` and a
//! `status` other than `stale`.
//!
//! Peers are read-only (cas-604d): nothing here spawns, messages, claims or
//! writes a peer into a local store or roster.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Utc};

use crate::cloud::coordinator::AgentInfo;
use crate::types::Agent;

/// Metadata key carrying the repo's canonical id (e.g. `github.com/o/r`).
pub const META_CANONICAL_ID: &str = "canonical_id";
/// Metadata key carrying the agent role (`supervisor`, `worker`, ...).
pub const META_ROLE: &str = "role";
/// Metadata key carrying the supervisor's epic focus.
pub const META_FOCUS: &str = "focus";
/// Metadata key carrying the title of the focused epic (cas-8522), so a
/// peer's declared ownership reads without a lookup in its store.
pub const META_FOCUS_TITLE: &str = "focus_title";
/// Metadata key carrying the factory session name.
pub const META_FACTORY_SESSION: &str = "factory_session";
/// Metadata key carrying the machine's hostname, for display.
pub const META_HOSTNAME: &str = "hostname";
/// Metadata key carrying the email of the Cassy Cloud user that registered
/// the agent (cas-d58d), so a peer can be labelled with its owner.
pub const META_OWNER: &str = "owner";
/// Metadata key carrying the branch checked out where the agent works
/// (cas-f707). Optional; empty when unknown or detached.
pub const META_BRANCH: &str = "branch";
/// Metadata key a supervisor publishes while its own harness refuses every
/// turn for an account reason (GH #1319): the harness's message, e.g.
/// "Login expired · Please run /login". Empty or absent when healthy.
pub const META_HARNESS_AUTH_FAILED: &str = "harness_auth_failed";

/// A peer is live while its last heartbeat is at most this old. Matches the
/// local supervisor liveness window in `worker_status`.
pub const PEER_LIVE_SECS: i64 = 300;
/// Peers whose last heartbeat is older than this are not listed at all.
pub const PEER_LISTING_WINDOW_SECS: i64 = 24 * 60 * 60;

/// One supervisor of the same repo, as registered in Cassy Cloud.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Peer {
    pub agent_id: String,
    pub name: String,
    /// Hostname when registered, else the machine id, else `unknown`.
    pub machine: String,
    pub machine_id: Option<String>,
    /// Factory session name.
    pub session: Option<String>,
    pub canonical_id: Option<String>,
    pub focus: Option<String>,
    /// Title of the focused epic, when the supervisor published it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_title: Option<String>,
    pub last_heartbeat: DateTime<Utc>,
    pub heartbeat_age_secs: i64,
    pub live: bool,
    /// The Cassy Cloud user that registered this supervisor (cas-2a5a). The
    /// account-wide directory lists, and the mailbox addresses, only peers
    /// whose user is the caller's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// Email of the registering user, when the registration carried it
    /// (cas-d58d).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The branch checked out where the supervisor works (cas-f707), which
    /// is the branch it delivers into (cas-a131).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Whether the caller's own account registered this supervisor:
    /// `None` when either account is unknown (cas-d58d).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub own: Option<bool>,
    /// GH #1319: the supervisor's harness cannot authenticate, so nothing it
    /// receives is read. Such a peer is never `live`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness_auth_failed: Option<String>,
}

impl Peer {
    /// The supervisor's declared ownership (cas-8522): its focused epic id
    /// and title, or `none`.
    pub fn epic_label(&self) -> String {
        match (self.focus.as_deref(), self.focus_title.as_deref()) {
            (Some(id), Some(title)) => format!("{id} \"{title}\""),
            (Some(id), None) => id.to_string(),
            (None, _) => "none".to_string(),
        }
    }

    /// The short project name people type (`violet` for
    /// `github.com/Richards-LLC/violet`), from the registered canonical id.
    pub fn project_name(&self) -> Option<String> {
        self.canonical_id.as_deref().and_then(project_name)
    }
}

/// The last path segment of a canonical id, without a `.git` suffix.
pub fn project_name(canonical_id: &str) -> Option<String> {
    let id = canonical_id.trim().trim_end_matches('/');
    let last = id.rsplit(['/', ':']).next()?.trim_end_matches(".git").trim();
    (!last.is_empty()).then(|| last.to_string())
}

/// This machine's label, as registration metadata ([`META_HOSTNAME`]) and
/// claim holders both record it: the hostname, else the generated machine
/// id. One function, so a claim and discovery name a machine the same way
/// (cas-f707).
pub fn machine_label() -> String {
    crate::cloud::device::DeviceConfig::hostname()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(crate::types::Agent::get_or_generate_machine_id)
}

/// The label of the machine running the project at `cas_root`: the
/// project's `[cloud] machine_label` when set, else [`machine_label`].
///
/// cas-c7c7: this one label is what people see and type for a machine:
/// registration publishes it as [`META_HOSTNAME`], claim holders record it,
/// and a received peer message shows the sender's. The registered
/// `machine_id` stays the machine's stable identity and is never displayed
/// in its place unless the label is unknown.
pub fn machine_label_for(cas_root: &Path) -> String {
    crate::config::Config::load(cas_root)
        .ok()
        .and_then(|config| config.cloud)
        .and_then(|cloud| cloud.machine_label)
        .map(|label| label.trim().to_string())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(machine_label)
}

/// The cloud metadata a registration carries so peers can find each other:
/// the agent's own metadata plus role, repo identity, focus, factory session
/// and hostname. Empty values are omitted.
pub fn identity_metadata(
    agent: &Agent,
    canonical_id: Option<&str>,
    focus: Option<&str>,
    hostname: Option<&str>,
) -> HashMap<String, String> {
    let mut metadata = agent.metadata.clone();
    let mut put = |key: &str, value: Option<&str>| {
        if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
            metadata.insert(key.to_string(), value.to_string());
        }
    };
    put(META_ROLE, Some(&agent.role.to_string().to_lowercase()));
    put(META_CANONICAL_ID, canonical_id);
    put(META_FOCUS, focus);
    put(META_FACTORY_SESSION, agent.factory_session.as_deref());
    put(META_HOSTNAME, hostname);
    metadata
}

/// The supervisors of `canonical_id` among `agents`, excluding `self_id`.
///
/// Repo identity is compared with the cloud's normalizer and the registered
/// alias class, so `git@github.com:o/r.git` and `github.com/o/r` match.
/// Shut-down agents and agents last seen more than
/// [`PEER_LISTING_WINDOW_SECS`] ago are omitted; the rest are listed live
/// first, with heartbeat age and liveness.
pub fn repo_peers(
    agents: &[AgentInfo],
    canonical_id: &str,
    alias_class: &[String],
    self_id: Option<&str>,
    now: DateTime<Utc>,
) -> Vec<Peer> {
    repo_rows(agents, canonical_id, alias_class, self_id, now, supervisor_peer)
}

fn repo_rows(
    agents: &[AgentInfo],
    canonical_id: &str,
    alias_class: &[String],
    self_id: Option<&str>,
    now: DateTime<Utc>,
    row: fn(&AgentInfo, DateTime<Utc>) -> Option<Peer>,
) -> Vec<Peer> {
    let mut peers: Vec<Peer> = agents
        .iter()
        .filter(|agent| Some(agent.id.as_str()) != self_id)
        .filter(|agent| {
            agent_meta(agent, META_CANONICAL_ID).is_some_and(|repo| {
                crate::cloud::project_ids_match_with_aliases(&repo, canonical_id, alias_class)
            })
        })
        .filter_map(|agent| row(agent, now))
        .collect();
    sort_peers(&mut peers);
    peers
}

fn agent_meta(agent: &AgentInfo, key: &str) -> Option<String> {
    agent
        .metadata
        .get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// `agent` as a listed peer: a supervisor that has not shut down and was
/// seen within [`PEER_LISTING_WINDOW_SECS`].
fn supervisor_peer(agent: &AgentInfo, now: DateTime<Utc>) -> Option<Peer> {
    if agent.status == "shutdown" {
        return None;
    }
    supervisor_row(agent, now)
}

/// GH #1319: `agent` as an ended supervisor (its session shut down) seen
/// within [`PEER_LISTING_WINDOW_SECS`]. Never listed; kept only so a message
/// addressed to it can reach its successor or be refused by name.
fn ended_supervisor(agent: &AgentInfo, now: DateTime<Utc>) -> Option<Peer> {
    if agent.status != "shutdown" {
        return None;
    }
    supervisor_row(agent, now).map(|peer| Peer { live: false, ..peer })
}

fn supervisor_row(agent: &AgentInfo, now: DateTime<Utc>) -> Option<Peer> {
    if agent_meta(agent, META_ROLE).as_deref() != Some("supervisor") {
        return None;
    }
    let last_heartbeat = DateTime::parse_from_rfc3339(&agent.last_heartbeat)
        .ok()?
        .with_timezone(&Utc);
    let heartbeat_age_secs = (now - last_heartbeat).num_seconds().max(0);
    (heartbeat_age_secs <= PEER_LISTING_WINDOW_SECS).then(|| Peer {
        agent_id: agent.id.clone(),
        name: agent.name.clone(),
        machine: agent_meta(agent, META_HOSTNAME)
            .or_else(|| agent.machine_id.clone())
            .unwrap_or_else(|| "unknown".to_string()),
        machine_id: agent.machine_id.clone(),
        session: agent_meta(agent, META_FACTORY_SESSION),
        canonical_id: agent_meta(agent, META_CANONICAL_ID),
        focus: agent_meta(agent, META_FOCUS),
        focus_title: agent_meta(agent, META_FOCUS).and(agent_meta(agent, META_FOCUS_TITLE)),
        last_heartbeat,
        heartbeat_age_secs,
        // The cloud computes `stale` live (cloud PR #153). A supervisor whose
        // harness is logged out heartbeats but reads nothing (GH #1319).
        live: heartbeat_age_secs <= PEER_LIVE_SECS
            && agent.status != "stale"
            && agent_meta(agent, META_HARNESS_AUTH_FAILED).is_none(),
        user_id: agent
            .user_id
            .as_deref()
            .map(str::trim)
            .filter(|user| !user.is_empty())
            .map(str::to_string),
        owner: agent_meta(agent, META_OWNER),
        branch: agent_meta(agent, META_BRANCH),
        own: None,
        harness_auth_failed: agent_meta(agent, META_HARNESS_AUTH_FAILED),
    })
}

/// Mark each peer as the viewer's own or another user's (cas-d58d). A peer
/// stays unmarked when either account is unknown.
pub fn mark_own(peers: &mut [Peer], viewer_user_id: Option<&str>) {
    for peer in peers {
        peer.own = viewer_user_id
            .zip(peer.user_id.as_deref())
            .map(|(viewer, owner)| viewer == owner);
    }
}

/// Split marked peers by the project's same-repo scope (cas-d58d): `team`
/// keeps all of them; `own` keeps only the viewer's own and returns the rest
/// (another user's, or an account that cannot be told) as hidden.
pub fn apply_same_repo_scope(
    peers: Vec<Peer>,
    scope: crate::config::SameRepoScope,
) -> (Vec<Peer>, Vec<Peer>) {
    match scope {
        crate::config::SameRepoScope::Team => (peers, Vec::new()),
        crate::config::SameRepoScope::Own => {
            peers.into_iter().partition(|peer| peer.own == Some(true))
        }
    }
}

/// The project's `peers.same_repo_scope` (default `team`).
pub fn same_repo_scope(cas_root: &Path) -> crate::config::SameRepoScope {
    crate::config::Config::load(cas_root)
        .ok()
        .and_then(|config| config.peers)
        .map(|peers| peers.same_repo_scope)
        .unwrap_or_default()
}

/// Who owns `peer`, for display (cas-d58d): `you`, the owner's email, a
/// short user id, or `unknown account`.
pub fn owner_label(peer: &Peer) -> String {
    if peer.own == Some(true) {
        return "you".to_string();
    }
    let who = peer
        .owner
        .clone()
        .or_else(|| {
            peer.user_id
                .as_deref()
                .map(|id| format!("user {}", id.chars().take(8).collect::<String>()))
        })
        .unwrap_or_else(|| "unknown account".to_string());
    if peer.own == Some(false) {
        format!("teammate {who}")
    } else {
        who
    }
}

/// The account viewing peers (cas-d58d): the user of the caller's own
/// registered row, else the single user of `own_agents` (an agent list asked
/// for without a project, which the cloud answers with the token's own
/// agents only). Never inferred from a project-visible row of another agent.
pub fn viewer_user_id(
    agents: &[AgentInfo],
    self_id: Option<&str>,
    own_agents: impl FnOnce() -> Option<Vec<AgentInfo>>,
) -> Option<String> {
    let user = |agent: &AgentInfo| {
        agent
            .user_id
            .as_deref()
            .map(str::trim)
            .filter(|user| !user.is_empty())
            .map(str::to_string)
    };
    if let Some(own) = self_id.and_then(|id| agents.iter().find(|agent| agent.id == id)) {
        return user(own);
    }
    let own_agents = own_agents()?;
    let users: std::collections::BTreeSet<_> = own_agents.iter().filter_map(user).collect();
    (users.len() == 1).then(|| users.into_iter().next()).flatten()
}

fn sort_peers(peers: &mut [Peer]) {
    peers.sort_by(|a, b| {
        b.live
            .cmp(&a.live)
            .then(a.heartbeat_age_secs.cmp(&b.heartbeat_age_secs))
            .then(a.name.cmp(&b.name))
    });
}

/// The account whose supervisors the caller may see (cas-2a5a): the user
/// that registered `self_id`, else the single user every listed row
/// belongs to. Fails closed: no user, or rows of more than one user with no
/// registered caller to decide between them, lists nobody.
pub fn caller_user_id(agents: &[AgentInfo], self_id: Option<&str>) -> Result<String, String> {
    let user = |agent: &AgentInfo| {
        agent
            .user_id
            .as_deref()
            .map(str::trim)
            .filter(|user| !user.is_empty())
            .map(str::to_string)
    };
    if let Some(own) = self_id.and_then(|id| agents.iter().find(|agent| agent.id == id)) {
        return user(own).ok_or_else(|| {
            "Cassy Cloud did not report which account registered this supervisor; \
             no cross-project peers are listed"
                .to_string()
        });
    }
    let users: std::collections::BTreeSet<_> = agents.iter().filter_map(user).collect();
    match users.len() {
        1 => Ok(users.into_iter().next().expect("one user")),
        0 => Err("Cassy Cloud listed no account for these agents; no cross-project peers \
                  are listed"
            .to_string()),
        _ => Err("Cassy Cloud listed agents of more than one account and this caller is not \
                  registered; no cross-project peers are listed"
            .to_string()),
    }
}

/// Every supervisor of `owner_user_id`, in any project and on any machine,
/// excluding `self_id` (cas-2a5a). A row of any other user, or with no user,
/// is never listed, even when the cloud returns it (a teammate on a shared
/// team project).
pub fn user_peers(
    agents: &[AgentInfo],
    owner_user_id: &str,
    self_id: Option<&str>,
    now: DateTime<Utc>,
) -> Vec<Peer> {
    user_rows(agents, owner_user_id, self_id, now, supervisor_peer)
}

fn user_rows(
    agents: &[AgentInfo],
    owner_user_id: &str,
    self_id: Option<&str>,
    now: DateTime<Utc>,
    row: fn(&AgentInfo, DateTime<Utc>) -> Option<Peer>,
) -> Vec<Peer> {
    let mut peers: Vec<Peer> = agents
        .iter()
        .filter(|agent| Some(agent.id.as_str()) != self_id)
        .filter_map(|agent| row(agent, now))
        .filter(|peer| peer.user_id.as_deref() == Some(owner_user_id))
        .collect();
    sort_peers(&mut peers);
    peers
}

/// `45s`, `15m`, `3h`, `2d`.
fn age(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// Human-readable lines for `worker_status`, `cas status` and
/// `coordination action=peers`.
pub fn render_peers(canonical_id: &str, peers: &[Peer]) -> String {
    // cas-f707: two short lines per peer, so the list fits 80 columns.
    let mut out =
        crate::cloud::presence::wrap_line(&format!("Peers ({canonical_id}, via Cassy Cloud):"));
    if peers.is_empty() {
        out.push_str("  no other supervisors of this repo seen in the last 24h\n");
        return out;
    }
    for peer in peers {
        let state = match (peer.live, peer.harness_auth_failed.is_some()) {
            (_, true) => "logged out",
            (true, false) => "live",
            (false, false) => "stale",
        };
        // cas-d58d: every peer names its owning account, on its own line so
        // a long email never pushes the session line past 80 columns.
        out.push_str(&format!(
            "  {} [{state}] on {}, heartbeat {} ago\n    owner {}\n    session {} | focus {}\n",
            peer.name,
            peer.machine,
            age(peer.heartbeat_age_secs),
            owner_label(peer),
            peer.session.as_deref().unwrap_or("-"),
            peer.epic_label(),
        ));
    }
    out
}

/// Where a plain same-repo message target goes (cas-d58d).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SameRepoTarget<'a> {
    /// A listed peer of this repo: deliver to it.
    Peer(&'a Peer),
    /// Another account's supervisor that `peers.same_repo_scope = own` hid:
    /// refuse with this message.
    Refused(String),
    /// No same-repo peer of that name: try other routes.
    NotFound,
}

/// Resolve `target` among the listed peers, then among the ones the `own`
/// scope hid, so a hidden teammate is refused by name.
pub fn resolve_same_repo_target<'a>(
    peers: &'a [Peer],
    hidden: &[Peer],
    target: &str,
) -> Result<SameRepoTarget<'a>, String> {
    if let Some(peer) = crate::cloud::peer_mailbox::resolve_peer_target(peers, target)? {
        return Ok(SameRepoTarget::Peer(peer));
    }
    Ok(match crate::cloud::peer_mailbox::resolve_peer_target(hidden, target) {
        Ok(Some(other)) => SameRepoTarget::Refused(format!(
            "'{target}' is {}@{}, a supervisor of {} (another Cassy Cloud account). This \
             project's peers.same_repo_scope is own, so only your own supervisors are \
             messaged. Nothing was sent.",
            other.name,
            other.machine,
            owner_label(other)
        )),
        // Ambiguous among hidden supervisors is still another account's.
        Err(_) => SameRepoTarget::Refused(format!(
            "'{target}' names supervisors of other Cassy Cloud accounts. This project's \
             peers.same_repo_scope is own, so only your own supervisors are messaged. \
             Nothing was sent."
        )),
        Ok(None) => SameRepoTarget::NotFound,
    })
}

/// Who reads the mailbox a resolved target names (GH #1319).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotHolder<'a> {
    /// The addressed supervisor reads its own mailbox (or is down without a
    /// successor and will claim it when it returns): send to it.
    Addressed,
    /// The addressed supervisor ended, or its harness is logged out, and one
    /// live supervisor of the same project on the same machine holds its
    /// slot. `note` tells the sender who got it instead.
    Successor { peer: &'a Peer, note: String },
    /// Nobody can read it. The reason names why; nothing is sent.
    Refused(String),
}

/// The most recent ended supervisor `target` names: an agent id, a name or
/// `name@machine` (GH #1319).
pub fn ended_target<'a>(ended: &'a [Peer], target: &str) -> Option<&'a Peer> {
    let target = target.trim();
    let target = target.strip_prefix("supervisor:").unwrap_or(target).trim();
    let (name, machine) = match target.rsplit_once('@') {
        Some((name, machine)) => (name, Some(machine)),
        None => (target, None),
    };
    ended
        .iter()
        .filter(|peer| {
            peer.agent_id.eq_ignore_ascii_case(target)
                || (peer.name.eq_ignore_ascii_case(name)
                    && machine.is_none_or(|machine| same_machine_label(peer, machine)))
        })
        .max_by_key(|peer| peer.last_heartbeat)
}

fn same_machine_label(peer: &Peer, machine: &str) -> bool {
    peer.machine.eq_ignore_ascii_case(machine)
        || peer
            .machine_id
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case(machine))
}

/// Whether `addressed` can read a message, and if not, which live supervisor
/// of the same project on the same machine holds its slot (GH #1319).
///
/// A supervisor that only went quiet keeps its address while no successor
/// exists: its daemon claims the message when it returns. One that ended, or
/// whose harness is logged out, never will, so the send is refused by name
/// rather than reported as delivered.
pub fn slot_holder<'a>(peers: &'a [Peer], addressed: &Peer, ended: bool) -> SlotHolder<'a> {
    let label = format!("{}@{}", addressed.name, addressed.machine);
    let why = if ended {
        format!(
            "{label} ended {} ago (its session shut down)",
            age(addressed.heartbeat_age_secs)
        )
    } else if let Some(failure) = addressed.harness_auth_failed.as_deref() {
        format!("{label}'s harness is logged out (\"{failure}\"), so it reads no messages")
    } else if !addressed.live {
        format!("{label} was last seen {} ago", age(addressed.heartbeat_age_secs))
    } else {
        return SlotHolder::Addressed;
    };
    let project_of = |peer: &Peer| {
        peer.canonical_id
            .as_deref()
            .and_then(crate::cloud::normalize_project_canonical_id)
    };
    let project = project_of(addressed);
    let same_project = |peer: &&Peer| {
        project.is_some()
            && project_of(peer) == project
            && peer.agent_id != addressed.agent_id
            && peer.live
    };
    let on_machine = |peer: &&Peer| {
        same_machine_label(peer, &addressed.machine)
            || addressed
                .machine_id
                .as_deref()
                .is_some_and(|id| same_machine_label(peer, id))
    };
    let successors: Vec<&Peer> = peers.iter().filter(same_project).filter(on_machine).collect();
    let project_name = addressed
        .project_name()
        .unwrap_or_else(|| "its project".to_string());
    match successors.as_slice() {
        [successor] => SlotHolder::Successor {
            peer: successor,
            note: format!(
                "{why}; delivered to its successor {}@{} instead.",
                successor.name, successor.machine
            ),
        },
        // Only quiet: it claims its own mail when it returns.
        _ if !ended && addressed.harness_auth_failed.is_none() => SlotHolder::Addressed,
        [] => {
            let elsewhere: Vec<String> = peers
                .iter()
                .filter(same_project)
                .map(|peer| format!("{}@{}", peer.name, peer.machine))
                .collect();
            let elsewhere = if elsewhere.is_empty() {
                String::new()
            } else {
                format!(" (live elsewhere: {}; address one by name)", elsewhere.join(", "))
            };
            SlotHolder::Refused(format!(
                "{why}, and no live supervisor of {project_name} runs on {} to take its \
                 messages{elsewhere}. Nothing was sent.",
                addressed.machine
            ))
        }
        many => SlotHolder::Refused(format!(
            "{why}, and {} live supervisors of {project_name} run on {}: {}. Address one by \
             name. Nothing was sent.",
            many.len(),
            addressed.machine,
            many.iter()
                .map(|peer| format!(
                    "{}@{} (session {})",
                    peer.name,
                    peer.machine,
                    peer.session.as_deref().unwrap_or("unknown")
                ))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The note under a same-repo list whose `own` scope hid other accounts'
/// supervisors (cas-d58d). Empty when nothing was hidden.
pub fn render_hidden_note(hidden: usize) -> String {
    if hidden == 0 {
        return String::new();
    }
    crate::cloud::presence::wrap_line(&format!(
        "  {hidden} supervisor{} of other accounts hidden: peers.same_repo_scope is own",
        if hidden == 1 { "" } else { "s" }
    ))
}

/// Per-request cap for a peer lookup; status views must not hang on the cloud.
pub const DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Result of a peer lookup for one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerDiscovery {
    /// Peers of `canonical_id` (possibly none), each marked as the
    /// caller's own or another user's.
    Found {
        canonical_id: String,
        peers: Vec<Peer>,
        /// Supervisors the `own` scope excluded (cas-d58d), so a send to one
        /// can be refused by name rather than falling through.
        hidden: Vec<Peer>,
        /// Live workers in the listed peers' sessions (cas-9f53), so a send
        /// to one names the supervisor to relay through.
        workers: Vec<crate::cloud::peers_detail::PeerWorkerRef>,
        /// Supervisors of this repo whose session ended (GH #1319), so a
        /// message to one reaches its successor or is refused by name.
        ended: Vec<Peer>,
    },
    /// Not logged in to Cassy Cloud: cross-machine peers are unknowable.
    NotLoggedIn,
    /// The project has no canonical id to scope peers by.
    NoProjectIdentity,
}

/// A plain operator terminal may have no harness session or registered
/// ancestor PID. Agents registered in this project's own store still belong
/// to this clone, so none of them are remote peers of the project.
pub(crate) fn exclude_project_agents(
    cas_root: &Path,
    peers: &mut Vec<Peer>,
) -> Result<(), crate::error::CasError> {
    let local_ids: std::collections::HashSet<_> = crate::store::open_agent_store(cas_root)?
        .list(None)?
        .into_iter()
        .map(|agent| agent.id)
        .collect();
    let retired = super::peer_claims::retired_local_agents(cas_root);
    peers.retain(|peer| !local_ids.contains(&peer.agent_id)
        && !retired.get(&peer.agent_id).is_some_and(|ended| peer.last_heartbeat <= ended.ended_at));
    Ok(())
}

/// Look up the peers of the project rooted at `cas_root` through Cassy Cloud.
pub fn discover_peers(
    cas_root: &Path,
    self_id: Option<&str>,
) -> Result<PeerDiscovery, crate::error::CasError> {
    let Some(canonical_id) = crate::cloud::resolve_canonical_id(cas_root) else {
        return Ok(PeerDiscovery::NoProjectIdentity);
    };
    // The project's own cloud config, as the daemon's coordinator loads it;
    // never the machine-wide login, so a project that is not syncing does not
    // reach the cloud here either.
    let config = crate::cloud::CloudConfig::load_from_cas_dir(cas_root)?;
    if !config.is_logged_in() {
        return Ok(PeerDiscovery::NotLoggedIn);
    }
    let alias_class = crate::cloud::project_aliases_from_config_toml(cas_root)
        .into_iter()
        .chain(std::iter::once(canonical_id.clone()))
        .collect::<Vec<_>>();
    // cas-f707: ask the cloud for this project (petra-stella-cloud#151).
    let agents = crate::cloud::CloudCoordinator::new(config.clone())?
        .with_timeout(DISCOVERY_TIMEOUT)
        .with_canonical_id(Some(canonical_id.clone()))
        .list_agent_infos()?;
    let mut peers = repo_peers(&agents, &canonical_id, &alias_class, self_id, Utc::now());
    exclude_project_agents(cas_root, &mut peers)?;
    let mut ended =
        repo_rows(&agents, &canonical_id, &alias_class, self_id, Utc::now(), ended_supervisor);
    exclude_project_agents(cas_root, &mut ended)?;
    // cas-d58d: label each peer with its account; the project's scope may
    // keep only the caller's own.
    // The token's own agent list is asked for only when it can decide
    // something: a peer that carries an account.
    let any_account = peers.iter().any(|peer| peer.user_id.is_some());
    let viewer = viewer_user_id(&agents, self_id, || {
        if !any_account {
            return None;
        }
        crate::cloud::CloudCoordinator::new(config)
            .ok()?
            .with_timeout(DISCOVERY_TIMEOUT)
            .list_agent_infos()
            .ok()
    });
    mark_own(&mut peers, viewer.as_deref());
    let (peers, hidden) = apply_same_repo_scope(peers, same_repo_scope(cas_root));
    let workers =
        crate::cloud::peers_detail::live_peer_workers(&peers, &agents, &alias_class, Utc::now());
    Ok(PeerDiscovery::Found {
        canonical_id,
        peers,
        hidden,
        workers,
        ended,
    })
}

/// Result of the account-wide supervisor lookup (cas-2a5a).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserPeerDiscovery {
    /// Every other supervisor of the caller's own Cassy Cloud user.
    Found {
        owner_user_id: String,
        peers: Vec<Peer>,
        /// The user's supervisors whose session ended (GH #1319).
        ended: Vec<Peer>,
    },
    /// Not logged in to Cassy Cloud: other projects' supervisors are unknowable.
    NotLoggedIn,
}

/// Look up every live or recent supervisor of the caller's own Cassy Cloud
/// user, in any project and on any machine (cas-2a5a).
///
/// The agent list is asked for without a project, which the cloud answers
/// with the caller's own agents only (`user_id = caller`); the user filter is
/// applied here as well, so a server that also returned a teammate's agents
/// still lists none of them.
pub fn discover_user_peers(
    cas_root: &Path,
    self_id: Option<&str>,
) -> Result<UserPeerDiscovery, crate::error::CasError> {
    let config = crate::cloud::CloudConfig::load_from_cas_dir(cas_root)?;
    if !config.is_logged_in() {
        return Ok(UserPeerDiscovery::NotLoggedIn);
    }
    let agents = crate::cloud::CloudCoordinator::new(config)?
        .with_timeout(DISCOVERY_TIMEOUT)
        .list_agent_infos()?;
    let owner = caller_user_id(&agents, self_id).map_err(crate::error::CasError::Other)?;
    let mut peers = user_peers(&agents, &owner, self_id, Utc::now());
    exclude_project_agents(cas_root, &mut peers)?;
    let mut ended = user_rows(&agents, &owner, self_id, Utc::now(), ended_supervisor);
    exclude_project_agents(cas_root, &mut ended)?;
    Ok(UserPeerDiscovery::Found {
        owner_user_id: owner,
        peers,
        ended,
    })
}

/// Human-readable lines for `coordination action=peers scope=all` and
/// `cas peers --all` (cas-2a5a): a verdict line, the live supervisors to
/// message, the stale ones collapsed to a count per project (listed in full
/// with `verbose`), and one copyable message command.
pub fn render_user_peers(peers: &[Peer], verbose: bool) -> String {
    use crate::cloud::presence::wrap_line;
    // One row per supervisor identity: a re-registration of the same name,
    // machine and session shows once, under its freshest heartbeat.
    let mut seen = std::collections::HashSet::new();
    let rows: Vec<&Peer> = peers
        .iter()
        .filter(|peer| {
            seen.insert((
                peer.name.to_ascii_lowercase(),
                peer.machine.to_ascii_lowercase(),
                peer.session.clone(),
            ))
        })
        .collect();
    let project = |peer: &Peer| peer.project_name().unwrap_or_else(|| "unknown".to_string());
    let projects: std::collections::BTreeSet<String> = rows.iter().map(|p| project(p)).collect();
    let (live, stale): (Vec<&Peer>, Vec<&Peer>) = rows.iter().partition(|peer| peer.live);
    let mut out = format!(
        "[OK] {} supervisor{} in {} project{} | {} live\n",
        rows.len(),
        if rows.len() == 1 { "" } else { "s" },
        projects.len(),
        if projects.len() == 1 { "" } else { "s" },
        live.len(),
    );
    if rows.is_empty() {
        out.push_str("  no other supervisors of your account seen in the last 24h\n");
        return out;
    }
    // cas-8522: each row declares what its supervisor owns.
    let row = |peer: &Peer| {
        // GH #1319: a logged-out harness heartbeats but reads nothing.
        let logged_out = if peer.harness_auth_failed.is_some() {
            " | harness logged out"
        } else {
            ""
        };
        format!(
            "  {} {} on {}, seen {} ago\n    session {} | epic {}{logged_out}\n",
            peer.name,
            project(peer),
            peer.machine,
            age(peer.heartbeat_age_secs),
            peer.session.as_deref().unwrap_or("-"),
            peer.epic_label(),
        )
    };
    if live.is_empty() {
        out.push_str("Live: none\n");
    } else {
        out.push_str("Live:\n");
        for peer in &live {
            out.push_str(&row(peer));
        }
    }
    if !stale.is_empty() {
        if verbose {
            out.push_str("Stale, seen in the last 24h:\n");
            for peer in &stale {
                out.push_str(&row(peer));
            }
        } else {
            let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
            for peer in &stale {
                *counts.entry(project(peer)).or_default() += 1;
            }
            let counts = counts
                .iter()
                .map(|(project, count)| format!("{project} {count}"))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&wrap_line(&format!(
                "Stale, seen in the last 24h ({}): {counts}. List them with --verbose or --json.",
                stale.len()
            )));
        }
    }
    // One copyable command, to a live supervisor if any: by project when
    // that names exactly one live supervisor, else by name@machine.
    if let Some(peer) = live.first().or(stale.first()) {
        let name = project(peer);
        let unique =
            name != "unknown" && live.iter().filter(|other| project(other) == name).count() <= 1;
        let target = if unique {
            format!("supervisor:{name}")
        } else if let Some(epic) = peer.focus.as_deref() {
            format!("supervisor:{epic}")
        } else {
            format!("{}@{}", peer.name, peer.machine)
        };
        out.push_str(&wrap_line(&format!(
            "Message one: coordination action=message target={target} summary=\"<one line>\" \
             message=\"<text>\""
        )));
    }
    out.push_str(&wrap_line(
        "Cross-project messages are advisory: never urgent, never the operator.",
    ));
    out
}

/// `coordination action=peers scope=all` and `cas peers --all`: the
/// account-wide directory as text, or as one JSON document (cas-2a5a). A
/// failed lookup is an error in both forms, so the CLI exits non-zero.
pub fn user_peers_report(
    cas_root: &Path,
    self_id: Option<&str>,
    json: bool,
    verbose: bool,
) -> Result<String, crate::error::CasError> {
    let discovery = discover_user_peers(cas_root, self_id).map_err(|error| {
        crate::error::CasError::Other(format!("Cassy Cloud lookup failed: {error}"))
    })?;
    if !json {
        return Ok(match discovery {
            UserPeerDiscovery::Found { peers, .. } => render_user_peers(&peers, verbose),
            UserPeerDiscovery::NotLoggedIn => crate::cloud::presence::wrap_line(
                "[WARN] Not logged in to Cassy Cloud, so your other projects' supervisors are \
                 unknown. Run `cas login`.",
            ),
        });
    }
    let value = match discovery {
        UserPeerDiscovery::Found { peers, .. } => serde_json::json!({
            "scope": "user",
            "status": "ready",
            "peers": peers
                .iter()
                .map(|peer| {
                    let mut value = serde_json::to_value(peer).unwrap_or_default();
                    value["project"] = serde_json::json!(peer.project_name());
                    value
                })
                .collect::<Vec<_>>(),
            "limitations": [
                "Only supervisors of your own Cassy Cloud account are listed.",
                "Messages across projects are advisory: never urgent, never the operator.",
            ],
        }),
        UserPeerDiscovery::NotLoggedIn => serde_json::json!({
            "scope": "user",
            "status": "not_logged_in",
            "peers": [],
        }),
    };
    serde_json::to_string_pretty(&value)
        .map_err(|error| crate::error::CasError::Other(error.to_string()))
}

/// The Peers section shown by `coordination action=peers`,
/// `factory action=worker_status` and `cas status --verbose`. Blocking (one
/// HTTP round trip per page, [`DISCOVERY_TIMEOUT`] each); a failure is
/// reported in the section, never raised. It opens with the caller's own
/// cloud presence (cas-f707) and ends with the peer mailbox status (cas-13c7).
pub fn peers_section(cas_root: &Path, self_id: Option<&str>) -> String {
    let mut section = presence_line(cas_root, self_id);
    section.push_str(&peers_discovery_section(cas_root, self_id));
    append_mailbox_status(cas_root, self_id, &mut section);
    section
}

/// The caller's own registration state, from the record the daemon keeps
/// (cas-f707). Empty when the caller is unknown or the project does not
/// reach Cassy Cloud.
pub(crate) fn presence_line(cas_root: &Path, self_id: Option<&str>) -> String {
    let logged_in = crate::cloud::CloudConfig::load_from_cas_dir(cas_root)
        .is_ok_and(|config| config.is_logged_in());
    if !logged_in || crate::cloud::resolve_canonical_id(cas_root).is_none() {
        return String::new();
    }
    // An explicit `cloud.auto_sync = false` keeps this project's agents out
    // of the registry; discovery still reads it, so it is one-way.
    if crate::cloud::presence::opted_out(cas_root) {
        return format!(
            "{}{}",
            crate::cloud::presence::disabled_lines(),
            crate::cloud::presence::wrap_line(
                "Peers: discovery is one-way: this project still lists peers that \
                 registered, but they cannot see its supervisors.",
            )
        );
    }
    let shutdown = crate::cloud::shutdown::health(cas_root, Utc::now());
    let Some(self_id) = self_id else { return shutdown; };
    let record = crate::cloud::presence::load(cas_root, self_id);
    format!("{}{shutdown}", crate::cloud::presence::render_line(record.as_ref(), Utc::now()))
}

fn peers_discovery_section(cas_root: &Path, self_id: Option<&str>) -> String {
    match discover_peers(cas_root, self_id) {
        Ok(PeerDiscovery::Found {
            canonical_id,
            peers,
            hidden,
            ..
        }) => format!(
            "{}{}",
            render_peers(&canonical_id, &peers),
            render_hidden_note(hidden.len())
        ),
        Ok(PeerDiscovery::NotLoggedIn) => crate::cloud::presence::wrap_line(
            "Peers: not logged in to Cassy Cloud, so supervisors on other machines are \
             unknown (`cas login`).",
        ),
        Ok(PeerDiscovery::NoProjectIdentity) => crate::cloud::presence::wrap_line(
            "Peers: this project has no canonical id to match peers by (set [project] \
             canonical_id or a git origin).",
        ),
        Err(error) => {
            crate::cloud::presence::wrap_line(&format!("Peers: Cassy Cloud lookup failed: {error}"))
        }
    }
}

/// After the peers: the epic splits agreed with them (cas-4736), then the
/// mailbox footer. Both are read fresh from this clone, never cached with
/// the discovery.
pub(crate) fn append_mailbox_status(cas_root: &Path, self_id: Option<&str>, section: &mut String) {
    section.push_str(&crate::cloud::epic_split::render_splits(cas_root));
    append_mailbox_health(cas_root, self_id, section);
}

pub(crate) fn append_mailbox_health(cas_root: &Path, self_id: Option<&str>, section: &mut String) {
    let agents = crate::store::open_agent_store(cas_root).ok();
    if let Some(id) = self_id {
        let session = agents
            .and_then(|store| store.get(id).ok())
            .and_then(|agent| agent.factory_session);
        section.push_str(&crate::cloud::peer_mailbox::mailbox_status_section(
            cas_root,
            session.as_deref(),
        ));
        return;
    }
    // The CLI has no caller id. Show the live factory sessions on this clone,
    // rather than suggesting a logged-in daemon cannot receive.
    let mut sessions = agents
        .and_then(|store| store.list(None).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|a| {
            a.role == crate::types::AgentRole::Supervisor
                && matches!(
                    a.status,
                    crate::types::AgentStatus::Active | crate::types::AgentStatus::Idle
                )
        })
        .filter_map(|a| a.factory_session)
        .collect::<Vec<_>>();
    sessions.sort();
    sessions.dedup();
    if sessions.is_empty() {
        section.push_str(&crate::cloud::peer_mailbox::mailbox_status_section(
            cas_root, None,
        ));
    } else {
        for session in sessions {
            section.push_str(&crate::cloud::peer_mailbox::mailbox_status_section(
                cas_root,
                Some(&session),
            ));
        }
    }
}

/// How long [`peers_section_cached`] reuses a lookup.
pub const PEERS_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// [`peers_section`], with the cloud lookup reused for [`PEERS_CACHE_TTL`]
/// per project and caller so a supervisor polling `worker_status` does not
/// query the cloud on every call. The presence line is always read fresh.
pub fn peers_section_cached(cas_root: &Path, self_id: Option<&str>) -> String {
    type Key = (std::path::PathBuf, Option<String>);
    static CACHE: std::sync::Mutex<Option<HashMap<Key, (std::time::Instant, String)>>> =
        std::sync::Mutex::new(None);
    let presence = presence_line(cas_root, self_id);
    let key = (cas_root.to_path_buf(), self_id.map(str::to_string));
    if let Some((at, text)) = CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|cache| cache.get(&key))
        && at.elapsed() < PEERS_CACHE_TTL
    {
        let mut text = format!("{presence}{text}");
        append_mailbox_status(cas_root, self_id, &mut text);
        return text;
    }
    let text = peers_discovery_section(cas_root, self_id);
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(key, (std::time::Instant::now(), text.clone()));
    let mut text = format!("{presence}{text}");
    append_mailbox_status(cas_root, self_id, &mut text);
    text
}

/// The agent a CLI invocation runs under, so `cas status` does not list its
/// own session as a peer (cas-f707): the harness session id in the
/// environment, else the agent registered for one of this process's
/// ancestors (the harness that ran the shell that ran `cas`).
pub fn cli_self_id(cas_root: &Path) -> Option<String> {
    let from_env = ["CAS_SESSION_ID", "CLAUDE_CODE_SESSION_ID"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty());
    if from_env.is_some() {
        return from_env;
    }
    let store = crate::store::open_agent_store(cas_root).ok()?;
    ancestor_pids(6)
        .into_iter()
        .find_map(|pid| store.get_by_pid(pid).ok().flatten())
        .map(|agent| agent.id)
}

/// Up to `depth` ancestors of this process, nearest first.
#[allow(unused_mut)]
fn ancestor_pids(depth: usize) -> Vec<u32> {
    let mut pids = Vec::new();
    #[cfg(unix)]
    {
        let mut pid = std::os::unix::process::parent_id();
        while pid > 1 && pids.len() < depth {
            pids.push(pid);
            // Linux: the parent is field 4 of /proc/<pid>/stat, after the
            // parenthesised command name. Elsewhere only the parent is known.
            let Some(next) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|stat| {
                    let rest = &stat[stat.rfind(')')? + 1..];
                    rest.split_whitespace().nth(1)?.parse::<u32>().ok()
                })
            else {
                break;
            };
            pid = next;
        }
    }
    let _ = depth;
    pids
}

#[cfg(test)]
#[path = "peers_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "peers_scope_tests.rs"]
mod scope_tests;
