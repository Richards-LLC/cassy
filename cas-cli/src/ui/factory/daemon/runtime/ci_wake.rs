//! Event reminders for GitHub CI completion and pull-request landing
//! (cas-0f05).
//!
//! A supervisor waiting on CI or the merge queue used to cover the wait with a
//! 10–15 minute delay reminder and often woke long after the run had finished.
//! `ci_completed` and `merge_queue_landed` are durable external conditions,
//! like `branch_contained_in`: the pending reminder row is the edge state, and
//! the daemon probes GitHub on [`CI_WAKE_INTERVAL`] while such a row exists.
//! The probe runs on its own thread, so a slow `gh` call never stalls the
//! daemon loop; the main loop only fires rows whose condition came back true.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::ci_watch::{CiWatchError, GhCiTransport};

pub(crate) const EXTERNAL_CI_COMPLETED_EVENT: &str = "ci_completed";
pub(crate) const EXTERNAL_MERGE_QUEUE_LANDED_EVENT: &str = "merge_queue_landed";
pub(crate) const CI_WAKE_EVENTS: [&str; 2] = [
    EXTERNAL_CI_COMPLETED_EVENT,
    EXTERNAL_MERGE_QUEUE_LANDED_EVENT,
];

/// Probe cadence while a CI reminder is pending. The contract is delivery
/// within 60 s of the run completing: one interval, one daemon refresh tick
/// (2 s) and the bounded `gh` calls (8 s each) stay inside it. GitHub cost: one run-list call per pending SHA (or
/// one PR read per pending PR) every 30 s, and nothing when no row is pending.
pub(crate) const CI_WAKE_INTERVAL: Duration = Duration::from_secs(30);

/// Events that never answer "is CI for this commit done": a scheduled
/// workflow runs against the default-branch tip on a timer and may sit queued
/// for days. They count only when the reminder names the workflow or event.
const UNFILTERED_IGNORED_EVENTS: [&str; 1] = ["schedule"];

/// When the newest page of runs for a SHA is all scheduled runs, these are
/// queried one by one so an idle default-branch tip still resolves.
const FALLBACK_EVENTS: [&str; 3] = ["push", "pull_request", "merge_group"];

/// Conclusions that count as a pass in the fired summary.
const PASSING_CONCLUSIONS: [&str; 3] = ["success", "skipped", "neutral"];

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum CiWakeTarget {
    /// A commit, 7–40 lowercase hex characters. A short SHA is resolved with
    /// the project's local git; the full SHA works for remote-only commits.
    Sha(String),
    /// A branch whose current forge tip is the commit to wait on.
    Branch(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum CiWakeCondition {
    CiCompleted {
        target: CiWakeTarget,
        /// Workflow name (`CI`) or file (`ci.yml` / `.github/workflows/ci.yml`).
        workflow: Option<String>,
        /// GitHub event (`push`, `pull_request`, `merge_group`, ...).
        event: Option<String>,
    },
    MergeQueueLanded {
        pr: u64,
    },
}

impl CiWakeCondition {
    pub(crate) fn event_type(&self) -> &'static str {
        match self {
            Self::CiCompleted { .. } => EXTERNAL_CI_COMPLETED_EVENT,
            Self::MergeQueueLanded { .. } => EXTERNAL_MERGE_QUEUE_LANDED_EVENT,
        }
    }

    pub(crate) fn to_json(&self) -> serde_json::Value {
        match self {
            Self::CiCompleted {
                target,
                workflow,
                event,
            } => {
                let mut value = match target {
                    CiWakeTarget::Sha(sha) => serde_json::json!({"sha": sha}),
                    CiWakeTarget::Branch(branch) => serde_json::json!({"branch": branch}),
                };
                if let Some(workflow) = workflow {
                    value["workflow"] = serde_json::Value::String(workflow.clone());
                }
                if let Some(event) = event {
                    value["event"] = serde_json::Value::String(event.clone());
                }
                value
            }
            Self::MergeQueueLanded { pr } => serde_json::json!({"pr": pr}),
        }
    }
}

/// Parse the JSON filter accepted by `coordination.remind` for the two CI
/// events. Strict, like the git conditions: a typo is rejected at creation
/// rather than becoming a reminder that can never fire.
///
/// - `ci_completed`: exactly one of `sha` (alias `commit`) or `branch`;
///   optional `workflow` and `event`.
/// - `merge_queue_landed`: `pr` as a number, `"1193"` or `"#1193"`.
pub(crate) fn parse_ci_wake_condition(
    event_type: &str,
    filter: &serde_json::Value,
) -> Result<CiWakeCondition, String> {
    let object = filter
        .as_object()
        .ok_or_else(|| "external remind_filter must be a JSON object".to_string())?;
    let optional_token = |name: &str| -> Result<Option<String>, String> {
        match object.get(name) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(value)) => {
                let value = value.trim();
                if value.is_empty() || value.starts_with('-') {
                    return Err(format!(
                        "{event_type} remind_filter field `{name}` must be a non-empty string not starting with '-'"
                    ));
                }
                Ok(Some(value.to_string()))
            }
            Some(_) => Err(format!(
                "{event_type} remind_filter field `{name}` must be a string"
            )),
        }
    };

    match event_type {
        EXTERNAL_CI_COMPLETED_EVENT => {
            let sha = match optional_token("sha")? {
                Some(sha) => Some(sha),
                None => optional_token("commit")?,
            };
            let branch = optional_token("branch")?;
            let target = match (sha, branch) {
                (Some(sha), None) => {
                    let sha = sha.to_ascii_lowercase();
                    if !(7..=40).contains(&sha.len())
                        || !sha.chars().all(|c| c.is_ascii_hexdigit())
                    {
                        return Err(
                            "ci_completed `sha` must be 7–40 hex characters".to_string()
                        );
                    }
                    CiWakeTarget::Sha(sha)
                }
                (None, Some(branch)) => {
                    if branch.chars().any(char::is_whitespace)
                        || branch.contains("..")
                        || branch.ends_with('/')
                    {
                        return Err("ci_completed `branch` must be a valid branch name".to_string());
                    }
                    CiWakeTarget::Branch(branch)
                }
                (Some(_), Some(_)) => {
                    return Err(
                        "ci_completed remind_filter takes `sha` or `branch`, not both".to_string()
                    );
                }
                (None, None) => {
                    return Err(
                        "ci_completed remind_filter requires `sha` (or `branch`), e.g. {\"sha\":\"<sha>\",\"workflow\":\"CI\"}"
                            .to_string(),
                    );
                }
            };
            let workflow = optional_token("workflow")?;
            let event = optional_token("event")?;
            if let Some(event) = &event
                && !event
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_')
            {
                return Err("ci_completed `event` must be a GitHub event name such as push".to_string());
            }
            Ok(CiWakeCondition::CiCompleted {
                target,
                workflow,
                event,
            })
        }
        EXTERNAL_MERGE_QUEUE_LANDED_EVENT => {
            let pr = match object.get("pr") {
                Some(serde_json::Value::Number(number)) => number.as_u64(),
                Some(serde_json::Value::String(text)) => {
                    text.trim().trim_start_matches('#').parse::<u64>().ok()
                }
                _ => None,
            }
            .filter(|pr| *pr > 0)
            .ok_or_else(|| {
                "merge_queue_landed remind_filter requires a positive `pr` number, e.g. {\"pr\":1193}"
                    .to_string()
            })?;
            Ok(CiWakeCondition::MergeQueueLanded { pr })
        }
        _ => Err(format!("unsupported CI reminder event: {event_type}")),
    }
}

