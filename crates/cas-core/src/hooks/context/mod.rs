//! Context building for session start injection
//!
//! Builds a context string from proven rules, high-value entries,
//! ready tasks, and enabled skills to inject at session start.
//!
//! # Progressive Disclosure
//!
//! Context is built using progressive disclosure - showing summaries with
//! token estimates rather than full content. This allows Claude to decide
//! what to fetch based on relevance.
//!
//! # Architecture
//!
//! This module provides the core context building logic that can be used
//! by both CLI and MCP interfaces. Store access is abstracted via traits
//! from `cas-store`.
//!
//! # Scoring System
//!
//! Entries are scored for context selection using pluggable scorers:
//! - `BasicContextScorer` - Simple formula based on type, feedback, age, importance
//! - Hybrid scorers (provided by CLI) - Use semantic embeddings for relevance

use std::collections::{HashMap, HashSet};

use cas_store::KnowledgeStore;
use cas_store::{AgentStore, RuleStore, SkillStore, Store, TaskStore};
use cas_types::{Entry, EntryType, Rule, RuleCategory, Skill, Task};

// ============================================================================
// Context Scoring System
// ============================================================================

/// Query context for semantic scoring
///
/// Provides information about the current session to help score entries
/// by relevance to the current work context.
#[derive(Debug, Clone, Default)]
pub struct ContextQuery {
    /// Titles of in-progress tasks (most relevant context)
    pub task_titles: Vec<String>,
    /// Current working directory
    pub cwd: String,
    /// User's current prompt (if available)
    pub user_prompt: Option<String>,
    /// Recent file paths being worked on
    pub recent_files: Vec<String>,
}

impl ContextQuery {
    /// Build a search query string from the context
    pub fn to_query_string(&self) -> String {
        let mut parts = Vec::new();

        // Task titles are most important
        for title in &self.task_titles {
            parts.push(title.clone());
        }

        // User prompt if available
        if let Some(ref prompt) = self.user_prompt {
            // Take first 200 chars of prompt
            let truncated = if prompt.len() > 200 {
                let mut end = 200.min(prompt.len());
                while end > 0 && !prompt.is_char_boundary(end) {
                    end -= 1;
                }
                &prompt[..end]
            } else {
                prompt
            };
            parts.push(truncated.to_string());
        }

        // Extract project name from cwd
        if !self.cwd.is_empty() {
            if let Some(project) = std::path::Path::new(&self.cwd)
                .file_name()
                .and_then(|n| n.to_str())
            {
                parts.push(project.to_string());
            }
        }

        // Include recent files for session-aware context
        // Extract meaningful parts from file paths (module names, file names)
        for file_path in &self.recent_files {
            let path = std::path::Path::new(file_path);
            // Add file name without extension
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if !stem.is_empty() && stem != "mod" && stem != "index" {
                    parts.push(stem.to_string());
                }
            }
            // Add parent directory (module context)
            if let Some(parent) = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
            {
                if !parent.is_empty() && parent != "src" {
                    parts.push(parent.to_string());
                }
            }
        }

        parts.join(" ")
    }

    /// Check if the query has meaningful content for semantic search
    pub fn has_content(&self) -> bool {
        !self.task_titles.is_empty() || self.user_prompt.is_some() || !self.recent_files.is_empty()
    }
}

/// Trait for scoring entries for context selection
///
/// Implementations can use different strategies:
/// - BasicContextScorer: Simple formula (type × feedback × age × importance)
/// - HybridContextScorer: Semantic embeddings + BM25 + temporal (from CLI)
pub trait ContextScorer: Send + Sync {
    /// Score entries for context selection
    ///
    /// Returns entries paired with their scores (higher = more relevant).
    /// The returned vector should be sorted by score descending.
    fn score_entries(&self, entries: &[Entry], context: &ContextQuery) -> Vec<(Entry, f32)>;

    /// Name of the scorer for debugging/tracing
    fn name(&self) -> &'static str;
}

