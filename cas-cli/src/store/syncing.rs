//! Syncing rule store wrapper
//!
//! Automatically syncs rules to Claude Code on add/update/delete,
//! and optionally queues changes for cloud sync. When a team is configured
//! and the rule passes the T1 filter policy, the write is dual-enqueued
//! to both the personal queue and the team queue.

use std::path::PathBuf;
use std::sync::Arc;

use crate::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
use crate::store::share_policy::{eligible_for_team_rule, resolve_team_id};
use crate::store::{Result, RuleStore};
use crate::types::Rule;
use cas_core::Syncer;

/// A rule store wrapper that syncs rules to Claude Code and cloud
pub struct SyncingRuleStore {
    inner: Arc<dyn RuleStore>,
    syncer: Syncer,
    /// Optional cloud sync queue
    cloud_queue: Option<Arc<SyncQueue>>,
    /// Pre-resolved team UUID for dual-enqueue. Populated only when
    /// BOTH `with_cloud_queue` and `with_cloud_config` were called —
    /// a config without a queue has nowhere to dual-enqueue, so the
    /// builder silently drops it (see `with_cloud_config` doc).
    team_id: Option<Arc<str>>,
}

impl SyncingRuleStore {
    /// Create a new syncing rule store (local sync only)
    pub fn new(inner: Arc<dyn RuleStore>, target_dir: PathBuf, min_helpful: i32) -> Self {
        Self {
            inner,
            syncer: Syncer::new(target_dir, min_helpful),
            cloud_queue: None,
            team_id: None,
        }
    }

    /// Create a new syncing rule store with cloud sync
    pub fn with_cloud_queue(
        inner: Arc<dyn RuleStore>,
        target_dir: PathBuf,
        min_helpful: i32,
        cloud_queue: Arc<SyncQueue>,
    ) -> Self {
        Self {
            inner,
            syncer: Syncer::new(target_dir, min_helpful),
            cloud_queue: Some(cloud_queue),
            team_id: None,
        }
    }

    /// Attach a cloud config for team auto-promotion. Meaningful only
    /// when `with_cloud_queue` also provided a queue — without a queue
    /// the resolved team_id is harmless (queue_upsert/delete early-return
    /// on `cloud_queue.is_none()`). The `debug_assert` catches the misuse
    /// in dev builds; release trusts the caller and proceeds, which is
    /// safe because the downstream guards make it a no-op anyway.
    #[must_use]
    pub fn with_cloud_config(mut self, cloud_config: Arc<CloudConfig>) -> Self {
        debug_assert!(
            self.cloud_queue.is_some(),
            "SyncingRuleStore::with_cloud_config called without with_cloud_queue — team dual-enqueue will silently no-op"
        );
        self.team_id = resolve_team_id(&cloud_config);
        self
    }

    fn try_sync(&self, rule: &Rule) {
        // Ignore sync errors - syncing is best-effort
        let _ = self.syncer.sync_rule(rule);
    }

    fn try_remove(&self, rule_id: &str) {
        let _ = self.syncer.remove_rule(rule_id);
    }

    fn queue_upsert(&self, rule: &Rule) {
        let Some(queue) = &self.cloud_queue else {
            return;
        };
        let payload = match serde_json::to_string(rule) {
            Ok(p) => p,
            Err(_) => return,
        };

        let _ = queue.enqueue(
            EntityType::Rule,
            &rule.id,
            SyncOperation::Upsert,
            Some(&payload),
        );

        if let Some(team_id) = self.team_id.as_deref()
            && eligible_for_team_rule(rule)
        {
            let _ = queue.enqueue_for_team(
                EntityType::Rule,
                &rule.id,
                SyncOperation::Upsert,
                Some(&payload),
                team_id,
            );
        }
    }

    fn queue_delete(&self, id: &str) {
        let Some(queue) = &self.cloud_queue else {
            return;
        };
        let _ = queue.enqueue(EntityType::Rule, id, SyncOperation::Delete, None);

        // See `share_policy` module docs: delete fans out unconditionally
        // when a team is configured.
        if let Some(team_id) = self.team_id.as_deref() {
            let _ = queue.enqueue_for_team(
                EntityType::Rule,
                id,
                SyncOperation::Delete,
                None,
                team_id,
            );
        }
    }
}

impl RuleStore for SyncingRuleStore {
    fn init(&self) -> Result<()> {
        self.inner.init()
    }

