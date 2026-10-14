//! cas-e369 tests. The logs are the real `gh run view --log-failed` lines for
//! PR #1198's two 2026-10-14 ejections (HUB-J4 flake, then a memory-admission
//! stop), trimmed to the lines read; the file list is #1198's real diff
//! less its release-notes draft, whose name carries a version literal.

use super::*;
use crate::ui::factory::daemon::runtime::ci_watch::{
    CiRun, CiWatchError, DeliveryPullRequest, FailedJob, MergeQueuePullRequest, ci_watch_interval,
    collect_tracked_merge_queue_ejections,
};
use crate::ui::factory::daemon::runtime::lifecycle::{
    WorkerAttentionRelayOutcome, enqueue_merge_queue_ejection_relay,
};
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;

const HUB_J4_LOG: &str = include_str!("fixtures/mq-eject-38635679146.log");
const MEMORY_STOP_LOG: &str = include_str!("fixtures/mq-eject-38639494500.log");
const PR_1198_FILES: &str = include_str!("fixtures/pr-1198-files.txt");
const WEB_INPUTS: &str = include_str!("../../../../../../scripts/classify-web-inputs.py");
const PREFLIGHT: &str = "Fast Validation — preflight (no test suite)";
const COMMANDER_STEP: &str = "Build and test Commander web assets";
const EPIC_BRANCH: &str = "epic/release-pipeline-cas-a229";
const HEAD: &str = "4ae7e39872120730ac9d63ee54069ff6dd2a6f21";

fn pr_1198_files() -> Vec<String> {
    PR_1198_FILES.lines().map(str::to_string).collect()
}

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-10-14T21:16:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc)
}

struct Fake {
    runs: RefCell<Vec<CiRun>>,
    queued: Cell<bool>,
    log: RefCell<&'static str>,
    files: Option<Vec<String>>,
    enqueues: RefCell<Vec<(String, String)>>,
}

impl Fake {
    fn new(files: Option<Vec<String>>) -> Self {
        Self {
            runs: RefCell::new(Vec::new()),
            queued: Cell::new(true),
            log: RefCell::new(HUB_J4_LOG),
            files,
            enqueues: RefCell::new(Vec::new()),
        }
    }

    /// GitHub after a failed merge-group run: the PR is out of the queue.
    fn fail(&self, run_id: u64, completed_at: &str, log: &'static str) {
        self.runs.borrow_mut().push(CiRun {
            id: run_id,
            name: "CI".to_string(),
            head_branch: "gh-readonly-queue/main/pr-1198-5e572d51dc9cc289bf283c017c468bb5d6c8cc18"
                .to_string(),
            head_sha: format!("queue-{run_id}"),
            html_url: format!("https://github.test/runs/{run_id}"),
            status: "completed".to_string(),
            conclusion: Some("failure".to_string()),
            event: "merge_group".to_string(),
            updated_at: completed_at.to_string(),
        });
        *self.log.borrow_mut() = log;
        self.queued.set(false);
    }
}

impl CiTransport for Fake {
    fn completed_runs(&self) -> Result<Vec<CiRun>, CiWatchError> {
        Ok(self.runs.borrow().clone())
    }
    fn failing_job(&self, _: u64) -> Result<Option<String>, CiWatchError> {
        Ok(Some(PREFLIGHT.to_string()))
    }
    fn failed_job_detail(&self, _: u64) -> Result<Option<FailedJob>, CiWatchError> {
        Ok(Some(FailedJob {
            name: PREFLIGHT.to_string(),
            step: Some(COMMANDER_STEP.to_string()),
        }))
    }
    fn failed_log(&self, _: u64) -> Result<Option<String>, CiWatchError> {
        Ok(Some(self.log.borrow().to_string()))
    }
    fn pull_request_files(&self, _: u64) -> Result<Option<Vec<String>>, CiWatchError> {
        Ok(self.files.clone())
    }
    fn merge_queue_pull_requests(&self) -> Result<Vec<MergeQueuePullRequest>, CiWatchError> {
        Ok(vec![MergeQueuePullRequest {
            id: "PR_kwDO1198".to_string(),
            number: 1198,
            head_branch: EPIC_BRANCH.to_string(),
            head_sha: HEAD.to_string(),
            is_in_merge_queue: self.queued.get(),
            updated_at: format!("2026-10-14T21:15:{:02}Z", self.runs.borrow().len()),
            auto_merge_request: None,
            status_check_rollup: None,
        }])
    }
    fn delivery_pull_requests(&self, _: &str) -> Result<Vec<DeliveryPullRequest>, CiWatchError> {
        Ok(Vec::new())
    }
    fn enqueue_pull_request(&self, node: &str, head: &str) -> Result<(), CiWatchError> {
        self.enqueues
            .borrow_mut()
            .push((node.to_string(), head.to_string()));
        self.queued.set(true);
        Ok(())
    }
}

