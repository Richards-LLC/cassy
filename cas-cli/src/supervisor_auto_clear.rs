//! Clear a supervisor at an idle boundary under live context pressure.
//! GH #1665: cumulative cached re-reads are billing, not prompt occupancy.
//! The latest call's input is compared with 80% of the reported model window,
//! capped by `factory.supervisor_wake_input_budget`. Without a reported window,
//! only the configured per-call cap applies; zero disables automatic clearing.
//!
//! The steps run only when a turn has just ended, no newer turn has started,
//! and nothing is in flight (see [`idle_blockers`]). It is never mid-release.
//!
//! 1. Cassy saves a handoff memory from the session-end lane snapshot
//!    (cas-790e): owned epics and tasks, operator approvals from task decision
//!    notes, pending asks, and workers. It also writes the full JSON receipt.
//! 2. The supervisor gets one turn to save its judgement lines as a handoff.
//!    Cassy merges them under the facts.
//! 3. At the end of that turn, Cassy queues the existing context-reset control
//!    (cas-dffe) to the supervisor pane. Workers keep running. SessionStart
//!    injects the current handoff, and once the new transcript appears
//!    [`poll`] queues the handoff to the new session too.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use cas_store::{NotificationPriority, PromptQueueStore, QueueOrigin};
use cas_types::{Agent, Entry, EntryType};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const SOURCE: &str = "supervisor-auto-clear";
const ROLE: &str = "supervisor";
/// A turn read back from an old transcript is history, not a wake.
const FRESH_MINUTES: i64 = 30;
/// A clear that leaves no new transcript in this window is abandoned.
const LANDING_MINUTES: i64 = 15;
const SECTION_CHARS: usize = 1_400;
const RECEIPT_ROWS: usize = 100;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
enum Phase {
    /// Over budget, waiting for an idle boundary.
    Deferred,
    /// Handoff saved; the supervisor has one turn to add judgement.
    AwaitingJudgement { handoff_id: String, saved_at: DateTime<Utc>, notice_id: i64 },
    /// Reset control queued; waiting for the new transcript.
    ClearQueued {
        handoff_id: String,
        queued_at: DateTime<Utc>,
        dir: PathBuf,
        before: BTreeSet<PathBuf>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct State {
    agent_id: String,
    harness_session: String,
    trigger_tokens: u64,
    /// Old cumulative-budget state must not cause a clear after upgrading.
    #[serde(default)]
    latest_call: bool,
    budget: u64,
    triggered_at: DateTime<Utc>,
    #[serde(default)]
    blockers: Vec<String>,
    phase: Phase,
}

/// One newly recorded supervisor turn, as the scanner saw it.
pub(crate) struct TurnEnd<'a> {
    pub agent: &'a Agent,
    pub cli: cas_mux::SupervisorCli,
    pub factory_session: &'a str,
    pub harness_session: &'a str,
    pub transcript: &'a Path,
    pub at: DateTime<Utc>,
    /// A newer turn had already started when the scan stopped reading.
    pub mid_turn: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Step {
    Nothing,
    Deferred(Vec<String>),
    HandoffSaved(String),
    ClearQueued(i64),
    Resumed(String),
}

fn state_path(root: &Path, agent_id: &str) -> PathBuf {
    let safe: String = agent_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    root.join(SOURCE).join(format!("{safe}.json"))
}

fn load(root: &Path, agent_id: &str) -> anyhow::Result<Option<State>> {
    match std::fs::read(state_path(root, agent_id)) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn save(root: &Path, state: &State) -> anyhow::Result<()> {
    let path = state_path(root, &state.agent_id);
    let dir = path.parent().context("state path has no parent")?;
    std::fs::create_dir_all(dir)?;
    let temporary = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer_pretty(temporary.as_file(), state)?;
    temporary.persist(&path)?;
    Ok(())
}

fn clear_state(root: &Path, agent_id: &str) {
    let _ = std::fs::remove_file(state_path(root, agent_id));
}

fn tokens(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (index, c) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Why this is not an idle boundary. Empty means the clear may proceed.
pub(crate) fn idle_blockers(
    root: &Path,
    queue: &dyn PromptQueueStore,
    agent: &Agent,
    factory_session: &str,
) -> Vec<String> {
    use crate::mcp::tools::core::workflow::release_freeze;
    let mut blockers = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root.join("locks").join("delivery-target")) {
        for entry in entries.flatten() {
            let Ok(file) = std::fs::File::open(entry.path()) else { continue };
            if matches!(fs2::FileExt::try_lock_exclusive(&file), Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock) {
                blockers.push("a worktree_merge holds a delivery-target lock".to_string());
                break;
            }
            let _ = fs2::FileExt::unlock(&file);
        }
    }
    match crate::store::open_task_store(root).and_then(|store| Ok(store.list(None)?)) {
        Ok(tasks) => {
            for task in &tasks {
                if crate::train_guardrails::repair_cycle_open(task, &tasks) {
                    blockers.push(format!("release train {} has a repair cycle open", task.id));
                }
                if release_freeze::freeze_of(task).is_some()
                    && release_freeze::active(root, task).is_some()
                {
                    blockers.push(format!("release train {} is frozen for a cut", task.id));
                }
                if !task.is_terminal()
                    && task.labels.iter().any(|label| label == release_freeze::REPAIR_LABEL)
                {
                    blockers.push(format!("release repair {} is open", task.id));
                }
            }
        }
        Err(error) => blockers.push(format!("tasks unreadable: {error}")),
    }
    match crate::store::open_reminder_store(root).and_then(|store| Ok(store.list_pending(&agent.id)?)) {
        Ok(reminders) => {
            for reminder in reminders {
                if let Some(ask) = reminder.trigger_filter.as_ref().and_then(|f| f.get("operator_ask")) {
                    blockers.push(format!("operator ask #{ask} is unanswered"));
                }
            }
        }
        Err(error) => blockers.push(format!("reminders unreadable: {error}")),
    }
    match queue.peek_for_targets(&["supervisor", &agent.name], Some(factory_session), 50) {
        Ok(rows) => {
            if rows.iter().any(|row| matches!(row.origin, Some(QueueOrigin::PairedDevice { .. }))) {
                blockers.push("an operator message is waiting for the supervisor".to_string());
            }
        }
        Err(error) => blockers.push(format!("queue unreadable: {error}")),
    }
    blockers
}

fn clip(text: &str, limit: usize) -> String {
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = clean.chars().filter(|c| !c.is_control()).take(limit).collect();
    if clean.chars().count() > limit {
        out.push_str("...");
    }
    out
}

fn section(out: &mut String, label: &str, lines: &[String]) {
    out.push_str(&format!("{label} ({}):\n", lines.len()));
    let mut used = 0;
    for (index, line) in lines.iter().enumerate() {
        if used + line.len() > SECTION_CHARS {
            out.push_str(&format!("- {} more in the receipt\n", lines.len() - index));
            break;
        }
        used += line.len();
        out.push_str(line);
    }
    if lines.is_empty() {
        out.push_str("- none\n");
    }
}

/// Keep auto-clear receipts bounded without changing retirement/recovery evidence.
fn compact_snapshot(snapshot: &mut cas_store::SupervisorLaneSnapshot, now: DateTime<Utc>) {
    let dead_owners: BTreeSet<String> = snapshot.workers.iter()
        .filter(|worker| !matches!(worker["status"].as_str(), Some("active" | "idle")))
        .flat_map(|worker| ["id", "name"].into_iter().filter_map(|key| worker[key].as_str().map(str::to_owned)))
        .collect();
    snapshot.workers.retain(|worker| matches!(worker["status"].as_str(), Some("active" | "idle")));
    snapshot.tasks.retain(|task| match task["status"].as_str() {
        Some("awaiting_merge") => true, // A parked delivery survives its worker.
        Some("in_progress") => !task["assignee"].as_str().is_some_and(|owner| dead_owners.contains(owner)),
        _ => false,
    });
    for task in &mut snapshot.tasks {
        let notes = task["notes"].as_str().unwrap_or_default().lines()
            .filter(|line| line.contains("DECISION") && line.to_ascii_lowercase().contains("operator"))
            .map(|line| clip(line, 320)).collect::<Vec<_>>();
        let notes = notes.iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join("\n");
        task["notes"] = Value::String(notes);
        for (key, limit) in [("title", 90), ("external_ref", 180)] {
            if let Some(text) = task[key].as_str() { task[key] = Value::String(clip(text, limit)); }
        }
        // Long proof histories remain in the task store, not each new context.
        let mut refs = serde_json::Map::new();
        for key in ["parked_branch", "parked_head", "pr_url", "target_branch"] {
            if let Some(text) = task["deliverables"][key].as_str() {
                refs.insert(key.into(), Value::String(clip(text, 180)));
            }
        }
        task["deliverables"] = Value::Object(refs);
    }
    for row in &mut snapshot.pending {
        let payload: Value = row["payload"].as_str()
            .and_then(|text| serde_json::from_str(text).ok()).unwrap_or(Value::Null);
        let filter: Value = row["trigger_filter"].as_str()
            .and_then(|text| serde_json::from_str(text).ok()).unwrap_or(Value::Null);
        let actor = ["actor", "agent_name", "worker_name", "source"].into_iter()
            .find_map(|key| payload[key].as_str()).or_else(|| row["source"].as_str()).unwrap_or("supervisor");
        let kind = row["kind"].as_str().or_else(|| row["event_type"].as_str())
            .or_else(|| row["trigger_type"].as_str()).unwrap_or("message");
        let task = payload["task_id"].as_str().or_else(|| row["task_id"].as_str())
            .or_else(|| filter["task_id"].as_str()).unwrap_or("none");
        let created = row["created_at"].as_str().and_then(|raw| {
            DateTime::parse_from_rfc3339(raw).map(|at| at.with_timezone(&Utc)).ok()
                .or_else(|| chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S").ok().map(|at| at.and_utc()))
        });
        let age_minutes = created.map(|at| (now - at).num_minutes().max(0));
        let summary = [payload.get("message"), row.get("prompt"), row.get("message")].into_iter()
            .flatten().find_map(Value::as_str).filter(|text| !text.trim_start().starts_with(['{', '[']))
            .map(|text| clip(text, 100)).unwrap_or_default();
        *row = serde_json::json!({"id":row["id"], "actor":clip(actor, 60), "kind":clip(kind, 60),
            "task_id":clip(task, 80), "age_minutes":age_minutes, "summary":summary});
    }
    let omitted = [snapshot.tasks.len().saturating_sub(RECEIPT_ROWS),
        snapshot.workers.len().saturating_sub(RECEIPT_ROWS), snapshot.pending.len().saturating_sub(RECEIPT_ROWS)];
    snapshot.tasks.truncate(RECEIPT_ROWS);
    snapshot.workers.truncate(RECEIPT_ROWS);
    snapshot.pending.truncate(RECEIPT_ROWS);
    if omitted.iter().any(|count| *count > 0) {
        snapshot.reason.push_str(&format!("; additional tasks/workers/pending in stores: {}/{}/{}", omitted[0], omitted[1], omitted[2]));
    }
}

/// Render the lane snapshot. Approvals come first so a truncated injection
/// keeps them.
fn render(snapshot: &cas_store::SupervisorLaneSnapshot, state: &State, receipt: &Path) -> String {
    let field = |row: &Value, key: &str| row[key].as_str().unwrap_or_default().to_string();
    let mut approvals = Vec::new();
    for task in &snapshot.tasks {
        for line in field(task, "notes").lines() {
            if line.contains("DECISION") && line.to_ascii_lowercase().contains("operator") {
                approvals.push(format!("- {}: {}\n", field(task, "id"), clip(line, 320)));
            }
        }
    }
    let pending: Vec<String> = snapshot
        .pending
        .iter()
        .map(|row| {
            let age = row["age_minutes"].as_i64().map(|age| format!("{age}m")).unwrap_or_else(|| "unknown".into());
            format!("- #{} {}: {}; task {}; age {}; {}\n", row["id"],
                field(row, "actor"), field(row, "kind"), field(row, "task_id"), age, field(row, "summary"))
        })
        .collect();
    let tasks: Vec<String> = snapshot
        .tasks
        .iter()
        .map(|task| {
            let mut line = format!("- {} [{}] {}", field(task, "id"), field(task, "status"), clip(&field(task, "title"), 90));
            if !field(task, "assignee").is_empty() {
                line.push_str(&format!("; assignee {}", field(task, "assignee")));
            }
            if let Some(branch) = task["deliverables"]["parked_branch"].as_str() {
                line.push_str(&format!("; parked {branch}"));
            }
            line.push('\n');
            line
        })
        .collect();
    let workers: Vec<String> = snapshot
        .workers
        .iter()
        .map(|worker| format!("- {}: {}\n", field(worker, "name"), field(worker, "status")))
        .collect();
    let mut out = format!(
        "Automatic context clear: latest call at {} used {} input tokens; the context threshold is {}. \
         Cassy wrote these facts. Your own judgement lines follow them when you saved any.\n\
         Snapshot: {}\nHandoff receipt: {}\n",
        crate::cli::release_report::eastern_time(state.triggered_at),
        tokens(state.trigger_tokens),
        tokens(state.budget),
        snapshot.reason,
        receipt.display(),
    );
    section(&mut out, "Operator approvals (task decision notes)", &approvals);
    section(&mut out, "Pending operator decisions, peer asks and reminders", &pending);
    section(&mut out, "Owned epics and tasks", &tasks);
    section(&mut out, "Workers (still running; do not respawn them)", &workers);
    out
}

fn add_handoff(root: &Path, title: &str, content: String) -> anyhow::Result<Entry> {
    use cas_core::memory::handoff;
    let store = crate::store::open_store(root)?;
    let mut entry = Entry::new(store.generate_id()?, content);
    entry.entry_type = EntryType::Context;
    entry.title = Some(title.to_string());
    entry.tags = vec![handoff::HANDOFF_TAG.into(), format!("{}{ROLE}", handoff::ROLE_TAG_PREFIX), SOURCE.into()];
    store.add(&entry)?;
    let now = Utc::now();
    for previous in handoff::handoffs_superseded_by(&store.list()?, &entry.id, ROLE) {
        let mut previous = previous.clone();
        handoff::mark_superseded(&mut previous, &entry.id, now);
        store.update(&previous)?;
    }
    Ok(entry)
}

fn save_handoff(root: &Path, agent: &Agent, state: &State) -> anyhow::Result<String> {
    let mut snapshot = cas_store::SqliteAgentStore::open(root)?
        .supervisor_lane_snapshot(&agent.id, "automatic context clear")?
        .context("supervisor row missing")?;
    compact_snapshot(&mut snapshot, Utc::now());
    let dir = root.join("supervisor-handoffs");
    std::fs::create_dir_all(&dir)?;
    let receipt = dir.join(format!("auto-clear-{}-{}.json", agent.name, state.triggered_at.timestamp()));
    std::fs::write(&receipt, serde_json::to_vec_pretty(&snapshot)?)?;
    let title = format!(
        "Supervisor handoff before automatic context clear ({}, {})",
        agent.name,
        crate::cli::release_report::eastern_time(state.triggered_at)
    );
    Ok(add_handoff(root, &title, render(&snapshot, state, &receipt))?.id)
}

/// Merge a handoff the supervisor saved after Cassy's facts under those facts.
fn merge_judgement(root: &Path, handoff_id: &str, saved_at: DateTime<Utc>) -> anyhow::Result<String> {
    use cas_core::memory::handoff;
    let store = crate::store::open_store(root)?;
    let entries = store.list()?;
    let Some(current) = handoff::current_handoff(&entries, ROLE).filter(|c| c.id != handoff_id && c.created >= saved_at) else {
        return Ok(handoff_id.to_string());
    };
    let facts = entries.iter().find(|e| e.id == handoff_id).map(|e| e.content.clone()).unwrap_or_default();
    let title = current.title.clone().unwrap_or_else(|| "Supervisor handoff before automatic context clear".into());
    let content = format!("{facts}\nSupervisor judgement (saved before the clear):\n{}\n", clip_block(current.content.trim(), SECTION_CHARS));
    Ok(add_handoff(root, &title, content)?.id)
}

/// Advance the state machine for one newly recorded supervisor turn.
pub(crate) fn on_turn_end(root: &Path, queue: &dyn PromptQueueStore, turn: &TurnEnd<'_>) -> anyhow::Result<Step> {
    let mut state = load(root, &turn.agent.id)?;
    if state.as_ref().is_some_and(|s| {
        (!s.latest_call || s.harness_session != turn.harness_session)
            && !matches!(s.phase, Phase::ClearQueued { .. })
    }) {
        // A different session or a legacy cumulative trigger cannot clear this one.
        clear_state(root, &turn.agent.id);
        state = None;
    }
    let mut state = match state {
        Some(state) => state,
        None => {
            let cap = crate::config::Config::load(root)?.factory().supervisor_wake_input_budget;
            if cap == 0 { return Ok(Step::Nothing); }
            let Some(usage) = crate::mcp::tools::service::factory_ops::read_context_usage_from_tail_for_cli(
                turn.transcript, turn.cli,
            ) else { return Ok(Step::Nothing) };
            let budget = usage.model_context_window.filter(|window| *window > 0)
                .map(|window| (window.saturating_mul(80) / 100).min(cap)).unwrap_or(cap);
            let trigger_tokens = usage.input_tokens;
            if trigger_tokens < budget { return Ok(Step::Nothing); }
            if Utc::now() - turn.at > Duration::minutes(FRESH_MINUTES)
                || crate::factory_context_reset::context_reset_command(turn.cli).is_none()
            {
                return Ok(Step::Nothing);
            }
            State {
                agent_id: turn.agent.id.clone(),
                harness_session: turn.harness_session.to_string(),
                trigger_tokens,
                latest_call: true,
                budget,
                triggered_at: turn.at,
                blockers: Vec::new(),
                phase: Phase::Deferred,
            }
        }
    };
    if let Phase::AwaitingJudgement { notice_id, .. } = &state.phase {
        // The judgement turn is the first one to end after the notice arrived.
        let delivered = queue
            .queued_prompt(*notice_id)?
            .and_then(|row| row.processed_at)
            .is_some_and(|at| at <= turn.at);
        if !delivered {
            return Ok(Step::Nothing);
        }
    }
    if matches!(state.phase, Phase::ClearQueued { .. }) {
        return Ok(Step::Nothing);
    }
    let mut blockers = idle_blockers(root, queue, turn.agent, turn.factory_session);
    if turn.mid_turn {
        blockers.insert(0, "a newer supervisor turn is running".to_string());
    }
    if !blockers.is_empty() {
        if state.blockers != blockers {
            tracing::info!(target: "cas::coordination", stage = "supervisor_auto_clear_deferred",
                agent = %turn.agent.name, blockers = %blockers.join("; "), "automatic context clear waits for an idle boundary");
        }
        state.blockers = blockers.clone();
        save(root, &state)?;
        return Ok(Step::Deferred(blockers));
    }
    state.blockers.clear();
    match state.phase.clone() {
        Phase::Deferred => {
            let handoff_id = save_handoff(root, turn.agent, &state)?;
            let notice = format!(
                "Context budget: your latest call at {} used {} input tokens; the context threshold is {}. Cassy saved your handoff as {handoff_id} \
                 from facts (owned tasks, operator approvals from decision notes, pending asks, workers). In this turn, add only \
                 judgement Cassy can't know (what you were about to do, promises, risks) with `memory action=remember \
                 entry_type=handoff content=\"<your lines>\"`; Cassy merges it under the facts. Start no new work. When this \
                 turn ends and nothing is in flight, Cassy clears your context. Workers keep running, and the handoff is \
                 injected into your new session.",
                crate::cli::release_report::eastern_time(state.triggered_at),
                tokens(state.trigger_tokens),
                tokens(state.budget),
            );
            let notice_id = queue.enqueue_idempotent(
                SOURCE, &turn.agent.name, &notice, Some(turn.factory_session), Some("Context budget: save judgement, then Cassy clears"),
                Some(NotificationPriority::High), &format!("{SOURCE}:notice:{handoff_id}"), Some(&QueueOrigin::Daemon),
            )?;
            let notice_id = match notice_id {
                cas_store::EnqueueIdempotentResult::Created(id) | cas_store::EnqueueIdempotentResult::AlreadyExists(id) => id,
            };
            state.phase = Phase::AwaitingJudgement { handoff_id: handoff_id.clone(), saved_at: Utc::now(), notice_id };
            save(root, &state)?;
            Ok(Step::HandoffSaved(handoff_id))
        }
        Phase::AwaitingJudgement { handoff_id, saved_at, .. } => {
            let handoff_id = merge_judgement(root, &handoff_id, saved_at)?;
            let dir = turn.transcript.parent().context("transcript has no directory")?.to_path_buf();
            let before = crate::factory_context_reset::snapshot_transcripts(std::slice::from_ref(&dir));
            let control_id = queue
                .enqueue_urgent_with_outcome(
                    SOURCE,
                    &turn.agent.name,
                    crate::factory_context_reset::CONTEXT_RESET_CONTROL,
                    Some(turn.factory_session),
                    Some(crate::factory_context_reset::CONTEXT_RESET_SUMMARY),
                    Some(NotificationPriority::Critical),
                    true,
                    Some(&QueueOrigin::Daemon),
                )?
                .id();
            state.phase = Phase::ClearQueued { handoff_id, queued_at: Utc::now(), dir, before };
            save(root, &state)?;
            tracing::info!(target: "cas::coordination", stage = "supervisor_auto_clear_queued",
                agent = %turn.agent.name, control_id, trigger_tokens = state.trigger_tokens, "automatic supervisor context clear queued");
            Ok(Step::ClearQueued(control_id))
        }
        Phase::ClearQueued { .. } => Ok(Step::Nothing),
    }
}

/// Confirm a queued clear from the new transcript, then hand the new session
/// its handoff. Called on every scan; cheap when no clear is queued.
pub(crate) fn poll(root: &Path, queue: &dyn PromptQueueStore, agent: &Agent, factory_session: &str) -> anyhow::Result<Step> {
    let Some(state) = load(root, &agent.id)? else { return Ok(Step::Nothing) };
    let Phase::ClearQueued { handoff_id, queued_at, dir, before } = &state.phase else {
        return Ok(Step::Nothing);
    };
    let Some(evidence) = crate::factory_context_reset::detect_context_reset(std::slice::from_ref(dir), before) else {
        if Utc::now() - *queued_at > Duration::minutes(LANDING_MINUTES) {
            tracing::warn!(target: "cas::coordination", stage = "supervisor_auto_clear_unconfirmed",
                agent = %agent.name, "no new supervisor transcript recorded the automatic /clear; giving up");
            clear_state(root, &agent.id);
        }
        return Ok(Step::Nothing);
    };
    let body = crate::store::open_store(root)?.get(handoff_id).map(|e| e.content).unwrap_or_default();
    let brief = format!(
        "Cassy cleared your context at an idle boundary: the latest call used {} input tokens (context threshold {}). Workers kept running. \
         Your handoff is memory {handoff_id}; it is below and was injected at session start.\n\n{}",
        tokens(state.trigger_tokens),
        tokens(state.budget),
        clip_block(&body, 8_000),
    );
    queue.enqueue_idempotent(
        SOURCE, &agent.name, &brief, Some(factory_session), Some("Context cleared: resume from your handoff"),
        Some(NotificationPriority::High), &format!("{SOURCE}:resume:{handoff_id}"), Some(&QueueOrigin::Daemon),
    )?;
    clear_state(root, &agent.id);
    Ok(Step::Resumed(evidence.session_id))
}

fn clip_block(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n... (read the rest with `memory action=get`)", &text[..end])
}
