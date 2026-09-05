//! Factory orchestration for CAS multi-agent coordination.
//!
//! This crate provides the core orchestration logic for managing multiple
//! Claude Code agents working together in a factory session.
//!
//! # Example
//!
//! ```ignore
//! use cas_factory::{FactoryCore, FactoryConfig};
//!
//! let config = FactoryConfig::default();
//! let mut factory = FactoryCore::new(config)?;
//!
//! // Spawn supervisor first
//! factory.spawn_supervisor(Some("my-supervisor"))?;
//!
//! // Then spawn workers
//! factory.spawn_worker("worker-1", None)?;
//! factory.spawn_worker("worker-2", None)?;
//!
//! // Poll for events
//! for event in factory.poll_events() {
//!     println!("Event: {:?}", event);
//! }
//! ```

pub mod changes;
pub mod config;
pub mod core;
pub mod director;
pub mod notify;
pub mod probe;
pub mod recording;
pub mod routing;
pub mod session;
pub mod spec_resolver;
pub use changes::{FileChangeInfo, GitFileStatus, SourceChangesInfo};
pub use config::{
    AiEnrichmentConfig, AutoPromptConfig, DEFAULT_STALL_THRESHOLD_SECS,
    DEFAULT_SUPERVISOR_STALL_AFTER_SECS, EpicState, FactoryConfig,
    NotifyBackend, NotifyConfig,
};
pub use core::{FactoryCore, FactoryError, FactoryEvent, PaneId, PaneInfo, Result};
pub use director::{
    ActiveLeaseSummary, AgentSummary, DirectorData, DirectorStores, EpicGroup, TaskSummary,
};
pub use notify::{DaemonNotifier, notify_daemon, notify_socket_path};
pub use recording::RecordingManager;
pub use routing::{
    CAPABILITY_AVAILABLE_TTL_MS, CAPABILITY_UNAVAILABLE_TTL_MS, CAPABILITY_UNKNOWN_TTL_MS,
    CapabilityAvailability, CapabilityEvidence, CapabilitySnapshot, CapabilityStatus, Lane,
    LaneDefinition, LaneRegistry, Recipe, RecipeStatus, RouteIdentity, RouteRecipe,
    RoutingDecision, RoutingError, default_worker_effort_for_cli, default_worker_model_for_cli,
    embedded_registry, parse_registry, recipe_route_identity, registered_harnesses, registry,
    render_route_table, render_spawn_recipes, resolve_lane, resolve_lane_from_registry,
    resolve_lane_specs, validate_explicit, validate_lane_request, validate_model_effort_policy,
    is_claude_model_slug, validate_model_is_active, validate_model_slug, validate_model_slug_with,
};
pub use session::lifecycle::SessionManager;
pub use session::resume::{
    SharedUnifiedSessionManager, UnifiedSessionConfig, UnifiedSessionManager,
    new_shared_unified_manager,
};
pub use session::state::{
    AgentState, SessionCache, SessionError, SessionId, SessionInfo, SessionState, SessionSummary,
    SessionType,
};
pub use spec_resolver::{
    ConfigSources, SpecResolverError, apply_codex_fallback, apply_codex_fallback_for_supervisor,
    configured_factory_default_model, resolve_specs, resolve_supervisor_spec,
    worker_slot_cli_configured, worker_slot_effort_configured,
};
