use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::mcp::tools::types::defaults::{
    default_entry_type, default_importance, default_recent, default_scope_project,
};

/// Filters and presentation options for `memory action=list`.
///
/// Tags are compared as case-insensitive exact values. When callers supply
/// multiple comma-separated tags, an entry must carry every requested tag.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemoryListRequest {
    /// Maximum number of items
    #[schemars(description = "Maximum items to return")]
    #[serde(default)]
    pub limit: Option<usize>,

    /// Scope filter
    #[schemars(description = "Filter by scope: 'global', 'project', or 'all' (default)")]
    #[serde(default = "crate::mcp::tools::types::defaults::default_scope_all")]
    pub scope: String,

    /// Tags filter
    #[schemars(
        description = "Comma-separated tags; entries must contain every requested tag (case-insensitive AND)"
    )]
    #[serde(default)]
    pub tags: Option<String>,

    /// Memory tier filter
    #[schemars(description = "Filter by memory tier: 'working', 'cold', or 'archive'")]
    #[serde(default)]
    pub tier: Option<String>,

    /// Sort field
    #[schemars(description = "Sort by: 'created', 'updated', 'importance', 'title'")]
    #[serde(default)]
    pub sort: Option<String>,

    /// Sort order
    #[schemars(description = "Sort order: 'asc' or 'desc' (default: desc)")]
    #[serde(default)]
    pub sort_order: Option<String>,

    /// Team ID filter
    #[schemars(description = "Filter to entries shared with a specific team")]
    #[serde(default)]
    pub team_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RememberRequest {
    /// The content to remember
    #[schemars(
        description = "The content to remember. Can be a fact, preference, context, or observation."
    )]
    pub content: String,

    /// Entry type
    #[schemars(
        description = "Type of memory: 'learning' (default), 'preference', 'context', or 'observation'"
    )]
    #[serde(default = "default_entry_type")]
    pub entry_type: String,

    /// Optional tags for categorization
    #[schemars(
        description = "Comma-separated tags for categorization (e.g., 'rust,cli,important')"
    )]
    #[serde(default)]
    pub tags: Option<String>,

    /// Optional title
    #[schemars(description = "Optional short title for the entry")]
    #[serde(default)]
    pub title: Option<String>,

    /// Importance score
    #[schemars(description = "Importance score from 0.0 to 1.0 (default: 0.5)")]
    #[serde(default = "default_importance")]
    pub importance: f32,

    /// Storage scope
    #[schemars(
        description = "Scope: 'global' (user prefs, general learnings) or 'project' (default, project-specific context)"
    )]
    #[serde(default = "default_scope_project")]
    pub scope: String,

    /// Valid from timestamp (RFC3339)
    #[schemars(description = "When this fact becomes valid (RFC3339 format)")]
    #[serde(default)]
    pub valid_from: Option<String>,

    /// Valid until timestamp (RFC3339)
    #[schemars(description = "When this fact expires (RFC3339 format)")]
    #[serde(default)]
    pub valid_until: Option<String>,

    /// Team ID for team-scoped entries
    #[schemars(description = "Team ID to share this entry with a team")]
    #[serde(default)]
    pub team_id: Option<String>,

    /// Skip pre-insert overlap detection. Reserved for bulk imports and
    /// tests that intentionally create overlapping memories. Normal callers
    /// should leave this unset so duplicates are caught at creation time.
    #[schemars(
        description = "Skip overlap detection (bulk imports / tests only — defaults to false)"
    )]
    #[serde(default)]
    pub bypass_overlap: Option<bool>,

    /// Overlap-handling mode. `"interactive"` (default) returns a structured
    /// `Blocked` response on high overlap. `"autofix"` explicitly opts in to
    /// an atomic update of the overlapping memory.
    #[schemars(
        description = "Overlap handling mode: 'interactive' (default) | 'autofix' (atomic high-overlap merge)"
    )]
    #[serde(default)]
    pub mode: Option<String>,

    /// Optimistic-concurrency timestamp for an `autofix` merge. If supplied,
    /// it must match the overlapping entry's current RFC3339 update timestamp
    /// or no data is changed and the response reports a conflict.
    #[schemars(
        description = "For mode=autofix: expected existing-entry update timestamp (RFC3339); stale values return a conflict"
    )]
    #[serde(default)]
    pub expected_updated_at: Option<String>,

    /// Force a personal (non-team) note even in a team-linked project.
    ///
    /// By default, `cas remember` in a project that has an active team will
    /// automatically scope the entry to that team so other members receive it
    /// on their next pull (`team_auto_promote`). Set `personal=true` to opt
    /// out for a one-off private note that stays in your personal sync queue.
    ///
    /// Ignored when `team_id` is set explicitly — an explicit `team_id` always
    /// wins regardless of this flag.
    #[schemars(
        description = "Set true to keep the note personal (skip team auto-promote) even in a team-linked project"
    )]
    #[serde(default)]
    pub personal: Option<bool>,
}