/// Basic context scorer using simple formula
///
/// Score = type_weight × feedback_boost × age_decay × importance_boost × stability_boost
///
/// This is the fallback when hybrid search infrastructure isn't available.
#[derive(Debug, Default)]
pub struct BasicContextScorer;

impl BasicContextScorer {
    /// Calculate score for a single entry
    pub fn calculate_score(entry: &Entry) -> f32 {
        // Base score from entry type
        let type_weight = match entry.entry_type {
            EntryType::Learning => 1.5,
            EntryType::Context => 1.3,
            EntryType::Preference => 1.2,
            EntryType::Observation => 0.3, // Much lower - these are raw, unprocessed
        };

        // Feedback score boost (helpful_count - harmful_count)
        let feedback_boost = 1.0 + (entry.feedback_score().max(0) as f32 * 0.3);

        // Age decay - but cap at 50% so old high-value entries still surface
        let now = chrono::Utc::now();
        let days_old: f32 = (now - entry.created).num_days().max(0) as f32;
        let age_decay: f32 = (1.0_f32 - (days_old * 0.02).min(0.5)).max(0.5);

        // Importance boost
        let importance_boost = 1.0 + (entry.importance * 0.5);

        // Stability boost (entries that have been accessed/reviewed)
        let stability_boost = 1.0 + (entry.stability * 0.2);

        // Session-aware access boost: recently accessed entries get boosted
        // This implements the "boost related items" from session-aware context
        let access_boost = if let Some(last_access) = entry.last_accessed {
            let hours_since_access = (now - last_access).num_hours().max(0) as f32;
            if hours_since_access < 1.0 {
                // Same session (< 1 hour): high boost
                1.5
            } else if hours_since_access < 24.0 {
                // Recent sessions (< 24h): medium boost with decay
                1.0 + (0.5 * (1.0 - hours_since_access / 24.0))
            } else {
                // Older: minimal boost from access_count
                1.0 + (entry.access_count.min(10) as f32 * 0.02)
            }
        } else {
            1.0
        };

        type_weight * feedback_boost * age_decay * importance_boost * stability_boost * access_boost
    }
}

impl ContextScorer for BasicContextScorer {
    fn score_entries(&self, entries: &[Entry], _context: &ContextQuery) -> Vec<(Entry, f32)> {
        let mut scored: Vec<(Entry, f32)> = entries
            .iter()
            .map(|e| (e.clone(), Self::calculate_score(e)))
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        scored
    }

    fn name(&self) -> &'static str {
        "basic"
    }
}

/// Estimate token count for a string (approximately 4 chars per token)
pub fn estimate_tokens(s: &str) -> usize {
    s.len().div_ceil(4)
}

/// Format token count for display
pub fn token_display(tokens: usize) -> String {
    if tokens < 100 {
        format!("~{tokens}tk")
    } else if tokens < 1000 {
        format!("~{}tk", (tokens / 10) * 10)
    } else {
        format!("~{:.1}k tk", tokens as f64 / 1000.0)
    }
}

/// Rewrite every `mcp__cas__<tool>` reference in `text` to use `tool_prefix`
/// instead (`mcp__cs__` for Codex, `cas__` for Grok; a no-op for Claude,
/// whose prefix already IS `mcp__cas__`).
///
/// Context/reminder builders in this module author their guidance text
/// against Claude's `mcp__cas__` prefix as the canonical baseline — it's
/// correct as-is for a Claude reader — and call this exactly once, on the
/// fully-assembled text, right before returning it. Centralizing the remap
/// here (rather than scattering per-string closures across every call site
/// that contributes to the assembled context) is deliberate: EPIC cas-8888
/// (cas-fd9f) found that the old per-site approach silently missed several
/// contributors (`coordination::render_factory_coordination`'s Codex-worker
/// note, `plan_mode`'s planning-tools hints) that fed into the same
/// `context_parts` buffer without ever passing through the local remap
/// closure. A single end-of-function pass can't be skipped by a new
/// contributor the way a per-site convention can.
pub(crate) fn remap_tool_prefix(text: &str, tool_prefix: &str) -> String {
    if tool_prefix == "mcp__cas__" {
        text.to_string()
    } else {
        text.replace("mcp__cas__", tool_prefix)
    }
}