/// One GitHub Actions workflow run, as returned by
/// `GET /repos/{owner}/{repo}/actions/runs`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct WorkflowRun {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
    pub head_sha: String,
    #[serde(default)]
    pub head_branch: Option<String>,
    #[serde(default)]
    pub html_url: String,
    pub status: String,
    pub conclusion: Option<String>,
    #[serde(default)]
    pub event: String,
}

impl WorkflowRun {
    /// `CI`, `ci.yml` and `.github/workflows/ci.yml` all name the same run.
    fn is_workflow(&self, workflow: &str) -> bool {
        self.name == workflow
            || self.path == workflow
            || self
                .path
                .rsplit('/')
                .next()
                .is_some_and(|file| file == workflow)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct WorkflowRunPage {
    #[serde(default)]
    pub total_count: u64,
    pub workflow_runs: Vec<WorkflowRun>,
}

/// `GET /repos/{owner}/{repo}/pulls/{number}`, reduced to landing state.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct PullRequestState {
    pub number: u64,
    pub state: String,
    #[serde(default)]
    pub merged_at: Option<String>,
    #[serde(default)]
    pub merge_commit_sha: Option<String>,
    #[serde(default)]
    pub html_url: String,
}

pub(crate) trait CiWakeTransport {
    /// Newest-first runs for one commit, optionally limited to one event.
    fn runs_for_head_sha(
        &self,
        sha: &str,
        event: Option<&str>,
    ) -> Result<WorkflowRunPage, CiWatchError>;
    /// The branch's current tip on the forge, `None` when absent.
    fn branch_tip(&self, branch: &str) -> Result<Option<String>, CiWatchError>;
    fn pull_request(&self, number: u64) -> Result<PullRequestState, CiWatchError>;
}

impl CiWakeTransport for GhCiTransport {
    fn runs_for_head_sha(
        &self,
        sha: &str,
        event: Option<&str>,
    ) -> Result<WorkflowRunPage, CiWatchError> {
        let mut args = vec![
            "api".to_string(),
            "-X".to_string(),
            "GET".to_string(),
            format!("repos/{}/actions/runs", self.repo),
            "-f".to_string(),
            format!("head_sha={sha}"),
            "-f".to_string(),
            "per_page=100".to_string(),
        ];
        if let Some(event) = event {
            args.push("-f".to_string());
            args.push(format!("event={event}"));
        }
        self.gh_json(&args)
    }

