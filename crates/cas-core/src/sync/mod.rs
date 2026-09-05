//! Claude Code syncing
//!
//! Syncs proven rules and enabled skills to Claude Code directories for integration.
//!
//! # Two-Tier Architecture
//!
//! CAS uses a two-tier storage architecture:
//! - **Global rules** (`~/.config/cas/`) sync to a user-wide Claude Code rules directory
//! - **Project rules** (`./.cas/`) sync to project's `.claude/rules/cas/`
//!
//! # Sync Targets
//!
//! - **Rules**: `.claude/rules/cas/` - Proven rules become Claude Code rules
//! - **Skills**: `.claude/skills/cas-<name>/` - Enabled skills become Agent Skills
//!
//! # Usage
//!
//! ```rust,ignore
//! use cas_core::sync::Syncer;
//!
//! // Project-level syncing
//! let syncer = Syncer::with_defaults(project_root);
//! let report = syncer.sync_all(&rules)?;
//!
//! // Two-tier syncing
//! let syncer = Syncer::for_two_tier(global_root, project_root);
//! let report = syncer.sync_all_tiered(&global_rules, &project_rules)?;
//! ```
//!
//! # Sync Behavior
//!
//! - Rules must be "proven" (status = proven and helpful_count >= min_helpful)
//! - Stale files are automatically removed when rules become unproven
//! - Skills are synced when enabled, removed when disabled

pub mod agents_md;
pub mod skills;
pub mod specs;
pub mod yaml_scalar;

pub use agents_md::{
    AgentsMdFileReport, AgentsMdSyncMode, AgentsMdSyncReport, GENERATED_HEADER, sync_agents_md,
    transform_agents_md,
};
pub use skills::{SkillSyncReport, SkillSyncer, create_planning_skill, generate_planning_skill};
pub use specs::{SpecSyncReport, SpecSyncer};
pub use yaml_scalar::yaml_scalar;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::CoreError;
use cas_types::{Rule, RuleStatus, Scope};

/// Syncs rules to Claude Code
pub struct Syncer {
    /// Target directory for project rules
    target_dir: PathBuf,
    /// Target directory for global rules (optional)
    global_target_dir: Option<PathBuf>,
    /// Minimum helpful count for a rule to be synced
    min_helpful: i32,
}

/// Report of sync operation
#[derive(Debug, Default)]
pub struct SyncReport {
    /// Number of rules synced
    pub synced: usize,
    /// Number of stale files removed
    pub removed: usize,
    /// Rules that were synced
    pub synced_ids: Vec<String>,
    /// Files that were removed
    pub removed_ids: Vec<String>,
    /// Number of global rules synced
    pub global_synced: usize,
    /// Number of global stale files removed
    pub global_removed: usize,
}

impl Syncer {
    /// Create a new syncer
    pub fn new(target_dir: PathBuf, min_helpful: i32) -> Self {
        Self {
            target_dir,
            global_target_dir: None,
            min_helpful,
        }
    }

    /// Create a syncer with default settings
    pub fn with_defaults(project_root: &Path) -> Self {
        Self {
            target_dir: project_root.join(".claude/rules/cas"),
            global_target_dir: None,
            min_helpful: 1,
        }
    }

    /// Create a syncer for two-tier architecture
    ///
    /// # Arguments
    /// * `global_root` - Optional global CAS root (~/.config/cas/)
    /// * `project_root` - Project root directory
    pub fn for_two_tier(global_root: Option<&Path>, project_root: &Path) -> Self {
        Self {
            target_dir: project_root.join(".claude/rules/cas"),
            global_target_dir: global_root
                .map(|g| g.parent().unwrap_or(g).join(".claude/rules/cas-global")),
            min_helpful: 1,
        }
    }

    /// Set the global target directory
    pub fn with_global_target(mut self, global_target: PathBuf) -> Self {
        self.global_target_dir = Some(global_target);
        self
    }

    /// Check if a rule should be synced to Claude Code
    pub fn is_proven(&self, rule: &Rule) -> bool {
        rule.status == RuleStatus::Proven && rule.helpful_count >= self.min_helpful
    }

