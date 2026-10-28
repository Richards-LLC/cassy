//! Factory loop v2 C4a (cas-786a6, spec §3.5 and §4.3): urgent and
//! context-reset injects are commands on the loop.
//!
//! The pass that decides an urgent inject sends the turn break and records
//! who owns the inject here; the mux holds the per-pane settle state
//! ([`cas_mux::Mux::begin_interrupt_and_inject`]). Every pass then advances
//! the mux, and each finished inject comes back as an outcome that is
//! applied to its owner: the queue row's stamps, a broadcast's tally, a
//! context reset's bookkeeping, or an operator's log line. No pass waits
//! for a settle.

use std::collections::{HashMap, VecDeque};

/// Operator urgent injects queued behind one settling on the same pane
/// (cas-786a6 review M4), at most this many.
const OPERATOR_BACKLOG_CAP: usize = 16;

use super::delivery::NudgeReport;
use super::queue_and_events::UrgentWakeProbe;
use crate::ui::factory::daemon::FactoryDaemon;

/// Everything the direct delivery path needs after its inject to record
/// what happened (the same state `deliver_queue_selection` held inline).
#[derive(Debug, Clone)]
pub(crate) struct DirectDelivery {
    pub(crate) message_batch: Vec<(cas_store::QueuedPrompt,u64)>,
    pub(crate) message_senders: Vec<String>,
    pub(crate) queued: cas_store::QueuedPrompt,
    /// The pane the row was delivered to (`supervisor` is name-normalised).
    pub(crate) pane_target: String,
    /// The in-flight generation `QueueJobs::admit` gave the inject.
    pub(crate) generation: u64,
    pub(crate) wake_deferred: bool,
    pub(crate) prior_copy_unread: bool,
    pub(crate) nudge_only: bool,
    /// Charge cadence only after this attempt reports a non-composer outcome.
    pub(crate) renudge_granted: bool,
    pub(crate) urgent_wake_probe_opened: bool,
    /// Wall-clock floor for the normal-delivery watchdog (cas-ac97).
    pub(crate) delivery_started_at_utc: chrono::DateTime<chrono::Utc>,
}

/// An all-workers broadcast whose urgent members are still settling.
#[derive(Debug, Clone)]
pub(crate) struct BroadcastTally {
    pub(crate) queued: cas_store::QueuedPrompt,
    pub(crate) attempted: u32,
    pub(crate) succeeded: u32,
    pub(crate) failed: u32,
    pub(crate) notes: Vec<String>,
    /// Members whose inject has not reported yet.
    pub(crate) pending: usize,
}

impl BroadcastTally {
    pub(crate) fn new(queued: cas_store::QueuedPrompt, attempted: u32) -> Self {
        Self {
            queued,
            attempted,
            succeeded: 0,
            failed: 0,
            notes: Vec::new(),
            pending: 0,
        }
    }
}

/// Who owns a started urgent inject.
#[derive(Debug)]
pub(crate) enum UrgentInject {
    /// Normal body-bearing PTY row awaiting payload AND submit receipts.
    NormalRow {
        delivery: Box<DirectDelivery>,
        inline_supervisor: bool,
    },
    /// A direct urgent queue row; its wake probe opens once it is typed.
    Row {
        delivery: Box<DirectDelivery>,
        /// The pane's output counter before the turn break (cas-ac7e).
        bytes_at_inject: u64,
    },
    /// One worker of an urgent all-workers broadcast.
    BroadcastMember {
        row: i64,
        worker: String,
        claim_token: cas_store::ClaimToken,
    },
    /// A queued context-reset command (cas-dffe).
    ContextReset { row: i64, target: String },
    /// An operator's urgent inject from a GUI or WebSocket client.
    Operator { pane: String },
}

/// The daemon's started urgent injects and settling broadcasts.
#[derive(Debug, Default)]
pub(crate) struct UrgentInjects {
    pub(crate) owners: HashMap<cas_mux::InjectTicket, UrgentInject>,
    pub(crate) broadcasts: HashMap<i64, BroadcastTally>,
    /// Operator urgent injects waiting for their pane's settling inject
    /// (review M4): pane, payload, settle. Started in order.
    pub(crate) operator_backlog: VecDeque<(String, String, std::time::Duration)>,
}

