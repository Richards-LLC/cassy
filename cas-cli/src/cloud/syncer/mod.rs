//! Cloud synchronization logic
//!
//! Handles pushing queued changes to cloud and pulling updates from cloud.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::cloud::{CloudConfig, EntityType, SyncQueue};
use crate::error::CasError;
use crate::types::{Entry, Rule, Skill};

mod knowledge;
pub use knowledge::{
    KNOWLEDGE_ENTITY, KnowledgePageRecord, KnowledgePullReport, knowledge_share_scope,
};
pub(crate) mod pull;
pub(crate) use pull::{SyncWarningSummary, collect_sync_warnings, entity_matches_project};
mod push;
mod team_push;

#[cfg(test)]
mod tests;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct PushBacklog {
    /// Rows still eligible for a push attempt.
    pub pending: usize,
    /// Rows retained after reaching the retry limit (including parked rows).
    pub failed: usize,
    /// Operator-facing diagnostics from retained failed rows.
    pub failed_errors: Vec<String>,
    /// Terminal rows the cloud explicitly rejected, grouped by its reason.
    /// These are a subset of `failed`: the remainder are transport or payload
    /// failures that never received a per-row verdict.
    pub rejected_by_reason: BTreeMap<String, usize>,
}

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
    /// Number of task dependency edges pushed
    pub pushed_task_dependencies: usize,
    /// Number of distilled knowledge pages pushed (T5)
    pub pushed_knowledge_pages: usize,
    /// Rows the cloud kept a newer version of and the client therefore removed
    /// from the queue. These are successful outcomes, not failures: reporting
    /// them separately is what stops a benign LWW loss reading as "rows
    /// failed".
    pub skipped_lww_acked: usize,
    /// Terminal rows requeued once because only an older client build had
    /// parked them.
    pub requeued_after_upgrade: usize,
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
    /// Number of task dependency edges pulled
    pub pulled_task_dependencies: usize,
    /// Number of local dependency edges removed by a cloud deletion tombstone.
    pub deleted_task_dependencies: usize,
    /// Number of local edges a tombstone kept out of the push queue. These are
    /// edges another machine deleted; re-pushing them is the resurrection this
    /// client exists to prevent.
    pub skipped_task_dependencies_by_tombstone: usize,
    /// Number of local task dependency edges queued for cloud healing.
    pub healed_task_dependencies_to_cloud: usize,
    /// Number of task dependency edges materialized from the cloud during
    /// healing. This is separate from `pulled_task_dependencies` so callers
    /// can distinguish ordinary pull application from reconciliation.
    pub healed_task_dependencies_from_cloud: usize,
    /// Number of distilled knowledge pages pulled (T5)
    pub pulled_knowledge_pages: usize,
    /// Number of conflicts resolved
    pub conflicts_resolved: usize,
    /// Number of conflicts resolved in favor of the local row.
    pub conflicts_resolved_local: usize,
    /// Number of conflicts resolved in favor of the remote row.
    pub conflicts_resolved_remote: usize,
    /// Conflict decisions retained for verbose human-facing output.
    pub conflicts: Vec<SyncConflict>,
    /// Errors encountered during sync
    pub errors: Vec<String>,
    /// Number of personal queue batches fetched and attempted.
    pub batches_run: usize,
    /// Personal queue rows that remain after this push invocation.
    pub remaining_backlog: PushBacklog,
    /// Duration of sync in milliseconds
    pub duration_ms: u64,
    /// Task lifecycle changes actually applied by a pull. These are kept
    /// separately from `pulled_tasks`: an unchanged task body is not a
    /// lifecycle transition and must not make the operator think work was
    /// reopened or closed.
    pub task_status_transitions: Vec<TaskStatusTransition>,
}

/// An auditable task lifecycle transition applied from a cloud pull.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskStatusTransition {
    pub task_id: String,
    pub project_id: String,
    pub source: String,
    pub from: crate::types::TaskStatus,
    pub to: crate::types::TaskStatus,
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
                "task_dependencies",
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
    /// Total eligible personal rows in the selected scope.
    pub total_matching: usize,
    pub batch_limit: usize,
    /// Conservative saturation marker. At exactly the query limit the queue
    /// read cannot prove whether more matching rows exist, so callers must not
    /// present the selected count as the complete backlog size.
    pub batch_limit_reached: bool,
}

impl SyncResult {
    /// Retain one conflict decision and update its aggregate counters.
    pub fn record_conflict(&mut self, conflict: SyncConflict) {
        self.conflicts_resolved += 1;
        match conflict.action {
            ConflictAction::UseRemote => self.conflicts_resolved_remote += 1,
            ConflictAction::UseLocal | ConflictAction::Skip => {
                self.conflicts_resolved_local += 1;
            }
        }
        self.conflicts.push(conflict);
    }

