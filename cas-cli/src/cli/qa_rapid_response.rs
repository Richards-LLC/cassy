//! `cas qa rapid-response` (cas-1b4d): the operator opens or closes a
//! time-boxed rapid-response window for one epic. See `crate::rapid_response`.
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use std::path::Path;

use crate::config::operator_policy::{InvocationContext, operator_action_refusal};

#[derive(Debug, Args)]
pub struct RapidResponseArgs {
    #[command(subcommand)]
    pub command: RapidResponseCommand,
}

#[derive(Debug, Subcommand)]
pub enum RapidResponseCommand {
    /// Open a window for an epic (operator only); it expires on its own
    Enable {
        epic: String,
        /// How long the window stays open, such as 90m or 2h (at most 12h)
        #[arg(long = "for")]
        duration: String,
        /// Why the work must ship fast; recorded on the epic
        #[arg(long)]
        reason: String,
        /// How a bad release is undone; recorded on the epic
        #[arg(long)]
        rollback: String,
    },
    /// Close an epic's window now (operator only); its back-fill runs next
    Disable { epic: String },
    /// Show open and closed windows
    Status,
}

pub fn execute(args: &RapidResponseArgs, cas_root: Option<&Path>) -> Result<()> {
    let root = cas_root.context("No Cassy project; run cas init first")?;
    let now = chrono::Utc::now();
    let operator = || -> Result<()> {
        if let Some(refusal) = operator_action_refusal(
            "Rapid response mode",
            "run `cas qa rapid-response` yourself from your own terminal",
            &InvocationContext::from_process(),
        ) {
            anyhow::bail!(refusal);
        }
        Ok(())
    };
    match &args.command {
        RapidResponseCommand::Enable { epic, duration, reason, rollback } => {
            operator()?;
            let store = crate::store::open_task_store(root)?;
            let duration = crate::rapid_response::parse_duration(duration)?;
            let window = crate::rapid_response::enable(root, store.as_ref(), epic, duration, reason, rollback, now)?;
            println!(
                "Rapid response ON for {} until {} UTC. Skipped checks back-fill when it closes.",
                window.epic,
                window.expires_at.format("%Y-%m-%d %H:%M")
            );
        }
        RapidResponseCommand::Disable { epic } => {
            operator()?;
            let store = crate::store::open_task_store(root)?;
            if crate::rapid_response::disable(root, store.as_ref(), epic, now)? {
                println!("Rapid response OFF for {epic}; the factory back-fills its skipped checks next.");
            } else {
                println!("No open rapid-response window for {epic}.");
            }
        }
        RapidResponseCommand::Status => {
            let state = crate::rapid_response::load(root)?;
            if state.windows.is_empty() {
                println!("No rapid-response windows.");
            }
            for window in &state.windows {
                println!(
                    "{} {} until {} UTC, {} skipped check(s)",
                    window.epic,
                    if window.is_active(now) { "OPEN" } else { "closed" },
                    window.expires_at.format("%Y-%m-%d %H:%M"),
                    crate::rapid_response::ledger(root, window).len()
                );
            }
        }
    }
    Ok(())
}
