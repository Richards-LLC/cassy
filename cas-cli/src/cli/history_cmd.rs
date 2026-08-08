//! `cas history` — drive and inspect the structural git-history index
//! (EPIC cas-6212 / cas-7a21).
//!
//! `backfill` runs an indexing pass (full or delta, decided by the watermark);
//! `status` only reads. The split matters: spec §4.2 rule 5 says a freshness
//! check must never index, so `status` never shells out to anything that writes.

use std::path::Path;

use clap::{Args, Subcommand};

use crate::history::{self, WalkMode};

#[derive(Debug, Clone, Subcommand)]
pub enum HistoryCommands {
    /// Index commits into the history tables (full backfill, or a delta from
    /// the watermark when one exists)
    Backfill(BackfillArgs),
    /// Report the watermark, indexed counts and lag without indexing anything
    Status(StatusArgs),
}

#[derive(Debug, Clone, Args)]
pub struct BackfillArgs {
    /// Discard the watermark and re-walk the whole history
    #[arg(long)]
    pub force: bool,

    /// Report what the pass would do without writing rows
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Args)]
pub struct StatusArgs {
    /// Emit JSON instead of prose
    #[arg(long)]
    pub json: bool,
}

pub fn execute(
    command: &HistoryCommands,
    _cli: &crate::cli::Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    match command {
        HistoryCommands::Backfill(args) => execute_backfill(args, cas_root),
        HistoryCommands::Status(args) => execute_status(args, cas_root),
    }
}

fn execute_backfill(args: &BackfillArgs, cas_root: &Path) -> anyhow::Result<()> {
    let repo_root = history::repo_root_for(cas_root)?;

    if args.dry_run {
        let s = history::status(cas_root, &repo_root)?;
        let pending = match s.lag_commits {
            Some(lag) if s.state.as_ref().is_some_and(|st| st.backfill_complete) => lag,
            _ => s.repo_commits,
        };
        println!(
            "dry run: would index {pending} commit(s) ({} already indexed of {} reachable)",
            s.indexed_commits, s.repo_commits
        );
        return Ok(());
    }

    if args.force {
        let store = cas_store::SqliteHistoryStore::open(cas_root)?;
        cas_store::HistoryStore::reset_watermark(
            &store,
            &history::repository_id(&repo_root),
            cas_store::SOURCE_GIT,
        )?;
        println!("watermark cleared; re-walking full history");
    }

    let started = std::time::Instant::now();
    let outcome = history::run_index_pass(cas_root, &repo_root)?;
    let elapsed = started.elapsed();

    if outcome.watermark_reset {
        println!(
            "watermark was not an ancestor of HEAD (rebase/force-push); ran a full re-backfill"
        );
    }

    match outcome.mode {
        WalkMode::UpToDate => println!("history index already current at {}", outcome.head_sha),
        _ => println!(
            "{}: {} commit(s), {} file change(s) in {} chunk(s) — {:.1}s",
            outcome.mode.as_str(),
            outcome.commits_indexed,
            outcome.files_indexed,
            outcome.chunks,
            elapsed.as_secs_f64()
        ),
    }

    Ok(())
}

fn execute_status(args: &StatusArgs, cas_root: &Path) -> anyhow::Result<()> {
    let repo_root = history::repo_root_for(cas_root)?;
    let s = history::status(cas_root, &repo_root)?;

    let watermark = s
        .state
        .as_ref()
        .and_then(|st| st.last_indexed_sha.clone())
        .unwrap_or_else(|| "none".to_string());
    let backfill_complete = s.state.as_ref().is_some_and(|st| st.backfill_complete);
    // "unknown" rather than "0": a missing or unreachable watermark means the
    // lag is not known, and printing 0 would read as fresh (spec §10.1).
    let lag = s
        .lag_commits
        .map(|l| l.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    if args.json {
        let payload = serde_json::json!({
            "repository": s.repository,
            "head_sha": s.head_sha,
            "watermark": s.state.as_ref().and_then(|st| st.last_indexed_sha.clone()),
            "watermark_is_ancestor": s.watermark_is_ancestor,
            "backfill_complete": backfill_complete,
            "indexed_commits": s.indexed_commits,
            "indexed_commit_file_pairs": s.indexed_pairs,
            "repo_commits": s.repo_commits,
            "lag_commits": s.lag_commits,
            "current": s.is_current(),
            "last_indexed_at": s.state.as_ref().and_then(|st| st.last_indexed_at.clone()),
            "last_attempt_at": s.state.as_ref().and_then(|st| st.last_attempt_at.clone()),
            "last_error": s.state.as_ref().and_then(|st| st.last_error.clone()),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!("Code-history index — {}", s.repository);
    println!("  HEAD            {}", s.head_sha);
    println!("  watermark       {watermark}");
    if s.state.is_some() && !s.watermark_is_ancestor {
        println!("                  (not an ancestor of HEAD — next pass re-backfills)");
    }
    println!(
        "  commits         {} indexed of {} reachable",
        s.indexed_commits, s.repo_commits
    );
    println!("  file changes    {}", s.indexed_pairs);
    println!("  lag             {lag} commit(s) behind HEAD");
    println!(
        "  backfill        {}",
        if backfill_complete {
            "complete"
        } else {
            "incomplete"
        }
    );
    if let Some(state) = &s.state {
        if let Some(at) = &state.last_indexed_at {
            println!("  last indexed    {at}");
        }
        if let Some(at) = &state.last_attempt_at {
            println!("  last attempt    {at}");
        }
        if let Some(err) = &state.last_error {
            println!("  last error      {err}");
        }
    } else {
        println!("  status          never indexed — run `cas history backfill`");
    }

    Ok(())
}
