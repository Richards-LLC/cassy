//! Row and gate verdicts (design art-e3dfbeb7, "Row and verdict vocabulary").
//! The CLI, MCP, pack, PR comment and status use only these words.
//!
//! A row is one journey, or one golden-set cell × run. Its payload types live
//! with the check that produces them: [`BlockedExternal`] (provider failures,
//! 429 and 402, including resource-budget refusals) and [`ConfigMismatch`].
use super::blocked::BlockedExternal;
use super::flags::ConfigMismatch;
use super::printable;
use crate::qa_readback::Outcome;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Why a row was not walked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotExercisedCause {
    FixtureSetup,
    Automation,
    TimedOut,
    OperatorSkip,
}

impl NotExercisedCause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FixtureSetup => "fixture_setup",
            Self::Automation => "automation",
            Self::TimedOut => "timed_out",
            Self::OperatorSkip => "operator_skip",
        }
    }
}

/// One row's verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RowVerdict {
    Pass,
    /// A product defect.
    Fail { reason: String },
    NotARoute,
    NotExercised { cause: NotExercisedCause, detail: String },
    BlockedExternal(BlockedExternal),
    /// The evidence ran under flag values the target does not serve.
    ConfigMismatch { mismatches: Vec<ConfigMismatch> },
    /// The served components changed during the row, so the row is void.
    ServedDrift { components: Vec<String> },
}

impl RowVerdict {
    /// The vocabulary word.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail { .. } => "FAIL",
            Self::NotARoute => "NOT_A_ROUTE",
            Self::NotExercised { .. } => "NOT_EXERCISED",
            Self::BlockedExternal(_) => super::blocked::VERDICT,
            Self::ConfigMismatch { .. } => "CONFIG_MISMATCH",
            Self::ServedDrift { .. } => "SERVED_DRIFT",
        }
    }

    /// Grades a walked row. PASS needs a runner PASS (`Ok`), at least one
    /// frame and, when the journey declares a read-back (`Some`), every
    /// declared invariant read back ok. `None` means none is declared.
    pub fn grade(runner: Result<(), String>, frames: usize, readback: Option<Outcome>) -> Self {
        let not_exercised = |detail: String| Self::NotExercised {
            cause: NotExercisedCause::Automation,
            detail,
        };
        if let Err(reason) = runner {
            return Self::Fail { reason: printable(&reason) };
        }
        if frames == 0 {
            return not_exercised("no frame captured".into());
        }
        match readback {
            None => Self::Pass,
            Some(Outcome::Pass(readback)) if readback.proves() => Self::Pass,
            Some(Outcome::Pass(_)) => not_exercised("read-back names no subject or invariant".into()),
            Some(Outcome::Fail { failed, .. }) => Self::Fail {
                reason: format!("read-back failed: {}", failed.join(", ")),
            },
            Some(Outcome::BlockedExternal(blocked)) => Self::BlockedExternal(blocked),
            Some(Outcome::Missing(why)) => not_exercised(format!("read-back missing: {why}")),
        }
    }
}

impl fmt::Display for RowVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pass | Self::NotARoute => f.write_str(self.word()),
            Self::Fail { reason } => write!(f, "FAIL: {reason}"),
            Self::NotExercised { cause, detail } => {
                write!(f, "NOT_EXERCISED{{{}}}: {detail}", cause.as_str())
            }
            Self::BlockedExternal(blocked) => blocked.fmt(f),
            Self::ConfigMismatch { mismatches } => {
                let lines: Vec<String> = mismatches.iter().map(ToString::to_string).collect();
                f.write_str(&lines.join("; "))
            }
            Self::ServedDrift { components } => {
                write!(f, "SERVED_DRIFT: {} changed during the row", components.join(", "))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GateVerdict {
    /// Every row is PASS or NOT_A_ROUTE and nothing else is owed.
    Clean,
    /// At least one FAIL.
    Failed,
    /// Anything else, naming each owed row or PR and its cause.
    Owed,
}

/// One thing the gate still owes: a row id or `PR #N`, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owed {
    pub subject: String,
    pub cause: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gate {
    pub verdict: GateVerdict,
    pub failed: Vec<String>,
    pub owed: Vec<Owed>,
}

/// Derives the gate verdict from `(row id, verdict)` pairs plus anything else
/// owed (an unexercised PR). No rows is OWED, never vacuously CLEAN.
pub fn gate<'a>(rows: impl IntoIterator<Item = (&'a str, &'a RowVerdict)>, mut owed: Vec<Owed>) -> Gate {
    let mut failed = Vec::new();
    let mut any = false;
    for (id, verdict) in rows {
        any = true;
        match verdict {
            RowVerdict::Pass | RowVerdict::NotARoute => {}
            RowVerdict::Fail { .. } => failed.push(id.to_owned()),
            other => owed.push(Owed { subject: id.to_owned(), cause: other.to_string() }),
        }
    }
    if !any {
        owed.push(Owed { subject: "gate".into(), cause: "no rows were recorded".into() });
    }
    let verdict = if !failed.is_empty() {
        GateVerdict::Failed
    } else if owed.is_empty() {
        GateVerdict::Clean
    } else {
        GateVerdict::Owed
    };
    Gate { verdict, failed, owed }
}
