//! Capacity waiting is a worker admission episode, never a task lifecycle state.
//! Readers fence episodes with the current task lease. The existing notice
//! outbox makes deadline reconciliation safe across retries and daemon restarts.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::store::{open_agent_store, open_task_store};
use cas_types::{AgentRole, TaskStatus};

const RETRY_SECONDS: i64 = 120;
const ESCALATE_SECONDS: i64 = 20 * 60;
const LOWEST_PRIORITY: i32 = 4;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CapacityKind {
    #[default]
    Unclassified,
    Other,
    PriorityQueue,
    Disk,
}

#[derive(Default)]
struct ReclaimableCaches {
    caches: Vec<(PathBuf, u64)>,
    complete: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct CapacityWait {
    episode: String,
    pub worker_id: String,
    worker_name: String,
    pub task_id: String,
    lease_epoch: u64,
    checkout: PathBuf,
    session: Option<String>,
    pub since: DateTime<Utc>,
    pub next_retry: DateTime<Utc>,
    pub(crate) cause: String,
    notified: bool,
    #[serde(default)]
    kind: CapacityKind,
}

#[derive(Clone, Serialize, Deserialize)]
struct PressureNotice {
    episode: String,
    revision: u64,
    workers: Vec<(String, String, u64)>,
    reclaimable_bytes: Option<u64>,
    notified: bool,
}

fn read_pressure(root: &Path) -> Result<BTreeMap<String, PressureNotice>> {
    match std::fs::read(directory(root).join("pressure.state")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(error.into()),
    }
}

fn write_pressure(root: &Path, notices: &BTreeMap<String, PressureNotice>) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(directory(root))?;
    serde_json::to_writer(file.as_file_mut(), notices)?;
    file.persist(directory(root).join("pressure.state"))?;
    Ok(())
}

