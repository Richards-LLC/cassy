//! Merge idle prompts require settled CI and live target evidence.
use super::{DirectorData, DirectorEvent};
use crate::ui::factory::director::{
    MergeAlertEvidence, MergeAlertFreshness, check_merge_alert_freshness,
};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::Path;

const RETRY_INTERVAL: chrono::Duration = chrono::Duration::minutes(1);
const UNKNOWN_AFTER: chrono::Duration = chrono::Duration::minutes(10);

struct Retry {
    event: DirectorEvent,
    next_probe: DateTime<Utc>,
    unknown_since: Option<(String, DateTime<Utc>)>,
}

#[derive(Default)]
pub(super) struct Retries {
    entries: HashMap<String, Retry>,
    #[cfg(test)]
    pub now: Option<DateTime<Utc>>,
}

impl Retries {
    pub fn clock(&self) -> DateTime<Utc> {
        #[cfg(test)]
        if let Some(now) = self.now {
            return now;
        }
        Utc::now()
    }

    pub fn remember(&mut self, event: &DirectorEvent, now: DateTime<Utc>) {
        let Some(task) = task_id(event) else {
            return;
        };
        let entry = self
            .entries
            .entry(task.to_string())
            .or_insert_with(|| Retry {
                event: event.clone(),
                next_probe: now + RETRY_INTERVAL,
                unknown_since: None,
            });
        entry.event = event.clone();
        entry.next_probe = now + RETRY_INTERVAL;
    }

    pub fn actionable(&mut self, ready: &Ready, now: DateTime<Utc>) -> bool {
        let Some(retry) = self.entries.get_mut(&ready.evidence.task_id) else {
            return false;
        };
        if ready.ci == "unknown" {
            if let Some((tip, since)) = &retry.unknown_since {
                if tip == &ready.tip {
                    return now.signed_duration_since(*since) >= UNKNOWN_AFTER;
                }
            }
            retry.unknown_since = Some((ready.tip.clone(), now));
            return false;
        }
        // A known pending answer is not a missing answer. It never times out
        // into a merge demand, and resets the unknown observation interval.
        retry.unknown_since = None;
        ready.ci != "pending"
    }

    pub fn due(&mut self, now: DateTime<Utc>, data: &DirectorData) -> Vec<DirectorEvent> {
        self.entries.retain(|task_id, retry| {
            let DirectorEvent::WorkerIdle { worker, .. } = &retry.event else {
                return false;
            };
            data.in_progress_tasks.iter().any(|task| {
                task.id == *task_id
                    && task.status == cas_types::TaskStatus::AwaitingMerge
                    && task.assignee.as_deref().is_some_and(|assignee| {
                        assignee == worker || data.agent_id_to_name.get(assignee) == Some(worker)
                    })
            })
        });
        self.entries
            .values_mut()
            .filter_map(|retry| {
                if now < retry.next_probe {
                    return None;
                }
                retry.next_probe = now + RETRY_INTERVAL;
                Some(retry.event.clone())
            })
            .collect()
    }
}

impl super::FactoryApp {
    pub(crate) fn merge_alert_retry_events_at(&mut self, now: DateTime<Utc>) -> Vec<DirectorEvent> {
        self.merge_alert_retries
            .due(now, &self.unfiltered_director_data)
    }
}

pub(super) struct Ready {
    pub evidence: MergeAlertEvidence,
    pub tip: String,
    pub ci: &'static str,
}

pub(super) fn task_id(event: &DirectorEvent) -> Option<&str> {
    match event {
        DirectorEvent::WorkerIdle {
            active_task: Some(task),
            ..
        } if task.task_status == cas_types::TaskStatus::AwaitingMerge => Some(&task.task_id),
        _ => None,
    }
}

/// Active-task idle events can be rebound to an in-progress delivery during
/// fresh revalidation, so they need the same external lane as merge reads.
pub(super) fn needs_ci_read(event: &DirectorEvent) -> bool {
    matches!(event, DirectorEvent::WorkerIdle { active_task: Some(_), .. })
}

/// Positive evidence only: the live worker/task and its exact pushed tip have
/// unfinished CI. Unknown, unpushed and settled states keep the lifecycle relay.
/// Runs with the existing delivery Git/CI job, never on the control/read lane.
pub(super) fn read_worker_ci_wait(
    event: &DirectorEvent,
    data: &DirectorData,
    repo: &Path,
) -> Option<(String, (String, String))> {
    let DirectorEvent::WorkerIdle { worker, active_task: Some(_), .. } = event else {
        return None;
    };
    // Match delivery-time rebinding: an old parked idle event can now refer
    // to the worker's next task, whose own tip is the only relevant CI.
    let active = data.agents.iter().find(|agent| agent.name == *worker)?
        .active_lease.as_ref()?;
    let task = data.in_progress_tasks.iter().find(|task| {
        task.id == active.task_id && task.status == cas_types::TaskStatus::InProgress
            && task.assignee.as_deref().is_some_and(|assignee| {
                assignee == worker || data.agent_id_to_name.get(assignee) == Some(worker)
            })
    })?;
    let resolve = |branch: &str| crate::git_evidence::resolve_ref_commit_sha(repo, branch)
        .or_else(|| crate::git_evidence::resolve_ref_commit_sha(repo, &format!("origin/{branch}")));
    let (branch, tip) = crate::ui::factory::director::task_delivery_ref(
        &task.id, worker, None, None, resolve, |_, _| false,
    );
    let tip = tip?;
    if crate::git_evidence::fetch_parent_branch_result(repo, &branch)
        != crate::git_evidence::BoundedFetch::Succeeded
        || crate::git_evidence::resolve_ref_commit_sha(repo, &format!("origin/{branch}"))
            .as_deref() != Some(tip.as_str())
    {
        return None;
    }
    let pending = crate::mcp::tools::core::workflow::delivery_tip_ci_conclusion(&tip, repo) == "pending";
    // Do not borrow a running old tip if the worker committed during the probe.
    (pending && resolve(&branch).as_deref() == Some(tip.as_str()))
        .then(|| (worker.clone(), (task.id.clone(), tip)))
}

/// Runs off the loop. No result means defer; it never consumes the dedupe key.
pub(super) fn read(event: &DirectorEvent, data: &DirectorData, repo: &Path) -> Option<Ready> {
    let MergeAlertFreshness::Fresh(evidence) = check_merge_alert_freshness(event, data, repo)
    else {
        return None;
    };
    let tip = evidence.factory_tip.clone();
    let ci = crate::mcp::tools::core::workflow::delivery_tip_ci_conclusion(&tip, repo);
    // cas-e48d: the target can gain this delivery while GitHub answers the
    // CI probe. Refresh containment too, so a completed-CI relay cannot quote
    // the pre-CI target. This remains one bounded exact-target check on the
    // external lane; an unavailable target defers the demand.
    let MergeAlertFreshness::Fresh(evidence) = check_merge_alert_freshness(event, data, repo)
    else {
        return None;
    };
    // Never attach another tip's CI if the worker pushed during either probe.
    let current = crate::git_evidence::resolve_ref_commit_sha(repo, &evidence.factory_branch)
        .or_else(|| {
            crate::git_evidence::resolve_ref_commit_sha(
                repo,
                &format!("origin/{}", evidence.factory_branch),
            )
        })?;
    (tip == evidence.factory_tip && tip == current).then_some(Ready { evidence, tip, ci })
}
