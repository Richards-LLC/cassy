//! Rule type definitions

// Dead code check enabled - all items used

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::TypeError;
use crate::scope::Scope;

/// Category of a rule - helps with filtering and prioritization
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RuleCategory {
    /// General purpose rule
    #[default]
    General,
    /// Naming conventions, code style patterns
    Convention,
    /// Security-related guidelines
    Security,
    /// Performance optimization patterns
    Performance,
    /// Architectural patterns and boundaries
    Architecture,
    /// Error handling patterns
    ErrorHandling,
}

impl fmt::Display for RuleCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuleCategory::General => write!(f, "general"),
            RuleCategory::Convention => write!(f, "convention"),
            RuleCategory::Security => write!(f, "security"),
            RuleCategory::Performance => write!(f, "performance"),
            RuleCategory::Architecture => write!(f, "architecture"),
            RuleCategory::ErrorHandling => write!(f, "error-handling"),
        }
    }
}

impl FromStr for RuleCategory {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace('-', "_").as_str() {
            "general" => Ok(RuleCategory::General),
            "convention" => Ok(RuleCategory::Convention),
            "security" => Ok(RuleCategory::Security),
            "performance" => Ok(RuleCategory::Performance),
            "architecture" | "arch" => Ok(RuleCategory::Architecture),
            "error_handling" | "errorhandling" | "error" => Ok(RuleCategory::ErrorHandling),
            _ => Err(TypeError::InvalidRuleCategory(s.to_string())),
        }
    }
}

/// Status of a rule in its lifecycle
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RuleStatus {
    /// New, unproven rule
    #[default]
    Draft,
    /// Proven rule, synced to Claude Code
    Proven,
    /// Not accessed recently
    Stale,
    /// Explicitly disabled
    Retired,
}

impl fmt::Display for RuleStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuleStatus::Draft => write!(f, "draft"),
            RuleStatus::Proven => write!(f, "proven"),
            RuleStatus::Stale => write!(f, "stale"),
            RuleStatus::Retired => write!(f, "retired"),
        }
    }
}

impl FromStr for RuleStatus {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "draft" => Ok(RuleStatus::Draft),
            "proven" => Ok(RuleStatus::Proven),
            "stale" => Ok(RuleStatus::Stale),
            "retired" => Ok(RuleStatus::Retired),
            _ => Err(TypeError::InvalidRuleStatus(s.to_string())),
        }
    }
}

/// A rule extracted from memory entries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// Unique identifier in scope-rule-NNN format (e.g., g-rule-001 or p-rule-001)
    pub id: String,

    /// Storage scope (global or project)
    /// Global: user style preferences stored in ~/.config/cas/
    /// Project: project-specific conventions stored in ./.cas/
    #[serde(default)]
    pub scope: Scope,

    /// When the rule was created
    pub created: DateTime<Utc>,

    /// Entry IDs this rule was extracted from
    #[serde(default)]
    pub source_ids: Vec<String>,

    /// The rule description/content
    pub content: String,

    /// Current status of the rule
    #[serde(default)]
    pub status: RuleStatus,

    /// Number of times marked helpful
    #[serde(default)]
    pub helpful_count: i32,

    /// Number of times marked harmful
    #[serde(default)]
    pub harmful_count: i32,

    /// Optional categorization tags
    #[serde(default)]
    pub tags: Vec<String>,

    /// File pattern glob for rule applicability
    #[serde(default)]
    pub paths: String,

    /// Last time the rule was accessed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_accessed: Option<DateTime<Utc>>,

    /// When the rule should be reviewed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_after: Option<DateTime<Utc>>,

    /// Rule category for filtering and prioritization
    #[serde(default)]
    pub category: RuleCategory,

    /// Rule priority: 0=critical (always surface), 1=high, 2=normal (default), 3=low
    /// Critical rules bypass token budget limits
    #[serde(default = "default_priority")]
    pub priority: u8,

    /// Number of times this rule has been surfaced in context
    /// Tracked separately from helpful_count to enable quality-gated promotion
    #[serde(default)]
    pub surface_count: i32,

    /// Comma-separated list of tools to auto-approve when this rule matches
    /// Only applies to proven rules. Example: "Read,Glob,Grep"
    /// Dangerous tools (Bash, Write, Edit) cannot be auto-approved via rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_approve_tools: Option<String>,

    /// Glob patterns for paths that trigger auto-approval (separate from matching paths)
    /// When a tool operates on files matching these patterns AND the tool is in
    /// auto_approve_tools, the operation is auto-approved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_approve_paths: Option<String>,

    /// Team ID this rule belongs to (None = personal/not shared with team)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,

    /// Per-rule team-promotion override (T5 cas-07d7). `None` = T1
    /// auto-rule applies (Project-scope rules dual-enqueue);
    /// `Some(Private)` suppresses the team enqueue; `Some(Team)` force-
    /// promotes even Global-scope rules. No CLI currently writes this
    /// field for rules — dormant but wired to match Entry's shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<crate::scope::ShareScope>,

    /// cas-5372 (GH #990): who authorised this rule as an operator hard rule,
    /// bound to the exact content they authorised. Written only by a local
    /// create or update from the operator or a registered supervisor, and
    /// persisted only in this project's store. It is never serialised, so a
    /// rule pulled from the cloud or another project arrives without it.
    #[serde(skip)]
    pub operator_authority: Option<OperatorRuleAuthority>,
}

