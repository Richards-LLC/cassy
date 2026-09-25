//! Accepted `action` values for each multi-action MCP tool.
//!
//! Each list is the tool's dispatch table in cas-cli
//! (`cas-cli/src/mcp/tools/service/mod.rs`), in dispatch order, plus the
//! aliases that dispatch canonicalizes before matching. The request structs
//! publish these lists as a JSON Schema `enum` on their `action` field, so a
//! model sees one validated list instead of a prose list that drifts from the
//! dispatch. `cas-cli/tests/mcp_action_surface_test.rs` pins every list to its
//! dispatch table.

use schemars::{Schema, SchemaGenerator, json_schema};

pub const MEMORY_ACTIONS: &[&str] = &[
    "remember",
    "get",
    "list",
    "update",
    "delete",
    "archive",
    "unarchive",
    "helpful",
    "harmful",
    "mark_reviewed",
    "recent",
    "set_tier",
    "opinion_reinforce",
    "opinion_weaken",
    "opinion_contradict",
];

pub const TASK_ACTIONS: &[&str] = &[
    "create",
    "proposal_inbox",
    "proposal_accept",
    "proposal_reject",
    "proposal_reconcile",
    "show",
    "get",
    "update",
    "start",
    "close",
    "cancel",
    "reopen",
    "request_changes",
    "delete",
    "list",
    "ready",
    "blocked",
    "notes",
    "dep_add",
    "dep_remove",
    "dep_list",
    "claim",
    "release",
    "reset",
    "transfer",
    "available",
    "mine",
];

/// `(alias, canonical)` pairs the task dispatch rewrites before matching.
pub const TASK_ACTION_ALIASES: &[(&str, &str)] = &[("get", "show")];

pub const RULE_ACTIONS: &[&str] = &[
    "create",
    "show",
    "update",
    "delete",
    "history",
    "restore",
    "list",
    "list_all",
    "helpful",
    "promote",
    "harmful",
    "sync",
    "check_similar",
];

pub const SKILL_ACTIONS: &[&str] = &[
    "create", "show", "update", "delete", "history", "restore", "list", "list_all", "enable",
    "disable", "sync", "use",
];

/// Actions of the agent-facing `coordination` tool (D2 split, cas-8563b):
/// identity, messaging and reminders, which every worker needs.
pub const COORDINATION_ACTIONS: &[&str] = &[
    "register",
    "unregister",
    "whoami",
    "heartbeat",
    "session_start",
    "session_end",
    "inbox_poll",
    "inbox",
    "message",
    "interrupt",
    "message_ack",
    "message_status",
    "remind",
    "remind_list",
    "remind_cancel",
    "my_context",
];

/// Actions of the supervisor `factory` tool (D2 split, cas-8563b): fleet,
/// worktree, server, database, loop and queue control. `coordination` still
/// accepts each of them for one release as a deprecated alias.
pub const FACTORY_ACTIONS: &[&str] = &[
    // Fleet
    "spawn_workers",
    "shutdown_workers",
    "recycle_worker",
    "hold_worker",
    "release_worker",
    "worker_status",
    "worker_activity",
    "sweep_tasks",
    "clear_context",
    "sync_all_workers",
    "gc_report",
    "gc_cleanup",
    "epic_status",
    "focus_epic",
    "restart_spawn_queue",
    "agent_list",
    "agent_cleanup",
    "lease_history",
    // Servers
    "server_start",
    "server_stop",
    "server_list",
    // Disposable database branches
    "db_branch_create",
    "db_branch_show",
    "db_branch_delete",
    // Worktrees
    "worktree_create",
    "worktree_list",
    "worktree_show",
    "worktree_cleanup",
    "worktree_merge",
    "worktree_status",
    // Loops and queues
    "loop_start",
    "loop_cancel",
    "loop_status",
    "queue_notify",
    "queue_poll",
    "queue_peek",
    "queue_ack",
];

/// Every action `CoordinationRequest` deserializes: the `coordination`
/// actions plus the `factory` actions both tools share the request type for.
/// Each tool publishes only its own list (`tool_schema` narrows the enum).
pub fn coordination_request_actions() -> Vec<&'static str> {
    [COORDINATION_ACTIONS, FACTORY_ACTIONS].concat()
}

/// `(alias, canonical)` pairs the coordination dispatch rewrites before
/// matching. `interrupt` is not listed: it has its own dispatch arm.
pub const COORDINATION_ACTION_ALIASES: &[(&str, &str)] = &[("inbox", "inbox_poll")];

pub const SEARCH_ACTIONS: &[&str] = &[
    "search",
    "retrieval_feedback",
    "retrieval_metrics",
    "skill_impact",
    "impact_report",
    "context",
    "context_for_subagent",
    "observe",
    "entity_list",
    "entity_show",
    "entity_extract",
    "code_search",
    "code_show",
    "grep",
    "blame",
    "history",
];

