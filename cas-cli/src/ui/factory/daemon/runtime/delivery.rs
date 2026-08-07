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

use super::super::FactoryDaemon;

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
    Ok(queue.enqueue_with_summary(
        super::teams::DIRECTOR_AGENT_NAME,
        target,
        text,
        Some(factory_session),
        Some("Task assignment"),
    )?)
}

/// Prefix PTY-delivered text with literal sender attribution.
///
/// Emits exactly `Message from <sender>: <text>` — no summary interpolation before
/// the colon — because the Codex prompt (cas-83c8) matches on that literal prefix.
/// `source` is the human-readable sender name ("supervisor" or a worker name).
pub(crate) fn attribute_for_pty(source: &str, text: &str) -> String {
    format!("Message from {source}: {text}")
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

impl FactoryDaemon {
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
                let teams = self
                    .teams
                    .as_ref()
                    .expect("TeamsInbox channel requires active teams");
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
                let payload = frame_pty_payload(harness, source, text);
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
        worker_is_idle: bool,
        retract_task: Option<&str>,
    ) -> anyhow::Result<InjectOutcome> {
        let primary_outcome = self
            .deliver_to_worker(target, source, text, summary, color, None, retract_task, None)
            .await?;

        if primary_outcome != InjectOutcome::Delivered || !worker_is_idle {
            return Ok(primary_outcome);
        }

        self.pty_nudge(target, source, text).await
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
    /// `worker_is_idle == false` means the wake decision vetoed this pass: the
    /// row stays pending (the caller's `wake_deferred` bookkeeping is unchanged)
    /// and a later poll retries on the re-nudge cadence.
    pub(crate) async fn nudge_pane_only(
        &self,
        target: &str,
        source: &str,
        text: &str,
        worker_is_idle: bool,
    ) -> anyhow::Result<InjectOutcome> {
        if !worker_is_idle {
            return Ok(InjectOutcome::Delivered);
        }
        self.pty_nudge(target, source, text).await
    }

    /// Shared PTY-nudge tail: frame the payload for the recipient's harness and
    /// type it into the pane. Best-effort — every failure mode is logged and
    /// reported as `Delivered`, because by construction the recipient already
    /// holds an inbox copy by the time this runs.
    async fn pty_nudge(
        &self,
        target: &str,
        source: &str,
        text: &str,
    ) -> anyhow::Result<InjectOutcome> {
        let pane_target = if target == "supervisor" {
            self.app.supervisor_name()
        } else {
            target
        };
        let teams_active = self.teams.is_some();
        let harness = self.app.harness_for(pane_target);
        if !idle_nudge_applies(choose_channel(harness, teams_active)) {
            // Already PTY-delivered by the primary call above.
            return Ok(InjectOutcome::Delivered);
        }

        let payload = frame_pty_payload(harness, source, text);
        match self.app.mux.inject(pane_target, &payload).await {
            Ok(InjectOutcome::Delivered) => {
                tracing::info!(
                    target: "cas::coordination",
                    stage = "idle_nudge",
                    target_agent = %pane_target,
                    "cas-893c: nudged idle worker via PTY in addition to teams-inbox write"
                );
            }
            Ok(InjectOutcome::DeferredComposerDirty) => {
                tracing::info!(
                    target: "cas::coordination",
                    stage = "idle_nudge_deferred",
                    target_agent = %pane_target,
                    "cas-893c: skipped idle PTY nudge because the operator composer is dirty; inbox write already succeeded"
                );
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
            }
        }
        Ok(InjectOutcome::Delivered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
