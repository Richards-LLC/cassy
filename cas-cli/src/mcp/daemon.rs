//! Embedded daemon for MCP server
//!
//! Runs maintenance tasks in the background while the MCP server is active.
//! Includes idle detection to avoid running during active conversations.
//! Also handles cloud sync when user is logged in.
//!
//! # Architecture
//!
//! The daemon types (EmbeddedDaemonStatus, ActivityTracker, EmbeddedDaemonConfig,
//! MaintenanceResult) are defined in `cas-mcp` for cross-crate sharing.
//! This module provides the implementation that depends on CLI-specific modules.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::DateTime;
use chrono::Utc;
use tokio::sync::RwLock;

// Import types from cas-mcp
pub use cas_mcp::{ActivityTracker, EmbeddedDaemonConfig, EmbeddedDaemonStatus};

use crate::cloud::{
    CloudConfig, CloudCoordinator, CloudSyncer, CloudSyncerConfig, SyncQueue, SyncResult,
    maybe_mark_personal_scope_notice,
};
use crate::daemon::{CodeWatcher, DaemonConfig, DaemonRunResult, WatcherConfig};
use crate::error::CasError;
use crate::mcp::socket::{self, DaemonEvent, DaemonResponse};
use crate::orchestration::names as friendly_names;
use crate::store::open_agent_store;
use crate::store::{
    SqliteStore, open_commit_link_store, open_event_store, open_file_change_store,
    open_prompt_store, open_rule_store_local, open_skill_store_local, open_spec_store,
    open_store_local, open_task_store_local,
};
use crate::types::{Agent, AgentRole};

pub(crate) fn apply_factory_worker_metadata(agent: &mut Agent, clone_path: Option<&str>) {
    if let Some(path) = clone_path {
        agent
            .metadata
            .insert("clone_path".to_string(), path.to_string());
    } else if let Ok(path) = std::env::var("CAS_CLONE_PATH") {
        agent.metadata.insert("clone_path".to_string(), path);
    }

    let is_worker = agent.role == AgentRole::Worker
        || std::env::var("CAS_AGENT_ROLE")
            .map(|role| role.eq_ignore_ascii_case("worker"))
            .unwrap_or(false);
    if !is_worker {
        return;
    }

    if let Ok(model) = std::env::var("CAS_FACTORY_WORKER_MODEL") {
        agent.metadata.insert("worker_model".to_string(), model);
    }
    if let Ok(effort) = std::env::var("CAS_FACTORY_WORKER_EFFORT") {
        agent.metadata.insert("worker_effort".to_string(), effort);
    }
    // cas-058f (EPIC cas-8888 Phase 4): persist which harness this worker
    // runs so a separate process (`cas factory is-wedged`/`kill`, which only
    // has the agent-store row to go on) can pick the right transcript
    // resolver. Same env var `worker_harness_from_env` (harness_policy.rs)
    // reads in-process for tool-prefix selection — this just also writes it
    // to the durable agent record, mirroring the worker_model/worker_effort
    // pattern above.
    if let Ok(cli) = std::env::var("CAS_FACTORY_WORKER_CLI") {
        agent.metadata.insert("worker_cli".to_string(), cli);
    }
}

/// Extension trait for EmbeddedDaemonConfig to convert to DaemonConfig
///
/// This is CLI-specific since DaemonConfig is defined in cas-cli.
pub trait EmbeddedDaemonConfigExt {
    /// Convert to standard DaemonConfig for running maintenance
    fn to_daemon_config(&self) -> DaemonConfig;
}

impl EmbeddedDaemonConfigExt for EmbeddedDaemonConfig {
    fn to_daemon_config(&self) -> DaemonConfig {
        DaemonConfig {
            cas_root: self.cas_root.clone(),
            interval_minutes: self.maintenance_interval_secs / 60,
            min_idle_minutes: self.min_idle_secs / 60,
            batch_size: self.batch_size,
            process_observations: self.process_observations,
            consolidate_memories: false, // Don't run AI consolidation in background
            auto_prune: false,           // Safe default
            apply_decay: self.apply_decay,
            model: "haiku".to_string(),
            update_entity_summaries: false, // Disable for MCP embedded daemon
            // Code indexing - pass through from config
            index_code: self.index_code,
            code_watch_paths: self.code_watch_paths.clone(),
            code_index_interval_secs: self.code_index_interval_secs,
            agent_purge_age_hours: 24, // Delete stale agents after 24 hours
            // BM25 indexing
            index_bm25: true,
            index_batch_size: 32,
            index_max_per_run: 200,
            index_interval_secs: 120, // 2 minutes
        }
    }
}

/// Background daemon runner for the MCP server
///
/// This struct orchestrates background maintenance tasks while the MCP server
/// is running. It uses types from `cas-mcp` but contains CLI-specific logic
/// for running maintenance, cloud sync, and embedding generation.
pub struct EmbeddedDaemon {
    config: EmbeddedDaemonConfig,
    activity: Arc<ActivityTracker>,
    status: Arc<RwLock<EmbeddedDaemonStatus>>,
    shutdown: Arc<AtomicBool>,
    /// Cloud syncer (if user is logged in)
    cloud_syncer: Option<Arc<CloudSyncer>>,
    /// Cloud coordinator for real-time agent registration/heartbeat
    cloud_coordinator: RwLock<Option<CloudCoordinator>>,
    /// Code watcher (if code indexing is enabled)
    code_watcher: Option<Arc<std::sync::Mutex<CodeWatcher>>>,
    /// MCP proxy engine for hot-reload (set after server startup)
    #[cfg(feature = "mcp-proxy")]
    proxy: RwLock<Option<Arc<cmcp_core::ProxyEngine>>>,
    /// Last known mtime of .cas/proxy.toml for change detection
    #[cfg(feature = "mcp-proxy")]
    proxy_config_mtime: std::sync::Mutex<Option<std::time::SystemTime>>,
    /// Agent ID for heartbeat (set after registration)
    agent_id: RwLock<Option<String>>,
    /// PID → session ID mapping for hooks to look up their session
    /// Key is Claude Code's PID, value is the session ID
    pid_sessions: RwLock<std::collections::HashMap<u32, String>>,
}

