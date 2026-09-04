//! Recipient-aware message delivery (cas-b68a).
//!
//! Every supervisor→worker (and worker→supervisor / director→agent) message must
//! reach the recipient over a channel the recipient can actually read:
//!
//! - A **Claude** agent in a native Agent-Teams factory reads its inbox files, so
//!   delivery goes through [`TeamsManager::write_to_inbox`].
//! - A **Codex** agent is *not* a member of the Claude team and never polls an
//!   inbox; the only channel it can receive is a direct PTY write
//!   ([`Mux::inject`]) performed by the daemon that holds its PTY master.
//!
//! The historical bug was that every delivery site branched on whether the
//! **supervisor** was in teams mode (`self.teams.is_some()`), not on the
//! **recipient's** harness. A Codex worker under a Claude supervisor therefore had
//! its messages written to an inbox it could never read, and the PTY path — its
//! only viable channel — was never taken. This module centralises the routing
//! decision so it can no longer drift per call site.

use cas_mux::{InjectOutcome, SupervisorCli};
use cas_store::WakeAttempt;
use std::path::Path;

use super::super::FactoryDaemon;

/// Wake the daemon after a producer appends to `prompt_queue`.
///
/// The MCP message path has always sent this best-effort datagram, but daemon
/// lifecycle producers used to rely on the timer poll. That left a spawn brief
/// and other internally generated prompts at the mercy of the next unrelated
/// wake. The queue remains durable when no daemon is listening; this helper is
/// only the low-latency handoff signal.
pub(crate) fn wake_daemon_after_enqueue(cas_dir: &Path) {
    if let Err(error) = cas_factory::notify_daemon(cas_dir) {
        tracing::debug!(
            target: "cas::coordination",
            %error,
            "prompt_queue enqueue wake signal could not reach the daemon"
        );
    }
}

/// The result of a delivery that may carry a wake nudge (cas-7a01, GH #155).
///
/// `outcome` is the transport answer the caller has always acted on. `wake` is
/// the answer the caller could never get: whether Cassy actually woke the
/// recipient, tried and failed, or never tried. Keeping them in one value makes
/// it impossible for a delivery site to record transport state while silently
/// dropping the wake state, which is precisely how three GH incidents produced
/// a `wake: unobserved` with no way to tell what had happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NudgeReport {
    /// Transport outcome for the primary delivery.
    pub(crate) outcome: InjectOutcome,
    /// What the wake nudge did.
    pub(crate) wake: WakeAttempt,
    /// Why a wake failed, or which gate declined it.
    pub(crate) wake_detail: Option<String>,
}

impl NudgeReport {
    /// A delivery where no wake was attempted, with the reason stated.
    pub(crate) fn not_attempted(outcome: InjectOutcome, detail: &str) -> Self {
        Self {
            outcome,
            wake: WakeAttempt::NotAttempted,
            wake_detail: Some(detail.to_string()),
        }
    }
}

/// Outcome of executing a context-reset control command (cas-dffe, GH #145).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContextResetDelivery {
    /// The harness's reset command was typed into the pane.
    Injected,
    /// The pane is not ready for injection yet — retry on a later tick.
    NotReady,
    /// The recipient's harness has no verified in-place reset command. This is
    /// terminal: no amount of retrying makes a reset possible.
    Unsupported { detail: String },
    /// The PTY write failed — retryable.
    Failed { detail: String },
}

/// The channel a message should be delivered over, decided by the recipient's
/// harness and whether the factory is running native Agent Teams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeliveryChannel {
    /// Direct PTY write via `Mux::inject`.
    Pty,
    /// Claude Agent-Teams inbox file via `TeamsManager::write_to_inbox`.
    TeamsInbox,
}

/// Pure routing decision — the single source of truth for *recipient-aware*
/// delivery. Kept free of `self` so it is exhaustively unit-testable.
///
/// - **Codex** recipients are *always* PTY-delivered: they cannot read a Claude
///   team inbox, so this holds even when the supervisor is in teams mode. This is
///   the load-bearing fix for cas-b68a.
/// - **Claude** recipients use the team inbox when teams are active, and fall back
///   to PTY when they are not (codex-only / non-teams factories).
/// - **Grok** recipients are *always* PTY-delivered, same as Codex: EPIC
///   cas-8888 delta #4 — Grok has no CC Agent-Teams membership
///   (`--team-name`/`--agent-id`/`--teammate-mode` don't exist for it), so
///   it can never read a Claude team inbox regardless of the supervisor's
///   teams mode.
pub(crate) fn choose_channel(harness: SupervisorCli, teams_active: bool) -> DeliveryChannel {
    match harness {
        SupervisorCli::Codex | SupervisorCli::Grok => DeliveryChannel::Pty,
        // cas-a5da owns OpenCode factory delivery; it has no Teams transport.
        SupervisorCli::OpenCode => DeliveryChannel::Pty,
        SupervisorCli::Claude => {
            if teams_active {
                DeliveryChannel::TeamsInbox
            } else {
                DeliveryChannel::Pty
            }
        }
    }
}

/// The queue-bookkeeping decision for a single queued message after one
/// delivery attempt (cas-6257). Centralises the "record transport delivery only
/// after the inbox handoff succeeds" invariant so it is unit-testable and cannot
/// drift between call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueuedDeliveryOutcome {
    /// Handoff to the recipient's channel succeeded (inbox write / PTY inject) —
    /// the queue row may be marked processed.
    MarkProcessed,
    /// Handoff failed but the target is a live/known member of this session —
    /// leave the row **pending** (do not advance `processed_at`) so the next
    /// tick retries. This is the load-bearing retryable state.
    Retry,
    /// Handoff failed and the target is not a pane in this session and not a
    /// current worker/supervisor — the message can never be delivered as
    /// addressed, so the row is consumed (marked processed) and its content is
    /// re-routed to the supervisor rather than blocking the queue forever.
    Abandon,
}

/// Decide the queue bookkeeping for one single-target delivery attempt
/// (cas-6257).
///
/// - A successful handoff always marks the row processed.
/// - A failed handoff to a **known pane** (`pane_known`) is retryable — the pane
///   exists, the write just didn't land this tick (e.g. a transient inbox lock
///   or a not-yet-ready PTY), so the row stays pending.
/// - A failed handoff to an **unknown pane** is retryable *only* while the target
///   is still a current worker/supervisor (it may still be spawning); otherwise
///   it is abandoned so a stale cross-session row cannot wedge the queue.
///
/// Crucially, failure never yields `MarkProcessed`: a dropped inbox write leaves
/// the message deliverable on the next tick, matching the durable director-events
/// lane. `process_prompt_queue`'s single-target branch calls this directly, so
/// the contract is exercised by the production path (not a hand-written mirror).
pub(crate) fn classify_queued_delivery(
    delivered_ok: bool,
    pane_known: bool,
    target_is_current: bool,
) -> QueuedDeliveryOutcome {
    if delivered_ok {
        return QueuedDeliveryOutcome::MarkProcessed;
    }
    if pane_known {
        // Pane exists — the failure is transient; retry next tick.
        return QueuedDeliveryOutcome::Retry;
    }
    // Pane not found: retry while the target is still a live session member
    // (it may be mid-spawn); otherwise abandon so the queue can't wedge.
    if target_is_current {
        QueuedDeliveryOutcome::Retry
    } else {
        QueuedDeliveryOutcome::Abandon
    }
}

/// Whether a message to `harness` must clear the PTY pane-readiness gate before
/// injection. True exactly when the message is PTY-delivered — i.e. for every
/// Codex recipient (even under teams) and for everyone in a non-teams factory.
///
/// Claude inbox writes are plain file writes with no readline race, so they never
/// need the gate.
pub(crate) fn requires_pty_readiness_gate(harness: SupervisorCli, teams_active: bool) -> bool {
    matches!(choose_channel(harness, teams_active), DeliveryChannel::Pty)
}

