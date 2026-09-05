//! Core factory orchestration logic.
//!
//! FactoryCore manages PTY sessions for multiple Claude Code agents,
//! providing a clean API for spawning and managing workers.

use std::collections::HashMap;
use std::path::PathBuf;

use cas_mux::{Mux, MuxEvent, PaneKind};
use cas_recording::WriterStats;
use thiserror::Error;

use crate::config::FactoryConfig;
use crate::recording::RecordingManager;

/// Unique identifier for a pane
pub type PaneId = String;

/// Errors from factory operations
#[derive(Error, Debug)]
pub enum FactoryError {
    /// Worker already exists
    #[error("Worker '{0}' already exists")]
    WorkerExists(String),

    /// Worker not found
    #[error("Worker '{0}' not found")]
    WorkerNotFound(String),

    /// Session not found
    #[error("Session '{0}' not found")]
    SessionNotFound(String),

    /// Supervisor already spawned
    #[error("Supervisor already spawned")]
    SupervisorExists,

    /// Supervisor not spawned yet
    #[error("Supervisor not spawned")]
    NoSupervisor,

    /// Mux error
    #[error("Mux error: {0}")]
    Mux(String),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type for factory operations
pub type Result<T> = std::result::Result<T, FactoryError>;

/// Events from the factory
#[derive(Debug, Clone)]
pub enum FactoryEvent {
    /// A pane received output
    PaneOutput { pane_id: PaneId, data: Vec<u8> },
    /// A pane's process exited
    PaneExited {
        pane_id: PaneId,
        exit_code: Option<i32>,
    },
    /// Focus changed
    FocusChanged { from: Option<PaneId>, to: PaneId },
    /// A pane was added (could be worker or supervisor)
    PaneAdded { pane_id: PaneId },
    /// A pane was removed
    PaneRemoved { pane_id: PaneId },
}

impl From<MuxEvent> for FactoryEvent {
    fn from(event: MuxEvent) -> Self {
        match event {
            MuxEvent::PaneOutput { pane_id, data } => FactoryEvent::PaneOutput { pane_id, data },
            MuxEvent::PaneExited { pane_id, exit_code } => {
                FactoryEvent::PaneExited { pane_id, exit_code }
            }
            MuxEvent::FocusChanged { from, to } => FactoryEvent::FocusChanged { from, to },
            MuxEvent::PaneAdded { pane_id } => FactoryEvent::PaneAdded { pane_id },
            MuxEvent::PaneRemoved { pane_id } => FactoryEvent::PaneRemoved { pane_id },
        }
    }
}

/// Information about a pane
#[derive(Debug, Clone)]
pub struct PaneInfo {
    /// Pane identifier (agent name)
    pub id: PaneId,
    /// Kind of pane (worker, supervisor, etc.)
    pub kind: PaneKind,
    /// Whether this pane is currently focused
    pub focused: bool,
}

/// Core factory orchestration.
///
/// Manages the lifecycle of Claude Code agent PTY sessions,
/// abstracting away the details of the underlying mux system.
pub struct FactoryCore {
    /// Terminal multiplexer
    mux: Mux,
    /// Factory configuration
    config: FactoryConfig,
    /// Supervisor name (set after spawn)
    supervisor_name: Option<String>,
    /// Worker names
    worker_names: Vec<String>,
    /// Per-worker working directories
    worker_cwds: HashMap<String, PathBuf>,
    /// CAS root directory (for CAS_ROOT env var)
    cas_root: Option<PathBuf>,
    /// Terminal dimensions
    rows: u16,
    cols: u16,
    /// Recording manager (if recording is enabled)
    recording: Option<RecordingManager>,
}

impl FactoryCore {
    /// Create a new FactoryCore with the given configuration.
    ///
    /// This initializes the mux but does not spawn any agents yet.
    /// Call `spawn_supervisor()` to start the supervisor, then
    /// `spawn_worker()` to add workers.
    ///
    /// If `config.record` is true and running within a tokio runtime,
    /// recording will be automatically enabled for all spawned agents.
    pub fn new(config: FactoryConfig) -> Result<Self> {
        // Get terminal size (use default if running without terminal)
        let (cols, rows) = crossterm::terminal::size().unwrap_or((120, 40));

        // Create mux with no panes initially.
        // Build the default spec from singular fields then layer on any per-worker
        // overrides from the cascade resolver so `spawn_worker` picks them up.
        let mut mux = Mux::new(rows, cols);
        let default_effort = config
            .worker_effort
            .as_deref()
            .and_then(|s| s.parse::<cas_mux::Effort>().ok());
        mux.set_default_worker_spec(cas_mux::WorkerSpec {
            name: None,
            cli: config.worker_cli,
            model: config.worker_model.clone(),
            effort: default_effort,
            config_dir: None,
            requester_config_dir: None,
            requester_secure_storage_dir: None,
        });
        for spec in &config.resolved_worker_specs {
            if let Some(ref name) = spec.name {
                mux.set_worker_spec(name, spec.clone());
            }
        }

        // Initialize recording if enabled
        let recording = if config.record {
            let session_id = config
                .session_id
                .clone()
                .unwrap_or_else(generate_session_id);
            // RecordingManager::new requires a tokio runtime context
            // If we're not in one, recording will be disabled
            match tokio::runtime::Handle::try_current() {
                Ok(_) => Some(RecordingManager::new(session_id, cols, rows, None)),
                Err(_) => {
                    tracing::warn!("Recording enabled but no tokio runtime available");
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            mux,
            config,
            supervisor_name: None,
            worker_names: Vec::new(),
            worker_cwds: HashMap::new(),
            cas_root: None,
            rows,
            cols,
            recording,
        })
    }

    /// Set the CAS root directory.
    ///
    /// This path is passed to agents via CAS_ROOT env var,
    /// allowing workers in clone directories to access the main repo's CAS state.
    pub fn set_cas_root(&mut self, path: PathBuf) {
        self.cas_root = Some(path);
    }

    /// Spawn the supervisor agent.
    ///
    /// # Arguments
    /// * `name` - Optional name for the supervisor. If None, uses config or generates one.
    ///
    /// # Returns
    /// The pane ID on success.
    pub fn spawn_supervisor(&mut self, name: Option<&str>) -> Result<PaneId> {
        if self.supervisor_name.is_some() {
            return Err(FactoryError::SupervisorExists);
        }

        let supervisor_name = name
            .map(String::from)
            .or_else(|| self.config.supervisor_name.clone())
            .unwrap_or_else(|| "supervisor".to_string());

        // Create supervisor pane via mux
        let pane = cas_mux::Pane::supervisor(
            &supervisor_name,
            self.config.cwd.clone(),
            self.cas_root.as_ref(),
            self.rows,
            self.cols,
            self.config.supervisor_cli,
            self.config.worker_cli,
            &self.worker_names,
            self.config.supervisor_model.as_deref(),
            self.config.supervisor_effort.as_deref(),
            None, // teams: cas-factory doesn't use native Agent Teams yet
            None,
        )
        .map_err(|e| FactoryError::Mux(e.to_string()))?;

        self.mux.add_pane(pane);
        self.mux.focus(&supervisor_name);
        self.supervisor_name = Some(supervisor_name.clone());

        // Start recording if enabled
        if let Some(ref mut recording) = self.recording {
            recording.start_recording(&supervisor_name, "supervisor");
        }

        Ok(supervisor_name)
    }

    /// Spawn a worker agent.
    ///
    /// # Arguments
    /// * `name` - Worker name (also used as pane ID)
    /// * `cwd` - Optional working directory. If None, uses config cwd.
    ///
    /// # Returns
    /// The pane ID on success.
    pub fn spawn_worker(&mut self, name: &str, cwd: Option<PathBuf>) -> Result<PaneId> {
        // Check supervisor exists
        let supervisor_name = self
            .supervisor_name
            .as_ref()
            .ok_or(FactoryError::NoSupervisor)?;

        // Check worker doesn't already exist
        if self.worker_names.contains(&name.to_string()) {
            return Err(FactoryError::WorkerExists(name.to_string()));
        }

        // Determine working directory
        let worker_cwd = cwd.unwrap_or_else(|| self.config.cwd.clone());

        // Add worker via mux
        self.mux
            .add_worker(
                name,
                worker_cwd.clone(),
                self.cas_root.as_ref(),
                supervisor_name,
                None, // teams: cas-factory doesn't use native Agent Teams yet
                None, // spec: use Mux default
            )
            .map_err(|e| FactoryError::Mux(e.to_string()))?;

        // Track the worker
        self.worker_names.push(name.to_string());
        self.worker_cwds.insert(name.to_string(), worker_cwd);

        // Start recording if enabled
        if let Some(ref mut recording) = self.recording {
            recording.start_recording(name, "worker");
        }

        Ok(name.to_string())
    }

    /// Shutdown a worker by name.
    ///
    /// This kills the worker's PTY process, stops recording, and removes it from the mux.
    pub fn shutdown_worker(&mut self, name: &str) -> Result<()> {
        // Check worker exists
        if !self.worker_names.contains(&name.to_string()) {
            return Err(FactoryError::WorkerNotFound(name.to_string()));
        }

        // Stop recording if active
        if let Some(ref mut recording) = self.recording {
            recording.stop_recording(name);
        }

        // Remove from mux
        self.mux
            .remove_worker(name)
            .map_err(|e| FactoryError::Mux(e.to_string()))?;

        // Remove from tracking
        self.worker_names.retain(|n| n != name);
        self.worker_cwds.remove(name);

        Ok(())
    }

    /// Shutdown the supervisor.
    ///
    /// This kills the supervisor's PTY process, stops recording, and removes it from the mux.
    pub fn shutdown_supervisor(&mut self) -> Result<()> {
        let name = self
            .supervisor_name
            .take()
            .ok_or(FactoryError::NoSupervisor)?;

        // Stop recording if active
        if let Some(ref mut recording) = self.recording {
            recording.stop_recording(&name);
        }

        // Remove the supervisor pane (this drops the PTY, sending SIGHUP)
        self.mux.remove_pane(&name);

        Ok(())
    }

    /// Poll for events from all panes.
    ///
    /// Returns all pending events. Call this regularly to process
    /// pane output, exit events, etc. If recording is enabled,
    /// pane output is automatically written to recordings.
    pub fn poll_events(&mut self) -> Vec<FactoryEvent> {
        let (_bytes, mux_events) = self.mux.poll_batch();

        // Convert events and record output if recording is enabled
        let events: Vec<FactoryEvent> = mux_events.into_iter().map(FactoryEvent::from).collect();

        // Write output events to recordings
        if let Some(ref recording) = self.recording {
            for event in &events {
                if let FactoryEvent::PaneOutput { pane_id, data } = event {
                    recording.write_output(pane_id, data);
                }
            }
        }

        events
    }

    /// Get information about all panes.
    pub fn panes(&self) -> Vec<PaneInfo> {
        self.mux
            .panes()
            .map(|pane| PaneInfo {
                id: pane.id().to_string(),
                kind: pane.kind().clone(),
                focused: pane.is_focused(),
            })
            .collect()
    }

    /// Get the supervisor name, if spawned.
    pub fn supervisor_name(&self) -> Option<&str> {
        self.supervisor_name.as_deref()
    }

    /// Get the worker names.
    pub fn worker_names(&self) -> &[String] {
        &self.worker_names
    }

    /// Get mutable access to the underlying mux.
    ///
    /// Use this for operations not exposed by FactoryCore,
    /// like sending input to panes or resizing.
    pub fn mux_mut(&mut self) -> &mut Mux {
        &mut self.mux
    }

    /// Get read-only access to the underlying mux.
    pub fn mux(&self) -> &Mux {
        &self.mux
    }

    /// Focus a specific pane by name.
    pub fn focus(&mut self, name: &str) {
        self.mux.focus(name);
    }

    /// Get the terminal dimensions.
    pub fn terminal_size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Resize all panes.
    ///
    /// Also updates recordings with resize events if recording is enabled.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        let _ = self.mux.resize(cols, rows);

        // Update recordings with resize
        if let Some(ref mut recording) = self.recording {
            // Write resize event for all active recordings
            if let Some(ref name) = self.supervisor_name {
                recording.write_resize(name, cols, rows);
            }
            for name in &self.worker_names {
                recording.write_resize(name, cols, rows);
            }
        }
    }

