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
    /// Everything else that can block: git, gh, `/proc`, transcripts, HTTP.
    Blocking,
}

impl Lane {
    const ALL: [Lane; 3] = [Lane::Read, Lane::Write, Lane::Blocking];

    fn thread_name(self) -> &'static str {
        match self {
            Lane::Read => "factory-jobs-read",
            Lane::Write => "factory-jobs-write",
            Lane::Blocking => "factory-jobs-blocking",
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

/// A finished job, as posted to the completion queue.
#[derive(Debug)]
pub(crate) struct JobResult<T> {
    pub(crate) kind: &'static str,
    pub(crate) key: String,
    pub(crate) generation: u64,
    pub(crate) outcome: T,
    pub(crate) elapsed: Duration,
    /// True when the run took longer than its budget.
    pub(crate) over_budget: bool,
}

/// What [`JobRuntime::submit`] did. It never blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Submitted {
    /// Queued to run.
    Queued,
    /// A job with the same key was already queued or running; not run again.
    Coalesced,
    /// Queued; to make room, the oldest refresh job on the lane was dropped.
    QueuedDroppedOldest { dropped_key: String },
    /// The lane was full of critical work and this refresh job was dropped.
    DroppedRefresh,
    /// The runtime has shut down.
    Closed,
}

/// Lane occupancy, for the watchdog and `loop.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LaneStats {
    pub(crate) queued: usize,
    pub(crate) oldest_queued_age: Option<Duration>,
    pub(crate) dropped_refresh: u64,
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
        let _ = result;
        unimplemented!("cas-1240: generation gate")
    }
}

type Work<T> = Box<dyn FnOnce() -> T + Send + 'static>;

/// Runs [`JobSpec`]s on per-lane threads and collects their results.
pub(crate) struct JobRuntime<T: Send + 'static> {
    _marker: std::marker::PhantomData<T>,
}

impl<T: Send + 'static> JobRuntime<T> {
    /// Start one thread per [`Lane`]. `lane_capacity` bounds each lane's
    /// queue of refresh work.
    pub(crate) fn start(lane_capacity: usize) -> Self {
        let _ = lane_capacity;
        unimplemented!("cas-1240: job runtime")
    }

    /// Queue `work` under `spec`. Never blocks the caller.
    pub(crate) fn submit(
        &self,
        spec: JobSpec,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Submitted {
        let _ = (spec, Box::new(work) as Work<T>);
        unimplemented!("cas-1240: submit")
    }

    /// Every finished result, without waiting.
    pub(crate) fn drain_completions(&self) -> Vec<JobResult<T>> {
        unimplemented!("cas-1240: drain")
    }

    /// Occupancy of one lane.
    pub(crate) fn lane_stats(&self, lane: Lane) -> LaneStats {
        let _ = lane;
        unimplemented!("cas-1240: stats")
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

    fn wait_for<T: Send + 'static>(
        runtime: &JobRuntime<T>,
        count: usize,
    ) -> Vec<JobResult<T>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut results = Vec::new();
        while results.len() < count && Instant::now() < deadline {
            results.extend(runtime.drain_completions());
            std::thread::sleep(Duration::from_millis(2));
        }
        results
    }

    /// spec-4c5b §7 gate test (C0): a second submit for an in-flight key
    /// coalesces and its work never runs; a result older than the applied
    /// generation is dropped by the gate.
    #[test]
    fn flv2_job_single_flight_and_generations() {
        let runtime: JobRuntime<u64> = JobRuntime::start(8);
        let release = Arc::new(Barrier::new(2));
        let runs = Arc::new(AtomicUsize::new(0));

        let (r, n) = (Arc::clone(&release), Arc::clone(&runs));
        assert_eq!(
            runtime.submit(spec("snapshot", Lane::Read, Criticality::Refresh, 1), move || {
                n.fetch_add(1, Ordering::SeqCst);
                r.wait();
                1
            }),
            Submitted::Queued
        );
        let n = Arc::clone(&runs);
        assert_eq!(
            runtime.submit(spec("snapshot", Lane::Read, Criticality::Refresh, 2), move || {
                n.fetch_add(1, Ordering::SeqCst);
                2
            }),
            Submitted::Coalesced,
            "a key in flight is not run twice"
        );
        release.wait();
        let results = wait_for(&runtime, 1);
        assert_eq!(results.len(), 1);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert_eq!(results[0].generation, 1);

        // Once finished, the key is free again.
        assert_eq!(
            runtime.submit(spec("snapshot", Lane::Read, Criticality::Refresh, 3), || 3),
            Submitted::Queued
        );
        let newer = wait_for(&runtime, 1);
        assert_eq!(newer[0].generation, 3);

        let mut gate = GenerationGate::default();
        assert!(gate.accept(&newer[0]), "the newest generation is applied");
        assert!(!gate.accept(&results[0]), "an older generation is dropped");
        assert!(gate.accept(&newer[0]), "re-applying the same generation is allowed");
    }

    /// spec-4c5b §7 gate test (C0): with its lane blocked, submits return at
    /// once; the oldest refresh job is dropped to make room; critical jobs are
    /// never dropped, and the lane reports the oldest queued age.
    #[test]
    fn flv2_job_queue_never_blocks_submitter() {
        let runtime: JobRuntime<&'static str> = JobRuntime::start(2);
        let release = Arc::new(Barrier::new(2));
        let r = Arc::clone(&release);
        assert_eq!(
            runtime.submit(spec("blocker", Lane::Write, Criticality::Critical, 1), move || {
                r.wait();
                "blocker"
            }),
            Submitted::Queued
        );
        // Wait until the blocker is running, so the queue below is the backlog.
        std::thread::sleep(Duration::from_millis(50));

        let started = Instant::now();
        assert_eq!(
            runtime.submit(spec("refresh-a", Lane::Write, Criticality::Refresh, 1), || "a"),
            Submitted::Queued
        );
        assert_eq!(
            runtime.submit(spec("refresh-b", Lane::Write, Criticality::Refresh, 1), || "b"),
            Submitted::Queued
        );
        assert_eq!(
            runtime.submit(spec("refresh-c", Lane::Write, Criticality::Refresh, 1), || "c"),
            Submitted::QueuedDroppedOldest {
                dropped_key: "refresh-a".to_string()
            }
        );
        for i in 0..5 {
            assert_eq!(
                runtime.submit(
                    spec(&format!("receipt-{i}"), Lane::Write, Criticality::Critical, 1),
                    || "receipt"
                ),
                Submitted::Queued,
                "critical work is never dropped"
            );
        }
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "submitting to a blocked, full lane never waits: {:?}",
            started.elapsed()
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
}
