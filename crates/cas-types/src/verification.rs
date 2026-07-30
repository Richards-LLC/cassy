//! Verification type definitions
//!
//! Verifications are quality gates that check task completion before allowing closure.
//! A Haiku subagent reviews the work and approves or rejects based on completeness.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::TypeError;

/// Type of verification (task-level or epic-level)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VerificationType {
    /// Task-level verification (individual subtask)
    #[default]
    Task,
    /// Epic-level verification (merged code on master)
    Epic,
}

impl fmt::Display for VerificationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerificationType::Task => write!(f, "task"),
            VerificationType::Epic => write!(f, "epic"),
        }
    }
}

impl FromStr for VerificationType {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "task" => Ok(VerificationType::Task),
            "epic" => Ok(VerificationType::Epic),
            _ => Err(TypeError::Parse(format!("invalid verification type: {s}"))),
        }
    }
}

/// Server-derived authority that produced a verification record.
///
/// This is audit provenance, not a caller-selectable authorization hint.
/// New MCP adds may only produce `TaskVerifier` or `SupervisorDirect`;
/// `System` is reserved for internal close-flow writes. `Legacy` keeps
/// pre-provenance rows and payloads readable without granting them authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VerificationProvenance {
    /// Row predates typed verifier authority.
    #[default]
    Legacy,
    /// Distinct registered task-verifier child holding a one-time capability.
    TaskVerifier,
    /// Direct verification by a registered supervisor session.
    SupervisorDirect,
    /// Trusted internal close-flow record; never accepted from MCP input.
    System,
}

impl fmt::Display for VerificationProvenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerificationProvenance::Legacy => write!(f, "legacy"),
            VerificationProvenance::TaskVerifier => write!(f, "task_verifier"),
            VerificationProvenance::SupervisorDirect => write!(f, "supervisor_direct"),
            VerificationProvenance::System => write!(f, "system"),
        }
    }
}

impl FromStr for VerificationProvenance {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "legacy" => Ok(VerificationProvenance::Legacy),
            "task_verifier" => Ok(VerificationProvenance::TaskVerifier),
            "supervisor_direct" => Ok(VerificationProvenance::SupervisorDirect),
            "system" => Ok(VerificationProvenance::System),
            _ => Err(TypeError::Parse(format!(
                "invalid verification provenance: {s}"
            ))),
        }
    }
}

/// Durable server-issued authority for one task-verifier child.
///
/// The raw bearer token is never stored here. Only its SHA-256 digest is
/// persisted. A capability is task-scoped, expires, binds once to a distinct
/// registered child agent, and may be consumed once.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifierCapability {
    pub id: String,
    pub task_id: String,
    pub issuer_agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier_agent_id: Option<String>,
    pub token_hash: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_at: Option<DateTime<Utc>>,
}

/// Status of a verification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    /// Verification approved - work is complete
    #[default]
    Approved,
    /// Verification rejected - issues found
    Rejected,
    /// Verification failed with error
    Error,
    /// Verification skipped (force bypass)
    Skipped,
}

impl fmt::Display for VerificationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerificationStatus::Approved => write!(f, "approved"),
            VerificationStatus::Rejected => write!(f, "rejected"),
            VerificationStatus::Error => write!(f, "error"),
            VerificationStatus::Skipped => write!(f, "skipped"),
        }
    }
}

impl FromStr for VerificationStatus {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "approved" => Ok(VerificationStatus::Approved),
            "rejected" => Ok(VerificationStatus::Rejected),
            "error" => Ok(VerificationStatus::Error),
            "skipped" => Ok(VerificationStatus::Skipped),
            _ => Err(TypeError::Parse(format!(
                "invalid verification status: {s}"
            ))),
        }
    }
}

/// Severity of a verification issue
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IssueSeverity {
    /// Must be fixed before task can close
    #[default]
    Blocking,
    /// Should be fixed but not required
    Warning,
}

impl fmt::Display for IssueSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IssueSeverity::Blocking => write!(f, "blocking"),
            IssueSeverity::Warning => write!(f, "warning"),
        }
    }
}

impl FromStr for IssueSeverity {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "blocking" => Ok(IssueSeverity::Blocking),
            "warning" => Ok(IssueSeverity::Warning),
            _ => Err(TypeError::Parse(format!("invalid issue severity: {s}"))),
        }
    }
}

/// An issue found during verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationIssue {
    /// File where the issue was found
    pub file: String,

    /// Line number (if known)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,

    /// Issue severity
    #[serde(default)]
    pub severity: IssueSeverity,

    /// Category of issue (e.g., "todo_comment", "temporal_shortcut")
    pub category: String,

    /// Code snippet showing the issue
    #[serde(default)]
    pub code: String,

    /// Description of the problem
    pub problem: String,

    /// Suggested fix
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

impl VerificationIssue {
    /// Create a new verification issue
    pub fn new(file: String, category: String, problem: String) -> Self {
        Self {
            file,
            line: None,
            severity: IssueSeverity::Blocking,
            category,
            code: String::new(),
            problem,
            suggestion: None,
        }
    }

