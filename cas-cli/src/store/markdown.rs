//! Legacy Markdown storage backend
//!
//! Supports the original file-based storage format with YAML frontmatter.

use chrono::{DateTime, Utc};
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::MemError;
use crate::store::{Result, RuleStore, Store};
use crate::types::{Entry, EntryType, Rule, RuleStatus, Scope};

/// Markdown-based entry store (legacy format)
pub struct MarkdownStore {
    cas_dir: PathBuf,
    entries_dir: PathBuf,
    archive_dir: PathBuf,
}

impl MarkdownStore {
    /// Open a markdown store
    pub fn open(cas_dir: &Path) -> Result<Self> {
        Ok(Self {
            cas_dir: cas_dir.to_path_buf(),
            entries_dir: cas_dir.join("entries"),
            archive_dir: cas_dir.join("archive"),
        })
    }

    fn read_entry(&self, path: &Path) -> Result<Entry> {
        let content = fs::read_to_string(path)?;
        Self::parse_entry(&content)
    }

    fn parse_entry(content: &str) -> Result<Entry> {
        // Split frontmatter and content
        let parts: Vec<&str> = content.splitn(3, "---").collect();
        if parts.len() < 3 {
            return Err(MemError::Parse("Invalid markdown format".to_string()));
        }

        let frontmatter = parts[1].trim();
        let body = parts[2].trim();

        // Parse YAML frontmatter
        #[derive(serde::Deserialize)]
        struct Frontmatter {
            id: String,
            #[serde(rename = "type", default)]
            entry_type: Option<String>,
            #[serde(default)]
            tags: Vec<String>,
            created: DateTime<Utc>,
            #[serde(default)]
            helpful_count: i32,
            #[serde(default)]
            harmful_count: i32,
            #[serde(default)]
            last_accessed: Option<DateTime<Utc>>,
            #[serde(default)]
            title: Option<String>,
        }

        let fm: Frontmatter = serde_yaml::from_str(frontmatter)?;

        Ok(Entry {
            id: fm.id,
            scope: Scope::default(),
            entry_type: fm
                .entry_type
                .and_then(|t| t.parse().ok())
                .unwrap_or(EntryType::Learning),
            observation_type: None,
            tags: fm.tags,
            created: fm.created,
            content: body.to_string(),
            raw_content: None,
            compressed: false,
            memory_tier: Default::default(),
            title: fm.title,
            helpful_count: fm.helpful_count,
            harmful_count: fm.harmful_count,
            last_accessed: fm.last_accessed,
            archived: false,
            session_id: None,
            source_tool: None,
            pending_extraction: false,
            pending_embedding: true, // Default true for entries from markdown store
            stability: 0.5,
            access_count: 0,
            importance: 0.5,
            valid_from: None,
            valid_until: None,
            review_after: None,
            last_reviewed: None,
            domain: None,
            belief_type: Default::default(),
            confidence: 1.0,
            branch: None,
            team_id: None,
        })
    }

    fn write_entry(&self, entry: &Entry, dir: &Path) -> Result<()> {
        fs::create_dir_all(dir)?;

        let frontmatter = format!(
            "---\n\
             id: {}\n\
             type: {}\n\
             tags: [{}]\n\
             created: {}\n\
             helpful_count: {}\n\
             harmful_count: {}\n\
             {}{}\
             ---\n\n\
             {}",
            entry.id,
            entry.entry_type,
            entry.tags.join(", "),
            entry.created.to_rfc3339(),
            entry.helpful_count,
            entry.harmful_count,
            entry
                .last_accessed
                .map(|t| format!("last_accessed: {}\n", t.to_rfc3339()))
                .unwrap_or_default(),
            entry
                .title
                .as_ref()
                .map(|t| format!("title: {}\n", t))
                .unwrap_or_default(),
            entry.content
        );

        let path = dir.join(format!("{}.md", entry.id));
        fs::write(path, frontmatter)?;
        Ok(())
    }

    fn list_dir(&self, dir: &Path) -> Result<Vec<Entry>> {
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "md").unwrap_or(false) {
                if let Ok(e) = self.read_entry(&path) {
                    entries.push(e);
                }
            }
        }

        entries.sort_by(|a, b| b.created.cmp(&a.created));
        Ok(entries)
    }
}

