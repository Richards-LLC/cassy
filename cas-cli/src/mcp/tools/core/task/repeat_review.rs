use std::path::Path;

use cas_store::TaskStore;
use cas_types::{EventEntityType, EventType, Task, Verification, VerificationStatus};
use rmcp::ErrorData as McpError;
use rmcp::model::ErrorCode;

use super::release_train;

const REVIEW_NOTE: &str = "whole-flow review:";
const KEEP_NOTE: &str = "keep release train:";

fn latest_repeated_rejection(root: &Path, task_id: &str) -> anyhow::Result<Option<Verification>> {
    let history = crate::store::open_verification_store(root)?.get_for_task(task_id)?;
    let mut rejected = history
        .into_iter()
        .filter(|verdict| verdict.status == VerificationStatus::Rejected);
    let latest = rejected.next();
    Ok(rejected.next().and(latest))
}

fn supervisor_decisions(root: &Path, task_id: &str) -> anyhow::Result<Vec<String>> {
    Ok(crate::store::open_event_store(root)?
        .list_for_entity(EventEntityType::Task, task_id, 10_000)?
        .into_iter()
        .filter(|event| event.event_type == EventType::TaskNoteAdded)
        .filter_map(|event| {
            Some(
                event
                    .metadata?
                    .get("supervisor_decision")?
                    .as_str()?
                    .to_owned(),
            )
        })
        .collect())
}

// gate[G-170 repair]: Whole-flow review required after repeated verifier rejections
fn refusal(task_id: &str) -> McpError {
    let prefix = crate::mcp::tools::core::guidance::caller_prefix();
    McpError {
        code: ErrorCode::INVALID_PARAMS,
        message: crate::gates::tag("G-170", format!(
            "Whole-flow review required for {task_id} after repeated verifier rejections. Have an independent reviewer compare every actor and operation against the design in a table and list every mismatch. Publish the table with `{prefix}artifact action=publish task_id={task_id} path=<file>`. A supervisor then links it with `{prefix}task action=notes id={task_id} note_type=decision notes=\"{REVIEW_NOTE} <artifact-id>\"`. Retry with a repair brief covering the full mismatch list."
        )).into(),
        data: None,
    }
}

pub(crate) fn require_review(root: &Path, task_id: &str) -> Result<Option<String>, McpError> {
    let internal = |error: anyhow::Error| McpError {
        code: ErrorCode::INTERNAL_ERROR,
        message: format!("Could not inspect whole-flow review for {task_id}: {error}").into(),
        data: None,
    };
    let Some(rejection) = latest_repeated_rejection(root, task_id).map_err(internal)? else {
        return Ok(None);
    };
    let review = (|| -> anyhow::Result<Option<String>> {
        let artifacts = cas_store::SqliteArtifactStore::open(root)?.list_for_task(task_id)?;
        for note in supervisor_decisions(root, task_id)? {
            let Some(id) = note.trim().strip_prefix(REVIEW_NOTE).map(str::trim) else {
                continue;
            };
            if artifacts.iter().any(|artifact| {
                artifact.id == id
                    && artifact.revocation.is_none()
                    && chrono::DateTime::parse_from_rfc3339(&artifact.created_at)
                        .is_ok_and(|created_at| created_at >= rejection.created_at)
            }) {
                return Ok(Some(id.to_owned()));
            }
        }
        Ok(None)
    })()
    .map_err(internal)?;
    review.map(Some).ok_or_else(|| refusal(task_id))
}

pub(crate) fn hold_repeated_rejection(
    root: &Path,
    store: &dyn TaskStore,
    task: &Task,
    verdict: &Verification,
    owner: &str,
) -> Option<String> {
    if verdict.status != VerificationStatus::Rejected || task.is_terminal() {
        return None;
    }
    match latest_repeated_rejection(root, &task.id) {
        Ok(None) => None,
        Ok(Some(_)) => match detach_from_train(root, store, task, Some(owner)) {
            Ok(detached) => Some(format!(
                "\n{}{}",
                if detached {
                    "Detached from the release train. "
                } else {
                    ""
                },
                refusal(&task.id).message
            )),
            Err(error) => Some(format!(
                "\nVerdict recorded; train detachment deferred: {error}. Retry this exact verdict.\n{}",
                refusal(&task.id).message
            )),
        },
        Err(error) => Some(format!(
            "\nVerdict recorded; repair held because verification history could not be read: {error}. Retry this exact verdict."
        )),
    }
}

pub(crate) fn detach_from_train(
    root: &Path,
    store: &dyn TaskStore,
    task: &Task,
    owner: Option<&str>,
) -> anyhow::Result<bool> {
    if task.is_terminal()
        || !store
            .get_parent_epic(&task.id)?
            .is_some_and(|train| release_train::is_release_train(&train))
    {
        return Ok(false);
    }
    let Some(rejection) = latest_repeated_rejection(root, &task.id)? else {
        return Ok(false);
    };
    let _lock = release_train::lock_trains_blocking(root)?;
    let current = store.get(&task.id)?;
    let Some(train) = store
        .get_parent_epic(&task.id)?
        .filter(release_train::is_release_train)
    else {
        return Ok(false);
    };
    if current.is_terminal()
        || supervisor_decisions(root, &task.id)?.iter().any(|note| {
            note.trim()
                .strip_prefix(KEEP_NOTE)
                .and_then(|body| body.split_once(':'))
                .is_some_and(|(train_id, reason)| {
                    train_id.trim() == train.id && !reason.trim().is_empty()
                })
        })
    {
        return Ok(false);
    }
    let mut detached = current.clone();
    if let Some(target) = detached
        .deliverables
        .work_target
        .as_mut()
        .filter(|target| train.effective_epic_branch() == Some(target.target_branch.as_str()))
    {
        let integration = train
            .deliverables
            .work_target
            .as_ref()
            .map(|target| target.target_branch.clone())
            .or_else(|| super::repo_context::integration_branch(root).map(|(_, branch)| branch))
            .ok_or_else(|| {
                anyhow::anyhow!("No integration target is recorded for train {}", train.id)
            })?;
        target.target_branch = integration;
        store.update_if_current(&detached, &current)?;
    }
    if !store.remove_dependency_of_type(
        &task.id,
        &train.id,
        cas_types::DependencyType::ParentChild,
    )? {
        return Ok(false);
    }
    let agents = crate::store::open_agent_store(root)?.list(None)?;
    let recipient = crate::supervisor_notices::resolve(
        &agents,
        train.epic_verification_owner.as_deref().or(owner),
        None,
    );
    let integrated = if current.deliverables.green_merge_commit.is_some() {
        " This delivery already has a recorded merge; detaching does not revert it. Inspect the train before cutting."
    } else {
        ""
    };
    crate::supervisor_notices::enqueue_with_context(
        root,
        &recipient,
        "repeat-review-train-detached",
        &format!(
            "{} left release train {} after repeated verifier rejections. An independent whole-flow review is required before another repair or reassignment.{integrated}",
            task.id, train.id
        ),
        &format!("{}:{}:{}", task.id, train.id, rejection.id),
        Some(
            serde_json::json!({"task_id":task.id,"train_id":train.id,"verification_id":rejection.id}),
        ),
    )?;
    Ok(true)
}
