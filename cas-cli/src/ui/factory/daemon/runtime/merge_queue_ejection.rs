//! cas-fd0a: the merge-queue ejection wake. The CI watcher (cas-0f05) sees a
//! tracked PR leave the queue; the supervisor gets one typed wake naming the
//! PR and the failing job. cas-e369's diagnosis and automatic re-enqueue are
//! retired: a person decides what an ejection means (rule-176 (5)).

use std::path::Path;

use super::ci_watch::{CiTransport, SupervisorPr};

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

/// "job › step" of the failed merge_group run, when GitHub names one, plus
/// the failing tests its log names (cas-25ce: they become the worker's change
/// request). Lookup errors degrade to `None`; the wake is sent either way.
pub(crate) fn failing_job(transport: &dyn CiTransport, run_id: Option<u64>) -> Option<String> {
    /// Enough names to batch the repair; the run log holds the rest.
    const TESTS: usize = 10;
    let run_id = run_id?;
    let job = transport.failed_job_detail(run_id).ok().flatten()?;
    let mut named = match job.step {
        Some(step) => format!("{} › {step}", job.name),
        None => job.name,
    };
    let log = transport.failed_log(run_id).ok().flatten().unwrap_or_default();
    let tests: Vec<_> = super::ci_watch::failing_tests(&log).take(TESTS).collect();
    if !tests.is_empty() {
        named.push_str(&format!(" (failing tests: {})", tests.join(", ")));
    }
    Some(named)
}

/// cas-25ce: the group entry this run was built on failed the same job, so
/// the failure may be a group-mate's and must not spend this task's repair
/// cycle. Lookup errors count as unproven (false).
pub(crate) fn ahead_failed_same_job(
    transport: &dyn CiTransport,
    run_id: Option<u64>,
    ahead_run_id: Option<u64>,
) -> bool {
    let (Some(run_id), Some(ahead_run_id)) = (run_id, ahead_run_id) else {
        return false;
    };
    let job = |run| transport.failed_job_detail(run).ok().flatten().map(|job| job.name);
    job(run_id).is_some_and(|own| job(ahead_run_id).as_deref() == Some(own.as_str()))
}

#[cfg(test)]
#[path = "merge_queue_ejection_tests.rs"]
mod tests;
