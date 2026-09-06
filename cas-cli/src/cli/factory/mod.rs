//! Factory session management
//!
//! Spawns a native terminal multiplexer TUI with:
//! - Director (native panel) - monitors events, displays tasks/agents/activity
//! - Supervisor - plans epics, assigns tasks, handles merges
//! - Workers - execute tasks (read-only, inject only)

mod cloud_attach;
mod daemon;
mod doctor;
mod lifecycle;
pub(crate) mod parity;
pub(crate) mod probe_comm;
mod queries;
mod remote_attach;
// cas-728b: pub(crate) (not private) so the director's stall-detection
// confirmation step (ui/factory/director/events.rs) can call
// resolve_worker/transcript_mtime_age directly — the same transcript-mtime
// liveness signal `cas factory is-wedged` reads (cas-4513).
pub(crate) mod wedged;
mod worktree_ops;

use crate::cli::Cli;
use crate::config::Config;
use crate::orchestration::names::generate_unique;
use crate::ui::factory::{
    FactoryConfig, NotifyBackend, NotifyConfig, attach, find_session_for_project,
    generate_session_name,
};
use crate::worktree::GitOperations;
use anyhow::{Result, bail};
use cas_factory::routing::{CapabilitySnapshot, resolve_lane_specs, validate_lane_request};
use cas_factory::spec_resolver::{ConfigSources, resolve_specs, resolve_supervisor_spec};
use clap::{Args, Subcommand};
use std::io::IsTerminal;

pub use lifecycle::{execute_kill, execute_kill_all};
pub use queries::execute_list;

/// Name the Codex account home at the last decision point before a factory
/// attaches or starts. The profile shortcut already announces it during early
/// environment setup, but keeping it next to the session choice prevents that
/// account fact from being separated from the prompt by preflight output.
fn codex_account_home_suffix(supervisor_cli: &str) -> String {
    if supervisor_cli != "codex" {
        return String::new();
    }
    let home = std::env::var_os("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")));
    home.map(|home| format!(" (Codex account home: {})", home.display()))
        .unwrap_or_default()
}

/// cas-0bf4: bridge the `[factory]` `cargo_build_jobs` + `nice_cargo` config
/// knobs into process env so that worker PTY spawns (see `cas-pty`
/// `PtyConfig::{claude,codex}`) read them.
///
/// This runs from `cli::run` *before* `initialize_telemetry()` spawns its
/// background PostHog thread. That is load-bearing: `std::env::set_var`
/// in a multi-threaded process is undefined behaviour (Rust edition 2024
/// gated it behind `unsafe` precisely for this reason). Keep the call
/// site single-threaded.
///
/// Shell-level overrides already present in the parent env always win:
/// `CAS_FACTORY_CARGO_BUILD_JOBS=<N>` and `CAS_FACTORY_NICE_WORKER=1`.
/// Case-insensitive `"auto"` matches pass through to the cas-pty
/// auto-compute path.
pub fn apply_resource_contention_env(cas_root: Option<&std::path::Path>) {
    // cas-c614: enforce the single-thread SAFETY precondition at runtime so a
    // future refactor that relocates this call (e.g. into a daemon fork path
    // that is already multi-threaded) cannot silently re-introduce UB.
    //
    // Two layers of defence:
    //   1. `ENV_BRIDGE_INVOKED` hard-fails on the SECOND call, regardless of
    //      build profile. Two call sites both running after thread-spawn is
    //      the exact failure mode the task description warns about.
    //   2. `debug_assert!` on the Linux thread count catches the regression
    //      under `cargo test` / debug builds without paying a release-build
    //      /proc/self/stat read on every startup.
    if ENV_BRIDGE_INVOKED.set(()).is_err() {
        panic!(
            "apply_resource_contention_env called more than once; the second \
             call would std::env::set_var from a potentially multi-threaded \
             context (UB per Rust 2024). See cas-c614 / cas-0bf4."
        );
    }
    debug_assert!(
        check_single_threaded_precondition().is_ok(),
        "apply_resource_contention_env invoked post-thread-spawn; \
         std::env::set_var is UB in a multi-threaded process (Rust 2024). \
         See cas-c614 / cas-0bf4."
    );

    let Some(cas_root) = cas_root else {
        return;
    };
    let Ok(cfg) = Config::load(cas_root) else {
        return;
    };
    let fc = cfg.factory();
    let trimmed = fc.cargo_build_jobs.trim();
    // SAFETY: caller (cli::run) invokes this before the telemetry thread is
    // spawned, so the process is still single-threaded. That is the whole
    // reason this function exists as a separate entry point rather than
    // living inline in `factory::execute`. The guards above enforce the
    // invariant at runtime — see cas-c614.
    unsafe {
        if std::env::var("CAS_FACTORY_CARGO_BUILD_JOBS").is_err()
            && !trimmed.is_empty()
            && !trimmed.eq_ignore_ascii_case("auto")
        {
            std::env::set_var("CAS_FACTORY_CARGO_BUILD_JOBS", trimmed);
        }
        if std::env::var("CAS_FACTORY_NICE_WORKER").is_err() && fc.nice_cargo {
            std::env::set_var("CAS_FACTORY_NICE_WORKER", "1");
        }
    }
}

/// cas-c614: detection result for the `apply_resource_contention_env`
/// single-thread precondition check. `Ok(())` means the precondition holds
/// (or could not be proven to fail on this platform); `Err(count)` means we
/// observed >1 OS threads before the env-mutation call.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PreconditionError {
    /// /proc/self/stat reported `num_threads` > 1.
    ThreadsAlreadySpawned { count: u32 },
}

/// Guard latch preventing a second call to `apply_resource_contention_env`.
/// Separate from thread-count detection: it catches the "two code paths both
/// invoke the bridge, one of them after thread-spawn" failure mode even on
/// platforms where `current_num_threads` returns `None`.
static ENV_BRIDGE_INVOKED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

/// Returns `Ok(())` when the process appears single-threaded (or the thread
/// count cannot be determined — fail-open on non-Linux). Returns
/// `Err(PreconditionError::ThreadsAlreadySpawned{..})` when a `/proc/self/stat`
/// read shows `num_threads` > 1.
///
/// Linux-only: on macOS / BSD / Windows we return `Ok(())` because there is
/// no equally cheap thread-count probe; the guard still catches bugs on the
/// primary developer + CI platform (Linux), which is the platform that
/// stamped the adversarial-review P0.
pub(crate) fn check_single_threaded_precondition() -> Result<(), PreconditionError> {
    match current_num_threads() {
        Some(n) if n > 1 => Err(PreconditionError::ThreadsAlreadySpawned { count: n }),
        _ => Ok(()),
    }
}

/// Reads `num_threads` from `/proc/self/stat` (field 20). Returns `None` on
/// non-Linux targets or when the file cannot be read/parsed — callers treat
/// `None` as "cannot prove a violation" and fall through.
#[cfg(target_os = "linux")]
fn current_num_threads() -> Option<u32> {
    let data = std::fs::read_to_string("/proc/self/stat").ok()?;
    parse_num_threads_from_proc_stat(&data)
}

#[cfg(not(target_os = "linux"))]
fn current_num_threads() -> Option<u32> {
    None
}

/// Parses the `num_threads` field from a `/proc/<pid>/stat` line.
///
/// /proc/<pid>/stat is a space-separated record where field 2 (`comm`) is
/// parenthesised and may itself contain spaces and parentheses. We split on
/// the **last** `)` so anything after is safely whitespace-delimited.
/// `num_threads` is field 20 in the overall record (4th after `comm`), which
/// is index 17 after the split since the trailing fields start at index 0 =
/// field 3 (`state`).
#[cfg(target_os = "linux")]
fn parse_num_threads_from_proc_stat(data: &str) -> Option<u32> {
    let after_comm = data.rsplit_once(')')?.1.trim_start();
    // Index map (0-based, relative to after_comm):
    //   0 -> field 3 (state)
    //   1 -> field 4 (ppid)
    //   ...
    //   17 -> field 20 (num_threads)
    after_comm.split_ascii_whitespace().nth(17)?.parse().ok()
}

/// Launch hierarchical multi-agent factory session
#[derive(Args, Debug, Clone)]
pub struct FactoryArgs {
    /// Factory subcommand
    #[command(subcommand)]
    pub command: Option<FactoryCommands>,

    /// Number of worker agents (default: 0 for supervisor-only startup)
    #[arg(long, short = 'w', default_value = "0", global = true)]
    pub workers: u8,

    /// Custom session name
    #[arg(long, short = 'n', global = true)]
    pub name: Option<String>,

    /// Start a new session instead of auto-attaching an existing one
    #[arg(long = "new", global = true)]
    pub start_new: bool,

    /// Attach to an existing session without prompting (skip confirmation)
    #[arg(long, global = true)]
    pub attach: bool,

    /// Disable worktree-based worker isolation (all agents share the same directory)
    #[arg(long, global = true)]
    pub no_worktrees: bool,

    /// Custom directory for worker worktrees (default: .cas/worktrees under project)
    #[arg(long, global = true)]
    pub worktree_root: Option<std::path::PathBuf>,

    /// Remove all worker worktree directories
    #[arg(long, conflicts_with = "workers")]
    pub cleanup: bool,

    /// Show what would be removed without actually deleting (use with --cleanup)
    #[arg(long, requires = "cleanup")]
    pub dry_run: bool,

    /// Force cleanup without confirmation (use with --cleanup)
    #[arg(long, short = 'f', requires = "cleanup")]
    pub force: bool,

    /// Run daemon in foreground (no fork) instead of attaching
    #[arg(long, hide = true)]
    pub legacy: bool,

    /// Enable desktop/terminal notifications for task events
    #[arg(long)]
    pub notify: bool,

    /// Also ring terminal bell on notifications (use with --notify)
    #[arg(long, requires = "notify")]
    pub bell: bool,

    /// Use tabbed worker view instead of side-by-side (default: side-by-side)
    #[arg(long, global = true)]
    pub tabbed: bool,

    /// Record terminal sessions for time-travel playback
    #[arg(long, global = true)]
    pub record: bool,

