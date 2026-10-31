//! Integration tests for cas-factory crate.
//!
//! Tests cover:
//! - FactoryCore lifecycle (spawn/shutdown validation without real PTYs)
//! - DirectorData loading from CAS stores
//! - Config types and state management
//! - Git change tracking types

use std::path::PathBuf;

use cas_factory::{
    AutoPromptConfig, EpicState, FactoryConfig, FactoryCore, FactoryError, FileChangeInfo,
    GitFileStatus, NotifyBackend, NotifyConfig, SourceChangesInfo,
};
use cas_mux::SupervisorCli;

// =============================================================================
// FactoryConfig Tests
// =============================================================================

#[test]
fn test_factory_config_default() {
    let config = FactoryConfig::default();

    assert_eq!(config.workers, 0); // Supervisor-only by default
    assert!(config.worker_names.is_empty());
    assert!(config.supervisor_name.is_none());
    assert!(config.enable_worktrees);
    assert!(config.worktree_root.is_none());
    assert!(!config.tabbed_workers);
    assert!(!config.record);
    assert!(config.session_id.is_none());
}

// =============================================================================
// EpicState Tests
// =============================================================================

#[test]
fn test_epic_state_idle() {
    let state = EpicState::Idle;

    assert!(!state.is_active());
    assert!(state.epic_id().is_none());
    assert!(state.epic_title().is_none());
}

#[test]
fn test_epic_state_active() {
    let state = EpicState::Active {
        epic_id: "cas-1234".to_string(),
        epic_title: "Add feature X".to_string(),
    };

    assert!(state.is_active());
    assert_eq!(state.epic_id(), Some("cas-1234"));
    assert_eq!(state.epic_title(), Some("Add feature X"));
}

#[test]
fn test_epic_state_completing() {
    let state = EpicState::Completing {
        epic_id: "cas-5678".to_string(),
        epic_title: "Refactor Y".to_string(),
    };

    assert!(state.is_active());
    assert_eq!(state.epic_id(), Some("cas-5678"));
    assert_eq!(state.epic_title(), Some("Refactor Y"));
}

#[test]
fn test_epic_state_default() {
    let state = EpicState::default();

    assert!(matches!(state, EpicState::Idle));
    assert!(!state.is_active());
}

// =============================================================================
// NotifyConfig Tests
// =============================================================================

#[test]
fn test_notify_config_default() {
    let config = NotifyConfig::default();

    assert!(!config.enabled);
    assert!(!config.also_bell);
    // Backend is detected based on environment
}

#[test]
fn test_notify_backend_detect() {
    // This will return Native on macOS/Linux/Windows, Bell otherwise
    let backend = NotifyBackend::detect();

    // We can't assert a specific backend since it depends on environment
    // Just verify it's a valid variant
    match backend {
        NotifyBackend::Native | NotifyBackend::Bell | NotifyBackend::ITerm2 => {
            // Valid backend detected
        }
    }
}

// =============================================================================
// AutoPromptConfig Tests
// =============================================================================

#[test]
fn test_auto_prompt_config_default() {
    let config = AutoPromptConfig::default();

    assert!(config.enabled);
    assert!(config.on_task_assigned);
    assert!(config.on_task_completed);
    assert!(config.on_task_blocked);
    assert!(config.on_worker_idle);
    assert!(config.on_epic_completed);
    assert!(config.on_worker_ready);
}

// =============================================================================
// FactoryCore Lifecycle Tests
// =============================================================================

fn test_config() -> FactoryConfig {
    FactoryConfig {
        cwd: std::env::temp_dir(),
        workers: 0,
        worker_names: vec![],
        supervisor_name: Some("test-supervisor".to_string()),
        supervisor_cli: SupervisorCli::Claude,
        worker_cli: SupervisorCli::Claude,
        supervisor_model: None,
        worker_model: None,
        supervisor_effort: None,
        worker_effort: None,
        enable_worktrees: false,
        worktree_root: None,
        notify: NotifyConfig::default(),
        tabbed_workers: false,
        auto_prompt: AutoPromptConfig::default(),
        record: false,
        session_id: None,
        teams_configs: std::collections::HashMap::new(),
        lead_session_id: None,
        minions_theme: false,
        resolved_worker_specs: vec![],
        resolved_supervisor_spec: None,
        stall_threshold_secs: cas_factory::DEFAULT_STALL_THRESHOLD_SECS,
        stall_after_secs: cas_factory::DEFAULT_SUPERVISOR_STALL_AFTER_SECS,
        ai_enrichment: cas_factory::AiEnrichmentConfig::default(),
    }
}

