//! Refusals, overrides and false positives as countable facts keyed by gate code.
//!
//! Each fact is an `events` row: entity type `gate`, entity id the code, and
//! metadata naming the task, every code of the same refusal and the detail.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use cas_store::EventStore;
use cas_types::{Event, EventEntityType, EventType};
use chrono::{DateTime, Utc};

use super::{UNCODED, codes_in, find};

/// Rows read per fact kind when summarizing; far above any release's volume.
const SUMMARY_LIMIT: usize = 200_000;
/// Refusal rows searched for a task's latest refusal when attributing an override.
const OVERRIDE_LOOKBACK: usize = 2_000;

pub const KINDS: [EventType; 3] = [
    EventType::GateRefused,
    EventType::GateOverridden,
    EventType::GateFalsePositive,
];

fn record(
    store: &dyn EventStore,
    kind: EventType,
    codes: &[&str],
    task_id: Option<&str>,
    detail: &str,
) -> Result<()> {
    let detail: String = detail.chars().take(400).collect();
    for code in codes {
        let event = Event::new(kind, EventEntityType::Gate, *code, format!("{kind} {code}"))
            .with_metadata(serde_json::json!({
                "task_id": task_id,
                "codes": codes,
                "detail": detail,
            }));
        store.record(&event)?;
    }
    Ok(())
}

/// Record one refusal per code its text emits. A refusal with no registered
/// code is recorded under [`UNCODED`], so a missing code is itself countable.
pub fn record_refusal(store: &dyn EventStore, task_id: Option<&str>, text: &str) -> Result<Vec<&'static str>> {
    let codes = codes_in(text);
    let recorded: Vec<&str> = if codes.is_empty() { vec![UNCODED] } else { codes.clone() };
    let heading = text.lines().find(|line| !line.trim().is_empty()).unwrap_or_default();
    record(store, EventType::GateRefused, &recorded, task_id, heading)?;
    Ok(codes)
}

/// Registered codes named anywhere in free text, bracketed or bare.
pub fn codes_named(text: &str) -> Vec<&'static str> {
    let mut codes = Vec::new();
    for token in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
        if let Some(row) = find(token)
            && !codes.contains(&row.code)
        {
            codes.push(row.code);
        }
    }
    codes
}

/// Codes of the latest recorded refusal of `task_id`.
pub fn last_refused_codes(store: &dyn EventStore, task_id: &str) -> Result<Vec<String>> {
    let latest = store
        .list_by_type(EventType::GateRefused, OVERRIDE_LOOKBACK)?
        .into_iter()
        .find(|event| task_of(event).as_deref() == Some(task_id));
    Ok(latest
        .and_then(|event| event.metadata)
        .and_then(|metadata| metadata.get("codes").cloned())
        .and_then(|codes| serde_json::from_value(codes).ok())
        .unwrap_or_default())
}

/// Record a successful supervisor override. The override is attributed to the
/// codes its reason names, else to the task's latest refusal, else [`UNCODED`].
pub fn record_override(store: &dyn EventStore, task_id: &str, reason: &str) -> Result<Vec<String>> {
    let mut codes: Vec<String> = codes_named(reason).into_iter().map(str::to_string).collect();
    if codes.is_empty() {
        codes = last_refused_codes(store, task_id)?;
    }
    if codes.is_empty() {
        codes.push(UNCODED.to_string());
    }
    let refs: Vec<&str> = codes.iter().map(String::as_str).collect();
    record(store, EventType::GateOverridden, &refs, Some(task_id), reason)?;
    Ok(codes)
}

/// An override made at a boundary that already knows the affected checks,
/// including Git hooks without an associated task.
pub fn record_override_codes(
    store: &dyn EventStore,
    codes: &[String],
    task_id: Option<&str>,
    reason: &str,
) -> Result<()> {
    anyhow::ensure!(!codes.is_empty(), "an override requires a gate code");
    anyhow::ensure!(!reason.trim().is_empty(), "an override requires a reason");
    let mut registered = Vec::new();
    for code in codes {
        let row = find(code).ok_or_else(|| anyhow::anyhow!("{code} is not a registered gate code"))?;
        if !registered.contains(&row.code) {
            registered.push(row.code);
        }
    }
    record(store, EventType::GateOverridden, &registered, task_id, reason)
}

/// Record a supervisor-declared false positive of a registered code.
pub fn record_false_positive(
    store: &dyn EventStore,
    code: &str,
    task_id: Option<&str>,
    reason: &str,
) -> Result<()> {
    let Some(row) = find(code) else {
        anyhow::bail!("{code} is not a registered gate code; `cas gates list` prints them");
    };
    record(store, EventType::GateFalsePositive, &[row.code], task_id, reason)
}