/// Whether a PTY-delivered payload must carry the literal `Message from <sender>: `
/// framing. True iff the recipient is **Codex**, independent of teams mode.
///
/// The Codex worker/supervisor prompts (sibling task cas-83c8) key on EXACTLY this
/// prefix to recognise an injected turn as an actionable instruction, and they do
/// so in *every* codex factory — including a codex-only factory (teams=None) where
/// a codex-supervisor→codex-worker message is still PTY-injected. So framing is a
/// property of the recipient's harness, not of teams mode. A Claude recipient
/// reached via the PTY fallback (codex-supervised factory, teams=None) must NOT be
/// framed — it isn't a codex prompt and stays byte-for-byte bare.
///
/// EPIC cas-8888 (cas-9a31, Phase 1) SILENT SITE — audited: Grok is NOT
/// included here (revised from an earlier version of this comment that did
/// include it — see the task's coordination history). Checked the actual
/// mechanism first: `CODEX_WORKER_INSTRUCTIONS`/`CODEX_SUPERVISOR_INSTRUCTIONS`
/// (crates/cas-pty/src/pty.rs) EXPLICITLY tell Codex to "treat any injected
/// turn framed 'Message from <sender>: …' as an instruction to act on" — the
/// marker exists because it's baked into Codex's own prompt text, not because
/// of any inherent PTY-delivery or hooks property. No such prompt convention
/// exists for Grok yet (that's Phase 2/3's job to author), and Grok's design
/// otherwise mirrors Claude's (native hooks incl. UserPromptSubmit, a real
/// TUI textbox) — so absent a reason to invent an unbacked marker
/// requirement, Grok should behave like Claude's PTY-fallback case: bare,
/// unframed. Revisit once Phase 2/3 actually authors Grok's coordination
/// prompt, if it turns out to need its own recognition convention.
pub(crate) fn pty_payload_needs_framing(harness: SupervisorCli) -> bool {
    matches!(harness, SupervisorCli::Codex)
}

/// Pure decision (cas-893c): given the recipient's *primary* delivery
/// channel, should an idle nudge additionally run?
///
/// Only `TeamsInbox` qualifies — a `Pty`-delivered recipient (Codex, Grok, or
/// a Claude recipient in a non-teams factory) already received the message
/// over the one channel it can read; nudging again would type the same text
/// a second time into its pane.
pub(crate) fn idle_nudge_applies(channel: DeliveryChannel) -> bool {
    channel == DeliveryChannel::TeamsInbox
}

/// How a director-generated prompt should reach its recipient this tick
/// (cas-ae6d / GH #100).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectorPromptRoute {
    /// Hand the payload straight to the recipient's channel now.
    InjectNow,
    /// Park it in the durable `prompt_queue` instead, so the queue lane
    /// (readiness-gated + retrying, see [`classify_queued_delivery`]) delivers
    /// it on a later tick.
    DurableQueue,
}

/// Pure routing decision for a director prompt (cas-ae6d, GH #100).
///
/// The director prompt lane in `lifecycle.rs` is a **one-shot** `Mux::inject`
/// with no readiness gate and no retry, unlike the durable `prompt_queue` lane
/// which has both. For a Claude recipient under teams that costs nothing: the
/// prompt is a file write into an inbox that survives until the worker's next
/// turn boundary. For a **Codex** (or Grok) recipient the same lane is a raw
/// PTY write — if the pane isn't ready for injection yet, or the write fails,
/// the wake-up is gone forever, because the event detector's
/// `task_assigned_announced` guard never re-emits that (task, assignee) pair.
/// That asymmetry is exactly the reported failure: assignment silently fails to
/// wake codex workers while an identical assignment wakes a claude worker.
///
/// `durable` marks prompts whose loss is a stuck worker rather than a missed
/// FYI (today: `TaskAssigned`). Such a prompt bound for a PTY pane that isn't
/// ready goes to the durable queue rather than being written into the void.
/// Everything else keeps its historical behavior byte-for-byte.
pub(crate) fn route_director_prompt(
    channel: DeliveryChannel,
    pane_ready: bool,
    durable: bool,
) -> DirectorPromptRoute {
    if durable && channel == DeliveryChannel::Pty && !pane_ready {
        DirectorPromptRoute::DurableQueue
    } else {
        DirectorPromptRoute::InjectNow
    }
}

/// Whether a direct inject counts as a landed durable delivery (cas-ae6d).
///
/// `Mux::inject` reports `Delivered` the moment the write syscall returns — it
/// cannot observe whether the harness's readline was up yet. So a write into a
/// pane we already determined was NOT ready for injection (reached only when
/// the durable enqueue itself failed) must not be treated as delivery, or the
/// enqueue-failure path silently reinstates the very loss this fixes.
pub(crate) fn durable_delivery_landed(inject_delivered: bool, pane_was_unready: bool) -> bool {
    inject_delivered && !pane_was_unready
}

/// Whether a director prompt that already attempted direct delivery must be
/// re-queued on the durable lane (cas-ae6d).
///
/// True exactly when the prompt is loss-intolerant (`durable`) and the direct
/// attempt did not land — a transport error, or a
/// [`InjectOutcome::DeferredComposerDirty`] deferral that the director lane has
/// no way to retry on its own.
pub(crate) fn needs_durable_followup(delivered: bool, durable: bool) -> bool {
    durable && !delivered
}

/// Read the task state immediately before an assignment-like prompt crosses a
/// transport boundary. Event revalidation happens earlier in the lifecycle
/// tick, so the task may close in the gap before a direct Teams-inbox/PTY
/// write or a durable queue wake-time flush. Missing or unreadable state is
/// uncertainty and deliberately delivers; only positive terminal evidence
/// suppresses the stale `task start` imperative.
pub(crate) fn assignment_terminal_status(
    cas_dir: &Path,
    prompt: &str,
) -> Option<(String, cas_types::TaskStatus)> {
    let task_id = crate::prompt_revalidation::assignment_solicited_task_id(prompt)?;
    let store = crate::store::open_task_store_local(cas_dir).ok()?;
    let task = store.get(&task_id).ok()?;
    crate::prompt_revalidation::assignment_targets_terminal_task(prompt, task.status)
        .map(|task_id| (task_id, task.status))
}

/// cas-ae6d: hand a director prompt to the durable `prompt_queue` so the
/// readiness-gated, retrying queue lane delivers it instead of the one-shot
/// director lane. Returns the queue row id.
///
/// Uses the same `"director"` source as the spawn-time task brief (cas-28a4),
/// so `process_prompt_queue` applies identical recipient routing and Codex
/// framing — a queued wake-up reaches a Codex worker exactly the way a direct
/// one would have, only with retries behind it.
pub(crate) fn enqueue_director_prompt(
    cas_dir: &std::path::Path,
    factory_session: &str,
    target: &str,
    text: &str,
) -> anyhow::Result<i64> {
    let queue = crate::store::open_prompt_queue_store(cas_dir)?;
    let id = queue.enqueue_with_summary(
        super::teams::DIRECTOR_AGENT_NAME,
        target,
        text,
        Some(factory_session),
        Some("Task assignment"),
    )?;
    wake_daemon_after_enqueue(cas_dir);
    Ok(id)
}

/// Prefix PTY-delivered text with literal sender attribution.
///
/// Emits exactly `Message from <sender>: <text>` — no summary interpolation before
/// the colon — because the Codex prompt (cas-83c8) matches on that literal prefix.
/// `source` is the human-readable sender name ("supervisor" or a worker name).
pub(crate) fn attribute_for_pty(source: &str, text: &str) -> String {
    format!("Message from {source}: {text}")
}

/// The one line typed into a Claude+teams recipient's pane to WAKE it — never
/// the message body (cas-cdf9).
///
/// The body is written once, to the teams inbox, and the recipient's harness
/// renders that copy. Before this, the idle nudge typed the very same body into
/// the pane as well, so every factory message arrived twice: once wrapped in
/// the harness's `<teammate-message>` envelope and once as a bare turn. Both
/// copies carried the identical `CAS provenance:` header, because that header
/// is composed once at `queue_and_events.rs` and handed to both writes — so the
/// recipient had no way to tell the repeat from a new instruction, and a worker
/// that had already acted on the first copy was invited to act again.
///
/// This is the sibling of the rule cas-5fff already enforces for Codex
/// (`codex_recipient_is_never_double_typed_by_the_idle_nudge`): exactly one
/// body-bearing write per message. Codex satisfies it structurally, because its
/// channel is the PTY and no nudge applies. Claude+teams now satisfies it
/// structurally too — [`FactoryDaemon::pty_nudge`] is not given the body at
/// all, so it cannot type it.
///
/// The line still NAMES the notification id rather than being an empty wake, so
/// the wake degrades safely: if the harness ever fails to render the inbox copy,
/// the recipient still learns a message exists and can `inbox_poll` it, instead
/// of being woken to silence.
pub(crate) fn pointer_wake_payload(source: &str, notification_id: Option<i64>) -> String {
    match notification_id {
        Some(id) => format!(
            "CAS wake: message {id} from {source} is in your inbox — see inbox (body not repeated here)."
        ),
        None => format!(
            "CAS wake: a message from {source} is in your inbox — see inbox (body not repeated here)."
        ),
    }
}

