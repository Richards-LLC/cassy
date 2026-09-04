use super::deser;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Unified search, context, and entity operations request
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SearchContextRequest {
    /// Action to perform
    #[schemars(
        description = "Action: 'search', 'retrieval_feedback', 'retrieval_metrics' (optional strict agent session_id/CAS_SESSION_ID filter; reports identity and judge availability, distinct retrieved/injected/opened/explicit-used/judge-helpful stages, resolved-outcome quality rates, and session-scoped rolling judge precision), 'skill_impact' (impact_report alias), 'context', 'context_for_subagent', 'observe', 'entity_list', 'entity_show', 'entity_extract', 'code_search', 'code_show', 'grep', 'blame', 'history' (search indexed git commits by text/path/time)"
    )]
    pub action: String,

    /// Search query (for search)
    #[schemars(description = "Search query")]
    #[serde(default)]
    pub query: Option<String>,

    /// Opt into a versioned structured search response with provenance.
    /// Omit for the legacy text response. Currently only version 1 is supported.
    #[schemars(description = "Structured provenance response version for search (currently: 1)")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub provenance_version: Option<usize>,

    /// Retrieval query identity returned by a provenance search.
    #[schemars(description = "Query ID returned by a provenance search")]
    #[serde(default)]
    pub query_id: Option<String>,

    /// Retrieval result identity receiving an explicit outcome.
    #[schemars(description = "Result ID from the identified provenance search")]
    #[serde(default)]
    pub result_id: Option<String>,

    /// Explicit retrieval outcome. `ignored` means observed non-use;
    /// `unresolved` means no use/non-use evidence was available.
    #[schemars(
        description = "Outcome: 'used', 'helpful', 'ignored' (observed non-use), 'corrected', 'harmful', or 'unresolved' (no evidence)"
    )]
    #[serde(default)]
    pub outcome: Option<String>,

    /// Actor recording explicit retrieval feedback. Stored only as a hash.
    #[schemars(description = "Actor identity for feedback attribution (stored as a hash)")]
    #[serde(default)]
    pub actor_id: Option<String>,

    /// Optional opaque ID of the correcting entry/rule/task/code record.
    #[schemars(description = "Opaque correction record ID (required for corrected outcomes)")]
    #[serde(default)]
    pub correction_ref: Option<String>,

    /// Document type filter: entry, task, rule, skill, code_symbol, code_file
    #[schemars(
        description = "Filter by type: 'entry', 'task', 'rule', 'skill', 'code_symbol', 'code_file'"
    )]
    #[serde(default)]
    pub doc_type: Option<String>,

    /// Task ID (for context with task focus)
    #[schemars(description = "Task ID for focused context")]
    #[serde(default)]
    pub task_id: Option<String>,

    /// Max tokens for context
    #[schemars(description = "Maximum tokens for context")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub max_tokens: Option<usize>,

    /// Include related memories
    #[schemars(description = "Include related memories")]
    #[serde(default)]
    pub include_memories: Option<bool>,

    /// Observation content (for observe)
    #[schemars(description = "Content of the observation")]
    #[serde(default)]
    pub content: Option<String>,

    /// Observation type: general, decision, bugfix, feature, refactor, discovery
    #[schemars(
        description = "Observation type: 'general', 'decision', 'bugfix', 'feature', 'refactor', 'discovery'"
    )]
    #[serde(default)]
    pub observation_type: Option<String>,

    /// Source tool (for observe)
    #[schemars(description = "Tool that made the observation")]
    #[serde(default)]
    pub source_tool: Option<String>,

    /// Entity ID (for entity_show)
    #[schemars(description = "Entity ID")]
    #[serde(default)]
    pub id: Option<String>,

    /// Entity type filter: person, project, technology, file, concept, organization, domain
    #[schemars(
        description = "Entity type: 'person', 'project', 'technology', 'file', 'concept', 'organization', 'domain'"
    )]
    #[serde(default)]
    pub entity_type: Option<String>,

    /// Tags filter
    #[schemars(description = "Comma-separated tags")]
    #[serde(default)]
    pub tags: Option<String>,

    /// Scope filter
    #[schemars(description = "Scope: 'global', 'project', or 'all'")]
    #[serde(default)]
    pub scope: Option<String>,

    /// Limit for list/search
    #[schemars(description = "Maximum items to return")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub limit: Option<usize>,

    /// Sort field (for search)
    #[schemars(description = "Sort by: 'relevance' (default), 'created', 'updated'")]
    #[serde(default)]
    pub sort: Option<String>,

    /// Sort order (for search)
    #[schemars(description = "Sort order: 'asc' or 'desc' (default: desc)")]
    #[serde(default)]
    pub sort_order: Option<String>,

    // ========== Code Search Fields ==========
    /// Symbol kind filter (for code_search): function, struct, trait, enum, impl, method, const, type, module
    #[schemars(
        description = "Filter by symbol kind: 'function', 'struct', 'trait', 'enum', 'impl', 'method', 'const', 'type', 'module'"
    )]
    #[serde(default)]
    pub kind: Option<String>,

    /// Language filter (for code_search): rust, typescript, python, go
    #[schemars(description = "Filter by language: 'rust', 'typescript', 'python', 'go'")]
    #[serde(default)]
    pub language: Option<String>,

    /// Include source code in results (for code_search/code_show)
    #[schemars(description = "Include source code in results")]
    #[serde(default)]
    pub include_source: Option<bool>,

    /// Regex pattern for grep search
    #[schemars(description = "Regex pattern for grep action")]
    #[serde(default)]
    pub pattern: Option<String>,

    /// File glob pattern for grep (e.g., "*.rs", "src/**/*.ts")
    #[schemars(description = "File glob pattern to filter files (e.g., '*.rs', 'src/**/*.ts')")]
    #[serde(default)]
    pub glob: Option<String>,

    /// Lines of context before match (for grep)
    #[schemars(description = "Lines of context before each match (grep -B)")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub before_context: Option<usize>,

    /// Lines of context after match (for grep)
    #[schemars(description = "Lines of context after each match (grep -A)")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub after_context: Option<usize>,

    /// Case insensitive search (for grep)
    #[schemars(description = "Case insensitive search")]
    #[serde(default)]
    pub case_insensitive: Option<bool>,

    // ========== Blame Fields ==========
    /// File path for blame action (can include :line or :start-end)
    #[schemars(description = "File path to blame (optionally with :line or :start-end)")]
    #[serde(default)]
    pub file_path: Option<String>,

    /// Start line for blame range
    #[schemars(description = "Start line number for blame range")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub line_start: Option<usize>,

    /// End line for blame range
    #[schemars(description = "End line number for blame range")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub line_end: Option<usize>,

    /// Filter to only AI-generated lines (for blame)
    #[schemars(description = "Show only AI-generated lines")]
    #[serde(default)]
    pub ai_only: Option<bool>,

    /// Include full prompts in blame output
    #[schemars(description = "Include full prompts in blame output")]
    #[serde(default)]
    pub include_prompts: Option<bool>,

    /// Canonical agent session ID filter for retrieval_metrics; hashed before comparison.
    #[schemars(
        description = "Strictly filter retrieval_metrics by one canonical agent session ID/CAS_SESSION_ID (hashed before comparison); factory-session labels and agent names are diagnosed but never widened"
    )]
    #[serde(default)]
    pub session_id: Option<String>,

    // ========== Code-History Fields (action=history) ==========
    /// Path filter for history search
    #[schemars(
        description = "Only commits touching paths containing this substring (for history)"
    )]
    #[serde(default)]
    pub path: Option<String>,

    /// Symbol filter for history search
    #[schemars(
        description = "Only commits touching this exact qualified symbol (for history). Incompletely mapped commits are returned with their explicit mapping verdict rather than silently treated as non-matches."
    )]
    #[serde(default)]
    pub symbol: Option<String>,

    /// Lower time bound for history search
    #[schemars(
        description = "Lower bound for history search: relative (14d, 2w, 6h, 45m), a date (2026-08-01), or RFC3339"
    )]
    #[serde(default)]
    pub since: Option<String>,

    /// Upper time bound for history search
    #[schemars(description = "Upper bound for history search, same formats as 'since'")]
    #[serde(default)]
    pub until: Option<String>,

    /// Include merge commits in history results
    #[schemars(
        description = "Include merge commits in history results (excluded by default: their message is 'Merge branch x')"
    )]
    #[serde(default)]
    pub include_merges: Option<bool>,

    /// Request provenance on history results
    #[schemars(
        description = "Resolve which task/session produced each commit, with the link method and confidence per edge. Coverage is partial and measured (index_status reports it); commits with no populated edge are returned with a stated reason rather than dropped."
    )]
    #[serde(default)]
    pub include_provenance: Option<bool>,
}

