//! Pinned supervisor batch receipts and exact squash containment (GH1097).
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use super::lifecycle::close_ops::{
    DeliveryContentPresence, commit_is_merged_into_parent, delivery_content_presence_in_parent,
    effective_close_work_target, resolve_close_gate_repo_root, resolve_close_parent_branch,
    resolve_task_commit_receipt_sha,
};
use crate::git_evidence::measurement::CommandExt as _;
use crate::mcp::CasCore;
use cas_types::{IntegrationBatchEvidence, Task, TaskStatus};

fn git(repo: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .measurement_output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}
fn revision(repo: &Path, reference: &str) -> Option<String> {
    if reference.is_empty() || reference.starts_with('-') {
        return None;
    }
    let output = git(
        repo,
        &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
    )?;
    Some(String::from_utf8(output).ok()?.trim().into())
}
fn ancestor(repo: &Path, older: &str, newer: &str) -> bool {
    git(repo, &["merge-base", "--is-ancestor", older, newer]).is_some()
}

// Compare the squash's own changed paths and final blobs/modes. Old blobs
// need not match: the integration target may have advanced since batch cut.
// Raw, NUL-delimited paths and full object ids preserve binary and whitespace
// changes, deletions, mode changes and unusual filenames without patch fuzz.
fn full_sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn final_delta(repo: &Path, from: &str, to: &str) -> Option<BTreeMap<Vec<u8>, (String, String)>> {
    if !full_sha(from) || !full_sha(to) {
        return None;
    }
    let raw = git(
        repo,
        &[
            "diff",
            "--raw",
            "--no-abbrev",
            "--no-renames",
            "--no-ext-diff",
            "-z",
            from,
            to,
            "--",
        ],
    )?;
    let mut parts = raw.split(|byte| *byte == 0);
    let mut delta = BTreeMap::new();
    while let Some(header) = parts.next().filter(|part| !part.is_empty()) {
        let fields: Vec<_> = std::str::from_utf8(header)
            .ok()?
            .split_whitespace()
            .collect();
        if fields.len() != 5 || !fields[0].starts_with(':') {
            return None;
        }
        let path = parts.next()?.to_vec();
        if path.is_empty()
            || delta
                .insert(path, (fields[1].into(), fields[3].into()))
                .is_some()
        {
            return None;
        }
    }
    (!delta.is_empty()).then_some(delta)
}

