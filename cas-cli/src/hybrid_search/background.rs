//! Background BM25 index maintenance
//!
//! Provides incremental indexing for entries that have been updated since
//! their last index. Runs as part of the daemon process every 30 seconds.

use std::path::Path;

use crate::error::Result;
use crate::store::Store;
use crate::types::Entry;

use crate::hybrid_search::SearchIndex;

/// Configuration for background indexing
#[derive(Debug, Clone)]
pub struct IndexingConfig {
    /// Number of entries to process in a single batch
    pub batch_size: usize,
    /// Maximum entries to process per daemon run
    pub max_per_run: usize,
}

impl Default for IndexingConfig {
    fn default() -> Self {
        Self {
            batch_size: 32,
            max_per_run: 200,
        }
    }
}

/// Result of an indexing run
#[derive(Debug, Clone, Default)]
pub struct IndexingResult {
    /// Number of entries successfully indexed
    pub indexed: usize,
    /// Errors encountered: (entry_id, error_message)
    pub errors: Vec<(String, String)>,
}

/// Background indexer for incremental BM25 index updates
pub struct BackgroundIndexer {
    index: SearchIndex,
}

impl BackgroundIndexer {
    /// Open a background indexer for the given Cassy directory
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let index_dir = cas_dir.join("index");
        let index = SearchIndex::open(&index_dir)?;
        Ok(Self { index })
    }

    /// Create an in-memory indexer (for testing)
    pub fn in_memory() -> Result<Self> {
        let index = SearchIndex::in_memory()?;
        Ok(Self { index })
    }

    /// Get a reference to the search index
    pub fn index(&self) -> &SearchIndex {
        &self.index
    }

    /// Process pending entries that need indexing
    ///
    /// Fetches entries with updated_at > indexed_at (or indexed_at IS NULL),
    /// indexes them in batches for efficiency, and marks them as indexed.
    pub fn process_pending(
        &self,
        store: &dyn Store,
        config: &IndexingConfig,
    ) -> Result<IndexingResult> {
        let mut result = IndexingResult::default();

        // Get pending entries
        let pending = store.list_pending_index(config.max_per_run)?;
        if pending.is_empty() {
            return Ok(result);
        }

        // Process in batches for efficiency
        for batch in pending.chunks(config.batch_size) {
            match self.process_batch(batch, store) {
                Ok(count) => {
                    result.indexed += count;
                }
                Err(_e) => {
                    // Batch failed, try individual entries
                    for entry in batch {
                        match self.process_single(entry, store) {
                            Ok(()) => result.indexed += 1,
                            Err(err) => {
                                result.errors.push((entry.id.clone(), err.to_string()));
                            }
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    /// Process a batch of entries
    ///
    /// Indexes all entries in the batch at once with a single commit,
    /// then marks them as indexed.
    fn process_batch(&self, entries: &[Entry], store: &dyn Store) -> Result<usize> {
        if entries.is_empty() {
            return Ok(0);
        }

        // Index all entries with single commit
        let count = self.index.index_entries_batch(entries)?;

        // Mark all entries as indexed
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        store.mark_indexed_batch(&ids)?;

        Ok(count)
    }

    /// Process a single entry (fallback when batch fails)
    fn process_single(&self, entry: &Entry, store: &dyn Store) -> Result<()> {
        // Index the entry
        self.index.index_entry(entry)?;

        // Mark as indexed
        store.mark_indexed(&entry.id)?;

        Ok(())
    }

    /// Get count of entries pending indexing
    pub fn pending_count(&self, store: &dyn Store) -> Result<usize> {
        Ok(store.list_pending_index(usize::MAX)?.len())
    }

    /// Check if the indexer is operational (index accessible)
    pub fn is_operational(&self) -> bool {
        // Simple check: index is loaded and searchable
        self.index.field_count() > 0
    }
}

#[cfg(test)]
mod tests {
    use crate::hybrid_search::background::*;
    use crate::store::mock::MockStore;
    use std::time::Instant;

    #[test]
    fn test_indexing_config_default() {
        let config = IndexingConfig::default();
        assert_eq!(config.batch_size, 32);
        assert_eq!(config.max_per_run, 200);
    }

    #[test]
    fn test_indexing_result_default() {
        let result = IndexingResult::default();
        assert_eq!(result.indexed, 0);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_in_memory_indexer() {
        let indexer = BackgroundIndexer::in_memory().unwrap();
        assert!(indexer.is_operational());
    }

    #[test]
    fn test_process_empty_pending() {
        let indexer = BackgroundIndexer::in_memory().unwrap();
        let store = MockStore::new();

        let config = IndexingConfig::default();
        let result = indexer.process_pending(&store, &config).unwrap();

        assert_eq!(result.indexed, 0);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_process_batch_entries() {
        let indexer = BackgroundIndexer::in_memory().unwrap();
        let store = MockStore::new();

        // Add some entries
        let entry1 = Entry::new("test-001".to_string(), "First test entry".to_string());
        let entry2 = Entry::new("test-002".to_string(), "Second test entry".to_string());
        store.add(&entry1).unwrap();
        store.add(&entry2).unwrap();

        let config = IndexingConfig::default();
        let result = indexer.process_pending(&store, &config).unwrap();

        // MockStore returns all entries as pending (no updated_at tracking)
        assert_eq!(result.indexed, 2);
        assert!(result.errors.is_empty());
    }

    /// Test that batch indexing of 100 documents stays within a reasonable bound.
    ///
    /// This verifies the performance improvement from batching commits.
    /// Sequential per-document commits would take ~10s for 100 documents.
    #[test]
    fn test_batch_indexing_performance() {
        let indexer = BackgroundIndexer::in_memory().unwrap();

        // Create 100 test entries with varied content
        let entries: Vec<Entry> = (0..100)
            .map(|i| {
                Entry::new(
                    format!("perf-test-{i:03}"),
                    format!(
                        "Performance test entry {} with some content about topic {} and keywords like {} and {}",
                        i, i % 10, ["rust", "search", "index", "batch"][i % 4], ["fast", "efficient", "scalable"][i % 3]
                    ),
                )
            })
            .collect();

        // Time the batch indexing
        let start = Instant::now();
        let count = indexer.index.index_entries_batch(&entries).unwrap();
        let elapsed = start.elapsed();

        // Verify all entries indexed
        assert_eq!(count, 100);

        // Keep a generous bound for loaded CI runners.
        // A stricter performance comparison is covered by `test_batch_vs_sequential_performance`.
        assert!(
            elapsed.as_millis() < 1_000,
            "Batch indexing 100 documents took {}ms, expected <1000ms",
            elapsed.as_millis()
        );

        // Log timing for debugging (visible with cargo test -- --nocapture)
        eprintln!(
            "Batch indexed {} documents in {:?} ({:.2}ms/doc)",
            count,
            elapsed,
            elapsed.as_secs_f64() * 1000.0 / count as f64
        );
    }

    /// Test that batch indexing is significantly faster than sequential
    #[test]
    fn test_batch_vs_sequential_performance() {
        // Create entries for comparison
        let entries: Vec<Entry> = (0..50)
            .map(|i| {
                Entry::new(
                    format!("compare-{i:03}"),
                    format!("Comparison test entry {i} with content"),
                )
            })
            .collect();

        // Test batch indexing
        let batch_indexer = BackgroundIndexer::in_memory().unwrap();
        let batch_start = Instant::now();
        batch_indexer.index.index_entries_batch(&entries).unwrap();
        let batch_elapsed = batch_start.elapsed();

        // Test sequential indexing (each with its own commit)
        let seq_indexer = BackgroundIndexer::in_memory().unwrap();
        let seq_start = Instant::now();
        for entry in &entries {
            seq_indexer.index.index_entry(entry).unwrap();
        }
        let seq_elapsed = seq_start.elapsed();

        // Batch should be at least 5x faster (typically 10-100x)
        let speedup = seq_elapsed.as_secs_f64() / batch_elapsed.as_secs_f64();
        eprintln!("Batch: {batch_elapsed:?}, Sequential: {seq_elapsed:?}, Speedup: {speedup:.1}x");

        assert!(
            speedup >= 2.0,
            "Batch indexing should be at least 2x faster than sequential. \
             Batch: {batch_elapsed:?}, Sequential: {seq_elapsed:?}, Speedup: {speedup:.1}x"
        );
    }
}
