//! BM25 Full-text Search Index
//!
//! This module provides a generic BM25 search index using Tantivy.
//! It works with any type implementing the [`SearchDocument`] trait.
//!
//! # Features
//!
//! - Full-text search using BM25 scoring
//! - Type and tag filtering
//! - Atomic index rebuilding (temp index + swap)
//! - Batch indexing for efficiency
//!
//! # Example
//!
//! ```rust,ignore
//! use cas_search::{Bm25Index, SearchDocument, TextIndex};
//!
//! // Create index
//! let index = Bm25Index::open(&index_dir)?;
//!
//! // Index documents
//! index.index(&my_doc)?;
//!
//! // Search
//! let results = index.search("query", 10)?;
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, QueryParser, TermQuery};
use tantivy::schema::*;
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};

use crate::error::{Result, SearchError};
use crate::traits::{SearchDocument, TextIndex};

/// Configuration for BM25 index
#[derive(Debug, Clone)]
pub struct Bm25Config {
    /// Memory budget for index writer in bytes (default: 50MB)
    pub writer_memory: usize,
    /// Number of indexing threads (default: 1)
    pub num_threads: usize,
}

impl Default for Bm25Config {
    fn default() -> Self {
        Self {
            writer_memory: 50_000_000,
            num_threads: 1,
        }
    }
}

/// BM25 full-text search index backed by Tantivy
///
/// This index is generic over any document type implementing [`SearchDocument`].
/// It stores documents with their content, type, tags, and metadata for filtering.
pub struct Bm25Index {
    index: Mutex<Index>,
    schema: Schema,
    // Fields
    id_field: Field,
    content_field: Field,
    title_field: Field,
    doc_type_field: Field,
    tags_field: Field,
    // Configuration
    config: Bm25Config,
    // Index directory (None for in-memory)
    index_dir: Option<PathBuf>,
    // Cached IndexReader (auto-reloads on commit via ReloadPolicy)
    cached_reader: RwLock<Option<IndexReader>>,
    // Cached IndexWriter — avoids allocating 50MB per write operation.
    // Also serializes write operations (only one writer can exist at a time).
    cached_writer: Mutex<Option<IndexWriter>>,
    // Cached QueryParser — avoids rebuilding per search call
    cached_query_parser: Mutex<Option<QueryParser>>,
}

impl Bm25Index {
    /// Open or create a BM25 index at the given directory
    pub fn open(index_dir: &Path) -> Result<Self> {
        Self::open_with_config(index_dir, Bm25Config::default())
    }

    /// Open or create a BM25 index with custom configuration
    pub fn open_with_config(index_dir: &Path, config: Bm25Config) -> Result<Self> {
        let schema = Self::build_schema();

        let index = if index_dir.exists() && index_dir.join("meta.json").exists() {
            Index::open_in_dir(index_dir).map_err(|e| SearchError::Index(e.to_string()))?
        } else {
            std::fs::create_dir_all(index_dir)?;
            Index::create_in_dir(index_dir, schema.clone())
                .map_err(|e| SearchError::Index(e.to_string()))?
        };

        Ok(Self {
            schema: schema.clone(),
            id_field: schema.get_field("id").unwrap(),
            content_field: schema.get_field("content").unwrap(),
            title_field: schema.get_field("title").unwrap(),
            doc_type_field: schema.get_field("doc_type").unwrap(),
            tags_field: schema.get_field("tags").unwrap(),
            index: Mutex::new(index),
            config,
            index_dir: Some(index_dir.to_path_buf()),
            cached_reader: RwLock::new(None),
            cached_writer: Mutex::new(None),
            cached_query_parser: Mutex::new(None),
        })
    }

    /// Create an in-memory BM25 index (for testing)
    pub fn in_memory() -> Result<Self> {
        Self::in_memory_with_config(Bm25Config::default())
    }

    /// Create an in-memory BM25 index with custom configuration
    pub fn in_memory_with_config(config: Bm25Config) -> Result<Self> {
        let schema = Self::build_schema();
        let index = Index::create_in_ram(schema.clone());

        Ok(Self {
            schema: schema.clone(),
            id_field: schema.get_field("id").unwrap(),
            content_field: schema.get_field("content").unwrap(),
            title_field: schema.get_field("title").unwrap(),
            doc_type_field: schema.get_field("doc_type").unwrap(),
            tags_field: schema.get_field("tags").unwrap(),
            index: Mutex::new(index),
            config,
            index_dir: None,
            cached_reader: RwLock::new(None),
            cached_writer: Mutex::new(None),
            cached_query_parser: Mutex::new(None),
        })
    }

