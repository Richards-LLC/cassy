use std::{
    collections::{HashMap, HashSet},
    path::Path,
    time::Duration,
};

use cas_types::{
    AgentRole, AgentStatus, EventEntityType, EventType, VerificationDispatchState,
    VerificationStatus, WorkTarget,
};
use rusqlite::{Connection, OptionalExtension, params};

use super::{PrMetadata, origin_identity, repo_identity, same_repository};
use crate::mcp::tools::core::{
    task::repo_context::{BoundedRepoProbe, git_layout, resolve_repo_context},
    workflow::premerge_verification::{
        CANDIDATE_COMMIT_PREFIX, Candidate, original_candidate_at_ref,
    },
};
use crate::store::{KnownRepoStore, known_repos::open_host_known_repo_store};

pub(super) fn merge_refusal(
    cas_root: &Path,
    cwd: &Path,
    pr: &PrMetadata,
    supervisor: Option<&str>,
) -> Option<String> {
    match check_pr(cas_root, cwd, pr, supervisor) {
        Ok(refusal) => refusal,
        Err(error) => {
            tracing::warn!(%error, "Raw GitHub merge: verifier state unreadable");
            None
        }
    }
}

enum Binding<'a> {
    Receipt(WorkTarget),
    Candidate(&'a Candidate),
}

struct DispatchRow<'a> {
    id: String,
    task_id: String,
    requested_at: String,
    pr_number: Option<u64>,
    source_branch: String,
    binding: Binding<'a>,
}

fn dispatch_rows<'a>(conn: &Connection, head: &str, candidates: &'a [Candidate]) -> rusqlite::Result<Vec<DispatchRow<'a>>> {
    let mut receipt_query = conn.prepare(
        "SELECT d.id, d.task_id, d.requested_at, json_extract(task.deliverables, '$.delivery_pr_number'),
                r.source_branch, r.repo_selector, r.target_branch
         FROM worker_completion_receipts r
         CROSS JOIN worker_delivery_transactions t ON t.receipt_id=r.id AND t.task_id=r.task_id
         CROSS JOIN verification_dispatches d
            ON d.delivery_transaction_id=t.id AND d.receipt_id=r.id AND d.task_id=r.task_id
         JOIN tasks task ON task.id=d.task_id
         WHERE r.commit_sha=?1",
    )?;
    let pr_number = |row: &rusqlite::Row<'_>| -> rusqlite::Result<Option<u64>> {
        Ok(row.get::<_, Option<i64>>(3)?.and_then(|number| u64::try_from(number).ok()))
    };
    let mut rows = receipt_query.query_map([head], |row| Ok(DispatchRow {
        id: row.get(0)?,
        task_id: row.get(1)?,
        requested_at: row.get(2)?,
        pr_number: pr_number(row)?,
        source_branch: row.get(4)?,
        binding: Binding::Receipt(WorkTarget { repo_selector: row.get(5)?, target_branch: row.get(6)? }),
    }))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if !candidates.is_empty() {
        let mut candidate_query = conn.prepare(
            "SELECT d.id, d.task_id, d.requested_at, json_extract(task.deliverables, '$.delivery_pr_number')
             FROM verification_dispatches d JOIN tasks task ON task.id=d.task_id
             WHERE d.task_id=?1 AND d.delivery_transaction_id IS NULL
                AND json_extract(d.repository_proof, '$.head_commit')=?2",
        )?;
        for candidate in candidates {
            for row in candidate_query.query_map(params![candidate.task_id, candidate.proof.head_commit], |row| Ok(DispatchRow {
                id: row.get(0)?,
                task_id: row.get(1)?,
                requested_at: row.get(2)?,
                pr_number: pr_number(row)?,
                source_branch: candidate.source_branch.clone(),
                binding: Binding::Candidate(candidate),
            }))? {
                rows.push(row?);
            }
        }
    }
    rows.sort_unstable_by(|first, second| {
        (&second.requested_at, &second.id).cmp(&(&first.requested_at, &first.id))
    });
    Ok(rows)
}

