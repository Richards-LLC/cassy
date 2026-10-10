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
            self.append_factory_context_with_budget(
                result,
                remaining.min(std::time::Duration::from_millis(500)),
            )
            .await
        }
    }
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
    /// The deadline alone, with no early post-commit answer.
    #[cfg(test)]
    async fn call_with_deadline<F>(
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
        let task_id = arguments
            .and_then(|args| args.get("id"))
            .and_then(serde_json::Value::as_str);
        let receipt = super::mutation_receipt::Receipt::new(tool_name, action, task_id);
        let start = std::time::Instant::now();
        let mut handle = tokio::spawn(super::mutation_receipt::scope(receipt.clone(), future));
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
                let watcher_receipt = receipt.clone();
                let tool = tool_name.to_string();
                tokio::spawn(async move {
                    let _ = handle.await;
                    log_post_commit(&tool, &watcher_receipt, "finished in the background");
                });
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
                let id = this.inner.get_registered_agent_id_read_only().ok()?;
                let agent = this.inner.open_agent_store().ok()?.get(&id).ok()?;
                let name = std::env::var("CAS_AGENT_NAME").ok()?;
                if agent.name != name {
                    return None;
                }
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
        "rule" | "skill" | "spec" | "verification" | "coordination" | "system" | "team"
        | "pattern" | "knowledge" | "artifact" => {
            !matches!(action, "show" | "list" | "status" | "members")
        }
        // Unknown tool/action schemas must be treated as write-capable: an
        // optimistic "not committed" answer would invite a duplicate write.
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::potentially_mutating_call;

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
        assert!(
            tasks
                .get("cas-note")
                .unwrap()
                .notes
                .contains("committed note")
        );
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
