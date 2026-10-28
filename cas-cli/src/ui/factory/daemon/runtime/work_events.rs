//! Commander work events (cas-8f88): a paged, typed stream of what a factory
//! session is doing. The supervisor's thinking and tool calls come from its
//! own Claude or Codex transcript, task transitions and worker notices from
//! the supervisor queue, and worker commits and pushes from hook events.
//!
//! Zero model tokens: nothing here calls a model or writes to an agent. The
//! transcript is opened read-only, the stores are only queried, and no
//! credential file is read. Pages carry short labels; a tool's input and
//! result or a thinking block's full text are read on demand, and both are
//! redacted before they leave the daemon. Schema: `docs/guides/commander-work-events.md`.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cas_store::{SqliteEventStore, SqliteSupervisorQueueStore, SupervisorQueueStore};
use cas_types::EventType;
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::jobs::{Criticality, JobSpec, Lane};
use super::prompt_queue_jobs::{ExternalOutcome, QueueJobs};
use crate::ui::factory::{
    DaemonMessage, WORK_EVENT_PAGE_SIZE, WORK_EVENT_SCHEMA_VERSION, WorkChangedFile,
    WorkChangedFiles, WorkEvent, WorkEventKind, WorkLifecycle, WorkReasoning, WorkTool,
    WorkToolCounts,
};

/// How much of a transcript's tail is indexed when first read. Older turns
/// are not served (`has_earlier` ends there); the index then grows forward.
pub(crate) const TRANSCRIPT_WINDOW_BYTES: u64 = 64 * 1024 * 1024;
const LABEL_CHARS: usize = 120;
/// Largest detail sent; a longer one is cut and marked `truncated`.
pub(crate) const DETAIL_CHARS: usize = 16 * 1024;
/// Extra rows read past a page so rows sharing the cursor's millisecond are
/// not lost to the SQL limit.
const BOUNDARY_SLACK: usize = 32;
/// Hook events that belong on a worker's timeline. Heartbeats, file edits and
/// injections are telemetry, not work.
const HOOK_EVENT_TYPES: &[EventType] = &[
    EventType::WorkerGitCommit,
    EventType::WorkerPushBlocked,
    EventType::WorkerVerificationBlocked,
    EventType::WorkerSubagentSpawned,
    EventType::WorkerSubagentCompleted,
];
const JOB_KIND: &str = "WorkEvents";
const JOB_BUDGET: Duration = Duration::from_secs(2);

/// Redact with Cassy's existing passes: secret assignments, bearer values
/// and PEM blocks by line, then secret-shaped tokens by word.
pub(crate) fn redact(text: &str) -> String {
    let lines = crate::ai_enrichment::redact_string(text);
    crate::hooks::handlers::handlers_events::message_display::redact_secrets(lines).0
}

fn bounded(text: &str, limit: usize) -> (String, bool) {
    if text.chars().count() <= limit {
        return (text.to_owned(), false);
    }
    let mut cut: String = text.chars().take(limit.saturating_sub(1)).collect();
    cut.push('…');
    (cut, true)
}

/// First non-empty line, redacted and bounded.
fn label(text: &str) -> String {
    let line = text
        .lines()
        .map(|line| line.trim().trim_matches('*').trim())
        .find(|line| !line.is_empty())
        .unwrap_or("");
    bounded(&redact(line), LABEL_CHARS).0
}

/// Page position: events sort by time, then id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SortKey {
    at_ms: i64,
    id: String,
}

impl SortKey {
    fn encode(&self) -> String {
        format!("{}:{}", self.at_ms, self.id)
    }

    fn decode(cursor: &str) -> Option<Self> {
        let (at, id) = cursor.split_once(':')?;
        Some(Self {
            at_ms: at.parse().ok()?,
            id: id.to_owned(),
        })
    }
}

fn keyed(event: WorkEvent, at: DateTime<Utc>) -> (SortKey, WorkEvent) {
    (
        SortKey {
            at_ms: at.timestamp_millis(),
            id: event.id.clone(),
        },
        event,
    )
}

fn timestamp(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.get("timestamp")?.as_str()?)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// One supervisor of the session, as registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SupervisorSource {
    pub agent_id: String,
    pub agent_name: String,
    pub harness_session: String,
    pub cli: cas_mux::SupervisorCli,
    pub account_dir: Option<String>,
}

/// What one session's stream reads.
#[derive(Debug, Clone)]
pub(crate) struct WorkEventScope {
    pub session: String,
    pub supervisors: Vec<SupervisorSource>,
    /// The session's workers, read only for one worker's own timeline
    /// (cas-ff22); `SupervisorSource` describes any agent's transcript.
    pub workers: Vec<SupervisorSource>,
    /// Every agent of the session: id -> name.
    pub agents: HashMap<String, String>,
    /// No hook event of the session is older than this.
    pub floor: DateTime<Utc>,
}

impl WorkEventScope {
    /// The session's agents from the agent store.
    pub(crate) fn from_agents(session: &str, agents: &[cas_types::Agent]) -> Self {
        let members: Vec<&cas_types::Agent> = agents
            .iter()
            .filter(|agent| agent.factory_session.as_deref() == Some(session))
            .collect();
        let sources = |role: cas_types::AgentRole, prefix: &str| -> Vec<SupervisorSource> {
            members
                .iter()
                .filter(|agent| agent.role == role)
                .map(|agent| SupervisorSource {
                    agent_id: agent.id.clone(),
                    agent_name: agent.name.clone(),
                    harness_session: agent.cc_session_id.clone().unwrap_or_else(|| agent.id.clone()),
                    cli: agent
                        .metadata
                        .get(&format!("{prefix}_cli"))
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(cas_mux::SupervisorCli::Claude),
                    account_dir: agent.metadata.get(&format!("{prefix}_account_dir")).cloned(),
                })
                .collect()
        };
        let supervisors = sources(cas_types::AgentRole::Supervisor, "supervisor");
        let workers = sources(cas_types::AgentRole::Worker, "worker");
        let floor = members
            .iter()
            .map(|agent| agent.registered_at)
            .min()
            .unwrap_or_else(Utc::now)
            - chrono::Duration::hours(1);
        Self {
            session: session.to_owned(),
            supervisors,
            workers,
            agents: members
                .iter()
                .map(|agent| (agent.id.clone(), agent.name.clone()))
                .collect(),
            floor,
        }
    }

