//! Backlog hygiene (cas-e8d1, GH #1233): open work the record says is done.
//!
//! The 2026-10-16 cleanup found about 60 stale open tasks, about 25 of them
//! already delivered on the default branch, and 11 epics whose children had
//! all closed. Nothing flagged any of them. This module finds three kinds:
//!
//! 1. an open task whose delivery commit is on the default branch: a
//!    non-merge commit whose subject names the task;
//! 2. an open epic whose children have all closed or been cancelled;
//! 3. an open task idle for [`IDLE_DAYS`] or more.
//!
//! Each finding carries its evidence. Kinds 1 and 2 also carry the one call
//! that closes them through the ordinary close gates; nothing here closes,
//! reopens or edits a task. `cas doctor` and a supervisor's SessionStart
//! render the same report.

use std::path::Path;

use chrono::{DateTime, Utc};

use crate::types::{Dependency, DependencyType, Task, TaskType};

/// A task untouched this long is reported as idle.
pub(crate) const IDLE_DAYS: i64 = 14;

/// Commits read from the default branch, newest first. Bounds the scan on
/// very long histories; the `--since` floor normally ends it much sooner.
const MAX_COMMITS: usize = 20_000;

/// One non-merge commit on the default branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitRecord {
    pub sha: String,
    pub committed_at: DateTime<Utc>,
    pub subject: String,
}

/// Kind 1: an open task delivered on the default branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeliveredOpen {
    pub task_id: String,
    pub title: String,
    pub status: String,
    pub commit: CommitRecord,
    /// Further subject-matching commits beyond `commit`.
    pub more_commits: usize,
}

/// Kind 2: an open epic whose children have all ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FinishedEpic {
    pub epic_id: String,
    pub title: String,
    pub children: usize,
    pub last_closed_at: Option<DateTime<Utc>>,
}

/// Kind 3: an open task untouched for [`IDLE_DAYS`] or more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdleOpen {
    pub task_id: String,
    pub title: String,
    pub status: String,
    pub assignee: Option<String>,
    pub idle_days: i64,
}

/// What one scan found.
#[derive(Debug, Clone, Default)]
pub(crate) struct BacklogReport {
    /// The ref kind 1 was measured against, e.g. `origin/main`.
    pub default_ref: Option<String>,
    /// Why kind 1 could not be measured, when it could not.
    pub delivery_unknown: Option<String>,
    pub delivered: Vec<DeliveredOpen>,
    pub finished_epics: Vec<FinishedEpic>,
    pub idle: Vec<IdleOpen>,
}

impl BacklogReport {
    pub(crate) fn is_clean(&self) -> bool {
        self.delivered.is_empty() && self.finished_epics.is_empty() && self.idle.is_empty()
    }
}

/// Kind 1 over `commits` (the default branch's non-merge history, newest
/// first). Only a commit SUBJECT that names the task as a whole token counts:
/// workers commit `fix(cas-1234): …`, while a body that mentions a task as
/// context does not deliver it. Epics are kind 2's.
pub(crate) fn delivered_open(tasks: &[Task], commits: &[CommitRecord]) -> Vec<DeliveredOpen> {
    use crate::mcp::tools::core::task::lifecycle::close_ops::message_references_task;
    let mut found: Vec<DeliveredOpen> = tasks
        .iter()
        .filter(|task| task.is_open() && task.task_type != TaskType::Epic)
        .filter_map(|task| {
            let mut matching = commits
                .iter()
                .filter(|commit| message_references_task(&commit.subject, &task.id));
            let commit = matching.next()?.clone();
            Some(DeliveredOpen {
                task_id: task.id.clone(),
                title: task.title.clone(),
                status: task.status.to_string(),
                commit,
                more_commits: matching.count(),
            })
        })
        .collect();
    found.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    found
}

/// Kind 2 over every task and dependency. An epic counts only when it has
/// children and every one of them is closed or cancelled; a child the store
/// does not hold is not known to have ended, so its epic is left alone.
pub(crate) fn finished_open_epics(tasks: &[Task], deps: &[Dependency]) -> Vec<FinishedEpic> {
    let by_id: std::collections::HashMap<&str, &Task> =
        tasks.iter().map(|task| (task.id.as_str(), task)).collect();
    let mut found: Vec<FinishedEpic> = tasks
        .iter()
        .filter(|task| task.is_open() && task.task_type == TaskType::Epic)
        .filter_map(|epic| {
            let children: Vec<Option<&&Task>> = deps
                .iter()
                .filter(|dep| dep.dep_type == DependencyType::ParentChild && dep.to_id == epic.id)
                .map(|dep| by_id.get(dep.from_id.as_str()))
                .collect();
            if children.is_empty()
                || !children.iter().all(|child| child.is_some_and(|child| child.is_terminal()))
            {
                return None;
            }
            Some(FinishedEpic {
                epic_id: epic.id.clone(),
                title: epic.title.clone(),
                children: children.len(),
                last_closed_at: children
                    .iter()
                    .filter_map(|child| child.and_then(|child| child.closed_at))
                    .max(),
            })
        })
        .collect();
    found.sort_by(|a, b| a.epic_id.cmp(&b.epic_id));
    found
}

