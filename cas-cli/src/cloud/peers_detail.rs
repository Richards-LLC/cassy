//! Read-only peer work awareness (cas-559c). These observations never enter
//! the local agent registry or the supervisor-only messaging/spawn roster.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::coordinator::AgentInfo;
use super::peers::{self, Peer};
use crate::error::CasError;
use crate::store::TaskStore;
use crate::types::{LeaseStatus, TaskLease};

#[path = "peers_detail_work.rs"]
mod work;
pub use work::{ObservationStatus, PeerPr, PeerSplit, PeerTask, PeerWork, WaitingMerge};
pub(crate) use work::{presence_agent, save_pr_snapshot};

#[derive(Debug, Clone, Serialize)]
pub struct PeerClaim {
    pub task_id: String,
    pub lock_key: String,
    pub agent_id: String,
    pub agent_name: Option<String>,
    pub title: Option<String>,
    /// Status from this project's synced task store, when available.
    pub status: Option<String>,
    /// The cloud lease protocol exposes renewal time, not initial claim time.
    pub age_secs: i64,
    pub renewed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerWorker {
    pub agent_id: String,
    pub name: String,
    pub status: String,
    pub branch: Option<String>,
    pub heartbeat_age_secs: i64,
    pub claims: Vec<PeerClaim>,
    pub machine: String,
    pub observed_at: DateTime<Utc>,
    pub worktree_root: Option<String>,
    pub scratch_root: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerDetail {
    pub outcome: String,
    pub owner: String,
    pub supervisor: Peer,
    pub user_id: Option<String>,
    pub team_id: Option<String>,
    pub project_id: Option<String>,
    pub claims_visibility: ClaimsVisibility,
    pub branch: Option<String>,
    /// Active, unexpired claims held by live supervisors and workers.
    pub claims: Vec<PeerClaim>,
    /// Live workers only, within the peer's machine and factory session.
    pub workers: Vec<PeerWorker>,
    #[serde(flatten)]
    pub work: PeerWork,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimsVisibility {
    SameAccount,
    DifferentAccount,
    /// Project-scoped Cloud reads authorize this teammate's locks (#155).
    TeamProject,
    Unknown,
}

impl ClaimsVisibility {
    pub(crate) fn visible(self) -> bool {
        matches!(self, Self::SameAccount | Self::TeamProject)
    }
}

fn claims_visibility(viewer: Option<&str>, owner: Option<&AgentInfo>) -> ClaimsVisibility {
    match (viewer, owner.and_then(|a| a.user_id.as_deref())) {
        (Some(viewer), Some(owner)) if viewer == owner => ClaimsVisibility::SameAccount,
        (_, Some(_))
            if owner
                .and_then(|a| a.team_id.as_deref())
                .is_some_and(|id| !id.trim().is_empty()) =>
        {
            ClaimsVisibility::TeamProject
        }
        (Some(_), Some(_)) => ClaimsVisibility::DifferentAccount,
        _ => ClaimsVisibility::Unknown,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerEpicSplit {
    pub id: String,
    pub epic_id: String,
    pub peer_name: String,
    pub peer_machine: String,
    pub direction: &'static str,
    pub mine: Vec<String>,
    pub theirs: Vec<String>,
    /// Stored state; expiry does not mutate the record.
    pub state: &'static str,
    pub display_state: &'static str,
    pub reason: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerReport {
    #[cfg(feature = "mcp-proxy")]
    pub stakeholder_contacts: serde_json::Value,
    pub status: String,
    /// Original Cloud activation read for this registration; status-only, never authority.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activation: Option<serde_json::Value>,
    pub observed_at: DateTime<Utc>,
    pub limitations: Vec<String>,
    pub cloud_presence: Option<super::presence::PresenceRecord>,
    pub cloud_presence_message: String,
    pub mailbox_status: String,
    pub epic_splits_status: ObservationStatus,
    pub epic_splits: Vec<PeerEpicSplit>,
    pub canonical_id: Option<String>,
    pub detail: bool,
    pub peers: Vec<PeerDetail>,
    /// Same-repo supervisors of other accounts that `peers.same_repo_scope =
    /// own` left out of `peers` (cas-d58d).
    pub hidden_peers: usize,
    /// In-flight branches of yours and a peer's that change the same files
    /// on the way into the same target (cas-a131). Informational only.
    pub overlaps: Vec<super::peer_merge::BranchOverlap>,
    pub error: Option<String>,
}

impl PeerReport {
    pub(crate) fn new(cas_root: &Path, self_id: Option<&str>, detail: bool) -> Self {
        let mut mailbox_status = String::new();
        peers::append_mailbox_health(cas_root, self_id, &mut mailbox_status);
        let now = Utc::now();
        let splits = cas_store::list_epic_splits(cas_root);
        let epic_splits_status = ObservationStatus::at(
            if splits.is_ok() {
                "ready"
            } else {
                "unavailable"
            },
            Some(now),
            splits
                .as_ref()
                .err()
                .map(|_| "local split records unavailable"),
        );
        let epic_splits = splits
            .unwrap_or_default()
            .into_iter()
            .map(|split| PeerEpicSplit {
                state: split.state.as_str(),
                display_state: split.display_state(now),
                direction: split.direction.as_str(),
                id: split.id,
                epic_id: split.epic_id,
                peer_name: split.peer_name,
                peer_machine: split.peer_machine,
                mine: split.mine,
                theirs: split.theirs,
                reason: split.reason,
                expires_at: split.expires_at,
                decided_at: split.decided_at,
            })
            .collect();
        Self {
            #[cfg(feature = "mcp-proxy")]
            stakeholder_contacts: serde_json::json!({"state":"not_requested"}),
            activation: None,
            observed_at: now,
            limitations: vec![
                "Read-only: shows conflicts; does not prevent them.".into(),
                "Remote names are observations, not local spawn targets.".into(),
            ],
            cloud_presence: self_id.and_then(|id| super::presence::load(cas_root, id)),
            cloud_presence_message: peers::presence_line(cas_root, self_id),
            mailbox_status,
            epic_splits_status,
            epic_splits,
            canonical_id: super::resolve_canonical_id(cas_root),
            detail,
            status: "ready".into(),
            peers: Vec::new(),
            hidden_peers: 0,
            overlaps: Vec::new(),
            error: None,
        }
    }
}

fn meta<'a>(agent: &'a AgentInfo, key: &str) -> Option<&'a str> {
    agent
        .metadata
        .get(key)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
}

fn heartbeat_age(agent: &AgentInfo, now: DateTime<Utc>) -> Option<i64> {
    DateTime::parse_from_rfc3339(&agent.last_heartbeat)
        .ok()
        .map(|at| (now - at.with_timezone(&Utc)).num_seconds().max(0))
}

/// Cloud task keys carry a repository hash. Only unwrap keys from this
/// identity/alias class; legacy unscoped task IDs retain their spelling.
fn task_id_for_peer<'a>(key: &'a str, peer: &Peer, aliases: &[String]) -> Option<&'a str> {
    let Some((task_id, suffix)) = key.rsplit_once('~') else {
        return Some(key);
    };
    if suffix.len() != 8 || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(key);
    }
    aliases
        .iter()
        .map(String::as_str)
        .chain(peer.canonical_id.as_deref())
        .any(|repo| super::peer_claims::claim_key(repo, task_id) == key)
        .then_some(task_id)
}

/// Require both a session and a machine boundary: session names alone can
/// collide across clones. Agents without enough identity are not guessed in.
fn belongs_to_peer(
    agent: &AgentInfo,
    peer: &Peer,
    owner: Option<&AgentInfo>,
    aliases: &[String],
) -> bool {
    meta(agent, peers::META_ROLE) == Some("worker")
        && agent.status != "shutdown"
        && peer.session.is_some()
        && owner.is_none_or(|owner| {
            owner
                .user_id
                .as_ref()
                .is_none_or(|id| agent.user_id.as_ref() == Some(id))
                && owner
                    .team_id
                    .as_ref()
                    .is_none_or(|id| agent.team_id.as_ref() == Some(id))
        })
        && meta(agent, peers::META_FACTORY_SESSION) == peer.session.as_deref()
        && match (&agent.machine_id, &peer.machine_id) {
            (Some(worker_machine), Some(peer_machine)) => worker_machine == peer_machine,
            (None, None) => {
                meta(agent, peers::META_HOSTNAME) == Some(peer.machine.as_str())
                    && peer.machine != "unknown"
            }
            _ => false,
        }
        && meta(agent, peers::META_CANONICAL_ID).is_some_and(|repo| {
            peer.canonical_id.as_deref().is_some_and(|peer_repo| {
                super::project_ids_match_with_aliases(repo, peer_repo, aliases)
            })
        })
}

/// A live worker in a peer supervisor's factory session (cas-9f53), as
/// `peers detail=true` lists it. It takes instructions only from its own
/// supervisor, so a message for it is relayed through that supervisor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerWorkerRef {
    pub name: String,
    pub machine: String,
    pub supervisor: Peer,
}

