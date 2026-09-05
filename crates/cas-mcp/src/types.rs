//! MCP Request Types for CAS
//!
//! These types define the schema for the 7 consolidated MCP tools:
//! - cas_memory: Memory/entry operations
//! - cas_task: Task and dependency operations
//! - cas_rule: Rule operations
//! - cas_skill: Skill operations
//! - cas_agent: Agent and loop operations
//! - cas_search: Search, context, and entity operations
//! - cas_system: Diagnostics, stats, and maintenance

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Unified memory operations request
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemoryRequest {
    /// Action to perform
    #[schemars(
        description = "Action: 'remember', 'get', 'list', 'update', 'delete', 'archive', 'unarchive', 'helpful', 'harmful', 'recent', 'set_tier', 'opinion_reinforce', 'opinion_weaken', 'opinion_contradict'"
    )]
    pub action: String,

    /// Entry ID (for get, update, delete, archive, unarchive, helpful, harmful, set_tier, opinion_*)
    #[schemars(description = "Entry ID for operations that target a specific entry")]
    #[serde(default)]
    pub id: Option<String>,

    /// Content (for remember, update, opinion_*)
    #[schemars(description = "Content for remember or update, or evidence for opinion operations")]
    #[serde(default)]
    pub content: Option<String>,

    /// Entry type (for remember): learning, preference, context, observation
    #[schemars(
        description = "Entry type for remember: 'learning' (default), 'preference', 'context', 'observation'"
    )]
    #[serde(default)]
    pub entry_type: Option<String>,

    /// Tags (comma-separated). For `list`, entries must contain every
    /// requested tag (case-insensitive AND).
    #[schemars(
        description = "Comma-separated tags; for list, entries must contain every requested tag (case-insensitive AND)"
    )]
    #[serde(default)]
    pub tags: Option<String>,

    /// Title (for remember)
    #[schemars(description = "Optional title for the entry")]
    #[serde(default)]
    pub title: Option<String>,

    /// Importance score (0.0-1.0)
    #[schemars(description = "Importance score from 0.0 to 1.0")]
    #[serde(default)]
    pub importance: Option<f32>,

    /// Memory tier (for set_tier/list): working, cold, archive
    #[schemars(description = "Memory tier for set_tier or list: 'working', 'cold', 'archive'")]
    #[serde(default)]
    pub tier: Option<String>,

    /// Limit for list/recent
    #[schemars(description = "Maximum items to return")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub limit: Option<usize>,

    /// Scope filter
    #[schemars(description = "Scope: 'global', 'project', or 'all'")]
    #[serde(default)]
    pub scope: Option<String>,

    /// Team ID filter (for list/recent - filters to team-shared entries)
    #[schemars(description = "Team ID to filter entries shared with a specific team")]
    #[serde(default)]
    pub team_id: Option<String>,

    /// Skip pre-insert overlap detection (bulk imports / tests only).
    #[schemars(
        description = "Skip overlap detection on remember (bulk imports / tests only — defaults to false)"
    )]
    #[serde(default)]
    pub bypass_overlap: Option<bool>,

    /// Overlap handling mode for `remember`. `interactive` is the default;
    /// `autofix` explicitly opts in to atomically merging a high-overlap save.
    #[schemars(
        description = "Overlap handling mode: 'interactive' (default) | 'autofix' (atomic high-overlap merge)"
    )]
    #[serde(default)]
    pub mode: Option<String>,

    /// Optimistic-concurrency timestamp for an `autofix` merge. When set,
    /// the existing overlapping memory must still have this RFC3339 update
    /// timestamp or the merge returns a conflict without changing it.
    #[schemars(
        description = "For remember mode=autofix: RFC3339 updated timestamp expected on the existing memory; stale values return a conflict"
    )]
    #[serde(default)]
    pub expected_updated_at: Option<String>,

    /// Sort field (for list)
    #[schemars(description = "Sort by: 'created', 'updated', 'importance', 'title'")]
    #[serde(default)]
    pub sort: Option<String>,

    /// Sort order (for list)
    #[schemars(description = "Sort order: 'asc' or 'desc' (default: desc)")]
    #[serde(default)]
    pub sort_order: Option<String>,

    /// Valid from (RFC3339 timestamp for temporal validity)
    #[schemars(
        description = "When this fact becomes valid (RFC3339 format, e.g., 2025-01-01T00:00:00Z)"
    )]
    #[serde(default)]
    pub valid_from: Option<String>,

    /// Valid until (RFC3339 timestamp for temporal validity)
    #[schemars(
        description = "When this fact expires (RFC3339 format, e.g., 2025-12-31T23:59:59Z)"
    )]
    #[serde(default)]
    pub valid_until: Option<String>,

    /// Force a personal (non-team) note even in a team-linked project.
    ///
    /// By default, `remember` in a project with an active team auto-scopes
    /// the entry to that team (team_auto_promote). Set `personal=true` to
    /// opt out for a one-off private note.
    ///
    /// Ignored when `team_id` is set explicitly.
    #[schemars(
        description = "Set true to keep the note personal (skip team auto-promote) even in a team-linked project"
    )]
    #[serde(default)]
    pub personal: Option<bool>,
}

