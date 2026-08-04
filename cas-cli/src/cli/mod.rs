//! CLI commands for CAS
//!
//! Essential commands only. Use MCP tools for memory, tasks, rules, etc.

mod auth;
pub(crate) mod bridge;
mod changelog;
mod claude;
mod claude_md;
mod codemap_cmd;
mod known_repos;
mod project_overview_cmd;
mod provider_default;
mod sweep;
mod worktree;
// `pub` so integration tests in `cas-cli/tests/` can reach
// `cli::cloud::execute_team_push` (cas-1f44 T4). Internal; no stable API.
pub mod cloud;
mod config;
mod config_tui;
mod device;
mod doctor;
// cas-728b: pub(crate) so ui::factory::director::events can reach
// cli::factory::wedged's transcript-mtime liveness primitives.
pub(crate) mod factory;
mod factory_tooling;
mod hook;
mod init;
pub mod integrate;
pub mod interactive;
mod list;
mod mcp_cmd;
pub mod memory;
mod open;
mod queue;
mod status;
mod statusline;
mod update;
pub mod update_transaction;

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use crate::store::find_cas_root;

pub use auth::AuthCommands;
pub use bridge::BridgeArgs;
pub use changelog::ChangelogArgs;
pub use claude::ClaudeArgs;
pub use claude_md::ClaudeMdArgs;
pub use config::ConfigCommands;
pub use doctor::DoctorArgs;
pub use factory::{AttachArgs, FactoryArgs, KillAllArgs, KillArgs};
pub use hook::HookArgs;
pub use init::InitArgs;
pub use list::ListArgs;
pub use mcp_cmd::McpCommands;
pub use provider_default::DefaultArgs;
pub use status::StatusArgs;
pub use statusline::StatusLineArgs;
pub use open::OpenArgs;
pub use update::UpdateArgs;

/// Build version string including git hash and date
fn build_version() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let git_hash = option_env!("CAS_GIT_HASH").unwrap_or("unknown");
    let build_date = option_env!("CAS_BUILD_DATE").unwrap_or("unknown");
    format!("{version} ({git_hash} {build_date})")
}

const LOGO: &str = r#"
   ______   ___    _____
  / ____/  /   |  / ___/
 / /      / /| |  \__ \
/ /___   / ___ | ___/ /
\____/  /_/  |_|/____/
"#;

/// CAS - Multi-agent coding factory
#[derive(Parser)]
#[command(name = "cas")]
#[command(about = "Multi-agent coding factory with persistent memory and task coordination")]
#[command(version = build_version())]
#[command(before_help = LOGO)]
pub struct Cli {
    /// Output in JSON format
    #[arg(long, global = true)]
    pub json: bool,

    /// Include full content in JSON output
    #[arg(long, global = true)]
    pub full: bool,

    /// Verbose output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Interactive project picker — scan ~/projects/, select, launch or attach
    Open(OpenArgs),

    /// Initialize CAS in current directory
    Init(InitArgs),

    /// Attach to a running factory session
    Attach(AttachArgs),

    /// List running factory sessions
    #[command(alias = "ls")]
    List(ListArgs),

    /// Terminate a factory session
    Kill(KillArgs),

    /// Terminate all factory sessions
    KillAll(KillAllArgs),

    /// Launch factory session (bare `cas` runs factory with defaults)
    Factory(FactoryArgs),

    /// Launch Claude Code under an explicit account profile.
    ///
    /// `main` uses ~/.claude; any other profile uses ~/.claude-<profile>.
    Claude(ClaudeArgs),

    /// Launch factory with Codex as the supervisor (shortcut for `cas factory --supervisor-cli=codex`)
    ///
    /// All `cas factory` flags pass through. Use `--default` to also persist
    /// Codex as the default supervisor for future sessions.
    Codex(FactoryArgs),

    /// Launch factory with Grok as the supervisor (shortcut for `cas factory --supervisor-cli=grok`)
    ///
    /// All `cas factory` flags pass through. Use `--default` to also persist
    /// Grok as the default supervisor for future sessions. Requires the xAI
    /// Grok Build CLI (see https://x.ai) to be installed.
    Grok(FactoryArgs),

    /// Set the default supervisor provider without launching (persist only)
    ///
    /// `cas default codex` — persists `[llm.supervisor] harness = "codex"` to
    /// `~/.cas/config.toml` and exits.  `cas default claude` is symmetric.
    /// `cas default grok` is symmetric. To launch immediately AND persist,
    /// use `cas factory --supervisor-cli <provider> --default`.
    #[command(name = "default")]
    Default(DefaultArgs),

    /// Local helper server for external orchestration tools
    Bridge(BridgeArgs),

    /// Run MCP server for Claude Code integration
    Serve,

