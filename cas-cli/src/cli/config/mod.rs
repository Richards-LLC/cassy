//! Config command - comprehensive configuration management
//!
//! Provides commands for viewing, modifying, and managing Cassy configuration
//! with rich metadata, validation, and export/import capabilities.

mod docs_ops;
mod edit_ops;
mod io_ops;
pub mod operator_ops;
mod read_ops;
mod util;

pub(super) use read_ops::{config_key_has_override, reset_config_value, save_editor_changes};

use std::path::Path;

use clap::{Parser, Subcommand};

use crate::cli::Cli;

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Get a config value
    Get(ConfigGetArgs),

    /// Set a config value (write roots: `cas config set --help`)
    ///
    /// Keys that relax a Cassy guard (slack.transport, verification.enabled,
    /// verification.force_bypass_allowed, factory.supervisor_only_mcp and
    /// _env, factory.worker_credential_env, qa.evidence_gate,
    /// qa.independent_pass, release.claude_account_allowlist) are
    /// operator-only: they need an interactive terminal outside any agent.
    Set(ConfigSetArgs),

    /// List all config options
    List(ConfigListArgs),

    /// Show detailed information about a config option
    Describe(ConfigDescribeArgs),

    /// Show differences from default configuration
    Diff(ConfigDiffArgs),

    /// Remove a config override to restore its default (or reset all options)
    Reset(ConfigResetArgs),

    /// Export configuration to file
    Export(ConfigExportArgs),

    /// Import configuration from file
    Import(ConfigImportArgs),

    /// Search config options by keyword
    Search(ConfigSearchArgs),

    /// Show full configuration documentation
    Docs(ConfigDocsArgs),

    /// Generate shell completion scripts for config keys
    Completions(ConfigCompletionsArgs),

    /// Interactive config editor (simple line-based)
    Edit(ConfigEditArgs),

    /// Operator only: grant one task write access outside its worktree until it closes
    GrantWrite(operator_ops::ConfigGrantWriteArgs),

    /// Operator only: revoke a task's write grants
    RevokeWrite(operator_ops::ConfigRevokeWriteArgs),
}

#[derive(Parser)]
pub struct ConfigGetArgs {
    /// Config key to get
    pub key: String,
}

#[derive(Parser)]
#[command(
    after_help = concat!(
        "Project write roots (operator only):\n",
        "  cas config set factory.write_roots \"~/Downloads,/mnt/models\"\n",
        "  cas config set factory.write_roots \"\"    # clear\n",
        "\n",
        "Stored in .cas/config.toml as [factory] write_roots. Each root lets this\n",
        "project's supervisor and workers create, edit and delete files beneath it.\n",
        "Paths are canonicalized; /, $HOME and its parents, the project root, and\n",
        "anything containing or inside .cas are refused. Run it in your terminal or\n",
        "as `! cas config set ...` in an agent pane. Agents' own tool calls are\n",
        "refused, and it refuses inside a factory worker cgroup. Guardrail-grade,\n",
        "not security-grade: agents share your Unix user."
    )
)]
pub struct ConfigSetArgs {
    /// Config key to set
    pub key: String,

    /// Value to set
    pub value: String,

    /// cas-e1c7: which store to write when CAS_ROOT and this directory differ.
    #[command(flatten)]
    pub store: crate::cli::store_choice::StoreChoiceArgs,
}

#[derive(Parser)]
pub struct ConfigListArgs {
    /// Show only a specific section
    #[arg(short, long)]
    pub section: Option<String>,

    /// Show all options (including advanced)
    #[arg(short, long)]
    pub all: bool,

    /// Show only modified values
    #[arg(short, long)]
    pub modified: bool,
}

#[derive(Parser)]
pub struct ConfigDescribeArgs {
    /// Config key to describe
    pub key: String,
}

#[derive(Parser)]
pub struct ConfigDiffArgs {
    /// Show all differences (not just non-advanced)
    #[arg(short, long)]
    pub all: bool,
}

#[derive(Parser)]
pub struct ConfigResetArgs {
    /// Config key to reset (or 'all' for everything)
    pub key: String,

    /// Skip confirmation for reset all
    #[arg(short, long)]
    pub force: bool,

    /// cas-e1c7: which store to write when CAS_ROOT and this directory differ.
    #[command(flatten)]
    pub store: crate::cli::store_choice::StoreChoiceArgs,
}

#[derive(Parser)]
pub struct ConfigExportArgs {
    /// Output file (defaults to stdout)
    #[arg(short, long)]
    pub output: Option<String>,

