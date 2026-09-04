//! Syncing entry store wrapper
//!
//! Automatically queues entry changes for cloud sync on add/update/delete.
//! When a team is configured and the entry passes the T1 filter policy
//! (see `share_policy::eligible_for_team_entry`), the write is dual-enqueued
//! to both the personal queue and the team queue so the next
//! `cas cloud sync` drains both.

use std::path::Path;
use std::sync::Arc;

use crate::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
use crate::store::share_policy::{eligible_for_team_entry, resolve_team_id};
use crate::store::{Result, Store};
use crate::types::{Entry, Scope};

/// An entry store wrapper that queues changes for cloud sync
pub struct SyncingEntryStore {
    inner: Arc<dyn Store>,
    queue: Arc<SyncQueue>,
    /// Pre-resolved team UUID for dual-enqueue. Cached at
    /// `with_cloud_config` time rather than resolved per-write so the
    /// hot write path does zero heap allocations to read it. `None`
    /// preserves the historical personal-only behavior — used by tests
    /// and callers that predate the team-memories work.
    ///
    /// Known tradeoff: CloudConfig is snapshotted at store-open; an
    /// on-disk `cas cloud team set|clear` in another process only
    /// takes effect when this store is reconstructed. Documented in
    /// docs/requests/team-memories-filter-policy.md Decision 1.
    team_id: Option<Arc<str>>,
}

impl SyncingEntryStore {
    /// Create a new syncing entry store with personal-only sync.
    pub fn new(inner: Arc<dyn Store>, queue: Arc<SyncQueue>) -> Self {
        Self {
            inner,
            queue,
            team_id: None,
        }
    }

    /// Attach a cloud config so this store can auto-dual-enqueue eligible
    /// writes to the team queue when a team is configured. No-op when
    /// the config has no `active_team_id()`. Builder-style to preserve
    /// existing 2-arg `new` call sites.
    #[must_use]
    pub fn with_cloud_config(mut self, cloud_config: Arc<CloudConfig>) -> Self {
        self.team_id = resolve_team_id(&cloud_config);
        self
    }

    fn queue_upsert(&self, entry: &Entry) {
        // Serialise once — both enqueues share the same payload.
        let payload = match serde_json::to_string(entry) {
            Ok(p) => p,
            Err(_) => return, // Best-effort: skip on serialisation failure.
        };

        // Personal enqueue is best-effort (historical behaviour).
        let _ = self.queue.enqueue(
            EntityType::Entry,
            &entry.id,
            SyncOperation::Upsert,
            Some(&payload),
        );

        // Team enqueue: opt-in, predicate-gated. Best-effort (let _ = ...)
        // matches the personal path's historical contract — a SQLite
        // failure here silently drops the team row, same as it silently
        // drops the personal row. Symmetric. Propagating the error would
        // require changing the Store trait signature; out of scope.
        if let Some(team_id) = self.team_id.as_deref()
            && eligible_for_team_entry(entry)
        {
            let _ = self.queue.enqueue_for_team(
                EntityType::Entry,
                &entry.id,
                SyncOperation::Upsert,
                Some(&payload),
                team_id,
            );
        }
    }

    fn queue_delete(&self, id: &str) {
        let _ = self
            .queue
            .enqueue(EntityType::Entry, id, SyncOperation::Delete, None);

        // See `share_policy` module docs: delete fans out unconditionally
        // when a team is configured (predicate can't run without entity).
        if let Some(team_id) = self.team_id.as_deref() {
            let _ = self.queue.enqueue_for_team(
                EntityType::Entry,
                id,
                SyncOperation::Delete,
                None,
                team_id,
            );
        }
    }
}

/// SessionStart reinforces surfaced memories in place. Those updates change
/// only local access telemetry and should not become cloud content writes (or
/// make a destructive purge look like it has unsent work). Keep every other
/// field in the comparison so a real content edit is still queued.
fn is_access_metadata_only_update(before: &Entry, after: &Entry) -> bool {
    let (Ok(mut before), Ok(mut after)) = (
        serde_json::to_value(before),
        serde_json::to_value(after),
    ) else {
        return false;
    };
    for key in ["last_accessed", "access_count", "stability"] {
        if let Some(object) = before.as_object_mut() {
            object.remove(key);
        }
        if let Some(object) = after.as_object_mut() {
            object.remove(key);
        }
    }
    before == after
}

impl Store for SyncingEntryStore {
    fn init(&self) -> Result<()> {
        self.inner.init()
    }

    fn generate_id(&self) -> Result<String> {
        self.inner.generate_id()
    }

