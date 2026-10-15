//! Factory loop v2 C3 (cas-d2a2, spec-4c5b §4.3 and §4.5): the prompt queue's
//! store work runs as jobs instead of on the loop.
//!
//! - **QueueSelect** (write lane, critical) selects the delivery batch, stamps
//!   each row `Selected` and takes its transport claims in one transaction
//!   ([`cas_store::PromptQueueStore::select_and_claim_for_targets`]). Control
//!   delivers the returned rows on a later pass and decides wake and gate as
//!   before.
//! - **QueueStamp** (write lane, critical) records what a delivery did: claim
//!   releases, transport stamps and receipts, wake attempts and pending
//!   reasons. All stamps for one `(row, recipient)` share a single-flight key,
//!   so they land in submission order.
//! - **In-flight set** (spec §4.5 rules 2–3): control records `(row,
//!   recipient, generation)` before it injects and skips any selected row it
//!   holds. The entry clears when that generation's settling stamp completes.
//!   An inject with no outcome after [`INJECT_OUTCOME_TIMEOUT`] has its claim
//!   released, and the row retries on its normal cadence.
//! - **Idempotent stamps** (rule 4): a stamp repeated for the same row,
//!   recipient, generation and kind is not submitted twice; the store's stage
//!   stamps are monotonic besides.
//! - **RevalidateFresh → InboxPrune**: inbox retraction decides from a fresh
//!   read issued for that decision, never the cached director snapshot
//!   (app/mod.rs:1541–1547). The read runs on the read lane. The freshness
//!   checks (git) and the inbox flock run on the blocking lane, and the checks
//!   finish before the flock is taken.
//! - **MirrorWrite**: the Commander mirror scan, with its cursor state moved
//!   into the job and handed back with the result.
//! - [`ConfigCache`]: `Config` is reloaded only when its file changes.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use super::jobs::{
    Criticality, JobPanicked, JobResult, JobRuntime, JobSpec, Lane, LaneWiring, Submitted,
};
use super::teams::{InboxPruner, RetractionKeys};
use crate::ui::factory::director::{DirectorData, DirectorStores};

/// Watchdog budget for `QueueSelect` and `QueueStamp` (spec §4.4).
pub(crate) const QUEUE_JOB_BUDGET: Duration = Duration::from_millis(100);
/// Watchdog budget for a `RevalidateFresh` read (spec §4.4).
pub(crate) const REVALIDATE_FRESH_BUDGET: Duration = Duration::from_millis(250);
/// `InboxPrune` runs merge-freshness checks (bounded git) before the flock.
pub(crate) const INBOX_PRUNE_BUDGET: Duration = Duration::from_secs(5);
/// `MirrorWrite` budget (spec §4.4).
pub(crate) const MIRROR_WRITE_BUDGET: Duration = Duration::from_secs(2);
/// No inject outcome within this window releases the claim (spec §4.5 rule 3).
pub(crate) const INJECT_OUTCOME_TIMEOUT: Duration = Duration::from_secs(10);

const QUEUE_SELECT_KEY: &str = "QueueSelect";
const MIRROR_WRITE_KEY: &str = "MirrorWrite";
const LANE_CAPACITY: usize = 256;

/// What one store write records about a delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QueueStamp {
    /// Release a transport claim that was not handed off.
    ReleaseClaim,
    /// The row reached its recipient's transport.
    TransportDelivered,
    /// The recipient's transport receipt (`transport_claimed` surfacing).
    TransportReceipt,
    /// Why the row stays pending.
    PendingReason {
        reason: cas_store::PendingReason,
        detail: Option<String>,
    },
    /// What the wake nudge did.
    WakeAttempt {
        attempt: cas_store::WakeAttempt,
        detail: Option<String>,
    },
    /// No write: completes after every earlier stamp for the same row and
    /// recipient, and clears the row's in-flight entry.
    Settled,
}

impl QueueStamp {
    fn label(&self) -> &'static str {
        match self {
            Self::ReleaseClaim => "release_claim",
            Self::TransportDelivered => "transport_delivered",
            Self::TransportReceipt => "transport_receipt",
            Self::PendingReason { .. } => "pending_reason",
            Self::WakeAttempt { .. } => "wake_attempt",
            Self::Settled => "settled",
        }
    }

    fn apply(&self, queue: &dyn cas_store::PromptQueueStore, row: i64, recipient: &str) -> cas_store::Result<()> {
        match self {
            Self::ReleaseClaim => queue.release_recipient_transport(row, recipient),
            Self::TransportDelivered => queue.mark_transport_delivered(row),
            Self::TransportReceipt => queue.record_recipient_surfaced(
                row,
                recipient,
                cas_store::SurfacingSource::TransportClaimed,
            ),
            Self::PendingReason { reason, detail } => {
                queue.record_pending_reason(row, *reason, detail.as_deref())
            }
            Self::WakeAttempt { attempt, detail } => {
                queue.record_wake_attempt(row, *attempt, detail.as_deref())
            }
            Self::Settled => Ok(()),
        }
    }
}