    /// Check if recording is enabled.
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Get the recording session ID, if recording.
    pub fn recording_session_id(&self) -> Option<&str> {
        self.recording.as_ref().map(|r| r.session_id())
    }

    /// Get the recordings directory, if recording.
    pub fn recordings_dir(&self) -> Option<&PathBuf> {
        self.recording.as_ref().map(|r| r.recordings_dir())
    }

    /// Stop all recordings and return statistics.
    ///
    /// This should be called when shutting down the factory to ensure
    /// all recordings are properly finalized.
    pub fn stop_all_recordings(&mut self) -> HashMap<String, WriterStats> {
        if let Some(ref mut recording) = self.recording {
            recording.stop_all()
        } else {
            HashMap::new()
        }
    }

    /// Get the number of active recordings.
    pub fn active_recording_count(&self) -> usize {
        self.recording
            .as_ref()
            .map(|r| r.active_count())
            .unwrap_or(0)
    }
}

/// Generate a unique session ID for recordings.
fn generate_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    format!("factory-{timestamp:x}")
}

#[cfg(test)]
mod tests {
    use crate::core::*;

    fn test_config() -> FactoryConfig {
        FactoryConfig {
            cwd: std::env::temp_dir(),
            workers: 0,
            worker_names: vec![],
            supervisor_name: Some("test-supervisor".to_string()),
            supervisor_cli: cas_mux::SupervisorCli::Claude,
            worker_cli: cas_mux::SupervisorCli::Claude,
            supervisor_model: None,
            worker_model: None,
            supervisor_effort: None,
            worker_effort: None,
            enable_worktrees: false,
            worktree_root: None,
            notify: Default::default(),
            tabbed_workers: false,
            auto_prompt: Default::default(),
            record: false,
            session_id: None,
            teams_configs: std::collections::HashMap::new(),
            lead_session_id: None,
            minions_theme: false,
            resolved_worker_specs: vec![],
            resolved_supervisor_spec: None,
            stall_threshold_secs: crate::config::DEFAULT_STALL_THRESHOLD_SECS,
            stall_after_secs: crate::config::DEFAULT_SUPERVISOR_STALL_AFTER_SECS,
            ai_enrichment: crate::config::AiEnrichmentConfig::default(),
        }
    }