/// Unified system operations request
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SystemRequest {
    /// Action to perform
    #[schemars(
        description = "Action: 'version', 'preflight', 'doctor', 'stats', 'info', 'reindex', 'maintenance_run', 'maintenance_status', 'config_docs', 'config_search', 'report_cas_bug', 'proxy_add', 'proxy_remove', 'proxy_list', 'proxy_health'"
    )]
    pub action: String,

    /// Rebuild BM25 index (for reindex)
    #[schemars(description = "Rebuild BM25 full-text search index")]
    #[serde(default)]
    pub bm25: Option<bool>,

    /// Regenerate embeddings (deprecated - semantic search via cloud only)
    #[schemars(
        description = "Deprecated: embeddings are now cloud-only. This parameter is ignored."
    )]
    #[serde(default)]
    pub embeddings: Option<bool>,

    /// Only generate for entries missing embeddings (deprecated)
    #[schemars(
        description = "Deprecated: embeddings are now cloud-only. This parameter is ignored."
    )]
    #[serde(default)]
    pub missing_only: Option<bool>,

    /// Force maintenance run even if not idle
    #[schemars(description = "Force maintenance run even if not idle")]
    #[serde(default)]
    pub force: Option<bool>,

    /// Search query for config_search action
    #[schemars(
        description = "Search query for config_search (searches keys, descriptions, keywords, use cases)"
    )]
    #[serde(default)]
    pub query: Option<String>,

    // ========== Bug Reporting Fields (report_cas_bug) ==========
    /// Bug title (for report_cas_bug)
    #[schemars(
        description = "Brief title describing the bug (anonymize any project-specific info)"
    )]
    #[serde(default)]
    pub title: Option<String>,

    /// Bug description (for report_cas_bug)
    #[schemars(
        description = "Detailed description including steps to reproduce. IMPORTANT: Anonymize paths, remove credentials, avoid proprietary code"
    )]
    #[serde(default)]
    pub description: Option<String>,

    /// Expected behavior (for report_cas_bug)
    #[schemars(description = "What you expected to happen")]
    #[serde(default)]
    pub expected: Option<String>,

    /// Actual behavior (for report_cas_bug)
    #[schemars(description = "What actually happened (anonymize any sensitive output)")]
    #[serde(default)]
    pub actual: Option<String>,

    // ========== Proxy Management Fields (proxy_add/proxy_remove/proxy_list) ==========
    /// Server name for proxy operations
    #[schemars(description = "Server name for proxy_add/proxy_remove")]
    #[serde(default)]
    pub name: Option<String>,

    /// Transport type for proxy_add: 'stdio', 'http', or 'sse'
    #[schemars(description = "Transport type: 'stdio', 'http', or 'sse' (default: stdio)")]
    #[serde(default)]
    pub transport: Option<String>,

    /// URL for http/sse proxy servers
    #[schemars(description = "URL for http/sse transport")]
    #[serde(default)]
    pub url: Option<String>,

    /// Command for stdio proxy servers
    #[schemars(description = "Command for stdio transport")]
    #[serde(default)]
    pub command: Option<String>,

    /// Arguments for stdio proxy command (JSON array of strings)
    #[schemars(
        description = "Arguments for stdio command (JSON array of strings, e.g. '[\"--port\", \"3000\"]')"
    )]
    #[serde(default)]
    pub args: Option<String>,

    /// Environment variables for stdio proxy (JSON object)
    #[schemars(
        description = "Environment variables for stdio command (JSON object, e.g. '{\"API_KEY\": \"...\"}')"
    )]
    #[serde(default)]
    pub env: Option<String>,

    /// Auth token for http/sse proxy servers
    #[schemars(description = "Bearer auth token for http/sse transport")]
    #[serde(default)]
    pub auth: Option<String>,
}

