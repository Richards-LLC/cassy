use std::str::FromStr;

use rmcp::schemars::JsonSchema;
use serde::Deserialize;

use cas_types::TaskDepth;

use crate::mcp::tools::types::defaults::{
    default_dep_type, default_note_type, default_priority, default_subagent_tokens,
    default_task_type, default_true,
};

/// Allowed values for `execution_note` (cas-7fc1). Validated at the MCP
/// tool layer rather than via a SQL CHECK constraint so new values can be
/// added without a schema migration.
pub const EXECUTION_NOTE_VALUES: &[&str] =
    &["test-first", "characterization-first", "additive-only"];

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskStartRequest {
    /// Task ID
    #[schemars(description = "Task ID to start")]
    pub id: String,

    /// Return a context-affordable response containing this task's notes but
    /// no parent-epic or sibling-task payload.
    #[schemars(
        description = "Return a size-bounded start response with this task's notes only; omit parent-epic and sibling-task context"
    )]
    #[serde(default)]
    pub brief: Option<bool>,
}

/// Validate and normalize an incoming `execution_note` parameter. Returns
/// - `Ok(None)` if the input is absent or an empty string (clear/omitted)
/// - `Ok(Some(v))` for an accepted enum value
/// - `Err(message)` otherwise, with a human-readable hint listing the
///   allowed values
pub fn validate_execution_note(value: Option<&str>) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(s) if s.is_empty() => Ok(None),
        Some(s) if EXECUTION_NOTE_VALUES.contains(&s) => Ok(Some(s.to_string())),
        Some(s) => Err(format!(
            "Invalid execution_note: '{}'. Must be one of: {} (or empty/absent to clear)",
            s,
            EXECUTION_NOTE_VALUES.join(", ")
        )),
    }
}

