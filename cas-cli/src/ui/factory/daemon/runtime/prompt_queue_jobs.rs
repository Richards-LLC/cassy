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
use super::teams::{AlertIdentity, InboxPruner};
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
    /// Release the transport claim `token` names, which was not handed off.
    /// A late release never drops a newer claim (review round 1, L10).
    ReleaseClaim { claim_token: cas_store::ClaimToken },
    /// The row reached its recipient's transport.
    TransportDelivered,
    /// The recipient's transport receipt (`transport_claimed` surfacing).
    TransportReceipt,
    /// Why the row stays pending.
    PendingReason {
        reason: cas_store::PendingReason,
        detail: Option<String>,
    },
    /// One submitted supervisor turn, persisted off the control loop.
    DirectedWake { cas_dir: PathBuf, recipient: String, senders: Vec<String>, created_at: chrono::DateTime<chrono::Utc>, injected_at: chrono::DateTime<chrono::Utc> },
    /// What the wake nudge did.
    WakeAttempt {
        attempt: cas_store::WakeAttempt,
        detail: Option<String>,
    },
    /// Retry later for `reason` (cas-4272).
    Retry {
        reason: cas_store::PendingReason,
        detail: Option<String>,
    },
    /// Terminal: dropped (cas-4272).
    Dropped { detail: Option<String> },
    /// Terminal: superseded by a newer premise (cas-4272).
    Superseded { detail: String },
    /// Terminal: suppressed (cas-4272).
    Suppressed { detail: Option<String> },
    /// Terminal: abandoned, target unknown (cas-4272).
    Abandoned { detail: Option<String> },
    /// A broadcast's per-recipient outcome (cas-4272).
    BroadcastOutcome {
        attempted: u32,
        succeeded: u32,
        failed: u32,
        detail: Option<String>,
    },
    /// A Teams inbox copy was written while the row stays pending for a
    /// later wake (GH #751, cas-786a6 L4).
    DeferredInbox {
        pane_bytes: u64,
        warn_supervisor: String,
        session: String,
        claim_token: Option<cas_store::ClaimToken>,
    },
    /// One declined wake-gate pass (cas-9e81 detail; cas-786a6 L4). Control
    /// counts declines from the selection snapshot; this persists the one.
    WakeDecline { detail: String },
    /// The strong observed-wake receipt (cas-1a54).
    ObservedWakeReceipt,
    /// Withdraw unread rows for a recipient that shut down (cas-786a6 L4).
    /// `recipient` is the shut-down agent.
    CancelUnreadForShutdown { session: String, reason: String },
    /// Stop waking a wake-starved row; keep it for the recipient's next
    /// turn (cas-d1659; cas-786a6 L4).
    ParkBusy {
        detail: String,
        recheck_at: chrono::DateTime<chrono::Utc>,
    },
    /// Terminal: a lifecycle relay that expired undelivered (cas-7787).
    UndeliveredRelay { notice: String },
    /// cas-6eab/cas-bc8c: tell the requester why its merge request is
    /// withheld, then supersede the row. A guidance enqueue that fails leaves
    /// the row pending, to be decided again (cas-786a6).
    SupersedeWithGuidance {
        source: String,
        session: Option<String>,
        guidance: String,
        summary: String,
        detail: String,
    },
    /// cas-d5ab: the recipient worker was seen working (a wake-gate decline
    /// for a tool call in flight), recorded through cas-7156's per-worker
    /// activity recorder.
    WorkerActivity {
        cas_dir: PathBuf,
        agent_id: String,
        reason: String,
    },
    /// No write: completes after every earlier stamp for the same row and
    /// recipient, and clears the row's in-flight entry.
    Settled,
}

impl QueueStamp {
    fn label(&self) -> &'static str {
        match self {
            Self::ReleaseClaim { .. } => "release_claim",
            Self::TransportDelivered => "transport_delivered",
            Self::TransportReceipt => "transport_receipt",
            Self::PendingReason { .. } => "pending_reason",
            Self::DirectedWake { .. } => "directed_wake",
            Self::WakeAttempt { .. } => "wake_attempt",
            Self::Retry { .. } => "retry",
            Self::Dropped { .. } => "dropped",
            Self::Superseded { .. } => "superseded",
            Self::Suppressed { .. } => "suppressed",
            Self::Abandoned { .. } => "abandoned",
            Self::BroadcastOutcome { .. } => "broadcast_outcome",
            Self::DeferredInbox { .. } => "deferred_inbox",
            Self::WakeDecline { .. } => "wake_decline",
            Self::ObservedWakeReceipt => "observed_wake_receipt",
            Self::CancelUnreadForShutdown { .. } => "cancel_unread_for_shutdown",
            Self::ParkBusy { .. } => "park_busy",
            Self::UndeliveredRelay { .. } => "undelivered_relay",
            Self::SupersedeWithGuidance { .. } => "supersede_with_guidance",
            Self::WorkerActivity { .. } => "worker_activity",
            Self::Settled => "settled",
        }
    }

    pub(crate) fn apply(&self, queue: &dyn cas_store::PromptQueueStore, row: i64, recipient: &str) -> cas_store::Result<()> {
        match self {
            Self::DirectedWake { cas_dir, recipient, senders, created_at, injected_at } =>
                crate::supervisor_message_wakes::record(cas_dir,row,recipient,senders,*created_at,*injected_at)
                    .map_err(|error| cas_store::StoreError::Other(error.to_string())),
            Self::ReleaseClaim { claim_token } => queue
                .release_recipient_transport_claim(row, recipient, *claim_token)
                .map(drop),
            Self::TransportDelivered => queue.mark_transport_delivered(row),
            Self::TransportReceipt => queue.record_recipient_transport_delivered(row, recipient),
            Self::PendingReason { reason, detail } => {
                queue.record_pending_reason(row, *reason, detail.as_deref())
            }
            Self::WakeAttempt { attempt, detail } => {
                queue.record_wake_attempt(row, *attempt, detail.as_deref())
            }
            Self::Retry { reason, detail } => queue
                .record_retry(row, *reason, detail.as_deref())
                .map(drop),
            Self::Dropped { detail } => queue.mark_dropped(row, detail.as_deref()),
            Self::Superseded { detail } => queue.mark_superseded(row, detail),
            Self::Suppressed { detail } => queue.mark_suppressed(row, detail.as_deref()),
            Self::Abandoned { detail } => queue.mark_abandoned(row, detail.as_deref()),
            Self::BroadcastOutcome {
                attempted,
                succeeded,
                failed,
                detail,
            } => queue.mark_broadcast_outcome(row, *attempted, *succeeded, *failed, detail.as_deref()),
            Self::DeferredInbox {
                pane_bytes,
                warn_supervisor,
                session,
                claim_token,
            } => queue.record_deferred_inbox(row, *pane_bytes).inspect_err(|_| {
                // cas-1ba0: the Teams copy survives, but without this marker
                // a tool boundary cannot take the claim. Report the degraded
                // path durably, once per row, rather than only in logs.
                let notice = format!(
                    "Warning: deferred inbox handoff not recorded: notification_id={row}; \
                     target={recipient}. tool-boundary surfacing unavailable; the Teams copy \
                     waits until the recipient's turn boundary. Inspect with \
                     coordination action=message_status notification_id={row}",
                );
                if let Err(notice_error) = queue.enqueue_idempotent(
                    "daemon",
                    warn_supervisor,
                    &notice,
                    Some(session.as_str()),
                    Some("Deferred inbox handoff not recorded"),
                    None,
                    &format!("deferred-inbox-stamp-failed:{row}"),
                    Some(&cas_store::QueueOrigin::Daemon),
                ) {
                    tracing::error!(
                        prompt_id = row,
                        %notice_error,
                        "failed to persist supervisor warning about deferred inbox handoff"
                    );
                }
            }).and_then(|()| {
                // Release only after the durable handoff marker landed. If
                // that write failed, keep the claim so a hook cannot render
                // a second body alongside the surviving Teams copy.
                if let Some(token) = claim_token {
                    queue.release_recipient_transport_claim(row, recipient, *token)?;
                }
                Ok(())
            }),
            Self::WakeDecline { detail } => queue.record_wake_gate_decline(row, detail).map(drop),
            Self::ObservedWakeReceipt => queue.record_recipient_surfaced(
                row,
                recipient,
                cas_store::SurfacingSource::ObservedWake,
            ),
            Self::CancelUnreadForShutdown { session, reason } => queue
                .cancel_unread_for_shutdown(recipient, Some(session), reason)
                .map(drop),
            Self::ParkBusy { detail, recheck_at } => {
                queue.park_for_busy_recipient(row, Some(detail), *recheck_at).map(drop)
            }
            Self::UndeliveredRelay { notice } => queue
                .mark_undelivered_lifecycle_relay(row, Some(notice))
                .map(drop),
            Self::SupersedeWithGuidance {
                source,
                session,
                guidance,
                summary,
                detail,
            } => match queue.enqueue_urgent_with_outcome(
                "supervisor",
                source,
                guidance,
                session.as_deref(),
                Some(summary),
                Some(cas_store::NotificationPriority::High),
                false,
                Some(&cas_store::QueueOrigin::Daemon),
            ) {
                Ok(_) => queue.mark_superseded(row, detail),
                Err(error) => {
                    tracing::warn!(
                        prompt_id = row,
                        %error,
                        "cas-bc8c: worker guidance enqueue failed; retaining original merge request"
                    );
                    Ok(())
                }
            },
            Self::WorkerActivity {
                cas_dir,
                agent_id,
                reason,
            } => cas_factory::director::record_worker_activity(cas_dir, agent_id, "wake_gate", reason)
                .map_err(|error| cas_store::StoreError::Other(error.to_string())),
            Self::Settled => Ok(()),
        }
    }
}

