//! The factory director's two-second refresh reads, off the loop thread
//! (cas-ee9ab), as factory loop v2 jobs (cas-223f).
//!
//! Measured on a 1.6 GB store at N=16 (cas-98b24): 13.9% of daemon passes
//! took 100 ms or more, on the refresh cadence. Each refresh tick read every
//! task, agent and recent event on the loop thread (the full director
//! snapshot), and the delivery stage read the same snapshot again before it
//! revalidated events. Profiling the passes by phase put almost all of the
//! refresh step's time in those two reads.
//!
//! They now run as jobs on the daemon's one job runtime, owned by the prompt
//! queue's [`QueueJobs`] (C3) and submitted through its external API (kinds
//! and specs in [`super::factory_jobs`]): the snapshot (`DirectorSnapshot`)
//! and the fresh re-reads (`RevalidateFresh`) on the read lane, which keeps
//! its own SQLite connection, stall confirmation (`WorkerHealthProbe`) on the
//! blocking lane, and the git half of the supervisor-stall evidence
//! (`CiStatus`) on the external lane. The loop starts a job when it is due,
//! keeps passing, and applies the finished result on a later pass. Panels are
//! stale by at most one refresh. Delivery keeps its
//! fresh-read-after-detection order: events detected from one read are
//! revalidated against a second read started after detection.
//!
//! Every job the loop waits on is tracked by generation. A job that fails
//! (it panicked, the runtime dropped it, or it outlived
//! [`factory_jobs::in_flight_timeout`]) frees its slot so the next pass
//! retries, and a late result for a slot already given up is ignored.
//!
//! The idle-worker liveness a delivery read is revalidated with (agent store,
//! transcripts, `/proc`) runs beside it as a job on the blocking lane under
//! the same generation (cas-3b16); the read is applied once both report, and
//! a liveness job that fails leaves those workers unobserved (uncertain, so
//! their prompts are delivered).

use std::collections::HashMap;
use std::time::{Duration, Instant};
#[cfg(test)]
use std::sync::Arc;

use super::factory_jobs::{self, DeliverabilityBatch, FactoryJobOutput};
use super::jobs::{JobResult, Submitted};
use super::prompt_queue_jobs::{ExternalOutcome, QueueJobs};
use crate::ui::factory::app::delivery_evidence::StallExternalEvidence;
use crate::ui::factory::app::{
    DeliverabilityRequest, DeliveryInputs, DeliveryRequest, DirectorRefreshLoad,
    DirectorRefreshRequest, FactoryApp, IdleLivenessMap,
};
use crate::ui::factory::director::{
    DirectorEvent, DirectorStores, Prompt, WorkerHealthReport, WorkerHealthRequest,
};

/// How many times a delivery read that came back without fresh data is
/// retried before its events are delivered as detected (review round 2, M3).
/// A batch gets one read plus this many retries: four reads in all
/// (cas-3b16, review round 3, L-c).
const MAX_DELIVERY_READ_RETRIES: u32 = 3;

/// A gate a test holds every background read on, so a "slow" read finishes
/// exactly when the test opens it instead of after a sleep (cas-3b16, review
/// round 3, L-d).
#[cfg(test)]
#[derive(Default)]
pub(crate) struct ReadGate {
    open: std::sync::Mutex<bool>,
    opened: std::sync::Condvar,
}

#[cfg(test)]
impl ReadGate {
    pub(crate) fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }

    fn wait(&self) {
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.opened.wait(open).unwrap();
        }
    }
}

/// What a background read does before its real work: nothing in
/// production; in tests, sleep a fixed delay, wait for a [`ReadGate`], or
/// die (a job that never reports).
#[derive(Clone, Default)]
struct ReadHook {
    delay: Duration,
    #[cfg(test)]
    gate: Option<Arc<ReadGate>>,
    #[cfg(test)]
    panics: bool,
    /// Only the idle-worker liveness job dies (cas-4491f).
    #[cfg(test)]
    liveness_panics: bool,
    #[cfg(test)]
    process_table: Option<fn() -> Box<dyn crate::cli::factory::wedged::ProcessTable>>,
}

/// The [`ReadHook::run`] name of the idle-worker liveness job.
const IDLE_LIVENESS_JOB: &str = "idle-worker liveness";

impl ReadHook {
    fn run(&self, what: &str) {
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        #[cfg(test)]
        {
            if let Some(gate) = &self.gate {
                gate.wait();
            }
            assert!(!self.panics, "test: the {what} died");
            assert!(
                !(self.liveness_panics && what == IDLE_LIVENESS_JOB),
                "test: the {what} died"
            );
        }
        let _ = what;
    }
}