pub const SYSTEM_ACTIONS: &[&str] = &[
    "version",
    "preflight",
    "doctor",
    "stats",
    "info",
    "reindex",
    "maintenance_run",
    "maintenance_status",
    "config_docs",
    "config_search",
    "report_cas_bug",
];

/// System actions dispatched only in builds with the `mcp-proxy` feature.
pub const SYSTEM_PROXY_ACTIONS: &[&str] =
    &["proxy_add", "proxy_remove", "proxy_list", "proxy_health"];

pub const VERIFICATION_ACTIONS: &[&str] = &[
    "add",
    "show",
    "list",
    "latest",
    "qa_record",
    "qa_waive",
    "qa_status",
];

/// Verification actions dispatched only in builds with the `mcp-proxy` feature.
pub const VERIFICATION_PROXY_ACTIONS: &[&str] = &["external_verify"];

pub const ARTIFACT_ACTIONS: &[&str] = &["publish", "show", "list"];

pub const KNOWLEDGE_ACTIONS: &[&str] = &["search", "read", "write", "list", "status"];

pub const TEAM_ACTIONS: &[&str] = &["list", "show", "members", "sync"];

pub const PATTERN_ACTIONS: &[&str] = &[
    "create",
    "list",
    "show",
    "update",
    "archive",
    "adopt",
    "helpful",
    "harmful",
    "team_suggestions",
    "team_new_suggestions",
    "team_create_suggestion",
    "team_share",
    "team_adopt",
    "team_dismiss",
    "team_recommend",
    "team_archive_suggestion",
    "team_suggestion_analytics",
];

pub const SPEC_ACTIONS: &[&str] = &[
    "create",
    "show",
    "update",
    "delete",
    "list",
    "approve",
    "reject",
    "supersede",
    "link",
    "unlink",
    "sync",
    "get_for_task",
];

/// Values accepted by `memory action=remember entry_type=`.
pub const MEMORY_ENTRY_TYPES: &[&str] = &[
    "learning",
    "preference",
    "context",
    "observation",
    "handoff",
];

/// Rewrite an alias to its canonical action; any other value is returned as is.
pub fn canonical_action<'a>(aliases: &[(&'static str, &'static str)], action: &'a str) -> &'a str {
    aliases
        .iter()
        .find(|(alias, _)| *alias == action)
        .map(|(_, canonical)| *canonical)
        .unwrap_or(action)
}

fn string_enum(values: &[&str]) -> Schema {
    json_schema!({
        "type": "string",
        "enum": values,
    })
}

pub fn memory_action_schema(_: &mut SchemaGenerator) -> Schema {
    string_enum(MEMORY_ACTIONS)
}

pub fn task_action_schema(_: &mut SchemaGenerator) -> Schema {
    string_enum(TASK_ACTIONS)
}

pub fn rule_action_schema(_: &mut SchemaGenerator) -> Schema {
    string_enum(RULE_ACTIONS)
}

pub fn skill_action_schema(_: &mut SchemaGenerator) -> Schema {
    string_enum(SKILL_ACTIONS)
}

pub fn coordination_action_schema(_: &mut SchemaGenerator) -> Schema {
    string_enum(&coordination_request_actions())
}

pub fn search_action_schema(_: &mut SchemaGenerator) -> Schema {
    string_enum(SEARCH_ACTIONS)
}

pub fn system_action_schema(_: &mut SchemaGenerator) -> Schema {
    string_enum(&system_actions())
}

pub fn verification_action_schema(_: &mut SchemaGenerator) -> Schema {
    string_enum(&verification_actions())
}

pub fn artifact_action_schema(_: &mut SchemaGenerator) -> Schema {
    string_enum(ARTIFACT_ACTIONS)
}

pub fn knowledge_action_schema(_: &mut SchemaGenerator) -> Schema {
    string_enum(KNOWLEDGE_ACTIONS)
}

pub fn team_action_schema(_: &mut SchemaGenerator) -> Schema {
    string_enum(TEAM_ACTIONS)
}

pub fn pattern_action_schema(_: &mut SchemaGenerator) -> Schema {
    string_enum(PATTERN_ACTIONS)
}

pub fn spec_action_schema(_: &mut SchemaGenerator) -> Schema {
    string_enum(SPEC_ACTIONS)
}

pub fn memory_entry_type_schema(_: &mut SchemaGenerator) -> Schema {
    string_enum(MEMORY_ENTRY_TYPES)
}

/// System actions this build dispatches.
pub fn system_actions() -> Vec<&'static str> {
    feature_gated(SYSTEM_ACTIONS, SYSTEM_PROXY_ACTIONS)
}

/// Verification actions this build dispatches.
pub fn verification_actions() -> Vec<&'static str> {
    feature_gated(VERIFICATION_ACTIONS, VERIFICATION_PROXY_ACTIONS)
}

fn feature_gated(base: &[&'static str], proxy_only: &[&'static str]) -> Vec<&'static str> {
    let mut actions = base.to_vec();
    if cfg!(feature = "mcp-proxy") {
        actions.extend_from_slice(proxy_only);
    }
    actions
}