impl PeerWorkerRef {
    /// Whether `target` (`name` or `name@machine`) addresses this worker.
    pub fn matches(&self, target: &str) -> bool {
        let (name, machine) = match target.trim().rsplit_once('@') {
            Some((name, machine)) => (name, Some(machine)),
            None => (target.trim(), None),
        };
        self.name.eq_ignore_ascii_case(name)
            && machine.is_none_or(|machine| {
                self.machine.eq_ignore_ascii_case(machine)
                    || self
                        .supervisor
                        .machine_id
                        .as_deref()
                        .is_some_and(|id| id.eq_ignore_ascii_case(machine))
            })
    }
}

/// The live workers `peers detail=true` shows under each supervisor: same
/// membership and liveness rules, so a name it lists is resolvable here.
pub fn live_peer_workers(
    supervisors: &[Peer],
    agents: &[AgentInfo],
    aliases: &[String],
    now: DateTime<Utc>,
) -> Vec<PeerWorkerRef> {
    supervisors
        .iter()
        .flat_map(|supervisor| {
            let owner = agents.iter().find(|a| a.id == supervisor.agent_id);
            agents
                .iter()
                .filter(move |agent| belongs_to_peer(agent, supervisor, owner, aliases))
                .filter(move |agent| {
                    matches!(agent.status.as_str(), "active" | "idle")
                        && heartbeat_age(agent, now).is_some_and(|age| age <= peers::PEER_LIVE_SECS)
                })
                .map(move |agent| PeerWorkerRef {
                    name: agent.name.clone(),
                    machine: supervisor.machine.clone(),
                    supervisor: supervisor.clone(),
                })
        })
        .collect()
}

