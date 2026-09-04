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
const EXTERNAL_GIT_TIMEOUT: Duration = Duration::from_secs(2);
const GH_CALL_TIMEOUT: Duration = Duration::from_secs(8);
pub(crate) const REQUIRED_PR_LANE_CHECK: &str = "Scoped Validation (factory/PR)";

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
    fn failing_job(&self, run_id: u64) -> Result<String, CiWatchError>;
    fn failed_log(&self, run_id: u64) -> Result<Option<String>, CiWatchError>;
    fn merge_queue_pull_requests(&self) -> Result<Vec<MergeQueuePullRequest>, CiWatchError>;
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CiRun {
    pub id: u64,
    pub head_branch: String,
    pub head_sha: String,
    pub html_url: String,
    pub status: String,
    pub conclusion: Option<String>,
    #[serde(default)]
    pub event: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AwaitingMergeDelivery {
    pub task_id: String,
    pub worker: String,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct MergeQueuePullRequest {
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct MergeQueuePoll {
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

/// Parse the stable JSON contract accepted by `coordination.remind` for an
/// external condition. Keep this strict so a typo remains pending and visible
/// rather than silently becoming an always-true wake.
pub(crate) fn parse_external_wake_condition(
    event_type: &str,
    filter: &serde_json::Value,
) -> Result<ExternalWakeCondition, String> {
    let object = filter
        .as_object()
        .ok_or_else(|| "external remind_filter must be a JSON object".to_string())?;
    let string_field = |name: &str| {
        object
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && !value.chars().any(char::is_whitespace))
            .map(str::to_string)
            .ok_or_else(|| format!("external remind_filter requires non-empty string field {name:?}"))
    };

    match event_type {
        EXTERNAL_BRANCH_CONTAINED_EVENT => {
            let commit = object
                .get("commit")
                .or_else(|| object.get("branch_tip"))
                .or_else(|| object.get("branch"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty() && !value.chars().any(char::is_whitespace))
                .map(str::to_string)
                .ok_or_else(|| {
                    "branch_contained_in remind_filter requires string field `commit` (or `branch_tip`/`branch`)".to_string()
                })?;
            let target_branch = object
                .get("target_branch")
                .or_else(|| object.get("target"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty() && !value.chars().any(char::is_whitespace))
                .map(str::to_string)
                .ok_or_else(|| {
                    "branch_contained_in remind_filter requires string field `target_branch`".to_string()
                })?;
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
/// refreshes the named origin branch before resolving it, so a local branch or
/// stale remote-tracking ref can never satisfy a reminder for another target.
/// A normal non-zero git status means the condition is false (for example the
/// target branch or tag is not present yet); process failures/timeouts are
/// surfaced so the daemon can retain the pending row and retry on a later
/// cadence.
pub(crate) fn external_wake_condition_satisfied(
    project: &Path,
    condition: &ExternalWakeCondition,
) -> Result<bool, CiWatchError> {
    Ok(external_wake_condition_observation(project, condition)?.is_some())
}

/// Evaluate an external condition and return the exact ref/SHA that was
/// compared when it is satisfied. For branch conditions, the fetch is forced
/// into `refs/remotes/origin/<target>` and failures return false without
/// consulting any stale copy of that ref.
pub(crate) fn external_wake_condition_observation(
    project: &Path,
    condition: &ExternalWakeCondition,
) -> Result<Option<ExternalWakeObservation>, CiWatchError> {
    match condition {
        ExternalWakeCondition::BranchContained {
            commit,
            target_branch,
        } => {
            let check_args = vec![
                "check-ref-format".to_string(),
                "--branch".to_string(),
                target_branch.clone(),
            ];
            if !run_external_git_command(project, &check_args)?
                .status
                .success()
            {
                return Ok(None);
            }

            let target_ref = format!("refs/remotes/origin/{target_branch}");
            let fetch_args = vec![
                "fetch".to_string(),
                "--no-tags".to_string(),
                "origin".to_string(),
                format!("+refs/heads/{target_branch}:{target_ref}"),
            ];
            // Never fall back to an existing remote-tracking ref if the
            // refresh fails: that ref may be exactly the stale state that
            // caused a false positive such as reminder #1179.
            if !run_external_git_command(project, &fetch_args)?
                .status
                .success()
            {
                return Ok(None);
            }

            let Some(target_sha) = resolve_external_git_commit(project, &target_ref)? else {
                return Ok(None);
            };
            let Some(commit_sha) = resolve_external_git_commit(project, commit)? else {
                return Ok(None);
            };
            let merge_args = vec![
                "merge-base".to_string(),
                "--is-ancestor".to_string(),
                commit_sha,
                target_sha.clone(),
            ];
            if !run_external_git_command(project, &merge_args)?
                .status
                .success()
            {
                return Ok(None);
            }
            Ok(Some(ExternalWakeObservation {
                compared_ref: target_ref,
                compared_sha: target_sha,
            }))
        }
        ExternalWakeCondition::TagExists { tag } => {
            let tag_ref = format!("refs/tags/{tag}");
            let Some(tag_sha) = resolve_external_git_commit(project, &tag_ref)? else {
                return Ok(None);
            };
            Ok(Some(ExternalWakeObservation {
                compared_ref: tag_ref,
                compared_sha: tag_sha,
            }))
        }
    }
}

fn resolve_external_git_commit(
    project: &Path,
    revision: &str,
) -> Result<Option<String>, CiWatchError> {
    let args = vec![
        "rev-parse".to_string(),
        "--verify".to_string(),
        "--quiet".to_string(),
        format!("{revision}^{{commit}}"),
    ];
    let output = run_external_git_command(project, &args)?;
    if !output.status.success() {
        return Ok(None);
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!sha.is_empty()).then_some(sha))
}

fn run_external_git_command(
    project: &Path,
    args: &[String],
) -> Result<std::process::Output, CiWatchError> {
    let mut command = Command::new("git");
    command.args(args).current_dir(project);
    run_command(
        &mut command,
        Deadline::after(EXTERNAL_GIT_TIMEOUT),
        EXTERNAL_GIT_TIMEOUT,
    )
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
}

impl GhCiTransport {
    pub(crate) fn from_project(project: &Path) -> Result<Self, CiWatchError> {
        let output = Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(project)
            .output()
            .map_err(|e| CiWatchError::Unavailable(format!("cannot read git origin: {e}")))?;
        if !output.status.success() {
            return Err(CiWatchError::Unavailable(
                "cannot read git origin for CI watcher".to_string(),
            ));
        }
        let origin = String::from_utf8_lossy(&output.stdout);
        let repo = crate::cli::integrate::github::parse_origin_url(&origin)
            .map(|repo| repo.full_name())
            .ok_or_else(|| {
                CiWatchError::Unavailable(
                    "origin is not a GitHub owner/repo URL; CI watcher disabled".to_string(),
                )
            })?;
        Ok(Self {
            repo,
            cwd: project.to_path_buf(),
        })
    }

    fn gh_json<T: for<'de> Deserialize<'de>>(&self, args: &[String]) -> Result<T, CiWatchError> {
        let mut command = Command::new("gh");
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
        let mut command = Command::new("gh");
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

    fn failing_job(&self, run_id: u64) -> Result<String, CiWatchError> {
        let response: JobsResponse = self.gh_json(&[
            "api".to_string(),
            "-X".to_string(),
            "GET".to_string(),
            format!("repos/{}/actions/runs/{run_id}/jobs", self.repo),
        ])?;
        Ok(response
            .jobs
            .into_iter()
            .find(|job| job.conclusion.as_deref() == Some("failure"))
            .map(|job| job.name)
            .unwrap_or_else(|| "unknown failing job".to_string()))
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

    fn merge_queue_pull_requests(&self) -> Result<Vec<MergeQueuePullRequest>, CiWatchError> {
        let repo = crate::cli::integrate::github::RepoRef::from_owner_slash_repo(&self.repo)
            .ok_or_else(|| {
                CiWatchError::Unavailable(
                    "invalid GitHub repository for merge-queue watcher".to_string(),
                )
            })?;
        let query = "query($owner: String!, $name: String!) { repository(owner: $owner, name: $name) { pullRequests(first: 100, states: OPEN) { nodes { number headRefName headRefOid isInMergeQueue updatedAt autoMergeRequest { enabledAt } statusCheckRollup { state } } } } }";
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
}

fn merge_group_pr_number(branch: &str) -> Option<u64> {
    let (_, suffix) = branch.rsplit_once("/pr-")?;
    suffix.split_once('-')?.0.parse().ok()
}

/// The queue snapshot is intentionally small: one GraphQL open-PR listing and
/// the existing completed-runs listing per minute. A delivery is ejected when
/// its open PR leaves a previously-observed queue, or when a failed
/// `merge_group` run proves that ejection after a daemon restart.
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

pub(crate) fn collect_merge_queue_ejections_with_arm_state(
    transport: &dyn CiTransport,
    deliveries: &[AwaitingMergeDelivery],
    previously_queued: &BTreeSet<u64>,
    previously_armed: &BTreeSet<u64>,
) -> Result<MergeQueuePoll, CiWatchError> {
    let pulls = transport.merge_queue_pull_requests()?;
    let runs = transport.completed_runs()?;
    let mut failed_runs = BTreeMap::<u64, u64>::new();
    for run in &runs {
        if run.event == "merge_group"
            && run.conclusion.as_deref() == Some("failure")
            && let Some(pr) = merge_group_pr_number(&run.head_branch)
        {
            failed_runs
                .entry(pr)
                .and_modify(|current| *current = (*current).max(run.id))
                .or_insert(run.id);
        }
    }

    let mut poll = MergeQueuePoll::default();
    for delivery in deliveries {
        let Some(pr) = pulls.iter().find(|pr| pr.head_branch == delivery.branch) else {
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
        let failed_run_id = failed_runs.get(&pr.number).copied();
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
            task_id: delivery.task_id.clone(),
            worker: delivery.worker.clone(),
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
        let check_name = transport.failing_job(run.id)?;
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

/// Keep only the latest completed failure for each watched branch.
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
        if run.status != "completed" || !watched_branches.contains(&run.head_branch) {
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

        let mut suppressed_red_runs: Vec<_> = runs
            .iter()
            .filter(|older| {
                older.head_branch == run.head_branch
                    && older.id < run.id
                    && older.status == "completed"
                    && older.conclusion.as_deref() == Some("failure")
            })
            .map(|older| SuppressedCiRun {
                head_sha: older.head_sha.clone(),
                run_id: older.id,
            })
            .collect();
        suppressed_red_runs.sort_by_key(|older| older.run_id);

        let failing_job = transport.failing_job(run.id)?;
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
        });
    }
    Ok(failures)
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
    format!(
        "<ci-red-run branch=\"{}\" head_sha=\"{}\" run_id=\"{}\">\nCI failed on branch {} at {}.\nRun: {}\nFailing job: {}{}{}\nThis red run is the branch's latest completed conclusion; inspect or fix it before merging further lanes.\n</ci-red-run>",
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
    let summary = format!("CI FAILED: {} ({})", failure.branch, failure.head_sha);
    match queue
        .enqueue_idempotent(
            &source,
            "supervisor",
            &relay_body(failure),
            Some(factory_session),
            Some(&summary),
            Some(NotificationPriority::High),
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
        job: String,
        log: Option<String>,
        calls: Cell<u8>,
    }

    impl CiTransport for FakeTransport {
        fn completed_runs(&self) -> Result<Vec<CiRun>, CiWatchError> {
            Ok(self.runs.clone())
        }
        fn failing_job(&self, _: u64) -> Result<String, CiWatchError> {
            self.calls.set(self.calls.get() + 1);
            Ok(self.job.clone())
        }
        fn failed_log(&self, _: u64) -> Result<Option<String>, CiWatchError> {
            Ok(self.log.clone())
        }
        fn merge_queue_pull_requests(&self) -> Result<Vec<MergeQueuePullRequest>, CiWatchError> {
            Ok(self.pulls.clone())
        }
    }

    fn run(branch: &str, conclusion: Option<&str>) -> CiRun {
        run_with(branch, "deadbeef", 42, conclusion)
    }

    fn run_with(branch: &str, head_sha: &str, id: u64, conclusion: Option<&str>) -> CiRun {
        CiRun {
            id,
            head_branch: branch.to_string(),
            head_sha: head_sha.to_string(),
            html_url: format!("https://github.test/org/repo/actions/runs/{id}"),
            status: "completed".to_string(),
            conclusion: conclusion.map(str::to_string),
            event: "push".to_string(),
        }
    }

    fn pull(number: u64, branch: &str, queued: bool, updated_at: &str) -> MergeQueuePullRequest {
        MergeQueuePullRequest {
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

    #[test]
    fn failed_completed_live_lane_emits_a_relay_with_job_and_test() {
        let transport = FakeTransport {
            runs: vec![run("factory/bright-otter", Some("failure"))],
            pulls: Vec::new(),
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

    #[test]
    fn red_run_followed_by_green_is_silent_without_failed_run_lookups() {
        let transport = FakeTransport {
            runs: vec![
                run_with("main", "stale-red", 41, Some("failure")),
                run_with("main", "current-green", 42, Some("success")),
            ],
            pulls: Vec::new(),
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
        };
        assert!(emit_failure(&queue, "factory-session", &failure).unwrap());
        assert!(!emit_failure(&queue, "factory-session", &failure).unwrap());
        let rows = queue.peek_all(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, "lifecycle-wake:ci-red-run:main:abc123");
        assert!(rows[0].prompt.contains("Failing job: Fast Validation"));
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
            job: String::new(),
            log: None,
            calls: Cell::new(0),
        };
        let normal_delivery = AwaitingMergeDelivery {
            task_id: "cas-fc35".to_string(),
            worker: "fast-jaguar-59".to_string(),
            branch: "factory/fast-jaguar-59".to_string(),
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
            fn failing_job(&self, _: u64) -> Result<String, CiWatchError> {
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
        git(&repo, &["tag", "v1"]);
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
        assert!(external_wake_condition_satisfied(
            &repo,
            &ExternalWakeCondition::TagExists {
                tag: "v1".to_string(),
            }
        )
        .unwrap());
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
