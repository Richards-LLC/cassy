//! Factory loop v2 job framework (cas-1240, spec-4c5b §4.1–4.4).
//!
//! The control loop must never block on I/O. Every call that can block for
//! more than about 1 ms becomes a [`JobSpec`] submitted to a [`JobRuntime`].
//! The job runs on a lane thread and posts a [`JobResult`] to a completion
//! queue. The loop drains that queue without waiting
//! ([`JobRuntime::drain_completions`]) and keeps going.
//!
//! Guarantees:
//! - **Lanes.** [`Lane::Read`] runs on its own thread so reads never queue
//!   behind writes. It opens one dedicated context on first use (in the
//!   daemon, its own SQLite connection, see [`LaneWiring::daemon`]) and passes
//!   it to every [`JobRuntime::submit_read`] job. [`Lane::Write`] jobs run on
//!   the daemon's single writer, [`super::store_worker::StoreWorker`]; the
//!   framework adds no second writer. [`Lane::Blocking`] runs local blocking
//!   work (`/proc`, transcripts, files) and [`Lane::External`] runs slow
//!   external calls (git, gh, HTTP), so a slow fetch never stalls a probe.
//! - **Single-flight.** A second submit for a key whose job is queued or
//!   running is coalesced. The caller learns this and the work is not run
//!   twice.
//! - **Generations.** A result carries the generation it was computed for.
//!   [`GenerationGate`] drops a result older than the state the caller has
//!   already applied.
//! - **Bounded queues that never block the submitter.** A full lane drops its
//!   oldest [`Criticality::Refresh`] job. [`Criticality::Critical`] jobs
//!   (receipts, stage stamps, lifecycle relays) are never dropped. Their
//!   queue may grow past the cap, and its oldest age is reported for the
//!   watchdog.
//!
//! This is the API only. No daemon caller is migrated here; children C2 and
//! C3 move work onto it.

// cas-1240 lands the API before its first caller (children C2 and C3), so the
// non-test build sees it as unused until then.
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Which thread runs a job (spec §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Lane {
    /// SQLite reads on the dedicated reader connection.
    Read,
    /// SQLite writes: the daemon's single writer.
    Write,
    /// Local blocking work: `/proc` scans, transcript and file reads.
    Blocking,
    /// Slow external calls: git (fetch, rev-parse), gh, HTTP. Kept apart from
    /// [`Lane::Blocking`] so a 5 s fetch or a 10 s CI poll never stalls
    /// worker health probes (review round 1, item 5).
    External,
}

impl Lane {
    const ALL: [Lane; 4] = [Lane::Read, Lane::Write, Lane::Blocking, Lane::External];

    fn thread_name(self) -> &'static str {
        match self {
            Lane::Read => "factory-jobs-read",
            Lane::Write => "factory-jobs-write",
            Lane::Blocking => "factory-jobs-blocking",
            Lane::External => "factory-jobs-external",
        }
    }
}

/// Whether a full queue may drop a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Criticality {
    /// Never dropped: receipts, stage stamps, lifecycle relays.
    Critical,
    /// Replaceable by a later run: snapshots, probes, polls.
    Refresh,
}

/// What to run and how it may be coalesced or dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JobSpec {
    /// Job kind, e.g. `"DirectorSnapshot"`; also the watchdog's step name.
    pub(crate) kind: &'static str,
    /// Single-flight key: at most one job per key is queued or running.
    pub(crate) key: String,
    pub(crate) lane: Lane,
    pub(crate) criticality: Criticality,
    /// The generation of caller state this job serves (see [`GenerationGate`]).
    pub(crate) generation: u64,
    /// Watchdog budget for one run (spec §4.4).
    pub(crate) budget: Duration,
}

/// A job that panicked instead of returning (review round 1, item 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JobPanicked {
    pub(crate) message: String,
}

/// A finished job, as posted to the completion queue. Every job that runs
/// posts exactly one result, including one that panicked.
#[derive(Debug)]
pub(crate) struct JobResult<T> {
    pub(crate) kind: &'static str,
    pub(crate) key: String,
    pub(crate) generation: u64,
    pub(crate) outcome: Result<T, JobPanicked>,
    /// How long the job itself ran. For a write-lane job this excludes
    /// `writer_wait`.
    pub(crate) elapsed: Duration,
    /// Write lane only: how long the job waited for the StoreWorker (its
    /// backlog or a deferred-write retry) before it started. Not counted in
    /// `elapsed` or against the budget (review round 2, M3).
    pub(crate) writer_wait: Duration,
    /// True when the run took longer than its budget.
    pub(crate) over_budget: bool,
}

/// What [`JobRuntime::submit`] did. It never blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Submitted {
    /// Queued to run.
    Queued,
    /// A refresh job whose key was already queued or running. The newest such
    /// closure runs once more after the current job finishes (dirty flag), so
    /// a newer generation is never lost. `replaced_generation` names an
    /// earlier waiting rerun this one replaced, which will never run
    /// (cas-4272 review M1).
    Coalesced { replaced_generation: Option<u64> },
    /// A critical job whose key was already queued or running. It runs after
    /// that job, in submission order; it is never dropped.
    QueuedBehindKey,
    /// Queued; to make room, the oldest refresh job on the lane was dropped.
    /// `dropped_generation` is that job's own generation; `key_still_queued`
    /// is true when work waiting on the same key was requeued in its place
    /// and will still run (cas-4272 review M1). That work may be a coalesced
    /// rerun or only a critical follow-up; either way the key reports again.
    QueuedDroppedOldest {
        dropped_key: String,
        dropped_generation: u64,
        key_still_queued: bool,
    },
    /// The lane was full of critical work and this refresh job was dropped.
    DroppedRefresh,
    /// Refused: the runtime is closed ([`JobRuntime::close`]) or the lane's
    /// thread could not start. The work was not queued.
    Closed,
    /// Refused: the key is queued or running on another lane. All jobs for one
    /// key must use one lane, so a write can never run off the single writer
    /// or after its lane has exited (review round 2, L2). Not queued.
    LaneMismatch { busy_on: Lane },
}

/// The job a lane is running right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunningJob {
    pub(crate) lane: Lane,
    pub(crate) kind: &'static str,
    pub(crate) key: String,
    /// How long the job itself has run; zero while it waits for the writer.
    pub(crate) age: Duration,
    pub(crate) budget: Duration,
    /// Write lane: the job has not started on the StoreWorker yet.
    pub(crate) waiting_for_writer: bool,
    /// Write lane: time spent waiting for the StoreWorker, so far or in total.
    pub(crate) writer_wait: Duration,
}

impl RunningJob {
    /// The job itself has run past its budget. Writer wait does not count.
    pub(crate) fn over_budget(&self) -> bool {
        self.age > self.budget
    }

    /// A write job has waited for the StoreWorker longer than its own budget:
    /// the writer, not this job, is slow or wedged.
    pub(crate) fn writer_stalled(&self) -> bool {
        self.waiting_for_writer && self.writer_wait > self.budget
    }
}

