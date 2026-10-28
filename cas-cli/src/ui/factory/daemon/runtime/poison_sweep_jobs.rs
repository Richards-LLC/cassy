//! Poison-sweep snapshots and retryable Teams file delivery (cas-cc89).
//! Heavy reads and file writes run off the store writer; only short durable
//! claims, releases and enqueues use Write. The loop owns each pending phase.
use super::queue_and_events::message_attention::{AttentionInputs, MessageAttention};
use super::queue_and_events::{
    TEAMS_POINTER_FALLBACK_AFTER_SECS, TEAMS_POINTER_FALLBACK_WINDOW_SECS, delivery_stalled_notice,
    delivery_stalled_threshold_before_abandonment, teams_body_fallback_text,
};
use super::teams::TeamsManager;
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

#[derive(Default)]
pub(crate) struct PoisonJobs {
    pub(crate) generation: u64,
    pub(crate) stopping: bool,
    epochs: HashMap<String, Arc<AtomicBool>>,
    // QueueSelect repeats these names until registration changes. Keep the
    // current set only, so observing the same stopped pane is idempotent.
    stopped_panes: HashSet<String>,
    pub(crate) pointers: HashMap<String, PendingPointer>,
    pub(crate) idle_relayed: HashSet<i64>,
}
impl PoisonJobs {
    pub(crate) fn stop(&mut self) {
        self.stopping = true;
        self.generation = self.generation.wrapping_add(1);
        for live in self.epochs.values() {
            live.store(false, Ordering::Release);
        }
        self.epochs.clear();
    }
    pub(crate) fn epoch(&mut self, pane: &str) -> Arc<AtomicBool> {
        self.epochs
            .entry(pane.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(true)))
            .clone()
    }
    pub(crate) fn prune_epochs(&mut self, names: &HashSet<String>) {
        self.epochs.retain(|name, live| {
            let keep =
                names.contains(name) || self.pointers.values().any(|p| p.work.recipient == *name);
            if !keep {
                live.store(false, Ordering::Release);
            }
            keep
        });
    }
    pub(crate) fn observe_stopped_panes(&mut self, panes: &[String]) {
        let current: HashSet<_> = panes.iter().cloned().collect();
        let newly_stopped: Vec<_> = current.difference(&self.stopped_panes).cloned().collect();
        for pane in newly_stopped {
            self.forget_pane(&pane);
        }
        // A name that resumes is removed, so its next stop invalidates again.
        self.stopped_panes = current;
    }

    pub(crate) fn forget_pane(&mut self, pane: &str) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(live) = self.epochs.remove(pane) {
            live.store(false, Ordering::Release);
        }
    }
}
#[derive(Clone)]
pub(crate) struct PointerWork {
    pub(crate) cas_dir: std::path::PathBuf,
    pub(crate) row: cas_store::QueuedPrompt,
    pub(crate) recipient: String,
    pub(crate) inbox_target: String,
    pub(crate) source: String,
    pub(crate) primary: Arc<TeamsManager>,
    pub(crate) config_dir: Option<String>,
    pub(crate) live: Arc<AtomicBool>,
}
#[derive(Clone, Copy)]
pub(crate) enum PointerPhase {
    Claim,
    File,
    Release,
}
pub(crate) struct PendingPointer {
    pub(crate) work: PointerWork,
    pub(crate) phase: PointerPhase,
    pub(crate) retry_at: Instant,
}
/// Queue ID alone is not a message identity when a held job outlives a change.
pub(crate) fn same_message(a: &cas_store::QueuedPrompt, b: &cas_store::QueuedPrompt) -> bool {
    a.id == b.id
        && a.created_at == b.created_at
        && a.target == b.target
        && a.source == b.source
        && a.prompt == b.prompt
        && a.factory_session == b.factory_session
}
/// None means unreadability, not evidence that the subject was resolved.
pub(crate) fn still_unread(
    queue: &dyn cas_store::PromptQueueStore,
    old: &cas_store::QueuedPrompt,
) -> Option<bool> {
    let current = queue.queued_prompt(old.id).ok()?;
    let Some(row) = current else {
        return Some(false);
    };
    if !same_message(&row, old) || row.acked_at.is_some() {
        return Some(false);
    }
    if queue.recipient_action_satisfies_alarm(old.id).ok()? {
        return Some(false);
    }
    let report = queue.message_delivery_report(old.id).ok()??;
    Some(
        report.confirmed_at.is_none()
            && !report
                .recipient_receipt
                .is_some_and(cas_store::SurfacingSource::is_surfaced)
            && !matches!(
                report.stage,
                cas_store::DeliveryStage::Dropped
                    | cas_store::DeliveryStage::Abandoned
                    | cas_store::DeliveryStage::Suppressed
            ),
    )
}
pub(crate) fn read_pointers(
    cas_dir: &std::path::Path,
    session: &str,
    recipients: &[String],
) -> Vec<(cas_store::QueuedPrompt, String)> {
    let now = chrono::Utc::now();
    crate::store::open_prompt_queue_store(cas_dir)
        .and_then(|queue| {
            Ok(queue.teams_pointer_fallback_candidates(
                session,
                recipients,
                now - chrono::Duration::seconds(TEAMS_POINTER_FALLBACK_WINDOW_SECS),
                now - chrono::Duration::seconds(TEAMS_POINTER_FALLBACK_AFTER_SECS),
                20,
            )?)
        })
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "cas-1f0c: failed to scan unexpanded Teams pointer copies");
            vec![]
        })
}
pub(crate) fn claim_pointer(work: &PointerWork) -> Result<bool, String> {
    if !work.live.load(Ordering::Acquire) {
        return Ok(false);
    }
    let queue = crate::store::open_prompt_queue_store(&work.cas_dir).map_err(|e| e.to_string())?;
    let current = queue
        .queued_prompt(work.row.id)
        .map_err(|e| e.to_string())?;
    if !current
        .as_ref()
        .is_some_and(|row| same_message(row, &work.row))
    {
        return Ok(false);
    }
    queue
        .claim_teams_body_fallback(work.row.id, &work.recipient)
        .map_err(|e| e.to_string())
}
/// File-only phase. On panic the loop still owns this phase and resubmits it;
/// Teams' content dedupe makes a replay of the same body harmless.
pub(crate) fn file_pointer(work: &PointerWork) -> bool {
    if !work.live.load(Ordering::Acquire) {
        return false;
    }
    let Ok(queue) = crate::store::open_prompt_queue_store(&work.cas_dir) else {
        return false;
    };
    let Ok(Some(row)) = queue.queued_prompt(work.row.id) else {
        return false;
    };
    if !same_message(&row, &work.row) || (row.target != "all_workers" && row.acked_at.is_some()) {
        return false;
    }
    let Ok(Some(delivery)) = queue.recipient_delivery(row.id, &work.recipient) else {
        return false;
    };
    let row = delivery.into_prompt();
    let pane = &work.inbox_target;
    let view = work.primary.view_for_config_dir(work.config_dir.as_deref());
    let view = view.filter(|view| {
        view.provision_mirror_from(&work.primary, pane).map_err(|error| {
            tracing::warn!(%error, recipient = %pane, "could not provision recipient Teams tree; using primary");
        }).is_ok()
    });
    if !work.live.load(Ordering::Acquire) {
        return false;
    }
    match view.as_ref().unwrap_or(&work.primary).write_to_inbox(
        pane,
        &work.source,
        &teams_body_fallback_text(&row),
        row.summary.as_deref(),
        None,
    ) {
        Ok(()) => {
            tracing::warn!(target: "cas::coordination", stage = "teams_pointer_body_fallback",
                message_id = row.id, recipient = %pane, "cas-1f0c: wrote full Teams body once");
            true
        }
        Err(error) => {
            tracing::warn!(%error, message_id = row.id, recipient = %pane,
                "cas-1f0c: Teams body fallback failed; releasing claim for retry");
            false
        }
    }
}
pub(crate) fn release_pointer(work: &PointerWork) -> bool {
    crate::store::open_prompt_queue_store(&work.cas_dir)
        .and_then(|queue| Ok(queue.release_teams_body_fallback(work.row.id, &work.recipient)?))
        .map_err(|error| tracing::warn!(%error, "failed to release Teams fallback claim"))
        .is_ok()
}
pub(crate) struct BounceCandidate {
    pub(crate) row: cas_store::QueuedPrompt,
    pub(crate) notice: String,
    pub(crate) consumed: bool,
}
pub(crate) fn read_bounces(
    context: &AttentionInputs,
    harnesses: &HashMap<String, cas_mux::SupervisorCli>,
    default_harness: cas_mux::SupervisorCli,
) -> Vec<BounceCandidate> {
    // Config metadata and load belong to the read job too, even on a cache hit.
    let config = crate::config::Config::load(&context.cas_dir).unwrap_or_default();
    let factory = config.factory();
    let Ok(queue) = crate::store::open_prompt_queue_store(&context.cas_dir) else {
        return vec![];
    };
    let rows = match queue.delivery_stalled_candidates(
        &context.session_name,
        delivery_stalled_threshold_before_abandonment(factory.delivery_stalled_priority_secs),
        delivery_stalled_threshold_before_abandonment(factory.delivery_stalled_normal_secs),
        50,
    ) {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, "failed to scan aged unread coordination messages");
            return vec![];
        }
    };
    let decisions = context.decisions(queue.as_ref(), &rows);
    rows.into_iter()
        .filter(|row| matches!(decisions.get(&row.id), Some(MessageAttention::Needed | MessageAttention::Consumed)))
        .map(|row| {
            let report = queue.message_delivery_report(row.id).unwrap_or_else(|error| {
                tracing::warn!(%error, message_id = row.id, "failed to read delivery state for sender bounce"); None
            });
            let notice = delivery_stalled_notice(&row, harnesses.get(&row.target).copied().unwrap_or(default_harness), report.as_ref());
            let consumed = decisions.get(&row.id) == Some(&MessageAttention::Consumed);
            BounceCandidate { row, notice, consumed }
        }).collect()
}
pub(crate) fn write_bounce(
    context: &AttentionInputs,
    candidate: BounceCandidate,
    live: Arc<AtomicBool>,
) {
    if !live.load(Ordering::Acquire) {
        return;
    }
    let Ok(queue) = crate::store::open_prompt_queue_store(&context.cas_dir) else {
        return;
    };
    if candidate.consumed {
        // Only this positively correlated path may finish a transport claim.
        // The store validates the expected message and promotes atomically;
        // generic wake receipts (including pane growth) retain prior semantics.
        if let Err(error) = queue.record_transcript_consumed(&candidate.row) {
            tracing::warn!(%error, message_id = candidate.row.id, "could not persist transcript-correlated consumption");
        }
        return;
    }
    if still_unread(queue.as_ref(), &candidate.row) != Some(true) {
        return;
    }
    match queue.enqueue_delivery_stalled_bounce(
        candidate.row.id,
        &context.session_name,
        &candidate.notice,
        "delivery stalled — recipient unread",
    ) {
        Ok(Some(bounce_id)) => {
            tracing::warn!(target: "cas::coordination", stage = "delivery_stalled_bounced",
            message_id = candidate.row.id, bounce_id, sender = %candidate.row.source,
            recipient = %candidate.row.target, "aged unread message bounced to its sender");
            page_operator_for_stalled_message(queue.as_ref(), &context.session_name, &candidate, bounce_id);
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(%error, message_id = candidate.row.id, "failed to queue sender-side delivery-stalled bounce")
        }
    }
}

