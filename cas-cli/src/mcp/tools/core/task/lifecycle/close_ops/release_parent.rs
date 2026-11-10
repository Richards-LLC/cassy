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
        if task.status == TaskStatus::Closed {
            if let Err(error) = cas_store::release_reconcile_store::retire_closed_parent_dispatch(&self.cas_root, id, &inventory.scope.final_release_sha) {
                return held(format!("RELEASE_PARENT_DISPATCH_CLEANUP: {error}"), task.epic_verification_owner.clone());
            }
            return ParentOutcome::Closed;
        }
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
                if crate::qa_batch::release_close::parent_review(&self.cas_root, id).is_some()
                    && !crate::qa_batch::release_close::parent_dispatch_covered(&self.cas_root, id) { return Err("RELEASE_PARENT_VERIFICATION_BOUNDARY_CHANGED".into()); }
                let request = TaskCloseRequest {
                    id: id.into(), reason: Some(format!("Release {} at {}: independently reviewed parent acceptance and complete shipped child outcomes.", inventory.scope.release_id, inventory.scope.final_release_sha)),
                    commit_receipt: Some(inventory.scope.final_release_sha.clone()),
                    supervisor_override: None, no_delivery: None, stranded_branch_override: None, search_manifest: None,
                };
                let response = as_cassy_merge_close(crate::async_runtime::heap_future(|| self.cas_task_close(Parameters(request)))).await
                    .map_err(|e| e.to_string())?;
                if store.get(id).map_err(|e| e.to_string())?.status == TaskStatus::Closed {
                    cas_store::release_reconcile_store::retire_closed_parent_dispatch(&self.cas_root, id, &inventory.scope.final_release_sha).map_err(|e| e.to_string())?;
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
                if dispatch.deadline_at <= chrono::Utc::now() { return Err(format!("RELEASE_PARENT_VERIFICATION_EXPIRED: {}", dispatch.id)); }
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
        if dispatch.repository != boundary.repository { return Err("RELEASE_PARENT_VERIFICATION_BOUNDARY_CHANGED".into()); }
        Ok(Some(ParentOutcome::Pending { dispatch_id: dispatch.id, owner: dispatch.owner_agent_id }))
    }
}

fn held(gate: String, owner: Option<String>) -> ParentOutcome {
    ParentOutcome::Held { gate, owner, next_action: "Resolve the named gate, refresh the inventory, and replay this release operation.".into() }
}

