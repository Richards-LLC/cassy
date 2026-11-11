//! Validation shared by batch recording and close. No process or polling work.
pub(crate) mod release_close;
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

/// Immutable release identity and declared membership, shared by all adapters.
/// Proof references identify bytes, not approval; the proof consumer validates
/// author, coverage, content and candidate-to-release mapping separately.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ReleaseScope {
    pub project_id: String,
    pub repository: String,
    pub release_id: String,
    pub reviewed_sha: String,
    pub final_release_sha: String,
    pub promotion_pr: Option<u64>,
    pub release_tag: Option<String>,
    #[serde(default)]
    pub constituent_epic_ids: Vec<String>,
    #[serde(default)]
    pub included_task_ids: Vec<String>,
    #[serde(default)]
    pub no_code_members: BTreeMap<String, Vec<ReleaseProofRef>>,
    #[serde(default)]
    pub proof_refs: Vec<ReleaseProofRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReleaseProofKind {
    Qa,
    Review,
    Report,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ReleaseProofRef {
    pub kind: ReleaseProofKind,
    pub artifact_id: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub reviewed_sha: String,
    pub receipt_id: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ReleaseInventory {
    pub scope: ReleaseScope,
    pub deliveries: Vec<ReleaseDelivery>,
    pub exceptions: Vec<ReleaseInventoryException>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ReleaseDelivery {
    pub task_id: String,
    /// Conservative task snapshot token. ANY intervening task mutation means
    /// the consumer must re-evaluate; this is not a lease generation number.
    pub generation: String,
    pub anchor: Option<String>,
    pub base_sha: Option<String>,
    pub shipped_commit: Option<String>,
    pub membership: ReleaseMembership,
    pub proof_refs: Vec<ReleaseProofRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReleaseMembership {
    Ancestry,
    SquashTree,
    ExplicitNoCode,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReleaseInventoryCode {
    MissingTask,
    ForeignScope,
    NotEpic,
    Cancelled,
    MissingAnchor,
    RetiredAnchor,
    MissingBase,
    InvalidBase,
    MissingMapping,
    MissingNoCodeProof,
    Quarantined,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ReleaseInventoryException {
    pub task_id: String,
    pub code: ReleaseInventoryCode,
}

/// Read-only, deterministic inventory at an exact commit. No network, branch
/// discovery, commit-subject parsing, proof approval or lifecycle writes.
/// Epic descendants and live recorded deliveries are discovery candidates.
/// Only declared tasks or proven shipped work create closeout obligations.
// gate-audit[evidence_only]: selector Filters delivery inventory by measured terminal outcome; no acceptance bypass.
pub(crate) fn resolve_release_inventory(
    root: &Path,
    repo: &Path,
    scope: &ReleaseScope,
) -> Result<ReleaseInventory, String> {
    use crate::mcp::tools::core::task::{self, lifecycle::close_ops, repo_context};
    use cas_types::{DependencyType, TaskStatus, TaskType};
    use std::collections::BTreeSet;
    let project = crate::cloud::resolve_canonical_id(root).ok_or("project identity unavailable")?;
    if scope.project_id != project || scope.release_id.trim().is_empty() {
        return Err("release scope does not identify this project and release".into());
    }
    let slug = crate::github_repo::origin_slug(repo, std::time::Duration::from_secs(3))?
        .ok_or("release repository has no origin")?;
    if !slug.eq_ignore_ascii_case(&scope.repository)
        || crate::worktree::GitOperations::new(repo.to_path_buf())
            .canonical_repo_key()
            .map_err(|e| e.to_string())?
            != crate::worktree::GitOperations::new(
                root.parent()
                    .ok_or("project checkout unavailable")?
                    .to_path_buf(),
            )
            .canonical_repo_key()
            .map_err(|e| e.to_string())?
    {
        return Err("release repository does not match project scope".into());
    }
    for sha in [&scope.reviewed_sha, &scope.final_release_sha] {
        if sha.len() != 40 || commit(repo, sha).as_deref() != Some(sha.as_str()) {
            return Err("release scope needs resolvable full commit SHAs".into());
        }
    }
    if let Some(tag) = &scope.release_tag {
        if commit(repo, &format!("refs/tags/{tag}")).as_deref()
            != Some(scope.final_release_sha.as_str())
        {
            return Err("release tag does not identify final_release_sha".into());
        }
    }
    // Structured identities must be immutable commits, never a branch alias.
    let recorded_commit =
        |sha: &str| commit(repo, sha).filter(|resolved| resolved.eq_ignore_ascii_case(sha));
    let store = crate::store::open_task_store(root).map_err(|e| e.to_string())?;
    let mut tasks: BTreeMap<_, _> = store
        .list(None)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|t| (t.id.clone(), t))
        .collect();
    // Board suppression cannot erase a shipped fact. `get` intentionally
    // permits quarantine inspection; the marker remains an explicit hold.
    let quarantined = crate::cloud::SyncQueue::open_read_only(root)
        .and_then(|queue| queue.quarantined_ids(crate::cloud::QUARANTINE_TASK))
        .unwrap_or_default();
    for id in &quarantined {
        if let Ok(task) = store.get(id) {
            tasks.insert(id.clone(), task);
        }
    }
    let dependencies = store
        .list_dependencies(Some(DependencyType::ParentChild))
        .map_err(|e| e.to_string())?;
    let receipts = cas_store::list_qa_batch_receipts(root, &project).map_err(|e| e.to_string())?;
    let mut inventory = ReleaseInventory {
        scope: scope.clone(),
        deliveries: Vec::new(),
        exceptions: Vec::new(),
    };
    let expected: BTreeSet<String> = scope
        .included_task_ids
        .iter()
        .cloned()
        .chain(scope.no_code_members.keys().cloned())
        .collect();
    let mut candidates = expected.clone();
    let mut pending = scope.constituent_epic_ids.clone();
    let mut epics = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !epics.insert(id.clone()) {
            continue;
        }
        if quarantined.contains(&id) {
            inventory.exceptions.push(ReleaseInventoryException {
                task_id: id.clone(),
                code: ReleaseInventoryCode::Quarantined,
            });
        }
        let code = match tasks.get(&id) {
            None => Some(ReleaseInventoryCode::MissingTask),
            Some(t) if !task::task_owned_by_project(t, root, Some(&project)) => {
                Some(ReleaseInventoryCode::ForeignScope)
            }
            Some(t) if t.task_type != TaskType::Epic => Some(ReleaseInventoryCode::NotEpic),
            _ => None,
        };
        if let Some(code) = code {
            inventory
                .exceptions
                .push(ReleaseInventoryException { task_id: id, code });
            continue;
        }
        for edge in dependencies.iter().filter(|d| d.to_id == id) {
            if tasks
                .get(&edge.from_id)
                .is_some_and(|t| t.task_type == TaskType::Epic)
            {
                pending.push(edge.from_id.clone());
            } else {
                candidates.insert(edge.from_id.clone());
            }
        }
    }
    candidates.extend(
        tasks
            .values()
            .filter(|t| {
                !t.is_terminal()
                    && t.task_type != TaskType::Epic
                    && task::task_owned_by_project(t, root, Some(&project))
                    && (t.deliverables.factory_branch_anchor.is_some()
                        || !t.deliverables.historical_factory_branch_anchors.is_empty()
                        || t.deliverables.green_merge_commit.is_some()
                        || t.deliverables.delivery_pr_merge_commit.is_some()
                        || t.deliverables.merge_commit.is_some())
            })
            .map(|t| t.id.clone()),
    );
    for id in candidates {
        let mut issue = |code| {
            inventory.exceptions.push(ReleaseInventoryException {
                task_id: id.clone(),
                code,
            })
        };
        let Some(t) = tasks.get(&id) else {
            if expected.contains(&id) {
                issue(ReleaseInventoryCode::MissingTask);
            }
            continue;
        };
        let d = &t.deliverables;
        let base = d.factory_branch_start.as_ref()
            .and_then(|start| recorded_commit(&start.base_sha));
        let recorded = d.factory_branch_anchor.as_ref()
            .or(d.green_merge_commit.as_ref())
            .or(d.delivery_pr_merge_commit.as_ref())
            .or(d.merge_commit.as_ref());
        let anchor = recorded.and_then(|sha| recorded_commit(sha));
        let valid_base = |anchor: &str| base.as_deref()
            .is_some_and(|base| ancestor(repo, base, anchor) == Some(true) && base != anchor);
        let shipped_mapping = |anchor: &str| {
            if ancestor(repo, anchor, &scope.final_release_sha) == Some(true) {
                Some((anchor.to_string(), ReleaseMembership::Ancestry))
            } else {
                valid_base(anchor)
                    .then(|| close_ops::find_squash_tree_match(repo, anchor, &scope.final_release_sha))
                    .flatten()
                    .filter(|mapped| ancestor(repo, mapped, &scope.final_release_sha) == Some(true))
                    .map(|mapped| (mapped, ReleaseMembership::SquashTree))
            }
        };
        let shipped = anchor.as_deref().and_then(&shipped_mapping);
        if !expected.contains(&id) && shipped.is_none()
            && !d.historical_factory_branch_anchors.iter()
                .filter_map(|sha| recorded_commit(sha))
                .any(|historical| shipped_mapping(&historical).is_some())
        {
            continue;
        }
        if !task::task_owned_by_project(t, root, Some(&project)) || t.task_type == TaskType::Epic {
            issue(ReleaseInventoryCode::ForeignScope);
            continue;
        }
        if t.status == TaskStatus::Cancelled {
            issue(ReleaseInventoryCode::Cancelled);
            continue;
        }
        if quarantined.contains(&id) {
            issue(ReleaseInventoryCode::Quarantined);
        }
        let parent = dependencies
            .iter()
            .find(|d| d.from_id == id)
            .and_then(|d| tasks.get(&d.to_id));
        if let Some(target) = close_ops::effective_close_work_target(t, parent) {
            if !repo_context::repo_answers_to(repo, &target.repo_selector) {
                issue(ReleaseInventoryCode::ForeignScope);
                continue;
            }
        }
        let explicit = scope.no_code_members.get(&id);
        let (anchor, shipped_commit, membership, proofs) = if crate::qa_pass::is_no_code(t)
            || t.deliverables.evidence_only.is_some()
        {
            let Some(proofs) = explicit.filter(|proofs| {
                !proofs.is_empty()
                    && proofs.iter().all(|p| {
                        !p.artifact_id.is_empty()
                            && p.sha256.len() == 64
                            && p.sha256.bytes().all(|b| b.is_ascii_hexdigit())
                            && p.size_bytes > 0
                            && p.reviewed_sha == scope.reviewed_sha
                    })
            }) else {
                issue(ReleaseInventoryCode::MissingNoCodeProof);
                continue;
            };
            (
                None,
                None,
                ReleaseMembership::ExplicitNoCode,
                proofs.clone(),
            )
        } else {
            if d.factory_branch_anchor.is_none() && !d.historical_factory_branch_anchors.is_empty()
            {
                issue(ReleaseInventoryCode::RetiredAnchor);
                continue;
            }
            let Some(anchor) = anchor else {
                issue(ReleaseInventoryCode::MissingAnchor);
                continue;
            };
            if base.is_none() {
                issue(ReleaseInventoryCode::MissingBase);
            } else if !valid_base(&anchor) {
                issue(ReleaseInventoryCode::InvalidBase);
            }
            let Some((shipped, membership)) = shipped else {
                issue(ReleaseInventoryCode::MissingMapping);
                continue;
            };
            let proofs = release_proof_refs(&receipts, &id, &anchor);
            (Some(anchor), Some(shipped), membership, proofs)
        };
        inventory.deliveries.push(ReleaseDelivery {
            task_id: id,
            generation: t.updated_at.to_rfc3339(),
            anchor,
            base_sha: base,
            shipped_commit,
            membership,
            proof_refs: proofs,
        });
    }
    inventory
        .exceptions
        .sort_by(|a, b| a.task_id.cmp(&b.task_id));
    Ok(inventory)
}

fn release_proof_refs(
    receipts: &[QaBatchReceipt],
    task_id: &str,
    anchor: &str,
) -> Vec<ReleaseProofRef> {
    let mut proofs: Vec<_> = receipts
        .iter()
        .filter_map(|r| {
            let kind = match r.schema_version {
                cas_store::QA_BATCH_RECEIPT_SCHEMA => ReleaseProofKind::Qa,
                cas_store::RELEASE_VERDICT_SCHEMA => ReleaseProofKind::Review,
                _ => return None, // a published tag never becomes QA or code review
            };
            (r.task_heads.get(task_id).is_some_and(|head| head == anchor)).then(|| {
                ReleaseProofRef {
                    kind,
                    artifact_id: r.ledger_artifact_id.clone(),
                    sha256: r.ledger_sha256.clone(),
                    size_bytes: r.ledger_size_bytes,
                    reviewed_sha: r.staging_sha.clone(),
                    receipt_id: Some(r.id.clone()),
                }
            })
        })
        .collect();
    proofs.sort_by(|a, b| a.receipt_id.cmp(&b.receipt_id));
    proofs
}

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

/// GH #1641: a task merged through a GitHub PR has no recorded anchor. Resolve
/// it as close's `commit_receipt` does: its recorded branch tip once integrated
/// on `target`, else the commit on `target` whose tree equals that tip (squash).
fn merged_branch_anchor(repo: &Path, task: &Task, target: &str) -> Option<String> {
    use crate::mcp::tools::core::task::lifecycle::close_ops::find_squash_tree_match;
    let d = &task.deliverables;
    let branches = d
        .parked_branch
        .iter()
        .chain(d.factory_branch_start.iter().map(|s| &s.branch))
        .chain(d.handoff_branches.iter());
    for branch in branches {
        let branch = branch.strip_prefix("origin/").unwrap_or(branch);
        let Some(tip) = [format!("origin/{branch}"), branch.to_string()]
            .iter()
            .find_map(|r| commit(repo, r))
        else {
            continue;
        };
        // A reused lane is not task identity: skip a tip that is another task's.
        if !crate::mcp::tools::core::task::lifecycle::merge_close::delivery_tip_belongs_to_task(repo, task, branch, &tip) {
            continue;
        }
        // A lane that moved on for other work still ends at someone else's tip:
        // the task must own at least one commit between its start and the tip.
        if let Some(start) = d.factory_branch_start.as_ref().filter(|start| start.branch == branch) {
            use crate::mcp::tools::core::task::lifecycle::close_ops::{commit_is_task_attributable, task_commit_identity};
            let identity = task_commit_identity(task, None);
            let range = Command::new("git")
                .args(["rev-list", &format!("{}..{tip}", start.base_sha)])
                .current_dir(repo)
                .output()
                .ok()
                .filter(|out| out.status.success())
                .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
                .unwrap_or_default();
            if !range.lines().any(|c| commit_is_task_attributable(repo, c, &identity)) {
                continue;
            }
        }
        if integration_state(repo, &tip, target) == Some(true) {
            return Some(tip);
        }
        if let Some(squash) = find_squash_tree_match(repo, &tip, target) {
            return Some(squash);
        }
    }
    None
}

/// The delivery identity every receipt consumer shares: the recorded anchor, or,
/// when none was ever recorded (PR merge), the merged branch tip. Invalid
/// recorded anchors never fall back.
pub(crate) fn resolved_delivery_anchor(repo: &Path, task: &Task, target: &str) -> Option<String> {
    delivery_anchor(repo, task).or_else(|| {
        let d = &task.deliverables;
        (d.factory_branch_anchor.is_none() && d.historical_factory_branch_anchors.is_empty())
            .then(|| merged_branch_anchor(repo, task, target))
            .flatten()
    })
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

/// cas-4b22: a parked task's ACTIVE delivery anchor once it is integrated on
/// its own target, as close's merge gate measures it. History never counts: a
/// task reopened for rework after an earlier merge has not shipped its rework.
pub(crate) fn shipped_anchor(root: &Path, task: &Task) -> Option<String> {
    if task.status != cas_types::TaskStatus::AwaitingMerge {
        return None;
    }
    let (repo, target) = task_context(root, task).ok()?;
    let anchor = commit(&repo, task.deliverables.factory_branch_anchor.as_deref()?)?;
    (integration_state(&repo, &anchor, &target) == Some(true)).then_some(anchor)
}

/// Use exactly close's target inheritance and repository resolver.
pub(crate) fn task_context(root: &Path, task: &Task) -> Result<(PathBuf, String), String> {
    use crate::mcp::tools::core::task::{lifecycle::close_ops, repo_context};
    let store = crate::store::open_task_store(root).map_err(|e| e.to_string())?;
    let parent = store.get_parent_epic(&task.id).map_err(|e| e.to_string())?;
    if let Some(target) = close_ops::effective_close_work_target(task, parent.as_ref()) {
        let context = repo_context::resolve_repo_context(root, &target)?;
        let target = release_close::target(root, &task.id).unwrap_or(context.target_branch);
        return Ok((context.repo_root, target));
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
    let target = release_close::target(root, &task.id).unwrap_or(target);
    Ok((repo, target))
}

pub(crate) fn validate_ledger(
    root: &Path,
    artifact_id: &str,
    path: &Path,
) -> Result<(cas_store::PublishedArtifact, PathBuf), String> {
    let artifact = crate::artifacts::lookup_metadata(root, None, artifact_id)
        .map_err(|e| e.to_string())?;
    crate::artifacts::require_available(&artifact).map_err(|e| e.to_string())?;
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
    let path = match crate::artifacts::paths::resolve_publishable_path(path, &roots) {
        Ok(path) => path,
        Err(crate::artifacts::paths::PathRefusal::Missing(_)) => {
            let (resolved, path) = crate::artifacts::proof::fetch_task(
                root, &artifact.task_id, artifact_id,
            ).map_err(|e| e.to_string())?;
            if !crate::artifacts::proof::same_artifact(&artifact, &resolved) {
                return Err("ledger metadata changed while resolving its bytes".into());
            }
            crate::artifacts::paths::resolve_publishable_path(&path, &roots)
                .map_err(|e| e.to_string())?
        }
        Err(error) => return Err(error.to_string()),
    };
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
// gate-audit[forward]: Stored operator approval is returned to receipt admission (G-116); coverage lookups replay evidence and do not mint an override.
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
    // cas-f497: the recorded start still names the implementer once the assignee is cleared.
    actors.extend(task.deliverables.factory_branch_start.as_ref().map(|start| start.assignee.clone()));
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
        "ledger author {author} is in {task}'s implementation lineage; the ledger must be walked by a QA reviewer, verifier or a worker that did not deliver it. Self-run checks remain the worker's own proof, not independent QA. A QA-ledger-only release verdict can use a supervisor's exact-head qa_waive (docs/qa/batch-qa.md). Otherwise only the operator can override, from their own terminal: `cas qa author-override {task} --note \"<why>\"`"
    )
}

/// GH #1655: a supervisor may accept the author of a QA record for release
/// closeout. This never supplies independent QA or covers product/catalog work.
/// Measure immutable Git paths, not a no-code declaration or mutable task notes.
pub(crate) fn release_author_waiver(
    root: &Path,
    task: &Task,
    repo: &Path,
    head: &str,
) -> Result<Option<cas_types::QaPass>, String> {
    let passes = cas_store::list_qa_passes(root, &task.id).map_err(|e| e.to_string())?;
    let Some(pass) = passes.into_iter().next().filter(|pass| {
        pass.state == cas_types::QaPassState::Waived
            && pass.bound_head == head
            && pass.issuer_agent_id.as_deref().is_some_and(|issuer| !issuer.trim().is_empty())
            && pass.summary.as_deref().is_some_and(|reason| !reason.trim().is_empty())
    }) else { return Ok(None) };
    let Some(base) = task.deliverables.factory_branch_start.as_ref()
        .and_then(|start| commit(repo, &start.base_sha)) else { return Ok(None) };
    if delivery_anchor(repo, task).as_deref() != Some(head)
        || ancestor(repo, &base, head) != Some(true)
    {
        return Ok(None);
    }
    let output = Command::new("git")
        .args(["diff", "--no-ext-diff", "--name-only", "--no-renames", "-z", &base, head, "--"])
        .current_dir(repo).output().map_err(|e| e.to_string())?;
    if !output.status.success() || output.stdout.is_empty() {
        return Ok(None);
    }
    let records_only = output.stdout.split(|byte| *byte == 0).filter(|path| !path.is_empty()).all(|path| {
        std::str::from_utf8(path).is_ok_and(|path| {
            (path.starts_with("docs/qa/") && path.ends_with("/LEDGER.md"))
                || path.strip_prefix("docs/qa/batch-evaluations/")
                    .is_some_and(|name| !name.contains('/') && name.ends_with(".md")
                        && !matches!(name, "AGENTS.md" | "CLAUDE.md" | "GROK.md" | "SKILL.md"))
        })
    });
    Ok(records_only.then_some(pass))
}

/// Whether `report` names `task` as a whole ID, so `cas-12` is not `cas-123`.
fn report_names(report: &str, task: &str) -> bool {
    report
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .any(|word| word == task)
}

pub(crate) fn prepare_receipt(
    root: &Path,
    recorder: &cas_types::Agent,
    author: &str,
    task_ids: &[String],
    staging: &str,
    artifact_id: &str,
    ledger: &Path,
    schema: u32,
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
    let body = std::fs::read_to_string(&ledger).map_err(|e| e.to_string())?;
    // Declared coverage is authoritative for both QA and release receipts.
    // Historical ledgers keep their existing whole-ID and row checks.
    let mut coverage = crate::qa_ledger::coverage(&body);
    if coverage.covers.is_some() {
        coverage.require_coverage(task_ids);
        if !coverage.ok { return Err(coverage.refusal()); }
    }
    // A QA receipt is only as good as the ledger the close gate will read, so
    // run that gate's validator (`cas qa ledger check --task <owner>`) first and
    // refuse with its diagnostics before anything is recorded.
    if schema == cas_store::QA_BATCH_RECEIPT_SCHEMA {
        let owner = crate::store::open_task_store(root)
            .map_err(|e| e.to_string())?
            .get(&artifact.task_id)
            .map_err(|e| e.to_string())?;
        let report = crate::cli::qa_ledger::check(&ledger, Some(&owner));
        if !report.ok {
            return Err(format!("ledger fails `cas qa ledger check`:\n{}", report.refusal()));
        }
    }
    if schema == cas_store::RELEASE_VERDICT_SCHEMA {
        if let Some(id) = task_ids.iter().find(|id| !report_names(&body, id)) {
            return Err(format!("the published report does not name {id}; one verdict must name every delivery it covers"));
        }
    }
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
        let (repo, target) = task_context(root, &task)?;
        let actors = implementation_lineage(root, &task, &repo)?;
        if actors.is_empty() {
            return Err(format!("task {id} has no implementer"));
        }
        if in_lineage(agents.as_ref(), &actors, &recorder.id) {
            return Err(format!("no self-review: recorder implemented {id}"));
        }
        let anchor = resolved_delivery_anchor(&repo, &task, &target)
            .ok_or_else(|| format!("task {id} has no resolvable delivery anchor"))?;
        if author_gated(&task, &repo) && in_lineage(agents.as_ref(), &actors, &author_id) {
            match author_override(root, &task) {
                Some(words) => {
                    overrides.insert(id.clone(), words);
                }
                None if schema == cas_store::RELEASE_VERDICT_SCHEMA
                    && release_author_waiver(root, &task, &repo, &anchor)?.is_some() => {},
                None => return Err(author_refusal(id, &author_id)),
            }
        }
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
        schema_version: schema,
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
        release_tag: None,
        ledger_author_id: Some(author_id),
        ledger_author_role: Some(author_role),
        author_overrides: overrides,
        imported: false,
    })
}

/// Prepare a receipt whose explicit verdict kind is a published tag. The tag
/// must contain each named delivery's tree effect; no QA ledger or ledger
/// author is invented for this evidence.
// gate[G-117 release-verdict]: Release tag verdict carries each delivery's tree effect
pub(crate) fn prepare_release_tag_receipt(
    root: &Path,
    task_ids: &[String],
    tag: &str,
) -> Result<QaBatchReceipt, String> {
    if task_ids.is_empty() || task_ids.len() > 500 {
        return Err(crate::gates::tag("G-117", "release tag verdict needs 1 to 500 task IDs"));
    }
    if tag.is_empty() || tag.starts_with('-') {
        return Err(crate::gates::tag("G-117", "release tag is invalid"));
    }
    let project = crate::cloud::resolve_canonical_id(root).ok_or("project identity unavailable")?;
    let tasks = crate::store::open_task_store(root).map_err(|e| e.to_string())?;
    let mut heads = BTreeMap::new();
    let mut tag_sha = None;
    for id in task_ids {
        let task = tasks.get(id).map_err(|e| format!("task {id}: {e}"))?;
        if !crate::mcp::tools::core::task::task_owned_by_project(&task, root, Some(&project)) {
            return Err(crate::gates::tag("G-117", format!("task {id} belongs to another project or has no project binding")));
        }
        let (repo, target) = task_context(root, &task)?;
        let candidate = commit(&repo, &format!("refs/tags/{tag}"))
            .ok_or_else(|| crate::gates::tag("G-117", format!("published release tag {tag} is unavailable in {}", repo.display())))?;
        if tag_sha.as_ref().is_some_and(|prior| prior != &candidate) {
            return Err(crate::gates::tag("G-117", format!("release tag {tag} resolves differently across task repositories")));
        }
        tag_sha.get_or_insert(candidate.clone());
        let anchor = resolved_delivery_anchor(&repo, &task, &target)
            .ok_or_else(|| crate::gates::tag("G-117", format!("task {id} has no resolvable delivery anchor")))?;
        if !release_tag_ships_delivery(&repo, &anchor, &candidate, &task.id) {
            return Err(crate::gates::tag("G-117", format!("release tag {tag} does not contain task {id}'s delivery tree effect")));
        }
        heads.insert(id.clone(), anchor);
    }
    Ok(QaBatchReceipt {
        schema_version: cas_store::RELEASE_TAG_VERDICT_SCHEMA,
        id: format!("qtag-{}", uuid::Uuid::new_v4().simple()),
        project_id: project,
        ledger_artifact_id: String::new(),
        ledger_path: String::new(),
        ledger_sha256: String::new(),
        ledger_size_bytes: 0,
        staging_sha: tag_sha.ok_or("release tag verdict has no tasks")?,
        task_heads: heads,
        recorder_agent_id: "release-automation".into(),
        recorded_at: chrono::Utc::now().to_rfc3339(),
        release_tag: Some(tag.to_string()),
        ledger_author_id: None,
        ledger_author_role: None,
        author_overrides: BTreeMap::new(),
        imported: false,
    })
}

/// A release tag contains a delivery when it carries the delivery commit or
/// preserves the delivery's tree effect, including a later edit to its files.
pub(crate) fn release_tag_ships_delivery(repo: &Path, anchor: &str, tag_sha: &str, task_id: &str) -> bool {
    ancestor(repo, anchor, tag_sha) == Some(true)
        || matches!(
            crate::mcp::tools::core::task::lifecycle::close_ops::delivery_content_presence_on_target_for_task(
                repo, anchor, tag_sha, task_id,
            ),
            crate::mcp::tools::core::task::lifecycle::close_ops::DeliveryContentPresence::Present { .. }
                | crate::mcp::tools::core::task::lifecycle::close_ops::DeliveryContentPresence::Superseded { .. }
        )
}

/// The QA receipt covering this delivery.
pub(crate) fn satisfying_receipt(
    root: &Path,
    task: &Task,
    repo: &Path,
    target: &str,
    head: &str,
) -> Result<Option<QaBatchReceipt>, String> {
    covering_receipt(root, task, repo, target, head, cas_store::qa_batch_receipts_for_task)
}

/// The release verdict covering this delivery, bound exactly as a QA receipt is.
pub(crate) fn satisfying_release_verdict(
    root: &Path,
    task: &Task,
    repo: &Path,
    target: &str,
    head: &str,
) -> Result<Option<QaBatchReceipt>, String> {
    covering_receipt(root, task, repo, target, head, cas_store::release_verdicts_for_task)
}

/// A later release review can retire an expired Tier 1 dispatch, but cannot
/// replace a live cycle, a rejected review, or immutable delivery authority.
/// Keep the old dispatch as audit history; every integration gate still runs.
pub(crate) fn release_supersedes_stale_dispatch(
    root: &Path,
    task: &Task,
) -> Result<Option<QaBatchReceipt>, String> {
    use cas_types::VerificationDispatchState::{Claimed, Pending, TimedOut};
    let Some(dispatch) = cas_store::get_latest_verification_dispatch(root, &task.id)
        .map_err(|e| e.to_string())? else { return Ok(None) };
    if !matches!(dispatch.state, TimedOut | Pending | Claimed)
        || (dispatch.state != TimedOut && dispatch.deadline_at > chrono::Utc::now())
        || dispatch.receipt_id.is_some()
        || dispatch.delivery_transaction_id.is_some()
        || task.deliverables.factory_branch_start.is_none()
        || task.deliverables.factory_branch_anchor.is_none()
    {
        return Ok(None);
    }
    let latest = crate::store::open_verification_store(root)
        .map_err(|e| e.to_string())?
        .get_latest_for_task(&task.id).map_err(|e| e.to_string())?;
    if latest.is_some_and(|verdict| !matches!(
        verdict.status, cas_types::VerificationStatus::Pending | cas_types::VerificationStatus::Approved
    )) {
        return Ok(None);
    }
    let (repo, target) = task_context(root, task)?;
    if !crate::mcp::tools::core::task::lifecycle::review_tier::release_verdict_owns(root, task, &repo) {
        return Ok(None);
    }
    let Some(anchor) = task.deliverables.factory_branch_anchor.as_deref() else { return Ok(None) };
    Ok(satisfying_release_verdict(root, task, &repo, &target, anchor)?
        .filter(|receipt| chrono::DateTime::parse_from_rfc3339(&receipt.recorded_at)
            .is_ok_and(|recorded| recorded > dispatch.requested_at)))
}

/// Deferred Tier 1 batch debt follows its release verdict's exact anchor,
/// project, independent author and published bytes. This does not turn a code
/// review into general QA evidence or waive Tier 2 / dedicated review paths.
pub(crate) fn satisfying_deferred_receipt(
    root: &Path,
    task: &Task,
    repo: &Path,
    target: &str,
    head: &str,
) -> Result<Option<QaBatchReceipt>, String> {
    if let Some(receipt) = satisfying_receipt(root, task, repo, target, head)? {
        return Ok(Some(receipt));
    }
    if crate::mcp::tools::core::task::lifecycle::review_tier::release_verdict_owns(root, task, repo) {
        return satisfying_release_verdict(root, task, repo, target, head);
    }
    Ok(None)
}

/// cas-f497: whether the close gate would answer WAITING FOR RELEASE VERDICT for
/// this delivery. Anything unreadable is `false`, so the delivery keeps its usual
/// stall prompt rather than going quiet. Runs Git; call it off the factory loop.
pub(crate) fn awaits_release_verdict(root: &Path, task: &Task) -> bool {
    use crate::mcp::tools::core::task::lifecycle::review_tier::release_verdict_owns;
    use cas_types::VerificationDispatchState::{Invalidated, Resolved};
    let (Some(_), Some(anchor)) = (
        task.deliverables.factory_branch_start.as_ref(),
        task.deliverables.factory_branch_anchor.as_deref(),
    ) else {
        return false;
    };
    if task.depth == cas_types::TaskDepth::Light
        || !crate::config::Config::load(root).is_ok_and(|config| config.verification_enabled())
    {
        return false;
    }
    let Ok((repo, target)) = task_context(root, task) else {
        return false;
    };
    let no_verifier_on_record = matches!(
        cas_store::get_latest_verification_dispatch(root, &task.id),
        Ok(None) | Ok(Some(cas_types::VerificationDispatch { state: Resolved | Invalidated, .. }))
    ) && crate::store::open_verification_store(root)
        .is_ok_and(|store| store.get_latest_for_task(&task.id).is_ok_and(|latest| latest.is_none()));
    no_verifier_on_record
        && release_verdict_owns(root, task, &repo)
        && matches!(satisfying_release_verdict(root, task, &repo, &target, anchor), Ok(None))
}

/// A receipt never covers a new task tip, an unrelated project/target, or changed ledger bytes.
// gate-audit[author_override]: selector Read-only validation of issued QA evidence; the accepting batch-receipt producer records G-116, not coverage replays.
// gate-audit[release_author_waiver]: selector Read-only replay of release-bound evidence; verification_batch_receipt records G-116 when issuing evidence under that waiver.
fn covering_receipt(
    root: &Path,
    task: &Task,
    repo: &Path,
    target: &str,
    head: &str,
    receipts_for_task: fn(&Path, &str, &str) -> cas_store::Result<Vec<QaBatchReceipt>>,
) -> Result<Option<QaBatchReceipt>, String> {
    let project = crate::cloud::resolve_canonical_id(root).ok_or("project identity unavailable")?;
    if !crate::mcp::tools::core::task::task_owned_by_project(task, root, Some(&project)) {
        return Ok(None);
    }
    let Some(anchor) = resolved_delivery_anchor(repo, task, target) else {
        return Ok(None);
    };
    // Retired history can recover only reviewed delivery. Without an active
    // park, a later integrated head may be an unreviewed correction, rather
    // than an umbrella integration receipt for the reviewed work. cas-bc15:
    // request_changes retires anchors, and a close's commit_receipt binds a
    // new one, so a receipt may name any of them; a retired one covers only a
    // head inside the receipt's reviewed staging.
    let active = task.deliverables.factory_branch_anchor.as_ref().and(Some(anchor.as_str()));
    let retired: Vec<String> = task.deliverables.historical_factory_branch_anchors.iter()
        .filter_map(|head| commit(repo, head))
        .collect();
    let receipts = receipts_for_task(root, &project, &task.id).map_err(|e| e.to_string())?;
    let fixed_target = release_close::target(root, &task.id);
    let target = fixed_target.as_deref().unwrap_or(target);
    for receipt in receipts {
        if !release_close::receipt_allowed(root, task, &receipt) { continue; }
        let Some(reviewed) = receipt.task_heads.get(&task.id) else { continue };
        let known = *reviewed == anchor || retired.contains(reviewed);
        if receipt.schema_version == cas_store::RELEASE_TAG_VERDICT_SCHEMA {
            let Some(tag) = receipt.release_tag.as_deref() else { continue };
            let tag_ref = format!("refs/tags/{tag}");
            if !known
                || ![anchor.as_str(), head].contains(&reviewed.as_str())
                || commit(repo, &tag_ref).as_deref() != Some(receipt.staging_sha.as_str())
                || !release_tag_ships_delivery(repo, reviewed, &receipt.staging_sha, &task.id)
            {
                continue;
            }
            return Ok(Some(receipt));
        }
        let inside_review = active == Some(reviewed.as_str())
            || head == reviewed
            || ancestor(repo, head, &receipt.staging_sha) == Some(true);
        if !known
            || !inside_review
            || receipt.project_id != project
            || ancestor(repo, reviewed, head) != Some(true)
            || ancestor(repo, reviewed, &receipt.staging_sha) != Some(true)
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
            && !(receipt.schema_version == cas_store::RELEASE_VERDICT_SCHEMA
                && release_author_waiver(root, task, repo, reviewed)?.is_some())
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

// gate-audit[author_override]: selector Read-only evidence lookup replays operator authorization; no new approval or delivery accepted.
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
// gate[G-83 close-after-merge]: Deferred batch QA debt (waiting for batch receipt)
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
    // A post-merge reopen clears the active anchor; the retired one still names the
    // reviewed delivery, exactly as `qa_status` reads it (cas-c041).
    let head = match receipt.or(task.deliverables.factory_branch_anchor.as_deref()) {
        Some(head) => commit(&repo, head),
        None => delivery_anchor(&repo, task),
    }
    .ok_or_else(|| debt_refusal(&task.id))?;
    let satisfies = |head: &str| -> Result<bool, String> {
        Ok(
            satisfying_deferred_receipt(root, task, &repo, &target, head)?.is_some()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The receipt step and the close gate must read a ledger with one validator.
    #[test]
    fn receipt_refuses_a_ledger_the_check_command_rejects_and_accepts_one_it_passes() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        let cas = crate::store::init_cas_dir(project).unwrap();
        env.set("CAS_ROOT", cas.to_str().unwrap());
        env.set("CAS_CLONE_PATH", project.to_str().unwrap());
        std::fs::write(cas.join("config.toml"), "[project]\ncanonical_id='receipt-ledger-fixture'\n[qa]\nmode='batch'\n").unwrap();
        let mut owner = Task::new("cas-train".into(), "train".into());
        owner.execution_note = Some("no-code".into());
        crate::store::open_task_store(&cas).unwrap().add(&owner).unwrap();
        let recorder = cas_types::Agent::new_with_role("sup".into(), "sup".into(), cas_types::AgentRole::Supervisor);
        let staging = "0".repeat(40);
        let ledger = project.join("LEDGER.md");
        let receipt = |body: &str| {
            std::fs::write(&ledger, body).unwrap();
            let store = cas_store::SqliteArtifactStore::open(&cas).unwrap();
            let context = crate::artifacts::PublishContext {
                store: &store,
                roots: crate::artifacts::paths::PublishRoots::new(&cas, &project.join("artifacts"), "cas-train"),
                task_id: "cas-train".into(), cloud: None,
            };
            let artifact = crate::artifacts::publish(&context, &ledger).unwrap().artifact.id;
            prepare_receipt(&cas, &recorder, "external-qa", &["cas-train".into()], &staging, &artifact, &ledger, cas_store::QA_BATCH_RECEIPT_SCHEMA)
        };
        let valid = crate::qa_ledger::scaffold(&owner, &[], "rev");
        std::fs::write(&ledger, &valid).unwrap();
        assert!(crate::cli::qa_ledger::check(&ledger, Some(&owner)).ok);
        // Same shape as the rejected r2 ledger: its headline repeats a count.
        let broken = valid.replacen("cells=1;", "cells=1; cells=1;", 1);
        std::fs::write(&ledger, &broken).unwrap();
        let expected = crate::cli::qa_ledger::check(&ledger, Some(&owner));
        assert!(!expected.ok);
        let refused = receipt(&broken).unwrap_err();
        assert!(refused.contains(&expected.refusal()), "{refused}");
        // A ledger the command accepts is not refused by the ledger gate (later gates may still apply).
        let later = receipt(&valid).err().unwrap_or_default();
        assert!(!later.contains(&expected.refusal()), "{later}");
    }
}
