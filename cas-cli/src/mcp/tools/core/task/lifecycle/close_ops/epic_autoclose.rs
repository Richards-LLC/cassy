//! cas-7f60 (GH #1232): a finished epic closes without manual steps.
//!
//! The 2026-10-16 cleanup of 16 epics needed a manual owner transfer per
//! epic, hand-supplied receipts, and seven stranded-branch override
//! narratives for child branches whose content was already on the default
//! branch. This module holds the decisions that remove those steps:
//!
//! * [`dead_owner_adoption`]: a live registered supervisor adopts an epic
//!   whose `epic_verification_owner` is a registered agent that is dead.
//! * [`landed_on_default_branch`]: a child branch the epic-branch gate calls
//!   stranded is not stranded when the default branch already carries its
//!   content, measured by ancestry (`rev-list --count`) or patch-equivalence
//!   (`git cherry`).

use std::path::Path;

use chrono::{DateTime, Utc};

/// An owner whose newest heartbeat is older than this is dead, even when its
/// row still says active (a crashed session never writes `shutdown`). Three
/// times the 5-minute stale window used elsewhere, so a slow or briefly
/// suspended supervisor is never adopted out from under itself.
pub(crate) const EPIC_OWNER_DEAD_SECS: i64 = 15 * 60;

fn age(secs: i64) -> String {
    match secs {
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// Why `owner` may be adopted, or `None` when it must not be.
///
/// The owner is matched against every registered agent by id, name or
/// factory session. Adoption needs positive evidence of death: every
/// matching agent is shut down, stale, or silent for longer than
/// [`EPIC_OWNER_DEAD_SECS`]. An owner no registered agent matches is unknown
/// (it may be a supervisor on another machine) and is never adopted.
pub(crate) fn dead_owner_adoption(
    owner: &str,
    agents: &[cas_types::Agent],
    now: DateTime<Utc>,
) -> Option<String> {
    let owner = owner.trim();
    if owner.is_empty() {
        return None;
    }
    let matching: Vec<&cas_types::Agent> = agents
        .iter()
        .filter(|agent| {
            agent.id.trim() == owner
                || agent.name.trim() == owner
                || agent
                    .factory_session
                    .as_deref()
                    .is_some_and(|session| session.trim() == owner)
        })
        .collect();
    if matching.is_empty() {
        return None;
    }
    let mut evidence = Vec::new();
    for agent in matching {
        let silent = (now - agent.last_heartbeat).num_seconds().max(0);
        let dead = matches!(
            agent.status,
            cas_types::AgentStatus::Shutdown | cas_types::AgentStatus::Stale
        ) || silent > EPIC_OWNER_DEAD_SECS;
        if !dead {
            return None;
        }
        evidence.push(format!(
            "`{}` ({}) is {}, last heartbeat {} ago",
            agent.name,
            agent.id,
            agent.status,
            age(silent)
        ));
    }
    Some(evidence.join("; "))
}

/// The default branch ref to measure against: `origin/<default>` when it
/// exists (what shipped), else the local default branch.
pub(crate) fn default_branch_ref(repo: &Path) -> Option<String> {
    let branch = crate::mcp::tools::core::task::repo_context::resolve_default_branch(repo).ok()?;
    let remote = format!("origin/{branch}");
    if resolves(repo, &remote) {
        Some(remote)
    } else if resolves(repo, &branch) {
        Some(branch)
    } else {
        None
    }
}

fn resolves(repo: &Path, reference: &str) -> bool {
    git(repo, &["rev-parse", "--verify", "--quiet", &format!("{reference}^{{commit}}")]).is_ok()
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|error| format!("git {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The default branch name to measure an epic against when its branch
/// `epic_branch` resolves neither locally nor as `origin/<epic_branch>`, or
/// `None` while the branch exists (or no default branch resolves).
pub(crate) fn gone_epic_target(repo: &Path, epic_branch: &str) -> Option<String> {
    let epic_branch = epic_branch.trim();
    if epic_branch.is_empty()
        || resolves(repo, epic_branch)
        || resolves(repo, &format!("origin/{epic_branch}"))
    {
        return None;
    }
    let default =
        crate::mcp::tools::core::task::repo_context::resolve_default_branch(repo).ok()?;
    (default != epic_branch && default_branch_ref(repo).is_some()).then_some(default)
}

/// `children` with every work target naming the gone `epic_branch` pointed
/// at `default_branch`, so each child's delivery is proven there.
pub(crate) fn retarget_children(
    children: &[cas_types::Task],
    epic_branch: &str,
    default_branch: &str,
) -> Vec<cas_types::Task> {
    children
        .iter()
        .cloned()
        .map(|mut child| {
            if let Some(target) = child.deliverables.work_target.as_mut()
                && target.target_branch.trim() == epic_branch.trim()
            {
                target.target_branch = default_branch.to_string();
            }
            child
        })
        .collect()
}

/// How a child's commit-ish relates to the default branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DefaultBranchContainment {
    /// Every commit is an ancestor of the default branch.
    Ancestor,
    /// Commits remain by ancestry, but `git cherry` finds each patch applied.
    PatchEquivalent { commits: usize },
    /// At least one patch is not on the default branch.
    Unlanded { patches: usize },
}

/// Measure `commit_ish` against `default_ref`. `Err` when Git cannot tell;
/// callers treat that as "still stranded".
pub(crate) fn landed_on_default_branch(
    repo: &Path,
    default_ref: &str,
    commit_ish: &str,
) -> Result<DefaultBranchContainment, String> {
    let range = format!("{default_ref}..{commit_ish}");
    let ahead: usize = git(repo, &["rev-list", "--count", &range])?
        .parse()
        .map_err(|error| format!("rev-list --count {range}: {error}"))?;
    if ahead == 0 {
        return Ok(DefaultBranchContainment::Ancestor);
    }
    let cherry = git(repo, &["cherry", default_ref, commit_ish])?;
    let unlanded = cherry.lines().filter(|line| line.starts_with('+')).count();
    if unlanded == 0 {
        Ok(DefaultBranchContainment::PatchEquivalent { commits: ahead })
    } else {
        Ok(DefaultBranchContainment::Unlanded { patches: unlanded })
    }
}

/// Decision notes for the rows the epic-branch gate would block whose every
/// measured ref is already on the default branch, keyed by child task id.
///
/// Only plain ancestry rows qualify: a row with dropped paths, a content
/// check error or an unresolved ref keeps blocking. When the parent already
/// is the default branch there is nothing new to measure.
pub(super) fn landed_on_default_notes(
    rows: &[super::EpicChildBranchStatus],
    parent_branch: &str,
    repo: &Path,
) -> std::collections::BTreeMap<String, String> {
    let mut notes = std::collections::BTreeMap::new();
    let candidates: Vec<_> = rows
        .iter()
        .filter(|row| {
            row.unmerged_count > 0
                && row.dropped_paths.is_empty()
                && row.content_check_error.is_none()
                && !row.refs_unresolved
        })
        .collect();
    if candidates.is_empty() {
        return notes;
    }
    let Some(default_ref) = default_branch_ref(repo) else {
        return notes;
    };
    if default_ref == parent_branch || default_ref == format!("origin/{parent_branch}") {
        return notes;
    }
    for row in candidates {
        // Every ref the gate read, plus the recorded delivery anchor: a live
        // lane can be fully merged while the task's own anchor still holds a
        // work commit that never landed anywhere.
        let Some(mut refs) = row
            .checked_ref_reads
            .iter()
            .map(|read| read.read_ref())
            .collect::<Option<Vec<&str>>>()
            .filter(|refs| !refs.is_empty())
        else {
            continue;
        };
        if let Some(anchor) = row.recorded_anchor.as_deref()
            && !refs.contains(&anchor)
        {
            refs.push(anchor);
        }
        let verdicts: Option<Vec<String>> = refs
            .iter()
            .map(|reference| match landed_on_default_branch(repo, &default_ref, reference) {
                Ok(DefaultBranchContainment::Ancestor) => {
                    Some(format!("`{reference}` has 0 commits off `{default_ref}`"))
                }
                Ok(DefaultBranchContainment::PatchEquivalent { commits }) => Some(format!(
                    "`{reference}` has {commits} commit(s) off `{default_ref}` by ancestry and \
                     every patch applied there (git cherry)"
                )),
                Ok(DefaultBranchContainment::Unlanded { .. }) | Err(_) => None,
            })
            .collect();
        if let Some(verdicts) = verdicts {
            notes.insert(
                row.task_id.clone(),
                format!(
                    "decision: child task `{task}` carries {count} commit(s) not on \
                     `{parent_branch}`, but `{default_ref}` already carries its content: \
                     {verdicts}. Not stranded (cas-7f60).",
                    task = row.task_id,
                    count = row.unmerged_count,
                    verdicts = verdicts.join("; "),
                ),
            );
        }
    }
    notes
}

#[cfg(test)]
#[path = "epic_autoclose_tests.rs"]
mod tests;
