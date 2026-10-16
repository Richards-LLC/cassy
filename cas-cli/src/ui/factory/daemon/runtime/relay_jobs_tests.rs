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

/// A project with this factory session's active worker registered, so the
/// relays' reads have a candidate to look at.
fn project_with_worker() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
    let mut worker = cas_types::Agent::new("w-1".into(), "relay-worker".into());
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

#[tokio::test]
async fn c4491f_usage_limit_relay_reads_nothing_on_the_loop() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    daemon.app.add_worker_name_for_test("relay-worker");
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
    daemon.app.add_worker_name_for_test("relay-worker");
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
    daemon.app.add_worker_name_for_test("relay-worker");
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
    task.assignee = Some("relay-worker".into());
    crate::store::open_task_store_local(&cas_dir).unwrap().add(&task).unwrap();
    let mut daemon = daemon_on_lanes(&cas_dir);
    let progress = super::loop_watchdog::LoopProgress::new();
    let store_worker = super::store_worker::global();
    let mut refresh = DirectorRefresh::new();
    let wake = crate::ui::factory::director::Prompt {
        target: "relay-worker".into(),
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

/// A prompt the inject path sees after its re-check never reported.
fn unread_batch(text: &str) -> super::factory_jobs::DeliverabilityBatch {
    super::factory_jobs::DeliverabilityBatch {
        prompts: vec![(
            crate::ui::factory::director::Prompt {
                target: "relay-worker".into(),
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
    task.assignee = Some("relay-worker".into());
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
        .filter(|row| row.target == "relay-worker")
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
    task.assignee = Some("relay-worker".into());
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
            .any(|row| row.source == "task-assignment" && row.target == "relay-worker"),
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

/// cas-26c8 review L1 (wiring): a death group whose folded relay keeps
/// asking for a retry stays held under its own key, while a later lone death
/// is written and settles on the same pass instead of waiting behind it.
#[tokio::test]
async fn cas_26c8_a_retrying_group_does_not_hold_back_a_later_death() {
    let (_dir, cas_dir) = project_with_worker();
    let mut daemon = daemon_on_lanes(&cas_dir);
    // The stuck group's relay never gets its prompt queued; a lone death relays.
    daemon.relay_jobs.death_group_override = Some(|_, group, _| group.len() < 2);
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
            worker: "relay-worker".into(),
            status: "exited with code 1".into(),
            death: None,
            recording: true,
        });
        daemon.mark_registered_worker_stale(
            "relay-worker",
            "exited with code 1",
            None,
            crate::mcp::tools::service::orphan_recovery::DeathRelay::Deferred,
            Some(at),
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
    assert!(exit.death.is_some(), "the death was recorded on the store worker");
    let agent = crate::store::open_agent_store(&cas_dir).unwrap().get("w-1").unwrap();
    assert_eq!(agent.status, cas_types::AgentStatus::Stale, "the registration is stale");
}

/// cas-ff42 C6 control: the old shape, writing the death record inline in
/// the enforced step, trips the lane guard. This is what makes the test above
/// a real proof rather than a guard that never fires.
#[tokio::test]
#[should_panic]
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
        "relay-worker",
        Some("w-1"),
        "exited with code 1",
        None,
        crate::mcp::tools::service::orphan_recovery::DeathRelay::Deferred,
    );
}
