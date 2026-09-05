use crate::config::Config;
use crate::store::find_cas_root;
use crate::ui::factory::{
    BootConfig, DaemonConfig, FactoryConfig, ForkFirstResult, NotifyBackend, NotifyConfig, attach,
    daemon_log_path, daemonize, fork_first_daemon, run_boot_screen_client,
};
use anyhow::{Result, bail};
use cas_factory::routing::CapabilitySnapshot;
use cas_factory::spec_resolver::{ConfigSources, resolve_specs, resolve_supervisor_spec};
use std::time::Duration;

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_daemon(
    session: &str,
    cwd: &std::path::Path,
    workers: u8,
    no_worktrees: bool,
    worktree_root: Option<std::path::PathBuf>,
    notify: bool,
    tabbed: bool,
    record: bool,
    phone_home: bool,
    supervisor_cli: cas_mux::SupervisorCli,
    worker_cli: cas_mux::SupervisorCli,
    foreground: bool,
    boot_progress: bool,
    supervisor_name: Option<String>,
    worker_names: Vec<String>,
    worker_spec_jsons: Vec<String>,
    supervisor_spec_json: Option<String>,
) -> Result<()> {
    let cas_root = find_cas_root()?;
    let cas_config = Config::load(&cas_root).unwrap_or_default();

    // Register the project root in the host-scoped known_repos registry so
    // a later `cas sweep-all` can discover it. Non-fatal best-effort upsert.
    // `cas_root` is the `.cas` dir; the repo root is its parent.
    if let Some(repo_root) = cas_root.parent() {
        crate::store::known_repos::register_repo(repo_root);
    }

    // Opportunistic cross-repo sweep (EPIC cas-7c88 Unit 3) — debounced
    // via `~/.cas/last_global_sweep`. Dispatched on a detached OS thread
    // because execute_daemon runs synchronously before Tokio is available
    // here. Any panic on the sweep thread is caught; any error is logged.
    let wt_cfg = cas_config.worktrees().clone();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(|| {
            match crate::worktree::sweep::opportunistic::run_if_due(&wt_cfg) {
                Ok(Some(summary)) => tracing::info!(
                    repos = summary.repos_visited,
                    reclaimed = summary.reclaimed,
                    salvaged = summary.salvaged,
                    "opportunistic sweep complete"
                ),
                Ok(None) => {}
                Err(e) => tracing::error!(error = %e, "opportunistic sweep failed"),
            }
        });
        if result.is_err() {
            tracing::error!("opportunistic sweep panicked — swallowed");
        }
    });

    let effective_workers = if worker_names.is_empty() {
        workers as usize
    } else {
        worker_names.len()
    };

    let llm = cas_config.llm();

    // Resolve per-worker specs from the cascade (cas-2992).
    let resolved_worker_specs = {
        // EPIC cas-8888 (cas-9a31, Phase 1) SILENT SITE — audited:
        // harness-agnostic, Grok flows through as Some(Grok) correctly
        // (same pattern as cli/factory/mod.rs).
        let sources = ConfigSources {
            cli_flag: if worker_cli != cas_mux::SupervisorCli::Claude {
                Some(worker_cli)
            } else {
                None
            },
            model_flag: llm.model_for_role("worker").map(String::from),
            effort_flag: llm
                .reasoning_effort_for_role("worker")
                .and_then(|s| s.parse().ok()),
            config_dir_flag: None,
            worker_spec_jsons,
            supervisor_spec_json: None,
            user_config: None, // auto-resolve from home dir
            project_config: Some(cwd.join(".cas").join("config.toml")),
        };
        let specs = resolve_specs(effective_workers, sources)
            .map_err(|e| anyhow::anyhow!("Failed to resolve worker specs: {e}"))?;
        for spec in &specs {
            cas_factory::validate_explicit(spec, &CapabilitySnapshot::default())
                .map_err(|e| anyhow::anyhow!("Failed to validate worker routing spec: {e}"))?;
        }
        specs
    };

    // Resolve supervisor spec from the cascade (cas-1948).
    let resolved_supervisor_spec = {
        // EPIC cas-8888 (cas-9a31, Phase 1) SILENT SITE — audited, same as
        // resolved_worker_specs above.
        let sources = ConfigSources {
            cli_flag: if supervisor_cli != cas_mux::SupervisorCli::Claude {
                Some(supervisor_cli)
            } else {
                None
            },
            model_flag: llm.model_for_role("supervisor").map(String::from),
            effort_flag: llm
                .reasoning_effort_for_role("supervisor")
                .and_then(|s| s.parse().ok()),
            config_dir_flag: None,
            worker_spec_jsons: vec![],
            supervisor_spec_json,
            user_config: None, // auto-resolve from home dir
            project_config: Some(cwd.join(".cas").join("config.toml")),
        };
        let spec = resolve_supervisor_spec(sources)
            .map_err(|e| anyhow::anyhow!("Failed to resolve supervisor spec: {e}"))?;
        cas_factory::validate_explicit(&spec, &CapabilitySnapshot::default())
            .map_err(|e| anyhow::anyhow!("Failed to validate supervisor routing spec: {e}"))?;
        spec
    };

    // Resolve supervisor name up front so teams_configs and FactoryConfig agree.
    // When the caller doesn't provide a name, generate one so the teams config
    // key matches the Mux pane name (app/init.rs uses generate_unique for this).
    let resolved_supervisor_name = supervisor_name
        .unwrap_or_else(|| crate::orchestration::names::generate_unique(1).remove(0));

    // Build native Agent Teams spawn configs so agents start with Teams CLI flags.
    let (teams_configs, lead_session_id) = {
        use crate::ui::factory::daemon::runtime::teams::TeamsManager;
        TeamsManager::build_configs_for_mux(session, &resolved_supervisor_name, &worker_names)
    };

    let config = FactoryConfig {
        cwd: cwd.to_path_buf(),
        workers: effective_workers,
        worker_names,
        supervisor_name: Some(resolved_supervisor_name),
        supervisor_cli,
        worker_cli,
        supervisor_model: llm.model_for_role("supervisor").map(String::from),
        worker_model: llm.model_for_role("worker").map(String::from),
        supervisor_effort: llm
            .reasoning_effort_for_role("supervisor")
            .map(String::from),
        worker_effort: llm.reasoning_effort_for_role("worker").map(String::from),
        resolved_worker_specs,
        resolved_supervisor_spec: Some(resolved_supervisor_spec),
        enable_worktrees: !no_worktrees,
        worktree_root,
        notify: NotifyConfig {
            enabled: notify,
            backend: NotifyBackend::detect(),
            also_bell: false,
        },
        tabbed_workers: tabbed,
        auto_prompt: cas_config.orchestration().auto_prompt.clone(),
        record,
        session_id: if record {
            Some(session.to_string())
        } else {
            None
        },
        teams_configs,
        lead_session_id: Some(lead_session_id),
        minions_theme: cas_config
            .theme
            .as_ref()
            .map(|t| t.variant == crate::ui::theme::ThemeVariant::Minions)
            .unwrap_or(false),
        // cas-9829: activity-based stall detection threshold.
        stall_threshold_secs: cas_config.factory().stall_threshold_secs,
        stall_after_secs: cas_config.factory().stall_after_secs,
        ai_enrichment: cas_config.factory().ai_enrichment,
    };

    let daemon_config = DaemonConfig {
        session_name: session.to_string(),
        factory_config: config,
        foreground,
        boot_progress,
        phone_home,
    };

    let rt = tokio::runtime::Runtime::new()?;
    if daemon_config.boot_progress {
        let supervisor_name = daemon_config
            .factory_config
            .supervisor_name
            .clone()
            .unwrap_or_else(|| "supervisor".to_string());
        let worker_names = daemon_config.factory_config.worker_names.clone();
        rt.block_on(crate::ui::factory::run_daemon_with_boot_progress(
            daemon_config,
            supervisor_name,
            worker_names,
        ))
    } else {
        rt.block_on(crate::ui::factory::run_daemon(daemon_config))
    }
}

