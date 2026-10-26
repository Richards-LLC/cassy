use crate::ui::factory::daemon::imports::*;

/// Daemonize and run factory daemon (legacy - spawns subprocess)
#[cfg(unix)]
pub fn daemonize(config: DaemonConfig) -> anyhow::Result<()> {
    crate::ui::factory::session::validate_daemon_start(&config.session_name, &config.factory_config.cwd)?;
    // Get current executable
    let exe = std::env::current_exe()?;

    // Build command with all config options
    let mut cmd = Command::new(exe);
    cmd.args(daemon_relaunch_args(&config));

    // Spawn daemon process (redirect stderr to log file for debugging)
    let log_path = daemon_log_path(&config.session_name);
    let log_file = open_log_file_append(&log_path)?;
    let child = spawn_daemon_process(cmd, log_file)?;

    if let Some(store) = crate::hub::DaemonExitEvidenceStore::default_for_user() {
        let _ = crate::hub::supervise_spawned_daemon(&config.session_name, child, store);
    }

    Ok(())
}

/// One spawn boundary for the production re-exec and subprocess lifecycle tests.
#[cfg(unix)]
pub(super) fn spawn_daemon_process(
    mut cmd: Command,
    log_file: std::fs::File,
) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    // The re-exec path must have the same terminal isolation as the fork
    // paths: a detached factory must not receive its former client's HUP.
    // SAFETY: setsid and errno inspection are async-signal-safe after fork.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file))
        .spawn()
}

#[cfg(unix)]
pub(super) struct DaemonSignalGuard(tokio::task::JoinHandle<()>);

#[cfg(unix)]
impl Drop for DaemonSignalGuard {
    fn drop(&mut self) { self.0.abort(); }
}

/// Signals end the session through the same owned-child cleanup as quit.
/// Keep the listener scoped to this daemon invocation, including error returns.
#[cfg(unix)]
pub(super) fn shutdown_on_signals(shutdown: Arc<AtomicBool>) -> std::io::Result<DaemonSignalGuard> {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    let mut hup = signal(SignalKind::hangup())?;
    Ok(DaemonSignalGuard(tokio::spawn(async move {
        tokio::select! {
            _ = term.recv() => {},
            _ = int.recv() => {},
            _ = hup.recv() => {},
        }
        shutdown.store(true, Ordering::Relaxed);
    })))
}

/// The `cas factory daemon …` arguments that relaunch this factory as a
/// subprocess daemon (the macOS path, where forking a multi-threaded process
/// before exec can crash). Everything the parent resolved must survive the
/// re-exec, because the child resolves its configuration again from these
/// arguments.
pub(crate) fn daemon_relaunch_args(config: &DaemonConfig) -> Vec<std::ffi::OsString> {
    let factory = &config.factory_config;
    let mut args: Vec<std::ffi::OsString> = vec![
        "factory".into(),
        "daemon".into(),
        "--session".into(),
        config.session_name.clone().into(),
        "--cwd".into(),
        factory.cwd.clone().into_os_string(),
        "--workers".into(),
        factory.workers.to_string().into(),
        "--supervisor-cli".into(),
        factory.supervisor_cli.backend().name().into(),
        "--worker-cli".into(),
        factory.worker_cli.backend().name().into(),
        "--foreground".into(),
    ];
    if config.boot_progress {
        args.push("--boot-progress".into());
    }
    if let Some(ref supervisor_name) = factory.supervisor_name {
        args.push("--supervisor-name".into());
        args.push(supervisor_name.clone().into());
    }
    for worker_name in &factory.worker_names {
        args.push("--worker-name".into());
        args.push(worker_name.clone().into());
    }
    // Pass worktree settings
    if !factory.enable_worktrees {
        args.push("--no-worktrees".into());
    }
    if let Some(ref worktree_root) = factory.worktree_root {
        args.push("--worktree-root".into());
        args.push(worktree_root.clone().into_os_string());
    }
    // Pass notification settings
    if factory.notify.enabled {
        args.push("--notify".into());
    }
    // Pass phone-home setting
    if !config.phone_home {
        args.push("--no-phone-home".into());
    }
    // cas-a5c0 (cas-2992 item 2): the resolved specs, so per-worker and
    // supervisor cli/model/effort/account-dir overrides survive the re-exec.
    for spec in &factory.resolved_worker_specs {
        args.push("--worker-spec-json".into());
        args.push(relaunch_spec_json(spec).into());
    }
    if let Some(ref spec) = factory.resolved_supervisor_spec {
        args.push("--supervisor-spec-json".into());
        args.push(relaunch_spec_json(spec).into());
    }
    args
}