    /// Get or create the cached IndexReader.
    ///
    /// The reader uses `ReloadPolicy::OnCommitWithDelay` so it automatically
    /// picks up new segments after normal writes without manual reload.
    fn reader(&self) -> Result<IndexReader> {
        // Fast path: reader already cached
        {
            let guard = self
                .cached_reader
                .read()
                .map_err(|_| SearchError::Index("Reader lock poisoned".to_string()))?;
            if let Some(reader) = guard.as_ref() {
                return Ok(reader.clone());
            }
        }
        // Slow path: create and cache. Clone the Index (cheap, Arc-backed) to
        // release the Mutex before the potentially slow reader construction.
        let index_clone = self
            .index
            .lock()
            .map_err(|_| SearchError::Index("Index lock poisoned".to_string()))?
            .clone();
        let reader: IndexReader = index_clone
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e: tantivy::TantivyError| SearchError::Index(e.to_string()))?;
        let mut guard = self
            .cached_reader
            .write()
            .map_err(|_| SearchError::Index("Reader lock poisoned".to_string()))?;
        // Double-check: another thread may have initialized while we waited
        if guard.is_none() {
            *guard = Some(reader.clone());
        }
        Ok(guard.as_ref().unwrap().clone())
    }

    /// Invalidate the cached reader, forcing a fresh one on next access.
    /// Called after operations that fundamentally change the index (e.g., rebuild).
    fn invalidate_reader(&self) {
        if let Ok(mut guard) = self.cached_reader.write() {
            *guard = None;
        }
    }

    /// Build the index schema
    fn build_schema() -> Schema {
        let mut builder = Schema::builder();

        // Document ID - stored and indexed for exact lookups
        builder.add_text_field("id", STRING | STORED);

        // Main content - full-text indexed
        builder.add_text_field("content", TEXT);

        // Title - full-text indexed with higher weight
        builder.add_text_field("title", TEXT);

        // Document type - stored and indexed for filtering
        builder.add_text_field("doc_type", STRING | STORED);

        // Tags - space-separated, full-text indexed
        builder.add_text_field("tags", TEXT);

        builder.build()
    }

    /// Get a cached index writer, creating one if needed.
    ///
    /// The writer is taken from the cache; callers must call `return_writer()`
    /// after committing to put it back. The `cached_writer` Mutex serializes
    /// concurrent write attempts.
    fn take_writer(&self) -> Result<IndexWriter> {
        let mut guard = self
            .cached_writer
            .lock()
            .map_err(|_| SearchError::Index("Writer lock poisoned".to_string()))?;
        if let Some(writer) = guard.take() {
            return Ok(writer);
        }
        drop(guard);
        let index = self
            .index
            .lock()
            .map_err(|_| SearchError::Index("Index lock poisoned".to_string()))?
            .clone();
        index
            .writer(self.config.writer_memory)
            .map_err(|e| SearchError::Index(e.to_string()))
    }

    /// Return a writer to the cache for reuse.
    fn return_writer(&self, writer: IndexWriter) {
        if let Ok(mut guard) = self.cached_writer.lock() {
            *guard = Some(writer);
        }
    }

    /// Invalidate the cached writer (e.g., after rebuild_atomic swaps the index).
    fn invalidate_writer(&self) {
        if let Ok(mut guard) = self.cached_writer.lock() {
            *guard = None;
        }
    }

    /// Get a cached QueryParser, creating one if needed.
    fn query_parser(&self) -> Result<QueryParser> {
        {
            let guard = self
                .cached_query_parser
                .lock()
                .map_err(|_| SearchError::Index("QueryParser lock poisoned".to_string()))?;
            if let Some(parser) = guard.as_ref() {
                return Ok(parser.clone());
            }
        }
        // Create and cache
        let index = self
            .index
            .lock()
            .map_err(|_| SearchError::Index("Index lock poisoned".to_string()))?
            .clone();
        let parser = QueryParser::for_index(
            &index,
            vec![self.content_field, self.title_field, self.tags_field],
        );
        let mut guard = self
            .cached_query_parser
            .lock()
            .map_err(|_| SearchError::Index("QueryParser lock poisoned".to_string()))?;
        if guard.is_none() {
            *guard = Some(parser.clone());
        }
        Ok(guard.as_ref().unwrap().clone())
    }

    /// Quote field-like tokens whose field name is not registered in this
    /// index schema so Tantivy treats them as free text rather than rejecting
    /// the whole query. Registered fields stay untouched and therefore retain
    /// Tantivy's normal syntax validation.
    fn literalize_unknown_field_tokens(&self, query: &str) -> String {
        query
            .split_whitespace()
            .map(|token| {
                let Some((field, _)) = token.split_once(':') else {
                    return token.to_string();
                };

                if self.schema.get_field(field).is_ok() {
                    return token.to_string();
                }

                let escaped = token.replace('\\', "\\\\").replace('"', "\\\"");
                format!("\"{escaped}\"")
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Invalidate the cached QueryParser (e.g., after rebuild_atomic swaps the index).
    fn invalidate_query_parser(&self) {
        if let Ok(mut guard) = self.cached_query_parser.lock() {
            *guard = None;
        }
    }

    /// Index a batch of documents efficiently
    ///
    /// This is more efficient than indexing documents one at a time
    /// as it uses a single commit for all documents.
    pub fn index_batch<'a>(
        &self,
        docs: impl IntoIterator<Item = &'a dyn SearchDocument>,
    ) -> Result<usize> {
        let mut writer = self.take_writer()?;
        let mut count = 0;

        for doc in docs {
            // Delete existing document with same ID
            let id_term = Term::from_field_text(self.id_field, doc.doc_id());
            writer.delete_term(id_term);

            // Build tantivy document
            let mut tantivy_doc = TantivyDocument::new();
            tantivy_doc.add_text(self.id_field, doc.doc_id());
            tantivy_doc.add_text(self.content_field, doc.doc_content());
            tantivy_doc.add_text(self.doc_type_field, doc.doc_type());
            tantivy_doc.add_text(self.tags_field, doc.doc_tags().join(" "));

            if let Some(title) = doc.doc_title() {
                tantivy_doc.add_text(self.title_field, title);
            }

            writer
                .add_document(tantivy_doc)
                .map_err(|e| SearchError::Index(e.to_string()))?;
            count += 1;
        }

        writer
            .commit()
            .map_err(|e| SearchError::Index(e.to_string()))?;
        self.return_writer(writer);

        self.invalidate_reader();
        Ok(count)
    }

    /// Delete multiple documents by ID efficiently
    pub fn delete_batch<'a>(&self, doc_ids: impl IntoIterator<Item = &'a str>) -> Result<usize> {
        let mut writer = self.take_writer()?;
        let mut count = 0;

        for doc_id in doc_ids {
            let id_term = Term::from_field_text(self.id_field, doc_id);
            writer.delete_term(id_term);
            count += 1;
        }

        writer
            .commit()
            .map_err(|e| SearchError::Index(e.to_string()))?;
        self.return_writer(writer);

        self.invalidate_reader();
        Ok(count)
    }

    /// Rebuild the index atomically with new documents
    ///
    /// This builds a new index in a temporary directory, then swaps it
    /// with the current index. This ensures the index is always in a
    /// consistent state, even if the rebuild is interrupted.
    ///
    /// For in-memory indexes, this simply clears and rebuilds in place.
    pub fn rebuild_atomic<'a>(
        &self,
        docs: impl IntoIterator<Item = &'a dyn SearchDocument>,
    ) -> Result<usize> {
        // For in-memory indexes, just clear and rebuild
        if self.index_dir.is_none() {
            let mut writer = self.take_writer()?;
            writer
                .delete_all_documents()
                .map_err(|e| SearchError::Index(e.to_string()))?;
            writer
                .commit()
                .map_err(|e| SearchError::Index(e.to_string()))?;

            // Re-index documents using the same writer
            let mut count = 0;

            for doc in docs {
                let mut tantivy_doc = TantivyDocument::new();
                tantivy_doc.add_text(self.id_field, doc.doc_id());
                tantivy_doc.add_text(self.content_field, doc.doc_content());
                tantivy_doc.add_text(self.doc_type_field, doc.doc_type());
                tantivy_doc.add_text(self.tags_field, doc.doc_tags().join(" "));

                if let Some(title) = doc.doc_title() {
                    tantivy_doc.add_text(self.title_field, title);
                }

                writer
                    .add_document(tantivy_doc)
                    .map_err(|e| SearchError::Index(e.to_string()))?;
                count += 1;
            }

            writer
                .commit()
                .map_err(|e| SearchError::Index(e.to_string()))?;
            self.return_writer(writer);

            self.invalidate_reader();
            return Ok(count);
        }

        // For disk-based indexes, use atomic swap
        let index_dir = self.index_dir.as_ref().unwrap();
        let temp_dir = index_dir.with_extension("tmp");
        let backup_dir = index_dir.with_extension("bak");

        // Clean up any leftover temp/backup dirs
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::remove_dir_all(&backup_dir);

        // Create new index in temp directory
        std::fs::create_dir_all(&temp_dir)?;

        let temp_index = Index::create_in_dir(&temp_dir, self.schema.clone())
            .map_err(|e| SearchError::Index(e.to_string()))?;

        let mut writer = temp_index
            .writer(self.config.writer_memory)
            .map_err(|e| SearchError::Index(e.to_string()))?;

        let mut count = 0;

        for doc in docs {
            let mut tantivy_doc = TantivyDocument::new();
            tantivy_doc.add_text(self.id_field, doc.doc_id());
            tantivy_doc.add_text(self.content_field, doc.doc_content());
            tantivy_doc.add_text(self.doc_type_field, doc.doc_type());
            tantivy_doc.add_text(self.tags_field, doc.doc_tags().join(" "));

            if let Some(title) = doc.doc_title() {
                tantivy_doc.add_text(self.title_field, title);
            }

            writer
                .add_document(tantivy_doc)
                .map_err(|e| SearchError::Index(e.to_string()))?;
            count += 1;
        }

        writer
            .commit()
            .map_err(|e| SearchError::Index(e.to_string()))?;
        drop(writer);

        // Atomic swap: backup current -> move temp to current -> remove backup
        if index_dir.exists() {
            std::fs::rename(index_dir, &backup_dir)?;
        }

        std::fs::rename(&temp_dir, index_dir)?;

        // Clean up backup
        let _ = std::fs::remove_dir_all(&backup_dir);

        // Reopen the Index from the new directory so subsequent reads use fresh data
        let new_index = Index::open_in_dir(index_dir)
            .map_err(|e| SearchError::Index(e.to_string()))?;
        if let Ok(mut guard) = self.index.lock() {
            *guard = new_index;
        }

        self.invalidate_writer();
        self.invalidate_query_parser();
        self.invalidate_reader();
        Ok(count)
    }

    /// Get the number of documents in the index
    pub fn num_docs(&self) -> Result<u64> {
        let reader = self.reader()?;
        Ok(reader.searcher().num_docs())
    }

    /// Check if a document exists in the index
    pub fn exists(&self, doc_id: &str) -> Result<bool> {
        let reader = self.reader()?;
        let searcher = reader.searcher();
        let term = Term::from_field_text(self.id_field, doc_id);
        let query = TermQuery::new(term, IndexRecordOption::Basic);
        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(1))
            .map_err(|e| SearchError::Index(e.to_string()))?;

        Ok(!top_docs.is_empty())
    }

    /// Search with both type and tag filters
    pub fn search_filtered(
        &self,
        query: &str,
        doc_type: Option<&str>,
        tags: &[&str],
        limit: usize,
    ) -> Result<Vec<(String, f64)>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let reader = self.reader()?;
        let searcher = reader.searcher();

        // Build full-text query using cached QueryParser
        let query_parser = self.query_parser()?;
        let text_query = query_parser
            .parse_query(&self.literalize_unknown_field_tokens(query))
            .map_err(|e| SearchError::Query(e.to_string()))?;

        // If no filters, just run the text query
        if doc_type.is_none() && tags.is_empty() {
            let top_docs = searcher
                .search(&text_query, &TopDocs::with_limit(limit))
                .map_err(|e| SearchError::Index(e.to_string()))?;

            return self.extract_results(&searcher, top_docs);
        }

        // Build boolean query with filters
        let mut clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> =
            vec![(Occur::Must, text_query)];

        // Add type filter
        if let Some(dtype) = doc_type {
            let type_term = Term::from_field_text(self.doc_type_field, dtype);
            let type_query = TermQuery::new(type_term, IndexRecordOption::Basic);
            clauses.push((Occur::Must, Box::new(type_query)));
        }

        // Add tag filters (all tags must be present)
        for tag in tags {
            let tag_term = Term::from_field_text(self.tags_field, tag);
            let tag_query = TermQuery::new(tag_term, IndexRecordOption::Basic);
            clauses.push((Occur::Must, Box::new(tag_query)));
        }

        let bool_query = BooleanQuery::new(clauses);

        let top_docs = searcher
            .search(&bool_query, &TopDocs::with_limit(limit))
            .map_err(|e| SearchError::Index(e.to_string()))?;

        self.extract_results(&searcher, top_docs)
    }

    /// Extract results from top docs
    fn extract_results(
        &self,
        searcher: &tantivy::Searcher,
        top_docs: Vec<(f32, tantivy::DocAddress)>,
    ) -> Result<Vec<(String, f64)>> {
        let mut results = Vec::with_capacity(top_docs.len());

        for (score, doc_addr) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_addr)
                .map_err(|e| SearchError::Index(e.to_string()))?;

            let id = doc
                .get_first(self.id_field)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            results.push((id, score as f64));
        }

        Ok(results)
    }
}

