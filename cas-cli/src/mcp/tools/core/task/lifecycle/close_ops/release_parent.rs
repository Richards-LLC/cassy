//! Release-owned parent lifecycle; the ordinary close remains the terminal writer.
use super::*;
use crate::qa_batch::{ReleaseInventory, release_close::ReleaseClose};
use crate::release_reconcile::ParentOutcome;

impl CasCore {
    pub(crate) async fn close_release_parent(&self, inventory: &ReleaseInventory, id: &str) -> ParentOutcome {
        let store = match self.open_task_store() {
            Ok(store) => store,
            Err(error) => return held(error.to_string(), None),
        };
        let task = match store.get(id) {
            Ok(task) => task,
            Err(error) => return held(error.to_string(), None),
        };
        if task.status == TaskStatus::Closed { return ParentOutcome::Closed; }
        let owner = task.epic_verification_owner.clone();
        if task.task_type != TaskType::Epic || task.is_terminal() {
            return held("RELEASE_PARENT_NOT_LIVE_EPIC".into(), owner);
        }
        if let Err(gate) = crate::phase_outcome::check(&self.cas_root, store.as_ref(), id, true) {
            return held(gate, owner);
        }
        let result = async {
            let queue = crate::cloud::SyncQueue::open_read_only(&self.cas_root).map_err(|e| e.to_string())?;
            if queue.quarantined_ids(crate::cloud::QUARANTINE_TASK).map_err(|e| e.to_string())?.iter().any(|q| q == id) {
                return Err("RELEASE_PARENT_QUARANTINED".into());
            }
            let repo = resolve_close_gate_repo_root(&self.cas_root)?;
            let children = store.get_subtasks(id).map_err(|e| e.to_string())?;
            let context = ReleaseClose::prepare(&self.cas_root, &repo, inventory)?
                .for_parent(&task, &children).await?;
            context.scope(async {
                if crate::qa_batch::release_close::parent_review(&self.cas_root, id).is_none() {
                    if let Some(pending) = self.queue_release_epic_verification(&task, &repo, &inventory.scope.final_release_sha)? { return Ok(pending); }
                }
                let request = TaskCloseRequest {
                    id: id.into(), reason: Some(format!("Release {} at {}: independently reviewed parent acceptance and complete shipped child outcomes.", inventory.scope.release_id, inventory.scope.final_release_sha)),
                    commit_receipt: Some(inventory.scope.final_release_sha.clone()),
                    supervisor_override: None, no_delivery: None, stranded_branch_override: None, search_manifest: None,
                };
                let response = as_cassy_merge_close(crate::async_runtime::heap_future(|| self.cas_task_close(Parameters(request)))).await
                    .map_err(|e| e.to_string())?;
                if store.get(id).map_err(|e| e.to_string())?.status == TaskStatus::Closed {
                    Ok(ParentOutcome::Closed)
                } else {
                    let gate = response.content.iter().filter_map(|c| match &c.raw {
                        rmcp::model::RawContent::Text(t) => Some(t.text.as_str()), _ => None,
                    }).collect::<Vec<_>>().join("\n");
                    Ok(held(gate, owner.clone()))
                }
            }).await
        }.await;
        result.unwrap_or_else(|gate| held(gate, owner))
    }

    /// Missing acceptance queues the existing proof-cycle mechanism once. A
    /// retry or a replacement supervisor reports the same owner/cycle; it never
    /// impersonates the absent reviewer or reissues an expired dispatch.
    fn queue_release_epic_verification(&self, task: &Task, repo: &std::path::Path, final_sha: &str) -> Result<Option<ParentOutcome>, String> {
        let repository = super::super::repository_proof::capture_repository_proof_at_target(repo, final_sha, vec![])?;
        if let Some(dispatch) = cas_store::get_latest_verification_dispatch(&self.cas_root, &task.id).map_err(|e| e.to_string())? {
            if dispatch.repository.as_ref() != Some(&repository) {
                return Err(format!("RELEASE_PARENT_VERIFICATION_BOUNDARY_CHANGED: {}", dispatch.id));
            }
            if matches!(dispatch.state, cas_types::VerificationDispatchState::Pending | cas_types::VerificationDispatchState::Claimed) {
                return Ok(Some(ParentOutcome::Pending { dispatch_id: dispatch.id, owner: dispatch.owner_agent_id }));
            }
            if dispatch.state == cas_types::VerificationDispatchState::Resolved
                && cas_store::get_verification_for_dispatch(&self.cas_root, &dispatch.id).map_err(|e| e.to_string())?
                    .is_some_and(|v| v.verification_type == cas_types::VerificationType::Epic && v.status == cas_types::VerificationStatus::Approved)
            { return Ok(None); }
            return Err(format!("RELEASE_PARENT_VERIFICATION_REQUIRES_REVIEW: {} ({})", dispatch.id, dispatch.state));
        }
        let requester = self.get_agent_id().map_err(|e| e.to_string())?;
        let agents = self.open_agent_store().map_err(|e| e.to_string())?.list(None).map_err(|e| e.to_string())?;
        let recipient = crate::supervisor_notices::resolve(&agents, task.epic_verification_owner.as_deref(), None);
        if recipient.id == "operator" { return Err("RELEASE_PARENT_VERIFICATION_OWNER_UNAVAILABLE".into()); }
        let boundary = cas_types::VerificationProofBoundary::task_at(repository);
        let dispatch = match request_close_verification_dispatch(
            &self.cas_root, &task.id, &requester, &recipient.id, &boundary, false, None,
        ).map_err(|e| e.to_string())? {
            CloseVerificationDispatch::Requested(d) | CloseVerificationDispatch::AlreadyActive(d) => d,
        };
        Ok(Some(ParentOutcome::Pending { dispatch_id: dispatch.id, owner: dispatch.owner_agent_id }))
    }
}

fn held(gate: String, owner: Option<String>) -> ParentOutcome {
    ParentOutcome::Held { gate, owner, next_action: "Resolve the named gate, refresh the inventory, and replay this release operation.".into() }
}
