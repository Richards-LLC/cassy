//! Capability-gated cloud embeddings for distilled knowledge pages (T5).
//!
//! # The boundary this module defends
//!
//! Local SQLite is the **source of truth** for project knowledge: pages, bodies,
//! provenance and the `locked` bit all live on disk and are fully functional with
//! no account, no network and no cloud build. The cloud contributes exactly two
//! optional things: (1) embedding vectors, computed here and cached locally, and
//! (2) team distribution of pages via the existing `/api/sync` endpoints. Nothing
//! in the retrieval path may *require* either one.
//!
//! # Capability gating
//!
//! [`KnowledgeEmbedder::from_config`] returns `None` whenever the user is not
//! logged in. `None` is not a degraded mode with zeroed vectors — it means the
//! semantic channel reports itself absent ([`HybridSearch::has_semantic`] stays
//! false, and T3's weight redistribution hands that channel's mass to the live
//! ones). No LMDB environment is created, no directory is touched, and no HTTP
//! request is made. This mirrors the `dims = 0` pattern documented in the
//! TencentDB agent-memory review: a provider-absent deployment must not
//! materialise vector storage it will never fill.
//!
//! # Two invariants worth stating out loud
//!
//! 1. **Never cache a zero vector.** A provider that fails soft (returns an
//!    all-zero row rather than an error) would otherwise poison the cache with
//!    a vector that is equidistant from every query. Zero vectors are rejected
//!    at the boundary and the page stays `pending_embedding = 1`, so the next
//!    run retries it instead of silently believing it is done.
//! 2. **Auto-reindex when the embedding model changes.** Vectors from two
//!    different models are not comparable, so mixing them silently corrupts
//!    ranking. The cache records `{provider, model, dims}` alongside the
//!    vectors; on mismatch the whole cache is dropped and every page is marked
//!    `pending_embedding` again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cloud::CloudConfig;
use crate::error::CasError;
use cas_search::VectorStore;
use cas_search::lmdb_store::LmdbVectorStore;
use cas_store::{KnowledgePage, KnowledgeStore};

/// Provider name recorded in [`EmbeddingMeta`] for vectors produced here.
pub const CLOUD_PROVIDER: &str = "cas-cloud";

/// Model requested from the cloud endpoint when the config does not name one.
pub const DEFAULT_EMBEDDING_MODEL: &str = "cas-embed-v1";

/// Dimension the default model returns. The first successful response is
/// authoritative — this is only the value used to open the cache before any
/// response has been seen.
pub const DEFAULT_EMBEDDING_DIMS: usize = 1024;

/// Default cap on pages embedded per invocation, so a first run on a large
/// repo cannot turn into an unbounded burst of cloud calls.
pub const DEFAULT_EMBED_BATCH: usize = 32;

/// Identity of the embedding space a cached vector belongs to.
///
/// Vectors are only comparable within one `(provider, model, dims)` triple.
/// Persisted next to the cache so a model swap is detectable rather than
/// silently corrupting similarity scores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingMeta {
    pub provider: String,
    pub model: String,
    pub dims: usize,
}

impl EmbeddingMeta {
    pub fn new(provider: impl Into<String>, model: impl Into<String>, dims: usize) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            dims,
        }
    }
}

/// What [`embed_pending_pages`] actually did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmbedReport {
    /// Pages whose vector was computed and cached.
    pub embedded: usize,
    /// Pages the provider answered with an unusable (zero) vector. These stay
    /// `pending_embedding` and are retried next run.
    pub rejected_zero: usize,
    /// Pages skipped because the provider returned the wrong dimension.
    pub rejected_dims: usize,
    /// True when the cache was wiped because the embedding model changed.
    pub reindexed: bool,
    /// Per-page failures (id, message). Non-fatal: other pages still proceed.
    pub errors: Vec<(String, String)>,
}

/// A client for the cloud embedding endpoint.
///
/// Construct via [`KnowledgeEmbedder::from_config`], which is the capability
/// gate: no token, no embedder, no cloud calls.
#[derive(Debug, Clone)]
pub struct KnowledgeEmbedder {
    endpoint: String,
    token: String,
    model: String,
    dims: usize,
    timeout: Duration,
}

