//! Cloud sync module for CAS
//!
//! Provides automatic synchronization of CAS data with CAS Cloud.
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
pub(crate) mod me;
mod sync_queue;
mod syncer;

// T6: first-run backfill — `pub` so integration tests can call the inner seam.
pub use backfill::{BackfillOutcome, maybe_apply_team_backfill, maybe_apply_team_backfill_inner};
pub use config::{
    CanonicalIdCollision, CanonicalIdSource, CloudConfig, LocalRootIdentity, PersonalScopeNotice,
    TeamInfo, canonical_id_from_cas_root, canonical_id_from_config_toml,
    derive_canonical_id_from_git_remote, detect_canonical_id_collisions, get_project_canonical_id,
    invalidate_cached_project_id, maybe_mark_personal_scope_notice,
    personal_scope_notice_for_configs, resolve_canonical_id, resolve_canonical_id_with_source,
    set_canonical_id_in_config_toml, should_adopt_canonical_id,
};
pub(crate) use config::{default_endpoint, is_acceptable_endpoint, normalize_git_remote_url, user_level_cloud_json_path};
// T2: /api/me fetch helpers — `pub` so integration tests can call them directly.
pub use me::{FetchTeamsOutcome, fetch_and_cache_teams, fetch_and_cache_teams_inner,
    teams_cache_stale};
pub use coordinator::CloudCoordinator;
pub use device::DeviceConfig;
pub use sync_queue::{EntityType, QueuedSync, SyncOperation, SyncQueue};
pub use syncer::{
    CloudSyncer, CloudSyncerConfig, ConflictAction, ConflictResolution, SyncConflict, SyncResult,
    TeamMemoriesResponse, TeamProject, TeamProjectsResponse,
};
