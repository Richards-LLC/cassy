use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use chrono::Utc;

use crate::store::Store;
use crate::store::mock::id_counter::IdCounter;
use crate::types::{Entry, EntryType, MemoryTier, Scope};
use cas_store::{Result, StoreError};

/// In-memory mock implementation of the Store trait.
#[derive(Debug)]
pub struct MockStore {
    entries: RwLock<HashMap<String, Entry>>,
    archived: RwLock<HashMap<String, Entry>>,
    id_counter: IdCounter,
    cas_dir: PathBuf,
    error_on_next: RwLock<Option<StoreError>>,
}

impl Default for MockStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MockStore {
    /// Create a new empty mock store.
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            archived: RwLock::new(HashMap::new()),
            id_counter: IdCounter::default(),
            cas_dir: PathBuf::from("/tmp/cas-mock"),
            error_on_next: RwLock::new(None),
        }
    }

    /// Create a mock store with pre-populated test data.
    pub fn with_entries(entries: Vec<Entry>) -> Self {
        let store = Self::new();
        {
            let mut map = store.entries.write().unwrap();
            for entry in entries {
                map.insert(entry.id.clone(), entry);
            }
        }
        store
    }

    /// Inject an error that will be returned on the next operation.
    pub fn inject_error(&self, error: StoreError) {
        *self.error_on_next.write().unwrap() = Some(error);
    }

    fn check_error(&self) -> Result<()> {
        let mut error = self.error_on_next.write().unwrap();
        if let Some(value) = error.take() {
            return Err(value);
        }
        Ok(())
    }

    /// Get the number of entries (for testing).
    pub fn len(&self) -> usize {
        self.entries.read().unwrap().len()
    }

    /// Check if store is empty (for testing).
    pub fn is_empty(&self) -> bool {
        self.entries.read().unwrap().is_empty()
    }
}

impl Store for MockStore {
    fn init(&self) -> Result<()> {
        self.check_error()
    }

    fn generate_id(&self) -> Result<String> {
        self.check_error()?;
        let date = Utc::now().format("%Y-%m-%d");
        let counter = self.id_counter.next();
        Ok(format!("{date}-{counter:03}"))
    }