    fn branch_tip(&self, branch: &str) -> Result<Option<String>, CiWatchError> {
        super::ci_watch::CiTransport::branch_tip(self, branch)
    }

    fn pull_request(&self, number: u64) -> Result<PullRequestState, CiWatchError> {
        self.gh_json(&[
            "api".to_string(),
            "-X".to_string(),
            "GET".to_string(),
            format!("repos/{}/pulls/{number}", self.repo),
        ])
    }
}

/// One completed run in a fired `ci_completed` summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletedRun {
    pub id: u64,
    pub name: String,
    pub event: String,
    pub conclusion: String,
    pub url: String,
}

impl CompletedRun {
    fn passed(&self) -> bool {
        PASSING_CONCLUSIONS.contains(&self.conclusion.as_str())
    }
}

/// The remote state that satisfied a condition, carried into the fired
/// reminder so the supervisor acts without a second GitHub read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CiWakeObservation {
    CiCompleted {
        sha: String,
        runs: Vec<CompletedRun>,
    },
    PullRequest {
        pr: u64,
        merged: bool,
        merge_commit: Option<String>,
        url: String,
    },
}

impl CiWakeObservation {
    pub(crate) fn to_json(&self, condition: &CiWakeCondition) -> serde_json::Value {
        let mut value = condition.to_json();
        match self {
            Self::CiCompleted { sha, runs } => {
                value["head_sha"] = serde_json::Value::String(sha.clone());
                value["outcome"] = serde_json::Value::String(
                    if runs.iter().all(CompletedRun::passed) {
                        "success"
                    } else {
                        "failure"
                    }
                    .to_string(),
                );
                value["runs"] = runs
                    .iter()
                    .map(|run| {
                        serde_json::json!({
                            "id": run.id,
                            "name": run.name,
                            "event": run.event,
                            "conclusion": run.conclusion,
                            "url": run.url,
                        })
                    })
                    .collect();
            }
            Self::PullRequest {
                merged,
                merge_commit,
                url,
                ..
            } => {
                value["merged"] = serde_json::Value::Bool(*merged);
                value["merge_commit"] = merge_commit
                    .clone()
                    .map_or(serde_json::Value::Null, serde_json::Value::String);
                value["url"] = serde_json::Value::String(url.clone());
            }
        }
        value
    }

    pub(crate) fn description(&self) -> String {
        match self {
            Self::CiCompleted { sha, runs } => {
                let short = &sha[..sha.len().min(12)];
                let failed: Vec<&CompletedRun> =
                    runs.iter().filter(|run| !run.passed()).collect();
                let listed = |runs: &[&CompletedRun]| {
                    runs.iter()
                        .map(|run| format!("{} ({}) {} {}", run.name, run.event, run.conclusion, run.url))
                        .collect::<Vec<_>>()
                        .join("; ")
                };
                if failed.is_empty() {
                    let all: Vec<&CompletedRun> = runs.iter().collect();
                    format!(
                        "CI completed for {short}: all {} run(s) passed — {}",
                        runs.len(),
                        listed(&all)
                    )
                } else {
                    format!(
                        "CI completed for {short}: {} of {} run(s) did not pass — {}",
                        failed.len(),
                        runs.len(),
                        listed(&failed)
                    )
                }
            }
            Self::PullRequest {
                pr,
                merged: true,
                merge_commit,
                url,
            } => format!(
                "PR #{pr} landed as {} {url}",
                merge_commit.as_deref().unwrap_or("an unreported merge commit")
            ),
            Self::PullRequest { pr, url, .. } => {
                format!("PR #{pr} was closed without merging {url}")
            }
        }
    }
}

/// Evaluate one condition. `Ok(None)` means not yet true; an error leaves the
/// row pending for the next probe.
pub(crate) fn ci_wake_condition_observation(
    transport: &dyn CiWakeTransport,
    project: &Path,
    condition: &CiWakeCondition,
) -> Result<Option<CiWakeObservation>, CiWatchError> {
    match condition {
        CiWakeCondition::CiCompleted {
            target,
            workflow,
            event,
        } => {
            let sha = match target {
                CiWakeTarget::Sha(sha) if sha.len() == 40 => sha.clone(),
                CiWakeTarget::Sha(short) => {
                    super::ci_watch::resolve_external_git_commit(project, short)?.ok_or_else(
                        || {
                            CiWatchError::Unavailable(format!(
                                "short sha {short} is not in the local repository; pass the full 40-character sha"
                            ))
                        },
                    )?
                }
                CiWakeTarget::Branch(branch) => match transport.branch_tip(branch)? {
                    Some(tip) => tip,
                    None => return Ok(None),
                },
            };
            let runs = relevant_runs(transport, &sha, workflow.as_deref(), event.as_deref())?;
            if runs.is_empty() || runs.iter().any(|run| run.status != "completed") {
                return Ok(None);
            }
            Ok(Some(CiWakeObservation::CiCompleted {
                sha,
                runs: runs
                    .into_iter()
                    .map(|run| CompletedRun {
                        id: run.id,
                        name: run.name,
                        event: run.event,
                        conclusion: run.conclusion.unwrap_or_else(|| "unknown".to_string()),
                        url: run.html_url,
                    })
                    .collect(),
            }))
        }
        CiWakeCondition::MergeQueueLanded { pr } => {
            let state = transport.pull_request(*pr)?;
            let merged = state.merged_at.is_some();
            if !merged && state.state != "closed" {
                return Ok(None);
            }
            Ok(Some(CiWakeObservation::PullRequest {
                pr: *pr,
                merged,
                merge_commit: merged.then_some(state.merge_commit_sha).flatten(),
                url: state.html_url,
            }))
        }
    }
}

