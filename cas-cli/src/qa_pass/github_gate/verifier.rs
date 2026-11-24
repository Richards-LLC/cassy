use std::{collections::HashSet, path::Path};

use cas_types::{
    AgentRole, AgentStatus, EventEntityType, EventType, VerificationDispatchState,
    VerificationStatus, WorkTarget,
};
use rusqlite::{Connection, OptionalExtension, params};

use super::{PrMetadata, origin_slug, repo_slug};
use crate::mcp::tools::core::{
    task::repo_context::resolve_repo_context,
    workflow::premerge_verification::original_candidate_for_dispatch,
};

pub(super) fn merge_refusal(
    cas_root: &Path,
    pr: &PrMetadata,
    supervisor: Option<&str>,
) -> Option<String> {
    match check_pr(cas_root, pr, supervisor) {
        Ok(refusal) => refusal,
        Err(error) => {
            tracing::warn!(%error, "Raw GitHub merge: verifier state unreadable");
            None
        }
    }
}

fn dispatch_rows(conn: &Connection) -> rusqlite::Result<rusqlite::Statement<'_>> {
    conn.prepare(
        "SELECT d.id, d.task_id, json_extract(task.deliverables, '$.delivery_pr_number'),
                r.source_branch, r.repo_selector, r.target_branch
         FROM verification_dispatches d
         JOIN tasks task ON task.id=d.task_id
         LEFT JOIN worker_delivery_transactions t
            ON t.id=d.delivery_transaction_id AND t.receipt_id=d.receipt_id AND t.task_id=d.task_id
         LEFT JOIN worker_completion_receipts r ON r.id=t.receipt_id AND r.task_id=d.task_id
         WHERE r.commit_sha=?1 OR (d.delivery_transaction_id IS NULL AND d.repository_proof IS NOT NULL)
         ORDER BY d.requested_at DESC, d.id DESC",
    )
}

