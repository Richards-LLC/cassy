//! One integration per release: fixes merge to the train, the train goes
//! through the gate once.
//!
//! A task created with no epic and no explicit target joins the repository's
//! open release-train epic (cut from the integration branch) instead of
//! targeting that branch directly, so its delivery merges locally into the
//! train lane. A train whose content has landed on the integration branch is
//! closed here, and the next one opens lazily on the next create.

use std::fs::File;
use std::path::Path;

use fs2::FileExt;

use rmcp::ErrorData as McpError;

use crate::mcp::server::CasCore;
use crate::mcp::tools::core::task::lifecycle::close_ops::{
    BranchContentDirection, branch_content_direction,
};
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

fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .status()
        .is_ok_and(|status| status.success())
}

/// A train has landed when it carried at least one delivered child, none is
/// still in flight, and every copy of its lane is on the integration branch
/// (by ancestry, or byte-identical content after a squash).
fn landed(store: &dyn cas_store::TaskStore, repo: &Path, train: &Task, integration: &str) -> bool {
    let children = store.get_subtasks(&train.id).unwrap_or_default();
    if children.iter().any(|child| !child.is_terminal())
        || !children.iter().any(|child| child.status == TaskStatus::Closed)
    {
        return false;
    }
    train.effective_epic_branch().is_some_and(|lane| lane_landed(repo, lane, integration))
}

fn lane_landed(repo: &Path, lane: &str, integration: &str) -> bool {
    let lane_refs: Vec<String> = [lane.to_string(), format!("origin/{lane}")]
        .into_iter()
        .filter(|candidate| ref_exists(repo, candidate))
        .collect();
    let targets: Vec<String> = [integration.to_string(), format!("origin/{integration}")]
        .into_iter()
        .filter(|candidate| ref_exists(repo, candidate))
        .collect();
    !lane_refs.is_empty()
        && lane_refs.iter().all(|lane_ref| {
            targets.iter().any(|target| is_ancestor(repo, lane_ref, target))
                || matches!(
                    branch_content_direction(repo, lane_ref, integration),
                    BranchContentDirection::ContentPresent { .. }
                )
        })
}

