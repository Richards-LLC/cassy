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
use crate::types::{Agent, AgentRole, AgentStatus, AgentType};

/// Longest the symbol index may go un-refreshed before the daemon stops waiting for an idle
/// window and indexes anyway (cas-499c).
///
/// The original design hard-gated code indexing on `ActivityTracker::is_idle()`. On a busy
/// factory daemon that window never arrives, so the job never fired and the symbol index stayed
/// empty on every install that ever existed. This ceiling turns idleness from a *precondition*
/// into a *preference*: quiet moments are still preferred, but politeness may only defer the
/// work, never cancel it.
pub(crate) const CODE_INDEX_MAX_STALENESS_SECS: u64 = 300;

/// Should the code-index cycle run on this tick?
///
/// Idle-preferred with a max-staleness override. Extracted from the `select!` arm so the policy
/// is testable without standing up a daemon.
pub(crate) fn should_run_code_index(is_idle: bool, stale_for: Duration) -> bool {
    is_idle || stale_for >= Duration::from_secs(CODE_INDEX_MAX_STALENESS_SECS)
}

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
    if let Ok(account_dir) = std::env::var("CAS_FACTORY_WORKER_ACCOUNT_DIR") {
        agent
            .metadata
            .insert("worker_account_dir".to_string(), account_dir);
    }
}

/// Parse the harness-provided durable role hint used by every registration
/// entrypoint. Verification authority is still bound independently to the
/// in-process [`AgentIdentitySource`](crate::mcp::server::AgentIdentitySource);
/// this helper only keeps the registered row aligned with the factory launch
/// contract so routing and session ownership can resolve it.
pub(crate) fn parse_agent_role_hint(role: Option<&str>) -> Option<AgentRole> {
    role.and_then(|value| value.trim().parse().ok())
}

fn apply_agent_role(agent: &mut Agent, role: AgentRole) {
    agent.role = role;
    match role {
        AgentRole::Worker => agent.agent_type = AgentType::Worker,
        AgentRole::Supervisor | AgentRole::Director => agent.agent_type = AgentType::Primary,
        AgentRole::Standard => {}
    }
}

/// Register while preserving the store's general anti-forgery contract, then
/// explicitly reconcile a role that came from the harness environment or a
/// typed server-internal caller. `AgentStore::register` deliberately preserves
/// role on conflict, so this narrow second step is required to repair rows
/// created as `standard` by an earlier registration path.
pub(crate) fn register_with_role_reconciliation(
    store: &dyn crate::store::AgentStore,
    agent: &Agent,
    configured_role: Option<AgentRole>,
) -> crate::store::Result<Agent> {
    store.register(agent)?;
    let mut persisted = store.get(&agent.id)?;
    if let Some(role) = configured_role
        && (persisted.role != role
            || (role == AgentRole::Worker && persisted.agent_type != AgentType::Worker)
            || (matches!(role, AgentRole::Supervisor | AgentRole::Director)
                && persisted.agent_type != AgentType::Primary))
    {
        apply_agent_role(&mut persisted, role);
        store.update(&persisted)?;
    }
    Ok(persisted)
}

/// Queue the factory daemon's proven teardown for a worker maintenance just declared dead.
pub(crate) fn queue_stale_factory_worker_shutdown(
    cas_root: &std::path::Path,
    agent: &Agent,
) -> Result<Option<i64>, String> {
    if agent.role != AgentRole::Worker {
        return Ok(None);
    }
    let Some(factory_session) = agent.factory_session.as_deref() else {
        return Ok(None);
    };
    let queue = crate::store::open_spawn_queue_store(cas_root)
        .map_err(|error| format!("open shutdown queue: {error}"))?;
    let already_queued = queue
        .peek(512)
        .map_err(|error| format!("inspect shutdown queue: {error}"))?
        .iter()
        .any(|request| {
            request.action == cas_store::SpawnAction::Shutdown
                && request.factory_session.as_deref() == Some(factory_session)
                && request.worker_names.iter().any(|name| name == &agent.name)
        });
    if already_queued {
        return Ok(None);
    }
    queue
        .enqueue_shutdown(None, std::slice::from_ref(&agent.name), true, Some(factory_session))
        .map(Some)
        .map_err(|error| format!("queue factory shutdown: {error}"))
}

