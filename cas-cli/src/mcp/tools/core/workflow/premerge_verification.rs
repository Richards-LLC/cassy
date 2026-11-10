//! The existing task verifier reviews an immutable merge candidate before publication.
//! Candidate preparation never holds the integration target lock. Promotion retains
//! the original dispatch and records the exact candidate-to-landed tree mapping.
use std::path::{Path, PathBuf};
use std::process::Command;

use cas_types::{
    RepositoryProofBoundary, Task, Verification, VerificationDispatch, VerificationProvenance,
    VerificationStatus,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::mcp::tools::core::imports::*;
use crate::mcp::tools::core::task::lifecycle::{repository_proof, review_tier};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub cas_root: PathBuf,
    pub task_id: String,
    pub repository: PathBuf,
    pub source_branch: String,
    pub source_sha: String,
    pub target_branch: String,
    pub target_sha: String,
    pub tree: String,
    pub context_digest: String,
    pub proof: RepositoryProofBoundary,
    pub preparation_ms: u128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherited_supervisor_call: Option<InheritedSupervisorCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct InheritedSupervisorCall {
    pub prior_dispatch_id: String,
    pub question_verification_id: String,
    pub answer_verification_id: String,
    pub question: String,
    pub answer: String,
    pub answer_status: VerificationStatus,
    pub source_sha: String,
    pub prior_target_sha: String,
    pub target_sha: String,
}

fn inherited_supervisor_call_for_candidate(
    task: &Task,
    prior_dispatch: &VerificationDispatch,
    prior_candidate: &Candidate,
    candidate: &Candidate,
    verifications: &[Verification],
) -> Option<InheritedSupervisorCall> {
    if prior_dispatch.state != cas_types::VerificationDispatchState::Resolved
        || prior_dispatch.task_id != task.id
        || candidate.task_id != task.id
        || prior_candidate.task_id != task.id
        || prior_candidate.repository != candidate.repository
        || prior_candidate.source_branch != candidate.source_branch
        || prior_candidate.target_branch != candidate.target_branch
        || prior_candidate.context_digest != candidate.context_digest
        || prior_candidate.source_sha != candidate.source_sha
        || prior_candidate.target_sha == candidate.target_sha
        || task.deliverables.factory_branch_anchor.as_deref()
            != Some(candidate.source_sha.as_str())
        || prior_dispatch.repository.as_ref() != Some(&prior_candidate.proof)
    {
        return None;
    }

    let mut calls = verifications.iter().filter(|verification| {
        verification.dispatch_id.as_deref() == Some(prior_dispatch.id.as_str())
            && verification.provenance == VerificationProvenance::TaskVerifier
            && verification.status == VerificationStatus::SupervisorCall
            && prior_dispatch
                .verifier_agent_id
                .as_deref()
                .is_some_and(|id| verification.agent_id.as_deref() == Some(id))
            && verification.summary.to_ascii_uppercase().contains("NOT EXERCISED")
    });
    let question = calls.next()?;
    if calls.next().is_some() {
        return None;
    }
    let mut answers = verifications.iter().filter(|verification| {
        verification.dispatch_id.as_deref() == Some(prior_dispatch.id.as_str())
            && verification.provenance == VerificationProvenance::SupervisorDirect
            && verification
                .issuer_agent_id
                .as_deref()
                .is_some_and(|id| !id.trim().is_empty())
            && matches!(verification.status, VerificationStatus::Approved | VerificationStatus::Rejected)
            && verification.created_at >= question.created_at
    });
    let answer = answers.next()?;
    if answers.next().is_some() {
        return None;
    }
    Some(InheritedSupervisorCall {
        prior_dispatch_id: prior_dispatch.id.clone(),
        question_verification_id: question.id.clone(),
        answer_verification_id: answer.id.clone(),
        question: question.summary.clone(),
        answer: answer.summary.clone(),
        answer_status: answer.status,
        source_sha: candidate.source_sha.clone(),
        prior_target_sha: prior_candidate.target_sha.clone(),
        target_sha: candidate.target_sha.clone(),
    })
}

#[derive(Debug, Clone)]
pub(crate) struct Approval {
    pub candidate: Candidate,
    pub dispatch_id: String,
    pub verdict_id: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Promotion {
    dispatch_id: String,
    verdict_id: String,
    source_sha: String,
    target_sha: String,
    candidate_tree: String,
    landed_sha: String,
}

mod peer;
mod automatic;
pub(crate) use peer::{PeerReceiptIngested, ingest_peer_receipt};

const CANDIDATE_COMMIT_PREFIX: &str = "CAS_PREMERGE_CANDIDATE_V1 ";

fn proof_candidate_key(proof: &RepositoryProofBoundary) -> Result<Option<String>, String> {
    let message = git(
        Path::new(&proof.repository_root),
        &["show", "-s", "--format=%B", &proof.head_commit],
    )?;
    let Some(key) = message.strip_prefix(CANDIDATE_COMMIT_PREFIX) else {
        return Ok(None);
    };
    if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("candidate context identity is invalid".into());
    }
    Ok(Some(key.to_string()))
}

/// Outputs scripts/regenerate-generated-artifacts.sh commits after a merge.
fn is_regenerated_output(path: &str) -> bool {
    path.starts_with("hub-web/dist/")
        || matches!(
            path,
            "cas-cli/src/builtins/reference-history.json"
                | "cas-cli/src/builtins/codex/agents/task-verifier.toml"
                | ".codex/agents/task-verifier.toml"
        )
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    if args.first() == Some(&"merge-tree") {
        // Candidate preparation precedes the manager's merge preflight.
        crate::worktree::git::ensure_merge_drivers(repo, args.get(2..).unwrap_or_default());
    }
    let _phase = (args.first() == Some(&"merge-tree"))
        .then(|| crate::mcp::tools::service::mutation_receipt::phase("merge-tree"));
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .map_err(|error| format!("PREMERGE_GIT_IO: git at {}: {error}", repo.display()))?;
    if !output.status.success() {
        // cas-c5db: `merge-base --is-ancestor` answers "no" with an empty
        // stderr; name the whole command and its exit so a refusal is actionable.
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            format!("exited with {} and no stderr", output.status)
        } else {
            stderr
        };
        return Err(format!("candidate `git {}`: {detail}", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// cas-d4b7: whitespace-exact, full-context (three-line) patch-id of the
/// change `tree` adds to `target`. Patch-ids ignore hunk headers, so a
/// zero-context id would hide a target edit beside a delivered hunk (`if ok`
/// to `if !ok` two lines away); only full-context identity may rebind. An
/// empty or unreadable patch is an error, never a match.
pub(crate) fn delivered_patch_id(repo: &Path, target: &str, tree: &str) -> Result<String, String> {
    delivered_patch_id_for_paths(repo, target, tree, &[])
}

fn delivered_patch_id_for_paths(repo: &Path, target: &str, tree: &str, paths: &[&str]) -> Result<String, String> {
    use std::io::Write;
    use std::process::Stdio;
    let patch_id = |context: &str| -> Result<String, String> {
        let isolated = |command: &mut Command| {
            command
                .arg("-C")
                .arg(repo)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_COMMON_DIR")
                .env_remove("GIT_INDEX_FILE");
        };
        let mut diff = Command::new("git");
        isolated(&mut diff);
        let diff = diff
            .args(["diff", "--binary", "--no-color", "--no-renames", "--no-ext-diff"])
            .args(["--no-textconv", context, target, tree])
            .arg("--")
            .args(paths)
            .output()
            .map_err(|error| error.to_string())?;
        if !diff.status.success() || diff.stdout.is_empty() {
            return Err(format!("delivered patch {target}..{tree} is empty or unreadable"));
        }
        let mut hash = Command::new("git");
        isolated(&mut hash);
        let mut child = hash
            .args(["patch-id", "--verbatim"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|error| error.to_string())?;
        child
            .stdin
            .take()
            .ok_or("patch-id stdin missing")?
            .write_all(&diff.stdout)
            .map_err(|error| error.to_string())?;
        let output = child.wait_with_output().map_err(|error| error.to_string())?;
        String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .next()
            .filter(|_| output.status.success())
            .map(str::to_string)
            .ok_or_else(|| "git patch-id produced no id".to_string())
    };
    patch_id("-U3")
}

/// Review the fetched published target. An unpublished local merge can be
/// rolled back after a failed push; binding its SHA would invalidate an
/// approval when the published target later advances normally (cas-015e).
/// Local-only repositories retain their existing target fallback.
fn review_target(repo: &Path, target: &str) -> Result<String, String> {
    if let Ok(remote) = git(
        repo,
        &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/origin/{target}^{{commit}}")],
    ) {
        return Ok(remote);
    }
    git(repo, &["rev-parse", target])
}

/// Refresh origin's copy of the target and refuse a diverged target before
/// any candidate review is dispatched; the merge would refuse it afterwards.
fn refresh_review_target(repo: &Path, target: &str) -> Result<(), String> {
    let operations = crate::worktree::GitOperations::new(repo.to_path_buf());
    // Offline keeps the existing local authority; the merge reports it.
    let Ok(Some(remote)) = operations.fetch_merge_target(target) else {
        return Ok(());
    };
    // cas-bfa8: a train created on another machine has no local branch until
    // the merge creates it. Origin is then the only copy, so nothing diverges.
    let Ok(local) = git(
        repo,
        &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{target}^{{commit}}")],
    ) else {
        return Ok(());
    };
    let ordered = |older: &str, newer: &str| {
        git(repo, &["merge-base", "--is-ancestor", older, newer]).is_ok()
    };
    if local != remote && !ordered(&local, &remote) && !ordered(&remote, &local) {
        return Err(format!(
            "TARGET_DIVERGED_FROM_ORIGIN: local {target} at {} and origin/{target} at {} each hold commits the other lacks. No candidate review was dispatched. Reconcile {target} with origin/{target} (`git fetch origin {target}` then `git merge origin/{target}` on {target}), then retry worktree_merge",
            &local[..local.len().min(12)],
            &remote[..remote.len().min(12)],
        ));
    }
    Ok(())
}

fn fail(message: impl std::fmt::Display) -> McpError {
    CasCore::error(
        ErrorCode::INVALID_PARAMS,
        format!("PRE-MERGE VERIFICATION: {message}. Publication or close is held."),
    )
}

fn candidate_key(
    task: &str,
    repo: &Path,
    source: &str,
    source_sha: &str,
    target: &str,
    target_sha: &str,
    context: &str,
) -> Result<String, String> {
    let bytes = serde_json::to_vec(&(
        "premerge-v1",
        task,
        repo,
        source,
        source_sha,
        target,
        target_sha,
        context,
    ))
    .map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn bound_candidate_key(
    task: &str,
    repo: &Path,
    source: &str,
    source_sha: &str,
    target: &str,
    target_sha: &str,
    context: &str,
    inherited: Option<&InheritedSupervisorCall>,
) -> Result<String, String> {
    let base = candidate_key(task, repo, source, source_sha, target, target_sha, context)?;
    let Some(inherited) = inherited else {
        return Ok(base);
    };
    let bytes = serde_json::to_vec(&("premerge-supervisor-call-v1", base, inherited))
        .map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn candidate_manifest_key(candidate: &Candidate) -> Result<String, String> {
    bound_candidate_key(
        &candidate.task_id,
        &candidate.repository,
        &candidate.source_branch,
        &candidate.source_sha,
        &candidate.target_branch,
        &candidate.target_sha,
        &candidate.context_digest,
        candidate.inherited_supervisor_call.as_ref(),
    )
}

fn write_once<T: Serialize + serde::de::DeserializeOwned + PartialEq>(
    path: &Path,
    value: &T,
) -> Result<(), String> {
    use std::io::Write;
    let directory = path.parent().ok_or("immutable record directory missing")?;
    let io_error = |error| format!("PREMERGE_CANDIDATE_RECORD_IO: {}: {error}", path.display());
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(io_error)?;
    file.write_all(&serde_json::to_vec(value).map_err(|error| error.to_string())?)
        .map_err(io_error)?;
    file.as_file()
        .sync_all()
        .map_err(io_error)?;
    match file.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            let previous: T =
                serde_json::from_slice(&std::fs::read(path).map_err(io_error)?)
                    .map_err(|error| format!("PREMERGE_CANDIDATE_RECORD_PARSE: {}: {error}", path.display()))?;
            if &previous == value {
                Ok(())
            } else {
                Err("immutable candidate record changed".into())
            }
        }
        Err(error) => Err(io_error(error.error)),
    }
}

fn task_context(task: &Task) -> serde_json::Value {
    serde_json::json!({
        "id": task.id, "title": task.title, "description": task.description,
        "design": task.design, "acceptance_criteria": task.acceptance_criteria,
        "depth": task.depth, "labels": task.labels, "risk": task.risk,
        "proof_targets": task.proof_targets, "execution_note": task.execution_note,
        "work_target": task.deliverables.work_target,
        "proof": task.deliverables.parked_close_reason,
    })
}

fn hash_plan_context(root: &Path, task: &Task, hash: &mut Sha256) -> Result<(), String> {
    hash.update(serde_json::to_vec(&task_context(task)).map_err(|error| error.to_string())?);
    let tasks = crate::store::open_task_store(root).map_err(|error| error.to_string())?;
    let specs = crate::store::open_spec_store(root).map_err(|error| error.to_string())?;
    let mut plans = vec![task.clone()];
    let mut parent = tasks
        .get_parent_epic(&task.id)
        .map_err(|error| error.to_string())?;
    while let Some(epic) = parent {
        if plans.iter().any(|plan| plan.id == epic.id) {
            return Err("cyclic epic plan ancestry".into());
        }
        parent = tasks
            .get_parent_epic(&epic.id)
            .map_err(|error| error.to_string())?;
        plans.push(epic);
    }
    for plan in plans {
        hash.update(serde_json::to_vec(&task_context(&plan)).map_err(|error| error.to_string())?);
        let mut linked = specs
            .get_for_task(&plan.id)
            .map_err(|error| error.to_string())?;
        linked.sort_by(|a, b| a.id.cmp(&b.id));
        hash.update(serde_json::to_vec(&linked).map_err(|error| error.to_string())?);
    }
    Ok(())
}

fn portable_plan_digest(root: &Path, task: &Task) -> Result<String, String> {
    let mut hash = Sha256::new();
    hash.update(b"cas-peer-premerge-plan-v1\0");
    hash_plan_context(root, task, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn context_digest(root: &Path, task: &Task) -> Result<String, String> {
    let mut hash = Sha256::new();
    hash.update(b"cas-premerge-candidate-v1\0");
    hash_plan_context(root, task, &mut hash)?;
    let config = crate::config::Config::load(root).map_err(|error| error.to_string())?;
    hash.update(format!(
        "{}:{:?}",
        config.verification_enabled(),
        config.qa().mode
    ));
    let paths = crate::config::resolved_factory_artifact_paths(
        root,
        config.factory().artifacts_root.as_deref(),
    );
    for directory in crate::config::factory_task_artifact_dirs(root, &paths.base, &task.id) {
        let ledger = directory.join("LEDGER.md");
        match std::fs::read(&ledger) {
            Ok(bytes) => {
                hash.update(b"ledger\0");
                hash.update((bytes.len() as u64).to_le_bytes());
                hash.update(bytes);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("PREMERGE_CONTEXT_LEDGER_READ: {}: {error}", ledger.display())),
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(crate) fn candidate_for_dispatch(
    root: &Path,
    dispatch: &VerificationDispatch,
) -> Result<Option<Candidate>, String> {
    let Some(proof) = dispatch.repository.as_ref() else {
        return Ok(None);
    };
    let Some(bound_key) = proof_candidate_key(proof)? else {
        return Ok(None);
    };
    let manifest = root
        .join("verification-candidates")
        .join(&bound_key)
        .join("candidate.json");
    let bytes = std::fs::read(&manifest).map_err(|error| format!(
        "VERIFIED_DELIVERY_CANDIDATE_READ: candidate manifest {} for dispatch {} (key {bound_key}): {error}",
        manifest.display(), dispatch.id,
    ))?;
    let candidate: Candidate = serde_json::from_slice(&bytes).map_err(|error| format!(
        "VERIFIED_DELIVERY_CANDIDATE_PARSE: candidate manifest {} for dispatch {} (key {bound_key}): {error}",
        manifest.display(), dispatch.id,
    ))?;
    let key = candidate_manifest_key(&candidate)?;
    if candidate.proof != *proof
        || candidate.task_id != dispatch.task_id
        || candidate.repository.to_string_lossy() != proof.repository_root
        || candidate.cas_root != root
        || bound_key != key
        || git(
            &candidate.repository,
            &["rev-parse", &format!("{}^{{tree}}", proof.head_commit)],
        )? != candidate.tree
    {
        return Err("candidate does not match its immutable dispatch proof".into());
    }
    Ok(Some(candidate))
}

impl Candidate {
    fn directory(&self) -> Result<PathBuf, String> {
        let key = candidate_manifest_key(self)?;
        Ok(self.cas_root.join("verification-candidates").join(key))
    }

    pub(crate) fn validate(&self, root: &Path, task: &Task) -> Result<(), String> {
        if context_digest(root, task)? != self.context_digest
            || git(&self.repository, &["rev-parse", &self.source_branch])? != self.source_sha
        {
            return Err(
                "source, plan, policy or evidence changed; review the new candidate".into(),
            );
        }
        repository_proof::verify_repository_proof(&self.proof)
            .map_err(|error| error.to_string())?;
        let target = review_target(&self.repository, &self.target_branch)?;
        if git(
            &self.repository,
            &["merge-base", "--is-ancestor", &self.source_sha, &target],
        )
        .is_ok()
        {
            self.verify_landing(&target)
        } else {
            self.tree_on(&target).map(|_| ())
        }
    }

    /// A late child may finish only the exact still-current reviewed candidate.
    pub(crate) fn validate_late_verdict(&self, root: &Path, task: &Task) -> Result<(), String> {
        self.validate(root, task)?;
        let target = review_target(&self.repository, &self.target_branch)?;
        if self.tree_on(&target)? != self.tree {
            return Err(
                "late verifier candidate tree changed; review the current candidate".into(),
            );
        }
        Ok(())
    }

    /// Independently reviewed siblings cannot widen this delivery's approval.
    fn tree_on(&self, target: &str) -> Result<String, String> {
        // cas-4af9: the approval covers (source tip, candidate tree). A target
        // move that recomputes the exact reviewed tree, such as a ref fast-
        // forward or a queue merge of commits the source already carries,
        // changes no reviewed content, whatever the path overlap says.
        let merged = git(
            &self.repository,
            &["merge-tree", "--write-tree", target, &self.source_sha],
        )?;
        let tree = merged.lines().next().ok_or("merge tree missing")?.to_string();
        if tree == self.tree {
            return Ok(self.tree.clone());
        }
        git(
            &self.repository,
            &["merge-base", "--is-ancestor", &self.target_sha, target],
        )?;
        let base = git(
            &self.repository,
            &["merge-base", &self.target_sha, &self.source_sha],
        )?;
        // Shared overview additions may touch the same file while leaving
        // every delivered hunk and its context unchanged. Compare the actual
        // clean merge's full-context patch, never waive the whole document.
        let overview = "docs/PRODUCT_OVERVIEW.md";
        let unchanged_overview = delivered_patch_id_for_paths(
            &self.repository, &self.target_sha, &self.tree, &[overview],
        ).ok().zip(delivered_patch_id_for_paths(
            &self.repository, target, &tree, &[overview],
        ).ok()).is_some_and(|(reviewed, current)| reviewed == current);
        let reviewed_changes = |paths: Vec<String>| paths.into_iter()
            .filter(|path| path != overview || !unchanged_overview).collect::<Vec<_>>();
        let changed = reviewed_changes(repository_proof::delivery_paths_changed(
            &self.repository,
            &base,
            &self.source_sha,
            &self.target_sha,
            target,
        )?);
        if !changed.is_empty() {
            return Err(format!(
                "target changed reviewed delivery files: {}; review the new candidate",
                changed.join(", ")
            ));
        }
        let changed = reviewed_changes(repository_proof::delivery_paths_changed(
            &self.repository,
            &base,
            &self.source_sha,
            &self.tree,
            &tree,
        )?);
        if !changed.is_empty() {
            return Err(format!(
                "recomputed merge changed reviewed delivery content: {}",
                changed.join(", ")
            ));
        }
        Ok(tree)
    }

    /// cas-7a8f: worktree_merge may add a regeneration commit
    /// (scripts/regenerate-generated-artifacts.sh) on top of the delivery
    /// merge. The landing is the first-parent commit below any such
    /// single-parent commits that touch only generated outputs.
    fn delivery_landing(&self, landed: &str) -> Result<String, String> {
        let mut landed = landed.to_string();
        while landed != self.source_sha {
            let parents = git(&self.repository, &["rev-list", "--parents", "-n", "1", &landed])?;
            let [_, parent] = parents.split_whitespace().collect::<Vec<_>>()[..] else {
                break;
            };
            let generated_only = git(
                &self.repository,
                &["diff", "--name-only", "--no-renames", parent, &landed],
            )?
            .lines()
            .all(is_regenerated_output);
            if !generated_only
                || git(
                    &self.repository,
                    &["merge-base", "--is-ancestor", &self.source_sha, parent],
                )
                .is_err()
            {
                break;
            }
            landed = parent.to_string();
        }
        Ok(landed)
    }

    fn landing_base(&self, landed: &str) -> Result<String, String> {
        let landed = self.delivery_landing(landed)?;
        let landed = landed.as_str();
        git(
            &self.repository,
            &["merge-base", "--is-ancestor", &self.source_sha, landed],
        )?;
        if landed == self.source_sha {
            return Ok(self.target_sha.clone());
        }
        let parent = git(&self.repository, &["rev-parse", &format!("{landed}^1")])?;
        if git(
            &self.repository,
            &["merge-base", "--is-ancestor", &self.source_sha, &parent],
        )
        .is_ok()
        {
            return Err("this is subsequent target progress, not the delivery landing".into());
        }
        Ok(parent)
    }

    fn verify_landing(&self, landed: &str) -> Result<(), String> {
        let expected = self.tree_on(&self.landing_base(landed)?)?;
        let landed = self.delivery_landing(landed)?;
        if git(
            &self.repository,
            &["rev-parse", &format!("{landed}^{{tree}}")],
        )? != expected
        {
            return Err(
                "landed tree differs from the recomputed approved delivery; publication refused"
                    .into(),
            );
        }
        Ok(())
    }
}

impl Approval {
    fn promotion(&self) -> Result<Option<Promotion>, String> {
        let path = self.candidate.directory()?.join("promotion.json");
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("PREMERGE_PROMOTION_READ: {} for dispatch {}: {error}", path.display(), self.dispatch_id)),
        };
        let record: Promotion = serde_json::from_slice(&bytes).map_err(|error| format!(
            "PREMERGE_PROMOTION_PARSE: {} for dispatch {}: {error}", path.display(), self.dispatch_id,
        ))?;
        let candidate = &self.candidate;
        if record.dispatch_id != self.dispatch_id
            || record.verdict_id != self.verdict_id
            || record.source_sha != candidate.source_sha
            || record.target_sha != candidate.target_sha
            || record.candidate_tree != candidate.tree
        {
            return Err("candidate promotion does not match the approved proof".into());
        }
        candidate.verify_landing(&record.landed_sha)?;
        // The immutable landing was just checked against the approved tree.
        // Later edits on the same target do not undo that published review.
        let reachable = [
            &candidate.target_branch,
            &format!("origin/{}", candidate.target_branch),
        ]
        .iter()
        .any(|target| {
            git(
                &candidate.repository,
                &["merge-base", "--is-ancestor", &record.landed_sha, target],
            )
            .is_ok()
        });
        if !reachable {
            return Err("published candidate is absent from its target history".into());
        }
        Ok(Some(record))
    }

    /// Reconcile an interrupted publication against the reviewed base and tree.
    fn reconcile_promotion(&self) -> Result<bool, String> {
        if self.promotion()?.is_some() {
            return Ok(true);
        }
        let candidate = &self.candidate;
        // A local merge whose push failed is not a landed mapping. With an
        // origin, only fetched remote history can repair an interrupted write.
        let operations = crate::worktree::GitOperations::new(candidate.repository.clone());
        let target = operations
            .fetch_merge_target(&candidate.target_branch)
            .map_err(|error| format!("publication could not be confirmed: {error}"))?
            .unwrap_or_else(|| candidate.target_branch.clone());
        let commits = git(
            &candidate.repository,
            &[
                "rev-list",
                "--first-parent",
                "--max-count=256",
                &format!("{}..{}", candidate.target_sha, target),
            ],
        )?;
        for landed in commits.lines() {
            if self.candidate.verify_landing(landed).is_ok() {
                self.promote(landed)?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn validate_for_merge(&self, root: &Path, task: &Task) -> Result<(), String> {
        let dispatch = cas_store::get_verification_dispatch(root, &self.dispatch_id)
            .map_err(|error| error.to_string())?;
        let verdict = cas_store::get_verification_for_dispatch(root, &self.dispatch_id)
            .map_err(|error| error.to_string())?;
        if dispatch.state != cas_types::VerificationDispatchState::Resolved
            || !verdict.is_some_and(|verdict| {
                verdict.id == self.verdict_id && verdict.status == VerificationStatus::Approved
            })
        {
            return Err("the exact candidate approval is no longer current".into());
        }
        self.candidate.validate(root, task)
    }

    /// Reconciled delivery can close on its original mapping after other
    /// deliveries advance the target; it must not push those changes again.
    pub(crate) fn confirmed_publication(
        &self,
        operations: &crate::worktree::GitOperations,
    ) -> Result<Option<(String, String)>, String> {
        if !self.reconcile_promotion()? {
            return Ok(None);
        }
        let record = self.promotion()?.ok_or("published mapping missing")?;
        let target = operations
            .fetch_merge_target(&self.candidate.target_branch)
            .map_err(|error| error.to_string())?
            .unwrap_or(git(
                &self.candidate.repository,
                &["rev-parse", &self.candidate.target_branch],
            )?);
        git(
            &self.candidate.repository,
            &["merge-base", "--is-ancestor", &record.landed_sha, &target],
        )?;
        self.share_published();
        Ok(Some((record.landed_sha, target)))
    }

    pub(crate) fn verify_landing(&self, landed: &str) -> Result<(), String> {
        self.candidate.verify_landing(landed)
    }

    pub(crate) fn promote(&self, landed: &str) -> Result<(), String> {
        self.verify_landing(landed)?;
        let candidate = &self.candidate;
        let record = Promotion {
            dispatch_id: self.dispatch_id.clone(),
            verdict_id: self.verdict_id.clone(),
            source_sha: candidate.source_sha.clone(),
            target_sha: candidate.target_sha.clone(),
            candidate_tree: candidate.tree.clone(),
            landed_sha: landed.into(),
        };
        write_once(&candidate.directory()?.join("promotion.json"), &record)
    }
    /// Persist only a confirmed landing. An unconfirmed first publication
    /// restores our own target ref, so another lane cannot ship a held source.
    pub(crate) fn finish_publication(
        &self,
        operations: &crate::worktree::GitOperations,
        outcome: &crate::worktree::git::TargetPushOutcome,
        attempted: &str,
    ) -> Result<Option<String>, String> {
        use crate::worktree::git::TargetPushOutcome;
        if outcome.is_published() {
            let landed = match outcome {
                TargetPushOutcome::Pushed { sha } | TargetPushOutcome::AlreadyCurrent { sha } => {
                    sha.as_str()
                }
                _ => attempted,
            };
            if let Some(record) = self.promotion()? {
                git(
                    &self.candidate.repository,
                    &["merge-base", "--is-ancestor", &record.landed_sha, landed],
                )?;
            } else {
                self.promote(landed)?;
            }
            self.share_published();
            return Ok(None);
        }
        if self.promotion()?.is_some() {
            return Ok(Some(
                "Previously published mapping retained; no local history was rewound.".into(),
            ));
        }
        self.verify_landing(attempted)?;
        let attempted_base = self.candidate.landing_base(attempted)?;
        let restored = restore_unpublished_merge(
            operations, &self.candidate.target_branch, &attempted_base, attempted,
        );
        Ok(Some(match restored {
            Ok(()) => format!(
                "Unconfirmed reviewed merge restored locally to {}. Candidate, source and approval retained; fix the push cause and retry.",
                attempted_base
            ),
            Err(error) => format!(
                "Local rollback declined: {error}. Inspect the target before retrying; concurrent history was preserved."
            ),
        }))
    }
}

/// Restore only the merge this publication attempted, retaining unrelated
/// working edits and any concurrent writer. Reviewed and no-dispatch merges
/// use the same compare-and-swap plus merge-path checkout repair.
pub(crate) fn restore_unpublished_merge(
    operations: &crate::worktree::GitOperations,
    target: &str,
    base: &str,
    attempted: &str,
) -> Result<(), String> {
    operations.compare_and_swap_ref(target, base, attempted)
        .map_err(|error| error.to_string())?;
    let checkouts = operations.list_worktrees().map_err(|error| error.to_string())?;
    for checkout in checkouts.iter().filter(|checkout| checkout.branch.as_deref() == Some(target)) {
        git(&checkout.path, &["read-tree", "-u", "-m", attempted, base])?;
    }
    Ok(())
}

impl CasCore {
    /// Close consumes the original approval through its immutable landed mapping.
    pub(crate) fn verify_premerge_promotion(
        &self,
        task: &Task,
        dispatch: &VerificationDispatch,
        verdict_id: &str,
    ) -> Result<(), McpError> {
        if peer::verify_peer_promotion(&self.cas_root, task, dispatch, verdict_id).map_err(fail)? {
            return Ok(());
        }
        let Some(candidate) = candidate_for_dispatch(&self.cas_root, dispatch).map_err(fail)?
        else {
            return Ok(());
        };
        if task.deliverables.factory_branch_anchor.as_deref() != Some(candidate.source_sha.as_str())
        {
            return Err(fail("landed candidate does not match this task delivery"));
        }
        let approval = Approval {
            candidate,
            dispatch_id: dispatch.id.clone(),
            verdict_id: verdict_id.into(),
        };
        if !approval.reconcile_promotion().map_err(fail)? {
            return Err(fail(
                "candidate has no matching landed mapping; close cannot dispatch another verifier",
            ));
        }
        Ok(())
    }

    pub(crate) fn emit_premerge_review_handoff(
        &self,
        task: &Task,
        dispatch: &VerificationDispatch,
    ) -> Result<Option<std::thread::JoinHandle<()>>, McpError> {
        let _phase = crate::mcp::tools::service::mutation_receipt::phase("dispatch");
        if self.daemon.is_some() {
            return match automatic::launch(self, task, dispatch) {
                Ok(handle) => Ok(handle),
                Err(error) => {
                    automatic::attention(self, dispatch, &format!("Automatic verifier could not start: {error}"))
                        .map_err(fail)?;
                    Ok(None)
                }
            };
        }
        let queue = crate::store::open_prompt_queue_store(&self.cas_root).map_err(fail)?;
        let location = dispatch.repository.as_ref().map(|proof| format!(
            "Review the disposable candidate at {} (head {}). PASS continues this merge; no worker close retry or second verifier is needed.",
            proof.worktree_root, proof.head_commit,
        ));
        crate::mcp::tools::core::task::lifecycle::supervisor_push::emit_candidate_verification_handoff(
            &self.cas_root, queue.as_ref(), dispatch, task.assignee.as_deref().unwrap_or("worker"),
            location.as_deref().unwrap_or("Review the exact candidate bound to this dispatch."),
        ).map_err(fail)?;
        Ok(None)
    }

    pub(crate) async fn continue_premerge_after_verdict(
        &self,
        dispatch: &VerificationDispatch,
        status: VerificationStatus,
    ) -> Result<String, McpError> {
        if self
            .open_task_store()?
            .get(&dispatch.task_id)
            .map_err(fail)?
            .is_terminal()
        {
            return Ok(
                "\nThe delivery is already complete; its original approval is retained.".into(),
            );
        }
        if status == VerificationStatus::Approved && automatic::owns(&self.cas_root, dispatch) {
            return automatic::approved(self, dispatch).await;
        }
        let agents = self.open_agent_store()?;
        let rows = agents.list(None).map_err(fail)?;
        let recipient =
            crate::supervisor_notices::resolve(&rows, Some(&dispatch.owner_agent_id), None);
        let verdict = cas_store::get_verification_for_dispatch(&self.cas_root, &dispatch.id).map_err(fail)?;
        let findings = verdict.as_ref().map(|verdict| format!("\n{}\nFindings: {}", verdict.summary,
            serde_json::to_string(&verdict.issues).unwrap_or_default())).unwrap_or_default();
        let action = if status == VerificationStatus::Approved {
            "The existing task verifier approved the candidate. Retry this delivery's worktree_merge; no worker close retry or second verifier is needed."
        } else {
            "This delivery remains unpublished. Resolve this verdict or return only this delivery for repair; unrelated deliveries may continue."
        };
        crate::supervisor_notices::enqueue(
            &self.cas_root,
            &recipient,
            "premerge_verification",
            &format!(
                "Task {} dispatch {}: {status}. {action}{findings}",
                dispatch.task_id, dispatch.id
            ),
            &format!("{}:{status}", dispatch.id),
        )
        .map_err(fail)?;
        Ok(format!("\n{action}"))
    }

    fn inherited_supervisor_call_for_target(
        &self,
        task: &Task,
        repo: &Path,
        source: &str,
        target: &str,
        digest: &str,
        prior_dispatch: &VerificationDispatch,
        prior_candidate: &Candidate,
    ) -> Result<Option<InheritedSupervisorCall>, McpError> {
        let repository = repo.canonicalize().map_err(fail)?;
        let preview = Candidate {
            cas_root: self.cas_root.clone(),
            task_id: task.id.clone(),
            repository,
            source_branch: source.into(),
            source_sha: git(repo, &["rev-parse", source]).map_err(fail)?,
            target_branch: target.into(),
            target_sha: review_target(repo, target).map_err(fail)?,
            tree: String::new(),
            context_digest: digest.into(),
            proof: prior_candidate.proof.clone(),
            preparation_ms: 0,
            inherited_supervisor_call: None,
        };
        let verifications = self
            .open_verification_store()?
            .get_for_task(&task.id)
            .map_err(fail)?;
        Ok(inherited_supervisor_call_for_candidate(
            task,
            prior_dispatch,
            prior_candidate,
            &preview,
            &verifications,
        ))
    }

    /// Return a reusable approval, or dispatch this delivery's existing task
    /// verifier and return a hold. This runs before acquiring the target lock.
    pub(crate) fn prepare_premerge_verification(
        &self,
        task: &Task,
        repo: &Path,
        source: &str,
        target: &str,
    ) -> Result<Option<Approval>, McpError> {
        use crate::harness_policy::{
            supervisor_harness_from_env, verification_policy, worker_harness_from_env,
        };
        let config = self.load_config();
        let fast_lane =
            crate::fast_lane::owns_review(&self.cas_root, task, repo, Some((source, target)));
        if task.task_type == cas_types::TaskType::Epic
            || (task.depth == cas_types::TaskDepth::Light && !fast_lane)
            || !config.verification_enabled()
            || !verification_policy(supervisor_harness_from_env(), worker_harness_from_env())
                .task_required()
        {
            return Ok(None);
        }
        let latest =
            cas_store::get_latest_verification_dispatch(&self.cas_root, &task.id).map_err(fail)?;
        let recorded = latest.is_some()
            || self
                .open_verification_store()?
                .get_latest_for_task(&task.id)
                .map_err(fail)?
                .is_some();
        if !recorded
            && !fast_lane
            && (review_tier::tier_for_delivery(task, repo)
                .is_some_and(|tier| tier.tier == review_tier::VerificationTier::Mechanical)
                || review_tier::release_verdict_owns(&self.cas_root, task, repo))
        {
            return Ok(None);
        }
        if !recorded
            && !fast_lane
            && !crate::rapid_response::verification_sampled(&task.id)
            && crate::rapid_response::active_for_task_at(&self.cas_root, task, None).is_some_and(
                |window| {
                    crate::rapid_response::record_skip(
                        &self.cas_root,
                        &window,
                        &task.id,
                        "verification",
                        "pre-merge verification not sampled (one task in four runs it)",
                    )
                },
            )
        {
            return Ok(None);
        }
        refresh_review_target(repo, target).map_err(fail)?;
        let digest = context_digest(&self.cas_root, task).map_err(fail)?;
        let repository = repo.canonicalize().map_err(|error| {
            fail(format!("could not resolve candidate repository {}: {error}", repo.display()))
        })?;
        let mut inherited_supervisor_call = None;
        if let Some(dispatch) = latest.as_ref()
            && let Some(candidate) =
                candidate_for_dispatch(&self.cas_root, dispatch).map_err(fail)?
        {
            let verdict = cas_store::get_verification_for_dispatch(&self.cas_root, &dispatch.id)
                .map_err(fail)?;
            if dispatch.state == cas_types::VerificationDispatchState::Resolved
                && candidate.repository == repository
                && candidate.source_branch == source
                && candidate.target_branch == target
                && task.deliverables.factory_branch_anchor.as_deref()
                    == Some(candidate.source_sha.as_str())
                && let Some(verdict) = verdict
                    .as_ref()
                    .filter(|verdict| verdict.status == VerificationStatus::Approved)
            {
                let approval = Approval {
                    candidate: candidate.clone(),
                    dispatch_id: dispatch.id.clone(),
                    verdict_id: verdict.id.clone(),
                };
                if approval.reconcile_promotion().map_err(fail)? {
                    return Ok(Some(approval));
                }
            }
            if candidate.context_digest == digest
                && dispatch.state != cas_types::VerificationDispatchState::Invalidated
                && candidate.repository == repository
                && candidate.source_branch == source
                && candidate.target_branch == target
                && (candidate.validate(&self.cas_root, task).is_ok()
                    || (dispatch.delivery_transaction_id.is_some()
                        && repository_proof::verify_repository_proof(&candidate.proof).is_ok()))
            {
                if dispatch.state == cas_types::VerificationDispatchState::Resolved
                    && let Some(verdict) =
                        verdict.filter(|verdict| verdict.status == VerificationStatus::Approved)
                {
                    return Ok(Some(Approval {
                        candidate,
                        dispatch_id: dispatch.id.clone(),
                        verdict_id: verdict.id,
                    }));
                }
                if matches!(
                    dispatch.state,
                    cas_types::VerificationDispatchState::Pending
                        | cas_types::VerificationDispatchState::Claimed
                ) {
                    self.emit_premerge_review_handoff(task, dispatch)?;
                }
                if dispatch.state == cas_types::VerificationDispatchState::TimedOut {
                    return Err(fail(format!(
                        "exact dispatch {} timed out. A registered supervisor must recover it. Re-dispatch task-verifier for task {} on this dispatch, or record its verdict directly: {}verification action=add task_id={} dispatch_id={} status=approved|rejected summary=\"<review and verdict>\". Then the supervisor retries {}factory action=worktree_merge id={} task_id={}. Do not ask the worker to finish an expired review.",
                        dispatch.id,
                        task.id,
                        crate::mcp::tools::core::guidance::supervisor_prefix(),
                        task.id,
                        dispatch.id,
                        crate::mcp::tools::core::guidance::supervisor_prefix(),
                        source,
                        task.id,
                    )));
                }
                return Err(fail(format!(
                    "task {} holds on dispatch {} ({:?}); finish this exact verdict before retrying the merge",
                    task.id, dispatch.id, dispatch.state
                )));
            }
            if dispatch.delivery_transaction_id.is_some() {
                return Err(fail(
                    "transactional candidate drifted; request changes and submit a fresh receipt against the current target before another review",
                ));
            }
            if let Some(approval) =
                self.rebind_unchanged_patch(task, repo, source, target, &digest, dispatch, &candidate)?
            {
                return Ok(Some(approval));
            }
            inherited_supervisor_call = self.inherited_supervisor_call_for_target(
                task,
                repo,
                source,
                target,
                &digest,
                dispatch,
                &candidate,
            )?;
            cas_store::invalidate_verification_dispatch_for_repository_drift(
                &self.cas_root,
                &dispatch.id,
            )
            .map_err(fail)?;
        }
        crate::mcp::tools::core::task::lifecycle::qa_evidence_gate::ledger_preflight(
            &self.cas_root,
            task,
            true,
        )
        .map_err(fail)?;
        let candidate = self.build_verification_candidate(
            task,
            repo,
            source,
            target,
            digest,
            inherited_supervisor_call,
        )?;
        candidate.validate(&self.cas_root, task).map_err(fail)?;
        if let Some(inherited) = &candidate.inherited_supervisor_call {
            let store = self.open_task_store()?;
            let current = store.get(&task.id).map_err(fail)?;
            let marker = format!(
                "INHERITED_SUPERVISOR_CALL: {}:{}",
                inherited.prior_dispatch_id, inherited.answer_verification_id
            );
            if !current.notes.lines().any(|line| line.contains(&marker)) {
                let inherited_json = serde_json::to_string(inherited).map_err(fail)?;
                let note = format!(
                    "[{}] 📝 PROGRESS {marker}; inherited={inherited_json}.",
                    chrono::Utc::now().format("%Y-%m-%d %H:%M"),
                );
                store.append_note(&task.id, &note).map_err(fail)?;
            }
        }
        let requester = self.get_agent_id()?;
        let owner = self.verification_dispatch_owner(&requester)?;
        let dispatch = cas_store::create_verification_dispatch_bound(
            &self.cas_root,
            &task.id,
            &requester,
            &owner,
            &cas_types::VerificationProofBoundary::task_at(candidate.proof.clone()),
            chrono::Utc::now() + chrono::Duration::minutes(10),
            false,
        )
        .map_err(fail)?;
        if self.emit_premerge_review_handoff(task, &dispatch)?.is_some() || automatic::owns(&self.cas_root, &cas_store::get_verification_dispatch(&self.cas_root, &dispatch.id).map_err(fail)?) {
            return Err(fail(format!(
                "task {}: dispatch {} needs approval. The automatic verifier is running on it; retry the merge after its verdict. Do not spawn another verifier",
                task.id, dispatch.id
            )));
        }
        Err(fail(format!(
            "task {}: dispatch {} needs approval. Spawn task-verifier: prompt=\"Verify task {}\"; read `{}verification action=latest task_id={}`",
            task.id, dispatch.id, task.id,
            crate::mcp::tools::core::guidance::supervisor_prefix(), task.id
        )))
    }

    /// cas-d4b7: the target moved under an approved candidate, so the reviewed
    /// tree no longer recomputes. When the source tip, plan context and the
    /// patch the merge adds to the new target are all unchanged, the approval
    /// is rebound to the new candidate with a receipt and no verifier runs.
    /// Any mismatch returns None and the caller dispatches a fresh review.
    #[allow(clippy::too_many_arguments)]
    fn rebind_unchanged_patch(
        &self,
        task: &Task,
        repo: &Path,
        source: &str,
        target: &str,
        digest: &str,
        dispatch: &VerificationDispatch,
        prior: &Candidate,
    ) -> Result<Option<Approval>, McpError> {
        let Some(verdict) = cas_store::get_verification_for_dispatch(&self.cas_root, &dispatch.id)
            .map_err(fail)?
            .filter(|verdict| verdict.status == VerificationStatus::Approved)
        else {
            return Ok(None);
        };
        let repository = repo.canonicalize().map_err(|error| {
            fail(format!("could not resolve candidate repository {}: {error}", repo.display()))
        })?;
        if dispatch.state != cas_types::VerificationDispatchState::Resolved
            || dispatch.receipt_id.is_some()
            || prior.context_digest != digest
            || prior.repository != repository
            || prior.source_branch != source
            || prior.target_branch != target
            || task.deliverables.factory_branch_anchor.as_deref() != Some(prior.source_sha.as_str())
            || git(repo, &["rev-parse", source]).ok().as_deref() != Some(prior.source_sha.as_str())
            || review_target(repo, target).ok().as_deref() == Some(prior.target_sha.as_str())
        {
            return Ok(None);
        }
        let Ok(approved) = delivered_patch_id(repo, &prior.target_sha, &prior.tree) else {
            return Ok(None);
        };
        // The fresh-review path enforces this gate; a rebind may not skip it.
        if crate::mcp::tools::core::task::lifecycle::qa_evidence_gate::ledger_preflight(
            &self.cas_root,
            task,
            true,
        )
        .is_err()
        {
            return Ok(None);
        }
        // cas-d4b7 requires a proven patch match. A conflicting/unbuildable
        // candidate must fall through so the caller invalidates the old approval
        // before the fresh-review path reports the candidate error.
        let Ok(candidate) =
            self.build_verification_candidate(task, repo, source, target, digest.to_string(), None)
        else {
            return Ok(None);
        };
        if candidate.validate(&self.cas_root, task).is_err() {
            return Ok(None);
        }
        let Ok(current) = delivered_patch_id(repo, &candidate.target_sha, &candidate.tree) else {
            return Ok(None);
        };
        if current != approved {
            return Ok(None);
        }
        let matched = format!("full-context patch-id {current}");
        let receipt = format!(
            "VERDICT REBIND: approved verdict {} (dispatch {}) carried to candidate {} with no verifier; source {} unchanged, target {} -> {}, delivered {matched} identical.",
            verdict.id,
            dispatch.id,
            candidate.proof.head_commit,
            candidate.source_sha,
            prior.target_sha,
            candidate.target_sha,
        );
        let requester = self.get_agent_id()?;
        let owner = self.verification_dispatch_owner(&requester)?;
        let (rebound, approval) = cas_store::rebind_approved_verification_dispatch(
            &self.cas_root,
            &dispatch.id,
            &verdict.id,
            &requester,
            &owner,
            &cas_types::VerificationProofBoundary::task_at(candidate.proof.clone()),
            &receipt,
        )
        .map_err(fail)?;
        let store = self.open_task_store()?;
        let mut current = store.get(&task.id).map_err(fail)?;
        let expected = current.clone();
        current.notes.push_str(&format!("\n\n{receipt} Rebound dispatch {}.", rebound.id));
        if let Err(error) = store.update_if_current(&current, &expected) {
            tracing::warn!(task_id = %task.id, %error, "cas-d4b7: verdict rebind receipt note not written");
        }
        Ok(Some(Approval {
            candidate,
            dispatch_id: rebound.id,
            verdict_id: approval.id,
        }))
    }

    pub(crate) fn build_completion_candidate(
        &self,
        task: &Task,
        repo: &Path,
        source: &str,
        target: &str,
    ) -> Result<Candidate, McpError> {
        crate::mcp::tools::core::task::lifecycle::qa_evidence_gate::ledger_preflight(
            &self.cas_root,
            task,
            true,
        )
        .map_err(fail)?;
        let candidate = self.build_verification_candidate(
            task,
            repo,
            source,
            target,
            context_digest(&self.cas_root, task).map_err(fail)?,
            None,
        )?;
        candidate.validate(&self.cas_root, task).map_err(fail)?;
        Ok(candidate)
    }

    fn build_verification_candidate(
        &self,
        task: &Task,
        repo: &Path,
        source: &str,
        target: &str,
        digest: String,
        inherited_supervisor_call: Option<InheritedSupervisorCall>,
    ) -> Result<Candidate, McpError> {
        let started = std::time::Instant::now();
        let source_sha = git(repo, &["rev-parse", source]).map_err(fail)?;
        let target_sha = review_target(repo, target).map_err(fail)?;
        let repository = repo.canonicalize().map_err(|error| {
            fail(format!("could not resolve candidate repository {}: {error}", repo.display()))
        })?;
        let key = bound_candidate_key(
            &task.id,
            &repository,
            source,
            &source_sha,
            target,
            &target_sha,
            &digest,
            inherited_supervisor_call.as_ref(),
        )
        .map_err(fail)?;
        let directory = self.cas_root.join("verification-candidates").join(key);
        let _candidate_lock = crate::worktree::target_lock::lock_delivery_target(
            &self.cas_root,
            repo,
            &format!("candidate:{}", directory.display()),
        )
        .map_err(fail)?;
        let manifest = directory.join("candidate.json");
        if manifest.exists() {
            let bytes = std::fs::read(&manifest).map_err(|error| {
                fail(format!("could not read cached candidate manifest {}: {error}", manifest.display()))
            })?;
            let candidate: Candidate = serde_json::from_slice(&bytes).map_err(|error| {
                fail(format!("could not parse cached candidate manifest {}: {error}", manifest.display()))
            })?;
            if candidate.task_id != task.id
                || candidate.repository != repository
                || candidate.source_branch != source
                || candidate.source_sha != source_sha
                || candidate.target_branch != target
                || candidate.target_sha != target_sha
                || candidate.context_digest != digest
                || candidate.inherited_supervisor_call != inherited_supervisor_call
                || candidate.cas_root != self.cas_root
                || proof_candidate_key(&candidate.proof)
                    .map_err(fail)?
                    .as_deref()
                    != directory.file_name().and_then(|name| name.to_str())
                || candidate.proof.repository_root != repository.to_string_lossy()
                || git(
                    repo,
                    &[
                        "rev-parse",
                        &format!("{}^{{tree}}", candidate.proof.head_commit),
                    ],
                )
                .map_err(fail)?
                    != candidate.tree
            {
                return Err(fail(
                    "cached candidate identity does not match the requested proof",
                ));
            }
            return Ok(candidate);
        }
        std::fs::create_dir_all(&directory).map_err(|error| {
            fail(format!("could not create candidate directory {}: {error}", directory.display()))
        })?;
        if let Some(dispatch) =
            cas_store::get_latest_verification_dispatch(&self.cas_root, &task.id).map_err(fail)?
            && let Some(proof) = dispatch.repository.as_ref()
            && proof_candidate_key(proof).map_err(fail)?.as_deref()
                == directory.file_name().and_then(|name| name.to_str())
        {
            return Err(fail(format!(
                "candidate manifest {} is missing for existing dispatch {}; explicit recovery required",
                manifest.display(), dispatch.id,
            )));
        }
        let merged = git(
            repo,
            &["merge-tree", "--write-tree", &target_sha, &source_sha],
        )
        .map_err(fail)?;
        let tree = merged
            .lines()
            .next()
            .ok_or_else(|| fail("merge candidate tree missing"))?
            .to_string();
        let candidate_sha = git(
            repo,
            &[
                "commit-tree",
                &tree,
                "-p",
                &target_sha,
                "-p",
                &source_sha,
                "-m",
                &format!(
                    "{CANDIDATE_COMMIT_PREFIX}{}",
                    directory
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| fail("candidate key missing"))?
                ),
            ],
        )
        .map_err(fail)?;
        let key = directory
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| fail("candidate key missing"))?;
        git(
            repo,
            &[
                "update-ref",
                &format!("refs/cas/verification-candidates/{key}"),
                &candidate_sha,
            ],
        )
        .map_err(fail)?;
        let checkout = crate::factory_review_scratch::create_candidate(
            &self.cas_root,
            repo,
            &task.id,
            &candidate_sha,
        )
        .map_err(fail)?;
        let mut proof =
            repository_proof::capture_repository_proof_at_target(repo, &candidate_sha, Vec::new())
                .map_err(fail)?;
        // The reviewed object is immutable; the managed checkout is a reading
        // location whose eventual GC cannot rewrite the original verdict.
        proof.worktree_root = checkout
            .path
            .canonicalize()
            .map_err(|error| {
                fail(format!("could not resolve candidate checkout {}: {error}", checkout.path.display()))
            })?
            .to_string_lossy()
            .into_owned();
        let candidate = Candidate {
            cas_root: self.cas_root.clone(),
            task_id: task.id.clone(),
            repository,
            source_branch: source.into(),
            source_sha,
            target_branch: target.into(),
            target_sha,
            tree,
            context_digest: digest,
            proof,
            preparation_ms: started.elapsed().as_millis(),
            inherited_supervisor_call,
        };
        write_once(&manifest, &candidate).map_err(fail)?;
        let store = self.open_task_store()?;
        let mut current = store.get(&task.id).map_err(fail)?;
        let expected = current.clone();
        let marker = "PREMERGE CANDIDATE PREPARATION";
        if !current
            .notes
            .lines()
            .any(|line| line.starts_with(&format!("{marker}:")))
        {
            current.notes.push_str(&format!(
                "\n\n{marker}: {} ms; source={} target={} candidate={} tree={}",
                candidate.preparation_ms,
                candidate.source_sha,
                candidate.target_sha,
                candidate.proof.head_commit,
                candidate.tree
            ));
            store.update_if_current(&current, &expected).map_err(fail)?;
        }
        Ok(candidate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inheritance_candidate(task_id: &str, source: &str, target: &str) -> Candidate {
        let repository = PathBuf::from("/repo");
        let proof = RepositoryProofBoundary {
            peer_premerge: None,
            repository_root: repository.display().to_string(),
            worktree_root: "/repo/candidate".into(),
            head_commit: "candidate-head".into(),
            state_digest: "state-digest".into(),
            target_branch: Some("epic/train".into()),
            anchor_commits: vec![source.into()],
        };
        Candidate {
            cas_root: PathBuf::from("/repo/.cas"),
            task_id: task_id.into(),
            repository,
            source_branch: "factory/worker".into(),
            source_sha: source.into(),
            target_branch: "epic/train".into(),
            target_sha: target.into(),
            tree: "candidate-tree".into(),
            context_digest: "context-digest".into(),
            proof,
            preparation_ms: 0,
            inherited_supervisor_call: None,
        }
    }

    #[test]
    fn carries_an_answer_for_unchanged_delivery_after_target_movement() {
        let now = chrono::Utc::now();
        let mut task = Task::new("cas-inherit-call".into(), "Review".into());
        task.deliverables.factory_branch_anchor = Some("a".repeat(40));
        let prior_candidate = inheritance_candidate(&task.id, &"a".repeat(40), &"b".repeat(40));
        let candidate = inheritance_candidate(&task.id, &"a".repeat(40), &"c".repeat(40));
        let prior_dispatch = VerificationDispatch {
            id: "vdispatch-prior".into(),
            task_id: task.id.clone(),
            receipt_id: None,
            delivery_transaction_id: None,
            repository: Some(prior_candidate.proof.clone()),
            requester_agent_id: "worker".into(),
            owner_agent_id: "supervisor".into(),
            verifier_agent_id: Some("verifier".into()),
            capability_id: None,
            state: cas_types::VerificationDispatchState::Resolved,
            requested_at: now - chrono::Duration::minutes(2),
            deadline_at: now + chrono::Duration::minutes(8),
            resolved_at: Some(now - chrono::Duration::minutes(1)),
            recovery_action: Default::default(),
        };
        let mut question = Verification::new("ver-question".into(), task.id.clone());
        question.status = VerificationStatus::SupervisorCall;
        question.provenance = VerificationProvenance::TaskVerifier;
        question.agent_id = Some("verifier".into());
        question.dispatch_id = Some(prior_dispatch.id.clone());
        question.summary = "SUPERVISOR CALL: 1 NOT EXERCISED row: HUB-J26".into();
        question.created_at = now - chrono::Duration::minutes(2);
        let mut answer = Verification::new("ver-answer".into(), task.id.clone());
        answer.status = VerificationStatus::Approved;
        answer.provenance = VerificationProvenance::SupervisorDirect;
        answer.issuer_agent_id = Some(prior_dispatch.owner_agent_id.clone());
        answer.dispatch_id = Some(prior_dispatch.id.clone());
        answer.summary = "Supervisor answer payload".into();
        answer.created_at = now - chrono::Duration::minutes(1);

        let inherited = inherited_supervisor_call_for_candidate(
            &task,
            &prior_dispatch,
            &prior_candidate,
            &candidate,
            &[question.clone(), answer.clone()],
        )
        .expect("target-only movement should carry the same row's answer");
        assert_eq!(inherited.prior_dispatch_id, prior_dispatch.id);
        assert_eq!(inherited.question_verification_id, question.id);
        assert_eq!(inherited.answer_verification_id, answer.id);
        assert_eq!(inherited.question, question.summary);
        assert_eq!(inherited.answer, answer.summary);

        let provenances = [
            VerificationProvenance::Legacy,
            VerificationProvenance::TaskVerifier,
            VerificationProvenance::SupervisorDirect,
            VerificationProvenance::System,
            VerificationProvenance::PeerAttested,
        ];
        for call_provenance in provenances {
            for answer_provenance in provenances {
                let mut pair = [question.clone(), answer.clone()];
                pair[0].provenance = call_provenance;
                pair[1].provenance = answer_provenance;
                assert_eq!(
                    inherited_supervisor_call_for_candidate(
                        &task,
                        &prior_dispatch,
                        &prior_candidate,
                        &candidate,
                        &pair,
                    )
                    .is_some(),
                    call_provenance == VerificationProvenance::TaskVerifier
                        && answer_provenance == VerificationProvenance::SupervisorDirect,
                    "call={call_provenance:?}, answer={answer_provenance:?}",
                );
            }
        }

        let statuses = [
            VerificationStatus::Approved,
            VerificationStatus::Rejected,
            VerificationStatus::SupervisorCall,
            VerificationStatus::Pending,
            VerificationStatus::Skipped,
            VerificationStatus::Error,
        ];
        for question_status in statuses {
            for answer_status in statuses {
                let mut pair = [question.clone(), answer.clone()];
                pair[0].status = question_status;
                pair[1].status = answer_status;
                assert_eq!(
                    inherited_supervisor_call_for_candidate(
                        &task,
                        &prior_dispatch,
                        &prior_candidate,
                        &candidate,
                        &pair,
                    )
                    .is_some(),
                    question_status == VerificationStatus::SupervisorCall
                        && matches!(answer_status, VerificationStatus::Approved | VerificationStatus::Rejected),
                    "question={question_status:?}, answer={answer_status:?}",
                );
            }
        }

        for verifier_id in [Some("verifier"), Some("other-verifier"), None] {
            let mut question_variant = question.clone();
            question_variant.agent_id = verifier_id.map(str::to_string);
            assert_eq!(
                inherited_supervisor_call_for_candidate(
                    &task,
                    &prior_dispatch,
                    &prior_candidate,
                    &candidate,
                    &[question_variant, answer.clone()],
                )
                .is_some(),
                verifier_id == Some("verifier"),
                "verifier={verifier_id:?}",
            );
        }

        for issuer_id in [Some("supervisor"), Some("other-supervisor"), Some(""), None] {
            let mut answer_variant = answer.clone();
            answer_variant.issuer_agent_id = issuer_id.map(str::to_string);
            assert_eq!(
                inherited_supervisor_call_for_candidate(
                    &task,
                    &prior_dispatch,
                    &prior_candidate,
                    &candidate,
                    &[question.clone(), answer_variant],
                )
                .is_some(),
                issuer_id.is_some_and(|id| !id.trim().is_empty()),
                "issuer={issuer_id:?}",
            );
        }

        let mut wrong_dispatch_answer = answer.clone();
        wrong_dispatch_answer.dispatch_id = Some("vdispatch-other".into());
        assert!(inherited_supervisor_call_for_candidate(
            &task,
            &prior_dispatch,
            &prior_candidate,
            &candidate,
            &[question.clone(), wrong_dispatch_answer],
        )
        .is_none());

        let changed_candidate = inheritance_candidate(&task.id, &"d".repeat(40), &"c".repeat(40));
        task.deliverables.factory_branch_anchor = Some(changed_candidate.source_sha.clone());
        assert!(inherited_supervisor_call_for_candidate(
            &task,
            &prior_dispatch,
            &prior_candidate,
            &changed_candidate,
            &[question, answer],
        )
        .is_none(), "changed delivery content must require a fresh supervisor decision");
    }

    pub(super) fn run(repo: &Path, args: &[&str]) -> String {
        git(repo, &["-c", "user.name=t", "-c", "user.email=t@example.invalid"].iter().chain(args).copied().collect::<Vec<_>>()).unwrap()
    }

    pub(super) fn commit_file(repo: &Path, path: &str, body: &str, message: &str) -> String {
        let file = repo.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, body).unwrap();
        run(repo, &["add", "-A"]);
        run(repo, &["commit", "-q", "-m", message]);
        run(repo, &["rev-parse", "HEAD"])
    }

    fn parked_candidate(repo: &Path, source: &str, target: &str) -> Candidate {
        let source_sha = run(repo, &["rev-parse", source]);
        let tree = run(repo, &["merge-tree", "--write-tree", target, source])
            .lines().next().unwrap().to_string();
        Candidate {
            cas_root: repo.to_path_buf(), task_id: source.into(), repository: repo.to_path_buf(),
            source_branch: source.into(), source_sha, target_branch: "main".into(),
            target_sha: target.into(), tree, context_digest: String::new(),
            proof: RepositoryProofBoundary {
                peer_premerge: None, repository_root: String::new(), worktree_root: String::new(),
                head_commit: String::new(), state_digest: String::new(), target_branch: None,
                anchor_commits: Vec::new(),
            },
            inherited_supervisor_call: None,
            preparation_ms: 0,
        }
    }

    #[test]
    fn shared_upkeep_keeps_back_to_back_candidate_approvals_cas_e79d() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        run(repo, &["init", "-q", "-b", "main"]);
        let body = (0..24).map(|n| format!("section {n}\n")).collect::<String>();
        for path in [".claude/CODEMAP.md", "docs/PRODUCT_OVERVIEW.md",
            "cas-cli/src/builtins/reference-history.json"]
        {
            commit_file(repo, path, &body, "base upkeep");
        }
        let base = run(repo, &["rev-parse", "HEAD"]);
        for (branch, marker) in [("factory/a", 2), ("factory/b", 20)] {
            run(repo, &["checkout", "-q", "-b", branch, &base]);
            for path in [".claude/CODEMAP.md", "docs/PRODUCT_OVERVIEW.md",
                "cas-cli/src/builtins/reference-history.json"]
            {
                let edited = body.replace(&format!("section {marker}\n"),
                    &format!("section {marker}\nadded by {branch}\n"));
                commit_file(repo, path, &edited, "delivery upkeep");
            }
        }
        let first = parked_candidate(repo, "factory/a", &base);
        let second = parked_candidate(repo, "factory/b", &base);
        run(repo, &["checkout", "-q", "main"]);
        for candidate in [&first, &second] {
            let target = run(repo, &["rev-parse", "main"]);
            let tree = candidate.tree_on(&target).expect("unchanged reviewed doc hunks keep approval");
            run(repo, &["merge", "-q", "--no-ff", &candidate.source_branch, "-m", "land delivery"]);
            let landed = run(repo, &["rev-parse", "main"]);
            assert_eq!(run(repo, &["rev-parse", "main^{tree}"]), tree);
            assert_eq!(candidate.verify_landing(&landed), Ok(()));
        }
        let overview = std::fs::read_to_string(repo.join("docs/PRODUCT_OVERVIEW.md")).unwrap();
        assert!(overview.contains("added by factory/a") && overview.contains("added by factory/b"));
    }

    #[test]
    fn overlapping_code_still_holds_candidate_cas_e79d() {
      for path in ["src/lib.rs", "docs/PRODUCT_OVERVIEW.md"] {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        run(repo, &["init", "-q", "-b", "main"]);
        let base = commit_file(repo, path, "first\nsecond\nthird\nfourth\nfifth\nlast\n", "base");
        run(repo, &["checkout", "-q", "-b", "factory/a"]);
        commit_file(repo, path, "delivered\nsecond\nthird\nfourth\nfifth\nlast\n", "delivery");
        let candidate = parked_candidate(repo, "factory/a", &base);
        run(repo, &["checkout", "-q", "main"]);
        let target = commit_file(repo, path, "first\nsecond\nthird\nchanged\nfifth\nlast\n", "sibling edit");
        assert!(git(repo, &["merge-tree", "--write-tree", &target, &candidate.source_sha]).is_ok(),
            "the review guard must hold even a clean merge when reviewed context changed");
        assert!(candidate.tree_on(&target).is_err());
        assert_eq!(run(repo, &["rev-parse", "main"]), target);
      }
    }

    /// cas-bfa8: a train created on another machine exists only on origin
    /// until the first merge into it. Its first candidate must resolve.
    #[test]
    fn origin_only_train_resolves_on_the_first_candidate_cas_bfa8() {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin.git");
        let seed = dir.path().join("seed");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&seed).unwrap();
        run(&seed, &["init", "-q", "-b", "main"]);
        commit_file(&seed, "README.md", "base\n", "base");
        run(&seed, &["clone", "-q", "--bare", seed.to_str().unwrap(), origin.to_str().unwrap()]);
        run(&seed, &["clone", "-q", origin.to_str().unwrap(), repo.to_str().unwrap()]);
        // The other machine creates the train after this clone was made.
        run(&seed, &["remote", "add", "origin", origin.to_str().unwrap()]);
        let train = commit_file(&seed, "train.md", "train\n", "train");
        run(&seed, &["push", "-q", "origin", "HEAD:refs/heads/epic/train"]);
        refresh_review_target(&repo, "epic/train").unwrap();
        assert_eq!(review_target(&repo, "epic/train").unwrap(), train);
        // A local train that diverged from origin is still refused.
        run(&repo, &["switch", "-q", "-c", "epic/train", "main"]);
        commit_file(&repo, "local.md", "local\n", "local");
        let refused = refresh_review_target(&repo, "epic/train").unwrap_err();
        assert!(refused.starts_with("TARGET_DIVERGED_FROM_ORIGIN"), "{refused}");
    }

    #[tokio::test]
    async fn premerge_rejection_notice_carries_findings_cas_381f() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        env.set("CAS_ROOT", &root);
        let core = CasCore::with_daemon(root.clone(), None, None);
        let owner = cas_types::Agent::new_with_role("owner".into(), "supervisor".into(), cas_types::AgentRole::Supervisor);
        core.open_agent_store().unwrap().register(&owner).unwrap();
        let task = Task::new("cas-381f-fixture".into(), "Review".into());
        core.open_task_store().unwrap().add(&task).unwrap();
        let dispatch = cas_store::create_verification_dispatch(&root, &task.id, "worker", &owner.id,
            chrono::Utc::now() + chrono::Duration::minutes(10)).unwrap();
        let mut verdict = cas_types::Verification::new("verdict".into(), task.id.clone());
        verdict.status = VerificationStatus::Rejected;
        verdict.provenance = cas_types::VerificationProvenance::System;
        verdict.summary = "REJECTED: candidate loses the operator draft".into();
        verdict.dispatch_id = Some(dispatch.id.clone());
        cas_store::add_system_verification(&root, &verdict).unwrap();
        core.continue_premerge_after_verdict(&dispatch, verdict.status).await.unwrap();
        let prompts = crate::store::open_prompt_queue_store(&root).unwrap().peek_all(10).unwrap();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].prompt.contains(&verdict.summary), "supervisor needs the actual findings: {:?}", prompts[0]);
    }

    #[test]
    fn landing_verifies_past_a_regenerated_artifact_commit_cas_7a8f() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        run(repo, &["init", "-q", "-b", "main"]);
        let target = commit_file(repo, "README.md", "base\n", "base");
        run(repo, &["checkout", "-q", "-b", "factory/w"]);
        let source = commit_file(repo, "cas-cli/src/builtins/skills/s.md", "new\n", "delivery");
        run(repo, &["checkout", "-q", "main"]);
        run(repo, &["merge", "-q", "--no-ff", "-m", "merge delivery", "factory/w"]);
        let regenerated = commit_file(
            repo,
            "cas-cli/src/builtins/reference-history.json",
            "{}\n",
            "build(builtins): regenerate reference-history from merged history",
        );
        let tree = run(repo, &["rev-parse", &format!("{source}^{{tree}}")]);
        let candidate = Candidate {
            cas_root: repo.to_path_buf(),
            task_id: "cas-7a8f".into(),
            repository: repo.to_path_buf(),
            source_branch: "factory/w".into(),
            source_sha: source,
            target_branch: "main".into(),
            target_sha: target,
            tree,
            context_digest: String::new(),
            proof: RepositoryProofBoundary {
                peer_premerge: None,
                repository_root: String::new(),
                worktree_root: String::new(),
                head_commit: String::new(),
                state_digest: String::new(),
                target_branch: None,
                anchor_commits: Vec::new(),
            },
            preparation_ms: 0,
            inherited_supervisor_call: None,
        };
        assert_eq!(candidate.verify_landing(&regenerated), Ok(()));

        // Adversarial probe: an ordinary commit on top is still later target
        // progress, never a landing of this delivery.
        let later = commit_file(repo, "README.md", "edited\n", "unrelated");
        assert_eq!(
            candidate.verify_landing(&later),
            Err("this is subsequent target progress, not the delivery landing".to_string())
        );
    }
    #[test]
    fn candidate_regenerates_dist_like_worktree_merge_cas_e4b9() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        run(repo, &["init", "-q", "-b", "main"]);
        run(repo, &["config", "user.name", "Fixture"]);
        run(repo, &["config", "user.email", "fixture@example.invalid"]);
        commit_file(repo, ".gitignore", ".cas/\n", "state");
        commit_file(repo, ".gitattributes", "hub-web/dist/** merge=cas-generated\n", "attributes");
        commit_file(repo, "hub-web/src/a", "a0\n", "source a");
        commit_file(repo, "hub-web/src/b", "b0\n", "source b");
        commit_file(repo, "hub-web/dist/app.js", "a0\nb0\n", "bundle");
        commit_file(repo, crate::worktree::git::REGENERATE_ARTIFACTS_SCRIPT,
            "#!/usr/bin/env bash\nset -euo pipefail\n\
             git diff --quiet \"$1\" \"$2\" -- hub-web/src && exit 0\n\
             cat hub-web/src/a hub-web/src/b > hub-web/dist/app.js\n\
             if [ -n \"$(git status --porcelain -- hub-web/dist)\" ]; then\n\
               git add hub-web/dist\n\
               git commit -q -m 'build(hub-web): regenerate dist from merged sources'\n\
             fi\n", "generator");
        run(repo, &["checkout", "-q", "-b", "factory/w"]);
        commit_file(repo, "hub-web/src/b", "b1\n", "delivery source");
        commit_file(repo, "hub-web/dist/app.js", "a0\nb1\n", "delivery bundle");
        run(repo, &["checkout", "-q", "main"]);
        commit_file(repo, "hub-web/src/a", "a1\n", "target source");
        let target = commit_file(repo, "hub-web/dist/app.js", "a1\nb0\n", "target bundle");
        let root = crate::store::init_cas_dir(repo).unwrap();
        env.set("CAS_ROOT", &root);
        let core = CasCore::with_daemon(root.clone(), None, None);
        let task = Task::new("cas-e4b9-fixture".into(), "Review".into());
        core.open_task_store().unwrap().add(&task).unwrap();
        let candidate = core.build_verification_candidate(
            &task, repo, "factory/w", "main", "fixture".into(), None,
        ).unwrap();
        let reviewed = run(repo, &["show", &format!("{}:hub-web/dist/app.js", candidate.proof.head_commit)]);
        assert_eq!(reviewed, "a1\nb1", "review must execute the bundle built from both merged inputs");
        assert_eq!(run(repo, &["rev-parse", "main"]), target);
        assert_eq!(run(repo, &["status", "--porcelain"]), "");
        assert_eq!(candidate.tree_on(&target).unwrap(), candidate.tree);
        let moved = commit_file(repo, "README.md", "unrelated\n", "target moved again");
        candidate.tree_on(&moved).unwrap();
        let operations = crate::worktree::GitOperations::new(repo.to_path_buf());
        let landed = operations.merge_branch("main", "factory/w", true).unwrap().unwrap();
        assert_eq!(run(repo, &["show", &format!("{landed}:hub-web/dist/app.js")]), reviewed);
        assert_eq!(candidate.verify_landing(&landed), Ok(()));
        // Rebuild the same original target to compare the complete tree, too.
        run(repo, &["checkout", "-q", "-b", "original-target", &target]);
        let exact = operations.merge_branch("original-target", "factory/w", true).unwrap().unwrap();
        assert_eq!(run(repo, &["rev-parse", &format!("{exact}^{{tree}}")]), candidate.tree);
        // A broken generator cannot publish a stale review checkout.
        run(repo, &["checkout", "-q", "factory/w"]);
        commit_file(repo, crate::worktree::git::REGENERATE_ARTIFACTS_SCRIPT,
            "#!/usr/bin/env bash\nexit 3\n", "broken generator");
        assert!(core.build_verification_candidate(
            &task, repo, "factory/w", "original-target", "failure".into(), None,
        ).is_err());
    }

    /// cas-ded57: replay parallel top-of-map insertions through factory's manager,
    /// including its merge-tree preflight, before the real merge installs a driver.
    #[test]
    fn worktree_merge_parallel_journey_families_cas_ded57() {
        use crate::worktree::{WorktreeConfig, WorktreeError, WorktreeManager};
        use crate::worktree::git::GitError;
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        run(repo, &["init", "-q", "-b", "main"]);
        run(repo, &["config", "user.name", "Fixture"]);
        run(repo, &["config", "user.email", "fixture@example.invalid"]);
        commit_file(repo, ".gitignore", ".cas/\n", "fixture state");
        let config = WorktreeConfig {
            auto_merge: true,
            base_path: repo.join(".cas/worker-fixtures").display().to_string(),
            ..Default::default()
        };
        let mut manager = WorktreeManager::new(repo, config).unwrap();
        let path = "docs/qa/journeys.yaml";
        std::fs::create_dir_all(repo.join("docs/qa")).unwrap();
        commit_file(repo, ".gitattributes", "docs/qa/journeys.yaml merge=cas-journeys\n", "attributes");
        let base = "version: 1\nupdated: '2026-11-07'\nenforce: enforce\nbug_families:\n  existing:\n    tasks: [cas-base]\njourneys: []\n";
        commit_file(repo, path, base, "registry");
        let target = manager.create_epic_branch("Journey Merge").unwrap();
        manager.git().checkout(&target).unwrap();
        // Every worker forks the same tip, before either has landed.
        let mut lanes = Vec::new();
        for (name, date, body) in [
            ("one", "2026-11-08", base.replace("bug_families:\n", "bug_families:\n  one:\n    tasks: [cas-one]\n")),
            ("two", "2026-11-09", base.replace("bug_families:\n", "bug_families:\n  two:\n    tasks: [cas-two]\n")),
            ("same-a", "2026-11-08", base.replace("cas-base", "cas-a").replace("enforce: enforce", "enforce: warn")),
            ("same-b", "2026-11-09", base.replace("cas-base", "cas-b")),
            ("policy", "2026-11-09", base.replace("enforce: enforce", "enforce: off")),
        ] {
            let lane = manager.create_for_worker(name).unwrap();
            commit_file(&lane.path, path, &body.replace("2026-11-07", date), name);
            lanes.push(lane);
        }
        // A non-queued delivery builds its candidate before manager preflight.
        git(repo, &["merge-tree", "--write-tree", &lanes[0].branch, &lanes[1].branch])
            .expect("candidate preparation installs the registry driver too");
        for lane in &mut lanes[..2] {
            // Independently prove manager preflight reinstalls the driver,
            // rather than inheriting the candidate preparation's config.
            run(repo, &["config", "--remove-section", "merge.cas-journeys"]);
            manager.merge_and_cleanup(lane, false, false).expect("disjoint families merge");
        }
        // Load through the actual registry loader, rather than just looking for text.
        let registry = crate::qa_registry::load(&repo.join(path)).unwrap();
        assert_eq!(registry.updated.as_deref(), Some("2026-11-09"));
        assert_eq!(registry.bug_families.len(), 3);
        for key in ["one", "two", "existing"] {
            assert!(registry.bug_families.contains_key(key));
        }
        manager.merge_and_cleanup(&mut lanes[2], false, false).unwrap();
        let before = manager.git().ref_sha(&target).unwrap();
        for lane in &mut lanes[3..] {
            let error = manager.merge_and_cleanup(lane, false, false).unwrap_err();
            assert!(matches!(error, WorktreeError::Git(GitError::MergeConflictPaths(ref paths))
                if paths == &[path.to_string()]), "{error:?}");
            assert_eq!(manager.git().ref_sha(&target).unwrap(), before);
            assert!(!manager.git().merge_in_progress());
        }
    }

}