/// Unified task operations request
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskRequest {
    /// Action to perform
    #[schemars(
        description = "Action: 'create', 'proposal_inbox', 'proposal_accept', 'proposal_reject', 'proposal_reconcile', 'show', 'update', 'start', 'close', 'cancel', 'reopen', 'delete', 'list', 'ready', 'blocked', 'notes', 'dep_add', 'dep_remove', 'dep_list', 'claim', 'release', 'transfer', 'available', 'mine'"
    )]
    pub action: String,

    /// Task ID (for most operations except create, list, ready, blocked, available, mine)
    #[schemars(description = "Task ID for operations targeting a specific task")]
    #[serde(default)]
    pub id: Option<String>,

    /// Context-affordable task-start response. When true for `action=start`,
    /// return only bounded notes from the task itself and omit sibling/epic
    /// context.
    #[schemars(
        description = "For start: return a size-bounded response with own-task notes only, omitting sibling and epic context"
    )]
    #[serde(default, deserialize_with = "deser::option_bool")]
    pub brief: Option<bool>,

    /// Title (for create)
    #[schemars(description = "Task title for create")]
    #[serde(default)]
    pub title: Option<String>,

    /// Explicit target canonical project ID for cross-project proposal actions.
    #[schemars(
        description = "Explicit canonical project ID. On create, presence selects the cross-project proposal path; never inferred from cwd."
    )]
    #[serde(default)]
    pub project: Option<String>,

    /// Canonical owning project identity for a local task update. Unlike
    /// `project`, this does not select a cross-project proposal; it is an
    /// explicit supervisor-authorized origin reassignment.
    #[schemars(
        description = "For update: canonical origin project identity; only a live registered supervisor may reassign it"
    )]
    #[serde(default)]
    pub origin_project: Option<String>,

    /// Cloud proposal ID for accept/reject.
    #[schemars(description = "Cloud proposal ID for proposal_accept or proposal_reject")]
    #[serde(default)]
    pub proposal_id: Option<String>,

    /// Optional local origin task blocked by the proposed target task.
    #[schemars(
        description = "For cross-project create: local origin task ID that remains blocked until the accepted target task closes"
    )]
    #[serde(default)]
    pub blocks_origin_task_id: Option<String>,

    /// Explicit identity for one cross-project create attempt.
    #[schemars(
        description = "For cross-project create: explicit idempotency identity for one logical attempt. Reuse it only when retrying an ambiguous result; use a new value for a later identical create."
    )]
    #[serde(default)]
    pub proposal_attempt_id: Option<String>,

    /// Description (for create)
    #[schemars(description = "Task description")]
    #[serde(default)]
    pub description: Option<String>,

    /// Priority 0-4 (for create, update). Accepts numeric (0-4), numeric
    /// string ("0"-"4"), or named alias (critical, high, medium, low, backlog).
    #[schemars(
        description = "Priority: 0=Critical, 1=High, 2=Medium (default), 3=Low, 4=Backlog. \
                       Accepts numeric (0-4) or named alias (critical/high/medium/low/backlog)."
    )]
    #[serde(default, deserialize_with = "deser::option_priority")]
    pub priority: Option<u8>,

    /// Task type (for create): task, bug, feature, epic, chore, spike, gate
    #[schemars(
        description = "Task type: 'task', 'bug', 'feature', 'epic', 'chore', 'spike', 'gate'"
    )]
    #[serde(default)]
    pub task_type: Option<String>,

    /// Labels (comma-separated)
    #[schemars(description = "Comma-separated labels")]
    #[serde(default)]
    pub labels: Option<String>,

    /// Notes (for create, update, notes). For `action=notes`, omission reads
    /// only the task's notes; presence appends the supplied value.
    #[schemars(
        description = "Notes content. For action=notes: omit to read only the task's notes; supply to append"
    )]
    #[serde(default)]
    pub notes: Option<String>,

    /// Merge patch for the compact structured task resume state. The patch is
    /// validated and applied atomically by the task store; null removes a
    /// field. Prose notes remain the human/audit surface.
    #[schemars(
        description = "For update: JSON merge patch for structured execution state (phase, receipts, files_touched, decisions, next_step); null deletes a field"
    )]
    #[serde(default)]
    pub state_patch: Option<serde_json::Value>,

    /// Note type (for notes action): progress, blocker, decision, discovery, question
    #[schemars(
        description = "Note type: 'progress', 'blocker', 'decision', 'discovery', 'question'"
    )]
    #[serde(default)]
    pub note_type: Option<String>,

    /// Close reason (for close action). IMPORTANT: Verification must pass before close.
    #[schemars(
        description = "Reason for closing. IMPORTANT: Verification must pass BEFORE close. Workers should attempt close first; if close returns verification-required guidance, follow the indicated verifier ownership workflow."
    )]
    #[serde(default)]
    pub reason: Option<String>,

    /// Portable task/commit/PR pointer for `action=cancel` when another result
    /// superseded this task.
    #[schemars(
        description = "Optional portable task, commit, or PR pointer that superseded cancelled work"
    )]
    #[serde(default)]
    pub superseded_by: Option<String>,

    /// Supervisor override for close gates.
    ///
    /// When `true`, the close path honors only the explicitly documented
    /// supervisor exceptions. The caller must be a registered supervisor and
    /// provide a non-empty close reason; the accepted decision is logged on
    /// the task.
    #[schemars(
        description = "Supervisor override for close gates. Only honored when the caller is a registered supervisor and a non-empty reason is supplied; logs the accepted decision on the task."
    )]
    #[serde(default, deserialize_with = "deser::option_bool")]
    pub supervisor_override: Option<bool>,

    /// One-release compatibility alias for `supervisor_override`.
    #[schemars(skip)]
    #[serde(
        default,
        rename = "bypass_code_review",
        skip_serializing,
        deserialize_with = "deser::option_bool"
    )]
    pub legacy_bypass_code_review: Option<bool>,

    /// Supervisor override for the epic close stranded-branch gate (cas-b192).
    ///
    /// A narrative string, not a boolean: the value IS the audit record of what
    /// was inspected before waiving the gate, and it is stored next to the
    /// gate's own measurements. Only honored for a live registered supervisor.
    #[schemars(
        description = "Supervisor override for the epic close stranded-branch gate. Pass a narrative describing what you inspected (e.g. 'diffed all 11 delivered paths against main; content present and later refactored by PR #406'). Only honored for a live registered supervisor. The narrative and the gate's own per-branch measurements are logged as a decision note. Use this instead of improvising a workaround when every lane is measurably stale."
    )]
    #[serde(default)]
    pub stranded_branch_override: Option<String>,

    /// Supervisor-authorized close for a measured negative result whose
    /// experimental branch is deliberately not merged.
    ///
    /// This is distinct from `supervisor_override`: it is accepted only from
    /// a registered supervisor and requires both structured evidence fields
    /// below plus a non-empty close `reason`. Ordinary closes that omit this
    /// flag retain the merge-state gate unchanged.
    #[schemars(
        description = "Set true only when a registered supervisor is closing a measured negative result whose experimental delivery was deliberately left unmerged. Requires negative_result_artifact_path, negative_result_reference, and a non-empty reason; CAS logs the supervisor decision. Ordinary delivery closes must omit this flag and still require merge evidence."
    )]
    #[serde(default, deserialize_with = "deser::option_bool")]
    pub negative_result: Option<bool>,

    /// Existing durable evidence beneath the task's configured artifacts
    /// directory, required when `negative_result=true`.
    #[schemars(
        description = "Absolute path to existing durable measurement evidence under configured [factory] artifacts_root/<task-id>/. Required with negative_result=true."
    )]
    #[serde(default)]
    pub negative_result_artifact_path: Option<String>,

    /// Audit reference to the closed-unmerged PR or experimental branch,
    /// required when `negative_result=true`.
    #[schemars(
        description = "Portable audit reference to the deliberately closed-unmerged PR or experimental branch (for example a PR URL or branch:factory/name). Required with negative_result=true."
    )]
    #[serde(default)]
    pub negative_result_reference: Option<String>,

    /// Search manifest for investigation-task (`Spike`) closes (cas-49f1).
    ///
    /// Optional JSON array of `{command, hits}` entries: every search
    /// command the worker actually ran and the hit count each returned.
    /// The close gate only consults this for `Spike`-type tasks; if any
    /// entry reports `hits == 0`, a warning note is appended rather than
    /// letting the close proceed silently. Ordinary code tasks and tasks
    /// that omit this field are entirely unaffected.
    #[schemars(description = "Serialized JSON array of search steps run during an \
                       investigation (Spike) task, e.g. \
                       [{\"command\": \"grep -c foo file\", \"hits\": 3}]. \
                       Optional; when present on a Spike-type close, any \
                       entry with hits=0 is surfaced as a warning note \
                       instead of a silent pass.")]
    #[serde(default)]
    pub search_manifest: Option<String>,

    /// Task-attributed commit receipt for a merged-before-close recovery.
    ///
    /// The close gate accepts a full commit SHA or unambiguous hexadecimal
    /// abbreviation, persists its full immutable ID, and verifies a non-empty
    /// diff, reachability from the task's parent branch, and either continued
    /// presence or explicit post-integration evolution of that commit's tree
    /// effect on the current target. It is only needed
    /// when no commit-time factory anchor was captured.
    #[schemars(
        description = "Full commit SHA or unambiguous hexadecimal abbreviation produced by this \
                       task. On close, CAS resolves it to a full immutable commit ID, validates \
                       that it carries a non-empty diff, is an ancestor of the task's parent \
                       branch, and still has its tree effect present or explicitly evolved by \
                       post-integration work on the current target; CAS \
                       persists only the full ID. Use this for merged-before-close \
                       work when no automatic factory anchor was captured."
    )]
    #[serde(default)]
    pub commit_receipt: Option<String>,

    /// Serialized immutable worker-delivery receipt. This is an additive,
    /// opt-in transactional handoff; omitting it preserves legacy close.
    #[schemars(
        description = "Serialized WorkerCompletionReceiptInput JSON. CAS revalidates \
                       the registered worker, task, repository, branch, commit/base/target \
                       tips, proof reference, and optional artifact_path before persisting an \
                       immutable delivery transaction. artifact_path must be an existing file \
                       or directory under configured [factory] artifacts_root/<task-id>/. \
                       Omit to use the legacy close path unchanged."
    )]
    #[serde(default)]
    pub completion_receipt: Option<String>,

    /// Include dependencies (for show)
    #[schemars(description = "Include dependency information")]
    #[serde(default, deserialize_with = "deser::option_bool")]
    pub with_deps: Option<bool>,

    /// Blocked by task IDs (for create)
    #[schemars(description = "Comma-separated task IDs that block this task")]
    #[serde(default)]
    pub blocked_by: Option<String>,

    /// Target task ID (for dep_add, dep_remove)
    #[schemars(description = "Target task ID for dependency operations")]
    #[serde(default)]
    pub to_id: Option<String>,

    /// Dependency type: blocks, related, parent, duplicate
    #[schemars(description = "Dependency type: 'blocks', 'related', 'parent', 'duplicate'")]
    #[serde(default)]
    pub dep_type: Option<String>,

    /// Lease duration in seconds (for claim)
    #[schemars(description = "Lease duration in seconds (default: 600)")]
    #[serde(default, deserialize_with = "deser::option_i64")]
    pub duration_secs: Option<i64>,

    /// Target agent ID (for transfer)
    #[schemars(description = "Target agent ID for transfer")]
    #[serde(default)]
    pub to_agent: Option<String>,

    /// Limit for list operations
    #[schemars(description = "Maximum items to return")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub limit: Option<usize>,

    /// Include rows whose origin belongs to another project. Task list
    /// surfaces hide foreign replicas by default.
    #[schemars(
        description = "For list, ready, blocked, and available: include foreign-origin task rows (default: false)"
    )]
    #[serde(default, deserialize_with = "deser::option_bool")]
    pub include_foreign: Option<bool>,

    /// Scope filter
    #[schemars(description = "Scope: 'global', 'project', or 'all'")]
    #[serde(default)]
    pub scope: Option<String>,

    /// Design notes (for create, update)
    #[schemars(description = "Design notes or technical approach")]
    #[serde(default)]
    pub design: Option<String>,

    /// Acceptance criteria (for create, update)
    #[schemars(description = "Acceptance criteria for task completion")]
    #[serde(default)]
    pub acceptance_criteria: Option<String>,

    /// Demo statement (for create, update) - what can be demonstrated when task is complete
    #[schemars(
        description = "What can be demonstrated when this task is complete (e.g., 'Type a query, results filter live')"
    )]
    #[serde(default)]
    pub demo_statement: Option<String>,

    /// Execution note (for create, update, and no-code close) - methodology used to execute this task
    #[schemars(
        description = "Execution methodology for this task. One of: test-first, characterization-first, additive-only, value-only, no-code. value-only permits edits to existing values but rejects added, deleted, copied, or renamed files at close. no-code declares an operations/artifact task and requires external_ref proof at close. For action=close, only no-code is accepted and is persisted before verification dispatch or atomically with an already-approved close. Pass empty string to clear on update."
    )]
    #[serde(default)]
    pub execution_note: Option<String>,

    /// Supervisor-only proof-scope correction for `action=update`.
    #[schemars(
        description = "For update only: supervisor-authorized correction of target_repo/target_branch after MERGE REQUIRED. For a task already marked execution_note=no-code, target_repo=\"\" clears a stale code anchor. Requires a non-empty reason, invalidates the stale proof cycle, records a decision note, and reopens the task without review-failed semantics."
    )]
    #[serde(default, deserialize_with = "deser::option_bool")]
    pub proof_scope_fix: Option<bool>,

    /// External reference (for create, update)
    #[schemars(description = "External reference (URL, ticket ID, etc.)")]
    #[serde(default)]
    pub external_ref: Option<String>,

    /// Assignee (for create, update)
    #[schemars(description = "Assignee identifier (agent ID or name)")]
    #[serde(default)]
    pub assignee: Option<String>,

    /// Status (for list: filter; for update: set new status)
    #[schemars(
        description = "Filter by status (for list) or set new status (for update): 'open', 'in_progress', 'closed', 'blocked'"
    )]
    #[serde(default)]
    pub status: Option<String>,

    /// Epic ID (for create, update) - adds ParentChild dependency to link task to an epic
    #[schemars(
        description = "Epic task ID to associate this task with (creates ParentChild dependency)"
    )]
    #[serde(default)]
    pub epic: Option<String>,

    /// Sort field (for list, ready, blocked, available)
    #[schemars(
        description = "Sort by: 'created', 'updated', 'priority', 'title'. Honoured by list, ready, blocked and available; ready/blocked/available default to priority when unset."
    )]
    #[serde(default)]
    pub sort: Option<String>,

    /// Sort order (for list, ready, blocked, available)
    #[schemars(
        description = "Sort order: 'asc' or 'desc' (default: desc for dates, asc for priority)"
    )]
    #[serde(default)]
    pub sort_order: Option<String>,

    /// Epic verification owner (for update on epics)
    #[schemars(
        description = "Agent ID responsible for epic verification (supervisor in factory mode)"
    )]
    #[serde(default)]
    pub epic_verification_owner: Option<String>,

    /// Execution depth (for create, update). EPIC cas-1255 speed mode.
    #[schemars(
        description = "Execution depth: 'deep' (default, full rigor) or 'light' (fast, feel-driven pass). Omit to leave unchanged (update) or default to deep (create)."
    )]
    #[serde(default)]
    pub depth: Option<String>,

    /// Factory branch delivery route (for create/update). `push_branch` is
    /// the normal remote-branch flow; `local_merge` keeps worker branches
    /// local for supervisor integration.
    #[schemars(
        description = "Factory delivery mode: 'push_branch' (default) or 'local_merge' (supervisor merges the local worker branch)"
    )]
    #[serde(default)]
    pub delivery_mode: Option<String>,

    /// Explicit acknowledgement of a planning-race or duplicate-task warning on create.
    #[schemars(
        description = "For action=create, confirm creation after CAS reports a recent competing epic plan or a high-similarity open task."
    )]
    #[serde(default, deserialize_with = "deser::option_bool")]
    pub confirm_warning: Option<bool>,

    /// Explicit repository containing this task's code changes.
    #[schemars(
        description = "Repository path containing this task's code changes (create/update). With update proof_scope_fix=true on an already-no-code parked task, pass an empty string to clear a stale code anchor."
    )]
    #[serde(default)]
    pub target_repo: Option<String>,

    /// Expected integration branch in the target repository.
    #[schemars(description = "Expected integration branch in target_repo (create/update).")]
    #[serde(default)]
    pub target_branch: Option<String>,

    /// Override the alive-worker safety guard on `action=reset` (cas-86c5).
    ///
    /// By default, `reset` refuses to drop the lease of a task whose
    /// assignee has a fresh heartbeat — the worker is alive and actively
    /// working the task. Pass `force=true` to override this guard and
    /// reset anyway. The audit note will record that the reset was forced.
    #[schemars(
        description = "Force reset even when the task's assignee has a fresh heartbeat (alive worker). \
                       Omit or false → warn and abort; true → reset immediately and log a forced-reset audit note."
    )]
    #[serde(default, deserialize_with = "deser::option_bool")]
    pub force: Option<bool>,
}

