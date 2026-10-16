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
    peers
        .iter()
        .filter(|peer| {
            peer.live
                && peer.session.as_deref().is_some_and(|session| !session.trim().is_empty())
                && peer.branch.as_deref().map(str::trim) == Some(target.trim())
        })
        .collect()
}

/// Summary and body of the note.
pub fn merge_intent_message(intent: &MergeIntent<'_>) -> (String, String) {
    let tip: String = intent.tip.chars().take(10).collect();
    let task = intent
        .task_id
        .map(|task| format!(" ({task})"))
        .unwrap_or_default();
    let summary = format!("About to merge into {}", intent.target);
    let body = format!(
        "About to merge {}{task} @ {tip} into {}, by {}. This is a heads-up so you can \
         bring {} in after it lands; nothing waits for a reply.",
        intent.source, intent.target, intent.merger, intent.target
    );
    (summary, body)
}

/// Send the note to every recipient among `peers`, once per
/// target/source/tip, recording it in the store's ledger.
pub fn deliver_merge_intent(
    cas_root: &Path,
    intent: &MergeIntent<'_>,
    peers: &[Peer],
    mut send: impl FnMut(&Peer, &str, &str) -> Result<(), String>,
) -> MergeIntentOutcome {
    let recipients = merge_intent_recipients(peers, intent.target);
    if recipients.is_empty() {
        return MergeIntentOutcome::NoPeers;
    }
    let key = format!("{}\u{0}{}\u{0}{}", intent.target, intent.source, intent.tip);
    let path = cas_root.join(MERGE_INTENT_LEDGER);
    let now = Utc::now();
    let mut ledger: Ledger = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    ledger
        .sent
        .retain(|_, at| now - *at <= chrono::Duration::days(LEDGER_RETENTION_DAYS));
    if ledger.sent.contains_key(&key) {
        return MergeIntentOutcome::AlreadySent;
    }
    // Recorded before sending: a retry never repeats an attempted note.
    ledger.sent.insert(key, now);
    if let Ok(text) = serde_json::to_string_pretty(&ledger) {
        let _ = std::fs::write(&path, text);
    }
    let (summary, body) = merge_intent_message(intent);
    let mut sent = Vec::new();
    let mut errors = Vec::new();
    for peer in recipients {
        let label = format!("{}@{}", peer.name, peer.machine);
        match send(peer, &summary, &body) {
            Ok(()) => sent.push(label),
            Err(error) => errors.push(format!("{label}: {error}")),
        }
    }
    if sent.is_empty() {
        MergeIntentOutcome::Failed(errors)
    } else {
        MergeIntentOutcome::Sent(sent)
    }
}

/// Discover this project's peers (honouring `peers.same_repo_scope`) and send
/// the note through the peer mailbox. Best effort: never an error.
pub fn announce_merge(
    cas_root: &Path,
    sender_agent_id: &str,
    intent: &MergeIntent<'_>,
) -> MergeIntentOutcome {
    let peers = match crate::cloud::peers::discover_peers(cas_root, Some(sender_agent_id)) {
        Ok(crate::cloud::peers::PeerDiscovery::Found { peers, .. }) => peers,
        Ok(other) => return MergeIntentOutcome::Unavailable(format!("{other:?}")),
        Err(error) => return MergeIntentOutcome::Unavailable(error.to_string()),
    };
    deliver_merge_intent(cas_root, intent, &peers, |peer, summary, body| {
        crate::cloud::peer_mailbox::send_to_peer(
            cas_root,
            sender_agent_id,
            peer,
            body,
            Some(summary),
            None,
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
    })
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
    let resolve = |name: &str| {
        [name.to_string(), format!("origin/{name}")]
            .into_iter()
            .find(|candidate| git_ok(repo, &["rev-parse", "--verify", "--quiet", &format!("{candidate}^{{commit}}")]))
    };
    let target_ref = resolve(target)?;
    let branch_ref = resolve(branch)?;
    // A merged branch is no longer in flight.
    if git_ok(repo, &["merge-base", "--is-ancestor", &branch_ref, &target_ref]) {
        return None;
    }
    let output = std::process::Command::new("git")
        .args(["diff", "--name-only", "--no-renames", &format!("{target_ref}...{branch_ref}")])
        .current_dir(repo)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let mut files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if files.len() > MAX_CHANGED_FILES {
        return None;
    }
    files.sort();
    Some(files)
}

fn git_ok(repo: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Same-file overlaps between this project's in-flight branches and the
/// peers' (`(peer label, in-flight branch)`), within `deadline`.
pub fn find_overlaps(
    repo: &Path,
    own: &[InFlight],
    peers: &[(String, InFlight)],
    deadline: Instant,
) -> Vec<BranchOverlap> {
    let mut cache: BTreeMap<(String, String), Option<Vec<String>>> = BTreeMap::new();
    let mut files = |flight: &InFlight| -> Option<Vec<String>> {
        let key = (flight.target.clone(), flight.branch.clone());
        if let Some(found) = cache.get(&key) {
            return found.clone();
        }
        if cache.len() >= MAX_OVERLAP_BRANCHES || Instant::now() >= deadline {
            return None;
        }
        let found = changed_files(repo, &flight.target, &flight.branch);
        cache.insert(key, found.clone());
        found
    };
    let mut overlaps = Vec::new();
    for mine in own {
        for (peer, theirs) in peers {
            if theirs.target != mine.target || theirs.branch == mine.branch {
                continue;
            }
            let (Some(left), Some(right)) = (files(mine), files(theirs)) else {
                continue;
            };
            let shared: Vec<String> =
                left.iter().filter(|file| right.contains(file)).cloned().collect();
            if !shared.is_empty() {
                overlaps.push(BranchOverlap {
                    target: mine.target.clone(),
                    own_branch: mine.branch.clone(),
                    peer: peer.clone(),
                    peer_branch: theirs.branch.clone(),
                    files: shared,
                });
            }
        }
    }
    overlaps
}

/// Board lines for the overlaps, each within 80 columns.
pub fn render_overlaps(overlaps: &[BranchOverlap]) -> Vec<String> {
    let fit = |text: String| -> String {
        if text.chars().count() <= 80 {
            text
        } else {
            let mut cut: String = text.chars().take(77).collect();
            cut.push_str("...");
            cut
        }
    };
    let mut lines = Vec::new();
    for overlap in overlaps {
        lines.push(fit(format!("[WARN] Same files in flight into {}:", overlap.target)));
        lines.push(fit(format!("  yours  {}", overlap.own_branch)));
        lines.push(fit(format!("  theirs {} ({})", overlap.peer_branch, overlap.peer)));
        let shown: Vec<&str> = overlap.files.iter().take(4).map(String::as_str).collect();
        let more = overlap.files.len().saturating_sub(shown.len());
        lines.push(fit(format!(
            "  files  {}{}",
            shown.join(", "),
            if more > 0 { format!(" (+{more} more)") } else { String::new() }
        )));
    }
    if !overlaps.is_empty() {
        lines.push(fit(
            "  Agree the merge order by message; nothing is blocked.".to_string(),
        ));
    }
    lines
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Ledger {
    sent: BTreeMap<String, DateTime<Utc>>,
}

#[cfg(test)]
#[path = "peer_merge_tests.rs"]
mod tests;