/// Unified verification operations request
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct VerificationRequest {
    /// Action to perform
    #[schemars(description = "Action: 'add', 'show', 'list', 'latest', 'external_verify'")]
    pub action: String,

    /// Verification ID (for show)
    #[schemars(description = "Verification ID")]
    #[serde(default)]
    pub id: Option<String>,

    /// Task ID (for add, list, latest)
    #[schemars(description = "Task ID")]
    #[serde(default)]
    pub task_id: Option<String>,

    /// Status (for add): approved, rejected, error, skipped
    #[schemars(description = "Status: 'approved', 'rejected', 'error', 'skipped'")]
    #[serde(default)]
    pub status: Option<String>,

    /// Summary (for add)
    #[schemars(description = "Verification summary")]
    #[serde(default)]
    pub summary: Option<String>,

    /// Confidence score 0.0-1.0 (for add)
    #[schemars(description = "Confidence score from 0.0 to 1.0")]
    #[serde(default)]
    pub confidence: Option<f32>,

    /// Issues found as JSON array (for add)
    #[schemars(description = "JSON array of issues found")]
    #[serde(default)]
    pub issues: Option<String>,

    /// Files reviewed, comma-separated (for add)
    #[schemars(description = "Comma-separated list of files reviewed")]
    #[serde(default)]
    pub files: Option<String>,

    /// Duration of verification in milliseconds (for add)
    #[schemars(description = "Duration in milliseconds")]
    #[serde(default, deserialize_with = "deser::option_u64")]
    pub duration_ms: Option<u64>,

    /// Limit for list
    #[schemars(description = "Maximum items to return")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub limit: Option<usize>,

    /// Verification type: 'task' (default) or 'epic'
    #[schemars(description = "Verification type: 'task' (default) or 'epic'")]
    #[serde(default)]
    pub verification_type: Option<String>,

    /// Legacy explicit bearer compatibility. New task-verifier children use
    /// a sealed server-side handoff and omit this field.
    #[schemars(
        description = "Legacy explicit task-verifier bearer; new registered verifier children omit it"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier_capability: Option<String>,

    /// Exact dispatch to resolve (required for supervisor-direct add).
    #[schemars(description = "Exact durable verification dispatch ID")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_id: Option<String>,

    /// Supervisor-owned external verification prompt.
    #[schemars(description = "external_verify: bounded read-only verification request")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// Durable local proof that must exist before external delegation.
    #[schemars(description = "external_verify: non-secret local proof reference")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_proof_reference: Option<String>,

    /// Exact checks the structured external response must satisfy.
    #[schemars(description = "external_verify: JSON array of {name, expected} check objects")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_checks: Option<String>,
}

/// Unified distilled-knowledge (project wiki) operations request.
///
/// This is the page surface of the knowledge store (EPIC cas-7d31). It is
/// distinct from `MemoryRequest`'s `opinion_*` actions, which operate on
/// belief-typed memory entries and happen to live in a source file also named
/// `knowledge.rs`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct KnowledgeRequest {
    /// Action to perform
    #[schemars(description = "Action: 'search', 'read', 'write', 'list', 'status'")]
    pub action: String,

    /// Full-text query (for search)
    #[schemars(description = "Full-text query across page titles, snippets and bodies")]
    #[serde(default)]
    pub query: Option<String>,

    /// Page ID (for read)
    #[schemars(description = "Page ID, e.g. 'cas-kn007' (read)")]
    #[serde(default)]
    pub id: Option<String>,

    /// Page path relative to the knowledge dir (for read/write)
    #[schemars(
        description = "Page path relative to the knowledge directory, e.g. 'subsystem/hooks.md' (read, or write to an existing page)"
    )]
    #[serde(default)]
    pub rel_path: Option<String>,

    /// Page title (for write)
    #[schemars(
        description = "Page title — with page_type it determines the canonical path (write)"
    )]
    #[serde(default)]
    pub title: Option<String>,

    /// Page type (for write)
    #[schemars(
        description = "Page category: 'architecture', 'subsystem', 'workflow', 'guide', 'configuration', … (write; default 'guide')"
    )]
    #[serde(default)]
    pub page_type: Option<String>,

    /// Markdown body (for write)
    #[schemars(description = "Markdown body of the page (write)")]
    #[serde(default)]
    pub body: Option<String>,

    /// One-or-two sentence index-injectable summary (for write)
    #[schemars(
        description = "Short summary used for index injection (write; derived from the body when omitted)"
    )]
    #[serde(default)]
    pub snippet: Option<String>,

    /// Comma-separated provenance paths (for write)
    #[schemars(
        description = "Comma-separated source paths this page was written from (write; defaults to 'manual://mcp')"
    )]
    #[serde(default)]
    pub sources: Option<String>,

    /// Include the full markdown body in list/search output
    #[schemars(description = "Include full page bodies in the response (default: false)")]
    #[serde(default, deserialize_with = "deser::option_bool")]
    pub include_body: Option<bool>,

    /// Limit for list/search operations
    #[schemars(description = "Maximum items to return")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub limit: Option<usize>,
}

/// Unified team operations request
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamRequest {
    /// Action to perform
    #[schemars(description = "Action: 'list', 'show', 'members', 'sync'")]
    pub action: String,

    /// Team ID (for show, members, sync)
    #[schemars(description = "Team ID for operations targeting a specific team")]
    #[serde(default)]
    pub team_id: Option<String>,

    /// Limit for list operations
    #[schemars(description = "Maximum items to return")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub limit: Option<usize>,
}

