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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::Utc;

use super::{git_is_ancestor, last_close_rejection_reason};
use super::assignment_evidence::{AssignmentEvidence, AssignmentRequest};
use crate::ui::factory::director::{
    DeliveryHold, DirectorData, DirectorStores, MergedCloseBlockedTask, TaskSummary,
    blocker_note_after_park, hold_decision_after_park, merge_target_for_task, task_delivery_ref,
};

/// cas-207e: lane CI of parked deliveries by (branch, tip). A settled result
/// is final for its tip; a pending one is re-read after a minute. Shared with
/// the `CiStatus` jobs.
pub(crate) type LaneCiCache = Arc<Mutex<HashMap<(String, String), (std::time::Instant, bool)>>>;

/// The full epic-status verdict for a parked delivery is immutable while its
/// delivery metadata and local/origin tips stay fixed. Unknowns are retried.
pub(crate) type DeliveryContainmentCache = Arc<Mutex<HashMap<String, bool>>>;

fn cached_containment(
    cache: &DeliveryContainmentCache,
    key: String,
    compute: impl FnOnce() -> Option<bool>,
) -> Option<bool> {
    if let Ok(entries) = cache.lock()
        && let Some(merged) = entries.get(&key)
    {
        return Some(*merged);
    }
    // No lock across Git or content proof work.
    let merged = compute()?;
    if let Ok(mut entries) = cache.lock() {
        if entries.len() >= 256 {
            entries.clear();
        }
        entries.insert(key, merged);
    }
    Some(merged)
}

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
    /// Preserve the same delivery/snapshot metadata epic_status evaluates.
    record: Option<cas_types::Task>,
}

/// Store facts by task id.
pub(crate) type ParkedFactsMap = HashMap<String, ParkedFacts>;

