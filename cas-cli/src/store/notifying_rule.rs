//! Notifying rule store wrapper
//!
//! Emits notification events on rule add/update for TUI display.

use std::sync::Arc;

use crate::config::NotificationConfig;
use crate::notifications::{NotificationEvent, get_global_notifier};
use crate::store::{Result, RuleStore};
use crate::types::{Rule, RuleStatus};
use cas_store::RuleVersion;

/// A rule store wrapper that emits notification events
pub struct NotifyingRuleStore {
    inner: Arc<dyn RuleStore>,
    config: NotificationConfig,
}

impl NotifyingRuleStore {
    /// Create a new notifying rule store
    pub fn new(inner: Arc<dyn RuleStore>, config: NotificationConfig) -> Self {
        Self { inner, config }
    }

    fn notify_created(&self, rule: &Rule) {
        if self.config.enabled && self.config.rules.on_created {
            if let Some(notifier) = get_global_notifier() {
                notifier.notify(NotificationEvent::rule_created(&rule.id));
            }
        }
    }

    fn notify_status_change(&self, rule: &Rule, old_status: Option<RuleStatus>) {
        if let Some(old) = old_status {
            // Promoted: was not Proven, now is Proven
            if old != RuleStatus::Proven
                && rule.status == RuleStatus::Proven
                && self.config.rules.on_promoted
            {
                if let Some(notifier) = get_global_notifier() {
                    notifier.notify(NotificationEvent::rule_promoted(&rule.id));
                }
                return;
            }

            // Demoted: was Proven, now is Stale
            if old == RuleStatus::Proven
                && rule.status == RuleStatus::Stale
                && self.config.rules.on_demoted
            {
                if let Some(notifier) = get_global_notifier() {
                    notifier.notify(NotificationEvent::rule_demoted(&rule.id));
                }
            }
        }
    }
}

impl RuleStore for NotifyingRuleStore {
    fn init(&self) -> Result<()> {
        self.inner.init()
    }

    fn generate_id(&self) -> Result<String> {
        self.inner.generate_id()
    }

    fn add(&self, rule: &Rule) -> Result<()> {
        self.inner.add(rule)?;
        self.notify_created(rule);
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Rule> {
        self.inner.get(id)
    }

    fn update(&self, rule: &Rule) -> Result<()> {
        // Get old status for transition detection
        let old_status = self.inner.get(&rule.id).ok().map(|r| r.status);

        self.inner.update(rule)?;
        self.notify_status_change(rule, old_status);
        Ok(())
    }

    fn update_with_metadata(
        &self,
        rule: &Rule,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        let old_status = self.inner.get(&rule.id).ok().map(|r| r.status);
        self.inner
            .update_with_metadata(rule, changed_by, change_note)?;
        self.notify_status_change(rule, old_status);
        Ok(())
    }

    fn increment_surface_count(&self, id: &str) -> Result<()> {
        self.inner.increment_surface_count(id)
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
        let old_status = self.inner.get(id).ok().map(|r| r.status);
        self.inner
            .restore_version(id, version, changed_by, change_note)?;
        let restored = self.inner.get(id)?;
        self.notify_status_change(&restored, old_status);
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
    use crate::store::SqliteRuleStore;
    use crate::store::notifying_rule::*;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, NotifyingRuleStore) {
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path();

        let inner = SqliteRuleStore::open(cas_dir).unwrap();
        inner.init().unwrap();

        let config = NotificationConfig::default();
        let store = NotifyingRuleStore::new(Arc::new(inner), config);
        (temp, store)
    }

    #[test]
    fn test_store_operations_work() {
        let (_temp, store) = create_test_store();

        // Test add
        let rule = Rule::new("rule-001".to_string(), "Test rule content".to_string());
        store.add(&rule).unwrap();

        // Test get
        let fetched = store.get("rule-001").unwrap();
        assert_eq!(fetched.content, "Test rule content");

        // Test update (status change to Proven)
        let mut updated_rule = rule.clone();
        updated_rule.status = RuleStatus::Proven;
        store.update(&updated_rule).unwrap();

        let fetched = store.get("rule-001").unwrap();
        assert_eq!(fetched.status, RuleStatus::Proven);

        // Test delete
        store.delete("rule-001").unwrap();
        assert_eq!(store.get("rule-001").unwrap().status, RuleStatus::Retired);
    }
}