fn relevant_runs(
    transport: &dyn CiWakeTransport,
    sha: &str,
    workflow: Option<&str>,
    event: Option<&str>,
) -> Result<Vec<WorkflowRun>, CiWatchError> {
    let keep = |run: &WorkflowRun| {
        run.head_sha.eq_ignore_ascii_case(sha)
            && workflow.is_none_or(|workflow| run.is_workflow(workflow))
            && event.is_none_or(|event| run.event == event)
            && (workflow.is_some()
                || event.is_some()
                || !UNFILTERED_IGNORED_EVENTS.contains(&run.event.as_str()))
    };
    let page = transport.runs_for_head_sha(sha, event)?;
    let truncated = page.total_count > page.workflow_runs.len() as u64;
    let mut runs: Vec<WorkflowRun> = page.workflow_runs.into_iter().filter(keep).collect();
    if runs.is_empty() && truncated && event.is_none() {
        // The newest page is full of scheduled runs; the commit's own runs
        // are older. Ask for them by event instead.
        for fallback in FALLBACK_EVENTS {
            runs.extend(
                transport
                    .runs_for_head_sha(sha, Some(fallback))?
                    .workflow_runs
                    .into_iter()
                    .filter(keep),
            );
        }
    }
    Ok(runs)
}

/// A pending reminder whose CI condition is to be probed.
#[derive(Debug, Clone)]
pub(crate) struct CiWakeCandidate {
    pub reminder: cas_store::Reminder,
    pub condition: CiWakeCondition,
}

/// A pending reminder whose CI condition is now true.
#[derive(Debug, Clone)]
pub(crate) struct CiWakeReady {
    pub reminder: cas_store::Reminder,
    pub condition: CiWakeCondition,
    pub observation: CiWakeObservation,
}

/// Probe every candidate once. Identical conditions share one GitHub read.
pub(crate) fn collect_ci_wake_ready(
    transport: &dyn CiWakeTransport,
    project: &Path,
    candidates: Vec<CiWakeCandidate>,
) -> Vec<CiWakeReady> {
    let mut memo: HashMap<CiWakeCondition, Option<CiWakeObservation>> = HashMap::new();
    let mut ready = Vec::new();
    for candidate in candidates {
        let observation = match memo.get(&candidate.condition) {
            Some(observation) => observation.clone(),
            None => {
                let observation =
                    match ci_wake_condition_observation(transport, project, &candidate.condition)
                    {
                        Ok(observation) => observation,
                        Err(error) => {
                            tracing::warn!(
                                reminder_id = candidate.reminder.id,
                                event_type = candidate.condition.event_type(),
                                %error,
                                "CI reminder probe unavailable; retaining pending row"
                            );
                            None
                        }
                    };
                memo.insert(candidate.condition.clone(), observation.clone());
                observation
            }
        };
        if let Some(observation) = observation {
            ready.push(CiWakeReady {
                reminder: candidate.reminder,
                condition: candidate.condition,
                observation,
            });
        }
    }
    ready
}

/// Off-loop scheduler for the CI probe. At most one probe is in flight; its
/// result is collected on a later daemon tick without blocking.
#[derive(Debug, Default)]
pub(crate) struct CiWakeProbe {
    in_flight: Option<JoinHandle<Vec<CiWakeReady>>>,
    last_scan: Option<Instant>,
}

impl CiWakeProbe {
    /// True when no probe is running and the cadence has elapsed.
    pub(crate) fn due(&self, now: Instant) -> bool {
        self.in_flight.is_none()
            && self
                .last_scan
                .is_none_or(|last| now.saturating_duration_since(last) >= CI_WAKE_INTERVAL)
    }

