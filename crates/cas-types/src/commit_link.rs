//! Commit link for associating git commits with AI sessions
//!
//! This module tracks the relationship between git commits and AI sessions,
//! enabling "git blame for AI" functionality - tracing commits back to the
//! sessions, agents, and prompts that created them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::Scope;

/// `link_method` for a commit link the PostToolUse hook *observed* as it
/// happened (EPIC cas-6212 / cas-519f, spec §5.3).
///
/// It lives here rather than beside the rest of the `LINK_METHOD_*` vocabulary
/// in `cas-store` because `CommitLink` is the type that carries it and
/// `cas-types` cannot depend on `cas-store`. `cas_store` re-exports it, so
/// callers still see one vocabulary.
pub const LINK_METHOD_HOOK_OBSERVED: &str = "hook_observed";

/// A link between a git commit and an AI session
///
/// Tracks which session, agent, and prompts led to a commit being created.
/// This enables attribution queries like "which AI session created this commit?"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitLink {
    /// Git commit hash (full 40-char SHA) - primary key
    pub commit_hash: String,

    /// Session that created this commit
    pub session_id: String,

    /// Agent that created this commit
    pub agent_id: String,

    /// Branch the commit was made on
    pub branch: String,

    /// Commit message
    pub message: String,

    /// Files changed in this commit (relative paths)
    pub files_changed: Vec<String>,

    /// Prompt IDs that led to this commit (from file_changes)
    pub prompt_ids: Vec<String>,

    /// When the commit was made
    pub committed_at: DateTime<Utc>,

    /// Git author (from git config)
    pub author: String,

    /// Scope (project or global)
    pub scope: Scope,

    /// How this link came to exist (EPIC cas-6212 / cas-519f, spec §5.3).
    ///
    /// `hook_observed` means the PostToolUse hook *watched* the commit happen.
    /// Anything else is a link the history indexer **reconstructed** from a
    /// populated edge, and the whole point of recording it is that a
    /// reconstructed link must never be mistaken for an observed one.
    ///
    /// `None` is legacy: until M5 the hook was the only writer, so a row with
    /// no method is read as `hook_observed`.
    ///
    /// `#[serde(default)]` is load-bearing rather than decorative — `pull.rs`
    /// deserializes a `CommitLink` straight out of a cloud payload, and a
    /// required field would turn every pre-M5 remote row into a hard error.
    #[serde(default)]
    pub link_method: Option<String>,
}

impl CommitLink {
    /// Create a new commit link
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        commit_hash: String,
        session_id: String,
        agent_id: String,
        branch: String,
        message: String,
        files_changed: Vec<String>,
        prompt_ids: Vec<String>,
        author: String,
    ) -> Self {
        Self {
            commit_hash,
            session_id,
            agent_id,
            branch,
            message,
            files_changed,
            prompt_ids,
            committed_at: Utc::now(),
            author,
            scope: Scope::Project,
            // The constructor's only caller is the PostToolUse hook, which is
            // by definition an observation. A reconstructed link is built
            // through `reconstructed()` instead, so the two can never be
            // confused by forgetting to set a field.
            link_method: Some(LINK_METHOD_HOOK_OBSERVED.to_string()),
        }
    }

    /// A link the history indexer derived from a populated edge rather than
    /// observing directly (spec §5.3).
    #[allow(clippy::too_many_arguments)]
    pub fn reconstructed(
        commit_hash: String,
        session_id: String,
        agent_id: String,
        branch: String,
        message: String,
        files_changed: Vec<String>,
        committed_at: DateTime<Utc>,
        author: String,
        link_method: impl Into<String>,
    ) -> Self {
        Self {
            commit_hash,
            session_id,
            agent_id,
            branch,
            message,
            files_changed,
            // Deliberately empty: the prompt edge is not what was reconstructed.
            // Inventing prompt ids here would make "which prompt caused this
            // line" answerable-looking on evidence that never mentioned one.
            prompt_ids: Vec::new(),
            committed_at,
            author,
            scope: Scope::Project,
            link_method: Some(link_method.into()),
        }
    }

    /// Was this link observed as it happened, rather than reconstructed after
    /// the fact? Legacy rows (no method) predate any reconstructing writer, so
    /// they are observations too.
    pub fn is_observed(&self) -> bool {
        match self.link_method.as_deref() {
            None => true,
            Some(method) => method == LINK_METHOD_HOOK_OBSERVED,
        }
    }

    /// Get a short hash (first 7 characters)
    pub fn short_hash(&self) -> &str {
        if self.commit_hash.len() >= 7 {
            &self.commit_hash[..7]
        } else {
            &self.commit_hash
        }
    }

    /// Get the first line of the commit message
    pub fn message_summary(&self) -> &str {
        self.message.lines().next().unwrap_or(&self.message)
    }

    /// Get a short preview of the commit
    pub fn preview(&self, max_len: usize) -> String {
        let summary = self.message_summary();
        if summary.len() > max_len {
            crate::preview::truncate_preview(summary, max_len)
        } else {
            summary.to_string()
        }
    }

    /// Number of files changed
    pub fn file_count(&self) -> usize {
        self.files_changed.len()
    }

    /// Check if a file was changed in this commit
    pub fn includes_file(&self, file_path: &str) -> bool {
        self.files_changed.iter().any(|f| f == file_path)
    }
}

impl Default for CommitLink {
    fn default() -> Self {
        Self {
            commit_hash: String::new(),
            session_id: String::new(),
            agent_id: String::new(),
            branch: String::new(),
            message: String::new(),
            files_changed: Vec::new(),
            prompt_ids: Vec::new(),
            committed_at: Utc::now(),
            author: String::new(),
            scope: Scope::Project,
            link_method: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::commit_link::*;

    #[test]
    fn test_commit_link_new() {
        let link = CommitLink::new(
            "abc123def456789".to_string(),
            "session-1".to_string(),
            "agent-1".to_string(),
            "main".to_string(),
            "Add new feature".to_string(),
            vec!["src/main.rs".to_string(), "src/lib.rs".to_string()],
            vec!["prompt-1".to_string()],
            "John Doe <john@example.com>".to_string(),
        );

        assert_eq!(link.commit_hash, "abc123def456789");
        assert_eq!(link.branch, "main");
        assert_eq!(link.file_count(), 2);
    }

    #[test]
    fn test_short_hash() {
        let link = CommitLink {
            commit_hash: "abc123def456789".to_string(),
            ..Default::default()
        };

        assert_eq!(link.short_hash(), "abc123d");
    }

    #[test]
    fn test_message_summary() {
        let link = CommitLink {
            message: "Add new feature\n\nThis is a longer description.".to_string(),
            ..Default::default()
        };

        assert_eq!(link.message_summary(), "Add new feature");
    }

    #[test]
    fn test_preview() {
        let link = CommitLink {
            message: "This is a very long commit message that should be truncated".to_string(),
            ..Default::default()
        };

        let preview = link.preview(20);
        assert!(preview.len() <= 20);
        assert!(preview.ends_with("..."));
    }

    #[test]
    fn test_includes_file() {
        let link = CommitLink {
            files_changed: vec!["src/main.rs".to_string(), "src/lib.rs".to_string()],
            ..Default::default()
        };

        assert!(link.includes_file("src/main.rs"));
        assert!(!link.includes_file("src/other.rs"));
    }
}
