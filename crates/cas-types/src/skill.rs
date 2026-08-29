//! Skill type definitions
//!
//! Skills are specialized agent capabilities that can be invoked by CAS.

// Dead code check enabled - all items used

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::TypeError;
use crate::scope::Scope;

/// A single hook entry for skill-scoped hooks
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillHookEntry {
    /// Hook type: "command" for shell commands
    #[serde(rename = "type")]
    pub hook_type: String,
    /// Command to execute
    pub command: String,
    /// Optional timeout in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u32>,
}

impl SkillHookEntry {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            hook_type: "command".to_string(),
            command: command.into(),
            timeout: None,
        }
    }

    pub fn with_timeout(mut self, timeout_ms: u32) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }
}

/// Hook configuration with optional matcher
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillHookConfig {
    /// Optional matcher pattern (e.g., "Write|Edit" for tool matching)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    /// List of hooks to execute
    pub hooks: Vec<SkillHookEntry>,
}

impl SkillHookConfig {
    pub fn new(hooks: Vec<SkillHookEntry>) -> Self {
        Self {
            matcher: None,
            hooks,
        }
    }

    pub fn with_matcher(mut self, matcher: impl Into<String>) -> Self {
        self.matcher = Some(matcher.into());
        self
    }
}

/// Skill-scoped hooks (PreToolUse, PostToolUse, Stop)
/// These hooks are scoped to the skill's lifecycle
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillHooks {
    /// Hooks to run before tool execution
    #[serde(rename = "PreToolUse", skip_serializing_if = "Option::is_none")]
    pub pre_tool_use: Option<Vec<SkillHookConfig>>,
    /// Hooks to run after tool execution
    #[serde(rename = "PostToolUse", skip_serializing_if = "Option::is_none")]
    pub post_tool_use: Option<Vec<SkillHookConfig>>,
    /// Hooks to run when skill stops
    #[serde(rename = "Stop", skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<SkillHookConfig>>,
}

impl SkillHooks {
    pub fn is_empty(&self) -> bool {
        self.pre_tool_use.is_none() && self.post_tool_use.is_none() && self.stop.is_none()
    }
}

/// Status of a skill
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SkillStatus {
    /// Skill is active and can be used
    #[default]
    Enabled,
    /// Skill is disabled
    Disabled,
    /// Skill is experimental/draft
    Draft,
    /// Skill is retained for history but must not be invoked or synced
    Retired,
}

impl fmt::Display for SkillStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkillStatus::Enabled => write!(f, "enabled"),
            SkillStatus::Disabled => write!(f, "disabled"),
            SkillStatus::Draft => write!(f, "draft"),
            SkillStatus::Retired => write!(f, "retired"),
        }
    }
}

impl FromStr for SkillStatus {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "enabled" | "active" => Ok(SkillStatus::Enabled),
            "disabled" | "inactive" => Ok(SkillStatus::Disabled),
            "draft" | "experimental" => Ok(SkillStatus::Draft),
            "retired" | "archived" => Ok(SkillStatus::Retired),
            _ => Err(TypeError::Parse(format!("invalid skill status: {s}"))),
        }
    }
}

/// Type of skill invocation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SkillType {
    /// Shell command to execute
    #[default]
    Command,
    /// MCP server tool
    Mcp,
    /// Claude Code plugin
    Plugin,
    /// Internal function
    Internal,
}

impl fmt::Display for SkillType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkillType::Command => write!(f, "command"),
            SkillType::Mcp => write!(f, "mcp"),
            SkillType::Plugin => write!(f, "plugin"),
            SkillType::Internal => write!(f, "internal"),
        }
    }
}

impl FromStr for SkillType {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "command" | "cmd" | "shell" => Ok(SkillType::Command),
            "mcp" | "mcp-server" => Ok(SkillType::Mcp),
            "plugin" => Ok(SkillType::Plugin),
            "internal" | "builtin" => Ok(SkillType::Internal),
            _ => Err(TypeError::Parse(format!("invalid skill type: {s}"))),
        }
    }
}

/// Default scope for skills (global since skills are typically reusable)
fn default_global_scope() -> Scope {
    Scope::Global
}

