//! Factory loop v2 C2 (cas-223f, spec §4.3): the supervisor-stall delivery
//! evidence, computed off the loop.
//!
//! Every applied refresh asks whether the supervisor is stalled. For each
//! parked delivery in the focused epic that check used to run, on the daemon
//! loop, a task-store read, a QA pass read, `git rev-parse` and
//! `merge-base --is-ancestor` (twice), and on a CI cache miss `git` plus `gh`
//! with 2 s timeouts (measured: supervisor stall p99 1,820 ms on 10-15).
//!
//! [`StallEvidenceRequest`] captures what that evidence needs on the loop,
//! and [`StallEvidenceRequest::evidence`] computes it inside the
//! `DirectorSnapshot` job (and inside the delivery read for revalidation).
//! The loop keeps the decision: it feeds the precomputed
//! [`DeliveryEvidence`] to `supervisor_actionable_state_with_classifiers`
//! through lookups. A parked delivery whose evidence has not arrived yet is
//! [`DeliveryHold::EvidencePending`], never "merge now".

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::Utc;

use super::{git_is_ancestor, last_close_rejection_reason};
use crate::ui::factory::director::{
    DeliveryHold, DirectorData, DirectorStores, MergedCloseBlockedTask, TaskSummary,
    blocker_note_after_park, hold_decision_after_park, merge_target_for_task, task_delivery_ref,
};

/// cas-207e: lane CI of parked deliveries by (branch, tip). A settled result
/// is final for its tip; a pending one is re-read after a minute. Shared with
/// the jobs that compute evidence.
pub(crate) type LaneCiCache = Arc<Mutex<HashMap<(String, String), (std::time::Instant, bool)>>>;

/// What the stall check needs to know about one parked delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeliveryEvidence {
    /// The worker the evidence was computed for; a reassigned task has none.
    pub(crate) worker: String,
    /// The delivery branch and commit a merge-now alert names (cas-58d3).
    pub(crate) delivery: (String, Option<String>),
    /// Why the delivery is deliberately not "merge now" (GH #896).
    pub(crate) hold: Option<DeliveryHold>,
    /// The delivery's anchor is already on its target (merged, close blocked).
    pub(crate) merged_close_blocked: Option<MergedCloseBlockedTask>,
}

/// Evidence by task id.
pub(crate) type DeliveryEvidenceMap = HashMap<String, DeliveryEvidence>;

/// A parked delivery the evidence is computed for.
#[derive(Debug, Clone)]
struct ParkedDelivery {
    task: TaskSummary,
    worker: String,
    target: Option<String>,
}

/// The parked deliveries of `epic_id` in `data`, as the stall check walks
/// them: AwaitingMerge children with an assignee.
fn parked_deliveries(data: &DirectorData, epic_id: &str) -> Vec<ParkedDelivery> {
    data.in_progress_tasks
        .iter()
        .filter(|task| {
            task.epic.as_deref() == Some(epic_id)
                && task.status == cas_types::TaskStatus::AwaitingMerge
        })
        .filter_map(|task| {
            let assignee = task.assignee.as_deref()?;
            let worker = data
                .agent_id_to_name
                .get(assignee)
                .map(String::as_str)
                .unwrap_or(assignee)
                .to_string();
            Some(ParkedDelivery {
                task: task.clone(),
                worker,
                target: merge_target_for_task(data, task),
            })
        })
        .collect()
}

/// Everything the evidence needs, captured on the loop.
#[derive(Debug, Clone)]
pub(crate) struct StallEvidenceRequest {
    cas_dir: PathBuf,
    repo_root: PathBuf,
    epic_id: String,
    /// The parked deliveries in the loop's current snapshot, for a job that
    /// reads no new snapshot (the database is unchanged).
    parked: Vec<ParkedDelivery>,
    lane_ci_pending: LaneCiCache,
}

