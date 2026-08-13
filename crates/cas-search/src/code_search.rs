//! Code Search Module
//!
//! This module provides semantic and pattern-based code search over indexed symbols.
//!
//! # Features
//!
//! - **Semantic search** using vector embeddings (HNSW)
//! - **Pattern search** using BM25 full-text search
//! - **Batch symbol fetching** to avoid N+1 queries
//! - **Shared embedder** - no model loading per search
//!
//! # Example
//!
//! ```rust,ignore
//! use cas_search::{CodeSearch, CodeSearchOptions};
//! use std::sync::Arc;
//!
//! let search = CodeSearch::new(store, vector_store, bm25_index, embedder);
//! let results = search.search(&CodeSearchOptions {
//!     query: "parse config file".to_string(),
//!     limit: 10,
//!     ..Default::default()
//! })?;
//! ```

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cas_code::{CodeSymbol, Language, SymbolKind};
use cas_store::CodeStore;

use crate::error::{Result, SearchError};
use crate::traits::{TextIndex, VectorStore};

/// Embedder trait for generating text embeddings
///
/// This is a local trait to avoid tight coupling with cas-embedding.
/// Implementations should delegate to the actual embedder.
pub trait Embedder: Send + Sync {
    /// Generate embedding vector for text
    fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Get embedding dimension
    fn dimension(&self) -> usize;
}

/// Result from code search
#[derive(Debug, Clone)]
pub struct CodeSearchResult {
    /// Symbol ID
    pub id: String,
    /// Fully qualified name
    pub name: String,
    /// Symbol kind (function, struct, etc.)
    pub kind: SymbolKind,
    /// Programming language
    pub language: Language,
    /// File path
    pub file_path: String,
    /// Line range
    pub line_start: usize,
    pub line_end: usize,
    /// Search score (0.0 - 1.0)
    pub score: f64,
    /// Source code (if include_source=true)
    pub source: Option<String>,
    /// Short snippet (first 3 lines)
    pub snippet: Option<String>,
    /// Documentation
    pub documentation: Option<String>,
}

impl CodeSearchResult {
    /// Create a snippet from source (first 3 lines, max 200 UTF-8 bytes).
    pub fn create_snippet(source: &str) -> String {
        const MAX_SNIPPET_BYTES: usize = 200;
        const ELLIPSIS: &str = "...";

        let lines: Vec<&str> = source.lines().take(3).collect();
        let snippet = lines.join("\n");
        if snippet.len() > MAX_SNIPPET_BYTES {
            let content_limit = MAX_SNIPPET_BYTES - ELLIPSIS.len();
            let content: String = snippet
                .chars()
                .scan(0, |bytes, character| {
                    let next = *bytes + character.len_utf8();
                    if next > content_limit {
                        None
                    } else {
                        *bytes = next;
                        Some(character)
                    }
                })
                .collect();
            format!("{content}{ELLIPSIS}")
        } else if source.lines().count() > 3 {
            format!("{snippet}{ELLIPSIS}")
        } else {
            snippet
        }
    }

    /// Create from a CodeSymbol with score
    pub fn from_symbol(symbol: CodeSymbol, score: f64, include_source: bool) -> Self {
        let snippet = Some(Self::create_snippet(&symbol.source));
        Self {
            id: symbol.id,
            name: symbol.qualified_name,
            kind: symbol.kind,
            language: symbol.language,
            file_path: symbol.file_path,
            line_start: symbol.line_start,
            line_end: symbol.line_end,
            score,
            source: if include_source {
                Some(symbol.source)
            } else {
                None
            },
            snippet,
            documentation: symbol.documentation,
        }
    }
}

/// Code search options
#[derive(Debug, Clone, Default)]
pub struct CodeSearchOptions {
    /// Search query (natural language or symbol name)
    pub query: String,
    /// Maximum results to return
    pub limit: usize,
    /// Filter by symbol kind
    pub kind: Option<SymbolKind>,
    /// Filter by language
    pub language: Option<Language>,
    /// Include full source code in results
    pub include_source: bool,
    /// Minimum score threshold (0.0 - 1.0)
    pub min_score: f32,
    /// Use semantic search (requires embedder)
    pub semantic: bool,
}

/// Code search statistics
#[derive(Debug, Clone, Default)]
pub struct CodeSearchStats {
    /// Number of indexed files
    pub file_count: usize,
    /// Number of indexed symbols
    pub symbol_count: usize,
}

/// Code search engine
///
/// Combines semantic (vector) and pattern (BM25) search for code symbols.
/// Uses shared resources to avoid repeated initialization.
pub struct CodeSearch<S: CodeStore, V: VectorStore, T: TextIndex> {
    /// Code store for symbol data
    store: Arc<S>,
    /// Vector store for semantic search
    vector_store: Arc<V>,
    /// BM25 index for pattern search
    bm25_index: Arc<T>,
    /// Embedder for query vectors (shared, not owned)
    embedder: Option<Arc<dyn Embedder>>,
}

