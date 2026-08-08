//! Cloud synchronization logic
//!
//! Handles pushing queued changes to cloud and pulling updates from cloud.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::cloud::{CloudConfig, EntityType, SyncQueue};
use crate::types::{Entry, Rule, Skill};

mod knowledge;
pub use knowledge::{KNOWLEDGE_ENTITY, KnowledgePageRecord, KnowledgePullReport, knowledge_share_scope};
mod pull;
mod push;
mod team_push;

#[cfg(test)]
mod tests;

#[derive(Debug, Default, Clone, Serialize)]
pub struct SyncResult {
    /// Number of entries pushed
    pub pushed_entries: usize,
    /// Number of tasks pushed
    pub pushed_tasks: usize,
    /// Number of rules pushed
    pub pushed_rules: usize,
    /// Number of skills pushed
    pub pushed_skills: usize,
    /// Number of sessions pushed
    pub pushed_sessions: usize,
    /// Number of verifications pushed
    pub pushed_verifications: usize,
    /// Number of events pushed
    pub pushed_events: usize,
    /// Number of prompts pushed
    pub pushed_prompts: usize,
    /// Number of file changes pushed
    pub pushed_file_changes: usize,
    /// Number of commit links pushed
    pub pushed_commit_links: usize,
    /// Number of agents pushed
    pub pushed_agents: usize,
    /// Number of worktrees pushed
    pub pushed_worktrees: usize,
    /// Number of distilled knowledge pages pushed (T5)
    pub pushed_knowledge_pages: usize,
    /// Number of entries pulled
    pub pulled_entries: usize,
    /// Number of tasks pulled
    pub pulled_tasks: usize,
    /// Number of rules pulled
    pub pulled_rules: usize,
    /// Number of skills pulled
    pub pulled_skills: usize,
    /// Number of specs pulled
    pub pulled_specs: usize,
    /// Number of events pulled
    pub pulled_events: usize,
    /// Number of prompts pulled
    pub pulled_prompts: usize,
    /// Number of file changes pulled
    pub pulled_file_changes: usize,
    /// Number of commit links pulled
    pub pulled_commit_links: usize,
    /// Number of distilled knowledge pages pulled (T5)
    pub pulled_knowledge_pages: usize,
    /// Number of conflicts resolved
    pub conflicts_resolved: usize,
    /// Errors encountered during sync
    pub errors: Vec<String>,
    /// Duration of sync in milliseconds
    pub duration_ms: u64,
}

/// Entity selection for a personal queue-driven push.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PushScope {
    All,
    EntriesOnly,
    TasksOnly,
}

impl PushScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::EntriesOnly => "entries_only",
            Self::TasksOnly => "tasks_only",
        }
    }

    fn entity_type(self) -> Option<EntityType> {
        match self {
            Self::All => None,
            Self::EntriesOnly => Some(EntityType::Entry),
            Self::TasksOnly => Some(EntityType::Task),
        }
    }

    fn planned_keys(self) -> &'static [&'static str] {
        match self {
            Self::EntriesOnly => &["entries"],
            Self::TasksOnly => &["tasks"],
            Self::All => &[
                "entries",
                "tasks",
                "rules",
                "skills",
                "sessions",
                "verifications",
                "events",
                "prompts",
                "file_changes",
                "commit_links",
                "agents",
                "worktrees",
            ],
        }
    }
}

/// Read-only description of the next queue batch a push would attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PushPlan {
    pub source: &'static str,
    pub scope: PushScope,
    pub counts: BTreeMap<String, usize>,
    pub total_in_next_batch: usize,
    pub batch_limit: usize,
    /// Conservative saturation marker. At exactly the query limit the queue
    /// read cannot prove whether more matching rows exist, so callers must not
    /// present the selected count as the complete backlog size.
    pub batch_limit_reached: bool,
}

impl SyncResult {
    pub fn total_pushed(&self) -> usize {
        self.pushed_entries
            + self.pushed_tasks
            + self.pushed_rules
            + self.pushed_skills
            + self.pushed_sessions
            + self.pushed_verifications
            + self.pushed_events
            + self.pushed_prompts
            + self.pushed_file_changes
            + self.pushed_commit_links
            + self.pushed_agents
            + self.pushed_worktrees
            + self.pushed_knowledge_pages
    }

