//! Read-only same-file overlap warnings for in-flight project branches.
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

/// At most this many branch pairs are compared per board.
pub const MAX_OVERLAP_BRANCHES: usize = 24;
/// A branch that changes more files than this is not compared.
pub const MAX_CHANGED_FILES: usize = 2_000;
/// Overall time a board may spend on overlap diffs.
pub const OVERLAP_BUDGET: Duration = Duration::from_secs(3);

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

#[cfg(test)]
#[path = "peer_merge_tests.rs"]
mod tests;