/// Apply the shared Codex sender framing when the recipient needs it.
///
/// Used by both normal `deliver_to_worker` PTY injection and the urgent
/// interrupt-and-inject paths (direct + `all_workers` + ClientMessage::Inject).
/// Urgent delivery previously skipped this helper, so Codex recipients saw bare
/// text that their prompt contract does not recognise as an actionable message
/// (cas-ab80). Claude/Grok payloads stay byte-for-byte unchanged.
pub(crate) fn frame_pty_payload(harness: SupervisorCli, source: &str, text: &str) -> String {
    if pty_payload_needs_framing(harness) {
        attribute_for_pty(source, text)
    } else {
        text.to_string()
    }
}

/// Prepare the exact machine turn for PTY injection.
///
/// This is the single delivery authority for the raw harness path. Every
/// regular, urgent, interrupt, director, and lifecycle PTY injection passes
/// here, so hook capture may trust an absent envelope to mean an operator
/// supplied the turn. The sidecar is keyed to the final bytes the harness will
/// submit, never to the rendered provenance line inside those bytes.
pub(crate) fn prepare_pty_machine_delivery(
    cas_dir: &std::path::Path,
    recipient: &str,
    harness: SupervisorCli,
    source: &str,
    text: &str,
    notification_id: Option<i64>,
) -> String {
    let payload = frame_pty_payload(harness, source, text);
    register_pty_machine_payload(cas_dir, recipient, &payload, source, notification_id);
    payload
}

/// Register raw machine input that must not receive sender framing, such as a
/// harness-native reset command. Kept beside [`prepare_pty_machine_delivery`]
/// so this cannot become a second provenance authority.
pub(crate) fn register_pty_machine_payload(
    cas_dir: &std::path::Path,
    recipient: &str,
    payload: &str,
    source: &str,
    notification_id: Option<i64>,
) {
    if let Err(error) = crate::hooks::delivery_provenance::register(
        cas_dir,
        recipient,
        payload,
        crate::hooks::delivery_provenance::origin_for_source(source),
        notification_id,
    ) {
        // Do not make the durable queue unavailable because its supplemental
        // hook metadata failed; retries will submit a freshly registered turn.
        tracing::error!(%error, source, "failed to register machine prompt provenance");
    }
}

/// Commander controls that both daemon client transports route through the
/// same execution seam. Keeping this owned makes the GUI and WebSocket
/// dispatchers thin adapters: neither transport can grow a second interrupt or
/// semantic-message implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CommanderControl {
    InterruptPane {
        pane_id: String,
    },
    SendMessage {
        target: String,
        text: String,
        summary: Option<String>,
        urgent: bool,
        attribution: crate::ui::factory::protocol::MessageAttribution,
    },
}

impl CommanderControl {
    /// Preserve the reviewed per-verb daemon error wording across both
    /// transports while sharing the execution path itself.
    pub(super) fn error_prefix(&self) -> &'static str {
        match self {
            Self::InterruptPane { .. } => "targeted interrupt failed",
            Self::SendMessage { .. } => "semantic message enqueue failed",
        }
    }
}

/// Recognize the additive Commander controls without changing the legacy
/// `ClientMessage::Interrupt` path.
pub(super) fn commander_control_from_message(
    message: &crate::ui::factory::protocol::ClientMessage,
) -> Option<CommanderControl> {
    use crate::ui::factory::protocol::ClientMessage;

    match message {
        ClientMessage::InterruptPane { pane_id } => Some(CommanderControl::InterruptPane {
            pane_id: pane_id.clone(),
        }),
        ClientMessage::SendMessage {
            target,
            text,
            summary,
            urgent,
            attribution,
        } => Some(CommanderControl::SendMessage {
            target: target.clone(),
            text: text.clone(),
            summary: summary.clone(),
            urgent: *urgent,
            attribution: attribution.clone(),
        }),
        _ => None,
    }
}

/// Store one Commander semantic message in the exact prompt queue drained by
/// coordination delivery. Split from the daemon method so parity tests can
/// compare a Commander row with an MCP coordination row in one isolated DB.
pub(super) fn enqueue_commander_message(
    cas_dir: &std::path::Path,
    factory_session: &str,
    target: &str,
    text: &str,
    summary: Option<&str>,
    urgent: bool,
    attribution: &crate::ui::factory::protocol::MessageAttribution,
) -> anyhow::Result<cas_store::EnqueueOutcome> {
    let queue = crate::store::open_prompt_queue_store(cas_dir)?;
    let attribution_json = serde_json::to_value(attribution)?;
    let priority = urgent.then_some(cas_store::NotificationPriority::Critical);
    Ok(queue.enqueue_attributed_urgent_with_outcome(
        &attribution.queue_source(),
        target,
        text,
        Some(factory_session),
        summary,
        priority,
        urgent,
        Some(&attribution_json),
    )?)
}

/// cas-c73d (GH #177): which Claude config dir does this worker's harness run
/// under, if not the daemon's?
///
/// Pure mirror of the resolution [`cas_mux::Mux::add_worker`] applies when it
/// builds the PTY env: an explicit `config_dir` wins, else the config dir of
/// the supervisor that requested the spawn, else no override at all. Keeping it
/// a separate function (rather than inlining the match) is what lets a test pin
/// the precedence against the spawn path without a live daemon — if the two
/// ever disagree, the daemon writes inbox rows into a tree the PTY was not
/// launched with, which is precisely the bug.
/// A RELATIVE value is deliberately rejected: the PTY passes it to the worker
/// verbatim (`push_claude_config_dir_env`, cas-pty), so Claude Code resolves it
/// against the worker's cwd — its worktree — while this daemon would resolve it
/// against `$HOME` (`claude_config_dir_from`, cas-5b96 semantics). Two
/// different trees is the bug, so we decline to guess and keep writing into the
/// daemon's own tree, which is no worse than the pre-fix behaviour and stays
/// visible to the retract sweeps.
pub(crate) fn recipient_config_dir(spec: &cas_mux::WorkerSpec) -> Option<String> {
    let raw = spec
        .config_dir
        .clone()
        .or_else(|| spec.requester_config_dir.clone())?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.starts_with('~') && !std::path::Path::new(trimmed).is_absolute() {
        tracing::warn!(
            target: "cas::coordination",
            config_dir = %trimmed,
            "cas-c73d: relative CLAUDE_CONFIG_DIR resolves differently for the worker's \
             harness than for this daemon — not redirecting inbox delivery"
        );
        return None;
    }
    Some(trimmed.to_string())
}

impl FactoryDaemon {
    /// Execute a Commander control after either client transport recognizes it.
    /// Targeted interrupt reaches the same `Mux::break_turn` primitive used by
    /// urgent coordination delivery, and semantic messages reach the same
    /// durable queue consumed by `process_prompt_queue`.
    pub(super) async fn dispatch_commander_control(
        &self,
        control: CommanderControl,
    ) -> anyhow::Result<()> {
        match control {
            CommanderControl::InterruptPane { pane_id } => self.interrupt_pane_turn(&pane_id).await,
            CommanderControl::SendMessage {
                target,
                text,
                summary,
                urgent,
                attribution,
            } => {
                self.enqueue_attributed_message(
                    &target,
                    &text,
                    summary.as_deref(),
                    urgent,
                    &attribution,
                )?;
                Ok(())
            }
        }
    }

    /// Enqueue a Commander semantic message through the same durable prompt
    /// queue drained by MCP coordination messages. Delivery therefore reuses
    /// the existing inbox+wake and urgent interrupt-and-redirect machinery.
    pub(crate) fn enqueue_attributed_message(
        &self,
        target: &str,
        text: &str,
        summary: Option<&str>,
        urgent: bool,
        attribution: &crate::ui::factory::protocol::MessageAttribution,
    ) -> anyhow::Result<cas_store::EnqueueOutcome> {
        let outcome = enqueue_commander_message(
            self.app.cas_dir(),
            &self.session_name,
            target,
            text,
            summary,
            urgent,
            attribution,
        )?;

        // Match coordination's best-effort wake signal. This daemon will also
        // observe the row on its next queue pass if signaling is unavailable.
        if matches!(outcome, cas_store::EnqueueOutcome::Created(_)) {
            wake_daemon_after_enqueue(self.app.cas_dir());
        }
        Ok(outcome)
    }

    /// Targeted Commander interrupt. Coordination urgent delivery enters the
    /// same canonical `Mux::break_turn` primitive before injecting its message.
    pub(crate) async fn interrupt_pane_turn(&self, pane_id: &str) -> anyhow::Result<()> {
        let actual = self.resolve_pane_name(pane_id);
        self.app.mux.break_turn(&actual).await.map_err(Into::into)
    }

