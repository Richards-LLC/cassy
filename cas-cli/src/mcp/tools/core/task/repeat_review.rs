use std::path::Path;

use cas_store::{REPEAT_REVIEW_REQUIRED, RequestChangesOutcome, SqliteVerificationStore, StoreError, TaskStore, request_changes_for_reviewed_delivery};
use cas_types::{Event, EventEntityType, EventType, Task, Verification, VerificationStatus};
use rmcp::ErrorData as McpError;
use rmcp::model::ErrorCode;

use super::release_train;

const REVIEW_NOTE: &str = "whole-flow review:";
const KEEP_NOTE: &str = "keep release train:";

fn latest_repeated_rejection(root: &Path, task_id: &str) -> anyhow::Result<Option<(String, chrono::DateTime<chrono::Utc>)>> {
    Ok(SqliteVerificationStore::open(root)?.latest_repeated_rejection(task_id)?)
}

fn task_events(root: &Path, task_id: &str) -> anyhow::Result<Vec<Event>> {
    Ok(crate::store::open_event_store(root)?.list_for_entity(EventEntityType::Task, task_id, 10_000)?)
}

fn supervisor_decisions(events: &[Event]) -> impl Iterator<Item = &str> {
    events.iter()
        .filter(|event| event.event_type == EventType::TaskNoteAdded)
        .filter_map(|event| event.metadata.as_ref()?.get("supervisor_decision")?.as_str())
}

// gate[G-170 repair]: Whole-flow review required after repeated verifier rejections
fn refusal(task_id: &str) -> McpError {
    let prefix = crate::mcp::tools::core::guidance::caller_prefix();
    McpError {
        code: ErrorCode::INVALID_PARAMS,
        message: crate::gates::tag("G-170", format!(
            "Whole-flow review required for {task_id} after repeated verifier rejections. Have an independent reviewer publish a Markdown review from the task's artifacts directory: a table with Actor, Operation, Design and Observed columns that compares every actor and operation against the design, followed by a Mismatches heading and a list of every mismatch. Publish it with `{prefix}artifact action=publish task_id={task_id} path=<file>`. A supervisor then links it with `{prefix}task action=notes id={task_id} note_type=decision notes=\"{REVIEW_NOTE} <artifact-id>\"`. Retry with a repair brief covering the full mismatch list."
        )).into(),
        data: None,
    }
}

pub(crate) fn require_review(root: &Path, task_id: &str) -> Result<Option<String>, McpError> {
    Ok(admitted_review(root, task_id)?.map(|(_, artifact)| artifact))
}

fn admitted_review(root: &Path, task_id: &str) -> Result<Option<(String, String)>, McpError> {
    let internal = |error: anyhow::Error| McpError {
        code: ErrorCode::INTERNAL_ERROR,
        message: format!("Could not inspect whole-flow review for {task_id}: {error}").into(),
        data: None,
    };
    let Some((rejection, rejected_at)) = latest_repeated_rejection(root, task_id).map_err(internal)? else {
        return Ok(None);
    };
    let events = task_events(root, task_id).map_err(internal)?;
    for id in supervisor_decisions(&events).filter_map(|note| note.trim().strip_prefix(REVIEW_NOTE).map(str::trim)) {
        if let Ok(artifact) = crate::artifacts::lookup_metadata(root, Some(task_id), id)
            && chrono::DateTime::parse_from_rfc3339(&artifact.created_at).is_ok_and(|created| created >= rejected_at)
            && let Ok(markdown) = crate::artifacts::proof::read_verified_text(root, &artifact, 256 * 1024)
            && is_whole_flow_review(&markdown)
        {
            return Ok(Some((rejection, id.to_owned())));
        }
    }
    Err(refusal(task_id))
}

fn is_whole_flow_review(markdown: &str) -> bool {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    let mut text = String::new();
    let mut cells = Vec::new();
    let mut table = false;
    let mut matrix = false;
    let mut heading = false;
    let mut mismatches = false;
    let mut mismatch_item = false;
    for event in Parser::new_ext(markdown, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::TableHead | Tag::TableRow) => cells.clear(),
            Event::Start(Tag::TableCell | Tag::Heading { .. } | Tag::Item) => text.clear(),
            Event::End(TagEnd::TableCell) => cells.push(text.trim().to_owned()),
            Event::End(TagEnd::TableHead) => {
                table = cells.iter().map(|cell| cell.to_ascii_lowercase())
                    .eq(["actor", "operation", "design", "observed"].map(String::from));
            }
            Event::End(TagEnd::TableRow) if table => {
                matrix |= cells.len() == 4 && cells.iter().all(|cell| !cell.is_empty());
            }
            Event::End(TagEnd::Table) => table = false,
            Event::End(TagEnd::Heading(_)) => {
                heading = text.trim().eq_ignore_ascii_case("mismatches");
                mismatches = false;
            }
            Event::Start(Tag::List(_)) if heading => mismatches = true,
            Event::End(TagEnd::List(_)) => mismatches = false,
            Event::End(TagEnd::Item) if mismatches => mismatch_item |= !text.trim().is_empty(),
            Event::Text(part) | Event::Code(part) => text.push_str(&part),
            _ => {}
        }
    }
    matrix && mismatch_item
}

