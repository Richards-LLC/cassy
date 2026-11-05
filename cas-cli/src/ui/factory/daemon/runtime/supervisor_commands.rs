//! Bounded, caller-owned supervisor controls. PTY submission never means execution.
use std::collections::HashMap;
use std::time::{Duration, Instant};
use chrono::{DateTime, Utc};
use crate::ui::factory::{MessageAttribution, DaemonMessage};
use crate::ui::factory::supervisor_commands::SupervisorCommandRequest as Request;
use crate::ui::factory::daemon::FactoryDaemon;
use crate::ui::factory::supervisor_commands::{SupervisorCommand as Command, SupervisorCommandKey as Key, SupervisorCommandPhase as Phase, SupervisorCommandReceipt as Receipt};

type Owner = (String, String);
/// Receipts held at once. Only a burst inside the replay window can reach it.
const RECEIPT_CAP: usize = 256;
/// A finished receipt answers replays of its reference for this long after its
/// last activity. Commander polls one reference for at most 95 seconds.
const REPLAY_WINDOW: Duration = Duration::from_secs(10 * 60);
#[derive(Default)]
pub(crate) struct CommandState {
    records: HashMap<Owner, Record>,
    /// Native turn evidence is read off-loop; footer repaint is not a turn.
    readiness: Option<tokio::task::JoinHandle<Option<bool>>>,
    readiness_polled: Option<Instant>,
    /// A deliberate restart between the kill and the replacement pane.
    restart: Option<PendingRestart>,
}
struct PendingRestart { killed_at: Instant, exited: bool }
/// Replace the pane this long after the kill even if no exit was reported: a
/// descendant holding the PTY open must not leave the factory without a supervisor.
const RESTART_EXIT_DEADLINE: Duration = Duration::from_secs(10);
/// PTY silence required before a command is typed. Codex hides its busy text
/// while it streams a reply, so an idle-looking screen alone is insufficient;
/// Codex's own turn markers are the busy signal, this is a second guard.
const COMMAND_SETTLE: Duration = Duration::from_secs(2);
/// A completed reply may still be inside the silence window when its operator
/// sends a command. Give it time to settle, without waiting on a running turn.
const COMMAND_SETTLE_WAIT: Duration = Duration::from_secs(3);
const COMMAND_READINESS_POLL: Duration = Duration::from_millis(250);
impl CommandState {
    /// Drop finished receipts past the replay window. An in-flight command or
    /// open chooser is never evicted, and nothing inside the window is, so a
    /// replayed reference still returns its stored receipt instead of re-running.
    fn evict_expired(&mut self) {
        self.records.retain(|_, record| {
            matches!(record.receipt.phase, Phase::Queued | Phase::Submitted) || record.receipt.interactive || record.started.elapsed() <= REPLAY_WINDOW
        });
    }
}
struct Record {
    receipt: Receipt,
    started: Instant,
    at: DateTime<Utc>,
    before: String,
    // Keep the pre-command history when chooser navigation replaces `before`.
    before_command: String,
    write: Box<dyn Fn() -> Option<Result<(), String>> + Send>,
    hook: Option<tokio::task::JoinHandle<Option<String>>>,
    last_key: Option<Key>,
    /// Pane output stream position when the command was typed.
    output_from: u64,
    /// Only a different live registry PID with a recorded version proves reload.
    reload_before: Option<u32>,
}

fn registered_cassy_server(root: &std::path::Path, session: &str, supervisor: &str) -> Option<crate::factory_binary_skew::ServerVersion> {
    // No PATH/version probe: the observation is the live process's binary epoch.
    let snapshot = crate::factory_binary_skew::snapshot_with_installed(root, "", Utc::now()).ok()?;
    let mut matching = snapshot.servers.into_iter().filter(|server| {
        server.role == cas_types::AgentRole::Supervisor && server.name == supervisor
            && server.session.as_deref() == Some(session)
            && server.version.as_deref().is_some_and(|version| !version.trim().is_empty() && version != "unknown")
    });
    let server = matching.next()?;
    matching.next().is_none().then_some(server)
}

fn reload_observation(root: &std::path::Path, session: &str, supervisor: &str, before: u32) -> Option<String> {
    let server = registered_cassy_server(root, session, supervisor)?;
    if server.pid == before { return None; }
    Some(format!("Cassy reloaded: PID {}, version {} (previous PID {before}).", server.pid, server.version?))
}

impl FactoryDaemon {
    /// Apply `factory action=restart_supervisor` (cas-b492): the daemon owns the
    /// supervisor's PTY, so it ends that pane and starts another while the loop
    /// and worker panes keep running. SIGKILL is deliberate. A SessionEnd hook
    /// would announce the supervisor's epics as orphaned and race its successor,
    /// whose registration already records this session's end and retires its row.
    pub(super) fn take_supervisor_restart_request(&mut self) {
        let Some(request) = crate::factory_daemon_health::take_supervisor_restart(self.app.cas_dir(), &self.session_name, Utc::now()) else { return };
        if self.supervisor_commands.restart.is_some() { return; }
        let name = self.app.supervisor_name().to_string();
        if let Err(error) = self.app.mux.preflight_supervisor_restart(self.app.worker_names()) {
            let detail = format!("Supervisor restart refused; this session keeps running: {error}");
            tracing::warn!(requester = %request.requester, "{detail}");
            let (root, session) = (self.app.cas_dir().to_path_buf(), self.session_name.clone());
            super::store_worker::submit_write("supervisor restart refusal", move || {
                let queue = crate::store::open_prompt_queue_store(&root).map_err(|error| error.to_string())?;
                queue.enqueue_with_session("daemon", &name, &detail, &session).map(|_| ()).map_err(|error| error.to_string())
            });
            return;
        }
        let Some(pane) = self.app.mux.get_mut(&name).filter(|pane| !pane.has_exited()) else { return };
        tracing::info!(requester = %request.requester, "Supervisor restart requested; replacing its pane");
        pane.kill_tree_force();
        self.supervisor_commands.restart = Some(PendingRestart { killed_at: Instant::now(), exited: false });
    }

    /// Whether this supervisor exit belongs to a requested restart. An exit that
    /// does not is the factory's shutdown signal.
    pub(super) fn absorb_supervisor_restart_exit(&mut self) -> bool {
        let Some(pending) = self.supervisor_commands.restart.as_mut() else { return false };
        pending.exited = true;
        true
    }

    /// Start the replacement once the old pane has gone. The armed state clears
    /// first, so any later exit of the new pane is ordinary. If it cannot start,
    /// the factory stops as for any supervisor death: nothing would drive workers.
    pub(super) async fn advance_supervisor_restart(&mut self) {
        let Some(pending) = &self.supervisor_commands.restart else { return };
        if !pending.exited && pending.killed_at.elapsed() < RESTART_EXIT_DEADLINE { return; }
        self.supervisor_commands.restart = None;
        if self.shutdown.load(std::sync::atomic::Ordering::Relaxed) { return; }
        let name = self.app.supervisor_name().to_string();
        let lead = uuid::Uuid::new_v4().to_string();
        if let Some(teams) = &self.teams && let Err(error) = teams.set_lead_session(&lead) {
            tracing::warn!(%error, "could not record the restarted supervisor as team lead");
        }
        match self.app.restart_supervisor_pane(&lead) {
            Ok(previous) => {
                // Dropping a PTY can stall on its terminal (GH #1363).
                if let Some(previous) = previous { tokio::task::spawn_blocking(move || drop(previous)); }
                self.pane_buffers.remove(&name);
                self.forget_probe_pane(&name);
                for record in self.supervisor_commands.records.values_mut() {
                    if matches!(record.receipt.phase, Phase::Queued | Phase::Submitted) || record.receipt.interactive {
                        record.receipt.phase = Phase::Unconfirmed;
                        record.receipt.interactive = false;
                        record.receipt.detail = "The supervisor restarted before this command was confirmed. Do not resend automatically.".into();
                    }
                }
                if let Some(job) = self.supervisor_commands.readiness.take() { job.abort(); }
                self.gui_notify_pane_added(&name, cas_mux::PaneKind::Supervisor);
                self.send_state_update();
                if self.app.record_enabled() && let Err(error) = self.app.start_recording_for_pane(&name).await {
                    tracing::error!(%error, "could not start recording the restarted supervisor");
                }
                // Building the intro reads the context bundle; keep it off the loop.
                let (root, cli, workers, session) = (self.app.cas_dir().to_path_buf(), self.app.harness_for(&name), self.app.worker_names().to_vec(), self.session_name.clone());
                tokio::task::spawn_blocking(move || crate::ui::factory::app::queue_supervisor_restart_prompt(&root, &name, cli, &workers, &lead, &session));
                tracing::info!(workers = self.app.worker_names().len(), "Supervisor restarted; workers kept running");
            }
            Err(error) => {
                let detail = format!("Supervisor restart could not start its replacement; shutting the factory down: {error}");
                tracing::error!("{detail}");
                self.app.set_error(detail);
                self.shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

fn screen(pane: &cas_mux::Pane) -> String {
    (0..pane.rows()).filter_map(|row| pane.dump_row(row).ok()).collect::<Vec<_>>().join("\n")
}

/// A startup grace or a quiet PTY is insufficient: require a known empty native
/// prompt and refuse the harness's busy indicator or an existing modal.
fn idle_prompt(text: &str) -> bool {
    let lower = text.to_lowercase();
    !lower.contains("esc to interrupt") && !lower.contains("select model")
        && !lower.contains("select a model") && !lower.contains("select effort")
        && !lower.contains("compacting conversation") && !lower.contains("esc to cancel")
        && !text.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with(['✢', '✻', '✽', '✶', '✳', '✣', '·', '*'])
                && (line.contains('…') || line.contains("..."))
        })
        && text.lines().rev().take(12).any(|line| {
            let line = line.trim();
            matches!(line, "❯" | "›") || line.starts_with("❯ Try \"")
                || line == "› Ask Codex to do anything"
        })
}

/// Claude's Usage tab varies with account/quota state. Recognize either the
/// quota sections or the session-cost table, using several native fields and
/// whole heading lines rather than requiring one account-dependent label.
fn claude_usage_view(text: &str) -> bool {
    let lower = text.to_lowercase();
    let heading = |label: &str| lower.lines().any(|line| line.trim() == label);
    let field = |label: &str| lower.lines().any(|line| line.trim_start().starts_with(label));
    let quotas = heading("current session") && (field("current week") || field("resets "));
    let costs = (heading("session") || heading("current session"))
        && field("total cost:") && field("total duration (") && field("usage by model:");
    !idle_prompt(text) && (quotas || costs)
}

/// `output` is what the pane emitted since the command was typed. A repeat
/// Codex /status can leave the screen unchanged while its old copy scrolls
/// away, so a marker it printed again is fresh even when the screen had it.
fn native_observation(command: Command, before: &str, current: &str, output: &str) -> Option<(String, bool)> {
    if before == current && output.is_empty() { return None; }
    let lower = current.to_lowercase();
    let output = output.to_lowercase();
    let fresh = |marker: &str| lower.contains(marker) && (!before.to_lowercase().contains(marker) || output.contains(marker));
    match command {
        Command::Compact if fresh("compacted (ctrl+o") || fresh("context compacted ·") =>
            Some(("Context compaction confirmed by the supervisor's harness.".into(), false)),
        Command::Clear if lower.contains("openai codex") && idle_prompt(current)
            && (fresh("tip: use the desktop app") || fresh("to get started, describe a task")) =>
                Some(("Codex returned to its fresh native welcome prompt after Clear.".into(), false)),
        Command::Model if fresh("select model") || fresh("select a model")
            || fresh("select reasoning effort") || fresh("select reasoning level") => Some(("Model chooser opened in the supervisor's harness.".into(), true)),
        Command::Model if codex_model_selection(before, current).is_some() || fresh("set model to") || fresh("model changed to") => Some(("The harness confirmed a model selection.".into(), false)),
        Command::Usage if claude_usage_view(current) && !claude_usage_view(before)
            || command == Command::Usage && fresh("context window") && lower.contains("model:") =>
                Some(("Usage displayed by the supervisor's harness.".into(), !idle_prompt(current))),
        _ => None,
    }
}

/// Codex's native info cell, emitted after the model selection succeeds.
/// Source: openai/codex codex-rs/tui/src/app/event_dispatch.rs.
fn codex_model_selection(before: &str, current: &str) -> Option<(String, Option<String>)> {
    current.lines().filter(|line| !before.lines().any(|old| old == *line)).find_map(|line| {
        let line = line.trim().strip_prefix("• ")?;
        let selected = line.strip_prefix("Model changed to ")?;
        let selected = selected.strip_suffix(" for this conversation").unwrap_or(selected);
        let mut words = selected.split_whitespace();
        let model = words.next()?;
        if !model.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ':')) { return None; }
        let effort = words.next().filter(|level| matches!(*level, "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"));
        if words.next().is_some() { return None; }
        Some((model.to_owned(), effort.map(str::to_owned)))
    })
}