/// Lane occupancy, for the watchdog and `loop.json`. `running` makes a job
/// that is wedged mid-run visible (review round 1, item 6). Work waiting
/// behind a busy key counts as queued (review round 2, M2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LaneStats {
    /// Jobs waiting to run, on the lane queue or behind a busy key; not the
    /// running one.
    pub(crate) queued: usize,
    pub(crate) oldest_queued_age: Option<Duration>,
    /// Critical jobs waiting, on the lane queue or behind a busy key.
    pub(crate) critical_backlog: usize,
    /// Age of the oldest waiting critical job (spec §4.2 watchdog metric).
    pub(crate) oldest_critical_age: Option<Duration>,
    pub(crate) dropped_refresh: u64,
    pub(crate) running: Option<RunningJob>,
}

/// Drops results computed for state the caller has already moved past.
#[derive(Debug, Default)]
pub(crate) struct GenerationGate {
    applied: HashMap<String, u64>,
}

impl GenerationGate {
    /// Accept `result` if its generation is at least the newest applied for
    /// its key, and record it as applied. Returns false for a stale result.
    pub(crate) fn accept<T>(&mut self, result: &JobResult<T>) -> bool {
        let applied = self.applied.entry(result.key.clone()).or_insert(0);
        if result.generation < *applied {
            return false;
        }
        *applied = result.generation;
        true
    }

    /// Record that the caller applied `generation` for `key` itself, without
    /// a job, so older in-flight results are dropped when they arrive.
    pub(crate) fn advance(&mut self, key: &str, generation: u64) {
        let applied = self.applied.entry(key.to_string()).or_insert(0);
        *applied = (*applied).max(generation);
    }

    /// Forget a key whose subject is gone (a delivered row, a retired
    /// worker), so per-row keys stay bounded (review round 1, item 8).
    pub(crate) fn forget(&mut self, key: &str) {
        self.applied.remove(key);
    }

    /// Keep only the keys `keep` accepts.
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.applied.retain(|key, _| keep(key));
    }

    pub(crate) fn len(&self) -> usize {
        self.applied.len()
    }
}

/// Exponential backoff per key for jobs whose outcome keeps failing
/// (spec §4.2; review round 1, item 11). The caller asks
/// [`Backoff::ready`] before submitting and reports each outcome. Time is
/// passed in, so tests use a fake clock.
#[derive(Debug)]
pub(crate) struct Backoff {
    base: Duration,
    max: Duration,
    /// key -> (consecutive failures, not before)
    failures: HashMap<String, (u32, Instant)>,
}

impl Backoff {
    pub(crate) fn new(base: Duration, max: Duration) -> Self {
        Self {
            base,
            max,
            failures: HashMap::new(),
        }
    }

    /// Whether `key` may run at `now`.
    pub(crate) fn ready(&self, key: &str, now: Instant) -> bool {
        self.failures
            .get(key)
            .is_none_or(|(_, not_before)| now >= *not_before)
    }

    /// Record a failure at `now`; returns the delay before the next attempt.
    pub(crate) fn record_failure(&mut self, key: &str, now: Instant) -> Duration {
        let entry = self.failures.entry(key.to_string()).or_insert((0, now));
        entry.0 = entry.0.saturating_add(1);
        let delay = self
            .base
            .saturating_mul(1u32 << (entry.0 - 1).min(16))
            .min(self.max);
        entry.1 = now + delay;
        delay
    }

    /// A success clears the key's backoff.
    pub(crate) fn record_success(&mut self, key: &str) {
        self.failures.remove(key);
    }
}

/// The single writer write-lane jobs run on (review round 1, item 3). In the
/// daemon this is the existing [`super::store_worker::StoreWorker`], so the
/// job framework adds no second writer.
pub(crate) trait WriteExecutor: Send + Sync + 'static {
    /// Run `job` on the writer thread; false when the writer is gone.
    fn execute(&self, label: &'static str, job: Box<dyn FnOnce() + Send + 'static>) -> bool;
}

/// Opens the read lane's dedicated context (in the daemon, its own SQLite
/// connection) once, on the read thread, on first use. The context never
/// leaves that thread, so it need not be `Send`.
pub(crate) type ReadContextFactory =
    Box<dyn FnOnce() -> Option<Box<dyn std::any::Any>> + Send + 'static>;

/// How the lanes reach the stores.
pub(crate) struct LaneWiring {
    pub(crate) reader: Option<ReadContextFactory>,
    pub(crate) writer: Arc<dyn WriteExecutor>,
}

impl LaneWiring {
    /// Writes go through the daemon's StoreWorker; no read context.
    pub(crate) fn daemon_writer() -> Self {
        Self {
            reader: None,
            writer: Arc::new(GlobalStoreWorker),
        }
    }

    /// The daemon's wiring: writes go through its StoreWorker, and the read
    /// lane opens its own [`crate::ui::factory::director::DirectorStores`]
    /// connection, as the refresh reader does, so reads hold no in-process
    /// connection mutex the loop or the writer needs. Read jobs take
    /// `Option<&DirectorStores>`; `None` when the connection could not open.
    pub(crate) fn daemon(cas_dir: std::path::PathBuf, project_id: Option<String>) -> Self {
        Self {
            reader: Some(Box::new(move || {
                match crate::ui::factory::director::DirectorStores::open_dedicated(&cas_dir) {
                    Ok(stores) => {
                        Some(Box::new(stores.with_project_id(project_id)) as Box<dyn std::any::Any>)
                    }
                    Err(error) => {
                        tracing::warn!(%error, "factory read lane could not open its own connection");
                        None
                    }
                }
            })),
            writer: Arc::new(GlobalStoreWorker),
        }
    }
}

struct GlobalStoreWorker;

impl WriteExecutor for GlobalStoreWorker {
    fn execute(&self, label: &'static str, job: Box<dyn FnOnce() + Send + 'static>) -> bool {
        super::store_worker::global().execute(label, job)
    }
}

type ReadWork<T> = Box<dyn FnOnce(Option<&dyn std::any::Any>) -> T + Send + 'static>;

