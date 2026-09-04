//! Verification type definitions
//!
//! Verifications are quality gates that check task completion before allowing closure.
//! A Haiku subagent reviews the work and approves or rejects based on completeness.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::TypeError;

const VERIFIER_SUMMARY_MAX_BYTES: usize = 4_096;
const VERIFIER_ISSUE_FILE_MAX_BYTES: usize = 512;
const VERIFIER_ISSUE_CATEGORY_MAX_BYTES: usize = 128;
const VERIFIER_ISSUE_TEXT_MAX_BYTES: usize = 4_096;
const VERIFIER_FILES_REVIEWED_MAX: usize = 256;
const VERIFIER_ISSUES_MAX: usize = 128;
const REDACTED_VERIFIER_SECRET: &str = "[REDACTED_SECRET]";
const REDACTED_VERIFIER_PATH: &str = "[REDACTED_PATH]";
const REDACTED_VERIFIER_CONTROL: &str = "[REDACTED_CONTROL]";
const TRUNCATED_VERIFIER_TEXT: &str = "[TRUNCATED]";

/// Literal secret headers that are unsafe wherever they appear, with no
/// surrounding token boundary required.
const VERIFIER_SECRET_PHRASES: &[&str] = &[
    "-----begin private key",
    "-----begin rsa private key",
    "-----begin ec private key",
];

/// Credential-bearing key names. Each is unsafe when followed — across any
/// amount of ASCII/Unicode whitespace — by an `=` or `:` separator, so
/// `token=x`, `token =x`, `token :x`, and `token\t= x` are all caught.
const VERIFIER_SECRET_KEYS: &[&str] = &[
    "authorization",
    "token",
    "access_token",
    "refresh_token",
    "password",
    "passwd",
    "secret",
    "client_secret",
    "api_key",
    "apikey",
    "private_key",
];

/// Vendor credential prefixes that are unsafe at any token boundary.
const VERIFIER_SECRET_PREFIXES: &[&str] = &[
    "vcap-",
    "sk-",
    "ghp_",
    "gho_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "aiza",
];

/// Minimum alphanumeric run required after an `AKIA` boundary match before it
/// is treated as an AWS access key id rather than an ordinary word.
const VERIFIER_AKIA_MIN_RUN: usize = 16;

/// Lowercase a value and collapse every Unicode whitespace form to a single
/// space.
///
/// Marker detection then sees `Bearer\tsecret`, `Bearer\nsecret`, and
/// `Bearer\u{a0}secret` identically to `Bearer secret`, so separator
/// obfuscation cannot smuggle an auth marker past the boundary. Scanning
/// happens over `char`s (not bytes) so multi-byte input cannot desynchronize
/// the neighbour checks.
fn verifier_scan_chars(value: &str) -> Vec<char> {
    value
        .chars()
        .map(|ch| {
            if ch.is_whitespace() {
                ' '
            } else {
                ch.to_ascii_lowercase()
            }
        })
        .collect()
}

/// Characters that continue an identifier, and therefore suppress a token
/// boundary before them.
fn is_verifier_identifier_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

/// True when `index` starts a fresh token (string start, or preceded by a
/// non-identifier character). This is what lets an embedded `at=AKIA...` or
/// `](C:\...)` be seen while `task-verifier` still does not match `sk-`.
fn at_verifier_token_boundary(chars: &[char], index: usize) -> bool {
    index == 0 || !is_verifier_identifier_char(chars[index - 1])
}

/// True when `needle` occurs at exactly `index`.
fn verifier_matches_at(chars: &[char], index: usize, needle: &str) -> bool {
    let mut cursor = index;
    for expected in needle.chars() {
        if chars.get(cursor) != Some(&expected) {
            return false;
        }
        cursor += 1;
    }
    true
}