impl EmbeddedDaemon {
    /// Create a new embedded daemon
    pub fn new(config: EmbeddedDaemonConfig) -> Self {
        let activity = Arc::new(ActivityTracker::new(config.min_idle_secs));

        // Initialize cloud syncer if logged in and enabled
        let cloud_syncer = if config.cloud_sync_enabled {
            init_cloud_syncer(&config.cas_root)
        } else {
            None
        };

        // Initialize cloud coordinator for real-time agent registration
        let cloud_coordinator = if config.cloud_sync_enabled {
            init_cloud_coordinator(&config.cas_root)
        } else {
            None
        };

        // Initialize code watcher if code indexing is enabled
        let code_watcher = if config.index_code {
            init_code_watcher(&config)
        } else {
            None
        };

        Self {
            config,
            activity,
            status: Arc::new(RwLock::new(EmbeddedDaemonStatus::default())),
            shutdown: Arc::new(AtomicBool::new(false)),
            cloud_syncer,
            cloud_coordinator: RwLock::new(cloud_coordinator),
            code_watcher,
            #[cfg(feature = "mcp-proxy")]
            proxy: RwLock::new(None),
            #[cfg(feature = "mcp-proxy")]
            proxy_config_mtime: std::sync::Mutex::new(None),
            agent_id: RwLock::new(None),
            pid_sessions: RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Set the agent ID for heartbeat tracking
    pub async fn set_agent_id(&self, id: String) {
        let mut agent_id = self.agent_id.write().await;
        *agent_id = Some(id);
    }

    /// Set the proxy engine for hot-reload watching
    #[cfg(feature = "mcp-proxy")]
    pub async fn set_proxy(&self, proxy: Arc<cmcp_core::ProxyEngine>) {
        // Record initial mtime so we don't reload on first check
        let proxy_path = self.config.cas_root.join("proxy.toml");
        if let Ok(metadata) = std::fs::metadata(&proxy_path) {
            if let Ok(mtime) = metadata.modified() {
                if let Ok(mut guard) = self.proxy_config_mtime.lock() {
                    *guard = Some(mtime);
                }
            }
        }
        let mut proxy_guard = self.proxy.write().await;
        *proxy_guard = Some(proxy);
    }

    /// Check if proxy.toml has changed since last check, reload if so
    #[cfg(feature = "mcp-proxy")]
    async fn check_proxy_config_reload(&self) {
        let proxy_path = self.config.cas_root.join("proxy.toml");
        let proxy = {
            let proxy_guard = self.proxy.read().await;
            proxy_guard.as_ref().cloned()
        };
        let Some(proxy) = proxy else {
            return;
        };

        if proxy.retry_unhealthy().await > 0 {
            crate::mcp::server::write_proxy_catalog_cache(&self.config.cas_root, &proxy).await;
            crate::mcp::server::write_proxy_health_cache(&self.config.cas_root, &proxy).await;
        }

        // Check mtime
        let new_mtime = match std::fs::metadata(&proxy_path) {
            Ok(m) => m.modified().ok(),
            Err(_) => None,
        };

        let changed = {
            match self.proxy_config_mtime.lock().ok() {
                Some(guard) => *guard != new_mtime,
                None => false,
            }
        };

        if !changed {
            return;
        }

        // Update stored mtime
        if let Ok(mut guard) = self.proxy_config_mtime.lock() {
            *guard = new_mtime;
        }

        eprintln!("[CAS] Proxy config changed, reloading...");

        let cfg = cmcp_core::config::Config::load_merged(if proxy_path.exists() {
            Some(&proxy_path)
        } else {
            None
        });

        match cfg {
            Ok(cfg) => {
                let server_count = cfg.servers.len();
                match proxy.reload(cfg.servers).await {
                    Ok(()) => {
                        let tool_count = proxy.tool_count().await;
                        eprintln!(
                            "[CAS] Proxy reloaded ({server_count} server(s), {tool_count} tools)"
                        );
                        crate::mcp::server::write_proxy_catalog_cache(
                            &self.config.cas_root,
                            &proxy,
                        )
                        .await;
                        crate::mcp::server::write_proxy_health_cache(&self.config.cas_root, &proxy)
                            .await;
                    }
                    Err(e) => {
                        eprintln!("[CAS] Proxy reload failed: {e}");
                    }
                }
            }
            Err(e) => {
                eprintln!("[CAS] Failed to load proxy config: {e}");
            }
        }
    }

    /// Get the activity tracker for use by the MCP service
    pub fn activity_tracker(&self) -> Arc<ActivityTracker> {
        Arc::clone(&self.activity)
    }

    /// Get current status
    pub async fn status(&self) -> EmbeddedDaemonStatus {
        let mut status = self.status.read().await.clone();
        status.idle_seconds = self.activity.idle_seconds();
        status.is_idle = self.activity.is_idle();

        // Update cloud sync status
        {
            status.cloud_sync_available = self.cloud_syncer.is_some();
            if let Some(syncer) = &self.cloud_syncer {
                status.cloud_sync_pending = syncer.queue().queue_depth().unwrap_or(0);
            }
        }

        status
    }

    /// Signal shutdown
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    /// Run the background daemon loop using proper Tokio intervals
    pub async fn run(self: Arc<Self>) -> Result<(), CasError> {
        use tokio::sync::watch;

        // Create shutdown channel
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        // Store shutdown sender for external shutdown signals
        let shutdown_flag = Arc::clone(&self.shutdown);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                if shutdown_flag.load(Ordering::SeqCst) {
                    let _ = shutdown_tx.send(true);
                    break;
                }
            }
        });

        // Mark as running
        {
            let mut status = self.status.write().await;
            status.running = true;
            {
                status.cloud_sync_available = self.cloud_syncer.is_some();
            }
            status.next_maintenance = Some(
                Utc::now()
                    + chrono::Duration::seconds(self.config.maintenance_interval_secs as i64),
            );
        }

        // Register daemon instance for statusline tracking (DB-based, not PID file)
        let daemon_id = format!("daemon-{:08x}", std::process::id());
        if let Ok(store) = open_agent_store(&self.config.cas_root) {
            let _ = store.register_daemon(&daemon_id, "mcp_embedded");
        }

        // Create Unix socket for hook communication
        let socket_listener = match socket::create_listener(&self.config.cas_root) {
            Ok(listener) => {
                eprintln!(
                    "[CAS] Daemon socket listening at {:?}",
                    socket::socket_path(&self.config.cas_root)
                );
                Some(listener)
            }
            Err(e) => {
                eprintln!("[CAS] Warning: Could not create daemon socket: {e}");
                None
            }
        };

