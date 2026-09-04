//! `cas known-repos {list,status,bind,unbind,seed,prune-missing}` — inspect and
//! maintain the host repo registry.
//!
//! The registry itself lives in `~/.cas/cas.db::known_repos` and is upserted
//! automatically by `cas init`, factory daemon startup, and MCP server
//! startup. The commands here exist for diagnostics and for one-time seeding
//! on hosts that pre-date the auto-upsert hooks.

use anyhow::Result;
use cas_store::KnownRepoStore;
use clap::Subcommand;
use std::path::PathBuf;

use crate::store::known_repos::{ensure_host_schema, open_host_known_repo_store};
use crate::worktree::discovery::{list_tracked_repos, seed};

#[derive(Subcommand, Clone, Debug)]
pub enum KnownReposCommands {
    /// Print every repo in the host-scoped known_repos registry.
    List,
    /// Show explicit selector bindings and validate their live host identity.
    Status,
    /// Explicitly bind a portable selector to one canonical repository root.
    Bind {
        /// Canonical repository root to select. Symlinks and nested paths are rejected.
        #[arg(long)]
        repo: PathBuf,
    },
    /// Remove one exact host-local selector binding. Repositories and their
    /// known-repo registrations are never removed.
    Unbind {
        /// Exact portable selector shown by `cas known-repos status`.
        selector: String,
    },
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
        KnownReposCommands::Status => execute_status(),
        KnownReposCommands::Bind { repo } => execute_bind(repo),
        KnownReposCommands::Unbind { selector } => execute_unbind(selector),
        KnownReposCommands::Seed { scan_home } => execute_seed(*scan_home),
        KnownReposCommands::PruneMissing { dry_run } => execute_prune_missing(*dry_run),
    }
}

fn execute_bind(repo: &std::path::Path) -> Result<()> {
    let (selector, repo_root, git_common_dir) =
        crate::mcp::tools::core::task::repo_context::binding_identity_for_path(repo)
            .map_err(anyhow::Error::msg)?;
    let store = open_host_known_repo_store()?;
    store
        .bind(&selector, &repo_root, &git_common_dir)
        .map_err(anyhow::Error::from)?;
    println!(
        "Bound selector `{selector}` to host repository {}.",
        repo_root.display()
    );
    println!("Portable task and delivery records remain path-free.");
    Ok(())
}

fn execute_unbind(selector: &str) -> Result<()> {
    let store = open_host_known_repo_store()?;
    let removed = store.unbind(selector)?;
    if removed == 0 {
        println!("No host-local binding exists for selector `{selector}`.");
    } else {
        println!(
            "Removed host-local binding for selector `{selector}`. Repository registration and files were not changed."
        );
    }
    Ok(())
}

fn execute_status() -> Result<()> {
    let store = open_host_known_repo_store()?;
    let bindings = store.list_bindings()?;
    if bindings.is_empty() {
        println!("No explicit host-local repository bindings.");
        return Ok(());
    }
    println!("{} host-local binding(s):", bindings.len());
    for binding in bindings {
        let state = if crate::mcp::tools::core::task::repo_context::binding_is_live(&binding) {
            "valid"
        } else {
            "STALE"
        };
        println!(
            "  [{state}] {} -> {}",
            binding.selector,
            binding.repo_root.display()
        );
    }
    Ok(())
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
    let bindings = open_host_known_repo_store()?.list_bindings()?;
    if !bindings.is_empty() {
        println!(
            "{} explicit binding(s); run `cas known-repos status` for validated details.",
            bindings.len()
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

    /// cas-647c: the incident had no exit. `prune-missing` only removes rows
    /// whose path is gone; the artifacts fixture still existed, so the
    /// supervisor had to hand-write `DELETE FROM known_repos`. `forget` is that
    /// missing verb: it removes the row and any binding that points at it,
    /// prints a receipt, and never touches the files.
    #[test]
    fn forget_removes_a_live_registry_row_and_its_bindings_and_is_idempotent_cas_647c() {
        TestEnvGuard::run_with_temp_home(|home| {
            ensure_host_schema().unwrap();
            let fixture = home.join("fresh-proxy");
            std::fs::create_dir_all(fixture.join(".git")).unwrap();
            let keep = home.join("myproject");
            std::fs::create_dir_all(&keep).unwrap();

            let store = open_host_known_repo_store().unwrap();
            store.upsert(&keep).unwrap();
            store
                .bind("project:fresh-proxy", &fixture, &fixture.join(".git"))
                .unwrap();
            assert_eq!(store.count().unwrap(), 2);

            let report = forget(&fixture, false).unwrap();
            assert_eq!(report.removed, 1);
            assert_eq!(report.unbound, vec!["project:fresh-proxy".to_string()]);
            assert_eq!(store.count().unwrap(), 1);
            assert!(store.get_binding("project:fresh-proxy").unwrap().is_none());
            assert!(fixture.exists(), "forget must never delete repository files");

            // Idempotent: a second run is a no-op receipt, not an error.
            assert_eq!(
                forget(&fixture, false).unwrap(),
                ForgetReport {
                    removed: 0,
                    unbound: Vec::new(),
                }
            );
            assert_eq!(store.count().unwrap(), 1);
        });
    }

    /// Forgetting the store you are standing in is almost always a mistake, so
    /// it needs an explicit `--yes`.
    #[test]
    fn forget_refuses_the_current_project_root_without_yes_cas_647c() {
        TestEnvGuard::run_with_temp_home(|home| {
            ensure_host_schema().unwrap();
            // TestEnvGuard pins CAS_ROOT to <home>/.cas, so <home> is the
            // current project root.
            let store = open_host_known_repo_store().unwrap();
            store.upsert(home).unwrap();

            let refusal = forget(home, false).unwrap_err().to_string();
            assert!(refusal.contains("current project root"), "{refusal}");
            assert!(refusal.contains("--yes"), "{refusal}");
            assert_eq!(store.count().unwrap(), 1, "refusal must not mutate");

            assert_eq!(forget(home, true).unwrap().removed, 1);
            assert_eq!(store.count().unwrap(), 0);
        });
    }

    #[test]
    fn prune_missing_never_removes_or_rebinds_stale_selector_binding() {
        TestEnvGuard::run_with_temp_home(|home| {
            ensure_host_schema().unwrap();
            let stale = home.join("removed");
            std::fs::create_dir_all(stale.join(".git")).unwrap();
            let store = open_host_known_repo_store().unwrap();
            store
                .bind("project:stale", &stale, &stale.join(".git"))
                .unwrap();
            std::fs::remove_dir_all(&stale).unwrap();

            assert_eq!(
                prune_missing(false).unwrap(),
                PruneMissingReport {
                    missing: 1,
                    removed: 1,
                }
            );
            assert_eq!(store.count().unwrap(), 0);
            assert!(
                store.get_binding("project:stale").unwrap().is_some(),
                "stale binding must remain explicit until operator unbind"
            );
        });
    }
}