#[test]
fn whole_flow_review_predicate_gh2373() {
    let header = "| Actor | Operation | Design | Observed |";
    let row = "| Owner | Abort | Allowed | Unreachable |";
    let mismatches = "## Mismatches\n\n- Abort is unreachable.";
    for (head, body, tail, valid) in [
        (header, row, mismatches, true),
        ("| Actor | Design | Operation | Observed |", row, mismatches, false),
        (header, "| Owner | Abort | | Unreachable |", mismatches, false),
        (header, row, "## Mismatches\n\nAbort is unreachable.", false),
        (header, row, "## Mismatches\n\n## Other\n\n- Abort is unreachable.", false),
    ] {
        let markdown = format!("{head}\n| --- | --- | --- | --- |\n{body}\n\n{tail}\n");
        assert_eq!(is_whole_flow_review(&markdown), valid, "{markdown}");
    }
}

pub(crate) fn mismatch_brief(artifact_id: &str) -> String {
    format!("\nWhole-flow mismatch list: {artifact_id}. Use the review's full list for this repair.")
}

pub(crate) fn repair_transition<T>(
    root: &Path, task_id: &str, reason: &str,
    transition: impl FnOnce(&str, Option<&str>) -> Result<T, McpError>,
) -> Result<(T, String), McpError> {
    let mut brief = reason.trim().to_string();
    let review = admitted_review(root, task_id)?;
    if let Some((_, artifact)) = &review {
        brief.push_str(&mismatch_brief(artifact));
    }
    transition(&brief, review.as_ref().map(|(rejection, _)| rejection.as_str()))
        .map(|outcome| (outcome, brief))
}

pub(crate) fn transition_error(task_id: &str, error: StoreError) -> McpError {
    if matches!(&error, StoreError::Parse(reason) if reason == REPEAT_REVIEW_REQUIRED) {
        return refusal(task_id);
    }
    McpError::new(ErrorCode::INVALID_PARAMS, format!("request_changes rejected: {error}"), None)
}

pub(crate) fn repair_parked(
    root: &Path, task_id: &str, owner: &str, reason: &str,
) -> anyhow::Result<(RequestChangesOutcome, String)> {
    repair_transition(root, task_id, reason, |brief, reviewed| {
        request_changes_for_reviewed_delivery(root, task_id, owner, brief, &[], reviewed)
            .map_err(|error| transition_error(task_id, error))
    }).map_err(|error| anyhow::anyhow!(error.message.into_owned()))
}

pub(crate) struct RepeatHold {
    pub detail: String,
    pub notice_queued: bool,
}

pub(crate) fn hold_repeated_rejection(
    root: &Path,
    store: &dyn TaskStore,
    task: &Task,
    verdict: &Verification,
    owner: &str,
) -> Option<RepeatHold> {
    if verdict.status != VerificationStatus::Rejected || task.is_terminal() {
        return None;
    }
    match latest_repeated_rejection(root, &task.id) {
        Ok(None) => None,
        Ok(Some((latest, _))) => Some(match apply_repeat_hold(root, store, task, &verdict.id, &latest, Some(owner)) {
            Ok((detail, notice_queued)) => RepeatHold { detail: format!("\n{detail}"), notice_queued },
            Err(error) => RepeatHold {
                detail: format!("\nVerdict recorded; repeat-review action incomplete: {error}. Retry this exact verdict.\n{}", refusal(&task.id).message),
                notice_queued: false,
            },
        }),
        Err(error) => Some(RepeatHold {
            detail: format!("\nVerdict recorded; repair held because verification history could not be read: {error}. Retry this exact verdict."),
            notice_queued: false,
        }),
    }
}

