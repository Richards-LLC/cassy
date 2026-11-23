//! cas-ddae (GH #1500): the factory progress watchdog. One rule: a session
//! with locally actionable work and no task, lane or message change for 45 minutes wakes
//! its supervisor on that cadence with what is waiting on whom; still nothing after
//! another 45, it pages the operator once. Heartbeats and liveness never
//! count; measured owned child CPU/output does. Neither do the daemon's own rows (relays, fired reminders).
//! A delivery parked for a frozen release train waits on the operator's
//! lift, not on anyone here: it is reported once per freeze, never chased.
//! Dependency-blocked tasks and held workers likewise have no local next step.
//! A current gate-refused close is reported with its code and remedy instead
//! of starting an operator-page clock, unless the gate explicitly needs the operator.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Utc};

const QUIET_SECS: i64 = 45 * 60;

/// Per-session episode: the last fingerprint, when it was first seen, and
/// the last wake cadence and whether this episode paged the operator.
static EPISODES: Mutex<Option<HashMap<String, (String, DateTime<Utc>, Option<i64>, bool)>>> = Mutex::new(None);

/// Freeze notices already relayed by this daemon; the relay key also
/// dedupes them durably across restarts.
static HELD_REPORTED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Parked deliveries held by a train freeze, by train: when the freeze
/// started and since when it holds, then the held task ids.
type FreezeHeld = BTreeMap<String, (i64, String, Vec<String>)>;
type CloseHeld = BTreeMap<String, crate::mcp::tools::core::task::close_refusal::Refusal>;

/// cas-e37f: one sample's reads, gathered off the store writer; only
/// [`apply`] writes.
pub(crate) struct Observation {
    fingerprint: String,
    waiting: Vec<String>,
    supervisor: crate::supervisor_notices::Recipient,
    freeze_held: FreezeHeld,
    close_held: CloseHeld,
    train_ready: bool,
    /// A parked delivery is among the waiting work.
    parked: bool,
    /// Completion edges for parked deliveries a recorded hold explains,
    /// decided here (store reads and git) and persisted by `apply`.
    held: Vec<crate::factory_delivery_wake::ArmPlan>,
    /// Open deliveries with no completion wake: `(task, anchor, supervisor)`.
    unwatched: Vec<(String, String, cas_types::Agent)>,
}