/// Unified factory operations request for dynamic worker management
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FactoryRequest {
    /// Action to perform
    #[schemars(
        description = "Action: 'spawn_workers', 'shutdown_workers', 'hold_worker', 'release_worker', 'worker_status', 'worker_activity', 'clear_context' (real harness context reset — confirmed against the recipient's new session id, or it errors; never reports an unverified success), 'my_context', 'sync_all_workers', 'gc_report', 'gc_cleanup', 'epic_status' (per-child branch merge state), 'focus_epic' (pin or clear displayed epic focus), 'remind' (create reminder), 'remind_list' (list reminders), 'remind_cancel' (cancel a reminder), 'server_start' (run a long-lived server under CAS), 'server_stop', 'server_list' (what is listening and who started it)"
    )]
    pub action: String,

    /// Generic id field used by entity-targeted actions. For
    /// `shutdown_workers`, this accepts either the worker's registered id or
    /// exact display name.
    #[schemars(
        description = "ID for actions that target a specific entity (e.g., worker id/name for shutdown_workers, epic id for epic_status or sync_all_workers)"
    )]
    #[serde(default)]
    pub id: Option<String>,

    /// Number of workers to spawn/shutdown
    #[schemars(
        description = "Number of workers (for spawn: how many to create, for shutdown: how many to stop, 0 = all)"
    )]
    #[serde(default, deserialize_with = "deser::option_i32")]
    pub count: Option<i32>,

    /// Specific worker names (comma-separated)
    #[schemars(
        description = "Comma-separated worker names (optional for spawn, specific targets for shutdown)"
    )]
    #[serde(default)]
    pub worker_names: Option<String>,

    /// Task to pre-assign to the spawned worker (spawn_workers only).
    #[schemars(
        description = "spawn_workers only: task ID to pre-assign to the spawned worker once it boots (single-worker requests only — count must be 1 or worker_names must name exactly one worker). Eliminates the spawn-then-message race: the worker's first `task mine` already shows the task, no follow-up message required."
    )]
    #[serde(default)]
    pub task_id: Option<String>,

    /// Factory delivery route for spawn_workers/focus_epic.
    #[schemars(
        description = "Factory delivery mode: 'push_branch' (default) or 'local_merge' (supervisor merges local worker branches)"
    )]
    #[serde(default)]
    pub delivery_mode: Option<String>,

    /// Target agent for hold_worker, release_worker, clear_context, or remind
    #[schemars(
        description = "Target agent name for hold_worker/release_worker/clear_context/remind (or 'all_workers' for broadcast where supported). For remind: agent who receives the reminder (defaults to self)"
    )]
    #[serde(default)]
    pub target: Option<String>,

    /// Message text for remind
    #[schemars(description = "Message text for remind operations")]
    #[serde(default)]
    pub message: Option<String>,

    /// Explicit consent for shutdown/sync safety overrides.
    #[schemars(
        description = "sync_all_workers: consent to rebase worktrees that are dirty (WIP is stashed and restored) or whose assignee is mid-task; a worktree already mid-rebase is refused regardless. shutdown_workers: required when any selected worker is mid-task or has dirty/unpushed work. Default false."
    )]
    #[serde(default)]
    pub force: Option<bool>,

    /// Preview target-cache reclamation without deleting artifacts.
    #[schemars(
        description = "gc_cleanup: preview exact target-cache candidates and bytes without deleting them (target-cache cleanup defaults to dry-run unless explicitly false)"
    )]
    #[serde(default)]
    pub dry_run: Option<bool>,

    /// Clear the pinned epic focus for focus_epic
    #[schemars(description = "Clear the pinned epic focus (focus_epic only)")]
    #[serde(default)]
    pub clear: Option<bool>,

    /// Branch/ref target for sync operations
    #[schemars(description = "Target branch/ref for sync actions (e.g., 'epic/my-epic')")]
    #[serde(default)]
    pub branch: Option<String>,

    /// Threshold used by cleanup/report actions (seconds)
    #[schemars(description = "Optional threshold in seconds for cleanup/report actions")]
    #[serde(default, deserialize_with = "deser::option_i64")]
    pub older_than_secs: Option<i64>,

    /// Whether spawned workers need isolated worktrees (git worktree per worker)
    #[schemars(
        description = "Whether workers need isolated git worktrees. true gives each worker its own branch and directory. false or omitted shares one mutable checkout/HEAD across workers and is contamination-prone: HEAD can switch between tool calls, commits can land on a foreign worker branch, HEAD:<mine> pushes can graft foreign commits, and skill files can change on disk mid-session. Prefer true; non-isolated spawn receipts warn explicitly."
    )]
    #[serde(default)]
    pub isolate: Option<bool>,

    /// Reminder message to deliver when triggered
    #[schemars(description = "Reminder message to deliver when triggered")]
    #[serde(default)]
    pub remind_message: Option<String>,

    /// Delay in seconds before reminder fires (time-based trigger)
    #[schemars(description = "Delay in seconds before reminder fires (time-based trigger)")]
    #[serde(default, deserialize_with = "deser::option_i64")]
    pub remind_delay_secs: Option<i64>,

    /// Event type that triggers the reminder (event-based trigger). External
    /// durable conditions are `branch_contained_in` with filter
    /// `{"commit":"<sha-or-ref>","target_branch":"main"}` and
    /// `tag_exists` with filter `{"tag":"<tag>"}`; external conditions
    /// require `cross_session=true` and default to no expiry.
    #[schemars(
        description = "Event type that triggers reminder: 'task_completed', 'task_blocked', 'worker_idle', 'epic_completed', 'branch_contained_in', or 'tag_exists'. External conditions require cross_session=true; use JSON filters {commit,target_branch} or {tag}."
    )]
    #[serde(default)]
    pub remind_event: Option<String>,

    /// JSON filter for event matching. External branch/tag conditions use the
    /// schemas documented on `remind_event`.
    #[schemars(
        description = "JSON filter for event matching, e.g. '{\"task_id\":\"cas-a1b2\"}' or '{\"worker\":\"worker-3\"}'"
    )]
    #[serde(default)]
    pub remind_filter: Option<String>,

    /// Reminder ID for cancel operations
    #[schemars(description = "Reminder ID for cancel operations")]
    #[serde(default, deserialize_with = "deser::option_i64")]
    pub remind_id: Option<i64>,

    /// TTL in seconds for the reminder (default: 3600; zero means no expiry)
    #[schemars(
        description = "Time-to-live in seconds for the reminder before auto-expiry (default: 3600; zero means no expiry)"
    )]
    #[serde(default, deserialize_with = "deser::option_i64")]
    pub remind_ttl_secs: Option<i64>,

    /// Allow a reminder to survive its creator's session end. Cross-session
    /// reminders always identify their origin and creation time on delivery.
    #[schemars(
        description = "Opt in to a reminder surviving its creator session (default false). Cross-session deliveries include origin-session and created-at context."
    )]
    #[serde(default)]
    pub cross_session: Option<bool>,

    // ========== Spawn-time worker spec overrides (cas-2992) ==========
    /// Registry lane to resolve for spawn_workers. A lane is mutually
    /// exclusive with explicit cli/model/effort recipe fields.
    #[schemars(
        description = "spawn_workers only: registry lane to resolve (for example 'light', 'standard', 'taste', or 'heavy'). The lane chooses the ordered recipe; do not combine it with cli, model, or effort."
    )]
    #[serde(default)]
    pub lane: Option<String>,

    /// Worker CLI override for spawn_workers ('claude' or 'codex').
    /// Applies to every spawned worker in this request.
    #[schemars(
        description = "Worker CLI to use for spawned workers: 'claude' (default) or 'codex'. Applies to all workers in this spawn request."
    )]
    #[serde(default)]
    pub cli: Option<String>,

    /// Worker model override for spawn_workers.
    /// Applies to every spawned worker in this request.
    #[schemars(
        description = "Worker model override (e.g. 'claude-opus-4-5'). Applies to all workers in this spawn request."
    )]
    #[serde(default)]
    pub model: Option<String>,

    /// Worker reasoning effort override for spawn_workers.
    /// Applies to every spawned worker in this request.
    #[schemars(
        description = "Worker reasoning effort override: 'minimal', 'low', 'medium', 'high', 'xhigh'. Applies to all workers in this spawn request."
    )]
    #[serde(default)]
    pub effort: Option<String>,

    /// Claude configuration directory override for spawn_workers. Applies to
    /// every spawned worker in this request; only Claude workers use it.
    #[schemars(
        description = "Claude configuration directory for spawned Claude workers (for example '~/.claude-alt'). Applies to all workers in this spawn request. Claude-only: Codex/Grok workers ignore it and emit a warning. An explicit value also removes inherited ANTHROPIC_API_KEY so the selected OAuth account is used."
    )]
    #[serde(default)]
    pub config_dir: Option<String>,

    /// Per-worker spawn overrides as a JSON array. Entries are applied in
    /// worker_names/count order after the batch cli/model/effort/config_dir
    /// defaults, for example
    /// `[{"name":"research","cli":"codex","config_dir":"~/.codex-work"}]`.
    #[schemars(
        description = "spawn_workers only: JSON array of per-worker {name?, cli?, model?, effort?, config_dir?} overrides. Entries align with worker_names (or count slots) and override the batch cli/model/effort/config_dir defaults; account directories stay provider-scoped (CLAUDE_CONFIG_DIR for Claude, CODEX_HOME for Codex)."
    )]
    #[serde(default)]
    pub workers: Option<String>,

    // ========== Server registry (cas-7c93, GH #87) ==========
    /// Shell command for `server_start`.
    #[schemars(
        description = "server_start: the shell command to run (for example 'npm run dev'). Runs under `sh -c` from the given cwd; stdout/stderr are captured to a log file, never inherited."
    )]
    #[serde(default)]
    pub command: Option<String>,

    /// Working directory for `server_start`.
    #[schemars(
        description = "server_start: working directory to run the command in (defaults to the current directory)"
    )]
    #[serde(default)]
    pub cwd: Option<String>,

    /// Expected listening port for `server_start`.
    #[schemars(
        description = "server_start: the port the server is expected to listen on. Advisory only — server_list reports the ports actually bound, observed from the process itself."
    )]
    #[serde(default, deserialize_with = "deser::option_i32")]
    pub port: Option<i32>,

    /// Whether a registered server outlives its worker.
    #[schemars(
        description = "server_start: true to place the server outside worker containment so it survives worker teardown (shared/long-lived services). Default false: the server stays in the worker's containment scope and dies with it."
    )]
    #[serde(default)]
    pub shared: Option<bool>,
}

