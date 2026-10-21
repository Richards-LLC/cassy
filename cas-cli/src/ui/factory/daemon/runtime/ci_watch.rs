//! Low-frequency GitHub Actions failure watcher for live factory lanes.
//!
//! The factory refreshes its local state every two seconds, but GitHub is
//! deliberately queried only once a minute.  That is at most sixty REST calls
//! per hour for the run list plus the occasional failed-run detail lookup.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use cas_store::{NotificationPriority, PromptQueueStore};
use serde::Deserialize;

use crate::bounded_process::{Deadline, run_command};

/// The GitHub Actions polling cadence.  A factory uses at most 60 list calls
/// per hour; failed runs add one jobs call and one optional log lookup.
pub(crate) const CI_WATCH_INTERVAL: Duration = Duration::from_secs(60);
/// External reminder conditions use the same low-frequency cadence as the
/// GitHub watcher. Their state is the pending reminder row, so a daemon
/// restart simply evaluates the row again rather than losing an in-memory
/// edge detector.
pub(crate) const EXTERNAL_WAKE_INTERVAL: Duration = Duration::from_secs(60);
pub(crate) const EXTERNAL_BRANCH_CONTAINED_EVENT: &str = "branch_contained_in";
pub(crate) const EXTERNAL_TAG_EXISTS_EVENT: &str = "tag_exists";
/// cas-0f05: fires once every GitHub Actions run for a commit has completed.
pub(crate) const EXTERNAL_CI_COMPLETED_EVENT: &str = "ci_completed";
/// The watcher's cadence while a `ci_completed` reminder is pending, so a
/// completed run wakes the supervisor within a minute.
pub(crate) const CI_WAKE_INTERVAL: Duration = Duration::from_secs(30);

/// cas-e369: poll at the faster wake cadence while a `ci_completed` reminder
/// is pending or a tracked PR sits in the merge queue, so an ejection reaches
/// the supervisor within 30 s.
pub(crate) fn ci_watch_interval(ci_wake_pending: bool, tracked_pr_queued: bool) -> Duration {
    if ci_wake_pending || tracked_pr_queued {
        CI_WAKE_INTERVAL
    } else {
        CI_WATCH_INTERVAL
    }
}
const EXTERNAL_GIT_TIMEOUT: Duration = Duration::from_secs(2);
const GH_CALL_TIMEOUT: Duration = Duration::from_secs(8);
pub(crate) const REQUIRED_PR_LANE_CHECK: &str = "Scoped Validation (factory/PR)";
pub(crate) const DELIVERY_MERGE_CLOSE_GRACE: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CiFailure {
    pub branch: String,
    pub head_sha: String,
    pub run_id: u64,
    pub run_url: String,
    pub failing_job: String,
    pub failing_test: Option<String>,
    /// Older failed runs on this branch that the current failure supersedes.
    /// They are retained only to make the one current alert explain what did
    /// not get replayed to the supervisor.
    suppressed_red_runs: Vec<SuppressedCiRun>,
    /// cas-6c13: the integration branch that already contains the red
    /// commit. A merged lane's red run is a blocker, not a routine notice.
    pub merged_into: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SuppressedCiRun {
    head_sha: String,
    run_id: u64,
}

impl CiFailure {
    pub(crate) fn dedupe_key(&self) -> String {
        format!("ci-red-run:{}:{}", self.branch, self.head_sha)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrLaneFailure {
    pub task_id: String,
    pub worker: String,
    pub pr_number: u64,
    pub head_sha: String,
    pub run_id: u64,
    pub run_url: String,
    pub check_name: String,
}

impl PrLaneFailure {
    pub(crate) fn dedupe_key(&self) -> String {
        format!("pr-lane-failed:{}:{}", self.pr_number, self.head_sha)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CiWatchError {
    /// `gh` missing, unauthenticated, rate-limited, or otherwise unavailable.
    /// The daemon logs this only once per process and keeps running.
    Unavailable(String),
    Malformed(String),
}

impl std::fmt::Display for CiWatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "{reason}"),
            Self::Malformed(reason) => write!(f, "{reason}"),
        }
    }
}

pub(crate) trait CiTransport {
    fn completed_runs(&self) -> Result<Vec<CiRun>, CiWatchError>;
    /// The run's first job that actually failed (`failure` or `timed_out`),
    /// or `None` when no job did — a run whose jobs were only cancelled is not
    /// a red run (cas-c188).
    fn failing_job(&self, run_id: u64) -> Result<Option<String>, CiWatchError>;
    /// cas-c188: the branch's current tip on the forge, so a red run for an
    /// older commit is never relayed as the branch's state. `None` when the tip
    /// cannot be read; the watcher then keeps its previous behaviour.
    fn branch_tip(&self, _branch: &str) -> Result<Option<String>, CiWatchError> {
        Ok(None)
    }
    fn failed_log(&self, run_id: u64) -> Result<Option<String>, CiWatchError>;
    fn merge_queue_pull_requests(&self) -> Result<Vec<MergeQueuePullRequest>, CiWatchError>;
    fn delivery_pull_requests(
        &self,
        branch: &str,
    ) -> Result<Vec<DeliveryPullRequest>, CiWatchError>;
    /// cas-0f05: every run for one commit, whatever its status.
    fn runs_for_sha(&self, _sha: &str) -> Result<Vec<CiRun>, CiWatchError> {
        Ok(Vec::new())
    }
    /// cas-e845: the commit's check-runs (`check_runs` objects), or `None`
    /// when this transport cannot read them.
    fn check_runs_for_sha(
        &self,
        _sha: &str,
    ) -> Result<Option<Vec<serde_json::Value>>, CiWatchError> {
        Ok(None)
    }
    /// cas-e369: the run's first failed job and, when known, its failed step.
    fn failed_job_detail(&self, run_id: u64) -> Result<Option<FailedJob>, CiWatchError> {
        Ok(self.failing_job(run_id)?.map(|name| FailedJob { name, step: None }))
    }
}

/// cas-e369: a failed job and the step inside it that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FailedJob {
    pub name: String,
    pub step: Option<String>,
}

/// A `ci_completed` reminder filter: `{"sha":"<sha>","workflow":"CI"}`, the
/// workflow being optional. GitHub's `head_sha` filter needs the full SHA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CiCompleted {
    pub sha: String,
    pub workflow: Option<String>,
}

pub(crate) fn parse_ci_completed(filter: &serde_json::Value) -> Result<CiCompleted, String> {
    let field = |name: &str| filter.get(name).and_then(serde_json::Value::as_str).map(str::trim);
    let sha = field("sha")
        .filter(|sha| sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or("ci_completed remind_filter requires `sha`, the full 40-character commit SHA")?;
    Ok(CiCompleted {
        sha: sha.to_ascii_lowercase(),
        workflow: field("workflow").filter(|w| !w.is_empty()).map(str::to_string),
    })
}

/// The commit's runs once every one has completed; `None` while any is still
/// queued or running, or before the first run exists. Scheduled runs count
/// only when their workflow is named: they run on the default-branch tip on a
/// timer, and some sit queued for days.
pub(crate) fn ci_completed_runs(
    transport: &impl CiTransport,
    condition: &CiCompleted,
) -> Result<Option<Vec<CiRun>>, CiWatchError> {
    let runs: Vec<CiRun> = transport
        .runs_for_sha(&condition.sha)?
        .into_iter()
        .filter(|run| match &condition.workflow {
            Some(workflow) => &run.name == workflow,
            None => run.event != "schedule",
        })
        .collect();
    let done = !runs.is_empty() && runs.iter().all(|run| run.status == "completed");
    if !done {
        return Ok(None);
    }
    // cas-e845: a run reports completed/success while one of its jobs is still
    // in_progress. Gate on merge admission's check-run rules, limited to these
    // runs' check suites so scheduled or unrelated workflows never hold the
    // wake. cas-ae6a: a failed check, or one cancelled with no rerun, settles.
    // Runs without a suite id keep run status.
    let suites: Option<Vec<u64>> = runs.iter().map(|run| run.check_suite_id).collect();
    if let (Some(suites), Some(checks)) = (suites, transport.check_runs_for_sha(&condition.sha)?) {
        let ours: Vec<serde_json::Value> = checks
            .into_iter()
            .filter(|check| {
                check
                    .pointer("/check_suite/id")
                    .and_then(serde_json::Value::as_u64)
                    .is_some_and(|suite| suites.contains(&suite))
            })
            .collect();
        if crate::mcp::tools::core::workflow::check_runs_still_running(&ours) {
            return Ok(None);
        }
    }
    Ok(Some(runs))
}

/// Probe each pending `ci_completed` reminder once; a failed read leaves
/// the row pending for the next poll.
pub(crate) fn collect_ci_wakes(
    transport: &impl CiTransport,
    candidates: &[(cas_store::Reminder, CiCompleted)],
) -> Vec<CiWake> {
    let wake = |(reminder, condition): &(cas_store::Reminder, CiCompleted)| {
        match ci_completed_runs(transport, condition) {
            Ok(runs) => runs.map(|runs| CiWake {
                reminder: reminder.clone(),
                condition: condition.clone(),
                runs,
            }),
            Err(error) => {
                tracing::warn!(reminder_id = reminder.id, %error, "ci_completed probe unavailable");
                None
            }
        }
    };
    candidates.iter().filter_map(wake).collect()
}

/// The wake's overall verdict over its completed runs: `success`, `cancelled`
/// when nothing failed but a run was cancelled (cas-ae6a), else `failure`.
pub(crate) fn ci_outcome(runs: &[CiRun]) -> &'static str {
    let passed =
        |run: &CiRun| matches!(run.conclusion.as_deref(), Some("success" | "skipped" | "neutral"));
    let cancelled = |run: &CiRun| run.conclusion.as_deref() == Some("cancelled");
    if runs.iter().all(passed) {
        "success"
    } else if runs.iter().all(|run| passed(run) || cancelled(run)) {
        "cancelled"
    } else {
        "failure"
    }
}

/// A `ci_completed` reminder whose runs have all completed.
#[derive(Debug, Clone)]
pub(crate) struct CiWake {
    pub reminder: cas_store::Reminder,
    pub condition: CiCompleted,
    pub runs: Vec<CiRun>,
}

