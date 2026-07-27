use crate::ui::factory::daemon::imports::*;
use crate::ui::factory::director::AgentSummary;

/// Select the next control action without allowing a slow worker spawn to
/// wedge shutdown. Ordinary actions remain FIFO and wait for the in-flight
/// spawn; shutdown is allowed to jump that queue so it can cancel the spawn.
fn take_next_pending_spawn(
    pending: &mut VecDeque<PendingSpawn>,
    spawn_in_flight: bool,
) -> Option<PendingSpawn> {
    if !spawn_in_flight {
        return pending.pop_front();
    }
    let shutdown_index = pending
        .iter()
        .position(|action| matches!(action, PendingSpawn::Shutdown { .. }))?;
    pending.remove(shutdown_index)
}

fn shutdown_targets(
    live_workers: &[String],
    in_flight_worker: Option<&str>,
    count: Option<usize>,
    names: &[String],
) -> Vec<String> {
    if !names.is_empty() {
        return names.to_vec();
    }

    let count = count.unwrap_or(0);
    let mut targets: Vec<String> = if count == 0 {
        live_workers.to_vec()
    } else {
        live_workers.iter().take(count).cloned().collect()
    };
    let should_include_in_flight = count == 0 || targets.len() < count;
    if should_include_in_flight {
        if let Some(worker) = in_flight_worker {
            if !targets.iter().any(|target| target == worker) {
                targets.push(worker.to_string());
            }
        }
    }
    targets
}

impl FactoryDaemon {
    pub(super) fn handle_mux_event(&mut self, event: cas_mux::MuxEvent) {
        match event {
            cas_mux::MuxEvent::PaneOutput { pane_id, data } => {
                // Always buffer raw PTY bytes (warm buffer for future viewers)
                self.buffer_pane_output(&pane_id, &data);
                // Forward to any active web viewers
                self.forward_pane_output(&pane_id, &data);
                // Forward to GUI and WebSocket clients
                self.forward_pane_output_to_gui(&pane_id, &data);
                self.forward_pane_output_to_ws(&pane_id, &data);
            }
            cas_mux::MuxEvent::PaneExited { pane_id, exit_code } => {
                // Notify GUI and WS clients
                self.gui_notify_pane_exited(&pane_id, exit_code);
                let is_supervisor = pane_id == self.app.supervisor_name();
                let is_worker = self.app.worker_names().contains(&pane_id);

                if is_supervisor {
                    // Supervisor exited (either /exit or crash) — shut down the whole factory
                    tracing::info!("Supervisor exited with code {exit_code:?}, shutting down");
                    self.shutdown.store(true, Ordering::Relaxed);
                } else if is_worker {
                    let _ = self.handle_worker_crash(&pane_id, exit_code);
                }
            }
            _ => {}
        }
    }

    /// Handle worker crash
    fn handle_worker_crash(
        &mut self,
        worker_name: &str,
        exit_code: Option<i32>,
    ) -> anyhow::Result<()> {
        let agent_store = open_agent_store(self.app.cas_dir())?;

        // Look up agent by name
        let agent_id = self
            .app
            .director_data()
            .agents
            .iter()
            .find(|a| is_exact_agent_name_match(a, worker_name))
            .map(|a| a.id.clone());

        if let Some(id) = agent_id {
            let _ = agent_store.mark_stale(&id);
        }

        self.app.mark_worker_crashed(worker_name);
        self.dead_workers.insert(worker_name.to_string());

        let exit_info = match exit_code {
            Some(0) => "exited normally".to_string(),
            Some(code) => format!("crashed with exit code {code}"),
            None => "was terminated".to_string(),
        };

        self.app
            .set_error(format!("Worker '{worker_name}' {exit_info}"));
        self.app.notifier().notify_crash(worker_name, &exit_info);

        Ok(())
    }

    /// Check if a message source is a dead (shutdown/crashed) worker.
    ///
    /// Returns true only for sources that were known factory workers but have
    /// since been removed. External sources (openclaw, bridge, etc.) pass through.
    ///
    /// Worker names are reusable: `dead_workers` is insert-only (a name enters it
    /// on shutdown/crash and is never cleared), but a *new, live* worker can be
    /// spawned into a retired worker's name — most commonly a Codex worker
    /// respawned into the name a Claude worker vacated (cas-5a5c). If we keyed the
    /// drop on the name alone, every message from that live worker would be
    /// silently discarded (marked processed with no delivery), which is exactly
    /// the bug that made Codex workers appear to "not communicate": the supervisor
    /// never saw their completion/blocker messages.
    ///
    /// So a source counts as dead only when its name is in `dead_workers` AND no
    /// currently-live worker owns that name. If a live worker holds the name, its
    /// messages must flow.
    fn is_dead_worker_source(&self, source: &str) -> bool {
        Self::source_is_dead(&self.dead_workers, self.app.worker_names(), source)
    }

    /// Pure liveness rule behind [`Self::is_dead_worker_source`], split out so it
    /// is exhaustively unit-testable without constructing a full daemon (cas-5a5c).
    ///
    /// A source is dead iff its name is in the insert-only `dead` set AND no
    /// currently-`live` worker owns that name (name reuse — e.g. a Codex worker
    /// respawned into a retired Claude worker's name — must NOT be treated as dead).
    fn source_is_dead(
        dead: &std::collections::HashSet<String>,
        live: &[String],
        source: &str,
    ) -> bool {
        dead.contains(source) && !live.iter().any(|w| w == source)
    }

    /// Detect idle-like messages that don't carry new information.
    ///
    /// The daemon rate-limits these (1 per 5 min per source) and silently
    /// marks the rest as processed, so any false positive here is a
    /// *dropped message*, not merely a noisy one.
    ///
    /// Matching rules (intentionally strict):
    ///   1. The message must be short (<= 300 chars). A long message with
    ///      "standing by" buried in it is almost certainly a real status
    ///      report, not an idle heartbeat.
    ///   2. The trimmed, lowercased message must **start with** one of the
    ///      stock idle phrases. Substring matches were dropping messages
    ///      like "Fix 1 for the WorkerIdle debounce race" or "the idle
    ///      detector emits …" — both of which contain the literal word
    ///      "idle" but are clearly not idle heartbeats.
    ///
    /// Previously this used unanchored substring matches including a bare
    /// `"idle"` and the phrase `"mcp tools unavailable"`, which produced
    /// false positives on legitimate status/debug messages. See cas-f9e8.
    fn is_idle_message(text: &str) -> bool {
        const MAX_IDLE_LEN: usize = 300;

        // Stock idle heartbeats workers are instructed to send when they
        // have nothing to do. Must be lowercase and pre-trimmed for the
        // `starts_with` check below to be meaningful.
        const IDLE_PREFIXES: &[&str] = &[
            "standing by",
            "ready for task",
            "ready for tasks",
            "awaiting instructions",
            "awaiting task",
            "awaiting tasks",
            "waiting for work",
            "no task assigned",
            "no tasks assigned",
        ];

        if text.len() > MAX_IDLE_LEN {
            return false;
        }
        let lower = text.trim().to_lowercase();
        IDLE_PREFIXES.iter().any(|prefix| lower.starts_with(prefix))
    }