/// Assemble observations from a single agents snapshot and active leases.
/// Task titles/statuses come only from the current project's synced store.
pub fn assemble_peer_details(
    supervisors: Vec<Peer>,
    agents: &[AgentInfo],
    locks: &[TaskLease],
    aliases: &[String],
    tasks: &dyn TaskStore,
    viewer_user_id: Option<&str>,
    now: DateTime<Utc>,
) -> Vec<PeerDetail> {
    supervisors
        .into_iter()
        .map(|supervisor| {
            let owner = agents.iter().find(|a| a.id == supervisor.agent_id);
            let visibility = claims_visibility(viewer_user_id, owner);
            let members: Vec<_> = agents
                .iter()
                .filter(|agent| belongs_to_peer(agent, &supervisor, owner, aliases))
                .collect();
            let mut claims: Vec<_> = locks
                .iter()
                .filter(|_| visibility.visible())
                .filter(|lock| lock.status == LeaseStatus::Active && lock.expires_at > now)
                .filter(|lock| {
                    agents
                        .iter()
                        .find(|agent| agent.id == lock.agent_id)
                        .is_some_and(|agent| {
                            agent.status != "shutdown"
                                && ((now - lock.renewed_at).num_seconds() <= peers::PEER_LIVE_SECS
                                    || heartbeat_age(agent, now)
                                        .is_some_and(|age| age <= peers::PEER_LIVE_SECS))
                        })
                })
                .filter(|lock| {
                    lock.agent_id == supervisor.agent_id
                        || members.iter().any(|m| m.id == lock.agent_id)
                })
                .filter_map(|lock| {
                    let task_id = task_id_for_peer(&lock.task_id, &supervisor, aliases)?;
                    let task = tasks.get(task_id).ok();
                    Some(PeerClaim {
                        task_id: task_id.to_string(),
                        lock_key: lock.task_id.clone(),
                        agent_id: lock.agent_id.clone(),
                        agent_name: agents
                            .iter()
                            .find(|a| a.id == lock.agent_id)
                            .map(|a| a.name.clone()),
                        title: task.as_ref().map(|t| t.title.clone()),
                        status: task.map(|t| t.status.to_string()),
                        age_secs: (now - lock.renewed_at).num_seconds().max(0),
                        renewed_at: lock.renewed_at,
                        expires_at: lock.expires_at,
                    })
                })
                .collect();
            claims.sort_by(|a, b| a.task_id.cmp(&b.task_id).then(a.agent_id.cmp(&b.agent_id)));
            let mut workers: Vec<_> = members
                .into_iter()
                .filter_map(|agent| {
                    let age = heartbeat_age(agent, now)?;
                    (matches!(agent.status.as_str(), "active" | "idle")
                        && age <= peers::PEER_LIVE_SECS)
                        .then(|| PeerWorker {
                            agent_id: agent.id.clone(),
                            name: agent.name.clone(),
                            status: agent.status.clone(),
                            branch: meta(agent, "branch").map(str::to_string),
                            heartbeat_age_secs: age,
                            machine: supervisor.machine.clone(),
                            observed_at: now - chrono::Duration::seconds(age),
                            worktree_root: meta(agent, "clone_path").map(str::to_string),
                            scratch_root: meta(agent, "scratch_root").map(str::to_string),
                            claims: claims
                                .iter()
                                .filter(|c| c.agent_id == agent.id)
                                .cloned()
                                .collect(),
                        })
                })
                .collect();
            workers.sort_by(|a, b| a.name.cmp(&b.name).then(a.agent_id.cmp(&b.agent_id)));
            let branch = agents
                .iter()
                .find(|a| a.id == supervisor.agent_id)
                .and_then(|a| meta(a, "branch"))
                .map(str::to_string);
            let mut detail = PeerDetail {
                outcome: peer_outcome(&supervisor).into(),
                owner: supervisor.name.clone(),
                work: PeerWork::new(&supervisor, owner, visibility, now),
                supervisor,
                user_id: owner.and_then(|a| a.user_id.clone()),
                team_id: owner.and_then(|a| a.team_id.clone()),
                project_id: owner.and_then(|a| a.project_id.clone()),
                claims_visibility: visibility,
                branch,
                claims,
                workers,
            };
            work::inventory(&mut detail, owner, tasks, agents, aliases, now);
            detail
        })
        .collect()
}