/// The finished store write for one stamp.
#[derive(Debug)]
pub(crate) struct StampDone {
    pub(crate) row: i64,
    pub(crate) recipient: String,
    pub(crate) generation: u64,
    pub(crate) stamp: &'static str,
    pub(crate) error: Option<String>,
}

/// What control hands `QueueSelect`.
#[derive(Debug, Clone)]
pub(crate) struct QueueSelectRequest {
    pub(crate) cas_dir: PathBuf,
    pub(crate) session_name: String,
    pub(crate) supervisor_name: String,
    pub(crate) worker_names: Vec<String>,
    pub(crate) dead_workers: HashSet<String>,
    pub(crate) limit: usize,
}

/// One delivery batch and the store state it was selected against.
#[derive(Debug, Default)]
pub(crate) struct QueueSelection {
    pub(crate) generation: u64,
    /// Every registered agent, read in the selecting job.
    pub(crate) registered_agents: Vec<cas_types::Agent>,
    /// Rows whose QA-rejection target moved; control forgets their state.
    pub(crate) retargeted_rows: Vec<i64>,
    /// Recipients that shut down; their unread rows were withdrawn.
    pub(crate) shut_down_recipients: Vec<String>,
    /// Session agents served by their native extension, not by the daemon.
    pub(crate) native_agents: HashSet<String>,
    /// The batch, with the claims the selection took.
    pub(crate) rows: Vec<cas_store::SelectedPrompt>,
}

/// A fresh read for one retraction decision.
pub(crate) type FreshRead =
    Box<dyn FnOnce(Option<&DirectorStores>) -> anyhow::Result<DirectorData> + Send + 'static>;

/// What one inbox prune removed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct PruneReport {
    pub(crate) recipient: String,
    pub(crate) idle: usize,
    pub(crate) merge: usize,
    pub(crate) epic: usize,
    pub(crate) error: Option<String>,
}

/// Results posted by the prompt-queue jobs.
pub(crate) enum QueueJobOutcome {
    Selected(anyhow::Result<QueueSelection>),
    Stamped(StampDone),
    Revalidated {
        recipient: String,
        data: anyhow::Result<DirectorData>,
    },
    Pruned(PruneReport),
    Mirrored(Box<super::MirrorScanState>),
}

/// Where queue jobs run. `Inline` runs each job on the submitting thread
/// before `submit` returns: the factory daemon's unit tests use it so one
/// `process_prompt_queue` call still selects, delivers and stamps. The daemon
/// itself always uses `Lanes`.
enum Exec {
    Lanes(JobRuntime<QueueJobOutcome>),
    Inline(VecDeque<JobResult<QueueJobOutcome>>),
}

impl Exec {
    fn submit(
        &mut self,
        spec: JobSpec,
        work: impl FnOnce() -> QueueJobOutcome + Send + 'static,
    ) -> Submitted {
        match self {
            Self::Lanes(runtime) => runtime.submit(spec, work),
            Self::Inline(done) => {
                done.push_back(run_inline(spec, work));
                Submitted::Queued
            }
        }
    }

    fn submit_read(
        &mut self,
        spec: JobSpec,
        work: impl FnOnce(Option<&DirectorStores>) -> QueueJobOutcome + Send + 'static,
    ) -> Submitted {
        match self {
            Self::Lanes(runtime) => runtime.submit_read::<DirectorStores>(spec, work),
            Self::Inline(done) => {
                done.push_back(run_inline(spec, || work(None)));
                Submitted::Queued
            }
        }
    }

    fn drain(&mut self) -> Vec<JobResult<QueueJobOutcome>> {
        match self {
            Self::Lanes(runtime) => runtime.drain_completions(),
            Self::Inline(done) => done.drain(..).collect(),
        }
    }
}

fn run_inline(
    spec: JobSpec,
    work: impl FnOnce() -> QueueJobOutcome,
) -> JobResult<QueueJobOutcome> {
    let started = Instant::now();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).map_err(|panic| {
        JobPanicked {
            message: panic
                .downcast_ref::<&str>()
                .map(|message| (*message).to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "non-string panic payload".to_string()),
        }
    });
    let elapsed = started.elapsed();
    JobResult {
        kind: spec.kind,
        key: spec.key,
        generation: spec.generation,
        outcome,
        elapsed,
        writer_wait: Duration::ZERO,
        over_budget: elapsed > spec.budget,
    }
}

