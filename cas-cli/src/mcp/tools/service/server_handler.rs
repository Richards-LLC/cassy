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
            let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);

            let budget = std::time::Duration::from_secs(55);
            let result = self
                .call_with_deadline(
                    &tool_name,
                    timeout_arguments.as_ref(),
                    budget,
                    self.tool_router.call(tcc),
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

impl CasService {
    async fn call_with_deadline<F>(
        &self,
        tool_name: &str,
        arguments: Option<&serde_json::Map<String, serde_json::Value>>,
        budget: std::time::Duration,
        future: F,
    ) -> Result<rmcp::model::CallToolResult, rmcp::ErrorData>
    where
        F: std::future::Future<Output = Result<rmcp::model::CallToolResult, rmcp::ErrorData>>,
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
        match tokio::time::timeout(
            budget,
            super::mutation_receipt::scope(receipt.clone(), future),
        )
        .await
        {
            Ok(result) => {
                let elapsed = start.elapsed();
                if elapsed.as_secs() >= 5 {
                    info!(tool = tool_name, elapsed_ms = elapsed.as_millis() as u64, "MCP slow request");
                }
                result
            }
            Err(_) => {
                let elapsed = start.elapsed();
                let commit = receipt.commit.get();
                let outcome = self.mutation_timeout_outcome(tool_name, arguments, commit);
                warn!(tool = tool_name, elapsed_ms = elapsed.as_millis() as u64, budget_ms = budget.as_millis() as u64, mutation_outcome = %outcome, "MCP response deadline elapsed");
                Err(rmcp::ErrorData {
                    code: rmcp::model::ErrorCode::INTERNAL_ERROR,
                    message: format!("Tool '{tool_name}' response deadline elapsed after {:.3}s (budget {:.3}s). Mutation outcome: {outcome}", elapsed.as_secs_f64(), budget.as_secs_f64()).into(),
                    data: Some(serde_json::json!({
                        "mutation_outcome": if commit.is_some() { "COMMITTED" } else if potentially_mutating_call(tool_name, action) { "UNKNOWN" } else { "NOT_APPLICABLE" },
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