// ============================================================================
// MemoryRememberResponse (cas-e382)
//
// Structured response returned by `mcp__cas__memory action=remember`. The
// JSON payload is carried on `CallToolResult::structured_content` so that
// agents can pattern-match on the tagged `status` field without parsing the
// free-text message. A human-readable text block is also included for
// legacy text-only clients.
// ============================================================================

/// Per-dimension overlap score breakdown returned inside `Blocked`.
/// Mirrors [`cas_core::memory::DimensionScores`] but lives in the MCP
/// response layer so the public wire format is decoupled from the internal
/// scoring type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DimensionBreakdown {
    pub problem_statement: u8,
    pub root_cause: u8,
    pub solution_approach: u8,
    pub referenced_files: u8,
    pub tags: u8,
    /// Combined module + track mismatch penalty. Always ≤ 0.
    pub penalty: i8,
    /// Net score after penalty, floored at 0. Ranges 0..=5.
    pub net: u8,
}

/// Tagged-union response shape for `action=remember`.
///
/// `Created` covers low + moderate overlap, `Blocked` reports safe default
/// high-overlap behavior, and explicit `autofix` calls produce either
/// `Merged` or a non-mutating `Conflict`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MemoryRememberResponse {
    /// The memory was successfully inserted. `related_memories` is empty
    /// for a low-overlap insert, or populated with the slugs of each
    /// cross-referenced match on a moderate-overlap insert. When at least
    /// one of those matches has already hit the 3-link cap, the
    /// `refresh_recommended` flag is set so the caller knows to surface a
    /// refresh prompt.
    Created {
        slug: String,
        related_memories: Vec<String>,
        refresh_recommended: bool,
    },

    /// The memory was blocked because a high-overlap match (score 4–5)
    /// already exists. The caller should follow `recommendation` (typically
    /// update the existing entry in place). `other_high_scoring` carries
    /// any additional slugs that also scored 4+ (empty in the common case).
    Blocked {
        reason: BlockReason,
        existing_slug: String,
        dimension_scores: DimensionBreakdown,
        recommended_action: RecommendedAction,
        other_high_scoring: Vec<String>,
    },

    /// An explicit `mode=autofix` request atomically merged into the existing
    /// high-overlap memory. `slug` remains the surviving stable identifier.
    Merged {
        slug: String,
        receipt: MemoryMergeReceipt,
    },

    /// The target memory changed after the caller's observed timestamp, so
    /// an autofix merge was rejected without silently overwriting it.
    Conflict {
        slug: String,
        expected_updated_at: String,
        actual_updated_at: String,
    },
}

/// Durable receipt returned after an atomic high-overlap autofix merge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryMergeReceipt {
    pub merged_into: String,
    pub expected_updated_at: String,
    pub updated_at: String,
}

/// Reason a memory insert was blocked. Currently only `HighOverlap` is
/// emitted; the enum exists so future block reasons (e.g. quota, validation)
/// fit the same wire shape.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BlockReason {
    HighOverlap,
}

/// Recommended follow-up action when a block is returned. Mirrors
/// [`cas_core::memory::OverlapRecommendation`] with a stable wire name.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecommendedAction {
    UpdateExisting,
    SurfaceForUserDecision,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecentRequest {
    /// Number of entries
    #[schemars(description = "Number of recent entries to return (default: 10)")]
    #[serde(default = "default_recent")]
    pub n: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemoryTierRequest {
    /// Entry ID
    #[schemars(description = "ID of the entry")]
    pub id: String,

    /// Memory tier
    #[schemars(description = "Memory tier: 'working', 'cold', or 'archive'")]
    pub tier: String,
}
