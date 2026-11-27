//! One integration per release: fixes merge to the train, the train goes
//! through the gate once.
//!
//! A task created with no epic and no explicit target joins the repository's
//! open release-train epic (cut from the integration branch) instead of
//! targeting that branch directly, so its delivery merges locally into the
//! train lane. A landed train is finalized by release reconciliation; the next
//! train opens lazily on the next create.

use std::fs::File;
use std::path::Path;

use fs2::FileExt;

use rmcp::ErrorData as McpError;

use crate::mcp::server::CasCore;
use crate::mcp::tools::types::TaskCreateRequest;
use crate::types::{Task, TaskStatus, TaskType};

pub(crate) const RELEASE_TRAIN_LABEL: &str = "release-train";
pub(crate) const RELEASE_TRAIN_CUT_LABEL: &str = "release-train-cut";
const RELEASE_TRAIN_TITLE: &str = "Release train vNEXT";

pub(crate) fn is_release_train(task: &Task) -> bool {
    task.task_type == TaskType::Epic && task.labels.iter().any(|l| l == RELEASE_TRAIN_LABEL)
}

pub(crate) fn is_cut_train(task: &Task) -> bool {
    is_release_train(task) && task.labels.iter().any(|label| label == RELEASE_TRAIN_CUT_LABEL)
}

/// Active trains bound for `integration`, oldest first. A train cut for another
/// integration branch (the setting changed) is left for its owner to land.
pub(crate) fn active_trains(store: &dyn cas_store::TaskStore, integration: &str) -> Vec<Task> {
    let mut trains: Vec<Task> = store
        .list(None)
        .unwrap_or_default()
        .into_iter()
        .filter(|task| {
            is_release_train(task)
                && !task.is_terminal()
                && task
                    .deliverables
                    .work_target
                    .as_ref()
                    .is_some_and(|target| target.target_branch == integration)
        })
        .collect();
    trains.sort_by_key(|task| task.created_at);
    trains
}

/// Prepare an operator-chosen epic for the same admission consumers as an
/// ordinary release train. The caller holds the train lock and persists this
/// together with the freeze, so a failed validation cannot leave half a train.
pub(crate) fn prepare_promotion(
    store: &dyn cas_store::TaskStore,
    repo: &Path,
    task: &mut Task,
    tip: &str,
) -> anyhow::Result<()> {
    use anyhow::Context;
    let (_, integration) = super::repo_context::integration_branch(&repo.join(".cas"))
        .context("Cannot resolve the release integration branch")?;
    let git = crate::worktree::GitOperations::new(repo.to_path_buf());
    let mut divergent = Vec::new();
    for other in open_trains(store, &integration).into_iter().filter(|other| other.id != task.id) {
        if !other.deliverables.work_target.as_ref()
            .is_some_and(|target| super::repo_context::repo_answers_to(repo, &target.repo_selector)) {
            continue;
        }
        let lane = other.effective_epic_branch()
            .with_context(|| format!("Release train {} has no lane", other.id))?;
        let other_tip = git.fetch_merge_target(lane).ok().flatten()
            .or_else(|| branch_tip(repo, lane))
            .with_context(|| format!("Release train {} lane `{lane}` is unavailable; fetch origin", other.id))?;
        if !is_ancestor(repo, tip, &other_tip) && !is_ancestor(repo, &other_tip, tip) {
            divergent.push(other.id);
        }
    }
    if !divergent.is_empty() {
        divergent.push(task.id.clone());
        divergent.sort();
        anyhow::bail!("Divergent release trains: {}. Reconcile their lanes before promotion.", divergent.join(", "));
    }
    let mut target = match task.deliverables.work_target.clone() {
        Some(target) => target,
        None => super::repo_context::standalone_work_target(&repo.join(".cas"))
            .map_err(anyhow::Error::msg)?.0.context("Cannot resolve the epic repository selector")?,
    };
    target.target_branch = integration;
    task.deliverables.work_target = Some(target);
    if !is_release_train(task) {
        task.labels.push(RELEASE_TRAIN_LABEL.into());
    }
    Ok(())
}

/// Trains still accepting new work. A cut train remains active for its
/// landing/installation receipts, but the next task opens another train.
pub(crate) fn open_trains(store: &dyn cas_store::TaskStore, integration: &str) -> Vec<Task> {
    active_trains(store, integration).into_iter().filter(|train| !is_cut_train(train)).collect()
}

fn ref_exists(repo: &Path, refname: &str) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", &format!("{refname}^{{commit}}")])
        .output()
        .is_ok_and(|output| output.status.success())
}

pub(super) fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .status()
        .is_ok_and(|status| status.success())
}

/// cas-25f1: a train's lane must exist on origin the moment it opens, or a
/// worker retargeted there cannot fetch it. Idempotent: an existing local lane
/// is kept, a lane already on origin is left alone, and the push retries.
fn publish_train_branch(repo: &Path, lane: &str, integration: &str) -> Result<(), String> {
    let run = |args: &[&str]| {
        std::process::Command::new("git").arg("-C").arg(repo).args(args).output()
            .map_err(|error| error.to_string())
    };
    let checked = |args: &[&str]| git_stdout(run(args)?);
    let lane_ref = format!("refs/heads/{lane}");
    if ref_exists(repo, &format!("refs/remotes/origin/{lane}")) {
        return Ok(());
    }
    if !ref_exists(repo, &lane_ref) {
        let base = [format!("origin/{integration}"), integration.to_string()]
            .into_iter().find(|candidate| ref_exists(repo, candidate))
            .ok_or_else(|| format!("no `{integration}` to cut `{lane}` from"))?;
        checked(&["branch", lane, &base])?;
    }
    let git = crate::worktree::GitOperations::new(repo.to_path_buf());
    let tip = git.resolve_commit(&lane_ref).ok_or_else(|| format!("no local `{lane}` tip"))?;
    let upstream = format!("refs/remotes/origin/{integration}");
    if !ref_exists(repo, &upstream) || !is_ancestor(repo, &tip, &upstream) {
        return push_train_ref(repo, &lane_ref, &lane_ref, None);
    }
    if git.resolve_commit("HEAD").as_deref() == Some(tip.as_str())
        && checked(&["status", "--porcelain", "--untracked-files=no"])?.is_empty()
    {
        return push_train_ref(repo, &tip, &lane_ref, None);
    }

    let remote = run(&["ls-remote", "--exit-code", "--refs", "origin", &lane_ref])?;
    if remote.status.code() != Some(2) {
        return git_stdout(remote).map(|_| ());
    }
    let hooks = checked(&["rev-parse", "--path-format=absolute", "--git-path", "hooks"])?;
    let common_dir = checked(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let scratch = tempfile::Builder::new().prefix("cas-train-push-").tempdir_in(common_dir)
        .map_err(|error| error.to_string())?;
    let checkout = scratch.path().join("checkout");
    let checkout_path = checkout.to_str().ok_or("release train checkout path is not UTF-8")?;
    checked(&["worktree", "add", "--quiet", "--detach", checkout_path, &tip])?;
    let pushed = push_train_ref(&checkout, &tip, &lane_ref, Some(&hooks));
    if let Err(error) = checked(&["worktree", "remove", "--force", checkout_path]) {
        let path = scratch.keep();
        let cleanup = format!("could not remove train publication worktree at {}: {error}", path.display());
        return Err(pushed.err().map_or_else(|| cleanup.clone(), |push_error| format!("{push_error}\n{cleanup}")));
    }
    pushed
}

fn git_stdout(output: std::process::Output) -> Result<String, String> {
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    String::from_utf8(output.stdout).map(|stdout| stdout.trim_end().to_string())
        .map_err(|error| error.to_string())
}

fn push_train_ref(repo: &Path, source: &str, destination: &str, hooks: Option<&str>) -> Result<(), String> {
    let refspec = format!("{source}:{destination}");
    let mut last = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_secs(attempt));
        }
        let mut command = std::process::Command::new("git");
        command.arg("-C").arg(repo);
        if let Some(hooks) = hooks {
            command.args(["-c", &format!("core.hooksPath={hooks}")]);
        }
        let output = command.args(["push", "origin", &refspec]).output().map_err(|error| error.to_string())?;
        if output.status.success() {
            return Ok(());
        }
        last = String::from_utf8_lossy(&output.stderr).trim().to_string();
    }
    Err(last)
}

