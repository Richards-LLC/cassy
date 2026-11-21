//! Owner-issued delegation for one exact peer verifier question and answer.
//! Synced question notes are public context, never authority. Only authenticated
//! owner mail admits an answer, and the existing verification transaction records it.
use super::*;
use crate::mcp::tools::VerificationAddRequest;
use cas_types::{
    Agent, Task, Verification, VerificationDispatch, VerificationProvenance, VerificationStatus,
};
use sha2::{Digest, Sha256};

const NOTE: &str = "PEER_VERIFICATION_QUESTION: ";
const TAG: &str = "CAS_PEER_VERIFICATION_ANSWER_V1 ";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Question {
    project: String,
    train: String,
    owner: String,
    generation: String,
    task: String,
    dispatch: String,
    boundary: String,
    question: String,
    summary: String,
    verifier: String,
    responder: String,
    machine: String,
    session: String,
    deadline: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Answer {
    question: Question,
    summary: String,
}

fn boundary(task: &Task, dispatch: &VerificationDispatch) -> Result<String, String> {
    Ok(hex::encode(Sha256::digest(
        serde_json::to_vec(&(
            task.deliverables.delivery_cycle,
            &dispatch.receipt_id,
            &dispatch.delivery_transaction_id,
            &dispatch.repository,
            &dispatch.requested_at,
            &dispatch.requester_agent_id,
            &dispatch.owner_agent_id,
        ))
        .map_err(|e| e.to_string())?,
    )))
}

fn scope(root: &Path, task: &Task, question: &Question) -> Result<bool, String> {
    let Some(train) = train_parks::owner_train(root, task)? else {
        return Ok(false);
    };
    Ok(!task.is_terminal()
        && !train.is_terminal()
        && question.task == task.id
        && question.train == train.id
        && crate::cloud::resolve_canonical_id(root).as_deref() == Some(&question.project)
        && train.epic_verification_owner.as_deref() == Some(&question.owner)
        && crate::factory_session_scope::train_owner_generation(&train)? == question.generation)
}

pub(crate) fn publish(
    root: &Path,
    task: &Task,
    dispatch: &VerificationDispatch,
    call: &Verification,
) -> Result<Option<String>, String> {
    if call.status != VerificationStatus::SupervisorCall
        || call.provenance != VerificationProvenance::TaskVerifier
    {
        return Ok(None);
    }
    let Some(train) = train_parks::owner_train(root, task)? else {
        return Ok(None);
    };
    let agents = crate::store::open_agent_store(root).map_err(|e| e.to_string())?;
    // The dispatch's registered supervising session is the only delegate.
    let Ok(responder) = agents.get(&dispatch.owner_agent_id) else {
        return Ok(None);
    };
    if responder.role != AgentRole::Supervisor
        || !responder.is_alive()
        || owns_recipient(
            root,
            &responder,
            train.epic_verification_owner.as_deref().unwrap(),
        )
    {
        return Ok(None);
    }
    let (Some(machine), Some(session), Some(project), Some(verifier)) = (
        responder.machine_id,
        responder.factory_session,
        crate::cloud::resolve_canonical_id(root),
        dispatch.verifier_agent_id.clone(),
    ) else {
        return Ok(None);
    };
    if call.agent_id.as_deref() != Some(&verifier) {
        return Ok(None);
    }
    let question = Question {
        project,
        train: train.id.clone(),
        owner: train.epic_verification_owner.clone().unwrap(),
        generation: crate::factory_session_scope::train_owner_generation(&train)?,
        task: task.id.clone(),
        dispatch: dispatch.id.clone(),
        boundary: boundary(task, dispatch)?,
        question: call.id.clone(),
        summary: call.summary.clone(),
        verifier,
        responder: responder.id,
        machine,
        session,
        deadline: dispatch.deadline_at,
    };
    let note = format!(
        "{NOTE}{}",
        serde_json::to_string(&question).map_err(|e| e.to_string())?
    );
    let tasks = crate::store::open_task_store(root).map_err(|e| e.to_string())?;
    if !tasks
        .get(&task.id)
        .map_err(|e| e.to_string())?
        .notes
        .lines()
        .any(|line| line == note)
    {
        tasks
            .append_note(&task.id, &note)
            .map_err(|e| e.to_string())?;
    }
    Ok(Some(format!(
        "Train owner {} can inspect the public question in task notes and authorize its answer with verification action=add task_id={} dispatch_id={} status=supervisor_call summary=\"<exact answer>\". Authenticated owner mail delegates only that answer to supervisor {} on the builder; the independent verifier still owes its final verdict.",
        question.owner, task.id, dispatch.id, question.responder
    )))
}

fn answer_only(req: &VerificationAddRequest) -> bool {
    req.status.eq_ignore_ascii_case("supervisor_call")
        && req.verifier_capability.is_none()
        && req.confidence.is_none()
        && req.issues.is_none()
        && req.files_reviewed.is_none()
        && req.duration_ms.is_none()
        && req.verification_type.is_none()
}

/// The owner has no local dispatch. Queue authority, never fabricate a verdict.
pub(crate) fn queue_answer(
    root: &Path,
    task: &Task,
    actor: &Agent,
    req: &VerificationAddRequest,
    summary: &str,
) -> Result<Option<String>, String> {
    let Some(id) = req.dispatch_id.as_deref() else {
        return Ok(None);
    };
    if cas_store::get_verification_dispatch(root, id).is_ok() {
        return Ok(None);
    }
    let Some(question) = task
        .notes
        .lines()
        .rev()
        .filter_map(|line| line.strip_prefix(NOTE))
        .filter_map(|line| serde_json::from_str::<Question>(line).ok())
        .find(|q| q.dispatch == id && q.task == task.id)
    else {
        return Ok(None);
    };
    if !scope(root, task, &question)?
        || actor.id != question.owner
        || question.deadline <= chrono::Utc::now()
        || !answer_only(req)
    {
        return Err("Peer question requires its current train owner and an unexpired exact dispatch. Send only task_id, dispatch_id, status=supervisor_call and summary; final verdicts remain with the independent verifier.".into());
    }
    let answer = Answer {
        question,
        summary: summary.into(),
    };
    let encoded = serde_json::to_string(&answer).map_err(|e| e.to_string())?;
    let call = serde_json::json!({"task_id":task.id, "dispatch_id":id, "status":"supervisor_call", "summary":summary});
    let message = PeerSend {
        project_id: answer.question.project.clone(),
        sender_agent_id: actor.id.clone(),
        recipient_agent_id: answer.question.responder.clone(),
        body: attach_message_intent(
            &format!(
                "{TAG}{encoded}\n\nRecord the owner's exact answer through verification action=add with {call}. Then let the independent verifier continue on this dispatch."
            ),
            Some("answer"),
            false,
        )?,
        summary: Some(format!("Owner answer: {}", task.id)),
        in_reply_to: None,
        dedupe_key: format!(
            "verification-answer:{}",
            hex::encode(Sha256::digest(encoded.as_bytes()))
        ),
        scope: PeerScope::Project,
        recipient_project_id: None,
    };
    let row = outbox::enqueue_notice(root, message, &answer.question.responder)?;
    Ok(Some(format!(
        "Owner answer authorized and queued as peer-outbox:{row} for {}. The builder must record this exact answer; no verdict or task transition was recorded here.",
        answer.question.responder
    )))
}

fn answer_path(root: &Path, dispatch: &str) -> std::path::PathBuf {
    root.join("peer-verification-answers").join(format!(
        "{}.json",
        hex::encode(Sha256::digest(dispatch.as_bytes()))
    ))
}

fn current(root: &Path, task: &Task, actor: &Agent, answer: &Answer) -> Result<bool, String> {
    let q = &answer.question;
    if actor.role != AgentRole::Supervisor
        || actor.id != q.responder
        || actor.machine_id.as_deref() != Some(&q.machine)
        || actor.factory_session.as_deref() != Some(&q.session)
        || !scope(root, task, q)?
    {
        return Ok(false);
    }
    let dispatch =
        cas_store::get_verification_dispatch(root, &q.dispatch).map_err(|e| e.to_string())?;
    if dispatch.task_id != task.id
        || boundary(task, &dispatch)? != q.boundary
        || cas_store::get_latest_verification_dispatch(root, &task.id)
            .map_err(|e| e.to_string())?
            .is_none_or(|d| d.id != q.dispatch)
    {
        return Ok(false);
    }
    let Some(latest) =
        cas_store::get_verification_for_dispatch(root, &q.dispatch).map_err(|e| e.to_string())?
    else {
        return Ok(false);
    };
    if latest.status != VerificationStatus::SupervisorCall {
        return Ok(false);
    }
    if latest.provenance == VerificationProvenance::SupervisorDirect {
        // Lost-response retries authorize no new content or boundary.
        return Ok(
            latest.agent_id.as_deref() == Some(&actor.id) && latest.summary == answer.summary
        );
    }
    Ok(
        dispatch.state == cas_types::VerificationDispatchState::Claimed
            && dispatch.deadline_at > chrono::Utc::now()
            && q.deadline == dispatch.deadline_at
            && latest.id == q.question
            && latest.summary == q.summary
            && latest.provenance == VerificationProvenance::TaskVerifier
            && latest.agent_id.as_deref() == Some(&q.verifier)
            && dispatch.verifier_agent_id.as_deref() == Some(&q.verifier),
    )
}

pub(crate) fn authorized(
    root: &Path,
    task: &Task,
    actor: &Agent,
    req: &VerificationAddRequest,
    summary: &str,
) -> Result<bool, String> {
    if !answer_only(req) {
        return Ok(false);
    }
    let Some(id) = req.dispatch_id.as_deref() else {
        return Ok(false);
    };
    let bytes = match std::fs::read(answer_path(root, id)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    let answer: Answer = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    Ok(answer.question.dispatch == id
        && answer.summary == summary
        && current(root, task, actor, &answer)?)
}

pub(crate) fn ingest(
    root: &Path,
    recipient: &Agent,
    message: &ClaimedPeerMessage,
) -> Result<Option<PeerAckOutcome>, String> {
    let (_, body) = split_message_intent(&message.body);
    let Some(encoded) = body.strip_prefix(TAG).and_then(|body| body.lines().next()) else {
        return Ok(None);
    };
    let reject = Ok(Some(PeerAckOutcome::Rejected));
    let Ok(answer) = serde_json::from_str::<Answer>(encoded) else {
        return reject;
    };
    let q = &answer.question;
    // The Cloud-stamped sender is the pinned owner, never a name in prose.
    if message.scope != PeerScope::Project
        || message.project_id != q.project
        || message.sender_agent_id != q.owner
        || message.recipient_agent_id != q.responder
        || recipient.id != q.responder
        || answer.summary.trim().is_empty()
    {
        return reject;
    }
    let task = crate::store::open_task_store_local(root)
        .map_err(|e| e.to_string())?
        .get(&q.task)
        .map_err(|e| e.to_string())?;
    if !current(root, &task, recipient, &answer)? {
        return reject;
    }
    let path = answer_path(root, &q.dispatch);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let temporary = path.with_extension(format!("tmp.{}", uuid::Uuid::new_v4()));
    std::fs::write(
        &temporary,
        serde_json::to_vec(&answer).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(temporary, path).map_err(|e| e.to_string())?;
    // Normal mailbox admission wakes the exact delegate with the tool payload.
    Ok(None)
}
