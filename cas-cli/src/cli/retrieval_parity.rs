//! `cas retrieval-parity` — capture and replay memory-retrieval baselines.
//!
//! See [`crate::retrieval_parity`] for the design. Both subcommands are
//! read-only against the memory and knowledge stores; `capture` writes exactly
//! one file, the baseline fixture, and nothing else.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};

use crate::retrieval_parity::{
    self, Baseline, DEFAULT_RANK_TOLERANCE, ParityContext, QuerySet, machine_id,
};

/// Default location of the committed query set, relative to the repo root.
pub const DEFAULT_QUERYSET: &str = "fixtures/retrieval-parity/queryset.toml";
/// Directory holding the committed per-machine baselines.
pub const DEFAULT_BASELINE_DIR: &str = "fixtures/retrieval-parity";

#[derive(Debug, Subcommand)]
pub enum RetrievalParityCommands {
    /// Run the query set against the current system and write a baseline.
    Capture(CaptureArgs),
    /// Re-run the query set and diff it against a captured baseline.
    Replay(ReplayArgs),
}

#[derive(Debug, Args)]
pub struct CaptureArgs {
    /// Query-set TOML to run.
    #[arg(long, default_value = DEFAULT_QUERYSET)]
    pub queryset: PathBuf,
    /// Where to write the baseline. Defaults to the per-machine fixture path.
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Capture even though the query set does not probe every entry type and
    /// tier present in the store. Records a knowingly partial baseline.
    #[arg(long)]
    pub allow_uncovered: bool,
}

#[derive(Debug, Args)]
pub struct ReplayArgs {
    /// Query-set TOML to run. Must be the set the baseline was captured with.
    #[arg(long, default_value = DEFAULT_QUERYSET)]
    pub queryset: PathBuf,
    /// Baseline to compare against. Defaults to the per-machine fixture path.
    #[arg(long)]
    pub baseline: Option<PathBuf>,
    /// Positions a hit may slip before it counts as a regression. Overridden
    /// by any per-case `rank_tolerance` in the query set.
    #[arg(long)]
    pub rank_tolerance: Option<usize>,
    /// Emit the full report as JSON instead of text.
    #[arg(long)]
    pub json: bool,
}

/// Conventional fixture path for a machine's baseline.
pub fn default_baseline_path(machine: &str) -> PathBuf {
    Path::new(DEFAULT_BASELINE_DIR).join(format!("baseline-{machine}.json"))
}

/// Resolve the global memory store a parity run should merge.
///
/// Prefers the host root `~/.cas` — which is where the live global CAS state
/// actually lives — and only falls back to [`crate::config::global_cas_dir`]
/// (`~/.config/cas` on Linux, `Application Support` on macOS) when the host
/// root holds no database. See the rationale comment on
/// [`crate::store::known_repos`] for the history of that split.
///
/// Using `global_cas_dir()` alone is what made every parity run on this host
/// silently project-only: the config dir has no `cas.db`, so the global tier
/// was filtered out before it was ever measured (cas-96ae).
pub fn resolve_global_cas_dir() -> Option<PathBuf> {
    let host = crate::store::known_repos::host_cas_dir();
    if host.join("cas.db").exists() {
        return Some(host);
    }
    crate::config::global_cas_dir()
}

pub fn execute(cmd: &RetrievalParityCommands, cas_root: Option<&Path>) -> anyhow::Result<()> {
    let cas_root = cas_root.ok_or_else(|| {
        anyhow::anyhow!("no .cas directory found — run this from inside an initialized project")
    })?;
    // The SessionStart merge reads project-then-global, so the merge channel
    // needs the global store to reproduce the dedup faithfully.
    let ctx = ParityContext::new(cas_root).with_global(resolve_global_cas_dir());

    match &ctx.global_cas_dir {
        Some(dir) => println!("global store: {}", dir.display()),
        None => match &ctx.global_unavailable {
            // Loud on stderr as well as in the per-case status: a run that
            // could not reach the global store measures strictly less than it
            // appears to.
            Some(reason) => eprintln!("WARNING: {reason}"),
            None => println!("global store: none configured (project-only run)"),
        },
    }

    match cmd {
        RetrievalParityCommands::Capture(args) => run_capture(&ctx, args),
        RetrievalParityCommands::Replay(args) => run_replay(&ctx, args),
    }
}

fn run_capture(ctx: &ParityContext, args: &CaptureArgs) -> anyhow::Result<()> {
    let set = QuerySet::load(&args.queryset)?;
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| default_baseline_path(&machine_id()));

    let now = chrono::Utc::now().to_rfc3339();
    let baseline = retrieval_parity::capture(ctx, &set, now, args.allow_uncovered)?;
    retrieval_parity::save_baseline(&out, &baseline)?;

    let unavailable: Vec<&str> = baseline
        .results
        .iter()
        .filter(|r| !r.status.is_ok())
        .map(|r| r.id.as_str())
        .collect();
    let hits: usize = baseline.results.iter().map(|r| r.hits.len()).sum();

    println!(
        "captured baseline for machine '{}' -> {}",
        baseline.machine,
        out.display()
    );
    println!(
        "  {} case(s), {hits} hit(s), corpus: {} active entries, types [{}], tiers [{}]",
        baseline.results.len(),
        baseline.corpus.active_entries,
        baseline.corpus.entry_types.join(", "),
        baseline.corpus.tiers.join(", "),
    );
    if !unavailable.is_empty() {
        // Loud, because a baseline captured with a channel down blesses
        // nothing about that channel — the replay will merely agree it is
        // still down.
        println!(
            "  WARNING: {} case(s) captured with the channel UNAVAILABLE, so they \
             assert nothing: {}",
            unavailable.len(),
            unavailable.join(", ")
        );
    }
    Ok(())
}

fn run_replay(ctx: &ParityContext, args: &ReplayArgs) -> anyhow::Result<()> {
    let set = QuerySet::load(&args.queryset)?;
    let path = args
        .baseline
        .clone()
        .unwrap_or_else(|| default_baseline_path(&machine_id()));
    let baseline: Baseline = retrieval_parity::load_baseline(&path)?;

    let tolerance = args.rank_tolerance.unwrap_or(DEFAULT_RANK_TOLERANCE);
    let report = retrieval_parity::replay(ctx, &set, &baseline, tolerance)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render());
    }

    if !report.passed() {
        // Non-zero exit so CI and the migration runbook fail closed.
        std::process::exit(1);
    }
    Ok(())
}