/// Fetch full details only when requested. Discovery and locks are project
/// scoped, including teammates, and filtered against the discovered owners.
/// Missing lock evidence is an error rather than an invented empty claim list.
pub fn discover_peer_report(
    cas_root: &Path,
    self_id: Option<&str>,
    detail: bool,
    selected: Option<&str>,
) -> Result<PeerReport, CasError> {
    let detail = detail || selected.is_some();
    let mut report = PeerReport::new(cas_root, self_id, detail);
    #[cfg(feature = "mcp-proxy")]
    if detail {
        report.stakeholder_contacts = crate::coordination::stakeholder_contacts::StakeholderContacts::from_project(cas_root).status();
    }
    let Some(repo) = report.canonical_id.as_deref() else {
        report.status = "no_project_identity".into();
        report.error = Some("project has no canonical id; configure [project] canonical_id".into());
        return Ok(report);
    };
    let config = super::CloudConfig::load_from_cas_dir(cas_root)?;
    if !config.is_logged_in() {
        report.status = "not_logged_in".into();
        report.error = Some("not logged in to Cassy Cloud; run cas login".into());
        return Ok(report);
    }
    let aliases: Vec<_> = super::project_aliases_from_config_toml(cas_root)
        .into_iter()
        .chain(std::iter::once(repo.to_string()))
        .collect();
    let coordinator = super::CloudCoordinator::new(config.clone())?
        .with_canonical_id(Some(repo.to_string()))
        .with_timeout(peers::DISCOVERY_TIMEOUT);
    let agents = match coordinator.list_agent_infos() {
        Ok(agents) => agents,
        Err(error) => {
            report.status = "lookup_failed".into();
            report.error = Some(error.to_string());
            return Ok(report);
        }
    };
    // Without project= the server returns only agents owned by this token.
    // Never infer account authority from a project-visible self row: it may
    // be absent (operator CLI) or belong to another team member.
    let own_agents = super::CloudCoordinator::new(config)?
        .with_timeout(peers::DISCOVERY_TIMEOUT)
        .list_agent_infos()?;
    let own_users: std::collections::BTreeSet<_> = own_agents
        .iter()
        .filter_map(|a| a.user_id.as_deref())
        .filter(|id| !id.trim().is_empty())
        .collect();
    let viewer_user_id = (own_users.len() == 1)
        .then(|| own_users.first().copied())
        .flatten();
    let now = Utc::now();
    let mut supervisors = peers::repo_peers(&agents, repo, &aliases, self_id, now);
    peers::exclude_project_agents(cas_root, &mut supervisors)?;
    // cas-d58d: every peer names its account; `own` keeps only the viewer's.
    peers::mark_own(&mut supervisors, viewer_user_id);
    let (mut supervisors, hidden) =
        peers::apply_same_repo_scope(supervisors, peers::same_repo_scope(cas_root));
    report.hidden_peers = hidden.len();
    if let Some(selected) = selected
        && super::peer_mailbox::resolve_peer_target(&supervisors, selected)
            .map_err(CasError::Other)?
            .is_none()
        && let Some(other) = super::peer_mailbox::resolve_peer_target(&hidden, selected)
            .map_err(CasError::Other)?
    {
        return Err(CasError::Other(format!(
            "Peer supervisor '{selected}' belongs to {} (another Cassy Cloud account); \
             peers.same_repo_scope is own, so only your own supervisors are shown",
            peers::owner_label(other)
        )));
    }
    // cas-a131: same-file overlap between in-flight branches into the same
    // target, before any narrowing to one selected peer.
    report.overlaps = in_flight_overlaps(cas_root, &supervisors, &agents, &aliases, now);
    if let Some(selected) = selected {
        let peer = super::peer_mailbox::resolve_peer_target(&supervisors, selected)
            .map_err(CasError::Other)?
            .ok_or_else(|| {
                CasError::Other(format!(
                    "Peer supervisor '{selected}' not found in this repository"
                ))
            })?;
        supervisors = vec![peer.clone()];
    }
    if detail {
        // #155: the Cloud project visibility rule includes teammates' locks.
        // An unreadable list is unknown, never an invented empty inventory.
        let locks = if supervisors.is_empty() {
            Ok(Vec::new())
        } else {
            coordinator.list_locks()
        };
        // Awareness must not run the cloud outbox's repair-on-open writer.
        let tasks = crate::store::open_task_store_local(cas_root)?;
        report.peers = assemble_peer_details(
            supervisors,
            &agents,
            locks.as_deref().unwrap_or_default(),
            &aliases,
            tasks.as_ref(),
            viewer_user_id,
            now,
        );
        if let Err(error) = locks {
            report.status = "partial".into();
            for peer in &mut report.peers {
                if peer.claims_visibility.visible() {
                    peer.work.claims_status.state = "unavailable".into();
                    peer.work.claims_status.reason = Some(error.to_string());
                    peer.work.tasks_status = peer.work.claims_status.clone();
                }
            }
        }
    } else {
        report.peers = supervisors
            .into_iter()
            .map(|supervisor| PeerDetail {
                outcome: peer_outcome(&supervisor).into(),
                owner: supervisor.name.clone(),
                work: PeerWork::new(
                    &supervisor,
                    agents.iter().find(|a| a.id == supervisor.agent_id),
                    claims_visibility(
                        viewer_user_id,
                        agents.iter().find(|a| a.id == supervisor.agent_id),
                    ),
                    now,
                ),
                user_id: agents
                    .iter()
                    .find(|a| a.id == supervisor.agent_id)
                    .and_then(|a| a.user_id.clone()),
                team_id: agents
                    .iter()
                    .find(|a| a.id == supervisor.agent_id)
                    .and_then(|a| a.team_id.clone()),
                project_id: agents
                    .iter()
                    .find(|a| a.id == supervisor.agent_id)
                    .and_then(|a| a.project_id.clone()),
                claims_visibility: claims_visibility(
                    viewer_user_id,
                    agents.iter().find(|a| a.id == supervisor.agent_id),
                ),
                supervisor,
                branch: None,
                claims: Vec::new(),
                workers: Vec::new(),
            })
            .collect();
    }
    Ok(report)
}