/// How long a row whose stamp failed (and was handed to the StoreWorker)
/// stays out of delivery while the deferred write lands. Review L1: under a
/// persistent store failure the deferred write can take longer than this
/// (six attempts with backoff up to 5 s); the row may then be selected while
/// its write is still retrying. That window is narrower than the pre-C3
/// inline write, which failed in the pass itself, and is accepted.
pub(crate) const SETTLING_DEFERRED_HOLD: Duration = Duration::from_secs(30);

/// A stamp lane with work outstanding but no progress for this long stops
/// gating QueueSelect (review L3), so a wedged writer cannot stop delivery.
pub(crate) const STAMP_STALL_LIMIT: Duration = Duration::from_secs(10);

/// The finished store write for one stamp.
#[derive(Debug)]
pub(crate) struct StampDone {
    pub(crate) row: i64,
    pub(crate) recipient: String,
    pub(crate) generation: u64,
    pub(crate) stamp: &'static str,
    pub(crate) operator_message: Option<crate::ui::factory::ConversationHistoryMessage>,
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
    /// Same-name replacements queued, provisioning or awaiting registration.
    pub(crate) replacement_workers: HashSet<String>,
    pub(crate) limit: usize,
    /// Read the operator replies Commander clients are owed (cas-786a6).
    pub(crate) operator_replies: bool,
    /// Pending replies still waiting for their monotonic retry deadlines.
    pub(crate) deferred_operator_replies: HashSet<i64>,
    /// Adopt owner notices first (cas-5c2c): not while shutting down.
    pub(crate) adopt_operator_notices: bool,
}

/// One delivery batch and the store state it was selected against.
#[derive(Debug, Default)]
pub(crate) struct QueueSelection {
    pub(crate) generation: u64,
    /// When control submitted the selecting job (set when it is taken).
    pub(crate) submitted_at: Option<Instant>,
    /// Every registered agent, read in the selecting job.
    pub(crate) registered_agents: Vec<cas_types::Agent>,
    /// Rows whose QA-rejection target moved; control forgets their state.
    pub(crate) retargeted_rows: Vec<i64>,
    /// Recipients that shut down; their unread rows were withdrawn.
    pub(crate) shut_down_recipients: Vec<String>,
    /// Replacement intent read by selection, retained through control delivery.
    pub(crate) replacement_workers: HashSet<String>,
    /// Session agents served by their native extension, not by the daemon.
    pub(crate) native_agents: HashSet<String>,
    /// The batch, stamped `Selected`. No claims: control claims each row just
    /// before its transport handoff, so an undelivered selection leaves
    /// nothing behind (review round 1, H1).
    pub(crate) rows: Vec<cas_store::QueuedPrompt>,
    /// The store reads each row's delivery decision needs, read in the
    /// selecting job (cas-786a6, H3), so control decides without a store.
    pub(crate) facts: HashMap<i64, super::queue_and_events::RowFacts>,
    /// Operator replies to forward to Commander clients, read and stamped
    /// `Selected` in the selecting job (cas-786a6).
    pub(crate) operator_replies: Vec<super::queue_and_events::OperatorReplyRead>,
}

/// What a `ClaimForInject` job found (cas-786a6, H3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClaimOutcome {
    /// The transport claim was taken: the row may be injected on its next pass.
    Claimed(cas_store::ClaimToken),
    /// The recipient already has the row: a delivered handoff, a hook or a
    /// poll (review M2), or explicitly settled its own copy. This control
    /// result does not create transport or body evidence.
    HandedOff,
    /// A bare transport claim nobody handed off holds the row.
    NotClaimed,
    /// The fresh re-read found the row consumed, gone or re-addressed.
    Stale,
    /// The store failed; the row retries on a later pass.
    Failed(String),
}

/// A finished `ClaimForInject` job.
#[derive(Debug)]
pub(crate) struct ClaimDone {
    pub(crate) row: i64,
    pub(crate) recipient: String,
    /// The claim job's generation: only the job control waits for may
    /// deliver its claim; any other `Claimed` token is released (review M1).
    pub(crate) generation: u64,
    pub(crate) cas_dir: PathBuf,
    pub(crate) outcome: ClaimOutcome,
    /// Rows the QA retarget moved; control forgets their state.
    pub(crate) retargeted: Vec<i64>,
}

#[derive(Debug, Clone)]
enum ClaimState {
    /// A claim job of this generation is out.
    Pending(Instant, u64),
    Ready(ClaimOutcome, Instant),
}

/// What control may do with a row's transport claim this pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimStep {
    /// Claimed: inject now under this token.
    Claimed(cas_store::ClaimToken),
    /// The claim job has not come back; the row waits for a later pass.
    Waiting,
    /// The recipient already has the row (review M2).
    HandedOff,
    /// Not claimed: a bare claim holds the row, or it went stale.
    NotClaimed,
}

/// A claim that came back but whose row was not delivered within this
/// window is released, so hooks and polls can take the row (cas-786a6).
pub(crate) const CLAIM_READY_TTL: Duration = Duration::from_secs(3);

/// The `ClaimForInject` job (cas-786a6, H3; review round 1, H2): on the
/// writer, re-target a QA rejection, re-read the row and take its transport
/// claim for `recipient`. `target` is the row's address as control saw it;
/// a row that moved is stale, not claimed.
pub(crate) fn run_claim_for_inject(
    cas_dir: &Path,
    session: &str,
    row: i64,
    recipient: &str,
    target: &str,
    generation: u64,
) -> ClaimDone {
    let result = (|| -> anyhow::Result<(ClaimOutcome, Vec<i64>)> {
        let queue = job_thread_prompt_queue(cas_dir)?;
        let retargeted = queue.refresh_qa_rejection_targets(session, Some(row))?;
        let Some(fresh) = queue.queued_prompt(row)? else {
            return Ok((ClaimOutcome::Stale, retargeted));
        };
        if fresh.processed_at.is_some() || fresh.target != target {
            return Ok((ClaimOutcome::Stale, retargeted));
        }
        let outcome = match queue.claim_recipient_transport_token(row, recipient)? {
            Some(token) => ClaimOutcome::Claimed(token),
            // An explicit ACK/answer retires this member without claiming any
            // new transport handoff or body observation. In particular, a
            // broadcast ACK before first surfacing must not count as a failed
            // bare claim and consume the broadcast's retry budget.
            None if matches!(queue.recipient_delivery(row, recipient)?,
                Some(cas_store::RecipientDelivery::Settled(_))) => ClaimOutcome::HandedOff,
            None if queue.recipient_handed_off(row, recipient)? => ClaimOutcome::HandedOff,
            None => ClaimOutcome::NotClaimed,
        };
        Ok((outcome, retargeted))
    })();
    let (outcome, retargeted) = result.unwrap_or_else(|error| {
        forget_job_thread_stores(cas_dir);
        (ClaimOutcome::Failed(error.to_string()), Vec::new())
    });
    ClaimDone {
        row,
        recipient: recipient.to_string(),
        generation,
        cas_dir: cas_dir.to_path_buf(),
        outcome,
        retargeted,
    }
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
    /// A `ClaimForInject` job's result (cas-786a6).
    Claimed(ClaimDone),
    Mirrored(Box<super::MirrorScanState>),
    /// Another daemon child's job on the same runtime (factory loop v2 C2):
    /// its own result type, handed back by [`QueueJobs::take_external_completions`].
    External(Box<dyn std::any::Any + Send>),
    /// The lane dropped this refresh job to make room (or the lane was full
    /// of critical work). It never ran; its owner must clear its in-flight
    /// state (C2 review). `external` says which owner hears of it.
    Dropped { external: bool },
}

/// A finished job submitted by another child through
/// [`QueueJobs::submit_external`]. A job that panicked has `outcome` Err.
pub(crate) enum ExternalOutcome {
    /// The job ran and returned this value.
    Done(Box<dyn std::any::Any + Send>),
    /// The lane dropped the job before it ran: clear its in-flight slot.
    Dropped,
}

/// A refresh job just submitted, before the lane says what it did with it.
struct SubmittedRefresh {
    key: String,
    kind: &'static str,
    generation: u64,
    external: bool,
}

/// Who learns a refresh key's results, kept so a drop can be reported to the
/// job's owner rather than only to whoever's submit displaced it.
#[derive(Debug, Clone)]
struct KeyOwner {
    kind: &'static str,
    external: bool,
    /// Generations submitted for this key whose result has not been
    /// collected. Entries are removed by generation, never by position:
    /// a job can finish (result not yet collected) while its rerun is
    /// requeued and then dropped (cas-4272 review M1).
    outstanding: VecDeque<u64>,
}

