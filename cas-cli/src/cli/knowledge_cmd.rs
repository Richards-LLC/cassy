//! `cas knowledge` — drive and inspect the distilled project wiki
//! (EPIC cas-7d31 / cas-c9be).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};

use crate::knowledge::{
    ClaudeCliRunner, DistillConfig, LlmRunner, ScriptedLlm, SymbolLite, scan_sources,
    run_distillation_until,
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
    /// Full-text search across distilled pages
    Search(SearchArgs),
    /// Print one distilled page (metadata + markdown body)
    Read(ReadArgs),
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
    #[arg(long, default_value_t = DEFAULT_MAX_SYMBOLS)]
    pub max_symbols: usize,

    /// Maximum wall-clock time for the complete knowledge build
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECS, value_parser = parse_timeout_secs)]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Args)]
pub struct ListArgs {
    /// Maximum pages to show
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
}

#[derive(Debug, Clone, Args)]
pub struct SearchArgs {
    /// Free-text query. Any term may match, ranked by relevance; wrap words in
    /// double quotes to require them adjacent as a phrase.
    pub query: Vec<String>,

    /// Maximum hits to show
    #[arg(long, default_value_t = 10)]
    pub limit: usize,
}

#[derive(Debug, Clone, Args)]
pub struct ReadArgs {
    /// Page ID (`cas-kn007`) or path relative to the knowledge dir
    /// (`subsystem/hooks.md`)
    pub target: String,
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
        KnowledgeCommands::Search(args) => execute_search(args, cas_root),
        KnowledgeCommands::Read(args) => execute_read(args, cas_root),
    }
}

/// `.cas` lives inside the project, so the project root is its parent.
fn project_root_of(cas_root: &Path) -> anyhow::Result<PathBuf> {
    cas_root
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("cannot resolve project root from {}", cas_root.display()))
}

/// How many symbols to pull per query while paging the code index.
const SYMBOL_PAGE: usize = 2_000;

/// Default ceiling on symbols loaded to seed `code://` module sources.
pub const DEFAULT_MAX_SYMBOLS: usize = 5_000;

/// Hard ceiling for a complete knowledge build. This is a CLI contract rather
/// than a shell convention, so every platform gets the same bound and
/// process-group cleanup implemented by the Rust runner.
pub const DEFAULT_TIMEOUT_SECS: u64 = 90;

fn parse_timeout_secs(value: &str) -> Result<u64, String> {
    let seconds = value.parse::<u64>().map_err(|_| {
        format!("timeout must be an integer from 1 to {DEFAULT_TIMEOUT_SECS} seconds")
    })?;
    if seconds == 0 || seconds > DEFAULT_TIMEOUT_SECS {
        return Err(format!(
            "timeout must be between 1 and {DEFAULT_TIMEOUT_SECS} seconds"
        ));
    }
    Ok(seconds)
}

/// What a symbol load produced, and whether it saw the whole index.
pub struct SymbolLoad {
    pub symbols: Vec<SymbolLite>,
    /// True when the load stopped at `limit` rather than at the end of the
    /// index. The module source set is then a partial view of the code, and
    /// acting on it as if it were complete would tombstone real modules.
    pub truncated: bool,
}

/// Load indexed symbols to seed code-module sources. A missing or empty code
/// index is not an error — the pass just has no module summaries.
///
/// The load is paged rather than a single `LIMIT`, because `search_symbols`
/// orders by name: a bare limit returns the alphabetically-first N symbols
/// repo-wide, so whole modules would drop out of the source set (and be
/// cascade-deleted) whenever an early-sorting symbol was added.
pub fn load_symbols(cas_root: &Path, limit: usize) -> SymbolLoad {
    let Ok(store) = crate::store::open_code_store(cas_root) else {
        return SymbolLoad {
            symbols: Vec::new(),
            truncated: false,
        };
    };

    let mut symbols = Vec::new();
    let mut offset = 0usize;
    let mut truncated = false;

    while symbols.len() < limit {
        let want = SYMBOL_PAGE.min(limit - symbols.len());
        let page = store
            .search_symbols_paginated("%", None, None, want, offset)
            .unwrap_or_default();
        let received = page.len();

        symbols.extend(page.into_iter().map(|symbol| SymbolLite {
            file_path: symbol.file_path,
            name: symbol.name,
            kind: format!("{:?}", symbol.kind).to_lowercase(),
            signature: symbol.signature,
            doc: symbol.documentation,
        }));

        if received < want {
            break; // index exhausted
        }
        offset += received;
        if symbols.len() >= limit {
            // We stopped because of the cap, not because the index ran out.
            truncated = !store
                .search_symbols_paginated("%", None, None, 1, offset)
                .unwrap_or_default()
                .is_empty();
        }
    }

    SymbolLoad { symbols, truncated }
}

