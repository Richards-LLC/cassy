//! Cloud sync module for Cassy
//!
//! Provides automatic synchronization of Cassy data with Cassy Cloud.
//!
//! # Components
//!
//! - [`CloudConfig`] - Cloud authentication and endpoint configuration
//! - [`SyncQueue`] - Persistent queue for pending sync operations
//! - [`CloudSyncer`] - Push/pull synchronization logic
//! - [`CloudCoordinator`] - Multi-agent coordination via cloud
//!
//! # Architecture
//!
//! Auto-sync works by:
//! 1. Queueing changes on write operations (non-blocking)
//! 2. Processing the queue during idle periods (daemon)
//! 3. Pulling latest changes on MCP server startup

mod backfill;
pub mod comments;
mod config;
mod coordinator;
pub mod device;
// T5: capability-gated cloud embeddings for distilled knowledge pages.
pub mod code_embeddings;
pub mod embeddings;
// M7 (cas-db6e): the daemon-tick drain that keeps every corpus embedded without
// anyone running `cas cloud sync`.
pub mod embed_drain;
/// GH #701: refuse to mint a cloud identity for a throwaway checkout.
pub mod ephemeral_project;
pub(crate) mod me;
/// GH #669: consume the cloud's per-project `aliases` record so alias-scoped
/// rows are attributed to their canonical project instead of counted foreign.
pub mod project_aliases;
mod sync_queue;
pub(crate) mod syncer;
pub mod task_proposals;
// cas-c117: explicit, verified project↔team registration for `cas cloud sync`.
pub mod team_registration;

// T6: first-run backfill — `pub` so integration tests can call the inner seam.
pub use backfill::{BackfillOutcome, maybe_apply_team_backfill, maybe_apply_team_backfill_inner};
pub use config::{
    CanonicalIdCollision, CanonicalIdSource, CloudConfig, LocalRootIdentity, PersonalScopeNotice,
    TeamInfo, TeamScopeAdoption, adopt_team_scope_for_configs, canonical_id_from_cas_root,
    canonical_id_from_config_toml, canonical_project_id, canonical_project_id_with_pin,
    clear_login_credentials, derive_canonical_id_from_git_remote, detect_canonical_id_collisions,
    git_origin_url,
    get_project_canonical_id, invalidate_cached_project_alias_class, invalidate_cached_project_id,
    maybe_adopt_team_scope, maybe_mark_personal_scope_notice, normalize_project_canonical_id,
    normalized_git_remote_for_push, personal_scope_notice_for_configs,
    project_aliases_from_config_toml, project_ids_match, project_ids_match_with_aliases,
    resolve_canonical_id, resolve_canonical_id_for_sync, resolve_canonical_id_with_source,
    set_canonical_id_in_config_toml,
    set_project_aliases_in_config_toml, should_adopt_canonical_id, store_login_credentials,
};
pub use ephemeral_project::{ProjectDurability, classify_project_root};

/// The pull ingest predicates, exposed so integration tests can measure the
/// GH #701 origin filter against captured cloud payloads. Not part of the
/// supported surface — production code calls these through `CloudSyncer::pull`.
#[doc(hidden)]
pub mod syncer_testing {
    /// Would `raw` be ingested as a row of `current_project_id`?
    pub fn accepts_entity(
        raw: &serde_json::Value,
        current_project_id: &str,
        entity_kind: &str,
    ) -> bool {
        super::syncer::pull::entity_matches_project(raw, current_project_id, entity_kind)
    }

    /// Would `raw` be ingested as a task-dependency edge of `current_project_id`?
    pub fn accepts_task_dependency(raw: &serde_json::Value, current_project_id: &str) -> bool {
        super::syncer::pull::task_dependency_matches_project(raw, current_project_id)
    }
}
pub(crate) use config::{
    default_endpoint, is_acceptable_endpoint, normalize_git_remote_url, user_level_cloud_json_path,
};
pub use project_aliases::{
    ProjectAliasRecord, fetch_project_alias_record, refresh_project_alias_record,
    select_alias_record,
};
// T2: /api/me fetch helpers — `pub` so integration tests can call them directly.
pub use coordinator::CloudCoordinator;
pub use device::DeviceConfig;
pub use embed_drain::{DRAIN_BATCH, DrainReport, drain_all_pending, embed_pending_history};
pub use embeddings::{
    EmbedReport, EmbedUnit, EmbeddingMeta, KnowledgeEmbedder, KnowledgeVectorCache, RateLimiter,
    VectorNamespace, drain_units, embed_pending_pages, history_commit_key, history_doc_key,
};
pub use me::{
    FetchTeamsOutcome, fetch_and_cache_teams, fetch_and_cache_teams_inner, teams_cache_stale,
};
pub(crate) use sync_queue::TaskSyncIntent;
pub use sync_queue::{
    EntityType, QUARANTINE_TASK, QUARANTINED_ROW_STATEMENTS, QueueHealth, QueuedSync,
    QuarantinedRow, SYNC_REVISION_STATEMENTS, SyncOperation, SyncQueue,
    TASK_DEPENDENCY_TOMBSTONE_RETENTION_DAYS, TASK_DEPENDENCY_TOMBSTONE_STATEMENTS,
    parse_wire_revision, wire_revision,
};
pub use syncer::{
    CloudSyncer, CloudSyncerConfig, ConflictAction, ConflictResolution, KNOWLEDGE_ENTITY,
    KnowledgePageRecord, KnowledgePullReport, PushBacklog, PushPlan, PushScope, SyncConflict,
    SyncResult, TaskStatusTransition, TeamMemoriesResponse, TeamProject, TeamProjectsResponse,
    knowledge_share_scope, push_reason_hint,
};
pub(crate) use syncer::{SyncWarningSummary, collect_sync_warnings, entity_matches_project};
pub use team_registration::{
    REGISTRATION_TIMEOUT, RegistrationFailure, RegistrationOutcome, TeamRegistration,
};