enum Work<T> {
    Plain(Box<dyn FnOnce() -> T + Send + 'static>),
    Read(ReadWork<T>),
}

/// Work waiting for a key that is queued or running.
struct KeyFollowUps<T> {
    /// The lane the key's work runs on; follow-ups are queued there.
    lane: Lane,
    /// Critical submits for the key, run in order after it.
    critical: VecDeque<Queued<T>>,
    /// The newest refresh submitted while the key was busy (dirty flag).
    rerun: Option<Queued<T>>,
}

impl<T> KeyFollowUps<T> {
    fn new(lane: Lane) -> Self {
        Self {
            lane,
            critical: VecDeque::new(),
            rerun: None,
        }
    }
}

struct Queued<T> {
    spec: JobSpec,
    work: Work<T>,
    enqueued_at: Instant,
}

/// The job a lane thread took, as `lane_stats` sees it.
struct Running {
    kind: &'static str,
    key: String,
    /// When the lane took the job.
    taken_at: Instant,
    budget: Duration,
    /// When the job itself started: at once on most lanes, on the
    /// StoreWorker thread for the write lane.
    started: Arc<OnceLock<Instant>>,
}

struct LaneState<T> {
    queue: VecDeque<Queued<T>>,
    closed: bool,
    dropped_refresh: u64,
    running: Option<Running>,
}

struct LaneSlot<T> {
    state: Mutex<LaneState<T>>,
    ready: Condvar,
}

struct Shared<T> {
    lanes: HashMap<Lane, LaneSlot<T>>,
    /// Keys queued or running (single-flight), with the work waiting on each.
    /// Lock order: a lane's state, then this map; never the reverse.
    in_flight: Mutex<HashMap<String, KeyFollowUps<T>>>,
    capacity: usize,
}

impl<T> Shared<T> {
    /// The key's job is done (run, or dropped from its queue). Returns the
    /// next follow-up to queue for it, keeping the key in flight, or frees the
    /// key when nothing waits on it.
    fn release_key(&self, key: &str) -> Option<Queued<T>> {
        let mut in_flight = lock(&self.in_flight);
        let followups = in_flight.get_mut(key)?;
        let next = followups
            .critical
            .pop_front()
            .or_else(|| followups.rerun.take());
        if next.is_none() {
            in_flight.remove(key);
        }
        next
    }
}

fn lock<M>(mutex: &Mutex<M>) -> std::sync::MutexGuard<'_, M> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runs [`JobSpec`]s on per-lane threads and collects their results.
pub(crate) struct JobRuntime<T: Send + 'static> {
    shared: Arc<Shared<T>>,
    completions: Mutex<mpsc::Receiver<JobResult<T>>>,
    threads: Vec<JoinHandle<()>>,
}

impl<T: Send + 'static> JobRuntime<T> {
    /// Start one thread per [`Lane`]. `lane_capacity` bounds each lane's
    /// queue: past it, refresh work replaces older refresh work, and critical
    /// work is still queued.
    pub(crate) fn start(lane_capacity: usize) -> Self {
        Self::start_with(lane_capacity, LaneWiring::daemon_writer())
    }

    /// [`JobRuntime::start`] with explicit wiring: the read lane's context
    /// factory and the single writer that write-lane jobs run on.
    pub(crate) fn start_with(lane_capacity: usize, wiring: LaneWiring) -> Self {
        let LaneWiring { reader, writer } = wiring;
        let mut reader = reader;
        let lanes = Lane::ALL
            .into_iter()
            .map(|lane| {
                (
                    lane,
                    LaneSlot {
                        state: Mutex::new(LaneState {
                            queue: VecDeque::new(),
                            closed: false,
                            dropped_refresh: 0,
                            running: None,
                        }),
                        ready: Condvar::new(),
                    },
                )
            })
            .collect();
        let shared = Arc::new(Shared {
            lanes,
            in_flight: Mutex::new(HashMap::new()),
            capacity: lane_capacity.max(1),
        });
        let (sender, receiver) = mpsc::channel();
        let threads = Lane::ALL
            .into_iter()
            .filter_map(|lane| {
                let thread_shared = Arc::clone(&shared);
                let sender = sender.clone();
                let lane_reader = if lane == Lane::Read {
                    reader.take()
                } else {
                    None
                };
                let lane_writer = (lane == Lane::Write).then(|| Arc::clone(&writer));
                match std::thread::Builder::new()
                    .name(lane.thread_name().into())
                    .spawn(move || {
                        run_lane(&thread_shared, lane, &sender, lane_reader, lane_writer)
                    }) {
                    Ok(handle) => Some(handle),
                    Err(error) => {
                        // A lane with no thread must refuse work, not swallow it
                        // (review round 1, item 7).
                        tracing::error!(?lane, %error, "could not start a factory job lane");
                        lock(&shared.lanes[&lane].state).closed = true;
                        None
                    }
                }
            })
            .collect();
        Self {
            shared,
            completions: Mutex::new(receiver),
            threads,
        }
    }

    /// Queue `work` under `spec`. Never blocks the caller beyond two short
    /// in-process mutexes. All jobs for one key must use the same lane; a
    /// submit for a key busy on another lane is refused with
    /// [`Submitted::LaneMismatch`].
    pub(crate) fn submit(
        &self,
        spec: JobSpec,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Submitted {
        self.enqueue(spec, Work::Plain(Box::new(work)))
    }

    /// Queue a read on [`Lane::Read`], which passes it the lane's dedicated
    /// context (`None` when the factory gave none or it is not a `C`).
    pub(crate) fn submit_read<C: 'static>(
        &self,
        mut spec: JobSpec,
        work: impl FnOnce(Option<&C>) -> T + Send + 'static,
    ) -> Submitted {
        spec.lane = Lane::Read;
        self.enqueue(
            spec,
            Work::Read(Box::new(move |ctx: Option<&dyn std::any::Any>| {
                work(ctx.and_then(|ctx| ctx.downcast_ref::<C>()))
            })),
        )
    }

    fn enqueue(&self, spec: JobSpec, work: Work<T>) -> Submitted {
        let Some(slot) = self.shared.lanes.get(&spec.lane) else {
            return Submitted::Closed;
        };
        let mut state = lock(&slot.state);
        if state.closed {
            return Submitted::Closed;
        }
        let queued = Queued {
            spec,
            work,
            enqueued_at: Instant::now(),
        };
        {
            let mut in_flight = lock(&self.shared.in_flight);
            if let Some(followups) = in_flight.get_mut(&queued.spec.key) {
                if followups.lane != queued.spec.lane {
                    tracing::error!(
                        kind = queued.spec.kind,
                        key = %queued.spec.key,
                        lane = ?queued.spec.lane,
                        busy_on = ?followups.lane,
                        "factory job refused: its key is busy on another lane"
                    );
                    return Submitted::LaneMismatch {
                        busy_on: followups.lane,
                    };
                }
                return match queued.spec.criticality {
                    Criticality::Critical => {
                        followups.critical.push_back(queued);
                        Submitted::QueuedBehindKey
                    }
                    Criticality::Refresh => {
                        let replaced_generation = followups
                            .rerun
                            .replace(queued)
                            .map(|replaced| replaced.spec.generation);
                        Submitted::Coalesced { replaced_generation }
                    }
                };
            }
            in_flight.insert(queued.spec.key.clone(), KeyFollowUps::new(queued.spec.lane));
        }
        let mut outcome = Submitted::Queued;
        if state.queue.len() >= self.shared.capacity
            && queued.spec.criticality == Criticality::Refresh
        {
            match state
                .queue
                .iter()
                .position(|q| q.spec.criticality == Criticality::Refresh)
            {
                Some(index) => {
                    let dropped = state.queue.remove(index).expect("index from position");
                    state.dropped_refresh += 1;
                    let next = self.shared.release_key(&dropped.spec.key);
                    let key_still_queued = next.is_some();
                    if let Some(next) = next {
                        state.queue.push_back(next);
                    }
                    outcome = Submitted::QueuedDroppedOldest {
                        dropped_key: dropped.spec.key,
                        dropped_generation: dropped.spec.generation,
                        key_still_queued,
                    };
                }
                None => {
                    state.dropped_refresh += 1;
                    if let Some(next) = self.shared.release_key(&queued.spec.key) {
                        state.queue.push_back(next);
                    }
                    return Submitted::DroppedRefresh;
                }
            }
        }
        state.queue.push_back(queued);
        drop(state);
        slot.ready.notify_one();
        outcome
    }

    /// Every finished result, without waiting.
    pub(crate) fn drain_completions(&self) -> Vec<JobResult<T>> {
        let receiver = lock(&self.completions);
        std::iter::from_fn(|| receiver.try_recv().ok()).collect()
    }

    /// Occupancy of one lane: waiting jobs (on the queue or behind a busy
    /// key, not the running one), the critical backlog, their oldest ages,
    /// refresh jobs dropped so far, and the running job.
    pub(crate) fn lane_stats(&self, lane: Lane) -> LaneStats {
        let Some(slot) = self.shared.lanes.get(&lane) else {
            return LaneStats::default();
        };
        // Lock order: lane state, then in_flight.
        let state = lock(&slot.state);
        let in_flight = lock(&self.shared.in_flight);
        let now = Instant::now();
        let mut stats = LaneStats {
            dropped_refresh: state.dropped_refresh,
            ..LaneStats::default()
        };
        let behind_keys = in_flight
            .values()
            .filter(|followups| followups.lane == lane)
            .flat_map(|followups| followups.critical.iter().chain(followups.rerun.iter()));
        for waiting in state.queue.iter().chain(behind_keys) {
            let age = now.saturating_duration_since(waiting.enqueued_at);
            stats.queued += 1;
            stats.oldest_queued_age = stats.oldest_queued_age.max(Some(age));
            if waiting.spec.criticality == Criticality::Critical {
                stats.critical_backlog += 1;
                stats.oldest_critical_age = stats.oldest_critical_age.max(Some(age));
            }
        }
        stats.running = state.running.as_ref().map(|running| {
            let started = running.started.get().copied();
            RunningJob {
                lane,
                kind: running.kind,
                key: running.key.clone(),
                age: started.map_or(Duration::ZERO, |at| now.saturating_duration_since(at)),
                budget: running.budget,
                waiting_for_writer: started.is_none(),
                writer_wait: started
                    .unwrap_or(now)
                    .saturating_duration_since(running.taken_at),
            }
        });
        stats
    }

    /// Jobs still running past their budget, and write jobs still waiting
    /// for a StoreWorker that has stalled past their budget, logged at WARN on
    /// `cas::loop_watchdog`. The watchdog (C1) calls this on its own cadence,
    /// so a wedged job is reported while it is stuck, not only when it ends.
    /// The two cases are logged apart, so a slow writer is not blamed on the
    /// job waiting for it.
    pub(crate) fn over_budget_running(&self) -> Vec<RunningJob> {
        let stuck: Vec<RunningJob> = Lane::ALL
            .into_iter()
            .filter_map(|lane| self.lane_stats(lane).running)
            .filter(|job| job.over_budget() || job.writer_stalled())
            .collect();
        for job in &stuck {
            if job.writer_stalled() {
                tracing::warn!(
                    target: "cas::loop_watchdog",
                    lane = ?job.lane,
                    kind = job.kind,
                    key = %job.key,
                    writer_wait_ms = job.writer_wait.as_millis() as u64,
                    budget_ms = job.budget.as_millis() as u64,
                    "factory write job still waiting for the store writer"
                );
            } else {
                tracing::warn!(
                    target: "cas::loop_watchdog",
                    lane = ?job.lane,
                    kind = job.kind,
                    key = %job.key,
                    elapsed_ms = job.age.as_millis() as u64,
                    budget_ms = job.budget.as_millis() as u64,
                    "factory job still running past its budget"
                );
            }
        }
        stuck
    }
}

