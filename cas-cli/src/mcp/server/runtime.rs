use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Context;

use crate::mcp::server::CasCore;
use crate::store::open_agent_store;

/// Run the MCP server with 13 meta-tools (11 Cassy + 2 proxy)
pub async fn run_server() -> anyhow::Result<()> {
    run_server_impl().await
}

// Startup pull is an apply-remote path: using the normal openers here wraps
// the stores in Syncing*, which feeds each pulled row back into SyncQueue.
// Keep these narrow helpers so the startup call site and its regression test
// share the same non-syncing contract.
fn open_startup_pull_entry_store(
    cas_root: &std::path::Path,
) -> crate::error::Result<Arc<dyn crate::store::Store>> {
    crate::store::open_store_local(cas_root)
}

fn open_startup_pull_task_store(
    cas_root: &std::path::Path,
) -> crate::error::Result<Arc<dyn crate::store::TaskStore>> {
    crate::store::open_task_store_local(cas_root)
}

/// Bring the project database to the schema this MCP binary requires before
/// any daemon, cloud sync, or store opener can issue application queries.
///
/// A factory worker shares its project's database with long-lived MCP servers.
/// When a newly installed binary adds a column, letting it continue to eager
/// store opening would turn this recoverable startup boundary into raw SQL
/// errors on the first tool call.
fn ensure_mcp_schema(cas_root: &std::path::Path) -> anyhow::Result<()> {
    let status = crate::migration::check_migrations(cas_root)
        .context("could not determine the Cassy schema required by this MCP binary")?;
    if !status.has_pending() {
        return Ok(());
    }

    let pending_migration = status
        .pending
        .first()
        .map(|migration| format!("m{} `{}`", migration.id, migration.name))
        .unwrap_or_else(|| "an unknown migration".to_string());
    let mismatch = || crate::error::CasError::SchemaOutdated {
        binary: env!("CARGO_PKG_VERSION").to_string(),
        current: status.current_version,
        required: status.latest_version,
        pending_migration: pending_migration.clone(),
    };

    match crate::migration::run_migrations(cas_root, false) {
        Ok(result) => {
            if result.applied_count > 0 {
                eprintln!(
                    "[Cassy] Applied {} pending schema migration(s) before MCP startup.",
                    result.applied_count
                );
            }
        }
        Err(error) => {
            return Err(anyhow::anyhow!(
                "{mismatch}; automatic application of pending migration {pending_migration} failed: {error}",
                mismatch = mismatch(),
            ));
        }
    }

    let final_status = crate::migration::check_migrations(cas_root)
        .context("could not verify the Cassy schema after automatic migration")?;
    if final_status.has_pending() {
        return Err(anyhow::Error::from(mismatch()));
    }
    Ok(())
}

