//! Incremental native-turn accounting, independent of Commander mirroring.
//!
//! Only typed facts survive a line: no prompts, arguments, commands or results
//! are retained or exported. A no-op means no successful action was observed;
//! it is not a model judgment about the usefulness of a turn.
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use cas_store::{
    NewQualityEvent, PromptQueueStore, QualityPayload, QueueOrigin, TurnAction, WakeChannel,
};
use cas_types::{Agent, AgentRole};
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::commander_mirror::{RolloutOwnership, codex_rollout_owns_recipient, no_action_reply};
use super::work_events::{
    SupervisorSource, WorkEventScope, codex_call, published_success, result_facts,
};

#[derive(Default)]
pub(super) struct Scanner {
    cursors: HashMap<String, Cursor>,
    misses: HashMap<String, (SupervisorSource, Instant)>,
    ownership: RefCell<HashMap<PathBuf, RolloutOwnership>>,
    #[cfg(test)]
    pub(super) before_read: Option<Box<dyn FnOnce(&Path) + Send>>,
}

struct Cursor {
    source: SupervisorSource,
    path: PathBuf,
    offset: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    pending: Option<Pending>,
    usage: (u64, u64),
    outbox: VecDeque<NewQualityEvent>,
    retry_at: Option<Instant>,
    #[cfg(test)]
    before_read: Option<Box<dyn FnOnce(&Path) + Send>>,
    #[cfg(test)]
    bytes_read: u64,
}

struct Pending {
    id: String,
    started: Option<DateTime<Utc>>,
    wake: Option<WakeChannel>,
    may_answer: bool,
    actions: BTreeSet<TurnAction>,
    calls: HashMap<String, (TurnAction, bool)>,
    // Claude streams multiple rows with one message id; use its final usage once.
    messages: HashMap<String, (u64, u64)>,
    usage: Option<(u64, u64)>,
    baseline: (u64, u64),
    reply: String,
}

fn timestamp(value: &Value) -> Option<DateTime<Utc>> {
    value
        .get("timestamp")
        .or_else(|| value.get("ts"))
        .and_then(|at| {
            at.as_str()
                .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
                .map(|at| at.with_timezone(&Utc))
                .or_else(|| {
                    at.as_i64()
                        .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
                })
        })
}

