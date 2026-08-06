//! Multi-persona code review findings schema.
//!
//! This module defines the structured contract that all eight reviewer
//! personas (correctness, testing, maintainability, project-standards,
//! fallow, security, performance, adversarial) emit, and that the
//! orchestrator merges, dedupes, and routes.
//!
//! The shape is fixed by the Phase 1 subsystem A brainstorm
//! (`docs/brainstorms/2026-04-09-multi-persona-code-review-requirements.md`,
//! requirement R3). A corresponding human-readable schema reference lives at
//! `cas-cli/src/builtins/skills/cas-code-review/references/findings-schema.md`
//! and is embedded into the `cas-code-review` skill by Unit 6.
//!
//! # Validation
//!
//! Deserializing a `Finding` or `ReviewerOutput` via serde performs *structural*
//! validation (unknown enum variants are rejected by `#[serde(deny_unknown_fields)]`
//! and the `rename_all` tags). Semantic validation — title length, confidence
//! bounds, non-empty evidence, relative file path — is enforced by calling
//! [`Finding::validate`] / [`ReviewerOutput::validate`] or the convenience
//! [`parse_reviewer_output`] helper which does both in one step.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Maximum title length in characters (per R3).
pub const MAX_TITLE_LEN: usize = 100;

/// Severity level assigned by a reviewer persona.
///
/// Mapped 1:1 to CAS task priorities by the review-to-task flow:
/// `P0→0, P1→1, P2→2, P3→3`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Severity {
    /// Blocker. Hard-blocks `task.close` in autofix mode.
    P0,
    /// High.
    P1,
    /// Medium.
    P2,
    /// Low.
    P3,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Severity::P0 => "P0",
            Severity::P1 => "P1",
            Severity::P2 => "P2",
            Severity::P3 => "P3",
        };
        f.write_str(s)
    }
}

/// Autofix routing class — how the orchestrator should handle the finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutofixClass {
    /// Safe to apply automatically by the fixer sub-agent.
    SafeAuto,
    /// Can be auto-applied, but only behind an explicit gate / human ack.
    GatedAuto,
    /// Requires a human or downstream agent to edit code.
    Manual,
    /// Informational only; never becomes a task, never auto-applied.
    Advisory,
}

impl fmt::Display for AutofixClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            AutofixClass::SafeAuto => "safe_auto",
            AutofixClass::GatedAuto => "gated_auto",
            AutofixClass::Manual => "manual",
            AutofixClass::Advisory => "advisory",
        };
        f.write_str(s)
    }
}

/// Who owns resolving a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Owner {
    /// The fixer sub-agent inside cas-code-review.
    ReviewFixer,
    /// A downstream CAS task (review-to-task flow).
    DownstreamResolver,
    /// A human operator — must not be routed to automation.
    Human,
}

impl fmt::Display for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Owner::ReviewFixer => "review-fixer",
            Owner::DownstreamResolver => "downstream-resolver",
            Owner::Human => "human",
        };
        f.write_str(s)
    }
}

/// A single code-review finding emitted by a persona.
///
/// All fields required by the schema in R3 are present. Use
/// [`Finding::validate`] after deserialization to enforce the semantic rules
/// that serde cannot express.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    /// Short human-readable label (≤ [`MAX_TITLE_LEN`] characters).
    pub title: String,
    /// Severity (P0..P3).
    pub severity: Severity,
    /// File path, **relative** to the repository root.
    pub file: String,
    /// 1-based line number of the finding.
    pub line: u32,
    /// Why this matters — the consequence if unaddressed.
    pub why_it_matters: String,
    /// How the orchestrator should handle the fix.
    pub autofix_class: AutofixClass,
    /// Who owns resolving the finding.
    pub owner: Owner,
    /// Reviewer confidence in the finding, 0.0..=1.0 inclusive.
    pub confidence: f32,
    /// Code-grounded evidence strings. Must contain at least one entry.
    pub evidence: Vec<String>,
    /// True if the finding exists on `main` / pre-diff; false if introduced
    /// by the change under review.
    pub pre_existing: bool,
    /// Optional concrete suggested fix (diff, patch hint, or prose).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_fix: Option<String>,
    /// Whether the finding requires re-verification after a fix.
    #[serde(default)]
    pub requires_verification: bool,
}

/// Errors returned by [`Finding::validate`] / [`ReviewerOutput::validate`].
#[derive(Debug, Clone, PartialEq)]
pub enum FindingValidationError {
    /// Title is empty.
    EmptyTitle,
    /// Title exceeds [`MAX_TITLE_LEN`].
    TitleTooLong { len: usize, max: usize },
    /// `confidence` is NaN or outside 0.0..=1.0.
    ConfidenceOutOfRange { value: f32 },
    /// `evidence` array is empty.
    EmptyEvidence,
    /// A single evidence string is empty / whitespace.
    EmptyEvidenceEntry { index: usize },
    /// `file` is empty.
    EmptyFilePath,
    /// `file` is an absolute path.
    AbsoluteFilePath { path: String },
    /// `why_it_matters` is empty.
    EmptyWhyItMatters,
    /// Reviewer name is empty.
    EmptyReviewerName,
}

