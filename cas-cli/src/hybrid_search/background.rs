//! Background BM25 index maintenance
//!
//! Provides incremental indexing for entries that have been updated since
//! their last index. Runs as part of the daemon process every 30 seconds.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

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
    rebuild_marker: Option<std::path::PathBuf>,
    rebuild_required: AtomicBool,
}

impl BackgroundIndexer {
    /// Open a background indexer for the given Cassy directory
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let index_dir = crate::hybrid_search::tantivy_index_dir(cas_dir);
        let index = SearchIndex::open(&index_dir)?;
        let rebuild_marker = crate::hybrid_search::tantivy_rebuild_marker(cas_dir);
        let rebuild_required = rebuild_marker.is_file();
        Ok(Self {
            index,
            rebuild_marker: Some(rebuild_marker),
            rebuild_required: AtomicBool::new(rebuild_required),
        })
    }

    /// Create an in-memory indexer (for testing)
    pub fn in_memory() -> Result<Self> {
        let index = SearchIndex::in_memory()?;
        Ok(Self {
            index,
            rebuild_marker: None,
            rebuild_required: AtomicBool::new(false),
        })
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

        // A mismatched pre-versioned index was quarantined during open. Its
        // indexed_at timestamps are no longer evidence that the new index is
        // complete, so requeue all entry rows exactly once before draining the
        // normal pending queue. The database update is durable even if the
        // process exits before the marker is removed.
        if self.rebuild_required.load(Ordering::Acquire) {
            let entries = store.list()?;
            let ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
            store.mark_index_pending_batch(&ids)?;
            if self.rebuild_required.swap(false, Ordering::AcqRel) {
                if let Some(marker) = &self.rebuild_marker {
                    let _ = std::fs::remove_file(marker);
                }
            }
        }

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
        let count = self.index.index_entries_batch_and_merge(entries)?;

        // Mark all entries as indexed
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        store.mark_indexed_batch(&ids)?;

        Ok(count)
    }

    /// Process a single entry (fallback when batch fails)
    fn process_single(&self, entry: &Entry, store: &dyn Store) -> Result<()> {
        // Keep the fallback on the same merge-completing commit path. A batch
        // error must not reintroduce one permanent segment per entry.
        self.index.index_entries_batch_and_merge(std::slice::from_ref(entry))?;

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
    use crate::hooks::HybridContextScorer;
    use crate::hybrid_search::background::*;
    use crate::hybrid_search::{DocType, HybridSearch, HybridSearchOptions, SearchOptions};
    use crate::store::mock::MockStore;
    use crate::store::open_store;
    use crate::types::MemoryTier;
    use cas_core::hooks::{ContextQuery, ContextScorer};
    use std::time::Instant;

    fn create_mismatched_legacy_index(path: &std::path::Path) {
        use tantivy::schema::{Schema, STORED, TEXT};

        std::fs::create_dir_all(path).expect("create legacy index directory");
        let mut builder = Schema::builder();
        builder.add_text_field("id", TEXT | STORED);
        builder.add_text_field("content", TEXT);
        let index = tantivy::Index::create_in_dir(path, builder.build()).expect("create legacy index");
        let mut writer = index.writer(15_000_000).expect("legacy writer");
        writer
            .add_document(tantivy::doc!(
                index.schema().get_field("id").expect("id field") => "legacy-entry",
                index.schema().get_field("content").expect("content field") => "legacy content"
            ))
            .expect("write legacy document");
        writer.commit().expect("commit legacy document");
    }

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

    /// Regression for cas-bc42: the daemon's pending-entry writer and every
    /// query reader must resolve the same on-disk Tantivy index.
    #[test]
    fn background_only_entry_is_visible_to_hybrid_search() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("create .cas");
        let store = open_store(&cas_root).expect("store");
        let entry = Entry {
            memory_tier: MemoryTier::Working,
            ..Entry::new(
                "daemon-only-entry".to_string(),
                "quasarplum daemon-only indexing regression".to_string(),
            )
        };
        store.add(&entry).expect("add entry");

        let indexer = BackgroundIndexer::open(&cas_root).expect("background indexer");
        let result = indexer
            .process_pending(store.as_ref(), &IndexingConfig::default())
            .expect("process pending");
        assert_eq!(result.indexed, 1);

        let search = HybridSearch::open(&cas_root).expect("hybrid search");
        let entries = store.list().expect("entries");
        let results = search
            .search(
                &HybridSearchOptions {
                    base: SearchOptions {
                        query: "quasarplum".to_string(),
                        doc_types: vec![DocType::Entry],
                        ..Default::default()
                    },
                    enable_temporal: false,
                    enable_graph: false,
                    ..Default::default()
                },
                &entries,
            )
            .expect("search");

        assert!(
            results.iter().any(|result| result.id == entry.id),
            "daemon-indexed entry must be visible to the reader: {results:?}"
        );
        assert!(
            !cas_root.join("index/meta.json").exists(),
            "background indexing must not create a second Tantivy root"
        );

        let scorer = HybridContextScorer::open(&cas_root).expect("helpful-memory scorer");
        let scored = scorer.score_entries(
            &entries,
            &ContextQuery {
                user_prompt: Some("quasarplum daemon-only".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(
            scored.first().map(|(entry, _)| entry.id.as_str()),
            Some(entry.id.as_str()),
            "Helpful Memories scorer must see the daemon-only document"
        );
    }

    #[test]
    fn constructors_share_the_canonical_tantivy_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cas_root = temp.path().join(".cas");
        let canonical = crate::hybrid_search::tantivy_index_dir(&cas_root);

        let _background = BackgroundIndexer::open(&cas_root).expect("background indexer");
        let _reader = HybridSearch::open(&cas_root).expect("hybrid reader");

        assert!(canonical.join("meta.json").exists());
        assert!(!cas_root.join("index/meta.json").exists());
    }

    #[test]
    fn compatible_pre_versioned_index_moves_once_and_remains_searchable() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("create .cas");
        let entry = Entry::new(
            "compatible-legacy".to_string(),
            "compatible legacy index survives migration".to_string(),
        );
        let legacy_dir = crate::hybrid_search::legacy_tantivy_index_dir(&cas_root);
        let legacy = SearchIndex::open(&legacy_dir).expect("legacy index");
        legacy.index_entry(&entry).expect("index legacy entry");
        drop(legacy);

        let current_dir = crate::hybrid_search::tantivy_index_dir(&cas_root);
        let migrated = SearchIndex::open(&current_dir).expect("migrate index");
        assert!(current_dir.join("meta.json").is_file());
        assert!(!legacy_dir.exists(), "migration must retire the old path once");
        let results = migrated
            .search(
                &SearchOptions {
                    query: "survives migration".to_string(),
                    ..Default::default()
                },
                std::slice::from_ref(&entry),
            )
            .expect("search migrated index");
        assert_eq!(results.first().map(|result| result.id.as_str()), Some(entry.id.as_str()));

        // A second open sees only the versioned path and must not repeat or
        // recreate the legacy directory.
        drop(migrated);
        let _again = SearchIndex::open(&current_dir).expect("reopen migrated index");
        assert!(!legacy_dir.exists());
    }

    #[test]
    fn mismatched_pre_versioned_index_is_quarantined_and_rebuilt_once() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("create .cas");
        let store = open_store(&cas_root).expect("store");
        let entry = Entry::new(
            "mismatched-legacy".to_string(),
            "mismatched legacy index is rebuilt safely".to_string(),
        );
        store.add(&entry).expect("add entry");
        store.mark_indexed(&entry.id).expect("simulate old index state");

        let legacy_dir = crate::hybrid_search::legacy_tantivy_index_dir(&cas_root);
        create_mismatched_legacy_index(&legacy_dir);
        let current_dir = crate::hybrid_search::tantivy_index_dir(&cas_root);
        let marker = crate::hybrid_search::tantivy_rebuild_marker(&cas_root);

        let indexer = BackgroundIndexer::open(&cas_root).expect("open versioned index");
        assert!(current_dir.join("meta.json").is_file());
        assert!(!legacy_dir.exists(), "mismatched path must be quarantined, not deleted");
        assert!(marker.is_file(), "rebuild marker must survive the open");
        assert!(cas_root.join("index/tantivy-legacy-v2").is_dir());

        let first = indexer
            .process_pending(store.as_ref(), &IndexingConfig::default())
            .expect("rebuild pending entries");
        assert_eq!(first.indexed, 1);
        assert!(!marker.exists(), "background indexing consumes the marker once");

        let second = BackgroundIndexer::open(&cas_root)
            .expect("reopen rebuilt index")
            .process_pending(store.as_ref(), &IndexingConfig::default())
            .expect("drain second cycle");
        assert_eq!(second.indexed, 0, "the migration must not requeue repeatedly");
        let entries = store.list().expect("list entries");
        let results = indexer
            .index()
            .search(
                &SearchOptions {
                    query: "rebuilt safely".to_string(),
                    ..Default::default()
                },
                &entries,
            )
            .expect("search rebuilt index");
        assert_eq!(results.first().map(|result| result.id.as_str()), Some(entry.id.as_str()));
    }

    #[test]
    fn mismatched_versioned_open_preserves_the_existing_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cas_root = temp.path().join(".cas");
        let current_dir = crate::hybrid_search::tantivy_index_dir(&cas_root);
        create_mismatched_legacy_index(&current_dir);
        let metadata = current_dir.join("meta.json");
        let metadata_before = std::fs::read(&metadata).expect("read mismatched metadata");

        let error = match SearchIndex::open(&current_dir) {
            Ok(_) => panic!("mismatch must fail closed"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("schema mismatch"));
        assert!(metadata.is_file(), "open must not delete a mismatched index");
        assert_eq!(
            std::fs::read(&metadata).expect("read preserved metadata"),
            metadata_before,
            "mismatch handling must leave the existing index byte-for-byte intact"
        );
    }

    #[test]
    fn explicit_rebuild_clears_documents_after_migration() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("create .cas");
        let entry = Entry::new(
            "rebuild-clears".to_string(),
            "stale document must not survive explicit rebuild".to_string(),
        );
        let current_dir = crate::hybrid_search::tantivy_index_dir(&cas_root);
        let index = SearchIndex::open(&current_dir).expect("open index");
        index.index_entry(&entry).expect("index entry");
        drop(index);

        let rebuilt = SearchIndex::rebuild(&current_dir).expect("rebuild index");
        let results = rebuilt
            .search(
                &SearchOptions {
                    query: "stale document".to_string(),
                    ..Default::default()
                },
                std::slice::from_ref(&entry),
            )
            .expect("search rebuilt index");
        assert!(results.is_empty(), "explicit rebuild must clear prior documents");
    }

    #[test]
    fn background_batches_wait_for_segment_merges() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("create .cas");
        let store = open_store(&cas_root).expect("store");
        let indexer = BackgroundIndexer::open(&cas_root).expect("background indexer");
        let config = IndexingConfig {
            batch_size: 1,
            max_per_run: 1,
        };

        for ordinal in 0..64 {
            let entry = Entry::new(
                format!("segment-{ordinal:03}"),
                format!("segment merge regression document {ordinal}"),
            );
            store.add(&entry).expect("add entry");
            assert_eq!(
                indexer
                    .process_pending(store.as_ref(), &config)
                    .expect("process one batch")
                    .indexed,
                1
            );
        }

        let segments = indexer.index().segment_count().expect("segment count");
        assert!(
            segments <= 15,
            "64 one-document daemon batches left {segments} searchable segments"
        );
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
