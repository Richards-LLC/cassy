//! cas-4491f (factory loop v2 C2c): the relays and the director-prompt
//! inject read and write nothing on the loop.
use super::super::imports::*;
use super::director_refresh::DirectorRefresh;
use cas_store::{lane_guard, wait_budget};

/// A test daemon on real job lanes (the provisioning fixture runs queue jobs
/// inline, which would put every job back on the loop).
fn daemon_on_lanes(cas_dir: &std::path::Path) -> FactoryDaemon {
    let mut daemon = super::provisioning_tests::daemon(cas_dir);
    daemon.queue_jobs = super::prompt_queue_jobs::QueueJobs::start(
        cas_dir.to_path_buf(),
        daemon.app.project_scope(),
    );
    daemon
}

/// The loop's side of one pass: store access is forbidden and the thread is
/// the control lane, so every store access, git command, `/proc` scan and
/// transcript read counts.
pub(super) struct LoopPass {
    store: u64,
    lane: u64,
    _forbid: wait_budget::ForbidStoreGuard,
    _control: lane_guard::LaneGuard,
}

impl LoopPass {
    pub(super) fn start() -> Self {
        Self {
            store: wait_budget::store_access_violations(),
            lane: lane_guard::thread_violations(),
            _forbid: wait_budget::forbid_store_access(),
            _control: lane_guard::enter_lane(lane_guard::Lane::Control),
        }
    }

    /// Count on the lane the caller already entered (an enforced one, say).
    pub(super) fn counting() -> LoopCount {
        LoopCount {
            store: wait_budget::store_access_violations(),
            lane: lane_guard::thread_violations(),
            _forbid: wait_budget::forbid_store_access(),
        }
    }

    pub(super) fn assert_no_io(&self, what: &str) {
        assert_eq!(
            wait_budget::store_access_violations(),
            self.store,
            "{what} read the store on the loop: {:?}",
            wait_budget::last_store_access_violation()
        );
        assert_eq!(
            lane_guard::thread_violations(),
            self.lane,
            "{what} ran blocking work on the loop: {:?}",
            lane_guard::last_thread_violation()
        );
    }
}

/// [`LoopPass`] on a lane the caller entered.
pub(super) struct LoopCount {
    store: u64,
    lane: u64,
    _forbid: wait_budget::ForbidStoreGuard,
}

impl LoopCount {
    pub(super) fn assert_no_io(&self, what: &str) {
        assert_eq!(
            wait_budget::store_access_violations(),
            self.store,
            "{what} read the store on the loop: {:?}",
            wait_budget::last_store_access_violation()
        );
        assert_eq!(
            lane_guard::thread_violations(),
            self.lane,
            "{what} ran blocking work on the loop: {:?}",
            lane_guard::last_thread_violation()
        );
    }
}

/// cas-675a: the fixture worker's name, unique to this test process. Death
/// recording asks the host's real process table whether a process still runs
/// as `--agent-name <name>` or `CAS_AGENT_NAME=<name>`, and rightly records no
/// death while one does. A shared literal let any such host process (a
/// sibling test's real harness respawn, say) make this dead worker look alive.
fn relay_worker() -> &'static str {
    static NAME: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| format!("relay-worker-{}", std::process::id()));
    &NAME
}

/// A project with this factory session's active worker registered, so the
/// relays' reads have a candidate to look at.
fn project_with_worker() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut worker = cas_types::Agent::new("w-1".into(), relay_worker().into());
    worker.role = cas_types::AgentRole::Worker;
    worker.factory_session = Some("provision-test".into());
    crate::store::open_agent_store(&cas_dir).unwrap().register(&worker).unwrap();
    (dir, cas_dir)
}

/// Apply relay results until none is in flight (hang guard: 30 s).
async fn settle_relays(daemon: &mut FactoryDaemon) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        daemon.drain_relay_jobs();
        if daemon.relay_jobs.idle() {
            return;
        }
        assert!(Instant::now() < deadline, "relay jobs never settled");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

struct ReconcileCloud {
    lookup: std::result::Result<serde_json::Value, crate::coordination::external_work::RemoteError>,
    lookups: std::sync::atomic::AtomicUsize,
}

impl crate::coordination::external_work::CloudCoordinationPort for ReconcileCloud {
    fn post(&self, _path: &str, _payload: &serde_json::Value, _timeout: Duration) -> std::result::Result<serde_json::Value, crate::coordination::external_work::RemoteError> {
        Err(crate::coordination::external_work::RemoteError::unavailable("reconciliation must use operation lookup, not replay the mutation"))
    }

    fn lookup(&self, _request_id: &str, _registration_id: &str, _timeout: Duration) -> std::result::Result<serde_json::Value, crate::coordination::external_work::RemoteError> {
        self.lookups.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.lookup.clone()
    }
}

struct ReconcileViolet;

impl crate::coordination::external_work::VioletThreadResolver for ReconcileViolet {
    fn resolve(&self, _external_ref: &str, _timeout: Duration) -> std::result::Result<serde_json::Value, crate::coordination::external_work::RemoteError> {
        Err(crate::coordination::external_work::RemoteError::unavailable("journaled wire rows do not need Violet resolution"))
    }
}

fn due_external_work_service(
    cas_dir: &std::path::Path,
    lookup: std::result::Result<serde_json::Value, crate::coordination::external_work::RemoteError>,
) -> (
    crate::coordination::external_work::ExternalWorkService,
    crate::coordination::external_work::SqliteRequestJournal,
    std::sync::Arc<ReconcileCloud>,
    String,
) {
    use crate::coordination::external_work::{ExternalWorkService, OperationContext, SqliteRequestJournal};
    let journal = SqliteRequestJournal::open(cas_dir).unwrap();
    let request_id = "00000081-2222-4222-8222-000000000081".to_string();
    journal.prepare(&request_id, "release_claim", &serde_json::json!({
        "resource_id":"00000b01-1111-4111-8111-000000000b01", "expected_generation":1
    })).unwrap();
    journal.set_wire_request(&request_id, &serde_json::json!({
        "request_id":request_id.clone(), "expected_generation":1
    })).unwrap();
    journal.begin_attempt(&request_id).unwrap();
    journal.record_result(&request_id, &serde_json::json!({
        "schema":"cassy.coordination/v1", "code":"outcome_unknown",
        "receipt":{"request_id":request_id.clone()}
    }), Some(Duration::ZERO)).unwrap();
    let cloud = std::sync::Arc::new(ReconcileCloud { lookup, lookups: std::sync::atomic::AtomicUsize::new(0) });
    let service = ExternalWorkService::new(
        journal.clone(), cloud.clone(), std::sync::Arc::new(ReconcileViolet),
        OperationContext { project_id: "github.com/richards-llc/cassy".into(), registration_id: "reg-supervisor-alpha-01".into() },
    );
    (service, journal, cloud, request_id)
}