/// Register the durable identity announced by a SessionStart hook.
///
/// Factory workers can receive a second SessionStart while the Claude Code
/// process itself is still running (notably around an urgent interrupt). The
/// eager MCP bootstrap may already have registered that same process under
/// `ppid`, while an earlier socket registration records it under `pid`.
/// Treating the newly supplied session id as authoritative in either case
/// mints a ghost agent row. Reuse only a same-name Worker attached to the
/// same live Claude Code PID; ordinary sessions and a genuinely new process
/// retain their supplied session id.
///
/// The returned boolean is true when an existing durable worker identity was
/// refreshed instead of creating a row for the incoming session id.
pub(crate) fn register_session_start_agent(
    store: &dyn crate::store::AgentStore,
    session_id: &str,
    agent_name: Option<&str>,
    agent_role: Option<&str>,
    cc_pid: u32,
    clone_path: Option<&str>,
) -> crate::store::Result<(Agent, bool)> {
    let configured_role = parse_agent_role_hint(agent_role);
    let requested_worker = configured_role == Some(AgentRole::Worker);
    let reusable = if requested_worker {
        agent_name.and_then(|name| {
            [
                store.get_by_pid(cc_pid).ok().flatten(),
                store.get_by_cc_pid(cc_pid).ok().flatten(),
            ]
            .into_iter()
            .flatten()
            .find(|existing| {
                if existing.id == session_id
                    || existing.name != name
                    || existing.role != AgentRole::Worker
                {
                    return false;
                }

                // Socket registration records the Claude Code PID directly.
                // Eager MCP registration records the MCP process and the
                // Claude Code PID as its parent. Both describe the same
                // durable worker identity, but the direct form can use its
                // stronger start-time fingerprint check.
                existing.pid == Some(cc_pid)
                    && matches!(
                        evaluate_liveness(existing, pid_alive, pid_matches_fingerprint),
                        LivenessOutcome::Alive { .. }
                    )
                    || existing.ppid == Some(cc_pid) && pid_alive(cc_pid)
            })
        })
    } else {
        None
    };

    let reused = reusable.is_some();
    let name = agent_name
        .map(str::to_owned)
        .unwrap_or_else(friendly_names::generate);
    let mut agent = reusable.unwrap_or_else(|| Agent::new(session_id.to_string(), name.clone()));
    agent.name = name;
    agent.status = AgentStatus::Active;
    agent.pid = Some(cc_pid);
    agent.ppid = None;
    stamp_pid_fingerprint(&mut agent, cc_pid);
    agent.machine_id = Some(Agent::get_or_generate_machine_id());

    if let Some(role) = configured_role {
        apply_agent_role(&mut agent, role);
    }
    apply_factory_worker_metadata(&mut agent, clone_path);

    let persisted = register_with_role_reconciliation(store, &agent, configured_role)?;
    Ok((persisted, reused))
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
            curated_importance_floor: self.curated_importance_floor,
            promote_on_access: self.promote_on_access,
            model: "haiku".to_string(),
            update_entity_summaries: false, // Disable for MCP embedded daemon
            // Code indexing - pass through from config
            index_code: self.index_code,
            code_watch_paths: self.code_watch_paths.clone(),
            code_index_interval_secs: self.code_index_interval_secs,
            agent_purge_age_hours: 24, // Delete stale agents after 24 hours
            archive_max_bytes: self.archive_max_bytes,
            archive_retention_days: self.archive_retention_days,
            // BM25 indexing
            index_bm25: true,
            index_batch_size: 32,
            index_max_per_run: 200,
            index_interval_secs: 120, // 2 minutes
            relevance_sampling_enabled: self.relevance_sampling_enabled,
            relevance_sampling_interval_secs: self.relevance_sampling_interval_secs,
            relevance_sampling_sample_size: self.relevance_sampling_sample_size,
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
    /// Last successful authoritative snapshot refresh. Healthy proxies still
    /// republish periodically so stopped servers cannot leave indefinitely
    /// fresh-looking evidence behind.
    #[cfg(feature = "mcp-proxy")]
    proxy_snapshot_refreshed_at: std::sync::Mutex<Option<std::time::Instant>>,
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
            #[cfg(feature = "mcp-proxy")]
            proxy_snapshot_refreshed_at: std::sync::Mutex::new(None),
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
        if let Ok(mut guard) = self.proxy_snapshot_refreshed_at.lock() {
            *guard = Some(std::time::Instant::now());
        }
        let mut proxy_guard = self.proxy.write().await;
        *proxy_guard = Some(proxy);
    }

    #[cfg(feature = "mcp-proxy")]
    fn proxy_snapshot_refresh_due(&self) -> bool {
        self.proxy_snapshot_refreshed_at
            .lock()
            .ok()
            .and_then(|guard| *guard)
            .is_none_or(|last| last.elapsed() >= std::time::Duration::from_secs(30))
    }

    #[cfg(feature = "mcp-proxy")]
    fn mark_proxy_snapshot_refreshed(&self) {
        if let Ok(mut guard) = self.proxy_snapshot_refreshed_at.lock() {
            *guard = Some(std::time::Instant::now());
        }
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
            let retried = proxy.retry_unhealthy().await > 0;
            if retried || self.proxy_snapshot_refresh_due() {
                match crate::mcp::server::write_proxy_snapshot_cache(&self.config.cas_root, &proxy)
                    .await
                {
                    Ok(()) => self.mark_proxy_snapshot_refreshed(),
                    Err(error) => {
                        eprintln!("[Cassy] Failed to publish MCP proxy state: {error}");
                    }
                }
            }
            return;
        }

        // Update stored mtime
        if let Ok(mut guard) = self.proxy_config_mtime.lock() {
            *guard = new_mtime;
        }

        eprintln!("[Cassy] Proxy config changed, reloading...");

        let cfg = cmcp_core::config::Config::load_merged(if proxy_path.exists() {
            Some(&proxy_path)
        } else {
            None
        });

        match cfg {
            Ok(cfg) => {
                let server_count = cfg.servers.len();
                let snapshot_config = cfg.clone();
                // Close the dispatch gate before mutating upstreams. Reload may
                // await connection setup, and a removed route must not remain
                // usable during that window. A failed reload deliberately
                // leaves the proxy deny-all until a valid snapshot arrives.
                proxy
                    .set_policy(std::sync::Arc::new(
                        cmcp_core::ExternalToolAllowlistPolicy::default(),
                    ))
                    .await;
                match proxy.reload(cfg.servers).await {
                    Ok(()) => {
                        crate::mcp::server::install_proxy_policy(&proxy, &snapshot_config).await;
                        let tool_count = proxy.tool_count().await;
                        eprintln!(
                            "[Cassy] Proxy reloaded ({server_count} server(s), {tool_count} tools)"
                        );
                        match crate::mcp::server::write_proxy_snapshot_cache_for_config(
                            &self.config.cas_root,
                            &proxy,
                            &snapshot_config,
                        )
                        .await
                        {
                            Ok(()) => self.mark_proxy_snapshot_refreshed(),
                            Err(error) => {
                                eprintln!("[Cassy] Failed to publish MCP proxy state: {error}");
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[Cassy] Proxy reload failed: {e}");
                        match crate::mcp::server::write_unavailable_proxy_snapshot_cache(
                            &self.config.cas_root,
                            Some(&snapshot_config),
                            crate::mcp::server::ProxySnapshotFailure::EngineReloadFailed,
                        ) {
                            Ok(()) => self.mark_proxy_snapshot_refreshed(),
                            Err(error) => {
                                eprintln!(
                                    "[Cassy] Failed to publish MCP proxy failure state: {error}"
                                );
                            }
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("[Cassy] Failed to load proxy config: {e}");
                match crate::mcp::server::write_unavailable_proxy_snapshot_cache(
                    &self.config.cas_root,
                    None,
                    crate::mcp::server::ProxySnapshotFailure::ConfigInvalid,
                ) {
                    Ok(()) => self.mark_proxy_snapshot_refreshed(),
                    Err(error) => {
                        eprintln!("[Cassy] Failed to publish MCP proxy failure state: {error}");
                    }
                }
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

        // Record the binary epoch this process opens (EPIC cas-6212 / cas-8d2a,
        // spec §9). Without it, "is symptom X fixed" can only be answered
        // against tag dates — the mistake cas-9d92 had to retract, because a
        // pre-install daemon kept serving the old binary for 34 minutes after
        // the new one landed. Best-effort: a daemon must never fail to start
        // because the history index is unavailable.
        {
            use cas_store::{HistoryStore, SqliteHistoryStore};
            let started_at = Utc::now().to_rfc3339();
            match SqliteHistoryStore::open(&self.config.cas_root) {
                Ok(store) => {
                    let epoch = crate::history::epochs::current_daemon_epoch(&started_at);
                    if let Err(e) = store.record_epoch(&epoch) {
                        tracing::debug!(error = %e, "history epoch not recorded");
                    }
                }
                Err(e) => tracing::debug!(error = %e, "history epoch store unavailable"),
            }
        }

        // Unix socket for hook communication.
        //
        // cas-eabe (GH #163): this is an *election*, not a one-shot bind. If
        // another `cas serve` already owns `daemon.sock` we stand by and retry,
        // so when that owner dies a survivor takes the role over within a
        // bounded interval instead of the project going daemonless. If our own
        // binary is replaced on disk while we hold the role, we hand it back so
        // a current-binary serve can pick it up. Spawned as its own task so the
        // accept loop is never blocked by maintenance/sync/indexing below.
        let election =
            socket::ElectionConfig::new(self.config.cas_root.clone(), Arc::clone(&self.shutdown));
        let socket_task = {
            let daemon = Arc::clone(&self);
            tokio::spawn(socket::run_socket_election(election, move |mut stream| {
                let daemon = Arc::clone(&daemon);
                async move {
                    if let Some(event) = socket::read_event(&mut stream).await {
                        let response = daemon.handle_socket_event(event).await;
                        let _ = socket::send_response(&mut stream, &response).await;
                    }
                }
            }))
        };

        // Create interval timers
        let mut cloud_sync_interval =
            tokio::time::interval(Duration::from_secs(self.config.cloud_sync_interval_secs));
        let mut maintenance_interval =
            tokio::time::interval(Duration::from_secs(self.config.maintenance_interval_secs));
        let mut relevance_sampling_interval = tokio::time::interval(Duration::from_secs(
            self.config.relevance_sampling_interval_secs.max(1),
        ));
        let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(30)); // Agent heartbeat every 30s
        let mut code_index_interval =
            tokio::time::interval(Duration::from_secs(self.config.code_index_interval_secs));
        // Structural git-history index (EPIC cas-6212 / cas-7a21, spec §4.3).
        let mut history_index_interval = tokio::time::interval(Duration::from_secs(
            self.config.history_index_interval_secs.max(1),
        ));
        // GitHub + CHANGELOG docs (EPIC cas-6212 / cas-9a38, spec §8).
        let mut history_docs_interval = tokio::time::interval(Duration::from_secs(
            self.config.history_docs_interval_secs.max(1),
        ));
        // Automagic embedding drain (EPIC cas-6212 / cas-db6e, spec §4.4, §7).
        let mut embed_drain_interval = tokio::time::interval(Duration::from_secs(
            self.config.embed_drain_interval_secs.max(1),
        ));
        // cas-499c: the max-staleness half of the idle-preferred scheduler. Starts "now" so a
        // freshly-started daemon still waits out one full ceiling before overriding idleness.
        let mut last_code_index = tokio::time::Instant::now();
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
        relevance_sampling_interval.tick().await;
        heartbeat_interval.tick().await;
        code_index_interval.tick().await;
        history_index_interval.tick().await;
        history_docs_interval.tick().await;
        embed_drain_interval.tick().await;
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
                    "[Cassy] Adopting pre-registered agent: {} (registered via fallback)",
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
            eprintln!("[Cassy] Running initial cloud sync (push stale + pull)...");
            match self.run_cloud_sync().await {
                Ok(result) => {
                    let pushed = result.total_pushed();
                    let pulled = result.total_pulled();
                    if pushed > 0 || pulled > 0 {
                        eprintln!(
                            "[Cassy] Initial cloud sync complete: {pushed} pushed, {pulled} pulled"
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

                // Cloud sync is a bounded background push/pull. Unlike full
                // maintenance, it must not require an idle window: a busy
                // factory keeps receiving MCP requests often enough to reset
                // ActivityTracker forever, which used to leave its local
                // queue wedged despite the configured 60-second cadence.
                _ = cloud_sync_interval.tick() => {
                    if self.cloud_syncer.is_some() {
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

                // Injected-relevance evaluation is a separate, weekly-scale
                // job. It samples only while idle and leaves unlabeled rows
                // untouched when no receiving-agent/scheduled judge is
                // attached, so evaluation never blocks interactive traffic.
                _ = relevance_sampling_interval.tick(), if self.config.relevance_sampling_enabled => {
                    if self.activity.is_idle() {
                        match self.run_relevance_sampling().await {
                            Ok(report) => {
                                if report.sampled > 0 {
                                    tracing::info!(
                                        sampled = report.sampled,
                                        labels_recorded = report.labels_recorded,
                                        unlabeled = report.unlabeled,
                                        "injected relevance sampling completed"
                                    );
                                }
                            }
                            Err(error) => {
                                let mut status = self.status.write().await;
                                status.last_error = Some(format!(
                                    "Injected relevance sampling failed: {error}"
                                ));
                            }
                        }
                    }
                }

                // Agent heartbeat - keep agent alive
                _ = heartbeat_interval.tick() => {
                    // Binary epoch first, and deliberately NOT inside
                    // `send_agent_heartbeat` (EPIC cas-6212 / cas-8d2a, spec §9):
                    // that function returns early whenever the harness client
                    // behind the agent is gone, and a daemon whose client died
                    // is exactly the survivor still serving a superseded binary
                    // — the one whose liveness tail defines the MIXED window.
                    // Losing its stamp would silently shrink MIXED and let
                    // both-binaries data read as post-fix.
                    self.touch_binary_epoch();
                    self.send_agent_heartbeat().await;
                    #[cfg(feature = "mcp-proxy")]
                    if let Some(proxy) = self.proxy.read().await.clone() {
                        if let Err(error) = crate::mcp::viktor_watch::poll_due_watches(
                            &self.config.cas_root,
                            &proxy,
                        ).await {
                            tracing::warn!(error = %error, "Viktor inbound watch tick failed");
                        }
                        if let Err(error) = crate::mcp::viktor_watch::discover_originated_messages(
                            &self.config.cas_root,
                            &proxy,
                        ).await {
                            tracing::warn!(error = %error, "Viktor originated-thread discovery tick failed");
                        }
                    }
                }

                // Code indexing - idle-PREFERRED, with a max-staleness override.
                //
                // cas-499c (operator ruling, amended): this used to be a hard `is_idle()` gate,
                // and that gate is precisely why the symbol index had never run anywhere — a
                // busy factory daemon is never idle, so the gated job never fired and
                // `code_files` stayed at 0 forever. "Automatic but polite" must not degrade
                // into "never runs".
                //
                // So: index whenever the daemon is idle (the polite path, unchanged), and if it
                // has not managed an idle window for CODE_INDEX_MAX_STALENESS_SECS, index anyway.
                // The ceiling is what converts politeness into a deferral rather than a refusal.
                // `cas doctor` reports the resulting lag; `cas index code` forces it now.
                _ = code_index_interval.tick() => {
                    if self.code_watcher.is_some() {
                        let stale_for = last_code_index.elapsed();
                        let is_idle = self.activity.is_idle();
                        if should_run_code_index(is_idle, stale_for) {
                            if !is_idle {
                                eprintln!(
                                    "[Cassy] Code indexing: no idle window for {}s, indexing anyway \
                                     (max staleness {CODE_INDEX_MAX_STALENESS_SECS}s)",
                                    stale_for.as_secs()
                                );
                            }
                            // Stamped regardless of outcome: a cycle that fails must not spin the
                            // override every tick.
                            last_code_index = tokio::time::Instant::now();
                            if let Err(e) = self.run_code_index_cycle().await {
                                let mut status = self.status.write().await;
                                status.last_error = Some(format!("Code indexing failed: {e}"));
                            }
                        }
                    }
                }

                // Structural git-history indexing (EPIC cas-6212 / cas-7a21).
                //
                // Not gated on `activity.is_idle()` at all (spec §4.3). Both
                // arms are answering the same lesson from opposite ends: a hard
                // idleness gate on a daemon that is never idle means the job
                // never runs, which is why `code_files` read 0 on a repo with
                // thousands of commits. The code-index arm above solves it with
                // idle-preferred scheduling plus a max-staleness ceiling
                // (cas-499c), because indexing every source file is expensive
                // enough to be worth deferring. A history delta pass is not: it
                // is a `rev-list` plus two `git log` reads over the day's
                // commits, so it is cheap enough to be rate-limited by its
                // interval alone and needs no politeness machinery.
                _ = history_index_interval.tick() => {
                    if self.config.index_history {
                        if let Err(e) = self.run_history_index_cycle().await {
                            let mut status = self.status.write().await;
                            status.last_error = Some(format!("History indexing failed: {e}"));
                        }
                    }
                }

                // GitHub + CHANGELOG doc indexing (EPIC cas-6212 / cas-9a38).
                //
                // Ungated like the git arm above, but on a fifteen-minute
                // interval rather than five: this half is bounded by a third
                // party's rate limits and by how fast humans write issues, not
                // by local work (spec §8). Absent `gh`, an unset `issues.repo`
                // or a missing CHANGELOG are declared boundaries recorded on
                // the ledger rows — they must never surface as a daemon error,
                // or every repository without GitHub configured would report a
                // permanently unhealthy daemon.
                _ = history_docs_interval.tick() => {
                    if self.config.index_history_docs {
                        if let Err(e) = self.run_history_docs_cycle().await {
                            let mut status = self.status.write().await;
                            status.last_error = Some(format!("History doc indexing failed: {e}"));
                        }
                    }
                }

                // Automagic embedding drain (EPIC cas-6212 / cas-db6e, spec §4.4).
                //
                // Vectors used to be computed only inside `cas cloud sync`, which
                // made "is my corpus embedded?" a question about whether a human
                // had recently typed a command — and a 107-page knowledge backlog
                // duly sat un-embedded until someone ran sync by hand. That manual
                // step is the defect this arm removes.
                //
                // Ungated on idleness, for the reason the history arms are: a
                // daemon in a busy factory is never idle, and a gate that never
                // opens is indistinguishable from a feature that was never built.
                // The work is bounded instead — DRAIN_BATCH units per tick, chunked
                // at the endpoint's 32-input cap and paced under its 120 req/60 s
                // limit — so ungated does not mean unbounded.
                //
                // Logged out, or an endpoint with no `/api/embeddings`, is a
                // declared boundary: the drain returns capability_absent, creates
                // no LMDB environment and makes no request.
                _ = embed_drain_interval.tick() => {
                    if self.config.embed_drain {
                        if let Err(e) = self.run_embedding_drain_cycle().await {
                            let mut status = self.status.write().await;
                            status.last_error = Some(format!("Embedding drain failed: {e}"));
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
            eprintln!("[Cassy] Running final cloud sync before shutdown...");
            match tokio::time::timeout(Duration::from_secs(10), self.run_cloud_sync()).await {
                Ok(Ok(result)) => {
                    let pushed = result.total_pushed();
                    let pulled = result.total_pulled();
                    if pushed > 0 || pulled > 0 {
                        eprintln!(
                            "[Cassy] Final cloud sync complete: {pushed} pushed, {pulled} pulled"
                        );
                    } else {
                        eprintln!("[Cassy] Final cloud sync complete (nothing pending)");
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

        // Stop the socket election. It polls the shutdown flag (already set to
        // get us here) and removes the socket file itself if it owns it, so
        // give it a moment to exit cleanly before aborting.
        {
            let mut task = socket_task;
            if tokio::time::timeout(Duration::from_secs(2), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
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

        // Socket cleanup belongs exclusively to the election lease.  If the
        // task had to be aborted, its listener and lease are dropped together;
        // a stale pathname is safe for the next lease to reclaim, whereas an
        // out-of-band unlink here could remove a successor's live socket.

        Ok(())
    }

    /// Run one structural git-history indexing pass (EPIC cas-6212 / cas-7a21).
    ///
    /// Backfills on first run, then walks only `watermark..HEAD`. Shells out to
    /// git and writes SQLite, so it runs on the blocking pool. A repo-less or
    /// git-less environment is a no-op, not an error: the daemon must keep
    /// ticking for every other subsystem.
    async fn run_history_index_cycle(&self) -> Result<(), CasError> {
        let cas_root = self.config.cas_root.clone();

        let outcome = tokio::task::spawn_blocking(move || {
            let repo_root = match crate::history::repo_root_for(&cas_root) {
                Ok(root) => root,
                // Not a git repo — nothing to index, and nothing to complain
                // about on every tick.
                Err(_) => return Ok::<_, anyhow::Error>(None),
            };
            let walked = crate::history::run_index_pass(&cas_root, &repo_root)?;

            // The commit → session spine repair (EPIC cas-6212 / cas-519f,
            // spec §5.3). It runs after indexing, on the same tick, but it is
            // NOT gated on this pass having found commits: the pass is driven
            // by "indexed commits with no link", so it also drains the backlog
            // that existed before the repair did.
            //
            // A repair failure must not fail the index pass. Indexing is the
            // load-bearing half — the query surface works without a spine, and
            // it did for all of M4 — so a broken edge is reported and dropped
            // rather than allowed to stop the walker on every tick.
            let repaired = match crate::history::provenance::repair_commit_links(
                &cas_root,
                &repo_root,
                crate::history::provenance::REPAIR_BATCH,
            ) {
                Ok(outcome) => Some(outcome),
                Err(e) => {
                    eprintln!("[Cassy] History provenance repair failed: {e}");
                    None
                }
            };
            Ok::<_, anyhow::Error>(Some((walked, repaired)))
        })
        .await
        .map_err(|e| CasError::Other(format!("Task join error: {e}")))?
        .map_err(|e| CasError::Other(format!("History indexing failed: {e}")))?;

        if let Some((outcome, repaired)) = outcome {
            if outcome.commits_indexed > 0 {
                eprintln!(
                    "[Cassy] History indexing ({}): {} commits, {} file changes",
                    outcome.mode.as_str(),
                    outcome.commits_indexed,
                    outcome.files_indexed,
                );
            }
            if let Some(repaired) = repaired {
                if repaired.written > 0 {
                    eprintln!(
                        "[Cassy] History provenance: {} commit link(s) reconstructed \
                         ({} examined, {} with no session-bearing edge, {} ambiguous)",
                        repaired.written,
                        repaired.examined,
                        repaired.no_session_edge,
                        repaired.skipped_ambiguous,
                    );
                }
            }
        }

        Ok(())
    }

    /// Run one GitHub + CHANGELOG doc indexing pass (EPIC cas-6212 / cas-9a38).
    ///
    /// Returns `Ok` for every *declared boundary* — no git repo, no `gh`, no
    /// `issues.repo`, no CHANGELOG — because those are states of the world, not
    /// daemon failures, and each has already been recorded on its own
    /// `history_index_state` row where `cas history status` will report it
    /// (spec §10.2). Only a local store failure reaches the caller.
    async fn run_history_docs_cycle(&self) -> Result<(), CasError> {
        let cas_root = self.config.cas_root.clone();

        let outcome = tokio::task::spawn_blocking(move || {
            let Ok(repo_root) = crate::history::repo_root_for(&cas_root) else {
                return None;
            };
            let repo = crate::config::Config::load(&cas_root)
                .unwrap_or_default()
                .issues
                .as_ref()
                .and_then(|i| i.repo.clone())
                .map(|r| r.trim().to_string())
                .filter(|r| !r.is_empty());
            Some(crate::history::run_docs_pass(
                &cas_root,
                &repo_root,
                repo.as_deref(),
                false,
                true,
                true,
            ))
        })
        .await
        .map_err(|e| CasError::Other(format!("Task join error: {e}")))?;

        if let Some(outcome) = outcome
            && let Some(Ok(fetch)) = &outcome.github
            && fetch.docs_total() > 0
        {
            eprintln!(
                "[Cassy] History docs: {} issue(s), {} PR(s), {} comment(s)",
                fetch.issues, fetch.pull_requests, fetch.comments,
            );
        }

        Ok(())
    }

    /// Drain every pending vector — knowledge pages AND code history — without
    /// anyone having to run `cas cloud sync` (EPIC cas-6212 / cas-db6e).
    ///
    /// Problems are put where a human will find them: the drain's own report
    /// fields, the `history_index_state('embeddings')` ledger row that
    /// `cas doctor` reads, and `status.last_error`. Deliberately not a
    /// `tracing::warn!` and nothing else — that is the shape that let cas-a924's
    /// permanent `400` read as a cheerful "0 embedded" for weeks.
    async fn run_embedding_drain_cycle(&self) -> Result<(), CasError> {
        let cas_root = self.config.cas_root.clone();

        let report = tokio::task::spawn_blocking(move || {
            crate::cloud::drain_all_pending(&cas_root, crate::cloud::DRAIN_BATCH)
        })
        .await
        .map_err(|e| CasError::Other(format!("Task join error: {e}")))??;

        // No capability is a state of the installation, not news. Say nothing.
        if report.capability_absent {
            return Ok(());
        }

        if report.did_work() {
            eprintln!(
                "[Cassy] Embedding drain: {} embedded ({} request(s), {} skipped), {} still pending",
                report.embedded(),
                report.requests(),
                report.skipped(),
                report.pending_after(),
            );
        }

        let problems = report.problems();
        if !problems.is_empty() {
            let mut status = self.status.write().await;
            status.last_error = Some(format!(
                "Embedding drain: {} ({} unit(s) still awaiting a vector)",
                problems.join("; "),
                report.pending_after()
            ));
        }

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
                "[Cassy] Code indexing: {} indexed, {} deleted, {} symbols",
                result.files_indexed, result.files_deleted, result.symbols_indexed,
            );

            // EPIC cas-7d31 (cas-c9be): the code index just told us something
            // moved, which is exactly when distilled knowledge goes stale.
            // Opt-in only — a distillation pass spends tokens, so it never
            // starts from a background cycle unless the operator asked for it.
            if crate::knowledge::auto_distill_enabled() {
                self.run_knowledge_distillation().await;
            }
        }

        Ok(())
    }

    /// Distill changed sources into the knowledge wiki (opt-in, see
    /// [`crate::knowledge::AUTO_DISTILL_ENV`]).
    ///
    /// Failures are logged, never propagated: knowledge is an enrichment, and a
    /// missing provider binary must not take down the daemon's index cycle.
    async fn run_knowledge_distillation(&self) {
        let cas_root = self.config.cas_root.clone();
        let model = self.config.to_daemon_config().model;

        let outcome = tokio::task::spawn_blocking(move || {
            let store = cas_store::SqliteKnowledgeStore::open(&cas_root)?;
            let project_root = cas_root
                .parent()
                .map(std::path::Path::to_path_buf)
                .ok_or_else(|| anyhow::anyhow!("cannot resolve project root"))?;
            // The source set MUST be the complete one: `run_distillation`
            // tombstones every ledger path missing from it, so handing it a
            // set with no `code://` module sources would cascade-delete every
            // module page the CLI had built (and re-bill them next pass).
            let symbols = crate::cli::knowledge_symbols_with_limit(&cas_root, crate::cli::KNOWLEDGE_MAX_SYMBOLS);
            let scan = crate::knowledge::scan_sources(&project_root, &symbols.symbols);
            // A source that could not be decoded is named here rather than
            // dropped: its page will be missing from the wiki either way, and a
            // silent hole is the one an operator never chases (cas-c736).
            for note in scan.skip_notes() {
                tracing::warn!(%note, "Knowledge distillation skipped a source");
            }
            let sources = scan.sources;
            let runner = crate::knowledge::ClaudeCliRunner::new(Some(model));
            let config = crate::knowledge::DistillConfig {
                protected_prefixes: if symbols.truncated {
                    vec![crate::knowledge::sources::CODE_MODULE_SCHEME.to_string()]
                } else {
                    Vec::new()
                },
                ..crate::knowledge::DistillConfig::default()
            };
            crate::knowledge::run_distillation(&store, &runner, &sources, &config)
        })
        .await;

        match outcome {
            Ok(Ok(report)) if !report.is_noop() => {
                eprintln!(
                    "[Cassy] Knowledge distillation: {} pages written, {} llm calls",
                    report.pages_written, report.llm_calls
                );
            }
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "Knowledge distillation failed");
            }
            Err(error) => {
                tracing::warn!(error = %error, "Knowledge distillation task join error");
            }
        }
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
                    eprintln!("[Cassy] {msg}");
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
                    if !crate::daemon::heartbeat_stale_agent_should_be_reaped(&agent, |name| {
                        crate::cli::factory::wedged::find_worker_pid(
                            &crate::cli::factory::wedged::RealProcessTable,
                            name,
                        )
                        .filter(|pid| pid_alive(*pid))
                    }) {
                        tracing::warn!(
                            worker = %agent.name,
                            agent_id = %agent.id,
                            "heartbeat stale but live factory worker process found; skipping reap"
                        );
                        continue;
                    }
                    let held: Vec<String> = agent_store
                        .list_agent_leases(&agent.id)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|l| l.task_id)
                        .collect();
                    if let Err(e) = agent_store.mark_stale(&agent.id) {
                        let msg = format!("Failed to mark stale agent {}: {}", agent.id, e);
                        eprintln!("[Cassy] {msg}");
                        result.errors.push(msg);
                    } else {
                        match queue_stale_factory_worker_shutdown(&cas_root, &agent) {
                            Ok(Some(request_id)) => tracing::warn!(worker = %agent.name, agent_id = %agent.id, request_id, "heartbeat reap queued factory process-tree teardown"),
                            Ok(None) => {}
                            Err(error) => {
                                let msg = format!("Failed to queue factory teardown for stale worker {}: {error}", agent.id);
                                eprintln!("[Cassy] {msg}");
                                result.errors.push(msg);
                            }
                        }
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
                eprintln!("[Cassy] {msg}");
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
        status.curated_entries_protected = result.curated_entries_protected;
        status.promoted_on_access = result.promoted_on_access;

        if let Some(err) = result.errors.first() {
            status.last_error = Some(err.clone());
        } else {
            status.last_error = None;
        }

        Ok(result)
    }

    /// Run the bounded injected-relevance sampler without holding up the
    /// async daemon loop. The default scheduled judge is deliberately
    /// unconfigured; tests and receiving-agent integrations use the public
    /// callback-based daemon function directly.
    async fn run_relevance_sampling(&self) -> Result<cas_store::RelevanceSamplingReport, CasError> {
        let cas_root = self.config.cas_root.clone();
        let sample_size = self.config.relevance_sampling_sample_size;
        let cooldown_secs = self.config.relevance_sampling_interval_secs;
        tokio::task::spawn_blocking(move || {
            crate::daemon::relevance::run_unconfigured_injected_relevance_sampling(
                &cas_root,
                sample_size,
                cooldown_secs,
            )
        })
        .await
        .map_err(|error| CasError::Other(format!("Relevance sampling task join error: {error}")))?
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
                    "[Cassy] Socket: SessionStart for {} (name: {:?}, role: {:?}, pid: {})",
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
                    "[Cassy] Socket: SessionEnd for {}",
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
                        "[Cassy] Worker activity: {} - {}",
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
        // record exists for their own daemon to adopt via PID matching).
        // `register_session_start_agent` first reconciles a factory worker
        // with a live same-PID row created by either prior socket handling or
        // eager MCP bootstrap, rather than minting a fresh session-id row.
        let mut registered_id = None;
        if let Ok(store) = open_agent_store(&self.config.cas_root) {
            if let Ok((agent, reused)) = register_session_start_agent(
                store.as_ref(),
                &session_id,
                agent_name.as_deref(),
                agent_role.as_deref(),
                cc_pid,
                clone_path.as_deref(),
            ) {
                registered_id = Some(agent.id.clone());
                eprintln!(
                    "[Cassy] Daemon {} agent: {} (role: {}, ours: {})",
                    if reused { "refreshed" } else { "registered" },
                    &agent.id[..8.min(agent.id.len())],
                    agent.role,
                    is_our_agent,
                );

                // The socket's in-memory PID mapping must agree with the
                // canonical durable identity; otherwise GetSession would
                // immediately feed the discarded fresh id back into MCP.
                if reused {
                    self.pid_sessions
                        .write()
                        .await
                        .insert(cc_pid, agent.id.clone());
                }

                // Force an immediate heartbeat so the agent doesn't start the
                // stale countdown waiting for the next 30s daemon tick.
                if let Err(e) = store.heartbeat(&agent.id) {
                    tracing::warn!(
                        agent_id = %&agent.id[..8.min(agent.id.len())],
                        error = %e,
                        "Immediate post-registration heartbeat failed"
                    );
                }

                // A replayed SessionStart only refreshes local liveness; it
                // does not represent a second cloud agent either.
                if !reused {
                    let mut coord_guard = self.cloud_coordinator.write().await;
                    if let Some(ref mut coord) = *coord_guard {
                        match coord.register(&agent) {
                            Ok(_) => {
                                eprintln!(
                                    "[Cassy] Cloud registered agent: {}",
                                    &session_id[..8.min(session_id.len())]
                                );
                            }
                            Err(e) => {
                                eprintln!(
                                    "[Cassy] Cloud agent registration failed (best-effort): {e}"
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
            *guard = Some(registered_id.unwrap_or(session_id));
        }
    }

    /// Advance the `ended_at` of this process's binary epoch (spec §9).
    ///
    /// Best-effort and synchronous: one small UPDATE on the shared connection,
    /// once every 30s. A failure here must never disturb the daemon loop — an
    /// un-advanced epoch degrades a verdict to INSUFFICIENT, which is the safe
    /// direction.
    fn touch_binary_epoch(&self) {
        use cas_store::{HistoryStore, SqliteHistoryStore};
        if let Ok(history) = SqliteHistoryStore::open(&self.config.cas_root) {
            let _ = history.touch_epoch_end(std::process::id() as i64, &Utc::now().to_rfc3339());
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
                        "[Cassy] Adopted agent by PID match: {} (pid: {})",
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
                    eprintln!("[Cassy] {msg}");
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
#[cfg(target_os = "linux")]
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

    // Register before walking the tree. Changes during the walk are then in
    // either the snapshot or the event stream, never in an unobserved gap.
    let scan_paths = watcher.watch_paths().to_vec();
    let extensions = config.code_extensions.clone();
    let excludes = config.code_exclude_patterns.clone();
    if let Err(e) = watcher.start_with_initial_scan(|| {
        crate::daemon::indexing::collect_source_files(&scan_paths, &extensions, &excludes)
    }) {
        eprintln!("[Cassy] Failed to start code watcher: {e}");
        return None;
    }

    eprintln!("[Cassy] Code watcher started");
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
    // SqliteStore::open expects the Cassy directory path, not cas.db.
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
