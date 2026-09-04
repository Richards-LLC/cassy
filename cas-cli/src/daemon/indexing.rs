use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::daemon::{CodeIndexResult, CodeWatcher, DaemonConfig, EmbeddingResult, WatchEvent};
use crate::error::CasError;
use crate::store::Store;

/// Run embedding-only maintenance cycle (no-op - daemon removed).
pub fn run_embedding_cycle(_config: &DaemonConfig) -> Result<EmbeddingResult, CasError> {
    Ok(EmbeddingResult::default())
}

pub(crate) fn generate_bm25_index(
    store: &Arc<dyn Store>,
    config: &DaemonConfig,
) -> Result<crate::hybrid_search::IndexingResult, CasError> {
    use crate::hybrid_search::{BackgroundIndexer, IndexingConfig};

    // Legacy-root repair is BEST EFFORT, per cycle. It must never short-circuit
    // the cycle: a persistently un-retirable root (malformed `.managed.json`,
    // EPERM, a writer lock held by a pre-fix daemon) used to return early and
    // so permanently disabled ALL background memory indexing, with no self-heal
    // short of hand-deleting `.cas/index/meta.json` (cas-25a9 P1-B). Failures
    // are recorded and the cycle continues into `process_pending`.
    let mut repair_errors: Vec<(String, String)> = Vec::new();
    let mut repaired = 0usize;
    match crate::hybrid_search::repair_legacy_index_bounded(
        &config.cas_root,
        // Bounded at the CALL SITE as well as inside: the inner probe cannot
        // close the window where another process takes META_LOCK between the
        // probe and `Index::reader()`, and tantivy 0.25 offers no timeout on
        // the reader's own acquisition. Running repair on its own thread and
        // abandoning it after a budget means a blocked reader can never wedge
        // the maintenance select! arm (cas-25a9).
        Arc::clone(store),
        // Bounded on the daemon path: the maintenance `select!` arm also runs
        // agent reaping, lease reclaim and worktree cleanup, so a huge legacy
        // root must not hold it for a full re-index (cas-25a9 P2-C).
        crate::hybrid_search::LegacyRepairLimits {
            batch_size: config.index_batch_size,
            max_per_run: config.index_max_per_run,
        },
        crate::hybrid_search::DAEMON_REPAIR_BUDGET,
    ) {
        Ok(crate::hybrid_search::LegacyRepairOutcome::Repaired(repair)) => {
            repaired = repair.indexed_entries;
            repair_errors.extend(repair.errors);
            for (doc_type, count) in &repair.retired_non_entry_documents {
                repair_errors.push((
                    "legacy-index-repair".to_string(),
                    format!(
                        "retired {count} legacy `{doc_type}` document(s) with no re-queue path;                          reindex that document type to restore it"
                    ),
                ));
            }
            if !repair.unswept_files.is_empty() {
                repair_errors.push((
                    "legacy-index-repair".to_string(),
                    format!(
                        "{} legacy file(s) not yet swept; will resume next cycle",
                        repair.unswept_files.len()
                    ),
                ));
            }
        }
        Ok(crate::hybrid_search::LegacyRepairOutcome::NoLegacyRoot) => {}
        Ok(crate::hybrid_search::LegacyRepairOutcome::Busy { reason }) => {
            // Normal, retryable: skip this cycle's repair, keep indexing.
            repair_errors.push((
                "legacy-index-repair".to_string(),
                format!("skipped this cycle: {reason}"),
            ));
        }
        Err(error) => {
            repair_errors.push(("legacy-index-repair".to_string(), error.to_string()));
        }
    }

    let indexer = match BackgroundIndexer::open(&config.cas_root) {
        Ok(indexer) => indexer,
        Err(error) => {
            repair_errors.push(("index".to_string(), error.to_string()));
            return Ok(crate::hybrid_search::IndexingResult {
                indexed: repaired,
                errors: repair_errors,
            });
        }
    };

    let index_config = IndexingConfig {
        batch_size: config.index_batch_size,
        max_per_run: config.index_max_per_run,
    };

    let mut result = indexer.process_pending(store.as_ref(), &index_config)?;
    result.indexed += repaired;
    // Repair diagnostics ride alongside the cycle's own errors; they never
    // replace the cycle.
    result.errors.splice(0..0, repair_errors);
    Ok(result)
}

