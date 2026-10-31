//! Validation shared by batch recording and close. No process or polling work.
use crate::mcp::tools::core::task::lifecycle::close_ops::{
    TaskCommitReceiptWindow, squash_receipt_qa_anchor,
};
use cas_store::QaBatchReceipt;
use cas_types::Task;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn commit(repo: &Path, rev: &str) -> Option<String> {
    let out = Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{rev}^{{commit}}"),
        ])
        .current_dir(repo)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Recovery retires the active merge authority, but retains delivery identity.
/// QA may attest that same immutable delivery after an evidence-only restart.
/// Prefer an active anchor; otherwise use only the latest recorded history.
/// An unresolvable newer anchor must never resurrect an older reviewed tip.
pub(crate) fn delivery_anchor(repo: &Path, task: &Task) -> Option<String> {
    task.deliverables
        .factory_branch_anchor
        .as_ref()
        .or_else(|| task.deliverables.historical_factory_branch_anchors.last())
        .and_then(|head| commit(repo, head))
}

pub(crate) fn ancestor(repo: &Path, from: &str, to: &str) -> Option<bool> {
    let code = Command::new("git")
        .args(["merge-base", "--is-ancestor", from, to])
        .current_dir(repo)
        .output()
        .ok()?
        .status
        .code()?;
    match code {
        0 => Some(true),
        1 => Some(false),
        _ => None,
    }
}

pub(crate) fn integration_state(repo: &Path, head: &str, target: &str) -> Option<bool> {
    use crate::mcp::tools::core::task::lifecycle::close_ops::commit_is_merged_into_parent;
    // Share the actual merge gate's local OR origin authority. A stale
    // remote-tracking ref cannot turn integrated code back into a deferral.
    if commit_is_merged_into_parent(repo, head, target) {
        return Some(true);
    }
    let tips: Vec<_> = [target.to_string(), format!("origin/{target}")]
        .iter()
        .filter_map(|target| commit(repo, target))
        .collect();
    if tips.is_empty() || tips.iter().any(|tip| ancestor(repo, head, tip).is_none()) {
        None
    } else {
        Some(false)
    }
}

/// Use exactly close's target inheritance and repository resolver.
pub(crate) fn task_context(root: &Path, task: &Task) -> Result<(PathBuf, String), String> {
    use crate::mcp::tools::core::task::{lifecycle::close_ops, repo_context};
    let store = crate::store::open_task_store(root).map_err(|e| e.to_string())?;
    let parent = store.get_parent_epic(&task.id).map_err(|e| e.to_string())?;
    if let Some(target) = close_ops::effective_close_work_target(task, parent.as_ref()) {
        let context = repo_context::resolve_repo_context(root, &target)?;
        return Ok((context.repo_root, context.target_branch));
    }
    let repo = root
        .parent()
        .ok_or("project checkout unavailable")?
        .to_path_buf();
    let worktree_parent = task
        .worktree_id
        .as_deref()
        .map(|id| {
            crate::store::open_worktree_store(root)
                .and_then(|store| Ok(store.get(id)?))
                .map(|worktree| worktree.parent_branch)
                .map_err(|e| e.to_string())
        })
        .transpose()?;
    let target = close_ops::resolve_close_parent_branch(
        worktree_parent,
        parent.as_ref().and_then(|epic| epic.branch.clone()),
        parent.as_ref().and_then(|epic| {
            epic.deliverables
                .work_target
                .as_ref()
                .map(|t| t.target_branch.clone())
        }),
        &repo,
    )?;
    Ok((repo, target))
}