#[tokio::test]
async fn external_work_reconcile_job_runs_due_row_without_turn_or_hook() {
    let (_dir, cas_dir) = project_with_worker();
    let request_id = "00000081-2222-4222-8222-000000000081";
    let (service, journal, cloud, request_id) = due_external_work_service(&cas_dir, Ok(serde_json::json!({
        "schema":"cassy.coordination/v1", "code":"released",
        "receipt":{"request_id":request_id}
    })));
    let mut daemon = daemon_on_lanes(&cas_dir);
    let now_ms = chrono::Utc::now().timestamp_millis() + 50;
    {
        let pass = LoopPass::start();
        assert!(daemon.relay_jobs.submit_external_work_reconciliation(&mut daemon.queue_jobs, move || {
            super::external_work_reconcile_jobs::reconcile_service_due(&service, now_ms)
        }));
        pass.assert_no_io("external-work reconciliation submission");
    }
    settle_relays(&mut daemon).await;
    assert_eq!(cloud.lookups.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(journal.get(&request_id).unwrap().unwrap().stage, "terminal");
}

#[tokio::test]
async fn external_work_reconcile_job_reschedules_cloud_outage() {
    use crate::coordination::external_work::RemoteError;
    let (_dir, cas_dir) = project_with_worker();
    let (service, journal, cloud, request_id) = due_external_work_service(&cas_dir, Err(RemoteError::unavailable("Cloud is offline")));
    let mut daemon = daemon_on_lanes(&cas_dir);
    let now_ms = chrono::Utc::now().timestamp_millis() + 50;
    {
        let pass = LoopPass::start();
        assert!(daemon.relay_jobs.submit_external_work_reconciliation(&mut daemon.queue_jobs, move || {
            super::external_work_reconcile_jobs::reconcile_service_due(&service, now_ms)
        }));
        pass.assert_no_io("external-work outage reconciliation submission");
    }
    settle_relays(&mut daemon).await;
    let row = journal.get(&request_id).unwrap().unwrap();
    assert_eq!(row.stage, "retry_wait");
    assert!(row.next_retry_ms.is_some_and(|retry_at| retry_at > now_ms - 50));
    assert_eq!(cloud.lookups.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn c4491f_usage_limit_relay_reads_nothing_on_the_loop() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    daemon.app.add_worker_name_for_test(relay_worker());
    {
        let pass = LoopPass::start();
        daemon.relay_usage_limited_workers();
        pass.assert_no_io("the usage-limit relay");
    }
    settle_relays(&mut daemon).await;
}

#[tokio::test]
async fn c4491f_auth_failure_relay_reads_nothing_on_the_loop() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    daemon.app.add_worker_name_for_test(relay_worker());
    {
        let pass = LoopPass::start();
        daemon.relay_auth_failed_workers();
        pass.assert_no_io("the auth-failure relay");
    }
    settle_relays(&mut daemon).await;
}

#[tokio::test]
async fn c4491f_permission_relay_reads_nothing_on_the_loop() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    daemon.app.add_worker_name_for_test(relay_worker());
    {
        let pass = LoopPass::start();
        daemon.relay_worker_permission_requests();
        pass.assert_no_io("the permission relay");
    }
    settle_relays(&mut daemon).await;
}

/// The correlated-deaths relay writes on the store worker, and the group
/// leaves the window once its write reports.
#[tokio::test]
async fn c4491f_correlated_deaths_relay_writes_nothing_on_the_loop() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    // Older than the ten-minute retry limit, so the group settles whether or
    // not this bare project can persist the relay.
    let at = chrono::Utc::now() - chrono::Duration::minutes(11);
    for worker in ["dead-a", "dead-b"] {
        daemon
            .recent_worker_exits
            .push(super::ObservedWorkerExit {
                at,
                worker: worker.into(),
                status: "exit 1".into(),
                death: None,
                recording: false,
            });
    }
    {
        let pass = LoopPass::start();
        daemon.relay_correlated_worker_deaths();
        pass.assert_no_io("the correlated-deaths relay");
    }
    settle_relays(&mut daemon).await;
    assert!(daemon.recent_worker_exits.is_empty(), "the group settled");
}

#[tokio::test]
async fn c4491f_relay_escalation_reads_nothing_on_the_loop() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    {
        let pass = LoopPass::start();
        daemon.escalate_undelivered_supervisor_relays();
        pass.assert_no_io("the relay escalation");
    }
    settle_relays(&mut daemon).await;
}

