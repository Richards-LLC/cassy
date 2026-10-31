use std::collections::HashMap;

use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, GetPromptRequestParams, GetPromptResult, Implementation,
    ListPromptsResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ProtocolVersion, ReadResourceRequestParams, ReadResourceResult, ResourceContents,
    ServerCapabilities, ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use tracing::{info, warn};

use crate::mcp::server::CasCore;
use crate::mcp::tools::service::CasService;

/// Always injected by the client, and cut at 2 KB by Claude Code: keep it to
/// routing guidance, well under 200 characters.
pub(crate) const SERVER_INSTRUCTIONS: &str = "Cassy: tasks, memory, search and factory coordination. If these tools are deferred, load task, coordination, search and memory together before first use.";

const WORKER_MCP_ACTIVITY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Reserve before any identity/store lookup, without holding a mutex across
/// SQLite. Concurrent clones skip an in-progress attempt; a failed attempt
/// releases its reservation so registration or transient errors can recover.
struct WorkerMcpActivityPermit<'a> {
    state: &'a std::sync::Mutex<crate::mcp::server::WorkerMcpActivityState>,
    recorded: bool,
    not_worker: bool,
}

impl<'a> WorkerMcpActivityPermit<'a> {
    fn acquire(core: &'a CasCore) -> Option<Self> {
        let mut state = core.worker_mcp_activity.lock().ok()?;
        if state.not_worker || state.recording || state.last_recorded_at.is_some_and(|at| at.elapsed() < WORKER_MCP_ACTIVITY_INTERVAL) {
            return None;
        }
        state.recording = true;
        Some(Self { state: &core.worker_mcp_activity, recorded: false, not_worker: false })
    }
}

impl Drop for WorkerMcpActivityPermit<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            if self.recorded { state.last_recorded_at = Some(std::time::Instant::now()); }
            if self.not_worker { state.not_worker = true; }
            state.recording = false;
        }
    }
}

/// Activity belongs to this server's authenticated caller, never a subject
/// named by a tool payload. Heartbeat/registration telemetry is not progress.
fn record_worker_mcp_activity(core: &CasCore, tool: &str, action: &str) {
    use crate::mcp::server::AgentIdentitySource;
    use cas_types::{AgentRole, AgentStatus, Event, EventEntityType, EventType};
    if matches!(tool, "coordination" | "agent")
        && matches!(action, "heartbeat" | "register" | "unregister" | "session_start" | "session_end")
    {
        return;
    }
    let Some(mut permit) = WorkerMcpActivityPermit::acquire(core) else { return };
    let already_bound = core.agent_id.get().is_some();
    let Ok(id) = core.get_registered_agent_id_read_only() else { return };
    // A first request can resolve the registered server session from its
    // environment/PID mapping. Public registration must never rebind it.
    if !already_bound {
        let _ = core.bind_agent_identity(id.clone(), AgentIdentitySource::ServerInternal);
    }
    if !core.has_server_internal_identity(&id) { return; }
    let Ok(agents) = core.open_agent_store() else { return };
    let Ok(agent) = agents.get(&id) else { return };
    if agent.role != AgentRole::Worker {
        // Standard is a bootstrap role: registration can reconcile the same
        // caller to Worker after this request. Only stable factory roles cache.
        permit.not_worker = matches!(agent.role, AgentRole::Supervisor | AgentRole::Director);
        return;
    }
    if !matches!(agent.status, AgentStatus::Active | AgentStatus::Idle) {
        return;
    }
    if let Ok(events) = crate::store::open_event_store(&core.cas_root) {
        let event = Event::new(EventType::WorkerActivity, EventEntityType::Agent, &id,
            format!("MCP activity: {tool} {action}"))
            .with_session(&id)
            .with_metadata(serde_json::json!({"source":"mcp_dispatch", "tool":tool, "action":action}));
        match events.record(&event) {
            Ok(_) => permit.recorded = true,
            Err(error) => tracing::debug!(%error, "worker MCP activity could not be recorded"),
        }
    }
}

