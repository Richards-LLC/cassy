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
//! They now run as jobs on the jobs runtime ([`super::factory_jobs`]): the
//! snapshot (`DirectorSnapshot`) and the fresh re-reads (`RevalidateFresh`)
//! on the read lane, which keeps its own SQLite connection, and stall
//! confirmation (`WorkerHealthProbe`) on the blocking lane. The loop starts a
//! job when it is due, keeps passing, and applies the finished result on a
//! later pass. Panels are stale by at most one refresh. Delivery keeps its
//! fresh-read-after-detection order: events detected from one read are
//! revalidated against a second read started after detection.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use super::factory_jobs::{self, DeliverabilityBatch, FactoryJobOutput, FactoryJobRuntime};
use super::jobs::{JobResult, Submitted};
use crate::ui::factory::app::{
    DeliverabilityRequest, DeliveryInputs, DeliveryRequest, DirectorRefreshLoad,
    DirectorRefreshRequest, FactoryApp,
};
use crate::ui::factory::director::{
    DirectorEvent, DirectorStores, Prompt, WorkerHealthReport, WorkerHealthRequest,
};

/// A job whose result the loop is waiting for.
struct Slot<T> {
    in_flight: bool,
    ready: Option<T>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            in_flight: false,
            ready: None,
        }
    }
}

impl<T> Slot<T> {
    fn busy(&self) -> bool {
        self.in_flight || self.ready.is_some()
    }
}

/// Prompts waiting for their fresh re-check, kept so a re-check that never
/// reports (it panicked) still delivers them: uncertainty delivers.
struct PendingDeliverability {
    prompts: Vec<Prompt>,
    events: Vec<DirectorEvent>,
    refresh_started: std::time::Instant,
}

/// Background refresh jobs for one daemon loop.
pub(crate) struct DirectorRefresh {
    jobs: Option<Arc<FactoryJobRuntime>>,
    generation: u64,
    director: Slot<DirectorRefreshLoad>,
    delivery: Slot<(Vec<DirectorEvent>, DeliveryInputs)>,
    /// Events detected while a delivery read was in flight; revalidated by
    /// the next one, in order.
    delivery_backlog: Vec<DirectorEvent>,
    deliverability_pending: HashMap<u64, PendingDeliverability>,
    deliverability_ready: Vec<DeliverabilityBatch>,
    /// Health probes in flight by job key.
    health_in_flight: HashMap<String, (String, String)>,
    health_ready: Vec<WorkerHealthReport>,
    /// Probes that will not report (refused, dropped or panicked).
    health_failed: Vec<(String, String)>,
    /// Test hook: sleep this long inside every background read.
    read_delay: Duration,
}

impl DirectorRefresh {
    /// Refresh jobs on a runtime of their own, started on first use.
    pub(crate) fn new() -> Self {
        Self {
            jobs: None,
            generation: 0,
            director: Slot::default(),
            delivery: Slot::default(),
            delivery_backlog: Vec::new(),
            deliverability_pending: HashMap::new(),
            deliverability_ready: Vec::new(),
            health_in_flight: HashMap::new(),
            health_ready: Vec::new(),
            health_failed: Vec::new(),
            read_delay: Duration::ZERO,
        }
    }

    /// Refresh jobs on the daemon's runtime. This refresh drains its
    /// completion queue.
    pub(crate) fn with_jobs(jobs: Arc<FactoryJobRuntime>) -> Self {
        let mut refresh = Self::new();
        refresh.jobs = Some(jobs);
        refresh
    }

    /// Model a slow store: every read sleeps `delay` first.
    #[cfg(test)]
    pub(crate) fn with_read_delay(mut self, delay: Duration) -> Self {
        self.read_delay = delay;
        self
    }

    /// The runtime the jobs run on, once started.
    #[cfg(test)]
    pub(crate) fn jobs(&self) -> Option<&Arc<FactoryJobRuntime>> {
        self.jobs.as_ref()
    }

    fn runtime(&mut self, app: &FactoryApp) -> Arc<FactoryJobRuntime> {
        Arc::clone(
            self.jobs.get_or_insert_with(|| {
                factory_jobs::start_runtime(app.cas_dir(), app.project_scope())
            }),
        )
    }

    fn next_generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// Collect finished jobs without waiting.
    fn poll(&mut self) {
        let Some(jobs) = self.jobs.as_ref() else {
            return;
        };
        for result in jobs.drain_completions() {
            self.accept(result);
        }
    }

