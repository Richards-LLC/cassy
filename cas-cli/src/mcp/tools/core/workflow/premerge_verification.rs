//! The existing task verifier reviews an immutable merge candidate before publication.
//! Candidate preparation never holds the integration target lock. Promotion retains
//! the original dispatch and records the exact candidate-to-landed tree mapping.
use std::path::{Path, PathBuf};
use std::process::Command;

use cas_types::{RepositoryProofBoundary, Task, VerificationDispatch, VerificationStatus};
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

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "candidate git {}: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn fail(message: impl std::fmt::Display) -> McpError {
    CasCore::error(
        ErrorCode::INVALID_PARAMS,
        format!("PRE-MERGE VERIFICATION: {message}. No delivery was published."),
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

fn write_once<T: Serialize + serde::de::DeserializeOwned + PartialEq>(
    path: &Path,
    value: &T,
) -> Result<(), String> {
    use std::io::Write;
    let directory = path.parent().ok_or("immutable record directory missing")?;
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
    file.write_all(&serde_json::to_vec(value).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    file.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    match file.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            let previous: T =
                serde_json::from_slice(&std::fs::read(path).map_err(|error| error.to_string())?)
                    .map_err(|error| error.to_string())?;
            if &previous == value {
                Ok(())
            } else {
                Err("immutable candidate record changed".into())
            }
        }
        Err(error) => Err(error.error.to_string()),
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

fn context_digest(root: &Path, task: &Task) -> Result<String, String> {
    let mut hash = Sha256::new();
    hash.update(b"cas-premerge-candidate-v1\0");
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
        match std::fs::read(ledger) {
            Ok(bytes) => hash.update(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
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
    let candidate: Candidate =
        serde_json::from_slice(&std::fs::read(manifest).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let key = candidate_key(
        &candidate.task_id,
        &candidate.repository,
        &candidate.source_branch,
        &candidate.source_sha,
        &candidate.target_branch,
        &candidate.target_sha,
        &candidate.context_digest,
    )?;
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
        let key = candidate_key(
            &self.task_id,
            &self.repository,
            &self.source_branch,
            &self.source_sha,
            &self.target_branch,
            &self.target_sha,
            &self.context_digest,
        )?;
        Ok(self.cas_root.join("verification-candidates").join(key))
    }

    pub(crate) fn validate(&self, root: &Path, task: &Task) -> Result<(), String> {
        if context_digest(root, task)? != self.context_digest
            || git(&self.repository, &["rev-parse", &self.source_branch])? != self.source_sha
            || git(&self.repository, &["rev-parse", &self.target_branch])? != self.target_sha
        {
            return Err(
                "source, target, plan, policy or evidence changed; review the new candidate".into(),
            );
        }
        repository_proof::verify_repository_proof(&self.proof)
            .map_err(|error| error.to_string())?;
        let tree = git(
            &self.repository,
            &[
                "merge-tree",
                "--write-tree",
                &self.target_sha,
                &self.source_sha,
            ],
        )?;
        if tree.lines().next() != Some(self.tree.as_str()) {
            return Err("merge content differs from the approved candidate".into());
        }
        Ok(())
    }
}

impl Approval {
    fn promotion(&self) -> Result<Option<Promotion>, String> {
        let path = self.candidate.directory()?.join("promotion.json");
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let record: Promotion =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        let candidate = &self.candidate;
        if record.dispatch_id != self.dispatch_id
            || record.verdict_id != self.verdict_id
            || record.source_sha != candidate.source_sha
            || record.target_sha != candidate.target_sha
            || record.candidate_tree != candidate.tree
            || git(
                &candidate.repository,
                &["rev-parse", &format!("{}^{{tree}}", record.landed_sha)],
            )? != candidate.tree
        {
            return Err("candidate promotion does not match the approved proof".into());
        }
        git(
            &candidate.repository,
            &[
                "merge-base",
                "--is-ancestor",
                &record.landed_sha,
                &candidate.target_branch,
            ],
        )?;
        Ok(Some(record))
    }

    /// Reconcile an interrupted publication against the reviewed base and tree.
    fn reconcile_promotion(&self) -> Result<bool, String> {
        if self.promotion()?.is_some() {
            return Ok(true);
        }
        let candidate = &self.candidate;
        let commits = git(
            &candidate.repository,
            &[
                "rev-list",
                "--first-parent",
                "--max-count=256",
                &format!("{}..{}", candidate.target_sha, candidate.target_branch),
            ],
        )?;
        for landed in commits.lines() {
            let first_parent = git(
                &candidate.repository,
                &["rev-parse", &format!("{landed}^1")],
            )?;
            if (first_parent == candidate.target_sha || landed == candidate.source_sha)
                && git(
                    &candidate.repository,
                    &["rev-parse", &format!("{landed}^{{tree}}")],
                )? == candidate.tree
            {
                self.promote(landed)?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn validate_for_merge(&self, root: &Path, task: &Task) -> Result<(), String> {
        if let Some(record) = self.promotion()? {
            if git(
                &self.candidate.repository,
                &["rev-parse", &self.candidate.target_branch],
            )? == record.landed_sha
                && task.deliverables.factory_branch_anchor.as_deref()
                    == Some(self.candidate.source_sha.as_str())
            {
                return repository_proof::verify_repository_proof(&self.candidate.proof)
                    .map_err(|error| error.to_string());
            }
        }
        self.candidate.validate(root, task)
    }

    pub(crate) fn promote(&self, landed: &str) -> Result<(), String> {
        let candidate = &self.candidate;
        if git(
            &candidate.repository,
            &["rev-parse", &format!("{landed}^{{tree}}")],
        )? != candidate.tree
        {
            return Err(
                "landed tree differs from the approved candidate; publication refused".into(),
            );
        }
        git(
            &candidate.repository,
            &["merge-base", "--is-ancestor", &candidate.target_sha, landed],
        )?;
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
}

impl CasCore {
    /// Close consumes the original approval through its immutable landed mapping.
    pub(crate) fn verify_premerge_promotion(
        &self,
        task: &Task,
        dispatch: &VerificationDispatch,
        verdict_id: &str,
    ) -> Result<(), McpError> {
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
    ) -> Result<(), McpError> {
        let queue = crate::store::open_prompt_queue_store(&self.cas_root).map_err(fail)?;
        let location = dispatch.repository.as_ref().map(|proof| format!(
            "Review the disposable candidate at {} (head {}). PASS continues this merge; no worker close retry or second verifier is needed.",
            proof.worktree_root, proof.head_commit,
        ));
        crate::mcp::tools::core::task::lifecycle::supervisor_push::emit_candidate_verification_handoff(
            &self.cas_root, queue.as_ref(), dispatch, task.assignee.as_deref().unwrap_or("worker"),
            location.as_deref().unwrap_or("Review the exact candidate bound to this dispatch."),
        ).map_err(fail)?;
        Ok(())
    }

    pub(crate) fn continue_premerge_after_verdict(
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
        let agents = self.open_agent_store()?;
        let rows = agents.list(None).map_err(fail)?;
        let recipient =
            crate::supervisor_notices::resolve(&rows, Some(&dispatch.owner_agent_id), None);
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
                "Task {} dispatch {}: {status}. {action}",
                dispatch.task_id, dispatch.id
            ),
            &format!("{}:{status}", dispatch.id),
        )
        .map_err(fail)?;
        Ok(format!("\n{action}"))
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
        if task.task_type == cas_types::TaskType::Epic
            || task.depth == cas_types::TaskDepth::Light
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
            && (review_tier::tier_for_delivery(task, repo)
                .is_some_and(|tier| tier.tier == review_tier::VerificationTier::Mechanical)
                || review_tier::release_verdict_owns(&self.cas_root, task, repo))
        {
            return Ok(None);
        }
        if !recorded
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
        let digest = context_digest(&self.cas_root, task).map_err(fail)?;
        let repository = repo.canonicalize().map_err(fail)?;
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
        let candidate = self.build_verification_candidate(task, repo, source, target, digest)?;
        candidate.validate(&self.cas_root, task).map_err(fail)?;
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
        self.emit_premerge_review_handoff(task, &dispatch)?;
        Err(fail(format!(
            "task {} requires candidate verdict on dispatch {} before merge",
            task.id, dispatch.id
        )))
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
    ) -> Result<Candidate, McpError> {
        let started = std::time::Instant::now();
        let source_sha = git(repo, &["rev-parse", source]).map_err(fail)?;
        let target_sha = git(repo, &["rev-parse", target]).map_err(fail)?;
        let repository = repo.canonicalize().map_err(fail)?;
        let key = candidate_key(
            &task.id,
            &repository,
            source,
            &source_sha,
            target,
            &target_sha,
            &digest,
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
            let candidate: Candidate =
                serde_json::from_slice(&std::fs::read(manifest).map_err(fail)?).map_err(fail)?;
            if candidate.task_id != task.id
                || candidate.repository != repository
                || candidate.source_branch != source
                || candidate.source_sha != source_sha
                || candidate.target_branch != target
                || candidate.target_sha != target_sha
                || candidate.context_digest != digest
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
        std::fs::create_dir_all(&directory).map_err(fail)?;
        if let Some(dispatch) =
            cas_store::get_latest_verification_dispatch(&self.cas_root, &task.id).map_err(fail)?
            && let Some(proof) = dispatch.repository.as_ref()
            && proof_candidate_key(proof).map_err(fail)?.as_deref()
                == directory.file_name().and_then(|name| name.to_str())
        {
            return Err(fail(
                "candidate manifest missing for an existing dispatch; explicit recovery required",
            ));
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
            .map_err(fail)?
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
