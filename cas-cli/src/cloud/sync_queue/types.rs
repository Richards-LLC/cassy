use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// Type of entity being synced
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntityType {
    Entry,
    Task,
    Rule,
    Skill,
    Session,
    Verification,
    Event,
    Prompt,
    FileChange,
    CommitLink,
    Agent,
    Worktree,
    /// A distilled project-knowledge page (T5). Local SQLite remains the
    /// source of truth; the cloud carries pages so teammates share them.
    KnowledgePage,
}

impl EntityType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EntityType::Entry => "entry",
            EntityType::Task => "task",
            EntityType::Rule => "rule",
            EntityType::Skill => "skill",
            EntityType::Session => "session",
            EntityType::Verification => "verification",
            EntityType::Event => "event",
            EntityType::Prompt => "prompt",
            EntityType::FileChange => "file_change",
            EntityType::CommitLink => "commit_link",
            EntityType::Agent => "agent",
            EntityType::Worktree => "worktree",
            EntityType::KnowledgePage => "knowledge_page",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "entry" => Some(EntityType::Entry),
            "task" => Some(EntityType::Task),
            "rule" => Some(EntityType::Rule),
            "skill" => Some(EntityType::Skill),
            "session" => Some(EntityType::Session),
            "verification" => Some(EntityType::Verification),
            "event" => Some(EntityType::Event),
            "prompt" => Some(EntityType::Prompt),
            "file_change" => Some(EntityType::FileChange),
            "commit_link" => Some(EntityType::CommitLink),
            "agent" => Some(EntityType::Agent),
            "worktree" => Some(EntityType::Worktree),
            "knowledge_page" => Some(EntityType::KnowledgePage),
            _ => None,
        }
    }

    /// Collection key used by personal/team push JSON envelopes.
    pub fn collection_key(&self) -> &'static str {
        match self {
            EntityType::Entry => "entries",
            EntityType::Task => "tasks",
            EntityType::Rule => "rules",
            EntityType::Skill => "skills",
            EntityType::Session => "sessions",
            EntityType::Verification => "verifications",
            EntityType::Event => "events",
            EntityType::Prompt => "prompts",
            EntityType::FileChange => "file_changes",
            EntityType::CommitLink => "commit_links",
            EntityType::Agent => "agents",
            EntityType::Worktree => "worktrees",
            EntityType::KnowledgePage => "knowledge_pages",
        }
    }
}

impl fmt::Display for EntityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Type of sync operation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncOperation {
    /// Create or update an entity
    Upsert,
    /// Delete an entity
    Delete,
}

impl SyncOperation {
    pub fn as_str(&self) -> &'static str {
        match self {
            SyncOperation::Upsert => "upsert",
            SyncOperation::Delete => "delete",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "upsert" => Some(SyncOperation::Upsert),
            "delete" => Some(SyncOperation::Delete),
            _ => None,
        }
    }
}

impl fmt::Display for SyncOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A queued sync operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedSync {
    /// Queue item ID
    pub id: i64,
    /// Type of entity
    pub entity_type: EntityType,
    /// Entity ID (e.g., entry ID, task ID)
    pub entity_id: String,
    /// Operation to perform
    pub operation: SyncOperation,
    /// JSON-serialized entity data (for upsert operations)
    pub payload: Option<String>,
    /// Team ID for team-scoped sync (None for personal sync)
    pub team_id: Option<String>,
    /// When the item was queued
    pub created_at: DateTime<Utc>,
    /// Number of sync attempts
    pub retry_count: i32,
    /// Last error message (if any)
    pub last_error: Option<String>,
}

/// Queue statistics
#[derive(Debug, Clone, Serialize)]
pub struct QueueStats {
    /// Total items in queue
    pub total: usize,
    /// Items pending sync (under max retries)
    pub pending: usize,
    /// Items that have failed (at max retries)
    pub failed: usize,
    /// Count by entity type
    pub by_type: HashMap<String, usize>,
    /// Oldest item timestamp
    pub oldest_item: Option<String>,
}

/// Pending items grouped by entity type
#[derive(Debug, Default)]
pub struct PendingByType {
    pub entries: Vec<QueuedSync>,
    pub tasks: Vec<QueuedSync>,
    pub rules: Vec<QueuedSync>,
    pub skills: Vec<QueuedSync>,
    pub sessions: Vec<QueuedSync>,
    pub verifications: Vec<QueuedSync>,
    pub events: Vec<QueuedSync>,
    pub prompts: Vec<QueuedSync>,
    pub file_changes: Vec<QueuedSync>,
    pub commit_links: Vec<QueuedSync>,
    pub agents: Vec<QueuedSync>,
    pub worktrees: Vec<QueuedSync>,
    pub knowledge_pages: Vec<QueuedSync>,
}

impl PendingByType {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
            && self.tasks.is_empty()
            && self.rules.is_empty()
            && self.skills.is_empty()
            && self.sessions.is_empty()
            && self.verifications.is_empty()
            && self.events.is_empty()
            && self.prompts.is_empty()
            && self.file_changes.is_empty()
            && self.commit_links.is_empty()
            && self.agents.is_empty()
            && self.worktrees.is_empty()
            && self.knowledge_pages.is_empty()
    }

    pub fn total(&self) -> usize {
        self.entries.len()
            + self.tasks.len()
            + self.rules.len()
            + self.skills.len()
            + self.sessions.len()
            + self.verifications.len()
            + self.events.len()
            + self.prompts.len()
            + self.file_changes.len()
            + self.commit_links.len()
            + self.agents.len()
            + self.worktrees.len()
    }
}
