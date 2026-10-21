//! Read-back: a journey's durable effects checked after it runs, shared by
//! post-release and the release gate.
//!
//! A journey declares which subject kinds and invariant IDs it reads back.
//! Cassy forwards only those subject names, their opaque ids and the declared
//! invariant IDs, never a query. Results take the post-release [`Readback`]
//! shape; observed values are never kept.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// One read-back result. Post-release probes report it directly; the gate
/// builds it from the adapter's `readback` verb.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Readback {
    pub journey: String,
    pub subject: String,
    pub invariants: Vec<String>,
    pub ok: bool,
}

impl Readback {
    /// A passing read-back that names its journey, subject and invariants.
    pub fn proves(&self) -> bool {
        self.ok && !self.journey.is_empty() && !self.subject.is_empty() && !self.invariants.is_empty()
    }
}

/// A journey's `readback:` declaration in the registry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Declaration {
    /// Subject kinds, for example `account_uid` or `workspace_id`.
    pub subjects: Vec<String>,
    /// Invariant IDs, for example `INV-WORKSPACE`.
    pub invariants: Vec<String>,
}

/// What the gate does with one read-back.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Every declared invariant read back ok.
    Pass(Readback),
    /// At least one invariant read back false: a product defect.
    Fail { readback: Readback, failed: Vec<String> },
    /// A provider failed, throttled or ran out (`external`, `rate_limited`,
    /// `insufficient`).
    BlockedExternal { provider: Option<String>, error: String },
    /// No complete answer, so the row cannot pass.
    Missing(String),
}

const MAX_ID: usize = 128;

/// Opaque ids are short tokens. Whitespace, quotes and SQL punctuation are
/// refused, so a query cannot pass for an id.
fn opaque_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':' | b'@'))
}

fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// The allow-listed `readback` verb arguments, or why the request is refused
/// before any adapter call.
pub fn request_args(
    journey: &str,
    declaration: Option<&Declaration>,
    subjects: &BTreeMap<String, String>,
) -> Result<Value, String> {
    let declaration = declaration
        .filter(|d| !d.subjects.is_empty() && !d.invariants.is_empty())
        .ok_or_else(|| format!("journey {journey} declares no readback"))?;
    if let Some(bad) = declaration.invariants.iter().find(|id| !name(id)) {
        return Err(format!(
            "journey {journey} readback invariant {:?} is not an invariant ID",
            bad.chars().take(40).collect::<String>()
        ));
    }
    if subjects.is_empty() {
        return Err(format!("journey {journey} readback has no subject ids"));
    }
    for (kind, id) in subjects {
        if !declaration.subjects.contains(kind) {
            return Err(format!(
                "subject {:?} is not in journey {journey}'s readback allow-list",
                kind.chars().take(40).collect::<String>()
            ));
        }
        if !opaque_id(id) {
            return Err(format!("subject {kind} is not an opaque id"));
        }
    }
    Ok(serde_json::json!({
        "subjects": subjects,
        "invariants": declaration.invariants,
    }))
}

/// Maps the adapter's `{invariant_id: {ok, observed}}` answer, or its error,
/// to an [`Outcome`]. Any false invariant fails; an unanswered one cannot pass.
pub fn classify(
    journey: &str,
    declaration: &Declaration,
    subjects: &BTreeMap<String, String>,
    answer: Result<Value, crate::qa_release::CallError>,
) -> Outcome {
    use crate::qa_release::{CallError, ErrorKind};
    let data = match answer {
        Ok(data) => data,
        Err(CallError::Adapter(_, error))
            if matches!(
                error.kind,
                ErrorKind::External | ErrorKind::RateLimited | ErrorKind::Insufficient
            ) =>
        {
            return Outcome::BlockedExternal {
                provider: error.provider,
                error: error.message,
            };
        }
        Err(error) => return Outcome::Missing(error.to_string()),
    };
    let readback = |ok| Readback {
        journey: journey.into(),
        subject: subjects
            .iter()
            .map(|(kind, id)| format!("{kind}={id}"))
            .collect::<Vec<_>>()
            .join(" "),
        invariants: declaration.invariants.clone(),
        ok,
    };
    let answered = |id: &str| data.get(id).and_then(|r| r.get("ok")).and_then(Value::as_bool);
    let failed: Vec<String> = declaration
        .invariants
        .iter()
        .filter(|id| answered(id) == Some(false))
        .cloned()
        .collect();
    if !failed.is_empty() {
        return Outcome::Fail {
            readback: readback(false),
            failed,
        };
    }
    let missing: Vec<&str> = declaration
        .invariants
        .iter()
        .filter(|id| answered(id).is_none())
        .map(String::as_str)
        .collect();
    if missing.is_empty() {
        Outcome::Pass(readback(true))
    } else {
        Outcome::Missing(format!("not read back: {}", missing.join(", ")))
    }
}