impl fmt::Display for FindingValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTitle => write!(f, "finding title is empty"),
            Self::TitleTooLong { len, max } => {
                write!(f, "finding title is {len} chars, max is {max}")
            }
            Self::ConfidenceOutOfRange { value } => {
                write!(f, "confidence {value} out of range 0.0..=1.0")
            }
            Self::EmptyEvidence => write!(f, "finding evidence array is empty"),
            Self::EmptyEvidenceEntry { index } => {
                write!(f, "finding evidence[{index}] is empty")
            }
            Self::EmptyFilePath => write!(f, "finding file path is empty"),
            Self::AbsoluteFilePath { path } => {
                write!(f, "finding file path must be relative, got '{path}'")
            }
            Self::EmptyWhyItMatters => write!(f, "finding why_it_matters is empty"),
            Self::EmptyReviewerName => write!(f, "reviewer name is empty"),
        }
    }
}

impl std::error::Error for FindingValidationError {}

fn is_absolute_path(path: &str) -> bool {
    // Treat Unix absolute paths and Windows drive/UNC paths as absolute.
    // Relative paths like `src/foo.rs` or `./src/foo.rs` are fine.
    if path.starts_with('/') || path.starts_with('\\') {
        return true;
    }
    // Windows drive: `C:\...` or `C:/...`
    let bytes = path.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
    {
        return true;
    }
    false
}

impl Finding {
    /// Enforce the semantic rules that serde cannot express.
    pub fn validate(&self) -> Result<(), FindingValidationError> {
        if self.title.trim().is_empty() {
            return Err(FindingValidationError::EmptyTitle);
        }
        let title_len = self.title.chars().count();
        if title_len > MAX_TITLE_LEN {
            return Err(FindingValidationError::TitleTooLong {
                len: title_len,
                max: MAX_TITLE_LEN,
            });
        }
        if !(self.confidence.is_finite() && (0.0..=1.0).contains(&self.confidence)) {
            return Err(FindingValidationError::ConfidenceOutOfRange {
                value: self.confidence,
            });
        }
        if self.evidence.is_empty() {
            return Err(FindingValidationError::EmptyEvidence);
        }
        for (i, e) in self.evidence.iter().enumerate() {
            if e.trim().is_empty() {
                return Err(FindingValidationError::EmptyEvidenceEntry { index: i });
            }
        }
        if self.file.trim().is_empty() {
            return Err(FindingValidationError::EmptyFilePath);
        }
        if is_absolute_path(&self.file) {
            return Err(FindingValidationError::AbsoluteFilePath {
                path: self.file.clone(),
            });
        }
        if self.why_it_matters.trim().is_empty() {
            return Err(FindingValidationError::EmptyWhyItMatters);
        }
        Ok(())
    }
}

impl fmt::Display for Finding {
    /// Orchestrator-friendly single-finding rendering.
    ///
    /// Format: `[P0] title — file:line (autofix_class, owner, conf=0.87)`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{sev}] {title} — {file}:{line} ({class}, {owner}, conf={conf:.2})",
            sev = self.severity,
            title = self.title,
            file = self.file,
            line = self.line,
            class = self.autofix_class,
            owner = self.owner,
            conf = self.confidence,
        )
    }
}

/// The full envelope a single persona returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewerOutput {
    /// Reviewer persona name, e.g. `"correctness"`, `"security"`.
    pub reviewer: String,
    /// Findings emitted by this reviewer (may be empty).
    #[serde(default)]
    pub findings: Vec<Finding>,
    /// Risks the reviewer saw but could not confirm — surfaced to the
    /// orchestrator but never turned into tasks automatically.
    #[serde(default)]
    pub residual_risks: Vec<String>,
    /// Testing gaps the reviewer noticed (e.g., "no test covers the retry path").
    #[serde(default)]
    pub testing_gaps: Vec<String>,
    /// Non-empty when the reviewer persona did not actually run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_reason: Option<String>,
}

impl ReviewerOutput {
    /// Validate every contained finding and the envelope fields themselves.
    pub fn validate(&self) -> Result<(), FindingValidationError> {
        if self.reviewer.trim().is_empty() {
            return Err(FindingValidationError::EmptyReviewerName);
        }
        for f in &self.findings {
            f.validate()?;
        }
        Ok(())
    }
}

impl fmt::Display for ReviewerOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "== reviewer: {} ({} findings) ==",
            self.reviewer,
            self.findings.len()
        )?;
        for finding in &self.findings {
            writeln!(f, "  {finding}")?;
        }
        if !self.residual_risks.is_empty() {
            writeln!(f, "  residual_risks:")?;
            for r in &self.residual_risks {
                writeln!(f, "    - {r}")?;
            }
        }
        if !self.testing_gaps.is_empty() {
            writeln!(f, "  testing_gaps:")?;
            for g in &self.testing_gaps {
                writeln!(f, "    - {g}")?;
            }
        }
        if let Some(reason) = &self.skipped_reason {
            writeln!(f, "  skipped_reason: {reason}")?;
        }
        Ok(())
    }
}

