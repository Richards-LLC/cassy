use std::path::Path;
use std::process::Command;

use cas_types::{
    AgentRole, AgentStatus, EventEntityType, EventType, Task, VerificationDispatchState,
    VerificationStatus,
};
use rusqlite::{Connection, OptionalExtension, params};

use super::{
    GH_LOOKUP_TIMEOUT, GithubMerge, MergeSelector, PrHead, gh_bin, gh_pr_head, origin_slug,
    repo_slug, run_bounded_detailed, task_repo,
};
use crate::mcp::tools::core::workflow::premerge_verification::candidate_for_dispatch;

pub(super) fn merge_refusal(
    cas_root: &Path,
    cwd: &Path,
    merges: &[GithubMerge],
    tasks: &[Task],
    supervisor: Option<&str>,
) -> Option<String> {
    match check_merges(cas_root, cwd, merges, tasks, supervisor) {
        Ok(refusal) => refusal,
        Err(error) => {
            tracing::warn!(%error, "Raw GitHub merge: verifier state unreadable");
            None
        }
    }
}

// gate-audit[audited_override]: bypass H-24 A registered supervisor may override a rejected dispatch only after the reason is recorded.
fn check_merges(
    cas_root: &Path,
    cwd: &Path,
    merges: &[GithubMerge],
    tasks: &[Task],
    supervisor: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let conn = Connection::open_with_flags(
        cas_root.join("cas.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut statement = conn.prepare(
        "SELECT DISTINCT task_id FROM verification_dispatches
         WHERE repository_proof IS NOT NULL OR delivery_transaction_id IS NOT NULL",
    )?;
    let dispatched: std::collections::HashSet<String> = statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    let tasks: Vec<_> = tasks
        .iter()
        .filter(|task| dispatched.contains(&task.id))
        .collect();
    if tasks.is_empty() {
        return Ok(None);
    }

    for merge in merges {
        let (pr, number, repo) = match resolve_pr(cwd, merge) {
            Ok(pr) => pr,
            Err(error) => {
                tracing::warn!(%error, "Raw GitHub merge: verifier could not resolve PR head");
                continue;
            }
        };
        for task in &tasks {
            if number != task.deliverables.delivery_pr_number || number.is_none() {
                let implementer = super::super::delivery_implementer(cas_root, task);
                if super::super::delivery_branch(task, &[], &implementer, |name| name == pr.branch)
                    != pr.branch
                {
                    continue;
                }
            }
            if let (Some(target), Some(task_origin)) =
                (&repo, origin_slug(&task_repo(cas_root, task)))
                && target != &task_origin
            {
                continue;
            }
            let (dispatch, code, verdict) = match verifier_hold(&conn, cas_root, task, &pr.sha) {
                Ok(Some(hold)) => hold,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(task_id = %task.id, %error, "Raw GitHub merge: verifier state unreadable");
                    continue;
                }
            };
            if verdict == "rejected"
                && supervisor
                    .is_some_and(|actor| audited_override(cas_root, task, actor, &dispatch))
            {
                continue;
            }
            return Ok(Some(format!(
                "{code}: verifier verdict {verdict} for {} at {} (dispatch {dispatch}). {}",
                task.id,
                pr.sha,
                if verdict == "rejected" {
                    "Resolve the findings, or have a supervisor record a decision note: Raw GitHub merge override <dispatch>: <reason>."
                } else {
                    "Wait for the verifier's verdict; this hold cannot be overridden."
                },
            )));
        }
    }
    Ok(None)
}