/// A skill (specialized capability) in CAS
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Unique identifier (e.g., g-cas-sk01 or p-cas-sk01)
    pub id: String,

    /// Storage scope (global or project)
    /// Global: reusable skills stored in ~/.config/cas/ (default for skills)
    /// Project: project-specific skills stored in ./.cas/
    #[serde(default = "default_global_scope")]
    pub scope: Scope,

    /// Human-readable name
    pub name: String,

    /// Description of what the skill does
    pub description: String,

    /// Type of skill
    pub skill_type: SkillType,

    /// Command or tool to invoke (depends on skill_type)
    /// For Command: shell command template
    /// For MCP: server name and tool
    /// For Plugin: plugin path
    pub invocation: String,

    /// JSON schema for parameters (optional)
    #[serde(default)]
    pub parameters_schema: String,

    /// Example invocation
    #[serde(default)]
    pub example: String,

    /// Pre-conditions that must be satisfied before the skill can run
    /// e.g., "Docker must be running", "Node.js >= 18 required"
    #[serde(default)]
    pub preconditions: Vec<String>,

    /// Post-conditions that will be true after successful execution
    /// e.g., "Database schema will be updated", "File will be formatted"
    #[serde(default)]
    pub postconditions: Vec<String>,

    /// Validation script to check if the skill is available/working
    ///
    /// Examples: `which docker && docker info`, `node --version | grep -E 'v(1[89]|[2-9][0-9])'`
    #[serde(default)]
    pub validation_script: String,

    /// Status
    #[serde(default)]
    pub status: SkillStatus,

    /// Tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,

    /// Short 1-2 line summary for token-efficient context
    #[serde(default)]
    pub summary: String,

    /// Whether this skill can be manually invoked with arguments (e.g., /cas-task "title")
    /// Skills with invokable=true get argument-hint in SKILL.md frontmatter
    #[serde(default)]
    pub invokable: bool,

    /// Hint for expected arguments when invoked (e.g., "[title]", "[query] [limit]")
    /// Maps to argument-hint frontmatter in SKILL.md
    #[serde(default)]
    pub argument_hint: String,

    /// Context mode for skill execution (e.g., "fork" for forked sub-agent context)
    /// Maps to `context: fork` frontmatter in SKILL.md
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_mode: Option<String>,

    /// Agent type to use for skill execution
    /// Maps to `agent` frontmatter in SKILL.md
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,

    /// List of tools this skill is allowed to use
    /// Maps to `allowed-tools` frontmatter in SKILL.md (YAML list format)
    #[serde(default)]
    pub allowed_tools: Vec<String>,

    /// List of tools this skill is not allowed to use
    /// Maps to `disallowed-tools` frontmatter in SKILL.md (YAML list format)
    /// Used to enforce bans at the harness level (Claude Code 2.1.152+)
    #[serde(default)]
    pub disallowed_tools: Vec<String>,

    /// Skill-scoped hooks (PreToolUse, PostToolUse, Stop)
    /// Maps to `hooks` frontmatter in SKILL.md (Claude Code 2.1.0+)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<SkillHooks>,

    /// When true, prevents the skill from invoking the model (Claude Code 2.1.3+)
    /// Maps to `disable-model-invocation: true` frontmatter in SKILL.md
    /// Use for skills that should only execute commands without AI assistance
    #[serde(default)]
    pub disable_model_invocation: bool,

    /// Number of times the skill has been used
    #[serde(default)]
    pub usage_count: i32,

    /// When the skill was created
    pub created_at: DateTime<Utc>,

    /// When the skill was last updated
    pub updated_at: DateTime<Utc>,

    /// When the skill was last used
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<DateTime<Utc>>,

    /// Team ID this skill belongs to (None = personal/not shared with team)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,

    /// Per-skill team-promotion override (T5). See `Rule.share` for
    /// semantics. Dormant — no CLI currently writes this field for
    /// skills — but present to match Entry's shape end-to-end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<crate::scope::ShareScope>,
}

impl Skill {
    /// Create a new skill with the given ID and name (defaults to global scope)
    pub fn new(id: String, name: String) -> Self {
        Self::with_scope(id, name, Scope::Global)
    }

