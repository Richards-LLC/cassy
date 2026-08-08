//! The semantic retrieval channel — present only when the cloud is (T5).
//!
//! T3 made the scorer honest: [`ChannelCapabilities`] zeroes the weight of any
//! channel that cannot return rows and redistributes its mass to the live ones.
//! Until now `has_semantic()` was hardcoded `false`, because there was nothing
//! behind it. This module is what makes the flag able to be true — and, just as
//! importantly, keeps it false in exactly the cases where the channel would
//! return nothing.
//!
//! A [`SemanticChannel`] exists only when both halves are real:
//! - a [`KnowledgeEmbedder`], which requires cloud auth, and
//! - a non-empty local vector cache.
//!
//! A logged-out installation gets `None` from [`open_semantic_channel`], no
//! LMDB environment on disk, and no HTTP traffic at query time.
//!
//! [`ChannelCapabilities`]: crate::hybrid_search::scorer::ChannelCapabilities

use std::path::Path;

use crate::cloud::CloudConfig;
use crate::cloud::embeddings::{
    KnowledgeEmbedder, KnowledgeVectorCache, is_zero_vector, page_embedding_text,
};
use crate::error::Result;
use cas_store::KnowledgePage;

/// Cloud embedder plus the local vector cache it fills.
pub struct SemanticChannel {
    embedder: KnowledgeEmbedder,
    cache: KnowledgeVectorCache,
}

impl SemanticChannel {
    pub fn new(embedder: KnowledgeEmbedder, cache: KnowledgeVectorCache) -> Self {
        Self { embedder, cache }
    }

    pub fn embedder(&self) -> &KnowledgeEmbedder {
        &self.embedder
    }

    pub fn cache(&self) -> &KnowledgeVectorCache {
        &self.cache
    }

    /// Number of vectors available locally.
    ///
    /// A channel with zero cached vectors is *configured* but not yet *live*:
    /// it can only return an empty list, so the capability flag must stay
    /// false or the scorer would again allocate weight to a dead channel —
    /// the exact dishonesty T3 removed.
    ///
    /// Counted **in the knowledge namespace only**: since M7 the same LMDB env
    /// also holds `history:*` vectors (spec §4.4), and a raw env count would
    /// report this channel live off the back of commits it cannot resolve to
    /// pages — the same dishonesty by a different route.
    pub fn cached_vectors(&self) -> usize {
        self.cache
            .count_in(crate::cloud::embeddings::VectorNamespace::Knowledge)
            .unwrap_or(0)
    }

    pub fn is_live(&self) -> bool {
        self.cached_vectors() > 0
    }

    /// Embed the text of one page (title + snippet + body), exactly as the
    /// indexing side does.
    pub fn embed_page_text(page: &KnowledgePage, body: &str) -> String {
        page_embedding_text(page, body)
    }

    /// Nearest cached pages for a natural-language query.
    ///
    /// Returns an empty list — never an error — when the provider answers with
    /// an unusable vector, so a flaky embedding endpoint degrades retrieval
    /// instead of failing the whole search.
    pub fn search(&self, query: &str, k: usize) -> Result<Vec<(String, f32)>> {
        if query.trim().is_empty() || k == 0 {
            return Ok(Vec::new());
        }
        let vectors = match self.embedder.embed_batch(&[query.to_string()]) {
            Ok(vectors) => vectors,
            Err(e) => {
                tracing::debug!(error = %e, "semantic channel: query embedding failed");
                return Ok(Vec::new());
            }
        };
        let Some(query_vector) = vectors.into_iter().next() else {
            return Ok(Vec::new());
        };
        if is_zero_vector(&query_vector) {
            return Ok(Vec::new());
        }
        match self.cache.nearest(&query_vector, k) {
            Ok(hits) => Ok(hits),
            Err(e) => {
                tracing::debug!(error = %e, "semantic channel: vector lookup failed");
                Ok(Vec::new())
            }
        }
    }
}

