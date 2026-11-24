use super::{ancestor, commit};
use crate::config::Config;
use crate::git_evidence::git_text;
use crate::mcp::tools::core::task::{lifecycle::qa_evidence_gate::evidence_tier, repo_context};
use crate::qa_evidence::EvidenceTier;
use crate::qa_ledger::{Report, Verdict, evidence_cell_path};
use crate::store::open_task_store;
use cas_store::ProductionCandidate;
use cas_types::Task;
use std::{io::Read, path::Path};

pub(crate) fn branch(root: &Path, repo: &Path) -> Option<String> {
    let default = repo_context::resolve_default_branch(repo).ok()?;
    Config::configured_epic_base_branch(repo)
        .or_else(|| Config::load(root).ok()?.factory().epic_base_branch)
        .is_none_or(|target| target == default)
        .then_some(default)
}

pub(super) fn rendered_journeys(
    root: &Path, repo: &Path, task: &Task, anchor: &str, production_base: &str,
) -> Result<Option<Vec<String>>, String> {
    if !cas_store::has_qa_batch_deferral(root, &task.id).map_err(|error| error.to_string())? {
        return Ok(None);
    }
    let qa = Config::load(root).map_err(|error| error.to_string())?.qa();
    let base = task.deliverables.factory_branch_start.as_ref()
        .map(|start| start.base_sha.clone())
        .or_else(|| git_text(repo, &["merge-base", production_base, anchor]))
        .ok_or_else(|| format!("PRODUCTION_BATCH_QA_DELIVERY_UNRESOLVED task={} anchor={anchor}", task.id))?;
    let paths = crate::qa_evidence::content_paths(repo, &base, anchor)?;
    let rendered_paths: Vec<_> = paths.iter().filter(|path| {
        !crate::qa_pass::is_non_surface_path(path)
            && crate::qa_pass::first_user_facing_path(std::slice::from_ref(*path), &qa.api_paths).is_none()
    }).cloned().collect();
    if rendered_paths.is_empty() {
        return Ok(None);
    }
    let journeys = crate::qa_evidence::journeys::journeys_for_paths_at(repo, anchor, &rendered_paths)?;
    let reasons = crate::qa_pass::user_facing_reasons(task, &qa, Some(&paths), &journeys).reasons;
    Ok((evidence_tier(&reasons, false) == EvidenceTier::Bundle).then_some(journeys))
}

pub(super) fn recording_candidate(
    root: &Path, repo: &Path, task: &Task, anchor: &str, head: &str,
    body: &str, ledger: &Path, report: &Report,
) -> Result<Option<ProductionCandidate>, String> {
    let Some(branch) = branch(root, repo) else { return Ok(None) };
    if !cas_store::has_qa_batch_deferral(root, &task.id).map_err(|error| error.to_string())? {
        return Ok(None);
    }
    let unresolved = || format!("PRODUCTION_BATCH_QA_CANDIDATE_UNRESOLVED candidate_sha={head}");
    let base = commit(repo, &format!("refs/remotes/origin/{branch}"))
        .or_else(|| commit(repo, &format!("refs/heads/{branch}")))
        .ok_or_else(unresolved)?;
    let Some(journeys) = rendered_journeys(root, repo, task, anchor, &base)? else { return Ok(None) };
    let mut builds = body.lines().filter_map(|line| line.strip_prefix("build: ")).map(str::trim);
    if builds.next() != Some(head) || builds.next().is_some() {
        return Err(format!("PRODUCTION_BATCH_QA_BUILD_MISMATCH candidate_sha={head}"));
    }
    let browser_pass = |row: &&crate::qa_ledger::Row| {
        row.verdict == Verdict::Pass
            && row.label == "real-build"
            && row.observed.starts_with("visible:")
            && ledger.parent().and_then(|directory| evidence_cell_path(directory, &row.evidence_path))
                .is_some_and(|capture| {
                    let mut signature = [0; 8];
                    std::fs::File::open(capture).and_then(|mut file| file.read_exact(&mut signature)).is_ok()
                        && signature == *b"\x89PNG\r\n\x1a\n"
                })
    };
    let passed: Vec<_> = report.rows.iter().filter(browser_pass).map(|row| row.id.as_str()).collect();
    if passed.is_empty() || journeys.iter().any(|journey| !passed.contains(&journey.as_str())) {
        return Err(format!("PRODUCTION_BATCH_QA_BROWSER_PASS_REQUIRED candidate_sha={head}"));
    }
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
    Ok(Some(ProductionCandidate { base_sha: base, source_sha: source.into(), tree }))
}

pub(crate) fn affected(
    root: &Path, repo: &Path, target: &str, base: &str, source: &str,
) -> Result<Vec<(String, String)>, String> {
    let target = target.strip_prefix("refs/heads/").unwrap_or(target);
    let target = target.strip_prefix("origin/").unwrap_or(target);
    if branch(root, repo).as_deref() != Some(target) {
        return Ok(Vec::new());
    }
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
        if reachability(source)? && !reachability(base)? && rendered_journeys(root, repo, &task, &anchor, base)?.is_some() {
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
    if scope.is_empty() {
        return Ok(());
    }
    if let Some(id) = waiver_task
        && cas_store::list_qa_passes(root, id).map_err(|error| error.to_string())?
            .iter().find(|pass| pass.bound_head == candidate.source_sha)
            .is_some_and(|pass| pass.state == cas_types::QaPassState::Waived
                && pass.issuer_agent_id.as_deref().is_some_and(|issuer| !issuer.trim().is_empty())
                && pass.summary.as_deref().is_some_and(|reason| !reason.trim().is_empty()))
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
    Err(format!("PRODUCTION_BATCH_QA_REQUIRED candidate_sha={named} source_sha={} base_sha={} tree={}",
        candidate.source_sha, candidate.base_sha, candidate.tree))
}

pub(crate) fn admit_tip(
    root: &Path, repo: &Path, target: &str, base: &str, source: &str, waiver_task: Option<&str>,
) -> Result<(), String> {
    if affected(root, repo, target, base, source)
        .map_err(|error| format!("PRODUCTION_BATCH_QA_SCOPE_UNRESOLVED candidate_sha={source}: {error}"))?.is_empty()
    {
        return Ok(());
    }
    let tree = git_text(repo, &["merge-tree", "--write-tree", base, source])
        .and_then(|result| result.lines().next().map(str::to_owned))
        .ok_or_else(|| format!("PRODUCTION_BATCH_QA_CANDIDATE_UNRESOLVED candidate_sha={source}"))?;
    let candidate = ProductionCandidate { base_sha: base.into(), source_sha: source.into(), tree };
    admit(root, repo, target, &candidate, Some(source), waiver_task)
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