/// The session's open work, and a fingerprint of every task, lane and
/// message change. `None` when a store is unreadable: uncertainty is
/// neither progress nor a stall. Reads only: the full task scans and git
/// calls here must never hold the store writer (cas-e37f).
pub(crate) fn observe(cas_dir: &Path, session: &str, now: DateTime<Utc>) -> Option<Observation> {
    use cas_types::{AgentRole, TaskStatus};
    let agents: Vec<_> = crate::store::open_agent_store(cas_dir).ok()?.list(None).ok()?
        .into_iter().filter(|agent| agent.factory_session.as_deref() == Some(session)).collect();
    let supervisor = agents.iter().filter(|agent| agent.role == AgentRole::Supervisor)
        .max_by_key(|agent| agent.registered_at)
        .map(crate::supervisor_notices::Recipient::agent)
        .unwrap_or_else(|| crate::supervisor_notices::Recipient::operator(session));
    let names: Vec<String> = agents.iter().map(|agent| agent.name.clone()).collect();
    let store = crate::store::open_task_store_local(cas_dir).ok()?;
    let mut tasks = Vec::new();
    // A close or merge moves a task out of these statuses, which changes the
    // fingerprint as any status change does.
    for status in [TaskStatus::Open, TaskStatus::InProgress, TaskStatus::Blocked, TaskStatus::AwaitingMerge] {
        tasks.extend(store.list(Some(status)).ok()?);
    }
    tasks.retain(|task| task.assignee.as_ref()
        .is_some_and(|who| agents.iter().any(|agent| &agent.name == who || &agent.id == who)));
    let held_workers = if crate::ui::factory::session::metadata_path(session).exists() {
        crate::ui::factory::worker_holds_from_session_metadata_named(session)?
    } else {
        HashSet::new()
    };
    let held_assignees: HashSet<_> = agents.iter().filter(|agent| agent.role == AgentRole::Worker
        && held_workers.contains(&agent.name)).flat_map(|agent| [&agent.name, &agent.id]).collect();
    let mut dependency_held = HashSet::new();
    let mut refusals = CloseHeld::new();
    for task in &tasks {
        if task.status == TaskStatus::Blocked && !store.get_blockers(&task.id).ok()?.is_empty() {
            dependency_held.insert(task.id.clone());
        }
        // An unreadable receipt is uncertainty, never proof of a stall.
        match crate::mcp::tools::core::task::close_refusal::current(store.as_ref(), task) {
            Ok(Some(refusal)) => { refusals.insert(task.id.clone(), refusal); }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(task_id = %task.id, %error, "close refusal unreadable; excluding only this task from stall judgement");
                dependency_held.insert(task.id.clone());
            }
        }
    }
    // cas-c1fb: historical local assignees do not make a peer's release
    // train actionable here. Only structured ownership and Git proof count.
    let peer_tasks: HashSet<_> = crate::cloud::epic_split::current_epic_splits(cas_dir).ok()?
        .into_iter().filter(|split| split.is_binding(now)).flat_map(|split| split.theirs).collect();
    let peer_branches: HashSet<_> = peer_tasks.iter().filter_map(|id| store.get(id).ok())
        .filter(|task| task.task_type == cas_types::TaskType::Epic)
        .filter_map(|task| task.effective_epic_branch().map(str::to_string)).collect();
    let mut all_tasks = None;
    let mut freeze_held = FreezeHeld::new();
    let mut close_held = CloseHeld::new();
    let parked: Vec<_> = tasks.iter().filter(|task| task.status == TaskStatus::AwaitingMerge
        && task.deliverables.factory_branch_anchor.as_deref()
            .is_some_and(crate::factory_delivery_wake::full_sha)).cloned().collect();
    tasks.retain(|task| {
        if task.task_type == cas_types::TaskType::Epic
            && crate::mcp::tools::core::workflow::release_freeze::recorded(task).is_some()
        { return false; }
        // Like a frozen-train park, these intentional waits have no local
        // next step. Removing them also resets any previous stall episode.
        if dependency_held.contains(&task.id)
            || task.assignee.as_ref().is_some_and(|who| held_assignees.contains(who))
        {
            return false;
        }
        // Applies to every close path and status, including finished epics.
        // Only explicit machine metadata can make a gate an operator action.
        if let Some(refusal) = refusals.get(&task.id)
            && !refusal.operator_action_required
        {
            close_held.insert(task.id.clone(), refusal.clone());
            return false;
        }
        // cas-e239: a blocked operator check is not locally actionable.
        // Inspect only the current blocker, never historical operator waits.
        if task.status == TaskStatus::Blocked {
            if is_held(&task.notes) { return false; }
            use crate::mcp::tools::core::task::block_hold;
            let reason = block_hold::current_hold_reason(store.as_ref(), &task.id, &task.notes);
            if reason.is_some_and(|reason| block_hold::reason_is_operator_owned(&reason)) {
                return false;
            }
        }
        // An old delivery anchor cannot hide a worker's new local work.
        if task.status != TaskStatus::AwaitingMerge {
            return true;
        }
        // cas-e239: a delivery the supervisor deliberately holds for a later
        // train waits on that decision, not on anyone here.
        if is_held(&task.notes) || merge_held(store.as_ref(), task, now) {
            return false;
        }
        if peer_tasks.contains(&task.id) {
            return false;
        }
        let parent = store.get_parent_epic(&task.id).ok().flatten();
        if parent.as_ref().is_some_and(|epic| peer_tasks.contains(&epic.id)) {
            return false;
        }
        let target = task.deliverables.work_target.as_ref().map(|target| target.target_branch.as_str())
            .or(task.branch.as_deref()).filter(|target| !target.trim().is_empty())
            .or_else(|| parent.as_ref().and_then(|epic| epic.effective_epic_branch()));
        let Some(target) = target else { return true };
        if peer_branches.contains(target) || cas_dir.parent().is_some_and(|repo| delivery_is_contained(repo, task, target)) {
            return false;
        }
        // cas-1f65: lifting a release freeze is an explicit decision; a repair may
        // still merge under a red, so it stays actionable here.
        use crate::mcp::tools::core::workflow::release_freeze;
        if task.labels.iter().any(|label| label == release_freeze::REPAIR_LABEL) {
            return true;
        }
        let trains = all_tasks.get_or_insert_with(|| store.list(None).unwrap_or_default());
        let target = crate::mcp::tools::core::task::repo_context::effective_epic_ref(cas_dir, target);
        let Some((train, freeze)) = release_freeze::frozen_target(cas_dir, trains, &target) else { return true };
        freeze_held.entry(train.id.clone())
            .or_insert_with(|| (freeze.at.timestamp(), release_freeze::frozen_since(&train, &freeze), Vec::new()))
            .2.push(task.id.clone());
        false
    });
    // Recorded holds already explain these deliveries. Decide their
    // completion edges here, git included; `apply` only persists them.
    let held: Vec<_> = parked.into_iter().filter(|parked| !tasks.iter().any(|task| task.id == parked.id))
        .filter_map(|held| crate::factory_delivery_wake::plan_task(cas_dir, &held, "")
            .inspect_err(|error| tracing::warn!(task_id=%held.id, %error, "held delivery completion wake unavailable"))
            .ok().flatten())
        .collect();
    // A train belongs to its verification owner, even when no task assignee
    // remains after its last child merges. Older explicitly owned trains have
    // only the release-train branch, without the newer label.
    let mut ready_trains = HashSet::new();
    for train in store.list(None).ok()? {
        if train.task_type != cas_types::TaskType::Epic
            || !matches!(train.status, TaskStatus::Open | TaskStatus::InProgress)
            || !(crate::mcp::tools::core::task::release_train::is_release_train(&train)
                || train.effective_epic_branch().is_some_and(|lane| lane.starts_with("epic/release-train-")))
            || !train.epic_verification_owner.as_ref().is_some_and(|owner| owner == &supervisor.id || owner == &supervisor.name)
            || peer_tasks.contains(&train.id)
            || is_held(&train.notes)
            || close_held.contains_key(&train.id)
            || crate::mcp::tools::core::workflow::release_freeze::recorded(&train).is_some()
        { continue; }
        let children = store.get_subtasks(&train.id).ok()?;
        if !children.iter().any(|child| child.status == TaskStatus::Closed)
            || children.iter().any(|child| matches!(child.status, TaskStatus::InProgress | TaskStatus::AwaitingMerge))
        { continue; }
        ready_trains.insert(train.id.clone());
        tasks.retain(|task| task.id != train.id);
        tasks.push(train);
    }
    let unwatched = match find_unwatched(cas_dir, &tasks, now) {
        Ok(unwatched) => unwatched,
        Err(error) => { tracing::warn!(%error, "delivery wake coverage unavailable"); return None; }
    };
    // An absent completion edge is a plumbing defect, not idle work.
    tasks.retain(|task| !unwatched.iter().any(|(id, _, _)| id == &task.id));
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    let mut fingerprint: Vec<String> = tasks.iter()
        .map(|task| format!("{} {} {}", task.id, task.status, task.updated_at.to_rfc3339())).collect();
    let waiting = tasks.iter().map(|task| {
        if ready_trains.contains(&task.id) {
            return format!("- Train {} has merged work and no children in flight: run tip CI and freeze; resolve any remaining open rows before the cut.", task.id);
        }
        let title: String = task.title.chars().take(70).collect();
        let gate = refusals.get(&task.id).map(|refusal| format!(
            "\n  operator action required; code={}; next_action={}",
            refusal.code, refusal.next_action,
        )).unwrap_or_default();
        format!("- {} {} ({}): {title}{gate}", task.id, task.status, task.assignee.as_deref().unwrap_or_default())
    }).collect();
    let latest = crate::store::open_prompt_queue_store(cas_dir).ok()?.latest_agent_message_id(&names, session).ok()?;
    fingerprint.push(format!("message {}", latest.unwrap_or(0)));
    // Long non-Rust gates can advance only their child logs. Reuse status's
    // worker/session/PID fences; unchanged counters never reset the episode.
    for agent in agents.iter().filter(|agent| agent.role == AgentRole::Worker
        && tasks.iter().any(|task| task.status == TaskStatus::InProgress
            && task.assignee.as_ref().is_some_and(|who| who == &agent.id || who == &agent.name)))
    {
        if let Some(at) = crate::mcp::tools::service::worker_liveness::child_progress(agent, now) {
            fingerprint.push(format!("child {} {}", agent.id, at.to_rfc3339()));
        }
    }
    // Worker lane tips; a repo git cannot read contributes nothing.
    // cas-e239: a release commit pushed to a train is progress too.
    let mut lanes: Vec<String> = agents.iter().filter(|agent| agent.role == AgentRole::Worker)
        .flat_map(|agent| [format!("refs/heads/factory/{}", agent.name), format!("refs/heads/factory/{}-*", agent.name)])
        .collect();
    lanes.extend(["refs/heads/epic/release-train-*", "refs/remotes/origin/epic/release-train-*"].map(String::from));
    if let Some(repo) = cas_dir.parent().filter(|repo| repo.join(".git").exists())
        && let Ok(output) = std::process::Command::new("git").arg("-C").arg(repo)
            .args(["for-each-ref", "--format=%(refname) %(objectname)"]).args(&lanes).output()
    {
        fingerprint.push(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Some(Observation {
        fingerprint: fingerprint.join("\n"), waiting, supervisor, freeze_held, close_held,
        train_ready: !ready_trains.is_empty(), held, unwatched,
        parked: tasks.iter().any(|task| task.status == TaskStatus::AwaitingMerge),
    })
}

fn report_close_held(cas_dir: &Path, held: CloseHeld) {
    for (id, refusal) in held {
        let key = format!("close-gate-held:{id}:{}", crate::mcp::tools::core::task::close_refusal::key(&refusal));
        super::lifecycle::enqueue_worker_attention_relay_detail_with_key(
            cas_dir, "progress_close_refused", "supervisor", Some(&id), None,
            |prefix| format!(
                "Close is held by a Cassy gate: {id}; code={}; next_action={}.\nResolve the recorded refusal, then retry `{prefix}task action=close id={id}`. The watchdog will not page the operator for this held close.",
                refusal.code, refusal.next_action,
            ), &key, Some(&key),
        );
    }
}

/// Tell the supervisor once per freeze which deliveries it holds and who
/// lifts it, so a frozen train is never reported as a stall.
// gate[N-01 notice]: Train-freeze held
fn report_freeze_held(cas_dir: &Path, held: FreezeHeld) {
    let mut reported = HELD_REPORTED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let reported = reported.get_or_insert_with(HashSet::new);
    for (train, (at, since, mut ids)) in held {
        let key = format!("train-freeze-held:{train}:{at}");
        if reported.contains(&key) {
            continue;
        }
        ids.sort();
        let detail = format!(
            "Parked for a frozen release train, so not a stall: {}. {since}. The operator lifts it with Lift freeze on the {train} train card in Commander, or the train's supervisor with train_freeze decision=lift and a reason; publishing the release also ends it. The progress watchdog will not chase these deliveries or page the operator for them.",
            ids.join(", ")
        );
        if matches!(
            super::lifecycle::enqueue_worker_attention_relay_detail_with_key(cas_dir, "progress_operator_held", "supervisor",
                None, None, |_| detail.clone(), &key, Some(&key)),
            super::lifecycle::WorkerAttentionRelayOutcome::Persisted { .. }
        ) {
            reported.insert(key);
        }
    }
}

/// Whether the newest decision in `notes` is a `DECISION HOLD`; any later
/// decision lifts it. Entries start at a `[YYYY-MM-DD HH:MM] ` line.
fn is_held(notes: &str) -> bool {
    let mut held = false;
    for entry in notes.split("\n[") {
        if let Some((_, rest)) = entry.split_once("DECISION") {
            held = rest.trim_start().starts_with("HOLD");
        } else if entry.contains("🚫 BLOCKER") {
            held = false;
        }
    }
    held
}

/// GH #2278: a supervisor's structured hold for an external review covers
/// this parked anchor until its deadline. A lapse or a re-park at another
/// anchor makes the delivery actionable again.
fn merge_held(store: &dyn cas_store::TaskStore, task: &cas_types::Task, now: DateTime<Utc>) -> bool {
    store.get_execution_state(&task.id).ok().flatten()
        .and_then(|state| serde_json::from_value::<cas_types::TaskMergeHold>(state.get("merge_hold")?.clone()).ok())
        .is_some_and(|hold| hold.until > now
            && task.deliverables.factory_branch_anchor.as_deref() == Some(hold.anchor.as_str()))
}

/// A parked delivery already on its recorded target waits for a receipt,
/// not another local merge. Missing refs remain eligible for escalation.
fn delivery_is_contained(repo: &Path, task: &cas_types::Task, target: &str) -> bool {
    use crate::git_evidence::{git_commit_is_ancestor, resolve_ref_commit_sha};
    let Some(anchor) = task.deliverables.factory_branch_anchor.as_deref()
        .and_then(|anchor| resolve_ref_commit_sha(repo, anchor)) else { return false };
    [target.to_string(), format!("origin/{target}")].iter()
        .filter_map(|branch| resolve_ref_commit_sha(repo, branch))
        .any(|tip| git_commit_is_ancestor(repo, &anchor, &tip))
}

/// One watchdog sample for `session` at `now`.
#[cfg(test)]
pub(crate) fn sweep(cas_dir: &Path, session: &str, now: DateTime<Utc>) {
    if let Some(observation) = observe(cas_dir, session, now) {
        apply(cas_dir, session, now, observation);
    }
}

/// The sample's writes, on the store writer: wake edges, notices and pages.
pub(crate) fn apply(cas_dir: &Path, session: &str, now: DateTime<Utc>, observation: Observation) {
    let Observation { fingerprint, waiting, supervisor, freeze_held, close_held, train_ready, parked, held, unwatched } = observation;
    // Preserve held deliveries' own notices, and leave durable completion
    // edges ready for the hold's lift.
    for plan in &held {
        if let Err(error) = crate::factory_delivery_wake::arm_plan(cas_dir, plan) {
            tracing::warn!(task_id=%plan.task, %error, "held delivery completion wake unavailable");
        }
    }
    if let Err(error) = report_unwatched(cas_dir, &unwatched) {
        tracing::warn!(%error, "delivery wake coverage unavailable");
        return;
    }
    report_freeze_held(cas_dir, freeze_held);
    report_close_held(cas_dir, close_held);
    let mut episodes = EPISODES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let episodes = episodes.get_or_insert_with(HashMap::new);
    // A project hold is the operator's own stop, not a stall.
    let open = !waiting.is_empty() && !cas_store::project_hold::is_held(cas_dir);
    let episode = episodes.entry(session.to_string()).or_insert_with(|| (fingerprint.clone(), now, None, false));
    if !open || episode.0 != fingerprint {
        *episode = (fingerprint, now, None, false);
    }
    if !open { return; }
    let quiet = (now - episode.1).num_seconds();
    let (minutes, start) = (quiet / 60, episode.1.to_rfc3339());
    let waiting = waiting.join("\n");
    // gate[W-01 watchdog]: Progress watchdog supervisor wake (progress_stalled)
    let cadence = quiet / QUIET_SECS;
    if (train_ready || quiet >= QUIET_SECS) && episode.2 != Some(cadence) {
        let detail = format!(
            "Nothing has moved in this factory for {minutes} min with work open: no task change, lane commit, merge or message from any agent here. Heartbeats do not count.\nWaiting:\n{waiting}\nFind who owes the next step and chase it, take it over, or tell the operator. If nothing moves in another {} min, the operator is paged.{}",
            QUIET_SECS / 60,
            if parked { "\nA parked delivery that waits on an external review: hold it with `task action=update id=<id> state_patch={\"merge_hold\":{\"owner\":\"<who>\",\"reason\":\"<why>\",\"until\":\"<RFC3339>\"}}`." } else { "" },
        );
        let key = format!("progress-stall:{session}:{start}:{cadence}");
        let recorded = if train_ready {
            crate::supervisor_notices::enqueue(cas_dir, &supervisor, "train_ready",
                &format!("Train ready for its supervisor:\n{waiting}\nIf nothing moves in another {} min, the watchdog wakes you again and pages the operator after {} min without progress.", QUIET_SECS / 60, 2 * QUIET_SECS / 60), &key).is_ok()
        } else { matches!(
            super::lifecycle::enqueue_worker_attention_relay_detail_with_key(cas_dir, "progress_stalled", "supervisor",
                None, Some(quiet as u64), |_| detail.clone(), &start, Some(&key)),
            super::lifecycle::WorkerAttentionRelayOutcome::Persisted { .. }
        ) };
        if recorded { episode.2 = Some(cadence); }
    }
    // gate[W-02 watchdog]: Progress watchdog operator page
    if quiet >= 2 * QUIET_SECS && !episode.3 {
        let message = format!(
            "Nothing has moved in factory session {session} for {minutes} min, though work is open and heartbeats look healthy. Supervisor {} was woken and nothing has moved since.\nWaiting:\n{waiting}\nCheck the supervisor's pane, chase the peer, or take the work over.",
            supervisor.name
        );
        // The dedupe key names the episode, so a retry cannot page twice.
        episode.3 = super::lifecycle::record_operator_page(cas_dir, "progress-watchdog",
            &format!("Factory {session}: nothing moved for {minutes} min"), message,
            &format!("progress-stall-page:{session}:{start}"));
    }
}

/// A park commits before it arms its completion wakes: the arming probes Git
/// and the CI workflow, which takes about half a second under load. A delivery
/// that changed this recently may still be inside its own park (GH #2332), so
/// the check waits it out; an unarmed delivery is reported once the grace ends.
const ARMING_GRACE: chrono::Duration = chrono::Duration::seconds(60);

fn find_unwatched(cas_dir: &Path, actionable: &[cas_types::Task], now: DateTime<Utc>) -> anyhow::Result<Vec<(String, String, cas_types::Agent)>> {
    let mut unwatched = Vec::new();
    for task in actionable.iter().filter(|task| task.status == cas_types::TaskStatus::AwaitingMerge
        && now - task.updated_at >= ARMING_GRACE) {
        let Some(sha) = task.deliverables.factory_branch_anchor.as_deref()
            .filter(|sha| crate::factory_delivery_wake::full_sha(sha)) else { continue };
        let Some(supervisor) = crate::factory_delivery_wake::owner(cas_dir, &task, "")? else { continue };
        if crate::factory_delivery_wake::watched(cas_dir, &task, &supervisor)? { continue; }
        unwatched.push((task.id.clone(), sha.to_string(), supervisor));
    }
    Ok(unwatched)
}

fn report_unwatched(cas_dir: &Path, unwatched: &[(String, String, cas_types::Agent)]) -> anyhow::Result<()> {
    for (task, sha, supervisor) in unwatched {
        let key = format!("{}:{}:{sha}", crate::factory_delivery_wake::MISSING_WAKE, task);
        crate::store::open_prompt_queue_store(cas_dir)?.enqueue_idempotent(
            &format!("lifecycle-wake:{key}"), &supervisor.name,
            &format!("{}: open delivery {} at {sha} has no durable supervisor completion wake. Repair its CI/merge event watch; this is a Cassy defect, not a worker stall.", crate::factory_delivery_wake::MISSING_WAKE, task),
            supervisor.factory_session.as_deref(), Some("Delivery completion wake missing"),
            Some(cas_store::NotificationPriority::High), &key, Some(&cas_store::QueueOrigin::Daemon),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use cas_types::{Agent, AgentRole, AgentStatus, Task, TaskStatus};

    #[test]
    fn final_child_merge_keeps_owned_train_waking_and_paging_cas_2ba4() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        for labelled in [false, true] {
            let session = format!("last-child-2ba4-{labelled}");
            env.set("CAS_FACTORY_SESSION", &session);
            let project = env.home().join(&session);
            std::fs::create_dir_all(&project).unwrap();
            let root = crate::store::init_cas_dir(&project).unwrap();
            let agents = crate::store::open_agent_store(&root).unwrap();
            for (id, role) in [("owner-2ba4", AgentRole::Supervisor), ("worker-2ba4", AgentRole::Worker)] {
                let mut agent = Agent::new_with_role(id.into(), id.into(), role);
                agent.factory_session = Some(session.clone());
                agents.register(&agent).unwrap();
            }
            let tasks = crate::store::open_task_store_local(&root).unwrap();
            let mut train = Task::new("cas-train-2ba4".into(), "Overnight train".into());
            train.task_type = cas_types::TaskType::Epic;
            train.epic_verification_owner = Some("owner-2ba4".into());
            train.branch = Some("epic/release-train-overnight-2ba4".into());
            if labelled {
                train.labels.push("release-train".into());
                train.assignee = Some("owner-2ba4".into());
            }
            tasks.add(&train).unwrap();
            let mut child = Task::new("cas-child-2ba4".into(), "Last delivery".into());
            child.assignee = Some("worker-2ba4".into());
            child.status = TaskStatus::AwaitingMerge;
            tasks.add(&child).unwrap();
            tasks.add_dependency(&cas_types::Dependency::new(child.id.clone(), train.id.clone(), cas_types::DependencyType::ParentChild)).unwrap();
            let queue = crate::store::open_prompt_queue_store(&root).unwrap();
            let now = chrono::Utc::now();
            let sweep = |minutes| super::sweep(&root, &session, now + chrono::Duration::minutes(minutes));
            sweep(0);
            sweep(45);
            child.status = TaskStatus::Closed;
            tasks.update(&child).unwrap();
            for minutes in [50, 95, 140, 141] { sweep(minutes); }
            let rows = queue.peek_all(50).unwrap();
            let ready: Vec<_> = rows.iter().filter(|row| row.prompt.contains("kind=\"train_ready\"")).collect();
            assert_eq!(ready.len(), 3, "{rows:?}");
            assert!(ready.iter().all(|row| row.target == "owner-2ba4" && row.prompt.contains(&train.id)));
            assert_eq!(rows.iter().filter(|row| row.target == "operator").count(), 1);
            // A recorded freeze or a foreign owner ends local chasing.
            train.labels.push(format!("release-train-frozen:{}:{}", now.timestamp(), "a".repeat(40)));
            tasks.update(&train).unwrap();
            sweep(185);
            train.labels.retain(|label| !label.starts_with("release-train-frozen:"));
            tasks.update(&train).unwrap();
            // A foreign owner arrives only through the verified authority projection.
            assert!(tasks.project_train_owner(&tasks.get(&train.id).unwrap(), &cas_types::TrainOwnerProjection {
                train: train.id.clone(), project: "fixture".into(), target: train.branch.clone().unwrap(),
                authority: "a".repeat(40), owner: "peer-owner".into(), incarnation: "peer-process".into(),
                epoch: 1, generation: "peer-generation".into(), phase: "active".into(),
            }).unwrap());
            sweep(230);
            assert_eq!(queue.peek_all(50).unwrap().len(), rows.len());
        }
    }

    #[test]
    fn unreadable_refusal_does_not_hide_other_work_cas_6583() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        let mut owner = Agent::new_with_role("owner".into(), "supervisor".into(), AgentRole::Supervisor);
        owner.factory_session = Some("refusal-isolation".into());
        crate::store::open_agent_store(&root).unwrap().register(&owner).unwrap();
        let tasks = crate::store::open_task_store_local(&root).unwrap();
        for id in ["cas-unreadable", "cas-actionable"] {
            let mut task = Task::new(id.into(), id.into());
            task.assignee = Some(owner.name.clone());
            tasks.add(&task).unwrap();
        }
        // Replay an already-corrupt persisted row. The normal writer rightly
        // rejects malformed state before it can reach the watchdog.
        rusqlite::Connection::open(root.join("cas.db")).unwrap().execute(
            "INSERT INTO task_execution_states(task_id,state,updated_at) VALUES (?1,?2,?3)",
            rusqlite::params!["cas-unreadable", serde_json::json!({"close_refusal": {"invalid": true}}).to_string(), chrono::Utc::now().to_rfc3339()],
        ).unwrap();
        let waiting = super::observe(&root, "refusal-isolation", chrono::Utc::now()).unwrap().waiting;
        assert_eq!(waiting.len(), 1);
        assert!(waiting[0].contains("cas-actionable"));
    }

    #[test]
    fn missing_delivery_wake_is_a_one_cycle_defect_cas_f16b() {
        const SESSION: &str = "unarmed-delivery-f16b";
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_FACTORY_SESSION", SESSION);
        let project = env.home().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let root = crate::store::init_cas_dir(&project).unwrap();
        let agents = crate::store::open_agent_store(&root).unwrap();
        let mut owner = Agent::new_with_role("owner-f16b".into(), "supervisor".into(), AgentRole::Supervisor);
        owner.factory_session = Some(SESSION.into());
        agents.register(&owner).unwrap();
        let mut other = Agent::new_with_role("other-f16b".into(), "other-supervisor".into(), AgentRole::Supervisor);
        other.factory_session = Some("other-session".into());
        agents.register(&other).unwrap();
        let mut task = Task::new("cas-f16b-unarmed".into(), "Unwatched delivery".into());
        task.status = TaskStatus::AwaitingMerge;
        task.assignee = Some(owner.id.clone());
        let sha = "1234567890abcdef1234567890abcdef12345678";
        task.deliverables.factory_branch_anchor = Some(sha.into());
        task.deliverables.parked_branch = Some("factory/worker".into());
        task.deliverables.work_target = Some(cas_types::WorkTarget { repo_selector: "project:fixture".into(), target_branch: "main".into() });
        crate::store::open_task_store_local(&root).unwrap().add(&task).unwrap();
        crate::factory_delivery_wake::arm_event(&root, &other, &task.id, "ci_completed", &serde_json::json!({"sha": sha, "workflow":"CI"}), false).unwrap();
        let queue = crate::store::open_prompt_queue_store(&root).unwrap();
        let now = chrono::Utc::now();
        // Inside the delivery's own arming grace nothing is reported yet.
        super::sweep(&root, SESSION, now);
        assert!(queue.peek_all(20).unwrap().is_empty());
        let now = now + super::ARMING_GRACE;
        super::sweep(&root, SESSION, now);
        let rows = queue.peek_all(20).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, owner.name);
        assert!(rows[0].source.starts_with(&format!("lifecycle-wake:{}:", crate::factory_delivery_wake::MISSING_WAKE)));
        for minutes in [45, 90] {
            super::sweep(&root, SESSION, now + chrono::Duration::minutes(minutes));
        }
        assert_eq!(queue.peek_all(20).unwrap().len(), 1);
        crate::factory_delivery_wake::arm_task(&root, &task, &owner.id).unwrap();
        assert!(crate::factory_delivery_wake::watched(&root, &task, &owner).unwrap());
        super::sweep(&root, SESSION, now + chrono::Duration::minutes(91));
        assert_eq!(queue.peek_all(20).unwrap().len(), 1);
    }

    #[test]
    fn dependency_wait_does_not_stall_cas_74d3() {
        const SESSION: &str = "dependency-wait-74d3";
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_FACTORY_SESSION", SESSION);
        let project = env.home().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let cas_dir = crate::store::init_cas_dir(&project).unwrap();
        let mut supervisor = Agent::new("supervisor-74d3-dep".into(), "supervisor".into());
        (supervisor.role, supervisor.factory_session) = (AgentRole::Supervisor, Some(SESSION.into()));
        crate::store::open_agent_store(&cas_dir).unwrap().register(&supervisor).unwrap();
        let tasks = crate::store::open_task_store_local(&cas_dir).unwrap();
        let mut waiting = Task::new("cas-74d3-waiting".into(), "Finished work waiting for the next cut".into());
        (waiting.status, waiting.assignee) = (TaskStatus::Blocked, Some(supervisor.name.clone()));
        tasks.add(&waiting).unwrap();
        let mut cut = Task::new("cas-74d3-cut".into(), "Morning cut decision".into());
        tasks.add(&cut).unwrap();
        tasks.add_dependency(&cas_types::Dependency::new(
            waiting.id.clone(), cut.id.clone(), cas_types::DependencyType::Blocks,
        )).unwrap();
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let now = chrono::Utc::now();
        for minutes in [0, 45, 90, 180] {
            super::sweep(&cas_dir, SESSION, now + chrono::Duration::minutes(minutes));
        }
        assert!(queue.peek_all(50).unwrap().is_empty());

        // Resolving the dependency makes unfinished local work actionable again.
        cut.status = TaskStatus::Closed;
        tasks.update(&cut).unwrap();
        waiting.status = TaskStatus::InProgress;
        tasks.update(&waiting).unwrap();
        for minutes in [181, 226, 271] {
            super::sweep(&cas_dir, SESSION, now + chrono::Duration::minutes(minutes));
        }
        let rows = queue.peek_all(50).unwrap();
        assert_eq!(rows.iter().filter(|row| row.target == "supervisor").count(), 2, "{rows:?}");
        assert_eq!(rows.iter().filter(|row| row.target == "operator").count(), 1, "{rows:?}");
    }

    #[test]
    fn held_worker_does_not_stall_cas_74d3() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        for by_id in [false, true] {
            let session = format!("held-worker-74d3-{by_id}");
            env.set("CAS_FACTORY_SESSION", &session);
            let project = env.home().join(&session);
            std::fs::create_dir_all(&project).unwrap();
            let cas_dir = crate::store::init_cas_dir(&project).unwrap();
            let agents = crate::store::open_agent_store(&cas_dir).unwrap();
            let mut supervisor = Agent::new("supervisor-74d3-hold".into(), "supervisor".into());
            (supervisor.role, supervisor.factory_session) = (AgentRole::Supervisor, Some(session.clone()));
            agents.register(&supervisor).unwrap();
            let mut worker = Agent::new("worker-74d3".into(), "held-worker".into());
            (worker.role, worker.factory_session) = (AgentRole::Worker, Some(session.clone()));
            agents.register(&worker).unwrap();
            let path = crate::ui::factory::session::metadata_path(&session);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let metadata = crate::ui::factory::session::create_metadata(
                &session, 1, &supervisor.name, &[worker.name.clone()], None, None, None,
            );
            std::fs::write(&path, serde_json::to_string(&metadata).unwrap()).unwrap();
            crate::ui::factory::persist_session_metadata_worker_hold_at(&path, &worker.name, true).unwrap();
            let tasks = crate::store::open_task_store_local(&cas_dir).unwrap();
            let mut task = Task::new("cas-74d3-held".into(), "Intentionally paused worker".into());
            (task.status, task.assignee) = (TaskStatus::InProgress,
                Some(if by_id { worker.id.clone() } else { worker.name.clone() }));
            tasks.add(&task).unwrap();
            let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
            let now = chrono::Utc::now();
            for minutes in [0, 45, 90, 180] {
                super::sweep(&cas_dir, &session, now + chrono::Duration::minutes(minutes));
            }
            assert!(queue.peek_all(50).unwrap().is_empty(), "assignee by id: {by_id}");

            // Releasing a hold without changing the task restores ordinary escalation.
            crate::ui::factory::persist_session_metadata_worker_hold_at(&path, &worker.name, false).unwrap();
            for minutes in [181, 226, 271] {
                super::sweep(&cas_dir, &session, now + chrono::Duration::minutes(minutes));
            }
            let rows = queue.peek_all(50).unwrap();
            assert_eq!(rows.iter().filter(|row| row.target == "supervisor").count(), 2, "{rows:?}");
            assert_eq!(rows.iter().filter(|row| row.target == "operator").count(), 1, "{rows:?}");
        }
    }

    #[test]
    fn operator_blocked_epic_does_not_stall_cas_e239() {
        const SESSION: &str = "operator-blocked-epic-e239";
        let _env = crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", SESSION)]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        let mut supervisor = Agent::new("supervisor-e239".into(), "supervisor".into());
        supervisor.role = AgentRole::Supervisor;
        supervisor.factory_session = Some(SESSION.into());
        crate::store::open_agent_store(&cas_dir).unwrap().register(&supervisor).unwrap();
        let tasks = crate::store::open_task_store_local(&cas_dir).unwrap();
        let mut epic = Task::new("cas-9be9".into(), "Release verification".into());
        epic.task_type = cas_types::TaskType::Epic;
        epic.status = TaskStatus::Blocked;
        epic.assignee = Some(supervisor.name.clone());
        epic.notes = "[2026-11-06 18:00] ✅ DECISION HOLD owner: operator; deferred desktop verification.".into();
        tasks.add(&epic).unwrap();
        // GH #1904: a worker's older blocker note must not outrank the newer
        // structured hold the supervisor recorded for the operator's decision.
        let mut worker = Agent::new("worker-285a".into(), "zen-badger-285a".into());
        worker.role = AgentRole::Worker;
        // The incident had no live worker: its worker was already shut down.
        worker.status = AgentStatus::Shutdown;
        worker.factory_session = Some(SESSION.into());
        crate::store::open_agent_store(&cas_dir).unwrap().register(&worker).unwrap();
        let mut held = Task::new("cas-285a-held".into(), "Held delivery".into());
        held.status = TaskStatus::Blocked;
        held.assignee = Some(worker.name.clone());
        // The common sequence: the worker blocks, and the supervisor's hold
        // follows within the same minute.
        held.notes = format!("[{}] 🚫 BLOCKER owner: worker; rebase failed.", chrono::Utc::now().format("%Y-%m-%d %H:%M"));
        tasks.add(&held).unwrap();
        let hold = |reason: &str| serde_json::json!({"blocked_hold": cas_types::TaskBlockedHold {
            reason: reason.into(), who: supervisor.name.clone(), when: chrono::Utc::now(),
        }});
        tasks.patch_execution_state(&held.id, &hold("held for operator decision, do not page tonight")).unwrap();
        let mut child = Task::new("cas-e239-merged".into(), "All release work merged".into());
        child.status = TaskStatus::Closed;
        tasks.add(&child).unwrap();
        tasks.add_dependency(&cas_types::Dependency::new(
            child.id.clone(), epic.id.clone(), cas_types::DependencyType::ParentChild,
        )).unwrap();
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let now = chrono::Utc::now();
        for minutes in [0, 45, 90, 135, 180] {
            super::sweep(&cas_dir, SESSION, now + chrono::Duration::minutes(minutes));
        }
        assert!(queue.peek_all(50).unwrap().is_empty(),
            "GH #1625: an operator-owned blocked epic must never nag the supervisor or page the operator");

        // A newer blocker returns responsibility to the factory. Historical
        // operator ownership must not hide this genuine stall.
        epic.notes.push_str("\n\n[2026-11-06 18:01] 🚫 BLOCKER owner: worker; release repair has not started; operator check is complete.");
        tasks.update(&epic).unwrap();
        // The same task held by the factory again (a newer hold naming the
        // worker) is a stall like any other, so this episode still pages once.
        tasks.patch_execution_state(&held.id, &hold("owner: worker; waiting on a rebase")).unwrap();
        for minutes in [181, 226, 271, 316] {
            super::sweep(&cas_dir, SESSION, now + chrono::Duration::minutes(minutes));
        }
        let rows = queue.peek_all(50).unwrap();
        assert_eq!(rows.iter().filter(|row| row.target == "supervisor").count(), 3,
            "a genuine local stall must keep waking the supervisor: {rows:?}");
        assert_eq!(rows.iter().filter(|row| row.target == "operator").count(), 1,
            "a genuine local stall must still page once: {rows:?}");
        assert!(rows.iter().filter(|row| row.target == "supervisor").all(|row| row.prompt.contains(&held.id)),
            "the wake must name the held task once ownership returns to the worker: {rows:?}");
    }

    #[test]
    fn held_deliveries_and_release_commits_are_not_stalls_cas_e239() {
        const SESSION: &str = "held-release-e239";
        let _env = crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", SESSION)]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(temp.path())
            .args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap().status.success());
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "base"]);
        git(&["branch", "epic/release-train-cas-e239"]);
        let mut supervisor = Agent::new("supervisor-e239b".into(), "supervisor".into());
        supervisor.role = AgentRole::Supervisor;
        supervisor.factory_session = Some(SESSION.into());
        crate::store::open_agent_store(&cas_dir).unwrap().register(&supervisor).unwrap();
        let tasks = crate::store::open_task_store_local(&cas_dir).unwrap();
        let mut held = Task::new("cas-2f4d".into(), "Held for the next train".into());
        held.status = TaskStatus::AwaitingMerge;
        held.assignee = Some(supervisor.name.clone());
        held.notes = "[2026-11-07 12:00] ✅ DECISION HOLD: merge on the next train after the operator scope freeze.".into();
        tasks.add(&held).unwrap();
        let mut waiting = Task::new("cas-78d2".into(), "Waiting on the supervisor".into());
        waiting.assignee = Some(supervisor.name.clone());
        tasks.add(&waiting).unwrap();
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let now = chrono::Utc::now();
        // The release commit lands on the train mid-episode: progress, so no wake.
        super::sweep(&cas_dir, SESSION, now);
        super::sweep(&cas_dir, SESSION, now + chrono::Duration::minutes(30));
        git(&["checkout", "-q", "epic/release-train-cas-e239"]);
        git(&["commit", "-q", "--allow-empty", "-m", "release: v4.4.0"]);
        super::sweep(&cas_dir, SESSION, now + chrono::Duration::minutes(50));
        super::sweep(&cas_dir, SESSION, now + chrono::Duration::minutes(80));
        assert!(queue.peek_all(50).unwrap().is_empty(), "a release commit is progress: {:?}", queue.peek_all(50).unwrap());
        // Only the held delivery left open: nothing is locally actionable.
        let mut done = tasks.get("cas-78d2").unwrap();
        done.status = TaskStatus::Closed;
        tasks.update(&done).unwrap();
        for minutes in [81, 130, 180, 230, 280] {
            super::sweep(&cas_dir, SESSION, now + chrono::Duration::minutes(minutes));
        }
        assert!(queue.peek_all(50).unwrap().is_empty(), "a held delivery is not a stall");
    }

    /// GH #2278: a parked delivery the supervisor holds for an external
    /// review wakes and pages no one until the hold's deadline. After it, the
    /// delivery is chased again from a fresh quiet period.
    #[test]
    fn external_review_hold_quiets_a_parked_delivery_until_its_deadline_gh2278() {
        const SESSION: &str = "external-review-2278";
        let _env = crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", SESSION)]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        let mut supervisor = Agent::new("supervisor-2278".into(), "supervisor-2278".into());
        (supervisor.role, supervisor.factory_session) = (AgentRole::Supervisor, Some(SESSION.into()));
        agents.register(&supervisor).unwrap();
        let mut worker = Agent::new("worker-2278".into(), "worker-2278".into());
        (worker.role, worker.status, worker.factory_session) = (AgentRole::Worker, AgentStatus::Shutdown, Some(SESSION.into()));
        agents.register(&worker).unwrap();
        let tasks = crate::store::open_task_store_local(&cas_dir).unwrap();
        let anchor = "6f92".repeat(10);
        let mut parked = Task::new("cas-6f92".into(), "Prompt text awaiting the product owner's review".into());
        (parked.status, parked.assignee) = (TaskStatus::AwaitingMerge, Some(worker.name.clone()));
        parked.deliverables.factory_branch_anchor = Some(anchor.clone());
        parked.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:fixture".into(), target_branch: "main".into(),
        });
        // The reported decision note: prose alone never holds a delivery.
        parked.notes = "[2026-11-21 03:00] ✅ DECISION Not stalled: HELD BY DESIGN for Ben's GitHub review".into();
        tasks.add(&parked).unwrap();
        crate::factory_delivery_wake::arm_task(&cas_dir, &parked, &worker.id).unwrap();
        let now = chrono::Utc::now();
        tasks.patch_execution_state(&parked.id, &serde_json::json!({"merge_hold": cas_types::TaskMergeHold {
            owner: "Ben".into(), reason: "GitHub review of prompt text".into(),
            until: now + chrono::Duration::minutes(180), supervisor: supervisor.name.clone(), anchor,
        }})).unwrap();
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let sweep = |minutes| super::sweep(&cas_dir, SESSION, now + chrono::Duration::minutes(minutes));
        for minutes in [0, 45, 90, 135, 179] { sweep(minutes); }
        let rows = queue.peek_all(50).unwrap();
        assert!(!rows.iter().any(|row| row.prompt.contains("progress_stalled") || row.target == "operator"), "{rows:?}");
        for minutes in [181, 226, 271] { sweep(minutes); }
        let rows = queue.peek_all(50).unwrap();
        let wakes = rows.iter().filter(|row| row.target == supervisor.name && row.prompt.contains("progress_stalled")).count();
        let pages = rows.iter().filter(|row| row.target == "operator" && row.prompt.contains("cas-6f92")).count();
        assert_eq!((wakes, pages), (2, 1), "a lapsed hold is chased again: {rows:?}");
    }

    #[test]
    fn only_locally_actionable_work_pages_cas_c1fb() {
        use cas_store::{EpicSplit, SplitDirection, SplitState};
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let repo = env.home().join("project");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").args(args).current_dir(&repo).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&["init", "--initial-branch=main"]);
        git(&["-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-m", "base"]);
        git(&["checkout", "-b", "factory/delivery"]);
        git(&["-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-m", "delivery"]);
        let anchor = git(&["rev-parse", "HEAD"]);
        git(&["branch", "epic/train"]);
        git(&["update-ref", "refs/remotes/origin/epic/peer-train", &anchor]);
        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        let tasks = crate::store::open_task_store_local(&cas_dir).unwrap();
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let now = chrono::Utc::now();
        let mut peer_epic = Task::new("cas-peer-epic".into(), "Peer's release train".into());
        peer_epic.task_type = cas_types::TaskType::Epic;
        peer_epic.branch = Some("epic/owned-by-peer".into());
        tasks.add(&peer_epic).unwrap();
        cas_store::insert_epic_split(&cas_dir, &EpicSplit {
            id: "split-c1fb".into(), epic_id: "cas-shared-epic".into(),
            local_agent_id: "local-supervisor".into(), peer_agent_id: "peer-supervisor".into(),
            peer_name: "peer".into(), peer_machine: "other-host".into(),
            direction: SplitDirection::Outgoing, mine: vec![],
            theirs: vec!["cas-c1fb-peer-handoff".into(), "cas-c1fb-local-work".into(), peer_epic.id.clone()],
            state: SplitState::Accepted, reason: None, message_id: None,
            expires_at: now + chrono::Duration::days(1), created_at: now,
            decided_at: Some(now), decided_by: Some("local-supervisor".into()),
            cleanup_pending: false, expiry_notice_pending: false,
        }).unwrap();

        // Replay the peer release wait with zero live workers, then prove
        // that an unmerged delivery and a silent local worker still page.
        for (case, target, status, page) in [
            ("contained", "epic/train", TaskStatus::AwaitingMerge, false),
            ("origin-contained", "epic/peer-train", TaskStatus::AwaitingMerge, false),
            ("peer-handoff", "main", TaskStatus::AwaitingMerge, false),
            ("peer-epic", "main", TaskStatus::AwaitingMerge, false),
            ("peer-branch", "epic/owned-by-peer", TaskStatus::AwaitingMerge, false),
            ("unmerged", "main", TaskStatus::AwaitingMerge, true),
            ("missing-target", "epic/missing", TaskStatus::AwaitingMerge, true),
            ("prose-hold", "main", TaskStatus::AwaitingMerge, true),
            ("local-work", "epic/train", TaskStatus::InProgress, true),
        ] {
            let session = format!("cas-c1fb-{case}");
            env.set("CAS_FACTORY_SESSION", &session);
            let mut supervisor = Agent::new(format!("{session}-supervisor"), format!("supervisor-{case}"));
            (supervisor.role, supervisor.factory_session) = (AgentRole::Supervisor, Some(session.clone()));
            agents.register(&supervisor).unwrap();
            let mut worker = Agent::new(format!("{session}-worker"), format!("worker-{case}"));
            (worker.role, worker.status, worker.factory_session) = (
                AgentRole::Worker,
                if status == TaskStatus::InProgress { AgentStatus::Active } else { AgentStatus::Shutdown },
                Some(session.clone()),
            );
            agents.register(&worker).unwrap();
            let mut task = Task::new(format!("cas-c1fb-{case}"), "Release train delivery".into());
            (task.status, task.assignee) = (status, Some(worker.name.clone()));
            task.deliverables.factory_branch_anchor = Some(anchor.clone());
            task.deliverables.parked_branch = Some("factory/delivery".into());
            task.deliverables.work_target = Some(cas_types::WorkTarget {
                repo_selector: "project:fixture".into(), target_branch: target.into(),
            });
            if case == "prose-hold" {
                task.notes = "[2026-11-06 18:00] ✅ DECISION deliberately not merged yet; peer merges next train".into();
            }
            tasks.add(&task).unwrap();
            if status == TaskStatus::AwaitingMerge {
                crate::factory_delivery_wake::arm_task(&cas_dir, &task, &worker.id).unwrap();
            }
            if case == "peer-epic" {
                tasks.add_dependency(&cas_types::Dependency::new(
                    task.id.clone(), peer_epic.id.clone(), cas_types::DependencyType::ParentChild,
                )).unwrap();
            }
            for minutes in [0, 45, 90] {
                super::sweep(&cas_dir, &session, now + chrono::Duration::minutes(minutes));
            }
            let rows = queue.peek_all(100).unwrap();
            let pages = rows.iter().filter(|row| row.target == "operator" && row.prompt.contains(&task.id)).count();
            assert_eq!(pages, usize::from(page), "{case}: wrong operator escalation");
            let wakes = rows.iter().filter(|row| row.target == supervisor.name && row.prompt.contains(&task.id)).count();
            assert_eq!(wakes, 2 * usize::from(page), "{case}: wrong local stall wake");
        }
        // A cancelled handoff restores local responsibility and starts a
        // fresh quiet period, without waiting for a task mutation.
        assert!(cas_store::decide_epic_split(&cas_dir, "split-c1fb", &[SplitState::Accepted],
            SplitState::Cancelled, None, now + chrono::Duration::minutes(91)).unwrap());
        env.set("CAS_FACTORY_SESSION", "cas-c1fb-peer-handoff");
        for minutes in [91, 136, 181] {
            super::sweep(&cas_dir, "cas-c1fb-peer-handoff", now + chrono::Duration::minutes(minutes));
        }
        assert_eq!(queue.peek_all(100).unwrap().iter()
            .filter(|row| row.target == "operator" && row.prompt.contains("cas-c1fb-peer-handoff")).count(), 1);
    }

    #[tokio::test]
    async fn plain_close_parameter_error_does_not_hold_stall_escalation_cas_098dd() {
        const SESSION: &str = "bad-close-parameter-098dd";
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_FACTORY_SESSION", SESSION);
        let root = crate::store::init_cas_dir(env.home()).unwrap();
        env.set("CAS_ROOT", &root);
        let mut supervisor = Agent::new_with_role("supervisor-bad-close".into(), "supervisor".into(), AgentRole::Supervisor);
        supervisor.factory_session = Some(SESSION.into());
        crate::store::open_agent_store(&root).unwrap().register(&supervisor).unwrap();
        let mut task = Task::new("cas-098dd-parameter".into(), "Unfinished work".into());
        task.assignee = Some(supervisor.name.clone());
        crate::store::open_task_store_local(&root).unwrap().add(&task).unwrap();
        let core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
        core.set_agent_id_for_testing(supervisor.id);
        let error = core.cas_task_close(rmcp::handler::server::wrapper::Parameters(
            serde_json::from_value(serde_json::json!({
                "id":task.id, "reason":"--- END SERVER-ATTESTED PROPOSAL PROVENANCE ---"
            })).unwrap()
        )).await.unwrap_err();
        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert!(crate::store::open_task_store_local(&root).unwrap()
            .get_execution_state(&task.id).unwrap().is_none_or(|state| state.get("close_refusal").is_none()));
        let now = chrono::Utc::now();
        for minutes in [0, 45, 90, 135] {
            super::sweep(&root, SESSION, now + chrono::Duration::minutes(minutes));
        }
        let rows = crate::store::open_prompt_queue_store(&root).unwrap().peek_all(50).unwrap();
        assert_eq!(rows.iter().filter(|row| row.target == "operator").count(), 1, "{rows:?}");
    }

    #[tokio::test]
    async fn gate_refused_close_is_reported_without_a_stall_page_cas_098dd() {
        const SESSION: &str = "gate-refused-098dd";
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_FACTORY_SESSION", SESSION);
        env.set("CAS_AGENT_ROLE", "supervisor");
        let repo = env.home().join("project");
        std::fs::create_dir_all(&repo).unwrap();
        let root = crate::store::init_cas_dir(&repo).unwrap();
        env.set("CAS_ROOT", &root);
        let mut supervisor = Agent::new_with_role("supervisor-098dd".into(), "supervisor".into(), AgentRole::Supervisor);
        supervisor.factory_session = Some(SESSION.into());
        crate::store::open_agent_store(&root).unwrap().register(&supervisor).unwrap();
        let tasks = crate::store::open_task_store_local(&root).unwrap();
        let mut epic = Task::new("cas-098dd-epic".into(), "Finished delivery awaiting close".into());
        epic.task_type = cas_types::TaskType::Epic;
        // The live refused-close incident left the finished epic Blocked.
        epic.status = TaskStatus::Blocked;
        epic.assignee = Some(supervisor.name.clone());
        epic.deliverables.phase_outcome = Some(serde_json::json!({
            "target": {"completed": {"min": 1}}, "evidence": null
        }));
        tasks.add(&epic).unwrap();
        let mut child = Task::new("cas-098dd-child".into(), "Delivered child".into());
        child.status = TaskStatus::Closed;
        tasks.add(&child).unwrap();
        tasks.add_dependency(&cas_types::Dependency::new(child.id, epic.id.clone(), cas_types::DependencyType::ParentChild)).unwrap();
        let core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
        core.set_agent_id_for_testing(supervisor.id);
        let refused = core.cas_task_close(rmcp::handler::server::wrapper::Parameters(
            serde_json::from_value(serde_json::json!({"id":epic.id,"reason":"Delivered work is ready"})).unwrap()
        )).await.unwrap();
        assert_eq!(refused.is_error, Some(true));
        assert_eq!(refused.structured_content.as_ref().unwrap()["code"], "phase_outcome_gate");
        assert_eq!(tasks.get_execution_state(&epic.id).unwrap().unwrap()["close_refusal"]["code"], "phase_outcome_gate");
        let queue = crate::store::open_prompt_queue_store(&root).unwrap();
        let now = chrono::Utc::now();
        for minutes in [0, 45, 90, 135] {
            super::sweep(&root, SESSION, now + chrono::Duration::minutes(minutes));
        }
        let rows = queue.peek_all(50).unwrap();
        assert_eq!(rows.iter().filter(|row| row.target == "operator").count(), 0, "{rows:?}");
        let notices: Vec<_> = rows.iter().filter(|row| row.prompt.contains("phase_outcome_gate")).collect();
        assert_eq!(notices.len(), 1, "{rows:?}");
        assert!(notices[0].prompt.contains("provide_outcome_receipt"));

        // Timeline notes do not resolve the gate or start an escalation clock.
        epic.notes.push_str("\nMore diagnostic evidence");
        tasks.update(&epic).unwrap();
        super::sweep(&root, SESSION, now + chrono::Duration::minutes(180));
        assert_eq!(queue.peek_all(50).unwrap().iter().filter(|row| row.target == "operator").count(), 0);
        // A changed close input makes the old refusal stale; ordinary stalls
        // must still page, without requiring a process restart.
        epic.deliverables.phase_outcome = None;
        tasks.update(&epic).unwrap();
        for minutes in [181, 226, 271] {
            super::sweep(&root, SESSION, now + chrono::Duration::minutes(minutes));
        }
        assert!(tasks.get_execution_state(&epic.id).unwrap()
            .is_none_or(|state| state.get("close_refusal").is_none()));
        assert_eq!(queue.peek_all(50).unwrap().iter().filter(|row| row.target == "operator").count(), 1);

        // Another real close gate used to be omitted by the identity-only
        // recorder. Its untyped response must be held too, while a specific
        // identity still wins when supplied by the gate.
        epic.status = TaskStatus::Closed;
        tasks.update(&epic).unwrap();
        assert!(!crate::qa_post_release::batch_notice_deliverable(&root, queue.as_ref(), notices[0]).unwrap());
        let mut post_release = Task::new("cas-098dd-post-release".into(), "Post-release observation pending".into());
        post_release.task_type = cas_types::TaskType::Epic;
        post_release.status = TaskStatus::Blocked;
        post_release.assignee = epic.assignee.clone();
        tasks.add(&post_release).unwrap();
        std::fs::write(root.join("config.toml"), "[qa.post_release]\ncommand = 'probe'\n").unwrap();
        let refused = core.cas_task_close(rmcp::handler::server::wrapper::Parameters(
            serde_json::from_value(serde_json::json!({"id":post_release.id,"reason":"Delivered work is ready"})).unwrap()
        )).await.unwrap();
        assert_eq!(refused.is_error, Some(true));
        for (metadata, code) in [(None, "close_gate"), (refused.structured_content.clone(), "post_release_gate")] {
            let mut reply = refused.clone();
            reply.structured_content = metadata;
            crate::mcp::tools::core::task::close_refusal::record(&root, &post_release.id, &Ok(reply)).unwrap();
            for minutes in [300, 345, 390, 435] {
                super::sweep(&root, SESSION, now + chrono::Duration::minutes(minutes));
            }
            let rows = queue.peek_all(50).unwrap();
            assert_eq!(rows.iter().filter(|row| row.target == "operator" && row.prompt.contains(&post_release.id)).count(), 0, "{rows:?}");
            assert_eq!(tasks.get_execution_state(&post_release.id).unwrap().unwrap()["close_refusal"]["code"], code);
            assert_eq!(rows.iter().filter(|row| row.prompt.contains(code) && row.prompt.contains(&post_release.id)).count(), 1, "{rows:?}");
        }
    }

    /// cas-1f65: the Nov 9 overnight. A train frozen for release (lift is
    /// operator-only) holds two parked children. The supervisor hears once
    /// that the freeze holds them and who lifts it; nothing is chased as a
    /// stall or paged. After the lift they are local work again.
    #[test]
    fn deliveries_held_by_a_train_freeze_are_reported_once_not_paged_cas_1f65() {
        use crate::mcp::tools::core::workflow::release_freeze;
        const SESSION: &str = "frozen-train-1f65";
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_FACTORY_SESSION", SESSION);
        let repo = env.home().join("project");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").current_dir(&repo)
                .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"]).args(args).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "base"]);
        let lane = "epic/release-train-vnext-cas-db63";
        git(&["branch", lane]);
        git(&["checkout", "-q", "-b", "factory/delivery"]);
        git(&["commit", "-q", "--allow-empty", "-m", "delivery"]);
        let anchor = git(&["rev-parse", "HEAD"]);
        let cas_dir = crate::store::init_cas_dir(&repo).unwrap();
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        for (name, role) in [("ready-cobra-72", AgentRole::Supervisor), ("bright-hound-47", AgentRole::Worker)] {
            let mut agent = Agent::new(format!("{name}-id"), name.into());
            (agent.role, agent.factory_session) = (role, Some(SESSION.into()));
            agents.register(&agent).unwrap();
        }
        let tasks = crate::store::open_task_store_local(&cas_dir).unwrap();
        let mut train = Task::new("cas-db63".into(), "Release train vNEXT".into());
        (train.task_type, train.labels, train.branch) =
            (cas_types::TaskType::Epic, vec!["release-train".into()], Some(lane.into()));
        tasks.add(&train).unwrap();
        for id in ["cas-0f47", "cas-1d99"] {
            let mut task = Task::new(id.into(), "Parked train child".into());
            (task.status, task.assignee) = (TaskStatus::AwaitingMerge, Some("bright-hound-47".into()));
            task.deliverables.factory_branch_anchor = Some(anchor.clone());
            task.deliverables.work_target = Some(cas_types::WorkTarget {
                repo_selector: "project:fixture".into(), target_branch: lane.into(),
            });
            tasks.add(&task).unwrap();
        }
        release_freeze::freeze(&cas_dir, "cas-db63", "operator", None).unwrap();
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let now = chrono::Utc::now();
        for minutes in [0, 45, 90, 135, 180] {
            super::sweep(&cas_dir, SESSION, now + chrono::Duration::minutes(minutes));
        }
        let rows = queue.peek_all(50).unwrap();
        assert!(rows.iter().all(|row| row.target != "operator" && !row.prompt.contains("progress_stalled")),
            "a frozen train is not a stall: {rows:?}");
        let held: Vec<_> = rows.iter().filter(|row| row.prompt.contains("kind=\"progress_operator_held\"")).collect();
        assert_eq!(held.len(), 1, "one notice per freeze: {rows:?}");
        assert_eq!(held[0].target, "ready-cobra-72");
        assert!(["cas-0f47", "cas-1d99", "cas-db63"].iter().all(|id| held[0].prompt.contains(id)), "{}", held[0].prompt);

        // The lift makes them mergeable here again: an ordinary stall.
        release_freeze::lift(&cas_dir, "cas-db63", "operator").unwrap();
        for minutes in [181, 226, 271] {
            super::sweep(&cas_dir, SESSION, now + chrono::Duration::minutes(minutes));
        }
        let rows = queue.peek_all(50).unwrap();
        assert_eq!(rows.iter().filter(|row| row.prompt.contains("progress_stalled")).count(), 2, "{rows:?}");
        assert_eq!(rows.iter().filter(|row| row.target == "operator").count(), 1, "{rows:?}");
    }

    /// GH #1500 / #1302: an overnight with a live, heartbeating but silent
    /// peer and an idle supervisor whose only wake is event-based. The
    /// supervisor is woken every 45 min, the operator is paged once at
    /// 90, heartbeats never reset the clock, and real movement starts a new
    /// episode.
    #[test]
    fn an_overnight_silent_live_peer_wakes_the_supervisor_at_n_and_pages_the_operator_at_2n_gh1500() {
        const SESSION: &str = "overnight-silent-peer";
        let _env = crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", SESSION)]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        for (id, name, role, session) in [("supervisor-id", "supervisor", AgentRole::Supervisor, SESSION),
            ("worker-id", "busy-otter", AgentRole::Worker, SESSION),
            ("peer-id", "lively-cheetah-57", AgentRole::Supervisor, "peer-session")]
        {
            let mut agent = Agent::new(id.into(), name.into());
            (agent.role, agent.status, agent.factory_session) = (role, AgentStatus::Active, Some(session.into()));
            agents.register(&agent).unwrap();
        }
        let mut gate = Task::new("cas-gate".into(), "Release batch awaiting peer review".into());
        (gate.status, gate.assignee) = (TaskStatus::AwaitingMerge, Some("busy-otter".into()));
        crate::store::open_task_store_local(&cas_dir).unwrap().add(&gate).unwrap();
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        queue.enqueue_with_session("supervisor", "lively-cheetah-57", "ETA on your review?", SESSION).unwrap();
        // The supervisor's only wake is event-based: it can never fire.
        crate::store::open_reminder_store(&cas_dir).unwrap()
            .create_with_scope("supervisor-id", None, "CI done", cas_store::ReminderTriggerType::Event, None,
                Some("task_completed"), Some(&serde_json::json!({ "task_id": "cas-gate" })), 0, Some(SESSION), None, false, None)
            .unwrap();
        let rows = |target: &str| queue.peek_all(50).unwrap().into_iter()
            .filter(|r| r.target == target && r.factory_session.as_deref() == Some(SESSION)).collect::<Vec<_>>();
        let t0 = chrono::Utc::now() + chrono::Duration::minutes(1);
        let sweep = |mins: i64| super::sweep(&cas_dir, SESSION, t0 + chrono::Duration::minutes(mins));

        for mins in [0, 10, 30, 44] {
            agents.heartbeat("worker-id").unwrap();
            agents.heartbeat("peer-id").unwrap();
            sweep(mins);
        }
        assert!(rows("supervisor").is_empty(), "heartbeats are not progress");
        sweep(45);
        let woke = rows("supervisor");
        assert_eq!(woke.len(), 1, "{woke:?}");
        // The task id identifies the work this wake asks the owner to resolve.
        assert!(woke[0].prompt.contains("cas-gate"), "{}", woke[0].prompt);
        assert!(crate::prompt_revalidation::parse_blocker_envelope(&woke[0].prompt).is_some());
        sweep(60);
        assert!(rows("operator").is_empty(), "the wake itself is not progress");
        sweep(90);
        sweep(7 * 60);
        let paged = rows("operator");
        assert_eq!(paged.len(), 1, "one page per episode: {paged:?}");
        // pin: the page names the waiting task and the woken supervisor
        assert!(paged[0].prompt.contains("cas-gate") && paged[0].prompt.contains("supervisor"), "{}", paged[0].prompt);
        assert_eq!(rows("supervisor").len(), 3, "one wake per sampled stall cadence");

        // The peer finally answers, which is not this session's progress;
        // the supervisor then acts, which starts a new episode.
        queue.enqueue_with_session("lively-cheetah-57", "supervisor", "approved", "peer-session").unwrap();
        queue.enqueue_with_session("supervisor", "busy-otter", "merging now", SESSION).unwrap();
        sweep(7 * 60 + 1);
        sweep(7 * 60 + 46);
        assert_eq!(rows("supervisor").len(), 4, "movement starts a new episode");
    }
}