/// Prefer shared origin history over a machine's possibly stale local branch.
pub(super) fn branch_tip(repo: &Path, branch: &str) -> Option<String> {
    [format!("refs/remotes/origin/{branch}"), format!("refs/heads/{branch}")]
        .iter()
        .find_map(|reference| {
            let output = std::process::Command::new("git")
                .arg("-C").arg(repo)
                .args(["rev-parse", "--verify", "--quiet", &format!("{reference}^{{commit}}")])
                .output().ok()?;
            output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        })
}

fn delivered_idle(store: &dyn cas_store::TaskStore, train: &Task) -> bool {
    let children = store.get_subtasks(&train.id).unwrap_or_default();
    children.iter().all(Task::is_terminal)
        && children.iter().any(|child| child.status == TaskStatus::Closed)
}

/// The same repository election serves task creation and release CLI fallback.
/// Contained tips lose to their live successor; only divergent live tips refuse.
pub(crate) fn select_train(
    store: &dyn cas_store::TaskStore,
    repo: &Path,
    integration: &str,
) -> Result<Option<Task>, String> {
    let base = branch_tip(repo, integration)
        .ok_or_else(|| format!("Cannot resolve release integration branch `{integration}`."))?;
    let mut candidates = Vec::new();
    for train in open_trains(store, integration).into_iter().filter(|t| !accepts_no_work(t)) {
        let lane = train.effective_epic_branch()
            .ok_or_else(|| format!("Release train {} has no lane.", train.id))?;
        let tip = branch_tip(repo, lane)
            .ok_or_else(|| format!("Release train {} lane `{lane}` is unavailable; fetch origin before creating work.", train.id))?;
        if (tip != base && is_ancestor(repo, &tip, &base))
            || (tip == base && delivered_idle(store, &train))
        {
            continue;
        }
        candidates.push((train, tip));
    }
    // A live lane carrying work takes precedence over a fresh empty lane,
    // even when main advanced independently since that live lane forked.
    if candidates.iter().any(|(_, tip)| tip != &base) {
        candidates.retain(|(_, tip)| tip != &base);
    }
    let survivors: Vec<_> = candidates.iter().filter(|(_, tip)| {
        !candidates.iter().any(|(_, other)| tip != other && is_ancestor(repo, tip, other))
    }).collect();
    let mut distinct: Vec<_> = survivors.iter().map(|(_, tip)| tip.as_str()).collect();
    distinct.sort_unstable();
    distinct.dedup();
    if distinct.len() > 1 {
        let mut ids: Vec<_> = survivors.iter().map(|(train, _)| train.id.as_str()).collect();
        ids.sort_unstable();
        return Err(format!(
            "Divergent release trains: {}. Choose an explicit epic or reconcile their lanes before creating work.",
            ids.join(", ")
        ));
    }
    Ok(survivors.into_iter()
        .max_by_key(|(train, _)| (train.created_at, &train.id))
        .map(|(train, _)| train.clone()))
}

/// The cut boundary retires admission only. Reconciliation owns terminal state.
/// Fresh equal-main siblings survive, including a just-opened rollover train.
pub(crate) fn mark_cut_trains(
    cas_root: &Path,
    repo: &Path,
    store: &dyn cas_store::TaskStore,
    cut_id: &str,
    cut_tip: &str,
    integration: &str,
    included: &[String],
) -> Result<(), String> {
    let _lock = lock_trains_blocking(cas_root).map_err(|error| error.to_string())?;
    let cut = store.get(cut_id).map_err(|error| error.to_string())?;
    let base = branch_tip(repo, integration)
        .ok_or_else(|| format!("Cannot resolve release integration branch `{integration}`."))?;
    let mut retired = Vec::new();
    for mut train in active_trains(store, integration) {
        if is_cut_train(&train) {
            if train.id == cut_id { retired.push(train); }
            continue;
        }
        let retire = train.id == cut_id || train.effective_epic_branch()
            .and_then(|lane| branch_tip(repo, lane)).is_some_and(|tip| {
                tip != base && (
                    is_ancestor(repo, &tip, &base)
                    || (tip != cut_tip && is_ancestor(repo, &tip, cut_tip))
                    || (tip == cut_tip && (train.created_at, &train.id) < (cut.created_at, &cut.id))
                )
            });
        if retire {
            train.labels.push(RELEASE_TRAIN_CUT_LABEL.into());
            train.updated_at = chrono::Utc::now();
            store.update(&train).map_err(|error| error.to_string())?;
            retired.push(train);
        }
    }
    // An explicitly cut train rolls over even when default train joining is
    // disabled (for example an operator-promoted epic).
    if !retired.is_empty() {
        let successor = ensure_successor(repo, store, &cut, integration)?;
        for train in retired.iter().filter(|train| train.id != cut_id) {
            let mut row = store.get(&train.id).map_err(|error| error.to_string())?;
            row.labels.retain(|label| !label.starts_with(SUCCESSOR_LABEL));
            row.labels.push(format!("{SUCCESSOR_LABEL}{}", successor.id));
            store.update(&row).map_err(|error| error.to_string())?;
        }
        rollover_children(cas_root, repo, store, &retired, &successor, cut_tip, included)?;
        let agents = crate::store::open_agent_store(cas_root).map_err(|error| error.to_string())?;
        for agent in agents.list(None).map_err(|error| error.to_string())? {
            if agent.role != cas_types::AgentRole::Supervisor { continue; }
            if let Some(session) = agent.factory_session.as_deref()
                && crate::ops::fleet::pinned_epic(session).is_some_and(|id| retired.iter().any(|train| train.id == id))
            {
                crate::ops::fleet::restore_epic_focus(cas_root, session, &successor.id);
            }
        }
    }
    Ok(())
}

