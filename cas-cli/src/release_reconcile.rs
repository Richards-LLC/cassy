//! Durable release completion. Byte publication and closeout have separate outcomes.
use std::collections::BTreeMap;
use std::path::Path;

use cas_store::release_reconcile_store::{ReleaseOperation, ReleaseOperationKey};
use serde::{Deserialize, Serialize};

use crate::issue_reconcile::IssueOutcome;
use crate::mcp::tools::core::task::lifecycle::merge_close::CassyMergeClose;
use crate::qa_batch::{ReleaseInventory, ReleaseScope};

tokio::task_local! { static RECONCILING: (); }

pub(crate) fn active() -> bool {
    RECONCILING.try_with(|_| ()).is_ok()
}

type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReconcileTrigger {
    Publish { tag: String },
    Promotion { pr: u64, merge_sha: String },
    Installed,
    Backfill,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReleaseReconcileReceipt {
    pub key: ReleaseOperationKey,
    #[serde(default)]
    pub reconciler_version: Option<String>,
    #[serde(default)]
    pub reconciler_stale: bool,
    pub scope: ReleaseScope,
    pub triggers: Vec<ReconcileTrigger>,
    pub members: BTreeMap<String, CassyMergeClose>,
    pub close_intents: BTreeMap<String, serde_json::Value>,
    pub issue_outcomes: Vec<IssueOutcome>,
    pub parents: BTreeMap<String, ParentOutcome>,
    pub exceptions: Vec<crate::qa_batch::ReleaseInventoryException>,
    pub started_at: String,
    pub updated_at: String,
    pub complete: bool,
    pub sync_pending: Vec<String>,
    pub residuals: Vec<ReleaseResidual>,
    pub notice_intent: Option<ReleaseNoticeIntent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReleaseResidual {
    pub task_id: String,
    pub gate: String,
    pub owner: String,
    pub next_action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReleaseNoticeIntent {
    pub key: String,
    pub recipient: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ParentOutcome {
    Closed,
    Pending {
        dispatch_id: String,
        owner: String,
    },
    Held {
        gate: String,
        owner: Option<String>,
        next_action: String,
    },
}

/// Only effect boundaries are replaceable; the identity lock and checkpoint
/// transactions are the production store in both replay tests and adapters.
trait ReleaseEffects {
    fn reconciler_version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }
    fn sync_pending(&self, _ids: &[String]) -> Result<Vec<String>> {
        Ok(vec![])
    }
    fn intent(&self, id: &str) -> Result<serde_json::Value>;
    async fn deliveries(
        &mut self,
        inventory: &ReleaseInventory,
        checkpoint: &mut impl FnMut(&str, CassyMergeClose) -> Result<()>,
    ) -> Result<()>;
    fn issues(
        &mut self,
        inventory: &ReleaseInventory,
        members: &BTreeMap<String, CassyMergeClose>,
    ) -> Vec<IssueOutcome>;
    async fn parents(
        &mut self,
        inventory: &ReleaseInventory,
        checkpoint: &mut impl FnMut(&str, ParentOutcome) -> Result<()>,
    ) -> Result<()>;
    fn recipient(&self) -> Result<String>;
    fn residuals(
        &self,
        members: &BTreeMap<String, CassyMergeClose>,
        parents: &BTreeMap<String, ParentOutcome>,
    ) -> Result<Vec<ReleaseResidual>> {
        outcome_residuals(members, parents, &self.recipient()?)
    }
}

fn retire_unproven_members(receipt: &mut ReleaseReconcileReceipt, inventory: &ReleaseInventory) {
    // Older discovery treated unshipped work as declared membership. Retire
    // only those unproven holds when the current inventory excludes them.
    // A saved close intent pins a real delivery cycle and is never discarded.
    let current_members: std::collections::BTreeSet<_> = inventory.deliveries.iter()
        .map(|delivery| delivery.task_id.as_str())
        .chain(inventory.exceptions.iter().map(|exception| exception.task_id.as_str()))
        .chain(inventory.scope.included_task_ids.iter().map(String::as_str))
        .chain(inventory.scope.no_code_members.keys().map(String::as_str))
        .collect();
    let discarded: std::collections::BTreeSet<_> = receipt.exceptions.iter()
        .filter(|exception| matches!(exception.code,
            crate::qa_batch::ReleaseInventoryCode::MissingAnchor
            | crate::qa_batch::ReleaseInventoryCode::MissingMapping
            | crate::qa_batch::ReleaseInventoryCode::MissingNoCodeProof))
        .filter(|exception| !current_members.contains(exception.task_id.as_str())
            && !receipt.close_intents.contains_key(&exception.task_id)
            && matches!(receipt.members.get(&exception.task_id), Some(CassyMergeClose::Held(_))))
        .map(|exception| exception.task_id.clone())
        .collect();
    receipt.members.retain(|id, _| !discarded.contains(id));
    receipt.sync_pending.retain(|id| !discarded.contains(id));
    receipt.residuals.retain(|residual| !discarded.contains(&residual.task_id));
}

async fn resume(
    root: &Path,
    scope: &ReleaseScope,
    inventory: &ReleaseInventory,
    trigger: ReconcileTrigger,
    effects: &mut impl ReleaseEffects,
) -> Result<ReleaseReconcileReceipt> {
    if serde_json::to_value(scope).map_err(|e| e.to_string())?
        != serde_json::to_value(&inventory.scope).map_err(|e| e.to_string())?
    {
        return Err(crate::gates::tag("G-128", "RELEASE_SCOPE_MISMATCH"));
    }
    let key = ReleaseOperationKey {
        project_id: scope.project_id.clone(),
        release_id: scope.release_id.clone(),
        final_release_sha: scope.final_release_sha.clone(),
    };
    let lock_root = root.to_path_buf();
    let lock_key = key.clone();
    let deadline = cas_store::wait_budget::wait_deadline();
    let operation = tokio::task::spawn_blocking(move || {
        let _budget = deadline.map(cas_store::wait_budget::bound_waits_until);
        ReleaseOperation::open(&lock_root, lock_key)
    })
    .await
    .map_err(|e| format!("RELEASE_OPERATION_LOCK_FAILED: {e}"))?
    .map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut receipt: ReleaseReconcileReceipt = match operation.load().map_err(|e| e.to_string())? {
        Some(saved) => serde_json::from_value(saved).map_err(|e| e.to_string())?,
        None => ReleaseReconcileReceipt {
            key,
            reconciler_version: None,
            reconciler_stale: false,
            scope: scope.clone(),
            triggers: vec![],
            members: BTreeMap::new(),
            close_intents: BTreeMap::new(),
            issue_outcomes: vec![],
            parents: BTreeMap::new(),
            exceptions: vec![],
            started_at: now.clone(),
            updated_at: now,
            complete: false,
            sync_pending: vec![],
            residuals: vec![],
            notice_intent: None,
        },
    };
    // Proof may be supplied on a recovery attempt. Membership and the reviewed
    // and final commits remain frozen for the lifetime of the operation.
    let immutable = |scope: &ReleaseScope| {
        let mut scope = scope.clone();
        scope.proof_refs.clear();
        scope.included_task_ids.extend(scope.no_code_members.keys().cloned());
        scope.included_task_ids.sort();
        scope.included_task_ids.dedup();
        scope.no_code_members.clear();
        serde_json::to_value(scope).map_err(|e| e.to_string())
    };
    if immutable(&receipt.scope)? != immutable(scope)? {
        return Err(crate::gates::tag("G-128", "RELEASE_IDENTITY_CHANGED"));
    }
    if !receipt.triggers.contains(&trigger) {
        receipt.triggers.push(trigger);
    }
    let save = |receipt: &ReleaseReconcileReceipt| {
        operation
            .save(&serde_json::to_value(receipt).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    };
    let tag_version = release_version(&receipt.scope)?;
    let stale = stale_reconciler(&receipt)?;
    let version = effects.reconciler_version();
    let current = semver::Version::parse(version)
        .map_err(|_| "RELEASE_RECONCILER_VERSION_INVALID".to_string())?;
    if tag_version.as_ref().is_some_and(|tag| &current < tag) {
        // Never overwrite progress made by a qualified binary with an older one.
        if stale {
            receipt.reconciler_version = Some(version.into());
            receipt.reconciler_stale = true;
            receipt.complete = false;
            save(&receipt)?;
        }
        return Err("RELEASE_RECONCILER_STALE".into());
    }
    if stale {
        // Reevaluate old decisions once under qualified code. Saved intents
        // continue to pin each delivery cycle; the core keeps closed tasks idempotent.
        receipt.complete = false;
        receipt.members.clear();
        receipt.parents.clear();
        receipt.issue_outcomes.clear();
        receipt.residuals.clear();
    }
    if receipt.complete {
        receipt.sync_pending = effects.sync_pending(
            &receipt
                .members
                .keys()
                .chain(receipt.parents.keys())
                .cloned()
                .collect::<Vec<_>>(),
        )?;
        save(&receipt)?;
        return Ok(receipt);
    }
    receipt.reconciler_version = Some(version.into());
    receipt.reconciler_stale = false;
    retire_unproven_members(&mut receipt, inventory);
    receipt.scope = scope.clone();
    receipt.exceptions = inventory.exceptions.clone();
    let successful = |outcome: &CassyMergeClose| {
        matches!(
            outcome,
            CassyMergeClose::Closed { .. } | CassyMergeClose::AlreadyClosed
        )
    };
    let mut pending = inventory.clone();
    pending.deliveries.retain(|delivery| {
        !receipt
            .members
            .get(&delivery.task_id)
            .is_some_and(&successful)
    });
    // The member core owns exception outcomes too: a closed task remains
    // AlreadyClosed even if its historical inventory has missing evidence.
    let mut refused = Vec::new();
    for delivery in &pending.deliveries {
        if refused.contains(&delivery.task_id) {
            continue;
        }
        let intent = effects.intent(&delivery.task_id)?;
        match receipt.close_intents.get(&delivery.task_id) {
            Some(original) if original != &intent => {
                refused.push(delivery.task_id.clone());
                receipt.members.insert(
                    delivery.task_id.clone(),
                    CassyMergeClose::Held("RELEASE_CLOSE_CYCLE_CHANGED".into()),
                );
            }
            Some(_) => {}
            None => {
                receipt
                    .close_intents
                    .insert(delivery.task_id.clone(), intent);
            }
        }
    }
    pending.deliveries.retain(|d| !refused.contains(&d.task_id));
    // Commit intent before effects. A process that dies after a member closes
    // resumes its exact cycle; the ordinary core recognizes its committed close.
    save(&receipt)?;
    effects
        .deliveries(&pending, &mut |id, outcome| {
            if refused.iter().any(|refused| refused == id)
                || receipt.members.get(id).is_some_and(&successful)
            {
                return Ok(());
            }
            receipt.members.insert(id.into(), outcome);
            receipt.updated_at = chrono::Utc::now().to_rfc3339();
            save(&receipt)
        })
        .await?;
    receipt.issue_outcomes = effects.issues(inventory, &receipt.members);
    save(&receipt)?;
    effects
        .parents(inventory, &mut |id, outcome| {
            receipt.parents.insert(id.into(), outcome);
            receipt.updated_at = chrono::Utc::now().to_rfc3339();
            save(&receipt)
        })
        .await?;
    receipt.sync_pending = effects.sync_pending(
        &receipt
            .members
            .keys()
            .chain(receipt.parents.keys())
            .cloned()
            .collect::<Vec<_>>(),
    )?;
    receipt.residuals = effects.residuals(&receipt.members, &receipt.parents)?;
    receipt.complete = receipt.residuals.is_empty()
        && receipt.members.values().all(successful)
        && receipt
            .parents
            .values()
            .all(|p| *p == ParentOutcome::Closed)
        && receipt.issue_outcomes.iter().all(|i| {
            matches!(
                i.action,
                crate::issue_reconcile::IssueAction::Closed
                    | crate::issue_reconcile::IssueAction::Skipped
            )
        });
    let fingerprint = outcome_fingerprint(&receipt)?;
    let recipient = match &receipt.notice_intent {
        Some(intent) => intent.recipient.clone(),
        None => effects.recipient()?,
    };
    // This is a persisted intent, never a claim that a notice was sent. The
    // publish/promotion adapter alone enqueues it with this stable dedupe key.
    receipt.notice_intent = Some(ReleaseNoticeIntent {
        key: format!(
            "release-closeout:{}",
            receipt.key.id().map_err(|e| e.to_string())?
        ),
        recipient,
        fingerprint,
    });
    receipt.updated_at = chrono::Utc::now().to_rfc3339();
    save(&receipt)?;
    Ok(receipt)
}

fn outcome_fingerprint(receipt: &ReleaseReconcileReceipt) -> Result<String> {
    use sha2::{Digest, Sha256};
    Ok(format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                &receipt.members,
                &receipt.issue_outcomes,
                &receipt.parents,
                &receipt.exceptions,
                &receipt.residuals,
                receipt.complete,
            ))
            .map_err(|e| e.to_string())?
        )
    ))
}