/// Where an in-flight inject stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InjectPhase {
    /// Sent; no outcome yet. Times out after [`INJECT_OUTCOME_TIMEOUT`].
    AwaitingOutcome,
    /// Outcome recorded; its stamps are being written.
    Stamping,
    /// The settling stamp is queued; the entry clears when it completes.
    Settling,
}

#[derive(Debug, Clone, Copy)]
struct InFlight {
    generation: u64,
    since: Instant,
    phase: InjectPhase,
}

/// Rows control has injected whose stamps have not landed (spec §4.5 rule 2).
/// Time is passed in, so tests drive it with a fake clock.
#[derive(Debug, Default)]
pub(crate) struct InFlightSet {
    entries: HashMap<(i64, String), InFlight>,
}

impl InFlightSet {
    /// Record an inject about to be sent at `now`.
    pub(crate) fn begin(&mut self, row: i64, recipient: &str, generation: u64, now: Instant) {
        self.entries.insert(
            (row, recipient.to_string()),
            InFlight {
                generation,
                since: now,
                phase: InjectPhase::AwaitingOutcome,
            },
        );
    }

    /// Whether any recipient of `row` is in flight.
    pub(crate) fn holds_row(&self, row: i64) -> bool {
        self.entries.keys().any(|(id, _)| *id == row)
    }

    pub(crate) fn generation_of(&self, row: i64, recipient: &str) -> Option<u64> {
        self.entries
            .get(&(row, recipient.to_string()))
            .map(|entry| entry.generation)
    }

    /// The inject's outcome arrived.
    pub(crate) fn record_outcome(&mut self, row: i64, recipient: &str, generation: u64) {
        if let Some(entry) = self.entries.get_mut(&(row, recipient.to_string()))
            && entry.generation == generation
            && entry.phase == InjectPhase::AwaitingOutcome
        {
            entry.phase = InjectPhase::Stamping;
        }
    }

    /// Entries whose outcome is in and whose settling stamp is not queued yet;
    /// marks them settling.
    fn take_unsettled(&mut self) -> Vec<(i64, String, u64)> {
        self.entries
            .iter_mut()
            .filter(|(_, entry)| entry.phase == InjectPhase::Stamping)
            .map(|((row, recipient), entry)| {
                entry.phase = InjectPhase::Settling;
                (*row, recipient.clone(), entry.generation)
            })
            .collect()
    }

    /// The settling stamp for `generation` completed: clear the entry. A
    /// stamp from an older generation leaves a newer entry alone.
    pub(crate) fn settle(&mut self, row: i64, recipient: &str, generation: u64) -> bool {
        let key = (row, recipient.to_string());
        if self
            .entries
            .get(&key)
            .is_some_and(|entry| entry.generation == generation && entry.phase == InjectPhase::Settling)
        {
            self.entries.remove(&key);
            return true;
        }
        false
    }

    /// Remove and return injects with no outcome after `timeout`.
    pub(crate) fn expire(&mut self, now: Instant, timeout: Duration) -> Vec<(i64, String, u64)> {
        let expired: Vec<(i64, String, u64)> = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                entry.phase == InjectPhase::AwaitingOutcome
                    && now.saturating_duration_since(entry.since) >= timeout
            })
            .map(|((row, recipient), entry)| (*row, recipient.clone(), entry.generation))
            .collect();
        for (row, recipient, _) in &expired {
            self.entries.remove(&(*row, recipient.clone()));
        }
        expired
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// `Config` reloaded only when `config.toml` or `config.yaml` changes, so a
/// queue pass never parses the file (spec §4.3).
#[derive(Default)]
pub(crate) struct ConfigCache {
    cached: Option<(ConfigStamp, Arc<crate::config::Config>)>,
}

type ConfigStamp = [(Option<SystemTime>, u64); 2];

fn config_stamp(cas_dir: &Path) -> ConfigStamp {
    ["config.toml", "config.yaml"].map(|name| {
        std::fs::metadata(cas_dir.join(name))
            .map(|meta| (meta.modified().ok(), meta.len()))
            .unwrap_or((None, 0))
    })
}

impl ConfigCache {
    pub(crate) fn get(&mut self, cas_dir: &Path) -> Arc<crate::config::Config> {
        let stamp = config_stamp(cas_dir);
        if let Some((cached_stamp, config)) = &self.cached
            && *cached_stamp == stamp
        {
            return Arc::clone(config);
        }
        let config = Arc::new(crate::config::Config::load(cas_dir).unwrap_or_default());
        self.cached = Some((stamp, Arc::clone(&config)));
        config
    }
}

/// How long a worker's transcript resolution (an agent-store read) is reused.
pub(crate) const PROBE_RESOLUTION_TTL: Duration = Duration::from_secs(2);

