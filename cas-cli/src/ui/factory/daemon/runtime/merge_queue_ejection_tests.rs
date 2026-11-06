//! cas-fd0a: the ejection wake names the failing job and tracks epic PRs; it
//! never re-enqueues.

use super::*;
use crate::ui::factory::daemon::runtime::ci_watch::{
    CiRun, CiWatchError, DeliveryPullRequest, FailedJob, MergeQueuePullRequest,
};

/// A transport whose failed run has `job` (and optionally `step`), or whose
/// job lookup errors when `job` is `None`. `ahead` names run 2's job.
#[derive(Default)]
struct Fake {
    job: Option<&'static str>,
    step: Option<&'static str>,
    log: Option<&'static str>,
    ahead: Option<&'static str>,
    log_reads: std::cell::Cell<u32>,
}

fn failing_job(transport: &Fake, run_id: Option<u64>) -> Option<String> {
    FailedRunLookups::default().failing_job(transport, run_id)
}

impl CiTransport for Fake {
    fn completed_runs(&self) -> Result<Vec<CiRun>, CiWatchError> {
        Ok(Vec::new())
    }
    fn failing_job(&self, _: u64) -> Result<Option<String>, CiWatchError> {
        Ok(self.job.map(str::to_string))
    }
    fn failed_job_detail(&self, run_id: u64) -> Result<Option<FailedJob>, CiWatchError> {
        match if run_id == 2 { self.ahead } else { self.job } {
            Some(name) => Ok(Some(FailedJob {
                name: name.to_string(),
                step: self.step.map(str::to_string),
            })),
            None => Err(CiWatchError::Unavailable("gh is offline".to_string())),
        }
    }
    fn failed_log(&self, _: u64) -> Result<Option<String>, CiWatchError> {
        self.log_reads.set(self.log_reads.get() + 1);
        Ok(self.log.map(str::to_string))
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
        log: None,
        ..Fake::default()
    };
    assert_eq!(
        failing_job(&with_step, Some(38647056637)).as_deref(),
        Some("Fast Validation — CI tiers › Test release publication guards")
    );
    // cas-25ce: the log's failing tests ride along, each once, for the
    // worker's one batched repair.
    let with_tests = Fake {
        job: Some("Rust Suite"),
        step: Some("nextest"),
        log: Some("test a::passes ... ok\ntest a::red ... FAILED\ntest b::red ... FAILED\ntest a::red ... FAILED\n"),
        ..Fake::default()
    };
    // cas-f2ea: one watcher's lookups keep the named job across polls, so a
    // repeat ejection never downloads its failed log again.
    let lookups = FailedRunLookups::default();
    for _ in 0..3 {
        assert_eq!(
            lookups.failing_job(&with_tests, Some(1)).as_deref(),
            Some("Rust Suite › nextest (failing tests: a::red, b::red)")
        );
    }
    assert_eq!(with_tests.log_reads.get(), 1);
    let job_only = Fake {
        job: Some("macOS Check"),
        step: None,
        log: None,
        ..Fake::default()
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
        log: None,
        ..Fake::default()
    };
    assert_eq!(
        failing_job(&job, None),
        None,
        "a dequeue with no failed run names no job"
    );
    let offline = Fake {
        job: None,
        step: None,
        log: None,
        ..Fake::default()
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

/// cas-f2ea: the entry ahead counts as the possible cause when it failed the
/// same job, is still running, or cannot be read; only a clear entry ahead or
/// one that failed a different job lets this run spend the repair cycle.
#[test]
fn ahead_may_be_cause_blames_same_job_running_or_unreadable_entries_ahead() {
    use crate::ui::factory::daemon::runtime::ci_watch::AheadEntry;
    let blamed = |ahead_job, ahead| {
        let transport = Fake { job: Some("Rust Suite"), ahead: ahead_job, ..Fake::default() };
        FailedRunLookups::default().ahead_may_be_cause(&transport, Some(1), ahead)
    };
    assert!(blamed(Some("Rust Suite"), AheadEntry::Failed(2)), "same job ahead");
    assert!(!blamed(Some("macOS Check"), AheadEntry::Failed(2)), "different job ahead");
    assert!(blamed(None, AheadEntry::Failed(2)), "unreadable job ahead");
    assert!(blamed(None, AheadEntry::Unknown), "entry ahead still running");
    assert!(!blamed(None, AheadEntry::Clear), "no failed entry ahead");
}

#[test]
fn deliveries_skip_a_parked_task_whose_pr_merge_is_recorded_cas_c24e() {
    let temp = tempfile::TempDir::new().unwrap();
    let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
    let store = crate::store::open_task_store_cached(&cas_dir).unwrap();
    for (id, merged) in [("cas-open-pr", false), ("cas-merged-pr", true)] {
        let mut task = cas_types::Task::new(id.to_string(), id.to_string());
        task.status = cas_types::TaskStatus::AwaitingMerge;
        task.assignee = Some(format!("worker-{id}"));
        task.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:fixture".to_string(),
            target_branch: "main".to_string(),
        });
        task.deliverables.delivery_pr_number = Some(7);
        if merged {
            task.deliverables.delivery_pr_merge_commit = Some("merge-tip".to_string());
            task.deliverables.delivery_pr_merged_at = Some(chrono::Utc::now());
        }
        store.add(&task).unwrap();
    }
    let polled: Vec<_> = awaiting_merge_deliveries(&cas_dir).into_iter().map(|d| d.task_id).collect();
    assert_eq!(polled, vec!["cas-open-pr".to_string()], "a landed PR is not polled again");
}