/// Every trusted publish/promotion/recovery adapter uses this operation. The
/// trigger identifies the event, while #2's validator owns proof authority.
// gate[G-128 release]: Release reconciler closes shipped work keyed on the release SHA
pub(crate) async fn reconcile_release(
    root: &Path,
    scope: &ReleaseScope,
    inventory: &ReleaseInventory,
    trigger: ReconcileTrigger,
) -> Result<ReleaseReconcileReceipt> {
    match &trigger {
        ReconcileTrigger::Publish { tag } if scope.release_tag.as_ref() != Some(tag) => {
            return Err(crate::gates::tag("G-128", "RELEASE_TRIGGER_TAG_MISMATCH"));
        }
        ReconcileTrigger::Promotion { pr, merge_sha }
            if scope.promotion_pr != Some(*pr) || merge_sha != &scope.final_release_sha =>
        {
            return Err(crate::gates::tag("G-128", "RELEASE_TRIGGER_PROMOTION_MISMATCH"));
        }
        _ => {}
    }
    let repo =
        crate::mcp::tools::core::task::lifecycle::close_ops::resolve_close_gate_repo_root(root)?;
    // Reconciliation writes task/sync outcomes; initialize optional queue tables
    // before issue, quarantine, or sync-state reads on a fresh project store.
    let queue = crate::cloud::SyncQueue::open(root).map_err(|e| e.to_string())?;
    queue.init().map_err(|e| e.to_string())?;
    let mut effects = CoreEffects {
        root,
        repo,
        core: crate::mcp::CasCore::for_cli(root.into()),
    };
    RECONCILING
        .scope((), resume(root, scope, inventory, trigger, &mut effects))
        .await
}

