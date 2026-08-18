//! Main multiplexer
//!
//! Manages multiple panes, handles input routing, and coordinates rendering.

use crate::error::{Error, Result};
use crate::harness::SupervisorCli;
use crate::pane::{Pane, PaneId, PaneKind, push_factory_session_env};
use crate::pty::{PtyConfig, PtyEvent, TeamsSpawnConfig};
use crate::spec::WorkerSpec;
use cas_factory_protocol::ServerMessage;
use indexmap::IndexMap;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Result of a non-urgent PTY injection attempt.
///
/// `Delivered` means the payload was written to the pane. A dirty-composer
/// deferral is deliberately distinct so durable queue owners do not advance
/// their transport-delivery state before any write occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectOutcome {
    /// The payload was written to the pane.
    Delivered,
    /// No write occurred because the composer is not at a safe turn boundary:
    /// either the attached operator has an active draft or a prior CAS payload
    /// is still awaiting its submit CR.
    DeferredComposerDirty,
}

/// Configuration for the multiplexer
#[derive(Debug, Clone)]
pub struct MuxConfig {
    /// Working directory for agents (supervisor and fallback for workers)
    pub cwd: PathBuf,
    /// Path to the .cas directory (if set, passes CAS_ROOT env var to agents)
    /// This allows workers in clone directories to access the main repo's CAS state.
    pub cas_root: Option<PathBuf>,
    /// Factory session name to tag spawned agent processes and their MCP servers.
    pub factory_session: Option<String>,
    /// Per-worker working directories (worker_name -> clone path)
    /// Workers not in this map use the default `cwd`.
    pub worker_cwds: HashMap<String, PathBuf>,
    /// Number of worker agents
    pub workers: usize,
    /// Worker names (if not provided, generated)
    pub worker_names: Vec<String>,
    /// Supervisor name
    pub supervisor_name: String,
    /// Supervisor CLI (codex, claude, or pi)
    pub supervisor_cli: SupervisorCli,
    /// Worker CLI (codex, claude, or pi)
    pub worker_cli: SupervisorCli,
    /// Model for supervisor (passed as --model flag)
    pub supervisor_model: Option<String>,
    /// Model for workers (passed as --model flag)
    pub worker_model: Option<String>,
    /// Reasoning effort for supervisor (passed as --effort flag; defaults to "xhigh")
    pub supervisor_effort: Option<String>,
    /// Reasoning effort for workers (passed as --effort flag; defaults to "high")
    pub worker_effort: Option<String>,
    /// Include director pane
    pub include_director: bool,
    /// Terminal size
    pub rows: u16,
    pub cols: u16,
    /// Per-agent Teams spawn configs (agent_name -> config).
    /// When set, agents are spawned with native Agent Teams CLI flags.
    pub teams_configs: HashMap<String, TeamsSpawnConfig>,
    /// Resolved per-worker specs from the cascade resolver.
    ///
    /// When non-empty, each named worker in this list overrides the singular
    /// `worker_cli/worker_model/worker_effort` for that specific worker slot.
    /// Workers without a matching entry fall back to the singular fields.
    /// Populated by `cas_factory::spec_resolver::resolve_specs` and passed
    /// through to `Mux::factory` / `Mux::factory_pane_configs`.
    pub resolved_worker_specs: Vec<WorkerSpec>,
}

impl Default for MuxConfig {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_default(),
            cas_root: None,
            factory_session: None,
            worker_cwds: HashMap::new(),
            workers: 2,
            worker_names: vec![],
            supervisor_name: "supervisor".to_string(),
            supervisor_cli: SupervisorCli::Codex,
            worker_cli: SupervisorCli::Claude,
            supervisor_model: None,
            worker_model: None,
            supervisor_effort: None,
            worker_effort: None,
            include_director: true,
            rows: 24,
            cols: 80,
            teams_configs: HashMap::new(),
            resolved_worker_specs: vec![],
        }
    }
}

/// Events from the multiplexer
#[derive(Debug, Clone)]
pub enum MuxEvent {
    /// A pane received output (includes raw bytes for client-side rendering)
    PaneOutput {
        pane_id: PaneId,
        /// Raw PTY bytes for client-side terminal emulation
        data: Vec<u8>,
    },
    /// A pane's process exited
    PaneExited {
        pane_id: PaneId,
        exit_code: Option<i32>,
    },
    /// Focus changed
    FocusChanged { from: Option<PaneId>, to: PaneId },
    /// A pane was added
    PaneAdded { pane_id: PaneId },
    /// A pane was removed
    PaneRemoved { pane_id: PaneId },
}

/// The main terminal multiplexer
pub struct Mux {
    /// All panes, keyed by ID (insertion order preserved)
    panes: IndexMap<PaneId, Pane>,
    /// Currently focused pane
    focused: Option<PaneId>,
    /// Event sender
    event_tx: mpsc::Sender<MuxEvent>,
    /// Event receiver
    event_rx: mpsc::Receiver<MuxEvent>,
    /// Terminal size
    rows: u16,
    cols: u16,
    /// Default spec applied to all dynamic worker spawns that have no per-worker
    /// override. Mutated by the deprecated `set_worker_cli/model/effort` setters
    /// (preserved for callers that haven't switched to `set_worker_spec` yet).
    default_worker_spec: WorkerSpec,
    /// Per-worker spec overrides, keyed by worker name.
    /// Populated by `Mux::factory` from `MuxConfig.resolved_worker_specs`.
    /// Also settable at runtime via `set_worker_spec`.
    worker_specs: HashMap<String, WorkerSpec>,
    /// Supervisor CLI for propagating harness context into worker spawns.
    supervisor_cli: SupervisorCli,
    /// Factory session propagated into dynamically spawned agents.
    factory_session: Option<String>,
}

impl Mux {
    /// Create a new multiplexer
    pub fn new(rows: u16, cols: u16) -> Self {
        let (event_tx, event_rx) = mpsc::channel(256);
        Self {
            panes: IndexMap::new(),
            focused: None,
            event_tx,
            event_rx,
            rows,
            cols,
            default_worker_spec: WorkerSpec::builtin_default(),
            worker_specs: HashMap::new(),
            supervisor_cli: SupervisorCli::Claude,
            factory_session: None,
        }
    }