impl CiWake {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "sha": self.condition.sha,
            "workflow": self.condition.workflow,
            "outcome": ci_outcome(&self.runs),
            "runs": self.runs.iter().map(|run| serde_json::json!({
                "name": run.name, "conclusion": run.conclusion, "url": run.html_url,
            })).collect::<Vec<_>>(),
        })
    }

    pub(crate) fn description(&self) -> String {
        let run = |run: &CiRun| {
            let conclusion = run.conclusion.as_deref().unwrap_or("unknown");
            format!("{} {conclusion} {}", run.name, run.html_url)
        };
        let runs: Vec<String> = self.runs.iter().map(run).collect();
        format!("CI completed for {}: {}", &self.condition.sha[..12], runs.join("; "))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CiRun {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    pub head_branch: String,
    pub head_sha: String,
    pub html_url: String,
    pub status: String,
    pub conclusion: Option<String>,
    #[serde(default)]
    pub event: String,
    /// The run's check suite; its jobs are the check-runs that carry this id.
    #[serde(default)]
    pub check_suite_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AwaitingMergeDelivery {
    pub task_id: String,
    pub worker: String,
    pub branch: String,
    pub branch_tip: Option<String>,
    pub pr_number: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct DeliveryPullRequest {
    pub number: u64,
    #[serde(rename = "headRefName")]
    pub head_branch: String,
    #[serde(rename = "headRefOid")]
    pub head_sha: String,
    pub state: String,
    #[serde(rename = "mergedAt")]
    pub merged_at: Option<String>,
    #[serde(rename = "mergeCommit")]
    pub merge_commit: Option<DeliveryMergeCommit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct DeliveryMergeCommit {
    pub oid: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeliveryPrObservation {
    pub task_id: String,
    pub worker: String,
    pub branch: String,
    pub pr_number: u64,
    pub head_sha: String,
    pub merge_commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct MergeQueuePullRequest {
    #[serde(default)]
    pub url: Option<String>,
    pub number: u64,
    #[serde(rename = "headRefName")]
    pub head_branch: String,
    #[serde(rename = "headRefOid")]
    pub head_sha: String,
    #[serde(rename = "isInMergeQueue")]
    pub is_in_merge_queue: bool,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(rename = "autoMergeRequest")]
    pub auto_merge_request: Option<AutoMergeRequest>,
    #[serde(rename = "statusCheckRollup")]
    pub status_check_rollup: Option<StatusCheckRollup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct AutoMergeRequest {
    #[serde(rename = "enabledAt")]
    pub enabled_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct StatusCheckRollup {
    pub state: String,
}

impl MergeQueuePullRequest {
    fn auto_merge_armed(&self) -> bool {
        self.auto_merge_request.is_some()
    }

    fn checks_green(&self) -> bool {
        self.status_check_rollup
            .as_ref()
            .is_some_and(|rollup| rollup.state == "SUCCESS")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MergeQueueEjection {
    pub task_id: String,
    pub worker: String,
    pub pr_number: u64,
    pub failed_run_id: Option<u64>,
    pub occurrence: String,
}

/// cas-e369: a supervisor-owned PR (an open epic's `epic/` branch) that the
/// watcher tracks through the merge queue like a parked delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SupervisorPr {
    pub task_id: String,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct MergeQueuePoll {
    /// Reuse the bounded poll's actual PR records for read-only peer awareness.
    pub observed_prs: Vec<crate::cloud::peers_detail::PeerPr>,
    pub queued_prs: BTreeSet<u64>,
    pub ejections: Vec<MergeQueueEjection>,
    pub auto_merge_prs: BTreeSet<u64>,
    pub pr_lane_failures: Vec<PrLaneFailure>,
}

/// A durable condition encoded in an event reminder's JSON filter. The
/// commit form is preferred for branch containment because a merge queue may
/// delete the source branch after landing; callers may still use a branch ref
/// in `commit` when that ref is intentionally retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExternalWakeCondition {
    BranchContained {
        commit: String,
        target_branch: String,
    },
    TagExists { tag: String },
}

/// The ref and commit that an external condition actually compared. Keeping
/// this separate from the user-supplied filter makes the fired reminder
/// explain which remote state caused the wake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalWakeObservation {
    pub(crate) compared_ref: String,
    pub(crate) compared_sha: String,
}

impl ExternalWakeCondition {
    /// The remote read this condition depends on. Conditions sharing a key
    /// share one fetch or `ls-remote` per scan (GH #1208).
    pub(crate) fn probe_key(&self) -> ExternalProbeKey {
        match self {
            Self::BranchContained { target_branch, .. } => {
                ExternalProbeKey::Branch(target_branch.clone())
            }
            Self::TagExists { tag } => ExternalProbeKey::Tag(tag.clone()),
        }
    }

    pub(crate) fn event_type(&self) -> &'static str {
        match self {
            Self::BranchContained { .. } => EXTERNAL_BRANCH_CONTAINED_EVENT,
            Self::TagExists { .. } => EXTERNAL_TAG_EXISTS_EVENT,
        }
    }

    pub(crate) fn to_json(&self) -> serde_json::Value {
        match self {
            Self::BranchContained {
                commit,
                target_branch,
            } => serde_json::json!({
                "commit": commit,
                "target_branch": target_branch,
            }),
            Self::TagExists { tag } => serde_json::json!({"tag": tag}),
        }
    }

    pub(crate) fn description(&self) -> String {
        match self {
            Self::BranchContained {
                commit,
                target_branch,
            } => format!(
                "external condition satisfied: commit {commit} is contained in {target_branch}"
            ),
            Self::TagExists { tag } => {
                format!("external condition satisfied: tag {tag} exists")
            }
        }
    }

    pub(crate) fn description_with_observation(
        &self,
        observation: &ExternalWakeObservation,
    ) -> String {
        match self {
            Self::BranchContained { commit, .. } => format!(
                "external condition satisfied: commit {commit} is contained in {}@{}",
                observation.compared_ref, observation.compared_sha
            ),
            Self::TagExists { tag } => format!(
                "external condition satisfied: tag {tag} exists at {}@{}",
                observation.compared_ref, observation.compared_sha
            ),
        }
    }
}

/// GH #1208: `target_branch` names a branch on origin. A remote-tracking
/// spelling (`origin/staging`, `refs/remotes/origin/staging`) or a full branch
/// ref (`refs/heads/staging`) becomes the bare branch name. Unnormalised, it
/// fetched `+refs/heads/origin/staging`, which fails on every probe, so the
/// reminder could never fire. One prefix is stripped; an empty result is the
/// caller's to reject.
pub(crate) fn normalize_external_target_branch(target: &str) -> &str {
    let target = target.trim();
    ["refs/remotes/origin/", "refs/heads/", "origin/"]
        .iter()
        .find_map(|prefix| target.strip_prefix(prefix))
        .unwrap_or(target)
}

/// Select the first usable alias, shared by reminder creation and execution.
/// A present but empty or malformed preferred key must not hide a valid alias.
pub(crate) fn external_filter_string(
    filter: &serde_json::Value,
    names: &[&str],
) -> Option<String> {
    let object = filter.as_object()?;
    names.iter().find_map(|name| {
        object
            .get(*name)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && !value.chars().any(char::is_whitespace))
            .map(str::to_string)
    })
}

/// Parse the stable JSON contract accepted by `coordination.remind` for an
/// external condition. Keep this strict so a typo remains pending and visible
/// rather than silently becoming an always-true wake.
pub(crate) fn parse_external_wake_condition(
    event_type: &str,
    filter: &serde_json::Value,
) -> Result<ExternalWakeCondition, String> {
    filter
        .as_object()
        .ok_or_else(|| "external remind_filter must be a JSON object".to_string())?;
    let string_field = |name: &str| {
        external_filter_string(filter, &[name])
            .ok_or_else(|| format!("external remind_filter requires non-empty string field {name:?}"))
    };

    match event_type {
        EXTERNAL_BRANCH_CONTAINED_EVENT => {
            // `sha` names the commit for ci_completed and branch containment.
            let commit = external_filter_string(filter, &["commit", "branch_tip", "branch", "sha"])
                .ok_or_else(|| {
                    "branch_contained_in remind_filter requires string field `commit` (or `branch_tip`/`branch`/`sha`)".to_string()
                })?;
            let target_branch = external_filter_string(filter, &["target_branch", "target"])
                .ok_or_else(|| {
                    "branch_contained_in remind_filter requires string field `target_branch`".to_string()
                })?;
            let target_branch = normalize_external_target_branch(&target_branch).to_string();
            if target_branch.is_empty() {
                return Err(
                    "branch_contained_in target_branch must name a branch on origin".to_string(),
                );
            }
            if commit.starts_with('-') || target_branch.starts_with('-') {
                return Err("external git refs may not start with '-'".to_string());
            }
            Ok(ExternalWakeCondition::BranchContained {
                commit,
                target_branch,
            })
        }
        EXTERNAL_TAG_EXISTS_EVENT => {
            let tag = string_field("tag")?;
            if tag.starts_with('-') || tag.ends_with('/') {
                return Err("external tag must be a valid git ref name".to_string());
            }
            Ok(ExternalWakeCondition::TagExists { tag })
        }
        _ => Err(format!("unsupported external reminder event: {event_type}")),
    }
}

/// Evaluate one external condition using bounded git reads. Branch containment
/// refreshes the named origin branch, and tag existence queries origin directly,
/// so local refs cannot satisfy a reminder for unpublished state.
/// A normal non-zero git status means the condition is false (for example the
/// target branch or tag is not present yet); process failures/timeouts are
/// surfaced so the daemon can retain the pending row and retry on a later
/// cadence.
#[cfg(test)]
pub(crate) fn external_wake_condition_satisfied(
    project: &Path,
    condition: &ExternalWakeCondition,
) -> Result<bool, CiWatchError> {
    Ok(external_wake_condition_observation(project, condition)?.is_some())
}

/// Evaluate an external condition and return the exact ref/SHA that was
/// compared when it is satisfied. For branch conditions, the fetch is forced
/// into `refs/remotes/origin/<target>`; for tags, the remote is queried without
/// updating local refs. Failures return false without consulting local state.
#[cfg(test)]
pub(crate) fn external_wake_condition_observation(
    project: &Path,
    condition: &ExternalWakeCondition,
) -> Result<Option<ExternalWakeObservation>, CiWatchError> {
    ExternalWakeProbe::new(|args: &[String]| run_external_git_command(project, args))
        .observe(condition)
}

/// The remote read an external condition depends on: one origin branch or one
/// origin tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum ExternalProbeKey {
    Branch(String),
    Tag(String),
}

impl std::fmt::Display for ExternalProbeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Branch(branch) => write!(f, "origin branch {branch}"),
            Self::Tag(tag) => write!(f, "origin tag {tag}"),
        }
    }
}

/// The parts of one bounded git invocation an external probe reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalGitOutput {
    pub(crate) success: bool,
    pub(crate) stdout: String,
}

/// What one scan learned about a remote ref.
#[derive(Debug, Clone)]
enum RemoteRead {
    /// The ref exists on origin at this commit or tag object.
    Tip(String),
    /// Origin answered and the ref is absent, or its name is invalid.
    Absent,
    /// The fetch or `ls-remote` exited non-zero, for example a missing
    /// remote branch or an unreachable origin.
    Unreachable,
    /// git timed out or could not run.
    Unavailable(CiWatchError),
}

impl RemoteRead {
    fn failed(&self) -> bool {
        matches!(self, Self::Unreachable | Self::Unavailable(_))
    }
}

/// One scan's view of origin (GH #1208). Each remote branch is fetched, and
/// each tag listed, at most once per probe however many reminders name it;
/// the per-reminder work after that is local (`rev-parse`, `merge-base`).
/// `run` executes one bounded git command in the project, so tests can count
/// and fail the network calls.
pub(crate) struct ExternalWakeProbe<R> {
    run: R,
    reads: BTreeMap<ExternalProbeKey, RemoteRead>,
}

impl<R> ExternalWakeProbe<R>
where
    R: FnMut(&[String]) -> Result<ExternalGitOutput, CiWatchError>,
{
    pub(crate) fn new(run: R) -> Self {
        Self {
            run,
            reads: BTreeMap::new(),
        }
    }

    fn git(&mut self, args: &[&str]) -> Result<ExternalGitOutput, CiWatchError> {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
        (self.run)(&args)
    }

    fn resolve_commit(&mut self, revision: &str) -> Result<Option<String>, CiWatchError> {
        let output = self.git(&["rev-parse", "--verify", "--quiet", &format!("{revision}^{{commit}}")])?;
        let sha = output.stdout.trim();
        Ok((output.success && !sha.is_empty()).then(|| sha.to_string()))
    }

    fn read_branch(&mut self, target_branch: &str) -> RemoteRead {
        match self.git(&["check-ref-format", "--branch", target_branch]) {
            Err(error) => return RemoteRead::Unavailable(error),
            Ok(output) if !output.success => return RemoteRead::Absent,
            Ok(_) => {}
        }
        let target_ref = format!("refs/remotes/origin/{target_branch}");
        // Never fall back to an existing remote-tracking ref if the refresh
        // fails: that ref may be exactly the stale state that caused a false
        // positive such as reminder #1179.
        let refspec = format!("+refs/heads/{target_branch}:{target_ref}");
        match self.git(&["fetch", "--no-tags", "origin", &refspec]) {
            Err(error) => return RemoteRead::Unavailable(error),
            Ok(output) if !output.success => return RemoteRead::Unreachable,
            Ok(_) => {}
        }
        match self.resolve_commit(&target_ref) {
            Err(error) => RemoteRead::Unavailable(error),
            Ok(None) => RemoteRead::Absent,
            Ok(Some(sha)) => RemoteRead::Tip(sha),
        }
    }

    fn read_tag(&mut self, tag: &str) -> RemoteRead {
        let tag_ref = format!("refs/tags/{tag}");
        let output = match self.git(&["ls-remote", "--tags", "--refs", "origin", &tag_ref]) {
            Err(error) => return RemoteRead::Unavailable(error),
            Ok(output) if !output.success => return RemoteRead::Unreachable,
            Ok(output) => output,
        };
        // ls-remote patterns can match suffixes and globs. Accept only the
        // exact ref requested, including for annotated tags.
        output
            .stdout
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .find_map(|(sha, remote_ref)| {
                (remote_ref == tag_ref && !sha.is_empty()).then(|| sha.to_string())
            })
            .map_or(RemoteRead::Absent, RemoteRead::Tip)
    }

    fn remote_read(&mut self, key: &ExternalProbeKey) -> RemoteRead {
        if let Some(read) = self.reads.get(key) {
            return read.clone();
        }
        let read = match key {
            ExternalProbeKey::Branch(branch) => self.read_branch(branch),
            ExternalProbeKey::Tag(tag) => self.read_tag(tag),
        };
        self.reads.insert(key.clone(), read.clone());
        read
    }

    /// Evaluate one condition against this scan's view of origin. A normal
    /// non-zero git status means the condition is false (for example the
    /// target branch or tag is not present yet); timeouts and spawn failures
    /// are returned so the caller retains the pending row.
    pub(crate) fn observe(
        &mut self,
        condition: &ExternalWakeCondition,
    ) -> Result<Option<ExternalWakeObservation>, CiWatchError> {
        let compared_sha = match self.remote_read(&condition.probe_key()) {
            RemoteRead::Tip(sha) => sha,
            RemoteRead::Absent | RemoteRead::Unreachable => return Ok(None),
            RemoteRead::Unavailable(error) => return Err(error),
        };
        match condition {
            ExternalWakeCondition::BranchContained {
                commit,
                target_branch,
            } => {
                let Some(commit_sha) = self.resolve_commit(commit)? else {
                    return Ok(None);
                };
                let contained = self
                    .git(&["merge-base", "--is-ancestor", &commit_sha, &compared_sha])?
                    .success;
                Ok(contained.then(|| ExternalWakeObservation {
                    compared_ref: format!("refs/remotes/origin/{target_branch}"),
                    compared_sha,
                }))
            }
            ExternalWakeCondition::TagExists { tag } => Ok(Some(ExternalWakeObservation {
                compared_ref: format!("refs/tags/{tag}"),
                compared_sha,
            })),
        }
    }

    /// Each remote read this probe made, and whether it failed (a fetch or
    /// `ls-remote` that exited non-zero, timed out or could not run).
    pub(crate) fn read_outcomes(&self) -> Vec<(ExternalProbeKey, bool)> {
        self.reads
            .iter()
            .map(|(key, read)| (key.clone(), read.failed()))
            .collect()
    }
}