    /// Create a new skill with explicit scope
    pub fn with_scope(id: String, name: String, scope: Scope) -> Self {
        let now = Utc::now();
        Self {
            id,
            scope,
            name,
            description: String::new(),
            skill_type: SkillType::Command,
            invocation: String::new(),
            parameters_schema: String::new(),
            example: String::new(),
            preconditions: Vec::new(),
            postconditions: Vec::new(),
            validation_script: String::new(),
            status: SkillStatus::Enabled,
            tags: Vec::new(),
            summary: String::new(),
            invokable: false,
            argument_hint: String::new(),
            context_mode: None,
            agent_type: None,
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            hooks: None,
            disable_model_invocation: false,
            usage_count: 0,
            created_at: now,
            updated_at: now,
            last_used: None,
            team_id: None,
            share: None,
        }
    }

    /// Validate the skill is available by running the validation script
    /// Returns Ok(true) if validation passes, Ok(false) if validation fails, Err if there's an error
    pub fn validate(&self) -> Result<bool, std::io::Error> {
        if self.validation_script.is_empty() {
            return Ok(true); // No validation script = always valid
        }

        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(&self.validation_script)
            .output()?;

        Ok(output.status.success())
    }

    /// Get a token-efficient summary (uses summary field if set, otherwise truncated description)
    pub fn token_summary(&self, max_len: usize) -> String {
        if !self.summary.is_empty() {
            if self.summary.len() <= max_len {
                self.summary.clone()
            } else {
                format!("{}...", &self.summary[..max_len.saturating_sub(3)])
            }
        } else {
            self.preview(max_len)
        }
    }

    /// Check if the skill is enabled
    pub fn is_enabled(&self) -> bool {
        self.status == SkillStatus::Enabled
    }

    /// Get a short preview of the description
    pub fn preview(&self, max_len: usize) -> String {
        crate::preview::truncate_preview(&self.description, max_len)
    }

    /// Record a usage of this skill
    pub fn record_usage(&mut self) {
        self.usage_count += 1;
        self.last_used = Some(Utc::now());
        self.updated_at = Utc::now();
    }
}

impl Default for Skill {
    fn default() -> Self {
        Self::with_scope(String::new(), String::new(), Scope::Global)
    }
}

#[cfg(test)]
mod tests {
    use crate::skill::*;

    #[test]
    fn test_skill_status_from_str() {
        assert_eq!(
            SkillStatus::from_str("enabled").unwrap(),
            SkillStatus::Enabled
        );
        assert_eq!(
            SkillStatus::from_str("disabled").unwrap(),
            SkillStatus::Disabled
        );
        assert_eq!(SkillStatus::from_str("draft").unwrap(), SkillStatus::Draft);
        assert!(SkillStatus::from_str("invalid").is_err());
    }

    #[test]
    fn test_skill_type_from_str() {
        assert_eq!(SkillType::from_str("command").unwrap(), SkillType::Command);
        assert_eq!(SkillType::from_str("mcp").unwrap(), SkillType::Mcp);
        assert_eq!(SkillType::from_str("plugin").unwrap(), SkillType::Plugin);
        assert_eq!(
            SkillType::from_str("internal").unwrap(),
            SkillType::Internal
        );
    }

    #[test]
    fn test_skill_new() {
        let skill = Skill::new("cas-sk01".to_string(), "Test Skill".to_string());
        assert_eq!(skill.id, "cas-sk01");
        assert_eq!(skill.name, "Test Skill");
        assert!(skill.is_enabled());
        assert_eq!(skill.usage_count, 0);
        // New invokable fields default to false/empty
        assert!(!skill.invokable);
        assert!(skill.argument_hint.is_empty());
    }

    #[test]
    fn test_skill_invokable() {
        let mut skill = Skill::new("cas-sk01".to_string(), "Task".to_string());
        skill.invokable = true;
        skill.argument_hint = "[title]".to_string();
        assert!(skill.invokable);
        assert_eq!(skill.argument_hint, "[title]");
    }

    #[test]
    fn test_skill_record_usage() {
        let mut skill = Skill::new("cas-sk01".to_string(), "Test".to_string());
        assert!(skill.last_used.is_none());

        skill.record_usage();
        assert_eq!(skill.usage_count, 1);
        assert!(skill.last_used.is_some());

        skill.record_usage();
        assert_eq!(skill.usage_count, 2);
    }
}