const SUCCESSOR_LABEL: &str = "release-train-successor:";
const ROLLOVER_PENDING: &str = "release-train-rollover:";

/// Called under the train lock. The durable link makes retries resume the
/// same successor; following links also handles a later cut of that train.
fn ensure_successor(repo: &Path, store: &dyn cas_store::TaskStore, train: &Task, integration: &str) -> Result<Task, String> {
    let mut previous = store.get(&train.id).map_err(|error| error.to_string())?;
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = previous.labels.iter().find_map(|label| label.strip_prefix(SUCCESSOR_LABEL)) {
        if !seen.insert(previous.id.clone()) { return Err("release train successor cycle".into()); }
        let next = store.get(id).map_err(|error| error.to_string())?;
        if !next.is_terminal() && !is_cut_train(&next) && !accepts_no_work(&next) {
            publish_train_branch(repo, next.effective_epic_branch().ok_or("successor has no lane")?, integration)?;
            return Ok(next);
        }
        previous = next;
    }
    // GH #2181: work filed while this train was frozen lazily opened a train
    // that already is the successor. Adopt the oldest such open train.
    let repo_of = |task: &Task| task.deliverables.work_target.as_ref().map(|target| target.repo_selector.clone());
    let lazy = open_trains(store, integration).into_iter().find(|other| {
        other.created_at > train.created_at && !seen.contains(&other.id) && other.id != previous.id
            && !accepts_no_work(other) && repo_of(other) == repo_of(&previous)
    });
    let next = match lazy {
        Some(next) => next,
        None => {
            let id = store.generate_id().map_err(|error| error.to_string())?;
            let mut next = Task::new(id, RELEASE_TRAIN_TITLE.into());
            next.task_type = TaskType::Epic;
            next.labels.push(RELEASE_TRAIN_LABEL.into());
            next.branch = Some(format!("epic/release-train-vnext-{}", next.id));
            next.origin_project = previous.origin_project.clone();
            next.epic_verification_owner = previous.epic_verification_owner.clone();
            next.deliverables.work_target = previous.deliverables.work_target.clone();
            store.add(&next).map_err(|error| error.to_string())?;
            next
        }
    };
    previous.labels.push(format!("{SUCCESSOR_LABEL}{}", next.id));
    previous.updated_at = chrono::Utc::now();
    store.update(&previous).map_err(|error| error.to_string())?;
    publish_train_branch(repo, next.effective_epic_branch().unwrap(), integration)?;
    Ok(next)
}

fn rollover_children(root: &Path, repo: &Path, store: &dyn cas_store::TaskStore, retired: &[Task], next: &Task, cut_tip: &str, included: &[String]) -> Result<(), String> {
    let lane = next.effective_epic_branch().ok_or("successor has no lane")?;
    let base = branch_tip(repo, lane).ok_or("successor base missing")?;
    let agents = crate::store::open_agent_store(root).map_err(|error| error.to_string())?.list(None).map_err(|error| error.to_string())?;
    let queue = crate::store::open_prompt_queue_store(root).map_err(|error| error.to_string())?;
    for mut child in store.list(None).map_err(|error| error.to_string())?.into_iter().filter(|task| !task.is_terminal() && !is_release_train(task) && !included.contains(&task.id)) {
        let parent = store.get_parent_epic(&child.id).map_err(|error| error.to_string())?;
        let old = retired.iter().find(|train| {
            parent.as_ref().is_some_and(|parent| parent.id == train.id)
                || child.deliverables.work_target.as_ref().is_some_and(|target| train.effective_epic_branch() == Some(target.target_branch.as_str()))
                || child.labels.iter().any(|label| label == &format!("{ROLLOVER_PENDING}{}", train.id))
        });
        let Some(old) = old else { continue };
        let anchor = crate::qa_batch::delivery_anchor(repo, &child);
        // Included/reachable deliveries stay for the release's ordinary
        // bookkeeping close. A red cut may still receive its own repair.
        if anchor.as_deref().is_some_and(|anchor| is_ancestor(repo, anchor, cut_tip))
            || (child.labels.iter().any(|label| label == crate::mcp::tools::core::workflow::release_freeze::REPAIR_LABEL)
                && old.effective_epic_branch().is_some_and(|lane| check_cut_merge(root, lane, Some(&child.id)).is_ok()))
        { continue; }
        let tip = anchor.or_else(|| child.deliverables.factory_branch_start.as_ref().and_then(|start| branch_tip(repo, &start.branch)));
        let was_parked = child.status == TaskStatus::AwaitingMerge;
        let requires_merge = tip.as_deref().map_or(was_parked, |tip| !is_ancestor(repo, &base, tip));
        let repair_hold = if was_parked && requires_merge {
            match super::repeat_review::admit_repair(root, &child.id) {
                Ok(_) => { child.status = TaskStatus::Open; None }
                Err(error) => Some(error.message),
            }
        } else { None };
        let mut target = child.deliverables.work_target.clone().or_else(|| next.deliverables.work_target.clone()).ok_or("child has no repository target")?;
        target.target_branch = lane.into();
        child.deliverables.work_target = Some(target.clone());
        // Keep the old start for an unchanged parked candidate's existing
        // review. Resumable work follows the new target without rewriting its
        // source branch, base SHA, anchors or any test/review receipts.
        if !was_parked || requires_merge {
            if let Some(start) = child.deliverables.factory_branch_start.as_mut() { start.work_target = target; }
        }
        let pending = format!("{ROLLOVER_PENDING}{}", old.id);
        if !child.labels.contains(&pending) { child.labels.push(pending.clone()); }
        child.updated_at = chrono::Utc::now();
        store.update(&child).map_err(|error| error.to_string())?;
        if parent.as_ref().is_some_and(|parent| parent.id != next.id) {
            store.remove_dependency_of_type(&child.id, &parent.as_ref().unwrap().id, cas_types::DependencyType::ParentChild).map_err(|error| error.to_string())?;
        }
        store.add_dependency(&cas_types::Dependency::new(child.id.clone(), next.id.clone(), cas_types::DependencyType::ParentChild)).map_err(|error| error.to_string())?;
        if let Some(assignee) = child.assignee.as_deref() {
            let worker = agents.iter().filter(|agent| agent.name == assignee || agent.id == assignee).max_by_key(|agent| agent.registered_at);
            let recipient = worker.map_or(assignee, |worker| worker.name.as_str());
            let action = if let Some(hold) = repair_hold.as_deref() { hold }
                else if requires_merge { "Merge the successor into your source branch before tests or CI, then re-park." }
                else if tip.is_some() { "Your source contains the successor base; keep its existing proof." }
                else { "Start your source from the successor before tests or CI." };
            let payload = serde_json::json!({"task_id":child.id,"target_train":next.id,"target_branch":lane,"requires_target_merge":requires_merge});
            queue.enqueue_idempotent("release-train", recipient,
                &format!("{} was cut; {} now targets {} ({lane}). {action}\n{payload}", old.id, child.id, next.id),
                worker.and_then(|worker| worker.factory_session.as_deref()), Some("Release train target moved"),
                Some(cas_store::NotificationPriority::High), &format!("train-rollover:{}:{}:{}", old.id,next.id,child.id), Some(&cas_store::QueueOrigin::Daemon))
                .map_err(|error| error.to_string())?;
        }
        child.labels.retain(|label| label != &pending);
        store.update(&child).map_err(|error| error.to_string())?;
    }
    let _ = cas_factory::notify_daemon(root);
    Ok(())
}