    /// Retain a resolver-produced detail. Resolver calls that keep the local
    /// row are counted by their pull loop's `Skipped` branch; remote winners
    /// are counted here because their pull loop reports an applied update.
    pub(crate) fn record_conflict_detail(&mut self, conflict: SyncConflict) {
        if conflict.action == ConflictAction::UseRemote {
            self.conflicts_resolved += 1;
            self.conflicts_resolved_remote += 1;
        }
        self.conflicts.push(conflict);
    }

    /// Record a conflict where the local row was retained without a detailed
    /// remote timestamp (for example an append-only duplicate or terminal
    /// status guard).
    pub fn record_local_conflict(&mut self) {
        self.conflicts_resolved += 1;
        self.conflicts_resolved_local += 1;
    }

    /// Render the one-line dependency healing receipt when reconciliation did
    /// work. A quiet no-op is intentional for steady-state pulls.
    pub fn dependency_heal_summary(&self) -> Option<String> {
        (self.healed_task_dependencies_to_cloud > 0
            || self.healed_task_dependencies_from_cloud > 0
            || self.deleted_task_dependencies > 0
            || self.skipped_task_dependencies_by_tombstone > 0)
            .then(|| {
                let mut parts = vec![format!(
                    "healed {} edge(s) to cloud, {} from cloud",
                    self.healed_task_dependencies_to_cloud,
                    self.healed_task_dependencies_from_cloud
                )];
                if self.deleted_task_dependencies > 0 {
                    parts.push(format!(
                        "{} deleted by tombstone",
                        self.deleted_task_dependencies
                    ));
                }
                if self.skipped_task_dependencies_by_tombstone > 0 {
                    parts.push(format!(
                        "{} skipped by tombstone",
                        self.skipped_task_dependencies_by_tombstone
                    ));
                }
                parts.join(", ")
            })
    }

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
            + self.pushed_task_dependencies
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
            + self.pulled_task_dependencies
            + self.pulled_knowledge_pages
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Render errors for human-facing commands without repeating raw server
    /// JSON for every rejected row. Permanent ownership refusals are grouped
    /// by reason and retain a few IDs for queue drill-down.
    pub fn concise_errors(&self) -> Vec<String> {
        let mut parked: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut other: BTreeMap<String, usize> = BTreeMap::new();

        for error in &self.errors {
            let Some(fields) = error.strip_prefix("permanent cloud rejection: ") else {
                let concise = error
                    .split("; server response:")
                    .next()
                    .unwrap_or(error)
                    .to_string();
                *other.entry(concise).or_default() += 1;
                continue;
            };
            let reason = fields
                .split("; ")
                .find_map(|field| field.strip_prefix("reason="));
            let id = fields
                .split("; ")
                .find_map(|field| field.strip_prefix("id="));
            match (reason, id) {
                (Some(reason), Some(id)) => parked.entry(reason).or_default().push(id),
                _ => *other.entry(error.clone()).or_default() += 1,
            }
        }

        let mut summaries = parked
            .into_iter()
            .map(|(reason, ids)| {
                let samples = ids.iter().take(3).copied().collect::<Vec<_>>().join(", ");
                format!(
                    "{reason}: {} item(s) parked (sample IDs: {samples}); inspect with `cas cloud queue --verbose`",
                    ids.len()
                )
            })
            .collect::<Vec<_>>();
        summaries.extend(other.into_iter().map(|(error, count)| {
            if count == 1 {
                error
            } else {
                format!("{error} ({count} occurrences)")
            }
        }));
        summaries
    }
}

/// Strategy for resolving sync conflicts
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictResolution {
    /// Remote version wins (default for team sync)
    #[default]
    RemoteWins,
    /// Local version wins
    LocalWins,
    /// Keep more recent version based on timestamps
    KeepRecent,
}

impl ConflictResolution {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RemoteWins => "remote_wins",
            Self::LocalWins => "local_wins",
            Self::KeepRecent => "timestamp_lww",
        }
    }
}

/// Action to take after conflict resolution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictAction {
    UseRemote,
    UseLocal,
    Skip,
}

/// A sync conflict that was resolved
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncConflict {
    /// Type of entity (entry, task, rule, skill)
    pub entity_type: String,
    /// ID of the entity
    pub entity_id: String,
    /// Local timestamp
    pub local_updated: chrono::DateTime<chrono::Utc>,
    /// Remote timestamp
    pub remote_updated: chrono::DateTime<chrono::Utc>,
    /// Server revision this client last observed for the local row.
    pub local_revision: Option<i64>,
    /// Server revision carried by the incoming row.
    pub remote_revision: Option<i64>,
    /// How it was resolved
    pub resolution: ConflictResolution,
    /// Action taken
    pub action: ConflictAction,
}