struct Factory {
    _env: crate::test_support::TestEnvGuard,
    _temp: tempfile::TempDir,
    cas_dir: std::path::PathBuf,
}

fn factory() -> Factory {
    let env = crate::test_support::TestEnvGuard::with_vars(&[(
        "CAS_FACTORY_SESSION",
        "merge-queue-recovery-test",
    )]);
    let temp = tempfile::TempDir::new().unwrap();
    let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
    let agents = crate::store::open_agent_store(&cas_dir).unwrap();
    let mut supervisor =
        cas_types::Agent::new("supervisor-id".to_string(), "supervisor".to_string());
    supervisor.role = cas_types::AgentRole::Supervisor;
    supervisor.status = cas_types::AgentStatus::Active;
    supervisor.factory_session = Some("merge-queue-recovery-test".to_string());
    agents.register(&supervisor).unwrap();
    let mut epic = cas_types::Task::new("cas-a229".to_string(), "release pipeline".to_string());
    epic.task_type = cas_types::TaskType::Epic;
    epic.status = cas_types::TaskStatus::InProgress;
    epic.branch = Some(EPIC_BRANCH.to_string());
    crate::store::open_task_store_cached(&cas_dir)
        .unwrap()
        .add(&epic)
        .unwrap();
    Factory {
        _env: env,
        _temp: temp,
        cas_dir,
    }
}

/// One watcher poll as the daemon runs it: collect, recover, relay.
fn poll(
    factory: &Factory,
    fake: &Fake,
    queued: &BTreeSet<u64>,
) -> (BTreeSet<u64>, Vec<EjectionReport>) {
    let tracked = supervisor_prs(&factory.cas_dir);
    let mut poll =
        collect_tracked_merge_queue_ejections(fake, &[], &tracked, queued, &BTreeSet::new())
            .unwrap();
    let mut reports = Vec::new();
    for mut ejection in std::mem::take(&mut poll.ejections) {
        let report = recover(&factory.cas_dir, fake, &mut ejection, now());
        if report.decision == RecoveryDecision::AutoRequeued {
            poll.queued_prs.insert(ejection.pr_number);
        }
        let outcome = enqueue_merge_queue_ejection_relay(
            &factory.cas_dir,
            &ejection.task_id,
            &ejection.worker,
            ejection.pr_number,
            ejection.failed_run_id,
            &ejection.occurrence,
            Some(&report),
        );
        assert!(matches!(
            outcome,
            WorkerAttentionRelayOutcome::Persisted { .. }
        ));
        reports.push(report);
    }
    (poll.queued_prs, reports)
}

fn prompts(factory: &Factory) -> Vec<cas_store::QueuedPrompt> {
    crate::store::open_prompt_queue_store(&factory.cas_dir)
        .unwrap()
        .peek_all(50)
        .unwrap()
}

fn epic_notes(factory: &Factory) -> String {
    crate::store::open_task_store_cached(&factory.cas_dir)
        .unwrap()
        .get("cas-a229")
        .unwrap()
        .notes
}

#[test]
fn commander_product_inputs_are_classifier_web_inputs() {
    for input in COMMANDER_PRODUCT_INPUTS {
        assert!(
            WEB_INPUTS.contains(&format!("'{input}',")),
            "{input} must stay a WEB_INPUTS entry in scripts/classify-web-inputs.py"
        );
    }
}

#[test]
fn hub_j4_flake_on_a_pr_without_hub_web_changes_is_outside_the_diff() {
    let files = pr_1198_files();
    let (scope, reason) = classify(
        Some(PREFLIGHT),
        Some(COMMANDER_STEP),
        Some(HUB_J4_LOG),
        Some(&files),
    );
    assert_eq!(scope, FailureScope::OutsideDiff, "{reason}");
    // The passing HUB-J3 line contains "failed"; the failed test is the ✘ line.
    assert_eq!(
        error_line(HUB_J4_LOG).as_deref(),
        Some(
            "✘ 116 [journeys] › e2e/journeys/read-history.journey.ts:9:1 › HUB-J4 read the conversation history (9.0s)"
        )
    );

    let mut with_web = files.clone();
    with_web.push("hub-web/src/history.ts".to_string());
    let (scope, reason) = classify(
        Some(PREFLIGHT),
        Some(COMMANDER_STEP),
        Some(HUB_J4_LOG),
        Some(&with_web),
    );
    assert_eq!(scope, FailureScope::TouchesDiff);
    assert!(reason.contains("hub-web/src/history.ts"), "{reason}");

    let (scope, _) = classify(
        Some(PREFLIGHT),
        Some(COMMANDER_STEP),
        Some(HUB_J4_LOG),
        None,
    );
    assert_eq!(
        scope,
        FailureScope::Unclassified,
        "an unread diff is never outside"
    );
}

