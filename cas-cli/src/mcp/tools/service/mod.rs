//! MCP Tools Service for CAS
//!
//! This module exposes consolidated meta-tools:
//! - cas_memory: All memory/entry operations
//! - cas_task: All task and dependency operations
//! - cas_rule: All rule operations
//! - cas_skill: All skill operations
//! - cas_coordination: Agent, factory, and worktree operations (merged)
//! - cas_search: Search, context, and entity operations
//! - cas_system: Diagnostics, stats, and maintenance
//! - cas_verification: Task quality gates
//! - cas_team: Team collaboration
//! - cas_pattern: Personal patterns
//! - cas_spec: Specifications
//! - mcp_search: Search tools across connected upstream MCP servers
//! - mcp_execute: Execute tool calls across connected upstream MCP servers

use rmcp::handler::server::wrapper::Parameters;
#[cfg(feature = "mcp-proxy")]
use rmcp::model::Content;
use rmcp::model::{CallToolResult, ErrorCode};
use rmcp::{ErrorData as McpError, tool, tool_router};

use crate::mcp::server::CasCore;

mod imports;

// Re-export types from cas-mcp for MCP tool parameters
pub use cas_mcp::{
    AgentRequest, CoordinationRequest, ExecuteRequest, FactoryRequest, MemoryRequest,
    PatternRequest, RuleRequest, SearchContextRequest, SkillRequest, SpecRequest, SystemRequest,
    TaskRequest, TeamRequest, VerificationRequest,
};

// ============================================================================
// Git Blame Helper Types
// ============================================================================

/// A single line from git blame output
pub(super) struct GitBlameLine {
    pub(super) commit_hash: String,
    pub(super) line_number: usize,
    pub(super) author: String,
    pub(super) content: String,
}

/// Parse git blame porcelain format
pub(super) fn parse_git_blame_porcelain(content: &str) -> Vec<GitBlameLine> {
    let mut lines_iter = content.lines().peekable();
    let mut results = Vec::new();
    let mut line_number = 0usize;

    while let Some(header) = lines_iter.next() {
        // Header format: <hash> <orig-line> <final-line> [<num-lines>]
        let parts: Vec<&str> = header.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        let commit_hash = parts[0].to_string();

        // Read metadata lines until we hit the content line (starts with \t)
        let mut author = String::new();

        while let Some(line) = lines_iter.peek() {
            if line.starts_with('\t') {
                break;
            }
            let meta_line = lines_iter.next().unwrap();

            if let Some(author_val) = meta_line.strip_prefix("author ") {
                author = author_val.to_string();
            }
        }

        // Read content line (prefixed with tab)
        if let Some(content_line) = lines_iter.next() {
            line_number += 1;
            let content = content_line.strip_prefix('\t').unwrap_or(content_line);

            results.push(GitBlameLine {
                commit_hash,
                line_number,
                author,
                content: content.to_string(),
            });
        }
    }

    results
}

pub(super) use super::truncate_str;

/// Internal worktree request type used by handler methods.
/// The MCP-facing type is CoordinationRequest; this is used for internal dispatch.
#[derive(Debug)]
pub struct WorktreeRequest {
    pub action: String,
    pub id: Option<String>,
    pub task_id: Option<String>,
    pub all: Option<bool>,
    pub status: Option<String>,
    pub orphans: Option<bool>,
    pub dry_run: Option<bool>,
    pub force: Option<bool>,
    /// Explicit trunk merge intent (cas-0b32) — independent of force.
    pub allow_trunk: Option<bool>,
    /// cas-369f: remove worktree after merge (independent of force=dirty).
    pub cleanup: Option<bool>,
}

// ============================================================================
// Tool Router Implementation
// ============================================================================

use rmcp::handler::server::router::tool::ToolRouter;

/// CAS MCP service with consolidated meta-tools
///
/// Provides action-based tools that consolidate related operations,
/// reducing MCP tool context overhead. Agent, factory, and worktree
/// tools are merged into a single `coordination` tool.
///
/// When a proxy engine is configured (via `.cas/proxy.toml`), two additional
/// tools (`mcp_search` and `mcp_execute`) are exposed for routing through
/// upstream MCP servers.
#[derive(Clone)]
pub struct CasService {
    pub inner: CasCore,
    /// MCP proxy engine for upstream server aggregation (optional).
    #[cfg(feature = "mcp-proxy")]
    pub proxy: Option<std::sync::Arc<cmcp_core::ProxyEngine>>,
    /// Tool router used internally by rmcp's #[tool_router] macro
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl CasService {
    pub fn new(
        inner: CasCore,
        #[cfg(feature = "mcp-proxy")] proxy: Option<std::sync::Arc<cmcp_core::ProxyEngine>>,
    ) -> Self {
        Self {
            inner,
            #[cfg(feature = "mcp-proxy")]
            proxy,
            tool_router: Self::tool_router(),
        }
    }

    /// Names of all MCP tools registered on this service, in router order.
    ///
    /// Used by `cas serve` startup to log the actual registered tool set and to
    /// guard against shipping an empty registry (which would silently surface
    /// to the MCP client as "0 tools available" with no error). See cas-5c05.
    pub fn registered_tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .into_iter()
            .map(|t| t.name.into_owned())
            .collect()
    }

    #[allow(dead_code)]
    fn success(text: impl Into<String>) -> CallToolResult {
        CasCore::success(text)
    }

