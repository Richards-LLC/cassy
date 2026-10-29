//! Bounded, caller-owned supervisor controls. PTY submission never means execution.
use std::collections::HashMap;
use std::time::{Duration, Instant};
use chrono::{DateTime, Utc};
use crate::ui::factory::{MessageAttribution, DaemonMessage};
use crate::ui::factory::supervisor_commands::SupervisorCommandRequest as Request;
use crate::ui::factory::daemon::FactoryDaemon;
use crate::ui::factory::supervisor_commands::{SupervisorCommand as Command, SupervisorCommandKey as Key, SupervisorCommandPhase as Phase, SupervisorCommandReceipt as Receipt};

type Owner = (String, String);
#[derive(Default)]
pub(crate) struct CommandState { records: HashMap<Owner, Record> }
struct Record {
    receipt: Receipt,
    started: Instant,
    at: DateTime<Utc>,
    before: String,
    write: Box<dyn Fn() -> Option<Result<(), String>> + Send>,
    hook: Option<tokio::task::JoinHandle<Option<String>>>,
    last_key: Option<Key>,
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
        && text.lines().rev().take(12).any(|line| matches!(line.trim(), "❯" | "›"))
}

fn native_observation(command: Command, before: &str, current: &str) -> Option<(String, bool)> {
    if before == current { return None; }
    let lower = current.to_lowercase();
    let fresh = |marker: &str| lower.contains(marker) && !before.to_lowercase().contains(marker);
    match command {
        Command::Model if fresh("select model") || fresh("select a model")
            || fresh("select reasoning effort") => Some(("Model chooser opened in the supervisor's harness.".into(), true)),
        Command::Model if fresh("set model to") || fresh("model changed to") => Some(("The harness confirmed a model selection.".into(), false)),
        Command::Usage if fresh("current session") && (lower.contains("current week") || lower.contains("resets"))
            || command == Command::Usage && fresh("context window") && lower.contains("model:") =>
                Some(("Usage displayed by the supervisor's harness.".into(), !idle_prompt(current))),
        _ => None,
    }
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
            if let Some(record) = self.supervisor_commands.records.get(&owner) {
                anyhow::ensure!(record.receipt.command == *command, "Command reference already belongs to another command.");
                return Ok(DaemonMessage::SupervisorCommandReceipt { receipt: record.receipt.clone() });
            }
            anyhow::ensure!(self.supervisor_commands.records.len() < 256, "This session's command receipt limit was reached.");
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
            self.supervisor_commands.records.insert(owner, Record { receipt: receipt.clone(), started: Instant::now(), at: Utc::now(), before, write: Box::new(move || write.result()), hook: None, last_key: None });
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
            anyhow::ensure!(native_observation(record.receipt.command, "", &current).is_some_and(|(_, open)| open), "The native chooser is no longer open.");
            pane.deliver_user_input(key.bytes(), cas_mux::UserInputKind::KeyStream).await?;
            record.before = current;
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
            } else if let Some(Err(error)) = (record.write)() {
                record.receipt.phase = Phase::Unconfirmed;
                record.receipt.detail = format!("Command write failed; execution is uncertain: {error}. Do not resend automatically.");
            } else if let Some((detail, interactive)) = native_observation(record.receipt.command,
                if matches!(record.last_key, Some(Key::Next | Key::Previous | Key::Select)) && current != record.before { "" } else { &record.before }, &current) {
                record.receipt.phase = Phase::Observed;
                record.receipt.detail = detail;
                record.receipt.interactive = interactive;
                record.receipt.output = super::work_events::redact(&current).chars().take(8 * 1024).collect();
            } else if record.hook.as_ref().is_some_and(|job| job.is_finished()) {
                if let Some(Ok(Some(detail))) = record.hook.take().map(|job| futures_util::FutureExt::now_or_never(job)).flatten() {
                    record.receipt.phase = Phase::Observed;
                    record.receipt.detail = detail;
                }
            } else if record.hook.is_none() && matches!(record.receipt.command, Command::Compact | Command::Clear) {
                let root = self.app.cas_dir().to_path_buf();
                let session = self.session_name.clone(); let supervisor = self.app.supervisor_name().to_owned();
                let command = record.receipt.command; let after = record.at;
                record.hook = Some(tokio::task::spawn_blocking(move || hook_observation(&root, &session, &supervisor, command, after)));
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
    #[test]
    fn command_echo_is_not_execution_and_native_dialogs_do_not_claim_model_changes() {
        assert!(native_observation(Command::Compact, "❯", "❯ /compact\n❯").is_none());
        assert!(native_observation(Command::Model, "Set model to Sonnet\n❯", "Set model to Sonnet\n❯ /model").is_none());
        let opened = native_observation(Command::Model, "❯", "Select model\nOpus\nSonnet").unwrap();
        assert!(opened.1);
        assert!(opened.0.contains("chooser opened"));
        assert!(native_observation(Command::Model, "Select model\nOpus\nSonnet", "Select model\nOpus\nSonnet").is_none());
        let confirmed = native_observation(Command::Model, "Select model", "Set model to Sonnet\n❯").unwrap();
        assert!(!confirmed.1);
    }
    #[test]
    fn prompt_readiness_refuses_busy_and_modal_screens() {
        assert!(idle_prompt("A reply\n❯\n? for shortcuts"));
        assert!(!idle_prompt("Working · esc to interrupt\n❯"));
        assert!(!idle_prompt("Select model\n❯"));
        assert!(!idle_prompt("Still booting"));
    }
}