/// Run one bounded git command for an external reminder probe.
pub(crate) fn run_external_git_command(
    project: &Path,
    args: &[String],
) -> Result<ExternalGitOutput, CiWatchError> {
    // Reported when it runs on a daemon lane (cas-4491f): the reminder tests
    // count these instead of timing the pass.
    cas_store::lane_guard::note_blocking_call(
        cas_store::lane_guard::BlockingKind::Command,
        "git external reminder probe",
    );
    let mut command = Command::new("git");
    command.args(args).current_dir(project);
    run_command(
        &mut command,
        Deadline::after(EXTERNAL_GIT_TIMEOUT),
        EXTERNAL_GIT_TIMEOUT,
    )
    .map(|output| ExternalGitOutput {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
    })
    .map_err(|error| {
        CiWatchError::Unavailable(match error {
            crate::bounded_process::BoundedCommandError::TimedOut => {
                "git external reminder probe timed out".to_string()
            }
            crate::bounded_process::BoundedCommandError::Io => {
                "git is unavailable for external reminder probe".to_string()
            }
        })
    })
}

#[derive(Deserialize)]
struct RunsResponse {
    workflow_runs: Vec<CiRun>,
}

#[derive(Deserialize)]
struct JobsResponse {
    jobs: Vec<CiJob>,
}

#[derive(Deserialize)]
struct CiJob {
    name: String,
    conclusion: Option<String>,
    #[serde(default)]
    steps: Vec<CiStep>,
}

#[derive(Deserialize)]
struct CiStep {
    name: String,
    conclusion: Option<String>,
}

/// A job conclusion that makes a run red. `cancelled` and `skipped` do not:
/// a run whose jobs were all cancelled failed nothing (cas-c188).
fn job_conclusion_is_failure(conclusion: Option<&str>) -> bool {
    matches!(conclusion, Some("failure" | "timed_out"))
}

#[derive(Deserialize)]
struct BranchResponse {
    commit: BranchCommit,
}

#[derive(Deserialize)]
struct BranchCommit {
    sha: String,
}

#[derive(Deserialize)]
struct MergeQueueGraphqlResponse {
    data: MergeQueueGraphqlData,
}

#[derive(Deserialize)]
struct MergeQueueGraphqlData {
    repository: MergeQueueRepository,
}

#[derive(Deserialize)]
struct MergeQueueRepository {
    #[serde(rename = "pullRequests")]
    pull_requests: MergeQueuePullRequests,
}

#[derive(Deserialize)]
struct MergeQueuePullRequests {
    nodes: Vec<MergeQueuePullRequest>,
}

/// Real, bounded `gh` transport.  The watcher intentionally uses the CLI so
/// it inherits the operator's normal `gh auth`/token setup without persisting
/// credentials in Cassy.
pub(crate) struct GhCiTransport {
    repo: String,
    cwd: PathBuf,
    gh_binary: PathBuf,
}

impl GhCiTransport {
    pub(crate) fn from_project(project: &Path) -> Result<Self, CiWatchError> {
        let gh_binary = crate::github_repo::gh_binary();
        let repo = crate::github_repo::resolve_origin(project, &gh_binary, Duration::from_secs(2))
            .map_err(CiWatchError::Unavailable)?
            .canonical;
        Ok(Self {
            repo,
            cwd: project.to_path_buf(),
            gh_binary,
        })
    }

