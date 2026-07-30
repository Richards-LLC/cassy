//! `cas known-repos {list,seed,prune-missing}` — inspect and maintain the host
//! repo registry.
//!
//! The registry itself lives in `~/.cas/cas.db::known_repos` and is upserted
//! automatically by `cas init`, factory daemon startup, and MCP server
//! startup. The commands here exist for diagnostics and for one-time seeding
//! on hosts that pre-date the auto-upsert hooks.

use anyhow::Result;
use cas_store::KnownRepoStore;
use clap::Subcommand;

use crate::store::known_repos::{ensure_host_schema, open_host_known_repo_store};
use crate::worktree::discovery::{list_tracked_repos, seed};

#[derive(Subcommand, Clone, Debug)]
pub enum KnownReposCommands {
    /// Print every repo in the host-scoped known_repos registry.
    List,
    /// Seed the registry from existing host state (sessions.cwd + session
    /// JSON files). Idempotent.
    Seed {
        /// Additionally scan $HOME up to depth 5 for `.cas/` directories.
        /// Slow on large home directories; opt in explicitly.
        #[arg(long)]
        scan_home: bool,
    },
    /// Remove registry rows for paths that no longer exist.
    ///
    /// Repository files are never deleted. A moved/restored checkout can be
    /// registered again with `cas init` or `cas known-repos seed`.
    PruneMissing {
        /// Report what would be removed without changing the registry.
        #[arg(long)]
        dry_run: bool,
    },
}

pub fn execute(cmd: &KnownReposCommands) -> Result<()> {
    // `known-repos` is the bootstrap entry point for the registry, so we
    // install the schema here (idempotent) to cover hosts where `cas init`
    // predates the registry.
    ensure_host_schema()?;
    match cmd {
        KnownReposCommands::List => execute_list(),
        KnownReposCommands::Seed { scan_home } => execute_seed(*scan_home),
        KnownReposCommands::PruneMissing { dry_run } => execute_prune_missing(*dry_run),
    }
}

fn execute_prune_missing(dry_run: bool) -> Result<()> {
    let report = prune_missing(dry_run)?;
    if dry_run {
        println!(
            "Dry run: {} missing known-repo row(s) would be removed.",
            report.missing
        );
    } else {
        println!(
            "Removed {} missing known-repo row(s). Repository files were not changed.",
            report.removed
        );
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct PruneMissingReport {
    missing: usize,
    removed: usize,
}

fn prune_missing(dry_run: bool) -> Result<PruneMissingReport> {
    let store = open_host_known_repo_store()?;
    let missing = store
        .list()?
        .into_iter()
        .filter(|repo| !repo.path.exists())
        .collect::<Vec<_>>();
    let mut removed = 0;
    if !dry_run {
        for repo in &missing {
            // Recheck immediately before the registry-only delete so a path
            // restored during the scan is retained.
            if !repo.path.exists() {
                removed += store.forget(&repo.path)?;
            }
        }
    }
    Ok(PruneMissingReport {
        missing: missing.len(),
        removed,
    })
}

fn execute_list() -> Result<()> {
    let repos = list_tracked_repos()?;
    if repos.is_empty() {
        println!(
            "No known repos yet. Run `cas init` in a project, or `cas known-repos seed` to bootstrap from existing sessions."
        );
        return Ok(());
    }
    println!("{} known repo(s):", repos.len());
    for r in repos {
        let flag = if r.healthy { "ok    " } else { "MISSING" };
        println!(
            "  [{flag}] touch_count={:<4} {}",
            r.touch_count,
            r.path.display()
        );
    }
    Ok(())
}

fn execute_seed(scan_home: bool) -> Result<()> {
    eprintln!(
        "Seeding known_repos from sessions.cwd + ~/.cas/sessions/*.json{}...",
        if scan_home {
            " + $HOME walk (slow)"
        } else {
            ""
        }
    );
    let report = seed(scan_home)?;
    println!(
        "Seed complete: {} new, {} already-present, {} skipped (no .cas/)",
        report.new.len(),
        report.existing.len(),
        report.skipped_missing.len(),
    );
    if !report.new.is_empty() {
        println!("Newly registered:");
        for p in &report.new {
            println!("  + {}", p.display());
        }
    }
    if !report.skipped_missing.is_empty() {
        println!("Skipped (path has no .cas/ subdirectory):");
        for p in &report.skipped_missing {
            println!("  - {}", p.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;

    #[test]
    fn prune_missing_dry_run_is_non_mutating_and_apply_keeps_existing_paths() {
        TestEnvGuard::run_with_temp_home(|home| {
            ensure_host_schema().unwrap();
            let existing = home.join("existing");
            let missing = home.join("missing");
            std::fs::create_dir(&existing).unwrap();
            let store = open_host_known_repo_store().unwrap();
            store.upsert(&existing).unwrap();
            store.upsert(&missing).unwrap();

            assert_eq!(
                prune_missing(true).unwrap(),
                PruneMissingReport {
                    missing: 1,
                    removed: 0,
                }
            );
            assert_eq!(store.count().unwrap(), 2);

            assert_eq!(
                prune_missing(false).unwrap(),
                PruneMissingReport {
                    missing: 1,
                    removed: 1,
                }
            );
            let rows = store.list().unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].path, existing.canonicalize().unwrap());
        });
    }
}
