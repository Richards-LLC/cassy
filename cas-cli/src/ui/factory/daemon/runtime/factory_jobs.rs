//! Factory loop v2 C2 (cas-223f, spec-4c5b §4.3–4.4): the daemon's job kinds
//! on the [`super::jobs`] framework, and the one runtime that runs them.
//!
//! | Kind | Lane | Budget | What moved off the loop |
//! |---|---|---|---|
//! | `DirectorSnapshot` | read | 250 ms | the two-second director read, plus the supervisor-stall delivery evidence (git, gh, task store, QA passes) |
//! | `RevalidateFresh` | read | 250 ms | the delivery read after detection (with idle-worker liveness) and the inject loop's final re-check |
//! | `WorkerHealthProbe` | blocking | 100 ms | stall confirmation: worker resolution, `/proc` scan, transcript reads |
//! | `ReminderScan` | write (StoreWorker) | 100 ms | reminder expiry, due reads and firing |
//!
//! Revalidation is always a fresh read on the read lane's own connection,
//! never the cached snapshot (app/mod.rs, `prompt_is_still_deliverable`).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::jobs::{Criticality, JobRuntime, JobSpec, Lane, LaneWiring};
use crate::ui::factory::app::{DeliveryInputs, DirectorRefreshLoad};
use crate::ui::factory::director::{DirectorEvent, Prompt, WorkerHealthReport};

pub(crate) const DIRECTOR_SNAPSHOT: &str = "DirectorSnapshot";
pub(crate) const REVALIDATE_FRESH: &str = "RevalidateFresh";
pub(crate) const WORKER_HEALTH_PROBE: &str = "WorkerHealthProbe";
pub(crate) const REMINDER_SCAN: &str = "ReminderScan";

/// Spec §4.4 budgets.
pub(crate) const DIRECTOR_SNAPSHOT_BUDGET: Duration = Duration::from_millis(250);
pub(crate) const REVALIDATE_FRESH_BUDGET: Duration = Duration::from_millis(250);
pub(crate) const WORKER_HEALTH_PROBE_BUDGET: Duration = Duration::from_millis(100);
pub(crate) const REMINDER_SCAN_BUDGET: Duration = Duration::from_millis(100);

/// Queue cap per lane. Refresh work past it replaces older refresh work;
/// critical work is never dropped.
pub(crate) const LANE_CAPACITY: usize = 64;

pub(crate) const DIRECTOR_SNAPSHOT_KEY: &str = "director-snapshot";
pub(crate) const DELIVERY_READ_KEY: &str = "delivery-read";
pub(crate) const DELIVERABILITY_KEY: &str = "deliverability";
pub(crate) const REMINDER_SCAN_KEY: &str = "reminder-scan";

/// A director snapshot: replaceable by the next one.
pub(crate) fn director_snapshot_spec(generation: u64) -> JobSpec {
    JobSpec {
        kind: DIRECTOR_SNAPSHOT,
        key: DIRECTOR_SNAPSHOT_KEY.to_string(),
        lane: Lane::Read,
        criticality: Criticality::Refresh,
        generation,
        budget: DIRECTOR_SNAPSHOT_BUDGET,
    }
}

/// The delivery read carries detected events, so it is never dropped.
pub(crate) fn delivery_read_spec(generation: u64) -> JobSpec {
    JobSpec {
        kind: REVALIDATE_FRESH,
        key: DELIVERY_READ_KEY.to_string(),
        lane: Lane::Read,
        criticality: Criticality::Critical,
        generation,
        budget: REVALIDATE_FRESH_BUDGET,
    }
}

/// The inject loop's re-check carries prompts, so it is never dropped.
pub(crate) fn deliverability_spec(generation: u64) -> JobSpec {
    JobSpec {
        kind: REVALIDATE_FRESH,
        key: DELIVERABILITY_KEY.to_string(),
        lane: Lane::Read,
        criticality: Criticality::Critical,
        generation,
        budget: REVALIDATE_FRESH_BUDGET,
    }
}