    /// Export format
    #[arg(short, long, default_value = "yaml")]
    pub format: String,
}

#[derive(Parser)]
pub struct ConfigImportArgs {
    /// Input file to import
    pub file: String,

    /// Overwrite existing values
    #[arg(short, long)]
    pub force: bool,

    /// Dry run - show what would change
    #[arg(long)]
    pub dry_run: bool,

    /// cas-e1c7: which store to write when CAS_ROOT and this directory differ.
    #[command(flatten)]
    pub store: crate::cli::store_choice::StoreChoiceArgs,
}

#[derive(Parser)]
pub struct ConfigSearchArgs {
    /// Search query
    pub query: String,
}

#[derive(Parser)]
pub struct ConfigDocsArgs {
    /// Show only a specific section
    #[arg(short, long)]
    pub section: Option<String>,

    /// Output format: text, markdown, or man
    #[arg(short, long, default_value = "text")]
    pub format: String,
}

#[derive(Parser)]
pub struct ConfigCompletionsArgs {
    /// Shell to generate completions for
    #[arg(value_enum)]
    pub shell: CompletionShell,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Parser)]
pub struct ConfigEditArgs {
    /// Edit only a specific section
    #[arg(short, long)]
    pub section: Option<String>,

    /// Edit only modified values
    #[arg(short, long)]
    pub modified: bool,

    /// Include advanced options
    #[arg(short, long)]
    pub all: bool,
}

/// Key-specific host commands do not depend on project discovery or CAS_ROOT.
pub(super) fn host_root_for_command(cmd: &ConfigCommands) -> Option<std::path::PathBuf> {
    let key = match cmd {
        ConfigCommands::Get(args) => args.key.as_str(),
        ConfigCommands::Set(args) => args.key.as_str(),
        ConfigCommands::Reset(args) => args.key.as_str(),
        ConfigCommands::Describe(args) => args.key.as_str(),
        ConfigCommands::List(args) if args.section.as_deref() == Some("hub") => "hub.",
        _ => return None,
    };
    read_ops::host_scoped_key(key).then(crate::config::host_config_dir)
}

/// Execute a config command
///
/// cas_root is resolved once at CLI entry point and passed here.
pub fn execute_subcommand(cmd: &ConfigCommands, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    match cmd {
        // GH #1182: write roots are operator-only. `set` validates every path
        // and migrates legacy cas-3147 roots; `get` also notes legacy roots.
        ConfigCommands::Get(args) if args.key == "factory.write_roots" => {
            operator_ops::execute_get_write_roots(cli, cas_root)
        }
        ConfigCommands::Set(args) if args.key == "factory.write_roots" => {
            operator_ops::execute_set_write_roots(&args.value, cli, cas_root)
        }
        ConfigCommands::Get(args) => crate::cli::config::read_ops::execute_get(args, cli, cas_root),
        ConfigCommands::Set(args) => crate::cli::config::read_ops::execute_set(args, cli, cas_root),
        ConfigCommands::List(args) => {
            crate::cli::config::read_ops::execute_list(args, cli, cas_root)
        }
        ConfigCommands::Describe(args) => {
            crate::cli::config::read_ops::execute_describe(args, cli, cas_root)
        }
        ConfigCommands::Diff(args) => {
            crate::cli::config::read_ops::execute_diff(args, cli, cas_root)
        }
        ConfigCommands::Reset(args) => {
            crate::cli::config::read_ops::execute_reset(args, cli, cas_root)
        }
        ConfigCommands::Export(args) => {
            crate::cli::config::io_ops::execute_export(args, cli, cas_root)
        }
        ConfigCommands::Import(args) => {
            crate::cli::config::io_ops::execute_import(args, cli, cas_root)
        }
        ConfigCommands::Search(args) => {
            crate::cli::config::read_ops::execute_search(args, cli, cas_root)
        }
        ConfigCommands::Docs(args) => crate::cli::config::docs_ops::execute_docs(args, cli),
        ConfigCommands::Completions(args) => {
            crate::cli::config::docs_ops::execute_completions(args, cli)
        }
        ConfigCommands::Edit(args) => {
            crate::cli::config::edit_ops::execute_edit(args, cli, cas_root)
        }
        ConfigCommands::GrantWrite(args) => operator_ops::execute_grant_write(args, cli, cas_root),
        ConfigCommands::RevokeWrite(args) => operator_ops::execute_revoke_write(args, cli, cas_root),
    }
}
