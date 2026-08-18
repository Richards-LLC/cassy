//! CLI commands for Cassy
//!
//! Essential commands only. Use MCP tools for memory, tasks, rules, etc.

mod account_picker;
mod auth;
pub(crate) mod bridge;
mod changelog;
mod claude;
mod claude_md;
mod codex;
mod codemap_cmd;
mod history_cmd;
mod hub;
mod hub_reverse_pairing;
mod hub_service;
mod index_cmd;
mod knowledge_cmd;
mod memory_migrate;
mod purge_fixtures;

// EPIC cas-7d31: the daemon's auto-distill path needs the same complete symbol
// load the CLI does — a narrower source set would cascade-delete module pages.
pub use knowledge_cmd::{
    DEFAULT_MAX_SYMBOLS as KNOWLEDGE_MAX_SYMBOLS, SymbolLoad,
    load_symbols as knowledge_symbols_with_limit,
};
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
// cas-fc6fa: read-only cross-project contamination scan used by `cas doctor`.
pub mod foreign_rows;
// cas-9e81: pub(crate) so factory_ops can reuse `config_gen`'s canonical
// known-Claude-config-dir list instead of re-deriving `~/.claude` by hand.
pub(crate) mod hook;
mod init;
pub mod integrate;
pub mod interactive;
mod limits;
mod list;
mod mcp_cmd;
pub mod memory;
mod open;
mod queue;
pub mod retrieval_parity;
mod status;
mod statusline;
mod sync;
mod update;
pub mod update_transaction;
mod viktor;

use std::path::{Path, PathBuf};

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

use crate::store::find_cas_root;

pub use auth::AuthCommands;
pub use bridge::BridgeArgs;
pub use changelog::ChangelogArgs;
pub use claude::ClaudeArgs;
pub use codex::CodexArgs;
pub use claude_md::ClaudeMdArgs;
pub use config::ConfigCommands;
pub use doctor::DoctorArgs;
pub use factory::{AttachArgs, FactoryArgs, KillAllArgs, KillArgs};
pub use hook::HookArgs;
pub use hub::HubArgs;
pub use init::InitArgs;
pub use limits::LimitsArgs;
pub use list::ListArgs;
pub use mcp_cmd::McpCommands;
pub use open::OpenArgs;
pub use provider_default::DefaultArgs;
pub use status::StatusArgs;
pub use statusline::StatusLineArgs;
pub use sync::SyncCommands;
pub use update::UpdateArgs;
pub use viktor::ViktorArgs;

/// Build version string including git hash and date
fn build_version() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let git_hash = option_env!("CAS_GIT_HASH").unwrap_or("unknown");
    let build_date = option_env!("CAS_BUILD_DATE").unwrap_or("unknown");
    format!("{version} ({git_hash} {build_date})")
}

/// Compact Doom-style Cassy wordmark. Its widest row is 53 cells, so it fits
/// comfortably in an 80-column terminal without wrapping.
const CASSY_WORDMARK: &str = r#"
 ██████╗ █████╗ ███████╗███████╗██╗   ██╗
██╔════╝██╔══██╗██╔════╝██╔════╝╚██╗ ██╔╝
██║     ███████║███████╗███████╗ ╚████╔╝
██║     ██╔══██║╚════██║╚════██║  ╚██╔╝
╚██████╗██║  ██║███████║███████║   ██║
 ╚═════╝╚═╝  ╚═╝╚══════╝╚══════╝   ╚═╝
"#;

const CASSY_PLAIN_WORDMARK: &str = "Cassy";

/// Select the wordmark that is safe for the destination before clap renders
/// help. Piped output and `NO_COLOR` deliberately get a compact text fallback.
pub fn help_wordmark() -> &'static str {
    match crate::ui::components::OutputMode::detect() {
        crate::ui::components::OutputMode::Styled => CASSY_WORDMARK,
        crate::ui::components::OutputMode::Plain => CASSY_PLAIN_WORDMARK,
    }
}

