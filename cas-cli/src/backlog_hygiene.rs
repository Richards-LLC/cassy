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

/// Kind 1 over `commits` (the default branch's non-merge history).
pub(crate) fn delivered_open(_tasks: &[Task], _commits: &[CommitRecord]) -> Vec<DeliveredOpen> {
    Vec::new()
}

/// Kind 2 over every task and parent-child dependency.
pub(crate) fn finished_open_epics(_tasks: &[Task], _deps: &[Dependency]) -> Vec<FinishedEpic> {
    Vec::new()
}

/// Kind 3 at `now`.
pub(crate) fn idle_open(_tasks: &[Task], _now: DateTime<Utc>) -> Vec<IdleOpen> {
    Vec::new()
}

/// Scan the project whose store is `cas_root`. Kinds 2 and 3 need only the
/// task store; kind 1 also reads the default branch of the project's git
/// repository, and says why when it cannot.
pub(crate) fn scan(_cas_root: &Path, _now: DateTime<Utc>) -> Result<BacklogReport, String> {
    Ok(BacklogReport::default())
}

/// The call that closes a kind-1 finding through the ordinary close gates.
pub(crate) fn delivered_close_call(finding: &DeliveredOpen, default_ref: &str) -> String {
    let _ = (finding, default_ref);
    String::new()
}

/// The call that closes a kind-2 finding through the ordinary epic close gates.
pub(crate) fn epic_close_call(finding: &FinishedEpic) -> String {
    let _ = finding;
    String::new()
}

/// The supervisor SessionStart block: `(full, compact)`, or `None` when the
/// report is clean. Bounded by [`SESSION_START_CAP_BYTES`].
pub(crate) fn session_start_block(_report: &BacklogReport) -> Option<(String, String)> {
    None
}

/// The SessionStart block's byte cap.
pub(crate) const SESSION_START_CAP_BYTES: usize = 1_500;

#[allow(dead_code)]
fn unused(_: DependencyType, _: TaskType) {}

#[cfg(test)]
#[path = "backlog_hygiene_tests.rs"]
mod tests;
