//! Cassy-specific hybrid source-code search.
//!
//! Structural SQLite + BM25 search is always available once indexed. The
//! semantic channel is attached only when cloud auth and a non-empty,
//! model-compatible isolated code cache both exist; logged-out lookup neither
//! opens nor creates vector storage.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use cas_search::{
    Bm25Index, CodeSearch, CodeSearchOptions, CodeSearchResult, CodeSearchStats, SearchError,
    VectorStore,
};
use cas_store::{CodeStore, SqliteCodeStore};

use crate::cloud::CloudConfig;
use crate::cloud::embeddings::{
    CODE_KEY_PREFIX, KnowledgeEmbedder, KnowledgeVectorCache, VectorNamespace, is_zero_vector,
};
use crate::error::{MemError, Result};

#[derive(Default)]
struct DisabledVectorStore;

impl VectorStore for DisabledVectorStore {
    fn store(&self, _: &str, _: &[f32]) -> cas_search::Result<()> {
        Ok(())
    }
    fn get(&self, _: &str) -> cas_search::Result<Option<Vec<f32>>> {
        Ok(None)
    }
    fn delete(&self, _: &str) -> cas_search::Result<()> {
        Ok(())
    }
    fn search(&self, _: &[f32], _: usize) -> cas_search::Result<Vec<(String, f32)>> {
        Ok(Vec::new())
    }
    fn exists(&self, _: &str) -> cas_search::Result<bool> {
        Ok(false)
    }
    fn count(&self) -> cas_search::Result<usize> {
        Ok(0)
    }
    fn list_ids(&self) -> cas_search::Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn dimension(&self) -> usize {
        1
    }
}

type PatternCodeSearch = CodeSearch<SqliteCodeStore, DisabledVectorStore, Bm25Index>;

struct CodeSemanticChannel {
    embedder: KnowledgeEmbedder,
    cache: KnowledgeVectorCache,
}

impl CodeSemanticChannel {
    fn open(cas_root: &Path, config: &CloudConfig) -> Option<Self> {
        let embedder = KnowledgeEmbedder::from_config(config)?;
        let cache = KnowledgeVectorCache::open_existing_code(cas_root).ok()??;
        if cache.meta() != &embedder.meta() || cache.count_in(VectorNamespace::Code).ok()? == 0 {
            return None;
        }
        Some(Self { embedder, cache })
    }

    fn search(&self, query: &str, limit: usize) -> Vec<(String, f32)> {
        if query.trim().is_empty() || limit == 0 {
            return Vec::new();
        }
        let Ok(vectors) = self.embedder.embed_batch(&[query.to_string()]) else {
            return Vec::new();
        };
        let Some(vector) = vectors.into_iter().next() else {
            return Vec::new();
        };
        if is_zero_vector(&vector) || vector.len() != self.cache.meta().dims {
            return Vec::new();
        }
        self.cache
            .nearest_in(VectorNamespace::Code, &vector, limit)
            .unwrap_or_default()
    }
}

/// Source-code lookup with an always-local pattern channel and an optional
/// capability-gated semantic channel.
pub struct CasCodeSearch {
    store: Arc<SqliteCodeStore>,
    pattern: PatternCodeSearch,
    semantic: Option<CodeSemanticChannel>,
}

impl CasCodeSearch {
    pub fn has_semantic(&self) -> bool {
        self.semantic.is_some()
    }

    pub fn stats(&self) -> cas_search::Result<CodeSearchStats> {
        self.pattern.stats()
    }

