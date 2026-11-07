//! An approval for correction B cannot close the already-merged delivery A.
use super::*;
use crate::mcp::tools::core::workflow::premerge_verification::{self, Candidate};

pub(super) struct ReviewedDelivery {
    pub(super) dispatch: String,
    pub(super) repository: cas_types::RepositoryProofBoundary,
    candidate: Option<Candidate>,
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
    let candidate = premerge_verification::candidate_for_dispatch(root, &dispatch)?;
    Ok(dispatch.repository.map(|repository| ReviewedDelivery {
        dispatch: dispatch.id,
        repository,
        candidate,
    }))
}

impl ReviewedDelivery {
    fn premerge_source_is_contained(
        &self,
        repo: &std::path::Path,
        task: &Task,
        target: &str,
        reference: &str,
    ) -> bool {
        let Some(candidate) = self.candidate.as_ref() else {
            return false;
        };
        let Some(landed) = resolve_branch_sha(repo, reference) else {
            return false;
        };
        if candidate.task_id != task.id
            || candidate.target_branch != target
            || task.deliverables.factory_branch_anchor.as_deref()
                != Some(candidate.source_sha.as_str())
            || !git_commit_is_ancestor(repo, &candidate.source_sha, &landed)
            || !git_stdout_lines(
                repo,
                &["show", "-s", "--format=%P", &self.repository.head_commit],
            )
            .is_ok_and(|parents| {
                parents == vec![format!("{} {}", candidate.target_sha, candidate.source_sha)]
            })
        {
            return false;
        }
        let Some(base) = git_merge_base(repo, &candidate.target_sha, &candidate.source_sha) else {
            return false;
        };
        // The first first-parent descendant of the source is its queue
        // landing. If that descendant's first parent IS the source, the source
        // itself landed by fast-forward. Never search for a later matching
        // patch: that could excuse a dropping merge followed by a repair.
        let landing = if landed == candidate.source_sha {
            landed.clone()
        } else {
            let range = format!("{}..{landed}", candidate.source_sha);
            let Some(first) = git_stdout_lines(
                repo,
                &[
                    "rev-list",
                    "--first-parent",
                    "--reverse",
                    "--ancestry-path",
                    &range,
                ],
            )
            .ok()
            .and_then(|history| history.into_iter().next()) else {
                return false;
            };
            let Some(parent) = resolve_branch_sha(repo, &format!("{first}^1")) else {
                return false;
            };
            if parent == candidate.source_sha {
                candidate.source_sha.clone()
            } else {
                first
            }
        };
        // The verifier reviewed the merged candidate, including target edits
        // beside source hunks. Its exact tree at the first landing is stronger
        // evidence than reverse-applying the source's older patch context.
        if resolve_branch_sha(repo, &format!("{landing}^{{tree}}")).as_deref()
            == Some(candidate.tree.as_str())
        {
            return true;
        }
        // A later unrelated target advance can change the full landing tree.
        // Compare the actual merge delta with the reviewed candidate delta,
        // retaining full patch context and whitespace in the existing hash.
        if let Some(parent) = resolve_branch_sha(repo, &format!("{landing}^1"))
            && let Ok(reviewed) = premerge_verification::delivered_patch_id(
                repo, &candidate.target_sha, &candidate.tree,
            )
            && premerge_verification::delivered_patch_id(repo, &parent, &landing)
                .is_ok_and(|landed| landed == reviewed)
        {
            return true;
        }
        // Prove the complete reviewed patch at that immutable landing. Later
        // ordinary edits to the same lines do not undo a legitimate delivery.
        // A private index keeps HEAD, the worktree and the real index untouched.
        reverse_delivery_patch_applies_to_tree(repo, &base, &candidate.source_sha, &landing, None)
            .unwrap_or(false)
    }

    /// A batch receipt may cover later sibling edits after this exact
    /// committed target proof landed. It cannot bless a dirty worktree proof,
    /// an earlier anchor in place of a correction, or an unlanded review.
    pub(super) fn landed_proof_is_covered(
        &self,
        root: &std::path::Path,
        repo: &std::path::Path,
        task: &Task,
        target: &str,
    ) -> bool {
        let Some(anchor) = task.deliverables.factory_branch_anchor.as_deref() else {
            return false;
        };
        self.repository.target_branch.is_some()
            && self.repository.anchor_commits.iter().any(|bound| bound == anchor)
            && super::super::repository_proof::capture_repository_proof_at_target(
                repo, &self.repository.head_commit, self.repository.anchor_commits.clone(),
            ).is_ok_and(|bound| bound.state_digest == self.repository.state_digest)
            && self.merge_refusal(repo, task, target, None).is_none()
            && (crate::qa_batch::satisfying_release_verdict(root, task, repo, target, anchor)
                .is_ok_and(|receipt| receipt.is_some())
                || crate::qa_batch::satisfying_receipt(root, task, repo, target, anchor)
                    .is_ok_and(|receipt| receipt.is_some()))
    }

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
        // A queue publishes its own merge, not the synthetic reviewed commit.
        // The immutable candidate must bind this task's exact delivery and the
        // authoritative target must contain both its identity and all its patch.
        let contained = if self.candidate.is_some() {
            // Synthetic approval never falls back to a squash receipt or bare
            // commit ancestry when its bound source/patch check fails.
            self.premerge_source_is_contained(repo, task, target, &reference)
        } else {
            // Preserve an explicit canonical squash receipt only when its full
            // tree equals this exact verified head and it is on the authoritative
            // target. A receipt for earlier A cannot stand in for changed B.
            if receipt.is_some_and(|receipt| {
                git_commit_is_ancestor(repo, receipt, &reference)
                    && target_reachable_squash_of(
                        repo,
                        &self.repository.head_commit,
                        receipt,
                        target,
                    )
            }) {
                return None;
            }
            git_commit_is_ancestor(repo, &self.repository.head_commit, &reference)
        };
        (!contained).then(|| format!(
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
                receipt_scope::park_refusal(&self.cas_root, repo, task, &paths, head, target, None)
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
                "\n\n[{}] Reviewed correction: dispatch {} approved {head}; delivery now follows measured ref {pushed}, replacing merged anchor {previous}. Preservation refs were not pushed.",
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
