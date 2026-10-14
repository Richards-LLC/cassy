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
/// Metadata key carrying the factory session name.
pub const META_FACTORY_SESSION: &str = "factory_session";
/// Metadata key carrying the machine's hostname, for display.
pub const META_HOSTNAME: &str = "hostname";
/// Metadata key carrying the branch checked out where the agent works
/// (cas-f707). Optional; empty when unknown or detached.
pub const META_BRANCH: &str = "branch";

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
    pub last_heartbeat: DateTime<Utc>,
    pub heartbeat_age_secs: i64,
    pub live: bool,
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
    let meta = |agent: &AgentInfo, key: &str| {
        agent
            .metadata
            .get(key)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let mut peers: Vec<Peer> = agents
        .iter()
        .filter(|agent| Some(agent.id.as_str()) != self_id)
        .filter(|agent| agent.status != "shutdown")
        .filter(|agent| meta(agent, META_ROLE).as_deref() == Some("supervisor"))
        .filter(|agent| {
            meta(agent, META_CANONICAL_ID).is_some_and(|repo| {
                crate::cloud::project_ids_match_with_aliases(&repo, canonical_id, alias_class)
            })
        })
        .filter_map(|agent| {
            let last_heartbeat = DateTime::parse_from_rfc3339(&agent.last_heartbeat)
                .ok()?
                .with_timezone(&Utc);
            let heartbeat_age_secs = (now - last_heartbeat).num_seconds().max(0);
            (heartbeat_age_secs <= PEER_LISTING_WINDOW_SECS).then(|| Peer {
                agent_id: agent.id.clone(),
                name: agent.name.clone(),
                machine: meta(agent, META_HOSTNAME)
                    .or_else(|| agent.machine_id.clone())
                    .unwrap_or_else(|| "unknown".to_string()),
                machine_id: agent.machine_id.clone(),
                session: meta(agent, META_FACTORY_SESSION),
                canonical_id: meta(agent, META_CANONICAL_ID),
                focus: meta(agent, META_FOCUS),
                last_heartbeat,
                heartbeat_age_secs,
                // The cloud computes `stale` live (cloud PR #153).
                live: heartbeat_age_secs <= PEER_LIVE_SECS && agent.status != "stale",
            })
        })
        .collect();
    peers.sort_by(|a, b| {
        b.live
            .cmp(&a.live)
            .then(a.heartbeat_age_secs.cmp(&b.heartbeat_age_secs))
            .then(a.name.cmp(&b.name))
    });
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
        let state = if peer.live { "live" } else { "stale" };
        out.push_str(&format!(
            "  {} [{state}] on {}, heartbeat {} ago\n    session {} | focus {}\n",
            peer.name,
            peer.machine,
            age(peer.heartbeat_age_secs),
            peer.session.as_deref().unwrap_or("-"),
            peer.focus.as_deref().unwrap_or("-"),
        ));
    }
    out
}

/// Per-request cap for a peer lookup; status views must not hang on the cloud.
pub const DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Result of a peer lookup for one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerDiscovery {
    /// Peers of `canonical_id` (possibly none).
    Found {
        canonical_id: String,
        peers: Vec<Peer>,
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
    peers.retain(|peer| !local_ids.contains(&peer.agent_id));
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
    let mut peers = crate::cloud::CloudCoordinator::new(config)?
        .with_timeout(DISCOVERY_TIMEOUT)
        // cas-f707: ask the cloud for this project (petra-stella-cloud#151).
        .with_canonical_id(Some(canonical_id.clone()))
        .list_repo_peers(&canonical_id, &alias_class, self_id, Utc::now())?;
    exclude_project_agents(cas_root, &mut peers)?;
    Ok(PeerDiscovery::Found {
        canonical_id,
        peers,
    })
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
    let Some(self_id) = self_id else {
        return String::new();
    };
    let record = crate::cloud::presence::load(cas_root, self_id);
    crate::cloud::presence::render_line(record.as_ref(), Utc::now())
}

fn peers_discovery_section(cas_root: &Path, self_id: Option<&str>) -> String {
    match discover_peers(cas_root, self_id) {
        Ok(PeerDiscovery::Found {
            canonical_id,
            peers,
        }) => render_peers(&canonical_id, &peers),
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