/// Truncate a string to a maximum length
pub fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len.saturating_sub(3).min(s.len());
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

/// Format rule category as a short badge for display
pub(crate) fn format_category_badge(category: RuleCategory) -> &'static str {
    match category {
        RuleCategory::Security => "🔒sec",
        RuleCategory::Performance => "⚡perf",
        RuleCategory::Convention => "📝conv",
        RuleCategory::Architecture => "🏗arch",
        RuleCategory::ErrorHandling => "⚠️err",
        RuleCategory::General => "gen",
    }
}

// calculate_entry_score moved to BasicContextScorer::calculate_score

/// Check if a rule's path pattern matches the current directory
pub fn rule_matches_path(rule: &Rule, cwd: &str) -> bool {
    if rule.paths.is_empty() {
        return true; // No path restriction
    }

    // Use proper glob pattern matching
    for pattern in rule.paths.split(',') {
        let pattern = pattern.trim();
        if pattern.is_empty() || pattern == "**" || pattern == "*" {
            return true;
        }

        // Try glob pattern matching first
        if let Ok(glob_pattern) = glob::Pattern::new(pattern) {
            if glob_pattern.matches(cwd) {
                return true;
            }
            // Also check if cwd is within the pattern's directory
            let pattern_base = pattern.trim_end_matches("/**").trim_end_matches("/*");
            if cwd.ends_with(pattern_base) || cwd.contains(&format!("/{pattern_base}/")) {
                return true;
            }
        }

        // Fallback: check if the pattern's core directory is in the path
        let core_pattern = pattern
            .trim_start_matches("**/")
            .trim_end_matches("/**")
            .trim_end_matches("/*");
        if !core_pattern.is_empty() && cwd.contains(core_pattern) {
            return true;
        }
    }

    false
}

/// Cache for rule path matches to avoid repeated glob parsing
///
/// Pre-computes which rules match a given cwd. Reusable across multiple
/// context builds in the same session (same cwd = same matches).
#[derive(Debug, Default)]
pub struct RuleMatchCache {
    /// Current working directory this cache is valid for
    cwd: String,
    /// Map of rule_id -> matches_path
    matches: HashMap<String, bool>,
}

impl RuleMatchCache {
    /// Create a new empty cache
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a cache for all rules against a given cwd
    pub fn build(rules: &[Rule], cwd: &str) -> Self {
        let mut matches = HashMap::new();
        for rule in rules {
            let matched = rule_matches_path(rule, cwd);
            matches.insert(rule.id.clone(), matched);
        }
        Self {
            cwd: cwd.to_string(),
            matches,
        }
    }

    /// Check if a rule matches (uses cached result if available)
    pub fn matches(&self, rule: &Rule, cwd: &str) -> bool {
        // If cwd changed, fall back to direct matching
        if self.cwd != cwd {
            return rule_matches_path(rule, cwd);
        }

        // Use cached result or compute on-demand
        self.matches
            .get(&rule.id)
            .copied()
            .unwrap_or_else(|| rule_matches_path(rule, cwd))
    }

    /// Check if the cache is valid for a given cwd
    pub fn is_valid_for(&self, cwd: &str) -> bool {
        self.cwd == cwd
    }

    /// Get the cwd this cache was built for
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Number of cached entries
    pub fn len(&self) -> usize {
        self.matches.len()
    }

    /// Check if cache is empty
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }
}

/// Context item with token estimate for progressive disclosure
#[derive(Debug)]
pub struct ContextItem {
    pub id: String,
    pub summary: String,
    pub tokens: usize,
    pub item_type: ContextItemType,
}

#[derive(Debug, Clone, Copy)]
pub enum ContextItemType {
    Task,
    Rule,
    Skill,
    Memory,
}

impl ContextItem {
    pub fn from_task(task: &Task) -> Self {
        let content = format!("{}: {}", task.id, task.title);
        Self {
            id: task.id.clone(),
            summary: task.preview(60),
            tokens: estimate_tokens(&content),
            item_type: ContextItemType::Task,
        }
    }