impl Store for MarkdownStore {
    fn init(&self) -> Result<()> {
        fs::create_dir_all(&self.entries_dir)?;
        fs::create_dir_all(&self.archive_dir)?;
        Ok(())
    }

    fn generate_id(&self) -> Result<String> {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let pattern = format!("{}-", today);

        let mut max = 0;
        if self.entries_dir.exists() {
            for entry in fs::read_dir(&self.entries_dir)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&pattern) {
                    if let Some(num_str) = name
                        .strip_prefix(&pattern)
                        .and_then(|s| s.strip_suffix(".md"))
                    {
                        if let Ok(num) = num_str.parse::<i32>() {
                            max = max.max(num);
                        }
                    }
                }
            }
        }

        Ok(format!("{}-{:03}", today, max + 1))
    }

    fn add(&self, entry: &Entry) -> Result<()> {
        self.write_entry(entry, &self.entries_dir)
    }

    fn get(&self, id: &str) -> Result<Entry> {
        let path = self.entries_dir.join(format!("{}.md", id));
        if !path.exists() {
            return Err(MemError::EntryNotFound(id.to_string()));
        }
        let mut entry = self.read_entry(&path)?;
        entry.archived = false;
        Ok(entry)
    }

    fn get_archived(&self, id: &str) -> Result<Entry> {
        let path = self.archive_dir.join(format!("{}.md", id));
        if !path.exists() {
            return Err(MemError::EntryNotFound(id.to_string()));
        }
        let mut entry = self.read_entry(&path)?;
        entry.archived = true;
        Ok(entry)
    }

    fn update(&self, entry: &Entry) -> Result<()> {
        let dir = if entry.archived {
            &self.archive_dir
        } else {
            &self.entries_dir
        };
        let path = dir.join(format!("{}.md", entry.id));
        if !path.exists() {
            return Err(MemError::EntryNotFound(entry.id.clone()));
        }
        self.write_entry(entry, dir)
    }

    fn delete(&self, id: &str) -> Result<()> {
        let path = self.entries_dir.join(format!("{}.md", id));
        if path.exists() {
            fs::remove_file(path)?;
            return Ok(());
        }

        let archive_path = self.archive_dir.join(format!("{}.md", id));
        if archive_path.exists() {
            fs::remove_file(archive_path)?;
            return Ok(());
        }

        Err(MemError::EntryNotFound(id.to_string()))
    }

    fn list(&self) -> Result<Vec<Entry>> {
        self.list_dir(&self.entries_dir)
    }

    fn recent(&self, n: usize) -> Result<Vec<Entry>> {
        let mut entries = self.list()?;
        entries.truncate(n);
        Ok(entries)
    }

    fn archive(&self, id: &str) -> Result<()> {
        let src = self.entries_dir.join(format!("{}.md", id));
        if !src.exists() {
            return Err(MemError::EntryNotFound(id.to_string()));
        }

        fs::create_dir_all(&self.archive_dir)?;
        let dst = self.archive_dir.join(format!("{}.md", id));
        fs::rename(src, dst)?;
        Ok(())
    }

    fn unarchive(&self, id: &str) -> Result<()> {
        let src = self.archive_dir.join(format!("{}.md", id));
        if !src.exists() {
            return Err(MemError::EntryNotFound(id.to_string()));
        }

        let dst = self.entries_dir.join(format!("{}.md", id));
        fs::rename(src, dst)?;
        Ok(())
    }

    fn list_archived(&self) -> Result<Vec<Entry>> {
        let mut entries = self.list_dir(&self.archive_dir)?;
        for entry in &mut entries {
            entry.archived = true;
        }
        Ok(entries)
    }

    fn list_by_branch(&self, branch: &str) -> Result<Vec<Entry>> {
        let entries = self.list()?;
        Ok(entries
            .into_iter()
            .filter(|e| e.branch.as_deref() == Some(branch))
            .collect())
    }

    fn list_pending(&self, _limit: usize) -> Result<Vec<Entry>> {
        // Markdown store doesn't support pending extraction tracking
        Ok(vec![])
    }

    fn mark_extracted(&self, _id: &str) -> Result<()> {
        // No-op for markdown store
        Ok(())
    }

    fn list_pinned(&self) -> Result<Vec<Entry>> {
        // Markdown store doesn't support memory tier filtering
        // Return empty list for compatibility
        Ok(vec![])
    }

    fn list_helpful(&self, limit: usize) -> Result<Vec<Entry>> {
        // Filter entries by positive feedback score
        let mut entries: Vec<Entry> = self
            .list()?
            .into_iter()
            .filter(|e| e.feedback_score() > 0)
            .collect();

        // Sort by score descending, then by created descending
        entries.sort_by(|a, b| {
            b.feedback_score()
                .cmp(&a.feedback_score())
                .then_with(|| b.created.cmp(&a.created))
        });

        entries.truncate(limit);
        Ok(entries)
    }

    fn list_by_session(&self, session_id: &str) -> Result<Vec<Entry>> {
        // Filter list() by session_id - markdown store doesn't have indexed queries
        Ok(self
            .list()?
            .into_iter()
            .filter(|e| e.session_id.as_deref() == Some(session_id))
            .collect())
    }

    fn list_unreviewed_learnings(&self, limit: usize) -> Result<Vec<Entry>> {
        // Filter list() for unreviewed learnings
        Ok(self
            .list()?
            .into_iter()
            .filter(|e| e.entry_type == EntryType::Learning && e.last_reviewed.is_none())
            .take(limit)
            .collect())
    }

    fn mark_reviewed(&self, id: &str) -> Result<()> {
        let mut entry = self.get(id)?;
        entry.last_reviewed = Some(chrono::Utc::now());
        self.update(&entry)
    }

    fn list_pending_index(&self, limit: usize) -> Result<Vec<Entry>> {
        // MarkdownStore doesn't track updated_at/indexed_at, return all entries
        Ok(self.list()?.into_iter().take(limit).collect())
    }

    fn mark_indexed(&self, _id: &str) -> Result<()> {
        // MarkdownStore doesn't persist indexed_at
        Ok(())
    }

    fn mark_indexed_batch(&self, _ids: &[&str]) -> Result<()> {
        // MarkdownStore doesn't persist indexed_at
        Ok(())
    }

    fn cas_dir(&self) -> &Path {
        &self.cas_dir
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}

