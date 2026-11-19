//! Administrative retargeting can carry an approval, never rewrite its verdict.
use super::*;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Authorization {
    dispatch_id: String,
    verdict_id: String,
    target: cas_types::WorkTarget,
    scope: String,
    cycle: u32,
}

#[derive(Serialize, Deserialize)]
struct Mapping {
    authorization: Authorization,
    candidate_key: String,
}

/// Routing through a release train is not a new feature plan. Keep individual
/// task/spec/policy semantics, excluding close prose and the routing target.
fn scope(root: &Path, task: &Task) -> Result<String, String> {
    let tasks = crate::store::open_task_store(root).map_err(|e| e.to_string())?;
    let specs = crate::store::open_spec_store(root).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut current = Some(task.clone());
    let mut seen = std::collections::HashSet::new();
    while let Some(plan) = current {
        if !seen.insert(plan.id.clone()) { return Err("cyclic carry-over plan".into()); }
        if !crate::mcp::tools::core::task::release_train::is_release_train(&plan) {
            let mut context = task_context(&plan);
            context.as_object_mut().unwrap().remove("work_target");
            context.as_object_mut().unwrap().remove("proof");
            hash.update(serde_json::to_vec(&context).map_err(|e| e.to_string())?);
            let mut linked = specs.get_for_task(&plan.id).map_err(|e| e.to_string())?;
            linked.sort_by(|a,b| a.id.cmp(&b.id));
            hash.update(serde_json::to_vec(&linked).map_err(|e| e.to_string())?);
        }
        current = tasks.get_parent_epic(&plan.id).map_err(|e| e.to_string())?;
    }
    let config = crate::config::Config::load(root).map_err(|e| e.to_string())?;
    hash.update(format!("{}:{:?}", config.verification_enabled(), config.qa().mode));
    Ok(format!("{:x}", hash.finalize()))
}

fn authorization_path(candidate: &Candidate, target: &cas_types::WorkTarget, scope: &str) -> Result<PathBuf,String> {
    let key = Sha256::digest(serde_json::to_vec(&(target, scope)).map_err(|e| e.to_string())?);
    Ok(candidate.directory()?.join(format!("retarget-{key:x}.json")))
}

fn verdict(root: &Path, dispatch: &VerificationDispatch) -> Result<Option<Verification>,String> {
    Ok(cas_store::get_verification_for_dispatch(root, &dispatch.id).map_err(|e| e.to_string())?
        .filter(|v| v.task_id == dispatch.task_id && v.status == VerificationStatus::Approved
            && v.verification_type == cas_types::VerificationType::Task
            && matches!(v.provenance, cas_types::VerificationProvenance::TaskVerifier | cas_types::VerificationProvenance::SupervisorDirect)))
}

impl CasCore {
    /// Called only by the authenticated, target-only proof_scope_fix path.
    /// This intent grants nothing until a clean merge and equal patch are proven.
    pub(crate) fn authorize_approved_retarget(&self, task: &Task, target: &cas_types::WorkTarget) -> Result<(),McpError> {
        let Some(dispatch) = cas_store::get_latest_verification_dispatch(&self.cas_root, &task.id).map_err(fail)? else { return Ok(()) };
        if dispatch.state != cas_types::VerificationDispatchState::Resolved { return Ok(()); }
        let Some(verdict) = verdict(&self.cas_root, &dispatch).map_err(fail)? else { return Ok(()) };
        let Some(original) = original_candidate_for_dispatch(&self.cas_root, &dispatch).map_err(fail)? else { return Ok(()) };
        let context = crate::mcp::tools::core::task::repo_context::resolve_repo_context(&self.cas_root, target).map_err(fail)?;
        if context.repo_root.canonicalize().map_err(fail)? != original.repository
            || target.target_branch == original.target_branch { return Ok(()); }
        repository_proof::verify_repository_proof(&original.proof).map_err(fail)?;
        let authorization = Authorization { dispatch_id:dispatch.id, verdict_id:verdict.id,
            target:target.clone(), scope:scope(&self.cas_root, task).map_err(fail)?, cycle:task.deliverables.delivery_cycle };
        // Write before the store correction: a failed correction leaves the old
        // WorkTarget, which cannot consume this target-specific intent.
        write_once(&authorization_path(&original, target, &authorization.scope).map_err(fail)?, &authorization).map_err(fail)
    }
}