impl SyncConflict {
    /// Log this conflict for debugging without writing directly to stderr.
    pub fn log(&self) {
        // Name the revisions when they exist: with revisions in play the
        // timestamps alone can look like the wrong side won, and that is
        // exactly the reading this feature exists to correct.
        let render = |revision: Option<i64>| {
            revision.map_or_else(|| "-".to_string(), |revision| revision.to_string())
        };
        tracing::debug!(
            "[Cassy sync] Conflict resolved: {} {} local={} remote={} rev_local={} rev_remote={} strategy={:?} action={:?}",
            self.entity_type,
            self.entity_id,
            self.local_updated.format("%H:%M:%S"),
            self.remote_updated.format("%H:%M:%S"),
            render(self.local_revision),
            render(self.remote_revision),
            self.resolution,
            self.action,
        );
    }

    /// Whether the winner was chosen by revision rather than by clock.
    pub fn decided_by_revision(&self) -> bool {
        matches!(
            (self.local_revision, self.remote_revision),
            (Some(local), Some(remote)) if local != remote
        ) && self.resolution == ConflictResolution::KeepRecent
    }

    /// Journal strategy label, so an audit can tell the two regimes apart.
    pub fn strategy_label(&self) -> &'static str {
        if self.decided_by_revision() {
            "revision"
        } else {
            self.resolution.as_str()
        }
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
    /// Explicit project scope for callers that already own a concrete Cassy
    /// root. Legacy callers leave this unset and retain cwd-based resolution.
    push_project_canonical_id: Option<String>,
    /// Optional normalized `origin` identity sent with personal pushes.
    /// Missing/non-git remotes deliberately remain absent from the envelope.
    personal_push_git_remote: Option<String>,
    /// The `.cas` root this syncer pushes for (cas-f64e ephemeral guard). `None`
    /// for the legacy process-wide constructor, which falls back to `find_cas_root`.
    push_cas_root: Option<std::path::PathBuf>,
    /// Conflict decisions are collected during pull application and folded
    /// into the returned `SyncResult` after the operation completes.
    conflict_log: Arc<Mutex<Vec<SyncConflict>>>,
    /// Server revisions carried by the rows of the pull currently being
    /// applied, keyed by (entity type, id).
    ///
    /// The revision travels on the raw wire row, but conflict resolution
    /// happens deep inside typed upsert helpers. Staging it here lets every
    /// entity kind consult it without threading an extra argument through six
    /// upsert signatures — and, more importantly, without one of them being
    /// forgotten and silently falling back to clock comparison.
    incoming_revisions: Arc<Mutex<HashMap<(String, String), i64>>>,
}