impl<T: Send + 'static> JobRuntime<T> {
    /// Stop taking work: later submits return [`Submitted::Closed`]. Lanes
    /// still run every queued and waiting critical job and skip queued
    /// refresh work. Daemon shutdown calls this when the loop stops, then
    /// [`JobRuntime::drain`] (see there for the order against the
    /// StoreWorker).
    pub(crate) fn close(&self) {
        for slot in self.shared.lanes.values() {
            lock(&slot.state).closed = true;
            slot.ready.notify_all();
        }
    }

    /// Stop taking work and wait up to `timeout` for the lanes to finish:
    /// every queued and waiting critical job runs; queued refresh work is
    /// skipped. Returns true when every lane thread exited in time.
    ///
    /// **Shutdown order.** Write-lane jobs run on the StoreWorker, so call
    /// this after the loop has stopped and *before* the daemon waits for the
    /// StoreWorker (`store_worker.wait_idle` in `lifecycle.rs`, about line
    /// 2976) or drains it. Waiting on the StoreWorker first can report it idle
    /// while critical write jobs still sit on the write lane, and draining it
    /// first fails those writes ("the store writer is unavailable").
    pub(crate) fn drain(mut self, timeout: Duration) -> bool {
        self.close();
        let deadline = Instant::now() + timeout;
        while self.threads.iter().any(|handle| !handle.is_finished()) {
            if Instant::now() >= deadline {
                tracing::warn!("factory job lanes still busy at shutdown; leaving them to finish");
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
        true
    }
}

impl<T: Send + 'static> Drop for JobRuntime<T> {
    /// Stop taking work. The lane threads still run every queued and waiting
    /// critical job before they exit, even though nobody reads the results;
    /// the caller is not blocked. Use [`JobRuntime::drain`] to wait.
    fn drop(&mut self) {
        self.close();
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_string())
}