impl UrgentInjects {
    /// Whether an urgent inject for queue row `row` is still settling. Such
    /// a row is not delivered again until its outcome is applied.
    pub(crate) fn holds_row(&self, row: i64) -> bool {
        self.broadcasts.contains_key(&row)
            || self.owners.values().any(|owner| match owner {
                UrgentInject::Row { delivery, .. } | UrgentInject::NormalRow { delivery, .. } => {
                    delivery.queued.id == row || delivery.message_batch.iter().any(|(member,_)|member.id==row)
                }
                UrgentInject::ContextReset { row: id, .. } => *id == row,
                UrgentInject::BroadcastMember { row: id, .. } => *id == row,
                UrgentInject::Operator { .. } => false,
            })
    }
}

impl FactoryDaemon {
    /// Advance every settling urgent inject and apply the outcomes that came
    /// in. Called once per loop pass and at the top of each prompt-queue
    /// pass; never waits.
    pub(crate) async fn advance_urgent_injects(&mut self) {
        self.advance_urgent_injects_with_ports(std::time::Instant::now(), |write| {
            super::store_worker::submit_write("blocked PTY write notice", write);
        })
        .await;
    }

    /// The production handler with clock and off-loop persistence boundaries.
    pub(super) async fn advance_urgent_injects_with_ports(
        &mut self,
        now: std::time::Instant,
        mut persist: impl FnMut(Box<dyn Fn() -> Result<(), String> + Send>),
    ) {
        let completions = self.app.mux.advance_injects_at(now).await;
        // Observe separately from completion. Returning an error or releasing
        // the claim on this deadline could retry bytes already partly written.
        for stalled in self
            .app
            .mux
            .take_stalled_injects(now, super::prompt_queue_jobs::INJECT_OUTCOME_TIMEOUT)
        {
            let row =
                self.urgent_injects
                    .owners
                    .get(&stalled.ticket)
                    .and_then(|owner| match owner {
                        UrgentInject::NormalRow { delivery, .. }
                        | UrgentInject::Row { delivery, .. } => Some(delivery.queued.id),
                        UrgentInject::BroadcastMember { row, .. }
                        | UrgentInject::ContextReset { row, .. } => Some(*row),
                        UrgentInject::Operator { .. } => None,
                    });
            // Include the original pane incarnation: ticket counters restart
            // with a daemon, and a same-name respawn must be a new episode.
            let occurrence = serde_json::json!([
                self.session_name.as_str(),
                stalled.pane_id.as_str(),
                format!("{:?}", stalled.pane_spawned_at),
                stalled.ticket.get(),
                row,
            ])
            .to_string();
            let pane = stalled.pane_id;
            let detail = format!(
                "PTY payload or submit write to pane {pane}, ticket {}, queue row {}, \
                     was still pending after {}s at detection. The write may be partial. \
                     CAS retains the original receipt and ownership; do not resend the message. \
                     Inspect the pane and recover or explicitly close/respawn it if it cannot read. \
                     Physical completion/failure or pane-incarnation loss releases the command; \
                     this deadline does not mark it Delivered or prove a turn started.",
                stalled.ticket.get(),
                row.map_or_else(|| "operator/unknown".into(), |row| row.to_string()),
                stalled.pending_for.as_secs(),
            );
            let cas_dir = self.app.cas_dir().to_path_buf();
            persist(Box::new(
                move || match super::lifecycle::enqueue_blocked_pty_write_relay(
                    &cas_dir,
                    &pane,
                    &detail,
                    &occurrence,
                ) {
                    super::lifecycle::WorkerAttentionRelayOutcome::Persisted { .. } => Ok(()),
                    _ => Err("blocked PTY write relay not yet persisted".to_string()),
                },
            ));
        }
        self.start_operator_backlog().await;
        for completion in completions {
            let Some(owner) = self.urgent_injects.owners.remove(&completion.ticket) else {
                tracing::warn!(
                    target: "cas::coordination",
                    ticket = completion.ticket.get(),
                    pane = %completion.pane_id,
                    "urgent inject finished with no owner"
                );
                continue;
            };
            // A completed tracked receipt includes BOTH physical writes. This
            // is stronger than dispatch/admission, but still not a harness turn.
            if matches!(&completion.result, Ok(cas_mux::InjectOutcome::Delivered)) {
                let message_id = match &owner {
                    UrgentInject::NormalRow { delivery, .. }
                    | UrgentInject::Row { delivery, .. } => Some(delivery.queued.id),
                    UrgentInject::BroadcastMember { row, .. }
                    | UrgentInject::ContextReset { row, .. } => Some(*row),
                    UrgentInject::Operator { .. } => None,
                };
                tracing::info!(
                    target: "cas::coordination",
                    stage = "pty_submit_confirmed",
                    message_id,
                    ticket = completion.ticket.get(),
                    target_agent = %completion.pane_id,
                    turn_acceptance = "unconfirmed",
                    "PTY payload and submit CR written; harness turn acceptance unconfirmed"
                );
            }
            tracing::debug!(
                target: "cas::coordination",
                stage = "urgent_inject_settled",
                ticket = completion.ticket.get(),
                target_agent = %completion.pane_id,
                settled_ms = completion.settled_after.as_millis() as u64,
                ok = completion.result.is_ok(),
                "urgent inject reached its payload step"
            );
            let result: anyhow::Result<cas_mux::InjectOutcome> =
                completion.result.map_err(Into::into);
            match owner {
                UrgentInject::NormalRow {
                    delivery,
                    inline_supervisor,
                } => {
                    let report = result.map(|outcome| {
                        let mut report = NudgeReport::not_attempted(
                            outcome,
                            super::delivery::DeliveryChannel::Pty,
                            super::delivery::PTY_DELIVERY_WAKE_DETAIL,
                        );
                        if inline_supervisor && outcome == cas_mux::InjectOutcome::Delivered {
                            report.wake = cas_store::WakeAttempt::Fired;
                        }
                        report
                    });
                    self.finish_direct_delivery(*delivery, report);
                }
                UrgentInject::Row {
                    mut delivery,
                    bytes_at_inject,
                } => {
                    let report = result.map(|outcome| match outcome {
                        cas_mux::InjectOutcome::Delivered => {
                            self.urgent_wake_probes.insert(
                                delivery.queued.id,
                                UrgentWakeProbe {
                                    pane: delivery.pane_target.clone(),
                                    target: delivery.queued.target.clone(),
                                    bytes_at_inject,
                                    injected_at: std::time::Instant::now(),
                                },
                            );
                            delivery.urgent_wake_probe_opened = true;
                            NudgeReport {
                                outcome,
                                channel: super::delivery::DeliveryChannel::Pty,
                                wake: cas_store::WakeAttempt::Fired,
                                wake_detail: Some("urgent interrupt-and-inject".to_string()),
                            }
                        }
                        cas_mux::InjectOutcome::DeferredComposerDirty => NudgeReport::not_attempted(
                            outcome,
                            super::delivery::DeliveryChannel::Pty,
                            "urgent delivery retained until the operator composer is clean",
                        ),
                    });
                    self.finish_direct_delivery(*delivery, report);
                }
                UrgentInject::BroadcastMember {
                    row,
                    worker,
                    claim_token,
                } => {
                    let Some(mut tally) = self.urgent_injects.broadcasts.remove(&row) else {
                        continue;
                    };
                    tally.pending = tally.pending.saturating_sub(1);
                    self.tally_broadcast_member(&mut tally, &worker, claim_token, result);
                    if tally.pending == 0 {
                        self.finish_broadcast(tally);
                    } else {
                        self.urgent_injects.broadcasts.insert(row, tally);
                    }
                }
                UrgentInject::ContextReset { row, target } => {
                    let outcome = match result {
                        Ok(cas_mux::InjectOutcome::Delivered) => {
                            super::delivery::ContextResetDelivery::Injected
                        }
                        Ok(cas_mux::InjectOutcome::DeferredComposerDirty) => {
                            super::delivery::ContextResetDelivery::NotReady
                        }
                        Err(error) => super::delivery::ContextResetDelivery::Failed {
                            detail: format!("pane inject failed: {error}"),
                        },
                    };
                    self.finish_context_reset(row, &target, outcome);
                }
                UrgentInject::Operator { pane } => match result {
                    Ok(outcome) => tracing::info!(
                        target: "cas::coordination",
                        stage = "operator_urgent_inject",
                        target_agent = %pane,
                        ?outcome,
                        "operator urgent inject finished"
                    ),
                    Err(error) => tracing::warn!(
                        target: "cas::coordination",
                        stage = "operator_urgent_inject",
                        target_agent = %pane,
                        %error,
                        "operator urgent inject failed"
                    ),
                },
            }
        }
    }

