//! Syncing rule store wrapper
//!
//! Automatically syncs rules to Claude Code on add/update/delete,
//! and optionally queues changes for cloud sync. When a team is configured
//! and the rule passes the T1 filter policy, the write is dual-enqueued
//! to both the personal queue and the team queue.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
use crate::store::foreign_project_guard::ForeignProjectGuard;
use crate::store::share_policy::{
    eligible_for_team_rule, origin_is_foreign_or_unknown, resolve_team_id,
};
use crate::store::{Result, RuleStore};
use crate::types::{Rule, RuleStatus};
use cas_core::Syncer;
use cas_store::RuleVersion;

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
    team_only: bool,
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
            team_only: false,
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
            team_only: false,
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
        self.team_only = cloud_config.team_only && self.team_id.is_some();
        self
    }

    /// Enable the foreign-project sync guard for the project at
    /// `project_root` (cas-caae): a rule naming another registered project is
    /// kept out of (and removed from) this project's rule files, and is never
    /// queued for personal or team push (cas-42ee).
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

    // Keep receipts outside the generated directory: removing that entire
    // directory must still withdraw its previously projected policy.
    fn receipt_path(project_root: &Path, id: &str) -> PathBuf {
        let filename: String = id.as_bytes().iter().map(|byte| format!("{byte:02x}")).collect();
        project_root.join(".cas/rule-projections").join(filename)
    }

    /// Shared by wrapped and raw openers, before a pull can add any rules.
    /// Only active rules at this one-time cut are legacy HOLD candidates.
    pub(crate) fn start_receipts(cas_dir: &Path, inner: &dyn RuleStore) -> Result<()> {
        let directory = cas_dir.join("rule-projections");
        let path = directory.join("started.json");
        match std::fs::metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            // Existing, corrupt or unreadable markers are never re-snapshotted.
            // Reads below interpret unavailable evidence as an empty cut.
            _ => return Ok(()),
        }
        let ids: HashSet<String> = inner.list()?.into_iter()
            .filter(Self::live).map(|rule| rule.id).collect();
        let bytes = serde_json::to_vec(&ids)
            .map_err(|error| cas_store::StoreError::Parse(error.to_string()))?;
        std::fs::create_dir_all(&directory)?;
        let mut marker = tempfile::NamedTempFile::new_in(&directory)?;
        marker.write_all(&bytes)?;
        match marker.persist_noclobber(path) {
            Ok(_) => Ok(()),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error.error.into()),
        }
    }

    fn predates_receipts(&self, id: &str) -> bool {
        self.project_root.as_deref()
            .and_then(|root| std::fs::read(root.join(".cas/rule-projections/started.json")).ok())
            .and_then(|bytes| serde_json::from_slice::<HashSet<String>>(&bytes).ok())
            .is_some_and(|ids| ids.contains(id))
    }

    fn receipt(&self, id: &str) -> Option<String> {
        let root = self.project_root.as_deref().filter(|p| !p.as_os_str().is_empty())?;
        let bytes = std::fs::read(Self::receipt_path(root, id)).ok()?;
        let (state, target): (String, PathBuf) = serde_json::from_slice(&bytes).ok()?;
        // Unreadable/invalid metadata is missing evidence, not a reason to
        // break all rule injection. A held source still needs a keep decision.
        (matches!(state.as_str(), "held" | "projected" | "unprojected")
            && (state == "held" || target == self.syncer.target_dir())).then_some(state)
    }

    fn record_projection(&self, id: &str, state: &str) -> Result<()> {
        if let Some(root) = self.project_root.as_deref().filter(|p| !p.as_os_str().is_empty()) {
            let path = Self::receipt_path(root, id);
            let data = serde_json::to_vec(&(state, self.syncer.target_dir()))
                .map_err(|error| cas_store::StoreError::Parse(error.to_string()))?;
            if std::fs::read(&path).is_ok_and(|existing| existing == data) {
                return Ok(());
            }
            let parent = path.parent().expect("receipt parent");
            std::fs::create_dir_all(parent)?;
            let mut receipt = tempfile::NamedTempFile::new_in(parent)?;
            receipt.write_all(&data)?;
            receipt.persist(path).map_err(|error| error.error)?;
        }
        Ok(())
    }

    /// A held source remains visible in rule listing, but is not active policy.
    pub(crate) fn is_held(project_root: &Path, id: &str) -> bool {
        std::fs::read(Self::receipt_path(project_root, id)).ok()
            .and_then(|bytes| serde_json::from_slice::<(String, PathBuf)>(&bytes).ok())
            .is_some_and(|(state, _)| state == "held")
    }

    fn live(rule: &Rule) -> bool {
        rule.status == RuleStatus::Proven || rule.is_active_operator_hard_rule()
    }

    fn reconcile(&self, mut rule: Rule) -> Result<Rule> {
        if self.project_root.as_deref().is_none_or(|p| p.as_os_str().is_empty())
            || !Self::live(&rule) || self.names_foreign_project(&rule)
        {
            return Ok(rule);
        }
        let receipt = self.receipt(&rule.id);
        let file = self.syncer.target_dir().join(format!("{}.md", rule.id));
        if file.is_file() && receipt.as_deref() != Some("held") {
            // Adopt an existing pre-receipt projection without re-writing it.
            self.record_projection(&rule.id, "projected")?;
            return Ok(rule);
        }
        if receipt.as_deref() == Some("unprojected")
            || (receipt.is_none() && !self.predates_receipts(&rule.id))
        {
            // Known new writes and post-marker Cloud arrivals are new policy,
            // not unknown legacy absence; projection must not re-enqueue pull.
            self.record_projection(&rule.id, "unprojected")?;
            self.try_sync(&rule)?;
            return Ok(rule);
        }
        let deleted = receipt.as_deref() == Some("projected");
        let reason = if deleted {
            "retired: previously projected rule file was removed"
        } else {
            self.record_projection(&rule.id, "held")?;
            "held: file absent; use rule action=restore to keep, or delete to retire"
        };
        rule.status = if deleted { RuleStatus::Retired } else { RuleStatus::Stale };
        self.inner.update_with_metadata(&rule, Some("rule projection reconciliation"), Some(reason))?;
        // A legacy hold is local uncertainty, never a speculative Cloud change.
        if deleted { self.queue_upsert(&rule); }
        Ok(rule)
    }

    fn try_sync(&self, rule: &Rule) -> Result<()> {
        if self.names_foreign_project(rule) {
            self.try_remove(&rule.id);
            return Ok(());
        }
        // A held rule cannot be revived by helpful/update or a remote replay.
        if self.receipt(&rule.id).as_deref() == Some("held") {
            return Ok(());
        }
        // File sync remains best-effort; never claim a projection on failure.
        match self.syncer.sync_rule(rule) {
            Ok(true) => self.record_projection(&rule.id, "projected")?,
            Ok(false) if Self::live(rule) => self.record_projection(&rule.id, "unprojected")?,
            Ok(false) => self.forget_projection(&rule.id),
            Err(_) => {} // No new projection was successfully written.
        }
        Ok(())
    }

    fn forget_projection(&self, id: &str) {
        if let Some(root) = self.project_root.as_deref().filter(|p| !p.as_os_str().is_empty()) {
            let _ = std::fs::remove_file(Self::receipt_path(root, id));
        }
    }

    fn try_remove(&self, rule_id: &str) {
        let _ = self.syncer.remove_rule(rule_id);
        self.forget_projection(rule_id);
    }

    fn prepare_update(&self, id: &str) -> Result<bool> {
        let before = self.get(id)?;
        // A draft promoted through the explicit write path is new policy;
        // an already active/held rule must respect its missing projection.
        Ok(!Self::live(&before) && self.receipt(id).is_none())
    }

    fn after_write(&self, id: &str, new_projection: bool) -> Result<()> {
        let stored = self.inner.get(id)?;
        let stored = if new_projection { stored } else { self.reconcile(stored)? };
        self.try_sync(&stored)?;
        if self.receipt(id).as_deref() != Some("held") { self.queue_upsert(&stored); }
        Ok(())
    }

    fn queue_upsert(&self, rule: &Rule) {
        let Some(queue) = &self.cloud_queue else {
            return;
        };
        // cas-42ee: a rule naming another registered project is not this
        // project's to publish. A personal or team push would stamp it with
        // this project's origin, and every pull elsewhere would accept it as
        // ours (the cas-caae Gabber rules). Drop anything queued earlier for
        // it as well, so an older payload cannot publish it either.
        if self.names_foreign_project(rule) {
            let _ = queue.drop_queued_pushes_for(EntityType::Rule.as_str(), &rule.id);
            return;
        }
        let payload = match serde_json::to_string(rule) {
            Ok(p) => p,
            Err(_) => return,
        };

        if !(self.team_only && eligible_for_team_rule(rule)) {
            let _ = queue.enqueue(
                EntityType::Rule,
                &rule.id,
                SyncOperation::Upsert,
                Some(&payload),
            );
        }

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

    fn queue_delete(&self, id: &str, team_eligible: bool) {
        let Some(queue) = &self.cloud_queue else {
            return;
        };
        if !(self.team_only && team_eligible) {
            let _ = queue.enqueue(EntityType::Rule, id, SyncOperation::Delete, None);
        }

        // See `share_policy` module docs: delete fans out unconditionally
        // when a team is configured.
        if let Some(team_id) = self.team_id.as_deref()
            && (!self.team_only || team_eligible)
        {
            let _ =
                queue.enqueue_for_team(EntityType::Rule, id, SyncOperation::Delete, None, team_id);
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
        self.after_write(&rule.id, true)
    }

    fn get(&self, id: &str) -> Result<Rule> {
        self.reconcile(self.inner.get(id)?)
    }

    fn update(&self, rule: &Rule) -> Result<()> {
        let new_projection = self.prepare_update(&rule.id)?;
        self.inner.update(rule)?;
        self.after_write(&rule.id, new_projection)
    }

    fn update_with_metadata(
        &self,
        rule: &Rule,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        let new_projection = self.prepare_update(&rule.id)?;
        self.inner.update_with_metadata(rule, changed_by, change_note)?;
        self.after_write(&rule.id, new_projection)
    }

    fn update_clearing_stakeholder(
        &self,
        rule: &Rule,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        let new_projection = self.prepare_update(&rule.id)?;
        self.inner.update_clearing_stakeholder(rule, changed_by, change_note)?;
        self.after_write(&rule.id, new_projection)
    }

    fn retire_expired_drafts(
        &self, cutoff: chrono::DateTime<chrono::Utc>, change_note: &str,
    ) -> Result<Vec<Rule>> {
        let retired = self.inner.retire_expired_drafts(cutoff, change_note)?;
        for rule in &retired {
            self.after_write(&rule.id, false)?;
        }
        Ok(retired)
    }

    fn increment_surface_count(&self, id: &str) -> Result<()> {
        self.inner.increment_surface_count(id)
    }

    fn delete(&self, id: &str) -> Result<()> {
        let before = self.inner.get(id).ok();
        let team_eligible = before.as_ref().is_some_and(eligible_for_team_rule);
        let local = self
            .cloud_queue
            .as_ref()
            .and_then(|queue| crate::cloud::resolve_canonical_id(queue.cas_dir()));
        let foreign = before.as_ref().is_some_and(|rule| {
            rule.scope == crate::types::Scope::Project
                && origin_is_foreign_or_unknown(rule.origin_project.as_deref(), local.as_deref())
        });
        self.inner.delete(id)?;
        self.try_remove(id);
        if foreign {
            if let Some(queue) = &self.cloud_queue {
                let _ = queue.drop_queued_pushes_for(EntityType::Rule.as_str(), id);
            }
            return Ok(());
        }
        self.queue_delete(id, team_eligible);
        Ok(())
    }

    fn delete_with_metadata(
        &self,
        id: &str,
        changed_by: Option<&str>,
        change_note: Option<&str>,
    ) -> Result<()> {
        let before = self.inner.get(id).ok();
        let team_eligible = before.as_ref().is_some_and(eligible_for_team_rule);
        let local = self
            .cloud_queue
            .as_ref()
            .and_then(|queue| crate::cloud::resolve_canonical_id(queue.cas_dir()));
        let foreign = before.as_ref().is_some_and(|rule| {
            rule.scope == crate::types::Scope::Project
                && origin_is_foreign_or_unknown(rule.origin_project.as_deref(), local.as_deref())
        });
        self.inner
            .delete_with_metadata(id, changed_by, change_note)?;
        self.try_remove(id);
        if foreign {
            if let Some(queue) = &self.cloud_queue {
                let _ = queue.drop_queued_pushes_for(EntityType::Rule.as_str(), id);
            }
            return Ok(());
        }
        self.queue_delete(id, team_eligible);
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
        // Reconcile first so a legacy row's original state is in history.
        self.get(id)?;
        let version = if version.is_none() && self.receipt(id).as_deref() == Some("held") {
            // Repeated edits/votes while held may record additional Stale
            // snapshots; the keep decision restores the newest live source.
            self.inner.list_versions(id)?.into_iter().find_map(|version| {
                serde_json::from_str::<Rule>(&version.snapshot_json).ok()
                    .filter(Self::live).map(|_| version.version)
            })
        } else { version };
        self.inner.restore_version(id, version, changed_by, change_note)?;
        self.forget_projection(id);
        self.after_write(id, true)
    }

    fn list(&self) -> Result<Vec<Rule>> {
        self.inner.list()?.into_iter().map(|rule| self.reconcile(rule)).collect()
    }

    fn list_proven(&self) -> Result<Vec<Rule>> {
        Ok(self.inner.list_proven()?.into_iter().map(|rule| self.reconcile(rule))
            .collect::<Result<Vec<_>>>()?.into_iter()
            .filter(|rule| rule.status == RuleStatus::Proven).collect())
    }

    fn list_critical(&self) -> Result<Vec<Rule>> {
        Ok(self.inner.list_critical()?.into_iter()
            .map(|rule| self.reconcile(rule)).collect::<Result<Vec<_>>>()?
            .into_iter().filter(|rule| matches!(rule.status, RuleStatus::Proven | RuleStatus::Draft)).collect())
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

    #[test]
    fn deleting_projected_rule_files_retires_their_store_source_cas_83c3() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = TempDir::new().unwrap();
        let root = temp.path().join(".cas");
        std::fs::create_dir(&root).unwrap();
        let inner = Arc::new(SqliteRuleStore::open(&root).unwrap());
        inner.init().unwrap();
        let target = temp.path().join(".claude/rules/cas");
        let store = SyncingRuleStore::new(inner.clone(), target.clone(), 1)
            .with_project_root(temp.path().to_path_buf());
        let mut rule = make_rule("rule-deleted", Scope::Project);
        rule.status = cas_types::RuleStatus::Proven;
        rule.helpful_count = 1;
        store.add(&rule).unwrap();
        assert!(target.join("rule-deleted.md").exists());
        let mut hard = make_rule("rule-hard-deleted", Scope::Project);
        hard.content = "HARD RULE: operator policy".into();
        hard.authorize_operator_hard_rule("operator:fixture");
        store.add(&hard).unwrap();
        assert!(target.join("rule-hard-deleted.md").exists());
        // Generated files need not be tracked: deleting the whole rules
        // directory must still withdraw the policy from prompt injection.
        std::fs::remove_dir_all(&target).unwrap();
        // A fresh wrapper must discover the deletion from durable receipts.
        let store = SyncingRuleStore::new(inner, target.clone(), 1)
            .with_project_root(temp.path().to_path_buf());
        let rows = store.list().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|rule| rule.status == cas_types::RuleStatus::Retired));
        cas_core::Syncer::new(target.clone(), 1).sync_all(&rows).unwrap();
        assert!(!target.join("rule-deleted.md").exists());
        assert!(store.list_proven().unwrap().is_empty());
        assert_eq!(store.get("rule-deleted").unwrap().content, rule.content,
                   "retirement preserves the source and history");
    }

    #[test]
    fn legacy_missing_rules_are_held_until_explicit_restore_cas_83c3() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = TempDir::new().unwrap();
        let root = temp.path().join(".cas");
        std::fs::create_dir(&root).unwrap();
        let inner = Arc::new(SqliteRuleStore::open(&root).unwrap());
        inner.init().unwrap();
        let mut rule = make_rule("rule-legacy", Scope::Project);
        rule.status = cas_types::RuleStatus::Proven;
        rule.helpful_count = 1;
        inner.add(&rule).unwrap();
        let mut inactive = Vec::new();
        for (id, status) in [("rule-later-draft", RuleStatus::Draft), ("rule-later-retired", RuleStatus::Retired)] {
            let mut row = make_rule(id, Scope::Project);
            row.status = status;
            inner.add(&row).unwrap();
            inactive.push(row);
        }
        // Opening the base store takes the cut before later arrivals.
        let inner = crate::store::open_rule_store_local(&root).unwrap();
        let target = temp.path().join(".claude/rules/cas");
        let queue = Arc::new(SyncQueue::open(&root).unwrap());
        queue.init().unwrap();
        let store = SyncingRuleStore::with_cloud_queue(inner.clone(), target.clone(), 1, queue.clone())
            .with_project_root(temp.path().to_path_buf());
        assert!(store.list_proven().unwrap().is_empty(), "legacy missing files must not inject");
        assert_eq!(inner.get(&rule.id).unwrap().status, cas_types::RuleStatus::Stale);
        assert!(!target.join("rule-legacy.md").exists());
        assert!(store.list_critical().unwrap().is_empty());
        // A helpful/update write cannot silently adopt the held source.
        store.update(&rule).unwrap();
        assert!(store.list_proven().unwrap().is_empty());
        assert!(!target.join("rule-legacy.md").exists());
        assert!(queue.pending(100, 5).unwrap().is_empty(), "a local hold must not propagate to Cloud");
        // The original version is retained; restore is the explicit keep decision.
        store.restore_version(&rule.id, None, Some("operator"), Some("keep this rule")).unwrap();
        assert_eq!(store.list_proven().unwrap().len(), 1);
        assert!(target.join("rule-legacy.md").exists());
        let moved = SyncingRuleStore::new(inner.clone(), temp.path().join("custom-rules"), 1)
            .with_project_root(temp.path().to_path_buf());
        assert_eq!(moved.get(&rule.id).unwrap().status, RuleStatus::Stale,
            "changing the configured target is not proof of repository deletion");
        for mut row in inactive {
            row.status = RuleStatus::Proven;
            row.helpful_count = 1;
            inner.update(&row).unwrap();
            assert_eq!(store.get(&row.id).unwrap().status, RuleStatus::Proven,
                "inactive rows at the cut must not be legacy HOLD candidates");
            assert!(target.join(format!("{}.md", row.id)).is_file());
        }
    }

    #[test]
    fn post_marker_local_arrivals_project_and_bad_receipts_do_not_disable_rules_cas_83c3() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = TempDir::new().unwrap();
        let root = temp.path().join(".cas");
        std::fs::create_dir(&root).unwrap();
        let inner = crate::store::open_rule_store_local(&root).unwrap();
        let target = temp.path().join(".claude/rules/cas");
        let queue = Arc::new(SyncQueue::open(&root).unwrap());
        queue.init().unwrap();
        let store = SyncingRuleStore::with_cloud_queue(inner.clone(), target.clone(), 1, queue.clone())
            .with_project_root(temp.path().to_path_buf());
        assert!(store.list().unwrap().is_empty()); // Fresh project has no policy.
        drop(store);
        // Cloud pull uses this unwrapped store; remote creation time may be old.
        let mut incoming = make_rule("rule-from-peer", Scope::Project);
        incoming.created = chrono::Utc::now() - chrono::Duration::days(30);
        incoming.status = RuleStatus::Proven;
        incoming.helpful_count = 1;
        inner.add(&incoming).unwrap();
        let store = SyncingRuleStore::with_cloud_queue(inner, target.clone(), 1, queue.clone())
            .with_project_root(temp.path().to_path_buf());
        assert_eq!(store.get(&incoming.id).unwrap().status, RuleStatus::Proven);
        assert!(target.join("rule-from-peer.md").is_file());
        assert!(queue.pending(100, 5).unwrap().is_empty(), "pull projection must not echo into the push queue");
        for bytes in [b"not JSON".to_vec(), vec![0xff]] {
            std::fs::write(SyncingRuleStore::receipt_path(temp.path(), &incoming.id), bytes).unwrap();
            assert_eq!(store.list_proven().unwrap().len(), 1, "bad metadata must not break rule injection");
            assert!(target.join("rule-from-peer.md").is_file());
        }
    }

    #[test]
    fn second_machine_init_pull_then_session_keeps_rules_active_cas_83c3() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = TempDir::new().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        // No wrapped rule operation occurs between init and Cloud/hub pull.
        let local = crate::store::open_rule_store_local(&root).unwrap();
        let mut incoming = make_rule("rule-second-machine", Scope::Project);
        incoming.created = chrono::Utc::now() - chrono::Duration::days(30);
        incoming.status = RuleStatus::Proven;
        incoming.helpful_count = 1;
        local.add(&incoming).unwrap();
        let session = crate::store::open_rule_store(&root).unwrap();
        assert_eq!(session.get(&incoming.id).unwrap().status, RuleStatus::Proven);
        assert!(temp.path().join(".claude/rules/cas/rule-second-machine.md").is_file());
    }

    #[test]
    fn unreadable_start_marker_is_an_empty_cut_cas_83c3() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = TempDir::new().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        let marker = root.join("rule-projections/started.json");
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        for (index, bytes) in [b"not JSON".to_vec(), Vec::new(), vec![0xff]].into_iter().enumerate() {
            std::fs::write(&marker, bytes).unwrap();
            let local = crate::store::open_rule_store_local(&root).unwrap();
            let mut rule = make_rule(&format!("rule-corrupt-start-{index}"), Scope::Project);
            rule.status = RuleStatus::Proven;
            rule.helpful_count = 1;
            local.add(&rule).unwrap();
            let session = crate::store::open_rule_store(&root).unwrap();
            assert!(session.list_proven().unwrap().iter().any(|row| row.id == rule.id));
            assert!(temp.path().join(format!(".claude/rules/cas/{}.md", rule.id)).is_file());
        }
        // An actual read I/O error has the same harmless empty-cut meaning.
        std::fs::remove_file(&marker).unwrap();
        std::fs::create_dir(&marker).unwrap();
        let session = crate::store::open_rule_store(&root).unwrap();
        assert_eq!(session.list_proven().unwrap().len(), 3);
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

    /// cas-42ee: the guard that keeps a foreign rule out of rule files also
    /// keeps it out of the personal and team push queues, including a payload
    /// queued before the rule turned foreign.
    #[test]
    fn foreign_project_rule_is_never_queued_for_push() {
        let (temp, store) = create_team_store(None);
        let store = store.with_project_guard(ForeignProjectGuard::new(
            "cas-src",
            ["gabber-studio".to_string()],
        ));
        let queue = SyncQueue::open(temp.path()).unwrap();

        let own = make_rule("rule-001", Scope::Project);
        store.add(&own).unwrap();
        assert_eq!(
            queue_counts(&queue),
            (1, 1),
            "an own rule is queued as before"
        );
        queue.clear().unwrap();

        let mut rule = make_rule("rule-002", Scope::Project);
        store.add(&rule).unwrap();
        assert_eq!(queue_counts(&queue), (1, 1));

        rule.content = "Gabber Studio branching: ALWAYS cut new branches from `staging`.".into();
        store.update(&rule).unwrap();
        assert_eq!(
            queue_counts(&queue),
            (0, 0),
            "a rule naming another project is not pushed, and its earlier payload is dropped"
        );

        store.add(&make_rule("rule-003", Scope::Global)).unwrap();
        let mut foreign = make_rule("rule-004", Scope::Project);
        foreign.content = "Deploy gabber-studio from staging.".into();
        store.add(&foreign).unwrap();
        assert_eq!(
            queue_counts(&queue),
            (1, 0),
            "only the own global rule is queued"
        );

        // An explicit `project:<slug>` tag declares the scope, as for rule files.
        foreign.tags = vec!["project:gabber-studio".to_string()];
        store.update(&foreign).unwrap();
        assert_eq!(queue_counts(&queue), (2, 1));
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
    fn team_only_rule_routes_project_to_team_and_global_to_personal() {
        let (temp, mut store) = create_team_store(None);
        store.team_only = true;
        let queue = SyncQueue::open(temp.path()).unwrap();
        store
            .add(&make_rule("team-only-rule", Scope::Project))
            .unwrap();
        assert_eq!(queue_counts(&queue), (0, 1));
        store.add(&make_rule("global-rule", Scope::Global)).unwrap();
        assert_eq!(queue_counts(&queue), (1, 1));
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