/// Internal implementation for running the MCP server
async fn run_server_impl() -> anyhow::Result<()> {
    let enable_daemon = true;
    use crate::cloud::{CloudConfig, CloudSyncer, CloudSyncerConfig, SyncQueue};
    use crate::mcp::daemon::{EmbeddedDaemonConfig, spawn_daemon};
    use crate::mcp::tools::CasService;
    use crate::store::{
        open_commit_link_store, open_event_store, open_file_change_store, open_prompt_store,
        open_rule_store_local, open_skill_store_local, open_spec_store,
    };
    use rmcp::ServiceExt;
    use rmcp::transport::stdio;

    let cas_root = resolve_mcp_serve_root()?;

    // Install panic hook before anything else can panic. Routes panics to a
    // dedicated file under `.cas/logs/cas-serve-{date}.log` with timestamp,
    // PID, and a full backtrace. The default hook still runs so stderr goes
    // to the MCP client as before.
    //
    // Without this, panics in tool handlers kill the process and the crash
    // output is lost — the MCP client only sees "Connection closed" and the
    // auto-respawn path gives us no diagnostic trail.
    install_serve_panic_hook(&cas_root);

    // Parent-death watchdog (cas-82d6c). Armed before any store is opened so a
    // startup that wedges *before* the transport exists is covered too. It
    // must also precede schema migration: an orphaned worker can otherwise
    // hold the database mid-migration indefinitely. See `parent_watchdog` for
    // why stdin EOF is not sufficient and why the server reaps itself rather
    // than being reaped.
    let parent_watchdog = crate::mcp::server::parent_watchdog::spawn();

    // This is deliberately before every remaining background path and every
    // store opener. All MCP configurations — including freshly spawned
    // factory workers — enter through `cas serve` and share this boundary.
    ensure_mcp_schema(&cas_root)?;

    // Register this repo in the host-scoped known_repos registry. Fires
    // every time `cas serve` starts in a directory with `.cas/`, catching
    // repos that pre-date the `cas init` registration hook. Non-fatal:
    // failure here must not block MCP serve startup.
    if let Some(repo_root) = cas_root.parent() {
        crate::store::known_repos::register_repo(repo_root);
    }

    // Opportunistic cross-repo sweep — debounced via
    // `~/.cas/last_global_sweep`. Runs on a detached blocking task so MCP
    // startup is NEVER delayed. Any panic is caught and logged; any error
    // is warn-logged. This is Unit 3's keystone wiring (EPIC cas-7c88).
    let sweep_cas_config = crate::config::Config::load(&cas_root).unwrap_or_default();
    tokio::task::spawn_blocking(move || {
        let wt_cfg = sweep_cas_config.worktrees().clone();
        match crate::worktree::sweep::opportunistic::run_if_due(&wt_cfg) {
            Ok(Some(summary)) => {
                eprintln!(
                    "[Cassy] opportunistic sweep: visited {} repo(s), reclaimed {}, salvaged {}",
                    summary.repos_visited, summary.reclaimed, summary.salvaged,
                );
            }
            Ok(None) => {
                // Skipped by debounce — no user-visible output.
            }
            Err(e) => {
                tracing::error!(error = %e, "opportunistic sweep failed");
            }
        }
    });

    // Run startup cloud pull in a background task with a short timeout
    // so a slow/unreachable cloud endpoint never blocks MCP server startup.
    //
    // Hold the JoinHandle so that if `eager_init_stores` aborts startup we can
    // cancel this task instead of leaving it racing the dying process to open
    // the same DB that just refused to open (cas-5c05 review A2).
    let cloud_sync_handle = {
        let cas_root_bg = cas_root.clone();
        tokio::task::spawn(async move {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                tokio::task::spawn_blocking(move || {
                    let cloud_config = match CloudConfig::load_from_cas_dir(&cas_root_bg) {
                        Ok(c) if c.is_logged_in() => c,
                        _ => return,
                    };
                    let queue = match SyncQueue::open(&cas_root_bg) {
                        Ok(q) => {
                            let _ = q.init();
                            q
                        }
                        Err(_) => return,
                    };
                    let config = CloudSyncerConfig {
                        timeout: std::time::Duration::from_secs(5),
                        ..Default::default()
                    };
                    let syncer = CloudSyncer::new(std::sync::Arc::new(queue), cloud_config, config);
                    let Ok(store) = open_startup_pull_entry_store(&cas_root_bg) else {
                        return;
                    };
                    let Ok(task_store) = open_startup_pull_task_store(&cas_root_bg) else {
                        return;
                    };
                    let Ok(rule_store) = open_rule_store_local(&cas_root_bg) else {
                        return;
                    };
                    let Ok(skill_store) = open_skill_store_local(&cas_root_bg) else {
                        return;
                    };
                    let Ok(spec_store) = open_spec_store(&cas_root_bg) else {
                        return;
                    };
                    let Ok(event_store) = open_event_store(&cas_root_bg) else {
                        return;
                    };
                    let Ok(prompt_store) = open_prompt_store(&cas_root_bg) else {
                        return;
                    };
                    let Ok(file_change_store) = open_file_change_store(&cas_root_bg) else {
                        return;
                    };
                    let Ok(commit_link_store) = open_commit_link_store(&cas_root_bg) else {
                        return;
                    };

                    match syncer.pull(
                        store.as_ref(),
                        task_store.as_ref(),
                        rule_store.as_ref(),
                        skill_store.as_ref(),
                        spec_store.as_ref(),
                        event_store.as_ref(),
                        prompt_store.as_ref(),
                        file_change_store.as_ref(),
                        commit_link_store.as_ref(),
                    ) {
                        Ok(result) if result.total_pulled() > 0 => {
                            eprintln!("[Cassy] Synced {} items from cloud", result.total_pulled());
                        }
                        Err(e) => {
                            eprintln!("[Cassy] Cloud sync failed (continuing): {e}");
                        }
                        _ => {}
                    }
                }),
            )
            .await;
            if result.is_err() {
                eprintln!("[Cassy] Cloud sync timed out (continuing without sync)");
            }
        })
    };

    let (daemon, activity, _handle) = if enable_daemon {
        let cas_config = crate::config::Config::load(&cas_root).unwrap_or_default();
        let code_config = cas_config.code();
        let cloud_config = cas_config.cloud.clone().unwrap_or_default();
        let project_dir = cas_root.parent().unwrap_or(&cas_root);
        let code_watch_paths: Vec<std::path::PathBuf> = code_config
            .watch_paths
            .iter()
            .map(|p| project_dir.join(p))
            .collect();

        let config = EmbeddedDaemonConfig {
            cas_root: cas_root.clone(),
            cloud_sync_enabled: cloud_config.auto_sync,
            cloud_sync_interval_secs: cloud_config.interval_secs.max(1),
            index_code: code_config.enabled,
            code_watch_paths,
            code_extensions: code_config.extensions.clone(),
            code_exclude_patterns: code_config.exclude_patterns.clone(),
            code_index_interval_secs: code_config.index_interval_secs,
            code_debounce_ms: code_config.debounce_ms,
            ..Default::default()
        };
        let (daemon, handle) = spawn_daemon(config);
        let activity = daemon.activity_tracker();
        (Some(daemon), Some(activity), Some(handle))
    } else {
        (None, None, None)
    };

    let core = CasCore::with_daemon(cas_root.clone(), activity, daemon.clone());

    // Eagerly initialize all stores before serving MCP requests.
    // This moves cold-start overhead (connection open, schema init) out of the
    // first tool call path, preventing timeouts on the initial request.
    //
    // Failure here is fatal: a partially-initialized server would respond to
    // `tools/list` with the full registry but every call would error, which is
    // the silent-degradation mode this guard exists to prevent (cas-5c05).
    if let Err(e) = eager_init_stores(&core, &cas_root) {
        // Cancel the cloud-sync task before bubbling the error so it stops
        // racing for the same DB during the parent's shutdown window.
        cloud_sync_handle.abort();
        return Err(e);
    }

    // Eager auto-registration for factory workers where SessionStart hook may not fire.
    // When CAS_SESSION_ID is set (by PtyConfig::claude()), register immediately so the
    // agent appears in worker_status before any MCP tool call is made.
    if let Ok(session_id) = std::env::var("CAS_SESSION_ID") {
        if !session_id.is_empty() {
            let agent_name =
                std::env::var("CAS_AGENT_NAME").unwrap_or_else(|_| "worker".to_string());
            eprintln!(
                "[Cassy] Eager registration: {} ({})",
                agent_name,
                &session_id[..8.min(session_id.len())]
            );
            match core.register_agent(session_id.clone(), agent_name, None) {
                Ok(_) => {
                    // Tell the daemon so it sends heartbeats
                    if let Some(ref d) = daemon {
                        let d = Arc::clone(d);
                        let sid = session_id.clone();
                        tokio::spawn(async move {
                            d.set_agent_id(sid).await;
                        });
                    }
                }
                Err(e) => {
                    eprintln!("[Cassy] Eager registration failed: {e}");
                }
            }
        }
    }

    #[cfg(feature = "mcp-proxy")]
    let project_proxy_path = cas_root.join("proxy.toml");

    // Refresh the credential-free, user-scoped Viktor default before loading
    // proxy configuration. An explicit project .cas/proxy.toml is an opt-out:
    // loading it still inherits user server definitions, so refreshing the
    // managed default here would otherwise make a project-selected proxy
    // surface connect to Viktor unexpectedly.
    #[cfg(feature = "mcp-proxy")]
    if !project_proxy_path.exists()
        && let Ok(path) = cmcp_core::config::Scope::User.config_path()
        && let Err(error) = cmcp_core::config::Config::refresh_viktor_managed_default(&path)
    {
        eprintln!("[Cassy] Failed to refresh managed Viktor proxy config: {error}");
    }

    // Load MCP proxy config from .cas/proxy.toml (project) and ~/.config/code-mode-mcp/config.toml (user)
    #[cfg(feature = "mcp-proxy")]
    let proxy = {
        let cfg = cmcp_core::config::Config::load_merged(if project_proxy_path.exists() {
            Some(&project_proxy_path)
        } else {
            None
        });
        match cfg {
            Ok(cfg) if !cfg.servers.is_empty() => {
                eprintln!(
                    "[Cassy] Connecting to {} upstream MCP server(s)...",
                    cfg.servers.len()
                );
                let snapshot_config = cfg.clone();
                match cmcp_core::ProxyEngine::from_configs(cfg.servers).await {
                    Ok(engine) => {
                        install_proxy_policy(&engine, &snapshot_config).await;
                        engine
                            .set_call_observer(std::sync::Arc::new(
                                crate::mcp::viktor_watch::ViktorWatchRecorder::new(
                                    cas_root.clone(),
                                ),
                            ))
                            .await;
                        let count = engine.tool_count().await;
                        eprintln!("[Cassy] MCP proxy ready ({count} upstream tools)");
                        if let Err(error) = write_proxy_snapshot_cache_for_config(
                            &cas_root,
                            &engine,
                            &snapshot_config,
                        )
                        .await
                        {
                            eprintln!("[Cassy] Failed to publish MCP proxy state: {error}");
                        }
                        Some(std::sync::Arc::new(engine))
                    }
                    Err(e) => {
                        eprintln!("[Cassy] MCP proxy init failed (continuing without proxy): {e}");
                        if let Err(error) = write_unavailable_proxy_snapshot_cache(
                            &cas_root,
                            Some(&snapshot_config),
                            ProxySnapshotFailure::EngineStartFailed,
                        ) {
                            eprintln!("[Cassy] Failed to publish MCP proxy failure state: {error}");
                        }
                        None
                    }
                }
            }
            Ok(cfg) => {
                if let Err(error) = publish_non_live_proxy_snapshot(
                    &cas_root,
                    Some(&cfg),
                    ProxySnapshotState::Empty,
                    None,
                ) {
                    eprintln!("[Cassy] Failed to publish empty MCP proxy state: {error}");
                }
                None
            }
            Err(error) => {
                eprintln!(
                    "[Cassy] Failed to load MCP proxy config (continuing without proxy): {error}"
                );
                if let Err(error) = write_unavailable_proxy_snapshot_cache(
                    &cas_root,
                    None,
                    ProxySnapshotFailure::ConfigInvalid,
                ) {
                    eprintln!("[Cassy] Failed to publish MCP proxy failure state: {error}");
                }
                None
            }
        }
    };
    #[cfg(not(feature = "mcp-proxy"))]
    let _proxy: Option<()> = None;

    // Register proxy with daemon for hot-reload watching
    #[cfg(feature = "mcp-proxy")]
    if let (Some(d), Some(p)) = (&daemon, &proxy) {
        d.set_proxy(Arc::clone(p)).await;
    }

    #[cfg(feature = "mcp-proxy")]
    let proxy_active = proxy.is_some();
    #[cfg(not(feature = "mcp-proxy"))]
    let proxy_active = false;

    #[cfg(feature = "mcp-proxy")]
    let service = CasService::new(core, proxy);
    #[cfg(not(feature = "mcp-proxy"))]
    let service = CasService::new(core);

    // Empty-registry guard — if the tool router somehow ends up empty, refuse
    // to start. Otherwise the server would respond to `tools/list` with `[]`
    // and the MCP client (e.g. Claude Code) silently shows zero Cassy tools to
    // the agent with no surfaced error. See cas-5c05.
    let tool_names = service.registered_tool_names();
    if tool_names.is_empty() {
        anyhow::bail!(
            "MCP tool registry is empty. This is a Cassy build bug — refusing to \
             start a server that would silently expose zero tools to the client. \
             Rebuild Cassy and retry."
        );
    }
    eprintln!(
        "[Cassy] Starting MCP server ({} tools: {}{})",
        tool_names.len(),
        tool_names.join(", "),
        if proxy_active { ", proxy active" } else { "" }
    );

    let server = service.serve(stdio()).await?;
    // Two ways out: the transport ends (stdin EOF / client disconnect), or the
    // watchdog proves the harness that spawned us is gone. The second exists
    // because the first never fires when the stdin write end outlives the
    // harness — a pty, or a sibling that inherited the pipe (cas-82d6c).
    let waiting = server.waiting();
    tokio::pin!(waiting);
    tokio::select! {
        result = &mut waiting => {
            if let Err(e) = result {
                eprintln!("[Cassy] MCP server terminated with error: {e}");
            }
        }
        () = parent_watchdog.tripped() => {}
    }

    eprintln!("[Cassy] Shutting down, releasing tasks...");
    {
        use crate::agent_id::read_session_for_mcp;
        if let Ok(agent_id) = read_session_for_mcp(&cas_root) {
            if let Err(e) = release_agent_tasks(&cas_root, &agent_id) {
                eprintln!("[Cassy] Failed to release agent tasks for {agent_id}: {e}");
            }
        }
    }

    if let Some(d) = daemon {
        d.shutdown();
    }

    Ok(())
}