/// Unified rule operations request
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct RuleRequest {
    /// Action to perform
    #[schemars(
        description = "Action: 'create', 'show', 'update', 'delete', 'list', 'list_all', 'history', 'restore', 'helpful', 'harmful', 'sync', 'check_similar'"
    )]
    pub action: String,

    /// Rule ID (for get, update, delete, helpful, harmful)
    #[schemars(description = "Rule ID for operations targeting a specific rule")]
    #[serde(default)]
    pub id: Option<String>,

    /// Content (for create, update)
    #[schemars(description = "Rule content/instruction")]
    #[serde(default)]
    pub content: Option<String>,

    /// Paths pattern (for create, update)
    #[schemars(description = "Glob patterns for files this rule applies to")]
    #[serde(default)]
    pub paths: Option<String>,

    /// Tags (comma-separated)
    #[schemars(description = "Comma-separated tags")]
    #[serde(default)]
    pub tags: Option<String>,

    /// Source entry IDs (comma-separated, for create)
    #[schemars(description = "Source entry IDs this rule was derived from (comma-separated)")]
    #[serde(default)]
    pub source_ids: Option<String>,

    /// Limit for list operations
    #[schemars(description = "Maximum items to return")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub limit: Option<usize>,

    /// Scope filter
    #[schemars(description = "Scope: 'global' or 'project'")]
    #[serde(default)]
    pub scope: Option<String>,

    /// Auto-approve tools (for create, update)
    #[schemars(description = "Tools to auto-approve (comma-separated, e.g., 'Read,Glob,Grep')")]
    #[serde(default)]
    pub auto_approve_tools: Option<String>,

    /// Auto-approve paths (for create, update)
    #[schemars(description = "Path patterns for auto-approval (comma-separated globs)")]
    #[serde(default)]
    pub auto_approve_paths: Option<String>,

    /// Similarity threshold (for check_similar)
    #[schemars(description = "Similarity threshold for check_similar (0.0-1.0, default: 0.75)")]
    #[serde(default)]
    pub threshold: Option<f32>,

    /// Version number to restore; omitted restores the newest prior state.
    #[serde(default)]
    pub version: Option<i64>,

    /// Compatibility field for clients that call this version_id.
    #[serde(default)]
    pub version_id: Option<i64>,

    /// Actor recorded in rule history.
    #[serde(default)]
    pub changed_by: Option<String>,

    /// Reason recorded in rule history.
    #[serde(default)]
    pub change_note: Option<String>,
}