        // Spawn socket listener as a separate task so it's never blocked by
        // maintenance/sync/indexing handlers in the select loop below
        let _socket_task = if let Some(listener) = socket_listener {
            let daemon = Arc::clone(&self);
            Some(tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok((mut stream, _)) => {
                            let daemon = Arc::clone(&daemon);
                            tokio::spawn(async move {
                                if let Some(event) = socket::read_event(&mut stream).await {
                                    let response = daemon.handle_socket_event(event).await;
                                    let _ = socket::send_response(&mut stream, &response).await;
                                }
                            });
                        }
                        Err(e) => {
                            eprintln!("[CAS] Socket accept error: {e}");
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
            }))
        } else {
            None
        };

        // Create interval timers
        let mut cloud_sync_interval =
            tokio::time::interval(Duration::from_secs(self.config.cloud_sync_interval_secs));
        let mut maintenance_interval =
            tokio::time::interval(Duration::from_secs(self.config.maintenance_interval_secs));
        let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(30)); // Agent heartbeat every 30s
        let mut code_index_interval =
            tokio::time::interval(Duration::from_secs(self.config.code_index_interval_secs));
        // Proxy config hot-reload interval (no-op when mcp-proxy feature is disabled)
        let proxy_config_secs = if cfg!(feature = "mcp-proxy") {
            3
        } else {
            86400
        };
        let mut proxy_config_interval =
            tokio::time::interval(Duration::from_secs(proxy_config_secs));

        // Skip the first immediate tick for maintenance tasks
        cloud_sync_interval.tick().await;
        maintenance_interval.tick().await;
        heartbeat_interval.tick().await;
        code_index_interval.tick().await;
        proxy_config_interval.tick().await;

        // Check if agent was already registered directly (fallback path in SessionStart hook)
        // This happens when the hook runs before the daemon socket exists
        // The agent's PID is Claude Code's PID (our parent), not the MCP server's PID
        #[cfg(unix)]
        let cc_pid = std::os::unix::process::parent_id();
        #[cfg(not(unix))]
        let cc_pid = std::process::id();

        if let Ok(store) = open_agent_store(&self.config.cas_root) {
            if let Ok(Some(agent)) = store.get_by_pid(cc_pid) {
                eprintln!(
                    "[CAS] Adopting pre-registered agent: {} (registered via fallback)",
                    agent.id
                );
                // Populate pid_sessions so GetSession queries work
                {
                    let mut pid_sessions = self.pid_sessions.write().await;
                    pid_sessions.insert(cc_pid, agent.id.clone());
                }
                self.set_agent_id(agent.id).await;
            }
        }

        // Initial cloud sync: push any stale items from previous sessions, then pull
        if self.cloud_syncer.is_some() {
            eprintln!("[CAS] Running initial cloud sync (push stale + pull)...");
            match self.run_cloud_sync().await {
                Ok(result) => {
                    let pushed = result.total_pushed();
                    let pulled = result.total_pulled();
                    if pushed > 0 || pulled > 0 {
                        eprintln!(
                            "[CAS] Initial cloud sync complete: {pushed} pushed, {pulled} pulled"
                        );
                    }
                    let mut status = self.status.write().await;
                    status.cloud_items_pushed += pushed;
                    status.cloud_items_pulled += pulled;
                    status.last_cloud_sync = Some(Utc::now());
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Initial cloud sync failed — will retry on next interval");
                }
            }
        }

        loop {
            tokio::select! {
                // Shutdown signal
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }

                // Cloud sync - runs after short idle (lighter threshold than maintenance)
                _ = cloud_sync_interval.tick() => {
                    if self.cloud_syncer.is_some() && self.activity.idle_seconds() >= self.config.cloud_sync_idle_secs {
                        match self.run_cloud_sync().await {
                            Ok(result) => {
                                let mut status = self.status.write().await;
                                status.cloud_items_pushed += result.total_pushed();
                                status.cloud_items_pulled += result.total_pulled();
                                status.last_cloud_sync = Some(Utc::now());
                                if result.has_errors() {
                                    status.last_error = result.errors.first().cloned();
                                }
                            }
                            Err(e) => {
                                let mut status = self.status.write().await;
                                status.last_error = Some(format!("Cloud sync failed: {e}"));
                            }
                        }
                    }
                }

                // Full maintenance - only when idle
                _ = maintenance_interval.tick() => {
                    if self.activity.is_idle() {
                        if let Err(e) = self.run_maintenance().await {
                            let mut status = self.status.write().await;
                            status.last_error = Some(format!("Maintenance failed: {e}"));
                        }

                        // Update next maintenance time
                        let mut status = self.status.write().await;
                        status.next_maintenance = Some(
                            Utc::now()
                                + chrono::Duration::seconds(self.config.maintenance_interval_secs as i64),
                        );
                    }
                }

                // Agent heartbeat - keep agent alive
                _ = heartbeat_interval.tick() => {
                    self.send_agent_heartbeat().await;
                }

                // Code indexing - runs when idle and code watcher has pending files
                _ = code_index_interval.tick() => {
                    if self.code_watcher.is_some() && self.activity.is_idle() {
                        if let Err(e) = self.run_code_index_cycle().await {
                            let mut status = self.status.write().await;
                            status.last_error = Some(format!("Code indexing failed: {e}"));
                        }
                    }
                }

                // Proxy config hot-reload - check .cas/proxy.toml for changes
                _ = proxy_config_interval.tick() => {
                    #[cfg(feature = "mcp-proxy")]
                    self.check_proxy_config_reload().await;
                }
            }
        }

        // Final cloud sync: drain any pending items before shutdown
        if self.cloud_syncer.is_some() {
            eprintln!("[CAS] Running final cloud sync before shutdown...");
            match tokio::time::timeout(Duration::from_secs(10), self.run_cloud_sync()).await {
                Ok(Ok(result)) => {
                    let pushed = result.total_pushed();
                    let pulled = result.total_pulled();
                    if pushed > 0 || pulled > 0 {
                        eprintln!(
                            "[CAS] Final cloud sync complete: {pushed} pushed, {pulled} pulled"
                        );
                    } else {
                        eprintln!("[CAS] Final cloud sync complete (nothing pending)");
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "Final cloud sync failed — items may sync next startup");
                }
                Err(_) => {
                    tracing::warn!(
                        "Final cloud sync timed out after 10s — items may sync next startup"
                    );
                }
            }
        }

        // Abort socket listener task
        if let Some(task) = _socket_task {
            task.abort();
        }

        // Mark as stopped and unregister daemon
        {
            let mut status = self.status.write().await;
            status.running = false;
        }

        // Unregister daemon instance
        if let Ok(store) = open_agent_store(&self.config.cas_root) {
            let _ = store.unregister_daemon(&daemon_id);
        }

        // Cleanup socket
        socket::cleanup_socket(&self.config.cas_root);

        Ok(())
    }

    /// Run code indexing cycle
    async fn run_code_index_cycle(&self) -> Result<(), CasError> {
        let watcher = match &self.code_watcher {
            Some(w) => Arc::clone(w),
            None => return Ok(()),
        };
        let cas_root = self.config.cas_root.clone();

        // Run in blocking task since it's CPU intensive
        let result = tokio::task::spawn_blocking(move || {
            let watcher_guard = watcher
                .lock()
                .map_err(|e| CasError::Other(format!("Watcher lock error: {e}")))?;
            crate::daemon::run_code_index_cycle(&watcher_guard, &cas_root)
        })
        .await
        .map_err(|e| CasError::Other(format!("Task join error: {e}")))??;

        if result.files_indexed > 0 || result.files_deleted > 0 {
            eprintln!(
                "[CAS] Code indexing: {} indexed, {} deleted, {} symbols",
                result.files_indexed, result.files_deleted, result.symbols_indexed,
            );
        }

        Ok(())
    }

    /// Run full maintenance cycle
    async fn run_maintenance(&self) -> Result<DaemonRunResult, CasError> {
        let daemon_config = self.config.to_daemon_config();
        let cas_root = self.config.cas_root.clone();

        // Run in blocking task
        let mut result =
            tokio::task::spawn_blocking(move || crate::daemon::run_maintenance(&daemon_config))
                .await
                .map_err(|e| CasError::Other(format!("Task join error: {e}")))??;

        // Prune old failed items from sync queue (7 days, max 5 retries)
        if let Ok(queue) = SyncQueue::open(&cas_root) {
            if queue.init().is_ok() {
                if let Err(e) = queue.prune_failed(7, 5) {
                    let msg = format!("Failed to prune failed sync queue items: {e}");
                    eprintln!("[CAS] {msg}");
                    result.errors.push(msg);
                }
            }
        }

        // Agent cleanup: mark stale agents dead, reclaim expired leases,
        // and (cas-2e81) park orphaned InProgress tasks + emit worker_died.
        if let Ok(agent_store) = open_agent_store(&cas_root) {
            // Mark agents with no heartbeat in 600s (10 min) as stale
            // This is only for crash detection - normal cleanup via SessionEnd hook
            if let Ok(stale_agents) = agent_store.list_stale(600) {
                for agent in stale_agents {
                    let held: Vec<String> = agent_store
                        .list_agent_leases(&agent.id)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|l| l.task_id)
                        .collect();
                    if let Err(e) = agent_store.mark_stale(&agent.id) {
                        let msg = format!("Failed to mark stale agent {}: {}", agent.id, e);
                        eprintln!("[CAS] {msg}");
                        result.errors.push(msg);
                    } else {
                        let _ = crate::mcp::tools::service::orphan_recovery::recover_worker_vanished(
                            &cas_root,
                            agent_store.as_ref(),
                            &agent,
                            &held,
                            "embedded daemon maintenance: heartbeat stale",
                        );
                    }
                }
            }
            // Reclaim expired leases (+ orphan recovery for dead holders)
            let expired: Vec<(String, String)> = agent_store
                .list_active_leases()
                .unwrap_or_default()
                .into_iter()
                .filter(|l| l.is_expired())
                .map(|l| (l.task_id, l.agent_id))
                .collect();
            if let Err(e) = agent_store.reclaim_expired_leases() {
                let msg = format!("Failed to reclaim expired leases: {e}");
                eprintln!("[CAS] {msg}");
                result.errors.push(msg);
            } else if !expired.is_empty() {
                let _ = crate::mcp::tools::service::orphan_recovery::recover_expired_leases_for_dead_holders(
                    &cas_root,
                    agent_store.as_ref(),
                    &expired,
                    600,
                );
            }
        }

        // Update status
        let mut status = self.status.write().await;
        status.last_maintenance = Some(Utc::now());
        status.observations_processed += result.observations_processed;
        status.decay_applied += result.decay_applied;

        if let Some(err) = result.errors.first() {
            status.last_error = Some(err.clone());
        } else {
            status.last_error = None;
        }

        Ok(result)
    }

    /// Trigger immediate maintenance (ignores idle check)
    pub async fn trigger_maintenance(&self) -> Result<DaemonRunResult, CasError> {
        self.run_maintenance().await
    }

    /// Handle events from hooks via Unix socket
    async fn handle_socket_event(&self, event: DaemonEvent) -> DaemonResponse {
        match event {
            DaemonEvent::SessionStart {
                session_id,
                agent_name,
                agent_role,
                cc_pid,
                clone_path,
            } => {
                eprintln!(
                    "[CAS] Socket: SessionStart for {} (name: {:?}, role: {:?}, pid: {})",
                    &session_id[..8.min(session_id.len())],
                    agent_name,
                    agent_role,
                    cc_pid
                );
                // Store PID → session mapping
                {
                    let mut pid_sessions = self.pid_sessions.write().await;
                    pid_sessions.insert(cc_pid, session_id.clone());
                }
                // Register agent immediately with name, role, and PID from hook's environment
                self.register_agent(session_id, agent_name, agent_role, cc_pid, clone_path)
                    .await;
                DaemonResponse::Ok
            }
            DaemonEvent::SessionEnd { session_id, cc_pid } => {
                eprintln!(
                    "[CAS] Socket: SessionEnd for {}",
                    &session_id[..8.min(session_id.len())]
                );
                // Remove PID → session mapping
                if let Some(pid) = cc_pid {
                    let mut pid_sessions = self.pid_sessions.write().await;
                    pid_sessions.remove(&pid);
                }
                // Clear cached agent_id if it matches
                let mut guard = self.agent_id.write().await;
                if guard.as_ref() == Some(&session_id) {
                    *guard = None;
                }
                DaemonResponse::Ok
            }
            DaemonEvent::GetSession { cc_pid } => {
                let pid_sessions = self.pid_sessions.read().await;
                match pid_sessions.get(&cc_pid) {
                    Some(session_id) => DaemonResponse::Session {
                        session_id: session_id.clone(),
                    },
                    None => DaemonResponse::NoSession,
                }
            }
            DaemonEvent::Ping => DaemonResponse::Pong,
            DaemonEvent::WorkerActivity {
                session_id,
                event_type,
                description,
                entity_id,
            } => {
                // Store worker activity in EventStore for Activity tab visibility
                use cas_store::{EventStore, SqliteEventStore};
                use cas_types::{Event, EventEntityType, EventType as CasEventType};

                if let Ok(event_store) = SqliteEventStore::open(&self.config.cas_root) {
                    // Map string event_type to EventType enum
                    let cas_event_type = match event_type.as_str() {
                        "worker_subagent_spawned" => CasEventType::WorkerSubagentSpawned,
                        "worker_subagent_completed" => CasEventType::WorkerSubagentCompleted,
                        "worker_file_edited" => CasEventType::WorkerFileEdited,
                        "worker_git_commit" => CasEventType::WorkerGitCommit,
                        "worker_verification_blocked" => CasEventType::WorkerVerificationBlocked,
                        "verification_started" => CasEventType::VerificationStarted,
                        "verification_added" => CasEventType::VerificationAdded,
                        "epic_subtasks_complete" => CasEventType::EpicSubtasksComplete,
                        "audit_trail_gap" => CasEventType::AuditTrailGap,
                        _ => CasEventType::WorkerSubagentSpawned, // Fallback
                    };

                    let event = Event::new(
                        cas_event_type,
                        EventEntityType::Agent,
                        entity_id.as_deref().unwrap_or(&session_id),
                        &description,
                    )
                    .with_session(&session_id);

                    let _ = event_store.record(&event);
                    eprintln!(
                        "[CAS] Worker activity: {} - {}",
                        &session_id[..8.min(session_id.len())],
                        description
                    );
                }
                DaemonResponse::Ok
            }
        }
    }

    /// Register an agent with the given session_id
    ///
    /// The agent_name is passed from the hook's environment (CAS_AGENT_NAME in Claude Code process).
    /// If not provided, falls back to generating a friendly name.
    ///
    /// The agent_role is passed from the hook's environment (CAS_AGENT_ROLE set by factory mode).
    /// The cc_pid is the Claude Code process's PID (the process that sent the event).
    async fn register_agent(
        &self,
        session_id: String,
        agent_name: Option<String>,
        agent_role: Option<String>,
        cc_pid: u32,
        clone_path: Option<String>,
    ) {
        // Determine if this registration belongs to OUR Claude Code instance.
        // In factory mode, all agents share .cas/daemon.sock — only the first
        // daemon to start owns the socket, so it receives SessionStart events
        // from ALL agents. We must only set self.agent_id for our own agent
        // (matching parent PID) to avoid one daemon stealing another's heartbeat.
        #[cfg(unix)]
        let our_cc_pid = std::os::unix::process::parent_id();
        #[cfg(not(unix))]
        let our_cc_pid = std::process::id();
        let is_our_agent = cc_pid == our_cc_pid;

        // Quick check: already registered with same ID
        if is_our_agent {
            let guard = self.agent_id.read().await;
            if guard.as_ref() == Some(&session_id) {
                return;
            }
        }

        // Register in database (always — even for other agents, so their
        // record exists for their own daemon to adopt via PID matching)
        if let Ok(store) = open_agent_store(&self.config.cas_root) {
            // Use name from hook's environment, fall back to generated name
            let name = agent_name.unwrap_or_else(friendly_names::generate);
            let mut agent = Agent::new(session_id.clone(), name);
            // Use the Claude Code process's PID, not the daemon's PID
            agent.pid = Some(cc_pid);
            // PID-reuse fingerprint (cas-ea46): pair `agent.pid` with the
            // /proc/<pid>/stat starttime so the heartbeat liveness gate can
            // detect kernel PID recycling. Missing fingerprint (non-Linux,
            // /proc hidden) falls back to pid-only liveness.
            stamp_pid_fingerprint(&mut agent, cc_pid);
            // PPID is less reliable from the hook, so we skip it for socket-registered agents
            agent.machine_id = Some(Agent::get_or_generate_machine_id());

            // Set role from the event (passed from hook's environment)
            if let Some(role_str) = agent_role {
                if let Ok(role) = role_str.parse::<AgentRole>() {
                    agent.role = role;
                }
            }

            apply_factory_worker_metadata(&mut agent, clone_path.as_deref());

            if store.register(&agent).is_ok() {
                eprintln!(
                    "[CAS] Daemon registered agent: {} (role: {}, ours: {})",
                    &session_id[..8.min(session_id.len())],
                    agent.role,
                    is_our_agent,
                );

                // Force an immediate heartbeat so the agent doesn't start the
                // stale countdown waiting for the next 30s daemon tick.
                if let Err(e) = store.heartbeat(&session_id) {
                    tracing::warn!(
                        agent_id = %&session_id[..8.min(session_id.len())],
                        error = %e,
                        "Immediate post-registration heartbeat failed"
                    );
                }

                // Register with cloud coordinator (best-effort)
                {
                    let mut coord_guard = self.cloud_coordinator.write().await;
                    if let Some(ref mut coord) = *coord_guard {
                        match coord.register(&agent) {
                            Ok(_) => {
                                eprintln!(
                                    "[CAS] Cloud registered agent: {}",
                                    &session_id[..8.min(session_id.len())]
                                );
                            }
                            Err(e) => {
                                eprintln!(
                                    "[CAS] Cloud agent registration failed (best-effort): {e}"
                                );
                            }
                        }
                    }
                }
            }
        }

        // Only adopt as our own agent if the PID matches our Claude Code parent.
        // Other agents' daemons will discover their agent via PID-based adoption
        // in the heartbeat loop.
        if is_our_agent {
            let mut guard = self.agent_id.write().await;
            *guard = Some(session_id);
        }
    }

    /// Send agent heartbeat to keep agent alive
    ///
    /// Agent registration is handled via Unix socket events from hooks.
    /// Heartbeat only sends keepalive for the registered agent.
    ///
    /// When agent_id is None (e.g. this daemon lost the socket race in factory
    /// mode), tries to adopt the agent by matching our Claude Code parent PID
    /// against agent records in the database.
    ///
    /// Retries up to 3 times with backoff on failure, since heartbeat
    /// failures under SQLite lock contention can cause workers to be
    /// incorrectly marked stale in multi-agent factory sessions.
    async fn send_agent_heartbeat(&self) {
        if let Ok(store) = open_agent_store(&self.config.cas_root) {
            // If we don't have an agent_id yet, try to adopt one by PID.
            // This handles the factory case where another daemon owns the
            // shared socket and received our SessionStart event — the agent
            // was registered in the DB but this daemon never got notified.
            if self.agent_id.read().await.is_none() {
                #[cfg(unix)]
                let our_cc_pid = std::os::unix::process::parent_id();
                #[cfg(not(unix))]
                let our_cc_pid = std::process::id();

                if let Ok(Some(agent)) = store.get_by_pid(our_cc_pid) {
                    eprintln!(
                        "[CAS] Adopted agent by PID match: {} (pid: {})",
                        &agent.id[..8.min(agent.id.len())],
                        our_cc_pid
                    );
                    // Populate pid_sessions so GetSession queries work
                    {
                        let mut pid_sessions = self.pid_sessions.write().await;
                        pid_sessions.insert(our_cc_pid, agent.id.clone());
                    }
                    self.set_agent_id(agent.id).await;
                }
            }

            // Send agent heartbeat if registered
            if let Some(id) = self.agent_id.read().await.clone() {
                // Liveness gate (EPIC cas-9508 / cas-2749): before heartbeating,
                // verify the Claude Code client process our agent record belongs
                // to is still alive. In factory mode a shared `cas serve` daemon
                // can outlive a crashed CC client (e.g. Bun/React-Ink unhandled
                // rejection keeps the event loop running while the UI is dead),
                // which previously kept the worker's last_heartbeat fresh
                // forever — supervisors saw "heartbeat: 13s ago" for zombie
                // workers and couldn't tell the worker had died.
                //
                // If the registered CC pid has exited (ESRCH), mark the agent
                // stale, clear our local agent_id, and skip heartbeat. Next
                // tick will no-op.
                if let Ok(agent) = store.get(&id) {
                    let short_id = &id[..8.min(id.len())];
                    match evaluate_liveness(&agent, pid_alive, pid_matches_fingerprint) {
                        LivenessOutcome::NoPidRecorded => {
                            // Legacy agents (pre-cas-2749) have no pid. warn! so
                            // ops investigators see the cohort drain; clears
                            // naturally as sessions cycle.
                            tracing::warn!(
                                agent_id = %short_id,
                                "Agent has no registered CC pid — liveness gate skipped \
                                 (cas-2749). Heartbeat continues; consider re-registering \
                                 the agent to activate the gate."
                            );
                        }
                        LivenessOutcome::Alive {
                            cc_pid,
                            fingerprint_checked: true,
                        } => {
                            // Strict pid+starttime check passed; gate cleared.
                            let _ = cc_pid;
                        }
                        LivenessOutcome::Alive {
                            cc_pid,
                            fingerprint_checked: false,
                        } => {
                            // Pre-cas-ea46 agent: pid is tracked but no starttime
                            // fingerprint stashed. warn! per supervisor feedback on
                            // cas-ea46 so ops investigators can see PID-reuse
                            // protection is NOT active for this agent.
                            tracing::warn!(
                                agent_id = %short_id,
                                cc_pid = cc_pid,
                                "Agent pid registered but no pid_starttime \
                                 fingerprint — falling back to pid-only liveness \
                                 (cas-ea46). Recycle the session to activate \
                                 PID-reuse protection."
                            );
                        }
                        LivenessOutcome::Dead {
                            cc_pid,
                            fingerprint_checked,
                        } => {
                            // cas-3e56: registered pid may be wrong (pre-fix
                            // SessionStart only matched "claude" and walked to
                            // grandparent for Grok). Before mark_stale, scan the
                            // live process table for this agent name — if the
                            // real harness is mid-turn, keep heartbeating.
                            let live_pid = crate::cli::factory::wedged::find_worker_pid(
                                &crate::cli::factory::wedged::RealProcessTable,
                                &agent.name,
                            );
                            if let Some(resolved) = live_pid {
                                tracing::warn!(
                                    agent_id = %short_id,
                                    registered_cc_pid = cc_pid,
                                    resolved_pid = resolved,
                                    fingerprint_checked = fingerprint_checked,
                                    "Registered harness PID dead/recycled but live \
                                     process found by agent name — continuing heartbeat \
                                     (cas-3e56)"
                                );
                                // Best-effort: re-stamp the correct pid so
                                // subsequent ticks use the strict path.
                                if let Ok(mut refreshed) = store.get(&id) {
                                    refreshed.pid = Some(resolved);
                                    stamp_pid_fingerprint(&mut refreshed, resolved);
                                    let _ = store.update(&refreshed);
                                }
                            } else {
                                tracing::info!(
                                    agent_id = %short_id,
                                    cc_pid = cc_pid,
                                    fingerprint_checked = fingerprint_checked,
                                    "Harness client process is gone (or PID recycled to \
                                     a different process) — marking agent stale and stopping \
                                     heartbeat (cas-2749/cas-ea46/cas-3e56 liveness gate)"
                                );
                                let _ = store.mark_stale(&id);
                                let mut guard = self.agent_id.write().await;
                                *guard = None;
                                return;
                            }
                        }
                    }
                }

                let mut succeeded = false;
                let mut terminal = false;
                for attempt in 0..3 {
                    match store.heartbeat(&id) {
                        Ok(()) => {
                            succeeded = true;
                            break;
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            // Agent was shut down or marked stale — stop heartbeating
                            if msg.contains("shutdown") || msg.contains("stale") {
                                tracing::info!(
                                    agent_id = %&id[..8.min(id.len())],
                                    "Agent is in terminal state, stopping heartbeat"
                                );
                                terminal = true;
                                break;
                            }
                            if attempt < 2 {
                                // Backoff: 100ms, 300ms
                                let delay = std::time::Duration::from_millis(
                                    100 * (1 + attempt as u64 * 2),
                                );
                                tokio::time::sleep(delay).await;
                            } else {
                                tracing::warn!(
                                    agent_id = %&id[..8.min(id.len())],
                                    error = %e,
                                    "Agent heartbeat failed after 3 attempts — \
                                     worker may be marked stale under DB contention"
                                );
                            }
                        }
                    }
                }
                if terminal {
                    // Clear agent_id so we stop heartbeating on future ticks
                    let mut guard = self.agent_id.write().await;
                    *guard = None;
                } else if !succeeded {
                    tracing::error!(
                        agent_id = %&id[..8.min(id.len())],
                        "All heartbeat retries exhausted"
                    );
                }

                // Send cloud heartbeat (best-effort, in blocking task to avoid stalling async loop)
                {
                    let coord_guard = self.cloud_coordinator.read().await;
                    if let Some(ref coord) = *coord_guard {
                        let coord_clone = coord.clone();
                        drop(tokio::task::spawn_blocking(move || {
                            let _ = coord_clone.heartbeat();
                        }));
                    }
                }
            }

            // Send daemon heartbeat (best-effort, not critical for worker liveness)
            let daemon_id = format!("daemon-{:08x}", std::process::id());
            if let Err(e) = store.daemon_heartbeat(&daemon_id) {
                tracing::debug!(error = %e, "Daemon heartbeat failed");
            }
        }
    }

    /// Run cloud sync cycle
    async fn run_cloud_sync(&self) -> Result<SyncResult, CasError> {
        let syncer = self
            .cloud_syncer
            .as_ref()
            .ok_or_else(|| CasError::Other("Cloud syncer not available".to_string()))?;

        let cas_root = self.config.cas_root.clone();
        let syncer = Arc::clone(syncer);

        // Run in blocking task (ureq is synchronous)
        tokio::task::spawn_blocking(move || {
            match maybe_mark_personal_scope_notice(&cas_root) {
                Ok(Some(notice)) => {
                    let msg = notice.message();
                    eprintln!("[CAS] {msg}");
                    tracing::info!(target: "cas::sync", team_id = %notice.team_id, team_slug = %notice.team_slug, "{}", msg);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        target: "cas::sync",
                        error = %e,
                        "could not persist personal-scope team availability notice"
                    );
                }
            }

            // cas-7fbb: open_*_local skips Syncing* / SyncQueue wrappers so
            // pull apply does not re-enqueue remote rows (which would feed
            // push → pull forever). Local edits still use open_store (etc.)
            // and correctly enqueue.
            let store = open_store_local(&cas_root)?;
            let task_store = open_task_store_local(&cas_root)?;
            let rule_store = open_rule_store_local(&cas_root)?;
            let skill_store = open_skill_store_local(&cas_root)?;
            // cas-bba4: extra stores for the extended pull surface (specs +
            // events + prompts + file_changes + commit_links). The auto-sync
            // path now imports the full content set just like `cas cloud pull`.
            // These kinds are not wrapped by Syncing* openers today.
            let spec_store = open_spec_store(&cas_root)?;
            let event_store = open_event_store(&cas_root)?;
            let prompt_store = open_prompt_store(&cas_root)?;
            let file_change_store = open_file_change_store(&cas_root)?;
            let commit_link_store = open_commit_link_store(&cas_root)?;

            // Get sessions to sync (sessions are stored directly, not queued)
            let sessions = get_sessions_for_sync(&cas_root, syncer.queue());

            syncer.sync_with_sessions(
                store.as_ref(),
                task_store.as_ref(),
                rule_store.as_ref(),
                skill_store.as_ref(),
                spec_store.as_ref(),
                event_store.as_ref(),
                prompt_store.as_ref(),
                file_change_store.as_ref(),
                commit_link_store.as_ref(),
                &sessions,
            )
        })
        .await
        .map_err(|e| CasError::Other(format!("Task join error: {e}")))?
    }

    /// Trigger immediate cloud sync (ignores idle check)
    pub async fn trigger_cloud_sync(&self) -> Result<SyncResult, CasError> {
        self.run_cloud_sync().await
    }
}

