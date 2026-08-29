//! Notifying skill store wrapper
//!
//! Emits notification events on skill add/update for TUI display.

use std::sync::Arc;

use crate::config::NotificationConfig;
use crate::notifications::{NotificationEvent, get_global_notifier};
use crate::store::{Result, SkillStore};
use crate::types::{Skill, SkillStatus};
use cas_store::SkillVersion;

/// A skill store wrapper that emits notification events
pub struct NotifyingSkillStore {
    inner: Arc<dyn SkillStore>,
    config: NotificationConfig,
}

impl NotifyingSkillStore {
    /// Create a new notifying skill store
    pub fn new(inner: Arc<dyn SkillStore>, config: NotificationConfig) -> Self {
        Self { inner, config }
    }

    fn notify_created(&self, skill: &Skill) {
        if self.config.enabled && self.config.skills.on_created {
            if let Some(notifier) = get_global_notifier() {
                notifier.notify(NotificationEvent::skill_created(&skill.id, &skill.name));
            }
        }
    }

    fn notify_status_change(&self, skill: &Skill, old_status: Option<SkillStatus>) {
        if let Some(old) = old_status {
            // Enabled: was not Enabled, now is Enabled
            if old != SkillStatus::Enabled
                && skill.status == SkillStatus::Enabled
                && self.config.skills.on_enabled
            {
                if let Some(notifier) = get_global_notifier() {
                    notifier.notify(NotificationEvent::skill_enabled(&skill.id, &skill.name));
                }
                return;
            }

            // Disabled: was Enabled, now is Disabled
            if old == SkillStatus::Enabled
                && skill.status == SkillStatus::Disabled
                && self.config.skills.on_disabled
            {
                if let Some(notifier) = get_global_notifier() {
                    notifier.notify(NotificationEvent::skill_disabled(&skill.id, &skill.name));
                }
            }
        }
    }
}

impl SkillStore for NotifyingSkillStore {
    fn init(&self) -> Result<()> {
        self.inner.init()
    }

    fn generate_id(&self) -> Result<String> {
        self.inner.generate_id()
    }

    fn add(&self, skill: &Skill) -> Result<()> {
        self.inner.add(skill)?;
        self.notify_created(skill);
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Skill> {
        self.inner.get(id)
    }

    fn update(&self, skill: &Skill) -> Result<()> {
        // Get old status for transition detection
        let old_status = self.inner.get(&skill.id).ok().map(|s| s.status);

        self.inner.update(skill)?;
        self.notify_status_change(skill, old_status);
        Ok(())
    }

    fn update_with_metadata(
        &self,
        skill: &Skill,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        let old_status = self.inner.get(&skill.id).ok().map(|s| s.status);
        self.inner
            .update_with_metadata(skill, changed_by, change_note)?;
        self.notify_status_change(skill, old_status);
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.inner.delete(id)
    }

    fn delete_with_metadata(
        &self,
        id: &str,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        self.inner.delete_with_metadata(id, changed_by, change_note)
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
        let old_status = self.inner.get(id).ok().map(|s| s.status);
        self.inner
            .restore_version(id, version, changed_by, change_note)?;
        let restored = self.inner.get(id)?;
        self.notify_status_change(&restored, old_status);
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
    use crate::store::notifying_skill::*;
    use crate::types::SkillType;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, NotifyingSkillStore) {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path();

        let inner = SqliteSkillStore::open(cas_dir).unwrap();
        inner.init().unwrap();

        let config = NotificationConfig::default();
        let store = NotifyingSkillStore::new(Arc::new(inner), config);
        (temp, store)
    }

    #[test]
    fn test_store_operations_work() {
        let (_temp, store) = create_test_store();

        // Test add
        let mut skill = Skill::new("skill-001".to_string(), "Test Skill".to_string());
        skill.description = "A test skill".to_string();
        skill.invocation = "test".to_string();
        skill.skill_type = SkillType::Command;
        skill.status = SkillStatus::Disabled;
        store.add(&skill).unwrap();

        // Test get
        let fetched = store.get("skill-001").unwrap();
        assert_eq!(fetched.name, "Test Skill");
        assert_eq!(fetched.status, SkillStatus::Disabled);

        // Test update (enable the skill)
        skill.status = SkillStatus::Enabled;
        store.update(&skill).unwrap();

        let fetched = store.get("skill-001").unwrap();
        assert_eq!(fetched.status, SkillStatus::Enabled);

        // Test delete
        store.delete("skill-001").unwrap();
        assert_eq!(store.get("skill-001").unwrap().status, SkillStatus::Retired);
    }
}