/// True when `key` occurs at `index` and is followed, across any run of
/// normalized spaces, by an `=` or `:` separator.
fn verifier_key_has_separator(chars: &[char], index: usize, key: &str) -> bool {
    if !verifier_matches_at(chars, index, key) {
        return false;
    }
    let mut cursor = index + key.chars().count();
    while chars.get(cursor) == Some(&' ') {
        cursor += 1;
    }
    matches!(chars.get(cursor), Some('=') | Some(':'))
}

/// True when a `bearer` boundary match is followed by a separator, i.e. it
/// introduces a credential value rather than merely ending the text.
fn verifier_bearer_has_separator(chars: &[char], index: usize) -> bool {
    matches!(chars.get(index), Some(' ') | Some(':') | Some('='))
}

/// True when at least `VERIFIER_AKIA_MIN_RUN` alphanumerics run from `index`.
fn verifier_has_key_id_run(chars: &[char], index: usize) -> bool {
    let mut run = 0usize;
    let mut cursor = index;
    while chars
        .get(cursor)
        .is_some_and(|ch| ch.is_ascii_alphanumeric())
    {
        run += 1;
        cursor += 1;
    }
    run >= VERIFIER_AKIA_MIN_RUN
}

fn verifier_text_contains_secret(value: &str) -> bool {
    let chars = verifier_scan_chars(value);

    (0..chars.len()).any(|index| {
        if VERIFIER_SECRET_PHRASES
            .iter()
            .any(|phrase| verifier_matches_at(&chars, index, phrase))
        {
            return true;
        }

        if !at_verifier_token_boundary(&chars, index) {
            return false;
        }

        if verifier_matches_at(&chars, index, "bearer")
            && verifier_bearer_has_separator(&chars, index + "bearer".len())
        {
            return true;
        }

        if VERIFIER_SECRET_KEYS
            .iter()
            .any(|key| verifier_key_has_separator(&chars, index, key))
        {
            return true;
        }

        if VERIFIER_SECRET_PREFIXES
            .iter()
            .any(|prefix| verifier_matches_at(&chars, index, prefix))
        {
            return true;
        }

        verifier_matches_at(&chars, index, "akia") && verifier_has_key_id_run(&chars, index)
    })
}

/// Characters that can begin the first segment of a path body. Requiring one
/// of these after a root `/` keeps prose like `a / b` out of the path class.
fn is_verifier_path_body_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '~' | '%' | '+')
}

/// True when a `/` at this position is a filesystem root rather than a
/// separator inside a relative path or a URL authority.
///
/// Rejects alphanumeric predecessors (`src/main.rs`), a preceding `/`
/// (`https://host`), and relative-path punctuation (`./x`, `../x`), while
/// accepting assignment, quoting, and Markdown punctuation (`at=/home/x`,
/// `"/home/x"`, `](/home/x)`).
fn is_verifier_unix_root_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(ch) => {
            !ch.is_ascii_alphanumeric() && !matches!(ch, '/' | '.' | '_' | '-' | '~' | '+' | '%')
        }
    }
}