impl TextIndex for Bm25Index {
    fn index(&self, doc: &dyn SearchDocument) -> Result<()> {
        let mut writer = self.take_writer()?;

        // Delete existing document with same ID
        let id_term = Term::from_field_text(self.id_field, doc.doc_id());
        writer.delete_term(id_term);

        // Build tantivy document
        let mut tantivy_doc = TantivyDocument::new();
        tantivy_doc.add_text(self.id_field, doc.doc_id());
        tantivy_doc.add_text(self.content_field, doc.doc_content());
        tantivy_doc.add_text(self.doc_type_field, doc.doc_type());
        tantivy_doc.add_text(self.tags_field, doc.doc_tags().join(" "));

        if let Some(title) = doc.doc_title() {
            tantivy_doc.add_text(self.title_field, title);
        }

        writer
            .add_document(tantivy_doc)
            .map_err(|e| SearchError::Index(e.to_string()))?;
        writer
            .commit()
            .map_err(|e| SearchError::Index(e.to_string()))?;
        self.return_writer(writer);

        self.invalidate_reader();
        Ok(())
    }

    fn remove(&self, doc_id: &str) -> Result<()> {
        let mut writer = self.take_writer()?;
        let id_term = Term::from_field_text(self.id_field, doc_id);
        writer.delete_term(id_term);
        writer
            .commit()
            .map_err(|e| SearchError::Index(e.to_string()))?;
        self.return_writer(writer);

        self.invalidate_reader();
        Ok(())
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<(String, f64)>> {
        self.search_filtered(query, None, &[], limit)
    }

    fn search_with_type(
        &self,
        query: &str,
        doc_type: &str,
        limit: usize,
    ) -> Result<Vec<(String, f64)>> {
        self.search_filtered(query, Some(doc_type), &[], limit)
    }

    fn search_with_tags(
        &self,
        query: &str,
        tags: &[&str],
        limit: usize,
    ) -> Result<Vec<(String, f64)>> {
        self.search_filtered(query, None, tags, limit)
    }

    fn commit(&self) -> Result<()> {
        // Commits happen automatically after each operation
        // This is a no-op for explicit commit requests
        Ok(())
    }
}

// Allow Bm25Index to be shared across threads
unsafe impl Send for Bm25Index {}
unsafe impl Sync for Bm25Index {}

// =============================================================================
// Async implementation (requires `parallel` feature)
// =============================================================================

#[cfg(feature = "parallel")]
use std::sync::Arc;

#[cfg(feature = "parallel")]
use crate::traits::AsyncTextIndex;

/// Async wrapper for Bm25Index
///
/// Provides async methods that wrap CPU-bound Tantivy operations in `spawn_blocking`.
/// The inner `Bm25Index` is wrapped in `Arc` for safe sharing across tasks.
#[cfg(feature = "parallel")]
pub struct AsyncBm25Index {
    inner: Arc<Bm25Index>,
}

#[cfg(feature = "parallel")]
impl AsyncBm25Index {
    /// Create a new async wrapper around a Bm25Index
    pub fn new(index: Bm25Index) -> Self {
        Self {
            inner: Arc::new(index),
        }
    }