/// An assignment wake's task is read with its `RevalidateFresh` re-check,
/// and the assignment row is written on the store worker: neither the
/// delivery step nor the inject reads the task store on the loop.
#[tokio::test]
async fn c4491f_assignment_wake_reads_its_task_off_the_loop() {
    let (_dir, cas_dir) = project_with_worker();
    let mut task = cas_types::Task::new("cas-a551".into(), "assigned work".into());
    task.assignee = Some(relay_worker().into());
    crate::store::open_task_store_local(&cas_dir).unwrap().add(&task).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let progress = super::loop_watchdog::LoopProgress::new();
    let store_worker = super::store_worker::global();
    let mut refresh = DirectorRefresh::new();
    let wake = crate::ui::factory::director::Prompt {
        merge_alert: None,
        target: relay_worker().into(),
        text: "You have been assigned a new task:\nTask ID: cas-a551\nTitle: assigned work".into(),
        stall_nudge_task: None,
        retract_worker: None,
        retract_task: None,
        retract_epic: None,
        drop_if_worker_assigned: None,
        durable_retry: true,
    };
    {
        let pass = LoopPass::start();
        daemon
            .deliver_director_events(
                &mut refresh,
                Vec::new(),
                (Vec::new(), vec![wake], None),
                Instant::now(),
                0.0,
                store_worker,
                &progress,
            )
            .await;
        pass.assert_no_io("the delivery step");
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let batch = loop {
        if let Some(batch) = refresh.take_deliverability(&mut daemon.queue_jobs).pop() {
            break batch;
        }
        assert!(Instant::now() < deadline, "the re-check never finished");
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert!(
        matches!(
            batch.assignments.first(),
            Some(crate::ui::factory::app::AssignmentRead::Task(task)) if task.id == "cas-a551"
        ),
        "the job read the assigned task: {:?}",
        batch.assignments
    );
    {
        let pass = LoopPass::start();
        daemon.inject_rechecked_batch(batch, store_worker).await;
        pass.assert_no_io("the assignment inject");
    }
}

/// A transport-boundary read of a task that moved on suppresses the wake;
/// one it cannot read is uncertainty (delivered directly).
#[test]
fn c4491f_assignment_read_names_a_stale_task() {
    use crate::ui::factory::app::AssignmentRead;
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut task = cas_types::Task::new("cas-c105".into(), "done".into());
    task.status = cas_types::TaskStatus::Closed;
    crate::store::open_task_store_local(&cas_dir).unwrap().add(&task).unwrap();
    let wake = |id: &str| crate::ui::factory::director::Prompt {
        merge_alert: None,
        target: "w".into(),
        text: format!("You have been assigned a new task:\nTask ID: {id}"),
        stall_nudge_task: None,
        retract_worker: None,
        retract_task: None,
        retract_epic: None,
        drop_if_worker_assigned: None,
        durable_retry: true,
    };
    assert!(matches!(
        AssignmentRead::read(&cas_dir, None, &wake("cas-c105")),
        AssignmentRead::Stale(id, cas_types::TaskStatus::Closed) if id == "cas-c105"
    ));
    assert!(matches!(
        AssignmentRead::read(&cas_dir, None, &wake("cas-9999")),
        AssignmentRead::Unavailable
    ));
}

/// cas-4491f review M1: an escalation read answers only for the notices it
/// asked about. A notice opened while the read was in flight is not
/// announced resolved.
#[test]
fn c4491f_notice_opened_during_the_read_is_not_resolved() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = super::provisioning_tests::daemon(&cas_dir);
    daemon.open_operator_notices.insert(1, 100);
    // Opened after the read asked about notice 1.
    daemon.open_operator_notices.insert(2, 200);
    // The read asked about notice 1 only, and found it resolved (gone).
    daemon.decide_undelivered_supervisor_relays(1, vec![1], Some(Vec::new()), Vec::new());
    assert_eq!(
        daemon.open_operator_notices.keys().copied().collect::<Vec<_>>(),
        vec![2],
        "notice 1 resolved; notice 2 was not asked about and stays open"
    );
}

/// cas-4491f review M2: a read given up at its timeout is read again; the
/// key's rerun reports under the new generation and the late result of the
/// first is ignored, so one read is applied, not two.
#[tokio::test]
async fn c4491f_late_relay_read_is_ignored_and_its_rerun_reports() {
    use super::relay_jobs::{Relay, RelayJobs, RelayOutput};
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let mut relays = RelayJobs::default();
    relays.timeout_override = Some(Duration::ZERO);
    assert!(relays.submit_read(&mut daemon.queue_jobs, Relay::UsageLimit, move || {
        let _ = wait.recv();
        RelayOutput::UsageLimitRead(vec![("late".into(), "late".into(), crate::mcp::tools::service::factory_ops::UsageLimitEvidence::Unavailable)])
    }));
    assert!(relays.take(&mut daemon.queue_jobs).is_empty());
    assert!(!relays.reading(Relay::UsageLimit), "the timed-out read freed its key");
    relays.timeout_override = Some(Duration::from_secs(30));
    assert!(
        relays.submit_read(&mut daemon.queue_jobs, Relay::UsageLimit, || {
            RelayOutput::UsageLimitRead(vec![("rerun".into(), "rerun".into(), crate::mcp::tools::service::factory_ops::UsageLimitEvidence::Unavailable)])
        }),
        "a submit coalesced behind the running read is in flight"
    );
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut applied = Vec::new();
    while relays.reading(Relay::UsageLimit) {
        for done in relays.take(&mut daemon.queue_jobs) {
            if let RelayOutput::UsageLimitRead(observed) = done.output {
                applied.extend(observed.into_iter().map(|(id, _, _)| id));
            }
        }
        assert!(Instant::now() < deadline, "the rerun never reported");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(applied, vec!["rerun".to_string()], "only the rerun's result applies");
}

/// cas-4491f review M2: a write is never given up at a timeout, so it cannot
/// be submitted twice (a second correlated-deaths write would drain a newer
/// group).
#[tokio::test]
async fn c4491f_relay_write_is_never_given_up() {
    use super::relay_jobs::{RelayJobs, RelayOutput};
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let mut relays = RelayJobs::default();
    relays.timeout_override = Some(Duration::ZERO);
    assert!(relays.submit_write(&mut daemon.queue_jobs, "correlated-deaths", None, move || {
        let _ = wait.recv();
        RelayOutput::DeathsRelayed { group: Vec::new(), done: true }
    }));
    assert!(relays.take(&mut daemon.queue_jobs).is_empty());
    assert!(relays.writing("correlated-deaths"), "a slow write keeps its key");
    assert!(
        !relays.submit_write(&mut daemon.queue_jobs, "correlated-deaths", None, || {
            RelayOutput::DeathsRelayed { group: Vec::new(), done: true }
        }),
        "no second write while the first runs"
    );
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut reported = 0;
    while relays.writing("correlated-deaths") {
        reported += relays.take(&mut daemon.queue_jobs).len();
        assert!(Instant::now() < deadline, "the write never reported");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    reported += relays.take(&mut daemon.queue_jobs).len();
    assert_eq!(reported, 1);
}

/// cas-4491f review M3: a batch's writes are applied before its reads, and a
/// read submitted before a write of the same key landed cannot ask for that
/// write again (no duplicate notice, no second permission answer).
#[test]
fn c4491f_writes_apply_before_reads_and_stale_reads_do_not_rewrite() {
    use super::relay_jobs::{Relay, RelayJobs, RelayOutput};
    let mut jobs = super::prompt_queue_jobs::QueueJobs::inline();
    let mut relays = RelayJobs::default();
    // The read is submitted first and its write after: both finish inline.
    assert!(relays.submit_read(&mut jobs, Relay::Permission, || RelayOutput::PermissionRead(Vec::new())));
    let request = crate::factory_permission_relay::WorkerPermissionRequest {
        worker: "w".into(),
        request_id: "req-1".into(),
        tool_name: "Bash".into(),
        tool_use_id: None,
        command_excerpt: String::new(),
        age_secs: 1,
    };
    let answered = request.clone();
    assert!(relays.submit_write(&mut jobs, "permission-answer:req-1", None, move || {
        RelayOutput::PermissionAnswered { request: answered, result: Ok(()) }
    }));
    let finished = relays.take(&mut jobs);
    let order: Vec<&str> = finished
        .iter()
        .map(|done| match done.output {
            RelayOutput::PermissionAnswered { .. } => "written",
            RelayOutput::PermissionRead(_) => "read",
            _ => "other",
        })
        .collect();
    assert_eq!(order, vec!["written", "read"], "the write applies first");
    let read_basis = finished
        .iter()
        .find(|done| matches!(done.output, RelayOutput::PermissionRead(_)))
        .map(|done| done.generation)
        .unwrap();
    assert!(
        !relays.submit_write(&mut jobs, "permission-answer:req-1", Some(read_basis), move || {
            RelayOutput::PermissionAnswered { request, result: Ok(()) }
        }),
        "the read predates the answer, so it cannot answer again"
    );
}

/// cas-4491f review M4: an assignment wake whose re-check never reported is
/// marked unread, not "unavailable": it is never delivered without its stale
/// check.
#[tokio::test]
async fn c4491f_expired_recheck_leaves_the_assignment_unread() {
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let gate = Arc::new(super::director_refresh::ReadGate::default());
    let mut refresh = DirectorRefresh::new()
        .with_read_gate(Arc::clone(&gate))
        .with_timeout(Duration::ZERO);
    let wake = crate::ui::factory::director::Prompt {
        merge_alert: None,
        target: "w".into(),
        text: "You have been assigned a new task:\nTask ID: cas-a551".into(),
        stall_nudge_task: None,
        retract_worker: None,
        retract_task: None,
        retract_epic: None,
        drop_if_worker_assigned: None,
        durable_retry: true,
    };
    refresh
        .start_deliverability_check(&daemon.app, &mut daemon.queue_jobs, vec![wake], Vec::new(), Instant::now())
        .unwrap();
    let batch = refresh.take_deliverability(&mut daemon.queue_jobs).pop().expect("given up at once");
    gate.open();
    assert!(
        matches!(batch.assignments.first(), Some(crate::ui::factory::app::AssignmentRead::Unread)),
        "{:?}",
        batch.assignments
    );
}

/// cas-4491f review M4 (GH #682): an unread assignment wake is read on the
/// store worker before anything carries it, and a task that moved on
/// suppresses it there.
#[test]
fn c4491f_unread_assignment_is_checked_before_it_is_delivered() {
    use crate::ui::factory::app::{UnreadAssignmentOutcome, resolve_unread_assignment};
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut task = cas_types::Task::new("cas-c105".into(), "done".into());
    task.status = cas_types::TaskStatus::Closed;
    crate::store::open_task_store_local(&cas_dir).unwrap().add(&task).unwrap();
    let wake = crate::ui::factory::director::Prompt {
        merge_alert: None,
        target: "w".into(),
        text: "You have been assigned a new task:\nTask ID: cas-c105".into(),
        stall_nudge_task: None,
        retract_worker: None,
        retract_task: None,
        retract_epic: None,
        drop_if_worker_assigned: None,
        durable_retry: true,
    };
    assert_eq!(
        resolve_unread_assignment(&cas_dir, "provision-test", &wake),
        UnreadAssignmentOutcome::Suppressed
    );
}

/// A watchdog recovery is historical evidence even when its original body
/// quotes an assignment whose task is now closed.
#[test]
fn cas_b2da7_recovered_assignment_surfaces_at_turn_start() {
    use crate::hooks::handlers::handlers_middle::factory_inbox::surface_factory_inbox;
    use cas_store::QueueOrigin;
    let mut env = crate::test_support::TestEnvGuard::temp_home();
    env.set("CAS_FACTORY_SESSION", "provision-test");
    env.set("CAS_AGENT_ROLE", "worker");
    env.set("CAS_AGENT_NAME", relay_worker());
    let (_dir, cas_dir) = project_with_worker();
    let mut task = cas_types::Task::new("cas-b2da7".into(), "Closed assignment".into());
    task.status = cas_types::TaskStatus::Closed;
    crate::store::open_task_store_local(&cas_dir)
        .unwrap()
        .add(&task)
        .unwrap();
    let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
    let body = "You are assigned task cas-b2da7. Keep the pending decision for review.";
    let original = queue
        .enqueue_urgent_with_outcome(
            relay_worker(),
            "ended-worker",
            body,
            Some("provision-test"),
            Some("decision still needs review"),
            None,
            false,
            Some(&QueueOrigin::RegisteredAgent {
                agent_id: "w-1".into(),
            }),
        )
        .unwrap()
        .id();
    queue
        .record_wake_gate_decline(original, "active composer")
        .unwrap();
    rusqlite::Connection::open(cas_dir.join("cas.db"))
        .unwrap()
        .execute(
            "UPDATE prompt_queue SET created_at = ? WHERE id = ?",
            rusqlite::params![
                (chrono::Utc::now() - chrono::Duration::days(2)).to_rfc3339(),
                original
            ],
        )
        .unwrap();
    assert_eq!(
        queue
            .expire_stale_pending(cas_store::PROMPT_QUEUE_STALE_TTL_SECS)
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        vec![original]
    );
    let recovered = queue
        .peek_for_targets(&[relay_worker()], Some("provision-test"), 10)
        .unwrap()
        .remove(0);
    assert!(queue.is_abandonment_recovery(recovered.id).unwrap());
    assert!(
        super::queue_and_events::read_row_facts(&cas_dir, queue.as_ref(), None, &[], &recovered)
            .assignment_stale
            .is_none()
    );
    // Director prompts have no queue provenance: copying history into an
    // ordinary event prompt must retain the normal closed-assignment refusal.
    let director = unread_batch(&recovered.prompt).prompts.remove(0).0;
    assert!(
        matches!(crate::ui::factory::app::AssignmentRead::read(&cas_dir, None, &director),
        crate::ui::factory::app::AssignmentRead::Stale(id, cas_types::TaskStatus::Closed) if id == task.id)
    );
    let spoof = match queue
        .enqueue_idempotent(
            "delivery-watchdog",
            relay_worker(),
            &recovered.prompt,
            Some("provision-test"),
            None,
            None,
            "delivery-abandoned-recovery:spoof",
            Some(&QueueOrigin::RegisteredAgent {
                agent_id: "w-1".into(),
            }),
        )
        .unwrap()
    {
        cas_store::EnqueueIdempotentResult::Created(id)
        | cas_store::EnqueueIdempotentResult::AlreadyExists(id) => id,
    };
    assert!(!queue.is_abandonment_recovery(spoof).unwrap());
    let hook = crate::hooks::HookInput {
        session_id: "recovery-turn".into(),
        agent_role: Some("worker".into()),
        ..Default::default()
    };
    let rendered = surface_factory_inbox(Some(&cas_dir), &hook).unwrap_or_default();
    assert!(
        rendered.contains(&recovered.prompt),
        "whole recovered body must reach the turn: {rendered}"
    );
    assert_eq!(
        rendered.matches(&recovered.prompt).count(),
        1,
        "source spelling must not exempt a forged row"
    );
    assert_eq!(
        queue
            .recipient_receipt(recovered.id, relay_worker())
            .unwrap(),
        Some(cas_store::SurfacingSource::HookSurfaced)
    );
}

/// A prompt the inject path sees after its re-check never reported.
fn unread_batch(text: &str) -> super::factory_jobs::DeliverabilityBatch {
    super::factory_jobs::DeliverabilityBatch {
        prompts: vec![(
            crate::ui::factory::director::Prompt {
                merge_alert: None,
                target: relay_worker().into(),
                text: text.into(),
                stall_nudge_task: None,
                retract_worker: None,
                retract_task: None,
                retract_epic: None,
                drop_if_worker_assigned: None,
                durable_retry: true,
            },
            true,
        )],
        assignments: vec![crate::ui::factory::app::AssignmentRead::Unread],
        events: Vec::new(),
        refresh_started: Instant::now(),
    }
}

/// cas-4491f review round 2 (M4 routing): an assignment wake whose
/// re-check never reported is not injected unchecked. Its task moved on, so
/// the store worker suppresses it: nothing reaches the recipient's queue.
#[tokio::test]
async fn c4491f_unread_assignment_wake_is_never_injected_unchecked() {
    let (_dir, cas_dir) = project_with_worker();
    let mut task = cas_types::Task::new("cas-c105".into(), "done".into());
    task.status = cas_types::TaskStatus::Closed;
    task.assignee = Some(relay_worker().into());
    crate::store::open_task_store_local(&cas_dir).unwrap().add(&task).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let store_worker = super::store_worker::global();
    daemon
        .inject_rechecked_batch(
            unread_batch("You have been assigned a new task:\nTask ID: cas-c105"),
            store_worker,
        )
        .await;
    assert!(store_worker.wait_idle(Duration::from_secs(30)));
    let queued: Vec<_> = crate::store::open_prompt_queue_store(&cas_dir)
        .unwrap()
        .peek_all(50)
        .unwrap()
        .into_iter()
        .filter(|row| row.target == relay_worker())
        .collect();
    assert!(queued.is_empty(), "a stale assignment wake reached the queue: {queued:?}");
}

/// cas-4491f review round 2: a live unread assignment is carried by its
/// durable assignment row.
#[test]
fn c4491f_unread_live_assignment_writes_its_assignment_row() {
    use crate::ui::factory::app::{UnreadAssignmentOutcome, resolve_unread_assignment};
    let (_dir, cas_dir) = project_with_worker();
    let mut task = cas_types::Task::new("cas-1a55".into(), "live".into());
    task.assignee = Some(relay_worker().into());
    crate::store::open_task_store_local(&cas_dir).unwrap().add(&task).unwrap();
    let wake = unread_batch("You have been assigned a new task:\nTask ID: cas-1a55").prompts.remove(0).0;
    assert_eq!(
        resolve_unread_assignment(&cas_dir, "provision-test", &wake),
        UnreadAssignmentOutcome::AssignmentRow
    );
    assert!(
        crate::store::open_prompt_queue_store(&cas_dir)
            .unwrap()
            .peek_all(50)
            .unwrap()
            .iter()
            .any(|row| row.source == "task-assignment" && row.target == relay_worker()),
        "the assignment row carries the wake"
    );
}

/// cas-4491f review L1: a finished snapshot is applied before a due one
/// starts, so the due tick does not defer into a second, redundant read.
#[tokio::test]
async fn c4491f_due_tick_with_a_finished_snapshot_reads_once() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let progress = super::loop_watchdog::LoopProgress::new();
    let store_worker = super::store_worker::global();
    let mut refresh = DirectorRefresh::new();
    refresh.start_director_read(&daemon.app, &mut daemon.queue_jobs);
    let deadline = Instant::now() + Duration::from_secs(30);
    while !refresh.has_finished(&mut daemon.queue_jobs) {
        assert!(Instant::now() < deadline, "the snapshot never finished");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // The tick is due while that snapshot waits to be applied.
    assert!(
        daemon
            .director_refresh_pass(&mut refresh, true, store_worker, &progress)
            .await
    );
    assert!(
        !refresh.refresh_requested(),
        "the due tick started its read; no second read is owed"
    );
}

/// cas-4491f review L3: a relay scan the runtime refused is not stamped, so
/// the next pass scans again (and the first permission scan keeps its
/// prune).
#[tokio::test]
async fn c4491f_refused_relay_scans_are_retried() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    daemon.queue_jobs.close_lanes_for_test();
    daemon.relay_usage_limited_workers();
    daemon.relay_auth_failed_workers();
    daemon.relay_worker_permission_requests();
    assert!(daemon.last_usage_limit_scan.is_none());
    assert!(daemon.last_auth_failure_scan.is_none());
    assert!(
        daemon.last_permission_request_scan.is_none(),
        "the first scan, with its prune, runs again"
    );
}

/// cas-4491f review L5: the escalation write re-checks each candidate: a
/// relay acknowledged since the read is not escalated.
#[test]
fn c4491f_escalation_rechecks_each_candidate_under_its_write() {
    use cas_store::PromptQueueStore;
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
    for n in 1..=2 {
        queue
            .enqueue_idempotent(
                &format!("lifecycle-wake:{n}"),
                "supervisor",
                "a lifecycle relay",
                Some("provision-test"),
                None,
                Some(cas_store::NotificationPriority::High),
                &format!("c4491f-relay:{n}"),
                Some(&cas_store::QueueOrigin::Daemon),
            )
            .unwrap();
    }
    let candidates = queue.peek_all(10).unwrap();
    assert_eq!(candidates.len(), 2);
    // The supervisor took the first after the read.
    queue.ack(candidates[0].id).unwrap();
    let still = super::queue_and_events::still_undelivered_relays(&cas_dir, queue.as_ref(), candidates.clone());
    assert_eq!(
        still.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![candidates[1].id]
    );
}

/// cas-4491f review round 2: write records are kept only while a read that
/// predates them is in flight.
#[test]
fn c4491f_write_records_do_not_accumulate() {
    use super::relay_jobs::{Relay, RelayJobs, RelayOutput};
    let mut jobs = super::prompt_queue_jobs::QueueJobs::inline();
    let mut relays = RelayJobs::default();
    for n in 0..5 {
        let key = format!("permission-answer:req-{n}");
        assert!(relays.submit_write(&mut jobs, &key, None, || RelayOutput::EscalationWritten(Vec::new())));
        relays.take(&mut jobs);
    }
    assert!(relays.submit_read(&mut jobs, Relay::Permission, || RelayOutput::PermissionRead(Vec::new())));
    relays.take(&mut jobs);
    relays.take(&mut jobs);
    assert_eq!(relays.write_records(), 0, "no read predates any of them");
}

/// A daemon whose session has metadata on disk (in a temporary HOME), with
/// epic-focus writes deferred as the daemon loop runs them.
fn daemon_with_session_metadata(
    cas_dir: &std::path::Path,
    session: &str,
) -> (FactoryDaemon, std::path::PathBuf) {
    let path = crate::ui::factory::session::metadata_path(session);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let metadata = crate::ui::factory::session::create_metadata(
        session, 1, "supervisor", &[], None, None, None,
    );
    std::fs::write(&path, serde_json::to_string_pretty(&metadata).unwrap()).unwrap();
    let mut daemon = super::provisioning_tests::daemon(cas_dir);
    daemon.app.set_factory_session_name_for_test(session);
    daemon.app.set_deferred_session_metadata_writes(true);
    (daemon, path)
}

fn persisted_epic(path: &std::path::Path) -> Option<String> {
    let metadata: crate::ui::factory::protocol::SessionMetadata =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    metadata.epic_id
}

/// cas-5e72: an epic-focus change on the loop writes nothing there; the
/// session metadata write waits for the store worker.
#[test]
fn c5e72_epic_focus_change_writes_nothing_on_the_loop() {
    let home = tempfile::tempdir().unwrap();
    let _env = crate::test_support::TestEnvGuard::with_vars(&[("HOME", home.path().to_str().unwrap())]);
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let (mut daemon, path) = daemon_with_session_metadata(&cas_dir, "focus-off-loop");
    daemon
        .app
        .set_session_focus_for_test("cas-e41c", crate::ui::factory::app::EpicFocusSource::SessionDefault);
    {
        // Enforced, as C6 runs the daemon's proven-clean steps: a blocking
        // call here panics.
        let _lane = super::loop_watchdog::enforce_test_lane(lane_guard::Lane::Control);
        let pass = LoopPass::counting();
        let changes = daemon.app.handle_epic_events(&[crate::ui::factory::director::DirectorEvent::EpicStarted {
            epic_id: "cas-e41c".into(),
            epic_title: "focused".into(),
        }]);
        assert_eq!(changes.len(), 1, "the epic was adopted");
        pass.assert_no_io("the epic-focus change");
    }
    assert_eq!(persisted_epic(&path), None, "nothing written yet");
    daemon.flush_epic_focus_write(super::store_worker::global());
    assert!(super::store_worker::global().wait_idle(Duration::from_secs(30)));
    assert_eq!(persisted_epic(&path).as_deref(), Some("cas-e41c"));
}

/// cas-5e72: rapid focus changes persist the last one, whether or not the
/// loop handed off the writes in between.
#[test]
fn c5e72_rapid_focus_changes_persist_the_last() {
    use crate::ui::factory::director::DirectorEvent;
    let home = tempfile::tempdir().unwrap();
    let _env = crate::test_support::TestEnvGuard::with_vars(&[("HOME", home.path().to_str().unwrap())]);
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let (mut daemon, path) = daemon_with_session_metadata(&cas_dir, "focus-last-wins");
    let store_worker = super::store_worker::global();
    let start = |daemon: &mut FactoryDaemon, epic: &str| {
        daemon
            .app
            .set_session_focus_for_test(epic, crate::ui::factory::app::EpicFocusSource::SessionDefault);
        daemon.app.handle_epic_events(&[DirectorEvent::EpicStarted {
            epic_id: epic.into(),
            epic_title: epic.into(),
        }]);
    };
    start(&mut daemon, "cas-aaa1");
    daemon.flush_epic_focus_write(store_worker);
    daemon.app.handle_epic_events(&[DirectorEvent::EpicCompleted { epic_id: "cas-aaa1".into() }]);
    daemon.flush_epic_focus_write(store_worker);
    // Two changes before the next hand-off: only the last is written.
    start(&mut daemon, "cas-bbb2");
    daemon.app.reset_epic_state();
    start(&mut daemon, "cas-ccc3");
    daemon.flush_epic_focus_write(store_worker);
    assert!(store_worker.wait_idle(Duration::from_secs(30)));
    assert_eq!(persisted_epic(&path).as_deref(), Some("cas-ccc3"));

    daemon.app.reset_epic_state();
    daemon.flush_epic_focus_write(store_worker);
    assert!(store_worker.wait_idle(Duration::from_secs(30)));
    assert_eq!(persisted_epic(&path), None, "a later clear wins too");
}

/// cas-5e72: a worker exit is handled inside the daemon's enforced PTY
/// exchange. Removing the worker from the detector clears its durable hold
/// on the store worker, never on the lane.
#[test]
fn c5e72_removed_worker_hold_clears_off_the_loop() {
    let home = tempfile::tempdir().unwrap();
    let _env = crate::test_support::TestEnvGuard::with_vars(&[("HOME", home.path().to_str().unwrap())]);
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let (mut daemon, path) = daemon_with_session_metadata(&cas_dir, "hold-off-loop");
    crate::ui::factory::persist_session_metadata_worker_hold_at(&path, "gone-worker", true).unwrap();
    let progress = super::loop_watchdog::LoopProgress::new();
    {
        let (attachment, enforced) = super::loop_watchdog::enforced_daemon_step(
            &progress,
            super::loop_watchdog::LoopPhase::PtyOutput,
        );
        let pass = LoopPass::counting();
        daemon.app.remove_worker_from_event_detector("gone-worker");
        pass.assert_no_io("clearing a removed worker's hold");
        drop(enforced);
        drop(attachment);
    }
    assert!(super::store_worker::global().wait_idle(Duration::from_secs(30)));
    let metadata: crate::ui::factory::protocol::SessionMetadata =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(metadata.held_workers.is_empty(), "the hold was cleared on the store worker");
}

/// cas-26c8 review L1 (wiring): a death group whose folded relay keeps
/// asking for a retry stays held under its own key, while a later lone death
/// is written and settles on the same pass instead of waiting behind it.
#[tokio::test]
async fn cas_26c8_a_retrying_group_does_not_hold_back_a_later_death() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    // The stuck group's relay never gets its prompt queued; a lone death relays.
    daemon.relay_jobs.death_group_override = Some(|_, _, group, _| group.len() < 2);
    let t0 = chrono::Utc::now() - chrono::Duration::seconds(60);
    let exit = |worker: &str, offset_ms: i64| super::ObservedWorkerExit {
        at: t0 + chrono::Duration::milliseconds(offset_ms),
        worker: worker.into(),
        status: "exit 1".into(),
        death: None,
        recording: false,
    };
    daemon.recent_worker_exits = vec![exit("stuck-a", 0), exit("stuck-b", 500), exit("lone-c", 20_000)];

    daemon.relay_correlated_worker_deaths();
    settle_relays(&mut daemon).await;
    assert_eq!(
        daemon.recent_worker_exits.iter().map(|exit| exit.worker.as_str()).collect::<Vec<_>>(),
        ["stuck-a", "stuck-b"],
        "the lone death settled; only the retrying group is still held"
    );
    daemon.relay_correlated_worker_deaths();
    settle_relays(&mut daemon).await;
    assert_eq!(daemon.recent_worker_exits.len(), 2, "the group is retried, not dropped");
}

/// cas-54a2: a focus change made just before shutdown (outside the refresh
/// pass that hands writes off) is persisted by the shutdown path.
#[test]
fn c54a2_focus_change_just_before_shutdown_persists() {
    let home = tempfile::tempdir().unwrap();
    let _env = crate::test_support::TestEnvGuard::with_vars(&[("HOME", home.path().to_str().unwrap())]);
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let (mut daemon, path) = daemon_with_session_metadata(&cas_dir, "focus-at-shutdown");
    daemon
        .app
        .set_session_focus_for_test("cas-5d0e", crate::ui::factory::app::EpicFocusSource::SessionDefault);
    daemon.app.handle_epic_events(&[crate::ui::factory::director::DirectorEvent::EpicStarted {
        epic_id: "cas-5d0e".into(),
        epic_title: "last moment".into(),
    }]);
    // No refresh pass hands the write off before shutdown.
    assert!(daemon.finish_shutdown_writes(super::store_worker::global(), Duration::from_secs(30)));
    assert_eq!(persisted_epic(&path).as_deref(), Some("cas-5d0e"));
}

/// cas-54a2: the shutdown drain is bounded; a wedged store write cannot
/// hang shutdown.
#[test]
fn c54a2_shutdown_drain_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut daemon = super::provisioning_tests::daemon(&cas_dir);
    let worker = super::store_worker::StoreWorker::start();
    let (release, wedged) = std::sync::mpsc::channel::<()>();
    assert!(worker.submit("wedged write", move || {
        let _ = wedged.recv();
    }));
    let started = Instant::now();
    assert!(!daemon.finish_shutdown_writes(&worker, Duration::from_millis(50)));
    assert!(started.elapsed() < Duration::from_secs(10), "shutdown waited {:?}", started.elapsed());
    release.send(()).unwrap();
}

