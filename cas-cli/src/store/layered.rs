//! Layered store for two-tier (global + project) storage
//!
//! The LayeredStore wraps both global and project stores, routing operations
//! based on scope and merging results from both tiers.

use std::path::Path;
use std::sync::Arc;

use crate::types::{Entry, Rule, Scope, ScopeFilter, Skill, SkillStatus};

use crate::store::{Result, RuleStore, SkillStore, Store};

/// Layered entry store combining global and project tiers
pub struct LayeredEntryStore {
    /// Global store (~/.config/cas/)
    global: Arc<dyn Store>,
    /// Project store (./.cas/) - optional, may not exist
    project: Option<Arc<dyn Store>>,
}

impl LayeredEntryStore {
    /// Create a new layered store with both tiers
    pub fn new(global: Arc<dyn Store>, project: Option<Arc<dyn Store>>) -> Self {
        Self { global, project }
    }

    /// Create a layered store with only global tier
    pub fn global_only(global: Arc<dyn Store>) -> Self {
        Self {
            global,
            project: None,
        }
    }

    /// Get the global store
    pub fn global(&self) -> &Arc<dyn Store> {
        &self.global
    }

    /// Get the project store if available
    pub fn project(&self) -> Option<&Arc<dyn Store>> {
        self.project.as_ref()
    }

    /// Check if project store is available
    pub fn has_project(&self) -> bool {
        self.project.is_some()
    }

    /// Get the appropriate store for a scope
    fn store_for_scope(&self, scope: Scope) -> &Arc<dyn Store> {
        match scope {
            Scope::Global => &self.global,
            Scope::Project => self.project.as_ref().unwrap_or(&self.global),
        }
    }

    /// Generate ID with scope prefix
    pub fn generate_id(&self, scope: Scope) -> Result<String> {
        let store = self.store_for_scope(scope);
        let base_id = store.generate_id()?;
        Ok(format!("{}-{}", scope.id_prefix(), base_id))
    }

    /// Add entry to appropriate store based on its scope
    pub fn add(&self, entry: &Entry) -> Result<()> {
        let store = self.store_for_scope(entry.scope);
        store.add(entry)
    }

    /// Get entry by ID, checking both stores
    pub fn get(&self, id: &str) -> Result<Entry> {
        // Try to determine scope from ID prefix
        if let Some(scope) = Scope::from_id(id) {
            let store = self.store_for_scope(scope);
            return store.get(id);
        }

        // No prefix - try project first, then global
        if let Some(ref project) = self.project {
            if let Ok(entry) = project.get(id) {
                return Ok(entry);
            }
        }
        self.global.get(id)
    }

    /// Update entry in appropriate store
    pub fn update(&self, entry: &Entry) -> Result<()> {
        let store = self.store_for_scope(entry.scope);
        store.update(entry)
    }

    /// Delete entry from appropriate store
    pub fn delete(&self, id: &str) -> Result<()> {
        if let Some(scope) = Scope::from_id(id) {
            let store = self.store_for_scope(scope);
            return store.delete(id);
        }

        // Try both stores
        if let Some(ref project) = self.project {
            if project.delete(id).is_ok() {
                return Ok(());
            }
        }
        self.global.delete(id)
    }

    /// List entries with scope filter
    pub fn list(&self, filter: ScopeFilter) -> Result<Vec<Entry>> {
        match filter {
            ScopeFilter::Global => self.global.list(),
            ScopeFilter::Project => {
                if let Some(ref project) = self.project {
                    project.list()
                } else {
                    Ok(Vec::new())
                }
            }
            ScopeFilter::All => {
                let mut entries = self.global.list()?;
                if let Some(ref project) = self.project {
                    let project_entries = project.list()?;
                    entries.reserve(project_entries.len());
                    entries.extend(project_entries);
                }
                // Sort by creation date descending
                entries.sort_unstable_by(|a, b| b.created.cmp(&a.created));
                Ok(entries)
            }
        }
    }

    /// Get recent entries from both stores
    pub fn recent(&self, n: usize, filter: ScopeFilter) -> Result<Vec<Entry>> {
        match filter {
            ScopeFilter::Global => self.global.recent(n),
            ScopeFilter::Project => {
                if let Some(ref project) = self.project {
                    project.recent(n)
                } else {
                    Ok(Vec::new())
                }
            }
            ScopeFilter::All => {
                // Get more than n from each to ensure good coverage
                let mut entries = self.global.recent(n)?;
                if let Some(ref project) = self.project {
                    entries.extend(project.recent(n)?);
                }
                // Sort by creation date descending and take top n
                entries.sort_by(|a, b| b.created.cmp(&a.created));
                entries.truncate(n);
                Ok(entries)
            }
        }
    }