/// Unified coordination operations request combining agent, factory, and worktree operations.
///
/// Agent actions: register, unregister, whoami, heartbeat, agent_list, agent_cleanup,
///   session_start, session_end, loop_start, loop_cancel, loop_status, lease_history,
///   queue_notify, queue_poll, queue_peek, queue_ack, inbox_poll, message,
///   message_ack, message_status.
/// Factory actions: spawn_workers, shutdown_workers, hold_worker, release_worker, worker_status, worker_activity,
///   clear_context, my_context, sync_all_workers, gc_report, gc_cleanup, focus_epic,
///   remind, remind_list, remind_cancel.
/// Worktree actions: worktree_create, worktree_list, worktree_show, worktree_cleanup,
///   worktree_merge, worktree_status.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CoordinationRequest {
    /// Action to perform
    #[schemars(
        description = "Action: agent ops (register, unregister, whoami, heartbeat, agent_list, agent_cleanup, session_start, session_end, loop_start, loop_cancel, loop_status, lease_history, queue_notify, queue_poll, queue_peek, queue_ack, inbox_poll, message, interrupt, message_ack, message_status), factory ops (spawn_workers, shutdown_workers, hold_worker, release_worker, worker_status, worker_activity, clear_context, my_context, sync_all_workers, gc_report, gc_cleanup, focus_epic, remind, remind_list, remind_cancel), worktree ops (worktree_create, worktree_list, worktree_show, worktree_cleanup, worktree_merge, worktree_status). Only available in factory mode. 'interrupt' is shorthand for 'message' with urgent=true (breaks the target's in-flight turn, then injects). shutdown_workers requires force=true for mid-task or dirty/unpushed workers. sync_all_workers skips worktrees that are dirty or whose assignee is mid-task unless force=true, and always refuses one already mid-rebase."
    )]
    pub action: String,

    // ========== Shared Fields ==========
    /// Agent ID, worktree ID, or branch name
    #[schemars(description = "Agent ID or worktree ID/branch name")]
    #[serde(default)]
    pub id: Option<String>,

    /// Task ID (for loop_start, worktree_create, spawn_workers, remind, or a
    /// merge_request message).
    /// A reminder linked to this task is quarantined when it closes unless
    /// `cross_session=true` explicitly keeps it.
    #[schemars(
        description = "Task ID. For loop_start/worktree_create: the task the loop/worktree is scoped to. For spawn_workers: pre-assign this task to the spawned worker (single-worker requests only). For remind: bind stale-context cleanup to this task; close quarantines it unless cross_session=true explicitly keeps it. For message with merge_request=true: identify the parked merge delivery. An open task_id also authorizes the spawn on its own, so a standalone follow-up needs no active EPIC."
    )]
    #[serde(default)]
    pub task_id: Option<String>,

    /// Factory delivery route for spawn_workers/focus_epic.
    #[schemars(
        description = "Factory delivery mode: 'push_branch' (default) or 'local_merge' (supervisor merges local worker branches)"
    )]
    #[serde(default)]
    pub delivery_mode: Option<String>,

    /// Explicit worker merge-request message type. Only this type receives
    /// CAS's structured merge envelope and stale-merge suppression.
    #[schemars(
        description = "message action only: mark this worker-to-supervisor message as a merge request. CAS attaches the cas-merge-request envelope and suppresses it only if its branch tip is already integrated. Omit or false for blockers, questions, close failures, and all other free-form messages."
    )]
    #[serde(default)]
    pub merge_request: Option<bool>,

    /// Explicit blocker escalation (cas-8725). Only this type receives CAS's
    /// `<cas-blocker …>` envelope, which is what wakes an idle supervisor.
    #[schemars(
        description = "message action only: mark this message as a blocker escalation. CAS attaches the cas-blocker envelope so an idle supervisor is woken now instead of reading the row at its next turn. Omit or false for status updates, questions and ordinary traffic."
    )]
    #[serde(default)]
    pub blocker: Option<bool>,

    /// Explicit notification this message acknowledges or answers.
    #[schemars(
        description = "message action only: notification_id of the direct message this response explicitly acknowledges. CAS validates the endpoints, confirms that exact message, and includes the reference in the delivered reply."
    )]
    #[serde(default, deserialize_with = "deser::option_i64")]
    pub in_reply_to: Option<i64>,

    /// Target agent name for hold_worker/release_worker/clear_context/message/remind
    #[schemars(
        description = "Target agent name for hold_worker/release_worker/clear_context/message/remind (or 'all_workers' for broadcast where supported). For remind: agent who receives the reminder (defaults to self)"
    )]
    #[serde(default)]
    pub target: Option<String>,

    /// Message content for message action
    #[schemars(
        description = "Message text to send to the target agent, or content for message action"
    )]
    #[serde(default)]
    pub message: Option<String>,

    /// Short summary of the message (shown in UI notifications)
    #[schemars(
        description = "A short one-line summary of the message, shown as a preview in the UI"
    )]
    #[serde(default)]
    pub summary: Option<String>,

    /// Urgent (interrupt-and-redirect) delivery for message/interrupt actions.
    #[schemars(
        description = "When true (or with action=interrupt), deliver the message URGENTLY: break the target worker's in-flight turn (Esc) and inject the correction as its next prompt, bypassing the Claude Code inbox even in agent-teams mode. This DISCARDS the worker's in-flight reasoning/partial work — use only when the worker is demonstrably off the rails. Default false = normal, non-disruptive inbox/queue delivery."
    )]
    #[serde(default)]
    pub urgent: Option<bool>,

    /// Force operation (shutdown, worktree cleanup/merge, gc_cleanup, sync_all_workers)
    #[schemars(
        description = "Force operation even with uncommitted changes (dirty worktree cleanup/merge). For sync_all_workers: consent to rebase worktrees that are dirty (WIP is stashed and restored) or whose assignee is mid-task — without it those are skipped; a worktree already mid-rebase is refused either way. Does NOT authorize trunk as a merge target — use allow_trunk for that (cas-0b32)."
    )]
    #[serde(default)]
    pub force: Option<bool>,

    /// Explicit intent to use the configured trunk fallback when no epic or
    /// task WorkTarget is declared (cas-0b32/cas-84df). Independent of `force`
    /// so authorizing trunk never bypasses dirty-worktree protection.
    #[schemars(
        description = "worktree_merge only: authorize a genuine fallback to the configured trunk branch when no epic or task WorkTarget is declared. A declared WorkTarget does not require this flag. Refusals name the resolved trunk destination before authorization, and successful trunk pushes carry a loud warning. Separate from force= (dirty worktree override)."
    )]
    #[serde(default)]
    pub allow_trunk: Option<bool>,

    /// worktree_merge only: remove the worktree directory and delete the
    /// factory branch after a successful merge (cas-369f). Independent of
    /// `force` (dirty-tree override). Default for System-B factory workers is
    /// preserve (mid-session merges leave the worker cwd intact); pass
    /// `cleanup=true` for end-of-lane consume.
    #[schemars(
        description = "worktree_merge only: remove worktree + delete branch after merge (end-of-lane). Separate from force= (dirty only). System-B default is preserve so mid-epic merges do not destroy a live worker cwd."
    )]
    #[serde(default)]
    pub cleanup: Option<bool>,

    /// Clear the pinned epic focus for focus_epic
    #[schemars(description = "Clear the pinned epic focus (focus_epic only)")]
    #[serde(default)]
    pub clear: Option<bool>,

    /// Maximum items to return
    #[schemars(description = "Maximum items to return")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub limit: Option<usize>,

    // ========== Agent Fields ==========
    /// Human-readable agent name (for register)
    #[schemars(description = "Human-readable name for the agent")]
    #[serde(default)]
    pub name: Option<String>,

    /// Agent type: primary, sub_agent, worker, ci
    #[schemars(description = "Agent type: 'primary', 'sub_agent', 'worker', 'ci'")]
    #[serde(default)]
    pub agent_type: Option<String>,

    /// Parent agent ID (for sub-agents)
    #[schemars(description = "Parent agent ID if this is a sub-agent")]
    #[serde(default)]
    pub parent_id: Option<String>,

    /// Session ID from Claude Code (used as agent ID)
    #[schemars(description = "Session ID from Claude Code (used as agent ID)")]
    #[serde(default)]
    pub session_id: Option<String>,

    /// Loop prompt (for loop_start)
    #[schemars(description = "The prompt to repeat each iteration")]
    #[serde(default)]
    pub prompt: Option<String>,

    /// Max iterations (for loop_start, 0 = unlimited)
    #[schemars(description = "Maximum iterations (0 = unlimited)")]
    #[serde(default, deserialize_with = "deser::option_u32")]
    pub max_iterations: Option<u32>,

    /// Completion promise (for loop_start)
    #[schemars(description = "Text that signals completion")]
    #[serde(default)]
    pub completion_promise: Option<String>,

    /// Reason (for loop_cancel)
    #[schemars(description = "Reason for cancelling")]
    #[serde(default)]
    pub reason: Option<String>,

    /// Stale threshold seconds (for agent_cleanup)
    #[schemars(description = "Seconds since last heartbeat to consider stale")]
    #[serde(default, deserialize_with = "deser::option_i64")]
    pub stale_threshold_secs: Option<i64>,

    /// Supervisor ID (for queue operations)
    #[schemars(description = "Supervisor agent ID for queue operations")]
    #[serde(default)]
    pub supervisor_id: Option<String>,

    /// Event type (for queue_notify)
    #[schemars(
        description = "Event type for notification: 'task_completed', 'task_blocked', 'worker_died', 'worker_idle'"
    )]
    #[serde(default)]
    pub event_type: Option<String>,

    /// Payload (for queue_notify)
    #[schemars(description = "JSON payload containing event details")]
    #[serde(default)]
    pub payload: Option<String>,

    /// Notification priority (for queue_notify)
    #[schemars(
        description = "Notification priority: 'critical' (0), 'high' (1), 'normal' (2, default)"
    )]
    #[serde(default)]
    pub priority: Option<String>,

    /// Notification ID (for queue_ack, message_ack, message_status)
    #[schemars(
        description = "Notification ID for queue_ack, message_ack, or message_status. The message action returns this value as notification_id."
    )]
    #[serde(default, deserialize_with = "deser::option_i64")]
    pub notification_id: Option<i64>,

    // ========== Factory Fields ==========
    /// Number of workers (for spawn/shutdown)
    #[schemars(
        description = "Number of workers (for spawn: how many to create, for shutdown: how many to stop, 0 = all)"
    )]
    #[serde(default, deserialize_with = "deser::option_i32")]
    pub count: Option<i32>,

    /// Comma-separated worker names
    #[schemars(
        description = "Comma-separated worker names (optional for spawn, specific targets for shutdown)"
    )]
    #[serde(default)]
    pub worker_names: Option<String>,

    /// Target branch/ref for sync actions
    #[schemars(description = "Target branch/ref for sync actions (e.g., 'epic/my-epic')")]
    #[serde(default)]
    pub branch: Option<String>,

    /// Threshold in seconds for cleanup/report actions
    #[schemars(description = "Optional threshold in seconds for cleanup/report actions")]
    #[serde(default, deserialize_with = "deser::option_i64")]
    pub older_than_secs: Option<i64>,

    /// Whether workers need isolated git worktrees
    #[schemars(
        description = "Whether workers need isolated git worktrees. true gives each worker its own branch and directory. false or omitted shares one mutable checkout/HEAD across workers and is contamination-prone: HEAD can switch between tool calls, commits can land on a foreign worker branch, HEAD:<mine> pushes can graft foreign commits, and skill files can change on disk mid-session. Prefer true; non-isolated spawn receipts warn explicitly."
    )]
    #[serde(default)]
    pub isolate: Option<bool>,

    /// Registry lane to resolve for spawn_workers. Mutually exclusive with
    /// explicit cli/model/effort recipe fields.
    #[schemars(
        description = "spawn_workers only: registry lane to resolve (for example 'light', 'standard', 'taste', or 'heavy'). The lane chooses the ordered recipe; do not combine it with cli, model, or effort."
    )]
    #[serde(default)]
    pub lane: Option<String>,

    /// Worker CLI override for spawn_workers ('claude' or 'codex').
    /// Applies to every spawned worker in this request.
    #[schemars(
        description = "Worker CLI to use for spawned workers: 'claude' (default) or 'codex'. Applies to all workers in this spawn_workers request."
    )]
    #[serde(default)]
    pub cli: Option<String>,

    /// Worker model override for spawn_workers.
    /// Applies to every spawned worker in this request.
    #[schemars(
        description = "Worker model override (e.g. 'claude-opus-4-5'). Applies to all workers in this spawn_workers request."
    )]
    #[serde(default)]
    pub model: Option<String>,

    /// Worker reasoning effort override for spawn_workers.
    /// Applies to every spawned worker in this request.
    #[schemars(
        description = "Worker reasoning effort override: 'minimal', 'low', 'medium', 'high', 'xhigh'. Applies to all workers in this spawn_workers request."
    )]
    #[serde(default)]
    pub effort: Option<String>,

    /// Claude configuration directory override for spawn_workers. Applies to
    /// every spawned worker in this request; only Claude workers use it.
    #[schemars(
        description = "Claude configuration directory for spawned Claude workers (for example '~/.claude-alt'). Applies to all workers in this spawn_workers request. Claude-only: Codex/Grok workers ignore it and emit a warning. An explicit value also removes inherited ANTHROPIC_API_KEY so the selected OAuth account is used."
    )]
    #[serde(default)]
    pub config_dir: Option<String>,

    /// Per-worker spawn overrides as a JSON array. Entries align with
    /// worker_names/count slots and override batch spawn defaults.
    #[schemars(
        description = "spawn_workers only: JSON array of per-worker {name?, cli?, model?, effort?, config_dir?} overrides. Entries align with worker_names (or count slots) and override batch cli/model/effort/config_dir defaults."
    )]
    #[serde(default)]
    pub workers: Option<String>,

    /// Reminder message to deliver when triggered
    #[schemars(description = "Reminder message to deliver when triggered")]
    #[serde(default)]
    pub remind_message: Option<String>,

    /// Delay in seconds before reminder fires (time-based trigger)
    #[schemars(description = "Delay in seconds before reminder fires (time-based trigger)")]
    #[serde(default, deserialize_with = "deser::option_i64")]
    pub remind_delay_secs: Option<i64>,

    /// Event type that triggers reminder. External durable conditions are
    /// `branch_contained_in` with `{\"commit\":\"<sha-or-ref>\",\"target_branch\":\"main\"}`
    /// and `tag_exists` with `{\"tag\":\"<tag>\"}`; both require
    /// `cross_session=true` and default to no expiry.
    #[schemars(
        description = "Event type that triggers reminder: 'task_completed', 'task_blocked', 'worker_idle', 'epic_completed', 'branch_contained_in', or 'tag_exists'. External conditions require cross_session=true; use JSON filters {commit,target_branch} or {tag}."
    )]
    #[serde(default)]
    pub remind_event: Option<String>,

    /// JSON filter for event matching. External branch/tag conditions use the
    /// schemas documented on `remind_event`.
    #[schemars(
        description = "JSON filter for event matching, e.g. '{\"task_id\":\"cas-a1b2\"}' or '{\"worker\":\"worker-3\"}'"
    )]
    #[serde(default)]
    pub remind_filter: Option<String>,

    /// Reminder ID for cancel operations
    #[schemars(description = "Reminder ID for cancel operations")]
    #[serde(default, deserialize_with = "deser::option_i64")]
    pub remind_id: Option<i64>,

    /// Time-to-live in seconds for the reminder (default: 3600; zero means no expiry)
    #[schemars(
        description = "Time-to-live in seconds for the reminder before auto-expiry (default: 3600; zero means no expiry)"
    )]
    #[serde(default, deserialize_with = "deser::option_i64")]
    pub remind_ttl_secs: Option<i64>,

    /// Explicitly keep a reminder across its creator's session end and a
    /// linked task close. Delivery includes origin, creation-time, and task
    /// status context.
    #[schemars(
        description = "Explicitly keep a reminder across creator session end and linked task close (default false). Deliveries include origin-session, created-at, and linked-task status context."
    )]
    #[serde(default)]
    pub cross_session: Option<bool>,

    // ========== Worktree Fields ==========
    /// Show all worktrees including removed/merged (for worktree_list)
    #[schemars(description = "Show all worktrees including removed/merged")]
    #[serde(default)]
    pub all: Option<bool>,

    /// Worktree status filter (for worktree_list)
    #[schemars(
        description = "Filter by status: 'active', 'merged', 'abandoned', 'conflict', 'removed'"
    )]
    #[serde(default)]
    pub status: Option<String>,

    /// Show only orphaned worktrees (for worktree_list)
    #[schemars(description = "Show only orphaned worktrees")]
    #[serde(default)]
    pub orphans: Option<bool>,

    /// Preview cleanup without making changes.
    #[schemars(
        description = "Preview cleanup without making changes (worktree_cleanup; gc_cleanup target caches default to preview unless explicitly false)"
    )]
    #[serde(default)]
    pub dry_run: Option<bool>,

    // ========== Server registry (cas-7c93, GH #87) ==========
    /// Shell command for `server_start`.
    #[schemars(
        description = "server_start: the shell command to run (for example 'npm run dev'). Runs under `sh -c` from the given cwd; stdout/stderr are captured to a log file, never inherited."
    )]
    #[serde(default)]
    pub command: Option<String>,

    /// Working directory for `server_start`.
    #[schemars(
        description = "server_start: working directory to run the command in (defaults to the current directory)"
    )]
    #[serde(default)]
    pub cwd: Option<String>,

    /// Expected listening port for `server_start`.
    #[schemars(
        description = "server_start: the port the server is expected to listen on. Advisory only — server_list reports the ports actually bound, observed from the process itself."
    )]
    #[serde(default, deserialize_with = "deser::option_i32")]
    pub port: Option<i32>,

    /// Whether a registered server outlives its worker.
    #[schemars(
        description = "server_start: true to place the server outside worker containment so it survives worker teardown (shared/long-lived services). Default false: the server stays in the worker's containment scope and dies with it."
    )]
    #[serde(default)]
    pub shared: Option<bool>,
}