    /// Run diagnostics
    Doctor(DoctorArgs),

    /// Manage configuration
    #[command(subcommand)]
    Config(ConfigCommands),

    /// Show session status
    Status(StatusArgs),

    /// Output status line for Claude Code integration
    #[command(alias = "statusline")]
    StatusLine(StatusLineArgs),

    /// Handle Claude Code hook events
    Hook(HookArgs),

    /// Authentication commands (login, logout, whoami)
    #[command(subcommand)]
    Auth(AuthCommands),

    /// Log in to CAS Cloud (shortcut for 'auth login')
    Login(auth::LoginArgs),

    /// Log out (shortcut for 'auth logout')
    Logout,

    /// Show current user (shortcut for 'auth whoami')
    Whoami,

    /// Update CAS to the latest version
    Update(UpdateArgs),

    /// Show release notes and changelog from GitHub releases
    Changelog(ChangelogArgs),

    /// Manage upstream MCP servers
    #[command(subcommand)]
    Mcp(McpCommands),

    /// Prompt queue operations (poll/ack for native extensions)
    #[command(subcommand)]
    Queue(queue::QueueCommands),

    /// Cloud sync (push/pull data to CAS Cloud)
    #[command(subcommand, hide = true)]
    Cloud(cloud::CloudCommands),

    /// Manage registered devices
    #[command(subcommand)]
    Device(device::DeviceCommands),

    /// Evaluate and optimize CLAUDE.md files for token efficiency
    #[command(name = "claude-md")]
    ClaudeMd(ClaudeMdArgs),

    /// Codemap staleness info and pending changes
    #[command(subcommand)]
    Codemap(codemap_cmd::CodemapCommands),

    /// PRODUCT_OVERVIEW.md staleness info and pending changes
    #[command(subcommand, name = "project-overview")]
    ProjectOverview(project_overview_cmd::ProjectOverviewCommands),

    /// Auto-integrate the project with Vercel/Neon/GitHub (writes SKILL files)
    #[command(subcommand)]
    Integrate(integrate::IntegrateCommands),

    /// Share or unshare personal memories with your team (retroactive)
    #[command(subcommand)]
    Memory(memory::MemoryCommands),

    /// Inspect and bootstrap the host-scoped known_repos registry
    #[command(subcommand, name = "known-repos")]
    KnownRepos(known_repos::KnownReposCommands),

    /// Worktree-scoped diagnostics and maintenance (sweep, ...)
    #[command(subcommand)]
    Worktree(worktree::WorktreeCommands),

    /// Shortcut for `cas worktree sweep --all-repos`
    #[command(name = "sweep-all")]
    SweepAll(sweep::SweepBaseArgs),
}

/// Authentication requirement for a command.
#[derive(Copy, Clone, Eq, PartialEq)]
enum AuthRequirement {
    NotRequired,
    Required,
}

/// Determine whether a command requires authentication.
fn auth_requirement(command: &Option<Commands>) -> AuthRequirement {
    let Some(command) = command else {
        // Bare `cas` defaults to local factory behavior.
        return AuthRequirement::NotRequired;
    };

    match command {
        // Auth commands
        Commands::Login(_) | Commands::Logout | Commands::Whoami | Commands::Auth(_) => {
            AuthRequirement::NotRequired
        }

        // Local/offline commands
        Commands::Init(_)
        | Commands::Open(_)
        | Commands::Doctor(_)
        | Commands::Update(_)
        | Commands::Changelog(_)
        | Commands::Hook(_)
        | Commands::Factory(_)
        | Commands::Claude(_)
        | Commands::Codex(_)
        | Commands::Grok(_)
        | Commands::Default(_)
        | Commands::Attach(_)
        | Commands::List(_)
        | Commands::Kill(_)
        | Commands::KillAll(_)
        | Commands::Bridge(_)
        | Commands::Config(_)
        | Commands::Status(_)
        | Commands::StatusLine(_)
        | Commands::Mcp(_)
        | Commands::Queue(_)
        | Commands::ClaudeMd(_)
        | Commands::Codemap(_)
        | Commands::ProjectOverview(_)
        | Commands::Integrate(_)
        | Commands::Memory(_)
        | Commands::KnownRepos(_)
        | Commands::Worktree(_)
        | Commands::SweepAll(_) => AuthRequirement::NotRequired,

        Commands::Serve => AuthRequirement::NotRequired,

        Commands::Cloud(_) => AuthRequirement::Required,

        Commands::Device(_) => AuthRequirement::Required,
    }
}

/// Ensure the user is authenticated before running a command
fn ensure_authenticated() -> anyhow::Result<()> {
    {
        let config = crate::cloud::CloudConfig::load().unwrap_or_default();
        if config.token.is_some() {
            return Ok(());
        }
        anyhow::bail!("Not logged in. Run `cas login` to authenticate.")
    }
}

