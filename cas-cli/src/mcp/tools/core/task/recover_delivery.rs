//! Audited recovery of integrated delivery identity before independent QA.
use super::lifecycle::close_ops;
use crate::mcp::CasCore;
use cas_types::TaskType;
use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, ErrorCode},
};

impl CasCore {
    /// GH #1447 / #1427: recover identity before recording QA, without asking
    /// close to waive the very receipt which needs that identity to exist.
    pub(crate) fn recover_task_delivery(
        &self,
        id: &str,
        receipt: &str,
        reason: &str,
        supervisor_override: bool,
    ) -> Result<CallToolResult, McpError> {
        let reject = |reason: String| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("DELIVERY RECOVERY REFUSED: {reason}"),
            )
        };
        let supervisor = self.resolve_live_supervisor_authority().map_err(|_| {
            reject("only a live registered supervisor may recover a delivery".into())
        })?;
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(reject("a non-empty recovery reason is required".into()));
        }
        if let Some(error) = super::caller_note_marker_rejection(reason) {
            return Err(reject(error.into()));
        }
        let tasks = self.open_task_store()?;
        let expected = tasks.get(id).map_err(|error| reject(error.to_string()))?;
        let project = super::current_project_id(&self.cas_root);
        if expected.is_terminal()
            || expected.task_type == TaskType::Epic
            || !super::task_owned_by_project(&expected, &self.cas_root, project.as_deref())
        {
            return Err(reject(
                "recovery requires a nonterminal delivery in this project".into(),
            ));
        }
        let agents = self.open_agent_store()?;
        if agents
            .get_lease(id)
            .map_err(|error| reject(error.to_string()))?
            .is_some_and(|lease| lease.is_valid())
        {
            return Err(reject(
                "an active task lease remains; resolve ownership before recovery".into(),
            ));
        }
        if expected.deliverables.integration_batch.is_some()
            || cas_store::get_latest_worker_delivery(&self.cas_root, id)
                .map_err(|error| reject(error.to_string()))?
                .is_some()
        {
            return Err(reject(
                "an immutable delivery transaction already owns this identity".into(),
            ));
        }
        let (repo, target) =
            crate::qa_batch::task_context(&self.cas_root, &expected).map_err(reject)?;
        let head = close_ops::resolve_task_commit_receipt_sha(&repo, receipt).map_err(reject)?;
        if let Some(active) = expected
            .deliverables
            .factory_branch_anchor
            .as_deref()
            .and_then(|anchor| crate::qa_batch::commit(&repo, anchor))
            && active != head
        {
            return Err(reject("a resolvable active anchor already owns this delivery; request changes before replacing it".into()));
        }
        let identity = close_ops::task_commit_identity(&expected, None);
        if !close_ops::commit_is_task_attributable(&repo, &head, &identity)
            && !identity.known_commits.iter().any(|source| {
                close_ops::find_squash_tree_match(&repo, source, &target).as_ref() == Some(&head)
            })
        {
            return Err(reject(
                "the integrated commit is not attributable to this task".into(),
            ));
        }
        let target_refs = || {
            [target.clone(), format!("origin/{target}")]
                .map(|reference| crate::qa_batch::commit(&repo, &reference))
        };
        let target_before = target_refs();
        let history = agents
            .get_lease_history(id, None)
            .map_err(|error| reject(error.to_string()))?;
        let mut window =
            close_ops::resolve_task_commit_receipt_window(expected.created_at, &history, identity);
        // Recovery may credit deliberately evolved content only through an
        // explicit live-supervisor review. The shared receipt validator checks
        // target reachability and every dropped path's final state below.
        if supervisor_override && let Some(review) = reason.strip_prefix("reviewed-drop:") {
            let (commits, _) = review.split_once(" -- ").ok_or_else(|| {
                reject("reviewed-drop requires descendant commit receipts and a review reason".into())
            })?;
            for named in commits.split(',') {
                let descendant = close_ops::resolve_task_commit_receipt_sha(&repo, named.trim())
                    .map_err(reject)?;
                if descendant == head
                    || crate::qa_batch::ancestor(&repo, &head, &descendant) != Some(true)
                {
                    return Err(reject(format!(
                        "reviewed recovery commit `{descendant}` must descend from delivery `{head}`"
                    )));
                }
            }
            window.supervisor_override_reason = Some(reason.to_owned());
        }
        let proof = close_ops::validate_task_commit_receipt(&repo, &head, &target, &window)
            .map_err(reject)?;
        if target_before != target_refs() {
            return Err(reject(
                "integration refs changed during recovery; inspect and retry".into(),
            ));
        }
        if expected.deliverables.factory_branch_anchor.as_deref() == Some(&head) {
            return Ok(Self::success(format!(
                "Task {id} already records integrated delivery {head}."
            )));
        }
        let mut recovered = expected.clone();
        recovered
            .deliverables
            .retain_factory_branch_anchor_as_history();
        recovered.deliverables.factory_branch_anchor = Some(head.clone());
        recovered.deliverables.green_merge_commit = None;
        recovered.updated_at = chrono::Utc::now();
        let audit = serde_json::json!({
            "actor": supervisor.id, "reason": reason, "head": head,
            "previous_anchor": expected.deliverables.factory_branch_anchor,
            "historical_anchors": expected.deliverables.historical_factory_branch_anchors,
            "target": target, "target_tips": target_before, "proof": proof,
        });
        recovered.notes.push_str(&format!(
            "\n\n[{}] DELIVERY IDENTITY RECOVERED: {audit}",
            recovered.updated_at.format("%Y-%m-%d %H:%M")
        ));
        tasks
            .update_if_current(&recovered, &expected)
            .map_err(|error| reject(error.to_string()))?;
        crate::quality_ledger::intervention(
            &self.cas_root,
            cas_store::InterventionKind::Rebind,
            format!("recover-delivery|{id}|{head}"),
            Some(id),
            Some(&supervisor.id),
            Some(reason),
        );
        Ok(Self::success(format!(
            "Task {id} integrated delivery recovered at {head}. Record independent batch evidence, then retry ordinary close."
        )))
    }
}