// gate-audit[audited_override]: bypass H-24 A registered supervisor may override a rejected dispatch only after the reason is recorded.
fn check_pr(
    cas_root: &Path,
    pr: &PrMetadata,
    supervisor: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let (Some(repo), Some(number), Some(branch), Some(head)) =
        (pr.repository(), pr.number, pr.branch(), pr.head())
    else {
        tracing::warn!("Raw GitHub merge: verifier PR metadata incomplete");
        return Ok(None);
    };
    let conn = Connection::open_with_flags(
        cas_root.join("cas.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut statement = dispatch_rows(&conn)?;
    let mut rows = statement.query([head])?;
    let mut decided = HashSet::new();
    while let Some(row) = rows.next()? {
        let dispatch_id: String = row.get(0)?;
        let task_id: String = row.get(1)?;
        if decided.contains(&task_id) {
            continue;
        }
        let task_pr = row.get::<_, Option<i64>>(2)?.and_then(|value| u64::try_from(value).ok());
        let source_branch: Option<String> = row.get(3)?;
        let dispatch = cas_store::get_verification_dispatch_with_conn(&conn, &dispatch_id)?;
        if let Some(source_branch) = source_branch {
            if task_pr != Some(number) && source_branch != branch {
                continue;
            }
            let target = WorkTarget { repo_selector: row.get(4)?, target_branch: row.get(5)? };
            if receipt_repo(cas_root, &target).as_deref() != Some(repo.as_str()) {
                continue;
            }
        } else {
            if !dispatch.repository.as_ref().is_some_and(|proof| {
                origin_slug(Path::new(&proof.repository_root)).as_deref() == Some(repo.as_str())
            }) {
                continue;
            }
            match original_candidate_for_dispatch(cas_root, &dispatch) {
                Ok(Some(candidate)) if candidate.source_sha == head &&
                    (task_pr == Some(number) || candidate.source_branch == branch) => {}
                Ok(_) => continue,
                Err(error) => {
                    tracing::warn!(%task_id, %dispatch_id, %error, "Raw GitHub merge: verifier candidate unreadable");
                    continue;
                }
            }
        }
        let verdict = match dispatch.state {
            VerificationDispatchState::Pending
            | VerificationDispatchState::Claimed
            | VerificationDispatchState::TimedOut => VerificationStatus::Pending,
            VerificationDispatchState::Resolved | VerificationDispatchState::Invalidated => {
                let verdict: Option<(String, Option<String>)> = conn
                    .query_row(
                        "SELECT status,status_detail FROM verifications
                     WHERE dispatch_id=?1 AND task_id=?2 AND verification_type='task'
                     ORDER BY created_at DESC,id DESC LIMIT 1",
                        params![dispatch_id, task_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                let Some((status, detail)) = verdict else {
                    if dispatch.state == VerificationDispatchState::Resolved {
                        tracing::warn!(%task_id, %dispatch_id, "Raw GitHub merge: resolved dispatch has no verdict");
                    }
                    continue;
                };
                let verdict = VerificationStatus::from_storage(&status, detail.as_deref());
                if dispatch.state == VerificationDispatchState::Invalidated && verdict != VerificationStatus::Rejected {
                    continue;
                }
                verdict
            }
        };
        decided.insert(task_id.clone());
        let (code, state, hint) = match verdict {
            VerificationStatus::Rejected => {
                if supervisor.is_some_and(|actor| audited_override(cas_root, &task_id, actor, &dispatch_id)) {
                    continue;
                }
                ("PR_VERIFIER_REJECTED", "rejected", "Resolve the findings, or have a supervisor record a decision note: Raw GitHub merge override <dispatch>: <reason>.")
            }
            VerificationStatus::Pending | VerificationStatus::SupervisorCall => {
                ("PR_VERIFIER_PENDING", "pending", "Wait for the verifier's verdict; this hold cannot be overridden.")
            }
            _ => continue,
        };
        return Ok(Some(format!("{code}: verifier verdict {state} for {task_id} at {head} (dispatch {dispatch_id}). {hint}")));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmapped_head_reads_no_dispatch_rows_cas_eb2e() {
        let repo = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(repo.path()).unwrap();
        let tasks = crate::store::open_task_store(&root).unwrap();
        let conn = Connection::open(root.join("cas.db")).unwrap();
        for index in 0..12 {
            let task_id = format!("cas-unrelated-{index}");
            tasks.add(&cas_types::Task::new(task_id.clone(), "Unrelated review".into())).unwrap();
            conn.execute(
                "INSERT INTO verification_dispatches (id, task_id, requester_agent_id, owner_agent_id,
                    requested_at, deadline_at, repository_proof) VALUES (?1, ?1, 'fixture', 'fixture',
                    '2026-11-24T00:00:00Z', '2026-11-24T00:10:00Z', ?2)",
                params![task_id, serde_json::json!({"repository_root": repo.path(), "head_commit": "a".repeat(40)}).to_string()],
            ).unwrap();
        }
        let mut statement = dispatch_rows(&conn).unwrap();
        let mut rows = statement.query(["b".repeat(40)]).unwrap();
        let mut count = 0;
        while rows.next().unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 0);
    }
}

fn receipt_repo(cas_root: &Path, target: &WorkTarget) -> Option<String> {
    let selector = &target.repo_selector;
    if !selector.starts_with("project:")
        && let Some(repo) = repo_slug(selector.strip_prefix("remote:").unwrap_or(selector))
    {
        return Some(repo);
    }
    match resolve_repo_context(cas_root, target) {
        Ok(context) => origin_slug(&context.repo_root),
        Err(error) => {
            tracing::warn!(%error, "Raw GitHub merge: verifier receipt repository unreadable");
            None
        }
    }
}

fn audited_override(cas_root: &Path, task_id: &str, actor: &str, dispatch: &str) -> bool {
    let Ok(agent) =
        crate::store::open_agent_store(cas_root).and_then(|store| Ok(store.get(actor)?))
    else {
        return false;
    };
    if agent.role != AgentRole::Supervisor
        || !matches!(agent.status, AgentStatus::Active | AgentStatus::Idle)
    {
        return false;
    }
    let Ok(events) = crate::store::open_event_store(cas_root) else {
        return false;
    };
    let Ok(notes) = events.list_for_entity(EventEntityType::Task, task_id, 10_000) else {
        return false;
    };
    let marker = format!("Raw GitHub merge override {dispatch}:");
    let reason = notes.iter().find_map(|note| {
        if note.event_type != EventType::TaskNoteAdded || note.session_id.as_deref() != Some(actor)
        {
            return None;
        }
        let reason = note
            .metadata
            .as_ref()?
            .get("supervisor_decision")?
            .as_str()?
            .strip_prefix(&marker)?
            .trim();
        (!reason.is_empty()).then_some(reason)
    });
    let Some(reason) = reason else { return false };
    match crate::gates::facts::record_override_code(
        events.as_ref(),
        "H-24",
        task_id,
        &format!("{dispatch}: {reason}"),
    ) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%task_id, %error, "Raw GitHub merge: verifier override could not be logged");
            false
        }
    }
}