    /// cas-893c: whether `target` looks idle enough that a plain (non-
    /// cancelling) PTY nudge is safe right now.
    ///
    /// Deliberately independent of the director's `consecutive_idle_ticks`
    /// debounce (`director/events.rs`) — that machinery exists to avoid
    /// flooding the *supervisor* with repeated `WorkerIdle` notifications,
    /// and once a supervisor message lands during an idle streak it sets
    /// `idle_handled_by_supervisor`, which permanently short-circuits that
    /// debounce for the rest of the streak (see task notes on cas-893c).
    /// This is a different question — "is it safe to type into this pane
    /// right now" — answered fresh on every delivery attempt, using the same
    /// fresh-heartbeat + recent-activity signals the director uses
    /// (`FRESH_HEARTBEAT_SECS` / `RECENT_ACTIVITY_SECS`) so the two notions
    /// of "idle" in this codebase stay consistent without sharing mutable
    /// state.
    ///
    /// Conservative on missing data: an unknown agent (e.g. mid-spawn) is
    /// treated as NOT idle so delivery falls back to the plain inbox write
    /// rather than guessing.
    fn worker_looks_idle(
        data: &crate::ui::factory::director::DirectorData,
        target: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> bool {
        use crate::ui::factory::director::{FRESH_HEARTBEAT_SECS, RECENT_ACTIVITY_SECS};

        let Some(agent) = data.agents.iter().find(|a| a.name == target) else {
            return false;
        };
        if agent.current_task.is_some() {
            return false;
        }
        let has_fresh_heartbeat = agent
            .last_heartbeat
            .map(|hb| {
                let age_secs = (now - hb).num_seconds();
                age_secs >= 0 && age_secs < FRESH_HEARTBEAT_SECS
            })
            .unwrap_or(false);
        let has_recent_activity = agent
            .latest_activity
            .as_ref()
            .map(|(_, ts)| {
                let age_secs = (now - *ts).num_seconds();
                age_secs >= 0 && age_secs < RECENT_ACTIVITY_SECS
            })
            .unwrap_or(false);
        !(has_fresh_heartbeat && has_recent_activity)
    }

    /// Minimum settle floor between an urgent turn-break (Esc) and the
    /// follow-up inject (cas-c931 / cas-4208).
    ///
    /// This used to be treated as the *entire* wait — this function snapshot-
    /// logged `bytes_received` and then returned a flat constant that ignored
    /// it, so nothing ever detected whether the child had actually finished
    /// cancelling the turn before the daemon typed into it. A live repro
    /// against a real `codex` binary (task cas-4208 notes) proved that gap is
    /// real: Codex's TUI shows a transitional "Conversation interrupted"
    /// banner after Esc, and typing (especially the trailing submit CR) while
    /// that transition is still in flight gets silently swallowed, leaving
    /// the correction stuck as an unsent draft — every later message just
    /// types more text into the same stuck draft instead of delivering,
    /// matching the reported "turn_aborted then permanent silence" symptom.
    ///
    /// The real fix — actively polling `Mux::pane_bytes_received` for
    /// genuine output quiescence instead of guessing a constant — now lives
    /// in `Mux::interrupt_and_inject` (Codex only; Claude/Grok keep this flat
    /// floor verbatim per the cas-4208 control-group evidence that a flat
    /// sleep already works fine for them). This function's job has therefore
    /// narrowed to just returning that floor.
    ///
    /// 1200ms remains the starting value for CC's turn-cancel latency. We do
    /// NOT vary it per CLI here: `Pane::inject_prompt`'s Codex-vs-Claude split
    /// is an *input-buffer* settle, a different quantity from *turn-cancel*
    /// latency, so inferring a Codex-specific floor delta would be
    /// unvalidated — Codex's extra safety margin instead comes from the
    /// quiescence poll, not a bigger flat number.
    pub(super) fn urgent_settle_duration(&self, pane_target: &str) -> std::time::Duration {
        let bytes_before = self.app.mux.pane_bytes_received(pane_target).unwrap_or(0);
        tracing::debug!(
            target: "cas::coordination",
            stage = "urgent_settle",
            target_agent = %pane_target,
            bytes_before,
            "urgent interrupt settle snapshot (diagnostic only — actual gating is Mux::interrupt_and_inject's quiescence poll)"
        );
        // 1200ms: comfortably above CC's turn-cancel latency while staying well
        // under the daemon's prompt poll interval so delivery stays prompt.
        std::time::Duration::from_millis(1200)
    }

    /// Process prompt queue
    pub(super) async fn process_prompt_queue(&mut self) -> anyhow::Result<()> {
        use cas_store::{EventStore, SqliteEventStore};
        use cas_types::{Event, EventEntityType, EventType};

        let queue = open_prompt_queue_store(self.app.cas_dir())?;

        // Build target list: this session's supervisor + workers + "all_workers".
        // This prevents us from consuming messages meant for a different factory
        // session running in the same project directory.
        let supervisor_name = self.app.supervisor_name().to_string();
        let worker_names = self.app.worker_names().to_vec();
        // cas-73c8: include `director` so outbound replies to the permanent
        // team director member are delivered (write_to_inbox / PTY), matching
        // inbound director → agent delivery. Without this, messages queued
        // to target=director sat forever while registration reported them
        // as "not yet registered".
        let mut targets: Vec<&str> = Vec::with_capacity(worker_names.len() + 3);
        targets.push(&supervisor_name);
        targets.push("all_workers");
        targets.push(super::teams::DIRECTOR_AGENT_NAME);
        for w in &worker_names {
            targets.push(w.as_str());
        }

        // Peek first, only ack after successful injection to provide at-least-once delivery.
        // Filter by targets AND session to prevent cross-session message theft.
        let prompts = queue.peek_for_targets(&targets, Some(&self.session_name), 10)?;

        if !prompts.is_empty() {
            tracing::info!("Processing {} prompts from queue", prompts.len());

            // cas-f9e8 telemetry: record the wait each message spent in the
            // queue before the daemon picked it up. The gap between `now`
            // and `queued.created_at` is the queue→deliver latency, which
            // is what the P99 SLO targets. Logged at debug; enable via
            // `RUST_LOG=cas::coordination=debug`.
            // cas-2c5f: stamp selected_at so message_status can report the
            // Selected stage (authoritative daemon observation).
            let now = chrono::Utc::now();
            for queued in &prompts {
                let _ = queue.record_selected(queued.id);
                let wait_ms = (now - queued.created_at).num_milliseconds();
                tracing::debug!(
                    target: "cas::coordination",
                    stage = "daemon_pickup",
                    channel = "prompt_queue",
                    message_id = queued.id,
                    source = %queued.source,
                    target_agent = %queued.target,
                    priority = ?queued.priority,
                    wait_ms,
                    "prompt_queue message picked up by daemon"
                );
            }
        }

        // Best-effort event recording (for external tooling acks, activity feed, playback).
        let event_store = SqliteEventStore::open(self.app.cas_dir()).ok();

        // Build a set of agent names with native_extension=true.
        // These agents handle message delivery via their own extension
        // so we skip PTY text injection for them.
        let native_agents: std::collections::HashSet<String> = open_agent_store(self.app.cas_dir())
            .ok()
            .and_then(|store| store.list(None).ok())
            .map(|agents| {
                agents
                    .into_iter()
                    .filter(|a| {
                        a.metadata
                            .get("native_extension")
                            .map(|v| v == "true")
                            .unwrap_or(false)
                    })
                    .map(|a| a.name)
                    .collect()
            })
            .unwrap_or_default();

        for queued in prompts {
            let target = &queued.target;

            // Suppress messages from workers that have been shut down or crashed.
            // These workers are no longer in the session and their messages (especially
            // idle notifications) would just add noise to the supervisor context.
            if self.is_dead_worker_source(&queued.source) {
                tracing::debug!(
                    prompt_id = queued.id,
                    source = %queued.source,
                    target = %queued.target,
                    "Dropping message from dead worker"
                );
                // cas-2c5f: not transport delivery — structured stage=dropped.
                let _ = queue.mark_dropped(queued.id, Some("source worker is dead/shut down"));
                continue;
            }

            // Dedup idle-like messages from the same worker (max 1 per 5 minutes).
            // Workers often send repeated "standing by", "ready", "idle" messages
            // that flood the supervisor context without adding information.
            // Urgent (interrupt-and-redirect) messages are never deduped — the
            // supervisor sent them precisely to break the recipient's turn.
            if !queued.urgent && Self::is_idle_message(&queued.prompt) {
                let now = std::time::Instant::now();
                let dominated = self
                    .last_idle_message_times
                    .get(&queued.source)
                    .is_some_and(|last| {
                        now.duration_since(*last) < std::time::Duration::from_secs(300)
                    });
                if dominated {
                    tracing::debug!(
                        prompt_id = queued.id,
                        source = %queued.source,
                        "Suppressing duplicate idle message (rate-limited to 5min)"
                    );
                    // cas-2c5f: not transport delivery — structured stage=suppressed.
                    let _ = queue.mark_suppressed(
                        queued.id,
                        Some("duplicate idle message rate-limited (5min)"),
                    );
                    continue;
                }
                self.last_idle_message_times
                    .insert(queued.source.clone(), now);
            }

            // Skip PTY injection for native extension agents that use plain PTY mode —
            // they poll the queue and deliver messages via their own extension API.
            //
            // cas-7210 AC4: this used to be a bare `continue` — the row stayed
            // `processed_at IS NULL` (correctly, since this daemon isn't the
            // one delivering it) but left no forensic trail explaining why, so
            // `message_status` on such a row looked identical to a message
            // stuck for an unknown/unexplained reason. Record the (accurate,
            // non-blocking) reason so the distinction is visible without
            // changing the retry/ownership semantics at all.
            if target != "all_workers" && native_agents.contains(target.as_str()) {
                let _ = queue.record_pending_reason(
                    queued.id,
                    cas_store::PendingReason::AwaitingDelivery,
                    Some("native extension agent — delivered via its own polling API, not daemon PTY/inbox"),
                );
                continue;
            }

            // Gate PTY injection on pane readiness: harnesses flush the PTY input
            // buffer during startup, so text written before readline initialization
            // is silently lost. Wait for output + a grace period before injecting.
            //
            // The gate applies to any PTY-delivered recipient. Originally this was
            // `self.teams.is_none()` (everyone PTY-delivered in a non-teams factory).
            // cas-b68a note b adds the missing case: a Codex recipient is PTY-delivered
            // even under a Claude teams supervisor, and its *first* message was being
            // dropped because the gate was skipped whenever `teams.is_some()`.
            //
            // cas-c931: an urgent message always takes the PTY interrupt-and-redirect
            // path (even for a Claude recipient under teams), so it needs a ready pane
            // too. `all_workers` is not a single pane, so harness/readiness can't be
            // resolved for it — it keeps the original `teams.is_none()` semantics
            // exactly (the per-worker loop below resolves each worker, urgent included,
            // individually). Claude inbox writes need no gate (plain file write).
            {
                let pane_target = if target == "supervisor" {
                    self.app.supervisor_name()
                } else {
                    target.as_str()
                };
                let pty_delivered = self.teams.is_none()
                    || (target != "all_workers"
                        && (queued.urgent
                            || super::delivery::requires_pty_readiness_gate(
                                self.app.harness_for(pane_target),
                                true,
                            )));
                if pty_delivered && !self.app.mux.pane_ready_for_injection(pane_target) {
                    // Don't ack — the prompt stays in the queue for the next tick.
                    // cas-2c5f: durable pending reason for message_status.
                    let _ = queue.record_pending_reason(
                        queued.id,
                        cas_store::PendingReason::GatedNotReady,
                        Some("pane not ready for injection"),
                    );
                    continue;
                }
            }

            let prompt_with_instructions = queued.prompt.clone();
            let preview: String = queued.prompt.chars().take(50).collect();

            // Resolve the queue source to a valid team member name for inbox writes.
            // The source must be a registered team member name for Claude Code to
            // accept it. The supervisor's team name is "supervisor" (not the generated
            // pane name), so we also accept the pane name and map it.
            let inbox_source = if self.teams.is_some() {
                let src = queued.source.as_str();
                if src == "supervisor"
                    || worker_names.iter().any(|w| w == src)
                    || src == super::teams::DIRECTOR_AGENT_NAME
                {
                    queued.source.clone()
                } else if src == supervisor_name {
                    "supervisor".to_string()
                } else {
                    super::teams::DIRECTOR_AGENT_NAME.to_string()
                }
            } else {
                queued.source.clone()
            };

            tracing::info!("Injecting prompt to '{}': {}", target, preview);

            let record_injection = |store: &SqliteEventStore,
                                    prompt_id: i64,
                                    queue_source: &str,
                                    queue_target: &str,
                                    actual_target: &str,
                                    status: &str,
                                    error: Option<String>| {
                let mut meta = serde_json::json!({
                    "prompt_id": prompt_id,
                    "queue_source": queue_source,
                    "queue_target": queue_target,
                    "actual_target": actual_target,
                    "status": status,
                });
                if let Some(err) = error {
                    meta["error"] = serde_json::Value::String(err);
                }
                let summary =
                    format!("Injected queued prompt {prompt_id} to {actual_target} ({status})");
                let ev = Event::new(
                    EventType::SupervisorInjected,
                    EventEntityType::Agent,
                    actual_target,
                    summary,
                )
                .with_metadata(meta);
                let _ = store.record(&ev);
            };

            let mut success = false;
            if target == "all_workers" {
                // cas-2c5f: truthful broadcast outcomes — never stamp full
                // Delivered on any_success. Count intended/succeeded/failed.
                let workers: Vec<String> = self
                    .app
                    .worker_names()
                    .iter()
                    .filter(|name| {
                        // Skip native extension agents (they self-serve via extension polling).
                        !native_agents.contains(name.as_str())
                    })
                    .cloned()
                    .collect();
                tracing::info!("all_workers target, workers: {:?}", workers);
                let attempted = workers.len() as u32;
                if attempted == 0 {
                    let _ = queue.mark_broadcast_outcome(
                        queued.id,
                        0,
                        0,
                        0,
                        Some("no non-native workers for all_workers broadcast"),
                    );
                    continue;
                }
                let mut succeeded: u32 = 0;
                let mut failed: u32 = 0;
                let mut fail_notes: Vec<String> = Vec::new();
                for name in &workers {
                    // For urgent broadcasts, skip workers whose pane isn't ready
                    // yet — count as failed for this tick (not full delivery).
                    if queued.urgent && !self.app.mux.pane_ready_for_injection(name) {
                        failed += 1;
                        fail_notes.push(format!("{name}: pane not ready"));
                        continue;
                    }
                    let inject_result: anyhow::Result<()> = if queued.urgent {
                        // cas-ab80: urgent Codex recipients still need the shared
                        // `Message from <sender>:` framing (same contract as
                        // normal `deliver_to_worker`); Claude/Grok stay bare.
                        let harness = self.app.harness_for(name);
                        let payload = super::delivery::frame_pty_payload(
                            harness,
                            &inbox_source,
                            &prompt_with_instructions,
                        );
                        let settle = self.urgent_settle_duration(name);
                        self.app
                            .mux
                            .interrupt_and_inject(name, &payload, settle)
                            .await
                            .map_err(Into::into)
                    } else {
                        // Recipient-aware routing (cas-b68a): each worker may run a
                        // different harness, so resolve per-worker inside the loop.
                        // color=None: peer/supervisor senders; team manager resolves
                        // configured color from the sender's team record.
                        self.deliver_to_worker(
                            name,
                            &inbox_source,
                            &prompt_with_instructions,
                            queued.summary.as_deref(),
                            None,
                            None,
                        )
                        .await
                    };
                    match inject_result {
                        Ok(_) => {
                            succeeded += 1;
                            tracing::info!("Injected to worker '{}'", name);
                            if let Some(ref store) = event_store {
                                record_injection(
                                    store,
                                    queued.id,
                                    &queued.source,
                                    &queued.target,
                                    name,
                                    "ok",
                                    None,
                                );
                            }
                        }
                        Err(e) => {
                            failed += 1;
                            fail_notes.push(format!("{name}: {e}"));
                            tracing::error!("Failed to inject to '{}': {}", name, e);
                            if let Some(ref store) = event_store {
                                record_injection(
                                    store,
                                    queued.id,
                                    &queued.source,
                                    &queued.target,
                                    name,
                                    "error",
                                    Some(e.to_string()),
                                );
                            }
                        }
                    }
                }
                let detail = if fail_notes.is_empty() {
                    None
                } else {
                    Some(fail_notes.join("; "))
                };
                if let Err(e) = queue.mark_broadcast_outcome(
                    queued.id,
                    attempted,
                    succeeded,
                    failed,
                    detail.as_deref(),
                ) {
                    tracing::error!(
                        "Failed to stamp broadcast outcome for prompt {}: {}",
                        queued.id,
                        e
                    );
                }
                // Do not fall through to mark_transport_delivered — outcome already stamped.
                continue;
            } else {
                // Resolve the pane name for diagnostics / event records. Delivery
                // itself (channel selection + name normalisation) is handled by the
                // recipient-aware helper (cas-b68a).
                // Owned (not `&str` borrowed from `self.app`): cas-4208's
                // `interrupt_and_inject` now takes `&mut self.app.mux` to
                // actively drain during its quiescence poll, so this name
                // must not keep `self.app` immutably borrowed across that
                // call.
                let pane_target = if target == "supervisor" {
                    self.app.supervisor_name().to_string()
                } else {
                    target.clone()
                };
                let inject_result: anyhow::Result<()> = if queued.urgent {
                    // Urgent: interrupt-and-redirect by name via the PTY,
                    // bypassing the inbox even in teams mode. Break the turn
                    // (Esc), wait the bounded settle window for the turn to
                    // actually break, then inject.
                    // cas-ab80: apply shared Codex framing before inject so
                    // urgent direct delivery matches normal PTY framing.
                    let harness = self.app.harness_for(&pane_target);
                    let payload = super::delivery::frame_pty_payload(
                        harness,
                        &inbox_source,
                        &prompt_with_instructions,
                    );
                    let settle = self.urgent_settle_duration(&pane_target);
                    tracing::info!(
                        target: "cas::coordination",
                        stage = "urgent_interrupt",
                        message_id = queued.id,
                        target_agent = %pane_target,
                        settle_ms = settle.as_millis() as u64,
                        "urgent message: breaking turn then injecting"
                    );
                    self.app
                        .mux
                        .interrupt_and_inject(&pane_target, &payload, settle)
                        .await
                        .map_err(Into::into)
                } else {
                    // Recipient-aware routing (cas-b68a): delivery channel +
                    // name normalisation handled inside the helper.
                    // color=None: peer/supervisor senders; team manager resolves
                    // configured color from the sender's team record.
                    //
                    // cas-893c: when the target is a worker (not the
                    // supervisor) and looks genuinely idle right now, also
                    // PTY-nudge the teams-inbox write so it isn't left
                    // sitting in a file nobody is polling.
                    let worker_is_idle = target != "supervisor"
                        && Self::worker_looks_idle(
                            self.app.director_data(),
                            target.as_str(),
                            chrono::Utc::now(),
                        );
                    self.deliver_to_worker_with_idle_nudge(
                        target,
                        &inbox_source,
                        &prompt_with_instructions,
                        queued.summary.as_deref(),
                        None,
                        worker_is_idle,
                    )
                    .await
                };
                match inject_result {
                    Ok(_) => {
                        success = true;
                        // cas-f9e8 telemetry: end-to-end delivery latency
                        // measured from the sender-assigned `created_at` to
                        // the moment the daemon completed the inbox write.
                        // This is the number the P99 SLO tracks.
                        let deliver_ms =
                            (chrono::Utc::now() - queued.created_at).num_milliseconds();
                        tracing::info!(
                            target: "cas::coordination",
                            stage = "delivered",
                            channel = "prompt_queue",
                            message_id = queued.id,
                            source = %queued.source,
                            target_agent = %pane_target,
                            deliver_ms,
                            "prompt_queue message delivered to inbox"
                        );
                        if let Some(ref store) = event_store {
                            record_injection(
                                store,
                                queued.id,
                                &queued.source,
                                &queued.target,
                                &pane_target,
                                "ok",
                                None,
                            );
                        }
                    }
                    Err(e) => {
                        // cas-6257 + cas-2c5f: centralised bookkeeping via
                        // classify_queued_delivery — a FAILED handoff is never
                        // marked processed/transport-delivered. It is retryable
                        // while the target is a live/known session member
                        // (transient inbox/PTY failure, or the pane is still
                        // spawning), and abandoned only when the target is not a
                        // pane in this session and not a current worker/supervisor.
                        // `success` stays false in the retry cases so the row is
                        // left pending (processed_at not advanced). The structured
                        // cas-2c5f status (pending reason / abandoned) is stamped
                        // alongside so message_status stays truthful about *why* a
                        // row is still pending.
                        let pane_known = self.app.mux.get(&pane_target).is_some();
                        let target_is_current =
                            self.app.worker_names().contains(&pane_target.to_string())
                                || pane_target == self.app.supervisor_name();
                        match super::delivery::classify_queued_delivery(
                            false,
                            pane_known,
                            target_is_current,
                        ) {
                            super::delivery::QueuedDeliveryOutcome::MarkProcessed => {
                                // Unreachable for delivered_ok=false, but handle
                                // defensively: leave the row pending rather than
                                // falsely advancing processed_at on a failed write.
                            }
                            super::delivery::QueuedDeliveryOutcome::Retry => {
                                if pane_known {
                                    // Pane exists — transient adapter failure;
                                    // record it and leave the row pending for the
                                    // next tick.
                                    tracing::error!("Failed to inject to '{}': {}", pane_target, e);
                                    // cas-2c5f: adapter failure leaves row pending for retry.
                                    let _ = queue.record_pending_reason(
                                        queued.id,
                                        cas_store::PendingReason::AdapterRetryable,
                                        Some(&e.to_string()),
                                    );
                                    if let Some(ref store) = event_store {
                                        record_injection(
                                            store,
                                            queued.id,
                                            &queued.source,
                                            &queued.target,
                                            &pane_target,
                                            "error",
                                            Some(e.to_string()),
                                        );
                                    }
                                } else {
                                    // Pane missing but the target is still a current
                                    // session member (mid-spawn) — bare retry.
                                    // cas-2c5f: known target still spawning — retryable unavail.
                                    let _ = queue.record_pending_reason(
                                        queued.id,
                                        cas_store::PendingReason::TargetUnavailable,
                                        Some("pane missing; target is current session member"),
                                    );
                                }
                            }
                            super::delivery::QueuedDeliveryOutcome::Abandon => {
                                // Pane not found and not a current worker/supervisor.
                                // Stale messages for workers from previous sessions
                                // would otherwise block the queue forever (peek_all
                                // has a limit), so consume the row and re-route its
                                // content to the supervisor.
                                tracing::warn!(
                                    prompt_id = queued.id,
                                    target = pane_target,
                                    source = %queued.source,
                                    "Abandoning queued prompt for unknown target — \
                                     message will not be delivered"
                                );
                                // cas-2c5f: not transport delivery — structured stage=abandoned.
                                let _ = queue.mark_abandoned(
                                    queued.id,
                                    Some(&format!(
                                        "target '{pane_target}' not found in current session"
                                    )),
                                );

                                // Record the drop and notify the supervisor so the
                                // message isn't silently lost.
                                if let Some(ref store) = event_store {
                                    record_injection(
                                        store,
                                        queued.id,
                                        &queued.source,
                                        &queued.target,
                                        &pane_target,
                                        "abandoned",
                                        Some(format!(
                                            "Target '{}' not found in current session",
                                            pane_target
                                        )),
                                    );
                                }

                                // Re-queue to supervisor so the message content isn't
                                // lost. The supervisor can then re-assign or re-send.
                                let notice = format!(
                                    "<system-notice>\n\
                                     Undelivered message from '{}' to '{}' (target not in session):\n\n\
                                     {}\n\
                                     </system-notice>",
                                    queued.source, pane_target, &queued.prompt
                                );
                                let _ = queue.enqueue_with_session(
                                    super::teams::DIRECTOR_AGENT_NAME,
                                    self.app.supervisor_name(),
                                    &notice,
                                    &self.session_name,
                                );
                            }
                        }
                    }
                }
            }

            if success {
                // cas-2c5f: authoritative transport handoff only.
                if let Err(e) = queue.mark_transport_delivered(queued.id) {
                    tracing::error!(
                        "Failed to mark prompt {} as transport-delivered: {}",
                        queued.id,
                        e
                    );
                }
            }
        }

        Ok(())
    }

    /// Poll the spawn queue and enqueue individual actions (non-blocking).
    ///
    /// Instead of spawning workers synchronously (which blocks the TUI for seconds),
    /// this converts spawn requests into individual PendingSpawn items that are
    /// processed one-per-tick in the main loop.
    pub(super) fn enqueue_spawn_requests(&mut self) -> anyhow::Result<()> {
        let queue = open_spawn_queue_store(self.app.cas_dir())?;
        let requests = queue.poll(&self.session_name, 10)?;

        for request in requests {
            tracing::info!("Enqueuing spawn request: {:?}", request.action);
            match request.action {
                SpawnAction::Spawn => {
                    let count = request.count.unwrap_or(1) as usize;
                    let isolate = request.isolate;
                    // cas-2992: deserialize the optional WorkerSpec from the queue row.
                    // Invalid JSON is logged and treated as "no override" so a corrupt row
                    // does not block all subsequent spawns.
                    let spec: Option<cas_mux::WorkerSpec> = request
                        .worker_spec
                        .as_deref()
                        .and_then(|json| match serde_json::from_str(json) {
                            Ok(s) => Some(s),
                            Err(e) => {
                                tracing::warn!(
                                    "spawn queue: invalid worker_spec JSON ({}); using session default",
                                    e
                                );
                                None
                            }
                        });
                    // cas-6913: task_id pre-assigns a task to the spawned
                    // worker. The MCP layer (factory_spawn_workers) already
                    // rejects any request where this would be ambiguous
                    // (count != 1 / more than one worker_names entry), so
                    // in practice this loop only ever runs once when
                    // task_id is Some. Defensively `.take()` it anyway so a
                    // future caller that enqueues the store directly with a
                    // multi-worker request can't accidentally assign the
                    // same task to every spawned worker.
                    let mut task_id = request.task_id;
                    if request.worker_names.is_empty() {
                        self.app.spawning_count += count;
                        for _ in 0..count {
                            self.pending_spawns.push_back(PendingSpawn::Anonymous {
                                isolate,
                                spec: spec.clone(),
                                task_id: task_id.take(),
                            });
                        }
                    } else {
                        self.app.spawning_count += request.worker_names.len();
                        for name in request.worker_names {
                            self.pending_spawns.push_back(PendingSpawn::Named {
                                name,
                                isolate,
                                spec: spec.clone(),
                                task_id: task_id.take(),
                            });
                        }
                    }
                }
                SpawnAction::Shutdown => {
                    self.pending_spawns.push_back(PendingSpawn::Shutdown {
                        count: request.count.map(|c| c as usize),
                        names: request.worker_names,
                        force: request.force,
                    });
                }
                SpawnAction::Respawn => {
                    for name in request.worker_names {
                        self.pending_spawns.push_back(PendingSpawn::Respawn(name));
                    }
                }
            }
        }

        Ok(())
    }

    /// Process pending spawn actions without blocking the main loop.
    ///
    /// Git worktree creation (the slow part) runs on a background thread via
    /// `spawn_blocking`. Only one background spawn runs at a time. Each tick we
    /// either: (a) check if the in-flight spawn finished, or (b) start a new one.
    pub(super) async fn process_pending_spawns(&mut self) {
        // Step 1: Check if in-flight background spawn completed
        let spawn_finished = self
            .spawn_task
            .as_ref()
            .map(|(_, _, _, handle)| handle.is_finished())
            .unwrap_or(false);
        if spawn_finished {
            let (pending_name, pending_spec, pending_task_id, handle) =
                self.spawn_task.take().unwrap();
            // Remove from pending workers (boot pane transitions to real pane or disappears)
            self.app.remove_pending_worker(&pending_name);
            // cas-7a94: if this worker was cancelled via shutdown while the
            // isolate worktree was still building, drop the result and release
            // any early pre-assign so the task is not stuck on a never-started
            // worker.
            let cancelled = self.dead_workers.contains(&pending_name);
            match handle.await {
                Ok(Ok(_result)) if cancelled => {
                    tracing::info!(
                        worker = %pending_name,
                        "cas-7a94: spawn finished after shutdown cancel — discarding pane, releasing pre-assign"
                    );
                    if let Some(ref task_id) = pending_task_id {
                        crate::ui::factory::app::render_and_ops::epic_workers::release_preassign_if_bound(
                            self.app.cas_dir(),
                            task_id,
                            &pending_name,
                        );
                    }
                    crate::ui::factory::app::render_and_ops::epic_workers::release_worker_task_bindings(
                        self.app.cas_dir(),
                        &pending_name,
                    );
                    // Do not call finish_worker_spawn — the worktree (if any) is
                    // left for the reaper; no PTY pane is registered.
                }
                Ok(Ok(result)) => {
                    // Build per-worker Teams config before finish_worker_spawn adds to worker list
                    let worker_name_for_teams = result.worker_name.clone();
                    let color_idx = self.app.worker_names().len();
                    let teams_config = self.teams.as_ref().map(|t| {
                        use super::teams::TeamsManager;
                        t.spawn_config_for(
                            &worker_name_for_teams,
                            "general-purpose",
                            TeamsManager::color_for_index(color_idx),
                            None,
                        )
                    });
                    // Register TUI color to match the Teams color
                    if let Some(ref tc) = teams_config {
                        crate::ui::theme::register_agent_color(&tc.agent_name, &tc.agent_color);
                    }
                    let task_id_for_finish = pending_task_id.clone();
                    match self.app.finish_worker_spawn(
                        result,
                        teams_config,
                        pending_spec,
                        task_id_for_finish,
                    ) {
                        Ok(name) => {
                            tracing::info!("Spawned worker (async): {}", name);
                            // A worker may reuse a retired name (e.g. a Codex worker
                            // spawned into a Claude worker's old name). Clear it from
                            // the insert-only dead set so its messages aren't dropped
                            // as "from a dead worker" (cas-5a5c).
                            self.dead_workers.remove(&name);
                            // Register new worker with native Agent Teams
                            if let Some(ref teams) = self.teams {
                                let worker_cwd = self
                                    .app
                                    .worktree_manager()
                                    .map(|mgr| mgr.worktree_path_for_worker(&name))
                                    .unwrap_or_else(|| self.app.project_path().to_path_buf());
                                if let Err(e) = teams.add_member(&name, &worker_cwd, color_idx) {
                                    tracing::error!(
                                        "Failed to add worker '{}' to teams: {}",
                                        name,
                                        e
                                    );
                                }
                            }
                            if self.app.record_enabled() {
                                if let Err(e) = self.app.start_recording_for_pane(&name).await {
                                    tracing::error!(
                                        "Failed to start recording for {}: {}",
                                        name,
                                        e
                                    );
                                }
                            }
                            // Notify web viewers of updated pane list
                            if let Some(ref handle) = self.cloud_handle {
                                let mut panes = self.app.worker_names().to_vec();
                                panes.insert(0, self.app.supervisor_name().to_string());
                                handle.send_pane_list(panes);
                            }
                            // Notify GUI and WS clients of new worker pane
                            self.gui_notify_pane_added(&name, cas_mux::PaneKind::Worker);
                        }
                        Err(e) => {
                            crate::telemetry::track(
                                "factory_worker_spawn_result",
                                vec![
                                    ("success", "false"),
                                    ("reason", "finish_worker_spawn_failed"),
                                ],
                            );
                            // cas-7a94: early pre-assign may have bound the task —
                            // release so a failed finish cannot leave a ghost assignee.
                            if let Some(ref task_id) = pending_task_id {
                                crate::ui::factory::app::render_and_ops::epic_workers::release_preassign_if_bound(
                                    self.app.cas_dir(),
                                    task_id,
                                    &pending_name,
                                );
                            }
                            self.app.set_error(format!("Failed to finish spawn: {e}"));
                        }
                    }
                }
                Ok(Err(e)) => {
                    crate::telemetry::track(
                        "factory_worker_spawn_result",
                        vec![("success", "false"), ("reason", "background_spawn_failed")],
                    );
                    // cas-7a94: isolate worktree failed after early pre-assign.
                    if let Some(ref task_id) = pending_task_id {
                        crate::ui::factory::app::render_and_ops::epic_workers::release_preassign_if_bound(
                            self.app.cas_dir(),
                            task_id,
                            &pending_name,
                        );
                    }
                    self.app.set_error(format!("Failed to spawn worker: {e}"));
                }
                Err(e) => {
                    crate::telemetry::track(
                        "factory_worker_spawn_result",
                        vec![("success", "false"), ("reason", "spawn_task_panicked")],
                    );
                    if let Some(ref task_id) = pending_task_id {
                        crate::ui::factory::app::render_and_ops::epic_workers::release_preassign_if_bound(
                            self.app.cas_dir(),
                            task_id,
                            &pending_name,
                        );
                    }
                    self.app.set_error(format!("Spawn task panicked: {e}"));
                }
            }
            self.app.spawning_count = self.app.spawning_count.saturating_sub(1);
            return; // One completion per tick
        }

        // Step 2: Pop FIFO when no spawn is active. While one is active, only
        // a shutdown may pass so it can mark/cancel that in-flight worker.
        let action = match take_next_pending_spawn(
            &mut self.pending_spawns,
            self.spawn_task.is_some(),
        ) {
            Some(a) => a,
            None => return,
        };

        match action {
            PendingSpawn::Anonymous {
                isolate,
                spec,
                task_id,
            } => {
                match self.app.prepare_worker_spawn(None, isolate) {
                    Ok(prep) => {
                        let worker_name = prep.worker_name.clone();
                        // cas-7a94: bind task_id as soon as the worker name is
                        // known — before the isolate worktree finishes — so
                        // codex+isolate async gaps cannot skip pre-assign.
                        // finish_worker_spawn confirms; failure/cancel paths
                        // release via release_preassign_if_bound.
                        if let Some(ref tid) = task_id {
                            let _ = crate::ui::factory::app::render_and_ops::epic_workers::assign_task_to_new_worker(
                                self.app.cas_dir(),
                                tid,
                                &worker_name,
                            );
                        }
                        self.app.add_pending_worker(worker_name.clone(), isolate);
                        self.spawn_task = Some((
                            worker_name,
                            spec,
                            task_id,
                            tokio::task::spawn_blocking(move || prep.run()),
                        ));
                    }
                    Err(e) => {
                        crate::telemetry::track(
                            "factory_worker_spawn_result",
                            vec![
                                ("success", "false"),
                                ("reason", "prepare_worker_spawn_failed"),
                            ],
                        );
                        self.app.set_error(format!("Failed to prepare spawn: {e}"));
                        self.app.spawning_count = self.app.spawning_count.saturating_sub(1);
                    }
                }
            }
            PendingSpawn::Named {
                name,
                isolate,
                spec,
                task_id,
            } => {
                match self.app.prepare_worker_spawn(Some(&name), isolate) {
                    Ok(prep) => {
                        let worker_name = prep.worker_name.clone();
                        // cas-7a94: early pre-assign once name is final (see Anonymous).
                        if let Some(ref tid) = task_id {
                            let _ = crate::ui::factory::app::render_and_ops::epic_workers::assign_task_to_new_worker(
                                self.app.cas_dir(),
                                tid,
                                &worker_name,
                            );
                        }
                        self.app.add_pending_worker(worker_name.clone(), isolate);
                        self.spawn_task = Some((
                            worker_name,
                            spec,
                            task_id,
                            tokio::task::spawn_blocking(move || prep.run()),
                        ));
                    }
                    Err(e) => {
                        crate::telemetry::track(
                            "factory_worker_spawn_result",
                            vec![
                                ("success", "false"),
                                ("reason", "prepare_named_spawn_failed"),
                            ],
                        );
                        self.app
                            .set_error(format!("Failed to prepare spawn '{name}': {e}"));
                        self.app.spawning_count = self.app.spawning_count.saturating_sub(1);
                    }
                }
            }
            PendingSpawn::Shutdown {
                count,
                names,
                force,
            } => {
                // Shutdowns are fast - process synchronously
                // Collect worker names before shutdown for GUI notification
                let workers_to_stop = shutdown_targets(
                    self.app.worker_names(),
                    self.spawn_task.as_ref().map(|(name, _, _, _)| name.as_str()),
                    count,
                    &names,
                );

                // cas-7a94: drop still-queued spawns for these names and release
                // any early pre-assigns so "shutdown before boot finishes" cannot
                // leave Open/InProgress ghosts on never-started workers.
                {
                    let stop_set: std::collections::HashSet<&str> =
                        workers_to_stop.iter().map(String::as_str).collect();
                    let cancel_all = names.is_empty() && count.unwrap_or(0) == 0;
                    let mut kept = std::collections::VecDeque::new();
                    while let Some(pending) = self.pending_spawns.pop_front() {
                        match pending {
                            PendingSpawn::Named {
                                name,
                                isolate,
                                spec,
                                task_id,
                            } if cancel_all || stop_set.contains(name.as_str()) => {
                                if let Some(ref tid) = task_id {
                                    crate::ui::factory::app::render_and_ops::epic_workers::release_preassign_if_bound(
                                        self.app.cas_dir(),
                                        tid,
                                        &name,
                                    );
                                }
                                crate::ui::factory::app::render_and_ops::epic_workers::release_worker_task_bindings(
                                    self.app.cas_dir(),
                                    &name,
                                );
                                self.app.spawning_count = self.app.spawning_count.saturating_sub(1);
                                tracing::info!(
                                    worker = %name,
                                    "cas-7a94: cancelled pending spawn on shutdown — released pre-assign"
                                );
                                let _ = (isolate, spec); // consumed
                            }
                            PendingSpawn::Anonymous {
                                isolate,
                                spec,
                                task_id,
                            } if cancel_all => {
                                // Anonymous names are only known once prepare runs;
                                // if still in the queue, no early assign has fired yet.
                                let _ = (isolate, spec, task_id);
                                self.app.spawning_count = self.app.spawning_count.saturating_sub(1);
                            }
                            other => kept.push_back(other),
                        }
                    }
                    self.pending_spawns = kept;
                }

                if self.app.record_enabled() {
                    for name in &workers_to_stop {
                        let _ = self.app.stop_recording_for_pane(name).await;
                    }
                }
                // Track shut-down workers so their queued messages are dropped
                // and any in-flight isolate spawn is discarded on completion.
                for name in &workers_to_stop {
                    self.dead_workers.insert(name.clone());
                    // Also release bindings for workers that never made it into
                    // worker_names (early pre-assign only) — shutdown_worker
                    // only runs for live workers.
                    crate::ui::factory::app::render_and_ops::epic_workers::release_worker_task_bindings(
                        self.app.cas_dir(),
                        name,
                    );
                }
                if let Err(e) = self.app.shutdown_workers(count, &names, force) {
                    let target = if !names.is_empty() {
                        names.join(", ")
                    } else if let Some(c) = count {
                        if c == 0 {
                            "all workers".to_string()
                        } else {
                            format!("{c} worker(s)")
                        }
                    } else {
                        "all workers".to_string()
                    };
                    self.app
                        .set_error(format!("Failed to shutdown {target}: {e}"));
                    tracing::error!("Failed to shutdown {}: {}", target, e);
                } else {
                    // Remove shut-down workers from native Agent Teams
                    if let Some(ref teams) = self.teams {
                        for name in &workers_to_stop {
                            let _ = teams.remove_member(name);
                        }
                    }
                    // Notify GUI and WS clients that panes were removed
                    for name in &workers_to_stop {
                        self.gui_notify_pane_removed(name);
                    }
                }
            }
            PendingSpawn::Respawn(name) => {
                // Build per-worker Teams config for the respawned worker
                let teams_config = self.teams.as_ref().map(|t| {
                    use super::teams::TeamsManager;
                    let color_idx = self.app.worker_names().len();
                    t.spawn_config_for(
                        &name,
                        "general-purpose",
                        TeamsManager::color_for_index(color_idx),
                        None,
                    )
                });
                // Register TUI color to match the Teams color
                if let Some(ref tc) = teams_config {
                    crate::ui::theme::register_agent_color(&tc.agent_name, &tc.agent_color);
                }
                // Respawn reuses existing worktree - fast enough to run synchronously
                match self.app.respawn_worker(&name, teams_config) {
                    Ok(()) => {
                        // The respawned worker is live again under the same name;
                        // clear it from the insert-only dead set so its messages
                        // are no longer dropped as "from a dead worker" (cas-5a5c).
                        self.dead_workers.remove(&name);
                        if self.app.record_enabled() {
                            if let Err(e) = self.app.start_recording_for_pane(&name).await {
                                tracing::error!(
                                    "Failed to start recording for respawned {}: {}",
                                    name,
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        self.app.set_error(format!("Failed to respawn {name}: {e}"));
                    }
                }
            }
            PendingSpawn::Shell { name, shell } => {
                let cwd = self.app.project_path().to_path_buf();
                match self.app.mux.add_shell(&name, cwd, shell.as_deref()) {
                    Ok(_) => {
                        tracing::info!("Shell pane '{}' spawned", name);
                        self.gui_notify_pane_added(&name, cas_mux::PaneKind::Shell);
                    }
                    Err(e) => {
                        self.app
                            .set_error(format!("Failed to spawn shell '{name}': {e}"));
                        tracing::error!("Failed to spawn shell '{}': {}", name, e);
                    }
                }
            }
            PendingSpawn::KillShell { name } => match self.app.mux.remove_shell(&name) {
                Ok(()) => {
                    tracing::info!("Shell pane '{}' killed", name);
                    self.gui_notify_pane_removed(&name);
                }
                Err(e) => {
                    self.app
                        .set_error(format!("Failed to kill shell '{name}': {e}"));
                    tracing::error!("Failed to kill shell '{}': {}", name, e);
                }
            },
        }
    }

    /// Process pending reminders (time-based and event-based)
    ///
    /// Called during the 2-second refresh cycle with the events detected in this tick.
    /// Time-based reminders fire when trigger_at <= now.
    /// Event-based reminders fire when a matching DirectorEvent is detected.
    /// Delivery uses both the supervisor notification queue (for structured data / web UI)
    /// and the prompt queue (for PTY injection into the supervisor's session).
    pub(super) fn process_reminders(&self, events: &[crate::ui::factory::director::DirectorEvent]) {
        use crate::store::{
            open_prompt_queue_store, open_reminder_store, open_supervisor_queue_store,
        };

        let reminder_store = match open_reminder_store(self.app.cas_dir()) {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!("Failed to open reminder store: {}", e);
                return;
            }
        };

        // Expire stale reminders
        if let Err(e) = reminder_store.expire_stale() {
            tracing::error!("Failed to expire stale reminders: {}", e);
        }

        // Check time-based reminders
        let due_reminders = match reminder_store.get_due_time_reminders() {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to get due reminders: {}", e);
                Vec::new()
            }
        };

        let supervisor_queue = if !due_reminders.is_empty() || !events.is_empty() {
            open_supervisor_queue_store(self.app.cas_dir()).ok()
        } else {
            None
        };

        // Open prompt queue for PTY injection of fired reminders
        let prompt_queue = if !due_reminders.is_empty() || !events.is_empty() {
            open_prompt_queue_store(self.app.cas_dir()).ok()
        } else {
            None
        };

        let agent_id_to_name = &self.app.director_data().agent_id_to_name;

        for reminder in &due_reminders {
            fire_reminder(
                reminder,
                &reminder_store,
                &supervisor_queue,
                &prompt_queue,
                &self.session_name,
                agent_id_to_name,
                None,
            );
        }

        // Check event-based reminders against detected events
        for event in events {
            let event_type = event.event_type();
            let candidates = match reminder_store.get_event_reminders(event_type) {
                Ok(r) => r,
                Err(_) => continue,
            };

            for reminder in &candidates {
                // cas-fcd4: only fire reminds registered in this factory session
                // (shared cas.db otherwise cross-fires concurrent sessions).
                if !reminder_matches_factory_session(
                    reminder.session_id.as_deref(),
                    &self.session_name,
                ) {
                    continue;
                }
                if matches_event_filter(reminder, event) {
                    fire_reminder(
                        reminder,
                        &reminder_store,
                        &supervisor_queue,
                        &prompt_queue,
                        &self.session_name,
                        agent_id_to_name,
                        Some(event),
                    );
                }
            }
        }
    }

    /// Handle epic state change
    ///
    /// Manages git branches when epic state transitions:
    /// - Started: Creates epic branch, workers branch from it
    /// - Completed: Merges worker branches to epic branch
    pub(super) async fn handle_epic_change(
        &mut self,
        change: EpicStateChange,
    ) -> anyhow::Result<()> {
        match change {
            EpicStateChange::Started {
                epic_id,
                epic_title,
                previous_state,
            } => {
                // Update terminal title with the new epic
                set_terminal_title(self.app.project_path(), Some(&epic_title));

                // Create epic branch when transitioning from Idle
                if matches!(previous_state, crate::ui::factory::app::EpicState::Idle) {
                    match self.app.create_epic_branch(&epic_title) {
                        Ok(branch) => {
                            tracing::info!(
                                "EPIC {} started - created branch '{}' for workers",
                                epic_id,
                                branch
                            );
                        }
                        Err(e) => {
                            tracing::error!("Failed to create epic branch for {}: {}", epic_id, e);
                            self.app
                                .set_error(format!("Failed to create epic branch: {e}"));
                        }
                    }
                } else if self.resumed_epic_ids.insert(epic_id.clone()) {
                    tracing::info!(
                        "EPIC {} started (resuming) - using existing branch",
                        epic_id
                    );
                }
            }

            EpicStateChange::Completed {
                epic_id,
                epic_title,
            } => {
                // Update terminal title to show no active epic
                set_terminal_title(self.app.project_path(), None);

                // Merge worker branches to epic branch
                tracing::info!(
                    "EPIC {} ({}) completed - merging worker branches",
                    epic_id,
                    epic_title
                );

                match self.app.merge_workers_to_epic() {
                    Ok(results) => {
                        let success_count = results.iter().filter(|(_, ok, _)| *ok).count();
                        let fail_count = results.len() - success_count;

                        if fail_count > 0 {
                            let failures: Vec<_> = results
                                .iter()
                                .filter(|(_, ok, _)| !ok)
                                .map(|(name, _, msg)| {
                                    format!(
                                        "{}: {}",
                                        name,
                                        msg.as_deref().unwrap_or("unknown error")
                                    )
                                })
                                .collect();
                            tracing::warn!(
                                "EPIC {} merge: {}/{} workers merged. Failures: {:?}",
                                epic_id,
                                success_count,
                                results.len(),
                                failures
                            );
                            self.app.set_error(format!(
                                "Epic merge: {fail_count} worker(s) failed to merge"
                            ));
                        } else {
                            tracing::info!(
                                "EPIC {} merge complete: all {} workers merged",
                                epic_id,
                                success_count
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to merge workers for EPIC {}: {}", epic_id, e);
                        self.app.set_error(format!("Failed to merge workers: {e}"));
                    }
                }

                // Note: Worker branch cleanup is handled via /factory-merge-epic skill
                // to give supervisor control over the cleanup process
            }
        }
        Ok(())
    }
}

/// Fire a reminder by delivering it to both the notification queue
/// (for web UI / structured data) and the prompt queue (for PTY injection).
///
/// `agent_id_to_name` maps agent UUIDs to pane names that the prompt queue
/// can route to. Falls back to `"supervisor"` when the target agent ID is
/// not found in the map.
///
/// `triggering_event` is the DirectorEvent that caused this reminder to fire
/// (only set for event-based reminders). Its context is included in the
/// delivered prompt so the recipient knows what happened.
fn fire_reminder(
    reminder: &cas_store::Reminder,
    reminder_store: &std::sync::Arc<dyn cas_store::ReminderStore>,
    supervisor_queue: &Option<std::sync::Arc<dyn cas_store::SupervisorQueueStore>>,
    prompt_queue: &Option<std::sync::Arc<dyn cas_store::PromptQueueStore>>,
    session_name: &str,
    agent_id_to_name: &std::collections::HashMap<String, String>,
    triggering_event: Option<&crate::ui::factory::director::DirectorEvent>,
) {
    // Build event JSON for persistence
    let event_json = triggering_event.map(|e| {
        serde_json::json!({
            "event_type": e.event_type(),
            "data": e.to_json(),
            "description": e.description(),
        })
    });

    // Mark as fired first to prevent double-fire on next tick
    if let Err(e) = reminder_store.mark_fired(reminder.id, event_json.as_ref()) {
        tracing::error!("Failed to mark reminder {} as fired: {}", reminder.id, e);
        return;
    }

    let mut payload = serde_json::json!({
        "reminder_id": reminder.id,
        "message": reminder.message,
        "target_id": reminder.target_id,
        "trigger_type": reminder.trigger_type.to_string(),
    });
    if let Some(event) = triggering_event {
        payload["event_type"] = serde_json::Value::String(event.event_type().to_string());
        payload["event"] = event.to_json();
    }
    let payload = payload.to_string();

    // Enqueue to notification queue (for web UI / structured data).
    // Notify the owner so they know their reminder fired.
    if let Some(queue) = supervisor_queue {
        if let Err(e) = queue.notify(
            &reminder.owner_id,
            "reminder_fired",
            &payload,
            cas_store::NotificationPriority::Normal,
        ) {
            tracing::error!("Failed to enqueue reminder notification: {}", e);
        }
    }

    // Enqueue to prompt queue for PTY injection into the target agent's session.
    // Resolve the target agent UUID to its pane name. process_prompt_queue also
    // resolves the logical name "supervisor" to the actual pane name, so we use
    // that as fallback when the target ID isn't in the map.
    if let Some(queue) = prompt_queue {
        let target = agent_id_to_name
            .get(&reminder.target_id)
            .map(|s| s.as_str())
            .unwrap_or("supervisor");

        // Include triggering event context for event-based reminders
        let prompt = match triggering_event {
            Some(event) => format!(
                "Reminder #{}: {} (triggered by: {})",
                reminder.id,
                reminder.message,
                event.description()
            ),
            None => format!("Reminder #{}: {}", reminder.id, reminder.message),
        };

        if let Err(e) =
            queue.enqueue_with_session(&reminder.owner_id, target, &prompt, session_name)
        {
            tracing::error!("Failed to enqueue reminder prompt: {}", e);
        } else {
            tracing::info!(
                "Fired reminder #{} → {} ({}): {}",
                reminder.id,
                target,
                reminder.target_id,
                reminder.message
            );
        }
    }
}

/// Whether this factory daemon session may fire the reminder (cas-fcd4).
///
/// Reminders registered with a non-empty `session_id` only fire when the
/// processing daemon's session name matches. Legacy / non-factory reminds
/// (`session_id` unset) still fire in any session so single-session behavior
/// is unchanged.
pub(crate) fn reminder_matches_factory_session(
    reminder_session_id: Option<&str>,
    current_session: &str,
) -> bool {
    match reminder_session_id.map(str::trim).filter(|s| !s.is_empty()) {
        None => true,
        Some(sid) => sid == current_session,
    }
}

/// Check if a reminder's event filter matches a detected DirectorEvent.
///
/// Uses JSON subset matching: every key-value in the filter must appear
/// in the event's JSON representation. An empty or missing filter matches
/// any event of the correct type (after session scoping — see
/// [`reminder_matches_factory_session`]).
fn matches_event_filter(
    reminder: &cas_store::Reminder,
    event: &crate::ui::factory::director::DirectorEvent,
) -> bool {
    let filter = match &reminder.trigger_filter {
        Some(f) => f,
        None => return true, // No filter = match any event of this type
    };

    let event_data = event.to_json();

    match (filter.as_object(), event_data.as_object()) {
        (Some(filter_obj), Some(event_obj)) => {
            for (key, expected) in filter_obj {
                match event_obj.get(key) {
                    Some(actual) if actual == expected => continue,
                    _ => return false,
                }
            }
            true
        }
        _ => false,
    }
}

fn is_exact_agent_name_match(agent: &AgentSummary, worker_name: &str) -> bool {
    agent.name == worker_name
}

#[cfg(test)]
mod tests {
    use super::{
        is_exact_agent_name_match, matches_event_filter, reminder_matches_factory_session,
        shutdown_targets, take_next_pending_spawn,
    };
    use crate::ui::factory::daemon::{FactoryDaemon, PendingSpawn};
    use crate::ui::factory::director::{AgentSummary, DirectorData, DirectorEvent};
    use cas_types::AgentStatus;
    use std::collections::{HashMap, VecDeque};

    // -----------------------------------------------------------------------
    // cas-893c: idle-nudge eligibility (`FactoryDaemon::worker_looks_idle`)
    // -----------------------------------------------------------------------

    fn agent_summary(
        name: &str,
        current_task: Option<&str>,
        last_heartbeat: Option<chrono::DateTime<chrono::Utc>>,
        latest_activity: Option<chrono::DateTime<chrono::Utc>>,
    ) -> AgentSummary {
        AgentSummary {
            id: format!("id-{name}"),
            name: name.to_string(),
            status: AgentStatus::Active,
            registered_at: chrono::Utc::now(),
            current_task: current_task.map(str::to_string),
            latest_activity: latest_activity.map(|ts| ("checkpoint".to_string(), ts)),
            last_heartbeat,
            pending_messages: 0,
            pending_supervisor_messages: 0,
            latest_supervisor_message_at: None,
            active_lease: None,
            effort: None,
        }
    }

    fn director_data_with(agents: Vec<AgentSummary>) -> DirectorData {
        DirectorData {
            ready_tasks: vec![],
            in_progress_tasks: vec![],
            epic_tasks: vec![],
            agents,
            activity: vec![],
            agent_id_to_name: HashMap::new(),
            changes: vec![],
            git_loaded: true,
            reminders: vec![],
            epic_closed_counts: HashMap::new(),
        }
    }

    #[test]
    fn worker_looks_idle_true_for_taskless_worker_with_no_signals() {
        let now = chrono::Utc::now();
        let data = director_data_with(vec![agent_summary("swift-fox", None, None, None)]);
        assert!(
            FactoryDaemon::worker_looks_idle(&data, "swift-fox", now),
            "no current task and no heartbeat/activity data at all must read as idle \
             (an absent signal is inactive, not treated as busy — mirrors the \
             director's WorkerIdle gate)"
        );
    }

    #[test]
    fn worker_looks_idle_false_when_current_task_set() {
        let now = chrono::Utc::now();
        let data = director_data_with(vec![agent_summary(
            "swift-fox",
            Some("cas-1234"),
            None,
            None,
        )]);
        assert!(
            !FactoryDaemon::worker_looks_idle(&data, "swift-fox", now),
            "a worker holding a current task must never be nudged"
        );
    }

    #[test]
    fn worker_looks_idle_false_when_heartbeat_and_activity_both_fresh() {
        let now = chrono::Utc::now();
        let data = director_data_with(vec![agent_summary(
            "swift-fox",
            None,
            Some(now - chrono::Duration::seconds(5)),
            Some(now - chrono::Duration::seconds(5)),
        )]);
        assert!(
            !FactoryDaemon::worker_looks_idle(&data, "swift-fox", now),
            "fresh heartbeat + fresh activity means between-turns, not idle — \
             nudging here would type into a pane that's still mid-work"
        );
    }

    #[test]
    fn worker_looks_idle_true_when_heartbeat_or_activity_is_stale() {
        let now = chrono::Utc::now();
        // Heartbeat long past FRESH_HEARTBEAT_SECS (60s).
        let data = director_data_with(vec![agent_summary(
            "swift-fox",
            None,
            Some(now - chrono::Duration::seconds(300)),
            Some(now - chrono::Duration::seconds(5)),
        )]);
        assert!(
            FactoryDaemon::worker_looks_idle(&data, "swift-fox", now),
            "a stale heartbeat alone is enough to fail the fresh+recent AND gate, \
             so this must read as idle even with recent activity recorded"
        );
    }

    #[test]
    fn worker_looks_idle_false_for_unknown_agent() {
        let now = chrono::Utc::now();
        let data = director_data_with(vec![]);
        assert!(
            !FactoryDaemon::worker_looks_idle(&data, "ghost-worker", now),
            "an agent absent from DirectorData (e.g. mid-spawn) must not be \
             guessed idle — fall back to the plain inbox write"
        );
    }

    #[test]
    fn shutdown_bypasses_an_in_flight_spawn_without_reordering_other_actions() {
        let mut pending = VecDeque::from([
            PendingSpawn::Shell {
                name: "later-shell".into(),
                shell: None,
            },
            PendingSpawn::Shutdown {
                count: None,
                names: vec!["booting-worker".into()],
                force: false,
            },
        ]);

        assert!(matches!(
            take_next_pending_spawn(&mut pending, true),
            Some(PendingSpawn::Shutdown { .. })
        ));
        assert!(matches!(
            pending.front(),
            Some(PendingSpawn::Shell { name, .. }) if name == "later-shell"
        ));
        assert!(take_next_pending_spawn(&mut pending, true).is_none());
        assert!(matches!(
            take_next_pending_spawn(&mut pending, false),
            Some(PendingSpawn::Shell { .. })
        ));

        let live = vec!["live-worker".to_string()];
        assert_eq!(
            shutdown_targets(&live, Some("booting-worker"), Some(0), &[]),
            vec!["live-worker".to_string(), "booting-worker".to_string()],
            "shutdown-all must mark the in-flight worker dead so its late spawn result is discarded"
        );
    }

    // -----------------------------------------------------------------------
    // cas-fcd4: event remind session scoping
    // -----------------------------------------------------------------------

    #[test]
    fn reminder_session_scope_blocks_foreign_factory_session() {
        assert!(
            !reminder_matches_factory_session(Some("session-a"), "session-b"),
            "session A remind must not fire in session B"
        );
        assert!(
            reminder_matches_factory_session(Some("session-a"), "session-a"),
            "same-session must match"
        );
    }

    #[test]
    fn reminder_session_scope_legacy_none_matches_any_session() {
        // Single-session / pre-cas-fcd4 rows keep working.
        assert!(reminder_matches_factory_session(None, "session-a"));
        assert!(reminder_matches_factory_session(None, "session-b"));
        assert!(reminder_matches_factory_session(Some(""), "session-a"));
        assert!(reminder_matches_factory_session(Some("  "), "session-b"));
    }

    #[test]
    fn matches_event_filter_task_id_still_works() {
        let reminder = cas_store::Reminder {
            id: 1,
            owner_id: "owner".into(),
            target_id: "owner".into(),
            message: "review".into(),
            trigger_type: cas_store::ReminderTriggerType::Event,
            trigger_at: None,
            trigger_event: Some("task_completed".into()),
            trigger_filter: Some(serde_json::json!({"task_id": "cas-keep"})),
            status: cas_store::ReminderStatus::Pending,
            ttl_secs: 3600,
            created_at: chrono::Utc::now(),
            fired_at: None,
            cancelled_at: None,
            fired_event: None,
            session_id: Some("session-a".into()),
        };
        let match_event = DirectorEvent::TaskCompleted {
            task_id: "cas-keep".into(),
            task_title: "Keep".into(),
            worker: "worker-1".into(),
        };
        let other_event = DirectorEvent::TaskCompleted {
            task_id: "cas-other".into(),
            task_title: "Other".into(),
            worker: "worker-2".into(),
        };
        assert!(matches_event_filter(&reminder, &match_event));
        assert!(!matches_event_filter(&reminder, &other_event));
        // Session gate is separate: filter alone still matches; session blocks foreign.
        assert!(reminder_matches_factory_session(
            reminder.session_id.as_deref(),
            "session-a"
        ));
        assert!(!reminder_matches_factory_session(
            reminder.session_id.as_deref(),
            "session-b"
        ));
    }

    #[test]
    fn test_agent_match_is_exact_not_substring() {
        let worker_10 = AgentSummary {
            id: "agent-10".to_string(),
            name: "worker-10".to_string(),
            status: AgentStatus::Active,
            registered_at: chrono::Utc::now(),
            current_task: None,
            latest_activity: None,
            last_heartbeat: None,
            pending_messages: 0,
            pending_supervisor_messages: 0,
            latest_supervisor_message_at: None,
            active_lease: None,
            effort: None,
        };

        assert!(
            !is_exact_agent_name_match(&worker_10, "worker-1"),
            "worker-1 must not match worker-10"
        );
        assert!(is_exact_agent_name_match(&worker_10, "worker-10"));
    }

    /// Regression for cas-5a5c: a worker name is reusable. When a Claude worker
    /// shuts down, its name enters the insert-only `dead_workers` set; a Codex
    /// worker later respawned into that same name must still be able to send
    /// messages. Keying the drop on the name alone silently discarded every
    /// message from the live Codex worker (marked processed, never delivered),
    /// which is what made Codex workers appear to "not communicate".
    #[test]
    fn test_source_is_dead_respects_live_name_reuse() {
        use std::collections::HashSet;

        let mut dead: HashSet<String> = HashSet::new();
        dead.insert("backend-admin".to_string());
        dead.insert("frontend-dry".to_string());

        // No live worker owns the retired name → genuinely dead, drop its messages.
        assert!(FactoryDaemon::source_is_dead(&dead, &[], "backend-admin"));

        // A live worker was respawned into the same name → NOT dead, must deliver.
        let live = vec!["backend-admin".to_string(), "frontend-dry".to_string()];
        assert!(!FactoryDaemon::source_is_dead(
            &dead,
            &live,
            "backend-admin"
        ));
        assert!(!FactoryDaemon::source_is_dead(&dead, &live, "frontend-dry"));

        // A source never in the dead set (external sender / fresh worker) passes.
        assert!(!FactoryDaemon::source_is_dead(&dead, &live, "openclaw"));
        assert!(!FactoryDaemon::source_is_dead(&dead, &[], "supervisor"));
    }

    #[test]
    fn test_is_idle_message_matches_stock_heartbeats() {
        // Bare stock phrases.
        assert!(FactoryDaemon::is_idle_message("Standing by."));
        assert!(FactoryDaemon::is_idle_message("Ready for task."));
        assert!(FactoryDaemon::is_idle_message("Ready for tasks."));
        assert!(FactoryDaemon::is_idle_message("Awaiting instructions."));
        assert!(FactoryDaemon::is_idle_message("Awaiting task."));
        assert!(FactoryDaemon::is_idle_message("Waiting for work."));
        assert!(FactoryDaemon::is_idle_message("No task assigned."));
        // Case-insensitive and leading whitespace tolerant.
        assert!(FactoryDaemon::is_idle_message("  STANDING BY."));
        assert!(FactoryDaemon::is_idle_message(
            "standing by for further direction"
        ));
    }

    /// Regression for cas-f9e8: the old unanchored substring filter silently
    /// dropped any message containing the literal word "idle" or an idle
    /// phrase buried mid-message. These are real status/debug messages that
    /// must flow through to the supervisor.
    #[test]
    fn test_is_idle_message_does_not_match_status_reports_containing_idle_words() {
        // "idle" as a bare substring — the old filter would have dropped this.
        assert!(!FactoryDaemon::is_idle_message(
            "Fix 1 for the WorkerIdle debounce race is in HEAD."
        ));
        assert!(!FactoryDaemon::is_idle_message(
            "the idle detector now requires two consecutive ticks"
        ));
        assert!(!FactoryDaemon::is_idle_message(
            "I am idle, waiting for work." // starts with "I am", not a stock phrase
        ));
        // Idle phrase buried mid-message, not at the start.
        assert!(!FactoryDaemon::is_idle_message(
            "Task cas-1234 closed. Standing by for the next assignment now."
        ));
        // Diagnostic message that previously matched "mcp tools unavailable"
        // as a substring — that phrase has been removed from the filter.
        assert!(!FactoryDaemon::is_idle_message(
            "MCP tools unavailable — falling back to direct sqlite; see bugfix memory."
        ));
        // Real work reports.
        assert!(!FactoryDaemon::is_idle_message(
            "COMPLETED task cas-1234. Commit: abc123."
        ));
        assert!(!FactoryDaemon::is_idle_message(
            "Blocked: cannot compile due to missing dep."
        ));
        assert!(!FactoryDaemon::is_idle_message(
            "Fixed the bug in parser.rs, tests pass."
        ));
    }

    /// Regression for cas-f9e8: very long messages that happen to mention an
    /// idle phrase must never be classified as idle heartbeats, because the
    /// daemon silently drops rate-limited matches without delivering them.
    #[test]
    fn test_is_idle_message_rejects_long_messages_even_when_starting_with_idle_phrase() {
        let long_report = format!(
            "Standing by. {}",
            "x".repeat(320) // pushes total length past MAX_IDLE_LEN
        );
        assert!(
            !FactoryDaemon::is_idle_message(&long_report),
            "long messages must never be treated as idle heartbeats even when they \
             start with a stock phrase — idle filter silently drops matches, so a \
             false positive here would lose the entire report"
        );
    }
}
