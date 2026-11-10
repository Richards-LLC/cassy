//! Durable release completion. Byte publication and closeout have separate outcomes.
use std::collections::BTreeMap;
use std::path::Path;

use cas_store::release_reconcile_store::{ReleaseOperation, ReleaseOperationKey};
use serde::{Deserialize, Serialize};

use crate::issue_reconcile::IssueOutcome;
use crate::mcp::tools::core::task::lifecycle::merge_close::CassyMergeClose;
use crate::qa_batch::{ReleaseInventory, ReleaseScope};

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
    pub scope: ReleaseScope,
    pub triggers: Vec<ReconcileTrigger>,
    pub members: BTreeMap<String, CassyMergeClose>,
    pub issues: Vec<IssueOutcome>,
    pub parents: BTreeMap<String, ParentOutcome>,
    pub exceptions: Vec<crate::qa_batch::ReleaseInventoryException>,
    pub started_at: String,
    pub updated_at: String,
    pub complete: bool,
    pub notice_recipient: Option<String>,
    pub notice_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ParentOutcome {
    Closed,
    Pending { dispatch_id: String, owner: String },
    Held { gate: String, owner: Option<String>, next_action: String },
}

/// Only effect boundaries are replaceable; the identity lock and checkpoint
/// transactions are the production store in both replay tests and adapters.
trait ReleaseEffects {
    async fn deliveries(
        &mut self,
        inventory: &ReleaseInventory,
        checkpoint: &mut impl FnMut(&str, CassyMergeClose) -> Result<()>,
    ) -> Result<()>;
    fn issues(&mut self, inventory: &ReleaseInventory, members: &BTreeMap<String, CassyMergeClose>) -> Vec<IssueOutcome>;
    async fn parents(&mut self, inventory: &ReleaseInventory, checkpoint: &mut impl FnMut(&str, ParentOutcome) -> Result<()>) -> Result<()>;
    fn notice(&mut self, receipt: &ReleaseReconcileReceipt) -> Result<String>;
}

async fn resume(
    root: &Path,
    scope: &ReleaseScope,
    inventory: &ReleaseInventory,
    trigger: ReconcileTrigger,
    effects: &mut impl ReleaseEffects,
) -> Result<ReleaseReconcileReceipt> {
    let key = ReleaseOperationKey {
        project_id: scope.project_id.clone(), release_id: scope.release_id.clone(),
        final_release_sha: scope.final_release_sha.clone(),
    };
    let now = chrono::Utc::now().to_rfc3339();
    Ok(ReleaseReconcileReceipt {
        key, scope: scope.clone(), triggers: vec![trigger], members: BTreeMap::new(),
        issues: vec![], parents: BTreeMap::new(), exceptions: inventory.exceptions.clone(),
        started_at: now.clone(), updated_at: now, complete: false,
        notice_recipient: None, notice_fingerprint: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    struct Effects {
        closed: BTreeSet<String>,
        crash: bool,
        actual_closes: usize,
        parent_passes: usize,
        notices: usize,
        owner: String,
    }
    impl ReleaseEffects for Effects {
        async fn deliveries(&mut self, inventory: &ReleaseInventory, checkpoint: &mut impl FnMut(&str, CassyMergeClose) -> Result<()>) -> Result<()> {
            for delivery in &inventory.deliveries {
                let id = &delivery.task_id;
                if self.closed.insert(id.clone()) { self.actual_closes += 1; }
                // Model a process death after the ordinary core commits a close,
                // but before its caller records the second member's result.
                if id == "cas-b" && std::mem::take(&mut self.crash) { return Err("injected_crash".into()); }
                checkpoint(id, CassyMergeClose::AlreadyClosed)?;
            }
            Ok(())
        }
        fn issues(&mut self, _: &ReleaseInventory, _: &BTreeMap<String, CassyMergeClose>) -> Vec<IssueOutcome> { vec![] }
        async fn parents(&mut self, _: &ReleaseInventory, checkpoint: &mut impl FnMut(&str, ParentOutcome) -> Result<()>) -> Result<()> {
            self.parent_passes += 1;
            checkpoint("cas-epic", ParentOutcome::Closed)
        }
        fn notice(&mut self, _: &ReleaseReconcileReceipt) -> Result<String> { self.notices += 1; Ok(self.owner.clone()) }
    }

    #[tokio::test]
    async fn release_replay_recovers_after_committed_effect_and_supervisor_turnover() {
        let temp = tempfile::tempdir().unwrap();
        let scope = ReleaseScope {
            project_id: "project-a".into(), repository: "org/repo".into(), release_id: "v9.99.0".into(),
            reviewed_sha: "a".repeat(40), final_release_sha: "a".repeat(40), promotion_pr: None,
            release_tag: Some("v9.99.0".into()), constituent_epic_ids: vec!["cas-epic".into()],
            included_task_ids: vec!["cas-a".into(), "cas-b".into()], no_code_members: BTreeMap::new(), proof_refs: vec![],
        };
        let inventory = ReleaseInventory {
            scope: scope.clone(), exceptions: vec![],
            deliveries: scope.included_task_ids.iter().map(|id| crate::qa_batch::ReleaseDelivery {
                task_id: id.clone(), generation: "generation-a".into(), anchor: Some("a".repeat(40)),
                base_sha: Some("b".repeat(40)), shipped_commit: Some("a".repeat(40)),
                membership: crate::qa_batch::ReleaseMembership::Ancestry, proof_refs: vec![],
            }).collect(),
        };
        let mut effects = Effects { closed: BTreeSet::new(), crash: true, actual_closes: 0, parent_passes: 0, notices: 0, owner: "retired-supervisor".into() };
        assert!(resume(temp.path(), &scope, &inventory, ReconcileTrigger::Installed, &mut effects).await.is_err());
        let key = ReleaseOperationKey { project_id: scope.project_id.clone(), release_id: scope.release_id.clone(), final_release_sha: scope.final_release_sha.clone() };
        let checkpoint = ReleaseOperation::open(temp.path(), key).unwrap().load().unwrap().unwrap();
        assert!(checkpoint["members"].get("cas-a").is_some());
        assert!(checkpoint["members"].get("cas-b").is_none());
        effects.owner = "replacement-supervisor".into();
        let recovered = resume(temp.path(), &scope, &inventory, ReconcileTrigger::Backfill, &mut effects).await.unwrap();
        assert!(recovered.complete);
        assert_eq!(recovered.members.len(), 2);
        assert_eq!(effects.actual_closes, 2);
        assert_eq!(effects.notices, 1);
        assert_eq!(recovered.notice_recipient.as_deref(), Some("replacement-supervisor"));
        let replay = resume(temp.path(), &scope, &inventory, ReconcileTrigger::Installed, &mut effects).await.unwrap();
        assert!(replay.complete);
        assert_eq!(effects.actual_closes, 2);
        assert_eq!(effects.notices, 1);
    }
}
