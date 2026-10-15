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

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::factory_jobs::{self, DeliverabilityBatch, FactoryJobOutput};
use super::jobs::{JobResult, Submitted};
use super::prompt_queue_jobs::QueueJobs;
use crate::ui::factory::app::delivery_evidence::GitEvidenceMap;
use crate::ui::factory::app::{
    DeliverabilityRequest, DeliveryInputs, DeliveryRequest, DirectorRefreshLoad,
    DirectorRefreshRequest, FactoryApp,
};
use crate::ui::factory::director::{
    DirectorEvent, DirectorStores, Prompt, WorkerHealthReport, WorkerHealthRequest,
};

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
            .is_some_and(|(_, since)| since.elapsed() > timeout)
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
    /// The events riding on the delivery read in flight, re-queued if the
    /// read never reports.
    delivery_events: Vec<DirectorEvent>,
    /// Events detected while a delivery read was in flight; revalidated by
    /// the next one, in order.
    delivery_backlog: Vec<DirectorEvent>,
    deliverability_pending: HashMap<u64, PendingDeliverability>,
    deliverability_ready: Vec<DeliverabilityBatch>,
    /// Health probes in flight by job key.
    health_in_flight: HashMap<String, HealthInFlight>,
    health_ready: Vec<WorkerHealthReport>,
    /// Probes that will not report (refused, dropped, panicked or timed out).
    health_failed: Vec<(String, String)>,
    ci_status: Slot<GitEvidenceMap>,
    /// Test hook: sleep this long inside every background read.
    read_delay: Duration,
}

impl DirectorRefresh {
    pub(crate) fn new() -> Self {
        Self {
            generation: 0,
            director: Slot::default(),
            delivery: Slot::default(),
            delivery_events: Vec::new(),
            delivery_backlog: Vec::new(),
            deliverability_pending: HashMap::new(),
            deliverability_ready: Vec::new(),
            health_in_flight: HashMap::new(),
            health_ready: Vec::new(),
            health_failed: Vec::new(),
            ci_status: Slot::default(),
            read_delay: Duration::ZERO,
        }
    }

    /// Model a slow store: every read sleeps `delay` first.
    #[cfg(test)]
    pub(crate) fn with_read_delay(mut self, delay: Duration) -> Self {
        self.read_delay = delay;
        self
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
        for result in jobs.take_external_completions() {
            self.accept(result);
        }
        self.expire();
    }

