//! An approval for correction B cannot close the already-merged delivery A.
use super::*;

pub(super) struct ReviewedDelivery {
    pub(super) dispatch: String,
    pub(super) repository: cas_types::RepositoryProofBoundary,
}

/// A recorded correction B on the reusable lane supersedes a preservation
/// ref A only when that ref cannot carry B and A is retained as its history.
pub(super) fn tracked_correction_branch(
    repo: &std::path::Path,
    task: &Task,
    assignee: &str,
) -> Option<String> {
    let parked = task.deliverables.parked_branch.as_deref()?;
    let anchor = task.deliverables.factory_branch_anchor.as_deref()?;
    let preserved = worker_task_branch_ref(repo, assignee, &task.id)?;
    (parked == format!("factory/{assignee}")
        && resolve_branch_sha(repo, parked).as_deref() == Some(anchor)
        && !git_commit_is_ancestor(repo, anchor, &preserved)
        && task
            .deliverables
            .historical_factory_branch_anchors
            .iter()
            .any(|old| git_commit_is_ancestor(repo, old, anchor)))
    .then(|| parked.to_string())
}

pub(super) fn current(
    root: &std::path::Path,
    task: &Task,
) -> Result<Option<ReviewedDelivery>, String> {
    let Some(dispatch) = cas_store::get_latest_verification_dispatch(root, &task.id)
        .map_err(|error| format!("Cannot read exact delivery verification: {error}"))?
    else {
        return Ok(None);
    };
    if dispatch.state != cas_types::VerificationDispatchState::Resolved {
        return Ok(None);
    }
    let verdict = cas_store::get_verification_for_dispatch(root, &dispatch.id)
        .map_err(|error| format!("Cannot read exact delivery verdict: {error}"))?;
    if !verdict.is_some_and(|verdict| {
        verdict.verification_type == VerificationType::Task
            && matches!(
                verdict.status,
                VerificationStatus::Approved | VerificationStatus::Skipped
            )
    }) {
        return Ok(None);
    }
    Ok(dispatch.repository.map(|repository| ReviewedDelivery {
        dispatch: dispatch.id,
        repository,
    }))
}

impl ReviewedDelivery {
    pub(super) fn merge_refusal(
        &self,
        repo: &std::path::Path,
        task: &Task,
        target: &str,
        receipt: Option<&str>,
    ) -> Option<String> {
        // Synthetic origin refs do not displace a local-only project's
        // integration authority. Published projects answer for live origin.
        let reference = if !origin_remote_configured(repo) {
            target.to_string()
        } else {
            preferred_live_target_ref_for_delivery(repo, target, task.delivery_mode).0
        };
        // Preserve an explicit canonical squash receipt only when its full
        // tree equals this exact verified head and it is on the authoritative
        // target. A receipt for earlier A cannot stand in for changed B.
        if receipt.is_some_and(|receipt| {
            git_commit_is_ancestor(repo, receipt, &reference)
                && target_reachable_squash_of(repo, &self.repository.head_commit, receipt, target)
        }) {
            return None;
        }
        (!git_commit_is_ancestor(repo, &self.repository.head_commit, &reference)).then(|| format!(
            "MERGE REQUIRED: task {} was verified at exact head {} (dispatch {}), which is not contained in {reference}. An earlier merged anchor cannot authorize this correction. Merge this exact verified delivery into {target}, then retry close.",
            task.id, self.repository.head_commit, self.dispatch,
        ))
    }
}

impl CasCore {
    /// Follow an approved same-task correction on the worker's actual pushed
    /// lane. Do not move/push its old preservation ref (or trigger CI again).
    /// Ordinary upkeep, QA, merge, content and verification gates then judge B.
    pub(super) fn track_reviewed_correction(
        &self,
        tasks: &dyn cas_store::TaskStore,
        task: &Task,
        repo: &std::path::Path,
        target: &str,
        reviewed: &ReviewedDelivery,
    ) -> Result<Option<Task>, String> {
        let Some(previous) = task.deliverables.factory_branch_anchor.as_deref() else {
            return Ok(None);
        };
        let head = &reviewed.repository.head_commit;
        if task.status != TaskStatus::AwaitingMerge
            || previous == head
            || !commit_is_merged_into_parent(repo, previous, target)
            || !git_commit_is_ancestor(repo, previous, head)
        {
            return Ok(None);
        }
        if super::super::repository_proof::verify_repository_proof(&reviewed.repository).is_err() {
            return Ok(None); // Ordinary close invalidates the drifted proof.
        }
        let Some(assignee) = task.assignee.as_deref() else {
            return Ok(None);
        };
        let branches = [
            crate::factory_isolation::worker_task_branch(assignee, &task.id),
            format!("factory/{assignee}"),
        ];
        for branch in branches {
            let pushed = if task.delivery_mode == cas_types::DeliveryMode::LocalMerge {
                branch.clone()
            } else {
                if crate::git_evidence::fetch_parent_branch_result(repo, &branch)
                    != BoundedFetch::Succeeded
                {
                    continue;
                }
                format!("origin/{branch}")
            };
            if resolve_branch_sha(repo, &pushed).as_deref() != Some(head) {
                continue;
            }
            // The resolved verdict authorizes resuming this task's plain
            // lane, but cannot erase sibling leases or foreign task commits.
            let mut correction = task.clone();
            correction.status = TaskStatus::InProgress;
            if !self.tip_is_own_task_lineage(
                tasks,
                &correction,
                repo,
                Some(&branch),
                previous,
                Some(head),
            ) || !parked_anchor_advance_with_target_is_attributable(
                repo,
                previous,
                head,
                &task.id,
                self.sibling_lease_boundary(tasks, task),
                target,
            ) {
                continue;
            }
            let paths = crate::qa_pass::changed_paths_for_delivery(repo, target, head)
                .map_err(|error| format!("Cannot measure reviewed correction: {error}"))?;
            if let Some(refusal) =
                receipt_scope::park_refusal(&self.cas_root, repo, task, &paths, head, target)
            {
                return Err(refusal);
            }
            let mut next = task.clone();
            next.deliverables
                .record_park(Some(&branch), Some(head), Some(assignee));
            if next.deliverables.factory_branch_anchor.as_deref() != Some(head) {
                next.deliverables.retain_factory_branch_anchor_as_history();
                next.deliverables.factory_branch_anchor = Some(head.clone());
            }
            next.notes.push_str(&format!(
                "\n\n[{}] Reviewed correction: dispatch {} approved {head}; delivery now follows pushed {branch}, replacing merged anchor {previous}. Preservation refs were not pushed.",
                chrono::Utc::now().format("%Y-%m-%d %H:%M"), reviewed.dispatch,
            ));
            tasks.update_if_current(&next, task).map_err(|error| {
                format!("Reviewed delivery changed concurrently; retry close: {error}")
            })?;
            return tasks
                .get(&task.id)
                .map(Some)
                .map_err(|error| error.to_string());
        }
        Ok(None)
    }
}