impl CasCore {
    pub(crate) fn prepare_integration_batch(
        &self,
        task: &Task,
        selector: &str,
    ) -> Result<Option<IntegrationBatchEvidence>, String> {
        let supervisor = self.resolve_live_supervisor_authority().map_err(|_| {
            "INTEGRATION BATCH REJECTED: merged_into requires a live registered supervisor"
                .to_string()
        })?;
        let selector = selector.trim();
        if selector.is_empty() {
            return Ok(None);
        }
        if task.status != TaskStatus::AwaitingMerge {
            return Err(
                "INTEGRATION BATCH REJECTED: only a parked awaiting_merge delivery can be staged"
                    .into(),
            );
        }
        let head = task
            .deliverables
            .factory_branch_anchor
            .as_deref()
            .ok_or("INTEGRATION BATCH REJECTED: parked delivery has no recorded anchor")?;
        let (branch, tip) = selector
            .rsplit_once('@')
            .ok_or("INTEGRATION BATCH REJECTED: expected merged_into=<batch-ref>@<commit SHA>")?;
        if branch.is_empty() || branch.starts_with('-') {
            return Err("INTEGRATION BATCH REJECTED: unsafe or empty batch ref".into());
        }
        let parent = self
            .open_task_store()
            .map_err(|err| err.to_string())?
            .get_parent_epic(&task.id)
            .map_err(|err| err.to_string())?;
        let context = effective_close_work_target(task, parent.as_ref())
            .map(|target| super::repo_context::resolve_repo_context(&self.cas_root, &target))
            .transpose()?;
        let repo = match &context {
            Some(context) => context.repo_root.clone(),
            None => resolve_close_gate_repo_root(&self.cas_root)?,
        };
        let target = match context {
            Some(context) => context.target_branch,
            None => resolve_close_parent_branch(
                None,
                parent.as_ref().and_then(|epic| epic.branch.clone()),
                parent.as_ref().and_then(|epic| {
                    epic.deliverables
                        .work_target
                        .as_ref()
                        .map(|target| target.target_branch.clone())
                }),
                &repo,
            )?,
        };
        let tip = resolve_task_commit_receipt_sha(&repo, tip)?;
        if revision(&repo, branch).as_deref() != Some(tip.as_str()) {
            return Err(
                "INTEGRATION BATCH REJECTED: batch ref does not resolve to the supplied exact tip"
                    .into(),
            );
        }
        if !ancestor(&repo, head, &tip)
            || !matches!(
                delivery_content_presence_in_parent(&repo, head, &tip),
                DeliveryContentPresence::Present { .. }
                    | DeliveryContentPresence::Superseded { .. }
            )
        {
            return Err("INTEGRATION BATCH REJECTED: pinned batch must contain the parked anchor and its delivery content".into());
        }
        let live_target = revision(&repo, &format!("origin/{target}"))
            .or_else(|| revision(&repo, &target))
            .ok_or("INTEGRATION BATCH REJECTED: target cannot be resolved")?;
        let base = git(&repo, &["merge-base", &live_target, &tip])
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(|text| text.trim().to_string())
            .ok_or("INTEGRATION BATCH REJECTED: batch has no target merge base")?;
        final_delta(&repo, &base, &tip)
            .ok_or("INTEGRATION BATCH REJECTED: batch delta is empty or unknowable")?;
        Ok(Some(IntegrationBatchEvidence {
            branch: branch.into(),
            tip,
            base,
            delivered_head: head.into(),
            supervisor_id: supervisor.id,
            recorded_at: chrono::Utc::now(),
        }))
    }
}

/// Accept only a target-reachable commit whose own first-parent delta matches
/// the complete pinned batch. A dropped or added path is never containment.
/// Keep the original anchor for review, evidence and executable-hook scope.
pub(crate) fn landed_batch_squash(
    task: &Task,
    repo: &Path,
    target: &str,
    receipt: Option<&str>,
) -> Option<String> {
    let batch = task.deliverables.integration_batch.as_ref()?;
    if !full_sha(&batch.base)
        || !full_sha(&batch.tip)
        || !full_sha(&batch.delivered_head)
        || task.deliverables.factory_branch_anchor.as_deref() != Some(batch.delivered_head.as_str())
        || !ancestor(repo, &batch.delivered_head, &batch.tip)
    {
        return None;
    }
    let expected = final_delta(repo, &batch.base, &batch.tip)?;
    let mut candidates = Vec::new();
    if let Some(receipt) =
        receipt.and_then(|receipt| resolve_task_commit_receipt_sha(repo, receipt).ok())
    {
        candidates.push(receipt);
    }
    for reference in [target.to_string(), format!("origin/{target}")] {
        let Some(tip) = revision(repo, &reference) else {
            continue;
        };
        // The recorded base bounds the search to changes since batch cut.
        let range = format!("{}..{tip}", batch.base);
        if let Some(history) = git(repo, &["rev-list", "--first-parent", &range, "--"]) {
            candidates.extend(String::from_utf8(history).ok()?.lines().map(str::to_string));
        }
    }
    let mut seen = std::collections::HashSet::new();
    for candidate in candidates {
        if !seen.insert(candidate.clone())
            || !commit_is_merged_into_parent(repo, &candidate, target)
        {
            continue;
        }
        let Some(parent) = revision(repo, &format!("{candidate}^1")) else {
            continue;
        };
        // A merge commit is an ordinary topology proof, not a squash receipt.
        if revision(repo, &format!("{candidate}^2")).is_some() {
            continue;
        }
        if final_delta(repo, &parent, &candidate).as_ref() == Some(&expected)
            && matches!(
                delivery_content_presence_in_parent(repo, &candidate, target),
                DeliveryContentPresence::Present { .. }
                    | DeliveryContentPresence::Superseded { .. }
            )
        {
            return Some(candidate);
        }
    }
    None
}