    /// Create a blocking issue with full details
    pub fn blocking(
        file: String,
        line: Option<u32>,
        category: String,
        code: String,
        problem: String,
        suggestion: Option<String>,
    ) -> Self {
        Self {
            file,
            line,
            severity: IssueSeverity::Blocking,
            category,
            code,
            problem,
            suggestion,
        }
    }

    /// Create a warning issue
    pub fn warning(file: String, category: String, problem: String) -> Self {
        Self {
            file,
            line: None,
            severity: IssueSeverity::Warning,
            category,
            code: String::new(),
            problem,
            suggestion: None,
        }
    }

    /// Check if this is a blocking issue
    pub fn is_blocking(&self) -> bool {
        self.severity == IssueSeverity::Blocking
    }
}

/// A task verification result
///
/// Created when attempting to close a task. A Haiku subagent reviews
/// the work and either approves or rejects with a list of issues.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verification {
    /// Unique identifier (e.g., ver-a1b2)
    pub id: String,

    /// Task ID being verified
    pub task_id: String,

    /// Agent ID of the verifier (subagent that performed verification)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,

    /// Type of verification (task or epic level)
    #[serde(default)]
    pub verification_type: VerificationType,

    /// Server-derived verifier authority provenance.
    #[serde(default)]
    pub provenance: VerificationProvenance,

    /// One-time verifier capability used for this record, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_id: Option<String>,

    /// Registered parent session that issued the task-verifier capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_agent_id: Option<String>,

    /// Verification status
    #[serde(default)]
    pub status: VerificationStatus,

    /// Confidence score (0.0 to 1.0)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,

    /// Summary of the verification decision
    #[serde(default)]
    pub summary: String,

    /// Issues found during verification
    #[serde(default)]
    pub issues: Vec<VerificationIssue>,

    /// Files that were reviewed
    #[serde(default)]
    pub files_reviewed: Vec<String>,

    /// How long verification took (milliseconds)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,

    /// When the verification was created
    pub created_at: DateTime<Utc>,
}

impl Verification {
    /// Create a new verification
    pub fn new(id: String, task_id: String) -> Self {
        Self {
            id,
            task_id,
            agent_id: None,
            verification_type: VerificationType::Task,
            provenance: VerificationProvenance::System,
            capability_id: None,
            issuer_agent_id: None,
            status: VerificationStatus::Approved,
            confidence: None,
            summary: String::new(),
            issues: Vec::new(),
            files_reviewed: Vec::new(),
            duration_ms: None,
            created_at: Utc::now(),
        }
    }

    /// Create an approved verification
    pub fn approved(id: String, task_id: String, summary: String) -> Self {
        Self {
            id,
            task_id,
            agent_id: None,
            verification_type: VerificationType::Task,
            provenance: VerificationProvenance::System,
            capability_id: None,
            issuer_agent_id: None,
            status: VerificationStatus::Approved,
            confidence: None,
            summary,
            issues: Vec::new(),
            files_reviewed: Vec::new(),
            duration_ms: None,
            created_at: Utc::now(),
        }
    }

    /// Create a rejected verification with issues
    pub fn rejected(
        id: String,
        task_id: String,
        summary: String,
        issues: Vec<VerificationIssue>,
    ) -> Self {
        Self {
            id,
            task_id,
            agent_id: None,
            verification_type: VerificationType::Task,
            provenance: VerificationProvenance::System,
            capability_id: None,
            issuer_agent_id: None,
            status: VerificationStatus::Rejected,
            confidence: None,
            summary,
            issues,
            files_reviewed: Vec::new(),
            duration_ms: None,
            created_at: Utc::now(),
        }
    }

    /// Create an error verification
    pub fn error(id: String, task_id: String, error_message: String) -> Self {
        Self {
            id,
            task_id,
            agent_id: None,
            verification_type: VerificationType::Task,
            provenance: VerificationProvenance::System,
            capability_id: None,
            issuer_agent_id: None,
            status: VerificationStatus::Error,
            confidence: None,
            summary: error_message,
            issues: Vec::new(),
            files_reviewed: Vec::new(),
            duration_ms: None,
            created_at: Utc::now(),
        }
    }

    /// Create a skipped verification (force bypass)
    pub fn skipped(id: String, task_id: String, reason: String) -> Self {
        Self {
            id,
            task_id,
            agent_id: None,
            verification_type: VerificationType::Task,
            provenance: VerificationProvenance::System,
            capability_id: None,
            issuer_agent_id: None,
            status: VerificationStatus::Skipped,
            confidence: None,
            summary: reason,
            issues: Vec::new(),
            files_reviewed: Vec::new(),
            duration_ms: None,
            created_at: Utc::now(),
        }
    }

    /// Check if verification was approved
    pub fn is_approved(&self) -> bool {
        self.status == VerificationStatus::Approved
    }

    /// Check if verification was rejected
    pub fn is_rejected(&self) -> bool {
        self.status == VerificationStatus::Rejected
    }