    fn error(code: ErrorCode, message: impl Into<String>) -> McpError {
        CasCore::error(code, message)
    }
}

#[tool_router]
impl CasService {
    // ========================================================================
    // cas_memory - All memory operations
    // ========================================================================

    #[tool(
        description = "Memory operations. Actions: remember (store new), get (by ID), list, update, delete, archive, unarchive, helpful, harmful, recent, set_tier (working/cold/archive), opinion_reinforce, opinion_weaken, opinion_contradict."
    )]
    pub async fn memory(
        &self,
        Parameters(req): Parameters<MemoryRequest>,
    ) -> Result<CallToolResult, McpError> {
        let this = self.clone();
        panic_catch::dispatch_with_catch("memory", async move {
            let action = req.action.clone();
            let is_mutating = matches!(
                req.action.as_str(),
                "remember"
                    | "update"
                    | "delete"
                    | "archive"
                    | "unarchive"
                    | "helpful"
                    | "harmful"
                    | "mark_reviewed"
                    | "set_tier"
                    | "opinion_reinforce"
                    | "opinion_weaken"
                    | "opinion_contradict"
            );

            let result = match req.action.as_str() {
                "remember" => this.memory_remember(req).await,
                "get" => this.memory_get(req).await,
                "list" => this.memory_list(req).await,
                "update" => this.memory_update(req).await,
                "delete" => this.memory_delete(req).await,
                "archive" => this.memory_archive(req).await,
                "unarchive" => this.memory_unarchive(req).await,
                "helpful" => this.memory_helpful(req).await,
                "harmful" => this.memory_harmful(req).await,
                "mark_reviewed" => this.memory_mark_reviewed(req).await,
                "recent" => this.memory_recent(req).await,
                "set_tier" => this.memory_set_tier(req).await,
                "opinion_reinforce" => this.memory_opinion_reinforce(req).await,
                "opinion_weaken" => this.memory_opinion_weaken(req).await,
                "opinion_contradict" => this.memory_opinion_contradict(req).await,
                _ => Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "Unknown memory action: {}. Valid: remember, get, list, update, delete, archive, unarchive, helpful, harmful, mark_reviewed, recent, set_tier, opinion_reinforce, opinion_weaken, opinion_contradict",
                        req.action
                    ),
                )),
            };

            // Notify client of resource changes (Claude Code 2.1.0+)
            if is_mutating && result.is_ok() {
                this.inner.notify_resources_changed().await;
            }

            // Track MCP tool usage
            crate::telemetry::track_mcp_tool("memory", &action, result.is_ok());

            result
        })
        .await
    }

    // ========================================================================
    // cas_task - All task operations
    // ========================================================================

    #[tool(
        description = "Task operations. Actions: create, show, update, start, close, reopen, delete, list, ready (actionable), blocked, notes (add progress), dep_add, dep_remove, dep_list, claim, release, reset, transfer, available, mine. Prefer `start` for normal worker execution; use `claim` for manual lease control/recovery; use `reset` to revive a task orphaned by a dead session (atomic: force-releases lease, clears assignee, forces status=open). IMPORTANT for 'close': verification must pass first. Workers should attempt close; if close returns verification-required guidance, follow the indicated verifier ownership workflow."
    )]
    pub async fn task(
        &self,
        Parameters(req): Parameters<TaskRequest>,
    ) -> Result<CallToolResult, McpError> {
        let this = self.clone();
        panic_catch::dispatch_with_catch("task", async move {
            let action = req.action.clone();
            let event_task_id = req.id.clone().unwrap_or_default();
            let is_mutating = matches!(
                req.action.as_str(),
                "create"
                    | "update"
                    | "start"
                    | "close"
                    | "reopen"
                    | "delete"
                    | "notes"
                    | "dep_add"
                    | "dep_remove"
                    | "claim"
                    | "release"
                    | "reset"
                    | "transfer"
            );

            let result = match req.action.as_str() {
                "create" => this.task_create(req).await,
                "show" => this.task_show(req).await,
                "update" => this.task_update(req).await,
                "start" => this.task_start(req).await,
                "close" => this.task_close(req).await,
                "reopen" => this.task_reopen(req).await,
                "delete" => this.task_delete(req).await,
                "list" => this.task_list(req).await,
                "ready" => this.task_ready(req).await,
                "blocked" => this.task_blocked(req).await,
                "notes" => this.task_notes(req).await,
                "dep_add" => this.task_dep_add(req).await,
                "dep_remove" => this.task_dep_remove(req).await,
                "dep_list" => this.task_dep_list(req).await,
                "claim" => this.task_claim(req).await,
                "release" => this.task_release(req).await,
                "reset" => this.task_reset(req).await,
                "transfer" => this.task_transfer(req).await,
                "available" => this.task_available(req).await,
                "mine" => this.task_mine(req).await,
                _ => Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "Unknown task action: {}. Valid: create, show, update, start, close, reopen, delete, list, ready, blocked, notes, dep_add, dep_remove, dep_list, claim, release, reset, transfer, available, mine",
                        req.action
                    ),
                )),
            };

            if let Err(error) = &result {
                let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(
                    &this.inner.cas_root,
                    "error",
                    &[
                        ("tool", "task"),
                        ("action", &action),
                        ("task_id", &event_task_id),
                        ("message", error.message.as_ref()),
                    ],
                );
            }

            // Notify client of resource changes (Claude Code 2.1.0+)
            if is_mutating && result.is_ok() {
                this.inner.notify_resources_changed().await;
            }

            // Track MCP tool usage
            crate::telemetry::track_mcp_tool("task", &action, result.is_ok());

            result
        })
        .await
    }

    // ========================================================================
    // cas_rule - All rule operations
    // ========================================================================

    #[tool(
        description = "Rule operations. Actions: create, show, update, delete, list (proven only), list_all, helpful (promotes to proven), harmful, sync (to .claude/rules/), check_similar (find similar existing rules)."
    )]
    pub async fn rule(
        &self,
        Parameters(req): Parameters<RuleRequest>,
    ) -> Result<CallToolResult, McpError> {
        let this = self.clone();
        panic_catch::dispatch_with_catch("rule", async move {
            let action = req.action.clone();
            let is_mutating = matches!(
                req.action.as_str(),
                "create" | "update" | "delete" | "helpful" | "harmful" | "sync"
            );

            let result = match req.action.as_str() {
                "create" => this.rule_create(req).await,
                "show" => this.rule_show(req).await,
                "update" => this.rule_update(req).await,
                "delete" => this.rule_delete(req).await,
                "list" => this.rule_list(req).await,
                "list_all" => this.rule_list_all(req).await,
                "helpful" => this.rule_helpful(req).await,
                "harmful" => this.rule_harmful(req).await,
                "sync" => this.rule_sync(req).await,
                "check_similar" => this.rule_check_similar(req).await,
                _ => Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "Unknown rule action: {}. Valid: create, show, update, delete, list, list_all, helpful, harmful, sync, check_similar",
                        req.action
                    ),
                )),
            };

            // Notify client of resource changes (Claude Code 2.1.0+)
            if is_mutating && result.is_ok() {
                this.inner.notify_resources_changed().await;
            }

            // Track MCP tool usage
            crate::telemetry::track_mcp_tool("rule", &action, result.is_ok());

            result
        })
        .await
    }

    // ========================================================================
    // cas_skill - All skill operations
    // ========================================================================

    #[tool(
        description = "Skill operations. Actions: create, show, update, delete, list (enabled), list_all, enable, disable, sync (to .claude/skills/), use (record usage)."
    )]
    pub async fn skill(
        &self,
        Parameters(req): Parameters<SkillRequest>,
    ) -> Result<CallToolResult, McpError> {
        let this = self.clone();
        panic_catch::dispatch_with_catch("skill", async move {
            let action = req.action.clone();
            let is_mutating = matches!(
                req.action.as_str(),
                "create" | "update" | "delete" | "enable" | "disable" | "sync" | "use"
            );

            let result = match req.action.as_str() {
                "create" => this.skill_create(req).await,
                "show" => this.skill_show(req).await,
                "update" => this.skill_update(req).await,
                "delete" => this.skill_delete(req).await,
                "list" => this.skill_list(req).await,
                "list_all" => this.skill_list_all(req).await,
                "enable" => this.skill_enable(req).await,
                "disable" => this.skill_disable(req).await,
                "sync" => this.skill_sync(req).await,
                "use" => this.skill_use(req).await,
                _ => Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "Unknown skill action: {}. Valid: create, show, update, delete, list, list_all, enable, disable, sync, use",
                        req.action
                    ),
                )),
            };

            // Notify client of resource changes (Claude Code 2.1.0+)
            if is_mutating && result.is_ok() {
                this.inner.notify_resources_changed().await;
            }

            // Track MCP tool usage
            crate::telemetry::track_mcp_tool("skill", &action, result.is_ok());

            result
        })
        .await
    }

    // ========================================================================
    // cas_coordination - Agent, factory, and worktree operations (merged)
    // ========================================================================

    #[tool(
        description = "Coordination operations combining agent, factory, and worktree management. Agent actions: register, unregister, whoami, heartbeat, agent_list, agent_cleanup, session_start, session_end, loop_start, loop_cancel, loop_status, lease_history, queue_notify, queue_poll, queue_peek, queue_ack, inbox_poll, message, message_ack, message_status. Factory actions: spawn_workers, shutdown_workers, worker_status, worker_activity, clear_context, my_context, sync_all_workers, gc_report, gc_cleanup, epic_status (per-child branch merge state for an epic), focus_epic, remind, remind_list, remind_cancel. Worktree actions: worktree_create, worktree_list, worktree_show, worktree_cleanup, worktree_merge, worktree_status. Only available in factory mode. For shutdown_workers, supervisor should verify worktree cleanliness/policy before issuing shutdown."
    )]
    pub async fn coordination(
        &self,
        Parameters(req): Parameters<CoordinationRequest>,
    ) -> Result<CallToolResult, McpError> {
        let this = self.clone();
        panic_catch::dispatch_with_catch("coordination", async move {
            let action = req.action.clone();
            let event_target = req.target.clone().unwrap_or_default();
            let event_task_id = req.task_id.clone().unwrap_or_default();

            let result = match action.as_str() {
                // ---- Agent domain ----
                "register" | "unregister" | "whoami" | "heartbeat" | "session_start"
                | "session_end" | "loop_start" | "loop_cancel" | "loop_status"
                | "lease_history" | "queue_notify" | "queue_poll" | "queue_peek"
                | "queue_ack" | "inbox_poll" | "message" | "interrupt" | "message_ack"
                | "message_status" => {
                    // `interrupt` is sugar for `message` with urgent=true.
                    let agent_req = if action == "interrupt" {
                        let mut r = req.to_agent_request("message");
                        r.urgent = Some(true);
                        r
                    } else {
                        req.to_agent_request(&action)
                    };
                    match action.as_str() {
                        "register" => this.agent_register(agent_req).await,
                        "unregister" => this.agent_unregister(agent_req).await,
                        "whoami" => this.agent_whoami(agent_req).await,
                        "heartbeat" => this.agent_heartbeat(agent_req).await,
                        "session_start" => this.agent_session_start(agent_req).await,
                        "session_end" => this.agent_session_end(agent_req).await,
                        "loop_start" => this.loop_start(agent_req).await,
                        "loop_cancel" => this.loop_cancel(agent_req).await,
                        "loop_status" => this.loop_status(agent_req).await,
                        "lease_history" => this.lease_history(agent_req).await,
                        "queue_notify" => this.queue_notify(agent_req).await,
                        "queue_poll" => this.queue_poll(agent_req).await,
                        "queue_peek" => this.queue_peek(agent_req).await,
                        "queue_ack" => this.queue_ack(agent_req).await,
                        "inbox_poll" => this.inbox_poll(agent_req).await,
                        "message" | "interrupt" => this.message_send(agent_req).await,
                        "message_ack" => this.message_ack(agent_req).await,
                        "message_status" => this.message_status_query(agent_req).await,
                        _ => unreachable!(),
                    }
                }
                // agent_list and agent_cleanup: prefixed to avoid collision with worktree
                "agent_list" => {
                    let agent_req = req.to_agent_request("list");
                    this.agent_list(agent_req).await
                }
                "agent_cleanup" => {
                    let agent_req = req.to_agent_request("cleanup");
                    this.agent_cleanup(agent_req).await
                }

                // ---- Factory domain ----
                "spawn_workers" | "shutdown_workers" | "worker_status" | "worker_activity"
                | "clear_context" | "my_context" | "sync_all_workers" | "gc_report"
                | "gc_cleanup" | "epic_status" | "focus_epic" | "remind" | "remind_list"
                | "remind_cancel" => {
                    let factory_req = req.to_factory_request();
                    match action.as_str() {
                        "spawn_workers" => this.factory_spawn_workers(factory_req).await,
                        "shutdown_workers" => this.factory_shutdown_workers(factory_req).await,
                        "worker_status" => this.factory_worker_status(factory_req).await,
                        "clear_context" => this.factory_clear_context(factory_req).await,
                        "my_context" => this.factory_my_context(factory_req).await,
                        "worker_activity" => this.factory_worker_activity(factory_req).await,
                        "sync_all_workers" => this.factory_sync_all_workers(factory_req).await,
                        "gc_report" => this.factory_gc_report(factory_req).await,
                        "gc_cleanup" => this.factory_gc_cleanup(factory_req).await,
                        // cas-8f8f: per-child branch merge-state diagnostic.
                        // Same data source as the epic-close gate so report
                        // and gate cannot disagree.
                        "epic_status" => this.factory_epic_status(factory_req).await,
                        "focus_epic" => this.factory_focus_epic(factory_req).await,
                        "remind" => this.factory_remind(factory_req).await,
                        "remind_list" => this.factory_remind_list(factory_req).await,
                        "remind_cancel" => this.factory_remind_cancel(factory_req).await,
                        _ => unreachable!(),
                    }
                }

                // ---- Worktree domain (prefixed with worktree_) ----
                "worktree_create" | "worktree_list" | "worktree_show" | "worktree_cleanup"
                | "worktree_merge" | "worktree_status" => {
                    let wt_action = action.strip_prefix("worktree_").unwrap();

                    // Gate: require System A (`worktrees.enabled`) for mutating or
                    // detail operations, but let `status`, `list`, and `merge`
                    // through always.
                    //
                    // `status` reports configuration — must work regardless.
                    // `list` must reflect reality: factory (System B) worktrees are
                    // created by `spawn_workers isolate=true` independently of the
                    // System A flag, and the handler distinguishes both systems in
                    // its output (cas-af86). Blocking `list` here was the bug:
                    // it returned a misleading "disabled" message even when workers
                    // were actively running in real git worktrees.
                    //
                    // `merge` is exempt for the same reason (cas-1d11): spawn's
                    // `isolate=true` never checks this flag (it gates on the
                    // separate `--worktrees` factory CLI flag instead, default
                    // on), so System-B worker worktrees exist and need merging
                    // regardless of `worktrees.enabled`. Blocking `merge` here
                    // left supervisors with no CAS-tracked way to fold a spawned
                    // worker's branch back in — the reported fallback was manual
                    // `git worktree add` + merge + push, bypassing factory
                    // tracking/lease/cleanup entirely. `worktree_merge`'s own
                    // handler resolves System A first, then falls back to the
                    // System B `<cas_root>/worktrees/<assignee>` convention, and
                    // returns an accurate "not found" for neither — so removing
                    // this gate never masks a genuine absence, only the false
                    // "disabled" refusal for worktrees that demonstrably exist.
                    //
                    // create / show / cleanup still genuinely require System A —
                    // they're pure WorktreeStore CRUD with no System-B analogue.
                    if wt_action != "status" && wt_action != "list" && wt_action != "merge" {
                        let config = crate::config::Config::load(&this.inner.cas_root)
                            .map_err(|e| {
                                Self::error(
                                    ErrorCode::INTERNAL_ERROR,
                                    format!("Failed to load config: {e}"),
                                )
                            })?;
                        if !config.worktrees_enabled() {
                            return Ok(Self::success(
                                "Worktrees are experimental and disabled by default.\n\n\
                                To enable, add to .cas/config.toml:\n\n\
                                  worktrees:\n\
                                    enabled: true\n\n\
                                Use `coordination action=worktree_status` to see current configuration.",
                            ));
                        }
                    }

                    let wt_req = WorktreeRequest {
                        action: wt_action.to_string(),
                        id: req.id,
                        task_id: req.task_id,
                        all: req.all,
                        status: req.status,
                        orphans: req.orphans,
                        dry_run: req.dry_run,
                        force: req.force,
                        allow_trunk: req.allow_trunk,
                        cleanup: req.cleanup,
                    };
                    match wt_action {
                        "create" => this.worktree_create(wt_req).await,
                        "list" => this.worktree_list(wt_req).await,
                        "show" => this.worktree_show(wt_req).await,
                        "cleanup" => this.worktree_cleanup(wt_req).await,
                        "merge" => this.worktree_merge(wt_req).await,
                        "status" => this.worktree_status(wt_req).await,
                        _ => unreachable!(),
                    }
                }

                _ => Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "Unknown coordination action: '{action}'. Valid actions:\n\
                         Agent: register, unregister, whoami, heartbeat, agent_list, agent_cleanup, session_start, session_end, loop_start, loop_cancel, loop_status, lease_history, queue_notify, queue_poll, queue_peek, queue_ack, inbox_poll, message, message_ack, message_status\n\
                         Factory: spawn_workers, shutdown_workers, worker_status, worker_activity, clear_context, my_context, sync_all_workers, gc_report, gc_cleanup, epic_status, focus_epic, remind, remind_list, remind_cancel\n\
                         Worktree: worktree_create, worktree_list, worktree_show, worktree_cleanup, worktree_merge, worktree_status"
                    ),
                )),
            };

            if let Err(error) = &result {
                let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(
                    &this.inner.cas_root,
                    "error",
                    &[
                        ("tool", "coordination"),
                        ("action", &action),
                        ("target", &event_target),
                        ("task_id", &event_task_id),
                        ("message", error.message.as_ref()),
                    ],
                );
            }

            // Track with domain-specific tool name for backwards-compatible telemetry
            let domain = if action.starts_with("worktree_") {
                "worktree"
            } else if matches!(
                action.as_str(),
                "spawn_workers"
                    | "shutdown_workers"
                    | "worker_status"
                    | "worker_activity"
                    | "clear_context"
                    | "my_context"
                    | "sync_all_workers"
                    | "gc_report"
                    | "gc_cleanup"
                    | "epic_status"
                    | "focus_epic"
                    | "remind"
                    | "remind_list"
                    | "remind_cancel"
            ) {
                "factory"
            } else {
                "agent"
            };
            crate::telemetry::track_mcp_tool(domain, &action, result.is_ok());

            result
        })
        .await
    }

    // ========================================================================
    // cas_search - Search, context, and entity operations
    // ========================================================================

    #[tool(
        description = "Search and context operations. Actions: search (BM25 full-text), retrieval_feedback (explicit retrieval outcome), retrieval_metrics (offline outcome aggregation), context (session context), context_for_subagent, observe (record observation), entity_list, entity_show, entity_extract, code_search (search code symbols), code_show (show symbol details), grep, blame."
    )]
    pub async fn search(
        &self,
        Parameters(req): Parameters<SearchContextRequest>,
    ) -> Result<CallToolResult, McpError> {
        let this = self.clone();
        panic_catch::dispatch_with_catch("search", async move {
            let action = req.action.clone();
            let result = match req.action.as_str() {
                "search" => this.search_impl(req).await,
                "retrieval_feedback" => this.retrieval_feedback_impl(req).await,
                "retrieval_metrics" => this.retrieval_metrics_impl().await,
                "context" => this.context_impl(req).await,
                "context_for_subagent" => this.context_for_subagent_impl(req).await,
                "observe" => this.observe_impl(req).await,
                "entity_list" => this.entity_list_impl(req).await,
                "entity_show" => this.entity_show_impl(req).await,
                "entity_extract" => this.entity_extract_impl(req).await,
                "code_search" => this.code_search_impl(req).await,
                "code_show" => this.code_show_impl(req).await,
                "grep" => this.grep_impl(req).await,
                "blame" => this.blame_impl(req).await,
                _ => Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "Unknown search action: {}. Valid: search, retrieval_feedback, retrieval_metrics, context, context_for_subagent, observe, entity_list, entity_show, entity_extract, code_search, code_show, grep, blame",
                        req.action
                    ),
                )),
            };

            // Track MCP tool usage
            crate::telemetry::track_mcp_tool("search", &action, result.is_ok());

            result
        })
        .await
    }

    // ========================================================================
    // cas_system - System and maintenance operations
    // ========================================================================

    #[tool(
        description = "System operations. Actions: version (CAS version info), doctor (diagnostics), stats, info (system info), reindex (BM25 index), maintenance_run, maintenance_status, config_docs (full config reference), config_search (search configs by query), report_cas_bug (submit CAS bug to GitHub - ANONYMIZE DATA: remove paths, credentials, proprietary code before submitting), proxy_add (add upstream MCP server), proxy_remove (remove server), proxy_list (list servers), proxy_health (credential-free upstream health/backoff state)."
    )]
    pub async fn system(
        &self,
        Parameters(req): Parameters<SystemRequest>,
    ) -> Result<CallToolResult, McpError> {
        let this = self.clone();
        panic_catch::dispatch_with_catch("system", async move {
            // cas-3b51 regression seam: double-underscore action cannot
            // collide with real input; `#[cfg(test)]` strips in release.
            #[cfg(test)]
            if req.action == "__panic_for_test__" {
                panic!("forced test panic from system handler (cas-3b51 regression)");
            }

            let action = req.action.clone();
            let result = match req.action.as_str() {
                "version" => this.system_version().await,
                "doctor" => this.system_doctor(req).await,
                "stats" => this.system_stats(req).await,
                "info" => this.system_info(req).await,
                "reindex" => this.system_reindex(req).await,
                "maintenance_run" => this.system_maintenance_run(req).await,
                "maintenance_status" => this.system_maintenance_status(req).await,
                "config_docs" => this.system_config_docs().await,
                "config_search" => this.system_config_search(req).await,
                "report_cas_bug" => this.system_report_cas_bug(req).await,
                #[cfg(feature = "mcp-proxy")]
                "proxy_add" => this.system_proxy_add(req).await,
                #[cfg(feature = "mcp-proxy")]
                "proxy_remove" => this.system_proxy_remove(req).await,
                #[cfg(feature = "mcp-proxy")]
                "proxy_list" => this.system_proxy_list(req).await,
                #[cfg(feature = "mcp-proxy")]
                "proxy_health" => this.system_proxy_health(req).await,
                _ => Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "Unknown system action: {}. Valid: version, doctor, stats, info, reindex, maintenance_run, maintenance_status, config_docs, config_search, report_cas_bug{}",
                        req.action,
                        if cfg!(feature = "mcp-proxy") { ", proxy_add, proxy_remove, proxy_list, proxy_health" } else { "" }
                    ),
                )),
            };

            // Track MCP tool usage
            crate::telemetry::track_mcp_tool("system", &action, result.is_ok());

            result
        })
        .await
    }

    // ========================================================================
    // cas_verification - Verification operations (task quality gates)
    // ========================================================================

    #[tool(
        description = "Verification operations (task quality gates). Actions: add (record verification result), show (verification details), list (verifications for task), latest (most recent for task)."
    )]
    pub async fn verification(
        &self,
        Parameters(req): Parameters<VerificationRequest>,
    ) -> Result<CallToolResult, McpError> {
        let this = self.clone();
        panic_catch::dispatch_with_catch("verification", async move {
            let action = req.action.clone();
            let result = match req.action.as_str() {
                "add" => this.verification_add(req).await,
                "show" => this.verification_show(req).await,
                "list" => this.verification_list(req).await,
                "latest" => this.verification_latest(req).await,
                _ => Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "Unknown verification action: {}. Valid: add, show, list, latest",
                        req.action
                    ),
                )),
            };

            // Track MCP tool usage
            crate::telemetry::track_mcp_tool("verification", &action, result.is_ok());

            result
        })
        .await
    }

    // ========================================================================
    // cas_team - Team operations for multi-user collaboration
    // ========================================================================

    #[tool(
        description = "Team operations. Actions: list (teams user belongs to), show (team details and stats), members (list team members with roles), sync (trigger team push + pull)."
    )]
    pub async fn team(
        &self,
        Parameters(req): Parameters<TeamRequest>,
    ) -> Result<CallToolResult, McpError> {
        let this = self.clone();
        panic_catch::dispatch_with_catch("team", async move {
            let action = req.action.clone();
            let result = match req.action.as_str() {
                "list" => this.team_list(req).await,
                "show" => this.team_show(req).await,
                "members" => this.team_members(req).await,
                "sync" => this.team_sync(req).await,
                _ => Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "Unknown team action: {}. Valid: list, show, members, sync",
                        req.action
                    ),
                )),
            };

            // Track MCP tool usage
            crate::telemetry::track_mcp_tool("team", &action, result.is_ok());

            result
        })
        .await
    }

    // ========================================================================
    // cas_pattern - Personal patterns (cross-project conventions)
    // ========================================================================

    #[tool(
        description = "Personal pattern operations (cross-project conventions). Actions: create (new pattern), list (with filters), show (by ID), update (modify fields), archive (soft delete), adopt (from rule), helpful (increment), harmful (increment). Team actions (require team_id): team_suggestions (list), team_new_suggestions (pending only), team_create_suggestion, team_share (share personal pattern), team_adopt (adopt suggestion), team_dismiss, team_recommend, team_archive_suggestion, team_suggestion_analytics."
    )]
    pub async fn pattern(
        &self,
        Parameters(req): Parameters<PatternRequest>,
    ) -> Result<CallToolResult, McpError> {
        let this = self.clone();
        panic_catch::dispatch_with_catch("pattern", async move {
            let action = req.action.clone();
            let result = match req.action.as_str() {
                "create" => this.pattern_create(req).await,
                "list" => this.pattern_list(req).await,
                "show" => this.pattern_show(req).await,
                "update" => this.pattern_update(req).await,
                "archive" => this.pattern_archive(req).await,
                "adopt" => this.pattern_adopt(req).await,
                "helpful" => this.pattern_helpful(req).await,
                "harmful" => this.pattern_harmful(req).await,
                "team_suggestions" => this.team_suggestions(req).await,
                "team_new_suggestions" => this.team_new_suggestions(req).await,
                "team_create_suggestion" => this.team_create_suggestion(req).await,
                "team_share" => this.team_share(req).await,
                "team_adopt" => this.team_adopt_suggestion(req).await,
                "team_dismiss" => this.team_dismiss_suggestion(req).await,
                "team_recommend" => this.team_recommend_suggestion(req).await,
                "team_archive_suggestion" => this.team_archive_suggestion(req).await,
                "team_suggestion_analytics" => this.team_suggestion_analytics(req).await,
                _ => Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "Unknown pattern action: {}. Valid: create, list, show, update, archive, adopt, helpful, harmful, team_suggestions, team_new_suggestions, team_create_suggestion, team_share, team_adopt, team_dismiss, team_recommend, team_archive_suggestion, team_suggestion_analytics",
                        req.action
                    ),
                )),
            };

            // Track MCP tool usage
            crate::telemetry::track_mcp_tool("pattern", &action, result.is_ok());

            result
        })
        .await
    }

    // ========================================================================
    // cas_spec - All spec operations
    // ========================================================================

    #[tool(
        description = "Spec operations. Actions: create, show, update, delete, list, approve, reject, supersede, link, unlink, sync."
    )]
    pub async fn spec(
        &self,
        Parameters(req): Parameters<SpecRequest>,
    ) -> Result<CallToolResult, McpError> {
        let this = self.clone();
        panic_catch::dispatch_with_catch("spec", async move {
            let action = req.action.clone();
            let is_mutating = matches!(
                req.action.as_str(),
                "create"
                    | "update"
                    | "delete"
                    | "approve"
                    | "reject"
                    | "supersede"
                    | "link"
                    | "unlink"
                    | "sync"
            );

            let result = match req.action.as_str() {
                "create" => this.spec_create(req).await,
                "show" => this.spec_show(req).await,
                "update" => this.spec_update(req).await,
                "delete" => this.spec_delete(req).await,
                "list" => this.spec_list(req).await,
                "approve" => this.spec_approve(req).await,
                "reject" => this.spec_reject(req).await,
                "supersede" => this.spec_supersede(req).await,
                "link" => this.spec_link(req).await,
                "unlink" => this.spec_unlink(req).await,
                "sync" => this.spec_sync(req).await,
                "get_for_task" => this.spec_get_for_task(req).await,
                _ => Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "Unknown spec action: {}. Valid: create, show, update, delete, list, approve, reject, supersede, link, unlink, sync, get_for_task",
                        req.action
                    ),
                )),
            };

            // Notify client of resource changes (Claude Code 2.1.0+)
            if is_mutating && result.is_ok() {
                this.inner.notify_resources_changed().await;
            }

            // Track MCP tool usage
            crate::telemetry::track_mcp_tool("spec", &action, result.is_ok());

            result
        })
        .await
    }

    // ========================================================================
    // mcp_search - Search across all connected MCP servers
    // ========================================================================

    #[cfg_attr(
        feature = "mcp-proxy",
        tool(
            description = "Search across all tools from all connected MCP servers. Pass a keyword query to filter by tool name and description (case-insensitive). Use 'server:name' prefix to filter by server. Examples: 'screenshot', 'server:github issue', 'file read'."
        )
    )]
    #[cfg_attr(
        not(feature = "mcp-proxy"),
        tool(
            description = "Search across all tools from all connected MCP servers. Write TypeScript code to filter the tool catalog. A typed `tools` array is available with { server, name, description, input_schema } fields."
        )
    )]
    pub async fn mcp_search(
        &self,
        #[allow(unused_variables)] Parameters(req): Parameters<ExecuteRequest>,
    ) -> Result<CallToolResult, McpError> {
        #[cfg(feature = "mcp-proxy")]
        {
            let proxy = self.proxy.as_ref().ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_REQUEST,
                    "MCP proxy not configured. Add upstream servers to .cas/proxy.toml",
                )
            })?;

            match proxy.search(&req.code, req.max_length).await {
                Ok(value) => {
                    let text = serde_json::to_string_pretty(&value).unwrap_or_default();
                    crate::telemetry::track_mcp_tool("mcp_proxy", "search", true);
                    Ok(Self::success(text))
                }
                Err(e) => {
                    crate::telemetry::track_mcp_tool("mcp_proxy", "search", false);
                    Err(Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("MCP search failed: {e}"),
                    ))
                }
            }
        }

        #[cfg(not(feature = "mcp-proxy"))]
        Err(Self::error(
            ErrorCode::INVALID_REQUEST,
            "MCP proxy requires mcp-proxy feature. Build with: cargo build --features mcp-proxy",
        ))
    }

    // ========================================================================
    // mcp_execute - Execute tool calls across connected MCP servers
    // ========================================================================

    #[cfg_attr(
        feature = "mcp-proxy",
        tool(
            description = "Execute tool calls across all connected MCP servers. Use JSON dispatch: {\"server\": \"name\", \"tool\": \"tool_name\", \"args\": {...}} or an array for parallel calls. Also supports dot-call syntax: server.tool_name({\"param\": \"value\"})."
        )
    )]
    #[cfg_attr(
        not(feature = "mcp-proxy"),
        tool(
            description = "Execute TypeScript code that calls tools across all connected MCP servers. Each server is a typed global object (e.g. `canva`, `figma`) where every tool is an async function with typed parameters: `await server.tool_name({ param: value })`. Chain calls sequentially or run them in parallel with Promise.all across different servers."
        )
    )]
    pub async fn mcp_execute(
        &self,
        #[allow(unused_variables)] Parameters(req): Parameters<ExecuteRequest>,
    ) -> Result<CallToolResult, McpError> {
        #[cfg(feature = "mcp-proxy")]
        {
            let proxy = self.proxy.as_ref().ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_REQUEST,
                    "MCP proxy not configured. Add upstream servers to .cas/proxy.toml",
                )
            })?;

            match proxy.execute(&req.code, req.max_length).await {
                Ok(result) => {
                    crate::telemetry::track_mcp_tool("mcp_proxy", "execute", true);
                    let mut content = vec![Content::text(result.text)];
                    for img in result.images {
                        content.push(Content::image(img.data, img.mime_type));
                    }
                    Ok(CallToolResult::success(content))
                }
                Err(e) => {
                    crate::telemetry::track_mcp_tool("mcp_proxy", "execute", false);
                    Err(Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("MCP execute failed: {e}"),
                    ))
                }
            }
        }

        #[cfg(not(feature = "mcp-proxy"))]
        Err(Self::error(
            ErrorCode::INVALID_REQUEST,
            "MCP proxy requires mcp-proxy feature. Build with: cargo build --features mcp-proxy",
        ))
    }
}

