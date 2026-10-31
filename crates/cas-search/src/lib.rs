//! CAS Search Infrastructure
//!
//! This crate provides search capabilities for CAS:
//!
//! - **BM25 full-text search** via Tantivy ([`Bm25Index`])
//! - **Vector storage** via heed (LMDB wrapper) ([`LmdbVectorStore`])
//!
//! Multi-channel hybrid search lives in `cas-cli`'s `hybrid_search` module; the
//! unused second implementation that once lived here was removed in cas-7909
//! (EPIC cas-b129 M6). This crate provides the indexing primitives it builds on.
//!
//! # Core Traits
//!
//! - [`SearchDocument`] - Generic document representation for indexing
//! - [`VectorStore`] - Vector storage and similarity search
//! - [`TextIndex`] - Full-text (BM25) search index
//!
//! # Example
//!
//! ```rust,ignore
//! use cas_search::{Bm25Index, SearchDocument, VectorStore, TextIndex};
//!
//! // Implement SearchDocument for your types
//! struct MyDoc { /* ... */ }
//! impl SearchDocument for MyDoc { /* ... */ }
//!
//! // Create BM25 index
//! let index = Bm25Index::in_memory()?;
//! index.index(&my_doc)?;
//! let results = index.search("query", 10)?;
//! ```

pub mod bm25;
pub mod code_search;
pub mod document;
pub mod error;
pub mod grep;
pub mod lmdb_store;
pub mod traits;

// Re-export core types
pub use bm25::{Bm25Config, Bm25Index};
pub use code_search::{CodeSearch, CodeSearchOptions, CodeSearchResult, CodeSearchStats, Embedder};
pub use error::{Result, SearchError};
pub use lmdb_store::{LmdbEnvInfo, LmdbVectorStore};
pub use traits::{ScoreSource, SearchDocument, SearchResult, TextIndex, VectorStore};

// Re-export async types when parallel feature is enabled
#[cfg(feature = "parallel")]
pub use bm25::AsyncBm25Index;
#[cfg(feature = "parallel")]
pub use traits::{AsyncEmbedder, AsyncTextIndex, AsyncVectorStore};

// Re-export grep types
pub use grep::{GrepMatch, GrepOptions, GrepSearch};

// Re-export external crates for consumers
/// Re-export heed for vector storage backends
pub use heed;

/// Re-export tantivy for BM25 search backends
pub use tantivy;