    /// The transcripts one page reads: the supervisors' for the session
    /// stream, or the named agent's own for its timeline.
    fn transcripts(&self, agent: Option<&str>) -> Vec<&SupervisorSource> {
        match agent {
            None => self.supervisors.iter().collect(),
            Some(name) => self.members().filter(|source| source.agent_name == name).collect(),
        }
    }

    fn members(&self) -> impl Iterator<Item = &SupervisorSource> {
        self.supervisors.iter().chain(&self.workers)
    }
}

struct Indexed {
    key: SortKey,
    event: WorkEvent,
    line: u64,
    block: usize,
    result_line: Option<u64>,
}

/// Forward, incremental index of one transcript's work events.
struct TranscriptIndex {
    source: SupervisorSource,
    session: String,
    path: PathBuf,
    /// Byte offset after the last complete line indexed.
    end: Option<u64>,
    events: Vec<Indexed>,
    open_calls: HashMap<String, usize>,
    turn: Option<String>,
    command_sessions: HashMap<String, usize>,
    poll_parents: HashMap<String, usize>,
    changed_files: HashMap<Option<String>, Vec<WorkChangedFile>>,
}

impl TranscriptIndex {
    fn new(source: SupervisorSource, session: &str, path: PathBuf) -> Self {
        Self {
            source,
            session: session.to_owned(),
            path,
            end: None,
            events: Vec::new(),
            open_calls: HashMap::new(),
            turn: None,
            command_sessions: HashMap::new(),
            poll_parents: HashMap::new(),
            changed_files: HashMap::new(),
        }
    }

