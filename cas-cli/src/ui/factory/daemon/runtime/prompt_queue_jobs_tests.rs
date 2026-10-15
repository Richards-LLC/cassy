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

/// A single writer thread, like the StoreWorker, that records which jobs ran
/// on it. With `gated`, jobs wait until [`TestWriter::open`].
struct TestWriter {
    sender: Mutex<mpsc::Sender<(&'static str, WriterJob)>>,
    ran: Arc<Mutex<Vec<(&'static str, String)>>>,
    gate: Arc<(Mutex<bool>, Condvar)>,
}

impl TestWriter {
    fn start(gated: bool) -> Arc<Self> {
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
                    job();
                    thread_ran.lock().unwrap().push((label, thread));
                }
            })
            .unwrap();
        Arc::new(Self {
            sender: Mutex::new(sender),
            ran,
            gate,
        })
    }

    fn open(&self) {
        let (open, ready) = &*self.gate;
        *open.lock().unwrap() = true;
        ready.notify_all();
    }

    fn ran(&self) -> Vec<(&'static str, String)> {
        self.ran.lock().unwrap().clone()
    }
}

struct SharedWriter(Arc<TestWriter>);

impl WriteExecutor for SharedWriter {
    fn execute(&self, label: &'static str, job: Box<dyn FnOnce() + Send + 'static>) -> bool {
        self.0.sender.lock().unwrap().send((label, job)).is_ok()
    }
}