/// Install the production external dispatch policy from parsed configuration.
/// This is unconditional: an empty allowlist deliberately replaces the proxy
/// crate's compatibility default with a fail-closed policy.
#[cfg(feature = "mcp-proxy")]
pub(crate) async fn install_proxy_policy(
    engine: &cmcp_core::ProxyEngine,
    config: &cmcp_core::config::Config,
) {
    let routes = config
        .allowlist
        .iter()
        .map(|route| cmcp_core::ExternalToolRoute::new(route.server.clone(), route.tool.clone()));
    let delegation_routes = config
        .delegation
        .external_production_verification
        .iter()
        .flat_map(|gateway| {
            [
                cmcp_core::ExternalToolRoute::new(
                    gateway.server.clone(),
                    gateway.start_tool.clone(),
                ),
                cmcp_core::ExternalToolRoute::new(
                    gateway.server.clone(),
                    gateway.wait_tool.clone(),
                ),
            ]
        });
    let policy = cmcp_core::ExternalToolAllowlistPolicy::new(routes)
        .with_supervisor_delegation_routes(delegation_routes);
    engine.set_policy(std::sync::Arc::new(policy)).await;
}

/// Total time budget for the eager store-init phase before `cas serve` aborts.
///
/// This budget exists to convert silent zero-tools mode into a loud failure —
/// not to time-police healthy startup. Three forces set its value:
///
/// 1. **Real-incident floor.** The cas-5c05 trigger was a 15-hour `cas init`
///    hang on the same project. Anything in the seconds-to-minute range
///    catches that with massive headroom.
/// 2. **Thundering-herd ceiling.** investigation-mcp-worktree.md (cas-09f1,
///    2026-03-25) documents 6 concurrent `cas serve` processes opening the
///    same `cas.db`. Each store has a 5s SQLite `busy_timeout`, so realistic
///    cross-process contention can stack to a low-tens-of-seconds for a
///    legitimate factory startup. The budget must tolerate that.
/// 3. **MCP client deadline.** Claude Code's `initialize`/`tools/list`
///    handshake gives up around 60s. The budget must be strictly less so the
///    abort surfaces as a visible error to the client rather than racing the
///    client's own timeout.
///
/// 45s sits comfortably between all three: ~1200× the realistic contention
/// floor, ~15s margin under the MCP client deadline, and orders of magnitude
/// shorter than any pathological hang the original incident exhibited.
/// Tuned per cas-5c05 review (supervisor verification).
const EAGER_INIT_BUDGET: std::time::Duration = std::time::Duration::from_secs(45);

/// Eagerly open every store and the search index before serving MCP requests.
///
/// Returns an error (which `cas serve` propagates as a non-zero exit) if any
/// store fails to open or if the total init phase exceeds `EAGER_INIT_BUDGET`.
/// This converts the previously silent failure mode (server starts, registry
/// looks fine to the client, but every tool call later errors) into a loud
/// startup failure that the parent factory can detect and report.
fn eager_init_stores(core: &CasCore, cas_root: &std::path::Path) -> anyhow::Result<()> {
    let start = std::time::Instant::now();

    let step = |name: &'static str,
                f: &mut dyn FnMut() -> Result<(), anyhow::Error>|
     -> anyhow::Result<()> {
        if start.elapsed() > EAGER_INIT_BUDGET {
            anyhow::bail!(
                "store init exceeded {}s budget before reaching '{name}'. \
                 Likely cause: another process holds a write lock on \
                 {db}. Inspect with `lsof {db}` or `fuser {db}` and stop \
                 the offending process before retrying `cas serve`.",
                EAGER_INIT_BUDGET.as_secs(),
                db = cas_root.join("cas.db").display()
            );
        }
        f().with_context(|| format!("eager store init failed at '{name}'"))?;
        Ok(())
    };

    step("entry_store", &mut || {
        core.open_store().map(|_| ()).map_err(map_mcp_err)
    })?;
    step("task_store", &mut || {
        core.open_task_store().map(|_| ()).map_err(map_mcp_err)
    })?;
    step("rule_store", &mut || {
        core.open_rule_store().map(|_| ()).map_err(map_mcp_err)
    })?;
    step("skill_store", &mut || {
        core.open_skill_store().map(|_| ()).map_err(map_mcp_err)
    })?;
    step("agent_store", &mut || {
        core.open_agent_store().map(|_| ()).map_err(map_mcp_err)
    })?;
    step("entity_store", &mut || {
        core.open_entity_store().map(|_| ()).map_err(map_mcp_err)
    })?;
    step("verification_store", &mut || {
        core.open_verification_store()
            .map(|_| ())
            .map_err(map_mcp_err)
    })?;
    step("worktree_store", &mut || {
        core.open_worktree_store().map(|_| ()).map_err(map_mcp_err)
    })?;
    step("search_index", &mut || {
        core.open_search_index().map(|_| ()).map_err(map_mcp_err)
    })?;
    // Note: `core.load_config()` is intentionally not in the eager-init list.
    // It returns Config (not Result) and falls back to a default on read
    // failure, so it cannot signal anything actionable to surface here. It
    // gets called lazily via the OnceLock cache on first tool dispatch.

    eprintln!(
        "[Cassy] Stores initialized in {}ms",
        start.elapsed().as_millis()
    );
    Ok(())
}

fn map_mcp_err(e: rmcp::ErrorData) -> anyhow::Error {
    anyhow::anyhow!("{}", e.message)
}

/// Install a panic hook that writes panic info + backtrace to a daily log
/// under `{cas_root}/logs/cas-serve-{date}.log`.
///
/// Preserves the previous hook (so Rust's default stderr output still reaches
/// the MCP client) and appends a timestamped record to the file. Failures
/// during hook setup or write are swallowed — the hook must never itself
/// panic or abort serve startup.
fn install_serve_panic_hook(cas_root: &std::path::Path) {
    use std::io::Write;

    let log_dir = cas_root.join("logs");
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!(
            "[Cassy] Warning: could not create serve log dir {}: {e}",
            log_dir.display()
        );
        return;
    }
    let today = chrono::Local::now().format("%Y-%m-%d");
    let log_path = log_dir.join(format!("cas-serve-{today}.log"));
    eprintln!("[Cassy] Serve panic log: {}", log_path.display());

    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
            let pid = std::process::id();
            let agent = std::env::var("CAS_AGENT_NAME").unwrap_or_else(|_| "-".to_string());
            let session = std::env::var("CAS_SESSION_ID").unwrap_or_else(|_| "-".to_string());
            let _ = writeln!(
                f,
                "---\n{ts} pid={pid} agent={agent} session={session} PANIC"
            );
            let _ = writeln!(f, "{info}");
            let bt = std::backtrace::Backtrace::force_capture();
            let _ = writeln!(f, "{bt}");
            let _ = f.flush();
        }
        default(info);
    }));
}

/// Resolve the Cassy project root for an MCP stdio server.
///
/// Called once at `cas serve` startup.  Priority order:
///
/// 1. `CLAUDE_PROJECT_DIR` — Claude Code 2.1.139+ sets this env var when it
///    spawns `cas serve` as a stdio MCP server.  Using it avoids a cwd-mismatch
///    when Claude Code starts the server from an unexpected working directory.
/// 2. Existing `find_cas_root()` — `CAS_ROOT` override, git-worktree detection,
///    directory walk from cwd.
///
/// Logs the chosen resolution strategy at DEBUG level for diagnosability.
pub(crate) fn resolve_mcp_serve_root() -> anyhow::Result<std::path::PathBuf> {
    use crate::store::find_cas_root_from;

    if let Ok(dir) = std::env::var("CLAUDE_PROJECT_DIR") {
        let project_dir = std::path::PathBuf::from(&dir);
        if project_dir.is_dir() {
            tracing::debug!(
                path = %project_dir.display(),
                "resolve_mcp_serve_root: using CLAUDE_PROJECT_DIR"
            );
            return find_cas_root_from(&project_dir).map_err(|_| {
                anyhow::anyhow!(
                    "Cassy not initialized in CLAUDE_PROJECT_DIR={dir}. Run `cas init` first."
                )
            });
        }
        tracing::debug!(
            path = %dir,
            "resolve_mcp_serve_root: CLAUDE_PROJECT_DIR is not a readable directory, \
             falling back to cwd detection"
        );
    } else {
        tracing::debug!(
            "resolve_mcp_serve_root: CLAUDE_PROJECT_DIR not set, using cwd-based detection"
        );
    }

    crate::store::find_cas_root()
        .map_err(|_| anyhow::anyhow!("Cassy not initialized. Run `cas init` in your project first."))
}

/// Release all tasks claimed by an agent on shutdown and unregister the agent
fn release_agent_tasks(cas_root: &std::path::Path, agent_id: &str) -> anyhow::Result<()> {
    let agent_store = open_agent_store(cas_root)?;
    agent_store.graceful_shutdown(agent_id)?;
    agent_store.clear_working_epics(agent_id)?;
    agent_store.unregister(agent_id)?;
    Ok(())
}

