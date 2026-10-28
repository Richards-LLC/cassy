//! Factory loop v2 C3 gate tests (cas-d2a2, spec-4c5b §7).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use cas_store::PromptQueueStore;

use super::super::jobs::{LaneWiring, WriteExecutor};
use super::super::teams::InboxPruner;
use super::*;

type WriterJob = Box<dyn FnOnce() + Send + 'static>;

/// A single writer thread, like the StoreWorker, that records job entry before
/// the job can publish completion. With `gated`, jobs wait until [`TestWriter::open`].
pub(in super::super) struct TestWriter {
    sender: Mutex<mpsc::Sender<(&'static str, WriterJob)>>,
    ran: Arc<Mutex<Vec<(&'static str, String)>>>,
    gate: Arc<(Mutex<bool>, Condvar)>,
}

impl TestWriter {
    pub(in super::super) fn start(gated: bool) -> Arc<Self> {
        let (sender, receiver) = mpsc::channel::<(&'static str, WriterJob)>();
        let ran = Arc::new(Mutex::new(Vec::new()));
        let gate = Arc::new((Mutex::new(!gated), Condvar::new()));
        let thread_ran = Arc::clone(&ran);
        let thread_gate = Arc::clone(&gate);
        std::thread::Builder::new()
            .name("test-store-writer".into())
            .spawn(move || {
                for (label, job) in receiver {
                    {
                        let (open, ready) = &*thread_gate;
                        let mut open = open.lock().unwrap();
                        while !*open {
                            open = ready.wait(open).unwrap();
                        }
                    }
                    let thread = std::thread::current().name().unwrap_or("").to_string();
                    thread_ran.lock().unwrap().push((label, thread));
                    job();
                }
            })
            .unwrap();
        Arc::new(Self {
            sender: Mutex::new(sender),
            ran,
            gate,
        })
    }

    pub(in super::super) fn open(&self) {
        let (open, ready) = &*self.gate;
        *open.lock().unwrap() = true;
        ready.notify_all();
    }

    pub(in super::super) fn ran(&self) -> Vec<(&'static str, String)> {
        self.ran.lock().unwrap().clone()
    }
}

struct SharedWriter(Arc<TestWriter>);

impl WriteExecutor for SharedWriter {
    fn execute(&self, label: &'static str, job: Box<dyn FnOnce() + Send + 'static>) -> bool {
        self.0.sender.lock().unwrap().send((label, job)).is_ok()
    }
}

pub(in super::super) fn jobs_on(writer: &Arc<TestWriter>) -> QueueJobs {
    QueueJobs::start_with(LaneWiring {
        reader: None,
        writer: Arc::new(SharedWriter(Arc::clone(writer))),
    })
}

fn project() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let cas_dir = crate::store::init_cas_dir(root.path()).unwrap();
    (root, cas_dir)
}

fn enqueue(cas_dir: &Path, target: &str, text: &str) -> i64 {
    crate::store::open_prompt_queue_store(cas_dir)
        .unwrap()
        .enqueue_with_session("supervisor", target, text, "s")
        .unwrap()
}

fn request(cas_dir: &Path) -> QueueSelectRequest {
    QueueSelectRequest {
        cas_dir: cas_dir.to_path_buf(),
        session_name: "s".into(),
        supervisor_name: "lead".into(),
        worker_names: vec!["worker-a".into()],
        dead_workers: Default::default(),
        limit: 10,
        operator_replies: false,
        deferred_operator_replies: Default::default(),
        adopt_operator_notices: false,
    }
}

/// Apply completions until `done` holds or ten seconds pass.
fn settle(jobs: &mut QueueJobs, mut done: impl FnMut(&mut QueueJobs) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        jobs.apply_completions();
        if done(jobs) {
            return;
        }
        assert!(Instant::now() < deadline, "queue jobs did not finish in time");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn claim_source(cas_dir: &Path, row: i64, recipient: &str) -> Option<String> {
    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
    conn.query_row(
        "SELECT source FROM prompt_queue_recipient_seen WHERE prompt_id = ? AND recipient = ?",
        rusqlite::params![row, recipient],
        |r| r.get(0),
    )
    .ok()
}

fn store_claim(cas_dir: &Path, row: i64, recipient: &str) -> anyhow::Result<Option<cas_store::ClaimToken>> {
    Ok(crate::store::open_prompt_queue_store(cas_dir)?.claim_recipient_transport_token(row, recipient)?)
}

fn receipts(cas_dir: &Path) -> i64 {
    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
    conn.query_row("SELECT COUNT(*) FROM prompt_queue_recipient_seen", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn flv2_queue_stamps_on_writer() {
    let (_root, cas_dir) = project();
    let delivered = enqueue(&cas_dir, "worker-a", "first");
    let released = enqueue(&cas_dir, "worker-a", "second");
    let writer = TestWriter::start(false);
    let mut jobs = jobs_on(&writer);

    // QueueSelect runs its store work on the writer and returns the batch.
    let select_threads = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&select_threads);
    jobs.submit_select(request(&cas_dir), move |request, generation| {
        recorded
            .lock()
            .unwrap()
            .push(std::thread::current().name().unwrap_or("").to_string());
        super::super::queue_and_events::run_queue_select(request, generation)
    });
    assert!(jobs.select_in_flight());
    settle(&mut jobs, |jobs| !jobs.select_in_flight());
    let selection = jobs.take_selection().expect("the selection lands as a completion");
    assert_eq!(
        select_threads.lock().unwrap().as_slice(),
        ["test-store-writer"],
        "the selection's store work ran on the writer, not the caller"
    );
    let ids: Vec<i64> = selection.rows.iter().map(|row| row.id).collect();
    assert_eq!(ids, vec![delivered, released]);
    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
    let selected_at: Option<String> = conn
        .query_row("SELECT selected_at FROM prompt_queue WHERE id = ?", [delivered], |r| r.get(0))
        .unwrap();
    assert!(selected_at.is_some(), "QueueSelect stamps the Selected stage");
    assert_eq!(receipts(&cas_dir), 0, "selection takes no claim (review round 1, H1)");

    // Control delivers the first row; the second is claimed, then gated.
    let cas = cas_dir.clone();
    let generation = match jobs
        .admit(delivered, "worker-a", Instant::now(), || store_claim(&cas, delivered, "worker-a"))
        .unwrap()
    {
        Admission::Inject { generation } => generation,
        other => panic!("the unclaimed row is admitted, got {other:?}"),
    };
    jobs.record_outcome(delivered, "worker-a", generation);
    jobs.stamp(&cas_dir, delivered, "worker-a", QueueStamp::TransportReceipt);
    jobs.stamp(&cas_dir, delivered, "worker-a", QueueStamp::TransportDelivered);
    let cas = cas_dir.clone();
    let gated = match jobs
        .admit(released, "worker-a", Instant::now(), || store_claim(&cas, released, "worker-a"))
        .unwrap()
    {
        Admission::Inject { generation } => generation,
        other => panic!("the second row is admitted, got {other:?}"),
    };
    jobs.release_inject_claim(&cas_dir, released, "worker-a");
    jobs.record_outcome(released, "worker-a", gated);
    jobs.settle_injects(&cas_dir);
    settle(&mut jobs, |jobs| jobs.in_flight().len() == 0);

    // Every selection and stamp ran on the writer thread, none on the caller.
    let ran = writer.ran();
    assert_eq!(
        ran.iter().map(|(label, _)| *label).collect::<Vec<_>>(),
        vec!["QueueSelect", "QueueStamp", "QueueStamp", "QueueStamp", "QueueSettle", "QueueSettle"],
        "the selection, then receipt, delivered, release and two settles: {ran:?}"
    );
    assert!(ran.iter().all(|(_, thread)| thread == "test-store-writer"), "{ran:?}");

    let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
    let row = queue.queued_prompt(delivered).unwrap().unwrap();
    assert!(row.processed_at.is_some(), "the delivered stamp landed");
    assert_eq!(claim_source(&cas_dir, released, "worker-a"), None, "the gated row's claim was released");
    let pending = queue.queued_prompt(released).unwrap().unwrap();
    assert!(pending.processed_at.is_none(), "the gated row stays pending");
}

#[test]
fn flv2_no_double_inject() {
    let (_root, cas_dir) = project();
    let row_id = enqueue(&cas_dir, "worker-a", "only once");
    // Stamps are held at the writer, so they have not landed while the row
    // is selected a second time.
    let writer = TestWriter::start(true);
    let mut jobs = jobs_on(&writer);

    let t0 = Instant::now();
    let mut injects = 0;
    for pass in 0..2 {
        let cas = cas_dir.clone();
        let issued = Arc::new(Mutex::new(None));
        let seen = Arc::clone(&issued);
        match jobs
            .admit(row_id, "worker-a", t0, move || {
                let token = store_claim(&cas, row_id, "worker-a")?;
                *seen.lock().unwrap() = token;
                Ok(token)
            })
            .unwrap()
        {
            Admission::Inject { generation } => {
                injects += 1;
                assert!(issued.lock().unwrap().is_some(), "the inject holds a real store claim");
                jobs.record_outcome(row_id, "worker-a", generation);
                jobs.stamp(&cas_dir, row_id, "worker-a", QueueStamp::TransportDelivered);
                // A repeated stamp for the same generation is not submitted.
                jobs.stamp(&cas_dir, row_id, "worker-a", QueueStamp::TransportDelivered);
            }
            Admission::InFlight => assert_eq!(pass, 1),
            Admission::NotClaimed => panic!("pass {pass}: nothing else holds the row"),
        }
        jobs.settle_injects(&cas_dir);
        jobs.apply_completions();
    }
    assert_eq!(injects, 1, "a row selected twice before its stamp lands is injected once");
    assert!(jobs.in_flight().holds_row(row_id));
    // Even without the in-flight set, the store claim refuses a second inject.
    assert_eq!(store_claim(&cas_dir, row_id, "worker-a").unwrap(), None);

    // A late release naming another claim never drops this one.
    jobs.stamp(
        &cas_dir,
        row_id,
        "worker-a",
        QueueStamp::ReleaseClaim { claim_token: cas_store::ClaimToken::never_issued() },
    );
    writer.open();
    settle(&mut jobs, |jobs| !jobs.in_flight().holds_row(row_id));
    settle(&mut jobs, |_| writer.ran().len() >= 3);
    assert_eq!(
        claim_source(&cas_dir, row_id, "worker-a").as_deref(),
        Some("transport_claimed"),
        "the delivered row keeps its claim"
    );
    let stale_release = crate::store::open_prompt_queue_store(&cas_dir)
        .unwrap()
        .release_recipient_transport_claim(row_id, "worker-a", cas_store::ClaimToken::never_issued())
        .unwrap();
    assert!(!stale_release, "a stale token releases nothing");
    assert_eq!(claim_source(&cas_dir, row_id, "worker-a").as_deref(), Some("transport_claimed"));

    // An inject whose outcome never arrives releases its own claim after the
    // outcome timeout (fake clock); the row can be claimed again.
    let lost = enqueue(&cas_dir, "worker-a", "outcome lost");
    let cas = cas_dir.clone();
    assert!(matches!(
        jobs.admit(lost, "worker-a", t0, move || store_claim(&cas, lost, "worker-a")).unwrap(),
        Admission::Inject { .. }
    ));
    jobs.settle_injects(&cas_dir);
    assert_eq!(jobs.expire_injects(&cas_dir, t0 + INJECT_OUTCOME_TIMEOUT - Duration::from_millis(1)), 0);
    assert!(jobs.in_flight().holds_row(lost));
    // A physical write still owned by the mux must not release its claim
    // and invite a second copy while the first writer is blocked.
    jobs.retain_owned_injects(t0 + INJECT_OUTCOME_TIMEOUT, |row| row == lost);
    assert_eq!(jobs.expire_injects(&cas_dir, t0 + INJECT_OUTCOME_TIMEOUT), 0);
    assert!(jobs.in_flight().holds_row(lost));
    // Once orphaned, the existing bounded missing-outcome timeout applies.
    assert_eq!(jobs.expire_injects(&cas_dir, t0 + 2 * INJECT_OUTCOME_TIMEOUT), 1);
    assert!(!jobs.in_flight().holds_row(lost));
    settle(&mut jobs, |_| claim_source(&cas_dir, lost, "worker-a").is_none());
    assert!(store_claim(&cas_dir, lost, "worker-a").unwrap().is_some());
}

/// Review round 1, H1: a selection that is never delivered (the daemon
/// stopped with it queued or unread, or crashed) leaves no claim, so the row
/// is still delivered, once, after restart.
#[test]
fn an_undelivered_selection_leaves_its_rows_deliverable() {
    let (_root, cas_dir) = project();
    let row_id = enqueue(&cas_dir, "worker-a", "survive a restart");

    // Shutdown with a select queued behind a held writer.
    let held = TestWriter::start(true);
    let mut stopping = jobs_on(&held);
    stopping.submit_select(request(&cas_dir), super::super::queue_and_events::run_queue_select);
    held.open();
    assert!(stopping.drain_for_shutdown(&cas_dir, Duration::from_secs(10)));
    drop(stopping);
    assert_eq!(receipts(&cas_dir), 0, "the unread selection holds no claim");

    // Crash after a selection landed but before delivery.
    let writer = TestWriter::start(false);
    let mut crashed = jobs_on(&writer);
    crashed.submit_select(request(&cas_dir), super::super::queue_and_events::run_queue_select);
    settle(&mut crashed, |jobs| !jobs.select_in_flight());
    assert!(crashed.take_selection().is_some());
    drop(crashed);
    assert_eq!(receipts(&cas_dir), 0);

    // After restart the row is selected again and delivered exactly once.
    let mut restarted = jobs_on(&writer);
    restarted.submit_select(request(&cas_dir), super::super::queue_and_events::run_queue_select);
    settle(&mut restarted, |jobs| !jobs.select_in_flight());
    let rows = restarted.take_selection().unwrap().rows;
    assert_eq!(rows.iter().map(|row| row.id).collect::<Vec<_>>(), vec![row_id]);
    assert!(store_claim(&cas_dir, row_id, "worker-a").unwrap().is_some());
    assert!(store_claim(&cas_dir, row_id, "worker-a").unwrap().is_none());
}

/// Review round 1, M5: a settle that never completes stops holding its row.
#[test]
fn an_unsettled_inject_stops_holding_its_row() {
    let (_root, cas_dir) = project();
    let row_id = enqueue(&cas_dir, "worker-a", "settle lost");
    let writer = TestWriter::start(true);
    let mut jobs = jobs_on(&writer);
    let cas = cas_dir.clone();
    let generation = match jobs
        .admit(row_id, "worker-a", Instant::now(), move || store_claim(&cas, row_id, "worker-a"))
        .unwrap()
    {
        Admission::Inject { generation } => generation,
        other => panic!("{other:?}"),
    };
    jobs.record_outcome(row_id, "worker-a", generation);
    jobs.settle_injects(&cas_dir);
    let now = Instant::now();
    jobs.expire_injects(&cas_dir, now);
    assert!(jobs.in_flight().holds_row(row_id), "a settle in progress holds the row");
    jobs.expire_injects(&cas_dir, now + SETTLE_TIMEOUT);
    assert!(!jobs.in_flight().holds_row(row_id), "a settle that never lands stops holding it");
    writer.open();
}

/// C2 shares this runtime; its RevalidateFresh jobs that panic must come
/// back to it, or its in-flight slot never clears.
#[test]
fn a_panicked_non_inbox_revalidation_is_handed_back() {
    let writer = TestWriter::start(false);
    let mut jobs = jobs_on(&writer);
    jobs.submit_external_read(
        super::super::jobs::JobSpec {
            kind: "RevalidateFresh",
            key: "RevalidateFresh:director:delivery".into(),
            lane: Lane::Read,
            criticality: super::super::jobs::Criticality::Refresh,
            generation: 7,
            budget: Duration::from_millis(250),
        },
        |_stores| panic!("director read failed"),
    );
    let mut done = Vec::new();
    settle(&mut jobs, |jobs| {
        done.extend(jobs.take_external_completions());
        !done.is_empty()
    });
    assert_eq!(done[0].key, "RevalidateFresh:director:delivery");
    assert_eq!(done[0].generation, 7);
    assert!(done[0].outcome.is_err());
}

fn task(id: &str, assignee: &str) -> cas_factory::TaskSummary {
    cas_factory::TaskSummary {
        id: id.to_string(),
        title: format!("Task {id}"),
        status: cas_types::TaskStatus::InProgress,
        priority: cas_types::Priority::MEDIUM,
        assignee: Some(assignee.to_string()),
        task_type: cas_types::TaskType::Task,
        epic: None,
        branch: None,
        updated_at: None,
        epic_verification_owner: None,
    }
}

fn data(in_progress_tasks: Vec<cas_factory::TaskSummary>) -> DirectorData {
    DirectorData {
        ready_tasks: Vec::new(),
        in_progress_tasks,
        epic_tasks: Vec::new(),
        agents: Vec::new(),
        activity: Vec::new(),
        agent_id_to_name: HashMap::new(),
        changes: Vec::new(),
        git_loaded: true,
        reminders: Vec::new(),
        epic_closed_counts: HashMap::new(),
        start_gated_task_ids: Default::default(),
    }
}

fn write_idle_alert(inboxes: &Path, worker: &str) {
    let rows = serde_json::json!([{
        "from": "director",
        "text": format!("{worker} is idle"),
        "timestamp": "2026-10-15T00:00:00Z",
        "color": "blue",
        "read": false,
        "retract_worker": worker,
    }]);
    std::fs::write(inboxes.join("supervisor.json"), rows.to_string()).unwrap();
}

fn unread_rows(inboxes: &Path) -> usize {
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(inboxes.join("supervisor.json")).unwrap()).unwrap();
    rows.len()
}

#[test]
fn flv2_retraction_reads_fresh() {
    let inboxes = tempfile::tempdir().unwrap();
    write_idle_alert(inboxes.path(), "worker-a");
    let pruner = InboxPruner::for_dir(inboxes.path().to_path_buf());
    let writer = TestWriter::start(false);
    let mut jobs = jobs_on(&writer);

    // The cached snapshot is stale: it still shows worker-a assigned, so a
    // decision from it would retract a legitimate idle alert.
    let cached = data(vec![task("cas-old", "worker-a")]);
    assert!(crate::ui::factory::director::worker_now_has_real_assignment(&cached, "worker-a"));

    // The fresh read shows worker-a idle: the row stays.
    let reads = Arc::new(AtomicUsize::new(0));
    let read_threads = Arc::new(Mutex::new(Vec::new()));
    let (counter, threads) = (Arc::clone(&reads), Arc::clone(&read_threads));
    jobs.submit_inbox_retraction(
        "supervisor",
        pruner.clone(),
        inboxes.path().to_path_buf(),
        Box::new(move |_stores| {
            counter.fetch_add(1, Ordering::SeqCst);
            threads
                .lock()
                .unwrap()
                .push(std::thread::current().name().unwrap_or("").to_string());
            Ok(data(Vec::new()))
        }),
    );
    settle(&mut jobs, |jobs| jobs.prunes_done == 1);
    assert_eq!(reads.load(Ordering::SeqCst), 1, "the decision issued exactly one fresh read");
    assert_eq!(read_threads.lock().unwrap().as_slice(), ["factory-jobs-read"]);
    assert_eq!(unread_rows(inboxes.path()), 1, "a stale cached snapshot never retracts a live row");

    // A failed fresh read is uncertainty: the row stays.
    jobs.submit_inbox_retraction(
        "supervisor",
        pruner.clone(),
        inboxes.path().to_path_buf(),
        Box::new(|_stores| Err(anyhow::anyhow!("store busy"))),
    );
    settle(&mut jobs, |jobs| jobs.retractions.is_empty());
    assert_eq!(unread_rows(inboxes.path()), 1);

    // When the fresh read shows the assignment, the alert is retracted.
    jobs.submit_inbox_retraction(
        "supervisor",
        pruner,
        inboxes.path().to_path_buf(),
        Box::new(|_stores| Ok(data(vec![task("cas-new", "worker-a")]))),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while unread_rows(inboxes.path()) != 0 {
        jobs.apply_completions();
        assert!(Instant::now() < deadline, "the fresh-read retraction did not land");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn config_cache_reloads_only_when_the_file_changes() {
    let (_root, cas_dir) = project();
    let mut cache = ConfigCache::default();
    let first = cache.get(&cas_dir);
    let again = cache.get(&cas_dir);
    assert!(Arc::ptr_eq(&first, &again), "an unchanged file is not parsed again");
    std::fs::write(cas_dir.join("config.toml"), "[factory]\n# changed\n").unwrap();
    let reloaded = cache.get(&cas_dir);
    assert!(!Arc::ptr_eq(&first, &reloaded), "a changed file is reloaded");
}

#[test]
fn transcript_probe_reads_again_only_after_the_transcript_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    std::fs::write(&path, "{}\n").unwrap();
    let cache: TranscriptProbeCache<usize> = TranscriptProbeCache::default();
    let resolves = AtomicUsize::new(0);
    let probes = AtomicUsize::new(0);
    let t0 = Instant::now();
    let run = |now: Instant| {
        cache.probe(
            "worker-a",
            now,
            || {
                resolves.fetch_add(1, Ordering::SeqCst);
                Some((path.clone(), cas_mux::SupervisorCli::Claude))
            },
            |_, _| probes.fetch_add(1, Ordering::SeqCst) + 1,
        )
    };
    assert_eq!(run(t0), Some(1));
    assert_eq!(run(t0 + Duration::from_millis(10)), Some(1), "unchanged transcript: cached");
    assert_eq!(probes.load(Ordering::SeqCst), 1);
    assert_eq!(resolves.load(Ordering::SeqCst), 1);
    std::fs::write(&path, "{}\n{\"more\":1}\n").unwrap();
    assert_eq!(run(t0 + Duration::from_millis(20)), Some(2), "a grown transcript is read again");
    assert_eq!(run(t0 + PROBE_RESOLUTION_TTL + Duration::from_millis(20)), Some(2));
    assert_eq!(resolves.load(Ordering::SeqCst), 2, "the path is re-resolved after its TTL");
    assert_eq!(probes.load(Ordering::SeqCst), 2, "re-resolving the same path keeps the result");
}

#[test]
fn another_childs_jobs_share_the_runtime_and_come_back_typed() {
    let writer = TestWriter::start(false);
    let mut jobs = jobs_on(&writer);
    let spec = |kind: &'static str, lane| super::super::jobs::JobSpec {
        kind,
        key: kind.to_string(),
        lane,
        criticality: super::super::jobs::Criticality::Refresh,
        generation: 1,
        budget: Duration::from_millis(250),
    };
    jobs.submit_external(spec("DirectorSnapshot", Lane::Blocking), || Box::new(41_u32));
    jobs.submit_external_read(spec("ReminderScan", Lane::Read), |stores| {
        Box::new(stores.is_none())
    });
    let mut done = Vec::new();
    settle(&mut jobs, |jobs| {
        done.extend(jobs.take_external_completions());
        done.len() == 2
    });
    done.sort_by_key(|result| result.kind);
    assert_eq!(done[0].kind, "DirectorSnapshot");
    let ExternalOutcome::Done(value) = done.remove(0).outcome.unwrap() else {
        panic!("the job ran");
    };
    assert_eq!(value.downcast_ref::<u32>(), Some(&41));
    let ExternalOutcome::Done(read) = done.remove(0).outcome.unwrap() else {
        panic!("the read ran");
    };
    assert_eq!(read.downcast_ref::<bool>(), Some(&true), "no read context is wired in this test");
    assert!(jobs.take_selection().is_none(), "external results never look like a selection");
}

/// Review round 1, M6: a retraction removes only the rows it judged. An
/// alert written after the decision stays, even for the same worker.
#[test]
fn retraction_removes_only_the_rows_it_judged() {
    let root = tempfile::tempdir().unwrap();
    let teams = super::super::teams::TeamsManager::rooted_at(root.path().join("t"), "t");
    std::fs::create_dir_all(root.path().join("t").join("inboxes")).unwrap();
    teams
        .write_to_inbox_for_worker_idle("supervisor", "director", "worker-a is idle (old)", None, None, "worker-a")
        .unwrap();
    let pruner = teams.inbox_pruner();
    let judged: std::collections::HashSet<_> = pruner
        .unread_alerts("supervisor")
        .into_iter()
        .map(|alert| alert.identity)
        .collect();
    assert_eq!(judged.len(), 1);
    teams
        .write_to_inbox_for_worker_idle("supervisor", "director", "worker-a is idle (new)", None, None, "worker-a")
        .unwrap();
    let retracted = pruner.retract_exact("supervisor", &judged).unwrap();
    assert_eq!(retracted.idle, 1);
    let left = pruner.unread_alerts("supervisor");
    assert_eq!(left.len(), 1, "the alert written after the decision stays");
    assert!(!judged.contains(&left[0].identity));
}

/// flv2_retraction_reads_fresh through the production wiring: the daemon's
/// own retraction call (lifecycle.rs), its fresh read of the real store and
/// the supervisor's real Teams inbox.
#[test]
fn flv2_retraction_reads_fresh_through_the_daemon() {
    let (_root, cas_dir) = project();
    let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
    let teams_root = tempfile::tempdir().unwrap();
    let teams = super::super::teams::TeamsManager::rooted_at(teams_root.path().join("t"), "t");
    std::fs::create_dir_all(teams_root.path().join("t").join("inboxes")).unwrap();
    teams
        .write_to_inbox_for_worker_idle("supervisor", "director", "worker-a is idle", None, None, "worker-a")
        .unwrap();
    let pruner = teams.inbox_pruner();
    daemon.teams = Some(teams);
    // The cached snapshot is stale: it shows worker-a assigned.
    daemon.app.set_director_data_for_test(data(vec![task("cas-old", "worker-a")]));

    daemon.submit_supervisor_inbox_retraction();
    daemon.queue_jobs.apply_completions();
    daemon.queue_jobs.apply_completions();
    assert_eq!(daemon.queue_jobs.prunes_done, 1);
    assert_eq!(
        pruner.unread_alerts("supervisor").len(),
        1,
        "the fresh store read shows worker-a idle; the stale cache never retracts"
    );

    // The store now shows the assignment: the alert is retracted.
    let mut assigned = cas_types::Task::new("cas-live".into(), "live".into());
    assigned.status = cas_types::TaskStatus::InProgress;
    assigned.assignee = Some("worker-a".into());
    crate::store::open_task_store_local(&cas_dir).unwrap().add(&assigned).unwrap();
    daemon.app.set_director_data_for_test(data(Vec::new()));
    daemon.submit_supervisor_inbox_retraction();
    daemon.queue_jobs.apply_completions();
    daemon.queue_jobs.apply_completions();
    assert_eq!(daemon.queue_jobs.prunes_done, 2);
    assert!(pruner.unread_alerts("supervisor").is_empty(), "a fresh read showing the assignment retracts it");
}

/// cas-3b16 (C2 review round 1, L8): the refresh's delivery step is what
/// starts the retraction. One delivered batch, with no prompts at all, runs
/// it as a job against a fresh read; the stale cache never retracts.
#[tokio::test]
async fn c3b16_refresh_delivery_runs_the_inbox_retraction() {
    let (_root, cas_dir) = project();
    let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
    let teams_root = tempfile::tempdir().unwrap();
    let teams = super::super::teams::TeamsManager::rooted_at(teams_root.path().join("t"), "t");
    std::fs::create_dir_all(teams_root.path().join("t").join("inboxes")).unwrap();
    teams
        .write_to_inbox_for_worker_idle("supervisor", "director", "worker-a is idle", None, None, "worker-a")
        .unwrap();
    let pruner = teams.inbox_pruner();
    daemon.teams = Some(teams);
    let mut assigned = cas_types::Task::new("cas-live".into(), "live".into());
    assigned.status = cas_types::TaskStatus::InProgress;
    assigned.assignee = Some("worker-a".into());
    crate::store::open_task_store_local(&cas_dir).unwrap().add(&assigned).unwrap();
    // The cached snapshot is stale: it does not show the assignment.
    daemon.app.set_director_data_for_test(data(Vec::new()));

    let mut refresh = super::super::director_refresh::DirectorRefresh::new();
    daemon
        .deliver_director_events(
            &mut refresh,
            Vec::new(),
            (Vec::new(), Vec::new(), None),
            std::time::Instant::now(),
            0.0,
            super::super::store_worker::global(),
            &super::super::loop_watchdog::LoopProgress::new(),
        )
        .await;
    daemon.queue_jobs.apply_completions();
    daemon.queue_jobs.apply_completions();
    assert_eq!(daemon.queue_jobs.prunes_done, 1, "the delivery step started the retraction");
    assert!(
        pruner.unread_alerts("supervisor").is_empty(),
        "decided from the fresh read, which shows worker-a assigned"
    );
}

/// Review round 2: a stamp the lane refuses (closed at shutdown) is handed
/// to the StoreWorker, never dropped and never written on the caller.
#[test]
fn a_refused_stamp_is_handed_to_the_store_worker() {
    let (_root, cas_dir) = project();
    let row_id = enqueue(&cas_dir, "worker-a", "handed off");
    let writer = TestWriter::start(false);
    let mut jobs = jobs_on(&writer);
    jobs.close_lanes_for_test();
    jobs.stamp(&cas_dir, row_id, "worker-a", QueueStamp::TransportDelivered);
    let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while queue.queued_prompt(row_id).unwrap().unwrap().processed_at.is_none() {
        assert!(Instant::now() < deadline, "the refused stamp never landed");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(writer.ran().is_empty(), "the closed lane ran nothing");
}

/// Review round 2: a stamp that failed every retry is deferred, as
/// `persist_or_defer` did before C3.
#[test]
fn an_exhausted_stamp_is_deferred_not_dropped() {
    let (_root, cas_dir) = project();
    let row_id = enqueue(&cas_dir, "worker-a", "deferred");
    let token = store_claim(&cas_dir, row_id, "worker-a").unwrap().unwrap();
    let writer = TestWriter::start(false);
    let mut jobs = jobs_on(&writer);
    // Every retry on the job lane fails; the stamp must still land.
    inject_stamp_failures(&cas_dir, 4);
    jobs.stamp(
        &cas_dir,
        row_id,
        "worker-a",
        QueueStamp::ReleaseClaim { claim_token: token },
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while claim_source(&cas_dir, row_id, "worker-a").is_some() {
        jobs.apply_completions();
        assert!(Instant::now() < deadline, "the exhausted release never landed");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !take_injected_stamp_failure(&cas_dir),
        "all four injected failures were consumed by the stamp's retries"
    );
    settle(&mut jobs, |_| writer.ran().len() == 1);
    assert_eq!(writer.ran()[0].0, "QueueStamp", "the job ran once on the lane; the StoreWorker landed it");
}

fn refresh_spec(kind: &'static str, key: &str, lane: Lane, generation: u64) -> super::super::jobs::JobSpec {
    super::super::jobs::JobSpec {
        kind,
        key: key.to_string(),
        lane,
        criticality: super::super::jobs::Criticality::Refresh,
        generation,
        budget: Duration::from_secs(5),
    }
}

/// C2 review: a full lane that drops a queued refresh job tells that job's
/// owner, not only the submitter that displaced it, so its in-flight slot
/// clears.
#[test]
fn a_dropped_job_is_reported_to_its_owner() {
    let writer = TestWriter::start(false);
    let mut jobs = QueueJobs::start_with_capacity(
        LaneWiring {
            reader: None,
            writer: Arc::new(SharedWriter(Arc::clone(&writer))),
        },
        1,
    );
    // Hold the blocking lane busy so the next jobs queue.
    let (release, held) = mpsc::channel::<()>();
    let held = Mutex::new(held);
    let (started_tx, started) = mpsc::channel::<()>();
    let started_tx = Mutex::new(started_tx);
    jobs.submit_external(refresh_spec("Busy", "busy", Lane::Blocking, 1), move || {
        let _ = started_tx.lock().unwrap().send(());
        let _ = held.lock().unwrap().recv();
        Box::new(())
    });
    // Barrier: the busy job is running, so later submits queue behind it.
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    jobs.submit_external(
        refresh_spec("WorkerHealthProbe", "WorkerHealthProbe:worker-a", Lane::Blocking, 2),
        || Box::new(()),
    );
    // A third refresh job on the full lane displaces the queued probe.
    let displaced = jobs.submit_external(
        refresh_spec("DirectorSnapshot", "DirectorSnapshot", Lane::Blocking, 3),
        || Box::new(()),
    );
    assert_eq!(
        displaced,
        super::super::jobs::Submitted::QueuedDroppedOldest {
            dropped_key: "WorkerHealthProbe:worker-a".into(),
            dropped_generation: 2,
            key_still_queued: false,
        }
    );
    jobs.apply_completions();
    let dropped = jobs.take_external_completions();
    assert_eq!(dropped.len(), 1, "exactly the displaced job is reported");
    assert_eq!(dropped[0].key, "WorkerHealthProbe:worker-a");
    assert_eq!(dropped[0].kind, "WorkerHealthProbe");
    assert_eq!(dropped[0].generation, 2);
    assert!(matches!(dropped[0].outcome, Ok(ExternalOutcome::Dropped)));
    release.send(()).unwrap();

    // The queue's own owner: a dropped retraction read clears its slot, so
    // the next retraction is not refused forever.
    let (release, held) = mpsc::channel::<()>();
    let held = Mutex::new(held);
    let (started_tx, started) = mpsc::channel::<()>();
    let started_tx = Mutex::new(started_tx);
    jobs.submit_external_read(refresh_spec("Busy", "busy-read", Lane::Read, 4), move |_| {
        let _ = started_tx.lock().unwrap().send(());
        let _ = held.lock().unwrap().recv();
        Box::new(())
    });
    // Barrier: the busy job is running, so later submits queue behind it.
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    let inboxes = tempfile::tempdir().unwrap();
    jobs.submit_inbox_retraction(
        "supervisor",
        InboxPruner::for_dir(inboxes.path().to_path_buf()),
        inboxes.path().to_path_buf(),
        Box::new(|_| Ok(data(Vec::new()))),
    );
    assert!(jobs.retractions.contains_key("supervisor"));
    jobs.submit_external_read(refresh_spec("ReminderScan", "ReminderScan", Lane::Read, 5), |_| {
        Box::new(())
    });
    jobs.apply_completions();
    assert!(
        !jobs.retractions.contains_key("supervisor"),
        "the dropped retraction no longer holds its slot"
    );
    release.send(()).unwrap();
}

/// cas-4272: a dropped key whose coalesced rerun is requeued is not
/// reported Dropped; the rerun reports Done with its own generation, and the
/// owner map forgets keys with nothing outstanding.
#[test]
fn a_dropped_key_with_a_requeued_rerun_reports_only_its_rerun() {
    let writer = TestWriter::start(false);
    let mut jobs = QueueJobs::start_with_capacity(
        LaneWiring {
            reader: None,
            writer: Arc::new(SharedWriter(Arc::clone(&writer))),
        },
        1,
    );
    let (release, held) = mpsc::channel::<()>();
    let held = Mutex::new(held);
    let (started_tx, started) = mpsc::channel::<()>();
    let started_tx = Mutex::new(started_tx);
    jobs.submit_external(refresh_spec("Busy", "busy", Lane::Blocking, 1), move || {
        let _ = started_tx.lock().unwrap().send(());
        let _ = held.lock().unwrap().recv();
        Box::new(())
    });
    // Barrier: the busy job is running, so later submits queue behind it.
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    let probe = "WorkerHealthProbe:worker-a";
    jobs.submit_external(refresh_spec("WorkerHealthProbe", probe, Lane::Blocking, 10), || {
        Box::new(10_u64)
    });
    assert_eq!(
        jobs.submit_external(refresh_spec("WorkerHealthProbe", probe, Lane::Blocking, 11), || {
            Box::new(11_u64)
        }),
        super::super::jobs::Submitted::Coalesced { replaced_generation: None }
    );
    jobs.submit_external(refresh_spec("DirectorSnapshot", "DirectorSnapshot", Lane::Blocking, 12), || {
        Box::new(12_u64)
    });
    jobs.apply_completions();
    assert!(
        jobs.take_external_completions().is_empty(),
        "the dropped key's coalesced rerun is requeued, so no Dropped is reported"
    );

    release.send(()).unwrap();
    let mut done = Vec::new();
    settle(&mut jobs, |jobs| {
        done.extend(jobs.take_external_completions());
        done.len() == 3
    });
    let rerun = done.iter().find(|result| result.key == probe).expect("the rerun reports");
    assert_eq!(rerun.generation, 11, "the rerun reports its own generation");
    assert!(matches!(rerun.outcome, Ok(ExternalOutcome::Done(_))));
    assert!(jobs.job_owners.is_empty(), "keys with nothing outstanding are forgotten");
}

/// cas-4272: a stale mirror result (its drop already reported, a newer scan
/// in flight) never overwrites the newer scan's slot.
#[test]
fn a_stale_mirror_result_is_ignored() {
    // A gated writer holds the scans, so a stale result can arrive while a
    // newer scan is in flight.
    let writer = TestWriter::start(true);
    let mut jobs = jobs_on(&writer);
    let (stale_ran_tx, stale_ran) = mpsc::channel::<()>();
    let stale_ran_tx = Mutex::new(stale_ran_tx);
    jobs.submit_mirror_scan(move |_| {
        let _ = stale_ran_tx.lock().unwrap().send(());
    });
    let stale = jobs.mirror_generation;
    // Its drop is reported (as a full lane would): the slot frees and a
    // newer scan takes it, coalesced behind the stale one on the same key.
    jobs.mirror_failed(stale);
    assert!(!jobs.mirror_in_flight);
    let (release_current, current_held) = mpsc::channel::<()>();
    let current_held = Mutex::new(current_held);
    jobs.submit_mirror_scan(move |state| {
        let _ = current_held.lock().unwrap().recv();
        state.operator_messages.push(crate::ui::factory::ConversationHistoryMessage {
            notification_id: 77, client_ref: None, target: "terminal-history".into(), text: "Typed in the terminal".into(),
            state: "delivered".into(), stamped: false, reply_to: None, device_id: "terminal".into(),
            operator_label: Some("Terminal".into()), session: "factory-1".into(), at: chrono::Utc::now().to_rfc3339(),
        });
    });
    let current = jobs.mirror_generation;
    assert!(current > stale && jobs.mirror_in_flight);
    // A stale drop report changes nothing either.
    jobs.mirror_failed(stale);
    assert!(jobs.mirror_in_flight, "a stale drop report must not free the newer scan's slot");
    writer.open();
    // The stale scan finishes; its result is posted before the current scan
    // starts (jobs.rs posts, then the lane takes the requeued rerun).
    stale_ran.recv_timeout(Duration::from_secs(10)).unwrap();
    settle(&mut jobs, |jobs| jobs.mirror_results_seen == 1);
    assert!(jobs.mirror_in_flight, "the stale result must not end the newer scan");
    assert!(jobs.mirror_state.is_none(), "the newer scan still holds the cursors");
    release_current.send(()).unwrap();
    settle(&mut jobs, |jobs| !jobs.mirror_in_flight);
    assert!(jobs.mirror_state.is_some(), "the current scan handed its cursors back");
    assert_eq!(jobs.take_operator_messages().iter().map(|row| row.notification_id).collect::<Vec<_>>(), vec![77]);
    assert!(jobs.take_operator_messages().is_empty(), "a completed scan publishes only once");
}

/// cas-4272: a terminal stamp now lands on the writer, so until it does the
/// row is held out of delivery; a re-read that still shows it pending must
/// not deliver it.
#[test]
fn a_row_with_a_terminal_stamp_in_flight_is_held_out_of_delivery() {
    let (_root, cas_dir) = project();
    let row_id = enqueue(&cas_dir, "worker-a", "superseded");
    let writer = TestWriter::start(true);
    let mut jobs = jobs_on(&writer);
    jobs.stamp(
        &cas_dir,
        row_id,
        "worker-a",
        QueueStamp::Superseded { detail: "premise expired".into() },
    );
    let now = Instant::now();
    assert!(jobs.row_settling(row_id, now), "held while the stamp is queued");
    let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
    assert!(queue.queued_prompt(row_id).unwrap().unwrap().processed_at.is_none());
    writer.open();
    settle(&mut jobs, |jobs| !jobs.row_settling(row_id, Instant::now()));
    assert!(queue.queued_prompt(row_id).unwrap().unwrap().processed_at.is_some());

    // A refused terminal stamp is deferred, and the row is held meanwhile.
    let other = enqueue(&cas_dir, "worker-a", "dropped");
    jobs.close_lanes_for_test();
    jobs.stamp(&cas_dir, other, "worker-a", QueueStamp::Dropped { detail: None });
    assert!(jobs.row_settling(other, Instant::now()));
    assert!(
        !jobs.row_settling(other, Instant::now() + SETTLING_DEFERRED_HOLD),
        "the hold ends after the deferred write had time to land"
    );
}

/// cas-4272 review M1: a job that finished but whose result is not collected
/// yet stays outstanding; when its requeued rerun is then dropped, the drop
/// is attributed to the rerun's own generation, and nothing is lost.
#[test]
fn a_drop_after_a_finished_uncollected_job_is_attributed_by_generation() {
    let writer = TestWriter::start(false);
    let mut jobs = QueueJobs::start_with_capacity(
        LaneWiring {
            reader: None,
            writer: Arc::new(SharedWriter(Arc::clone(&writer))),
        },
        1,
    );
    let probe = "WorkerHealthProbe:worker-a";
    // g10 runs and blocks; g11 coalesces as its rerun.
    let (release, held) = mpsc::channel::<()>();
    let held = Mutex::new(held);
    let (started_tx, started) = mpsc::channel::<()>();
    let started_tx = Mutex::new(started_tx);
    jobs.submit_external(refresh_spec("WorkerHealthProbe", probe, Lane::Blocking, 10), move || {
        let _ = started_tx.lock().unwrap().send(());
        let _ = held.lock().unwrap().recv();
        Box::new(10_u64)
    });
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    // g11 is the rerun; it blocks too, so it is still queued when dropped.
    let (release_rerun, rerun_held) = mpsc::channel::<()>();
    let rerun_held = Mutex::new(rerun_held);
    assert_eq!(
        jobs.submit_external(refresh_spec("WorkerHealthProbe", probe, Lane::Blocking, 11), move || {
            let _ = rerun_held.lock().unwrap().recv();
            Box::new(11_u64)
        }),
        super::super::jobs::Submitted::Coalesced { replaced_generation: None }
    );
    // A second busy job queues behind, so g11 requeues behind it when g10
    // finishes; then g10's result is NOT collected (no apply yet).
    let (release_busy, busy_held) = mpsc::channel::<()>();
    let busy_held = Mutex::new(busy_held);
    jobs.submit_external(refresh_spec("Busy", "busy", Lane::Blocking, 20), move || {
        let _ = busy_held.lock().unwrap().recv();
        Box::new(())
    });
    release.send(()).unwrap();
    // Wait until g10 finished and released its key (g11 requeued): the lane
    // is now running "busy" or holding it, with g11 queued.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let stats = match &jobs.exec {
            Exec::Lanes(runtime) => runtime.lane_stats(Lane::Blocking),
            Exec::Inline(_) => unreachable!(),
        };
        if stats.running.as_ref().is_some_and(|running| running.key == "busy") {
            break;
        }
        assert!(Instant::now() < deadline, "busy never started: {stats:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
    // A full-lane submit now drops the queued g11 (no rerun left behind it).
    let dropped = jobs.submit_external(refresh_spec("ReminderScan", "ReminderScan", Lane::Blocking, 30), || {
        Box::new(())
    });
    assert!(
        matches!(
            &dropped,
            super::super::jobs::Submitted::QueuedDroppedOldest { dropped_key, dropped_generation: 11, key_still_queued: false }
                if dropped_key == probe
        ),
        "{dropped:?}"
    );
    release_busy.send(()).unwrap();
    drop(release_rerun);
    let mut done = Vec::new();
    settle(&mut jobs, |jobs| {
        done.extend(jobs.take_external_completions());
        done.iter().any(|result| result.key == "ReminderScan")
            && done.iter().filter(|result| result.key == probe).count() == 2
    });
    let probe_results: Vec<(u64, bool)> = done
        .iter()
        .filter(|result| result.key == probe)
        .map(|result| (result.generation, matches!(result.outcome, Ok(ExternalOutcome::Dropped))))
        .collect();
    assert!(probe_results.contains(&(10, false)), "g10's Done is delivered: {probe_results:?}");
    assert!(probe_results.contains(&(11, true)), "g11's drop is reported as g11: {probe_results:?}");
    assert!(jobs.job_owners.get(probe).is_none(), "nothing is left outstanding for the key");
}

/// cas-4272 review L3: a Retry stamp holds its row and blocks QueueSelect
/// until it lands; a selection then skips rows stamped after it was
/// submitted.
#[test]
fn a_retry_stamp_holds_its_row_and_gates_selection_until_it_lands() {
    let (_root, cas_dir) = project();
    let row_id = enqueue(&cas_dir, "worker-a", "retry later");
    let writer = TestWriter::start(true);
    let mut jobs = jobs_on(&writer);
    jobs.stamp(
        &cas_dir,
        row_id,
        "worker-a",
        QueueStamp::Retry { reason: cas_store::PendingReason::GatedNotReady, detail: None },
    );
    assert!(jobs.row_settling(row_id, Instant::now()), "the row is held while its Retry is queued");
    jobs.submit_select(request(&cas_dir), super::super::queue_and_events::run_queue_select);
    assert!(!jobs.select_in_flight(), "no QueueSelect while a stamp is outstanding");

    writer.open();
    settle(&mut jobs, |jobs| !jobs.row_settling(row_id, Instant::now()));
    jobs.submit_select(request(&cas_dir), super::super::queue_and_events::run_queue_select);
    assert!(jobs.select_in_flight(), "selection resumes once every stamp landed");
    settle(&mut jobs, |jobs| !jobs.select_in_flight());
    let selection = jobs.take_selection().unwrap();
    let since = selection.submitted_at.unwrap();
    assert!(!jobs.stamped_since(row_id, since), "the Retry landed before this selection");
}

/// cas-4272 review L2: a stamp job that panics releases the selection gate
/// and holds its row only for the deferral window; a panicked settle never
/// lowers a hold it did not raise.
#[test]
fn a_panicked_stamp_job_releases_the_gate_and_holds_its_row_briefly() {
    let (_root, cas_dir) = project();
    let row_id = enqueue(&cas_dir, "worker-a", "panics");
    let writer = TestWriter::start(false);
    let mut jobs = jobs_on(&writer);
    inject_stamp_panics(&cas_dir, 1);
    jobs.stamp(&cas_dir, row_id, "worker-a", QueueStamp::Dropped { detail: None });
    settle(&mut jobs, |jobs| !jobs.stamps_pending(Instant::now()));
    let now = Instant::now();
    assert!(jobs.row_settling(row_id, now), "held while a deferred write could still land");
    assert!(!jobs.row_settling(row_id, now + SETTLING_DEFERRED_HOLD), "the hold expires");

    // A second stamp on the same row raises the count; a panicked settle on
    // that key must not lower it.
    let other = enqueue(&cas_dir, "worker-a", "settle panics");
    let cas = cas_dir.clone();
    let generation = match jobs
        .admit(other, "worker-a", Instant::now(), move || store_claim(&cas, other, "worker-a"))
        .unwrap()
    {
        Admission::Inject { generation } => generation,
        admission => panic!("{admission:?}"),
    };
    jobs.record_outcome(other, "worker-a", generation);
    jobs.settle_injects(&cas_dir);
    settle(&mut jobs, |jobs| !jobs.in_flight().holds_row(other));
    assert!(!jobs.row_settling(other, Instant::now()), "a settle raised no hold and lowers none");
}

/// cas-786a6 review M1: a claim job whose wait expired, and whose result
/// lands after a newer job was submitted, never holds the row: its claim is
/// released, and the row is claimable again.
#[test]
fn a_superseded_claim_job_releases_its_claim() {
    let (_root, cas_dir) = project();
    let row = enqueue(&cas_dir, "worker-a", "claimed twice");
    let writer = TestWriter::start(true);
    let mut jobs = jobs_on(&writer);
    assert_eq!(
        jobs.claim_for_inject(&cas_dir, "s", row, "worker-a", "worker-a"),
        ClaimStep::Waiting
    );
    // The first job's wait expires before it ran (a stalled writer).
    jobs.expire_claims(&cas_dir, Instant::now() + INJECT_OUTCOME_TIMEOUT + Duration::from_secs(1));
    assert!(!jobs.has_claim(row, "worker-a"));
    assert_eq!(
        jobs.claim_for_inject(&cas_dir, "s", row, "worker-a", "worker-a"),
        ClaimStep::Waiting
    );
    writer.open();
    // Job 1 claims; job 2 then finds a bare claim. Job 1's late token is
    // released rather than overwritten.
    // Job 2 comes back (it ran after job 1, so job 1's result is applied
    // too), then job 1's late claim is released.
    settle(&mut jobs, |jobs| jobs.claim_resolved(&cas_dir, "s", row, "worker-a", "worker-a"));
    settle(&mut jobs, |_| claim_source(&cas_dir, row, "worker-a").is_none());
    assert_eq!(
        jobs.claim_for_inject(&cas_dir, "s", row, "worker-a", "worker-a"),
        ClaimStep::NotClaimed,
        "job 2 found job 1's bare claim"
    );
    assert!(store_claim(&cas_dir, row, "worker-a").unwrap().is_some(), "the row is claimable again");
}

/// cas-786a6 review L4: a claim that came back but whose row was never
/// delivered is released after the claim TTL.
#[test]
fn an_untaken_claim_is_released_after_its_ttl() {
    let (_root, cas_dir) = project();
    let row = enqueue(&cas_dir, "worker-a", "claimed, never delivered");
    let writer = TestWriter::start(false);
    let mut jobs = jobs_on(&writer);
    jobs.claim_for_inject(&cas_dir, "s", row, "worker-a", "worker-a");
    // The claim job's result is back (not just its write).
    settle(&mut jobs, |jobs| jobs.claim_resolved(&cas_dir, "s", row, "worker-a", "worker-a"));
    jobs.expire_claims(&cas_dir, Instant::now());
    assert!(claim_source(&cas_dir, row, "worker-a").is_some(), "held within the TTL");
    jobs.expire_claims(&cas_dir, Instant::now() + CLAIM_READY_TTL + Duration::from_millis(10));
    settle(&mut jobs, |_| claim_source(&cas_dir, row, "worker-a").is_none());
    assert!(!jobs.has_claim(row, "worker-a"));
}


#[test]
fn cas_afe0_settled_broadcast_member_retires_without_fabricating_a_handoff() {
    let (_root, cas_dir) = project();
    let row = enqueue(&cas_dir, "all_workers", "Shared work");
    let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
    queue.acknowledge_for_recipients(row, &["worker-a".into()], None).unwrap();
    let settled = run_claim_for_inject(&cas_dir, "s", row, "worker-a", "all_workers", 1);
    assert_eq!(settled.outcome, ClaimOutcome::HandedOff);
    assert_eq!(queue.recipient_receipt(row, "worker-a").unwrap(), None);
    assert!(!queue.recipient_handed_off(row, "worker-a").unwrap());
    let peer = run_claim_for_inject(&cas_dir, "s", row, "worker-b", "all_workers", 1);
    assert!(matches!(peer.outcome, ClaimOutcome::Claimed(_)));
    assert!(queue.queued_prompt(row).unwrap().unwrap().acked_at.is_none());
}
