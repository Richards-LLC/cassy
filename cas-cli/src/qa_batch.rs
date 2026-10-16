//! Validation shared by batch recording and close. No process or polling work.
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

pub(crate) fn target_tip(repo: &Path, target: &str) -> Option<String> {
    commit(repo, &format!("origin/{target}")).or_else(|| commit(repo, target))
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
                .and_then(|store| store.get(id))
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

pub(crate) fn prepare_receipt(
    root: &Path,
    recorder: &cas_types::Agent,
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
    let project = crate::cloud::resolve_canonical_id(root).ok_or("project identity unavailable")?;
    let (artifact, ledger) = validate_ledger(root, artifact_id, ledger)?;
    let tasks = crate::store::open_task_store(root).map_err(|e| e.to_string())?;
    let agents = crate::store::open_agent_store(root).map_err(|e| e.to_string())?;
    let mut heads = BTreeMap::new();
    for id in task_ids {
        let task = tasks.get(id).map_err(|e| format!("task {id}: {e}"))?;
        if !crate::mcp::tools::core::task::task_belongs_to_project(&task, Some(&project)) {
            return Err(format!(
                "task {id} belongs to another project or has no project binding"
            ));
        }
        let implementer = task
            .assignee
            .as_deref()
            .ok_or_else(|| format!("task {id} has no implementer"))?;
        let implementer_id =
            crate::mcp::tools::core::task::resolve_agent_identity(agents.as_ref(), implementer);
        if implementer == recorder.id
            || implementer == recorder.name
            || implementer_id.is_some_and(|a| a.id == recorder.id)
        {
            return Err(format!("no self-review: recorder implemented {id}"));
        }
        let (repo, target) = task_context(root, &task)?;
        let anchor = task
            .deliverables
            .factory_branch_anchor
            .as_deref()
            .and_then(|head| commit(&repo, head))
            .ok_or_else(|| format!("task {id} has no resolvable delivery anchor"))?;
        let staging = commit(&repo, staging).ok_or("staging_sha does not resolve to a commit")?;
        let tip =
            target_tip(&repo, &target).ok_or_else(|| format!("target {target} is unresolved"))?;
        if ancestor(&repo, &anchor, &staging) != Some(true) {
            return Err(format!(
                "task {id} delivery anchor is not an ancestor of staging_sha"
            ));
        }
        if ancestor(&repo, &staging, &tip) != Some(true) {
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
    if !crate::mcp::tools::core::task::task_belongs_to_project(task, Some(&project)) {
        return Ok(None);
    }
    let Some(anchor) = task
        .deliverables
        .factory_branch_anchor
        .as_deref()
        .and_then(|h| commit(repo, h))
    else {
        return Ok(None);
    };
    let Some(target_tip) = target_tip(repo, target) else {
        return Ok(None);
    };
    let receipts = cas_store::list_qa_batch_receipts(root, &project).map_err(|e| e.to_string())?;
    for receipt in receipts {
        if receipt.schema_version != 1
            || receipt.project_id != project
            || receipt.task_heads.get(&task.id) != Some(&anchor)
            || ancestor(repo, &anchor, head) != Some(true)
            || ancestor(repo, head, &receipt.staging_sha) != Some(true)
            || ancestor(repo, &receipt.staging_sha, &target_tip) != Some(true)
        {
            continue;
        }
        let Ok((artifact, _)) = validate_ledger(
            root,
            &receipt.ledger_artifact_id,
            Path::new(&receipt.ledger_path),
        ) else {
            continue;
        };
        if artifact.sha256 == receipt.ledger_sha256
            && artifact.size_bytes == receipt.ledger_size_bytes
        {
            return Ok(Some(receipt));
        }
    }
    Ok(None)
}
