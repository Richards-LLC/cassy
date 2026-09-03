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

/// Hard cap on how many inputs the cloud endpoint accepts in ONE request.
///
/// Server contract: more than this is a `400`, always. This is the number
/// [`embed_pending_pages`] chunks on — it is a wire-protocol constant, not a
/// tuning knob, and no caller may pass a longer `input` list.
pub const MAX_EMBED_INPUTS_PER_REQUEST: usize = 32;

/// Default cap on pages embedded per invocation, so a first run on a large
/// repo cannot turn into an unbounded burst of cloud calls.
///
/// This is a *page* budget, not a request size: [`embed_pending_pages`] splits
/// the pages it fetched into chunks of [`MAX_EMBED_INPUTS_PER_REQUEST`], so
/// this many pages costs `ceil(n / 32)` requests. The two used to be the same
/// number by coincidence, which hid the fact that nothing chunked at all
/// (cas-a924).
pub const DEFAULT_EMBED_BATCH: usize = 512;

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

/// Why an embedding request did not produce vectors.
///
/// The split is load-bearing: `Unsupported` is a boundary of the installation
/// (this endpoint has no embedding capability) and should be reported as such,
/// while `Failed` is a real error that must never be swallowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedError {
    /// The endpoint does not implement `/api/embeddings` (404/501).
    Unsupported(String),
    /// Auth, rate limit, transport or malformed-response failure. Retrying the
    /// same request later can succeed, so the drain defers rather than discards.
    Failed(String),
    /// The provider refused this input and will refuse it again: the payload
    /// itself is the problem (over the model's token cap, malformed, wrong
    /// type). Retrying is guaranteed to fail, so the drain must isolate the
    /// offending unit instead of halting the corpus behind it (GH #695).
    Rejected(String),
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbedError::Unsupported(m) | EmbedError::Failed(m) | EmbedError::Rejected(m) => {
                write!(f, "{m}")
            }
        }
    }
}

impl std::error::Error for EmbedError {}

impl From<EmbedError> for CasError {
    fn from(e: EmbedError) -> Self {
        CasError::Other(e.to_string())
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
    /// Embedding requests actually issued this run. With chunking this is
    /// `ceil(pages / MAX_EMBED_INPUTS_PER_REQUEST)`, so a caller (or a test)
    /// can see that chunking happened rather than inferring it.
    pub requests: usize,
    /// Pages that never reached the provider because a request failed and the
    /// run stopped issuing more. They keep `pending_embedding`.
    pub deferred: usize,
    /// Request-level failures, verbatim. A non-empty vector means this run did
    /// NOT do what it was asked to; callers must surface it, never swallow it.
    pub request_errors: Vec<String>,
    /// True when the endpoint answered "no such capability" (404/501) rather
    /// than failing. A boundary to report, not an error to alarm about.
    pub capability_absent: bool,
    /// Pages still awaiting an embedding once this run finished — including
    /// pages beyond this run's `limit`. This is the number that must drain to
    /// zero across runs; it is the honest measure of coverage.
    pub pending_after: usize,
    /// Units deliberately excluded from embedding and retired from the queue
    /// without a vector — merge commits whose whole message is
    /// `Merge branch 'x'` (spec §7.1, §12 Q5). Counted rather than silent, so
    /// "embedded + skipped < listed" is always explainable.
    pub skipped: usize,
    /// Units the provider refused and this run retired from the queue with the
    /// refusal recorded. Unlike `deferred` these will never be retried on their
    /// own: something about the payload has to change first.
    pub quarantined: usize,
    /// `(id, provider message)` for each quarantined unit, so the reason is
    /// reportable rather than only countable.
    pub quarantine_errors: Vec<(String, String)>,
}

impl EmbedReport {
    /// True when this run left work undone for a reason the user should see.
    pub fn had_trouble(&self) -> bool {
        !self.request_errors.is_empty()
            || !self.errors.is_empty()
            || self.rejected_zero > 0
            || self.rejected_dims > 0
    }
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
    /// The response shape is the flat single-key `{"embeddings": [[..]]}` — the
    /// cloud team has committed to it as the contract and will not emit the
    /// OpenAI-compatible `data[].embedding` envelope, so the client no longer
    /// accepts that shape (cas-a924).
    ///
    /// `texts` must not exceed [`MAX_EMBED_INPUTS_PER_REQUEST`]; the server
    /// rejects a longer input list with `400`. Callers that may have more work
    /// than that should go through [`embed_pending_pages`], which chunks.
    pub fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        if texts.len() > MAX_EMBED_INPUTS_PER_REQUEST {
            return Err(EmbedError::Failed(format!(
                "refusing to send {} inputs in one embedding request: the endpoint caps a request at {MAX_EMBED_INPUTS_PER_REQUEST}",
                texts.len()
            )));
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
                .map_err(|e| EmbedError::Failed(format!("Embedding response parse failed: {e}")))?,
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                // 404/501 is not a failure of this run: it says the endpoint
                // has no embedding capability at all. Classified separately so
                // the caller can report a boundary instead of an error.
                if code == 404 || code == 501 {
                    return Err(EmbedError::Unsupported(format!(
                        "endpoint {} does not provide /api/embeddings (status {code})",
                        self.endpoint
                    )));
                }
                if is_provider_rejection(code, &body) {
                    return Err(EmbedError::Rejected(format!(
                        "Embedding request rejected with status {code}: {body}"
                    )));
                }
                return Err(EmbedError::Failed(format!(
                    "Embedding request failed with status {code}: {body}"
                )));
            }
            Err(ureq::Error::Transport(e)) => {
                return Err(EmbedError::Failed(format!("Network error: {e}")));
            }
        };

