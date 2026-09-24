//! Independent QA and polish pass records (cas-619f).
//!
//! A user-facing factory delivery that parks for merge gets a QA pass: a
//! separate agent — never the implementer — builds the delivered branch,
//! walks its journeys and scores its polish, then records a verdict bound to
//! the exact branch tip it reviewed. The verdict deliberately lives here and
//! not in the shared `verifications` table: the task-verifier close gate
//! reads that table untyped, and older binaries parse unknown verification
//! types as `task`, so a QA approval stored there could satisfy the wrong
//! gate.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::TypeError;

/// Lifecycle of one QA round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QaPassState {
    /// Dispatched; no reviewer has started it yet.
    Pending,
    /// A reviewer (not the implementer) has started the QA task.
    Claimed,
    /// The reviewer approved the delivery at `bound_head`.
    Passed,
    /// The reviewer rejected the delivery; the task went back to its
    /// implementer through request_changes.
    Failed,
    /// The deadline passed before a verdict was recorded.
    TimedOut,
    /// The delivered branch moved while the round was open.
    Superseded,
    /// A supervisor waived the pass with a logged reason.
    Waived,
}

impl QaPassState {
    /// Pending or claimed rounds are still waiting for a verdict.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Pending | Self::Claimed)
    }

    /// States that satisfy the merge and close gates for their `bound_head`.
    pub fn satisfies_gate(self) -> bool {
        matches!(self, Self::Passed | Self::Waived)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Claimed => "claimed",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Superseded => "superseded",
            Self::Waived => "waived",
        }
    }
}

impl fmt::Display for QaPassState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for QaPassState {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "claimed" => Ok(Self::Claimed),
            "passed" => Ok(Self::Passed),
            "failed" => Ok(Self::Failed),
            "timed_out" => Ok(Self::TimedOut),
            "superseded" => Ok(Self::Superseded),
            "waived" => Ok(Self::Waived),
            other => Err(TypeError::Parse(format!("invalid QA pass state: {other}"))),
        }
    }
}

/// The reviewer's verdict on one round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QaVerdict {
    Approved,
    Rejected,
}

impl FromStr for QaVerdict {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "approved" | "approve" | "pass" | "passed" => Ok(Self::Approved),
            "rejected" | "reject" | "fail" | "failed" => Ok(Self::Rejected),
            other => Err(TypeError::Parse(format!(
                "invalid QA verdict: {other} (expected approved or rejected)"
            ))),
        }
    }
}

/// One QA round for one delivery task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QaPass {
    pub id: String,
    /// The delivery task under review.
    pub task_id: String,
    /// 1-based; counts rejected rounds plus one.
    pub round: u32,
    /// The assignee who delivered the branch. Never allowed to review it.
    pub implementer_agent_id: String,
    /// Factory branch that carries the delivery.
    pub branch: String,
    /// Exact branch tip this round reviews.
    pub bound_head: String,
    /// The QA work item the reviewer starts.
    pub qa_task_id: Option<String>,
    pub reviewer_agent_id: Option<String>,
    pub state: QaPassState,
    /// Verdict summary, or the supervisor's waiver reason.
    pub summary: Option<String>,
    /// JSON array of findings (same shape as verification issues).
    pub issues_json: Option<String>,
    /// Absolute path of the round's LEDGER.md.
    pub ledger_path: Option<String>,
    /// Supervisor that waived the pass, when `state == Waived`.
    pub issuer_agent_id: Option<String>,
    pub requested_at: DateTime<Utc>,
    pub deadline_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// Summary prefix of a round withdrawn because its delivery stopped needing
/// independent QA (cas-5c38): the `demo_statement` that made it user-facing
/// was cleared, or the task is a no-code task with nothing to review. A
/// withdrawn round stays on record for audit but never binds the gates.
pub const QA_PASS_WITHDRAWN_PREFIX: &str = "withdrawn: ";

impl QaPass {
    /// Whether this round was withdrawn rather than superseded by a new tip.
    pub fn is_withdrawn(&self) -> bool {
        self.state == QaPassState::Superseded
            && self
                .summary
                .as_deref()
                .is_some_and(|summary| summary.starts_with(QA_PASS_WITHDRAWN_PREFIX))
    }

    /// Short head used in titles and messages.
    pub fn head8(&self) -> &str {
        let end = self.bound_head.len().min(8);
        &self.bound_head[..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_and_classifies() {
        for state in [
            QaPassState::Pending,
            QaPassState::Claimed,
            QaPassState::Passed,
            QaPassState::Failed,
            QaPassState::TimedOut,
            QaPassState::Superseded,
            QaPassState::Waived,
        ] {
            assert_eq!(state.as_str().parse::<QaPassState>().unwrap(), state);
        }
        assert!(QaPassState::Pending.is_active());
        assert!(QaPassState::Claimed.is_active());
        assert!(!QaPassState::Passed.is_active());
        assert!(QaPassState::Passed.satisfies_gate());
        assert!(QaPassState::Waived.satisfies_gate());
        assert!(!QaPassState::Failed.satisfies_gate());
        assert!(!QaPassState::TimedOut.satisfies_gate());
        assert!("bogus".parse::<QaPassState>().is_err());
    }

    #[test]
    fn verdict_parses_only_the_two_outcomes() {
        assert_eq!("approved".parse::<QaVerdict>().unwrap(), QaVerdict::Approved);
        assert_eq!(" Rejected ".parse::<QaVerdict>().unwrap(), QaVerdict::Rejected);
        assert!("skipped".parse::<QaVerdict>().is_err());
        assert!("".parse::<QaVerdict>().is_err());
    }
}