/// Check whether a PID corresponds to a live process.
///
/// Used by the agent heartbeat liveness gate (EPIC cas-9508 / cas-2749) so the
/// shared `cas serve` daemon stops faking fresh heartbeats for a Claude Code
/// client that has already died.
///
/// On Unix, sends signal 0 via `libc::kill` — returns false on ESRCH (no such
/// process). On non-Unix, falls back to `true` (best-effort; liveness gating
/// is only observed to matter on Linux factory hosts today).
#[cfg(unix)]
pub(crate) fn pid_alive(pid: u32) -> bool {
    // kill(pid, 0) performs the permission/existence check without delivering
    // a signal. errno == ESRCH (3) means the process is gone. EPERM means it
    // exists but we can't signal it — still alive, so return true.
    // Safety: `libc::kill` with signal 0 has no side effects on the target.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    errno != libc::ESRCH
}

#[cfg(not(unix))]
pub(crate) fn pid_alive(_pid: u32) -> bool {
    true
}

/// Metadata key used to stash a PID's /proc/<pid>/stat starttime fingerprint
/// (EPIC cas-9508 / cas-ea46). All writers and readers must use this constant
/// so a typo on one side cannot silently disable the liveness gate.
pub(crate) const PID_STARTTIME_KEY: &str = "pid_starttime";

