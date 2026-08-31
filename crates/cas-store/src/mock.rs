//! Mock store implementations for testing
//!
//! Provides in-memory implementations of all store traits for unit testing
//! without requiring a real database.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use chrono::Utc;

use crate::error::{Result, StoreError};
use crate::{RuleStore, SkillStore, Store};
use cas_types::{Entry, MemoryTier, Rule, RuleStatus, Scope, Skill, SkillStatus};

/// Thread-safe counter for generating unique IDs
#[derive(Debug, Default)]
struct IdCounter(RwLock<u32>);

impl IdCounter {
    fn next(&self) -> u32 {
        let mut counter = self.0.write().unwrap();
        *counter += 1;
        *counter
    }
}

// =============================================================================
// MockStore - Entry storage
// =============================================================================

/// In-memory mock implementation of the Store trait
#[derive(Debug)]
pub struct MockStore {
    entries: RwLock<HashMap<String, Entry>>,
    archived: RwLock<HashMap<String, Entry>>,
    id_counter: IdCounter,
    cas_dir: PathBuf,
}

impl Default for MockStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MockStore {
    /// Create a new empty mock store
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            archived: RwLock::new(HashMap::new()),
            id_counter: IdCounter::default(),
            cas_dir: PathBuf::from("/tmp/cas-mock"),
        }
    }

    /// Create a mock store with pre-populated test data
    pub fn with_entries(entries: Vec<Entry>) -> Self {
        let store = Self::new();
        {
            let mut map = store.entries.write().unwrap();
            for entry in entries {
                map.insert(entry.id.clone(), entry);
            }
        }
        store
    }

    /// Get the number of entries (for testing)
    pub fn len(&self) -> usize {
        self.entries.read().unwrap().len()
    }

    /// Check if store is empty (for testing)
    pub fn is_empty(&self) -> bool {
        self.entries.read().unwrap().is_empty()
    }
}

impl Store for MockStore {
    fn init(&self) -> Result<()> {
        Ok(())
    }

    fn generate_id(&self) -> Result<String> {
        let date = Utc::now().format("%Y-%m-%d");
        let counter = self.id_counter.next();
        Ok(format!("{date}-{counter:03}"))
    }

    fn add(&self, entry: &Entry) -> Result<()> {
        let mut entries = self.entries.write().unwrap();
        if entries.contains_key(&entry.id) {
            return Err(StoreError::EntryExists(entry.id.clone()));
        }
        entries.insert(entry.id.clone(), entry.clone());
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Entry> {
        let entries = self.entries.read().unwrap();
        entries
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::EntryNotFound(id.to_string()))
    }