    /// Record a scan at `now`; start `job` on its own thread when there is
    /// anything to probe.
    pub(crate) fn start<F>(&mut self, now: Instant, job: Option<F>)
    where
        F: FnOnce() -> Vec<CiWakeReady> + Send + 'static,
    {
        self.last_scan = Some(now);
        let Some(job) = job else {
            return;
        };
        match std::thread::Builder::new()
            .name("cas-ci-wake".to_string())
            .spawn(job)
        {
            Ok(handle) => self.in_flight = Some(handle),
            Err(error) => tracing::warn!(%error, "could not start the CI reminder probe"),
        }
    }

    /// Results of a finished probe; empty while it is still running.
    pub(crate) fn take_finished(&mut self) -> Vec<CiWakeReady> {
        if !self.in_flight.as_ref().is_some_and(JoinHandle::is_finished) {
            return Vec::new();
        }
        match self.in_flight.take().expect("checked above").join() {
            Ok(ready) => ready,
            Err(_) => {
                tracing::warn!("CI reminder probe panicked; retrying on its next cadence");
                Vec::new()
            }
        }
    }

    /// Daemon entry point: collect a finished probe, and start the next one
    /// against the real `gh` transport when due and a candidate is pending.
    pub(crate) fn poll_gh(
        &mut self,
        now: Instant,
        project: &Path,
        load_candidates: impl FnOnce() -> Vec<CiWakeCandidate>,
    ) -> Vec<CiWakeReady> {
        let ready = self.take_finished();
        if self.due(now) {
            let candidates = load_candidates();
            let project: PathBuf = project.to_path_buf();
            let job = (!candidates.is_empty()).then(|| {
                move || match GhCiTransport::from_project(&project) {
                    Ok(transport) => collect_ci_wake_ready(&transport, &project, candidates),
                    Err(error) => {
                        tracing::warn!(%error, "CI reminder probe unavailable; retaining pending rows");
                        Vec::new()
                    }
                }
            });
            self.start(now, job);
        }
        ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};

    /// Real GitHub API responses, trimmed to the fields Cassy reads:
    /// the runs for fba7a3365 (the merge of PR #1193 and the push to this
    /// epic branch) and PR #1193 itself, captured 2026-10-14.
    const RUNS_FIXTURE: &str = include_str!("fixtures/ci_wake/runs-fba7a336-completed.json");
    const PULL_FIXTURE: &str = include_str!("fixtures/ci_wake/pull-1193-merged.json");
    const FIXTURE_SHA: &str = "fba7a3365eb0cb6403be9d708e8f03151fc42054";
    /// The acceptance contract: delivered within 60 s of the run completing.
    const CI_WAKE_MAX_LATENCY: Duration = Duration::from_secs(60);

    fn fixture_runs() -> WorkflowRunPage {
        serde_json::from_str(RUNS_FIXTURE).unwrap()
    }

    #[derive(Default)]
    struct FakeTransport {
        runs: HashMap<(String, Option<String>), WorkflowRunPage>,
        tips: HashMap<String, String>,
        pulls: HashMap<u64, PullRequestState>,
        calls: RefCell<Vec<String>>,
    }

    impl CiWakeTransport for FakeTransport {
        fn runs_for_head_sha(
            &self,
            sha: &str,
            event: Option<&str>,
        ) -> Result<WorkflowRunPage, CiWatchError> {
            self.calls
                .borrow_mut()
                .push(format!("runs {sha} {}", event.unwrap_or("*")));
            Ok(self
                .runs
                .get(&(sha.to_string(), event.map(str::to_string)))
                .cloned()
                .unwrap_or(WorkflowRunPage {
                    total_count: 0,
                    workflow_runs: Vec::new(),
                }))
        }

        fn branch_tip(&self, branch: &str) -> Result<Option<String>, CiWatchError> {
            Ok(self.tips.get(branch).cloned())
        }

        fn pull_request(&self, number: u64) -> Result<PullRequestState, CiWatchError> {
            self.pulls
                .get(&number)
                .cloned()
                .ok_or_else(|| CiWatchError::Unavailable(format!("no PR {number}")))
        }
    }

    fn sha_condition(workflow: Option<&str>) -> CiWakeCondition {
        CiWakeCondition::CiCompleted {
            target: CiWakeTarget::Sha(FIXTURE_SHA.to_string()),
            workflow: workflow.map(str::to_string),
            event: None,
        }
    }

    fn transport_with(page: WorkflowRunPage) -> FakeTransport {
        let mut transport = FakeTransport::default();
        transport
            .runs
            .insert((FIXTURE_SHA.to_string(), None), page);
        transport
    }

    fn observe(transport: &FakeTransport, condition: &CiWakeCondition) -> Option<CiWakeObservation> {
        ci_wake_condition_observation(transport, Path::new("."), condition).unwrap()
    }

    /// The fixture as it looked while the push CI run was still going.
    fn with_ci_in_progress(mut page: WorkflowRunPage) -> WorkflowRunPage {
        for run in &mut page.workflow_runs {
            if run.name == "CI" && run.event == "push" {
                run.status = "in_progress".to_string();
                run.conclusion = None;
            }
        }
        page
    }