/// Unified skill operations request
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SkillRequest {
    /// Action to perform
    #[schemars(
        description = "Action: 'create', 'show', 'update', 'delete', 'list', 'list_all', 'history', 'restore', 'enable', 'disable', 'sync', 'use'"
    )]
    pub action: String,

    /// Skill ID (for get, update, delete, enable, disable, use)
    #[schemars(description = "Skill ID for operations targeting a specific skill")]
    #[serde(default)]
    pub id: Option<String>,

    /// Name (required for create)
    #[schemars(description = "Human-readable name for the skill (required for create)")]
    #[serde(default)]
    pub name: Option<String>,

    /// Description (required for create, optional for update) - full body content
    #[schemars(
        description = "Full skill instructions/content (goes in SKILL.md body). Include commands, code examples, guidelines. Required for create."
    )]
    #[serde(default)]
    pub description: Option<String>,

    /// Invocation (required for create, optional for update)
    #[schemars(
        description = "How to invoke the skill (required for create). Examples: '/my-skill', 'cargo fmt', 'npm test'"
    )]
    #[serde(default)]
    pub invocation: Option<String>,

    /// Skill type: command, mcp, plugin, internal
    #[schemars(description = "Skill type: 'command', 'mcp', 'plugin', 'internal'")]
    #[serde(default)]
    pub skill_type: Option<String>,

    /// Tags (comma-separated)
    #[schemars(description = "Comma-separated tags")]
    #[serde(default)]
    pub tags: Option<String>,

    /// Source entry IDs (comma-separated, for create)
    #[schemars(description = "Source entry IDs this skill was derived from (comma-separated)")]
    #[serde(default)]
    pub source_ids: Option<String>,

    /// Limit for list operations
    #[schemars(description = "Maximum items to return")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub limit: Option<usize>,

    /// Scope filter
    #[schemars(description = "Scope: 'global' or 'project'")]
    #[serde(default)]
    pub scope: Option<String>,

    /// Short summary - trigger description for frontmatter
    #[schemars(
        description = "Short trigger description (1-2 lines) for SKILL.md frontmatter. Describes WHEN to use the skill. Example: 'Run E2E tests. Use when running tests or debugging failures.'"
    )]
    #[serde(default)]
    pub summary: Option<String>,

    /// Example usage
    #[schemars(description = "Example usage of the skill")]
    #[serde(default)]
    pub example: Option<String>,

    /// Pre-conditions (comma-separated)
    #[schemars(description = "Pre-conditions required (comma-separated)")]
    #[serde(default)]
    pub preconditions: Option<String>,

    /// Post-conditions (comma-separated)
    #[schemars(description = "Expected post-conditions (comma-separated)")]
    #[serde(default)]
    pub postconditions: Option<String>,

    /// Validation script
    #[schemars(description = "Script to check if skill is available")]
    #[serde(default)]
    pub validation_script: Option<String>,

    /// Make skill invokable via slash command
    #[schemars(description = "Enable /skill-name invocation")]
    #[serde(default, deserialize_with = "deser::option_bool")]
    pub invokable: Option<bool>,

    /// Argument hint for invokable skills
    #[schemars(description = "Argument hint (e.g., '[query]', '[file] [line]')")]
    #[serde(default)]
    pub argument_hint: Option<String>,

    /// Context mode for execution
    #[schemars(description = "Context mode: 'fork' for forked sub-agent")]
    #[serde(default)]
    pub context_mode: Option<String>,

    /// Agent type for execution
    #[schemars(description = "Agent type: 'Explore', 'code-reviewer', etc.")]
    #[serde(default)]
    pub agent_type: Option<String>,

    /// Allowed tools (comma-separated)
    #[schemars(description = "Allowed tools (comma-separated, e.g., 'Read,Grep,Glob')")]
    #[serde(default)]
    pub allowed_tools: Option<String>,

    /// Disallowed tools (comma-separated) — harness-enforced bans (Claude Code 2.1.152+)
    #[schemars(
        description = "Disallowed tools (comma-separated). Harness-enforced at runtime (Claude Code 2.1.152+)."
    )]
    #[serde(default)]
    pub disallowed_tools: Option<String>,

    /// Start as draft
    #[schemars(description = "Create skill as draft (not enabled)")]
    #[serde(default, deserialize_with = "deser::option_bool")]
    pub draft: Option<bool>,

    /// Disable model invocation (Claude Code 2.1.3+)
    #[schemars(description = "Prevent skill from invoking the model (for command-only skills)")]
    #[serde(default, deserialize_with = "deser::option_bool")]
    pub disable_model_invocation: Option<bool>,

    /// Version number to restore; omitted restores the newest prior state.
    #[serde(default)]
    pub version: Option<i64>,

    /// Compatibility field for clients that call this version_id.
    #[serde(default)]
    pub version_id: Option<i64>,

    /// Actor recorded in skill history.
    #[serde(default)]
    pub changed_by: Option<String>,

    /// Reason recorded in skill history.
    #[serde(default)]
    pub change_note: Option<String>,
}

