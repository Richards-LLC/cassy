use crate::ui::factory::daemon::imports::*;

use crate::ui::factory::protocol::{self, DaemonMessage};
use std::path::PathBuf;

/// Result of fork_first_daemon
pub enum ForkFirstResult {
    /// Parent process - should run boot screen client
    Parent {
        session_name: String,
        sock_path: PathBuf,
        daemon_pid: u32,
    },
    /// Child process - should run initialization then daemon loop
    Child { init_phase: Box<DaemonInitPhase> },
}

/// Initialization phase for fork-first daemon
///
/// After fork, the child process uses this to:
/// 1. Accept the parent as the first client (for progress updates)
/// 2. Do all initialization (worktrees, CAS data, PTY spawning)
/// 3. Send progress messages to parent
/// 4. Convert to FactoryDaemon when ready
pub struct DaemonInitPhase {
    /// Session name
    pub session_name: String,
    /// Factory configuration
    pub factory_config: FactoryConfig,
    /// Boot configuration for names
    pub supervisor_name: String,
    /// Worker names
    pub worker_names: Vec<String>,
    /// Whether to enable cloud phone-home
    pub phone_home: bool,
    /// Socket listener
    listener: UnixListener,
    /// First client connection (parent's boot screen)
    init_client: Option<UnixStream>,
}

/// Create a daemon init phase without calling fork().
///
/// Used by subprocess-based daemon startup paths that still want to stream
/// boot progress over the standard factory socket protocol.
#[cfg(unix)]
pub fn init_phase_without_fork(
    session_name: String,
    factory_config: FactoryConfig,
    supervisor_name: String,
    worker_names: Vec<String>,
    phone_home: bool,
) -> anyhow::Result<(DaemonInitPhase, PathBuf)> {
    let sock_path = socket_path(&session_name);

    if sock_path.exists() {
        std::fs::remove_file(&sock_path)?;
    }

    if let Some(parent) = sock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(&sock_path)?;
    // Keep blocking accept semantics for boot client handshake.
    listener.set_nonblocking(false)?;

    Ok((
        DaemonInitPhase {
            session_name,
            factory_config,
            supervisor_name,
            worker_names,
            phone_home,
            listener,
            init_client: None,
        },
        sock_path,
    ))
}

/// Fork immediately, child becomes daemon and does initialization
///
/// This is the correct approach (like tmux): fork first, then initialize.
/// PTY reader threads are created IN the daemon process, never crossing fork.
///
/// Returns:
/// - `ForkFirstResult::Parent` - Parent should run boot screen client
/// - `ForkFirstResult::Child` - Child should initialize and run daemon
#[cfg(unix)]
pub fn fork_first_daemon(
    session_name: String,
    factory_config: FactoryConfig,
    supervisor_name: String,
    worker_names: Vec<String>,
    phone_home: bool,
) -> anyhow::Result<ForkFirstResult> {
    use nix::unistd::{ForkResult as NixForkResult, fork};
    use std::os::unix::io::AsRawFd;

    // Create socket path (before fork so both know it)
    let sock_path = socket_path(&session_name);

    // Remove stale socket
    if sock_path.exists() {
        std::fs::remove_file(&sock_path)?;
    }

    // Ensure parent directory exists
    if let Some(parent) = sock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Fork!
    match unsafe { fork() } {
        Ok(NixForkResult::Parent { child }) => {
            // Parent returns immediately
            tracing::info!("Forked daemon child with PID {}", child);
            Ok(ForkFirstResult::Parent {
                session_name,
                sock_path,
                daemon_pid: child.as_raw() as u32,
            })
        }
        Ok(NixForkResult::Child) => {
            // Child: daemonize first
            let _ = nix::unistd::setsid();

            // Redirect stdio to log files
            let devnull = std::fs::File::open("/dev/null")?;
            let devnull_fd = devnull.as_raw_fd();
            unsafe {
                libc::dup2(devnull_fd, 0); // stdin
                libc::dup2(devnull_fd, 1); // stdout
            }
            // Redirect stderr to log file
            let log_path = daemon_log_path(&session_name);
            let log_file = super::process::open_log_file_append(&log_path)?;
            let log_fd = log_file.as_raw_fd();
            unsafe {
                libc::dup2(log_fd, 2); // stderr
            }

            // Set up tracing
            let trace_path = daemon_trace_log_path(&session_name);
            let trace_file = super::process::open_log_file_truncate(&trace_path)?;
            let subscriber = tracing_subscriber::fmt()
                .with_writer(trace_file)
                .with_ansi(false)
                .with_max_level(tracing::Level::DEBUG)
                .finish();
            tracing::subscriber::set_global_default(subscriber).ok();
            super::process::install_panic_hook(panic_log_path(&session_name));

            // Create socket listener
            let listener = UnixListener::bind(&sock_path)?;
            // Blocking initially for accepting first client during init
            listener.set_nonblocking(false)?;

            Ok(ForkFirstResult::Child {
                init_phase: Box::new(DaemonInitPhase {
                    session_name,
                    factory_config,
                    supervisor_name,
                    worker_names,
                    phone_home,
                    listener,
                    init_client: None,
                }),
            })
        }
        Err(e) => {
            anyhow::bail!("Fork failed: {e}");
        }
    }
}