fn record_supervisor_model(root: &std::path::Path, session: &str, supervisor: &str, model: &str, effort: Option<&str>) {
    let Some(agent) = crate::store::open_agent_store(root).ok().and_then(|store| store.list(None).ok())
        .and_then(|agents| agents.into_iter().filter(|agent| agent.role == cas_types::AgentRole::Supervisor
            && agent.name == supervisor && agent.factory_session.as_deref() == Some(session)).max_by_key(|agent| agent.registered_at)) else { return };
    crate::agent_model::record(root, &agent.id, Some(model), effort);
}

/// A recorded request alone is still only submission. Observe its own native
/// turn end; never interpret the model's prose as proof of a successful wrap-up.
fn finish_turn_handled(rows: &[serde_json::Value], cli: cas_mux::SupervisorCli, after: DateTime<Utc>) -> bool {
    let mut requested = false;
    for row in rows {
        if row["isSidechain"] == true || !row["timestamp"].as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok()).is_some_and(|at| at >= after) { continue; }
        match cli {
            cas_mux::SupervisorCli::Claude => {
                if row["type"] == "user" && row["isMeta"] != true {
                    if let Some(text) = row.pointer("/message/content").and_then(|v| v.as_str()) {
                        requested = text.contains("<command-name>/finish</command-name>");
                    }
                }
                if requested && row["type"] == "assistant" && row.pointer("/message/stop_reason").is_some_and(|v| v == "end_turn") { return true; }
            }
            cas_mux::SupervisorCli::Codex => {
                if row["type"] == "response_item" && row["payload"]["type"] == "message" && row["payload"]["role"] == "user" {
                    let text = row["payload"]["content"].as_array().map(|blocks| blocks.iter()
                        .filter_map(|block| block["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
                    requested = matches!(text.trim(), "finish" | "$finish");
                }
                if requested && row["type"] == "event_msg" && row["payload"]["type"] == "task_complete" { return true; }
            }
            _ => return false,
        }
    }
    false
}

fn supervisor_transcript(cas_dir: &std::path::Path, session: &str, supervisor: &str, cli: cas_mux::SupervisorCli) -> Option<std::path::PathBuf> {
    let store = crate::store::open_agent_store(cas_dir).ok()?;
    let agent = store.list(None).ok()?.into_iter().find(|agent|
        (agent.name == supervisor || agent.id == supervisor)
            && agent.role == cas_types::AgentRole::Supervisor
            && agent.factory_session.as_deref() == Some(session))?;
    crate::mcp::tools::service::factory_ops::resolve_worker_transcript_path_for_account(
        cas_dir.parent()?.to_str(), agent.cc_session_id.as_deref()?, cli,
        agent.metadata.get("supervisor_account_dir").map(String::as_str))
}

/// Read the running session, not `claude --version`: an install can replace
/// that executable while an older supervisor is still alive.
fn verify_claude_reload_version(root: &std::path::Path, session: &str, supervisor: &str) -> anyhow::Result<()> {
    const MINIMUM: [u64; 3] = [2, 1, 311];
    let observed = supervisor_transcript(root, session, supervisor, cas_mux::SupervisorCli::Claude)
        .and_then(|path| crate::mcp::tools::service::worker_liveness::read_jsonl_tail(&path, 256 * 1024))
        .and_then(|(_, tail)| tail.split_inclusive('\n').filter(|line| line.ends_with('\n')).rev()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find_map(|row| row.get("version").and_then(serde_json::Value::as_str).map(str::to_owned)));
    let parsed = observed.as_deref().and_then(|version| {
        let mut parts = version.split('.');
        let triple = [parts.next()?.parse::<u64>().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?];
        parts.next().is_none().then_some(triple)
    });
    anyhow::ensure!(parsed.is_some_and(|version| version >= MINIMUM),
        "Reload Cassy was not sent: Claude Code has an unverified version ({}). This action requires observed Claude Code 2.1.311 or newer.", observed.as_deref().unwrap_or("unknown"));
    Ok(())
}

/// Codex streams silently for seconds under an idle-looking prompt (cas-3980),
/// so its rollout decides: busy while the last turn marker is a start. Only the
/// last marker counts; a killed or forked session leaves earlier starts unclosed.
/// The window grows until it holds a marker, as one turn can outgrow any tail.
/// `None` means the rollout could not be read.
fn codex_turn_open(path: &std::path::Path) -> Option<bool> {
    const MAX_WINDOW: u64 = 64 * 1024 * 1024;
    let mut window = 256 * 1024;
    loop {
        let (start, tail) = crate::mcp::tools::service::worker_liveness::read_jsonl_tail(path, window)?;
        // A last line without its newline is still being written.
        let marker = tail.split_inclusive('\n').filter(|line| line.ends_with('\n')).rev()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find_map(|row| {
                if crate::mcp::tools::service::harness_observation::codex_turn_end_kind(&row).is_some() { return Some(false) }
                (row["type"] == "event_msg" && matches!(row["payload"]["type"].as_str(), Some("task_started" | "turn_started"))).then_some(true)
            });
        if marker.is_some() { return marker; }
        if start == 0 { return Some(false); }
        if window >= MAX_WINDOW { return None; }
        window *= 2;
    }
}

/// Positive native completion may release an idle prompt even while a shell
/// repaints the footer. A newer user turn keeps Finish queued (cas-88ca).
fn command_turn_idle(root: &std::path::Path, session: &str, supervisor: &str, cli: cas_mux::SupervisorCli) -> Option<bool> {
    let path = supervisor_transcript(root, session, supervisor, cli)?;
    if cli == cas_mux::SupervisorCli::Codex { return codex_turn_open(&path).map(|open| !open); }
    let turns = crate::mcp::tools::service::harness_observation::latest_turn_observations(&path, cli);
    if turns.completion.is_some() { Some(true) }
    else if turns.wake.is_some() { Some(false) }
    else { None }
}

fn finish_observation(cas_dir: &std::path::Path, session: &str, supervisor: &str, cli: cas_mux::SupervisorCli, after: DateTime<Utc>) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let path = supervisor_transcript(cas_dir, session, supervisor, cli)?;
    let mut file = std::fs::File::open(path).ok()?;
    let start = file.metadata().ok()?.len().saturating_sub(512 * 1024);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new(); file.take(512 * 1024).read_to_end(&mut bytes).ok()?;
    let rows = String::from_utf8_lossy(&bytes).lines().skip(usize::from(start > 0))
        .filter_map(|line| serde_json::from_str(line).ok()).collect::<Vec<_>>();
    finish_turn_handled(&rows, cli, after).then(|| "The harness completed the Finish request. Review its reply for the wrap-up result; this does not imply the session exited.".into())
}

/// The hook is an independent source: SessionStart(source=compact|clear) says
/// the harness has created that context. Never infer it from command echo.
fn hook_observation(cas_dir: &std::path::Path, session: &str, supervisor: &str, command: Command, after: DateTime<Utc>) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let source = match command { Command::Compact => "compact", Command::Clear => "clear", _ => return None };
    for date in [after.date_naive(), Utc::now().date_naive()] {
        let path = cas_dir.join("logs").join(format!("factory-session-{date}.log"));
        let Ok(mut file) = std::fs::File::open(path) else { continue };
        let len = file.metadata().ok()?.len();
        file.seek(SeekFrom::Start(len.saturating_sub(64 * 1024))).ok()?;
        let mut tail = String::new(); file.take(64 * 1024).read_to_string(&mut tail).ok()?;
        for line in tail.lines() {
            let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            if row["event"] == "session_start_fired" && row["factory_session"] == session
                && row["agent"] == supervisor && row["source"] == source
                && row["timestamp"].as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).is_some_and(|at| at >= after) {
                return Some(format!("The harness started its {source} context (SessionStart hook)."));
            }
        }
    }
    None
}

