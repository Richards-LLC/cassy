//! Validation shared by batch recording and close. No process or polling work.
pub(crate) mod release_close;
use crate::mcp::tools::core::task::lifecycle::close_ops::{
    TaskCommitReceiptWindow, squash_receipt_qa_anchor,
};
use cas_store::{QaBatchReceipt, TaskStore};
use cas_types::Task;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
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

impl ReleaseScope {
    // gate-audit[evidence_only]: selector Binds report proof metadata; proof validation and acceptance remain with the consumers.
    pub(crate) fn bind_reports(&mut self, root: &Path) -> Result<(), String> {
        let tasks = release_close::read_tasks(root)?;
        self.no_code_members.clear();
        for id in &self.included_task_ids {
            let Ok(task) = tasks.get(id) else { continue; };
            // Terminal members need no new proof to be closed again. Their
            // external_ref may identify the original request, not report bytes.
            if task.is_terminal() { continue; }
            if crate::qa_pass::is_no_code(&task) || task.deliverables.evidence_only.is_some() {
                self.no_code_members.insert(id.clone(), no_code_proofs(root, self, &task)?);
            }
        }
        Ok(())
    }
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

/// G-32 already records the report reference at park. Freeze its registered
/// bytes for the release member instead of asking the caller to type them again.
pub(crate) fn no_code_proofs(root: &Path, scope: &ReleaseScope, task: &Task) -> Result<Vec<ReleaseProofRef>, String> {
    let reference = task.external_ref.as_deref().ok_or_else(|| format!("RELEASE_NO_CODE_PROOF_MISSING: {}", task.id))?;
    let reference = reference.trim();
    let reference = reference.strip_prefix("artifact:").unwrap_or(reference);
    let artifact = release_close::read_artifact(root, reference)
        .map_err(|e| format!("RELEASE_NO_CODE_PROOF_BINDING: {}: {e}", task.id))?;
    let proofs = vec![ReleaseProofRef {
        kind: ReleaseProofKind::Report, artifact_id: artifact.id,
        sha256: artifact.sha256, size_bytes: artifact.size_bytes,
        reviewed_sha: scope.reviewed_sha.clone(), receipt_id: None,
    }];
    crate::release_reconcile::validate_no_code_proofs(root, scope, &task.id, &proofs)?;
    Ok(proofs)
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
    /// The member's delivery cycle (`delivery_generation`). A new cycle is a
    /// new delivery; an anchor change inside a cycle is checked at close.
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
    Reverted,
    UnprovenContent,
}

impl ReleaseInventoryCode {
    pub(crate) fn close_gate(&self) -> String {
        match self {
            Self::Reverted => "RELEASE_MEMBER_REVERTED".into(),
            Self::UnprovenContent => "RELEASE_CONTENT_UNPROVEN".into(),
            other => format!("RELEASE_MEMBER_HELD: {other:?}"),
        }
    }
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
            .map_err(|e| e.to_string())
            .or_else(|_| {
                // A replay store may be copied outside Git. Its original
                // repository is still the caller's cwd, as in release preview.
                let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
                crate::worktree::GitOperations::new(cwd)
                    .canonical_repo_key()
                    .map_err(|e| e.to_string())
            })?
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
    let store = release_close::read_tasks(root)?;
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
    let receipts = release_close::read_receipts(root, &project)?;
    let mut inventory = ReleaseInventory {
        scope: scope.clone(),
        deliveries: Vec::new(),
        exceptions: Vec::new(),
    };
    let review = release_close::ReleaseClose::prepare(root, repo, &inventory)?;
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
    let mut members = Vec::new();
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
        let anchor = resolved_delivery_anchor(repo, t, &scope.final_release_sha);
        let valid_base = |anchor: &str| base.as_deref()
            .is_some_and(|base| ancestor(repo, base, anchor) == Some(true));
        let release_mapping = |anchor: &str| {
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
        let mapped = anchor.as_deref().and_then(&release_mapping);
        if !expected.contains(&id) && mapped.is_none()
            && !d.historical_factory_branch_anchors.iter()
                .filter_map(|sha| recorded_commit(sha))
                .any(|historical| release_mapping(&historical).is_some())
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
                            && recorded_commit(&p.reviewed_sha).is_some()
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
            if review.ledger_reverts(&id, Some(&anchor)) {
                issue(ReleaseInventoryCode::Reverted);
                continue;
            }
            let Some((shipped, membership)) = mapped else {
                issue(ReleaseInventoryCode::MissingMapping);
                continue;
            };
            let proofs = release_proof_refs(&receipts, &id, &anchor);
            (Some(anchor), Some(shipped), membership, proofs)
        };
        let measured = membership != ReleaseMembership::ExplicitNoCode;
        members.push((ReleaseDelivery {
            task_id: id,
            generation: delivery_generation(t),
            anchor,
            base_sha: base,
            shipped_commit,
            membership,
            proof_refs: proofs,
        }, measured));
    }
    // cas-0e29e: every member's content walk crosses the same release
    // history. Walk it once per delivered path for all members, then judge
    // each member in order from those answers.
    let measured: Vec<_> = members
        .iter()
        .filter(|(_, measured)| *measured)
        .filter_map(|(delivery, _)| Some((delivery.base_sha.as_deref(), delivery.anchor.as_deref()?, delivery.task_id.as_str())))
        .collect();
    prewalk_release_effects(repo, &scope.final_release_sha, &measured);
    for (delivery, measured) in members {
        if measured
            && let Some(anchor) = delivery.anchor.as_deref()
            && let Err(code) = release_delivery_effect_for_task(
                root, repo, &tasks[&delivery.task_id], delivery.base_sha.as_deref(), anchor, &scope.final_release_sha,
                review.reviewed_drop(&delivery.task_id, Some(anchor)).as_deref(),
            )
        {
            inventory.exceptions.push(ReleaseInventoryException { task_id: delivery.task_id, code });
            continue;
        }
        inventory.deliveries.push(delivery);
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
/// An unresolvable newer anchor must never resurrect an older reviewed tip,
/// and a recorded anchor is a commit id, never a ref name.
pub(crate) fn delivery_anchor(repo: &Path, task: &Task) -> Option<String> {
    task.deliverables
        .factory_branch_anchor
        .as_ref()
        .or_else(|| task.deliverables.historical_factory_branch_anchors.last())
        .and_then(|head| commit(repo, head).filter(|resolved| resolved.eq_ignore_ascii_case(head)))
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

/// A release member's generation: its delivery cycle (design rev 3.1 §2.5).
pub(crate) fn delivery_generation(task: &Task) -> String {
    format!("cycle:{}", task.deliverables.delivery_cycle)
}

/// The delivery identity every receipt consumer shares: the recorded anchor, or,
/// when none was ever recorded (PR merge), the merged branch tip. Invalid
/// recorded anchors never fall back.
pub(crate) fn resolved_delivery_anchor(repo: &Path, task: &Task, target: &str) -> Option<String> {
    delivery_anchor(repo, task).or_else(|| {
        let d = &task.deliverables;
        (d.factory_branch_anchor.is_none() && d.historical_factory_branch_anchors.is_empty())
            .then(|| merged_branch_anchor(repo, task, target).or_else(|| {
                [&d.green_merge_commit, &d.delivery_pr_merge_commit, &d.merge_commit]
                    .into_iter().flatten()
                    .find_map(|sha| commit(repo, sha).filter(|resolved| resolved.eq_ignore_ascii_case(sha)))
            }))
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

/// An integrated active delivery, or its recorded batch-pending recovery.
/// A restarted/reused branch is never delivery identity. Genuine new work and
/// explicit review rejection still hold dependents.
pub(crate) fn shipped_anchor(root: &Path, task: &Task) -> Option<String> {
    let (repo, target) = task_context(root, task).ok()?;
    let anchor = if task.status == cas_types::TaskStatus::AwaitingMerge {
        commit(&repo, task.deliverables.factory_branch_anchor.as_deref()?)?
    } else {
        if task.status != cas_types::TaskStatus::Blocked
            || !cas_store::has_qa_batch_deferral(root, &task.id).ok()?
        {
            return None;
        }
        let anchor = delivery_anchor(&repo, task)?;
        let observed = cas_store::observed_delivery_merges_into(root, &target).ok()?
            .into_iter().rev().find(|merge| merge.task_id == task.id)?;
        if observed.source_commit_sha != anchor
            || integration_state(&repo, &observed.merge_commit_sha, &target) != Some(true)
        {
            return None;
        }
        if let Some(dispatch) = cas_store::get_latest_verification_dispatch(root, &task.id).ok()?
            && let Some(verdict) = cas_store::get_verification_for_dispatch(root, &dispatch.id).ok()?
            && verdict.status == cas_types::VerificationStatus::Rejected
        {
            return None;
        }
        anchor
    };
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
    let anchors: Vec<String> = task
        .deliverables
        .factory_branch_anchor
        .iter()
        .chain(task.deliverables.historical_factory_branch_anchors.iter())
        .chain(task.deliverables.declined_factory_branch_anchors.iter().flatten())
        .filter_map(|anchor| commit(repo, anchor))
        .collect();
    let handoffs = task.deliverables.handoff_branches.iter().map(|branch| (branch, true));
    for (branch, handoff) in task.deliverables.parked_branch.iter().map(|branch| (branch, false)).chain(handoffs) {
        let branch = branch.strip_prefix("origin/").unwrap_or(branch);
        let Some(owner) = branch.strip_prefix("factory/") else { continue };
        if let Some(owner) = owner.strip_suffix(&format!("-{id}")) {
            actors.push(owner.into());
        } else if !handoff || branch_contributed(repo, branch, &anchors, task) {
            // cas-1b7d: a bare factory/<agent> handoff is that agent's shared
            // checkout; it names an implementer only when it carries delivery commits.
            actors.push(owner.into());
        }
    }
    for anchor in anchors {
        {
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

/// Whether `branch` (local or on origin) shares a commit with a delivery anchor
/// beyond the task's recorded start base. A missing branch carries nothing; an
/// unknown start base keeps the owner counted.
fn branch_contributed(repo: &Path, branch: &str, anchors: &[String], task: &Task) -> bool {
    let Some(tip) = commit(repo, branch).or_else(|| commit(repo, &format!("origin/{branch}"))) else {
        return false;
    };
    let Some(base) = task.deliverables.factory_branch_start.as_ref().and_then(|start| commit(repo, &start.base_sha)) else {
        return true;
    };
    let git = |args: &[&str]| Command::new("git").args(args).current_dir(repo).output().ok();
    anchors.iter().any(|anchor| {
        git(&["merge-base", &tip, anchor])
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
            .is_some_and(|shared| {
                git(&["merge-base", "--is-ancestor", &shared, &base]).is_some_and(|out| !out.status.success())
            })
    })
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

// gate-audit[author_override]: bypass G-116 Accepts a new receipt from a lineage author under stored operator approval.
// gate-audit[release_author_waiver]: bypass G-116 Accepts a new release verdict under its exact-head QA-author waiver.
// gate-audit[accepted_by]: verification_batch_receipt
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
    let (derived_head, derived_ids);
    let (task_ids, staging) = if schema == cas_store::RELEASE_VERDICT_SCHEMA
        && (task_ids.is_empty() || staging.is_empty()) {
        derived_ids = coverage.covers.clone().ok_or("release ledger needs covers: member IDs")?;
        let ids = derived_ids.as_slice();
        let mut builds = body.lines().filter_map(|line| line.strip_prefix("build: "));
        derived_head = builds.next().ok_or("release ledger needs build: reviewed commit")?.trim().to_string();
        if builds.next().is_some() || (!task_ids.is_empty() && task_ids.iter().collect::<std::collections::BTreeSet<_>>() != ids.iter().collect())
            || (!staging.is_empty() && staging != derived_head) {
            return Err("release receipt inputs differ from the published ledger".into());
        }
        (ids, derived_head.as_str())
    } else { (task_ids, staging) };
    if staging.len() != 40 || !staging.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("staging_sha must be a full 40-hex commit SHA".into());
    }
    if task_ids.is_empty() || task_ids.len() > 500 {
        return Err("task_ids must contain 1 to 500 PASS task IDs".into());
    }
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
        // A structurally valid ledger can still record failures (GH #1713);
        // those are evidence, never a PASS receipt.
        let failed: Vec<_> = report.rows.iter()
            .filter(|row| row.verdict == crate::qa_ledger::Verdict::Fail)
            .map(|row| format!("{} (line {}, defect {})", row.id, row.line, row.defect_task))
            .collect();
        if !failed.is_empty() {
            return Err(format!("ledger records FAIL cells, so it cannot back a PASS receipt: {}", failed.join(", ")));
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
    let reviews = release_close::read_receipts(root, &project)?;
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
        let reviewed_drop = release_close::reviewed_drop_for_tag(&reviews, &repo, id, &anchor, &candidate);
        if release_delivery_effect_for_task(root, &repo, &task,
            task.deliverables.factory_branch_start.as_ref().map(|start| start.base_sha.as_str()),
            &anchor, &candidate, reviewed_drop.as_deref()).is_err() {
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

/// History membership cannot prove shipment: a later revert retains ancestry.
/// The final tree must preserve the delivery's effect, including ordinary edits.
pub(crate) fn release_tag_ships_delivery(root: &Path, repo: &Path, task: &Task, anchor: &str, tag_sha: &str) -> bool {
    release_delivery_effect_for_task(root, repo, task,
        task.deliverables.factory_branch_start.as_ref().map(|start| start.base_sha.as_str()),
        anchor, tag_sha, None).is_ok()
}

pub(crate) fn release_delivery_effect(
    repo: &Path, base: Option<&str>, anchor: &str, target: &str, task_id: &str,
) -> Result<(), ReleaseInventoryCode> {
    release_delivery_effect_reviewed(repo, base, anchor, target, task_id, None)
}

fn release_delivery_effect_for_task(
    root: &Path, repo: &Path, task: &Task, base: Option<&str>, anchor: &str, target: &str,
    reviewed_drop: Option<&str>,
) -> Result<(), ReleaseInventoryCode> {
    use crate::mcp::tools::core::{task::{self, lifecycle::close_ops, repo_context}, workflow::premerge_verification::cycle};
    let presence = release_delivery_effect_reviewed(repo, base, anchor, target, &task.id, reviewed_drop);
    if !matches!(presence, Err(ReleaseInventoryCode::UnprovenContent)) {
        return presence;
    }
    let source_effect = || {
        let base = base?;
        let grants = close_ops::task_commit_identity(task, None).approved_merge_parents;
        if grants.is_empty() || ancestor(repo, anchor, target) != Some(true) {
            return None;
        }
        let output = Command::new("git").args(["show", "-s", "--format=%P", anchor, "--"])
            .current_dir(repo).output().ok().filter(|result| result.status.success())?;
        let parents = std::str::from_utf8(&output.stdout).ok()?.split_whitespace().collect::<Vec<_>>();
        let [parent, source_head] = parents.as_slice() else { return None; };
        let tree = |revision: &str| Command::new("git")
            .args(["rev-parse", "--verify", "--end-of-options", &format!("{revision}^{{tree}}")])
            .current_dir(repo).output().ok().filter(|result| result.status.success())
            .map(|result| result.stdout);
        let same_tree = |left: &str, right: &str| tree(left).is_some_and(|tree_id| tree(right) == Some(tree_id));
        if *parent != base || !grants.iter().any(|grant| grant == source_head)
            || !same_tree(anchor, source_head) {
            return None;
        }
        let project = crate::cloud::resolve_canonical_id(root)?;
        let sources = release_close::read_tasks(root).ok()?.list(Some(cas_types::TaskStatus::Closed)).ok()?;
        let mut sources = sources.iter().filter(|source| {
            source.id != task.id && source.task_type != cas_types::TaskType::Epic
                && !crate::qa_pass::is_no_code(source) && source.deliverables.evidence_only.is_none()
                && task::task_owned_by_project(source, root, Some(&project))
                && source.deliverables.factory_branch_anchor.as_deref() == Some(*source_head)
        });
        let source = sources.next()?;
        let start = source.deliverables.factory_branch_start.as_ref()?;
        if sources.next().is_some() || start.base_sha == *source_head
            || commit(repo, &start.base_sha).as_deref() != Some(start.base_sha.as_str())
            || ancestor(repo, &start.base_sha, source_head) != Some(true)
            || !repo_context::repo_answers_to(repo, &start.work_target.repo_selector)
            || !same_tree(base, &start.base_sha) {
            return None;
        }
        let approved = cycle::approved(root, repo, source, source_head, target).ok()?.is_some()
            || (release_close::no_local_review(&release_close::read_connection(root).ok()?, &source.id).ok()?
                && release_close::carried_approval(source, source_head));
        approved.then(|| release_delivery_effect(repo, Some(&start.base_sha), source_head, target, &source.id))
    };
    source_effect().or_else(|| reviewed_import_effect(root, repo, task, base?, anchor, target))
        .unwrap_or(presence)
}

type ReleaseTreeEntries = BTreeMap<Vec<u8>, Vec<u8>>;

fn release_git(repo: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git").args(args).current_dir(repo).output().ok()?;
    output.status.success().then_some(output.stdout)
}

fn release_tree_entries(repo: &Path, revision: &str) -> Option<ReleaseTreeEntries> {
    let output = release_git(repo, &["ls-tree", "-rz", "--full-tree", revision])?;
    let mut entries = BTreeMap::new();
    for record in output.split(|byte| *byte == 0).filter(|record| !record.is_empty()) {
        let separator = record.iter().position(|byte| *byte == b'\t')?;
        if entries.insert(record[separator + 1..].to_vec(), record[..separator].to_vec()).is_some() {
            return None;
        }
    }
    Some(entries)
}

// gate-audit[evidence_only]: selector Excludes report-only rows from fragment ownership; no delivery approval or refusal is waived.
fn reviewed_import_effect(
    root: &Path, repo: &Path, task: &Task, base: &str, anchor: &str, target: &str,
) -> Option<Result<(), ReleaseInventoryCode>> {
    use crate::mcp::tools::core::{task::{self, lifecycle::close_ops, repo_context}, workflow::premerge_verification::cycle};

    let delivery = &task.deliverables;
    let start = delivery.factory_branch_start.as_ref()?;
    let work_target = delivery.work_target.as_ref()?;
    let landing = delivery.green_merge_commit.as_deref()?;
    let grants = close_ops::task_commit_identity(task, None).approved_merge_parents;
    if grants.len() < 2 || start.base_sha != base || work_target != &start.work_target
        || delivery.factory_branch_anchor.as_deref() != Some(anchor)
        || delivery.effective_merge_ci_admission() != cas_types::MergeCiAdmission::Green
        || !repo_context::repo_answers_to(repo, &work_target.repo_selector)
        || [base, anchor, landing, target].iter().any(|oid| commit(repo, oid).as_deref() != Some(*oid))
        || ancestor(repo, anchor, landing) != Some(true) || ancestor(repo, landing, target) != Some(true)
    {
        return None;
    }
    let tree = |revision: &str| release_git(repo, &["rev-parse", "--verify", "--end-of-options", &format!("{revision}^{{tree}}")]);
    if tree(anchor)? != tree(landing)? { return None; }
    let project = crate::cloud::resolve_canonical_id(root)?;
    let connection = release_close::read_connection(root).ok()?;
    let approved = |source: &Task, head: &str| Some(
        cycle::approved(root, repo, source, head, target).ok()?.is_some()
        || (release_close::no_local_review(&connection, &source.id).ok()?
            && release_close::carried_approval(source, head))
    );
    if !task::task_owned_by_project(task, root, Some(&project)) || !approved(task, anchor)? {
        return None;
    }

    let history = String::from_utf8(release_git(repo, &[
        "rev-list", "--first-parent", "--parents", "--max-count=201", &format!("{base}..{anchor}"),
    ])?).ok()?;
    let edges = history.lines().collect::<Vec<_>>();
    if !(2..=200).contains(&edges.len()) { return None; }
    let baseline = release_tree_entries(repo, base)?;
    let mut lane = baseline.clone();
    let mut tip = base;
    let mut contributors = BTreeMap::new();
    for edge in edges.into_iter().rev() {
        let commits = edge.split_whitespace().collect::<Vec<_>>();
        let [merge, parent, side] = commits.as_slice() else { return None; };
        if *parent != tip || !grants.iter().any(|grant| grant == side) { return None; }
        let common = String::from_utf8(release_git(repo, &["merge-base", "--all", parent, side])?).ok()?;
        let mut bases = common.lines();
        let common = bases.next()?;
        if bases.next().is_some() { return None; }
        let ancestor_tree = release_tree_entries(repo, common)?;
        let imported_tree = release_tree_entries(repo, side)?;
        let merged = release_tree_entries(repo, merge)?;
        let paths: BTreeSet<_> = ancestor_tree.keys().chain(lane.keys())
            .chain(imported_tree.keys()).chain(merged.keys()).collect();
        for path in paths {
            let prior = ancestor_tree.get(path);
            let retained = lane.get(path);
            let imported = imported_tree.get(path);
            let expected = if retained == imported || imported == prior { retained }
                else if retained == prior { imported }
                else { return None; };
            if merged.get(path) != expected { return None; }
            if expected != retained {
                contributors.insert(path.clone(), (*parent, *side));
            }
        }
        lane = merged;
        tip = merge;
    }
    if tip != anchor { return None; }
    contributors.retain(|path, _| baseline.get(path) != lane.get(path));
    if contributors.is_empty() { return None; }

    let published = release_tree_entries(repo, target)?;
    let mut sources = None;
    let mut source_histories = BTreeMap::new();
    for (path, (parent, side)) in contributors {
        let entry = lane.get(&path);
        let shipped = published.get(&path);
        if shipped == entry { continue; }
        let name = std::str::from_utf8(&path).ok();
        if let Some(name) = name.filter(|name| shipped.is_none() && entry.is_some()
            && ["changelog.d/", "docs/product-overview.d/", "docs/qa/bug-families/"].iter().any(|prefix| name.starts_with(prefix)))
        {
            let entry = entry?;
            if !entry.starts_with(b"100644 blob ") && !entry.starts_with(b"100755 blob ") { return None; }
            if sources.is_none() {
                sources = Some(release_close::read_tasks(root).ok()?.list(Some(cas_types::TaskStatus::Closed)).ok()?);
            }
            let source_commits = match source_histories.entry((parent, side)) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => {
                    let history = String::from_utf8(release_git(repo, &["rev-list", side, &format!("^{parent}")])?).ok()?;
                    entry.insert(history.lines().map(str::to_owned).collect::<BTreeSet<_>>())
                }
            };
            let literal = format!(":(literal){name}");
            let expected = [entry.as_slice(), b"\t", path.as_slice(), b"\0"].concat();
            let mut folded = None;
            for source in sources.as_ref()? {
                let source_delivery = &source.deliverables;
                let (Some(head), Some(source_start)) = (source_delivery.factory_branch_anchor.as_deref(),
                    source_delivery.factory_branch_start.as_ref()) else { continue; };
                if !source_commits.contains(head) || source.id == task.id || source.task_type == cas_types::TaskType::Epic
                    || crate::qa_pass::is_no_code(source) || source_delivery.evidence_only.is_some()
                    || !task::task_owned_by_project(source, root, Some(&project))
                    || head == source_start.base_sha
                    || !repo_context::repo_answers_to(repo, &source_start.work_target.repo_selector)
                    || release_git(repo, &["ls-tree", "-rz", "--full-tree", head, "--", &literal])? != expected
                    || commit(repo, &source_start.base_sha).as_deref() != Some(source_start.base_sha.as_str())
                    || ancestor(repo, &source_start.base_sha, head) != Some(true)
                {
                    continue;
                }
                let Some(result) = close_ops::release_owned_fragment_fold(
                    repo, &source_start.base_sha, head, target, &source.id, name,
                ) else { continue; };
                if !approved(source, head)? { continue; }
                if folded.replace(result).is_some() { return None; }
            }
            if !folded? { return Some(Err(ReleaseInventoryCode::Reverted)); }
        } else if shipped == baseline.get(&path) || shipped.is_none() {
            return Some(Err(ReleaseInventoryCode::Reverted));
        } else {
            return None;
        }
    }
    Some(Ok(()))
}

/// As [`release_delivery_effect`], where a supervisor's reviewed drop may
/// accept measured loss. The default stays held: the review must name
/// commits that resolve, reach the target and touch the dropped paths.
pub(crate) fn release_delivery_effect_reviewed(
    repo: &Path, base: Option<&str>, anchor: &str, target: &str, task_id: &str,
    reviewed_drop: Option<&str>,
) -> Result<(), ReleaseInventoryCode> {
    use crate::mcp::tools::core::task::lifecycle::close_ops::{self, DeliveryContentPresence};
    // An explicitly empty delivery has no content that can be reverted.
    let Some((base, shipped)) = release_effect_range(repo, base, anchor, target)? else { return Ok(()) };
    let shipped = shipped.as_str();
    let presence = match base {
        Some(base) => close_ops::delivery_content_presence_from_base_on_target_for_task(repo, base, anchor, shipped, target, task_id),
        None => close_ops::delivery_content_presence_on_target_for_task(repo, shipped, target, task_id),
    };
    match presence {
        DeliveryContentPresence::Present { .. } | DeliveryContentPresence::Superseded { .. } => Ok(()),
        // GH #2026: on-target ancestry plus a measured range lets later edits
        // classify as evolution; only a real revert holds the member.
        DeliveryContentPresence::Dropped { paths } if shipped == anchor && base.is_some_and(|base|
            !paths.iter().any(|path| close_ops::release_dropped_path_reverted(repo, base, anchor, target, path))) => {
                if paths.iter().all(|path| close_ops::release_generated_output(repo, path))
                    && !base.is_some_and(|base| close_ops::release_delivery_has_source(repo, base, anchor, task_id))
                {
                    Err(ReleaseInventoryCode::UnprovenContent)
                } else { Ok(()) }
            },
        DeliveryContentPresence::Dropped { paths } if reviewed_drop.is_some_and(|reason| matches!(
            close_ops::validated_delivery_drop_review(repo, anchor, target, &paths, Some(reason)),
            Ok(Some(_)))) => Ok(()),
        DeliveryContentPresence::Dropped { .. } => Err(ReleaseInventoryCode::Reverted),
        DeliveryContentPresence::Unknown { .. } => Err(ReleaseInventoryCode::UnprovenContent),
    }
}

/// The measured base and shipped commit of a release delivery, or `None` for
/// an explicitly empty delivery.
fn release_effect_range<'a>(
    repo: &Path, base: Option<&'a str>, anchor: &str, target: &str,
) -> Result<Option<(Option<&'a str>, String)>, ReleaseInventoryCode> {
    use crate::mcp::tools::core::task::lifecycle::close_ops;
    // A squash has the reviewed source tree but different commit identity.
    // Continuity starts at its actual integration, then follows later edits.
    let mapped = if ancestor(repo, anchor, target) == Some(true) { None } else {
        Some(close_ops::find_squash_tree_match(repo, anchor, target)
            .filter(|mapped| ancestor(repo, mapped, target) == Some(true))
            .ok_or(ReleaseInventoryCode::UnprovenContent)?)
    };
    let shipped = mapped.unwrap_or_else(|| anchor.to_string());
    // A reused lane can record a newer start than the preserved delivery.
    // Such a start cannot define this delivery's forward content range.
    let base = base.filter(|base| ancestor(repo, base, anchor) == Some(true));
    Ok((base != Some(anchor)).then_some((base, shipped)))
}

/// cas-0e29e: walk every member's delivered paths once, together, before
/// `release_delivery_effect` judges each member (`(base, anchor, task_id)`).
fn prewalk_release_effects(repo: &Path, target: &str, members: &[(Option<&str>, &str, &str)]) {
    use crate::mcp::tools::core::task::lifecycle::close_ops::{self, TaskDeliveryRange};
    let ranges: Vec<_> = members
        .iter()
        .filter_map(|(base, anchor, task_id)| {
            let (base, shipped) = release_effect_range(repo, *base, anchor, target).ok()??;
            Some((base, *anchor, shipped, *task_id))
        })
        .collect();
    let deliveries: Vec<_> = ranges
        .iter()
        .map(|(base, anchor, shipped, task_id)| TaskDeliveryRange { base: *base, anchor, shipped, task_id })
        .collect();
    close_ops::prewalk_task_deliveries(repo, target, &deliveries);
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

/// Batch QA remains independent of the merge-time code approval.
pub(crate) fn satisfying_deferred_receipt(
    root: &Path, task: &Task, repo: &Path, target: &str, head: &str,
) -> Result<Option<QaBatchReceipt>, String> {
    satisfying_receipt(root, task, repo, target, head)
}

/// A receipt never covers a new task tip, an unrelated project/target, or changed ledger bytes.
fn covering_receipt(
    root: &Path,
    task: &Task,
    repo: &Path,
    target: &str,
    head: &str,
    receipts_for_task: fn(&Path, &str, &str) -> cas_store::Result<Vec<QaBatchReceipt>>,
) -> Result<Option<QaBatchReceipt>, String> {
    let mut covering = None;
    walk_receipts(root, task, repo, target, head, receipts_for_task, |receipt, gap| {
        if gap.is_none() {
            covering = Some(receipt.clone());
        }
        gap.is_none()
    })?;
    Ok(covering)
}

/// Judge each recorded receipt against the delivery in order, handing the
/// visitor the receipt and the first reason it does not cover (`None`: it
/// covers). The visitor stops the walk by returning true.
// gate-audit[author_override]: selector Read-only validation of issued QA evidence; the accepting batch-receipt producer records G-116, not coverage replays.
// gate-audit[release_author_waiver]: selector Read-only replay of release-bound evidence; verification_batch_receipt records G-116 when issuing evidence under that waiver.
fn walk_receipts(
    root: &Path,
    task: &Task,
    repo: &Path,
    target: &str,
    head: &str,
    receipts_for_task: fn(&Path, &str, &str) -> cas_store::Result<Vec<QaBatchReceipt>>,
    mut visit: impl FnMut(&QaBatchReceipt, Option<&'static str>) -> bool,
) -> Result<(), String> {
    let project = crate::cloud::resolve_canonical_id(root).ok_or("project identity unavailable")?;
    if !crate::mcp::tools::core::task::task_owned_by_project(task, root, Some(&project)) {
        return Ok(());
    }
    let Some(anchor) = resolved_delivery_anchor(repo, task, target) else {
        return Ok(());
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
        let gap = 'judge: {
            if !release_close::receipt_allowed(root, task, &receipt) { break 'judge Some("RECEIPT_NOT_ALLOWED"); }
            let Some(reviewed) = receipt.task_heads.get(&task.id) else { break 'judge Some("TASK_NOT_LISTED"); };
            let known = *reviewed == anchor || retired.contains(reviewed);
            if receipt.schema_version == cas_store::RELEASE_TAG_VERDICT_SCHEMA {
                let Some(tag) = receipt.release_tag.as_deref() else { break 'judge Some("TAG_UNBOUND"); };
                let tag_ref = format!("refs/tags/{tag}");
                if !known
                    || ![anchor.as_str(), head].contains(&reviewed.as_str())
                    || commit(repo, &tag_ref).as_deref() != Some(receipt.staging_sha.as_str())
                    || release_delivery_effect_for_task(root, repo, task, task.deliverables.factory_branch_start.as_ref().map(|start| start.base_sha.as_str()), reviewed, &receipt.staging_sha, None).is_err()
                {
                    break 'judge Some("TAG_UNBOUND");
                }
                break 'judge None;
            }
            if !known { break 'judge Some("ANCHOR_MISMATCH"); }
            let inside_review = active == Some(reviewed.as_str())
                || head == reviewed
                || ancestor(repo, head, &receipt.staging_sha) == Some(true);
            if !inside_review { break 'judge Some("HEAD_OUTSIDE_REVIEW"); }
            if receipt.project_id != project { break 'judge Some("PROJECT_MISMATCH"); }
            if ancestor(repo, reviewed, head) != Some(true) { break 'judge Some("REVIEWED_NOT_IN_HEAD"); }
            if ancestor(repo, reviewed, &receipt.staging_sha) != Some(true) { break 'judge Some("REVIEWED_NOT_IN_STAGING"); }
            if !(ancestor(repo, head, &receipt.staging_sha) == Some(true)
                || (ancestor(repo, &receipt.staging_sha, head) == Some(true)
                    && integration_state(repo, head, target) == Some(true)))
            {
                break 'judge Some("STAGING_NOT_COVERING_HEAD");
            }
            if integration_state(repo, &receipt.staging_sha, target) != Some(true) {
                break 'judge Some("STAGING_NOT_ON_TARGET");
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
                break 'judge Some("AUTHOR_IN_LINEAGE");
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
            (!ledger_ok).then_some("LEDGER_CHANGED")
        };
        if visit(&receipt, gap) {
            break;
        }
    }
    Ok(())
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

/// The deferred-debt refusal, with the delivery identity, each recorded piece of
/// evidence and why it does not count, and the one-call supervisor close. The
/// `debt:` line is machine-readable: `anchor=` and `head=` are commits,
/// `receipt:<id>=` and `qa_pass:<id>=` carry a gap code (see `walk_receipts`,
/// `pass_gap`), and `supervisor_close=` is the call that clears the debt.
pub(crate) fn debt_refusal(root: &Path, task: &Task, repo: &Path, target: &str, head: Option<&str>) -> String {
    let anchor = resolved_delivery_anchor(repo, task, target);
    let mut evidence = Vec::new();
    if let Some(head) = head {
        let _ = walk_receipts(root, task, repo, target, head, cas_store::qa_batch_receipts_for_task, |receipt, gap| {
            evidence.push(format!("receipt:{}={}", receipt.id, gap.unwrap_or("COVERS")));
            false
        });
        for pass in cas_store::list_qa_passes(root, &task.id).unwrap_or_default() {
            evidence.push(format!("qa_pass:{}={}:{}", pass.id, pass.state, pass_gap(root, task, repo, anchor.as_deref(), &pass)));
        }
    }
    if evidence.is_empty() {
        evidence.push("none".into());
    }
    format!(
        "waiting for batch receipt: {id} deferred evidence at its batch park; a valid batch_receipt listing this delivery anchor or a covering passed independent QA round is required before close\n\
         debt: task={id} anchor={anchor} head={head} evidence={evidence}\n\
         supervisor_close: task action=close id={id} supervisor_override=true reason=\"<why>\"{receipt}",
        id = task.id,
        anchor = anchor.as_deref().unwrap_or("none"),
        head = head.unwrap_or("none"),
        evidence = evidence.join(","),
        receipt = head.map(|head| format!(" commit_receipt={head}")).unwrap_or_default(),
    )
}

/// Why a recorded QA round does not satisfy deferred debt (`COVERS`: it does,
/// so the debt is on the head's integration, not the round).
// gate-audit[author_override]: selector Read-only diagnosis of recorded QA rounds replays operator authorization; no new approval or delivery accepted.
fn pass_gap(root: &Path, task: &Task, repo: &Path, anchor: Option<&str>, pass: &cas_types::QaPass) -> &'static str {
    if pass.state != cas_types::QaPassState::Passed {
        return "QA_PASS_NOT_PASSED";
    }
    if anchor.is_none() || commit(repo, &pass.bound_head).as_deref() != anchor {
        return "QA_PASS_ANCHOR_MISMATCH";
    }
    match pass.reviewer_agent_id.as_deref() {
        Some(reviewer) if author_override(root, task).is_none()
            && !author_is_independent(root, task, repo, reviewer).unwrap_or(true) => "QA_REVIEWER_IN_LINEAGE",
        _ => "COVERS",
    }
}

/// Final transition guard, independent of QA mode, classification, merge
/// outcome and whether the earlier optional evidence gate was reached.
// gate[G-83 close-after-merge as-of]: Deferred batch QA debt (waiting for batch receipt)
// gate-audit[forward]: G-83 Deferred-evidence verdict; final close emits its code.
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
    .ok_or_else(|| debt_refusal(root, task, &repo, &target, None))?;
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
    Err(debt_refusal(root, task, &repo, &target, Some(&head)))
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
        // GH #1713: a structurally valid ledger with a FAIL cell never backs a PASS receipt.
        let failing = format!("schema: 2\ncells=1; PASS=0; FAIL=1; NOT EXERCISED=0\nlabels: fixture=1\n{}\n| M07 | walk | shown | missing | FAIL | fixture | capture.txt | cas-defect |\n", crate::qa_ledger::HEADER);
        std::fs::write(&ledger, &failing).unwrap();
        assert!(crate::cli::qa_ledger::check(&ledger, Some(&owner)).ok);
        let refused = receipt(&failing).unwrap_err();
        assert!(refused.contains("M07") && refused != later, "{refused}");
    }
}