/// Run the CLI with the given arguments
pub fn run(cli: Cli) -> anyhow::Result<()> {
    let tracer_timer = std::time::Instant::now();
    let dev_tracing_enabled = initialize_dev_tracer();
    let command_name = get_command_name(&cli.command);

    // cas-0bf4: apply the factory resource-contention env bridge BEFORE
    // `initialize_telemetry` spawns its background thread. Any
    // `std::env::set_var` after that spawn is UB in a multi-threaded
    // process. Only mutates env for the factory command path.
    if matches!(
        cli.command,
        Some(Commands::Factory(_))
            | Some(Commands::Codex(_))
            | Some(Commands::Grok(_))
            | None
    ) {
        let early_cas_root = find_cas_root().ok();
        factory::apply_resource_contention_env(early_cas_root.as_deref());
    }

    initialize_telemetry();

    let cas_root: Option<PathBuf> = find_cas_root().ok();

    if auth_requirement(&cli.command) == AuthRequirement::Required {
        ensure_authenticated()?;
    }

    crate::telemetry::track_command(&command_name);

    let result = run_command(&cli, cas_root.as_deref());

    if let Err(ref e) = result {
        let error_type = categorize_error(e);
        crate::telemetry::track_error(&error_type, Some(&command_name), true);
    }

    if dev_tracing_enabled {
        if let Some(tracer) = crate::tracing::DevTracer::get() {
            if tracer.should_trace_commands() {
                let duration_ms = tracer_timer.elapsed().as_millis() as u64;
                let (success, error) = match &result {
                    Ok(_) => (true, None),
                    Err(e) => (false, Some(e.to_string())),
                };
                let _ = tracer.record_command(
                    &command_name,
                    &[],
                    duration_ms,
                    success,
                    error.as_deref(),
                );
            }
        }
    }

    result
}

fn categorize_error(e: &anyhow::Error) -> String {
    let err_str = e.to_string().to_lowercase();
    if err_str.contains("not found") {
        "not_found".to_string()
    } else if err_str.contains("permission") || err_str.contains("access denied") {
        "permission".to_string()
    } else if err_str.contains("network") || err_str.contains("connection") {
        "network".to_string()
    } else if err_str.contains("parse") || err_str.contains("invalid") {
        "parse".to_string()
    } else if err_str.contains("database") || err_str.contains("sqlite") {
        "database".to_string()
    } else if err_str.contains("not initialized") {
        "not_initialized".to_string()
    } else {
        "unknown".to_string()
    }
}

fn initialize_dev_tracer() -> bool {
    use crate::store::find_cas_root;
    use crate::tracing::DevTracer;

    if DevTracer::is_enabled() {
        return true;
    }

    if let Ok(cas_root) = find_cas_root() {
        DevTracer::init_global(&cas_root).unwrap_or(false)
    } else {
        false
    }
}

fn initialize_telemetry() {
    use crate::store::find_cas_root;

    if crate::telemetry::get().is_some() {
        return;
    }

    if let Ok(cas_root) = find_cas_root() {
        if crate::telemetry::init(&cas_root).is_ok() {
            crate::telemetry::track_session_started();
        }
    }
}

fn get_command_name(cmd: &Option<Commands>) -> String {
    let Some(cmd) = cmd else {
        return "factory".to_string();
    };
    match cmd {
        Commands::Open(_) => "open".to_string(),
        Commands::Init(_) => "init".to_string(),
        Commands::Attach(_) => "attach".to_string(),
        Commands::List(_) => "list".to_string(),
        Commands::Kill(_) => "kill".to_string(),
        Commands::KillAll(_) => "kill-all".to_string(),
        Commands::Factory(_) => "factory".to_string(),
        Commands::Claude(_) => "claude".to_string(),
        Commands::Codex(_) => "codex".to_string(),
        Commands::Grok(_) => "grok".to_string(),
        Commands::Default(_) => "default".to_string(),
        Commands::Bridge(_) => "bridge".to_string(),
        Commands::Serve => "serve".to_string(),
        Commands::Doctor(_) => "doctor".to_string(),
        Commands::Config(_) => "config".to_string(),
        Commands::Status(_) => "status".to_string(),
        Commands::StatusLine(_) => "statusline".to_string(),
        Commands::Hook(_) => "hook".to_string(),
        Commands::Auth(_) => "auth".to_string(),
        Commands::Login(_) => "login".to_string(),
        Commands::Logout => "logout".to_string(),
        Commands::Whoami => "whoami".to_string(),
        Commands::Update(_) => "update".to_string(),
        Commands::Changelog(_) => "changelog".to_string(),
        Commands::Mcp(_) => "mcp".to_string(),
        Commands::Queue(_) => "queue".to_string(),
        Commands::Cloud(_) => "cloud".to_string(),
        Commands::Device(_) => "device".to_string(),
        Commands::ClaudeMd(_) => "claude-md".to_string(),
        Commands::Codemap(_) => "codemap".to_string(),
        Commands::ProjectOverview(_) => "project-overview".to_string(),
        Commands::Integrate(_) => "integrate".to_string(),
        Commands::Memory(_) => "memory".to_string(),
        Commands::KnownRepos(_) => "known-repos".to_string(),
        Commands::Worktree(_) => "worktree".to_string(),
        Commands::SweepAll(_) => "sweep-all".to_string(),
    }
}