/// Reuse the bounded, read-only GC inventory; incomplete scans never claim an
/// exact reclaimable total or turn changing partial scans into new notices.
fn reclaimable_caches(root: &Path) -> Result<ReclaimableCaches> {
    use crate::factory_target_cache::{CacheDisposition, TargetCachePolicy};
    let config = crate::config::Config::load(root)?;
    let agents = open_agent_store(root)?.list(None)?;
    let mut known = Vec::new();
    let mut live = Vec::new();
    for agent in agents {
        let path = agent.metadata.get("clone_path").map(PathBuf::from)
            .unwrap_or_else(|| root.join("worktrees").join(&agent.name));
        known.push(path.clone());
        if agent.is_alive() { live.push(path); }
    }
    known.extend(crate::store::open_worktree_store(root)?.list()?.into_iter().map(|worktree| worktree.path));
    if let Some(repo) = root.parent() {
        if let Ok(entries) = std::fs::read_dir(config.worktrees().resolve_base_path(repo)) {
            known.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
    let report = crate::factory_target_cache::inspect_until(root, TargetCachePolicy::from(config.factory()),
        &known, &live, std::time::Instant::now() + std::time::Duration::from_secs(2))?;
    Ok(ReclaimableCaches {
        caches: report.caches.into_iter().filter(|cache| matches!(cache.disposition,
            CacheDisposition::Eligible | CacheDisposition::Selected)).map(|cache| (cache.path, cache.bytes)).collect(),
        complete: report.inventory_complete,
    })
}

fn effective_priority(priority: i32, since: DateTime<Utc>, now: DateTime<Utc>) -> i32 {
    let waited_seconds = (now - since).num_seconds().max(0);
    // Aging one level per escalation interval guarantees that a long-waiting
    // task eventually ties P0 and FIFO order then prevents starvation.
    (priority.clamp(0, LOWEST_PRIORITY) - (waited_seconds / ESCALATE_SECONDS) as i32).max(0)
}

fn same_identity(left: &CapacityWait, right: &CapacityWait) -> bool {
    left.worker_id == right.worker_id
        && left.task_id == right.task_id
        && left.lease_epoch == right.lease_epoch
        && left.checkout == right.checkout
        && left.session == right.session
}

fn compare_position(
    left_priority: i32,
    left_since: &DateTime<Utc>,
    left_name: &str,
    right_priority: i32,
    right_since: &DateTime<Utc>,
    right_name: &str,
) -> std::cmp::Ordering {
    left_priority
        .cmp(&right_priority)
        .then_with(|| left_since.cmp(&right_since))
        .then_with(|| left_name.cmp(right_name))
}

fn directory(root: &Path) -> PathBuf {
    root.join("capacity-waits")
}
fn path(root: &Path, worker: &str) -> PathBuf {
    directory(root).join(format!(
        "{}.json",
        hex::encode(Sha256::digest(worker.as_bytes()))
    ))
}
fn lock(root: &Path) -> Result<File> {
    std::fs::create_dir_all(directory(root))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory(root).join("state.lock"))?;
    file.lock_exclusive()?;
    Ok(file)
}
fn read(file: &Path) -> Result<Option<CapacityWait>> {
    match std::fs::read(file) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn write(root: &Path, wait: &CapacityWait) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(directory(root))?;
    serde_json::to_writer(file.as_file_mut(), wait)?;
    file.persist(path(root, &wait.worker_id))?;
    Ok(())
}
fn entries(root: &Path) -> Result<Vec<(PathBuf, CapacityWait)>> {
    let entries = match std::fs::read_dir(directory(root)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut waits = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.path().extension().is_some_and(|ext| ext == "json")
        {
            if let Some(wait) = read(&entry.path())? {
                waits.push((entry.path(), wait));
            }
        }
    }
    waits.sort_by(|(_, a), (_, b)| a.worker_name.cmp(&b.worker_name));
    Ok(waits)
}

fn current(root: &Path, wait: &CapacityWait, now: DateTime<Utc>) -> Result<bool> {
    let agents = open_agent_store(root)?;
    let Ok(worker) = agents.get(&wait.worker_id) else {
        return Ok(false);
    };
    if worker.role != AgentRole::Worker
        || !worker.is_alive()
        || worker.factory_session != wait.session
    {
        return Ok(false);
    }
    let tasks = open_task_store(root)?;
    let Ok(task) = tasks.get(&wait.task_id) else {
        return Ok(false);
    };
    if task.status != TaskStatus::InProgress
        || !crate::mcp::tools::core::task::task_assignee_matches_agent(
            agents.as_ref(),
            task.assignee.as_deref(),
            &worker,
        )
    {
        return Ok(false);
    }
    Ok(agents.get_lease(&wait.task_id)?.is_some_and(|lease| {
        lease.agent_id == wait.worker_id
            && lease.epoch == wait.lease_epoch
            && lease.expires_at > now
    }))
}

/// A live task lease can outlast the worker's last admission retry. Keep its
/// wait record for aging and escalation, but stop reserving queue position
/// until it retries.
fn admission_current(root: &Path, wait: &CapacityWait, now: DateTime<Utc>) -> Result<bool> {
    if now > wait.next_retry + Duration::seconds(RETRY_SECONDS) {
        return Ok(false);
    }
    current(root, wait, now)
}

/// Resolve only a registered worker actually holding an InProgress task in
/// this checkout. Standalone checks have no factory episode to record.
fn binding(root: &Path, repo: &Path, now: DateTime<Utc>) -> Result<Option<CapacityWait>> {
    let repo = repo.canonicalize()?;
    let agents = open_agent_store(root)?;
    let tasks = open_task_store(root)?;
    let mut candidates = Vec::new();
    for worker in agents
        .list(None)?
        .into_iter()
        .filter(|agent| agent.role == AgentRole::Worker && agent.is_alive())
    {
        let checkout = worker
            .metadata
            .get("clone_path")
            .map(PathBuf::from)
            .filter(|path| path.exists())
            .unwrap_or_else(|| root.join("worktrees").join(&worker.name));
        if checkout.canonicalize().ok().as_ref() != Some(&repo) {
            continue;
        }
        for lease in agents.list_agent_leases(&worker.id)? {
            let task = match tasks.get(&lease.task_id) {
                Ok(task) => task,
                Err(cas_store::StoreError::TaskNotFound(_)) => continue,
                Err(error) => return Err(error.into()),
            };
            if task.status != TaskStatus::InProgress
                || lease.expires_at <= now
                || !crate::mcp::tools::core::task::task_assignee_matches_agent(
                    agents.as_ref(),
                    task.assignee.as_deref(),
                    &worker,
                )
            {
                continue;
            }
            candidates.push(CapacityWait {
                episode: uuid::Uuid::new_v4().to_string(),
                worker_id: worker.id.clone(),
                worker_name: worker.name.clone(),
                task_id: task.id,
                lease_epoch: lease.epoch,
                checkout: repo.clone(),
                session: worker.factory_session.clone(),
                since: now,
                next_retry: now + Duration::seconds(RETRY_SECONDS),
                cause: String::new(),
                notified: false,
                kind: CapacityKind::Other,
            });
        }
    }
    match candidates.len() {
        0 => Ok(None),
        1 => Ok(candidates.pop()),
        _ => bail!(
            "capacity wait has ambiguous worker/task ownership for {}",
            repo.display()
        ),
    }
}

/// Called only for typed admission-capacity refusals, not dirty trees,
/// compiler/test failures or I/O errors. First refusal survives each retry.
#[cfg(test)]
pub(crate) fn record_refusal(
    root: &Path,
    repo: &Path,
    cause: &str,
    now: DateTime<Utc>,
) -> Result<Option<CapacityWait>> {
    record_refusal_with_kind(root, repo, cause, CapacityKind::Other, now)
}

pub(crate) fn record_refusal_with_kind(
    root: &Path,
    repo: &Path,
    cause: &str,
    kind: CapacityKind,
    now: DateTime<Utc>,
) -> Result<Option<CapacityWait>> {
    let Some(mut wait) = binding(root, repo, now)? else {
        return Ok(None);
    };
    let _lock = lock(root)?;
    if let Some(previous) = read(&path(root, &wait.worker_id))?
        && same_identity(&previous, &wait)
        && current(root, &previous, now)?
    {
        wait = previous;
    }
    let priority = open_task_store(root)?.get(&wait.task_id)?.priority.0;
    let priority = effective_priority(priority, wait.since, now);
    wait.cause = format!("priority=P{priority} {cause}");
    wait.kind = kind;
    wait.next_retry = now + Duration::seconds(RETRY_SECONDS);
    write(root, &wait)?;
    Ok(Some(wait))
}

/// Refuse a lower-ranked retry while a live, higher-ranked waiter is ahead in
/// the queue. Call while the worker-check admission lock is held.
pub(crate) fn priority_refusal(
    root: &Path,
    repo: &Path,
    now: DateTime<Utc>,
    available_slots: usize,
) -> Result<Option<String>> {
    let Some(mut requester) = binding(root, repo, now)? else {
        return Ok(None);
    };
    let _lock = lock(root)?;
    if let Some(previous) = read(&path(root, &requester.worker_id))?
        && same_identity(&previous, &requester)
        && current(root, &previous, now)?
    {
        requester = previous;
    }
    let tasks = open_task_store(root)?;
    let request_priority = effective_priority(
        tasks.get(&requester.task_id)?.priority.0,
        requester.since,
        now,
    );
    let mut first_ahead: Option<(i32, DateTime<Utc>, String)> = None;
    let mut ahead_count = 0;
    for (_, wait) in entries(root)? {
        if wait.worker_id == requester.worker_id || !admission_current(root, &wait, now)? {
            continue;
        }
        let task = match tasks.get(&wait.task_id) {
            Ok(task) => task,
            Err(_) => continue,
        };
        let priority = effective_priority(task.priority.0, wait.since, now);
        if compare_position(
            priority,
            &wait.since,
            &wait.worker_name,
            request_priority,
            &requester.since,
            &requester.worker_name,
        ) != std::cmp::Ordering::Less
        {
            continue;
        }
        ahead_count += 1;
        let position = (priority, wait.since.clone(), wait.worker_name);
        if first_ahead.as_ref().is_none_or(|first| {
            compare_position(
                position.0,
                &position.1,
                &position.2,
                first.0,
                &first.1,
                &first.2,
            ) == std::cmp::Ordering::Less
        }) {
            first_ahead = Some(position);
        }
    }
    if available_slots == 0 || ahead_count < available_slots {
        return Ok(None);
    }
    Ok(first_ahead
        .map(|(priority, _, worker)| format!("queued behind priority=P{priority} worker={worker}")))
}

/// Admission ends this checkout's wait before Cargo starts. Stale identities
/// are harmless too: a later task must not inherit an earlier deadline.
pub(crate) fn admitted(root: &Path, repo: &Path) -> Result<()> {
    if !directory(root).exists() {
        return Ok(());
    }
    let repo = repo.canonicalize()?;
    let _lock = lock(root)?;
    for (file, wait) in entries(root)? {
        if wait.checkout == repo {
            std::fs::remove_file(file)?;
        }
    }
    // End a fully resolved pressure episode here, not at the next daemon
    // scan: a worker may be refused again before that scan gets a turn.
    let mut pressure = read_pressure(root)?;
    if !pressure.is_empty() {
        let mut remaining = BTreeSet::new();
        for (_, wait) in entries(root)? {
            if wait.kind == CapacityKind::Disk && !wait.notified {
                remaining.insert(serde_json::to_string(&wait.session)?);
            }
        }
        let before = pressure.len();
        pressure.retain(|key, _| remaining.contains(key));
        if pressure.len() != before { write_pressure(root, &pressure)?; }
    }
    Ok(())
}

/// Read-only view, including stale-lease rejection, shared by status and tests.
#[cfg(test)]
pub(crate) fn view(root: &Path, worker: &str, now: DateTime<Utc>) -> Result<Option<CapacityWait>> {
    match read(&path(root, worker))? {
        Some(wait) if admission_current(root, &wait, now)? => Ok(Some(wait)),
        _ => Ok(None),
    }
}

pub(crate) fn render(root: &Path, session: Option<&str>, now: DateTime<Utc>) -> String {
    let result = (|| -> Result<String> {
        let mut lines = Vec::new();
        for (_, wait) in entries(root)? {
            if session.is_some() && wait.session.as_deref() != session {
                continue;
            }
            if !admission_current(root, &wait, now)? {
                continue;
            }
            lines.push(format!(
                "Capacity wait: {} ({})\n  cause: {}\n  next retry: {}\n  waiting since: {}{}",
                wait.worker_name,
                wait.task_id,
                wait.cause,
                wait.next_retry.to_rfc3339(),
                wait.since.to_rfc3339(),
                if wait.notified {
                    "; supervisor notice queued"
                } else {
                    ""
                }
            ));
        }
        Ok(lines.join("\n"))
    })();
    result.unwrap_or_else(|error| format!("Capacity wait state unavailable: {error}"))
}

/// Explicit blocker messages already queue an immediate supervisor wake.
/// Consume automatic escalation only after that durable acceptance succeeds.
pub(crate) fn assistance_queued(
    root: &Path,
    worker: &str,
    task: Option<&str>,
    target: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    if !path(root, worker).exists() {
        return Ok(());
    }
    let _lock = lock(root)?;
    if let Some(mut wait) = read(&path(root, worker))?
        && current(root, &wait, now)?
        && task.is_none_or(|task| task == wait.task_id)
    {
        // Match the logical supervisor routing policy, including its session
        // metadata fallback. A successful message to another live supervisor
        // must not consume this owner's automatic capacity notice.
        let agents = open_agent_store(root)?;
        let owns_notice = target.eq_ignore_ascii_case("supervisor")
            || wait.session.as_deref().filter(|session| !session.trim().is_empty())
                .and_then(|session| {
                    crate::mcp::tools::core::task::lifecycle::supervisor_push::resolve_worker_supervisor(
                        agents.as_ref(), Some(session))
                })
                .is_some_and(|owner| owner.name.eq_ignore_ascii_case(target));
        if owns_notice {
            wait.notified = true;
            write(root, &wait)?;
        }
    }
    Ok(())
}

/// Runs on the daemon's existing background reminder-scan lane, even with no
/// retry or database change. Existing idempotent outbox owns crash recovery.
pub(crate) fn reconcile(root: &Path, session: Option<&str>, now: DateTime<Utc>) -> Result<usize> {
    reconcile_with_inventory(root, session, now, || reclaimable_caches(root))
}

fn reconcile_with_inventory(
    root: &Path,
    session: Option<&str>,
    now: DateTime<Utc>,
    inventory: impl FnOnce() -> Result<ReclaimableCaches>,
) -> Result<usize> {
    if !directory(root).exists() {
        return Ok(0);
    }
    let _lock = lock(root)?;
    let mut notified = 0;
    let mut disk_groups: BTreeMap<String, Vec<CapacityWait>> = BTreeMap::new();
    for (file, mut wait) in entries(root)? {
        if session.is_some() && wait.session.as_deref() != session {
            continue;
        }
        if !current(root, &wait, now)? {
            std::fs::remove_file(file)?;
            continue;
        }
        if matches!(wait.kind, CapacityKind::PriorityQueue | CapacityKind::Unclassified) || wait.notified {
            continue;
        }
        if wait.kind == CapacityKind::Disk {
            disk_groups.entry(serde_json::to_string(&wait.session)?).or_default().push(wait);
            continue;
        }
        if now - wait.since < Duration::seconds(ESCALATE_SECONDS) {
            continue;
        }
        let agents = open_agent_store(root)?.list(None)?;
        let recipient = crate::supervisor_notices::resolve(&agents, None, wait.session.as_deref());
        let detail = format!(
            "Worker {} has waited for build capacity on {} for at least 20 minutes.\nCause: {}\nNext retry: {}. Check capacity or help the worker resume; its task and lease remain active.",
            wait.worker_name,
            wait.task_id,
            wait.cause,
            wait.next_retry.to_rfc3339()
        );
        crate::supervisor_notices::enqueue(
            root,
            &recipient,
            "capacity_wait",
            &detail,
            &wait.episode,
        )?;
        wait.notified = true;
        write(root, &wait).context("persist delivered capacity escalation")?;
        notified += 1;
    }
    let mut pressure = read_pressure(root)?;
    let selected_session = session.map(|session| serde_json::to_string(&Some(session))).transpose()?;
    let previous_count = pressure.len();
    pressure.retain(|key, _| selected_session.as_ref().is_some_and(|selected| selected != key)
        || disk_groups.contains_key(key));
    if pressure.len() != previous_count { write_pressure(root, &pressure)?; }
    if disk_groups.values().all(|waits| waits.iter().all(|wait| now - wait.since < Duration::seconds(ESCALATE_SECONDS))) {
        return Ok(notified);
    }
    let inventory = inventory()?;
    let reclaimable_bytes = inventory.complete.then(|| inventory.caches.iter()
        .fold(0u64, |total, (_, bytes)| total.saturating_add(*bytes)));
    let agents = open_agent_store(root)?.list(None)?;
    for (key, waits) in disk_groups {
        if waits.iter().all(|wait| now - wait.since < Duration::seconds(ESCALATE_SECONDS)) { continue; }
        let workers = waits.iter().map(|wait| (wait.worker_id.clone(), wait.task_id.clone(), wait.lease_epoch)).collect::<Vec<_>>();
        let previous = pressure.entry(key.clone()).or_insert_with(|| PressureNotice {
            episode: uuid::Uuid::new_v4().to_string(), revision: 0, workers: workers.clone(), reclaimable_bytes, notified: false,
        });
        if previous.workers != workers || previous.reclaimable_bytes != reclaimable_bytes {
            previous.workers = workers;
            previous.reclaimable_bytes = reclaimable_bytes;
            previous.revision += 1;
            previous.notified = false;
        }
        if previous.notified { continue; }
        let occurrence = format!("{}:{}", previous.episode, previous.revision);
        // Persist the outbox identity first, so a crash after enqueue can only
        // retry the same idempotent notice, including snapshot reversions.
        write_pressure(root, &pressure)?;
        let workers = waits.iter().map(|wait| format!("- {} ({}): {}", wait.worker_name, wait.task_id, wait.cause)).collect::<Vec<_>>().join("\n");
        let caches = inventory.caches.iter().map(|(path, bytes)| format!("- {}: {bytes} bytes", path.display())).collect::<Vec<_>>().join("\n");
        let total = reclaimable_bytes.map_or_else(|| "unknown (inventory incomplete)".to_string(), |bytes| format!("{bytes} bytes"));
        let detail = format!("Disk pressure is delaying builds. Waiting workers:\n{workers}\nReclaimable target caches (logical bytes): {total}\n{}\nThis notice updates only when waiting workers or reclaimable bytes change; tasks and leases remain active.", if caches.is_empty() { "- none found" } else { &caches });
        let recipient = crate::supervisor_notices::resolve(&agents, None, waits[0].session.as_deref());
        crate::supervisor_notices::enqueue(root, &recipient, "capacity_wait", &detail, &occurrence)?;
        pressure.get_mut(&key).expect("pressure episode").notified = true;
        write_pressure(root, &pressure)?;
        notified += 1;
    }
    Ok(notified)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{open_supervisor_queue_store, open_prompt_queue_store};
    use cas_types::{Agent, Task};

    #[test]
    fn capacity_notices_aggregate_disk_and_silence_priority_queue_cas_ed71() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        let agents = open_agent_store(&root).unwrap();
        agents.init().unwrap();
        let tasks = open_task_store(&root).unwrap();
        tasks.init().unwrap();
        let mut supervisor = Agent::new_with_role("capacity-lead".into(), "capacity-lead".into(), AgentRole::Supervisor);
        supervisor.factory_session = Some("capacity-session".into());
        agents.register(&supervisor).unwrap();
        let now = Utc::now();
        let mut repos = Vec::new();
        for name in ["capacity-first", "capacity-second"] {
            let mut worker = Agent::new_with_role(name.into(), name.into(), AgentRole::Worker);
            worker.factory_session = supervisor.factory_session.clone();
            agents.register(&worker).unwrap();
            let mut task = Task::new(format!("cas-{name}"), "capacity fixture".into());
            task.status = TaskStatus::InProgress;
            task.assignee = Some(name.into());
            tasks.add(&task).unwrap();
            agents.try_claim(&task.id, name, 7200, Some("working")).unwrap();
            let repo = root.join("worktrees").join(name);
            std::fs::create_dir_all(&repo).unwrap();
            record_refusal_with_kind(&root, &repo, "priority_fixture", CapacityKind::PriorityQueue, now).unwrap();
            repos.push(repo);
        }
        let later = now + Duration::minutes(25);
        let notices = open_supervisor_queue_store(&root).unwrap();
        assert_eq!(reconcile_with_inventory(&root, Some("capacity-session"), later,
            || panic!("a priority queue has no disk inventory to inspect")).unwrap(), 0);
        assert_eq!(notices.pending_count("capacity-lead").unwrap(), 0);
        // Old wait records have no typed cause; wait for a retry to classify
        // them rather than turning unknown legacy queues into fresh pages.
        let first = path(&root, "capacity-first");
        let mut legacy: serde_json::Value = serde_json::from_slice(&std::fs::read(&first).unwrap()).unwrap();
        legacy.as_object_mut().unwrap().remove("kind");
        std::fs::write(first, serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(reconcile(&root, Some("capacity-session"), later).unwrap(), 0);
        for repo in &repos {
            record_refusal_with_kind(&root, repo, "disk_fixture", CapacityKind::Disk, later).unwrap();
        }
        let inventory = |bytes| Ok(ReclaimableCaches { caches: vec![(PathBuf::from("/fixture/retired-cache"), bytes)], complete: true });
        assert_eq!(reconcile_with_inventory(&root, Some("capacity-session"), later, || inventory(10)).unwrap(), 1);
        assert_eq!(notices.pending_count("capacity-lead").unwrap(), 1);
        let prompts = open_prompt_queue_store(&root).unwrap().peek_all(100).unwrap();
        let prompt = &prompts.iter().find(|prompt| prompt.source.starts_with("supervisor-notice:")).unwrap().prompt;
        for name in ["capacity-first", "capacity-second", "/fixture/retired-cache"] {
            assert!(prompt.contains(name)); // Fixture identities, not human-readable wording.
        }
        // One worker can finish a build and rejoin while another still
        // waits: the waiting set and pressure episode have not changed.
        admitted(&root, &repos[1]).unwrap();
        record_refusal_with_kind(&root, &repos[1], "disk_fixture", CapacityKind::Disk, later).unwrap();
        assert_eq!(reconcile_with_inventory(&root, Some("capacity-session"), later, || inventory(10)).unwrap(), 0);
        // Same snapshot after another deadline (or daemon restart) is quiet.
        assert_eq!(reconcile_with_inventory(&root, Some("capacity-session"), later + Duration::minutes(21), || inventory(10)).unwrap(), 0);
        let later = later + Duration::minutes(21);
        assert_eq!(reconcile_with_inventory(&root, Some("capacity-session"), later, || inventory(20)).unwrap(), 1);
        admitted(&root, &repos[1]).unwrap();
        assert_eq!(reconcile_with_inventory(&root, Some("capacity-session"), later, || inventory(20)).unwrap(), 1);
        assert_eq!(notices.pending_count("capacity-lead").unwrap(), 3);
        // A resolved episode can later recur with an identical waiting set.
        admitted(&root, &repos[0]).unwrap();
        assert_eq!(reconcile_with_inventory(&root, Some("capacity-session"), later, || inventory(20)).unwrap(), 0);
        record_refusal_with_kind(&root, &repos[0], "disk_fixture", CapacityKind::Disk, later).unwrap();
        assert_eq!(reconcile_with_inventory(&root, Some("capacity-session"), later + Duration::minutes(25), || inventory(20)).unwrap(), 1);
        assert_eq!(notices.pending_count("capacity-lead").unwrap(), 4);
        // Walk the production read-only inventory too: this fixture has no
        // target caches, so its complete zero-byte snapshot is a real change.
        assert_eq!(reconcile(&root, Some("capacity-session"), later + Duration::minutes(25)).unwrap(), 1);
        assert_eq!(notices.pending_count("capacity-lead").unwrap(), 5);
        // Admission can finish an episode between daemon scans. An identical
        // worker set in a fresh wait must still get its new episode notice.
        let restart = later + Duration::minutes(25);
        admitted(&root, &repos[0]).unwrap();
        record_refusal_with_kind(&root, &repos[0], "disk_fixture", CapacityKind::Disk, restart).unwrap();
        assert_eq!(reconcile_with_inventory(&root, Some("capacity-session"), restart + Duration::minutes(25), || inventory(0)).unwrap(), 1);
        assert_eq!(notices.pending_count("capacity-lead").unwrap(), 6);
    }
}
