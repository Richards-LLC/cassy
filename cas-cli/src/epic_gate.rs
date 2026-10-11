//! Release-epic integration gate (cas-6f48).
//!
//! The October 2026 release cut found six blockers only at assembly. The
//! daemon's rolling integration already runs the full workspace suite and the no-build release
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
use std::path::{Path, PathBuf};

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

fn gate_path(cas_dir: &Path) -> PathBuf {
    cas_dir.join(GATE_FILE)
}

/// Read the gate file. A missing or unreadable file is an empty gate: the
/// gate never blocks on its own corruption.
pub fn load(cas_dir: &Path) -> GateFile {
    std::fs::read(gate_path(cas_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(cas_dir: &Path, gate: &GateFile) -> Result<(), String> {
    let path = gate_path(cas_dir);
    let parent = path.parent().ok_or("gate file has no parent")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let tmp = parent.join(format!(".epic-gate.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(gate).map_err(|error| error.to_string())?;
    std::fs::write(&tmp, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|error| error.to_string())
}

fn update(
    cas_dir: &Path,
    branch: &str,
    epic_id: &str,
    change: impl FnOnce(&mut EpicGate),
) -> Result<(), String> {
    let mut gate = load(cas_dir);
    let entry = gate
        .epics
        .entry(branch.to_string())
        .or_insert_with(|| EpicGate {
            epic_id: epic_id.to_string(),
            branch: branch.to_string(),
            ..Default::default()
        });
    entry.epic_id = epic_id.to_string();
    change(entry);
    entry.updated_at = chrono::Utc::now().to_rfc3339();
    save(cas_dir, &gate)
}

/// An integration run passed with `tip` of `branch`: the epic is green there.
pub fn record_green(cas_dir: &Path, epic_id: &str, branch: &str, tip: &str) -> Result<(), String> {
    update(cas_dir, branch, epic_id, |entry| {
        entry.last_green_tip = Some(tip.to_string());
        entry.last_checked_tip = Some(tip.to_string());
        entry.red = None;
    })
}

/// An integration run failed and named `branch` as the cause.
///
/// A later red run of the same episode keeps the first culprit: the merge
/// that broke the epic stays named until a green run clears it.
pub fn record_red(cas_dir: &Path, epic_id: &str, branch: &str, red: RedTip) -> Result<(), String> {
    update(cas_dir, branch, epic_id, |entry| {
        entry.last_checked_tip = Some(red.tip.clone());
        let red = match entry.red.take() {
            Some(previous) if previous.first_red_merge.is_some() => RedTip {
                first_red_merge: previous.first_red_merge,
                first_red_subject: previous.first_red_subject,
                since: previous.since,
                overrides: previous.overrides,
                ..red
            },
            Some(previous) => RedTip {
                since: previous.since,
                overrides: previous.overrides,
                ..red
            },
            None => red,
        };
        entry.red = Some(red);
    })
}

/// A supervisor merged into a red epic anyway; remember who and why.
pub fn record_override(cas_dir: &Path, branch: &str, note: &str) -> Result<(), String> {
    let gate = load(cas_dir);
    let Some(epic_id) = gate
        .epics
        .get(branch)
        .filter(|entry| entry.red.is_some())
        .map(|entry| entry.epic_id.clone())
    else {
        return Ok(());
    };
    update(cas_dir, branch, &epic_id, |entry| {
        if let Some(red) = entry.red.as_mut() {
            red.overrides.push(note.to_string());
        }
    })
}

fn short(sha: &str) -> &str {
    sha.get(..9).unwrap_or(sha)
}

fn culprit_text(red: &RedTip) -> String {
    match (&red.first_red_merge, &red.first_red_subject) {
        (Some(merge), Some(subject)) => format!("first red merge {} \"{subject}\"", short(merge)),
        (Some(merge), None) => format!("first red merge {}", short(merge)),
        _ => format!(
            "red at tip {} (culprit merge not isolated)",
            short(&red.tip)
        ),
    }
}

fn failing_text(red: &RedTip) -> String {
    const SHOWN: usize = 5;
    if red.failing.is_empty() {
        return red.detail.clone();
    }
    let mut text = red
        .failing
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if red.failing.len() > SHOWN {
        text.push_str(&format!(" and {} more", red.failing.len() - SHOWN));
    }
    text
}

/// The refusal for a merge into `target_branch`, while that epic is red.
pub fn merge_refusal(cas_dir: &Path, target_branch: &str) -> Option<String> {
    let gate = load(cas_dir);
    let entry = gate.epics.get(target_branch)?;
    let red = entry.red.as_ref()?;
    Some(format!(
        "EPIC GATE RED: refusing worktree_merge into {branch} ({epic}). Its integration run \
         failed since {since}: {culprit}; failing: {failing}. Last green tip: {green}. Merge \
         the fix with supervisor_override=true and a non-empty reason, or revert the culprit; \
         the next green integration run clears the gate. No merge was attempted.",
        branch = entry.branch,
        epic = entry.epic_id,
        since = red.since,
        culprit = culprit_text(red),
        failing = failing_text(red),
        green = entry
            .last_green_tip
            .as_deref()
            .map(short)
            .unwrap_or("none recorded"),
    ))
}

/// One line for `epic_status`: the gate of `branch`, or empty when unknown.
pub fn epic_status_line(cas_dir: &Path, branch: &str) -> String {
    let gate = load(cas_dir);
    let Some(entry) = gate.epics.get(branch) else {
        return String::new();
    };
    match &entry.red {
        Some(red) => format!(
            "\nIntegration gate: RED since {} — {}; failing: {}. Merges into {} are refused \
             until a green run (supervisor_override to merge the fix).\n",
            red.since,
            culprit_text(red),
            failing_text(red),
            entry.branch
        ),
        None => entry
            .last_green_tip
            .as_deref()
            .map(|tip| format!("\nIntegration gate: green at {}.\n", short(tip)))
            .unwrap_or_default(),
    }
}

/// The `worker_status` section: every red epic gate, or nothing.
pub fn status_section(cas_dir: &Path) -> String {
    let gate = load(cas_dir);
    let red: Vec<String> = gate
        .epics
        .values()
        .filter_map(|entry| {
            entry.red.as_ref().map(|red| {
                format!(
                    "⛔ Integration gate RED: {} ({}) since {} — {}; failing: {}. Merges into it \
                     are refused until green.",
                    entry.branch,
                    entry.epic_id,
                    red.since,
                    culprit_text(red),
                    failing_text(red)
                )
            })
        })
        .collect();
    if red.is_empty() {
        String::new()
    } else {
        format!("{}\n\n", red.join("\n"))
    }
}

/// The merges a bisect walks for `tip`: its first-parent commits after
/// `last_green` (oldest first), or its newest [`MAX_BISECT_WINDOW`] when no
/// green tip is known or `last_green` is not an ancestor.
pub fn bisect_candidates(
    repo: &Path,
    last_green: Option<&str>,
    tip: &str,
) -> Result<Vec<String>, String> {
    let range = match last_green {
        Some(green) if is_ancestor(repo, green, tip) => format!("{green}..{tip}"),
        _ => tip.to_string(),
    };
    let output = std::process::Command::new("git")
        .args([
            "rev-list",
            "--first-parent",
            &format!("--max-count={MAX_BISECT_WINDOW}"),
            &range,
        ])
        .current_dir(repo)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "git rev-list {range}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut commits: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    commits.reverse();
    Ok(commits)
}

fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> bool {
    std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(repo)
        .status()
        .is_ok_and(|status| status.success())
}

/// Result of a culprit bisect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Culprit {
    /// The first candidate whose failing tests fail.
    pub commit: String,
    /// Whether an earlier candidate was seen passing (or a green tip bounds
    /// the range); false means "this or an earlier merge".
    pub isolated: bool,
    pub runs: usize,
}

/// Find the first merge on `tip`'s first-parent history, after `last_green`,
/// whose failing tests fail. `probe(commit)` runs only those tests and
/// answers whether they pass. The tip is known to fail, so it is never
/// probed; each probe halves the range.
///
/// Returns `None` when the tests pass at the tip itself on its own — the
/// failure exists only in the integration union.
pub fn first_red_merge(
    repo: &Path,
    last_green: Option<&str>,
    tip: &str,
    probe: &mut impl FnMut(&str) -> Result<bool, String>,
) -> Result<Option<Culprit>, String> {
    let candidates = bisect_candidates(repo, last_green, tip)?;
    let Some(last) = candidates.len().checked_sub(1) else {
        return Ok(None);
    };
    let mut runs = 0;
    // The integration union failed, but the epic alone may not.
    runs += 1;
    if probe(&candidates[last])? {
        return Ok(None);
    }
    let bounded = last_green.is_some_and(|green| is_ancestor(repo, green, tip));
    let (mut lo, mut hi) = (0usize, last);
    let mut saw_pass = bounded;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        runs += 1;
        if probe(&candidates[mid])? {
            saw_pass = true;
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Ok(Some(Culprit {
        commit: candidates[hi].clone(),
        isolated: saw_pass || hi > 0,
        runs,
    }))
}

/// The subject line of `commit`, for the refusal text.
pub fn commit_subject(repo: &Path, commit: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["log", "-1", "--format=%s", commit])
        .current_dir(repo)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|subject| !subject.is_empty())
}

#[cfg(test)]
#[path = "epic_gate_tests.rs"]
mod tests;
