//! Factory loop v2 C2c (cas-4491f, GH #1211): the daemon's relays as jobs.
//!
//! The relays (usage-limited workers, account failures, teammate permission
//! requests, correlated worker deaths, and the operator escalation of
//! undelivered supervisor relays) used to read the agent store, transcripts,
//! Teams inboxes and the prompt queue on the loop, and write their notices
//! there too. Each one is now three steps:
//!
//! 1. a read job on the blocking lane (`RelayRead:<relay>`, single-flight);
//! 2. a decision on the loop, which keeps the per-daemon dedupe state;
//! 3. a write job on the write lane, the store worker (`RelayWrite:<key>`),
//!    whose outcome updates that dedupe state when it reports.
//!
//! The cadence, dedupe and notices are the relays' own (queue_and_events.rs);
//! only where the IO runs changed. A job that panics, is dropped, or outlives
//! [`super::factory_jobs::in_flight_timeout`] frees its key so the next scan
//! retries; a write is never given up. Results are matched to the
//! generation in flight, so a late one is ignored, and writes are applied
//! before reads (review M2/M3).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::jobs::{Criticality, JobResult, JobSpec, Lane, Submitted};
use super::prompt_queue_jobs::{ExternalOutcome, QueueJobs};
use super::queue_and_events::{ObservedWorkerExit, RelayOperatorAlert};

pub(crate) const RELAY_READ: &str = "RelayRead";
pub(crate) const RELAY_WRITE: &str = "RelayWrite";

/// Whether an external job result belongs to the relays (the director
/// refresh takes every other one).
pub(crate) fn is_relay_kind(kind: &str) -> bool {
    kind == RELAY_READ || kind == RELAY_WRITE
}

/// Spec §4.4 budgets: a relay read is a blocking-lane probe, a relay write a
/// store-worker write.
const RELAY_READ_BUDGET: Duration = Duration::from_millis(100);
const RELAY_WRITE_BUDGET: Duration = Duration::from_millis(100);

/// One relay's read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Relay {
    UsageLimit,
    AuthFailure,
    Permission,
    Escalation,
    /// cas-26c8: recorded worker deaths whose prompt no live hold covers.
    DeathSweep,
}

impl Relay {
    fn name(self) -> &'static str {
        match self {
            Self::UsageLimit => "usage-limit",
            Self::AuthFailure => "auth-failure",
            Self::Permission => "permission",
            Self::Escalation => "escalation",
            Self::DeathSweep => "death-sweep",
        }
    }

    fn key(self) -> String {
        format!("{RELAY_READ}:{}", self.name())
    }
}

/// A teammate permission request the read found, with whether CAS already
/// allowed that exact call.
#[derive(Debug, Clone)]
pub(crate) struct PermissionCandidate {
    pub(crate) root: PathBuf,
    pub(crate) request: crate::factory_permission_relay::WorkerPermissionRequest,
    pub(crate) pre_approved: bool,
}

/// What a relay job hands back to the loop.
pub(crate) enum RelayOutput {
    /// Each candidate worker's usage-limit evidence: `(agent id, name,
    /// evidence)`.
    UsageLimitRead(
        Vec<(
            String,
            String,
            crate::mcp::tools::service::factory_ops::UsageLimitEvidence,
        )>,
    ),
    UsageLimitWritten {
        agent_id: String,
        worker: String,
        occurrence: String,
        persisted: bool,
    },
    /// Each candidate worker with its account-failure evidence.
    AuthRead(Vec<(cas_types::Agent, crate::factory_auth_health::AuthFailureEvidence)>),
    AuthWritten {
        agent_id: String,
        worker: String,
        occurrence: String,
        cli: String,
        result: Result<(), String>,
    },
    PermissionRead(Vec<PermissionCandidate>),
    PermissionAnswered {
        request: crate::factory_permission_relay::WorkerPermissionRequest,
        result: Result<(), String>,
    },
    PermissionRelayed {
        request: crate::factory_permission_relay::WorkerPermissionRequest,
        persisted: bool,
    },
    /// cas-26c8: a death group's write reported. `done` is false only when
    /// a folded relay must be tried again.
    DeathsRelayed {
        group: Vec<ObservedWorkerExit>,
        done: bool,
    },
    /// cas-26c8: this session's folded relays (`(id, payload)`) and
    /// per-worker deaths whose prompt never reached the queue.
    DeathSweepRead {
        folded: Vec<(i64, serde_json::Value)>,
        deaths: Vec<crate::mcp::tools::service::orphan_recovery::UnstampedWorkerDeath>,
        /// cas-ff42 L3: rows that just aged out of the sweep.
        given_up: Vec<super::queue_and_events::GivenUpDeath>,
    },
    /// cas-ff42 C6: an exited worker's stale mark and death record were
    /// written on the store worker. `at` names its exit in the correlation
    /// window, if it waits there.
    WorkerDeathRecorded {
        worker: String,
        at: Option<chrono::DateTime<chrono::Utc>>,
        death: Option<super::queue_and_events::DeferredWorkerDeath>,
    },
    /// cas-26c8: one swept relay was (re)injected, or found already handled.
    DeathSweepWritten,
    /// The open operator notices the read asked about, their states
    /// (`None` when unreadable) and the undelivered supervisor relays still
    /// worth an alert.
    EscalationRead {
        notice_ids: Vec<i64>,
        notice_states: Option<Vec<cas_store::RelayAlertState>>,
        candidates: Vec<cas_store::QueuedPrompt>,
    },
    EscalationWritten(Vec<RelayOperatorAlert>),
}

