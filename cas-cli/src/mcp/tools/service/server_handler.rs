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
            instructions: Some(
                "CAS (Coding Agent System) provides unified memory, tasks, rules, and skills."
                    .to_string(),
            ),
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
            info!(method = "resources/list", count = resources.len(), elapsed_ms = start.elapsed().as_millis() as u64, "MCP resources/list DONE");
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
            let tools = self.tool_router.list_all();
            info!(method = "tools/list", count = tools.len(), elapsed_ms = start.elapsed().as_millis() as u64, "MCP tools/list DONE");

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

            // Timeout after 55s to prevent silent hangs (Claude Code cancels at 60s)
            let result = match tokio::time::timeout(
                std::time::Duration::from_secs(55),
                self.tool_router.call(tcc),
            )
            .await
            {
                Ok(result) => {
                    let elapsed = start.elapsed();
                    if elapsed.as_secs() >= 5 {
                        info!(method = "tools/call", tool = %tool_name, id = %request_id, elapsed_ms = elapsed.as_millis() as u64, "MCP slow request");
                    }
                    result
                }
                Err(_) => {
                    warn!(method = "tools/call", tool = %tool_name, id = %request_id, "MCP tool call TIMED OUT after 55s — handler hung");
                    let mutation_outcome =
                        self.mutation_timeout_outcome(&tool_name, timeout_arguments.as_ref());
                    Err(rmcp::ErrorData {
                        code: rmcp::model::ErrorCode::INTERNAL_ERROR,
                        message: format!(
                            "Tool '{}' timed out after 55s. This is a Cassy server bug — please report it. Mutation outcome: {}",
                            tool_name, mutation_outcome
                        ).into(),
                        data: None,
                    })
                }
            };

            result
        }
    }
}

impl CasService {
    /// A response-layer timeout cancels the handler, but a synchronous store
    /// write may have committed just before that cancellation. Never leave a
    /// caller guessing whether retrying a mutating request would duplicate it.
    fn mutation_timeout_outcome(
        &self,
        tool_name: &str,
        arguments: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> String {
        let action = arguments
            .and_then(|args| args.get("action"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();

        if !potentially_mutating_call(tool_name, action) {
            return "not applicable (the timed-out call was read-only)".to_string();
        }

        // Task close is the dangerous retry shape: its postcondition is cheap
        // and authoritative, so report it rather than a vague generic hint.
        if tool_name == "task"
            && action == "close"
            && let Some(task_id) = arguments
                .and_then(|args| args.get("id"))
                .and_then(serde_json::Value::as_str)
        {
            if let Ok(store) = self.inner.open_task_store()
                && let Ok(task) = store.get(task_id)
                && task.status == crate::types::TaskStatus::Closed
            {
                return format!(
                    "COMMITTED (task `{task_id}` is Closed; do not retry close, re-query task state)"
                );
            }
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
        "rule" | "skill" | "spec" | "verification" | "coordination" | "system" | "team"
        | "pattern" | "knowledge" => !matches!(action, "show" | "list" | "status" | "members"),
        // Unknown tool/action schemas must be treated as write-capable: an
        // optimistic "not committed" answer would invite a duplicate write.
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::potentially_mutating_call;

    #[test]
    fn timeout_backstop_never_treats_task_close_as_read_only() {
        assert!(potentially_mutating_call("task", "close"));
        assert!(!potentially_mutating_call("task", "show"));
        assert!(potentially_mutating_call("unknown_future_tool", "anything"));
    }
}
