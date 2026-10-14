//! cas-e369: diagnose a merge-queue ejection and re-enqueue it once when the
//! failure is outside the PR's diff.
//!
//! The CI watcher (cas-0f05) already notices a tracked PR leaving the queue.
//! For each fresh ejection this module names the failing job, step and error
//! line and decides whether the failure touches the PR's diff:
//!
//! - **outside the diff**: an infrastructure stop (a memory-admission stop,
//!   a lost or shut-down runner, a full disk), or a Commander step failure on a
//!   PR that changes no Commander input. The PR is re-enqueued once,
//!   bound to the ejected head.
//! - **touches the diff / unclassified**: the supervisor decides.
//!
//! A second failure on the same PR head after the automatic re-enqueue
//! escalates instead of looping. Each decision is one marked line in the
//! epic's notes, keyed by the failed `merge_group` run. That line is also the
//! durable ledger, so repeated polls and daemon restarts never act twice.

use std::path::Path;

use super::ci_watch::{CiTransport, MergeQueueEjection, SupervisorPr, first_failing_test};

/// Paths a Commander (hub-web) journey or unit failure can be caused by: the
/// app, the journey catalog it implements, its design tokens and the visual-QA
/// contract it imports. Each is a `WEB_INPUTS` entry in
/// `scripts/classify-web-inputs.py`, which a test pins. The classifier's
/// wrapper and workflow entries decide whether the step runs, not how a
/// journey behaves, so they are not listed.
pub(crate) const COMMANDER_PRODUCT_INPUTS: &[&str] = &[
    "hub-web/",
    "docs/qa/journeys.md",
    "docs/design/design-tokens.json",
    "scripts/visual-qa.mjs",
];

/// Log messages that mean the runner or host stopped the job, not its code.
/// Routine `host memory admission: {...}` status lines appear in every run,
/// so only the stop messages are listed.
const INFRASTRUCTURE_STOPS: &[&str] = &[
    "reached assembly memory headroom",
    "admission deadline expired",
    "received a shutdown signal",
    "lost communication with the server",
    "no space left on device",
];

/// The memory-admission policy itself. A PR that edits it owns a stop it causes.
const MEMORY_ADMISSION_POLICY: &[&str] = &["scripts/host_memory.py", "scripts/worker-memory.py"];

const MARKER: &str = "merge-queue-recovery";

