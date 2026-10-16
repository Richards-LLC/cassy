//! cas-a131: cooperative merge helpers for supervisors that deliver into the
//! same target branch. They inform and never block: a merge proceeds exactly
//! as it would without them.
//!
//! - An "about to merge" note: when a supervisor starts a Cassy merge into a
//!   target branch, every other live supervisor of the project registered on
//!   that branch gets one peer message, once per merged tip.
//! - A same-file overlap warning: in-flight branches of this project's
//!   workers and of a peer's workers that change the same files on the way
//!   into the same target. Read-only git, bounded in time and size.
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::cloud::peers::Peer;

/// File under the Cassy store recording which merge notes were sent.
pub const MERGE_INTENT_LEDGER: &str = "peer-merge-intents.json";
/// Ledger rows older than this are forgotten.
const LEDGER_RETENTION_DAYS: i64 = 7;
/// At most this many branch pairs are compared per board.
pub const MAX_OVERLAP_BRANCHES: usize = 24;
/// A branch that changes more files than this is not compared.
pub const MAX_CHANGED_FILES: usize = 2_000;
/// Overall time a board may spend on overlap diffs.
pub const OVERLAP_BUDGET: Duration = Duration::from_secs(3);

/// One merge about to happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeIntent<'a> {
    pub target: &'a str,
    pub source: &'a str,
    pub task_id: Option<&'a str>,
    /// The commit being merged; the note is sent once per tip.
    pub tip: &'a str,
    /// `name@machine` of the merging supervisor.
    pub merger: &'a str,
}

/// What [`deliver_merge_intent`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeIntentOutcome {
    /// Sent to these `name@machine` peers.
    Sent(Vec<String>),
    /// This tip was already announced (a retry).
    AlreadySent,
    /// No other live supervisor is registered on the target branch.
    NoPeers,
    /// Sends failed; the merge is unaffected.
    Failed(Vec<String>),
    /// Peer discovery was unavailable; the merge is unaffected.
    Unavailable(String),
}

/// The live supervisors that receive the note: a factory daemon is running
/// and their registered branch is `target`.
pub fn merge_intent_recipients<'a>(peers: &'a [Peer], target: &str) -> Vec<&'a Peer> {
    let _ = (peers, target);
    Vec::new()
}

/// Summary and body of the note.
pub fn merge_intent_message(intent: &MergeIntent<'_>) -> (String, String) {
    let _ = intent;
    (String::new(), String::new())
}

/// Send the note to every recipient among `peers`, once per
/// target/source/tip, recording it in the store's ledger.
pub fn deliver_merge_intent(
    cas_root: &Path,
    intent: &MergeIntent<'_>,
    peers: &[Peer],
    send: impl FnMut(&Peer, &str, &str) -> Result<(), String>,
) -> MergeIntentOutcome {
    let _ = (cas_root, intent, peers, send);
    MergeIntentOutcome::NoPeers
}

/// One pair of in-flight branches that change the same files on the way into
/// `target`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BranchOverlap {
    pub target: String,
    pub own_branch: String,
    /// The peer supervisor, as `name@machine (owner)`.
    pub peer: String,
    pub peer_branch: String,
    pub files: Vec<String>,
}

/// An in-flight branch into `target`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InFlight {
    pub branch: String,
    pub target: String,
}

/// Files `branch` changes since it left `target`, or `None` when it is
/// merged, unknown, or changes more than [`MAX_CHANGED_FILES`] files.
pub fn changed_files(repo: &Path, target: &str, branch: &str) -> Option<Vec<String>> {
    let _ = (repo, target, branch);
    None
}

/// Same-file overlaps between this project's in-flight branches and the
/// peers' (`(peer label, in-flight branch)`), within `deadline`.
pub fn find_overlaps(
    repo: &Path,
    own: &[InFlight],
    peers: &[(String, InFlight)],
    deadline: Instant,
) -> Vec<BranchOverlap> {
    let _ = (repo, own, peers, deadline);
    Vec::new()
}

/// Board lines for the overlaps, each within 80 columns.
pub fn render_overlaps(overlaps: &[BranchOverlap]) -> Vec<String> {
    let _ = overlaps;
    Vec::new()
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Ledger {
    sent: BTreeMap<String, DateTime<Utc>>,
}

#[cfg(test)]
#[path = "peer_merge_tests.rs"]
mod tests;