    /// Create from an existing Arc<Bm25Index>
    pub fn from_arc(index: Arc<Bm25Index>) -> Self {
        Self { inner: index }
    }

    /// Get a reference to the inner index for sync operations
    pub fn inner(&self) -> &Bm25Index {
        &self.inner
    }

    /// Get a clone of the inner Arc for sharing
    pub fn clone_inner(&self) -> Arc<Bm25Index> {
        Arc::clone(&self.inner)
    }
}

/// Helper struct for indexing that captures document data
#[cfg(feature = "parallel")]
struct IndexableDoc {
    id: String,
    content: String,
    doc_type: String,
    tags: Vec<String>,
    title: Option<String>,
}

#[cfg(feature = "parallel")]
impl SearchDocument for IndexableDoc {
    fn doc_id(&self) -> &str {
        &self.id
    }

    fn doc_content(&self) -> &str {
        &self.content
    }

    fn doc_type(&self) -> &str {
        &self.doc_type
    }

    fn doc_tags(&self) -> Vec<&str> {
        self.tags.iter().map(|s| s.as_str()).collect()
    }

    fn doc_metadata(&self) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::new()
    }

    fn doc_title(&self) -> Option<&str> {
        self.title.as_deref()
    }
}

#[cfg(feature = "parallel")]
impl AsyncTextIndex for AsyncBm25Index {
    async fn index_async<D: SearchDocument + Sync>(&self, doc: &D) -> Result<()> {
        let index = Arc::clone(&self.inner);

        // Capture document data to move into spawn_blocking
        let indexable = IndexableDoc {
            id: doc.doc_id().to_string(),
            content: doc.doc_content().to_string(),
            doc_type: doc.doc_type().to_string(),
            tags: doc.doc_tags().iter().map(|s| s.to_string()).collect(),
            title: doc.doc_title().map(|s| s.to_string()),
        };

        tokio::task::spawn_blocking(move || index.index(&indexable))
            .await
            .map_err(|e| SearchError::Index(format!("spawn_blocking: {}", e)))?
    }

