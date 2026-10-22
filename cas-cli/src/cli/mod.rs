//! CLI commands for Cassy
//!
//! Essential commands only. Use MCP tools for memory, tasks, rules, etc.

pub(crate) mod account_picker;
mod auth;
pub(crate) mod bridge;
mod changelog;
pub(crate) mod claude;
mod claude_md;
pub(crate) mod codex;
mod codemap_cmd;
mod history_cmd;
mod artifact_cmd;
pub use artifact_cmd::render_artifact_line;
pub(crate) mod hub;
mod hub_operator;
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
mod peers;
mod project_overview_cmd;
mod qa;
mod qa_impact;
pub(crate) mod qa_post_release;
mod provider_default;
mod release_report;
mod sweep;
mod worktree;
// `pub` so integration tests in `cas-cli/tests/` can reach
// `cli::cloud::execute_team_push` (cas-1f44 T4). Internal; no stable API.
pub mod cloud;
mod config;
mod config_tui;
mod coordination;
mod device;
mod doctor;
// cas-728b: pub(crate) so ui::factory::director::events can reach
// cli::factory::wedged's transcript-mtime liveness primitives.
pub(crate) mod factory;
mod factory_tooling;
// cas-c741: the one friendly line a brand-new machine gets from bare `cas`.
pub mod first_run;
// cas-fc6fa: read-only cross-project contamination scan used by `cas doctor`.
pub mod foreign_rows;
// cas-9e81: pub(crate) so factory_ops can reuse `config_gen`'s canonical
// known-Claude-config-dir list instead of re-deriving `~/.claude` by hand.
pub(crate) mod hook;
pub use hook::{execute_native_pre_tool_use, is_native_codex_pre_tool};
mod init;
pub(crate) mod project_files;
pub mod integrate;
pub mod interactive;
mod limits;
pub(crate) mod accounts;
mod list;
pub(crate) mod mcp_cmd;
pub mod memory;
mod open;
mod queue;
pub mod retrieval_parity;
mod setup;
mod status;
mod statusline;
pub mod store_choice;
mod sync;
mod update;
pub mod update_transaction;
pub(crate) mod viktor;
mod violet_cmd;

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
pub use release_report::ReleaseReportArgs;
pub use setup::SetupArgs;
pub use status::StatusArgs;
pub use statusline::StatusLineArgs;
pub use sync::SyncCommands;
pub use update::{UpdateArgs, startup_config_notices};
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
/// help. Piped output, `NO_COLOR` and non-UTF-8 locales get a compact text fallback.
pub fn help_wordmark() -> &'static str {
    help_wordmark_for_mode(crate::ui::components::OutputMode::detect())
}

fn help_wordmark_for_mode(mode: crate::ui::components::OutputMode) -> &'static str {
    if supports_styled_glyphs(mode) {
        CASSY_WORDMARK
    } else {
        CASSY_PLAIN_WORDMARK
    }
}