impl StallEvidenceRequest {
    pub(crate) fn new(
        cas_dir: PathBuf,
        repo_root: PathBuf,
        epic_id: String,
        current: &DirectorData,
        lane_ci_pending: LaneCiCache,
    ) -> Self {
        let parked = parked_deliveries(current, &epic_id);
        Self {
            cas_dir,
            repo_root,
            epic_id,
            parked,
            lane_ci_pending,
        }
    }

    /// Compute the evidence for every parked delivery, from `fresh` when the
    /// job read a new snapshot, otherwise from the loop's. Runs git, gh and
    /// store reads: call it only off the loop (or from a caller that already
    /// reads synchronously).
    pub(crate) fn evidence(
        &self,
        fresh: Option<&DirectorData>,
        stores: Option<&DirectorStores>,
    ) -> DeliveryEvidenceMap {
        let parked = match fresh {
            Some(data) => parked_deliveries(data, &self.epic_id),
            None => self.parked.clone(),
        };
        parked
            .into_iter()
            .map(|parked| {
                let evidence = self.evidence_for(&parked, stores);
                (parked.task.id.clone(), evidence)
            })
            .collect()
    }

    fn evidence_for(
        &self,
        parked: &ParkedDelivery,
        stores: Option<&DirectorStores>,
    ) -> DeliveryEvidence {
        let task = &parked.task;
        let delivery = resolve_task_delivery(stores, task, &parked.worker, &self.repo_root);
        let hold = self.classify_delivery_hold(stores, parked, &delivery);
        let merged_close_blocked = if hold.is_none() {
            parked.target.as_deref().and_then(|target| {
                classify_merged_close_blocked_task(
                    stores,
                    task,
                    &delivery.0,
                    target,
                    &self.repo_root,
                )
            })
        } else {
            None
        };
        DeliveryEvidence {
            worker: parked.worker.clone(),
            delivery,
            hold,
            merged_close_blocked,
        }
    }

    /// GH #896 (cas-e4f8): whether a parked delivery is deliberately waiting
    /// on a decision, so the stall relay leaves it out of "merge now". Reads
    /// the task's notes (a blocker or hold decision raised after it last
    /// parked), the QA pass rows (an open pass whose deadline has not passed)
    /// and the delivery's lane CI (cas-207e).
    fn classify_delivery_hold(
        &self,
        stores: Option<&DirectorStores>,
        parked: &ParkedDelivery,
        delivery: &(String, Option<String>),
    ) -> Option<DeliveryHold> {
        let task = &parked.task;
        if let Some(stores) = stores
            && let Ok(record) = cas_store::TaskStore::get(&stores.task_store, &task.id)
        {
            if blocker_note_after_park(&record.notes) {
                return Some(DeliveryHold::BlockerNote);
            }
            // cas-58d3: a supervisor decision holding the merge.
            if hold_decision_after_park(&record.notes) {
                return Some(DeliveryHold::SupervisorDecision);
            }
        }
        let now = Utc::now();
        if cas_store::list_qa_passes(&self.cas_dir, &task.id)
            .ok()?
            .into_iter()
            .any(|pass| pass.state.is_active() && pass.deadline_at > now)
        {
            return Some(DeliveryHold::QaPassOpen);
        }
        self.delivery_ci_pending(parked, delivery)
            .then_some(DeliveryHold::LaneCiPending)
    }

    /// cas-207e: a parked delivery whose lane CI is still running is not a
    /// ready merge. Read through a cache so repeated checks stay cheap.
    fn delivery_ci_pending(
        &self,
        parked: &ParkedDelivery,
        delivery: &(String, Option<String>),
    ) -> bool {
        let (branch, Some(tip)) = delivery else {
            return false;
        };
        let key = (branch.clone(), tip.clone());
        if let Ok(cache) = self.lane_ci_pending.lock()
            && let Some((at, pending)) = cache.get(&key)
            && (!pending || at.elapsed() < std::time::Duration::from_secs(60))
        {
            return *pending;
        }
        let target = parked.target.clone().unwrap_or_else(|| "main".to_string());
        // The lock is not held across git and gh.
        let pending = crate::mcp::tools::core::workflow::delivery_ci_pending(
            branch,
            &target,
            &self.repo_root,
        );
        if let Ok(mut cache) = self.lane_ci_pending.lock() {
            cache.insert(key, (std::time::Instant::now(), pending));
        }
        pending
    }
}