/// Run indexing-only maintenance cycle (for incremental BM25 updates).
pub fn run_indexing_cycle(
    config: &DaemonConfig,
) -> Result<crate::hybrid_search::IndexingResult, CasError> {
    use crate::store::open_store;

    if !config.index_bm25 {
        return Ok(crate::hybrid_search::IndexingResult::default());
    }

    let store = open_store(&config.cas_root)?;
    generate_bm25_index(&store, config)
}

/// Resolve the repository identity for a source file: `(work-tree root, repository name)`.
///
/// cas-499c / spec §1.1 defect 2: this used to be the *parent directory* name, so
/// `crates/cas-code/src/parser/mod.rs` and `cas-cli/src/parser/mod.rs` both landed under
/// repository = `"parser"`, which defeats the point of `UNIQUE(repository, path)` and makes
/// `get_file_by_path` collide across unrelated trees. We now walk up to the first ancestor
/// holding a `.git` entry — a directory in a normal clone, a *file* in a linked worktree or
/// submodule, hence `exists()` rather than `is_dir()` — and use that directory's name.
///
/// Outside a git tree there is nothing better to use, so the old parent-directory name is kept
/// as the fallback rather than inventing an identity.
pub fn resolve_repository(file_path: &Path) -> (Option<PathBuf>, String) {
    let mut cursor = if file_path.is_dir() {
        Some(file_path)
    } else {
        file_path.parent()
    };

    while let Some(dir) = cursor {
        if dir.join(".git").exists() {
            let name = dir
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            return (Some(dir.to_path_buf()), name);
        }
        cursor = dir.parent();
    }

    let fallback = file_path
        .parent()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    (None, fallback)
}