/// Parse a JSON string into a [`ReviewerOutput`] and run semantic validation.
///
/// Returns a boxed error so callers can mix serde and validation errors freely.
pub fn parse_reviewer_output(json: &str) -> Result<ReviewerOutput, Box<dyn std::error::Error>> {
    let out: ReviewerOutput = serde_json::from_str(json)?;
    out.validate()?;
    Ok(out)
}

/// End-to-end review outcome that the cas-code-review skill passes back
/// to the `task.close` MCP handler via `TaskCloseRequest.code_review_findings`.
///
/// This is the structured envelope the worker assembles from the
/// orchestrator → merge → autofix → review-to-task pipeline before it
/// retries `task.close`. The close-gate logic in close_ops.rs reads
/// `residual` to decide whether to P0-block the close.
///
/// The shape is intentionally thin — the heavy lifting (dedup, routing,
/// confidence gating, autofix) is already done by the time the worker
/// emits this envelope. Here we only need what the gate and the audit
/// trail depend on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewOutcome {
    /// Findings that survived the autofix loop — i.e., anything the
    /// fixer sub-agent did not resolve within the bounded 2-round
    /// budget. The close gate blocks on any `severity == P0` entry
    /// that is not `pre_existing`.
    #[serde(default)]
    pub residual: Vec<Finding>,
    /// Findings that existed on the base ref, surfaced for context but
    /// never blocking. Separated from `residual` per R4 / R9.
    #[serde(default)]
    pub pre_existing: Vec<Finding>,
    /// Invocation mode the review ran in: `"autofix"`, `"interactive"`,
    /// `"report-only"`, or `"headless"`. Required so the audit trail
    /// and downstream consumers can tell whether the envelope came
    /// from the primary close-gate path or an out-of-band invocation.
    pub mode: String,
    /// cas-acf83 (GH #108): what the review actually DID.
    ///
    /// Without this, "every persona ran and found nothing" and "no persona
    /// ever launched" are the same envelope — `residual: []` — and the close
    /// gate, which only looks for P0s, accepts both. That is not a
    /// hypothetical: when the Codex transport ran out of credits, every
    /// persona skipped, the workflow returned `residual: []` with
    /// `personas_run: 0`, and the gate waved it through. Two workers caught it
    /// by hand; their voluntary re-reviews found a workspace-build break and a
    /// P0 regression that had already passed the gate.
    ///
    /// `None` means the producer reported nothing about execution. That is
    /// NOT the same as "it ran" — see [`ReviewOutcome::execution_status`].
    #[serde(default)]
    pub execution: Option<ReviewExecution>,
}

/// What a review run actually executed (cas-acf83 / GH #108).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReviewExecution {
    /// Personas that produced a verdict. Zero means no review happened,
    /// whatever the findings arrays say.
    pub personas_run: u32,
    /// Personas that were dispatched but never produced a verdict, with the
    /// reason. Named so a launch failure is diagnosable from the envelope
    /// alone instead of requiring the workflow transcript.
    #[serde(default)]
    pub personas_failed: Vec<String>,
    /// Why the review declined to run at all (empty diff, missing base SHA,
    /// invalid shard coverage, …). Present only when nothing ran.
    #[serde(default)]
    pub skipped_reason: Option<String>,
    /// Mandatory personas that produced no verdict (cas-acf83).
    ///
    /// `personas_run > 0` alone is too weak a bar. In the reported outage every
    /// Codex-transport persona failed — which is all four always-on reviewers,
    /// because only `security` runs on the Claude transport. If that one
    /// persona happened to be activated, the run would report `personas_run: 1`
    /// and pass a check that only asked "did anything run", with correctness,
    /// testing, maintainability and project-standards having never looked at
    /// the diff. A review missing a mandatory lane is not a review.
    #[serde(default)]
    pub required_personas_missing: Vec<String>,
}

/// Whether a [`ReviewOutcome`] represents a review that actually happened
/// (cas-acf83).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewExecutionStatus {
    /// Every mandatory persona produced a verdict.
    Executed {
        personas_run: u32,
        /// Optional personas that failed — the review stands, but degraded.
        personas_failed: Vec<String>,
    },
    /// Personas ran, but at least one MANDATORY lane produced no verdict, so
    /// whole classes of defect went unexamined (cas-acf83).
    Incomplete {
        personas_run: u32,
        /// The mandatory lanes that are missing.
        required_missing: Vec<String>,
        /// Every failure the producer recorded, for diagnosis.
        personas_failed: Vec<String>,
    },
    /// The producer explicitly reported that nothing ran.
    DidNotExecute {
        /// Why, if the producer said. Includes named persona failures.
        reason: String,
    },
    /// The envelope carries no execution information at all. Indistinguishable
    /// from a hand-written envelope, so it can never be treated as evidence a
    /// review occurred.
    Unreported,
}

impl ReviewExecutionStatus {
    /// Whether this outcome may be trusted as evidence a review ran.
    pub fn is_executed(&self) -> bool {
        matches!(self, Self::Executed { .. })
    }
}