pub(super) fn execute_legacy_daemon(
    session_name: String,
    config: FactoryConfig,
    phone_home: bool,
) -> Result<()> {
    let daemon_config = DaemonConfig {
        session_name,
        factory_config: config,
        foreground: true,
        boot_progress: false,
        phone_home,
    };
    daemonize(daemon_config)
}

#[cfg(unix)]
pub(super) fn run_factory_with_daemon(
    session_name: String,
    config: FactoryConfig,
    phone_home: bool,
) -> Result<()> {
    // macOS can crash when forking a multi-threaded process before exec.
    // Default to subprocess daemon mode there; allow explicit override.
    if should_use_subprocess_daemon_on_macos() {
        let supervisor_name = config
            .supervisor_name
            .clone()
            .unwrap_or_else(|| "supervisor".to_string());
        let worker_names = config.worker_names.clone();
        let worktrees_enabled = config.enable_worktrees;
        let minions_theme = config.minions_theme;
        let cwd = config.cwd.to_string_lossy().to_string();
        let profile = build_boot_profile(&config, worker_names.len());

        let daemon_config = DaemonConfig {
            session_name: session_name.clone(),
            factory_config: config,
            foreground: true,
            boot_progress: true,
            phone_home,
        };
        daemonize(daemon_config)?;

        let sock_path = crate::ui::factory::socket_path(&session_name);

        let boot_config = BootConfig {
            supervisor_name,
            worker_names,
            cwd,
            session_name: session_name.clone(),
            profile,
            skip_animation: false,
            minions_theme,
        };

        if let Err(e) = run_boot_screen_client(&boot_config, &sock_path, 0) {
            tracing::warn!("Boot screen client failed on macOS subprocess path: {}", e);
            let client_error = explain_boot_error(&e.to_string(), worktrees_enabled);

            std::thread::sleep(Duration::from_millis(200));
            let log_path = daemon_log_path(&session_name);
            if let Ok(log) = std::fs::read_to_string(&log_path) {
                let log = log.trim();
                if !log.is_empty() {
                    let msg = log.strip_prefix("Error: ").unwrap_or(log);
                    bail!("{}", explain_boot_error(msg, worktrees_enabled));
                }
            }

            wait_for_socket(&sock_path, Duration::from_secs(30))
                .map_err(|_| anyhow::anyhow!(client_error))?;
        }

        return attach_with_retry(&session_name, Duration::from_secs(10));
    }

    let supervisor_name = config
        .supervisor_name
        .clone()
        .unwrap_or_else(|| "supervisor".to_string());
    let worker_names = config.worker_names.clone();
    let worktrees_enabled = config.enable_worktrees;
    let minions_theme = config.minions_theme;
    let cwd = config.cwd.to_string_lossy().to_string();
    let profile = build_boot_profile(&config, worker_names.len());

    match fork_first_daemon(
        session_name.clone(),
        config,
        supervisor_name.clone(),
        worker_names.clone(),
        phone_home,
    )? {
        ForkFirstResult::Parent {
            session_name,
            sock_path,
            daemon_pid,
        } => {
            let _daemon_reaper =
                crate::hub::supervise_forked_daemon(session_name.clone(), daemon_pid);
            let boot_config = BootConfig {
                supervisor_name,
                worker_names,
                cwd,
                session_name: session_name.clone(),
                profile,
                skip_animation: false,
                minions_theme,
            };

            if let Err(e) = run_boot_screen_client(&boot_config, &sock_path, daemon_pid) {
                tracing::warn!("Boot screen client failed: {}", e);
                let client_error = explain_boot_error(&e.to_string(), worktrees_enabled);

                std::thread::sleep(Duration::from_millis(200));

                let log_path = daemon_log_path(&session_name);
                if let Ok(log) = std::fs::read_to_string(&log_path) {
                    let log = log.trim();
                    if !log.is_empty() {
                        let msg = log.strip_prefix("Error: ").unwrap_or(log);
                        bail!("{}", explain_boot_error(msg, worktrees_enabled));
                    }
                }

                let start = std::time::Instant::now();
                let timeout = Duration::from_secs(30);
                while start.elapsed() < timeout {
                    if sock_path.exists() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }

                if !sock_path.exists() {
                    bail!("{client_error}");
                }
            }

            attach_with_retry(&session_name, Duration::from_secs(10))
        }
        ForkFirstResult::Child { init_phase } => {
            let mut daemon = init_phase.run_with_progress()?;
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(daemon.run())
        }
    }
}

