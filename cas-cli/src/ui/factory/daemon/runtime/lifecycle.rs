use crate::ui::factory::daemon::imports::*;

/// Persist one worker-health incident and hand it to the supervisor's durable
/// prompt queue. The detector has already debounced the episode and the event
/// has already been delivery-revalidated when this runs.
fn enqueue_worker_attention_relay(
    cas_dir: &std::path::Path,
    event: &crate::ui::factory::director::DirectorEvent,
) {
    use crate::mcp::tools::core::task::lifecycle::supervisor_push::{
        LIFECYCLE_WAKE_SOURCE_PREFIX, resolve_owning_supervisor,
    };
    use cas_store::{NotificationPriority, NotifyIdempotentResult};

    let (kind, worker, task_id, elapsed_secs) = match event {
        crate::ui::factory::director::DirectorEvent::WorkerIdle {
            worker,
            active_task: None,
        } => ("worker_idle", worker.as_str(), None, None),
        crate::ui::factory::director::DirectorEvent::WorkerStalled {
            worker,
            task_id,
            elapsed_secs,
            escalate: true,
        } => (
            "worker_stalled",
            worker.as_str(),
            Some(task_id.as_str()),
            Some(*elapsed_secs),
        ),
        _ => return,
    };
    let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
    let Ok(agent_store) = crate::store::open_agent_store(cas_dir) else {
        tracing::warn!("worker attention relay skipped: agent store unavailable");
        return;
    };
    let Some(supervisor) =
        resolve_owning_supervisor(agent_store.as_ref(), factory_session.as_deref())
    else {
        return;
    };
    let Ok(supervisor_queue) = crate::store::open_supervisor_queue_store(cas_dir) else {
        tracing::warn!(worker = %worker, kind, "worker attention relay skipped: supervisor queue unavailable");
        return;
    };
    // The event detector emits once per episode. The timestamp distinguishes a
    // later recovered-and-stalled episode while the queue dedupes retries of
    // this exact durable occurrence.
    let occurrence = chrono::Utc::now().to_rfc3339();
    let key = format!(
        "worker-attention:{}:{kind}:{worker}:{}:{occurrence}",
        factory_session.as_deref().unwrap_or(""),
        task_id.unwrap_or("")
    );
    let payload = serde_json::json!({
        "kind": kind,
        "worker": worker,
        "task_id": task_id,
        "elapsed_secs": elapsed_secs,
        "factory_session": factory_session,
        "occurrence": occurrence,
    })
    .to_string();
    let notification_id = match supervisor_queue.notify_idempotent(
        &supervisor.agent_id,
        kind,
        &payload,
        NotificationPriority::High,
        &key,
    ) {
        Ok(NotifyIdempotentResult::Created(id)) => id,
        Ok(NotifyIdempotentResult::AlreadyExists {
            id: _,
            prompt_delivered: true,
        }) => return,
        Ok(NotifyIdempotentResult::AlreadyExists {
            id,
            prompt_delivered: false,
        }) => id,
        Err(error) => {
            tracing::error!(worker = %worker, kind, %error, "worker attention relay durable enqueue failed");
            return;
        }
    };
    let Ok(prompt_queue) = crate::store::open_prompt_queue_store(cas_dir) else {
        tracing::error!(worker = %worker, kind, notification_id, "worker attention relay left pending: prompt queue unavailable");
        return;
    };
    let detail = match (kind, task_id, elapsed_secs) {
        ("worker_stalled", Some(task), Some(elapsed)) => format!(
            "Worker {worker} is stalled on {task} after {}m without activity.",
            elapsed / 60
        ),
        _ => format!("Worker {worker} is idle with no active task and needs supervisor attention."),
    };
    let body = format!(
        "<worker-attention kind=\"{kind}\" worker=\"{worker}\" notification_id=\"{notification_id}\">\n{detail}\nRun `coordination action=worker_status` and reassign or recover the worker as needed.\n</worker-attention>"
    );
    let source = format!("{LIFECYCLE_WAKE_SOURCE_PREFIX}worker-attention:{notification_id}");
    if let Err(error) = prompt_queue.enqueue_idempotent(
        &source,
        "supervisor",
        &body,
        factory_session.as_deref(),
        Some(&format!("{kind}: {worker}")),
        Some(NotificationPriority::High),
        &format!("worker-attention-outbox:{notification_id}"),
    ) {
        tracing::error!(worker = %worker, kind, notification_id, %error, "worker attention relay prompt enqueue failed");
        return;
    }
    if let Err(error) = supervisor_queue.mark_prompt_delivered(notification_id) {
        tracing::warn!(notification_id, %error, "worker attention relay prompt stamp failed; idempotent replay remains safe");
    }
}

#[cfg(test)]
mod worker_attention_tests {
    use super::*;
    use cas_types::{Agent, AgentRole, AgentStatus};

    #[test]
    fn websocket_keyframe_requests_stay_before_pty_delta_drain() {
        let source = include_str!("lifecycle.rs");
        let request = source
            .find("let ws_activity = self.process_ws_client_input().await;")
            .expect("daemon loop must process WS keyframe requests");
        let drain = source
            .find("let (bytes_processed, events) = self.app.mux.poll_batch();")
            .expect("daemon loop must drain PTY deltas");
        assert!(
            request < drain,
            "WS keyframe capture must remain before PTY drain so queued bytes become ordered post-keyframe deltas"
        );
    }

    fn register_supervisor(cas_dir: &std::path::Path, session: &str) {
        let agents = crate::store::open_agent_store(cas_dir).unwrap();
        let mut supervisor = Agent::new("supervisor-id".to_string(), "supervisor".to_string());
        supervisor.role = AgentRole::Supervisor;
        supervisor.status = AgentStatus::Active;
        supervisor.factory_session = Some(session.to_string());
        agents.register(&supervisor).unwrap();
    }