/// Request type for MCP proxy execute/search operations.
///
/// Used by both `mcp_search` (discover tools) and `mcp_execute` (call tools).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[schemars(title = "ExecuteRequest")]
pub struct ExecuteRequest {
    /// TypeScript code to execute.
    ///
    /// For `mcp_search`: filter code against a typed `tools` array.
    /// For `mcp_execute`: call tools across connected servers as typed async functions.
    #[schemars(
        description = "TypeScript code to execute. Each connected server is a typed global object where every tool is an async function. Type declarations are auto-generated from tool schemas. Chain calls sequentially: await chrome_devtools.navigate_page({ url: \"https://example.com\" }); const screenshot = await chrome_devtools.take_screenshot({ format: \"png\" }); return screenshot; Or run calls in parallel with Promise.all: const [issues, designs] = await Promise.all([github.list_issues({ repo: \"myorg/app\" }), canva.list_designs({})]);"
    )]
    pub code: String,

    /// Max response length in characters. Default: 40000.
    #[schemars(
        description = "Max response length in characters. Default: 40000. Use your code to extract only what you need rather than increasing this."
    )]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub max_length: Option<usize>,
}

impl CoordinationRequest {
    /// Convert to AgentRequest, mapping agent_list→list, agent_cleanup→cleanup
    pub fn to_agent_request(&self, action: &str) -> super::AgentRequest {
        super::AgentRequest {
            action: action.to_string(),
            id: self.id.clone(),
            name: self.name.clone(),
            agent_type: self.agent_type.clone(),
            parent_id: self.parent_id.clone(),
            session_id: self.session_id.clone(),
            task_id: self.task_id.clone(),
            merge_request: self.merge_request,
            blocker: self.blocker,
            in_reply_to: self.in_reply_to,
            prompt: self.prompt.clone(),
            max_iterations: self.max_iterations,
            completion_promise: self.completion_promise.clone(),
            reason: self.reason.clone(),
            stale_threshold_secs: self.stale_threshold_secs,
            limit: self.limit,
            supervisor_id: self.supervisor_id.clone(),
            event_type: self.event_type.clone(),
            payload: self.payload.clone(),
            priority: self.priority.clone(),
            notification_id: self.notification_id,
            target: self.target.clone(),
            message: self.message.clone(),
            summary: self.summary.clone(),
            urgent: self.urgent,
        }
    }