pub(crate) fn validate_ledger(
    root: &Path,
    artifact_id: &str,
    path: &Path,
) -> Result<(cas_store::PublishedArtifact, PathBuf), String> {
    let artifact = cas_store::SqliteArtifactStore::open(root)
        .map_err(|e| e.to_string())?
        .get(artifact_id)
        .map_err(|e| e.to_string())?
        .ok_or("ledger artifact is not published in this project")?;
    if !matches!(artifact.mime.as_str(), "text/markdown" | "text/plain") {
        return Err("published ledger must be Markdown or plain text".into());
    }
    let config = crate::config::Config::load(root).map_err(|e| e.to_string())?;
    let owner = crate::store::open_task_store(root)
        .map_err(|e| e.to_string())?
        .get(&artifact.task_id)
        .map_err(|e| e.to_string())?;
    let project = crate::cloud::resolve_canonical_id(root);
    if !crate::mcp::tools::core::task::task_owned_by_project(&owner, root, project.as_deref()) {
        return Err("ledger artifact owner belongs to another project".into());
    }
    let paths = crate::config::resolved_factory_artifact_paths(
        root,
        config.factory().artifacts_root.as_deref(),
    );
    let roots = crate::artifacts::paths::PublishRoots::new(root, &paths.base, &artifact.task_id);
    let path = crate::artifacts::paths::resolve_publishable_path(path, &roots)
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .map_err(|e| e.to_string())?
        .take(cas_types::ARTIFACT_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 != artifact.size_bytes
        || bytes.len() as u64 > cas_types::ARTIFACT_MAX_BYTES
        || format!("{:x}", Sha256::digest(&bytes)) != artifact.sha256
    {
        return Err("ledger bytes differ from the published artifact (size or SHA-256)".into());
    }
    if std::str::from_utf8(&bytes)
        .map_err(|_| "ledger must be UTF-8 text")?
        .trim()
        .is_empty()
    {
        return Err("ledger must contain nonempty text".into());
    }
    Ok((artifact, path))
}

/// The operator's recorded override for this task, if any (cas-26ac). Only
/// the operator-written file counts; task notes never do.
pub(crate) fn author_override(root: &Path, task: &Task) -> Option<String> {
    crate::qa_author_override::for_task(root, &task.id)
}

/// Every identity that implemented `task` across assignments and deliveries:
/// audit tables, the assignee, factory branch owners and exact Git actors of
/// its delivery anchors. Ancestry alone never makes an integrator an implementer.
pub(crate) fn implementation_lineage(
    root: &Path,
    task: &Task,
    repo: &Path,
) -> Result<Vec<String>, String> {
    let id = &task.id;
    let mut actors = cas_store::qa_task_implementation_actors(root, id).map_err(|e| e.to_string())?;
    actors.extend(task.assignee.clone());
    for branch in task
        .deliverables
        .parked_branch
        .iter()
        .chain(task.deliverables.handoff_branches.iter())
    {
        let branch = branch.strip_prefix("origin/").unwrap_or(branch);
        if let Some(owner) = branch.strip_prefix("factory/") {
            actors.push(owner.strip_suffix(&format!("-{id}")).unwrap_or(owner).into());
        }
    }
    for anchor in task
        .deliverables
        .factory_branch_anchor
        .iter()
        .chain(task.deliverables.historical_factory_branch_anchors.iter())
        .chain(task.deliverables.declined_factory_branch_anchors.iter().flatten())
    {
        if let Some(anchor) = commit(repo, anchor) {
            let out = Command::new("git")
                .args(["show", "-s", "--format=%an%x00%ae%x00%cn%x00%ce", &anchor])
                .current_dir(repo)
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(format!("task {id}: cannot read delivery actors"));
            }
            actors.extend(String::from_utf8_lossy(&out.stdout).trim().split('\0').map(str::to_owned));
        }
    }
    Ok(actors)
}

/// Whether the identity `token` (agent id or name) is one of `actors`.
fn in_lineage(agents: &dyn cas_store::AgentStore, actors: &[String], token: &str) -> bool {
    let agent = crate::mcp::tools::core::task::resolve_agent_identity(agents, token);
    actors.iter().any(|actor| {
        actor == token
            || agent.as_ref().is_some_and(|agent| {
                actor == &agent.id
                    || actor == &agent.name
                    || crate::mcp::tools::core::task::resolve_agent_identity(agents, actor)
                        .is_some_and(|other| other.id == agent.id)
            })
    })
}

/// Tier 0 (docs/tests only) needs no QA ledger, so its author is not gated.
/// An unmeasurable delivery keeps the gate.
fn author_gated(task: &Task, repo: &Path) -> bool {
    use crate::mcp::tools::core::task::lifecycle::review_tier::{VerificationTier, tier_for_delivery};
    tier_for_delivery(task, repo).is_none_or(|decision| decision.tier != VerificationTier::Mechanical)
}

/// Independent QA evidence for `task` cannot come from its own lineage.
/// Self-run checks remain the implementer's proof, never independent QA.
pub(crate) fn author_is_independent(
    root: &Path,
    task: &Task,
    repo: &Path,
    author: &str,
) -> Result<bool, String> {
    if !author_gated(task, repo) {
        return Ok(true);
    }
    let agents = crate::store::open_agent_store(root).map_err(|e| e.to_string())?;
    Ok(!in_lineage(agents.as_ref(), &implementation_lineage(root, task, repo)?, author))
}

pub(crate) fn author_refusal(task: &str, author: &str) -> String {
    format!(
        "ledger author {author} is in {task}'s implementation lineage; the ledger must be walked by a QA reviewer, verifier or a worker that did not deliver it. Self-run checks remain the worker's own proof, not independent QA. Only the operator can override, from their own terminal: `cas qa author-override {task} --note \"<why>\"`"
    )
}