fn execute_build(args: &BuildArgs, cas_root: &Path) -> anyhow::Result<()> {
    let build_deadline = Instant::now() + Duration::from_secs(args.timeout_secs);
    let project_root = project_root_of(cas_root)?;
    let store = SqliteKnowledgeStore::open(cas_root)?;
    let load = load_symbols(cas_root, args.max_symbols);
    let scan = scan_sources(&project_root, &load.symbols);
    // Taken before the sources are moved out: a file that could not be decoded
    // is reported with the pass, not dropped on the floor (cas-c736).
    let skip_notes = scan.skip_notes();
    let sources = scan.sources;

    let config = DistillConfig {
        max_sources_per_pass: args.max_sources,
        dry_run: args.dry_run,
        // A truncated symbol load means the `code://` source set is a partial
        // view of the code. Protect those ledger rows so "I could not see it"
        // is not mistaken for "it was deleted".
        protected_prefixes: if load.truncated {
            vec![crate::knowledge::sources::CODE_MODULE_SCHEME.to_string()]
        } else {
            Vec::new()
        },
        ..DistillConfig::default()
    };

    if load.truncated {
        eprintln!(
            "[Cassy] Only the first {} indexed symbols were loaded; code module pages are left untouched this pass (raise --max-symbols).",
            args.max_symbols
        );
    }

    // A dry run classifies for real but never prompts, so the runner it is
    // handed is one that would refuse to answer — proof it is never used.
    let runner: Box<dyn LlmRunner> = if args.dry_run {
        Box::new(ScriptedLlm::new(Vec::new()))
    } else {
        Box::new(
            ClaudeCliRunner::new(args.model.clone())
                .with_timeout(Duration::from_secs(args.timeout_secs)),
        )
    };

    let mut report =
        run_distillation_until(&store, runner.as_ref(), &sources, &config, build_deadline)?;
    report.notes.extend(skip_notes);

    if args.dry_run {
        println!("Knowledge distillation (dry run — nothing was written)");
        println!("  sources scanned:    {}", report.sources_scanned);
        println!("  unchanged (skipped):{}", report.sources_skipped);
        println!("  would distill:      {}", report.sources_pending);
        for note in &report.notes {
            println!("  {note}");
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

    for note in &report.notes {
        println!("  note: {note}");
    }
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

fn execute_search(args: &SearchArgs, cas_root: &Path) -> anyhow::Result<()> {
    let query = args.query.join(" ");
    if query.trim().is_empty() {
        anyhow::bail!("nothing to search for — pass a query, e.g. `cas knowledge search hooks`");
    }

    let store = SqliteKnowledgeStore::open(cas_root)?;
    let hits = store.search(&query, args.limit.max(1))?;
    if hits.is_empty() {
        println!("No distilled pages match '{query}'.");
        return Ok(());
    }

    println!("{} page(s) matching '{query}':", hits.len());
    for hit in &hits {
        println!(
            "{} {} [{}] {}",
            if hit.page.locked { "🔒" } else { "  " },
            hit.page.rel_path,
            hit.page.id,
            hit.page.title
        );
        if !hit.page.snippet.is_empty() {
            println!("    {}", hit.page.snippet);
        }
    }
    println!("\nRead one with: cas knowledge read <id-or-path>");
    Ok(())
}

fn execute_read(args: &ReadArgs, cas_root: &Path) -> anyhow::Result<()> {
    let store = SqliteKnowledgeStore::open(cas_root)?;

    // `target` accepts either identity, so a path copied straight out of
    // `search` output works without a flag.
    let page = match store.get_page_by_rel_path(&args.target)? {
        Some(page) => page,
        None => store
            .get_page(&args.target)
            .map_err(|_| anyhow::anyhow!("no knowledge page '{}'", args.target))?,
    };

    let body = store.read_body(&page.rel_path)?;
    println!("# {} [{}]", page.title, page.id);
    println!("type:    {}", page.page_type);
    println!("path:    {}", page.rel_path);
    println!("locked:  {}", page.locked);
    println!(
        "sources: {}",
        if page.sources.is_empty() {
            "(none)".to_string()
        } else {
            page.sources.join(", ")
        }
    );
    println!("updated: {}", page.updated_at.to_rfc3339());
    println!();
    println!("{body}");
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
        let load = load_symbols(&dir.path().join("nonexistent"), 10);
        assert!(load.symbols.is_empty());
        assert!(!load.truncated, "an absent index is complete, not truncated");
    }

    #[test]
    fn knowledge_build_cli_accepts_only_a_positive_timeout_at_or_below_ninety() {
        for value in ["0", "91", "18446744073709551615", "not-a-duration"] {
            assert!(
                crate::cli::try_parse_from_with_wordmark([
                    "cas",
                    "knowledge",
                    "build",
                    "--timeout-secs",
                    value,
                ])
                .is_err(),
                "invalid timeout {value} must be rejected by clap"
            );
        }
        assert!(
            crate::cli::try_parse_from_with_wordmark([
                "cas",
                "knowledge",
                "build",
                "--timeout-secs",
                "90",
            ])
            .is_ok()
        );

        let parsed = crate::cli::try_parse_from_with_wordmark(["cas", "knowledge", "build"])
            .expect("the default timeout must be valid");
        match parsed.command {
            Some(crate::cli::Commands::Knowledge(KnowledgeCommands::Build(args))) => {
                assert_eq!(args.timeout_secs, DEFAULT_TIMEOUT_SECS);
            }
            _ => panic!("expected knowledge build command"),
        }
    }
}