    #[test]
    fn taskless_idle_and_escalated_stall_use_durable_wake_relay() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "worker-attention-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "worker-attention-test");

        enqueue_worker_attention_relay(
            &cas_dir,
            &crate::ui::factory::director::DirectorEvent::WorkerIdle {
                worker: "calm-owl".to_string(),
                active_task: None,
            },
        );
        enqueue_worker_attention_relay(
            &cas_dir,
            &crate::ui::factory::director::DirectorEvent::WorkerStalled {
                worker: "steady-otter".to_string(),
                task_id: "cas-d4ae".to_string(),
                elapsed_secs: 300,
                escalate: true,
            },
        );

        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let rows = queue.peek_all(10).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| {
            row.target == "supervisor"
                && crate::mcp::tools::core::task::lifecycle::supervisor_push::is_lifecycle_wake_source(&row.source)
                && crate::prompt_revalidation::is_supervisor_wake_envelope(&row.prompt)
        }));
        assert!(rows.iter().any(|row| row.prompt.contains("kind=\"worker_idle\"")));
        assert!(rows.iter().any(|row| {
            row.prompt.contains("kind=\"worker_stalled\"") && row.prompt.contains("cas-d4ae")
        }));
    }
}

impl FactoryDaemon {
    pub fn new(config: DaemonConfig) -> anyhow::Result<Self> {
        // Get initial terminal size (default for daemon without terminal)
        let (cols, rows) = (120, 40);

        // Extract fields before factory_config is moved
        let project_dir = config.factory_config.cwd.to_string_lossy().to_string();
        let lead_session_id = config.factory_config.lead_session_id.clone();

        // Set factory session env var so PTY children (and their MCP servers) inherit it.
        // SAFETY: called before spawning any threads or async tasks in this process.
        unsafe { std::env::set_var("CAS_FACTORY_SESSION", &config.session_name) };

        // Create the factory app (this spawns Claude instances)
        let mut app = FactoryApp::new(config.factory_config)?;
        app.set_factory_session(config.session_name.clone());

        // Track factory session start
        crate::telemetry::track_factory_started("supervisor", app.worker_names().len());
        let initial_workers = app.worker_names().len().to_string();
        crate::telemetry::track(
            "factory_session_started",
            vec![
                ("mode", "daemon"),
                (
                    "worktrees_enabled",
                    if app.worktrees_enabled() {
                        "true"
                    } else {
                        "false"
                    },
                ),
                ("initial_workers", &initial_workers),
            ],
        );

        // Create socket
        let sock_path = socket_path(&config.session_name);

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

        // Create GUI socket (for desktop GUI clients using JSON protocol)
        let gui_sock_path = gui_socket_path(&config.session_name);
        if gui_sock_path.exists() {
            std::fs::remove_file(&gui_sock_path)?;
        }
        let gui_listener = UnixListener::bind(&gui_sock_path)?;
        gui_listener.set_nonblocking(true)?;

        // Bind WebSocket listener on localhost with OS-assigned port.
        // Use std TcpListener first (sync constructor), then convert to tokio in run().
        let ws_listener = match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(l) => {
                l.set_nonblocking(true)?;
                match tokio::net::TcpListener::from_std(l) {
                    Ok(tl) => Some(tl),
                    Err(e) => {
                        tracing::warn!("Failed to convert WS listener to tokio: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to bind WS listener: {}", e);
                None
            }
        };
        let ws_port = ws_listener
            .as_ref()
            .and_then(|l| l.local_addr().ok())
            .map(|a| a.port());

        let session_manager = SessionManager::new();

        // Optionally start cloud phone-home client
        let cloud_handle = if config.phone_home {
            Self::try_start_cloud_client(&config.session_name)
        } else {
            None
        };

        // Remove orphaned team directories from previous crashed sessions
        super::teams::TeamsManager::cleanup_orphans();

        // Initialize native Agent Teams for inter-agent messaging (Claude CLI only).
        let teams = {
            let tm = super::teams::TeamsManager::new(&config.session_name);
            let worker_cwds: std::collections::HashMap<String, std::path::PathBuf> = app
                .worktree_manager()
                .map(|mgr| {
                    app.worker_names()
                        .iter()
                        .map(|name| (name.clone(), mgr.worktree_path_for_worker(name)))
                        .collect()
                })
                .unwrap_or_default();
            let lead_sid = lead_session_id.as_deref().unwrap_or(&config.session_name);
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

        // Save session metadata (after teams init so team_name is included)
        let mut metadata = create_metadata(
            &config.session_name,
            std::process::id(),
            app.supervisor_name(),
            app.worker_names(),
            app.epic_state().epic_id(),
            Some(&project_dir),
            ws_port,
        );
        metadata.team_name = teams.as_ref().map(|t| t.team_name().to_string());
        session_manager.save_metadata(&metadata)?;

        Ok(Self {
            session_name: config.session_name,
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
            ws_listener,
            ws_clients: HashMap::new(),
            next_ws_client_id: 0,
            tui_pane_sizes: HashMap::new(),
            web_pane_sizes: HashMap::new(),
            teams,
            notify_rx,
            dead_workers: std::collections::HashSet::new(),
            cancelled_spawns: std::collections::HashSet::new(),
            last_idle_message_times: HashMap::new(),
            lifecycle_redelivery_attempts: HashMap::new(),
            lifecycle_redelivery_counts: HashMap::new(),
            inbox_deferred_writes: std::collections::HashMap::new(),
            urgent_wake_probes: HashMap::new(),
            normal_delivery_probes: HashMap::new(),
            last_pane_output_bytes: HashMap::new(),
            pane_silent_since: HashMap::new(),
            last_prompt_poison_sweep: Some(Instant::now()),
            resumed_epic_ids: std::collections::HashSet::new(),
            spawn_started_at: None,
            last_spawn_queue_stall_scan: None,
            reported_stalled_spawn_requests: std::collections::HashSet::new(),
        })
    }

    /// Get the shutdown flag for external control
    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown)
    }