impl CloudSyncer {
    /// Create a new cloud syncer
    pub fn new(
        queue: Arc<SyncQueue>,
        cloud_config: CloudConfig,
        config: CloudSyncerConfig,
    ) -> Self {
        let personal_push_git_remote = crate::store::find_cas_root()
            .ok()
            .and_then(|cas_root| crate::cloud::normalized_git_remote_for_push(&cas_root));
        Self {
            config,
            queue,
            cloud_config,
            push_project_canonical_id: None,
            personal_push_git_remote,
            push_cas_root: None,
            conflict_log: Arc::new(Mutex::new(Vec::new())),
            incoming_revisions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create a syncer whose personal push envelopes are pinned to the same
    /// project root that owns `queue` and `cloud_config`.
    pub fn new_for_project(
        queue: Arc<SyncQueue>,
        cloud_config: CloudConfig,
        config: CloudSyncerConfig,
        project_canonical_id: String,
        cas_root: &std::path::Path,
    ) -> Self {
        Self {
            config,
            queue,
            cloud_config,
            push_project_canonical_id: Some(project_canonical_id),
            personal_push_git_remote: crate::cloud::normalized_git_remote_for_push(cas_root),
            push_cas_root: Some(cas_root.to_path_buf()),
            conflict_log: Arc::new(Mutex::new(Vec::new())),
            incoming_revisions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn personal_push_project_id(&self) -> Result<String, crate::error::CasError> {
        self.push_project_canonical_id
            .clone()
            .or_else(crate::cloud::get_project_canonical_id)
            .ok_or_else(|| {
                crate::error::CasError::Other(
                    "Cannot sync: not inside a Cassy project directory".to_string(),
                )
            })
    }

    /// Check if cloud sync is available (user logged in)
    pub fn is_available(&self) -> bool {
        self.cloud_config.is_logged_in()
    }

    /// Requeue terminal cloud failures whose version gate is satisfied by the
    /// current client before either personal or team push reads the queue.
    pub(crate) fn requeue_version_gated_items(&self) -> Result<usize, CasError> {
        let requeued = self
            .queue
            .requeue_version_gated_failures(env!("CARGO_PKG_VERSION"), self.config.max_retries)?;
        if requeued > 0 {
            tracing::debug!("requeued {requeued} version-gated item(s)");
        }
        Ok(requeued)
    }

    /// Give rows that only an older client build parked one fresh attempt.
    ///
    /// This is the general form of the version-gate requeue: a row parked by
    /// a 429 storm, a transport failure, or a server refusal an older client
    /// could not classify is not evidence that this build cannot push it.
    /// Permanent per-row rejections are excluded by the queue itself.
    pub(crate) fn requeue_stale_client_failures(&self) -> Result<usize, CasError> {
        let requeued = self
            .queue
            .requeue_stale_client_failures(env!("CARGO_PKG_VERSION"), self.config.max_retries)?;
        if requeued > 0 {
            tracing::debug!("requeued {requeued} item(s) parked by an older client build");
        }
        Ok(requeued)
    }

    /// Get the sync queue
    pub fn queue(&self) -> &SyncQueue {
        &self.queue
    }

    /// Resolve a sync conflict using the given strategy.
    ///
    /// Revision-free callers keep the historical timestamp behaviour exactly.
    fn resolve_conflict(
        &self,
        entity_type: &str,
        entity_id: &str,
        local_time: chrono::DateTime<chrono::Utc>,
        remote_time: chrono::DateTime<chrono::Utc>,
        strategy: ConflictResolution,
    ) -> ConflictAction {
        let local_revision = EntityType::parse(entity_type)
            .and_then(|entity| self.queue.revision(entity, entity_id).ok().flatten());
        let remote_revision = self
            .incoming_revisions
            .lock()
            .ok()
            .and_then(|revisions| {
                revisions
                    .get(&(entity_type.to_string(), entity_id.to_string()))
                    .copied()
            });
        self.resolve_conflict_with_revisions(
            entity_type,
            entity_id,
            local_time,
            remote_time,
            local_revision,
            remote_revision,
            strategy,
        )
    }

    /// Whether an incoming row supersedes the local one.
    ///
    /// The same rule the conflict resolver applies, for the paths that compare
    /// timestamps inline rather than going through `resolve_conflict`: server
    /// revisions decide when both sides have one, and the clock decides only
    /// when they do not. Those paths are the common case for personal pulls,
    /// so leaving them on the raw timestamp comparison would have made this
    /// feature reachable in tests and absent in practice.
    pub(crate) fn remote_supersedes_local(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        local_time: chrono::DateTime<chrono::Utc>,
        remote_time: chrono::DateTime<chrono::Utc>,
    ) -> bool {
        let local_revision = self.queue.revision(entity_type, entity_id).ok().flatten();
        let remote_revision = self.incoming_revisions.lock().ok().and_then(|revisions| {
            revisions
                .get(&(entity_type.as_str().to_string(), entity_id.to_string()))
                .copied()
        });
        match (local_revision, remote_revision) {
            (Some(local), Some(remote)) if local != remote => remote > local,
            _ => remote_time > local_time,
        }
    }

    /// Stage the server revision carried by a row about to be applied.
    pub(crate) fn note_incoming_revision(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        raw: &serde_json::Value,
    ) {
        let Some(revision) = crate::cloud::wire_revision(raw) else {
            return;
        };
        if let Ok(mut revisions) = self.incoming_revisions.lock() {
            revisions.insert(
                (entity_type.as_str().to_string(), entity_id.to_string()),
                revision,
            );
        }
    }

    pub(crate) fn clear_incoming_revisions(&self) {
        if let Ok(mut revisions) = self.incoming_revisions.lock() {
            revisions.clear();
        }
    }

    /// Resolve a sync conflict, preferring the server's per-row revisions over
    /// either machine's clock.
    ///
    /// Revisions are server-owned and monotonic, so when both sides carry one
    /// they are the truth about which row is newer and the timestamps are not
    /// consulted at all — that is what stops a machine with a wrong clock from
    /// silently winning or losing. When the revisions are equal, or when either
    /// side has none (a row this client has never pulled, or a cloud build that
    /// does not send them), the original timestamp comparison runs unchanged.
    ///
    /// `RemoteWins`/`LocalWins` are explicit operator choices and stay
    /// authoritative: revisions only arbitrate the "keep whichever is newer"
    /// question that `KeepRecent` asks.
    #[allow(clippy::too_many_arguments)]
    fn resolve_conflict_with_revisions(
        &self,
        entity_type: &str,
        entity_id: &str,
        local_time: chrono::DateTime<chrono::Utc>,
        remote_time: chrono::DateTime<chrono::Utc>,
        local_revision: Option<i64>,
        remote_revision: Option<i64>,
        strategy: ConflictResolution,
    ) -> ConflictAction {
        let action = match strategy {
            ConflictResolution::RemoteWins => ConflictAction::UseRemote,
            ConflictResolution::LocalWins => ConflictAction::UseLocal,
            ConflictResolution::KeepRecent => {
                match (local_revision, remote_revision) {
                    (Some(local), Some(remote)) if remote > local => ConflictAction::UseRemote,
                    (Some(local), Some(remote)) if local > remote => ConflictAction::UseLocal,
                    // Equal revisions mean the same server state; fall through
                    // so an unpushed local edit can still be recognised.
                    _ => {
                        if remote_time > local_time {
                            ConflictAction::UseRemote
                        } else if local_time > remote_time {
                            ConflictAction::UseLocal
                        } else {
                            // Same timestamp, skip to avoid unnecessary writes
                            ConflictAction::Skip
                        }
                    }
                }
            }
        };

        // Log the conflict for debugging
        let conflict = SyncConflict {
            entity_type: entity_type.to_string(),
            entity_id: entity_id.to_string(),
            local_updated: local_time,
            remote_updated: remote_time,
            local_revision,
            remote_revision,
            resolution: strategy,
            action,
        };
        conflict.log();
        if let Ok(mut conflicts) = self.conflict_log.lock() {
            conflicts.push(conflict);
        }

        action
    }

    pub(crate) fn clear_conflict_log(&self) {
        if let Ok(mut conflicts) = self.conflict_log.lock() {
            conflicts.clear();
        }
    }

    pub(crate) fn take_conflict_log(&self) -> Vec<SyncConflict> {
        self.conflict_log
            .lock()
            .map(|mut conflicts| std::mem::take(&mut *conflicts))
            .unwrap_or_default()
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
    /// Task dependency edges are opaque cloud blobs but use a dedicated
    /// envelope key so the pull path can materialize them after tasks.
    #[serde(default)]
    task_dependencies: Option<Vec<serde_json::Value>>,
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
    #[serde(default)]
    task_dependencies: Option<Vec<serde_json::Value>>,
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
    /// Legacy servers returned integer counts per entity. The live server
    /// returns `{inserted, updated, skipped}` objects instead. Keep the wire
    /// value raw so the team path can recognize both and fail closed on a
    /// malformed-but-present skip signal.
    #[serde(default)]
    synced: serde_json::Value,
    /// Newer cloud builds may return complete per-row outcomes at the top
    /// level. Keep this alongside `synced` so the team path can consume both
    /// response generations without changing the aggregate contract.
    #[serde(default)]
    rows: Option<Vec<PushRowResult>>,
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
    #[serde(skip)]
    raw_body: String,
}

/// A row the cloud accepted at HTTP level but refused to write because the
/// existing global-keyed row belongs to a different project or sync scope.
///
/// The client deliberately keeps this wire type strict.  An itemized response
/// is useful only when it names a unique local queue row; malformed itemization
/// must therefore retain the whole sub-batch for retry rather than guessing.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PushRejection {
    pub id: String,
    pub reason: PushRejectionReason,
    pub existing_canonical_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PushRejectionReason {
    ProjectMismatch,
    ScopeMismatch,
}

impl PushRejectionReason {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::ProjectMismatch => "project_mismatch",
            Self::ScopeMismatch => "scope_mismatch",
        }
    }

    /// These identify an immutable ownership collision in the old global-key
    /// server identity model. Retrying cannot change the outcome.
    pub(crate) fn is_permanent(&self) -> bool {
        matches!(self, Self::ProjectMismatch | Self::ScopeMismatch)
    }
}

/// A row the cloud excluded because its revision cannot be accepted.
///
/// Unlike [`PushRejection`], this is a property of the submitted revision
/// itself, rather than a project/scope ownership collision. The server returns
/// these under the optional per-entity `invalid` sibling so clients can name
/// the precise queue row instead of reducing it to an aggregate skip count.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PushInvalid {
    pub id: String,
    pub reason: PushInvalidReason,
    /// Server-provided explanation; preserved as JSON so future cloud builds
    /// can enrich it without making an otherwise actionable row opaque.
    pub detail: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PushInvalidReason {
    InvalidRevision,
}

impl PushInvalidReason {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidRevision => "invalid_revision",
        }
    }
}

/// A server-itemized row that must remain visible in the local sync queue.
#[derive(Debug, Clone)]
pub(crate) enum PushItemizedFailure {
    Rejection(PushRejection),
    Invalid(PushInvalid),
}

/// Outcome for one row in a push response. The server may include these rows
/// in addition to aggregate counts so the client can distinguish a benign LWW
/// loss from a rejected write without retrying or parking neighboring rows.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PushRowOutcome {
    Inserted,
    Updated,
    SkippedLww,
    Rejected,
}