/// Parse with a destination-aware top-level help banner.
pub fn try_parse_from_with_wordmark<I, T>(args: I) -> Result<Cli, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let matches = Cli::command()
        .before_help(help_wordmark())
        .try_get_matches_from(args)?;
    Cli::from_arg_matches(&matches)
}

/// Cassy - Multi-agent coding factory
#[derive(Parser)]
#[command(name = "cas")]
#[command(about = "Cassy — a multi-agent coding factory with persistent memory and task coordination")]
#[command(version = build_version())]
#[command(before_help = CASSY_WORDMARK)]
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

    /// Initialize Cassy in current directory
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

    /// Launch factory with Claude as the supervisor on a chosen account profile
    ///
    /// `cas claude alt` runs Cassy supervised by Claude, signed in as the account
    /// in ~/.claude-alt; `main` uses ~/.claude. Spawned workers inherit the same
    /// account. All `cas factory` flags pass through. Use `--list-profiles` to
    /// see detected accounts, or `--bare` to open plain Claude Code instead.
    Claude(ClaudeArgs),

    /// Launch factory with Codex as the supervisor on a chosen account profile
    ///
    /// `cas codex alt` runs Cassy supervised by Codex, signed in as the account in
    /// ~/.codex-alt; `main` uses ~/.codex. Spawned codex workers inherit the same
    /// account through CODEX_HOME. All `cas factory` flags pass through. Use
    /// `--list-profiles` to see detected accounts, or `--bare` to open plain
    /// Codex instead.
    Codex(CodexArgs),

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

    /// Stable machine-local Commander hub
    Hub(HubArgs),

    /// Run the CAS MCP server
    Serve,

    /// Run diagnostics
    Doctor(DoctorArgs),

    /// Show credential-safe provisioning status for the managed Viktor gateway
    Viktor(ViktorArgs),

    /// Manage configuration
    #[command(subcommand)]
    Config(ConfigCommands),

    /// Show session status
    Status(StatusArgs),

    /// Show local provider rate-limit and credit availability
    Limits(LimitsArgs),

    /// Output a status line for agent integrations
    #[command(alias = "statusline")]
    StatusLine(StatusLineArgs),

    /// Handle Claude Code hook events
    Hook(HookArgs),

    /// Authentication commands (login, logout, whoami)
    #[command(subcommand)]
    Auth(AuthCommands),

    /// Log in to Cassy Cloud (shortcut for 'auth login')
    Login(auth::LoginArgs),

    /// Log out (shortcut for 'auth logout')
    Logout,

    /// Show current user (shortcut for 'auth whoami')
    Whoami,

    /// Update Cassy to the latest version
    Update(UpdateArgs),

    /// Show release notes and changelog from GitHub releases
    Changelog(ChangelogArgs),

    /// Manage upstream MCP servers
    #[command(subcommand)]
    Mcp(McpCommands),

    /// Prompt queue operations (poll/ack for native extensions)
    #[command(subcommand)]
    Queue(queue::QueueCommands),

    /// Sync data with Cassy Cloud
    #[command(subcommand)]
    Cloud(cloud::CloudCommands),

    /// Manage registered devices
    #[command(subcommand)]
    Device(device::DeviceCommands),

    /// Synchronize generated project files
    #[command(subcommand)]
    Sync(SyncCommands),

    /// Evaluate and optimize CLAUDE.md files for token efficiency
    #[command(name = "claude-md")]
    ClaudeMd(ClaudeMdArgs),

    /// Codemap staleness info and pending changes
    #[command(subcommand)]
    Codemap(codemap_cmd::CodemapCommands),

    /// Structural git-history index (backfill/status)
    #[command(subcommand)]
    History(history_cmd::HistoryCommands),

    /// Build local search indexes on demand (`cas index code`)
    #[command(subcommand)]
    Index(index_cmd::IndexCommands),

    /// Distilled project knowledge wiki (build/status/list)
    #[command(subcommand)]
    Knowledge(knowledge_cmd::KnowledgeCommands),

    /// Migrate the legacy memory store into knowledge pages
    #[command(name = "memory-migrate")]
    MemoryMigrate(memory_migrate::MemoryMigrateArgs),

    /// Delete integration-test fixture memories that leaked into real stores
    #[command(name = "purge-test-fixtures", hide = true)]
    PurgeTestFixtures(purge_fixtures::PurgeFixturesArgs),

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

    /// Capture and replay memory-retrieval baselines
    #[command(name = "retrieval-parity", subcommand, hide = true)]
    RetrievalParity(retrieval_parity::RetrievalParityCommands),
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
        | Commands::Viktor(_)
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
        | Commands::Hub(_)
        | Commands::Config(_)
        | Commands::Status(_)
        | Commands::Limits(_)
        | Commands::StatusLine(_)
        | Commands::Mcp(_)
        | Commands::Queue(_)
        | Commands::ClaudeMd(_)
        | Commands::Codemap(_)
        | Commands::History(_)
        | Commands::Index(_)
        | Commands::Knowledge(_)
        | Commands::MemoryMigrate(_)
        | Commands::PurgeTestFixtures(_)
        | Commands::ProjectOverview(_)
        | Commands::Integrate(_)
        | Commands::Memory(_)
        | Commands::KnownRepos(_)
        | Commands::Worktree(_)
        | Commands::SweepAll(_)
        | Commands::Sync(_)
        | Commands::RetrievalParity(_) => AuthRequirement::NotRequired,

        Commands::Serve => AuthRequirement::NotRequired,

        Commands::Cloud(_) => AuthRequirement::Required,

        Commands::Device(_) => AuthRequirement::Required,
    }
}