/// True when `\\host\` begins a UNC share at `index`.
///
/// The trailing share separator is required so escaped snippets such as
/// `"a\\nb"` are not misread as host paths.
fn is_verifier_unc_share(chars: &[char], index: usize) -> bool {
    if !(chars.get(index) == Some(&'\\') && chars.get(index + 1) == Some(&'\\')) {
        return false;
    }
    let mut cursor = index + 2;
    let mut host = 0usize;
    while chars
        .get(cursor)
        .is_some_and(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    {
        host += 1;
        cursor += 1;
    }
    host > 0 && chars.get(cursor) == Some(&'\\')
}

/// True when a `X:/` or `X:\` drive-letter path begins at `index`.
///
/// The boundary requirement keeps `https://` and `12:30` out while still
/// seeing an embedded `at=C:\Users\operator`.
fn is_verifier_drive_letter(chars: &[char], index: usize) -> bool {
    chars.get(index).is_some_and(|ch| ch.is_ascii_alphabetic())
        && at_verifier_token_boundary(chars, index)
        && chars.get(index + 1) == Some(&':')
        && matches!(chars.get(index + 2), Some('/') | Some('\\'))
}

fn verifier_text_contains_absolute_path(value: &str) -> bool {
    let chars = verifier_scan_chars(value);

    (0..chars.len()).any(|index| {
        let ch = chars[index];
        let prev = index.checked_sub(1).map(|prev_index| chars[prev_index]);

        if ch == '/'
            && is_verifier_unix_root_boundary(prev)
            && chars
                .get(index + 1)
                .copied()
                .is_some_and(is_verifier_path_body_char)
        {
            return true;
        }

        if ch == '~'
            && chars.get(index + 1) == Some(&'/')
            && at_verifier_token_boundary(&chars, index)
        {
            return true;
        }

        if is_verifier_unc_share(&chars, index) || is_verifier_drive_letter(&chars, index) {
            return true;
        }

        at_verifier_token_boundary(&chars, index) && verifier_matches_at(&chars, index, "file:/")
    })
}

fn verifier_text_contains_unsafe_control(value: &str) -> bool {
    value.chars().any(|ch| {
        (ch.is_control() && !matches!(ch, '\n' | '\t'))
            || matches!(
                ch,
                '\u{200b}'
                    | '\u{200c}'
                    | '\u{200d}'
                    | '\u{202a}'..='\u{202e}'
                    | '\u{2066}'..='\u{2069}'
                    | '\u{feff}'
            )
    })
}

fn truncate_verifier_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }

    let keep = max_bytes.saturating_sub(TRUNCATED_VERIFIER_TEXT.len());
    let mut end = keep.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &value[..end], TRUNCATED_VERIFIER_TEXT)
}

fn sanitize_verifier_text(value: &str, max_bytes: usize) -> String {
    if verifier_text_contains_secret(value) {
        REDACTED_VERIFIER_SECRET.to_string()
    } else if verifier_text_contains_absolute_path(value) {
        REDACTED_VERIFIER_PATH.to_string()
    } else if verifier_text_contains_unsafe_control(value) {
        REDACTED_VERIFIER_CONTROL.to_string()
    } else {
        truncate_verifier_text(value, max_bytes)
    }
}

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
    /// Exact durable verification dispatch (and therefore proof cycle).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_id: Option<String>,
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

/// Lifecycle state for one explicit verification dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VerificationDispatchState {
    /// A verifier owner has been assigned but no child has claimed the work.
    #[default]
    Pending,
    /// A distinct task-verifier child claimed the dispatch.
    Claimed,
    /// The deadline elapsed without a verdict.
    TimedOut,
    /// A legitimate verifier or supervisor recorded a verdict.
    Resolved,
    /// A later task lifecycle explicitly started a new proof cycle.
    Invalidated,
}

impl fmt::Display for VerificationDispatchState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerificationDispatchState::Pending => write!(f, "pending"),
            VerificationDispatchState::Claimed => write!(f, "claimed"),
            VerificationDispatchState::TimedOut => write!(f, "timed_out"),
            VerificationDispatchState::Resolved => write!(f, "resolved"),
            VerificationDispatchState::Invalidated => write!(f, "invalidated"),
        }
    }
}

impl FromStr for VerificationDispatchState {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pending" => Ok(VerificationDispatchState::Pending),
            "claimed" => Ok(VerificationDispatchState::Claimed),
            "timed_out" => Ok(VerificationDispatchState::TimedOut),
            "resolved" => Ok(VerificationDispatchState::Resolved),
            "invalidated" => Ok(VerificationDispatchState::Invalidated),
            _ => Err(TypeError::Parse(format!(
                "invalid verification dispatch state: {s}"
            ))),
        }
    }
}

/// Recovery path advertised when a verification dispatch misses its deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VerificationRecoveryAction {
    /// The registered supervisor must re-dispatch a verifier or record a
    /// direct verdict using server-derived supervisor authority.
    #[default]
    SupervisorRedispatchOrDirect,
}

