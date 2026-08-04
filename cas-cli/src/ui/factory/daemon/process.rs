use crate::ui::factory::daemon::imports::*;

/// Daemonize and run factory daemon (legacy - spawns subprocess)
#[cfg(unix)]
pub fn daemonize(config: DaemonConfig) -> anyhow::Result<()> {
    use std::process::Stdio;

    // Get current executable
    let exe = std::env::current_exe()?;

    // Build command with all config options
    let mut cmd = Command::new(exe);
    cmd.arg("factory")
        .arg("daemon")
        .arg("--session")
        .arg(&config.session_name)
        .arg("--cwd")
        .arg(&config.factory_config.cwd)
        .arg("--workers")
        .arg(config.factory_config.workers.to_string())
        .arg("--supervisor-cli")
        .arg(config.factory_config.supervisor_cli.as_str())
        .arg("--worker-cli")
        .arg(config.factory_config.worker_cli.as_str())
        .arg("--foreground");

    if config.boot_progress {
        cmd.arg("--boot-progress");
    }

    if let Some(ref supervisor_name) = config.factory_config.supervisor_name {
        cmd.arg("--supervisor-name").arg(supervisor_name);
    }

    for worker_name in &config.factory_config.worker_names {
        cmd.arg("--worker-name").arg(worker_name);
    }

    // Pass worktree settings
    if !config.factory_config.enable_worktrees {
        cmd.arg("--no-worktrees");
    }
    if let Some(ref worktree_root) = config.factory_config.worktree_root {
        cmd.arg("--worktree-root").arg(worktree_root);
    }

    // Pass notification settings
    if config.factory_config.notify.enabled {
        cmd.arg("--notify");
    }

    // Pass phone-home setting
    if !config.phone_home {
        cmd.arg("--no-phone-home");
    }

    // Spawn daemon process (redirect stderr to log file for debugging)
    let log_path = daemon_log_path(&config.session_name);
    let log_file = open_log_file_append(&log_path)?;
    let _child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file))
        .spawn()?;

    Ok(())
}

#[cfg(not(unix))]
pub fn daemonize(_config: DaemonConfig) -> anyhow::Result<()> {
    anyhow::bail!("Factory daemon mode is only supported on Unix systems")
}

/// Fork after initialization - parent becomes client, child becomes daemon
///
/// This is the preferred approach for boot screen with real progress:
/// 1. Parent does all initialization (shown in boot screen)
/// 2. After init, fork()
/// 3. Child inherits PTY file descriptors, becomes daemon
/// 4. Parent connects as client
///
/// Returns Ok(true) in parent (should attach as client)
/// Returns Ok(false) in child (will run daemon loop)
#[cfg(unix)]
pub fn fork_into_daemon(app: FactoryApp, session_name: String) -> anyhow::Result<ForkResult> {
    use nix::unistd::{ForkResult as NixForkResult, fork};
    use std::os::unix::io::AsRawFd;

    // Create socket BEFORE fork so both processes know the path
    let sock_path = socket_path(&session_name);

    // Remove stale socket if it exists
    if sock_path.exists() {
        std::fs::remove_file(&sock_path)?;
    }

    // Ensure parent directory exists
    if let Some(parent) = sock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Create listener
    let listener = UnixListener::bind(&sock_path)?;
    listener.set_nonblocking(true)?;

    // Save session metadata before fork (no ws_port for Unix socket daemon)
    let session_manager = SessionManager::new();
    let project_dir = app.cas_dir().to_string_lossy().to_string();
    let metadata = create_metadata(
        &session_name,
        std::process::id(),
        app.supervisor_name(),
        app.worker_names(),
        app.epic_state().epic_id(),
        Some(&project_dir),
        None, // ws_port - Unix socket daemon doesn't use WebSocket
    );
    session_manager.save_metadata(&metadata)?;

    // Fork!
    match unsafe { fork() } {
        Ok(NixForkResult::Parent { child }) => {
            // Parent process - will become client
            // Close the listener (child owns it now)
            drop(listener);

            // Drop the app (child owns the PTYs now)
            // The PTY file descriptors are still open in child
            drop(app);

            tracing::info!("Forked daemon child with PID {}", child);
            Ok(ForkResult::Parent)
        }
        Ok(NixForkResult::Child) => {
            // Child process - becomes daemon
            // Detach from terminal
            let _ = nix::unistd::setsid();

            // Redirect stdin/stdout/stderr to /dev/null
            let devnull = std::fs::File::open("/dev/null")?;
            let devnull_fd = devnull.as_raw_fd();
            unsafe {
                libc::dup2(devnull_fd, 0); // stdin
                libc::dup2(devnull_fd, 1); // stdout
            }
            // Keep stderr for logging or redirect to log file
            let log_path = daemon_log_path(&session_name);
            let log_file = open_log_file_append(&log_path)?;
            let log_fd = log_file.as_raw_fd();
            unsafe {
                libc::dup2(log_fd, 2); // stderr
            }

            // Set up tracing to file
            let trace_path = daemon_trace_log_path(&session_name);
            let trace_file = open_log_file_truncate(&trace_path)?;
            let subscriber = tracing_subscriber::fmt()
                .with_writer(trace_file)
                .with_ansi(false)
                .with_max_level(tracing::Level::DEBUG)
                .finish();
            tracing::subscriber::set_global_default(subscriber).ok();
            install_panic_hook(panic_log_path(&session_name));

            Ok(ForkResult::Child {
                app,
                listener,
                session_name,
            })
        }
        Err(e) => {
            anyhow::bail!("Fork failed: {e}");
        }
    }
}