    fn accept(&mut self, result: JobResult<FactoryJobOutput>) {
        let JobResult {
            kind,
            key,
            generation,
            outcome,
            ..
        } = result;
        match outcome {
            Ok(FactoryJobOutput::DirectorSnapshot(load)) => {
                self.director.in_flight = false;
                self.director.ready = Some(load);
            }
            Ok(FactoryJobOutput::Delivery(events, inputs)) => {
                self.delivery.in_flight = false;
                self.delivery.ready = Some((events, inputs));
            }
            Ok(FactoryJobOutput::Deliverability(batch)) => {
                self.deliverability_pending.remove(&generation);
                self.deliverability_ready.push(batch);
            }
            Ok(FactoryJobOutput::WorkerHealth(report)) => {
                self.health_in_flight.remove(&key);
                self.health_ready.push(report);
            }
            Ok(FactoryJobOutput::ReminderScan) => {}
            Err(panicked) => {
                tracing::error!(kind, key = %key, message = %panicked.message, "factory refresh job panicked");
                match key.as_str() {
                    factory_jobs::DIRECTOR_SNAPSHOT_KEY => self.director.in_flight = false,
                    // As before cas-223f, a delivery read that died loses its
                    // events; the next detection sees the same state again.
                    factory_jobs::DELIVERY_READ_KEY => self.delivery.in_flight = false,
                    factory_jobs::DELIVERABILITY_KEY => {
                        if let Some(pending) = self.deliverability_pending.remove(&generation) {
                            self.deliverability_ready.push(DeliverabilityBatch {
                                prompts: pending.prompts.into_iter().map(|p| (p, true)).collect(),
                                events: pending.events,
                                refresh_started: pending.refresh_started,
                            });
                        }
                    }
                    _ => {
                        if let Some(probe) = self.health_in_flight.remove(&key) {
                            self.health_failed.push(probe);
                        }
                    }
                }
            }
        }
    }

    /// Whether a finished job is waiting to be applied.
    pub(crate) fn has_finished(&mut self) -> bool {
        self.poll();
        self.director.ready.is_some()
            || self.delivery.ready.is_some()
            || !self.deliverability_ready.is_empty()
            || !self.health_ready.is_empty()
            || !self.health_failed.is_empty()
    }

    /// Start a director snapshot unless one is already in flight. A delivery
    /// read in flight also defers it, so its events are delivered before the
    /// next detection.
    pub(crate) fn start_director_read(&mut self, app: &FactoryApp) {
        self.poll();
        if self.director.busy() || self.delivery.busy() {
            return;
        }
        let request: DirectorRefreshRequest = app.director_refresh_request();
        let delay = self.read_delay;
        let generation = self.next_generation();
        let submitted = self.runtime(app).submit_read(
            factory_jobs::director_snapshot_spec(generation),
            move |stores: Option<&DirectorStores>| {
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                FactoryJobOutput::DirectorSnapshot(request.read(stores))
            },
        );
        self.director.in_flight = matches!(
            submitted,
            Submitted::Queued | Submitted::QueuedDroppedOldest { .. }
        );
    }

    pub(crate) fn take_director_read(&mut self) -> Option<DirectorRefreshLoad> {
        self.poll();
        self.director.ready.take()
    }

    /// Start the delivery read for `events`. Events detected while a read is
    /// in flight wait for the next one. Hands the events back only when no
    /// job can run (the runtime could not start its lanes), so the caller
    /// can deliver them another way.
    pub(crate) fn start_delivery_read(
        &mut self,
        app: &FactoryApp,
        events: Vec<DirectorEvent>,
    ) -> Result<(), Vec<DirectorEvent>> {
        self.poll();
        if self.delivery.busy() {
            self.delivery_backlog.extend(events);
            return Ok(());
        }
        let request: DeliveryRequest = app.delivery_request(&events);
        let delay = self.read_delay;
        let generation = self.next_generation();
        // The events ride along with the read: if it is refused they come
        // back with the closure dropped, so keep a copy to hand back.
        let fallback = events.clone();
        let submitted = self.runtime(app).submit_read(
            factory_jobs::delivery_read_spec(generation),
            move |stores: Option<&DirectorStores>| {
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                let inputs = request.read(stores);
                FactoryJobOutput::Delivery(events, inputs)
            },
        );
        match submitted {
            Submitted::Queued | Submitted::QueuedBehindKey => {
                self.delivery.in_flight = true;
                Ok(())
            }
            refused => {
                tracing::warn!(?refused, "delivery read refused by the job runtime");
                Err(fallback)
            }
        }
    }

