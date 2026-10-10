//! Release-epic integration gate (cas-6f48).
//!
//! 3.50.0 found six release blockers only at assembly. The daemon's rolling
//! integration already runs the full workspace suite and the no-build release
//! rows on the union of open epics after every `worktree_merge`; what was
//! missing is a consequence. This module keeps, per epic branch, the last tip
//! that passed and, while the epic is red, the first merge that broke it:
//!
//! - the daemon records green or red after each integration run
//!   ([`record_green`], [`record_red`]), naming the culprit merge found by a
//!   cheap bisect over the merges since the last green tip
//!   ([`first_red_merge`]: only the failing tests run at each step);
//! - `worktree_merge` into a red epic refuses ([`merge_refusal`]) unless a
//!   registered supervisor overrides with a reason ([`record_override`]);
//! - `worker_status` and `epic_status` show the gate ([`status_section`],
//!   [`epic_status_line`]).
//!
//! State lives in `<shared .cas>/merge-sweeps/epic-gate.json`, keyed by epic
//! branch, and is written atomically (temp file plus rename): one writer (the
//! daemon's integration lock holder), any number of readers.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Gate file, relative to the shared `.cas` directory.
pub const GATE_FILE: &str = "merge-sweeps/epic-gate.json";

/// Most first-parent merges a bisect considers when no green tip is known.
pub const MAX_BISECT_WINDOW: usize = 32;

/// All epic gates of one repository, keyed by epic branch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateFile {
    #[serde(default)]
    pub epics: BTreeMap<String, EpicGate>,
}

/// One epic branch's integration state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpicGate {
    pub epic_id: String,
    pub branch: String,
    /// Newest epic tip whose integration run passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_green_tip: Option<String>,
    /// Newest epic tip an integration run judged, green or red.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checked_tip: Option<String>,
    /// Set while the epic is red; cleared by the next green run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub red: Option<RedTip>,
    #[serde(default)]
    pub updated_at: String,
}

/// Why and since when an epic is red.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedTip {
    /// The epic tip the failing run integrated.
    pub tip: String,
    /// The first merge on the epic whose tip fails, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_red_merge: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_red_subject: Option<String>,
    /// Failing tests or release rows.
    #[serde(default)]
    pub failing: Vec<String>,
    pub detail: String,
    pub since: String,
    /// Cost of attributing the culprit: probe runs and wall seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bisect: Option<BisectCost>,
    /// Supervisor overrides accepted while red.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BisectCost {
    pub runs: usize,
    pub secs: u64,
}

// Red stub (cas-6f48): the gate is not implemented yet.
pub fn load(_cas_dir: &Path) -> GateFile {
    GateFile::default()
}

pub fn record_green(
    _cas_dir: &Path,
    _epic_id: &str,
    _branch: &str,
    _tip: &str,
) -> Result<(), String> {
    Ok(())
}

pub fn record_red(
    _cas_dir: &Path,
    _epic_id: &str,
    _branch: &str,
    _red: RedTip,
) -> Result<(), String> {
    Ok(())
}

pub fn record_override(_cas_dir: &Path, _branch: &str, _note: &str) -> Result<(), String> {
    Ok(())
}

pub fn merge_refusal(_cas_dir: &Path, _target_branch: &str) -> Option<String> {
    None
}

pub fn epic_status_line(_cas_dir: &Path, _branch: &str) -> String {
    String::new()
}

pub fn status_section(_cas_dir: &Path) -> String {
    String::new()
}

pub fn bisect_candidates(
    _repo: &Path,
    _last_green: Option<&str>,
    _tip: &str,
) -> Result<Vec<String>, String> {
    Ok(Vec::new())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Culprit {
    pub commit: String,
    pub isolated: bool,
    pub runs: usize,
}

pub fn first_red_merge(
    _repo: &Path,
    _last_green: Option<&str>,
    _tip: &str,
    _probe: &mut impl FnMut(&str) -> Result<bool, String>,
) -> Result<Option<Culprit>, String> {
    Ok(None)
}

pub fn commit_subject(_repo: &Path, _commit: &str) -> Option<String> {
    None
}

#[cfg(test)]
#[path = "epic_gate_tests.rs"]
mod tests;
