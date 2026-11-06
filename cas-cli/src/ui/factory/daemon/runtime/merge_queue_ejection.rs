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

/// cas-f2ea: failed-run lookups the CI watcher keeps across polls, so a
/// repeat ejection reuses its job and log instead of downloading them every
/// poll. Only successful lookups are kept; an error retries next poll.
#[derive(Clone, Default)]
pub(crate) struct FailedRunLookups {
    jobs: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<u64, super::ci_watch::FailedJob>>>,
    named: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<u64, String>>>,
}

impl FailedRunLookups {
    /// Runs remembered before the oldest are forgotten.
    const CAPACITY: usize = 256;

    fn remember<V: Clone>(
        map: &std::sync::Mutex<std::collections::HashMap<u64, V>>,
        run_id: u64,
        lookup: impl FnOnce() -> Option<V>,
    ) -> Option<V> {
        if let Some(value) = map.lock().ok()?.get(&run_id).cloned() {
            return Some(value);
        }
        let value = lookup()?;
        if let Ok(mut map) = map.lock() {
            if map.len() >= Self::CAPACITY {
                let oldest = *map.keys().min().expect("non-empty");
                map.remove(&oldest);
            }
            map.insert(run_id, value.clone());
        }
        Some(value)
    }

    fn job(&self, transport: &dyn CiTransport, run_id: u64) -> Option<super::ci_watch::FailedJob> {
        Self::remember(&self.jobs, run_id, || transport.failed_job_detail(run_id).ok().flatten())
    }

    /// "job › step" of the failed merge_group run, when GitHub names one, plus
    /// the failing tests its log names (cas-25ce: they become the worker's
    /// change request). Lookup errors degrade to `None`; the wake is sent
    /// either way.
    pub(crate) fn failing_job(&self, transport: &dyn CiTransport, run_id: Option<u64>) -> Option<String> {
        /// Enough names to batch the repair; the run log holds the rest.
        const TESTS: usize = 10;
        let run_id = run_id?;
        Self::remember(&self.named, run_id, || {
            let job = self.job(transport, run_id)?;
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
        })
    }

    /// cas-25ce/cas-f2ea: whether the merge-group entry ahead may have caused
    /// this failure, so it must not spend the task's repair cycle: it failed
    /// the same job, or it is still running or unreadable. Only an entry
    /// ahead that is clear, or that failed a different job, lets it count.
    pub(crate) fn ahead_may_be_cause(
        &self,
        transport: &dyn CiTransport,
        run_id: Option<u64>,
        ahead: super::ci_watch::AheadEntry,
    ) -> bool {
        use super::ci_watch::AheadEntry;
        let (Some(run_id), AheadEntry::Failed(ahead_run_id)) = (run_id, ahead) else {
            return ahead == AheadEntry::Unknown;
        };
        match (self.job(transport, run_id), self.job(transport, ahead_run_id)) {
            (Some(own), Some(ahead)) => own.name == ahead.name,
            _ => true,
        }
    }
}

#[cfg(test)]
#[path = "merge_queue_ejection_tests.rs"]
mod tests;