/// One resolved spec in the `--worker-spec` JSON schema the child's resolver
/// reads: `{name, cli, model, effort, config_dir}`, unset fields omitted.
fn relaunch_spec_json(spec: &cas_mux::WorkerSpec) -> String {
    let mut object = serde_json::Map::new();
    if let Some(ref name) = spec.name {
        object.insert("name".into(), name.clone().into());
    }
    object.insert("cli".into(), spec.cli.backend().name().into());
    if let Some(ref model) = spec.model {
        object.insert("model".into(), model.clone().into());
    }
    if let Some(effort) = spec.effort {
        object.insert("effort".into(), effort.as_str().into());
    }
    if let Some(ref config_dir) = spec.config_dir {
        object.insert("config_dir".into(), config_dir.clone().into());
    }
    serde_json::Value::Object(object).to_string()
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

    // Ensure parent directory exists
    if let Some(parent) = sock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Create listener
    let listener = bind_factory_socket(&sock_path)?;
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
    session_manager.save_daemon_metadata(metadata, app.cas_dir())?;

    // cas-65cce: close idle pooled SQLite connections before fork(). A child
    // that inherits an open connection shares its file descriptors and locks
    // with the parent, which SQLite does not support across fork.
    cas_store::shared_db::close_idle_connections();
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
            // The daemon outlives the clients it writes to (cas-5918).
            crate::server_signals::ignore_sigpipe_for_server();

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
    crate::server_signals::ignore_sigpipe_for_server();
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
    let gui_listener = bind_factory_socket(&gui_sock_path)?;
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
        let _ = session_manager.save_daemon_metadata(metadata, app.cas_dir());
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
    let ai_enrichment = crate::config::Config::load(app.cas_dir())
        .unwrap_or_default()
        .factory()
        .ai_enrichment;

    // cas-d2a2: the prompt queue's store work runs on its own job lanes.
    let queue_jobs = crate::ui::factory::daemon::runtime::prompt_queue_jobs::QueueJobs::start(app.cas_dir().to_path_buf(), app.project_scope());
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
        binding_intake: Default::default(),
        spawn_task: None,
        spawn_verifications: HashMap::new(),
        cloud_handle,
        phone_home: false,
        relay_clients: HashMap::new(),
        pane_watchers: HashMap::new(),
        pane_buffers: HashMap::new(),
        session_summarizer: super::runtime::session_summarizer::SessionSummarizer::new(
            ai_enrichment,
        ),
        gui_listener,
        gui_clients: HashMap::new(),
        next_gui_client_id: 0,
        ws_listener: None,
        ws_clients: HashMap::new(),
        terminal_exchange: Default::default(),
        next_ws_client_id: 0,
        relay_token: None,
        web_pane_sizes: HashMap::new(),
        teams,
        notify_rx,
        dead_workers: std::collections::HashSet::new(),
        recent_worker_exits: Vec::new(),
        reported_unavailable_workers: std::collections::HashMap::new(),
        last_usage_limit_scan: None,
        open_operator_notices: std::collections::HashMap::new(),
        last_permission_request_scan: None,
        last_death_sweep: None,
        reported_permission_requests: std::collections::HashSet::new(),
        relay_jobs: Default::default(),
        last_commander_mirror_scan: None,
        queue_jobs,
        reported_auth_failed_workers: std::collections::HashMap::new(),
        last_auth_failure_scan: None,
        cancelled_spawns: std::collections::HashSet::new(),
        last_idle_message_times: HashMap::new(),
        stall_nudge_gate_checks: HashMap::new(),
        lifecycle_redelivery_attempts: HashMap::new(),
        lifecycle_redelivery_counts: HashMap::new(),
        inbox_deferred_writes: std::collections::HashMap::new(),
        urgent_wake_probes: HashMap::new(),
        urgent_injects: Default::default(),
        operator_refresh_requested: false,
        send_receipts: super::runtime::send_dedupe::SendReceipts::default(),
        normal_delivery_probes: HashMap::new(),
        last_pane_output_bytes: HashMap::new(),
        pane_silent_since: HashMap::new(),
        gate_activity_recorded_at: HashMap::new(),
        last_prompt_poison_sweep: Some(Instant::now()),
        resumed_epic_ids: std::collections::HashSet::new(),
        spawn_started_at: None,
        spawn_cancellation: None,
        last_spawn_queue_stall_scan: None,
        external_wake: Default::default(),
        reminder_scan_generation: 0,
        pending_ci_wakes: Vec::new(),
        reported_stalled_spawn_requests: std::collections::HashSet::new(),
    };

    daemon.run().await
}