    /// cas-c73d (GH #177): the Agent-Teams tree the RECIPIENT's harness really
    /// reads, when that is not the daemon's own.
    ///
    /// A worker spawned with `config_dir` (`spawn_queue.worker_spec.config_dir`
    /// — the two-account Slack route) runs `claude` with a different
    /// `CLAUDE_CONFIG_DIR`, and Claude Code only polls
    /// `$CLAUDE_CONFIG_DIR/teams/{team}/inboxes/{self}.json`. `self.teams` is
    /// rooted at the DAEMON's config dir, so every normal delivery to such a
    /// worker was written where nothing reads — the observed "worker boots
    /// deaf, only an urgent PTY interrupt lands" failure.
    ///
    /// The resolution order mirrors [`cas_mux::Mux::add_worker`] exactly
    /// (explicit `config_dir`, else the requesting supervisor's) so the tree we
    /// write to is the one the PTY was actually launched with. Returns `None`
    /// — meaning "use `self.teams` unchanged" — for the supervisor, for
    /// workers with no override, for a config dir that resolves to the daemon's
    /// own tree, and if provisioning the mirror fails (a write into the
    /// daemon's tree is no worse than today's behaviour and keeps the row's
    /// retract tags where the sweeps can see them).
    pub(crate) fn recipient_teams_view(
        &self,
        pane_target: &str,
    ) -> Option<super::teams::TeamsManager> {
        let primary = self.teams.as_ref()?;
        if pane_target == self.app.supervisor_name() {
            return None;
        }
        let spec = self.app.mux.effective_worker_spec(pane_target, None);
        let config_dir = recipient_config_dir(&spec)?;
        let view = primary.view_for_config_dir(Some(&config_dir))?;
        if let Err(error) = view.provision_mirror_from(primary, pane_target) {
            tracing::warn!(
                target: "cas::coordination",
                worker = %pane_target,
                config_dir = %config_dir,
                %error,
                "cas-c73d: could not provision the recipient's teams tree — \
                 falling back to the daemon's own tree for this write"
            );
            return None;
        }
        tracing::debug!(
            target: "cas::coordination",
            stage = "recipient_tree_resolved",
            channel = "teams_inbox",
            worker = %pane_target,
            config_dir = %config_dir,
            path = %view.teams_dir().display(),
            "cas-c73d: delivering into the recipient's own config-dir teams tree"
        );
        Some(view)
    }

    /// cas-ae6d (GH #100): should this director prompt be parked on the durable
    /// `prompt_queue` instead of injected directly this tick?
    ///
    /// Resolves the recipient's harness and pane readiness, then defers to the
    /// pure [`route_director_prompt`] decision.
    pub(crate) fn route_director_prompt_to_queue(
        &self,
        prompt: &crate::ui::factory::director::Prompt,
    ) -> bool {
        if !prompt.durable_retry {
            return false;
        }
        let pane_target = if prompt.target == "supervisor" {
            self.app.supervisor_name()
        } else {
            prompt.target.as_str()
        };
        let channel = choose_channel(self.app.harness_for(pane_target), self.teams.is_some());
        let pane_ready = self.app.mux.pane_ready_for_injection(pane_target);
        route_director_prompt(channel, pane_ready, prompt.durable_retry)
            == DirectorPromptRoute::DurableQueue
    }

    /// cas-ae6d: daemon-bound wrapper over [`enqueue_director_prompt`].
    pub(crate) fn enqueue_director_prompt(
        &self,
        prompt: &crate::ui::factory::director::Prompt,
    ) -> anyhow::Result<i64> {
        enqueue_director_prompt(
            self.app.cas_dir(),
            self.session_name.as_str(),
            &prompt.target,
            &prompt.text,
        )
    }

    /// Deliver `text` to `target` over the channel the recipient can actually
    /// read, decided by the recipient's harness (cas-b68a).
    ///
    /// `target` may be the logical name `"supervisor"`, the supervisor's pane
    /// name, or a worker name. `source` is the (already team-resolved) sender
    /// name; `summary` is the optional one-line preview carried to Claude inboxes
    /// and used for PTY attribution.
    ///
    /// `color` overrides the message bubble color when writing to a Claude
    /// Agent-Teams inbox. Pass `Some(DIRECTOR_AGENT_COLOR)` for director
    /// messages so the advertised color matches the config.json record (cas-405f
    /// D-4). Pass `None` for peer/supervisor messages — the team manager resolves
    /// each sender's configured color from the team record.
    ///
    /// `retract_worker` (cas-ed6c): `Some(worker)` when `text` is a
    /// `WorkerIdle`-class alert about `worker` — tags the queued
    /// `TeamsInbox` row so a later sweep (`prune_stale_idle_alerts`) can
    /// retract it if `worker` gains a real assignment before the recipient
    /// ever reads it. `None` for every other prompt kind. Ignored entirely
    /// on the `Pty` channel (no queued row exists there to tag).
    ///
    /// `retract_task` (cas-e48f): `Some(task_id)` when `text` is the
    /// actionable MERGE REQUIRED / `AwaitingMerge` idle alert — tags the
    /// queued row so a later sweep (`prune_stale_merge_alerts`) can retract
    /// it if the merge lands (or the task leaves `AwaitingMerge`) before the
    /// recipient ever reads it. Mutually exclusive with `retract_worker` in
    /// practice (callers pass at most one `Some`); both `None` for every
    /// other prompt kind. Ignored entirely on the `Pty` channel.
    ///
    /// `retract_epic` (cas-06ca): `Some(epic_id)` for the typed
    /// `EpicAllSubtasksClosed` occurrence. It tags a Teams inbox row for
    /// best-effort retraction if the epic closes or a subtask reopens before
    /// the recipient reads it. Already-consumed PTY/inbox messages cannot be
    /// recalled.
    ///
    /// Returns `Delivered` only after a successful write to the chosen
    /// channel. A composer-dirty PTY target returns `DeferredComposerDirty`
    /// so the durable prompt queue can leave its row pending.
    pub(crate) async fn deliver_to_worker(
        &self,
        target: &str,
        source: &str,
        text: &str,
        summary: Option<&str>,
        color: Option<&str>,
        retract_worker: Option<&str>,
        retract_task: Option<&str>,
        retract_epic: Option<&str>,
    ) -> anyhow::Result<InjectOutcome> {
        // Normalise the target into the two name forms the two channels expect:
        //   - `pane_target`  : the real pane id `Mux::inject` routes on
        //   - `inbox_target` : the logical team member name `write_to_inbox` expects
        let pane_target = if target == "supervisor" {
            self.app.supervisor_name()
        } else {
            target
        };
        let inbox_target = if pane_target == self.app.supervisor_name() {
            "supervisor"
        } else {
            pane_target
        };

        let teams_active = self.teams.is_some();
        let harness = self.app.harness_for(pane_target);

        match choose_channel(harness, teams_active) {
            DeliveryChannel::TeamsInbox => {
                // Safe: TeamsInbox is only chosen when teams_active, i.e. teams.is_some().
                let primary = self
                    .teams
                    .as_ref()
                    .expect("TeamsInbox channel requires active teams");
                // cas-c73d: a `config_dir`-spawned worker polls a tree in ITS
                // config dir, not the daemon's. Write where the reader looks.
                let recipient_view = self.recipient_teams_view(pane_target);
                let teams = recipient_view.as_ref().unwrap_or(primary);
                match (retract_worker, retract_task, retract_epic) {
                    (Some(worker), _, _) => teams.write_to_inbox_for_worker_idle(
                        inbox_target,
                        source,
                        text,
                        summary,
                        color,
                        worker,
                    ),
                    (None, Some(task_id), _) => teams.write_to_inbox_for_merge_alert(
                        inbox_target,
                        source,
                        text,
                        summary,
                        color,
                        task_id,
                    ),
                    (None, None, Some(epic_id)) => teams.write_to_inbox_for_epic_completion(
                        inbox_target,
                        source,
                        text,
                        summary,
                        color,
                        epic_id,
                    ),
                    (None, None, None) => {
                        teams.write_to_inbox(inbox_target, source, text, summary, color)
                    }
                }
                .map(|()| InjectOutcome::Delivered)
            }
            DeliveryChannel::Pty => {
                // Frame based on the RECIPIENT's harness, not teams mode: a Codex
                // recipient always gets the literal `Message from <sender>: ` prefix
                // its prompt keys on (even codex-only, teams=None); a Claude
                // recipient reached via the PTY fallback stays byte-for-byte bare.
                // Shared helper also used by urgent interrupt-and-inject (cas-ab80).
                let payload = prepare_pty_machine_delivery(
                    self.app.cas_dir(),
                    pane_target,
                    harness,
                    source,
                    text,
                    None,
                );
                self.app
                    .mux
                    .inject(pane_target, &payload)
                    .await
                    .map_err(Into::into)
            }
        }
    }