#[test]
fn memory_admission_stop_is_infrastructure_but_routine_admission_lines_are_not() {
    let files = pr_1198_files();
    let (scope, reason) = classify(
        Some(PREFLIGHT),
        Some(COMMANDER_STEP),
        Some(MEMORY_STOP_LOG),
        Some(&files),
    );
    assert_eq!(scope, FailureScope::OutsideDiff);
    assert!(reason.starts_with("infrastructure stop: worker memory admission: worker suite reached assembly memory headroom"), "{reason}");
    assert_eq!(
        error_line(MEMORY_STOP_LOG).as_deref(),
        Some(
            "worker memory admission: worker suite reached assembly memory headroom; stopping its process group"
        )
    );

    // Every run logs `host memory admission: {...}`; that alone is no stop.
    let routine = MEMORY_STOP_LOG.lines().next().unwrap();
    assert!(routine.contains("host memory admission: {"));
    assert_eq!(infrastructure_stop(routine), None);

    let policy = vec!["scripts/host_memory.py".to_string()];
    let (scope, _) = classify(None, None, Some(MEMORY_STOP_LOG), Some(&policy));
    assert_eq!(scope, FailureScope::TouchesDiff);
}

#[test]
fn a_rust_job_failure_is_not_classified_outside_the_diff() {
    let log =
        "Check\tRun tests\t2026-10-14T21:00:00.0000000Z test store::tests::roundtrip ... FAILED\n";
    let (scope, _) = classify(
        Some("Check"),
        Some("Run tests"),
        Some(log),
        Some(&pr_1198_files()),
    );
    assert_eq!(scope, FailureScope::Unclassified);
    assert_eq!(
        error_line(log).as_deref(),
        Some("test store::tests::roundtrip FAILED")
    );
}

/// Acceptance 1: an ejection of a tracked PR is a typed supervisor wake in
/// the first poll that sees it, naming the job, step and error line.
#[test]
fn epic_pr_ejection_is_a_typed_wake_within_one_poll_with_job_and_error_line() {
    let factory = factory();
    let fake = Fake::new(Some(pr_1198_files()));

    let (queued, reports) = poll(&factory, &fake, &BTreeSet::new());
    assert_eq!(queued, BTreeSet::from([1198]), "the epic PR is tracked");
    assert!(reports.is_empty());
    assert_eq!(
        ci_watch_interval(false, !queued.is_empty()),
        std::time::Duration::from_secs(30),
        "a queued tracked PR is polled every 30 s"
    );

    fake.fail(38635679146, "2026-10-14T21:15:32Z", HUB_J4_LOG);
    let (_, reports) = poll(&factory, &fake, &queued);
    assert_eq!(reports.len(), 1, "the very next poll wakes");

    let rows = prompts(&factory);
    assert_eq!(
        rows.len(),
        1,
        "a supervisor wake and no worker row: {rows:?}"
    );
    let wake = &rows[0];
    assert_eq!(wake.target, "supervisor");
    assert!(
        crate::mcp::tools::core::task::lifecycle::supervisor_push::is_lifecycle_wake_source(
            &wake.source
        )
    );
    for expected in [
        "kind=\"merge_queue_ejected\"",
        "Epic PR #1198 for cas-a229 left the merge queue after failed merge_group run 38635679146.",
        "Failing job: Fast Validation — preflight (no test suite) (step: Build and test Commander web assets)",
        "Error: ✘ 116 [journeys] › e2e/journeys/read-history.journey.ts:9:1 › HUB-J4 read the conversation history (9.0s)",
        "Failure scope: outside-diff",
        "Recovery: Re-enqueued automatically once",
    ] {
        assert!(
            wake.prompt.contains(expected),
            "missing {expected:?} in {}",
            wake.prompt
        );
    }
    let notification = crate::store::open_supervisor_queue_store(&factory.cas_dir)
        .unwrap()
        .poll("supervisor-id", 10)
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&notification[0].payload).unwrap();
    assert_eq!(payload["kind"], "merge_queue_ejected");
    assert_eq!(payload["supervisor_owned"], true);
    assert_eq!(payload["recovery"]["failing_step"], COMMANDER_STEP);
    assert_eq!(payload["recovery"]["failure_scope"], "outside-diff");
    assert_eq!(payload["recovery"]["recovery"], "auto-requeued");
}

