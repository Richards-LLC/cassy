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
const IDLE_RELAY_BACKOFF: chrono::Duration = chrono::Duration::minutes(30);
const UNBOUND_REMINDER_HORIZON: chrono::Duration = chrono::Duration::minutes(10);

#[derive(PartialEq, Eq)]
struct IdleSignal {
    task: String,
    tip: Option<String>,
    ci: &'static str,
}

struct Retry {
    event: DirectorEvent,
    next_probe: DateTime<Utc>,
    unknown_since: Option<(String, DateTime<Utc>)>,
}

#[derive(Default)]
pub(super) struct Retries {
    entries: HashMap<String, Retry>,
    idle_signals: HashMap<String, (IdleSignal, DateTime<Utc>)>,
    #[cfg(test)]
    pub now: Option<DateTime<Utc>>,
}

impl Retries {
    pub fn waiting(&mut self, worker: &str) {
        // A later completed wait is a changed state, even at the same tip.
        self.idle_signals.remove(worker);
    }

    /// Consume only a rendered, live relay. Suppressed waits never consume it.
    pub fn admit_idle(&mut self, worker: &str, read: &WorkerWait, now: DateTime<Utc>) -> bool {
        let signal = IdleSignal { task: read.task.clone(), tip: read.tip.clone(), ci: read.ci };
        if let Some((previous, sent_at)) = self.idle_signals.get(worker)
            && previous == &signal && now.signed_duration_since(*sent_at) < IDLE_RELAY_BACKOFF
        {
            return false;
        }
        self.idle_signals.insert(worker.to_string(), (signal, now));
        true
    }

    pub fn clock(&self) -> DateTime<Utc> {
        #[cfg(test)]
        if let Some(now) = self.now {
            return now;
        }
        Utc::now()
    }