    #[test]
    fn parses_the_filter_contract_strictly() {
        let parse = |event: &str, filter: serde_json::Value| parse_ci_wake_condition(event, &filter);
        assert_eq!(
            parse(
                "ci_completed",
                serde_json::json!({"sha": "FBA7A3365", "workflow": "CI"})
            )
            .unwrap(),
            CiWakeCondition::CiCompleted {
                target: CiWakeTarget::Sha("fba7a3365".to_string()),
                workflow: Some("CI".to_string()),
                event: None,
            }
        );
        assert_eq!(
            parse(
                "ci_completed",
                serde_json::json!({"branch": "factory/kind-tiger-16", "event": "push"})
            )
            .unwrap(),
            CiWakeCondition::CiCompleted {
                target: CiWakeTarget::Branch("factory/kind-tiger-16".to_string()),
                workflow: None,
                event: Some("push".to_string()),
            }
        );
        assert_eq!(
            parse("ci_completed", serde_json::json!({"commit": FIXTURE_SHA})).unwrap(),
            sha_condition(None)
        );
        for bad in [
            serde_json::json!({}),
            serde_json::json!({"sha": "abc"}),
            serde_json::json!({"sha": "not-a-sha-at-all"}),
            serde_json::json!({"sha": FIXTURE_SHA, "branch": "main"}),
            serde_json::json!({"branch": "-main"}),
            serde_json::json!({"branch": "a b"}),
            serde_json::json!({"sha": FIXTURE_SHA, "workflow": 3}),
            serde_json::json!({"sha": FIXTURE_SHA, "event": "Push!"}),
        ] {
            assert!(parse("ci_completed", bad.clone()).is_err(), "{bad} accepted");
        }

        for pr in [
            serde_json::json!({"pr": 1193}),
            serde_json::json!({"pr": "1193"}),
            serde_json::json!({"pr": "#1193"}),
        ] {
            assert_eq!(
                parse("merge_queue_landed", pr).unwrap(),
                CiWakeCondition::MergeQueueLanded { pr: 1193 }
            );
        }
        for bad in [
            serde_json::json!({}),
            serde_json::json!({"pr": 0}),
            serde_json::json!({"pr": "x"}),
            serde_json::json!({"pr": -4}),
        ] {
            assert!(parse("merge_queue_landed", bad.clone()).is_err(), "{bad} accepted");
        }
        assert!(parse("tag_exists", serde_json::json!({"tag": "v1"})).is_err());
    }

    #[test]
    fn fixture_run_fires_only_once_every_commit_run_completed() {
        let condition = sha_condition(None);

        let running = transport_with(with_ci_in_progress(fixture_runs()));
        assert_eq!(observe(&running, &condition), None);

        let done = transport_with(fixture_runs());
        let observation = observe(&done, &condition).expect("all fixture runs completed");
        let CiWakeObservation::CiCompleted { sha, runs } = &observation else {
            panic!("wrong observation {observation:?}");
        };
        assert_eq!(sha, FIXTURE_SHA);
        // Scheduled watchdog runs on the same sha are not this commit's CI.
        assert!(runs.iter().all(|run| run.event != "schedule"));
        assert!(runs.iter().any(|run| run.name == "CI" && run.event == "push"));
        assert!(runs.iter().any(|run| run.event == "merge_group"));

        let json = observation.to_json(&condition);
        assert_eq!(json["outcome"], "success");
        assert_eq!(json["sha"], FIXTURE_SHA);
        assert!(
            observation
                .description()
                .starts_with("CI completed for fba7a3365eb0: all "),
            "{}",
            observation.description()
        );
    }