/// Validate and parse an incoming `depth` parameter (EPIC cas-1255). Returns
/// - `Ok(None)` if the input is absent or an empty string (no-op / default)
/// - `Ok(Some(depth))` for an accepted value (`light` / `deep`)
/// - `Err(message)` otherwise, with a human-readable hint
pub fn validate_depth(value: Option<&str>) -> Result<Option<TaskDepth>, String> {
    match value.map(str::trim) {
        None => Ok(None),
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => TaskDepth::from_str(s).map(Some).map_err(|_| {
            format!("Invalid depth: '{s}'. Must be one of: light, deep (or empty/absent)")
        }),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskCreateRequest {
    /// Task title
    #[schemars(description = "Short descriptive title for the task")]
    pub title: String,

    /// Task description
    #[schemars(description = "Detailed description of what needs to be done")]
    #[serde(default)]
    pub description: Option<String>,

    /// Priority (0-4)
    #[schemars(description = "Priority: 0=Critical, 1=High, 2=Medium (default), 3=Low, 4=Backlog")]
    #[serde(default = "default_priority")]
    pub priority: u8,

    /// Task type
    #[schemars(description = "Type: 'task' (default), 'bug', 'feature', 'epic', 'chore', 'spike'")]
    #[serde(default = "default_task_type")]
    pub task_type: String,

    /// Labels
    #[schemars(description = "Comma-separated labels for categorization")]
    #[serde(default)]
    pub labels: Option<String>,

    /// Working notes
    #[schemars(description = "Working notes (use for ongoing updates)")]
    #[serde(default)]
    pub notes: Option<String>,

    /// Blocked by task IDs
    #[schemars(description = "Comma-separated task IDs that block this task")]
    #[serde(default)]
    pub blocked_by: Option<String>,

    /// Design notes
    #[schemars(description = "Design notes or technical approach")]
    #[serde(default)]
    pub design: Option<String>,

    /// Acceptance criteria
    #[schemars(description = "Acceptance criteria for task completion")]
    #[serde(default)]
    pub acceptance_criteria: Option<String>,

    /// External reference
    #[schemars(description = "External reference (URL, ticket ID, etc.)")]
    #[serde(default)]
    pub external_ref: Option<String>,

    /// Assignee
    #[schemars(description = "Assignee identifier (agent ID or name)")]
    #[serde(default)]
    pub assignee: Option<String>,

    /// Demo statement
    #[schemars(
        description = "What can be demonstrated when this task is complete (e.g., 'Type a query, results filter live')"
    )]
    #[serde(default)]
    pub demo_statement: Option<String>,

    /// Execution methodology note
    #[schemars(
        description = "Execution methodology for this task. One of: test-first, characterization-first, additive-only. Omit or leave null if no methodology is declared."
    )]
    #[serde(default)]
    pub execution_note: Option<String>,

    /// Epic ID to associate with
    #[schemars(
        description = "Epic task ID to associate this task with (creates ParentChild dependency)"
    )]
    #[serde(default)]
    pub epic: Option<String>,

    /// Execution depth (EPIC cas-1255 speed mode)
    #[schemars(
        description = "Execution depth: 'deep' (default, full rigor) or 'light' (fast, feel-driven pass). Omit to default to deep."
    )]
    #[serde(default)]
    pub depth: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskCloseRequest {
    /// Task ID
    #[schemars(description = "Task ID to close")]
    pub id: String,

    /// Close reason
    #[schemars(description = "Reason for closing (resolution notes)")]
    #[serde(default)]
    pub reason: Option<String>,

    /// Supervisor override for the cas-code-review P0 close gate (cas-b39f).
    ///
    /// When `true`, the close path skips the multi-persona code-review
    /// gate that would otherwise hard-block on P0 findings. Only honored
    /// when the caller runs under a supervisor role
    /// (`CAS_AGENT_ROLE=supervisor`); non-supervisor callers that set
    /// this flag get an explicit rejection. The override is logged as a
    /// decision note on the task so the audit trail captures who
    /// downgraded a P0 block and why.
    #[schemars(
        description = "Supervisor override for the code-review P0 gate. \
                       Only honored when the caller is a supervisor; other \
                       roles are rejected. Logs a decision note on the task."
    )]
    #[serde(default)]
    pub bypass_code_review: Option<bool>,

    /// Structured cas-code-review results passed in by the worker
    /// before it retries close (cas-b39f, option (a)). The worker
    /// invokes the cas-code-review skill, collects the merged
    /// ReviewOutcome envelope, serializes it to JSON, and sends it
    /// here as a string. The close gate parses it via
    /// `serde_json::from_str::<ReviewOutcome>` and runs validation
    /// before deciding the P0 block verdict.
    ///
    /// A string (instead of a typed struct) is used at the MCP
    /// boundary because `ReviewOutcome` lives in `cas-types`, which
    /// intentionally has no `schemars` dependency — taking a JSON
    /// blob keeps the schema surface narrow. When this field is
    /// `None` and the task has reviewable code changes (and no
    /// supervisor override), the gate returns `CODE_REVIEW_REQUIRED`.
    #[schemars(description = "LEGACY `[code_review] owner = \"worker\"` MODE ONLY. \
                       Serialized ReviewOutcome JSON envelope from a \
                       cas-code-review run. Under the DEFAULT \
                       owner = \"supervisor\" configuration a factory worker \
                       must NOT run cas-code-review (skill, workflow, or \
                       hand-spawned personas) and must NOT pass this \
                       field — attempt the close without it; the close \
                       transitions the task to PendingSupervisorReview and \
                       the supervisor runs the review on their own schedule \
                       (cas-4fef). Check with \
                       `cas config get code_review.owner`. \
                       Under owner = \"worker\" it is \
                       required whenever the task has reviewable code \
                       changes unless bypass_code_review=true or the \
                       task is additive-only. Shape: \
                       {residual: Finding[], pre_existing: Finding[], mode: string, \
                       execution: {personas_run: int, personas_failed: string[], \
                       required_personas_missing: string[], skipped_reason: string|null}}. \
                       `execution` is required and must be copied from the review \
                       result, not hand-written: without it an empty residual[] is \
                       indistinguishable from a review that never ran (cas-acf83). \
                       Each Finding requires: title, severity, file, line, \
                       why_it_matters, autofix_class, owner, confidence, \
                       evidence, pre_existing (optional: suggested_fix, \
                       requires_verification)."
    )]
    #[serde(default)]
    pub code_review_findings: Option<String>,

    /// Search manifest for investigation-task (`Spike`) closes (cas-49f1).
    /// This is the per-action mirror of the unified `TaskRequest` field of
    /// the same name (`crates/cas-mcp/src/types.rs`).
    #[schemars(
        description = "Serialized JSON array of search steps run during an \
                       investigation (Spike) task, e.g. \
                       [{\"command\": \"grep -c foo file\", \"hits\": 3}]. \
                       Optional; when present on a Spike-type close, any \
                       entry with hits=0 is surfaced as a warning note \
                       instead of a silent pass."
    )]
    #[serde(default)]
    pub search_manifest: Option<String>,

    /// Task-attributed commit receipt for merged-before-close recovery.
    #[schemars(
        description = "Full commit SHA or unambiguous hexadecimal abbreviation produced by this \
                       task. CAS resolves it to a full immutable commit ID, validates that it \
                       carries a non-empty diff and is an ancestor of the task's parent branch, \
                       and persists only the full ID."
    )]
    #[serde(default)]
    pub commit_receipt: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskReopenRequest {
    /// Task ID
    #[schemars(description = "Task ID to reopen")]
    pub id: String,

    /// Reopen reason (cas-cd24)
    #[schemars(
        description = "Reason for reopening (e.g. why a blocker was withdrawn or a \
                       closed task needs rework). Recorded on the task's audit \
                       trail as a note."
    )]
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskRequestChangesRequest {
    /// Task ID whose unmerged delivery was declined
    #[schemars(description = "AwaitingMerge task ID whose delivery needs changes")]
    pub id: String,

    /// Required supervisor verdict and branch-handling instruction
    #[schemars(
        description = "Required supervisor review reason. State what prior work may remain and what must be corrected or reverted before re-delivery."
    )]
    pub reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskUpdateRequest {
    /// Task ID to update
    #[schemars(description = "ID of the task to update")]
    pub id: String,

    /// Update title
    #[schemars(description = "New title for the task")]
    #[serde(default)]
    pub title: Option<String>,

    /// Add to notes
    #[schemars(description = "Notes to append (working notes, progress updates)")]
    #[serde(default)]
    pub notes: Option<String>,

    /// Update priority
    #[schemars(description = "New priority (0-4)")]
    #[serde(default)]
    pub priority: Option<u8>,

    /// Add labels
    #[schemars(description = "Comma-separated labels to add")]
    #[serde(default)]
    pub labels: Option<String>,

    /// Add blocking dependencies (cas-d45f / GH #98).
    ///
    /// Additive, like `labels`: each listed task becomes a blocker of this
    /// task (`this blocked_by that`). Existing blockers are left in place;
    /// use `dep_remove` to drop one.
    #[schemars(
        description = "Comma-separated task IDs that block this task (adds Blocks dependencies; existing blockers are kept)"
    )]
    #[serde(default)]
    pub blocked_by: Option<String>,

    /// Update description
    #[schemars(description = "New description")]
    #[serde(default)]
    pub description: Option<String>,

    /// Update design notes
    #[schemars(description = "New design notes")]
    #[serde(default)]
    pub design: Option<String>,

    /// Update acceptance criteria
    #[schemars(description = "New acceptance criteria")]
    #[serde(default)]
    pub acceptance_criteria: Option<String>,

    /// Update demo statement
    #[schemars(
        description = "What can be demonstrated when this task is complete (e.g., 'Type a query, results filter live')"
    )]
    #[serde(default)]
    pub demo_statement: Option<String>,

    /// Update execution note
    #[schemars(
        description = "Execution methodology for this task. One of: test-first, characterization-first, additive-only. Pass an empty string to clear."
    )]
    #[serde(default)]
    pub execution_note: Option<String>,

    /// Update external reference
    #[schemars(description = "New external reference")]
    #[serde(default)]
    pub external_ref: Option<String>,

    /// Update assignee. Empty string clears (unassigns). Omit to leave unchanged.
    #[schemars(
        description = "New assignee (worker display name). Pass empty string to clear/unassign; omit to leave unchanged."
    )]
    #[serde(default)]
    pub assignee: Option<String>,

    /// Update status
    #[schemars(description = "New status: 'open', 'in_progress', 'closed', 'blocked'")]
    #[serde(default)]
    pub status: Option<String>,

    /// Set or change epic association
    #[schemars(
        description = "Epic task ID to associate this task with (updates ParentChild dependency)"
    )]
    #[serde(default)]
    pub epic: Option<String>,

    /// Set epic verification owner (for epics in factory mode).
    ///
    /// cas-cc74: this is an authorized transfer — only the current owner or a
    /// supervisor may change it, and the target must be a live
    /// supervisor/director identity. Unauthorized rewrites are rejected.
    #[schemars(
        description = "Agent ID responsible for epic verification (supervisor in factory mode)"
    )]
    #[serde(default)]
    pub epic_verification_owner: Option<String>,

    /// Update execution depth (EPIC cas-1255 speed mode)
    #[schemars(
        description = "Execution depth: 'deep' (full rigor) or 'light' (fast, feel-driven pass). Omit to leave unchanged."
    )]
    #[serde(default)]
    pub depth: Option<String>,
}