/// cas-a131: this project's active worktrees and each listed peer's live
/// workers, as in-flight branches into their targets, compared file by file.
/// A peer's target is the branch its supervisor registered.
fn in_flight_overlaps(
    cas_root: &Path,
    supervisors: &[Peer],
    agents: &[AgentInfo],
    aliases: &[String],
    now: DateTime<Utc>,
) -> Vec<super::peer_merge::BranchOverlap> {
    use super::peer_merge::{InFlight, OVERLAP_BUDGET, find_overlaps};
    let Some(repo) = cas_root.parent() else {
        return Vec::new();
    };
    let own: Vec<InFlight> = crate::store::open_worktree_store(cas_root)
        .ok()
        .and_then(|store| store.list_active().ok())
        .unwrap_or_default()
        .into_iter()
        .map(|worktree| InFlight { branch: worktree.branch, target: worktree.parent_branch })
        .collect();
    if own.is_empty() {
        return Vec::new();
    }
    let mut theirs = Vec::new();
    for supervisor in supervisors {
        let Some(target) = supervisor.branch.clone() else {
            continue;
        };
        let owner = agents.iter().find(|a| a.id == supervisor.agent_id);
        let label = format!(
            "{}@{} ({})",
            supervisor.name,
            supervisor.machine,
            peers::owner_label(supervisor)
        );
        for agent in agents.iter().filter(|agent| {
            belongs_to_peer(agent, supervisor, owner, aliases)
                && heartbeat_age(agent, now).is_some_and(|age| age <= peers::PEER_LIVE_SECS)
        }) {
            if let Some(branch) = meta(agent, peers::META_BRANCH) {
                theirs.push((label.clone(), InFlight { branch: branch.to_string(), target: target.clone() }));
            }
        }
    }
    if theirs.is_empty() {
        return Vec::new();
    }
    find_overlaps(repo, &own, &theirs, std::time::Instant::now() + OVERLAP_BUDGET)
}