/// Identity for an early close refusal. A frozen train may not have cut yet;
/// reserve its successor under the same lock so the remedy names a real lane.
pub(crate) fn successor_for_target(root: &Path, target: &str) -> Result<Option<Task>, String> {
    let store = crate::store::open_task_store(root).map_err(|error| error.to_string())?;
    let Some(train) = store.list(None).map_err(|error| error.to_string())?.into_iter().find(|train| is_release_train(train) && train.effective_epic_branch().is_some_and(|lane| super::repo_context::effective_epic_ref(root, lane) == target)) else { return Ok(None) };
    if let Some(id) = train.labels.iter().find_map(|label| label.strip_prefix(SUCCESSOR_LABEL)) {
        return store.get(id).map(Some).map_err(|error| error.to_string());
    }
    let repo = crate::train_guardrails::train_repo_root(root, &train).map_err(|error| error.to_string())?;
    let integration = train.deliverables.work_target.as_ref().ok_or("train has no integration target")?.target_branch.clone();
    let _lock = lock_trains_blocking(root).map_err(|error| error.to_string())?;
    ensure_successor(&repo, store.as_ref(), &train, &integration).map(Some)
}

/// Checked at target resolution and again under the delivery-target lock.
/// Release tracking takes that same lock before persisting the cut marker.
/// A frozen train (cas-2f4d) admits only repairs of its red.
// gate[G-100 merge-admission integrity]: Release train cut or frozen (merge refused)
pub(crate) fn check_cut_merge(cas_root: &Path, target: &str, task_id: Option<&str>) -> Result<(), String> {
    use crate::mcp::tools::core::workflow::release_freeze;

    let store = crate::store::open_task_store(cas_root).map_err(|error| error.to_string())?;
    for train in store.list(None).map_err(|error| error.to_string())? {
        if !is_cut_train(&train) {
            continue;
        }
        let Some(lane) = train.effective_epic_branch() else { continue };
        if super::repo_context::effective_epic_ref(cas_root, lane) != target {
            continue;
        }
        // A cut ejected for red CI must still take its own repair. Keep both
        // control markers and let the freeze gate verify/audit the red below.
        if release_freeze::active(cas_root, &train).is_some()
            && let Some(task) = task_id.and_then(|id| store.get(id).ok())
            && !task.is_terminal()
            && task.labels.iter().any(|label| label == release_freeze::REPAIR_LABEL)
            && store.get_parent_epic(&task.id).map_err(|error| error.to_string())?
                .is_some_and(|parent| parent.id == train.id)
        {
            continue;
        }
        return Err(crate::gates::tag("G-100", format!(
            "RELEASE TRAIN CUT: {target} ({}) is frozen and accepts no further merges. No merge was attempted. Retarget unfinished work to the next train before retrying worktree_merge.",
            train.id
        )));
    }
    release_freeze::check_merge(cas_root, target, task_id)
}

/// A frozen train keeps its own work but takes no new tasks (cas-2f4d).
fn accepts_no_work(train: &Task) -> bool {
    crate::mcp::tools::core::workflow::release_freeze::freeze_of(train).is_some()
}

/// Trains are opt-in (`factory.release_train`, default off). `None` when the
/// project config cannot be read.
fn enabled(repo: &Path) -> Option<bool> {
    crate::config::Config::load(&repo.join(".cas")).ok().map(|config| config.factory().release_train)
}

/// GH #1613: with trains off, retire the trains the old default opened. Each
/// unfinished task under or targeting an uncut train leaves it for the
/// integration branch once, with a decision note, and a train left with no
/// unfinished work is cancelled as superseded. Idempotent; acts only on a
/// readable config with trains off, so a broken config never cancels a live
/// train. Returns how many tasks were retargeted.
pub(crate) fn retire_disabled_trains(cas_root: &Path, store: &dyn cas_store::TaskStore) -> usize {
    let Some((repo, integration)) = super::repo_context::integration_branch(cas_root) else {
        return 0;
    };
    if enabled(&repo) != Some(false) {
        return 0;
    }
    let open_train = [TaskStatus::Open, TaskStatus::InProgress, TaskStatus::Blocked].into_iter().any(|status| {
        store.list(Some(status)).is_ok_and(|tasks| tasks.iter().any(|t| is_release_train(t) && !is_cut_train(t)))
    });
    if !open_train {
        return 0;
    }
    let Ok(tasks) = store.list(None) else { return 0 };
    let trains: Vec<(&Task, Vec<String>)> = tasks
        .iter()
        .filter(|task| is_release_train(task) && !is_cut_train(task))
        .map(|train| {
            let children = store.get_subtasks(&train.id).unwrap_or_default();
            (train, children.into_iter().map(|child| child.id).collect())
        })
        .collect();
    let now = chrono::Utc::now();
    let mut retargeted = 0;
    for task in tasks.iter().filter(|task| !task.is_terminal() && !is_release_train(task)) {
        let parent = trains.iter().find(|(_, children)| children.contains(&task.id)).map(|(t, _)| *t);
        let lane = task.deliverables.work_target.as_ref().and_then(|target| {
            trains.iter().map(|(t, _)| *t)
                .find(|train| train.effective_epic_branch() == Some(target.target_branch.as_str()))
        });
        let Some(train) = parent.or(lane) else { continue };
        if let Some(parent) = parent
            && let Err(error) = store.remove_dependency_of_type(
                &task.id, &parent.id, cas_types::DependencyType::ParentChild,
            )
        {
            tracing::warn!(task = %task.id, train = %parent.id, %error, "release train detach deferred");
            continue;
        }
        let Ok(mut row) = store.get(&task.id) else { continue };
        if let Some(target) = row.deliverables.work_target.as_mut().filter(|_| lane.is_some()) {
            target.target_branch = integration.clone();
        }
        row.updated_at = now;
        if let Err(error) = store.update(&row) {
            tracing::warn!(task = %task.id, train = %train.id, %error, "release train retarget deferred");
            continue;
        }
        retargeted += 1;
        if let Err(error) = store.append_note(&task.id, &format!(
            "[{}] ✅ DECISION Release trains are off in this project: left {} for `{integration}`, which is now this task's target.",
            now.format("%Y-%m-%d %H:%M"), train.id,
        )) {
            tracing::warn!(task = %task.id, %error, "release train retarget note deferred");
        }
    }
    for (train, _) in trains.into_iter().filter(|(train, _)| !train.is_terminal()) {
        if store.get_subtasks(&train.id).is_ok_and(|children| children.iter().any(|c| !c.is_terminal())) {
            continue;
        }
        let mut cancelled = train.clone();
        cancelled.status = TaskStatus::Cancelled;
        cancelled.terminal_outcome =
            Some(cas_types::TaskTerminalOutcome::Cancelled { superseded_by: None });
        cancelled.closed_at = Some(now);
        cancelled.updated_at = now;
        cancelled.close_reason = Some(format!(
            "Superseded: release trains are off in this project; its work targets `{integration}`."
        ));
        if let Err(error) = store.update(&cancelled) {
            tracing::warn!(train = %train.id, %error, "release train cancel deferred");
        }
    }
    retargeted
}

