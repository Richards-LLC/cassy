//! `cas knowledge` — drive and inspect the distilled project wiki
//! (EPIC cas-7d31 / cas-c9be).

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};

use crate::knowledge::{
    ClaudeCliRunner, DistillConfig, LlmRunner, ScriptedLlm, SymbolLite, collect_sources,
    run_distillation,
};
use cas_store::{KnowledgeStore, SqliteKnowledgeStore};

#[derive(Debug, Clone, Subcommand)]
pub enum KnowledgeCommands {
    /// Distill changed sources into the project knowledge wiki
    Build(BuildArgs),
    /// Show ledger and page counts without distilling anything
    Status,
    /// List distilled pages
    List(ListArgs),
}

#[derive(Debug, Clone, Args)]
pub struct BuildArgs {
    /// Model passed to the provider CLI (e.g. `haiku`, `sonnet`)
    #[arg(long)]
    pub model: Option<String>,

    /// Plan the pass without calling a model: reports what would be distilled
    #[arg(long)]
    pub dry_run: bool,

    /// Maximum sources distilled in this pass (cost guard)
    #[arg(long, default_value_t = 25)]
    pub max_sources: usize,

    /// Maximum indexed symbols loaded when seeding code module summaries
    #[arg(long, default_value_t = 5_000)]
    pub max_symbols: usize,
}

#[derive(Debug, Clone, Args)]
pub struct ListArgs {
    /// Maximum pages to show
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
}

pub fn execute(
    command: &KnowledgeCommands,
    _cli: &crate::cli::Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    match command {
        KnowledgeCommands::Build(args) => execute_build(args, cas_root),
        KnowledgeCommands::Status => execute_status(cas_root),
        KnowledgeCommands::List(args) => execute_list(args, cas_root),
    }
}

/// `.cas` lives inside the project, so the project root is its parent.
fn project_root_of(cas_root: &Path) -> anyhow::Result<PathBuf> {
    cas_root
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("cannot resolve project root from {}", cas_root.display()))
}

/// Load indexed symbols to seed code-module sources. A missing or empty code
/// index is not an error — the pass just has no module summaries.
fn load_symbols(cas_root: &Path, limit: usize) -> Vec<SymbolLite> {
    let Ok(store) = crate::store::open_code_store(cas_root) else {
        return Vec::new();
    };
    store
        .search_symbols("%", None, None, limit)
        .unwrap_or_default()
        .into_iter()
        .map(|symbol| SymbolLite {
            file_path: symbol.file_path,
            name: symbol.name,
            kind: format!("{:?}", symbol.kind).to_lowercase(),
            signature: symbol.signature,
            doc: symbol.documentation,
        })
        .collect()
}

fn execute_build(args: &BuildArgs, cas_root: &Path) -> anyhow::Result<()> {
    let project_root = project_root_of(cas_root)?;
    let store = SqliteKnowledgeStore::open(cas_root)?;
    let symbols = load_symbols(cas_root, args.max_symbols);
    let sources = collect_sources(&project_root, &symbols);

    let config = DistillConfig {
        max_sources_per_pass: args.max_sources,
        dry_run: args.dry_run,
        ..DistillConfig::default()
    };

    // A dry run classifies for real but never prompts, so the runner it is
    // handed is one that would refuse to answer — proof it is never used.
    let runner: Box<dyn LlmRunner> = if args.dry_run {
        Box::new(ScriptedLlm::new(Vec::new()))
    } else {
        Box::new(ClaudeCliRunner::new(args.model.clone()))
    };

    let report = run_distillation(&store, runner.as_ref(), &sources, &config)?;

    if args.dry_run {
        println!("Knowledge distillation (dry run — nothing was written)");
        println!("  sources scanned:    {}", report.sources_scanned);
        println!("  unchanged (skipped):{}", report.sources_skipped);
        println!("  would distill:      {}", report.sources_pending);
        for error in &report.errors {
            println!("  {error}");
        }
        return Ok(());
    }

    println!("Knowledge distillation ({})", runner.label());
    println!("  sources scanned:    {}", report.sources_scanned);
    println!("  unchanged (skipped):{}", report.sources_skipped);
    println!("  distilled:          {}", report.sources_distilled);
    println!("  failed:             {}", report.sources_failed);
    if report.sources_deferred > 0 {
        println!(
            "  deferred to next pass: {} (raise --max-sources)",
            report.sources_deferred
        );
    }
    println!("  pages written:      {}", report.pages_written);
    println!("  pages locked (kept):{}", report.pages_locked_skipped);
    println!("  sources tombstoned: {}", report.sources_tombstoned);
    println!("  pages cascade-deleted: {}", report.pages_cascade_deleted);
    if report.wikilinks_rewritten > 0 {
        println!("  dangling wikilinks fixed: {}", report.wikilinks_rewritten);
    }
    println!(
        "  merge tiers: union-only {} / rewrite {} / append {}",
        report.tier_union_only, report.tier_full_rewrite, report.tier_append_delta
    );
    println!("  llm calls:          {}", report.llm_calls);

    for error in report.errors.iter().take(10) {
        eprintln!("  ! {error}");
    }
    if report.errors.len() > 10 {
        eprintln!("  ! ... and {} more", report.errors.len() - 10);
    }

    Ok(())
}

fn execute_status(cas_root: &Path) -> anyhow::Result<()> {
    let store = SqliteKnowledgeStore::open(cas_root)?;
    let pages = store.list_pages()?;
    let ledger = store.list_sources()?;
    let locked = pages.iter().filter(|page| page.locked).count();

    println!("Knowledge store: {}", store.knowledge_dir().display());
    println!("  pages:   {} ({locked} locked)", pages.len());
    println!("  sources: {}", ledger.len());
    for status in [
        cas_store::SourceStatus::Ingested,
        cas_store::SourceStatus::Uploaded,
        cas_store::SourceStatus::Failed,
    ] {
        let count = ledger.iter().filter(|row| row.status == status).count();
        if count > 0 {
            println!("    {}: {count}", status.as_str());
        }
    }
    Ok(())
}

fn execute_list(args: &ListArgs, cas_root: &Path) -> anyhow::Result<()> {
    let store = SqliteKnowledgeStore::open(cas_root)?;
    let pages = store.list_pages()?;
    if pages.is_empty() {
        println!("No distilled pages yet. Run `cas knowledge build`.");
        return Ok(());
    }
    for page in pages.iter().take(args.limit) {
        println!(
            "{} {:<12} {} {}",
            if page.locked { "🔒" } else { "  " },
            page.page_type,
            page.rel_path,
            page.title
        );
    }
    if pages.len() > args.limit {
        println!("... and {} more", pages.len() - args.limit);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_root_is_the_parent_of_cas_root() {
        let root = project_root_of(Path::new("/repo/.cas")).expect("root");
        assert_eq!(root, PathBuf::from("/repo"));
    }

    #[test]
    fn project_root_rejects_a_bare_root_path() {
        assert!(project_root_of(Path::new("/")).is_err());
    }

    #[test]
    fn missing_code_index_yields_no_module_sources() {
        let dir = tempfile::tempdir().expect("tempdir");
        // No code store initialized: the loader degrades to an empty list
        // rather than failing the whole pass.
        let symbols = load_symbols(&dir.path().join("nonexistent"), 10);
        assert!(symbols.is_empty());
    }
}