    /// Set CLI used for worker pane spawns.
    ///
    /// # Deprecated
    /// Prefer [`set_default_worker_spec`][Self::set_default_worker_spec] or
    /// [`set_worker_spec`][Self::set_worker_spec]. This setter is kept only
    /// for backwards-compatibility with callers that pass `worker_cli` as a
    /// scalar flag.
    #[deprecated(
        since = "0.1.0",
        note = "use set_default_worker_spec or set_worker_spec"
    )]
    pub fn set_worker_cli(&mut self, worker_cli: SupervisorCli) {
        self.default_worker_spec.cli = worker_cli;
    }

    /// Set model used for worker pane spawns.
    ///
    /// # Deprecated
    /// Prefer [`set_default_worker_spec`][Self::set_default_worker_spec] or
    /// [`set_worker_spec`][Self::set_worker_spec].
    #[deprecated(
        since = "0.1.0",
        note = "use set_default_worker_spec or set_worker_spec"
    )]
    pub fn set_worker_model(&mut self, model: Option<String>) {
        self.default_worker_spec.model = model;
    }

    /// Set reasoning effort used for worker pane spawns.
    ///
    /// # Deprecated
    /// Prefer [`set_default_worker_spec`][Self::set_default_worker_spec] or
    /// [`set_worker_spec`][Self::set_worker_spec].
    ///
    /// The effort string is parsed to an [`Effort`][crate::spec::Effort] enum;
    /// unrecognised values are silently cleared (effort falls back to the
    /// backend default).
    #[deprecated(
        since = "0.1.0",
        note = "use set_default_worker_spec or set_worker_spec"
    )]
    pub fn set_worker_effort(&mut self, effort: Option<String>) {
        self.default_worker_spec.effort = effort
            .as_deref()
            .and_then(|s| s.parse::<crate::spec::Effort>().ok());
    }

    /// Replace the default spec applied to all worker spawns that have no
    /// per-worker override.
    pub fn set_default_worker_spec(&mut self, spec: WorkerSpec) {
        self.default_worker_spec = spec;
    }

    /// Set (or replace) the per-worker spec for a specific worker name.
    ///
    /// Takes effect for any subsequent `add_worker` / `factory` spawn for that
    /// name. Does not retroactively affect already-running panes.
    pub fn set_worker_spec(&mut self, name: &str, spec: WorkerSpec) {
        self.worker_specs.insert(name.to_string(), spec);
    }

    /// Resolve the effective `WorkerSpec` for a dynamic spawn.
    ///
    /// Priority (first match wins):
    /// 1. Explicitly supplied `spec` argument.
    /// 2. Per-worker entry in `self.worker_specs` keyed by `name`.
    /// 3. `self.default_worker_spec` (the Mux-wide fallback).
    pub fn effective_worker_spec(&self, name: &str, explicit: Option<WorkerSpec>) -> WorkerSpec {
        explicit
            .or_else(|| self.worker_specs.get(name).cloned())
            .unwrap_or_else(|| self.default_worker_spec.clone())
    }

    /// Resolve the worker names for a [`MuxConfig`].
    ///
    /// If `config.worker_names` is non-empty those names are used as-is.
    /// Otherwise names are auto-generated as `worker-1`, `worker-2`, … up to
    /// `config.workers`.  Extracted to eliminate the identical block that
    /// previously appeared in both `factory_pane_configs` and `factory`.
    fn resolve_worker_names(config: &MuxConfig) -> Vec<String> {
        if config.worker_names.is_empty() {
            (0..config.workers)
                .map(|i| format!("worker-{}", i + 1))
                .collect()
        } else {
            config.worker_names.clone()
        }
    }

    /// Resolve the launch fields for a named worker from a [`MuxConfig`].
    ///
    /// Checks `config.resolved_worker_specs` first (cascade resolver output),
    /// then falls back to the singular `worker_cli/worker_model/worker_effort`
    /// fields.  Extracted to eliminate the identical block that previously
    /// appeared in both `factory_pane_configs` and `factory`.
    fn resolve_worker_spec_from_config(
        name: &str,
        config: &MuxConfig,
    ) -> (
        SupervisorCli,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        config
            .resolved_worker_specs
            .iter()
            .find(|s| s.name.as_deref() == Some(name))
            .map(|spec| {
                let effort_str = spec
                    .effort
                    .map(|e| spec.cli.backend().effort_arg(e).to_string());
                let (config_dir, config_dir_source) = match (
                    spec.config_dir.clone(),
                    spec.requester_config_dir.clone(),
                ) {
                    (Some(dir), _) => (Some(dir), Some("explicit".to_string())),
                    (None, Some(dir)) => (Some(dir), Some("supervisor".to_string())),
                    (None, None) => (None, None),
                };
                (
                    spec.cli,
                    spec.model.clone(),
                    effort_str,
                    config_dir,
                    config_dir_source,
                )
            })
            .unwrap_or_else(|| {
                (
                    config.worker_cli,
                    config.worker_model.clone(),
                    config.worker_effort.clone(),
                    None,
                    None,
                )
            })
    }

    /// Build the `PtyConfig`s that `factory()` would spawn for each worker and
    /// supervisor pane, without actually spawning any processes.
    ///
    /// Returns `(agent_name, PtyConfig)` pairs — workers first, then the
    /// supervisor. The director pane (no PTY) is excluded.
    ///
    /// Primary use: integration tests that need to verify effort / model args
    /// flow from `MuxConfig` all the way to the CLI subprocess arguments,
    /// without requiring a real `claude` or `codex` binary.
    pub fn factory_pane_configs(config: &MuxConfig) -> Vec<(String, PtyConfig)> {
        let worker_names = Self::resolve_worker_names(config);

        let mut result = Vec::with_capacity(worker_names.len() + 1);

        for name in &worker_names {
            let worker_cwd = config
                .worker_cwds
                .get(name)
                .cloned()
                .unwrap_or_else(|| config.cwd.clone());
            let teams = config.teams_configs.get(name);

            // Resolve per-worker spec via the shared helper.
            let (cli, model_opt, effort_opt, config_dir, config_dir_source) =
                Self::resolve_worker_spec_from_config(name, config);

            let pty_config = Pane::build_worker_config(
                name,
                worker_cwd,
                config.cas_root.as_ref(),
                &config.supervisor_name,
                config.supervisor_cli,
                cli,
                model_opt.as_deref(),
                effort_opt.as_deref(),
                config_dir.as_deref(),
                config_dir_source.as_deref(),
                teams,
                // The whole initial fleet is known here, so every worker in it
                // is derated against the real total rather than the assumed
                // floor (cas-4614, GH #107).
                Some(worker_names.len()),
            );
            let mut pty_config = pty_config;
            push_factory_session_env(&mut pty_config, cli, config.factory_session.as_deref());
            result.push((name.clone(), pty_config));
        }

        let sup_teams = config.teams_configs.get(&config.supervisor_name);
        let sup_config = Pane::build_supervisor_config(
            &config.supervisor_name,
            config.cwd.clone(),
            config.cas_root.as_ref(),
            config.supervisor_cli,
            config.worker_cli,
            &worker_names,
            config.supervisor_model.as_deref(),
            config.supervisor_effort.as_deref(),
            sup_teams,
        );
        let mut sup_config = sup_config;
        push_factory_session_env(
            &mut sup_config,
            config.supervisor_cli,
            config.factory_session.as_deref(),
        );
        result.push((config.supervisor_name.clone(), sup_config));

        result
    }

    /// Initialize the non-PTY state shared by factory construction and
    /// dynamic-worker configuration previews.
    fn configured_factory(config: &MuxConfig) -> Self {
        let mut mux = Self::new(config.rows, config.cols);
        mux.factory_session = config.factory_session.clone();

        // Build the default spec from singular fields (backward compat with
        // callers that only pass worker_cli/worker_model/worker_effort).
        let default_effort = config
            .worker_effort
            .as_deref()
            .and_then(|s| s.parse::<crate::spec::Effort>().ok());
        mux.set_default_worker_spec(WorkerSpec {
            name: None,
            cli: config.worker_cli,
            model: config.worker_model.clone(),
            effort: default_effort,
            config_dir: None,
            requester_config_dir: None,
        });
        mux.supervisor_cli = config.supervisor_cli;

        // Populate per-worker specs from the cascade resolver output so that
        // subsequent `add_worker` calls (dynamic spawns) inherit the same
        // per-name decision without callers having to re-supply the spec.
        for spec in &config.resolved_worker_specs {
            if let Some(ref name) = spec.name {
                mux.worker_specs.insert(name.clone(), spec.clone());
            }
        }

        mux
    }

    /// Build factory configuration state without spawning panes.
    ///
    /// This is intentionally test-only: production callers must use
    /// [`Self::factory`] so they cannot accidentally create a factory whose
    /// supervisor and initial workers were never launched.
    #[cfg(test)]
    pub(crate) fn factory_state_for_test(config: &MuxConfig) -> Self {
        Self::configured_factory(config)
    }

    /// Create a multiplexer with factory configuration
    pub fn factory(config: MuxConfig) -> Result<Self> {
        let mut mux = Self::configured_factory(&config);

        // Calculate pane sizes based on layout
        // Layout: [Workers] [Supervisor] [Director]
        let num_panes = config.workers + 1 + if config.include_director { 1 } else { 0 };
        let pane_cols = config.cols / num_panes as u16;
        let pane_rows = config.rows;

        // Create worker panes
        let worker_names = Self::resolve_worker_names(&config);

        for name in &worker_names {
            // Use worker-specific CWD if available, otherwise fall back to default
            let worker_cwd = config
                .worker_cwds
                .get(name)
                .cloned()
                .unwrap_or_else(|| config.cwd.clone());
            let teams = config.teams_configs.get(name);

            // Resolve per-worker spec via the shared helper.
            let (cli, model_opt, effort_opt, config_dir, config_dir_source) =
                Self::resolve_worker_spec_from_config(name, &config);

            let pane = Pane::worker(
                name,
                worker_cwd,
                config.cas_root.as_ref(),
                &config.supervisor_name,
                config.supervisor_cli,
                cli,
                model_opt.as_deref(),
                effort_opt.as_deref(),
                config_dir.as_deref(),
                config_dir_source.as_deref(),
                pane_rows,
                pane_cols,
                teams,
                config.factory_session.as_deref(),
                // Same as `factory_pane_configs`: the initial fleet size is
                // known before the first pane exists (cas-4614, GH #107).
                Some(worker_names.len()),
            )?;
            mux.add_pane(pane);
        }

        // Create supervisor pane (always uses main cwd)
        let sup_teams = config.teams_configs.get(&config.supervisor_name);
        let supervisor = Pane::supervisor(
            &config.supervisor_name,
            config.cwd.clone(),
            config.cas_root.as_ref(),
            pane_rows,
            pane_cols,
            config.supervisor_cli,
            config.worker_cli,
            &worker_names,
            config.supervisor_model.as_deref(),
            config.supervisor_effort.as_deref(),
            sup_teams,
            config.factory_session.as_deref(),
        )?;
        mux.add_pane(supervisor);

        // Create director pane (no PTY)
        if config.include_director {
            let director = Pane::director("director", pane_rows, pane_cols)?;
            mux.add_pane(director);
        }

        // Focus the first worker
        if let Some(first) = worker_names.first() {
            mux.focus(first);
        }

        Ok(mux)
    }

    /// Add a pane to the multiplexer
    pub fn add_pane(&mut self, pane: Pane) {
        let id = pane.id().to_string();
        self.panes.insert(id.clone(), pane);

        // If no pane is focused, focus this one
        if self.focused.is_none() {
            self.focused = Some(id.clone());
            if let Some(pane) = self.panes.get_mut(&id) {
                pane.set_focused(true);
            }
        }

        let _ = self.event_tx.try_send(MuxEvent::PaneAdded { pane_id: id });
    }

    /// Remove a pane
    pub fn remove_pane(&mut self, id: &str) -> Option<Pane> {
        let pane = self.panes.shift_remove(id);

        // If we removed the focused pane, focus the next one
        if self.focused.as_deref() == Some(id) {
            self.focused = self.panes.keys().next().cloned();
            if let Some(new_focus) = &self.focused
                && let Some(pane) = self.panes.get_mut(new_focus)
            {
                pane.set_focused(true);
            }
        }

        if pane.is_some() {
            let _ = self.event_tx.try_send(MuxEvent::PaneRemoved {
                pane_id: id.to_string(),
            });
        }

        pane
    }

    /// Get a pane by ID
    pub fn get(&self, id: &str) -> Option<&Pane> {
        self.panes.get(id)
    }

    /// Get a mutable pane by ID
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Pane> {
        self.panes.get_mut(id)
    }

    /// Return the tracked process group for a pane.
    pub fn pane_process_group_id(&self, id: &str) -> Option<u32> {
        self.panes.get(id).and_then(Pane::process_group_id)
    }

    /// Get the focused pane
    pub fn focused(&self) -> Option<&Pane> {
        self.focused.as_ref().and_then(|id| self.panes.get(id))
    }

    /// Get the focused pane mutably
    pub fn focused_mut(&mut self) -> Option<&mut Pane> {
        if let Some(id) = self.focused.clone() {
            self.panes.get_mut(&id)
        } else {
            None
        }
    }

    /// Get the focused pane ID
    pub fn focused_id(&self) -> Option<&str> {
        self.focused.as_deref()
    }

    /// Focus a pane by ID
    pub fn focus(&mut self, id: &str) -> bool {
        if !self.panes.contains_key(id) {
            return false;
        }

        let old_focus = self.focused.take();

        // Unfocus old pane
        if let Some(old_id) = &old_focus
            && let Some(pane) = self.panes.get_mut(old_id)
        {
            pane.set_focused(false);
        }

        // Focus new pane
        self.focused = Some(id.to_string());
        if let Some(pane) = self.panes.get_mut(id) {
            pane.set_focused(true);
        }

        let _ = self.event_tx.try_send(MuxEvent::FocusChanged {
            from: old_focus,
            to: id.to_string(),
        });

        true
    }

    /// Focus the next pane
    pub fn focus_next(&mut self) {
        let len = self.panes.len();
        if len == 0 {
            return;
        }

        let current_idx = self
            .focused
            .as_ref()
            .and_then(|f| self.panes.get_index_of(f))
            .unwrap_or(0);

        let next_idx = (current_idx + 1) % len;
        if let Some((id, _)) = self.panes.get_index(next_idx) {
            let id = id.clone();
            self.focus(&id);
        }
    }

    /// Focus the previous pane
    pub fn focus_prev(&mut self) {
        let len = self.panes.len();
        if len == 0 {
            return;
        }

        let current_idx = self
            .focused
            .as_ref()
            .and_then(|f| self.panes.get_index_of(f))
            .unwrap_or(0);

        let prev_idx = if current_idx == 0 {
            len - 1
        } else {
            current_idx - 1
        };
        if let Some((id, _)) = self.panes.get_index(prev_idx) {
            let id = id.clone();
            self.focus(&id);
        }
    }

    /// Get all pane IDs
    pub fn pane_ids(&self) -> Vec<&str> {
        self.panes.keys().map(|s| s.as_str()).collect()
    }

    /// Get all panes
    pub fn panes(&self) -> impl Iterator<Item = &Pane> {
        self.panes.values()
    }

    /// Get all panes mutably
    pub fn panes_mut(&mut self) -> impl Iterator<Item = &mut Pane> {
        self.panes.values_mut()
    }

    /// Get panes of a specific kind
    pub fn panes_by_kind(&self, kind: PaneKind) -> impl Iterator<Item = &Pane> {
        self.panes.values().filter(move |p| *p.kind() == kind)
    }

    /// Get worker panes
    pub fn workers(&self) -> impl Iterator<Item = &Pane> {
        self.panes_by_kind(PaneKind::Worker)
    }

    /// Get the count of all panes
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    /// Get the count of worker panes
    pub fn worker_count(&self) -> usize {
        self.panes
            .values()
            .filter(|p| *p.kind() == PaneKind::Worker)
            .count()
    }

    /// Build the `PtyConfig` that `add_worker` would use for the given worker,
    /// without actually spawning a process.
    ///
    /// Spec resolution order:
    /// 1. Explicitly supplied `spec`.
    /// 2. Per-worker entry in `self.worker_specs` (set by `set_worker_spec` or
    ///    populated from `MuxConfig.resolved_worker_specs` in `Mux::factory`).
    /// 3. `self.default_worker_spec` (Mux-wide fallback).
    ///
    /// Primary use: tests that need to verify CLI / model / effort routing
    /// without spawning a real process.
    pub fn build_add_worker_config(
        &self,
        name: &str,
        cwd: PathBuf,
        cas_root: Option<&PathBuf>,
        supervisor_name: &str,
        teams: Option<&TeamsSpawnConfig>,
        spec: Option<WorkerSpec>,
    ) -> PtyConfig {
        let effective = self.effective_worker_spec(name, spec);
        let effort_str = effective
            .effort
            .map(|e| effective.cli.backend().effort_arg(e).to_string());
        let (config_dir, config_dir_source) = match (
            effective.config_dir.as_deref(),
            effective.requester_config_dir.as_deref(),
        ) {
            (Some(dir), _) => (Some(dir), Some("explicit")),
            (None, Some(dir)) => (Some(dir), Some("supervisor")),
            (None, None) => (None, None),
        };
        let mut config = Pane::build_worker_config(
            name,
            cwd,
            cas_root,
            supervisor_name,
            self.supervisor_cli,
            effective.cli,
            effective.model.as_deref(),
            effort_str.as_deref(),
            config_dir,
            config_dir_source,
            teams,
            // Mirrors `add_worker`: this helper previews the config that
            // spawn would produce, so it must derate identically or the
            // preview would lie about the env (cas-4614, GH #107).
            //
            // Unlike `add_worker` this can be called for a worker that
            // already has a pane, so only count the +1 when the spawn would
            // actually add one — otherwise the preview over-counts the fleet
            // and under-reports jobs against what that worker really got.
            Some(if self.panes.contains_key(name) {
                self.worker_count()
            } else {
                self.worker_count() + 1
            }),
        );
        push_factory_session_env(&mut config, effective.cli, self.factory_session.as_deref());
        config
    }

    /// Add a worker pane at runtime
    ///
    /// This is the primary method for dynamic worker spawning.
    ///
    /// # Arguments
    /// * `name` - Worker name (also used as pane ID)
    /// * `cwd` - Working directory for the worker (typically a clone directory)
    /// * `cas_root` - Optional path to .cas directory for CAS_ROOT env var
    /// * `supervisor_name` - Name of the supervisor (enables `target: supervisor` in message action)
    /// * `teams` - Optional native Agent Teams spawn config
    /// * `spec` - Optional per-call `WorkerSpec` override. When `None`, the
    ///   per-worker spec stored via `set_worker_spec` (or the Mux-wide default)
    ///   is used. Callers that do not yet supply per-worker specs should pass
    ///   `None` to preserve the existing behaviour.
    ///
    /// # Returns
    /// The pane ID on success
    pub fn add_worker(
        &mut self,
        name: &str,
        cwd: PathBuf,
        cas_root: Option<&PathBuf>,
        supervisor_name: &str,
        teams: Option<&TeamsSpawnConfig>,
        spec: Option<WorkerSpec>,
    ) -> Result<PaneId> {
        // Check if pane with this name already exists
        if self.panes.contains_key(name) {
            return Err(Error::pty(format!("Pane '{name}' already exists")));
        }

        let effective = self.effective_worker_spec(name, spec);
        let effort_str = effective
            .effort
            .map(|e| effective.cli.backend().effort_arg(e).to_string());
        let (config_dir, config_dir_source) = match (
            effective.config_dir.as_deref(),
            effective.requester_config_dir.as_deref(),
        ) {
            (Some(dir), _) => (Some(dir), Some("explicit")),
            (None, Some(dir)) => (Some(dir), Some("supervisor")),
            (None, None) => (None, None),
        };
        // cas-4614 (GH #107): derate this worker's CARGO_BUILD_JOBS against
        // the fleet it is joining. `worker_count()` is the live pane count
        // *before* this insertion, so +1 counts the worker being spawned —
        // otherwise the first worker of a fleet would size itself as if it
        // were alone. The Mux is the right source: it owns the panes, so the
        // number is exact at the moment the env is built, with nothing to
        // thread through the caller.
        let fleet_size = self.worker_count() + 1;
        let pane = Pane::worker(
            name,
            cwd,
            cas_root,
            supervisor_name,
            self.supervisor_cli,
            effective.cli,
            effective.model.as_deref(),
            effort_str.as_deref(),
            config_dir,
            config_dir_source,
            self.rows,
            self.cols,
            teams,
            self.factory_session.as_deref(),
            Some(fleet_size),
        )?;
        let id = pane.id().to_string();
        // Persist the resolved spec so `effective_worker_spec(name, None)` is
        // authoritative for this worker's lifetime. Without this, a worker spawned
        // with an explicit per-spawn `spec` override (e.g. a Codex worker added to a
        // Claude-default factory via SpawnWorkers) would not be in `worker_specs`,
        // and a later harness lookup would silently fall back to the Mux-wide default
        // — mis-routing message delivery for that worker (cas-b68a).
        self.worker_specs.insert(name.to_string(), effective);
        self.add_pane(pane);
        Ok(id)
    }

    /// Add a new shell pane to the mux.
    pub fn add_shell(
        &mut self,
        name: &str,
        cwd: PathBuf,
        shell_command: Option<&str>,
    ) -> Result<PaneId> {
        if self.panes.contains_key(name) {
            return Err(Error::pty(format!("Pane '{name}' already exists")));
        }
        let pane = Pane::shell(name, cwd, shell_command, self.rows, self.cols)?;
        let id = pane.id().to_string();
        self.add_pane(pane);
        Ok(id)
    }

    /// Remove a shell pane by name.
    pub fn remove_shell(&mut self, name: &str) -> Result<()> {
        if let Some(pane) = self.panes.get(name) {
            if *pane.kind() != PaneKind::Shell {
                return Err(Error::pty(format!(
                    "Pane '{}' is not a shell (is {:?})",
                    name,
                    pane.kind()
                )));
            }
        } else {
            return Err(Error::pane_not_found(name));
        }
        self.remove_pane(name);
        Ok(())
    }

    /// Remove a worker pane by name and cleanup its PTY
    ///
    /// This is the primary method for dynamic worker shutdown.
    /// The pane's PTY will be dropped, sending SIGHUP to the process.
    pub fn remove_worker(&mut self, name: &str) -> Result<()> {
        // Verify it's a worker
        if let Some(pane) = self.panes.get(name) {
            if *pane.kind() != PaneKind::Worker {
                return Err(Error::pty(format!(
                    "Pane '{}' is not a worker (is {:?})",
                    name,
                    pane.kind()
                )));
            }
        } else {
            return Err(Error::pane_not_found(name));
        }

        // Remove the pane (this drops the PTY, sending SIGHUP)
        self.remove_pane(name);
        Ok(())
    }

    /// Kill a worker's PTY process tree, then remove it from the registry (cas-8c5a).
    ///
    /// Unlike `remove_worker` (which drops the PTY and relies on SIGHUP), this
    /// method explicitly signals the **entire process group** before releasing
    /// the registry entry.  This is required for multi-process workers such as
    /// Codex, where a node → codex child tree survives a bare SIGHUP.
    ///
    /// * `force = true`  → SIGKILL to the process group (cannot be ignored)
    /// * `force = false` → SIGTERM with SIGKILL escalation for the process group
    ///
    /// After the signal, `remove_pane` still drops the PTY master, which sends
    /// a redundant SIGHUP as belt-and-suspenders.
    pub async fn kill_worker(&mut self, name: &str, force: bool) -> Result<()> {
        // Verify it's a worker pane
        if let Some(pane) = self.panes.get(name) {
            if *pane.kind() != PaneKind::Worker {
                return Err(Error::pty(format!(
                    "Pane '{}' is not a worker (is {:?})",
                    name,
                    pane.kind()
                )));
            }
        } else {
            return Err(Error::pane_not_found(name));
        }

        // Kill the process group before removing the pane
        if let Some(pane) = self.panes.get_mut(name) {
            pane.kill_tree(force).await;
        }

        // Remove the pane (drops the PTY master, sends residual SIGHUP)
        self.remove_pane(name);
        Ok(())
    }

    /// Get the supervisor pane
    pub fn supervisor(&self) -> Option<&Pane> {
        self.panes_by_kind(PaneKind::Supervisor).next()
    }

    /// Get the director pane
    pub fn director(&self) -> Option<&Pane> {
        self.panes_by_kind(PaneKind::Director).next()
    }

    /// Check whether a pane is ready to accept prompt injection.
    pub fn pane_ready_for_injection(&self, pane_id: &str) -> bool {
        self.panes
            .get(pane_id)
            .map(|p| p.ready_for_injection())
            .unwrap_or(false)
    }

    /// Inject a prompt into a specific pane
    pub async fn inject(&self, pane_id: &str, prompt: &str) -> Result<InjectOutcome> {
        let pane = self
            .panes
            .get(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;
        if pane.is_composer_dirty() {
            tracing::info!(
                target: "cas::coordination",
                stage = "composer_inject_deferred",
                target_agent = %pane_id,
                "PTY inject deferred to its durable owner while attached operator composer is dirty"
            );
            return Ok(InjectOutcome::DeferredComposerDirty);
        }
        if pane.inject_submit_pending() {
            tracing::info!(
                target: "cas::coordination",
                stage = "composer_inject_deferred",
                target_agent = %pane_id,
                "PTY inject deferred until the prior injected payload reaches its submit boundary"
            );
            return Ok(InjectOutcome::DeferredComposerDirty);
        }
        pane.inject_prompt(prompt).await?;
        Ok(InjectOutcome::Delivered)
    }

    /// Compatibility no-op retained for callers from cas-1a4d.
    ///
    /// Deferred payloads now stay with their durable queue owner; the mux
    /// intentionally has nothing to flush or carry across pane generations.
    pub async fn flush_deferred_injections(&self) {}

    /// Total bytes of PTY output observed for a pane by name.
    ///
    /// Returns `None` if the pane does not exist. Used by the urgent
    /// interrupt-and-redirect path to detect output quiescence after a
    /// turn-break before injecting the correction.
    pub fn pane_bytes_received(&self, pane_id: &str) -> Option<u64> {
        self.panes.get(pane_id).map(|p| p.bytes_received())
    }

    /// Whether an attached operator currently has an unsubmitted draft in this
    /// pane's composer (cas-f02b / cas-eacc).
    ///
    /// `Mux::inject` defers for as long as this remains true. There is no
    /// elapsed-time escape hatch: time does not make writing into a human draft
    /// safe. An unknown pane reports `false`; the caller's own liveness checks
    /// decide whether to proceed.
    pub fn pane_composer_dirty(&self, pane_id: &str) -> bool {
        self.panes
            .get(pane_id)
            .is_some_and(|p| p.is_composer_dirty())
    }

    /// Break a specific worker's current turn by name by sending a single
    /// Esc (0x1b) — the canonical Claude Code "stop this turn" key.
    ///
    /// Distinct from [`Mux::interrupt`], which sends Ctrl+C (0x03). Targets a
    /// pane by name independent of TUI focus.
    pub async fn break_turn(&self, pane_id: &str) -> Result<()> {
        let pane = self
            .panes
            .get(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;
        pane.break_turn().await
    }

    /// Urgent interrupt-and-redirect: break the target worker's in-flight turn
    /// (Esc), wait for the turn to actually break, then inject the correction
    /// as the worker's next prompt — by name, independent of TUI focus.
    ///
    /// `floor` is the minimum delay between the turn-break and the inject,
    /// sized per-harness by the caller. For harnesses with a real textbox
    /// submit (Claude, Grok) this floor alone is honored unconditionally, same
    /// as before cas-4208 — the "Claude is unaffected" control group in that
    /// task's report (7 live interrupts, 100% recovery) showed a flat sleep is
    /// already sufficient there, so that path stays a zero-risk no-op.
    ///
    /// For Codex (`!supports_textbox_submit`) `floor` is only the minimum: see
    /// [`Mux::wait_for_injection_readiness`] for why a flat sleep alone lost a
    /// real race for that harness and what replaces it.
    ///
    /// Takes `&mut self` (not `&self`, as before cas-4208): the quiescence
    /// poll needs to actively drain the pane via [`Mux::poll_batch`] — see
    /// that method's doc for why passively reading
    /// [`Mux::pane_bytes_received`] is not enough on its own.
    pub async fn interrupt_and_inject(
        &mut self,
        pane_id: &str,
        prompt: &str,
        floor: std::time::Duration,
    ) -> Result<()> {
        // Shared targeted interrupt primitive: Commander pane interrupts and
        // coordination urgent delivery must never drift into parallel cancel
        // implementations.
        self.break_turn(pane_id).await?;
        self.wait_for_injection_readiness(pane_id, floor).await;
        let pane = self
            .panes
            .get(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;
        pane.inject_prompt(prompt).await
    }

    /// Urgent queue delivery that preserves an attached operator's draft.
    ///
    /// Urgent still means "interrupt the agent's current turn", but only after
    /// the human has submitted or cleared their composer. A dirty composer
    /// returns [`InjectOutcome::DeferredComposerDirty`] without sending Esc or
    /// payload bytes, allowing the durable queue owner to retry the same urgent
    /// row at the next safe boundary without losing its priority or attribution.
    ///
    /// Deliberately separate from [`Self::interrupt_and_inject`]: explicit
    /// destructive controls such as context reset still need the raw primitive.
    pub async fn interrupt_and_inject_preserving_composer(
        &mut self,
        pane_id: &str,
        prompt: &str,
        floor: std::time::Duration,
    ) -> Result<InjectOutcome> {
        let pane = self
            .panes
            .get(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;
        if pane.is_composer_dirty() || pane.inject_submit_pending() {
            tracing::info!(
                target: "cas::coordination",
                stage = "urgent_composer_inject_deferred",
                target_agent = %pane_id,
                "urgent queue delivery retained until the composer reaches a safe boundary"
            );
            return Ok(InjectOutcome::DeferredComposerDirty);
        }
        self.interrupt_and_inject(pane_id, prompt, floor).await?;
        Ok(InjectOutcome::Delivered)
    }

    /// Bounded post-break-turn settle used by [`Mux::interrupt_and_inject`]
    /// before typing the redirect (cas-4208).
    ///
    /// BACKGROUND: `urgent_settle_duration` (cas-cli's
    /// `queue_and_events.rs`) used to be the *entire* wait — a flat sleep
    /// with no feedback from the child at all. A live repro against a real
    /// `codex` binary (task cas-4208 notes) proved that loses a real race:
    /// after Esc, Codex's TUI renders a transitional "Conversation
    /// interrupted" banner before its composer is actually ready for fresh
    /// input again. Typing (and, worse, the trailing CR that submits it)
    /// while that transition is still in flight gets silently absorbed —
    /// the correction is left sitting in the composer as an unsent draft
    /// forever. Every later message (urgent or normal) just types MORE
    /// text into that same stuck draft rather than delivering a fresh
    /// prompt, which is exactly the reported "turn_aborted then permanent
    /// silence, deaf to all later messages" signature: repro'd live with
    /// settle=0 and confirmed to compound across a second injected
    /// message; recovered live at floor=1200ms/settle=500ms — see task
    /// notes for the full rollout excerpts of both.
    ///
    /// FIX: for harnesses without a real textbox submit (Codex), actively
    /// poll [`Mux::pane_bytes_received`] for genuine output quiescence
    /// instead of trusting a guessed constant — this is the mechanism the
    /// old doc comment on `interrupt_and_inject` already claimed existed
    /// ("the caller may have already confirmed output quiescence") but no
    /// caller actually implemented. `floor` is still honored as an
    /// absolute minimum (so well-behaved fast panes see no latency
    /// change), and `MAX_EXTRA_WAIT` bounds the total additional wait so a
    /// pane that never truly goes quiet (e.g. a long-running background
    /// command still echoing output) still gets the correction delivered
    /// rather than stalling delivery forever — same bounded-fallback
    /// guarantee the flat sleep gave, just no longer blind.
    ///
    /// CRITICAL: this loop must actively call [`Mux::poll_batch`] itself on
    /// every tick, not just read [`Mux::pane_bytes_received`] and sleep.
    /// The daemon's main loop only drains PTYs once per iteration, strictly
    /// *before* it awaits `process_prompt_queue` (which is what eventually
    /// calls this); nothing else runs concurrently to advance the byte
    /// counters while this same task is parked in `tokio::time::sleep`. An
    /// earlier version of this fix polled `pane_bytes_received` without
    /// draining and saw a permanently frozen count — indistinguishable
    /// from real quiescence on the very first check — which silently
    /// collapsed back to "inject right after the floor," i.e. no fix at
    /// all. Draining only the target pane here (not a full
    /// `poll_batch`-driven event dispatch) keeps the blast radius of this
    /// extra work scoped to the one pane being interrupted.
    async fn wait_for_injection_readiness(&mut self, pane_id: &str, floor: std::time::Duration) {
        let Some(pane) = self.panes.get(pane_id) else {
            return;
        };
        if pane
            .harness()
            .backend()
            .capabilities()
            .supports_textbox_submit
        {
            tokio::time::sleep(floor).await;
            return;
        }

        const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(75);
        const STABILITY_WINDOW: std::time::Duration = std::time::Duration::from_millis(300);
        const MAX_EXTRA_WAIT: std::time::Duration = std::time::Duration::from_secs(4);

        let start = tokio::time::Instant::now();
        let max_wait = floor + MAX_EXTRA_WAIT;
        let mut last_bytes = self.pane_bytes_received(pane_id).unwrap_or(0);
        let mut last_change_at = start;

        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            // Actively drain this pane so the counter we're about to read
            // reflects reality — see the doc comment above.
            if let Some(pane) = self.panes.get_mut(pane_id) {
                let _ = pane.drain_output();
            } else {
                break; // pane gone mid-wait
            }
            let Some(bytes) = self.pane_bytes_received(pane_id) else {
                break;
            };
            if bytes != last_bytes {
                last_bytes = bytes;
                last_change_at = tokio::time::Instant::now();
            }
            let elapsed = start.elapsed();
            let quiescent_long_enough = last_change_at.elapsed() >= STABILITY_WINDOW;
            if elapsed >= max_wait || (elapsed >= floor && quiescent_long_enough) {
                if elapsed >= max_wait {
                    tracing::debug!(
                        target: "cas::coordination",
                        stage = "urgent_settle_timeout",
                        target_agent = %pane_id,
                        elapsed_ms = elapsed.as_millis() as u64,
                        "post-break settle hit max wait without confirmed quiescence; injecting anyway"
                    );
                }
                break;
            }
        }
    }

    /// Inject a prompt into the focused pane
    pub async fn inject_focused(&self, prompt: &str) -> Result<()> {
        let pane = self
            .focused()
            .ok_or_else(|| Error::pty("No focused pane"))?;
        pane.inject_prompt(prompt).await
    }

    /// Inject a prompt into all workers
    pub async fn inject_all_workers(&self, prompt: &str) -> Result<()> {
        for pane in self.workers() {
            pane.inject_prompt(prompt).await?;
        }
        Ok(())
    }

    /// Inject a prompt into the supervisor
    pub async fn inject_supervisor(&self, prompt: &str) -> Result<()> {
        let pane = self
            .supervisor()
            .ok_or_else(|| Error::pane_not_found("supervisor"))?;
        pane.inject_prompt(prompt).await
    }

    /// Raw write to the focused pane (no turn-in-flight side effects).
    ///
    /// Prefer [`Mux::deliver_user_input`] for keyboard/paste so submit
    /// semantics stay unified across surfaces (cas-7f6f). Keep this for
    /// non-submit control bytes (SGR, scroll, raw Esc).
    pub async fn send_input(&self, data: &[u8]) -> Result<()> {
        let pane = self
            .focused()
            .ok_or_else(|| Error::pty("No focused pane"))?;
        pane.write(data).await?;
        pane.observe_raw_client_input(data);
        Ok(())
    }

    /// Raw write to a specific pane (no turn-in-flight side effects).
    pub async fn send_input_to(&self, pane_id: &str, data: &[u8]) -> Result<()> {
        let pane = self
            .panes
            .get(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;
        pane.write(data).await?;
        pane.observe_raw_client_input(data);
        Ok(())
    }

    /// Deliver user input to the focused pane with explicit submit kind.
    ///
    /// Unified API for terminal, GUI, WebSocket, and relay (cas-7f6f):
    /// - [`UserInputKind::KeyStream`]: Enter marks turn-in-flight
    /// - [`UserInputKind::StructuredPaste`]: never marks (paste/drop)
    pub async fn deliver_user_input(
        &self,
        data: &[u8],
        kind: crate::pane::UserInputKind,
    ) -> Result<()> {
        let pane = self
            .focused()
            .ok_or_else(|| Error::pty("No focused pane"))?;
        pane.deliver_user_input(data, kind).await
    }

    /// Deliver user input to a specific pane with explicit submit kind.
    pub async fn deliver_user_input_to(
        &self,
        pane_id: &str,
        data: &[u8],
        kind: crate::pane::UserInputKind,
    ) -> Result<()> {
        let pane = self
            .panes
            .get(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;
        pane.deliver_user_input(data, kind).await
    }

    /// Refresh harness turn state on the focused pane (Grok `turn_ended`).
    pub fn refresh_focused_harness_turn_state(&self) {
        if let Some(pane) = self.focused() {
            pane.refresh_harness_turn_state();
        }
    }

    /// Refresh harness turn state on a pane.
    pub fn refresh_harness_turn_state(&self, pane_id: &str) {
        if let Some(pane) = self.panes.get(pane_id) {
            pane.refresh_harness_turn_state();
        }
    }

    /// Record normal turn completion supplied by an authoritative caller.
    ///
    /// Cancel clears via [`Pane::break_turn`] / [`Pane::interrupt`]. Grok
    /// supplies normal completion through `turn_ended` in
    /// [`Pane::refresh_harness_turn_state`]; Claude/Codex currently supply no
    /// equivalent signal, so callers must not infer completion from PTY quiet.
    pub fn note_turn_completed(&self, pane_id: &str) {
        if let Some(pane) = self.panes.get(pane_id) {
            pane.mark_turn_completed();
        }
    }

    /// Record caller-known normal completion on the focused pane.
    ///
    /// See [`Mux::note_turn_completed`] for harness limitations.
    pub fn note_turn_completed_focused(&self) {
        if let Some(pane) = self.focused() {
            pane.mark_turn_completed();
        }
    }

    /// Poll all panes for events (non-blocking)
    ///
    /// Returns one event at a time. Call in a loop until None to process all events.
    pub fn poll(&mut self) -> Option<MuxEvent> {
        // First check the event queue
        if let Ok(event) = self.event_rx.try_recv() {
            return Some(event);
        }

        // Poll each pane for ONE event (not draining all)
        // This ensures we return events as they come without losing any
        for (id, pane) in self.panes.iter_mut() {
            if let Some(event) = pane.poll() {
                match event {
                    PtyEvent::Output(data) => {
                        return Some(MuxEvent::PaneOutput {
                            pane_id: id.clone(),
                            data,
                        });
                    }
                    PtyEvent::Exited(code) => {
                        return Some(MuxEvent::PaneExited {
                            pane_id: id.clone(),
                            exit_code: code,
                        });
                    }
                    PtyEvent::Error(e) => {
                        tracing::error!("PTY error in pane {}: {}", id, e);
                        // Continue to next pane/event
                    }
                }
            }
        }

        None
    }

    /// Poll all panes and drain all available events at once (more efficient for multi-pane)
    ///
    /// Returns (total_bytes, events). Uses coalesced output feeding for efficiency when
    /// multiple Claude instances are generating long responses simultaneously.
    ///
    /// The MuxEvent::PaneOutput events include the raw PTY bytes so WebSocket clients
    /// can feed them to their own terminal emulators.
    pub fn poll_batch(&mut self) -> (usize, Vec<MuxEvent>) {
        let mut events = Vec::new();
        let mut total_bytes = 0;

        // First drain the event queue
        while let Ok(event) = self.event_rx.try_recv() {
            events.push(event);
        }

        // Drain each pane using coalesced output (more efficient for high throughput)
        for (id, pane) in self.panes.iter_mut() {
            let (data, other_events) = pane.drain_output();

            if !data.is_empty() {
                total_bytes += data.len();
                // Include raw data for WebSocket clients
                events.push(MuxEvent::PaneOutput {
                    pane_id: id.clone(),
                    data,
                });
            }

            // Forward non-output events
            for event in other_events {
                match event {
                    PtyEvent::Exited(code) => {
                        events.push(MuxEvent::PaneExited {
                            pane_id: id.clone(),
                            exit_code: code,
                        });
                    }
                    PtyEvent::Error(e) => {
                        tracing::error!("PTY error in pane {}: {}", id, e);
                    }
                    _ => {}
                }
            }
        }

        (total_bytes, events)
    }

    /// Get incremental terminal updates for all panes with changes.
    ///
    /// Returns a list of (pane_id, ServerMessage::PaneRowsUpdate) for each pane
    /// that has dirty rows since the last call. This is the tmux-style approach
    /// where the server renders terminals and sends pre-rendered cells to clients.
    ///
    /// Call this after `poll_batch()` to get rendered updates instead of raw bytes.
    pub fn get_incremental_updates(&mut self) -> Vec<ServerMessage> {
        let mut updates = Vec::new();

        for (id, pane) in self.panes.iter_mut() {
            match pane.get_incremental_update() {
                Ok(Some((rows, cursor, seq))) => {
                    updates.push(ServerMessage::PaneRowsUpdate {
                        pane_id: id.clone(),
                        rows,
                        cursor,
                        seq,
                    });
                }
                Ok(None) => {
                    // No updates for this pane
                }
                Err(e) => {
                    tracing::warn!("Pane {}: get_incremental_update failed: {}", id, e);
                }
            }
        }

        updates
    }

    /// Get full terminal snapshot for a specific pane.
    ///
    /// Used for initial sync when a client connects or when scrollback is requested.
    pub fn get_pane_snapshot(
        &self,
        pane_id: &str,
    ) -> Option<(cas_factory_protocol::TerminalSnapshot, u32, u32)> {
        self.panes.get(pane_id).and_then(|pane| {
            pane.get_full_snapshot()
                .ok()
                .map(|snapshot| (snapshot, pane.scroll_offset(), pane.scrollback_lines()))
        })
    }

    /// Receive the next event (blocking)
    pub async fn recv(&mut self) -> Option<MuxEvent> {
        self.event_rx.recv().await
    }

    /// Resize all panes
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.rows = rows;
        self.cols = cols;

        // Recalculate pane sizes
        let num_panes = self.panes.len();
        if num_panes == 0 {
            return Ok(());
        }

        let pane_cols = cols / num_panes as u16;
        let pane_rows = rows;

        for pane in self.panes.values_mut() {
            pane.resize(pane_rows, pane_cols)?;
        }

        Ok(())
    }

    /// Get the terminal size
    pub fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// Interrupt the focused pane
    pub async fn interrupt_focused(&self) -> Result<()> {
        let pane = self
            .focused()
            .ok_or_else(|| Error::pty("No focused pane"))?;
        pane.interrupt().await
    }

    /// Interrupt a specific pane by ID
    pub async fn interrupt(&self, pane_id: &str) -> Result<()> {
        let pane = self
            .panes
            .get(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;
        pane.interrupt().await
    }

    /// Whether the focused pane's inner process is in alt-screen mode.
    ///
    /// Alt-screen means the ghostty_vt scrollback is empty — `scroll_focused`
    /// will be a no-op. Use this to decide whether to forward wheel events to
    /// the PTY instead.
    pub fn focused_is_in_alt_screen(&self) -> bool {
        self.focused()
            .map(|p| p.is_in_alt_screen())
            .unwrap_or(false)
    }

    /// Scroll the focused pane by delta lines
    ///
    /// Positive delta scrolls down (towards newer content), negative scrolls up (towards older content).
    pub fn scroll_focused(&mut self, delta: i32) -> Result<()> {
        let pane = self
            .focused_mut()
            .ok_or_else(|| Error::pty("No focused pane"))?;
        pane.scroll(delta)
    }

    /// Scroll the focused pane to top of scrollback
    pub fn scroll_focused_to_top(&mut self) -> Result<()> {
        let pane = self
            .focused_mut()
            .ok_or_else(|| Error::pty("No focused pane"))?;
        pane.scroll_to_top()
    }

    /// Scroll the focused pane to bottom (most recent content)
    pub fn scroll_focused_to_bottom(&mut self) -> Result<()> {
        let pane = self
            .focused_mut()
            .ok_or_else(|| Error::pty("No focused pane"))?;
        pane.scroll_to_bottom()
    }

    /// Scroll a specific pane by delta lines
    pub fn scroll_pane(&mut self, pane_id: &str, delta: i32) -> Result<()> {
        let pane = self
            .get_mut(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;
        pane.scroll(delta)
    }

    /// Scroll a specific pane and return snapshot with cache rows for smooth scrolling.
    ///
    /// This is the main entry point for handling Scroll messages with cache_window.
    /// Returns (snapshot, cache_rows, cache_start_row, scroll_offset, scrollback_lines).
    #[allow(clippy::type_complexity)]
    pub fn scroll_pane_with_cache(
        &mut self,
        pane_id: &str,
        delta: i32,
        cache_window: u32,
    ) -> Result<(
        cas_factory_protocol::TerminalSnapshot,
        Vec<cas_factory_protocol::CacheRow>,
        Option<u32>,
        u32,
        u32,
    )> {
        let pane = self
            .get_mut(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;

        // Apply scroll
        pane.scroll(delta)?;

        // Get snapshot with cache rows
        let (snapshot, cache_rows, cache_start) = pane.create_snapshot_with_cache(cache_window)?;
        let scroll_offset = pane.scroll_offset();
        let scrollback_lines = pane.scrollback_lines();

        Ok((
            snapshot,
            cache_rows,
            cache_start,
            scroll_offset,
            scrollback_lines,
        ))
    }

    /// Scroll a specific pane and return RowData snapshot with cache rows.
    ///
    /// Returns (snapshot_rows, cache_rows, cache_start_row, scroll_offset, scrollback_lines).
    #[allow(clippy::type_complexity)]
    pub fn scroll_pane_with_cache_rows(
        &mut self,
        pane_id: &str,
        delta: i32,
        cache_window: u32,
    ) -> Result<(
        Vec<cas_factory_protocol::RowData>,
        Vec<cas_factory_protocol::CacheRow>,
        Option<u32>,
        u32,
        u32,
    )> {
        let pane = self
            .get_mut(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;

        // Apply scroll
        pane.scroll(delta)?;

        // Get snapshot rows with cache
        let (snapshot_rows, cache_rows, cache_start) =
            pane.create_snapshot_rows_with_cache(cache_window)?;
        let scroll_offset = pane.scroll_offset();
        let scrollback_lines = pane.scrollback_lines();

        Ok((
            snapshot_rows,
            cache_rows,
            cache_start,
            scroll_offset,
            scrollback_lines,
        ))
    }

    /// Get the current scroll offset for a pane (lines from bottom).
    pub fn scroll_offset(&self, pane_id: &str) -> Option<u32> {
        self.get(pane_id).map(|p| p.scroll_offset())
    }

    /// Scroll a specific pane to bottom
    pub fn scroll_pane_to_bottom(&mut self, pane_id: &str) -> Result<()> {
        let pane = self
            .get_mut(pane_id)
            .ok_or_else(|| Error::pane_not_found(pane_id))?;
        pane.scroll_to_bottom()
    }

    /// Kill all panes (terminate all PTY processes)
    ///
    /// This should be called during shutdown to ensure all descendant process
    /// groups, not only the direct interactive children, are terminated.
    pub fn kill_all(&mut self) {
        for pane in self.panes.values_mut() {
            pane.kill_tree_force();
        }
    }
}

#[cfg(test)]
#[path = "mux_tests/tests.rs"]
mod tests;