/// Exclusive lock over train lookup, landing and opening (the
/// `worker_task_queue` pattern): blocks off the runtime until the holder,
/// in this or another process, releases it by dropping the file.
pub(crate) fn lock_trains_blocking(cas_root: &Path) -> std::io::Result<File> {
    let dir = cas_root.join("locks");
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("release-train.lock"))?;
    file.lock_exclusive()?;
    Ok(file)
}

async fn lock_trains(cas_root: &Path) -> std::io::Result<File> {
    let cas_root = cas_root.to_path_buf();
    tokio::task::spawn_blocking(move || lock_trains_blocking(&cas_root))
        .await.map_err(std::io::Error::other)?
}

impl CasCore {
    /// The release train a new task without an epic or explicit target joins,
    /// with a one-line receipt. `None` keeps the per-task integration target:
    /// the setting is off, the store is not in Git, or no integration branch
    /// resolves. A train that cannot be opened degrades to that target too,
    /// with the reason in the receipt.
    pub(crate) async fn release_train_for_new_task(
        &self,
        store: &dyn cas_store::TaskStore,
    ) -> Result<(Option<String>, String), McpError> {
        let Some((repo, integration)) = super::repo_context::integration_branch(&self.cas_root) else {
            return Ok((None, String::new()));
        };
        if enabled(&repo) != Some(true) {
            return Ok((None, String::new()));
        }
        // Find-or-open and cut retirement share this cross-process lock.
        let _lock = match lock_trains(&self.cas_root).await {
            Ok(lock) => lock,
            Err(error) => return Ok((None, format!(
                "\n\nRELEASE TRAIN UNAVAILABLE: train lock: {error}. This task targets `{integration}` directly."
            ))),
        };
        if let Some(train) = select_train(store, &repo, &integration)
            .map_err(|error| McpError::invalid_params(error, None))?
        {
            let lane = train.effective_epic_branch().unwrap_or(&integration);
            let _ = publish_train_branch(&repo, lane, &integration);
            return Ok((Some(train.id.clone()), format!(
                "\n\nRELEASE TRAIN: joined {} ({lane}); its delivery merges into the train, which reaches `{integration}` once per release. For a one-item hotfix train, run `scripts/release.sh --hotfix` from its lane.", train.id
            )));
        }
        let request: TaskCreateRequest = serde_json::from_value(serde_json::json!({
            "title": RELEASE_TRAIN_TITLE,
            "task_type": "epic",
            "priority": 1,
            "labels": RELEASE_TRAIN_LABEL,
            "description": format!(
                "Default target for tasks created without an epic. Children merge into this lane; the release goes to `{integration}` once."
            ),
        })).map_err(|error| McpError::internal_error(error.to_string(), None))?;
        let repo_arg = repo.to_string_lossy().to_string();
        let created = Box::pin(self.cas_task_create_with_delivery_mode(
            request, Some(&repo_arg), Some(&integration), true, None,
        )).await;
        if let Err(McpError { message, .. }) = created {
            return Ok((None, format!(
                "\n\nRELEASE TRAIN UNAVAILABLE: {message}. This task targets `{integration}` directly."
            )));
        }
        let train = select_train(store, &repo, &integration)
            .map_err(|error| McpError::invalid_params(error, None))?;
        match train {
            Some(train) => {
                let lane = train.effective_epic_branch().unwrap_or(&integration);
                let published = match publish_train_branch(&repo, lane, &integration) {
                    Ok(()) => String::new(),
                    Err(error) => format!(" Publishing `{lane}` to origin failed ({error}); push it before retargeting work there."),
                };
                Ok((Some(train.id.clone()), format!(
                    "\n\nRELEASE TRAIN: opened {} ({lane}) from `{integration}`; this task joins it.{published}", train.id
                )))
            }
            None => Ok((None, String::new())),
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;
    use cas_types::{Dependency, DependencyType};
    use rmcp::handler::server::wrapper::Parameters;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git").args(args).current_dir(repo).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
    }

    fn repo() -> TempDir {
        let temp = TempDir::new().unwrap();
        let repo = temp.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.email", "test@cas.test"]);
        git(repo, &["config", "user.name", "Cassy Test"]);
        git(repo, &["remote", "add", "origin", "https://github.com/example/train-test.git"]);
        std::fs::write(repo.join("seed.txt"), "seed\n").unwrap();
        git(repo, &["add", "seed.txt"]);
        git(repo, &["commit", "-q", "-m", "seed"]);
        temp
    }

    fn request(title: &str) -> TaskCreateRequest {
        serde_json::from_value(serde_json::json!({
            "title": title, "task_type": "task", "priority": 2, "risk": "none",
        }))
        .unwrap()
    }

    /// Two creates racing in separate cores (as separate MCP processes)
    /// must share one train.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_creates_open_a_single_train() {
        let _env = TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().unwrap();
        let temp = repo();
        let root = temp.path().to_path_buf();
        std::fs::create_dir_all(root.join(".cas")).unwrap();
        std::fs::write(root.join(".cas/config.toml"), "[factory]\nrelease_train = true\n").unwrap();
        let spawn = |title: &'static str| {
            let core = CasCore::with_daemon(root.clone(), None, None);
            tokio::spawn(async move {
                core.cas_task_create(Parameters(request(title))).await.map(|_| ()).map_err(|e| e.message)
            })
        };
        let (a, b) = (spawn("Repair alpha parser"), spawn("Tune beta renderer colours"));
        a.await.unwrap().unwrap();
        b.await.unwrap().unwrap();
        let store = CasCore::with_daemon(root.clone(), None, None).open_task_store().unwrap();
        let trains: Vec<_> = store.list(None).unwrap().into_iter().filter(is_release_train).collect();
        assert_eq!(trains.len(), 1, "{trains:?}");
        assert_eq!(store.get_subtasks(&trains[0].id).unwrap().len(), 2);
    }

    /// cas-25f1: a train opened on task create is on origin immediately.
    #[tokio::test]
    async fn opened_train_branch_is_published_to_origin_cas_25f1() {
        let _env = TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().unwrap();
        let temp = repo();
        let root = temp.path().to_path_buf();
        let bare = TempDir::new().unwrap();
        git(bare.path(), &["init", "-q", "--bare", "-b", "main"]);
        // Keep the GitHub-shaped origin the repo resolves by; git pushes it to the bare repo.
        git(&root, &["config", &format!("url.{}.pushInsteadOf", bare.path().display()), "https://github.com/example/train-test.git"]);
        git(&root, &["push", "-q", "origin", "main"]);
        std::fs::create_dir_all(root.join(".cas")).unwrap();
        std::fs::write(root.join(".cas/config.toml"), "[factory]\nrelease_train = true\nepic_base_branch = \"main\"\n").unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        let created = core.cas_task_create(Parameters(request("Repair alpha parser"))).await.unwrap();
        let store = core.open_task_store().unwrap();
        let train = store.list(None).unwrap().into_iter().find(is_release_train).unwrap_or_else(|| panic!("no train: {created:?}"));
        let lane = train.effective_epic_branch().unwrap().to_string();
        let remote = Command::new("git").args(["ls-remote", "--heads", "."]).arg(&lane).current_dir(bare.path()).output().unwrap();
        assert!(String::from_utf8_lossy(&remote.stdout).contains(&lane), "{lane} not on origin");
    }

    /// GH #1613: retirement acts only on a readable config with trains off.
    /// With trains on, or a config that cannot be read, the train and its
    /// child stay exactly as they were.
    #[tokio::test]
    async fn retire_leaves_trains_alone_unless_config_reads_off() {
        let _env = TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().unwrap();
        let temp = repo();
        let root = temp.path().to_path_buf();
        std::fs::create_dir_all(root.join(".cas")).unwrap();
        std::fs::write(root.join(".cas/config.toml"), "[factory]\nrelease_train = true\n").unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        core.cas_task_create(Parameters(request("Repair alpha parser"))).await.unwrap();
        let store = core.open_task_store().unwrap();
        let snapshot = || {
            let train = store.list(None).unwrap().into_iter().find(is_release_train).unwrap();
            let child = store.get_subtasks(&train.id).unwrap().pop().unwrap();
            (train.status, child.deliverables.work_target, child.notes)
        };
        let before = snapshot();
        for config in ["[factory]\nrelease_train = true\n", "[factory\nrelease_train = false\n"] {
            std::fs::write(root.join(".cas/config.toml"), config).unwrap();
            assert_eq!(retire_disabled_trains(&root, store.as_ref()), 0, "{config}");
            assert_eq!(snapshot(), before, "{config}");
        }
    }

    /// cas-2f4d: a frozen train keeps its own work, but a new task, even one
    /// aimed at the frozen train, joins the next train.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn new_work_joins_the_next_train_while_one_is_frozen() {
        let _env = TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().unwrap();
        let temp = repo();
        let root = temp.path().to_path_buf();
        std::fs::create_dir_all(root.join(".cas")).unwrap();
        std::fs::write(root.join(".cas/config.toml"), "[factory]\nrelease_train = true\n").unwrap();
        let core = CasCore::with_daemon(root.clone(), None, None);
        core.cas_task_create(Parameters(request("Repair alpha parser"))).await.unwrap();
        let store = core.open_task_store().unwrap();
        let frozen = store.list(None).unwrap().into_iter().find(is_release_train).unwrap();
        let lane = frozen.effective_epic_branch().unwrap().to_string();
        // The train carries work main lacks; a tip already on main is landed.
        git(&root, &["checkout", "-qB", &lane]);
        std::fs::write(root.join("work.txt"), "work\n").unwrap();
        git(&root, &["add", "work.txt"]);
        git(&root, &["commit", "-qm", "work"]);
        git(&root, &["checkout", "-q", "main"]);
        crate::mcp::tools::core::workflow::release_freeze::freeze(&root, &frozen.id, "operator", None).unwrap();
        let aimed: TaskCreateRequest = serde_json::from_value(serde_json::json!({
            "title": "Tune beta renderer colours", "task_type": "task", "priority": 2, "risk": "none",
            "epic": frozen.id,
        }))
        .unwrap();
        core.cas_task_create(Parameters(aimed)).await.map_err(|error| error.message).unwrap();
        let next: Vec<_> = store.list(None).unwrap().into_iter()
            .filter(|train| is_release_train(train) && train.id != frozen.id).collect();
        assert_eq!(next.len(), 1, "{next:?}");
        assert_eq!(store.get_subtasks(&frozen.id).unwrap().len(), 1);
        assert_eq!(store.get_subtasks(&next[0].id).unwrap().len(), 1);

        // Commander's assignee-only update is staffing too: a new assignment on
        // the frozen train's open child is refused and not persisted; a repair
        // assignment is admitted; unassigning stays allowed.
        let child = store.get_subtasks(&frozen.id).unwrap().remove(0);
        let mut stored = store.get(&child.id).unwrap();
        stored.assignee = Some("old-worker".into());
        store.update(&stored).unwrap();
        let mut repair = Task::new("cas-repair".into(), "Repair the red tip".into());
        repair.labels = vec![crate::mcp::tools::core::workflow::release_freeze::REPAIR_LABEL.into()];
        store.add(&repair).unwrap();
        store
            .add_dependency(&Dependency::new(repair.id.clone(), frozen.id.clone(), DependencyType::ParentChild))
            .unwrap();
        let assign = |id: &str, assignee: &str| {
            let update: crate::mcp::tools::TaskUpdateRequest =
                serde_json::from_value(serde_json::json!({"id": id, "assignee": assignee})).unwrap();
            core.cas_task_update_with_target(update, None, None, false, None, None)
        };
        assert!(assign(&child.id, "swift-lark-3").await.is_err());
        assert_eq!(store.get(&child.id).unwrap().assignee.as_deref(), Some("old-worker"));
        assign("cas-repair", "swift-lark-3").await.unwrap();
        assert_eq!(store.get("cas-repair").unwrap().assignee.as_deref(), Some("swift-lark-3"));
        assign(&child.id, "").await.unwrap();
        assert_eq!(store.get(&child.id).unwrap().assignee, None);
    }

    /// Both machines must use shared branch facts, not an older local train
    /// with unfinished children. The actual task-create handler owns routing.
    #[tokio::test]
    async fn default_train_uses_shared_live_branch_across_machines_cas_7ab6() {
        let _env = TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().unwrap();
        let seed = repo();
        let bare = TempDir::new().unwrap();
        git(bare.path(), &["init", "-q", "--bare", "-b", "main"]);
        git(seed.path(), &["tag", "seed"]);
        git(seed.path(), &["branch", "epic/stale"]);
        std::fs::write(seed.path().join("seed.txt"), "published release\n").unwrap();
        git(seed.path(), &["commit", "-qam", "published release"]);
        for lane in ["epic/empty-old", "epic/empty-new"] {
            git(seed.path(), &["branch", lane]);
        }
        git(seed.path(), &["checkout", "-qb", "epic/live"]);
        std::fs::write(seed.path().join("work.txt"), "unreleased work\n").unwrap();
        git(seed.path(), &["add", "work.txt"]);
        git(seed.path(), &["commit", "-qm", "unreleased work"]);
        git(seed.path(), &["branch", "epic/contained"]);
        std::fs::write(seed.path().join("work.txt"), "more unreleased work\n").unwrap();
        git(seed.path(), &["commit", "-qam", "more work"]);
        git(seed.path(), &["checkout", "-qb", "epic/other", "main"]);
        std::fs::write(seed.path().join("other.txt"), "divergent work\n").unwrap();
        git(seed.path(), &["add", "other.txt"]);
        git(seed.path(), &["commit", "-qm", "divergent work"]);
        git(seed.path(), &["config", &format!("url.{}.insteadOf", bare.path().display()),
            "https://github.com/example/train-test.git"]);
        git(seed.path(), &["push", "-q", "origin", "--all"]);
        git(seed.path(), &["push", "-q", "origin", "--tags"]);
        let train = |id: &str, lane: &str, age: i64| {
            let mut task = Task::new(id.into(), RELEASE_TRAIN_TITLE.into());
            task.task_type = TaskType::Epic;
            task.labels = vec![RELEASE_TRAIN_LABEL.into()];
            task.branch = Some(lane.into());
            task.created_at -= chrono::Duration::seconds(age);
            task.deliverables.work_target = Some(cas_types::WorkTarget {
                repo_selector: "project:train-fixture".into(), target_branch: "main".into(),
            });
            task
        };
        let stale = train("cas-a111", "epic/stale", 600);
        let live = train("cas-a222", "epic/live", 300);
        let copies = TempDir::new().unwrap();
        for machine in ["one", "two"] {
            let repo = copies.path().join(machine);
            git(bare.path(), &["clone", "-q", bare.path().to_str().unwrap(), repo.to_str().unwrap()]);
            git(&repo, &["remote", "set-url", "origin", "https://github.com/example/train-test.git"]);
            git(&repo, &["config", &format!("url.{}.insteadOf", bare.path().display()),
                "https://github.com/example/train-test.git"]);
            if machine == "one" {
                git(&repo, &["checkout", "-q", "--detach"]);
                git(&repo, &["branch", "-f", "main", "seed"]);
            }
            std::fs::create_dir(repo.join(".cas")).unwrap();
            std::fs::write(repo.join(".cas/config.toml"),
                "[project]\ncanonical_id='train-fixture'\n[factory]\nrelease_train=true\nepic_base_branch='main'\n").unwrap();
            let core = CasCore::with_daemon(repo.clone(), None, None);
            let store = core.open_task_store().unwrap();
            for row in [&stale, &live] { store.add(row).unwrap(); }
            let fresh = train("cas-a888", "epic/empty-old", 0);
            store.add(&fresh).unwrap();
            let unfinished = Task::new("cas-a000".into(), "Unfinished old work".into());
            store.create_atomic(&unfinished, &[], Some(&stale.id), None).unwrap();
            core.cas_task_create(Parameters(request("New repair"))).await.unwrap();
            let created = store.list(None).unwrap().into_iter().find(|t| t.title == "New repair").unwrap();
            assert_eq!(store.get_parent_epic(&created.id).unwrap().unwrap().id, live.id, "{machine}");

            let contained = train("cas-a666", "epic/contained", 200);
            store.add(&contained).unwrap();
            core.cas_task_create(Parameters(request("Contained sibling repair"))).await.unwrap();
            let created = store.list(None).unwrap().into_iter().find(|t| t.title == "Contained sibling repair").unwrap();
            assert_eq!(store.get_parent_epic(&created.id).unwrap().unwrap().id, live.id);

            let mut alias = train("cas-a777", "epic/live", 0);
            alias.created_at = live.created_at; // Equal-tip and timestamp tie uses the stable ID.
            store.add(&alias).unwrap();
            core.cas_task_create(Parameters(request("Equivalent lane repair"))).await.unwrap();
            let created = store.list(None).unwrap().into_iter().find(|t| t.title == "Equivalent lane repair").unwrap();
            assert_eq!(store.get_parent_epic(&created.id).unwrap().unwrap().id, alias.id);

            let other = train("cas-a333", "epic/other", 100);
            store.add(&other).unwrap();
            let before = store.list(None).unwrap().len();
            let refusal = core.cas_task_create(Parameters(request("Ambiguous repair"))).await.unwrap_err();
            assert_eq!(refusal.code, rmcp::model::ErrorCode::INVALID_PARAMS);
            for id in [&live.id, &other.id] { assert!(refusal.message.contains(id)); }
            assert_eq!(store.list(None).unwrap().len(), before);

            for mut row in [live.clone(), other, contained, alias] {
                row.labels.push(RELEASE_TRAIN_CUT_LABEL.into());
                store.update(&row).unwrap();
            }
            let mut fresh = store.get("cas-a888").unwrap();
            fresh.labels.push(RELEASE_TRAIN_CUT_LABEL.into());
            store.update(&fresh).unwrap();
            let older = train("cas-a444", "epic/empty-old", 60);
            let newer = train("cas-a555", "epic/empty-new", 0);
            // Reverse insertion order on the other machine must not affect election.
            let empty = if machine == "one" { [&older, &newer] } else { [&newer, &older] };
            for row in empty { store.add(row).unwrap(); }
            core.cas_task_create(Parameters(request("Fresh train work"))).await.unwrap();
            let created = store.list(None).unwrap().into_iter().find(|t| t.title == "Fresh train work").unwrap();
            assert_eq!(store.get_parent_epic(&created.id).unwrap().unwrap().id, newer.id);
            for id in [&stale.id, &live.id] { assert_eq!(store.get(id).unwrap().status, TaskStatus::Open); }
        }
    }

    /// GH #2181: the cut adopts a successor opened lazily during the freeze
    /// instead of opening a second one; a cut with none still opens one.
    #[test]
    fn cut_adopts_the_lazy_successor_and_opens_one_only_when_none_gh2181() {
        let mut env = TestEnvGuard::temp_home();
        env.remove("CAS_ROOT");
        env.remove("CAS_SKIP_FAST_ROWS");
        env.remove("ZERO_BASE_REF");
        env.set("CAS_BIN", "/bin/false");
        let temp = repo();
        let repo = temp.path();
        let bare = TempDir::new().unwrap();
        git(bare.path(), &["init", "-q", "--bare", "-b", "main"]);
        git(repo, &["config", &format!("url.{}.insteadOf", bare.path().display()), "https://github.com/example/train-test.git"]);
        std::fs::create_dir(repo.join("scripts")).unwrap();
        for (name, source) in [
            ("fast-rows-pre-push.sh", include_str!("../../../../../../scripts/fast-rows-pre-push.sh")),
            ("worker-run.sh", include_str!("../../../../../../scripts/worker-run.sh")),
            ("classify-web-inputs.py", "print('web false')\n"),
            ("release-gate.sh", concat!(
                "set -eu\n",
                "test \"$1\" = --fast-rows\n",
                "test \"$2\" = --base\n",
                "tip=$(git rev-parse HEAD)\n",
                "test \"$tip\" = \"$(git rev-parse \"$3\")\"\n",
                "common=$(git rev-parse --path-format=absolute --git-common-dir)\n",
                "printf 'fast-rows: PASS %s base=%s fixture\\n' \"$tip\" \"$3\" > \"$common/cas/fast-rows/$tip.pass\"\n",
            )),
        ] {
            std::fs::write(repo.join("scripts").join(name), source).unwrap();
        }
        git(repo, &["add", "scripts"]);
        git(repo, &["commit", "-qm", "fast-row fixture"]);
        git(repo, &["push", "-q", "origin", "main"]);
        git(repo, &["checkout", "-qb", "epic/cut"]);
        std::fs::write(repo.join("work.txt"), "work\n").unwrap();
        git(repo, &["add", "work.txt"]);
        git(repo, &["commit", "-qm", "work"]);
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["branch", "epic/lazy"]);
        git(repo, &["push", "-q", "origin", "epic/cut", "epic/lazy"]);
        let read_git = |checkout: &Path, args: &[&str]| {
            let output = Command::new("git").args(args).current_dir(checkout).output().unwrap();
            assert!(output.status.success(), "git {args:?}: {output:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };
        let tip = |branch: &str| read_git(repo, &["rev-parse", branch]);
        let cas_root = crate::store::init_cas_dir(repo).unwrap();
        let store = crate::store::open_task_store(&cas_root).unwrap();
        let train = |id: &str, lane: &str, age: i64| {
            let mut task = Task::new(id.into(), RELEASE_TRAIN_TITLE.into());
            task.task_type = TaskType::Epic;
            task.labels = vec![RELEASE_TRAIN_LABEL.into()];
            task.branch = Some(lane.into());
            task.created_at -= chrono::Duration::seconds(age);
            task.deliverables.work_target = Some(cas_types::WorkTarget {
                repo_selector: "project:train-fixture".into(), target_branch: "main".into(),
            });
            task
        };
        store.add(&train("cas-b111", "epic/cut", 600)).unwrap();
        store.add(&train("cas-b222", "epic/lazy", 60)).unwrap();
        let trains = || store.list(None).unwrap().into_iter().filter(is_release_train).map(|t| t.id).collect::<Vec<_>>();
        let successor = |id: &str| store.get(id).unwrap().labels.iter()
            .find_map(|label| label.strip_prefix(SUCCESSOR_LABEL).map(str::to_string));

        mark_cut_trains(&cas_root, repo, store.as_ref(), "cas-b111", &tip("epic/cut"), "main", &[]).unwrap();
        assert_eq!(successor("cas-b111").as_deref(), Some("cas-b222"));
        assert_eq!(trains().len(), 2);

        git(repo, &["checkout", "-q", "epic/cut"]);
        git(repo, &["config", "extensions.worktreeConfig", "true"]);
        git(repo, &["config", "--worktree", "core.hooksPath", "fixture-hooks"]);
        let hook = repo.join("fixture-hooks/pre-push");
        std::fs::create_dir(hook.parent().unwrap()).unwrap();
        std::fs::write(&hook, "#!/bin/sh\nexec bash scripts/fast-rows-pre-push.sh \"$@\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(repo.join("seed.txt"), "staged\n").unwrap();
        git(repo, &["add", "seed.txt"]);
        std::fs::write(repo.join("seed.txt"), "unstaged\n").unwrap();
        let before = read_git(repo, &["status", "--porcelain"]);
        let worktrees = read_git(repo, &["worktree", "list", "--porcelain"]);

        mark_cut_trains(&cas_root, repo, store.as_ref(), "cas-b222", &tip("main"), "main", &[]).unwrap();
        let opened = successor("cas-b222").unwrap();
        let next = store.get(&opened).unwrap();
        let lane = next.effective_epic_branch().unwrap();
        let base = tip("main");
        let receipt = repo.join(format!(".git/cas/fast-rows/{base}.pass"));
        assert_eq!(read_git(bare.path(), &["rev-parse", lane]), base);
        assert!(receipt.is_file());
        assert_eq!(read_git(repo, &["symbolic-ref", "--short", "HEAD"]), "epic/cut");
        assert_eq!(read_git(repo, &["status", "--porcelain"]), before);
        assert_eq!(read_git(repo, &["show", ":seed.txt"]), "staged");
        assert_eq!(std::fs::read_to_string(repo.join("seed.txt")).unwrap(), "unstaged\n");
        assert_eq!(read_git(repo, &["worktree", "list", "--porcelain"]), worktrees);
        assert_eq!(trains().len(), 3);

        let existing = tip("main^");
        git(bare.path(), &["update-ref", &format!("refs/heads/{lane}"), &existing]);
        git(repo, &["update-ref", "-d", &format!("refs/remotes/origin/{lane}")]);
        mark_cut_trains(&cas_root, repo, store.as_ref(), "cas-b222", &base, "main", &[]).unwrap();
        assert_eq!(successor("cas-b222").as_deref(), Some(opened.as_str()));
        assert_eq!(read_git(bare.path(), &["rev-parse", lane]), existing);
        assert_eq!(trains().len(), 3);

        git(repo, &["branch", "epic/unproved", "epic/cut"]);
        let refused = publish_train_branch(repo, "epic/unproved", "main").unwrap_err();
        assert!(refused.contains("[C-07]") && refused.contains("[H-38]"), "{refused}");
        assert!(!ref_exists(bare.path(), "refs/heads/epic/unproved"));
    }
}