/// Kind 3 at `now`: open non-epic tasks untouched for [`IDLE_DAYS`] or more,
/// longest idle first. An epic waits on its children by design.
pub(crate) fn idle_open(tasks: &[Task], now: DateTime<Utc>) -> Vec<IdleOpen> {
    let mut found: Vec<IdleOpen> = tasks
        .iter()
        .filter(|task| task.is_open() && task.task_type != TaskType::Epic)
        .filter_map(|task| {
            let idle_days = (now - task.updated_at).num_days();
            (idle_days >= IDLE_DAYS).then(|| IdleOpen {
                task_id: task.id.clone(),
                title: task.title.clone(),
                status: task.status.to_string(),
                assignee: task.assignee.clone(),
                idle_days,
            })
        })
        .collect();
    found.sort_by(|a, b| b.idle_days.cmp(&a.idle_days).then_with(|| a.task_id.cmp(&b.task_id)));
    found
}

/// Scan the project whose store is `cas_root`. Kinds 2 and 3 need only the
/// task store; kind 1 also reads the default branch of the project's git
/// repository (the store's parent directory), and says why when it cannot.
pub(crate) fn scan(cas_root: &Path, now: DateTime<Utc>) -> Result<BacklogReport, String> {
    let store = crate::store::open_task_store_local(cas_root).map_err(|error| error.to_string())?;
    let tasks = store.list(None).map_err(|error| error.to_string())?;
    let deps = store
        .list_dependencies(Some(DependencyType::ParentChild))
        .map_err(|error| error.to_string())?;
    let mut report = BacklogReport {
        finished_epics: finished_open_epics(&tasks, &deps),
        idle: idle_open(&tasks, now),
        ..BacklogReport::default()
    };
    let oldest_open = tasks
        .iter()
        .filter(|task| task.is_open() && task.task_type != TaskType::Epic)
        .map(|task| task.created_at)
        .min();
    let Some(oldest_open) = oldest_open else {
        return Ok(report);
    };
    let repo = cas_root.parent().unwrap_or(cas_root);
    match default_ref(repo) {
        Ok(reference) => {
            match read_commits(repo, &reference, oldest_open - chrono::Duration::days(1)) {
                Ok(commits) => report.delivered = delivered_open(&tasks, &commits),
                Err(error) => report.delivery_unknown = Some(error),
            }
            report.default_ref = Some(reference);
        }
        Err(error) => report.delivery_unknown = Some(error),
    }
    Ok(report)
}

/// The default branch as git knows it: `origin/<branch>` when the remote ref
/// exists, since that is what shipped, else the local branch.
fn default_ref(repo: &Path) -> Result<String, String> {
    let branch = crate::mcp::tools::core::task::repo_context::resolve_default_branch(repo)?;
    let remote = format!("refs/remotes/origin/{branch}");
    let has_remote = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", &remote])
        .output()
        .is_ok_and(|output| output.status.success());
    Ok(if has_remote { format!("origin/{branch}") } else { branch })
}

/// Non-merge commits on `reference` since `since`, newest first.
fn read_commits(
    repo: &Path,
    reference: &str,
    since: DateTime<Utc>,
) -> Result<Vec<CommitRecord>, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "log",
            reference,
            "--no-merges",
            &format!("--max-count={MAX_COMMITS}"),
            &format!("--since={}", since.to_rfc3339()),
            "--format=%H%x1f%cI%x1f%s%x1e",
        ])
        .output()
        .map_err(|error| format!("cannot run git log on {reference}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git log {reference} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\u{1e}')
        .filter_map(|record| {
            let mut fields = record.trim().splitn(3, '\u{1f}');
            let sha = fields.next()?.to_string();
            let committed_at = DateTime::parse_from_rfc3339(fields.next()?)
                .ok()?
                .with_timezone(&Utc);
            let subject = fields.next()?.to_string();
            (!sha.is_empty()).then_some(CommitRecord { sha, committed_at, subject })
        })
        .collect())
}

fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut clipped: String = text.chars().take(max.saturating_sub(1)).collect();
    clipped.push('…');
    clipped
}