/// Run the factory daemon (called by the daemon subprocess - legacy mode)
pub async fn run_daemon(config: DaemonConfig) -> anyhow::Result<()> {
    crate::ui::factory::session::validate_daemon_start(&config.session_name, &config.factory_config.cwd)?;
    // A client that vanishes mid-write must not kill the daemon (cas-5918).
    crate::server_signals::ignore_sigpipe_for_server();
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
    crate::ui::factory::session::validate_daemon_start(&config.session_name, &config.factory_config.cwd)?;
    // Boot progress goes to a parent that may exit first (cas-5918).
    crate::server_signals::ignore_sigpipe_for_server();
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

    fn relaunch_config() -> DaemonConfig {
        let mut factory = FactoryConfig::default();
        factory.workers = 2;
        factory.worker_names = vec!["alice".into(), "bob".into()];
        factory.supervisor_name = Some("lead".into());
        factory.resolved_worker_specs = vec![
            cas_mux::WorkerSpec {
                name: Some("alice".into()),
                cli: cas_mux::SupervisorCli::Codex,
                model: Some("gpt-5.5-codex".into()),
                effort: Some(cas_mux::Effort::XHigh),
                config_dir: Some("/tmp/codex-home-a".into()),
                requester_config_dir: None,
                requester_secure_storage_dir: None,
            },
            cas_mux::WorkerSpec {
                name: Some("bob".into()),
                cli: cas_mux::SupervisorCli::Claude,
                model: Some("claude-opus-5-5".into()),
                effort: Some(cas_mux::Effort::Medium),
                config_dir: None,
                requester_config_dir: None,
                requester_secure_storage_dir: None,
            },
        ];
        factory.resolved_supervisor_spec = Some(cas_mux::WorkerSpec {
            name: Some("lead".into()),
            cli: cas_mux::SupervisorCli::Claude,
            model: Some("claude-fable-5".into()),
            effort: Some(cas_mux::Effort::Max),
            config_dir: None,
            requester_config_dir: None,
            requester_secure_storage_dir: None,
        });
        DaemonConfig {
            session_name: "proj-bright-owl-1".into(),
            factory_config: factory,
            foreground: true,
            boot_progress: true,
            phone_home: true,
        }
    }

    fn flag_values(args: &[std::ffi::OsString], flag: &str) -> Vec<String> {
        args.windows(2)
            .filter(|pair| pair[0] == flag)
            .map(|pair| pair[1].to_string_lossy().into_owned())
            .collect()
    }

    /// cas-a5c0 (cas-2992 item 2): the macOS subprocess relaunch carries the
    /// resolved worker and supervisor specs, so a `--worker-spec` override
    /// (cli, model, effort, account dir) is not silently lost on re-exec. The
    /// arguments parse as `cas factory daemon`, and resolving them again
    /// yields the specs the parent resolved.
    #[test]
    fn subprocess_relaunch_carries_resolved_specs_cas_a5c0() {
        use clap::Parser;
        let config = relaunch_config();
        let args = daemon_relaunch_args(&config);

        let mut argv: Vec<std::ffi::OsString> = vec!["cas".into()];
        argv.extend(args.iter().cloned());
        crate::cli::Cli::try_parse_from(argv).expect("relaunch arguments parse as `cas factory daemon`");

        let worker_jsons = flag_values(&args, "--worker-spec-json");
        assert_eq!(worker_jsons.len(), 2, "{args:?}");
        let missing = std::path::PathBuf::from("/nonexistent/cas-a5c0/config.toml");
        let sources = cas_factory::spec_resolver::ConfigSources {
            worker_spec_jsons: worker_jsons,
            user_config: Some(missing.clone()),
            project_config: Some(missing.clone()),
            ..Default::default()
        };
        let resolved = cas_factory::spec_resolver::resolve_specs(2, sources).unwrap();
        for (got, want) in resolved.iter().zip(&config.factory_config.resolved_worker_specs) {
            assert_eq!(
                (&got.name, got.cli, &got.model, got.effort, &got.config_dir),
                (&want.name, want.cli, &want.model, want.effort, &want.config_dir)
            );
        }

        let supervisor_json = flag_values(&args, "--supervisor-spec-json");
        assert_eq!(supervisor_json.len(), 1, "{args:?}");
        let sources = cas_factory::spec_resolver::ConfigSources {
            supervisor_spec_json: supervisor_json.into_iter().next(),
            user_config: Some(missing.clone()),
            project_config: Some(missing),
            ..Default::default()
        };
        let supervisor = cas_factory::spec_resolver::resolve_supervisor_spec(sources).unwrap();
        let want = config.factory_config.resolved_supervisor_spec.as_ref().unwrap();
        assert_eq!(
            (supervisor.cli, &supervisor.model, supervisor.effort),
            (want.cli, &want.model, want.effort)
        );
    }

    /// A factory with no resolved specs relaunches exactly as before.
    #[test]
    fn subprocess_relaunch_without_specs_adds_no_spec_flags_cas_a5c0() {
        let mut config = relaunch_config();
        config.factory_config.resolved_worker_specs.clear();
        config.factory_config.resolved_supervisor_spec = None;
        let args = daemon_relaunch_args(&config);
        assert!(flag_values(&args, "--worker-spec-json").is_empty());
        assert!(flag_values(&args, "--supervisor-spec-json").is_empty());
        assert_eq!(flag_values(&args, "--worker-name"), vec!["alice", "bob"]);
    }

    /// cas-65cce: both daemon fork paths close idle shared_db connections
    /// immediately before fork(), so no child inherits a pooled SQLite handle.
    #[test]
    fn daemon_fork_paths_close_idle_db_connections_before_fork_cas_65cce() {
        // pin: fork() cannot be exercised safely inside the test process, so the close-before-fork ordering is pinned at source level.
        for (name, source) in [
            ("process.rs (daemonize)", include_str!("process.rs")),
            ("fork_first.rs (fork_first_daemon)", include_str!("fork_first.rs")),
        ] {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let forks: Vec<usize> = production.match_indices("match unsafe { fork() }").map(|(at, _)| at).collect();
            assert_eq!(forks.len(), 1, "{name}: one fork() call");
            let before = &production[..forks[0]];
            let close = before
                .rfind("cas_store::shared_db::close_idle_connections();")
                .unwrap_or_else(|| panic!("{name}: close_idle_connections() must precede fork()"));
            let between = &before[close..];
            assert!(
                between.lines().skip(1).all(|line| {
                    let line = line.trim();
                    line.is_empty() || line.starts_with("//")
                }),
                "{name}: nothing may run between close_idle_connections() and fork():\n{between}"
            );
        }
    }
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