    fn add(&self, entry: &Entry) -> Result<()> {
        self.check_error()?;
        let mut entries = self.entries.write().unwrap();
        if entries.contains_key(&entry.id) {
            return Err(StoreError::EntryExists(entry.id.clone()));
        }
        entries.insert(entry.id.clone(), entry.clone());
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Entry> {
        self.check_error()?;
        let entries = self.entries.read().unwrap();
        entries
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    fn get_archived(&self, id: &str) -> Result<Entry> {
        self.check_error()?;
        let archived = self.archived.read().unwrap();
        archived
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    fn update(&self, entry: &Entry) -> Result<()> {
        self.check_error()?;
        let mut entries = self.entries.write().unwrap();
        if !entries.contains_key(&entry.id) {
            return Err(StoreError::NotFound(entry.id.clone()));
        }
        entries.insert(entry.id.clone(), entry.clone());
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.check_error()?;
        let mut entries = self.entries.write().unwrap();
        entries
            .remove(id)
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        Ok(())
    }

    fn list(&self) -> Result<Vec<Entry>> {
        self.check_error()?;
        let entries = self.entries.read().unwrap();
        let mut list: Vec<Entry> = entries.values().cloned().collect();
        list.sort_by(|a, b| b.created.cmp(&a.created));
        Ok(list)
    }

    fn list_by_scope_and_tag(&self, scope: Scope, tag: &str) -> Result<Vec<Entry>> {
        self.check_error()?;
        let entries = self.entries.read().unwrap();
        let mut list: Vec<Entry> = entries
            .values()
            .filter(|entry| entry.scope == scope)
            .filter(|entry| {
                entry
                    .tags
                    .iter()
                    .any(|entry_tag| entry_tag.trim().eq_ignore_ascii_case(tag))
            })
            .cloned()
            .collect();
        list.sort_by(|a, b| b.created.cmp(&a.created));
        Ok(list)
    }

    fn recent(&self, n: usize) -> Result<Vec<Entry>> {
        self.check_error()?;
        let mut list = self.list()?;
        list.truncate(n);
        Ok(list)
    }

    fn archive(&self, id: &str) -> Result<()> {
        self.check_error()?;
        let mut entries = self.entries.write().unwrap();
        let mut archived = self.archived.write().unwrap();
        let entry = entries
            .remove(id)
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        archived.insert(id.to_string(), entry);
        Ok(())
    }

    fn unarchive(&self, id: &str) -> Result<()> {
        self.check_error()?;
        let mut entries = self.entries.write().unwrap();
        let mut archived = self.archived.write().unwrap();
        let entry = archived
            .remove(id)
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        entries.insert(id.to_string(), entry);
        Ok(())
    }

    fn list_archived(&self) -> Result<Vec<Entry>> {
        self.check_error()?;
        let archived = self.archived.read().unwrap();
        let mut list: Vec<Entry> = archived.values().cloned().collect();
        list.sort_by(|a, b| b.created.cmp(&a.created));
        Ok(list)
    }

    fn list_by_branch(&self, branch: &str) -> Result<Vec<Entry>> {
        self.check_error()?;
        let entries = self.entries.read().unwrap();
        let list: Vec<Entry> = entries
            .values()
            .filter(|e| e.branch.as_deref() == Some(branch))
            .cloned()
            .collect();
        Ok(list)
    }

    fn list_pending(&self, limit: usize) -> Result<Vec<Entry>> {
        self.check_error()?;
        let entries = self.entries.read().unwrap();
        let mut list: Vec<Entry> = entries
            .values()
            .filter(|entry| entry.pending_extraction)
            .cloned()
            .collect();
        list.sort_by(|a, b| a.created.cmp(&b.created));
        list.truncate(limit);
        Ok(list)
    }

    fn mark_extracted(&self, id: &str) -> Result<()> {
        self.check_error()?;
        let mut entries = self.entries.write().unwrap();
        if let Some(entry) = entries.get_mut(id) {
            entry.pending_extraction = false;
            Ok(())
        } else {
            Err(StoreError::NotFound(id.to_string()))
        }
    }

    fn list_pinned(&self) -> Result<Vec<Entry>> {
        self.check_error()?;
        let entries = self.entries.read().unwrap();
        let mut list: Vec<Entry> = entries
            .values()
            .filter(|entry| entry.memory_tier == MemoryTier::InContext)
            .cloned()
            .collect();
        list.sort_by(|a, b| b.created.cmp(&a.created));
        Ok(list)
    }

    fn list_helpful(&self, limit: usize) -> Result<Vec<Entry>> {
        self.check_error()?;
        let entries = self.entries.read().unwrap();
        let mut list: Vec<Entry> = entries
            .values()
            .filter(|entry| entry.helpful_count > entry.harmful_count)
            .cloned()
            .collect();
        list.sort_by(|a, b| {
            let score_a = a.helpful_count - a.harmful_count;
            let score_b = b.helpful_count - b.harmful_count;
            score_b.cmp(&score_a)
        });
        list.truncate(limit);
        Ok(list)
    }

    fn list_by_session(&self, session_id: &str) -> Result<Vec<Entry>> {
        self.check_error()?;
        let entries = self.entries.read().unwrap();
        let mut list: Vec<Entry> = entries
            .values()
            .filter(|entry| entry.session_id.as_deref() == Some(session_id))
            .cloned()
            .collect();
        list.sort_by(|a, b| a.created.cmp(&b.created));
        Ok(list)
    }

    fn list_unreviewed_learnings(&self, limit: usize) -> Result<Vec<Entry>> {
        self.check_error()?;
        let entries = self.entries.read().unwrap();
        let mut list: Vec<Entry> = entries
            .values()
            .filter(|entry| {
                entry.entry_type == EntryType::Learning && entry.last_reviewed.is_none()
            })
            .cloned()
            .collect();
        list.sort_by(|a, b| b.created.cmp(&a.created));
        list.truncate(limit);
        Ok(list)
    }

    fn mark_reviewed(&self, id: &str) -> Result<()> {
        self.check_error()?;
        let mut entries = self.entries.write().unwrap();
        if let Some(entry) = entries.get_mut(id) {
            entry.last_reviewed = Some(Utc::now());
            Ok(())
        } else {
            Err(StoreError::NotFound(id.to_string()))
        }
    }

    fn list_pending_index(&self, limit: usize) -> Result<Vec<Entry>> {
        self.check_error()?;
        let entries = self.entries.read().unwrap();
        let mut list: Vec<Entry> = entries
            .values()
            .filter(|entry| !entry.archived)
            .cloned()
            .collect();
        list.sort_by(|a, b| b.created.cmp(&a.created));
        list.truncate(limit);
        Ok(list)
    }

    fn mark_indexed(&self, id: &str) -> Result<()> {
        self.check_error()?;
        let entries = self.entries.read().unwrap();
        if entries.contains_key(id) {
            Ok(())
        } else {
            Err(StoreError::NotFound(id.to_string()))
        }
    }

    fn mark_indexed_batch(&self, ids: &[&str]) -> Result<()> {
        self.check_error()?;
        let entries = self.entries.read().unwrap();
        for id in ids {
            if !entries.contains_key(*id) {
                return Err(StoreError::NotFound((*id).to_string()));
            }
        }
        Ok(())
    }

    fn mark_index_pending_batch(&self, ids: &[&str]) -> Result<()> {
        self.mark_indexed_batch(ids)
    }

    fn cas_dir(&self) -> &Path {
        &self.cas_dir
    }

    fn close(&self) -> Result<()> {
        self.check_error()
    }
}