    pub fn from_rule(rule: &Rule) -> Self {
        Self {
            id: rule.id.clone(),
            summary: truncate(&rule.content, 60),
            tokens: estimate_tokens(&rule.content),
            item_type: ContextItemType::Rule,
        }
    }

    pub fn from_skill(skill: &Skill) -> Self {
        let full_content = format!("{}\n{}", skill.description, skill.invocation);
        Self {
            id: skill.id.clone(),
            summary: truncate(&skill.description, 50),
            tokens: estimate_tokens(&full_content),
            item_type: ContextItemType::Skill,
        }
    }

    pub fn from_entry(entry: &Entry) -> Self {
        Self {
            id: entry.id.clone(),
            summary: entry.preview(60),
            tokens: estimate_tokens(&entry.content),
            item_type: ContextItemType::Memory,
        }
    }
}

/// CAS usage reminder for MCP tools
pub(crate) const USAGE_REMINDER: &str = r#"## 📋 CAS Context

**Use CAS MCP tools for task/memory management (NOT built-in TodoWrite):**
- `mcp__cas__task` - Track work
- `mcp__cas__memory` - Store learnings
- `mcp__cas__search` - Find context

<IMPORTANT>
**When to use `memory` with action: remember (PROACTIVELY store learnings):**
- After discovering project-specific patterns or conventions
- After fixing non-trivial bugs (capture root cause + solution)
- After learning how unfamiliar code works
- When you find important architectural decisions
- After resolving configuration or setup issues

Don't wait to be asked - if you learned something valuable, store it immediately.
</IMPORTANT>

**Search guidance:**
- For exploratory searches ("where is X handled?", "how does Y work?"), prefer `mcp__cas__search` - it combines code + tasks + memories with semantic understanding
- For exact pattern matching (specific regex, literal strings), use Grep directly
"#;

/// Stores required for context building
pub struct ContextStores<'a> {
    /// Global entry store (optional)
    pub global_store: Option<&'a dyn Store>,
    /// Project entry store (optional)
    pub project_store: Option<&'a dyn Store>,
    /// Global rule store (optional)
    pub global_rule_store: Option<&'a dyn RuleStore>,
    /// Project rule store (optional)
    pub project_rule_store: Option<&'a dyn RuleStore>,
    /// Task store (project only)
    pub task_store: Option<&'a dyn TaskStore>,
    /// Agent store (project only)
    pub agent_store: Option<&'a dyn AgentStore>,
    /// Global skill store (optional)
    pub global_skill_store: Option<&'a dyn SkillStore>,
    /// Project skill store (optional)
    pub project_skill_store: Option<&'a dyn SkillStore>,
    /// Distilled project-knowledge store (optional)
    ///
    /// Present means SessionStart injects the knowledge *index* (title +
    /// snippet + id per page) and an instruction to pull bodies on demand.
    /// Bodies themselves never enter the injected block.
    pub knowledge_store: Option<&'a dyn KnowledgeStore>,
    /// Entry scorer for context selection (optional, defaults to BasicContextScorer)
    pub entry_scorer: Option<&'a dyn ContextScorer>,
    /// Rule match cache for avoiding repeated glob parsing (optional)
    pub rule_match_cache: Option<&'a RuleMatchCache>,
    /// Recent files from session (for context query boosting)
    pub recent_files: Vec<String>,
}

impl<'a> ContextStores<'a> {
    /// Create empty stores (for testing)
    pub fn empty() -> Self {
        Self {
            global_store: None,
            project_store: None,
            global_rule_store: None,
            project_rule_store: None,
            task_store: None,
            agent_store: None,
            global_skill_store: None,
            project_skill_store: None,
            knowledge_store: None,
            entry_scorer: None,
            rule_match_cache: None,
            recent_files: Vec::new(),
        }
    }

