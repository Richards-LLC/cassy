use clap::Args;

use cas_core::sync::{AgentsMdSyncMode, sync_agents_md};

use crate::cli::Cli;

/// Arguments for `cas sync agents-md`.
#[derive(Debug, Clone, Args)]
pub struct AgentsMdArgs {
    /// Exit unsuccessfully instead of writing when AGENTS.md is stale
    #[arg(long, conflicts_with = "write")]
    pub check: bool,

    /// Write generated AGENTS.md files (the default unless --check is used)
    #[arg(long)]
    pub write: bool,
}

pub fn execute(args: &AgentsMdArgs, _cli: &Cli) -> anyhow::Result<()> {
    let project_root = std::env::current_dir()?;
    let mode = if args.check {
        AgentsMdSyncMode::Check
    } else {
        AgentsMdSyncMode::Write
    };
    let report = sync_agents_md(&project_root, mode)?;

    if args.check && report.stale_count() > 0 {
        for file in report.stale_files() {
            eprintln!("stale: {}", file.output.display());
        }
        anyhow::bail!(
            "{} AGENTS.md file(s) are stale; run `cas sync agents-md --write`",
            report.stale_count()
        );
    }

    if args.check {
        println!(
            "AGENTS.md files are current ({} checked).",
            report.files.len()
        );
    } else {
        println!("Synchronized {} AGENTS.md file(s).", report.files.len());
    }
    Ok(())
}