/// The call that closes a kind-1 finding through the ordinary close gates:
/// the commit receipt is validated against the target there, not here.
pub(crate) fn delivered_close_call(finding: &DeliveredOpen, default_ref: &str) -> String {
    format!(
        "task action=close id={} commit_receipt={} reason=\"delivered on {default_ref} in {}\"",
        finding.task_id,
        finding.commit.sha,
        short(&finding.commit.sha),
    )
}

/// The call that closes a kind-2 finding through the ordinary epic close gates.
pub(crate) fn epic_close_call(finding: &FinishedEpic) -> String {
    format!(
        "task action=close id={} reason=\"all {} children closed\"",
        finding.epic_id, finding.children
    )
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(9)]
}

/// `1 delivered but open, 2 finished epics open, 3 idle 14+ days`.
pub(crate) fn summary_line(report: &BacklogReport) -> String {
    format!(
        "{} delivered but open, {} finished epic{} open, {} idle {IDLE_DAYS}+ days",
        report.delivered.len(),
        report.finished_epics.len(),
        if report.finished_epics.len() == 1 { "" } else { "s" },
        report.idle.len(),
    )
}

/// One kind-1 line: the task, the commit that delivered it, and its close.
pub(crate) fn delivered_line(finding: &DeliveredOpen, default_ref: &str) -> String {
    format!(
        "{} ({}) \"{}\": on {default_ref} as {} {} ({}){}. Close: {}",
        finding.task_id,
        finding.status,
        clip(&finding.title, 50),
        short(&finding.commit.sha),
        clip(&finding.commit.subject, 60),
        finding.commit.committed_at.format("%Y-%m-%d"),
        match finding.more_commits {
            0 => String::new(),
            n => format!(", +{n} more commit{}", if n == 1 { "" } else { "s" }),
        },
        delivered_close_call(finding, default_ref),
    )
}

/// One kind-2 line: the epic, its ended children, and its close.
pub(crate) fn epic_line(finding: &FinishedEpic) -> String {
    format!(
        "{} \"{}\": all {} children closed{}. Close: {}",
        finding.epic_id,
        clip(&finding.title, 50),
        finding.children,
        finding
            .last_closed_at
            .map(|at| format!(", last on {}", at.format("%Y-%m-%d")))
            .unwrap_or_default(),
        epic_close_call(finding),
    )
}

/// One kind-3 line: the task and how long it has sat.
pub(crate) fn idle_line(finding: &IdleOpen) -> String {
    format!(
        "{} ({}{}) \"{}\": untouched {} days",
        finding.task_id,
        finding.status,
        finding
            .assignee
            .as_deref()
            .map(|assignee| format!(", {assignee}"))
            .unwrap_or_default(),
        clip(&finding.title, 50),
        finding.idle_days,
    )
}

/// The supervisor SessionStart block: `(full, compact)`, or `None` when the
/// report is clean. The full form shows a few entries of each kind with their
/// evidence and close call, bounded by [`SESSION_START_CAP_BYTES`]; the rest
/// are counted and `cas doctor` lists them all.
pub(crate) fn session_start_block(report: &BacklogReport) -> Option<(String, String)> {
    if report.is_clean() {
        return None;
    }
    let reference = report.default_ref.as_deref().unwrap_or("the default branch");
    let head = format!("Backlog hygiene: {}.", summary_line(report));
    let compact = format!("{head} Run `cas doctor` for the evidence and close calls.");
    let mut lines: Vec<String> = Vec::new();
    let mut omitted = 0usize;
    let mut take = |entries: Vec<String>, limit: usize, lines: &mut Vec<String>| {
        omitted += entries.len().saturating_sub(limit);
        lines.extend(entries.into_iter().take(limit).map(|line| format!("- {line}")));
    };
    take(report.delivered.iter().map(|f| delivered_line(f, reference)).collect(), 3, &mut lines);
    take(report.finished_epics.iter().map(epic_line).collect(), 2, &mut lines);
    take(report.idle.iter().map(idle_line).collect(), 2, &mut lines);
    let mut full = head.clone();
    let tail_reserve = 80;
    let mut shown = 0usize;
    for line in &lines {
        if full.len() + 1 + line.len() + tail_reserve > SESSION_START_CAP_BYTES {
            break;
        }
        full.push('\n');
        full.push_str(line);
        shown += 1;
    }
    let more = omitted + (lines.len() - shown);
    full.push('\n');
    full.push_str(&if more > 0 {
        format!("+{more} more; run `cas doctor` for the full list. Nothing here closes on its own.")
    } else {
        "Nothing here closes on its own; each close goes through the ordinary gates.".to_string()
    });
    Some((full, compact))
}

/// The SessionStart block's byte cap.
pub(crate) const SESSION_START_CAP_BYTES: usize = 1_500;

#[cfg(test)]
#[path = "backlog_hygiene_tests.rs"]
mod tests;