impl<S: CodeStore, V: VectorStore, T: TextIndex> CodeSearch<S, V, T> {
    /// Create a new code search instance
    ///
    /// # Arguments
    /// * `store` - Code store for symbol data
    /// * `vector_store` - Vector store for semantic search
    /// * `bm25_index` - BM25 index for pattern search
    /// * `embedder` - Optional shared embedder for semantic search
    pub fn new(
        store: Arc<S>,
        vector_store: Arc<V>,
        bm25_index: Arc<T>,
        embedder: Option<Arc<dyn Embedder>>,
    ) -> Self {
        Self {
            store,
            vector_store,
            bm25_index,
            embedder,
        }
    }

    /// Check if semantic search is available
    pub fn has_semantic(&self) -> bool {
        self.embedder.is_some()
    }

    /// Search for code symbols
    ///
    /// Automatically chooses between semantic and pattern search based on options.
    pub fn search(&self, opts: &CodeSearchOptions) -> Result<Vec<CodeSearchResult>> {
        if opts.query.is_empty() {
            return Ok(Vec::new());
        }

        if opts.semantic && self.embedder.is_some() {
            self.semantic_search(opts)
        } else {
            self.pattern_search(opts)
        }
    }

    /// Pattern-based search using BM25
    ///
    /// Uses proper BM25 scoring from Tantivy instead of rank-based hacks.
    pub fn pattern_search(&self, opts: &CodeSearchOptions) -> Result<Vec<CodeSearchResult>> {
        if opts.query.is_empty() {
            return Ok(Vec::new());
        }

        // Search with BM25 - get proper scores
        let doc_type = "code_symbol";
        let bm25_results = self
            .bm25_index
            .search_with_type(&opts.query, doc_type, opts.limit * 3)
            .map_err(|e| SearchError::Index(e.to_string()))?;

        if bm25_results.is_empty() {
            return Ok(Vec::new());
        }

        // Batch fetch symbols (avoid N+1)
        let symbol_ids: Vec<&str> = bm25_results.iter().map(|(id, _)| id.as_str()).collect();
        let symbols = self
            .store
            .get_symbols_batch(&symbol_ids)
            .map_err(|e| SearchError::Storage(e.to_string()))?;

        // Build ID -> symbol map
        let symbol_map: HashMap<&str, &CodeSymbol> =
            symbols.iter().map(|s| (s.id.as_str(), s)).collect();

        // Build ID -> score map
        let score_map: HashMap<&str, f64> = bm25_results
            .iter()
            .map(|(id, score)| (id.as_str(), *score))
            .collect();

        // Normalize scores
        let max_score = bm25_results.iter().map(|(_, s)| *s).fold(0.0f64, f64::max);
        let normalize = |score: f64| -> f64 {
            if max_score > 0.0 {
                (score / max_score) * 0.85
            } else {
                0.0
            }
        };

        // Build results with filters
        let mut results: Vec<CodeSearchResult> = bm25_results
            .iter()
            .filter_map(|(id, _)| {
                let symbol = symbol_map.get(id.as_str())?;
                let score = score_map.get(id.as_str()).copied().unwrap_or(0.0);

                // Apply filters
                if let Some(kind_filter) = opts.kind {
                    if symbol.kind != kind_filter {
                        return None;
                    }
                }
                if let Some(lang_filter) = opts.language {
                    if symbol.language != lang_filter {
                        return None;
                    }
                }

                let normalized_score = normalize(score);
                if normalized_score < opts.min_score as f64 {
                    return None;
                }

                Some(CodeSearchResult::from_symbol(
                    (*symbol).clone(),
                    normalized_score,
                    opts.include_source,
                ))
            })
            .collect();

        // Deduplicate by name (same symbol from different paths)
        let mut seen: HashSet<String> = HashSet::new();
        results.retain(|r| seen.insert(r.name.clone()));

        // Limit results
        results.truncate(opts.limit);

        Ok(results)
    }