impl KnowledgeEmbedder {
    /// The capability gate. `None` means "this installation has no semantic
    /// channel", which is a first-class supported state, not an error.
    pub fn from_config(config: &CloudConfig) -> Option<Self> {
        if !config.is_logged_in() {
            return None;
        }
        let token = config.token.clone()?;
        Some(Self {
            endpoint: config.endpoint.clone(),
            token,
            model: DEFAULT_EMBEDDING_MODEL.to_string(),
            dims: DEFAULT_EMBEDDING_DIMS,
            timeout: Duration::from_secs(30),
        })
    }

    /// Build an embedder against an explicit endpoint/token (tests, and any
    /// caller that already resolved credentials).
    pub fn new(endpoint: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            token: token.into(),
            model: DEFAULT_EMBEDDING_MODEL.to_string(),
            dims: DEFAULT_EMBEDDING_DIMS,
            timeout: Duration::from_secs(30),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>, dims: usize) -> Self {
        self.model = model.into();
        self.dims = dims;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn meta(&self) -> EmbeddingMeta {
        EmbeddingMeta::new(CLOUD_PROVIDER, self.model.clone(), self.dims)
    }

    pub fn dims(&self) -> usize {
        self.dims
    }

    /// `POST {endpoint}/api/embeddings` with `{model, input: [..]}`.
    ///
    /// Accepts both the flat shape (`{"embeddings": [[..]]}`) and the
    /// OpenAI-compatible shape (`{"data": [{"embedding": [..]}]}`) so the
    /// client does not have to be re-released if the cloud settles on the
    /// other one.
    pub fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CasError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let url = format!("{}/api/embeddings", self.endpoint);
        let payload = serde_json::json!({
            "model": self.model,
            "input": texts,
        });

        let response = ureq::post(&url)
            .timeout(self.timeout)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .send_json(payload);

        let body: serde_json::Value = match response {
            Ok(resp) => resp
                .into_json()
                .map_err(|e| CasError::Other(format!("Embedding response parse failed: {e}")))?,
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                return Err(CasError::Other(format!(
                    "Embedding request failed with status {code}: {body}"
                )));
            }
            Err(ureq::Error::Transport(e)) => {
                return Err(CasError::Other(format!("Network error: {e}")));
            }
        };

        let vectors = parse_embedding_response(&body).ok_or_else(|| {
            CasError::Other(
                "Embedding response had neither `embeddings` nor `data[].embedding`".to_string(),
            )
        })?;

        if vectors.len() != texts.len() {
            return Err(CasError::Other(format!(
                "Embedding provider returned {} vectors for {} inputs",
                vectors.len(),
                texts.len()
            )));
        }

        Ok(vectors)
    }
}

/// Extract vectors from either supported response shape.
pub fn parse_embedding_response(body: &serde_json::Value) -> Option<Vec<Vec<f32>>> {
    if let Some(list) = body.get("embeddings").and_then(|v| v.as_array()) {
        return Some(list.iter().map(json_to_vector).collect());
    }
    if let Some(list) = body.get("data").and_then(|v| v.as_array()) {
        return Some(
            list.iter()
                .map(|item| {
                    item.get("embedding")
                        .map(json_to_vector)
                        .unwrap_or_default()
                })
                .collect(),
        );
    }
    None
}

