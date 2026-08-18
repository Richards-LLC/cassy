//! `cas memory-migrate` — carry the legacy memory store into the distilled
//! knowledge system (cas-f4c1, M3 of EPIC cas-b129).
//!
//! Dry-run first, always: without `--apply` this reads both databases
//! read-only, routes every row through the §4 decision procedure, prints the
//! loss audit and the full contamination quarantine, and writes nothing to the
//! knowledge store.

use std::path::{Path, PathBuf};

use clap::Args;

use crate::memory_migration::{self, DbLabel, MigrationConfig, SourceDb};

/// Ledger location relative to the project Cassy root.
const DEFAULT_LEDGER_SUBDIR: &str = "migration/cas-b129";

#[derive(Debug, Clone, Args)]
pub struct MemoryMigrateArgs {
    /// Actually write pages. Without this the command is report-only.
    #[arg(long)]
    pub apply: bool,

    /// Which legacy databases to migrate.
    #[arg(long, value_parser = ["project", "global", "both"], default_value = "both")]
    pub scope: String,

    /// Override the project Cassy root (default: the detected one).
    ///
    /// `cas_root` detection honours `CAS_ROOT` *before* the working directory
    /// (`store/detect.rs:53`), so `cd`-ing into a copy of a `.cas` tree does
    /// NOT retarget this command — under a factory worker, `CAS_ROOT` points at
    /// the live database. Rehearsing the migration on a copy therefore requires
    /// this flag, and the banner below prints the resolved paths before any
    /// work starts.
    #[arg(long)]
    pub project_root: Option<PathBuf>,

    /// Override the global Cassy root (default: `~/.cas`).
    #[arg(long)]
    pub global_root: Option<PathBuf>,

    /// Where the migration ledger, audit and quarantine files are written.
    #[arg(long)]
    pub ledger: Option<PathBuf>,

    /// Ledger the stranded `sync_queue` entry payloads, then delete them (§5.3).
    #[arg(long)]
    pub invalidate_sync_queue: bool,

    /// Spec §6: rebuild the knowledge-page FTS index from the on-disk bodies
    /// and verify every page is retrievable. Runs with or without --apply, so
    /// the cutover runbook can invoke it as its own step.
    #[arg(long)]
    pub reindex: bool,

    /// Rows read per extraction page.
    #[arg(long, default_value_t = 500)]
    pub page_size: usize,

    /// Undo a previous apply, driven by the ledger (M5 rollback).
    ///
    /// Report-only unless `--apply` is also given, exactly like the migration
    /// itself. Only pages this migration's ledger records are considered, and a
    /// page whose `rel_path` no longer matches the ledger is reported and left
    /// alone rather than deleted.
    #[arg(long, conflicts_with = "reindex")]
    pub rollback: bool,
}

/// Resolve which databases this invocation reads and writes.
///
/// Split out of [`execute`] so the rehearsal safety rail below is testable
/// without running a migration.
///
/// The rail: `--project-root` means "do not touch the detected root", which is
/// only ever passed when rehearsing against a copy. Defaulting the *global*
/// side to `~/.cas` in that state would read the live global database and write
/// pages back into it — the exact live-data mutation the flag exists to
/// prevent. So a redirected project root requires the global side to be stated
/// too, or excluded with `--scope project`.
fn resolve_sources(
    args: &MemoryMigrateArgs,
    cas_root: &Path,
    home_dir: Option<PathBuf>,
) -> anyhow::Result<(Vec<SourceDb>, Vec<String>)> {
    let project_root = args
        .project_root
        .clone()
        .unwrap_or_else(|| cas_root.to_path_buf());
    let mut sources = Vec::new();
    let mut notes = Vec::new();

    if args.scope != "global" {
        sources.push(SourceDb {
            label: DbLabel::Project,
            db_path: project_root.join("cas.db"),
            cas_root: project_root.clone(),
        });
    }

    if args.scope != "project" {
        if args.project_root.is_some() && args.global_root.is_none() {
            anyhow::bail!(
                "--project-root redirects the project side to {}, but the global side would \
                 still default to ~/.cas — the LIVE global database, which --apply would write \
                 pages into. Pass --global-root <copy> as well, or --scope project.",
                project_root.display()
            );
        }
        let global_root = args
            .global_root
            .clone()
            .or_else(|| home_dir.map(|home| home.join(".cas")));
        match global_root {
            // Compared against `project_root`, not the detected root: when the
            // project root *is* ~/.cas the same database must not be migrated
            // twice under two labels.
            Some(root) if root.join("cas.db").exists() && root != project_root => {
                sources.push(SourceDb {
                    label: DbLabel::Global,
                    db_path: root.join("cas.db"),
                    cas_root: root,
                });
            }
            Some(root) => {
                notes.push(format!(
                    "(no separate global legacy database at {} — skipping)",
                    root.display()
                ));
            }
            None => notes
                .push("(cannot resolve a home directory — skipping the global database)".into()),
        }
    }

    Ok((sources, notes))
}