pub(crate) fn detach_at_park(
    root: &Path, store: &dyn TaskStore, task: &Task,
) -> anyhow::Result<bool> {
    if task.is_terminal() {
        return Ok(false);
    }
    let Some((rejection, _)) = latest_repeated_rejection(root, &task.id)? else {
        return Ok(false);
    };
    let Some(intent) = detach_from_train(root, store, &task.id, &rejection, true, None)? else { return Ok(false) };
    enqueue_hold(root, &task.id, &rejection, Some(&intent), None)?;
    Ok(true)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct DetachIntent {
    verdict: String,
    train_id: String,
    owner: Option<String>,
    integrated: bool,
}

fn apply_repeat_hold(
    root: &Path, store: &dyn TaskStore, task: &Task, verdict: &str, latest: &str, owner: Option<&str>,
) -> anyhow::Result<(String, bool)> {
    let intent = detach_from_train(root, store, &task.id, verdict, verdict == latest, owner)?;
    if intent.is_some() || verdict == latest {
        return enqueue_hold(root, &task.id, verdict, intent.as_ref(), owner).map(|detail| (detail, true));
    }
    Ok((refusal(&task.id).message.into_owned(), false))
}

fn detach_from_train(
    root: &Path, store: &dyn TaskStore, task_id: &str, verdict: &str, allow_new: bool, owner: Option<&str>,
) -> anyhow::Result<Option<DetachIntent>> {
    let _lock = release_train::lock_trains_blocking(root)?;
    let current = store.get(task_id)?;
    let parent = store.get_parent_epic(task_id)?.filter(release_train::is_release_train);
    let events = task_events(root, task_id)?;
    let pending = events.iter().filter_map(|event| {
        serde_json::from_value::<DetachIntent>(event.metadata.as_ref()?.get("repeat_review_detach")?.clone()).ok()
    }).find(|intent| intent.verdict == verdict);
    let intent = if let Some(intent) = pending {
        intent
    } else {
        let Some(train) = parent.as_ref().filter(|_| allow_new && !current.is_terminal()) else { return Ok(None) };
        if supervisor_decisions(&events).any(|note| {
            note.trim()
                .strip_prefix(KEEP_NOTE)
                .and_then(|body| body.split_once(':'))
                .is_some_and(|(train_id, reason)| {
                    train_id.trim() == train.id && !reason.trim().is_empty()
                })
        }) { return Ok(None); }
        let intent = DetachIntent {
            verdict: verdict.to_owned(), train_id: train.id.clone(),
            owner: train.epic_verification_owner.clone().or_else(|| owner.map(str::to_owned)),
            integrated: current.deliverables.green_merge_commit.is_some(),
        };
        crate::store::open_event_store(root)?.record(
            &Event::new(EventType::TaskNoteAdded, EventEntityType::Task, task_id, "Release train detachment notice pending")
                .with_metadata(serde_json::json!({"repeat_review_detach": intent})),
        )?;
        intent
    };
    let Some(train) = parent.filter(|train| train.id == intent.train_id) else { return Ok(Some(intent)) };
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
    store.remove_dependency_of_type(
        task_id,
        &train.id,
        cas_types::DependencyType::ParentChild,
    )?;
    Ok(Some(intent))
}

fn enqueue_hold(
    root: &Path, task_id: &str, verdict: &str, detached: Option<&DetachIntent>, owner: Option<&str>,
) -> anyhow::Result<String> {
    let agents = crate::store::open_agent_store(root)?.list(None)?;
    let recipient = crate::supervisor_notices::resolve(
        &agents,
        detached.and_then(|intent| intent.owner.as_deref()).or(owner),
        None,
    );
    let integrated = if detached.is_some_and(|intent| intent.integrated) {
        " This delivery already has a recorded merge; detaching does not revert it. Inspect the train before cutting."
    } else {
        ""
    };
    let detail = detached.map_or_else(
        || format!("{task_id} held after repeated verifier rejections. {}", refusal(task_id).message),
        |intent| format!("{task_id} detached from release train {} after repeated verifier rejections.{integrated} {}", intent.train_id, refusal(task_id).message),
    );
    crate::supervisor_notices::enqueue_keyed(
        root,
        &recipient,
        if detached.is_some() { "repeat-review-train-detached" } else { "premerge_verification" },
        &detail,
        &format!("repeat-review:{task_id}:{verdict}"),
        Some(serde_json::json!({"task_id":task_id,"train_id":detached.map(|intent| &intent.train_id),"verification_id":verdict,"rejection_action":"hold_escalate"})),
    )?;
    Ok(detail)
}