    /// Open the loop's prompt-queue handle and release the transport claims
    /// a previous daemon of this session took but never handed off (review
    /// M2, spec §4.5 rule 5), before this daemon claims anything. Runs once,
    /// before the loop thread becomes a lane.
    pub(crate) fn release_orphaned_claims_at_start(&mut self) {
        let queue = match self.queue_jobs.store(self.app.cas_dir()) {
            Ok(queue) => queue,
            Err(error) => {
                tracing::warn!(%error, "prompt queue store unavailable at loop start; opened on first use");
                return;
            }
        };
        match queue.release_orphaned_transport_claims(&self.session_name) {
            Ok(0) => {}
            Ok(released) => tracing::warn!(
                target: "cas::coordination",
                released,
                factory_session = %self.session_name,
                "released transport claims a previous daemon never handed off"
            ),
            Err(error) => tracing::warn!(%error, "could not release orphaned transport claims"),
        }
    }

    /// An operator's urgent inject (GUI or WebSocket `Inject { urgent }`):
    /// the turn break now, the payload after the settle. Raw semantics, as
    /// before: no composer preservation. One that arrives while another
    /// urgent inject settles on the pane waits for it (review M4).
    pub(crate) async fn begin_operator_urgent_inject(
        &mut self,
        pane: &str,
        payload: &str,
        settle: std::time::Duration,
    ) {
        let queued_behind = self
            .urgent_injects
            .operator_backlog
            .iter()
            .any(|(queued, _, _)| queued == pane);
        // Review round 2, M: wait while the pane settles an inject or its
        // payload awaits its submit CR; a turn break then would land
        // between that payload and its CR.
        if queued_behind || self.app.mux.pane_inject_busy(pane) {
            self.queue_operator_urgent_inject(pane, payload, settle);
            return;
        }
        self.start_operator_urgent_inject(pane, payload, settle).await;
    }

