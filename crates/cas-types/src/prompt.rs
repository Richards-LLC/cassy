//! Prompt type for tracking user prompts in AI sessions
//!
//! Captures the full text of prompts sent to AI agents, enabling
//! attribution of code changes back to the prompts that triggered them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::scope::Scope;

/// Role of a message in a conversation
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// User/human message
    #[default]
    User,
    /// Assistant response
    Assistant,
    /// Tool use by the assistant
    ToolUse,
}

/// A single message in a conversation transcript
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Role of this message
    pub role: MessageRole,
    /// Content of the message
    pub content: String,
    /// Tool name (for ToolUse messages)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Tool input (for ToolUse messages)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<serde_json::Value>,
    /// When this message was sent
    pub timestamp: DateTime<Utc>,
}

impl Message {
    /// Create a user message
    pub fn user(content: String) -> Self {
        Self {
            role: MessageRole::User,
            content,
            tool_name: None,
            tool_input: None,
            timestamp: Utc::now(),
        }
    }

    /// Create an assistant message
    pub fn assistant(content: String) -> Self {
        Self {
            role: MessageRole::Assistant,
            content,
            tool_name: None,
            tool_input: None,
            timestamp: Utc::now(),
        }
    }

    /// Create a tool use message
    pub fn tool_use(tool_name: String, tool_input: serde_json::Value, summary: String) -> Self {
        Self {
            role: MessageRole::ToolUse,
            content: summary,
            tool_name: Some(tool_name),
            tool_input: Some(tool_input),
            timestamp: Utc::now(),
        }
    }
}

/// Information about the AI agent that processed the prompt
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Tool name (e.g., "claude-code")
    pub tool: String,
    /// Model ID (e.g., "claude-sonnet-4-20250514")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Tool version (e.g., "1.0.34")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// A captured user prompt from a Claude Code session
///
/// Prompts are captured via the UserPromptSubmit hook and linked to
/// file changes made in response to them. This enables "git blame"
/// style attribution: given any line of code, trace it back to the
/// prompt that requested its creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    /// Unique identifier (prompt-{ulid})
    pub id: String,

    /// Session ID where this prompt was submitted
    pub session_id: String,

    /// Agent ID that received the prompt
    pub agent_id: String,

    /// Full prompt text as submitted by the user
    pub content: String,

    /// SHA-256 hash of content for deduplication
    pub content_hash: String,

    /// When the prompt was submitted
    pub timestamp: DateTime<Utc>,

    /// When Claude started responding (if tracked)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_started: Option<DateTime<Utc>>,

    /// Task ID being worked on when prompt was submitted (if any)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,

    /// Storage scope (always project for prompts)
    #[serde(default)]
    pub scope: Scope,

    // Blame v2 fields
    /// Full conversation transcript (messages array)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<Message>,

    /// Model that processed this prompt (e.g., "claude-sonnet-4-20250514")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Claude Code version used (e.g., "1.0.34")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_version: Option<String>,
}

impl Prompt {
    /// Create a new prompt with the given content
    pub fn new(id: String, session_id: String, agent_id: String, content: String) -> Self {
        let content_hash = compute_content_hash(&content);

        Self {
            id,
            session_id,
            agent_id,
            content,
            content_hash,
            timestamp: Utc::now(),
            response_started: None,
            task_id: None,
            scope: Scope::Project,
            messages: Vec::new(),
            model: None,
            tool_version: None,
        }
    }

    /// Create a new prompt with all fields specified
    pub fn with_task(
        id: String,
        session_id: String,
        agent_id: String,
        content: String,
        task_id: Option<String>,
    ) -> Self {
        let content_hash = compute_content_hash(&content);

        Self {
            id,
            session_id,
            agent_id,
            content,
            content_hash,
            timestamp: Utc::now(),
            response_started: None,
            task_id,
            scope: Scope::Project,
            messages: Vec::new(),
            model: None,
            tool_version: None,
        }
    }

    /// Create a new prompt with model info
    pub fn with_model_info(
        id: String,
        session_id: String,
        agent_id: String,
        content: String,
        model: Option<String>,
        tool_version: Option<String>,
    ) -> Self {
        let content_hash = compute_content_hash(&content);

        Self {
            id,
            session_id,
            agent_id,
            content,
            content_hash,
            timestamp: Utc::now(),
            response_started: None,
            task_id: None,
            scope: Scope::Project,
            messages: Vec::new(),
            model,
            tool_version,
        }
    }

    /// Add a message to the conversation transcript
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Add a user message to the transcript
    pub fn add_user_message(&mut self, content: String) {
        self.messages.push(Message::user(content));
    }

    /// Add an assistant message to the transcript
    pub fn add_assistant_message(&mut self, content: String) {
        self.messages.push(Message::assistant(content));
    }

    /// Add a tool use to the transcript
    pub fn add_tool_use(
        &mut self,
        tool_name: String,
        tool_input: serde_json::Value,
        summary: String,
    ) {
        self.messages
            .push(Message::tool_use(tool_name, tool_input, summary));
    }

    /// Get a preview of the prompt content (first N characters)
    pub fn preview(&self, max_len: usize) -> String {
        crate::preview::truncate_preview(&self.content, max_len)
    }

    /// Mark when response started
    pub fn mark_response_started(&mut self) {
        self.response_started = Some(Utc::now());
    }
}

impl Default for Prompt {
    fn default() -> Self {
        Self {
            id: String::new(),
            session_id: String::new(),
            agent_id: String::new(),
            content: String::new(),
            content_hash: String::new(),
            timestamp: Utc::now(),
            response_started: None,
            task_id: None,
            scope: Scope::Project,
            messages: Vec::new(),
            model: None,
            tool_version: None,
        }
    }
}

/// Compute SHA-256 hash of content for deduplication
fn compute_content_hash(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use crate::prompt::*;

    #[test]
    fn test_prompt_creation() {
        let prompt = Prompt::new(
            "prompt-abc123".to_string(),
            "session-xyz".to_string(),
            "agent-1".to_string(),
            "Add a login button to the header".to_string(),
        );

        assert_eq!(prompt.id, "prompt-abc123");
        assert_eq!(prompt.session_id, "session-xyz");
        assert_eq!(prompt.agent_id, "agent-1");
        assert!(!prompt.content_hash.is_empty());
        assert_eq!(prompt.scope, Scope::Project);
    }

    #[test]
    fn test_prompt_with_task() {
        let prompt = Prompt::with_task(
            "prompt-abc123".to_string(),
            "session-xyz".to_string(),
            "agent-1".to_string(),
            "Fix the bug".to_string(),
            Some("cas-1234".to_string()),
        );

        assert_eq!(prompt.task_id, Some("cas-1234".to_string()));
    }

    #[test]
    fn test_content_hash_deterministic() {
        let content = "Test prompt content";
        let hash1 = compute_content_hash(content);
        let hash2 = compute_content_hash(content);

        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA-256 produces 64 hex chars
    }

    #[test]
    fn test_mark_response_started() {
        let mut prompt = Prompt::new(
            "prompt-1".to_string(),
            "session-1".to_string(),
            "agent-1".to_string(),
            "Test prompt".to_string(),
        );

        assert!(prompt.response_started.is_none());
        prompt.mark_response_started();
        assert!(prompt.response_started.is_some());
    }
}
