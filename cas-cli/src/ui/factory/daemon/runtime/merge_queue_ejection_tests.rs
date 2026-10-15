//! cas-fd0a: the ejection wake names the failing job and tracks epic PRs; it
//! never re-enqueues.

use super::*;
use crate::ui::factory::daemon::runtime::ci_watch::{
    CiRun, CiWatchError, DeliveryPullRequest, FailedJob, MergeQueuePullRequest,
};

/// A transport whose failed run has `job` (and optionally `step`), or whose
/// job lookup errors when `job` is `None`.
struct Fake {
    job: Option<&'static str>,
    step: Option<&'static str>,
}

impl CiTransport for Fake {
    fn completed_runs(&self) -> Result<Vec<CiRun>, CiWatchError> {
        Ok(Vec::new())
    }
    fn failing_job(&self, _: u64) -> Result<Option<String>, CiWatchError> {
        Ok(self.job.map(str::to_string))
    }
    fn failed_job_detail(&self, _: u64) -> Result<Option<FailedJob>, CiWatchError> {
        match self.job {
            Some(name) => Ok(Some(FailedJob {
                name: name.to_string(),
                step: self.step.map(str::to_string),
            })),
            None => Err(CiWatchError::Unavailable("gh is offline".to_string())),
        }
    }
    fn failed_log(&self, _: u64) -> Result<Option<String>, CiWatchError> {
        Ok(None)
    }
    fn merge_queue_pull_requests(&self) -> Result<Vec<MergeQueuePullRequest>, CiWatchError> {
        Ok(Vec::new())
    }
    fn delivery_pull_requests(&self, _: &str) -> Result<Vec<DeliveryPullRequest>, CiWatchError> {
        Ok(Vec::new())
    }
}

#[test]
fn failing_job_names_the_job_and_its_failed_step() {
    let with_step = Fake {
        job: Some("Fast Validation — CI tiers"),
        step: Some("Test release publication guards"),
    };
    assert_eq!(
        failing_job(&with_step, Some(38647056637)).as_deref(),
        Some("Fast Validation — CI tiers › Test release publication guards")
    );
    let job_only = Fake {
        job: Some("macOS Check"),
        step: None,
    };
    assert_eq!(
        failing_job(&job_only, Some(1)).as_deref(),
        Some("macOS Check")
    );
}

#[test]
fn failing_job_degrades_to_none_without_a_run_or_a_readable_job() {
    let job = Fake {
        job: Some("macOS Check"),
        step: None,
    };
    assert_eq!(
        failing_job(&job, None),
        None,
        "a dequeue with no failed run names no job"
    );
    let offline = Fake {
        job: None,
        step: None,
    };
    assert_eq!(
        failing_job(&offline, Some(1)),
        None,
        "a lookup error never drops the wake"
    );
}

#[test]
fn supervisor_prs_track_open_epics_on_epic_branches_only() {
    let temp = tempfile::TempDir::new().unwrap();
    let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
    let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
    for (id, task_type, status, branch) in [
        (
            "cas-epic",
            cas_types::TaskType::Epic,
            cas_types::TaskStatus::InProgress,
            Some("epic/release"),
        ),
        (
            "cas-done",
            cas_types::TaskType::Epic,
            cas_types::TaskStatus::Closed,
            Some("epic/old"),
        ),
        (
            "cas-plain",
            cas_types::TaskType::Epic,
            cas_types::TaskStatus::Open,
            Some("main"),
        ),
        (
            "cas-task",
            cas_types::TaskType::Task,
            cas_types::TaskStatus::Open,
            Some("epic/not-an-epic"),
        ),
    ] {
        let mut task = cas_types::Task::new(id.to_string(), id.to_string());
        task.task_type = task_type;
        task.status = status;
        task.branch = branch.map(str::to_string);
        store.add(&task).unwrap();
    }
    assert_eq!(
        supervisor_prs(&cas_dir),
        vec![SupervisorPr {
            task_id: "cas-epic".to_string(),
            branch: "epic/release".to_string()
        }]
    );
}