/// Refresh the authoritative proxy snapshot used by SessionStart and health consumers.
#[cfg(feature = "mcp-proxy")]
pub async fn write_proxy_catalog_cache(
    cas_root: &std::path::Path,
    engine: &cmcp_core::ProxyEngine,
) {
    if let Err(error) = write_proxy_snapshot_cache(cas_root, engine).await {
        eprintln!("[Cassy] Failed to write proxy snapshot cache: {error}");
    }
}

#[cfg(feature = "mcp-proxy")]
const PROXY_CACHE_LOCK: &str = ".proxy_snapshot.lock";
#[cfg(feature = "mcp-proxy")]
const PROXY_SNAPSHOT_CACHE: &str = "proxy_snapshot.json";
#[cfg(feature = "mcp-proxy")]
const PROXY_SNAPSHOT_SCHEMA_VERSION: u32 = 1;
#[cfg(feature = "mcp-proxy")]
const MAX_PROXY_SNAPSHOT_BYTES: u64 = 1024 * 1024;
#[cfg(feature = "mcp-proxy")]
const PROXY_SNAPSHOT_MAX_AGE_MS: u64 = 120_000;

#[cfg(feature = "mcp-proxy")]
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProxySnapshotState {
    Ready,
    Empty,
    Unavailable,
}

#[cfg(feature = "mcp-proxy")]
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProxySnapshotFailure {
    ConfigInvalid,
    EngineStartFailed,
    EngineReloadFailed,
}

#[cfg(feature = "mcp-proxy")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProxySnapshotCache {
    pub schema_version: u32,
    pub generation: String,
    pub generated_at_ms: u64,
    pub config_fingerprint: Option<String>,
    pub state: ProxySnapshotState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<ProxySnapshotFailure>,
    pub catalog: BTreeMap<String, Vec<String>>,
    pub health: cmcp_core::ProxyHealthSnapshot,
}

#[cfg(feature = "mcp-proxy")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxySnapshotReadErrorKind {
    Missing,
    Invalid,
    ConfigInvalid,
    ConfigMismatch,
    Unavailable,
}

#[cfg(feature = "mcp-proxy")]
#[derive(Debug)]
pub struct ProxySnapshotReadError {
    pub kind: ProxySnapshotReadErrorKind,
}

#[cfg(feature = "mcp-proxy")]
impl std::fmt::Display for ProxySnapshotReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self.kind {
            ProxySnapshotReadErrorKind::Missing => "proxy snapshot is unavailable",
            ProxySnapshotReadErrorKind::Invalid => "proxy snapshot is invalid",
            ProxySnapshotReadErrorKind::ConfigInvalid => "proxy configuration is invalid",
            ProxySnapshotReadErrorKind::ConfigMismatch => "proxy snapshot is stale",
            ProxySnapshotReadErrorKind::Unavailable => "proxy startup is unavailable",
        };
        formatter.write_str(message)
    }
}

#[cfg(feature = "mcp-proxy")]
impl std::error::Error for ProxySnapshotReadError {}

#[cfg(feature = "mcp-proxy")]
fn with_proxy_cache_lock<T>(
    cas_root: &std::path::Path,
    exclusive: bool,
    operation: impl FnOnce() -> std::io::Result<T>,
) -> std::io::Result<T> {
    use fs2::FileExt;

    let lock = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(cas_root.join(PROXY_CACHE_LOCK))?;
    if exclusive {
        lock.lock_exclusive()?;
    } else {
        lock.lock_shared()?;
    }
    let result = operation();
    let unlock_result = FileExt::unlock(&lock);
    match (result, unlock_result) {
        (Err(error), _) => Err(error),
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(error)) => Err(error),
    }
}

#[cfg(feature = "mcp-proxy")]
fn atomic_write_proxy_cache_file(
    cas_root: &std::path::Path,
    name: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    atomic_write_proxy_cache_file_with(cas_root, name, bytes, |_| Ok(()))
}

#[cfg(feature = "mcp-proxy")]
fn atomic_write_proxy_cache_file_with(
    cas_root: &std::path::Path,
    name: &str,
    bytes: &[u8],
    before_commit: impl FnOnce(&std::path::Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let temp_name = format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let temp_path = cas_root.join(temp_name);
    let final_path = cas_root.join(name);
    if let Err(error) = std::fs::write(&temp_path, bytes) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(error);
    }
    if let Err(error) = before_commit(&temp_path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(error);
    }
    std::fs::OpenOptions::new()
        .read(true)
        .open(&temp_path)?
        .sync_all()?;
    let result = std::fs::rename(&temp_path, final_path);
    if result.is_err() {
        let _ = std::fs::remove_file(temp_path);
    }
    result
}

#[cfg(feature = "mcp-proxy")]
fn publish_proxy_snapshot(
    cas_root: &std::path::Path,
    snapshot: &ProxySnapshotCache,
) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(snapshot)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    with_proxy_cache_lock(cas_root, true, || {
        // This rename is the only authoritative commit point. The historical
        // two-file projections below are compatibility/forensic artifacts;
        // production readers never use them as evidence.
        atomic_write_proxy_cache_file(cas_root, PROXY_SNAPSHOT_CACHE, &bytes)?;
        if snapshot.state != ProxySnapshotState::Unavailable {
            let catalog = serde_json::to_vec(&snapshot.catalog)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            let health = serde_json::to_vec_pretty(&snapshot.health)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            // Compatibility projections are best effort and never influence
            // whether the authoritative commit succeeded.
            let _ = atomic_write_proxy_cache_file(cas_root, "proxy_catalog.json", &catalog);
            let _ = atomic_write_proxy_cache_file(cas_root, "proxy_health.json", &health);
        }
        Ok(())
    })
}

#[cfg(feature = "mcp-proxy")]
fn now_millis() -> Result<u64, std::time::SystemTimeError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
}

#[cfg(feature = "mcp-proxy")]
fn next_proxy_generation(generated_at_ms: u64) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};

    static GENERATION_SEQUENCE: AtomicU64 = AtomicU64::new(1);
    format!(
        "snapshot-{generated_at_ms}-{}-{}",
        std::process::id(),
        GENERATION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(feature = "mcp-proxy")]
pub fn proxy_config_fingerprint(config: &cmcp_core::config::Config) -> String {
    use sha2::{Digest, Sha256};

    fn field(hasher: &mut Sha256, label: &[u8], value: &[u8]) {
        hasher.update((label.len() as u64).to_be_bytes());
        hasher.update(label);
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value);
    }

    fn sorted_fields(
        hasher: &mut Sha256,
        label: &[u8],
        values: &std::collections::HashMap<String, String>,
    ) {
        for (name, value) in values.iter().collect::<BTreeMap<_, _>>() {
            field(hasher, label, name.as_bytes());
            field(hasher, b"value", value.as_bytes());
        }
    }

    let mut hasher = Sha256::new();
    hasher.update(b"cas.proxy.config.v1\0");
    for (name, server) in config.servers.iter().collect::<BTreeMap<_, _>>() {
        field(&mut hasher, b"server", name.as_bytes());
        match server {
            cmcp_core::config::ServerConfig::Stdio { command, args, env } => {
                field(&mut hasher, b"transport", b"stdio");
                field(&mut hasher, b"command", command.as_bytes());
                for arg in args {
                    field(&mut hasher, b"arg", arg.as_bytes());
                }
                sorted_fields(&mut hasher, b"env", env);
            }
            cmcp_core::config::ServerConfig::Http {
                url,
                auth,
                headers,
                oauth,
            } => {
                field(&mut hasher, b"transport", b"http");
                field(&mut hasher, b"url", url.as_bytes());
                field(
                    &mut hasher,
                    b"auth",
                    auth.as_deref().unwrap_or_default().as_bytes(),
                );
                sorted_fields(&mut hasher, b"header", headers);
                field(&mut hasher, b"oauth", if *oauth { b"1" } else { b"0" });
            }
            cmcp_core::config::ServerConfig::Sse {
                url,
                auth,
                headers,
                oauth,
            } => {
                field(&mut hasher, b"transport", b"sse");
                field(&mut hasher, b"url", url.as_bytes());
                field(
                    &mut hasher,
                    b"auth",
                    auth.as_deref().unwrap_or_default().as_bytes(),
                );
                sorted_fields(&mut hasher, b"header", headers);
                field(&mut hasher, b"oauth", if *oauth { b"1" } else { b"0" });
            }
        }
    }
    format!("sha256:{:x}", hasher.finalize())
}

#[cfg(feature = "mcp-proxy")]
fn load_proxy_config(cas_root: &std::path::Path) -> anyhow::Result<cmcp_core::config::Config> {
    let proxy_path = cas_root.join("proxy.toml");
    cmcp_core::config::Config::load_merged(proxy_path.exists().then_some(proxy_path.as_path()))
}