    /// Sync a single rule to target directory
    ///
    /// Returns true if the rule was synced, false if it wasn't proven
    pub fn sync_rule(&self, rule: &Rule) -> Result<bool, CoreError> {
        if !self.is_proven(rule) {
            // If rule exists but is no longer proven, remove it
            let filepath = self.target_dir.join(format!("{}.md", rule.id));
            if filepath.exists() {
                fs::remove_file(&filepath)?;
            }
            return Ok(false);
        }

        fs::create_dir_all(&self.target_dir)?;

        let filepath = self.target_dir.join(format!("{}.md", rule.id));

        let content = if rule.paths.is_empty() {
            format!("---\nid: {}\n---\n\n{}", rule.id, rule.content.trim())
        } else {
            format!(
                "---\nid: {}\npaths: \"{}\"\n---\n\n{}",
                rule.id,
                rule.paths,
                rule.content.trim()
            )
        };

        fs::write(&filepath, content)?;
        Ok(true)
    }

    /// Sync all proven rules and remove stale files
    pub fn sync_all(&self, rules: &[Rule]) -> Result<SyncReport, CoreError> {
        let mut report = SyncReport::default();

        // Collect IDs of proven rules
        let proven_ids: HashSet<_> = rules
            .iter()
            .filter(|r| self.is_proven(r))
            .map(|r| r.id.clone())
            .collect();

        // Sync proven rules
        for rule in rules {
            if self.sync_rule(rule)? {
                report.synced += 1;
                report.synced_ids.push(rule.id.clone());
            }
        }

        // Remove stale files
        if self.target_dir.exists() {
            for entry in fs::read_dir(&self.target_dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.extension().map(|e| e == "md").unwrap_or(false) {
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default();

                    if !proven_ids.contains(stem) {
                        fs::remove_file(&path)?;
                        report.removed += 1;
                        report.removed_ids.push(stem.to_string());
                    }
                }
            }
        }

        Ok(report)
    }

    /// Remove a specific rule file
    pub fn remove_rule(&self, rule_id: &str) -> Result<(), CoreError> {
        let filepath = self.target_dir.join(format!("{rule_id}.md"));
        if filepath.exists() {
            fs::remove_file(filepath)?;
        }
        Ok(())
    }

    /// Get the target directory path
    pub fn target_dir(&self) -> &Path {
        &self.target_dir
    }

    /// List all synced rule files
    pub fn list_synced(&self) -> Result<Vec<String>, CoreError> {
        let mut ids = Vec::new();

        if self.target_dir.exists() {
            for entry in fs::read_dir(&self.target_dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.extension().map(|e| e == "md").unwrap_or(false) {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        ids.push(stem.to_string());
                    }
                }
            }
        }