/// Markdown-based rule store (legacy format)
pub struct MarkdownRuleStore {
    rules_dir: PathBuf,
}

impl MarkdownRuleStore {
    /// Open a markdown rule store
    pub fn open(cas_dir: &Path) -> Result<Self> {
        Ok(Self {
            rules_dir: cas_dir.join("rules"),
        })
    }

    fn read_rule(&self, path: &Path) -> Result<Rule> {
        let content = fs::read_to_string(path)?;
        Self::parse_rule(&content)
    }

    fn parse_rule(content: &str) -> Result<Rule> {
        let parts: Vec<&str> = content.splitn(3, "---").collect();
        if parts.len() < 3 {
            return Err(MemError::Parse("Invalid rule format".to_string()));
        }

        let frontmatter = parts[1].trim();
        let body = parts[2].trim();

        #[derive(serde::Deserialize)]
        struct Frontmatter {
            id: String,
            created: DateTime<Utc>,
            #[serde(default)]
            source_ids: Vec<String>,
            #[serde(default)]
            helpful_count: i32,
            #[serde(default)]
            harmful_count: i32,
            #[serde(default)]
            tags: Vec<String>,
            #[serde(default)]
            paths: String,
            #[serde(default)]
            status: Option<String>,
            #[serde(default)]
            last_accessed: Option<DateTime<Utc>>,
            #[serde(default)]
            review_after: Option<DateTime<Utc>>,
            #[serde(default)]
            hook_command: Option<String>,
            #[serde(default)]
            category: Option<String>,
            #[serde(default)]
            priority: Option<u8>,
            #[serde(default)]
            surface_count: Option<i32>,
        }

        let fm: Frontmatter = serde_yaml::from_str(frontmatter)?;

        Ok(Rule {
            id: fm.id,
            scope: Scope::default(),
            created: fm.created,
            source_ids: fm.source_ids,
            content: body.to_string(),
            status: fm
                .status
                .and_then(|s| s.parse().ok())
                .unwrap_or(RuleStatus::Draft),
            helpful_count: fm.helpful_count,
            harmful_count: fm.harmful_count,
            tags: fm.tags,
            paths: fm.paths,
            last_accessed: fm.last_accessed,
            review_after: fm.review_after,
            hook_command: fm.hook_command,
            category: fm.category.and_then(|s| s.parse().ok()).unwrap_or_default(),
            priority: fm.priority.unwrap_or(2),
            surface_count: fm.surface_count.unwrap_or(0),
        })
    }