/// Where queue jobs run. `Inline` runs each job on the submitting thread
/// before `submit` returns: the factory daemon's unit tests use it so one
/// `process_prompt_queue` call still selects, delivers and stamps. The daemon
/// itself always uses `Lanes`.
enum Exec {
    Lanes(Arc<JobRuntime<QueueJobOutcome>>),
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

/// A settling stamp that never completes (its job panicked or the lane
/// refused it) must not hold its row forever (review round 1, M5).
pub(crate) const SETTLE_TIMEOUT: Duration = Duration::from_secs(60);

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

#[derive(Debug, Clone)]
struct InFlight {
    generation: u64,
    /// When the current phase began.
    since: Instant,
    phase: InjectPhase,
    /// The transport claim the inject holds, released by token.
    claim_token: Option<cas_store::ClaimToken>,
}

/// An inject whose outcome never arrived: release `token`'s claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpiredInject {
    pub(crate) row: i64,
    pub(crate) recipient: String,
    pub(crate) generation: u64,
    pub(crate) claim_token: Option<cas_store::ClaimToken>,
}

/// Rows control has injected whose stamps have not landed (spec §4.5 rule 2).
/// Time is passed in, so tests drive it with a fake clock.
#[derive(Debug, Default)]
pub(crate) struct InFlightSet {
    entries: HashMap<(i64, String), InFlight>,
}

impl InFlightSet {
    /// Record an inject about to be sent at `now`.
    pub(crate) fn begin(
        &mut self,
        row: i64,
        recipient: &str,
        generation: u64,
        now: Instant,
        claim_token: Option<cas_store::ClaimToken>,
    ) {
        self.entries.insert(
            (row, recipient.to_string()),
            InFlight {
                generation,
                since: now,
                phase: InjectPhase::AwaitingOutcome,
                claim_token,
            },
        );
    }

    /// The transport claim token an in-flight inject holds.
    pub(crate) fn token_of(&self, row: i64, recipient: &str) -> Option<cas_store::ClaimToken> {
        self.entries
            .get(&(row, recipient.to_string()))
            .and_then(|entry| entry.claim_token)
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
    pub(crate) fn record_outcome(&mut self, row: i64, recipient: &str, generation: u64, now: Instant) {
        if let Some(entry) = self.entries.get_mut(&(row, recipient.to_string()))
            && entry.generation == generation
            && entry.phase == InjectPhase::AwaitingOutcome
        {
            entry.phase = InjectPhase::Stamping;
            entry.since = now;
        }
    }

    /// Entries whose outcome is in and whose settling stamp is not queued yet;
    /// marks them settling.
    fn take_unsettled(&mut self, now: Instant) -> Vec<(i64, String, u64)> {
        self.entries
            .iter_mut()
            .filter(|(_, entry)| entry.phase == InjectPhase::Stamping)
            .map(|((row, recipient), entry)| {
                entry.phase = InjectPhase::Settling;
                entry.since = now;
                (*row, recipient.clone(), entry.generation)
            })
            .collect()
    }

    /// Clear `(row, recipient)` at `generation` whatever its phase: its
    /// settling stamp failed to run.
    fn clear(&mut self, row: i64, recipient: &str, generation: u64) -> bool {
        let key = (row, recipient.to_string());
        if self.entries.get(&key).is_some_and(|entry| entry.generation == generation) {
            self.entries.remove(&key);
            return true;
        }
        false
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
    pub(crate) fn expire(&mut self, now: Instant, timeout: Duration) -> Vec<ExpiredInject> {
        let expired: Vec<ExpiredInject> = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                entry.phase == InjectPhase::AwaitingOutcome
                    && now.saturating_duration_since(entry.since) >= timeout
            })
            .map(|((row, recipient), entry)| ExpiredInject {
                row: *row,
                recipient: recipient.clone(),
                generation: entry.generation,
                claim_token: entry.claim_token,
            })
            .collect();
        for expired in &expired {
            self.entries.remove(&(expired.row, expired.recipient.clone()));
        }
        expired
    }

    /// Remove and return entries whose outcome is in but whose stamps have
    /// not settled after `timeout` (review round 1, M5).
    pub(crate) fn expire_unsettled(&mut self, now: Instant, timeout: Duration) -> Vec<(i64, String, u64)> {
        let stale: Vec<(i64, String, u64)> = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                entry.phase != InjectPhase::AwaitingOutcome
                    && now.saturating_duration_since(entry.since) >= timeout
            })
            .map(|((row, recipient), entry)| (*row, recipient.clone(), entry.generation))
            .collect();
        for (row, recipient, _) in &stale {
            self.entries.remove(&(*row, recipient.clone()));
        }
        stale
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
    #[cfg_attr(not(test), allow(dead_code))]
    NotClaimed,
}

/// The prompt queue's jobs and the control-side state that orders them.
pub(crate) struct QueueJobs {
    /// Commander transport scheduling stays on the existing controller pass.
    pub(crate) operator_reply_retries: HashMap<i64, super::queue_and_events::OperatorReplyRetry>,
    /// Durable ParkBusy rows awaiting an input-ready/turn-end edge. The
    /// existing durable recheck remains the fallback after daemon restart.
    pub(crate) inline_parked: HashMap<i64, (cas_store::QueuedPrompt, chrono::DateTime<chrono::Utc>)>,
    pub(crate) spawn_ingress: super::queue_control_jobs::SpawnIngress,
    pub(crate) prompt_preparation: super::queue_control_jobs::PromptPreparation,
    pub(crate) normal_probe_jobs: super::delivery_probe_jobs::NormalProbeJobs,
    pub(crate) poison_jobs: super::poison_sweep_jobs::PoisonJobs,
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
    /// Retractions waiting for their fresh read: recipient -> (pruner, repo).
    retractions: HashMap<String, (InboxPruner, PathBuf, u64)>,
    /// Finished jobs other children submitted to this runtime, until taken.
    external: Vec<JobResult<ExternalOutcome>>,
    operator_messages: Vec<crate::ui::factory::ConversationHistoryMessage>,
    /// The owner of each refresh key with a result still to come, by key.
    /// A key is forgotten once nothing is outstanding (cas-4272).
    job_owners: HashMap<String, KeyOwner>,
    /// Dropped jobs, reported as results on the next apply.
    dropped: Vec<JobResult<QueueJobOutcome>>,
    mirror_in_flight: bool,
    /// The generation of the mirror scan in flight; an older result is stale.
    mirror_generation: u64,
    mirror_state: Option<Box<super::MirrorScanState>>,
    config: ConfigCache,
    store: Option<Arc<dyn cas_store::PromptQueueStore>>,
    /// Rows with a settling stamp not yet landed, by row: the count of such
    /// stamps outstanding, and a hold deadline once one was deferred.
    settling_rows: HashMap<i64, (usize, Option<Instant>)>,
    /// When a stamp last landed or was deferred.
    last_stamp_progress: Instant,
    /// When each row's latest stamp landed or was deferred, for skipping
    /// rows a selection may have read before their stamp.
    stamp_landed_at: HashMap<i64, Instant>,
    /// Transport claims taken (or being taken) by `ClaimForInject` jobs,
    /// by `(row, recipient)` (cas-786a6, H3).
    claims: HashMap<(i64, String), ClaimState>,
    /// Rows a claim job's QA retarget moved, until control forgets them.
    claim_retargeted: Vec<i64>,
    /// Rows waiting for their claim jobs, with their snapshot facts.
    carried: HashMap<i64, (cas_store::QueuedPrompt, super::queue_and_events::RowFacts)>,
    /// The roster of the last selection, for batches of carried rows.
    last_roster: Option<(Vec<cas_types::Agent>, HashSet<String>)>,
    /// When the current or last QueueSelect was submitted.
    select_submitted_at: Instant,
    /// Mirror results collected, current or stale (tests).
    #[cfg(test)]
    mirror_results_seen: usize,
    /// Inbox prunes completed (watchdog and tests).
    pub(crate) prunes_done: u64,
}

impl QueueJobs {
    /// The daemon's queue jobs: lanes of their own, writes on the StoreWorker
    /// and reads on a dedicated connection.
    pub(crate) fn start(cas_dir: PathBuf, project_id: Option<String>) -> Self {
        Self::with_exec(Exec::Lanes(Arc::new(JobRuntime::start_with(
            LANE_CAPACITY,
            LaneWiring::daemon(cas_dir, project_id),
        ))))
    }

    /// Jobs on explicit wiring (tests: a recording or gated writer).
    #[cfg(test)]
    pub(crate) fn start_with(wiring: LaneWiring) -> Self {
        Self::with_exec(Exec::Lanes(Arc::new(JobRuntime::start_with(LANE_CAPACITY, wiring))))
    }

    /// The daemon's queue jobs with a writer of their own (tests, cas-a8dc).
    /// A daemon process owns the global StoreWorker, but in a test binary
    /// every test daemon shares it, so this daemon's claims and stamps would
    /// queue behind other tests' writes. Each deferred write against a test
    /// directory that has since been deleted retries for about 3.1 s.
    #[cfg(test)]
    pub(crate) fn start_with_private_writer(cas_dir: PathBuf, project_id: Option<String>) -> Self {
        let mut wiring = LaneWiring::daemon(cas_dir, project_id);
        wiring.writer = Arc::new(super::store_worker::StoreWorker::start());
        Self::start_with(wiring)
    }

    /// Jobs on explicit wiring with a small lane capacity (tests: drops).
    #[cfg(test)]
    pub(crate) fn start_with_capacity(wiring: LaneWiring, capacity: usize) -> Self {
        Self::with_exec(Exec::Lanes(Arc::new(JobRuntime::start_with(capacity, wiring))))
    }