        ids.sort();
        Ok(ids)
    }

    /// Get the global target directory path (if configured)
    pub fn global_target_dir(&self) -> Option<&Path> {
        self.global_target_dir.as_deref()
    }

    /// Sync a single rule to the appropriate target directory based on scope
    pub fn sync_rule_with_scope(&self, rule: &Rule) -> Result<bool, CoreError> {
        let target = match rule.scope {
            Scope::Global => {
                if let Some(ref global_dir) = self.global_target_dir {
                    global_dir
                } else {
                    // Fall back to project directory if no global configured
                    &self.target_dir
                }
            }
            Scope::Project => &self.target_dir,
        };

        if !self.is_proven(rule) {
            // If rule exists but is no longer proven, remove it
            let filepath = target.join(format!("{}.md", rule.id));
            if filepath.exists() {
                fs::remove_file(&filepath)?;
            }
            return Ok(false);
        }

        fs::create_dir_all(target)?;

        let filepath = target.join(format!("{}.md", rule.id));

        let scope_indicator = match rule.scope {
            Scope::Global => "global",
            Scope::Project => "project",
        };

        let content = if rule.paths.is_empty() {
            format!(
                "---\nid: {}\nscope: {}\n---\n\n{}",
                rule.id,
                scope_indicator,
                rule.content.trim()
            )
        } else {
            format!(
                "---\nid: {}\nscope: {}\npaths: \"{}\"\n---\n\n{}",
                rule.id,
                scope_indicator,
                rule.paths,
                rule.content.trim()
            )
        };

        fs::write(&filepath, content)?;
        Ok(true)
    }

    /// Sync rules from both global and project stores
    ///
    /// Global rules are synced to global_target_dir, project rules to target_dir.
    pub fn sync_all_tiered(
        &self,
        global_rules: &[Rule],
        project_rules: &[Rule],
    ) -> Result<SyncReport, CoreError> {
        let mut report = SyncReport::default();

        // Sync project rules
        let project_report = self.sync_rules_to_dir(&self.target_dir, project_rules)?;
        report.synced = project_report.synced;
        report.removed = project_report.removed;
        report.synced_ids = project_report.synced_ids;
        report.removed_ids = project_report.removed_ids;

        // Sync global rules if global target is configured
        if let Some(ref global_dir) = self.global_target_dir {
            let global_report = self.sync_rules_to_dir(global_dir, global_rules)?;
            report.global_synced = global_report.synced;
            report.global_removed = global_report.removed;
            report.synced_ids.extend(global_report.synced_ids);
            report.removed_ids.extend(global_report.removed_ids);
        }

        Ok(report)
    }

    /// Internal helper to sync rules to a specific directory
    fn sync_rules_to_dir(
        &self,
        target_dir: &Path,
        rules: &[Rule],
    ) -> Result<SyncReport, CoreError> {
        let mut report = SyncReport::default();

        // Collect IDs of proven rules
        let proven_ids: HashSet<_> = rules
            .iter()
            .filter(|r| self.is_proven(r))
            .map(|r| r.id.clone())
            .collect();

        // Sync proven rules
        for rule in rules {
            if self.is_proven(rule) {
                fs::create_dir_all(target_dir)?;

                let filepath = target_dir.join(format!("{}.md", rule.id));

                let content = if rule.paths.is_empty() {
                    format!("---\nid: {}\n---\n\n{}", rule.id, rule.content.trim())
                } else {
                    format!(
                        "---\nid: {}\npaths: \"{}\"\n---\n\n{}",
                        rule.id,
                        rule.paths,
                        rule.content.trim()
                    )
                };

                fs::write(&filepath, content)?;
                report.synced += 1;
                report.synced_ids.push(rule.id.clone());
            }
        }

        // Remove stale files
        if target_dir.exists() {
            for entry in fs::read_dir(target_dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.extension().map(|e| e == "md").unwrap_or(false) {
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default();

                    if !proven_ids.contains(stem) {
                        fs::remove_file(&path)?;
                        report.removed += 1;
                        report.removed_ids.push(stem.to_string());
                    }
                }
            }
        }

        Ok(report)
    }

    /// List all synced rule files from both project and global directories
    pub fn list_all_synced(&self) -> Result<(Vec<String>, Vec<String>), CoreError> {
        let project_ids = self.list_synced()?;
        let global_ids = if let Some(ref global_dir) = self.global_target_dir {
            self.list_synced_from_dir(global_dir)?
        } else {
            Vec::new()
        };
        Ok((project_ids, global_ids))
    }

    /// List synced rules from a specific directory
    fn list_synced_from_dir(&self, dir: &Path) -> Result<Vec<String>, CoreError> {
        let mut ids = Vec::new();

        if dir.exists() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.extension().map(|e| e == "md").unwrap_or(false) {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        ids.push(stem.to_string());
                    }
                }
            }
        }

        ids.sort();
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use crate::sync::*;
    use tempfile::TempDir;

    #[test]
    fn test_is_proven() {
        let temp = TempDir::new().unwrap();
        let syncer = Syncer::new(temp.path().to_path_buf(), 2);

        let mut rule = Rule::new("rule-001".to_string(), "Test rule".to_string());

        // Not proven yet (helpful_count = 0)
        assert!(!syncer.is_proven(&rule));

        // Still not proven (helpful_count = 1)
        rule.helpful_count = 1;
        assert!(!syncer.is_proven(&rule));

        // Now proven (helpful_count = 2)
        rule.helpful_count = 2;
        rule.status = RuleStatus::Proven;
        assert!(syncer.is_proven(&rule));

        // Retired rules are never proven
        rule.status = RuleStatus::Retired;
        assert!(!syncer.is_proven(&rule));

        // Stale rules are no longer injected after a harmful/corrected outcome
        rule.status = RuleStatus::Stale;
        assert!(!syncer.is_proven(&rule));
    }

    #[test]
    fn test_sync_rule() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("claude/rules/cas");
        let syncer = Syncer::new(target.clone(), 1);

        let mut rule = Rule::new("rule-001".to_string(), "Test rule content".to_string());
        rule.helpful_count = 1;
        rule.status = RuleStatus::Proven;

        // Sync the rule
        assert!(syncer.sync_rule(&rule).unwrap());

        // Check file was created
        let filepath = target.join("rule-001.md");
        assert!(filepath.exists());

        let content = fs::read_to_string(&filepath).unwrap();
        assert!(content.contains("id: rule-001"));
        assert!(content.contains("Test rule content"));
    }

    #[test]
    fn test_sync_all() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("claude/rules/cas");
        let syncer = Syncer::new(target.clone(), 1);

        let mut rule1 = Rule::new("rule-001".to_string(), "Rule 1".to_string());
        rule1.helpful_count = 2;
        rule1.status = RuleStatus::Proven;

        let mut rule2 = Rule::new("rule-002".to_string(), "Rule 2".to_string());
        rule2.helpful_count = 0; // Not proven

        let rules = vec![rule1, rule2];
        let report = syncer.sync_all(&rules).unwrap();

        assert_eq!(report.synced, 1);
        assert!(target.join("rule-001.md").exists());
        assert!(!target.join("rule-002.md").exists());
    }

    #[test]
    fn test_for_two_tier() {
        let temp = TempDir::new().unwrap();
        let global_root = temp.path().join("global");
        let project_root = temp.path().join("project");
        fs::create_dir_all(&global_root).unwrap();
        fs::create_dir_all(&project_root).unwrap();

        let syncer = Syncer::for_two_tier(Some(&global_root), &project_root);

        assert_eq!(syncer.target_dir(), project_root.join(".claude/rules/cas"));
        assert!(syncer.global_target_dir().is_some());
    }

    #[test]
    fn test_sync_all_tiered() {
        let temp = TempDir::new().unwrap();
        let project_target = temp.path().join("project/.claude/rules/cas");
        let global_target = temp.path().join("global/.claude/rules/cas-global");

        let syncer =
            Syncer::new(project_target.clone(), 1).with_global_target(global_target.clone());

        // Create global rules
        let mut global_rule = Rule::new("g-rule-001".to_string(), "Global rule".to_string());
        global_rule.helpful_count = 1;
        global_rule.status = RuleStatus::Proven;
        global_rule.scope = Scope::Global;

        // Create project rules
        let mut project_rule = Rule::new("p-rule-001".to_string(), "Project rule".to_string());
        project_rule.helpful_count = 1;
        project_rule.status = RuleStatus::Proven;
        project_rule.scope = Scope::Project;

        let report = syncer
            .sync_all_tiered(&[global_rule], &[project_rule])
            .unwrap();

        // Check both directories have the synced rules
        assert_eq!(report.synced, 1);
        assert_eq!(report.global_synced, 1);
        assert!(project_target.join("p-rule-001.md").exists());
        assert!(global_target.join("g-rule-001.md").exists());
    }

    #[test]
    fn test_sync_rule_with_scope() {
        let temp = TempDir::new().unwrap();
        let project_target = temp.path().join("project/.claude/rules/cas");
        let global_target = temp.path().join("global/.claude/rules/cas-global");

        let syncer =
            Syncer::new(project_target.clone(), 1).with_global_target(global_target.clone());

        // Test global rule goes to global target
        let mut global_rule = Rule::new("g-rule-001".to_string(), "Global rule".to_string());
        global_rule.helpful_count = 1;
        global_rule.status = RuleStatus::Proven;
        global_rule.scope = Scope::Global;
        assert!(syncer.sync_rule_with_scope(&global_rule).unwrap());
        assert!(global_target.join("g-rule-001.md").exists());

        // Test project rule goes to project target
        let mut project_rule = Rule::new("p-rule-001".to_string(), "Project rule".to_string());
        project_rule.helpful_count = 1;
        project_rule.status = RuleStatus::Proven;
        project_rule.scope = Scope::Project;
        assert!(syncer.sync_rule_with_scope(&project_rule).unwrap());
        assert!(project_target.join("p-rule-001.md").exists());

        // Check content includes scope indicator
        let content = fs::read_to_string(global_target.join("g-rule-001.md")).unwrap();
        assert!(content.contains("scope: global"));

        let content = fs::read_to_string(project_target.join("p-rule-001.md")).unwrap();
        assert!(content.contains("scope: project"));
    }

    #[test]
    fn test_list_all_synced() {
        let temp = TempDir::new().unwrap();
        let project_target = temp.path().join("project/.claude/rules/cas");
        let global_target = temp.path().join("global/.claude/rules/cas-global");

        let syncer =
            Syncer::new(project_target.clone(), 1).with_global_target(global_target.clone());

        // Create some files
        fs::create_dir_all(&project_target).unwrap();
        fs::create_dir_all(&global_target).unwrap();
        fs::write(project_target.join("p-rule-001.md"), "project rule").unwrap();
        fs::write(global_target.join("g-rule-001.md"), "global rule").unwrap();

        let (project_ids, global_ids) = syncer.list_all_synced().unwrap();

        assert_eq!(project_ids, vec!["p-rule-001".to_string()]);
        assert_eq!(global_ids, vec!["g-rule-001".to_string()]);
    }
}