/// cas-ff42 C6: exit handling runs inside the loop's enforced PTY-exchange
/// step. The crash-time stale mark and death record are written on the store
/// worker, so the step stays clean; the exit waits, recording, until they
/// report.
#[tokio::test]
async fn cas_ff42_crash_stale_mark_writes_nothing_inside_the_enforced_step() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    daemon.queue_jobs.store(&cas_dir).unwrap();
    let at = chrono::Utc::now();
    let progress = super::loop_watchdog::LoopProgress::new();
    {
        let lane = super::loop_watchdog::enforced_daemon_step(
            &progress,
            super::loop_watchdog::LoopPhase::ClientInput,
        );
        let store_free = cas_store::wait_budget::forbid_store_access();
        let before = cas_store::lane_guard::thread_violations();
        daemon.recent_worker_exits.push(super::ObservedWorkerExit {
            at,
            worker: relay_worker().into(),
            status: "exited with code 1".into(),
            death: None,
            recording: true,
        });
        daemon.mark_registered_worker_stale(
            relay_worker(),
            "exited with code 1",
            None,
            crate::mcp::tools::service::orphan_recovery::DeathRelay::Deferred,
            Some(at),
            None,
        );
        assert_eq!(
            cas_store::lane_guard::thread_violations(),
            before,
            "the stale mark ran blocking work in the enforced step: {:?}",
            cas_store::lane_guard::last_thread_violation()
        );
        drop(store_free);
        drop(lane);
    }
    settle_relays(&mut daemon).await;
    let exit = &daemon.recent_worker_exits[0];
    assert!(!exit.recording, "the record reported");
    let agent = crate::store::open_agent_store(&cas_dir).unwrap().get("w-1").unwrap();
    assert!(exit.death.is_some(), "the death was recorded on the store worker: {exit:?}; w-1 {agent:?}");
    assert_eq!(agent.status, cas_types::AgentStatus::Stale, "the registration is stale");
}

