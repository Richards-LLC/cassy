//! `cas worktree sweep` / `cas sweep-all` — reclaim factory worktree directories.
//!
//! Wraps the pure sweep logic in `crate::worktree::sweep` with human-readable
//! stdout formatting. The sweep module itself returns structured reports; the
//! command-line interaction (flag parsing, "would do" vs. "did", per-repo
//! counts, cumulative bytes) lives here.

use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

use crate::worktree::sweep::{
    Disposition, RepoSweepReport, SweepOptions, sweep_all_known, sweep_one_repo,
};

/// Flags shared by `cas worktree sweep` and `cas sweep-all`.
#[derive(Args, Clone, Debug)]
pub struct SweepBaseArgs {
    /// Report only — do not remove any worktree or write any patch.
    #[arg(long)]
    pub dry_run: bool,
    /// Dirty worktrees are normally skipped. With this flag, dirty+merged
    /// worktrees are first captured to `<repo>/.cas/salvage/*.patch` (via the
    /// Unit 2 salvage module) and then removed. Unmerged worktrees are still
    /// preserved regardless of this flag.
    #[arg(long)]
    pub salvage_dirty: bool,
}

#[derive(Args, Clone, Debug)]
pub struct SweepArgs {
    /// Sweep every repo in the host known_repos registry instead of just the
    /// current repo.
    #[arg(long)]
    pub all_repos: bool,
    #[command(flatten)]
    pub base: SweepBaseArgs,
}

/// `cas worktree sweep`.
pub fn execute_sweep(args: &SweepArgs) -> Result<()> {
    let opts = SweepOptions {
        dry_run: args.base.dry_run,
        salvage_dirty: args.base.salvage_dirty,
    };

    if args.all_repos {
        return run_all(opts);
    }

    let cwd = std::env::current_dir()?;
    let repo_root = find_repo_root(&cwd)?;
    let report = sweep_one_repo(&repo_root, opts);
    print_repo(&report, opts);
    Ok(())
}

/// `cas sweep-all` — equivalent to `cas worktree sweep --all-repos`. No
/// `--all-repos` flag here because it is redundant at this entry point.
pub fn execute_sweep_all(args: &SweepBaseArgs) -> Result<()> {
    let opts = SweepOptions {
        dry_run: args.dry_run,
        salvage_dirty: args.salvage_dirty,
    };
    run_all(opts)
}

fn run_all(opts: SweepOptions) -> Result<()> {
    let report = sweep_all_known(opts)?;
    if report.repos.is_empty() {
        println!(
            "No repos in the host known_repos registry. \
             Run `cas known-repos seed` to bootstrap from existing state, or \
             `cas init` in a repo to register it."
        );
        return Ok(());
    }
    for repo in &report.repos {
        print_repo(repo, opts);
    }
    let removed = report.total_removed();
    let skipped = report.total_skipped();
    let bytes = report.total_bytes_reclaimed();
    println!(
        "\n— Total: {removed} removed, {skipped} skipped across {} repo(s){} ({}) —",
        report.repos.len(),
        if opts.dry_run { " (dry run)" } else { "" },
        format_bytes(bytes),
    );
    // Log the summary line to the host sweep log for Unit 3's debounce
    // mtime check to be meaningful to humans grepping the log later.
    append_log_line(&format!(
        "{} {} mode={} repos={} removed={} skipped={} bytes={}",
        chrono::Utc::now().to_rfc3339(),
        env!("CARGO_PKG_VERSION"),
        if opts.dry_run { "dry" } else { "wet" },
        report.repos.len(),
        removed,
        skipped,
        bytes,
    ));
    Ok(())
}

fn print_repo(report: &RepoSweepReport, _opts: SweepOptions) {
    println!("\n{}", report.repo_root.display());
    if let Some(err) = &report.repo_error {
        println!("  ! {err}");
        return;
    }
    if report.worktrees.is_empty() {
        println!("  (no factory worktrees)");
        return;
    }
    for wt in &report.worktrees {
        println!(
            "  {} {}",
            badge(&wt.disposition),
            wt.worktree_path.display()
        );
        if let Some(detail) = detail(&wt.disposition) {
            println!("      {detail}");
        }
    }
    println!(
        "  → {} removed, {} skipped{}",
        report.removed_count(),
        report.skipped_count(),
        if report.prune_ran {
            ", prune ran"
        } else {
            ""
        },
    );
}

fn badge(d: &Disposition) -> &'static str {
    match d {
        Disposition::Removed => "[removed]       ",
        Disposition::SalvagedAndRemoved { .. } => "[salvaged+rm]   ",
        Disposition::SkippedDirty { .. } => "[skip: dirty]   ",
        Disposition::SkippedUnmerged { .. } => "[skip: unmerged]",
        Disposition::SkippedDirtyUnmerged { .. } => "[skip: d+u]     ",
        Disposition::WouldRemove => "[would remove]  ",
        Disposition::WouldSalvageAndRemove => "[would salvage] ",
        Disposition::Error { .. } => "[error]         ",
    }
}

fn detail(d: &Disposition) -> Option<String> {
    match d {
        Disposition::SalvagedAndRemoved { patch_path } => {
            Some(format!("patch: {}", patch_path.display()))
        }
        Disposition::SkippedDirty { modified_files } => {
            Some(format!("{modified_files} modified file(s); rerun with --salvage-dirty to capture"))
        }
        Disposition::SkippedUnmerged { unmerged_commits } => Some(format!(
            "{unmerged_commits} unmerged commit(s); merge or delete the branch first"
        )),
        Disposition::SkippedDirtyUnmerged {
            modified_files,
            unmerged_commits,
        } => Some(format!(
            "{modified_files} modified + {unmerged_commits} unmerged; \
             unmerged worktrees are never auto-removed"
        )),
        Disposition::Error { reason } => Some(reason.clone()),
        _ => None,
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.2} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn find_repo_root(cwd: &std::path::Path) -> Result<PathBuf> {
    // Delegate to the established resolver so we honor CAS_ROOT, factory
    // worktree detection, and git-file parsing exactly like every other
    // Cassy command. Returns the `.cas` directory; the repo root is its parent.
    let cas_root = crate::store::find_cas_root_from(cwd).map_err(|_| {
        anyhow::anyhow!(
            "Not inside a Cassy-initialized repo. Run `cas init`, \
             or pass --all-repos to sweep every known repo."
        )
    })?;
    cas_root
        .parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("cannot resolve repo root from {}", cas_root.display()))
}

fn append_log_line(line: &str) {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let logs = home.join(".cas").join("logs");
    if let Err(e) = std::fs::create_dir_all(&logs) {
        tracing::warn!(error = %e, "could not create ~/.cas/logs");
        return;
    }
    let path = logs.join("global-sweep.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
}
