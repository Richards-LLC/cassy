//! Syncing rule store wrapper
//!
//! Automatically syncs rules to Claude Code on add/update/delete,
//! and optionally queues changes for cloud sync. When a team is configured
//! and the rule passes the T1 filter policy, the write is dual-enqueued
//! to both the personal queue and the team queue.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use crate::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
use crate::store::foreign_project_guard::ForeignProjectGuard;
use crate::store::share_policy::{eligible_for_team_rule, resolve_team_id};
use crate::store::{Result, RuleStore};
use crate::types::Rule;
use cas_store::RuleVersion;
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
    /// Project root for the foreign-project sync guard (cas-caae). The guard
    /// reads the host registry, so it is built on the first rule sync rather
    /// than on every store open.
    project_root: Option<PathBuf>,
    project_guard: OnceLock<ForeignProjectGuard>,
}

impl SyncingRuleStore {
    /// Create a new syncing rule store (local sync only)
    pub fn new(inner: Arc<dyn RuleStore>, target_dir: PathBuf, min_helpful: i32) -> Self {
        Self {
            inner,
            syncer: Syncer::new(target_dir, min_helpful),
            cloud_queue: None,
            team_id: None,
            project_root: None,
            project_guard: OnceLock::new(),
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
            project_root: None,
            project_guard: OnceLock::new(),
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

    /// Enable the foreign-project sync guard for the project at
    /// `project_root` (cas-caae): a rule naming another registered project is
    /// kept out of (and removed from) this project's rule files.
    #[must_use]
    pub fn with_project_root(mut self, project_root: PathBuf) -> Self {
        self.project_root = Some(project_root);
        self
    }

    /// Test seam: install a guard built from an explicit registry.
    #[cfg(test)]
    fn with_project_guard(mut self, guard: ForeignProjectGuard) -> Self {
        self.project_root = Some(PathBuf::new());
        let _ = self.project_guard.set(guard);
        self
    }

    fn names_foreign_project(&self, rule: &Rule) -> bool {
        let Some(project_root) = self.project_root.as_deref() else {
            return false;
        };
        self.project_guard
            .get_or_init(|| ForeignProjectGuard::for_project_root(project_root))
            .check_rule(&rule.content, &rule.tags)
            .is_some()
    }

    fn try_sync(&self, rule: &Rule) {
        if self.names_foreign_project(rule) {
            self.try_remove(&rule.id);
            return;
        }
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

    fn update_with_metadata(
        &self,
        rule: &Rule,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        self.inner
            .update_with_metadata(rule, changed_by, change_note)?;
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

    fn delete_with_metadata(
        &self,
        id: &str,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        self.inner.delete_with_metadata(id, changed_by, change_note)?;
        self.try_remove(id);
        self.queue_delete(id);
        Ok(())
    }

    fn list_versions(&self, id: &str) -> Result<Vec<RuleVersion>> {
        self.inner.list_versions(id)
    }

    fn restore_version(
        &self,
        id: &str,
        version: Option<i64>,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        self.inner
            .restore_version(id, version, changed_by, change_note)?;
        let restored = self.inner.get(id)?;
        self.try_sync(&restored);
        self.queue_upsert(&restored);
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

    /// cas-caae (skills audit M27): a proven rule naming another registered
    /// project is never written to this project's rule files, and an
    /// existing file for it is removed on its next write.
    #[test]
    fn foreign_project_rule_is_not_synced_to_rule_files() {
        let temp = TempDir::new().unwrap();
        let inner = SqliteRuleStore::open(temp.path()).unwrap();
        inner.init().unwrap();
        let target = temp.path().join("rules");
        let store = SyncingRuleStore::new(Arc::new(inner), target.clone(), 0).with_project_guard(
            ForeignProjectGuard::new("cas-src", ["gabber-studio".to_string()]),
        );
        let proven = |id: &str, content: &str| {
            let mut rule = make_rule(id, Scope::Project);
            rule.content = content.to_string();
            rule.status = cas_types::RuleStatus::Proven;
            rule.helpful_count = 1;
            rule
        };

        store
            .add(&proven("rule-001", "Epics branch from main."))
            .unwrap();
        assert!(target.join("rule-001.md").exists());

        let gabber = proven(
            "rule-002",
            "Gabber Studio branching: ALWAYS cut new branches from `staging`.",
        );
        store.add(&gabber).unwrap();
        assert!(!target.join("rule-002.md").exists());

        // A file synced before the guard existed is removed on the next write.
        std::fs::write(target.join("rule-002.md"), "stale").unwrap();
        store.update(&gabber).unwrap();
        assert!(!target.join("rule-002.md").exists());

        // Tagging it `project:gabber-studio` declares the scope explicitly.
        let mut scoped = gabber.clone();
        scoped.tags = vec!["project:gabber-studio".to_string()];
        store.update(&scoped).unwrap();
        assert!(target.join("rule-002.md").exists());
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