/// cas-ff42 C6 control: the old shape, writing the death record inline in
/// the enforced step, trips the lane guard. This is what makes the test above
/// a real proof rather than a guard that never fires.
#[tokio::test]
#[should_panic(expected = "enforce mode: move it to the jobs lane")]
async fn cas_ff42_inline_death_record_trips_the_enforced_step() {
    let (_dir, cas_dir) = project_with_worker();
    let progress = super::loop_watchdog::LoopProgress::new();
    let _lane = super::loop_watchdog::enforced_daemon_step(
        &progress,
        super::loop_watchdog::LoopPhase::ClientInput,
    );
    let _store_free = cas_store::wait_budget::forbid_store_access();
    super::queue_and_events::record_worker_death_io(
        &cas_dir,
        "provision-test",
        relay_worker(),
        Some("w-1"),
        "exited with code 1",
        None,
        crate::mcp::tools::service::orphan_recovery::DeathRelay::Deferred,
        None,
    );
}

/// cas-ff42 review M: a death record job that panics reports nothing. Its
/// exit is settled without a death instead of staying `recording`, so its
/// group and the sweep's hold on that worker are released.
#[tokio::test]
async fn cas_ff42_a_panicked_death_record_does_not_hold_its_exit_forever() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    daemon.relay_jobs.death_record_override = Some(|_, _, _, _, _, _, _, _| panic!("record job panicked"));
    let at = chrono::Utc::now() - chrono::Duration::seconds(60);
    daemon.recent_worker_exits.push(super::ObservedWorkerExit {
        at,
        worker: relay_worker().into(),
        status: "exited with code 1".into(),
        death: None,
        recording: true,
    });
    daemon.mark_registered_worker_stale(
        relay_worker(),
        "exited with code 1",
        None,
        crate::mcp::tools::service::orphan_recovery::DeathRelay::Deferred,
        Some(at),
        None,
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !daemon.recent_worker_exits.is_empty() {
        daemon.relay_correlated_worker_deaths();
        assert!(Instant::now() < deadline, "the exit stayed recording: {:?}", daemon.recent_worker_exits);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    settle_relays(&mut daemon).await;
}

/// cas-58a3: the whole worker-exit path writes nothing inside the loop's
/// enforced PTY exchange (C6): the stale mark and death record, the reap and
/// worktree check, and the boot-time spawn notice all run as jobs. The exit is
/// then recorded, cleaned and relayed in that order.
#[tokio::test]
async fn cas_58a3_worker_exit_writes_nothing_inside_the_enforced_step() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    daemon.queue_jobs.store(&cas_dir).unwrap();
    daemon.app.add_worker_name_for_test(relay_worker());
    daemon.spawn_verifications.insert(
        relay_worker().to_string(),
        super::super::SpawnVerification {
            request_id: Some(7),
            launched_at: Instant::now(),
            registered_at: None,
            task_id: None,
            launched_wall: std::time::SystemTime::now(),
        },
    );
    // cas-031e integration: worker-exit cleanup also cancels evidence owned
    // by that pane, without restoring any inline crash I/O.
    let probe = super::queue_and_events::NormalDeliveryProbe {
        pane: relay_worker().into(),
        target: relay_worker().into(),
        bytes_at_delivery: 0,
        delivered_at: Instant::now(),
        delivered_at_utc: chrono::Utc::now(),
        prompt: "pending delivery".into(),
        nudge_sent_at: None,
        nudges: 0,
        next_check_at: None,
    };
    let live = daemon.queue_jobs.normal_probe_jobs.track(42, &probe);
    daemon.normal_delivery_probes.insert(42, probe);
    let progress = super::loop_watchdog::LoopProgress::new();
    {
        let lane = super::loop_watchdog::enforced_daemon_step(
            &progress,
            super::loop_watchdog::LoopPhase::ClientInput,
        );
        let store_free = cas_store::wait_budget::forbid_store_access();
        let before = cas_store::lane_guard::thread_violations();
        let store_before = cas_store::wait_budget::store_access_violations();
        daemon
            .handle_worker_crash(relay_worker(), Some(1), None)
            .await
            .unwrap();
        assert_eq!(
            cas_store::lane_guard::thread_violations(),
            before,
            "the exit path ran blocking work in the enforced step: {:?}",
            cas_store::lane_guard::last_thread_violation()
        );
        assert_eq!(
            cas_store::wait_budget::store_access_violations(),
            store_before,
            "the exit path touched the store in the enforced step: {:?}",
            cas_store::wait_budget::last_store_access_violation()
        );
        drop(store_free);
        drop(lane);
    }
    assert!(!live.load(std::sync::atomic::Ordering::Acquire), "old evidence was cancelled");
    assert!(!daemon.normal_delivery_probes.contains_key(&42), "old pane probe was removed");
    assert!(!daemon.app.worker_names().iter().any(|name| name == relay_worker()));
    // The record reports on a drain; its cleanup is submitted by the next
    // relay pass, outside the drained batch, as the loop's tick does.
    let deadline = Instant::now() + Duration::from_secs(30);
    while daemon.app.crash_cleanup_pending(relay_worker()) {
        daemon.relay_correlated_worker_deaths();
        assert!(Instant::now() < deadline, "the cleanup never applied");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    settle_relays(&mut daemon).await;
    let exit = &daemon.recent_worker_exits[0];
    assert!(!exit.recording, "recorded and cleaned");
    let agent = crate::store::open_agent_store(&cas_dir).unwrap().get("w-1").unwrap();
    assert!(exit.death.is_some(), "the death was recorded off the loop: {exit:?}; w-1 {agent:?}");
    assert_eq!(agent.status, cas_types::AgentStatus::Stale);
    let row = crate::store::open_prompt_queue_store(&cas_dir)
        .unwrap()
        .peek_all(10)
        .unwrap()
        .into_iter()
        .any(|row| row.prompt.contains(relay_worker()));
    assert!(row, "the boot-time spawn notice was written off the loop");
}

/// cas-58a3: recorded, then crashed, then relayed. While the crashed
/// worker's cleanup still runs, its exit stays recording and is not relayed.
#[tokio::test]
async fn cas_58a3_an_exit_waits_for_its_crash_cleanup_before_it_is_relayed() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    daemon.relay_jobs.crash_cleanup_override = Some(|_| {
        std::thread::sleep(Duration::from_millis(400));
        crate::ui::factory::app::render_and_ops::epic_workers::CrashCleanupOutcome { worktree: None }
    });
    daemon.app.add_worker_name_for_test(relay_worker());
    // Dated at the crash, after the worker registered, as production does.
    let at = chrono::Utc::now();
    daemon.recent_worker_exits.push(super::ObservedWorkerExit {
        at,
        worker: relay_worker().into(),
        status: "exited with code 1".into(),
        death: None,
        recording: true,
    });
    let cleanup = daemon.app.mark_worker_crashed_in_memory(relay_worker());
    daemon.mark_registered_worker_stale(
        relay_worker(),
        "exited with code 1",
        None,
        crate::mcp::tools::service::orphan_recovery::DeathRelay::Deferred,
        Some(at),
        Some(cleanup),
    );
    let cleanup_key = super::queue_and_events::worker_crash_cleanup_key(relay_worker(), at);
    let deadline = Instant::now() + Duration::from_secs(30);
    while !daemon.relay_jobs.writing(&cleanup_key) {
        daemon.relay_correlated_worker_deaths();
        assert!(Instant::now() < deadline, "the cleanup never started");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    daemon.relay_correlated_worker_deaths();
    let exit = &daemon.recent_worker_exits[0];
    assert!(exit.death.is_some(), "recorded first");
    assert!(exit.recording, "still cleaning, so not ready");
    assert!(
        !daemon.relay_jobs.writing(&super::queue_and_events::death_group_key(
            &daemon.recent_worker_exits
        )),
        "not relayed while its cleanup runs"
    );
    while !daemon.recent_worker_exits.is_empty() {
        daemon.relay_correlated_worker_deaths();
        assert!(Instant::now() < deadline, "the exit was never relayed");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    settle_relays(&mut daemon).await;
}

/// cas-58a3 N1: a death group's result handled before a death record's
/// result in the same drained batch must not settle that record's exit as
/// vanished (take() frees the whole batch's keys before applying them).
#[tokio::test]
async fn cas_58a3_a_same_batch_record_is_not_mistaken_for_a_vanished_one() {
    use super::relay_jobs::RelayOutput;
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let at = chrono::Utc::now() - chrono::Duration::seconds(60);
    daemon.recent_worker_exits.push(super::ObservedWorkerExit {
        at,
        worker: relay_worker().into(),
        status: "exited with code 1".into(),
        death: None,
        recording: true,
    });
    // First in the batch: a death group's report, whose settle advances.
    assert!(daemon.relay_jobs.submit_write(&mut daemon.queue_jobs, "earlier-group", None, || {
        RelayOutput::DeathsRelayed { group: Vec::new(), done: true }
    }));
    // Second: this exit's death record.
    let record_key = super::queue_and_events::worker_death_record_key(relay_worker(), at);
    let record_key_out = record_key.clone();
    assert!(daemon.relay_jobs.submit_write(&mut daemon.queue_jobs, &record_key, None, move || {
        RelayOutput::WorkerDeathRecorded {
            worker: relay_worker().into(),
            at: Some(at),
            death: None,
            record_key: record_key_out,
        }
    }));
    // cas-58a3 review M3: both finish before one drain takes them as a
    // single batch; wait for both, not for a fixed time.
    let deadline = Instant::now() + Duration::from_secs(30);
    while daemon.queue_jobs.external_completions_ready() < 2 {
        assert!(Instant::now() < deadline, "the two jobs never finished");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let group_key = super::queue_and_events::death_group_key(&daemon.recent_worker_exits);
    daemon.drain_relay_jobs();
    assert!(
        !daemon.relay_jobs.writing(&group_key),
        "the exit was relayed mid-batch as if its record had vanished"
    );
    assert_eq!(
        daemon.recent_worker_exits.len(),
        1,
        "the exit is still held: nothing relayed it before its record applied"
    );
    assert!(!daemon.recent_worker_exits[0].recording, "the record itself settled it");
    settle_relays(&mut daemon).await;
}

/// cas-58a3 review H1: a respawn or named spawn of a crashed worker's name
/// waits until that worker's late reap and worktree check are applied;
/// otherwise the old worker's cleanup would act on the new one.
#[tokio::test]
async fn cas_58a3_a_respawn_waits_for_the_crashed_workers_cleanup() {
    static RELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    daemon.relay_jobs.crash_cleanup_override = Some(|_| {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !RELEASE.load(std::sync::atomic::Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        crate::ui::factory::app::render_and_ops::epic_workers::CrashCleanupOutcome { worktree: None }
    });
    daemon.app.add_worker_name_for_test(relay_worker());
    daemon.handle_worker_crash(relay_worker(), Some(1), None).await.unwrap();
    let at = daemon.recent_worker_exits[0].at;
    let cleanup_key = super::queue_and_events::worker_crash_cleanup_key(relay_worker(), at);
    let deadline = Instant::now() + Duration::from_secs(30);
    while !daemon.relay_jobs.writing(&cleanup_key) {
        daemon.relay_correlated_worker_deaths();
        assert!(Instant::now() < deadline, "the cleanup never started");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(daemon.app.crash_cleanup_pending(relay_worker()), "reserved while it cleans");

    daemon
        .pending_spawns
        .push_back(super::super::PendingSpawn::Respawn(relay_worker().into()));
    daemon.process_pending_spawns().await;
    assert!(daemon.spawn_task.is_none(), "no respawn started while the cleanup runs");
    assert!(
        daemon
            .pending_spawns
            .iter()
            .any(|spawn| matches!(spawn, super::super::PendingSpawn::Respawn(name) if name == relay_worker())),
        "the respawn was requeued, not dropped"
    );
    assert!(
        daemon.app.snapshot_worker_spawn(Some(relay_worker()), false, None).is_err(),
        "a named spawn treats the name as busy"
    );

    RELEASE.store(true, std::sync::atomic::Ordering::Release);
    while daemon.app.crash_cleanup_pending(relay_worker()) {
        daemon.relay_correlated_worker_deaths();
        assert!(Instant::now() < deadline, "the cleanup never applied");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(daemon.relay_jobs.crash_cleanups.is_empty());
    assert!(daemon.app.snapshot_worker_spawn(Some(relay_worker()), false, None).is_ok(), "released");
    // The requeued respawn now runs instead of waiting again. (This test
    // daemon has no harness, so the relaunch itself fails; what matters is
    // that it was attempted, not refused as still cleaning.)
    daemon.app.clear_error();
    daemon.process_pending_spawns().await;
    assert!(
        !daemon
            .pending_spawns
            .iter()
            .any(|spawn| matches!(spawn, super::super::PendingSpawn::Respawn(name) if name == relay_worker())),
        "the respawn was taken once the cleanup applied"
    );
    assert!(
        daemon
            .app
            .error_message
            .as_deref()
            .is_none_or(|error| !error.contains("still being cleaned up")),
        "the respawn ran past the reservation: {:?}",
        daemon.app.error_message
    );
    settle_relays(&mut daemon).await;
}

/// cas-58a3 review L-a: two crash cleanups of the same worker (the boot
/// failure path and the PTY exit both mark it) each hold the name; the first
/// to finish does not release it while the second is still pending.
#[test]
fn cas_58a3_a_name_stays_reserved_while_another_cleanup_of_it_is_pending() {
    use crate::ui::factory::app::render_and_ops::epic_workers::CrashCleanupOutcome;
    let (_dir, cas_dir) = project_with_worker();
    let mut app = crate::ui::factory::app::FactoryApp::for_test_at(cas_dir);
    app.add_worker_name_for_test(relay_worker());
    let _first = app.mark_worker_crashed_in_memory(relay_worker());
    let _second = app.mark_worker_crashed_in_memory(relay_worker());
    app.apply_crash_cleanup(relay_worker(), CrashCleanupOutcome { worktree: None });
    assert!(app.crash_cleanup_pending(relay_worker()), "the second cleanup still holds it");
    app.abandon_crash_cleanup(relay_worker());
    assert!(!app.crash_cleanup_pending(relay_worker()), "released after the last one");
}

/// cas-58a3 review M1: a death record job that panics still lets the crashed
/// worker's reap run, and the exit is released by the cleanup, not lost.
#[tokio::test]
async fn cas_58a3_a_panicked_death_record_still_runs_the_crash_cleanup() {
    static CLEANED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    daemon.relay_jobs.death_record_override = Some(|_, _, _, _, _, _, _, _| panic!("record job panicked"));
    daemon.relay_jobs.crash_cleanup_override = Some(|_| {
        CLEANED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        crate::ui::factory::app::render_and_ops::epic_workers::CrashCleanupOutcome { worktree: None }
    });
    daemon.app.add_worker_name_for_test(relay_worker());
    daemon.handle_worker_crash(relay_worker(), Some(1), None).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while daemon.app.crash_cleanup_pending(relay_worker())
        || daemon.recent_worker_exits.first().is_some_and(|exit| exit.recording)
    {
        daemon.relay_correlated_worker_deaths();
        assert!(Instant::now() < deadline, "the reap was lost with the record");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(CLEANED.load(std::sync::atomic::Ordering::SeqCst), 1, "the cleanup ran exactly once");
    assert!(daemon.relay_jobs.crash_cleanups.is_empty());
    settle_relays(&mut daemon).await;
}

/// cas-58a3 review M1: a cleanup job that keeps panicking is retried, then
/// given up loudly; the name and exit are released instead of held forever.
#[tokio::test]
async fn cas_58a3_a_cleanup_that_keeps_failing_is_given_up_and_releases_the_name() {
    static TRIES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    daemon.relay_jobs.crash_cleanup_override = Some(|_| {
        TRIES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        panic!("cleanup job panicked")
    });
    daemon.app.add_worker_name_for_test(relay_worker());
    daemon.handle_worker_crash(relay_worker(), Some(1), None).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while daemon.app.crash_cleanup_pending(relay_worker())
        || daemon.recent_worker_exits.first().is_some_and(|exit| exit.recording)
    {
        daemon.relay_correlated_worker_deaths();
        assert!(Instant::now() < deadline, "a failing cleanup held the name forever");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        TRIES.load(std::sync::atomic::Ordering::SeqCst),
        super::queue_and_events::CRASH_CLEANUP_MAX_ATTEMPTS,
        "retried up to the cap"
    );
    assert!(daemon.relay_jobs.crash_cleanups.is_empty());
    settle_relays(&mut daemon).await;
}

/// A git repo with `worker`'s worktree tracked by a manager (cas-58a3 M2).
fn worktree_fixture(worker: &str) -> (tempfile::TempDir, crate::worktree::WorktreeManager) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git").args(args).current_dir(&repo).output().unwrap();
        assert!(output.status.success(), "fixture git failed: {}", String::from_utf8_lossy(&output.stderr));
    };
    git(&["init"]);
    git(&["config", "user.email", "test@test.com"]);
    git(&["config", "user.name", "Test"]);
    std::fs::write(repo.join("README.md"), "# fixture").unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "init"]);
    let mut manager = crate::worktree::WorktreeManager::new(
        &repo,
        crate::worktree::WorktreeConfig {
            enabled: true,
            base_path: repo.join(".cas/worktrees").to_string_lossy().into_owned(),
            branch_prefix: "factory/".into(),
            auto_merge: false,
            cleanup_on_close: false,
            promote_entries_on_merge: false,
        },
    )
    .unwrap();
    manager.create_for_worker(worker).unwrap();
    (dir, manager)
}

/// cas-58a3 review M2 control: the old shape, reaping and checking the
/// worktree inline in the enforced step, trips the guard. With a tracked
/// worktree the inline path reaches the store, so the proof below is real.
#[tokio::test]
#[should_panic(expected = "enforce mode: move it to the jobs lane")]
async fn cas_58a3_inline_crash_cleanup_trips_the_enforced_step() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let (_repo, manager) = worktree_fixture(relay_worker());
    daemon.app.set_worktree_manager_for_test(manager);
    daemon.app.add_worker_name_for_test(relay_worker());
    let progress = super::loop_watchdog::LoopProgress::new();
    let _lane = super::loop_watchdog::enforced_daemon_step(
        &progress,
        super::loop_watchdog::LoopPhase::ClientInput,
    );
    let _store_free = cas_store::wait_budget::forbid_store_access();
    daemon.app.mark_worker_crashed(relay_worker()).await;
}

/// cas-58a3 review M2: with a live tracked process group and a tracked
/// worktree, the exit path still does nothing inside the enforced step; the
/// group is then reaped and the clean worktree reclaimed off the loop.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn cas_58a3_exit_reaps_the_group_and_worktree_off_the_enforced_step() {
    use std::os::unix::process::CommandExt;
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    daemon.queue_jobs.store(&cas_dir).unwrap();
    let (_repo, manager) = worktree_fixture(relay_worker());
    let worktree = manager.worker_removal_plan(relay_worker()).unwrap().worktree.path;
    daemon.app.set_worktree_manager_for_test(manager);
    daemon.app.add_worker_name_for_test(relay_worker());
    let mut command = std::process::Command::new("sh");
    command.args(["-c", "sleep 300 & wait"]);
    // SAFETY: setsid is async-signal-safe and runs in the child between fork
    // and exec, isolating the synthetic worker from the test's group.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    let session = daemon.app.factory_session().unwrap_or("provision-test").to_string();
    let record =
        crate::ui::factory::process_groups::track(&cas_dir, relay_worker(), &session, child.id()).unwrap();
    assert!(crate::ui::factory::process_groups::is_live(&record));

    let progress = super::loop_watchdog::LoopProgress::new();
    {
        let lane = super::loop_watchdog::enforced_daemon_step(
            &progress,
            super::loop_watchdog::LoopPhase::ClientInput,
        );
        let store_free = cas_store::wait_budget::forbid_store_access();
        let before = cas_store::lane_guard::thread_violations();
        let store_before = cas_store::wait_budget::store_access_violations();
        daemon.handle_worker_crash(relay_worker(), Some(1), None).await.unwrap();
        assert_eq!(
            cas_store::lane_guard::thread_violations(),
            before,
            "the exit path ran blocking work in the enforced step: {:?}",
            cas_store::lane_guard::last_thread_violation()
        );
        assert_eq!(
            cas_store::wait_budget::store_access_violations(),
            store_before,
            "the exit path touched the store in the enforced step: {:?}",
            cas_store::wait_budget::last_store_access_violation()
        );
        drop(store_free);
        drop(lane);
    }
    assert!(crate::ui::factory::process_groups::is_live(&record), "not reaped on the loop");
    assert!(worktree.exists(), "not removed on the loop");

    let deadline = Instant::now() + Duration::from_secs(30);
    while daemon.app.crash_cleanup_pending(relay_worker()) {
        daemon.relay_correlated_worker_deaths();
        assert!(Instant::now() < deadline, "the cleanup never applied");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let _ = child.wait();
    assert!(!crate::ui::factory::process_groups::is_live(&record), "the group was reaped");
    assert!(crate::ui::factory::process_groups::list(&cas_dir).unwrap().is_empty(), "its record dropped");
    assert!(!worktree.exists(), "the clean worktree was reclaimed");
    assert!(
        daemon
            .app
            .worktree_manager()
            .and_then(|manager| manager.worker_removal_plan(relay_worker()))
            .is_none(),
        "and forgotten"
    );
    settle_relays(&mut daemon).await;
}
