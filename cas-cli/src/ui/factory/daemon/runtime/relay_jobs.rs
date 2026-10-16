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
//! retries.

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
}

impl Relay {
    fn name(self) -> &'static str {
        match self {
            Self::UsageLimit => "usage-limit",
            Self::AuthFailure => "auth-failure",
            Self::Permission => "permission",
            Self::Escalation => "escalation",
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
    DeathsRelayed {
        group: Vec<ObservedWorkerExit>,
        persisted: bool,
    },
    /// The open operator notices' states (`None` when unread or
    /// unreadable) and the undelivered supervisor relays still worth an
    /// alert.
    EscalationRead {
        notice_states: Option<Vec<cas_store::RelayAlertState>>,
        candidates: Vec<cas_store::QueuedPrompt>,
    },
    EscalationWritten(Vec<RelayOperatorAlert>),
}

/// The relays' jobs in flight and their finished results.
#[derive(Default)]
pub(crate) struct RelayJobs {
    generation: u64,
    /// Job keys in flight, with when they were submitted.
    in_flight: HashMap<String, Instant>,
    inline: Vec<RelayOutput>,
    /// Test hook: give up in-flight jobs after this.
    #[cfg(test)]
    pub(crate) timeout_override: Option<Duration>,
}

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
        self.in_flight.contains_key(&format!("{RELAY_WRITE}:{key}"))
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
            key: key.clone(),
            lane: Lane::Blocking,
            criticality: Criticality::Refresh,
            generation: self.next_generation(),
            budget: RELAY_READ_BUDGET,
        };
        self.submit(jobs, spec, read)
    }

    /// Run `write` on the write lane (the store worker), unless the write
    /// keyed `key` is in flight. Returns whether it was submitted.
    pub(crate) fn submit_write(
        &mut self,
        jobs: &mut QueueJobs,
        key: &str,
        write: impl FnOnce() -> RelayOutput + Send + 'static,
    ) -> bool {
        let key = format!("{RELAY_WRITE}:{key}");
        if self.in_flight.contains_key(&key) {
            return false;
        }
        let spec = JobSpec {
            kind: RELAY_WRITE,
            key: key.clone(),
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
        self.in_flight.insert(key.clone(), Instant::now());
        let _ = (jobs, spec);
        self.in_flight.remove(&key);
        self.inline.push(work());
        let submitted = Submitted::Queued;
        match submitted {
            Submitted::Queued | Submitted::QueuedBehindKey | Submitted::QueuedDroppedOldest { .. } => true,
            refused => {
                tracing::warn!(key = %key, ?refused, "relay job refused by the job runtime; retrying next scan");
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

    /// The relays' finished jobs, without waiting. Jobs that will not report
    /// (panicked, dropped, or timed out) free their keys.
    pub(crate) fn take(&mut self, jobs: &mut QueueJobs) -> Vec<RelayOutput> {
        jobs.apply_completions();
        let mut outputs = std::mem::take(&mut self.inline);
        for result in jobs.take_external_completions_where(is_relay_kind) {
            if let Some(output) = self.accept(result) {
                outputs.push(output);
            }
        }
        let timeout = self.timeout(RELAY_READ_BUDGET.max(RELAY_WRITE_BUDGET));
        self.in_flight.retain(|key, since| {
            let expired = since.elapsed() >= timeout;
            if expired {
                tracing::warn!(key = %key, "relay job never reported; retrying next scan");
            }
            !expired
        });
        outputs
    }

    fn accept(&mut self, result: JobResult<ExternalOutcome>) -> Option<RelayOutput> {
        let JobResult { key, outcome, .. } = result;
        self.in_flight.remove(&key);
        match outcome {
            Ok(ExternalOutcome::Done(value)) => match value.downcast::<RelayOutput>() {
                Ok(output) => Some(*output),
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
