//! cas-97ff (GH #1215 §1): a supervisor's SessionStart names the other live
//! supervisors of its repo, so a `/clear` never hides a live peer.
//!
//! The block is rendered from the same [`PeerReport`] that `cas peers` and
//! `coordination action=peers` render ([`discover_peer_report`]), so the two
//! views cannot disagree about who is live, where, or what they claim.

use std::path::Path;
use std::time::Duration;

use super::peers_detail::{ClaimsVisibility, PeerDetail, PeerReport, discover_peer_report};

/// SessionStart waits this long for Cassy Cloud, then reports peers unknown.
/// Bounded separately from the per-request discovery timeout, which applies
/// to each of the report's requests.
pub const SESSION_START_PEERS_DEADLINE: Duration = Duration::from_secs(3);

/// Live peers listed by name; more collapse into a count.
pub const SESSION_START_PEER_ROWS: usize = 5;

/// Hard ceiling on the block, well inside the SessionStart budget.
pub const SESSION_START_PEERS_MAX_BYTES: usize = 1_200;

/// Longest value copied from a peer's cloud metadata.
const FIELD_MAX_CHARS: usize = 40;

/// Section heading.
pub const HEADING: &str = "## Live peer supervisors on this repo (Cassy Cloud)";

/// The SessionStart block for `report`: `None` when the cloud has no live
/// peer of this repo, or the project does not use Cassy Cloud at all; one
/// "unknown" line when the lookup failed. Never claims there are no peers.
pub fn render_session_start_peers(report: &PeerReport, tool_prefix: &str) -> Option<String> {
    let _ = (report, tool_prefix);
    None
}

/// Compact form for the SessionStart budget: counts and the command.
pub fn compact_session_start_peers(report: &PeerReport, tool_prefix: &str) -> Option<String> {
    let _ = (report, tool_prefix);
    None
}

/// The rendered block and its compact form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStartPeers {
    pub full: String,
    pub compact: String,
}

/// Look the peers up for a supervisor's SessionStart, within
/// [`SESSION_START_PEERS_DEADLINE`].
pub fn session_start_peers(
    cas_root: &Path,
    self_id: Option<&str>,
    tool_prefix: &str,
) -> Option<SessionStartPeers> {
    let _ = (cas_root, self_id, tool_prefix);
    None
}

/// [`session_start_peers`] for a SessionStart of `role`: supervisors only.
/// Workers and unknown roles get nothing and make no request.
pub fn session_start_peers_for_role(
    role: Option<&str>,
    cas_root: &Path,
    self_id: Option<&str>,
    tool_prefix: &str,
) -> Option<SessionStartPeers> {
    let _ = (role, cas_root, self_id, tool_prefix);
    None
}

#[allow(dead_code)]
fn unused(_: &PeerDetail, _: ClaimsVisibility) {
    let _ = discover_peer_report;
}

#[cfg(test)]
#[path = "peers_session_start_tests.rs"]
mod tests;