/// Every first-parent commit after the approved anchor must be an automatic
/// two-parent merge from this target's history, with no resolution/new content.
fn equivalent(original: &Candidate, current: &Candidate) -> Result<bool,String> {
    let repo = &current.repository;
    if original.repository != *repo || original.task_id != current.task_id
        || original.source_branch != current.source_branch { return Ok(false); }
    if regenerated_merge_tree(repo, &current.target_sha, &current.source_sha)? != current.tree { return Ok(false); }
    if git(repo, &["merge-base", "--is-ancestor", &original.source_sha, &current.source_sha]).is_err() { return Ok(false); }
    let tail = git(repo, &["rev-list", "--first-parent", "--parents", &format!("{}..{}", original.source_sha, current.source_sha)])?;
    let mut expected = current.source_sha.as_str();
    for line in tail.lines() {
        let parents = line.split_whitespace().collect::<Vec<_>>();
        if parents.len() != 3 || parents[0] != expected
            || git(repo, &["merge-base", "--is-ancestor", parents[2], &current.target_sha]).is_err()
        { return Ok(false); }
        let Ok(tree) = git(repo, &["merge-tree", "--write-tree", parents[1], parents[2]]) else { return Ok(false) };
        if tree.lines().next() != Some(git(repo, &["rev-parse", &format!("{}^{{tree}}", parents[0])])?.as_str()) { return Ok(false); }
        expected = parents[1];
    }
    if expected != original.source_sha { return Ok(false); }
    let base = git(repo, &["merge-base", &original.source_sha, &original.target_sha])?;
    if !repository_proof::delivery_paths_changed(repo, &base, &original.source_sha, &original.target_sha, &current.target_sha)?.is_empty() { return Ok(false); }
    Ok(delivered_patch_id(repo, &original.target_sha, &original.tree).ok()
        .zip(delivered_patch_id(repo, &current.target_sha, &current.tree).ok())
        .is_some_and(|(old,new)| old == new))
}

fn read_mapping(original: &Candidate) -> Result<Option<Mapping>,String> {
    let path = original.directory()?.join("carryover.json");
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("carry mapping {}: {e}",path.display())),
    }
}