    /// Archive entry
    pub fn archive(&self, id: &str) -> Result<()> {
        if let Some(scope) = Scope::from_id(id) {
            let store = self.store_for_scope(scope);
            return store.archive(id);
        }

        if let Some(ref project) = self.project {
            if project.archive(id).is_ok() {
                return Ok(());
            }
        }
        self.global.archive(id)
    }

    /// Unarchive entry
    pub fn unarchive(&self, id: &str) -> Result<()> {
        if let Some(scope) = Scope::from_id(id) {
            let store = self.store_for_scope(scope);
            return store.unarchive(id);
        }

        if let Some(ref project) = self.project {
            if project.unarchive(id).is_ok() {
                return Ok(());
            }
        }
        self.global.unarchive(id)
    }

    /// List helpful entries from both stores
    pub fn list_helpful(&self, limit: usize, filter: ScopeFilter) -> Result<Vec<Entry>> {
        match filter {
            ScopeFilter::Global => self.global.list_helpful(limit),
            ScopeFilter::Project => {
                if let Some(ref project) = self.project {
                    project.list_helpful(limit)
                } else {
                    Ok(Vec::new())
                }
            }
            ScopeFilter::All => {
                let mut entries = self.global.list_helpful(limit)?;
                if let Some(ref project) = self.project {
                    entries.extend(project.list_helpful(limit)?);
                }
                // Sort by feedback score descending
                entries.sort_by_key(|e| std::cmp::Reverse(e.feedback_score()));
                entries.truncate(limit);
                Ok(entries)
            }
        }
    }

    /// List unreviewed learning entries from both stores
    pub fn list_unreviewed_learnings(&self, limit: usize, filter: ScopeFilter) -> Result<Vec<Entry>> {
        match filter {
            ScopeFilter::Global => self.global.list_unreviewed_learnings(limit),
            ScopeFilter::Project => {
                if let Some(ref project) = self.project {
                    project.list_unreviewed_learnings(limit)
                } else {
                    Ok(Vec::new())
                }
            }
            ScopeFilter::All => {
                let mut entries = self.global.list_unreviewed_learnings(limit)?;
                if let Some(ref project) = self.project {
                    entries.extend(project.list_unreviewed_learnings(limit)?);
                }
                // Sort by creation date descending
                entries.sort_by(|a, b| b.created.cmp(&a.created));
                entries.truncate(limit);
                Ok(entries)
            }
        }
    }

    /// Mark an entry as reviewed
    pub fn mark_reviewed(&self, id: &str) -> Result<()> {
        if let Some(scope) = Scope::from_id(id) {
            let store = self.store_for_scope(scope);
            return store.mark_reviewed(id);
        }

        // Try both stores
        if let Some(ref project) = self.project {
            if project.mark_reviewed(id).is_ok() {
                return Ok(());
            }
        }
        self.global.mark_reviewed(id)
    }

    /// Get the .cas directory path (project if available, otherwise global)
    pub fn cas_dir(&self) -> &Path {
        if let Some(ref project) = self.project {
            project.cas_dir()
        } else {
            self.global.cas_dir()
        }
    }

    /// Get the global cas directory
    pub fn global_cas_dir(&self) -> &Path {
        self.global.cas_dir()
    }

    /// Get the project cas directory if available
    pub fn project_cas_dir(&self) -> Option<&Path> {
        self.project.as_ref().map(|p| p.cas_dir())
    }
}

/// Layered rule store combining global and project tiers
pub struct LayeredRuleStore {
    global: Arc<dyn RuleStore>,
    project: Option<Arc<dyn RuleStore>>,
}

impl LayeredRuleStore {
    /// Create a new layered rule store
    pub fn new(global: Arc<dyn RuleStore>, project: Option<Arc<dyn RuleStore>>) -> Self {
        Self { global, project }
    }

    /// Create with only global tier
    pub fn global_only(global: Arc<dyn RuleStore>) -> Self {
        Self {
            global,
            project: None,
        }
    }

    /// Get store for scope
    fn store_for_scope(&self, scope: Scope) -> &Arc<dyn RuleStore> {
        match scope {
            Scope::Global => &self.global,
            Scope::Project => self.project.as_ref().unwrap_or(&self.global),
        }
    }

    /// Generate ID with scope prefix
    pub fn generate_id(&self, scope: Scope) -> Result<String> {
        let store = self.store_for_scope(scope);
        let base_id = store.generate_id()?;
        Ok(format!("{}-{}", scope.id_prefix(), base_id))
    }

    /// Add rule to appropriate store
    pub fn add(&self, rule: &Rule) -> Result<()> {
        let store = self.store_for_scope(rule.scope);
        store.add(rule)
    }