    /// Like [`Self::deliver_to_worker`], but when the recipient's channel is
    /// the Claude Agent-Teams inbox AND the caller has established the
    /// recipient is genuinely idle, also PTY-nudge the same payload directly
    /// into the pane (cas-893c).
    ///
    /// Root cause this addresses: `TeamsInbox` delivery is a plain file
    /// write (`TeamsManager::write_to_inbox`). Claude Code polls its inbox at
    /// turn boundaries; a worker parked idle awaiting input has no upcoming
    /// turn boundary, so the write can sit unread indefinitely — the daemon
    /// marks the queue row transport-delivered the moment the write
    /// succeeds, but "transport delivered" is not "received" (cas can't
    /// observe Claude Code's internal read-tracking; see the `read` field
    /// comment on `InboxMessage`). Idle is the one moment a plain (non-
    /// cancelling) PTY inject is safe: there is no in-flight turn to
    /// disturb, so typing the message directly creates a genuine new turn
    /// the same way a human pressing Enter would.
    ///
    /// No-op (beyond the primary delivery) when:
    /// - `worker_is_idle` is false — normal inbox write only, matching prior
    ///   behavior exactly.
    /// - The chosen channel is already `Pty` (Codex, Grok, or a Claude
    ///   recipient in a non-teams factory) — that recipient already got the
    ///   message over the only channel it reads; nudging again would
    ///   double-submit the same text.
    ///
    /// The nudge is best-effort: a failure is logged, not propagated,
    /// because the primary inbox write already succeeded by the time this
    /// runs — the message is not lost, only the idle fast-path failed and
    /// the worker will still see it whenever it next reaches a turn
    /// boundary on its own.
    /// `retract_task` (cas-6eab): `Some(task_id)` when `text` is a worker's
    /// merge request that was still live at transport time — tags the queued
    /// `TeamsInbox` row so `prune_stale_merge_alerts` can retract it if the
    /// merge lands before the supervisor ever reads it. `None` for every other
    /// message. Ignored on the `Pty` channel (no queued row exists to tag).
    pub(crate) async fn deliver_to_worker_with_idle_nudge(
        &self,
        target: &str,
        source: &str,
        text: &str,
        summary: Option<&str>,
        color: Option<&str>,
        wake: super::queue_and_events::WakeDecision,
        retract_task: Option<&str>,
        notification_id: Option<i64>,
    ) -> anyhow::Result<NudgeReport> {
        let primary_outcome = self
            .deliver_to_worker(target, source, text, summary, color, None, retract_task, None)
            .await?;

        if primary_outcome != InjectOutcome::Delivered {
            return Ok(NudgeReport::not_attempted(
                primary_outcome,
                "primary delivery did not complete; no wake attempted",
            ));
        }
        if !wake.allowed {
            return Ok(NudgeReport::not_attempted(
                primary_outcome,
                // cas-9e81: name the signal that decided. The old fixed
                // string ("idle gate declined the wake for this pass") was on
                // 34 of 35 rows during the reported incident and told an
                // operator nothing about which of six conditions vetoed.
                &format!("wake gate declined this pass: {}", wake.reason),
            ));
        }

        self.pty_nudge(target, source, notification_id).await
    }

    /// cas-ef14 (GH #139): the pane-nudge half of
    /// [`Self::deliver_to_worker_with_idle_nudge`], WITHOUT the inbox write.
    ///
    /// Used when the recipient's harness has already drained our inbox copy
    /// into its own pending-message store but never surfaced it as a turn. The
    /// message is not lost and must not be written again (that is the GH #124
    /// storm) — the only thing still owed is the turn, and on this transport a
    /// PTY inject is the only way to create one for a Claude teammate parked at
    /// its prompt.
    ///
    /// A denied `wake` means the wake decision vetoed this pass: the row stays
    /// pending (the caller's `wake_deferred` bookkeeping is unchanged) and a
    /// later poll retries on the re-nudge cadence. Its `reason` is recorded so
    /// the veto is diagnosable (cas-9e81).
    pub(crate) async fn nudge_pane_only(
        &self,
        target: &str,
        source: &str,
        notification_id: Option<i64>,
        wake: super::queue_and_events::WakeDecision,
    ) -> anyhow::Result<NudgeReport> {
        if !wake.allowed {
            return Ok(NudgeReport::not_attempted(
                InjectOutcome::Delivered,
                &format!("wake gate declined this pass: {}", wake.reason),
            ));
        }
        self.pty_nudge(target, source, notification_id).await
    }

    /// cas-dffe (GH #145): execute a queued context-reset control command.
    ///
    /// This is deliberately NOT part of the message-delivery path. A context
    /// reset is a command for the harness itself, so it is typed into the pane
    /// as the harness's own command
    /// ([`crate::factory_context_reset::context_reset_command`]) over the same
    /// interrupt-and-inject channel urgent messages use — never written to a
    /// team inbox, never framed with sender attribution, and never rendered as
    /// readable content. Routing it as a message is exactly the reported bug:
    /// the worker read the four characters "/clear" as a teammate note,
    /// acknowledged them, and kept its whole conversation loaded.
    ///
    /// Returns the queue bookkeeping the caller should apply, so the decision
    /// stays in one place and can't drift from the log lines that explain it.
    pub(crate) async fn deliver_context_reset(&mut self, target: &str) -> ContextResetDelivery {
        use crate::factory_context_reset::context_reset_command;

        let pane_target = if target == "supervisor" {
            self.app.supervisor_name().to_string()
        } else {
            target.to_string()
        };
        let harness = self.app.harness_for(&pane_target);
        let Some(command) = context_reset_command(harness) else {
            return ContextResetDelivery::Unsupported {
                detail: crate::factory_context_reset::unsupported_reason(harness),
            };
        };

        if !self.app.mux.pane_ready_for_injection(&pane_target) {
            return ContextResetDelivery::NotReady;
        }

        let settle = self.urgent_settle_duration(&pane_target);
        tracing::info!(
            target: "cas::coordination",
            stage = "context_reset_inject",
            target_agent = %pane_target,
            harness = harness.backend().name(),
            command = %command,
            settle_ms = settle.as_millis() as u64,
            "cas-dffe: typing the harness's own context-reset command into the pane"
        );
        register_pty_machine_payload(
            self.app.cas_dir(),
            &pane_target,
            command,
            "lifecycle-wake:context-reset",
            None,
        );
        match self
            .app
            .mux
            .interrupt_and_inject(&pane_target, command, settle)
            .await
        {
            Ok(()) => ContextResetDelivery::Injected,
            Err(error) => ContextResetDelivery::Failed {
                detail: format!("pane inject failed: {error}"),
            },
        }
    }

