//! The latest local close outcome, shared with the progress watchdog.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cas_types::Task;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Refusal {
    pub code: String,
    pub next_action: String,
    pub message: String,
    pub operator_action_required: bool,
    input: Value,
}

// Notes and timestamps are diagnostic traffic, not a resolution of the gate.
fn input(task: &Task) -> Value {
    serde_json::json!({
        "id": task.id, "status": task.status, "assignee": task.assignee,
        "branch": task.branch, "criteria": task.acceptance_criteria,
        "execution_note": task.execution_note, "risk": task.risk,
        "proof_targets": task.proof_targets,
        "target": task.deliverables.work_target,
        "start": task.deliverables.factory_branch_start,
        "anchor": task.deliverables.factory_branch_anchor,
        "outcome": task.deliverables.phase_outcome,
    })
}

fn path(root: &Path, id: &str) -> PathBuf {
    root.join("close-refusals").join(format!("{:x}.json", Sha256::digest(id.as_bytes())))
}

impl Refusal {
    pub fn key(&self) -> String {
        format!("{:x}", Sha256::digest(serde_json::to_vec(self).expect("serializable close refusal")))
    }
}

pub(crate) fn current(root: &Path, task: &Task) -> Result<Option<Refusal>> {
    let bytes = match std::fs::read(path(root, &task.id)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let refusal: Refusal = serde_json::from_slice(&bytes)?;
    Ok((refusal.input == input(task)).then_some(refusal))
}

/// Called after every close path, including early gates and background closes.
/// Preserve machine metadata when a gate supplies it; legacy text-only gates
/// share an explicit code and retain their complete remedy, without prose parsing.
pub(crate) fn record(
    root: &Path,
    id: &str,
    result: &Result<rmcp::model::CallToolResult, rmcp::ErrorData>,
) -> Result<()> {
    let (message, metadata) = match result {
        Ok(answer) if answer.is_error == Some(true) => (
            answer.content.iter().filter_map(|content| match &content.raw {
                rmcp::model::RawContent::Text(text) => Some(text.text.as_str()),
                _ => None,
            }).collect::<Vec<_>>().join("\n"),
            answer.structured_content.clone().unwrap_or(Value::Null),
        ),
        Err(error) if error.code == rmcp::model::ErrorCode::INVALID_PARAMS
            || error.code == rmcp::model::ErrorCode::INVALID_REQUEST =>
            (error.message.to_string(), error.data.clone().unwrap_or(Value::Null)),
        _ => {
            return match std::fs::remove_file(path(root, id)) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            };
        }
    };
    let task = crate::store::open_task_store_local(root)?.get(id)?;
    let refusal = Refusal {
        code: metadata.get("rule_id").or_else(|| metadata.get("code"))
            .and_then(Value::as_str).unwrap_or("task_close_refused").to_string(),
        next_action: metadata.get("next_action").and_then(Value::as_str)
            .unwrap_or("resolve_close_gate").to_string(),
        operator_action_required: metadata.get("operator_action_required").and_then(Value::as_bool) == Some(true)
            || metadata.get("required_actor").and_then(Value::as_str) == Some("operator"),
        message,
        input: input(&task),
    };
    let destination = path(root, id);
    let directory = destination.parent().expect("close receipt directory");
    std::fs::create_dir_all(directory)?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(file.as_file_mut(), &refusal)?;
    file.as_file().sync_all()?;
    file.persist(destination).context("persist close refusal")?;
    Ok(())
}
