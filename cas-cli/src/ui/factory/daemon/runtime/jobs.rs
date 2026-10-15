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
//!   behind writes. In the daemon, that thread holds the refresh reader's
//!   dedicated connection. [`Lane::Write`] is the single writer.
//!   [`Lane::Blocking`] runs everything else: git, gh, `/proc`, transcripts,
//!   network.
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
use std::sync::{Arc, Condvar, Mutex, mpsc};
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
    pub(crate) elapsed: Duration,
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
    /// a newer generation is never lost.
    Coalesced,
    /// A critical job whose key was already queued or running. It runs after
    /// that job, in submission order; it is never dropped.
    QueuedBehindKey,
    /// Queued; to make room, the oldest refresh job on the lane was dropped.
    QueuedDroppedOldest { dropped_key: String },
    /// The lane was full of critical work and this refresh job was dropped.
    DroppedRefresh,
    /// The runtime has shut down.
    Closed,
}

/// The job a lane is running right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunningJob {
    pub(crate) lane: Lane,
    pub(crate) kind: &'static str,
    pub(crate) key: String,
    pub(crate) age: Duration,
    pub(crate) budget: Duration,
}

impl RunningJob {
    pub(crate) fn over_budget(&self) -> bool {
        self.age > self.budget
    }
}

/// Lane occupancy, for the watchdog and `loop.json`. `running` makes a job
/// that is wedged mid-run visible (review round 1, item 6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LaneStats {
    pub(crate) queued: usize,
    pub(crate) oldest_queued_age: Option<Duration>,
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

type Work<T> = Box<dyn FnOnce() -> T + Send + 'static>;

/// Work waiting for a key that is queued or running.
struct KeyFollowUps<T> {
    /// Critical submits for the key, run in order after it.
    critical: VecDeque<Queued<T>>,
    /// The newest refresh submitted while the key was busy (dirty flag).
    rerun: Option<Queued<T>>,
}

impl<T> Default for KeyFollowUps<T> {
    fn default() -> Self {
        Self {
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

struct LaneState<T> {
    queue: VecDeque<Queued<T>>,
    closed: bool,
    dropped_refresh: u64,
    /// `(kind, key, started_at, budget)` of the job running now.
    running: Option<(&'static str, String, Instant, Duration)>,
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
                match std::thread::Builder::new()
                    .name(lane.thread_name().into())
                    .spawn(move || run_lane(&thread_shared, lane, &sender))
                {
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
    /// in-process mutexes. All jobs for one key must use the same lane.
    pub(crate) fn submit(
        &self,
        spec: JobSpec,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Submitted {
        let Some(slot) = self.shared.lanes.get(&spec.lane) else {
            return Submitted::Closed;
        };
        let mut state = lock(&slot.state);
        if state.closed {
            return Submitted::Closed;
        }
        let queued = Queued {
            spec,
            work: Box::new(work),
            enqueued_at: Instant::now(),
        };
        {
            let mut in_flight = lock(&self.shared.in_flight);
            if let Some(followups) = in_flight.get_mut(&queued.spec.key) {
                return match queued.spec.criticality {
                    Criticality::Critical => {
                        followups.critical.push_back(queued);
                        Submitted::QueuedBehindKey
                    }
                    Criticality::Refresh => {
                        followups.rerun = Some(queued);
                        Submitted::Coalesced
                    }
                };
            }
            in_flight.insert(queued.spec.key.clone(), KeyFollowUps::default());
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
                    if let Some(next) = self.shared.release_key(&dropped.spec.key) {
                        state.queue.push_back(next);
                    }
                    outcome = Submitted::QueuedDroppedOldest {
                        dropped_key: dropped.spec.key,
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

    /// Occupancy of one lane: queued jobs (not counting the one running), the
    /// age of the oldest, and refresh jobs dropped so far.
    pub(crate) fn lane_stats(&self, lane: Lane) -> LaneStats {
        let Some(slot) = self.shared.lanes.get(&lane) else {
            return LaneStats::default();
        };
        let state = lock(&slot.state);
        LaneStats {
            queued: state.queue.len(),
            oldest_queued_age: state
                .queue
                .front()
                .map(|queued| queued.enqueued_at.elapsed()),
            dropped_refresh: state.dropped_refresh,
            running: state
                .running
                .as_ref()
                .map(|(kind, key, started, budget)| RunningJob {
                    lane,
                    kind: *kind,
                    key: key.clone(),
                    age: started.elapsed(),
                    budget: *budget,
                }),
        }
    }

    /// Jobs still running past their budget, logged at WARN on
    /// `cas::loop_watchdog`. The watchdog (C1) calls this on its own cadence,
    /// so a wedged job is reported while it is stuck, not only when it ends.
    pub(crate) fn over_budget_running(&self) -> Vec<RunningJob> {
        let stuck: Vec<RunningJob> = Lane::ALL
            .into_iter()
            .filter_map(|lane| self.lane_stats(lane).running)
            .filter(RunningJob::over_budget)
            .collect();
        for job in &stuck {
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
        stuck
    }
}

impl<T: Send + 'static> JobRuntime<T> {
    fn close(&self) {
        for slot in self.shared.lanes.values() {
            lock(&slot.state).closed = true;
            slot.ready.notify_all();
        }
    }

    /// Stop taking work and wait up to `timeout` for the lanes to finish:
    /// every queued and waiting critical job runs; queued refresh work is
    /// skipped. Returns true when every lane thread exited in time. Used at
    /// daemon shutdown, after the loop has stopped (like `StoreWorker::drain`).
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

fn run_lane<T: Send + 'static>(
    shared: &Shared<T>,
    lane: Lane,
    completions: &mpsc::Sender<JobResult<T>>,
) {
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
                    state.running = Some((
                        queued.spec.kind,
                        queued.spec.key.clone(),
                        Instant::now(),
                        queued.spec.budget,
                    ));
                    break Some(queued);
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
        let Some(Queued { spec, work, .. }) = next else {
            return;
        };
        let started = Instant::now();
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).map_err(|panic| {
                JobPanicked {
                    message: panic_message(panic.as_ref()),
                }
            });
        let elapsed = started.elapsed();
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
                Submitted::Coalesced,
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

        let submitting = Instant::now();
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
                dropped_key: "refresh-a".to_string()
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
        assert!(
            submitting.elapsed() < Duration::from_millis(50),
            "submitting to a blocked, full lane never waits: {:?}",
            submitting.elapsed()
        );
        let stats = runtime.lane_stats(Lane::Write);
        assert_eq!(stats.dropped_refresh, 1);
        assert!(stats.queued >= 7, "{stats:?}");
        assert!(stats.oldest_queued_age.is_some());

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
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            release.wait();
        });
        assert!(
            runtime.drain(Duration::from_secs(5)),
            "lanes exit after the backlog"
        );
        releaser.join().unwrap();
        let ran = ran.lock().unwrap().clone();
        assert_eq!(ran.iter().filter(|k| **k == "row-1").count(), 2, "{ran:?}");
        assert!(ran.contains(&"row-2"), "{ran:?}");
        assert!(
            !ran.contains(&"snapshot"),
            "refresh work is skipped at shutdown: {ran:?}"
        );
    }
}