    fn get_archived(&self, id: &str) -> Result<Entry> {
        let archived = self.archived.read().unwrap();
        archived
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::EntryNotFound(id.to_string()))
    }

    fn update(&self, entry: &Entry) -> Result<()> {
        let mut entries = self.entries.write().unwrap();
        if !entries.contains_key(&entry.id) {
            return Err(StoreError::EntryNotFound(entry.id.clone()));
        }
        entries.insert(entry.id.clone(), entry.clone());
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        let mut entries = self.entries.write().unwrap();
        entries
            .remove(id)
            .ok_or_else(|| StoreError::EntryNotFound(id.to_string()))?;
        Ok(())
    }

    fn list(&self) -> Result<Vec<Entry>> {
        let entries = self.entries.read().unwrap();
        let mut list: Vec<Entry> = entries.values().cloned().collect();
        list.sort_by(|a, b| b.created.cmp(&a.created));
        Ok(list)
    }

    fn list_by_scope_and_tag(&self, scope: Scope, tag: &str) -> Result<Vec<Entry>> {
        let entries = self.entries.read().unwrap();
        let mut list: Vec<Entry> = entries
            .values()
            .filter(|entry| entry.scope == scope)
            .filter(|entry| {
                entry
                    .tags
                    .iter()
                    .any(|entry_tag| entry_tag.trim().eq_ignore_ascii_case(tag))
            })
            .cloned()
            .collect();
        list.sort_by(|a, b| b.created.cmp(&a.created));
        Ok(list)
    }

    fn recent(&self, n: usize) -> Result<Vec<Entry>> {
        let mut list = self.list()?;
        list.truncate(n);
        Ok(list)
    }

    fn archive(&self, id: &str) -> Result<()> {
        let mut entries = self.entries.write().unwrap();
        let mut archived = self.archived.write().unwrap();
        let entry = entries
            .remove(id)
            .ok_or_else(|| StoreError::EntryNotFound(id.to_string()))?;
        archived.insert(id.to_string(), entry);
        Ok(())
    }

    fn unarchive(&self, id: &str) -> Result<()> {
        let mut entries = self.entries.write().unwrap();
        let mut archived = self.archived.write().unwrap();
        let entry = archived
            .remove(id)
            .ok_or_else(|| StoreError::EntryNotFound(id.to_string()))?;
        entries.insert(id.to_string(), entry);
        Ok(())
    }

    fn list_archived(&self) -> Result<Vec<Entry>> {
        let archived = self.archived.read().unwrap();
        let mut list: Vec<Entry> = archived.values().cloned().collect();
        list.sort_by(|a, b| b.created.cmp(&a.created));
        Ok(list)
    }

    fn list_by_branch(&self, branch: &str) -> Result<Vec<Entry>> {
        let entries = self.entries.read().unwrap();
        Ok(entries
            .values()
            .filter(|e| e.branch.as_deref() == Some(branch))
            .cloned()
            .collect())
    }

    fn list_pending(&self, limit: usize) -> Result<Vec<Entry>> {
        let entries = self.entries.read().unwrap();
        let mut list: Vec<Entry> = entries
            .values()
            .filter(|e| e.pending_extraction)
            .cloned()
            .collect();
        list.sort_by(|a, b| a.created.cmp(&b.created));
        list.truncate(limit);
        Ok(list)
    }

    fn mark_extracted(&self, id: &str) -> Result<()> {
        let mut entries = self.entries.write().unwrap();
        if let Some(entry) = entries.get_mut(id) {
            entry.pending_extraction = false;
            Ok(())
        } else {
            Err(StoreError::EntryNotFound(id.to_string()))
        }
    }

    fn list_pinned(&self) -> Result<Vec<Entry>> {
        let entries = self.entries.read().unwrap();
        let mut list: Vec<Entry> = entries
            .values()
            .filter(|e| e.memory_tier == MemoryTier::InContext)
            .cloned()
            .collect();
        list.sort_by(|a, b| b.created.cmp(&a.created));
        Ok(list)
    }

    fn list_helpful(&self, limit: usize) -> Result<Vec<Entry>> {
        let entries = self.entries.read().unwrap();
        let mut list: Vec<Entry> = entries
            .values()
            .filter(|e| e.helpful_count > e.harmful_count)
            .cloned()
            .collect();
        list.sort_by(|a, b| {
            let score_a = a.helpful_count - a.harmful_count;
            let score_b = b.helpful_count - b.harmful_count;
            score_b.cmp(&score_a)
        });
        list.truncate(limit);
        Ok(list)
    }

    fn list_by_session(&self, session_id: &str) -> Result<Vec<Entry>> {
        let entries = self.entries.read().unwrap();
        let mut list: Vec<Entry> = entries
            .values()
            .filter(|e| e.session_id.as_deref() == Some(session_id))
            .cloned()
            .collect();
        list.sort_by(|a, b| a.created.cmp(&b.created));
        Ok(list)
    }

    fn list_unreviewed_learnings(&self, limit: usize) -> Result<Vec<Entry>> {
        let entries = self.entries.read().unwrap();
        let mut list: Vec<Entry> = entries
            .values()
            .filter(|e| e.entry_type == cas_types::EntryType::Learning && e.last_reviewed.is_none())
            .cloned()
            .collect();
        list.sort_by(|a, b| b.created.cmp(&a.created));
        list.truncate(limit);
        Ok(list)
    }

    fn mark_reviewed(&self, id: &str) -> Result<()> {
        let mut entries = self.entries.write().unwrap();
        if let Some(entry) = entries.get_mut(id) {
            entry.last_reviewed = Some(Utc::now());
            Ok(())
        } else {
            Err(StoreError::EntryNotFound(id.to_string()))
        }
    }

    fn list_pending_index(&self, limit: usize) -> Result<Vec<Entry>> {
        // MockStore doesn't track updated_at/indexed_at, so return all non-archived entries
        let entries = self.entries.read().unwrap();
        let mut list: Vec<Entry> = entries.values().filter(|e| !e.archived).cloned().collect();
        list.sort_by(|a, b| b.created.cmp(&a.created));
        list.truncate(limit);
        Ok(list)
    }

    fn mark_indexed(&self, id: &str) -> Result<()> {
        // MockStore doesn't track indexed_at, so just verify entry exists
        let entries = self.entries.read().unwrap();
        if entries.contains_key(id) {
            Ok(())
        } else {
            Err(StoreError::EntryNotFound(id.to_string()))
        }
    }

    fn mark_indexed_batch(&self, ids: &[&str]) -> Result<()> {
        let entries = self.entries.read().unwrap();
        for id in ids {
            if !entries.contains_key(*id) {
                return Err(StoreError::EntryNotFound((*id).to_string()));
            }
        }
        Ok(())
    }

    fn mark_index_pending_batch(&self, ids: &[&str]) -> Result<()> {
        self.mark_indexed_batch(ids)
    }

    fn cas_dir(&self) -> &Path {
        &self.cas_dir
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}