    fn gh_json<T: for<'de> Deserialize<'de>>(&self, args: &[String]) -> Result<T, CiWatchError> {
        let mut command = Command::new(&self.gh_binary);
        command.args(args).current_dir(&self.cwd);
        let output = run_command(
            &mut command,
            Deadline::after(GH_CALL_TIMEOUT),
            GH_CALL_TIMEOUT,
        )
        .map_err(|error| {
            CiWatchError::Unavailable(match error {
                crate::bounded_process::BoundedCommandError::TimedOut => {
                    "gh CI watcher request timed out".to_string()
                }
                crate::bounded_process::BoundedCommandError::Io => {
                    "gh is unavailable for the CI watcher".to_string()
                }
            })
        })?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr)
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("gh CI watcher request failed")
                .to_string();
            return Err(CiWatchError::Unavailable(detail));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|e| CiWatchError::Malformed(format!("invalid GitHub Actions JSON: {e}")))
    }

    fn gh_text(&self, args: &[String]) -> Result<String, CiWatchError> {
        let mut command = Command::new(&self.gh_binary);
        command.args(args).current_dir(&self.cwd);
        let output = run_command(
            &mut command,
            Deadline::after(GH_CALL_TIMEOUT),
            GH_CALL_TIMEOUT,
        )
        .map_err(|_| CiWatchError::Unavailable("gh log lookup unavailable".to_string()))?;
        if !output.status.success() {
            return Ok(String::new()); // Test extraction is optional.
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

impl CiTransport for GhCiTransport {
    fn completed_runs(&self) -> Result<Vec<CiRun>, CiWatchError> {
        let response: RunsResponse = self.gh_json(&[
            "api".to_string(),
            "-X".to_string(),
            "GET".to_string(),
            format!("repos/{}/actions/runs", self.repo),
            "-f".to_string(),
            "status=completed".to_string(),
            "-f".to_string(),
            "per_page=100".to_string(),
        ])?;
        Ok(response.workflow_runs)
    }

    fn failing_job(&self, run_id: u64) -> Result<Option<String>, CiWatchError> {
        let response: JobsResponse = self.gh_json(&[
            "api".to_string(),
            "-X".to_string(),
            "GET".to_string(),
            format!("repos/{}/actions/runs/{run_id}/jobs", self.repo),
        ])?;
        Ok(response
            .jobs
            .into_iter()
            .find(|job| job_conclusion_is_failure(job.conclusion.as_deref()))
            .map(|job| job.name))
    }

    fn branch_tip(&self, branch: &str) -> Result<Option<String>, CiWatchError> {
        // An unreadable tip (a deleted branch, a transient API error) keeps
        // the pre-cas-c188 behaviour rather than silencing a real red run.
        Ok(self
            .gh_json::<BranchResponse>(&[
                "api".to_string(),
                "-X".to_string(),
                "GET".to_string(),
                format!("repos/{}/branches/{branch}", self.repo),
            ])
            .ok()
            .map(|response| response.commit.sha)
            .filter(|sha| !sha.is_empty()))
    }

    fn failed_log(&self, run_id: u64) -> Result<Option<String>, CiWatchError> {
        let log = self.gh_text(&[
            "run".to_string(),
            "view".to_string(),
            run_id.to_string(),
            "--repo".to_string(),
            self.repo.clone(),
            "--log-failed".to_string(),
        ])?;
        Ok((!log.is_empty()).then_some(log))
    }

    fn runs_for_sha(&self, sha: &str) -> Result<Vec<CiRun>, CiWatchError> {
        let response: RunsResponse = self.gh_json(&[
            "api".to_string(),
            "-X".to_string(),
            "GET".to_string(),
            format!("repos/{}/actions/runs", self.repo),
            "-f".to_string(),
            format!("head_sha={sha}"),
            "-f".to_string(),
            "per_page=100".to_string(),
        ])?;
        Ok(response.workflow_runs)
    }

    fn check_runs_for_sha(
        &self,
        sha: &str,
    ) -> Result<Option<Vec<serde_json::Value>>, CiWatchError> {
        let response: serde_json::Value = self.gh_json(&[
            "api".to_string(),
            "-X".to_string(),
            "GET".to_string(),
            format!("repos/{}/commits/{sha}/check-runs", self.repo),
            "-f".to_string(),
            "per_page=100".to_string(),
        ])?;
        response
            .get("check_runs")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .map(Some)
            .ok_or_else(|| {
                CiWatchError::Malformed("check-runs response omitted check_runs".to_string())
            })
    }

    fn failed_job_detail(&self, run_id: u64) -> Result<Option<FailedJob>, CiWatchError> {
        let response: JobsResponse = self.gh_json(&[
            "api".to_string(),
            "-X".to_string(),
            "GET".to_string(),
            format!("repos/{}/actions/runs/{run_id}/jobs", self.repo),
        ])?;
        Ok(response
            .jobs
            .into_iter()
            .find(|job| job_conclusion_is_failure(job.conclusion.as_deref()))
            .map(|job| FailedJob {
                step: job
                    .steps
                    .into_iter()
                    .find(|step| job_conclusion_is_failure(step.conclusion.as_deref()))
                    .map(|step| step.name),
                name: job.name,
            }))
    }

    fn merge_queue_pull_requests(&self) -> Result<Vec<MergeQueuePullRequest>, CiWatchError> {
        let repo = crate::cli::integrate::github::RepoRef::from_owner_slash_repo(&self.repo)
            .ok_or_else(|| {
                CiWatchError::Unavailable(
                    "invalid GitHub repository for merge-queue watcher".to_string(),
                )
            })?;
        let query = "query($owner: String!, $name: String!) { repository(owner: $owner, name: $name) { pullRequests(first: 100, states: OPEN) { nodes { number url headRefName headRefOid isInMergeQueue updatedAt autoMergeRequest { enabledAt } statusCheckRollup { state } } } } }";
        let response: MergeQueueGraphqlResponse = self.gh_json(&[
            "api".to_string(),
            "graphql".to_string(),
            "-f".to_string(),
            format!("query={query}"),
            "-F".to_string(),
            format!("owner={}", repo.owner),
            "-F".to_string(),
            format!("name={}", repo.repo),
        ])?;
        Ok(response.data.repository.pull_requests.nodes)
    }

    fn delivery_pull_requests(
        &self,
        branch: &str,
    ) -> Result<Vec<DeliveryPullRequest>, CiWatchError> {
        self.gh_json(&[
            "pr".to_string(),
            "list".to_string(),
            "--repo".to_string(),
            self.repo.clone(),
            "--state".to_string(),
            "all".to_string(),
            "--head".to_string(),
            branch.to_string(),
            "--limit".to_string(),
            "20".to_string(),
            "--json".to_string(),
            "number,headRefName,headRefOid,state,mergedAt,mergeCommit".to_string(),
        ])
    }
}

fn merge_group_pr_number(branch: &str) -> Option<u64> {
    let (_, suffix) = branch.rsplit_once("/pr-")?;
    suffix.split_once('-')?.0.parse().ok()
}

/// Match each parked delivery to its PR and retain the merge commit when the
/// PR has landed. The task's recorded PR number wins after discovery; before
/// discovery, the branch tip anchor prevents an older PR on a reused factory
/// branch from being attributed to the current task.
pub(crate) fn collect_delivery_pr_observations(
    transport: &dyn CiTransport,
    deliveries: &[AwaitingMergeDelivery],
) -> Result<Vec<DeliveryPrObservation>, CiWatchError> {
    let mut observations = Vec::new();
    for delivery in deliveries {
        let pulls = transport.delivery_pull_requests(&delivery.branch)?;
        let candidate = pulls
            .into_iter()
            .filter(|pull| pull.head_branch == delivery.branch)
            .filter(|pull| {
                if let Some(number) = delivery.pr_number {
                    number == pull.number
                } else {
                    delivery
                        .branch_tip
                        .as_deref()
                        .is_none_or(|tip| tip == pull.head_sha)
                }
            })
            .max_by_key(|pull| pull.number);
        let Some(pull) = candidate else {
            continue;
        };
        let merged = pull.merged_at.is_some() || pull.state.eq_ignore_ascii_case("MERGED");
        observations.push(DeliveryPrObservation {
            task_id: delivery.task_id.clone(),
            worker: delivery.worker.clone(),
            branch: delivery.branch.clone(),
            pr_number: pull.number,
            head_sha: pull.head_sha,
            merge_commit: merged
                .then(|| pull.merge_commit)
                .flatten()
                .map(|commit| commit.oid),
        });
    }
    Ok(observations)
}

/// The queue snapshot is intentionally small: one GraphQL open-PR listing and
/// the existing completed-runs listing per minute. A delivery is ejected when
/// its open PR leaves a previously-observed queue, or when a failed
/// `merge_group` run proves that ejection after a daemon restart.
#[cfg(test)]
pub(crate) fn collect_merge_queue_ejections(
    transport: &dyn CiTransport,
    deliveries: &[AwaitingMergeDelivery],
    previously_queued: &BTreeSet<u64>,
) -> Result<MergeQueuePoll, CiWatchError> {
    collect_merge_queue_ejections_with_arm_state(
        transport,
        deliveries,
        previously_queued,
        &BTreeSet::new(),
    )
}

#[cfg(test)]
pub(crate) fn collect_merge_queue_ejections_with_arm_state(
    transport: &dyn CiTransport,
    deliveries: &[AwaitingMergeDelivery],
    previously_queued: &BTreeSet<u64>,
    previously_armed: &BTreeSet<u64>,
) -> Result<MergeQueuePoll, CiWatchError> {
    collect_tracked_merge_queue_ejections(
        transport,
        deliveries,
        &[],
        previously_queued,
        previously_armed,
    )
}

/// cas-e369: as above, also tracking supervisor-owned epic PRs. An epic PR
/// that leaves the queue unmerged wakes the supervisor exactly like a parked
/// delivery does; it has no worker to instruct.
pub(crate) fn collect_tracked_merge_queue_ejections(
    transport: &dyn CiTransport,
    deliveries: &[AwaitingMergeDelivery],
    supervisor_prs: &[SupervisorPr],
    previously_queued: &BTreeSet<u64>,
    previously_armed: &BTreeSet<u64>,
) -> Result<MergeQueuePoll, CiWatchError> {
    let pulls = transport.merge_queue_pull_requests()?;
    let runs = transport.completed_runs()?;
    let mut failed_runs = BTreeMap::<u64, &CiRun>::new();
    for run in &runs {
        if run.event == "merge_group"
            && run.conclusion.as_deref() == Some("failure")
            && let Some(pr) = merge_group_pr_number(&run.head_branch)
        {
            failed_runs
                .entry(pr)
                .and_modify(|current| {
                    if run.id > current.id {
                        *current = run;
                    }
                })
                .or_insert(run);
        }
    }

    let mut poll = MergeQueuePoll::default();
    poll.observed_prs = pulls.iter().map(|pr| crate::cloud::peers_detail::PeerPr {
        url: pr.url.clone(),
        head_sha: pr.head_sha.clone(),
        branch: pr.head_branch.clone(),
        number: pr.number,
        state: "OPEN".into(),
        in_merge_queue: pr.is_in_merge_queue,
    }).collect();
    let tracked = deliveries
        .iter()
        .map(|delivery| (&delivery.task_id, delivery.worker.as_str(), &delivery.branch))
        .chain(
            supervisor_prs
                .iter()
                .map(|epic| (&epic.task_id, "", &epic.branch)),
        );
    for (task_id, worker, branch) in tracked {
        let Some(pr) = pulls.iter().find(|pr| &pr.head_branch == branch) else {
            // A normally merged PR leaves the open-PR listing; it is never an ejection.
            continue;
        };
        if pr.auto_merge_armed() {
            poll.auto_merge_prs.insert(pr.number);
        }
        if pr.is_in_merge_queue {
            poll.queued_prs.insert(pr.number);
            continue;
        }
        let failed_run_id = failed_runs.get(&pr.number).map(|run| run.id);
        let arm_vanished = pr.head_branch.starts_with("factory/")
            && previously_armed.contains(&pr.number)
            && !pr.auto_merge_armed()
            && pr.checks_green();
        if !previously_queued.contains(&pr.number) && failed_run_id.is_none() && !arm_vanished {
            continue;
        }
        // A prior observed queue membership starts a fresh episode even when
        // GitHub still lists the previous red run. This re-arms an admin
        // dequeue/auto-merge disarm after a requeue; a restart without that
        // in-memory observation instead keys directly to the failed run.
        let occurrence = if previously_queued.contains(&pr.number) {
            format!(
                "queue-exit:{}:{}",
                pr.updated_at,
                failed_run_id
                    .map(|run_id| format!("merge-group-run:{run_id}"))
                    .unwrap_or_else(|| "no-failed-run".to_string())
            )
        } else if arm_vanished {
            format!("auto-merge-disarmed:{}:{}", pr.head_sha, pr.updated_at)
        } else {
            failed_run_id
                .map(|run_id| format!("merge-group-run:{run_id}"))
                .expect("failed run required without an observed queue membership")
        };
        poll.ejections.push(MergeQueueEjection {
            task_id: task_id.clone(),
            worker: worker.to_string(),
            pr_number: pr.number,
            failed_run_id,
            occurrence,
        });
    }
    poll.pr_lane_failures = collect_pr_lane_failures(transport, deliveries, &pulls, &runs)?;
    Ok(poll)
}

/// Find the latest failed required PR-lane run for each current delivery PR.
/// Factory PRs receive this check from their branch push event, so `event` is
/// deliberately not used as the discriminator; the required job name is the
/// stable check identity. Matching the current PR head prevents a stale red
/// run from waking a worker after a corrective push.
pub(crate) fn collect_pr_lane_failures(
    transport: &dyn CiTransport,
    deliveries: &[AwaitingMergeDelivery],
    pulls: &[MergeQueuePullRequest],
    runs: &[CiRun],
) -> Result<Vec<PrLaneFailure>, CiWatchError> {
    let mut latest = BTreeMap::<(u64, String), (&AwaitingMergeDelivery, &CiRun)>::new();
    for delivery in deliveries {
        let Some(pr) = pulls.iter().find(|pr| pr.head_branch == delivery.branch) else {
            continue;
        };
        if !pr.head_branch.starts_with("factory/") || pr.head_sha.is_empty() {
            continue;
        }
        for run in runs {
            if run.status != "completed"
                || run.conclusion.as_deref() != Some("failure")
                || run.head_branch != delivery.branch
                || run.head_sha != pr.head_sha
            {
                continue;
            }
            let key = (pr.number, run.head_sha.clone());
            let replace = latest.get(&key).is_none_or(|(_, current)| run.id > current.id);
            if replace {
                latest.insert(key, (delivery, run));
            }
        }
    }

    let mut failures = Vec::new();
    for ((pr_number, head_sha), (delivery, run)) in latest {
        let Some(check_name) = transport.failing_job(run.id)? else {
            continue;
        };
        if check_name != REQUIRED_PR_LANE_CHECK {
            continue;
        }
        failures.push(PrLaneFailure {
            task_id: delivery.task_id.clone(),
            worker: delivery.worker.clone(),
            pr_number,
            head_sha,
            run_id: run.id,
            run_url: run.html_url.clone(),
            check_name,
        });
    }
    Ok(failures)
}

fn is_branch_ci_verdict(run: &CiRun) -> bool {
    run.name == "CI"
        && matches!(run.event.as_str(), "push" | "merge_group")
        && run.status == "completed"
}

/// Keep only the latest completed CI push/merge-group failure per watched branch.
///
/// GitHub Action run IDs increase monotonically.  Selecting the highest run ID
/// per branch makes the current completed conclusion authoritative: a newer
/// green run resolves an older failure, while a newer red run replaces it.  We
/// deliberately do this before looking up failed jobs/logs so a historical
/// backlog is both silent and cheap to scan.
pub(crate) fn collect_failures(
    transport: &dyn CiTransport,
    watched_branches: &BTreeSet<String>,
) -> Result<Vec<CiFailure>, CiWatchError> {
    let runs = transport.completed_runs()?;
    let mut latest_by_branch = BTreeMap::<String, &CiRun>::new();

    for run in &runs {
        if !is_branch_ci_verdict(run) || !watched_branches.contains(&run.head_branch) {
            continue;
        }
        let replace = latest_by_branch
            .get(&run.head_branch)
            .is_none_or(|current| run.id > current.id);
        if replace {
            latest_by_branch.insert(run.head_branch.clone(), run);
        }
    }

    let mut failures = Vec::new();
    for (_, run) in latest_by_branch {
        if run.conclusion.as_deref() != Some("failure") {
            continue;
        }
        // cas-c188: a run for a commit the branch has moved past is history,
        // not the branch's state. Main can go weeks without a completed run of
        // its own (merge-queue receipts are reused), which made a seven-week-old
        // run the "latest" and relayed it as current.
        if let Some(tip) = transport.branch_tip(&run.head_branch)?
            && tip != run.head_sha
        {
            continue;
        }
        // A run whose jobs were cancelled, not failed, is not red; with no
        // failed job there is nothing to name or fix.
        let Some(failing_job) = transport.failing_job(run.id)? else {
            continue;
        };

        let mut suppressed_red_runs: Vec<_> = runs
            .iter()
            .filter(|older| {
                older.head_branch == run.head_branch
                    && older.id < run.id
                    && is_branch_ci_verdict(older)
                    && older.conclusion.as_deref() == Some("failure")
            })
            .map(|older| SuppressedCiRun {
                head_sha: older.head_sha.clone(),
                run_id: older.id,
            })
            .collect();
        suppressed_red_runs.sort_by_key(|older| older.run_id);

        let failing_test = transport
            .failed_log(run.id)?
            .as_deref()
            .and_then(first_failing_test);
        failures.push(CiFailure {
            branch: run.head_branch.clone(),
            head_sha: run.head_sha.clone(),
            run_id: run.id,
            run_url: run.html_url.clone(),
            failing_job,
            failing_test,
            suppressed_red_runs,
            merged_into: None,
        });
    }
    Ok(failures)
}

/// cas-6c13: name, for each red factory-lane run, a non-factory branch that
/// already contains its commit (`git branch -a --contains`). Read-only Git.
pub(crate) fn mark_merged_failures(project: &Path, mut failures: Vec<CiFailure>) -> Vec<CiFailure> {
    for failure in failures
        .iter_mut()
        .filter(|f| f.branch.starts_with("factory/"))
    {
        let args = [
            "branch",
            "-a",
            "--format=%(refname:short)",
            "--contains",
            &failure.head_sha,
        ];
        let Ok(output) = run_external_git_command(project, &args.map(str::to_string)) else {
            continue;
        };
        failure.merged_into = output
            .stdout
            .lines()
            .map(|name| name.trim().trim_start_matches("origin/"))
            .find(|name| !name.is_empty() && !name.starts_with("factory/") && *name != "HEAD")
            .map(str::to_string);
    }
    failures
}

/// Extract the common Rust/nextest failure shape without treating log parsing
/// as authoritative.  Missing or unfamiliar logs simply omit the test name.
pub(crate) fn first_failing_test(log: &str) -> Option<String> {
    log.lines().find_map(|line| {
        let line = line.trim();
        let test = line.strip_prefix("test ")?;
        let (name, outcome) = test.rsplit_once(" ... ")?;
        (outcome.contains("FAILED")).then(|| name.trim().to_string())
    })
}

pub(crate) fn relay_body(failure: &CiFailure) -> String {
    let test = failure
        .failing_test
        .as_deref()
        .map(|name| format!("\nFirst failing test: {name}"))
        .unwrap_or_default();
    let suppressed = failure
        .suppressed_red_runs
        .first()
        .zip(failure.suppressed_red_runs.last())
        .map(|(oldest, newest)| {
            format!(
                "\nSuppressed {} historical red run(s) on this branch ({} through {}).",
                failure.suppressed_red_runs.len(),
                oldest.head_sha,
                newest.head_sha,
            )
        })
        .unwrap_or_default();
    let next = match &failure.merged_into {
        Some(integration) => format!(
            "BLOCKER: this commit is already merged into {integration}. Fix or revert it there before {integration} is queued."
        ),
        None => "This red run is the branch's latest completed conclusion; inspect or fix it before merging further lanes.".to_string(),
    };
    format!(
        "<ci-red-run branch=\"{}\" head_sha=\"{}\" run_id=\"{}\">\nCI failed on branch {} at {}.\nRun: {}\nFailing job: {}{}{}\n{next}\n</ci-red-run>",
        failure.branch,
        failure.head_sha,
        failure.run_id,
        failure.branch,
        failure.head_sha,
        failure.run_url,
        failure.failing_job,
        test,
        suppressed,
    )
}

/// Durable prompt handoff.  The stable key makes repeated polls and daemon
/// restarts silent for the same `(branch, head_sha)` red run.
pub(crate) fn emit_failure(
    queue: &dyn PromptQueueStore,
    factory_session: &str,
    failure: &CiFailure,
) -> Result<bool, String> {
    let key = failure.dedupe_key();
    let source = format!("lifecycle-wake:{key}");
    let (summary, priority) = match &failure.merged_into {
        Some(integration) => (
            format!(
                "BLOCKER: merged lane {} is red in {integration}",
                failure.branch
            ),
            NotificationPriority::Critical,
        ),
        None => (
            format!("CI FAILED: {} ({})", failure.branch, failure.head_sha),
            NotificationPriority::High,
        ),
    };
    match queue
        .enqueue_idempotent(
            &source,
            "supervisor",
            &relay_body(failure),
            Some(factory_session),
            Some(&summary),
            Some(priority),
            &key,
            Some(&cas_store::QueueOrigin::Daemon),
        )
        .map_err(|e| format!("could not enqueue CI red-run relay: {e}"))?
    {
        cas_store::EnqueueIdempotentResult::Created(_) => Ok(true),
        cas_store::EnqueueIdempotentResult::AlreadyExists(_) => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::SqlitePromptQueueStore;
    use std::cell::Cell;

    struct FakeTransport {
        runs: Vec<CiRun>,
        pulls: Vec<MergeQueuePullRequest>,
        delivery_pulls: Vec<DeliveryPullRequest>,
        job: String,
        log: Option<String>,
        calls: Cell<u8>,
    }

    impl CiTransport for FakeTransport {
        fn completed_runs(&self) -> Result<Vec<CiRun>, CiWatchError> {
            Ok(self.runs.clone())
        }
        fn failing_job(&self, _: u64) -> Result<Option<String>, CiWatchError> {
            self.calls.set(self.calls.get() + 1);
            Ok(Some(self.job.clone()))
        }
        fn failed_log(&self, _: u64) -> Result<Option<String>, CiWatchError> {
            Ok(self.log.clone())
        }
        fn merge_queue_pull_requests(&self) -> Result<Vec<MergeQueuePullRequest>, CiWatchError> {
            Ok(self.pulls.clone())
        }
        fn delivery_pull_requests(
            &self,
            _: &str,
        ) -> Result<Vec<DeliveryPullRequest>, CiWatchError> {
            Ok(self.delivery_pulls.clone())
        }
        fn runs_for_sha(&self, sha: &str) -> Result<Vec<CiRun>, CiWatchError> {
            Ok(self.runs.iter().filter(|run| run.head_sha == sha).cloned().collect())
        }
    }

    #[test]
    fn cas_cd42_reuses_actual_poll_records_without_inventing_missing_pr_urls() {
        let mut unknown_url = pull(42, "epic/release", true, "2026-10-16T12:00:00Z");
        unknown_url.url = None;
        let transport = FakeTransport {
            runs: Vec::new(), pulls: vec![unknown_url], delivery_pulls: Vec::new(),
            job: "unused".into(), log: None, calls: Cell::new(0),
        };
        let poll = collect_tracked_merge_queue_ejections(
            &transport, &[], &[], &BTreeSet::new(), &BTreeSet::new(),
        ).unwrap();
        assert_eq!(poll.observed_prs.len(), 1);
        assert_eq!(poll.observed_prs[0].head_sha, "deadbeef");
        assert!(poll.observed_prs[0].in_merge_queue);
        assert!(poll.observed_prs[0].url.is_none());
        assert_eq!(transport.calls.get(), 0, "inventory must not add detail lookups");
    }

    /// cas-0f05: GitHub's runs for fba7a3365 (push, merge_group and
    /// scheduled), recorded 2026-10-14 and trimmed to the fields read.
    const FIXTURE_SHA: &str = "fba7a3365eb0cb6403be9d708e8f03151fc42054";

    /// The recorded runs, with the push `CI` run set to `status`/`conclusion`.
    fn recorded_runs(status: &str, conclusion: Option<&str>) -> FakeTransport {
        let page: RunsResponse =
            serde_json::from_str(include_str!("fixtures/ci-runs-fba7a336.json")).unwrap();
        let mut runs = page.workflow_runs;
        for run in runs.iter_mut().filter(|run| run.name == "CI" && run.event == "push") {
            run.status = status.to_string();
            run.conclusion = conclusion.map(str::to_string);
        }
        FakeTransport {
            runs,
            pulls: Vec::new(),
            delivery_pulls: Vec::new(),
            job: String::new(),
            log: None,
            calls: Cell::new(0),
        }
    }

    fn ci_completed(workflow: Option<&str>) -> CiCompleted {
        parse_ci_completed(&serde_json::json!({"sha": FIXTURE_SHA, "workflow": workflow})).unwrap()
    }

    #[test]
    fn ci_completed_is_pending_until_every_run_completes() {
        let running = recorded_runs("in_progress", None);
        assert!(ci_completed_runs(&running, &ci_completed(None)).unwrap().is_none());
        assert!(ci_completed_runs(&running, &ci_completed(Some("CI"))).unwrap().is_none());
        // No runs yet for a just-pushed commit: pending, not vacuously done.
        let other = CiCompleted { sha: "0".repeat(40), workflow: None };
        assert!(ci_completed_runs(&running, &other).unwrap().is_none());

        let done = recorded_runs("completed", Some("success"));
        let runs = ci_completed_runs(&done, &ci_completed(None))
            .unwrap()
            .expect("all recorded runs completed");
        assert!(runs.iter().all(|run| run.event != "schedule"));
        assert!(runs.iter().any(|run| run.event == "merge_group"));
    }

    /// cas-e845 / cas-ae6a: `ci_completed` waits only while a check-run of the
    /// selected runs is still running. A check cancelled with no rerun at the
    /// SHA, or any failed check, settles the wake. Driven through the real
    /// gh transport (runs + check-runs) with check_suite ids in the fixtures.
    #[test]
    fn ci_completed_settles_on_check_runs_through_gh_cas_ae6a() {
        let sha = "39612776866aaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let check = |id: u64, name: &str, suite: u64, status: &str, conclusion: Option<&str>| {
            serde_json::json!({"id": id, "name": name, "status": status,
                "conclusion": conclusion, "check_suite": {"id": suite}})
        };
        // A queued job of an unselected suite never holds the wake.
        let other_suite = check(90, "watchdog", 999, "queued", None);
        let rows: [(&str, &str, Vec<serde_json::Value>, Option<&str>); 4] = [
            (
                "a completed run with an in_progress job still waits (cas-e845)",
                "success",
                vec![
                    check(1, "Classify CI diff", 7, "completed", Some("success")),
                    check(2, "Scoped Validation (fast)", 7, "in_progress", None),
                ],
                None,
            ),
            (
                "a run cancelled with no rerun wakes as cancelled",
                "cancelled",
                vec![
                    check(1, "Classify CI diff", 7, "completed", Some("cancelled")),
                    check(2, "Scoped Validation (fast)", 7, "completed", Some("cancelled")),
                ],
                Some("cancelled"),
            ),
            (
                "one failed and one cancelled check wakes as failure",
                "failure",
                vec![
                    check(1, "Scoped Validation (fast)", 7, "completed", Some("failure")),
                    check(2, "Docs Lint", 7, "completed", Some("cancelled")),
                ],
                Some("failure"),
            ),
            (
                "a cancelled check whose rerun is running still waits",
                "success",
                vec![
                    check(1, "Scoped Validation (fast)", 7, "completed", Some("cancelled")),
                    check(5, "Scoped Validation (fast)", 7, "in_progress", None),
                ],
                None,
            ),
        ];
        for (name, run_conclusion, mut checks, expected) in rows {
            checks.push(other_suite.clone());
            let temp = tempfile::TempDir::new().unwrap();
            let runs = serde_json::json!({"workflow_runs": [{"id": 39612776866_u64, "name": "CI",
                "head_branch": "factory/x", "head_sha": sha,
                "html_url": "https://github.test/runs/39612776866", "status": "completed",
                "conclusion": run_conclusion, "event": "push", "check_suite_id": 7}]});
            std::fs::write(temp.path().join("runs.json"), runs.to_string()).unwrap();
            let checks = serde_json::json!({"check_runs": checks});
            std::fs::write(temp.path().join("checks.json"), checks.to_string()).unwrap();
            let gh = temp.path().join("gh");
            crate::test_paths::warm_stub(
                &gh,
                &format!(
                    "#!/bin/sh\ncase \"$4\" in\nrepos/org/repo/actions/runs) cat '{dir}/runs.json' ;;\nrepos/org/repo/commits/{sha}/check-runs) cat '{dir}/checks.json' ;;\n*) exit 97 ;;\nesac\n",
                    dir = temp.path().display(),
                ),
            );
            let transport = GhCiTransport {
                repo: "org/repo".to_string(),
                cwd: temp.path().to_path_buf(),
                gh_binary: gh,
            };
            let condition = CiCompleted { sha: sha.to_string(), workflow: Some("CI".to_string()) };
            let runs = ci_completed_runs(&transport, &condition).unwrap();
            assert_eq!(runs.as_deref().map(ci_outcome), expected, "{name}");
        }
    }

    #[test]
    fn ci_completed_carries_a_failure_conclusion() {
        let failed = recorded_runs("completed", Some("failure"));
        let runs = ci_completed_runs(&failed, &ci_completed(Some("CI"))).unwrap().unwrap();
        assert!(runs.iter().any(|run| run.conclusion.as_deref() == Some("failure")));
    }

    #[test]
    fn ci_completed_filter_needs_the_full_sha() {
        assert!(parse_ci_completed(&serde_json::json!({"sha": "fba7a3365"})).is_err());
        assert!(parse_ci_completed(&serde_json::json!({"workflow": "CI"})).is_err());
    }

    /// One watcher interval plus one daemon refresh tick plus the bounded
    /// origin, run-list and (cas-e845) check-runs `gh` calls stay inside the
    /// 60 s contract.
    #[test]
    fn ci_completed_wakes_within_a_minute() {
        let tick = super::super::lifecycle::DAEMON_REFRESH_INTERVAL;
        let worst = CI_WAKE_INTERVAL + tick + 3 * GH_CALL_TIMEOUT;
        assert!(worst <= Duration::from_secs(60), "{worst:?}");
    }

    fn run(branch: &str, conclusion: Option<&str>) -> CiRun {
        run_with(branch, "deadbeef", 42, conclusion)
    }

    fn run_with(branch: &str, head_sha: &str, id: u64, conclusion: Option<&str>) -> CiRun {
        CiRun {
            id,
            name: "CI".to_string(),
            head_branch: branch.to_string(),
            head_sha: head_sha.to_string(),
            html_url: format!("https://github.test/org/repo/actions/runs/{id}"),
            status: "completed".to_string(),
            conclusion: conclusion.map(str::to_string),
            event: "push".to_string(),
            check_suite_id: None,
        }
    }

    fn pull(number: u64, branch: &str, queued: bool, updated_at: &str) -> MergeQueuePullRequest {
        MergeQueuePullRequest {
            url: Some(format!("https://github.com/acme/widgets/pull/{number}")),
            number,
            head_branch: branch.to_string(),
            head_sha: "deadbeef".to_string(),
            is_in_merge_queue: queued,
            updated_at: updated_at.to_string(),
            auto_merge_request: None,
            status_check_rollup: None,
        }
    }

    fn armed_pull(
        number: u64,
        branch: &str,
        head_sha: &str,
        queued: bool,
        updated_at: &str,
    ) -> MergeQueuePullRequest {
        MergeQueuePullRequest {
            url: Some(format!("https://github.com/acme/widgets/pull/{number}")),
            number,
            head_branch: branch.to_string(),
            head_sha: head_sha.to_string(),
            is_in_merge_queue: queued,
            updated_at: updated_at.to_string(),
            auto_merge_request: Some(AutoMergeRequest { enabled_at: None }),
            status_check_rollup: Some(StatusCheckRollup {
                state: "SUCCESS".to_string(),
            }),
        }
    }

    fn delivery_pull(
        number: u64,
        branch: &str,
        head_sha: &str,
        merged_at: Option<&str>,
        merge_commit: Option<&str>,
    ) -> DeliveryPullRequest {
        DeliveryPullRequest {
            number,
            head_branch: branch.to_string(),
            head_sha: head_sha.to_string(),
            state: if merged_at.is_some() {
                "MERGED".to_string()
            } else {
                "OPEN".to_string()
            },
            merged_at: merged_at.map(str::to_string),
            merge_commit: merge_commit.map(|oid| DeliveryMergeCommit {
                oid: oid.to_string(),
            }),
        }
    }

    #[test]
    fn merged_delivery_pr_observation_carries_pr_and_merge_commit() {
        let delivery = AwaitingMergeDelivery {
            task_id: "cas-7ea6".to_string(),
            worker: "calm-octopus-51".to_string(),
            branch: "factory/calm-octopus-51".to_string(),
            branch_tip: Some("worker-tip".to_string()),
            pr_number: None,
        };
        let transport = FakeTransport {
            runs: Vec::new(),
            pulls: Vec::new(),
            delivery_pulls: vec![delivery_pull(
                932,
                &delivery.branch,
                "worker-tip",
                Some("2026-09-21T12:00:00Z"),
                Some("merge-tip"),
            )],
            job: String::new(),
            log: None,
            calls: Cell::new(0),
        };

        let observations =
            collect_delivery_pr_observations(&transport, std::slice::from_ref(&delivery)).unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].pr_number, 932);
        assert_eq!(observations[0].merge_commit.as_deref(), Some("merge-tip"));
    }

    #[cfg(unix)]
    #[test]
    fn ci_watch_moved_origin_ignores_upstream_cas_28c8() {
        let mut env = crate::test_support::TestEnvGuard::new();
        let temp = tempfile::TempDir::new().unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["remote", "add", "origin", "git@github.com:pippenz/cas.git"],
            vec![
                "remote",
                "add",
                "upstream",
                "https://github.com/codingagentsystem/cas.git",
            ],
        ] {
            assert!(
                Command::new("git")
                    .current_dir(temp.path())
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let gh = temp.path().join("gh");
        crate::test_paths::warm_stub(
            &gh,
            "#!/bin/sh\ncase \"$1 $2 $3\" in\n'repo view pippenz/cas') printf '%s' '{\"nameWithOwner\":\"Richards-LLC/cassy\"}' ;;\n'api -X GET') [ \"$4\" = 'repos/Richards-LLC/cassy/actions/runs' ] || exit 97; printf '%s' '{\"workflow_runs\":[]}' ;;\n*) exit 98 ;;\nesac\n",
        );
        env.set(crate::github_issue_attach::GH_BIN_ENV, &gh);
        let transport = GhCiTransport::from_project(temp.path()).unwrap();
        assert_eq!(transport.repo, "Richards-LLC/cassy");
        assert!(transport.completed_runs().unwrap().is_empty());
    }

    #[test]
    fn gh_delivery_pull_request_query_parses_stubbed_gh_output() {
        let temp = tempfile::TempDir::new().unwrap();
        let gh = temp.path().join("gh");
        crate::test_paths::warm_stub(
            &gh,
            "#!/bin/sh\nprintf '%s' '[{\"number\":932,\"headRefName\":\"factory/calm-octopus-51\",\"headRefOid\":\"worker-tip\",\"state\":\"MERGED\",\"mergedAt\":\"2026-09-21T12:00:00Z\",\"mergeCommit\":{\"oid\":\"merge-tip\"}}]'\n",
        );
        let transport = GhCiTransport {
            repo: "org/repo".to_string(),
            cwd: temp.path().to_path_buf(),
            gh_binary: gh,
        };

        let pulls = transport
            .delivery_pull_requests("factory/calm-octopus-51")
            .unwrap();
        assert_eq!(pulls.len(), 1);
        assert_eq!(pulls[0].number, 932);
        assert_eq!(pulls[0].merge_commit.as_ref().unwrap().oid, "merge-tip");
    }

    #[test]
    fn delivery_pr_observation_ignores_stale_head_from_a_reused_branch() {
        let delivery = AwaitingMergeDelivery {
            task_id: "cas-7ea6".to_string(),
            worker: "calm-octopus-51".to_string(),
            branch: "factory/calm-octopus-51".to_string(),
            branch_tip: Some("current-tip".to_string()),
            pr_number: None,
        };
        let transport = FakeTransport {
            runs: Vec::new(),
            pulls: Vec::new(),
            delivery_pulls: vec![delivery_pull(
                931,
                &delivery.branch,
                "old-tip",
                Some("2026-09-21T11:00:00Z"),
                Some("old-merge"),
            )],
            job: String::new(),
            log: None,
            calls: Cell::new(0),
        };
        assert!(
            collect_delivery_pr_observations(&transport, std::slice::from_ref(&delivery))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn failed_completed_live_lane_emits_a_relay_with_job_and_test() {
        let transport = FakeTransport {
            runs: vec![run("factory/bright-otter", Some("failure"))],
            pulls: Vec::new(),
            delivery_pulls: Vec::new(),
            job: "Fast Validation".to_string(),
            log: Some("test contract_conflict_regression ... FAILED".to_string()),
            calls: Cell::new(0),
        };
        let watched = BTreeSet::from(["main".to_string(), "factory/bright-otter".to_string()]);
        let failures = collect_failures(&transport, &watched).unwrap();
        assert_eq!(failures.len(), 1);
        let body = relay_body(&failures[0]);
        assert!(body.contains("branch=\"factory/bright-otter\""));
        assert!(body.contains("Failing job: Fast Validation"));
        assert!(body.contains("First failing test: contract_conflict_regression"));
        assert!(crate::prompt_revalidation::parse_ci_red_run_envelope(&body));
        assert_eq!(transport.calls.get(), 1);
    }

    /// cas-c188: a transport that knows the branch tip and the run's failed
    /// job (or that the run only had cancelled jobs).
    struct TipTransport {
        runs: Vec<CiRun>,
        tip: Option<&'static str>,
        failed_job: Option<&'static str>,
        job_lookups: Cell<u8>,
    }

    impl CiTransport for TipTransport {
        fn completed_runs(&self) -> Result<Vec<CiRun>, CiWatchError> {
            Ok(self.runs.clone())
        }
        fn failing_job(&self, _: u64) -> Result<Option<String>, CiWatchError> {
            self.job_lookups.set(self.job_lookups.get() + 1);
            Ok(self.failed_job.map(str::to_string))
        }
        fn branch_tip(&self, _: &str) -> Result<Option<String>, CiWatchError> {
            Ok(self.tip.map(str::to_string))
        }
        fn failed_log(&self, _: u64) -> Result<Option<String>, CiWatchError> {
            Ok(None)
        }
        fn merge_queue_pull_requests(&self) -> Result<Vec<MergeQueuePullRequest>, CiWatchError> {
            Ok(Vec::new())
        }
        fn delivery_pull_requests(
            &self,
            _: &str,
        ) -> Result<Vec<DeliveryPullRequest>, CiWatchError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn cas_233e_watchdog_failure_is_not_main_ci_and_enqueues_no_relay() {
        let mut watchdog = run("main", Some("failure"));
        watchdog.name = "Stale queued-run watchdog".to_string();
        watchdog.event = "schedule".to_string();
        let transport = TipTransport {
            runs: vec![watchdog], tip: Some("deadbeef"),
            failed_job: Some("Cancel stale non-merge-group queued runs"), job_lookups: Cell::new(0),
        };
        let failures = collect_failures(&transport, &BTreeSet::from(["main".to_string()])).unwrap();
        let temp = tempfile::TempDir::new().unwrap();
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        for failure in &failures { emit_failure(&queue, "factory-session", failure).unwrap(); }
        assert!(queue.peek_all(10).unwrap().is_empty(), "housekeeping must never claim CI failed on main");
        assert_eq!(transport.job_lookups.get(), 0);
    }

    #[test]
    fn cas_233e_housekeeping_cannot_hide_ci_failure_or_pollute_suppressed_history() {
        let mut old_watchdog = run_with("main", "deadbeef", 40, Some("failure"));
        old_watchdog.name = "Stale queued-run watchdog".to_string();
        old_watchdog.event = "schedule".to_string();
        let mut green_watchdog = old_watchdog.clone();
        green_watchdog.id = 42;
        green_watchdog.conclusion = Some("success".to_string());
        let transport = TipTransport {
            runs: vec![old_watchdog, run_with("main", "deadbeef", 41, Some("failure")), green_watchdog],
            tip: Some("deadbeef"), failed_job: Some("Fast Validation"), job_lookups: Cell::new(0),
        };
        let failures = collect_failures(&transport, &BTreeSet::from(["main".to_string()])).unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].run_id, 41);
        assert!(failures[0].suppressed_red_runs.is_empty());
        assert_eq!(transport.job_lookups.get(), 1);
    }

    #[test]
    fn cas_233e_only_ci_push_and_merge_group_define_branch_verdict() {
        for (name, event, eligible) in [
            ("CI", "push", true), ("CI", "merge_group", true),
            ("CI", "schedule", false), ("CI", "workflow_dispatch", false),
            ("CI", "pull_request", false), ("Stale queued-run watchdog", "push", false),
            ("Other workflow", "merge_group", false),
        ] {
            let mut ci = run("main", Some("failure"));
            ci.name = name.to_string();
            ci.event = event.to_string();
            let transport = TipTransport {
                runs: vec![ci], tip: Some("deadbeef"),
                failed_job: Some("Fast Validation"), job_lookups: Cell::new(0),
            };
            let failures = collect_failures(&transport, &BTreeSet::from(["main".to_string()])).unwrap();
            assert_eq!(!failures.is_empty(), eligible, "{name}/{event}");
        }
    }

    /// cas-c188: main's latest completed run was seven weeks old, for commit
    /// 5cf566c4, while main stood at fb1fb4da. A red run for a commit that is
    /// not the branch tip is history and must not wake the supervisor.
    #[test]
    fn a_red_run_for_a_commit_that_is_not_the_branch_tip_is_silent_cas_c188() {
        let transport = TipTransport {
            runs: vec![run_with(
                "main",
                "5cf566c4",
                31_120_167_290,
                Some("failure"),
            )],
            tip: Some("fb1fb4da"),
            failed_job: Some("Fast Validation"),
            job_lookups: Cell::new(0),
        };
        let failures = collect_failures(&transport, &BTreeSet::from(["main".to_string()]))
            .expect("the run list is valid");
        assert!(failures.is_empty());
        assert_eq!(
            transport.job_lookups.get(),
            0,
            "no job lookup for a stale run"
        );
    }

    /// cas-c188: a run whose jobs were cancelled (and the rest passed) failed
    /// nothing; it is not a red run and there is no failing job to name.
    #[test]
    fn a_run_with_only_cancelled_jobs_is_not_red_cas_c188() {
        let transport = TipTransport {
            runs: vec![run_with("main", "fb1fb4da", 43, Some("failure"))],
            tip: Some("fb1fb4da"),
            failed_job: None,
            job_lookups: Cell::new(0),
        };
        let failures = collect_failures(&transport, &BTreeSet::from(["main".to_string()]))
            .expect("the run list is valid");
        assert!(failures.is_empty());
        assert!(job_conclusion_is_failure(Some("failure")));
        assert!(job_conclusion_is_failure(Some("timed_out")));
        assert!(!job_conclusion_is_failure(Some("cancelled")));
        assert!(!job_conclusion_is_failure(Some("skipped")));
        assert!(!job_conclusion_is_failure(None));
    }

    /// cas-c188: a red run at the branch tip with a failed job still wakes the
    /// supervisor, naming that job.
    #[test]
    fn a_red_run_at_the_tip_names_its_failed_job_cas_c188() {
        let transport = TipTransport {
            runs: vec![run_with("main", "fb1fb4da", 44, Some("failure"))],
            tip: Some("fb1fb4da"),
            failed_job: Some("Fast Validation"),
            job_lookups: Cell::new(0),
        };
        let failures = collect_failures(&transport, &BTreeSet::from(["main".to_string()]))
            .expect("the run list is valid");
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].failing_job, "Fast Validation");
        assert!(relay_body(&failures[0]).contains("Failing job: Fast Validation"));
    }

    #[test]
    fn red_run_followed_by_green_is_silent_without_failed_run_lookups() {
        let transport = FakeTransport {
            runs: vec![
                run_with("main", "stale-red", 41, Some("failure")),
                run_with("main", "current-green", 42, Some("success")),
            ],
            pulls: Vec::new(),
            delivery_pulls: Vec::new(),
            job: "should not run".to_string(),
            log: None,
            calls: Cell::new(0),
        };

        let failures = collect_failures(&transport, &BTreeSet::from(["main".to_string()]))
            .expect("the run list is valid");

        assert!(failures.is_empty());
        assert_eq!(transport.calls.get(), 0);
    }

    #[test]
    fn newest_red_run_is_the_only_alert_and_names_suppressed_range() {
        let transport = FakeTransport {
            runs: vec![
                run_with("main", "oldest-red", 40, Some("failure")),
                run_with("main", "middle-red", 41, Some("failure")),
                run_with("main", "current-red", 42, Some("failure")),
            ],
            pulls: Vec::new(),
            delivery_pulls: Vec::new(),
            job: "Fast Validation".to_string(),
            log: None,
            calls: Cell::new(0),
        };

        let failures = collect_failures(&transport, &BTreeSet::from(["main".to_string()]))
            .expect("the run list is valid");

        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].head_sha, "current-red");
        assert_eq!(transport.calls.get(), 1);
        let body = relay_body(&failures[0]);
        assert!(body.contains("Suppressed 2 historical red run(s)"));
        assert!(body.contains("oldest-red through middle-red"));
    }

    #[test]
    fn repeat_failure_has_one_stable_branch_sha_dedupe_key() {
        let failure = CiFailure {
            branch: "main".to_string(),
            head_sha: "abc123".to_string(),
            run_id: 1,
            run_url: "url".to_string(),
            failing_job: "test".to_string(),
            failing_test: None,
            suppressed_red_runs: Vec::new(),
            merged_into: None,
        };
        assert_eq!(failure.dedupe_key(), "ci-red-run:main:abc123");
        assert_eq!(failure.dedupe_key(), failure.clone().dedupe_key());
    }

    #[test]
    fn repeated_failure_enqueues_exactly_one_lifecycle_wake_relay() {
        let temp = tempfile::TempDir::new().unwrap();
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        let failure = CiFailure {
            branch: "main".to_string(),
            head_sha: "abc123".to_string(),
            run_id: 1,
            run_url: "https://github.test/runs/1".to_string(),
            failing_job: "Fast Validation".to_string(),
            failing_test: None,
            suppressed_red_runs: Vec::new(),
            merged_into: None,
        };
        assert!(emit_failure(&queue, "factory-session", &failure).unwrap());
        assert!(!emit_failure(&queue, "factory-session", &failure).unwrap());
        let rows = queue.peek_all(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, "lifecycle-wake:ci-red-run:main:abc123");
        assert!(rows[0].prompt.contains("Failing job: Fast Validation"));
    }

    /// cas-6c13: a red run on a lane already merged into an integration
    /// branch is a critical blocker wake that names that branch.
    #[test]
    fn red_run_on_a_merged_lane_is_a_blocker_naming_the_integration_branch() {
        let temp = tempfile::TempDir::new().unwrap();
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        let failure = CiFailure {
            branch: "factory/noble-gopher-88-cas-38e7".to_string(),
            head_sha: "bc452ab0d".to_string(),
            run_id: 38684348776,
            run_url: "https://github.test/runs/38684348776".to_string(),
            failing_job: "Scoped Validation".to_string(),
            failing_test: None,
            suppressed_red_runs: Vec::new(),
            merged_into: Some("release-flow/c-runtime-removal".to_string()),
        };
        assert!(emit_failure(&queue, "factory-session", &failure).unwrap());
        let rows = queue.peek_all(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].priority, NotificationPriority::Critical);
        assert_eq!(
            rows[0].summary.as_deref(),
            Some(
                "BLOCKER: merged lane factory/noble-gopher-88-cas-38e7 is red in release-flow/c-runtime-removal"
            )
        );
        assert!(rows[0].prompt.contains(
            "BLOCKER: this commit is already merged into release-flow/c-runtime-removal"
        ));
        assert!(crate::prompt_revalidation::parse_ci_red_run_envelope(
            &rows[0].prompt
        ));
    }

    #[test]
    fn mark_merged_failures_names_the_branch_that_contains_the_red_commit() {
        let temp = tempfile::TempDir::new().unwrap();
        let repo = temp.path();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
                .args(args)
                .current_dir(repo)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "base"]);
        git(&["checkout", "-q", "-b", "factory/merged"]);
        git(&["commit", "-q", "--allow-empty", "-m", "merged lane"]);
        let merged = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "-b", "factory/open", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "open lane"]);
        let open = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "-b", "epic/release", "main"]);
        git(&[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "merge lane",
            "factory/merged",
        ]);
        let failure = |branch: &str, sha: &str| CiFailure {
            branch: branch.to_string(),
            head_sha: sha.to_string(),
            run_id: 1,
            run_url: "url".to_string(),
            failing_job: "Scoped Validation".to_string(),
            failing_test: None,
            suppressed_red_runs: Vec::new(),
            merged_into: None,
        };
        let marked = mark_merged_failures(
            repo,
            vec![
                failure("factory/merged", &merged),
                failure("factory/open", &open),
            ],
        );
        assert_eq!(marked[0].merged_into.as_deref(), Some("epic/release"));
        assert_eq!(
            marked[1].merged_into, None,
            "an unmerged lane stays a routine notice"
        );
    }

    #[test]
    fn red_run_receipt_survives_delivery_cleanup_and_reopened_watch_cycle() {
        let temp = tempfile::TempDir::new().unwrap();
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        let failure = CiFailure {
            branch: "main".to_string(),
            head_sha: "abc123".to_string(),
            run_id: 1,
            run_url: "https://github.test/runs/1".to_string(),
            failing_job: "Fast Validation".to_string(),
            failing_test: None,
            suppressed_red_runs: Vec::new(),
            merged_into: None,
        };

        assert!(emit_failure(&queue, "factory-session", &failure).unwrap());
        assert_eq!(queue.poll_for_target("supervisor", 10).unwrap().len(), 1);
        assert_eq!(queue.cleanup_old(0).unwrap(), 0);
        drop(queue);

        let reopened = SqlitePromptQueueStore::open(temp.path()).unwrap();
        reopened.init().unwrap();
        assert!(!emit_failure(&reopened, "factory-session", &failure).unwrap());
    }

    #[test]
    fn green_runs_are_silent_without_expensive_job_lookup() {
        let transport = FakeTransport {
            runs: vec![run("main", Some("success"))],
            pulls: Vec::new(),
            delivery_pulls: Vec::new(),
            job: "should not run".to_string(),
            log: None,
            calls: Cell::new(0),
        };
        let failures = collect_failures(&transport, &BTreeSet::from(["main".to_string()])).unwrap();
        assert!(failures.is_empty());
        assert_eq!(transport.calls.get(), 0);
    }

    #[test]
    fn failed_merge_group_ejection_is_once_per_episode_and_rearms_on_requeue() {
        let delivery = AwaitingMergeDelivery {
            task_id: "cas-fc35".to_string(),
            worker: "fast-jaguar-59".to_string(),
            branch: "factory/fast-jaguar-59".to_string(),
            branch_tip: None,
            pr_number: None,
        };
        let mut failed = run_with(
            "gh-readonly-queue/main/pr-556-base",
            "queue-sha",
            901,
            Some("failure"),
        );
        failed.event = "merge_group".to_string();
        let ejected = FakeTransport {
            runs: vec![failed.clone()],
            pulls: vec![pull(556, &delivery.branch, false, "2026-08-20T15:32:03Z")],
            delivery_pulls: Vec::new(),
            job: String::new(),
            log: None,
            calls: Cell::new(0),
        };
        let first = collect_merge_queue_ejections(
            &ejected,
            std::slice::from_ref(&delivery),
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(first.ejections.len(), 1);
        assert_eq!(first.ejections[0].failed_run_id, Some(901));
        assert_eq!(first.ejections[0].occurrence, "merge-group-run:901");

        let requeued = FakeTransport {
            runs: vec![failed],
            pulls: vec![pull(556, &delivery.branch, true, "2026-08-20T15:40:00Z")],
            delivery_pulls: Vec::new(),
            job: String::new(),
            log: None,
            calls: Cell::new(0),
        };
        let queued = collect_merge_queue_ejections(
            &requeued,
            std::slice::from_ref(&delivery),
            &BTreeSet::new(),
        )
        .unwrap();
        assert!(queued.ejections.is_empty(), "normal requeue must be silent");
        assert_eq!(queued.queued_prs, BTreeSet::from([556]));

        let mut second_failed = run_with(
            "gh-readonly-queue/main/pr-556-next-base",
            "queue-sha-2",
            902,
            Some("failure"),
        );
        second_failed.event = "merge_group".to_string();
        let re_ejected = FakeTransport {
            runs: vec![second_failed],
            pulls: vec![pull(556, &delivery.branch, false, "2026-08-20T15:42:00Z")],
            delivery_pulls: Vec::new(),
            job: String::new(),
            log: None,
            calls: Cell::new(0),
        };
        let second =
            collect_merge_queue_ejections(&re_ejected, &[delivery], &queued.queued_prs).unwrap();
        assert_eq!(second.ejections.len(), 1);
        assert_eq!(second.ejections[0].failed_run_id, Some(902));
        assert_ne!(
            first.ejections[0].occurrence,
            second.ejections[0].occurrence
        );

        let normal_merge = FakeTransport {
            runs: Vec::new(),
            pulls: Vec::new(),
            delivery_pulls: Vec::new(),
            job: String::new(),
            log: None,
            calls: Cell::new(0),
        };
        let normal_delivery = AwaitingMergeDelivery {
            task_id: "cas-fc35".to_string(),
            worker: "fast-jaguar-59".to_string(),
            branch: "factory/fast-jaguar-59".to_string(),
            branch_tip: None,
            pr_number: None,
        };
        assert!(
            collect_merge_queue_ejections(
                &normal_merge,
                &[normal_delivery],
                &BTreeSet::from([556]),
            )
            .unwrap()
            .ejections
            .is_empty(),
            "a normally merged PR must not relay"
        );
    }

    #[test]
    fn failed_required_pr_lane_run_matches_current_head_and_delivery() {
        let delivery = AwaitingMergeDelivery {
            task_id: "cas-pr-lane".to_string(),
            worker: "bright-otter".to_string(),
            branch: "factory/bright-otter".to_string(),
            branch_tip: None,
            pr_number: None,
        };
        let transport = FakeTransport {
            runs: vec![run_with(
                "factory/bright-otter",
                "current-head",
                903,
                Some("failure"),
            )],
            pulls: vec![armed_pull(
                659,
                &delivery.branch,
                "current-head",
                true,
                "2026-08-31T20:31:00Z",
            )],
            delivery_pulls: Vec::new(),
            job: REQUIRED_PR_LANE_CHECK.to_string(),
            log: None,
            calls: Cell::new(0),
        };
        let poll = collect_merge_queue_ejections_with_arm_state(
            &transport,
            std::slice::from_ref(&delivery),
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .unwrap();

        assert_eq!(poll.queued_prs, BTreeSet::from([659]));
        assert_eq!(poll.pr_lane_failures.len(), 1);
        let failure = &poll.pr_lane_failures[0];
        assert_eq!(failure.task_id, "cas-pr-lane");
        assert_eq!(failure.worker, "bright-otter");
        assert_eq!(failure.pr_number, 659);
        assert_eq!(failure.head_sha, "current-head");
        assert_eq!(failure.run_id, 903);
        assert_eq!(failure.check_name, REQUIRED_PR_LANE_CHECK);
    }

    #[test]
    fn stale_head_and_other_checks_do_not_create_pr_lane_failure() {
        let delivery = AwaitingMergeDelivery {
            task_id: "cas-pr-lane".to_string(),
            worker: "bright-otter".to_string(),
            branch: "factory/bright-otter".to_string(),
            branch_tip: None,
            pr_number: None,
        };
        let transport = FakeTransport {
            runs: vec![
                run_with(
                    &delivery.branch,
                    "old-head",
                    904,
                    Some("failure"),
                ),
                run_with(
                    &delivery.branch,
                    "current-head",
                    905,
                    Some("failure"),
                ),
            ],
            pulls: vec![armed_pull(
                659,
                &delivery.branch,
                "current-head",
                true,
                "2026-08-31T20:31:00Z",
            )],
            delivery_pulls: Vec::new(),
            job: "macOS Check".to_string(),
            log: None,
            calls: Cell::new(0),
        };
        let failures = collect_pr_lane_failures(
            &transport,
            std::slice::from_ref(&delivery),
            &transport.pulls,
            &transport.runs,
        )
        .unwrap();
        assert!(failures.is_empty());
        assert_eq!(transport.calls.get(), 1);
    }

    #[test]
    fn auto_merge_arm_loss_after_green_checks_is_one_episode() {
        let delivery = AwaitingMergeDelivery {
            task_id: "cas-pr-arm".to_string(),
            worker: "bright-otter".to_string(),
            branch: "factory/bright-otter".to_string(),
            branch_tip: None,
            pr_number: None,
        };
        let armed = FakeTransport {
            runs: Vec::new(),
            pulls: vec![armed_pull(
                660,
                &delivery.branch,
                "current-head",
                false,
                "2026-08-31T20:00:00Z",
            )],
            delivery_pulls: Vec::new(),
            job: String::new(),
            log: None,
            calls: Cell::new(0),
        };
        let first = collect_merge_queue_ejections_with_arm_state(
            &armed,
            std::slice::from_ref(&delivery),
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .unwrap();
        assert!(first.ejections.is_empty());
        assert_eq!(first.auto_merge_prs, BTreeSet::from([660]));

        let mut disarmed_pull = armed.pulls[0].clone();
        disarmed_pull.is_in_merge_queue = false;
        disarmed_pull.updated_at = "2026-08-31T20:04:00Z".to_string();
        disarmed_pull.auto_merge_request = None;
        let disarmed = FakeTransport {
            runs: Vec::new(),
            pulls: vec![disarmed_pull],
            delivery_pulls: Vec::new(),
            job: String::new(),
            log: None,
            calls: Cell::new(0),
        };
        let second = collect_merge_queue_ejections_with_arm_state(
            &disarmed,
            std::slice::from_ref(&delivery),
            &first.queued_prs,
            &first.auto_merge_prs,
        )
        .unwrap();
        assert_eq!(second.ejections.len(), 1);
        assert_eq!(second.ejections[0].failed_run_id, None);
        assert_eq!(
            second.ejections[0].occurrence,
            "auto-merge-disarmed:current-head:2026-08-31T20:04:00Z"
        );

        let replay = collect_merge_queue_ejections_with_arm_state(
            &disarmed,
            std::slice::from_ref(&delivery),
            &second.queued_prs,
            &second.auto_merge_prs,
        )
        .unwrap();
        assert!(replay.ejections.is_empty());
    }

    #[test]
    fn pr_lane_failure_dedupe_is_pr_head_scoped() {
        let failure = PrLaneFailure {
            task_id: "cas-pr-lane".to_string(),
            worker: "bright-otter".to_string(),
            pr_number: 659,
            head_sha: "head-a".to_string(),
            run_id: 1,
            run_url: "url".to_string(),
            check_name: REQUIRED_PR_LANE_CHECK.to_string(),
        };
        assert_eq!(failure.dedupe_key(), "pr-lane-failed:659:head-a");
        let rerun = PrLaneFailure { run_id: 2, ..failure.clone() };
        assert_eq!(failure.dedupe_key(), rerun.dedupe_key());
        let corrective = PrLaneFailure {
            head_sha: "head-b".to_string(),
            ..failure
        };
        assert_ne!(rerun.dedupe_key(), corrective.dedupe_key());
    }

    #[test]
    fn unavailable_auth_or_transport_is_a_nonfatal_result() {
        struct NoAuth;
        impl CiTransport for NoAuth {
            fn completed_runs(&self) -> Result<Vec<CiRun>, CiWatchError> {
                Err(CiWatchError::Unavailable(
                    "gh auth login required".to_string(),
                ))
            }
            fn failing_job(&self, _: u64) -> Result<Option<String>, CiWatchError> {
                unreachable!()
            }
            fn failed_log(&self, _: u64) -> Result<Option<String>, CiWatchError> {
                unreachable!()
            }
            fn merge_queue_pull_requests(
                &self,
            ) -> Result<Vec<MergeQueuePullRequest>, CiWatchError> {
                unreachable!()
            }
            fn delivery_pull_requests(
                &self,
                _: &str,
            ) -> Result<Vec<DeliveryPullRequest>, CiWatchError> {
                unreachable!()
            }
        }
        assert!(matches!(
            collect_failures(&NoAuth, &BTreeSet::new()),
            Err(CiWatchError::Unavailable(_))
        ));
    }

    #[test]
    fn external_wake_filters_have_durable_branch_and_tag_shapes() {
        let branch = parse_external_wake_condition(
            EXTERNAL_BRANCH_CONTAINED_EVENT,
            &serde_json::json!({
                "commit": "abc123",
                "branch": "factory/worker",
                "target_branch": "main"
            }),
        )
        .unwrap();
        assert_eq!(
            branch,
            ExternalWakeCondition::BranchContained {
                commit: "abc123".to_string(),
                target_branch: "main".to_string(),
            }
        );

        let tag = parse_external_wake_condition(
            EXTERNAL_TAG_EXISTS_EVENT,
            &serde_json::json!({"tag": "v3.6.0"}),
        )
        .unwrap();
        assert_eq!(
            tag,
            ExternalWakeCondition::TagExists {
                tag: "v3.6.0".to_string()
            }
        );
    }

    /// GH #1208: `origin/staging` used to fetch `refs/heads/origin/staging`,
    /// which never exists, so the reminder could never fire.
    #[test]
    fn external_target_branch_drops_remote_tracking_prefix() {
        for (raw, bare) in [
            ("staging", "staging"),
            ("origin/staging", "staging"),
            ("refs/remotes/origin/epic/v41", "epic/v41"),
            ("refs/heads/main", "main"),
            (" origin/main ", "main"),
            ("origin/", ""),
        ] {
            assert_eq!(normalize_external_target_branch(raw), bare, "{raw:?}");
        }
        let legacy = parse_external_wake_condition(
            EXTERNAL_BRANCH_CONTAINED_EVENT,
            &serde_json::json!({"branch": "factory/x", "target": "origin/staging"}),
        )
        .unwrap();
        assert_eq!(
            legacy,
            ExternalWakeCondition::BranchContained {
                commit: "factory/x".to_string(),
                target_branch: "staging".to_string(),
            }
        );
        assert!(parse_external_wake_condition(
            EXTERNAL_BRANCH_CONTAINED_EVENT,
            &serde_json::json!({"commit": "abc", "target_branch": "origin/"}),
        )
        .is_err());
    }

    #[test]
    fn external_wake_git_probe_uses_fresh_origin_and_reports_observation() {
        let temp = tempfile::TempDir::new().unwrap();
        git(temp.path(), &["init", "--bare", "-q", "origin.git"]);
        let origin = temp.path().join("origin.git");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("file"), "first\n").unwrap();
        git(&repo, &["add", "file"]);
        git(&repo, &["commit", "-qm", "first"]);
        let first = git_output(&repo, &["rev-parse", "HEAD"]);
        // release.sh creates an annotated tag before the build and push.
        git(&repo, &["tag", "-a", "v1", "-m", "release v1"]);
        git(&repo, &["branch", "-M", "factory/crisp-crane-67"]);
        git(&repo, &["checkout", "--orphan", "main"]);
        git(&repo, &["rm", "-rf", "."]);
        std::fs::write(repo.join("unrelated"), "main\n").unwrap();
        git(&repo, &["add", "unrelated"]);
        git(&repo, &["commit", "-qm", "unrelated main"]);
        git(&repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
        git(&repo, &["push", "-q", "origin", "main"]);
        git(&repo, &["checkout", "-q", "factory/crisp-crane-67"]);
        // Keep the local main ref pointed at the delivered commit while the
        // remote main remains unrelated. The pre-fix probe incorrectly fired
        // by resolving the unqualified target_branch against this local ref.
        git(&repo, &["branch", "-f", "main", first.as_str()]);
        // Also pin a stale remote-tracking ref and make the remote
        // unavailable: a failed refresh must not fall back to this stale
        // value.
        git(
            &repo,
            &["update-ref", "refs/remotes/origin/main", first.as_str()],
        );
        let missing_origin = temp.path().join("missing-origin.git");
        git(
            &repo,
            &["remote", "set-url", "origin", missing_origin.to_str().unwrap()],
        );

        let condition = ExternalWakeCondition::BranchContained {
            commit: first.clone(),
            target_branch: "main".to_string(),
        };
        let tag_condition = ExternalWakeCondition::TagExists {
            tag: "v1".to_string(),
        };
        assert!(external_wake_condition_observation(&repo, &tag_condition)
            .unwrap()
            .is_none());
        assert!(!external_wake_condition_satisfied(&repo, &condition).unwrap());
        assert!(external_wake_condition_observation(&repo, &condition)
            .unwrap()
            .is_none());

        // Move the remote target forward to contain the source commit. The
        // next fresh fetch flips the condition exactly once for the pending
        // reminder edge, and records the ref/SHA used for that decision.
        git(
            &repo,
            &["remote", "set-url", "origin", origin.to_str().unwrap()],
        );
        // The tag only becomes observable once it is published to origin.
        git(&repo, &["push", "-q", "origin", "refs/tags/v1"]);
        let tag_observation = external_wake_condition_observation(&repo, &tag_condition)
            .unwrap()
            .unwrap();
        assert_eq!(tag_observation.compared_ref, "refs/tags/v1");
        assert_eq!(
            tag_observation.compared_sha,
            git_output(&repo, &["rev-parse", "refs/tags/v1"])
        );
        assert!(tag_condition
            .description_with_observation(&tag_observation)
            .contains(&format!("refs/tags/v1@{}", tag_observation.compared_sha)));
        git(&repo, &["tag", "-d", "v1"]);
        assert!(external_wake_condition_satisfied(&repo, &tag_condition).unwrap());
        git(
            &repo,
            &[
                "push",
                "-q",
                "--force",
                "origin",
                "factory/crisp-crane-67:main",
            ],
        );
        let observation = external_wake_condition_observation(&repo, &condition)
            .unwrap()
            .unwrap();
        assert_eq!(observation.compared_ref, "refs/remotes/origin/main");
        assert_eq!(observation.compared_sha, first);
        assert!(external_wake_condition_satisfied(&repo, &condition).unwrap());

        assert!(external_wake_condition_satisfied(
            &repo,
            &ExternalWakeCondition::BranchContained {
                commit: observation.compared_sha.clone(),
                target_branch: "main".to_string(),
            }
        )
        .unwrap());
        // A failed remote probe must ignore even an existing local tag.
        git(&repo, &["tag", "-a", "v1", "-m", "local replacement"]);
        git(
            &repo,
            &["remote", "set-url", "origin", missing_origin.to_str().unwrap()],
        );
        assert!(external_wake_condition_observation(&repo, &tag_condition)
            .unwrap()
            .is_none());
        assert!(!external_wake_condition_satisfied(&repo, &tag_condition).unwrap());
        assert!(!external_wake_condition_satisfied(
            &repo,
            &ExternalWakeCondition::TagExists {
                tag: "not-created".to_string(),
            }
        )
        .unwrap());
    }

    fn git(repo: &std::path::Path, args: &[&str]) {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(repo)
                .status()
                .unwrap()
                .success(),
            "git {:?} failed",
            args
        );
    }

    fn git_output(repo: &std::path::Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {:?} failed", args);
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }
}