/// A job whose result the loop is waiting for.
struct Slot<T> {
    /// The generation in flight, and when it was submitted.
    in_flight: Option<(u64, Instant)>,
    ready: Option<T>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            in_flight: None,
            ready: None,
        }
    }
}

impl<T> Slot<T> {
    fn busy(&self) -> bool {
        self.in_flight.is_some() || self.ready.is_some()
    }

    fn start(&mut self, generation: u64) {
        self.in_flight = Some((generation, Instant::now()));
    }

    /// Whether `generation` is the one in flight.
    fn owns(&self, generation: u64) -> bool {
        self.in_flight.is_some_and(|(owned, _)| owned == generation)
    }

    /// Accept the result of `generation`; a late result is ignored.
    fn finish(&mut self, generation: u64, value: T) -> bool {
        if !self.owns(generation) {
            return false;
        }
        self.in_flight = None;
        self.ready = Some(value);
        true
    }

    /// Give `generation` up (failed); returns whether it was in flight.
    fn fail(&mut self, generation: u64) -> bool {
        if !self.owns(generation) {
            return false;
        }
        self.in_flight = None;
        true
    }

    /// Give up a job in flight longer than `timeout`.
    fn expire(&mut self, timeout: Duration) -> bool {
        if self
            .in_flight
            .is_some_and(|(_, since)| since.elapsed() >= timeout)
        {
            self.in_flight = None;
            return true;
        }
        false
    }
}

/// Prompts waiting for their fresh re-check, kept so a re-check that never
/// reports still delivers them: uncertainty delivers.
pub(crate) struct PendingDeliverability {
    pub(crate) prompts: Vec<Prompt>,
    pub(crate) events: Vec<DirectorEvent>,
    pub(crate) refresh_started: Instant,
    since: Instant,
}

impl PendingDeliverability {
    /// Deliver every prompt: the re-check never reported.
    fn uncertain(self) -> DeliverabilityBatch {
        DeliverabilityBatch {
            // Review M4: never read, so read on the store worker, never
            // delivered without its stale check.
            assignments: vec![
                crate::ui::factory::app::AssignmentRead::Unread;
                self.prompts.len()
            ],
            prompts: self.prompts.into_iter().map(|prompt| (prompt, true)).collect(),
            events: self.events,
            refresh_started: self.refresh_started,
        }
    }
}

/// A health probe the loop is waiting for.
struct HealthInFlight {
    probe: (String, String),
    generation: u64,
    since: Instant,
}

/// Background refresh jobs for one daemon loop. The jobs run on the daemon's
/// [`QueueJobs`] runtime, passed to each call.
pub(crate) struct DirectorRefresh {
    generation: u64,
    director: Slot<DirectorRefreshLoad>,
    delivery: Slot<(Vec<DirectorEvent>, DeliveryInputs)>,
    /// The idle-worker liveness observed beside the delivery read in flight,
    /// on the blocking lane, under the read's generation (cas-3b16).
    idle_liveness: Slot<IdleLivenessMap>,
    /// The events riding on the delivery read in flight, re-queued if the
    /// read never reports.
    delivery_events: Vec<DirectorEvent>,
    /// Events detected while a delivery read was in flight; revalidated by
    /// the next one, in order.
    delivery_backlog: Vec<DirectorEvent>,
    deliverability_pending: HashMap<u64, PendingDeliverability>,
    deliverability_ready: Vec<DeliverabilityBatch>,
    merge_alert_inbox_ready: Vec<super::delivery::CompletedMergeAlert>,
    /// Health probes in flight by job key.
    health_in_flight: HashMap<String, HealthInFlight>,
    health_ready: Vec<WorkerHealthReport>,
    /// Probes that will not report (refused, dropped, panicked or timed out).
    health_failed: Vec<(String, String)>,
    ci_status: Slot<StallExternalEvidence>,
    /// Consecutive delivery reads that came back without fresh data.
    failed_delivery_reads: u32,
    /// A snapshot was asked for while one, or a delivery read and its
    /// liveness job, was in flight. It runs as soon as they finish; several
    /// such requests coalesce into one (cas-4491f, C2b review L4).
    refresh_requested: bool,
    /// Test hook: give up in-flight jobs after this instead of
    /// [`factory_jobs::in_flight_timeout`].
    #[cfg(test)]
    timeout_override: Option<Duration>,
    /// Test hook run inside every background read.
    hook: ReadHook,
    /// Test hook: every delivery read comes back without fresh data; counts
    /// the reads.
    #[cfg(test)]
    stale_delivery_reads: Option<Arc<std::sync::atomic::AtomicUsize>>,
    /// Late results ignored because their job was already given up (tests).
    #[cfg(test)]
    pub(crate) late_results_ignored: usize,
}