    fn refresh(&mut self) -> std::io::Result<()> {
        let mut file = std::fs::File::open(&self.path)?;
        let len = file.metadata()?.len();
        if self.end.is_some_and(|end| len < end) {
            // Truncated or replaced: start over.
            *self = Self::new(
                self.source.clone(),
                &self.session.clone(),
                self.path.clone(),
            );
        }
        let mut position = match self.end {
            Some(end) if end == len => return Ok(()),
            Some(end) => {
                file.seek(SeekFrom::Start(end))?;
                end
            }
            None => {
                let start = len.saturating_sub(TRANSCRIPT_WINDOW_BYTES);
                if start == 0 {
                    0
                } else {
                    // Begin at the first line that starts inside the window.
                    file.seek(SeekFrom::Start(start - 1))?;
                    let mut partial = Vec::new();
                    let skipped = BufReader::new(&mut file).read_until(b'\n', &mut partial)?;
                    let position = start - 1 + skipped as u64;
                    file.seek(SeekFrom::Start(position))?;
                    position
                }
            }
        };
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = reader.read_until(b'\n', &mut line)?;
            if read == 0 || line.last() != Some(&b'\n') {
                break;
            }
            self.index_line(position, &line);
            position += read as u64;
        }
        self.end = Some(position);
        Ok(())
    }

    fn push(
        &mut self,
        at: DateTime<Utc>,
        line: u64,
        block: usize,
        kind: WorkEventKind,
        text: String,
        call: Option<&str>,
    ) {
        let event = WorkEvent {
            id: format!("t:{}:{line:015}:{block:03}", self.source.agent_id),
            kind,
            session: self.session.clone(),
            agent: Some(self.source.agent_name.clone()),
            turn: self.turn.clone(),
            at: at.to_rfc3339(),
            label: label(&text),
            has_detail: true,
            error: false,
            tool: None,
            lifecycle: None,
            reasoning: None,
            changed_files: None,
        };
        let (key, event) = keyed(event, at);
        if let Some(call) = call {
            self.open_calls.insert(call.to_owned(), self.events.len());
        }
        self.events.push(Indexed {
            key,
            event,
            line,
            block,
            result_line: None,
        });
    }

    fn annotate_tool(&mut self, name: &str, input: &Value, call: Option<&str>) {
        let parent = if name.rsplit('.').next() == Some("write_stdin") {
            input
                .get("session_id")
                .and_then(session_key)
                .and_then(|id| self.command_sessions.get(&id).copied())
        } else {
            None
        };
        let Some(indexed) = self.events.last_mut() else {
            return;
        };
        indexed.event.tool = Some(tool_payload(name, input));
        indexed.event.lifecycle = Some(WorkLifecycle {
            id: call
                .map(str::to_owned)
                .unwrap_or_else(|| indexed.event.id.clone()),
            stage: "start".into(),
            phase: None,
            outcome: None,
            started_at: Some(indexed.event.at.clone()),
            ended_at: None,
        });
        if let (Some(call), Some(parent)) = (call, parent) {
            self.poll_parents.insert(call.to_owned(), parent);
        }
    }

    fn annotate_reasoning(&mut self, source: &str) {
        if let Some(indexed) = self.events.last_mut() {
            indexed.event.reasoning = Some(WorkReasoning {
                summary: indexed.event.label.clone(),
                source: source.into(),
            });
        }
    }

    fn settle(
        &mut self,
        call: &str,
        line: u64,
        at: DateTime<Utc>,
        error: bool,
        result: &Value,
        known_success: bool,
    ) {
        let Some(index) = self.open_calls.remove(call) else {
            return;
        };
        let (exit_code, session) = result_facts(result, 0);
        let parent = self.poll_parents.remove(call);
        let running = session.is_some() && exit_code.is_none() && !error;
        let failed = error
            || result
                .get("isError")
                .or_else(|| result.get("is_error"))
                .and_then(Value::as_bool)
                == Some(true)
            || exit_code.is_some_and(|code| code != 0);
        for target in std::iter::once(index).chain(parent) {
            let indexed = &mut self.events[target];
            indexed.result_line = Some(line);
            indexed.event.error = failed;
            if let Some(tool) = &mut indexed.event.tool {
                tool.exit_code = exit_code;
                let files = result.get("numFiles").and_then(Value::as_u64);
                let matches = result.get("numMatches").and_then(Value::as_u64);
                if files.is_some() || matches.is_some() {
                    tool.counts = Some(WorkToolCounts { files, matches });
                }
            }
            if let Some(lifecycle) = &mut indexed.event.lifecycle {
                lifecycle.stage = if running { "phase" } else { "end" }.into();
                lifecycle.phase = running.then(|| "running".into());
                lifecycle.ended_at = (!running).then(|| at.to_rfc3339());
                lifecycle.outcome = (!running).then(|| {
                    if failed {
                        "error"
                    } else if known_success || exit_code == Some(0) {
                        "success"
                    } else {
                        "unknown"
                    }
                    .into()
                });
            }
            if let Some(session) = &session {
                self.command_sessions
                    .insert(session.clone(), parent.unwrap_or(index));
            }
            if !running
                && !failed
                && (known_success || exit_code == Some(0))
                && indexed.event.turn.is_some()
                && let Some(tool) = &indexed.event.tool
                && tool.operation == "edit"
            {
                let files = self
                    .changed_files
                    .entry(indexed.event.turn.clone())
                    .or_default();
                for path in &tool.targets {
                    let file = WorkChangedFile {
                        path: path.clone(),
                        operation: "edit".into(),
                        source_event_id: indexed.event.id.clone(),
                    };
                    if let Some(existing) = files.iter_mut().find(|file| &file.path == path) {
                        *existing = file;
                    } else if files.len() < 128 {
                        files.push(file);
                    }
                }
                if !files.is_empty() {
                    indexed.event.changed_files = Some(WorkChangedFiles {
                        files: files.clone(),
                        provenance: "successful_tool_calls".into(),
                        complete: false,
                    });
                }
            }
        }
        let turn = self.events[index].event.turn.clone();
        if let Some(files) = self.changed_files.get(&turn) {
            for indexed in &mut self.events {
                if indexed.event.turn == turn
                    && let Some(summary) = &mut indexed.event.changed_files
                {
                    summary.files = files.clone();
                }
            }
        }
        if !running && let Some(parent) = parent {
            self.command_sessions.retain(|_, index| *index != parent);
        }
    }

    fn lifecycle_notice(
        &mut self,
        at: DateTime<Utc>,
        offset: u64,
        stage: &str,
        phase: Option<&str>,
        outcome: Option<&str>,
    ) {
        let text = phase.unwrap_or(if stage == "end" {
            "Turn completed"
        } else {
            "Turn started"
        });
        self.push(at, offset, 0, WorkEventKind::Notice, text.into(), None);
        let indexed = self.events.last_mut().unwrap();
        indexed.event.has_detail = false;
        indexed.event.lifecycle = Some(WorkLifecycle {
            id: self
                .turn
                .clone()
                .unwrap_or_else(|| indexed.event.id.clone()),
            stage: stage.into(),
            phase: phase.map(str::to_owned),
            outcome: outcome.map(str::to_owned),
            started_at: (stage == "start").then(|| at.to_rfc3339()),
            ended_at: (stage == "end").then(|| at.to_rfc3339()),
        });
    }

    fn index_line(&mut self, offset: u64, bytes: &[u8]) {
        match self.source.cli {
            cas_mux::SupervisorCli::Codex => self.index_codex(offset, bytes),
            _ => self.index_claude(offset, bytes),
        }
    }

    fn index_claude(&mut self, offset: u64, bytes: &[u8]) {
        // Most lines are request copies and UI state; skip them unparsed.
        if bytes.starts_with(b"{\"type\":\"")
            && !bytes.starts_with(b"{\"type\":\"assistant\"")
            && !bytes.starts_with(b"{\"type\":\"user\"")
            && !bytes.starts_with(b"{\"type\":\"system\"")
        {
            return;
        }
        let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
            return;
        };
        if value.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return;
        }
        let Some(at) = timestamp(&value) else { return };
        let content = value.pointer("/message/content");
        match value.get("type").and_then(Value::as_str) {
            Some("user") => {
                let blocks = content.and_then(Value::as_array);
                let results: Vec<(&str, bool, &Value)> = blocks
                    .into_iter()
                    .flatten()
                    .filter(|block| {
                        block.get("type").and_then(Value::as_str) == Some("tool_result")
                    })
                    .filter_map(|block| {
                        Some((
                            block.get("tool_use_id")?.as_str()?,
                            block.get("is_error").and_then(Value::as_bool) == Some(true),
                            block,
                        ))
                    })
                    .collect();
                for (call, error, block) in &results {
                    let result = if results.len() == 1 {
                        value.get("toolUseResult").unwrap_or(block)
                    } else {
                        block
                    };
                    self.settle(call, offset, at, *error, result, true);
                }
                if results.is_empty() && value.get("isMeta").and_then(Value::as_bool) != Some(true)
                {
                    self.turn = value
                        .get("promptId")
                        .or_else(|| value.get("uuid"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or(self.turn.take());
                }
            }
            Some("assistant") => {
                for (block, item) in content
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    match item.get("type").and_then(Value::as_str) {
                        Some("thinking") => {
                            let text = item.get("thinking").and_then(Value::as_str).unwrap_or("");
                            // Claude Code records most thinking as an empty,
                            // signed block: nothing to show.
                            if !text.trim().is_empty() {
                                self.push(
                                    at,
                                    offset,
                                    block,
                                    WorkEventKind::Thinking,
                                    text.to_owned(),
                                    None,
                                );
                                self.annotate_reasoning("claude_thinking");
                            }
                        }
                        Some("tool_use") => {
                            let name = item.get("name").and_then(Value::as_str).unwrap_or("tool");
                            let input = item.get("input").cloned().unwrap_or(Value::Null);
                            let call = item.get("id").and_then(Value::as_str);
                            let (kind, text) = tool_summary(name, &input);
                            self.push(at, offset, block, kind, text, call);
                            self.annotate_tool(name, &input, call);
                        }
                        _ => {}
                    }
                }
            }
            Some("system") => match value.get("subtype").and_then(Value::as_str) {
                Some("turn_duration") => {
                    self.lifecycle_notice(at, offset, "end", None, Some("success"))
                }
                Some("compact_boundary") => {
                    self.lifecycle_notice(at, offset, "phase", Some("compacted"), Some("success"))
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn index_codex(&mut self, offset: u64, bytes: &[u8]) {
        let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
            return;
        };
        let Some(at) = timestamp(&value) else { return };
        let Some(payload) = value.get("payload") else {
            return;
        };
        let kind = payload.get("type").and_then(Value::as_str);
        match value.get("type").and_then(Value::as_str) {
            Some("event_msg") if matches!(kind, Some("task_started" | "turn_started")) => {
                if let Some(turn) = payload.get("turn_id").and_then(Value::as_str) {
                    self.turn = Some(turn.to_owned());
                }
                self.lifecycle_notice(at, offset, "start", Some("setup"), None);
            }
            Some("event_msg")
                if matches!(
                    kind,
                    Some("task_complete" | "turn_complete" | "turn_aborted")
                ) =>
            {
                self.lifecycle_notice(
                    at,
                    offset,
                    "end",
                    None,
                    Some(if kind == Some("turn_aborted") {
                        "cancelled"
                    } else {
                        "success"
                    }),
                );
            }
            Some("compacted") => {
                self.lifecycle_notice(at, offset, "phase", Some("compacted"), Some("success"))
            }
            Some("response_item") => match kind {
                Some("reasoning") => {
                    let text = codex_reasoning(payload);
                    if !text.trim().is_empty() {
                        self.push(at, offset, 0, WorkEventKind::Thinking, text, None);
                        self.annotate_reasoning("codex_summary");
                    }
                }
                Some("function_call" | "custom_tool_call" | "local_shell_call") => {
                    let (name, input) = codex_call(payload);
                    let (event_kind, text) = tool_summary(&name, &input);
                    let call = payload.get("call_id").and_then(Value::as_str);
                    self.push(at, offset, 0, event_kind, text, call);
                    self.annotate_tool(&name, &input, call);
                }
                Some(
                    "function_call_output" | "custom_tool_call_output" | "local_shell_call_output",
                ) => {
                    if let Some(call) = payload.get("call_id").and_then(Value::as_str) {
                        let result = payload.get("output").unwrap_or(&Value::Null);
                        let known_success = published_success(result, 0);
                        self.settle(call, offset, at, false, result, known_success);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn find(&self, line: u64, block: usize) -> Option<&Indexed> {
        let start = self.events.partition_point(|indexed| indexed.line < line);
        self.events[start..]
            .iter()
            .take_while(|indexed| indexed.line == line)
            .find(|indexed| indexed.block == block)
    }

    fn read_line(&self, offset: u64) -> Option<Value> {
        let mut file = std::fs::File::open(&self.path).ok()?;
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut bytes = Vec::new();
        BufReader::new(file).read_until(b'\n', &mut bytes).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// The unredacted detail of one indexed event.
    fn detail(&self, line: u64, block: usize) -> Option<String> {
        let indexed = self.find(line, block)?;
        let value = self.read_line(line)?;
        let result = indexed
            .result_line
            .and_then(|offset| self.read_line(offset));
        match self.source.cli {
            cas_mux::SupervisorCli::Codex => {
                let payload = value.get("payload")?;
                if indexed.event.kind == WorkEventKind::Thinking {
                    return Some(codex_reasoning(payload));
                }
                let (name, input) = codex_call(payload);
                let output = result
                    .as_ref()
                    .and_then(|value| value.pointer("/payload/output"))
                    .map(output_text);
                Some(tool_detail(
                    &name,
                    &input,
                    output.as_deref(),
                    indexed.event.error,
                ))
            }
            _ => {
                let item = value.pointer("/message/content")?.get(block)?;
                if indexed.event.kind == WorkEventKind::Thinking {
                    return item
                        .get("thinking")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                let call = item.get("id").and_then(Value::as_str);
                let output = result.as_ref().and_then(|value| {
                    value
                        .pointer("/message/content")?
                        .as_array()?
                        .iter()
                        .find(|block| block.get("tool_use_id").and_then(Value::as_str) == call)
                        .and_then(|block| block.get("content"))
                        .map(output_text)
                });
                Some(tool_detail(
                    item.get("name").and_then(Value::as_str).unwrap_or("tool"),
                    item.get("input").unwrap_or(&Value::Null),
                    output.as_deref(),
                    indexed.event.error,
                ))
            }
        }
    }
}

fn codex_reasoning(payload: &Value) -> String {
    payload
        .get("summary")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn codex_call(payload: &Value) -> (String, Value) {
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("shell")
        .to_owned();
    let input = match payload.get("arguments").or_else(|| payload.get("input")) {
        Some(Value::String(raw)) => {
            serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.clone()))
        }
        Some(other) => other.clone(),
        None => payload.get("action").cloned().unwrap_or(Value::Null),
    };
    (name, input)
}

/// Only structured runner metadata; do not guess exit status from tool prose.
fn result_facts(value: &Value, depth: usize) -> (Option<i64>, Option<String>) {
    if depth > 8 {
        return (None, None);
    }
    if let Some(text) = value.as_str() {
        if let Ok(value) = serde_json::from_str::<Value>(text) {
            return result_facts(&value, depth + 1);
        }
        // Older Codex shell tools publish a fixed runner header before Final output.
        let header = text
            .split_once("Final output:")
            .map(|(header, _)| header)
            .unwrap_or("");
        let code = header.lines().find_map(|line| {
            line.strip_prefix("Process exited with code ")?
                .trim()
                .parse()
                .ok()
        });
        let session = header.lines().find_map(|line| {
            line.strip_prefix("Process running with session ID ")
                .map(|id| id.trim().to_owned())
        });
        return (code, session);
    }
    let code = value
        .get("exit_code")
        .or_else(|| value.get("exitCode"))
        .or_else(|| value.pointer("/metadata/exit_code"))
        .and_then(Value::as_i64);
    let session = value.get("session_id").and_then(session_key);
    if code.is_some() || session.is_some() {
        return (code, session);
    }
    if let Some(parts) = value.as_array() {
        for part in parts {
            let result = result_facts(part.get("text").unwrap_or(part), depth + 1);
            if result.0.is_some() || result.1.is_some() {
                return result;
            }
        }
    }
    (None, None)
}

/// apply_patch has a published success header; arbitrary output is not proof of an edit.
fn published_success(value: &Value, depth: usize) -> bool {
    if depth > 8 {
        return false;
    }
    if let Some(text) = value.as_str() {
        return if let Ok(value) = serde_json::from_str::<Value>(text) {
            published_success(&value, depth + 1)
        } else {
            text.starts_with("Success. Updated the following files:")
        };
    }
    if value.get("success").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    value.as_array().is_some_and(|parts| {
        parts
            .iter()
            .any(|part| published_success(part.get("text").unwrap_or(part), depth + 1))
    })
}

fn session_key(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|id| id.to_string()))
}

fn tool_payload(name: &str, input: &Value) -> WorkTool {
    let short = name.rsplit('.').next().unwrap_or(name);
    let operation = match short {
        "Read" | "read_file" => "read",
        "Grep" | "Glob" | "grep" | "search" | "search_query" => "search",
        "Bash" | "shell" | "shell_command" | "exec_command" | "local_shell" => "command",
        "Edit" | "MultiEdit" | "Write" | "apply_patch" => "edit",
        _ => "other",
    };
    let clean = |text: &str| bounded(&redact(text), 2048).0;
    let mut targets: Vec<String> = ["file_path", "path", "file", "target_file"]
        .iter()
        .filter_map(|key| input.get(*key).and_then(Value::as_str))
        .map(clean)
        .collect();
    if short == "apply_patch"
        && let Some(patch) = input
            .as_str()
            .or_else(|| input.get("patch").and_then(Value::as_str))
    {
        for line in patch.lines() {
            if let Some(path) = [
                "*** Update File: ",
                "*** Add File: ",
                "*** Delete File: ",
                "*** Move to: ",
            ]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
                && targets.len() < 128
            {
                targets.push(clean(path));
            }
        }
    }
    targets.sort();
    targets.dedup();
    let files = matches!(operation, "read" | "edit")
        .then_some(targets.len() as u64)
        .filter(|count| *count > 0);
    let command = input
        .get("cmd")
        .or_else(|| input.get("command"))
        .map(|command| {
            clean(&if let Some(parts) = command.as_array() {
                parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                command.as_str().unwrap_or("").to_owned()
            })
        });
    WorkTool {
        name: clean(name),
        operation: operation.into(),
        targets,
        command,
        cwd: input
            .get("cwd")
            .or_else(|| input.get("workdir"))
            .and_then(Value::as_str)
            .map(clean),
        exit_code: None,
        counts: files.map(|files| WorkToolCounts {
            files: Some(files),
            matches: None,
        }),
    }
}

fn output_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|part| match part.get("type").and_then(Value::as_str) {
                Some("text" | "input_text" | "output_text") => part
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                Some(other) => format!("[{other}]"),
                None => part.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

fn display_name(name: &str) -> String {
    name.strip_prefix("mcp__")
        .map(|rest| rest.replace("__", "."))
        .unwrap_or_else(|| name.to_owned())
}

/// The event kind and one-line text of a tool call.
fn tool_summary(name: &str, input: &Value) -> (WorkEventKind, String) {
    let steps = input
        .get("todos")
        .or_else(|| input.get("plan"))
        .and_then(Value::as_array);
    if let (Some(steps), true) = (steps, matches!(name, "TodoWrite" | "update_plan")) {
        let done = steps
            .iter()
            .filter(|step| step.get("status").and_then(Value::as_str) == Some("completed"))
            .count();
        let active = steps
            .iter()
            .find(|step| step.get("status").and_then(Value::as_str) == Some("in_progress"))
            .and_then(|step| {
                ["activeForm", "content", "step"]
                    .iter()
                    .find_map(|key| step.get(*key).and_then(Value::as_str))
            });
        let mut text = format!("Todos {done}/{}", steps.len());
        if let Some(active) = active {
            text.push_str(" · ");
            text.push_str(active);
        }
        return (WorkEventKind::Todo, text);
    }
    let field = |key: &str| match input.get(key) {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Array(parts)) => Some(
            parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    };
    let hint = if let Some(action) = field("action") {
        let subject = ["id", "target", "task_id", "query"]
            .iter()
            .find_map(|key| field(key));
        Some(match subject {
            Some(subject) => format!("{action} {subject}"),
            None => action,
        })
    } else {
        [
            "description", "command", "cmd", "file_path", "path", "pattern", "query", "url",
            "skill", "prompt",
        ]
        .iter()
        .find_map(|key| field(key))
    }
    .or_else(|| input.as_str().map(str::to_owned));
    let name = display_name(name);
    let text = match hint.filter(|hint| !hint.trim().is_empty()) {
        Some(hint) => format!("{name}: {}", hint.trim()),
        None => name,
    };
    (WorkEventKind::Tool, text)
}

fn tool_detail(name: &str, input: &Value, output: Option<&str>, error: bool) -> String {
    let input = match input {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    };
    let mut detail = format!("{}\n\nInput:\n{input}", display_name(name));
    if let Some(output) = output {
        detail.push_str(if error { "\n\nResult (error):\n" } else { "\n\nResult:\n" });
        detail.push_str(output);
    }
    detail
}

fn words(event_type: &str) -> String {
    event_type.replace('_', " ")
}

/// A supervisor-queue row as a work event.
fn queue_event(session: &str, row: &cas_store::SupervisorNotification) -> (SortKey, WorkEvent) {
    let payload: Value = serde_json::from_str(&row.payload).unwrap_or(Value::Null);
    let field = |key: &str| {
        payload
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
    };
    let event_type = row.event_type.as_str();
    let (kind, text) = if event_type == "task_lifecycle" || event_type.starts_with("task_") {
        let transition = field("transition").unwrap_or(event_type);
        let verb = match (transition, field("new_status")) {
            ("task_started", _) | (_, Some("in_progress")) => "started",
            ("task_blocked", _) | (_, Some("blocked")) => "blocked",
            ("task_awaiting_merge", _) | (_, Some("awaiting_merge")) => "parked for merge",
            ("task_close_rejected", _) => "close rejected",
            ("task_closed", _) | (_, Some("closed")) => "closed",
            ("task_ready", _) | (_, Some("open")) => "reopened",
            (other, _) => other.trim_start_matches("task_"),
        };
        let subject = field("task_id").unwrap_or("task");
        let title = field("title").or(field("task_title")).unwrap_or("");
        let verb = words(verb);
        (
            WorkEventKind::Task,
            format!("{subject} {verb}: {title}")
                .trim_end_matches([':', ' '])
                .to_owned(),
        )
    } else {
        let kind = if event_type.starts_with("worker") {
            WorkEventKind::Worker
        } else if ["merge", "verification", "epic", "delivery"]
            .iter()
            .any(|prefix| event_type.starts_with(prefix))
        {
            WorkEventKind::Task
        } else {
            WorkEventKind::Notice
        };
        let text = field("detail")
            .or(field("message"))
            .map(str::to_owned)
            .unwrap_or_else(|| words(event_type));
        (kind, text)
    };
    let agent = ["actor", "worker", "worker_name"]
        .iter()
        .find_map(|key| field(key))
        .map(str::to_owned);
    keyed(
        WorkEvent {
            id: format!("q:{:015}", row.id),
            kind,
            session: session.to_owned(),
            agent,
            turn: None,
            at: row.created_at.to_rfc3339(),
            label: label(&text),
            has_detail: true,
            error: false,
            tool: None,
            lifecycle: None,
            reasoning: None,
            changed_files: None,
        },
        row.created_at,
    )
}

fn hook_event(scope: &WorkEventScope, event: &cas_types::Event) -> (SortKey, WorkEvent) {
    let agent = event
        .session_id
        .as_deref()
        .and_then(|id| scope.agents.get(id))
        .cloned();
    keyed(
        WorkEvent {
            id: format!("e:{:015}", event.id),
            kind: WorkEventKind::Worker,
            session: scope.session.clone(),
            agent,
            turn: None,
            at: event.created_at.to_rfc3339(),
            label: label(&event.summary),
            has_detail: false,
            error: false,
            tool: None,
            lifecycle: None,
            reasoning: None,
            changed_files: None,
        },
        event.created_at,
    )
}

/// Read-only stores the stream queries. Either may be missing.
#[derive(Default)]
pub(crate) struct WorkEventStores {
    pub queue: Option<SqliteSupervisorQueueStore>,
    pub events: Option<SqliteEventStore>,
}

impl WorkEventStores {
    pub(crate) fn open(cas_dir: &Path) -> Self {
        Self {
            queue: SqliteSupervisorQueueStore::open(cas_dir).ok(),
            events: SqliteEventStore::open(cas_dir).ok(),
        }
    }
}

/// Finds a supervisor's transcript.
pub(crate) type TranscriptResolver<'a> = &'a dyn Fn(&SupervisorSource) -> Option<PathBuf>;

/// Per-daemon transcript indexes, kept across requests so paging and
/// detail reads never re-parse a transcript.
#[derive(Default)]
pub(crate) struct WorkEventIndex {
    transcripts: HashMap<String, TranscriptIndex>,
    /// Worker transcript paths the change probe stats (cas-ff22), resolved
    /// once per agent.
    probe_paths: HashMap<String, PathBuf>,
}

impl WorkEventIndex {
    fn refresh(
        &mut self,
        scope: &WorkEventScope,
        sources: &[&SupervisorSource],
        resolve: TranscriptResolver<'_>,
    ) {
        self.transcripts
            .retain(|id, _| scope.members().any(|source| &source.agent_id == id));
        for &source in sources {
            let cached = self
                .transcripts
                .get(&source.agent_id)
                .filter(|index| &index.source == source && index.path.is_file());
            if cached.is_none() {
                match resolve(source) {
                    Some(path) => {
                        self.transcripts.insert(
                            source.agent_id.clone(),
                            TranscriptIndex::new(source.clone(), &scope.session, path),
                        );
                    }
                    None => {
                        self.transcripts.remove(&source.agent_id);
                        continue;
                    }
                }
            }
            if let Some(index) = self.transcripts.get_mut(&source.agent_id)
                && let Err(error) = index.refresh()
            {
                tracing::debug!(agent = %source.agent_name, %error, "work events: transcript unreadable");
            }
        }
    }

    /// One page, newest `limit` events before `before`, oldest first.
    pub(crate) fn page(
        &mut self,
        scope: &WorkEventScope,
        stores: &WorkEventStores,
        resolve: TranscriptResolver<'_>,
        request_id: String,
        before: Option<&str>,
        limit: u16,
        agent: Option<&str>,
    ) -> anyhow::Result<DaemonMessage> {
        let cursor = match before {
            Some(cursor) => Some(
                SortKey::decode(cursor)
                    .ok_or_else(|| anyhow::anyhow!("invalid work event cursor"))?,
            ),
            None => None,
        };
        let limit = usize::from(if limit == 0 {
            WORK_EVENT_PAGE_SIZE
        } else {
            limit.min(WORK_EVENT_PAGE_SIZE)
        });
        let wanted = limit + 1;
        let earlier = |key: &SortKey| cursor.as_ref().is_none_or(|cursor| key < cursor);
        // A row in the cursor's millisecond may still be unserved.
        let until = cursor
            .as_ref()
            .and_then(|cursor| DateTime::<Utc>::from_timestamp_millis(cursor.at_ms + 1));

        let sources = scope.transcripts(agent);
        self.refresh(scope, &sources, resolve);
        let ours =
            |event: &WorkEvent| agent.is_none_or(|name| event.agent.as_deref() == Some(name));
        let mut candidates: Vec<(SortKey, WorkEvent)> = Vec::new();
        let read: Vec<&TranscriptIndex> = sources
            .iter()
            .filter_map(|source| self.transcripts.get(&source.agent_id))
            .collect();
        for index in read {
            let mut eligible: Vec<&Indexed> = index
                .events
                .iter()
                .filter(|indexed| earlier(&indexed.key))
                .collect();
            eligible.sort_by(|left, right| right.key.cmp(&left.key));
            candidates.extend(
                eligible
                    .into_iter()
                    .take(wanted)
                    .map(|indexed| (indexed.key.clone(), indexed.event.clone())),
            );
        }
        if let Some(queue) = &stores.queue {
            let supervisors: Vec<String> = scope
                .supervisors
                .iter()
                .map(|source| source.agent_id.clone())
                .collect();
            for row in queue.list_for_session_until(
                &scope.session,
                &supervisors,
                until,
                wanted + BOUNDARY_SLACK,
            )? {
                let (key, event) = queue_event(&scope.session, &row);
                if earlier(&key) && ours(&event) {
                    candidates.push((key, event));
                }
            }
        }
        if let Some(events) = &stores.events {
            let agents: Vec<String> = scope
                .agents
                .iter()
                .filter(|(_, name)| agent.is_none_or(|agent| agent == name.as_str()))
                .map(|(id, _)| id.clone())
                .collect();
            for row in events.list_for_sessions_until(
                &agents,
                HOOK_EVENT_TYPES,
                scope.floor,
                until,
                wanted + BOUNDARY_SLACK,
            )? {
                let (key, event) = hook_event(scope, &row);
                if earlier(&key) {
                    candidates.push((key, event));
                }
            }
        }
        candidates.sort_by(|left, right| right.0.cmp(&left.0));
        candidates.dedup_by(|left, right| left.0 == right.0);
        let has_earlier = candidates.len() > limit;
        candidates.truncate(limit);
        let next_before = candidates.last().map(|(key, _)| key.encode());
        candidates.reverse();
        Ok(DaemonMessage::WorkEvents {
            request_id,
            schema_version: WORK_EVENT_SCHEMA_VERSION,
            events: candidates.into_iter().map(|(_, event)| event).collect(),
            has_earlier,
            next_before,
        })
    }

    /// A fingerprint of the stream's newest state (cas-ff22): the session
    /// stream's newest event and the length of each worker's transcript. A
    /// worker's transcript is only stat'ed here, never indexed; it is read
    /// when a client asks for that worker's timeline.
    pub(crate) fn fingerprint(
        &mut self,
        scope: &WorkEventScope,
        stores: &WorkEventStores,
        resolve: TranscriptResolver<'_>,
    ) -> anyhow::Result<String> {
        let newest = match self.page(scope, stores, resolve, String::new(), None, 1, None)? {
            DaemonMessage::WorkEvents { events, .. } => events.last().map(|event| event.id.clone()),
            _ => None,
        };
        self.probe_paths
            .retain(|id, _| scope.members().any(|source| &source.agent_id == id));
        let mut fingerprint = newest.unwrap_or_default();
        for source in scope.members() {
            let cached = self
                .probe_paths
                .get(&source.agent_id)
                .filter(|path| path.is_file())
                .cloned();
            let Some(path) = cached.or_else(|| resolve(source)) else {
                continue;
            };
            let len = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            self.probe_paths.insert(source.agent_id.clone(), path);
            fingerprint.push_str(&format!("|{}={len}", source.agent_id));
        }
        Ok(fingerprint)
    }

    /// The redacted detail of one event of this session.
    pub(crate) fn detail(
        &mut self,
        scope: &WorkEventScope,
        stores: &WorkEventStores,
        resolve: TranscriptResolver<'_>,
        request_id: String,
        event_id: String,
    ) -> DaemonMessage {
        let raw = self.raw_detail(scope, stores, resolve, &event_id);
        let (detail, truncated) = match raw {
            Some(raw) => {
                let (detail, truncated) = bounded(&redact(&raw), DETAIL_CHARS);
                (Some(detail), truncated)
            }
            None => (None, false),
        };
        DaemonMessage::WorkEventDetail {
            request_id,
            event_id,
            detail,
            truncated,
        }
    }

    fn raw_detail(
        &mut self,
        scope: &WorkEventScope,
        stores: &WorkEventStores,
        resolve: TranscriptResolver<'_>,
        event_id: &str,
    ) -> Option<String> {
        let mut parts = event_id.split(':');
        match parts.next()? {
            "t" => {
                let (agent, line, block) = (parts.next()?, parts.next()?, parts.next()?);
                let source = scope.members().find(|source| source.agent_id == agent)?;
                if !self.transcripts.contains_key(agent) {
                    self.refresh(scope, &[source], resolve);
                }
                self.transcripts
                    .get(agent)?
                    .detail(line.parse().ok()?, block.parse().ok()?)
            }
            "q" => {
                let row = stores.queue.as_ref()?.get(parts.next()?.parse().ok()?).ok()??;
                let payload: Value = serde_json::from_str(&row.payload).unwrap_or(Value::Null);
                let addressed = scope
                    .supervisors
                    .iter()
                    .any(|source| source.agent_id == row.supervisor_id)
                    || payload.get("factory_session").and_then(Value::as_str)
                        == Some(scope.session.as_str());
                addressed.then(|| {
                    let payload = crate::ai_enrichment::redact_json(&payload);
                    format!(
                        "{}\n\n{}",
                        words(&row.event_type),
                        serde_json::to_string_pretty(&payload).unwrap_or_default()
                    )
                })
            }
            _ => None,
        }
    }
}

/// A request the job lane answers.
pub(crate) enum WorkEventRequest {
    Page {
        before: Option<String>,
        limit: u16,
        agent: Option<String>,
    },
    Detail {
        event_id: String,
    },
}

/// How often the change probe runs while a client follows the stream.
pub(crate) const PROBE_INTERVAL: Duration = Duration::from_secs(2);
const PROBE_KEY: &str = "WorkEvents:probe";

/// The daemon's handle: one shared index, and the requests in flight so a
/// dropped or failed job still answers its client.
#[derive(Default)]
pub(crate) struct WorkEventService {
    index: Arc<Mutex<WorkEventIndex>>,
    next: u64,
    pending: HashMap<String, (usize, String)>,
    /// Clients that have read a page; they are told when the stream grows.
    subscribers: HashSet<usize>,
    probe_in_flight: bool,
    last_probe: Option<Instant>,
    /// `None` until the first probe sets the baseline.
    fingerprint: Option<String>,
}

/// One answer for one WebSocket client.
pub(crate) struct WorkEventReply {
    pub client_id: usize,
    pub message: DaemonMessage,
}

impl WorkEventService {
    /// Run one request on the blocking lane; the loop never reads a
    /// transcript or the stores for it.
    pub(crate) fn submit(
        &mut self,
        jobs: &mut QueueJobs,
        client_id: usize,
        request_id: String,
        request: WorkEventRequest,
        cas_dir: PathBuf,
        project_path: PathBuf,
        session: String,
    ) -> Option<WorkEventReply> {
        if matches!(request, WorkEventRequest::Page { .. }) {
            self.subscribers.insert(client_id);
        }
        self.next += 1;
        let key = format!("{JOB_KIND}:{client_id}:{}", self.next);
        self.pending
            .insert(key.clone(), (client_id, request_id.clone()));
        let index = Arc::clone(&self.index);
        let spec = JobSpec {
            kind: JOB_KIND,
            key: key.clone(),
            lane: Lane::Blocking,
            criticality: Criticality::Refresh,
            generation: self.next,
            budget: JOB_BUDGET,
        };
        let submitted = jobs.submit_external(spec, move || {
            let message = answer(
                &index,
                &cas_dir,
                &project_path,
                &session,
                request_id,
                request,
            );
            Box::new(message) as Box<dyn std::any::Any + Send>
        });
        use super::jobs::Submitted;
        match submitted {
            Submitted::Queued
            | Submitted::QueuedBehindKey
            | Submitted::QueuedDroppedOldest { .. }
            | Submitted::Coalesced { .. } => None,
            refused => {
                tracing::warn!(?refused, "work events request refused by the job runtime");
                let (client_id, request_id) = self.pending.remove(&key)?;
                Some(WorkEventReply {
                    client_id,
                    message: failure(request_id, "work events are busy; retry"),
                })
            }
        }
    }

    /// Whether the change probe should run: a live client follows the stream
    /// and the last probe is `PROBE_INTERVAL` old (cas-ff22). The probe runs
    /// on the blocking lane like a page; the loop only compares strings.
    pub(crate) fn probe_due(&mut self, now: Instant, live: impl Fn(usize) -> bool) -> bool {
        self.subscribers.retain(|client| live(*client));
        if self.subscribers.is_empty() {
            self.fingerprint = None;
            return false;
        }
        !self.probe_in_flight && self.last_probe.is_none_or(|last| now.duration_since(last) >= PROBE_INTERVAL)
    }

    pub(crate) fn start_probe(
        &mut self,
        jobs: &mut QueueJobs,
        now: Instant,
        cas_dir: PathBuf,
        project_path: PathBuf,
        session: String,
    ) {
        self.last_probe = Some(now);
        self.next += 1;
        let index = Arc::clone(&self.index);
        let spec = JobSpec {
            kind: JOB_KIND,
            key: PROBE_KEY.to_owned(),
            lane: Lane::Blocking,
            criticality: Criticality::Refresh,
            generation: self.next,
            budget: JOB_BUDGET,
        };
        let submitted = jobs.submit_external(spec, move || {
            Box::new(probe(&index, &cas_dir, &project_path, &session)) as Box<dyn std::any::Any + Send>
        });
        use super::jobs::Submitted;
        self.probe_in_flight = matches!(
            submitted,
            Submitted::Queued | Submitted::QueuedBehindKey | Submitted::QueuedDroppedOldest { .. } | Submitted::Coalesced { .. }
        );
    }

    /// A finished probe: the subscribers to tell, when the stream changed.
    fn settle_probe(&mut self, fingerprint: Option<String>) -> Vec<WorkEventReply> {
        self.probe_in_flight = false;
        let Some(fingerprint) = fingerprint else {
            return Vec::new();
        };
        let changed = self
            .fingerprint
            .as_ref()
            .is_some_and(|previous| previous != &fingerprint);
        self.fingerprint = Some(fingerprint);
        if !changed {
            return Vec::new();
        }
        let mut clients: Vec<usize> = self.subscribers.iter().copied().collect();
        clients.sort_unstable();
        clients
            .into_iter()
            .map(|client_id| WorkEventReply {
                client_id,
                message: DaemonMessage::WorkEventsAppended {
                    schema_version: WORK_EVENT_SCHEMA_VERSION,
                },
            })
            .collect()
    }

    /// Answers whose jobs finished, including failures, and the change
    /// notices a finished probe produced.
    pub(crate) fn take(&mut self, jobs: &mut QueueJobs) -> Vec<WorkEventReply> {
        if self.pending.is_empty() && !self.probe_in_flight {
            return Vec::new();
        }
        jobs.apply_completions();
        let mut notices = Vec::new();
        let mut replies: Vec<WorkEventReply> = jobs
            .take_external_completions_where(|kind| kind == JOB_KIND)
            .into_iter()
            .filter_map(|result| {
                if result.key == PROBE_KEY {
                    let fingerprint = match result.outcome {
                        Ok(ExternalOutcome::Done(done)) => done
                            .downcast::<Option<String>>()
                            .ok()
                            .and_then(|found| *found),
                        _ => None,
                    };
                    notices.extend(self.settle_probe(fingerprint));
                    return None;
                }
                let (client_id, request_id) = self.pending.remove(&result.key)?;
                let message = match result.outcome {
                    Ok(ExternalOutcome::Done(done)) => match done.downcast::<DaemonMessage>() {
                        Ok(message) => *message,
                        Err(_) => failure(request_id, "work events failed"),
                    },
                    Ok(ExternalOutcome::Dropped) => {
                        failure(request_id, "work events are busy; retry")
                    }
                    Err(panicked) => {
                        tracing::warn!(error = %panicked.message, "work events job panicked");
                        failure(request_id, "work events failed")
                    }
                };
                Some(WorkEventReply { client_id, message })
            })
            .collect();
        replies.extend(notices);
        replies
    }
}

fn failure(request_id: String, message: &str) -> DaemonMessage {
    DaemonMessage::Error {
        message: message.to_owned(),
        client_ref: Some(request_id),
    }
}

/// The live session scope and its stores, with the shared index locked.
fn with_index<T>(
    index: &Mutex<WorkEventIndex>,
    cas_dir: &Path,
    project_path: &Path,
    session: &str,
    run: impl FnOnce(
        &mut WorkEventIndex,
        &WorkEventScope,
        &WorkEventStores,
        TranscriptResolver<'_>,
    ) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let agents = crate::store::open_agent_store(cas_dir)?.list(None)?;
    let scope = WorkEventScope::from_agents(session, &agents);
    let stores = WorkEventStores::open(cas_dir);
    let resolve = |source: &SupervisorSource| {
        crate::mcp::tools::service::factory_ops::resolve_worker_transcript_path_for_account(
            project_path.to_str(),
            &source.harness_session,
            source.cli,
            source.account_dir.as_deref(),
        )
    };
    let mut index = index
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    run(&mut index, &scope, &stores, &resolve)
}

fn answer(
    index: &Mutex<WorkEventIndex>,
    cas_dir: &Path,
    project_path: &Path,
    session: &str,
    request_id: String,
    request: WorkEventRequest,
) -> DaemonMessage {
    let id = request_id.clone();
    with_index(
        index,
        cas_dir,
        project_path,
        session,
        |index, scope, stores, resolve| match request {
            WorkEventRequest::Page {
                before,
                limit,
                agent,
            } => index.page(
                scope,
                stores,
                resolve,
                request_id,
                before.as_deref(),
                limit,
                agent.as_deref(),
            ),
            WorkEventRequest::Detail { event_id } => {
                Ok(index.detail(scope, stores, resolve, request_id, event_id))
            }
        },
    )
    .unwrap_or_else(|error| failure(id, &format!("work events failed: {error}")))
}

/// The change probe's job: the stream's fingerprint, or `None` when the
/// stores cannot be read this time.
fn probe(
    index: &Mutex<WorkEventIndex>,
    cas_dir: &Path,
    project_path: &Path,
    session: &str,
) -> Option<String> {
    with_index(
        index,
        cas_dir,
        project_path,
        session,
        |index, scope, stores, resolve| index.fingerprint(scope, stores, resolve),
    )
    .inspect_err(|error| tracing::debug!(%error, "work events: change probe failed"))
    .ok()
}

#[cfg(test)]
#[path = "work_events_tests.rs"]
mod tests;