#[cfg(feature = "mcp-proxy")]
fn sanitized_catalog(catalog: BTreeMap<String, Vec<String>>) -> BTreeMap<String, Vec<String>> {
    const MAX_SERVERS: usize = 64;
    const MAX_TOOLS: usize = 256;
    let public_servers = cas_types::public_upstream_ids(catalog.keys().map(String::as_str));
    let mut sanitized = BTreeMap::new();
    for (raw_server, raw_tools) in catalog.into_iter().take(MAX_SERVERS) {
        let Some(server) = public_servers.get(&raw_server).cloned() else {
            continue;
        };
        let public_tools = cas_types::public_tool_ids(raw_tools.iter().map(String::as_str));
        let mut tools = raw_tools
            .iter()
            .take(MAX_TOOLS)
            .filter_map(|tool| public_tools.get(tool).cloned())
            .collect::<Vec<_>>();
        tools.sort();
        tools.dedup();
        sanitized.insert(server, tools);
    }
    sanitized
}

#[cfg(feature = "mcp-proxy")]
fn read_proxy_snapshot_manifest(
    cas_root: &std::path::Path,
) -> Result<ProxySnapshotCache, ProxySnapshotReadError> {
    let bytes = with_proxy_cache_lock(cas_root, false, || {
        let path = cas_root.join(PROXY_SNAPSHOT_CACHE);
        let metadata = std::fs::metadata(&path)?;
        if metadata.len() > MAX_PROXY_SNAPSHOT_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "proxy snapshot exceeds the bounded cache size",
            ));
        }
        std::fs::read(path)
    })
    .map_err(|error| ProxySnapshotReadError {
        kind: if error.kind() == std::io::ErrorKind::NotFound {
            ProxySnapshotReadErrorKind::Missing
        } else {
            ProxySnapshotReadErrorKind::Invalid
        },
    })?;
    let mut snapshot: ProxySnapshotCache =
        serde_json::from_slice(&bytes).map_err(|_| ProxySnapshotReadError {
            kind: ProxySnapshotReadErrorKind::Invalid,
        })?;
    let fingerprint_is_safe = snapshot.config_fingerprint.as_deref().is_none_or(|value| {
        value.len() == 71
            && value.starts_with("sha256:")
            && value[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
    });
    if snapshot.schema_version != PROXY_SNAPSHOT_SCHEMA_VERSION
        || snapshot.generation.len() > 96
        || !snapshot
            .generation
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || snapshot.health.generated_at_ms != snapshot.generated_at_ms
        || !fingerprint_is_safe
        || (snapshot.state == ProxySnapshotState::Empty
            && (!snapshot.catalog.is_empty()
                || snapshot.health.healthy != 0
                || snapshot.health.degraded != 0
                || !snapshot.health.servers.is_empty()))
        || (snapshot.state == ProxySnapshotState::Unavailable
            && (!snapshot.catalog.is_empty()
                || snapshot.health.healthy != 0
                || snapshot.health.degraded != 0
                || !snapshot.health.servers.is_empty()
                || snapshot.failure.is_none()))
        || (snapshot.state != ProxySnapshotState::Unavailable && snapshot.failure.is_some())
    {
        return Err(ProxySnapshotReadError {
            kind: ProxySnapshotReadErrorKind::Invalid,
        });
    }
    snapshot.catalog = sanitized_catalog(snapshot.catalog);
    snapshot.health = snapshot.health.sanitized();
    Ok(snapshot)
}

#[cfg(feature = "mcp-proxy")]
pub fn read_proxy_snapshot_cache(
    cas_root: &std::path::Path,
) -> Result<ProxySnapshotCache, ProxySnapshotReadError> {
    let snapshot = read_proxy_snapshot_manifest(cas_root)?;
    let observed_at_ms = now_millis().map_err(|_| ProxySnapshotReadError {
        kind: ProxySnapshotReadErrorKind::Invalid,
    })?;
    if snapshot.generated_at_ms > observed_at_ms.saturating_add(30_000) {
        return Err(ProxySnapshotReadError {
            kind: ProxySnapshotReadErrorKind::Invalid,
        });
    }
    if snapshot.state == ProxySnapshotState::Ready
        && observed_at_ms.saturating_sub(snapshot.generated_at_ms) > PROXY_SNAPSHOT_MAX_AGE_MS
    {
        return Err(ProxySnapshotReadError {
            kind: ProxySnapshotReadErrorKind::ConfigMismatch,
        });
    }
    let config = load_proxy_config(cas_root).map_err(|_| ProxySnapshotReadError {
        kind: ProxySnapshotReadErrorKind::ConfigInvalid,
    })?;
    if snapshot.config_fingerprint.as_deref() != Some(proxy_config_fingerprint(&config).as_str()) {
        return Err(ProxySnapshotReadError {
            kind: ProxySnapshotReadErrorKind::ConfigMismatch,
        });
    }
    if snapshot.state == ProxySnapshotState::Unavailable {
        return Err(ProxySnapshotReadError {
            kind: ProxySnapshotReadErrorKind::Unavailable,
        });
    }
    let expected_state = if config.servers.is_empty() {
        ProxySnapshotState::Empty
    } else {
        ProxySnapshotState::Ready
    };
    if snapshot.state != expected_state {
        return Err(ProxySnapshotReadError {
            kind: ProxySnapshotReadErrorKind::ConfigMismatch,
        });
    }
    Ok(snapshot)
}

#[cfg(feature = "mcp-proxy")]
pub fn read_proxy_catalog_cache(cas_root: &std::path::Path) -> std::io::Result<Vec<u8>> {
    let snapshot = read_proxy_snapshot_cache(cas_root)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()))?;
    serde_json::to_vec(&snapshot.catalog)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

#[cfg(feature = "mcp-proxy")]
pub fn read_proxy_health_cache(cas_root: &std::path::Path) -> std::io::Result<Vec<u8>> {
    let snapshot = read_proxy_snapshot_cache(cas_root)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()))?;
    serde_json::to_vec_pretty(&snapshot.health)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

#[cfg(feature = "mcp-proxy")]
async fn proxy_catalog(engine: &cmcp_core::ProxyEngine) -> BTreeMap<String, Vec<String>> {
    let servers = engine.catalog_entries_by_server().await;
    let simplified = servers
        .into_iter()
        .map(|(server, entries)| {
            let names = entries.into_iter().map(|entry| entry.name).collect();
            (server, names)
        })
        .collect();
    sanitized_catalog(simplified)
}

#[cfg(feature = "mcp-proxy")]
pub async fn write_proxy_snapshot_cache(
    cas_root: &std::path::Path,
    engine: &cmcp_core::ProxyEngine,
) -> anyhow::Result<()> {
    let config = load_proxy_config(cas_root)?;
    write_proxy_snapshot_cache_for_config(cas_root, engine, &config).await
}

#[cfg(feature = "mcp-proxy")]
pub async fn write_proxy_snapshot_cache_for_config(
    cas_root: &std::path::Path,
    engine: &cmcp_core::ProxyEngine,
    config: &cmcp_core::config::Config,
) -> anyhow::Result<()> {
    let health = engine.health_snapshot().await.sanitized();
    let generated_at_ms = health.generated_at_ms;
    if generated_at_ms == 0 {
        anyhow::bail!("proxy snapshot clock is unavailable");
    }
    let snapshot = ProxySnapshotCache {
        schema_version: PROXY_SNAPSHOT_SCHEMA_VERSION,
        generation: next_proxy_generation(generated_at_ms),
        generated_at_ms,
        config_fingerprint: Some(proxy_config_fingerprint(config)),
        state: if config.servers.is_empty() {
            ProxySnapshotState::Empty
        } else {
            ProxySnapshotState::Ready
        },
        failure: None,
        catalog: proxy_catalog(engine).await,
        health,
    };
    publish_proxy_snapshot(cas_root, &snapshot)?;
    Ok(())
}

#[cfg(feature = "mcp-proxy")]
fn empty_health(generated_at_ms: u64) -> cmcp_core::ProxyHealthSnapshot {
    cmcp_core::ProxyHealthSnapshot {
        session_id: format!("proxy-{}-{generated_at_ms}-0", std::process::id()),
        generated_at_ms,
        healthy: 0,
        degraded: 0,
        servers: Vec::new(),
    }
}

#[cfg(feature = "mcp-proxy")]
fn publish_non_live_proxy_snapshot(
    cas_root: &std::path::Path,
    config: Option<&cmcp_core::config::Config>,
    state: ProxySnapshotState,
    failure: Option<ProxySnapshotFailure>,
) -> anyhow::Result<()> {
    let generated_at_ms = now_millis()?;
    publish_proxy_snapshot(
        cas_root,
        &ProxySnapshotCache {
            schema_version: PROXY_SNAPSHOT_SCHEMA_VERSION,
            generation: next_proxy_generation(generated_at_ms),
            generated_at_ms,
            config_fingerprint: config.map(proxy_config_fingerprint),
            state,
            failure,
            catalog: BTreeMap::new(),
            health: empty_health(generated_at_ms),
        },
    )?;
    Ok(())
}