// ============================================================================
// Backwards-compatible wrapper methods for tests
// These are not exposed as MCP tools; use `coordination` tool instead.
// ============================================================================

impl CasService {
    /// Wrapper for factory operations (used by tests). Delegates to coordination.
    #[allow(dead_code)]
    pub async fn factory(
        &self,
        Parameters(req): Parameters<FactoryRequest>,
    ) -> Result<CallToolResult, McpError> {
        let action = req.action.clone();
        let result = match action.as_str() {
            "spawn_workers" => self.factory_spawn_workers(req).await,
            "shutdown_workers" => self.factory_shutdown_workers(req).await,
            "worker_status" => self.factory_worker_status(req).await,
            "clear_context" => self.factory_clear_context(req).await,
            "my_context" => self.factory_my_context(req).await,
            "worker_activity" => self.factory_worker_activity(req).await,
            "sync_all_workers" => self.factory_sync_all_workers(req).await,
            "gc_report" => self.factory_gc_report(req).await,
            "gc_cleanup" => self.factory_gc_cleanup(req).await,
            "epic_status" => self.factory_epic_status(req).await,
            "focus_epic" => self.factory_focus_epic(req).await,
            "remind" => self.factory_remind(req).await,
            "remind_list" => self.factory_remind_list(req).await,
            "remind_cancel" => self.factory_remind_cancel(req).await,
            _ => Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("Unknown factory action: {action}"),
            )),
        };
        crate::telemetry::track_mcp_tool("factory", &action, result.is_ok());
        result
    }
}

