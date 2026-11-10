//! Input-bound close gate facts in the existing task execution state.
use std::path::Path;

use anyhow::Result;
use cas_store::TaskStore;
pub(crate) use cas_types::TaskCloseRefusal as Refusal;
use cas_types::{Task, TaskStatus};
use rmcp::model::CallToolResult;
use serde_json::Value;
use sha2::{Digest, Sha256};

// Notes and timestamps are diagnostic traffic, not a resolution of the gate.
fn input_digest(task: &Task) -> String {
    let input = serde_json::json!({
        "id": task.id, "status": task.status, "assignee": task.assignee,
        "branch": task.branch, "criteria": task.acceptance_criteria,
        "execution_note": task.execution_note, "risk": task.risk,
        "proof_targets": task.proof_targets,
        "target": task.deliverables.work_target,
        "start": task.deliverables.factory_branch_start,
        "anchor": task.deliverables.factory_branch_anchor,
        "outcome": task.deliverables.phase_outcome,
    });
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&input).expect("serializable close inputs"))
    )
}

pub(crate) fn key(refusal: &Refusal) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(refusal).expect("serializable close refusal"))
    )
}

pub(crate) fn current(store: &dyn TaskStore, task: &Task) -> Result<Option<Refusal>> {
    let Some(state) = store.get_execution_state(&task.id)? else {
        return Ok(None);
    };
    let Some(value) = state.get("close_refusal") else {
        return Ok(None);
    };
    let refusal: Refusal = serde_json::from_value(value.clone())?;
    if !task.is_terminal()
        && task.status != TaskStatus::AwaitingMerge
        && refusal.input_digest == input_digest(task)
    {
        return Ok(Some(refusal));
    }
    store.patch_execution_state(&task.id, &serde_json::json!({"close_refusal": null}))?;
    Ok(None)
}

/// Give an actual gate a machine identity at its rejecting branch. Ordinary
/// parameter errors and park handoffs never acquire a gate identity here.
pub(crate) fn gate(code: &str, next_action: &str, message: impl Into<String>) -> CallToolResult {
    CallToolResult {
        structured_content: Some(serde_json::json!({
            "code": code, "next_action": next_action, "operator_action_required": false,
        })),
        ..CallToolResult::error(vec![rmcp::model::Content::text(message.into())])
    }
}

/// Called after every close path, including early gates and background closes.
/// Only explicit structured gate identities can hold the watchdog. A later
/// successful close, untyped failure or park clears the previous gate fact.
pub(crate) fn record(
    root: &Path,
    id: &str,
    result: &Result<CallToolResult, rmcp::ErrorData>,
) -> Result<()> {
    use super::lifecycle::close_ops::is_merge_handoff;
    let metadata = match result {
        Ok(answer)
            if answer.is_error == Some(true)
                && !answer.content.iter().any(|content| {
                    matches!(&content.raw,
                rmcp::model::RawContent::Text(text) if text.text.lines().any(is_merge_handoff))
                }) =>
        {
            answer.structured_content.as_ref()
        }
        Err(error) if !error.message.lines().any(is_merge_handoff) => error.data.as_ref(),
        _ => None,
    };
    let store = crate::store::open_task_store_local(root)?;
    let task = store.get(id)?;
    let refusal = metadata.and_then(|metadata| {
        if task.is_terminal() || task.status == TaskStatus::AwaitingMerge {
            return None;
        }
        let code = metadata
            .get("rule_id")
            .or_else(|| metadata.get("code"))
            .and_then(Value::as_str)
            .filter(|code| !code.trim().is_empty())?;
        Some(Refusal {
            code: code.to_string(),
            next_action: metadata
                .get("next_action")
                .and_then(Value::as_str)
                .filter(|action| !action.trim().is_empty())
                .unwrap_or("resolve_close_gate")
                .to_string(),
            operator_action_required: metadata
                .get("operator_action_required")
                .and_then(Value::as_bool)
                == Some(true)
                || metadata.get("required_actor").and_then(Value::as_str) == Some("operator"),
            input_digest: input_digest(&task),
        })
    });
    if refusal.is_some()
        || store
            .get_execution_state(id)?
            .is_some_and(|state| state.get("close_refusal").is_some())
    {
        store.patch_execution_state(id, &serde_json::json!({"close_refusal": refusal}))?;
    }
    Ok(())
}