    pub fn remember(&mut self, event: &DirectorEvent, now: DateTime<Utc>) {
        let DirectorEvent::WorkerIdle { active_task: Some(task), .. } = event else {
            return;
        };
        if !matches!(task.task_status, cas_types::TaskStatus::InProgress | cas_types::TaskStatus::AwaitingMerge) {
            return;
        }
        let entry = self
            .entries
            .entry(task.task_id.clone())
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
        self.idle_signals.retain(|worker, _| data.agents.iter().any(|agent| agent.name == *worker));
        self.entries.retain(|task_id, retry| {
            let DirectorEvent::WorkerIdle { worker, .. } = &retry.event else {
                return false;
            };
            data.in_progress_tasks.iter().any(|task| {
                task.id == *task_id
                    && matches!(task.status, cas_types::TaskStatus::InProgress | cas_types::TaskStatus::AwaitingMerge)
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

pub(super) struct WorkerWait {
    pub task: String,
    pub tip: Option<String>,
    pub ci: &'static str,
    pub waiting: bool,
}

/// A self-reminder is a promised wake, not a stalled worker. Old-task,
/// cancelled, fired and expired reminders cannot hide the current idle signal.
/// An unbound reminder covers only a near wake/expiry, never a distant promise.
pub(super) fn worker_has_pending_reminder(
    data: &DirectorData,
    worker: &str,
    task: Option<&str>,
    now: DateTime<Utc>,
) -> bool {
    let Some(agent) = data.agents.iter().find(|agent| agent.name == worker) else { return false; };
    let owns = |id: &str| id == agent.id || id == agent.name;
    data.reminders.iter().any(|reminder| {
        // cas-c53a: the store's TTL anchor, used by both checks below: a time
        // reminder counts from its due time, an event reminder from creation.
        let ttl_start = match reminder.trigger_type {
            cas_store::ReminderTriggerType::Time => reminder.trigger_at.unwrap_or(reminder.created_at),
            cas_store::ReminderTriggerType::Event => reminder.created_at,
        };
        let horizon = UNBOUND_REMINDER_HORIZON.num_seconds();
        let bound_or_soon = match reminder.task_id.as_deref() {
            Some(id) => Some(id) == task,
            None => match reminder.trigger_type {
                cas_store::ReminderTriggerType::Time => reminder.trigger_at.is_some_and(|due| {
                    (-horizon..=horizon).contains(&due.signed_duration_since(now).num_seconds())
                }),
                cas_store::ReminderTriggerType::Event => reminder.ttl_secs > 0 &&
                    (0..=horizon).contains(&reminder.ttl_secs.saturating_sub(
                        now.signed_duration_since(ttl_start).num_seconds(),
                    )),
            },
        };
        reminder.status == cas_store::ReminderStatus::Pending
            && owns(&reminder.owner_id) && owns(&reminder.target_id)
            && reminder.origin_session_id.as_deref().is_none_or(owns)
            && bound_or_soon
            && (reminder.ttl_secs == 0 || now.signed_duration_since(ttl_start).num_seconds() <= reminder.ttl_secs)
    })
}

/// Read the current delivery, not the remembered event's task. Local build
/// evidence comes from the registered clone's OS lane lock; CI is exact-tip.
/// Runs with the existing external delivery job, never on the control lane.
pub(super) fn read_worker_wait(
    event: &DirectorEvent,
    data: &DirectorData,
    cas_dir: &Path,
    repo: &Path,
) -> Option<(String, WorkerWait)> {
    let DirectorEvent::WorkerIdle { worker, active_task: Some(_), .. } = event else {
        return None;
    };
    // Match delivery-time rebinding: an old parked idle event can now refer
    // to the worker's next task, whose own tip is the only relevant CI.
    let agent = data.agents.iter().find(|agent| agent.name == *worker)?;
    let active = agent.active_lease.as_ref()?;
    let task = data.in_progress_tasks.iter().find(|task| {
        task.id == active.task_id && task.status == cas_types::TaskStatus::InProgress
            && task.assignee.as_deref().is_some_and(|assignee| {
                assignee == worker || data.agent_id_to_name.get(assignee) == Some(worker)
            })
    })?;
    let stored_agent = crate::store::open_agent_store(cas_dir).ok()?.get(&agent.id).ok()?;
    let build_pending = stored_agent.machine_id == Some(cas_types::Agent::get_or_generate_machine_id())
        && stored_agent.metadata.get("clone_path").is_some_and(|clone| {
            crate::factory_worker_check::lane_is_busy(cas_dir, Path::new(clone)).unwrap_or(false)
        });
    let mut read = WorkerWait { task: task.id.clone(), tip: None, ci: "unknown", waiting: build_pending };
    if build_pending { return Some((worker.clone(), read)); }
    let resolve = |branch: &str| crate::git_evidence::resolve_ref_commit_sha(repo, branch)
        .or_else(|| crate::git_evidence::resolve_ref_commit_sha(repo, &format!("origin/{branch}")));
    let parked = crate::store::open_task_store_cached(cas_dir).ok()
        .and_then(|store| store.get(&task.id).ok()).and_then(|task| task.deliverables.parked_branch);
    let (branch, tip) = crate::ui::factory::director::task_delivery_ref(
        &task.id, worker, parked.as_deref(), None, resolve, |_, _| false,
    );
    read.tip = tip;
    let Some(tip) = read.tip.as_deref() else { return Some((worker.clone(), read)); };
    if crate::git_evidence::fetch_parent_branch_result(repo, &branch)
        != crate::git_evidence::BoundedFetch::Succeeded
        || crate::git_evidence::resolve_ref_commit_sha(repo, &format!("origin/{branch}"))
            .as_deref() != Some(tip)
    {
        read.ci = "unpushed";
        return Some((worker.clone(), read));
    }
    read.ci = crate::mcp::tools::core::workflow::delivery_tip_ci_conclusion(tip, repo);
    // Do not borrow a running old tip if the worker committed during the probe.
    if resolve(&branch).as_deref() != Some(tip) { return None; }
    read.waiting = read.ci == "pending";
    Some((worker.clone(), read))
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
