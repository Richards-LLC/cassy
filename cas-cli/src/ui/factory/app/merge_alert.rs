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