/// Acceptance 2, replaying #1198: the HUB-J4 flake re-enqueues exactly once;
/// the memory-admission stop on the same head escalates instead of looping.
#[test]
fn unrelated_failure_requeues_once_and_a_second_failure_escalates() {
    let factory = factory();
    let fake = Fake::new(Some(pr_1198_files()));
    let (queued, _) = poll(&factory, &fake, &BTreeSet::new());

    fake.fail(38635679146, "2026-10-14T21:15:32Z", HUB_J4_LOG);
    let (queued, first) = poll(&factory, &fake, &queued);
    assert_eq!(first[0].decision, RecoveryDecision::AutoRequeued);
    assert_eq!(
        *fake.enqueues.borrow(),
        vec![("PR_kwDO1198".to_string(), HEAD.to_string())],
        "one re-enqueue, bound to the ejected head"
    );
    assert!(queued.contains(&1198));

    // The re-enqueued PR is back in the queue: quiet, nothing re-decided.
    let (queued, reports) = poll(&factory, &fake, &queued);
    assert!(reports.is_empty());

    fake.fail(38639494500, "2026-10-14T21:15:59Z", MEMORY_STOP_LOG);
    let (queued, second) = poll(&factory, &fake, &queued);
    let RecoveryDecision::Escalated { reason } = &second[0].decision else {
        panic!("second failure must escalate: {:?}", second[0].decision);
    };
    assert!(reason.contains("second consecutive"), "{reason}");
    assert_eq!(
        second[0].diagnosis.as_ref().unwrap().scope,
        FailureScope::OutsideDiff
    );
    assert_eq!(fake.enqueues.borrow().len(), 1, "no second re-enqueue");
    assert!(!queued.contains(&1198));

    // Repeated polls of the same ejection re-decide nothing.
    let (_, replay) = poll(&factory, &fake, &queued);
    assert_eq!(
        replay[0].decision,
        RecoveryDecision::AlreadyDecided {
            decision: "escalated".to_string()
        }
    );
    assert_eq!(fake.enqueues.borrow().len(), 1);

    let wakes: Vec<_> = prompts(&factory)
        .into_iter()
        .filter(|row| row.target == "supervisor")
        .collect();
    assert_eq!(wakes.len(), 2, "one wake per ejection");
    assert!(
        wakes[1]
            .prompt
            .contains("Recovery: Escalated, not re-enqueued")
    );
    assert!(wakes[1].prompt.contains("reached assembly memory headroom"));

    let notes = epic_notes(&factory);
    assert_eq!(
        notes
            .matches(&format!(
                "merge-queue-recovery pr=#1198 head={HEAD} run=38635679146 decision=auto-requeued"
            ))
            .count(),
        1
    );
    assert_eq!(
        notes
            .matches(&format!(
                "merge-queue-recovery pr=#1198 head={HEAD} run=38639494500 decision=escalated"
            ))
            .count(),
        1
    );
}

#[test]
fn a_failure_touching_the_diff_is_not_requeued() {
    let factory = factory();
    let mut files = pr_1198_files();
    files.push("hub-web/e2e/journeys/read-history.journey.ts".to_string());
    let fake = Fake::new(Some(files));
    let (queued, _) = poll(&factory, &fake, &BTreeSet::new());
    fake.fail(38635679146, "2026-10-14T21:15:32Z", HUB_J4_LOG);
    let (_, reports) = poll(&factory, &fake, &queued);
    assert!(matches!(
        reports[0].decision,
        RecoveryDecision::SupervisorDecides { .. }
    ));
    assert!(fake.enqueues.borrow().is_empty());
    assert!(epic_notes(&factory).contains("run=38635679146 decision=supervisor-decides"));
}

#[test]
fn an_old_red_run_is_not_requeued() {
    let factory = factory();
    let fake = Fake::new(Some(pr_1198_files()));
    let (queued, _) = poll(&factory, &fake, &BTreeSet::new());
    fake.fail(38635679146, "2026-10-14T20:00:00Z", HUB_J4_LOG);
    let (_, reports) = poll(&factory, &fake, &queued);
    let RecoveryDecision::SupervisorDecides { reason } = &reports[0].decision else {
        panic!("{:?}", reports[0].decision);
    };
    assert!(reason.contains("more than 15 minutes ago"), "{reason}");
    assert!(fake.enqueues.borrow().is_empty());
}