/// Preview the exact member decisions without operation, task, outbox or cache writes.
pub(crate) fn preview_release(
    root: &Path, repo: &Path, inventory: &ReleaseInventory,
) -> Result<BTreeMap<String, CassyMergeClose>> {
    crate::qa_batch::release_close::reconcile_members(root, repo, inventory, false, &mut |_, _| Ok(()))
}

/// The sole authoritative completed-operation lookup. Partial/held operations
/// return None so a trusted event can resume them; JSON exports are derived.
pub(crate) fn completed_operation(
    root: &Path,
    key: &ReleaseOperationKey,
) -> Result<Option<ReleaseReconcileReceipt>> {
    let saved = cas_store::release_reconcile_store::load_release_operation(root, key)
        .map_err(|e| e.to_string())?
        .map(|value| {
            serde_json::from_value::<ReleaseReconcileReceipt>(value).map_err(|e| e.to_string())
        })
        .transpose()?;
    match saved {
        Some(receipt) if receipt.complete && !stale_reconciler(&receipt)? => Ok(Some(receipt)),
        _ => Ok(None),
    }
}

fn release_version(scope: &ReleaseScope) -> Result<Option<semver::Version>> {
    scope.release_tag.as_deref().map(|tag| {
        semver::Version::parse(tag.strip_prefix('v').unwrap_or(tag))
            .map_err(|_| "RELEASE_TAG_VERSION_INVALID".to_string())
    }).transpose()
}