pub fn execute(args: &MemoryMigrateArgs, cas_root: &Path) -> anyhow::Result<()> {
    let (sources, notes) = resolve_sources(args, cas_root, dirs::home_dir())?;
    for note in &notes {
        println!("{note}");
    }

    let config = MigrationConfig {
        sources,
        // Defaults under the *resolved* project root, not the detected one: a
        // rehearsal against a copy must not leave its ledger in the live tree,
        // where a later real run would read it as "already migrated".
        ledger_dir: args.ledger.clone().unwrap_or_else(|| {
            args.project_root
                .clone()
                .unwrap_or_else(|| cas_root.to_path_buf())
                .join(DEFAULT_LEDGER_SUBDIR)
        }),
        apply: args.apply,
        invalidate_sync_queue: args.invalidate_sync_queue,
        page_size: args.page_size,
        reindex: args.reindex,
        stop_after: None,
    };

    // Say out loud which databases are about to be read and written. This
    // command reads a legacy store and writes pages back into the same Cassy
    // root, so a mis-resolved root is a mutation of live data, not a bad
    // report — the operator gets to see the paths before that happens.
    println!(
        "{} — sources:",
        match (args.rollback, config.apply) {
            (true, true) => "ROLLBACK",
            (true, false) => "ROLLBACK PLAN",
            (false, true) => "APPLY",
            (false, false) => "DRY RUN",
        }
    );
    for db in &config.sources {
        println!(
            "  {:<8} read {}  ->  pages into {}",
            db.label.as_str(),
            db.db_path.display(),
            db.cas_root.display()
        );
    }
    println!();

    if args.rollback {
        // Rollback never re-reads the legacy corpus: it undoes exactly what the
        // ledger says was written, so a source database that has moved on since
        // the apply cannot change what gets removed.
        let audit = memory_migration::rollback(&config.sources, &config.ledger_dir, config.apply)?;
        print!("{}", audit.render(config.apply));
        println!("ledger: {}", config.ledger_dir.display());
        if !audit.is_clean() {
            anyhow::bail!(
                "rollback did not account for every ledgered page — {} diverged, {} of {} \
                 resolved. Nothing further was attempted; resolve by hand.",
                audit.diverged.len(),
                audit.removed + audit.already_absent,
                audit.ledgered
            );
        }
        if !config.apply {
            println!("ROLLBACK PLAN — nothing was deleted. Re-run with --apply to execute.");
        }
        return Ok(());
    }

    let outcome = memory_migration::run(&config)?;

    println!("{}", outcome.audit.render_table());
    println!(
        "{}",
        memory_migration::render_quarantine(&outcome.quarantine)
    );

    for report in &outcome.index_reports {
        print!("{}", report.render());
    }
    if outcome.sync_queue_invalidated > 0 {
        println!(
            "sync_queue: {} stranded entry row(s) ledgered in full, then deleted (spec §5.3) \
             — payloads are in {}/{}",
            outcome.sync_queue_invalidated,
            outcome.ledger_dir.display(),
            memory_migration::ledger::SYNC_QUEUE_FILE
        );
    }
    if outcome.sync_queue_pending > 0 {
        println!(
            "sync_queue: {} stranded entry row(s) — drain with `cas cloud sync` or pass \
             --invalidate-sync-queue (spec §5.3)",
            outcome.sync_queue_pending
        );
    }
    println!("ledger: {}", outcome.ledger_dir.display());

    if config.apply {
        println!(
            "applied {} page(s); {} already migrated by an earlier run",
            outcome.applied, outcome.skipped_already_applied
        );
        if outcome.stopped_early {
            println!("stopped early — re-run to continue from the ledger");
        }
        if !args.reindex {
            // §6 is owned by this command; only its execution may be deferred.
            println!(
                "NEXT: run `cas memory-migrate --reindex` to rebuild the knowledge-page \
                 index from the on-disk bodies and verify every page is retrievable \
                 (spec §6). The Tantivy index is deliberately never touched: it holds no \
                 knowledge-page documents, and `SearchIndex::open` would delete and \
                 recreate the directory the surviving stay-entry rows still depend on."
            );
        }
    } else {
        println!(
            "DRY RUN — nothing was written to the knowledge store. Review the quarantine \
             list above, then re-run with --apply."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(scope: &str) -> MemoryMigrateArgs {
        MemoryMigrateArgs {
            apply: false,
            scope: scope.to_string(),
            project_root: None,
            global_root: None,
            ledger: None,
            invalidate_sync_queue: false,
            reindex: false,
            page_size: 500,
            rollback: false,
        }
    }

    fn root_with_db(dir: &Path, name: &str) -> PathBuf {
        let root = dir.join(name);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("cas.db"), b"").unwrap();
        root
    }

    #[test]
    fn a_redirected_project_root_refuses_to_default_the_global_side_to_live() {
        let temp = tempfile::TempDir::new().unwrap();
        let copy = root_with_db(temp.path(), "copy");
        let home = temp.path().join("home");
        let mut a = args("both");
        a.project_root = Some(copy);

        let err = resolve_sources(&a, temp.path(), Some(home))
            .unwrap_err()
            .to_string();
        assert!(err.contains("--global-root"), "{err}");
        assert!(err.contains("--scope project"), "{err}");
    }

    #[test]
    fn a_redirected_project_root_is_fine_once_the_global_side_is_stated() {
        let temp = tempfile::TempDir::new().unwrap();
        let project = root_with_db(temp.path(), "copy-project");
        let global = root_with_db(temp.path(), "copy-global");
        let mut a = args("both");
        a.project_root = Some(project.clone());
        a.global_root = Some(global.clone());

        let (sources, _) =
            resolve_sources(&a, temp.path(), Some(temp.path().join("home"))).unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].cas_root, project);
        assert_eq!(sources[1].cas_root, global);
        // Every source points inside the copies; the detected root is untouched.
        assert!(sources.iter().all(|s| s.db_path.starts_with(temp.path())));
    }

    #[test]
    fn a_redirected_project_root_alone_is_allowed_when_global_is_out_of_scope() {
        let temp = tempfile::TempDir::new().unwrap();
        let project = root_with_db(temp.path(), "copy-project");
        let mut a = args("project");
        a.project_root = Some(project.clone());

        let (sources, _) =
            resolve_sources(&a, temp.path(), Some(temp.path().join("home"))).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].cas_root, project);
    }

    #[test]
    fn the_same_root_is_never_migrated_twice_under_two_labels() {
        // Running inside ~/.cas itself: project detection and the global
        // default resolve to one database. Migrating it twice would double
        // every count in the loss audit.
        let temp = tempfile::TempDir::new().unwrap();
        let home = temp.path().join("home");
        let shared = root_with_db(&home, ".cas");

        let (sources, notes) = resolve_sources(&args("both"), &shared, Some(home)).unwrap();
        assert_eq!(sources.len(), 1, "{sources:?}");
        assert_eq!(sources[0].label.as_str(), "project");
        assert!(notes.iter().any(|n| n.contains("skipping")), "{notes:?}");
    }

    #[test]
    fn a_missing_global_database_is_skipped_not_invented() {
        let temp = tempfile::TempDir::new().unwrap();
        let project = root_with_db(temp.path(), "project");
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();

        let (sources, notes) = resolve_sources(&args("both"), &project, Some(home)).unwrap();
        assert_eq!(sources.len(), 1);
        assert!(notes.iter().any(|n| n.contains("skipping")), "{notes:?}");
    }
}