    /// Supervisor CLI to use (claude, codex, or grok)
    #[arg(long, default_value = "claude")]
    pub supervisor_cli: String,

    /// Worker CLI to use (claude, codex, or grok)
    #[arg(long, default_value = "claude")]
    pub worker_cli: String,

    /// Persist this supervisor provider as the default for future sessions.
    ///
    /// When set, writes `[llm.supervisor] harness = "<provider>"` to
    /// `~/.cas/config.toml` before launching, and prints a confirmation line.
    /// Use with `cas factory --supervisor-cli <provider> --default`.
    #[arg(long = "default")]
    pub set_default: bool,

    /// Internal: whether `supervisor_cli` was explicitly specified by the
    /// caller (provider shortcut or programmatic use) rather than left at the
    /// clap default. Set to `true` by provider shortcuts so that the
    /// config-harness override in `execute` does NOT clobber the explicit
    /// provider choice with the persisted default.
    ///
    /// NOT exposed as a CLI flag — callers that need to signal "explicit" set
    /// this field directly on the `FactoryArgs` value before calling execute.
    #[arg(skip)]
    pub supervisor_cli_explicit: bool,

    /// Disable cloud phone-home (push factory state to Cassy Cloud)
    #[arg(long, global = true)]
    pub no_phone_home: bool,

    /// Per-worker spec override as a JSON object (repeatable).
    ///
    /// Each value is a JSON object with optional fields `name`, `cli`,
    /// `model`, `effort`.  Matched to worker slots by name first, then
    /// by sequential position.  Example:
    ///   --worker-spec '{"name":"alice","cli":"codex"}' \
    ///   --worker-spec '{"name":"bob","cli":"claude","effort":"high"}'
    #[arg(long = "worker-spec", value_name = "JSON")]
    pub worker_spec: Vec<String>,

    /// Resolve workers through the embedded lane registry.
    ///
    /// A lane is mutually exclusive with explicit worker CLI/model/effort
    /// recipe controls; names and account metadata may still be supplied.
    #[arg(long, global = true)]
    pub lane: Option<String>,

    /// Supervisor spec override as a JSON object (singular).
    ///
    /// A JSON object with optional fields `cli`, `model`, `effort`.
    /// Overrides `--supervisor-cli`/`--supervisor-model`/`--supervisor-effort`
    /// and `[factory.supervisor]` config.  Example:
    ///   --supervisor-spec '{"cli":"claude","model":"claude-opus-5","effort":"xhigh"}'
    #[arg(long = "supervisor-spec", value_name = "JSON")]
    pub supervisor_spec: Option<String>,

    /// Refuse to silently fall back when a resolved worker/supervisor spec
    /// requests Codex but Codex is unavailable (binary missing, or missing
    /// `~/.codex/auth.json` login) — bail with an actionable error instead
    /// of rewriting the spec to Claude (cas-7199 / cas-a487). OR'd with
    /// `[factory] strict_cli` in `.cas/config.toml`; either being true
    /// enables strict mode. Default (this flag absent): fall back to
    /// Claude with a loud warning — see `apply_codex_fallback`.
    #[arg(long = "strict-cli", global = true)]
    pub strict_cli: bool,
}

impl Default for FactoryArgs {
    fn default() -> Self {
        Self {
            command: None,
            workers: 0,
            name: None,
            start_new: false,
            attach: false,
            no_worktrees: false,
            worktree_root: None,
            cleanup: false,
            dry_run: false,
            force: false,
            legacy: false,
            notify: false,
            bell: false,
            tabbed: false,
            record: false,
            supervisor_cli: "claude".to_string(),
            worker_cli: "claude".to_string(),
            no_phone_home: false,
            worker_spec: vec![],
            lane: None,
            supervisor_spec: None,
            set_default: false,
            supervisor_cli_explicit: false,
            strict_cli: false,
        }
    }
}

/// Resolve direct-CLI workers through the same registry lane seam used by the
/// MCP `spawn_workers` request. Identity/account metadata is allowed to remain
/// per-worker, but recipe controls are rejected so lane fallback cannot be
/// silently combined with an explicit override.
fn resolve_lane_worker_specs(
    lane: &str,
    slots: usize,
    worker_names: &[String],
    worker_cli: cas_mux::SupervisorCli,
    worker_spec_jsons: &[String],
    snapshot: &CapabilitySnapshot,
) -> Result<(Vec<cas_mux::WorkerSpec>, String)> {
    if slots == 0 {
        bail!("lane spawn requires at least one worker slot");
    }
    if worker_spec_jsons.len() > slots {
        bail!(
            "--worker-spec has {} entries but this spawn has only {slots} worker slot(s)",
            worker_spec_jsons.len()
        );
    }
    validate_lane_request(
        lane,
        worker_cli != cas_mux::SupervisorCli::Claude,
        false,
        false,
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;

    let decisions = resolve_lane_specs(lane, slots, snapshot)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let recipe_id = decisions
        .first()
        .map(|decision| decision.recipe_id.clone())
        .unwrap_or_else(|| "none".to_string());
    let mut warnings = decisions
        .iter()
        .flat_map(|decision| decision.warnings.iter().cloned())
        .collect::<Vec<_>>();
    warnings.sort();
    warnings.dedup();
    let mut specs = decisions
        .into_iter()
        .enumerate()
        .map(|(slot, decision)| {
            let mut spec = decision.spec;
            if let Some(name) = worker_names.get(slot) {
                spec.name = Some(name.clone());
            }
            spec
        })
        .collect::<Vec<_>>();

    for (slot, json) in worker_spec_jsons.iter().enumerate() {
        let value: serde_json::Value = serde_json::from_str(json).map_err(|error| {
            anyhow::anyhow!("invalid --worker-spec JSON for lane mode: {error}")
        })?;
        let object = value.as_object().ok_or_else(|| {
            anyhow::anyhow!("invalid --worker-spec JSON: every entry must be an object")
        })?;
        let explicit_fields = ["cli", "model", "effort"]
            .into_iter()
            .filter(|field| object.get(*field).is_some_and(|value| !value.is_null()))
            .collect::<Vec<_>>();
        if !explicit_fields.is_empty() {
            return Err(anyhow::anyhow!(
                "lane={:?} cannot be combined with explicit {} recipe field(s) in --worker-spec[{}]; choose --lane or an explicit cli/model/effort recipe",
                lane.trim(),
                explicit_fields.join(", "),
                slot
            ));
        }
        if let Some(name) = object.get("name") {
            specs[slot].name = Some(
                name.as_str()
                    .ok_or_else(|| {
                        anyhow::anyhow!("invalid --worker-spec[{slot}].name: expected a string")
                    })?
                    .to_string(),
            );
        }
        if let Some(config_dir) = object.get("config_dir") {
            specs[slot].config_dir = Some(
                config_dir
                    .as_str()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "invalid --worker-spec[{slot}].config_dir: expected a string"
                        )
                    })?
                    .to_string(),
            );
        }
    }

    for spec in &specs {
        cas_factory::validate_explicit(spec, snapshot)
            .map_err(|error| anyhow::anyhow!("Failed to validate lane routing spec: {error}"))?;
    }

    let mut notice = format!("lane={} resolved recipe={recipe_id}", lane.trim());
    for warning in warnings {
        notice.push_str(&format!("; warning: {warning}"));
    }
    Ok((specs, notice))
}

/// Arguments for `cas attach`
#[derive(Args, Debug, Clone)]
pub struct AttachArgs {
    /// Session name to attach to (default: most recent)
    pub name: Option<String>,

    /// Remote target: `device:factory-id` (SSH) or `factory-id` (cloud relay)
    #[arg(long)]
    pub remote: Option<String>,

    /// Specific worker to focus on (used with --remote)
    #[arg(long)]
    pub worker: Option<String>,
}

/// Arguments for `cas kill`
#[derive(Args, Debug, Clone)]
pub struct KillArgs {
    /// Session name to kill (interactive picker if omitted)
    pub name: Option<String>,

    /// Force kill without confirmation
    #[arg(long, short = 'f')]
    pub force: bool,
}

/// Arguments for `cas kill-all`
#[derive(Args, Debug, Clone)]
pub struct KillAllArgs {
    /// Force kill without confirmation
    #[arg(long, short = 'f')]
    pub force: bool,
}

/// Internal factory subcommands (hidden from help)
#[derive(Subcommand, Debug, Clone)]
pub enum FactoryCommands {
    /// Report local Claude/Codex/CAS-MCP readiness without spawning workers.
    Doctor,

    /// Run the bounded unified factory readiness report without spawning workers
    Preflight {
        /// Explicit Cassy root (.cas directory)
        #[arg(long)]
        cas_root: Option<std::path::PathBuf>,
    },

    /// Run as a factory daemon (internal use)
    #[command(hide = true)]
    Daemon {
        /// Session name
        #[arg(long)]
        session: String,

        /// Working directory
        #[arg(long)]
        cwd: std::path::PathBuf,

        /// Number of workers
        #[arg(long, default_value = "0")]
        workers: u8,

        /// Disable worktree-based worker isolation
        #[arg(long)]
        no_worktrees: bool,

        /// Custom directory for worker worktrees
        #[arg(long)]
        worktree_root: Option<std::path::PathBuf>,

        /// Enable notifications
        #[arg(long)]
        notify: bool,

        /// Supervisor CLI to use (claude, codex, or grok)
        #[arg(long, default_value = "claude")]
        supervisor_cli: String,

        /// Worker CLI to use (claude, codex, or grok)
        #[arg(long, default_value = "claude")]
        worker_cli: String,

        /// Use tabbed worker view
        #[arg(long)]
        tabbed: bool,

        /// Record terminal sessions
        #[arg(long)]
        record: bool,

        /// Run in foreground
        #[arg(long)]
        foreground: bool,

        /// Disable cloud phone-home
        #[arg(long)]
        no_phone_home: bool,

        /// Stream boot initialization progress via socket before attach (internal use)
        #[arg(long, hide = true)]
        boot_progress: bool,

        /// Explicit supervisor name (internal use)
        #[arg(long, hide = true)]
        supervisor_name: Option<String>,

        /// Explicit worker names (internal use, repeat per worker)
        #[arg(long = "worker-name", hide = true)]
        worker_names: Vec<String>,

        /// Per-worker spec JSON overrides (internal use, repeat per worker)
        #[arg(long = "worker-spec-json", hide = true)]
        worker_spec_jsons: Vec<String>,

        /// Supervisor spec JSON override (internal use, singular)
        #[arg(long = "supervisor-spec-json", hide = true)]
        supervisor_spec_json: Option<String>,
    },