/// The capability gate for retrieval.
///
/// `None` means this installation has no semantic channel — a first-class
/// supported state, not a failure. Nothing is created on disk in that case.
pub fn open_semantic_channel(cas_root: &Path, config: &CloudConfig) -> Option<SemanticChannel> {
    let embedder = KnowledgeEmbedder::from_config(config)?;
    // Only open the cache once we know an embedder exists: opening it creates
    // an LMDB environment, which a provider-absent install must never pay for.
    let cache = KnowledgeVectorCache::open(cas_root, embedder.meta()).ok()?;
    Some(SemanticChannel::new(embedder, cache))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::embeddings::EmbeddingMeta;

    fn logged_out() -> CloudConfig {
        CloudConfig {
            token: None,
            ..Default::default()
        }
    }

    #[test]
    fn no_auth_means_no_channel_and_no_cache_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(open_semantic_channel(tmp.path(), &logged_out()).is_none());
        assert!(!KnowledgeVectorCache::cache_dir(tmp.path()).exists());
    }

    #[test]
    fn a_configured_but_empty_channel_is_not_live() {
        let tmp = tempfile::tempdir().unwrap();
        let embedder = KnowledgeEmbedder::new("https://example.invalid", "t").with_model("m", 4);
        let cache = KnowledgeVectorCache::open(tmp.path(), embedder.meta()).unwrap();
        let channel = SemanticChannel::new(embedder, cache);
        assert!(
            !channel.is_live(),
            "an empty cache can only return nothing; the capability flag must say so"
        );
    }

    #[test]
    fn a_channel_with_vectors_is_live() {
        let tmp = tempfile::tempdir().unwrap();
        let embedder = KnowledgeEmbedder::new("https://example.invalid", "t").with_model("m", 4);
        let cache = KnowledgeVectorCache::open(tmp.path(), EmbeddingMeta::new("cas-cloud", "m", 4))
            .unwrap();
        cache.put("cas-kn001", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        let channel = SemanticChannel::new(embedder, cache);
        assert!(channel.is_live());
    }

    #[test]
    fn an_empty_query_never_reaches_the_provider() {
        // The endpoint is unroutable, so a network attempt would surface as a
        // long timeout rather than an empty result.
        let tmp = tempfile::tempdir().unwrap();
        let embedder = KnowledgeEmbedder::new("https://example.invalid", "t").with_model("m", 4);
        let cache = KnowledgeVectorCache::open(tmp.path(), embedder.meta()).unwrap();
        let channel = SemanticChannel::new(embedder, cache);
        assert!(channel.search("   ", 5).unwrap().is_empty());
        assert!(channel.search("real query", 0).unwrap().is_empty());
    }

    #[tokio::test]
    async fn ranks_cached_pages_for_a_query_via_a_mocked_provider() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "embeddings": [[0.0, 1.0, 0.0, 0.0]]
            })))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let hits = tokio::task::spawn_blocking(move || {
            let embedder = KnowledgeEmbedder::new(&endpoint, "test-token").with_model("m", 4);
            let cache = KnowledgeVectorCache::open(&root, embedder.meta()).unwrap();
            cache.put("cas-kn001", &[1.0, 0.0, 0.0, 0.0]).unwrap();
            cache.put("cas-kn002", &[0.0, 1.0, 0.0, 0.0]).unwrap();
            SemanticChannel::new(embedder, cache)
                .search("anything", 5)
                .unwrap()
        })
        .await
        .unwrap();

        assert_eq!(hits.len(), 1, "orthogonal vectors score 0 and are dropped");
        assert_eq!(hits[0].0, "cas-kn002");
    }

    #[tokio::test]
    async fn a_provider_failure_degrades_to_empty_not_an_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embeddings"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        let hits = tokio::task::spawn_blocking(move || {
            let embedder = KnowledgeEmbedder::new(&endpoint, "test-token").with_model("m", 4);
            let cache = KnowledgeVectorCache::open(&root, embedder.meta()).unwrap();
            cache.put("cas-kn001", &[1.0, 0.0, 0.0, 0.0]).unwrap();
            SemanticChannel::new(embedder, cache)
                .search("anything", 5)
                .unwrap()
        })
        .await
        .unwrap();

        assert!(
            hits.is_empty(),
            "a 500 from the embedding provider must not fail the search"
        );
    }
}