/// cas-5372: an operator hard rule's authorisation. `content_sha256` is the
/// digest of the rule content the author authorised; any later change to the
/// content by anyone else (an edit, a pull) invalidates it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorRuleAuthority {
    /// `supervisor:<name>` or `operator:<name>`.
    pub author: String,
    pub content_sha256: String,
}

impl OperatorRuleAuthority {
    fn digest(content: &str) -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(content.trim().as_bytes()))
    }

    /// Stored form: `<author>|<sha256>`.
    pub fn to_stored(&self) -> String {
        format!("{}|{}", self.author, self.content_sha256)
    }

    pub fn from_stored(stored: &str) -> Option<Self> {
        let (author, content_sha256) = stored.rsplit_once('|')?;
        (!author.is_empty() && content_sha256.len() == 64).then(|| Self {
            author: author.to_string(),
            content_sha256: content_sha256.to_string(),
        })
    }
}

fn default_priority() -> u8 {
    2 // Normal priority
}

impl Rule {
    /// Create a new rule with the given content (defaults to project scope)
    pub fn new(id: String, content: String) -> Self {
        Self::with_scope(id, content, Scope::Project)
    }

    /// Create a new rule with explicit scope
    pub fn with_scope(id: String, content: String, scope: Scope) -> Self {
        Self {
            id,
            scope,
            created: Utc::now(),
            source_ids: Vec::new(),
            content,
            status: RuleStatus::default(),
            helpful_count: 0,
            harmful_count: 0,
            tags: Vec::new(),
            paths: String::new(),
            last_accessed: None,
            review_after: Some(Utc::now() + chrono::Duration::days(30)),
            category: RuleCategory::default(),
            priority: 2,
            surface_count: 0,
            auto_approve_tools: None,
            auto_approve_paths: None,
            team_id: None,
            share: None,
            operator_authority: None,
        }
    }

    /// Check if this is a critical rule (priority 0) that should always be surfaced
    pub fn is_critical(&self) -> bool {
        self.priority == 0
    }

    /// Tag that marks a rule as an operator hard rule.
    pub const OPERATOR_HARD_RULE_TAG: &'static str = "hard-rule";

    /// cas-5372 (GH #990): an operator's verbatim hard rule. It takes effect
    /// the day it is recorded instead of waiting for promotion: it is synced
    /// to Claude Code and always surfaced at session start, labelled DRAFT
    /// until promoted. It must look like a hard rule and carry a valid
    /// [`OperatorRuleAuthority`] for its current content — text alone, from
    /// any agent or any pulled row, never qualifies.
    pub fn is_operator_hard_rule(&self) -> bool {
        self.looks_like_hard_rule()
            && self.operator_authority.as_ref().is_some_and(|authority| {
                authority.content_sha256 == OperatorRuleAuthority::digest(&self.content)
            })
    }

    /// Record `author` as authorising this rule's current content.
    pub fn authorize_operator_hard_rule(&mut self, author: &str) {
        self.operator_authority = Some(OperatorRuleAuthority {
            author: author.to_string(),
            content_sha256: OperatorRuleAuthority::digest(&self.content),
        });
    }