        let vectors = parse_embedding_response(&body).ok_or_else(|| {
            EmbedError::Failed("Embedding response had no `embeddings` array".to_string())
        })?;

        if vectors.len() != texts.len() {
            return Err(EmbedError::Failed(format!(
                "Embedding provider returned {} vectors for {} inputs",
                vectors.len(),
                texts.len()
            )));
        }

        Ok(vectors)
    }
}

/// Longest text this drain will send for one unit.
///
/// The provider's model caps input at 8,192 tokens. GH #695 measured the cliff
/// on real commits: 34,139 chars embedded, 43,392 chars was refused — roughly
/// four chars per token. 24,000 chars (~6k tokens) sits comfortably under it
/// with room for the tokenizer's worst case on dense text.
///
/// Truncating beats quarantining here. A squash-merge commit whose body
/// concatenates 2,800 lines of sub-commit messages still has its subject and
/// leading summary in the first 24k chars, so a truncated vector is a useful
/// answer to "what was this commit about"; no vector at all is not. Quarantine
/// remains the sink for whatever the provider still refuses.
pub const MAX_EMBED_TEXT_CHARS: usize = 24_000;

/// Cut `text` to [`MAX_EMBED_TEXT_CHARS`] on a char boundary.
///
/// Char-counted, not byte-sliced: a naive `&text[..n]` panics mid-codepoint on
/// any commit message with an emoji or accented name in it.
pub fn cap_embedding_text(text: String) -> String {
    if text.chars().count() <= MAX_EMBED_TEXT_CHARS {
        return text;
    }
    text.chars().take(MAX_EMBED_TEXT_CHARS).collect()
}

/// Is this HTTP answer the provider refusing the payload, rather than the
/// service being unavailable?
///
/// Two shapes count, because the deployed gateway currently emits the second:
///
/// 1. A direct client-error status — 400/413/422 — which by definition says the
///    request itself is unacceptable.
/// 2. Any status whose body names a provider 4xx, e.g.
///    `502 {"error":"Embedding provider returned 400"}`. The cloud wraps the
///    upstream 400 as a gateway error (petra-stella-cloud#63 tracks the server
///    half), so status alone would read a permanent refusal as a transient
///    outage and retry it forever — the exact GH #695 defect.
///
/// A bare 502/503/504 with no provider status in the body stays retryable: a
/// real gateway outage must not quarantine the units caught in it.
pub fn is_provider_rejection(status: u16, body: &str) -> bool {
    if matches!(status, 400 | 413 | 422) {
        return true;
    }
    let body = body.to_ascii_lowercase();
    let Some(rest) = body.split("provider returned ").nth(1) else {
        return false;
    };
    rest.trim_start()
        .get(..3)
        .and_then(|code| code.parse::<u16>().ok())
        .is_some_and(|code| (400..500).contains(&code))
}

/// Extract vectors from the committed response shape.
///
/// One shape only: `{"embeddings": [[..]]}`. The OpenAI-compatible
/// `data[].embedding` branch was dropped in cas-a924 — the cloud team's
/// contract response states they will not emit it, and keeping a second
/// accepted shape meant a malformed body could silently parse as a list of
/// empty vectors instead of failing.
pub fn parse_embedding_response(body: &serde_json::Value) -> Option<Vec<Vec<f32>>> {
    let list = body.get("embeddings").and_then(|v| v.as_array())?;
    Some(list.iter().map(json_to_vector).collect())
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

/// Key prefix for every code-history vector (spec §4.4).
///
/// History and knowledge vectors live in ONE LMDB env; this prefix is the only
/// thing that keeps the two corpora from being read as each other.
pub const HISTORY_KEY_PREFIX: &str = "history:";

/// Key prefix for semantic source-code vectors. Code vectors live in their
/// own LMDB environment as well as carrying this prefix: the directory is the
/// corpus boundary, while the prefix makes accidental cross-opening fail
/// closed instead of returning a plausible-looking foreign document.
pub const CODE_KEY_PREFIX: &str = "code:symbol:";

pub fn code_symbol_key(symbol_id: &str) -> String {
    format!("{CODE_KEY_PREFIX}{symbol_id}")
}

/// Vector key for a commit: `history:commit:{sha}`.
pub fn history_commit_key(sha: &str) -> String {
    format!("{HISTORY_KEY_PREFIX}commit:{sha}")
}

/// Vector key for a GitHub/CHANGELOG doc: `history:doc:{id}`.
pub fn history_doc_key(id: &str) -> String {
    format!("{HISTORY_KEY_PREFIX}doc:{id}")
}

/// Which corpus a cached vector belongs to.
///
/// Deliberately a closed enum over one shared env rather than two envs: LMDB
/// refuses a double-open in a process (see [`OPEN_ENVS`]), so a second
/// environment would add a failure mode without adding isolation that a key
/// prefix cannot provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorNamespace {
    /// Distilled knowledge pages.
    Knowledge,
    /// Code history: commits and GitHub/CHANGELOG docs.
    History,
    /// Current source-code symbols. Stored in an isolated LMDB environment.
    Code,
}