// =============================================================================
// MockRuleStore - Rule storage
// =============================================================================

/// In-memory mock implementation of the RuleStore trait
#[derive(Debug)]
pub struct MockRuleStore {
    rules: RwLock<HashMap<String, Rule>>,
    id_counter: IdCounter,
}

impl Default for MockRuleStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MockRuleStore {
    /// Create a new empty mock rule store
    pub fn new() -> Self {
        Self {
            rules: RwLock::new(HashMap::new()),
            id_counter: IdCounter::default(),
        }
    }

    /// Create with pre-populated rules
    pub fn with_rules(rules: Vec<Rule>) -> Self {
        let store = Self::new();
        {
            let mut map = store.rules.write().unwrap();
            for rule in rules {
                map.insert(rule.id.clone(), rule);
            }
        }
        store
    }

    /// Get count (for testing)
    pub fn len(&self) -> usize {
        self.rules.read().unwrap().len()
    }

    /// Check if empty (for testing)
    pub fn is_empty(&self) -> bool {
        self.rules.read().unwrap().is_empty()
    }
}

impl RuleStore for MockRuleStore {
    fn init(&self) -> Result<()> {
        Ok(())
    }

    fn generate_id(&self) -> Result<String> {
        let counter = self.id_counter.next();
        Ok(format!("rule-{counter:03}"))
    }

    fn add(&self, rule: &Rule) -> Result<()> {
        let mut rules = self.rules.write().unwrap();
        if rules.contains_key(&rule.id) {
            return Err(StoreError::EntryExists(rule.id.clone()));
        }
        rules.insert(rule.id.clone(), rule.clone());
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Rule> {
        let rules = self.rules.read().unwrap();
        rules
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::RuleNotFound(id.to_string()))
    }

    fn update(&self, rule: &Rule) -> Result<()> {
        let mut rules = self.rules.write().unwrap();
        if !rules.contains_key(&rule.id) {
            return Err(StoreError::RuleNotFound(rule.id.clone()));
        }
        rules.insert(rule.id.clone(), rule.clone());
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        let mut rules = self.rules.write().unwrap();
        rules
            .remove(id)
            .ok_or_else(|| StoreError::RuleNotFound(id.to_string()))?;
        Ok(())
    }