/// Unified spec operations request
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SpecRequest {
    /// Action to perform
    #[schemars(
        description = "Action: 'create', 'show', 'update', 'delete', 'list', 'approve', 'reject', 'supersede', 'link', 'unlink', 'sync', 'get_for_task'"
    )]
    pub action: String,

    /// Spec ID (for show, update, delete, approve, reject, supersede, link, unlink)
    #[schemars(description = "Spec ID for operations targeting a specific spec")]
    #[serde(default)]
    pub id: Option<String>,

    /// Title (for create, update)
    #[schemars(description = "Spec title")]
    #[serde(default)]
    pub title: Option<String>,

    /// Summary (for create, update)
    #[schemars(description = "Brief summary of what this spec covers")]
    #[serde(default)]
    pub summary: Option<String>,

    /// Goals (comma-separated, for create, update)
    #[schemars(description = "Goals and objectives (comma-separated)")]
    #[serde(default)]
    pub goals: Option<String>,

    /// In scope items (comma-separated, for create, update)
    #[schemars(description = "What is in scope for this spec (comma-separated)")]
    #[serde(default)]
    pub in_scope: Option<String>,

    /// Out of scope items (comma-separated, for create, update)
    #[schemars(description = "What is explicitly out of scope (comma-separated)")]
    #[serde(default)]
    pub out_of_scope: Option<String>,

    /// Target users (comma-separated, for create, update)
    #[schemars(description = "Target users or personas (comma-separated)")]
    #[serde(default)]
    pub users: Option<String>,

    /// Technical requirements (comma-separated, for create, update)
    #[schemars(description = "Technical requirements and constraints (comma-separated)")]
    #[serde(default)]
    pub technical_requirements: Option<String>,

    /// Acceptance criteria (comma-separated, for create, update)
    #[schemars(description = "Acceptance criteria for completion (comma-separated)")]
    #[serde(default)]
    pub acceptance_criteria: Option<String>,

    /// Design notes (for create, update)
    #[schemars(description = "Design notes and decisions")]
    #[serde(default)]
    pub design_notes: Option<String>,

    /// Additional notes (for create, update)
    #[schemars(description = "Additional notes or context")]
    #[serde(default)]
    pub additional_notes: Option<String>,

    /// Spec type (for create, update, list)
    #[schemars(description = "Spec type: 'epic', 'feature', 'api', 'component', 'migration'")]
    #[serde(default)]
    pub spec_type: Option<String>,

    /// Status (for list filter)
    #[schemars(
        description = "Filter by status: 'draft', 'under_review', 'approved', 'superseded', 'rejected'"
    )]
    #[serde(default)]
    pub status: Option<String>,

    /// Associated task ID (for create, update, link, unlink)
    #[schemars(description = "Associated task ID (e.g., epic task)")]
    #[serde(default)]
    pub task_id: Option<String>,

    /// Source entry IDs (comma-separated, for create, update)
    #[schemars(description = "Source entry IDs this spec was derived from (comma-separated)")]
    #[serde(default)]
    pub source_ids: Option<String>,

    /// ID of spec being superseded (for supersede action)
    #[schemars(description = "ID of the spec being superseded")]
    #[serde(default)]
    pub supersedes_id: Option<String>,

    /// Create new version instead of modifying (for update)
    #[schemars(description = "Create a new version instead of modifying in place")]
    #[serde(default, deserialize_with = "deser::option_bool")]
    pub new_version: Option<bool>,

    /// Tags (comma-separated)
    #[schemars(description = "Comma-separated tags")]
    #[serde(default)]
    pub tags: Option<String>,

    /// Scope filter
    #[schemars(description = "Scope: 'global' or 'project'")]
    #[serde(default)]
    pub scope: Option<String>,

    /// Limit for list operations
    #[schemars(description = "Maximum items to return")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub limit: Option<usize>,
}