    /// Semantic search using vector embeddings
    ///
    /// Uses batch symbol fetching to avoid N+1 queries.
    pub fn semantic_search(&self, opts: &CodeSearchOptions) -> Result<Vec<CodeSearchResult>> {
        if opts.query.is_empty() {
            return Ok(Vec::new());
        }

        let embedder = match &self.embedder {
            Some(e) => e,
            None => return self.pattern_search(opts), // Fallback
        };

        // Generate query embedding
        let query_embedding = embedder.embed(&opts.query)?;

        // Search vector store - request more results for filtering
        let has_filters = opts.kind.is_some() || opts.language.is_some();
        let search_limit = if has_filters {
            opts.limit * 20
        } else {
            opts.limit * 5
        };

        let vector_results = self
            .vector_store
            .search(&query_embedding, search_limit)
            .map_err(|e| SearchError::Vector(e.to_string()))?;

        // Extract symbol IDs from vector keys
        // Key format: code:{symbol_id}:{chunk_type}
        let mut symbol_scores: HashMap<String, f32> = HashMap::new();

        for (key, score) in vector_results {
            if let Some(rest) = key.strip_prefix("code:") {
                if let Some(symbol_id) = rest.split(':').next() {
                    // Keep highest score per symbol (multiple chunks)
                    let entry = symbol_scores.entry(symbol_id.to_string()).or_insert(0.0);
                    if score > *entry {
                        *entry = score;
                    }
                }
            }
        }

        if symbol_scores.is_empty() {
            return self.pattern_search(opts); // Fallback to pattern search
        }

        // Batch fetch symbols (avoid N+1)
        let symbol_ids: Vec<&str> = symbol_scores.keys().map(|s| s.as_str()).collect();
        let symbols = self
            .store
            .get_symbols_batch(&symbol_ids)
            .map_err(|e| SearchError::Storage(e.to_string()))?;

        // Build results with filtering
        let query_lower = opts.query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();

        let mut results: Vec<CodeSearchResult> = symbols
            .into_iter()
            .filter_map(|symbol| {
                // Apply filters
                if let Some(kind_filter) = opts.kind {
                    if symbol.kind != kind_filter {
                        return None;
                    }
                }
                if let Some(lang_filter) = opts.language {
                    if symbol.language != lang_filter {
                        return None;
                    }
                }

                let base_score = symbol_scores.get(&symbol.id).copied().unwrap_or(0.0) as f64;

                // Apply name match boosting
                let name_lower = symbol.qualified_name.to_lowercase();
                let boost = if name_lower == query_lower {
                    0.3 // Exact match
                } else if name_lower.contains(&query_lower) {
                    0.15 // Substring match
                } else if query_words.iter().any(|w| name_lower.contains(w)) {
                    0.05 // Word match
                } else {
                    0.0
                };

                let final_score = (base_score + boost).min(1.0);
                if final_score < opts.min_score as f64 {
                    return None;
                }

                Some(CodeSearchResult::from_symbol(
                    symbol,
                    final_score,
                    opts.include_source,
                ))
            })
            .collect();

        // Sort by score
        results.sort_by(|a, b| b.score.total_cmp(&a.score));

        // Deduplicate by name
        let mut seen: HashSet<String> = HashSet::new();
        results.retain(|r| seen.insert(r.name.clone()));

        // Limit results
        results.truncate(opts.limit);

        // Supplement with pattern search if needed
        if results.len() < opts.limit && has_filters {
            let existing_ids: HashSet<String> = results.iter().map(|r| r.id.clone()).collect();

            if let Ok(pattern_results) = self.pattern_search(opts) {
                for result in pattern_results {
                    if !existing_ids.contains(&result.id) && results.len() < opts.limit {
                        results.push(result);
                    }
                }
            }
        }

        Ok(results)
    }

    /// Search by exact symbol name
    pub fn search_by_name(&self, qualified_name: &str) -> Result<Vec<CodeSearchResult>> {
        let symbols = self
            .store
            .get_symbols_by_name(qualified_name)
            .map_err(|e| SearchError::Storage(e.to_string()))?;

        Ok(symbols
            .into_iter()
            .map(|s| CodeSearchResult::from_symbol(s, 1.0, true))
            .collect())
    }

    /// Get symbols in a file
    pub fn get_file_symbols(&self, file_id: &str) -> Result<Vec<CodeSearchResult>> {
        let symbols = self
            .store
            .get_symbols_in_file(file_id)
            .map_err(|e| SearchError::Storage(e.to_string()))?;

        Ok(symbols
            .into_iter()
            .map(|s| CodeSearchResult::from_symbol(s, 1.0, true))
            .collect())
    }

    /// Get callers of a symbol
    pub fn get_callers(&self, symbol_id: &str) -> Result<Vec<CodeSearchResult>> {
        let symbols = self
            .store
            .get_callers(symbol_id)
            .map_err(|e| SearchError::Storage(e.to_string()))?;

        Ok(symbols
            .into_iter()
            .map(|s| CodeSearchResult::from_symbol(s, 1.0, true))
            .collect())
    }

    /// Get callees of a symbol
    pub fn get_callees(&self, symbol_id: &str) -> Result<Vec<CodeSearchResult>> {
        let symbols = self
            .store
            .get_callees(symbol_id)
            .map_err(|e| SearchError::Storage(e.to_string()))?;

        Ok(symbols
            .into_iter()
            .map(|s| CodeSearchResult::from_symbol(s, 1.0, true))
            .collect())
    }