/// Run one job. Every panic, including one opening the read context, becomes
/// a [`JobPanicked`] outcome, so the caller always releases the key and posts
/// a result. `started` is set when the job itself begins.
fn run_work<T: Send + 'static>(
    work: Work<T>,
    label: &'static str,
    reader: &mut Option<ReadContextFactory>,
    context: &mut Option<Box<dyn std::any::Any>>,
    writer: Option<&Arc<dyn WriteExecutor>>,
    started: &Arc<OnceLock<Instant>>,
) -> Result<T, JobPanicked> {
    let caught = |panic: Box<dyn std::any::Any + Send>| JobPanicked {
        message: panic_message(panic.as_ref()),
    };
    match work {
        Work::Read(work) => {
            let _ = started.set(Instant::now());
            if let Some(open) = reader.take() {
                // A panicking factory must not wedge the lane (review round 2,
                // M1). This job fails; later reads run without a context, as
                // when the factory returns None.
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(open)) {
                    Ok(opened) => *context = opened,
                    Err(panic) => {
                        return Err(JobPanicked {
                            message: format!(
                                "opening the read context panicked: {}",
                                panic_message(panic.as_ref())
                            ),
                        });
                    }
                }
            }
            let ctx = context.as_deref();
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(ctx))).map_err(caught)
        }
        Work::Plain(work) => match writer {
            // Write-lane jobs run on the single writer, not on this thread.
            Some(writer) => {
                let (sender, receiver) = mpsc::channel();
                let started = Arc::clone(started);
                let job = Box::new(move || {
                    let _ = started.set(Instant::now());
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                        .map_err(|panic: Box<dyn std::any::Any + Send>| JobPanicked {
                            message: panic_message(panic.as_ref()),
                        });
                    let _ = sender.send(outcome);
                });
                if writer.execute(label, job) {
                    receiver.recv().unwrap_or_else(|_| {
                        Err(JobPanicked {
                            message: "the store writer dropped the job".to_string(),
                        })
                    })
                } else {
                    Err(JobPanicked {
                        message: "the store writer is unavailable".to_string(),
                    })
                }
            }
            None => {
                let _ = started.set(Instant::now());
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).map_err(caught)
            }
        },
    }
}