/// Unified agent and loop operations request
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentRequest {
    /// Action to perform
    #[schemars(
        description = "Action: 'register', 'unregister', 'whoami', 'heartbeat', 'list', 'cleanup', 'session_start', 'session_end', 'loop_start', 'loop_cancel', 'loop_status', 'lease_history', 'queue_notify', 'queue_poll', 'queue_peek', 'queue_ack', 'inbox_poll', 'message'"
    )]
    pub action: String,

    /// Agent ID (for unregister, whoami)
    #[schemars(description = "Agent ID")]
    #[serde(default)]
    pub id: Option<String>,

    /// Name (for register)
    #[schemars(description = "Human-readable name for the agent")]
    #[serde(default)]
    pub name: Option<String>,

    /// Agent type: primary, sub_agent, worker, ci
    #[schemars(description = "Agent type: 'primary', 'sub_agent', 'worker', 'ci'")]
    #[serde(default)]
    pub agent_type: Option<String>,

    /// Parent agent ID (for sub-agents spawned by Task tool)
    #[schemars(description = "Parent agent ID if this is a sub-agent")]
    #[serde(default)]
    pub parent_id: Option<String>,

    /// Session ID (from Claude Code) - this becomes the agent ID
    #[schemars(description = "Session ID from Claude Code (used as agent ID)")]
    #[serde(default)]
    pub session_id: Option<String>,

    /// Task ID (for loop_start)
    #[schemars(description = "Task ID")]
    #[serde(default)]
    pub task_id: Option<String>,

    /// Marks a worker-to-supervisor message as a merge request.
    ///
    /// CAS derives and attaches the immutable merge-request envelope only for
    /// this explicit message type. Ordinary worker escalations that happen to
    /// mention a task (including after that task's branch merged) must remain
    /// free-form messages and reach the supervisor unchanged.
    #[schemars(
        description = "message action only: mark this worker-to-supervisor message as a merge request. CAS then attaches its immutable cas-merge-request envelope and may suppress it only when the requested branch tip has already landed. Omit or false for blockers, questions, close failures, and every other ordinary message."
    )]
    #[serde(default)]
    pub merge_request: Option<bool>,

    /// Marks a message as a blocker escalation (cas-8725).
    ///
    /// CAS attaches the `<cas-blocker …>` envelope, which is what lets the row
    /// wake an idle supervisor instead of waiting for the recipient's next
    /// turn. The sender's own words are never inspected: a message that merely
    /// says "BLOCKER" stays inbox-only.
    #[schemars(
        description = "message action only: mark this message as a blocker escalation. CAS attaches its cas-blocker envelope so an idle supervisor is woken instead of finding the row at its next turn. Omit or false for status updates, questions and ordinary traffic."
    )]
    #[serde(default)]
    pub blocker: Option<bool>,

    /// Explicit notification this message acknowledges or answers.
    ///
    /// The recipient can use this durable queue ID to distinguish a real reply
    /// from delayed spawn boilerplate; CAS confirms the referenced message
    /// only after validating that this sender is its intended recipient.
    #[schemars(
        description = "message action only: notification_id of the prior direct message this response explicitly acknowledges. CAS validates the two endpoints, marks that exact prior message confirmed, and renders the reply with the durable reference."
    )]
    #[serde(default, deserialize_with = "deser::option_i64")]
    pub in_reply_to: Option<i64>,

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

    /// Cancel reason (for loop_cancel)
    #[schemars(description = "Reason for cancelling")]
    #[serde(default)]
    pub reason: Option<String>,

    /// Stale threshold seconds (for cleanup)
    #[schemars(description = "Seconds since last heartbeat to consider stale")]
    #[serde(default, deserialize_with = "deser::option_i64")]
    pub stale_threshold_secs: Option<i64>,

    /// Limit for list operations
    #[schemars(description = "Maximum items to return")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub limit: Option<usize>,

    // ========== Queue Operations Fields (Factory Mode) ==========
    /// Supervisor ID (for queue_notify, queue_poll, queue_peek)
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

    /// Priority (for queue_notify)
    #[schemars(
        description = "Notification priority: 'critical' (0), 'high' (1), 'normal' (2, default)"
    )]
    #[serde(default)]
    pub priority: Option<String>,

    /// Notification ID (for queue_ack)
    #[schemars(description = "Notification ID to acknowledge")]
    #[serde(default, deserialize_with = "deser::option_i64")]
    pub notification_id: Option<i64>,

    // ========== Message Queue Fields (Agent → Agent) ==========
    /// Target agent for message action (agent name, "supervisor", or "all_workers")
    #[schemars(
        description = "Target agent name, 'supervisor', or 'all_workers' for message action"
    )]
    #[serde(default)]
    pub target: Option<String>,

    /// Message content for message action (preferred over prompt for this action)
    #[schemars(
        description = "Message content to send (preferred over 'prompt' for message action)"
    )]
    #[serde(default)]
    pub message: Option<String>,

    /// Short summary of the message (shown in UI notifications)
    #[schemars(
        description = "A short one-line summary of the message, shown as a preview in the UI"
    )]
    #[serde(default)]
    pub summary: Option<String>,

    /// Urgent (interrupt-and-redirect) delivery for the message action.
    #[schemars(
        description = "When true, deliver the message URGENTLY: interrupt the target worker's in-flight turn (Esc) and inject the correction as its next prompt, bypassing the Claude Code inbox. Discards the worker's in-flight reasoning/partial work — use only when the worker is demonstrably off the rails. Default false = normal inbox/queue delivery."
    )]
    #[serde(default)]
    pub urgent: Option<bool>,
}

