//! Factory loop v2 C2 (cas-223f, spec §4.3): the supervisor-stall delivery
//! evidence, computed off the loop.
//!
//! Every applied refresh asks whether the supervisor is stalled. For each
//! parked delivery in the focused epic that check used to run, on the daemon
//! loop, a task-store read, a QA pass read, `git rev-parse` and
//! `merge-base --is-ancestor` (twice), and on a CI cache miss `git` plus `gh`
//! with 2 s timeouts (measured: supervisor stall p99 1,820 ms on 10-15).
//!
//! The evidence now comes from two jobs, so slow external calls never hold
//! the shared read lane (review round 1, M1):
//! - [`StallEvidenceRequest::facts`] reads the store half ([`ParkedFacts`]:
//!   blocker and hold notes, open QA passes, the recorded delivery refs)
//!   inside the `DirectorSnapshot` and delivery reads, on the read lane.
//! - [`CiStatusRequest::run`] resolves the git half ([`GitEvidence`]: the
//!   delivery tip, lane CI via `gh`, whether the anchor is already on its
//!   target) as a `CiStatus` job on the external lane.
//!
//! The loop keeps the decision: [`combine`] joins the two in memory and the
//! stall check looks the result up. A parked delivery whose evidence has not
//! arrived, or was computed for different inputs, is
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
/// the `CiStatus` jobs.
pub(crate) type LaneCiCache = Arc<Mutex<HashMap<(String, String), (std::time::Instant, bool)>>>;

/// The store half of one parked delivery's evidence (read lane).
#[derive(Debug, Clone)]
pub(crate) struct ParkedFacts {
    task: TaskSummary,
    worker: String,
    target: Option<String>,
    /// A blocker note, hold decision or open QA pass (GH #896, cas-58d3).
    store_hold: Option<DeliveryHold>,
    /// The QA pass read failed: as before cas-223f, no hold and no CI check.
    qa_unreadable: bool,
    parked_branch: Option<String>,
    anchor: Option<String>,
    close_rejection: String,
}

/// Store facts by task id.
pub(crate) type ParkedFactsMap = HashMap<String, ParkedFacts>;

/// The git half of one parked delivery's evidence (external lane).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitEvidence {
    /// The facts this was computed from; different facts make it stale.
    inputs: (String, Option<String>, Option<String>, Option<String>),
    /// The delivery branch and commit a merge-now alert names (cas-58d3).
    delivery: (String, Option<String>),
    /// Lane CI still running (cas-207e).
    ci_pending: bool,
    /// The anchor is already on its target (merged, close blocked).
    merged_close_blocked: Option<MergedCloseBlockedTask>,
}

/// Git evidence by task id.
pub(crate) type GitEvidenceMap = HashMap<String, GitEvidence>;

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

impl ParkedFacts {
    fn inputs(&self) -> (String, Option<String>, Option<String>, Option<String>) {
        (
            self.worker.clone(),
            self.parked_branch.clone(),
            self.anchor.clone(),
            self.target.clone(),
        )
    }

    /// Whether the git half is needed at all: a store hold or an unreadable
    /// QA read already decides.
    fn needs_git(&self) -> bool {
        self.store_hold.is_none() && !self.qa_unreadable
    }
}

/// A parked delivery the facts are read for.
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

/// Everything the store facts need, captured on the loop.
#[derive(Debug, Clone)]
pub(crate) struct StallEvidenceRequest {
    cas_dir: PathBuf,
    epic_id: String,
    /// The parked deliveries in the loop's current snapshot, for a job that
    /// reads no new snapshot (the database is unchanged).
    parked: Vec<ParkedDelivery>,
}

impl StallEvidenceRequest {
    pub(crate) fn new(cas_dir: PathBuf, epic_id: String, current: &DirectorData) -> Self {
        let parked = parked_deliveries(current, &epic_id);
        Self {
            cas_dir,
            epic_id,
            parked,
        }
    }