    /// Check if worktree is behind its sync target (used as SessionStart hook)
    CheckStaleness {
        /// Target branch to check against (auto-detected if not specified)
        #[arg(long, short = 'b')]
        branch: Option<String>,

        /// Fetch from remote before checking
        #[arg(long)]
        fetch: bool,
    },

    /// Sync worktree to its sync target (fetches when target is remote-tracking)
    Sync {
        /// Target branch to sync to (auto-detected if not specified)
        #[arg(long, short = 'b')]
        branch: Option<String>,
    },

    /// List known sessions (JSON-friendly; prefer `cas list --json`)
    Sessions {
        /// Only show sessions that can currently be attached to
        #[arg(long)]
        attachable_only: bool,
    },

    /// Show agent status for a session (reads Cassy AgentStore; does not attach)
    Agents {
        /// Session name (default: most recent attachable session for this project)
        #[arg(long)]
        session: Option<String>,

        /// Project directory to scope session discovery (default: current directory)
        #[arg(long)]
        project_dir: Option<std::path::PathBuf>,

        /// Include all active agents in the project store (not just this session's agents)
        #[arg(long)]
        all: bool,

        /// Explicit Cassy root (.cas directory) to use instead of resolving from project_dir/session metadata
        #[arg(long)]
        cas_root: Option<std::path::PathBuf>,
    },

    /// Show recent activity events for a session (reads Cassy EventStore)
    Activity {
        /// Session name (default: most recent attachable session for this project)
        #[arg(long)]
        session: Option<String>,

        /// Project directory to scope session discovery (default: current directory)
        #[arg(long)]
        project_dir: Option<std::path::PathBuf>,

        /// Include all recent events in the project store (not just this session's agents)
        #[arg(long)]
        all: bool,

        /// Max events to return
        #[arg(long, default_value = "50")]
        limit: usize,

        /// Explicit Cassy root (.cas directory) to use instead of resolving from project_dir/session metadata
        #[arg(long)]
        cas_root: Option<std::path::PathBuf>,
    },

    /// Aggregated status snapshot for a session (ideal for external tools)
    Status {
        /// Session name (default: most recent attachable session for this project)
        #[arg(long)]
        session: Option<String>,

        /// Project directory to scope session discovery (default: current directory)
        #[arg(long)]
        project_dir: Option<std::path::PathBuf>,

        /// Max activity events to return
        #[arg(long, default_value = "20")]
        activity_limit: usize,

        /// Explicit Cassy root (.cas directory) to use instead of resolving from project_dir/session metadata
        #[arg(long)]
        cas_root: Option<std::path::PathBuf>,
    },

    /// List valid messaging targets for a session (supervisor/workers/all_workers)
    Targets {
        /// Session name (default: most recent attachable session for this project)
        #[arg(long)]
        session: Option<String>,

        /// Project directory to scope session discovery (default: current directory)
        #[arg(long)]
        project_dir: Option<std::path::PathBuf>,
    },

    /// Inject a message into supervisor/workers via the prompt queue (no PTY attach)
    Message {
        /// Session name (default: most recent attachable session for this project)
        #[arg(long)]
        session: Option<String>,

        /// Project directory to scope session discovery (default: current directory)
        #[arg(long)]
        project_dir: Option<std::path::PathBuf>,

        /// Target: supervisor | all_workers | <worker-name>
        #[arg(long)]
        target: String,

        /// Message text to enqueue
        #[arg(long)]
        message: String,

        /// Source label for attribution in wrapped message (default: openclaw)
        #[arg(long, default_value = "openclaw")]
        from: String,

        /// Enqueue the raw message without wrapping in an XML `<message>` tag
        #[arg(long)]
        no_wrap: bool,

        /// Wait until the factory daemon records an injection event for this message ID
        #[arg(long)]
        wait_ack: bool,

        /// Timeout in milliseconds for --wait-ack
        #[arg(long, default_value = "5000")]
        timeout_ms: u64,

        /// Explicit Cassy root (.cas directory) to use instead of resolving from project_dir/session metadata
        #[arg(long)]
        cas_root: Option<std::path::PathBuf>,
    },

    /// Run the isolated supervisor/worker communication conformance probe
    /// Launched-agent skill + instruction parity conformance gate (cas-bd9d):
    /// emit a machine-readable 4×2 (Claude/Codex/Grok/OpenCode × supervisor/worker) report
    /// proving required skills/agents, effective role clauses, and tool prefix
    /// from the real launch configuration. Non-zero exit if any cell fails.
    Parity {
        /// Optional path to also write the per-cell report as JSONL.
        #[arg(long)]
        jsonl: Option<std::path::PathBuf>,
    },

    ProbeComm {
        /// JSONL output path for per-scenario stage evidence
        #[arg(long)]
        jsonl: std::path::PathBuf,

        /// Disposable Cassy root to use for the probe; defaults to a temp root
        #[arg(long)]
        cas_root: Option<std::path::PathBuf>,

        /// Recorded harness artifact root for Claude/Codex/Grok/OpenCode adapters
        #[arg(long)]
        artifact_root: Option<std::path::PathBuf>,

        /// Explicitly permit using the active parent Cassy root
        #[arg(long)]
        allow_active_cas_root: bool,

        /// Probe adapter. Unit 6 ships only the deterministic fake adapter.
        #[arg(long, value_enum, default_value = "fake")]
        adapter: probe_comm::ProbeAdapterKind,

        /// Legacy per-sample delivery threshold in milliseconds (hidden; use aggregate transport gates)
        #[arg(long, default_value = "2000", hide = true)]
        delivery_slo_ms: u64,

        /// Legacy fake-runner selection threshold in milliseconds
        #[arg(long, default_value = "250", hide = true)]
        selection_slo_ms: u64,

        /// Normal transport p95 SLO threshold in milliseconds (epic contract: <=2000ms)
        #[arg(long, default_value = "2000")]
        normal_transport_p95_slo_ms: u64,

        /// Normal transport max SLO threshold in milliseconds (epic contract: <=10000ms)
        #[arg(long, default_value = "10000")]
        normal_transport_max_slo_ms: u64,

        /// Urgent transport p95 SLO threshold in milliseconds (epic contract: <=2000ms)
        #[arg(long, default_value = "2000")]
        urgent_transport_p95_slo_ms: u64,

        /// Urgent transport max SLO threshold in milliseconds (epic contract: <=5000ms)
        #[arg(long, default_value = "5000")]
        urgent_transport_max_slo_ms: u64,

        /// Worker wake p95 SLO threshold in milliseconds (epic contract: <=5000ms)
        #[arg(long, default_value = "5000")]
        wake_p95_slo_ms: u64,

        /// Visible reaction p95 SLO threshold in milliseconds when observable (epic contract: <=15000ms)
        #[arg(long, default_value = "15000")]
        reaction_p95_slo_ms: u64,

        /// Inject a transport failure as SCENARIO:MESSAGE_ID
        #[arg(long, value_name = "SCENARIO:MESSAGE_ID")]
        inject_transport_failure: Option<String>,

        /// Inject an SLO failure as SCENARIO:MESSAGE_ID:DELIVERED_AFTER_MS
        #[arg(long, value_name = "SCENARIO:MESSAGE_ID:MS")]
        inject_slo_failure: Option<String>,
    },

    /// Classify a worker as alive / wedged / starved / dead (cas-4513).
    ///
    /// Exit codes (differentiated so supervisor scripts can branch without
    /// parsing stdout): 0=alive, 1=wedged, 2=starved, 3=dead.
    IsWedged {
        /// Worker name (as shown in `cas factory agents`)
        worker: String,

        /// Emit structured JSON instead of the human-readable block
        #[arg(long)]
        json: bool,

        /// Explicit Cassy root (.cas directory) to use instead of the default
        #[arg(long)]
        cas_root: Option<std::path::PathBuf>,
    },

    /// Print the tail of a worker's Claude Code transcript (cas-4513).
    Debug {
        /// Worker name
        worker: String,

        /// Number of trailing JSONL lines to print
        #[arg(long, default_value = "20")]
        tail: usize,

        /// Explicit Cassy root
        #[arg(long)]
        cas_root: Option<std::path::PathBuf>,
    },

    /// SIGKILL a wedged worker and reset its tasks (cas-4513).
    ///
    /// SIGTERM is observed to not exit cleanly on the Bun-wedged Claude Code
    /// process, so this verb uses SIGKILL. Idempotent — an already-dead
    /// worker still runs the task-reset cleanup.
    ///
    /// By default refuses to SIGKILL a PID whose `/proc/<pid>/stat`
    /// starttime fingerprint does not match what was recorded at
    /// agent registration (= PID was recycled; killing would hit an
    /// unrelated process). Pass `--force` to override — only use that
    /// on legacy agents predating the fingerprint (cas-ea46) or when
    /// you've independently verified the PID is still the worker.
    Kill {
        /// Worker name
        worker: String,

        /// Override the PID-recycling fingerprint guard. Prints a
        /// warning in the summary when exercised.
        #[arg(long)]
        force: bool,

        /// Explicit Cassy root
        #[arg(long)]
        cas_root: Option<std::path::PathBuf>,
    },
}

