//! cas-fd0a: the merge-queue ejection wake. The CI watcher (cas-0f05) sees a
//! tracked PR leave the queue; the supervisor gets one typed wake naming the
//! PR and the failing job. cas-e369's diagnosis and automatic re-enqueue are
//! retired: a person decides what an ejection means (rule-176 (5)).

use std::path::Path;

use super::ci_watch::{AwaitingMergeDelivery, CiTransport, SupervisorPr};

/// Parked deliveries to `main` whose PR GitHub has not yet been seen merging.
/// cas-c24e: a recorded merge is final, and in batch QA a merged delivery
/// stays AwaitingMerge until the release verdict; polling its PR every tick
/// was the factory's largest GitHub GraphQL cost.
pub(crate) fn awaiting_merge_deliveries(cas_dir: &Path) -> Vec<AwaitingMergeDelivery> {
    crate::store::open_task_store_cached(cas_dir)
        .ok()
        .and_then(|store| store.list(Some(cas_types::TaskStatus::AwaitingMerge)).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|task| {
            task.deliverables.delivery_pr_merge_commit.is_none()
                || task.deliverables.delivery_pr_merged_at.is_none()
        })
        .filter_map(|task| {
            let worker = task.assignee?;
            let work_target = task.deliverables.work_target.as_ref()?;
            if work_target.target_branch != "main" {
                return None;
            }
            let branch = task
                .deliverables
                .parked_branch
                .or(task.branch)
                .unwrap_or_else(|| format!("factory/{worker}"));
            Some(AwaitingMergeDelivery {
                task_id: task.id,
                worker,
                branch,
                branch_tip: task.deliverables.factory_branch_anchor,
                pr_number: task.deliverables.delivery_pr_number,
            })
        })
        .collect()
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
