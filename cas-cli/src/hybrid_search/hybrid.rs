//! Hybrid search combining BM25, semantic, temporal, graph, code, knowledge and
//! git-history search (Hindsight-inspired)
//!
//! This module provides a 7-channel hybrid search:
//! 1. BM25 (lexical) - traditional text matching
//! 2. Semantic - embedding-based similarity
//! 3. Temporal - time-aware retrieval using valid_from/valid_until
//! 4. Graph - spreading activation over entity relationships
//! 5. Code - semantic code search over indexed symbols
//! 6. Knowledge - FTS over distilled project-knowledge pages
//! 7. History - the structural git-history index (EPIC cas-6212, spec §6.2)
//! Reranking (optional) - ML-based score refinement
//!
//! The history channel is deliberately a **channel here** rather than a ranker
//! of its own. M6 (cas-7909) deleted a second, unrelated `HybridSearch` whose
//! `semantic_score` was hardcoded `0.0`; a history-specific ranker would
//! recreate that artifact, and it would not inherit the capability-honest
//! weight renormalization in [`crate::hybrid_search::scorer`] that keeps a
//! machine with no cloud login from scoring everything zero.

use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use std::path::Path as StdPath;

use crate::hybrid_search::cache::SearchCache;
use crate::hybrid_search::code::{CasCodeSearch, open_code_search};
use crate::hybrid_search::graph::{GraphRetriever, SpreadingActivationConfig};
use crate::hybrid_search::scorer::{
    ChannelCapabilities, SearchWeights, calibrate_scores, combine_multi_channel,
    percentile_normalize, rrf_with_magnitude,
};
use crate::hybrid_search::semantic::SemanticChannel;
use crate::hybrid_search::temporal::{TemporalRetriever, TimePeriod};
use crate::hybrid_search::{DocType, SearchIndex, SearchOptions, SearchResult};
// Note: Local embeddings have been removed. Semantic search is now cloud-only.
// The hybrid search continues to support BM25, temporal, graph, and code search locally.
use crate::error::Result;
use crate::store::EntityStore;
use crate::types::Entry;
use cas_search::CodeSearchOptions;
use cas_store::{
    HistoryQuery, HistoryStore, KnowledgeStore, SqliteHistoryStore, SqliteKnowledgeStore,
};

/// How many activated entities the knowledge channel follows back into the
/// page index. Each one costs an FTS query, so this bounds the fan-out.
const GRAPH_LINK_SEEDS: usize = 5;

/// Structural narrowing for the git-history channel (spec §6.1).
///
/// Separate from [`HistoryQuery`] because the ranker owns the text half: the
/// query string comes from `base.query` (after temporal extraction), so a
/// caller cannot accidentally search for one thing and rank another.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct HistoryFilter {
    /// Repository identity, as produced by `crate::history::repository_id`.
    pub repository: String,
    /// Substring match against a commit's touched paths.
    pub path: Option<String>,
    /// Exact qualified symbol name recorded by M3. Incompletely mapped commits
    /// remain candidates so the history response can state that uncertainty.
    pub symbol: Option<String>,
    /// Inclusive RFC3339 bounds on `committed_at`.
    pub since: Option<String>,
    pub until: Option<String>,
    /// Merge commits are excluded by default (their message is `Merge branch
    /// 'x'`, which is noise — spec §7.1).
    pub include_merges: bool,
    /// Restrict to this exact SHA set — how the `task_id` / `session_id`
    /// filters reach SQL (EPIC cas-6212 / cas-519f). Applied *before* `LIMIT`,
    /// so a task whose commits are not already in the top-k still answers.
    /// `Some(empty)` legitimately matches nothing; `None` is no filter.
    pub shas: Option<Vec<String>>,
}

/// Options specific to hybrid search
#[derive(Debug, Clone)]
pub struct HybridSearchOptions {
    /// Base search options (query, limit, filters)
    pub base: SearchOptions,

    /// Enable semantic search component
    pub enable_semantic: bool,

    /// Enable temporal search component (Hindsight-inspired)
    pub enable_temporal: bool,

    /// Enable graph-based search component (Hindsight-inspired)
    pub enable_graph: bool,

    /// Enable code search component (searches indexed code symbols)
    pub enable_code: bool,

    /// Enable the distilled-knowledge component (searches knowledge pages)
    pub enable_knowledge: bool,

    /// Enable the git-history component (searches indexed commits)
    pub enable_history: bool,

    /// Structural narrowing for the history channel: path, time window, merge
    /// handling. `repository` is required — the index is multi-repo and an
    /// unqualified query would return another checkout's commits.
    pub history_filter: Option<HistoryFilter>,

    /// Weight for BM25 score (0.0-1.0) - only used if use_adaptive_weights is false
    pub bm25_weight: f32,

    /// Weight for semantic score (0.0-1.0) - only used if use_adaptive_weights is false
    pub semantic_weight: f32,

    /// Weight for temporal score (0.0-1.0) - only used if use_adaptive_weights is false
    pub temporal_weight: f32,

    /// Weight for graph score (0.0-1.0) - only used if use_adaptive_weights is false
    pub graph_weight: f32,

    /// Weight for code score (0.0-1.0) - only used if use_adaptive_weights is false
    pub code_weight: f32,

    /// Weight for the knowledge channel (0.0-1.0)
    ///
    /// Unlike graph/code this is not a boost on existing rows: knowledge page
    /// IDs never collide with entry IDs, so knowledge hits are unioned into
    /// the result set at this weight.
    pub knowledge_weight: f32,

    /// Weight for the git-history channel (0.0-1.0)
    ///
    /// Unioned rather than boosted, for the same reason as `knowledge_weight`:
    /// commit SHAs never collide with entry IDs, so a multiplicative boost
    /// would be a guaranteed no-op and commits could never surface at all.
    pub history_weight: f32,

    /// Enable reranking of top results
    pub enable_rerank: bool,

    /// Number of candidates to fetch before reranking
    pub rerank_candidates: usize,

    /// Use Reciprocal Rank Fusion instead of weighted sum
    pub use_rrf: bool,

    /// RRF constant (typically 60)
    pub rrf_k: f64,

    /// Explicit temporal period to search (overrides auto-extraction)
    pub temporal_period: Option<TimePeriod>,

    /// Use adaptive weights based on query analysis (recommended)
    pub use_adaptive_weights: bool,

    /// Calibrate final scores to meaningful 0-1 range
    pub calibrate_scores: bool,
}