    pub fn search(&self, opts: &CodeSearchOptions) -> cas_search::Result<Vec<CodeSearchResult>> {
        let mut pattern_opts = opts.clone();
        pattern_opts.semantic = false;
        pattern_opts.limit = opts.limit.saturating_mul(3).max(opts.limit);
        let pattern = self.pattern.pattern_search(&pattern_opts)?;

        let semantic = if opts.semantic {
            self.semantic
                .as_ref()
                .map(|channel| channel.search(&opts.query, opts.limit.saturating_mul(5)))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        merge_code_results(self.store.as_ref(), opts, pattern, semantic)
    }
}

fn merge_code_results(
    store: &SqliteCodeStore,
    opts: &CodeSearchOptions,
    pattern: Vec<CodeSearchResult>,
    semantic: Vec<(String, f32)>,
) -> cas_search::Result<Vec<CodeSearchResult>> {
    let mut pattern_by_id: HashMap<String, CodeSearchResult> = pattern
        .into_iter()
        .map(|result| (result.id.clone(), result))
        .collect();
    let semantic_by_id: HashMap<String, f64> = semantic
        .into_iter()
        .filter_map(|(key, score)| {
            key.strip_prefix(CODE_KEY_PREFIX)
                .map(|id| (id.to_string(), f64::from(score).clamp(0.0, 1.0)))
        })
        .collect();

    let mut ids: HashSet<String> = pattern_by_id.keys().cloned().collect();
    ids.extend(semantic_by_id.keys().cloned());
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let symbols = store
        .get_symbols_batch(&id_refs)
        .map_err(|error| SearchError::Storage(error.to_string()))?;

    let query = opts.query.trim().to_lowercase();
    let mut results = Vec::new();
    for symbol in symbols {
        if opts.kind.is_some_and(|kind| symbol.kind != kind)
            || opts
                .language
                .is_some_and(|language| symbol.language != language)
        {
            continue;
        }
        let bm25 = pattern_by_id
            .remove(&symbol.id)
            .map(|result| result.score)
            .unwrap_or(0.0);
        let semantic = semantic_by_id.get(&symbol.id).copied().unwrap_or(0.0);
        let mut score = match (bm25 > 0.0, semantic > 0.0) {
            (true, true) => 0.62 * bm25 + 0.38 * semantic + 0.08,
            (true, false) => bm25,
            (false, true) => 0.78 * semantic,
            (false, false) => 0.0,
        }
        .min(1.0);

        let qualified = symbol.qualified_name.to_lowercase();
        let name = symbol.name.to_lowercase();
        let path = symbol.file_path.to_lowercase();
        let precision = if query == qualified || query == name || query == path {
            1.0
        } else if path.ends_with(&query) {
            0.99
        } else if qualified.contains(&query) || name.contains(&query) {
            0.97
        } else if path.contains(&query) {
            0.95
        } else {
            0.0
        };
        score = score.max(precision);
        if score < f64::from(opts.min_score) {
            continue;
        }
        results.push(CodeSearchResult::from_symbol(
            symbol,
            score,
            opts.include_source,
        ));
    }

    results.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.file_path.cmp(&b.file_path))
    });
    results.dedup_by(|a, b| a.id == b.id);
    results.truncate(opts.limit);
    Ok(results)
}

pub fn open_code_search(cas_root: &Path) -> Result<CasCodeSearch> {
    let config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root)
        .unwrap_or_default();
    open_code_search_with_config(cas_root, &config)
}

pub fn open_code_search_fast(cas_root: &Path) -> Result<CasCodeSearch> {
    open_code_search(cas_root)
}

fn open_code_search_with_config(cas_root: &Path, config: &CloudConfig) -> Result<CasCodeSearch> {
    let store = Arc::new(SqliteCodeStore::open(cas_root)?);
    let bm25 = Arc::new(
        Bm25Index::open(&cas_root.join("index").join("code"))
            .map_err(|error| MemError::Other(format!("Failed to open code BM25 index: {error}")))?,
    );
    let pattern = CodeSearch::new(
        Arc::clone(&store),
        Arc::new(DisabledVectorStore),
        bm25,
        None,
    );
    Ok(CasCodeSearch {
        store,
        pattern,
        semantic: CodeSemanticChannel::open(cas_root, config),
    })
}

