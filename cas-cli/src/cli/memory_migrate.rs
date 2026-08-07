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

/// Ledger location relative to the project CAS root.
const DEFAULT_LEDGER_SUBDIR: &str = "migration/cas-b129";

#[derive(Debug, Clone, Args)]
pub struct MemoryMigrateArgs {
    /// Actually write pages. Without this the command is report-only.
    #[arg(long)]
    pub apply: bool,

    /// Which legacy databases to migrate.
    #[arg(long, value_parser = ["project", "global", "both"], default_value = "both")]
    pub scope: String,

    /// Override the project CAS root (default: the detected one).
    ///
    /// `cas_root` detection honours `CAS_ROOT` *before* the working directory
    /// (`store/detect.rs:53`), so `cd`-ing into a copy of a `.cas` tree does
    /// NOT retarget this command — under a factory worker, `CAS_ROOT` points at
    /// the live database. Rehearsing the migration on a copy therefore requires
    /// this flag, and the banner below prints the resolved paths before any
    /// work starts.
    #[arg(long)]
    pub project_root: Option<PathBuf>,

    /// Override the global CAS root (default: `~/.cas`).
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
}

pub fn execute(args: &MemoryMigrateArgs, cas_root: &Path) -> anyhow::Result<()> {
    let project_root = args
        .project_root
        .clone()
        .unwrap_or_else(|| cas_root.to_path_buf());
    let mut sources = Vec::new();
    if args.scope != "global" {
        sources.push(SourceDb {
            label: DbLabel::Project,
            db_path: project_root.join("cas.db"),
            cas_root: project_root,
        });
    }
    if args.scope != "project" {
        let global_root = args
            .global_root
            .clone()
            .or_else(|| dirs::home_dir().map(|home| home.join(".cas")));
        match global_root {
            Some(root) if root.join("cas.db").exists() && root != cas_root => {
                sources.push(SourceDb {
                    label: DbLabel::Global,
                    db_path: root.join("cas.db"),
                    cas_root: root,
                });
            }
            Some(root) => {
                println!(
                    "(no global legacy database at {} — skipping)",
                    root.display()
                );
            }
            None => println!("(cannot resolve a home directory — skipping the global database)"),
        }
    }

    let config = MigrationConfig {
        sources,
        ledger_dir: args
            .ledger
            .clone()
            .unwrap_or_else(|| cas_root.join(DEFAULT_LEDGER_SUBDIR)),
        apply: args.apply,
        invalidate_sync_queue: args.invalidate_sync_queue,
        page_size: args.page_size,
        reindex: args.reindex,
        stop_after: None,
    };

    // Say out loud which databases are about to be read and written. This
    // command reads a legacy store and writes pages back into the same CAS
    // root, so a mis-resolved root is a mutation of live data, not a bad
    // report — the operator gets to see the paths before that happens.
    println!(
        "{} — sources:",
        if config.apply { "APPLY" } else { "DRY RUN" }
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

    let outcome = memory_migration::run(&config)?;

    println!("{}", outcome.audit.render_table());
    println!(
        "{}",
        memory_migration::render_quarantine(&outcome.quarantine)
    );

    for report in &outcome.index_reports {
        print!("{}", report.render());
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