pub(crate) fn prepare_receipt(
    root: &Path,
    recorder: &cas_types::Agent,
    author: &str,
    task_ids: &[String],
    staging: &str,
    artifact_id: &str,
    ledger: &Path,
) -> Result<QaBatchReceipt, String> {
    if staging.len() != 40 || !staging.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("staging_sha must be a full 40-hex commit SHA".into());
    }
    if task_ids.is_empty() || task_ids.len() > 500 {
        return Err("task_ids must contain 1 to 500 PASS task IDs".into());
    }
    let author = author.trim();
    if author.is_empty() {
        return Err("ledger_author (agent id or name of whoever walked the ledger) is required".into());
    }
    let project = crate::cloud::resolve_canonical_id(root).ok_or("project identity unavailable")?;
    let (artifact, ledger) = validate_ledger(root, artifact_id, ledger)?;
    let tasks = crate::store::open_task_store(root).map_err(|e| e.to_string())?;
    let agents = crate::store::open_agent_store(root).map_err(|e| e.to_string())?;
    // A peer on another machine need not be registered here; record it as external.
    let (author_id, author_role) =
        match crate::mcp::tools::core::task::resolve_agent_identity(agents.as_ref(), author) {
            Some(agent) => (agent.id, agent.role.to_string()),
            None => (author.to_owned(), "external".to_owned()),
        };
    let mut heads = BTreeMap::new();
    let mut overrides = BTreeMap::new();
    for id in task_ids {
        let task = tasks.get(id).map_err(|e| format!("task {id}: {e}"))?;
        if !crate::mcp::tools::core::task::task_owned_by_project(&task, root, Some(&project)) {
            return Err(format!(
                "task {id} belongs to another project or has no project binding"
            ));
        }
        if task.assignee.is_none() {
            return Err(format!("task {id} has no implementer"));
        }
        let (repo, target) = task_context(root, &task)?;
        let actors = implementation_lineage(root, &task, &repo)?;
        if in_lineage(agents.as_ref(), &actors, &recorder.id) {
            return Err(format!("no self-review: recorder implemented {id}"));
        }
        if author_gated(&task, &repo) && in_lineage(agents.as_ref(), &actors, &author_id) {
            match author_override(root, &task) {
                Some(words) => {
                    overrides.insert(id.clone(), words);
                }
                None => return Err(author_refusal(id, &author_id)),
            }
        }
        let anchor = delivery_anchor(&repo, &task)
            .ok_or_else(|| format!("task {id} has no resolvable delivery anchor"))?;
        let staging = commit(&repo, staging).ok_or("staging_sha does not resolve to a commit")?;
        if ancestor(&repo, &anchor, &staging) != Some(true) {
            return Err(format!(
                "task {id} delivery anchor is not an ancestor of staging_sha"
            ));
        }
        if integration_state(&repo, &staging, &target) != Some(true) {
            return Err(format!(
                "staging_sha is not on task {id}'s configured target {target}"
            ));
        }
        heads.insert(id.clone(), anchor);
    }
    Ok(QaBatchReceipt {
        schema_version: 1,
        id: format!("qab-{}", uuid::Uuid::new_v4().simple()),
        project_id: project,
        ledger_artifact_id: artifact.id,
        ledger_path: ledger.to_string_lossy().into_owned(),
        ledger_sha256: artifact.sha256,
        ledger_size_bytes: artifact.size_bytes,
        staging_sha: staging.to_ascii_lowercase(),
        task_heads: heads,
        recorder_agent_id: recorder.id.clone(),
        recorded_at: chrono::Utc::now().to_rfc3339(),
        ledger_author_id: Some(author_id),
        ledger_author_role: Some(author_role),
        author_overrides: overrides,
        imported: false,
    })
}

