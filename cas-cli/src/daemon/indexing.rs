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

    let indexer = match BackgroundIndexer::open(&config.cas_root) {
        Ok(indexer) => indexer,
        Err(error) => {
            return Ok(crate::hybrid_search::IndexingResult {
                indexed: 0,
                errors: vec![("index".to_string(), error.to_string())],
            });
        }
    };

    let index_config = IndexingConfig {
        batch_size: config.index_batch_size,
        max_per_run: config.index_max_per_run,
    };

    indexer.process_pending(store.as_ref(), &index_config)
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
fn head_commit(repo_root: &Path) -> Option<String> {
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

        let content = match std::fs::read_to_string(file_path) {
            Ok(content) => content,
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
                let previous_symbol_ids: Vec<String> = code_store
                    .get_symbols_in_file(&file_id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|symbol| symbol.id)
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
                        .into_iter()
                        .filter(|id| !surviving.contains(id.as_str())),
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
    if let Err(error) = publish_code_symbols(cas_root, &published, &retired) {
        result.errors.push(format!("code search index: {error}"));
    }

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
                eprintln!("[CAS] Watcher error: {message}");
                result.errors.push(format!("Watcher: {message}"));
            }
        }
    }

    if !deleted_paths.is_empty() {
        if let Ok(code_store) = open_code_store(cas_root) {
            let mut retired: Vec<String> = Vec::new();
            for path in &deleted_paths {
                // Same work-tree-root derivation the writer uses; a mismatch here would look up
                // a repository that was never written and silently leave the rows behind.
                let (_repo_root, repo_name) = resolve_repository(path);

                let path_str = path.to_string_lossy();
                if let Ok(Some(file)) = code_store.get_file_by_path(&repo_name, &path_str) {
                    retired.extend(
                        code_store
                            .get_symbols_in_file(&file.id)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|symbol| symbol.id),
                    );
                    if code_store.delete_file(&file.id).is_ok() {
                        result.files_deleted += 1;
                    }
                }
            }

            if let Err(error) = publish_code_symbols(cas_root, &[], &retired) {
                result.errors.push(format!("code search index: {error}"));
            }
        }
    }

    let pending_files = watcher.take_pending();
    if !pending_files.is_empty() {
        let index_result = index_code_files(&pending_files, cas_root)?;
        result.files_indexed = index_result.files_indexed;
        result.symbols_indexed = index_result.symbols_indexed;
        result.errors.extend(index_result.errors);
    }

    Ok(result)
}