/// Preserve the original dispatch proof; consumers resolve its derived mapping.
/// No old context, source change or explicit new delivery cycle can consume it.
pub(super) fn mapped(root: &Path, dispatch: &VerificationDispatch, original: &Candidate) -> Result<Option<Candidate>,String> {
    let Some(mapping) = read_mapping(original)? else { return Ok(None) };
    let current = cas_store::get_latest_verification_dispatch(root, &dispatch.task_id).map_err(|e| e.to_string())?;
    if current.as_ref().is_none_or(|d| d.id != dispatch.id || d.state != cas_types::VerificationDispatchState::Resolved) { return Ok(None); }
    let task = crate::store::open_task_store(root).map_err(|e| e.to_string())?.get(&dispatch.task_id).map_err(|e| e.to_string())?;
    let Some(verdict) = verdict(root, dispatch)? else { return Ok(None) };
    let auth = &mapping.authorization;
    if auth.dispatch_id != dispatch.id || auth.verdict_id != verdict.id
        || task.deliverables.work_target.as_ref() != Some(&auth.target)
        || task.deliverables.delivery_cycle != auth.cycle || scope(root,&task)? != auth.scope { return Ok(None); }
    let authorized: Authorization = serde_json::from_slice(&std::fs::read(authorization_path(original,&auth.target,&auth.scope)?).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    if &authorized != auth { return Err("carry mapping differs from authorized retarget".into()); }
    if mapping.candidate_key.len() != 64 || !mapping.candidate_key.bytes().all(|b|b.is_ascii_hexdigit()) { return Err("invalid carry candidate key".into()); }
    let candidate: Candidate = serde_json::from_slice(&std::fs::read(root.join("verification-candidates").join(&mapping.candidate_key).join("candidate.json")).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    if candidate_manifest_key(&candidate)? != mapping.candidate_key
        || candidate.cas_root != root || candidate.target_branch != auth.target.target_branch
        || task.deliverables.factory_branch_anchor.as_deref() != Some(candidate.source_sha.as_str()) { return Ok(None); }
    if candidate.proof.repository_root != original.proof.repository_root
        || proof_candidate_key(&candidate.proof)?.as_deref() != Some(mapping.candidate_key.as_str())
        || git(&candidate.repository, &["rev-parse", &format!("{}^{{tree}}", candidate.proof.head_commit)])? != candidate.tree {
        return Err("carry candidate does not match its immutable repository proof".into());
    }
    repository_proof::verify_repository_proof(&candidate.proof).map_err(|e|e.to_string())?;
    if !equivalent(original,&candidate)? { return Err("carried candidate changed reviewed content".into()); }
    Ok(Some(candidate))
}

pub(super) fn reuse(core: &CasCore, task: &Task, repo: &Path, source: &str, target: &str, digest: &str, dispatch: Option<&VerificationDispatch>) -> Result<Option<Approval>,McpError> {
    let Some(dispatch) = dispatch else { return Ok(None) };
    if !matches!(dispatch.state, cas_types::VerificationDispatchState::Resolved | cas_types::VerificationDispatchState::Invalidated) { return Ok(None); }
    let Some(verdict) = verdict(&core.cas_root,dispatch).map_err(fail)? else { return Ok(None) };
    let Some(original) = original_candidate_for_dispatch(&core.cas_root,dispatch).map_err(fail)? else { return Ok(None) };
    let Some(work_target) = task.deliverables.work_target.as_ref().filter(|t|t.target_branch == target) else { return Ok(None) };
    let semantic_scope = scope(&core.cas_root,task).map_err(fail)?;
    let path = authorization_path(&original,work_target,&semantic_scope).map_err(fail)?;
    let auth: Authorization = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(fail)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(fail(e)),
    };
    if auth.dispatch_id != dispatch.id || auth.verdict_id != verdict.id || auth.target != *work_target
        || auth.scope != semantic_scope || auth.cycle != task.deliverables.delivery_cycle { return Ok(None); }
    let source_sha = git(repo,&["rev-parse",source]).map_err(fail)?;
    let target_sha = review_target(repo,target).map_err(fail)?;
    let mut preview = original.clone();
    preview.source_sha = source_sha;
    preview.target_sha = target_sha;
    preview.target_branch = target.into();
    preview.tree = regenerated_merge_tree(repo,&preview.target_sha,&preview.source_sha).map_err(fail)?;
    if !equivalent(&original,&preview).map_err(fail)? { return Ok(None); }
    let candidate = core.build_verification_candidate(task,repo,source,target,digest.into(),None)?;
    candidate.validate(&core.cas_root,task).map_err(fail)?;
    let current = core.open_task_store()?.get(&task.id).map_err(fail)?;
    if current.deliverables.factory_branch_anchor.as_deref() != Some(candidate.source_sha.as_str())
        || current.deliverables.work_target.as_ref() != Some(&auth.target)
        || current.deliverables.delivery_cycle != auth.cycle || scope(&core.cas_root,&current).map_err(fail)? != auth.scope { return Err(fail("retarget carry raced a task change")); }
    let mapping = Mapping { authorization:auth, candidate_key:candidate_manifest_key(&candidate).map_err(fail)? };
    crate::cli::integrate::fs::atomic_write(&original.directory().map_err(fail)?.join("carryover.json"), &serde_json::to_string(&mapping).map_err(fail)?).map_err(fail)?;
    cas_store::carry_forward_approved_verification(&core.cas_root,&current,&dispatch.id,&verdict.id).map_err(fail)?;
    let mut carried = current.clone();
    carried.deliverables.premerge_approval = Some(cas_types::PortablePremergeApproval {
        verdict_id:verdict.id.clone(), dispatch_id:dispatch.id.clone(), approved_anchor:candidate.source_sha.clone(), delivery_cycle:carried.deliverables.delivery_cycle,
    });
    core.open_task_store()?.update_if_current(&carried,&current).map_err(fail)?;
    Ok(Some(Approval { candidate, dispatch_id:dispatch.id.clone(), verdict_id:verdict.id }))
}

/// Release/close consumes the same mapping, with a positively checked landing.
pub(super) fn published(root:&Path, task:&Task, anchor:&str, published:&str) -> Result<Option<String>,String> {
    let Some(dispatch)=cas_store::get_latest_verification_dispatch(root,&task.id).map_err(|e|e.to_string())? else { return Ok(None) };
    let Some(original)=original_candidate_for_dispatch(root,&dispatch)? else { return Ok(None) };
    let Some(candidate)=mapped(root,&dispatch,&original)? else { return Ok(None) };
    if candidate.source_sha != anchor { return Ok(None); }
    let Some(verdict)=verdict(root,&dispatch)? else { return Ok(None) };
    let approval=Approval {candidate,dispatch_id:dispatch.id,verdict_id:verdict.id.clone()};
    if approval.promotion()?.is_some_and(|record|git(&approval.candidate.repository,&["merge-base","--is-ancestor",&record.landed_sha,published]).is_ok()) { return Ok(Some(verdict.id)); }
    Ok(None)
}
