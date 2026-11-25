use std::path::Path;

use cas_store::{REPEAT_REVIEW_REQUIRED, RequestChangesOutcome, SqliteVerificationStore, StoreError, TaskStore, VerificationStore, request_changes_for_reviewed_delivery};
use cas_types::{Event, EventEntityType, EventType};
use rmcp::ErrorData as McpError;
use rmcp::model::ErrorCode;

use super::release_train;

const REVIEW_NOTE: &str = "whole-flow review:";
const KEEP_NOTE: &str = "keep release train:";
const REPAIR_HOLD_RULE: &str = "repeat_review_required";

fn latest_repeated_rejection(root: &Path, task_id: &str) -> anyhow::Result<Option<(String, chrono::DateTime<chrono::Utc>, u64)>> {
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

// gate[G-171 repair]: Whole-flow review required after repeated verifier rejections
fn repeat_review_refusal(task_id: &str, rejection_id: Option<&str>, count: Option<u64>) -> McpError {
    let prefix = crate::mcp::tools::core::guidance::caller_prefix();
    let count = count.map_or_else(|| "at least 2".into(), |count| count.to_string());
    McpError {
        code: ErrorCode::INVALID_PARAMS,
        message: crate::gates::tag("G-171", format!(
            "Whole-flow review required for {task_id}: {count} all-time verifier rejections. Have an independent reviewer publish a Markdown review from the task's artifacts directory: a table with Actor, Operation, Design and Observed columns that compares every actor and operation against the design, followed by a Mismatches heading and a list of every mismatch. Publish it with `{prefix}artifact action=publish task_id={task_id} path=<file>`. A supervisor then links it with `{prefix}task action=notes id={task_id} note_type=decision notes=\"{REVIEW_NOTE} <artifact-id>\"`. Retry with a repair brief covering the full mismatch list."
        )).into(),
        data: Some(serde_json::json!({"rule_id":REPAIR_HOLD_RULE, "task_id":task_id, "rejection_id":rejection_id})),
    }
}

pub(crate) fn is_repair_hold(error: &McpError) -> bool {
    error.data.as_ref().and_then(|data| data.get("rule_id")).and_then(|rule| rule.as_str()) == Some(REPAIR_HOLD_RULE)
}

pub(crate) fn require_review(root: &Path, task_id: &str) -> Result<Option<String>, McpError> {
    Ok(admit_repair(root, task_id)?.artifact_id)
}

#[derive(Default)]
pub(crate) struct RepairAdmission {
    reviewed_rejection: Option<String>,
    pub(crate) artifact_id: Option<String>,
}

impl RepairAdmission {
    pub(crate) fn reviewed(&self) -> Option<&str> {
        self.reviewed_rejection.as_deref()
    }

    pub(crate) fn brief(&self) -> String {
        self.artifact_id.as_deref().map(mismatch_brief).unwrap_or_default()
    }
}

pub(crate) fn admit_repair(root: &Path, task_id: &str) -> Result<RepairAdmission, McpError> {
    let internal = |error: anyhow::Error| McpError {
        code: ErrorCode::INTERNAL_ERROR,
        message: format!("Could not inspect whole-flow review for {task_id}: {error}").into(),
        data: None,
    };
    let Some((rejection, rejected_at, count)) = latest_repeated_rejection(root, task_id).map_err(internal)? else {
        return Ok(RepairAdmission::default());
    };
    let events = task_events(root, task_id).map_err(internal)?;
    for id in supervisor_decisions(&events).filter_map(|note| note.trim().strip_prefix(REVIEW_NOTE).map(str::trim)) {
        if let Ok(artifact) = crate::artifacts::lookup_metadata(root, Some(task_id), id)
            && chrono::DateTime::parse_from_rfc3339(&artifact.created_at).is_ok_and(|created| created >= rejected_at)
            && let Ok(markdown) = crate::artifacts::proof::read_verified_text(root, &artifact, 256 * 1024)
            && is_whole_flow_review(&markdown)
        {
            return Ok(RepairAdmission { reviewed_rejection: Some(rejection), artifact_id: Some(id.to_owned()) });
        }
    }
    Err(repeat_review_refusal(task_id, Some(&rejection), Some(count)))
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

#[cfg(test)]
pub(crate) async fn publish_test_review(root: &Path, task_id: &str, supervisor_id: &str) -> String {
    use rmcp::handler::server::wrapper::Parameters;

    let config = crate::config::Config::load(root).unwrap();
    let base = crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    let [task_dir, _] = crate::config::factory_task_artifact_dirs(root, &base, task_id);
    std::fs::create_dir_all(&task_dir).unwrap();
    let path = task_dir.join("whole-flow-review.md");
    std::fs::write(&path, "| Actor | Operation | Design | Observed |\n| --- | --- | --- | --- |\n| Owner | Abort | Allowed | Unreachable |\n\n## Mismatches\n- Abort is unreachable.\n").unwrap();
    let supervisor = crate::mcp::CasCore::with_daemon(root.to_path_buf(), None, None);
    supervisor.set_agent_id_for_testing(supervisor_id.into());
    supervisor.artifact_publish(Parameters(serde_json::from_value(serde_json::json!({
        "action": "publish", "task_id": task_id, "path": path,
    })).unwrap())).await.unwrap();
    let artifact = cas_store::SqliteArtifactStore::open(root).unwrap().list_for_task(task_id)
        .unwrap().into_iter().find(|artifact| artifact.name == "whole-flow-review.md").unwrap();
    supervisor.cas_task_notes(Parameters(crate::mcp::tools::TaskNotesRequest {
        id: task_id.into(), note_type: "decision".into(), note: format!("{REVIEW_NOTE} {}", artifact.id),
        reason: None, supervisor_override: None,
    })).await.unwrap();
    artifact.id
}

pub(crate) fn repair_transition<T>(
    root: &Path, task_id: &str, reason: &str,
    transition: impl FnOnce(&str, Option<&str>) -> Result<T, McpError>,
) -> Result<(T, String), McpError> {
    let admission = admit_repair(root, task_id)?;
    let brief = format!("{}{}", reason.trim(), admission.brief());
    transition(&brief, admission.reviewed())
        .map(|outcome| (outcome, brief))
}

pub(crate) fn transition_error(root: &Path, task_id: &str, error: StoreError) -> McpError {
    if matches!(&error, StoreError::Parse(reason) if reason == REPEAT_REVIEW_REQUIRED) {
        let latest = latest_repeated_rejection(root, task_id).ok().flatten();
        return repeat_review_refusal(task_id, latest.as_ref().map(|(id, _, _)| id.as_str()), latest.as_ref().map(|(_, _, count)| *count));
    }
    McpError::new(ErrorCode::INVALID_PARAMS, format!("request_changes rejected: {error}"), None)
}

pub(crate) fn repair_parked(
    root: &Path, task_id: &str, owner: &str, reason: &str,
) -> Result<(RequestChangesOutcome, String), McpError> {
    repair_transition(root, task_id, reason, |brief, reviewed| {
        request_changes_for_reviewed_delivery(root, task_id, owner, brief, &[], reviewed)
            .map_err(|error| transition_error(root, task_id, error))
    })
}

pub(crate) struct RepeatHold {
    pub detail: String,
}

fn transition_key(task_id: &str, rejection: &str) -> String {
    format!("repeat-review:{task_id}:{rejection}")
}

pub(crate) fn settle(root: &Path, task_id: &str) -> anyhow::Result<Option<RepeatHold>> {
    let verifications = SqliteVerificationStore::open(root)?;
    let Some((rejection, _, count)) = verifications.latest_repeated_rejection(task_id)? else {
        return Ok(None);
    };
    let store = crate::store::open_task_store(root)?;
    if store.get(task_id)?.is_terminal() {
        return Ok(None);
    }
    let verdict = verifications.get(&rejection)?;
    let owner = verdict.dispatch_id.as_deref()
        .map(|dispatch| cas_store::get_verification_dispatch(root, dispatch)).transpose()?
        .map(|dispatch| dispatch.owner_agent_id);
    let intent = detach_from_train(root, store.as_ref(), task_id, &rejection, owner.as_deref())?;
    let detail = enqueue_hold(root, task_id, &rejection, count, intent.as_ref(), owner.as_deref())?;
    Ok(Some(RepeatHold { detail: format!("\n{detail}") }))
}

pub(crate) fn settle_repeated_rejections(root: &Path, now: chrono::DateTime<chrono::Utc>) -> anyhow::Result<()> {
    let verifications = SqliteVerificationStore::open(root)?;
    let notices = crate::store::open_supervisor_queue_store(root)?;
    for task_id in verifications.tasks_with_repeated_rejections(now)? {
        let Some((rejection, _, _)) = verifications.latest_repeated_rejection(&task_id)? else { continue };
        if notices.get_by_transition_key(&transition_key(&task_id, &rejection))?
            .is_some_and(|notice| notice.prompt_delivered_at.is_some() || notice.supervisor_id == "operator")
        {
            continue;
        }
        if let Err(error) = settle(root, &task_id) {
            tracing::warn!(%task_id, %error, "repeat-review settlement unsuccessful");
        }
    }
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct DetachIntent {
    verdict: String,
    train_id: String,
    owner: Option<String>,
    integrated: bool,
    #[serde(default)]
    requires_target_merge: bool,
}

fn detach_from_train(
    root: &Path, store: &dyn TaskStore, task_id: &str, verdict: &str, owner: Option<&str>,
) -> anyhow::Result<Option<DetachIntent>> {
    let _lock = release_train::lock_trains_blocking(root)?;
    let current = store.get(task_id)?;
    let parent = store.get_parent_epic(task_id)?.filter(release_train::is_release_train);
    let events = task_events(root, task_id)?;
    let pending = events.iter().filter_map(|event| {
        serde_json::from_value::<DetachIntent>(event.metadata.as_ref()?.get("repeat_review_detach")?.clone()).ok()
    }).find(|intent| intent.verdict == verdict);
    if current.is_terminal() || (pending.is_none() && parent.as_ref().is_some_and(|train| {
        supervisor_decisions(&events).any(|note| {
            note.trim().strip_prefix(KEEP_NOTE).and_then(|body| body.split_once(':'))
                .is_some_and(|(train_id, reason)| train_id.trim() == train.id && !reason.trim().is_empty())
        })
    })) {
        return Ok(None);
    }
    let mut replacement = parent.as_ref().and_then(|train| {
        let target = current.deliverables.work_target.as_ref()?;
        (train.effective_epic_branch() == Some(target.target_branch.as_str())).then(|| target.clone())
    });
    if let (Some(target), Some(train)) = (replacement.as_mut(), parent.as_ref()) {
        target.target_branch = train.deliverables.work_target.as_ref()
            .map(|target| target.target_branch.clone())
            .or_else(|| super::repo_context::integration_branch(root).map(|(_, branch)| branch))
            .ok_or_else(|| anyhow::anyhow!("No integration target is recorded for train {}", train.id))?;
    }
    let intent = if let Some(intent) = pending {
        intent
    } else {
        let Some(train) = parent.as_ref() else { return Ok(None) };
        let intent = DetachIntent {
            verdict: verdict.to_owned(), train_id: train.id.clone(),
            owner: train.epic_verification_owner.clone().or_else(|| owner.map(str::to_owned)),
            integrated: current.deliverables.green_merge_commit.is_some(),
            requires_target_merge: replacement.as_ref().is_some_and(|target| {
                current.deliverables.factory_branch_start.as_ref().is_some_and(|start| {
                    !super::repo_context::resolve_repo_context(root, target).ok().is_some_and(|context| {
                        release_train::branch_tip(&context.repo_root, &target.target_branch).is_some_and(|tip|
                            release_train::is_ancestor(&context.repo_root, &start.base_sha, &tip))
                    })
                })
            }),
        };
        crate::store::open_event_store(root)?.record(
            &Event::new(EventType::TaskNoteAdded, EventEntityType::Task, task_id, "Release train detachment notice pending")
                .with_metadata(serde_json::json!({"repeat_review_detach": intent})),
        )?;
        intent
    };
    let Some(train) = parent.filter(|train| train.id == intent.train_id) else { return Ok(Some(intent)) };
    if let Some(target) = replacement {
        let mut detached = current.clone();
        detached.deliverables.work_target_pinned = true;
        detached.deliverables.work_target = Some(target.clone());
        if let Some(mut start) = current.deliverables.factory_branch_start.clone()
            && current.deliverables.work_target.as_ref() == Some(&start.work_target)
        {
            start.work_target = target;
            super::rebind::rebind_start(&mut detached, start, "cassy-repeat-review", "detached from release train", super::rebind::StartAudit::Note);
        }
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
    root: &Path, task_id: &str, verdict: &str, count: u64, detached: Option<&DetachIntent>, owner: Option<&str>,
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
    let guidance = repeat_review_refusal(task_id, Some(verdict), Some(count)).message;
    let detail = if let Some(intent) = detached {
        let merge = if intent.requires_target_merge {
            " The assigned worker must merge the integration branch into the source branch before tests or CI, then re-park."
        } else { "" };
        format!("{task_id} detached from release train {} after repeated verifier rejections.{integrated}{merge} {guidance}", intent.train_id)
    } else {
        format!("{task_id} held after repeated verifier rejections. {guidance}")
    };
    crate::supervisor_notices::enqueue_keyed(
        root,
        &recipient,
        if detached.is_some() { "repeat-review-train-detached" } else { "premerge_verification" },
        &detail,
        &transition_key(task_id, verdict),
        Some(serde_json::json!({"task_id":task_id,"train_id":detached.map(|intent| &intent.train_id),"verification_id":verdict,"rejection_action":"hold_escalate", "requires_target_merge":detached.is_some_and(|intent| intent.requires_target_merge)})),
    )?;
    Ok(detail)
}