// ============================================================================
// Implementation methods - delegate to inner CasService
// ============================================================================

pub(crate) mod agent_liveness;
mod agent_search_system;
mod core;
pub(crate) mod factory_ops;
mod factory_remind;
pub(crate) mod orphan_recovery;
mod panic_catch;
#[cfg(test)]
mod panic_regression_test;
mod pattern_ops;
mod server_handler;
mod spec_ops;
mod worktree_verification_team_ops;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Guards `cas serve`'s startup banner and empty-registry guard against
    /// silent registry shrink. If the `#[tool_router]` macro ever stops
    /// emitting a registration (refactor, feature flag, etc.) and the banner /
    /// empty-guard regression sneak in, this test fails immediately without
    /// requiring a full process spawn. See cas-5c05 review T7.
    #[test]
    fn registered_tool_names_includes_canonical_meta_tools() {
        let dir = TempDir::new().unwrap();
        let core = CasCore::with_daemon(dir.path().to_path_buf(), None, None);
        #[cfg(feature = "mcp-proxy")]
        let svc = CasService::new(core, None);
        #[cfg(not(feature = "mcp-proxy"))]
        let svc = CasService::new(core);

        let names = svc.registered_tool_names();

        // Sanity floor: 11 CAS meta-tools (without proxy) plus 2 proxy tools
        // that compile-in regardless of feature gating. If this drops below
        // 11, the registry shrank and `cas serve`'s empty-registry guard is
        // the next line of defense.
        assert!(
            names.len() >= 11,
            "registry shrank — expected at least 11 tools, got {}: {:?}",
            names.len(),
            names
        );
        for required in [
            "memory",
            "task",
            "rule",
            "skill",
            "search",
            "system",
            "coordination",
            "verification",
            "team",
            "pattern",
            "spec",
        ] {
            assert!(
                names.iter().any(|n| n == required),
                "missing canonical tool '{required}' in registry: {names:?}"
            );
        }
    }
}