impl Default for HybridSearchOptions {
    fn default() -> Self {
        Self {
            base: SearchOptions::default(),
            enable_semantic: true,
            enable_temporal: true, // Enable by default for Hindsight-style search
            enable_graph: true,    // Enable by default for Hindsight-style search
            enable_code: false,    // Disabled by default (requires indexed codebase)
            // Disabled by default: opt-in, because knowledge pages are a
            // different doc type and entry-only callers (the SessionStart
            // scorer) must not receive them.
            enable_knowledge: false,
            // Same opt-in discipline as knowledge: commits are a different doc
            // type, and entry-only callers (the SessionStart scorer) must not
            // suddenly receive them.
            enable_history: false,
            history_filter: None,
            bm25_weight: 0.30, // Fallback weights (not used when adaptive is enabled)
            semantic_weight: 0.30,
            temporal_weight: 0.15,
            graph_weight: 0.15,
            code_weight: 0.10,      // Code search weight
            knowledge_weight: 0.25, // Distilled knowledge weight
            history_weight: 0.25,   // Git-history weight
            enable_rerank: false,
            rerank_candidates: 10, // Reduced from 20 for better performance
            use_rrf: false,
            rrf_k: 60.0,
            temporal_period: None,
            use_adaptive_weights: true, // Use intelligent weight selection by default
            calibrate_scores: true,     // Produce meaningful 0-1 scores by default
        }
    }
}

impl HybridSearchOptions {
    /// Compute a hash of the search options for cache keying
    pub fn cache_key(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.base.query.hash(&mut hasher);
        self.base.limit.hash(&mut hasher);
        self.enable_semantic.hash(&mut hasher);
        self.enable_temporal.hash(&mut hasher);
        self.enable_graph.hash(&mut hasher);
        self.enable_code.hash(&mut hasher);
        self.enable_knowledge.hash(&mut hasher);
        self.enable_history.hash(&mut hasher);
        // The history filter is part of the query, not a display option: two
        // searches for the same words under different `path`/`since` filters
        // have different answers, and hashing only the words would serve the
        // first one's results to the second.
        self.history_filter.hash(&mut hasher);
        self.enable_rerank.hash(&mut hasher);
        self.use_rrf.hash(&mut hasher);
        self.use_adaptive_weights.hash(&mut hasher);
        // Hash weights as bits for determinism
        self.bm25_weight.to_bits().hash(&mut hasher);
        self.semantic_weight.to_bits().hash(&mut hasher);
        self.temporal_weight.to_bits().hash(&mut hasher);
        self.graph_weight.to_bits().hash(&mut hasher);
        self.code_weight.to_bits().hash(&mut hasher);
        self.knowledge_weight.to_bits().hash(&mut hasher);
        self.history_weight.to_bits().hash(&mut hasher);
        // Hash filter options
        for tag in &self.base.tags {
            tag.hash(&mut hasher);
        }
        for t in &self.base.types {
            t.hash(&mut hasher);
        }
        self.base.include_archived.hash(&mut hasher);
        hasher.finish()
    }
}

/// Extended search result with hybrid scores
#[derive(Debug, Clone)]
pub struct HybridSearchResult {
    /// Entry ID, or knowledge page ID when `doc_type` is `KnowledgePage`
    pub id: String,
    /// What `id` refers to. Entries unless the knowledge channel produced it.
    pub doc_type: DocType,
    /// Final combined score
    pub score: f64,
    /// BM25 component score (normalized)
    pub bm25_score: f64,
    /// Semantic component score (cosine similarity)
    pub semantic_score: f64,
    /// Temporal component score (time-relevance)
    pub temporal_score: f64,
    /// Graph component score (spreading activation)
    pub graph_score: f64,
    /// Code search component score
    pub code_score: f64,
    /// Distilled-knowledge component score
    pub knowledge_score: f64,
    /// Git-history component score
    pub history_score: f64,
    /// Rerank score (if reranking enabled)
    pub rerank_score: Option<f64>,
}

/// Consolidated channel scores for a single result
///
/// Instead of 5 separate HashMaps, we use a single map with this struct
/// to reduce memory allocation and simplify score merging logic.
#[derive(Debug, Clone, Default)]
struct ChannelScores {
    bm25: f64,
    semantic: f64,
    temporal: f64,
    graph: f64,
    code: f64,
    knowledge: f64,
    history: f64,
}

impl From<HybridSearchResult> for SearchResult {
    fn from(h: HybridSearchResult) -> Self {
        SearchResult {
            doc_type: h.doc_type,
            id: h.id,
            score: h.score,
            bm25_score: h.bm25_score,
            boosted_score: h.score,
        }
    }
}

/// Hybrid search orchestrator combining BM25, temporal, graph, and code search
///
/// Note: Local semantic/embedding search has been removed and is now cloud-only.
/// This orchestrator still supports BM25 full-text search, temporal filtering,
/// knowledge graph traversal, and code symbol search.
pub struct HybridSearch {
    bm25_index: SearchIndex,
    graph_retriever: Option<GraphRetriever>,
    /// Code search for semantic code symbol search
    code_search: Option<CasCodeSearch>,
    /// Distilled project knowledge (FTS over knowledge pages)
    knowledge_store: Option<Arc<dyn KnowledgeStore>>,
    /// Structural git-history index (FTS over commit prose + path/time filters)
    history_store: Option<Arc<dyn HistoryStore>>,
    /// Cloud-backed embedding channel (T5). `None` on any installation
    /// without cloud auth — see `hybrid_search::semantic`.
    semantic_channel: Option<Arc<SemanticChannel>>,
    /// Query and results cache for performance
    cache: Arc<SearchCache>,
}

impl HybridSearch {
    /// Create a new hybrid search instance
    pub fn new(bm25_index: SearchIndex) -> Self {
        Self {
            bm25_index,
            graph_retriever: None,
            code_search: None,
            knowledge_store: None,
            history_store: None,
            semantic_channel: None,
            cache: Arc::new(SearchCache::new()),
        }
    }

    /// Create a new hybrid search instance with a shared cache
    pub fn with_cache(bm25_index: SearchIndex, cache: Arc<SearchCache>) -> Self {
        Self {
            bm25_index,
            graph_retriever: None,
            code_search: None,
            knowledge_store: None,
            history_store: None,
            semantic_channel: None,
            cache,
        }
    }

    /// Create a new hybrid search instance with graph retriever
    pub fn with_graph(bm25_index: SearchIndex, entity_store: Arc<dyn EntityStore>) -> Self {
        Self {
            bm25_index,
            graph_retriever: Some(GraphRetriever::with_defaults(entity_store)),
            code_search: None,
            knowledge_store: None,
            history_store: None,
            semantic_channel: None,
            cache: Arc::new(SearchCache::new()),
        }
    }

    /// Create a new hybrid search instance with custom graph config
    pub fn with_graph_config(
        bm25_index: SearchIndex,
        entity_store: Arc<dyn EntityStore>,
        graph_config: SpreadingActivationConfig,
    ) -> Self {
        Self {
            bm25_index,
            graph_retriever: Some(GraphRetriever::new(entity_store, graph_config)),
            code_search: None,
            knowledge_store: None,
            history_store: None,
            semantic_channel: None,
            cache: Arc::new(SearchCache::new()),
        }
    }

    /// Open hybrid search from a CAS directory
    ///
    /// Note: Local semantic search has been removed and is now cloud-only.
    /// This opens BM25 search only.
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let index_dir = cas_dir.join("index").join("tantivy");