#[cfg(not(unix))]
pub fn fork_first_daemon(
    _session_name: String,
    _factory_config: FactoryConfig,
    _supervisor_name: String,
    _worker_names: Vec<String>,
    _phone_home: bool,
) -> anyhow::Result<ForkFirstResult> {
    anyhow::bail!("Fork-first daemon is only supported on Unix systems")
}

impl DaemonInitPhase {
    /// Run initialization with progress reporting
    ///
    /// This does all heavy lifting in the daemon process:
    /// 1. Waits for parent to connect
    /// 2. Sends progress updates during initialization
    /// 3. Spawns all PTYs (reader threads created here, in daemon)
    /// 4. Returns ready-to-run FactoryDaemon
    pub fn run_with_progress(mut self) -> anyhow::Result<FactoryDaemon> {
        use crate::config::Config;
        use crate::store::find_cas_root;
        use crate::ui::factory::app::{EpicState, epic_branch_name};
        use crate::ui::factory::director::DirectorData;
        use crate::worktree::{GitOperations, WorktreeConfig, WorktreeManager};
        use cas_mux::{Mux, MuxConfig};

        unsafe { std::env::set_var("CAS_FACTORY_SESSION", &self.session_name) };
        tracing::info!("Daemon init phase starting, waiting for parent to connect...");

        // Wait for parent to connect (with timeout)
        // UnixListener doesn't have set_read_timeout, so we use blocking accept
        // with a manual timeout via non-blocking polling
        self.listener.set_nonblocking(true)?;
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(30);
        let stream = loop {
            match self.listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if start.elapsed() > timeout {
                        anyhow::bail!("Timeout waiting for parent to connect");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e.into()),
            }
        };
        stream.set_write_timeout(Some(Duration::from_millis(500)))?;
        self.init_client = Some(stream);

        tracing::info!("Parent connected, starting initialization");

        // Step 1: Loading configuration
        self.send_progress("Loading configuration", 1, 6, false)?;
        let cas_dir = find_cas_root()?;
        self.send_progress("Loading configuration", 1, 6, true)?;

        // Step 2: Setting up worktree manager
        self.send_progress("Setting up worktree manager", 2, 6, false)?;
        // Determine worktree root: CLI flag > config file > default
        let worktree_root = self
            .factory_config
            .worktree_root
            .clone()
            .unwrap_or_else(|| {
                // Load config and use worktrees.base_path if set
                let config = Config::load(&cas_dir).unwrap_or_default();
                config
                    .worktrees()
                    .resolve_base_path(&self.factory_config.cwd)
            });

        // Create worktree manager if worktrees are enabled (even with 0 initial workers,
        // so dynamic spawning works later)
        let mut worktree_manager = if self.factory_config.enable_worktrees {
            let wt_config = WorktreeConfig {
                enabled: true,
                base_path: worktree_root.to_string_lossy().to_string(),
                branch_prefix: "factory/".to_string(),
                auto_merge: false,
                cleanup_on_close: false, // Factory manages cleanup
                promote_entries_on_merge: false,
            };
            Some(WorktreeManager::new(&self.factory_config.cwd, wt_config)?)
        } else {
            None
        };
        self.send_progress("Setting up worktree manager", 2, 6, true)?;