    /// Shared PTY-nudge tail: frame the payload for the recipient's harness and
    /// type it into the pane. Best-effort — every failure mode is logged and the
    /// `InjectOutcome` half stays `Delivered`, because by construction the
    /// recipient already holds an inbox copy by the time this runs.
    ///
    /// cas-7a01 (GH #155): the `WakeAttempt` half is the part that used to be
    /// thrown away. All three arms below returned a bare
    /// `InjectOutcome::Delivered`, so a nudge that fired, a nudge the channel
    /// vetoed and a nudge that errored were indistinguishable to every caller —
    /// and `message_status` had nothing to report but the constant
    /// `wake: unobserved`. The states were always computed here; they are now
    /// carried out instead of discarded.
    async fn pty_nudge(
        &self,
        target: &str,
        source: &str,
        notification_id: Option<i64>,
    ) -> anyhow::Result<NudgeReport> {
        let pane_target = if target == "supervisor" {
            self.app.supervisor_name()
        } else {
            target
        };
        let teams_active = self.teams.is_some();
        let harness = self.app.harness_for(pane_target);
        if !idle_nudge_applies(choose_channel(harness, teams_active)) {
            // Already PTY-delivered by the primary call above.
            return Ok(NudgeReport::not_attempted(
                InjectOutcome::Delivered,
                "recipient channel is PTY; the delivery itself is the turn",
            ));
        }

        // cas-cdf9: a WAKE, not a second copy of the message. The body was
        // written to the inbox by the primary delivery and is rendered from
        // there; retyping it here is what surfaced every factory message twice.
        let payload = prepare_pty_machine_delivery(
            self.app.cas_dir(),
            pane_target,
            harness,
            source,
            &pointer_wake_payload(source, notification_id),
            notification_id,
        );
        let report = match self.app.mux.inject(pane_target, &payload).await {
            Ok(InjectOutcome::Delivered) => {
                tracing::info!(
                    target: "cas::coordination",
                    stage = "idle_nudge",
                    target_agent = %pane_target,
                    "cas-893c: nudged idle worker via PTY in addition to teams-inbox write"
                );
                NudgeReport {
                    outcome: InjectOutcome::Delivered,
                    wake: WakeAttempt::Fired,
                    wake_detail: None,
                }
            }
            Ok(InjectOutcome::DeferredComposerDirty) => {
                tracing::info!(
                    target: "cas::coordination",
                    stage = "idle_nudge_deferred",
                    target_agent = %pane_target,
                    "cas-893c: skipped idle PTY nudge because the operator composer is dirty; inbox write already succeeded"
                );
                NudgeReport {
                    outcome: InjectOutcome::Delivered,
                    wake: WakeAttempt::Failed,
                    wake_detail: Some("operator composer is dirty".to_string()),
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "cas::coordination",
                    stage = "idle_nudge",
                    target_agent = %pane_target,
                    error = %e,
                    "cas-893c: idle PTY nudge failed; inbox write already succeeded, worker will \
                     still see the message at its next natural turn boundary"
                );
                NudgeReport {
                    outcome: InjectOutcome::Delivered,
                    wake: WakeAttempt::Failed,
                    wake_detail: Some(format!("pane inject failed: {e}")),
                }
            }
        };
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commander_attribution() -> crate::ui::factory::protocol::MessageAttribution {
        crate::ui::factory::protocol::MessageAttribution {
            device_id: Some("device-123".to_string()),
            credential_id: Some("credential-456".to_string()),
            device_label: Some("Pippenz phone".to_string()),
            operator_label: Some("Pippenz".to_string()),
            controller_origin: Some("https://commander.example".to_string()),
            request_id: Some("request-789".to_string()),
        }
    }

    /// cas-f65d: exercise the exact transport adapters called by each daemon
    /// client handler. Both must produce the same shared control value for both
    /// additive verbs, while legacy Interrupt remains outside this dispatcher.
    #[test]
    fn gui_and_websocket_route_commander_controls_through_one_dispatch_contract() {
        use crate::ui::factory::protocol::ClientMessage;

        let controls = [
            ClientMessage::InterruptPane {
                pane_id: "worker-1".to_string(),
            },
            ClientMessage::SendMessage {
                target: "worker-1".to_string(),
                text: "checkpoint now".to_string(),
                summary: Some("checkpoint".to_string()),
                urgent: true,
                attribution: commander_attribution(),
            },
        ];

        for message in &controls {
            let gui = super::super::gui_client::commander_control_from_gui_message(message)
                .expect("GUI must recognize Commander control");
            let ws = super::super::ws_client::commander_control_from_ws_message(message)
                .expect("WebSocket must recognize Commander control");
            assert_eq!(gui, ws, "both transports must enter one dispatcher");
            assert_eq!(gui, commander_control_from_message(message).unwrap());
        }

        assert!(
            super::super::gui_client::commander_control_from_gui_message(&ClientMessage::Interrupt)
                .is_none(),
            "legacy focused-pane Interrupt keeps its original transport path"
        );
        assert!(
            super::super::ws_client::commander_control_from_ws_message(&ClientMessage::Interrupt)
                .is_none(),
            "legacy focused-pane Interrupt keeps its original transport path"
        );
    }

    #[test]
    fn codex_recipient_always_pty_even_under_teams() {
        // AC1: a Codex recipient is PTY-delivered even when the supervisor runs
        // native Agent Teams (teams_active = true). This is the core bug fix.
        assert_eq!(
            choose_channel(SupervisorCli::Codex, true),
            DeliveryChannel::Pty
        );
        assert_eq!(
            choose_channel(SupervisorCli::Codex, false),
            DeliveryChannel::Pty
        );
    }

    #[test]
    fn claude_recipient_uses_inbox_when_teams_active_else_pty() {
        // AC3: Claude teammates still go through the team inbox under teams...
        assert_eq!(
            choose_channel(SupervisorCli::Claude, true),
            DeliveryChannel::TeamsInbox
        );
        // ...and fall back to PTY in a non-teams (codex-only / plain PTY) factory.
        assert_eq!(
            choose_channel(SupervisorCli::Claude, false),
            DeliveryChannel::Pty
        );
    }

    #[test]
    fn readiness_gate_required_exactly_for_pty_delivery() {
        // Codex always PTY → always gated (note b: first message was dropped
        // during codex startup because the gate was skipped under teams).
        assert!(requires_pty_readiness_gate(SupervisorCli::Codex, true));
        assert!(requires_pty_readiness_gate(SupervisorCli::Codex, false));
        // Claude under teams → inbox file write, no readline race, no gate.
        assert!(!requires_pty_readiness_gate(SupervisorCli::Claude, true));
        // Claude without teams → PTY → gated.
        assert!(requires_pty_readiness_gate(SupervisorCli::Claude, false));
    }

    /// cas-ae6d (GH #100): an assignment wake-up for a Codex worker whose pane
    /// is not yet ready must be parked on the durable queue, not injected into
    /// a PTY that will swallow it. The Claude-under-teams recipient keeps
    /// direct delivery (inbox file write — durable by construction), which is
    /// why the same assignment woke a claude worker and lost 2/2 codex ones.
    #[test]
    fn durable_director_prompt_queues_when_pty_pane_is_not_ready() {
        assert_eq!(
            route_director_prompt(DeliveryChannel::Pty, false, true),
            DirectorPromptRoute::DurableQueue
        );
        assert_eq!(
            route_director_prompt(DeliveryChannel::Pty, true, true),
            DirectorPromptRoute::InjectNow
        );
        // Teams inbox never needs the queue: the write itself is durable.
        assert_eq!(
            route_director_prompt(DeliveryChannel::TeamsInbox, false, true),
            DirectorPromptRoute::InjectNow
        );
        // Non-durable prompts keep their historical one-shot behavior.
        assert_eq!(
            route_director_prompt(DeliveryChannel::Pty, false, false),
            DirectorPromptRoute::InjectNow
        );
    }

    /// cas-ae6d (GH #100), end to end at the dispatch layer: an assignment
    /// wake-up for a Codex worker whose pane is not ready is parked on the
    /// durable queue as a real, pending, retryable row addressed to that
    /// worker — not dropped. `process_prompt_queue` then delivers it over the
    /// Codex PTY (with framing) once the pane is ready.
    #[test]
    fn assignment_wakeup_for_an_unready_codex_worker_lands_on_the_durable_queue() {
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();

        let prompt = crate::ui::factory::director::generate_prompt_at(
            &crate::ui::factory::director::DirectorEvent::TaskAssigned {
                task_id: "cas-ae6d".to_string(),
                task_title: "Wake the codex worker".to_string(),
                worker: "cosmic-crow-41".to_string(),
            },
            &director_data_fixture(),
            &director_data_fixture(),
            "supervisor",
            &crate::config::AutoPromptConfig::default(),
            SupervisorCli::Claude,
            SupervisorCli::Codex,
            &std::collections::HashSet::new(),
            None,
            chrono::Utc::now(),
        )
        .expect("assignment prompt is generated");

        // The routing decision the daemon makes for this prompt: codex worker
        // (always PTY) whose pane has not signalled readiness.
        assert_eq!(
            route_director_prompt(
                choose_channel(SupervisorCli::Codex, true),
                false,
                prompt.durable_retry,
            ),
            DirectorPromptRoute::DurableQueue
        );

        enqueue_director_prompt(&cas_dir, "factory-session", &prompt.target, &prompt.text).unwrap();

        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let queued = queue.peek_all(10).unwrap();
        assert_eq!(queued.len(), 1, "the wake-up must survive as a pending row");
        assert_eq!(queued[0].target, "cosmic-crow-41");
        assert_eq!(queued[0].source, super::super::teams::DIRECTOR_AGENT_NAME);
        assert!(
            queued[0].prompt.contains("cas-ae6d"),
            "the queued wake-up must name the task: {}",
            queued[0].prompt
        );
        assert!(
            queued[0].prompt.contains("action=start"),
            "the queued wake-up must tell the worker how to pick the task up: {}",
            queued[0].prompt
        );
    }

    /// GH #682: a direct director delivery and a durable wake-time flush share
    /// the same fresh task-state lookup. A task can close after event
    /// revalidation but before either transport writes the assignment.
    #[test]
    fn terminal_assignment_is_suppressed_at_the_final_transport_boundary() {
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        let task_store = crate::store::open_task_store(&cas_dir).unwrap();
        let task_id = "cas-2b0b";
        let mut task = cas_types::Task::new(task_id.to_string(), "closed before delivery".into());
        task.status = cas_types::TaskStatus::Closed;
        task_store.add(&task).unwrap();

        let prompt = format!(
            "You have been assigned a new task:\nTask ID: {task_id}\nStart working: \
             mcp__cs__task action=start id={task_id}\nThen send an ACK to supervisor."
        );
        assert_eq!(
            assignment_terminal_status(&cas_dir, &prompt),
            Some((task_id.to_string(), cas_types::TaskStatus::Closed)),
            "both direct delivery and wake-time queue flush must suppress the stale start instruction"
        );
    }

    fn director_data_fixture() -> crate::ui::factory::director::DirectorData {
        crate::ui::factory::director::DirectorData {
            ready_tasks: vec![],
            in_progress_tasks: vec![],
            epic_tasks: vec![],
            agents: vec![],
            activity: vec![],
            agent_id_to_name: std::collections::HashMap::new(),
            changes: vec![],
            git_loaded: true,
            reminders: vec![],
            epic_closed_counts: std::collections::HashMap::new(),
        }
    }

    /// cas-ae6d: a durable prompt whose direct attempt failed or was deferred
    /// (composer dirty) must be re-queued — the director lane has no retry of
    /// its own and the event detector will never re-emit the assignment.
    #[test]
    fn failed_durable_director_prompt_falls_back_to_the_queue() {
        assert!(needs_durable_followup(false, true));
        assert!(!needs_durable_followup(true, true));
        assert!(!needs_durable_followup(false, false));
        assert!(!needs_durable_followup(true, false));
    }

    /// cas-ae6d: the enqueue-failure path must not launder a delivered-looking
    /// PTY write into an unready pane as a real delivery — `Mux::inject`
    /// returns Delivered on write success, not on the harness reading it.
    #[test]
    fn write_into_a_known_unready_pane_is_not_a_landed_delivery() {
        assert!(!durable_delivery_landed(true, true));
        assert!(durable_delivery_landed(true, false));
        assert!(!durable_delivery_landed(false, false));
        assert!(needs_durable_followup(
            durable_delivery_landed(true, true),
            true
        ));
    }

    #[test]
    fn pty_framing_keys_on_codex_recipient_not_teams_mode() {
        // Codex recipient → framed in EVERY factory (incl. codex-only / teams=None),
        // because the codex prompt (cas-83c8) keys on the literal prefix.
        assert!(pty_payload_needs_framing(SupervisorCli::Codex));
        // Claude recipient via PTY fallback (codex-supervised, teams=None) → bare.
        assert!(!pty_payload_needs_framing(SupervisorCli::Claude));
    }

    /// EPIC cas-8888 (cas-9a31, Phase 1): Grok is always PTY-delivered (no
    /// team-transport) but must NOT be framed like Codex — no such prompt
    /// convention has been authored for Grok (unlike Codex's
    /// CODEX_WORKER_INSTRUCTIONS, which explicitly keys on the literal
    /// prefix), and Grok's design otherwise mirrors Claude's (native hooks,
    /// real TUI textbox). See the doc comment on `pty_payload_needs_framing`
    /// for the full reasoning trail (this was revised once already after
    /// checking the actual mechanism — don't re-flip without re-checking).
    #[test]
    fn pty_framing_does_not_apply_to_grok() {
        assert!(!pty_payload_needs_framing(SupervisorCli::Grok));
    }

    /// cas-6257: the queue-bookkeeping contract. A successful handoff marks the
    /// row processed; a FAILED handoff never does — it is retryable while the
    /// target is live, or abandoned only when the target is gone.
    #[test]
    fn queued_delivery_marks_processed_only_after_successful_handoff() {
        // Success → always MarkProcessed regardless of pane/current flags.
        for pane_known in [true, false] {
            for is_current in [true, false] {
                assert_eq!(
                    classify_queued_delivery(true, pane_known, is_current),
                    QueuedDeliveryOutcome::MarkProcessed,
                    "successful handoff must mark processed (pane_known={pane_known}, current={is_current})"
                );
            }
        }
    }

    #[test]
    fn queued_delivery_failure_to_known_pane_is_retryable() {
        // Inbox write / PTY inject failed but the pane exists → retry, never
        // mark processed (the core "don't falsely advance processed_at" rule).
        assert_eq!(
            classify_queued_delivery(false, true, true),
            QueuedDeliveryOutcome::Retry
        );
        assert_eq!(
            classify_queued_delivery(false, true, false),
            QueuedDeliveryOutcome::Retry
        );
    }

    #[test]
    fn queued_delivery_failure_to_unknown_pane_retries_only_while_current() {
        // Pane gone but target is still a current session member (mid-spawn) →
        // retry so its first message isn't lost.
        assert_eq!(
            classify_queued_delivery(false, false, true),
            QueuedDeliveryOutcome::Retry
        );
        // Pane gone and target is not in this session → abandon so a stale
        // cross-session row cannot wedge the queue forever.
        assert_eq!(
            classify_queued_delivery(false, false, false),
            QueuedDeliveryOutcome::Abandon
        );
    }

    #[test]
    fn attribution_uses_literal_sender_prefix() {
        // Exactly `Message from <sender>: <text>` — the string the codex prompt
        // matches on. No summary interpolation before the colon.
        assert_eq!(
            attribute_for_pty("supervisor", "do the thing"),
            "Message from supervisor: do the thing"
        );
        assert_eq!(
            attribute_for_pty("worker-3", "start cas-1234"),
            "Message from worker-3: start cas-1234"
        );
    }

    /// cas-ab80: urgent Codex delivery must use the same framing contract as
    /// normal PTY delivery. The shared helper is what both paths call.
    #[test]
    fn frame_pty_payload_frames_codex_with_sender_prefix() {
        assert_eq!(
            frame_pty_payload(SupervisorCli::Codex, "supervisor", "stop and re-close"),
            "Message from supervisor: stop and re-close"
        );
        assert_eq!(
            frame_pty_payload(SupervisorCli::Codex, "worker-2", "blocker: need merge"),
            "Message from worker-2: blocker: need merge"
        );
    }

    /// cas-893c: the idle-nudge fast path only applies to the TeamsInbox
    /// channel — Pty-delivered recipients already got the message the one
    /// way they can read it.
    #[test]
    fn idle_nudge_applies_only_to_teams_inbox_channel() {
        assert!(idle_nudge_applies(DeliveryChannel::TeamsInbox));
        assert!(!idle_nudge_applies(DeliveryChannel::Pty));
    }

    /// cas-893c: end-to-end sanity over the real `choose_channel` matrix —
    /// the idle nudge should fire for exactly the shapes where the primary
    /// channel is TeamsInbox (Claude recipient, teams active) and never for
    /// any Pty-delivered shape (Codex/Grok always, or Claude without teams).
    ///
    /// # cas-5fff re-audit — what was and was NOT wrong here
    ///
    /// cas-5fff was filed on the theory that this assertion encoded the wrong
    /// assumption and that the idle nudge needed to be extended to Codex.
    /// Measuring first (live, against `codex` 0.146.0, see
    /// `crates/cas-mux/tests/nonurgent_idle_codex_runtime.rs`) showed the
    /// opposite: **this assertion is correct and is deliberately kept.**
    ///
    /// A Codex recipient is already `Pty`-delivered by the primary call, so
    /// nudging would type the same text into the pane a second time — which,
    /// given the actual defect, would have appended a *second* copy of the
    /// message into the very draft that was already stuck in the composer,
    /// making the wedge worse rather than better. Routing was never the fault.
    ///
    /// The real defect was one layer down, in `Pane::inject_prompt`: the PTY
    /// write landed in full and only the trailing CR was lost, because Codex's
    /// paste-burst detector consumed it as the terminator of an unframed
    /// large write. The message was in the pane the whole time, as an
    /// unsubmitted draft. Fixed by framing Codex injections as an explicit
    /// bracketed paste.
    ///
    /// What cas-893c actually got wrong was not this test but its AC3
    /// **negative result** — "the Codex PTY path is NOT the cause", "don't
    /// re-suspect the Codex PTY injection mechanism itself". That was measured
    /// against an interactive **bash** stand-in because `codex` wasn't
    /// installed in that sandbox, and it was then stated as a general
    /// conclusion. bash accepts a bare write-then-CR; a full-screen TUI with a
    /// paste-burst detector does not. The conclusion was true of the stand-in
    /// and false of the real binary, and it steered three months of
    /// investigation away from the actual line. A harness-behavior claim is
    /// only as good as the harness it was measured against.
    #[test]
    fn idle_nudge_fires_only_for_claude_teams_recipients() {
        for harness in [SupervisorCli::Claude, SupervisorCli::Codex, SupervisorCli::Grok] {
            for teams_active in [true, false] {
                let channel = choose_channel(harness, teams_active);
                let expect_nudge = harness == SupervisorCli::Claude && teams_active;
                assert_eq!(
                    idle_nudge_applies(channel),
                    expect_nudge,
                    "harness={harness:?} teams_active={teams_active} channel={channel:?}"
                );
            }
        }
    }

    /// cas-5fff, stated as its own executable assertion so the reasoning above
    /// can't be lost to a doc-comment edit: a Codex recipient must reach the
    /// PTY exactly ONCE per message. Double-typing an idle Codex pane is not a
    /// harmless retry — before the `inject_prompt` framing fix it compounded
    /// the stuck-draft wedge, and after it, it would submit the message twice.
    #[test]
    fn codex_recipient_is_never_double_typed_by_the_idle_nudge() {
        for teams_active in [true, false] {
            let channel = choose_channel(SupervisorCli::Codex, teams_active);
            assert_eq!(channel, DeliveryChannel::Pty);
            assert!(
                !idle_nudge_applies(channel),
                "an idle Codex worker already got the message over the PTY; a nudge \
                 would type it a second time (teams_active={teams_active})"
            );
        }
    }

    /// cas-cdf9: the other half of cas-5fff's invariant, for the harness where
    /// it was violated by design rather than satisfied by the channel.
    ///
    /// A Claude+teams recipient IS nudged (the test above pins that), and the
    /// nudge used to carry the same body the inbox write already held — so
    /// every factory message was surfaced twice, once as the harness's
    /// `<teammate-message>` render of the inbox copy and once as the bare PTY
    /// turn. Measured on live rows 24508/24515/24535: one prompt_queue row
    /// each, `delivery_attempts=0`, one `transport_delivered_at`, and one
    /// `prompt_queue_recipient_seen` receipt stamped `transport_delivered` —
    /// so this was never redelivery and never the turn-start drain, which
    /// cannot surface a delivered row at all.
    ///
    /// The wake payload must therefore contain no part of the body.
    #[test]
    fn claude_teams_recipient_is_never_double_typed_by_the_idle_nudge() {
        let channel = choose_channel(SupervisorCli::Claude, true);
        assert!(
            idle_nudge_applies(channel),
            "precondition: this is the recipient that DOES get a nudge"
        );

        let body = "Verdict recorded: ver-515b41d9efeb pass on vdispatch-c8c7a08a";
        let text = format!("CAS provenance: notification_id=24508 origin=supervisor-authored\n\n{body}");
        let wake = pointer_wake_payload("supervisor", Some(24508));

        assert!(
            !wake.contains(body),
            "the nudge must not retype the body the inbox write already carries: {wake}"
        );
        assert!(
            !text.contains(&wake),
            "the wake is its own line, not a slice of the delivered text: {wake}"
        );
        // Claude is unframed, so what is typed is exactly the wake line — the
        // framing step cannot smuggle the body back in.
        assert_eq!(
            frame_pty_payload(SupervisorCli::Claude, "supervisor", &wake),
            wake
        );
    }

    /// The wake names the id so it degrades safely: a recipient woken without
    /// a rendered inbox copy can still `inbox_poll` the exact message.
    #[test]
    fn pointer_wake_names_the_notification_id_and_the_inbox() {
        let wake = pointer_wake_payload("supervisor", Some(24535));
        assert!(wake.contains("24535"), "{wake}");
        assert!(wake.contains("see inbox"), "{wake}");
        assert_eq!(wake.lines().count(), 1, "one line, not a second message");

        // An id-less wake still points at the inbox rather than claiming an id.
        let anonymous = pointer_wake_payload("supervisor", None);
        assert!(anonymous.contains("see inbox"), "{anonymous}");
        assert!(!anonymous.contains("message  "), "{anonymous}");
    }

    /// cas-cdf9: the PTY wake is registered against the REAL notification id.
    /// It used to pass `None`, so `delivery_provenance::register` synthesised a
    /// timestamp id and nothing downstream could tie the typed turn back to the
    /// row it came from.
    #[test]
    fn pty_wake_provenance_carries_the_real_notification_id() {
        let temp = tempfile::TempDir::new().expect("temp cas root");
        let payload = prepare_pty_machine_delivery(
            temp.path(),
            "rapid-leopard-25",
            SupervisorCli::Claude,
            "supervisor",
            &pointer_wake_payload("supervisor", Some(24535)),
            Some(24535),
        );

        let provenance = crate::hooks::delivery_provenance::consume(
            temp.path(),
            "rapid-leopard-25",
            &payload,
        )
        .expect("the wake must register provenance the hook can consume");
        assert_eq!(
            provenance.notification_id, 24535,
            "a synthesised id cannot be traced back to the queued row"
        );
    }

    /// cas-c73d (GH #177): the config dir the daemon resolves for a recipient
    /// must be the one its PTY was launched with, or inbox writes go to a tree
    /// nothing reads. Precedence is pinned against `Mux::add_worker` here
    /// (explicit wins over the requesting supervisor's) AND exercised through
    /// the real `build_add_worker_config` env below, so the two cannot drift.
    #[test]
    fn recipient_config_dir_matches_the_spawn_env_precedence_cas_c73d() {
        let spec = |explicit: Option<&str>, requester: Option<&str>| cas_mux::WorkerSpec {
            config_dir: explicit.map(str::to_string),
            requester_config_dir: requester.map(str::to_string),
            ..cas_mux::WorkerSpec::builtin_default()
        };

        assert_eq!(recipient_config_dir(&spec(None, None)), None);
        assert_eq!(
            recipient_config_dir(&spec(Some("~/.claude"), Some("/home/u/.claude-alt"))),
            Some("~/.claude".to_string()),
            "an explicit config_dir wins — this is the live spawn_queue id=605 shape"
        );
        assert_eq!(
            recipient_config_dir(&spec(None, Some("/home/u/.claude-alt"))),
            Some("/home/u/.claude-alt".to_string()),
            "with no explicit override the worker inherits the requesting supervisor's account"
        );

        assert_eq!(
            recipient_config_dir(&spec(Some("relative-dir"), None)),
            None,
            "a relative config dir resolves against the worker's cwd in the PTY and against \
             $HOME here — declining to redirect is the only safe answer"
        );

        // The value must resolve to the SAME directory the PTY was launched
        // with, for every spelling we do accept. Both sides expand `~` from the
        // process HOME, so this half runs under the crate-wide env lock — a
        // concurrent HOME-mutating test would otherwise compare two different
        // homes and fail for a reason that has nothing to do with the contract.
        let guard = crate::test_support::TestEnvGuard::temp_home();
        let home = guard.home().to_path_buf();
        let mux = cas_mux::Mux::new(24, 80);
        for raw in ["~/.claude", "/srv/claude-cfg"] {
            let config = mux.build_add_worker_config(
                "zen-merlin-47",
                std::path::PathBuf::from("/tmp"),
                None,
                "supervisor",
                None,
                Some(spec(Some(raw), Some("/home/u/.claude-alt"))),
            );
            let pty_dir = config
                .env
                .iter()
                .find(|(k, _)| k == "CLAUDE_CONFIG_DIR")
                .map(|(_, v)| std::path::PathBuf::from(v))
                .expect("worker PTY carries CLAUDE_CONFIG_DIR");
            let resolved = crate::ui::factory::daemon::runtime::teams::claude_config_dir_from(
                &home,
                recipient_config_dir(&spec(Some(raw), None)).as_deref(),
            );
            assert_eq!(
                resolved, pty_dir,
                "the daemon must resolve {raw} to the same tree the PTY runs under"
            );
        }
    }

    /// cas-ab80: Claude and Grok stay bare under urgent and normal paths alike.
    #[test]
    fn frame_pty_payload_leaves_claude_and_grok_unframed() {
        let text = "urgent: drop what you are doing";
        assert_eq!(
            frame_pty_payload(SupervisorCli::Claude, "supervisor", text),
            text
        );
        assert_eq!(
            frame_pty_payload(SupervisorCli::Grok, "supervisor", text),
            text
        );
    }
}
