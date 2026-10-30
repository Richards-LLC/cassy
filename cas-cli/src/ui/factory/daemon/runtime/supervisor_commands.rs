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
    /// A deliberate restart between the kill and the replacement pane.
    restart: Option<PendingRestart>,
}
struct PendingRestart { killed_at: Instant, exited: bool }
/// Replace the pane this long after the kill even if no exit was reported: a
/// descendant holding the PTY open must not leave the factory without a supervisor.
const RESTART_EXIT_DEADLINE: Duration = Duration::from_secs(10);
/// PTY silence required before a command is typed. Codex hides its busy text
/// while it streams a reply, so an idle-looking screen alone is insufficient.
const COMMAND_SETTLE: Duration = Duration::from_secs(2);
impl CommandState {
    /// Drop finished receipts past the replay window. An in-flight command or
    /// open chooser is never evicted, and nothing inside the window is, so a
    /// replayed reference still returns its stored receipt instead of re-running.
    fn evict_expired(&mut self) {
        self.records.retain(|_, record| {
            record.receipt.phase == Phase::Submitted || record.receipt.interactive || record.started.elapsed() <= REPLAY_WINDOW
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
                    if record.receipt.phase == Phase::Submitted || record.receipt.interactive {
                        record.receipt.phase = Phase::Unconfirmed;
                        record.receipt.interactive = false;
                        record.receipt.detail = "The supervisor restarted before this command was confirmed. Do not resend automatically.".into();
                    }
                }
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
            line.starts_with(['✢', '✻', '✽', '✶', '✳', '✣', '*'])
                && (line.contains('…') || line.contains("..."))
        })
        && text.lines().rev().take(12).any(|line| {
            let line = line.trim();
            matches!(line, "❯" | "›") || line.starts_with("❯ Try \"")
                || line == "› Ask Codex to do anything"
        })
}