    #[test]
    fn test_factory_core_new() {
        let config = test_config();
        let core = FactoryCore::new(config);
        assert!(core.is_ok());

        let core = core.unwrap();
        assert!(core.supervisor_name().is_none());
        assert!(core.worker_names().is_empty());
    }

    #[test]
    fn test_spawn_worker_without_supervisor_fails() {
        let config = test_config();
        let mut core = FactoryCore::new(config).unwrap();

        let result = core.spawn_worker("worker-1", None);
        assert!(matches!(result, Err(FactoryError::NoSupervisor)));
    }

    #[test]
    fn test_shutdown_nonexistent_worker_fails() {
        let config = test_config();
        let mut core = FactoryCore::new(config).unwrap();

        let result = core.shutdown_worker("nonexistent");
        assert!(matches!(result, Err(FactoryError::WorkerNotFound(_))));
    }

    #[test]
    fn test_spawn_supervisor_twice_fails() {
        let config = test_config();
        let mut core = FactoryCore::new(config).unwrap();

        // First spawn should succeed (but may fail due to no TTY in test)
        let first = core.spawn_supervisor(None);
        if first.is_ok() {
            // Second spawn should fail
            let second = core.spawn_supervisor(None);
            assert!(matches!(second, Err(FactoryError::SupervisorExists)));
        }
    }

    #[test]
    fn test_poll_events_empty() {
        let config = test_config();
        let mut core = FactoryCore::new(config).unwrap();

        let events = core.poll_events();
        assert!(events.is_empty());
    }

    #[test]
    fn test_panes_empty_initially() {
        let config = test_config();
        let core = FactoryCore::new(config).unwrap();

        let panes = core.panes();
        assert!(panes.is_empty());
    }
}
