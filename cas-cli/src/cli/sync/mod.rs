//! Local synchronization commands.

mod agents_md;

use clap::Subcommand;

use super::Cli;

pub use agents_md::AgentsMdArgs;

#[derive(Subcommand)]
pub enum SyncCommands {
    /// Generate AGENTS.md files from CLAUDE.md for Codex sessions
    #[command(name = "agents-md")]
    AgentsMd(AgentsMdArgs),
}

pub fn execute(command: &SyncCommands, cli: &Cli) -> anyhow::Result<()> {
    match command {
        SyncCommands::AgentsMd(args) => agents_md::execute(args, cli),
    }
}
