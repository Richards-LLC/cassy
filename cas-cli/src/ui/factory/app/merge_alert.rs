//! Merge idle prompts require settled CI and live target evidence.
use super::{DirectorData, DirectorEvent};
use crate::ui::factory::director::{
    MergeAlertEvidence, MergeAlertFreshness, check_merge_alert_freshness,
};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

const RETRY_INTERVAL: chrono::Duration = chrono::Duration::minutes(1);
const UNKNOWN_AFTER: chrono::Duration = chrono::Duration::minutes(10);
const IDLE_RELAY_BACKOFF: chrono::Duration = chrono::Duration::minutes(30);
const UNBOUND_REMINDER_HORIZON: chrono::Duration = chrono::Duration::minutes(10);
const SETTLED_RETRY: chrono::Duration = chrono::Duration::minutes(10);

pub(super) type ProbeCache = Arc<Probes>;

#[derive(Debug, Default)]
pub(super) struct Probes {
    reads: Mutex<HashMap<String, ProbeRead>>,
    last_cleanup: Mutex<Option<DateTime<Utc>>>,
}

#[derive(Debug, Clone)]
pub(super) struct ProbeRead {
    fingerprint: String,
    checked_at: DateTime<Utc>,
    ready: Option<Ready>,
    wait: Option<(String, WorkerWait)>,
    ci: &'static str,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ReadyReceipt {
    evidence: MergeAlertEvidence,
    worker: Option<String>,
}

fn ready_receipt_path(cas_dir: &Path, task: &str, tip: &str) -> std::path::PathBuf {
    use sha2::{Digest, Sha256};
    let key = serde_json::to_vec(&(task, tip)).expect("string receipt key");
    cas_dir.join("receipts/merge-ready").join(format!("{:x}.json", Sha256::digest(&key)))
}

fn prior_ready(cas_dir: &Path, task: &str, tip: &str) -> Option<Ready> {
    let receipt: ReadyReceipt = serde_json::from_slice(
        &std::fs::read(ready_receipt_path(cas_dir, task, tip)).ok()?,
    ).ok()?;
    (receipt.evidence.task_id == task && receipt.evidence.factory_tip == tip).then(|| Ready {
        tip: tip.to_string(), ci: "success", evidence: receipt.evidence, worker: receipt.worker,
    })
}

/// Receipts stay while a merge/verdict is pending, then retire with the task.
/// This runs at most once per slow-retry interval on the external read lane.
pub(super) fn prune_ready_receipts(cache: &ProbeCache, cas_dir: &Path, now: DateTime<Utc>) {
    {
        let mut last = cache.last_cleanup.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if last.is_some_and(|last| now.signed_duration_since(last) < SETTLED_RETRY) { return; }
        *last = Some(now);
    }
    let Ok(entries) = std::fs::read_dir(cas_dir.join("receipts/merge-ready")) else { return; };
    let Ok(tasks) = crate::store::open_task_store_local(cas_dir) else { return; };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(receipt) = std::fs::read(&path).ok()
            .and_then(|bytes| serde_json::from_slice::<ReadyReceipt>(&bytes).ok()) else { continue; };
        if tasks.get(&receipt.evidence.task_id).is_ok_and(|task| task.is_terminal()) {
            let _ = std::fs::remove_file(path);
            cache.reads.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&receipt.evidence.task_id);
        }
    }
}

/// Atomic per-delivery admission, shared across daemon lifetimes. Failure to
/// write defers the signal instead of claiming a receipt that cannot survive.
pub(super) fn admit_ready(cas_dir: &Path, ready: &Ready) -> bool {
    use std::io::Write;
    let path = ready_receipt_path(cas_dir, &ready.evidence.task_id, &ready.tip);
    let write = || -> std::io::Result<bool> {
        let parent = path.parent().expect("receipt parent");
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        let receipt = ReadyReceipt { evidence: ready.evidence.clone(), worker: ready.worker.clone() };
        file.write_all(&serde_json::to_vec(&receipt)?)?;
        file.as_file().sync_all()?;
        match file.persist_noclobber(&path) {
            Ok(_) => { std::fs::File::open(parent)?.sync_all()?; Ok(true) }
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(error.error),
        }
    };
    write().unwrap_or_else(|error| {
        tracing::warn!(%error, task = %ready.evidence.task_id, "READY receipt failed; deferring signal");
        false
    })
}