    /// Start the delivery read for events that waited behind the last one.
    pub(crate) fn start_delivery_backlog(
        &mut self,
        app: &FactoryApp,
    ) -> Result<(), Vec<DirectorEvent>> {
        if self.delivery.busy() || self.delivery_backlog.is_empty() {
            return Ok(());
        }
        let events = std::mem::take(&mut self.delivery_backlog);
        self.start_delivery_read(app, events)
    }

    pub(crate) fn take_delivery_read(&mut self) -> Option<(Vec<DirectorEvent>, DeliveryInputs)> {
        self.poll();
        self.delivery.ready.take()
    }

    /// Re-check `prompts` against a fresh read off the loop
    /// (`RevalidateFresh`). The batch comes back from
    /// [`Self::take_deliverability`], in order. Hands the prompts back when
    /// no job can run.
    pub(crate) fn start_deliverability_check(
        &mut self,
        app: &FactoryApp,
        prompts: Vec<Prompt>,
        events: Vec<DirectorEvent>,
        refresh_started: std::time::Instant,
    ) -> Result<(), Vec<Prompt>> {
        let request: DeliverabilityRequest = app.deliverability_request();
        let generation = self.next_generation();
        let delay = self.read_delay;
        let pending = PendingDeliverability {
            prompts: prompts.clone(),
            events: events.clone(),
            refresh_started,
        };
        let submitted = self.runtime(app).submit_read(
            factory_jobs::deliverability_spec(generation),
            move |stores: Option<&DirectorStores>| {
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                let verdicts = request.read(stores, &prompts);
                FactoryJobOutput::Deliverability(DeliverabilityBatch {
                    prompts: prompts.into_iter().zip(verdicts).collect(),
                    events,
                    refresh_started,
                })
            },
        );
        match submitted {
            Submitted::Queued | Submitted::QueuedBehindKey => {
                self.deliverability_pending.insert(generation, pending);
                Ok(())
            }
            refused => {
                tracing::warn!(?refused, "prompt re-check refused by the job runtime");
                Err(pending.prompts)
            }
        }
    }

    /// Re-checked prompt batches, in the order they finished.
    pub(crate) fn take_deliverability(&mut self) -> Vec<DeliverabilityBatch> {
        self.poll();
        std::mem::take(&mut self.deliverability_ready)
    }

    /// Run `requests` as `WorkerHealthProbe` jobs. Returns the probes that
    /// will not report, so the detector can ask again.
    pub(crate) fn submit_health_probes(
        &mut self,
        app: &FactoryApp,
        requests: Vec<WorkerHealthRequest>,
    ) -> Vec<(String, String)> {
        if requests.is_empty() {
            return Vec::new();
        }
        let jobs = self.runtime(app);
        let cas_root = app.cas_dir().to_path_buf();
        let mut refused = Vec::new();
        for request in requests {
            let generation = self.next_generation();
            let spec = factory_jobs::worker_health_spec(&request.worker, &request.task, generation);
            let key = spec.key.clone();
            let probe = (request.worker.clone(), request.task.clone());
            let cas_root = cas_root.clone();
            match jobs.submit(spec, move || {
                FactoryJobOutput::WorkerHealth(
                    crate::ui::factory::director::run_worker_health_probe(
                        Some(&cas_root),
                        &request,
                    ),
                )
            }) {
                Submitted::Queued => {
                    self.health_in_flight.insert(key, probe);
                }
                Submitted::QueuedDroppedOldest { dropped_key } => {
                    self.health_in_flight.insert(key, probe);
                    if let Some(dropped) = self.health_in_flight.remove(&dropped_key) {
                        refused.push(dropped);
                    }
                }
                // The key is already running: its report covers this ask.
                Submitted::Coalesced | Submitted::QueuedBehindKey => {}
                Submitted::DroppedRefresh | Submitted::Closed | Submitted::LaneMismatch { .. } => {
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
    ) -> (Vec<WorkerHealthReport>, Vec<(String, String)>) {
        self.poll();
        (
            std::mem::take(&mut self.health_ready),
            std::mem::take(&mut self.health_failed),
        )
    }
}

#[cfg(test)]
#[path = "director_refresh_tests.rs"]
mod tests;