    /// Every job runs on the submitting thread before `submit` returns.
    #[cfg(test)]
    pub(crate) fn inline() -> Self {
        Self::with_exec(Exec::Inline(VecDeque::new()))
    }

    fn with_exec(exec: Exec) -> Self {
        Self {
            operator_reply_retries: HashMap::new(),
            inline_parked: HashMap::new(),
            spawn_ingress: Default::default(),
            prompt_preparation: Default::default(),
            normal_probe_jobs: Default::default(),
            poison_jobs: Default::default(),
            exec,
            next_generation: 0,
            select_in_flight: false,
            selection: None,
            ready: Arc::new(AtomicBool::new(false)),
            in_flight: InFlightSet::default(),
            submitted_stamps: HashSet::new(),
            retractions: HashMap::new(),
            external: Vec::new(),
            operator_messages: Vec::new(),
            job_owners: HashMap::new(),
            dropped: Vec::new(),
            mirror_in_flight: false,
            mirror_generation: 0,
            mirror_state: Some(Box::default()),
            config: ConfigCache::default(),
            store: None,
            prunes_done: 0,
            settling_rows: HashMap::new(),
            last_stamp_progress: Instant::now(),
            stamp_landed_at: HashMap::new(),
            claims: HashMap::new(),
            claim_retargeted: Vec::new(),
            carried: HashMap::new(),
            last_roster: None,
            select_submitted_at: Instant::now(),
            #[cfg(test)]
            mirror_results_seen: 0,
        }
    }

    pub(crate) fn generation(&mut self) -> u64 {
        self.next_generation += 1;
        self.next_generation
    }

