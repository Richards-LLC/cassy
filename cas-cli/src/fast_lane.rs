//! Fast lane for small frontend-only hotfixes (design:
//! `docs/design/frontend-hotfix-fast-lane.brief.md`).
//!
//! A project opts in with `[fast_lane]`. An eligible delivery merges on the
//! configured `merge_checks` alone. Its deferred checks (hermetic e2e, the
//! production build and the journey QA walk) become a post-merge receipt that
//! blocks close, and an attributable receipt failure opens a revert PR.
//! Eligibility is measured from the delivered diff, never declared.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use cas_types::{Task, TaskType};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A task label that admits non-bug work, such as an operator's demo fix.
pub const HOTFIX_LABEL: &str = "hotfix";
/// The decision a task records when its diff touches no journey.
pub const NO_JOURNEY_IMPACT: &str = "no journey impact:";
const ELIGIBLE: &str = "FAST LANE ELIGIBLE";
const REFUSED: &str = "FAST LANE REFUSED";
const MERGED: &str = "FAST LANE MERGE";
const RERUN: &str = "FAST LANE RERUN";
const FOLLOW_UP: &str = "FAST LANE FOLLOW-UP";
const PROBE: Duration = Duration::from_secs(10);
const GIT_WRITE: Duration = Duration::from_secs(120);

/// `[fast_lane]` project configuration. Check and status names are exact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FastLaneConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Every changed path must match one of these globs.
    #[serde(default)]
    pub paths: Vec<String>,
    /// No changed path may match one of these globs.
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(default = "default_max_files")]
    pub max_files: usize,
    #[serde(default = "default_max_lines")]
    pub max_lines: usize,
    /// Check-run names that alone admit the merge.
    #[serde(default)]
    pub merge_checks: Vec<String>,
    /// Check-run names that must pass on the merge commit before close.
    #[serde(default)]
    pub receipt_checks: Vec<String>,
    /// Commit-status contexts (e.g. `Vercel`) that must succeed on the merge commit.
    #[serde(default)]
    pub receipt_statuses: Vec<String>,
    #[serde(default = "default_window")]
    pub receipt_window_mins: u32,
}

fn default_max_files() -> usize {
    10
}
fn default_max_lines() -> usize {
    300
}
fn default_window() -> u32 {
    30
}

impl Default for FastLaneConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            paths: Vec::new(),
            deny: Vec::new(),
            max_files: default_max_files(),
            max_lines: default_max_lines(),
            merge_checks: Vec::new(),
            receipt_checks: Vec::new(),
            receipt_statuses: Vec::new(),
            receipt_window_mins: default_window(),
        }
    }
}

/// The project's lane when enabled with paths and a merge check to gate on.
pub fn configured(cas_root: &Path) -> Option<FastLaneConfig> {
    crate::config::Config::load(cas_root)
        .ok()?
        .fast_lane
        .filter(|lane| lane.enabled && !lane.paths.is_empty() && !lane.merge_checks.is_empty())
}

/// One changed path and its added plus deleted lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Eligibility {
    Eligible { files: usize, lines: usize },
    Refused(String),
}