#[cfg(feature = "mcp-proxy")]
pub fn write_empty_proxy_snapshot_cache(cas_root: &std::path::Path) -> anyhow::Result<()> {
    let config = load_proxy_config(cas_root)?;
    publish_non_live_proxy_snapshot(cas_root, Some(&config), ProxySnapshotState::Empty, None)
}

#[cfg(feature = "mcp-proxy")]
pub fn write_unavailable_proxy_snapshot_cache(
    cas_root: &std::path::Path,
    config: Option<&cmcp_core::config::Config>,
    failure: ProxySnapshotFailure,
) -> anyhow::Result<()> {
    publish_non_live_proxy_snapshot(
        cas_root,
        config,
        ProxySnapshotState::Unavailable,
        Some(failure),
    )
}

/// Persist credential-free optional-upstream health for factory preflight.
#[cfg(feature = "mcp-proxy")]
pub async fn write_proxy_health_cache(cas_root: &std::path::Path, engine: &cmcp_core::ProxyEngine) {
    if let Err(error) = write_proxy_snapshot_cache(cas_root, engine).await {
        tracing::debug!(error = %error, "failed to write MCP proxy snapshot cache");
    }
}

// =============================================================================
// Unit tests for resolve_mcp_serve_root (cas-7cc3)
// =============================================================================
#[cfg(test)]
mod tests {
    use super::{
        ensure_mcp_schema, open_startup_pull_entry_store, open_startup_pull_task_store,
        resolve_mcp_serve_root,
    };
    use crate::cloud::{CloudConfig, EntityType, SyncQueue};
    use crate::store::{init_cas_dir, open_store, open_task_store};
    use crate::test_support::TestEnvGuard;
    use crate::types::{Entry, Task};
    use std::sync::Arc;
    use tempfile::TempDir;