/// Local ref reads are cheap; network evidence belongs to the external lane.
/// Keep both refs: a push can change origin without moving the local tip.
fn fingerprint(event: &DirectorEvent, data: &DirectorData, repo: &Path) -> Option<(String, String, String)> {
    let DirectorEvent::WorkerIdle { worker, active_task: Some(active) } = event else { return None; };
    let task = crate::store::open_task_store_local(&repo.join(".cas")).ok()?.get(&active.task_id).ok()?;
    let resolve = |branch: &str| crate::git_evidence::resolve_ref_commit_sha(repo, branch);
    let (branch, tip) = crate::ui::factory::director::task_delivery_ref(
        &task.id, worker, task.deliverables.parked_branch.as_deref(), None,
        |branch| resolve(branch).or_else(|| resolve(&format!("origin/{branch}"))), |_, _| false,
    );
    let lease = data.agents.iter().find(|agent| agent.name == *worker)
        .and_then(|agent| agent.active_lease.as_ref()).map(|lease| lease.task_id.as_str());
    // cas-43d2: batch staging and a declined tip settle reads without moving
    // a ref, so they belong to the fingerprint that decides when to re-read.
    let staged = task.deliverables.integration_batch.is_some();
    let declined = tip.as_ref().is_some_and(|tip| task.deliverables.declined_factory_anchors().contains(tip));
    let refs = format!("{:?}", (task.status, &branch, resolve(&branch), resolve(&format!("origin/{branch}")), lease, staged, declined));
    Some((task.id, tip.unwrap_or_default(), refs))
}

/// One observation supplies both readiness and worker waits. Settled results
/// are reused until refs change; failed CI is revisited slowly for same-SHA
/// reruns, while pending/unknown retains the existing retry cadence.
pub(super) fn read_delivery(
    cache: &ProbeCache, event: &DirectorEvent, data: &DirectorData, repo: &Path, now: DateTime<Utc>,
) -> (Option<Ready>, Option<(String, WorkerWait)>) {
    let cas_dir = repo.join(".cas");
    let Some((task, tip, refs)) = fingerprint(event, data, repo) else { return (None, None); };
    if let Some(ready) = prior_ready(&cas_dir, &task, &tip) {
        return (Some(ready), None);
    }
    let previous = cache.reads.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&task).cloned();
    if let Some(previous) = previous
        && previous.fingerprint == refs
        && !matches!(previous.ci, "pending" | "unknown")
        && (previous.ready.as_ref().is_some_and(|ready| ready.ci == "success")
            || matches!(previous.ci, "merged" | "unpushed" | "declined")
            || now.signed_duration_since(previous.checked_at) < SETTLED_RETRY)
    {
        let mut wait = previous.wait.clone();
        if let Some((worker, read)) = &mut wait {
            read.waiting = worker_build_pending(data, &cas_dir, worker) || read.ci == "pending";
        }
        return (previous.ready.clone(), wait);
    }
    let wait = read_worker_wait(event, data, &cas_dir, repo);
    let mut ci = wait.as_ref().map(|(_, wait)| wait.ci).unwrap_or("unknown");
    let ready = read(event, data, repo, wait.as_ref().map(|(_, wait)| wait), &mut ci);
    let refs = fingerprint(event, data, repo).map(|(_, _, refs)| refs).unwrap_or(refs);
    let mut reads = cache.reads.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    // An older in-flight observation cannot overwrite a newer request's read.
    if reads.get(&task).is_none_or(|previous| previous.checked_at <= now) {
        reads.insert(task, ProbeRead { fingerprint: refs, checked_at: now, ready: ready.clone(), wait: wait.clone(), ci });
    }
    (ready, wait)
}

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
    pub(super) probes: ProbeCache,
    entries: HashMap<String, Retry>,
    idle_signals: HashMap<String, (IdleSignal, DateTime<Utc>)>,
    #[cfg(test)]
    pub now: Option<DateTime<Utc>>,
}

