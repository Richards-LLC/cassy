//! Hybrid Search Module
//!
//! Provides BM25 full-text search with scoring utilities.
//! Semantic search is available via cloud API (premium feature).
//!
//! # Example
//!
//! ```rust,ignore
//! use cas_search::hybrid::{HybridSearch, HybridSearchOptions};
//!
//! let search = HybridSearch::new(bm25_index);
//! let results = search.search(&opts)?;
//! ```

use std::sync::Arc;
use std::time::Duration;

use crate::error::Result;
use crate::traits::TextIndex;

/// Channel-specific timeout (default 5 seconds)
pub const DEFAULT_CHANNEL_TIMEOUT: Duration = Duration::from_secs(5);

/// Result from hybrid search with component scores
#[derive(Debug, Clone)]
pub struct HybridSearchResult {
    /// Document ID
    pub id: String,
    /// Final combined score
    pub score: f64,
    /// BM25 component score
    pub bm25_score: f64,
    /// Semantic component score (always 0.0, semantic search via cloud)
    pub semantic_score: f64,
}

/// Options for hybrid search
#[derive(Debug, Clone)]
pub struct HybridSearchOptions {
    /// Search query
    pub query: String,
    /// Maximum results
    pub limit: usize,
    /// Document type filter (optional)
    pub doc_type: Option<String>,
    /// Tag filters (optional)
    pub tags: Vec<String>,
    /// Enable semantic search (ignored - semantic via cloud only)
    pub enable_semantic: bool,
    /// BM25 weight (0.0-1.0)
    pub bm25_weight: f32,
    /// Semantic weight (ignored - semantic via cloud only)
    pub semantic_weight: f32,
    /// Use RRF instead of weighted sum
    pub use_rrf: bool,
    /// RRF k constant
    pub rrf_k: f64,
    /// Timeout per channel
    pub channel_timeout: Duration,
}

impl Default for HybridSearchOptions {
    fn default() -> Self {
        Self {
            query: String::new(),
            limit: 10,
            doc_type: None,
            tags: Vec::new(),
            enable_semantic: false, // Disabled by default (cloud-only)
            bm25_weight: 1.0,       // Full weight on BM25
            semantic_weight: 0.0,   // No local semantic
            use_rrf: false,
            rrf_k: 60.0,
            channel_timeout: DEFAULT_CHANNEL_TIMEOUT,
        }
    }
}

/// Embedder trait for generating query embeddings
///
/// Kept for API compatibility. Local embedding generation removed;
/// use cloud API for semantic search.
pub trait Embedder: Send + Sync {
    /// Generate embedding vector for text
    fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Get embedding dimension
    fn dimension(&self) -> usize;
}

/// Hybrid search engine
///
/// Provides BM25 full-text search. Semantic search is available
/// via cloud API as a premium feature.
pub struct HybridSearch<T: TextIndex> {
    /// BM25 index for lexical search
    bm25_index: Arc<T>,
}

impl<T: TextIndex> HybridSearch<T> {
    /// Create a new hybrid search instance
    pub fn new(bm25_index: Arc<T>) -> Self {
        Self { bm25_index }
    }

    /// Check if semantic search is available (always false - cloud only)
    pub fn has_semantic(&self) -> bool {
        false
    }

    /// Synchronous search (BM25 only)
    pub fn search(&self, opts: &HybridSearchOptions) -> Result<Vec<HybridSearchResult>> {
        if opts.query.is_empty() {
            return Ok(Vec::new());
        }

        // BM25 search
        let bm25_results = self.bm25_search(opts)?;

        // Build results (semantic_score always 0.0)
        let results: Vec<HybridSearchResult> = bm25_results
            .into_iter()
            .take(opts.limit)
            .map(|(id, score)| HybridSearchResult {
                id,
                score,
                bm25_score: score,
                semantic_score: 0.0,
            })
            .collect();

        Ok(results)
    }

    /// BM25 channel search
    fn bm25_search(&self, opts: &HybridSearchOptions) -> Result<Vec<(String, f64)>> {
        let limit = opts.limit * 3; // Over-fetch for filtering

        let results = match (&opts.doc_type, opts.tags.is_empty()) {
            (Some(doc_type), true) => {
                self.bm25_index
                    .search_with_type(&opts.query, doc_type, limit)?
            }
            (None, false) => {
                let tag_refs: Vec<&str> = opts.tags.iter().map(|s| s.as_str()).collect();
                self.bm25_index
                    .search_with_tags(&opts.query, &tag_refs, limit)?
            }
            _ => self.bm25_index.search(&opts.query, limit)?,
        };

        Ok(results)
    }
}

// =============================================================================
// Async implementation (requires `parallel` feature)
// =============================================================================

