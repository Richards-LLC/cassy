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
pub(crate) mod me;
mod sync_queue;
mod syncer;
pub mod task_proposals;
// cas-c117: explicit, verified project↔team registration for `cas cloud sync`.
pub mod team_registration;

// T6: first-run backfill — `pub` so integration tests can call the inner seam.
pub use backfill::{BackfillOutcome, maybe_apply_team_backfill, maybe_apply_team_backfill_inner};
pub use config::{
    CanonicalIdCollision, CanonicalIdSource, CloudConfig, LocalRootIdentity, PersonalScopeNotice,
    TeamInfo, TeamScopeAdoption, adopt_team_scope_for_configs, canonical_id_from_cas_root,
    canonical_id_from_config_toml, clear_login_credentials, derive_canonical_id_from_git_remote,
    detect_canonical_id_collisions, get_project_canonical_id, invalidate_cached_project_id,
    maybe_adopt_team_scope, maybe_mark_personal_scope_notice, canonical_project_id,
    canonical_project_id_with_pin, normalize_project_canonical_id, normalized_git_remote_for_push,
    personal_scope_notice_for_configs, resolve_canonical_id, resolve_canonical_id_with_source,
    set_canonical_id_in_config_toml, should_adopt_canonical_id,
    store_login_credentials,
};
pub(crate) use config::{
    default_endpoint, is_acceptable_endpoint, normalize_git_remote_url, user_level_cloud_json_path,
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
pub use sync_queue::{EntityType, QueueHealth, QueuedSync, SyncOperation, SyncQueue};
pub use team_registration::{
    REGISTRATION_TIMEOUT, RegistrationFailure, RegistrationOutcome, TeamRegistration,
};
pub(crate) use syncer::entity_matches_project;
pub use syncer::{
    CloudSyncer, CloudSyncerConfig, ConflictAction, ConflictResolution, KNOWLEDGE_ENTITY,
    KnowledgePageRecord, KnowledgePullReport, PushPlan, PushScope, SyncConflict, SyncResult,
    TaskStatusTransition,
    TeamMemoriesResponse, TeamProject, TeamProjectsResponse, knowledge_share_scope,
};