pub fn ascii_terminal() -> bool {
    !crate::ui::components::locale_is_utf8()
}

/// Plain text, with no terminal control sequences from untrusted metadata.
/// The same report supplies the JSON form, which preserves the full values.
pub fn render_peer_report(report: &PeerReport, width: usize, ascii: bool) -> String {
    let width = width.clamp(40, 72);
    let clean = |value: &str| -> String {
        value
            .chars()
            .filter(|c| !c.is_control())
            .map(|c| if ascii && !c.is_ascii() { '?' } else { c })
            .collect()
    };
    let mut out = String::new();
    let mut truncated = false;
    let mut line = |text: String| {
        let text = clean(&text);
        if ratatui::text::Line::raw(&text).width() <= width {
            out.push_str(&text);
        } else {
            truncated = true;
            let suffix = if ascii { "..." } else { "…" };
            let room = width - suffix.len();
            let mut prefix = String::new();
            for ch in text.chars() {
                prefix.push(ch);
                if ratatui::text::Line::raw(&prefix).width() > room {
                    prefix.pop();
                    break;
                }
            }
            out.push_str(&prefix);
            out.push_str(suffix);
        }
        out.push('\n');
    };
    if let Some(error) = &report.error {
        line(format!("[WARN] Peers unavailable: {}", report.status));
        line(format!("  {error}"));
    } else {
        let live = report.peers.iter().filter(|p| p.supervisor.live).count();
        let count = report.peers.len();
        line(format!(
            "[{}] {count} peer supervisor{} | {live} live",
            if report.status == "partial" {
                "WARN"
            } else {
                "OK"
            },
            if count == 1 { "" } else { "s" }
        ));
        line(format!(
            "Repo {}",
            report.canonical_id.as_deref().unwrap_or("-")
        ));
        if let Some(activation) = &report.activation {
            line(super::activation::line(activation));
        }
        if report.peers.is_empty() {
            line("  no other supervisors seen in the last 24h".into());
        }
        for overlap_line in super::peer_merge::render_overlaps(&report.overlaps) {
            line(overlap_line);
        }
        if report.hidden_peers > 0 {
            line(format!(
                "  {} supervisor{} of other accounts hidden (peers.same_repo_scope own)",
                report.hidden_peers,
                if report.hidden_peers == 1 { "" } else { "s" }
            ));
        }
        for detail in &report.peers {
            let peer = &detail.supervisor;
            line(format!("[{}] Owner {}", detail.outcome, detail.owner));
            line(format!("  Account {}", peers::owner_label(peer)));
            line(format!("  Machine {}", peer.machine));
            line(format!(
                "  Session {}",
                peer.session.as_deref().unwrap_or("-")
            ));
            line(format!("  Epic    {}", peer.epic_label()));
            line(format!("  Heartbeat {}s ago", peer.heartbeat_age_secs));
            // GH #1319: it heartbeats, but nothing it receives is read.
            // Wrapped, never cut: the reason is the point of the line.
            if let Some(failure) = peer.harness_auth_failed.as_deref() {
                let text =
                    format!("Harness logged out: \"{}\"; messages to it are refused", clean(failure));
                for piece in wrap_words(&text, width, "  ", "    ") {
                    line(piece);
                }
            }
            if !report.detail {
                continue;
            }
            work::render_work(&detail.work, &mut line);
            line(format!(
                "  Branch  {}",
                detail.branch.as_deref().unwrap_or("unknown")
            ));
            if detail.claims_visibility == ClaimsVisibility::TeamProject {
                line("  Claims visible (team project)".into());
                line(format!(
                    "  Account {}",
                    detail.user_id.as_deref().unwrap_or("unknown")
                ));
            }
            match detail.claims_visibility {
                ClaimsVisibility::SameAccount | ClaimsVisibility::TeamProject
                    if detail.work.claims_status.state == "ready" =>
                {
                    line(format!(
                        "  Claims  {} observed | age since renewal (s)",
                        detail.claims.len()
                    ))
                }
                ClaimsVisibility::SameAccount | ClaimsVisibility::TeamProject => line(format!(
                    "  Claims {} (inventory unknown)",
                    detail.work.claims_status.state
                )),
                ClaimsVisibility::DifferentAccount => {
                    line("  Claims not visible (different account): private work".into())
                }
                ClaimsVisibility::Unknown => line("  Claims not visible (account unknown)".into()),
            }
            for claim in &detail.claims {
                line(format!(
                    "    [{}] Owner {}",
                    claim
                        .status
                        .as_deref()
                        .unwrap_or("claimed; title not synced"),
                    claim.agent_name.as_deref().unwrap_or("unknown agent")
                ));
                line(format!(
                    "      {} | renewal age {}s",
                    claim.task_id, claim.age_secs
                ));
                if let Some(title) = &claim.title {
                    line(format!("          {title}"));
                }
            }
            line(format!(
                "  Workers {} live (read-only)",
                detail.workers.len()
            ));
            for worker in &detail.workers {
                line(format!("    {} [{}]", worker.name, worker.status));
                line(format!("      Machine {}", worker.machine));
                line(format!(
                    "      Observed {}",
                    worker.observed_at.to_rfc3339()
                ));
                line(format!(
                    "      Worktree {}",
                    worker.worktree_root.as_deref().unwrap_or("unknown")
                ));
                line(format!(
                    "      Scratch {}",
                    worker.scratch_root.as_deref().unwrap_or("unknown")
                ));
                line(format!(
                    "      Branch {}",
                    worker.branch.as_deref().unwrap_or("unknown")
                ));
                let task_ids = worker
                    .claims
                    .iter()
                    .map(|c| c.task_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let task_label = match detail.claims_visibility {
                    ClaimsVisibility::SameAccount | ClaimsVisibility::TeamProject
                        if detail.work.claims_status.state != "ready" =>
                    {
                        "unknown"
                    }
                    ClaimsVisibility::SameAccount | ClaimsVisibility::TeamProject
                        if task_ids.is_empty() =>
                    {
                        "none"
                    }
                    ClaimsVisibility::SameAccount | ClaimsVisibility::TeamProject => &task_ids,
                    _ => "not visible",
                };
                line(format!("      Tasks {task_label}"));
            }
        }
    }
    if report.detail {
        for limitation in &report.limitations {
            line(limitation.clone());
        }
        line(format!("Observed {}", report.observed_at.to_rfc3339()));
    }
    for presence_line in report.cloud_presence_message.lines() {
        line(presence_line.to_string());
    }
    if report.detail {
        line(format!(
            "Local splits {} at {}",
            report.epic_splits_status.state,
            report
                .epic_splits_status
                .observed_at
                .map(|t| t.to_rfc3339())
                .unwrap_or_else(|| "unknown".into())
        ));
        for split in &report.epic_splits {
            line(format!(
                "  {} [{}] epic {}",
                split.id, split.display_state, split.epic_id
            ));
            line(format!(
                "    Peer {}@{} | {}",
                split.peer_name, split.peer_machine, split.direction
            ));
            line(format!("    Stored state {}", split.state));
            line(format!("    Mine {}", split.mine.join(", ")));
            line(format!("    Theirs {}", split.theirs.join(", ")));
            line(format!(
                "    Reason {}",
                split.reason.as_deref().unwrap_or("unknown")
            ));
            line(format!("    Expires {}", split.expires_at.to_rfc3339()));
            line(format!(
                "    Decided {}",
                split
                    .decided_at
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_else(|| "unknown".into())
            ));
        }
    }
    #[cfg(feature = "mcp-proxy")]
    if report.detail {
        for contact_line in crate::coordination::stakeholder_contacts::render_status(&report.stakeholder_contacts) {
            line(contact_line);
        }
    }
    for mailbox_line in report.mailbox_status.lines() {
        line(mailbox_line.to_string());
    }
    drop(line);
    if truncated {
        out.push_str("--json for untruncated values (MCP: json=true)\n");
    }
    out
}

/// Greedy word wrap to `width` display columns: `first` prefixes the first
/// line and `rest` each continuation. A single word wider than a line is
/// left whole, for the caller's own bound to cut.
fn wrap_words(text: &str, width: usize, first: &str, rest: &str) -> Vec<String> {
    let columns = |value: &str| ratatui::text::Line::raw(value).width();
    let mut lines = Vec::new();
    let mut current = first.to_string();
    let mut empty = true;
    for word in text.split_whitespace() {
        if !empty && columns(&current) + 1 + columns(word) > width {
            lines.push(std::mem::replace(&mut current, rest.to_string()));
            empty = true;
        }
        if !empty {
            current.push(' ');
        }
        current.push_str(word);
        empty = false;
    }
    if !empty {
        lines.push(current);
    }
    lines
}

/// A peer's headline state. A supervisor whose harness is logged out
/// heartbeats like a live one but reads nothing (GH #1319).
fn peer_outcome(peer: &peers::Peer) -> &'static str {
    match (peer.harness_auth_failed.is_some(), peer.live) {
        (true, _) => "logged out",
        (false, true) => "live",
        (false, false) => "stale",
    }
}

#[cfg(test)]
#[path = "peers_detail_tests.rs"]
mod tests;