    fn add(&self, entry: &Entry) -> Result<()> {
        self.inner.add(entry)?;
        self.queue_upsert(entry);
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Entry> {
        self.inner.get(id)
    }

    fn get_archived(&self, id: &str) -> Result<Entry> {
        self.inner.get_archived(id)
    }

    fn update(&self, entry: &Entry) -> Result<()> {
        let access_metadata_only = self
            .inner
            .get(&entry.id)
            .is_ok_and(|before| is_access_metadata_only_update(&before, entry));
        self.inner.update(entry)?;
        if !access_metadata_only {
            self.queue_upsert(entry);
        }
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.inner.delete(id)?;
        self.queue_delete(id);
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
        self.inner.archive(id)?;
        // Queue the archived entry state
        if let Ok(entry) = self.inner.get_archived(id) {
            self.queue_upsert(&entry);
        }
        Ok(())
    }

    fn unarchive(&self, id: &str) -> Result<()> {
        self.inner.unarchive(id)?;
        // Queue the unarchived entry state
        if let Ok(entry) = self.inner.get(id) {
            self.queue_upsert(&entry);
        }
        Ok(())
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
        self.inner.mark_extracted(id)?;
        // Queue the updated entry
        if let Ok(entry) = self.inner.get(id) {
            self.queue_upsert(&entry);
        }
        Ok(())
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
        self.inner.mark_reviewed(id)?;
        // Queue the updated entry for sync
        if let Ok(entry) = self.inner.get(id) {
            self.queue_upsert(&entry);
        }
        Ok(())
    }

    fn list_pending_index(&self, limit: usize) -> Result<Vec<Entry>> {
        self.inner.list_pending_index(limit)
    }

    fn mark_indexed(&self, id: &str) -> Result<()> {
        self.inner.mark_indexed(id)
        // Note: We don't queue for sync on mark_indexed as it's a local-only flag
    }

    fn mark_indexed_batch(&self, ids: &[&str]) -> Result<()> {
        self.inner.mark_indexed_batch(ids)
        // Note: We don't queue for sync on mark_indexed as it's a local-only flag
    }

    fn mark_index_pending_batch(&self, ids: &[&str]) -> Result<()> {
        self.inner.mark_index_pending_batch(ids)
        // Index state is local-only and must not enter the cloud sync queue.
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
    use crate::store::syncing_entry::*;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, SyncingEntryStore) {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path();

        let inner = SqliteStore::open(cas_dir).unwrap();
        inner.init().unwrap();

        let queue = SyncQueue::open(cas_dir).unwrap();
        queue.init().unwrap();

        let store = SyncingEntryStore::new(Arc::new(inner), Arc::new(queue));
        (temp, store)
    }

    #[test]
    fn test_add_queues_sync() {
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();

        let entry = Entry::new("entry-001".to_string(), "Test content".to_string());
        store.add(&entry).unwrap();

        let pending = queue.pending(10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].entity_type, EntityType::Entry);
        assert_eq!(pending[0].entity_id, entry.id);
        assert_eq!(pending[0].operation, SyncOperation::Upsert);
    }

    #[test]
    fn test_update_queues_sync() {
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut entry = Entry::new("entry-002".to_string(), "Test content".to_string());
        store.add(&entry).unwrap();

        // Clear queue
        queue.clear().unwrap();

        entry.content = "Updated content".to_string();
        store.update(&entry).unwrap();

        let pending = queue.pending(10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert!(
            pending[0]
                .payload
                .as_ref()
                .unwrap()
                .contains("Updated content")
        );
    }

    #[test]
    fn access_metadata_only_update_does_not_queue_sync() {
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();

        let entry = Entry::new("entry-access-refresh".to_string(), "Test content".to_string());
        store.add(&entry).unwrap();
        queue.clear().unwrap();

        let mut surfaced = store.get(&entry.id).unwrap();
        surfaced.reinforce();
        store.update(&surfaced).unwrap();

        assert!(
            queue.pending(10, 5).unwrap().is_empty(),
            "SessionStart access reinforcement is local telemetry, not a content push"
        );
        assert_eq!(store.get(&entry.id).unwrap().access_count, 1);
    }

    #[test]
    fn test_delete_queues_sync() {
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();

        let entry = Entry::new("entry-003".to_string(), "Test content".to_string());
        store.add(&entry).unwrap();

        // Clear queue
        queue.clear().unwrap();

        store.delete(&entry.id).unwrap();

        let pending = queue.pending(10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].operation, SyncOperation::Delete);
    }

    // ── Dual-enqueue behaviour (cas-82a1) ────────────────────────────────

    use cas_types::{EntryType, Scope, ShareScope};

    use crate::store::share_policy::TEST_TEAM_UUID as TEST_TEAM;

    /// Build a SyncingEntryStore with a team configured via CloudConfig.
    fn create_team_store(team_auto_promote: Option<bool>) -> (TempDir, SyncingEntryStore) {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path();

        let inner = SqliteStore::open(cas_dir).unwrap();
        inner.init().unwrap();

        let queue = SyncQueue::open(cas_dir).unwrap();
        queue.init().unwrap();

        let mut cfg = CloudConfig::default();
        cfg.set_team(TEST_TEAM, "test-team");
        cfg.team_auto_promote = team_auto_promote;

        let store = SyncingEntryStore::new(Arc::new(inner), Arc::new(queue))
            .with_cloud_config(Arc::new(cfg));
        (temp, store)
    }