        // Step 3: Loading CAS data
        self.send_progress("Loading CAS data", 3, 6, false)?;
        let director_data = DirectorData::load(&cas_dir, Some(&worktree_root))?;
        let preferred_epic_focus = preferred_epic_focus_from_session_metadata();
        let epic_state = resolve_epic_state_for_focus(&director_data, &preferred_epic_focus);
        let git_ops = GitOperations::new(self.factory_config.cwd.clone());
        let trunk = git_ops.detect_default_branch();
        let epic_branch = if let EpicState::Active { epic_title, .. } = &epic_state {
            let branch_name = epic_branch_name(epic_title);
            if let (Ok(head_sha), Ok(trunk_sha)) =
                (git_ops.ref_sha("HEAD"), git_ops.ref_sha(&trunk))
            {
                if head_sha != trunk_sha {
                    tracing::warn!(
                        "supervisor HEAD ({}) differs from trunk '{}' ({}); epic branch will be based on trunk",
                        &head_sha[..head_sha.len().min(8)],
                        trunk,
                        &trunk_sha[..trunk_sha.len().min(8)]
                    );
                }
            }
            if git_ops.create_branch_from(&branch_name, &trunk)? {
                tracing::info!("Created epic branch {} from trunk '{}'", branch_name, trunk);
            } else {
                tracing::info!("Using existing epic branch: {}", branch_name);
            }
            Some(branch_name)
        } else {
            None
        };
        self.send_progress("Loading CAS data", 3, 6, true)?;

        // Step 4: Preparing worker directories
        self.send_progress("Preparing worker directories", 4, 6, false)?;
        let mut worker_cwds = HashMap::new();
        if let Some(ref mut manager) = worktree_manager {
            let worker_base = epic_branch.as_deref().unwrap_or(trunk.as_str());
            for name in &self.worker_names {
                let worktree = manager.ensure_worker_worktree_from(name, worker_base)?;
                worker_cwds.insert(name.clone(), worktree.path.clone());
            }
        }
        self.send_progress("Preparing worker directories", 4, 6, true)?;