fn supports_styled_glyphs(mode: crate::ui::components::OutputMode) -> bool {
    mode == crate::ui::components::OutputMode::Styled && crate::ui::components::locale_is_utf8()
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

/// Where `cas hub serve` writes its tracing log (cas-093a).
///
/// The machine hub is host-level: one per machine, whatever directory it is
/// started from. `cas hub start` and `restart` launch it detached from the
/// caller's working directory, and the service unit runs it from the home
/// directory. Resolved like any command, its tracing log landed in whichever
/// project's `.cas/logs` was nearest the caller. For example, the 2026-09-26
/// hub logged into `~/Documents/Accounting/.cas/logs`, where nobody looking at
/// `~/.cas/hub` would find it.
///
/// For `cas hub serve` this returns the hub's own directory (`~/.cas/hub`,
/// created private first), so tracing goes to `~/.cas/hub/logs/` beside
/// `hub.log` and `audit.jsonl`. Every other command gets `None` and keeps its
/// project log.
pub fn host_tracing_root(cli: &Cli) -> Option<std::path::PathBuf> {
    let home = dirs::home_dir()?;
    let root = hub_tracing_root(cli.command.as_ref(), &home)?;
    // The hub directory is private (0700). Create it that way before the log
    // writer makes anything beneath it; the hub's own startup re-checks it.
    let _ = crate::hub::ensure_private_dir(&root);
    Some(root)
}

fn hub_tracing_root(
    command: Option<&Commands>,
    home: &std::path::Path,
) -> Option<std::path::PathBuf> {
    match command {
        Some(Commands::Hub(hub::HubArgs {
            command: Some(hub::HubCommands::Serve(_)),
            ..
        })) => Some(
            crate::hub::HubRuntimePaths::for_home(home)
                .root()
                .to_path_buf(),
        ),
        _ => None,
    }
}

/// Cassy - Multi-agent coding factory
#[derive(Parser)]
#[command(name = "cas")]
#[command(about = "Cassy - a multi-agent coding factory with persistent memory and task coordination")]
#[command(version = build_version())]
#[command(before_help = CASSY_PLAIN_WORDMARK)]
pub struct Cli {
    /// Output in JSON format
    #[arg(long, global = true)]
    pub json: bool,

    /// Show complete JSON, service paths/definitions, or GC report details
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
    /// Interactive project picker: scan ~/projects/, select, launch or attach
    Open(OpenArgs),

    /// Initialize Cassy in current directory
    Init(InitArgs),

    /// Guide a complete machine through installation, login, pairing, and a first project
    Setup(SetupArgs),

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
    /// `cas default codex` persists `[llm.supervisor] harness = "codex"` to
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

    /// Post local files and messages to Slack through the Violet hub, or read a channel
    Violet(violet_cmd::VioletArgs),

    /// Manage configuration
    #[command(subcommand)]
    Config(ConfigCommands),

    /// Show session status
    Status(StatusArgs),

    /// Show this repository's peer supervisors through Cassy Cloud
    Peers(peers::PeersArgs),

    /// Claim, transfer or reconcile ownership of a Slack thread
    #[command(subcommand)]
    Coordination(coordination::ExternalWorkCommands),

    /// Show local provider rate-limit and credit availability
    Limits(LimitsArgs),

    /// Share one base folder across every Claude and Codex account
    Accounts(accounts::AccountsArgs),

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

    /// Release lifecycle helpers
    #[command(subcommand)]
    Release(ReleaseCommands),

    /// Legacy spelling for `cas release report`
    #[command(name = "release-report", hide = true)]
    ReleaseReport(ReleaseReportArgs),

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

    /// Project QA registry and checks
    #[command(subcommand)]
    Qa(qa::QaCommand),

    /// Structural git-history index (backfill/status)
    #[command(subcommand)]
    History(history_cmd::HistoryCommands),

    /// Build local search indexes on demand (`cas index code`)
    #[command(subcommand)]
    Index(index_cmd::IndexCommands),

    /// Publish and inspect durable task artifacts (publish/show/list)
    #[command(subcommand)]
    Artifact(artifact_cmd::ArtifactCommands),

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

#[derive(Subcommand)]
pub enum ReleaseCommands {
    /// Assemble a Markdown, standalone HTML, and optional PDF release report
    Report(ReleaseReportArgs),
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
        | Commands::Setup(_)
        | Commands::Open(_)
        | Commands::Doctor(_)
        | Commands::Viktor(_)
        | Commands::Violet(_)
        | Commands::Update(_)
        | Commands::Changelog(_)
        | Commands::Release(_)
        | Commands::ReleaseReport(_)
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
        | Commands::Peers(_)
        | Commands::Coordination(_)
        | Commands::Limits(_)
        | Commands::Accounts(_)
        | Commands::StatusLine(_)
        | Commands::Mcp(_)
        | Commands::Queue(_)
        | Commands::ClaudeMd(_)
        | Commands::Qa(_)
        | Commands::Codemap(_)
        | Commands::History(_)
        | Commands::Index(_)
        | Commands::Artifact(_)
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

        // Local unlink only removes the project link file and is useful when
        // taking a project offline; remote purge remains auth-gated.
        Commands::Cloud(cloud::CloudCommands::Unlink(args)) if !args.purge_remote => {
            AuthRequirement::NotRequired
        }
        Commands::Cloud(cloud::CloudCommands::Project(args))
            if args.adopt_aliases
                || matches!(
                    args.command.as_ref(),
                    Some(cloud::CloudProjectCommands::AdoptAliases)
                ) =>
        {
            AuthRequirement::NotRequired
        }
        Commands::Cloud(cloud::CloudCommands::Queue(args))
            if matches!(
                args.command.as_ref(),
                Some(cloud::CloudQueueCommand::MigrateToTeam { apply: false })
            ) =>
        {
            AuthRequirement::NotRequired
        }
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

/// A failure raised after the command already printed its `--json` report.
/// The shared error path keeps it off stdout so callers still parse one document.
#[derive(Debug)]
pub struct JsonReportedFailure(pub anyhow::Error);

impl std::fmt::Display for JsonReportedFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#}", self.0)
    }
}

impl std::error::Error for JsonReportedFailure {}

/// Format the machine-readable message and human stderr diagnostic alike.
pub fn json_error_message(error: &anyhow::Error) -> String {
    error
        .downcast_ref::<crate::error::CasError>()
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("{error:#}"))
}

