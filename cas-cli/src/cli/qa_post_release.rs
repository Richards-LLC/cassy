//! Post-release CLI adapter. Jobs are persisted for the `cas serve` runner.
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use std::path::Path;

#[derive(Debug, Args)]
pub struct PostReleaseArgs {
    #[command(subcommand)]
    pub command: PostReleaseCommand,
}
#[derive(Debug, Subcommand)]
pub enum PostReleaseCommand {
    /// Observe a released SHA in the background (operator only; requires cas serve).
    Start {
        #[arg(long)]
        epic: String,
        #[arg(long)]
        sha: String,
    },
    /// Show the latest observation receipt and current verdict.
    Status {
        #[arg(long)]
        epic: String,
    },
}
pub fn execute(args: &PostReleaseArgs, cli: &super::Cli, cas_root: Option<&Path>) -> Result<()> {
    let root = cas_root.context("No Cassy project; run cas init first")?;
    let receipt = match &args.command {
        PostReleaseCommand::Start { epic, sha } => {
            use crate::config::operator_policy::{InvocationContext, operator_action_refusal};
            if let Some(refusal) = operator_action_refusal(
                "Recording a post-release SHA",
                "run `cas qa post-release start` yourself from your own terminal",
                &InvocationContext::from_process(),
            ) {
                anyhow::bail!(refusal);
            }
            crate::qa_post_release::start(root, epic, sha)?
        }
        PostReleaseCommand::Status { epic } => crate::qa_post_release::load(root, epic)?
            .context("No post-release check started for this epic")?,
    };
    let verdict = receipt.verdict(chrono::Utc::now());
    if cli.json {
        let mut value = serde_json::to_value(&receipt)?;
        value["verdict"] = serde_json::to_value(verdict)?;
        value["next_run_at"] = serde_json::to_value(receipt.next_run_at())?;
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        let state = serde_json::to_value(verdict)?;
        println!(
            "Post-release {}: {} ({}/{} runs)\nSHA: {}",
            receipt.epic,
            state.as_str().unwrap(),
            receipt.runs.len(),
            receipt.expected_runs(),
            receipt.sha
        );
        if let Some(line) = crate::qa_post_release::watch_first_line(&receipt.watch_first) {
            println!("{line}");
        }
    }
    Ok(())
}