pub fn code_search_available(cas_root: &Path) -> bool {
    let dir = cas_root.join("index").join("code");
    dir.exists() && dir.join("meta.json").exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_code::{CodeFile, CodeSymbol, Language, SymbolKind};
    use chrono::Utc;
    use tempfile::TempDir;

    fn seed_symbol(store: &SqliteCodeStore, id: &str, name: &str, path: &str) -> CodeSymbol {
        let now = Utc::now();
        let file_id = format!("file-{id}");
        store
            .add_file(&CodeFile {
                id: file_id.clone(),
                path: path.into(),
                repository: "repo".into(),
                language: Language::Rust,
                size: 10,
                line_count: 1,
                commit_hash: None,
                content_hash: format!("file-hash-{id}"),
                created: now,
                updated: now,
                scope: "project".into(),
            })
            .unwrap();
        let symbol = CodeSymbol {
            id: id.into(),
            qualified_name: format!("cache::{name}"),
            name: name.into(),
            kind: SymbolKind::Function,
            language: Language::Rust,
            file_path: path.into(),
            file_id,
            line_start: 1,
            line_end: 1,
            source: format!("fn {name}() {{}}"),
            documentation: None,
            signature: Some(format!("fn {name}()")),
            parent_id: None,
            repository: "repo".into(),
            commit_hash: None,
            created: now,
            updated: now,
            content_hash: format!("symbol-hash-{id}"),
            scope: "project".into(),
        };
        store.add_symbol(&symbol).unwrap();
        symbol
    }

    #[test]
    fn code_search_available_false_when_no_index() {
        let temp = TempDir::new().unwrap();
        assert!(!code_search_available(temp.path()));
    }

    #[test]
    fn logged_out_search_never_opens_a_vector_cache() {
        let temp = TempDir::new().unwrap();
        let config = CloudConfig {
            token: None,
            ..Default::default()
        };
        let search = open_code_search_with_config(temp.path(), &config).unwrap();
        assert!(!search.has_semantic());
        let _ = search.search(&CodeSearchOptions {
            query: "natural language concept".into(),
            limit: 10,
            semantic: true,
            ..Default::default()
        });
        assert!(!KnowledgeVectorCache::code_cache_dir(temp.path()).exists());
        assert!(!temp.path().join("vectors_code.lmdb").exists());
    }

    #[test]
    fn fixed_eval_paraphrase_recall_and_exact_precision() {
        let temp = TempDir::new().unwrap();
        let store = SqliteCodeStore::open(temp.path()).unwrap();
        let exact = seed_symbol(&store, "exact", "refresh_cache", "src/cache.rs");
        let concept = seed_symbol(&store, "concept", "evict_stale", "src/expiry.rs");

        let paraphrase = merge_code_results(
            &store,
            &CodeSearchOptions {
                query: "remove entries after expiry".into(),
                limit: 5,
                semantic: true,
                ..Default::default()
            },
            Vec::new(),
            vec![(format!("{CODE_KEY_PREFIX}{}", concept.id), 0.96)],
        )
        .unwrap();
        assert_eq!(paraphrase[0].id, concept.id, "semantic paraphrase missed");

        let exact_query = merge_code_results(
            &store,
            &CodeSearchOptions {
                query: "refresh_cache".into(),
                limit: 5,
                semantic: true,
                ..Default::default()
            },
            vec![CodeSearchResult::from_symbol(exact.clone(), 0.85, false)],
            vec![(format!("{CODE_KEY_PREFIX}{}", concept.id), 1.0)],
        )
        .unwrap();
        assert_eq!(exact_query[0].id, exact.id, "exact symbol lost precision");
        assert_eq!(exact_query[0].score, 1.0);
    }

    #[tokio::test]
    async fn semantic_query_reaches_paraphrased_symbol_that_bm25_cannot_name() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let mut query_vector = vec![0.0; 1024];
        query_vector[17] = 1.0;
        Mock::given(method("POST"))
            .and(path("/api/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "embeddings": [query_vector]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let temp = TempDir::new().unwrap();
        let store = SqliteCodeStore::open(temp.path()).unwrap();
        let concept = seed_symbol(&store, "concept", "evict_stale", "src/expiry.rs");
        let embedder = KnowledgeEmbedder::new(server.uri(), "token");
        let cache = KnowledgeVectorCache::open_code(temp.path(), embedder.meta()).unwrap();
        let mut symbol_vector = vec![0.0; 1024];
        symbol_vector[17] = 1.0;
        cache
            .put(
                &crate::cloud::embeddings::code_symbol_key(&concept.id),
                &symbol_vector,
            )
            .unwrap();
        let config = CloudConfig {
            endpoint: server.uri(),
            token: Some("token".into()),
            ..Default::default()
        };
        let search = open_code_search_with_config(temp.path(), &config).unwrap();
        assert!(search.has_semantic());
        let results = search
            .search(&CodeSearchOptions {
                query: "remove entries after their expiration deadline".into(),
                limit: 5,
                semantic: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(results[0].id, concept.id);
    }
}