    pub fn total_pulled(&self) -> usize {
        self.pulled_entries
            + self.pulled_tasks
            + self.pulled_rules
            + self.pulled_skills
            + self.pulled_specs
            + self.pulled_events
            + self.pulled_prompts
            + self.pulled_file_changes
            + self.pulled_commit_links
            + self.pulled_knowledge_pages
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// Strategy for resolving sync conflicts
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictResolution {
    /// Remote version wins (default for team sync)
    #[default]
    RemoteWins,
    /// Local version wins
    LocalWins,
    /// Keep more recent version based on timestamps
    KeepRecent,
}

/// Action to take after conflict resolution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictAction {
    UseRemote,
    UseLocal,
    Skip,
}

/// A sync conflict that was resolved
#[derive(Debug, Clone)]
pub struct SyncConflict {
    /// Type of entity (entry, task, rule, skill)
    pub entity_type: String,
    /// ID of the entity
    pub entity_id: String,
    /// Local timestamp
    pub local_updated: chrono::DateTime<chrono::Utc>,
    /// Remote timestamp
    pub remote_updated: chrono::DateTime<chrono::Utc>,
    /// How it was resolved
    pub resolution: ConflictResolution,
    /// Action taken
    pub action: ConflictAction,
}

impl SyncConflict {
    /// Log this conflict for debugging
    #[cfg(debug_assertions)]
    pub fn log(&self) {
        eprintln!(
            "[CAS sync] Conflict resolved: {} {} local={} remote={} strategy={:?} action={:?}",
            self.entity_type,
            self.entity_id,
            self.local_updated.format("%H:%M:%S"),
            self.remote_updated.format("%H:%M:%S"),
            self.resolution,
            self.action,
        );
    }

    /// Log this conflict for debugging (no-op in release)
    #[cfg(not(debug_assertions))]
    pub fn log(&self) {
        // No-op in release builds
    }
}

/// Configuration for CloudSyncer
#[derive(Debug, Clone)]
pub struct CloudSyncerConfig {
    /// HTTP request timeout
    pub timeout: Duration,
    /// Maximum retry attempts per item
    pub max_retries: i32,
    /// Base backoff duration in milliseconds for exponential backoff
    pub backoff_base_ms: u64,
    /// Maximum items to sync per batch
    pub batch_size: usize,
    /// Maximum serialized (pre-gzip) payload size per HTTP request.
    /// The 4 MiB default is conservative against the cloud's 4 MiB compressed
    /// request cap; the personal path also checks the actual gzip size.
    pub max_payload_bytes: usize,
    /// Default conflict resolution strategy for team sync
    pub team_conflict_resolution: ConflictResolution,
}

impl Default for CloudSyncerConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_retries: 5,
            backoff_base_ms: 1000,
            batch_size: 50,
            max_payload_bytes: 4 * 1024 * 1024,
            team_conflict_resolution: ConflictResolution::RemoteWins,
        }
    }
}

impl CloudSyncerConfig {
    /// Calculate backoff duration for a given retry attempt using exponential backoff
    pub fn backoff_duration(&self, attempt: u32) -> Duration {
        // Exponential backoff: base_ms * 2^attempt
        let base = self.backoff_base_ms * (1 << attempt.min(6)); // Cap at 2^6 = 64x
        // Simple jitter using system time
        let jitter = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_millis() as u64 % (base / 10 + 1))
            .unwrap_or(0);
        Duration::from_millis(base + jitter)
    }
}

/// Cloud synchronization service
pub struct CloudSyncer {
    config: CloudSyncerConfig,
    queue: Arc<SyncQueue>,
    cloud_config: CloudConfig,
    /// Explicit project scope for callers that already own a concrete CAS
    /// root. Legacy callers leave this unset and retain cwd-based resolution.
    push_project_canonical_id: Option<String>,
}

impl CloudSyncer {
    /// Create a new cloud syncer
    pub fn new(
        queue: Arc<SyncQueue>,
        cloud_config: CloudConfig,
        config: CloudSyncerConfig,
    ) -> Self {
        Self {
            config,
            queue,
            cloud_config,
            push_project_canonical_id: None,
        }
    }

    /// Create a syncer whose personal push envelopes are pinned to the same
    /// project root that owns `queue` and `cloud_config`.
    pub fn new_for_project(
        queue: Arc<SyncQueue>,
        cloud_config: CloudConfig,
        config: CloudSyncerConfig,
        project_canonical_id: String,
    ) -> Self {
        Self {
            config,
            queue,
            cloud_config,
            push_project_canonical_id: Some(project_canonical_id),
        }
    }

    fn personal_push_project_id(&self) -> Result<String, crate::error::CasError> {
        self.push_project_canonical_id
            .clone()
            .or_else(crate::cloud::get_project_canonical_id)
            .ok_or_else(|| {
                crate::error::CasError::Other(
                    "Cannot sync: not inside a CAS project directory".to_string(),
                )
            })
    }

    /// Check if cloud sync is available (user logged in)
    pub fn is_available(&self) -> bool {
        self.cloud_config.is_logged_in()
    }