/// Render the diagnostic for a person while leaving [`json_error_message`]
/// as the stable machine-readable representation.
pub fn human_error_message(error: &anyhow::Error) -> String {
    let message = format!("{error:#}");
    let Some(detail) = message
        .strip_prefix("refused: ")
        .filter(|_| message.contains("operator-only because"))
    else {
        return message;
    };

    let detail = if let Some((reason, remedy)) = detail.split_once("; run `") {
        if let Some((command, context)) = remedy.split_once('`') {
            let context = context
                .strip_prefix(" yourself from your own terminal.")
                .unwrap_or(context)
                .trim_start();
            format!(
                "  {}\n  Run this command yourself from your own terminal:\n  {command}\n  {}",
                crate::cloud::presence::wrap_line(&format!("{reason}.")).trim_end(),
                crate::cloud::presence::wrap_line(context).trim_end()
            )
        } else {
            format!("  {}", crate::cloud::presence::wrap_line(detail).trim_end())
        }
    } else {
        format!("  {}", crate::cloud::presence::wrap_line(detail).trim_end())
    };
    format!("Operator-only config change refused.\n{detail}")
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

    // Machine-scoped config follows HOME and does not require an initialized
    // project. Project telemetry/tracing retain their own discovery paths.
    let cas_root: Option<PathBuf> = match &cli.command {
        Some(Commands::Config(cmd)) => config::host_root_for_command(cmd)
            .or_else(|| find_cas_root().ok()),
        _ => find_cas_root().ok(),
    };

    // cas-e1c7: a command that would rewrite another store's identity or
    // config is refused before anything else runs (sign-in included).
    if let (Some(guard), Some(root)) = (command_store_guard(&cli.command), cas_root.as_deref()) {
        guarded_write_root(Some(guard), root)?;
    }

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
        Commands::Setup(_) => "setup".to_string(),
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
        Commands::Violet(_) => "violet".to_string(),
        Commands::Config(_) => "config".to_string(),
        Commands::Status(_) => "status".to_string(),
        Commands::Peers(_) => "peers".to_string(),
        Commands::Coordination(_) => "coordination".to_string(),
        Commands::Limits(_) => "limits".to_string(),
        Commands::Accounts(_) => "accounts".to_string(),
        Commands::StatusLine(_) => "statusline".to_string(),
        Commands::Hook(_) => "hook".to_string(),
        Commands::Auth(_) => "auth".to_string(),
        Commands::Login(_) => "login".to_string(),
        Commands::Logout => "logout".to_string(),
        Commands::Whoami => "whoami".to_string(),
        Commands::Update(_) => "update".to_string(),
        Commands::Changelog(_) => "changelog".to_string(),
        Commands::Release(_) => "release".to_string(),
        Commands::ReleaseReport(_) => "release-report".to_string(),
        Commands::Mcp(_) => "mcp".to_string(),
        Commands::Queue(_) => "queue".to_string(),
        Commands::Cloud(_) => "cloud".to_string(),
        Commands::Device(_) => "device".to_string(),
        Commands::Sync(_) => "sync".to_string(),
        Commands::ClaudeMd(_) => "claude-md".to_string(),
        Commands::Qa(_) => "qa".to_string(),
        Commands::Codemap(_) => "codemap".to_string(),
        Commands::History(_) => "history".to_string(),
        Commands::Index(_) => "index".to_string(),
        Commands::Artifact(_) => "artifact".to_string(),
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

/// cas-e1c7: a command that rewrites project identity or config, with its
/// `--store` choice. Such a command refuses, before writing, when CAS_ROOT and
/// the working directory are different stores and `--store` does not pick one.
type StoreGuard = Option<(&'static str, Option<store_choice::StoreChoice>)>;

fn config_store_guard(cmd: &config::ConfigCommands) -> StoreGuard {
    // Host policy ignores project-store selectors, just as the hub daemon does.
    if config::host_root_for_command(cmd).is_some() {
        return None;
    }
    match cmd {
        config::ConfigCommands::Set(args) => Some(("cas config set", args.store.store)),
        config::ConfigCommands::Reset(args) => Some(("cas config reset", args.store.store)),
        config::ConfigCommands::Import(args) if !args.dry_run => Some(("cas config import", args.store.store)),
        _ => None,
    }
}

fn cloud_store_guard(cmd: &cloud::CloudCommands) -> StoreGuard {
    match cmd {
        cloud::CloudCommands::Team(cloud::CloudTeamCommands::Set(args)) => {
            Some(("cas cloud team set", args.store.store))
        }
        cloud::CloudCommands::Project(cloud::CloudProjectArgs {
            command: Some(cloud::CloudProjectCommands::Set(args)),
            ..
        }) => Some(("cas cloud project set", args.store.store)),
        _ => None,
    }
}

fn command_store_guard(command: &Option<Commands>) -> StoreGuard {
    match command {
        Some(Commands::Config(cmd)) => config_store_guard(cmd),
        Some(Commands::Cloud(cmd)) => cloud_store_guard(cmd),
        _ => None,
    }
}

fn guarded_write_root(guard: StoreGuard, resolved: &Path) -> anyhow::Result<PathBuf> {
    match guard {
        Some((command, choice)) => store_choice::write_root(
            command,
            resolved,
            choice,
            crate::store::detect::root_conflict(),
        ),
        None => Ok(resolved.to_path_buf()),
    }
}

fn require_cas_root(cas_root: Option<&Path>) -> anyhow::Result<&Path> {
    cas_root.ok_or_else(|| anyhow::anyhow!("no Cassy store here; run `cas init`"))
}

fn run_command(cli: &Cli, cas_root: Option<&Path>) -> anyhow::Result<()> {
    let command = match &cli.command {
        Some(cmd) => cmd,
        None => {
            // cas-c741: someone who just ran the install one-liner and typed
            // `cas` gets one line naming the next command, not the factory
            // preflight's list of everything a fresh machine is missing.
            if first_run::machine_is_unconfigured(
                &crate::store::known_repos::host_cas_dir(),
                cas_root,
            ) {
                println!("{}", first_run::front_door_hint());
                return Ok(());
            }
            let default_args = FactoryArgs::default();
            return factory::execute(&default_args, cli, cas_root);
        }
    };

    match command {
        Commands::Open(args) => open::execute(args),
        Commands::Init(args) => init::execute(args, cli),
        Commands::Setup(args) => setup::execute(args, cli),
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
        Commands::Violet(args) => violet_cmd::execute(args, cli, cas_root),
        Commands::Config(cmd) => {
            let root = guarded_write_root(config_store_guard(cmd), require_cas_root(cas_root)?)?;
            config::execute_subcommand(cmd, cli, &root)
        }
        Commands::Status(args) => status::execute(args, cli, require_cas_root(cas_root)?),
        Commands::Peers(args) => peers::execute(args, cli, require_cas_root(cas_root)?),
        Commands::Coordination(command) => coordination::execute(command, cli, require_cas_root(cas_root)?),
        Commands::Limits(args) => limits::execute(args, cli),
        Commands::Accounts(args) => accounts::execute(args, cli),
        // cas-e1f4: outside a Cassy project the line still names the account.
        Commands::StatusLine(args) => match cas_root {
            Some(root) => statusline::execute(args, cli, root),
            None => statusline::execute_account_only(args),
        },
        Commands::Hook(args) => hook::execute(args, cli),
        Commands::Auth(cmd) => auth::execute(cmd, cli),
        Commands::Login(args) => auth::execute(&AuthCommands::Login(args.clone()), cli),
        Commands::Logout => auth::execute(&AuthCommands::Logout, cli),
        Commands::Whoami => auth::execute(&AuthCommands::Whoami, cli),
        Commands::Update(args) => update::execute(args, cli, cas_root),
        Commands::Changelog(args) => changelog::execute(args, cli),
        Commands::Release(ReleaseCommands::Report(args)) => release_report::execute(args, cli),
        Commands::ReleaseReport(args) => release_report::execute(args, cli),
        Commands::Mcp(cmd) => mcp_cmd::execute(cmd, cli, require_cas_root(cas_root)?),
        Commands::Queue(cmd) => queue::execute(cmd, cli),
        Commands::Cloud(cmd) => {
            let root = guarded_write_root(cloud_store_guard(cmd), require_cas_root(cas_root)?)?;
            cloud::execute(cmd, cli, &root)
        }
        Commands::Device(cmd) => device::execute(cmd, cli),
        Commands::Sync(cmd) => sync::execute(cmd, cli),
        Commands::ClaudeMd(args) => claude_md::execute(args, cli),
        Commands::Qa(cmd) => qa::execute(cmd, cli, cas_root),
        Commands::Codemap(cmd) => codemap_cmd::execute(cmd, cli, require_cas_root(cas_root)?),
        Commands::History(cmd) => history_cmd::execute(cmd, cli, require_cas_root(cas_root)?),
        Commands::Index(cmd) => index_cmd::execute(cmd, cli, require_cas_root(cas_root)?),
        Commands::Artifact(cmd) => artifact_cmd::execute(cmd, cli, require_cas_root(cas_root)?),
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
        Commands::KnownRepos(cmd) => known_repos::execute(cmd, cli.json),
        Commands::Worktree(cmd) => worktree::execute(cmd),
        Commands::SweepAll(args) => sweep::execute_sweep_all(args),
        Commands::RetrievalParity(cmd) => retrieval_parity::execute(cmd, cas_root),
    }
}

fn serve_execute() -> anyhow::Result<()> {
    // Long-lived: a proxy client that vanishes mid-write on the MCP socket
    // must not kill the server every harness session depends on (cas-5918).
    // Without the harness, stdout writes now fail with EPIPE and the service
    // ends through its normal shutdown path.
    crate::server_signals::ignore_sigpipe_for_server();
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    crate::mcp::configure_serve_allocator();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(crate::mcp::run_server());
    if result.is_err() {
        // cas-1895: the pre-initialize guard (cas-2a49) reads stdin from the
        // first moment of startup. Tokio's stdin is a blocking read that
        // cannot be cancelled, and dropping the runtime waits for it, so a
        // failed startup hung until the client wrote or closed stdin instead
        // of exiting with its diagnostic (cas-5c05). Give other blocking work
        // a short grace, then abandon the read.
        runtime.shutdown_timeout(SERVE_FAILED_STARTUP_GRACE);
    }
    result
}

/// How long a failed `cas serve` startup waits for blocking tasks before exit.
const SERVE_FAILED_STARTUP_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{CASSY_WORDMARK, Cli, help_wordmark_for_mode, hub_tracing_root, run};

    #[test]
    fn operator_config_refusal_has_a_wrapped_human_form() {
        let error = anyhow::anyhow!(
            "refused: CLAUDECODE is set, so this runs inside an agent session. verification.force_bypass_allowed is operator-only because it relaxes a Cassy guard; run `cas config set verification.force_bypass_allowed <value>` yourself from your own terminal. If this is your own terminal, it inherited CLAUDECODE from an agent pane: open a fresh terminal and run it there."
        );
        let rendered = super::human_error_message(&error);

        assert_eq!(
            rendered,
            "Operator-only config change refused.\n  CLAUDECODE is set, so this runs inside an agent session.\n  verification.force_bypass_allowed is operator-only because it relaxes\n  a Cassy guard.\n  Run this command yourself from your own terminal:\n  cas config set verification.force_bypass_allowed <value>\n  If this is your own terminal, it inherited CLAUDECODE from an agent\n  pane: open a fresh terminal and run it there."
        );
        assert!(rendered.lines().all(|line| line.chars().count() <= 80), "{rendered}");
        assert_eq!(
            super::json_error_message(&error),
            error.to_string(),
            "machine output keeps the original one-line refusal"
        );
    }

    /// cas-093a: `cas hub serve` traces into the host hub directory
    /// (`<home>/.cas/hub`, so `<home>/.cas/hub/logs/`), however it was
    /// launched and whatever the working directory was. Every other command,
    /// including the other hub verbs, keeps its project log.
    #[test]
    fn hub_serve_traces_into_the_host_hub_directory() {
        let home = std::path::Path::new("/home/operator");
        let root_for = |args: &[&str]| {
            let cli = Cli::try_parse_from(args).unwrap();
            hub_tracing_root(cli.command.as_ref(), home)
        };
        let expected = Some(home.join(".cas").join("hub"));
        assert_eq!(root_for(&["cas", "hub", "serve"]), expected);
        // The detached launcher's form and the service unit's form.
        assert_eq!(
            root_for(&["cas", "hub", "serve", "--launched-by", "start"]),
            expected
        );
        assert_eq!(
            root_for(&[
                "cas",
                "hub",
                "--tailscale-serve",
                "serve",
                "--port",
                "18765"
            ]),
            expected
        );
        // The root is the hub's own runtime directory, beside hub.log.
        assert_eq!(
            expected.as_deref().map(|root| root.join("hub.log")),
            Some(crate::hub::HubRuntimePaths::for_home(home).log_path())
        );
        // Short-lived hub verbs and every other command keep the project log.
        assert_eq!(root_for(&["cas", "hub", "status"]), None);
        assert_eq!(root_for(&["cas", "hub", "start"]), None);
        assert_eq!(root_for(&["cas", "doctor"]), None);
        assert_eq!(hub_tracing_root(None, home), None);
    }

    /// The resolution ignores the working directory: a hub started from
    /// inside a project still resolves to the host directory, never that
    /// project's `.cas`.
    #[test]
    fn hub_serve_tracing_root_does_not_follow_the_working_directory() {
        let project = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(project.path().join(".cas")).unwrap();
        let home = tempfile::TempDir::new().unwrap();
        let cli = Cli::try_parse_from(["cas", "hub", "serve"]).unwrap();
        let root = hub_tracing_root(cli.command.as_ref(), home.path()).unwrap();
        assert_eq!(root, home.path().join(".cas").join("hub"));
        assert!(!root.starts_with(project.path()));
    }

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
        // cas-a0f8: authored help stays ASCII when the destination uses the
        // plain banner. The actual C-locale CLI also exercises that selection.
        let help = Cli::command().before_help("Cassy").render_long_help().to_string();
        assert!(help.is_ascii(), "plain global help must be ASCII: {help}");
    }

    #[test]
    fn release_report_help_fits_80_and_120_columns_without_losing_detail() {
        let details = [
            "Directory receiving <version>.md, <version>.html, and (with --pdf) <version>.pdf",
            "Render a PDF with Playwright after rendering the HTML",
            "Re-fetch sources and replace the Markdown source, including a manual source",
            "A standalone `release-latency.receipt` from `scripts/release-latency-receipt.sh <tag> --run-dir <dir>`; its run directory is read alongside ~/.cas/artifacts/release",
        ];

        for width in [80, 120] {
            let mut command = Cli::command();
            let mut report = command
                .find_subcommand_mut("release")
                .expect("release subcommand")
                .find_subcommand_mut("report")
                .expect("report subcommand")
                .clone()
                .term_width(width);
            let mut help = Vec::new();
            report.write_long_help(&mut help).expect("render report help");
            let help = String::from_utf8(help).expect("help is UTF-8");

            for line in help.lines() {
                assert!(
                    line.chars().count() <= width,
                    "{width}-column help line is {} cells wide: {line}",
                    line.chars().count()
                );
            }
            let normalized = help.split_whitespace().collect::<Vec<_>>().join(" ");
            for detail in details {
                assert!(normalized.contains(detail), "help omitted `{detail}`: {help}");
            }
        }
    }

    #[test]
    fn cassy_wordmark_stays_compact_for_80_column_terminals() {
        use crate::ui::components::OutputMode;

        let rows: Vec<_> = CASSY_WORDMARK
            .lines()
            .filter(|row| !row.is_empty())
            .collect();
        assert_eq!(rows.len(), 6, "the splash must remain at most eight rows");
        assert!(rows.iter().all(|row| row.chars().count() <= 80));

        let mut env = crate::test_support::TestEnvGuard::new();
        for (locale, expected_styled) in [
            ("C", "Cassy"),
            ("en_US.ISO-8859-1", "Cassy"),
            ("C.UTF-8", CASSY_WORDMARK),
        ] {
            env.set("LC_ALL", locale);
            let wordmark = help_wordmark_for_mode(OutputMode::Styled);
            assert_eq!(wordmark, expected_styled, "styled root help in {locale}");
            assert_eq!(help_wordmark_for_mode(OutputMode::Plain), "Cassy");
            let help = Cli::command().before_help(wordmark).render_long_help().to_string();
            assert!(help.contains("Usage:"), "root help retains command entry");
            if expected_styled.is_ascii() {
                assert!(help.is_ascii(), "root help must be ASCII in {locale}: {help}");
                let default_help = Cli::command().render_long_help().to_string();
                assert!(default_help.is_ascii(), "raw clap entry must not bypass the locale fallback: {default_help}");
            }
        }
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

    #[test]
    fn cas_5aeb_migrate_preview_dispatch_does_not_cache_user_credentials() {
        use std::collections::BTreeMap;

        fn snapshot(root: &std::path::Path, base: &std::path::Path, files: &mut BTreeMap<std::path::PathBuf, Vec<u8>>) {
            for entry in std::fs::read_dir(root).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    snapshot(&path, base, files);
                } else {
                    files.insert(path.strip_prefix(base).unwrap().to_path_buf(), std::fs::read(path).unwrap());
                }
            }
        }

        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let project = env.home().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let root = crate::store::init_cas_dir(&project).unwrap();
        env.set("CAS_ROOT", &root);
        env.set_current_dir(&project);
        crate::cloud::set_canonical_id_in_config_toml(&root, "project").unwrap();
        let mut linked = crate::cloud::CloudConfig::default();
        linked.set_team("team-id", "fixture");
        linked.save_to_cas_dir(&root).unwrap();
        let queue = crate::cloud::SyncQueue::open(&root).unwrap();
        queue.init().unwrap();
        drop(queue);

        let user_cloud = env.home().join("machine-cloud.json");
        let user = crate::cloud::CloudConfig {
            token: Some("machine-token".into()),
            ..Default::default()
        };
        user.save_to(&user_cloud).unwrap();
        env.set("CAS_USER_CLOUD_JSON", &user_cloud);

        assert_eq!(crate::cloud::CloudConfig::load_effective().token.as_deref(), Some("machine-token"));
        assert_eq!(
            crate::cloud::CloudConfig::load_from_cas_dir(&root).unwrap().token.as_deref(),
            Some("machine-token"),
            "the fixture confirms auth preflight's cache-write path"
        );
        linked.save_to_cas_dir(&root).unwrap();

        let mut before = BTreeMap::new();
        snapshot(&root, &root, &mut before);
        let cli = Cli::try_parse_from(["cas", "cloud", "queue", "migrate-to-team"]).unwrap();
        run(cli).unwrap();
        let mut after = BTreeMap::new();
        snapshot(&root, &root, &mut after);
        assert_eq!(after.keys().collect::<Vec<_>>(), before.keys().collect::<Vec<_>>(), "preview must not create a project .cas file");
        assert!(after.get(std::path::Path::new("cloud.json")) == before.get(std::path::Path::new("cloud.json")), "preview must not cache machine credentials into project cloud.json");
    }
}