/// Current `HEAD` commit of a work tree, or `None` when git cannot answer
/// (no git, unborn branch, not a repository).
pub(crate) fn head_commit(repo_root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

/// Path of the code BM25 index (`<cas_root>/index/code`).
///
/// This is the directory `hybrid_search::code::open_code_search` reads through
/// `cas_search::Bm25Index`, and the one `code_search_available` probes. It is deliberately NOT
/// the main `<cas_root>/index` tantivy index: the two carry incompatible schemas, and the main
/// index is queried with an empty doc-type filter by `search_unified`, so writing tens of
/// thousands of symbol documents there would silently reshape every memory search.
pub(crate) fn code_index_dir(cas_root: &Path) -> PathBuf {
    cas_root.join("index").join("code")
}

/// Walk configured roots using gitignore plus explicit exclude globs.
/// Shared by startup reconciliation, the manual command, and doctor so all
/// three agree on the denominator called "eligible".
pub(crate) fn collect_source_files(
    roots: &[PathBuf],
    extensions: &[String],
    exclude_patterns: &[String],
) -> Vec<PathBuf> {
    let wanted: HashSet<String> = extensions.iter().map(|e| e.to_lowercase()).collect();
    let excludes: Vec<glob::Pattern> = exclude_patterns
        .iter()
        .filter_map(|pattern| glob::Pattern::new(pattern).ok())
        .collect();
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for root in roots {
        if root.is_file() {
            if is_wanted(root, &wanted) && seen.insert(root.clone()) {
                out.push(root.clone());
            }
            continue;
        }
        for entry in ignore::WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(true)
            .build()
            .flatten()
        {
            let path = entry.path();
            if !path.is_file() || !is_wanted(path, &wanted) {
                continue;
            }
            let relative = path.strip_prefix(root).unwrap_or(path);
            if excludes
                .iter()
                .any(|pattern| pattern.matches_path(relative))
            {
                continue;
            }
            let path = path.to_path_buf();
            if seen.insert(path.clone()) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn is_wanted(path: &Path, wanted: &HashSet<String>) -> bool {
    let Some(extension) = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_lowercase)
    else {
        return false;
    };

    wanted.contains(&extension)
        && !matches!(
            cas_code::Language::from_extension(&extension),
            cas_code::Language::Unknown
        )
}

/// Publish parsed symbols to the code BM25 index and retire deleted ones.
///
/// Before cas-499c nothing ever wrote this index, so `.cas/index/code` never existed,
/// `code_search_available` always returned false, and `code_search` answered every query with a
/// stub pointing at a command that did not exist either.
pub(crate) fn publish_code_symbols(
    cas_root: &Path,
    symbols: &[cas_code::CodeSymbol],
    retired_ids: &[String],
) -> Result<usize, String> {
    use cas_search::{Bm25Index, SearchDocument};

    if symbols.is_empty() && retired_ids.is_empty() {
        return Ok(0);
    }

    // Symbols stay in this isolated Bm25 index: the main memory index has an
    // incompatible schema and unfiltered searches would let symbols affect memory ranking.
    let index = Bm25Index::open(&code_index_dir(cas_root)).map_err(|e| e.to_string())?;

    if !retired_ids.is_empty() {
        index
            .delete_batch(retired_ids.iter().map(String::as_str))
            .map_err(|e| e.to_string())?;
    }

    if symbols.is_empty() {
        return Ok(0);
    }

    index
        .index_batch(symbols.iter().map(|symbol| symbol as &dyn SearchDocument))
        .map_err(|e| e.to_string())
}

/// Whether a publish error is another process holding the tantivy writer lock.
///
/// The code BM25 index is a single-writer directory, and `cas serve` caches its
/// `IndexWriter`, so any second process — a manual `cas index code`, a hook, a
/// second server — can meet a held lock. Before cas-8a03 that surfaced as
/// `failed to retire deleted source file: … LockBusy` and was counted as a
/// permanent file failure, which is how one cas-src project accumulated 592 of
/// them for files that had been deleted months earlier.
fn is_index_lock_busy(error: &str) -> bool {
    let error = error.to_lowercase();
    error.contains("lockbusy")
        || error.contains("failed to acquire index lock")
        || error.contains("failed to acquire lockfile")
}

/// How long one indexing run may spend, in total, waiting for the BM25 writer.
const WRITER_LOCK_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

/// A wait budget for BM25 writer-lock contention, shared by every retirement in
/// one indexing run.
///
/// Per-call retries would multiply: a run retiring 592 files behind a writer
/// that is held for the whole run would wait its backoff 592 times. The budget
/// is spent once — after that the run stops waiting and records the failures,
/// which the next run retries.
pub(crate) struct WriterLockBudget {
    remaining: std::time::Duration,
}

impl WriterLockBudget {
    pub(crate) fn new(total: std::time::Duration) -> Self {
        Self { remaining: total }
    }

    /// Run `attempt`, retrying while it fails on a held writer lock and budget
    /// remains. Any other error returns immediately: only contention is
    /// transient.
    pub(crate) fn run<T>(&mut self, attempt: impl Fn() -> Result<T, String>) -> Result<T, String> {
        let mut delay = std::time::Duration::from_millis(50);
        loop {
            match attempt() {
                Err(error) if is_index_lock_busy(&error) && !self.remaining.is_zero() => {
                    let wait = delay.min(self.remaining);
                    std::thread::sleep(wait);
                    self.remaining -= wait;
                    delay = (delay * 2).min(std::time::Duration::from_millis(800));
                }
                other => return other,
            }
        }
    }
}

/// Remove source-code vectors without creating the optional LMDB cache.
///
/// Indexing runs while logged out, so retirement may only open a cache that
/// already exists. This keeps delete/rename cleanup local and guarantees the
/// capability-absent path never materializes `index/code-vectors`.
fn retire_cached_code_vectors(cas_root: &Path, symbol_ids: &[String]) -> Result<(), String> {
    if symbol_ids.is_empty() {
        return Ok(());
    }
    let Some(cache) = crate::cloud::embeddings::KnowledgeVectorCache::open_existing_code(cas_root)
        .map_err(|error| error.to_string())?
    else {
        return Ok(());
    };
    for symbol_id in symbol_ids {
        cache
            .delete(&crate::cloud::embeddings::code_symbol_key(symbol_id))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Retire one deleted/renamed file from every source-code search channel.
///
/// The replayable secondary indexes are removed before SQLite source rows.
/// If any step fails, the file and its symbol ids remain available for the
/// next reconciliation pass instead of losing the only durable retirement
/// manifest. Every operation before the final SQLite delete is idempotent.
fn retire_code_file(
    cas_root: &Path,
    code_store: &dyn cas_store::CodeStore,
    file: &cas_code::CodeFile,
    budget: &mut WriterLockBudget,
) -> Result<usize, String> {
    let symbol_ids: Vec<String> = code_store
        .get_symbols_in_file(&file.id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|symbol| symbol.id)
        .collect();

    budget.run(|| publish_code_symbols(cas_root, &[], &symbol_ids))?;
    retire_cached_code_vectors(cas_root, &symbol_ids)?;
    cas_store::SqliteCodeVectorStore::open(cas_root)
        .map_err(|error| error.to_string())?
        .retire(&symbol_ids)
        .map_err(|error| error.to_string())?;
    code_store
        .delete_symbols_in_file(&file.id)
        .map_err(|error| error.to_string())?;
    code_store
        .delete_file(&file.id)
        .map_err(|error| error.to_string())?;
    Ok(symbol_ids.len())
}

/// Index changed code files (called by file watcher or periodic task).
///
/// Files whose content hash already matches the stored one are skipped.
pub fn index_code_files(files: &[PathBuf], cas_root: &Path) -> Result<CodeIndexResult, CasError> {
    index_code_files_with(files, cas_root, false)
}

/// Index code files, optionally re-parsing files whose content hash is unchanged.
///
/// `force` exists for `cas index code`: the content-hash skip is the right call for the watcher,
/// but it would make a rebuild of a lost or corrupted BM25 index a no-op, since the SQLite rows
/// it compares against would still be present.
pub fn index_code_files_with(
    files: &[PathBuf],
    cas_root: &Path,
    force: bool,
) -> Result<CodeIndexResult, CasError> {
    use cas_code::Language;
    use cas_code::parser::MultiLanguageParser;
    use sha2::{Digest, Sha256};
    use std::collections::HashMap;

    use crate::store::open_code_store;

    if files.is_empty() {
        return Ok(CodeIndexResult::default());
    }

    let code_store = open_code_store(cas_root)?;
    let vector_state = cas_store::SqliteCodeVectorStore::open(cas_root)
        .map_err(|error| CasError::Other(format!("Failed to open code vector queue: {error}")))?;

    let mut result = CodeIndexResult::default();
    let mut parser = match MultiLanguageParser::new() {
        Ok(parser) => parser,
        Err(error) => {
            result
                .errors
                .push(format!("Failed to create parser: {error}"));
            return Ok(result);
        }
    };

    // Repository identity and HEAD sha are per work-tree, not per file; resolving them once per
    // directory keeps the git shell-out off the hot path for a 2,000-file walk.
    let mut repo_cache: HashMap<PathBuf, (String, Option<String>)> = HashMap::new();
    let mut published: Vec<cas_code::CodeSymbol> = Vec::new();
    let mut retired: Vec<String> = Vec::new();

    for file_path in files {
        let extension = file_path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_lowercase())
            .unwrap_or_default();
        let language = Language::from_extension(&extension);

        if !parser.supports(language) {
            continue;
        }

        // GH #698: read bytes and decode, rather than demanding UTF-8 up front.
        // A BOM-marked UTF-16 file is ordinary source text; treating it as an
        // index failure made `cas index code` unable to ever clear the warning
        // it printed as the remedy.
        let content = match std::fs::read(file_path) {
            Ok(bytes) => match crate::daemon::source_text::decode_source(&bytes) {
                Ok(content) => content,
                Err(reason) => {
                    // Skipped, not failed: no retry of these bytes can succeed,
                    // so counting it as a failure would warn forever.
                    result
                        .skipped
                        .push((file_path.clone(), reason.as_str().to_string()));
                    continue;
                }
            },
            Err(error) => {
                result
                    .errors
                    .push(format!("{}: {}", file_path.display(), error));
                continue;
            }
        };

        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let content_hash = hex::encode(hasher.finalize());

        let dir_key = file_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let (repo_name, commit_hash) = repo_cache
            .entry(dir_key)
            .or_insert_with(|| {
                let (repo_root, repo_name) = resolve_repository(file_path);
                let commit = repo_root.as_deref().and_then(head_commit);
                (repo_name, commit)
            })
            .clone();

        if !force {
            if let Ok(Some(existing)) =
                code_store.get_file_by_path(&repo_name, &file_path.to_string_lossy())
            {
                if existing.content_hash == content_hash {
                    continue;
                }
            }
        }

        match parser.parse_file(file_path, &content, &repo_name) {
            Ok(parse_result) => {
                let now = chrono::Utc::now();
                let file_path_str = file_path.to_string_lossy().to_string();
                let file_id = code_store.generate_file_id_for(&repo_name, &file_path_str);

                // Symbols that existed in the previous parse of this file but are about to be
                // dropped must also leave the BM25 index, or a renamed/deleted function keeps
                // answering searches from a stale document.
                let previous_symbols = code_store.get_symbols_in_file(&file_id).unwrap_or_default();
                let previous_symbol_ids: Vec<String> = previous_symbols
                    .iter()
                    .map(|symbol| symbol.id.clone())
                    .collect();

                let _ = code_store.delete_symbols_in_file(&file_id);

                let file = cas_code::CodeFile {
                    id: file_id.clone(),
                    path: file_path_str,
                    repository: repo_name.clone(),
                    language,
                    size: content.len(),
                    line_count: content.lines().count(),
                    commit_hash: commit_hash.clone(),
                    content_hash,
                    created: now,
                    updated: now,
                    scope: "project".to_string(),
                };

                if let Err(error) = code_store.add_file(&file) {
                    result
                        .errors
                        .push(format!("{}: {}", file_path.display(), error));
                    continue;
                }

                let symbol_count = parse_result.symbols.len();
                let symbols: Vec<cas_code::CodeSymbol> = parse_result
                    .symbols
                    .into_iter()
                    .map(|mut symbol| {
                        symbol.file_id = file_id.clone();
                        symbol.commit_hash = commit_hash.clone();
                        symbol.id = code_store.generate_symbol_id_for(
                            &symbol.qualified_name,
                            &symbol.file_path,
                            &symbol.repository,
                        );
                        symbol
                    })
                    .collect();

                let surviving: std::collections::HashSet<&str> =
                    symbols.iter().map(|symbol| symbol.id.as_str()).collect();
                retired.extend(
                    previous_symbol_ids
                        .iter()
                        .filter(|id| !surviving.contains(id.as_str()))
                        .cloned(),
                );
                published.extend(symbols.iter().cloned());

                if let Err(_batch_err) = code_store.add_symbols_batch(&symbols) {
                    // Fall back to individual inserts on batch failure
                    for symbol in &symbols {
                        if let Err(error) = code_store.add_symbol(symbol) {
                            result
                                .errors
                                .push(format!("Symbol {}: {}", symbol.name, error));
                        }
                    }
                }

                // Re-arm only changed semantic chunks, retire symbols that
                // disappeared/became low-value, and remove cached vectors for
                // both cases so queries never observe an old vector while the
                // replacement is pending.
                let current_vectors: std::collections::HashMap<&str, &str> = symbols
                    .iter()
                    .filter(|symbol| symbol.kind.should_embed())
                    .map(|symbol| (symbol.id.as_str(), symbol.content_hash.as_str()))
                    .collect();
                let stale_vectors: Vec<String> = previous_symbols
                    .iter()
                    .filter(|symbol| {
                        current_vectors.get(symbol.id.as_str()).copied()
                            != Some(symbol.content_hash.as_str())
                    })
                    .map(|symbol| symbol.id.clone())
                    .collect();
                if let Err(error) = retire_cached_code_vectors(cas_root, &stale_vectors) {
                    result.errors.push(format!(
                        "{}: failed to retire stale code vectors: {error}",
                        file_path.display()
                    ));
                }
                if let Err(error) = vector_state.sync_file_symbols(&symbols, &previous_symbol_ids) {
                    result.errors.push(format!(
                        "{}: failed to reconcile code vector queue: {error}",
                        file_path.display()
                    ));
                }

                result.symbols_indexed += symbol_count;
                result.files_indexed += 1;
            }
            Err(error) => {
                result
                    .errors
                    .push(format!("{}: {}", file_path.display(), error));
            }
        }
    }

    // One BM25 commit for the whole batch — the singular per-symbol form commits per document.
    // A writer lock held by a concurrent `cas serve` is waited out, not turned
    // into a lost batch (cas-8a03).
    let mut writer_budget = WriterLockBudget::new(WRITER_LOCK_BUDGET);
    if let Err(error) = writer_budget.run(|| publish_code_symbols(cas_root, &published, &retired)) {
        result.errors.push(format!("code search index: {error}"));
    }
    Ok(result)
}

/// Reconcile the code-vector queue against the symbol table, recording the
/// outcome on `result`.
///
/// This is the step `cas doctor` has been telling operators to run since the
/// coverage counters landed: incremental indexing only visits files whose
/// bytes changed, so queue rows for deleted symbols, failed rows, and symbols
/// that were never queued survive every rerun (cas-8a03). Dropped rows also
/// lose their cached vector here, so a retired symbol cannot keep answering
/// semantic queries.
pub fn reconcile_code_vector_queue(cas_root: &Path, force: bool, result: &mut CodeIndexResult) {
    let store = match cas_store::SqliteCodeVectorStore::open(cas_root) {
        Ok(store) => store,
        Err(error) => {
            result
                .errors
                .push(format!("failed to open code vector queue: {error}"));
            return;
        }
    };
    let outcome = match store.reconcile(force) {
        Ok(outcome) => outcome,
        Err(error) => {
            result
                .errors
                .push(format!("failed to reconcile code vector queue: {error}"));
            return;
        }
    };
    if let Err(error) = retire_cached_code_vectors(cas_root, &outcome.dropped_symbol_ids) {
        result.errors.push(format!(
            "failed to retire cached vectors for dropped queue rows: {error}"
        ));
    }
    result.vector_reconcile = Some(outcome);
}

/// Full-tree reconciliation used on daemon startup and by `cas index code`
/// with its default root. In addition to content-hash incremental updates it
/// retires files that disappeared while the daemon was stopped and records a
/// durable coverage/HEAD receipt.
pub fn reconcile_code_tree(
    files: &[PathBuf],
    roots: &[PathBuf],
    cas_root: &Path,
    force: bool,
) -> Result<CodeIndexResult, CasError> {
    use crate::store::open_code_store;

    let mut result = index_code_files_with(files, cas_root, force)?;
    let code_store = open_code_store(cas_root)?;
    // One shared wait budget for the whole retirement sweep: see
    // [`WriterLockBudget`] — 592 files must not each wait out the same lock.
    let mut writer_budget = WriterLockBudget::new(WRITER_LOCK_BUDGET);

    let mut by_repo: std::collections::HashMap<String, (PathBuf, HashSet<String>)> =
        std::collections::HashMap::new();
    // Seed repositories from scan roots, not only from eligible files. An
    // empty eligible set is still an authoritative full-tree snapshot and
    // must retire every previously indexed row for that repository.
    for root in roots {
        let identity_probe = if root.is_dir() {
            root.join(".cas-reconciliation-root")
        } else {
            root.clone()
        };
        let (repo_root, repository) = resolve_repository(&identity_probe);
        by_repo.entry(repository).or_insert_with(|| {
            (
                repo_root.unwrap_or_else(|| {
                    if root.is_dir() {
                        root.clone()
                    } else {
                        root.parent().unwrap_or(root).to_path_buf()
                    }
                }),
                HashSet::new(),
            )
        });
    }
    // The eligible denominator must exclude files we skipped: leaving them in
    // makes `eligible - indexed` count them as failures on every scan, which is
    // exactly the permanent warning GH #698 reports.
    let skipped_paths: HashSet<PathBuf> = result
        .skipped
        .iter()
        .map(|(path, _)| path.clone())
        .collect();
    for file in files {
        if skipped_paths.contains(file) {
            continue;
        }
        let (root, repository) = resolve_repository(file);
        let file_id = code_store.generate_file_id_for(&repository, &file.to_string_lossy());
        by_repo
            .entry(repository)
            .or_insert_with(|| {
                (
                    root.unwrap_or_else(|| file.parent().unwrap_or(file).to_path_buf()),
                    HashSet::new(),
                )
            })
            .1
            .insert(file_id);
    }

    for (repository, (repo_root, current_ids)) in by_repo {
        let errors_before_retirement = result.errors.len();
        let stored = match code_store.list_files(&repository, None) {
            Ok(stored) => stored,
            Err(error) => {
                result.errors.push(format!(
                    "{repository}: failed to list indexed source files: {error}"
                ));
                Vec::new()
            }
        };
        for stale in stored.iter().filter(|file| !current_ids.contains(&file.id)) {
            match retire_code_file(cas_root, code_store.as_ref(), stale, &mut writer_budget) {
                Ok(_) => result.files_deleted += 1,
                Err(error) => result.errors.push(format!(
                    "{}: failed to retire deleted source file: {error}",
                    stale.path
                )),
            }
        }

        let current_indexed = code_store
            .list_files(&repository, None)
            .unwrap_or_default()
            .into_iter()
            .filter(|file| current_ids.contains(&file.id))
            .count();
        let retirement_errors = result.errors.len() - errors_before_retirement;
        let failed_files = current_ids
            .len()
            .saturating_sub(current_indexed)
            .saturating_add(retirement_errors);
        // Skips belong to the repository whose tree they were found in, so a
        // multi-repo scan does not report one repo's undecodable files against
        // another's coverage.
        let repo_skipped: Vec<String> = result
            .skipped
            .iter()
            .filter(|(path, _)| path.starts_with(&repo_root))
            .map(|(path, reason)| format!("{}: {reason}", path.display()))
            .collect();
        let scan_error = (!result.errors.is_empty()).then(|| {
            result
                .errors
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        });
        if let Err(error) = cas_store::SqliteCodeVectorStore::open(cas_root).and_then(|state| {
            state.record_scan(
                &repository,
                current_ids.len(),
                current_indexed,
                failed_files,
                repo_skipped.len(),
                (!repo_skipped.is_empty()).then(|| repo_skipped.join("; ")).as_deref(),
                head_commit(&repo_root).as_deref(),
                scan_error.as_deref(),
            )
        }) {
            result.errors.push(format!(
                "{repository}: failed to record code-index scan receipt: {error}"
            ));
        }
    }

    // Last, after every retirement and re-parse has landed: the queue is only
    // reconcilable against a symbol table this run has finished writing.
    reconcile_code_vector_queue(cas_root, force, &mut result);
    Ok(result)
}

/// Run code indexing cycle (for periodic background indexing).
pub fn run_code_index_cycle(
    watcher: &CodeWatcher,
    cas_root: &Path,
) -> Result<CodeIndexResult, CasError> {
    use crate::store::open_code_store;

    let mut result = CodeIndexResult::default();
    let mut deleted_paths: Vec<PathBuf> = Vec::new();

    while let Some(event) = watcher.try_recv() {
        match event {
            WatchEvent::Modified(_path) => {}
            WatchEvent::Deleted(path) => deleted_paths.push(path),
            WatchEvent::Error(message) => {
                eprintln!("[Cassy] Watcher error: {message}");
                result.errors.push(format!("Watcher: {message}"));
            }
        }
    }

    if !deleted_paths.is_empty() {
        if let Ok(code_store) = open_code_store(cas_root) {
            let mut writer_budget = WriterLockBudget::new(WRITER_LOCK_BUDGET);
            for path in &deleted_paths {
                // Same work-tree-root derivation the writer uses; a mismatch here would look up
                // a repository that was never written and silently leave the rows behind.
                let (_repo_root, repo_name) = resolve_repository(path);

                let path_str = path.to_string_lossy();
                if let Ok(Some(file)) = code_store.get_file_by_path(&repo_name, &path_str) {
                    match retire_code_file(cas_root, code_store.as_ref(), &file, &mut writer_budget)
                    {
                        Ok(_) => result.files_deleted += 1,
                        Err(error) => result.errors.push(format!(
                            "{}: failed to retire deleted source file: {error}",
                            path.display()
                        )),
                    }
                }
            }
        }
    }

    let initial_reconcile = watcher.take_initial_reconcile();
    let pending_files = watcher.take_pending();
    if initial_reconcile || !pending_files.is_empty() {
        let index_result = if initial_reconcile {
            reconcile_code_tree(&pending_files, watcher.watch_paths(), cas_root, false)?
        } else {
            index_code_files(&pending_files, cas_root)?
        };
        result.files_indexed = index_result.files_indexed;
        result.symbols_indexed = index_result.symbols_indexed;
        result.files_deleted += index_result.files_deleted;
        result.errors.extend(index_result.errors);
    }

    Ok(result)
}