/// Unified personal pattern operations request
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PatternRequest {
    /// Action to perform
    #[schemars(
        description = "Action: 'create', 'list', 'show', 'update', 'archive', 'adopt', 'helpful', 'harmful', 'team_suggestions', 'team_new_suggestions', 'team_create_suggestion', 'team_share', 'team_adopt', 'team_dismiss', 'team_recommend', 'team_archive_suggestion', 'team_suggestion_analytics'"
    )]
    pub action: String,

    /// Pattern ID (for show, update, archive, helpful, harmful, team_share)
    #[schemars(description = "Pattern ID for operations targeting a specific pattern")]
    #[serde(default)]
    pub id: Option<String>,

    /// Content (for create, team_create_suggestion)
    #[schemars(description = "Pattern content/instruction text")]
    #[serde(default)]
    pub content: Option<String>,

    /// Category (for create, update, list filter, team_create_suggestion)
    #[schemars(
        description = "Category: 'convention', 'security', 'performance', 'architecture', 'error_handling', 'general'"
    )]
    #[serde(default)]
    pub category: Option<String>,

    /// Priority 0-3 (for create, update, team_create_suggestion). Accepts
    /// numeric, numeric string, or named alias.
    #[schemars(
        description = "Priority: 0=Critical, 1=High, 2=Medium (default), 3=Low. \
                       Accepts numeric (0-3) or named alias (critical/high/medium/low)."
    )]
    #[serde(default, deserialize_with = "deser::option_priority")]
    pub priority: Option<u8>,

    /// Propagation mode (for create, update)
    #[schemars(description = "Propagation: 'all_projects' (default), 'tagged_projects'")]
    #[serde(default)]
    pub propagation: Option<String>,

    /// Propagation tags (comma-separated, for create, update when propagation=tagged_projects)
    #[schemars(
        description = "Comma-separated tags for tagged_projects propagation (e.g., 'elixir,phoenix')"
    )]
    #[serde(default)]
    pub tags: Option<String>,

    /// Status filter (for list): active, archived, all
    #[schemars(description = "Filter by status: 'active' (default), 'archived', 'all'")]
    #[serde(default)]
    pub status: Option<String>,

    /// Rule ID (for adopt action)
    #[schemars(description = "CAS rule ID to adopt as a personal pattern (for adopt action)")]
    #[serde(default)]
    pub rule_id: Option<String>,

    /// Limit for list operations
    #[schemars(description = "Maximum items to return")]
    #[serde(default, deserialize_with = "deser::option_usize")]
    pub limit: Option<usize>,

    /// Team ID (for team_* actions)
    #[schemars(description = "Team ID for team suggestion operations")]
    #[serde(default)]
    pub team_id: Option<String>,

    /// Suggestion ID (for team_adopt, team_dismiss, team_recommend, team_archive_suggestion, team_suggestion_analytics)
    #[schemars(description = "Suggestion ID for team suggestion operations")]
    #[serde(default)]
    pub suggestion_id: Option<String>,

    /// Pattern ID to share (for team_share)
    #[schemars(description = "Personal pattern ID to share as team suggestion")]
    #[serde(default)]
    pub pattern_id: Option<String>,

    /// Recommended flag (for team_recommend)
    #[schemars(description = "Whether to mark suggestion as recommended (default: true)")]
    #[serde(default, deserialize_with = "deser::option_bool")]
    pub recommended: Option<bool>,

    /// Include dismissed suggestions (for team_suggestions)
    #[schemars(description = "Include dismissed suggestions in listing (default: false)")]
    #[serde(default, deserialize_with = "deser::option_bool")]
    pub include_dismissed: Option<bool>,
}