    /// Get rule by ID
    pub fn get(&self, id: &str) -> Result<Rule> {
        if let Some(scope) = Scope::from_id(id) {
            let store = self.store_for_scope(scope);
            return store.get(id);
        }

        if let Some(ref project) = self.project {
            if let Ok(rule) = project.get(id) {
                return Ok(rule);
            }
        }
        self.global.get(id)
    }

    /// Update rule
    pub fn update(&self, rule: &Rule) -> Result<()> {
        let store = self.store_for_scope(rule.scope);
        store.update(rule)
    }

    /// Delete rule
    pub fn delete(&self, id: &str) -> Result<()> {
        if let Some(scope) = Scope::from_id(id) {
            let store = self.store_for_scope(scope);
            return store.delete(id);
        }

        if let Some(ref project) = self.project {
            if project.delete(id).is_ok() {
                return Ok(());
            }
        }
        self.global.delete(id)
    }

    /// List rules with scope filter
    pub fn list(&self, filter: ScopeFilter) -> Result<Vec<Rule>> {
        match filter {
            ScopeFilter::Global => self.global.list(),
            ScopeFilter::Project => {
                if let Some(ref project) = self.project {
                    project.list()
                } else {
                    Ok(Vec::new())
                }
            }
            ScopeFilter::All => {
                let mut rules = self.global.list()?;
                if let Some(ref project) = self.project {
                    rules.extend(project.list()?);
                }
                // Sort by creation date descending
                rules.sort_by(|a, b| b.created.cmp(&a.created));
                Ok(rules)
            }
        }
    }
}

/// Layered skill store combining global and project tiers
pub struct LayeredSkillStore {
    global: Arc<dyn SkillStore>,
    project: Option<Arc<dyn SkillStore>>,
}

impl LayeredSkillStore {
    /// Create a new layered skill store
    pub fn new(global: Arc<dyn SkillStore>, project: Option<Arc<dyn SkillStore>>) -> Self {
        Self { global, project }
    }

    /// Create with only global tier
    pub fn global_only(global: Arc<dyn SkillStore>) -> Self {
        Self {
            global,
            project: None,
        }
    }

    /// Get store for scope
    fn store_for_scope(&self, scope: Scope) -> &Arc<dyn SkillStore> {
        match scope {
            Scope::Global => &self.global,
            Scope::Project => self.project.as_ref().unwrap_or(&self.global),
        }
    }

    /// Generate ID with scope prefix
    pub fn generate_id(&self, scope: Scope) -> Result<String> {
        let store = self.store_for_scope(scope);
        let base_id = store.generate_id()?;
        Ok(format!("{}-{}", scope.id_prefix(), base_id))
    }

    /// Add skill to appropriate store
    pub fn add(&self, skill: &Skill) -> Result<()> {
        let store = self.store_for_scope(skill.scope);
        store.add(skill)
    }

    /// Get skill by ID
    pub fn get(&self, id: &str) -> Result<Skill> {
        if let Some(scope) = Scope::from_id(id) {
            let store = self.store_for_scope(scope);
            return store.get(id);
        }

        if let Some(ref project) = self.project {
            if let Ok(skill) = project.get(id) {
                return Ok(skill);
            }
        }
        self.global.get(id)
    }

    /// Update skill
    pub fn update(&self, skill: &Skill) -> Result<()> {
        let store = self.store_for_scope(skill.scope);
        store.update(skill)
    }

    /// Delete skill
    pub fn delete(&self, id: &str) -> Result<()> {
        if let Some(scope) = Scope::from_id(id) {
            let store = self.store_for_scope(scope);
            return store.delete(id);
        }

        if let Some(ref project) = self.project {
            if project.delete(id).is_ok() {
                return Ok(());
            }
        }
        self.global.delete(id)
    }

    /// List skills with scope filter
    pub fn list(&self, status: Option<SkillStatus>, filter: ScopeFilter) -> Result<Vec<Skill>> {
        match filter {
            ScopeFilter::Global => self.global.list(status),
            ScopeFilter::Project => {
                if let Some(ref project) = self.project {
                    project.list(status)
                } else {
                    Ok(Vec::new())
                }
            }
            ScopeFilter::All => {
                let mut skills = self.global.list(status)?;
                if let Some(ref project) = self.project {
                    skills.extend(project.list(status)?);
                }
                // Sort by name
                skills.sort_by(|a, b| a.name.cmp(&b.name));
                Ok(skills)
            }
        }
    }

    /// List enabled skills from both stores
    pub fn list_enabled(&self) -> Result<Vec<Skill>> {
        let mut skills = self.global.list_enabled()?;
        if let Some(ref project) = self.project {
            skills.extend(project.list_enabled()?);
        }
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(skills)
    }

