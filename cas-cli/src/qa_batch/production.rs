use super::{ancestor, commit};
use crate::config::Config;
use crate::git_evidence::git_text;
use crate::mcp::tools::core::task::lifecycle::close_ops::{resolve_task_commit_receipt_window, task_attribution, task_commit_identity};
use crate::mcp::tools::core::task::{lifecycle::qa_evidence_gate::evidence_tier, repo_context};
use crate::qa_evidence::EvidenceTier;
use crate::qa_ledger::{Report, Verdict, evidence_cell_path};
use crate::store::open_task_store;
use cas_store::ProductionCandidate;
use cas_types::Task;
use std::path::Path;

pub(crate) fn branch(root: &Path, repo: &Path) -> Option<String> {
    let default = repo_context::resolve_default_branch(repo).ok()?;
    Config::configured_epic_base_branch(repo)
        .or_else(|| Config::load(root).ok()?.factory().epic_base_branch)
        .is_none_or(|target| target == default)
        .then_some(default)
}

pub(super) fn rendered_journeys(
    root: &Path, repo: &Path, task: &Task, anchor: &str, target: &str, assembled: &str,
) -> Result<Option<Vec<String>>, String> {
    let qa = Config::load(root).map_err(|error| error.to_string())?.qa();
    let history = crate::store::open_agent_store(root).map_err(|error| error.to_string())?
        .get_lease_history(&task.id, None)
        .map_err(|error| error.to_string())?;
    let window = resolve_task_commit_receipt_window(task.created_at, &history, task_commit_identity(task, Some(anchor.into())));
    let paths = match task_attribution::qa_paths(repo, target, &window, Some(anchor)) {
        Some(paths) => paths,
        None => crate::qa_evidence::content_paths(repo, &format!("{anchor}^1"), anchor)?,
    };
    let rendered_paths: Vec<_> = paths.iter().filter(|path| {
        !crate::qa_pass::is_non_surface_path(path)
            && crate::qa_pass::first_user_facing_path(std::slice::from_ref(*path), &qa.api_paths).is_none()
    }).cloned().collect();
    if rendered_paths.is_empty() {
        return Ok(None);
    }
    let journeys = crate::qa_evidence::journeys::journeys_for_paths_at(repo, assembled, &rendered_paths)?;
    let reasons = crate::qa_pass::user_facing_reasons(task, &qa, Some(&paths), &journeys).reasons;
    Ok((evidence_tier(&reasons, false) == EvidenceTier::Bundle).then_some(journeys))
}

pub(super) fn recording_candidate(
    root: &Path, repo: &Path, task: &Task, anchor: &str, head: &str,
) -> Result<Option<(ProductionCandidate, Vec<String>)>, String> {
    let Some(branch) = branch(root, repo) else { return Ok(None) };
    if !cas_store::has_qa_batch_deferral(root, &task.id).map_err(|error| error.to_string())?
        || super::integration_state(repo, anchor, &branch) == Some(true)
    {
        return Ok(None);
    }
    let unresolved = || format!("PRODUCTION_BATCH_QA_CANDIDATE_UNRESOLVED candidate_sha={head}");
    let base = commit(repo, &format!("refs/remotes/origin/{branch}"))
        .or_else(|| commit(repo, &format!("refs/heads/{branch}")))
        .ok_or_else(unresolved)?;
    let Some(journeys) = rendered_journeys(root, repo, task, anchor, &branch, head)? else { return Ok(None) };
    if ancestor(repo, &base, head) != Some(true) {
        return Err(unresolved());
    }
    let parents = git_text(repo, &["show", "-s", "--format=%P", head]).ok_or_else(unresolved)?;
    let parents: Vec<_> = parents.split_ascii_whitespace().collect();
    let source = match parents.as_slice() {
        [first, second] if *first == base && git_text(repo, &["show", "-s", "--format=%s", head])
            .is_some_and(|subject| subject.starts_with(crate::mcp::tools::core::workflow::premerge_verification::CANDIDATE_COMMIT_PREFIX)) => *second,
        _ => head,
    };
    let tree = git_text(repo, &["rev-parse", &format!("{head}^{{tree}}")]).ok_or_else(unresolved)?;
    Ok(Some((ProductionCandidate { base_sha: base, source_sha: source.into(), tree }, journeys)))
}