/// Outcome of the daemon's agent-liveness evaluation (EPIC cas-9508 / cas-5b1c).
///
/// Extracted from the inline `if let` stack in `send_agent_heartbeat` so the
/// branch selection (fingerprint vs pid-only vs skip) can be unit-tested
/// without a live daemon, store, or tokio runtime. The caller is responsible
/// for the side effects (`tracing` + `store.mark_stale` + `self.agent_id`
/// clear) — the helper itself is pure data.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LivenessOutcome {
    /// Agent record has no CC pid. Legacy pre-cas-2749 cohort.
    /// Caller emits `tracing::warn!` and continues heartbeating.
    NoPidRecorded,
    /// CC client is alive. `fingerprint_checked=true` means the strict
    /// (pid+starttime) check verified the process; `false` means the
    /// caller fell back to pid-only liveness because no fingerprint
    /// was stashed at registration (pre-cas-ea46 cohort). The caller
    /// uses `cc_pid` to emit a diagnostic warn! on the pid-only path so
    /// operators can see PID-reuse protection is inactive for that agent.
    Alive {
        cc_pid: u32,
        fingerprint_checked: bool,
    },
    /// CC client is gone or PID was recycled to a different process. Caller
    /// marks agent stale, clears `self.agent_id`, and stops heartbeating.
    /// `fingerprint_checked=true` means the verdict came from the
    /// pid+starttime check; `false` means pid-only.
    Dead {
        cc_pid: u32,
        fingerprint_checked: bool,
    },
}