fn stale_reconciler(receipt: &ReleaseReconcileReceipt) -> Result<bool> {
    let Some(tag) = release_version(&receipt.scope)? else { return Ok(false); };
    Ok(receipt.reconciler_version.as_deref()
        .and_then(|version| semver::Version::parse(version).ok())
        .is_none_or(|version| version < tag))
}

fn outcome_residuals(
    members: &BTreeMap<String, CassyMergeClose>,
    parents: &BTreeMap<String, ParentOutcome>,
    owner: &str,
) -> Result<Vec<ReleaseResidual>> {
    let mut residuals = Vec::new();
    for (id, outcome) in members {
        let gate = match outcome {
            CassyMergeClose::Held(gate) => gate.clone(),
            CassyMergeClose::NotEligible => "RELEASE_CLOSE_NOT_ELIGIBLE".into(),
            _ => continue,
        };
        residuals.push(ReleaseResidual {
            task_id: id.clone(),
            gate,
            owner: owner.into(),
            next_action:
                "Resolve the named close gate, refresh the inventory, and replay the release."
                    .into(),
        });
    }
    for (id, outcome) in parents {
        let (gate, accountable, next_action) = match outcome {
            ParentOutcome::Closed => continue,
            ParentOutcome::Pending { dispatch_id, owner } => (
                format!("EPIC_VERIFICATION_PENDING:{dispatch_id}"),
                owner.clone(),
                "Complete the existing epic verification, then replay the release.".into(),
            ),
            ParentOutcome::Held {
                gate,
                owner: held_owner,
                next_action,
            } => (
                gate.clone(),
                held_owner.as_deref().unwrap_or(owner).into(),
                next_action.clone(),
            ),
        };
        residuals.push(ReleaseResidual {
            task_id: id.clone(),
            gate,
            owner: accountable,
            next_action,
        });
    }
    residuals.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    Ok(residuals)
}

