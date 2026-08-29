//! Syncing skill store wrapper
//!
//! Automatically queues skill changes for cloud sync on add/update/delete.
//! When a team is configured and the skill passes the T1 filter policy,
//! the write is dual-enqueued to both the personal queue and the team queue.

use std::sync::Arc;

use crate::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
use crate::store::share_policy::{eligible_for_team_skill, resolve_team_id};
use crate::store::{Result, SkillStore};
use crate::types::{Skill, SkillStatus};
use cas_store::SkillVersion;

/// A skill store wrapper that queues changes for cloud sync
pub struct SyncingSkillStore {
    inner: Arc<dyn SkillStore>,
    queue: Arc<SyncQueue>,
    /// Pre-resolved team UUID for dual-enqueue; see
    /// `SyncingEntryStore::team_id` for the protocol. `None` preserves
    /// personal-only behaviour.
    team_id: Option<Arc<str>>,
}

impl SyncingSkillStore {
    /// Create a new syncing skill store (personal queue only).
    pub fn new(inner: Arc<dyn SkillStore>, queue: Arc<SyncQueue>) -> Self {
        Self {
            inner,
            queue,
            team_id: None,
        }
    }

    /// Attach a cloud config for team auto-promotion.
    #[must_use]
    pub fn with_cloud_config(mut self, cloud_config: Arc<CloudConfig>) -> Self {
        self.team_id = resolve_team_id(&cloud_config);
        self
    }

    fn queue_upsert(&self, skill: &Skill) {
        let payload = match serde_json::to_string(skill) {
            Ok(p) => p,
            Err(_) => return,
        };

        let _ = self.queue.enqueue(
            EntityType::Skill,
            &skill.id,
            SyncOperation::Upsert,
            Some(&payload),
        );

        if let Some(team_id) = self.team_id.as_deref()
            && eligible_for_team_skill(skill)
        {
            let _ = self.queue.enqueue_for_team(
                EntityType::Skill,
                &skill.id,
                SyncOperation::Upsert,
                Some(&payload),
                team_id,
            );
        }
    }

    fn queue_delete(&self, id: &str) {
        let _ = self
            .queue
            .enqueue(EntityType::Skill, id, SyncOperation::Delete, None);

        // See `share_policy` module docs: delete fans out unconditionally
        // when a team is configured.
        if let Some(team_id) = self.team_id.as_deref() {
            let _ = self.queue.enqueue_for_team(
                EntityType::Skill,
                id,
                SyncOperation::Delete,
                None,
                team_id,
            );
        }
    }
}

impl SkillStore for SyncingSkillStore {
    fn init(&self) -> Result<()> {
        self.inner.init()
    }

    fn generate_id(&self) -> Result<String> {
        self.inner.generate_id()
    }

    fn add(&self, skill: &Skill) -> Result<()> {
        self.inner.add(skill)?;
        self.queue_upsert(skill);
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Skill> {
        self.inner.get(id)
    }

    fn update(&self, skill: &Skill) -> Result<()> {
        self.inner.update(skill)?;
        self.queue_upsert(skill);
        Ok(())
    }

    fn update_with_metadata(
        &self,
        skill: &Skill,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        self.inner
            .update_with_metadata(skill, changed_by, change_note)?;
        self.queue_upsert(skill);
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.inner.delete(id)?;
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
        self.queue_delete(id);
        Ok(())
    }

    fn list_versions(&self, id: &str) -> Result<Vec<SkillVersion>> {
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
        self.queue_upsert(&restored);
        Ok(())
    }

    fn list(&self, status: Option<SkillStatus>) -> Result<Vec<Skill>> {
        self.inner.list(status)
    }

    fn list_enabled(&self) -> Result<Vec<Skill>> {
        self.inner.list_enabled()
    }

    fn search(&self, query: &str) -> Result<Vec<Skill>> {
        self.inner.search(query)
    }

    fn close(&self) -> Result<()> {
        self.inner.close()
    }
}

#[cfg(test)]
mod tests {
    use crate::store::SqliteSkillStore;
    use crate::store::syncing_skill::*;
    use crate::types::SkillType;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, SyncingSkillStore) {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path();

        let inner = SqliteSkillStore::open(cas_dir).unwrap();
        inner.init().unwrap();

        let queue = SyncQueue::open(cas_dir).unwrap();
        queue.init().unwrap();

        let store = SyncingSkillStore::new(Arc::new(inner), Arc::new(queue));
        (temp, store)
    }