    /// Content opening with `HARD RULE`, or tagged
    /// [`Self::OPERATOR_HARD_RULE_TAG`]. Shape only; see
    /// [`Self::is_operator_hard_rule`] for the authority check.
    pub fn looks_like_hard_rule(&self) -> bool {
        let content = self.content.trim_start();
        content
            .get(..9)
            .is_some_and(|head| head.eq_ignore_ascii_case("hard rule"))
            // A whole phrase, not the start of "Hard rules are …".
            && !content[9..]
                .chars()
                .next()
                .is_some_and(|next| next.is_alphanumeric())
            || self
                .tags
                .iter()
                .any(|tag| tag.trim().eq_ignore_ascii_case(Self::OPERATOR_HARD_RULE_TAG))
    }

    /// cas-5372: an operator hard rule that is live — draft or proven, never
    /// stale or retired.
    pub fn is_active_operator_hard_rule(&self) -> bool {
        matches!(self.status, RuleStatus::Draft | RuleStatus::Proven)
            && self.is_operator_hard_rule()
    }

    /// cas-5372: a live operator hard rule still awaiting promotion.
    pub fn is_draft_operator_hard_rule(&self) -> bool {
        self.status == RuleStatus::Draft && self.is_operator_hard_rule()
    }

    /// Text a synced or surfaced rule carries: a draft operator hard rule is
    /// labelled so its standing is never mistaken for a promoted rule.
    pub fn surfaced_content(&self) -> String {
        if self.is_draft_operator_hard_rule() {
            format!(
                "DRAFT (operator hard rule, pending promotion; follow it as written): {}",
                self.content.trim()
            )
        } else {
            self.content.trim().to_string()
        }
    }

    /// Check if this is a security rule
    pub fn is_security(&self) -> bool {
        self.category == RuleCategory::Security
    }