#[cfg(feature = "parallel")]
impl<T> HybridSearch<T>
where
    T: TextIndex + Send + Sync + 'static,
{
    /// Asynchronous hybrid search (BM25 only)
    ///
    /// Executes BM25 search using `spawn_blocking` for CPU-bound operations.
    pub async fn search_async(
        &self,
        opts: &HybridSearchOptions,
    ) -> Result<Vec<HybridSearchResult>> {
        if opts.query.is_empty() {
            return Ok(Vec::new());
        }

        // Clone Arc for async task
        let bm25_index = Arc::clone(&self.bm25_index);
        let opts = opts.clone();

        // BM25 search in blocking task
        let bm25_handle = tokio::task::spawn_blocking(move || {
            let limit = opts.limit * 3;
            match (&opts.doc_type, opts.tags.is_empty()) {
                (Some(doc_type), true) => bm25_index.search_with_type(&opts.query, doc_type, limit),
                (None, false) => {
                    let tag_refs: Vec<&str> = opts.tags.iter().map(|s| s.as_str()).collect();
                    bm25_index.search_with_tags(&opts.query, &tag_refs, limit)
                }
                _ => bm25_index.search(&opts.query, limit),
            }
        });

        // Wait with timeout
        let bm25_results = match tokio::time::timeout(opts.channel_timeout, bm25_handle).await {
            Ok(Ok(Ok(results))) => results,
            Ok(Ok(Err(e))) => {
                eprintln!("BM25 channel error: {}", e);
                Vec::new()
            }
            Ok(Err(e)) => {
                eprintln!("BM25 task join error: {}", e);
                Vec::new()
            }
            Err(_) => {
                eprintln!("BM25 channel timeout");
                Vec::new()
            }
        };

        // Build results (semantic_score always 0.0)
        let results: Vec<HybridSearchResult> = bm25_results
            .into_iter()
            .take(opts.limit)
            .map(|(id, score)| HybridSearchResult {
                id,
                score,
                bm25_score: score,
                semantic_score: 0.0,
            })
            .collect();

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use crate::bm25::Bm25Index;
    use crate::hybrid::*;
    use crate::traits::SearchDocument;
    use std::collections::HashMap;

    struct TestDoc {
        id: String,
        content: String,
    }

    impl SearchDocument for TestDoc {
        fn doc_id(&self) -> &str {
            &self.id
        }

        fn doc_content(&self) -> &str {
            &self.content
        }

        fn doc_type(&self) -> &str {
            "test"
        }

        fn doc_tags(&self) -> Vec<&str> {
            Vec::new()
        }

        fn doc_metadata(&self) -> HashMap<String, String> {
            HashMap::new()
        }
    }

    #[test]
    fn test_hybrid_search_options_default() {
        let opts = HybridSearchOptions::default();
        assert!(!opts.enable_semantic); // Disabled by default
        assert_eq!(opts.bm25_weight, 1.0);
        assert_eq!(opts.semantic_weight, 0.0);
        assert!(!opts.use_rrf);
    }

    #[test]
    fn test_hybrid_search_bm25_only() {
        let bm25_index = Arc::new(Bm25Index::in_memory().unwrap());

        // Index some documents
        let docs = vec![
            TestDoc {
                id: "1".to_string(),
                content: "Rust programming language".to_string(),
            },
            TestDoc {
                id: "2".to_string(),
                content: "Python data science".to_string(),
            },
        ];

        for doc in &docs {
            bm25_index.index(doc).unwrap();
        }

        let search = HybridSearch::new(bm25_index);

        let opts = HybridSearchOptions {
            query: "rust".to_string(),
            limit: 10,
            ..Default::default()
        };

        let results = search.search(&opts).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].id, "1");
        assert!(results[0].bm25_score > 0.0);
        assert_eq!(results[0].semantic_score, 0.0); // Always 0 now
    }

    #[test]
    fn test_hybrid_search_result_fields() {
        let result = HybridSearchResult {
            id: "test-123".to_string(),
            score: 0.85,
            bm25_score: 0.85,
            semantic_score: 0.0,
        };

        assert_eq!(result.id, "test-123");
        assert_eq!(result.score, 0.85);
        assert_eq!(result.bm25_score, 0.85);
        assert_eq!(result.semantic_score, 0.0);
    }

    #[test]
    fn test_has_semantic_returns_false() {
        let bm25_index = Arc::new(Bm25Index::in_memory().unwrap());
        let search = HybridSearch::new(bm25_index);
        assert!(!search.has_semantic());
    }

    #[test]
    fn test_empty_query_returns_empty() {
        let bm25_index = Arc::new(Bm25Index::in_memory().unwrap());
        let search = HybridSearch::new(bm25_index);

        let opts = HybridSearchOptions {
            query: "".to_string(),
            ..Default::default()
        };

        let results = search.search(&opts).unwrap();
        assert!(results.is_empty());
    }

    // =========================================================================
    // Async tests (require `parallel` feature)
    // =========================================================================

    #[cfg(feature = "parallel")]
    mod async_tests {
        use std::sync::Arc;

        use crate::bm25::Bm25Index;
        use crate::hybrid::tests::TestDoc;
        use crate::hybrid::{HybridSearch, HybridSearchOptions};
        use crate::traits::TextIndex;

        #[tokio::test]
        async fn test_async_hybrid_search() {
            let bm25_index = Arc::new(Bm25Index::in_memory().unwrap());

            // Index documents
            let docs = vec![
                TestDoc {
                    id: "1".to_string(),
                    content: "Rust programming language".to_string(),
                },
                TestDoc {
                    id: "2".to_string(),
                    content: "Python data science".to_string(),
                },
            ];

            for doc in &docs {
                bm25_index.index(doc).unwrap();
            }

            let search = HybridSearch::new(bm25_index);

            let opts = HybridSearchOptions {
                query: "rust".to_string(),
                limit: 10,
                ..Default::default()
            };

            let results = search.search_async(&opts).await.unwrap();
            assert!(!results.is_empty());
            assert_eq!(results[0].id, "1");
        }

        #[tokio::test]
        async fn test_async_channel_timeout() {
            use std::time::Duration;

            let bm25_index = Arc::new(Bm25Index::in_memory().unwrap());

            // Index a document
            let doc = TestDoc {
                id: "1".to_string(),
                content: "Test content".to_string(),
            };
            bm25_index.index(&doc).unwrap();

            let search = HybridSearch::new(bm25_index);

            let opts = HybridSearchOptions {
                query: "test".to_string(),
                limit: 10,
                channel_timeout: Duration::from_millis(100), // Short timeout
                ..Default::default()
            };

            // Should complete without hanging
            let results = search.search_async(&opts).await.unwrap();
            assert!(!results.is_empty());
        }
    }
}