fn require_cas_root(cas_root: Option<&Path>) -> anyhow::Result<&Path> {
    cas_root.ok_or_else(|| {
        anyhow::anyhow!(
            "CAS not initialized. Run 'cas init' first or navigate to a directory with .cas/"
        )
    })
}

fn run_command(cli: &Cli, cas_root: Option<&Path>) -> anyhow::Result<()> {
    let command = match &cli.command {
        Some(cmd) => cmd,
        None => {
            let default_args = FactoryArgs::default();
            return factory::execute(&default_args, cli, cas_root);
        }
    };

    match command {
        Commands::Open(args) => open::execute(args),
        Commands::Init(args) => init::execute(args, cli),
        Commands::Attach(args) => factory::execute_attach(args),
        Commands::List(args) => factory::execute_list(cli, args),
        Commands::Kill(args) => factory::execute_kill(args.name.as_deref(), args.force),
        Commands::KillAll(args) => factory::execute_kill_all(args.force),
        Commands::Factory(args) => factory::execute(args, cli, cas_root),
        // cas-7f2c: provider shortcuts — preset supervisor_cli + explicit flag,
        // then delegate to the same factory::execute path.
        Commands::Claude(args) => claude::execute(args),
        Commands::Codex(args) => {
            let mut a = args.clone();
            a.supervisor_cli = "codex".to_string();
            a.supervisor_cli_explicit = true;
            factory::execute(&a, cli, cas_root)
        }
        Commands::Grok(args) => {
            let mut a = args.clone();
            a.supervisor_cli = "grok".to_string();
            a.supervisor_cli_explicit = true;
            factory::execute(&a, cli, cas_root)
        }
        Commands::Default(args) => provider_default::execute(args),
        Commands::Bridge(args) => bridge::execute(args, cli),
        Commands::Serve => serve_execute(),
        Commands::Doctor(args) => doctor::execute(args, cli, cas_root),
        Commands::Config(cmd) => config::execute_subcommand(cmd, cli, require_cas_root(cas_root)?),
        Commands::Status(args) => status::execute(args, cli, require_cas_root(cas_root)?),
        Commands::StatusLine(args) => statusline::execute(args, cli, require_cas_root(cas_root)?),
        Commands::Hook(args) => hook::execute(args, cli),
        Commands::Auth(cmd) => auth::execute(cmd, cli),
        Commands::Login(args) => auth::execute(&AuthCommands::Login(args.clone()), cli),
        Commands::Logout => auth::execute(&AuthCommands::Logout, cli),
        Commands::Whoami => auth::execute(&AuthCommands::Whoami, cli),
        Commands::Update(args) => update::execute(args, cli, cas_root),
        Commands::Changelog(args) => changelog::execute(args, cli),
        Commands::Mcp(cmd) => mcp_cmd::execute(cmd, cli, require_cas_root(cas_root)?),
        Commands::Queue(cmd) => queue::execute(cmd, cli),
        Commands::Cloud(cmd) => cloud::execute(cmd, cli, require_cas_root(cas_root)?),
        Commands::Device(cmd) => device::execute(cmd, cli),
        Commands::ClaudeMd(args) => claude_md::execute(args, cli),
        Commands::Codemap(cmd) => codemap_cmd::execute(cmd, cli, require_cas_root(cas_root)?),
        Commands::ProjectOverview(cmd) => {
            project_overview_cmd::execute(cmd, cli, require_cas_root(cas_root)?)
        }
        Commands::Integrate(cmd) => integrate::execute(cmd, cli),
        Commands::Memory(cmd) => memory::execute(cmd, cli, require_cas_root(cas_root)?),
        Commands::KnownRepos(cmd) => known_repos::execute(cmd),
        Commands::Worktree(cmd) => worktree::execute(cmd),
        Commands::SweepAll(args) => sweep::execute_sweep_all(args),
    }
}

fn serve_execute() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(crate::mcp::run_server())
}