impl FactoryDaemon {
    /// Keep an authenticated Finish until the current native turn ends. This
    /// runs independently of browser polling and never waits on the daemon loop.
    pub(super) fn advance_queued_finish(&mut self) {
        let Some(owner) = self.supervisor_commands.records.iter()
            .find(|(_, record)| record.receipt.phase == Phase::Queued).map(|(owner, _)| owner.clone()) else { return };
        if self.supervisor_commands.readiness.as_ref().is_none_or(|job| !job.is_finished()) {
            if self.supervisor_commands.readiness.is_none() {
                if self.supervisor_commands.readiness_polled.is_some_and(|at| at.elapsed() < COMMAND_READINESS_POLL) { return; }
                let root = self.app.cas_dir().to_path_buf();
                let session = self.session_name.clone();
                let supervisor = self.app.supervisor_name().to_owned();
                let cli = self.app.harness_for(&supervisor);
                self.supervisor_commands.readiness_polled = Some(Instant::now());
                self.supervisor_commands.readiness = Some(tokio::task::spawn_blocking(move || command_turn_idle(&root, &session, &supervisor, cli)));
            }
            return;
        }
        let idle = self.supervisor_commands.readiness.take()
            .and_then(|job| futures_util::FutureExt::now_or_never(job)).and_then(Result::ok).flatten();
        let Some(pane) = self.app.mux.supervisor() else { return };
        if idle != Some(true) || pane.has_exited() || !pane.ready_for_injection()
            || pane.is_composer_dirty() || self.app.mux.pane_inject_busy(pane.id()) || !idle_prompt(&screen(pane)) { return; }
        let before = screen(pane);
        let output_from = self.pane_buffers.get(pane.id()).map_or(0, |buffer| buffer.position());
        let write = pane.inject_prompt_tracked(Command::Finish.mapping(pane.harness()).expect("queued supported Finish"));
        let record = self.supervisor_commands.records.get_mut(&owner).expect("queued command remains owned");
        match write {
            Ok(write) => {
                record.before_command = before.clone(); record.before = before;
                record.output_from = output_from;
                record.at = Utc::now(); record.started = Instant::now();
                record.write = Box::new(move || write.result());
                record.receipt.phase = Phase::Submitted;
                record.receipt.detail = "Command submitted; harness execution is not confirmed yet.".into();
            }
            Err(error) => {
                record.receipt.phase = Phase::Unconfirmed;
                record.receipt.detail = format!("Command write failed; execution is uncertain: {error}. Do not resend automatically.");
            }
        }
    }
    /// The poll loop's silence clock, plus no bytes it has not yet sampled.
    fn output_settled(&self, pane: &cas_mux::Pane) -> bool {
        self.last_pane_output_bytes.get(pane.id()) == Some(&pane.bytes_received())
            && self.pane_silent_since.get(pane.id()).is_some_and(|since| since.elapsed() >= COMMAND_SETTLE)
    }
    async fn wait_for_command_prompt(&mut self) -> anyhow::Result<()> {
        let rollout = if self.app.mux.supervisor().is_some_and(|pane| pane.harness() == cas_mux::SupervisorCli::Codex) {
            let (root, session, supervisor) = (self.app.cas_dir().to_path_buf(), self.session_name.clone(), self.app.supervisor_name().to_owned());
            let path = tokio::task::spawn_blocking(move || supervisor_transcript(&root, &session, &supervisor, cas_mux::SupervisorCli::Codex)).await.ok().flatten();
            if path.is_none() { tracing::warn!("Codex supervisor rollout unavailable; command readiness falls back to output settle"); }
            path
        } else { None };
        let progress = super::loop_watchdog::LoopProgress::new();
        loop {
            // This handler owns the daemon loop while it waits. Drain through
            // the normal exchange so fresh output resets silence, and retain
            // its buffering, client delivery and process-exit observations.
            self.exchange_terminal(&progress).await;
            if let Some(path) = rollout.clone() {
                let open = tokio::task::spawn_blocking(move || codex_turn_open(&path)).await.ok().flatten();
                anyhow::ensure!(open != Some(true), "The supervisor is busy: its Codex turn has not ended. Wait until its turn ends.");
            }
            let pane = self.app.mux.supervisor().ok_or_else(|| anyhow::anyhow!("The supervisor has not started."))?;
            anyhow::ensure!(!pane.has_exited() && pane.ready_for_injection() && !pane.is_composer_dirty()
                && !self.app.mux.pane_inject_busy(pane.id()) && idle_prompt(&screen(pane)),
                "The supervisor is busy or its empty prompt is not confirmed. Wait until its turn ends.");
            let bytes = pane.bytes_received();
            if self.last_pane_output_bytes.insert(pane.id().to_owned(), bytes) != Some(bytes) || !self.pane_silent_since.contains_key(pane.id()) {
                self.pane_silent_since.insert(pane.id().to_owned(), Instant::now());
            }
            if self.output_settled(pane) { return Ok(()); }
            let remaining = COMMAND_SETTLE.saturating_sub(self.pane_silent_since.get(pane.id()).map_or(Duration::ZERO, Instant::elapsed));
            tokio::time::sleep(remaining.min(Duration::from_millis(50))).await;
        }
    }
    pub(super) async fn dispatch_supervisor_command(&mut self, request: Request, client_ref: String, attribution: MessageAttribution) -> anyhow::Result<DaemonMessage> {
        anyhow::ensure!(attribution.operator_verified, "Commands require an authenticated hub relay.");
        let device = attribution.device_id.as_deref().filter(|s| !s.is_empty()).ok_or_else(|| anyhow::anyhow!("missing command owner"))?;
        anyhow::ensure!(!client_ref.is_empty() && client_ref.len() <= 128 && client_ref.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'), "Invalid command reference.");
        let owner = (device.to_owned(), client_ref.clone());
        if let Request::Run { command, confirmed } = &request {
            for record in self.supervisor_commands.records.values_mut() {
                if record.receipt.phase == Phase::Submitted && record.started.elapsed() > Duration::from_secs(90) {
                    record.receipt.phase = Phase::Unconfirmed;
                    record.receipt.detail = if record.receipt.command == Command::ReloadCassy {
                        "Cassy reload is unconfirmed: no replacement live MCP PID and version were recorded. Check Claude's /mcp panel before retrying."
                    } else { "The harness outcome was not observed. Check Raw output before sending another command." }.into();
                }
            }
            if let Some(record) = self.supervisor_commands.records.get(&owner) {
                anyhow::ensure!(record.receipt.command == *command, "Command reference already belongs to another command.");
                return Ok(DaemonMessage::SupervisorCommandReceipt { receipt: record.receipt.clone() });
            }
            self.supervisor_commands.evict_expired();
            anyhow::ensure!(self.supervisor_commands.records.len() < RECEIPT_CAP, "Too many supervisor commands in the last 10 minutes. Try again shortly.");
            anyhow::ensure!(!command.needs_confirmation() || *confirmed, "This command needs confirmation.");
            anyhow::ensure!(!self.supervisor_commands.records.values().any(|r| matches!(r.receipt.phase, Phase::Queued | Phase::Submitted) || r.receipt.interactive), "Another supervisor command is awaiting an outcome.");
            if *command == Command::Finish {
                let pane = self.app.mux.supervisor().ok_or_else(|| anyhow::anyhow!("The supervisor has not started."))?;
                anyhow::ensure!(!pane.has_exited(), "The supervisor has ended.");
                anyhow::ensure!(command.mapping(pane.harness()).is_some(), "This harness does not support that command.");
                let receipt = Receipt { device_id: device.to_owned(), client_ref: client_ref.clone(), command: *command, phase: Phase::Queued,
                    detail: "Finish is queued until the supervisor's current turn ends and its prompt is empty. Nothing has been typed yet.".into(), output: String::new(), interactive: false };
                self.supervisor_commands.records.insert(owner.clone(), Record { receipt, started: Instant::now(), at: Utc::now(), before: String::new(), before_command: String::new(), write: Box::new(|| None), hook: None, last_key: None, output_from: 0, reload_before: None });
                self.supervisor_commands.readiness_polled = None;
                self.advance_queued_finish();
                return Ok(DaemonMessage::SupervisorCommandReceipt { receipt: self.supervisor_commands.records[&owner].receipt.clone() });
            }
            let reload_before = if *command == Command::ReloadCassy {
                let pane = self.app.mux.supervisor().ok_or_else(|| anyhow::anyhow!("The supervisor has not started."))?;
                anyhow::ensure!(command.mapping(pane.harness()).is_some(),
                    "Reload Cassy is unavailable for {}. It requires Claude Code.", format!("{:?}", pane.harness()).to_lowercase());
                let (root, session, supervisor) = (self.app.cas_dir().to_path_buf(), self.session_name.clone(), self.app.supervisor_name().to_owned());
                Some(tokio::task::spawn_blocking(move || {
                    verify_claude_reload_version(&root, &session, &supervisor)?;
                    Ok::<_, anyhow::Error>(registered_cassy_server(&root, &session, &supervisor))
                }).await??
                    .ok_or_else(|| anyhow::anyhow!("Reload Cassy was not sent: the supervisor's live Cassy MCP PID and version are unavailable. Check Claude's /mcp panel."))?.pid)
            } else { None };
            tokio::time::timeout(COMMAND_SETTLE_WAIT, self.wait_for_command_prompt()).await
                .map_err(|_| anyhow::anyhow!("The supervisor is busy: its output did not settle within 3 seconds. Wait until its turn ends."))??;
            let pane = self.app.mux.supervisor().ok_or_else(|| anyhow::anyhow!("The supervisor has not started."))?;
            let text = command.mapping(pane.harness()).ok_or_else(|| anyhow::anyhow!("This harness does not support that command."))?;
            let before = screen(pane);
            let output_from = self.pane_buffers.get(pane.id()).map_or(0, |buffer| buffer.position());
            let write = pane.inject_prompt_tracked(text)?;
            let receipt = Receipt { device_id: device.to_owned(), client_ref: client_ref.clone(), command: *command, phase: Phase::Submitted,
                detail: "Command submitted; harness execution is not confirmed yet.".into(), output: String::new(), interactive: false };
            self.supervisor_commands.records.insert(owner, Record { receipt: receipt.clone(), started: Instant::now(), at: Utc::now(), before_command: before.clone(), before, write: Box::new(move || write.result()), hook: None, last_key: None, output_from, reload_before });
            return Ok(DaemonMessage::SupervisorCommandReceipt { receipt });
        }
        anyhow::ensure!(self.supervisor_commands.records.contains_key(&owner), "The original command receipt is unavailable. Do not resend an uncertain command.");
        self.advance_queued_finish();
        let record = self.supervisor_commands.records.get_mut(&owner).expect("command owner checked");
        if record.receipt.phase == Phase::Queued {
            anyhow::ensure!(matches!(request, Request::Status), "No native command chooser is open.");
            return Ok(DaemonMessage::SupervisorCommandReceipt { receipt: record.receipt.clone() });
        }
        let pane = self.app.mux.supervisor().ok_or_else(|| anyhow::anyhow!("The supervisor is unavailable; use the session-ended receipt."))?;
        if let Request::Step { key } = request {
            // A timeout is uncertain execution, not evidence that Usage closed.
            // Recovery only permits Close, and still checks the live view below.
            let recover_usage = record.receipt.command == Command::Usage
                && record.receipt.phase == Phase::Unconfirmed && key == Key::Cancel;
            anyhow::ensure!((record.receipt.interactive || recover_usage) && matches!(record.receipt.command, Command::Model | Command::Usage), "No native command chooser is open.");
            anyhow::ensure!(record.receipt.command == Command::Model || key == Key::Cancel, "Usage only supports Close.");
            // Navigation remains scoped to the observed native dialog. A stale
            // browser cannot send a key after that dialog disappeared.
            let current = screen(pane);
            if !native_observation(record.receipt.command, "", &current, "").is_some_and(|(_, open)| open) {
                record.receipt.phase = Phase::Observed;
                record.receipt.interactive = false;
                record.receipt.detail = "The native chooser is no longer open. No additional change was confirmed.".into();
                return Ok(DaemonMessage::SupervisorCommandReceipt { receipt: record.receipt.clone() });
            }
            pane.deliver_user_input(key.bytes(), cas_mux::UserInputKind::KeyStream).await?;
            record.before = current;
            record.started = Instant::now();
            record.last_key = Some(key);
            record.receipt.phase = Phase::Submitted;
            record.receipt.interactive = false;
            record.receipt.detail = "Selection submitted; awaiting the harness's response.".into();
        }
        if record.receipt.phase == Phase::Submitted || record.receipt.interactive
            || record.receipt.phase == Phase::Unconfirmed && record.receipt.command == Command::Usage {
            let current = screen(pane);
            if pane.has_exited() && matches!(record.receipt.command, Command::Exit | Command::Finish) {
                record.receipt.phase = Phase::Observed;
                record.receipt.detail = "Supervisor process ended. See the session-ended receipt.".into();
                record.receipt.interactive = false;
            } else if record.last_key == Some(Key::Cancel) && current != record.before && idle_prompt(&current) {
                record.receipt.phase = Phase::Observed;
                record.receipt.detail = "The native chooser closed.".into();
                record.receipt.interactive = false;
            } else if record.receipt.interactive && idle_prompt(&current) {
                record.receipt.phase = Phase::Observed;
                record.receipt.detail = "The native chooser is no longer open. No additional change was confirmed.".into();
                record.receipt.interactive = false;
            } else if let Some(Err(error)) = (record.write)() {
                record.receipt.phase = Phase::Unconfirmed;
                record.receipt.detail = format!("Command write failed; execution is uncertain: {error}. Do not resend automatically.");
            } else if let Some((detail, interactive)) = native_observation(record.receipt.command,
                if matches!(record.last_key, Some(Key::Next | Key::Previous | Key::Select)) && current != record.before { "" } else { &record.before }, &current,
                // Codex prints /status into history; other harnesses keep the screen-diff rule.
                &if record.receipt.command == Command::Usage && pane.harness() == cas_mux::SupervisorCli::Codex {
                    self.pane_buffers.get(pane.id()).and_then(|buffer| buffer.plain_text_since(record.output_from)).unwrap_or_default()
                } else { String::new() }) {
                if record.receipt.command == Command::Model && !interactive && record.last_key == Some(Key::Select)
                    && pane.harness() == cas_mux::SupervisorCli::Codex
                {
                    let Some((model, effort)) = codex_model_selection(&record.before_command, &current) else {
                        record.receipt.phase = Phase::Unconfirmed;
                        record.receipt.detail = "No fresh model selection was confirmed by the harness.".into();
                        record.receipt.interactive = false;
                        record.receipt.output = super::work_events::redact(&current).chars().take(8 * 1024).collect();
                        return Ok(DaemonMessage::SupervisorCommandReceipt { receipt: record.receipt.clone() });
                    };
                    record_supervisor_model(self.app.cas_dir(), &self.session_name, self.app.supervisor_name(), &model, effort.as_deref());
                }
                record.receipt.phase = Phase::Observed;
                record.receipt.detail = detail;
                record.receipt.interactive = interactive;
                record.receipt.output = super::work_events::redact(&current).chars().take(8 * 1024).collect();
            } else if record.hook.as_ref().is_some_and(|job| job.is_finished()) {
                if let Some(Ok(Some(detail))) = record.hook.take().map(|job| futures_util::FutureExt::now_or_never(job)).flatten() {
                    record.receipt.phase = Phase::Observed;
                    record.receipt.detail = detail;
                }
            } else if record.hook.is_none() && matches!(record.receipt.command, Command::Compact | Command::Clear | Command::Finish | Command::ReloadCassy) {
                let root = self.app.cas_dir().to_path_buf();
                let session = self.session_name.clone(); let supervisor = self.app.supervisor_name().to_owned();
                let command = record.receipt.command; let after = record.at;
                let reload_before = record.reload_before;
                let cli = pane.harness();
                record.hook = Some(tokio::task::spawn_blocking(move || {
                    if command == Command::ReloadCassy { reload_before.and_then(|pid| reload_observation(&root, &session, &supervisor, pid)) }
                    else if command == Command::Finish { finish_observation(&root, &session, &supervisor, cli, after) }
                    else { hook_observation(&root, &session, &supervisor, command, after) }
                }));
            }
            if record.receipt.phase == Phase::Submitted && record.started.elapsed() > Duration::from_secs(90) {
                record.receipt.phase = Phase::Unconfirmed;
                record.receipt.detail = if record.receipt.command == Command::ReloadCassy {
                    "Cassy reload is unconfirmed: no replacement live MCP PID and version were recorded. Check Claude's /mcp panel before retrying."
                } else { "The harness outcome was not observed. Check Raw output before sending another command." }.into();
            }
        }
        Ok(DaemonMessage::SupervisorCommandReceipt { receipt: record.receipt.clone() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The daemon's poll loop samples PTY silence; these fixtures do not run it.
    fn quiet_for(daemon: &mut FactoryDaemon, name: &str, quiet: Duration) {
        let bytes = daemon.app.mux.pane_bytes_received(name).unwrap();
        daemon.last_pane_output_bytes.insert(name.into(), bytes);
        daemon.pane_silent_since.insert(name.into(), Instant::now() - quiet);
    }
    #[tokio::test(flavor = "current_thread")]
    async fn reload_cassy_requires_a_replacement_live_registry_pid_and_version() {
        use super::super::supervisor_single_turn_tests::{fixture_child, fixture_for};
        use cas_store::HistoryStore;
        if !fixture_child("ui::factory::daemon::runtime::supervisor_commands::tests::reload_cassy_requires_a_replacement_live_registry_pid_and_version") { return; }
        let _env = crate::test_env_guard::TestEnvGuard::temp_home();
        struct Child(std::process::Child);
        impl Drop for Child { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
        fn process() -> Child { Child(std::process::Command::new("/bin/sleep").arg("120").spawn().unwrap()) }
        fn record(daemon: &FactoryDaemon, pid: u32, version: Option<&str>) {
            let store = crate::store::open_agent_store(daemon.app.cas_dir()).unwrap();
            let mut agent = store.get("single-sup").unwrap();
            agent.pid = Some(pid);
            crate::mcp::daemon::stamp_pid_fingerprint(&mut agent, pid);
            store.update(&agent).unwrap();
            let mut epoch = crate::history::epochs::current_daemon_epoch(&Utc::now().to_rfc3339());
            epoch.pid = Some(pid.into()); epoch.version = version.map(str::to_owned);
            cas_store::SqliteHistoryStore::open(daemon.app.cas_dir()).unwrap().record_epoch(&epoch).unwrap();
        }
        let command: Command = serde_json::from_str("\"mcp\"").unwrap();
        let owner = MessageAttribution { device_id: Some("reload-owner".into()), operator_verified: true, ..Default::default() };
        for cli in [cas_mux::SupervisorCli::Claude, cas_mux::SupervisorCli::Codex, cas_mux::SupervisorCli::Grok, cas_mux::SupervisorCli::OpenCode] {
            let (root, mut daemon) = fixture_for(cli).await;
            let name = daemon.app.supervisor_name().to_owned();
            daemon.app.mux.get_mut(&name).unwrap().feed("\x1bc\x1b[20;1H❯".as_bytes()).unwrap();
            quiet_for(&mut daemon, &name, COMMAND_SETTLE);
            let run = Request::Run { command, confirmed: false };
            if cli != cas_mux::SupervisorCli::Claude {
                let error = daemon.dispatch_supervisor_command(run, "reload".into(), owner.clone()).await.unwrap_err();
                assert!(error.to_string().contains(&format!("{cli:?}").to_lowercase()), "{error}");
                assert!(std::fs::read(root.path().join("input")).unwrap().is_empty());
                continue;
            }
            let old = process();
            record(&daemon, old.0.id(), Some("9.99.1"));
            let store = crate::store::open_agent_store(daemon.app.cas_dir()).unwrap();
            let mut agent = store.get("single-sup").unwrap();
            agent.cc_session_id = Some("reload-version".into());
            store.update(&agent).unwrap();
            let projects = dirs::home_dir().unwrap().join(".claude/projects/fixture");
            std::fs::create_dir_all(&projects).unwrap();
            let transcript = projects.join("reload-version.jsonl");
            for version in [None, Some("2.1.310")] {
                std::fs::write(&transcript, format!("{}\n", serde_json::json!({"type":"user","sessionId":"reload-version","version":version}))).unwrap();
                let error = daemon.dispatch_supervisor_command(run.clone(), "reload-unverified".into(), owner.clone()).await.unwrap_err();
                assert!(error.to_string().contains("unverified version"), "{error}");
                assert!(std::fs::read(root.path().join("input")).unwrap().is_empty());
            }
            for version in ["2.1.311", "2.1.316"] {
                std::fs::write(&transcript, format!("{}\n", serde_json::json!({"type":"user","sessionId":"reload-version","version":version}))).unwrap();
                verify_claude_reload_version(daemon.app.cas_dir(), &daemon.session_name, &name)
                    .unwrap_or_else(|error| panic!("live-proved Claude {version} must permit reload: {error}"));
            }
            let receipt = daemon.dispatch_supervisor_command(run.clone(), "reload".into(), owner.clone()).await.unwrap();
            assert!(matches!(receipt, DaemonMessage::SupervisorCommandReceipt { receipt } if receipt.phase == Phase::Submitted));
            daemon.dispatch_supervisor_command(run, "reload".into(), owner.clone()).await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                while !std::fs::read(root.path().join("input")).unwrap().ends_with(b"\r") { tokio::task::yield_now().await; }
            }).await.unwrap();
            let writes = std::fs::read(root.path().join("input")).unwrap();
            assert_eq!(String::from_utf8_lossy(&writes).matches("/mcp reconnect cas").count(), 1, "one native command, even on replay");
            daemon.app.mux.get_mut(&name).unwrap().feed("\x1bcSuccessfully reconnected to cas\r\n❯".as_bytes()).unwrap();
            let echo = daemon.dispatch_supervisor_command(Request::Status, "reload".into(), owner.clone()).await.unwrap();
            assert!(matches!(echo, DaemonMessage::SupervisorCommandReceipt { receipt } if receipt.phase == Phase::Submitted));
            // Same PID and then an unversioned replacement both fail to prove reload.
            assert!(reload_observation(daemon.app.cas_dir(), &daemon.session_name, &name, old.0.id()).is_none());
            let replacement = process();
            record(&daemon, replacement.0.id(), None);
            assert!(reload_observation(daemon.app.cas_dir(), &daemon.session_name, &name, old.0.id()).is_none());
            record(&daemon, replacement.0.id(), Some("9.99.2"));
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let response = daemon.dispatch_supervisor_command(Request::Status, "reload".into(), owner.clone()).await.unwrap();
                    if let DaemonMessage::SupervisorCommandReceipt { receipt } = response {
                        if receipt.phase == Phase::Observed {
                            assert!(receipt.detail.contains(&format!("PID {}, version 9.99.2", replacement.0.id())), "{}", receipt.detail);
                            break;
                        }
                    }
                    tokio::task::yield_now().await;
                }
            }).await.expect("registry replacement must complete the original receipt");
            quiet_for(&mut daemon, &name, COMMAND_SETTLE);
            let replay = Request::Run { command, confirmed: false };
            daemon.dispatch_supervisor_command(replay.clone(), "reload-timeout".into(), owner.clone()).await.unwrap();
            let timeout_owner = ("reload-owner".into(), "reload-timeout".into());
            daemon.supervisor_commands.records.get_mut(&timeout_owner).unwrap().started -= Duration::from_secs(91);
            let run_timeout = daemon.dispatch_supervisor_command(replay, "reload-timeout".into(), owner.clone()).await.unwrap();
            let DaemonMessage::SupervisorCommandReceipt { receipt: run_timeout } = run_timeout else { panic!("owned reload receipt") };
            assert_eq!(run_timeout.phase, Phase::Unconfirmed);
            assert!(run_timeout.detail.contains("replacement live MCP PID and version"), "{}", run_timeout.detail);
            daemon.supervisor_commands.records.get_mut(&timeout_owner).unwrap().receipt.phase = Phase::Submitted;
            let status_timeout = daemon.dispatch_supervisor_command(Request::Status, "reload-timeout".into(), owner.clone()).await.unwrap();
            assert!(matches!(status_timeout, DaemonMessage::SupervisorCommandReceipt { receipt } if receipt.phase == Phase::Unconfirmed && receipt.detail == run_timeout.detail));
        }
    }
    #[tokio::test(flavor = "current_thread")]
    async fn finish_accepts_an_idle_supervisor_with_a_repainting_background_shell_cas_88ca() {
        use super::super::supervisor_single_turn_tests::{fixture_child, fixture};
        if !fixture_child("ui::factory::daemon::runtime::supervisor_commands::tests::finish_accepts_an_idle_supervisor_with_a_repainting_background_shell_cas_88ca") { return; }
        let _env = crate::test_env_guard::TestEnvGuard::temp_home();
        for mode in ["idle", "busy", "unknown"] {
            let mid_turn = mode == "busy";
            let (root, mut daemon) = fixture().await;
            let name = daemon.app.supervisor_name().to_owned();
            let store = crate::store::open_agent_store(daemon.app.cas_dir()).unwrap();
            let mut agent = store.get("single-sup").unwrap();
            agent.cc_session_id = Some("finish-background".into());
            store.update(&agent).unwrap();
            let projects = dirs::home_dir().unwrap().join(".claude/projects/fixture");
            std::fs::create_dir_all(&projects).unwrap();
            let at = Utc::now() - chrono::Duration::seconds(2);
            let start = serde_json::json!({"type":"user","timestamp":at,"message":{"role":"user","content":"Prior work"}});
            let end = serde_json::json!({"type":"assistant","timestamp":at + chrono::Duration::seconds(1),"message":{"role":"assistant","stop_reason":"end_turn"}});
            let transcript = projects.join("finish-background.jsonl");
            std::fs::write(&transcript, if mid_turn { format!("{start}\n") } else { format!("{start}\n{end}\n") }).unwrap();
            if mode == "unknown" { std::fs::remove_file(&transcript).unwrap(); }
            let pty = cas_mux::Pty::spawn(&name, cas_mux::PtyConfig {
                command: "/bin/sh".into(),
                args: vec!["-c".into(), "stty raw -echo; printf '\\033c\\033[20;1H❯'; (while :; do printf '\\0337\\033[24;1H1 running background task\\0338'; sleep 0.05; done) & repaint=$!; trap 'kill \"$repaint\"' EXIT; cat > \"$1\"".into(),
                    "fixture".into(), root.path().join("finish-input").to_str().unwrap().into()],
                cwd: Some(root.path().into()), env: vec![], env_remove: vec![], rows: 24, cols: 80,
            }).unwrap();
            let mut pane = cas_mux::Pane::with_pty(&name, cas_mux::PaneKind::Supervisor, pty, 24, 80, cas_mux::SupervisorCli::Claude).unwrap();
            pane.backdate_spawn_for_test(Duration::from_secs(6));
            daemon.app.mux.add_pane(pane);
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let pane = daemon.app.mux.get_mut(&name).unwrap();
                    pane.drain_output();
                    if idle_prompt(&screen(pane)) && screen(pane).contains("running background task") && root.path().join("finish-input").exists() { break; }
                    tokio::task::yield_now().await;
                }
            }).await.unwrap();
            if mode == "unknown" { quiet_for(&mut daemon, &name, COMMAND_SETTLE); }
            let owner = MessageAttribution { device_id: Some("device-a".into()), operator_verified: true, ..Default::default() };
            let receipt = daemon.dispatch_supervisor_command(
                Request::Run { command: Command::Finish, confirmed: true }, "finish-background".into(),
                owner.clone(),
            ).await.expect("native turn ended and the prompt is empty: background repaint must not refuse Finish");
            assert!(matches!(receipt, DaemonMessage::SupervisorCommandReceipt { receipt } if receipt.phase == Phase::Queued));
            if mid_turn || mode == "unknown" {
                tokio::time::timeout(Duration::from_secs(5), async {
                    while !daemon.supervisor_commands.readiness.as_ref().is_some_and(|job| job.is_finished()) { tokio::task::yield_now().await; }
                }).await.unwrap();
                let pending = daemon.dispatch_supervisor_command(Request::Status, "finish-background".into(), owner.clone()).await.unwrap();
                assert!(matches!(pending, DaemonMessage::SupervisorCommandReceipt { receipt } if receipt.phase == Phase::Queued));
                assert!(std::fs::read(root.path().join("finish-input")).unwrap().is_empty(), "a truly active turn receives no command");
                if mode == "unknown" { continue; }
                let mut foreign = owner.clone(); foreign.device_id = Some("device-b".into());
                assert!(daemon.dispatch_supervisor_command(Request::Status, "finish-background".into(), foreign).await.is_err());
                // Waiting on a long turn cannot expire the not-yet-submitted command.
                daemon.supervisor_commands.records.get_mut(&("device-a".into(), "finish-background".into())).unwrap().started = Instant::now() - REPLAY_WINDOW - Duration::from_secs(1);
                assert!(daemon.dispatch_supervisor_command(Request::Run { command: Command::Usage, confirmed: false }, "another-command".into(), owner.clone()).await.is_err());
                // Replay is caller-owned and cannot create another queued mutation.
                daemon.dispatch_supervisor_command(Request::Run { command: Command::Finish, confirmed: true }, "finish-background".into(), owner.clone()).await.unwrap();
                std::fs::write(&transcript, format!("{start}\n{end}\n")).unwrap();
            }
            tokio::time::timeout(Duration::from_secs(5), async {
                while !std::fs::read(root.path().join("finish-input")).unwrap().ends_with(b"\r") {
                    daemon.dispatch_supervisor_command(Request::Status, "finish-background".into(), owner.clone()).await.unwrap();
                    tokio::task::yield_now().await;
                }
            }).await.unwrap();
            assert_eq!(String::from_utf8_lossy(&std::fs::read(root.path().join("finish-input")).unwrap()).matches("/finish").count(), 1);
        }
    }
    #[tokio::test(flavor = "current_thread")]
    async fn command_immediately_after_real_reply_waits_for_silence_then_sends() {
        use super::super::supervisor_single_turn_tests::{fixture_child, fixture};
        if !fixture_child("ui::factory::daemon::runtime::supervisor_commands::tests::command_immediately_after_real_reply_waits_for_silence_then_sends") { return; }
        let _env = crate::test_env_guard::TestEnvGuard::temp_home();
        let (root, mut daemon) = fixture().await;
        let name = daemon.app.supervisor_name().to_owned();
        // A real PTY reply, not feed() or a backdated silence sample. The
        // command arrives as soon as the reply's empty prompt is visible.
        let pty = cas_mux::Pty::spawn(&name, cas_mux::PtyConfig {
            command: "/bin/sh".into(),
            args: vec!["-c".into(), "stty raw -echo; printf '\\033c\\033[20;1HReply complete\\r\\n❯'; exec cat > \"$1\"".into(),
                "fixture".into(), root.path().join("command-input").to_str().unwrap().into()],
            cwd: Some(root.path().into()), env: vec![], env_remove: vec![], rows: 24, cols: 80,
        }).unwrap();
        let mut pane = cas_mux::Pane::with_pty(&name, cas_mux::PaneKind::Supervisor, pty, 24, 80, cas_mux::SupervisorCli::Claude).unwrap();
        // Only skip harness startup grace; command silence still uses real time.
        pane.backdate_spawn_for_test(Duration::from_secs(6));
        daemon.app.mux.add_pane(pane);
        daemon.last_pane_output_bytes.remove(&name);
        daemon.pane_silent_since.remove(&name);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let pane = daemon.app.mux.get_mut(&name).unwrap();
                pane.drain_output();
                if idle_prompt(&screen(pane)) && root.path().join("command-input").exists() { break; }
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        let replied = Instant::now();
        let receipt = tokio::time::timeout(Duration::from_secs(4), daemon.dispatch_supervisor_command(
            Request::Run { command: Command::Exit, confirmed: true }, "cmd-after-reply".into(),
            MessageAttribution { device_id: Some("device-a".into()), operator_verified: true, ..Default::default() },
        )).await.expect("idle command must settle within a short bound").expect("a fresh idle reply must not cause a final busy refusal");
        assert!(matches!(receipt, DaemonMessage::SupervisorCommandReceipt { receipt } if receipt.phase == Phase::Submitted));
        assert!(replied.elapsed() >= COMMAND_SETTLE, "do not type before real silence is observed");
        tokio::time::timeout(Duration::from_secs(5), async {
            while !std::fs::read(root.path().join("command-input")).unwrap().ends_with(b"\r") { tokio::task::yield_now().await; }
        }).await.unwrap();
        let input = std::fs::read(root.path().join("command-input")).unwrap();
        assert_eq!(String::from_utf8_lossy(&input).matches("/exit").count(), 1);
    }
    #[tokio::test(flavor = "current_thread")]
    async fn native_dispatch_refuses_busy_and_foreign_callers_and_recovers_a_closed_chooser() {
        use super::super::supervisor_single_turn_tests::{fixture_child, fixture};
        if !fixture_child("ui::factory::daemon::runtime::supervisor_commands::tests::native_dispatch_refuses_busy_and_foreign_callers_and_recovers_a_closed_chooser") { return; }
        let _env = crate::test_env_guard::TestEnvGuard::temp_home();
        let (root, mut daemon) = fixture().await;
        let name = daemon.app.supervisor_name().to_owned();
        let owner = MessageAttribution { device_id: Some("device-a".into()), operator_verified: true, ..Default::default() };
        let request = Request::Run { command: Command::Model, confirmed: false };
        quiet_for(&mut daemon, &name, COMMAND_SETTLE);
        daemon.app.mux.get_mut(&name).unwrap().feed("\x1bc\x1b[20;1H✢ Flambéing… (7s · ↓ 633 tokens)\r\n❯".as_bytes()).unwrap();
        assert!(daemon.dispatch_supervisor_command(request.clone(), "cmd-model".into(), owner.clone()).await.is_err());
        assert!(std::fs::read(root.path().join("input")).unwrap().is_empty());
        daemon.app.mux.get_mut(&name).unwrap().feed("\x1bc\x1b[20;1H❯".as_bytes()).unwrap();
        let mut untrusted = owner.clone(); untrusted.operator_verified = false;
        assert!(daemon.dispatch_supervisor_command(request.clone(), "cmd-model".into(), untrusted).await.is_err());
        let first = daemon.dispatch_supervisor_command(request.clone(), "cmd-model".into(), owner.clone()).await.unwrap();
        assert!(matches!(first, DaemonMessage::SupervisorCommandReceipt { receipt } if receipt.phase == Phase::Submitted));
        daemon.dispatch_supervisor_command(request, "cmd-model".into(), owner.clone()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !std::fs::read(root.path().join("input")).unwrap().ends_with(b"\r") { tokio::task::yield_now().await; }
        }).await.unwrap();
        let writes = std::fs::read(root.path().join("input")).unwrap();
        assert_eq!(String::from_utf8_lossy(&writes).matches("/model").count(), 1);
        let mut foreign = owner.clone(); foreign.device_id = Some("device-b".into());
        assert!(daemon.dispatch_supervisor_command(Request::Status, "cmd-model".into(), foreign).await.is_err());
        daemon.app.mux.get_mut(&name).unwrap().feed(b"\x1bcSelect model\r\nOpus\r\nSonnet").unwrap();
        let opened = daemon.dispatch_supervisor_command(Request::Status, "cmd-model".into(), owner.clone()).await.unwrap();
        assert!(matches!(opened, DaemonMessage::SupervisorCommandReceipt { receipt } if receipt.phase == Phase::Observed && receipt.interactive));
        daemon.app.mux.get_mut(&name).unwrap().feed("\x1bc\x1b[20;1H❯".as_bytes()).unwrap();
        let closed = daemon.dispatch_supervisor_command(Request::Step { key: Key::Cancel }, "cmd-model".into(), owner.clone()).await.unwrap();
        assert!(matches!(closed, DaemonMessage::SupervisorCommandReceipt { receipt } if !receipt.interactive));
        assert_eq!(std::fs::read(root.path().join("input")).unwrap(), writes, "stale controls must send no key");
        daemon.dispatch_supervisor_command(Request::Run { command: Command::Usage, confirmed: false }, "cmd-usage".into(), owner).await.unwrap();
        // Pane names are human names; the registry's primary key is a different ID.
        let cas_dir = daemon.app.cas_dir();
        let store = crate::store::open_agent_store(cas_dir).unwrap();
        let mut agent = store.get("single-sup").unwrap();
        agent.cc_session_id = Some("finish-fixture".into()); store.update(&agent).unwrap();
        let projects = dirs::home_dir().unwrap().join(".claude/projects/fixture");
        std::fs::create_dir_all(&projects).unwrap();
        let at = Utc::now();
        std::fs::write(projects.join("finish-fixture.jsonl"), format!("{}\n{}\n",
            serde_json::json!({"type":"user","timestamp":at,"message":{"content":"<command-name>/finish</command-name>"}}),
            serde_json::json!({"type":"assistant","timestamp":at,"message":{"stop_reason":"end_turn"}}))).unwrap();
        assert!(finish_observation(cas_dir, &daemon.session_name, &name, cas_mux::SupervisorCli::Claude, at).is_some());
        assert!(finish_observation(cas_dir, "another-session", &name, cas_mux::SupervisorCli::Claude, at).is_none());
        let (root, mut codex) = super::super::supervisor_single_turn_tests::fixture_for(cas_mux::SupervisorCli::Codex).await;
        let name = codex.app.supervisor_name().to_owned();
        codex.app.mux.get_mut(&name).unwrap().feed("\x1bc\x1b[19;1H• Model changed to old-model high for this conversation\r\n›".as_bytes()).unwrap();
        let owner = MessageAttribution { device_id: Some("device-a".into()), operator_verified: true, ..Default::default() };
        // Codex can pause past the silence window under an idle-looking
        // prompt. Its real rollout must still refuse a command (cas-3980).
        let store = crate::store::open_agent_store(codex.app.cas_dir()).unwrap();
        let mut agent = store.get("single-sup").unwrap();
        let sid = "019f84af-3121-7950-ba14-b01db2dad6c7";
        agent.cc_session_id = Some(sid.into());
        agent.metadata.insert("supervisor_account_dir".into(), root.path().to_str().unwrap().into());
        store.update(&agent).unwrap();
        let sessions = root.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let rollout = sessions.join(format!("rollout-{sid}.jsonl"));
        let meta = serde_json::json!({"type":"session_meta","payload":{"id":sid,"cwd":root.path(),"originator":"codex-tui","source":"cli"}});
        let turn = |kind: &str| serde_json::json!({"type":"event_msg","payload":{"type":kind,"turn_id":"t1"}});
        std::fs::write(&rollout, format!("{meta}\n{}\n", turn("task_started"))).unwrap();
        assert!(supervisor_transcript(codex.app.cas_dir(), &codex.session_name, &name, cas_mux::SupervisorCli::Codex).is_some());
        quiet_for(&mut codex, &name, COMMAND_SETTLE);
        let input = std::fs::read(root.path().join("input")).unwrap();
        assert!(codex.dispatch_supervisor_command(Request::Run { command: Command::Model, confirmed: false }, "cmd-streaming".into(), owner.clone()).await.is_err());
        assert_eq!(std::fs::read(root.path().join("input")).unwrap(), input, "a streaming turn must receive no command");
        std::fs::write(&rollout, format!("{meta}\n{}\n{}\n", turn("task_started"), turn("task_complete"))).unwrap();
        quiet_for(&mut codex, &name, COMMAND_SETTLE);
        agent.metadata.insert("supervisor_model".into(), "old-model".into());
        store.update(&agent).unwrap();
        codex.dispatch_supervisor_command(Request::Run { command: Command::Model, confirmed: false }, "cmd-select".into(), owner.clone()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while codex.app.mux.pane_inject_busy(&name) { tokio::task::yield_now().await; }
        }).await.unwrap();
        codex.app.mux.get_mut(&name).unwrap().feed(b"\x1bcSelect model\r\ngpt-6.1-sol").unwrap();
        codex.dispatch_supervisor_command(Request::Status, "cmd-select".into(), owner.clone()).await.unwrap();
        assert_eq!(store.get("single-sup").unwrap().metadata.get("supervisor_model").map(String::as_str), Some("old-model"));
        codex.dispatch_supervisor_command(Request::Step { key: Key::Select }, "cmd-select".into(), owner.clone()).await.unwrap();
        // The old chat history reappears after a failed save. It is not a new selection.
        codex.app.mux.get_mut(&name).unwrap().feed("\x1bc\x1b[19;1H• Model changed to old-model high for this conversation\r\n• Failed to save default model: denied\r\n›".as_bytes()).unwrap();
        codex.dispatch_supervisor_command(Request::Status, "cmd-select".into(), owner.clone()).await.unwrap();
        assert!(!store.get("single-sup").unwrap().metadata.contains_key("supervisor_effort"), "resurfaced history cannot invent a fresh observation");
        codex.dispatch_supervisor_command(Request::Run { command: Command::Model, confirmed: false }, "cmd-select-2".into(), owner.clone()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while codex.app.mux.pane_inject_busy(&name) { tokio::task::yield_now().await; }
        }).await.unwrap();
        codex.app.mux.get_mut(&name).unwrap().feed(b"\x1bcSelect model\r\ngpt-6.1-sol").unwrap();
        codex.dispatch_supervisor_command(Request::Status, "cmd-select-2".into(), owner.clone()).await.unwrap();
        codex.dispatch_supervisor_command(Request::Step { key: Key::Select }, "cmd-select-2".into(), owner.clone()).await.unwrap();
        codex.app.mux.get_mut(&name).unwrap().feed("\x1bc\x1b[20;1H• Model changed to gpt-6.1-sol xhigh for this conversation\r\n›".as_bytes()).unwrap();
        codex.dispatch_supervisor_command(Request::Status, "cmd-select-2".into(), owner.clone()).await.unwrap();
        let observed = store.get("single-sup").unwrap();
        assert_eq!(observed.metadata.get("supervisor_model").map(String::as_str), Some("gpt-6.1-sol"));
        assert_eq!(observed.metadata.get("supervisor_effort").map(String::as_str), Some("xhigh"));
        codex.dispatch_supervisor_command(Request::Run { command: Command::Finish, confirmed: true }, "cmd-finish".into(), owner).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !std::fs::read(root.path().join("input")).unwrap().ends_with(b"\r") {
                codex.advance_queued_finish();
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        let input = std::fs::read(root.path().join("input")).unwrap();
        assert!(!input.windows(b"$finish".len()).any(|bytes| bytes == b"$finish"), "Codex's skill picker must not consume the command submit");
    }
    /// cas-7629: receipts past the replay window are evicted, so command 257
    /// still runs, while a replay inside the window returns its stored receipt.
    #[tokio::test(flavor = "current_thread")]
    async fn command_receipts_expire_after_the_replay_window_but_replays_inside_it_do_not_rerun() {
        use super::super::supervisor_single_turn_tests::{fixture_child, fixture};
        if !fixture_child("ui::factory::daemon::runtime::supervisor_commands::tests::command_receipts_expire_after_the_replay_window_but_replays_inside_it_do_not_rerun") { return; }
        let _env = crate::test_env_guard::TestEnvGuard::temp_home();
        let (root, mut daemon) = fixture().await;
        let name = daemon.app.supervisor_name().to_owned();
        quiet_for(&mut daemon, &name, COMMAND_SETTLE);
        daemon.app.mux.get_mut(&name).unwrap().feed("\x1bc\x1b[20;1H❯".as_bytes()).unwrap();
        let owner = MessageAttribution { device_id: Some("device-a".into()), operator_verified: true, ..Default::default() };
        let usage = Request::Run { command: Command::Usage, confirmed: false };
        let aged = Instant::now().checked_sub(REPLAY_WINDOW + Duration::from_secs(1)).unwrap();
        for n in 0..=RECEIPT_CAP {
            let reference = format!("cmd-{n}");
            let sent = daemon.dispatch_supervisor_command(usage.clone(), reference.clone(), owner.clone()).await
                .unwrap_or_else(|error| panic!("command {n} refused: {error:#}"));
            assert!(matches!(sent, DaemonMessage::SupervisorCommandReceipt { receipt } if receipt.phase == Phase::Submitted));
            tokio::time::timeout(Duration::from_secs(5), async {
                while daemon.app.mux.pane_inject_busy(&name) { tokio::task::yield_now().await; }
            }).await.unwrap();
            // The harness outcome was observed; all but the newest are past the window.
            let record = daemon.supervisor_commands.records.get_mut(&("device-a".to_owned(), reference)).unwrap();
            record.receipt.phase = Phase::Observed;
            if n < RECEIPT_CAP { record.started = aged; }
        }
        let runs = || String::from_utf8_lossy(&std::fs::read(root.path().join("input")).unwrap()).matches("/usage").count();
        tokio::time::timeout(Duration::from_secs(5), async { while runs() < RECEIPT_CAP + 1 { tokio::task::yield_now().await; } }).await.unwrap();
        assert_eq!(runs(), RECEIPT_CAP + 1);
        let newest = format!("cmd-{RECEIPT_CAP}");
        let replay = daemon.dispatch_supervisor_command(usage, newest, owner).await.unwrap();
        assert!(matches!(replay, DaemonMessage::SupervisorCommandReceipt { receipt } if receipt.phase == Phase::Observed),
            "a replay inside the window returns the stored receipt");
        assert_eq!(runs(), RECEIPT_CAP + 1, "a replay inside the window must not run the command again");
    }
    #[test]
    fn finish_needs_its_own_completed_harness_turn_not_an_echo_or_unrelated_reply() {
        use cas_mux::SupervisorCli::{Claude, Codex};
        use serde_json::json;
        let at = Utc::now();
        for (cli, prompt, done) in [
            (Claude, json!({"type":"user","timestamp":at,"message":{"content":"<command-message>finish</command-message>\n<command-name>/finish</command-name>"}}),
                json!({"type":"assistant","timestamp":at,"message":{"stop_reason":"end_turn"}})),
            (Codex, json!({"type":"response_item","timestamp":at,"payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"finish"}]}}),
                json!({"type":"event_msg","timestamp":at,"payload":{"type":"task_complete"}})),
        ] {
            assert!(!finish_turn_handled(&[prompt.clone()], cli, at));
            assert!(!finish_turn_handled(&[done.clone()], cli, at));
            assert!(finish_turn_handled(&[prompt.clone(), done.clone()], cli, at));
            assert!(!finish_turn_handled(&[prompt, done], cli, at + chrono::Duration::seconds(1)));
        }
    }
    #[test]
    fn codex_rollout_is_busy_from_turn_start_until_its_end_even_past_one_tail_window() {
        use std::io::Write;
        use serde_json::json;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout.jsonl");
        let event = |kind: &str, turn: &str| json!({"type":"event_msg","payload":{"type":kind,"turn_id":turn}}).to_string() + "\n";
        // A killed earlier turn left its start unclosed; the last marker decides.
        std::fs::write(&path, event("task_started", "killed") + &event("task_started", "t1") + &event("task_complete", "t1")).unwrap();
        assert_eq!(codex_turn_open(&path), Some(false));
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        // Streamed output larger than the first tail window, quoting an end marker as text.
        let text = format!("{} \"type\":\"task_complete\"", "7\n".repeat(200 * 1024));
        let item = json!({"type":"event_msg","payload":{"type":"item_completed","item":{"text":text}}}).to_string() + "\n";
        write!(file, "{}{item}", event("task_started", "t2")).unwrap();
        assert_eq!(codex_turn_open(&path), Some(true), "Codex is mid-turn while its output is silent");
        write!(file, "{}", event("task_complete", "t2")).unwrap();
        assert_eq!(codex_turn_open(&path), Some(false));
    }
    #[test]
    fn command_echo_is_not_execution_and_native_dialogs_do_not_claim_model_changes() {
        let reload: Command = serde_json::from_str("\"mcp\"").expect("Commander must accept its semantic Reload Cassy command");
        assert_eq!(reload.mapping(cas_mux::SupervisorCli::Claude), Some("/mcp reconnect cas"));
        for cli in [cas_mux::SupervisorCli::Codex, cas_mux::SupervisorCli::Grok, cas_mux::SupervisorCli::OpenCode] {
            assert_eq!(reload.mapping(cli), None);
        }
        assert!(native_observation(reload, "❯", "❯ /mcp reconnect cas\nSuccessfully reconnected to cas\n❯", "").is_none(), "only registry process/version evidence may confirm reload");
        assert!(native_observation(Command::Usage, "❯", "Current session\nCurrent week (all models)\nResets tomorrow", "").is_some_and(|(_, open)| open));
        assert!(native_observation(Command::Compact, "❯", "❯ /compact\n❯", "").is_none());
        assert!(native_observation(Command::Compact, "›", "• Context compacted · 44s\n›", "").is_some());
        assert!(native_observation(Command::Model, "Set model to Sonnet\n❯", "Set model to Sonnet\n❯ /model", "").is_none());
        let opened = native_observation(Command::Model, "❯", "Select model\nOpus\nSonnet", "").unwrap();
        assert!(opened.1);
        assert!(opened.0.contains("chooser opened"));
        assert!(native_observation(Command::Model, "Select model\nOpus\nSonnet", "Select model\nOpus\nSonnet", "").is_none());
        let confirmed = native_observation(Command::Model, "Select model", "Set model to Sonnet\n❯", "").unwrap();
        assert!(!confirmed.1);
        assert_eq!(codex_model_selection("Select model", "• Model changed to gpt-6.1-sol xhigh for this conversation\n›"), Some(("gpt-6.1-sol".into(), Some("xhigh".into()))));
        for text in ["Select model\ngpt-6.1-sol", "› /model gpt-6.1-sol", "Model changed to gpt-6.1-sol xhigh", "• Failed to save default model: error"] {
            assert!(codex_model_selection("", text).is_none(), "{text}");
        }
        assert!(codex_model_selection("• Model changed to gpt-6.1-sol high", "• Model changed to gpt-6.1-sol high\n›").is_none());
    }
    #[tokio::test(flavor = "current_thread")]
    async fn captured_claude_usage_can_be_observed_and_closed_even_after_timeout() {
        use super::super::supervisor_single_turn_tests::{fixture_child, fixture};
        if !fixture_child("ui::factory::daemon::runtime::supervisor_commands::tests::captured_claude_usage_can_be_observed_and_closed_even_after_timeout") { return; }
        let _env = crate::test_env_guard::TestEnvGuard::temp_home();
        let (root, mut daemon) = fixture().await;
        let name = daemon.app.supervisor_name().to_owned();
        let owner = MessageAttribution { device_id: Some("device-a".into()), operator_verified: true, ..Default::default() };
        quiet_for(&mut daemon, &name, COMMAND_SETTLE);
        daemon.app.mux.get_mut(&name).unwrap().feed("\x1bc\x1b[20;1H❯".as_bytes()).unwrap();
        daemon.dispatch_supervisor_command(Request::Run { command: Command::Usage, confirmed: false }, "usage-capture".into(), owner.clone()).await.unwrap();
        // A real view can only open after the paced command submit reaches
        // the harness. Do not race that submit with this fixture's Escape.
        tokio::time::timeout(Duration::from_secs(5), async {
            while daemon.app.mux.pane_inject_busy(&name)
                || !std::fs::read(root.path().join("input")).unwrap().ends_with(b"\r") {
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        // The original 90-second outcome expired before the Usage view arrived.
        daemon.supervisor_commands.records.get_mut(&("device-a".into(), "usage-capture".into())).unwrap().started -= Duration::from_secs(91);
        let expired = daemon.dispatch_supervisor_command(Request::Status, "usage-capture".into(), owner.clone()).await.unwrap();
        assert!(matches!(expired, DaemonMessage::SupervisorCommandReceipt { receipt } if receipt.phase == Phase::Unconfirmed));
        // Real Claude 2.1.312, 24x80, 2026-10-31. No "Current session".
        let captured = include_str!("fixtures/claude-2.1.312-usage.txt");
        let view = format!("\x1bc{}", captured.replace('\n', "\r\n"));
        daemon.app.mux.get_mut(&name).unwrap().feed(view.as_bytes()).unwrap();
        let observed = daemon.dispatch_supervisor_command(Request::Status, "usage-capture".into(), owner.clone()).await.unwrap();
        assert!(matches!(observed, DaemonMessage::SupervisorCommandReceipt { ref receipt } if receipt.phase == Phase::Observed && receipt.interactive), "{observed:?}");
        // Commander stops polling at Unconfirmed: its recovery Close must work
        // without first receiving the late observation above.
        let record = daemon.supervisor_commands.records.get_mut(&("device-a".into(), "usage-capture".into())).unwrap();
        record.receipt.phase = Phase::Unconfirmed;
        record.receipt.interactive = false;
        assert!(daemon.dispatch_supervisor_command(Request::Step { key: Key::Next }, "usage-capture".into(), owner.clone()).await.is_err());
        daemon.dispatch_supervisor_command(Request::Step { key: Key::Cancel }, "usage-capture".into(), owner.clone()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !std::fs::read(root.path().join("input")).unwrap().ends_with(b"\x1b") { tokio::task::yield_now().await; }
        }).await.unwrap();
        daemon.app.mux.get_mut(&name).unwrap().feed("\x1bc\x1b[20;1H❯".as_bytes()).unwrap();
        let closed = daemon.dispatch_supervisor_command(Request::Status, "usage-capture".into(), owner.clone()).await.unwrap();
        assert!(matches!(closed, DaemonMessage::SupervisorCommandReceipt { receipt } if receipt.phase == Phase::Observed && !receipt.interactive));
        let input = std::fs::read(root.path().join("input")).unwrap();
        assert_eq!(String::from_utf8_lossy(&input).matches("/usage").count(), 1, "Close must not replay the uncertain command");
        // A stale recovery control must not send Escape to the idle prompt.
        daemon.supervisor_commands.records.get_mut(&("device-a".into(), "usage-capture".into())).unwrap().receipt.phase = Phase::Unconfirmed;
        daemon.dispatch_supervisor_command(Request::Step { key: Key::Cancel }, "usage-capture".into(), owner.clone()).await.unwrap();
        assert_eq!(std::fs::read(root.path().join("input")).unwrap(), input);
        daemon.dispatch_supervisor_command(Request::Run { command: Command::Usage, confirmed: false }, "usage-next".into(), owner).await.unwrap();
    }
    #[tokio::test(flavor = "current_thread")]
    async fn native_clear_and_a_repeat_codex_status_are_observed() {
        use super::super::supervisor_single_turn_tests::{fixture_child, fixture_for};
        if !fixture_child("ui::factory::daemon::runtime::supervisor_commands::tests::native_clear_and_a_repeat_codex_status_are_observed") { return; }
        let _env = crate::test_env_guard::TestEnvGuard::temp_home();
        let (root, mut daemon) = fixture_for(cas_mux::SupervisorCli::Codex).await;
        let name = daemon.app.supervisor_name().to_owned();
        quiet_for(&mut daemon, &name, COMMAND_SETTLE);
        daemon.app.mux.get_mut(&name).unwrap().feed("\x1bc\x1b[20;1HREADY\r\n›".as_bytes()).unwrap();
        let owner = MessageAttribution { device_id: Some("device-a".into()), operator_verified: true, ..Default::default() };
        let submitted = daemon.dispatch_supervisor_command(
            Request::Run { command: Command::Clear, confirmed: true }, "cmd-clear".into(), owner.clone(),
        ).await.unwrap();
        assert!(matches!(submitted, DaemonMessage::SupervisorCommandReceipt { receipt } if receipt.phase == Phase::Submitted));
        tokio::time::timeout(Duration::from_secs(5), async {
            while !std::fs::read(root.path().join("input")).unwrap().ends_with(b"\r") { tokio::task::yield_now().await; }
        }).await.unwrap();
        daemon.app.mux.get_mut(&name).unwrap().feed(
            "\x1bc\x1b[1;1HOpenAI Codex (v0.165.0)\r\nTip: Use the desktop app.\r\n\x1b[20;1H› Ask Codex to do anything".as_bytes(),
        ).unwrap();
        let observed = daemon.dispatch_supervisor_command(Request::Status, "cmd-clear".into(), owner.clone()).await.unwrap();
        assert!(matches!(observed, DaemonMessage::SupervisorCommandReceipt { receipt }
            if receipt.phase == Phase::Observed && receipt.detail.contains("welcome prompt")));
        // A repeat /status prints the same cell; its old copy scrolls away and
        // the screen is unchanged, so only the pane's new output proves it (cas-7ec8).
        let status = "\x1bc\x1b[12;1H• Model: gpt-6.1-sol (reasoning high)\r\n• Context window: 72% left\r\n\x1b[20;1H› Ask Codex to do anything";
        daemon.app.mux.get_mut(&name).unwrap().feed(status.as_bytes()).unwrap();
        quiet_for(&mut daemon, &name, COMMAND_SETTLE);
        let input = std::fs::read(root.path().join("input")).unwrap().len();
        daemon.dispatch_supervisor_command(Request::Run { command: Command::Usage, confirmed: false }, "cmd-status".into(), owner.clone()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !std::fs::read(root.path().join("input")).unwrap()[input..].ends_with(b"\r") { tokio::task::yield_now().await; }
        }).await.unwrap();
        async fn daemon_status(daemon: &mut FactoryDaemon, client_ref: &str, owner: &MessageAttribution) -> Phase {
            match daemon.dispatch_supervisor_command(Request::Status, client_ref.into(), owner.clone()).await.unwrap() {
                DaemonMessage::SupervisorCommandReceipt { receipt } => receipt.phase,
                _ => unreachable!(),
            }
        }
        daemon.app.mux.get_mut(&name).unwrap().feed(status.as_bytes()).unwrap();
        assert_eq!(daemon_status(&mut daemon, "cmd-status", &owner).await, Phase::Submitted, "an unchanged screen alone proves nothing");
        daemon.buffer_pane_output(&name, status.as_bytes());
        assert_eq!(daemon_status(&mut daemon, "cmd-status", &owner).await, Phase::Observed);
    }
    #[test]
    fn prompt_readiness_refuses_busy_and_modal_screens() {
        assert!(idle_prompt("A reply\n❯\n? for shortcuts"));
        assert!(idle_prompt("❯ Try \"fix lint errors\"\n? for shortcuts"));
        assert!(!idle_prompt("✽ Compacting conversation… (14s · ↓ 975 tokens)\n❯"));
        assert!(!idle_prompt("✢ Flambéing… (7s · ↓ 633 tokens)\n❯"));
        assert!(!idle_prompt("· Flambéing… (8s · ↓ 640 tokens)\n❯"), "Claude's `·` spinner frame is busy (cas-1a93)");
        assert!(idle_prompt("✻ Cooked for 20s · done 11:21 PM\n❯"));
        assert!(!idle_prompt("Working · esc to interrupt\n❯"));
        assert!(!idle_prompt("Select model\n❯"));
        assert!(!idle_prompt("Still booting"));
    }
}