pub fn execute(args: &FactoryArgs, cli: &Cli, cas_root: Option<&std::path::Path>) -> Result<()> {
    if let Some(ref cmd) = args.command {
        return match cmd {
            FactoryCommands::Doctor => doctor::execute(args, cli, cas_root),
            FactoryCommands::Preflight {
                cas_root: sub_cas_root,
            } => execute_unified_preflight(cli, sub_cas_root.as_deref().or(cas_root)),
            FactoryCommands::Daemon {
                session,
                cwd,
                workers,
                no_worktrees,
                worktree_root,
                notify,
                supervisor_cli,
                worker_cli,
                tabbed,
                record,
                no_phone_home,
                foreground,
                boot_progress,
                supervisor_name,
                worker_names,
                worker_spec_jsons,
                supervisor_spec_json,
            } => daemon::execute_daemon(
                session,
                cwd,
                *workers,
                *no_worktrees,
                worktree_root.clone(),
                *notify,
                *tabbed,
                *record,
                !*no_phone_home,
                parse_supervisor_cli(supervisor_cli)?,
                parse_supervisor_cli(worker_cli)?,
                *foreground,
                *boot_progress,
                supervisor_name.clone(),
                worker_names.clone(),
                worker_spec_jsons.clone(),
                supervisor_spec_json.clone(),
            ),
            FactoryCommands::CheckStaleness { branch, fetch } => {
                worktree_ops::execute_check_staleness(branch.as_deref(), *fetch)
            }
            FactoryCommands::Sync { branch } => worktree_ops::execute_sync(branch.as_deref()),
            FactoryCommands::Sessions { attachable_only } => {
                queries::execute_sessions(cli, *attachable_only)
            }
            FactoryCommands::Agents {
                session,
                project_dir,
                all,
                cas_root,
            } => queries::execute_agents(
                cli,
                session.as_deref(),
                project_dir.as_deref(),
                *all,
                cas_root.as_deref(),
            ),
            FactoryCommands::Activity {
                session,
                project_dir,
                all,
                limit,
                cas_root,
            } => queries::execute_activity(
                cli,
                session.as_deref(),
                project_dir.as_deref(),
                *all,
                *limit,
                cas_root.as_deref(),
            ),
            FactoryCommands::Status {
                session,
                project_dir,
                activity_limit,
                cas_root,
            } => queries::execute_status(
                cli,
                session.as_deref(),
                project_dir.as_deref(),
                *activity_limit,
                cas_root.as_deref(),
            ),
            FactoryCommands::Targets {
                session,
                project_dir,
            } => queries::execute_targets(cli, session.as_deref(), project_dir.as_deref()),
            FactoryCommands::Message {
                session,
                project_dir,
                target,
                message,
                from,
                no_wrap,
                wait_ack,
                timeout_ms,
                cas_root,
            } => queries::execute_message(
                cli,
                session.as_deref(),
                project_dir.as_deref(),
                target,
                message,
                from,
                *no_wrap,
                *wait_ack,
                *timeout_ms,
                cas_root.as_deref(),
            ),
            FactoryCommands::Parity { jsonl } => parity::execute_parity(jsonl.clone()),
            FactoryCommands::ProbeComm {
                jsonl,
                cas_root: sub_cas_root,
                artifact_root,
                allow_active_cas_root,
                adapter,
                delivery_slo_ms,
                selection_slo_ms,
                normal_transport_p95_slo_ms,
                normal_transport_max_slo_ms,
                urgent_transport_p95_slo_ms,
                urgent_transport_max_slo_ms,
                wake_p95_slo_ms,
                reaction_p95_slo_ms,
                inject_transport_failure,
                inject_slo_failure,
            } => probe_comm::execute_probe_comm(
                jsonl.clone(),
                sub_cas_root.clone(),
                artifact_root.clone(),
                cas_root,
                *allow_active_cas_root,
                *adapter,
                *delivery_slo_ms,
                *selection_slo_ms,
                *normal_transport_p95_slo_ms,
                *normal_transport_max_slo_ms,
                *urgent_transport_p95_slo_ms,
                *urgent_transport_max_slo_ms,
                *wake_p95_slo_ms,
                *reaction_p95_slo_ms,
                probe_comm::parse_probe_failure(
                    inject_transport_failure.as_deref(),
                    inject_slo_failure.as_deref(),
                )?,
            ),
            FactoryCommands::IsWedged {
                worker,
                json,
                cas_root: sub_cas_root,
            } => wedged::execute_is_wedged(sub_cas_root.as_deref().or(cas_root), worker, *json),
            FactoryCommands::Debug {
                worker,
                tail,
                cas_root: sub_cas_root,
            } => wedged::execute_debug(sub_cas_root.as_deref().or(cas_root), worker, *tail),
            FactoryCommands::Kill {
                worker,
                force,
                cas_root: sub_cas_root,
            } => wedged::execute_kill(sub_cas_root.as_deref().or(cas_root), worker, *force),
        };
    }

    if args.cleanup {
        return worktree_ops::execute_cleanup(args);
    }

    if args.workers > 6 {
        bail!("Maximum 6 workers supported in factory mode");
    }

    let cwd = std::env::current_dir()?;

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        let hints = noninteractive_factory_hints(args, &cwd, cas_root);
        let mut msg = String::from(
            "Factory mode requires an interactive terminal.\n\n\
             Run this command in a terminal (not a non-interactive shell/pipe).\n\
             For automation, use non-interactive commands like `cas list`, `cas status`, or `cas factory status`.",
        );
        if !hints.is_empty() {
            msg.push_str("\n\nBefore launching factory interactively:");
            for hint in hints {
                msg.push_str(&format!("\n  - {hint}"));
            }
        }
        bail!(msg);
    }

    // Apply [llm] config harness defaults when CLI args are at their defaults.
    // CLI args explicitly set by the user take precedence over config values.
    //
    // cas-7f2c: use `supervisor_cli_explicit` (not string-equals-"claude") to
    // distinguish "user explicitly chose claude" from "default 'claude' that
    // should be overridden by the persisted config value".  The old check
    // (`== "claude"`) silently ignored an explicit `--supervisor-cli=claude`
    // invocation when `[llm.supervisor] harness = "codex"` was
    // persisted — the config value always won, even though the user asked for
    // Claude.
    let mut effective_args = args.clone();
    let cas_dir_buf = cwd.join(".cas");
    let effective_cas_dir = cas_root.or_else(|| {
        if cas_dir_buf.exists() {
            Some(cas_dir_buf.as_path())
        } else {
            None
        }
    });
    if let Some(cas_dir) = effective_cas_dir {
        if let Ok(cfg) = Config::load(cas_dir) {
            let llm = cfg.llm();
            if !effective_args.supervisor_cli_explicit {
                effective_args.supervisor_cli = llm.harness_for_role("supervisor").to_string();
            }
            if effective_args.worker_cli == "claude" {
                effective_args.worker_cli = llm.harness_for_role("worker").to_string();
            }
        }
    }

    // cas-7f2c: persist the chosen provider to ~/.cas/config.toml when
    // --default is set.
    if effective_args.set_default {
        use crate::cli::provider_default::set_supervisor_harness;
        use crate::store::known_repos::host_cas_dir;

        let host_dir = host_cas_dir();
        if let Err(e) = std::fs::create_dir_all(&host_dir) {
            eprintln!("Warning: could not create ~/.cas/: {e}");
        } else {
            // Do not turn a malformed host config into defaults before this
            // mutation: saving those defaults would destroy the original.
            let mut global_cfg = Config::load(&host_dir)?;
            set_supervisor_harness(&mut global_cfg, &effective_args.supervisor_cli);
            if let Err(e) = global_cfg.save(&host_dir) {
                eprintln!("Warning: could not persist supervisor default: {e}");
            } else {
                println!(
                    "supervisor default set to {}",
                    effective_args.supervisor_cli
                );
            }
        }
    }
    let args = &effective_args;

    let preflight = preflight_factory_launch(args, &cwd, cas_root)?;
    if !preflight.notices.is_empty() {
        let theme = crate::ui::theme::ActiveTheme::default();
        let mut stdout = std::io::stdout();
        let mut fmt = crate::ui::components::Formatter::stdout(&mut stdout, theme);
        for note in &preflight.notices {
            fmt.info(note)?;
        }
    }
    let codex_home = codex_account_home_suffix(&args.supervisor_cli);
    if !codex_home.is_empty() {
        let theme = crate::ui::theme::ActiveTheme::default();
        let mut stdout = std::io::stdout();
        let mut fmt = crate::ui::components::Formatter::stdout(&mut stdout, theme);
        fmt.info(&format!("Resolving factory session{codex_home}"))?;
    }

    // Auto-attach to existing session, or kill it if --new
    if !args.legacy && args.name.is_none() {
        let project_dir = cwd.to_string_lossy();
        if let Ok(Some(session)) = find_session_for_project(&project_dir, None) {
            if session.can_attach() {
                if args.start_new {
                    // --new: kill existing session before starting fresh
                    let theme = crate::ui::theme::ActiveTheme::default();
                    let mut stdout = std::io::stdout();
                    let mut fmt = crate::ui::components::Formatter::stdout(&mut stdout, theme);
                    fmt.info(&format!(
                        "Found running session: {} (workers: {}, pid: {}){codex_home}",
                        session.name,
                        session.worker_count(),
                        session.metadata.daemon_pid
                    ))?;
                    if crate::cli::interactive::confirm(
                        "Kill existing session and start fresh?",
                        true,
                    )? {
                        lifecycle::kill_session_if_running(&session.name)?;
                        fmt.info(&format!("Killed. Starting new session...{codex_home}"))?;
                        fmt.newline()?;
                    } else {
                        fmt.info("Attaching to existing session instead.")?;
                        fmt.newline()?;
                        return attach(Some(session.name));
                    }
                } else if args.attach {
                    // --attach: skip prompt, attach directly
                    let theme = crate::ui::theme::ActiveTheme::default();
                    let mut stdout = std::io::stdout();
                    let mut fmt = crate::ui::components::Formatter::stdout(&mut stdout, theme);
                    fmt.info(&format!(
                        "Attaching to running session: {}{codex_home}",
                        session.name
                    ))?;
                    fmt.newline()?;
                    return attach(Some(session.name));
                } else {
                    // Default: prompt user
                    let theme = crate::ui::theme::ActiveTheme::default();
                    let mut stdout = std::io::stdout();
                    let mut fmt = crate::ui::components::Formatter::stdout(&mut stdout, theme);
                    fmt.info(&format!(
                        "Found running session: {} (workers: {}, pid: {}){codex_home}",
                        session.name,
                        session.worker_count(),
                        session.metadata.daemon_pid
                    ))?;
                    if crate::cli::interactive::confirm("Attach to existing session?", true)? {
                        fmt.newline()?;
                        return attach(Some(session.name));
                    } else {
                        fmt.info(&format!(
                            "Starting new session...{codex_home} (use --new to skip this prompt)"
                        ))?;
                        fmt.newline()?;
                    }
                }
            }
        }
    }

    // Determine theme variant early so we can use themed names
    let theme_variant = {
        let cd = cwd.join(".cas");
        let cr = cas_root.or_else(|| {
            if cd.exists() {
                Some(cd.as_path())
            } else {
                None
            }
        });
        cr.and_then(|r| Config::load(r).ok())
            .and_then(|c| c.theme.as_ref().map(|t| t.variant))
            .unwrap_or_default()
    };
    let is_minions = theme_variant == crate::ui::theme::ThemeVariant::Minions;

    let (supervisor_name, worker_names) = if is_minions {
        use crate::orchestration::names::{generate_minion_supervisor, generate_minion_unique};
        let sup = generate_minion_supervisor();
        let workers = generate_minion_unique(args.workers as usize);
        (sup, workers)
    } else {
        let all_names = generate_unique(args.workers as usize + 1);
        let sup = all_names[0].clone();
        let workers: Vec<String> = all_names[1..].to_vec();
        (sup, workers)
    };

    let session_name = args
        .name
        .clone()
        .unwrap_or_else(|| generate_session_name(Some(&cwd.to_string_lossy())));

    let orphans_killed = lifecycle::cleanup_orphaned_daemons();
    if orphans_killed > 0 {
        tracing::info!("Cleaned up {} orphaned daemon(s)", orphans_killed);
    }

    if args.name.is_some() {
        match lifecycle::kill_session_if_running(&session_name) {
            Ok(true) => tracing::info!("Killed existing session: {}", session_name),
            Ok(false) => {}
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Failed to kill existing session '{session_name}': {e}"
                ));
            }
        }
    }

    let notify_config = NotifyConfig {
        enabled: args.notify,
        backend: NotifyBackend::detect(),
        also_bell: args.bell,
    };

    let cas_config = Config::load(&preflight.cas_root).unwrap_or_default();
    let auto_prompt = cas_config.orchestration().auto_prompt;
    let llm = cas_config.llm();
    // cas-9829: activity-based stall detection threshold, `[factory]
    // stall_threshold_secs` in `.cas/config.toml`.
    let stall_threshold_secs = cas_config.factory().stall_threshold_secs;

    // cas-0bf4: the factory.cargo_build_jobs + factory.nice_cargo config
    // bridge now fires in cli::run BEFORE the telemetry background thread
    // is spawned (see `apply_resource_contention_env` below + its call
    // site in `cli::run`). Doing the `std::env::set_var` here is UB in a
    // multi-threaded process — adversarial review cas-0bf4 P0 flagged
    // that `initialize_telemetry` spawns a PostHog thread before this
    // point. No-op at this site; the env is already set by the time
    // worker PTYs spawn.

    // Resolve per-worker specs from the cascade (cas-2992): config files,
    // CLI flags, and per-worker JSON overrides in priority order.
    let (mut resolved_worker_specs, worker_fallback_model, lane_route_notice) = {
        if let Some(lane) = args.lane.as_deref() {
            let (specs, notice) = resolve_lane_worker_specs(
                lane,
                args.workers as usize,
                &worker_names,
                preflight.worker_cli,
                &args.worker_spec,
                &preflight.capability_snapshot,
            )?;
            (specs, None, Some(notice))
        } else {
            // EPIC cas-8888 (cas-9a31, Phase 1) SILENT SITE — audited: this
            // `!= Claude` check is harness-agnostic by construction ("only pass
            // an explicit override when it's not the Claude default"), so Grok
            // already flows through correctly as `Some(Grok)` — no code change
            // needed here.
            let sources = ConfigSources {
                cli_flag: if preflight.worker_cli != cas_mux::SupervisorCli::Claude {
                    Some(preflight.worker_cli)
                } else {
                    None
                },
                model_flag: llm.model_for_role("worker").map(String::from),
                effort_flag: llm
                    .reasoning_effort_for_role("worker")
                    .and_then(|s| s.parse().ok()),
                config_dir_flag: None,
                worker_spec_jsons: args.worker_spec.clone(),
                supervisor_spec_json: None,
                user_config: None, // auto-resolve from home dir
                project_config: Some(cwd.join(".cas").join("config.toml")),
            };
            let fallback_model = cas_factory::configured_factory_default_model(&sources)
                .map_err(|e| anyhow::anyhow!("Failed to resolve factory defaults: {e}"))?;
            let specs = resolve_specs(args.workers as usize, sources)
                .map_err(|e| anyhow::anyhow!("Failed to resolve worker specs: {e}"))?;
            for spec in &specs {
                cas_factory::validate_explicit(spec, &CapabilitySnapshot::default())
                    .map_err(|e| anyhow::anyhow!("Failed to validate worker routing spec: {e}"))?;
            }
            (specs, fallback_model, None)
        }
    };

    if let Some(notice) = lane_route_notice.as_deref() {
        tracing::info!(target: "cas::factory", "Factory lane route: {notice}");
        let theme = crate::ui::theme::ActiveTheme::default();
        let mut stdout = std::io::stdout();
        let mut fmt = crate::ui::components::Formatter::stdout(&mut stdout, theme);
        fmt.info(&format!("Factory lane route: {notice}"))?;
    }

    // cas-7199 / cas-a487: any resolved worker spec that landed on Codex
    // (built-in default since cas-fbac, project config, CLI flag, or a
    // per-worker --worker-spec/[[factory.workers]] override — this single
    // post-cascade check catches all of those regardless of which layer
    // set it) must actually be usable. Default mode rewrites to Claude
    // with a loud, non-suppressible notice; --strict-cli / [factory]
    // strict_cli bails instead. An incompatible Codex model is replaced by
    // the cascaded `[factory.defaults].model`, if configured.
    let strict_cli = args.strict_cli || cas_config.factory().strict_cli;
    let codex_fallback_notices = if args.lane.is_some() {
        // Lane resolution owns capability-aware substitution and warning
        // semantics. The legacy Codex-only fallback must not rewrite a lane
        // decision behind the lane receipt's back.
        Vec::new()
    } else {
        cas_factory::apply_codex_fallback(
            &mut resolved_worker_specs,
            strict_cli,
            worker_fallback_model.as_deref(),
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?
    };
    for notice in &codex_fallback_notices {
        tracing::warn!(target: "cas::factory", "{notice}");
    }
    if !codex_fallback_notices.is_empty() {
        let theme = crate::ui::theme::ActiveTheme::default();
        let mut stdout = std::io::stdout();
        let mut fmt = crate::ui::components::Formatter::stdout(&mut stdout, theme);
        for notice in &codex_fallback_notices {
            fmt.warning(notice)?;
        }
    }

    // Resolve supervisor spec from the cascade (cas-1948): [factory.supervisor] TOML
    // + --supervisor-cli/model/effort CLI flags + --supervisor-spec JSON override.
    let (mut resolved_supervisor_spec, supervisor_fallback_model) = {
        // EPIC cas-8888 (cas-9a31, Phase 1) SILENT SITE — audited, same as
        // resolved_worker_specs above: harness-agnostic, Grok flows through
        // as Some(Grok) correctly.
        let sources = ConfigSources {
            cli_flag: if preflight.supervisor_cli != cas_mux::SupervisorCli::Claude {
                Some(preflight.supervisor_cli)
            } else {
                None
            },
            model_flag: llm.model_for_role("supervisor").map(String::from),
            effort_flag: llm
                .reasoning_effort_for_role("supervisor")
                .and_then(|s| s.parse().ok()),
            config_dir_flag: None,
            worker_spec_jsons: vec![],
            supervisor_spec_json: args.supervisor_spec.clone(),
            user_config: None, // auto-resolve from home dir
            project_config: Some(cwd.join(".cas").join("config.toml")),
        };
        let fallback_model = cas_factory::configured_factory_default_model(&sources)
            .map_err(|e| anyhow::anyhow!("Failed to resolve factory defaults: {e}"))?;
        let spec = resolve_supervisor_spec(sources)
            .map_err(|e| anyhow::anyhow!("Failed to resolve supervisor spec: {e}"))?;
        cas_factory::validate_explicit(&spec, &CapabilitySnapshot::default())
            .map_err(|e| anyhow::anyhow!("Failed to validate supervisor routing spec: {e}"))?;
        (spec, fallback_model)
    };

    // cas-7199 / cas-a487: same Codex-availability fallback as
    // resolved_worker_specs above, applied to the supervisor slot too — a
    // supervisor explicitly configured to `codex` (`[factory.supervisor]`
    // / `--supervisor-spec`) hits the identical bug, and the blast radius
    // is worse: a failed worker costs one lane, a failed supervisor costs
    // the whole session. `apply_codex_fallback_for_supervisor` (not the
    // worker variant) so the notice is unmistakably about the supervisor,
    // not read as just another worker line.
    let supervisor_codex_fallback_notices = cas_factory::apply_codex_fallback_for_supervisor(
        &mut resolved_supervisor_spec,
        strict_cli,
        supervisor_fallback_model.as_deref(),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    for notice in &supervisor_codex_fallback_notices {
        tracing::warn!(target: "cas::factory", "{notice}");
    }
    if !supervisor_codex_fallback_notices.is_empty() {
        let theme = crate::ui::theme::ActiveTheme::default();
        let mut stdout = std::io::stdout();
        let mut fmt = crate::ui::components::Formatter::stdout(&mut stdout, theme);
        for notice in &supervisor_codex_fallback_notices {
            fmt.warning(notice)?;
        }
    }

    // Build native Agent Teams spawn configs so agents start with Teams CLI flags.
    let (teams_configs, lead_session_id) = {
        use crate::ui::factory::daemon::runtime::teams::TeamsManager;
        TeamsManager::build_configs_for_mux(&session_name, &supervisor_name, &worker_names)
    };

    let config = FactoryConfig {
        cwd: cwd.clone(),
        workers: args.workers as usize,
        worker_names: worker_names.clone(),
        supervisor_name: Some(supervisor_name),
        supervisor_cli: preflight.supervisor_cli,
        worker_cli: preflight.worker_cli,
        supervisor_model: llm.model_for_role("supervisor").map(String::from),
        worker_model: llm.model_for_role("worker").map(String::from),
        supervisor_effort: llm
            .reasoning_effort_for_role("supervisor")
            .map(String::from),
        worker_effort: llm.reasoning_effort_for_role("worker").map(String::from),
        resolved_worker_specs,
        resolved_supervisor_spec: Some(resolved_supervisor_spec),
        enable_worktrees: preflight.enable_worktrees,
        worktree_root: args.worktree_root.clone(),
        notify: notify_config,
        tabbed_workers: args.tabbed,
        auto_prompt,
        record: args.record,
        session_id: if args.record {
            Some(session_name.clone())
        } else {
            None
        },
        teams_configs,
        lead_session_id: Some(lead_session_id),
        minions_theme: is_minions,
        stall_threshold_secs,
        stall_after_secs: cas_config.factory().stall_after_secs,
        ai_enrichment: cas_config.factory().ai_enrichment,
    };

    let phone_home = !args.no_phone_home;

    if args.legacy {
        daemon::execute_legacy_daemon(session_name, config, phone_home)
    } else {
        daemon::run_factory_with_daemon(session_name, config, phone_home)
    }
}

fn execute_unified_preflight(cli: &Cli, cas_root: Option<&std::path::Path>) -> Result<()> {
    let project_root = std::env::current_dir()?;
    let default_cas_root = project_root.join(".cas");
    let cas_root = cas_root.unwrap_or(&default_cas_root);
    let report =
        crate::factory_preflight::collect_factory_preflight(&project_root, cas_root, false, None);
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{}",
            crate::factory_preflight::render_factory_preflight_human(&report)
        );
    }
    if report.factory_blocked {
        anyhow::bail!("factory preflight found critical blockers");
    }
    Ok(())
}