fn json_to_vector(value: &serde_json::Value) -> Vec<f32> {
    value
        .as_array()
        .map(|nums| {
            nums.iter()
                .map(|n| n.as_f64().unwrap_or(0.0) as f32)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a vector carries no signal (all components zero, or empty).
///
/// A zero vector has cosine similarity 0 against everything, so caching one
/// is worse than caching nothing: the page looks embedded but can never be
/// retrieved semantically.
pub fn is_zero_vector(vector: &[f32]) -> bool {
    vector.is_empty() || vector.iter().all(|v| *v == 0.0)
}

/// Cosine similarity. Returns 0.0 for mismatched or zero-magnitude inputs
/// rather than NaN, so a corrupt row can never sort to the top.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a <= 0.0 || norm_b <= 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

/// Local vector cache for knowledge pages, tagged with the embedding space it
/// belongs to.
///
/// Deliberately NOT opened unless a [`KnowledgeEmbedder`] exists: opening it
/// creates an LMDB environment on disk, which a provider-absent installation
/// should never pay for.
pub struct KnowledgeVectorCache {
    store: Arc<LmdbVectorStore>,
    meta: EmbeddingMeta,
    root: PathBuf,
    /// True when opening wiped a cache built by a different model.
    reindexed: bool,
}

/// Process-wide registry of open LMDB environments, keyed by path.
///
/// Not an optimization — a correctness requirement. LMDB refuses to open the
/// same environment twice in one process ("environment already open in this
/// program"), and this cache legitimately has two openers: `cas cloud sync`
/// (writing new vectors) and the retrieval path (reading them). Without the
/// registry the second one fails at runtime, which is exactly the kind of
/// defect that only shows up once both features are enabled together.
static OPEN_ENVS: OnceLock<Mutex<HashMap<PathBuf, Arc<LmdbVectorStore>>>> = OnceLock::new();

fn open_envs() -> &'static Mutex<HashMap<PathBuf, Arc<LmdbVectorStore>>> {
    OPEN_ENVS.get_or_init(|| Mutex::new(HashMap::new()))
}

impl KnowledgeVectorCache {
    /// Directory holding the cache for a given CAS root.
    pub fn cache_dir(cas_root: &Path) -> PathBuf {
        cas_root.join("index").join("knowledge-vectors")
    }

    fn meta_path(dir: &Path) -> PathBuf {
        dir.join("embedding_meta.json")
    }

    /// Open (creating if needed) the cache for `meta`.
    ///
    /// If a cache exists for a *different* `(provider, model, dims)` triple it
    /// is destroyed first: vectors from two models are not comparable, and
    /// keeping them would silently corrupt ranking. Callers should re-mark
    /// pages pending when [`Self::reindexed`] is true.
    pub fn open(cas_root: &Path, meta: EmbeddingMeta) -> Result<Self, CasError> {
        let dir = Self::cache_dir(cas_root);
        let mut reindexed = false;

        let mut envs = open_envs().lock().unwrap_or_else(|p| p.into_inner());

        let existing = Self::read_meta(&dir);
        if let Some(existing) = existing {
            if existing != meta {
                // Drop our handle first: the directory is about to disappear
                // and a registry entry pointing at deleted files would hand
                // the next caller a store backed by nothing.
                envs.remove(&dir);
                std::fs::remove_dir_all(&dir).map_err(|e| {
                    CasError::Other(format!("Failed to clear stale embedding cache: {e}"))
                })?;
                reindexed = true;
            }
        }

        std::fs::create_dir_all(&dir)
            .map_err(|e| CasError::Other(format!("Failed to create embedding cache dir: {e}")))?;

        let store = match envs.get(&dir) {
            Some(store) if store.dimension() == meta.dims => Arc::clone(store),
            _ => {
                let store = Arc::new(LmdbVectorStore::open(&dir, meta.dims).map_err(|e| {
                    CasError::Other(format!("Failed to open embedding cache: {e}"))
                })?);
                envs.insert(dir.clone(), Arc::clone(&store));
                store
            }
        };
        drop(envs);

        Self::write_meta(&dir, &meta)?;

        Ok(Self {
            store,
            meta,
            root: dir,
            reindexed,
        })
    }

    fn read_meta(dir: &Path) -> Option<EmbeddingMeta> {
        let raw = std::fs::read_to_string(Self::meta_path(dir)).ok()?;
        serde_json::from_str(&raw).ok()
    }

    fn write_meta(dir: &Path, meta: &EmbeddingMeta) -> Result<(), CasError> {
        let raw = serde_json::to_string_pretty(meta)
            .map_err(|e| CasError::Other(format!("Failed to serialize embedding meta: {e}")))?;
        std::fs::write(Self::meta_path(dir), raw)
            .map_err(|e| CasError::Other(format!("Failed to write embedding meta: {e}")))
    }

    pub fn meta(&self) -> &EmbeddingMeta {
        &self.meta
    }

    pub fn reindexed(&self) -> bool {
        self.reindexed
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Cache a vector. Rejects zero vectors and dimension mismatches so the
    /// caller can leave the page pending instead of marking it done.
    pub fn put(&self, id: &str, vector: &[f32]) -> Result<(), CasError> {
        if is_zero_vector(vector) {
            return Err(CasError::Other(format!(
                "refusing to cache a zero vector for {id}"
            )));
        }
        if vector.len() != self.meta.dims {
            return Err(CasError::Other(format!(
                "embedding for {id} has {} dims, cache expects {}",
                vector.len(),
                self.meta.dims
            )));
        }
        self.store
            .store(id, vector)
            .map_err(|e| CasError::Other(format!("Failed to cache embedding for {id}: {e}")))
    }

    pub fn get(&self, id: &str) -> Result<Option<Vec<f32>>, CasError> {
        self.store
            .get(id)
            .map_err(|e| CasError::Other(format!("Failed to read cached embedding: {e}")))
    }

    pub fn count(&self) -> Result<usize, CasError> {
        self.store
            .count()
            .map_err(|e| CasError::Other(format!("Failed to count cached embeddings: {e}")))
    }

    pub fn delete(&self, id: &str) -> Result<(), CasError> {
        self.store
            .delete(id)
            .map_err(|e| CasError::Other(format!("Failed to delete cached embedding: {e}")))
    }

    /// Brute-force k-nearest-neighbour over the cached vectors.
    ///
    /// Brute force is deliberate, not a placeholder: `LmdbVectorStore::search`
    /// returns an error by design (it is a key-value store, not an ANN index),
    /// and a distilled-knowledge corpus is pages, not millions of rows. Ties
    /// break on id so results are stable across runs.
    pub fn nearest(&self, query: &[f32], k: usize) -> Result<Vec<(String, f32)>, CasError> {
        if is_zero_vector(query) || k == 0 {
            return Ok(Vec::new());
        }
        let ids = self
            .store
            .list_ids()
            .map_err(|e| CasError::Other(format!("Failed to list cached embeddings: {e}")))?;

        let mut scored: Vec<(String, f32)> = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(vector) = self.get(&id)? {
                let score = cosine_similarity(query, &vector);
                if score > 0.0 {
                    scored.push((id, score));
                }
            }
        }
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        scored.truncate(k);
        Ok(scored)
    }
}

/// Text sent to the embedding provider for a page: title, snippet and body.
///
/// Kept in one place so the query side and the index side cannot drift into
/// embedding different things.
pub fn page_embedding_text(page: &KnowledgePage, body: &str) -> String {
    format!("{}\n\n{}\n\n{}", page.title, page.snippet, body)
}

/// Embed up to `limit` pages that are still awaiting a vector.
///
/// Failure of one page never aborts the batch: it is recorded in
/// [`EmbedReport::errors`] and the page keeps its `pending_embedding` flag.
pub fn embed_pending_pages(
    store: &dyn KnowledgeStore,
    embedder: &KnowledgeEmbedder,
    cache: &KnowledgeVectorCache,
    limit: usize,
) -> Result<EmbedReport, CasError> {
    let mut report = EmbedReport {
        reindexed: cache.reindexed(),
        ..Default::default()
    };

    // A model change invalidates every cached vector, so everything must be
    // re-embedded — not just the pages that happened to be pending.
    if cache.reindexed() {
        store
            .mark_all_pending_embedding()
            .map_err(|e| CasError::Other(format!("Failed to re-mark pages for embedding: {e}")))?;
    }

    let pages = store
        .list_pending_embedding(limit)
        .map_err(|e| CasError::Other(format!("Failed to list pending pages: {e}")))?;
    if pages.is_empty() {
        return Ok(report);
    }

    let mut texts = Vec::with_capacity(pages.len());
    for page in &pages {
        let body = store.read_body(&page.rel_path).unwrap_or_default();
        texts.push(page_embedding_text(page, &body));
    }

    let vectors = embedder.embed_batch(&texts)?;

    for (page, vector) in pages.iter().zip(vectors.iter()) {
        if is_zero_vector(vector) {
            report.rejected_zero += 1;
            continue;
        }
        if vector.len() != cache.meta().dims {
            report.rejected_dims += 1;
            continue;
        }
        match cache.put(&page.id, vector) {
            Ok(()) => match store.mark_embedded(&page.id) {
                Ok(()) => report.embedded += 1,
                Err(e) => report.errors.push((page.id.clone(), e.to_string())),
            },
            Err(e) => report.errors.push((page.id.clone(), e.to_string())),
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::{IngestBatch, PageWrite, SqliteKnowledgeStore};

    fn config_logged_out() -> CloudConfig {
        CloudConfig {
            token: None,
            ..Default::default()
        }
    }

    fn config_logged_in(endpoint: &str) -> CloudConfig {
        CloudConfig {
            endpoint: endpoint.to_string(),
            token: Some("test-token".to_string()),
            ..Default::default()
        }
    }

    fn seed_store(root: &Path, titles: &[&str]) -> SqliteKnowledgeStore {
        let store = SqliteKnowledgeStore::open(root).unwrap();
        store.init().unwrap();
        let pages: Vec<PageWrite> = titles
            .iter()
            .enumerate()
            .map(|(i, title)| {
                let mut page = KnowledgePage::new(format!("cas-kn00{i}"), "architecture", *title);
                page.snippet = format!("snippet for {title}");
                PageWrite {
                    page,
                    body: format!("body of {title}"),
                }
            })
            .collect();
        store
            .commit_ingest(&IngestBatch {
                pages,
                sources: Vec::new(),
                tombstones: Vec::new(),
            })
            .unwrap();
        store
    }

    #[test]
    fn capability_gate_returns_none_without_auth() {
        assert!(KnowledgeEmbedder::from_config(&config_logged_out()).is_none());
    }

    #[test]
    fn capability_gate_returns_some_with_auth() {
        let embedder = KnowledgeEmbedder::from_config(&config_logged_in("https://example.test"));
        assert!(embedder.is_some());
        assert_eq!(embedder.unwrap().meta().provider, CLOUD_PROVIDER);
    }

    #[test]
    fn no_auth_never_materialises_the_vector_cache_directory() {
        // The provider-absent invariant: nothing on disk, not even an empty
        // LMDB environment, when there is no embedder to fill it.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(KnowledgeEmbedder::from_config(&config_logged_out()).is_none());
        assert!(
            !KnowledgeVectorCache::cache_dir(root).exists(),
            "vector cache dir must not exist when no embedder is configured"
        );
    }

    #[test]
    fn zero_vectors_are_never_cached() {
        let tmp = tempfile::tempdir().unwrap();
        let cache =
            KnowledgeVectorCache::open(tmp.path(), EmbeddingMeta::new("p", "m", 4)).unwrap();
        let err = cache.put("cas-kn001", &[0.0, 0.0, 0.0, 0.0]).unwrap_err();
        assert!(err.to_string().contains("zero vector"), "got: {err}");
        assert_eq!(cache.count().unwrap(), 0);
    }

    #[test]
    fn dimension_mismatch_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let cache =
            KnowledgeVectorCache::open(tmp.path(), EmbeddingMeta::new("p", "m", 4)).unwrap();
        let err = cache.put("cas-kn001", &[1.0, 2.0]).unwrap_err();
        assert!(err.to_string().contains("dims"), "got: {err}");
    }

    #[test]
    fn model_change_wipes_the_cache_and_flags_reindex() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let cache =
                KnowledgeVectorCache::open(tmp.path(), EmbeddingMeta::new("p", "m1", 4)).unwrap();
            cache.put("cas-kn001", &[1.0, 0.0, 0.0, 0.0]).unwrap();
            assert_eq!(cache.count().unwrap(), 1);
            assert!(!cache.reindexed());
        }
        // Same provider + dims, different model: still a different space.
        let cache =
            KnowledgeVectorCache::open(tmp.path(), EmbeddingMeta::new("p", "m2", 4)).unwrap();
        assert!(cache.reindexed(), "model change must trigger a reindex");
        assert_eq!(
            cache.count().unwrap(),
            0,
            "vectors from the old model must not survive"
        );
    }

    #[test]
    fn reopening_with_the_same_meta_preserves_vectors() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let cache =
                KnowledgeVectorCache::open(tmp.path(), EmbeddingMeta::new("p", "m1", 4)).unwrap();
            cache.put("cas-kn001", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        }
        let cache =
            KnowledgeVectorCache::open(tmp.path(), EmbeddingMeta::new("p", "m1", 4)).unwrap();
        assert!(!cache.reindexed());
        assert_eq!(cache.count().unwrap(), 1);
    }

    #[test]
    fn nearest_ranks_by_cosine_and_is_stable() {
        let tmp = tempfile::tempdir().unwrap();
        let cache =
            KnowledgeVectorCache::open(tmp.path(), EmbeddingMeta::new("p", "m", 3)).unwrap();
        cache.put("cas-kn001", &[1.0, 0.0, 0.0]).unwrap();
        cache.put("cas-kn002", &[0.9, 0.1, 0.0]).unwrap();
        cache.put("cas-kn003", &[0.0, 1.0, 0.0]).unwrap();

        let hits = cache.nearest(&[1.0, 0.0, 0.0], 2).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, "cas-kn001");
        assert_eq!(hits[1].0, "cas-kn002");

        let again = cache.nearest(&[1.0, 0.0, 0.0], 2).unwrap();
        assert_eq!(hits, again, "kNN order must be stable across calls");
    }

    #[test]
    fn nearest_on_a_zero_query_returns_nothing_rather_than_noise() {
        let tmp = tempfile::tempdir().unwrap();
        let cache =
            KnowledgeVectorCache::open(tmp.path(), EmbeddingMeta::new("p", "m", 3)).unwrap();
        cache.put("cas-kn001", &[1.0, 0.0, 0.0]).unwrap();
        assert!(cache.nearest(&[0.0, 0.0, 0.0], 5).unwrap().is_empty());
    }

    #[test]
    fn parses_flat_and_openai_response_shapes() {
        let flat = serde_json::json!({"embeddings": [[1.0, 2.0], [3.0, 4.0]]});
        assert_eq!(
            parse_embedding_response(&flat).unwrap(),
            vec![vec![1.0, 2.0], vec![3.0, 4.0]]
        );
        let openai = serde_json::json!({"data": [{"embedding": [1.0, 2.0]}]});
        assert_eq!(
            parse_embedding_response(&openai).unwrap(),
            vec![vec![1.0, 2.0]]
        );
        assert!(parse_embedding_response(&serde_json::json!({"oops": 1})).is_none());
    }

    #[tokio::test]
    async fn embeds_pending_pages_against_a_mocked_cloud() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embeddings"))
            .and(header("Authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "embeddings": [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]]
            })))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let report = tokio::task::spawn_blocking(move || {
            let store = seed_store(&root, &["Build System", "Retrieval"]);
            let embedder = KnowledgeEmbedder::from_config(&config_logged_in(&endpoint))
                .expect("auth present => embedder")
                .with_model("test-model", 4);
            let cache = KnowledgeVectorCache::open(&root, embedder.meta()).unwrap();
            let report = embed_pending_pages(&store, &embedder, &cache, 10).unwrap();
            (
                report,
                cache.count().unwrap(),
                store.list_pending_embedding(10).unwrap().len(),
            )
        })
        .await
        .unwrap();

        let (report, cached, still_pending) = report;
        assert_eq!(report.embedded, 2);
        assert_eq!(report.rejected_zero, 0);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        assert_eq!(cached, 2, "both vectors must be cached locally");
        assert_eq!(still_pending, 0, "pages must be marked embedded");
    }

    #[tokio::test]
    async fn a_zero_vector_from_the_provider_leaves_the_page_pending() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "embeddings": [[0.0, 0.0, 0.0, 0.0]]
            })))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let (report, cached, still_pending) = tokio::task::spawn_blocking(move || {
            let store = seed_store(&root, &["Build System"]);
            let embedder = KnowledgeEmbedder::new(&endpoint, "test-token").with_model("m", 4);
            let cache = KnowledgeVectorCache::open(&root, embedder.meta()).unwrap();
            let report = embed_pending_pages(&store, &embedder, &cache, 10).unwrap();
            (
                report,
                cache.count().unwrap(),
                store.list_pending_embedding(10).unwrap().len(),
            )
        })
        .await
        .unwrap();

        assert_eq!(report.embedded, 0);
        assert_eq!(report.rejected_zero, 1);
        assert_eq!(cached, 0, "a zero vector must never reach the cache");
        assert_eq!(
            still_pending, 1,
            "the page must stay pending so the next run retries it"
        );
    }
}