impl ReviewOutcome {
    /// Whether this envelope is evidence that a review actually ran
    /// (cas-acf83 / GH #108).
    ///
    /// Reads the explicit `execution` block. An envelope without one is
    /// `Unreported` — indistinguishable from hand-written JSON, so it can
    /// never be treated as evidence a review occurred.
    pub fn execution_status(&self) -> ReviewExecutionStatus {
        let Some(execution) = self.execution.as_ref() else {
            return ReviewExecutionStatus::Unreported;
        };
        if execution.personas_run == 0 {
            let mut reason = execution
                .skipped_reason
                .clone()
                .unwrap_or_else(|| "no personas produced a verdict".to_string());
            if !execution.personas_failed.is_empty() {
                reason.push_str(&format!(
                    " (failed: {})",
                    execution.personas_failed.join(", ")
                ));
            }
            return ReviewExecutionStatus::DidNotExecute { reason };
        }
        if !execution.required_personas_missing.is_empty() {
            return ReviewExecutionStatus::Incomplete {
                personas_run: execution.personas_run,
                required_missing: execution.required_personas_missing.clone(),
                personas_failed: execution.personas_failed.clone(),
            };
        }
        ReviewExecutionStatus::Executed {
            personas_run: execution.personas_run,
            personas_failed: execution.personas_failed.clone(),
        }
    }

    /// Validate every contained finding + the mode string. Called by
    /// the close gate before it trusts the envelope — a malformed
    /// envelope from the worker is treated like a reviewer error, not
    /// a free pass.
    pub fn validate(&self) -> Result<(), FindingValidationError> {
        if self.mode.trim().is_empty() {
            return Err(FindingValidationError::EmptyReviewerName);
        }
        for f in &self.residual {
            f.validate()?;
        }
        for f in &self.pre_existing {
            f.validate()?;
        }
        Ok(())
    }
}

/// Required `Finding` JSON keys (semantic rules still enforced by
/// [`Finding::validate`] after successful deserialization).
pub const FINDING_REQUIRED_FIELDS: &[&str] = &[
    "title",
    "severity",
    "file",
    "line",
    "why_it_matters",
    "autofix_class",
    "owner",
    "confidence",
    "evidence",
    "pre_existing",
];

/// Optional `Finding` JSON keys.
pub const FINDING_OPTIONAL_FIELDS: &[&str] = &["suggested_fix", "requires_verification"];

/// Compact shape + Finding field list for close-gate / tool error text.
///
/// Used when `code_review_findings` fails to parse so callers see the full
/// `Finding` contract in one response instead of discovering it field-by-field
/// across serial close attempts (cas-297e).
pub fn review_outcome_shape_hint() -> String {
    format!(
        "Expected shape: {{residual: Finding[], pre_existing: Finding[], mode: string, \
         execution: {{personas_run: int, personas_failed: string[], \
         required_personas_missing: string[], skipped_reason: string|null}}}}.\n\
         `execution` is required (cas-acf83): without it an empty residual[] \
         cannot be told apart from a review that never ran. Copy it from the \
         cas-code-review result; do not hand-write it.\n\
         Each Finding requires: {}.\n\
         Optional Finding fields: {}.",
        FINDING_REQUIRED_FIELDS.join(", "),
        FINDING_OPTIONAL_FIELDS.join(", "),
    )
}

/// Multi-error parse failure for a `ReviewOutcome` JSON envelope.
///
/// Unlike bare `serde_json::from_str`, this collects **all** missing
/// required Finding fields (and other structural issues) into a single
/// message so callers can fix the envelope in one round trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewOutcomeParseError {
    /// Human-readable multi-line diagnostic.
    pub message: String,
}

impl fmt::Display for ReviewOutcomeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ReviewOutcomeParseError {}

/// Parse + structurally multi-validate + semantically validate a
/// [`ReviewOutcome`] JSON string.
///
/// Structural missing-field checks run over every Finding in `residual[]`
/// and `pre_existing[]` **before** serde type conversion, so a single bad
/// envelope reports every missing key at once.
pub fn parse_review_outcome(json: &str) -> Result<ReviewOutcome, ReviewOutcomeParseError> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| {
        ReviewOutcomeParseError {
            message: format!("invalid JSON: {e}"),
        }
    })?;

    let obj = value.as_object().ok_or_else(|| ReviewOutcomeParseError {
        message: "ReviewOutcome must be a JSON object".to_string(),
    })?;

    let mut errors: Vec<String> = Vec::new();

    match obj.get("mode") {
        None => errors.push("missing field `mode`".to_string()),
        Some(v) if !v.is_string() => {
            errors.push("`mode` must be a string".to_string());
        }
        Some(v) if v.as_str().map(|s| s.trim().is_empty()).unwrap_or(true) => {
            errors.push("`mode` must be a non-empty string".to_string());
        }
        Some(_) => {}
    }

    for key in ["residual", "pre_existing"] {
        match obj.get(key) {
            None | Some(serde_json::Value::Null) => {}
            Some(v) => {
                if let Some(arr) = v.as_array() {
                    for (i, item) in arr.iter().enumerate() {
                        errors.extend(collect_finding_field_errors(
                            item,
                            &format!("{key}[{i}]"),
                        ));
                    }
                } else {
                    errors.push(format!("`{key}` must be an array of Finding objects"));
                }
            }
        }
    }

    for k in obj.keys() {
        if !matches!(
            k.as_str(),
            "residual" | "pre_existing" | "mode" | "execution"
        ) {
            errors.push(format!("unknown field `{k}`"));
        }
    }

    if !errors.is_empty() {
        return Err(ReviewOutcomeParseError {
            message: errors.join("\n"),
        });
    }

    let outcome: ReviewOutcome = serde_json::from_value(value).map_err(|e| {
        ReviewOutcomeParseError {
            message: format!("type/shape error: {e}"),
        }
    })?;

    outcome.validate().map_err(|e| ReviewOutcomeParseError {
        message: e.to_string(),
    })?;

    Ok(outcome)
}

