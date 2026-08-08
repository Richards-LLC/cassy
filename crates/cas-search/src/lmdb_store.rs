//! LMDB-based vector storage using heed
//!
//! This module provides a vector store implementation backed by LMDB via the
//! `heed` crate (Meilisearch's Rust LMDB wrapper).
//!
//! # Why LMDB?
//!
//! - Battle-tested since 2011 (used by OpenLDAP, Meilisearch, HyperDex)
//! - Zero-copy reads via memory-mapped files
//! - ACID transactions with crash recovery
//! - Read performance scales linearly with threads
//!
//! # Storage Structure
//!
//! The store uses two LMDB databases:
//! - `vectors`: Maps document IDs to embedding vectors (as bytes)
//! - `metadata`: Stores configuration like dimension, model info
//!
//! # Example
//!
//! ```rust,ignore
//! use cas_search::lmdb_store::LmdbVectorStore;
//!
//! let store = LmdbVectorStore::open("./vectors.lmdb", 1024)?;
//! store.store("doc-001", &embedding)?;
//! let retrieved = store.get("doc-001")?;
//! ```

use std::path::Path;
use std::sync::RwLock;

use heed::types::{Bytes, Str};
use heed::{Database, Env, EnvOpenOptions};

use crate::error::{Result, SearchError};
use crate::traits::VectorStore;

/// Default LMDB map size (10 GB) - auto-grows on Linux, fixed on other platforms
const DEFAULT_MAP_SIZE: usize = 10 * 1024 * 1024 * 1024;

/// Maximum number of named databases in the environment
const MAX_DBS: u32 = 10;

/// LMDB-backed vector store
///
/// Stores embeddings as raw bytes keyed by document ID.
/// Supports batch operations with single transactions for efficiency.
pub struct LmdbVectorStore {
    /// LMDB environment
    env: Env,
    /// Vectors database: doc_id -> embedding bytes
    vectors: Database<Str, Bytes>,
    /// Metadata database: key -> value
    metadata: Database<Str, Str>,
    /// Embedding dimension
    dimension: usize,
    /// Write lock for serializing writes (LMDB only allows one writer)
    write_lock: RwLock<()>,
}

impl LmdbVectorStore {
    /// Open or create an LMDB vector store
    ///
    /// # Arguments
    /// * `path` - Directory path for the LMDB environment
    /// * `dimension` - Expected embedding dimension
    ///
    /// # Errors
    /// Returns an error if the path cannot be created or the environment cannot be opened.
    pub fn open(path: impl AsRef<Path>, dimension: usize) -> Result<Self> {
        Self::open_with_config(path, dimension, DEFAULT_MAP_SIZE)
    }