    async fn remove_async(&self, doc_id: &str) -> Result<()> {
        let index = Arc::clone(&self.inner);
        let doc_id = doc_id.to_string();

        tokio::task::spawn_blocking(move || index.remove(&doc_id))
            .await
            .map_err(|e| SearchError::Index(format!("spawn_blocking: {}", e)))?
    }

    async fn search_async(&self, query: &str, limit: usize) -> Result<Vec<(String, f64)>> {
        let index = Arc::clone(&self.inner);
        let query = query.to_string();

        tokio::task::spawn_blocking(move || index.search(&query, limit))
            .await
            .map_err(|e| SearchError::Index(format!("spawn_blocking: {}", e)))?
    }

    async fn search_with_type_async(
        &self,
        query: &str,
        doc_type: &str,
        limit: usize,
    ) -> Result<Vec<(String, f64)>> {
        let index = Arc::clone(&self.inner);
        let query = query.to_string();
        let doc_type = doc_type.to_string();

        tokio::task::spawn_blocking(move || index.search_with_type(&query, &doc_type, limit))
            .await
            .map_err(|e| SearchError::Index(format!("spawn_blocking: {}", e)))?
    }

    async fn search_with_tags_async(
        &self,
        query: &str,
        tags: &[&str],
        limit: usize,
    ) -> Result<Vec<(String, f64)>> {
        let index = Arc::clone(&self.inner);
        let query = query.to_string();
        let tags: Vec<String> = tags.iter().map(|s| s.to_string()).collect();

        tokio::task::spawn_blocking(move || {
            let tag_refs: Vec<&str> = tags.iter().map(|s| s.as_str()).collect();
            index.search_with_tags(&query, &tag_refs, limit)
        })
        .await
        .map_err(|e| SearchError::Index(format!("spawn_blocking: {}", e)))?
    }

    async fn commit_async(&self) -> Result<()> {
        // Commits happen automatically, this is a no-op
        Ok(())
    }
}

#[cfg(feature = "parallel")]
impl Clone for AsyncBm25Index {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[cfg(test)]
#[path = "bm25_tests/tests.rs"]
mod tests;