    /// Convert to FactoryRequest
    pub fn to_factory_request(&self) -> super::FactoryRequest {
        super::FactoryRequest {
            action: self.action.clone(),
            id: self.id.clone(),
            count: self.count,
            worker_names: self.worker_names.clone(),
            task_id: self.task_id.clone(),
            delivery_mode: self.delivery_mode.clone(),
            target: self.target.clone(),
            message: self.message.clone(),
            force: self.force,
            dry_run: self.dry_run,
            clear: self.clear,
            branch: self.branch.clone(),
            older_than_secs: self.older_than_secs,
            isolate: self.isolate,
            remind_message: self.remind_message.clone(),
            remind_delay_secs: self.remind_delay_secs,
            remind_event: self.remind_event.clone(),
            remind_filter: self.remind_filter.clone(),
            remind_id: self.remind_id,
            remind_ttl_secs: self.remind_ttl_secs,
            cross_session: self.cross_session,
            lane: self.lane.clone(),
            cli: self.cli.clone(),
            model: self.model.clone(),
            effort: self.effort.clone(),
            config_dir: self.config_dir.clone(),
            workers: self.workers.clone(),
            command: self.command.clone(),
            cwd: self.cwd.clone(),
            port: self.port,
            shared: self.shared,
        }
    }
}