    /// Open an existing environment without creating databases or writing
    /// dimension metadata. This is the query-time counterpart to [`Self::open`].
    pub fn open_existing(path: impl AsRef<Path>, dimension: usize) -> Result<Option<Self>> {
        let path = path.as_ref();
        if !path.is_dir() {
            return Ok(None);
        }
        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(DEFAULT_MAP_SIZE)
                .max_dbs(MAX_DBS)
                .open(path)
                .map_err(|e| SearchError::storage(format!("LMDB open existing: {e}")))?
        };
        let rtxn = env
            .read_txn()
            .map_err(|e| SearchError::storage(format!("LMDB read txn: {e}")))?;
        let Some(vectors) = env
            .open_database::<Str, Bytes>(&rtxn, Some("vectors"))
            .map_err(|e| SearchError::storage(format!("LMDB open vectors db: {e}")))?
        else {
            return Ok(None);
        };
        let Some(metadata) = env
            .open_database::<Str, Str>(&rtxn, Some("metadata"))
            .map_err(|e| SearchError::storage(format!("LMDB open metadata db: {e}")))?
        else {
            return Ok(None);
        };
        let stored_dimension = metadata
            .get(&rtxn, "dimension")
            .map_err(|e| SearchError::storage(format!("LMDB read dimension: {e}")))?
            .ok_or_else(|| SearchError::storage("LMDB dimension metadata is missing"))?
            .parse::<usize>()
            .map_err(|_| SearchError::storage("Invalid dimension in metadata"))?;
        if stored_dimension != dimension {
            return Err(SearchError::DimensionMismatch {
                expected: stored_dimension,
                actual: dimension,
            });
        }
        rtxn.commit()
            .map_err(|e| SearchError::storage(format!("LMDB read txn commit: {e}")))?;
        Ok(Some(Self {
            env,
            vectors,
            metadata,
            dimension,
            write_lock: RwLock::new(()),
        }))
    }

    /// Open with custom map size
    ///
    /// # Arguments
    /// * `path` - Directory path for the LMDB environment
    /// * `dimension` - Expected embedding dimension
    /// * `map_size` - Maximum size of the memory map (in bytes)
    pub fn open_with_config(
        path: impl AsRef<Path>,
        dimension: usize,
        map_size: usize,
    ) -> Result<Self> {
        let path = path.as_ref();

        // Create directory if it doesn't exist
        std::fs::create_dir_all(path)?;

        // Open LMDB environment
        // Safety: We're opening with standard flags, no unsafe memory operations
        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(map_size)
                .max_dbs(MAX_DBS)
                .open(path)
                .map_err(|e| SearchError::storage(format!("LMDB open: {e}")))?
        };

        // Create or open databases
        let mut wtxn = env
            .write_txn()
            .map_err(|e| SearchError::storage(format!("LMDB write txn: {e}")))?;

        let vectors: Database<Str, Bytes> = env
            .create_database(&mut wtxn, Some("vectors"))
            .map_err(|e| SearchError::storage(format!("LMDB create vectors db: {e}")))?;

        let metadata: Database<Str, Str> = env
            .create_database(&mut wtxn, Some("metadata"))
            .map_err(|e| SearchError::storage(format!("LMDB create metadata db: {e}")))?;

        // Store dimension in metadata if not already set
        let stored_dim = metadata
            .get(&wtxn, "dimension")
            .map_err(|e| SearchError::storage(format!("LMDB read: {e}")))?;

        match stored_dim {
            Some(dim_str) => {
                let stored_dimension: usize = dim_str
                    .parse()
                    .map_err(|_| SearchError::storage("Invalid dimension in metadata"))?;
                if stored_dimension != dimension {
                    return Err(SearchError::DimensionMismatch {
                        expected: stored_dimension,
                        actual: dimension,
                    });
                }
            }
            None => {
                metadata
                    .put(&mut wtxn, "dimension", &dimension.to_string())
                    .map_err(|e| SearchError::storage(format!("LMDB write: {e}")))?;
            }
        }

        wtxn.commit()
            .map_err(|e| SearchError::storage(format!("LMDB commit: {e}")))?;

        Ok(Self {
            env,
            vectors,
            metadata,
            dimension,
            write_lock: RwLock::new(()),
        })
    }

    /// Store multiple embeddings in a single transaction (batch insert)
    ///
    /// This is more efficient than calling `store()` multiple times
    /// because it uses a single write transaction.
    ///
    /// # Arguments
    /// * `items` - Iterator of (doc_id, embedding) pairs
    ///
    /// # Returns
    /// Number of items stored successfully
    pub fn store_batch<'a>(
        &self,
        items: impl IntoIterator<Item = (&'a str, &'a [f32])>,
    ) -> Result<usize> {
        let _lock = self
            .write_lock
            .write()
            .map_err(|_| SearchError::storage("Write lock poisoned"))?;

        let mut wtxn = self
            .env
            .write_txn()
            .map_err(|e| SearchError::storage(format!("LMDB write txn: {e}")))?;

        let mut count = 0;
        for (doc_id, embedding) in items {
            if embedding.len() != self.dimension {
                continue; // Skip mismatched dimensions in batch
            }

            let bytes = embedding_to_bytes(embedding);
            self.vectors
                .put(&mut wtxn, doc_id, &bytes)
                .map_err(|e| SearchError::storage(format!("LMDB put: {e}")))?;
            count += 1;
        }

        wtxn.commit()
            .map_err(|e| SearchError::storage(format!("LMDB commit: {e}")))?;

        Ok(count)
    }

    /// Delete multiple embeddings in a single transaction
    pub fn delete_batch<'a>(&self, doc_ids: impl IntoIterator<Item = &'a str>) -> Result<usize> {
        let _lock = self
            .write_lock
            .write()
            .map_err(|_| SearchError::storage("Write lock poisoned"))?;

        let mut wtxn = self
            .env
            .write_txn()
            .map_err(|e| SearchError::storage(format!("LMDB write txn: {e}")))?;

        let mut count = 0;
        for doc_id in doc_ids {
            if self
                .vectors
                .delete(&mut wtxn, doc_id)
                .map_err(|e| SearchError::storage(format!("LMDB delete: {e}")))?
            {
                count += 1;
            }
        }

        wtxn.commit()
            .map_err(|e| SearchError::storage(format!("LMDB commit: {e}")))?;

        Ok(count)
    }

    /// Get metadata value by key
    pub fn get_metadata(&self, key: &str) -> Result<Option<String>> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| SearchError::storage(format!("LMDB read txn: {e}")))?;

        let value = self
            .metadata
            .get(&rtxn, key)
            .map_err(|e| SearchError::storage(format!("LMDB read: {e}")))?;

        Ok(value.map(|s| s.to_string()))
    }

    /// Set metadata value
    pub fn set_metadata(&self, key: &str, value: &str) -> Result<()> {
        let _lock = self
            .write_lock
            .write()
            .map_err(|_| SearchError::storage("Write lock poisoned"))?;

        let mut wtxn = self
            .env
            .write_txn()
            .map_err(|e| SearchError::storage(format!("LMDB write txn: {e}")))?;

        self.metadata
            .put(&mut wtxn, key, value)
            .map_err(|e| SearchError::storage(format!("LMDB write: {e}")))?;

        wtxn.commit()
            .map_err(|e| SearchError::storage(format!("LMDB commit: {e}")))?;

        Ok(())
    }

    /// Sync data to disk (force flush)
    pub fn sync(&self) -> Result<()> {
        self.env
            .force_sync()
            .map_err(|e| SearchError::storage(format!("LMDB sync: {e}")))?;
        Ok(())
    }

    /// Get LMDB environment info (for diagnostics)
    pub fn env_info(&self) -> LmdbEnvInfo {
        LmdbEnvInfo {
            map_size: DEFAULT_MAP_SIZE,
            dimension: self.dimension,
        }
    }
}