#[allow(clippy::manual_async_fn)]
impl ServerHandler for CasService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::LATEST,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_resources_list_changed()
                .enable_prompts()
                .build(),
            server_info: Implementation {
                name: "cas".to_string(),
                title: Some("Coding Agent System".to_string()),
                description: Some("Unified context system for AI agents: persistent memory, tasks, rules, and skills across sessions.".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
                icons: None,
                website_url: None,
            },
            instructions: Some(SERVER_INSTRUCTIONS.to_string()),
        }
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourcesResult, rmcp::ErrorData>> + Send + '_
    {
        async move {
            let start = std::time::Instant::now();
            info!(method = "resources/list", "MCP resources/list START");
            if let Ok(mut peer_guard) = self.inner.peer.write() {
                if peer_guard.is_none() {
                    *peer_guard = Some(context.peer.clone());
                }
            }

            let resources = self.inner.build_resources();
            info!(
                method = "resources/list",
                count = resources.len(),
                elapsed_ms = start.elapsed().as_millis() as u64,
                "MCP resources/list DONE"
            );
            Ok(ListResourcesResult {
                resources,
                next_cursor: None,
                meta: None,
            })
        }
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ReadResourceResult, rmcp::ErrorData>> + Send + '_
    {
        async move {
            let content = self.inner.read_resource_content(&request.uri)?;
            Ok(ReadResourceResult {
                contents: vec![ResourceContents::text(content, &request.uri)],
            })
        }
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListPromptsResult, rmcp::ErrorData>> + Send + '_
    {
        async move {
            Ok(ListPromptsResult {
                prompts: CasCore::build_prompts(),
                next_cursor: None,
                meta: None,
            })
        }
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetPromptResult, rmcp::ErrorData>> + Send + '_
    {
        async move {
            let args: HashMap<String, String> = request
                .arguments
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
                .collect();
            self.inner.get_prompt_content(&request.name, &args)
        }
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, rmcp::ErrorData>> + Send + '_
    {
        async move {
            let start = std::time::Instant::now();
            info!(method = "tools/list", "MCP tools/list START");
            let tools = self.tool_definitions();
            info!(
                method = "tools/list",
                count = tools.len(),
                elapsed_ms = start.elapsed().as_millis() as u64,
                "MCP tools/list DONE"
            );

            Ok(ListToolsResult {
                tools,
                meta: None,
                next_cursor: None,
            })
        }
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<rmcp::model::CallToolResult, rmcp::ErrorData>>
    + Send
    + '_ {
        async move {
            let start = std::time::Instant::now();
            let tool_name = request.name.clone();
            let request_id = format!("{}", context.id);
            // Keep enough of the request to distinguish a committed task close
            // from an unknown write when the response deadline wins the race.
            // `ToolCallContext` consumes `request` below.
            let timeout_arguments = request.arguments.clone();
            let caller_tool_use =
                claude_tool_use_id(request.meta.as_ref()).or_else(|| claude_tool_use_id(Some(&context.meta)));
            info!(method = "tools/call", tool = %tool_name, id = %request_id, "MCP call_tool START");
            // cas-b1dd: the handler owns its service clone so it can outlive an
            // early answer; a deadline never drops work past its commit.
            let service = self.clone();
            let handler = tracing::Instrument::instrument(
                async move {
                    let tcc = rmcp::handler::server::tool::ToolCallContext::new(
                        &service, request, context,
                    );
                    service.tool_router.call(tcc).await
                },
                tracing::Span::current(),
            );

            let budget = std::time::Duration::from_secs(55);
            let result = self
                .call_with_deadline_and_grace(
                    &tool_name,
                    timeout_arguments.as_ref(),
                    budget,
                    POST_COMMIT_GRACE,
                    handler,
                )
                .await;
            let remaining = budget.saturating_sub(start.elapsed());
            self.append_factory_context_for_caller(
                result,
                remaining.min(std::time::Duration::from_millis(500)),
                caller_tool_use,
            )
            .await
        }
    }
}

/// cas-badb (GH #1302): Claude Code tags every MCP call with the id of the
/// `tool_use` that issued it. Subagents share the parent's MCP server, so this
/// id is the only per-call evidence of who is calling.
fn claude_tool_use_id(meta: Option<&rmcp::model::Meta>) -> Option<String> {
    meta?
        .get("claudecode/toolUseId")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// Whether `tool_use_id` was issued by the session's own top-level turn: a
/// non-sidechain assistant record in its main transcript. A subagent's calls
/// are written to `<session>/subagents/` (or, in older builds, as sidechain
/// records), so they never match. Claude flushes the record about 50-100 ms
/// after the call starts; wait up to `wait` for it.
fn top_level_tool_use(transcript: &std::path::Path, tool_use_id: &str, wait: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + wait;
    loop {
        if transcript_tail_has_top_level_tool_use(transcript, tool_use_id) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn transcript_tail_has_top_level_tool_use(transcript: &std::path::Path, tool_use_id: &str) -> bool {
    const TAIL: u64 = 512 * 1024;
    let Some((_, tail)) = super::worker_liveness::read_jsonl_tail(transcript, TAIL) else {
        return false;
    };
    tail.lines()
        .filter(|line| line.contains(tool_use_id))
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .any(|record| {
            record["type"] == "assistant"
                && record["isSidechain"] != true
                && record["message"]["content"].as_array().is_some_and(|content| {
                    content
                        .iter()
                        .any(|block| block["type"] == "tool_use" && block["id"] == tool_use_id)
                })
        })
}

/// This MCP process belongs to the caller's cwd/account. A supervisor has no
/// worker clone, so worker-status's conventional worktree path is unsuitable.
fn caller_transcript_path(agent: &crate::types::Agent) -> Option<std::path::PathBuf> {
    if super::factory_ops::worker_cli_from_agent(agent) != cas_mux::SupervisorCli::Claude {
        return None;
    }
    let cwd = std::env::current_dir().ok()?;
    let slug: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if matches!(c, '/' | '.') { '-' } else { c })
        .collect();
    let account = agent
        .metadata
        .get("worker_account_dir")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from))
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))?;
    let session = agent.cc_session_id.as_deref().unwrap_or(&agent.id);
    let path = account
        .join("projects")
        .join(slug)
        .join(format!("{session}.jsonl"));
    path.is_file().then_some(path)
}

/// cas-b1dd: once a request's terminal task mutation has committed (a close's
/// Closed write, a note append), the caller is answered after at most this
/// much more post-commit work; the rest finishes in the background.
const POST_COMMIT_GRACE: std::time::Duration = std::time::Duration::from_secs(15);

type ToolResult = Result<rmcp::model::CallToolResult, rmcp::ErrorData>;

/// cas-3b81: the note label carrying a late close's final outcome.
const LATE_CLOSE_NOTE_MARKER: &str = "CLOSE_OUTCOME";
/// cas-3937: marks a task note recording work that finished after a
/// COMMITTED early answer.
const POST_COMMIT_NOTE_MARKER: &str = "POST_COMMIT_OUTCOME";

/// First line of a late close's answer, bounded for a task note.
fn late_close_summary(joined: &Result<ToolResult, tokio::task::JoinError>) -> String {
    let text = match joined {
        Ok(Ok(result)) => {
            let text = result
                .content
                .iter()
                .find_map(|content| match &content.raw {
                    rmcp::model::RawContent::Text(text) => Some(text.text.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            if result.is_error == Some(true) {
                format!("refused: {text}")
            } else {
                text
            }
        }
        Ok(Err(error)) => format!("error: {}", error.message),
        Err(error) => format!("handler ended abnormally: {error}"),
    };
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("(no output)");
    let mut summary: String = line.chars().take(300).collect();
    if line.chars().count() > 300 {
        summary.push('…');
    }
    summary
}

/// Post-commit duration of a request, logged when it is long enough to matter
/// (cas-b1dd): the stage the 55s deadline used to hide.
fn log_post_commit(tool: &str, receipt: &super::mutation_receipt::Receipt, how: &str) {
    if let Some(at) = receipt.terminal.get() {
        let post_commit = at.elapsed();
        if post_commit.as_millis() >= 1_000 {
            info!(tool, action = receipt.action(), task_id = receipt.task_id().unwrap_or_default(), post_commit_ms = post_commit.as_millis() as u64, how, "MCP post-commit work");
        }
    }
}

impl CasService {
    /// cas-3937: a factory request run through the MCP response budget
    /// and post-commit grace, as a real tool call is, so integration tests
    /// can drive slow post-commit work through the real handler.
    #[doc(hidden)]
    pub async fn factory_with_budget_for_testing(
        &self,
        req: cas_mcp::types::CoordinationRequest,
        budget: std::time::Duration,
        grace: std::time::Duration,
    ) -> ToolResult {
        let arguments = serde_json::to_value(&req)
            .ok()
            .and_then(|value| value.as_object().cloned());
        let service = self.clone();
        self.call_with_deadline_and_grace(
            "factory",
            arguments.as_ref(),
            budget,
            grace,
            async move {
                service
                    .factory(rmcp::handler::server::wrapper::Parameters(req))
                    .await
            },
        )
        .await
    }

    /// The deadline alone, with no early post-commit answer.
    #[cfg(test)]
    pub(crate) async fn call_with_deadline<F>(
        &self,
        tool_name: &str,
        arguments: Option<&serde_json::Map<String, serde_json::Value>>,
        budget: std::time::Duration,
        future: F,
    ) -> ToolResult
    where
        F: std::future::Future<Output = ToolResult> + Send + 'static,
    {
        let no_grace = budget.saturating_add(std::time::Duration::from_secs(3600));
        self.call_with_deadline_and_grace(tool_name, arguments, budget, no_grace, future)
            .await
    }

    /// Run a tool handler under the response budget (cas-b1dd).
    ///
    /// The handler runs as its own task. The close path's post-commit stages
    /// (search indexing, reminders, the lifecycle outbox, dependents, leases,
    /// DB-branch teardown) can each block on a cross-process lock or the
    /// network; the response used to wait for all of them, and the 55s
    /// deadline then dropped that work half-done. Now:
    /// - the handler finishing first returns its result, as before;
    /// - once the request's terminal mutation has committed, post-commit work
    ///   that outlives `grace` gets a COMMITTED success answer, and the work
    ///   keeps running to completion (one mutation, no retry invited);
    /// - the budget elapsing first returns the error it always did (COMMITTED
    ///   or UNKNOWN) and cancels the handler only when nothing committed.
    async fn call_with_deadline_and_grace<F>(
        &self,
        tool_name: &str,
        arguments: Option<&serde_json::Map<String, serde_json::Value>>,
        budget: std::time::Duration,
        grace: std::time::Duration,
        future: F,
    ) -> ToolResult
    where
        F: std::future::Future<Output = ToolResult> + Send + 'static,
    {
        let action = arguments
            .and_then(|args| args.get("action"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        // Before spawning the handler: a call in flight is already activity.
        // This universal boundary also covers factory and proxy tool calls.
        record_worker_mcp_activity(&self.inner, tool_name, action);
        let task_id = arguments
            .and_then(|args| args.get("id"))
            .and_then(serde_json::Value::as_str);
        let receipt = super::mutation_receipt::Receipt::new(tool_name, action, task_id);
        let start = std::time::Instant::now();
        let mut handle = tokio::spawn(super::mutation_receipt::scope(receipt.clone(), async move {
            #[cfg(all(target_os = "linux", target_env = "gnu"))]
            let _memory_call = crate::mcp::server::memory::Call::started();
            future.await
        }));
        let deadline = tokio::time::Instant::from_std(start + budget);
        let post_commit_grace = {
            let receipt = receipt.clone();
            async move {
                loop {
                    if let Some(at) = receipt.terminal.get() {
                        tokio::time::sleep_until(tokio::time::Instant::from_std(*at + grace)).await;
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            }
        };
        enum Won {
            Handler(Result<ToolResult, tokio::task::JoinError>),
            Grace,
            Deadline,
        }
        let won = tokio::select! {
            biased;
            joined = &mut handle => Won::Handler(joined),
            // The budget outranks the grace when both are due in one tick.
            () = tokio::time::sleep_until(deadline) => Won::Deadline,
            () = post_commit_grace => Won::Grace,
        };
        match won {
            Won::Handler(joined) => {
                let elapsed = start.elapsed();
                if elapsed.as_secs() >= 5 {
                    info!(tool = tool_name, elapsed_ms = elapsed.as_millis() as u64, "MCP slow request");
                }
                log_post_commit(tool_name, &receipt, "returned");
                match joined {
                    Ok(result) => result,
                    Err(error) => Err(rmcp::ErrorData {
                        code: rmcp::model::ErrorCode::INTERNAL_ERROR,
                        message: format!("Tool '{tool_name}' handler ended abnormally: {error}").into(),
                        data: None,
                    }),
                }
            }
            Won::Grace => {
                let elapsed = start.elapsed();
                let description = receipt
                    .commit
                    .get()
                    .map(|commit| commit.description.clone())
                    .unwrap_or_else(|| "write committed".to_string());
                warn!(tool = tool_name, action = receipt.action(), task_id = receipt.task_id().unwrap_or_default(), elapsed_ms = elapsed.as_millis() as u64, grace_ms = grace.as_millis() as u64, "MCP answered after commit; post-commit work continues");
                if receipt.action() != "worktree_merge" {
                    let watcher_receipt = receipt.clone();
                    let tool = tool_name.to_string();
                    tokio::spawn(async move {
                        let _ = handle.await;
                        log_post_commit(&tool, &watcher_receipt, "finished in the background");
                    });
                } else {
                    // cas-3937 review: the work after the early answer (a
                    // merge's close) must not finish where nobody sees it.
                    self.record_post_commit_outcome(handle, receipt.clone(), arguments, start);
                }
                // cas-3937: a factory merge is not a task mutation; name the
                // call itself, and its committed merge, instead.
                if tool_name != "task" {
                    return Ok(rmcp::model::CallToolResult::success(vec![rmcp::model::Content::text(format!(
                        "COMMITTED: `{tool_name} action={action}` is done ({description}). Its \
                         remaining work was still running {grace_s:.1}s after the commit and \
                         finishes in the background. Do not retry it; read the task's notes \
                         for the rest.",
                        action = receipt.action(),
                        grace_s = grace.as_secs_f64(),
                    ))]));
                }
                let task = receipt.task_id().map(|id| format!(" `{id}`")).unwrap_or_default();
                Ok(rmcp::model::CallToolResult::success(vec![rmcp::model::Content::text(format!(
                    "COMMITTED: task{task} {action} is done ({description}). Its post-commit work \
                     (search index, reminders, the lifecycle outbox, dependents, leases) was still \
                     running {grace_s:.1}s after the commit and finishes in the background. Do not \
                     retry; `task action=show` reflects the committed state.",
                    action = receipt.action(),
                    grace_s = grace.as_secs_f64(),
                ))]))
            }
            Won::Deadline => {
                let elapsed = start.elapsed();
                let commit = receipt.commit.get();
                // cas-24d8: a close commits intermediate writes (a gate note,
                // a parked anchor) long before its Closed write. Reporting
                // those as COMMITTED told the supervisor a close was done
                // while the task was still awaiting_merge. Only the terminal
                // write makes a close committed; until then it is running.
                //
                // cas-3b81 (GH #1142): the same holds when nothing has
                // committed yet. The close runs on the blocking pool and
                // cannot be cancelled, so "UNKNOWN" was followed by a late
                // Closed write or refusal nobody saw. It keeps running, and
                // its final outcome is recorded on the task as a note.
                let close_unfinished = tool_name == "task"
                    && receipt.action() == "close"
                    && receipt.terminal.get().is_none()
                    && receipt.task_id().is_some();
                if close_unfinished {
                    self.record_late_close_outcome(handle, receipt.clone(), start, budget);
                } else if commit.is_some() && receipt.action() == "worktree_merge" {
                    // cas-3937 review: committed, still running; keep its result.
                    self.record_post_commit_outcome(handle, receipt.clone(), arguments, start);
                } else if commit.is_none() {
                    // Nothing committed: cancel as before, so an UNKNOWN answer
                    // is not followed by a late write the caller never sees.
                    handle.abort();
                }
                let outcome = if close_unfinished && commit.is_some() {
                    format!(
                        "IN_PROGRESS (the close has not committed: only intermediate writes have, so the task's status is unchanged so far; the close keeps running in the background and records its final outcome as a {LATE_CLOSE_NOTE_MARKER} task note — re-query with task action=show before retrying)"
                    )
                } else if close_unfinished {
                    format!(
                        "IN_PROGRESS (nothing has committed yet, so the task's status is unchanged so far; the close keeps running in the background and records its final outcome as a {LATE_CLOSE_NOTE_MARKER} task note — re-query with task action=show before retrying)"
                    )
                } else {
                    self.mutation_timeout_outcome(tool_name, arguments, commit)
                };
                warn!(tool = tool_name, elapsed_ms = elapsed.as_millis() as u64, budget_ms = budget.as_millis() as u64, mutation_outcome = %outcome, "MCP response deadline elapsed");
                Err(rmcp::ErrorData {
                    code: rmcp::model::ErrorCode::INTERNAL_ERROR,
                    message: format!("Tool '{tool_name}' response deadline elapsed after {:.3}s (budget {:.3}s). Mutation outcome: {outcome}", elapsed.as_secs_f64(), budget.as_secs_f64()).into(),
                    data: Some(serde_json::json!({
                        "mutation_outcome": if close_unfinished { "IN_PROGRESS" } else if commit.is_some() { "COMMITTED" } else if potentially_mutating_call(tool_name, action) { "UNKNOWN" } else { "NOT_APPLICABLE" },
                        "notification_id": commit.and_then(|commit| commit.notification_id),
                        "elapsed_ms": elapsed.as_millis() as u64,
                        "budget_ms": budget.as_millis() as u64,
                    })),
                })
            }
        }
    }

    /// Preserve the tool's result and attach recoverable factory mail/recall.
    /// Only the registered process identity may read its inbox or transcript.
    #[cfg(test)]
    async fn append_factory_context(
        &self,
        result: Result<rmcp::model::CallToolResult, rmcp::ErrorData>,
    ) -> Result<rmcp::model::CallToolResult, rmcp::ErrorData> {
        self.append_factory_context_with_budget(result, std::time::Duration::from_millis(500))
            .await
    }

    /// The registered agent this MCP process speaks for, when it is the
    /// factory identity in this process's environment.
    fn registered_factory_agent(&self) -> Option<crate::types::Agent> {
        let id = self.inner.get_registered_agent_id_read_only().ok()?;
        let agent = self.inner.open_agent_store().ok()?.get(&id).ok()?;
        let name = std::env::var("CAS_AGENT_NAME").ok()?;
        (agent.name == name).then_some(agent)
    }

    /// cas-badb (GH #1302): the response fallback reads and acknowledges the
    /// registered session's own mail, so it runs only when the call came from
    /// that session's top-level turn. A Claude call tagged with a tool-use id
    /// that its top-level transcript does not show (a subagent, such as a
    /// task-verifier, sharing this server) neither surfaces nor consumes mail,
    /// and never receives mail deferred from an earlier response.
    async fn append_factory_context_for_caller(
        &self,
        result: Result<rmcp::model::CallToolResult, rmcp::ErrorData>,
        mut budget: std::time::Duration,
        caller_tool_use: Option<String>,
    ) -> Result<rmcp::model::CallToolResult, rmcp::ErrorData> {
        if result.is_ok() {
            if budget.is_zero() || crate::internal_llm::is_internal_invocation() {
                return result;
            }
            let started = std::time::Instant::now();
            let wait = (budget / 2).min(std::time::Duration::from_millis(300));
            let this = self.clone();
            let proven = tokio::task::spawn_blocking(move || {
                let Some(agent) = this.registered_factory_agent() else {
                    return false;
                };
                let Some(tool_use) = caller_tool_use else {
                    // A shared Claude server cannot attribute an untagged call
                    // to the parent. Leave its mail for a proven call or hook.
                    let claude = super::factory_ops::worker_cli_from_agent(&agent)
                        == cas_mux::SupervisorCli::Claude;
                    if claude {
                        tracing::debug!(agent_id = %agent.id, "untagged Claude call skipped parent mailbox");
                    }
                    return !claude;
                };
                caller_transcript_path(&agent)
                    .is_some_and(|path| top_level_tool_use(&path, &tool_use, wait))
            })
            .await
            .unwrap_or(false);
            if !proven {
                return result;
            }
            budget = budget.saturating_sub(started.elapsed());
        }
        self.append_factory_context_with_budget(result, budget)
            .await
    }

    async fn append_factory_context_with_budget(
        &self,
        result: Result<rmcp::model::CallToolResult, rmcp::ErrorData>,
        budget: std::time::Duration,
    ) -> Result<rmcp::model::CallToolResult, rmcp::ErrorData> {
        let Ok(mut output) = result else {
            return result;
        };
        if budget.is_zero() {
            return Ok(output);
        }
        // Concurrent responses never wait for another response's context lock.
        let Ok(mut pending) = self.factory_context.try_lock() else {
            return Ok(output);
        };
        if pending.is_none() {
            let this = self.clone();
            *pending = Some(tokio::task::spawn_blocking(move || {
                if crate::internal_llm::is_internal_invocation() {
                    return None;
                }
                let agent = this.registered_factory_agent()?;
                let role = match agent.role {
                    crate::types::AgentRole::Supervisor => "supervisor",
                    crate::types::AgentRole::Worker => "worker",
                    _ => return None,
                };
                let input = crate::hooks::HookInput {
                    session_id: agent.cc_session_id.clone().unwrap_or(agent.id.clone()),
                    agent_role: Some(role.into()),
                    transcript_path: caller_transcript_path(&agent)
                        .map(|path| path.to_string_lossy().into_owned()),
                    ..Default::default()
                };
                crate::hooks::turn_context::fallback_context(&this.inner.cas_root, &input)
            }));
        }
        let context = match tokio::time::timeout(budget, pending.as_mut().unwrap()).await {
            Ok(result) => {
                pending.take();
                result.ok().flatten()
            }
            Err(_) => {
                // Keep the handle: fallback_context may already have consumed mail.
                // Its completed output belongs on the next successful response.
                tracing::warn!("factory response context deferred; primary tool result preserved");
                None
            }
        };
        if let Some(context) = context {
            output.content.push(rmcp::model::Content::text(context));
        }
        Ok(output)
    }

    /// A response-layer timeout cancels the handler, but a synchronous store
    /// write may have committed just before that cancellation. Never leave a
    /// caller guessing whether retrying a mutating request would duplicate it.
    /// cas-3b81 (GH #1142): a close answered IN_PROGRESS at the response
    /// deadline keeps running; when it finishes, append its outcome and the
    /// task's resulting status to the task, so the caller can determine what
    /// happened with `task action=show` instead of guessing from "UNKNOWN".
    /// Appending is an atomic SQL append, so it cannot clobber the close's
    /// own row write.
    /// cas-3937 review: a non-task call that answered COMMITTED (a merge,
    /// after its push) keeps running. When it finishes, its result is
    /// appended to the task named by its `task_id` argument, so a close that
    /// a gate refused, or that failed, after the early answer is visible.
    fn record_post_commit_outcome(
        &self,
        handle: tokio::task::JoinHandle<ToolResult>,
        receipt: std::sync::Arc<super::mutation_receipt::Receipt>,
        arguments: Option<&serde_json::Map<String, serde_json::Value>>,
        start: std::time::Instant,
    ) {
        let task_id = arguments
            .and_then(|args| args.get("task_id"))
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .map(str::to_string);
        let service = self.clone();
        tokio::spawn(async move {
            let joined = handle.await;
            log_post_commit("factory", &receipt, "finished in the background");
            let Some(task_id) = task_id else {
                return;
            };
            // cas-3937 review: the close's own outcome, not the merge
            // answer's first line (always its CI block).
            let recorded_close = receipt
                .followup
                .lock()
                .ok()
                .and_then(|followup| followup.clone())
                .map(|text| {
                    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    crate::mcp::tools::truncate_str(&text, 400).to_string()
                });
            // cas-3fc8: a merge that fails after its close was recorded keeps
            // both: the close's outcome and the merge's own failure.
            let merge_failed = !matches!(&joined, Ok(Ok(result)) if result.is_error != Some(true));
            let followup = match recorded_close {
                Some(close) if merge_failed => {
                    format!("{close}. The merge itself then failed: {}", late_close_summary(&joined))
                }
                Some(close) => close,
                None => late_close_summary(&joined),
            };
            let action = receipt.action().to_string();
            let elapsed = start.elapsed();
            let recorded = tokio::task::spawn_blocking(move || {
                let store = service.inner.open_task_store().map_err(|error| error.message.to_string())?;
                let status = store
                    .get(&task_id)
                    .map(|task| task.status.to_string())
                    .unwrap_or_else(|error| format!("unreadable ({error})"));
                let note = format!(
                    "[{}] 📝 PROGRESS {POST_COMMIT_NOTE_MARKER}: `{action}` answered COMMITTED before its remaining work finished; that work finished after {:.1}s; task status now `{status}`. Close outcome: {followup}",
                    chrono::Utc::now().format("%Y-%m-%d %H:%M"),
                    elapsed.as_secs_f64(),
                );
                store
                    .append_note(&task_id, &note)
                    .map_err(|error| format!("{task_id}: {error}"))
            })
            .await;
            match recorded {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => warn!(error = %error, "MCP post-commit outcome not recorded"),
                Err(error) => warn!(error = %error, "MCP post-commit outcome recorder ended abnormally"),
            }
        });
    }

    fn record_late_close_outcome(
        &self,
        handle: tokio::task::JoinHandle<ToolResult>,
        receipt: std::sync::Arc<super::mutation_receipt::Receipt>,
        start: std::time::Instant,
        budget: std::time::Duration,
    ) {
        let Some(task_id) = receipt.task_id().map(str::to_string) else {
            return;
        };
        let service = self.clone();
        tokio::spawn(async move {
            let joined = handle.await;
            let elapsed = start.elapsed();
            let summary = late_close_summary(&joined);
            let recorded = tokio::task::spawn_blocking(move || {
                let store = service.inner.open_task_store().map_err(|error| error.message.to_string())?;
                let status = store
                    .get(&task_id)
                    .map(|task| task.status.to_string())
                    .unwrap_or_else(|error| format!("unreadable ({error})"));
                let note = format!(
                    "[{}] 📝 PROGRESS {LATE_CLOSE_NOTE_MARKER}: a close answered IN_PROGRESS at the {:.0}s response budget finished after {:.1}s; task status now `{status}`. Result: {summary}",
                    chrono::Utc::now().format("%Y-%m-%d %H:%M"),
                    budget.as_secs_f64(),
                    elapsed.as_secs_f64(),
                );
                store
                    .append_note(&task_id, &note)
                    .map(|_| task_id.clone())
                    .map_err(|error| format!("{task_id}: {error}"))
            })
            .await;
            match recorded {
                Ok(Ok(task_id)) => {
                    info!(task_id = %task_id, elapsed_ms = elapsed.as_millis() as u64, "MCP late close outcome recorded")
                }
                Ok(Err(error)) => warn!(error = %error, "MCP late close outcome not recorded"),
                Err(error) => {
                    warn!(error = %error, "MCP late close outcome recorder ended abnormally")
                }
            }
        });
    }

    fn mutation_timeout_outcome(
        &self,
        tool_name: &str,
        arguments: Option<&serde_json::Map<String, serde_json::Value>>,
        commit: Option<&super::mutation_receipt::Commit>,
    ) -> String {
        if let Some(commit) = commit {
            return format!("COMMITTED ({})", commit.description);
        }
        let action = arguments
            .and_then(|args| args.get("action"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();

        if !potentially_mutating_call(tool_name, action) {
            return "not applicable (the timed-out call was read-only)".to_string();
        }

        "UNKNOWN (the response deadline elapsed before Cassy could confirm a committed write; re-query state before retrying)".to_string()
    }
}

fn potentially_mutating_call(tool_name: &str, action: &str) -> bool {
    match tool_name {
        "task" => !matches!(
            action,
            "show" | "list" | "ready" | "blocked" | "dep_list" | "available" | "mine"
        ),
        "memory" => !matches!(action, "get" | "list" | "recent"),
        "factory" => !matches!(
            action,
            "worker_status"
                | "worker_activity"
                | "epic_status"
                | "gc_report"
                | "server_list"
                | "agent_list"
                | "lease_history"
                | "loop_status"
                | "queue_peek"
                | "worktree_list"
                | "worktree_show"
                | "worktree_status"
                | "db_branch_show"
        ),
        // cas-e477: `peers` only reads the cloud agent registry.
        "coordination" => !matches!(action, "show" | "list" | "status" | "members" | "peers"),
        "rule" | "skill" | "spec" | "verification" | "system" | "team" | "pattern"
        | "knowledge" | "artifact" => !matches!(action, "show" | "list" | "status" | "members"),
        // Unknown tool/action schemas must be treated as write-capable: an
        // optimistic "not committed" answer would invite a duplicate write.
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::potentially_mutating_call;

    #[test]
    fn cas_4e19_authenticated_non_worker_verdict_skips_store_access_across_clones() {
        use crate::mcp::server::{AgentIdentitySource, CasCore};
        let _env = crate::test_support::TestEnvGuard::temp_home();
        for role in [cas_types::AgentRole::Supervisor, cas_types::AgentRole::Director] {
            let dir = tempfile::TempDir::new().unwrap();
            let root = crate::store::init_cas_dir(dir.path()).unwrap();
            let core = CasCore::with_daemon(root.clone(), None, None);
            let supervisor = cas_types::Agent::new_with_role("cached-supervisor".into(), "cached-supervisor".into(), role);
            core.open_agent_store().unwrap().register(&supervisor).unwrap();
            core.bind_agent_identity(supervisor.id.clone(), AgentIdentitySource::ServerInternal).unwrap();
            super::record_worker_mcp_activity(&core, "task", "notes");
            let before = cas_store::wait_budget::store_access_violations();
            {
                let _forbid = cas_store::wait_budget::forbid_store_access();
                for _ in 0..16 {
                    super::record_worker_mcp_activity(&core.clone(), "task", "notes");
                }
            }
            assert_eq!(cas_store::wait_budget::store_access_violations(), before,
                "a confirmed non-worker core must not read the store again");
            assert!(crate::store::open_event_store(&root).unwrap().list_by_type(cas_types::EventType::WorkerActivity, 100).unwrap().is_empty());

            let worker_core = CasCore::with_daemon(root.clone(), None, None);
            let worker = cas_types::Agent::new_with_role("independent-worker".into(), "independent-worker".into(), cas_types::AgentRole::Worker);
            worker_core.open_agent_store().unwrap().register(&worker).unwrap();
            worker_core.bind_agent_identity(worker.id.clone(), AgentIdentitySource::ServerInternal).unwrap();
            super::record_worker_mcp_activity(&worker_core, "task", "notes");
            assert!(crate::store::open_event_store(&root).unwrap().latest_worker_activity(&worker.id, &worker.name).unwrap().is_some(),
                "a non-worker verdict belongs only to that immutable caller core");
        }
    }

    #[test]
    fn cas_4e19_inactive_worker_verdict_is_retryable() {
        use crate::mcp::server::{AgentIdentitySource, CasCore};
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::TempDir::new().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        let mut worker = cas_types::Agent::new_with_role("recovering-worker".into(), "recovering-worker".into(), cas_types::AgentRole::Worker);
        worker.status = cas_types::AgentStatus::Shutdown;
        core.open_agent_store().unwrap().register(&worker).unwrap();
        core.bind_agent_identity(worker.id.clone(), AgentIdentitySource::ServerInternal).unwrap();
        super::record_worker_mcp_activity(&core, "task", "notes");
        assert!(crate::store::open_event_store(&root).unwrap().latest_worker_activity(&worker.id, &worker.name).unwrap().is_none());
        worker.status = cas_types::AgentStatus::Active;
        core.open_agent_store().unwrap().update(&worker).unwrap();
        super::record_worker_mcp_activity(&core, "task", "notes");
        assert!(crate::store::open_event_store(&root).unwrap().latest_worker_activity(&worker.id, &worker.name).unwrap().is_some(),
            "inactive workers must not be cached as non-workers");
    }

    #[test]
    fn cas_4e19_standard_to_worker_reconciliation_records_activity() {
        use crate::mcp::server::{AgentIdentitySource, CasCore};
        use cas_types::{Agent, AgentRole};
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::TempDir::new().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        let caller = Agent::new("bootstrap-worker".into(), "bootstrap-worker".into());
        core.open_agent_store().unwrap().register(&caller).unwrap();
        core.bind_agent_identity(caller.id.clone(), AgentIdentitySource::ServerInternal).unwrap();
        super::record_worker_mcp_activity(&core, "task", "notes");
        let events = crate::store::open_event_store(&root).unwrap();
        assert!(events.latest_worker_activity(&caller.id, &caller.name).unwrap().is_none());

        core.register_agent_with_hints(caller.id.clone(), caller.name.clone(), None, None,
            Some(AgentRole::Worker), AgentIdentitySource::ServerInternal).unwrap();
        assert_eq!(core.open_agent_store().unwrap().get(&caller.id).unwrap().role, AgentRole::Worker);
        super::record_worker_mcp_activity(&core.clone(), "task", "notes");
        assert!(events.latest_worker_activity(&caller.id, &caller.name).unwrap().is_some(),
            "a hook-created Standard row corrected to Worker must record MCP activity on the same core");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cas_6eec_mcp_burst_records_one_activity_across_service_clones() {
        use crate::mcp::server::{CasCore, AgentIdentitySource};
        use crate::mcp::tools::service::CasService;
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::TempDir::new().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        let worker = cas_types::Agent::new_with_role("burst-worker".into(), "burst-worker".into(), cas_types::AgentRole::Worker);
        core.open_agent_store().unwrap().register(&worker).unwrap();
        core.bind_agent_identity(worker.id.clone(), AgentIdentitySource::ServerInternal).unwrap();
        let service = CasService::new(core, #[cfg(feature = "mcp-proxy")] None);
        let calls = (0..16).map(|_| {
            let service = service.clone();
            tokio::spawn(async move {
                service.call_with_deadline_and_grace("task", None, std::time::Duration::from_secs(5),
                    std::time::Duration::from_secs(5), async { Ok(rmcp::model::CallToolResult::success(vec![])) }).await.unwrap();
            })
        });
        for call in futures::future::join_all(calls).await { call.unwrap(); }
        let events = crate::store::open_event_store(&root).unwrap();
        let rows = events.list_by_type(cas_types::EventType::WorkerActivity, 100).unwrap();
        assert_eq!(rows.len(), 1, "rapid concurrent MCP calls must share one throttled activity row");
        service.inner.worker_mcp_activity.lock().unwrap().last_recorded_at =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(31));
        service.call_with_deadline_and_grace("task", None, std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(5), async { Ok(rmcp::model::CallToolResult::success(vec![])) }).await.unwrap();
        assert_eq!(events.list_by_type(cas_types::EventType::WorkerActivity, 100).unwrap().len(), 2,
            "activity resumes when the 30-second interval expires");
    }

    #[test]
    fn cas_6eec_activity_throttle_releases_failed_attempts_and_inflight_reservations() {
        use crate::mcp::server::{CasCore, AgentIdentitySource};
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::TempDir::new().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        core.bind_agent_identity("late-worker".into(), AgentIdentitySource::ServerInternal).unwrap();
        super::record_worker_mcp_activity(&core, "task", "notes");
        assert!(core.worker_mcp_activity.lock().unwrap().last_recorded_at.is_none());
        let worker = cas_types::Agent::new_with_role("late-worker".into(), "late-worker".into(), cas_types::AgentRole::Worker);
        core.open_agent_store().unwrap().register(&worker).unwrap();
        let permit = super::WorkerMcpActivityPermit::acquire(&core).unwrap();
        assert!(super::WorkerMcpActivityPermit::acquire(&core.clone()).is_none(), "inflight reservation is shared");
        drop(permit);
        super::record_worker_mcp_activity(&core, "task", "notes");
        assert_eq!(crate::store::open_event_store(&root).unwrap().list_by_type(cas_types::EventType::WorkerActivity, 100).unwrap().len(), 1,
            "failed attempts must not silence a now-registered worker for 30s");
    }

    #[tokio::test]
    async fn cas_6eec_activity_rejects_payload_identity_and_heartbeat_or_public_registration() {
        use crate::mcp::server::{CasCore, AgentIdentitySource};
        use crate::mcp::tools::service::CasService;
        use cas_types::{Agent, AgentRole, AgentStatus};
        let _env = crate::test_support::TestEnvGuard::temp_home();
        for (index, (role, status, source, tool, action, expected)) in [
            (AgentRole::Worker, AgentStatus::Active, AgentIdentitySource::ServerInternal, "task", "notes", true),
            (AgentRole::Supervisor, AgentStatus::Active, AgentIdentitySource::ServerInternal, "task", "notes", false),
            (AgentRole::Worker, AgentStatus::Active, AgentIdentitySource::PublicRegistration, "task", "notes", false),
            (AgentRole::Worker, AgentStatus::Shutdown, AgentIdentitySource::ServerInternal, "task", "notes", false),
            (AgentRole::Worker, AgentStatus::Active, AgentIdentitySource::ServerInternal, "coordination", "heartbeat", false),
            (AgentRole::Worker, AgentStatus::Active, AgentIdentitySource::ServerInternal, "coordination", "register", false),
        ].into_iter().enumerate() {
            let dir = tempfile::TempDir::new().unwrap();
            let root = crate::store::init_cas_dir(dir.path()).unwrap();
            let core = CasCore::with_daemon(root.clone(), None, None);
            let mut caller = Agent::new_with_role(format!("caller-{index}"), format!("caller-{index}"), role);
            caller.status = status;
            core.open_agent_store().unwrap().register(&caller).unwrap();
            core.bind_agent_identity(caller.id.clone(), source).unwrap();
            let victim = Agent::new_with_role(format!("victim-{index}"), format!("victim-{index}"), AgentRole::Worker);
            core.open_agent_store().unwrap().register(&victim).unwrap();
            let args = serde_json::json!({"action": action, "id": victim.id, "target": victim.name, "session_id": victim.id});
            let service = CasService::new(core, #[cfg(feature = "mcp-proxy")] None);
            service.call_with_deadline_and_grace(tool, args.as_object(), std::time::Duration::from_secs(5),
                std::time::Duration::from_secs(5), async { Ok(rmcp::model::CallToolResult::success(vec![])) }).await.unwrap();
            let events = crate::store::open_event_store(&root).unwrap();
            assert_eq!(events.latest_worker_activity(&caller.id, &caller.name).unwrap().is_some(), expected, "case {index}");
            assert!(events.latest_worker_activity(&victim.id, &victim.name).unwrap().is_none(), "payload must not spoof another worker's activity");
        }
    }

    #[tokio::test]
    async fn cas_6eec_mcp_dispatch_records_its_authenticated_workers_activity() {
        use crate::mcp::tools::service::CasService;
        use rmcp::model::CallToolResult;
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::TempDir::new().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        let core = crate::mcp::server::CasCore::with_daemon(root.clone(), None, None);
        let worker = cas_types::Agent::new_with_role("worker-session".into(), "worker".into(), cas_types::AgentRole::Worker);
        core.open_agent_store().unwrap().register(&worker).unwrap();
        core.bind_agent_identity(worker.id.clone(), crate::mcp::server::AgentIdentitySource::ServerInternal).unwrap();
        let events = crate::store::open_event_store(&root).unwrap();
        let worker_id = worker.id.clone();
        let service = CasService::new(core, #[cfg(feature = "mcp-proxy")] None);
        service.call_with_deadline_and_grace("task", None, std::time::Duration::from_secs(5), std::time::Duration::from_secs(5), async move {
            // A call in flight must already be visible, even before its result.
            let activity = events.latest_worker_activity(&worker_id, "worker").unwrap()
                .expect("authenticated MCP calls are worker activity");
            assert_eq!(activity.session_id.as_deref(), Some(worker_id.as_str()));
            assert_eq!(activity.event_type, cas_types::EventType::WorkerActivity);
            assert!((chrono::Utc::now() - activity.created_at).num_seconds() < 5);
            Ok(CallToolResult::success(vec![]))
        }).await.unwrap();
    }

    fn result_text(result: &rmcp::model::CallToolResult) -> String {
        serde_json::to_string(result).unwrap()
    }

    /// cas-b1dd: a note append commits, then post-commit work stalls. The
    /// caller is answered COMMITTED (a success, no retry invited) once the
    /// grace elapses, well inside the budget; the stalled work is not dropped
    /// and finishes in the background; the note exists exactly once.
    #[tokio::test]
    async fn stalled_post_commit_work_after_a_note_answers_committed_and_keeps_running_cas_b1dd() {
        use crate::mcp::server::CasCore;
        use crate::mcp::tools::service::CasService;
        use rmcp::handler::server::wrapper::Parameters;
        use std::sync::atomic::{AtomicBool, Ordering};
        let temp = tempfile::tempdir().unwrap();
        let core = CasCore::with_daemon(temp.path().join(".cas"), None, None);
        std::fs::create_dir_all(&core.cas_root).unwrap();
        let tasks = core.open_task_store().unwrap();
        tasks
            .add(&crate::types::Task::new("cas-note".into(), "existing".into()))
            .unwrap();
        let service = CasService::new(
            core,
            #[cfg(feature = "mcp-proxy")]
            None,
        );
        let arguments = serde_json::json!({"action":"notes", "id":"cas-note", "notes":"progress after the commit"});
        let finished = std::sync::Arc::new(AtomicBool::new(false));
        let sender = service.clone();
        let request = serde_json::from_value(arguments.clone()).unwrap();
        let done = finished.clone();
        let call = async move {
            let result = sender.task(Parameters(request)).await;
            // Post-commit work that outlives the grace (a blocked lock, the network).
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            done.store(true, Ordering::SeqCst);
            result
        };
        let started = std::time::Instant::now();
        let answered = service
            .call_with_deadline_and_grace(
                "task",
                arguments.as_object(),
                std::time::Duration::from_secs(10),
                std::time::Duration::from_millis(50),
                call,
            )
            .await
            .expect("a committed note is a success, not a tool error");
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "answered within the grace, not the 10s budget");
        let text = result_text(&answered);
        assert!(text.contains("COMMITTED") && text.contains("cas-note") && text.contains("Do not") && text.contains("retry"), "{text}");
        assert!(!finished.load(Ordering::SeqCst), "answered while post-commit work was still running");
        for _ in 0..50 {
            if finished.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(finished.load(Ordering::SeqCst), "post-commit work kept running after the answer");
        let notes = tasks.get("cas-note").unwrap().notes;
        assert_eq!(notes.matches("progress after the commit").count(), 1, "{notes}");
    }

    /// cas-b1dd: the terminal mark belongs to the close's Closed write. A
    /// close that commits and then stalls is answered COMMITTED within the
    /// grace, and the task is Closed exactly once.
    #[tokio::test]
    async fn stalled_post_commit_work_after_a_close_answers_committed_once_cas_b1dd() {
        use crate::mcp::server::CasCore;
        use crate::mcp::tools::service::CasService;
        use rmcp::handler::server::wrapper::Parameters;
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();
        std::fs::write(cas_root.join("config.toml"), "[verification]\nenabled = false\n").unwrap();
        let core = CasCore::with_daemon(cas_root, None, None);
        let tasks = core.open_task_store().unwrap();
        let mut task = crate::types::Task::new("cas-done".into(), "planning chore".into());
        task.task_type = crate::types::TaskType::Chore;
        task.status = crate::types::TaskStatus::InProgress;
        task.execution_note = Some("no-code".into());
        task.external_ref = Some("https://example.com/runbook".into());
        tasks.add(&task).unwrap();
        let service = CasService::new(
            core,
            #[cfg(feature = "mcp-proxy")]
            None,
        );

        // The close commits its Closed write, then its post-commit work stalls past the grace.
        let arguments = serde_json::json!({"action":"close", "id":"cas-done", "reason":"runbook updated"});
        let sender = service.clone();
        let request = serde_json::from_value(arguments.clone()).unwrap();
        let close = async move {
            let result = sender.task(Parameters(request)).await;
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            result
        };
        let answered = service
            .call_with_deadline_and_grace(
                "task",
                arguments.as_object(),
                std::time::Duration::from_secs(10),
                std::time::Duration::from_millis(50),
                close,
            )
            .await;
        let closed = tasks.get("cas-done").unwrap();
        if closed.status == crate::types::TaskStatus::Closed {
            let text = result_text(&answered.expect("a committed close is a success"));
            assert!(text.contains("COMMITTED") && text.contains("cas-done") && text.contains("close"), "{text}");
            assert_eq!(closed.notes.matches("Closed: runbook updated").count(), 1, "{}", closed.notes);
        } else {
            // A close the gates refused never committed its Closed write, so
            // nothing may claim COMMITTED.
            let text = match &answered {
                Ok(result) => result_text(result),
                Err(error) => error.message.to_string(),
            };
            assert!(!text.contains("COMMITTED"), "{text}");
            panic!("fixture close was refused, so the close path was not exercised: {text}");
        }
    }

    #[tokio::test]
    async fn message_timeout_after_commit_reports_notification_id_cas_e4a8() {
        use crate::mcp::server::CasCore;
        use crate::mcp::tools::service::CasService;
        use crate::test_support::TestEnvGuard;
        use rmcp::handler::server::wrapper::Parameters;
        let mut env = TestEnvGuard::temp_home();
        env.set("CAS_AGENT_NAME", "timeout-supervisor");
        env.set("CAS_AGENT_ROLE", "supervisor");
        let temp = tempfile::tempdir().unwrap();
        let core = CasCore::with_daemon(temp.path().join(".cas"), None, None);
        std::fs::create_dir_all(&core.cas_root).unwrap();
        core.register_agent("timeout-sender".into(), "timeout-supervisor".into(), None)
            .unwrap();
        // Unknown targets are refused before enqueue (cas-a19b). This
        // post-commit timeout fixture needs a routable local recipient.
        crate::store::open_agent_store(&core.cas_root)
            .unwrap()
            .register(&crate::types::Agent::new(
                "timeout-recipient-id".into(),
                "timeout-recipient".into(),
            ))
            .unwrap();
        let service = CasService::new(
            core,
            #[cfg(feature = "mcp-proxy")]
            None,
        );
        let arguments = serde_json::json!({
            "action": "message", "target": "timeout-recipient",
            "summary": "committed before slow handoff", "message": "durable message"
        });
        let sender = service.clone();
        let request = serde_json::from_value(arguments.clone()).unwrap();
        let call = async move {
            sender.coordination(Parameters(request)).await.unwrap();
            // Simulate post-commit work that outlives the response budget.
            std::future::pending::<Result<rmcp::model::CallToolResult, rmcp::ErrorData>>().await
        };
        let started = std::time::Instant::now();
        let error = service
            .call_with_deadline(
                "coordination",
                arguments.as_object(),
                std::time::Duration::from_millis(50),
                call,
            )
            .await
            .unwrap_err();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "post-commit work blocked response"
        );
        let rows = crate::store::open_prompt_queue_store(&service.inner.cas_root)
            .unwrap()
            .peek_all(10)
            .unwrap();
        let row = rows
            .iter()
            .find(|row| row.target == "timeout-recipient")
            .unwrap();
        let outcome = error.message.to_string();
        assert_eq!(
            error.data.as_ref().unwrap()["mutation_outcome"],
            "COMMITTED"
        );
        assert_eq!(error.data.as_ref().unwrap()["notification_id"], row.id);
        assert!(
            outcome.contains("COMMITTED"),
            "durable notification {} reported {outcome}",
            row.id
        );
        assert!(
            outcome.contains(&format!("notification_id: {}", row.id)),
            "{outcome}"
        );
    }

    #[tokio::test]
    async fn task_create_and_notes_timeouts_confirm_only_their_write_cas_e4a8() {
        use crate::mcp::server::CasCore;
        use crate::mcp::tools::service::CasService;
        use crate::test_support::TestEnvGuard;
        use rmcp::handler::server::wrapper::Parameters;
        let _env = TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let core = CasCore::with_daemon(temp.path().join(".cas"), None, None);
        std::fs::create_dir_all(&core.cas_root).unwrap();
        // Start must observe its own commit even without a notifying/syncing
        // wrapper marking the SQLite write for it (cas-d5f4).
        std::fs::write(core.cas_root.join("config.toml"), "[notifications]\nenabled=false\n").unwrap();
        let agent = crate::types::Agent::new_with_role("start-agent".into(), "start-agent".into(), crate::types::AgentRole::Standard);
        core.open_agent_store().unwrap().register(&agent).unwrap();
        core.set_agent_id_for_testing(agent.id.clone());
        let tasks = core.open_task_store().unwrap();
        tasks
            .add(&crate::types::Task::new(
                "cas-note".into(),
                "existing".into(),
            ))
            .unwrap();
        let service = CasService::new(
            core,
            #[cfg(feature = "mcp-proxy")]
            None,
        );
        for arguments in [
            serde_json::json!({"action":"create", "title":"timeout creation", "risk":"none"}),
            serde_json::json!({"action":"notes", "id":"cas-note", "notes":"committed note"}),
            serde_json::json!({"action":"start", "id":"cas-note", "brief":true}),
        ] {
            let sender = service.clone();
            let request = serde_json::from_value(arguments.clone()).unwrap();
            let call = async move {
                sender.task(Parameters(request)).await.unwrap();
                std::future::pending().await
            };
            let error = service
                .call_with_deadline(
                    "task",
                    arguments.as_object(),
                    std::time::Duration::from_millis(100),
                    call,
                )
                .await
                .unwrap_err();
            assert!(error.message.contains("COMMITTED"), "{error:?}");
            assert_eq!(error.data.unwrap()["mutation_outcome"], "COMMITTED");
        }
        assert_eq!(tasks.list(None).unwrap().len(), 2);
        assert_eq!(tasks.get("cas-note").unwrap().status, crate::types::TaskStatus::InProgress);
        assert!(
            tasks
                .get("cas-note")
                .unwrap()
                .notes
                .contains("committed note")
        );
    }

    /// cas-3937: a worktree_merge whose merge and push committed, followed
    /// by a slow phase (the close gate and verifier dispatch), reports the
    /// merged commit and push state: COMMITTED at the deadline, and an early
    /// success once the post-commit grace runs out. Never UNKNOWN.
    #[tokio::test]
    async fn a_committed_merge_with_a_slow_close_reports_its_outcome_cas_3937() {
        use crate::mcp::server::CasCore;
        use crate::mcp::tools::service::CasService;
        let temp = tempfile::tempdir().unwrap();
        let service = CasService::new(
            CasCore::with_daemon(temp.path().to_path_buf(), None, None),
            #[cfg(feature = "mcp-proxy")]
            None,
        );
        let arguments =
            serde_json::json!({"action": "worktree_merge", "id": "sharp-cobra-83", "task_id": "cas-3fb4"});
        let merge_then_slow_close = || async {
            super::super::mutation_receipt::merge_committed("4dd898a71", "epic/v57", "pushed to origin");
            std::future::pending().await
        };

        let error = service
            .call_with_deadline(
                "factory",
                arguments.as_object(),
                std::time::Duration::from_millis(50),
                merge_then_slow_close(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.data.as_ref().unwrap()["mutation_outcome"], "COMMITTED", "{error:?}");
        assert!(
            error.message.contains("4dd898a71") && error.message.contains("pushed to origin"),
            "{error:?}"
        );

        let answered = service
            .call_with_deadline_and_grace(
                "factory",
                arguments.as_object(),
                std::time::Duration::from_secs(30),
                std::time::Duration::from_millis(50),
                merge_then_slow_close(),
            )
            .await
            .expect("a committed merge answers once its grace runs out");
        let text = format!("{answered:?}");
        assert!(text.contains("COMMITTED") && text.contains("4dd898a71"), "{text}");
        assert!(!text.contains("task `"), "a merge is not described as a task mutation: {text}");
    }

    /// cas-24d8: a close that committed only an intermediate write (a gate
    /// note, a parked anchor) before the deadline has not closed anything.
    /// It must not be reported as COMMITTED.
    #[tokio::test]
    async fn close_deadline_after_an_intermediate_write_reports_in_progress_cas_24d8() {
        use crate::mcp::server::CasCore;
        use crate::mcp::tools::service::CasService;
        let temp = tempfile::tempdir().unwrap();
        let service = CasService::new(
            CasCore::with_daemon(temp.path().to_path_buf(), None, None),
            #[cfg(feature = "mcp-proxy")]
            None,
        );
        let arguments = serde_json::json!({"action":"close", "id":"cas-park"});
        let close = async {
            super::super::mutation_receipt::task_committed("cas-park");
            std::future::pending().await
        };
        let error = service
            .call_with_deadline(
                "task",
                arguments.as_object(),
                std::time::Duration::from_millis(20),
                close,
            )
            .await
            .unwrap_err();
        assert!(error.message.contains("IN_PROGRESS"), "{error:?}");
        assert!(!error.message.contains("COMMITTED ("), "{error:?}");
        assert_eq!(error.data.unwrap()["mutation_outcome"], "IN_PROGRESS");
    }

    /// cas-3fc8: after a merge's early COMMITTED answer, the task note says
    /// what its close concluded ("not eligible" rather than a blank) and,
    /// when the merge itself then fails, that failure too.
    #[tokio::test]
    async fn post_commit_note_names_an_ineligible_close_and_a_later_merge_failure_cas_3fc8() {
        use crate::mcp::server::CasCore;
        use crate::mcp::tools::service::CasService;
        use crate::test_support::TestEnvGuard;
        let _env = TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let core = CasCore::with_daemon(temp.path().join(".cas"), None, None);
        std::fs::create_dir_all(&core.cas_root).unwrap();
        let tasks = core.open_task_store().unwrap();
        tasks.add(&crate::types::Task::new("cas-merged".into(), "merged".into())).unwrap();
        let service = CasService::new(
            core,
            #[cfg(feature = "mcp-proxy")]
            None,
        );
        let arguments = serde_json::json!({"action": "worktree_merge", "id": "wt", "task_id": "cas-merged"});
        let merge = async {
            super::super::mutation_receipt::merge_committed("abc123", "epic/x", "pushed to origin");
            // Cassy's close found the task not eligible: its receipt is empty.
            super::super::mutation_receipt::merge_followup(
                &crate::mcp::tools::core::task::lifecycle::merge_close::CassyMergeClose::NotEligible
                    .followup("cas-merged"),
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Err(rmcp::ErrorData {
                code: rmcp::model::ErrorCode::INTERNAL_ERROR,
                message: "Failed to mark worker delivery complete: disk full".into(),
                data: None,
            })
        };
        let answered = service
            .call_with_deadline_and_grace(
                "factory",
                arguments.as_object(),
                std::time::Duration::from_secs(30),
                std::time::Duration::ZERO,
                merge,
            )
            .await
            .expect("the committed merge answers early");
        assert!(format!("{answered:?}").contains("COMMITTED"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let notes = loop {
            let notes = tasks.get("cas-merged").unwrap().notes;
            if notes.contains("POST_COMMIT_OUTCOME") || std::time::Instant::now() > deadline {
                break notes;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        // pin: the note's wording for an ineligible close and a later failure (cas-3fc8).
        assert!(notes.contains("not eligible for Cassy's close"), "{notes}");
        assert!(notes.contains("disk full"), "{notes}");
    }

    /// cas-3b81 (GH #1142): a close still running at the deadline with
    /// nothing committed is not "UNKNOWN". It keeps running, the caller is
    /// told so, and its final outcome lands on the task as a note.
    #[tokio::test]
    async fn close_deadline_before_any_write_records_the_late_outcome_cas_3b81() {
        use crate::mcp::server::CasCore;
        use crate::mcp::tools::service::CasService;
        use crate::test_support::TestEnvGuard;
        let _env = TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let core = CasCore::with_daemon(temp.path().join(".cas"), None, None);
        std::fs::create_dir_all(&core.cas_root).unwrap();
        let tasks = core.open_task_store().unwrap();
        tasks
            .add(&crate::types::Task::new(
                "cas-late".into(),
                "slow close".into(),
            ))
            .unwrap();
        let service = CasService::new(
            core,
            #[cfg(feature = "mcp-proxy")]
            None,
        );
        let arguments = serde_json::json!({"action":"close", "id":"cas-late"});
        let close = async {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            Ok(rmcp::model::CallToolResult::error(vec![
                rmcp::model::Content::text(
                    "⚠️ MERGE REQUIRED\n\ntask close rejected: factory/x has 1 commit(s) from this task not on staging.",
                ),
            ]))
        };
        let error = service
            .call_with_deadline(
                "task",
                arguments.as_object(),
                std::time::Duration::from_millis(20),
                close,
            )
            .await
            .unwrap_err();
        assert!(error.message.contains("IN_PROGRESS"), "{error:?}");
        assert!(error.message.contains("CLOSE_OUTCOME"), "{error:?}");
        assert!(!error.message.contains("UNKNOWN"), "{error:?}");
        assert_eq!(error.data.unwrap()["mutation_outcome"], "IN_PROGRESS");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let notes = loop {
            let notes = tasks.get("cas-late").unwrap().notes;
            if notes.contains("CLOSE_OUTCOME") {
                break notes;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "late close outcome never recorded: {notes}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        assert!(notes.contains("task status now `open`"), "{notes}");
        assert!(notes.contains("refused: ⚠️ MERGE REQUIRED"), "{notes}");
        assert_eq!(notes.matches("CLOSE_OUTCOME").count(), 1, "{notes}");
    }

    #[test]
    fn late_close_summary_keeps_one_bounded_line_cas_3b81() {
        let long = format!("Closed task: cas-x - {}\nsecond line", "y".repeat(400));
        let joined = Ok(Ok(rmcp::model::CallToolResult::success(vec![
            rmcp::model::Content::text(long),
        ])));
        let summary = super::late_close_summary(&joined);
        assert!(summary.starts_with("Closed task: cas-x"), "{summary}");
        assert!(!summary.contains("second line"), "{summary}");
        assert_eq!(summary.chars().count(), 301, "{summary}");
    }

    #[tokio::test]
    async fn concurrent_timeout_receipts_do_not_turn_unknown_into_committed_cas_e4a8() {
        use crate::mcp::server::CasCore;
        use crate::mcp::tools::service::CasService;
        let temp = tempfile::tempdir().unwrap();
        let service = CasService::new(
            CasCore::with_daemon(temp.path().to_path_buf(), None, None),
            #[cfg(feature = "mcp-proxy")]
            None,
        );
        let arguments = serde_json::json!({"action":"message"});
        let budget = std::time::Duration::from_millis(10);
        let committed = async {
            super::super::mutation_receipt::message_committed(42);
            std::future::pending().await
        };
        let (committed, unknown) = tokio::join!(
            service.call_with_deadline("coordination", arguments.as_object(), budget, committed),
            service.call_with_deadline(
                "coordination",
                arguments.as_object(),
                budget,
                std::future::pending()
            ),
        );
        let committed = committed.unwrap_err().data.unwrap();
        assert_eq!(committed["mutation_outcome"], "COMMITTED");
        assert_eq!(committed["notification_id"], 42);
        let unknown = unknown.unwrap_err();
        let data = unknown.data.unwrap();
        assert_eq!(data["mutation_outcome"], "UNKNOWN");
        assert!(data["notification_id"].is_null());
        assert_eq!(data["budget_ms"], 10);
        assert!(data["elapsed_ms"].as_u64().unwrap() >= 10);
        assert!(
            !unknown.message.contains("55s"),
            "elapsed time must be measured"
        );
    }

    #[tokio::test]
    async fn slow_factory_context_preserves_result_and_retains_late_mail_cas_e4a8() {
        use crate::mcp::server::CasCore;
        use crate::mcp::tools::service::CasService;
        let temp = tempfile::tempdir().unwrap();
        let service = CasService::new(
            CasCore::with_daemon(temp.path().to_path_buf(), None, None),
            #[cfg(feature = "mcp-proxy")]
            None,
        );
        let (tx, rx) = tokio::sync::oneshot::channel();
        *service.factory_context.lock().await = Some(tokio::spawn(async move { rx.await.ok() }));
        let result = service
            .append_factory_context_with_budget(
                Ok(CasCore::success("notification_id: 42")),
                std::time::Duration::from_millis(10),
            )
            .await
            .unwrap();
        assert_eq!(result.content.len(), 1);
        assert!(
            service.factory_context.lock().await.is_some(),
            "late mail must remain reachable"
        );
        tx.send("late mail".into()).unwrap();
        let result = service
            .append_factory_context_with_budget(
                Ok(CasCore::success("next result")),
                std::time::Duration::from_secs(1),
            )
            .await
            .unwrap();
        assert_eq!(result.content.len(), 2);
        assert!(
            serde_json::to_string(&result)
                .unwrap()
                .contains("late mail")
        );
        assert!(
            service.factory_context.lock().await.is_none(),
            "delivered once"
        );
    }

    #[tokio::test]
    async fn response_fallback_preserves_result_and_surfaces_mail_once() {
        use crate::mcp::server::CasCore;
        use crate::mcp::tools::service::CasService;
        use crate::test_support::TestEnvGuard;
        use cas_store::{PromptQueueStore, SqlitePromptQueueStore};
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_NAME", Some("response-worker")),
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_SESSION", Some("response-factory")),
            (crate::internal_llm::INTERNAL_LLM_ENV, None),
        ]);
        let core = CasCore::with_daemon(temp.path().to_path_buf(), None, None);
        core.register_agent("response-session".into(), "response-worker".into(), None)
            .unwrap();
        let service = CasService::new(
            core,
            #[cfg(feature = "mcp-proxy")]
            None,
        );
        let account = temp.path().join("account");
        let slug: String = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .chars()
            .map(|c| if matches!(c, '/' | '.') { '-' } else { c })
            .collect();
        let transcripts = account.join("projects").join(slug);
        std::fs::create_dir_all(&transcripts).unwrap();
        std::fs::write(
            transcripts.join("response-session.jsonl"),
            r#"{"type":"user","promptId":"response-turn","message":{"content":"continue"}}"#,
        )
        .unwrap();
        let store = service.inner.open_agent_store().unwrap();
        let mut agent = store.get("response-session").unwrap();
        agent.metadata.insert(
            "worker_account_dir".into(),
            account.to_string_lossy().into_owned(),
        );
        agent.metadata.insert("worker_cli".into(), "claude".into());
        store.update(&agent).unwrap();
        assert_eq!(agent.role, crate::types::AgentRole::Worker);
        assert_eq!(agent.name, "response-worker");
        assert_eq!(
            service.inner.get_registered_agent_id_read_only().unwrap(),
            agent.id
        );
        assert_eq!(
            super::caller_transcript_path(&agent),
            Some(transcripts.join("response-session.jsonl"))
        );
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        queue
            .enqueue_with_session(
                "supervisor",
                "response-worker",
                "mail via MCP fallback",
                "response-factory",
            )
            .unwrap();
        let output = service
            .append_factory_context(Ok(CasCore::success("original result")))
            .await
            .unwrap();
        assert_eq!(output.content.len(), 2);
        let text = serde_json::to_string(&output).unwrap();
        assert!(text.contains("original result"));
        assert!(text.contains("mail via MCP fallback"));
        let repeated = service
            .append_factory_context(Ok(CasCore::success("next result")))
            .await
            .unwrap();
        assert_eq!(repeated.content.len(), 1);
    }

    /// cas-badb (GH #1302): a subagent (here a task-verifier) shares its
    /// parent's MCP server. Its CAS tool calls must neither show nor consume
    /// the parent's mail; the parent's own next call still receives it.
    #[tokio::test]
    async fn subagent_mcp_call_does_not_consume_parent_mail_cas_badb() {
        use crate::mcp::server::CasCore;
        use crate::mcp::tools::service::CasService;
        use crate::test_support::TestEnvGuard;
        use cas_store::{DeliveryStage, PromptQueueStore, SqlitePromptQueueStore, SurfacingSource};
        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_NAME", Some("parent-supervisor")),
            ("CAS_AGENT_ROLE", Some("supervisor")),
            ("CAS_FACTORY_SESSION", Some("badb-factory")),
            (crate::internal_llm::INTERNAL_LLM_ENV, None),
        ]);
        let core = CasCore::with_daemon(temp.path().to_path_buf(), None, None);
        core.register_agent("parent-session".into(), "parent-supervisor".into(), None)
            .unwrap();
        let service = CasService::new(
            core,
            #[cfg(feature = "mcp-proxy")]
            None,
        );
        let account = temp.path().join("account");
        let slug: String = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .chars()
            .map(|c| if matches!(c, '/' | '.') { '-' } else { c })
            .collect();
        let transcripts = account.join("projects").join(slug);
        let subagents = transcripts.join("parent-session").join("subagents");
        std::fs::create_dir_all(&subagents).unwrap();
        let tool_use = |id: &str, sidechain: bool| {
            serde_json::json!({
                "type": "assistant",
                "isSidechain": sidechain,
                "message": {"content": [{"type": "tool_use", "id": id, "name": "mcp__cas__task"}]},
            })
            .to_string()
        };
        // The parent's transcript names the verifier's call only inside a
        // sidechain record and a tool result, never as its own tool_use.
        let parent = [
            serde_json::json!({"type":"user", "promptId":"parent-turn",
                "timestamp": (chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339(),
                "message":{"content":"continue"}})
            .to_string(),
            tool_use("toolu_parent", false),
            tool_use("toolu_sidechain", true),
            serde_json::json!({"type": "user", "message": {"content": [
                {"type": "tool_result", "tool_use_id": "toolu_verifier"}]}})
            .to_string(),
        ]
        .join("\n");
        std::fs::write(transcripts.join("parent-session.jsonl"), &parent).unwrap();
        std::fs::write(
            subagents.join("agent-verifier.jsonl"),
            tool_use("toolu_verifier", false),
        )
        .unwrap();
        let store = service.inner.open_agent_store().unwrap();
        let mut agent = store.get("parent-session").unwrap();
        agent.metadata.insert(
            "worker_account_dir".into(),
            account.to_string_lossy().into_owned(),
        );
        agent.metadata.insert("worker_cli".into(), "claude".into());
        store.update(&agent).unwrap();
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        let first_body = "peer ask for the parent ".to_owned() + &"a".repeat(2_600);
        let first = queue
            .enqueue_with_summary(
                "worker-a",
                "parent-supervisor",
                &first_body,
                Some("badb-factory"),
                Some("review the pending branch"),
            )
            .unwrap();
        // The harness accepted the transport, but no recipient turn carried the body.
        queue.mark_transport_delivered(first).unwrap();
        queue
            .record_recipient_surfaced(
                first,
                "parent-supervisor",
                SurfacingSource::TransportDelivered,
            )
            .unwrap();
        let second_body = "next pending request ".to_owned() + &"b".repeat(2_600);
        let second = queue
            .enqueue_with_summary(
                "worker-b",
                "parent-supervisor",
                &second_body,
                Some("badb-factory"),
                Some("second pending decision"),
            )
            .unwrap();
        let before = queue.message_delivery_report(first).unwrap().unwrap();
        assert_eq!(before.stage, DeliveryStage::Delivered);
        assert!(before.confirmed_at.is_none());
        // pin: the existing message_status JSON field must distinguish handoff from rendering.
        assert_eq!(
            serde_json::to_value(&before).unwrap()["recipient_receipt"],
            "transport_delivered"
        );
        let budget = std::time::Duration::from_millis(500);

        for caller in [
            Some("toolu_verifier"),
            Some("toolu_sidechain"),
            Some("toolu_unknown"),
            None,
        ] {
            let output = service
                .append_factory_context_for_caller(
                    Ok(CasCore::success("verifier result")),
                    budget,
                    caller.map(str::to_owned),
                )
                .await
                .unwrap();
            assert_eq!(
                output.content.len(),
                1,
                "{caller:?} must not see the parent's mail"
            );
            assert_eq!(
                queue.recipient_receipt(first, "parent-supervisor").unwrap(),
                Some(SurfacingSource::TransportDelivered)
            );
            assert!(
                queue
                    .message_delivery_report(first)
                    .unwrap()
                    .unwrap()
                    .confirmed_at
                    .is_none()
            );
        }
        // The next real parent turn automatically takes the provisional handoff.
        let next = serde_json::json!({"type":"user", "promptId":"parent-next-turn",
            "timestamp": chrono::Utc::now().to_rfc3339(), "message":{"content":"continue again"}});
        let transcript = transcripts.join("parent-session.jsonl");
        std::fs::write(
            &transcript,
            format!("{parent}\n{next}\n{}", tool_use("toolu_parent_next", false)),
        )
        .unwrap();
        let hook = crate::hooks::HookInput {
            session_id: "parent-session".into(),
            agent_role: Some("supervisor".into()),
            transcript_path: Some(transcript.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let block = crate::hooks::handlers::handlers_middle::factory_inbox::surface_factory_inbox(
            Some(temp.path()),
            &hook,
        )
        .unwrap();
        assert!(block.contains(&first_body));
        assert!(!block.contains(&second_body), "overflow must remain unread");
        assert!(block.contains(&format!("Message #{second} did not fit")));
        // pin: overflow is actionable by id and original summary, not an anonymous count.
        assert!(block.contains("second pending decision"), "{block}");
        let rendered = queue.message_delivery_report(first).unwrap().unwrap();
        assert_eq!(
            rendered.recipient_receipt,
            Some(SurfacingSource::HookSurfaced)
        );
        assert!(rendered.confirmed_at.is_some());
        assert!(
            queue
                .message_delivery_report(second)
                .unwrap()
                .unwrap()
                .confirmed_at
                .is_none()
        );
        // The parent's next tool boundary recovers overflow without an explicit inbox poll.
        let output = service
            .append_factory_context_for_caller(
                Ok(CasCore::success("parent result")),
                budget,
                Some("toolu_parent_next".into()),
            )
            .await
            .unwrap();
        assert_eq!(output.content.len(), 2);
        assert!(result_text(&output).contains(&second_body));
        assert_eq!(
            queue
                .recipient_receipt(second, "parent-supervisor")
                .unwrap(),
            Some(SurfacingSource::HookSurfaced)
        );
    }

    #[test]
    fn claude_tool_use_id_reads_call_meta_cas_badb() {
        let meta: rmcp::model::Meta =
            serde_json::from_value(serde_json::json!({"claudecode/toolUseId": "toolu_1"})).unwrap();
        assert_eq!(super::claude_tool_use_id(Some(&meta)).as_deref(), Some("toolu_1"));
        assert_eq!(super::claude_tool_use_id(None), None);
        let empty: rmcp::model::Meta = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(super::claude_tool_use_id(Some(&empty)), None);
    }

    #[tokio::test]
    async fn response_fallback_does_not_replay_transport_delivered_mail() {
        use crate::mcp::server::CasCore;
        use crate::mcp::tools::service::CasService;
        use crate::test_support::TestEnvGuard;
        use cas_store::{PromptQueueStore, SqlitePromptQueueStore};

        let temp = tempfile::tempdir().unwrap();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("CAS_AGENT_NAME", Some("response-worker")),
            ("CAS_AGENT_ROLE", Some("worker")),
            ("CAS_FACTORY_SESSION", Some("response-factory")),
            (crate::internal_llm::INTERNAL_LLM_ENV, None),
        ]);
        let core = CasCore::with_daemon(temp.path().to_path_buf(), None, None);
        core.register_agent("response-session".into(), "response-worker".into(), None)
            .unwrap();
        let service = CasService::new(
            core,
            #[cfg(feature = "mcp-proxy")]
            None,
        );
        let account = temp.path().join("account");
        let slug: String = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .chars()
            .map(|c| if matches!(c, '/' | '.') { '-' } else { c })
            .collect();
        let transcripts = account.join("projects").join(slug);
        std::fs::create_dir_all(&transcripts).unwrap();
        std::fs::write(
            transcripts.join("response-session.jsonl"),
            r#"{"type":"user","promptId":"response-turn","message":{"content":"continue"}}"#,
        )
        .unwrap();
        let store = service.inner.open_agent_store().unwrap();
        let mut agent = store.get("response-session").unwrap();
        agent.metadata.insert(
            "worker_account_dir".into(),
            account.to_string_lossy().into_owned(),
        );
        agent.metadata.insert("worker_cli".into(), "claude".into());
        store.update(&agent).unwrap();
        let queue = SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        let id = queue
            .enqueue_with_session(
                "supervisor",
                "response-worker",
                "mail already injected by transport",
                "response-factory",
            )
            .unwrap();
        queue.mark_transport_delivered(id).unwrap();

        let output = service
            .append_factory_context(Ok(CasCore::success("original result")))
            .await
            .unwrap();
        assert_eq!(output.content.len(), 1);
        assert!(
            !serde_json::to_string(&output)
                .unwrap()
                .contains("mail already injected by transport")
        );
    }

    #[test]
    fn timeout_backstop_never_treats_task_close_as_read_only() {
        assert!(potentially_mutating_call("task", "close"));
        assert!(!potentially_mutating_call("task", "show"));
        assert!(potentially_mutating_call("unknown_future_tool", "anything"));
    }
}