fn verifier_hold(
    conn: &Connection,
    cas_root: &Path,
    task: &Task,
    sha: &str,
) -> anyhow::Result<Option<(String, &'static str, &'static str)>> {
    let mut statement = conn.prepare(
        "SELECT d.id, r.commit_sha FROM verification_dispatches d
         LEFT JOIN worker_delivery_transactions t
            ON t.id=d.delivery_transaction_id AND t.receipt_id=d.receipt_id AND t.task_id=d.task_id
         LEFT JOIN worker_completion_receipts r ON r.id=t.receipt_id AND r.task_id=d.task_id
         WHERE d.task_id=?1 AND (d.repository_proof IS NOT NULL OR r.commit_sha IS NOT NULL)
         ORDER BY d.requested_at DESC, d.id DESC",
    )?;
    let mut rows = statement.query([&task.id])?;
    while let Some(row) = rows.next()? {
        let dispatch_id: String = row.get(0)?;
        let receipt_head: Option<String> = row.get(1)?;
        let dispatch = cas_store::get_verification_dispatch_with_conn(conn, &dispatch_id)?;
        let matches_head = if let Some(receipt_head) = receipt_head {
            receipt_head == sha
        } else if dispatch
            .repository
            .as_ref()
            .is_some_and(|proof| proof.head_commit == sha)
        {
            true
        } else {
            candidate_for_dispatch(cas_root, &dispatch)
                .map_err(anyhow::Error::msg)?
                .is_some_and(|candidate| candidate.source_sha == sha)
        };
        if !matches_head {
            continue;
        }
        let verdict = match dispatch.state {
            VerificationDispatchState::Pending
            | VerificationDispatchState::Claimed
            | VerificationDispatchState::TimedOut => VerificationStatus::Pending,
            VerificationDispatchState::Resolved => {
                let verdict: Option<(String, Option<String>)> = conn
                    .query_row(
                        "SELECT status,status_detail FROM verifications
                     WHERE dispatch_id=?1 AND task_id=?2 AND verification_type='task'
                     ORDER BY created_at DESC,id DESC LIMIT 1",
                        params![dispatch.id, task.id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                let (status, detail) = verdict
                    .ok_or_else(|| anyhow::anyhow!("dispatch {} has no verdict", dispatch.id))?;
                VerificationStatus::from_storage(&status, detail.as_deref())
            }
            VerificationDispatchState::Invalidated => return Ok(None),
        };
        return Ok(match verdict {
            VerificationStatus::Rejected => Some((dispatch.id, "PR_VERIFIER_REJECTED", "rejected")),
            VerificationStatus::Pending | VerificationStatus::SupervisorCall => {
                Some((dispatch.id, "PR_VERIFIER_PENDING", "pending"))
            }
            _ => None,
        });
    }
    Ok(None)
}

fn audited_override(cas_root: &Path, task: &Task, actor: &str, dispatch: &str) -> bool {
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
    let Ok(notes) = events.list_for_entity(EventEntityType::Task, &task.id, 10_000) else {
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
        &task.id,
        &format!("{dispatch}: {reason}"),
    ) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(task_id = %task.id, %error, "Raw GitHub merge: verifier override could not be logged");
            false
        }
    }
}

fn resolve_pr(
    cwd: &Path,
    merge: &GithubMerge,
) -> Result<(PrHead, Option<u64>, Option<String>), String> {
    let repo = merge.repo.as_deref();
    let (selector, number) = match &merge.selector {
        MergeSelector::Number(number) => (Some(number.to_string()), Some(*number)),
        MergeSelector::Branch(branch) => (Some(branch.clone()), None),
        MergeSelector::CurrentBranch => (None, None),
        MergeSelector::Unresolvable(why) => {
            let statement = merge.statement.replace(r#"\""#, "\"");
            let id = statement
                .split_once("pullRequestId")
                .and_then(|(_, rest)| {
                    let value = rest.trim_start().strip_prefix(':')?.trim_start();
                    value.strip_prefix('"')?.split_once('"').map(|(id, _)| id)
                })
                .filter(|id| !id.is_empty())
                .ok_or_else(|| why.to_string())?;
            let mut command = Command::new(gh_bin());
            command.args([
                "api", "graphql", "-f", "query=query($id: ID!) { node(id: $id) { ... on PullRequest { number headRefName headRefOid repository { nameWithOwner } } } }", "-f",
            ]).arg(format!("id={id}")).current_dir(cwd);
            let bytes = run_bounded_detailed(command, GH_LOOKUP_TIMEOUT)?;
            let json: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            let node = &json["data"]["node"];
            let field = |key: &str| {
                node[key]
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            };
            let (Some(branch), Some(sha), Some(number), Some(repo)) = (
                field("headRefName"),
                field("headRefOid"),
                node["number"].as_u64(),
                node["repository"]["nameWithOwner"]
                    .as_str()
                    .and_then(repo_slug),
            ) else {
                return Err("GitHub did not resolve the pull request node".into());
            };
            return Ok((PrHead { branch, sha }, Some(number), Some(repo)));
        }
    };
    let pr = gh_pr_head(cwd, repo, selector.as_deref())?;
    Ok((
        pr,
        number,
        repo.and_then(repo_slug).or_else(|| origin_slug(cwd)),
    ))
}