    /// A finished job is waiting to be applied.
    pub(crate) fn has_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
            || self.selection.is_some()
            || self
                .external
                .iter()
                .any(|result| result.kind == super::queue_control_jobs::WAKE_GATE_READ)
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
        // cas-4272 review L3: a stamp released behind its key goes to the back
        // of the write lane, so a selection submitted now could read a row
        // before that stamp lands. Select only once every stamp has.
        let now = Instant::now();
        if self.stamps_pending(now) {
            return;
        }
        if self.stamps_outstanding() {
            tracing::warn!(
                target: "cas::coordination",
                stage = "stamp_gate_valve",
                stalled_ms = now.saturating_duration_since(self.last_stamp_progress).as_millis() as u64,
                "factory loop v2: stamps made no progress; selecting anyway, and rows whose stamps are still pending are held out of delivery"
            );
        }
        self.select_submitted_at = now;
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
        let submitted = self.submit_job(spec, false, move || {
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

    fn submit_stamp(
        &mut self,
        cas_dir: &Path,
        row: i64,
        recipient: &str,
        generation: u64,
        stamp: QueueStamp,
    ) -> Submitted {
        let cas_dir = cas_dir.to_path_buf();
        let recipient = recipient.to_string();
        let spec = JobSpec {
            // A settle writes nothing and raised no hold; its own kind keeps a
            // panicked settle from lowering a count it never raised (review
            // round 2, item 2). Same key, so it still lands after the row's
            // earlier stamps.
            kind: if stamp == QueueStamp::Settled { "QueueSettle" } else { "QueueStamp" },
            key: format!("QueueStamp:{row}:{recipient}"),
            lane: Lane::Write,
            criticality: Criticality::Critical,
            generation,
            budget: QUEUE_JOB_BUDGET,
        };
        if stamp != QueueStamp::Settled {
            // Review L3: the row is held until every queued stamp for it has
            // landed, and no QueueSelect runs meanwhile.
            self.settling_rows.entry(row).or_insert((0, None)).0 += 1;
        }
        let refused = (stamp != QueueStamp::Settled)
            .then(|| (cas_dir.clone(), recipient.clone(), stamp.clone()));
        let submitted = self.submit_job(spec, false, move || {
            #[cfg(test)]
            if stamp != QueueStamp::Settled && take_injected_stamp_panic(&cas_dir) {
                panic!("injected stamp job panic");
            }
            let error = if stamp == QueueStamp::Settled {
                None
            } else {
                // A stamp records something already done (a handoff, a
                // claim): never drop it. Past its retries, or if it panics,
                // the StoreWorker retries it as `persist_or_defer` did before
                // C3 (review round 2).
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    apply_stamp_with_retry(&cas_dir, row, &recipient, &stamp)
                }))
                .unwrap_or_else(|_| Some("the stamp panicked".to_string()));
                outcome.inspect(|error| {
                    defer_stamp(&cas_dir, row, &recipient, &stamp, error);
                })
            };
            // Publish a verified operator turn only after its transport stamp
            // committed. This read stays on the store lane, never the loop.
            let operator_message = if error.is_none() && stamp == QueueStamp::TransportDelivered {
                job_thread_prompt_queue(&cas_dir).ok().and_then(|queue| {
                    queue.queued_prompt(row).ok().flatten().and_then(|prompt| {
                        super::queue_and_events::commander_history_send(queue.as_ref(), prompt, "")
                    })
                }).filter(|message| message.state == "delivered")
            } else { None };
            QueueJobOutcome::Stamped(StampDone {
                row,
                recipient,
                generation,
                stamp: stamp.label(),
                operator_message,
                error,
            })
        });
        if !accepted(&submitted)
            && let Some((cas_dir, recipient, stamp)) = refused
        {
            // The lane refused it (closed at shutdown): the StoreWorker
            // still takes it, never the loop.
            defer_stamp(&cas_dir, row, &recipient, &stamp, "the queue job lane refused the stamp");
            self.stamp_deferred(row);
        }
        submitted
    }

    /// Decide whether control may inject `(row, recipient)` now (spec §4.5
    /// rule 2): not while an earlier inject of the row is in flight, and only
    /// under a transport claim `claim` takes right now. The claim is the
    /// fresh per-row check (review round 1, H2): it fails for a row a hook,
    /// poll or earlier handoff already holds. On `Inject` the row is in
    /// flight from `now` and holds the claim's token.
    #[cfg(test)]
    pub(crate) fn admit(
        &mut self,
        row: i64,
        recipient: &str,
        now: Instant,
        claim: impl FnOnce() -> anyhow::Result<Option<cas_store::ClaimToken>>,
    ) -> anyhow::Result<Admission> {
        if self.in_flight.holds_row(row) {
            return Ok(Admission::InFlight);
        }
        let Some(claim_token) = claim()? else {
            return Ok(Admission::NotClaimed);
        };
        let generation = self.generation();
        self.in_flight.begin(row, recipient, generation, now, Some(claim_token));
        Ok(Admission::Inject { generation })
    }

    /// The inject's outcome arrived (inline today; an io-lane event in C4a).
    pub(crate) fn record_outcome(&mut self, row: i64, recipient: &str, generation: u64) {
        self.in_flight
            .record_outcome(row, recipient, generation, Instant::now());
    }

    /// Release the transport claim the in-flight inject of `(row, recipient)`
    /// holds, by its token. No-op when there is none.
    pub(crate) fn release_inject_claim(&mut self, cas_dir: &Path, row: i64, recipient: &str) {
        if let Some(claim_token) = self.in_flight.token_of(row, recipient) {
            self.stamp(cas_dir, row, recipient, QueueStamp::ReleaseClaim { claim_token });
        }
    }

    pub(crate) fn inject_claim_token(&self, row: i64, recipient: &str) -> Option<cas_store::ClaimToken> {
        self.in_flight.token_of(row, recipient)
    }

    /// Cancel a prepared batch member if its leader was consumed meanwhile.
    /// Late pending completions follow the existing stale-generation release path.
    pub(crate) fn cancel_unadmitted_claim(&mut self, cas_dir: &Path, row: i64, recipient: &str) {
        if let Some(ClaimState::Ready(ClaimOutcome::Claimed(claim_token),_)) = self.claims.remove(&(row,recipient.to_string())) {
            self.submit_stamp(cas_dir,row,recipient,0,QueueStamp::ReleaseClaim { claim_token });
        }
    }

    /// Queue the settling stamp for every inject whose outcome is in. It runs
    /// after that row's earlier stamps (same key), and its completion clears
    /// the in-flight entry.
    pub(crate) fn settle_injects(&mut self, cas_dir: &Path) {
        for (row, recipient, generation) in self.in_flight.take_unsettled(Instant::now()) {
            let submitted =
                self.submit_stamp(cas_dir, row, &recipient, generation, QueueStamp::Settled);
            if !accepted(&submitted) {
                // M5: a settle that cannot run must not hold the row.
                self.forget_generation(row, &recipient, generation);
            }
        }
    }

    fn forget_generation(&mut self, row: i64, recipient: &str, generation: u64) {
        self.in_flight.clear(row, recipient, generation);
        self.submitted_stamps.retain(|(id, to, stamped, _)| {
            !(*id == row && to == recipient && *stamped == generation)
        });
    }

    /// A mux command with a live write receipt is not an orphan. Preserve its
    /// generation/claim while the writer is blocked, without re-typing bytes.
    pub(crate) fn retain_owned_injects(&mut self, now: Instant, owns: impl Fn(i64) -> bool) {
        for ((row, _), entry) in &mut self.in_flight.entries {
            if entry.phase == InjectPhase::AwaitingOutcome && owns(*row) {
                entry.since = now;
            }
        }
    }

    /// Release the claim of every inject with no outcome after the timeout.
    pub(crate) fn expire_injects(&mut self, cas_dir: &Path, now: Instant) -> usize {
        let expired = self.in_flight.expire(now, INJECT_OUTCOME_TIMEOUT);
        for inject in &expired {
            tracing::warn!(
                target: "cas::coordination",
                stage = "inject_outcome_timeout",
                message_id = inject.row,
                recipient = %inject.recipient,
                generation = inject.generation,
                "factory loop v2: no inject outcome in time; releasing the transport claim so the row retries"
            );
            if let Some(claim_token) = inject.claim_token {
                self.submit_stamp(
                    cas_dir,
                    inject.row,
                    &inject.recipient,
                    inject.generation,
                    QueueStamp::ReleaseClaim { claim_token },
                );
            }
        }
        for (row, recipient, generation) in self.in_flight.expire_unsettled(now, SETTLE_TIMEOUT) {
            tracing::warn!(
                target: "cas::coordination",
                stage = "inject_settle_timeout",
                message_id = row,
                %recipient,
                generation,
                "factory loop v2: an inject's stamps did not settle in time; the row is no longer held"
            );
            self.forget_generation(row, &recipient, generation);
        }
        expired.len()
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
        let submitted = self.submit_job_read(spec, false, move |stores| {
            let data = fresh(stores);
            QueueJobOutcome::Revalidated {
                recipient: owned,
                data,
            }
        });
        if accepted(&submitted) {
            self.retractions
                .insert(recipient.to_string(), (pruner, repo_root, generation));
        }
    }

    fn submit_inbox_prune(&mut self, recipient: String, data: DirectorData) {
        let Some((pruner, repo_root, _)) = self.retractions.remove(&recipient) else {
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
        self.submit_job(spec, false, move || {
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
        let submitted = self.submit_job(spec, false, move || {
            scan(&mut state);
            QueueJobOutcome::Mirrored(state)
        });
        if accepted(&submitted) {
            self.mirror_in_flight = true;
            self.mirror_generation = generation;
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
        let mut finished = std::mem::take(&mut self.dropped);
        let ran = self.exec.drain();
        // A job that ran (or panicked) settles its key's oldest generation;
        // a synthetic drop report was settled when it was made.
        for result in &ran {
            self.finish_key(&result.key, result.generation);
        }
        finished.extend(ran);
        for result in finished {
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
                        "MirrorWrite" => self.mirror_failed(result.generation),
                        "RevalidateFresh" if result.key.starts_with("RevalidateFresh:inbox:") => {
                            if let Some(recipient) = result.key.strip_prefix("RevalidateFresh:inbox:") {
                                self.retraction_failed(recipient, result.generation);
                            }
                        }
                        "QueueStamp" => {
                            // M5: a stamp job that panicked never settles its
                            // row; stop holding it.
                            if let Some((row, recipient)) = result
                                .key
                                .strip_prefix("QueueStamp:")
                                .and_then(|rest| rest.split_once(':'))
                                && let Ok(row) = row.parse::<i64>()
                            {
                                // Review L2: its write may not have landed;
                                // hold the row for the deferral window.
                                self.stamp_deferred(row);
                                if result.generation != 0 {
                                    self.forget_generation(row, recipient, result.generation);
                                }
                            }
                        }
                        "QueueSettle" => {
                            if let Some((row, recipient)) = result
                                .key
                                .strip_prefix("QueueStamp:")
                                .and_then(|rest| rest.split_once(':'))
                                && let Ok(row) = row.parse::<i64>()
                            {
                                self.forget_generation(row, recipient, result.generation);
                            }
                        }
                        "InboxPrune" => {}
                        "ClaimForInject" => {
                            // It may not have claimed; the row retries.
                            if let Some((row, recipient)) = result
                                .key
                                .strip_prefix("ClaimForInject:")
                                .and_then(|rest| rest.split_once(':'))
                                && let Ok(row) = row.parse::<i64>()
                            {
                                self.claims.remove(&(row, recipient.to_string()));
                            }
                        }
                        _ => self.external.push(JobResult {
                            kind: result.kind,
                            key: result.key,
                            generation: result.generation,
                            outcome: Err(panicked),
                            elapsed: result.elapsed,
                            writer_wait: result.writer_wait,
                            over_budget: result.over_budget,
                        }),
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
                    self.stamp_landed(&done);
                    if let Some(message) = done.operator_message {
                        self.operator_messages.push(message);
                    }
                    if let Some(error) = &done.error {
                        tracing::warn!(
                            target: "cas::coordination",
                            message_id = done.row,
                            recipient = %done.recipient,
                            stamp = done.stamp,
                            %error,
                            "factory loop v2: queue stamp failed its retries; handed to the store worker"
                        );
                    }
                    if done.stamp == "settled" {
                        self.in_flight.settle(done.row, &done.recipient, done.generation);
                        self.submitted_stamps.retain(|(row, recipient, generation, _)| {
                            !(*row == done.row && *recipient == done.recipient && *generation == done.generation)
                        });
                    }
                }
                QueueJobOutcome::Revalidated { recipient, data } if !self
                    .retraction_is_current(&recipient, result.generation) =>
                {
                    // A newer retraction owns the slot; this read is stale.
                    drop(data);
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
                        self.retraction_failed(&recipient, result.generation);
                    }
                },
                QueueJobOutcome::Pruned(report) => {
                    self.prunes_done += 1;
                    log_prune(&report);
                }
                QueueJobOutcome::Claimed(done) => {
                    self.claim_retargeted.extend(done.retargeted);
                    let key = (done.row, done.recipient);
                    let awaited = matches!(
                        self.claims.get(&key),
                        Some(ClaimState::Pending(_, generation)) if *generation == done.generation
                    );
                    if awaited {
                        // A claim nobody takes is released by `expire_claims`.
                        self.claims
                            .insert(key, ClaimState::Ready(done.outcome, Instant::now()));
                    } else if let ClaimOutcome::Claimed(claim_token) = done.outcome {
                        // Review M1: a superseded or expired job's claim is
                        // never left holding the row.
                        tracing::debug!(
                            target: "cas::coordination",
                            message_id = key.0,
                            recipient = %key.1,
                            generation = done.generation,
                            "factory loop v2: releasing a claim nobody waits for"
                        );
                        self.submit_stamp(
                            &done.cas_dir,
                            key.0,
                            &key.1,
                            0,
                            QueueStamp::ReleaseClaim { claim_token },
                        );
                    }
                }
                QueueJobOutcome::Mirrored(mut state) => {
                    #[cfg(test)]
                    {
                        self.mirror_results_seen += 1;
                    }
                    // cas-4272: only the scan in flight hands its cursors
                    // back; a scan whose drop was already reported is stale.
                    if self.mirror_in_flight && result.generation == self.mirror_generation {
                        self.mirror_in_flight = false;
                        self.operator_messages.append(&mut state.operator_messages);
                        self.mirror_state = Some(state);
                    }
                }
                QueueJobOutcome::Dropped { external } => {
                    tracing::debug!(
                        target: "cas::coordination",
                        kind = result.kind,
                        key = %result.key,
                        "factory loop v2: a full lane dropped a queued refresh job"
                    );
                    if external {
                        self.external.push(JobResult {
                            kind: result.kind,
                            key: result.key,
                            generation: result.generation,
                            outcome: Ok(ExternalOutcome::Dropped),
                            elapsed: Duration::ZERO,
                            writer_wait: Duration::ZERO,
                            over_budget: false,
                        });
                        continue;
                    }
                    match result.kind {
                        "RevalidateFresh" => {
                            if let Some(recipient) = result.key.strip_prefix("RevalidateFresh:inbox:") {
                                self.retraction_failed(recipient, result.generation);
                            }
                        }
                        // Its cursor state went with the dropped job.
                        "MirrorWrite" => self.mirror_failed(result.generation),
                        _ => {}
                    }
                }
                QueueJobOutcome::External(value) => self.external.push(JobResult {
                    kind: result.kind,
                    key: result.key,
                    generation: result.generation,
                    outcome: Ok(ExternalOutcome::Done(value)),
                    elapsed: result.elapsed,
                    writer_wait: result.writer_wait,
                    over_budget: result.over_budget,
                }),
            }
        }
        // Review L2: forget rows with no stamp outstanding whose hold expired.
        let now = Instant::now();
        self.settling_rows
            .retain(|_, (count, until)| *count > 0 || until.is_some_and(|until| until > now));
        self.stamp_landed_at
            .retain(|_, landed| now.saturating_duration_since(*landed) < Duration::from_secs(60));
        // A selection not yet delivered keeps the loop's fast path armed.
        self.ready.store(self.selection.is_some(), Ordering::Release);
    }

    fn submit_job(
        &mut self,
        spec: JobSpec,
        external: bool,
        work: impl FnOnce() -> QueueJobOutcome + Send + 'static,
    ) -> Submitted {
        let owner = self.owner_of(&spec, external);
        let submitted = self.exec.submit(spec, work);
        self.track(owner, &submitted);
        submitted
    }

    fn submit_job_read(
        &mut self,
        spec: JobSpec,
        external: bool,
        work: impl FnOnce(Option<&DirectorStores>) -> QueueJobOutcome + Send + 'static,
    ) -> Submitted {
        let owner = self.owner_of(&spec, external);
        let submitted = self.exec.submit_read(spec, work);
        self.track(owner, &submitted);
        submitted
    }

    fn owner_of(&self, spec: &JobSpec, external: bool) -> Option<SubmittedRefresh> {
        // Merge-alert jobs are critical, but their completion owner must also
        // survive shutdown until native delivery and fallback checks settle.
        (spec.criticality == Criticality::Refresh
            || (external && spec.kind == super::factory_jobs::MERGE_ALERT_INBOX))
        .then(|| SubmittedRefresh {
            key: spec.key.clone(),
            kind: spec.kind,
            generation: spec.generation,
            external,
        })
    }

    /// Record a refresh job's owner and generation, and report every job a
    /// submit dropped to that job's owner, with that job's own generation
    /// (C2 review; cas-4272): the queued job a full lane displaced, or the
    /// submitted one itself when the lane was full of critical work.
    fn track(&mut self, refresh: Option<SubmittedRefresh>, submitted: &Submitted) {
        let Some(refresh) = refresh else {
            return;
        };
        match submitted {
            Submitted::Queued | Submitted::QueuedBehindKey | Submitted::QueuedDroppedOldest { .. } => {
                self.owner_entry(&refresh).outstanding.push_back(refresh.generation);
            }
            Submitted::Coalesced { replaced_generation } => {
                // The newest refresh replaces the earlier waiting rerun it
                // names, which will never run (review M1: by generation,
                // never by position).
                let owner = self.owner_entry(&refresh);
                if let Some(replaced) = replaced_generation {
                    owner.outstanding.retain(|generation| generation != replaced);
                }
                owner.outstanding.push_back(refresh.generation);
            }
            Submitted::DroppedRefresh => {
                self.report_dropped(refresh.key, refresh.kind, refresh.generation, refresh.external);
            }
            Submitted::Closed
            | Submitted::LaneMismatch { .. } => {}
        }
        if let Submitted::QueuedDroppedOldest {
            dropped_key,
            dropped_generation,
            key_still_queued,
        } = submitted
            && let Some(owner) = self.job_owners.get_mut(dropped_key)
        {
            // Remove exactly the dropped generation: a finished job whose
            // result is not collected yet stays outstanding until it is.
            owner.outstanding.retain(|generation| generation != dropped_generation);
            let (kind, external) = (owner.kind, owner.external);
            if owner.outstanding.is_empty() {
                self.job_owners.remove(dropped_key);
            }
            // Work requeued on the key still runs and reports; only a key
            // with nothing left to run is reported Dropped (review 1).
            if !key_still_queued {
                self.report_dropped(dropped_key.clone(), kind, *dropped_generation, external);
            }
        }
    }

    fn owner_entry(&mut self, refresh: &SubmittedRefresh) -> &mut KeyOwner {
        self.job_owners
            .entry(refresh.key.clone())
            .or_insert_with(|| KeyOwner {
                kind: refresh.kind,
                external: refresh.external,
                outstanding: VecDeque::new(),
            })
    }

    fn report_dropped(&mut self, key: String, kind: &'static str, generation: u64, external: bool) {
        self.dropped.push(JobResult {
            kind,
            key,
            generation,
            outcome: Ok(QueueJobOutcome::Dropped { external }),
            elapsed: Duration::ZERO,
            writer_wait: Duration::ZERO,
            over_budget: false,
        });
    }

    /// A refresh key's job finished (or panicked): that generation is done. The key is forgotten when nothing is left.
    fn finish_key(&mut self, key: &str, generation: u64) {
        if let Some(owner) = self.job_owners.get_mut(key) {
            owner.outstanding.retain(|outstanding| *outstanding != generation);
            if owner.outstanding.is_empty() {
                self.job_owners.remove(key);
            }
        }
    }

    /// Submit another child's job (factory loop v2 C2) to the daemon's one
    /// job runtime, so the watchdog sees every lane in one registry. Its
    /// result comes back, typed by the caller, from
    /// [`Self::take_external_completions`] after [`Self::apply_completions`].
    /// Use kinds and keys distinct from this module's (`Queue*`,
    /// `RevalidateFresh:inbox:*`, `InboxPrune:*`, `MirrorWrite`).
    pub(crate) fn submit_external(
        &mut self,
        spec: JobSpec,
        work: impl FnOnce() -> Box<dyn std::any::Any + Send> + Send + 'static,
    ) -> Submitted {
        self.submit_job(spec, true, move || QueueJobOutcome::External(work()))
    }

    /// [`Self::submit_external`] on the read lane, with its dedicated
    /// `DirectorStores` connection (`None` inline or when it could not open).
    pub(crate) fn submit_external_read(
        &mut self,
        spec: JobSpec,
        work: impl FnOnce(Option<&DirectorStores>) -> Box<dyn std::any::Any + Send> + Send + 'static,
    ) -> Submitted {
        self.submit_job_read(spec, true, move |stores| QueueJobOutcome::External(work(stores)))
    }

    /// Completion owners must remain alive until their critical jobs settle.
    pub(crate) fn external_kind_pending(&self, kind: &str) -> bool {
        self.job_owners.values().any(|owner| {
            owner.external && owner.kind == kind && !owner.outstanding.is_empty()
        })
    }

    /// Accepted operator sends whose actual transport stamp just committed.
    pub(crate) fn take_operator_messages(&mut self) -> Vec<crate::ui::factory::ConversationHistoryMessage> {
        std::mem::take(&mut self.operator_messages)
    }

    /// Finished external jobs, including ones that panicked (`outcome` Err).
    pub(crate) fn take_external_completions(&mut self) -> Vec<JobResult<ExternalOutcome>> {
        std::mem::take(&mut self.external)
    }

    /// Finished external jobs applied but not yet taken (tests: wait for a
    /// whole batch to finish without taking it).
    #[cfg(test)]
    pub(crate) fn external_completions_ready(&mut self) -> usize {
        self.apply_completions();
        self.external.len()
    }

    /// The finished external jobs whose kind matches `owns`, leaving the
    /// rest for their own owner (cas-4491f: the relays and the director
    /// refresh share this runtime).
    pub(crate) fn take_external_completions_where(
        &mut self,
        owns: impl Fn(&str) -> bool,
    ) -> Vec<JobResult<ExternalOutcome>> {
        let (taken, kept) = std::mem::take(&mut self.external)
            .into_iter()
            .partition(|result| owns(result.kind));
        self.external = kept;
        taken
    }

    /// Close the lanes and wait up to `timeout` for queued critical work
    /// (stamps) to finish. Call before the StoreWorker drains (jobs.rs).
    pub(crate) fn drain_for_shutdown(&mut self, cas_dir: &Path, timeout: Duration) -> bool {
        self.settle_injects(cas_dir);
        let mut ingress = std::mem::take(&mut self.spawn_ingress);
        ingress.cancel(self, "Factory shutdown", true);
        self.spawn_ingress = ingress;
        match std::mem::replace(&mut self.exec, Exec::Inline(VecDeque::new())) {
            Exec::Lanes(runtime) => match Arc::try_unwrap(runtime) {
                Ok(runtime) => runtime.drain(timeout),
                // The watchdog still holds the lanes: stop taking work. Lanes
                // still run every queued critical stamp before they exit.
                Err(shared) => {
                    shared.close();
                    let deadline = Instant::now() + timeout;
                    while Instant::now() < deadline
                        && [Lane::Read, Lane::Write, Lane::Blocking, Lane::External]
                            .into_iter()
                            .any(|lane| {
                                let stats = shared.lane_stats(lane);
                                stats.running.is_some() || stats.critical_backlog > 0
                            })
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Instant::now() < deadline
                }
            },
            Exec::Inline(_) => true,
        }
    }

    /// Stop the lanes taking work, as at shutdown (tests).
    #[cfg(test)]
    pub(crate) fn close_lanes_for_test(&self) {
        if let Exec::Lanes(runtime) = &self.exec {
            runtime.close();
        }
    }

    /// The lanes, for the loop watchdog (`loop.json` and wedge reports).
    /// `None` inline.
    pub(crate) fn job_lanes(&self) -> Option<Arc<dyn super::loop_watchdog::JobLanes>> {
        match &self.exec {
            Exec::Lanes(runtime) => {
                Some(Arc::clone(runtime) as Arc<dyn super::loop_watchdog::JobLanes>)
            }
            Exec::Inline(_) => None,
        }
    }

    fn retraction_is_current(&self, recipient: &str, generation: u64) -> bool {
        self.retractions
            .get(recipient)
            .is_some_and(|(_, _, current)| *current == generation)
    }

    /// The retraction read of `generation` failed, panicked or was dropped:
    /// free the slot, unless a newer retraction already holds it.
    fn retraction_failed(&mut self, recipient: &str, generation: u64) {
        if self.retraction_is_current(recipient, generation) {
            self.retractions.remove(recipient);
        }
    }

    /// The mirror scan of `generation` failed, panicked or was dropped: its
    /// cursor state is gone, so start over with fresh cursors. A stale
    /// report for an older scan changes nothing.
    fn mirror_failed(&mut self, generation: u64) {
        if self.mirror_in_flight && generation == self.mirror_generation {
            self.mirror_in_flight = false;
            self.mirror_state = Some(Box::default());
        }
    }

    /// Whether `row` has a settling stamp (a terminal mark or a delivered
    /// stamp) not yet landed, so delivery must skip it this pass.
    pub(crate) fn row_settling(&mut self, row: i64, now: Instant) -> bool {
        match self.settling_rows.get(&row) {
            None => false,
            Some((0, Some(until))) if now >= *until => {
                self.settling_rows.remove(&row);
                false
            }
            Some(_) => true,
        }
    }

    fn stamp_landed(&mut self, done: &StampDone) {
        if done.stamp == "settled" {
            return;
        }
        self.last_stamp_progress = Instant::now();
        self.stamp_landed_at.insert(done.row, self.last_stamp_progress);
        if done.error.is_some() {
            // Handed to the StoreWorker: hold the row while it lands.
            self.stamp_deferred(done.row);
            return;
        }
        if let Some(entry) = self.settling_rows.get_mut(&done.row) {
            entry.0 = entry.0.saturating_sub(1);
            if entry.0 == 0 && entry.1.is_none() {
                self.settling_rows.remove(&done.row);
            }
        }
    }

    /// One of `row`'s stamps will not land through the lane (refused,
    /// panicked or failed its retries) but was handed to the StoreWorker:
    /// count it as done and hold the row while the deferred write lands.
    fn stamp_deferred(&mut self, row: i64) {
        self.last_stamp_progress = Instant::now();
        self.stamp_landed_at.insert(row, self.last_stamp_progress);
        let entry = self.settling_rows.entry(row).or_insert((0, None));
        entry.0 = entry.0.saturating_sub(1);
        entry.1 = Some(Instant::now() + SETTLING_DEFERRED_HOLD);
    }

    /// Whether a stamp submitted to the lane has not landed yet, so a new
    /// QueueSelect could read a row before its stamp (review L3). A lane
    /// that made no progress for [`STAMP_STALL_LIMIT`] stops gating, so a
    /// wedged writer cannot stop delivery outright (the watchdog reports it).
    pub(crate) fn stamps_pending(&self, now: Instant) -> bool {
        self.settling_rows.values().any(|(count, _)| *count > 0)
            && now.saturating_duration_since(self.last_stamp_progress) < STAMP_STALL_LIMIT
    }

    /// The finished selection, once.
    /// Phase one and two of a row's transport claim (cas-786a6, H3): the
    /// claim is taken by a `ClaimForInject` job on the writer, never on the
    /// loop. The first call submits it and returns `Waiting`; a call on a
    /// later pass, once the job came back, returns its outcome. Inline
    /// (unit tests) the job lands at once and the same call returns it.
    pub(crate) fn claim_for_inject(
        &mut self,
        cas_dir: &Path,
        session: &str,
        row: i64,
        recipient: &str,
        target: &str,
    ) -> ClaimStep {
        let key = (row, recipient.to_string());
        if !self.claims.contains_key(&key) {
            self.submit_claim(cas_dir, session, row, recipient, target);
            if matches!(self.exec, Exec::Inline(_)) {
                self.apply_completions();
            }
        }
        if !matches!(self.claims.get(&key), Some(ClaimState::Ready(..))) {
            return ClaimStep::Waiting;
        }
        let Some(ClaimState::Ready(outcome, _)) = self.claims.remove(&key) else {
            return ClaimStep::Waiting;
        };
        match outcome {
            ClaimOutcome::Claimed(token) => ClaimStep::Claimed(token),
            ClaimOutcome::HandedOff => ClaimStep::HandedOff,
            ClaimOutcome::NotClaimed | ClaimOutcome::Stale => ClaimStep::NotClaimed,
            ClaimOutcome::Failed(error) => {
                tracing::warn!(
                    target: "cas::coordination",
                    message_id = row,
                    %recipient,
                    %error,
                    "factory loop v2: transport claim failed; the row retries"
                );
                ClaimStep::NotClaimed
            }
        }
    }

    /// Make sure a claim job for `(row, recipient)` is out or back, and say
    /// whether it is back. A broadcast checks every member this way before
    /// injecting any (cas-786a6).
    pub(crate) fn claim_resolved(
        &mut self,
        cas_dir: &Path,
        session: &str,
        row: i64,
        recipient: &str,
        target: &str,
    ) -> bool {
        let key = (row, recipient.to_string());
        if !self.claims.contains_key(&key) {
            self.submit_claim(cas_dir, session, row, recipient, target);
            if matches!(self.exec, Exec::Inline(_)) {
                self.apply_completions();
            }
        }
        matches!(self.claims.get(&key), Some(ClaimState::Ready(..)))
    }

    fn submit_claim(&mut self, cas_dir: &Path, session: &str, row: i64, recipient: &str, target: &str) {
        let generation = self.generation();
        let ready = Arc::clone(&self.ready);
        let spec = JobSpec {
            kind: "ClaimForInject",
            key: format!("ClaimForInject:{row}:{recipient}"),
            lane: Lane::Write,
            criticality: Criticality::Critical,
            generation,
            budget: QUEUE_JOB_BUDGET,
        };
        let (job_dir, job_session, job_recipient, job_target) = (
            cas_dir.to_path_buf(),
            session.to_string(),
            recipient.to_string(),
            target.to_string(),
        );
        // Pending first: inline, the job's result is applied after this.
        self.claims.insert(
            (row, recipient.to_string()),
            ClaimState::Pending(Instant::now(), generation),
        );
        let submitted = self.submit_job(spec, false, move || {
            let done = run_claim_for_inject(
                &job_dir,
                &job_session,
                row,
                &job_recipient,
                &job_target,
                generation,
            );
            // Wake the loop: the row's next selection can inject it.
            ready.store(true, Ordering::Release);
            QueueJobOutcome::Claimed(done)
        });
        if !accepted(&submitted) {
            self.claims.remove(&(row, recipient.to_string()));
        }
    }

    /// Admit a row whose claim came back (spec §4.5 rule 2): in flight from
    /// `now` under `claim_token`, unless an earlier inject of the row has not
    /// settled, in which case the new claim is released.
    pub(crate) fn admit_claimed(
        &mut self,
        cas_dir: &Path,
        row: i64,
        recipient: &str,
        claim_token: cas_store::ClaimToken,
        now: Instant,
    ) -> Admission {
        if self.in_flight.holds_row(row) {
            self.submit_stamp(cas_dir, row, recipient, 0, QueueStamp::ReleaseClaim { claim_token });
            return Admission::InFlight;
        }
        let generation = self.generation();
        self.in_flight.begin(row, recipient, generation, now, Some(claim_token));
        Admission::Inject { generation }
    }

    /// Release claims that came back but were not taken within
    /// [`CLAIM_READY_TTL`], and forget claim jobs lost past the outcome
    /// timeout.
    pub(crate) fn expire_claims(&mut self, cas_dir: &Path, now: Instant) {
        let mut released = Vec::new();
        self.claims.retain(|(row, recipient), state| match state {
            ClaimState::Pending(since, _) => {
                now.saturating_duration_since(*since) < INJECT_OUTCOME_TIMEOUT
            }
            ClaimState::Ready(outcome, since) => {
                if now.saturating_duration_since(*since) < CLAIM_READY_TTL {
                    return true;
                }
                if let ClaimOutcome::Claimed(token) = outcome {
                    released.push((*row, recipient.clone(), *token));
                }
                false
            }
        });
        for (row, recipient, claim_token) in released {
            tracing::debug!(
                target: "cas::coordination",
                message_id = row,
                %recipient,
                "factory loop v2: a claimed row was not delivered in time; releasing its claim"
            );
            self.submit_stamp(cas_dir, row, &recipient, 0, QueueStamp::ReleaseClaim { claim_token });
        }
    }

    /// Whether a claim job for `(row, recipient)` is out or came back
    /// untaken (tests).
    #[cfg(test)]
    pub(crate) fn has_claim(&self, row: i64, recipient: &str) -> bool {
        self.claims.contains_key(&(row, recipient.to_string()))
    }

    /// Rows claim jobs re-targeted since the last call.
    pub(crate) fn take_claim_retargeted(&mut self) -> Vec<i64> {
        std::mem::take(&mut self.claim_retargeted)
    }

    pub(crate) fn take_selection(&mut self) -> Option<QueueSelection> {
        let Some(mut selection) = self.selection.take() else {
            return self.take_claimed_batch();
        };
        selection.submitted_at = Some(self.select_submitted_at);
        self.last_roster = Some((
            selection.registered_agents.clone(),
            selection.native_agents.clone(),
        ));
        // Rows whose claims are back join the batch (a claimed row is not
        // selectable again, spec §4.5 rule 1).
        let ready = self.take_claim_ready_rows();
        for (row, facts) in ready {
            if selection.rows.iter().any(|selected| selected.id == row.id) {
                continue;
            }
            selection.facts.insert(row.id, facts);
            selection.rows.push(row);
        }
        Some(selection)
    }

    /// Carry `row` until its claim job comes back: a claimed row is no
    /// longer selectable, so its second phase is delivered from here
    /// (cas-786a6, H3).
    pub(crate) fn carry_claimed_row(
        &mut self,
        row: cas_store::QueuedPrompt,
        facts: super::queue_and_events::RowFacts,
    ) {
        self.carried.insert(row.id, (row, facts));
    }

    /// Carried rows whose every claim job is back, removed from the carry.
    /// Rows with no claim left (expired) are dropped: they are selectable.
    fn take_claim_ready_rows(&mut self) -> Vec<(cas_store::QueuedPrompt, super::queue_and_events::RowFacts)> {
        let claims = &self.claims;
        let mut ready = Vec::new();
        self.carried.retain(|id, (row, facts)| {
            let mut any = false;
            let mut pending = false;
            for ((claim_row, _), state) in claims {
                if claim_row == id {
                    any = true;
                    pending |= matches!(state, ClaimState::Pending(..));
                }
            }
            if !any {
                return false;
            }
            if pending {
                return true;
            }
            ready.push((row.clone(), facts.clone()));
            false
        });
        ready.sort_by_key(|(row, _)| row.id);
        ready
    }

    /// A batch of only the carried rows whose claims came back, delivered
    /// against the last selection's roster.
    fn take_claimed_batch(&mut self) -> Option<QueueSelection> {
        let (registered_agents, native_agents) = self.last_roster.clone()?;
        let ready = self.take_claim_ready_rows();
        if ready.is_empty() {
            return None;
        }
        let mut batch = QueueSelection {
            registered_agents,
            native_agents,
            ..QueueSelection::default()
        };
        for (row, facts) in ready {
            batch.facts.insert(row.id, facts);
            batch.rows.push(row);
        }
        Some(batch)
    }

    /// Whether any stamp has been submitted and not landed or deferred.
    fn stamps_outstanding(&self) -> bool {
        self.settling_rows.values().any(|(count, _)| *count > 0)
    }

    /// Whether a stamp for `row` landed after `since` (a selection's submit
    /// time): the selection may have read the row before it, so delivery
    /// skips the row this pass (review round 2, item 3).
    pub(crate) fn stamped_since(&self, row: i64, since: Instant) -> bool {
        self.stamp_landed_at.get(&row).is_some_and(|landed| *landed > since)
    }
}