pub(super) fn validate_recording_ledger(
    repo: &Path, candidate: &ProductionCandidate, head: &str, journeys: &[String], body: &str, ledger: &Path, report: &Report,
) -> Result<(), String> {
    let mut builds = body.lines().filter_map(|line| line.strip_prefix("build: ")).map(str::trim);
    if builds.next() != Some(head) || builds.next().is_some() {
        return Err(format!("PRODUCTION_BATCH_QA_BUILD_MISMATCH candidate_sha={head}"));
    }
    let refused = |problem: String| format!("PRODUCTION_BATCH_QA_BROWSER_PASS_REQUIRED candidate_sha={head} — {problem}");
    let dir = ledger.parent().ok_or("ledger has no directory")?.canonicalize().map_err(|error| refused(error.to_string()))?;
    let not_before = crate::qa_evidence::committer_time(repo, head).ok_or_else(|| refused("cannot read the candidate commit time".into()))?;
    let (_, captures) = crate::qa_evidence::batch_visual_evidence(&dir, not_before, head, Some(&candidate.base_sha)).map_err(refused)?;
    let captured = |row: &&crate::qa_ledger::Row| {
        row.verdict == Verdict::Pass
            && evidence_cell_path(&dir, &row.evidence_path).is_some_and(|path| captures.iter().any(|shot| {
                shot.path == path && shot.cell == row.cell && crate::qa_ledger::capture_matches(&path, &shot.sha256)
            }))
    };
    let passed: Vec<_> = report.rows.iter().filter(captured).map(|row| row.id.as_str()).collect();
    if passed.is_empty() || journeys.iter().any(|journey| !passed.contains(&journey.as_str())) {
        return Err(refused("each affected rendered journey needs a PASS row citing a capture from this candidate's browser run".into()));
    }
    Ok(())
}

pub(crate) fn affected(
    root: &Path, repo: &Path, target: &str, base: &str, source: &str,
) -> Result<Vec<(String, String)>, String> {
    let target = target.strip_prefix("refs/heads/").unwrap_or(target);
    let target = target.strip_prefix("origin/").unwrap_or(target);
    if branch(root, repo).as_deref() != Some(target) {
        return Ok(Vec::new());
    }
    affected_deliveries(root, repo, target, base, source)
}

pub(crate) fn affected_deliveries(
    root: &Path, repo: &Path, target: &str, base: &str, source: &str,
) -> Result<Vec<(String, String)>, String> {
    let deferred = cas_store::production_qa_batch_deferrals(root).map_err(|error| error.to_string())?;
    if deferred.is_empty() {
        return Ok(Vec::new());
    }
    let project = crate::cloud::resolve_canonical_id(root).ok_or("project identity unavailable")?;
    let store = open_task_store(root).map_err(|error| error.to_string())?;
    let mut deliveries = Vec::new();
    for id in deferred {
        let task = store.get(&id).map_err(|error| error.to_string())?;
        if !crate::mcp::tools::core::task::task_owned_by_project(&task, root, Some(&project)) {
            continue;
        }
        let Some(anchor) = super::resolved_delivery_anchor(repo, &task, target) else { continue };
        let reachability = |head| ancestor(repo, &anchor, head)
            .ok_or_else(|| format!("cannot resolve delivery {} at {head}", task.id));
        if reachability(source)? && !reachability(base)? && rendered_journeys(root, repo, &task, &anchor, target, source)?.is_some() {
            deliveries.push((id, anchor));
        }
    }
    Ok(deliveries)
}

pub(crate) fn admit(
    root: &Path, repo: &Path, target: &str, candidate: &ProductionCandidate,
    candidate_sha: Option<&str>, waiver_task: Option<&str>,
) -> Result<(), String> {
    let named = candidate_sha.unwrap_or(&candidate.source_sha);
    let scope = affected(root, repo, target, &candidate.base_sha, &candidate.source_sha)
        .map_err(|error| format!("PRODUCTION_BATCH_QA_SCOPE_UNRESOLVED candidate_sha={named}: {error}"))?;
    admit_scoped(root, candidate, candidate_sha, waiver_task, &scope)
}

// gate-audit[has_production_waiver]: selector Revalidates the G-94 waiver already issued and logged for this exact source, base and tree; does not issue a new waiver.
pub(crate) fn admit_scoped(
    root: &Path, candidate: &ProductionCandidate, candidate_sha: Option<&str>, waiver_task: Option<&str>,
    scope: &[(String, String)],
) -> Result<(), String> {
    if scope.is_empty() {
        return Ok(());
    }
    if let Some(id) = waiver_task
        && crate::qa_pass::sync::has_production_waiver(root, id, candidate)?
    {
        return Ok(());
    }
    let project = crate::cloud::resolve_canonical_id(root).ok_or("project identity unavailable")?;
    if cas_store::list_qa_batch_receipts(root, &project).map_err(|error| error.to_string())?
        .iter().any(|receipt| {
            receipt.schema_version == cas_store::QA_BATCH_RECEIPT_SCHEMA
                && receipt.production_candidate.as_ref() == Some(candidate)
                && candidate_sha.is_none_or(|sha| receipt.staging_sha == sha)
                && scope.iter().all(|(task, anchor)| receipt.task_heads.get(task) == Some(anchor))
        })
    {
        return Ok(());
    }
    let named = candidate_sha.unwrap_or(&candidate.source_sha);
    Err(format!("PRODUCTION_BATCH_QA_REQUIRED candidate_sha={named} source_sha={} base_sha={} tree={}",
        candidate.source_sha, candidate.base_sha, candidate.tree))
}