    /// Get count of blocking issues
    pub fn blocking_count(&self) -> usize {
        self.issues.iter().filter(|i| i.is_blocking()).count()
    }

    /// Get count of warning issues
    pub fn warning_count(&self) -> usize {
        self.issues.iter().filter(|i| !i.is_blocking()).count()
    }

    /// Add an issue to the verification
    pub fn add_issue(&mut self, issue: VerificationIssue) {
        self.issues.push(issue);
    }

    /// Add a file to the reviewed list
    pub fn add_file_reviewed(&mut self, file: String) {
        if !self.files_reviewed.contains(&file) {
            self.files_reviewed.push(file);
        }
    }

    /// Set the duration
    pub fn set_duration(&mut self, duration_ms: u64) {
        self.duration_ms = Some(duration_ms);
    }

    /// Set the agent ID
    pub fn set_agent(&mut self, agent_id: String) {
        self.agent_id = Some(agent_id);
    }

    /// Set confidence score
    pub fn set_confidence(&mut self, confidence: f32) {
        self.confidence = Some(confidence.clamp(0.0, 1.0));
    }
}

impl Default for Verification {
    fn default() -> Self {
        Self {
            id: String::new(),
            task_id: String::new(),
            agent_id: None,
            verification_type: VerificationType::Task,
            provenance: VerificationProvenance::System,
            capability_id: None,
            issuer_agent_id: None,
            status: VerificationStatus::Approved,
            confidence: None,
            summary: String::new(),
            issues: Vec::new(),
            files_reviewed: Vec::new(),
            duration_ms: None,
            created_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::verification::*;

    #[test]
    fn test_verification_status_from_str() {
        assert_eq!(
            VerificationStatus::from_str("approved").unwrap(),
            VerificationStatus::Approved
        );
        assert_eq!(
            VerificationStatus::from_str("rejected").unwrap(),
            VerificationStatus::Rejected
        );
        assert_eq!(
            VerificationStatus::from_str("error").unwrap(),
            VerificationStatus::Error
        );
        assert_eq!(
            VerificationStatus::from_str("skipped").unwrap(),
            VerificationStatus::Skipped
        );
        assert!(VerificationStatus::from_str("invalid").is_err());
    }

    #[test]
    fn test_verification_new() {
        let v = Verification::new("ver-a1b2".to_string(), "cas-1234".to_string());
        assert_eq!(v.id, "ver-a1b2");
        assert_eq!(v.task_id, "cas-1234");
        assert!(v.is_approved());
        assert_eq!(v.blocking_count(), 0);
    }

    #[test]
    fn test_verification_rejected() {
        let issues = vec![
            VerificationIssue::blocking(
                "src/main.rs".to_string(),
                Some(42),
                "todo_comment".to_string(),
                "// TODO: implement".to_string(),
                "TODO comment found".to_string(),
                Some("Implement the function".to_string()),
            ),
            VerificationIssue::warning(
                "src/lib.rs".to_string(),
                "hardcoded_value".to_string(),
                "Magic number detected".to_string(),
            ),
        ];

        let v = Verification::rejected(
            "ver-a1b2".to_string(),
            "cas-1234".to_string(),
            "Found incomplete work".to_string(),
            issues,
        );

        assert!(v.is_rejected());
        assert_eq!(v.blocking_count(), 1);
        assert_eq!(v.warning_count(), 1);
    }

    #[test]
    fn test_verification_issue() {
        let issue = VerificationIssue::blocking(
            "src/api.rs".to_string(),
            Some(45),
            "temporal_shortcut".to_string(),
            "// for now just return empty".to_string(),
            "Temporal shortcut language detected".to_string(),
            Some("Implement proper logic".to_string()),
        );

        assert!(issue.is_blocking());
        assert_eq!(issue.file, "src/api.rs");
        assert_eq!(issue.line, Some(45));
    }

    #[test]
    fn test_issue_severity() {
        assert_eq!(
            IssueSeverity::from_str("blocking").unwrap(),
            IssueSeverity::Blocking
        );
        assert_eq!(
            IssueSeverity::from_str("warning").unwrap(),
            IssueSeverity::Warning
        );
    }

    #[test]
    fn test_add_files_reviewed() {
        let mut v = Verification::new("ver-a1b2".to_string(), "cas-1234".to_string());
        v.add_file_reviewed("src/main.rs".to_string());
        v.add_file_reviewed("src/lib.rs".to_string());
        v.add_file_reviewed("src/main.rs".to_string()); // duplicate

        assert_eq!(v.files_reviewed.len(), 2);
    }

    #[test]
    fn test_set_confidence() {
        let mut v = Verification::new("ver-a1b2".to_string(), "cas-1234".to_string());

        v.set_confidence(0.95);
        assert_eq!(v.confidence, Some(0.95));

        v.set_confidence(1.5); // Should clamp to 1.0
        assert_eq!(v.confidence, Some(1.0));

        v.set_confidence(-0.5); // Should clamp to 0.0
        assert_eq!(v.confidence, Some(0.0));
    }
}
