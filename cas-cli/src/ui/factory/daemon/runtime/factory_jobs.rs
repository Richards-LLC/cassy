//! Factory loop v2 C2 (cas-223f, spec-4c5b §4.3–4.4): the director's job
//! kinds on the [`super::jobs`] framework. They run on the daemon's one job
//! runtime, owned by the prompt queue's
//! [`super::prompt_queue_jobs::QueueJobs`] (C3), through its external API;
//! keys are prefixed with their kind and never use the queue's prefixes.
//!
//! | Kind | Lane | Budget | What moved off the loop |
//! |---|---|---|---|
//! | `DirectorSnapshot` | read | 250 ms | the two-second director read, plus the supervisor-stall delivery evidence (git, gh, task store, QA passes) |
//! | `RevalidateFresh` | read | 250 ms | the delivery read after detection (with idle-worker liveness) and the inject loop's final re-check |
//! | `WorkerHealthProbe` | blocking | 100 ms | stall confirmation: worker resolution, `/proc` scan, transcript reads |
//! | `ReminderScan` | write (StoreWorker) | 100 ms | reminder expiry, due reads and firing |
//! | `CiStatus` | external | 10 s | the git half of the supervisor-stall evidence: delivery tips, lane CI via `gh`, merged state |
//!
//! Revalidation is always a fresh read on the read lane's own connection,
//! never the cached snapshot (app/mod.rs, `prompt_is_still_deliverable`).

use std::time::Duration;

use super::jobs::{Criticality, JobSpec, Lane};
use crate::ui::factory::app::delivery_evidence::GitEvidenceMap;
use crate::ui::factory::app::{DeliveryInputs, DirectorRefreshLoad};
use crate::ui::factory::director::{DirectorEvent, Prompt, WorkerHealthReport};

pub(crate) const DIRECTOR_SNAPSHOT: &str = "DirectorSnapshot";
pub(crate) const REVALIDATE_FRESH: &str = "RevalidateFresh";
pub(crate) const WORKER_HEALTH_PROBE: &str = "WorkerHealthProbe";
pub(crate) const REMINDER_SCAN: &str = "ReminderScan";
pub(crate) const CI_STATUS: &str = "CiStatus";

/// Spec §4.4 budgets.
pub(crate) const DIRECTOR_SNAPSHOT_BUDGET: Duration = Duration::from_millis(250);
pub(crate) const REVALIDATE_FRESH_BUDGET: Duration = Duration::from_millis(250);
pub(crate) const WORKER_HEALTH_PROBE_BUDGET: Duration = Duration::from_millis(100);
pub(crate) const REMINDER_SCAN_BUDGET: Duration = Duration::from_millis(100);
pub(crate) const CI_STATUS_BUDGET: Duration = Duration::from_secs(10);

/// How long the loop waits for a job's result before it gives the job up
/// (it was dropped, or its result was lost) and retries: three budgets, and
/// never less than 30 s so a slow but healthy store is not retried early.
pub(crate) fn in_flight_timeout(budget: Duration) -> Duration {
    (budget * 3).max(Duration::from_secs(30))
}

pub(crate) const DIRECTOR_SNAPSHOT_KEY: &str = "DirectorSnapshot:director";
pub(crate) const DELIVERY_READ_KEY: &str = "RevalidateFresh:director:delivery";
pub(crate) const DELIVERABILITY_KEY: &str = "RevalidateFresh:director:deliverability";
pub(crate) const REMINDER_SCAN_KEY: &str = "ReminderScan:director";
pub(crate) const CI_STATUS_KEY: &str = "CiStatus:director";

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
    format!("WorkerHealthProbe:{worker}:{task}")
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

/// The git half of the supervisor-stall evidence: `git` and `gh` calls, so
/// it runs on the external lane, never the shared read lane (review M1).
pub(crate) fn ci_status_spec(generation: u64) -> JobSpec {
    JobSpec {
        kind: CI_STATUS,
        key: CI_STATUS_KEY.to_string(),
        lane: Lane::External,
        criticality: Criticality::Refresh,
        generation,
        budget: CI_STATUS_BUDGET,
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

/// What a director job hands back to the loop, boxed through the runtime's
/// external API and downcast by `DirectorRefresh`.
pub(crate) enum FactoryJobOutput {
    DirectorSnapshot(DirectorRefreshLoad),
    Delivery(Vec<DirectorEvent>, DeliveryInputs),
    Deliverability(DeliverabilityBatch),
    WorkerHealth(WorkerHealthReport),
    ReminderScan,
    CiStatus(GitEvidenceMap),
}