    #[cfg(feature = "mcp-proxy")]
    #[tokio::test]
    async fn boot_policy_is_fail_closed_exact_and_gateway_bound() {
        use cmcp_core::config::{
            Config as ProxyConfig, ExternalProductionVerificationConfig, ExternalToolConfig,
        };

        let engine = cmcp_core::ProxyEngine::from_configs(Default::default())
            .await
            .unwrap();
        let mut config = ProxyConfig::default();
        config.allowlist = vec![
            ExternalToolConfig {
                server: "github".to_string(),
                tool: "list_issues".to_string(),
            },
            ExternalToolConfig {
                server: "viktor".to_string(),
                tool: "ask_viktor".to_string(),
            },
            ExternalToolConfig {
                server: "viktor".to_string(),
                tool: "wait_for_run".to_string(),
            },
        ];
        config.delegation.external_production_verification =
            Some(ExternalProductionVerificationConfig {
                server: "viktor".to_string(),
                start_tool: "ask_viktor".to_string(),
                wait_tool: "wait_for_run".to_string(),
                reserved_amount: 1,
                max_per_run: 1,
                max_active_per_factory_session: 4,
                max_active_per_epic: 2,
                timeout_seconds: 120,
            });
        super::install_proxy_policy(&engine, &config).await;
        let caller = cmcp_core::ProxyCaller {
            agent_id: "supervisor".to_string(),
            role: crate::types::AgentRole::Supervisor,
            session_id: "supervisor".to_string(),
            factory_session: Some("factory-1".to_string()),
            active_task_ids: Vec::new(),
        };

        let foreign = engine
            .call_tool(&caller, "viktor-shadow", "ask_viktor", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(foreign.contains("external tool is not explicitly allowlisted"));
        let direct_delegation = engine
            .call_tool(&caller, "viktor", "ask_viktor", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(direct_delegation.contains("requires the registered supervisor gateway"));

        let ordinary = engine
            .call_tool(&caller, "github", "list_issues", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(ordinary.contains("server 'github' not connected"));
        assert!(!ordinary.contains("proxy policy denied"));
        let delegated = engine
            .call_external_production_verification_tool(&caller, "viktor", "ask_viktor", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(delegated.contains("server 'viktor' not connected"));
        assert!(!delegated.contains("proxy policy denied"));

        let audit = engine.policy_audit();
        assert_eq!(audit.iter().filter(|entry| entry.allowed).count(), 2);
        assert_eq!(audit.iter().filter(|entry| !entry.allowed).count(), 2);
    }

    #[cfg(feature = "mcp-proxy")]
    #[tokio::test]
    async fn managed_viktor_conversation_policy_is_exact_and_audits_worker_task_attribution() {
        use cmcp_core::config::{Config as ProxyConfig, VIKTOR_CONVERSATION_TOOLS, VIKTOR_SERVER};

        let engine = cmcp_core::ProxyEngine::from_configs(Default::default())
            .await
            .unwrap();
        let mut config = ProxyConfig::default();
        assert!(config.ensure_viktor_managed_default());
        super::install_proxy_policy(&engine, &config).await;
        let caller = cmcp_core::ProxyCaller {
            agent_id: "worker-session".to_string(),
            role: crate::types::AgentRole::Worker,
            session_id: "worker-session".to_string(),
            factory_session: Some("factory-2428".to_string()),
            active_task_ids: vec!["cas-2428".to_string()],
        };

        for tool in VIKTOR_CONVERSATION_TOOLS {
            let error = engine
                .call_tool(&caller, VIKTOR_SERVER, tool, None)
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("not connected"), "{tool}: {error}");
            assert!(!error.contains("policy denied"), "{tool}: {error}");
        }
        let forbidden = engine
            .call_tool(&caller, VIKTOR_SERVER, "get_file_download_url", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(forbidden.contains("external tool is not explicitly allowlisted"));

        let audit = engine.policy_audit();
        assert_eq!(audit.len(), VIKTOR_CONVERSATION_TOOLS.len() + 1);
        for receipt in audit.iter().take(VIKTOR_CONVERSATION_TOOLS.len()) {
            assert!(receipt.allowed);
            assert_eq!(receipt.caller.agent_id, "worker-session");
            assert_eq!(receipt.caller.active_task_ids, ["cas-2428"]);
            assert!(receipt.timestamp_ms > 0);
        }
        assert!(!audit.last().unwrap().allowed);
    }

    fn make_m233_pending(cas_root: &std::path::Path, wrong_ledger_identity: bool) {
        let conn = rusqlite::Connection::open(cas_root.join("cas.db")).unwrap();
        conn.execute("ALTER TABLE tasks DROP COLUMN terminal_outcome", [])
            .unwrap();
        conn.execute("DELETE FROM cas_migrations WHERE id = 233", [])
            .unwrap();
        if wrong_ledger_identity {
            conn.execute(
                "INSERT INTO cas_migrations (id, name, subsystem, applied_at)
                 VALUES (233, 'wrong_migration_identity', 'tasks', 'TEST')",
                [],
            )
            .unwrap();
        }
    }

    #[test]
    fn mcp_startup_applies_pending_schema_before_store_opening() {
        let temp = TempDir::new().unwrap();
        let cas_root = init_cas_dir(temp.path()).unwrap();
        make_m233_pending(&cas_root, false);

        ensure_mcp_schema(&cas_root).expect("MCP startup should repair a pending migration");

        let status = crate::migration::check_migrations(&cas_root).unwrap();
        assert!(status.pending.is_empty());
        let conn = rusqlite::Connection::open(cas_root.join("cas.db")).unwrap();
        assert!(cas_store::shared_db::column_exists(
            &conn,
            "tasks",
            "terminal_outcome"
        ));
    }

    #[test]
    fn casb123_mcp_startup_arms_parent_watchdog_before_schema_migration() {
        let source = include_str!("runtime.rs");
        let body = source
            .split_once("async fn run_server_impl()")
            .expect("run_server_impl source")
            .1;
        let watchdog = body
            .find("parent_watchdog::spawn()")
            .expect("startup watchdog arm");
        let migration = body
            .find("ensure_mcp_schema(&cas_root)?")
            .expect("startup schema check");
        assert!(
            watchdog < migration,
            "parent watchdog must be armed before a schema migration can hold the database"
        );
    }

    #[test]
    fn mcp_startup_refusal_names_binary_schema_pending_migration_and_fix_command() {
        let temp = TempDir::new().unwrap();
        let cas_root = init_cas_dir(temp.path()).unwrap();
        make_m233_pending(&cas_root, true);

        let error = ensure_mcp_schema(&cas_root)
            .expect_err("a corrupt migration ledger must refuse MCP startup")
            .to_string();
        assert!(error.contains("binary"), "missing binary version: {error}");
        // Derived from the migration ledger rather than hardcoded: the refusal
        // names whichever schema version this binary requires, so adding a
        // migration must not require editing this assertion.
        let required = crate::migration::MIGRATIONS
            .last()
            .map(|migration| migration.id)
            .expect("at least one migration");
        assert!(
            error.contains(&format!("requires schema v{required}")),
            "missing schema requirement: {error}"
        );
        assert!(
            error.contains("m233 `tasks_add_terminal_outcome`"),
            "missing pending migration name: {error}"
        );
        assert!(
            error.contains("cas update --schema-only"),
            "missing remediation command: {error}"
        );
    }

    #[test]
    fn startup_pull_apply_leaves_personal_and_team_queues_empty() {
        let temp = TempDir::new().unwrap();
        let cas_root = init_cas_dir(temp.path()).unwrap();
        let team_id = "550e8400-e29b-41d4-a716-446655440000";

        let mut cloud = CloudConfig::default();
        cloud.token = Some("test-token".to_string());
        cloud.team_id = Some(team_id.to_string());
        cloud.save_to_cas_dir(&cas_root).unwrap();

        let queue = Arc::new(SyncQueue::open(&cas_root).unwrap());
        queue.init().unwrap();

        // These adds stand in for the Created branch of CloudSyncer::pull's
        // remote apply. Both entities are team-eligible, so a syncing wrapper
        // would recreate the personal + team pair observed in production.
        let remote_entry = Entry::new("p-startup-pull".to_string(), "remote entry".to_string());
        let remote_task = Task::new("cas-startup-pull".to_string(), "remote task".to_string());
        open_startup_pull_entry_store(&cas_root)
            .unwrap()
            .add(&remote_entry)
            .unwrap();
        open_startup_pull_task_store(&cas_root)
            .unwrap()
            .add(&remote_task)
            .unwrap();

        for entity_type in [EntityType::Entry, EntityType::Task] {
            assert!(
                queue
                    .pending_for_entity_type(Some(entity_type), 10, 10)
                    .unwrap()
                    .is_empty(),
                "startup pull must not enqueue personal {entity_type:?} rows"
            );
        }
        assert!(
            queue.pending_for_team(team_id, 10, 10).unwrap().is_empty(),
            "startup pull must not enqueue team rows"
        );

        // Local edits retain their existing Syncing* behavior: each eligible
        // entity is still queued once for the personal and team scopes.
        open_store(&cas_root)
            .unwrap()
            .add(&Entry::new(
                "p-local-edit".to_string(),
                "local entry".to_string(),
            ))
            .unwrap();
        open_task_store(&cas_root)
            .unwrap()
            .add(&Task::new(
                "cas-local-edit".to_string(),
                "local task".to_string(),
            ))
            .unwrap();
        for entity_type in [EntityType::Entry, EntityType::Task] {
            assert_eq!(
                queue
                    .pending_for_entity_type(Some(entity_type), 10, 10)
                    .unwrap()
                    .len(),
                1,
                "local edit must enqueue one personal {entity_type:?} row"
            );
        }
        assert_eq!(queue.pending_for_team(team_id, 10, 10).unwrap().len(), 2);
    }

    #[cfg(feature = "mcp-proxy")]
    #[tokio::test(flavor = "current_thread")]
    async fn empty_proxy_state_clears_stale_catalog_and_writes_health() {
        let tmp = TempDir::new().unwrap();
        let cas_root = tmp.path();
        let home = tmp.path().join("home");
        let config_home = home.join(".config");
        std::fs::create_dir_all(&config_home).unwrap();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("HOME", Some(home.to_str().unwrap())),
            ("XDG_CONFIG_HOME", Some(config_home.to_str().unwrap())),
        ]);
        std::fs::write(cas_root.join("proxy_catalog.json"), r#"{"stale":["tool"]}"#).unwrap();
        std::fs::write(
            cas_root.join("proxy_health.json"),
            r#"{"session_id":"proxy-1","generated_at_ms":1,"healthy":1,"degraded":0,"servers":[{"name":"stale","transport":"http","state":"healthy","attempts":1,"consecutive_failures":0,"tool_count":1,"last_error_code":null,"last_attempt_at_ms":1,"next_retry_at_ms":null}]}"#,
        )
        .unwrap();

        super::write_empty_proxy_snapshot_cache(cas_root).unwrap();

        let catalog: serde_json::Value =
            serde_json::from_slice(&super::read_proxy_catalog_cache(cas_root).unwrap()).unwrap();
        assert_eq!(catalog, serde_json::json!({}));

        let health: serde_json::Value =
            serde_json::from_slice(&super::read_proxy_health_cache(cas_root).unwrap()).unwrap();
        assert_eq!(health["healthy"], 0);
        assert_eq!(health["degraded"], 0);
        assert!(
            health["session_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
        );
        assert_eq!(health["servers"], serde_json::json!([]));
    }

    #[cfg(feature = "mcp-proxy")]
    #[test]
    fn failed_manifest_commit_never_exposes_mixed_compatibility_members() {
        let tmp = TempDir::new().unwrap();
        let cas_root = tmp.path();
        let home = tmp.path().join("home");
        let config_home = home.join(".config");
        std::fs::create_dir_all(&config_home).unwrap();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("HOME", Some(home.to_str().unwrap())),
            ("XDG_CONFIG_HOME", Some(config_home.to_str().unwrap())),
        ]);
        let config = cmcp_core::config::Config::default();
        super::publish_non_live_proxy_snapshot(
            cas_root,
            Some(&config),
            super::ProxySnapshotState::Empty,
            None,
        )
        .unwrap();
        let old = super::read_proxy_snapshot_cache(cas_root).unwrap();
        let mut replacement = old.clone();
        replacement.generated_at_ms += 1;
        replacement.generation = super::next_proxy_generation(replacement.generated_at_ms);
        replacement.health.generated_at_ms = replacement.generated_at_ms;
        replacement.health.session_id = "proxy-replacement".to_string();
        let bytes = serde_json::to_vec_pretty(&replacement).unwrap();

        let error = super::with_proxy_cache_lock(cas_root, true, || {
            // Simulate the historical first member changing before the
            // authoritative manifest commit is interrupted.
            std::fs::write(cas_root.join("proxy_catalog.json"), br#"{"new":["tool"]}"#)?;
            super::atomic_write_proxy_cache_file_with(
                cas_root,
                super::PROXY_SNAPSHOT_CACHE,
                &bytes,
                |_| Err(std::io::Error::other("injected before manifest commit")),
            )
        })
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Other);

        let observed = super::read_proxy_snapshot_cache(cas_root).unwrap();
        assert_eq!(observed.generation, old.generation);
        assert_eq!(observed.health.session_id, old.health.session_id);
        super::publish_proxy_snapshot(cas_root, &replacement).unwrap();
        let recovered = super::read_proxy_snapshot_cache(cas_root).unwrap();
        assert_eq!(recovered.generation, replacement.generation);
        assert_eq!(recovered.generated_at_ms, replacement.generated_at_ms);
    }

    #[cfg(feature = "mcp-proxy")]
    #[test]
    fn legacy_or_incomplete_members_are_never_authoritative() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("proxy_catalog.json"), r#"{"old":["tool"]}"#).unwrap();
        std::fs::write(
            tmp.path().join("proxy_health.json"),
            r#"{"session_id":"proxy-1","generated_at_ms":1,"healthy":1,"degraded":0,"servers":[]}"#,
        )
        .unwrap();
        assert_eq!(
            super::read_proxy_snapshot_cache(tmp.path())
                .unwrap_err()
                .kind,
            super::ProxySnapshotReadErrorKind::Missing
        );

        std::fs::write(
            tmp.path().join(super::PROXY_SNAPSHOT_CACHE),
            r#"{"schema_version":1,"generation":"snapshot-incomplete"}"#,
        )
        .unwrap();
        assert_eq!(
            super::read_proxy_snapshot_cache(tmp.path())
                .unwrap_err()
                .kind,
            super::ProxySnapshotReadErrorKind::Invalid
        );
    }

    #[cfg(feature = "mcp-proxy")]
    #[test]
    fn config_fingerprint_is_deterministic_bounded_and_detects_private_drift() {
        let mut first = cmcp_core::config::Config::default();
        first.add_server(
            "unsafe/name".to_string(),
            cmcp_core::config::ServerConfig::Http {
                url: "https://user@example.invalid/private".to_string(),
                auth: Some("first-secret".to_string()),
                headers: std::collections::HashMap::from([(
                    "Authorization".to_string(),
                    "Bearer first-secret".to_string(),
                )]),
                oauth: false,
            },
        );
        let mut second = first.clone();
        if let Some(cmcp_core::config::ServerConfig::Http { auth, .. }) =
            second.servers.get_mut("unsafe/name")
        {
            *auth = Some("second-secret".to_string());
        }
        let first_fingerprint = super::proxy_config_fingerprint(&first);
        assert_eq!(first_fingerprint, super::proxy_config_fingerprint(&first));
        assert_eq!(first_fingerprint.len(), 71);
        assert!(first_fingerprint.starts_with("sha256:"));
        assert_ne!(first_fingerprint, super::proxy_config_fingerprint(&second));
        for forbidden in ["unsafe/name", "example.invalid", "first-secret"] {
            assert!(!first_fingerprint.contains(forbidden));
        }
    }

    #[cfg(feature = "mcp-proxy")]
    #[test]
    fn expired_or_future_manifest_is_fail_honest_when_configuration_is_unchanged() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let config_home = home.join(".config");
        std::fs::create_dir_all(&config_home).unwrap();
        let _env = TestEnvGuard::with_optional_vars(&[
            ("HOME", Some(home.to_str().unwrap())),
            ("XDG_CONFIG_HOME", Some(config_home.to_str().unwrap())),
        ]);
        let mut config = cmcp_core::config::Config::default();
        config.add_server(
            "optional".to_string(),
            cmcp_core::config::ServerConfig::Http {
                url: "https://example.invalid/mcp".to_string(),
                auth: None,
                headers: std::collections::HashMap::new(),
                oauth: false,
            },
        );
        config.save_to(&tmp.path().join("proxy.toml")).unwrap();
        super::publish_non_live_proxy_snapshot(
            tmp.path(),
            Some(&config),
            super::ProxySnapshotState::Ready,
            None,
        )
        .unwrap();
        let mut snapshot = super::read_proxy_snapshot_cache(tmp.path()).unwrap();
        snapshot.generated_at_ms = super::now_millis()
            .unwrap()
            .saturating_sub(super::PROXY_SNAPSHOT_MAX_AGE_MS + 1);
        snapshot.health.generated_at_ms = snapshot.generated_at_ms;
        std::fs::write(
            tmp.path().join(super::PROXY_SNAPSHOT_CACHE),
            serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
        assert_eq!(
            super::read_proxy_snapshot_cache(tmp.path())
                .unwrap_err()
                .kind,
            super::ProxySnapshotReadErrorKind::ConfigMismatch
        );

        // Stay a full minute beyond the accepted 30s skew so serialization,
        // filesystem I/O, and scheduler delay cannot cross the boundary.
        snapshot.generated_at_ms = super::now_millis().unwrap().saturating_add(90_000);
        snapshot.health.generated_at_ms = snapshot.generated_at_ms;
        std::fs::write(
            tmp.path().join(super::PROXY_SNAPSHOT_CACHE),
            serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
        assert_eq!(
            super::read_proxy_snapshot_cache(tmp.path())
                .unwrap_err()
                .kind,
            super::ProxySnapshotReadErrorKind::Invalid
        );
    }

    /// When CLAUDE_PROJECT_DIR is set to a directory that contains a `.cas/`,
    /// resolve_mcp_serve_root must return that `.cas/` path even if the process
    /// cwd is somewhere completely different.
    #[test]
    fn resolves_from_claude_project_dir_when_set() {
        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().canonicalize().unwrap();
        init_cas_dir(&tmp_path).unwrap();

        let _env = TestEnvGuard::with_optional_vars(&[
            ("CLAUDE_PROJECT_DIR", Some(tmp_path.to_str().unwrap())),
            ("CAS_ROOT", None),
        ]);
        let result = resolve_mcp_serve_root()
            .expect("should succeed when CLAUDE_PROJECT_DIR points to initialized project");
        assert_eq!(
            result,
            tmp_path.join(".cas"),
            "should resolve from CLAUDE_PROJECT_DIR, not cwd"
        );
    }

    /// When CLAUDE_PROJECT_DIR points at a path that does not exist (not a
    /// directory), the function must fall back to cwd / CAS_ROOT detection.
    #[test]
    fn falls_back_when_claude_project_dir_is_invalid() {
        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().canonicalize().unwrap();
        init_cas_dir(&tmp_path).unwrap();
        let cas_root_str = tmp_path.join(".cas").to_string_lossy().into_owned();

        let _env = TestEnvGuard::with_optional_vars(&[
            (
                "CLAUDE_PROJECT_DIR",
                Some("/nonexistent/path/that/definitely/does/not/exist"),
            ),
            ("CAS_ROOT", Some(&cas_root_str)),
        ]);
        let result = resolve_mcp_serve_root().expect("should succeed via CAS_ROOT fallback");
        assert_eq!(
            result,
            tmp_path.join(".cas"),
            "should fall back to CAS_ROOT when CLAUDE_PROJECT_DIR is invalid"
        );
    }

    /// When CLAUDE_PROJECT_DIR points to a real, readable directory that has
    /// NOT been `cas init`-ed (no `.cas/` subdirectory exists inside it), the
    /// function must return an Err whose message explicitly mentions
    /// CLAUDE_PROJECT_DIR — so the user knows which path to initialise.
    #[test]
    fn errors_when_claude_project_dir_has_no_cas_dir() {
        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().canonicalize().unwrap();
        // Deliberately do NOT call init_cas_dir — no .cas/ exists here.

        let _env = TestEnvGuard::with_optional_vars(&[
            ("CLAUDE_PROJECT_DIR", Some(tmp_path.to_str().unwrap())),
            ("CAS_ROOT", None),
        ]);
        let err =
            resolve_mcp_serve_root().expect_err("should fail: CLAUDE_PROJECT_DIR has no .cas/");
        let msg = err.to_string();
        assert!(
            msg.contains("CLAUDE_PROJECT_DIR"),
            "error message should mention CLAUDE_PROJECT_DIR so the user knows which \
             path to run `cas init` in; got: {msg}"
        );
    }

    /// When CLAUDE_PROJECT_DIR is not set, the function must still work via the
    /// normal CAS_ROOT / cwd-walk path.
    #[test]
    fn falls_back_to_cas_root_when_claude_project_dir_absent() {
        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().canonicalize().unwrap();
        init_cas_dir(&tmp_path).unwrap();
        let cas_root_str = tmp_path.join(".cas").to_string_lossy().into_owned();

        let _env = TestEnvGuard::with_optional_vars(&[
            ("CLAUDE_PROJECT_DIR", None),
            ("CAS_ROOT", Some(&cas_root_str)),
        ]);
        let result = resolve_mcp_serve_root().expect("should succeed via CAS_ROOT fallback");
        assert_eq!(
            result,
            tmp_path.join(".cas"),
            "should resolve via CAS_ROOT when CLAUDE_PROJECT_DIR absent"
        );
    }

    /// When CLAUDE_PROJECT_DIR is set to the path of a Cassy factory worktree
    /// (.cas/worktrees/<name>/), resolve_mcp_serve_root must return the PARENT
    /// repo's .cas/ directory — not a nested .cas/ inside the worktree.
    ///
    /// This is the regression guard for the core bug (cas-9db0): before the fix,
    /// `find_cas_root()` would walk up from the worktree path and find the main
    /// repo's .cas/ by luck, but if a nested .cas/ were present it would stop
    /// there. The `find_cas_root_from_cas_worktree()` fast-path now handles this
    /// reliably by detecting `.cas/worktrees/` in the path string.
    ///
    /// Claude Code 2.1.139+ sets CLAUDE_PROJECT_DIR to the project root where
    /// `cas serve` is launched, which for factory workers is the worktree root.
    #[test]
    fn resolves_to_parent_cas_when_claude_project_dir_is_worktree() {
        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().canonicalize().unwrap();

        // Initialize Cassy in the project root
        init_cas_dir(&tmp_path).unwrap();

        // Create the Cassy factory worktree directory (.cas/worktrees/<name>/)
        let worktree_path = tmp_path.join(".cas/worktrees/fox");
        std::fs::create_dir_all(&worktree_path).unwrap();

        let _env = TestEnvGuard::with_optional_vars(&[
            ("CLAUDE_PROJECT_DIR", Some(worktree_path.to_str().unwrap())),
            ("CAS_ROOT", None),
        ]);
        let result = resolve_mcp_serve_root()
            .expect("should succeed when CLAUDE_PROJECT_DIR is a Cassy worktree path");
        assert_eq!(
            result,
            tmp_path.join(".cas"),
            "worktree path must resolve to PARENT .cas/, not a nested .cas/ inside the worktree"
        );
    }

    /// Same as above but verifies that a SUBDIRECTORY of the worktree also
    /// resolves correctly. Workers' cwd may be a deeply nested path inside the
    /// worktree when CLAUDE_PROJECT_DIR is inherited from the outer shell.
    #[test]
    fn resolves_to_parent_cas_when_claude_project_dir_is_worktree_subdir() {
        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().canonicalize().unwrap();

        init_cas_dir(&tmp_path).unwrap();

        // Simulate a deeply nested cwd inside the worktree
        let nested_path = tmp_path.join(".cas/worktrees/fox/src/deep/nested");
        std::fs::create_dir_all(&nested_path).unwrap();

        let _env = TestEnvGuard::with_optional_vars(&[
            ("CLAUDE_PROJECT_DIR", Some(nested_path.to_str().unwrap())),
            ("CAS_ROOT", None),
        ]);
        let result = resolve_mcp_serve_root()
            .expect("should succeed from a nested path inside a Cassy worktree");
        assert_eq!(
            result,
            tmp_path.join(".cas"),
            "nested worktree path must resolve to parent .cas/"
        );
    }
}