/// All-or-nothing eligibility of one delivery's diff (design rules 1 to 5).
pub fn evaluate(lane: &FastLaneConfig, task: &Task, changes: &[Change]) -> Eligibility {
    use crate::mcp::tools::core::task::lifecycle::review_loop::FINDING_LABEL;
    use crate::mcp::tools::core::task::lifecycle::review_tier::{VerificationTier, tier_for_paths};
    let labelled = |label: &str| task.labels.iter().any(|own| own == label);
    if task.task_type == TaskType::Epic
        || !(task.task_type == TaskType::Bug || labelled(FINDING_LABEL) || labelled(HOTFIX_LABEL))
    {
        return Eligibility::Refused(format!(
            "a {} task is plan or phase work; the lane takes bugs, review findings and tasks labelled `{HOTFIX_LABEL}`",
            task.task_type
        ));
    }
    if changes.is_empty() {
        return Eligibility::Refused("the delivery changes no files".into());
    }
    let allowed: Vec<&str> = lane.paths.iter().map(String::as_str).collect();
    let denied: Vec<&str> = lane.deny.iter().map(String::as_str).collect();
    if let Some(change) = changes
        .iter()
        .find(|change| !crate::qa_registry::matches_path(allowed.iter().copied(), &change.path))
    {
        return Eligibility::Refused(format!("{} is outside fast_lane.paths", change.path));
    }
    if let Some(change) = changes
        .iter()
        .find(|change| crate::qa_registry::matches_path(denied.iter().copied(), &change.path))
    {
        return Eligibility::Refused(format!("{} matches fast_lane.deny", change.path));
    }
    let files = changes.len();
    let lines: usize = changes.iter().map(|change| change.lines).sum();
    if files > lane.max_files {
        return Eligibility::Refused(format!("{files} files exceed the {}-file cap", lane.max_files));
    }
    if lines > lane.max_lines {
        return Eligibility::Refused(format!("{lines} changed lines exceed the {}-line cap", lane.max_lines));
    }
    let paths: Vec<String> = changes.iter().map(|change| change.path.clone()).collect();
    if let Some((VerificationTier::Adversarial, reason)) = tier_for_paths(&paths) {
        return Eligibility::Refused(format!("Tier 2 delivery: {reason}"));
    }
    Eligibility::Eligible { files, lines }
}