fn accepted(submitted: &Submitted) -> bool {
    matches!(
        submitted,
        Submitted::Queued
            | Submitted::Coalesced { .. }
            | Submitted::QueuedBehindKey
            | Submitted::QueuedDroppedOldest { .. }
    )
}

thread_local! {
    /// Stores a job thread has opened, by kind and `.cas` directory.
    static JOB_THREAD_STORES: std::cell::RefCell<HashMap<(&'static str, PathBuf), Box<dyn std::any::Any>>> =
        std::cell::RefCell::new(HashMap::new());
}

/// A store opened, and its schema `init()` run, once per job thread and then
/// reused by every later job on that thread (review round 1, M4). Write-lane
/// jobs all run on the StoreWorker thread, so the daemon keeps one handle per
/// store there.
pub(crate) fn job_thread_store<T: ?Sized + 'static>(
    kind: &'static str,
    cas_dir: &Path,
    open: impl FnOnce(&Path) -> anyhow::Result<Arc<T>>,
) -> anyhow::Result<Arc<T>> {
    let key = (kind, cas_dir.to_path_buf());
    if let Some(store) = JOB_THREAD_STORES.with(|stores| {
        stores
            .borrow()
            .get(&key)
            .and_then(|store| store.downcast_ref::<Arc<T>>())
            .cloned()
    }) {
        return Ok(store);
    }
    let store = open(cas_dir)?;
    JOB_THREAD_STORES.with(|stores| {
        stores
            .borrow_mut()
            .insert(key, Box::new(Arc::clone(&store)) as Box<dyn std::any::Any>)
    });
    Ok(store)
}