impl VectorNamespace {
    pub fn contains(&self, id: &str) -> bool {
        match self {
            VectorNamespace::History => id.starts_with(HISTORY_KEY_PREFIX),
            VectorNamespace::Code => id.starts_with(CODE_KEY_PREFIX),
            VectorNamespace::Knowledge => {
                !id.starts_with(HISTORY_KEY_PREFIX) && !id.starts_with(CODE_KEY_PREFIX)
            }
        }
    }
}

/// Durable receipt that a vector cache generation was created by discarding an
/// older one.
///
/// Stored next to the vectors it describes and therefore removed with them by
/// the next wipe: whatever is on disk always describes the current generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheRebuild {
    pub rebuilt_at: chrono::DateTime<chrono::Utc>,
    /// Why the old cache was discarded, in operator-readable terms.
    pub reason: String,
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
    /// Directory holding the cache for a given Cassy root.
    pub fn cache_dir(cas_root: &Path) -> PathBuf {
        cas_root.join("index").join("knowledge-vectors")
    }

    /// Dedicated source-code vector environment. It is intentionally not a
    /// sub-database or key range of the knowledge cache: opening/querying one
    /// corpus can never enumerate the other.
    pub fn code_cache_dir(cas_root: &Path) -> PathBuf {
        cas_root.join("index").join("code-vectors")
    }

    fn meta_path(dir: &Path) -> PathBuf {
        dir.join("embedding_meta.json")
    }

    fn rebuild_path(dir: &Path) -> PathBuf {
        dir.join("rebuilt.json")
    }

    /// Open (creating if needed) the cache for `meta`.
    ///
    /// If a cache exists for a *different* `(provider, model, dims)` triple it
    /// is destroyed first: vectors from two models are not comparable, and
    /// keeping them would silently corrupt ranking. Callers should re-mark
    /// pages pending when [`Self::reindexed`] is true.
    pub fn open(cas_root: &Path, meta: EmbeddingMeta) -> Result<Self, CasError> {
        Self::open_dir(Self::cache_dir(cas_root), meta)
    }

    pub fn open_code(cas_root: &Path, meta: EmbeddingMeta) -> Result<Self, CasError> {
        Self::open_dir(Self::code_cache_dir(cas_root), meta)
    }

    /// Open the already-existing knowledge/history cache without creating a
    /// new embedding corpus. Query-time callers use this after capability
    /// resolution so a configured provider with no indexed vectors does not
    /// materialise an empty LMDB environment merely because a hook ran.
    pub fn open_existing(cas_root: &Path) -> Result<Option<Self>, CasError> {
        let dir = Self::cache_dir(cas_root);
        let Some(meta) = Self::read_meta(&dir) else {
            return Ok(None);
        };
        Self::open_existing_dir(dir, meta)
    }

    /// Query-only opener for the isolated source-code cache. Unlike
    /// [`Self::open_existing_code`], this never needs write access because it
    /// cannot be used by the structural indexer's retirement path.
    pub fn open_existing_code_read_only(cas_root: &Path) -> Result<Option<Self>, CasError> {
        let dir = Self::code_cache_dir(cas_root);
        let Some(meta) = Self::read_meta(&dir) else {
            return Ok(None);
        };
        Self::open_existing_dir(dir, meta)
    }

    /// Open an already-existing code cache using its persisted embedding
    /// identity. Used only to retire deleted symbols while logged out; it
    /// never creates storage and never performs a cloud call.
    pub fn open_existing_code(cas_root: &Path) -> Result<Option<Self>, CasError> {
        let dir = Self::code_cache_dir(cas_root);
        let Some(meta) = Self::read_meta(&dir) else {
            return Ok(None);
        };
        Self::open_dir(dir, meta).map(Some)
    }

    fn open_dir(dir: PathBuf, meta: EmbeddingMeta) -> Result<Self, CasError> {
        let mut reindexed = false;

        let mut envs = open_envs().lock().unwrap_or_else(|p| p.into_inner());

        let existing = Self::read_meta(&dir);
        let mut rebuild_reason = None;
        if let Some(existing) = existing {
            if existing != meta {
                rebuild_reason = Some(format!(
                    "embedding model changed from {}/{} ({}d) to {}/{} ({}d)",
                    existing.provider,
                    existing.model,
                    existing.dims,
                    meta.provider,
                    meta.model,
                    meta.dims
                ));
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
        // Written *after* the wipe, so the receipt always describes the
        // generation of vectors currently on disk. Without it, a rebuild is
        // indistinguishable from a lost index: both show every symbol pending
        // with no explanation (cas-73e7 / GH #696).
        if let Some(reason) = rebuild_reason {
            Self::write_rebuild(
                &dir,
                &CacheRebuild {
                    rebuilt_at: chrono::Utc::now(),
                    reason,
                },
            )?;
        }

        Ok(Self {
            store,
            meta,
            root: dir,
            reindexed,
        })
    }

    fn open_existing_dir(dir: PathBuf, meta: EmbeddingMeta) -> Result<Option<Self>, CasError> {
        let mut envs = open_envs().lock().unwrap_or_else(|p| p.into_inner());
        let store = match envs.get(&dir) {
            Some(store) if store.dimension() == meta.dims => Arc::clone(store),
            Some(_) => return Ok(None),
            None => {
                let Some(store) = LmdbVectorStore::open_existing(&dir, meta.dims)
                    .map_err(|e| CasError::Other(format!("Failed to open embedding cache: {e}")))?
                else {
                    return Ok(None);
                };
                let store = Arc::new(store);
                envs.insert(dir.clone(), Arc::clone(&store));
                store
            }
        };
        drop(envs);
        Ok(Some(Self {
            store,
            meta,
            root: dir,
            reindexed: false,
        }))
    }

    fn read_meta(dir: &Path) -> Option<EmbeddingMeta> {
        let raw = std::fs::read_to_string(Self::meta_path(dir)).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Read the rebuild receipt for the isolated source-code cache, if the
    /// current generation of that cache was created by wiping an older one.
    ///
    /// `None` means "no rebuild since this cache first appeared" — a first
    /// build is not a rebuild and must not be reported as one.
    pub fn code_cache_rebuild(cas_root: &Path) -> Option<CacheRebuild> {
        let raw = std::fs::read_to_string(Self::rebuild_path(&Self::code_cache_dir(cas_root))).ok()?;
        serde_json::from_str(&raw).ok()
    }

    fn write_rebuild(dir: &Path, rebuild: &CacheRebuild) -> Result<(), CasError> {
        let raw = serde_json::to_string_pretty(rebuild)
            .map_err(|e| CasError::Other(format!("Failed to serialize cache rebuild: {e}")))?;
        std::fs::write(Self::rebuild_path(dir), raw)
            .map_err(|e| CasError::Other(format!("Failed to write cache rebuild receipt: {e}")))
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

    /// Cached vectors belonging to one namespace.
    ///
    /// The raw [`Self::count`] answers "how big is the env", which stopped
    /// being the same question as "does the knowledge channel have anything to
    /// return" the moment history vectors moved in beside it (spec §4.4).
    pub fn count_in(&self, namespace: VectorNamespace) -> Result<usize, CasError> {
        let ids = self
            .store
            .list_ids()
            .map_err(|e| CasError::Other(format!("Failed to list cached embeddings: {e}")))?;
        Ok(ids.iter().filter(|id| namespace.contains(id)).count())
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
        self.nearest_in(VectorNamespace::Knowledge, query, k)
    }

    /// Brute-force kNN restricted to one namespace.
    ///
    /// The restriction is load-bearing, not hygiene. Knowledge pages and code
    /// history share one LMDB env (spec §4.4 — a second env would be a second
    /// double-open failure mode for no benefit), and the knowledge channel
    /// resolves every id it gets back as a page id. Without this filter a
    /// `history:commit:{sha}` hit would come back as a page that does not
    /// exist, and the better the commit matched the more certainly it would
    /// displace a real page.
    pub fn nearest_in(
        &self,
        namespace: VectorNamespace,
        query: &[f32],
        k: usize,
    ) -> Result<Vec<(String, f32)>, CasError> {
        if is_zero_vector(query) || k == 0 {
            return Ok(Vec::new());
        }
        let ids = self
            .store
            .list_ids()
            .map_err(|e| CasError::Other(format!("Failed to list cached embeddings: {e}")))?;

        let mut scored: Vec<(String, f32)> = Vec::with_capacity(ids.len());
        for id in ids {
            if !namespace.contains(&id) {
                continue;
            }
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
///
/// The pages are sent in chunks of [`MAX_EMBED_INPUTS_PER_REQUEST`] — the
/// endpoint's hard cap. Before cas-a924 every fetched page went out in a
/// single request, so any `limit` above 32 produced a permanent `400` and
/// **zero** pages embedded; the invariant was held only by a magic `32`
/// duplicated at the one production call site.
///
/// A request-level failure is reported, never swallowed: it lands in
/// [`EmbedReport::request_errors`], the remaining pages are counted as
/// [`EmbedReport::deferred`], and the function still returns `Ok` so the
/// caller gets the partial results *and* the failure. Only store errors are
/// `Err`.
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
        report.pending_after = count_pending(store)?;
        return Ok(report);
    }

    let units: Vec<EmbedUnit> = pages
        .iter()
        .map(|page| {
            let body = store.read_body(&page.rel_path).unwrap_or_default();
            // Key and id coincide for knowledge pages; history keys do not
            // (spec §4.4 namespaces them), which is why the unit carries both.
            EmbedUnit::new(
                page.id.clone(),
                page.id.clone(),
                cap_embedding_text(page_embedding_text(page, &body)),
            )
        })
        .collect();

    let mut mark = |id: &str| store.mark_embedded(id).map_err(|e| e.to_string());
    drain_units(
        embedder,
        cache,
        &units,
        &RateLimiter::cloud(),
        &mut mark,
        &mut report,
    );

    report.pending_after = count_pending(store)?;
    Ok(report)
}

/// One thing to embed: where its vector is filed, which row to clear when the
/// vector lands, and the text that gets sent.
///
/// The split between `key` and `id` is spec §4.4's namespacing: a knowledge
/// page's vector is filed under its own id, while a commit's is filed under
/// `history:commit:{sha}` in the *same* LMDB env. One store, two namespaces,
/// and no second environment to double-open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedUnit {
    /// Vector-store key.
    pub key: String,
    /// Row identity handed back to the caller's `mark embedded` callback.
    pub id: String,
    /// The text sent to the provider.
    pub text: String,
}

impl EmbedUnit {
    pub fn new(key: impl Into<String>, id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            id: id.into(),
            text: text.into(),
        }
    }
}

/// The one chunked, rate-limited, failure-reporting drain.
///
/// Every corpus goes through this function — knowledge pages and code history
/// alike — so there is exactly one place that knows the endpoint caps a request
/// at [`MAX_EMBED_INPUTS_PER_REQUEST`] inputs, one place that stops rather than
/// repeating a systemic failure per chunk, and one place that accounts for the
/// units it did not attempt. A second copy of this loop is how the cas-a924
/// defect (an un-chunked request that 400s forever, reported as "0 embedded")
/// would come back for a different corpus.
///
/// `mark` is called only after the vector is safely cached; a row whose vector
/// never landed keeps `pending_embedding = 1` and is retried next run.
pub fn drain_units(
    embedder: &KnowledgeEmbedder,
    cache: &KnowledgeVectorCache,
    units: &[EmbedUnit],
    limiter: &RateLimiter,
    mark: &mut dyn FnMut(&str) -> Result<(), String>,
    report: &mut EmbedReport,
) {
    drain_units_with_quarantine(embedder, cache, units, limiter, mark, &mut None, report);
}

/// [`drain_units`] with somewhere to put units the provider refuses.
///
/// Without a quarantine sink a refused unit can only stay pending, and because
/// the queue is drained in a deterministic order it is re-sent on every tick
/// and the corpus behind it never moves (GH #695: one 138k-char commit body
/// held 7,885 units for three days across two releases). Given a sink, the
/// drain bisects the failing chunk down to the offending unit(s), retires
/// exactly those, and keeps draining their neighbours.
///
/// Callers whose store has no quarantine state pass `None` and keep the
/// original halt-and-defer behaviour.
pub fn drain_units_with_quarantine(
    embedder: &KnowledgeEmbedder,
    cache: &KnowledgeVectorCache,
    units: &[EmbedUnit],
    limiter: &RateLimiter,
    mark: &mut dyn FnMut(&str) -> Result<(), String>,
    quarantine: &mut Option<&mut dyn FnMut(&str, &str) -> Result<(), String>>,
    report: &mut EmbedReport,
) {
    let mut chunks = units.chunks(MAX_EMBED_INPUTS_PER_REQUEST);
    let mut halted = false;

    for chunk in chunks.by_ref() {
        if !drain_chunk(embedder, cache, chunk, limiter, mark, quarantine, report) {
            halted = true;
            break;
        }
    }

    if halted {
        report.deferred += chunks.map(<[EmbedUnit]>::len).sum::<usize>();
    }
}

/// Embed one chunk. Returns false when the run must stop entirely.
///
/// A refusal is not a reason to stop: it is a property of the units in this
/// chunk, so the chunk is split and re-attempted until the offenders are
/// isolated. Every other failure is systemic — auth, rate limit, capability,
/// transport — and hammering the endpoint with the remaining chunks would only
/// multiply it.
fn drain_chunk(
    embedder: &KnowledgeEmbedder,
    cache: &KnowledgeVectorCache,
    chunk: &[EmbedUnit],
    limiter: &RateLimiter,
    mark: &mut dyn FnMut(&str) -> Result<(), String>,
    quarantine: &mut Option<&mut dyn FnMut(&str, &str) -> Result<(), String>>,
    report: &mut EmbedReport,
) -> bool {
    if chunk.is_empty() {
        return true;
    }
    let texts: Vec<String> = chunk.iter().map(|u| u.text.clone()).collect();

    limiter.acquire();
    report.requests += 1;
    let vectors = match embedder.embed_batch(&texts) {
        Ok(vectors) => vectors,
        Err(EmbedError::Rejected(message)) if quarantine.is_some() => {
            if let [unit] = chunk {
                // Isolated: this unit alone is what the provider refuses.
                let sink = quarantine
                    .as_mut()
                    .expect("guard proved the sink is present");
                match sink(&unit.id, &message) {
                    Ok(()) => {
                        report.quarantined += 1;
                        report
                            .quarantine_errors
                            .push((unit.id.clone(), message.clone()));
                    }
                    Err(e) => report.errors.push((unit.id.clone(), e)),
                }
                return true;
            }
            // Split and re-attempt: the refusal belongs to some subset, and
            // bisecting costs O(log n) extra requests instead of stranding the
            // whole chunk. Both halves are attempted even if the first one
            // fails, so two poison units in one chunk are both isolated.
            let (left, right) = chunk.split_at(chunk.len() / 2);
            let left_ok = drain_chunk(embedder, cache, left, limiter, mark, quarantine, report);
            let right_ok = drain_chunk(embedder, cache, right, limiter, mark, quarantine, report);
            return left_ok && right_ok;
        }
        Err(e) => {
            if matches!(e, EmbedError::Unsupported(_)) {
                report.capability_absent = true;
            }
            report.request_errors.push(e.to_string());
            report.deferred += chunk.len();
            return false;
        }
    };

    for (unit, vector) in chunk.iter().zip(vectors.iter()) {
        if is_zero_vector(vector) {
            report.rejected_zero += 1;
            continue;
        }
        if vector.len() != cache.meta().dims {
            report.rejected_dims += 1;
            continue;
        }
        match cache.put(&unit.key, vector) {
            Ok(()) => match mark(&unit.id) {
                Ok(()) => report.embedded += 1,
                Err(e) => report.errors.push((unit.id.clone(), e)),
            },
            Err(e) => report.errors.push((unit.id.clone(), e.to_string())),
        }
    }
    true
}

/// Client-side pacing for the embedding endpoint.
///
/// The server allows [`RATE_LIMIT_REQUESTS`] requests per
/// [`RATE_LIMIT_WINDOW_SECS`]; a full backfill of this repo is ~66 requests, so
/// the cap is only reachable when several corpora drain at once or a large
/// backlog clears in one tick. Pacing here rather than reacting to a 429 keeps
/// the drain from converting a burst into a request-level failure that halts it
/// — the halt is for *systemic* problems, and self-inflicted throttling is not
/// one.
///
/// Sleeps only when the window is genuinely full: a run under the limit pays
/// nothing, which is why the ordinary steady-state pass (2 requests/day per
/// spec §7.2) never blocks.
#[derive(Debug)]
pub struct RateLimiter {
    max_requests: usize,
    window: Duration,
    recent: Mutex<std::collections::VecDeque<std::time::Instant>>,
}

/// Server contract: requests allowed per [`RATE_LIMIT_WINDOW_SECS`].
pub const RATE_LIMIT_REQUESTS: usize = 120;
/// Server contract: the rate-limit window.
pub const RATE_LIMIT_WINDOW_SECS: u64 = 60;

impl RateLimiter {
    /// The cloud endpoint's published limit: 120 requests / 60 s.
    pub fn cloud() -> Self {
        Self::new(
            RATE_LIMIT_REQUESTS,
            Duration::from_secs(RATE_LIMIT_WINDOW_SECS),
        )
    }

    pub fn new(max_requests: usize, window: Duration) -> Self {
        Self {
            max_requests,
            window,
            recent: Mutex::new(std::collections::VecDeque::new()),
        }
    }

    /// Block until issuing one more request keeps the window under the cap.
    pub fn acquire(&self) {
        if self.max_requests == 0 {
            return;
        }
        loop {
            let wait = {
                let mut recent = self.recent.lock().unwrap_or_else(|p| p.into_inner());
                let now = std::time::Instant::now();
                while recent
                    .front()
                    .is_some_and(|t| now.duration_since(*t) >= self.window)
                {
                    recent.pop_front();
                }
                if recent.len() < self.max_requests {
                    recent.push_back(now);
                    return;
                }
                // Full: wait out the oldest request in the window.
                recent
                    .front()
                    .map(|t| self.window.saturating_sub(now.duration_since(*t)))
                    .unwrap_or_default()
            };
            std::thread::sleep(wait.max(Duration::from_millis(1)));
        }
    }

    /// Requests currently inside the window (tests and diagnostics).
    pub fn in_window(&self) -> usize {
        let mut recent = self.recent.lock().unwrap_or_else(|p| p.into_inner());
        let now = std::time::Instant::now();
        while recent
            .front()
            .is_some_and(|t| now.duration_since(*t) >= self.window)
        {
            recent.pop_front();
        }
        recent.len()
    }
}

/// How many pages are still awaiting an embedding, store-wide.
fn count_pending(store: &dyn KnowledgeStore) -> Result<usize, CasError> {
    store
        .count_pending_embedding()
        .map_err(|e| CasError::Other(format!("Failed to count pending pages: {e}")))
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
    fn code_model_or_dimension_change_rebuilds_only_the_isolated_code_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let knowledge =
            KnowledgeVectorCache::open(tmp.path(), EmbeddingMeta::new("p", "knowledge", 3))
                .unwrap();
        knowledge.put("cas-kn001", &[1.0, 0.0, 0.0]).unwrap();
        {
            let code =
                KnowledgeVectorCache::open_code(tmp.path(), EmbeddingMeta::new("p", "code-1", 3))
                    .unwrap();
            code.put(&code_symbol_key("sym-1"), &[1.0, 0.0, 0.0])
                .unwrap();
        }

        let code =
            KnowledgeVectorCache::open_code(tmp.path(), EmbeddingMeta::new("p", "code-2", 4))
                .unwrap();
        assert!(code.reindexed());
        assert_eq!(code.count().unwrap(), 0);
        assert!(code.put(&code_symbol_key("zero"), &[0.0; 4]).is_err());
        assert_eq!(
            knowledge.count().unwrap(),
            1,
            "knowledge cache was contaminated"
        );
    }

    /// A first build is not a rebuild; a wipe is, and it must leave a receipt
    /// naming when and why, so doctor can say "vectors are regenerating"
    /// instead of silently reporting a corpus that lost every vector.
    #[test]
    fn code_cache_rebuild_receipt_is_written_only_when_a_cache_is_discarded() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let _first =
                KnowledgeVectorCache::open_code(tmp.path(), EmbeddingMeta::new("p", "code-1", 3))
                    .unwrap();
        }
        assert!(
            KnowledgeVectorCache::code_cache_rebuild(tmp.path()).is_none(),
            "a first build must not be reported as a rebuild"
        );

        let before = chrono::Utc::now();
        {
            let second =
                KnowledgeVectorCache::open_code(tmp.path(), EmbeddingMeta::new("p", "code-2", 3))
                    .unwrap();
            assert!(second.reindexed());
        }
        let rebuild = KnowledgeVectorCache::code_cache_rebuild(tmp.path())
            .expect("a wipe must leave a receipt");
        assert!(rebuild.rebuilt_at >= before);
        assert!(
            rebuild.reason.contains("code-1") && rebuild.reason.contains("code-2"),
            "receipt must name the change: {}",
            rebuild.reason
        );

        // Reopening unchanged neither rebuilds nor rewrites the receipt.
        let _third =
            KnowledgeVectorCache::open_code(tmp.path(), EmbeddingMeta::new("p", "code-2", 3))
                .unwrap();
        assert_eq!(
            KnowledgeVectorCache::code_cache_rebuild(tmp.path()),
            Some(rebuild)
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
    fn parses_only_the_committed_flat_response_shape() {
        let flat = serde_json::json!({"embeddings": [[1.0, 2.0], [3.0, 4.0]]});
        assert_eq!(
            parse_embedding_response(&flat).unwrap(),
            vec![vec![1.0, 2.0], vec![3.0, 4.0]]
        );
        // The OpenAI-compatible envelope is NOT accepted any more (cas-a924):
        // the cloud contract is the flat single key, and tolerating a second
        // shape meant a stray `data` array parsed as empty vectors instead of
        // failing loudly.
        let openai = serde_json::json!({"data": [{"embedding": [1.0, 2.0]}]});
        assert!(parse_embedding_response(&openai).is_none());
        assert!(parse_embedding_response(&serde_json::json!({"oops": 1})).is_none());
    }

    /// GH #695: the deployed cloud used to wrap a provider 400 as a gateway
    /// 502, and now (petra-stella-cloud#63) returns the 400 directly. Both must
    /// read as "this payload will never be accepted"; a bare gateway failure
    /// must not, or an outage would quarantine every unit caught in it.
    #[test]
    fn provider_refusals_are_told_apart_from_gateway_outages() {
        // Direct client errors, the post-#63 shape.
        assert!(is_provider_rejection(
            400,
            r#"{"error":"embedding_input_rejected","message":"Invalid 'input[14]': maximum input length is 8,192 tokens."}"#
        ));
        assert!(is_provider_rejection(413, "payload too large"));
        assert!(is_provider_rejection(422, "unprocessable"));

        // The wrapper shape still deployed when GH #695 was filed.
        assert!(is_provider_rejection(
            502,
            r#"{"error":"Embedding provider returned 400"}"#
        ));
        assert!(is_provider_rejection(
            500,
            "Embedding provider returned 422 for input 3"
        ));

        // Genuinely transient: retrying can succeed, so these stay retryable.
        assert!(!is_provider_rejection(502, "upstream unavailable"));
        assert!(!is_provider_rejection(503, "service unavailable"));
        assert!(!is_provider_rejection(429, "rate limited"));
        assert!(!is_provider_rejection(500, "internal error"));
        // A provider 5xx named in the body is the provider being down, not the
        // payload being wrong.
        assert!(!is_provider_rejection(
            502,
            r#"{"error":"Embedding provider returned 503"}"#
        ));
    }

    /// Every corpus that builds embedding text must stay under the model's
    /// input cap — a long knowledge page is the same poison shape as the
    /// 138k-char commit body from GH #695.
    #[test]
    fn the_text_cap_is_char_safe_and_leaves_ordinary_text_alone() {
        assert_eq!(cap_embedding_text("short".to_string()), "short");

        let huge = "→".repeat(MAX_EMBED_TEXT_CHARS + 500);
        let capped = cap_embedding_text(huge);
        assert_eq!(capped.chars().count(), MAX_EMBED_TEXT_CHARS);
        // Multi-byte safety: a byte slice here would panic mid-codepoint.
        assert!(capped.chars().all(|c| c == '→'));
    }

    #[test]
    fn embed_batch_refuses_an_input_list_over_the_endpoint_cap() {
        // The client must never be the one that discovers the cap as a 400.
        let embedder = KnowledgeEmbedder::new("https://example.invalid", "t").with_model("m", 4);
        let texts: Vec<String> = (0..MAX_EMBED_INPUTS_PER_REQUEST + 1)
            .map(|i| format!("text {i}"))
            .collect();
        let err = embedder.embed_batch(&texts).unwrap_err();
        assert!(matches!(err, EmbedError::Failed(_)), "got {err:?}");
        assert!(err.to_string().contains("caps a request at 32"), "{err}");
    }

    /// Answers with one unit vector per input, and records how many inputs
    /// each request carried — that record is the chunking receipt.
    struct EchoEmbeddings {
        dims: usize,
        seen: std::sync::Arc<std::sync::Mutex<Vec<usize>>>,
    }

    impl wiremock::Respond for EchoEmbeddings {
        fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let n = body
                .get("input")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            self.seen.lock().unwrap().push(n);
            let vectors: Vec<Vec<f32>> = (0..n)
                .map(|_| {
                    let mut v = vec![0.0f32; self.dims];
                    v[0] = 1.0;
                    v
                })
                .collect();
            wiremock::ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "embeddings": vectors }))
        }
    }

    #[tokio::test]
    async fn pending_pages_are_chunked_at_the_endpoint_input_cap() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer};

        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embeddings"))
            .respond_with(EchoEmbeddings {
                dims: 4,
                seen: seen.clone(),
            })
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        // More pages than one request may carry: before chunking this was a
        // single 33+-input request and a permanent 400.
        let titles: Vec<String> = (0..70).map(|i| format!("Page {i}")).collect();

        let report = tokio::task::spawn_blocking(move || {
            let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
            let store = seed_store(&root, &refs);
            let embedder = KnowledgeEmbedder::new(&endpoint, "test-token").with_model("m", 4);
            let cache = KnowledgeVectorCache::open(&root, embedder.meta()).unwrap();
            embed_pending_pages(&store, &embedder, &cache, 100).unwrap()
        })
        .await
        .unwrap();

        let sizes = seen.lock().unwrap().clone();
        assert_eq!(
            sizes,
            vec![
                MAX_EMBED_INPUTS_PER_REQUEST,
                MAX_EMBED_INPUTS_PER_REQUEST,
                6
            ],
            "70 pages must go out as 32 + 32 + 6, never as one oversized request"
        );
        assert_eq!(report.requests, 3);
        assert_eq!(report.embedded, 70);
        assert_eq!(report.deferred, 0);
        assert!(report.request_errors.is_empty());
        assert_eq!(
            report.pending_after, 0,
            "the awaiting-embedding count must drain to zero"
        );
    }

    #[tokio::test]
    async fn a_failed_embedding_request_is_reported_not_swallowed() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embeddings"))
            .respond_with(ResponseTemplate::new(400).set_body_string("too many inputs"))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let titles: Vec<String> = (0..40).map(|i| format!("Page {i}")).collect();

        let report = tokio::task::spawn_blocking(move || {
            let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
            let store = seed_store(&root, &refs);
            let embedder = KnowledgeEmbedder::new(&endpoint, "test-token").with_model("m", 4);
            let cache = KnowledgeVectorCache::open(&root, embedder.meta()).unwrap();
            embed_pending_pages(&store, &embedder, &cache, 100).unwrap()
        })
        .await
        .unwrap();

        assert_eq!(report.embedded, 0);
        assert_eq!(
            report.requests, 1,
            "a systemic failure must stop the run, not repeat itself per chunk"
        );
        assert_eq!(
            report.deferred, 40,
            "every page not attempted must be accounted for"
        );
        assert_eq!(report.request_errors.len(), 1);
        assert!(report.request_errors[0].contains("400"));
        assert!(report.had_trouble(), "the run must not look successful");
        assert!(
            !report.capability_absent,
            "a 400 is a failure, not a boundary"
        );
        assert_eq!(report.pending_after, 40);
    }

    #[tokio::test]
    async fn an_endpoint_without_the_route_is_a_boundary_not_an_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embeddings"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let report = tokio::task::spawn_blocking(move || {
            let store = seed_store(&root, &["Build System"]);
            let embedder = KnowledgeEmbedder::new(&endpoint, "test-token").with_model("m", 4);
            let cache = KnowledgeVectorCache::open(&root, embedder.meta()).unwrap();
            embed_pending_pages(&store, &embedder, &cache, 100).unwrap()
        })
        .await
        .unwrap();

        assert!(
            report.capability_absent,
            "404 means this endpoint has no embedding capability"
        );
        assert_eq!(report.embedded, 0);
        assert_eq!(report.pending_after, 1);
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
