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
pub mod session;
pub mod spec_resolver;
pub use changes::{FileChangeInfo, GitFileStatus, SourceChangesInfo};
pub use config::{
    AiEnrichmentConfig, AutoPromptConfig, DEFAULT_STALL_THRESHOLD_SECS, EpicState, FactoryConfig,
    NotifyBackend, NotifyConfig,
};
pub use core::{FactoryCore, FactoryError, FactoryEvent, PaneId, PaneInfo, Result};
pub use director::{
    ActiveLeaseSummary, AgentSummary, DirectorData, DirectorStores, EpicGroup, TaskSummary,
};
pub use notify::{DaemonNotifier, notify_daemon, notify_socket_path};
pub use recording::RecordingManager;
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