    fn list(&self) -> Result<Vec<Rule>> {
        let rules = self.rules.read().unwrap();
        let mut list: Vec<Rule> = rules.values().cloned().collect();
        list.sort_by(|a, b| b.created.cmp(&a.created));
        Ok(list)
    }

    fn list_proven(&self) -> Result<Vec<Rule>> {
        let rules = self.list()?;
        Ok(rules
            .into_iter()
            .filter(|r| r.status == RuleStatus::Proven)
            .collect())
    }

    fn list_critical(&self) -> Result<Vec<Rule>> {
        let rules = self.list()?;
        Ok(rules
            .into_iter()
            .filter(|r| {
                r.priority == 0 && (r.status == RuleStatus::Proven || r.status == RuleStatus::Draft)
            })
            .collect())
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}

// =============================================================================
// MockSkillStore - Skill storage
// =============================================================================

/// In-memory mock implementation of the SkillStore trait
#[derive(Debug)]
pub struct MockSkillStore {
    skills: RwLock<HashMap<String, Skill>>,
    id_counter: IdCounter,
}

impl Default for MockSkillStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MockSkillStore {
    /// Create a new empty mock skill store
    pub fn new() -> Self {
        Self {
            skills: RwLock::new(HashMap::new()),
            id_counter: IdCounter::default(),
        }
    }

    /// Create with pre-populated skills
    pub fn with_skills(skills: Vec<Skill>) -> Self {
        let store = Self::new();
        {
            let mut map = store.skills.write().unwrap();
            for skill in skills {
                map.insert(skill.id.clone(), skill);
            }
        }
        store
    }

    /// Get count (for testing)
    pub fn len(&self) -> usize {
        self.skills.read().unwrap().len()
    }

    /// Check if empty (for testing)
    pub fn is_empty(&self) -> bool {
        self.skills.read().unwrap().is_empty()
    }
}

impl SkillStore for MockSkillStore {
    fn init(&self) -> Result<()> {
        Ok(())
    }

    fn generate_id(&self) -> Result<String> {
        let counter = self.id_counter.next();
        Ok(format!("cas-sk{counter:02x}"))
    }

    fn add(&self, skill: &Skill) -> Result<()> {
        let mut skills = self.skills.write().unwrap();
        if skills.contains_key(&skill.id) {
            return Err(StoreError::EntryExists(skill.id.clone()));
        }
        skills.insert(skill.id.clone(), skill.clone());
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Skill> {
        let skills = self.skills.read().unwrap();
        skills
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::SkillNotFound(id.to_string()))
    }

    fn update(&self, skill: &Skill) -> Result<()> {
        let mut skills = self.skills.write().unwrap();
        if !skills.contains_key(&skill.id) {
            return Err(StoreError::SkillNotFound(skill.id.clone()));
        }
        skills.insert(skill.id.clone(), skill.clone());
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        let mut skills = self.skills.write().unwrap();
        skills
            .remove(id)
            .ok_or_else(|| StoreError::SkillNotFound(id.to_string()))?;
        Ok(())
    }

    fn list(&self, status: Option<SkillStatus>) -> Result<Vec<Skill>> {
        let skills = self.skills.read().unwrap();
        let mut list: Vec<Skill> = skills
            .values()
            .filter(|s| status.is_none() || Some(s.status) == status)
            .cloned()
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    fn list_enabled(&self) -> Result<Vec<Skill>> {
        self.list(Some(SkillStatus::Enabled))
    }

    fn search(&self, query: &str) -> Result<Vec<Skill>> {
        let skills = self.skills.read().unwrap();
        let query_lower = query.to_lowercase();
        let mut list: Vec<Skill> = skills
            .values()
            .filter(|s| {
                s.name.to_lowercase().contains(&query_lower)
                    || s.description.to_lowercase().contains(&query_lower)
            })
            .cloned()
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}