/// cas-badb (GH #1302): the operator hears about an unread message at the same
/// moment its sender does. A sender bounce is written once per recipient
/// episode (until the recipient next reads), and the operator turn is keyed by
/// that bounce, so each episode pages the operator exactly once.
pub(crate) fn page_operator_for_stalled_message(
    queue: &dyn cas_store::PromptQueueStore,
    session: &str,
    candidate: &BounceCandidate,
    bounce_id: i64,
) {
    let row = &candidate.row;
    if row.target.eq_ignore_ascii_case("operator") {
        return;
    }
    let summary = format!("Message from {} to {} unread", row.source, row.target);
    let message = format!(
        "{} has not started a turn with message #{} from {} (sent {}). Its sender has been told. Type in {}'s pane to wake it, or take the work over.\n\n{}",
        row.target,
        row.id,
        row.source,
        row.created_at.to_rfc3339(),
        row.target,
        candidate.notice
    );
    let payload = match serde_json::to_string(&crate::ui::factory::OperatorReplyPayload {
        schema_version: 2,
        reply_to: None,
        message,
        summary: summary.clone(),
        device_id: "*".to_string(),
        operator_label: None,
        kind: crate::ui::factory::OperatorTurnKind::Blocker,
        options: Vec::new(),
        attachments: Vec::new(),
        reply_to_session: None,
    }) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::warn!(%error, "cas-badb: failed to encode the unread-message operator page");
            return;
        }
    };
    let dedupe_key = format!("delivery-stalled-operator:{bounce_id}");
    if let Err(error) = queue.record_operator_turn(&cas_store::OperatorTurn {
        source: "unread-message-watchdog",
        target: "operator",
        prompt: &payload,
        factory_session: (!session.is_empty()).then_some(session),
        metadata: cas_store::OperatorTurnMetadata {
            summary: Some(summary.as_str()),
            priority: Some(cas_store::NotificationPriority::High),
            origin: Some(&cas_store::QueueOrigin::Daemon),
            kind: Some("blocker"),
            dedupe_key: Some(&dedupe_key),
            ..Default::default()
        },
    }) {
        tracing::warn!(%error, message_id = row.id, "cas-badb: failed to record the unread-message operator page");
    }
}