    /// Get the sync queue
    pub fn queue(&self) -> &SyncQueue {
        &self.queue
    }

    /// Resolve a sync conflict using the given strategy
    fn resolve_conflict(
        &self,
        entity_type: &str,
        entity_id: &str,
        local_time: chrono::DateTime<chrono::Utc>,
        remote_time: chrono::DateTime<chrono::Utc>,
        strategy: ConflictResolution,
    ) -> ConflictAction {
        let action = match strategy {
            ConflictResolution::RemoteWins => ConflictAction::UseRemote,
            ConflictResolution::LocalWins => ConflictAction::UseLocal,
            ConflictResolution::KeepRecent => {
                if remote_time > local_time {
                    ConflictAction::UseRemote
                } else if local_time > remote_time {
                    ConflictAction::UseLocal
                } else {
                    // Same timestamp, skip to avoid unnecessary writes
                    ConflictAction::Skip
                }
            }
        };

        // Log the conflict for debugging
        let conflict = SyncConflict {
            entity_type: entity_type.to_string(),
            entity_id: entity_id.to_string(),
            local_updated: local_time,
            remote_updated: remote_time,
            resolution: strategy,
            action,
        };
        conflict.log();

        action
    }
}

enum UpsertResult {
    Created,
    Updated,
    Skipped,
}

/// Response from pull endpoint
///
/// Entities are kept as raw JSON values so that per-entity project filtering can be applied
/// before deserialization. This lets us reject entities from foreign projects even if the
/// strongly-typed structs don't carry a `project_canonical_id` field.
#[derive(Debug, Deserialize)]
struct PullResponse {
    #[serde(default)]
    entries: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    tasks: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    rules: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    skills: Option<Vec<serde_json::Value>>,
    // cas-bba4: re-added entity kinds, formerly imported unscoped by the
    // inline `cas cloud pull` path that cas-ed15 collapsed. Each is
    // `Option<Vec<_>>` with `#[serde(default)]` so a cloud build that
    // omits the field deserializes cleanly (zero rows). `specs` is not
    // yet returned by the cloud as of 2026-05-12 — tracked in
    // `docs/requests/FEATURE-cloud-sync-pull-return-specs.md`.
    #[serde(default)]
    specs: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    events: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    prompts: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    file_changes: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    commit_links: Option<Vec<serde_json::Value>>,
    /// T5: distilled knowledge pages. Consumed by the dedicated
    /// `pull_knowledge_pages` path (which needs a `KnowledgeStore` the generic
    /// pull does not carry), declared here so the field is part of one
    /// documented response contract rather than two.
    #[serde(default)]
    #[allow(dead_code)]
    knowledge_pages: Option<Vec<serde_json::Value>>,
    pulled_at: Option<String>,
}

/// Response from team pull endpoint
///
/// Entities are kept as raw JSON values for the same reason as `PullResponse`.
#[derive(Debug, Deserialize)]
struct TeamPullResponse {
    #[serde(default)]
    entries: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    tasks: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    rules: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    skills: Option<Vec<serde_json::Value>>,
    pulled_at: Option<String>,
    #[allow(dead_code)]
    team_id: Option<String>,
    #[allow(dead_code)]
    status: Option<String>,
}

/// Response from team projects endpoint
#[derive(Debug, Deserialize)]
pub struct TeamProjectsResponse {
    pub projects: Vec<TeamProject>,
}

/// A project within a team
#[derive(Debug, Deserialize, Serialize)]
pub struct TeamProject {
    pub id: String,
    pub canonical_id: String,
    pub name: String,
    pub contributor_count: u32,
    pub memory_count: u32,
}

/// Response from team push endpoint
#[derive(Debug, Deserialize)]
struct TeamPushResponse {
    synced: SyncedCounts,
    /// cas-8ca5 / contract §5: the canonical project id the server's resolver
    /// mapped this push to. `None` on older cloud builds that predate the
    /// resolver echo — the client then leaves its local pin untouched.
    #[serde(default)]
    canonical_id: Option<String>,
    /// cas-8ca5 / contract §5: the normalized git remote the server matched us
    /// to. Compared (case-insensitively) against our local remote before we
    /// adopt `canonical_id`, so a shared machine is never silently re-homed.
    #[serde(default)]
    git_remote: Option<String>,
}