#[cfg(test)]
pub(super) async fn probe_deleted_parent_and_single_pending_cycle(
    core: &CasCore, root: &std::path::Path, repo: &std::path::Path,
    scope: &crate::qa_batch::ReleaseScope, leaf: &str,
) {
    use cas_types::{Agent, AgentRole, AgentStatus, Dependency, DependencyType, WorkTarget};
    let tasks = core.open_task_store().unwrap();
    let agents = core.open_agent_store().unwrap();
    let mut retired = Agent::new_with_role("retired-parent-owner".into(), "retired-owner".into(), AgentRole::Supervisor);
    retired.status = AgentStatus::Shutdown;
    agents.register(&retired).unwrap();
    let mut parents = Vec::new();
    for id in ["cas-release-epic", "cas-release-train", "cas-release-pending"] {
        let mut parent = Task::new(id.into(), id.into());
        parent.task_type = TaskType::Epic;
        parent.branch = Some(format!("epic/deleted-{id}"));
        parent.epic_verification_owner = Some(retired.id.clone());
        parent.deliverables.work_target = Some(WorkTarget { repo_selector: format!("project:{}", scope.project_id), target_branch: "main".into() });
        parent.notes = format!("ASSEMBLY_PROOF: head={} result=PASS command=fixture log=fixture.log", scope.final_release_sha);
        tasks.add(&parent).unwrap();
        parents.push(parent);
    }
    for (child, parent) in [(leaf, "cas-release-epic"), ("cas-release-epic", "cas-release-train"), (leaf, "cas-release-pending")] {
        tasks.add_dependency(&Dependency::new(child.into(), parent.into(), DependencyType::ParentChild)).unwrap();
    }
    let mut review_scope = scope.clone();
    review_scope.constituent_epic_ids = vec!["cas-release-train".into(), "cas-release-pending".into()];
    let mut manifest = serde_json::json!({
        "project_id": scope.project_id, "release_id": scope.release_id,
        "reviewed_sha": scope.reviewed_sha, "final_release_sha": scope.final_release_sha,
        "epics": [
            {"task_id": parents[0].id, "generation": parents[0].updated_at.to_rfc3339(), "child_ids":[leaf], "acceptance":"pass"},
            {"task_id": parents[1].id, "generation": parents[1].updated_at.to_rfc3339(), "child_ids":[parents[0].id], "acceptance":"pass"},
            // A signed row with an incomplete child set grants no acceptance.
            {"task_id": parents[2].id, "generation": parents[2].updated_at.to_rfc3339(), "child_ids":[], "acceptance":"pass"}
        ]
    });
    let ledger = repo.join("parent-release-review.md");
    std::fs::write(&ledger, format!("# Parent acceptance\n{leaf} PASS\n```cassy-release-epics\n{manifest}\n```\n")).unwrap();
    let artifacts = cas_store::SqliteArtifactStore::open(root).unwrap();
    let context = crate::artifacts::PublishContext {
        store: &artifacts, roots: crate::artifacts::paths::PublishRoots::new(root, &repo.join("artifacts"), leaf), task_id: leaf.into(), cloud: None,
    };
    let artifact = crate::artifacts::publish(&context, &ledger).unwrap().artifact;
    let receipt = crate::qa_batch::prepare_receipt(root, &agents.get("supervisor-id").unwrap(), "supervisor-id", &[leaf.into()], &scope.reviewed_sha, &artifact.id, &ledger, cas_store::RELEASE_VERDICT_SCHEMA).unwrap();
    cas_store::record_qa_batch_receipt(root, &receipt).unwrap();
    review_scope.proof_refs = vec![crate::qa_batch::ReleaseProofRef {
        kind: crate::qa_batch::ReleaseProofKind::Review, artifact_id: artifact.id, sha256: artifact.sha256,
        size_bytes: artifact.size_bytes, reviewed_sha: scope.reviewed_sha.clone(), receipt_id: Some(receipt.id),
    }];
    let inventory = crate::qa_batch::resolve_release_inventory(root, repo, &review_scope).unwrap();
    let pending = core.close_release_parent(&inventory, "cas-release-pending").await;
    let ParentOutcome::Pending { dispatch_id, owner } = pending else { panic!("{pending:?}"); };
    assert_eq!(owner, "supervisor-id");
    let again = core.close_release_parent(&inventory, "cas-release-pending").await;
    assert!(matches!(again, ParentOutcome::Pending { dispatch_id: ref id, .. } if id == &dispatch_id));
    for id in ["cas-release-epic", "cas-release-train"] {
        let outcome = core.close_release_parent(&inventory, id).await;
        assert_eq!(outcome, ParentOutcome::Closed, "{id}: {outcome:?}");
        let closed = tasks.get(id).unwrap();
        assert_eq!(closed.status, TaskStatus::Closed);
        assert!(closed.epic_verification_owner.as_deref().unwrap().starts_with("release:"));
        assert!(cas_store::get_latest_verification_dispatch(root, id).unwrap().is_none());
    }
    // Acceptance arriving after the pending checkpoint consumes that same
    // release boundary; no second verifier or timeout wake survives the close.
    manifest["epics"][2]["child_ids"] = serde_json::json!([leaf]);
    let accepted_ledger = repo.join("parent-release-accepted.md");
    std::fs::write(&accepted_ledger, format!("# Parent acceptance\n{leaf} PASS\n```cassy-release-epics\n{manifest}\n```\n")).unwrap();
    let accepted_artifact = crate::artifacts::publish(&context, &accepted_ledger).unwrap().artifact;
    let accepted = crate::qa_batch::prepare_receipt(root, &agents.get("supervisor-id").unwrap(), "supervisor-id", &[leaf.into()], &scope.reviewed_sha, &accepted_artifact.id, &accepted_ledger, cas_store::RELEASE_VERDICT_SCHEMA).unwrap();
    cas_store::record_qa_batch_receipt(root, &accepted).unwrap();
    review_scope.proof_refs = vec![crate::qa_batch::ReleaseProofRef {
        kind: crate::qa_batch::ReleaseProofKind::Review, artifact_id: accepted_artifact.id, sha256: accepted_artifact.sha256,
        size_bytes: accepted_artifact.size_bytes, reviewed_sha: scope.reviewed_sha.clone(), receipt_id: Some(accepted.id),
    }];
    let resumed = crate::qa_batch::resolve_release_inventory(root, repo, &review_scope).unwrap();
    let completed = core.close_release_parent(&resumed, "cas-release-pending").await;
    assert_eq!(completed, ParentOutcome::Closed, "{completed:?}");
    let retired = cas_store::get_latest_verification_dispatch(root, "cas-release-pending").unwrap().unwrap();
    assert_eq!(retired.id, dispatch_id);
    assert_eq!(retired.state, cas_types::VerificationDispatchState::Invalidated);

}