fn run_lane<T: Send + 'static>(
    shared: &Shared<T>,
    lane: Lane,
    completions: &mpsc::Sender<JobResult<T>>,
    mut reader: Option<ReadContextFactory>,
    writer: Option<Arc<dyn WriteExecutor>>,
) {
    let mut context: Option<Box<dyn std::any::Any>> = None;
    let slot = &shared.lanes[&lane];
    loop {
        let next = {
            let mut state = lock(&slot.state);
            loop {
                if let Some(queued) = state.queue.pop_front() {
                    // After close, refresh work is skipped; critical work
                    // still runs (review round 1, item 2).
                    if state.closed && queued.spec.criticality == Criticality::Refresh {
                        if let Some(follow_up) = shared.release_key(&queued.spec.key) {
                            state.queue.push_back(follow_up);
                        }
                        continue;
                    }
                    let started = Arc::new(OnceLock::new());
                    state.running = Some(Running {
                        kind: queued.spec.kind,
                        key: queued.spec.key.clone(),
                        taken_at: Instant::now(),
                        budget: queued.spec.budget,
                        started: Arc::clone(&started),
                    });
                    break Some((queued, started));
                }
                if state.closed {
                    break None;
                }
                state = slot
                    .ready
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        let Some((Queued { spec, work, .. }, started)) = next else {
            return;
        };
        let taken_at = Instant::now();
        let outcome = run_work(
            work,
            spec.kind,
            &mut reader,
            &mut context,
            writer.as_ref(),
            &started,
        );
        let finished = Instant::now();
        // Time the job itself; a write job's wait for the StoreWorker is
        // reported apart (review round 2, M3). A job that never started (the
        // writer was gone) ran for zero time.
        let began = started.get().copied().unwrap_or(finished);
        let elapsed = finished.saturating_duration_since(began);
        let writer_wait = if writer.is_some() {
            began.saturating_duration_since(taken_at)
        } else {
            Duration::ZERO
        };
        if let Err(panicked) = &outcome {
            tracing::error!(lane = ?lane, kind = spec.kind, key = %spec.key, message = %panicked.message, "factory job panicked");
        }
        let over_budget = elapsed > spec.budget;
        if over_budget {
            tracing::warn!(
                target: "cas::loop_watchdog",
                lane = ?lane,
                kind = spec.kind,
                key = %spec.key,
                elapsed_ms = elapsed.as_millis() as u64,
                budget_ms = spec.budget.as_millis() as u64,
                "factory job over budget"
            );
        }
        // Release the key before posting, so a caller that drains this result
        // can submit the key again at once. Work waiting on the key is queued
        // now, critical first.
        {
            let mut state = lock(&slot.state);
            state.running = None;
            if let Some(follow_up) = shared.release_key(&spec.key) {
                state.queue.push_back(follow_up);
            }
        }
        // A closed receiver (the runtime was dropped) must not stop the lane:
        // queued critical work still runs.
        let _ = completions.send(JobResult {
            kind: spec.kind,
            key: spec.key,
            generation: spec.generation,
            outcome,
            elapsed,
            writer_wait,
            over_budget,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn spec(key: &str, lane: Lane, criticality: Criticality, generation: u64) -> JobSpec {
        JobSpec {
            kind: "TestJob",
            key: key.to_string(),
            lane,
            criticality,
            generation,
            budget: Duration::from_millis(100),
        }
    }

    fn wait_for<T: Send + 'static>(runtime: &JobRuntime<T>, count: usize) -> Vec<JobResult<T>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut results = Vec::new();
        while results.len() < count && Instant::now() < deadline {
            results.extend(runtime.drain_completions());
            std::thread::sleep(Duration::from_millis(2));
        }
        results
    }

    /// spec-4c5b §7 gate test (C0): a refresh submit for a busy key is
    /// coalesced into one rerun with the newest closure; a critical submit
    /// queues behind the key and is never dropped; the runtime can deliver a
    /// stale generation, and the gate drops it.
    #[test]
    fn flv2_job_single_flight_and_generations() {
        let runtime: JobRuntime<u64> = JobRuntime::start(8);
        let release = Arc::new(Barrier::new(2));
        let runs = Arc::new(AtomicUsize::new(0));

        let (r, n) = (Arc::clone(&release), Arc::clone(&runs));
        assert_eq!(
            runtime.submit(
                spec("snapshot", Lane::Read, Criticality::Refresh, 1),
                move || {
                    n.fetch_add(1, Ordering::SeqCst);
                    r.wait();
                    1
                }
            ),
            Submitted::Queued
        );
        for generation in [2, 3] {
            let n = Arc::clone(&runs);
            assert_eq!(
                runtime.submit(
                    spec("snapshot", Lane::Read, Criticality::Refresh, generation),
                    move || {
                        n.fetch_add(1, Ordering::SeqCst);
                        generation
                    }
                ),
                Submitted::Coalesced {
                    replaced_generation: (generation > 2).then(|| generation - 1)
                },
                "a busy key is not run twice at once"
            );
        }
        release.wait();
        let results = wait_for(&runtime, 2);
        let generations: Vec<u64> = results.iter().map(|r| r.generation).collect();
        assert_eq!(
            generations,
            vec![1, 3],
            "one rerun, with the newest closure"
        );
        assert_eq!(
            runs.load(Ordering::SeqCst),
            2,
            "generation 2's closure never runs"
        );

        let mut gate = GenerationGate::default();
        assert!(gate.accept(&results[1]), "the newest generation is applied");
        assert!(
            !gate.accept(&results[0]),
            "a stale delivered result is dropped"
        );
        assert!(
            gate.accept(&results[1]),
            "re-applying the same generation is allowed"
        );

        // Critical work for a busy key queues behind it, in order.
        let release = Arc::new(Barrier::new(2));
        let r = Arc::clone(&release);
        assert_eq!(
            runtime.submit(
                spec("row-7", Lane::Write, Criticality::Critical, 1),
                move || {
                    r.wait();
                    1
                }
            ),
            Submitted::Queued
        );
        assert_eq!(
            runtime.submit(spec("row-7", Lane::Write, Criticality::Critical, 2), || 2),
            Submitted::QueuedBehindKey
        );
        assert_eq!(
            runtime.submit(spec("row-7", Lane::Write, Criticality::Critical, 3), || 3),
            Submitted::QueuedBehindKey
        );
        release.wait();
        let stamps: Vec<u64> = wait_for(&runtime, 3)
            .into_iter()
            .map(|r| r.outcome.expect("no panic"))
            .collect();
        assert_eq!(
            stamps,
            vec![1, 2, 3],
            "critical writes run in order, none dropped"
        );
    }

    /// spec-4c5b §7 gate test (C0): with its lane blocked, submits return at
    /// once; the oldest refresh job is dropped to make room; critical jobs are
    /// never dropped, and the lane reports the oldest queued age.
    #[test]
    fn flv2_job_queue_never_blocks_submitter() {
        let runtime: JobRuntime<&'static str> = JobRuntime::start(2);
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (s, r) = (Arc::clone(&started), Arc::clone(&release));
        assert_eq!(
            runtime.submit(
                spec("blocker", Lane::Write, Criticality::Critical, 1),
                move || {
                    s.wait();
                    r.wait();
                    "blocker"
                }
            ),
            Submitted::Queued
        );
        // The blocker is running, so everything below is backlog.
        started.wait();

        // Every submit below returns while the lane is blocked; if one waited
        // for room, the test would hang here (no wall-clock assertion).
        assert_eq!(
            runtime.submit(
                spec("refresh-a", Lane::Write, Criticality::Refresh, 1),
                || "a"
            ),
            Submitted::Queued
        );
        assert_eq!(
            runtime.submit(
                spec("refresh-b", Lane::Write, Criticality::Refresh, 1),
                || "b"
            ),
            Submitted::Queued
        );
        assert_eq!(
            runtime.submit(
                spec("refresh-c", Lane::Write, Criticality::Refresh, 1),
                || "c"
            ),
            Submitted::QueuedDroppedOldest {
                dropped_key: "refresh-a".to_string(),
                dropped_generation: 1,
                key_still_queued: false,
            }
        );
        for i in 0..5 {
            assert_eq!(
                runtime.submit(
                    spec(
                        &format!("receipt-{i}"),
                        Lane::Write,
                        Criticality::Critical,
                        1
                    ),
                    || "receipt"
                ),
                Submitted::Queued,
                "critical work is never dropped"
            );
        }
        let stats = runtime.lane_stats(Lane::Write);
        assert_eq!(stats.dropped_refresh, 1);
        assert_eq!(stats.queued, 7, "{stats:?}");
        assert_eq!(stats.critical_backlog, 5, "{stats:?}");
        assert!(stats.oldest_queued_age.is_some());
        assert!(stats.oldest_critical_age.is_some());

        release.wait();
        let results = wait_for(&runtime, 8);
        let keys: Vec<&str> = results.iter().map(|r| r.key.as_str()).collect();
        assert!(!keys.contains(&"refresh-a"), "{keys:?}");
        assert_eq!(keys.iter().filter(|k| k.starts_with("receipt-")).count(), 5);
        assert!(keys.contains(&"refresh-b") && keys.contains(&"refresh-c"));
    }

    /// Review round 1, items 6 and 8: a wedged running job is visible in the
    /// lane stats and reported while still over budget; the gate can advance
    /// without a job and forget keys.
    #[test]
    fn flv2_job_running_job_visible_and_gate_bounded() {
        let runtime: JobRuntime<()> = JobRuntime::start(4);
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (s, r) = (Arc::clone(&started), Arc::clone(&release));
        let mut slow = spec("fetch", Lane::Blocking, Criticality::Refresh, 1);
        slow.budget = Duration::from_millis(1);
        runtime.submit(slow, move || {
            s.wait();
            r.wait();
        });
        started.wait();
        std::thread::sleep(Duration::from_millis(5));
        let stats = runtime.lane_stats(Lane::Blocking);
        let running = stats.running.expect("the wedged job is visible");
        assert_eq!((running.kind, running.key.as_str()), ("TestJob", "fetch"));
        assert!(running.over_budget());
        assert_eq!(runtime.over_budget_running().len(), 1);
        release.wait();
        wait_for(&runtime, 1);
        assert!(runtime.lane_stats(Lane::Blocking).running.is_none());

        let mut gate = GenerationGate::default();
        gate.advance("row-1", 5);
        let stale = JobResult {
            kind: "TestJob",
            key: "row-1".to_string(),
            generation: 4,
            outcome: Ok(()),
            elapsed: Duration::ZERO,
            writer_wait: Duration::ZERO,
            over_budget: false,
        };
        assert!(
            !gate.accept(&stale),
            "advance drops older in-flight results"
        );
        gate.advance("row-2", 1);
        gate.forget("row-1");
        gate.retain(|key| key != "row-2");
        assert_eq!(gate.len(), 0, "keys stay bounded");
    }

    /// spec §4.2 / review round 1, item 11: a failing key backs off
    /// exponentially up to the cap; a success clears it. Fake clock.
    #[test]
    fn flv2_job_backoff_is_exponential_and_capped() {
        let mut backoff = Backoff::new(Duration::from_secs(5), Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(backoff.ready("fetch", t0));
        let delays: Vec<u64> = (0..6)
            .map(|_| backoff.record_failure("fetch", t0).as_secs())
            .collect();
        assert_eq!(delays, vec![5, 10, 20, 40, 60, 60]);
        assert!(!backoff.ready("fetch", t0 + Duration::from_secs(59)));
        assert!(backoff.ready("fetch", t0 + Duration::from_secs(60)));
        assert!(backoff.ready("other", t0), "keys back off independently");
        backoff.record_success("fetch");
        assert!(backoff.ready("fetch", t0));
    }

    /// Review round 1, item 5: a slow external call never stalls local
    /// blocking work.
    #[test]
    fn flv2_job_external_lane_does_not_block_local_probes() {
        let runtime: JobRuntime<&'static str> = JobRuntime::start(4);
        let release = Arc::new(Barrier::new(2));
        let r = Arc::clone(&release);
        runtime.submit(
            spec("git-fetch", Lane::External, Criticality::Refresh, 1),
            move || {
                r.wait();
                "fetched"
            },
        );
        runtime.submit(
            spec("health-probe", Lane::Blocking, Criticality::Refresh, 1),
            || "probed",
        );
        let first = wait_for(&runtime, 1);
        assert_eq!(
            first[0].key, "health-probe",
            "the probe finished while the fetch was stuck"
        );
        release.wait();
        assert_eq!(wait_for(&runtime, 1)[0].key, "git-fetch");
    }

    /// Review round 1, item 3: write-lane jobs run on the single store
    /// writer's thread, not a second writer; read jobs share one dedicated
    /// context, opened once on the read thread.
    #[test]
    fn flv2_job_writes_on_store_worker_and_reads_share_one_context() {
        let opens = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&opens);
        let reader: ReadContextFactory = Box::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Some(Box::new(41_u32) as Box<dyn std::any::Any>)
        });
        let runtime: JobRuntime<String> = JobRuntime::start_with(
            4,
            LaneWiring {
                reader: Some(reader),
                writer: Arc::new(super::super::store_worker::StoreWorker::start()),
            },
        );
        runtime.submit(spec("stamp", Lane::Write, Criticality::Critical, 1), || {
            std::thread::current().name().unwrap_or("").to_string()
        });
        let wrote = wait_for(&runtime, 1).remove(0).outcome.expect("no panic");
        assert_eq!(
            wrote, "factory-store-worker",
            "writes run on the single writer"
        );

        for key in ["read-a", "read-b"] {
            runtime.submit_read(
                spec(key, Lane::Read, Criticality::Refresh, 1),
                |ctx: Option<&u32>| format!("{:?}", ctx.map(|value| value + 1)),
            );
        }
        let reads: Vec<String> = wait_for(&runtime, 2)
            .into_iter()
            .map(|r| r.outcome.expect("no panic"))
            .collect();
        assert_eq!(reads, vec!["Some(42)", "Some(42)"]);
        assert_eq!(
            opens.load(Ordering::SeqCst),
            1,
            "the read context opens once"
        );
    }

    /// Review round 1, item 4: a panicking job posts a result and frees its key.
    #[test]
    fn flv2_job_panic_posts_result() {
        let runtime: JobRuntime<u8> = JobRuntime::start(4);
        assert_eq!(
            runtime.submit(
                spec("probe", Lane::Blocking, Criticality::Refresh, 1),
                || { panic!("probe exploded") }
            ),
            Submitted::Queued
        );
        let results = wait_for(&runtime, 1);
        assert_eq!(
            results[0]
                .outcome
                .as_ref()
                .err()
                .map(|p| p.message.as_str()),
            Some("probe exploded")
        );
        assert_eq!(
            runtime.submit(spec("probe", Lane::Blocking, Criticality::Refresh, 2), || 2),
            Submitted::Queued,
            "the key is free after a panic"
        );
    }

    /// Review round 1, item 2: dropping or draining the runtime still runs
    /// every queued and waiting critical job; queued refresh work is skipped.
    #[test]
    fn flv2_job_shutdown_runs_queued_critical_work() {
        let runtime: JobRuntime<()> = JobRuntime::start(4);
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let ran = Arc::new(Mutex::new(Vec::new()));
        let (s, r, log) = (Arc::clone(&started), Arc::clone(&release), Arc::clone(&ran));
        runtime.submit(
            spec("row-1", Lane::Write, Criticality::Critical, 1),
            move || {
                s.wait();
                r.wait();
                log.lock().unwrap().push("row-1");
            },
        );
        started.wait();
        for (key, criticality) in [
            ("row-1", Criticality::Critical),
            ("row-2", Criticality::Critical),
            ("snapshot", Criticality::Refresh),
        ] {
            let log = Arc::clone(&ran);
            runtime.submit(spec(key, Lane::Write, criticality, 2), move || {
                log.lock().unwrap().push(key);
            });
        }
        // Close while row-1 still runs, so the backlog is drained under
        // shutdown rules, then let row-1 finish (no sleep, review round 2 L1).
        runtime.close();
        release.wait();
        assert!(
            runtime.drain(Duration::from_secs(5)),
            "lanes exit after the backlog"
        );
        let ran = ran.lock().unwrap().clone();
        assert_eq!(ran.iter().filter(|k| **k == "row-1").count(), 2, "{ran:?}");
        assert!(ran.contains(&"row-2"), "{ran:?}");
        assert!(
            !ran.contains(&"snapshot"),
            "refresh work is skipped at shutdown: {ran:?}"
        );
    }

    /// Review round 1, items 2 and 10: dropping the runtime (no drain) still
    /// runs the queued critical backlog, including critical work waiting on a
    /// busy key; queued refresh work is skipped.
    #[test]
    fn flv2_job_drop_runs_queued_critical_work() {
        let runtime: JobRuntime<()> = JobRuntime::start(4);
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let ran = Arc::new(Mutex::new(Vec::new()));
        let (s, r, log) = (Arc::clone(&started), Arc::clone(&release), Arc::clone(&ran));
        runtime.submit(
            spec("blocker", Lane::Blocking, Criticality::Critical, 1),
            move || {
                s.wait();
                r.wait();
                log.lock().unwrap().push("blocker");
            },
        );
        started.wait();
        for (key, criticality) in [
            ("blocker", Criticality::Critical),
            ("receipt", Criticality::Critical),
            ("probe", Criticality::Refresh),
        ] {
            let log = Arc::clone(&ran);
            runtime.submit(spec(key, Lane::Blocking, criticality, 2), move || {
                log.lock().unwrap().push(key);
            });
        }
        drop(runtime);
        release.wait();
        let deadline = Instant::now() + Duration::from_secs(5);
        while ran.lock().unwrap().len() < 3 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        // Give a wrongly-run refresh job time to show up before asserting.
        std::thread::sleep(Duration::from_millis(20));
        let ran = ran.lock().unwrap().clone();
        assert_eq!(
            ran.iter().filter(|k| **k == "blocker").count(),
            2,
            "{ran:?}"
        );
        assert!(ran.contains(&"receipt"), "{ran:?}");
        assert!(!ran.contains(&"probe"), "refresh work is skipped: {ran:?}");
    }

    /// Review round 1, items 7 and 11: a closed runtime refuses work with
    /// `Closed` instead of queuing it where nothing will run it.
    #[test]
    fn flv2_job_closed_runtime_refuses_work() {
        let runtime: JobRuntime<u8> = JobRuntime::start(4);
        runtime.close();
        for lane in Lane::ALL {
            assert_eq!(
                runtime.submit(spec("late", lane, Criticality::Critical, 1), || 1),
                Submitted::Closed,
                "{lane:?}"
            );
        }
        assert!(runtime.drain(Duration::from_secs(5)));
    }

    /// Review round 2, M1: a read-context factory that panics fails that job,
    /// frees its key and leaves the Read lane running (without a context).
    #[test]
    fn flv2_job_read_context_panic_does_not_wedge_the_lane() {
        let reader: ReadContextFactory = Box::new(|| panic!("connection exploded"));
        let runtime: JobRuntime<String> = JobRuntime::start_with(
            4,
            LaneWiring {
                reader: Some(reader),
                writer: Arc::new(GlobalStoreWorker),
            },
        );
        assert_eq!(
            runtime.submit_read(
                spec("snapshot", Lane::Read, Criticality::Refresh, 1),
                |_: Option<&u32>| "ran".to_string()
            ),
            Submitted::Queued
        );
        let first = wait_for(&runtime, 1).remove(0);
        let message = first
            .outcome
            .expect_err("the open panic fails the job")
            .message;
        assert!(message.contains("connection exploded"), "{message}");
        assert!(runtime.lane_stats(Lane::Read).running.is_none());
        assert_eq!(
            runtime.submit_read(
                spec("snapshot", Lane::Read, Criticality::Refresh, 2),
                |ctx: Option<&u32>| format!("{ctx:?}")
            ),
            Submitted::Queued,
            "the key is free"
        );
        let second = wait_for(&runtime, 1).remove(0);
        assert_eq!(second.outcome.expect("no panic"), "None");
    }

    /// Review round 2, M2: critical work waiting behind a busy key counts in
    /// the lane's queued total and critical backlog age.
    #[test]
    fn flv2_job_critical_behind_busy_key_is_counted() {
        let runtime: JobRuntime<()> = JobRuntime::start(4);
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (s, r) = (Arc::clone(&started), Arc::clone(&release));
        runtime.submit(
            spec("row-1", Lane::Blocking, Criticality::Critical, 1),
            move || {
                s.wait();
                r.wait();
            },
        );
        started.wait();
        for generation in [2, 3] {
            assert_eq!(
                runtime.submit(
                    spec("row-1", Lane::Blocking, Criticality::Critical, generation),
                    || ()
                ),
                Submitted::QueuedBehindKey
            );
        }
        runtime.submit(
            spec("row-1", Lane::Blocking, Criticality::Refresh, 4),
            || (),
        );
        let stats = runtime.lane_stats(Lane::Blocking);
        assert_eq!(stats.queued, 3, "{stats:?}");
        assert_eq!(stats.critical_backlog, 2, "{stats:?}");
        assert!(stats.oldest_critical_age.is_some(), "{stats:?}");
        assert!(stats.running.is_some());
        release.wait();
        assert_eq!(wait_for(&runtime, 4).len(), 4);
        let stats = runtime.lane_stats(Lane::Blocking);
        assert_eq!((stats.queued, stats.critical_backlog), (0, 0), "{stats:?}");
    }

    /// A writer that holds each job until the test opens its gate, standing in
    /// for a StoreWorker backlog or deferred-write retry.
    struct GatedWriter {
        gate: Arc<Barrier>,
    }

    impl WriteExecutor for GatedWriter {
        fn execute(&self, _label: &'static str, job: Box<dyn FnOnce() + Send + 'static>) -> bool {
            let gate = Arc::clone(&self.gate);
            std::thread::spawn(move || {
                gate.wait();
                job();
            });
            true
        }
    }

    /// Review round 2, M3: time a write job waits for the StoreWorker is
    /// reported apart from the job's own run time and budget.
    #[test]
    fn flv2_job_writer_wait_is_not_charged_to_the_job() {
        let gate = Arc::new(Barrier::new(2));
        let runtime: JobRuntime<()> = JobRuntime::start_with(
            4,
            LaneWiring {
                reader: None,
                writer: Arc::new(GatedWriter {
                    gate: Arc::clone(&gate),
                }),
            },
        );
        let mut stamp = spec("stamp", Lane::Write, Criticality::Critical, 1);
        stamp.budget = Duration::from_millis(20);
        runtime.submit(stamp, || ());
        let deadline = Instant::now() + Duration::from_secs(5);
        let waiting = loop {
            let running = runtime.lane_stats(Lane::Write).running;
            if let Some(job) = running.filter(|job| job.writer_wait > Duration::from_millis(40)) {
                break job;
            }
            assert!(Instant::now() < deadline, "the write job never queued");
            std::thread::sleep(Duration::from_millis(2));
        };
        assert!(waiting.waiting_for_writer);
        assert_eq!(waiting.age, Duration::ZERO);
        assert!(waiting.writer_stalled() && !waiting.over_budget());
        let stuck = runtime.over_budget_running();
        assert_eq!(stuck.len(), 1);
        assert!(stuck[0].writer_stalled());
        gate.wait();
        let result = wait_for(&runtime, 1).remove(0);
        assert!(
            result.writer_wait >= Duration::from_millis(40),
            "{result:?}"
        );
        assert!(
            !result.over_budget,
            "the writer's backlog is not the job's: {result:?}"
        );
        assert!(result.elapsed < result.writer_wait, "{result:?}");
    }

    /// Review round 2, L2: a key busy on one lane refuses a submit on another
    /// lane in every build, instead of running it off the single writer.
    #[test]
    fn flv2_job_mixed_lanes_for_a_key_are_refused() {
        let runtime: JobRuntime<u8> = JobRuntime::start(4);
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (s, r) = (Arc::clone(&started), Arc::clone(&release));
        runtime.submit(
            spec("row-9", Lane::Blocking, Criticality::Refresh, 1),
            move || {
                s.wait();
                r.wait();
                1
            },
        );
        started.wait();
        assert_eq!(
            runtime.submit(spec("row-9", Lane::Write, Criticality::Critical, 2), || 2),
            Submitted::LaneMismatch {
                busy_on: Lane::Blocking
            }
        );
        release.wait();
        let results = wait_for(&runtime, 1);
        assert_eq!(results.len(), 1);
        assert_eq!(
            runtime.submit(spec("row-9", Lane::Write, Criticality::Critical, 3), || 3),
            Submitted::Queued,
            "once the key is free, another lane may take it"
        );
        assert_eq!(wait_for(&runtime, 1)[0].outcome, Ok(3));
    }

    /// Review round 2: a panic inside a write-lane job posts a result, frees
    /// the key and leaves the StoreWorker serving later writes.
    #[test]
    fn flv2_job_write_lane_panic_posts_result() {
        let runtime: JobRuntime<String> = JobRuntime::start_with(
            4,
            LaneWiring {
                reader: None,
                writer: Arc::new(super::super::store_worker::StoreWorker::start()),
            },
        );
        assert_eq!(
            runtime.submit(
                spec("stamp", Lane::Write, Criticality::Critical, 1),
                || panic!("stamp exploded")
            ),
            Submitted::Queued
        );
        let failed = wait_for(&runtime, 1).remove(0);
        assert_eq!(
            failed.outcome.expect_err("panicked").message,
            "stamp exploded"
        );
        assert_eq!(
            runtime.submit(spec("stamp", Lane::Write, Criticality::Critical, 2), || {
                std::thread::current().name().unwrap_or("").to_string()
            }),
            Submitted::Queued,
            "the key is free after a panic"
        );
        assert_eq!(
            wait_for(&runtime, 1).remove(0).outcome.expect("no panic"),
            "factory-store-worker"
        );
    }
}
