//! `cas worktree ...` — worktree-scoped diagnostics and maintenance commands.
//!
//! Exposes `cas worktree sweep` and `cas worktree pr-body`.

use anyhow::Result;
use clap::{Args, Subcommand};
use std::path::PathBuf;

use crate::cli::sweep::{SweepArgs, execute_sweep};

#[derive(Subcommand, Clone, Debug)]
pub enum WorktreeCommands {
    /// Reclaim factory worker worktree directories that are clean+merged
    /// (and optionally salvage dirty ones via `--salvage-dirty`).
    Sweep(SweepArgs),
    /// Write a review body with a change tree, before/after evidence and recorded risk/door.
    PrBody(PrBodyArgs),
}

pub fn execute(cmd: &WorktreeCommands) -> Result<()> {
    match cmd {
        WorktreeCommands::Sweep(args) => execute_sweep(args),
        WorktreeCommands::PrBody(args) => execute_pr_body(args),
    }
}

#[derive(Args, Clone, Debug)]
pub struct PrBodyArgs {
    /// Task whose recorded risk, door and QA references belong to this delivery.
    #[arg(long)]
    pub task: Option<String>,
    /// Comparison base (resolved to an immutable commit).
    #[arg(long)]
    pub base: String,
    /// Delivery head.
    #[arg(long, default_value = "HEAD")]
    pub head: String,
    /// Observed baseline result, using the same command as --after.
    #[arg(long)]
    pub before: Option<String>,
    /// Observed changed result.
    #[arg(long)]
    pub after: Option<String>,
    /// Link or path to a QA bundle or base-vs-change run.
    #[arg(long)]
    pub evidence: Option<String>,
    /// Write the Markdown here for gh --body-file; omit to print the body.
    #[arg(long)]
    pub output: Option<PathBuf>,
}

fn execute_pr_body(args: &PrBodyArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let task = if let Some(id) = &args.task {
        let root = crate::store::find_cas_root()?;
        Some(crate::store::open_task_store_local(&root)?.get(id)?)
    } else {
        None
    };
    let body = crate::review_body::delivery_body(
        &cwd,
        task.as_ref(),
        &args.base,
        &args.head,
        args.before.as_deref(),
        args.after.as_deref(),
        args.evidence.as_deref(),
    )?;
    if let Some(path) = &args.output {
        std::fs::write(path, body)?;
    } else {
        print!("{body}");
    }
    Ok(())
}
