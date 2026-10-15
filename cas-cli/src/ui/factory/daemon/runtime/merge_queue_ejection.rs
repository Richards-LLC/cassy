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

/// "job › step" of the failed merge_group run, when GitHub names one.
/// Lookup errors degrade to `None`; the wake is sent either way.
pub(crate) fn failing_job(transport: &dyn CiTransport, run_id: Option<u64>) -> Option<String> {
    let job = transport.failed_job_detail(run_id?).ok().flatten()?;
    Some(match job.step {
        Some(step) => format!("{} › {step}", job.name),
        None => job.name,
    })
}

#[cfg(test)]
#[path = "merge_queue_ejection_tests.rs"]
mod tests;