    /// Get search statistics
    pub fn stats(&self) -> Result<CodeSearchStats> {
        Ok(CodeSearchStats {
            file_count: self
                .store
                .count_files()
                .map_err(|e| SearchError::Storage(e.to_string()))?,
            symbol_count: self
                .store
                .count_symbols()
                .map_err(|e| SearchError::Storage(e.to_string()))?,
        })
    }

    /// Check if the store has indexed content
    pub fn has_indexed_content(&self) -> bool {
        self.store.count_symbols().unwrap_or(0) > 0
    }
}

// =============================================================================
// Async implementation (requires `parallel` feature)
// =============================================================================

#[cfg(feature = "parallel")]
impl<S, V, T> CodeSearch<S, V, T>
where
    S: CodeStore + Send + Sync + 'static,
    V: VectorStore + Send + Sync + 'static,
    T: TextIndex + Send + Sync + 'static,
{
    /// Search for code symbols asynchronously
    ///
    /// Wraps CPU-bound search operations in `spawn_blocking`.
    pub async fn search_async(&self, opts: &CodeSearchOptions) -> Result<Vec<CodeSearchResult>> {
        let store = Arc::clone(&self.store);
        let vector_store = Arc::clone(&self.vector_store);
        let bm25_index = Arc::clone(&self.bm25_index);
        let embedder = self.embedder.clone();
        let opts = opts.clone();

        tokio::task::spawn_blocking(move || {
            // Create a temporary CodeSearch for the search operation
            let search = CodeSearch {
                store,
                vector_store,
                bm25_index,
                embedder,
            };
            search.search(&opts)
        })
        .await
        .map_err(|e| SearchError::Index(format!("spawn_blocking: {}", e)))?
    }

    /// Pattern-based search using BM25 asynchronously
    pub async fn pattern_search_async(
        &self,
        opts: &CodeSearchOptions,
    ) -> Result<Vec<CodeSearchResult>> {
        let store = Arc::clone(&self.store);
        let vector_store = Arc::clone(&self.vector_store);
        let bm25_index = Arc::clone(&self.bm25_index);
        let embedder = self.embedder.clone();
        let opts = opts.clone();

        tokio::task::spawn_blocking(move || {
            let search = CodeSearch {
                store,
                vector_store,
                bm25_index,
                embedder,
            };
            search.pattern_search(&opts)
        })
        .await
        .map_err(|e| SearchError::Index(format!("spawn_blocking: {}", e)))?
    }

    /// Semantic search using vector embeddings asynchronously
    pub async fn semantic_search_async(
        &self,
        opts: &CodeSearchOptions,
    ) -> Result<Vec<CodeSearchResult>> {
        let store = Arc::clone(&self.store);
        let vector_store = Arc::clone(&self.vector_store);
        let bm25_index = Arc::clone(&self.bm25_index);
        let embedder = self.embedder.clone();
        let opts = opts.clone();

        tokio::task::spawn_blocking(move || {
            let search = CodeSearch {
                store,
                vector_store,
                bm25_index,
                embedder,
            };
            search.semantic_search(&opts)
        })
        .await
        .map_err(|e| SearchError::Index(format!("spawn_blocking: {}", e)))?
    }

    /// Search by exact symbol name asynchronously
    pub async fn search_by_name_async(
        &self,
        qualified_name: &str,
    ) -> Result<Vec<CodeSearchResult>> {
        let store = Arc::clone(&self.store);
        let qualified_name = qualified_name.to_string();

        tokio::task::spawn_blocking(move || {
            let symbols = store
                .get_symbols_by_name(&qualified_name)
                .map_err(|e| SearchError::Storage(e.to_string()))?;

            Ok(symbols
                .into_iter()
                .map(|s| CodeSearchResult::from_symbol(s, 1.0, true))
                .collect())
        })
        .await
        .map_err(|e| SearchError::Index(format!("spawn_blocking: {}", e)))?
    }

    /// Get search statistics asynchronously
    pub async fn stats_async(&self) -> Result<CodeSearchStats> {
        let store = Arc::clone(&self.store);

        tokio::task::spawn_blocking(move || {
            Ok(CodeSearchStats {
                file_count: store
                    .count_files()
                    .map_err(|e| SearchError::Storage(e.to_string()))?,
                symbol_count: store
                    .count_symbols()
                    .map_err(|e| SearchError::Storage(e.to_string()))?,
            })
        })
        .await
        .map_err(|e| SearchError::Index(format!("spawn_blocking: {}", e)))?
    }
}

#[cfg(test)]
#[path = "code_search_tests/tests.rs"]
mod tests;
