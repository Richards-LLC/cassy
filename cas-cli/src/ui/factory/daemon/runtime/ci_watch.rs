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
const GH_CALL_TIMEOUT: Duration = Duration::from_secs(8);

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
    #[serde(rename = "isInMergeQueue")]
    pub is_in_merge_queue: bool,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
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
        let query = "query($owner: String!, $name: String!) { repository(owner: $owner, name: $name) { pullRequests(first: 100, states: OPEN) { nodes { number headRefName isInMergeQueue updatedAt } } } }";
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
    let pulls = transport.merge_queue_pull_requests()?;
    let runs = transport.completed_runs()?;
    let mut failed_runs = BTreeMap::<u64, u64>::new();
    for run in runs {
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
        if pr.is_in_merge_queue {
            poll.queued_prs.insert(pr.number);
            continue;
        }
        let failed_run_id = failed_runs.get(&pr.number).copied();
        if !previously_queued.contains(&pr.number) && failed_run_id.is_none() {
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
    Ok(poll)
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
            is_in_merge_queue: queued,
            updated_at: updated_at.to_string(),
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
}