impl TaskRequest {
    pub fn effective_supervisor_override(&self) -> Option<bool> {
        self.supervisor_override.or(self.legacy_bypass_code_review)
    }

    pub fn used_deprecated_supervisor_override_alias(&self) -> bool {
        self.legacy_bypass_code_review.is_some()
    }
}

pub(crate) mod deser;
mod ops_secondary;

pub use crate::types::ops_secondary::{
    CoordinationRequest, ExecuteRequest, FactoryRequest, KnowledgeRequest, SearchContextRequest,
    SystemRequest, TeamRequest, VerificationRequest,
};

#[cfg(test)]
#[path = "types_tests/tests.rs"]
mod tests;

#[cfg(test)]
mod worker_delivery_request_tests {
    use super::TaskRequest;

    #[test]
    fn legacy_close_request_omitting_completion_receipt_deserializes_unchanged() {
        let request: TaskRequest =
            serde_json::from_str(r#"{"action":"close","id":"cas-legacy","reason":"done"}"#)
                .unwrap();
        assert_eq!(request.action, "close");
        assert_eq!(request.id.as_deref(), Some("cas-legacy"));
        assert_eq!(request.reason.as_deref(), Some("done"));
        assert!(request.completion_receipt.is_none());
        assert_eq!(request.negative_result, None);
        assert!(request.negative_result_artifact_path.is_none());
        assert!(request.negative_result_reference.is_none());
    }

    #[test]
    fn completion_receipt_is_additive_opaque_json_at_the_mcp_boundary() {
        let request: TaskRequest = serde_json::from_str(
            r#"{"action":"close","id":"cas-new","completion_receipt":"{\"task_id\":\"cas-new\"}"}"#,
        )
        .unwrap();
        assert_eq!(
            request.completion_receipt.as_deref(),
            Some(r#"{"task_id":"cas-new"}"#)
        );
    }

    #[test]
    fn negative_result_receipts_are_additive_at_the_mcp_boundary() {
        let request: TaskRequest = serde_json::from_str(
            r#"{"action":"close","id":"cas-negative","reason":"experiment regressed","negative_result":true,"negative_result_artifact_path":"/durable/cas-negative/proof.json","negative_result_reference":"https://github.com/pippenz/cas/pull/242"}"#,
        )
        .unwrap();
        assert_eq!(request.negative_result, Some(true));
        assert_eq!(
            request.negative_result_artifact_path.as_deref(),
            Some("/durable/cas-negative/proof.json")
        );
        assert_eq!(
            request.negative_result_reference.as_deref(),
            Some("https://github.com/pippenz/cas/pull/242")
        );
    }
}