/// Attach to an existing factory session (local or remote via SSH)
pub fn execute_attach(args: &AttachArgs) -> Result<()> {
    if let Some(ref remote_target) = args.remote {
        {
            // If target contains ':', use SSH mode (device:factory-id)
            if remote_target.contains(':') {
                return remote_attach::execute_remote_attach(remote_target, args.worker.as_deref());
            }
            // Otherwise use cloud relay (just factory-id)
            return cloud_attach::execute_cloud_attach(remote_target);
        }
    }

    // If no name given and multiple sessions exist, show interactive picker
    let name = match &args.name {
        Some(n) => Some(n.clone()),
        None => {
            let manager = crate::ui::factory::SessionManager::new();
            let sessions = manager.list_sessions()?;
            let attachable: Vec<_> = sessions.iter().filter(|s| s.can_attach()).collect();
            if attachable.len() > 1 {
                let items: Vec<_> = attachable
                    .iter()
                    .map(|s| lifecycle::session_picker_item(s))
                    .collect();
                match crate::cli::interactive::pick("Attach to session", &items)? {
                    Some(idx) => Some(attachable[idx].name.clone()),
                    None => return Ok(()),
                }
            } else {
                None // let attach() handle single/zero sessions
            }
        }
    };

    attach(name)
}

/// Validate Cassy is initialized in the current project
fn validate_cas_root(
    cwd: &std::path::Path,
    cas_root: Option<&std::path::Path>,
) -> Result<std::path::PathBuf> {
    match cas_root {
        Some(root) => {
            let root = root.to_path_buf();
            let cas_parent = root.parent().unwrap_or(&root);
            let is_in_cwd = cas_parent == cwd;
            let is_git_root_ancestor = {
                let mut check = cwd.to_path_buf();
                loop {
                    if check.join(".git").exists() {
                        break check == cas_parent;
                    }
                    if !check.pop() {
                        break false;
                    }
                }
            };

            if !is_in_cwd && !is_git_root_ancestor {
                bail!(
                    "Cassy is not initialized in this project.\n\n\
                    Found Cassy at: {}\n\
                    Current directory: {}\n\n\
                    Run 'cas init' in this project first.",
                    root.display(),
                    cwd.display()
                );
            }
            Ok(root)
        }
        None => {
            bail!(
                "Cassy is not initialized in this directory.\n\n\
                Factory mode requires Cassy for task coordination.\n\n\
                Run 'cas init' first to initialize Cassy."
            );
        }
    }
}