/// Response shape from the personal push endpoint (`POST /api/sync/push`).
///
/// Backward-compatible contract: every field is `#[serde(default)]` so a
/// JSON body that omits one or all of them still deserializes cleanly to
/// `PushResponse::default()`. Older cloud builds that do not yet emit
/// `skipped` will be observed as `skipped: None`, and the client falls back
/// to legacy "trust the 200" behavior.
///
/// # `skipped` semantics (paired with cas-d656 server change)
///
/// When the cloud server's push route encounters a row whose
/// `project_canonical_id` does not match the existing row at the same primary
/// key, Postgres `ON CONFLICT DO UPDATE ... WHERE false ... RETURNING`
/// silently excludes that row from the result set. The server tallies the
/// excluded count per entity type and surfaces it here so the client can:
///
/// 1. Emit a structured warning to ops/users.
/// 2. Leave the affected local queue items un-marked-synced so they remain
///    retryable instead of being silently dropped from the local sync queue.
///
/// The shape is a `HashMap<String, usize>` (rather than a fixed-field struct)
/// so the wire format can grow new entity types without forcing a
/// client-version bump. Keys are the same entity-type strings the client
/// already uses (`"entries"`, `"tasks"`, `"rules"`, `"skills"`, etc.).
///
/// See cas-f645 for the client defensive read; cas-d656 for the server.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct PushResponse {
    /// Per-entity-type count of rows the server skipped (cross-project
    /// conflicts). `None` when the server build predates this field.
    #[serde(default)]
    pub skipped: Option<HashMap<String, usize>>,
}

impl PushResponse {
    /// Number of rows the server reported skipped for `entity_type`.
    ///
    /// Returns `0` both when no skip was reported for that entity type and
    /// when the server omitted the `skipped` field entirely (older build).
    /// Callers cannot distinguish "no skip" from "old server" — that
    /// ambiguity is intentional: under either condition the safe behavior
    /// is the legacy mark-synced path. The server-side schema NOT-NULL
    /// constraint from cas-d656 + the route-level rejection from cas-0bdc
    /// remain the actual guards; this client read is a defensive cross-check.
    pub fn skipped_count_for(&self, entity_type: &str) -> usize {
        self.skipped
            .as_ref()
            .and_then(|m| m.get(entity_type))
            .copied()
            .unwrap_or(0)
    }
}

/// Sync counts in push response
#[derive(Debug, Default, Deserialize)]
struct SyncedCounts {
    #[serde(default)]
    entries: usize,
    #[serde(default)]
    tasks: usize,
    #[serde(default)]
    rules: usize,
    #[serde(default)]
    skills: usize,
    #[serde(default)]
    sessions: usize,
    #[serde(default)]
    verifications: usize,
    #[serde(default)]
    events: usize,
    #[serde(default)]
    prompts: usize,
    #[serde(default)]
    file_changes: usize,
    #[serde(default)]
    commit_links: usize,
    #[serde(default)]
    agents: usize,
    #[serde(default)]
    worktrees: usize,
}

/// Response from team memories endpoint
#[derive(Debug, Deserialize)]
pub struct TeamMemoriesResponse {
    pub project: Option<TeamMemoriesProject>,
    pub memories: TeamMemoriesData,
    #[serde(default)]
    pub contributors: Vec<String>,
    pub pulled_at: Option<String>,
}

/// Project info in team memories response
#[derive(Debug, Deserialize)]
pub struct TeamMemoriesProject {
    pub id: String,
    pub canonical_id: String,
    pub name: String,
}

/// Team memories data grouped by type
#[derive(Debug, Default, Deserialize)]
pub struct TeamMemoriesData {
    #[serde(default)]
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub skills: Vec<Skill>,
}

/// Grouped queued items by entity type and operation
#[derive(Default)]
struct GroupedQueuedItems {
    upsert_entries: Vec<serde_json::Value>,
    upsert_tasks: Vec<serde_json::Value>,
    upsert_rules: Vec<serde_json::Value>,
    upsert_skills: Vec<serde_json::Value>,
    upsert_sessions: Vec<serde_json::Value>,
    upsert_verifications: Vec<serde_json::Value>,
    upsert_events: Vec<serde_json::Value>,
    upsert_prompts: Vec<serde_json::Value>,
    upsert_file_changes: Vec<serde_json::Value>,
    upsert_commit_links: Vec<serde_json::Value>,
    upsert_agents: Vec<serde_json::Value>,
    upsert_worktrees: Vec<serde_json::Value>,
    delete_entries: Vec<String>,
    delete_tasks: Vec<String>,
    delete_rules: Vec<String>,
    delete_skills: Vec<String>,
    delete_sessions: Vec<String>,
    delete_verifications: Vec<String>,
    delete_events: Vec<String>,
    delete_prompts: Vec<String>,
    delete_file_changes: Vec<String>,
    delete_commit_links: Vec<String>,
    delete_agents: Vec<String>,
    delete_worktrees: Vec<String>,
}