impl fmt::Display for VerificationRecoveryAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerificationRecoveryAction::SupervisorRedispatchOrDirect => {
                write!(f, "supervisor_redispatch_or_direct")
            }
        }
    }
}

impl FromStr for VerificationRecoveryAction {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "supervisor_redispatch_or_direct" => {
                Ok(VerificationRecoveryAction::SupervisorRedispatchOrDirect)
            }
            _ => Err(TypeError::Parse(format!(
                "invalid verification recovery action: {s}"
            ))),
        }
    }
}

/// Immutable snapshot of the repository state inspected by a verifier.
///
/// The digest covers the exact worktree contents (including untracked files)
/// while deliberately excluding CAS's own `.cas` state. The canonical paths
/// prevent a proof captured in one worktree from being replayed in another.
///
/// `anchor_commits` names the delivered commit identity this proof cycle is
/// really about (cas-5c33): the resolved commit receipt and, when the worktree
/// carries work beyond its integration base, the delivered tip. Delivered
/// content is immutable under its commit id, so a worker that later merges or
/// fast-forwards its branch — to start the next task — moves `head_commit`
/// without touching what the verifier reviewed. When anchors are bound, a
/// proof holds while every anchor stays reachable; when the list is empty
/// (nothing delivered yet) the whole boundary must still match exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryProofBoundary {
    pub repository_root: String,
    pub worktree_root: String,
    pub head_commit: String,
    pub state_digest: String,
    /// Empty for pre-cas-5c33 rows and for closes with nothing delivered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anchor_commits: Vec<String>,
}

/// Immutable external identity attached to a verification proof cycle.
///
/// A task-only close has neither delivery field; the dispatch ID itself is
/// still the exact proof boundary. Delivery verification binds both IDs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationProofBoundary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_transaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<RepositoryProofBoundary>,
}

impl VerificationProofBoundary {
    pub fn task() -> Self {
        Self::default()
    }

    pub fn delivery(receipt_id: String, delivery_transaction_id: String) -> Self {
        Self {
            receipt_id: Some(receipt_id),
            delivery_transaction_id: Some(delivery_transaction_id),
            repository: None,
        }
    }

    pub fn task_at(repository: RepositoryProofBoundary) -> Self {
        Self {
            repository: Some(repository),
            ..Self::default()
        }
    }
}

/// Durable task-scoped verification work assignment.
///
/// This record is the forcing-function state. It is separate from verdict
/// rows so unrelated work can continue while one exact task transition waits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationDispatch {
    pub id: String,
    pub task_id: String,
    /// Exact immutable delivery receipt, when this dispatch verifies delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<String>,
    /// Exact delivery transaction advanced by this dispatch's verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_transaction_id: Option<String>,
    /// Exact repository state inspected by a legacy task verifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<RepositoryProofBoundary>,
    pub requester_agent_id: String,
    pub owner_agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_id: Option<String>,
    #[serde(default)]
    pub state: VerificationDispatchState,
    pub requested_at: DateTime<Utc>,
    pub deadline_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub recovery_action: VerificationRecoveryAction,
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

    /// Exact durable dispatch whose proof boundary this verdict resolves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_id: Option<String>,

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
            provenance: VerificationProvenance::Legacy,
            capability_id: None,
            dispatch_id: None,
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
            provenance: VerificationProvenance::Legacy,
            capability_id: None,
            dispatch_id: None,
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
            provenance: VerificationProvenance::Legacy,
            capability_id: None,
            dispatch_id: None,
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
            provenance: VerificationProvenance::Legacy,
            capability_id: None,
            dispatch_id: None,
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
            provenance: VerificationProvenance::Legacy,
            capability_id: None,
            dispatch_id: None,
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

    /// Sanitize only caller-authored verifier payload fields.
    ///
    /// Server-derived identity, authority, dispatch, status, confidence, and
    /// timestamp fields are intentionally untouched. Clean text remains
    /// byte-for-byte identical; unsafe or oversized text is replaced or
    /// truncated deterministically before persistence or diagnostics.
    pub fn sanitize_verifier_authored_content(&mut self) {
        self.summary = sanitize_verifier_text(&self.summary, VERIFIER_SUMMARY_MAX_BYTES);

        self.issues.truncate(VERIFIER_ISSUES_MAX);
        for issue in &mut self.issues {
            issue.file = sanitize_verifier_text(&issue.file, VERIFIER_ISSUE_FILE_MAX_BYTES);
            issue.category =
                sanitize_verifier_text(&issue.category, VERIFIER_ISSUE_CATEGORY_MAX_BYTES);
            issue.code = sanitize_verifier_text(&issue.code, VERIFIER_ISSUE_TEXT_MAX_BYTES);
            issue.problem = sanitize_verifier_text(&issue.problem, VERIFIER_ISSUE_TEXT_MAX_BYTES);
            issue.suggestion = issue
                .suggestion
                .as_deref()
                .map(|value| sanitize_verifier_text(value, VERIFIER_ISSUE_TEXT_MAX_BYTES));
        }

        self.files_reviewed.truncate(VERIFIER_FILES_REVIEWED_MAX);
        for file in &mut self.files_reviewed {
            *file = sanitize_verifier_text(file, VERIFIER_ISSUE_FILE_MAX_BYTES);
        }
    }
}