/// Evaluate the liveness of a Claude Code client from an agent record
/// (EPIC cas-9508 / cas-5b1c).
///
/// Selection logic:
/// - `agent.pid == None` → `NoPidRecorded` (legacy cas-2749 cohort).
/// - `agent.pid == Some(pid)` + fingerprint resolvable (see below):
///   strict (pid, starttime) check via `fingerprint_matches_fn`.
/// - `agent.pid == Some(pid)` + no/malformed fingerprint: pid-only liveness
///   via `pid_alive_fn` → `Alive { fingerprint_checked: false }` or `Dead` with
///   `fingerprint_checked=false`.
///
/// Fingerprint resolution order (cas-b157 typed promotion):
/// 1. `agent.pid_starttime: Option<u64>` — the first-class typed field.
///    Preferred because non-numeric writers cannot stomp it and a future
///    migration-forgot-to-backfill bug would surface as `None` rather
///    than as a parse-failure that silently disables the gate.
/// 2. `agent.metadata[PID_STARTTIME_KEY]` — legacy fallback, kept for
///    one release so agents registered on an older binary and revived
///    mid-flight still benefit from the strong check.
///
/// Both probe functions are injected so tests can drive the outcome matrix
/// without real syscalls — in production the caller passes `pid_alive` and
/// `pid_matches_fingerprint`.
pub(crate) fn evaluate_liveness(
    agent: &crate::types::Agent,
    pid_alive_fn: impl Fn(u32) -> bool,
    fingerprint_matches_fn: impl Fn(u32, u64) -> bool,
) -> LivenessOutcome {
    let Some(cc_pid) = agent.pid else {
        return LivenessOutcome::NoPidRecorded;
    };
    let expected_starttime = agent.pid_starttime.or_else(|| {
        // cas-b157 fallback: legacy agents registered pre-migration
        // still have their fingerprint in metadata. Drop this branch
        // after one release when the fleet has churned through the
        // shadow-write window.
        agent
            .metadata
            .get(PID_STARTTIME_KEY)
            .and_then(|s| s.parse::<u64>().ok())
    });
    match expected_starttime {
        Some(expected) => {
            if fingerprint_matches_fn(cc_pid, expected) {
                LivenessOutcome::Alive {
                    cc_pid,
                    fingerprint_checked: true,
                }
            } else {
                LivenessOutcome::Dead {
                    cc_pid,
                    fingerprint_checked: true,
                }
            }
        }
        None => {
            if pid_alive_fn(cc_pid) {
                LivenessOutcome::Alive {
                    cc_pid,
                    fingerprint_checked: false,
                }
            } else {
                LivenessOutcome::Dead {
                    cc_pid,
                    fingerprint_checked: false,
                }
            }
        }
    }
}