fn noninteractive_factory_hints(
    args: &FactoryArgs,
    cwd: &std::path::Path,
    cas_root: Option<&std::path::Path>,
) -> Vec<String> {
    let mut hints = Vec::new();

    if validate_cas_root(cwd, cas_root).is_err() {
        hints.push("Initialize Cassy first with `cas doctor --fix` (or `cas init`).".to_string());
    }

    if args.workers > 0 && !args.no_worktrees {
        if !GitOperations::is_git_available() {
            hints.push("Install git to enable worker worktree isolation.".to_string());
        } else {
            match GitOperations::detect_repo_root(cwd) {
                Ok(repo_root) => {
                    let git = GitOperations::new(repo_root);
                    if !git.has_commits().unwrap_or(false) {
                        hints.push(
                            "Create an initial commit before launching workers: `git add . && git commit -m \"Initial commit\"`."
                                .to_string(),
                        );
                    }
                }
                Err(_) => hints.push(
                    "Initialize git for worker worktrees: `git init && git add . && git commit -m \"Initial commit\"` (or use `--no-worktrees`)."
                        .to_string(),
                ),
            }
        }
    }

    hints
}

fn is_claude_installed() -> bool {
    std::process::Command::new("claude")
        .arg("--version")
        .output()
        .is_ok()
}

/// EPIC cas-8888 (cas-9a31, Phase 1): Grok isn't yet a stock/default CLI
/// (no auto-upgrade or fallback targets it), but an explicit `--cli grok` /
/// `--worker-cli grok` request needs a real installed check.
fn is_grok_installed() -> bool {
    std::process::Command::new("grok")
        .arg("--version")
        .output()
        .is_ok()
}

fn is_opencode_installed() -> bool {
    std::process::Command::new("opencode")
        .arg("--version")
        .output()
        .is_ok()
}

fn parse_supervisor_cli(value: &str) -> Result<cas_mux::SupervisorCli> {
    value
        .parse::<cas_mux::SupervisorCli>()
        .map_err(|_| anyhow::anyhow!("Invalid CLI '{value}'. Use 'claude', 'codex', or 'grok'."))
}

struct FactoryPreflight {
    cas_root: std::path::PathBuf,
    supervisor_cli: cas_mux::SupervisorCli,
    worker_cli: cas_mux::SupervisorCli,
    capability_snapshot: CapabilitySnapshot,
    enable_worktrees: bool,
    notices: Vec<String>,
}

fn resolve_cli_choice(
    role: &str,
    requested: &str,
    claude_installed: bool,
    grok_installed: bool,
    codex_available: impl FnOnce() -> bool,
) -> Result<cas_mux::SupervisorCli> {
    let parsed = parse_supervisor_cli(requested)?;

    // cas-1c3a / cas-5eaf: `apply_codex_fallback` remains the post-cascade
    // authority for codex -> claude and strict-mode policy. Preflight only
    // consults the SAME shared binary+auth probe when Claude is absent, so it
    // can reject the otherwise-unrecoverable "neither harness is available"
    // case. When Claude can serve as fallback, Codex passes through for the
    // resolver to decide after the full config cascade.
    match parsed {
        cas_mux::SupervisorCli::Codex if claude_installed || codex_available() => Ok(parsed),
        cas_mux::SupervisorCli::Codex => bail!(
            "{role} 'codex' is unavailable (a runnable Codex CLI and \
             ~/.codex/auth.json login are required), and fallback 'claude' is \
             not installed. Run `codex login` after installing Codex, or install \
             Claude with: npm install -g @anthropic-ai/claude-cli"
        ),
        cas_mux::SupervisorCli::Claude if claude_installed => Ok(parsed),
        cas_mux::SupervisorCli::Claude => bail!(
            "{role} 'claude' is not installed. Install with: npm install -g @anthropic-ai/claude-cli"
        ),
        cas_mux::SupervisorCli::Grok if grok_installed => Ok(parsed),
        cas_mux::SupervisorCli::Grok => bail!(
            "{role} 'grok' is not installed. Install the xAI Grok Build CLI: \
             curl -fsSL https://x.ai/cli/install.sh | bash"
        ),
        cas_mux::SupervisorCli::OpenCode if is_opencode_installed() => Ok(parsed),
        cas_mux::SupervisorCli::OpenCode => bail!(
            "{role} 'opencode' is not installed. Install the OpenCode CLI before spawning"
        ),
    }
}