/// A receipt never covers a new task tip, an unrelated project/target, or changed ledger bytes.
pub(crate) fn satisfying_receipt(
    root: &Path,
    task: &Task,
    repo: &Path,
    target: &str,
    head: &str,
) -> Result<Option<QaBatchReceipt>, String> {
    let project = crate::cloud::resolve_canonical_id(root).ok_or("project identity unavailable")?;
    if !crate::mcp::tools::core::task::task_owned_by_project(task, root, Some(&project)) {
        return Ok(None);
    }
    let Some(anchor) = delivery_anchor(repo, task) else {
        return Ok(None);
    };
    // Retired history can recover only its exact delivery. Without an active
    // park, a later integrated head may be an unreviewed correction, rather
    // than an umbrella integration receipt for the reviewed work.
    if task.deliverables.factory_branch_anchor.is_none() && head != anchor {
        return Ok(None);
    }
    let receipts = cas_store::qa_batch_receipts_for_task(root, &project, &task.id)
        .map_err(|e| e.to_string())?;
    for receipt in receipts {
        if receipt.schema_version != 1
            || receipt.project_id != project
            || receipt.task_heads.get(&task.id) != Some(&anchor)
            || ancestor(repo, &anchor, head) != Some(true)
            || ancestor(repo, &anchor, &receipt.staging_sha) != Some(true)
            || !(ancestor(repo, head, &receipt.staging_sha) == Some(true)
                || (ancestor(repo, &receipt.staging_sha, head) == Some(true)
                    && integration_state(repo, head, target) == Some(true)))
            || integration_state(repo, &receipt.staging_sha, target) != Some(true)
        {
            continue;
        }
        // cas-26ac: lineage can grow after recording, so recheck the author.
        // Receipts from before authors were recorded carry none.
        if let Some(author) = receipt.ledger_author_id.as_deref()
            && !receipt.author_overrides.contains_key(&task.id)
            && author_override(root, task).is_none()
            && !author_is_independent(root, task, repo, author)?
        {
            continue;
        }
        // A post-review integration receipt may be later than staging. The
        // immutable task anchor, rather than that umbrella merge, binds QA.
        let ledger_ok = match validate_ledger(
            root,
            &receipt.ledger_artifact_id,
            Path::new(&receipt.ledger_path),
        ) {
            Ok((artifact, _)) => {
                artifact.sha256 == receipt.ledger_sha256
                    && artifact.size_bytes == receipt.ledger_size_bytes
            }
            // cas-6048: a peer's receipt names a ledger published on the
            // machine that recorded it, which validated those bytes there.
            // Without a local copy, its recorded SHA-256 and size stand.
            Err(_) => receipt.imported,
        };
        if ledger_ok {
            return Ok(Some(receipt));
        }
    }
    Ok(None)
}

pub(crate) fn covering_pass(
    root: &Path,
    task: &Task,
    repo: &Path,
    target: &str,
    head: &str,
) -> Result<Option<cas_types::QaPass>, String> {
    let Some(anchor) = delivery_anchor(repo, task) else {
        return Ok(None);
    };
    if task.deliverables.factory_branch_anchor.is_none() && head != anchor {
        return Ok(None);
    }
    if ancestor(repo, &anchor, head) != Some(true)
        || (head != anchor && integration_state(repo, head, target) != Some(true))
    {
        return Ok(None);
    }
    for pass in cas_store::satisfying_qa_passes(root, &task.id).map_err(|e| e.to_string())? {
        if pass.state != cas_types::QaPassState::Passed
            || commit(repo, &pass.bound_head).as_deref() != Some(anchor.as_str())
        {
            continue;
        }
        // cas-26ac: a reviewer later found in the task's lineage is not independent.
        if let Some(reviewer) = pass.reviewer_agent_id.as_deref()
            && author_override(root, task).is_none()
            && !author_is_independent(root, task, repo, reviewer)?
        {
            continue;
        }
        return Ok(Some(pass));
    }
    Ok(None)
}

pub(crate) fn debt_refusal(task: &str) -> String {
    format!(
        "waiting for batch receipt: {task} deferred evidence at its batch park; a valid batch_receipt listing this delivery anchor or a covering passed independent QA round is required before close"
    )
}

/// Final transition guard, independent of QA mode, classification, merge
/// outcome and whether the earlier optional evidence gate was reached.
pub(crate) fn ensure_deferred_close(
    root: &Path,
    task: &Task,
    receipt: Option<&str>,
    window: Option<&TaskCommitReceiptWindow>,
) -> Result<(), String> {
    if !cas_store::has_qa_batch_deferral(root, &task.id).map_err(|e| e.to_string())? {
        return Ok(());
    }
    // cas-1b4d: in an operator's rapid-response window each fix merges and
    // closes as it lands; the batch receipt back-fills when the window closes.
    if let Some(window) = crate::rapid_response::active_for_task_at(root, task, None)
        && crate::rapid_response::record_skip(root, &window, &task.id, "batch-qa-receipt", "closed before its batch QA receipt")
    {
        return Ok(());
    }
    let (repo, target) = task_context(root, task)?;
    let head = receipt
        .or(task.deliverables.factory_branch_anchor.as_deref())
        .and_then(|head| commit(&repo, head))
        .ok_or_else(|| debt_refusal(&task.id))?;
    let satisfies = |head: &str| -> Result<bool, String> {
        Ok(
            satisfying_receipt(root, task, &repo, &target, head)?.is_some()
                || covering_pass(root, task, &repo, &target, head)?.is_some(),
        )
    };
    if satisfies(&head)? {
        return Ok(());
    }
    // A validated squash can carry the reviewed delivery without retaining
    // its ancestry. Use the evidence gate's mapping and the same attribution
    // window, then recheck the immutable anchor, receipt and ledger binding.
    if let Some(anchor) = window
        .and_then(|window| squash_receipt_qa_anchor(&repo, &target, window, task, Some(&head)))
        && satisfies(&anchor)?
    {
        return Ok(());
    }
    Err(debt_refusal(&task.id))
}