fn git(repo: &Path, args: &[&str], cap: Duration) -> Result<String, String> {
    let mut command = Command::new("git");
    command.current_dir(repo).args(args);
    let output = crate::bounded_process::run_command(
        &mut command,
        crate::bounded_process::Deadline::after(cap),
        cap,
    )
    .map_err(|error| format!("git {}: {error:?}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The commit `rev` names in `repo`.
pub fn resolve(repo: &Path, rev: &str) -> Option<String> {
    git(repo, &["rev-parse", "--verify", "--quiet", "--end-of-options", &format!("{rev}^{{commit}}")], PROBE).ok()
}

/// Changed paths and line counts of `base..head`; a binary file counts zero lines.
pub fn delivery_changes(repo: &Path, base: &str, head: &str) -> Result<Vec<Change>, String> {
    let out = git(repo, &["diff", "--numstat", "--no-renames", "-z", base, head, "--"], PROBE)?;
    Ok(out
        .split('\0')
        .filter_map(|record| {
            let mut fields = record.splitn(3, '\t');
            let added = fields.next()?.trim();
            let deleted = fields.next()?;
            let path = fields.next()?.to_string();
            let lines = added.parse::<usize>().unwrap_or(0) + deleted.parse::<usize>().unwrap_or(0);
            (!path.is_empty()).then_some(Change { path, lines })
        })
        .collect())
}

/// Eligibility of `head` against its merge base with the live target.
pub fn evaluate_delivery(lane: &FastLaneConfig, task: &Task, repo: &Path, target: &str, head: &str) -> Eligibility {
    let target = target.strip_prefix("origin/").unwrap_or(target);
    let origin = format!("origin/{target}");
    let target_ref = if git(repo, &["rev-parse", "--verify", "--quiet", &origin], PROBE).is_ok() {
        origin
    } else {
        target.to_string()
    };
    match git(repo, &["merge-base", &target_ref, head], PROBE)
        .and_then(|base| delivery_changes(repo, &base, head))
    {
        Ok(changes) => evaluate(lane, task, &changes),
        Err(error) => Eligibility::Refused(format!("the delivery diff is unavailable ({error})")),
    }
}

/// The decision note recorded at park, for the verifier and operator to audit.
pub fn eligibility_note(lane: &FastLaneConfig, head: &str, eligibility: &Eligibility) -> String {
    match eligibility {
        Eligibility::Eligible { files, lines } => format!(
            "{ELIGIBLE} head={head} files={files} lines={lines}: merges on {}; upkeep and QA evidence are deferred to a post-merge receipt due {} minutes after merge.",
            lane.merge_checks.join(", "),
            lane.receipt_window_mins
        ),
        Eligibility::Refused(reason) => {
            format!("{REFUSED} head={head}: {reason}. The delivery takes the normal path.")
        }
    }
}

/// Recorded by `worktree_merge` when it admitted `head` on the lane.
pub fn merge_note(head: &str, merge: &str, at: DateTime<Utc>) -> String {
    format!("{MERGED} head={head} merge={merge} at={}: post-merge receipt owed.", at.to_rfc3339())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRecord {
    pub head: String,
    pub at: DateTime<Utc>,
}

/// The lane merge recorded for `merge`, if this delivery merged on the lane.
pub fn merged_record(notes: &str, merge: &str) -> Option<MergeRecord> {
    notes.lines().rev().find_map(|line| {
        let rest = &line[line.find(MERGED)? + MERGED.len()..];
        let field = |name: &str| {
            rest.split_whitespace()
                .find_map(|token| token.strip_prefix(name))
                .map(|value| value.trim_end_matches(':'))
        };
        (field("merge=")? == merge).then_some(())?;
        Some(MergeRecord {
            head: field("head=")?.to_string(),
            at: DateTime::parse_from_rfc3339(field("at=")?).ok()?.with_timezone(&Utc),
        })
    })
}

pub fn rerun_note(merge: &str, check: &Check) -> String {
    format!(
        "{RERUN} merge={merge} check=\"{}\" run={}: rerunning the failed jobs once.",
        check.name,
        check.run_id.as_deref().unwrap_or("unknown")
    )
}

pub fn reran(notes: &str, merge: &str, check: &str) -> bool {
    notes.contains(&format!("{RERUN} merge={merge} check=\"{check}\""))
}

pub fn follow_up_note(key: &str, task_id: &str) -> String {
    format!("{FOLLOW_UP} {key}: filed {task_id}.")
}

pub fn followed_up(notes: &str, key: &str) -> bool {
    notes.contains(&format!("{FOLLOW_UP} {key}: filed "))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Success,
    Failure,
}

/// One check run or commit status on a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub state: State,
    pub url: Option<String>,
    pub run_id: Option<String>,
}

/// The newest run per check name from a `commits/{sha}/check-runs` response.
/// Unfinished or cancelled runs are pending; anything but success is a failure.
pub fn check_runs(response: &serde_json::Value) -> Vec<Check> {
    let mut newest: std::collections::BTreeMap<String, (u64, Check)> = Default::default();
    for run in response.get("check_runs").and_then(serde_json::Value::as_array).into_iter().flatten() {
        let Some(name) = run.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let text = |key: &str| run.get(key).and_then(serde_json::Value::as_str);
        let state = match (text("status"), text("conclusion")) {
            (Some("completed"), Some("success")) => State::Success,
            (Some("completed"), Some("cancelled")) | (_, None) => State::Pending,
            (Some("completed"), Some(_)) => State::Failure,
            _ => State::Pending,
        };
        let url = text("html_url").map(str::to_string);
        let run_id = url
            .as_deref()
            .and_then(|url| url.split_once("/actions/runs/"))
            .and_then(|(_, tail)| tail.split('/').next())
            .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
            .map(str::to_string);
        let id = run.get("id").and_then(serde_json::Value::as_u64).unwrap_or(0);
        let check = Check { name: name.to_string(), state, url, run_id };
        if newest.get(name).is_none_or(|(seen, _)| id >= *seen) {
            newest.insert(name.to_string(), (id, check));
        }
    }
    newest.into_values().map(|(_, check)| check).collect()
}

/// Statuses from a combined `commits/{sha}/status` response, newest per context.
pub fn statuses(response: &serde_json::Value) -> Vec<Check> {
    let mut seen = std::collections::BTreeSet::new();
    response
        .get("statuses")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|status| {
            let name = status.get("context")?.as_str()?.to_string();
            seen.insert(name.clone()).then_some(())?;
            let state = match status.get("state")?.as_str()? {
                "success" => State::Success,
                "pending" => State::Pending,
                _ => State::Failure,
            };
            let url = status.get("target_url").and_then(serde_json::Value::as_str).map(str::to_string);
            Some(Check { name, state, url, run_id: None })
        })
        .collect()
}

fn find<'a>(checks: &'a [Check], name: &str) -> Option<&'a Check> {
    checks.iter().find(|check| check.name == name)
}

/// Merge admission for an eligible tip: only `merge_checks` are judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Green(Vec<String>),
    Pending(String),
    Red { name: String, url: Option<String> },
}