        // Open BM25 index
        let bm25_index = SearchIndex::open(&index_dir)?;

        Ok(Self {
            bm25_index,
            graph_retriever: None,  // Needs entity store to be set separately
            code_search: None,      // Needs code store to be set separately
            knowledge_store: None,  // Needs knowledge store to be set separately
            history_store: None,    // Needs history store to be set separately
            semantic_channel: None, // Needs cloud auth; see set_semantic_channel
            cache: Arc::new(SearchCache::new()),
        })
    }

    /// Open hybrid search with graph retriever (knowledge graph enabled)
    pub fn open_with_graph(cas_dir: &Path) -> Result<Self> {
        let mut search = Self::open(cas_dir)?;
        // Try to open entity store - if it fails, continue without graph
        if let Ok(entity_store) = crate::store::open_entity_store(cas_dir) {
            search.graph_retriever = Some(GraphRetriever::with_defaults(entity_store));
        }
        Ok(search)
    }

    /// Open hybrid search with graph retriever enabled
    ///
    /// Note: Local reranker has been removed and is now cloud-only.
    /// This is now equivalent to `open_with_graph`.
    pub fn open_full(cas_dir: &Path) -> Result<Self> {
        Self::open_with_graph(cas_dir)
    }

    /// Set the graph retriever (requires entity store)
    pub fn set_graph_retriever(&mut self, entity_store: Arc<dyn EntityStore>) {
        self.graph_retriever = Some(GraphRetriever::with_defaults(entity_store));
    }

    /// Set the graph retriever with custom config
    pub fn set_graph_retriever_with_config(
        &mut self,
        entity_store: Arc<dyn EntityStore>,
        config: SpreadingActivationConfig,
    ) {
        self.graph_retriever = Some(GraphRetriever::new(entity_store, config));
    }

    /// Set the code search from a CAS directory path
    ///
    /// Opens all required components (code store, vector store, BM25 index, embedder)
    /// and wires them together into a CasCodeSearch instance.
    pub fn set_code_search_from_path(&mut self, cas_dir: &StdPath) -> Result<()> {
        self.code_search = Some(open_code_search(cas_dir)?);
        Ok(())
    }

    /// Set the code search directly from an existing instance
    pub fn set_code_search(&mut self, code_search: CasCodeSearch) {
        self.code_search = Some(code_search);
    }

    /// Attach the distilled-knowledge store, enabling the knowledge channel.
    pub fn set_knowledge_store(&mut self, store: Arc<dyn KnowledgeStore>) {
        self.knowledge_store = Some(store);
    }

    /// Open and attach the knowledge store rooted at `cas_dir`.
    pub fn set_knowledge_store_from_path(&mut self, cas_dir: &StdPath) -> Result<()> {
        self.knowledge_store = Some(Arc::new(SqliteKnowledgeStore::open(cas_dir)?));
        Ok(())
    }

    /// Whether the distilled-knowledge channel can return rows.
    pub fn has_knowledge_store(&self) -> bool {
        self.knowledge_store.is_some()
    }

    /// Attach the structural git-history store, enabling the history channel.
    pub fn set_history_store(&mut self, store: Arc<dyn HistoryStore>) {
        self.history_store = Some(store);
    }

    /// Open and attach the history store rooted at `cas_dir`.
    pub fn set_history_store_from_path(&mut self, cas_dir: &StdPath) -> Result<()> {
        self.history_store = Some(Arc::new(SqliteHistoryStore::open(cas_dir)?));
        Ok(())
    }

    /// Whether the git-history channel can return rows.
    pub fn has_history_store(&self) -> bool {
        self.history_store.is_some()
    }

    /// Attach a cloud-backed semantic channel (T5).
    ///
    /// Optional by construction: retrieval is fully functional without it,
    /// and [`has_semantic`](Self::has_semantic) keeps reporting `false` until
    /// the channel can actually return rows.
    pub fn set_semantic_channel(&mut self, channel: Arc<SemanticChannel>) {
        self.semantic_channel = Some(channel);
    }

    /// Open and attach the semantic channel for `cas_dir` if the cloud
    /// capability is present. Returns whether a channel was attached.
    pub fn set_semantic_channel_from_config(
        &mut self,
        cas_dir: &StdPath,
        config: &crate::cloud::CloudConfig,
    ) -> bool {
        match crate::hybrid_search::semantic::open_semantic_channel(cas_dir, config) {
            Some(channel) => {
                self.semantic_channel = Some(Arc::new(channel));
                true
            }
            None => false,
        }
    }

    /// Whether the embedding channel can return rows.
    ///
    /// True only when a cloud embedder is attached AND vectors are cached
    /// locally. A configured-but-empty channel still reports `false`: it can
    /// only return an empty list, and telling [`ChannelCapabilities`]
    /// otherwise would re-introduce exactly the dishonest weight allocation
    /// T3 removed.
    pub fn has_semantic(&self) -> bool {
        self.semantic_channel
            .as_ref()
            .is_some_and(|channel| channel.is_live())
    }

    /// Which scored channels can actually contribute for this request.
    fn channel_capabilities(&self, opts: &HybridSearchOptions) -> ChannelCapabilities {
        ChannelCapabilities {
            // Structurally always live: no `HybridSearch` without a SearchIndex.
            bm25: true,
            semantic: opts.enable_semantic && self.has_semantic(),
            temporal: opts.enable_temporal,
        }
    }

    /// Perform hybrid search (6-channel: BM25 + semantic + temporal + graph + code + rerank)
    pub fn search(
        &self,
        opts: &HybridSearchOptions,
        entries: &[Entry],
    ) -> Result<Vec<HybridSearchResult>> {
        // Try to get cached hybrid results
        let cache_key = opts.cache_key();
        if let Some(cached) = self.cache.get_hybrid_results(cache_key) {
            return Ok(cached);
        }

        // Extract temporal period from query if enabled and not explicitly set
        let (search_query, temporal_period) =
            if opts.enable_temporal && opts.temporal_period.is_none() {
                if let Some((cleaned, period)) =
                    TemporalRetriever::extract_temporal_query(&opts.base.query)
                {
                    (cleaned, Some(period))
                } else {
                    (opts.base.query.clone(), None)
                }
            } else {
                (opts.base.query.clone(), opts.temporal_period.clone())
            };

        // 1. BM25 search (using cleaned query without temporal expressions)
        let mut bm25_opts = opts.base.clone();
        bm25_opts.query = search_query.clone();
        let bm25_results = self.bm25_index.search(&bm25_opts, entries)?;
        let bm25_scores: Vec<(String, f64)> = bm25_results
            .iter()
            .map(|r| (r.id.clone(), r.bm25_score))
            .collect();

        // 2. Semantic search (if enabled)
        // Filter to only include IDs that exist in the active entries list
        let valid_ids: std::collections::HashSet<&str> =
            entries.iter().map(|e| e.id.as_str()).collect();

        let semantic_scores: Vec<(String, f32)> =
            if opts.enable_semantic && !search_query.is_empty() {
                self.semantic_search(&search_query, opts.base.limit * 3)?
                    .into_iter()
                    .filter(|(id, _)| valid_ids.contains(id.as_str()))
                    .collect()
            } else {
                Vec::new()
            };

        // 3. Temporal search (if enabled and we have a period)
        let temporal_scores: Vec<(String, f64)> = if opts.enable_temporal {
            if let Some(ref period) = temporal_period {
                let retriever = TemporalRetriever::default();
                retriever
                    .retrieve(entries, period, opts.base.limit * 3)
                    .into_iter()
                    .map(|r| (r.id, r.temporal_score as f64))
                    .collect()
            } else {
                // No explicit temporal period - use recency as a fallback
                // Score entries by how recently they were created/accessed
                self.recency_scores(entries, opts.base.limit * 3)
            }
        } else {
            Vec::new()
        };

        // 4. Graph search (if enabled and graph retriever is available)
        let graph_scores: Vec<(String, f64)> =
            if opts.enable_graph && self.graph_retriever.is_some() {
                if let Some(ref retriever) = self.graph_retriever {
                    retriever
                        .retrieve_entries(&search_query, opts.base.limit * 3)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|r| valid_ids.contains(r.entry_id.as_str()))
                        .map(|r| (r.entry_id, r.activation_score as f64))
                        .collect()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

        // 5. Code search (if enabled and code search is available)
        let code_scores: Vec<(String, f64)> = if opts.enable_code && self.code_search.is_some() {
            if let Some(ref code_search) = self.code_search {
                let code_opts = CodeSearchOptions {
                    query: search_query.clone(),
                    limit: opts.base.limit * 3,
                    semantic: true,
                    ..Default::default()
                };
                code_search
                    .search(&code_opts)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|r| (r.id, r.score))
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        // 5b. Distilled-knowledge search (if enabled and a store is attached)
        let knowledge_scores: Vec<(String, f64)> = if opts.enable_knowledge {
            self.knowledge_scores(&search_query, opts.base.limit * 3)
        } else {
            Vec::new()
        };

        // 5c. Git-history search (if enabled and a store is attached)
        let history_scores: Vec<(String, f64)> = if opts.enable_history {
            self.history_scores(&search_query, opts)
        } else {
            Vec::new()
        };

        // 6. Combine scores using the new scoring system
        let semantic_f64: Vec<(String, f64)> = semantic_scores
            .into_iter()
            .map(|(id, s)| (id, s as f64))
            .collect();

        let combined = if opts.use_rrf {
            // Enhanced RRF with magnitude awareness
            let mut rankings = vec![bm25_scores.clone(), semantic_f64.clone()];
            if !temporal_scores.is_empty() {
                rankings.push(temporal_scores.clone());
            }
            if !graph_scores.is_empty() {
                rankings.push(graph_scores.clone());
            }
            if !code_scores.is_empty() {
                rankings.push(code_scores.clone());
            }
            if !knowledge_scores.is_empty() {
                rankings.push(knowledge_scores.clone());
            }
            if !history_scores.is_empty() {
                rankings.push(history_scores.clone());
            }
            rrf_with_magnitude(&rankings, opts.rrf_k)
        } else {
            // Determine weights - adaptive or manual, then renormalize over the
            // channels that can actually fire. Without this, a Conceptual query
            // hands 0.60 to the semantic channel, which returns nothing locally.
            let caps = self.channel_capabilities(opts);
            let weights = if opts.use_adaptive_weights {
                SearchWeights::from_query(&search_query)
            } else {
                SearchWeights::custom(opts.bm25_weight, opts.semantic_weight, opts.temporal_weight)
            }
            .for_capabilities(caps);

            // Single-step multi-channel combination
            let mut combined =
                combine_multi_channel(&bm25_scores, &semantic_f64, &temporal_scores, weights);

            // Add graph scores if available (as an additional boost)
            if !graph_scores.is_empty() && opts.graph_weight > 0.0 {
                let graph_map: std::collections::HashMap<&str, f64> = graph_scores
                    .iter()
                    .map(|(id, s)| (id.as_str(), *s))
                    .collect();

                for (id, score) in combined.iter_mut() {
                    if let Some(graph_score) = graph_map.get(id.as_str()) {
                        // Apply graph boost (multiplicative to preserve ranking)
                        let boost = 1.0 + (opts.graph_weight as f64) * graph_score;
                        *score *= boost;
                    }
                }

                // Re-sort after graph boost
                combined.sort_by(|a, b| b.1.total_cmp(&a.1));
            }

            // Add code scores if available (as an additional boost)
            if !code_scores.is_empty() && opts.code_weight > 0.0 {
                let code_map: std::collections::HashMap<&str, f64> = code_scores
                    .iter()
                    .map(|(id, s)| (id.as_str(), *s))
                    .collect();

                for (id, score) in combined.iter_mut() {
                    if let Some(code_score) = code_map.get(id.as_str()) {
                        // Apply code boost (multiplicative to preserve ranking)
                        let boost = 1.0 + (opts.code_weight as f64) * code_score;
                        *score *= boost;
                    }
                }

                // Re-sort after code boost
                combined.sort_by(|a, b| b.1.total_cmp(&a.1));
            }

            // Union in knowledge hits. Graph and code are applied above as
            // multiplicative boosts because their IDs can coincide with entry
            // IDs; knowledge page IDs (`cas-kn…`) never do, so a boost would be
            // a guaranteed no-op and the pages could never surface at all.
            if !knowledge_scores.is_empty() && opts.knowledge_weight > 0.0 {
                for (id, score) in percentile_normalize(&knowledge_scores, 90.0) {
                    combined.push((id, (opts.knowledge_weight as f64) * score));
                }
                combined.sort_by(|a, b| b.1.total_cmp(&a.1));
            }

            // History unions in for the same reason knowledge does: a commit
            // SHA is never an entry ID, so a multiplicative boost would match
            // nothing and the channel would be inert — the precise failure
            // §6.3 makes an acceptance gate.
            if !history_scores.is_empty() && opts.history_weight > 0.0 {
                for (id, score) in percentile_normalize(&history_scores, 90.0) {
                    combined.push((id, (opts.history_weight as f64) * score));
                }
                combined.sort_by(|a, b| b.1.total_cmp(&a.1));
            }

            combined
        };

        // Note: Calibration moved to after reranking (step 8b) so reranker scores also get calibrated

        // 7. Create initial results using consolidated score map
        // Single HashMap instead of 5 separate ones - reduces memory and simplifies lookups
        let mut score_map: std::collections::HashMap<String, ChannelScores> =
            std::collections::HashMap::new();

        // Populate from all score vectors in a single pass per channel
        for (id, score) in &bm25_scores {
            score_map.entry(id.clone()).or_default().bm25 = *score;
        }
        for (id, score) in &semantic_f64 {
            score_map.entry(id.clone()).or_default().semantic = *score;
        }
        for (id, score) in &temporal_scores {
            score_map.entry(id.clone()).or_default().temporal = *score;
        }
        for (id, score) in &graph_scores {
            score_map.entry(id.clone()).or_default().graph = *score;
        }
        for (id, score) in &code_scores {
            score_map.entry(id.clone()).or_default().code = *score;
        }
        let knowledge_ids: std::collections::HashSet<&str> =
            knowledge_scores.iter().map(|(id, _)| id.as_str()).collect();
        for (id, score) in &knowledge_scores {
            score_map.entry(id.clone()).or_default().knowledge = *score;
        }
        let history_ids: std::collections::HashSet<&str> =
            history_scores.iter().map(|(id, _)| id.as_str()).collect();
        for (id, score) in &history_scores {
            score_map.entry(id.clone()).or_default().history = *score;
        }

        let mut results: Vec<HybridSearchResult> = combined
            .into_iter()
            .take(if opts.enable_rerank {
                opts.rerank_candidates
            } else {
                opts.base.limit
            })
            .map(|(id, score)| {
                let channel_scores = score_map.get(&id).cloned().unwrap_or_default();
                let doc_type = if knowledge_ids.contains(id.as_str()) {
                    DocType::KnowledgePage
                } else if history_ids.contains(id.as_str()) {
                    DocType::HistoryCommit
                } else {
                    DocType::Entry
                };
                HybridSearchResult {
                    bm25_score: channel_scores.bm25,
                    semantic_score: channel_scores.semantic,
                    temporal_score: channel_scores.temporal,
                    graph_score: channel_scores.graph,
                    code_score: channel_scores.code,
                    knowledge_score: channel_scores.knowledge,
                    history_score: channel_scores.history,
                    doc_type,
                    id,
                    score,
                    rerank_score: None,
                }
            })
            .collect();

        // 8. Rerank if enabled
        // Note: Local reranking has been removed and is now cloud-only.
        // The enable_rerank option is preserved for API compatibility but has no effect locally.

        // 8b. Calibrate scores AFTER reranking so all scores are in meaningful 0-1 range
        if opts.calibrate_scores && !results.is_empty() {
            // Extract scores, calibrate, and apply back
            let mut scores: Vec<(String, f64)> =
                results.iter().map(|r| (r.id.clone(), r.score)).collect();
            calibrate_scores(&mut scores);

            // Apply calibrated scores back to results
            let score_map: std::collections::HashMap<&str, f64> =
                scores.iter().map(|(id, s)| (id.as_str(), *s)).collect();
            for result in results.iter_mut() {
                if let Some(&cal_score) = score_map.get(result.id.as_str()) {
                    result.score = cal_score;
                }
            }
        }

        // 9. Apply final limit
        results.truncate(opts.base.limit);

        // Cache the results
        self.cache.put_hybrid_results(cache_key, results.clone());

        Ok(results)
    }

    /// Generate recency scores for entries (fallback when no explicit temporal query)
    fn recency_scores(&self, entries: &[Entry], limit: usize) -> Vec<(String, f64)> {
        use chrono::Utc;

        let now = Utc::now();
        let mut scores: Vec<(String, f64)> = entries
            .iter()
            .filter(|e| !e.archived)
            .map(|e| {
                // Use last_accessed if available, otherwise created
                let last_time = e.last_accessed.unwrap_or(e.created);
                let days_ago = (now - last_time).num_days().max(0) as f64;

                // Exponential decay: score = 0.5^(days/30)
                // Recent entries score ~1.0, entries from 30 days ago score ~0.5
                let score = 0.5f64.powf(days_ago / 30.0);

                (e.id.clone(), score)
            })
            .collect();

        // Sort by score descending
        scores.sort_by(|a, b| b.1.total_cmp(&a.1));
        scores.truncate(limit);

        scores
    }

    /// Retrieve distilled knowledge pages for a query.
    ///
    /// Two passes over the same store:
    ///
    /// 1. **Lexical** — FTS5 over page title + snippet + body. SQLite's
    ///    `bm25()` is a cost (more negative = better match), so it is negated
    ///    into a relevance where larger = better, matching every other channel.
    /// 2. **Entity-graph links** — the query's entity candidates are resolved
    ///    to seed entities and spread over the existing entity graph; each
    ///    activated entity's name is then looked up in the page index. This is
    ///    what surfaces a page that never mentions the query's words but is
    ///    about an entity the query is about. Linked hits are scaled by their
    ///    activation and only fill in pages the lexical pass missed, so a
    ///    weak graph link can never outrank a direct textual match.
    ///
    /// Errors are swallowed to empty results: knowledge is an enrichment
    /// channel and must never fail a search.
    fn knowledge_scores(&self, query: &str, limit: usize) -> Vec<(String, f64)> {
        let Some(ref store) = self.knowledge_store else {
            return Vec::new();
        };
        if query.trim().is_empty() || limit == 0 {
            return Vec::new();
        }

        let mut scores: Vec<(String, f64)> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        for hit in store.search(query, limit).unwrap_or_default() {
            // SQLite bm25() is a cost: 0 is "no information", negative is a
            // match, and more negative is better. Negate so bigger = better.
            let relevance = (-hit.score).max(0.0);
            if seen.insert(hit.page.id.clone()) {
                scores.push((hit.page.id, relevance));
            }
        }

        // Cap graph-linked relevance at the weakest direct hit so a linked page
        // can never outrank a page that literally matched the query. With no
        // direct hits there is nothing to stay below, so use the full range.
        let lexical_floor = if scores.is_empty() {
            1.0
        } else {
            scores
                .iter()
                .map(|(_, s)| *s)
                .fold(f64::INFINITY, f64::min)
                .clamp(0.0, 1.0)
        };

        if let Some(ref retriever) = self.graph_retriever {
            let candidates = retriever.extract_entity_candidates(query);
            if !candidates.is_empty() {
                if let Ok(seeds) = retriever.find_seed_entities(&candidates) {
                    if let Ok(activations) = retriever.spread_activation(&seeds) {
                        // Deterministic order: activation desc, then entity id.
                        let mut activated: Vec<(String, f32)> = activations.into_iter().collect();
                        activated.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

                        for (entity_id, activation) in activated.into_iter().take(GRAPH_LINK_SEEDS)
                        {
                            if activation <= 0.0 {
                                continue;
                            }
                            let Ok(entity) = retriever.entity_store().get_entity(&entity_id) else {
                                continue;
                            };
                            for hit in store.search(&entity.name, limit).unwrap_or_default() {
                                if !seen.insert(hit.page.id.clone()) {
                                    continue;
                                }
                                let linked = lexical_floor * (activation as f64) * 0.5;
                                scores.push((hit.page.id, linked));
                            }
                        }
                    }
                }
            }
        }

        // Stable order for prompt-cache friendliness: score desc, then id.
        scores.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        scores.truncate(limit);
        scores
    }

    /// Retrieve indexed commits for a query (spec §6.2).
    ///
    /// Two modes, and the distinction is the whole design:
    ///
    /// 1. **With query text** — FTS5 BM25 over commit subject + body, narrowed
    ///    by the structural filter. Ranked by relevance.
    /// 2. **Without query text** — a purely structural question ("what touched
    ///    this path in this window"), ranked by the `0.5^(days/30)` recency
    ///    decay. This is not a degenerate case: §6.4's Q2 and Q3 *are* this
    ///    mode, and they are the two queries the epic must answer on M1 data.
    ///
    /// Errors are swallowed to empty results, matching the knowledge channel:
    /// an enrichment channel must never fail a search. What it must not do is
    /// pretend — an absent store yields no rows, and the caller reports the
    /// index status alongside (spec §6.5) rather than letting silence read as
    /// "nothing ever happened here".
    fn history_scores(&self, query: &str, opts: &HybridSearchOptions) -> Vec<(String, f64)> {
        let Some(ref store) = self.history_store else {
            return Vec::new();
        };
        // The repository is not optional: `history_commits` is multi-repo, and
        // an unqualified query would rank another checkout's commits into this
        // one's answer.
        let Some(filter) = opts.history_filter.as_ref() else {
            return Vec::new();
        };
        if opts.base.limit == 0 {
            return Vec::new();
        }

        let text = (!query.trim().is_empty()).then(|| query.to_string());
        let history_query = HistoryQuery {
            repository: filter.repository.clone(),
            text,
            path: filter.path.clone(),
            symbol: filter.symbol.clone(),
            since: filter.since.clone(),
            until: filter.until.clone(),
            include_merges: filter.include_merges,
            shas: filter.shas.clone(),
            limit: opts.base.limit * 3,
        };

        let mut scores: Vec<(String, f64)> = store
            .search_commits(&history_query)
            .unwrap_or_default()
            .into_iter()
            .map(|hit| (hit.commit.sha, hit.score))
            .collect();

        // Stable order for prompt-cache friendliness: score desc, then sha.
        scores.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        scores.truncate(opts.base.limit * 3);
        scores
    }

    /// Perform semantic-only search.
    ///
    /// Cloud-only by design: the query is embedded through the cloud endpoint
    /// and matched against locally cached page vectors. With no channel
    /// attached this returns an empty list, which is why `has_semantic()`
    /// reports the channel absent and the scorer reallocates its weight.
    fn semantic_search(&self, query: &str, k: usize) -> Result<Vec<(String, f32)>> {
        match &self.semantic_channel {
            Some(channel) => channel.search(query, k),
            None => Ok(Vec::new()),
        }
    }

    /// Index a single entry (BM25 only - embeddings are now cloud-only)
    pub fn index_entry(&self, entry: &Entry) -> Result<()> {
        // Invalidate cache entries that depend on this entry
        self.cache.invalidate_entry(&entry.id);

        // Index in BM25
        self.bm25_index.index_entry(entry)?;

        Ok(())
    }

    /// Delete from BM25 index
    pub fn delete(&self, id: &str) -> Result<()> {
        // Invalidate cache entries that depend on this entry
        self.cache.invalidate_entry(id);

        self.bm25_index.delete(id)?;
        Ok(())
    }

    /// Reindex all entries (BM25 only - embeddings are now cloud-only)
    pub fn reindex(&self, entries: &[Entry]) -> Result<()> {
        // Clear all caches since we're reindexing everything
        self.cache.clear();

        // Reindex BM25
        self.bm25_index.reindex(entries)?;

        Ok(())
    }

    /// Check if reranker is available
    ///
    /// Note: Local reranking has been removed and is now cloud-only.
    /// Always returns false.
    pub fn has_reranker(&self) -> bool {
        false
    }

    /// Check if graph retriever is available
    pub fn has_graph_retriever(&self) -> bool {
        self.graph_retriever.is_some()
    }

    /// Check if code search is available
    pub fn has_code_search(&self) -> bool {
        self.code_search.is_some()
    }

    /// Get a reference to the search cache
    pub fn cache(&self) -> &Arc<SearchCache> {
        &self.cache
    }

    /// Get cache statistics
    pub fn cache_stats(&self) -> super::cache::SearchCacheStats {
        self.cache.stats()
    }

    /// Invalidate cache entries for a specific entry ID
    ///
    /// Should be called when an entry is archived or otherwise modified
    /// outside of the index_entry/delete methods.
    pub fn invalidate_cache(&self, entry_id: &str) {
        self.cache.invalidate_entry(entry_id);
    }

    /// Clear all caches
    pub fn clear_cache(&self) {
        self.cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use crate::hybrid_search::hybrid::*;

    #[test]
    fn test_hybrid_search_options_default() {
        let opts = HybridSearchOptions::default();
        assert!(opts.enable_semantic);
        assert!(opts.enable_temporal);
        assert!(opts.enable_graph);
        assert!(!opts.enable_code); // Code search disabled by default
        assert_eq!(opts.bm25_weight, 0.30);
        assert_eq!(opts.semantic_weight, 0.30);
        assert_eq!(opts.temporal_weight, 0.15);
        assert_eq!(opts.graph_weight, 0.15);
        assert_eq!(opts.code_weight, 0.10);
        assert!(!opts.enable_rerank);
        assert!(!opts.use_rrf);
        assert!(opts.use_adaptive_weights);
        assert!(opts.calibrate_scores);
    }

    #[test]
    fn test_hybrid_result_to_search_result() {
        let hybrid = HybridSearchResult {
            id: "test".to_string(),
            score: 0.8,
            bm25_score: 0.7,
            semantic_score: 0.9,
            temporal_score: 0.6,
            graph_score: 0.5,
            code_score: 0.4,
            rerank_score: Some(0.85),
            doc_type: DocType::Entry,
            knowledge_score: 0.0,
            history_score: 0.0,
        };

        let search_result: SearchResult = hybrid.into();
        assert_eq!(search_result.id, "test");
        assert_eq!(search_result.score, 0.8);
    }

    #[test]
    fn test_temporal_query_extraction() {
        // Test that temporal expressions are extracted from queries
        let query = "what did I learn last week about rust";
        if let Some((cleaned, period)) = TemporalRetriever::extract_temporal_query(query) {
            assert!(!cleaned.contains("last week"));
            // Period should cover ~7 days ago to now
            assert!(period.start < chrono::Utc::now());
        }
    }

    // ── Distilled-knowledge channel (EPIC cas-7d31 / cas-86b2) ──────────

    fn knowledge_fixture(
        pages: &[(&str, &str, &str)],
    ) -> (tempfile::TempDir, std::sync::Arc<dyn KnowledgeStore>) {
        use cas_store::{IngestBatch, KnowledgePage, PageWrite};
        let temp = tempfile::tempdir().expect("tempdir");
        let store = SqliteKnowledgeStore::open(temp.path()).expect("open knowledge store");
        let writes: Vec<PageWrite> = pages
            .iter()
            .map(|(page_type, title, body)| {
                let id = store.generate_id().expect("id");
                let mut page = KnowledgePage::new(id, *page_type, *title);
                page.snippet = (*body).to_string();
                page.sources = vec!["docs/s.md".to_string()];
                PageWrite {
                    page,
                    body: (*body).to_string(),
                }
            })
            .collect();
        store
            .commit_ingest(&IngestBatch {
                pages: writes,
                ..Default::default()
            })
            .expect("commit");
        (temp, std::sync::Arc::new(store))
    }

    /// A HybridSearch with only a knowledge store attached, over a scratch
    /// tantivy index, so the knowledge channel is what is under test.
    fn search_with_knowledge(
        knowledge: std::sync::Arc<dyn KnowledgeStore>,
    ) -> (tempfile::TempDir, HybridSearch) {
        let temp = tempfile::tempdir().expect("tempdir");
        let index = SearchIndex::open(&temp.path().join("tantivy")).expect("open index");
        let mut hybrid = HybridSearch::new(index);
        hybrid.set_knowledge_store(knowledge);
        (temp, hybrid)
    }

    #[test]
    fn semantic_capability_is_false_with_no_channel_attached() {
        let temp = tempfile::tempdir().expect("tempdir");
        let index = SearchIndex::open(&temp.path().join("tantivy")).expect("open index");
        let hybrid = HybridSearch::new(index);
        assert!(
            !hybrid.has_semantic(),
            "a build with no cloud auth must report the semantic channel absent"
        );
        let opts = HybridSearchOptions {
            enable_semantic: true,
            ..Default::default()
        };
        assert!(
            !hybrid.channel_capabilities(&opts).semantic,
            "asking for semantic search cannot conjure a channel that isn't there"
        );
    }

    #[test]
    fn semantic_capability_tracks_the_attached_channel() {
        use crate::cloud::embeddings::{KnowledgeEmbedder, KnowledgeVectorCache};
        use crate::hybrid_search::semantic::SemanticChannel;

        let temp = tempfile::tempdir().expect("tempdir");
        let index = SearchIndex::open(&temp.path().join("tantivy")).expect("open index");
        let mut hybrid = HybridSearch::new(index);

        let embedder = KnowledgeEmbedder::new("https://example.invalid", "t").with_model("m", 4);
        let cache =
            KnowledgeVectorCache::open(temp.path(), embedder.meta()).expect("open vector cache");

        // Attached but empty: still not live, because it can only return
        // nothing and the scorer must not allocate weight to it.
        hybrid.set_semantic_channel(std::sync::Arc::new(SemanticChannel::new(
            embedder.clone(),
            cache,
        )));
        assert!(!hybrid.has_semantic());

        // One cached vector and the channel becomes real.
        let cache =
            KnowledgeVectorCache::open(temp.path(), embedder.meta()).expect("reopen vector cache");
        cache.put("cas-kn001", &[1.0, 0.0, 0.0, 0.0]).expect("put");
        hybrid.set_semantic_channel(std::sync::Arc::new(SemanticChannel::new(embedder, cache)));
        assert!(hybrid.has_semantic());

        let opts = HybridSearchOptions {
            enable_semantic: true,
            ..Default::default()
        };
        assert!(hybrid.channel_capabilities(&opts).semantic);
        // The per-request switch still wins over the capability.
        let off = HybridSearchOptions {
            enable_semantic: false,
            ..Default::default()
        };
        assert!(!hybrid.channel_capabilities(&off).semantic);
    }

    #[test]
    fn the_knowledge_channel_surfaces_pages_that_no_entry_could_match() {
        let (_kt, ks) = knowledge_fixture(&[
            ("subsystem", "Verifier", "the verifier enforces close gates"),
            (
                "subsystem",
                "Scheduler",
                "the scheduler assigns idle workers",
            ),
        ]);
        let (_it, hybrid) = search_with_knowledge(ks);

        let opts = HybridSearchOptions {
            base: SearchOptions {
                query: "verifier".to_string(),
                limit: 10,
                ..Default::default()
            },
            enable_knowledge: true,
            enable_temporal: false,
            enable_graph: false,
            ..Default::default()
        };

        // No entries at all: every hit must come from the knowledge channel.
        let results = hybrid.search(&opts, &[]).expect("search");

        assert!(
            !results.is_empty(),
            "knowledge channel returned nothing for a term that is in a page"
        );
        assert!(
            results.iter().all(|r| r.doc_type == DocType::KnowledgePage),
            "knowledge hits must be tagged as knowledge pages, not entries"
        );
        assert!(
            results.iter().any(|r| r.knowledge_score > 0.0),
            "knowledge hits must carry a positive channel score; a sign error \
             on SQLite's bm25() cost would show up here"
        );
    }

    #[test]
    fn the_knowledge_channel_stays_silent_when_disabled() {
        let (_kt, ks) = knowledge_fixture(&[("subsystem", "Verifier", "close gates")]);
        let (_it, hybrid) = search_with_knowledge(ks);

        let opts = HybridSearchOptions {
            base: SearchOptions {
                query: "verifier".to_string(),
                limit: 10,
                ..Default::default()
            },
            enable_knowledge: false,
            enable_temporal: false,
            enable_graph: false,
            ..Default::default()
        };

        let results = hybrid.search(&opts, &[]).expect("search");
        assert!(
            results.is_empty(),
            "knowledge pages leaked into a search that did not opt in"
        );
    }

    // ── Git-history channel (EPIC cas-6212 / cas-7f40, spec §6.2) ───────

    /// A history store seeded with `(sha, subject, path, committed_at)` rows.
    fn history_fixture(
        commits: &[(&str, &str, &str, &str)],
    ) -> (tempfile::TempDir, std::sync::Arc<dyn HistoryStore>) {
        use cas_store::{HistoryCommit, HistoryCommitFile};
        let temp = tempfile::tempdir().expect("tempdir");
        let store = SqliteHistoryStore::open(temp.path()).expect("open history store");
        let rows: Vec<HistoryCommit> = commits
            .iter()
            .map(|(sha, subject, _, at)| HistoryCommit {
                sha: (*sha).to_string(),
                short_sha: sha.chars().take(8).collect(),
                committed_at: (*at).to_string(),
                subject: (*subject).to_string(),
                repository: "/repo".to_string(),
                symbol_mapping: "pending".to_string(),
                ..Default::default()
            })
            .collect();
        let files: Vec<HistoryCommitFile> = commits
            .iter()
            .map(|(sha, _, path, _)| HistoryCommitFile {
                sha: (*sha).to_string(),
                file_path: (*path).to_string(),
                change_type: "M".to_string(),
                ..Default::default()
            })
            .collect();
        let watermark = commits.last().map(|c| c.0).unwrap_or("").to_string();
        store
            .commit_batch("/repo", &rows, &files, &watermark, true)
            .expect("commit batch");
        (temp, std::sync::Arc::new(store))
    }

    fn search_with_history(
        history: std::sync::Arc<dyn HistoryStore>,
    ) -> (tempfile::TempDir, HybridSearch) {
        let temp = tempfile::tempdir().expect("tempdir");
        let index = SearchIndex::open(&temp.path().join("tantivy")).expect("open index");
        let mut hybrid = HybridSearch::new(index);
        hybrid.set_history_store(history);
        (temp, hybrid)
    }

    fn history_opts(query: &str, filter: HistoryFilter) -> HybridSearchOptions {
        HybridSearchOptions {
            base: SearchOptions {
                query: query.to_string(),
                limit: 10,
                ..Default::default()
            },
            enable_history: true,
            history_filter: Some(filter),
            enable_temporal: false,
            enable_graph: false,
            ..Default::default()
        }
    }

    fn repo_filter() -> HistoryFilter {
        HistoryFilter {
            repository: "/repo".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn the_history_channel_surfaces_commits_no_entry_could_match() {
        let (_ht, hs) = history_fixture(&[
            (
                &"a".repeat(40),
                "fix the redelivery hot loop",
                "src/delivery/retry.rs",
                "2026-08-05T00:00:00Z",
            ),
            (
                &"b".repeat(40),
                "rename the pane widget",
                "src/ui/pane.rs",
                "2026-08-05T00:00:00Z",
            ),
        ]);
        let (_it, hybrid) = search_with_history(hs);

        // No entries at all: every hit must come from the history channel.
        let results = hybrid
            .search(&history_opts("redelivery", repo_filter()), &[])
            .expect("search");

        assert_eq!(results.len(), 1, "expected exactly the matching commit");
        assert_eq!(results[0].id, "a".repeat(40));
        assert_eq!(
            results[0].doc_type,
            DocType::HistoryCommit,
            "commits must be typed as commits, not smuggled in as entries"
        );
        assert!(
            results[0].history_score > 0.0,
            "a sign error on SQLite's bm25() cost would show up here"
        );
    }

    #[test]
    fn the_history_channel_stays_silent_when_disabled() {
        let (_ht, hs) = history_fixture(&[(
            &"a".repeat(40),
            "fix the redelivery hot loop",
            "src/delivery/retry.rs",
            "2026-08-05T00:00:00Z",
        )]);
        let (_it, hybrid) = search_with_history(hs);

        let opts = HybridSearchOptions {
            enable_history: false,
            ..history_opts("redelivery", repo_filter())
        };
        assert!(
            hybrid.search(&opts, &[]).expect("search").is_empty(),
            "commits leaked into a search that did not opt in"
        );
    }

    /// Q2/Q3's shape: no query text at all, just structure. A channel that
    /// required a text query would answer "what changed here lately" with
    /// nothing.
    #[test]
    fn a_structural_query_with_no_text_still_returns_commits() {
        let (_ht, hs) = history_fixture(&[
            (
                &"a".repeat(40),
                "delivery change",
                "src/delivery/retry.rs",
                "2026-08-05T00:00:00Z",
            ),
            (
                &"b".repeat(40),
                "ui change",
                "src/ui/pane.rs",
                "2026-08-05T00:00:00Z",
            ),
        ]);
        let (_it, hybrid) = search_with_history(hs);

        let results = hybrid
            .search(
                &history_opts(
                    "",
                    HistoryFilter {
                        path: Some("src/delivery".to_string()),
                        ..repo_filter()
                    },
                ),
                &[],
            )
            .expect("search");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "a".repeat(40));
    }

    /// The index is multi-repo. Without a repository the channel must decline
    /// rather than rank a different checkout's commits into this answer.
    #[test]
    fn the_history_channel_declines_without_a_repository_filter() {
        let (_ht, hs) = history_fixture(&[(
            &"a".repeat(40),
            "redelivery fix",
            "src/delivery/retry.rs",
            "2026-08-05T00:00:00Z",
        )]);
        let (_it, hybrid) = search_with_history(hs);

        let opts = HybridSearchOptions {
            history_filter: None,
            ..history_opts("redelivery", repo_filter())
        };
        assert!(hybrid.search(&opts, &[]).expect("search").is_empty());

        // ...and a *different* repository must not match either.
        let opts = history_opts(
            "redelivery",
            HistoryFilter {
                repository: "/elsewhere".to_string(),
                ..Default::default()
            },
        );
        assert!(hybrid.search(&opts, &[]).expect("search").is_empty());
    }

    /// The cache key must carry the filter. Two searches for the same words
    /// under different paths are different questions, and serving the first
    /// one's answer to the second is a silent wrong answer.
    #[test]
    fn the_history_filter_participates_in_the_cache_key() {
        let base = history_opts("change", repo_filter());
        let narrowed = history_opts(
            "change",
            HistoryFilter {
                path: Some("src/delivery".to_string()),
                ..repo_filter()
            },
        );
        assert_ne!(base.cache_key(), narrowed.cache_key());

        let windowed = history_opts(
            "change",
            HistoryFilter {
                since: Some("2026-08-01T00:00:00Z".to_string()),
                ..repo_filter()
            },
        );
        assert_ne!(base.cache_key(), windowed.cache_key());
    }

    /// End-to-end through the real ranker: a cached result must not be typed
    /// as an entry on the second call.
    #[test]
    fn cached_history_results_keep_their_doc_type() {
        let (_ht, hs) = history_fixture(&[(
            &"a".repeat(40),
            "redelivery fix",
            "src/delivery/retry.rs",
            "2026-08-05T00:00:00Z",
        )]);
        let (_it, hybrid) = search_with_history(hs);
        let opts = history_opts("redelivery", repo_filter());

        let first = hybrid.search(&opts, &[]).expect("search");
        let second = hybrid.search(&opts, &[]).expect("search");
        assert_eq!(first.len(), second.len());
        assert_eq!(second[0].doc_type, DocType::HistoryCommit);
    }

    #[test]
    fn knowledge_results_are_deterministically_ordered() {
        let (_kt, ks) = knowledge_fixture(&[
            ("subsystem", "Verifier One", "verifier gates"),
            ("subsystem", "Verifier Two", "verifier gates"),
            ("subsystem", "Verifier Three", "verifier gates"),
        ]);
        let (_it, hybrid) = search_with_knowledge(ks);

        let opts = HybridSearchOptions {
            base: SearchOptions {
                query: "verifier".to_string(),
                limit: 10,
                ..Default::default()
            },
            enable_knowledge: true,
            enable_temporal: false,
            enable_graph: false,
            ..Default::default()
        };

        // Tied scores must break on id, not on hash-map iteration order.
        let first: Vec<String> = hybrid
            .search(&opts, &[])
            .expect("search")
            .into_iter()
            .map(|r| r.id)
            .collect();
        let second: Vec<String> = hybrid
            .search(&opts, &[])
            .expect("search")
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(first, second, "knowledge ordering is not stable");
    }
}