impl Retries {
    /// Probe deliveries even while their workers execute or a close gate refuses
    /// to park. This only schedules external reads; it performs no Git/CI I/O.
    fn discover(&mut self, data: &DirectorData, workers: &[String], now: DateTime<Utc>) {
        for task in data.in_progress_tasks.iter().chain(&data.ready_tasks) {
            if !matches!(task.status, cas_types::TaskStatus::InProgress | cas_types::TaskStatus::Blocked) {
                continue;
            }
            let Some(assignee) = task.assignee.as_deref() else { continue; };
            let Some(agent) = data.agents.iter().find(|agent| agent.id == assignee || agent.name == assignee) else {
                continue;
            };
            if !workers.contains(&agent.name) { continue; }
            self.entries.entry(task.id.clone()).or_insert_with(|| Retry {
                event: DirectorEvent::WorkerIdle {
                    worker: agent.name.clone(),
                    active_task: Some(cas_factory::ActiveLeaseSummary {
                        task_id: task.id.clone(), task_title: task.title.clone(),
                        task_status: task.status, close_rejected_reason: None, pending_qa: None,
                    }),
                },
                next_probe: now,
                unknown_since: None,
            });
        }
    }

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
            data.in_progress_tasks.iter().chain(&data.ready_tasks).any(|task| {
                task.id == *task_id
                    && matches!(task.status, cas_types::TaskStatus::InProgress | cas_types::TaskStatus::AwaitingMerge | cas_types::TaskStatus::Blocked)
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
        // Only this supervisor's session workers may seed a delivery probe.
        self.merge_alert_retries.discover(&self.director_data, &self.worker_names, now);
        self.merge_alert_retries
            .due(now, &self.unfiltered_director_data)
    }
}

#[derive(Debug, Clone)]
pub(super) struct Ready {
    pub evidence: MergeAlertEvidence,
    pub tip: String,
    pub ci: &'static str,
    /// Present for an active delivery whose readiness does not depend on idle/park.
    pub worker: Option<String>,
}

pub(super) fn ready_prompt(
    ready: &Ready, data: &DirectorData, supervisor: &str, cli: cas_mux::SupervisorCli,
) -> Option<crate::ui::factory::director::Prompt> {
    let worker = ready.worker.as_deref()?;
    let (_, target, _) = crate::ui::factory::director::resolve_merge_target_for_task(data, &ready.evidence.task_id);
    let target = target?;
    let prefix = cli.backend().capabilities().tool_prefix;
    let action = if crate::ui::factory::director::target_is_protected_default(&target) {
        format!("Open a pull request from {} to {target} and queue it through the protected merge path.", ready.evidence.factory_branch)
    } else {
        format!("Merge now: `{prefix}factory action=worktree_merge id={worker} task_id={}` into {target}.", ready.evidence.task_id)
    };
    Some(crate::ui::factory::director::Prompt {
        merge_alert: Some(ready.evidence.clone()), target: supervisor.to_string(),
        text: format!(
            "READY TO MERGE — pushed delivery {} has green lane CI.\nTask {} on {}.\nMerge target: {target}.\nLive evidence: {} unmerged commit(s) on {} vs {target} (checked {} at {}).\n{action}\nDo not wait for park or a merge request; the merge needs no worker action. Existing CI and QA merge gates still apply.",
            ready.tip, ready.evidence.task_id, ready.evidence.factory_branch,
            ready.evidence.unmerged_count, ready.evidence.factory_branch,
            ready.evidence.checked_epic_ref, ready.evidence.epic_sha,
        ),
        retract_task: Some(ready.evidence.task_id.clone()), retract_worker: None,
        retract_epic: None, drop_if_worker_assigned: None, stall_nudge_task: None, durable_retry: false,
    })
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

#[derive(Debug, Clone)]
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

fn worker_build_pending(data: &DirectorData, cas_dir: &Path, worker: &str) -> bool {
    let Some(agent) = data.agents.iter().find(|agent| agent.name == worker) else { return false; };
    let Some(stored) = crate::store::open_agent_store(cas_dir).ok().and_then(|store| store.get(&agent.id).ok()) else {
        return false;
    };
    stored.machine_id == Some(cas_types::Agent::get_or_generate_machine_id())
        && stored.metadata.get("clone_path").is_some_and(|clone| {
            crate::factory_worker_check::lane_is_busy(cas_dir, Path::new(clone)).unwrap_or(false)
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
    let build_pending = worker_build_pending(data, cas_dir, worker);
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
        != crate::git_evidence::BoundedFetch::Succeeded {
        return Some((worker.clone(), read));
    }
    if crate::git_evidence::resolve_ref_commit_sha(repo, &format!("origin/{branch}")).as_deref() != Some(tip) {
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
fn read(event: &DirectorEvent, data: &DirectorData, repo: &Path, known: Option<&WorkerWait>, observed_ci: &mut &'static str) -> Option<Ready> {
    *observed_ci = "unknown";
    if let DirectorEvent::WorkerIdle { worker, active_task: Some(active) } = event {
        let summary = data.in_progress_tasks.iter().chain(&data.ready_tasks)
            .find(|task| task.id == active.task_id)?;
        if matches!(summary.status, cas_types::TaskStatus::InProgress | cas_types::TaskStatus::Blocked) {
            return read_active(worker, &active.task_id, data, repo, known, observed_ci);
        }
    }
    let evidence = match check_merge_alert_freshness(event, data, repo) {
        MergeAlertFreshness::Fresh(evidence) => evidence,
        MergeAlertFreshness::Stale => { *observed_ci = "merged"; return None; }
        _ => return None,
    };
    let tip = evidence.factory_tip.clone();
    let ci = crate::mcp::tools::core::workflow::delivery_tip_ci_conclusion(&tip, repo);
    // cas-e48d: the target can gain this delivery while GitHub answers the
    // CI probe. Refresh containment too, so a completed-CI relay cannot quote
    // the pre-CI target. This remains one bounded exact-target check on the
    // external lane; an unavailable target defers the demand.
    let evidence = match check_merge_alert_freshness(event, data, repo) {
        MergeAlertFreshness::Fresh(evidence) => evidence,
        MergeAlertFreshness::Stale => { *observed_ci = "merged"; return None; }
        _ => { *observed_ci = "unknown"; return None; }
    };
    // Never attach another tip's CI if the worker pushed during either probe.
    let current = crate::git_evidence::resolve_ref_commit_sha(repo, &evidence.factory_branch)
        .or_else(|| {
            crate::git_evidence::resolve_ref_commit_sha(
                repo,
                &format!("origin/{}", evidence.factory_branch),
            )
        })?;
    if tip != evidence.factory_tip || tip != current { return None; }
    *observed_ci = ci;
    Some(Ready { evidence, tip, ci, worker: None })
}

/// The same worker-owned pushed delivery as worktree_merge, with the stricter
/// signal requirement that origin and the observed lane tip still agree.
fn read_active(worker: &str, task_id: &str, data: &DirectorData, repo: &Path, known: Option<&WorkerWait>, observed_ci: &mut &'static str) -> Option<Ready> {
    use crate::git_evidence::{BoundedFetch, fetch_parent_branch_result, resolve_ref_commit_sha};
    use crate::ui::factory::director::{fresh_merge_alert_tip_evidence, resolve_merge_target_for_task};
    // A successful CI lookup alone cannot settle an unavailable delivery.
    *observed_ci = "unknown";
    let task = crate::store::open_task_store_local(&repo.join(".cas")).ok()?.get(task_id).ok()?;
    if !matches!(task.status, cas_types::TaskStatus::InProgress | cas_types::TaskStatus::Blocked) {
        return None;
    }
    let (_, target, _) = resolve_merge_target_for_task(data, task_id);
    let target = target?;
    let resolve = |branch: &str| resolve_ref_commit_sha(repo, branch)
        .or_else(|| resolve_ref_commit_sha(repo, &format!("origin/{branch}")));
    let (branch, tip) = crate::ui::factory::director::task_delivery_ref(
        task_id, worker, task.deliverables.parked_branch.as_deref(), None, resolve, |_, _| false,
    );
    let tip = tip?;
    let known = known.filter(|known| known.task == task_id && known.tip.as_deref() == Some(&tip));
    // cas-43d2: a rejected tip stays rejected until the worker commits, which
    // moves the fingerprint; settle it instead of re-reading every minute.
    if task.deliverables.declined_factory_anchors().contains(&tip) {
        *observed_ci = "declined";
        return None;
    }
    if known.is_none() && fetch_parent_branch_result(repo, &branch) != BoundedFetch::Succeeded { return None; }
    if resolve_ref_commit_sha(repo, &format!("origin/{branch}")).as_deref() != Some(&tip) {
        *observed_ci = "unpushed";
        return None;
    }
    // cas-16e1: a refusal can also rest on state the fingerprint does not
    // watch (assignee, task type), so "not-ready" settles only for the
    // ordinary slow-retry interval, like a failed CI read.
    if crate::mcp::tools::core::task::lifecycle::merge_close::pushed_delivery_tip(
        repo, &task, &branch, &tip,
    ).is_err() {
        *observed_ci = "not-ready";
        return None;
    }
    let ci = known.map(|known| known.ci)
        .unwrap_or_else(|| crate::mcp::tools::core::workflow::delivery_tip_ci_conclusion(&tip, repo));
    // Re-fetch the target after CI, just as the parked delivery path does.
    let evidence = match fresh_merge_alert_tip_evidence(
        repo, task_id, &branch, &target, Some(&tip),
    ) {
        MergeAlertFreshness::Fresh(evidence) => evidence,
        MergeAlertFreshness::Stale => { *observed_ci = "merged"; return None; }
        _ => return None,
    };
    if resolve(&branch).as_deref() != Some(&tip) { return None; }
    *observed_ci = ci;
    if ci != "success" { return None; }
    Some(Ready { evidence, tip, ci, worker: Some(worker.to_string()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cas-43d2: a staged delivery is cached as "merged" until its read
    /// fingerprint changes, so unstaging it must change the fingerprint.
    #[test]
    fn batch_staging_changes_the_delivery_read_fingerprint_cas_43d2() {
        let f = super::super::merge_alert_tests::Fixture::new();
        let cas_dir = f.app.cas_dir.clone();
        let repo = cas_dir.parent().unwrap();
        let data = DirectorData::load_with_stores_for_project(&cas_dir, None, false, None, None).unwrap();
        let unstaged = fingerprint(&f.event, &data, repo);
        let store = crate::store::open_task_store_local(&cas_dir).unwrap();
        let mut task = store.get("cas-9efb-test").unwrap();
        task.deliverables.integration_batch = Some(cas_types::IntegrationBatchEvidence {
            branch: "batch/x".into(), tip: "a".repeat(40), base: "a".repeat(40),
            delivered_head: "a".repeat(40), supervisor_id: "sup".into(), recorded_at: Utc::now(),
        });
        store.update(&task).unwrap();
        assert!(unstaged.is_some());
        assert_ne!(fingerprint(&f.event, &data, repo), unstaged);
    }
}