/// Ensure the user is authenticated before running a command
fn ensure_authenticated() -> anyhow::Result<()> {
    {
        // `load_effective`: the login is machine-wide (cas-046d), so this gate
        // must not report "not logged in" merely because the current directory
        // is not a Cassy project.
        let config = crate::cloud::CloudConfig::load_effective();
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
            | Some(Commands::Claude(_))
            | None
    ) {
        let early_cas_root = find_cas_root().ok();
        factory::apply_resource_contention_env(early_cas_root.as_deref());
    }

    // Select the Claude account for `cas claude <profile>` here, for the same
    // reason: `set_var` must land while the process is still single-threaded.
    // The factory supervisor pane and every spawned worker inherit it.
    if let Some(Commands::Claude(claude_args)) = &cli.command {
        claude::apply_profile_env(claude_args)?;
    }
    // Same for `cas codex <profile>`: CODEX_HOME must be exported while the
    // process is single-threaded, so the supervisor pane and every codex worker
    // inherit the selected ChatGPT account (cas-9cc3).
    if let Some(Commands::Codex(codex_args)) = &cli.command {
        codex::apply_profile_env(codex_args)?;
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
        Commands::Hub(_) => "hub".to_string(),
        Commands::Serve => "serve".to_string(),
        Commands::Doctor(_) => "doctor".to_string(),
        Commands::Viktor(_) => "viktor".to_string(),
        Commands::Config(_) => "config".to_string(),
        Commands::Status(_) => "status".to_string(),
        Commands::Limits(_) => "limits".to_string(),
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
        Commands::Sync(_) => "sync".to_string(),
        Commands::ClaudeMd(_) => "claude-md".to_string(),
        Commands::Codemap(_) => "codemap".to_string(),
        Commands::History(_) => "history".to_string(),
        Commands::Index(_) => "index".to_string(),
        Commands::Knowledge(_) => "knowledge".to_string(),
        Commands::MemoryMigrate(_) => "memory-migrate".to_string(),
        Commands::PurgeTestFixtures(_) => "purge-test-fixtures".to_string(),
        Commands::ProjectOverview(_) => "project-overview".to_string(),
        Commands::Integrate(_) => "integrate".to_string(),
        Commands::Memory(_) => "memory".to_string(),
        Commands::KnownRepos(_) => "known-repos".to_string(),
        Commands::Worktree(_) => "worktree".to_string(),
        Commands::SweepAll(_) => "sweep-all".to_string(),
        Commands::RetrievalParity(_) => "retrieval-parity".to_string(),
    }
}