/// The single-flight key of a worker health probe.
pub(crate) fn worker_health_key(worker: &str, task: &str) -> String {
    format!("worker-health:{worker}:{task}")
}

/// A worker health probe, single-flight per worker and task.
pub(crate) fn worker_health_spec(worker: &str, task: &str, generation: u64) -> JobSpec {
    JobSpec {
        kind: WORKER_HEALTH_PROBE,
        key: worker_health_key(worker, task),
        lane: Lane::Blocking,
        criticality: Criticality::Refresh,
        generation,
        budget: WORKER_HEALTH_PROBE_BUDGET,
    }
}

/// A reminder pass. One carrying director events, external observations or
/// CI wakes is critical (those are edges, consumed once). A pass with only
/// time reminders is refresh work: the next pass sees the same due rows.
pub(crate) fn reminder_scan_spec(carries_edges: bool, generation: u64) -> JobSpec {
    JobSpec {
        kind: REMINDER_SCAN,
        key: REMINDER_SCAN_KEY.to_string(),
        lane: Lane::Write,
        criticality: if carries_edges {
            Criticality::Critical
        } else {
            Criticality::Refresh
        },
        generation,
        budget: REMINDER_SCAN_BUDGET,
    }
}

/// The inject loop's prompts after their fresh re-check.
pub(crate) struct DeliverabilityBatch {
    /// Each prompt with whether it is still deliverable, in batch order.
    pub(crate) prompts: Vec<(Prompt, bool)>,
    /// The tick's revalidated events (stall nudges look up their episode).
    pub(crate) events: Vec<DirectorEvent>,
    /// When the tick's refresh started, for delivery latency telemetry.
    pub(crate) refresh_started: std::time::Instant,
}

/// What a factory job hands back to the loop.
pub(crate) enum FactoryJobOutput {
    DirectorSnapshot(DirectorRefreshLoad),
    Delivery(Vec<DirectorEvent>, DeliveryInputs),
    Deliverability(DeliverabilityBatch),
    WorkerHealth(WorkerHealthReport),
    ReminderScan,
}

pub(crate) type FactoryJobRuntime = JobRuntime<FactoryJobOutput>;

/// Start a runtime wired for the daemon: the read lane opens its own
/// connection to `cas_dir`, and writes run on the StoreWorker.
pub(crate) fn start_runtime(cas_dir: &Path, project_id: Option<String>) -> Arc<FactoryJobRuntime> {
    Arc::new(JobRuntime::start_with(
        LANE_CAPACITY,
        LaneWiring::daemon(cas_dir.to_path_buf(), project_id),
    ))
}

/// The daemon's job runtime, started on first use.
#[derive(Default)]
pub(crate) struct FactoryJobs {
    runtime: Option<Arc<FactoryJobRuntime>>,
}

impl FactoryJobs {
    /// The runtime, started now if it is not running yet.
    pub(crate) fn runtime(
        &mut self,
        cas_dir: &Path,
        project_id: Option<String>,
    ) -> Arc<FactoryJobRuntime> {
        Arc::clone(
            self.runtime
                .get_or_insert_with(|| start_runtime(cas_dir, project_id)),
        )
    }

    /// Shutdown: stop taking work and wait up to `timeout` for queued
    /// critical jobs. Call before waiting on the StoreWorker, since write-lane
    /// jobs run there (see [`JobRuntime::drain`]). Every other holder of the
    /// runtime must be dropped first; otherwise this only closes it.
    pub(crate) fn drain(&mut self, timeout: Duration) -> bool {
        let Some(runtime) = self.runtime.take() else {
            return true;
        };
        match Arc::try_unwrap(runtime) {
            Ok(runtime) => runtime.drain(timeout),
            Err(shared) => {
                shared.close();
                tracing::warn!(
                    "factory job runtime still shared at shutdown; closed without waiting"
                );
                false
            }
        }
    }
}