    #[test]
    fn test_add_queues_sync() {
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut skill = Skill::new("test-skill".to_string(), "Test Skill".to_string());
        skill.description = "A test skill".to_string();
        skill.invocation = "test".to_string();
        skill.skill_type = SkillType::Command;
        store.add(&skill).unwrap();

        let pending = queue.pending(10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].entity_type, EntityType::Skill);
        assert_eq!(pending[0].entity_id, skill.id);
        assert_eq!(pending[0].operation, SyncOperation::Upsert);
    }

    #[test]
    fn test_delete_queues_sync() {
        let (temp, store) = create_test_store();
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut skill = Skill::new("test-skill-2".to_string(), "Test Skill".to_string());
        skill.description = "A test skill".to_string();
        skill.invocation = "test".to_string();
        skill.skill_type = SkillType::Command;
        store.add(&skill).unwrap();

        // Clear queue
        queue.clear().unwrap();

        store.delete(&skill.id).unwrap();

        let pending = queue.pending(10, 5).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].operation, SyncOperation::Delete);
    }

    // ── Dual-enqueue behaviour (cas-82a1) ────────────────────────────────

    use cas_types::Scope;

    use crate::store::share_policy::TEST_TEAM_UUID as TEST_TEAM;

    fn create_team_store(team_auto_promote: Option<bool>) -> (TempDir, SyncingSkillStore) {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path();
        let inner = SqliteSkillStore::open(cas_dir).unwrap();
        inner.init().unwrap();
        let queue = SyncQueue::open(cas_dir).unwrap();
        queue.init().unwrap();
        let mut cfg = CloudConfig::default();
        cfg.set_team(TEST_TEAM, "test-team");
        cfg.team_auto_promote = team_auto_promote;
        let store = SyncingSkillStore::new(Arc::new(inner), Arc::new(queue))
            .with_cloud_config(Arc::new(cfg));
        (temp, store)
    }

    fn make_skill(id: &str) -> Skill {
        let mut skill = Skill::new(id.to_string(), "Test Skill".to_string());
        skill.description = "A test skill".to_string();
        skill.invocation = "test".to_string();
        skill.skill_type = SkillType::Command;
        skill
    }

    fn queue_counts(queue: &SyncQueue) -> (usize, usize) {
        let personal = queue.pending(100, 5).unwrap().len();
        let team = queue.pending_for_team(TEST_TEAM, 100, 5).unwrap().len();
        (personal, team)
    }

    #[test]
    fn skill_dual_enqueue_when_team_configured_and_project_scope() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        // Skill::new defaults to Global scope; explicitly set Project for
        // the auto-promote path (Skill type has no Preference analogue,
        // so scope is the only predicate input).
        let mut skill = make_skill("p-skill-001");
        skill.scope = Scope::Project;
        store.add(&skill).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 1);
    }

    #[test]
    fn skill_personal_only_when_global_scope() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut skill = make_skill("g-skill-001");
        skill.scope = Scope::Global;
        store.add(&skill).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 0);
    }

    #[test]
    fn skill_personal_only_when_kill_switch_engaged() {
        let (temp, store) = create_team_store(Some(false));
        let queue = SyncQueue::open(temp.path()).unwrap();

        // Project scope so we'd otherwise dual-enqueue; kill-switch must
        // still suppress it.
        let mut skill = make_skill("p-skill-002");
        skill.scope = Scope::Project;
        store.add(&skill).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 0);
    }

    #[test]
    fn skill_delete_dual_enqueues_when_team_configured() {
        let (temp, store) = create_team_store(None);
        let queue = SyncQueue::open(temp.path()).unwrap();

        let mut skill = make_skill("p-skill-003");
        skill.scope = Scope::Project;
        store.add(&skill).unwrap();
        queue.clear().unwrap();

        store.delete(&skill.id).unwrap();

        let (personal, team) = queue_counts(&queue);
        assert_eq!(personal, 1);
        assert_eq!(team, 1);
    }
}