pub fn merge_gate(lane: &FastLaneConfig, checks: &[Check]) -> Gate {
    let mut waiting = Vec::new();
    for name in &lane.merge_checks {
        match find(checks, name) {
            Some(check) if check.state == State::Failure => {
                return Gate::Red { name: name.clone(), url: check.url.clone() };
            }
            Some(check) if check.state == State::Success => {}
            Some(_) => waiting.push(format!("{name} is running")),
            None => waiting.push(format!("{name} has not reported")),
        }
    }
    if waiting.is_empty() {
        Gate::Green(lane.merge_checks.clone())
    } else {
        Gate::Pending(waiting.join("; "))
    }
}

/// Check runs and statuses observed on one commit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observed {
    pub checks: Vec<Check>,
    pub statuses: Vec<Check>,
}

/// What the post-merge receipt requires next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Receipt {
    /// Named checks or statuses have not finished.
    Pending(String),
    /// Every check passed; `excused` failed on the merge's first parent too.
    Passed { excused: Vec<Check> },
    /// A receipt check failed for the first time: rerun it once.
    Rerun(Check),
    /// Still red after its rerun and green on the first parent.
    Revert(Check),
    /// A receipt status (the production build) failed; production is unchanged.
    Return(Check),
}

pub fn receipt(
    lane: &FastLaneConfig,
    merge: &Observed,
    parent: Option<&Observed>,
    reran: impl Fn(&str) -> bool,
) -> Receipt {
    if let Some(status) = lane
        .receipt_statuses
        .iter()
        .find_map(|name| find(&merge.statuses, name).filter(|status| status.state == State::Failure))
    {
        return Receipt::Return(status.clone());
    }
    let mut waiting = Vec::new();
    let mut excused = Vec::new();
    for name in &lane.receipt_checks {
        match find(&merge.checks, name) {
            Some(check) if check.state == State::Failure => {
                if !reran(name) {
                    return Receipt::Rerun(check.clone());
                }
                match parent.and_then(|parent| find(&parent.checks, name)).map(|check| check.state) {
                    Some(State::Success) => return Receipt::Revert(check.clone()),
                    Some(State::Pending) => waiting.push(format!("{name} on the first parent is running")),
                    _ => excused.push(check.clone()),
                }
            }
            Some(check) if check.state == State::Success => {}
            Some(_) => waiting.push(format!("{name} is running")),
            None => waiting.push(format!("{name} has not reported")),
        }
    }
    for name in &lane.receipt_statuses {
        match find(&merge.statuses, name).map(|status| status.state) {
            Some(State::Success) => {}
            Some(_) => waiting.push(format!("{name} is pending")),
            None => waiting.push(format!("{name} has not reported")),
        }
    }
    if waiting.is_empty() {
        Receipt::Passed { excused }
    } else {
        Receipt::Pending(waiting.join("; "))
    }
}

/// The journey walk leg: a ledger with rows and no FAIL, or a `no journey impact:` decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Walk {
    Done,
    Missing,
    Failed(String),
}

pub fn journey_walk(ledger: &Path, notes: &str) -> Walk {
    if ledger.exists() {
        let report = crate::qa_ledger::check(ledger, 1);
        if let Some(row) = report.rows.iter().find(|row| row.verdict == crate::qa_ledger::Verdict::Fail) {
            return Walk::Failed(format!("{} {}: {}", row.id, row.cell, row.observed));
        }
        if report.ok && !report.rows.is_empty() {
            return Walk::Done;
        }
    }
    let decided = notes.lines().any(|line| {
        line.split_once("✅ DECISION ")
            .and_then(|(_, decision)| decision.strip_prefix(NO_JOURNEY_IMPACT))
            .is_some_and(|reason| !reason.trim().is_empty())
    });
    if decided { Walk::Done } else { Walk::Missing }
}