fn source_candidates(cas_root: &Path, cwd: &Path, head: &str, repository: &str) -> Vec<Candidate> {
    if !cas_root.join("verification-candidates").is_dir() {
        return Vec::new();
    }
    let mut directories: HashSet<_> = [cas_root, cwd].into_iter()
        .filter_map(|path| git_layout(path).ok().map(|(root, _)| root)).collect();
    if let Ok(registry) = open_host_known_repo_store() {
        if let Ok(repos) = registry.list() {
            directories.extend(repos.into_iter().map(|repo| repo.path));
        }
        if let Ok(bindings) = registry.list_bindings() {
            directories.extend(bindings.into_iter().map(|binding| binding.repo_root));
        }
    }
    let probe = BoundedRepoProbe::new(
        crate::bounded_process::Deadline::after(Duration::from_secs(4)),
        Duration::from_secs(1),
        directories.len(),
    );
    let mut candidates = Vec::new();
    for directory in directories {
        let Ok(refs) = probe.output(&directory, &[
            "for-each-ref", "--format=%(objectname)%09%(parent)%09%(tree)%09%(refname:strip=3)%09%(subject)",
            "refs/cas/verification-candidates/",
        ]) else { continue };
        let mut matching = refs.lines().filter_map(|line| {
            let mut fields = line.splitn(5, '\t');
            let (commit, parents, tree, key, subject) = (fields.next()?, fields.next()?, fields.next()?, fields.next()?, fields.next()?);
            (parents.split_whitespace().nth(1) == Some(head)
                && subject.strip_prefix(CANDIDATE_COMMIT_PREFIX) == Some(key))
                .then_some((commit, tree, key))
        }).peekable();
        if matching.peek().is_none() || !origin_identity(&directory)
            .is_some_and(|identity| same_repository(&identity, repository, &directory)) {
            continue;
        }
        for (commit, tree, key) in matching {
            match original_candidate_at_ref(cas_root, &directory, key, commit, tree) {
                Ok(candidate) if candidate.source_sha == head => candidates.push(candidate),
                Ok(_) => {},
                Err(error) => tracing::warn!(%commit, %error, "Raw GitHub merge: verifier candidate unreadable"),
            }
        }
    }
    candidates
}

// gate-audit[audited_override]: bypass H-24 A registered supervisor may override a rejected dispatch only after the reason is recorded.
fn check_pr(
    cas_root: &Path,
    cwd: &Path,
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
    let candidates = source_candidates(cas_root, cwd, head, &repo);
    let mut decided = HashSet::new();
    let mut receipt_repos = HashMap::new();
    for row in dispatch_rows(&conn, head, &candidates)? {
        let DispatchRow { id: dispatch_id, task_id, pr_number, source_branch, binding, .. } = row;
        if decided.contains(&task_id) || (pr_number != Some(number) && source_branch != branch) {
            continue;
        }
        if let Binding::Receipt(target) = &binding {
            let matched = receipt_repos.entry((target.repo_selector.clone(), target.target_branch.clone()))
                .or_insert_with(|| receipt_repo(cas_root, target)
                    .is_some_and(|identity| same_repository(&identity, &repo, cwd)));
            if !*matched {
                continue;
            }
        }
        let dispatch = cas_store::get_verification_dispatch_with_conn(&conn, &dispatch_id)?;
        if let Binding::Candidate(candidate) = binding
            && dispatch.repository.as_ref() != Some(&candidate.proof) {
            continue;
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
        let head = "b".repeat(40);
        let candidates = source_candidates(&root, repo.path(), &head, "github.com/acme/project");
        assert_eq!(dispatch_rows(&conn, &head, &candidates).unwrap().len(), 0);
    }
}

fn receipt_repo(cas_root: &Path, target: &WorkTarget) -> Option<String> {
    let selector = &target.repo_selector;
    if !selector.starts_with("project:")
        && let Some(repo) = repo_identity(selector.strip_prefix("remote:").unwrap_or(selector))
    {
        return Some(repo);
    }
    match resolve_repo_context(cas_root, target) {
        Ok(context) => origin_identity(&context.repo_root),
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