fn wait_for_socket(sock_path: &std::path::Path, timeout: Duration) -> Result<()> {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if sock_path.exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!(
        "Timed out waiting for daemon socket at {}",
        sock_path.display()
    )
}

fn attach_with_retry(session_name: &str, timeout: Duration) -> Result<()> {
    let start = std::time::Instant::now();
    loop {
        match attach(Some(session_name.to_string())) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let msg = e.to_string();
                let retryable = msg.contains("Failed to connect to daemon socket")
                    || msg.contains("daemon not running")
                    || msg.contains("Session '")
                    || msg.contains("No running factory sessions found");
                if retryable && start.elapsed() < timeout {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                return Err(e);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn should_use_subprocess_daemon_on_macos() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
fn should_use_subprocess_daemon_on_macos() -> bool {
    false
}

fn explain_boot_error(message: &str, worktrees_enabled: bool) -> String {
    let message = message
        .strip_prefix("Daemon initialization failed: ")
        .unwrap_or(message)
        .trim();

    if !worktrees_enabled {
        return message.to_string();
    }

    let lower = message.to_lowercase();
    if lower.contains("permission denied") || lower.contains("operation not permitted") {
        format!(
            "{message}\n\nWorktree setup failed due to filesystem permissions.\nTry:\n  1) `cas factory --no-worktrees`\n  2) `cas factory --worktree-root <writable-directory>`"
        )
    } else if lower.contains("not a git repository") {
        format!(
            "{message}\n\nDefault factory mode needs a git repo for worktree isolation.\nRun `git init` + first commit, or use `cas factory --no-worktrees`."
        )
    } else {
        message.to_string()
    }
}

fn build_boot_profile(config: &FactoryConfig, worker_count: usize) -> String {
    let mode = if config.enable_worktrees {
        "isolated worktrees"
    } else {
        "shared directory"
    };

    let supervisor_cli = config.supervisor_cli.backend().name();
    let worker_cli = config.worker_cli.backend().name();

    if worker_count == 0 {
        format!("supervisor-only • {mode} • {supervisor_cli}")
    } else {
        let worker_label = if worker_count == 1 {
            "worker"
        } else {
            "workers"
        };
        format!("{worker_count} {worker_label} • {mode} • {supervisor_cli}→{worker_cli}")
    }
}

#[cfg(not(unix))]
pub(super) fn run_factory_with_daemon(
    _session_name: String,
    _config: FactoryConfig,
    _phone_home: bool,
) -> Result<()> {
    bail!("Factory daemon mode is only supported on Unix systems")
}
