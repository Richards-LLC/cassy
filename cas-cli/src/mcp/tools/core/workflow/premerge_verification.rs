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

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
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
    let checkout = Path::new(&proof.worktree_root);
    if !checkout.starts_with(root.join("verification-candidates")) {
        return Ok(None);
    }
    let manifest = checkout
        .parent()
        .ok_or("candidate manifest directory missing")?
        .join("candidate.json");
    let candidate: Candidate =
        serde_json::from_slice(&std::fs::read(manifest).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    if candidate.proof != *proof || candidate.task_id != dispatch.task_id {
        return Err("candidate does not match its immutable dispatch proof".into());
    }
    Ok(Some(candidate))
}

impl Candidate {
    fn directory(&self) -> Result<&Path, String> {
        Path::new(&self.proof.worktree_root)
            .parent()
            .ok_or_else(|| "candidate directory missing".into())
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
        let path = candidate.directory()?.join("promotion.json");
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write;
                file.write_all(&serde_json::to_vec(&record).map_err(|error| error.to_string())?)
                    .map_err(|error| error.to_string())?;
                file.sync_all().map_err(|error| error.to_string())
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let previous: Promotion = serde_json::from_slice(
                    &std::fs::read(path).map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
                if previous == record {
                    Ok(())
                } else {
                    Err("candidate promotion identity changed".into())
                }
            }
            Err(error) => Err(error.to_string()),
        }
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

    pub(crate) fn continue_premerge_after_verdict(
        &self,
        dispatch: &VerificationDispatch,
        status: VerificationStatus,
    ) -> Result<String, McpError> {
        let agents = self.open_agent_store()?;
        let owner = agents.get(&dispatch.owner_agent_id).map_err(fail)?;
        let recipient =
            crate::mcp::tools::core::task::lifecycle::supervisor_push::resolve_notice_recipient(
                agents.as_ref(),
                owner.factory_session.as_deref(),
            );
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
        if let Some(dispatch) = latest.as_ref()
            && let Some(candidate) =
                candidate_for_dispatch(&self.cas_root, dispatch).map_err(fail)?
        {
            let verdict = cas_store::get_verification_for_dispatch(&self.cas_root, &dispatch.id)
                .map_err(fail)?;
            if dispatch.state == cas_types::VerificationDispatchState::Resolved
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
                && candidate.source_branch == source
                && candidate.target_branch == target
                && candidate.validate(&self.cas_root, task).is_ok()
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
                return Err(fail(format!(
                    "task {} holds on dispatch {} ({:?}); finish this exact verdict before retrying the merge",
                    task.id, dispatch.id, dispatch.state
                )));
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
        let queue = crate::store::open_prompt_queue_store(&self.cas_root).map_err(fail)?;
        crate::mcp::tools::core::task::lifecycle::supervisor_push::emit_verification_dispatch_handoff(
            &self.cas_root, queue.as_ref(), &dispatch.id, &task.id, &owner,
            dispatch.deadline_at, task.assignee.as_deref().unwrap_or("worker"),
            Some("Review this disposable candidate before publication; PASS continues the merge, never a worker close retry."),
            Some(&candidate.proof.head_commit), None,
        ).map_err(fail)?;
        Err(fail(format!(
            "task {} requires candidate verdict on dispatch {} before merge",
            task.id, dispatch.id
        )))
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
        let key = format!(
            "{:x}",
            Sha256::digest(format!(
                "{}:{source}:{source_sha}:{target}:{target_sha}:{digest}",
                task.id
            ))
        );
        let directory = self.cas_root.join("verification-candidates").join(key);
        let _candidate_lock = crate::worktree::target_lock::lock_delivery_target(
            &self.cas_root,
            repo,
            &format!("candidate:{}", directory.display()),
        )
        .map_err(fail)?;
        let manifest = directory.join("candidate.json");
        if manifest.exists() {
            return serde_json::from_slice(&std::fs::read(manifest).map_err(fail)?).map_err(fail);
        }
        std::fs::create_dir_all(&directory).map_err(fail)?;
        let checkout = directory.join("checkout");
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
                "Cassy task verification candidate",
            ],
        )
        .map_err(fail)?;
        git(
            repo,
            &[
                "worktree",
                "add",
                "--detach",
                checkout
                    .to_str()
                    .ok_or_else(|| fail("non-UTF8 candidate path"))?,
                &candidate_sha,
            ],
        )
        .map_err(fail)?;
        let proof =
            repository_proof::capture_repository_proof_with_anchors(repo, &checkout, Vec::new())
                .map_err(fail)?;
        let candidate = Candidate {
            task_id: task.id.clone(),
            repository: repo.canonicalize().map_err(fail)?,
            source_branch: source.into(),
            source_sha,
            target_branch: target.into(),
            target_sha,
            tree,
            context_digest: digest,
            proof,
            preparation_ms: started.elapsed().as_millis(),
        };
        std::fs::write(manifest, serde_json::to_vec(&candidate).map_err(fail)?).map_err(fail)?;
        let store = self.open_task_store()?;
        let mut current = store.get(&task.id).map_err(fail)?;
        let marker = "PREMERGE CANDIDATE PREPARATION";
        if !current.notes.contains(marker) {
            current.notes.push_str(&format!(
                "\n\n{marker}: {} ms; source={} target={} candidate={} tree={}",
                candidate.preparation_ms,
                candidate.source_sha,
                candidate.target_sha,
                candidate.proof.head_commit,
                candidate.tree
            ));
            store.update(&current).map_err(fail)?;
        }
        Ok(candidate)
    }
}
