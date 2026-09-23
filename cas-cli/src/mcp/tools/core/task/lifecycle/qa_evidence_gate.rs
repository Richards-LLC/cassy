//! QA evidence close gate (cas-0cd5).
//!
//! Runs on every delivery close, immediately before the factory merge gate.
//! A user-facing delivery can then neither park for merge (and so never gets
//! an independent reviewer spawned, cas-619f) nor finish its post-merge
//! re-close without valid implementer evidence for the delivered head. The
//! same check refuses any delivery that adds test skip/focus markers without
//! a `cas-allow-skip:` reason.
//!
//! Eligibility is `qa_pass::user_facing_reasons`, shared with cas-619f.
//! Validation lives in `crate::qa_evidence`. Design:
//! `docs/qa/evidence-close-gate.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::mcp::tools::core::imports::*;
use crate::qa_evidence::{
    EvidenceContext, EvidenceTier, SkipMarker, added_skip_markers, delivery_range, delivery_test_diff,
    range_paths, run_close_gate,
};
use crate::qa_pass::{catalog_journeys_for, user_facing_reasons};

use super::close_ops::resolve_branch_sha;

/// The delivered commit this close answers for.
///
/// For a factory worker: the `factory/<assignee>` tip while it still carries
/// unmerged work; once that tip is contained in the target, the anchor the
/// park recorded (the tip may since have moved on to the worker's next
/// task). Otherwise the commit receipt, then the repository HEAD.
pub(crate) fn delivered_head(
    task: &Task,
    repo: &Path,
    target_branch: &str,
    commit_receipt: Option<&str>,
) -> Option<String> {
    let rev = |spec: &str| resolve_branch_sha(repo, &format!("{spec}^{{commit}}"));
    if let Some(assignee) = task.assignee.as_deref()
        && let Some(tip) = resolve_branch_sha(repo, &format!("factory/{assignee}"))
    {
        let merged = !target_branch.is_empty()
            && Command::new("git")
                .args(["merge-base", "--is-ancestor", &tip, target_branch])
                .current_dir(repo)
                .status()
                .is_ok_and(|status| status.success());
        if !merged {
            return Some(tip);
        }
        return task
            .deliverables
            .factory_branch_anchor
            .as_deref()
            .and_then(rev)
            .or(Some(tip));
    }
    commit_receipt.and_then(rev).or_else(|| rev("HEAD"))
}

/// Which evidence the shared user-facing reasons demand.
pub(crate) fn evidence_tier(reasons: &[String]) -> EvidenceTier {
    if reasons.is_empty() {
        EvidenceTier::None
    } else if reasons
        .iter()
        .any(|reason| reason.starts_with("journeys:") || reason.starts_with("path:"))
    {
        EvidenceTier::Bundle
    } else {
        EvidenceTier::Ledger
    }
}

/// Run the gate. `Ok(notes)` carries decision-note lines to record on close;
/// `Err(message)` is the complete rejection text.
pub(crate) fn qa_evidence_close_gate(
    cas_root: &Path,
    task: &Task,
    repo: &Path,
    target_branch: &str,
    commit_receipt: Option<&str>,
) -> Result<Vec<String>, String> {
    let Ok(config) = crate::config::Config::load(cas_root) else {
        return Ok(Vec::new());
    };
    let qa = config.qa();
    if !qa.evidence_gate
        || task.task_type == TaskType::Epic
        || task.execution_note.as_deref() == Some("no-code")
    {
        return Ok(Vec::new());
    }
    let Some(head) = delivered_head(task, repo, target_branch, commit_receipt) else {
        return Ok(Vec::new());
    };
    let range = (!target_branch.is_empty())
        .then(|| delivery_range(repo, &head, target_branch))
        .flatten();
    let changed = range
        .as_ref()
        .and_then(|(from, to)| range_paths(repo, from, to));
    let journeys = changed
        .as_deref()
        .map(|paths| catalog_journeys_for(repo, paths))
        .unwrap_or_default();
    let reasons = user_facing_reasons(task, &qa, changed.as_deref(), &journeys).reasons;
    let markers: Vec<SkipMarker> = match (range.as_ref(), changed.as_deref()) {
        (Some((from, to)), Some(paths)) => delivery_test_diff(repo, from, to, paths)
            .map(|diff| added_skip_markers(&diff))
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let artifacts_dir: PathBuf =
        crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref())
            .join(&task.id);
    let ctx = EvidenceContext {
        task_id: &task.id,
        task_artifacts_dir: &artifacts_dir,
        repo,
        delivered_head: &head,
        notes: &task.notes,
    };
    run_close_gate(&ctx, evidence_tier(&reasons), &reasons, &markers).map(|pass| pass.notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_surface_reasons_need_the_bundle_and_demo_only_needs_the_ledger() {
        assert_eq!(evidence_tier(&[]), EvidenceTier::None);
        assert_eq!(
            evidence_tier(&["path:web/a.css (**/*.css)".into(), "demo_statement".into()]),
            EvidenceTier::Bundle
        );
        assert_eq!(evidence_tier(&["journeys:J03".into()]), EvidenceTier::Bundle);
        assert_eq!(evidence_tier(&["demo_statement".into()]), EvidenceTier::Ledger);
    }
}