impl DirectorRefresh {
    pub(crate) fn new() -> Self {
        Self {
            generation: 0,
            director: Slot::default(),
            delivery: Slot::default(),
            idle_liveness: Slot::default(),
            delivery_events: Vec::new(),
            delivery_backlog: Vec::new(),
            deliverability_pending: HashMap::new(),
            deliverability_ready: Vec::new(),
            merge_alert_inbox_ready: Vec::new(),
            health_in_flight: HashMap::new(),
            health_ready: Vec::new(),
            health_failed: Vec::new(),
            ci_status: Slot::default(),
            failed_delivery_reads: 0,
            refresh_requested: false,
            #[cfg(test)]
            timeout_override: None,
            hook: ReadHook::default(),
            #[cfg(test)]
            stale_delivery_reads: None,
            #[cfg(test)]
            late_results_ignored: 0,
        }
    }

    /// Model a read job that dies without a result.
    #[cfg(test)]
    pub(crate) fn with_panicking_reads(mut self) -> Self {
        self.hook.panics = true;
        self
    }

    #[cfg(test)]
    pub(crate) fn set_panicking_reads(&mut self, panics: bool) {
        self.hook.panics = panics;
    }

    /// Model an idle-worker liveness job that dies without a result.
    #[cfg(test)]
    pub(crate) fn with_failing_liveness(mut self) -> Self {
        self.hook.liveness_panics = true;
        self
    }

    /// Supply the OS process table on the real background snapshot lane.
    #[cfg(test)]
    pub(crate) fn with_process_table(
        mut self,
        table: fn() -> Box<dyn crate::cli::factory::wedged::ProcessTable>,
    ) -> Self {
        self.hook.process_table = Some(table);
        self
    }

    /// Model a slow store: every read sleeps `delay` first.
    #[cfg(test)]
    pub(crate) fn with_read_delay(mut self, delay: Duration) -> Self {
        self.hook.delay = delay;
        self
    }

    /// Model a slow store deterministically: every read waits for `gate`.
    #[cfg(test)]
    pub(crate) fn with_read_gate(mut self, gate: Arc<ReadGate>) -> Self {
        self.hook.gate = Some(gate);
        self
    }

    /// Model a store whose delivery reads keep failing; counts them.
    #[cfg(test)]
    pub(crate) fn with_stale_delivery_reads(
        mut self,
        reads: Arc<std::sync::atomic::AtomicUsize>,
    ) -> Self {
        self.stale_delivery_reads = Some(reads);
        self
    }

    /// A late result for a job already given up was ignored.
    fn ignored_late(&mut self) {
        #[cfg(test)]
        {
            self.late_results_ignored += 1;
        }
    }

    fn next_generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// Collect finished jobs without waiting, and give up jobs that outlived
    /// their timeout. The runtime is shared, so this applies the queue's
    /// completions too and takes only external ones.
    fn poll(&mut self, jobs: &mut QueueJobs) {
        jobs.apply_completions();
        // Each owner consumes only its own kinds on the shared runtime.
        for result in jobs.take_external_completions_where(super::factory_jobs::is_factory_kind) {
            self.accept(result);
        }
        self.expire();
    }