type FileStamp = (u64, Option<SystemTime>);

struct ProbeEntry<T> {
    resolved_at: Instant,
    transcript: Option<(PathBuf, cas_mux::SupervisorCli)>,
    evidence: Option<(FileStamp, T)>,
}

/// Cached wake-gate transcript probes (spec §4.3 "cached probes"): one result
/// per pane, recomputed only when the transcript file changes. A tool call
/// starting or finishing always appends to the transcript, so an unchanged
/// file (same length and mtime) has unchanged evidence. The pane's
/// transcript path, which costs an agent-store read, is reused for
/// [`PROBE_RESOLUTION_TTL`].
pub(crate) struct TranscriptProbeCache<T: Copy> {
    entries: std::sync::Mutex<HashMap<String, ProbeEntry<T>>>,
}

impl<T: Copy> Default for TranscriptProbeCache<T> {
    fn default() -> Self {
        Self {
            entries: std::sync::Mutex::new(HashMap::new()),
        }
    }
}

impl<T: Copy> TranscriptProbeCache<T> {
    /// `resolve` finds the pane's transcript; `probe` reads it. `None` when
    /// the pane has no resolvable transcript.
    pub(crate) fn probe(
        &self,
        pane: &str,
        now: Instant,
        resolve: impl FnOnce() -> Option<(PathBuf, cas_mux::SupervisorCli)>,
        probe: impl FnOnce(&Path, cas_mux::SupervisorCli) -> T,
    ) -> Option<T> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let fresh_resolution = entries
            .get(pane)
            .is_some_and(|entry| now.saturating_duration_since(entry.resolved_at) < PROBE_RESOLUTION_TTL);
        if !fresh_resolution {
            let transcript = resolve();
            let previous = entries.remove(pane);
            let evidence = previous
                .filter(|entry| entry.transcript == transcript)
                .and_then(|entry| entry.evidence);
            entries.insert(
                pane.to_string(),
                ProbeEntry {
                    resolved_at: now,
                    transcript,
                    evidence,
                },
            );
        }
        let entry = entries.get_mut(pane)?;
        let (path, cli) = entry.transcript.clone()?;
        let stamp = std::fs::metadata(&path)
            .map(|meta| (meta.len(), meta.modified().ok()))
            .unwrap_or((0, None));
        if let Some((cached_stamp, value)) = entry.evidence
            && cached_stamp == stamp
            && stamp.1.is_some()
        {
            return Some(value);
        }
        let value = probe(&path, cli);
        entry.evidence = Some((stamp, value));
        Some(value)
    }
}

/// Whether control may inject a selected row now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admission {
    /// Send it; it is in flight under `generation`.
    Inject { generation: u64 },
    /// An earlier inject of this row has not settled.
    InFlight,
    /// Another channel, or an earlier handoff that keeps its claim, holds it.
    NotClaimed,
}

/// Claims the current selection took, until they are handed off or released.
#[derive(Debug, Default)]
struct PassClaims {
    /// `(row, recipient)` -> handed off (true) or still held (false).
    taken: HashMap<(i64, String), bool>,
    /// What the selection saw for each `(row, recipient)`.
    observed: HashMap<(i64, String), bool>,
}

/// The prompt queue's jobs and the control-side state that orders them.
pub(crate) struct QueueJobs {
    exec: Exec,
    next_generation: u64,
    select_in_flight: bool,
    selection: Option<QueueSelection>,
    /// Set by a finished `QueueSelect` so the loop delivers it without waiting
    /// for its poll timer. Only selections set it: other jobs need no prompt
    /// reaction, and a stamp waking the loop would spin select→deliver.
    ready: Arc<AtomicBool>,
    in_flight: InFlightSet,
    /// Stamps submitted per in-flight generation, so a repeat is not
    /// submitted again. Cleared when the generation settles.
    submitted_stamps: HashSet<(i64, String, u64, String)>,
    claims: PassClaims,
    /// Retractions waiting for their fresh read: recipient -> (pruner, repo).
    retractions: HashMap<String, (InboxPruner, PathBuf)>,
    mirror_in_flight: bool,
    mirror_state: Option<Box<super::MirrorScanState>>,
    config: ConfigCache,
    store: Option<Arc<dyn cas_store::PromptQueueStore>>,
    /// Wake-gate "tool call in flight" probes, per pane.
    pub(crate) tool_call_probes: TranscriptProbeCache<bool>,
    /// Inbox prunes completed (watchdog and tests).
    pub(crate) prunes_done: u64,
}

