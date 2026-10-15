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
}

#[derive(Debug, Clone, Serialize)]
pub struct PeerDetail {
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimsVisibility {
    SameAccount,
    DifferentAccount,
    Unknown,
}

fn claims_visibility(viewer: Option<&str>, owner: Option<&AgentInfo>) -> ClaimsVisibility {
    match (viewer, owner.and_then(|a| a.user_id.as_deref())) {
        (Some(viewer), Some(owner)) if viewer == owner => ClaimsVisibility::SameAccount,
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
    pub cloud_presence: Option<super::presence::PresenceRecord>,
    pub cloud_presence_message: String,
    pub mailbox_status: String,
    pub epic_splits: Vec<PeerEpicSplit>,
    #[serde(skip)]
    pub(crate) epic_splits_message: String,
    pub canonical_id: Option<String>,
    pub detail: bool,
    pub status: String,
    pub peers: Vec<PeerDetail>,
    pub error: Option<String>,
}

impl PeerReport {
    pub(crate) fn new(cas_root: &Path, self_id: Option<&str>, detail: bool) -> Self {
        let mut mailbox_status = String::new();
        peers::append_mailbox_health(cas_root, self_id, &mut mailbox_status);
        let now = Utc::now();
        let epic_splits = cas_store::list_epic_splits(cas_root)
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
            cloud_presence: self_id.and_then(|id| super::presence::load(cas_root, id)),
            cloud_presence_message: peers::presence_line(cas_root, self_id),
            mailbox_status,
            epic_splits,
            epic_splits_message: super::epic_split::render_splits(cas_root),
            canonical_id: super::resolve_canonical_id(cas_root),
            detail,
            status: "ready".into(),
            peers: Vec::new(),
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
                .filter(|_| visibility == ClaimsVisibility::SameAccount)
                .filter(|lock| lock.status == LeaseStatus::Active && lock.expires_at > now)
                .filter(|lock| agents.iter().find(|agent| agent.id == lock.agent_id)
                    .is_some_and(|agent| matches!(agent.status.as_str(), "active" | "idle")
                        && heartbeat_age(agent, now).is_some_and(|age| age <= peers::PEER_LIVE_SECS)))
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
            PeerDetail {
                supervisor,
                user_id: owner.and_then(|a| a.user_id.clone()),
                team_id: owner.and_then(|a| a.team_id.clone()),
                project_id: owner.and_then(|a| a.project_id.clone()),
                claims_visibility: visibility,
                branch,
                claims,
                workers,
            }
        })
        .collect()
}

/// Fetch full details only when requested. Discovery is repo scoped; locks
/// remain account-private and are filtered against the discovered owners.
/// Missing lock evidence is an error rather than an invented empty claim list.
pub fn discover_peer_report(
    cas_root: &Path,
    self_id: Option<&str>,
    detail: bool,
    selected: Option<&str>,
) -> Result<PeerReport, CasError> {
    let detail = detail || selected.is_some();
    let mut report = PeerReport::new(cas_root, self_id, detail);
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
    let agents = coordinator.list_agent_infos()?;
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
        // Team discovery can expose another account's agents, but locks are
        // account-private. Do not query them when no selected peer is visible.
        let locks = if supervisors.iter().any(|peer| {
            claims_visibility(
                viewer_user_id,
                agents.iter().find(|a| a.id == peer.agent_id),
            ) == ClaimsVisibility::SameAccount
        }) {
            coordinator.list_locks()?
        } else {
            Vec::new()
        };
        // Awareness must not run the cloud outbox's repair-on-open writer.
        let tasks = crate::store::open_task_store_local(cas_root)?;
        report.peers = assemble_peer_details(
            supervisors,
            &agents,
            &locks,
            &aliases,
            tasks.as_ref(),
            viewer_user_id,
            now,
        );
    } else {
        report.peers = supervisors
            .into_iter()
            .map(|supervisor| PeerDetail {
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

pub fn ascii_terminal() -> bool {
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|s| !s.is_empty()))
        .unwrap_or_default()
        .to_ascii_lowercase();
    !(locale.contains("utf-8") || locale.contains("utf8"))
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
    for presence_line in report.cloud_presence_message.lines() {
        line(presence_line.to_string());
    }
    if let Some(error) = &report.error {
        line(format!("[WARN] Peers unavailable: {}", report.status));
        line(format!("  {error}"));
    } else {
        let live = report.peers.iter().filter(|p| p.supervisor.live).count();
        let count = report.peers.len();
        line(format!(
            "[OK] {count} peer supervisor{} | {live} live",
            if count == 1 { "" } else { "s" }
        ));
        line(format!(
            "Repo {}",
            report.canonical_id.as_deref().unwrap_or("-")
        ));
        if report.peers.is_empty() {
            line("  no other supervisors seen in the last 24h".into());
        }
        for detail in &report.peers {
            let peer = &detail.supervisor;
            line(format!(
                "{} [{}]",
                peer.name,
                if peer.live { "live" } else { "stale" }
            ));
            line(format!("  Machine {}", peer.machine));
            line(format!(
                "  Session {}",
                peer.session.as_deref().unwrap_or("-")
            ));
            line(format!(
                "  Focus   {}",
                peer.focus.as_deref().unwrap_or("-")
            ));
            line(format!("  Heartbeat {}s ago", peer.heartbeat_age_secs));
            if !report.detail {
                continue;
            }
            line(format!(
                "  Branch  {}",
                detail.branch.as_deref().unwrap_or("unknown")
            ));
            match detail.claims_visibility {
                ClaimsVisibility::SameAccount => line(format!(
                    "  Claims  {} | age since renewal (s)",
                    detail.claims.len()
                )),
                ClaimsVisibility::DifferentAccount => {
                    line("  Claims not visible (different account)".into())
                }
                ClaimsVisibility::Unknown => line("  Claims not visible (account unknown)".into()),
            }
            for claim in &detail.claims {
                line(format!(
                    "    {:>5} {} [{}]",
                    claim.age_secs,
                    claim.task_id,
                    claim
                        .status
                        .as_deref()
                        .unwrap_or("claimed; title not synced")
                ));
                line(format!(
                    "          Owner {}",
                    claim.agent_name.as_deref().unwrap_or("unknown agent")
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
                    ClaimsVisibility::SameAccount if task_ids.is_empty() => "none",
                    ClaimsVisibility::SameAccount => &task_ids,
                    _ => "not visible",
                };
                line(format!("      Tasks {task_label}"));
            }
        }
    }
    for split_line in report.epic_splits_message.lines() {
        line(split_line.to_string());
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

#[cfg(test)]
#[path = "peers_detail_tests.rs"]
mod tests;