/// Drop this thread's handles for `cas_dir`, so the next job reopens them.
pub(crate) fn forget_job_thread_stores(cas_dir: &Path) {
    JOB_THREAD_STORES.with(|stores| stores.borrow_mut().retain(|(_, dir), _| dir != cas_dir));
}

pub(crate) fn job_thread_prompt_queue(
    cas_dir: &Path,
) -> anyhow::Result<Arc<dyn cas_store::PromptQueueStore>> {
    job_thread_store("prompt_queue", cas_dir, |dir| {
        Ok(crate::store::open_prompt_queue_store(dir)?)
    })
}

/// Hand a stamp that could not land to the StoreWorker, which retries it
/// without a budget (GH #1165 `defer`). Never runs the write on the caller.
pub(crate) fn defer_stamp(
    cas_dir: &Path,
    row: i64,
    recipient: &str,
    stamp: &QueueStamp,
    first_error: &str,
) -> super::store_worker::Persisted {
    let label = stamp.label();
    let cas_dir = cas_dir.to_path_buf();
    let deferred_recipient = recipient.to_string();
    let stamp = stamp.clone();
    let persisted = super::store_worker::defer("queue stamp", first_error, move || {
        let queue = crate::store::open_prompt_queue_store(&cas_dir).map_err(|error| error.to_string())?;
        stamp
            .apply(queue.as_ref(), row, &deferred_recipient)
            .map_err(|error| error.to_string())
    });
    if persisted == super::store_worker::Persisted::Dropped {
        tracing::error!(
            target: "cas::coordination",
            message_id = row,
            %recipient,
            stamp = label,
            error = %first_error,
            "factory loop v2: queue stamp failed and could not be handed to the store worker"
        );
    }
    persisted
}

