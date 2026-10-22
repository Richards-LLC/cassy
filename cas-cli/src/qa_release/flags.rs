//! Config drift (cas-93d6): journeys declare the flags they depend on
//! (`depends_on_flags`), the gate snapshots their values at preflight through
//! the adapter's `flags` verb, and promotion compares that snapshot once with
//! the target environment. Evidence gathered under a different value is
//! `CONFIG_MISMATCH{flag, test, target}`.
use super::{CallError, ReleaseConfig, Request, Verb, call, printable};
use crate::qa_registry::Journey;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The LEDGER.md header line prefix that stores a [`FlagSnapshot`].
pub const LEDGER_PREFIX: &str = "Flags: ";

/// Flag values one environment served for a candidate, as `flags` reported them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FlagSnapshot {
    pub env: String,
    pub sha: String,
    /// Each requested flag and its value; adapters report an unset flag as `null`.
    pub values: BTreeMap<String, Value>,
}

/// One flag whose value differs between the tested and target environments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigMismatch {
    pub flag: String,
    /// The value the evidence was gathered under; `None` if the snapshot lacks it.
    pub test: Option<Value>,
    /// The target environment's value; `None` if its read lacks it.
    pub target: Option<Value>,
    /// The journeys whose evidence the difference voids.
    pub journeys: Vec<String>,
}

impl std::fmt::Display for ConfigMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let show = |value: &Option<Value>| value.as_ref().map_or("absent".into(), Value::to_string);
        write!(
            f,
            "CONFIG_MISMATCH {}: test={} target={} ({})",
            self.flag,
            show(&self.test),
            show(&self.target),
            self.journeys.join(", ")
        )
    }
}

/// The union of the journeys' `depends_on_flags`, sorted.
pub fn declared(journeys: &[Journey]) -> BTreeSet<String> {
    journeys.iter().flat_map(|j| j.depends_on_flags.iter().cloned()).collect()
}

/// Reads the declared flags' values in `env` through the adapter's `flags`
/// verb with `args: {keys}`. No declared flag means no call. Every requested
/// key must be answered; extra keys are dropped, never stored.
pub fn read(
    project_root: &Path,
    config: &ReleaseConfig,
    env: &str,
    sha: &str,
    journeys: &[Journey],
) -> Result<FlagSnapshot, CallError> {
    let keys = declared(journeys);
    let mut snapshot = FlagSnapshot { env: env.into(), sha: sha.into(), values: BTreeMap::new() };
    if keys.is_empty() {
        return Ok(snapshot);
    }
    let name = Verb::Flags.as_str();
    let data = call(
        project_root,
        config,
        Verb::Flags,
        Request { env, sha, args: serde_json::json!({ "keys": keys }), ..Request::default() },
    )?;
    // The parse error is dropped: it could quote adapter data.
    let Value::Object(mut answered) = data else {
        return Err(CallError::Protocol(name, "flags data is not a {flag: value} object".into()));
    };
    let mut missing = Vec::new();
    for key in keys {
        match answered.remove(&key) {
            Some(value) => {
                snapshot.values.insert(key, value);
            }
            None => missing.push(printable(&key)),
        }
    }
    if missing.is_empty() {
        Ok(snapshot)
    } else {
        Err(CallError::Protocol(name, format!("flags data omits {}", missing.join(", "))))
    }
}

/// Compares the evidence snapshot with the target environment's read, once,
/// at promotion. Each declared flag whose values differ (or that either side
/// lacks) is one mismatch naming both values and the affected journeys.
pub fn compare(test: &FlagSnapshot, target: &FlagSnapshot, journeys: &[Journey]) -> Vec<ConfigMismatch> {
    declared(journeys)
        .into_iter()
        .filter_map(|flag| {
            let (test, target) = (test.values.get(&flag), target.values.get(&flag));
            (test.is_none() || test != target).then(|| ConfigMismatch {
                journeys: journeys
                    .iter()
                    .filter(|j| j.depends_on_flags.contains(&flag))
                    .map(|j| j.id.clone())
                    .collect(),
                test: test.cloned(),
                target: target.cloned(),
                flag,
            })
        })
        .collect()
}

impl FlagSnapshot {
    /// One LEDGER.md header line: `Flags: <env>@<sha> {json}`. `|` is
    /// escaped so a value can never form a ledger table cell.
    pub fn ledger_line(&self) -> String {
        let values = serde_json::to_string(&self.values).unwrap_or_default().replace('|', "\\u007c");
        format!("{LEDGER_PREFIX}{}@{} {values}", printable(&self.env), printable(&self.sha))
    }

    /// The snapshot recorded in a ledger's `Flags:` header, if any.
    pub fn from_ledger(ledger: &str) -> Option<Self> {
        let rest = ledger.lines().find_map(|line| line.strip_prefix(LEDGER_PREFIX))?;
        let (target, values) = rest.split_once(' ')?;
        let (env, sha) = target.split_once('@')?;
        Some(Self { env: env.into(), sha: sha.into(), values: serde_json::from_str(values).ok()? })
    }
}
