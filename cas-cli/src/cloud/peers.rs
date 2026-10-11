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
    let _ = (agent, canonical_id, focus, hostname);
    HashMap::new()
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
    let _ = (agents, canonical_id, alias_class, self_id, now);
    Vec::new()
}

/// Human-readable lines for `worker_status`, `cas status` and
/// `coordination action=peers`.
pub fn render_peers(canonical_id: &str, peers: &[Peer]) -> String {
    let _ = (canonical_id, peers);
    String::new()
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
    let _ = (cas_root, self_id);
    Ok(PeerDiscovery::NoProjectIdentity)
}

#[cfg(test)]
#[path = "peers_tests.rs"]
mod tests;