/// Tests: fail the next N stamp attempts on the job lane, before the store
/// is touched, to drive the exhausted-retries path for real.
#[cfg(test)]
pub(crate) static INJECTED_STAMP_FAILURES: std::sync::Mutex<Option<HashMap<PathBuf, usize>>> =
    std::sync::Mutex::new(None);

/// Tests: fail the next `count` stamp attempts for stores in `cas_dir` only.
#[cfg(test)]
pub(crate) fn inject_stamp_failures(cas_dir: &Path, count: usize) {
    INJECTED_STAMP_FAILURES
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(cas_dir.to_path_buf(), count);
}

#[cfg(test)]
static INJECTED_STAMP_PANICS: std::sync::Mutex<Option<HashMap<PathBuf, usize>>> =
    std::sync::Mutex::new(None);

/// Tests: make the next `count` stamp jobs for `cas_dir` panic outside the
/// write's own catch, as a broken job would.
#[cfg(test)]
pub(crate) fn inject_stamp_panics(cas_dir: &Path, count: usize) {
    INJECTED_STAMP_PANICS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(cas_dir.to_path_buf(), count);
}

#[cfg(test)]
fn take_injected_stamp_panic(cas_dir: &Path) -> bool {
    let mut injected = INJECTED_STAMP_PANICS.lock().unwrap();
    match injected.as_mut().and_then(|map| map.get_mut(cas_dir)) {
        Some(left) if *left > 0 => {
            *left -= 1;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
fn take_injected_stamp_failure(cas_dir: &Path) -> bool {
    let mut injected = INJECTED_STAMP_FAILURES.lock().unwrap();
    match injected.as_mut().and_then(|map| map.get_mut(cas_dir)) {
        Some(left) if *left > 0 => {
            *left -= 1;
            true
        }
        _ => false,
    }
}

/// Run one stamp, retrying a busy store a few times on the writer. Returns
/// the last error when every attempt failed.
fn apply_stamp_with_retry(cas_dir: &Path, row: i64, recipient: &str, stamp: &QueueStamp) -> Option<String> {
    let mut pause = Duration::from_millis(100);
    let mut last = None;
    for attempt in 1..=4 {
        #[cfg(test)]
        if take_injected_stamp_failure(cas_dir) {
            last = Some("injected stamp failure".to_string());
            continue;
        }
        let result = job_thread_prompt_queue(cas_dir)
            .map_err(|error| error.to_string())
            .and_then(|queue| stamp.apply(queue.as_ref(), row, recipient).map_err(|error| error.to_string()));
        if result.is_err() {
            // A broken handle is reopened on the next attempt.
            forget_job_thread_stores(cas_dir);
        }
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
    use crate::ui::factory::director::{
        MergeAlertFreshness, check_merge_alert_freshness_for_task, epic_completion_is_current,
        worker_now_has_real_assignment,
    };
    // Judge each unread tagged row from the fresh read, outside the lock.
    // Each verdict is computed once per tag.
    let mut workers: HashMap<String, bool> = HashMap::new();
    let mut tasks: HashMap<String, bool> = HashMap::new();
    let mut epics: HashMap<String, bool> = HashMap::new();
    let stale: HashSet<AlertIdentity> = pruner
        .unread_alerts(recipient)
        .into_iter()
        .filter(|alert| {
            let worker_stale = alert.retract_worker.as_ref().is_some_and(|worker| {
                *workers
                    .entry(worker.clone())
                    .or_insert_with(|| worker_now_has_real_assignment(fresh, worker))
            });
            let task_stale = alert.retract_task.as_ref().is_some_and(|task| {
                *tasks.entry(task.clone()).or_insert_with(|| {
                    matches!(
                        check_merge_alert_freshness_for_task(task, fresh, repo_root),
                        MergeAlertFreshness::Stale
                    )
                })
            });
            let epic_stale = alert.retract_epic.as_ref().is_some_and(|epic| {
                *epics
                    .entry(epic.clone())
                    .or_insert_with(|| !epic_completion_is_current(fresh, epic))
            });
            worker_stale || task_stale || epic_stale
        })
        .map(|alert| alert.identity)
        .collect();
    let mut report = PruneReport {
        recipient: recipient.to_string(),
        ..PruneReport::default()
    };
    match pruner.retract_exact(recipient, &stale) {
        Ok(retracted) => {
            report.idle = retracted.idle;
            report.merge = retracted.merge;
            report.epic = retracted.epic;
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
pub(super) mod tests;

pub(crate) fn job_thread_spawn_queue(
    cas_dir: &Path,
) -> anyhow::Result<Arc<dyn cas_store::SpawnQueueStore>> {
    job_thread_store("spawn", cas_dir, |root| {
        Ok(crate::store::open_spawn_queue_store(root)?)
    })
}