/// A complete per-row push result returned by newer cloud builds.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct PushRowResult {
    pub id: String,
    pub outcome: PushRowOutcome,
    #[serde(default)]
    pub reason: Option<String>,
}

impl PushRowResult {
    pub(crate) fn acknowledges(&self) -> bool {
        matches!(
            self.outcome,
            PushRowOutcome::Inserted | PushRowOutcome::Updated | PushRowOutcome::SkippedLww
        )
    }

    /// Rejection reasons which describe a temporary server-side condition.
    /// Unknown rejection reasons remain parked: losing a diagnostic row is
    /// worse than requiring an operator to explicitly requeue it.
    pub(crate) fn rejection_is_retryable(&self) -> bool {
        let Some(reason) = self.reason.as_deref() else {
            return false;
        };
        matches!(
            reason.to_ascii_lowercase().as_str(),
            // A stale base revision is a lost race, not a bad row: the next
            // cycle pulls the winning state and retries from a valid base.
            "revision_conflict"
                | "retryable"
                | "temporary"
                | "transient"
                | "server_error"
                | "internal_error"
                | "service_unavailable"
                | "rate_limited"
                | "timeout"
        )
    }
}

/// Whether a cloud rejection reason describes a condition no client retry can
/// repair. Permanent reasons must survive a client upgrade: requeueing them
/// only replays the same refusal and hides the row's real diagnosis.
pub(crate) fn push_reason_is_permanent(reason: &str) -> bool {
    matches!(
        reason.trim().to_ascii_lowercase().as_str(),
        "project_mismatch" | "scope_mismatch"
    )
}