/// Checked at target resolution and again under the delivery-target lock.
/// Release tracking takes that same lock before persisting the cut marker.
/// A frozen train (cas-2f4d) admits only repairs of its red.
pub(crate) fn check_cut_merge(cas_root: &Path, target: &str, task_id: Option<&str>) -> Result<(), String> {
    let store = crate::store::open_task_store(cas_root).map_err(|error| error.to_string())?;
    for train in store.list(None).map_err(|error| error.to_string())? {
        if !is_cut_train(&train) {
            continue;
        }
        let Some(lane) = train.effective_epic_branch() else { continue };
        if super::repo_context::effective_epic_ref(cas_root, lane) != target {
            continue;
        }
        return Err(format!(
            "RELEASE TRAIN CUT: {target} ({}) is frozen and accepts no further merges. No merge was attempted. Retarget unfinished work to the next train before retrying worktree_merge.",
            train.id
        ));
    }
    crate::mcp::tools::core::workflow::release_freeze::check_merge(cas_root, target, task_id)
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
async fn lock_trains(cas_root: &Path) -> std::io::Result<File> {
    let dir = cas_root.join("locks");
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("release-train.lock"))?;
    tokio::task::spawn_blocking(move || file.lock_exclusive().map(|()| file))
        .await
        .map_err(std::io::Error::other)?
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
    ) -> (Option<String>, String) {
        let Some((repo, integration)) =
            super::repo_context::integration_branch(&self.cas_root)
        else {
            return (None, String::new());
        };
        if enabled(&repo) != Some(true) {
            return (None, String::new());
        }
        // Find-or-open is one critical section across MCP requests and
        // processes, so concurrent creates share a single train.
        let _lock = match lock_trains(&self.cas_root).await {
            Ok(lock) => lock,
            Err(error) => {
                return (
                    None,
                    format!(
                        "\n\nRELEASE TRAIN UNAVAILABLE: train lock: {error}. This task targets `{integration}` directly."
                    ),
                );
            }
        };
        let mut receipt = String::new();
        for train in active_trains(store, &integration) {
            if !landed(store, &repo, &train, &integration) {
                if is_cut_train(&train) || accepts_no_work(&train) {
                    continue;
                }
                return (
                    Some(train.id.clone()),
                    format!(
                        "{receipt}\n\nRELEASE TRAIN: joined {} ({}); its delivery merges into the train, which reaches `{integration}` once per release. For a one-item hotfix train, run `scripts/release.sh --hotfix` from its lane.",
                        train.id,
                        train.effective_epic_branch().unwrap_or(integration.as_str()),
                    ),
                );
            }
            let mut closed = train.clone();
            closed.status = TaskStatus::Closed;
            closed.terminal_outcome = Some(cas_types::TaskTerminalOutcome::Delivered);
            closed.closed_at = Some(chrono::Utc::now());
            closed.updated_at = chrono::Utc::now();
            closed.close_reason = Some(format!(
                "Release landed: train lane is on `{integration}`; the next train opens on the next create."
            ));
            if store.update(&closed).is_ok() {
                receipt.push_str(&format!(
                    "\n\nRELEASE TRAIN: {} landed on `{integration}` and is closed.",
                    train.id
                ));
            }
        }
        let request: TaskCreateRequest = match serde_json::from_value(serde_json::json!({
            "title": RELEASE_TRAIN_TITLE,
            "task_type": "epic",
            "priority": 1,
            "labels": RELEASE_TRAIN_LABEL,
            "description": format!(
                "Default target for tasks created without an epic. Children merge into this lane; the release goes to `{integration}` once."
            ),
        })) {
            Ok(request) => request,
            Err(error) => return (None, format!("{receipt}\n\nRELEASE TRAIN UNAVAILABLE: {error}")),
        };
        let repo_arg = repo.to_string_lossy().to_string();
        let created = Box::pin(self.cas_task_create_with_delivery_mode(
            request,
            Some(&repo_arg),
            Some(&integration),
            true,
            None,
        ))
        .await;
        if let Err(McpError { message, .. }) = created {
            return (
                None,
                format!(
                    "{receipt}\n\nRELEASE TRAIN UNAVAILABLE: {message}. This task targets `{integration}` directly."
                ),
            );
        }
        match open_trains(store, &integration).into_iter().find(|train| !accepts_no_work(train)) {
            Some(train) => (
                Some(train.id.clone()),
                format!(
                    "{receipt}\n\nRELEASE TRAIN: opened {} ({}) from `{integration}`; this task joins it.",
                    train.id,
                    train.effective_epic_branch().unwrap_or(integration.as_str()),
                ),
            ),
            None => (None, receipt),
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

    /// A train is not landed while a child is in flight, before any child
    /// delivered, or while its lane holds work the integration branch lacks.
    #[test]
    fn landed_requires_a_delivered_idle_train_whose_lane_is_on_trunk() {
        let temp = repo();
        let repo = temp.path();
        git(repo, &["branch", "epic/train"]);
        let core = CasCore::with_daemon(repo.to_path_buf(), None, None);
        let store = core.open_task_store().unwrap();
        let mut train = Task::new("cas-train".into(), RELEASE_TRAIN_TITLE.into());
        train.task_type = TaskType::Epic;
        train.labels = vec![RELEASE_TRAIN_LABEL.into()];
        train.branch = Some("epic/train".into());
        store.add(&train).unwrap();
        let child = |id: &str, status: TaskStatus| {
            let mut task = Task::new(id.into(), id.into());
            task.status = status;
            store.add(&task).unwrap();
            store
                .add_dependency(&Dependency::new(id.into(), train.id.clone(), DependencyType::ParentChild))
                .unwrap();
        };
        child("cas-cancelled", TaskStatus::Cancelled);
        assert!(!landed(store.as_ref(), repo, &train, "main"), "no delivered child");
        child("cas-open", TaskStatus::Open);
        child("cas-done", TaskStatus::Closed);
        assert!(!landed(store.as_ref(), repo, &train, "main"), "child in flight");
        let mut open = store.get("cas-open").unwrap();
        open.status = TaskStatus::Closed;
        store.update(&open).unwrap();
        git(repo, &["checkout", "-q", "epic/train"]);
        std::fs::write(repo.join("fix.txt"), "fix\n").unwrap();
        git(repo, &["add", "fix.txt"]);
        git(repo, &["commit", "-q", "-m", "fix"]);
        git(repo, &["checkout", "-q", "main"]);
        assert!(!landed(store.as_ref(), repo, &train, "main"), "lane ahead of main");
        git(repo, &["merge", "-q", "--no-ff", "epic/train"]);
        assert!(landed(store.as_ref(), repo, &train, "main"));
    }
}