/// Stamp the (pid, starttime) fingerprint onto an Agent record for use by the
/// heartbeat liveness gate (EPIC cas-9508 / cas-ea46 + cas-b157 typed
/// promotion).
///
/// Call sites that set `agent.pid = Some(pid)` should call this helper
/// immediately after to keep the pair consistent. When `read_pid_starttime`
/// returns `None` (non-Linux, /proc hidden), nothing is written and the
/// liveness gate falls back to pid-only liveness for that agent — same as
/// legacy agents registered before this fix.
///
/// cas-b157: writes BOTH the typed `agent.pid_starttime` field AND the
/// legacy `metadata[PID_STARTTIME_KEY]` shadow entry for one release.
/// The typed field is the source of truth for the liveness gate; the
/// metadata shadow protects agents registered on an older binary that
/// get revived through the upgraded reader path. Drop the shadow after
/// fleet rollout confirms zero reliance on it.
pub(crate) fn stamp_pid_fingerprint(agent: &mut crate::types::Agent, pid: u32) {
    if let Some(starttime) = read_pid_starttime(pid) {
        agent.pid_starttime = Some(starttime);
        agent
            .metadata
            .insert(PID_STARTTIME_KEY.to_string(), starttime.to_string());
    }
}

/// Read the OS process-start timestamp used to fingerprint a PID
/// (EPIC cas-9508 / cas-ea46).
///
/// The Linux kernel recycles PIDs — `pid_max` defaults to 4_194_304 and a busy
/// factory host can wrap it within hours. `pid_alive(pid)` alone cannot tell
/// the difference between "our original Claude Code client is still running"
/// and "some unrelated process got recycled into that PID slot". The starttime
/// field is per-process and invariant for the lifetime of that process, so
/// pairing `(pid, starttime)` gives a collision-resistant fingerprint for the
/// liveness gate.
///
/// Returns `None` when the file cannot be read (process gone, /proc not
/// mounted, permission denied on a pid_ns/cgroup boundary) or when the parse
/// fails. See `parse_starttime_from_stat` for the parsing contract.
#[cfg(target_os = "linux")]
pub(crate) fn read_pid_starttime(pid: u32) -> Option<u64> {
    let path = format!("/proc/{pid}/stat");
    let raw = std::fs::read_to_string(&path).ok()?;
    parse_starttime_from_stat(&raw)
}