fn native_observation(command: Command, before: &str, current: &str) -> Option<(String, bool)> {
    if before == current { return None; }
    let lower = current.to_lowercase();
    let fresh = |marker: &str| lower.contains(marker) && !before.to_lowercase().contains(marker);
    match command {
        Command::Compact if fresh("compacted (ctrl+o") || fresh("context compacted ·") =>
            Some(("Context compaction confirmed by the supervisor's harness.".into(), false)),
        Command::Clear if lower.contains("openai codex") && idle_prompt(current)
            && (fresh("tip: use the desktop app") || fresh("to get started, describe a task")) =>
                Some(("Codex returned to its fresh native welcome prompt after Clear.".into(), false)),
        Command::Model if fresh("select model") || fresh("select a model")
            || fresh("select reasoning effort") || fresh("select reasoning level") => Some(("Model chooser opened in the supervisor's harness.".into(), true)),
        Command::Model if codex_model_selection(before, current).is_some() || fresh("set model to") || fresh("model changed to") => Some(("The harness confirmed a model selection.".into(), false)),
        Command::Usage if fresh("current session") && (lower.contains("current week") || lower.contains("resets"))
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

fn finish_observation(cas_dir: &std::path::Path, session: &str, supervisor: &str, cli: cas_mux::SupervisorCli, after: DateTime<Utc>) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let store = crate::store::open_agent_store(cas_dir).ok()?;
    let agent = store.list(None).ok()?.into_iter().find(|agent|
        (agent.name == supervisor || agent.id == supervisor)
            && agent.role == cas_types::AgentRole::Supervisor
            && agent.factory_session.as_deref() == Some(session))?;
    let path = crate::mcp::tools::service::factory_ops::resolve_worker_transcript_path_for_account(
        cas_dir.parent()?.to_str(), agent.cc_session_id.as_deref()?, cli,
        agent.metadata.get("supervisor_account_dir").map(String::as_str))?;
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
    pub(super) async fn dispatch_supervisor_command(&mut self, request: Request, client_ref: String, attribution: MessageAttribution) -> anyhow::Result<DaemonMessage> {
        anyhow::ensure!(attribution.operator_verified, "Commands require an authenticated hub relay.");
        let device = attribution.device_id.as_deref().filter(|s| !s.is_empty()).ok_or_else(|| anyhow::anyhow!("missing command owner"))?;
        anyhow::ensure!(!client_ref.is_empty() && client_ref.len() <= 128 && client_ref.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'), "Invalid command reference.");
        let owner = (device.to_owned(), client_ref.clone());
        if let Request::Run { command, confirmed } = &request {
            for record in self.supervisor_commands.records.values_mut() {
                if record.receipt.phase == Phase::Submitted && record.started.elapsed() > Duration::from_secs(90) {
                    record.receipt.phase = Phase::Unconfirmed;
                    record.receipt.detail = "The harness outcome was not observed. Check Raw output before sending another command.".into();
                }
            }
            if let Some(record) = self.supervisor_commands.records.get(&owner) {
                anyhow::ensure!(record.receipt.command == *command, "Command reference already belongs to another command.");
                return Ok(DaemonMessage::SupervisorCommandReceipt { receipt: record.receipt.clone() });
            }
            self.supervisor_commands.evict_expired();
            anyhow::ensure!(self.supervisor_commands.records.len() < RECEIPT_CAP, "Too many supervisor commands in the last 10 minutes. Try again shortly.");
            anyhow::ensure!(!command.needs_confirmation() || *confirmed, "This command needs confirmation.");
            anyhow::ensure!(!self.supervisor_commands.records.values().any(|r| r.receipt.phase == Phase::Submitted || r.receipt.interactive), "Another supervisor command is awaiting an outcome.");
            let pane = self.app.mux.supervisor().ok_or_else(|| anyhow::anyhow!("The supervisor has not started."))?;
            let text = command.mapping(pane.harness()).ok_or_else(|| anyhow::anyhow!("This harness does not support that command."))?;
            let before = screen(pane);
            anyhow::ensure!(!pane.has_exited() && pane.ready_for_injection() && !pane.is_composer_dirty()
                && !self.app.mux.pane_inject_busy(pane.id()) && idle_prompt(&before),
                "The supervisor is busy or its empty prompt is not confirmed. Wait until its turn ends.");
            let write = pane.inject_prompt_tracked(text)?;
            let receipt = Receipt { device_id: device.to_owned(), client_ref: client_ref.clone(), command: *command, phase: Phase::Submitted,
                detail: "Command submitted; harness execution is not confirmed yet.".into(), output: String::new(), interactive: false };
            self.supervisor_commands.records.insert(owner, Record { receipt: receipt.clone(), started: Instant::now(), at: Utc::now(), before_command: before.clone(), before, write: Box::new(move || write.result()), hook: None, last_key: None });
            return Ok(DaemonMessage::SupervisorCommandReceipt { receipt });
        }
        let record = self.supervisor_commands.records.get_mut(&owner).ok_or_else(|| anyhow::anyhow!("The original command receipt is unavailable. Do not resend an uncertain command."))?;
        let pane = self.app.mux.supervisor().ok_or_else(|| anyhow::anyhow!("The supervisor is unavailable; use the session-ended receipt."))?;
        if let Request::Step { key } = request {
            anyhow::ensure!(record.receipt.interactive && matches!(record.receipt.command, Command::Model | Command::Usage), "No native command chooser is open.");
            anyhow::ensure!(record.receipt.command == Command::Model || key == Key::Cancel, "Usage only supports Close.");
            // Navigation remains scoped to the observed native dialog. A stale
            // browser cannot send a key after that dialog disappeared.
            let current = screen(pane);
            if !native_observation(record.receipt.command, "", &current).is_some_and(|(_, open)| open) {
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
        if record.receipt.phase == Phase::Submitted || record.receipt.interactive {
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
                if matches!(record.last_key, Some(Key::Next | Key::Previous | Key::Select)) && current != record.before { "" } else { &record.before }, &current) {
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
            } else if record.hook.is_none() && matches!(record.receipt.command, Command::Compact | Command::Clear | Command::Finish) {
                let root = self.app.cas_dir().to_path_buf();
                let session = self.session_name.clone(); let supervisor = self.app.supervisor_name().to_owned();
                let command = record.receipt.command; let after = record.at;
                let cli = pane.harness();
                record.hook = Some(tokio::task::spawn_blocking(move || {
                    if command == Command::Finish { finish_observation(&root, &session, &supervisor, cli, after) }
                    else { hook_observation(&root, &session, &supervisor, command, after) }
                }));
            }
            if record.receipt.phase == Phase::Submitted && record.started.elapsed() > Duration::from_secs(90) {
                record.receipt.phase = Phase::Unconfirmed;
                record.receipt.detail = "The harness outcome was not observed. Check Raw output before sending another command.".into();
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
        // Codex streams under an idle-looking prompt with no busy text (cas-1a93).
        quiet_for(&mut codex, &name, Duration::ZERO);
        let input = std::fs::read(root.path().join("input")).unwrap();
        assert!(codex.dispatch_supervisor_command(Request::Run { command: Command::Model, confirmed: false }, "cmd-streaming".into(), owner.clone()).await.is_err());
        assert_eq!(std::fs::read(root.path().join("input")).unwrap(), input, "a streaming turn must receive no command");
        quiet_for(&mut codex, &name, COMMAND_SETTLE);
        let store = crate::store::open_agent_store(codex.app.cas_dir()).unwrap();
        let mut agent = store.get("single-sup").unwrap();
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
            while !std::fs::read(root.path().join("input")).unwrap().ends_with(b"\r") { tokio::task::yield_now().await; }
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
    fn command_echo_is_not_execution_and_native_dialogs_do_not_claim_model_changes() {
        assert!(native_observation(Command::Compact, "❯", "❯ /compact\n❯").is_none());
        assert!(native_observation(Command::Compact, "›", "• Context compacted · 44s\n›").is_some());
        assert!(native_observation(Command::Model, "Set model to Sonnet\n❯", "Set model to Sonnet\n❯ /model").is_none());
        let opened = native_observation(Command::Model, "❯", "Select model\nOpus\nSonnet").unwrap();
        assert!(opened.1);
        assert!(opened.0.contains("chooser opened"));
        assert!(native_observation(Command::Model, "Select model\nOpus\nSonnet", "Select model\nOpus\nSonnet").is_none());
        let confirmed = native_observation(Command::Model, "Select model", "Set model to Sonnet\n❯").unwrap();
        assert!(!confirmed.1);
        assert_eq!(codex_model_selection("Select model", "• Model changed to gpt-6.1-sol xhigh for this conversation\n›"), Some(("gpt-6.1-sol".into(), Some("xhigh".into()))));
        for text in ["Select model\ngpt-6.1-sol", "› /model gpt-6.1-sol", "Model changed to gpt-6.1-sol xhigh", "• Failed to save default model: error"] {
            assert!(codex_model_selection("", text).is_none(), "{text}");
        }
        assert!(codex_model_selection("• Model changed to gpt-6.1-sol high", "• Model changed to gpt-6.1-sol high\n›").is_none());
    }
    #[tokio::test(flavor = "current_thread")]
    async fn native_clear_is_observed_when_codex_returns_to_its_welcome_prompt() {
        use super::super::supervisor_single_turn_tests::{fixture_child, fixture_for};
        if !fixture_child("ui::factory::daemon::runtime::supervisor_commands::tests::native_clear_is_observed_when_codex_returns_to_its_welcome_prompt") { return; }
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
        let observed = daemon.dispatch_supervisor_command(Request::Status, "cmd-clear".into(), owner).await.unwrap();
        assert!(matches!(observed, DaemonMessage::SupervisorCommandReceipt { receipt }
            if receipt.phase == Phase::Observed && receipt.detail.contains("welcome prompt")));
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