    #[test]
    fn workflow_filter_waits_for_that_workflow_only() {
        // Only CI is still running; the Release Prebuild workflow is done.
        let transport = transport_with(with_ci_in_progress(fixture_runs()));
        assert_eq!(observe(&transport, &sha_condition(Some("CI"))), None);
        let prebuild = observe(&transport, &sha_condition(Some("release-prebuild.yml")))
            .expect("Release Prebuild completed");
        let CiWakeObservation::CiCompleted { runs, .. } = prebuild else {
            panic!("wrong observation");
        };
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].name, "Release Prebuild");

        // A named scheduled workflow counts when asked for explicitly.
        let watchdog = observe(&transport, &sha_condition(Some("Merge queue watchdog")))
            .expect("watchdog runs completed");
        let CiWakeObservation::CiCompleted { runs, .. } = watchdog else {
            panic!("wrong observation");
        };
        assert!(runs.iter().all(|run| run.event == "schedule"));

        // An unknown workflow never fires.
        assert_eq!(observe(&transport, &sha_condition(Some("Nope"))), None);
    }

    #[test]
    fn failed_run_fires_with_a_failure_outcome() {
        let mut page = fixture_runs();
        let ci = page
            .workflow_runs
            .iter_mut()
            .find(|run| run.name == "CI" && run.event == "push")
            .unwrap();
        ci.conclusion = Some("failure".to_string());
        let condition = sha_condition(Some("CI"));
        let observation = observe(&transport_with(page), &condition).unwrap();
        assert_eq!(observation.to_json(&condition)["outcome"], "failure");
        assert!(observation.description().contains("did not pass — CI (push) failure https://github.com/Richards-LLC/cassy/actions/runs/"));
    }

    #[test]
    fn a_commit_without_runs_yet_stays_pending() {
        let transport = FakeTransport::default();
        assert_eq!(observe(&transport, &sha_condition(None)), None);
    }

    #[test]
    fn page_of_scheduled_runs_falls_back_to_commit_events() {
        let all = fixture_runs();
        let scheduled: Vec<WorkflowRun> = all
            .workflow_runs
            .iter()
            .filter(|run| run.event == "schedule")
            .cloned()
            .collect();
        let mut transport = transport_with(WorkflowRunPage {
            total_count: 457,
            workflow_runs: scheduled,
        });
        let push: Vec<WorkflowRun> = all
            .workflow_runs
            .iter()
            .filter(|run| run.event == "push")
            .cloned()
            .collect();
        transport.runs.insert(
            (FIXTURE_SHA.to_string(), Some("push".to_string())),
            WorkflowRunPage {
                total_count: push.len() as u64,
                workflow_runs: push,
            },
        );
        let observation = observe(&transport, &sha_condition(None)).expect("push runs found");
        let CiWakeObservation::CiCompleted { runs, .. } = observation else {
            panic!("wrong observation");
        };
        assert!(runs.iter().all(|run| run.event == "push"));
        assert_eq!(
            transport.calls.borrow().as_slice(),
            [
                format!("runs {FIXTURE_SHA} *"),
                format!("runs {FIXTURE_SHA} push"),
                format!("runs {FIXTURE_SHA} pull_request"),
                format!("runs {FIXTURE_SHA} merge_group"),
            ]
        );
    }

    #[test]
    fn branch_target_follows_the_forge_tip() {
        let mut transport = transport_with(fixture_runs());
        let condition = CiWakeCondition::CiCompleted {
            target: CiWakeTarget::Branch("main".to_string()),
            workflow: Some("CI".to_string()),
            event: Some("push".to_string()),
        };
        // Unknown tip: pending, not an error.
        assert_eq!(observe(&transport, &condition), None);
        transport.tips.insert("main".to_string(), FIXTURE_SHA.to_string());
        transport.runs.insert(
            (FIXTURE_SHA.to_string(), Some("push".to_string())),
            fixture_runs(),
        );
        let observation = observe(&transport, &condition).expect("main push CI completed");
        let CiWakeObservation::CiCompleted { sha, runs } = observation else {
            panic!("wrong observation");
        };
        assert_eq!(sha, FIXTURE_SHA);
        assert!(runs.iter().all(|run| run.name == "CI" && run.event == "push"));
    }

    #[test]
    fn merge_queue_landed_fires_on_the_fixture_merge_and_on_close() {
        let landed: PullRequestState = serde_json::from_str(PULL_FIXTURE).unwrap();
        let mut transport = FakeTransport::default();
        transport.pulls.insert(1193, landed.clone());
        let condition = CiWakeCondition::MergeQueueLanded { pr: 1193 };
        let observation = observe(&transport, &condition).expect("PR #1193 merged");
        assert_eq!(
            observation.description(),
            "PR #1193 landed as fba7a3365eb0cb6403be9d708e8f03151fc42054 https://github.com/Richards-LLC/cassy/pull/1193"
        );
        assert_eq!(observation.to_json(&condition)["merged"], true);

        let mut open = landed.clone();
        open.state = "open".to_string();
        open.merged_at = None;
        open.merge_commit_sha = Some("test-merge-commit".to_string());
        transport.pulls.insert(1193, open.clone());
        assert_eq!(observe(&transport, &condition), None);

        let mut closed = open;
        closed.state = "closed".to_string();
        transport.pulls.insert(1193, closed);
        let observation = observe(&transport, &condition).expect("closed PR wakes");
        assert!(observation.description().contains("closed without merging"));
        assert_eq!(observation.to_json(&condition)["merge_commit"], serde_json::Value::Null);

        // A transport error is surfaced, so the daemon keeps the row pending.
        assert!(
            ci_wake_condition_observation(
                &transport,
                Path::new("."),
                &CiWakeCondition::MergeQueueLanded { pr: 7 }
            )
            .is_err()
        );
    }

    fn reminder(id: i64) -> cas_store::Reminder {
        cas_store::Reminder {
            id,
            owner_id: "supervisor-1".to_string(),
            target_id: "supervisor-1".to_string(),
            message: "CI done; merge".to_string(),
            trigger_type: cas_store::ReminderTriggerType::Event,
            trigger_at: None,
            trigger_event: Some(EXTERNAL_CI_COMPLETED_EVENT.to_string()),
            trigger_filter: Some(sha_condition(None).to_json()),
            status: cas_store::ReminderStatus::Pending,
            ttl_secs: 0,
            created_at: chrono::Utc::now(),
            fired_at: None,
            cancelled_at: None,
            fired_event: None,
            session_id: Some("factory-session".to_string()),
            origin_session_id: None,
            cross_session: true,
            task_id: None,
        }
    }

    #[test]
    fn identical_conditions_share_one_github_read() {
        let transport = transport_with(fixture_runs());
        let candidates = (1..=3)
            .map(|id| CiWakeCandidate {
                reminder: reminder(id),
                condition: sha_condition(None),
            })
            .collect();
        let ready = collect_ci_wake_ready(&transport, Path::new("."), candidates);
        assert_eq!(ready.iter().map(|r| r.reminder.id).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(transport.calls.borrow().len(), 1);
    }

    /// Acceptance 1: drive the probe on the daemon's 2 s refresh tick over a
    /// fixture run that completes at an arbitrary moment, and require the
    /// ready reminder within 60 s of completion for every phase offset.
    #[test]
    fn completion_is_delivered_within_sixty_seconds_of_the_run_completing() {
        const TICK: Duration = Duration::from_secs(2);
        // Worst case for a branch target: the tip read and the run list both
        // run to their full 8 s gh timeout.
        const PROBE_DURATION: Duration = Duration::from_secs(16);

        for completes_after in (0..CI_WAKE_INTERVAL.as_secs() + 4).map(Duration::from_secs) {
            let base = Instant::now();
            let completed_at = base + Duration::from_secs(45) + completes_after;
            let mut probe = CiWakeProbe::default();
            // A probe launched at `launched` reads the remote state as of
            // `launched` and its result reaches the daemon PROBE_DURATION
            // later; the daemon fires on the first tick after that.
            let mut in_flight: Option<(Instant, Vec<CiWakeReady>)> = None;
            let mut delivered_at = None;
            let mut now = base;
            while now < completed_at + Duration::from_secs(120) {
                if let Some((done_at, ready)) = in_flight.take() {
                    if now < done_at {
                        in_flight = Some((done_at, ready));
                    } else if !ready.is_empty() {
                        delivered_at = Some(now);
                        break;
                    }
                }
                if in_flight.is_none() && probe.due(now) {
                    let page = if now >= completed_at {
                        fixture_runs()
                    } else {
                        with_ci_in_progress(fixture_runs())
                    };
                    let ready = collect_ci_wake_ready(
                        &transport_with(page),
                        Path::new("."),
                        vec![CiWakeCandidate {
                            reminder: reminder(1),
                            condition: sha_condition(Some("CI")),
                        }],
                    );
                    probe.start(now, None::<fn() -> Vec<CiWakeReady>>);
                    in_flight = Some((now + PROBE_DURATION, ready));
                }
                now += TICK;
            }
            let delivered_at = delivered_at.expect("completion was never delivered");
            assert!(delivered_at >= completed_at);
            let latency = delivered_at - completed_at;
            assert!(
                latency <= CI_WAKE_MAX_LATENCY,
                "completion at +{completes_after:?} delivered after {latency:?}"
            );
        }
    }

    #[test]
    fn probe_runs_off_thread_and_respects_its_cadence() {
        let mut probe = CiWakeProbe::default();
        let now = Instant::now();
        assert!(probe.due(now));
        // Nothing pending: the scan is recorded, no thread starts.
        probe.start(now, None::<fn() -> Vec<CiWakeReady>>);
        assert!(!probe.due(now + CI_WAKE_INTERVAL - Duration::from_secs(1)));
        assert!(probe.due(now + CI_WAKE_INTERVAL));

        let gate = Arc::new(Mutex::new(()));
        let held = gate.lock().unwrap();
        let job_gate = Arc::clone(&gate);
        let later = now + CI_WAKE_INTERVAL;
        probe.start(
            later,
            Some(move || {
                let _wait = job_gate.lock().unwrap();
                vec![CiWakeReady {
                    reminder: reminder(9),
                    condition: CiWakeCondition::MergeQueueLanded { pr: 1 },
                    observation: CiWakeObservation::PullRequest {
                        pr: 1,
                        merged: true,
                        merge_commit: None,
                        url: String::new(),
                    },
                }]
            }),
        );
        // While the probe is blocked the daemon tick returns immediately and
        // never starts a second probe.
        assert!(probe.take_finished().is_empty());
        assert!(!probe.due(later + CI_WAKE_INTERVAL * 2));
        drop(held);
        let deadline = Instant::now() + Duration::from_secs(10);
        let ready = loop {
            let ready = probe.take_finished();
            if !ready.is_empty() || Instant::now() > deadline {
                break ready;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].reminder.id, 9);
        assert!(probe.due(later + CI_WAKE_INTERVAL));
    }
}