    fn write_rule(&self, rule: &Rule) -> Result<()> {
        fs::create_dir_all(&self.rules_dir)?;

        let frontmatter = format!(
            "---\n\
             id: {}\n\
             created: {}\n\
             source_ids: [{}]\n\
             status: {}\n\
             category: {}\n\
             priority: {}\n\
             helpful_count: {}\n\
             harmful_count: {}\n\
             tags: [{}]\n\
             paths: \"{}\"\n\
             {}{}{}\
             ---\n\n\
             {}",
            rule.id,
            rule.created.to_rfc3339(),
            rule.source_ids.join(", "),
            rule.status,
            rule.category,
            rule.priority,
            rule.helpful_count,
            rule.harmful_count,
            rule.tags.join(", "),
            rule.paths,
            rule.last_accessed
                .map(|t| format!("last_accessed: {}\n", t.to_rfc3339()))
                .unwrap_or_default(),
            rule.review_after
                .map(|t| format!("review_after: {}\n", t.to_rfc3339()))
                .unwrap_or_default(),
            rule.hook_command
                .as_ref()
                .map(|c| format!("hook_command: \"{}\"\n", c))
                .unwrap_or_default(),
            rule.content
        );

        let path = self.rules_dir.join(format!("{}.md", rule.id));
        fs::write(path, frontmatter)?;
        Ok(())
    }
}

impl RuleStore for MarkdownRuleStore {
    fn init(&self) -> Result<()> {
        fs::create_dir_all(&self.rules_dir)?;
        Ok(())
    }

    fn generate_id(&self) -> Result<String> {
        let mut max = 0;
        if self.rules_dir.exists() {
            for entry in fs::read_dir(&self.rules_dir)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(num_str) = name
                    .strip_prefix("rule-")
                    .and_then(|s| s.strip_suffix(".md"))
                {
                    if let Ok(num) = num_str.parse::<i32>() {
                        max = max.max(num);
                    }
                }
            }
        }
        Ok(format!("rule-{:03}", max + 1))
    }

    fn add(&self, rule: &Rule) -> Result<()> {
        self.write_rule(rule)
    }

    fn get(&self, id: &str) -> Result<Rule> {
        let path = self.rules_dir.join(format!("{}.md", id));
        if !path.exists() {
            return Err(MemError::RuleNotFound(id.to_string()));
        }
        self.read_rule(&path)
    }

    fn update(&self, rule: &Rule) -> Result<()> {
        let path = self.rules_dir.join(format!("{}.md", rule.id));
        if !path.exists() {
            return Err(MemError::RuleNotFound(rule.id.clone()));
        }
        self.write_rule(rule)
    }

    fn delete(&self, id: &str) -> Result<()> {
        let path = self.rules_dir.join(format!("{}.md", id));
        if !path.exists() {
            return Err(MemError::RuleNotFound(id.to_string()));
        }
        fs::remove_file(path)?;
        Ok(())
    }

    fn list(&self) -> Result<Vec<Rule>> {
        if !self.rules_dir.exists() {
            return Ok(Vec::new());
        }

        let mut rules = Vec::new();
        for entry in fs::read_dir(&self.rules_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "md").unwrap_or(false) {
                if let Ok(r) = self.read_rule(&path) {
                    rules.push(r);
                }
            }
        }

        rules.sort_by(|a, b| b.created.cmp(&a.created));
        Ok(rules)
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}