fn text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_owned();
    }
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| match part.get("type").and_then(Value::as_str) {
            Some("text" | "input_text" | "output_text") => part.get("text").and_then(Value::as_str),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Context supplied by the harness is not the input that woke the turn.
fn launch_context(prompt: &str) -> bool {
    let prompt = prompt.trim();
    prompt.starts_with("# AGENTS.md instructions for ")
        || (prompt.starts_with("<environment_context>")
            && prompt.ends_with("</environment_context>"))
}

/// IDs inside a Teams envelope or a pointer select only authenticated queue
/// facts addressed to this recipient. Sender prose never grants provenance.
fn wake(
    prompt: &str,
    agent: &Agent,
    session: &str,
    queue: &dyn PromptQueueStore,
    agents: &[Agent],
) -> (WakeChannel, bool) {
    use crate::hooks::handlers::handlers_middle::factory_inbox::{envelope_ids, wake_named_ids};
    let ids = envelope_ids(prompt)
        .into_iter()
        .chain(wake_named_ids(prompt));
    let Some(row) = ids
        .filter_map(|id| queue.queued_prompt(id).ok().flatten())
        .find(|row| {
            row.factory_session.as_deref() == Some(session)
                && (row.target == agent.name
                    || (agent.role == AgentRole::Supervisor && row.target == "supervisor"))
        })
    else {
        return (WakeChannel::Unknown, false);
    };
    let channel = match &row.origin {
        Some(QueueOrigin::PairedDevice { .. }) => WakeChannel::Operator,
        Some(QueueOrigin::Daemon) if row.source == super::teams::DIRECTOR_AGENT_NAME => {
            WakeChannel::Director
        }
        Some(QueueOrigin::Daemon) => WakeChannel::Daemon,
        Some(QueueOrigin::RegisteredAgent { agent_id }) => {
            match agents.iter().find(|a| &a.id == agent_id).map(|a| a.role) {
                Some(AgentRole::Supervisor) => WakeChannel::Supervisor,
                Some(AgentRole::Worker) => WakeChannel::Worker,
                _ => WakeChannel::Unknown,
            }
        }
        _ => WakeChannel::Unknown,
    };
    // A question is verified against the durable authenticated row, not the
    // transcript's copied prompt. Status prose on other wakes isn't an action.
    let may_answer = channel == WakeChannel::Operator
        || (channel != WakeChannel::Unknown
            && (row.kind.as_deref() == Some("ask") || row.prompt.contains('?')));
    (channel, may_answer)
}

/// A candidate isn't an action until its result succeeds. Reads, waits, polls,
/// receipts and status restatements deliberately have no candidate.
fn candidate(name: &str, input: &Value) -> Option<(TurnAction, bool)> {
    let short = name.rsplit('.').next().unwrap_or(name);
    let action = input.get("action").and_then(Value::as_str).unwrap_or("");
    if short == "use_tool" {
        return candidate(input.get("tool_name")?.as_str()?, input.get("tool_input")?);
    }
    if short.ends_with("task")
        && matches!(
            action,
            "create"
                | "update"
                | "start"
                | "close"
                | "cancel"
                | "reopen"
                | "request_changes"
                | "claim"
                | "release"
                | "transfer"
                | "dep_add"
                | "dep_remove"
                | "notes"
        )
    {
        // Reading notes uses the same action, with no notes body.
        if action == "notes" && input.get("notes").is_none() {
            return None;
        }
        return Some((TurnAction::TaskMutation, false));
    }
    if short.ends_with("coordination") && matches!(action, "message" | "interrupt") {
        let kind = input.get("kind").and_then(Value::as_str).unwrap_or("");
        let message = input.get("message").and_then(Value::as_str).unwrap_or("");
        if matches!(kind, "receipt" | "status") || no_action_reply(message) {
            return None;
        }
        return Some((
            if input.get("merge_request").and_then(Value::as_bool) == Some(true)
                || input.get("blocker").and_then(Value::as_bool) == Some(true)
            {
                TurnAction::Decision
            } else {
                TurnAction::Answer
            },
            false,
        ));
    }
    if matches!(
        short,
        "Edit" | "Write" | "MultiEdit" | "apply_patch" | "search_replace" | "write_file"
    ) {
        return Some((TurnAction::FileEdit, false));
    }
    if matches!(
        short,
        "Bash" | "exec_command" | "shell" | "shell_command" | "run_terminal_command"
    ) {
        let command = input
            .get("cmd")
            .or_else(|| input.get("command"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let action = if command.starts_with("git commit ") {
            TurnAction::Commit
        } else if command.starts_with("git push ") {
            TurnAction::Push
        } else if command.starts_with("cargo nextest ")
            || command.starts_with("cargo check ")
            || command.starts_with("npm test")
        {
            TurnAction::Check
        } else {
            return None;
        };
        return Some((action, true));
    }
    None
}

fn failed(value: &Value) -> bool {
    if let Some(raw) = value.as_str() {
        return serde_json::from_str::<Value>(raw)
            .ok()
            .is_some_and(|v| failed(&v));
    }
    value.pointer("/output/ErrorOutput").is_some()
        || value
            .get("is_error")
            .or_else(|| value.get("isError"))
            .and_then(Value::as_bool)
            == Some(true)
        || value.get("success").and_then(Value::as_bool) == Some(false)
        || result_facts(value, 0).0.is_some_and(|code| code != 0)
        || value
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|parts| parts.iter().any(|p| failed(p.get("text").unwrap_or(p))))
}

impl Pending {
    fn new(id: &str, at: Option<DateTime<Utc>>, baseline: (u64, u64)) -> Self {
        Self {
            id: id.to_owned(),
            started: at,
            wake: None,
            may_answer: false,
            actions: BTreeSet::new(),
            calls: HashMap::new(),
            messages: HashMap::new(),
            usage: None,
            baseline,
            reply: String::new(),
        }
    }

    fn call(&mut self, id: &str, name: &str, input: &Value) {
        if self.calls.len() < 4096
            && let Some(candidate) = candidate(name, input)
        {
            self.calls.insert(id.to_owned(), candidate);
        }
    }

    fn settle(&mut self, id: &str, result: &Value, known: bool) {
        if let Some((action, shell)) = self.calls.remove(id)
            && !failed(result)
            && (if shell {
                result_facts(result, 0).0 == Some(0)
            } else {
                known || published_success(result, 0)
            })
        {
            self.actions.insert(action);
        }
    }

    fn attribute(
        &mut self,
        prompt: &str,
        agent: &Agent,
        session: &str,
        queue: &dyn PromptQueueStore,
        agents: &[Agent],
    ) {
        if self.wake.is_some() || launch_context(prompt) {
            return;
        }
        let (channel, may_answer) = wake(prompt, agent, session, queue, agents);
        self.wake = Some(channel);
        self.may_answer = may_answer;
    }

    fn reply(&mut self, content: &Value) {
        let reply = text(content);
        if self.may_answer
            && !reply.trim().is_empty()
            && !no_action_reply(&reply)
            && !reply.trim_start().starts_with("status:")
        {
            self.actions.insert(TurnAction::Answer);
        }
    }
}

impl Cursor {
    fn new(source: SupervisorSource, path: PathBuf) -> Self {
        Self {
            source,
            path,
            offset: 0,
            modified: None,
            created: None,
            pending: None,
            usage: (0, 0),
            outbox: VecDeque::new(),
            retry_at: None,
            #[cfg(test)]
            before_read: None,
            #[cfg(test)]
            bytes_read: 0,
        }
    }

    fn complete(&mut self, at: Option<DateTime<Utc>>, root: &Path) {
        let Some(at) = at else { return };
        let Some(turn) = self.pending.take() else {
            return;
        };
        let usage = if turn.messages.is_empty() {
            turn.usage
        } else {
            Some(turn.messages.values().fold((0u64, 0u64), |(i, o), (a, b)| {
                (i.saturating_add(*a), o.saturating_add(*b))
            }))
        };
        let mut event = NewQualityEvent::new(
            format!(
                "turn|{}|{}|{}",
                self.source.agent_id, self.source.harness_session, turn.id
            ),
            QualityPayload::Turn {
                woken_by: turn.wake.unwrap_or(WakeChannel::Unknown),
                no_op: turn.actions.is_empty(),
                actions: turn.actions.into_iter().collect(),
                input_tokens: usage.map(|u| u.0),
                output_tokens: usage.map(|u| u.1),
                minutes: turn
                    .started
                    .filter(|start| *start <= at)
                    .map(|start| (at - start).num_milliseconds() as f64 / 60_000.0),
            },
        );
        event.occurred_at = at;
        event.actor = Some(self.source.agent_id.clone());
        self.outbox.push_back(event);
        self.flush(root);
    }

    fn flush(&mut self, root: &Path) {
        if self.retry_at.is_some_and(|next| Instant::now() < next) {
            return;
        }
        while let Some(event) = self.outbox.front() {
            match crate::quality_ledger::emit_checked(root, event) {
                Ok(_) => {
                    self.outbox.pop_front();
                }
                Err(error) => {
                    tracing::warn!(%error, "turn ledger write deferred; retaining event");
                    self.retry_at = Some(Instant::now() + Duration::from_secs(30));
                    return;
                }
            }
        }
        self.retry_at = None;
    }

    fn line(
        &mut self,
        value: &Value,
        root: &Path,
        agent: &Agent,
        session: &str,
        queue: &dyn PromptQueueStore,
        agents: &[Agent],
    ) {
        if value.get("isSidechain").and_then(Value::as_bool) == Some(true)
            || value.get("isMeta").and_then(Value::as_bool) == Some(true)
        {
            return;
        }
        let at = timestamp(value);
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        if self.source.cli == cas_mux::SupervisorCli::Claude {
            let content = value.pointer("/message/content").unwrap_or(&Value::Null);
            if kind == "user" {
                let results = content
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|p| p["type"] == "tool_result")
                    .collect::<Vec<_>>();
                if !results.is_empty() {
                    if let Some(turn) = &mut self.pending {
                        for result in results {
                            if let Some(id) = result["tool_use_id"].as_str() {
                                turn.settle(id, result, true);
                            }
                        }
                    }
                    return;
                }
                let prompt = text(content);
                if launch_context(&prompt) {
                    return;
                }
                if let Some(id) = value
                    .get("promptId")
                    .or_else(|| value.get("uuid"))
                    .and_then(Value::as_str)
                    && self.pending.as_ref().is_none_or(|turn| turn.id != id)
                {
                    // A fresh native input abandons an unfinished prior turn.
                    self.pending = Some(Pending::new(id, at, self.usage));
                }
                if let Some(turn) = &mut self.pending {
                    turn.attribute(&prompt, agent, session, queue, agents);
                }
            } else if kind == "assistant" {
                if let Some(turn) = &mut self.pending {
                    if let (Some(id), Some(usage)) = (
                        value.pointer("/message/id").and_then(Value::as_str),
                        value.pointer("/message/usage"),
                    ) {
                        if let (Some(i), Some(o)) = (
                            usage["input_tokens"].as_u64(),
                            usage["output_tokens"].as_u64(),
                        ) {
                            // Cached input is also model input, reported separately by Claude.
                            let i = i
                                .saturating_add(
                                    usage["cache_read_input_tokens"].as_u64().unwrap_or(0),
                                )
                                .saturating_add(
                                    usage["cache_creation_input_tokens"].as_u64().unwrap_or(0),
                                );
                            turn.messages.insert(id.to_owned(), (i, o));
                        }
                    }
                    for part in content.as_array().into_iter().flatten() {
                        if part["type"] == "tool_use"
                            && let (Some(id), Some(name)) =
                                (part["id"].as_str(), part["name"].as_str())
                        {
                            turn.call(id, name, &part["input"]);
                        }
                    }
                    if value
                        .pointer("/message/stop_reason")
                        .and_then(Value::as_str)
                        == Some("end_turn")
                    {
                        turn.reply(content);
                        // Thinking-only end_turn rows precede a final text row in some harness versions.
                        if !text(content).is_empty() {
                            self.complete(at, root);
                        }
                    }
                }
            } else if kind == "system" && value["subtype"] == "turn_duration" {
                self.complete(at, root);
            }
            return;
        }
        if self.source.cli == cas_mux::SupervisorCli::Grok {
            self.grok(value, at, root, agent, session, queue, agents);
            return;
        }
        if self.source.cli != cas_mux::SupervisorCli::Codex {
            return;
        }
        let payload = &value["payload"];
        let entry = payload["type"].as_str().unwrap_or("");
        if kind == "event_msg" {
            match entry {
                "task_started" | "turn_started" => {
                    if let Some(id) = payload["turn_id"].as_str()
                        && self.pending.as_ref().is_none_or(|turn| turn.id != id)
                    {
                        self.pending = Some(Pending::new(id, at, self.usage));
                    }
                }
                "token_count" => {
                    let usage = &payload["info"]["total_token_usage"];
                    if let (Some(i), Some(o)) = (
                        usage["input_tokens"].as_u64(),
                        usage["output_tokens"].as_u64(),
                    ) {
                        self.usage = (i, o);
                        if let Some(turn) = &mut self.pending {
                            turn.usage = i
                                .checked_sub(turn.baseline.0)
                                .zip(o.checked_sub(turn.baseline.1));
                        }
                    }
                }
                "task_complete" | "turn_complete" | "turn_completed" | "turn_aborted" => {
                    if self.pending.as_ref().is_some_and(|turn| {
                        payload["turn_id"].as_str().is_none_or(|id| id == turn.id)
                    }) {
                        self.complete(at, root);
                    }
                }
                _ => {}
            }
        } else if kind == "response_item"
            && let Some(turn) = &mut self.pending
        {
            match entry {
                "message" if payload["role"] == "user" && turn.wake.is_none() => {
                    turn.attribute(&text(&payload["content"]), agent, session, queue, agents);
                }
                "message"
                    if payload["role"] == "assistant" && payload["phase"] == "final_answer" =>
                {
                    turn.reply(&payload["content"])
                }
                "function_call" | "custom_tool_call" => {
                    if let Some(id) = payload["call_id"].as_str() {
                        let (name, input) = codex_call(payload);
                        turn.call(id, &name, &input);
                    }
                }
                "function_call_output" | "custom_tool_call_output" => {
                    if let Some(id) = payload["call_id"].as_str() {
                        turn.settle(id, &payload["output"], true);
                    }
                }
                _ => {}
            }
        }
    }

    /// Grok ACP completion includes usage and prompt id. Its events.jsonl
    /// turn_ended is a liveness signal, not a second model turn.
    fn grok(
        &mut self,
        value: &Value,
        at: Option<DateTime<Utc>>,
        root: &Path,
        agent: &Agent,
        session: &str,
        queue: &dyn PromptQueueStore,
        agents: &[Agent],
    ) {
        let params = &value["params"];
        if params["sessionId"]
            .as_str()
            .is_some_and(|id| id != self.source.harness_session)
        {
            return;
        }
        let update = &params["update"];
        let kind = update["sessionUpdate"].as_str().unwrap_or("");
        let meta = &params["_meta"];
        if !matches!(
            kind,
            "user_message_chunk"
                | "agent_message_chunk"
                | "tool_call"
                | "tool_call_update"
                | "turn_completed"
        ) {
            return;
        }
        if self.pending.is_none() {
            let id = update["prompt_id"]
                .as_str()
                .or_else(|| meta["promptId"].as_str())
                .unwrap_or("pending");
            let start = meta["turnStartMs"]
                .as_i64()
                .and_then(DateTime::from_timestamp_millis)
                .or_else(|| {
                    at.zip(update["elapsed_ms"].as_i64())
                        .map(|(at, ms)| at - chrono::Duration::milliseconds(ms))
                })
                .or(at);
            self.pending = Some(Pending::new(id, start, (0, 0)));
        }
        let Some(turn) = &mut self.pending else {
            return;
        };
        if let Some(id) = meta["promptId"].as_str() {
            turn.id = id.to_owned();
        }
        if let Some(start) = meta["turnStartMs"]
            .as_i64()
            .and_then(DateTime::from_timestamp_millis)
        {
            turn.started = Some(start);
        }
        match kind {
            "user_message_chunk" if turn.wake.is_none() => {
                turn.attribute(
                    update["content"]["text"].as_str().unwrap_or(""),
                    agent,
                    session,
                    queue,
                    agents,
                );
            }
            "agent_message_chunk" if turn.may_answer => {
                // Bounded streaming ack detection; discarded at completion.
                let chunk = update["content"]["text"].as_str().unwrap_or("");
                if turn.reply.len() + chunk.len() <= 256 {
                    turn.reply.push_str(chunk);
                } else {
                    turn.actions.insert(TurnAction::Answer);
                }
            }
            "tool_call" | "tool_call_update" => {
                if let Some(id) = update["toolCallId"].as_str() {
                    if let Some(name) = update["_meta"]["x.ai/tool"]["name"].as_str() {
                        turn.call(id, name, &update["rawInput"]);
                    }
                    match update["status"].as_str() {
                        Some("completed") => turn.settle(id, &update["rawOutput"], true),
                        Some("failed" | "cancelled") => {
                            turn.calls.remove(id);
                        }
                        _ => {}
                    }
                }
            }
            "turn_completed" => {
                let Some(id) = update["prompt_id"]
                    .as_str()
                    .or_else(|| meta["promptId"].as_str())
                else {
                    return;
                };
                turn.id = id.to_owned();
                turn.usage = update["usage"]["inputTokens"]
                    .as_u64()
                    .zip(update["usage"]["outputTokens"].as_u64());
                let reply = Value::String(std::mem::take(&mut turn.reply));
                turn.reply(&reply);
                self.complete(at, root);
            }
            _ => {}
        }
    }

    fn refresh(
        &mut self,
        root: &Path,
        agent: &Agent,
        session: &str,
        queue: &dyn PromptQueueStore,
        agents: &[Agent],
    ) -> std::io::Result<()> {
        self.flush(root);
        // Backpressure bounds memory on a temporarily unavailable local store.
        if self.outbox.len() >= 512 {
            return Ok(());
        }
        let mut file = std::fs::File::open(&self.path)?;
        let metadata = file.metadata()?;
        let created = metadata.created().ok();
        let modified = metadata.modified().ok();
        if self.created != created
            || metadata.len() < self.offset
            || (metadata.len() == self.offset && self.modified != modified)
        {
            self.offset = 0;
            self.pending = None;
            self.usage = (0, 0);
        }
        #[cfg(test)]
        if let Some(hook) = self.before_read.take() {
            hook(&self.path);
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        // Bound one off-loop pass; resume exactly where this pass stops.
        let mut read = 0;
        while read < 8 * 1024 * 1024 && self.outbox.len() < 512 {
            line.clear();
            let count = reader.read_line(&mut line)?;
            if count == 0 || !line.ends_with('\n') {
                break;
            }
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                self.line(&value, root, agent, session, queue, agents);
            }
            #[cfg(test)]
            {
                self.bytes_read += count as u64;
            }
            self.offset += count as u64;
            read += count;
        }
        // Appends may have been consumed after the initial metadata snapshot.
        // Cache metadata from the file just read, so the next idle poll cannot
        // mistake that append for a same-length rewrite and replay the file.
        let after = reader.get_ref().metadata()?;
        self.created = after.created().ok();
        self.modified = after.modified().ok();
        Ok(())
    }
}

impl Scanner {
    #[cfg(test)]
    pub(super) fn bytes_read(&self) -> u64 {
        self.cursors.values().map(|cursor| cursor.bytes_read).sum()
    }
    pub(super) fn scan(
        &mut self,
        root: &Path,
        project: &Path,
        session: &str,
        queue: &dyn PromptQueueStore,
        agents: &[Agent],
    ) {
        let scope = WorkEventScope::from_agents(session, agents);
        let sources = scope
            .supervisors
            .into_iter()
            .chain(scope.workers)
            .collect::<Vec<_>>();
        self.cursors
            .retain(|id, _| sources.iter().any(|source| source.agent_id == *id));
        self.misses
            .retain(|id, _| sources.iter().any(|source| source.agent_id == *id));
        for source in sources {
            if !matches!(
                source.cli,
                cas_mux::SupervisorCli::Claude
                    | cas_mux::SupervisorCli::Codex
                    | cas_mux::SupervisorCli::Grok
            ) {
                continue;
            }
            let Some(agent) = agents.iter().find(|a| a.id == source.agent_id) else {
                continue;
            };
            if self
                .cursors
                .get(&source.agent_id)
                .is_some_and(|c| c.source != source || !c.path.is_file())
            {
                self.cursors.remove(&source.agent_id);
            }
            if !self.cursors.contains_key(&source.agent_id) {
                if self
                    .misses
                    .get(&source.agent_id)
                    .is_some_and(|(prior, until)| prior == &source && Instant::now() < *until)
                {
                    continue;
                }
                let synthetic = source.cli == cas_mux::SupervisorCli::Codex
                    && source.harness_session.starts_with("codex-");
                let path = if synthetic {
                    let startup_id = queue
                        .latest_from_source_to_targets(
                            super::queue_and_events::SUPERVISOR_STARTUP_SOURCE,
                            &[&source.agent_name],
                            Some(session),
                        )
                        .ok()
                        .flatten()
                        .filter(|r| r.origin == Some(QueueOrigin::Daemon))
                        .map(|r| r.id);
                    project.to_str().and_then(|project| {
                        crate::mcp::tools::service::factory_ops::resolve_codex_mirror_transcript(
                            project,
                            source.account_dir.as_deref(),
                            &|path| {
                                codex_rollout_owns_recipient(
                                    path,
                                    queue,
                                    session,
                                    &source.agent_name,
                                    startup_id,
                                    agent.role == AgentRole::Supervisor,
                                    &self.ownership,
                                )
                            },
                        )
                    })
                } else if let Some(account) = source.account_dir.as_deref() {
                    // The generic worker resolver honors named accounts only
                    // for Codex. Bind this scanner to every explicit account.
                    let directory = if source.cli == cas_mux::SupervisorCli::Claude {
                        "projects"
                    } else {
                        "sessions"
                    };
                    crate::mcp::tools::service::factory_ops::transcript_path_from_resolution(
                        crate::mcp::tools::service::factory_ops::resolve_transcript_in_roots(
                            &[PathBuf::from(account).join(directory)],
                            project.to_str(),
                            &source.harness_session,
                            source.cli,
                        ),
                    )
                } else {
                    crate::mcp::tools::service::factory_ops::resolve_worker_transcript_path_for_account(
                        project.to_str(), &source.harness_session, source.cli, None)
                };
                let Some(path) = path else {
                    self.misses.insert(
                        source.agent_id.clone(),
                        (source, Instant::now() + Duration::from_secs(30)),
                    );
                    continue;
                };
                self.misses.remove(&source.agent_id);
                self.cursors
                    .insert(source.agent_id.clone(), Cursor::new(source.clone(), path));
            }
            if let Some(cursor) = self.cursors.get_mut(&source.agent_id) {
                #[cfg(test)]
                {
                    cursor.before_read = self.before_read.take();
                }
                if let Err(error) = cursor.refresh(root, agent, session, queue, agents) {
                    tracing::debug!(%error, "turn ledger transcript read deferred");
                }
            }
        }
    }
}
