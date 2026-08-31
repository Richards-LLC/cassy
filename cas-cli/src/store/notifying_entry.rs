//! Notifying entry store wrapper
//!
//! Emits notification events on entry add/update/delete for TUI display.

use std::path::Path;
use std::sync::Arc;

use crate::config::NotificationConfig;
use crate::notifications::{NotificationEvent, get_global_notifier};
use crate::store::{Result, Store};
use crate::types::{Entry, Scope};

/// An entry store wrapper that emits notification events
pub struct NotifyingEntryStore {
    inner: Arc<dyn Store>,
    config: NotificationConfig,
}

impl NotifyingEntryStore {
    /// Create a new notifying entry store
    pub fn new(inner: Arc<dyn Store>, config: NotificationConfig) -> Self {
        Self { inner, config }
    }

    fn notify_added(&self, entry: &Entry) {
        if self.config.enabled && self.config.entries.on_added {
            if let Some(notifier) = get_global_notifier() {
                notifier.notify(NotificationEvent::entry_added(
                    &entry.id,
                    &entry.entry_type.to_string(),
                ));
            }
        }
    }

    fn notify_updated(&self, entry: &Entry) {
        if self.config.enabled && self.config.entries.on_updated {
            if let Some(notifier) = get_global_notifier() {
                notifier.notify(NotificationEvent::entry_updated(&entry.id));
            }
        }
    }

    fn notify_deleted(&self, id: &str) {
        if self.config.enabled && self.config.entries.on_deleted {
            if let Some(notifier) = get_global_notifier() {
                notifier.notify(NotificationEvent::entry_deleted(id));
            }
        }
    }
}

impl Store for NotifyingEntryStore {
    fn init(&self) -> Result<()> {
        self.inner.init()
    }

    fn generate_id(&self) -> Result<String> {
        self.inner.generate_id()
    }

    fn add(&self, entry: &Entry) -> Result<()> {
        self.inner.add(entry)?;
        self.notify_added(entry);
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Entry> {
        self.inner.get(id)
    }

    fn get_archived(&self, id: &str) -> Result<Entry> {
        self.inner.get_archived(id)
    }

    fn update(&self, entry: &Entry) -> Result<()> {
        self.inner.update(entry)?;
        self.notify_updated(entry);
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.inner.delete(id)?;
        self.notify_deleted(id);
        Ok(())
    }

    fn list(&self) -> Result<Vec<Entry>> {
        self.inner.list()
    }

    fn list_by_scope_and_tag(&self, scope: Scope, tag: &str) -> Result<Vec<Entry>> {
        self.inner.list_by_scope_and_tag(scope, tag)
    }

    fn recent(&self, n: usize) -> Result<Vec<Entry>> {
        self.inner.recent(n)
    }

    fn archive(&self, id: &str) -> Result<()> {
        self.inner.archive(id)
    }

    fn unarchive(&self, id: &str) -> Result<()> {
        self.inner.unarchive(id)
    }

    fn list_archived(&self) -> Result<Vec<Entry>> {
        self.inner.list_archived()
    }

    fn list_by_branch(&self, branch: &str) -> Result<Vec<Entry>> {
        self.inner.list_by_branch(branch)
    }

    fn list_pending(&self, limit: usize) -> Result<Vec<Entry>> {
        self.inner.list_pending(limit)
    }

    fn mark_extracted(&self, id: &str) -> Result<()> {
        self.inner.mark_extracted(id)
    }

    fn list_pinned(&self) -> Result<Vec<Entry>> {
        self.inner.list_pinned()
    }

    fn list_helpful(&self, limit: usize) -> Result<Vec<Entry>> {
        self.inner.list_helpful(limit)
    }

    fn list_by_session(&self, session_id: &str) -> Result<Vec<Entry>> {
        self.inner.list_by_session(session_id)
    }

    fn list_unreviewed_learnings(&self, limit: usize) -> Result<Vec<Entry>> {
        self.inner.list_unreviewed_learnings(limit)
    }

    fn mark_reviewed(&self, id: &str) -> Result<()> {
        self.inner.mark_reviewed(id)
    }

    fn list_pending_index(&self, limit: usize) -> Result<Vec<Entry>> {
        self.inner.list_pending_index(limit)
    }

    fn mark_indexed(&self, id: &str) -> Result<()> {
        self.inner.mark_indexed(id)
    }

    fn mark_indexed_batch(&self, ids: &[&str]) -> Result<()> {
        self.inner.mark_indexed_batch(ids)
    }

    fn mark_index_pending_batch(&self, ids: &[&str]) -> Result<()> {
        self.inner.mark_index_pending_batch(ids)
    }

    fn cas_dir(&self) -> &Path {
        self.inner.cas_dir()
    }

    fn close(&self) -> Result<()> {
        self.inner.close()
    }
}

#[cfg(test)]
mod tests {
    use crate::store::SqliteStore;
    use crate::store::notifying_entry::*;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, NotifyingEntryStore) {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path();

        let inner = SqliteStore::open(cas_dir).unwrap();
        inner.init().unwrap();

        let config = NotificationConfig::default();
        let store = NotifyingEntryStore::new(Arc::new(inner), config);
        (temp, store)
    }

    #[test]
    fn test_store_operations_work() {
        let (_temp, store) = create_test_store();

        // Test add
        let entry = Entry::new("entry-001".to_string(), "Test content".to_string());
        store.add(&entry).unwrap();

        // Test get
        let fetched = store.get("entry-001").unwrap();
        assert_eq!(fetched.content, "Test content");

        // Test update
        let mut updated_entry = entry.clone();
        updated_entry.content = "Updated content".to_string();
        store.update(&updated_entry).unwrap();

        let fetched = store.get("entry-001").unwrap();
        assert_eq!(fetched.content, "Updated content");

        // Test delete
        store.delete("entry-001").unwrap();
        assert!(store.get("entry-001").is_err());
    }
}