    fn queue_operator_urgent_inject(&mut self, pane: &str, payload: &str, settle: std::time::Duration) {
        if self.urgent_injects.operator_backlog.len() >= OPERATOR_BACKLOG_CAP {
            tracing::error!(
                target: "cas::coordination",
                stage = "operator_urgent_inject",
                target_agent = %pane,
                "operator urgent inject refused: too many already waiting for their panes"
            );
            return;
        }
        tracing::info!(
            target: "cas::coordination",
            stage = "operator_urgent_inject",
            target_agent = %pane,
            "operator urgent inject queued behind the one settling on this pane"
        );
        self.urgent_injects.operator_backlog.push_back((
            pane.to_string(),
            payload.to_string(),
            settle,
        ));
    }

    /// Start queued operator injects whose pane is free, in order per pane.
    async fn start_operator_backlog(&mut self) {
        let mut waiting = std::mem::take(&mut self.urgent_injects.operator_backlog);
        let mut busy = std::collections::HashSet::new();
        while let Some((pane, payload, settle)) = waiting.pop_front() {
            if busy.contains(&pane) || self.app.mux.pane_inject_busy(&pane) {
                busy.insert(pane.clone());
                self.urgent_injects.operator_backlog.push_back((pane, payload, settle));
                continue;
            }
            busy.insert(pane.clone());
            self.start_operator_urgent_inject(&pane, &payload, settle).await;
        }
    }

    async fn start_operator_urgent_inject(
        &mut self,
        pane: &str,
        payload: &str,
        settle: std::time::Duration,
    ) {
        match self
            .app
            .mux
            .begin_interrupt_and_inject(pane, payload, settle, false)
            .await
        {
            Ok(cas_mux::InjectStart::Started(ticket)) => {
                self.urgent_injects.owners.insert(
                    ticket,
                    UrgentInject::Operator {
                        pane: pane.to_string(),
                    },
                );
            }
            Ok(cas_mux::InjectStart::Deferred) => {
                self.queue_operator_urgent_inject(pane, payload, settle);
            }
            Err(error) => tracing::warn!(
                target: "cas::coordination",
                stage = "operator_urgent_inject",
                target_agent = %pane,
                %error,
                "operator urgent inject failed"
            ),
        }
    }
}