    /// Get primary store (project preferred, then global)
    pub fn primary_store(&self) -> Option<&'a dyn Store> {
        self.project_store.or(self.global_store)
    }

    /// Get primary rule store (project preferred, then global)
    pub fn primary_rule_store(&self) -> Option<&'a dyn RuleStore> {
        self.project_rule_store.or(self.global_rule_store)
    }
}

/// Merge entries from both global and project stores
pub(crate) fn merge_entries(stores: &ContextStores) -> Vec<Entry> {
    let mut seen_ids = HashSet::new();
    let mut entries = Vec::new();

    // Project entries first (higher priority)
    if let Some(store) = stores.project_store {
        if let Ok(project_entries) = store.list() {
            for entry in project_entries {
                let base_id = entry.id.trim_start_matches("p-").trim_start_matches("g-");
                seen_ids.insert(base_id.to_string());
                entries.push(entry);
            }
        }
    }

    // Then global entries (skip duplicates)
    if let Some(store) = stores.global_store {
        if let Ok(global_entries) = store.list() {
            for entry in global_entries {
                let base_id = entry.id.trim_start_matches("p-").trim_start_matches("g-");
                if !seen_ids.contains(base_id) {
                    seen_ids.insert(base_id.to_string());
                    entries.push(entry);
                }
            }
        }
    }

    entries
}

/// Merge rules from both global and project stores
pub(crate) fn merge_rules(stores: &ContextStores) -> Vec<Rule> {
    let mut seen_ids = HashSet::new();
    let mut rules = Vec::new();

    // Project rules first (higher priority)
    if let Some(store) = stores.project_rule_store {
        if let Ok(project_rules) = store.list() {
            for rule in project_rules {
                let base_id = rule.id.trim_start_matches("p-").trim_start_matches("g-");
                seen_ids.insert(base_id.to_string());
                rules.push(rule);
            }
        }
    }

    // Then global rules (skip duplicates)
    if let Some(store) = stores.global_rule_store {
        if let Ok(global_rules) = store.list() {
            for rule in global_rules {
                let base_id = rule.id.trim_start_matches("p-").trim_start_matches("g-");
                if !seen_ids.contains(base_id) {
                    seen_ids.insert(base_id.to_string());
                    rules.push(rule);
                }
            }
        }
    }

    rules
}

/// Merge skills from both global and project stores
pub(crate) fn merge_skills(stores: &ContextStores) -> Vec<Skill> {
    let mut seen_ids = HashSet::new();
    let mut skills = Vec::new();

    // Global skills first (skills default to global scope)
    if let Some(store) = stores.global_skill_store {
        if let Ok(global_skills) = store.list(None) {
            for skill in global_skills {
                let base_id = skill.id.trim_start_matches("p-").trim_start_matches("g-");
                seen_ids.insert(base_id.to_string());
                skills.push(skill);
            }
        }
    }

    // Then project skills (skip duplicates, project can override)
    if let Some(store) = stores.project_skill_store {
        if let Ok(project_skills) = store.list(None) {
            for skill in project_skills {
                let base_id = skill.id.trim_start_matches("p-").trim_start_matches("g-");
                if !seen_ids.contains(base_id) {
                    seen_ids.insert(base_id.to_string());
                    skills.push(skill);
                }
            }
        }
    }

    skills
}

/// Context building statistics (for tracing/metrics)
#[derive(Debug, Default)]
pub struct ContextStats {
    pub tasks_included: usize,
    pub rules_included: usize,
    pub skills_included: usize,
    pub memories_included: usize,
    pub pinned_included: usize,
    pub total_tokens: usize,
    pub items_omitted: usize,
}

/// Optional callback for surfaced items (for feedback tracking)
pub type SurfacedItemCallback = Box<dyn Fn(&str, &str, Option<&str>)>;

mod build_start;
mod coordination;
mod plan_mode;

#[cfg(test)]
mod tests;

pub use build_start::{KNOWLEDGE_PULL_INSTRUCTION, build_context_with_stores};
pub(crate) use coordination::{render_factory_coordination, render_normal_coordination};
pub use plan_mode::build_plan_context_with_stores;

#[cfg(test)]
pub(crate) use coordination::is_factory_participant;