#[cfg(not(unix))]
pub fn fork_into_daemon(_app: FactoryApp, _session_name: String) -> anyhow::Result<ForkResult> {
    anyhow::bail!("Fork-based daemon is only supported on Unix systems")
}

/// Result of fork_into_daemon
#[allow(clippy::large_enum_variant)]
pub enum ForkResult {
    /// Parent process - should attach as client
    Parent,
    /// Child process - should run daemon loop
    Child {
        app: FactoryApp,
        listener: UnixListener,
        session_name: String,
    },
}

/// Run the daemon loop after fork (called by child process)
pub async fn run_daemon_after_fork(
    mut app: FactoryApp,
    listener: UnixListener,
    session_name: String,
) -> anyhow::Result<()> {
    unsafe { std::env::set_var("CAS_FACTORY_SESSION", &session_name) };
    app.set_factory_session(session_name.clone());

    let trace_path = daemon_trace_log_path(&session_name);
    let trace_file = open_log_file_truncate(&trace_path)?;
    let subscriber = tracing_subscriber::fmt()
        .with_writer(trace_file)
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    tracing::subscriber::set_global_default(subscriber).ok();
    install_panic_hook(panic_log_path(&session_name));
    // Get terminal size from boot or use reasonable default
    // The client will send resize on connect to sync dimensions
    let (cols, rows) = crossterm::terminal::size().unwrap_or((120, 40));

    // Sync pane sizes to match current dimensions
    let _ = app.handle_resize(cols, rows);

    let session_manager = SessionManager::new();

    // Start cloud phone-home (best-effort, returns None if not authenticated)
    let cloud_handle = FactoryDaemon::try_start_cloud_client(&session_name);

    // Create GUI socket for desktop clients
    let gui_sock_path = gui_socket_path(&session_name);
    if gui_sock_path.exists() {
        let _ = std::fs::remove_file(&gui_sock_path);
    }
    let gui_listener = UnixListener::bind(&gui_sock_path)?;
    gui_listener.set_nonblocking(true)?;

    // Remove orphaned team directories from previous crashed sessions
    super::runtime::teams::TeamsManager::cleanup_orphans();

    // Initialize native Agent Teams for inter-agent messaging.
    let teams = {
        let tm = super::runtime::teams::TeamsManager::new(&session_name);
        let worker_cwds: std::collections::HashMap<String, std::path::PathBuf> = app
            .worktree_manager()
            .map(|mgr| {
                app.worker_names()
                    .iter()
                    .map(|name| (name.clone(), mgr.worktree_path_for_worker(name)))
                    .collect()
            })
            .unwrap_or_default();
        let lead_sid = app.lead_session_id().unwrap_or(&session_name).to_string();
        match tm.init_team_config(
            app.worker_names(),
            app.project_path(),
            &worker_cwds,
            &lead_sid,
        ) {
            Ok(()) => Some(tm),
            Err(e) => {
                tracing::error!("Failed to init Teams config: {}", e);
                None
            }
        }
    };

    // Re-save session metadata with team_name (metadata was saved pre-fork without it)
    if let Some(ref teams) = teams {
        let project_dir = app.cas_dir().to_string_lossy().to_string();
        let mut metadata = create_metadata(
            &session_name,
            std::process::id(),
            app.supervisor_name(),
            app.worker_names(),
            app.epic_state().epic_id(),
            Some(&project_dir),
            None,
        );
        metadata.team_name = Some(teams.team_name().to_string());
        let _ = session_manager.save_metadata(&metadata);
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

    let mut daemon = FactoryDaemon {
        session_name,
        app,
        listener,
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
        spawn_verifications: HashMap::new(),
        cloud_handle,
        phone_home: false,
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
        last_prompt_poison_sweep: Some(Instant::now()),
        resumed_epic_ids: std::collections::HashSet::new(),
        spawn_started_at: None,
        last_spawn_queue_stall_scan: None,
        reported_stalled_spawn_requests: std::collections::HashSet::new(),
    };

    daemon.run().await
}

/// Run the factory daemon (called by the daemon subprocess - legacy mode)
pub async fn run_daemon(config: DaemonConfig) -> anyhow::Result<()> {
    let trace_path = daemon_trace_log_path(&config.session_name);
    let log_file = open_log_file_truncate(&trace_path)?;
    let subscriber = tracing_subscriber::fmt()
        .with_writer(log_file)
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    tracing::subscriber::set_global_default(subscriber).ok();
    install_panic_hook(panic_log_path(&config.session_name));

    let mut daemon = FactoryDaemon::new(config)?;
    daemon.run().await
}

/// Run daemon with boot progress handshake (subprocess-safe, no raw fork).
pub async fn run_daemon_with_boot_progress(
    config: DaemonConfig,
    supervisor_name: String,
    worker_names: Vec<String>,
) -> anyhow::Result<()> {
    let trace_path = daemon_trace_log_path(&config.session_name);
    let log_file = open_log_file_truncate(&trace_path)?;
    let subscriber = tracing_subscriber::fmt()
        .with_writer(log_file)
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    tracing::subscriber::set_global_default(subscriber).ok();
    install_panic_hook(panic_log_path(&config.session_name));

    let phone_home = config.phone_home;
    let (init_phase, _sock_path) = super::fork_first::init_phase_without_fork(
        config.session_name,
        config.factory_config,
        supervisor_name,
        worker_names,
        phone_home,
    )?;
    let mut daemon = init_phase.run_with_progress()?;
    daemon.run().await
}

pub(super) fn open_log_file_append(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}

pub(super) fn open_log_file_truncate(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // One-time truncate, then hand back an O_APPEND descriptor. These must be
    // two separate opens: std rejects `append(true) + truncate(true)` with
    // EINVAL ("creating or truncating a file requires write or append access")
    // on every platform, which silently killed every daemon-entry path (cas-7fd9).
    //
    // O_APPEND on the returned descriptor is load-bearing: the spawn lifecycle
    // audit holds an independent append descriptor to this same file, and a
    // plain write descriptor would start at offset 0 and overwrite its records.
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}

pub(super) fn install_panic_hook(path: std::path::PathBuf) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut f) = open_log_file_append(&path) {
            let _ = writeln!(f, "PANIC: {info}");
            let bt = std::backtrace::Backtrace::force_capture();
            let _ = writeln!(f, "{bt}");
        }
        default(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: `append(true) + truncate(true)` is EINVAL on every platform.
    /// This function is the first statement of every daemon-entry path, so when
    /// it fails the daemon dies before serving its socket and the client hangs
    /// on the splash screen forever (cas-7fd9).
    #[test]
    fn open_log_file_truncate_succeeds_and_truncates_existing_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("trace.log");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"stale content from a previous run").unwrap();

        let mut file = open_log_file_truncate(&path).expect("must not fail with EINVAL");
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            0,
            "previous run's content must be truncated"
        );

        writeln!(file, "fresh").unwrap();
        file.flush().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fresh\n");
    }

    /// The returned descriptor must carry O_APPEND: the spawn lifecycle audit
    /// holds an independent append descriptor to the same file, and a plain
    /// write descriptor would seek to 0 and overwrite those records.
    #[test]
    fn open_log_file_truncate_returns_an_appending_descriptor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.log");

        let mut traced = open_log_file_truncate(&path).unwrap();
        let mut audit = open_log_file_append(&path).unwrap();

        writeln!(audit, "audit-record").unwrap();
        audit.flush().unwrap();
        writeln!(traced, "trace-record").unwrap();
        traced.flush().unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            contents.contains("audit-record"),
            "trace writes must not clobber the audit descriptor's records: {contents:?}"
        );
        assert!(contents.contains("trace-record"), "got {contents:?}");
    }

    /// The truncate happens once, at open. It must create a missing parent dir
    /// rather than failing, since daemon log dirs are session-scoped.
    #[test]
    fn open_log_file_truncate_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a").join("b").join("trace.log");

        open_log_file_truncate(&path).expect("parent dirs should be created");
        assert!(path.exists());
    }
}