impl QueueJobs {
    /// The daemon's queue jobs: lanes of their own, writes on the StoreWorker
    /// and reads on a dedicated connection.
    pub(crate) fn start(cas_dir: PathBuf, project_id: Option<String>) -> Self {
        Self::with_exec(Exec::Lanes(JobRuntime::start_with(
            LANE_CAPACITY,
            LaneWiring::daemon(cas_dir, project_id),
        )))
    }

    /// Jobs on explicit wiring (tests: a recording or gated writer).
    #[cfg(test)]
    pub(crate) fn start_with(wiring: LaneWiring) -> Self {
        Self::with_exec(Exec::Lanes(JobRuntime::start_with(LANE_CAPACITY, wiring)))
    }

    /// Every job runs on the submitting thread before `submit` returns.
    #[cfg(test)]
    pub(crate) fn inline() -> Self {
        Self::with_exec(Exec::Inline(VecDeque::new()))
    }

    fn with_exec(exec: Exec) -> Self {
        Self {
            exec,
            next_generation: 0,
            select_in_flight: false,
            selection: None,
            ready: Arc::new(AtomicBool::new(false)),
            in_flight: InFlightSet::default(),
            submitted_stamps: HashSet::new(),
            claims: PassClaims::default(),
            retractions: HashMap::new(),
            mirror_in_flight: false,
            mirror_state: Some(Box::default()),
            config: ConfigCache::default(),
            store: None,
            tool_call_probes: TranscriptProbeCache::default(),
            prunes_done: 0,
        }
    }

    fn generation(&mut self) -> u64 {
        self.next_generation += 1;
        self.next_generation
    }

    /// A finished job is waiting to be applied.
    pub(crate) fn has_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    pub(crate) fn config(&mut self, cas_dir: &Path) -> Arc<crate::config::Config> {
        self.config.get(cas_dir)
    }

    /// The loop's prompt-queue handle, opened once (schema init runs once).
    pub(crate) fn store(&mut self, cas_dir: &Path) -> anyhow::Result<Arc<dyn cas_store::PromptQueueStore>> {
        if let Some(store) = &self.store {
            return Ok(Arc::clone(store));
        }
        let store = crate::store::open_prompt_queue_store(cas_dir)?;
        self.store = Some(Arc::clone(&store));
        Ok(store)
    }

    pub(crate) fn in_flight(&self) -> &InFlightSet {
        &self.in_flight
    }

    pub(crate) fn select_in_flight(&self) -> bool {
        self.select_in_flight
    }

    /// Submit `QueueSelect` unless one is already running.
    pub(crate) fn submit_select(
        &mut self,
        request: QueueSelectRequest,
        select: impl FnOnce(&QueueSelectRequest, u64) -> anyhow::Result<QueueSelection> + Send + 'static,
    ) {
        if self.select_in_flight {
            return;
        }
        let generation = self.generation();
        let ready = Arc::clone(&self.ready);
        let spec = JobSpec {
            kind: "QueueSelect",
            key: QUEUE_SELECT_KEY.to_string(),
            lane: Lane::Write,
            criticality: Criticality::Critical,
            generation,
            budget: QUEUE_JOB_BUDGET,
        };
        let submitted = self.exec.submit(spec, move || {
            let selection = select(&request, generation);
            ready.store(true, Ordering::Release);
            QueueJobOutcome::Selected(selection)
        });
        self.select_in_flight = accepted(&submitted);
    }

    /// Queue `stamp` for `(row, recipient)`. A stamp for an in-flight inject
    /// carries its generation and is submitted once per generation; other
    /// stamps carry generation 0 and are always submitted.
    pub(crate) fn stamp(&mut self, cas_dir: &Path, row: i64, recipient: &str, stamp: QueueStamp) {
        let generation = self.in_flight.generation_of(row, recipient).unwrap_or(0);
        if generation != 0
            && !self
                .submitted_stamps
                .insert((row, recipient.to_string(), generation, format!("{stamp:?}")))
        {
            tracing::debug!(
                target: "cas::coordination",
                message_id = row,
                %recipient,
                generation,
                stamp = stamp.label(),
                "factory loop v2: repeated queue stamp ignored"
            );
            return;
        }
        self.submit_stamp(cas_dir, row, recipient, generation, stamp);
    }

    fn submit_stamp(&mut self, cas_dir: &Path, row: i64, recipient: &str, generation: u64, stamp: QueueStamp) {
        let cas_dir = cas_dir.to_path_buf();
        let recipient = recipient.to_string();
        let spec = JobSpec {
            kind: "QueueStamp",
            key: format!("QueueStamp:{row}:{recipient}"),
            lane: Lane::Write,
            criticality: Criticality::Critical,
            generation,
            budget: QUEUE_JOB_BUDGET,
        };
        self.exec.submit(spec, move || {
            let error = (stamp != QueueStamp::Settled)
                .then(|| apply_stamp_with_retry(&cas_dir, row, &recipient, &stamp))
                .flatten();
            QueueJobOutcome::Stamped(StampDone {
                row,
                recipient,
                generation,
                stamp: stamp.label(),
                error,
            })
        });
    }