fn task_of(event: &Event) -> Option<String> {
    event.metadata.as_ref()?.get("task_id")?.as_str().map(str::to_string)
}

/// Fact counts for one code.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Counts {
    pub refused: u64,
    pub overridden: u64,
    pub false_positive: u64,
}

/// Fact counts per code recorded at or after `since`.
pub fn summarize(store: &dyn EventStore, since: DateTime<Utc>) -> Result<BTreeMap<String, Counts>> {
    let mut counts = BTreeMap::<String, Counts>::new();
    for kind in KINDS {
        for event in store.list_by_type(kind, SUMMARY_LIMIT)? {
            if event.created_at < since || event.entity_type != EventEntityType::Gate {
                continue;
            }
            let entry = counts.entry(event.entity_id).or_default();
            match kind {
                EventType::GateRefused => entry.refused += 1,
                EventType::GateOverridden => entry.overridden += 1,
                _ => entry.false_positive += 1,
            }
        }
    }
    Ok(counts)
}

/// Open the project event store and record a tool refusal; failures only warn.
pub fn observe_refusal(cas_root: &Path, task_id: Option<&str>, text: &str) {
    let recorded = crate::store::open_event_store(cas_root)
        .map_err(anyhow::Error::from)
        .and_then(|store| record_refusal(store.as_ref(), task_id, text));
    if let Err(error) = recorded {
        tracing::warn!(%error, "could not record gate refusal fact");
    }
}

/// Open the project event store and record an override; failures only warn.
pub fn observe_override(cas_root: &Path, task_id: &str, reason: &str) {
    let recorded = crate::store::open_event_store(cas_root)
        .map_err(anyhow::Error::from)
        .and_then(|store| record_override(store.as_ref(), task_id, reason));
    if let Err(error) = recorded {
        tracing::warn!(%error, "could not record gate override fact");
    }
}

fn result_text(result: &std::result::Result<rmcp::model::CallToolResult, rmcp::ErrorData>) -> Option<(bool, String)> {
    match result {
        Ok(answer) if answer.is_error == Some(true) => Some((
            true,
            answer
                .content
                .iter()
                .filter_map(|content| match &content.raw {
                    rmcp::model::RawContent::Text(text) => Some(text.text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )),
        Ok(_) => None,
        Err(error) => Some((false, error.message.to_string())),
    }
}

/// Record the gate facts of one task tool answer: every refusal that emits a
/// code; every refused `start` or `close` (uncoded when it emits none); and a
/// successful supervisor override. Park handoffs are not refusals.
pub fn observe_task_result(
    cas_root: &Path,
    action: &str,
    task_id: &str,
    override_reason: Option<&str>,
    result: &std::result::Result<rmcp::model::CallToolResult, rmcp::ErrorData>,
) {
    use crate::mcp::tools::core::task::lifecycle::close_ops::is_merge_handoff;
    let task = (!task_id.is_empty()).then_some(task_id);
    match result_text(result) {
        // A tagged answer is a refusal even when it lists MERGE REQUIRED
        // lines; an untagged one is uncoded unless it is the park handoff.
        Some((refused, text)) => {
            let uncoded_gate = refused
                && matches!(action, "start" | "close")
                && !text.lines().any(is_merge_handoff);
            if uncoded_gate || !codes_in(&text).is_empty() {
                observe_refusal(cas_root, task, &text);
            }
        }
        None => {
            if let (Some(reason), Some(task_id)) = (override_reason, task) {
                observe_override(cas_root, task_id, reason);
            }
        }
    }
}

/// The deny reason of a hook answer: a PreToolUse or PermissionRequest deny,
/// or a coded Stop-style `block`.
pub fn hook_denial(output: &cas_core::hooks::HookOutput) -> Option<&str> {
    use cas_core::hooks::HookSpecificOutput;
    match output.hook_specific_output.as_ref() {
        Some(HookSpecificOutput::PreToolUse {
            permission_decision: Some(decision),
            permission_decision_reason,
            ..
        }) if decision == "deny" => Some(permission_decision_reason.as_deref().unwrap_or_default()),
        Some(HookSpecificOutput::PermissionRequest {
            permission_decision,
            permission_decision_reason,
        }) if permission_decision == "deny" => Some(permission_decision_reason.as_deref().unwrap_or_default()),
        _ => output
            .decision
            .as_deref()
            .filter(|decision| *decision == "block")
            .and_then(|_| output.reason.as_deref())
            .filter(|reason| !codes_in(reason).is_empty()),
    }
}

/// Record a hook denial as a refusal fact of the guard codes it emits.
pub fn observe_hook_output(cas_root: &Path, output: &cas_core::hooks::HookOutput) {
    if let Some(reason) = hook_denial(output) {
        observe_refusal(cas_root, None, reason);
    }
}