    fn generate_id(&self) -> Result<String> {
        self.inner.generate_id()
    }

    fn add(&self, rule: &Rule) -> Result<()> {
        self.inner.add(rule)?;
        self.try_sync(rule);
        self.queue_upsert(rule);
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Rule> {
        self.inner.get(id)
    }

    fn update(&self, rule: &Rule) -> Result<()> {
        self.inner.update(rule)?;
        self.try_sync(rule);
        self.queue_upsert(rule);
        Ok(())
    }

    fn increment_surface_count(&self, id: &str) -> Result<()> {
        self.inner.increment_surface_count(id)
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.inner.delete(id)?;
        self.try_remove(id);
        self.queue_delete(id);
        Ok(())
    }

    fn list(&self) -> Result<Vec<Rule>> {
        self.inner.list()
    }

    fn list_proven(&self) -> Result<Vec<Rule>> {
        self.inner.list_proven()
    }

    fn list_critical(&self) -> Result<Vec<Rule>> {
        self.inner.list_critical()
    }

    fn close(&self) -> Result<()> {
        self.inner.close()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SqliteRuleStore;
    use crate::store::share_policy::TEST_TEAM_UUID as TEST_TEAM;
    use cas_types::Scope;
    use tempfile::TempDir;

    fn create_team_store(team_auto_promote: Option<bool>) -> (TempDir, SyncingRuleStore) {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path();
        let inner = SqliteRuleStore::open(cas_dir).unwrap();
        inner.init().unwrap();
        let queue = SyncQueue::open(cas_dir).unwrap();
        queue.init().unwrap();
        let mut cfg = CloudConfig::default();
        cfg.set_team(TEST_TEAM, "test-team");
        cfg.team_auto_promote = team_auto_promote;
        let store = SyncingRuleStore::with_cloud_queue(
            Arc::new(inner),
            temp.path().join("rules"),
            0,
            Arc::new(queue),
        )
        .with_cloud_config(Arc::new(cfg));
        (temp, store)
    }

    fn make_rule(id: &str, scope: Scope) -> Rule {
        let mut r = Rule::default();
        r.id = id.to_string();
        r.scope = scope;
        r.content = format!("rule {id}");
        r
    }

    fn queue_counts(queue: &SyncQueue) -> (usize, usize) {
        let personal = queue.pending(100, 5).unwrap().len();
        let team = queue.pending_for_team(TEST_TEAM, 100, 5).unwrap().len();
        (personal, team)
    }

    #[test]
    fn rule_dual_enqueue_when_team_configured_and_project_scope() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let rule = make_rule("p-rule-001", Scope::Project);
        store.add(&rule).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 1);
    }

    #[test]
    fn rule_personal_only_when_global_scope() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let rule = make_rule("g-rule-001", Scope::Global);
        store.add(&rule).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 0);
    }

    #[test]
    fn rule_personal_only_when_kill_switch_engaged() {
        let (temp, store) = create_team_store(Some(false));
        let queue = SyncQueue::open(temp.path()).unwrap();

        let rule = make_rule("p-rule-002", Scope::Project);
        store.add(&rule).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 0);
    }

    #[test]
    fn rule_delete_dual_enqueues_when_team_configured() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let rule = make_rule("p-rule-003", Scope::Project);
        store.add(&rule).unwrap();
        queue.clear().unwrap();

        store.delete(&rule.id).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 1);
    }

    /// Verify the `with_cloud_config` no-queue guard: calling it after a
    /// local-only `new(...)` should silently produce a store that does
    /// not team-enqueue. The debug_assert fires in debug builds but we
    /// compile tests in debug, so this test also locks in the panic
    /// message (caught via catch_unwind) as a regression guard.
    #[test]
    #[cfg(debug_assertions)]
    fn with_cloud_config_without_queue_debug_asserts() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        let temp = TempDir::new().unwrap();
        let inner = SqliteRuleStore::open(temp.path()).unwrap();
        inner.init().unwrap();
        let base = SyncingRuleStore::new(Arc::new(inner), temp.path().join("rules"), 0);
        let cfg = Arc::new(CloudConfig::default());

        let result = catch_unwind(AssertUnwindSafe(|| base.with_cloud_config(cfg)));
        assert!(
            result.is_err(),
            "expected debug_assert panic when with_cloud_config is called without with_cloud_queue"
        );
    }
}