    fn accept(&mut self, result: JobResult<ExternalOutcome>) {
        let JobResult {
            kind,
            key,
            generation,
            outcome,
            ..
        } = result;
        let output = match outcome {
            // A full lane dropped the job before it ran (only refresh jobs):
            // free its slot and retry next pass. A coalesced rerun of the
            // same key may still report Done later; the generation check
            // below ignores it unless it is the one in flight.
            Ok(ExternalOutcome::Dropped) => {
                tracing::debug!(kind, key = %key, generation, "factory refresh job dropped by a full lane");
                self.fail(&key, generation);
                return;
            }
            Ok(ExternalOutcome::Done(value)) => match value.downcast::<FactoryJobOutput>() {
                Ok(output) => *output,
                Err(_) => {
                    tracing::debug!(kind, key = %key, "another child's external job result ignored");
                    return;
                }
            },
            Err(panicked) => {
                tracing::error!(kind, key = %key, message = %panicked.message, "factory refresh job panicked");
                self.fail(&key, generation);
                return;
            }
        };
        match output {
            FactoryJobOutput::DirectorSnapshot(load) => {
                if !self.director.finish(generation, load) {
                    tracing::debug!(generation, "late director snapshot ignored");
                    self.ignored_late();
                }
            }
            FactoryJobOutput::Delivery(events, inputs) => {
                if self.delivery.finish(generation, (events, inputs)) {
                    self.delivery_events.clear();
                } else {
                    tracing::debug!(generation, "late delivery read ignored; its events were re-queued");
                    self.ignored_late();
                }
            }
            FactoryJobOutput::IdleLiveness(liveness) => {
                if !self.idle_liveness.finish(generation, liveness) {
                    tracing::debug!(generation, "late idle-worker liveness ignored");
                    self.ignored_late();
                }
            }
            FactoryJobOutput::Deliverability(batch) => {
                if self.deliverability_pending.remove(&generation).is_some() {
                    self.deliverability_ready.push(batch);
                } else {
                    tracing::debug!(generation, "late prompt re-check ignored; its prompts were delivered");
                    self.ignored_late();
                }
            }
            FactoryJobOutput::WorkerHealth(report) => {
                if self
                    .health_in_flight
                    .get(&key)
                    .is_some_and(|probe| probe.generation == generation)
                {
                    self.health_in_flight.remove(&key);
                }
                // A late verdict is still a verdict; the detector caches it.
                self.health_ready.push(report);
            }
            FactoryJobOutput::CiStatus(evidence) => {
                if !self.ci_status.finish(generation, evidence) {
                    tracing::debug!(generation, "late CI status ignored");
                }
            }
            FactoryJobOutput::MergeAlertInbox(completed) => {
                self.merge_alert_inbox_ready.extend(completed);
            }
            FactoryJobOutput::ReminderScan => {}
        }
    }

    /// A job that will not report: it panicked, or the runtime dropped it.
    /// Free its slot so the next pass retries (review round 1, M2).
    fn fail(&mut self, key: &str, generation: u64) {
        match key {
            factory_jobs::DIRECTOR_SNAPSHOT_KEY => {
                self.director.fail(generation);
            }
            factory_jobs::DELIVERY_READ_KEY => {
                if self.delivery.fail(generation) {
                    self.idle_liveness = Slot::default();
                    self.requeue_delivery_events();
                }
            }
            // The read applies without it: those workers stay unobserved.
            factory_jobs::IDLE_LIVENESS_KEY => {
                self.idle_liveness.fail(generation);
            }
            factory_jobs::DELIVERABILITY_KEY => {
                if let Some(pending) = self.deliverability_pending.remove(&generation) {
                    self.deliverability_ready.push(pending.uncertain());
                }
            }
            factory_jobs::CI_STATUS_KEY => {
                self.ci_status.fail(generation);
            }
            factory_jobs::REMINDER_SCAN_KEY => {}
            _ => {
                if self
                    .health_in_flight
                    .get(key)
                    .is_some_and(|probe| probe.generation == generation)
                    && let Some(probe) = self.health_in_flight.remove(key)
                {
                    self.health_failed.push(probe.probe);
                }
            }
        }
    }

    /// The delivery read never reported: its events wait for the next one,
    /// so a lost read cannot lose them.
    fn requeue_delivery_events(&mut self) {
        let mut events = std::mem::take(&mut self.delivery_events);
        events.append(&mut self.delivery_backlog);
        self.delivery_backlog = events;
    }

    #[cfg(test)]
    fn timeout(&self, budget: Duration) -> Duration {
        self.timeout_override
            .unwrap_or_else(|| factory_jobs::in_flight_timeout(budget))
    }

    #[cfg(not(test))]
    fn timeout(&self, budget: Duration) -> Duration {
        factory_jobs::in_flight_timeout(budget)
    }