fn jobs_on(writer: &Arc<TestWriter>) -> QueueJobs {
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

#[test]
fn flv2_queue_stamps_on_writer() {
    let (_root, cas_dir) = project();
    let delivered = enqueue(&cas_dir, "worker-a", "first");
    let released = enqueue(&cas_dir, "worker-a", "second");
    let writer = TestWriter::start(false);
    let mut jobs = jobs_on(&writer);

    // QueueSelect runs on the writer and returns the batch with its claims,
    // taken in the selecting transaction.
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
    settle(&mut jobs, |jobs| jobs.has_ready() || !jobs.select_in_flight());
    jobs.apply_completions();
    let selection = jobs.take_selection().expect("the selection lands as a completion");
    assert_eq!(
        select_threads.lock().unwrap().as_slice(),
        ["test-store-writer"],
        "the selection's store work ran on the writer, not the caller"
    );
    let ids: Vec<i64> = selection.rows.iter().map(|row| row.prompt.id).collect();
    assert_eq!(ids, vec![delivered, released]);
    for row in &selection.rows {
        assert_eq!(row.claims, vec![("worker-a".to_string(), true)]);
        assert_eq!(claim_source(&cas_dir, row.prompt.id, "worker-a").as_deref(), Some("transport_claimed"));
    }
    let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
    let conn = rusqlite::Connection::open(cas_dir.join("cas.db")).unwrap();
    let selected_at: Option<String> = conn
        .query_row("SELECT selected_at FROM prompt_queue WHERE id = ?", [delivered], |r| r.get(0))
        .unwrap();
    assert!(selected_at.is_some(), "QueueSelect stamps the Selected stage");

    // Control delivers the first row and hands its claim off; the second row
    // is gated this pass, so the pass releases its claim.
    jobs.open_pass_claims(&selection.rows);
    let generation = match jobs.admit(delivered, "worker-a", Instant::now(), || unreachable!()).unwrap() {
        Admission::Inject { generation } => generation,
        other => panic!("the claimed row is admitted, got {other:?}"),
    };
    jobs.record_outcome(delivered, "worker-a", generation);
    jobs.stamp(&cas_dir, delivered, "worker-a", QueueStamp::TransportReceipt);
    jobs.stamp(&cas_dir, delivered, "worker-a", QueueStamp::TransportDelivered);
    assert_eq!(jobs.close_pass_claims(&cas_dir), 1, "the gated row's claim is released");
    jobs.settle_injects(&cas_dir);
    settle(&mut jobs, |jobs| jobs.in_flight().len() == 0);
    settle(&mut jobs, |_| writer.ran().len() >= 5);

    // Every selection and stamp ran on the writer thread, none on the caller.
    let ran = writer.ran();
    assert_eq!(
        ran.iter().map(|(label, _)| *label).collect::<Vec<_>>(),
        vec!["QueueSelect", "QueueStamp", "QueueStamp", "QueueStamp", "QueueStamp"],
        "the selection, then receipt, delivered, release and settle stamps: {ran:?}"
    );
    assert!(ran.iter().all(|(_, thread)| thread == "test-store-writer"), "{ran:?}");
    assert_ne!(std::thread::current().name(), Some("test-store-writer"));

    let row = queue.queued_prompt(delivered).unwrap().unwrap();
    assert!(row.processed_at.is_some(), "the delivered stamp landed");
    assert_eq!(claim_source(&cas_dir, released, "worker-a"), None, "the unused claim was released");
    let pending = queue.queued_prompt(released).unwrap().unwrap();
    assert!(pending.processed_at.is_none(), "the gated row stays pending");
}

#[test]
fn flv2_no_double_inject() {
    let (_root, cas_dir) = project();
    let row_id = enqueue(&cas_dir, "worker-a", "only once");
    let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
    let row = queue.queued_prompt(row_id).unwrap().unwrap();
    // Stamps are held at the writer, so they have not landed while the row
    // is selected a second time.
    let writer = TestWriter::start(true);
    let mut jobs = jobs_on(&writer);
    let selected = vec![cas_store::SelectedPrompt {
        prompt: row.clone(),
        // Worst case: both selections report the claim as theirs, so only
        // the in-flight set stands between the row and a second inject.
        claims: vec![("worker-a".to_string(), true)],
    }];

    let t0 = Instant::now();
    let mut injects = 0;
    for pass in 0..2 {
        jobs.open_pass_claims(&selected);
        match jobs.admit(row_id, "worker-a", t0, || unreachable!()).unwrap() {
            Admission::Inject { generation } => {
                injects += 1;
                jobs.record_outcome(row_id, "worker-a", generation);
                jobs.stamp(&cas_dir, row_id, "worker-a", QueueStamp::TransportDelivered);
                // A repeated stamp for the same generation is not submitted.
                jobs.stamp(&cas_dir, row_id, "worker-a", QueueStamp::TransportDelivered);
            }
            Admission::InFlight => assert_eq!(pass, 1),
            Admission::NotClaimed => panic!("the selection claimed the row"),
        }
        jobs.close_pass_claims(&cas_dir);
        jobs.settle_injects(&cas_dir);
        jobs.apply_completions();
    }
    assert_eq!(injects, 1, "a row selected twice before its stamp lands is injected once");
    assert!(jobs.in_flight().holds_row(row_id));

    // The stamps land; the entry clears.
    writer.open();
    settle(&mut jobs, |jobs| !jobs.in_flight().holds_row(row_id));
    let stamps = writer.ran().iter().filter(|(label, _)| *label == "QueueStamp").count();
    assert_eq!(stamps, 3, "delivered once, the second pass's unused claim, and the settle");

    // An inject whose outcome never arrives releases its claim after the
    // outcome timeout (fake clock), and the row is no longer in flight.
    let lost = enqueue(&cas_dir, "worker-a", "outcome lost");
    assert!(queue.claim_recipient_transport(lost, "worker-a").unwrap());
    let lost_row = vec![cas_store::SelectedPrompt {
        prompt: queue.queued_prompt(lost).unwrap().unwrap(),
        claims: vec![("worker-a".to_string(), true)],
    }];
    jobs.open_pass_claims(&lost_row);
    assert!(matches!(
        jobs.admit(lost, "worker-a", t0, || unreachable!()).unwrap(),
        Admission::Inject { .. }
    ));
    assert_eq!(jobs.close_pass_claims(&cas_dir), 0, "the claim was handed to the inject");
    jobs.settle_injects(&cas_dir);
    assert_eq!(jobs.expire_injects(&cas_dir, t0 + INJECT_OUTCOME_TIMEOUT - Duration::from_millis(1)), 0);
    assert!(jobs.in_flight().holds_row(lost));
    assert_eq!(jobs.expire_injects(&cas_dir, t0 + INJECT_OUTCOME_TIMEOUT), 1);
    assert!(!jobs.in_flight().holds_row(lost));
    settle(&mut jobs, |_| claim_source(&cas_dir, lost, "worker-a").is_none());
    assert!(
        queue.claim_recipient_transport(lost, "worker-a").unwrap(),
        "the released row can be claimed again on its normal cadence"
    );
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