    /// Decide whether control may inject `(row, recipient)` now (spec §4.5
    /// rules 1–2): not while an earlier inject of the row is in flight, and
    /// only under a transport claim. `claim` takes one when the selection did
    /// not consider this recipient. On `Inject` the row is in flight from
    /// `now`.
    pub(crate) fn admit(
        &mut self,
        row: i64,
        recipient: &str,
        now: Instant,
        claim: impl FnOnce() -> anyhow::Result<bool>,
    ) -> anyhow::Result<Admission> {
        if self.in_flight.holds_row(row) {
            return Ok(Admission::InFlight);
        }
        let claimed = match self.use_claim(row, recipient) {
            Some(claimed) => claimed,
            None => claim()?,
        };
        if !claimed {
            return Ok(Admission::NotClaimed);
        }
        Ok(Admission::Inject {
            generation: self.begin_inject(row, recipient, now),
        })
    }

    /// Record an inject about to be sent; returns its generation.
    pub(crate) fn begin_inject(&mut self, row: i64, recipient: &str, now: Instant) -> u64 {
        let generation = self.generation();
        self.in_flight.begin(row, recipient, generation, now);
        generation
    }

    /// The inject's outcome arrived (inline today; an io-lane event in C4a).
    pub(crate) fn record_outcome(&mut self, row: i64, recipient: &str, generation: u64) {
        self.in_flight.record_outcome(row, recipient, generation);
    }

    /// Queue the settling stamp for every inject whose outcome is in. It runs
    /// after that row's earlier stamps (same key), and its completion clears
    /// the in-flight entry.
    pub(crate) fn settle_injects(&mut self, cas_dir: &Path) {
        for (row, recipient, generation) in self.in_flight.take_unsettled() {
            self.submit_stamp(cas_dir, row, &recipient, generation, QueueStamp::Settled);
        }
    }

    /// Release the claim of every inject with no outcome after the timeout.
    pub(crate) fn expire_injects(&mut self, cas_dir: &Path, now: Instant) -> usize {
        let expired = self.in_flight.expire(now, INJECT_OUTCOME_TIMEOUT);
        for (row, recipient, generation) in &expired {
            tracing::warn!(
                target: "cas::coordination",
                stage = "inject_outcome_timeout",
                message_id = *row,
                %recipient,
                generation = *generation,
                "factory loop v2: no inject outcome in time; releasing the transport claim so the row retries"
            );
            self.submit_stamp(cas_dir, *row, recipient, *generation, QueueStamp::ReleaseClaim);
        }
        expired.len()
    }

    /// Start tracking `selection`'s claims for this pass.
    pub(crate) fn open_pass_claims(&mut self, rows: &[cas_store::SelectedPrompt]) {
        for selected in rows {
            for (recipient, claimed) in &selected.claims {
                let key = (selected.prompt.id, recipient.clone());
                self.claims.observed.insert(key.clone(), *claimed);
                if *claimed {
                    self.claims.taken.insert(key, false);
                }
            }
        }
    }

    /// The selection's claim for `(row, recipient)`, marked handed off.
    /// `None` when the selection did not consider this recipient.
    pub(crate) fn use_claim(&mut self, row: i64, recipient: &str) -> Option<bool> {
        let key = (row, recipient.to_string());
        let claimed = *self.claims.observed.get(&key)?;
        if let Some(used) = self.claims.taken.get_mut(&key) {
            *used = true;
        }
        Some(claimed)
    }

    /// Release every claim the pass took and did not hand off. Runs at the
    /// end of each pass, and at the start of the next in case a pass returned
    /// early.
    pub(crate) fn close_pass_claims(&mut self, cas_dir: &Path) -> usize {
        let claims = std::mem::take(&mut self.claims);
        let mut released = 0;
        for ((row, recipient), used) in claims.taken {
            if !used {
                released += 1;
                self.submit_stamp(cas_dir, row, &recipient, 0, QueueStamp::ReleaseClaim);
            }
        }
        released
    }