        // Step 5: Clean up stale agents from previous sessions
        self.send_progress("Cleaning up stale agents", 5, 6, false)?;
        if let Ok(agent_store) = open_agent_store(&cas_dir) {
            if let Ok(agents) = AgentStore::list(&*agent_store, None) {
                for agent in agents {
                    // Only check agents that appear active/idle
                    if agent.status == cas_types::AgentStatus::Active
                        || agent.status == cas_types::AgentStatus::Idle
                    {
                        // Check if the Claude Code process is still running
                        if let Some(ppid) = agent.ppid {
                            if !is_process_running(ppid) {
                                // Process dead - clean up this stale agent
                                if let Err(e) = agent_store.graceful_shutdown(&agent.id) {
                                    tracing::warn!(
                                        "Failed to cleanup stale agent {}: {}",
                                        agent.name,
                                        e
                                    );
                                } else {
                                    tracing::info!(
                                        "Cleaned up stale agent: {} (ppid {} not running)",
                                        agent.name,
                                        ppid
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        self.send_progress("Cleaning up stale agents", 5, 6, true)?;

        // Step 6: Spawning agents (this creates PTY reader threads IN the daemon)
        self.send_progress("Spawning agents", 6, 6, false)?;

        let (cols, rows) = (120, 40);

        // If a resolved supervisor spec is present, override scalar supervisor fields (cas-1948).
        let (supervisor_cli, supervisor_model, supervisor_effort) =
            if let Some(ref sup_spec) = self.factory_config.resolved_supervisor_spec {
                let effort_str = sup_spec.effort.map(|e| e.as_claude_arg().to_string());
                (sup_spec.cli, sup_spec.model.clone(), effort_str)
            } else {
                (
                    self.factory_config.supervisor_cli,
                    self.factory_config.supervisor_model.clone(),
                    self.factory_config.supervisor_effort.clone(),
                )
            };

        // Build mux config
        let mux_config = MuxConfig {
            cwd: self.factory_config.cwd.clone(),
            cas_root: Some(cas_dir.clone()),
            factory_session: Some(self.session_name.clone()),
            worker_cwds: worker_cwds.clone(),
            workers: self.worker_names.len(),
            worker_names: self.worker_names.clone(),
            supervisor_name: self.supervisor_name.clone(),
            supervisor_cli,
            worker_cli: self.factory_config.worker_cli,
            supervisor_model,
            worker_model: self.factory_config.worker_model.clone(),
            supervisor_effort,
            worker_effort: self.factory_config.worker_effort.clone(),
            include_director: false,
            rows,
            cols,
            teams_configs: self.factory_config.teams_configs.clone(),
            resolved_worker_specs: self.factory_config.resolved_worker_specs.clone(),
        };

        // Send supervisor progress (clone to avoid borrow conflicts)
        let supervisor_name = self.supervisor_name.clone();
        self.send_agent_progress(&supervisor_name, true, 0.0, false)?;

        // Create mux - this spawns all Claude PTYs with reader threads
        let mut mux = Mux::factory(mux_config)?;
        mux.focus(&supervisor_name);

        // Supervisor done
        self.send_agent_progress(&supervisor_name, true, 1.0, true)?;

        // Workers were spawned by Mux::factory, send their progress
        let worker_names = self.worker_names.clone();
        for name in &worker_names {
            self.send_agent_progress(name, false, 0.5, false)?;
            self.send_agent_progress(name, false, 1.0, true)?;
        }

        self.send_progress("Spawning agents", 6, 6, true)?;

        // Create the FactoryApp (clone configs that don't implement Copy)
        let mut app = FactoryApp::from_init_result(
            cas_dir,
            mux,
            worktree_manager,
            director_data,
            self.supervisor_name.clone(),
            self.worker_names.clone(),
            self.factory_config.notify.clone(),
            self.factory_config.tabbed_workers,
            self.factory_config.auto_prompt.clone(),
            self.factory_config.supervisor_cli,
            self.factory_config.worker_cli,
            cols,
            rows,
            self.factory_config.record,
            self.factory_config.session_id.clone(),
            self.factory_config.lead_session_id.clone(),
            self.factory_config.cwd.clone(),
        )?;
        app.set_factory_session(self.session_name.clone());

        // Defer cloud phone-home client start to daemon.run() where a Tokio
        // runtime is available.  In the fork-first path, run_with_progress()
        // executes *before* the Tokio runtime is created, so calling
        // tokio::spawn() here would panic.
        let cloud_handle = None;

        // Create GUI socket for desktop clients
        let gui_sock_path = gui_socket_path(&self.session_name);
        if gui_sock_path.exists() {
            let _ = std::fs::remove_file(&gui_sock_path);
        }
        let gui_listener = UnixListener::bind(&gui_sock_path)?;
        gui_listener.set_nonblocking(true)?;

        // Remove orphaned team directories from previous crashed sessions
        super::runtime::teams::TeamsManager::cleanup_orphans();

        // Initialize native Agent Teams for inter-agent messaging.
        let teams = {
            let tm = super::runtime::teams::TeamsManager::new(&self.session_name);
            let worker_cwds: std::collections::HashMap<String, std::path::PathBuf> = app
                .worktree_manager()
                .map(|mgr| {
                    app.worker_names()
                        .iter()
                        .map(|name| (name.clone(), mgr.worktree_path_for_worker(name)))
                        .collect()
                })
                .unwrap_or_default();
            let lead_sid = self
                .factory_config
                .lead_session_id
                .as_deref()
                .unwrap_or(&self.session_name);
            match tm.init_team_config(
                app.worker_names(),
                app.project_path(),
                &worker_cwds,
                lead_sid,
            ) {
                Ok(()) => Some(tm),
                Err(e) => {
                    tracing::error!("Failed to init Teams config: {}", e);
                    None
                }
            }
        };

        // Save session metadata (after teams init so team_name is included)
        let session_manager = SessionManager::new();
        let project_dir = self.factory_config.cwd.to_string_lossy().to_string();
        let mut metadata = create_metadata(
            &self.session_name,
            std::process::id(),
            &self.supervisor_name,
            &self.worker_names,
            epic_state.epic_id(),
            Some(&project_dir),
            None, // No WebSocket port - using Unix socket
        );
        metadata.team_name = teams.as_ref().map(|t| t.team_name().to_string());
        session_manager.save_metadata(&metadata)?;

        // The boot client switches to attach as soon as it receives InitComplete.
        // Publish metadata first so the parent cannot race into `attach` and
        // report the generated session as missing.
        self.send_init_complete()?;
        tracing::info!("Initialization complete, transitioning to daemon mode");

        // Give boot screen client time to receive InitComplete before we close the socket.
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Close the init client (boot screen connection).
        if let Some(stream) = self.init_client.take() {
            drop(stream);
        }

        // Bind notification socket for instant prompt queue wakeup
        let notify_rx = match cas_factory::DaemonNotifier::bind(app.cas_dir()) {
            Ok(n) => Some(n),
            Err(e) => {
                tracing::warn!(
                    "Failed to create notification socket, falling back to polling: {}",
                    e
                );
                None
            }
        };

        // Return a FactoryDaemon for the main event loop
        Ok(FactoryDaemon {
            session_name: self.session_name,
            app,
            listener: self.listener,
            clients: HashMap::new(),
            next_client_id: 0,
            owner_client_id: None,
            owner_last_activity: Instant::now(),
            session_manager,
            shutdown: Arc::new(AtomicBool::new(false)),
            cols,
            rows,
            pending_resize: None,
            pending_resize_at: Instant::now(),
            compact_terminal: None,
            compact_cols: 0,
            compact_rows: 0,
            pending_spawns: VecDeque::new(),
            spawn_task: None,
            cloud_handle,
            phone_home: self.phone_home,
            relay_clients: HashMap::new(),
            pane_watchers: HashMap::new(),
            pane_buffers: HashMap::new(),
            gui_listener,
            gui_clients: HashMap::new(),
            next_gui_client_id: 0,
            ws_listener: None,
            ws_clients: HashMap::new(),
            next_ws_client_id: 0,
            tui_pane_sizes: HashMap::new(),
            web_pane_sizes: HashMap::new(),
            teams,
            notify_rx,
            dead_workers: std::collections::HashSet::new(),
            cancelled_spawns: std::collections::HashSet::new(),
            last_idle_message_times: HashMap::new(),
            resumed_epic_ids: std::collections::HashSet::new(),
        })
    }

    fn send_progress(
        &mut self,
        step: &str,
        num: u8,
        total: u8,
        completed: bool,
    ) -> anyhow::Result<()> {
        let msg = DaemonMessage::InitProgress {
            step: step.to_string(),
            step_num: num,
            total_steps: total,
            completed,
        };
        self.send_message(&msg)
    }

    fn send_agent_progress(
        &mut self,
        name: &str,
        is_supervisor: bool,
        progress: f32,
        ready: bool,
    ) -> anyhow::Result<()> {
        let msg = DaemonMessage::AgentProgress {
            name: name.to_string(),
            is_supervisor,
            progress,
            ready,
        };
        self.send_message(&msg)
    }

    fn send_init_complete(&mut self) -> anyhow::Result<()> {
        self.send_message(&DaemonMessage::InitComplete)
    }

    fn send_message(&mut self, msg: &DaemonMessage) -> anyhow::Result<()> {
        if let Some(ref mut client) = self.init_client {
            let encoded = protocol::encode_message(msg)?;
            client.write_all(&encoded)?;
            client.flush()?;
        }
        Ok(())
    }
}

/// Check if a process is running by PID
#[cfg(unix)]
fn is_process_running(pid: u32) -> bool {
    // Use kill -0 to check if process exists (sends no signal, just checks)
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_process_running(_pid: u32) -> bool {
    // On non-Unix, assume running if we have the PID
    true
}