    /// Convenience: count rows in personal queue (team_id = '') and in
    /// the given team's queue.
    fn queue_counts(queue: &SyncQueue) -> (usize, usize) {
        let personal = queue.pending(100, 5).unwrap().len();
        let team = queue.pending_for_team(TEST_TEAM, 100, 5).unwrap().len();
        (personal, team)
    }

    #[test]
    fn dual_enqueue_when_team_configured_and_entry_eligible() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        // Default Entry is Project scope, Learning type, no share override —
        // passes T1 filter.
        let entry = Entry::new("p-test-001".to_string(), "team-visible".to_string());
        store.add(&entry).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1, "personal queue should have the entry");
        assert_eq!(team, 1, "team queue should have the entry (dual-enqueue)");
    }

    #[test]
    fn personal_only_when_no_cloud_config_attached() {
        // No with_cloud_config call — historical behavior preserved.
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();

        let entry = Entry::new("p-test-002".to_string(), "no-team".to_string());
        store.add(&entry).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 0, "no team configured → no team enqueue");
    }

    #[test]
    fn personal_only_when_team_id_unset() {
        let (temp, store) = {
            let temp = TempDir::new().unwrap();
            let cas_dir = temp.path();
            let inner = SqliteStore::open(cas_dir).unwrap();
            inner.init().unwrap();
            let queue = SyncQueue::open(cas_dir).unwrap();
            queue.init().unwrap();
            // CloudConfig with no team set.
            let cfg = CloudConfig::default();
            let store = SyncingEntryStore::new(Arc::new(inner), Arc::new(queue))
                .with_cloud_config(Arc::new(cfg));
            (temp, store)
        };
        let queue = SyncQueue::open(temp.path()).unwrap();

        let entry = Entry::new("p-test-003".to_string(), "no-team-id".to_string());
        store.add(&entry).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 0);
    }

    #[test]
    fn personal_only_when_entry_is_preference_type() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut entry = Entry::new("p-pref-001".to_string(), "I use vim".to_string());
        entry.entry_type = EntryType::Preference;
        store.add(&entry).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 0, "Preference type never auto-promotes");
    }

    #[test]
    fn personal_only_when_entry_is_global_scope() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut entry = Entry::new("g-test-001".to_string(), "global learning".to_string());
        entry.scope = Scope::Global;
        store.add(&entry).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 0, "Global scope never auto-promotes");
    }

    #[test]
    fn personal_only_when_share_is_private_override() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut entry = Entry::new("p-priv-001".to_string(), "I tried X".to_string());
        entry.share = Some(ShareScope::Private);
        store.add(&entry).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 0, "share=Private overrides auto-rule");
    }

    #[test]
    fn dual_enqueue_when_share_is_team_override_even_for_global_preference() {
        // The most extreme override: Global Preference with share=Team still
        // dual-enqueues per precedence table row (though server pull-filter
        // will strip Preference; user warned at T5 CLI time).
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut entry = Entry::new("g-pref-001".to_string(), "team-wide pref".to_string());
        entry.scope = Scope::Global;
        entry.entry_type = EntryType::Preference;
        entry.share = Some(ShareScope::Team);
        store.add(&entry).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(
            team, 1,
            "share=Team forces promotion even for Global+Preference"
        );
    }

    #[test]
    fn personal_only_when_team_auto_promote_disabled() {
        // team_id is set but the coarse kill-switch is engaged — the
        // syncing store sees active_team_id() == None and does NOT
        // dual-enqueue anything.
        let (temp, store) = create_team_store(Some(false));
        let queue = SyncQueue::open(temp.path()).unwrap();

        let entry = Entry::new("p-kill-001".to_string(), "should stay local".to_string());
        store.add(&entry).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(
            team, 0,
            "team_auto_promote=false disables dual-enqueue even with team_id set"
        );
    }

    #[test]
    fn delete_dual_enqueues_when_team_configured() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let entry = Entry::new("p-del-001".to_string(), "to-be-deleted".to_string());
        store.add(&entry).unwrap();
        // Clear so we can count the delete alone.
        queue.clear().unwrap();

        store.delete(&entry.id).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 1, "delete fans out to team queue too");
    }

    #[test]
    fn delete_personal_only_when_kill_switch_engaged() {
        // Regression guard for the kill-switch: team_auto_promote=false
        // must silence both upsert AND delete team enqueues. A future
        // refactor that drops active_team_id() from queue_delete would
        // leak deletes to the team queue when the user has opted out.
        let (temp, store) = create_team_store(Some(false));
        let queue = SyncQueue::open(temp.path()).unwrap();

        let entry = Entry::new("p-del-002".to_string(), "kill-switched delete".to_string());
        store.add(&entry).unwrap();
        queue.clear().unwrap();

        store.delete(&entry.id).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 0, "kill-switch silences delete fan-out");
    }
}