/// Validate portable artifact identity without interpreting a report as review
/// approval. Ordinary no-code close still owns all verification/evidence gates.
pub(crate) fn validate_no_code_proofs(
    root: &Path,
    scope: &ReleaseScope,
    task_id: &str,
    proofs: &[crate::qa_batch::ReleaseProofRef],
) -> Result<()> {
    if proofs.is_empty() {
        return Err(format!("RELEASE_NO_CODE_PROOF_MISSING: {task_id}"));
    }
    use cas_store::TaskStore;
    let tasks = crate::qa_batch::release_close::read_tasks(root)?;
    for proof in proofs {
        let artifact = crate::qa_batch::release_close::read_artifact(root, &proof.artifact_id)
            .map_err(|e| format!("RELEASE_NO_CODE_PROOF_BINDING: {task_id}: {e}"))?;
        let owner = tasks.get(&artifact.task_id).map_err(|e| e.to_string())?;
        if proof.sha256.len() != 64 || !proof.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || proof.sha256 != artifact.sha256 || proof.size_bytes == 0
            || proof.size_bytes != artifact.size_bytes
            || !crate::mcp::tools::core::task::task_owned_by_project(&owner, root, Some(&scope.project_id))
            || (proof.kind == crate::qa_batch::ReleaseProofKind::Report && artifact.task_id != task_id)
        {
            return Err(format!("RELEASE_NO_CODE_PROOF_BINDING: {task_id}"));
        }
        crate::artifacts::proof::validate(&artifact, &artifact.task_id)
            .map_err(|e| format!("RELEASE_NO_CODE_PROOF_BINDING: {task_id}: {e}"))?;
    }
    Ok(())
}

struct CoreEffects<'a> {
    root: &'a Path,
    repo: std::path::PathBuf,
    core: crate::mcp::CasCore,
}