/// The operator-facing next step for one cloud rejection reason.
///
/// Reporting a rejection without the repair leaves an operator with a count
/// and no move; an unknown reason is named honestly rather than guessed at.
pub fn push_reason_hint(reason: &str) -> &'static str {
    match reason.trim().to_ascii_lowercase().as_str() {
        "project_mismatch" => {
            "another project already owns this id in the cloud; re-link with `cas cloud link`, then `cas cloud queue --retry-reason project_mismatch`"
        }
        "scope_mismatch" => {
            "the cloud row belongs to a different sync scope (personal vs team); push it from the owning scope, then `cas cloud queue --retry-reason scope_mismatch`"
        }
        "revision_conflict" | "invalid_revision" => {
            "the cloud holds a newer revision; run `cas cloud pull`, then `cas cloud queue --retry`"
        }
        "version_gate" => {
            "this build is below the cloud's minimum client version; upgrade cas — the rows requeue themselves on the first push after the upgrade"
        }
        "sync_limit_exceeded" => {
            "the plan entity quota is exhausted; raise the plan limit or prune synced entities, then `cas cloud queue --retry`"
        }
        _ => {
            "unrecognized cloud reason; inspect `cas cloud queue --verbose` and report the diagnostic"
        }
    }
}

/// Parse and validate a complete per-row result list for one entity response.
/// A present list must cover exactly the submitted queue rows so an omitted
/// result can never be mistaken for an acknowledgement.
pub(crate) fn row_results_for(
    entity: &serde_json::Value,
    location: &str,
    queued_ids: impl Iterator<Item = String>,
) -> Result<Option<HashMap<String, PushRowResult>>, String> {
    let Some(detail) = entity.as_object() else {
        return Ok(None);
    };
    let Some(value) = detail.get("rows") else {
        return Ok(None);
    };
    let rows: Vec<PushRowResult> = serde_json::from_value(value.clone())
        .map_err(|error| format!("unrecognized {location}.rows: {error}"))?;
    let queued_ids = queued_ids.collect::<std::collections::HashSet<_>>();
    let mut by_id = HashMap::with_capacity(rows.len());
    for row in rows {
        if !queued_ids.contains(&row.id) {
            return Err(format!(
                "{location}.rows names row {} that was not in this sub-batch",
                row.id
            ));
        }
        if by_id.insert(row.id.clone(), row).is_some() {
            return Err(format!("{location}.rows contains a duplicate id"));
        }
    }
    for id in queued_ids {
        if !by_id.contains_key(&id) {
            return Err(format!("{location}.rows missing row {id}"));
        }
    }
    Ok(Some(by_id))
}

impl PushItemizedFailure {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Rejection(rejection) => &rejection.id,
            Self::Invalid(invalid) => &invalid.id,
        }
    }
}

/// Parse and validate the server's optional per-row rejection list.
///
/// `None` preserves the aggregate-only server contract. `Some` may itemize a
/// subset of skipped rows: the server also counts benign same-scope stale/no-op
/// writes as skipped, but only identity collisions have actionable rejection
/// details. Callers can safely fail the named rows and settle the remainder.
pub(crate) fn itemized_rejections_for(
    entity: &serde_json::Value,
    location: &str,
    skipped: usize,
    queued_ids: impl Iterator<Item = String>,
) -> Result<Option<HashMap<String, PushRejection>>, String> {
    let detail = entity
        .as_object()
        .ok_or_else(|| format!("{location} is not an object: {entity}"))?;
    let Some(value) = detail.get("rejected") else {
        return Ok(None);
    };
    let rejected: Vec<PushRejection> = serde_json::from_value(value.clone())
        .map_err(|error| format!("unrecognized {location}.rejected: {error}"))?;

    if rejected.len() > skipped {
        return Err(format!(
            "{location}.rejected count {} exceeds skipped count {skipped}",
            rejected.len()
        ));
    }

    let queued_ids = queued_ids.collect::<std::collections::HashSet<_>>();
    let mut by_id = HashMap::with_capacity(rejected.len());
    for rejection in rejected {
        if !queued_ids.contains(&rejection.id) {
            return Err(format!(
                "{location}.rejected names row {} that was not in this sub-batch",
                rejection.id
            ));
        }
        if by_id.insert(rejection.id.clone(), rejection).is_some() {
            return Err(format!("{location}.rejected contains a duplicate id"));
        }
    }
    Ok(Some(by_id))
}