    /// Run the daemon main loop with TUI rendering
    pub async fn run(&mut self) -> anyhow::Result<()> {
        // Bind WebSocket listener if not already bound (fork-first and legacy paths
        // set ws_listener=None because they run before the Tokio runtime exists).
        if self.ws_listener.is_none() {
            match std::net::TcpListener::bind("127.0.0.1:0") {
                Ok(l) => {
                    let _ = l.set_nonblocking(true);
                    match tokio::net::TcpListener::from_std(l) {
                        Ok(tl) => {
                            let port = tl.local_addr().ok().map(|a| a.port());
                            self.ws_listener = Some(tl);
                            // Update session metadata with ws_port
                            if let Some(port) = port {
                                let meta_path =
                                    crate::ui::factory::session::metadata_path(&self.session_name);
                                if let Ok(data) = std::fs::read_to_string(&meta_path) {
                                    if let Ok(mut meta) =
                                        serde_json::from_str::<
                                            crate::ui::factory::protocol::SessionMetadata,
                                        >(&data)
                                    {
                                        meta.ws_port = Some(port);
                                        let _ = self.session_manager.save_metadata(&meta);
                                    }
                                }
                                tracing::info!("WS listener bound on 127.0.0.1:{}", port);
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to convert WS listener to tokio: {}", e);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to bind WS listener: {}", e);
                }
            }
        }

        // Start deferred cloud phone-home client (fork-first path defers this
        // because run_with_progress() runs before the Tokio runtime exists).
        if self.phone_home && self.cloud_handle.is_none() {
            self.cloud_handle = Self::try_start_cloud_client(&self.session_name);
        }

        let session_started_at = Instant::now();

        // Create buffer backend for rendering
        let backend = BufferBackend::with_hyperlinks(
            self.cols,
            self.rows,
            self.app.full_pane_hyperlink_map(),
        );
        let mut terminal = Terminal::new(backend)?;

        // Set initial terminal title
        set_terminal_title(self.app.project_path(), self.app.epic_state().epic_title());

        // Start recording if enabled
        if self.app.record_enabled() {
            if let Err(e) = self.app.start_recording().await {
                tracing::error!("Failed to start recording: {}", e);
            }
        }

        // Refresh intervals
        let mut last_refresh = std::time::Instant::now();
        let mut last_prompt_poll = std::time::Instant::now();
        let mut last_spawn_poll = std::time::Instant::now();
        // GitHub polling is intentionally decoupled from the two-second UI
        // refresh. The blocking CLI calls run on Tokio's blocking pool and
        // results are handed back to this loop for durable prompt enqueue.
        let mut last_ci_watch = std::time::Instant::now()
            .checked_sub(super::ci_watch::CI_WATCH_INTERVAL)
            .unwrap_or_else(std::time::Instant::now);
        let mut ci_watch_task: Option<
            JoinHandle<Result<Vec<super::ci_watch::CiFailure>, super::ci_watch::CiWatchError>>,
        > = None;
        let mut ci_watch_unavailable_reported = false;
        let refresh_interval = Duration::from_secs(2);
        let poll_interval = Duration::from_millis(100);

        let mut prompt_notified = false;

        while !self.shutdown.load(Ordering::Relaxed) {
            // Error timeout must run in daemon mode too (not only local event loop path).
            let had_error = self.app.error_message.is_some();
            self.app.check_error_timeout();
            let error_cleared_by_timeout = had_error && self.app.error_message.is_none();

            // Accept new client connections (non-blocking)
            let new_clients = self.accept_clients()?;

            // Accept new GUI client connections (non-blocking)
            let new_gui_clients = self.accept_gui_clients();

            // Accept new WebSocket client connections (non-blocking)
            let new_ws_clients = self.accept_ws_clients().await;

            // Read and process input from clients
            let input_activity = self.process_client_input().await?;

            // Read and process input from GUI clients
            let gui_activity = self.process_gui_client_input().await;

            // ORDERING INVARIANT: WS input MUST be processed before poll_batch.
            // RequestPaneKeyframe captures Ghostty state and queues the frame
            // under this loop's exclusive `&mut FactoryDaemon`; PTY bytes still
            // queued in the backend are then drained and queued as later Output.
            // Reordering or parallelizing these calls creates a snapshot/tap gap
            // that can silently lose terminal output during attach.
            let ws_activity = self.process_ws_client_input().await;

            // Poll PTYs for output using coalesced batch drain (efficient for 6 Claudes generating)
            let (bytes_processed, events) = self.app.mux.poll_batch();
            let had_output = bytes_processed > 0;
            for event in events {
                self.handle_mux_event(event).await;
            }

            // Process relay events from cloud (remote terminal attach/input/detach)
            self.process_relay_events().await;

            // cas-1a4d: retry non-urgent PTY injects only after every attached
            // client surface had a chance to submit or clear its composer.
            // This stays outside process_prompt_queue so a deferred inject
            // never blocks the input loop that can make its target clean.
            self.app.mux.flush_deferred_injections().await;

            // Poll prompt queue (on notification or timer)
            if prompt_notified || last_prompt_poll.elapsed() >= poll_interval {
                if prompt_notified {
                    if let Some(ref mut notify) = self.notify_rx {
                        notify.drain();
                    }
                }
                let _ = self.process_prompt_queue().await;
                // cas-ecff: auto-drain pending lifecycle outbox (durable
                // task_lifecycle rows with prompt_delivered_at unset) so
                // partial failures recover without re-running task mutations.
                if let Ok(sq) = crate::store::open_supervisor_queue_store(self.app.cas_dir()) {
                    if let Ok(pq) = crate::store::open_prompt_queue_store(self.app.cas_dir()) {
                        match crate::mcp::tools::core::task::lifecycle::supervisor_push::drain_lifecycle_outbox(
                            sq.as_ref(),
                            pq.as_ref(),
                            50,
                        ) {
                            Ok(report) if report.recovered > 0 || report.failed > 0 => {
                                tracing::info!(
                                    recovered = report.recovered,
                                    failed = report.failed,
                                    attempted = report.attempted,
                                    "lifecycle outbox drain"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "lifecycle outbox drain failed");
                            }
                            _ => {}
                        }
                    }
                }
                last_prompt_poll = std::time::Instant::now();
                prompt_notified = false;
            }

            // Poll spawn queue (enqueues requests, doesn't execute them)
            if last_spawn_poll.elapsed() >= poll_interval {
                let _ = self.enqueue_spawn_requests();
                last_spawn_poll = std::time::Instant::now();
            }

            // Process pending spawns (non-blocking: git ops run on background thread)
            if self.spawn_task.is_some() || !self.pending_spawns.is_empty() {
                self.process_pending_spawns().await;
            }

            // Periodic CAS data refresh
            let mut refreshed = false;
            if last_refresh.elapsed() >= refresh_interval {
                // Collect a completed GitHub Actions snapshot in the background.
                // No supervisor action is required to notice a red run: the
                // completed result below becomes a lifecycle-wake relay.
                if ci_watch_task.is_none()
                    && last_ci_watch.elapsed() >= super::ci_watch::CI_WATCH_INTERVAL
                {
                    let project = self.app.project_path().to_path_buf();
                    let mut watched_branches =
                        std::collections::BTreeSet::from(["main".to_string()]);
                    if let Some(manager) = self.app.worktree_manager() {
                        watched_branches.extend(
                            self.app
                                .worker_names()
                                .iter()
                                .map(|worker| manager.branch_name_for_worker(worker)),
                        );
                    } else if let Ok(output) = Command::new("git")
                        .args(["rev-parse", "--abbrev-ref", "HEAD"])
                        .current_dir(&project)
                        .output()
                        && output.status.success()
                    {
                        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if branch.starts_with("factory/") {
                            watched_branches.insert(branch);
                        }
                    }
                    ci_watch_task = Some(tokio::task::spawn_blocking(move || {
                        let transport = super::ci_watch::GhCiTransport::from_project(&project)?;
                        super::ci_watch::collect_failures(&transport, &watched_branches)
                    }));
                    last_ci_watch = std::time::Instant::now();
                }

                if ci_watch_task.as_ref().is_some_and(JoinHandle::is_finished) {
                    let task = ci_watch_task.take().expect("checked above");
                    match task.await {
                        Ok(Ok(failures)) => {
                            ci_watch_unavailable_reported = false;
                            if !failures.is_empty() {
                                match crate::store::open_prompt_queue_store(self.app.cas_dir()) {
                                    Ok(queue) => {
                                        for failure in failures {
                                            match super::ci_watch::emit_failure(
                                                queue.as_ref(),
                                                &self.session_name,
                                                &failure,
                                            ) {
                                                Ok(true) => tracing::warn!(
                                                    branch = %failure.branch,
                                                    head_sha = %failure.head_sha,
                                                    run_url = %failure.run_url,
                                                    failing_job = %failure.failing_job,
                                                    "queued CI red-run lifecycle wake for supervisor"
                                                ),
                                                Ok(false) => tracing::debug!(
                                                    branch = %failure.branch,
                                                    head_sha = %failure.head_sha,
                                                    "suppressed duplicate CI red-run relay"
                                                ),
                                                Err(error) => {
                                                    tracing::warn!(%error, "failed to queue CI red-run relay")
                                                }
                                            }
                                        }
                                    }
                                    Err(error) => {
                                        tracing::warn!(%error, "could not open prompt queue for CI red-run relays")
                                    }
                                }
                            }
                        }
                        Ok(Err(error)) => {
                            if !ci_watch_unavailable_reported {
                                tracing::warn!(%error, "GitHub CI watcher unavailable; retrying silently on its next cadence");
                                ci_watch_unavailable_reported = true;
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, "GitHub CI watcher background task failed")
                        }
                    }
                }

                // A successful PTY spawn is not a verified worker. Confirm the
                // harness reached CAS registration (or surface a bounded
                // timeout) on the existing two-second lifecycle cadence.
                self.reconcile_spawn_verifications().await;
                // cas-f9e8 telemetry: the gap between the previous refresh
                // and this one is Channel C's worst-case delivery latency
                // for director-generated events. Logged at debug; enable
                // via `RUST_LOG=cas::coordination=debug`.
                let refresh_started = std::time::Instant::now();
                let tick_interval_ms = last_refresh.elapsed().as_secs_f64() * 1000.0;
                if let Ok(events) = self.app.refresh_data() {
                    // cas-627f: combined into one call so an idle tick with
                    // zero events short-circuits before touching the DB at
                    // all, and a non-idle tick shares a single unfiltered
                    // load between revalidation and prompt generation
                    // instead of two independent (and possibly divergent)
                    // full DirectorData loads. See
                    // `revalidate_and_prompt_for_delivery` doc comment.
                    let (delivery_events, prompts, unfiltered_data_for_sweep) =
                        self.app.revalidate_and_prompt_for_delivery(&events);
                    tracing::debug!(
                        target: "cas::coordination",
                        stage = "director_refresh",
                        channel = "director_events",
                        event_count = delivery_events.len(),
                        stale_event_count = events.len().saturating_sub(delivery_events.len()),
                        prompt_count = prompts.len(),
                        tick_interval_ms,
                        "director refresh tick processed"
                    );

                    // Record events for export
                    self.app.record_events(&delivery_events);

                    // Send notifications for detected events
                    self.app.notify_events(&delivery_events);

                    // cas-d4ae: the detector has already emitted exactly one
                    // event for this idle/stall episode and the app just
                    // revalidated it against current task/worker state. Send
                    // the actionable cases through the durable supervisor
                    // wake lane before ordinary prompt injection.
                    for event in &delivery_events {
                        enqueue_worker_attention_relay(self.app.cas_dir(), event);
                    }

                    // Handle epic state transitions
                    let changes = self.app.handle_epic_events(&delivery_events);
                    for change in changes {
                        let _ = self.handle_epic_change(change).await;
                    }

                    // Process reminders (time-based and event-based)
                    self.process_reminders(&delivery_events);

                    // Push state and events to cloud (best-effort, no-op if not connected)
                    self.push_cloud_events(&delivery_events);
                    self.push_cloud_state();

                    // cas-ed6c: retract stale WorkerIdle-class alerts already
                    // queued in the supervisor's inbox — before injecting any
                    // NEW prompts this tick — using the SAME live snapshot
                    // just loaded for revalidation (no extra DB load). A
                    // `WorkerIdle` alert is revalidated against live state
                    // only at the instant it's written; if the named worker
                    // gained a real assignment before the recipient's next
                    // turn boundary (Claude Code only polls its inbox then,
                    // and `read` is never flipped by production code — see
                    // `InboxMessage::retract_worker` doc), the written row
                    // just sits there, stale, with nothing to catch it. This
                    // is the live-evidence-quoted-a-superseded-tip class of
                    // bug (three workers announced idle/ready ~7 minutes
                    // after each had a genuine InProgress assignment).
                    if let (Some(teams), Some(unfiltered_data)) =
                        (self.teams.as_ref(), unfiltered_data_for_sweep.as_ref())
                    {
                        match teams.prune_stale_idle_alerts("supervisor", |worker| {
                            crate::ui::factory::director::worker_now_has_real_assignment(
                                unfiltered_data,
                                worker,
                            )
                        }) {
                            Ok(0) => {}
                            Ok(n) => tracing::info!(
                                target: "cas::coordination",
                                stage = "retract_stale_idle_alert",
                                channel = "teams_inbox",
                                retracted = n,
                                "swept stale WorkerIdle alert(s) from supervisor inbox before delivery"
                            ),
                            Err(e) => tracing::warn!(
                                target: "cas::coordination",
                                error = %e,
                                "prune_stale_idle_alerts failed — non-fatal, stale alerts may still be delivered"
                            ),
                        }

                        // cas-e48f: retract stale MERGE REQUIRED alerts the
                        // same way, keyed on task_id rather than worker name
                        // — see `InboxMessage::retract_task` / `Prompt::
                        // retract_task` doc for why `worker_now_has_real_
                        // assignment` above is the WRONG predicate for this
                        // alert class. `check_merge_alert_freshness_for_task`
                        // re-reads the CURRENT epic tip at sweep time (never
                        // the tip captured when the row was written) — the
                        // exact live incident this closes: an alert quoting
                        // "checked against epic tip 811377c" delivered after
                        // the epic had already advanced past that tip.
                        let repo_root = self
                            .app
                            .cas_dir()
                            .parent()
                            .unwrap_or(self.app.cas_dir())
                            .to_path_buf();
                        match teams.prune_stale_merge_alerts("supervisor", |task_id| {
                            matches!(
                                crate::ui::factory::director::check_merge_alert_freshness_for_task(
                                    task_id,
                                    unfiltered_data,
                                    &repo_root,
                                ),
                                crate::ui::factory::director::MergeAlertFreshness::Stale
                            )
                        }) {
                            Ok(0) => {}
                            Ok(n) => tracing::info!(
                                target: "cas::coordination",
                                stage = "retract_stale_merge_alert",
                                channel = "teams_inbox",
                                retracted = n,
                                "swept stale MERGE REQUIRED alert(s) from supervisor inbox before delivery"
                            ),
                            Err(e) => tracing::warn!(
                                target: "cas::coordination",
                                error = %e,
                                "prune_stale_merge_alerts failed — non-fatal, stale alerts may still be delivered"
                            ),
                        }

                        // cas-06ca: EpicAllSubtasksClosed bypasses the durable
                        // prompt_queue and is written directly to the Teams
                        // inbox. Carrying epic_id on that row lets this existing
                        // generic retraction mechanism re-check the same live
                        // predicate used before transport. This block only runs
                        // with an authoritative store snapshot; missing state is
                        // uncertainty and preserves the row.
                        match teams.prune_stale_epic_completion_alerts("supervisor", |epic_id| {
                            !crate::ui::factory::director::epic_completion_is_current(
                                unfiltered_data,
                                epic_id,
                            )
                        }) {
                            Ok(0) => {}
                            Ok(n) => tracing::info!(
                                target: "cas::coordination",
                                stage = "retract_stale_epic_completion",
                                channel = "teams_inbox",
                                retracted = n,
                                "swept stale epic-completion alert(s) before delivery"
                            ),
                            // Retraction is best-effort. A lock/read failure
                            // must never interrupt delivery or surface as a
                            // user-facing error.
                            Err(error) => tracing::debug!(
                                target: "cas::coordination",
                                error = %error,
                                "epic-completion inbox retraction skipped"
                            ),
                        }
                    }

                    // Inject prompts (config already checked in generate_prompt)
                    for prompt in prompts {
                        if !self.app.prompt_is_still_deliverable(&prompt) {
                            tracing::info!(
                                target: "cas::coordination",
                                stage = "drop_stale_taskless_worker_before_injection",
                                worker = prompt.drop_if_worker_assigned.as_deref().unwrap_or(""),
                                "dropped taskless-worker alert because assignment landed after batch revalidation"
                            );
                            continue;
                        }
                        // cas-ae6d (GH #100): a loss-intolerant prompt (today:
                        // the assignment wake-up) bound for a PTY pane that is
                        // not ready for injection goes to the durable
                        // prompt_queue instead of being written into a pane
                        // that silently swallows it during harness startup.
                        // The director lane has no readiness gate and no
                        // retry; the queue lane has both. Claude-under-teams
                        // recipients are unaffected — their inbox write is
                        // durable by construction, which is exactly why the
                        // same assignment woke a claude worker while codex
                        // workers were left idle.
                        let pane_was_unready = self.route_director_prompt_to_queue(&prompt);
                        if pane_was_unready {
                            match self.enqueue_director_prompt(&prompt) {
                                Ok(id) => {
                                    tracing::info!(
                                        target: "cas::coordination",
                                        stage = "director_prompt_queued",
                                        channel = "prompt_queue",
                                        target_agent = %prompt.target,
                                        prompt_id = id,
                                        "director prompt parked on the durable queue because the recipient's PTY pane is not ready"
                                    );
                                    continue;
                                }
                                Err(e) => tracing::warn!(
                                    target: "cas::coordination",
                                    stage = "director_prompt_queue_failed",
                                    target_agent = %prompt.target,
                                    error = %e,
                                    "durable enqueue failed; attempting direct injection instead"
                                ),
                            }
                        }
                        // cas-f9e8 telemetry: measure director prompt
                        // injection latency from the start of this refresh
                        // tick to the completion of the inbox write. This
                        // is Channel C's send→deliver envelope and is the
                        // number that tells us whether refresh_interval
                        // needs to be lowered for the P99 SLO.
                        let inject_started = std::time::Instant::now();
                        // Recipient-aware routing (cas-b68a): a director event aimed
                        // at a Codex agent must reach its PTY, not a Claude inbox.
                        let inject_result = self
                            .deliver_to_worker(
                                &prompt.target,
                                super::teams::DIRECTOR_AGENT_NAME,
                                &prompt.text,
                                None,
                                // D-4 (cas-405f): pass the director's config.json color so
                                // the inbox bubble matches the registered team entry.
                                Some(super::teams::DIRECTOR_AGENT_COLOR),
                                // cas-ed6c: tag WorkerIdle-class alerts so a
                                // later sweep can retract them if the named
                                // worker gains a real assignment before the
                                // recipient ever reads the queued row.
                                prompt.retract_worker.as_deref(),
                                // cas-e48f: tag MERGE REQUIRED alerts so a
                                // later sweep can retract them if this task's
                                // merge lands (or it leaves AwaitingMerge)
                                // before the recipient ever reads the row.
                                prompt.retract_task.as_deref(),
                                // cas-06ca: carry the epic occurrence identity
                                // through transport so unread completion rows
                                // can be retracted if live state advances.
                                prompt.retract_epic.as_deref(),
                            )
                            .await;
                        let inject_ms = inject_started.elapsed().as_secs_f64() * 1000.0;
                        let total_ms = refresh_started.elapsed().as_secs_f64() * 1000.0;
                        // cas-ae6d: a durable prompt that did not actually land
                        // (transport error, or a composer-dirty deferral this
                        // lane cannot retry) is re-queued rather than lost.
                        //
                        // `pane_was_unready` means we only reached this direct
                        // inject because the durable enqueue itself failed.
                        // `Mux::inject` reports Delivered as soon as the write
                        // syscall returns, so a pane still flushing its startup
                        // input buffer yields a delivered-looking write that the
                        // harness never sees. Do not let that count as durable
                        // delivery — retry it, and if the queue is still
                        // unwritable, say so loudly rather than silently.
                        let delivered = super::delivery::durable_delivery_landed(
                            matches!(inject_result, Ok(cas_mux::InjectOutcome::Delivered)),
                            pane_was_unready,
                        );
                        if super::delivery::needs_durable_followup(delivered, prompt.durable_retry)
                        {
                            match self.enqueue_director_prompt(&prompt) {
                                Ok(id) => tracing::info!(
                                    target: "cas::coordination",
                                    stage = "director_prompt_requeued",
                                    channel = "prompt_queue",
                                    target_agent = %prompt.target,
                                    prompt_id = id,
                                    "director prompt re-queued after a direct delivery attempt did not land"
                                ),
                                Err(e) => tracing::warn!(
                                    target: "cas::coordination",
                                    stage = "director_prompt_requeue_failed",
                                    target_agent = %prompt.target,
                                    error = %e,
                                    "durable re-queue failed; assignment wake-up may be lost"
                                ),
                            }
                        }
                        match inject_result {
                            Ok(cas_mux::InjectOutcome::Delivered) => tracing::info!(
                                target: "cas::coordination",
                                stage = "delivered",
                                channel = "director_events",
                                target_agent = %prompt.target,
                                inject_ms,
                                refresh_to_deliver_ms = total_ms,
                                "director prompt delivered to inbox"
                            ),
                            Ok(cas_mux::InjectOutcome::DeferredComposerDirty) => tracing::info!(
                                target: "cas::coordination",
                                stage = "composer_inject_deferred",
                                channel = "director_events",
                                target_agent = %prompt.target,
                                inject_ms,
                                "director prompt deferred before any PTY write because the operator composer is dirty"
                            ),
                            Err(e) => tracing::warn!(
                                target: "cas::coordination",
                                stage = "deliver_failed",
                                channel = "director_events",
                                target_agent = %prompt.target,
                                inject_ms,
                                error = %e,
                                "director prompt inject failed"
                            ),
                        }
                    }
                }
                last_refresh = std::time::Instant::now();
                refreshed = true;
            }

            // Apply debounced resize after 100ms of no new resize events
            let mut resize_applied = false;
            if let Some((cols, rows)) = self.pending_resize {
                if self.pending_resize_at.elapsed() >= Duration::from_millis(100) {
                    tracing::info!("Applying debounced resize: {}x{}", cols, rows);

                    // Determine if we have full-mode clients that need the full layout resize
                    let has_full_clients = self
                        .clients
                        .values()
                        .any(|c| c.view_mode == ClientViewMode::Full);

                    if has_full_clients {
                        // Use the largest full-mode client dimensions for the full layout
                        let (full_cols, full_rows) = self.dims_for_mode(ClientViewMode::Full);
                        if full_cols > 0 && full_rows > 0 {
                            self.cols = full_cols;
                            self.rows = full_rows;
                            let _ = self.app.handle_resize(full_cols, full_rows);
                        }
                    } else if cols >= COMPACT_WIDTH_THRESHOLD {
                        // No explicit full clients but this resize is full-sized
                        self.cols = cols;
                        self.rows = rows;
                        let _ = self.app.handle_resize(cols, rows);
                    }

                    // Update compact terminal dimensions if compact clients exist
                    let has_compact_clients = self
                        .clients
                        .values()
                        .any(|c| c.view_mode == ClientViewMode::Compact);

                    if has_compact_clients {
                        let (cc, cr) = self.dims_for_mode(ClientViewMode::Compact);
                        if cc > 0 && cr > 0 && (cc != self.compact_cols || cr != self.compact_rows)
                        {
                            self.compact_cols = cc;
                            self.compact_rows = cr;
                            // Resize supervisor PTY to fit compact layout if no full clients
                            // are connected (phone is the only viewer)
                            if !has_full_clients {
                                let sup_rows = cr.saturating_sub(1); // 1 for status bar
                                let sup_cols = cc;
                                let sup_name = self.app.supervisor_name().to_string();
                                if let Some(pane) = self.app.mux.get_mut(&sup_name) {
                                    let _ = pane.resize(sup_rows, sup_cols);
                                }
                            }
                            // Rebuild compact terminal
                            let backend = BufferBackend::with_hyperlinks(
                                cc,
                                cr,
                                self.app.compact_pane_hyperlink_map(),
                            );
                            self.compact_terminal = Some(Terminal::new(backend)?);
                        }
                    }

                    // Snapshot TUI pane sizes and reconcile with GUI/web
                    // constraints (smallest client wins per pane).
                    self.snapshot_tui_pane_sizes_and_reconcile();

                    // Rebuild pane ring buffers from the virtual terminal's
                    // current state — after resize the vt reflows content to
                    // the new dimensions, so we snapshot that and re-encode
                    // as ANSI bytes. This preserves history for web viewers.
                    self.rebuild_pane_buffers_from_snapshots();

                    self.pending_resize = None;
                    resize_applied = true;
                }
            }

            // Check if full-mode clients need a full redraw
            let needs_full_redraw = self
                .clients
                .values()
                .any(|c| c.view_mode == ClientViewMode::Full && c.needs_full_redraw);
            if needs_full_redraw || resize_applied {
                // Resize the backend to match current dimensions
                terminal.backend_mut().resize(self.cols, self.rows);
                terminal.autoresize()?;
                // When a client needs a full redraw (new connection, buffer overflow),
                // reset the terminal's diff state so the next draw() produces a
                // complete frame. Without this, autoresize() is a no-op when dims
                // haven't changed, and draw() emits only a diff against the previous
                // frame—which the new client doesn't have (its screen is blank).
                if needs_full_redraw {
                    terminal.clear()?;
                }
                for client in self.clients.values_mut() {
                    if client.view_mode == ClientViewMode::Full {
                        client.needs_full_redraw = false;
                    }
                }
            }

            // Check if compact clients need a full redraw
            let needs_compact_redraw = self
                .clients
                .values()
                .any(|c| c.view_mode == ClientViewMode::Compact && c.needs_full_redraw);
            if needs_compact_redraw || resize_applied {
                if let Some(ref mut ct) = self.compact_terminal {
                    ct.backend_mut()
                        .resize(self.compact_cols, self.compact_rows);
                    ct.autoresize()?;
                    if needs_compact_redraw {
                        ct.clear()?;
                    }
                }
                for client in self.clients.values_mut() {
                    if client.view_mode == ClientViewMode::Compact {
                        client.needs_full_redraw = false;
                    }
                }
            }

            // Suppress rendering while a resize is pending (debouncing).
            // Rendering at the old size while the terminal is already a new size
            // produces visual garbage. Wait for the debounce to settle.
            let resize_pending = self.pending_resize.is_some();

            // Send periodic state updates to GUI and WS clients on refresh
            if refreshed && (!self.gui_clients.is_empty() || !self.ws_clients.is_empty()) {
                self.send_state_update();
            }

            let spawning = self.app.spawning_count > 0;
            let dirty = had_output
                || input_activity
                || gui_activity
                || ws_activity
                || refreshed
                || new_clients
                || new_gui_clients
                || new_ws_clients
                || needs_full_redraw
                || needs_compact_redraw
                || resize_applied
                || spawning
                || error_cleared_by_timeout;
            if dirty && !resize_pending {
                // Render full TUI for full-mode clients (and relay clients)
                let has_full_clients = self
                    .clients
                    .values()
                    .any(|c| c.view_mode == ClientViewMode::Full);
                let has_relay = self.has_relay_clients();
                if has_full_clients || has_relay {
                    terminal.draw(|f| self.app.render(f))?;
                    let output = terminal.backend_mut().take_buffer();
                    if !output.is_empty() {
                        if has_full_clients {
                            self.broadcast_output_to(&output, ClientViewMode::Full);
                        }
                        if has_relay {
                            self.broadcast_relay_output(&output);
                        }
                    }
                }

                // Render compact TUI for compact-mode clients
                let has_compact_clients = self
                    .clients
                    .values()
                    .any(|c| c.view_mode == ClientViewMode::Compact);
                if has_compact_clients {
                    if let Some(ref mut ct) = self.compact_terminal {
                        ct.draw(|f| self.app.render_compact(f))?;
                        let output = ct.backend_mut().take_buffer();
                        if !output.is_empty() {
                            self.broadcast_output_to(&output, ClientViewMode::Compact);
                        }
                    }
                }
            }
            // Flush any pending client output even if nothing new was rendered
            if !self.clients.is_empty() && self.clients.values().any(|c| !c.output_buf.is_empty()) {
                self.flush_client_output();
            }

            // Flush pending GUI client output
            if !self.gui_clients.is_empty()
                && self.gui_clients.values().any(|c| !c.write_buf.is_empty())
            {
                self.flush_gui_client_output();
            }

            // Flush pending WebSocket client output
            if !self.ws_clients.is_empty() {
                self.flush_ws_client_output().await;
            }

            // Adaptive sleep: ~120fps when active, ~60fps idle with clients,
            // ~2fps headless (no clients, no GUI) to minimize CPU usage.
            let has_any_client = !self.clients.is_empty()
                || !self.gui_clients.is_empty()
                || !self.ws_clients.is_empty()
                || self.has_relay_clients();
            let sleep_ms = if had_output && has_any_client {
                4
            } else if spawning && has_any_client {
                100 // Spinner updates every 100ms
            } else if has_any_client {
                8
            } else {
                500 // Headless: no rendering needed, sleep longer
            };
            let sleep_dur = Duration::from_millis(sleep_ms);
            if let Some(ref mut notify) = self.notify_rx {
                tokio::select! {
                    result = notify.recv() => {
                        if result.is_ok() {
                            prompt_notified = true;
                        }
                    }
                    _ = tokio::time::sleep(sleep_dur) => {}
                }
            } else {
                tokio::time::sleep(sleep_dur).await;
            }
        }

        // Stop recording if it was enabled
        if self.app.record_enabled() {
            if let Err(e) = self.app.stop_recording().await {
                tracing::error!("Failed to stop recording: {}", e);
            }

            // Upload recordings to cloud (best-effort, before disconnect)
            self.upload_recordings();
        }

        // Cleanup
        let cleanup_result = self.cleanup().await;

        let duration_secs = session_started_at.elapsed().as_secs().to_string();
        let final_workers = self.app.worker_names().len().to_string();
        crate::telemetry::track(
            "factory_session_ended",
            vec![
                ("mode", "daemon"),
                (
                    "status",
                    if cleanup_result.is_ok() {
                        "ok"
                    } else {
                        "error"
                    },
                ),
                ("duration_secs", &duration_secs),
                ("final_workers", &final_workers),
            ],
        );

        cleanup_result?;

        Ok(())
    }

    /// Cleanup on shutdown
    async fn cleanup(&mut self) -> anyhow::Result<()> {
        // Clean up notification socket
        if let Some(ref notify) = self.notify_rx {
            notify.cleanup();
        }

        // Clean up native Agent Teams directory
        if let Some(ref teams) = self.teams {
            teams.cleanup();
        }

        // Disconnect cloud phone-home client
        self.disconnect_cloud();

        // Kill all PTY process groups. Snapshot worker PGIDs first so their
        // durable ownership records can be removed only after death is
        // confirmed; any survivor stays visible to gc_report.
        let worker_process_groups: Vec<u32> = self
            .app
            .worker_names()
            .iter()
            .filter_map(|name| self.app.mux.pane_process_group_id(name))
            .collect();
        self.app.mux.kill_all();
        for pgid in worker_process_groups {
            self.app.untrack_worker_process_group_if_gone(pgid).await;
        }

        // Unregister all factory agents (supervisor + workers)
        if let Ok(agent_store) = open_agent_store(self.app.cas_dir()) {
            // Collect all agent names to unregister
            let mut names_to_unregister = vec![self.app.supervisor_name().to_string()];
            names_to_unregister.extend(self.app.worker_names().iter().cloned());

            // Query agent store directly instead of using cached director_data
            // to ensure we find all agents, including those registered after cache refresh
            if let Ok(all_agents) = agent_store.list(None) {
                for name in &names_to_unregister {
                    for agent in all_agents.iter().filter(|a| {
                        a.name == *name
                            && a.factory_session.as_deref() == Some(self.session_name.as_str())
                    }) {
                        if let Err(e) = agent_store.unregister(&agent.id) {
                            tracing::warn!("Failed to unregister agent {}: {}", name, e);
                        }
                    }
                }
            }
        }

        // Remove session metadata
        self.session_manager.remove_metadata(&self.session_name)?;

        // Clean up GUI socket
        let gui_sock = gui_socket_path(&self.session_name);
        let _ = std::fs::remove_file(&gui_sock);

        // Drop WebSocket clients (connections will close on drop)
        self.ws_clients.clear();

        // Send leave alternate screen to all clients
        let cleanup = b"\x1b[?25h\x1b[?1049l";
        for client in self.clients.values_mut() {
            let _ = client.stream.write_all(cleanup);
        }

        Ok(())
    }
}