// ============================================================================
// Task Show Request
// ============================================================================

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskShowRequest {
    /// Task ID
    #[schemars(description = "ID of the task to show")]
    pub id: String,

    /// Include dependencies
    #[schemars(description = "Include dependency information (default: true)")]
    #[serde(default = "default_true")]
    pub with_deps: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DependencyRequest {
    /// From task ID
    #[schemars(description = "Task that has the dependency")]
    pub from_id: String,

    /// To task ID
    #[schemars(description = "Task that blocks/relates to the first")]
    pub to_id: String,

    /// Dependency type
    #[schemars(description = "Type: 'blocks' (default), 'related', 'parent', 'duplicate'")]
    #[serde(default = "default_dep_type")]
    pub dep_type: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskNotesRequest {
    /// Task ID
    #[schemars(description = "ID of the task to add notes to")]
    pub id: String,

    /// Note content
    #[schemars(description = "The note content to append")]
    pub note: String,

    /// Note type for structured categorization
    #[schemars(
        description = "Type: 'progress' (default), 'blocker', 'decision', 'discovery', 'question'"
    )]
    #[serde(default = "default_note_type")]
    pub note_type: String,
}

// ============================================================================
// Sub-Agent Context Request
// ============================================================================

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubAgentContextRequest {
    /// Task ID to get context for
    #[schemars(description = "Task ID to build focused context for")]
    pub task_id: String,

    /// Maximum tokens for the context
    #[schemars(description = "Maximum tokens for sub-agent context (default: 2000)")]
    #[serde(default = "default_subagent_tokens")]
    pub max_tokens: usize,

    /// Include related memories
    #[schemars(description = "Search and include related memories (default: true)")]
    #[serde(default = "default_true")]
    pub include_memories: bool,
}