fn gh(binary: &Path, cwd: &Path, args: &[&str], cap: Duration) -> Result<String, String> {
    let mut command = Command::new(binary);
    command.current_dir(cwd).env("GH_HOST", "github.com").args(args);
    let output = crate::bounded_process::run_command(
        &mut command,
        crate::bounded_process::Deadline::after(cap),
        cap,
    )
    .map_err(|error| format!("gh {}: {error:?}", args.first().unwrap_or(&"")))?;
    if !output.status.success() {
        return Err(format!(
            "gh {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Read the receipt's check runs and statuses on `sha` from GitHub.
pub fn observe(repo: &Path, sha: &str, binary: &Path) -> Result<Observed, String> {
    let slug = crate::github_repo::resolve_origin(repo, binary, PROBE)?.canonical;
    let read = |endpoint: String| -> Result<serde_json::Value, String> {
        let body = gh(binary, repo, &["api", "--method", "GET", &endpoint, "-F", "per_page=100"], PROBE)?;
        serde_json::from_str(&body).map_err(|error| format!("gh api {endpoint}: {error}"))
    };
    Ok(Observed {
        checks: check_runs(&read(format!("repos/{slug}/commits/{sha}/check-runs"))?),
        statuses: statuses(&read(format!("repos/{slug}/commits/{sha}/status"))?),
    })
}

/// Rerun a check's failed jobs once.
pub fn rerun(repo: &Path, check: &Check, binary: &Path) -> Result<(), String> {
    let run_id = check
        .run_id
        .as_deref()
        .ok_or_else(|| format!("{} has no Actions run to rerun", check.name))?;
    gh(binary, repo, &["run", "rerun", run_id, "--failed"], PROBE).map(|_| ())
}

/// Revert `merge` on `target` in a scratch worktree, push
/// `fast-lane-revert/<task>` and open a PR; returns the PR URL.
pub fn open_revert_pr(
    repo: &Path,
    target: &str,
    merge: &str,
    task_id: &str,
    failed: &Check,
    binary: &Path,
) -> Result<String, String> {
    let target = target.strip_prefix("origin/").unwrap_or(target);
    let branch = format!("fast-lane-revert/{task_id}");
    git(repo, &["fetch", "--quiet", "origin", target], GIT_WRITE)?;
    let scratch = tempfile::Builder::new()
        .prefix("fast-lane-revert-")
        .tempdir()
        .map_err(|error| format!("revert scratch directory: {error}"))?;
    let tree = scratch.path().join("tree");
    let tree_arg = tree.to_string_lossy().to_string();
    git(repo, &["worktree", "add", "--quiet", "--detach", &tree_arg, &format!("origin/{target}")], GIT_WRITE)?;
    let result = (|| -> Result<String, String> {
        let parents = git(&tree, &["rev-list", "--parents", "-n", "1", merge], PROBE)?;
        let mut revert = vec!["revert", "--no-edit"];
        if parents.split_whitespace().count() > 2 {
            revert.extend(["-m", "1"]);
        }
        revert.push(merge);
        git(&tree, &revert, GIT_WRITE)?;
        git(&tree, &["push", "--quiet", "origin", &format!("HEAD:refs/heads/{branch}")], GIT_WRITE)?;
        let title = format!("Revert hotfix merge {}", &merge[..merge.len().min(12)]);
        let body = format!(
            "Automatic fast-lane revert. `{}` failed on merge {merge} after one rerun and passed on its first parent.\n\nFailing run: {}",
            failed.name,
            failed.url.as_deref().unwrap_or("unavailable")
        );
        let created = gh(
            binary,
            &tree,
            &["pr", "create", "--base", target, "--head", &branch, "--title", &title, "--body", &body],
            PROBE,
        )?;
        Ok(created.lines().last().unwrap_or_default().to_string())
    })();
    let _ = git(repo, &["worktree", "remove", "--force", &tree_arg], GIT_WRITE);
    result
}

/// File one follow-up task related to the hotfix.
pub fn file_follow_up(
    store: &dyn cas_store::TaskStore,
    hotfix: &Task,
    title: String,
    description: String,
) -> Result<String, String> {
    let id = store.generate_id().map_err(|error| error.to_string())?;
    let mut task = Task::new(id.clone(), title);
    task.task_type = TaskType::Chore;
    task.priority = cas_types::Priority::HIGH;
    task.risk = vec![cas_types::TaskRisk::None];
    task.labels = vec!["fast-lane-follow-up".to_string()];
    task.description = description;
    store.add(&task).map_err(|error| error.to_string())?;
    store
        .add_dependency(&cas_types::Dependency::new(
            id.clone(),
            hotfix.id.clone(),
            cas_types::DependencyType::Related,
        ))
        .map_err(|error| error.to_string())?;
    Ok(id)
}

#[cfg(test)]
#[path = "fast_lane_tests.rs"]
mod tests;