/// The git half of one parked delivery's evidence (external lane).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitEvidence {
    /// The facts this was computed from; different facts make it stale.
    inputs: (String, Option<String>, Option<String>, Option<String>, Option<String>),
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
    fn inputs(&self) -> (String, Option<String>, Option<String>, Option<String>, Option<String>) {
        (
            self.worker.clone(),
            self.parked_branch.clone(),
            self.anchor.clone(),
            self.target.clone(),
            self.record.as_ref().and_then(|record| {
                serde_json::to_string(&(record.status, &record.assignee, &record.deliverables)).ok()
            }),
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
    epic_id: Option<String>,
    session: String,
    /// The parked deliveries in the loop's current snapshot, for a job that
    /// reads no new snapshot (the database is unchanged).
    parked: Vec<ParkedDelivery>,
    assignment: Option<AssignmentRequest>,
}

impl StallEvidenceRequest {
    pub(crate) fn new(
        cas_dir: PathBuf,
        epic_id: Option<String>,
        session: String,
        current: &DirectorData,
    ) -> Self {
        let parked = epic_id
            .as_deref()
            .map(|epic_id| parked_deliveries(current, epic_id))
            .unwrap_or_default();
        let assignment = AssignmentRequest::new(cas_dir.clone(), current);
        Self { cas_dir, epic_id, session, parked, assignment }
    }

    pub(super) fn assignment_request(&self, fresh: Option<&DirectorData>) -> Option<AssignmentRequest> {
        match fresh {
            Some(data) => AssignmentRequest::new(self.cas_dir.clone(), data),
            None => self.assignment.clone(),
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
            Some(data) => self
                .epic_id
                .as_deref()
                .map(|epic_id| parked_deliveries(data, epic_id))
                .unwrap_or_default(),
            None => self.parked.clone(),
        };
        if parked.is_empty() {
            return HashMap::new();
        }
        // Synced tasks are not proof of local ownership. Read this on every
        // facts pass, including unchanged snapshots and delivery revalidation.
        let agents = match stores {
            Some(stores) => cas_store::AgentStore::list(&stores.agent_store, None).ok(),
            None => crate::store::open_agent_store(&self.cas_dir)
                .ok()
                .and_then(|store| store.list(None).ok()),
        };
        let (Some(agents), Ok(splits)) = (agents, crate::cloud::epic_split::current_epic_splits(&self.cas_dir))
        else {
            // Missing evidence remains pending, so it cannot demand a merge.
            return HashMap::new();
        };
        let machine = cas_types::Agent::get_or_generate_machine_id();
        let local_workers: HashSet<&str> = agents
            .iter()
            .filter(|agent| {
                agent.role == cas_types::AgentRole::Worker
                    && agent.visible_to_factory_session(Some(&self.session))
                    && agent.machine_id.as_deref() == Some(machine.as_str())
            })
            .flat_map(|agent| [agent.id.as_str(), agent.name.as_str()])
            .collect();
        let now = Utc::now();
        // Match the start/claim gate's binding rules, including cancellation
        // and expired proposals. Read all splits once per batch.
        let peer_tasks: HashSet<&str> = splits
            .iter()
            .filter(|split| split.is_binding(now))
            .flat_map(|split| split.theirs.iter().map(String::as_str))
            .collect();
        parked
            .into_iter()
            .map(|parked| {
                let outside_factory = peer_tasks.contains(parked.task.id.as_str())
                    || !parked
                        .task
                        .assignee
                        .as_deref()
                        .is_some_and(|id| local_workers.contains(id));
                let facts = self.facts_for(parked, stores, outside_factory);
                (facts.task.id.clone(), facts)
            })
            .collect()
    }

    fn facts_for(
        &self,
        parked: ParkedDelivery,
        stores: Option<&DirectorStores>,
        outside_factory: bool,
    ) -> ParkedFacts {
        let record = stores
            .and_then(|stores| cas_store::TaskStore::get(&stores.task_store, &parked.task.id).ok());
        let mut store_hold = outside_factory.then_some(DeliveryHold::OutsideFactory);
        if store_hold.is_none()
            && let Some(record) = &record
        {
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
            record: record.clone(),
            task: parked.task,
        }
    }
}

/// External stall evidence: delivery Git/CI plus ready-task ownership. Git,
/// `gh` and bounded peer-claim reads belong here, never on the shared read lane.
#[derive(Debug, Clone)]
pub(crate) struct CiStatusRequest {
    assignment: Option<AssignmentRequest>,
    repo_root: PathBuf,
    facts: Vec<ParkedFacts>,
    lane_ci_pending: LaneCiCache,
    containment: DeliveryContainmentCache,
}

pub(crate) struct StallExternalEvidence {
    pub(super) git: GitEvidenceMap,
    pub(super) assignment: Option<AssignmentEvidence>,
}

impl CiStatusRequest {
    /// `None` when no parked delivery needs git evidence.
    pub(crate) fn new(
        repo_root: PathBuf,
        facts: &ParkedFactsMap,
        lane_ci_pending: LaneCiCache,
        containment: DeliveryContainmentCache,
    ) -> Option<Self> {
        let facts: Vec<ParkedFacts> = facts.values().filter(|f| f.needs_git()).cloned().collect();
        (!facts.is_empty()).then_some(Self {
            assignment: None,
            repo_root,
            facts,
            lane_ci_pending,
            containment,
        })
    }

    pub(super) fn for_stall(
        repo_root: PathBuf, facts: &ParkedFactsMap, lane_ci_pending: LaneCiCache,
        containment: DeliveryContainmentCache, assignment: Option<AssignmentRequest>,
    ) -> Option<Self> {
        if assignment.is_none() {
            return Self::new(repo_root, facts, lane_ci_pending, containment);
        }
        Some(Self { repo_root, facts: facts.values().filter(|f| f.needs_git()).cloned().collect(),
            lane_ci_pending, containment, assignment })
    }

    /// Resolve assignments and deliveries off the loop, including when there
    /// are ready children but no parked deliveries.
    pub(crate) fn run(&self) -> StallExternalEvidence {
        StallExternalEvidence { assignment: self.assignment.as_ref().map(AssignmentRequest::read),
            git: self.run_git() }
    }

    fn run_git(&self) -> GitEvidenceMap {
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
        // A contained delivery no longer needs lane CI or another merge.
        let merged_close_blocked = facts.target.as_deref()
            .and_then(|target| self.merged_close_blocked(facts, &delivery, target));
        let ci_pending = merged_close_blocked.is_none() && self.delivery_ci_pending(facts, &delivery);
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

    /// Use the same full per-child containment computation as epic_status,
    /// including rewritten and intentionally evolved deliveries. Cache by
    /// immutable delivery metadata and every local/origin ref it measures.
    /// All of this runs on the external lane; the stall tick only does lookups.
    fn merged_close_blocked(
        &self,
        facts: &ParkedFacts,
        delivery: &(String, Option<String>),
        target_branch: &str,
    ) -> Option<MergedCloseBlockedTask> {
        let repo_root: &Path = &self.repo_root;
        let record = facts.record.as_ref()?;
        let target_branch = record.deliverables.work_target.as_ref()
            .map(|target| target.target_branch.trim()).filter(|target| !target.is_empty())
            .unwrap_or(target_branch);
        let resolve = |branch: &str| crate::git_evidence::resolve_branch_sha(repo_root, branch);
        let target_tip = resolve(target_branch).or_else(|| resolve(&format!("origin/{target_branch}")))?;
        let mut refs = vec![target_branch.to_string(), delivery.0.clone()];
        refs.extend(record.deliverables.parked_branch.clone());
        refs.extend(record.assignee.as_ref().map(|worker| format!("factory/{worker}")));
        refs.sort();
        refs.dedup();
        let tips: Vec<_> = refs.into_iter().flat_map(|branch| {
            let origin = format!("origin/{branch}");
            [(branch.clone(), resolve(&branch)), (origin.clone(), resolve(&origin))]
        }).collect();
        let key = serde_json::to_string(&(&self.repo_root, &record.id, facts.inputs(), &tips)).ok()?;
        let merged = cached_containment(&self.containment, key, || {
            use crate::mcp::tools::core::task::lifecycle::close_ops::{
                collect_epic_branch_statuses_with_options, EpicStatusOptions,
            };
            let collection = collect_epic_branch_statuses_with_options(
                std::slice::from_ref(record), target_branch, repo_root,
                EpicStatusOptions { offset: 0, limit: Some(1), summary: false, budget: std::time::Duration::from_secs(2) },
            );
            // Never store a verdict under tips that moved during its proof.
            if tips.iter().any(|(branch, tip)| resolve(branch) != *tip) {
                return None;
            }
            let row = collection.statuses.first()?;
            if row.refs_unresolved || row.content_check_error.is_some() {
                return None;
            }
            Some(row.unmerged_count == 0 && row.dropped_paths.is_empty())
        })?;
        if !merged {
            return None;
        }
        Some(MergedCloseBlockedTask {
            task_id: facts.task.id.clone(),
            factory_branch: delivery.0.clone(),
            anchor: facts.anchor.clone().or_else(|| delivery.1.clone())?,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cas_f439_containment_cache_retries_unknown_and_invalidates_changed_tips() {
        let cache = DeliveryContainmentCache::default();
        let computations = std::cell::Cell::new(0);
        let compute = |result| {
            computations.set(computations.get() + 1);
            result
        };
        assert_eq!(cached_containment(&cache, "delivery-a:target-a".into(), || compute(None)), None);
        assert_eq!(cached_containment(&cache, "delivery-a:target-a".into(), || compute(Some(true))), Some(true));
        assert_eq!(cached_containment(&cache, "delivery-a:target-a".into(), || panic!("unchanged tips must not walk Git again")), Some(true));
        assert_eq!(cached_containment(&cache, "delivery-b:target-a".into(), || compute(Some(false))), Some(false));
        assert_eq!(cached_containment(&cache, "delivery-b:target-b".into(), || compute(Some(true))), Some(true));
        assert_eq!(computations.get(), 4);
    }

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git").args(args).current_dir(repo).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn cas_f439_contained_deliveries_await_worker_close() {
        use crate::ui::factory::director::{
            SupervisorActionableState, SupervisorStallTracker, supervisor_actionable_state_with_classifiers,
        };
        use crate::mcp::tools::core::task::lifecycle::close_ops::collect_epic_branch_statuses;

        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-b", "main"]);
        git(repo, &["config", "user.name", "Fixture"]);
        git(repo, &["config", "user.email", "fixture@example.test"]);
        std::fs::write(repo.join("app"), "base\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-m", "base"]);
        git(repo, &["checkout", "-b", "factory/wa-cas-a"]);
        std::fs::write(repo.join("app"), "delivered\n").unwrap();
        git(repo, &["commit", "-am", "cas-a: delivery"]);
        let anchor = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "-b", "epic/x", "main"]);
        std::fs::write(repo.join("other"), "epic work\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-m", "other task"]);

        let mut facts = parked("cas-a", "wa", None);
        facts.anchor = Some(anchor.clone());
        let mut child = cas_types::Task::new("cas-a".into(), "delivery".into());
        child.status = cas_types::TaskStatus::AwaitingMerge;
        child.assignee = Some("wa".into());
        child.deliverables.parked_branch = facts.parked_branch.clone();
        child.deliverables.factory_branch_anchor = facts.anchor.clone();
        facts.record = Some(child.clone());
        let ci = LaneCiCache::default();
        let containment = DeliveryContainmentCache::default();
        ci.lock().unwrap().insert(("factory/wa-cas-a".into(), anchor.clone()), (std::time::Instant::now(), false));
        let classify = |facts: &ParkedFacts| {
            let request = CiStatusRequest::new(repo.into(), &HashMap::from([("cas-a".into(), facts.clone())]), Arc::clone(&ci), Arc::clone(&containment)).unwrap();
            let git = request.run().git;
            let evidence = combine(&HashMap::from([("cas-a".into(), facts.clone())]), &git);
            let mut data = DirectorData {
                ready_tasks: Vec::new(), in_progress_tasks: Vec::new(), epic_tasks: Vec::new(),
                agents: Vec::new(), activity: Vec::new(), agent_id_to_name: HashMap::new(),
                changes: Vec::new(), git_loaded: false, reminders: Vec::new(),
                epic_closed_counts: HashMap::new(), start_gated_task_ids: Default::default(),
            };
            data.in_progress_tasks.push(facts.task.clone());
            let mut epic = facts.task.clone();
            epic.id = "cas-epic".into();
            epic.status = cas_types::TaskStatus::InProgress;
            epic.task_type = cas_types::TaskType::Epic;
            epic.branch = Some("epic/x".into());
            data.epic_tasks.push(epic);
            supervisor_actionable_state_with_classifiers(&data, Some("cas-epic"), "supervisor", &Default::default(), Utc::now(), 600,
                |task, worker| evidence_delivery(&evidence, task, worker),
                |task, _, target, _| evidence_merged_close_blocked(&evidence, task, target),
                |task, worker| evidence_hold(&evidence, task, worker))
        };
        assert!(collect_epic_branch_statuses(&[child.clone()], "epic/x", repo)[0].unmerged_count > 0);
        assert!(matches!(classify(&facts), Some(SupervisorActionableState::MergeBranches { .. })), "genuinely unmerged delivery stays merge-now");

        git(repo, &["cherry-pick", &anchor]);
        // A rewritten delivery has the same app tree on its target, without
        // the reviewed anchor's ancestry. epic_status already reconciles it.
        let row = collect_epic_branch_statuses(&[child], "epic/x", repo).remove(0);
        assert_eq!(row.unmerged_count, 0, "fixture must match epic_status: {row:?}");
        let state = classify(&facts).expect("contained delivery needs a close notice");
        assert!(!matches!(state, SupervisorActionableState::MergeBranches { .. }), "contained delivery cannot demand another merge: {state:?}");
        assert!(state.next_step_text().contains("awaiting worker close"));
        let now = Utc::now();
        let mut tracker = SupervisorStallTracker::default();
        let activity = Some(now - chrono::Duration::seconds(600));
        assert!(tracker.observe(Some(state.clone()), activity, false, now, 600).wake.is_some());
        assert!(tracker.observe(Some(state.clone()), activity, false, now + chrono::Duration::seconds(1200), 600).wake.is_none());
        // Advancing unrelated epic work and supervisor tool activity cannot
        // create another close demand for the same contained delivery.
        std::fs::write(repo.join("other"), "later epic work\n").unwrap();
        git(repo, &["commit", "-am", "later task"]);
        let advanced = classify(&facts).unwrap();
        assert_eq!(state.merged_close_blocked_relay_key(), advanced.merged_close_blocked_relay_key());
        assert!(tracker.observe(Some(advanced), Some(now), false, now + chrono::Duration::seconds(1800), 600).wake.is_none());

        // Legacy tasks can lack an anchor while their actual delivery tip is merged.
        git(repo, &["merge", "-s", "ours", "--no-edit", "factory/wa-cas-a"]);
        facts.anchor = None;
        facts.record.as_mut().unwrap().deliverables.factory_branch_anchor = None;
        let state = classify(&facts).unwrap();
        assert!(!matches!(state, SupervisorActionableState::MergeBranches { .. }), "merged live tip cannot demand another merge: {state:?}");
    }

    fn parked(task_id: &str, worker: &str, store_hold: Option<DeliveryHold>) -> ParkedFacts {
        ParkedFacts {
            task: TaskSummary {
                id: task_id.into(),
                title: "parked".into(),
                status: cas_types::TaskStatus::AwaitingMerge,
                priority: cas_types::Priority::MEDIUM,
                assignee: Some(worker.into()),
                task_type: cas_types::TaskType::Task,
                epic: Some("cas-epic".into()),
                branch: None,
                updated_at: None,
                epic_verification_owner: None,
            },
            worker: worker.into(),
            target: Some("epic/x".into()),
            store_hold,
            qa_unreadable: false,
            parked_branch: Some(format!("factory/{worker}-{task_id}")),
            anchor: Some("abc123".into()),
            close_rejection: "MERGE REQUIRED".into(),
            record: None,
        }
    }

    #[test]
    fn cas_7809_stall_only_names_this_factorys_deliveries() {
        use crate::ui::factory::director::{
            SupervisorActionableState, supervisor_actionable_state_with_classifiers,
        };
        use cas_store::{
            AgentStore, EpicSplit, SplitDirection, SplitState, decide_epic_split, insert_epic_split,
        };
        use cas_types::{Agent, AgentRole, TaskStatus, TaskType};

        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::TempDir::new().unwrap();
        let stores = DirectorStores::open(dir.path()).unwrap();
        stores.agent_store.init().unwrap();
        let now = Utc::now();
        let machine = Agent::get_or_generate_machine_id();
        for (name, session, host, role) in [
            (
                "supervisor",
                "here",
                machine.as_str(),
                AgentRole::Supervisor,
            ),
            ("local", "here", machine.as_str(), AgentRole::Worker),
            // A split must win even if an old local worker registration remains.
            ("peer-kept", "here", machine.as_str(), AgentRole::Worker),
            (
                "other-session",
                "elsewhere",
                machine.as_str(),
                AgentRole::Worker,
            ),
            ("other-machine", "here", "peer-host", AgentRole::Worker),
        ] {
            let mut agent = Agent::new_with_role(format!("id-{name}"), name.into(), role);
            agent.factory_session = Some(session.into());
            agent.machine_id = Some(host.into());
            stores.agent_store.register(&agent).unwrap();
        }
        insert_epic_split(
            dir.path(),
            &EpicSplit {
                id: "split-owned".into(),
                epic_id: "cas-epic".into(),
                local_agent_id: "id-supervisor".into(),
                peer_agent_id: "peer-supervisor".into(),
                peer_name: "peer".into(),
                peer_machine: "peer-host".into(),
                direction: SplitDirection::Outgoing,
                mine: vec!["cas-local".into()],
                theirs: vec!["cas-peer-kept".into()],
                state: SplitState::Accepted,
                reason: None,
                message_id: None,
                expires_at: now,
                created_at: now,
                decided_at: Some(now),
                decided_by: None,
                cleanup_pending: false,
                expiry_notice_pending: false,
            },
        )
        .unwrap();
        let mut data = DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: Vec::new(),
            epic_tasks: Vec::new(),
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
            start_gated_task_ids: Default::default(),
        };
        let mut epic = parked("cas-epic", "local", None).task;
        epic.task_type = TaskType::Epic;
        epic.status = TaskStatus::InProgress;
        epic.epic = None;
        data.epic_tasks.push(epic);
        for worker in [
            "local",
            "peer-kept",
            "other-session",
            "other-machine",
            "unregistered",
        ] {
            let mut task = parked(&format!("cas-{worker}"), worker, None).task;
            // Exercise both canonical agent ids and the legacy display-name form.
            if worker == "local" {
                task.assignee = Some("id-local".into());
                data.agent_id_to_name
                    .insert("id-local".into(), "local".into());
            }
            data.in_progress_tasks.push(task);
        }
        let request = StallEvidenceRequest::new(
            dir.path().into(),
            Some("cas-epic".into()),
            "here".into(),
            &data,
        );
        let classify = |fresh| {
            let facts = request.facts(fresh, Some(&stores));
            let git = facts
                .iter()
                .map(|(id, fact)| {
                    (
                        id.clone(),
                        GitEvidence {
                            inputs: fact.inputs(),
                            delivery: (format!("factory/{}", fact.worker), Some("tip".into())),
                            ci_pending: false,
                            merged_close_blocked: None,
                        },
                    )
                })
                .collect();
            let evidence = combine(&facts, &git);
            supervisor_actionable_state_with_classifiers(
                &data,
                Some("cas-epic"),
                "supervisor",
                &Default::default(),
                now,
                600,
                |task, worker| evidence_delivery(&evidence, task, worker),
                |_, _, _, _| None,
                |task, worker| evidence_hold(&evidence, task, worker),
            )
        };
        // Exercise both a fresh read and an unchanged-snapshot read, as the daemon does.
        for fresh in [Some(&data), None] {
            assert_eq!(
                classify(fresh),
                Some(SupervisorActionableState::MergeBranches {
                    branches: vec![("cas-local".into(), "factory/local".into(), "tip".into())],
                }),
                "only this factory's delivery belongs in the stall relay"
            );
        }
        // Re-reading ownership releases a local delivery when the split ends,
        // even without refreshing the parked-task snapshot.
        assert!(
            decide_epic_split(
                dir.path(),
                "split-owned",
                &[SplitState::Accepted],
                SplitState::Cancelled,
                None,
                now,
            )
            .unwrap()
        );
        assert_eq!(
            classify(None),
            Some(SupervisorActionableState::MergeBranches {
                branches: vec![
                    ("cas-local".into(), "factory/local".into(), "tip".into()),
                    (
                        "cas-peer-kept".into(),
                        "factory/peer-kept".into(),
                        "tip".into()
                    ),
                ],
            })
        );
    }

    /// Review round 1, M1: the stall check joins store facts (read lane) with
    /// git evidence (external lane) in memory. Missing or mismatched git
    /// evidence is pending, never "merge now"; a store hold decides alone.
    #[test]
    fn c2_evidence_joins_store_facts_with_git_evidence() {
        let mut facts = ParkedFactsMap::new();
        facts.insert("cas-a".into(), parked("cas-a", "wa", None));
        facts.insert("cas-b".into(), parked("cas-b", "wb", None));
        facts.insert("cas-c".into(), parked("cas-c", "wc", Some(DeliveryHold::BlockerNote)));
        facts.insert("cas-d".into(), parked("cas-d", "wd", None));
        let mut git = GitEvidenceMap::new();
        git.insert(
            "cas-a".into(),
            GitEvidence {
                inputs: facts["cas-a"].inputs(),
                delivery: ("factory/wa-cas-a".into(), Some("tip-a".into())),
                ci_pending: false,
                merged_close_blocked: None,
            },
        );
        git.insert(
            "cas-b".into(),
            GitEvidence {
                inputs: facts["cas-b"].inputs(),
                delivery: ("factory/wb-cas-b".into(), Some("tip-b".into())),
                ci_pending: true,
                merged_close_blocked: None,
            },
        );
        // Computed for another worker: stale.
        let mut stale = facts["cas-d"].inputs();
        stale.0 = "someone-else".into();
        git.insert(
            "cas-d".into(),
            GitEvidence {
                inputs: stale,
                delivery: ("factory/x".into(), Some("tip-x".into())),
                ci_pending: false,
                merged_close_blocked: None,
            },
        );
        let combined = combine(&facts, &git);
        assert_eq!(combined["cas-a"].hold, None);
        assert_eq!(combined["cas-a"].delivery.1.as_deref(), Some("tip-a"));
        assert_eq!(combined["cas-b"].hold, Some(DeliveryHold::LaneCiPending));
        assert_eq!(combined["cas-c"].hold, Some(DeliveryHold::BlockerNote));
        assert_eq!(combined["cas-d"].hold, Some(DeliveryHold::EvidencePending));

        // Only deliveries without a store hold need the external job.
        let request = CiStatusRequest::new(PathBuf::from("/nonexistent"), &facts, LaneCiCache::default(), DeliveryContainmentCache::default())
            .expect("three deliveries need git evidence");
        assert_eq!(request.facts.len(), 3);
        assert!(CiStatusRequest::new(PathBuf::new(), &ParkedFactsMap::new(), LaneCiCache::default(), DeliveryContainmentCache::default()).is_none());
    }
}