impl VectorStore for LmdbVectorStore {
    fn store(&self, doc_id: &str, embedding: &[f32]) -> Result<()> {
        if embedding.len() != self.dimension {
            return Err(SearchError::DimensionMismatch {
                expected: self.dimension,
                actual: embedding.len(),
            });
        }

        let _lock = self
            .write_lock
            .write()
            .map_err(|_| SearchError::storage("Write lock poisoned"))?;

        let mut wtxn = self
            .env
            .write_txn()
            .map_err(|e| SearchError::storage(format!("LMDB write txn: {e}")))?;

        let bytes = embedding_to_bytes(embedding);
        self.vectors
            .put(&mut wtxn, doc_id, &bytes)
            .map_err(|e| SearchError::storage(format!("LMDB put: {e}")))?;

        wtxn.commit()
            .map_err(|e| SearchError::storage(format!("LMDB commit: {e}")))?;

        Ok(())
    }

    fn get(&self, doc_id: &str) -> Result<Option<Vec<f32>>> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| SearchError::storage(format!("LMDB read txn: {e}")))?;

        let bytes = self
            .vectors
            .get(&rtxn, doc_id)
            .map_err(|e| SearchError::storage(format!("LMDB read: {e}")))?;

        match bytes {
            Some(data) => Ok(Some(bytes_to_embedding(data))),
            None => Ok(None),
        }
    }

    fn delete(&self, doc_id: &str) -> Result<()> {
        let _lock = self
            .write_lock
            .write()
            .map_err(|_| SearchError::storage("Write lock poisoned"))?;

        let mut wtxn = self
            .env
            .write_txn()
            .map_err(|e| SearchError::storage(format!("LMDB write txn: {e}")))?;

        self.vectors
            .delete(&mut wtxn, doc_id)
            .map_err(|e| SearchError::storage(format!("LMDB delete: {e}")))?;

        wtxn.commit()
            .map_err(|e| SearchError::storage(format!("LMDB commit: {e}")))?;

        Ok(())
    }

    fn search(&self, _query: &[f32], _k: usize) -> Result<Vec<(String, f32)>> {
        // LmdbVectorStore is a key-value store, not a similarity search engine.
        // Semantic search is available via cloud API as a premium feature.
        //
        // This method exists to satisfy the VectorStore trait but should not be
        // used directly for search operations.
        Err(SearchError::storage(
            "LmdbVectorStore does not support similarity search. Use cloud API for semantic search.",
        ))
    }

    fn exists(&self, doc_id: &str) -> Result<bool> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| SearchError::storage(format!("LMDB read txn: {e}")))?;

        let exists = self
            .vectors
            .get(&rtxn, doc_id)
            .map_err(|e| SearchError::storage(format!("LMDB read: {e}")))?
            .is_some();

        Ok(exists)
    }

    fn count(&self) -> Result<usize> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| SearchError::storage(format!("LMDB read txn: {e}")))?;

        let count = self
            .vectors
            .len(&rtxn)
            .map_err(|e| SearchError::storage(format!("LMDB len: {e}")))?;

        Ok(count as usize)
    }

    fn list_ids(&self) -> Result<Vec<String>> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| SearchError::storage(format!("LMDB read txn: {e}")))?;

        let mut ids = Vec::new();
        let iter = self
            .vectors
            .iter(&rtxn)
            .map_err(|e| SearchError::storage(format!("LMDB iter: {e}")))?;

        for result in iter {
            let (key, _) = result.map_err(|e| SearchError::storage(format!("LMDB iter: {e}")))?;
            ids.push(key.to_string());
        }

        Ok(ids)
    }

    fn dimension(&self) -> usize {
        self.dimension
    }
}