pub(crate) fn admit_tip(
    root: &Path, repo: &Path, target: &str, base: &str, source: &str, waiver_task: Option<&str>,
) -> Result<(), String> {
    let scope = affected(root, repo, target, base, source)
        .map_err(|error| format!("PRODUCTION_BATCH_QA_SCOPE_UNRESOLVED candidate_sha={source}: {error}"))?;
    if scope.is_empty() { return Ok(()); }
    let candidate = merge_tree_candidate(repo, base, source)?;
    admit_scoped(root, &candidate, Some(source), waiver_task, &scope)
}

pub(crate) fn merge_tree_candidate(repo: &Path, base: &str, source: &str) -> Result<ProductionCandidate, String> {
    let tree = git_text(repo, &["merge-tree", "--write-tree", base, source])
        .and_then(|result| result.lines().next().map(str::to_owned))
        .ok_or_else(|| format!("PRODUCTION_BATCH_QA_CANDIDATE_UNRESOLVED candidate_sha={source}"))?;
    Ok(ProductionCandidate { base_sha: base.into(), source_sha: source.into(), tree })
}

pub(crate) fn candidate_for_waiver(
    root: &Path, repo: &Path, task: &Task, target: &str, source: &str,
) -> Result<Option<ProductionCandidate>, String> {
    let Some(production) = branch(root, repo) else { return Ok(None) };
    if target != production && task.task_type != cas_types::TaskType::Epic { return Ok(None); }
    let has_deferral = if task.task_type == cas_types::TaskType::Epic {
        cas_store::production_qa_batch_deferrals(root).map(|tasks| !tasks.is_empty())
    } else {
        cas_store::has_qa_batch_deferral(root, &task.id)
    }.map_err(|error| error.to_string())?;
    if !has_deferral { return Ok(None); }
    let base = crate::worktree::GitOperations::new(repo.to_path_buf()).fetch_merge_target(&production)
        .map_err(|error| error.to_string())?
        .or_else(|| commit(repo, &format!("refs/heads/{production}")))
        .ok_or_else(|| format!("PRODUCTION_BATCH_QA_CANDIDATE_UNRESOLVED candidate_sha={source}"))?;
    if affected_deliveries(root, repo, &production, &base, source)?.is_empty() { return Ok(None); }
    if task.task_type == cas_types::TaskType::Epic {
        return merge_tree_candidate(repo, &base, source).map(Some);
    }
    let tree = crate::mcp::tools::core::workflow::premerge_verification::regenerated_merge_tree(repo, &base, source)?;
    Ok(Some(ProductionCandidate { source_sha: source.into(), base_sha: base, tree }))
}

pub(crate) fn validate_landing(
    repo: &Path, candidate: &ProductionCandidate, candidate_sha: &str, landed: &str, base: &str,
) -> Result<(), String> {
    let range = format!("{base}..{landed}");
    let parents = format!(" {base} {}", candidate.source_sha);
    if base == candidate.base_sha
        && git_text(repo, &["rev-list", "--first-parent", "--parents", &range])
            .and_then(|history| history.lines().last().map(|line| line.ends_with(&parents))) == Some(true)
        && git_text(repo, &["rev-parse", &format!("{landed}^{{tree}}")]).as_deref() == Some(candidate.tree.as_str())
    {
        return Ok(());
    }
    Err(format!("PRODUCTION_BATCH_QA_REQUIRED candidate_sha={candidate_sha} base_sha={base} landing_sha={landed}"))
}

pub(super) fn integrated_candidate(repo: &Path, target: &str, receipt: &cas_store::QaBatchReceipt) -> Option<String> {
    let candidate = receipt.production_candidate.as_ref()?;
    if receipt.schema_version != cas_store::QA_BATCH_RECEIPT_SCHEMA || receipt.staging_sha == candidate.source_sha {
        return None;
    }
    let range = format!("{}..{}", candidate.base_sha, target);
    let history = git_text(repo, &["log", "--first-parent", "--reverse", "--format=%H %T %P", &range])?;
    let mut commits = history.lines();
    let mut first = commits.next()?.split_ascii_whitespace();
    let merge = first.next()?;
    let tree = first.next()?;
    if first.next()? != candidate.base_sha || first.next()? != candidate.source_sha || first.next().is_some() {
        return None;
    }
    if tree == candidate.tree {
        return Some(merge.to_owned());
    }
    let mut parent = merge;
    for line in commits {
        let mut fields = line.split_ascii_whitespace();
        let landed = fields.next()?;
        let tree = fields.next()?;
        if fields.next()? != parent || fields.next().is_some() { break; }
        if tree == candidate.tree { return Some(landed.to_owned()); }
        parent = landed;
    }
    None
}