impl Default for Verification {
    fn default() -> Self {
        Self {
            id: String::new(),
            task_id: String::new(),
            agent_id: None,
            verification_type: VerificationType::Task,
            provenance: VerificationProvenance::Legacy,
            capability_id: None,
            dispatch_id: None,
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

    #[test]
    fn verifier_authored_safe_content_is_byte_compatible() {
        let mut verification = Verification::rejected(
            "ver-safe".to_string(),
            "cas-safe".to_string(),
            "REJECTED: task-verifier found two ordinary findings\n\nPlease fix both.".to_string(),
            vec![VerificationIssue::blocking(
                "src/main.rs".to_string(),
                Some(42),
                "todo_comment".to_string(),
                "// TODO: validate".to_string(),
                "Function lacks input validation".to_string(),
                Some("Add validation for required fields.".to_string()),
            )],
        );
        verification.files_reviewed = vec!["src/main.rs".to_string(), "tests/api.rs".to_string()];
        let before = serde_json::to_vec(&verification).expect("serialize safe verification");

        verification.sanitize_verifier_authored_content();

        assert_eq!(
            serde_json::to_vec(&verification).expect("serialize sanitized verification"),
            before
        );
    }

    #[test]
    fn verifier_authored_unsafe_content_is_redacted_and_bounded() {
        let raw_capability =
            "vcap-0123456789abcdef0123456789abcdef.0123456789abcdef0123456789abcdef";
        let mut verification = Verification::rejected(
            "ver-unsafe".to_string(),
            "cas-unsafe".to_string(),
            format!("approved with {raw_capability}"),
            vec![
                VerificationIssue::blocking(
                    "/home/operator/private.rs".to_string(),
                    Some(7),
                    "security".to_string(),
                    "Authorization: Bearer private-value".to_string(),
                    "control\u{1b}[31msequence".to_string(),
                    Some("password=hunter2".to_string()),
                ),
                VerificationIssue::warning(
                    "src/lib.rs".to_string(),
                    "x".repeat(VERIFIER_ISSUE_CATEGORY_MAX_BYTES + 40),
                    "p".repeat(VERIFIER_ISSUE_TEXT_MAX_BYTES + 40),
                ),
            ],
        );
        verification.files_reviewed = vec![
            r"C:\Users\operator\private.rs".to_string(),
            "src/lib.rs".to_string(),
        ];
        verification.sanitize_verifier_authored_content();

        assert_eq!(verification.summary, REDACTED_VERIFIER_SECRET);
        assert_eq!(verification.issues[0].file, REDACTED_VERIFIER_PATH);
        assert_eq!(verification.issues[0].code, REDACTED_VERIFIER_SECRET);
        assert_eq!(verification.issues[0].problem, REDACTED_VERIFIER_CONTROL);
        assert_eq!(
            verification.issues[0].suggestion.as_deref(),
            Some(REDACTED_VERIFIER_SECRET)
        );
        assert_eq!(verification.files_reviewed[0], REDACTED_VERIFIER_PATH);
        assert_eq!(verification.files_reviewed[1], "src/lib.rs");
        assert!(verification.issues[1].category.len() <= VERIFIER_ISSUE_CATEGORY_MAX_BYTES);
        assert!(verification.issues[1].problem.len() <= VERIFIER_ISSUE_TEXT_MAX_BYTES);

        let serialized = serde_json::to_string(&verification).expect("serialize");
        for unsafe_value in [
            raw_capability,
            "/home/operator",
            r"C:\Users\operator",
            "private-value",
            "hunter2",
            "\u{1b}",
        ] {
            assert!(
                !serialized.contains(unsafe_value),
                "unsafe verifier content crossed sanitization: {unsafe_value:?}"
            );
        }
    }

    #[test]
    fn verifier_authored_clean_collection_and_text_bounds_are_utf8_safe() {
        let mut verification = Verification::approved(
            "ver-bounded".to_string(),
            "cas-bounded".to_string(),
            "é".repeat(VERIFIER_SUMMARY_MAX_BYTES),
        );
        verification.issues = (0..VERIFIER_ISSUES_MAX + 10)
            .map(|index| {
                VerificationIssue::warning(
                    format!("src/file-{index}.rs"),
                    "style".to_string(),
                    "p".repeat(VERIFIER_ISSUE_TEXT_MAX_BYTES + 10),
                )
            })
            .collect();
        verification.files_reviewed = (0..VERIFIER_FILES_REVIEWED_MAX + 10)
            .map(|index| format!("src/file-{index}.rs"))
            .collect();

        verification.sanitize_verifier_authored_content();

        assert_eq!(verification.issues.len(), VERIFIER_ISSUES_MAX);
        assert_eq!(
            verification.files_reviewed.len(),
            VERIFIER_FILES_REVIEWED_MAX
        );
        assert!(verification.summary.len() <= VERIFIER_SUMMARY_MAX_BYTES);
        assert!(
            verification
                .summary
                .is_char_boundary(verification.summary.len())
        );
        assert!(verification.summary.ends_with(TRUNCATED_VERIFIER_TEXT));
        assert!(
            verification
                .issues
                .iter()
                .all(|issue| issue.problem.len() <= VERIFIER_ISSUE_TEXT_MAX_BYTES)
        );
    }

    /// cas-da92: absolute host paths must be caught wherever they appear, not
    /// only when they begin a whitespace-delimited token.
    #[test]
    fn verifier_embedded_absolute_paths_are_redacted() {
        for value in [
            "at=/home/operator/private.rs",
            "proof at=/home/operator/private.rs line 12",
            "see [proof](/home/operator/private.rs) for detail",
            "<file:///etc/shadow>",
            "evidence=file:///etc/shadow",
            "FILE:///etc/shadow",
            r"at=C:\Users\operator\private.rs",
            r"[proof](C:/Users/operator/private.rs)",
            r"share=\\build-host\proofs\private.rs",
            "home=~/private/proof.json",
            "path:/etc/passwd",
            "\"/home/operator/private.rs\"",
            "{'file': '/home/operator/private.rs'}",
        ] {
            assert_eq!(
                sanitize_verifier_text(value, VERIFIER_ISSUE_TEXT_MAX_BYTES),
                REDACTED_VERIFIER_PATH,
                "embedded absolute path was not redacted: {value:?}"
            );
        }
    }

    /// cas-da92: auth markers must survive separator obfuscation across every
    /// ASCII/Unicode whitespace form.
    #[test]
    fn verifier_separator_obfuscated_secrets_are_redacted() {
        for value in [
            "Bearer\tsecret-material",
            "Bearer\nsecret-material",
            "Bearer\r\nsecret-material",
            "Bearer\u{a0}secret-material",
            "Bearer   secret-material",
            "Bearer: secret-material",
            "authorization :\tsecret-material",
            "token = secret-material",
            "password :\tsecret-material",
            "api_key\t=\tsecret-material",
            "client_secret\n= secret-material",
            "at=AKIAIOSFODNN7EXAMPLE",
            "creds(AKIAIOSFODNN7EXAMPLE)",
            "-----BEGIN RSA PRIVATE KEY-----",
        ] {
            assert_eq!(
                sanitize_verifier_text(value, VERIFIER_ISSUE_TEXT_MAX_BYTES),
                REDACTED_VERIFIER_SECRET,
                "obfuscated secret was not redacted: {value:?}"
            );
        }
    }

    /// cas-da92: the widened detectors must not swallow portable, benign
    /// identifiers that verifiers legitimately author.
    #[test]
    fn verifier_portable_identifiers_are_not_false_positives() {
        for value in [
            "src/main.rs",
            "crates/cas-types/src/verification.rs:118",
            "docs/release-notes/2026-07-31-topic-slack.md",
            "./relative/path.rs",
            "../sibling/path.rs",
            "https://example.com/docs/a",
            "http://example.com/docs/a?q=1",
            "read/write access is required",
            "either and/or is fine",
            "the ratio a / b is unstable",
            "meeting at 12:30 today",
            "ver-a1b2 verified cas-1234",
            "task-verifier found two ordinary findings",
            "println!(\"line\\nbreak\")",
            "TODO: add validation for required fields",
            "AKIA is mentioned without a key id",
            "100 km/h",
        ] {
            assert_eq!(
                sanitize_verifier_text(value, VERIFIER_ISSUE_TEXT_MAX_BYTES),
                value,
                "benign verifier text was falsely redacted: {value:?}"
            );
        }
    }

    /// cas-da92: embedded forms must not reach the serialized workspace that
    /// persistence and diagnostics both project from.
    #[test]
    fn verifier_embedded_forms_never_reach_serialized_workspace() {
        let mut verification = Verification::rejected(
            "ver-embedded".to_string(),
            "cas-embedded".to_string(),
            "evidence at=/home/operator/private.rs".to_string(),
            vec![VerificationIssue::blocking(
                "[proof](/home/operator/private.rs)".to_string(),
                Some(12),
                "security".to_string(),
                "Bearer\tsecret-material".to_string(),
                "token = secret-material".to_string(),
                Some(r"share=\\build-host\proofs\private.rs".to_string()),
            )],
        );
        verification.files_reviewed = vec![
            "evidence=file:///etc/shadow".to_string(),
            r"at=C:\Users\operator\private.rs".to_string(),
            "src/lib.rs".to_string(),
        ];

        verification.sanitize_verifier_authored_content();

        assert_eq!(verification.summary, REDACTED_VERIFIER_PATH);
        assert_eq!(verification.issues[0].file, REDACTED_VERIFIER_PATH);
        assert_eq!(verification.issues[0].code, REDACTED_VERIFIER_SECRET);
        assert_eq!(verification.issues[0].problem, REDACTED_VERIFIER_SECRET);
        assert_eq!(
            verification.issues[0].suggestion.as_deref(),
            Some(REDACTED_VERIFIER_PATH)
        );
        assert_eq!(
            verification.files_reviewed,
            vec![
                REDACTED_VERIFIER_PATH.to_string(),
                REDACTED_VERIFIER_PATH.to_string(),
                "src/lib.rs".to_string(),
            ]
        );

        let serialized = serde_json::to_string(&verification).expect("serialize");
        for unsafe_value in [
            "/home/operator",
            "/etc/shadow",
            r"C:\Users\operator",
            r"\\build-host",
            "secret-material",
        ] {
            assert!(
                !serialized.contains(unsafe_value),
                "embedded verifier content crossed sanitization: {unsafe_value:?}"
            );
        }
    }
}