/// Environment info for diagnostics
#[derive(Debug, Clone)]
pub struct LmdbEnvInfo {
    /// Map size in bytes
    pub map_size: usize,
    /// Embedding dimension
    pub dimension: usize,
}

/// Convert embedding to bytes (little-endian f32)
fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(embedding.len() * 4);
    for &val in embedding {
        bytes.extend_from_slice(&val.to_le_bytes());
    }
    bytes
}

/// Convert bytes to embedding (little-endian f32)
fn bytes_to_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::lmdb_store::*;
    use tempfile::tempdir;

    fn random_embedding(dim: usize, seed: u64) -> Vec<f32> {
        // Simple deterministic "random" for testing
        (0..dim)
            .map(|i| ((seed * 31 + i as u64) % 1000) as f32 / 1000.0)
            .collect()
    }

    #[test]
    fn test_store_and_get() {
        let dir = tempdir().unwrap();
        let store = LmdbVectorStore::open(dir.path(), 128).unwrap();

        let embedding = random_embedding(128, 42);
        store.store("doc-001", &embedding).unwrap();

        let retrieved = store.get("doc-001").unwrap().unwrap();
        assert_eq!(retrieved.len(), 128);

        // Verify values match
        for (a, b) in embedding.iter().zip(retrieved.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn test_get_nonexistent() {
        let dir = tempdir().unwrap();
        let store = LmdbVectorStore::open(dir.path(), 128).unwrap();

        let result = store.get("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_delete() {
        let dir = tempdir().unwrap();
        let store = LmdbVectorStore::open(dir.path(), 128).unwrap();

        let embedding = random_embedding(128, 42);
        store.store("doc-001", &embedding).unwrap();
        assert!(store.exists("doc-001").unwrap());

        store.delete("doc-001").unwrap();
        assert!(!store.exists("doc-001").unwrap());
    }

    #[test]
    fn test_count() {
        let dir = tempdir().unwrap();
        let store = LmdbVectorStore::open(dir.path(), 128).unwrap();

        assert_eq!(store.count().unwrap(), 0);

        store.store("a", &random_embedding(128, 1)).unwrap();
        store.store("b", &random_embedding(128, 2)).unwrap();
        store.store("c", &random_embedding(128, 3)).unwrap();

        assert_eq!(store.count().unwrap(), 3);
    }

    #[test]
    fn test_list_ids() {
        let dir = tempdir().unwrap();
        let store = LmdbVectorStore::open(dir.path(), 128).unwrap();

        store.store("alpha", &random_embedding(128, 1)).unwrap();
        store.store("beta", &random_embedding(128, 2)).unwrap();
        store.store("gamma", &random_embedding(128, 3)).unwrap();

        let mut ids = store.list_ids().unwrap();
        ids.sort();
        assert_eq!(ids, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn test_dimension_mismatch() {
        let dir = tempdir().unwrap();
        let store = LmdbVectorStore::open(dir.path(), 128).unwrap();

        let wrong_dim = random_embedding(256, 42);
        let result = store.store("doc", &wrong_dim);

        assert!(result.is_err());
        match result.unwrap_err() {
            SearchError::DimensionMismatch { expected, actual } => {
                assert_eq!(expected, 128);
                assert_eq!(actual, 256);
            }
            _ => panic!("Expected DimensionMismatch error"),
        }
    }

    #[test]
    fn test_persistence() {
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();

        // Store data
        {
            let store = LmdbVectorStore::open(&path, 128).unwrap();
            store.store("doc-001", &random_embedding(128, 42)).unwrap();
            store.store("doc-002", &random_embedding(128, 43)).unwrap();
        }

        // Reopen and verify
        {
            let store = LmdbVectorStore::open(&path, 128).unwrap();
            assert_eq!(store.count().unwrap(), 2);
            assert!(store.exists("doc-001").unwrap());
            assert!(store.exists("doc-002").unwrap());
        }
    }

    #[test]
    fn test_batch_store() {
        let dir = tempdir().unwrap();
        let store = LmdbVectorStore::open(dir.path(), 128).unwrap();

        let items: Vec<_> = (0..100)
            .map(|i| {
                let id = format!("doc-{i:03}");
                let emb = random_embedding(128, i as u64);
                (id, emb)
            })
            .collect();

        let refs: Vec<_> = items
            .iter()
            .map(|(id, emb)| (id.as_str(), emb.as_slice()))
            .collect();

        let count = store.store_batch(refs).unwrap();
        assert_eq!(count, 100);
        assert_eq!(store.count().unwrap(), 100);
    }

    #[test]
    fn test_batch_delete() {
        let dir = tempdir().unwrap();
        let store = LmdbVectorStore::open(dir.path(), 128).unwrap();

        // Store 10 docs
        for i in 0..10 {
            store
                .store(&format!("doc-{i}"), &random_embedding(128, i))
                .unwrap();
        }

        // Delete odd-numbered docs
        let to_delete: Vec<_> = (0..10).step_by(2).map(|i| format!("doc-{i}")).collect();
        let refs: Vec<_> = to_delete.iter().map(|s| s.as_str()).collect();

        let deleted = store.delete_batch(refs).unwrap();
        assert_eq!(deleted, 5);
        assert_eq!(store.count().unwrap(), 5);
    }

    #[test]
    fn test_metadata() {
        let dir = tempdir().unwrap();
        let store = LmdbVectorStore::open(dir.path(), 128).unwrap();

        store.set_metadata("model", "qwen3-0.6b").unwrap();
        let model = store.get_metadata("model").unwrap();
        assert_eq!(model, Some("qwen3-0.6b".to_string()));

        let missing = store.get_metadata("nonexistent").unwrap();
        assert!(missing.is_none());
    }

    #[test]
    fn test_dimension_stored_in_metadata() {
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();

        // Create with dimension 512
        {
            let store = LmdbVectorStore::open(&path, 512).unwrap();
            assert_eq!(store.dimension(), 512);
        }

        // Reopen with same dimension - should work
        {
            let store = LmdbVectorStore::open(&path, 512).unwrap();
            assert_eq!(store.dimension(), 512);
        }

        // Try to open with different dimension - should fail
        {
            let result = LmdbVectorStore::open(&path, 1024);
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_embedding_byte_conversion() {
        let original = vec![1.0f32, -2.5, 0.0, std::f32::consts::PI, f32::MAX, f32::MIN];
        let bytes = embedding_to_bytes(&original);
        let restored = bytes_to_embedding(&bytes);

        assert_eq!(original.len(), restored.len());
        for (a, b) in original.iter().zip(restored.iter()) {
            assert_eq!(a, b);
        }
    }

    #[test]
    fn test_10k_vectors_benchmark() {
        let dir = tempdir().unwrap();
        let store = LmdbVectorStore::open(dir.path(), 1024).unwrap();

        // Generate 10k vectors
        let items: Vec<_> = (0..10_000)
            .map(|i| {
                let id = format!("doc-{i:05}");
                let emb = random_embedding(1024, i as u64);
                (id, emb)
            })
            .collect();

        // Batch insert
        let start = std::time::Instant::now();
        let refs: Vec<_> = items
            .iter()
            .map(|(id, emb)| (id.as_str(), emb.as_slice()))
            .collect();
        let count = store.store_batch(refs).unwrap();
        let insert_time = start.elapsed();

        assert_eq!(count, 10_000);
        println!(
            "10k vector insert: {:?} ({:.0} vectors/sec)",
            insert_time,
            10_000.0 / insert_time.as_secs_f64()
        );

        // Random reads
        let start = std::time::Instant::now();
        for i in (0..1000).map(|x| x * 10) {
            let id = format!("doc-{i:05}");
            let _ = store.get(&id).unwrap();
        }
        let read_time = start.elapsed();
        println!(
            "1k random reads: {:?} ({:.0} reads/sec)",
            read_time,
            1000.0 / read_time.as_secs_f64()
        );

        // Count operation
        let start = std::time::Instant::now();
        let total = store.count().unwrap();
        let count_time = start.elapsed();
        assert_eq!(total, 10_000);
        println!("Count operation: {count_time:?}");
    }

    #[test]
    fn test_concurrent_reads() {
        use std::sync::Arc;
        use std::thread;

        let dir = tempdir().unwrap();
        let store = Arc::new(LmdbVectorStore::open(dir.path(), 128).unwrap());

        // Pre-populate with 100 vectors
        for i in 0..100 {
            store
                .store(&format!("doc-{i}"), &random_embedding(128, i))
                .unwrap();
        }

        // Spawn multiple reader threads
        let mut handles = Vec::new();
        for thread_id in 0..4 {
            let store_clone = Arc::clone(&store);
            let handle = thread::spawn(move || {
                let mut reads = 0;
                for i in 0..100 {
                    let doc_id = format!("doc-{}", (i + thread_id * 25) % 100);
                    if store_clone.get(&doc_id).unwrap().is_some() {
                        reads += 1;
                    }
                }
                reads
            });
            handles.push(handle);
        }

        // All threads should complete successfully
        let mut total_reads = 0;
        for handle in handles {
            total_reads += handle.join().unwrap();
        }

        assert_eq!(total_reads, 400);
    }
}