/// A failed run older than this is history, not the cause of this exit (an
/// old red run on a PR dequeued by hand, or one from before this watcher).
pub(crate) const RECENT_FAILURE_WINDOW: chrono::Duration = chrono::Duration::minutes(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureScope {
    OutsideDiff,
    TouchesDiff,
    Unclassified,
}

impl FailureScope {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::OutsideDiff => "outside-diff",
            Self::TouchesDiff => "touches-diff",
            Self::Unclassified => "unclassified",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EjectionDiagnosis {
    pub failing_job: Option<String>,
    pub failing_step: Option<String>,
    pub error_line: Option<String>,
    pub scope: FailureScope,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecoveryDecision {
    /// Re-enqueued once, automatically.
    AutoRequeued,
    /// A second failure on this head after the automatic re-enqueue.
    Escalated { reason: String },
    /// Eligible, but the re-enqueue call failed; the supervisor requeues.
    RequeueFailed { error: String },
    /// The failure may touch the diff, or there is no failed run to read.
    SupervisorDecides { reason: String },
    /// This failed run was already decided; nothing is done again.
    AlreadyDecided { decision: String },
}

impl RecoveryDecision {
    pub(crate) fn token(&self) -> &str {
        match self {
            Self::AutoRequeued => "auto-requeued",
            Self::Escalated { .. } => "escalated",
            Self::RequeueFailed { .. } => "requeue-failed",
            Self::SupervisorDecides { .. } => "supervisor-decides",
            Self::AlreadyDecided { decision } => decision,
        }
    }

    pub(crate) fn sentence(&self) -> String {
        match self {
            Self::AutoRequeued => "Re-enqueued automatically once (the failure is outside the PR's diff); a second failure on this head escalates.".to_string(),
            Self::Escalated { reason } => format!("Escalated, not re-enqueued: {reason}. Diagnose before requeueing."),
            Self::RequeueFailed { error } => format!("Eligible for one automatic re-enqueue, but it failed: {error}. Requeue by hand."),
            Self::SupervisorDecides { reason } => format!("Not re-enqueued automatically: {reason}. Inspect the failed run and requeue or request changes."),
            Self::AlreadyDecided { decision } => format!("This failed run was already handled ({decision}); nothing was re-enqueued. Inspect the PR before requeueing."),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EjectionReport {
    pub diagnosis: Option<EjectionDiagnosis>,
    pub decision: RecoveryDecision,
}

impl EjectionReport {
    /// Lines appended to the supervisor wake under the ejection sentence.
    pub(crate) fn detail(&self) -> String {
        let mut lines = Vec::new();
        if let Some(diagnosis) = &self.diagnosis {
            let job = diagnosis.failing_job.as_deref().unwrap_or("unknown");
            match &diagnosis.failing_step {
                Some(step) => lines.push(format!("Failing job: {job} (step: {step})")),
                None => lines.push(format!("Failing job: {job}")),
            }
            if let Some(error) = &diagnosis.error_line {
                lines.push(format!("Error: {error}"));
            }
            lines.push(format!(
                "Failure scope: {} ({})",
                diagnosis.scope.as_str(),
                diagnosis.reason
            ));
        }
        lines.push(format!("Recovery: {}", self.decision.sentence()));
        lines.join("\n")
    }

    pub(crate) fn payload(&self) -> serde_json::Value {
        let diagnosis = self.diagnosis.as_ref();
        serde_json::json!({
            "failing_job": diagnosis.and_then(|d| d.failing_job.clone()),
            "failing_step": diagnosis.and_then(|d| d.failing_step.clone()),
            "error_line": diagnosis.and_then(|d| d.error_line.clone()),
            "failure_scope": diagnosis.map(|d| d.scope.as_str()),
            "recovery": self.decision.token(),
        })
    }
}

/// Open epics whose `epic/` branch the supervisor queues as a PR.
pub(crate) fn supervisor_prs(cas_dir: &Path) -> Vec<SupervisorPr> {
    use cas_types::TaskStatus;
    let Ok(store) = crate::store::open_task_store_cached(cas_dir) else {
        return Vec::new();
    };
    [
        TaskStatus::Open,
        TaskStatus::InProgress,
        TaskStatus::Blocked,
        TaskStatus::AwaitingMerge,
    ]
    .into_iter()
    .flat_map(|status| store.list(Some(status)).unwrap_or_default())
    .filter(|task| task.task_type == cas_types::TaskType::Epic)
    .filter_map(|task| {
        let branch = task.branch.filter(|branch| branch.starts_with("epic/"))?;
        Some(SupervisorPr {
            task_id: task.id,
            branch,
        })
    })
    .collect()
}

/// The message part of a `gh run view --log-failed` line
/// (`<job>\t<step>\t<timestamp> <message>`).
fn log_message(line: &str) -> &str {
    let message = line.rsplit('\t').next().unwrap_or(line);
    match message.split_once(' ') {
        Some((stamp, rest)) if stamp.len() >= 20 && stamp.ends_with('Z') && stamp.contains('T') => {
            rest.trim()
        }
        _ => message.trim(),
    }
}

fn infrastructure_stop(log: &str) -> Option<String> {
    log.lines().map(log_message).find_map(|message| {
        let lower = message.to_ascii_lowercase();
        INFRASTRUCTURE_STOPS
            .iter()
            .any(|stop| lower.contains(stop))
            .then(|| message.to_string())
    })
}

/// The line that says what failed: an infrastructure stop, a failed Rust
/// test, a failed Playwright test (`✘`), or else the last `##[error]` line.
pub(crate) fn error_line(log: &str) -> Option<String> {
    if let Some(stop) = infrastructure_stop(log) {
        return Some(stop);
    }
    let messages: Vec<&str> = log.lines().map(log_message).collect();
    if let Some(test) = first_failing_test(&messages.join("\n")) {
        return Some(format!("test {test} FAILED"));
    }
    if let Some(failed) = messages.iter().find_map(|m| m.strip_prefix('✘')) {
        return Some(format!("✘ {}", failed.trim()));
    }
    messages
        .iter()
        .rev()
        .find_map(|m| m.strip_prefix("##[error]"))
        .map(|m| m.trim().to_string())
}

fn is_commander_step(job: Option<&str>, step: Option<&str>) -> bool {
    [job, step]
        .into_iter()
        .flatten()
        .any(|name| name.contains("Commander"))
}

fn touched<'a>(files: &'a [String], inputs: &[&str]) -> Option<&'a String> {
    files.iter().find(|path| {
        inputs.iter().any(|input| {
            path.as_str() == *input || (input.ends_with('/') && path.starts_with(input))
        })
    })
}

/// Decide whether a failure is outside the PR's diff. `files` is `None` when
/// the diff could not be read; that is never outside.
pub(crate) fn classify(
    job: Option<&str>,
    step: Option<&str>,
    log: Option<&str>,
    files: Option<&[String]>,
) -> (FailureScope, String) {
    if let Some(stop) = log.and_then(infrastructure_stop) {
        if let Some(path) = files.and_then(|files| touched(files, MEMORY_ADMISSION_POLICY))
            && stop.to_ascii_lowercase().contains("memory")
        {
            return (
                FailureScope::TouchesDiff,
                format!("a memory-admission stop, and the PR changes {path}"),
            );
        }
        return (
            FailureScope::OutsideDiff,
            format!("infrastructure stop: {stop}"),
        );
    }
    if is_commander_step(job, step) {
        return match files {
            None => (
                FailureScope::Unclassified,
                "a Commander step failed and the PR's diff could not be read".to_string(),
            ),
            Some(files) => match touched(files, COMMANDER_PRODUCT_INPUTS) {
                Some(path) => (
                    FailureScope::TouchesDiff,
                    format!("a Commander step failed and the PR changes {path}"),
                ),
                None => (
                    FailureScope::OutsideDiff,
                    "a Commander step failed and the PR changes no Commander input".to_string(),
                ),
            },
        };
    }
    (
        FailureScope::Unclassified,
        "the failure is not a known infrastructure stop or Commander step".to_string(),
    )
}

/// Read the failed run and the PR's diff. GitHub errors degrade to an
/// unclassified diagnosis rather than dropping the wake.
pub(crate) fn diagnose(
    transport: &dyn CiTransport,
    pr_number: u64,
    run_id: u64,
) -> EjectionDiagnosis {
    let job = transport.failed_job_detail(run_id).ok().flatten();
    let log = transport.failed_log(run_id).ok().flatten();
    let files = transport.pull_request_files(pr_number).ok().flatten();
    let failing_job = job.as_ref().map(|job| job.name.clone());
    let failing_step = job.and_then(|job| job.step);
    let (scope, reason) = classify(
        failing_job.as_deref(),
        failing_step.as_deref(),
        log.as_deref(),
        files.as_deref(),
    );
    EjectionDiagnosis {
        failing_job,
        failing_step,
        error_line: log.as_deref().and_then(error_line),
        scope,
        reason,
    }
}

fn failed_recently(completed_at: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> bool {
    completed_at
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .is_some_and(|at| now.signed_duration_since(at) <= RECENT_FAILURE_WINDOW)
}

/// `(run, decision)` pairs already recorded for this PR head.
fn recorded(notes: &str, pr_number: u64, head_sha: &str) -> Vec<(u64, String)> {
    let prefix = format!("{MARKER} pr=#{pr_number} head={head_sha} run=");
    notes
        .match_indices(&prefix)
        .filter_map(|(at, _)| {
            let rest = &notes[at + prefix.len()..];
            let (run, rest) = rest.split_once(' ')?;
            let decision = rest.strip_prefix("decision=")?;
            let decision: String = decision
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            Some((run.parse().ok()?, decision))
        })
        .collect()
}

/// The task whose notes hold the decision: the epic itself, a delivery's
/// parent epic, or the delivery when it has none.
fn ledger_task(store: &dyn cas_store::TaskStore, task_id: &str) -> Option<cas_types::Task> {
    let task = store.get(task_id).ok()?;
    if task.task_type == cas_types::TaskType::Epic {
        return Some(task);
    }
    Some(
        store
            .get_parent_epic(task_id)
            .ok()
            .flatten()
            .unwrap_or(task),
    )
}

/// Diagnose one ejection, re-enqueue it once when eligible, and record the
/// decision in the epic's notes.
///
/// A freshly decided ejection is keyed by its failed run
/// (`merge-group-run:<id>`), so the poll after the exit, which no longer saw
/// the PR queued, replays the same wake instead of sending a second one. An
/// exit explained only by an already-decided run (an admin dequeue after a
/// requeue) keeps its `queue-exit:` key and wakes again.
pub(crate) fn recover(
    cas_dir: &Path,
    transport: &dyn CiTransport,
    ejection: &mut MergeQueueEjection,
    now: chrono::DateTime<chrono::Utc>,
) -> EjectionReport {
    let Some(run_id) = ejection.failed_run_id else {
        return EjectionReport {
            diagnosis: None,
            decision: RecoveryDecision::SupervisorDecides {
                reason: "no failed merge_group run (dequeued by hand or auto-merge disarmed)"
                    .to_string(),
            },
        };
    };
    let store = crate::store::open_task_store_cached(cas_dir).ok();
    let ledger = store
        .as_deref()
        .and_then(|store| ledger_task(store, &ejection.task_id));
    let prior = ledger
        .as_ref()
        .map(|task| recorded(&task.notes, ejection.pr_number, &ejection.head_sha))
        .unwrap_or_default();
    if let Some((_, decision)) = prior.iter().find(|(run, _)| *run == run_id) {
        return EjectionReport {
            diagnosis: None,
            decision: RecoveryDecision::AlreadyDecided {
                decision: decision.clone(),
            },
        };
    }

    ejection.occurrence = format!("merge-group-run:{run_id}");
    let diagnosis = diagnose(transport, ejection.pr_number, run_id);
    let decision = if let Some((earlier, _)) = prior
        .iter()
        .find(|(_, decision)| decision == "auto-requeued")
    {
        RecoveryDecision::Escalated {
            reason: format!(
                "second consecutive merge-queue failure on head {} after the automatic re-enqueue that followed run {earlier}",
                ejection.head_sha
            ),
        }
    } else if diagnosis.scope != FailureScope::OutsideDiff {
        RecoveryDecision::SupervisorDecides {
            reason: format!("{} ({})", diagnosis.reason, diagnosis.scope.as_str()),
        }
    } else if !failed_recently(ejection.failed_run_completed_at.as_deref(), now) {
        RecoveryDecision::SupervisorDecides {
            reason: format!(
                "failed run {run_id} finished more than {} minutes ago, so it may not be why the PR left the queue",
                RECENT_FAILURE_WINDOW.num_minutes()
            ),
        }
    } else if ledger.is_none() {
        // Without the ledger a second failure could not be recognised.
        RecoveryDecision::RequeueFailed {
            error: format!(
                "task {} is unreadable, so the one-retry ledger is unavailable",
                ejection.task_id
            ),
        }
    } else if ejection.pr_node_id.is_empty() || ejection.head_sha.is_empty() {
        RecoveryDecision::RequeueFailed {
            error: "the PR's node ID or head is unknown".to_string(),
        }
    } else {
        match transport.enqueue_pull_request(&ejection.pr_node_id, &ejection.head_sha) {
            Ok(()) => RecoveryDecision::AutoRequeued,
            Err(error) => RecoveryDecision::RequeueFailed {
                error: error.to_string(),
            },
        }
    };

    if let (Some(store), Some(task)) = (store, ledger) {
        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M");
        let job = diagnosis.failing_job.as_deref().unwrap_or("unknown job");
        let error = diagnosis.error_line.as_deref().unwrap_or("no error line");
        let note = format!(
            "[{timestamp}] ✅ DECISION merge queue ejected PR #{pr} ({task_id}) on failed run {run_id}: {job}: {error}. {scope}: {reason}. {sentence} ({MARKER} pr=#{pr} head={head} run={run_id} decision={token})",
            pr = ejection.pr_number,
            task_id = ejection.task_id,
            scope = diagnosis.scope.as_str(),
            reason = diagnosis.reason,
            sentence = decision.sentence(),
            head = ejection.head_sha,
            token = decision.token(),
        );
        if let Err(error) = store.append_note(&task.id, &note) {
            tracing::warn!(task_id = %task.id, %error, "merge-queue recovery note write failed");
        }
    }
    EjectionReport {
        diagnosis: Some(diagnosis),
        decision,
    }
}

#[cfg(test)]
#[path = "merge_queue_recovery_tests.rs"]
mod tests;