fn collect_finding_field_errors(value: &serde_json::Value, path: &str) -> Vec<String> {
    let mut errs = Vec::new();
    let Some(obj) = value.as_object() else {
        errs.push(format!("{path}: expected a Finding object"));
        return errs;
    };

    for field in FINDING_REQUIRED_FIELDS {
        if !obj.contains_key(*field) {
            errs.push(format!("{path}: missing field `{field}`"));
        }
    }

    let known: std::collections::HashSet<&str> = FINDING_REQUIRED_FIELDS
        .iter()
        .chain(FINDING_OPTIONAL_FIELDS.iter())
        .copied()
        .collect();
    for k in obj.keys() {
        if !known.contains(k.as_str()) {
            errs.push(format!("{path}: unknown field `{k}`"));
        }
    }
    errs
}

#[cfg(test)]
mod tests {
    // --- cas-acf83 (GH #108): execution provenance ---------------------------

    fn execution(personas_run: u32, failed: &[&str], required_missing: &[&str]) -> ReviewExecution {
        ReviewExecution {
            personas_run,
            personas_failed: failed.iter().map(|s| s.to_string()).collect(),
            skipped_reason: None,
            required_personas_missing: required_missing.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn outcome_with(execution: Option<ReviewExecution>) -> ReviewOutcome {
        ReviewOutcome {
            residual: Vec::new(),
            pre_existing: Vec::new(),
            mode: "headless".to_string(),
            execution,
        }
    }

    /// The reported incident: every persona failed to launch, so the empty
    /// residual[] is an absent verdict rather than a passing one.
    #[test]
    fn zero_personas_is_not_an_executed_review() {
        let mut exec = execution(0, &["correctness: 402 insufficient credits"], &[]);
        exec.skipped_reason = Some("all personas failed to launch".to_string());
        match outcome_with(Some(exec)).execution_status() {
            ReviewExecutionStatus::DidNotExecute { reason } => {
                assert!(
                    reason.contains("all personas failed to launch"),
                    "must carry the producer's reason: {reason}"
                );
                assert!(
                    reason.contains("402 insufficient credits"),
                    "must name the persona failure so it is diagnosable: {reason}"
                );
            }
            other => panic!("expected DidNotExecute, got {other:?}"),
        }
    }

    /// With no reason recorded, the status still reports that nothing ran
    /// rather than falling back to "executed".
    #[test]
    fn zero_personas_without_a_reason_still_reports_did_not_execute() {
        match outcome_with(Some(execution(0, &[], &[]))).execution_status() {
            ReviewExecutionStatus::DidNotExecute { reason } => {
                assert!(!reason.trim().is_empty(), "must say something");
            }
            other => panic!("expected DidNotExecute, got {other:?}"),
        }
    }

    /// The narrower failure the all-or-nothing check misses: every always-on
    /// persona shares the Codex transport, so one outage silences all four
    /// while a single Claude-hosted persona still reports a run.
    #[test]
    fn a_surviving_persona_does_not_excuse_missing_mandatory_lanes() {
        let status = outcome_with(Some(execution(
            1,
            &["correctness: transport down", "testing: transport down"],
            &["correctness", "testing", "maintainability", "project-standards"],
        )))
        .execution_status();
        match &status {
            ReviewExecutionStatus::Incomplete {
                personas_run,
                required_missing,
                personas_failed,
            } => {
                assert_eq!(*personas_run, 1);
                assert_eq!(required_missing.len(), 4);
                assert!(required_missing.iter().any(|p| p == "correctness"));
                assert!(!personas_failed.is_empty(), "failures must survive for diagnosis");
            }
            other => panic!("a partial review must not read as Executed, got {other:?}"),
        }
        assert!(
            !status.is_executed(),
            "Incomplete must never be treated as a completed review"
        );
    }

    /// An envelope with no execution block proves nothing — it is exactly what
    /// hand-written JSON looks like.
    #[test]
    fn a_missing_execution_block_is_unreported_not_executed() {
        let status = outcome_with(None).execution_status();
        assert_eq!(status, ReviewExecutionStatus::Unreported);
        assert!(!status.is_executed());
    }

    /// A genuine full run is Executed, and an optional persona failing does not
    /// downgrade it.
    #[test]
    fn a_full_run_is_executed_even_with_an_optional_persona_failure() {
        let status = outcome_with(Some(execution(7, &["performance: timed out"], &[])))
            .execution_status();
        match &status {
            ReviewExecutionStatus::Executed {
                personas_run,
                personas_failed,
            } => {
                assert_eq!(*personas_run, 7);
                assert_eq!(personas_failed, &vec!["performance: timed out".to_string()]);
            }
            other => panic!("expected Executed, got {other:?}"),
        }
        assert!(status.is_executed());
    }

    /// The execution block must survive a JSON round-trip through the strict
    /// parser the close gate uses — an unknown-field rejection here would make
    /// every compliant envelope unparseable.
    #[test]
    fn the_strict_parser_accepts_and_preserves_the_execution_block() {
        let json = r#"{"residual":[],"pre_existing":[],"mode":"autofix",
            "execution":{"personas_run":4,"personas_failed":[],
            "required_personas_missing":[],"skipped_reason":null}}"#;
        let parsed = parse_review_outcome(json).expect("must parse");
        assert!(parsed.execution_status().is_executed());
    }

    use super::*;

    fn valid_finding() -> Finding {
        Finding {
            title: "Unwrap on parsed int can panic".to_string(),
            severity: Severity::P1,
            file: "src/foo.rs".to_string(),
            line: 42,
            why_it_matters: "Panics on malformed user input, crashing the worker.".to_string(),
            autofix_class: AutofixClass::SafeAuto,
            owner: Owner::ReviewFixer,
            confidence: 0.85,
            evidence: vec!["let n: u32 = s.parse().unwrap();".to_string()],
            pre_existing: false,
            suggested_fix: Some("Use `?` or `.map_err(...)`".to_string()),
            requires_verification: false,
        }
    }

    #[test]
    fn finding_roundtrip() {
        let f = valid_finding();
        let json = serde_json::to_string(&f).unwrap();
        let back: Finding = serde_json::from_str(&json).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn finding_validates_happy_path() {
        valid_finding().validate().unwrap();
    }

    #[test]
    fn title_too_long_rejected() {
        let mut f = valid_finding();
        f.title = "x".repeat(MAX_TITLE_LEN + 1);
        assert_eq!(
            f.validate().unwrap_err(),
            FindingValidationError::TitleTooLong {
                len: MAX_TITLE_LEN + 1,
                max: MAX_TITLE_LEN,
            }
        );
    }

    #[test]
    fn title_exactly_max_ok() {
        let mut f = valid_finding();
        f.title = "x".repeat(MAX_TITLE_LEN);
        f.validate().unwrap();
    }

    #[test]
    fn empty_title_rejected() {
        let mut f = valid_finding();
        f.title = "   ".to_string();
        assert_eq!(
            f.validate().unwrap_err(),
            FindingValidationError::EmptyTitle
        );
    }

    #[test]
    fn confidence_below_zero_rejected() {
        let mut f = valid_finding();
        f.confidence = -0.01;
        assert!(matches!(
            f.validate().unwrap_err(),
            FindingValidationError::ConfidenceOutOfRange { .. }
        ));
    }

    #[test]
    fn confidence_above_one_rejected() {
        let mut f = valid_finding();
        f.confidence = 1.01;
        assert!(matches!(
            f.validate().unwrap_err(),
            FindingValidationError::ConfidenceOutOfRange { .. }
        ));
    }

    #[test]
    fn confidence_nan_rejected() {
        let mut f = valid_finding();
        f.confidence = f32::NAN;
        assert!(matches!(
            f.validate().unwrap_err(),
            FindingValidationError::ConfidenceOutOfRange { .. }
        ));
    }

    #[test]
    fn confidence_bounds_inclusive() {
        let mut f = valid_finding();
        f.confidence = 0.0;
        f.validate().unwrap();
        f.confidence = 1.0;
        f.validate().unwrap();
    }

    #[test]
    fn empty_evidence_rejected() {
        let mut f = valid_finding();
        f.evidence.clear();
        assert_eq!(
            f.validate().unwrap_err(),
            FindingValidationError::EmptyEvidence
        );
    }

    #[test]
    fn empty_evidence_entry_rejected() {
        let mut f = valid_finding();
        f.evidence = vec!["ok".to_string(), "   ".to_string()];
        assert_eq!(
            f.validate().unwrap_err(),
            FindingValidationError::EmptyEvidenceEntry { index: 1 }
        );
    }

    #[test]
    fn absolute_file_path_rejected_unix() {
        let mut f = valid_finding();
        f.file = "/etc/passwd".to_string();
        assert!(matches!(
            f.validate().unwrap_err(),
            FindingValidationError::AbsoluteFilePath { .. }
        ));
    }

    #[test]
    fn absolute_file_path_rejected_windows() {
        let mut f = valid_finding();
        f.file = "C:\\Users\\foo.rs".to_string();
        assert!(matches!(
            f.validate().unwrap_err(),
            FindingValidationError::AbsoluteFilePath { .. }
        ));
    }

    #[test]
    fn relative_dotted_path_ok() {
        let mut f = valid_finding();
        f.file = "./src/foo.rs".to_string();
        f.validate().unwrap();
    }

    #[test]
    fn empty_file_rejected() {
        let mut f = valid_finding();
        f.file = "".to_string();
        assert_eq!(
            f.validate().unwrap_err(),
            FindingValidationError::EmptyFilePath
        );
    }

    #[test]
    fn empty_why_rejected() {
        let mut f = valid_finding();
        f.why_it_matters = "".to_string();
        assert_eq!(
            f.validate().unwrap_err(),
            FindingValidationError::EmptyWhyItMatters
        );
    }

    #[test]
    fn invalid_severity_enum_rejected() {
        let json = r#"{
            "title":"t","severity":"P5","file":"a.rs","line":1,
            "why_it_matters":"w","autofix_class":"safe_auto","owner":"human",
            "confidence":0.5,"evidence":["e"],"pre_existing":false
        }"#;
        assert!(serde_json::from_str::<Finding>(json).is_err());
    }

    #[test]
    fn invalid_autofix_class_rejected() {
        let json = r#"{
            "title":"t","severity":"P1","file":"a.rs","line":1,
            "why_it_matters":"w","autofix_class":"yolo","owner":"human",
            "confidence":0.5,"evidence":["e"],"pre_existing":false
        }"#;
        assert!(serde_json::from_str::<Finding>(json).is_err());
    }

