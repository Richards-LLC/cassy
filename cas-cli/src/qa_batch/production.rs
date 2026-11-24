use super::{ancestor, commit};
use crate::config::Config;
use crate::git_evidence::git_text;
use crate::mcp::tools::core::task::{lifecycle::qa_evidence_gate::evidence_tier, repo_context};
use crate::qa_evidence::EvidenceTier;
use crate::qa_ledger::{Report, Verdict, evidence_cell_path};
use cas_store::ProductionCandidate;
use cas_types::Task;
use std::{io::Read, path::Path};

pub(super) fn branch(repo: &Path) -> Option<String> {
    let default = repo_context::resolve_default_branch(repo).ok()?;
    Config::configured_epic_base_branch(repo)
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
    let Some(branch) = branch(repo) else { return Ok(None) };
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
            && matches!(row.label.as_str(), "real-build" | "deployed-verification")
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
        [first, second] if *first == base => *second,
        _ => head,
    };
    let tree = git_text(repo, &["rev-parse", &format!("{head}^{{tree}}")]).ok_or_else(unresolved)?;
    Ok(Some(ProductionCandidate { base_sha: base, source_sha: source.into(), tree }))
}