fn require_cas_root(cas_root: Option<&Path>) -> anyhow::Result<&Path> {
    cas_root.ok_or_else(|| {
        anyhow::anyhow!(
            "Cassy not initialized. Run 'cas init' first or navigate to a directory with .cas/"
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
        Commands::Claude(args) => claude::execute(args, cli, cas_root),
        Commands::Codex(args) => codex::execute(args, cli, cas_root),
        Commands::Grok(args) => {
            let mut a = args.clone();
            a.supervisor_cli = "grok".to_string();
            a.supervisor_cli_explicit = true;
            factory::execute(&a, cli, cas_root)
        }
        Commands::Default(args) => provider_default::execute(args),
        Commands::Bridge(args) => bridge::execute(args, cli),
        Commands::Hub(args) => hub::execute(args, cli),
        Commands::Serve => serve_execute(),
        Commands::Doctor(args) => doctor::execute(args, cli, cas_root),
        Commands::Viktor(args) => viktor::execute(args, cli, cas_root),
        Commands::Config(cmd) => config::execute_subcommand(cmd, cli, require_cas_root(cas_root)?),
        Commands::Status(args) => status::execute(args, cli, require_cas_root(cas_root)?),
        Commands::Limits(args) => limits::execute(args, cli),
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
        Commands::Sync(cmd) => sync::execute(cmd, cli),
        Commands::ClaudeMd(args) => claude_md::execute(args, cli),
        Commands::Codemap(cmd) => codemap_cmd::execute(cmd, cli, require_cas_root(cas_root)?),
        Commands::History(cmd) => history_cmd::execute(cmd, cli, require_cas_root(cas_root)?),
        Commands::Index(cmd) => index_cmd::execute(cmd, cli, require_cas_root(cas_root)?),
        Commands::Knowledge(cmd) => knowledge_cmd::execute(cmd, cli, require_cas_root(cas_root)?),
        Commands::MemoryMigrate(args) => memory_migrate::execute(args, require_cas_root(cas_root)?),
        Commands::PurgeTestFixtures(args) => {
            purge_fixtures::execute(args, require_cas_root(cas_root)?)
        }
        Commands::ProjectOverview(cmd) => {
            project_overview_cmd::execute(cmd, cli, require_cas_root(cas_root)?)
        }
        Commands::Integrate(cmd) => integrate::execute(cmd, cli),
        Commands::Memory(cmd) => memory::execute(cmd, cli, require_cas_root(cas_root)?),
        Commands::KnownRepos(cmd) => known_repos::execute(cmd),
        Commands::Worktree(cmd) => worktree::execute(cmd),
        Commands::SweepAll(args) => sweep::execute_sweep_all(args),
        Commands::RetrievalParity(cmd) => retrieval_parity::execute(cmd, cas_root),
    }
}

fn serve_execute() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(crate::mcp::run_server())
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{CASSY_WORDMARK, Cli};

    #[test]
    fn top_level_help_exposes_cloud_without_internal_migration_language() {
        let mut command = Cli::command();
        let mut help = Vec::new();
        command.write_long_help(&mut help).expect("render help");
        let help = String::from_utf8(help).expect("help is UTF-8");

        assert!(help.contains("cloud"));
        assert!(help.contains("Sync data with Cassy Cloud"));
        assert!(help.contains("memory-migrate"));
        assert!(!help.contains("purge-test-fixtures"));
        assert!(!help.contains("retrieval-parity"));
        assert!(!help.contains("EPIC cas-"));
    }

    #[test]
    fn cassy_wordmark_stays_compact_for_80_column_terminals() {
        let rows: Vec<_> = CASSY_WORDMARK
            .lines()
            .filter(|row| !row.is_empty())
            .collect();
        assert_eq!(rows.len(), 6, "the splash must remain at most eight rows");
        assert!(rows.iter().all(|row| row.chars().count() <= 80));
    }

    #[test]
    fn cloud_help_describes_the_available_sync_command() {
        let parsed = Cli::try_parse_from(["cas", "cloud", "sync", "--help"]);
        let Err(error) = parsed else {
            panic!("--help exits through clap");
        };
        let help = error.to_string();

        assert!(help.contains("Full sync (push then pull)"));
    }
}