    /// Retract stale alerts from `recipient`'s inbox. `fresh` is a read
    /// issued for this decision; the cached director snapshot is never used
    /// (app/mod.rs:1541–1547).
    pub(crate) fn submit_inbox_retraction(
        &mut self,
        recipient: &str,
        pruner: InboxPruner,
        repo_root: PathBuf,
        fresh: FreshRead,
    ) {
        if self.retractions.contains_key(recipient) {
            return;
        }
        let generation = self.generation();
        let owned = recipient.to_string();
        let spec = JobSpec {
            kind: "RevalidateFresh",
            key: format!("RevalidateFresh:inbox:{recipient}"),
            lane: Lane::Read,
            criticality: Criticality::Refresh,
            generation,
            budget: REVALIDATE_FRESH_BUDGET,
        };
        let submitted = self.exec.submit_read(spec, move |stores| {
            let data = fresh(stores);
            QueueJobOutcome::Revalidated {
                recipient: owned,
                data,
            }
        });
        if accepted(&submitted) {
            self.retractions
                .insert(recipient.to_string(), (pruner, repo_root));
        }
    }

    fn submit_inbox_prune(&mut self, recipient: String, data: DirectorData) {
        let Some((pruner, repo_root)) = self.retractions.remove(&recipient) else {
            return;
        };
        let generation = self.generation();
        let spec = JobSpec {
            kind: "InboxPrune",
            key: format!("InboxPrune:{recipient}"),
            lane: Lane::Blocking,
            criticality: Criticality::Refresh,
            generation,
            budget: INBOX_PRUNE_BUDGET,
        };
        self.exec.submit(spec, move || {
            let report = prune_inbox_from_fresh(&pruner, &recipient, &data, &repo_root);
            QueueJobOutcome::Pruned(report)
        });
    }

    /// Run the Commander mirror scan as a job unless one is running.
    pub(crate) fn submit_mirror_scan(
        &mut self,
        scan: impl FnOnce(&mut super::MirrorScanState) + Send + 'static,
    ) {
        if self.mirror_in_flight {
            return;
        }
        let Some(mut state) = self.mirror_state.take() else {
            return;
        };
        let generation = self.generation();
        let spec = JobSpec {
            kind: "MirrorWrite",
            key: MIRROR_WRITE_KEY.to_string(),
            lane: Lane::Write,
            criticality: Criticality::Refresh,
            generation,
            budget: MIRROR_WRITE_BUDGET,
        };
        let submitted = self.exec.submit(spec, move || {
            scan(&mut state);
            QueueJobOutcome::Mirrored(state)
        });
        if accepted(&submitted) {
            self.mirror_in_flight = true;
        } else {
            // A refused or dropped job never runs; start over with fresh
            // cursors rather than lose the slot.
            self.mirror_state = Some(Box::default());
        }
    }

