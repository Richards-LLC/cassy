//! A merged delivery must remain reviewable when another close gate refuses.
use super::*;

impl CasCore {
    pub(super) fn prepare_merged_verification_dispatch(
        &self,
        task: &Task,
        req: &TaskCloseRequest,
        repo: &std::path::Path,
        target: &str,
        declared_target: Option<&str>,
    ) -> Result<(), McpError> {
        let config = self.load_config();
        if task.status != TaskStatus::AwaitingMerge
            || task.depth == crate::types::TaskDepth::Light
            || !config.verification_enabled()
            || !verification_policy(supervisor_harness_from_env(), worker_harness_from_env())
                .task_required()
        {
            return Ok(());
        }
        let Some(anchor) = task.deliverables.factory_branch_anchor.as_deref() else {
            return Ok(());
        };
        if !commit_is_merged_into_parent(repo, anchor, target) {
            return Ok(());
        }
        // Preserve active/terminal authority, including explicit supervisor-only
        // timeout recovery. Only a missing or invalidated delivery cycle needs
        // preparation here; ordinary close still revalidates its exact proof.
        let latest_dispatch = cas_store::get_latest_verification_dispatch(&self.cas_root, &task.id)
            .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        if latest_dispatch.as_ref().is_some_and(|dispatch| {
            dispatch.state != cas_types::VerificationDispatchState::Invalidated
        }) {
            return Ok(());
        }
        // A task with no typed dispatch keeps the close gate's legacy
        // authority: an undispatched approval still authorizes close, so a
        // fresh dispatch here would only void it.
        if latest_dispatch.is_none()
            && self
                .open_verification_store()
                .ok()
                .and_then(|store| store.get_latest_for_task(&task.id).ok().flatten())
                .is_some_and(|row| {
                    row.dispatch_id.is_none()
                        && matches!(
                            row.status,
                            VerificationStatus::Approved | VerificationStatus::Skipped
                        )
                })
        {
            return Ok(());
        }
        super::super::qa_evidence_gate::ledger_preflight(&self.cas_root, task)
            .map_err(|message| Self::error(ErrorCode::INVALID_PARAMS, message))?;
        let anchors = super::super::repository_proof::delivered_anchor_commits(
            repo,
            Some(target),
            req.commit_receipt.as_deref(),
            Some(anchor),
        );
        let repository = if let Some(target) = declared_target {
            super::super::repository_proof::capture_repository_proof_at_target(
                repo, target, anchors,
            )
        } else {
            super::super::repository_proof::capture_repository_proof_with_anchors(
                repo, repo, anchors,
            )
        }
        .map_err(|reason| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("Cannot prepare exact merged-delivery verification: {reason}"),
            )
        })?;
        let requester = self.get_agent_id()?;
        let owner = self.verification_dispatch_owner(&requester)?;
        let dispatch = cas_store::create_verification_dispatch_bound(
            &self.cas_root,
            &task.id,
            &requester,
            &owner,
            &cas_types::VerificationProofBoundary::task_at(repository),
            chrono::Utc::now() + chrono::Duration::seconds(VERIFICATION_DISPATCH_TIMEOUT_SECS),
            false,
        )
        .map_err(|error| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("Cannot prepare merged-delivery verification dispatch: {error}"),
            )
        })?;
        // cas-6a8b: a merge may already have answered COMMITTED while this
        // close continues in the background. Its response cannot carry a
        // dispatch created later, even when requester and owner are the same
        // supervisor. Always queue the typed wake; the emitter deduplicates
        // by dispatch id, independently of the response and outcome note.
        // This creates a review destination, not an approval or an exemption
        // from ancestry, content, QA, or any later close gate.
        if let Ok(queue) = crate::store::open_prompt_queue_store(&self.cas_root)
            && let Err(error) = super::super::supervisor_push::emit_verification_dispatch_handoff(
                &self.cas_root,
                queue.as_ref(),
                &dispatch.id,
                &task.id,
                &owner,
                dispatch.deadline_at,
                task.assignee.as_deref().unwrap_or("worker"),
                req.reason.as_deref(),
                dispatch
                    .repository
                    .as_ref()
                    .map(|proof| proof.head_commit.as_str()),
                None,
            )
        {
            tracing::warn!(task_id=%task.id, %error, "merged verification dispatch persisted; handoff unavailable");
        }
        Ok(())
    }
}