    fn accept(&mut self, result: JobResult<Box<dyn std::any::Any + Send>>) {
        let JobResult {
            kind,
            key,
            generation,
            outcome,
            ..
        } = result;
        let output = match outcome {
            Ok(value) => match value.downcast::<FactoryJobOutput>() {
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
                }
            }
            FactoryJobOutput::Delivery(events, inputs) => {
                if self.delivery.finish(generation, (events, inputs)) {
                    self.delivery_events.clear();
                } else {
                    tracing::debug!(generation, "late delivery read ignored; its events were re-queued");
                }
            }
            FactoryJobOutput::Deliverability(batch) => {
                if self.deliverability_pending.remove(&generation).is_some() {
                    self.deliverability_ready.push(batch);
                } else {
                    tracing::debug!(generation, "late prompt re-check ignored; its prompts were delivered");
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
                    self.requeue_delivery_events();
                }
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

    /// Give up every job that outlived its timeout, logging it.
    fn expire(&mut self) {
        let timeout = factory_jobs::in_flight_timeout;
        if self
            .director
            .expire(timeout(factory_jobs::DIRECTOR_SNAPSHOT_BUDGET))
        {
            tracing::warn!("director snapshot never reported; retrying next pass");
        }
        if self
            .delivery
            .expire(timeout(factory_jobs::REVALIDATE_FRESH_BUDGET))
        {
            tracing::warn!("delivery read never reported; its events wait for the next one");
            self.requeue_delivery_events();
        }
        if self.ci_status.expire(timeout(factory_jobs::CI_STATUS_BUDGET)) {
            tracing::warn!("CI status job never reported; retrying next pass");
        }
        let deliverability_timeout = timeout(factory_jobs::REVALIDATE_FRESH_BUDGET);
        let expired: Vec<u64> = self
            .deliverability_pending
            .iter()
            .filter(|(_, pending)| pending.since.elapsed() > deliverability_timeout)
            .map(|(generation, _)| *generation)
            .collect();
        for generation in expired {
            if let Some(pending) = self.deliverability_pending.remove(&generation) {
                tracing::warn!("prompt re-check never reported; delivering its prompts");
                self.deliverability_ready.push(pending.uncertain());
            }
        }
        let health_timeout = timeout(factory_jobs::WORKER_HEALTH_PROBE_BUDGET);
        let expired: Vec<String> = self
            .health_in_flight
            .iter()
            .filter(|(_, probe)| probe.since.elapsed() > health_timeout)
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            if let Some(probe) = self.health_in_flight.remove(&key) {
                tracing::warn!(key = %key, "worker health probe never reported; asking again");
                self.health_failed.push(probe.probe);
            }
        }
    }

    /// Whether a finished job is waiting to be applied.
    pub(crate) fn has_finished(&mut self, jobs: &mut QueueJobs) -> bool {
        self.poll(jobs);
        self.director.ready.is_some()
            || self.delivery.ready.is_some()
            || self.ci_status.ready.is_some()
            || !self.deliverability_ready.is_empty()
            || !self.health_ready.is_empty()
            || !self.health_failed.is_empty()
    }

    /// Start a director snapshot unless one is already in flight. A delivery
    /// read in flight also defers it, so its events are delivered before the
    /// next detection.
    pub(crate) fn start_director_read(&mut self, app: &FactoryApp, jobs: &mut QueueJobs) {
        self.poll(jobs);
        if self.director.busy() || self.delivery.busy() {
            return;
        }
        let request: DirectorRefreshRequest = app.director_refresh_request();
        let delay = self.read_delay;
        let generation = self.next_generation();
        // Marked first: an inline runtime finishes the job inside submit.
        self.director.start(generation);
        let submitted = jobs.submit_external_read(
            factory_jobs::director_snapshot_spec(generation),
            move |stores: Option<&DirectorStores>| {
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                Box::new(FactoryJobOutput::DirectorSnapshot(request.read(stores)))
            },
        );
        if !matches!(
            submitted,
            Submitted::Queued | Submitted::QueuedDroppedOldest { .. }
        ) {
            tracing::warn!(?submitted, "director snapshot refused by the job runtime");
            self.director.fail(generation);
        }
        self.poll(jobs);
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
        if self.delivery.busy() {
            self.delivery_backlog.extend(events);
            return Ok(());
        }
        let request: DeliveryRequest = app.delivery_request(&events);
        let delay = self.read_delay;
        let generation = self.next_generation();
        self.delivery_events = events.clone();
        let carried = events;
        self.delivery.start(generation);
        let submitted = jobs.submit_external_read(
            factory_jobs::delivery_read_spec(generation),
            move |stores: Option<&DirectorStores>| {
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                let inputs = request.read(stores);
                Box::new(FactoryJobOutput::Delivery(carried, inputs))
            },
        );
        match submitted {
            Submitted::Queued | Submitted::QueuedBehindKey => {
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

    /// Start the delivery read for events that waited behind the last one.
    pub(crate) fn start_delivery_backlog(
        &mut self,
        app: &FactoryApp,
        jobs: &mut QueueJobs,
    ) -> Result<(), Vec<DirectorEvent>> {
        if self.delivery.busy() || self.delivery_backlog.is_empty() {
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
        self.delivery.ready.take()
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
        if !matches!(
            submitted,
            Submitted::Queued | Submitted::QueuedDroppedOldest { .. }
        ) {
            tracing::warn!(?submitted, "CI status job refused by the job runtime");
            self.ci_status.fail(generation);
        }
    }

    /// A finished `CiStatus` job's evidence.
    pub(crate) fn take_ci_status(&mut self, jobs: &mut QueueJobs) -> Option<GitEvidenceMap> {
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
        prompts: Vec<Prompt>,
        events: Vec<DirectorEvent>,
        refresh_started: Instant,
    ) -> Result<(), Vec<Prompt>> {
        let request: DeliverabilityRequest = app.deliverability_request();
        let generation = self.next_generation();
        let delay = self.read_delay;
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
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                let verdicts = request.read(stores, &prompts);
                Box::new(FactoryJobOutput::Deliverability(DeliverabilityBatch {
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

    /// Shutdown (review M4): wait up to `timeout` for prompt re-checks still
    /// in flight. Returns the finished batches and the re-checks that never
    /// reported, which the caller re-checks itself before injecting, as the
    /// inject loop did before cas-223f. No prompt is dropped.
    pub(crate) fn drain_deliverability(
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
            std::thread::sleep(Duration::from_millis(5));
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
            let previous = self.health_in_flight.insert(
                key.clone(),
                HealthInFlight {
                    probe: probe.clone(),
                    generation,
                    since: Instant::now(),
                },
            );
            match jobs.submit_external(spec, move || {
                Box::new(FactoryJobOutput::WorkerHealth(
                    crate::ui::factory::director::run_worker_health_probe(Some(&cas_root), &request),
                ))
            }) {
                Submitted::Queued => {}
                Submitted::QueuedDroppedOldest { dropped_key } => {
                    if let Some(dropped) = self.health_in_flight.remove(&dropped_key) {
                        refused.push(dropped.probe);
                    }
                }
                // The key is already running: its report covers this ask.
                Submitted::Coalesced | Submitted::QueuedBehindKey => {
                    if let Some(previous) = previous {
                        self.health_in_flight.insert(key, previous);
                    }
                }
                Submitted::DroppedRefresh | Submitted::Closed | Submitted::LaneMismatch { .. } => {
                    self.health_in_flight.remove(&key);
                    refused.push(probe);
                }
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
#[path = "director_refresh_tests.rs"]
mod tests;