/// Parse and validate optional per-row malformed-revision diagnostics.
///
/// As with rejections, malformed itemization fails closed: accepting a 2xx
/// without a unique local queue mapping would silently discard a row.
pub(crate) fn itemized_invalids_for(
    entity: &serde_json::Value,
    location: &str,
    skipped: usize,
    queued_ids: impl Iterator<Item = String>,
) -> Result<Option<HashMap<String, PushInvalid>>, String> {
    let detail = entity
        .as_object()
        .ok_or_else(|| format!("{location} is not an object: {entity}"))?;
    let Some(value) = detail.get("invalid") else {
        return Ok(None);
    };
    let invalid: Vec<PushInvalid> = serde_json::from_value(value.clone())
        .map_err(|error| format!("unrecognized {location}.invalid: {error}"))?;

    if invalid.len() > skipped {
        return Err(format!(
            "{location}.invalid count {} exceeds skipped count {skipped}",
            invalid.len()
        ));
    }

    let queued_ids = queued_ids.collect::<std::collections::HashSet<_>>();
    let mut by_id = HashMap::with_capacity(invalid.len());
    for invalid in invalid {
        if !queued_ids.contains(&invalid.id) {
            return Err(format!(
                "{location}.invalid names row {} that was not in this sub-batch",
                invalid.id
            ));
        }
        if by_id.insert(invalid.id.clone(), invalid).is_some() {
            return Err(format!("{location}.invalid contains a duplicate id"));
        }
    }
    Ok(Some(by_id))
}

/// Parse all optional itemized failure siblings for one entity response.
///
/// `rejected[]` keeps its existing ownership-collision contract. `invalid[]`
/// is an independent sibling for malformed revisions; the combined list must
/// still be a subset of the server's aggregate skipped count and every id must
/// map uniquely to this local sub-batch.
pub(crate) fn itemized_failures_for(
    entity: &serde_json::Value,
    location: &str,
    skipped: usize,
    queued_ids: impl Iterator<Item = String>,
) -> Result<Option<HashMap<String, PushItemizedFailure>>, String> {
    let queued_ids = queued_ids.collect::<Vec<_>>();
    let rejections =
        itemized_rejections_for(entity, location, skipped, queued_ids.iter().cloned())?;
    let invalids = itemized_invalids_for(entity, location, skipped, queued_ids.into_iter())?;

    if rejections.is_none() && invalids.is_none() {
        return Ok(None);
    }

    let mut failures = HashMap::new();
    for rejection in rejections.unwrap_or_default().into_values() {
        failures.insert(
            rejection.id.clone(),
            PushItemizedFailure::Rejection(rejection),
        );
    }
    for invalid in invalids.unwrap_or_default().into_values() {
        if failures
            .insert(invalid.id.clone(), PushItemizedFailure::Invalid(invalid))
            .is_some()
        {
            return Err(format!(
                "{location}.rejected and {location}.invalid contain a duplicate id"
            ));
        }
    }

    if failures.len() > skipped {
        return Err(format!(
            "{location}.rejected plus {location}.invalid count {} exceeds skipped count {skipped}",
            failures.len()
        ));
    }
    Ok(Some(failures))
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
/// 2. Consume rows named by per-row rejection results and park or retry them
///    according to the supplied reason; aggregate-only skips are acknowledged
///    under the server's LWW semantics because their row identities are absent.
///
/// Both the proposed top-level map and the live per-entity result objects are
/// accepted so the wire format can evolve without silently losing skips.
///
/// See cas-f645 for the client defensive read; cas-d656 for the server.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct PushResponse {
    /// Forward-looking top-level per-entity skip map. Kept as raw JSON so a
    /// malformed-but-present skip signal cannot deserialize away into the
    /// legacy "no skip report" path.
    #[serde(default)]
    pub skipped: Option<serde_json::Value>,

    /// Current cloud responses report counts inside each entity object, for
    /// example `{"tasks":{"inserted":0,"updated":0,"skipped":1}}`.
    /// Flattening preserves that shape alongside the older top-level map.
    #[serde(flatten)]
    pub fields: HashMap<String, serde_json::Value>,
    #[serde(skip)]
    pub raw_body: String,
}