/// A finished relay job: its output, and the generation it ran under.
pub(crate) struct RelayDone {
    pub(crate) generation: u64,
    pub(crate) output: RelayOutput,
}

/// The relays' jobs in flight and their finished results.
#[derive(Default)]
pub(crate) struct RelayJobs {
    generation: u64,
    /// Job keys in flight: the generation whose result is awaited, and when
    /// it was submitted. A result for any other generation is stale and is
    /// ignored (cas-4491f review M2).
    in_flight: HashMap<String, (u64, Instant)>,
    /// For each write key, the generation counter when its last result was
    /// accepted. A read submitted before then may not see that write, so it
    /// cannot ask for the same write again (cas-4491f review M3).
    written_at: HashMap<String, u64>,
    /// When a refusal was last logged at warn level (review L: one a minute,
    /// not one a tick).
    last_refusal_warning: Option<Instant>,
    /// Test hook: give up in-flight reads after this.
    #[cfg(test)]
    pub(crate) timeout_override: Option<Duration>,
    /// cas-ff42 L3: recorded-death rows the sweep already reported giving
    /// up on, so each is warned about once.
    pub(crate) death_sweep_given_up: std::collections::HashSet<i64>,
    /// Test hook (cas-26c8): relay a death group with this instead of
    /// `relay_death_group_io`, e.g. to make a group keep asking for a retry.
    #[cfg(test)]
    pub(crate) death_group_override:
        Option<fn(&std::path::Path, &str, &[ObservedWorkerExit], chrono::Duration) -> bool>,
    /// Test hook (cas-ff42): record a worker death with this instead of
    /// `record_worker_death_io`, e.g. to make the record job panic.
    #[cfg(test)]
    pub(crate) death_record_override: Option<DeathRecordFn>,
}

/// The shape of `record_worker_death_io`, for the cas-ff42 test hook.
#[cfg(test)]
pub(crate) type DeathRecordFn = fn(
    &std::path::Path,
    &str,
    &str,
    Option<&str>,
    &str,
    Option<&crate::prompt_revalidation::WorkerExitCause>,
    crate::mcp::tools::service::orphan_recovery::DeathRelay,
    Option<chrono::DateTime<chrono::Utc>>,
) -> Option<super::queue_and_events::DeferredWorkerDeath>;

impl RelayJobs {
    fn next_generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// Whether no relay job is in flight.
    #[cfg(test)]
    pub(crate) fn idle(&self) -> bool {
        self.in_flight.is_empty()
    }

    /// Whether `relay`'s read is in flight.
    pub(crate) fn reading(&self, relay: Relay) -> bool {
        self.in_flight.contains_key(&relay.key())
    }

    /// Whether the write keyed `key` is in flight.
    pub(crate) fn writing(&self, key: &str) -> bool {
        self.in_flight.contains_key(&write_key(key))
    }

    /// Run `read` for `relay` on the blocking lane, unless its read is in
    /// flight. Returns whether it was submitted.
    pub(crate) fn submit_read(
        &mut self,
        jobs: &mut QueueJobs,
        relay: Relay,
        read: impl FnOnce() -> RelayOutput + Send + 'static,
    ) -> bool {
        let key = relay.key();
        if self.in_flight.contains_key(&key) {
            return false;
        }
        let spec = JobSpec {
            kind: RELAY_READ,
            key,
            lane: Lane::Blocking,
            criticality: Criticality::Refresh,
            generation: self.next_generation(),
            budget: RELAY_READ_BUDGET,
        };
        self.submit(jobs, spec, read)
    }

    /// Run `write` on the write lane (the store worker), unless the write
    /// keyed `key` is in flight, or `basis` (the generation of the read the
    /// decision came from) predates that key's last write: such a read may
    /// not have seen it (review M3). Returns whether it was submitted.
    pub(crate) fn submit_write(
        &mut self,
        jobs: &mut QueueJobs,
        key: &str,
        basis: Option<u64>,
        write: impl FnOnce() -> RelayOutput + Send + 'static,
    ) -> bool {
        let key = write_key(key);
        if self.in_flight.contains_key(&key) {
            return false;
        }
        if let (Some(basis), Some(written)) = (basis, self.written_at.get(&key))
            && *written >= basis
        {
            tracing::debug!(key = %key, basis, written, "relay write skipped: decided from a read older than it");
            return false;
        }
        let spec = JobSpec {
            kind: RELAY_WRITE,
            key,
            lane: Lane::Write,
            criticality: Criticality::Critical,
            generation: self.next_generation(),
            budget: RELAY_WRITE_BUDGET,
        };
        self.submit(jobs, spec, write)
    }