    /// Search skills in both stores
    pub fn search(&self, query: &str) -> Result<Vec<Skill>> {
        let mut skills = self.global.search(query)?;
        if let Some(ref project) = self.project {
            skills.extend(project.search(query)?);
        }
        Ok(skills)
    }
}

#[cfg(test)]
mod tests {
    use crate::store::layered::*;
    use crate::store::mock::{MockRuleStore, MockSkillStore, MockStore};

    #[test]
    fn test_layered_entry_store_global_only() {
        let global: Arc<dyn Store> = Arc::new(MockStore::new());
        let layered = LayeredEntryStore::global_only(global);

        assert!(!layered.has_project());
    }

    #[test]
    fn test_layered_entry_store_with_project() {
        let global: Arc<dyn Store> = Arc::new(MockStore::new());
        let project: Arc<dyn Store> = Arc::new(MockStore::new());
        let layered = LayeredEntryStore::new(global, Some(project));

        assert!(layered.has_project());
    }

    #[test]
    fn test_layered_entry_store_add_and_get() {
        let global: Arc<dyn Store> = Arc::new(MockStore::new());
        let project: Arc<dyn Store> = Arc::new(MockStore::new());
        let layered = LayeredEntryStore::new(Arc::clone(&global), Some(Arc::clone(&project)));

        // Add global entry
        let mut global_entry = Entry::new("g-001".to_string(), "Global entry".to_string());
        global_entry.scope = Scope::Global;
        layered.add(&global_entry).unwrap();

        // Add project entry
        let mut project_entry = Entry::new("p-001".to_string(), "Project entry".to_string());
        project_entry.scope = Scope::Project;
        layered.add(&project_entry).unwrap();

        // Retrieve by ID
        let got_global = layered.get("g-001").unwrap();
        assert_eq!(got_global.content, "Global entry");

        let got_project = layered.get("p-001").unwrap();
        assert_eq!(got_project.content, "Project entry");
    }

    #[test]
    fn test_layered_entry_store_list_all() {
        let global: Arc<dyn Store> = Arc::new(MockStore::new());
        let project: Arc<dyn Store> = Arc::new(MockStore::new());
        let layered = LayeredEntryStore::new(Arc::clone(&global), Some(Arc::clone(&project)));

        // Add entries to each store
        let mut g1 = Entry::new("g-001".to_string(), "Global 1".to_string());
        g1.scope = Scope::Global;
        layered.add(&g1).unwrap();

        let mut p1 = Entry::new("p-001".to_string(), "Project 1".to_string());
        p1.scope = Scope::Project;
        layered.add(&p1).unwrap();

        // List all should return both
        let all = layered.list(ScopeFilter::All).unwrap();
        assert_eq!(all.len(), 2);

        // List global only
        let global_only = layered.list(ScopeFilter::Global).unwrap();
        assert_eq!(global_only.len(), 1);
        assert_eq!(global_only[0].id, "g-001");

        // List project only
        let project_only = layered.list(ScopeFilter::Project).unwrap();
        assert_eq!(project_only.len(), 1);
        assert_eq!(project_only[0].id, "p-001");
    }

    #[test]
    fn test_layered_rule_store() {
        let global: Arc<dyn RuleStore> = Arc::new(MockRuleStore::new());
        let project: Arc<dyn RuleStore> = Arc::new(MockRuleStore::new());
        let layered = LayeredRuleStore::new(Arc::clone(&global), Some(Arc::clone(&project)));

        // Add rules
        let mut g_rule = Rule::new("g-rule-001".to_string(), "Global rule".to_string());
        g_rule.scope = Scope::Global;
        layered.add(&g_rule).unwrap();

        let mut p_rule = Rule::new("p-rule-001".to_string(), "Project rule".to_string());
        p_rule.scope = Scope::Project;
        layered.add(&p_rule).unwrap();

        // List all
        let all = layered.list(ScopeFilter::All).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_layered_skill_store() {
        let global: Arc<dyn SkillStore> = Arc::new(MockSkillStore::new());
        let project: Arc<dyn SkillStore> = Arc::new(MockSkillStore::new());
        let layered = LayeredSkillStore::new(Arc::clone(&global), Some(Arc::clone(&project)));

        // Add skills (default scope is Global)
        let g_skill = Skill::new("g-sk01".to_string(), "Global Skill".to_string());
        layered.add(&g_skill).unwrap();

        let mut p_skill = Skill::new("p-sk01".to_string(), "Project Skill".to_string());
        p_skill.scope = Scope::Project;
        layered.add(&p_skill).unwrap();

        // List all
        let all = layered.list(None, ScopeFilter::All).unwrap();
        assert_eq!(all.len(), 2);
    }
}
