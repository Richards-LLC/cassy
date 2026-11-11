//! Factory check registry (cas-b5cd).
//!
//! Every factory check carries one comment declaration at its site:
//! `gate[<CODE> <stage> [integrity|as-of]]: <title>`, where CODE is a family letter (G close,
//! park, merge and release gates; H agent guards; W watchdogs; N notices;
//! C CI and release checks), a dash and a number. `cas-cli/build.rs` scans
//! the source for those declarations and generates [`REGISTRY`], so the count
//! is measured from code: deleting a check's declaration deletes its row.
//!
//! Refusals emit their code as a `[CODE]` token through [`tag`]. The task
//! router and the outer hook entry read those tokens from every refusal and
//! record a [`facts`] row keyed by the code; tagged accepted overrides and supervisor-declared
//! false positives are recorded the same way.

pub mod facts;

use std::collections::BTreeSet;

#[cfg(test)]
mod registry_tests;

/// One registered check, generated from its source declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct GateRow {
    pub code: &'static str,
    pub stage: &'static str,
    pub title: &'static str,
    /// `path:line` of the declaration, relative to the repository root.
    pub source: &'static str,
    /// Mandatory at delivery stages; other checks always use current policy.
    pub approval_policy: Option<ApprovalPolicy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    Integrity,
    AsOf,
}

/// An immutable, self-contained registry membership version. Unlike a hash,
/// this can answer historical membership without a second registry store.
/// PR 2 persists it on the approval fact keyed by cycle id + approved anchor,
/// and carries it unchanged when that approval carries forward.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct RegistryVersion(BTreeSet<String>);

/// Capture at approval write time, never at dispatch creation or delivery mint.
pub fn registry_version_in_force() -> RegistryVersion {
    RegistryVersion(REGISTRY.iter().map(|row| row.code.to_string()).collect())
}

/// Decide against the supplied approval stamp; this performs no approval lookup.
/// Missing stamps preserve existing checks, and unknown codes fail closed.
pub fn check_applies(code: &str, _stamp: Option<&RegistryVersion>) -> anyhow::Result<bool> {
    find(code).ok_or_else(|| anyhow::anyhow!("unregistered gate: {code}"))?;
    Ok(true)
}

impl GateRow {
    pub fn family(&self) -> Family {
        Family::of(self.code).expect("generated codes carry a family")
    }
}

include!(concat!(env!("OUT_DIR"), "/gate_registry.rs"));

/// Check families, in registry order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum Family {
    Gate,
    Guard,
    Watchdog,
    Notice,
    Ci,
}

impl Family {
    pub const ALL: [Family; 5] = [Self::Gate, Self::Guard, Self::Watchdog, Self::Notice, Self::Ci];

    pub fn of(code: &str) -> Option<Self> {
        match code.split_once('-')?.0 {
            "G" => Some(Self::Gate),
            "H" => Some(Self::Guard),
            "W" => Some(Self::Watchdog),
            "N" => Some(Self::Notice),
            "C" => Some(Self::Ci),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Gate => "G close, park, merge and release gates",
            Self::Guard => "H agent guards",
            Self::Watchdog => "W watchdogs",
            Self::Notice => "N notices",
            Self::Ci => "C CI and release checks",
        }
    }
}

/// Code recorded for a refusal that carries no registered code.
pub const UNCODED: &str = "uncoded";

pub fn find(code: &str) -> Option<&'static GateRow> {
    REGISTRY.iter().find(|row| row.code == code)
}

/// Append the gate's code to a refusal message, as the `[CODE]` token that
/// [`codes_in`] reads back.
pub fn tag(code: &'static str, message: impl Into<String>) -> String {
    debug_assert!(find(code).is_some(), "{code} has no gate[...] declaration");
    let mut message = message.into();
    message.push_str("\n\n[");
    message.push_str(code);
    message.push(']');
    message
}

/// Mark a successful operation's explicit gate bypass. The common result
/// observer consumes this machine tag without inferring acceptance from flags.
pub fn override_tag(code: &'static str, message: impl Into<String>) -> String {
    debug_assert!(find(code).is_some(), "{code} has no gate[...] declaration");
    format!("{}\n\n[override {code}]", message.into())
}

/// Registered codes emitted as `[CODE]` tokens in `text`, in order, deduplicated.
pub fn codes_in(text: &str) -> Vec<&'static str> {
    tagged_codes(text, "")
}

/// Preserve legacy non-error advisory responses while identifying a policy refusal.
pub fn refusal_tag(code: &'static str, message: impl Into<String>) -> String {
    format!("{}\n\n[refusal {code}]", tag(code, message))
}

pub fn refusal_codes_in(text: &str) -> Vec<&'static str> { tagged_codes(text, "refusal ") }

pub fn override_codes_in(text: &str) -> Vec<&'static str> {
    tagged_codes(text, "override ")
}

fn tagged_codes(text: &str, prefix: &str) -> Vec<&'static str> {
    let mut codes = Vec::new();
    for (start, _) in text.match_indices('[') {
        let rest = &text[start + 1..];
        let Some(end) = rest.find(']') else { break };
        if let Some(row) = rest[..end].strip_prefix(prefix).and_then(find)
            && !codes.contains(&row.code)
        {
            codes.push(row.code);
        }
    }
    codes
}

/// Carry accepted policy exceptions through a successful MCP response.
/// The common observer deduplicates codes; a later refusal consumes none.
pub(crate) fn append_override_tags(result: &mut rmcp::model::CallToolResult, codes: &[&'static str]) {
    if result.is_error != Some(true) && !codes.is_empty() {
        let text = codes.iter().fold(String::new(), |text, code| override_tag(code, text));
        result.content.push(rmcp::model::Content::text(text));
    }
}