    fn submit(
        &mut self,
        jobs: &mut QueueJobs,
        spec: JobSpec,
        work: impl FnOnce() -> RelayOutput + Send + 'static,
    ) -> bool {
        let key = spec.key.clone();
        // Marked first: an inline runtime finishes the job inside submit.
        self.in_flight.insert(key.clone(), (spec.generation, Instant::now()));
        let submitted = jobs.submit_external(spec, move || {
            Box::new(work()) as Box<dyn std::any::Any + Send>
        });
        match submitted {
            // A coalesced read is the key's rerun: it reports under this
            // generation once the job still running (one given up at its
            // timeout) finishes (review M2).
            Submitted::Queued
            | Submitted::QueuedBehindKey
            | Submitted::QueuedDroppedOldest { .. }
            | Submitted::Coalesced { .. } => true,
            refused => {
                const REFUSAL_WARNING_INTERVAL: Duration = Duration::from_secs(60);
                if self
                    .last_refusal_warning
                    .is_none_or(|last| last.elapsed() >= REFUSAL_WARNING_INTERVAL)
                {
                    self.last_refusal_warning = Some(Instant::now());
                    tracing::warn!(key = %key, ?refused, "relay job refused by the job runtime; retrying next scan");
                } else {
                    tracing::debug!(key = %key, ?refused, "relay job refused by the job runtime; retrying next scan");
                }
                self.in_flight.remove(&key);
                false
            }
        }
    }

    #[cfg(test)]
    fn timeout(&self, budget: Duration) -> Duration {
        self.timeout_override
            .unwrap_or_else(|| super::factory_jobs::in_flight_timeout(budget))
    }

    #[cfg(not(test))]
    fn timeout(&self, budget: Duration) -> Duration {
        super::factory_jobs::in_flight_timeout(budget)
    }

    /// The relays' finished jobs, without waiting, writes first: a read in
    /// the same batch is then decided against the dedupe its writes set
    /// (review M3). A read that panicked, was dropped or outlived its
    /// timeout frees its key so the next scan reads again. A write is never
    /// given up: the store worker runs every write it takes, and a second
    /// one could repeat a notice (review M2).
    pub(crate) fn take(&mut self, jobs: &mut QueueJobs) -> Vec<RelayDone> {
        jobs.apply_completions();
        self.forget_settled_writes();
        let mut writes = Vec::new();
        let mut reads = Vec::new();
        for result in jobs.take_external_completions_where(is_relay_kind) {
            let is_write = result.kind == RELAY_WRITE;
            if let Some(done) = self.accept(result) {
                if is_write {
                    writes.push(done);
                } else {
                    reads.push(done);
                }
            }
        }
        let timeout = self.timeout(RELAY_READ_BUDGET);
        self.in_flight.retain(|key, (_, since)| {
            let expired = key.starts_with(RELAY_READ) && since.elapsed() >= timeout;
            if expired {
                tracing::warn!(key = %key, "relay read never reported; reading again next scan");
            }
            !expired
        });
        writes.extend(reads);
        writes
    }

    /// Drop the write records no read can still predate (review L: one per
    /// permission request would otherwise accumulate). A record matters only
    /// while a read submitted before it is in flight; every read submitted
    /// later is newer than it. The previous batch's reads were applied before
    /// this call.
    fn forget_settled_writes(&mut self) {
        let oldest_read = self
            .in_flight
            .iter()
            .filter(|(key, _)| key.starts_with(RELAY_READ))
            .map(|(_, (generation, _))| *generation)
            .min();
        match oldest_read {
            Some(oldest) => self.written_at.retain(|_, written| *written >= oldest),
            None => self.written_at.clear(),
        }
    }

    /// Write records kept for reads in flight (tests).
    #[cfg(test)]
    pub(crate) fn write_records(&self) -> usize {
        self.written_at.len()
    }

    fn accept(&mut self, result: JobResult<ExternalOutcome>) -> Option<RelayDone> {
        let JobResult {
            kind,
            key,
            generation,
            outcome,
            ..
        } = result;
        if self
            .in_flight
            .get(&key)
            .is_none_or(|(awaited, _)| *awaited != generation)
        {
            tracing::debug!(key = %key, generation, "stale relay job result ignored");
            return None;
        }
        self.in_flight.remove(&key);
        if kind == RELAY_WRITE {
            self.written_at.insert(key.clone(), self.generation);
        }
        match outcome {
            Ok(ExternalOutcome::Done(value)) => match value.downcast::<RelayOutput>() {
                Ok(output) => Some(RelayDone {
                    generation,
                    output: *output,
                }),
                Err(_) => {
                    tracing::debug!(key = %key, "relay job result of an unexpected type ignored");
                    None
                }
            },
            Ok(ExternalOutcome::Dropped) => {
                tracing::debug!(key = %key, "relay job dropped by a full lane; retrying next scan");
                None
            }
            Err(panicked) => {
                tracing::error!(key = %key, message = %panicked.message, "relay job panicked; retrying next scan");
                None
            }
        }
    }
}

fn write_key(key: &str) -> String {
    format!("{RELAY_WRITE}:{key}")
}