impl ReleaseEffects for CoreEffects<'_> {
    fn intent(&self, id: &str) -> Result<serde_json::Value> {
        let store = crate::store::open_task_store(self.root).map_err(|e| e.to_string())?;
        let task = match store.get(id) {
            Ok(task) => task,
            Err(error) => {
                return Ok(serde_json::json!({"task_id": id, "unavailable": error.to_string()}));
            }
        };
        let transaction =
            cas_store::get_latest_worker_delivery(self.root, id).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({
            "task_id": id, "assignee": task.assignee, "work_target": task.deliverables.work_target,
            "anchor": task.deliverables.factory_branch_anchor, "start": task.deliverables.factory_branch_start,
            "parked_branch": task.deliverables.parked_branch, "close_reason": task.deliverables.parked_close_reason,
            "transaction_id": transaction.map(|(_, t)| t.id),
        }))
    }
    async fn deliveries(
        &mut self,
        inventory: &ReleaseInventory,
        checkpoint: &mut impl FnMut(&str, CassyMergeClose) -> Result<()>,
    ) -> Result<()> {
        self.core.close_release_inventory_with_checkpoint(inventory, checkpoint).await?;
        Ok(())
    }
    fn issues(
        &mut self,
        inventory: &ReleaseInventory,
        members: &BTreeMap<String, CassyMergeClose>,
    ) -> Vec<IssueOutcome> {
        crate::issue_reconcile::reconcile_release_issues(self.root, &self.repo, inventory, members)
    }
    async fn parents(
        &mut self,
        inventory: &ReleaseInventory,
        checkpoint: &mut impl FnMut(&str, ParentOutcome) -> Result<()>,
    ) -> Result<()> {
        use cas_types::{DependencyType, TaskType};
        let store = crate::store::open_task_store(self.root).map_err(|e| e.to_string())?;
        let mut candidates = std::collections::BTreeSet::new();
        let mut pending = inventory.scope.constituent_epic_ids.clone();
        pending.extend(inventory.deliveries.iter().map(|d| d.task_id.clone()));
        pending.extend(inventory.scope.included_task_ids.iter().cloned());
        let mut seen = std::collections::BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Ok(task) = store.get(&id) {
                if task.task_type == TaskType::Epic {
                    candidates.insert(id.clone());
                }
                for dep in store.get_dependencies(&id).map_err(|e| e.to_string())? {
                    if dep.dep_type == DependencyType::ParentChild {
                        pending.push(dep.to_id);
                    }
                }
            }
        }
        fn visit(
            store: &dyn cas_store::TaskStore,
            id: &str,
            visiting: &mut std::collections::BTreeSet<String>,
            seen: &mut std::collections::BTreeSet<String>,
            order: &mut Vec<String>,
        ) -> Result<()> {
            if seen.contains(id) {
                return Ok(());
            }
            if !visiting.insert(id.into()) {
                return Err(crate::gates::tag("G-128", "RELEASE_PARENT_CYCLE"));
            }
            for child in store.get_subtasks(id).map_err(|e| e.to_string())? {
                if child.task_type == TaskType::Epic {
                    visit(store, &child.id, visiting, seen, order)?;
                }
            }
            visiting.remove(id);
            seen.insert(id.into());
            order.push(id.into());
            Ok(())
        }
        let mut order = vec![];
        let mut seen = std::collections::BTreeSet::new();
        for id in candidates {
            visit(
                store.as_ref(),
                &id,
                &mut std::collections::BTreeSet::new(),
                &mut seen,
                &mut order,
            )?;
        }
        // Members' closes changed their generation tokens. Re-resolve them;
        // never rebind the original anchors to an unrelated newer delivery.
        let current =
            crate::qa_batch::resolve_release_inventory(self.root, &self.repo, &inventory.scope)?;
        for id in order {
            let outcome = self.core.close_release_parent(&current, &id).await;
            checkpoint(&id, outcome)?;
        }
        Ok(())
    }
    fn sync_pending(&self, ids: &[String]) -> Result<Vec<String>> {
        let queue =
            crate::cloud::SyncQueue::open_read_only(self.root).map_err(|e| e.to_string())?;
        ids.iter()
            .filter_map(|id| {
                match queue.has_pending_entity_change(crate::cloud::EntityType::Task, id) {
                    Ok(true) => Some(Ok(id.clone())),
                    Ok(false) => None,
                    Err(e) => Some(Err(e.to_string())),
                }
            })
            .collect()
    }
    fn recipient(&self) -> Result<String> {
        let agents = crate::store::open_agent_store(self.root)
            .map_err(|e| e.to_string())?
            .list(None)
            .map_err(|e| e.to_string())?;
        Ok(crate::supervisor_notices::resolve(&agents, None, None).id)
    }
    fn residuals(
        &self,
        members: &BTreeMap<String, CassyMergeClose>,
        parents: &BTreeMap<String, ParentOutcome>,
    ) -> Result<Vec<ReleaseResidual>> {
        let owner = self.recipient()?;
        let mut residuals = outcome_residuals(members, parents, &owner)?;
        let store = crate::store::open_task_store(self.root).map_err(|e| e.to_string())?;
        residuals.retain(|r| !store.get(&r.task_id).is_ok_and(|t| t.is_terminal()));
        for id in members.keys().chain(parents.keys()) {
            if store.get(id).is_ok_and(|t| !t.is_terminal())
                && !residuals.iter().any(|r| &r.task_id == id)
            {
                residuals.push(ReleaseResidual { task_id: id.clone(), gate: "RELEASE_TASK_CHANGED_AFTER_CLOSE".into(), owner: owner.clone(), next_action: "Inspect the newer task cycle; this release will not close it from an old outcome.".into() });
            }
        }
        residuals.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        Ok(residuals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    struct Effects {
        version: String,
        closed: BTreeSet<String>,
        crash: bool,
        actual_closes: usize,
        parent_passes: usize,
        owner: String,
    }
    impl ReleaseEffects for Effects {
        fn reconciler_version(&self) -> &str { &self.version }
        fn intent(&self, id: &str) -> Result<serde_json::Value> {
            Ok(serde_json::json!({"cycle": id}))
        }
        async fn deliveries(
            &mut self,
            inventory: &ReleaseInventory,
            checkpoint: &mut impl FnMut(&str, CassyMergeClose) -> Result<()>,
        ) -> Result<()> {
            for delivery in &inventory.deliveries {
                // Force another event to enter while this operation owns its lock.
                tokio::task::yield_now().await;
                let id = &delivery.task_id;
                if self.closed.insert(id.clone()) {
                    self.actual_closes += 1;
                }
                // Model a process death after the ordinary core commits a close,
                // but before its caller records the second member's result.
                if id == "cas-b" && std::mem::take(&mut self.crash) {
                    return Err("injected_crash".into());
                }
                checkpoint(id, CassyMergeClose::AlreadyClosed)?;
            }
            Ok(())
        }
        fn issues(
            &mut self,
            _: &ReleaseInventory,
            _: &BTreeMap<String, CassyMergeClose>,
        ) -> Vec<IssueOutcome> {
            vec![]
        }
        async fn parents(
            &mut self,
            _: &ReleaseInventory,
            checkpoint: &mut impl FnMut(&str, ParentOutcome) -> Result<()>,
        ) -> Result<()> {
            self.parent_passes += 1;
            checkpoint("cas-epic", ParentOutcome::Closed)
        }
        fn recipient(&self) -> Result<String> {
            Ok(self.owner.clone())
        }
    }

    #[tokio::test]
    async fn newer_reconciler_replays_stale_operation_once_cas_cf7c() {
        let current = env!("CARGO_PKG_VERSION");
        // Missing stamps model receipts written before version tracking existed.
        for recorded in [None, Some("0.0.0"), Some(current), Some("999.0.0")] {
            let temp = tempfile::tempdir().unwrap();
            let scope = ReleaseScope {
                project_id: "project-a".into(), repository: "org/repo".into(),
                release_id: format!("v{current}"), release_tag: Some(format!("v{current}")),
                reviewed_sha: "a".repeat(40), final_release_sha: "a".repeat(40),
                promotion_pr: None, constituent_epic_ids: vec![],
                included_task_ids: vec!["cas-a".into()], no_code_members: BTreeMap::new(), proof_refs: vec![],
            };
            let inventory = ReleaseInventory { scope: scope.clone(), exceptions: vec![], deliveries: vec![
                crate::qa_batch::ReleaseDelivery {
                    task_id: "cas-a".into(), generation: "generation-a".into(),
                    anchor: Some("a".repeat(40)), base_sha: Some("b".repeat(40)),
                    shipped_commit: Some("a".repeat(40)),
                    membership: crate::qa_batch::ReleaseMembership::Ancestry, proof_refs: vec![],
                },
            ] };
            let mut effects = Effects {
                version: "0.0.0".into(), closed: BTreeSet::new(), crash: false,
                actual_closes: 0, parent_passes: 0, owner: "owner".into(),
            };
            let key = ReleaseOperationKey {
                project_id: scope.project_id.clone(), release_id: scope.release_id.clone(),
                final_release_sha: scope.final_release_sha.clone(),
            };
            // An installed binary older than the tag records its refusal without effects.
            assert_eq!(resume(temp.path(), &scope, &inventory, ReconcileTrigger::Installed, &mut effects)
                .await.unwrap_err(), "RELEASE_RECONCILER_STALE");
            assert_eq!(effects.parent_passes, 0);
            let old = cas_store::release_reconcile_store::load_release_operation(temp.path(), &key)
                .unwrap().unwrap();
            assert_eq!(old["reconciler_version"], "0.0.0");
            assert_eq!(old["reconciler_stale"], true);
            effects.version = current.into();
            let receipt = resume(temp.path(), &scope, &inventory, ReconcileTrigger::Installed, &mut effects)
                .await.unwrap();
            assert!(receipt.complete);
            // Replay a persisted historical completion, including pre-stamp rows.
            let mut saved = serde_json::to_value(&receipt).unwrap();
            saved.as_object_mut().unwrap().remove("reconciler_stale");
            if let Some(version) = recorded { saved["reconciler_version"] = version.into(); }
            else { saved.as_object_mut().unwrap().remove("reconciler_version"); }
            ReleaseOperation::open(temp.path(), key.clone()).unwrap().save(&saved).unwrap();
            let stale = recorded.is_none() || recorded == Some("0.0.0");
            assert_eq!(completed_operation(temp.path(), &key).unwrap().is_none(), stale);
            let before = effects.parent_passes;
            for _ in 0..2 {
                let replay = resume(temp.path(), &scope, &inventory, ReconcileTrigger::Backfill, &mut effects)
                    .await.unwrap();
                assert!(replay.complete);
                assert_eq!(replay.key, receipt.key);
                assert_eq!(replay.close_intents, receipt.close_intents);
                assert_eq!(replay.members.len(), 1);
                assert_eq!(replay.notice_intent.as_ref().unwrap().key, receipt.notice_intent.as_ref().unwrap().key);
            }
            assert_eq!(effects.parent_passes - before, usize::from(stale));
            assert_eq!(effects.actual_closes, 1);
            let final_receipt = completed_operation(temp.path(), &key).unwrap().unwrap();
            let persisted = serde_json::to_value(final_receipt).unwrap();
            assert_eq!(persisted["reconciler_version"], if stale { current } else { recorded.unwrap() });
            assert_eq!(persisted["reconciler_stale"], false);
        }
    }

    #[tokio::test]
    async fn release_replay_recovers_after_committed_effect_and_supervisor_turnover() {
        let temp = tempfile::tempdir().unwrap();
        let scope = ReleaseScope {
            project_id: "project-a".into(),
            repository: "org/repo".into(),
            release_id: "v9.99.0".into(),
            reviewed_sha: "a".repeat(40),
            final_release_sha: "a".repeat(40),
            promotion_pr: None,
            release_tag: Some("v9.99.0".into()),
            constituent_epic_ids: vec!["cas-epic".into()],
            included_task_ids: vec!["cas-a".into(), "cas-b".into()],
            no_code_members: BTreeMap::new(),
            proof_refs: vec![],
        };
        let inventory = ReleaseInventory {
            scope: scope.clone(),
            exceptions: vec![],
            deliveries: scope
                .included_task_ids
                .iter()
                .map(|id| crate::qa_batch::ReleaseDelivery {
                    task_id: id.clone(),
                    generation: "generation-a".into(),
                    anchor: Some("a".repeat(40)),
                    base_sha: Some("b".repeat(40)),
                    shipped_commit: Some("a".repeat(40)),
                    membership: crate::qa_batch::ReleaseMembership::Ancestry,
                    proof_refs: vec![],
                })
                .collect(),
        };
        let mut effects = Effects {
            version: "9.99.1".into(),
            closed: BTreeSet::new(),
            crash: true,
            actual_closes: 0,
            parent_passes: 0,
            owner: "retired-supervisor".into(),
        };
        assert!(
            resume(
                temp.path(),
                &scope,
                &inventory,
                ReconcileTrigger::Installed,
                &mut effects
            )
            .await
            .is_err()
        );
        let key = ReleaseOperationKey {
            project_id: scope.project_id.clone(),
            release_id: scope.release_id.clone(),
            final_release_sha: scope.final_release_sha.clone(),
        };
        let mut checkpoint = ReleaseOperation::open(temp.path(), key.clone())
            .unwrap()
            .load()
            .unwrap()
            .unwrap();
        assert!(checkpoint["members"].get("cas-a").is_some());
        assert!(checkpoint["members"].get("cas-b").is_none());
        // GH #1790: older inventory discovery persisted these unrelated holds.
        for id in ["cas-unshipped", "cas-unstarted", "cas-undeclared-report"] {
            checkpoint["members"][id] = serde_json::to_value(
                CassyMergeClose::Held("RELEASE_MEMBER_HELD".into())).unwrap();
        }
        checkpoint["exceptions"] = serde_json::json!([
            {"task_id": "cas-unshipped", "code": "missing_mapping"},
            {"task_id": "cas-unstarted", "code": "missing_anchor"},
            {"task_id": "cas-undeclared-report", "code": "missing_no_code_proof"}
        ]);
        ReleaseOperation::open(temp.path(), key).unwrap().save(&checkpoint).unwrap();
        assert!(
            completed_operation(
                temp.path(),
                &ReleaseOperationKey {
                    project_id: scope.project_id.clone(),
                    release_id: scope.release_id.clone(),
                    final_release_sha: scope.final_release_sha.clone()
                }
            )
            .unwrap()
            .is_none()
        );
        effects.owner = "replacement-supervisor".into();
        let recovered = resume(
            temp.path(),
            &scope,
            &inventory,
            ReconcileTrigger::Backfill,
            &mut effects,
        )
        .await
        .unwrap();
        assert!(recovered.complete);
        assert_eq!(recovered.members.len(), 2);
        assert_eq!(effects.actual_closes, 2);
        let intent = recovered.notice_intent.as_ref().unwrap();
        assert_eq!(intent.recipient, "replacement-supervisor");
        let mut outbox = BTreeSet::from([intent.key.clone()]);
        effects.owner = "another-supervisor".into();
        let replay = resume(
            temp.path(),
            &scope,
            &inventory,
            ReconcileTrigger::Installed,
            &mut effects,
        )
        .await
        .unwrap();
        assert!(replay.complete);
        assert_eq!(effects.actual_closes, 2);
        let replay_intent = replay.notice_intent.as_ref().unwrap();
        assert!(!outbox.insert(replay_intent.key.clone()));
        assert_eq!(replay_intent.recipient, intent.recipient);
        assert_eq!(effects.parent_passes, 1);
        assert!(
            completed_operation(temp.path(), &replay.key)
                .unwrap()
                .unwrap()
                .complete
        );
        // Two events for a new immutable release share one effect/checkpoint
        // cycle without blocking the current-thread runtime's lock holder.
        let mut concurrent_scope = scope.clone();
        concurrent_scope.release_id = "v9.99.1".into();
        let mut concurrent_inventory = inventory.clone();
        concurrent_inventory.scope = concurrent_scope.clone();
        let effects_for = |owner: &str| Effects {
            version: "9.99.1".into(),
            closed: BTreeSet::new(),
            crash: false,
            actual_closes: 0,
            parent_passes: 0,
            owner: owner.into(),
        };
        let mut first = effects_for("first-supervisor");
        let mut second = effects_for("second-supervisor");
        let (first_receipt, second_receipt) = tokio::join!(
            resume(
                temp.path(),
                &concurrent_scope,
                &concurrent_inventory,
                ReconcileTrigger::Installed,
                &mut first
            ),
            resume(
                temp.path(),
                &concurrent_scope,
                &concurrent_inventory,
                ReconcileTrigger::Backfill,
                &mut second
            ),
        );
        let first_receipt = first_receipt.unwrap();
        let second_receipt = second_receipt.unwrap();
        assert!(first_receipt.complete && second_receipt.complete);
        assert_eq!(first.actual_closes + second.actual_closes, 2);
        assert_eq!(first.parent_passes + second.parent_passes, 1);
        let first_intent = first_receipt.notice_intent.unwrap();
        let second_intent = second_receipt.notice_intent.unwrap();
        assert_eq!(first_intent.key, second_intent.key);
        assert_eq!(first_intent.recipient, second_intent.recipient);
    }
}