    /// Read the store facts for every parked delivery, from `fresh` when the
    /// job read a new snapshot, otherwise from the loop's. Store reads only
    /// (task records and QA passes); no git, no network.
    pub(crate) fn facts(
        &self,
        fresh: Option<&DirectorData>,
        stores: Option<&DirectorStores>,
    ) -> ParkedFactsMap {
        let parked = match fresh {
            Some(data) => parked_deliveries(data, &self.epic_id),
            None => self.parked.clone(),
        };
        parked
            .into_iter()
            .map(|parked| {
                let facts = self.facts_for(parked, stores);
                (facts.task.id.clone(), facts)
            })
            .collect()
    }

    fn facts_for(&self, parked: ParkedDelivery, stores: Option<&DirectorStores>) -> ParkedFacts {
        let record = stores
            .and_then(|stores| cas_store::TaskStore::get(&stores.task_store, &parked.task.id).ok());
        let mut store_hold = None;
        if let Some(record) = &record {
            if blocker_note_after_park(&record.notes) {
                store_hold = Some(DeliveryHold::BlockerNote);
            } else if hold_decision_after_park(&record.notes) {
                // cas-58d3: a supervisor decision holding the merge.
                store_hold = Some(DeliveryHold::SupervisorDecision);
            }
        }
        let mut qa_unreadable = false;
        if store_hold.is_none() {
            let now = Utc::now();
            match cas_store::list_qa_passes(&self.cas_dir, &parked.task.id) {
                Ok(passes) => {
                    if passes
                        .into_iter()
                        .any(|pass| pass.state.is_active() && pass.deadline_at > now)
                    {
                        store_hold = Some(DeliveryHold::QaPassOpen);
                    }
                }
                Err(_) => qa_unreadable = true,
            }
        }
        let deliverables = record.as_ref().map(|record| &record.deliverables);
        ParkedFacts {
            worker: parked.worker,
            target: parked.target,
            store_hold,
            qa_unreadable,
            parked_branch: deliverables.and_then(|d| d.parked_branch.clone()),
            anchor: deliverables
                .and_then(|d| d.factory_branch_anchor.as_deref())
                .map(str::trim)
                .filter(|anchor| !anchor.is_empty())
                .map(str::to_string),
            close_rejection: record
                .as_ref()
                .and_then(|record| last_close_rejection_reason(&record.notes))
                .unwrap_or_else(|| "MERGE REQUIRED".to_string()),
            task: parked.task,
        }
    }
}

/// The `CiStatus` job: the git half of the evidence. Runs `git` and `gh`
/// (2 s timeouts each), so it belongs on the external lane, never the read
/// lane.
#[derive(Debug, Clone)]
pub(crate) struct CiStatusRequest {
    repo_root: PathBuf,
    facts: Vec<ParkedFacts>,
    lane_ci_pending: LaneCiCache,
}

impl CiStatusRequest {
    /// `None` when no parked delivery needs git evidence.
    pub(crate) fn new(
        repo_root: PathBuf,
        facts: &ParkedFactsMap,
        lane_ci_pending: LaneCiCache,
    ) -> Option<Self> {
        let facts: Vec<ParkedFacts> = facts.values().filter(|f| f.needs_git()).cloned().collect();
        (!facts.is_empty()).then_some(Self {
            repo_root,
            facts,
            lane_ci_pending,
        })
    }

    /// Resolve every delivery's tip, lane CI and merged state.
    pub(crate) fn run(&self) -> GitEvidenceMap {
        self.facts
            .iter()
            .map(|facts| (facts.task.id.clone(), self.evidence_for(facts)))
            .collect()
    }

    fn evidence_for(&self, facts: &ParkedFacts) -> GitEvidence {
        let repo_root = &self.repo_root;
        let delivery = task_delivery_ref(
            &facts.task.id,
            &facts.worker,
            facts.parked_branch.as_deref(),
            facts.anchor.as_deref(),
            |branch| crate::git_evidence::resolve_branch_sha(repo_root, branch),
            |anchor, tip| git_is_ancestor(repo_root, anchor, tip),
        );
        let ci_pending = self.delivery_ci_pending(facts, &delivery);
        let merged_close_blocked = if ci_pending {
            None
        } else {
            facts
                .target
                .as_deref()
                .and_then(|target| self.merged_close_blocked(facts, &delivery.0, target))
        };
        GitEvidence {
            inputs: facts.inputs(),
            delivery,
            ci_pending,
            merged_close_blocked,
        }
    }