impl PushResponse {
    /// Number of rows the server reported skipped for `entity_type`.
    ///
    /// Both the older proposed top-level map and the live nested entity shape
    /// are accepted. Absence remains backward-compatible (`Ok(0)`), but a
    /// present skip signal with an unknown type or contradictory counts is an
    /// error. Callers must then retain the affected queue rows for retry.
    pub fn skipped_count_for(&self, entity_type: &str) -> Result<usize, String> {
        fn count(value: &serde_json::Value, location: &str) -> Result<usize, String> {
            value
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| format!("unrecognized cloud skip count at {location}: {value}"))
        }

        let top_level = match self.skipped.as_ref() {
            None => None,
            Some(serde_json::Value::Object(by_entity)) => by_entity
                .get(entity_type)
                .map(|value| count(value, &format!("skipped.{entity_type}")))
                .transpose()?,
            Some(value) => {
                return Err(format!(
                    "unrecognized cloud skip report at skipped: {value}"
                ));
            }
        };

        let nested = self
            .fields
            .get(entity_type)
            .and_then(serde_json::Value::as_object)
            .and_then(|entity| entity.get("skipped"))
            .map(|value| count(value, &format!("{entity_type}.skipped")))
            .transpose()?;

        match (top_level, nested) {
            (Some(top), Some(nested)) if top != nested => Err(format!(
                "conflicting cloud skip counts for {entity_type}: top-level={top}, nested={nested}"
            )),
            (Some(value), _) | (_, Some(value)) => Ok(value),
            (None, None) => Ok(0),
        }
    }

    pub(crate) fn itemized_failures_for(
        &self,
        entity_type: &str,
        skipped: usize,
        queued_ids: impl Iterator<Item = String>,
    ) -> Result<Option<HashMap<String, PushItemizedFailure>>, String> {
        let Some(entity) = self.fields.get(entity_type) else {
            return Ok(None);
        };
        itemized_failures_for(entity, entity_type, skipped, queued_ids)
    }

    /// Locate the per-type detail object for `entity_type`.
    ///
    /// The personal route puts per-type keys at the top level; the team route
    /// nests them under `synced`. Both are checked so revision handling works
    /// on either envelope.
    fn entity_detail(&self, entity_type: &str) -> Option<&serde_json::Value> {
        self.fields.get(entity_type).or_else(|| {
            self.fields
                .get("synced")
                .and_then(serde_json::Value::as_object)
                .and_then(|synced| synced.get(entity_type))
        })
    }

    /// Server revisions echoed for rows this push accepted.
    ///
    /// Shape: `<type>.accepted[<id>] = {revision, canonical_id}`. Storing these
    /// keeps the client's base revision current without a follow-up pull; the
    /// next push for the row then declares a base the server will accept.
    pub(crate) fn accepted_revisions_for(&self, entity_type: &str) -> HashMap<String, i64> {
        let mut revisions = HashMap::new();
        let Some(accepted) = self
            .entity_detail(entity_type)
            .and_then(serde_json::Value::as_object)
            .and_then(|entity| entity.get("accepted"))
            .and_then(serde_json::Value::as_object)
        else {
            return revisions;
        };
        for (id, receipt) in accepted {
            if let Some(revision) = crate::cloud::parse_wire_revision(receipt.get("revision")) {
                revisions.insert(id.clone(), revision);
            }
        }
        revisions
    }

    /// Rows the server refused because our base revision was stale, mapped to
    /// the revision the server actually holds.
    pub(crate) fn revision_conflicts_for(&self, entity_type: &str) -> HashMap<String, Option<i64>> {
        let mut conflicts = HashMap::new();
        let Some(rejected) = self
            .entity_detail(entity_type)
            .and_then(serde_json::Value::as_object)
            .and_then(|entity| entity.get("rejected"))
            .and_then(serde_json::Value::as_array)
        else {
            return conflicts;
        };
        for rejection in rejected {
            let is_revision_conflict = rejection
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| reason == "revision_conflict");
            if !is_revision_conflict {
                continue;
            }
            if let Some(id) = rejection.get("id").and_then(serde_json::Value::as_str) {
                conflicts.insert(
                    id.to_string(),
                    crate::cloud::parse_wire_revision(rejection.get("current_revision")),
                );
            }
        }
        conflicts
    }

    pub(crate) fn row_results_for(
        &self,
        entity_type: &str,
        queued_ids: impl Iterator<Item = String>,
    ) -> Result<Option<HashMap<String, PushRowResult>>, String> {
        let queued_ids = queued_ids.collect::<Vec<_>>();
        if let Some(entity) = self.fields.get(entity_type) {
            if let Some(rows) = row_results_for(entity, entity_type, queued_ids.iter().cloned())? {
                return Ok(Some(rows));
            }
        }
        if let Some(rows) = self.fields.get("rows") {
            let wrapped = serde_json::json!({"rows": rows});
            return row_results_for(&wrapped, "rows", queued_ids.into_iter());
        }
        Ok(None)
    }
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