    /// Apply every finished job: stamps settle in-flight rows, a selection
    /// waits for [`Self::take_selection`], a fresh read starts its prune.
    pub(crate) fn apply_completions(&mut self) {
        self.ready.store(false, Ordering::Release);
        for result in self.exec.drain() {
            let outcome = match result.outcome {
                Ok(outcome) => outcome,
                Err(panicked) => {
                    tracing::error!(
                        target: "cas::coordination",
                        kind = result.kind,
                        key = %result.key,
                        message = %panicked.message,
                        "factory loop v2: prompt-queue job panicked"
                    );
                    match result.kind {
                        "QueueSelect" => self.select_in_flight = false,
                        "MirrorWrite" => {
                            self.mirror_in_flight = false;
                            self.mirror_state = Some(Box::default());
                        }
                        "RevalidateFresh" => {
                            if let Some(recipient) = result.key.strip_prefix("RevalidateFresh:inbox:") {
                                self.retractions.remove(recipient);
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
            };
            match outcome {
                QueueJobOutcome::Selected(selection) => {
                    self.select_in_flight = false;
                    match selection {
                        Ok(selection) => self.selection = Some(selection),
                        Err(error) => tracing::warn!(
                            target: "cas::coordination",
                            %error,
                            "factory loop v2: prompt-queue selection failed; retrying next tick"
                        ),
                    }
                }
                QueueJobOutcome::Stamped(done) => {
                    if let Some(error) = &done.error {
                        tracing::error!(
                            target: "cas::coordination",
                            message_id = done.row,
                            recipient = %done.recipient,
                            stamp = done.stamp,
                            %error,
                            "factory loop v2: queue stamp failed after retries"
                        );
                    }
                    if done.stamp == "settled" {
                        self.in_flight.settle(done.row, &done.recipient, done.generation);
                        self.submitted_stamps.retain(|(row, recipient, generation, _)| {
                            !(*row == done.row && *recipient == done.recipient && *generation == done.generation)
                        });
                    }
                }
                QueueJobOutcome::Revalidated { recipient, data } => match data {
                    Ok(data) => self.submit_inbox_prune(recipient, data),
                    Err(error) => {
                        // Missing state is uncertainty: keep every row.
                        tracing::debug!(
                            target: "cas::coordination",
                            %recipient,
                            %error,
                            "inbox retraction skipped: the fresh read failed"
                        );
                        self.retractions.remove(&recipient);
                    }
                },
                QueueJobOutcome::Pruned(report) => {
                    self.prunes_done += 1;
                    log_prune(&report);
                }
                QueueJobOutcome::Mirrored(state) => {
                    self.mirror_in_flight = false;
                    self.mirror_state = Some(state);
                }
            }
        }
    }

    /// Close the lanes and wait up to `timeout` for queued critical work
    /// (stamps) to finish. Call before the StoreWorker drains (jobs.rs).
    pub(crate) fn drain_for_shutdown(&mut self, cas_dir: &Path, timeout: Duration) -> bool {
        self.close_pass_claims(cas_dir);
        self.settle_injects(cas_dir);
        match std::mem::replace(&mut self.exec, Exec::Inline(VecDeque::new())) {
            Exec::Lanes(runtime) => runtime.drain(timeout),
            Exec::Inline(_) => true,
        }
    }

    /// The finished selection, once.
    pub(crate) fn take_selection(&mut self) -> Option<QueueSelection> {
        self.selection.take()
    }
}

fn accepted(submitted: &Submitted) -> bool {
    matches!(
        submitted,
        Submitted::Queued
            | Submitted::Coalesced
            | Submitted::QueuedBehindKey
            | Submitted::QueuedDroppedOldest { .. }
    )
}

/// Run one stamp, retrying a busy store a few times on the writer. Returns
/// the last error when every attempt failed.
fn apply_stamp_with_retry(cas_dir: &Path, row: i64, recipient: &str, stamp: &QueueStamp) -> Option<String> {
    let mut pause = Duration::from_millis(100);
    let mut last = None;
    for attempt in 1..=4 {
        let result = crate::store::open_prompt_queue_store(cas_dir)
            .map_err(|error| error.to_string())
            .and_then(|queue| stamp.apply(queue.as_ref(), row, recipient).map_err(|error| error.to_string()));
        match result {
            Ok(()) => return None,
            Err(error) => {
                if attempt < 4 {
                    tracing::debug!(message_id = row, stamp = stamp.label(), %error, "queue stamp failed; retrying");
                    std::thread::sleep(pause);
                    pause *= 2;
                }
                last = Some(error);
            }
        }
    }
    last
}

/// Decide which unread alerts in `recipient`'s inbox are stale from `fresh`,
/// then remove them under the inbox lock. Freshness checks (git for merge
/// alerts) run before the lock is taken.
pub(crate) fn prune_inbox_from_fresh(
    pruner: &InboxPruner,
    recipient: &str,
    fresh: &DirectorData,
    repo_root: &Path,
) -> PruneReport {
    let keys = pruner.unread_retraction_keys(recipient);
    let stale = RetractionKeys {
        workers: keys
            .workers
            .into_iter()
            .filter(|worker| crate::ui::factory::director::worker_now_has_real_assignment(fresh, worker))
            .collect(),
        tasks: keys
            .tasks
            .into_iter()
            .filter(|task| {
                matches!(
                    crate::ui::factory::director::check_merge_alert_freshness_for_task(task, fresh, repo_root),
                    crate::ui::factory::director::MergeAlertFreshness::Stale
                )
            })
            .collect(),
        epics: keys
            .epics
            .into_iter()
            .filter(|epic| !crate::ui::factory::director::epic_completion_is_current(fresh, epic))
            .collect(),
    };
    let mut report = PruneReport {
        recipient: recipient.to_string(),
        ..PruneReport::default()
    };
    match pruner.retract(recipient, &stale) {
        Ok((idle, merge, epic)) => {
            report.idle = idle;
            report.merge = merge;
            report.epic = epic;
        }
        Err(error) => report.error = Some(error.to_string()),
    }
    report
}

fn log_prune(report: &PruneReport) {
    if let Some(error) = &report.error {
        tracing::warn!(
            target: "cas::coordination",
            recipient = %report.recipient,
            %error,
            "inbox retraction failed; non-fatal, stale alerts may still be delivered"
        );
        return;
    }
    for (count, stage, what) in [
        (report.idle, "retract_stale_idle_alert", "WorkerIdle"),
        (report.merge, "retract_stale_merge_alert", "MERGE REQUIRED"),
        (report.epic, "retract_stale_epic_completion", "epic-completion"),
    ] {
        if count > 0 {
            tracing::info!(
                target: "cas::coordination",
                stage,
                channel = "teams_inbox",
                retracted = count,
                recipient = %report.recipient,
                "swept stale {what} alert(s) from the inbox before delivery"
            );
        }
    }
}

#[cfg(test)]
#[path = "prompt_queue_jobs_tests.rs"]
mod tests;