    /// cas-207e: a parked delivery whose lane CI is still running is not a
    /// ready merge. Read through a cache so repeated checks stay cheap.
    fn delivery_ci_pending(&self, facts: &ParkedFacts, delivery: &(String, Option<String>)) -> bool {
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
        let target = facts.target.clone().unwrap_or_else(|| "main".to_string());
        // The lock is not held across git and gh.
        let pending =
            crate::mcp::tools::core::workflow::delivery_ci_pending(branch, &target, &self.repo_root);
        if let Ok(mut cache) = self.lane_ci_pending.lock() {
            cache.insert(key, (std::time::Instant::now(), pending));
        }
        pending
    }

    /// Merged-close-blocked evidence only when the delivery anchor is already
    /// reachable from the resolved target. Missing git evidence is unknown,
    /// which keeps the merge demand rather than inventing a merged verdict.
    fn merged_close_blocked(
        &self,
        facts: &ParkedFacts,
        factory_branch: &str,
        target_branch: &str,
    ) -> Option<MergedCloseBlockedTask> {
        let repo_root: &Path = &self.repo_root;
        let target_tip = crate::git_evidence::resolve_branch_sha(repo_root, target_branch)?;
        let anchor = facts.anchor.as_deref()?;
        let anchor_sha = crate::git_evidence::resolve_branch_sha(repo_root, anchor)?;
        if !git_is_ancestor(repo_root, &anchor_sha, &target_tip) {
            return None;
        }
        Some(MergedCloseBlockedTask {
            task_id: facts.task.id.clone(),
            factory_branch: factory_branch.to_string(),
            anchor: anchor.to_string(),
            target_branch: target_branch.to_string(),
            target_tip,
            close_rejection: facts.close_rejection.clone(),
        })
    }
}

/// Join the store facts with the git evidence, in memory. A delivery whose
/// git evidence is missing or was computed for other facts is
/// [`DeliveryHold::EvidencePending`].
pub(crate) fn combine(facts: &ParkedFactsMap, git: &GitEvidenceMap) -> DeliveryEvidenceMap {
    facts
        .iter()
        .map(|(task_id, facts)| {
            let pending_delivery = || (format!("factory/{}", facts.worker), None);
            let evidence = if let Some(hold) = facts.store_hold {
                DeliveryEvidence {
                    worker: facts.worker.clone(),
                    delivery: pending_delivery(),
                    hold: Some(hold),
                    merged_close_blocked: None,
                }
            } else if facts.qa_unreadable {
                // As before cas-223f: an unreadable QA read is no hold, and
                // CI is not consulted.
                DeliveryEvidence {
                    worker: facts.worker.clone(),
                    delivery: git
                        .get(task_id)
                        .filter(|found| found.inputs == facts.inputs())
                        .map(|found| found.delivery.clone())
                        .unwrap_or_else(pending_delivery),
                    hold: None,
                    merged_close_blocked: None,
                }
            } else {
                match git.get(task_id).filter(|found| found.inputs == facts.inputs()) {
                    Some(found) => DeliveryEvidence {
                        worker: facts.worker.clone(),
                        delivery: found.delivery.clone(),
                        hold: found.ci_pending.then_some(DeliveryHold::LaneCiPending),
                        merged_close_blocked: found.merged_close_blocked.clone(),
                    },
                    None => DeliveryEvidence {
                        worker: facts.worker.clone(),
                        delivery: pending_delivery(),
                        hold: Some(DeliveryHold::EvidencePending),
                        merged_close_blocked: None,
                    },
                }
            };
            (task_id.clone(), evidence)
        })
        .collect()
}

/// The stall classifiers over combined evidence: lookups only, no IO.
pub(crate) fn evidence_hold(
    evidence: &DeliveryEvidenceMap,
    task: &TaskSummary,
    worker: &str,
) -> Option<DeliveryHold> {
    match evidence.get(&task.id).filter(|found| found.worker == worker) {
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