    /// Tools that are considered safe for auto-approval
    pub const SAFE_AUTO_APPROVE_TOOLS: &'static [&'static str] =
        &["Read", "Glob", "Grep", "WebFetch", "WebSearch"];

    /// Tools that are too dangerous to ever auto-approve via rules
    pub const DANGEROUS_TOOLS: &'static [&'static str] = &["Bash", "Write", "Edit", "NotebookEdit"];

    /// Check if this rule has auto-approval configured
    pub fn has_auto_approve(&self) -> bool {
        self.auto_approve_tools
            .as_ref()
            .map(|t| !t.is_empty())
            .unwrap_or(false)
    }

    /// Get the list of tools configured for auto-approval
    pub fn get_auto_approve_tools(&self) -> Vec<&str> {
        self.auto_approve_tools
            .as_ref()
            .map(|t| {
                t.split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Check if a specific tool is auto-approved by this rule
    pub fn auto_approves_tool(&self, tool_name: &str) -> bool {
        self.get_auto_approve_tools()
            .iter()
            .any(|t| t.eq_ignore_ascii_case(tool_name))
    }

    /// Validate the auto_approve_tools configuration
    /// Returns Ok if valid, Err with a message describing the problem
    pub fn validate_auto_approve(&self) -> Result<(), String> {
        let tools = self.get_auto_approve_tools();

        // Check for dangerous tools
        for tool in &tools {
            if Self::DANGEROUS_TOOLS
                .iter()
                .any(|d| d.eq_ignore_ascii_case(tool))
            {
                return Err(format!(
                    "Cannot auto-approve dangerous tool '{}'. Dangerous tools ({}) require explicit approval.",
                    tool,
                    Self::DANGEROUS_TOOLS.join(", ")
                ));
            }
        }

        // Warn about unknown tools (not an error, just informational)
        // This allows for future tools to be added without updating the rule validation
        Ok(())
    }

    /// Check if this rule can be used for auto-approval
    /// Rules must be proven and have valid auto-approve configuration
    pub fn can_auto_approve(&self) -> bool {
        self.status == RuleStatus::Proven
            && self.has_auto_approve()
            && self.validate_auto_approve().is_ok()
    }

    /// Get the list of paths configured for auto-approval
    pub fn get_auto_approve_paths(&self) -> Vec<&str> {
        self.auto_approve_paths
            .as_ref()
            .map(|p| {
                p.split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Check if a path matches the auto-approval patterns
    pub fn matches_auto_approve_path(&self, path: &str) -> bool {
        let patterns = self.get_auto_approve_paths();
        if patterns.is_empty() {
            return false;
        }
        for pattern in patterns {
            if let Ok(glob) = glob::Pattern::new(pattern) {
                if glob.matches(path) {
                    return true;
                }
            }
            // Also check simple prefix/suffix matching
            if pattern.ends_with('*') && path.starts_with(&pattern[..pattern.len() - 1]) {
                return true;
            }
            if pattern.starts_with('*') && path.ends_with(&pattern[1..]) {
                return true;
            }
        }
        false
    }

    /// Calculate the feedback score (helpful - harmful)
    pub fn feedback_score(&self) -> i32 {
        self.helpful_count - self.harmful_count
    }

    /// Check if the rule is active (not retired)
    pub fn is_active(&self) -> bool {
        self.status != RuleStatus::Retired
    }

    /// Check if the rule needs review
    pub fn needs_review(&self) -> bool {
        self.review_after.map(|r| r <= Utc::now()).unwrap_or(false)
    }

    /// Get a short preview of the content
    pub fn preview(&self, max_len: usize) -> String {
        let first_line = self.content.lines().next().unwrap_or(&self.content);
        crate::preview::truncate_preview(first_line, max_len)
    }
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            id: String::new(),
            scope: Scope::default(),
            created: Utc::now(),
            source_ids: Vec::new(),
            content: String::new(),
            status: RuleStatus::default(),
            helpful_count: 0,
            harmful_count: 0,
            tags: Vec::new(),
            paths: String::new(),
            last_accessed: None,
            review_after: None,
            category: RuleCategory::default(),
            priority: 2,
            surface_count: 0,
            auto_approve_tools: None,
            auto_approve_paths: None,
            team_id: None,
            share: None,
            operator_authority: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rule::*;

    #[test]
    fn operator_hard_rule_is_recognised_by_prefix_or_tag_cas_5372() {
        let rule = |content: &str, tags: &[&str], status: RuleStatus| {
            let mut rule = Rule::new("rule-1".to_string(), content.to_string());
            rule.tags = tags.iter().map(|tag| tag.to_string()).collect();
            rule.status = status;
            rule
        };
        assert!(rule("HARD RULE (Ben): no SMS changes", &[], RuleStatus::Draft).looks_like_hard_rule());
        assert!(rule("  hard rule: lower case", &[], RuleStatus::Draft).looks_like_hard_rule());
        assert!(rule("Test on staging", &["qa", "HARD-RULE"], RuleStatus::Draft).looks_like_hard_rule());
        assert!(!rule("Hard rules are hard", &[], RuleStatus::Draft).looks_like_hard_rule());
        assert!(!rule("Prefer small commits", &["hard"], RuleStatus::Draft).looks_like_hard_rule());
        assert!(!rule("HARD", &[], RuleStatus::Draft).looks_like_hard_rule());

        let mut draft = rule("HARD RULE: x", &[], RuleStatus::Draft);
        draft.authorize_operator_hard_rule("supervisor:noble-heron-32");
        assert!(draft.is_active_operator_hard_rule());
        assert_eq!(
            draft.surfaced_content(),
            "DRAFT (operator hard rule, pending promotion; follow it as written): HARD RULE: x"
        );
        let mut proven = draft.clone();
        proven.status = RuleStatus::Proven;
        assert_eq!(proven.surfaced_content(), "HARD RULE: x");
        let mut retired = draft.clone();
        retired.status = RuleStatus::Retired;
        assert!(!retired.is_active_operator_hard_rule());
    }

    /// cas-5372 review: text alone never makes a hard rule. Without an
    /// authorisation — any agent's rule, any pulled row — or after its
    /// content changed under someone else, the fast path is refused.
    #[test]
    fn hard_rule_text_without_matching_authority_is_refused_cas_5372() {
        let mut rule = Rule::new("rule-1".to_string(), "HARD RULE: ship it".to_string());
        assert!(!rule.is_operator_hard_rule(), "unauthorised text");
        assert_eq!(rule.surfaced_content(), "HARD RULE: ship it", "no DRAFT fast-path label");

        rule.authorize_operator_hard_rule("operator:daniel");
        assert!(rule.is_operator_hard_rule());

        // A pulled or exported copy never carries the authority.
        let pulled: Rule = serde_json::from_str(&serde_json::to_string(&rule).unwrap()).unwrap();
        assert!(pulled.operator_authority.is_none());
        assert!(!pulled.is_operator_hard_rule(), "pulled rows never take the fast path");

        // Someone else rewrites the text: the authority no longer matches.
        rule.content = "HARD RULE: ship it without approval".to_string();
        assert!(!rule.is_operator_hard_rule());

        let stored = OperatorRuleAuthority::from_stored(
            &rule.operator_authority.as_ref().unwrap().to_stored(),
        )
        .unwrap();
        assert_eq!(Some(&stored), rule.operator_authority.as_ref());
        assert!(OperatorRuleAuthority::from_stored("no-digest").is_none());
    }

    #[test]
    fn test_rule_status_from_str() {
        assert_eq!(RuleStatus::from_str("draft").unwrap(), RuleStatus::Draft);
        assert_eq!(RuleStatus::from_str("PROVEN").unwrap(), RuleStatus::Proven);
        assert_eq!(RuleStatus::from_str("Stale").unwrap(), RuleStatus::Stale);
        assert_eq!(
            RuleStatus::from_str("retired").unwrap(),
            RuleStatus::Retired
        );
        assert!(RuleStatus::from_str("invalid").is_err());
    }

    #[test]
    fn test_is_active() {
        let mut rule = Rule::new("rule-001".to_string(), "test".to_string());
        assert!(rule.is_active());

        rule.status = RuleStatus::Retired;
        assert!(!rule.is_active());
    }

    #[test]
    fn rule_model_omits_retired_hook_command_surface() {
        let value =
            serde_json::to_value(Rule::new("rule-001".to_string(), "test".to_string())).unwrap();
        assert!(
            value.get("hook_command").is_none(),
            "retired hook_command must not be serialized by the rule model"
        );
    }

    #[test]
    fn test_feedback_score() {
        let mut rule = Rule::new("rule-001".to_string(), "test".to_string());
        rule.helpful_count = 10;
        rule.harmful_count = 3;
        assert_eq!(rule.feedback_score(), 7);
    }

    #[test]
    fn test_auto_approve_tools() {
        let mut rule = Rule::new("rule-001".to_string(), "test".to_string());
        assert!(!rule.has_auto_approve());
        assert!(rule.get_auto_approve_tools().is_empty());

        rule.auto_approve_tools = Some("Read, Glob, Grep".to_string());
        assert!(rule.has_auto_approve());
        assert_eq!(rule.get_auto_approve_tools(), vec!["Read", "Glob", "Grep"]);
        assert!(rule.auto_approves_tool("Read"));
        assert!(rule.auto_approves_tool("read")); // case insensitive
        assert!(!rule.auto_approves_tool("Write"));
    }

    #[test]
    fn test_validate_auto_approve_rejects_dangerous() {
        let mut rule = Rule::new("rule-001".to_string(), "test".to_string());

        // Safe tools should pass
        rule.auto_approve_tools = Some("Read,Glob".to_string());
        assert!(rule.validate_auto_approve().is_ok());

        // Dangerous tools should fail
        rule.auto_approve_tools = Some("Bash".to_string());
        assert!(rule.validate_auto_approve().is_err());

        rule.auto_approve_tools = Some("Read,Write".to_string());
        assert!(rule.validate_auto_approve().is_err());

        rule.auto_approve_tools = Some("Edit".to_string());
        assert!(rule.validate_auto_approve().is_err());
    }

    #[test]
    fn test_can_auto_approve() {
        let mut rule = Rule::new("rule-001".to_string(), "test".to_string());
        rule.auto_approve_tools = Some("Read,Glob".to_string());

        // Draft rules cannot auto-approve
        assert!(!rule.can_auto_approve());

        // Proven rules with valid config can auto-approve
        rule.status = RuleStatus::Proven;
        assert!(rule.can_auto_approve());

        // Proven rules with dangerous tools cannot auto-approve
        rule.auto_approve_tools = Some("Bash".to_string());
        assert!(!rule.can_auto_approve());
    }
}