    /// Model a short in-flight timeout.
    #[cfg(test)]
    pub(crate) fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout_override = Some(timeout);
        self
    }

    /// A delivery read came back without fresh data (review round 2, M3).
    /// Its events wait for another fresh read, up to
    /// [`MAX_DELIVERY_READ_RETRIES`] retries in a row (so four reads in
    /// all); when the last retry fails too they are handed back to be
    /// delivered as detected, so a store that keeps failing cannot hold
    /// supervisor notices forever.
    pub(crate) fn retry_stale_delivery(
        &mut self,
        events: Vec<DirectorEvent>,
    ) -> Option<Vec<DirectorEvent>> {
        if self.failed_delivery_reads < MAX_DELIVERY_READ_RETRIES {
            self.failed_delivery_reads += 1;
            tracing::warn!(
                attempt = self.failed_delivery_reads,
                "delivery read failed; its events wait for another fresh read"
            );
            let mut retry = events;
            retry.append(&mut self.delivery_backlog);
            self.delivery_backlog = retry;
            return None;
        }
        self.failed_delivery_reads = 0;
        Some(events)
    }

    /// A delivery read came back fresh.
    pub(crate) fn note_fresh_delivery_read(&mut self) {
        self.failed_delivery_reads = 0;
    }

    /// Give up every job that outlived its timeout, logging it.
    fn expire(&mut self) {
        let snapshot_timeout = self.timeout(factory_jobs::DIRECTOR_SNAPSHOT_BUDGET);
        let revalidate_timeout = self.timeout(factory_jobs::REVALIDATE_FRESH_BUDGET);
        let ci_timeout = self.timeout(factory_jobs::CI_STATUS_BUDGET);
        let health_timeout = self.timeout(factory_jobs::WORKER_HEALTH_PROBE_BUDGET);
        if self
            .director
            .expire(snapshot_timeout)
        {
            tracing::warn!("director snapshot never reported; retrying next pass");
        }
        if self
            .delivery
            .expire(revalidate_timeout)
        {
            tracing::warn!("delivery read never reported; its events wait for the next one");
            self.idle_liveness = Slot::default();
            self.requeue_delivery_events();
        }
        if self.idle_liveness.expire(health_timeout) {
            tracing::warn!("idle-worker liveness never reported; delivering without it");
        }
        if self.ci_status.expire(ci_timeout) {
            tracing::warn!("CI status job never reported; retrying next pass");
        }
        let deliverability_timeout = revalidate_timeout;
        let mut expired: Vec<u64> = self
            .deliverability_pending
            .iter()
            .filter(|(_, pending)| pending.since.elapsed() >= deliverability_timeout)
            .map(|(generation, _)| *generation)
            .collect();
        // Oldest first, so prompts keep their order (cas-3b16, L7).
        expired.sort_unstable();
        for generation in expired {
            if let Some(pending) = self.deliverability_pending.remove(&generation) {
                tracing::warn!("prompt re-check never reported; delivering its prompts");
                self.deliverability_ready.push(pending.uncertain());
            }
        }
        let expired: Vec<String> = self
            .health_in_flight
            .iter()
            .filter(|(_, probe)| probe.since.elapsed() >= health_timeout)
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            if let Some(probe) = self.health_in_flight.remove(&key) {
                tracing::warn!(key = %key, "worker health probe never reported; asking again");
                self.health_failed.push(probe.probe);
            }
        }
    }

    /// Whether a finished job is waiting to be applied, or events wait for a
    /// delivery read the loop can start now (review round 2, M2).
    pub(crate) fn has_finished(&mut self, jobs: &mut QueueJobs) -> bool {
        self.poll(jobs);
        (!self.delivery_backlog.is_empty() && !self.delivery_busy())
            || (self.refresh_requested && !self.director.busy() && !self.delivery_busy())
            || self.director.ready.is_some()
            || self.delivery_ready()
            || self.ci_status.ready.is_some()
            || !self.deliverability_ready.is_empty()
            || !self.health_ready.is_empty()
            || !self.health_failed.is_empty()
    }

    /// Start a director snapshot unless one is already in flight. A delivery
    /// read in flight (with its liveness job) also defers it, so its events
    /// are delivered before the next detection. A deferred request is never
    /// dropped: it runs once they finish ([`Self::refresh_requested`]).
    pub(crate) fn start_director_read(&mut self, app: &FactoryApp, jobs: &mut QueueJobs) {
        self.poll(jobs);
        if self.director.busy() || self.delivery_busy() {
            self.refresh_requested = true;
            return;
        }
        self.refresh_requested = false;
        let request: DirectorRefreshRequest = app.director_refresh_request();
        let hook = self.hook.clone();
        let generation = self.next_generation();
        // Marked first: an inline runtime finishes the job inside submit.
        self.director.start(generation);
        let submitted = jobs.submit_external_read(
            factory_jobs::director_snapshot_spec(generation),
            move |stores: Option<&DirectorStores>| {
                hook.run("snapshot read");
                #[cfg(test)]
                if let Some(table) = hook.process_table {
                    return Box::new(FactoryJobOutput::DirectorSnapshot(
                        request.read_with_process_table(stores, table().as_ref()),
                    ));
                }
                Box::new(FactoryJobOutput::DirectorSnapshot(request.read(stores)))
            },
        );
        if !self.queued_or_coalesced(submitted) {
            tracing::warn!("director snapshot refused by the job runtime");
            self.director.fail(generation);
        }
        self.poll(jobs);
    }

    /// Whether a refresh job submitted with a new generation will report
    /// under that generation (review round 2, M1):
    /// - `Queued`: it runs.
    /// - `Coalesced`: the key is still busy (often with a job given up after
    ///   its timeout). The runtime keeps this submit as the key's rerun and
    ///   runs it next, so its generation stays in flight; the busy job's
    ///   result has an older generation and is ignored. A rerun it replaced
    ///   never reports, which is fine because only the newest is tracked.
    /// - `QueuedDroppedOldest`: it runs; the dropped job (its own generation)
    ///   is failed, so if it was ours its slot frees. When the dropped key
    ///   still has a queued rerun, that rerun carries a newer generation and
    ///   the generation check leaves it in flight.
    ///
    /// Anything else was refused.
    fn queued_or_coalesced(&mut self, submitted: Submitted) -> bool {
        match submitted {
            Submitted::Queued | Submitted::Coalesced { .. } | Submitted::QueuedBehindKey => true,
            Submitted::QueuedDroppedOldest {
                dropped_key,
                dropped_generation,
                ..
            } => {
                self.fail(&dropped_key, dropped_generation);
                true
            }
            refused => {
                tracing::warn!(?refused, "factory refresh job refused by the job runtime");
                false
            }
        }
    }

    /// A snapshot asked for while busy, still waiting to start.
    pub(crate) fn refresh_requested(&self) -> bool {
        self.refresh_requested
    }

    pub(crate) fn take_director_read(&mut self, jobs: &mut QueueJobs) -> Option<DirectorRefreshLoad> {
        self.poll(jobs);
        self.director.ready.take()
    }

    /// Start the delivery read for `events`. Events detected while a read is
    /// in flight wait for the next one. Hands the events back only when no
    /// job can run (the runtime refused it), so the caller can deliver them
    /// another way.
    pub(crate) fn start_delivery_read(
        &mut self,
        app: &FactoryApp,
        jobs: &mut QueueJobs,
        events: Vec<DirectorEvent>,
    ) -> Result<(), Vec<DirectorEvent>> {
        self.poll(jobs);
        if self.delivery_busy() {
            self.delivery_backlog.extend(events);
            return Ok(());
        }
        let mut request: DeliveryRequest = app.delivery_request(&events);
        let external = request.needs_external_lane();
        // cas-3b16: the liveness scan (agent store, transcripts, `/proc`)
        // runs beside the read on the blocking lane, not on the read lane.
        let liveness = request.take_idle_liveness();
        let hook = self.hook.clone();
        #[cfg(test)]
        let stale_reads = self.stale_delivery_reads.clone();
        let generation = self.next_generation();
        self.delivery_events = events.clone();
        let carried = events;
        self.delivery.start(generation);
        let work = move |stores: Option<&DirectorStores>| -> Box<dyn std::any::Any + Send> {
                hook.run("delivery read");
                #[cfg(test)]
                if let Some(reads) = stale_reads {
                    reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    return Box::new(FactoryJobOutput::Delivery(
                        carried,
                        DeliveryInputs::failed_for_test(),
                    ));
                }
                let inputs = request.read(stores);
                Box::new(FactoryJobOutput::Delivery(carried, inputs))
            };
        let mut spec = factory_jobs::delivery_read_spec(generation);
        let submitted = if external {
            spec.lane = super::jobs::Lane::External;
            spec.budget = factory_jobs::CI_STATUS_BUDGET;
            jobs.submit_external(spec, move || work(None))
        } else {
            jobs.submit_external_read(spec, work)
        };
        match submitted {
            Submitted::Queued | Submitted::QueuedBehindKey => {
                if let Some(liveness) = liveness {
                    self.start_idle_liveness(jobs, generation, liveness);
                }
                self.poll(jobs);
                Ok(())
            }
            refused => {
                tracing::warn!(?refused, "delivery read refused by the job runtime");
                self.delivery.fail(generation);
                Err(std::mem::take(&mut self.delivery_events))
            }
        }
    }

    /// Observe the idle workers' liveness on the blocking lane, under the
    /// delivery read's generation. If the runtime refuses it, the read is
    /// applied without it (those workers are unobserved, so delivered).
    fn start_idle_liveness(
        &mut self,
        jobs: &mut QueueJobs,
        generation: u64,
        request: crate::ui::factory::app::IdleLivenessRequest,
    ) {
        let hook = self.hook.clone();
        self.idle_liveness.start(generation);
        let submitted = jobs.submit_external(factory_jobs::idle_liveness_spec(generation), move || {
            hook.run(IDLE_LIVENESS_JOB);
            Box::new(FactoryJobOutput::IdleLiveness(request.read()))
        });
        if !matches!(submitted, Submitted::Queued | Submitted::QueuedBehindKey) {
            tracing::warn!(?submitted, "idle-worker liveness refused by the job runtime");
            self.idle_liveness.fail(generation);
        }
    }

    /// A delivery read (or the liveness observed beside it) is in flight or
    /// waiting to be applied.
    fn delivery_busy(&self) -> bool {
        self.delivery.busy() || self.idle_liveness.busy()
    }

    /// The delivery read reported, and so did its liveness job (or that
    /// failed).
    fn delivery_ready(&self) -> bool {
        self.delivery.ready.is_some() && self.idle_liveness.in_flight.is_none()
    }

    /// Prompt re-checks are still waiting; a later batch must wait behind
    /// them to keep prompt order (cas-3b16, C2 review round 1, L7).
    pub(crate) fn has_pending_deliverability(&self) -> bool {
        !self.deliverability_pending.is_empty() || !self.deliverability_ready.is_empty()
    }

    /// Start the delivery read for events that waited behind the last one.
    pub(crate) fn start_delivery_backlog(
        &mut self,
        app: &FactoryApp,
        jobs: &mut QueueJobs,
    ) -> Result<(), Vec<DirectorEvent>> {
        if self.delivery_busy() || self.delivery_backlog.is_empty() {
            return Ok(());
        }
        let events = std::mem::take(&mut self.delivery_backlog);
        self.start_delivery_read(app, jobs, events)
    }

    pub(crate) fn take_delivery_read(
        &mut self,
        jobs: &mut QueueJobs,
    ) -> Option<(Vec<DirectorEvent>, DeliveryInputs)> {
        self.poll(jobs);
        if !self.delivery_ready() {
            return None;
        }
        let (events, mut inputs) = self.delivery.ready.take()?;
        if let Some(liveness) = self.idle_liveness.ready.take() {
            inputs.set_idle_liveness(liveness);
        }
        Some((events, inputs))
    }

    /// Resolve the git half of the supervisor-stall evidence as a `CiStatus`
    /// job on the external lane, unless one is in flight (review M1).
    pub(crate) fn start_ci_status(&mut self, app: &FactoryApp, jobs: &mut QueueJobs) {
        if self.ci_status.busy() {
            return;
        }
        let Some(request) = app.ci_status_request() else {
            return;
        };
        let generation = self.next_generation();
        self.ci_status.start(generation);
        let submitted = jobs.submit_external(factory_jobs::ci_status_spec(generation), move || {
            Box::new(FactoryJobOutput::CiStatus(request.run()))
        });
        if !self.queued_or_coalesced(submitted) {
            tracing::warn!("CI status job refused by the job runtime");
            self.ci_status.fail(generation);
        }
    }

    /// A finished `CiStatus` job's evidence.
    pub(crate) fn take_ci_status(&mut self, jobs: &mut QueueJobs) -> Option<StallExternalEvidence> {
        self.poll(jobs);
        self.ci_status.ready.take()
    }

    /// Re-check `prompts` against a fresh read off the loop
    /// (`RevalidateFresh`). The batch comes back from
    /// [`Self::take_deliverability`], in order. Hands the prompts back when
    /// no job can run.
    pub(crate) fn start_deliverability_check(
        &mut self,
        app: &FactoryApp,
        jobs: &mut QueueJobs,
        mut prompts: Vec<Prompt>,
        events: Vec<DirectorEvent>,
        refresh_started: Instant,
    ) -> Result<(), Vec<Prompt>> {
        let request: DeliverabilityRequest = app.deliverability_request();
        let generation = self.next_generation();
        let hook = self.hook.clone();
        // Recorded first: an inline runtime finishes the job inside submit.
        self.deliverability_pending.insert(
            generation,
            PendingDeliverability {
                prompts: prompts.clone(),
                events: events.clone(),
                refresh_started,
                since: Instant::now(),
            },
        );
        let submitted = jobs.submit_external_read(
            factory_jobs::deliverability_spec(generation),
            move |stores: Option<&DirectorStores>| {
                hook.run("prompt re-check");
                let verdicts = request.read(stores, &mut prompts);
                // cas-4491f: assignment wakes read their task here too.
                let assignments = request.read_assignments(stores, &prompts);
                Box::new(FactoryJobOutput::Deliverability(DeliverabilityBatch {
                    assignments,
                    prompts: prompts.into_iter().zip(verdicts).collect(),
                    events,
                    refresh_started,
                }))
            },
        );
        match submitted {
            Submitted::Queued | Submitted::QueuedBehindKey => Ok(()),
            refused => {
                tracing::warn!(?refused, "prompt re-check refused by the job runtime");
                Err(self
                    .deliverability_pending
                    .remove(&generation)
                    .map(|pending| pending.prompts)
                    .unwrap_or_default())
            }
        }
    }

    /// Re-checked prompt batches, in the order they finished.
    pub(crate) fn take_deliverability(&mut self, jobs: &mut QueueJobs) -> Vec<DeliverabilityBatch> {
        self.poll(jobs);
        std::mem::take(&mut self.deliverability_ready)
    }

    pub(crate) fn take_merge_alert_inbox(
        &mut self,
        jobs: &mut QueueJobs,
    ) -> Vec<super::delivery::CompletedMergeAlert> {
        self.poll(jobs);
        std::mem::take(&mut self.merge_alert_inbox_ready)
    }

    /// Shutdown (review M4): wait up to `timeout` for prompt re-checks still
    /// in flight. Returns the finished batches and the re-checks that never
    /// reported, which the caller re-checks itself before injecting, as the
    /// inject loop did before cas-223f. No prompt is dropped. Waits on the
    /// runtime's timer, never by blocking the loop's thread (cas-3b16, C2
    /// review round 2, L1).
    pub(crate) async fn drain_deliverability(
        &mut self,
        jobs: &mut QueueJobs,
        timeout: Duration,
    ) -> (Vec<DeliverabilityBatch>, Vec<PendingDeliverability>) {
        let deadline = Instant::now() + timeout;
        loop {
            self.poll(jobs);
            if self.deliverability_pending.is_empty() || Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let mut pending: Vec<(u64, PendingDeliverability)> =
            self.deliverability_pending.drain().collect();
        pending.sort_by_key(|(generation, _)| *generation);
        (
            std::mem::take(&mut self.deliverability_ready),
            pending.into_iter().map(|(_, pending)| pending).collect(),
        )
    }

    /// Run `requests` as `WorkerHealthProbe` jobs. Returns the probes that
    /// will not report, so the detector can ask again.
    pub(crate) fn submit_health_probes(
        &mut self,
        app: &FactoryApp,
        jobs: &mut QueueJobs,
        requests: Vec<WorkerHealthRequest>,
    ) -> Vec<(String, String)> {
        let cas_root = app.cas_dir().to_path_buf();
        let mut refused = Vec::new();
        for request in requests {
            let generation = self.next_generation();
            let spec = factory_jobs::worker_health_spec(&request.worker, &request.task, generation);
            let key = spec.key.clone();
            let probe = (request.worker.clone(), request.task.clone());
            let cas_root = cas_root.clone();
            // Recorded first: an inline runtime finishes the job inside submit.
            self.health_in_flight.insert(
                key.clone(),
                HealthInFlight {
                    probe: probe.clone(),
                    generation,
                    since: Instant::now(),
                },
            );
            let submitted = jobs.submit_external(spec, move || {
                Box::new(FactoryJobOutput::WorkerHealth(
                    crate::ui::factory::director::run_worker_health_probe(Some(&cas_root), &request),
                ))
            });
            if !self.queued_or_coalesced(submitted) {
                self.health_in_flight.remove(&key);
                refused.push(probe);
            }
        }
        refused.extend(std::mem::take(&mut self.health_failed));
        refused
    }

    /// Finished health probes, and probes that will not report.
    pub(crate) fn take_health_reports(
        &mut self,
        jobs: &mut QueueJobs,
    ) -> (Vec<WorkerHealthReport>, Vec<(String, String)>) {
        self.poll(jobs);
        (
            std::mem::take(&mut self.health_ready),
            std::mem::take(&mut self.health_failed),
        )
    }
}

#[cfg(test)]
mod slot_tests {
    use super::*;

    /// Review round 1, M2: a job given up after its timeout frees the slot,
    /// and its late result is ignored rather than applied over a newer one.
    #[test]
    fn c2_timed_out_job_frees_its_slot_and_its_late_result_is_ignored() {
        let mut slot: Slot<&'static str> = Slot::default();
        slot.start(1);
        assert!(slot.busy());
        std::thread::sleep(Duration::from_millis(2));
        assert!(slot.expire(Duration::from_millis(1)));
        assert!(!slot.busy(), "a timed-out job frees its slot");
        slot.start(2);
        assert!(!slot.finish(1, "late"), "a late result is ignored");
        assert!(slot.fail(2));
        assert!(!slot.busy());
        slot.start(3);
        assert!(slot.finish(3, "fresh"));
        assert_eq!(slot.ready.take(), Some("fresh"));
    }
}

#[cfg(test)]
#[path = "director_refresh_tests.rs"]
mod tests;
