//! `cas qa release-gate`: start, inspect, record and abort the release-gate
//! run for a staging SHA. `cas serve` drives the automated suites.
use crate::qa_release::run::{self, Record, Run};
use crate::qa_release::verdict::NotExercisedCause;
use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct ReleaseGateArgs {
    /// Start the gate for this SHA, or show its run if one exists.
    #[arg(long)]
    pub sha: Option<String>,
    #[command(subcommand)]
    pub command: Option<ReleaseGateCommand>,
}

#[derive(Debug, Subcommand)]
pub enum ReleaseGateCommand {
    /// Show a run: state, freeze, rows by verdict and what is owed.
    Status {
        /// Defaults to the live run.
        #[arg(long)]
        sha: Option<String>,
    },
    /// Record a manual row, or re-walk an owed row of a sealed OWED gate.
    Record {
        #[arg(long)]
        sha: Option<String>,
        #[arg(long)]
        row: String,
        /// PASS, FAIL, NOT_A_ROUTE or NOT_EXERCISED.
        #[arg(long)]
        verdict: String,
        /// Directory of frames copied into the pack (required for PASS).
        #[arg(long)]
        evidence: Option<PathBuf>,
        #[arg(long)]
        reason: Option<String>,
        /// NOT_EXERCISED cause: fixture_setup, automation, timed_out or operator_skip.
        #[arg(long)]
        cause: Option<String>,
        /// Hand-walk minutes.
        #[arg(long)]
        minutes: Option<u32>,
        /// Read-back subject id, `kind=id` (repeatable).
        #[arg(long = "subject")]
        subjects: Vec<String>,
        /// PR number this row covers (repeatable).
        #[arg(long = "covers-pr")]
        covers_prs: Vec<u64>,
        /// Direct-push commit this row covers (repeatable).
        #[arg(long = "covers-commit")]
        covers_commits: Vec<String>,
    },
    /// End a live run without a pack and lift the freeze.
    Abort {
        #[arg(long)]
        sha: Option<String>,
        #[arg(long)]
        reason: String,
    },
    /// Evaluate the promotion check for a PR head and publish its status.
    Verify {
        #[arg(long)]
        sha: String,
    },
}

pub fn execute(args: &ReleaseGateArgs, cli: &super::Cli, cas_root: Option<&Path>) -> Result<()> {
    let root = cas_root.context("No Cassy project; run cas init first")?;
    let run = match (&args.command, &args.sha) {
        (None, Some(sha)) => run::start(root, sha)?,
        (None, None) => bail!("pass --sha <sha> to start a gate, or use status, record or abort"),
        (Some(ReleaseGateCommand::Status { sha }), _) => {
            // A corrupt run file is reported even when it hides the live run.
            for (path, error) in run::unreadable(root) {
                eprintln!("[WARN] unreadable release-gate run file {}: {error}", path.display());
            }
            run::resolve(root, sha.as_deref())?
        }
        (Some(ReleaseGateCommand::Abort { sha, reason }), _) => run::abort(root, sha.as_deref(), reason)?,
        (Some(ReleaseGateCommand::Verify { sha }), _) => {
            return crate::qa_release::promotion::verify_cli(root, sha, cli.json);
        }
        (
            Some(ReleaseGateCommand::Record {
                sha,
                row,
                verdict,
                evidence,
                reason,
                cause,
                minutes,
                subjects,
                covers_prs,
                covers_commits,
            }),
            _,
        ) => {
            let cause = cause
                .as_deref()
                .map(|c| {
                    serde_json::from_value::<NotExercisedCause>(c.into())
                        .with_context(|| format!("--cause {c} is not fixture_setup, automation, timed_out or operator_skip"))
                })
                .transpose()?;
            let subjects = subjects
                .iter()
                .map(|pair| {
                    pair.split_once('=')
                        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
                        .with_context(|| format!("--subject {pair} is not kind=id"))
                })
                .collect::<Result<BTreeMap<_, _>>>()?;
            let input = Record {
                row: row.clone(),
                verdict: verdict.clone(),
                reason: reason.clone(),
                cause,
                evidence: evidence.clone(),
                minutes: *minutes,
                subjects,
                covers_prs: covers_prs.clone(),
                covers_commits: covers_commits.clone(),
            };
            run::record(root, sha.as_deref(), &input)?
        }
    };
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&run::status_json(root, &run)?)?);
    } else {
        print!("{}", render(&run, &run::hand_walk(root, &run)));
    }
    Ok(())
}

/// The human status: state and freeze first, then counts, then what is owed.
fn render(run: &Run, hand_walk: &crate::qa_release::pack::HandWalk) -> String {
    let state = serde_json::to_value(run.state).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
    let mut out = format!("Release gate {} on {}: {state}\n", crate::qa_release::short(&run.sha), run.env);
    if let Some(gate) = &run.gate {
        let word = serde_json::to_value(gate.verdict).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
        out.push_str(&format!("Gate: {word} (pack generation {})\n", run.generation));
        for owed in &gate.owed {
            out.push_str(&format!("  owed {}: {}\n", owed.subject, owed.cause));
        }
    }
    if run.frozen(chrono::Utc::now()) {
        out.push_str(&format!("Freeze: merges into the integration branch refused until {}\n", run.freeze_until.format("%Y-%m-%d %H:%M UTC")));
    }
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for row in &run.rows {
        *counts.entry(row.verdict.word()).or_default() += 1;
    }
    let pending: Vec<&str> = run.pending().map(|p| p.id.as_str()).collect();
    let summary: Vec<String> = counts.iter().map(|(word, n)| format!("{n} {word}")).collect();
    out.push_str(&format!("Rows: {} planned; {}{}\n", run.plan.len(), summary.join(", "), if pending.is_empty() { String::new() } else { format!("; {} without a verdict", pending.len()) }));
    if let Some(summary) = hand_walk.summary(3) {
        out.push_str(&format!("Hand-walk debt: {summary}\n"));
    }
    for row in &run.rows {
        if !matches!(row.verdict.word(), "PASS" | "NOT_A_ROUTE") {
            out.push_str(&format!("  {}: {}\n", row.id, row.verdict));
        }
    }
    for planned in run.pending() {
        let how = if planned.manual { "manual: cas qa release-gate record --row <id> …" } else { "awaiting the cas serve driver" };
        out.push_str(&format!("  {}: {how}\n", planned.id));
    }
    if let Some(reason) = &run.abort_reason {
        out.push_str(&format!("Aborted: {reason}\n"));
    }
    out
}