fn preflight_factory_launch(
    args: &FactoryArgs,
    cwd: &std::path::Path,
    cas_root: Option<&std::path::Path>,
) -> Result<FactoryPreflight> {
    let mut failures: Vec<String> = Vec::new();
    let mut notices: Vec<String> = Vec::new();
    let mut missing_cas = false;
    let mut missing_git_repo = false;
    let mut missing_initial_commit = false;
    let mut missing_claude_commit = false;
    let mut missing_mcp_commit = false;

    let resolved_cas_root = match validate_cas_root(cwd, cas_root) {
        Ok(path) => Some(path),
        Err(_) => {
            failures.push("Cassy is not initialized in this project. Run `cas init`.".to_string());
            missing_cas = true;
            None
        }
    };

    if let Some(cas_root) = resolved_cas_root.as_deref() {
        let config = Config::load(cas_root).unwrap_or_default();
        let policy = crate::factory_target_cache::TargetCachePolicy::from(config.factory());
        match crate::factory_target_cache::capacity_status(cwd, policy) {
            Ok(capacity) if capacity.pressure => {
                let warning = format!(
                    "Cargo target-cache disk pressure: filesystem is {}% used (factory high watermark {}%, low watermark {}%). Run coordination gc_report, review TARGET_CACHE_STATUS_JSON, then explicitly run gc_cleanup force=true dry_run=false.",
                    capacity.used_percent,
                    capacity.high_watermark_percent,
                    capacity.low_watermark_percent,
                );
                if args.workers > 0 {
                    failures.push(format!(
                        "{warning} Worker launch is blocked before builds can reach ENOSPC; start supervisor-only with --workers 0 to inspect and reclaim."
                    ));
                } else {
                    notices.push(warning);
                }
            }
            Ok(_) => {}
            Err(error) => notices.push(format!(
                "Cargo target-cache capacity probe unavailable (cleanup will fail closed): {error}"
            )),
        }
    }

    let claude_installed = is_claude_installed();
    let grok_installed = is_grok_installed();

    let supervisor_cli = match resolve_cli_choice(
        "Supervisor CLI",
        &args.supervisor_cli,
        claude_installed,
        grok_installed,
        cas_factory::probe::codex_available,
    ) {
        Ok(cli) => Some(cli),
        Err(e) => {
            failures.push(e.to_string());
            None
        }
    };
    let worker_cli = if args.workers > 0 {
        match resolve_cli_choice(
            "Worker CLI",
            &args.worker_cli,
            claude_installed,
            grok_installed,
            cas_factory::probe::codex_available,
        ) {
            Ok(cli) => Some(cli),
            Err(e) => {
                failures.push(e.to_string());
                None
            }
        }
    } else {
        resolve_cli_choice(
            "Worker CLI",
            &args.worker_cli,
            claude_installed,
            grok_installed,
            cas_factory::probe::codex_available,
        )
        .ok()
        .or(supervisor_cli)
    };

    let mut enable_worktrees = !args.no_worktrees;
    if enable_worktrees {
        if !GitOperations::is_git_available() {
            if args.workers == 0 {
                enable_worktrees = false;
                notices.push(
                    "Git not found; starting supervisor-only in shared-directory mode. Install git to enable worktree isolation."
                        .to_string(),
                );
            } else {
                failures.push(
                    "Git is required for default factory worktrees. Install git.".to_string(),
                );
            }
        } else {
            match GitOperations::detect_repo_root(cwd) {
                Ok(repo_root) => {
                    let git = GitOperations::new(repo_root);
                    if !git.has_commits().unwrap_or(false) {
                        if args.workers == 0 {
                            enable_worktrees = false;
                            notices.push(
                                "No initial commit detected; starting supervisor-only in shared-directory mode. Create a first commit to enable worktree isolation."
                                    .to_string(),
                            );
                        } else {
                            failures.push(
                                "Repository has no commits. Create an initial commit before starting factory."
                                    .to_string(),
                            );
                            missing_initial_commit = true;
                        }
                    }
                }
                Err(_) => {
                    if args.workers == 0 {
                        enable_worktrees = false;
                        notices.push(
                            "Not in a git repository; starting supervisor-only in shared-directory mode. Run `git init` + first commit to enable worktree isolation."
                                .to_string(),
                        );
                    } else {
                        failures.push(
                            "Default factory mode requires a git repository. Run `git init` or use `cas factory --no-worktrees`."
                                .to_string(),
                        );
                        missing_git_repo = true;
                    }
                }
            }
        }
    }

    // Check if .claude/ is committed (required for worktree-based workers)
    if enable_worktrees && !missing_git_repo && !missing_initial_commit {
        let claude_tracked = std::process::Command::new("git")
            .args(["ls-files", "--error-unmatch", ".claude/settings.json"])
            .current_dir(cwd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !claude_tracked {
            if args.workers > 0 {
                failures.push(
                    ".claude/ directory is not committed. Workers need it in their worktrees."
                        .to_string(),
                );
                missing_claude_commit = true;
            } else {
                notices.push(
                    ".claude/ directory is not committed. Commit it before spawning workers: git add .claude/ CLAUDE.md .mcp.json .gitignore && git commit -m \"Configure Cassy\""
                        .to_string(),
                );
            }
        }
    }

    // Check if .mcp.json is committed (required for worktree-based workers)
    if enable_worktrees && !missing_git_repo && !missing_initial_commit {
        let mcp_tracked = std::process::Command::new("git")
            .args(["ls-files", "--error-unmatch", ".mcp.json"])
            .current_dir(cwd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !mcp_tracked {
            if args.workers > 0 {
                failures.push(
                    ".mcp.json is not committed. Workers need it for MCP tool access in their worktrees."
                        .to_string(),
                );
                missing_mcp_commit = true;
            } else {
                notices.push(
                    ".mcp.json is not committed. Commit it before spawning workers: git add .mcp.json && git commit -m \"Configure CAS MCP\""
                        .to_string(),
                );
            }
        }
    }

    if !failures.is_empty() {
        let details = failures
            .iter()
            .map(|f| format!("  - {f}"))
            .collect::<Vec<_>>()
            .join("\n");

        let mut msg = String::from("Factory preflight failed:\n");
        msg.push_str(&details);

        let mut steps: Vec<String> = Vec::new();
        if missing_git_repo {
            steps.push("git init".to_string());
        }
        if missing_git_repo || missing_initial_commit {
            steps.push("git add .".to_string());
            steps.push("git commit -m \"Initial commit\"".to_string());
        }
        if missing_cas {
            steps.push("cas init".to_string());
        }
        if missing_claude_commit {
            steps.push("git add .claude/ CLAUDE.md .mcp.json .gitignore".to_string());
            steps.push("git commit -m \"Configure Cassy\"".to_string());
        }
        if missing_mcp_commit && !missing_claude_commit {
            steps.push("git add .mcp.json".to_string());
            steps.push("git commit -m \"Configure CAS MCP\"".to_string());
        }
        let launch = if args.no_worktrees {
            "cas factory --no-worktrees"
        } else {
            "cas"
        };
        if !steps.is_empty() {
            steps.push(launch.to_string());
            msg.push_str("\n\nQuick start:");
            for (i, step) in steps.iter().enumerate() {
                msg.push_str(&format!("\n  {}) {}", i + 1, step));
            }
        }
        bail!(msg);
    }

    Ok(FactoryPreflight {
        cas_root: resolved_cas_root.expect("preflight must set cas_root on success"),
        supervisor_cli: supervisor_cli.expect("preflight must parse supervisor_cli on success"),
        worker_cli: worker_cli.expect("preflight must parse worker_cli on success"),
        capability_snapshot: if args.lane.is_some() {
            crate::factory_preflight::collect_live_capability_snapshot()
        } else {
            CapabilitySnapshot::default()
        },
        enable_worktrees,
        notices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::factory::FactoryConfig;

    #[test]
    fn taste_lane_cli_resolves_fable_medium() {
        let (specs, notice) = resolve_lane_worker_specs(
            "taste",
            1,
            &["taste-worker".to_string()],
            cas_mux::SupervisorCli::Claude,
            &[],
            &CapabilitySnapshot::default(),
        )
        .unwrap();
        assert_eq!(specs[0].name.as_deref(), Some("taste-worker"));
        assert_eq!(specs[0].cli, cas_mux::SupervisorCli::Claude);
        assert_eq!(specs[0].model.as_deref(), Some("claude-fable-5-1"));
        assert_eq!(specs[0].effort, Some(cas_mux::Effort::Medium));
        assert!(notice.contains("claude_fable"), "{notice}");
    }

    #[test]
    fn supervisor_lane_cli_launch_summary_names_opus_fallback() {
        let registry = cas_factory::embedded_registry().unwrap();
        let now = cas_factory::CapabilitySnapshot::now_ms();
        let mut snapshot = CapabilitySnapshot::default();
        snapshot.record(
            cas_factory::recipe_route_identity(
                &registry.recipes["claude_fable"],
                "default",
            ),
            cas_factory::CapabilityEvidence::new(
                cas_factory::CapabilityAvailability::Unavailable,
                now,
            )
            .with_reason("Claude Fable account unavailable"),
        );
        snapshot.record(
            cas_factory::recipe_route_identity(
                &registry.recipes["claude_opus"],
                "default",
            ),
            cas_factory::CapabilityEvidence::new(
                cas_factory::CapabilityAvailability::Available,
                now,
            ),
        );

        let (specs, notice) = resolve_lane_worker_specs(
            "supervisor",
            1,
            &["supervisor-worker".to_string()],
            cas_mux::SupervisorCli::Claude,
            &[],
            &snapshot,
        )
        .expect("supervisor fallback should resolve");
        assert_eq!(specs[0].model.as_deref(), Some("claude-opus-5"));
        assert_eq!(specs[0].effort, Some(cas_mux::Effort::High));
        assert!(
            notice.contains(
                "fallback: claude_opus (primary claude_fable unavailable: Claude Fable account unavailable)"
            ),
            "{notice}"
        );
    }

    // EPIC cas-8888 (cas-964a, Phase 3): resolve_cli_choice's Grok arm.
    #[test]
    fn resolve_cli_choice_grok_installed_passes_through() {
        let result = resolve_cli_choice("supervisor", "grok", true, true, || {
            panic!("Grok resolution must not probe Codex")
        });
        assert_eq!(result.unwrap(), cas_mux::SupervisorCli::Grok);
    }

    #[test]
    fn resolve_cli_choice_grok_missing_bails_with_installer_command() {
        let err = resolve_cli_choice("supervisor", "grok", true, false, || {
            panic!("Grok resolution must not probe Codex")
        })
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("'grok' is not installed"), "got: {msg}");
        assert!(
            msg.contains("curl -fsSL https://x.ai/cli/install.sh | bash"),
            "bail message should carry the exact installer command per cas-964a's AC, got: {msg}"
        );
    }

    #[test]
    fn resolve_cli_choice_grok_missing_does_not_silently_fall_back() {
        // An explicit `--cli grok` request that's not installed must bail
        // rather than silently swap to Claude.
        let result = resolve_cli_choice("supervisor", "grok", true, false, || {
            panic!("Grok resolution must not probe Codex")
        });
        assert!(result.is_err());
    }

    /// cas-7199 / cas-a487 (cas-e9e9 decision): missing `claude` must ALWAYS
    /// bail, never silently swap to `codex`. `apply_codex_fallback` is
    /// deliberately one-way and never touches Claude specs.
    #[test]
    fn resolve_cli_choice_claude_missing_never_falls_back_to_codex() {
        let err = resolve_cli_choice("supervisor", "claude", false, true, || {
            panic!("Claude resolution must not probe Codex")
        })
        .unwrap_err();
        assert!(
            err.to_string().contains("'claude' is not installed"),
            "got: {err}"
        );
    }

    /// cas-1c3a: preflight must not classify Codex from binary presence or
    /// rewrite it. The post-cascade resolver owns the binary+auth probe,
    /// codex -> claude fallback, and strict-mode error.
    #[test]
    fn resolve_cli_choice_codex_defers_to_post_cascade_authority() {
        let result = resolve_cli_choice("supervisor", "codex", true, false, || {
            panic!("Codex availability is irrelevant when its Claude fallback is installed")
        });
        assert_eq!(
            result.unwrap(),
            cas_mux::SupervisorCli::Codex,
            "preflight must preserve Codex for apply_codex_fallback to probe"
        );
    }

    #[test]
    fn resolve_cli_choice_codex_available_without_claude_passes_through() {
        let result = resolve_cli_choice("supervisor", "codex", false, false, || true);
        assert_eq!(result.unwrap(), cas_mux::SupervisorCli::Codex);
    }

    #[test]
    fn resolve_cli_choice_codex_and_claude_unavailable_errors_clearly() {
        let err = resolve_cli_choice("Worker CLI", "codex", false, false, || false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("'codex' is unavailable"), "got: {msg}");
        assert!(
            msg.contains("fallback 'claude' is not installed"),
            "must explain that neither requested nor fallback harness can run: {msg}"
        );
        assert!(
            msg.contains("codex login"),
            "must give Codex remediation: {msg}"
        );
        assert!(
            msg.contains("npm install -g @anthropic-ai/claude-cli"),
            "must give Claude remediation: {msg}"
        );
    }

    #[test]
    fn test_factory_config_default() {
        let config = FactoryConfig::default();
        assert_eq!(config.workers, 0);
        assert!(config.worker_names.is_empty());
        assert!(config.supervisor_name.is_none());
        assert_eq!(config.supervisor_cli, cas_mux::SupervisorCli::Claude);
        assert_eq!(config.worker_cli, cas_mux::SupervisorCli::Claude);
        assert!(config.enable_worktrees);
        assert!(config.worktree_root.is_none());
    }

    #[test]
    fn test_factory_args_default_has_attach_false() {
        let args = FactoryArgs::default();
        assert!(!args.attach, "--attach should default to false");
        assert!(!args.start_new, "--new should default to false");
    }

    // cas-2992: --worker-spec flag integration
    #[test]
    fn test_factory_args_worker_spec_field_defaults_empty() {
        let args = FactoryArgs::default();
        assert!(
            args.worker_spec.is_empty(),
            "--worker-spec should default to empty vec"
        );
    }

    #[test]
    fn test_factory_args_worker_spec_feeds_resolve_specs() {
        // Verify that the --worker-spec JSONs are consumed by resolve_specs
        // to produce the correct resolved_worker_specs for a FactoryConfig.
        // This is a unit test of the resolver wiring, not the full execute() flow.
        let worker_spec_json = r#"{"cli":"codex"}"#.to_string();
        let args = FactoryArgs {
            workers: 2,
            worker_spec: vec![worker_spec_json],
            ..FactoryArgs::default()
        };

        let sources = ConfigSources {
            worker_spec_jsons: args.worker_spec.clone(),
            // Skip config files: pass non-existent paths so resolver treats them as absent
            user_config: Some(std::path::PathBuf::from(
                "/nonexistent/user/.cas/config.toml",
            )),
            project_config: Some(std::path::PathBuf::from(
                "/nonexistent/project/.cas/config.toml",
            )),
            ..Default::default()
        };
        let specs =
            resolve_specs(args.workers as usize, sources).expect("resolve_specs should succeed");

        assert_eq!(specs.len(), 2, "should produce one spec per worker");
        // First slot gets the JSON override (codex)
        assert_eq!(
            specs[0].cli,
            cas_mux::SupervisorCli::Codex,
            "first worker spec should be Codex from JSON override"
        );
        // Second slot falls back to builtin default (Claude) since only one spec JSON was given
        assert_eq!(
            specs[1].cli,
            cas_mux::SupervisorCli::Claude,
            "second worker spec should be Claude (builtin default)"
        );
    }

    #[test]
    fn test_factory_args_attach_and_new_are_independent() {
        // Both flags can be set independently
        let mut args = FactoryArgs::default();
        args.attach = true;
        assert!(args.attach);
        assert!(!args.start_new);

        let mut args = FactoryArgs::default();
        args.start_new = true;
        assert!(!args.attach);
        assert!(args.start_new);
    }

    #[test]
    fn test_session_manager_no_sessions() {
        // Characterization: find_session_for_project returns None when no sessions exist
        let manager = crate::ui::factory::SessionManager::new();
        let result = manager
            .find_session_for_project(None, "/nonexistent/project/path")
            .unwrap();
        assert!(result.is_none());
    }

    // ------------------------------------------------------------------
    // cas-c614: single-thread precondition guard tests
    // ------------------------------------------------------------------
    //
    // Scope: `check_single_threaded_precondition` and its parser helper.
    // We do **not** exercise `apply_resource_contention_env` directly
    // here because the `ENV_BRIDGE_INVOKED` OnceLock is process-global
    // and the real `cli::run` call site may already have latched it in
    // other test-harness configurations. The helper-level tests are
    // sufficient to prove that a post-thread-spawn invocation would
    // trip the debug-assert.

    /// Fixture mirroring the shape of a real /proc/<pid>/stat line with
    /// `num_threads = 1`. Field 20 (num_threads) is the fourth numeric
    /// after the closing `)` in this layout.
    #[cfg(target_os = "linux")]
    const PROC_STAT_FIXTURE_1_THREAD: &str = "12345 (cas) S 1 12345 12345 \
        0 -1 4194304 100 0 0 0 10 20 0 0 20 0 1 0 1000000 123456789 1024 \
        18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0 0 0 \
        0 0 0 0 0 0 0\n";

    /// Same shape but with `num_threads = 4`.
    #[cfg(target_os = "linux")]
    const PROC_STAT_FIXTURE_4_THREADS: &str = "12345 (cas) S 1 12345 12345 \
        0 -1 4194304 100 0 0 0 10 20 0 0 20 0 4 0 1000000 123456789 1024 \
        18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0 0 0 \
        0 0 0 0 0 0 0\n";

    /// Fixture with a comm field containing spaces and inner parens,
    /// which must be handled by splitting on the **last** `)`.
    #[cfg(target_os = "linux")]
    const PROC_STAT_FIXTURE_COMM_WITH_PARENS: &str = "42 (cas (factory)) S \
        1 42 42 0 -1 4194304 100 0 0 0 10 20 0 0 20 0 2 0 1000000 123456789 \
        1024 18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0 0 \
        0 0 0 0 0 0 0 0\n";

    #[test]
    #[cfg(target_os = "linux")]
    fn parse_num_threads_from_proc_stat_reads_field_20_when_one() {
        assert_eq!(
            parse_num_threads_from_proc_stat(PROC_STAT_FIXTURE_1_THREAD),
            Some(1)
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn parse_num_threads_from_proc_stat_reads_field_20_when_many() {
        assert_eq!(
            parse_num_threads_from_proc_stat(PROC_STAT_FIXTURE_4_THREADS),
            Some(4)
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn parse_num_threads_from_proc_stat_handles_comm_with_spaces_and_parens() {
        assert_eq!(
            parse_num_threads_from_proc_stat(PROC_STAT_FIXTURE_COMM_WITH_PARENS),
            Some(2)
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn parse_num_threads_from_proc_stat_rejects_malformed() {
        assert_eq!(parse_num_threads_from_proc_stat(""), None);
        assert_eq!(parse_num_threads_from_proc_stat("no closing paren"), None);
        // Truncated after comm — no field 20.
        assert_eq!(parse_num_threads_from_proc_stat("1 (cas) S 1 1"), None);
    }

    /// CORE AC: the guard trips when invoked from a context that has
    /// already spawned a thread. Because the test harness itself may be
    /// multi-threaded (cargo runs tests on a thread pool), asserting
    /// from the test function's own thread is already sufficient proof
    /// that `>= 2` threads visibly trips the check. On non-Linux
    /// `current_num_threads` returns `None` and the helper is
    /// fail-open — in that case the invariant still holds vacuously.
    #[test]
    fn check_single_threaded_precondition_trips_when_threads_spawned() {
        // Spawn and join a helper thread to guarantee at least one
        // additional thread existed while the probe was inflight. On
        // Linux this deterministically produces num_threads >= 2 in
        // /proc/self/stat for the duration of the spawn.
        let handle = std::thread::spawn(|| check_single_threaded_precondition());
        let from_spawned = handle.join().expect("helper thread must join");

        #[cfg(target_os = "linux")]
        {
            // The helper thread itself runs inside a test harness that
            // has >1 OS threads (cargo's test runner + rayon + tokio
            // pools depending on features). So the probe from the
            // spawned worker must observe >1 threads.
            assert!(
                matches!(
                    from_spawned,
                    Err(PreconditionError::ThreadsAlreadySpawned { count }) if count > 1
                ),
                "expected ThreadsAlreadySpawned from spawned thread, got {:?}",
                from_spawned,
            );
        }

        #[cfg(not(target_os = "linux"))]
        {
            // On non-Linux the helper is fail-open; documenting that
            // explicitly so a future port knows it needs a platform-
            // specific probe.
            assert!(
                from_spawned.is_ok(),
                "non-Linux platforms fail-open: check_single_threaded_precondition \
                 must not invent errors it cannot prove",
            );
        }
    }
}