    #[test]
    fn unknown_field_rejected() {
        let json = r#"{
            "title":"t","severity":"P1","file":"a.rs","line":1,
            "why_it_matters":"w","autofix_class":"safe_auto","owner":"human",
            "confidence":0.5,"evidence":["e"],"pre_existing":false,
            "bogus_field":"x"
        }"#;
        assert!(serde_json::from_str::<Finding>(json).is_err());
    }

    #[test]
    fn severity_display() {
        assert_eq!(Severity::P0.to_string(), "P0");
        assert_eq!(Severity::P3.to_string(), "P3");
    }

    #[test]
    fn autofix_class_display() {
        assert_eq!(AutofixClass::SafeAuto.to_string(), "safe_auto");
        assert_eq!(AutofixClass::GatedAuto.to_string(), "gated_auto");
        assert_eq!(AutofixClass::Advisory.to_string(), "advisory");
    }

    #[test]
    fn owner_display() {
        assert_eq!(Owner::ReviewFixer.to_string(), "review-fixer");
        assert_eq!(Owner::DownstreamResolver.to_string(), "downstream-resolver");
        assert_eq!(Owner::Human.to_string(), "human");
    }

    #[test]
    fn reviewer_output_roundtrip() {
        let out = ReviewerOutput {
            reviewer: "correctness".to_string(),
            findings: vec![valid_finding()],
            residual_risks: vec!["retry path not verified".to_string()],
            testing_gaps: vec!["no integration test".to_string()],
            skipped_reason: None,
        };
        let json = serde_json::to_string(&out).unwrap();
        let back: ReviewerOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(out, back);
        back.validate().unwrap();
    }

    #[test]
    fn reviewer_output_empty_reviewer_name_rejected() {
        let out = ReviewerOutput {
            reviewer: " ".to_string(),
            findings: vec![],
            residual_risks: vec![],
            testing_gaps: vec![],
            skipped_reason: None,
        };
        assert_eq!(
            out.validate().unwrap_err(),
            FindingValidationError::EmptyReviewerName
        );
    }

    #[test]
    fn reviewer_output_propagates_finding_error() {
        let mut bad = valid_finding();
        bad.confidence = 2.0;
        let out = ReviewerOutput {
            reviewer: "correctness".to_string(),
            findings: vec![bad],
            residual_risks: vec![],
            testing_gaps: vec![],
            skipped_reason: None,
        };
        assert!(matches!(
            out.validate().unwrap_err(),
            FindingValidationError::ConfidenceOutOfRange { .. }
        ));
    }

    #[test]
    fn reviewer_output_defaults_for_optional_arrays() {
        let json = r#"{"reviewer":"security","findings":[]}"#;
        let out: ReviewerOutput = serde_json::from_str(json).unwrap();
        assert!(out.residual_risks.is_empty());
        assert!(out.testing_gaps.is_empty());
        assert_eq!(out.skipped_reason, None);
        out.validate().unwrap();
    }

    #[test]
    fn reviewer_output_accepts_skipped_reason() {
        let json = r#"{
            "reviewer":"gpt-5.5:independent",
            "findings":[],
            "residual_risks":[],
            "testing_gaps":[],
            "skipped_reason":"codex CLI not installed"
        }"#;
        let out = parse_reviewer_output(json).unwrap();
        assert_eq!(out.reviewer, "gpt-5.5:independent");
        assert!(out.findings.is_empty());
        assert_eq!(
            out.skipped_reason.as_deref(),
            Some("codex CLI not installed")
        );
    }

    #[test]
    fn parse_reviewer_output_helper_happy_path() {
        let out = ReviewerOutput {
            reviewer: "testing".to_string(),
            findings: vec![valid_finding()],
            residual_risks: vec![],
            testing_gaps: vec![],
            skipped_reason: None,
        };
        let json = serde_json::to_string(&out).unwrap();
        let parsed = parse_reviewer_output(&json).unwrap();
        assert_eq!(parsed, out);
    }

    #[test]
    fn parse_reviewer_output_helper_rejects_bad_path() {
        let json = r#"{
            "reviewer":"testing",
            "findings":[{
                "title":"t","severity":"P1","file":"/abs/path.rs","line":1,
                "why_it_matters":"w","autofix_class":"safe_auto","owner":"human",
                "confidence":0.5,"evidence":["e"],"pre_existing":false
            }]
        }"#;
        assert!(parse_reviewer_output(json).is_err());
    }

    #[test]
    fn finding_display_format() {
        let f = valid_finding();
        let s = f.to_string();
        assert!(s.starts_with("[P1] Unwrap on parsed int can panic — src/foo.rs:42"));
        assert!(s.contains("safe_auto"));
        assert!(s.contains("review-fixer"));
        assert!(s.contains("conf=0.85"));
    }

    #[test]
    fn reviewer_output_display_format() {
        let out = ReviewerOutput {
            reviewer: "correctness".to_string(),
            findings: vec![valid_finding()],
            residual_risks: vec!["r1".to_string()],
            testing_gaps: vec!["g1".to_string()],
            skipped_reason: None,
        };
        let s = out.to_string();
        assert!(s.contains("== reviewer: correctness (1 findings) =="));
        assert!(s.contains("residual_risks:"));
        assert!(s.contains("- r1"));
        assert!(s.contains("testing_gaps:"));
        assert!(s.contains("- g1"));
    }

    #[test]
    fn reviewer_output_display_includes_skipped_reason() {
        let out = ReviewerOutput {
            reviewer: "gpt-5.5:independent".to_string(),
            findings: vec![],
            residual_risks: vec![],
            testing_gaps: vec![],
            skipped_reason: Some("codex CLI not installed".to_string()),
        };
        let s = out.to_string();
        assert!(s.contains("== reviewer: gpt-5.5:independent (0 findings) =="));
        assert!(s.contains("skipped_reason: codex CLI not installed"));
    }

    // --- parse_review_outcome multi-field diagnostics (cas-297e) -----------

    #[test]
    fn parse_review_outcome_happy_path() {
        let env = ReviewOutcome {
            residual: vec![valid_finding()],
            pre_existing: vec![],
            mode: "autofix".to_string(),
            execution: None,
        };
        let json = serde_json::to_string(&env).unwrap();
        let parsed = parse_review_outcome(&json).unwrap();
        assert_eq!(parsed, env);
    }

    #[test]
    fn parse_review_outcome_reports_all_missing_finding_fields_at_once() {
        // Minimal Finding-shaped object missing several required keys
        // (the piecemeal failure mode reported in the Ozer bug).
        let json = r#"{
            "mode": "interactive",
            "residual": [{
                "title": "partial finding",
                "severity": "P2",
                "file": "src/a.rs",
                "line": 1
            }],
            "pre_existing": []
        }"#;
        let err = parse_review_outcome(json).unwrap_err();
        let msg = err.message;
        // All missing required fields must appear in ONE response.
        for field in [
            "why_it_matters",
            "autofix_class",
            "owner",
            "confidence",
            "evidence",
            "pre_existing",
        ] {
            assert!(
                msg.contains(&format!("missing field `{field}`")),
                "expected missing field `{field}` in multi-error message, got:\n{msg}"
            );
        }
        // Present fields must not be reported as missing.
        assert!(!msg.contains("missing field `title`"), "{msg}");
        assert!(!msg.contains("missing field `severity`"), "{msg}");
        assert!(!msg.contains("missing field `file`"), "{msg}");
        assert!(!msg.contains("missing field `line`"), "{msg}");
        // Path prefix points at the residual entry.
        assert!(msg.contains("residual[0]"), "{msg}");
    }

    #[test]
    fn parse_review_outcome_aggregates_errors_across_findings() {
        let json = r#"{
            "mode": "autofix",
            "residual": [
                { "title": "a", "severity": "P1", "file": "a.rs", "line": 1 },
                { "title": "b", "severity": "P2", "file": "b.rs", "line": 2 }
            ],
            "pre_existing": [
                { "title": "c", "severity": "P3", "file": "c.rs", "line": 3 }
            ]
        }"#;
        let err = parse_review_outcome(json).unwrap_err();
        let msg = err.message;
        assert!(msg.contains("residual[0]"), "{msg}");
        assert!(msg.contains("residual[1]"), "{msg}");
        assert!(msg.contains("pre_existing[0]"), "{msg}");
        // Each finding should report why_it_matters among the missing set.
        assert_eq!(
            msg.matches("missing field `why_it_matters`").count(),
            3,
            "expected 3 findings missing why_it_matters, got:\n{msg}"
        );
    }

    #[test]
    fn review_outcome_shape_hint_documents_finding_fields() {
        let hint = review_outcome_shape_hint();
        assert!(hint.contains("residual: Finding[]"));
        assert!(hint.contains("pre_existing: Finding[]"));
        assert!(hint.contains("mode: string"));
        for field in FINDING_REQUIRED_FIELDS {
            assert!(
                hint.contains(field),
                "shape hint must document required field `{field}`: {hint}"
            );
        }
        assert!(hint.contains("suggested_fix"));
        assert!(hint.contains("requires_verification"));
    }

    #[test]
    fn parse_review_outcome_rejects_empty_mode() {
        let json = r#"{"residual":[],"pre_existing":[],"mode":"   "}"#;
        let err = parse_review_outcome(json).unwrap_err();
        assert!(
            err.message.contains("mode"),
            "expected mode error, got: {}",
            err.message
        );
    }
}