#[test]
fn test_factory_core_new() {
    let config = test_config();
    let result = FactoryCore::new(config);

    assert!(result.is_ok());

    let core = result.unwrap();
    assert!(core.supervisor_name().is_none());
    assert!(core.worker_names().is_empty());
}

#[test]
fn test_factory_core_initial_state() {
    let config = test_config();
    let core = FactoryCore::new(config).unwrap();

    // Initially no panes
    assert!(core.panes().is_empty());
    assert!(core.supervisor_name().is_none());
    assert!(core.worker_names().is_empty());

    // Terminal size should be valid
    let (cols, rows) = core.terminal_size();
    assert!(cols > 0);
    assert!(rows > 0);
}

#[test]
fn test_spawn_worker_without_supervisor_fails() {
    let config = test_config();
    let mut core = FactoryCore::new(config).unwrap();

    // Spawning worker without supervisor should fail
    let result = core.spawn_worker("worker-1", None);
    assert!(matches!(result, Err(FactoryError::NoSupervisor)));
}

#[test]
fn test_shutdown_nonexistent_worker_fails() {
    let config = test_config();
    let mut core = FactoryCore::new(config).unwrap();

    // Shutting down non-existent worker should fail
    let result = core.shutdown_worker("nonexistent");
    assert!(matches!(result, Err(FactoryError::WorkerNotFound(_))));
}

#[test]
fn test_poll_events_empty_initially() {
    let config = test_config();
    let mut core = FactoryCore::new(config).unwrap();

    // No events on fresh factory
    let events = core.poll_events();
    assert!(events.is_empty());
}

#[test]
fn test_resize() {
    let config = test_config();
    let mut core = FactoryCore::new(config).unwrap();

    // Resize should work
    core.resize(100, 50);
    let (cols, rows) = core.terminal_size();
    assert_eq!(cols, 100);
    assert_eq!(rows, 50);
}

#[test]
fn test_set_cas_root() {
    let config = test_config();
    let mut core = FactoryCore::new(config).unwrap();

    let cas_root = PathBuf::from("/path/to/.cas");
    core.set_cas_root(cas_root);

    // No direct getter, but this should not panic
}

// =============================================================================
// Git Change Tracking Types Tests
// =============================================================================

#[test]
fn test_git_file_status_symbol() {
    assert_eq!(GitFileStatus::Modified.symbol(), "M");
    assert_eq!(GitFileStatus::Added.symbol(), "A");
    assert_eq!(GitFileStatus::Deleted.symbol(), "D");
    assert_eq!(GitFileStatus::Renamed.symbol(), "R");
    assert_eq!(GitFileStatus::Untracked.symbol(), "?");
}

// =============================================================================
// Factory Error Tests
// =============================================================================

#[test]
fn test_factory_error_display() {
    let worker_exists = FactoryError::WorkerExists("worker-1".to_string());
    assert!(worker_exists.to_string().contains("worker-1"));
    assert!(worker_exists.to_string().contains("exists"));

    let worker_not_found = FactoryError::WorkerNotFound("worker-2".to_string());
    assert!(worker_not_found.to_string().contains("worker-2"));
    assert!(worker_not_found.to_string().contains("not found"));

    let supervisor_exists = FactoryError::SupervisorExists;
    assert!(supervisor_exists.to_string().contains("Supervisor"));

    let no_supervisor = FactoryError::NoSupervisor;
    assert!(no_supervisor.to_string().contains("Supervisor"));

    let mux_error = FactoryError::Mux("test mux error".to_string());
    assert!(mux_error.to_string().contains("Mux"));
}