/// cas-58d3: the delivery branch and commit a merge-now alert names for a
/// parked task, from its per-task branch, recorded parked branch and delivery
/// anchor rather than the worker's base branch.
fn resolve_task_delivery(
    stores: Option<&DirectorStores>,
    task: &TaskSummary,
    worker: &str,
    repo_root: &Path,
) -> (String, Option<String>) {
    let parked =
        stores.and_then(|stores| cas_store::TaskStore::get(&stores.task_store, &task.id).ok());
    let deliverables = parked.as_ref().map(|parked| &parked.deliverables);
    task_delivery_ref(
        &task.id,
        worker,
        deliverables.and_then(|d| d.parked_branch.as_deref()),
        deliverables.and_then(|d| d.factory_branch_anchor.as_deref()),
        |branch| crate::git_evidence::resolve_branch_sha(repo_root, branch),
        |anchor, tip| git_is_ancestor(repo_root, anchor, tip),
    )
}

/// Merged-close-blocked evidence for a parked task only when its delivery
/// anchor is already reachable from the resolved target. Missing task or git
/// evidence is unknown, which keeps the merge demand rather than inventing a
/// merged verdict.
fn classify_merged_close_blocked_task(
    stores: Option<&DirectorStores>,
    task: &TaskSummary,
    factory_branch: &str,
    target_branch: &str,
    repo_root: &Path,
) -> Option<MergedCloseBlockedTask> {
    let target_tip = crate::git_evidence::resolve_branch_sha(repo_root, target_branch)?;
    let stores = stores?;
    let parked = cas_store::TaskStore::get(&stores.task_store, &task.id).ok()?;
    let anchor = parked
        .deliverables
        .factory_branch_anchor
        .as_deref()
        .map(str::trim)
        .filter(|anchor| !anchor.is_empty())?;
    let anchor_sha = crate::git_evidence::resolve_branch_sha(repo_root, anchor)?;
    if !git_is_ancestor(repo_root, &anchor_sha, &target_tip) {
        return None;
    }
    Some(MergedCloseBlockedTask {
        task_id: task.id.clone(),
        factory_branch: factory_branch.to_string(),
        anchor: anchor.to_string(),
        target_branch: target_branch.to_string(),
        target_tip,
        close_rejection: last_close_rejection_reason(&parked.notes)
            .unwrap_or_else(|| "MERGE REQUIRED".to_string()),
    })
}

/// The stall classifiers over precomputed evidence: lookups only, no IO.
/// `(resolve_delivery, classify_merged_close_blocked, classify_delivery_hold)`
/// in the order `supervisor_actionable_state_with_classifiers` takes them.
pub(crate) fn evidence_hold(
    evidence: &DeliveryEvidenceMap,
    task: &TaskSummary,
    worker: &str,
) -> Option<DeliveryHold> {
    match evidence
        .get(&task.id)
        .filter(|found| found.worker == worker)
    {
        Some(found) => found.hold,
        None => Some(DeliveryHold::EvidencePending),
    }
}

pub(crate) fn evidence_delivery(
    evidence: &DeliveryEvidenceMap,
    task: &TaskSummary,
    worker: &str,
) -> (String, Option<String>) {
    evidence
        .get(&task.id)
        .filter(|found| found.worker == worker)
        .map(|found| found.delivery.clone())
        .unwrap_or_else(|| (format!("factory/{worker}"), None))
}

pub(crate) fn evidence_merged_close_blocked(
    evidence: &DeliveryEvidenceMap,
    task: &TaskSummary,
    target_branch: &str,
) -> Option<MergedCloseBlockedTask> {
    evidence
        .get(&task.id)
        .and_then(|found| found.merged_close_blocked.clone())
        .filter(|merged| merged.target_branch == target_branch)
}