/// macOS exposes a stable `(seconds, microseconds)` start timestamp through
/// libproc's `PROC_PIDTBSDINFO`. Combining it into microseconds gives the same
/// equality-only fingerprint contract as Linux's clock-tick value.
#[cfg(target_os = "macos")]
pub(crate) fn read_pid_starttime(pid: u32) -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: proc_pidinfo initializes `size` bytes of the supplied
    // proc_bsdinfo buffer on success and does not retain the pointer.
    let written = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size as libc::c_int,
        )
    };
    if written != size as libc::c_int {
        return None;
    }
    // SAFETY: exact-size success above guarantees full initialization.
    let info = unsafe { info.assume_init() };
    info.pbi_start_tvsec
        .checked_mul(1_000_000)?
        .checked_add(info.pbi_start_tvusec)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn read_pid_starttime(_pid: u32) -> Option<u64> {
    // Platforms without a supported stable start-time API retain pid-only
    // agent liveness. Destructive process-group cleanup separately fails
    // closed when it cannot validate a durable fingerprint.
    None
}

/// Parse the `starttime` (field 22) out of a raw `/proc/<pid>/stat` line.
///
/// Extracted as a pure function so the parsing contract is testable without
/// a live PID (EPIC cas-9508 / cas-ea46, testing persona feedback).
///
/// Parsing note: field 2 is `comm` wrapped in parens and may itself contain
/// spaces and parens (e.g., `cc-wrapper (1)`). We split on the *last* `)` in
/// the file, not the first, before splitting the remainder on whitespace.
/// Field 22 is then index 19 of that remainder (fields 3–52 become indices
/// 0–49).
pub(crate) fn parse_starttime_from_stat(raw: &str) -> Option<u64> {
    let last_paren = raw.rfind(')')?;
    // Skip the `)` and the whitespace that follows it, then split the tail.
    let tail = raw.get(last_paren + 1..)?.trim_start();
    let fields: Vec<&str> = tail.split_whitespace().collect();
    // Field 22 = starttime; the tail begins at field 3, so index = 22 - 3 = 19.
    fields.get(19).and_then(|s| s.parse::<u64>().ok())
}

/// Verify `(pid, expected_starttime)` still identifies the *same* process that
/// was fingerprinted at agent registration (EPIC cas-9508 / cas-ea46).
///
/// Semantics — STRICT by design (adversarial review feedback):
/// - `pid` not alive → `false`.
/// - `pid` alive and starttime matches → `true`.
/// - `pid` alive but starttime differs OR /proc is unreadable → `false`.
///
/// Callers must only invoke this helper when they know a fingerprint was
/// previously stashed at registration (i.e., `agent.metadata` contains
/// `PID_STARTTIME_KEY`). If no fingerprint was stashed — the common case on
/// non-Linux or for legacy agents — the caller should bypass this helper and
/// use `pid_alive` directly. The strict None→false semantics exists so a
/// transient /proc read failure on a host where /proc *did* work at
/// registration is treated as suspicious, not silently trusted.
pub(crate) fn pid_matches_fingerprint(pid: u32, expected_starttime: u64) -> bool {
    if !pid_alive(pid) {
        return false;
    }
    matches!(read_pid_starttime(pid), Some(actual) if actual == expected_starttime)
}

/// Initialize cloud syncer if user is logged in
fn init_cloud_syncer(cas_root: &std::path::Path) -> Option<Arc<CloudSyncer>> {
    let cloud_config = CloudConfig::load_from_cas_dir(cas_root).ok()?;

    if !cloud_config.is_logged_in() {
        return None;
    }

    let queue = SyncQueue::open(cas_root).ok()?;
    let _ = queue.init();

    Some(Arc::new(CloudSyncer::new(
        Arc::new(queue),
        cloud_config,
        CloudSyncerConfig::default(),
    )))
}

fn init_cloud_coordinator(cas_root: &std::path::Path) -> Option<CloudCoordinator> {
    let cloud_config = CloudConfig::load_from_cas_dir(cas_root).ok()?;
    CloudCoordinator::new(cloud_config).ok()
}

/// Initialize code watcher if code indexing is enabled
fn init_code_watcher(config: &EmbeddedDaemonConfig) -> Option<Arc<std::sync::Mutex<CodeWatcher>>> {
    if !config.index_code {
        return None;
    }

    // Build watch paths - use configured paths or default to project root
    let watch_paths = if config.code_watch_paths.is_empty() {
        // Default: watch the project directory (parent of .cas)
        if let Some(project_root) = config.cas_root.parent() {
            vec![project_root.to_path_buf()]
        } else {
            return None;
        }
    } else {
        config.code_watch_paths.clone()
    };

    let watcher_config = WatcherConfig {
        watch_paths,
        extensions: config.code_extensions.clone(),
        debounce_ms: config.code_debounce_ms,
        ignore_patterns: config.code_exclude_patterns.clone(),
    };

    let mut watcher = CodeWatcher::new(watcher_config);

    // Start the watcher
    if let Err(e) = watcher.start() {
        eprintln!("[CAS] Failed to start code watcher: {e}");
        return None;
    }

    eprintln!("[CAS] Code watcher started");
    Some(Arc::new(std::sync::Mutex::new(watcher)))
}

/// Get sessions that need to be synced to cloud
fn get_sessions_for_sync(
    cas_root: &std::path::Path,
    queue: &SyncQueue,
) -> Vec<crate::types::Session> {
    // Get last session push timestamp from metadata
    let since = queue
        .get_metadata("last_session_push_at")
        .ok()
        .flatten()
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|| Utc::now() - chrono::Duration::days(30)); // Default: last 30 days

    // Open SqliteStore directly to access session-specific methods.
    // SqliteStore::open expects the CAS directory path, not cas.db.
    let sqlite_store = match SqliteStore::open(cas_root) {
        Ok(store) => store,
        Err(_) => return Vec::new(),
    };

    // Get sessions since last push
    sqlite_store.list_sessions_since(since).unwrap_or_default()
}

/// Spawn the embedded daemon as a background task
pub fn spawn_daemon(
    config: EmbeddedDaemonConfig,
) -> (Arc<EmbeddedDaemon>, tokio::task::JoinHandle<()>) {
    let daemon = Arc::new(EmbeddedDaemon::new(config));
    let daemon_clone = Arc::clone(&daemon);

    let handle = tokio::spawn(async move {
        if let Err(e) = daemon_clone.run().await {
            eprintln!("Embedded daemon error: {e}");
        }
    });

    (daemon, handle)
}

#[cfg(test)]
#[path = "daemon_tests/tests.rs"]
mod tests;
