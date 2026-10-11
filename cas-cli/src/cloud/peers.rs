//! Peer supervisor discovery (cas-e477).
//!
//! Supervisors working the same GitHub repo register with Cassy Cloud under
//! the repo's canonical id, on one machine or many. This module reads that
//! registry back: the live supervisors of this repo other than the caller,
//! with machine, factory session, epic focus and heartbeat age.
//!
//! The cloud scopes agents by user only (`GET /api/agents`), so the repo
//! filter is applied here, on the identity each supervisor registered in its
//! metadata. Its `status` column is only swept daily, so liveness comes from
//! `last_heartbeat`, not from `status`.
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

/// A peer is live while its last heartbeat is at most this old. Matches the
/// local supervisor liveness window in `worker_status`.
pub const PEER_LIVE_SECS: i64 = 300;
/// Peers whose last heartbeat is older than this are not listed at all.
pub const PEER_LISTING_WINDOW_SECS: i64 = 24 * 60 * 60;

/// One supervisor of the same repo, as registered in Cassy Cloud.
#[derive(Debug, Clone, PartialEq, Eq)]
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
                live: heartbeat_age_secs <= PEER_LIVE_SECS,
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
    let mut out = format!("Peers ({canonical_id}, via Cassy Cloud):\n");
    if peers.is_empty() {
        out.push_str("  no other supervisors of this repo seen in the last 24h\n");
        return out;
    }
    for peer in peers {
        let state = if peer.live { "live" } else { "stale" };
        out.push_str(&format!(
            "  {} [{state}] machine {} | session {} | focus {} | heartbeat {} ago\n",
            peer.name,
            peer.machine,
            peer.session.as_deref().unwrap_or("-"),
            peer.focus.as_deref().unwrap_or("-"),
            age(peer.heartbeat_age_secs),
        ));
    }
    out
}

/// Result of a peer lookup for one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerDiscovery {
    /// Peers of `canonical_id` (possibly none).
    Found { canonical_id: String, peers: Vec<Peer> },
    /// Not logged in to Cassy Cloud: cross-machine peers are unknowable.
    NotLoggedIn,
    /// The project has no canonical id to scope peers by.
    NoProjectIdentity,
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
    let peers = crate::cloud::CloudCoordinator::new(config)?
        .with_timeout(std::time::Duration::from_secs(10))
        .list_repo_peers(&canonical_id, &alias_class, self_id, Utc::now())?;
    Ok(PeerDiscovery::Found {
        canonical_id,
        peers,
    })
}

#[cfg(test)]
#[path = "peers_tests.rs"]
mod tests;
