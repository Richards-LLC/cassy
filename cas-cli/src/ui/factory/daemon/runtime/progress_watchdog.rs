//! cas-ddae (GH #1500): the factory progress watchdog. One rule: a session
//! with locally actionable work and no task, lane or message change for 45 minutes wakes
//! its supervisor once with what is waiting on whom; still nothing after
//! another 45, it pages the operator once. Heartbeats and liveness never
//! count, nor do the daemon's own rows (relays, fired reminders).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Utc};

const QUIET_SECS: i64 = 45 * 60;

/// Per-session episode: the last fingerprint, when it was first seen, and
/// whether this episode already woke the supervisor or paged the operator.
static EPISODES: Mutex<Option<HashMap<String, (String, DateTime<Utc>, bool, bool)>>> = Mutex::new(None);

/// The session's open work, and a fingerprint of every task, lane and
/// message change. `None` when a store is unreadable: uncertainty is
/// neither progress nor a stall.
fn observe(cas_dir: &Path, session: &str, now: DateTime<Utc>) -> Option<(String, Vec<String>, String)> {
    use cas_types::{AgentRole, TaskStatus};
    let agents: Vec<_> = crate::store::open_agent_store(cas_dir).ok()?.list(None).ok()?
        .into_iter().filter(|agent| agent.factory_session.as_deref() == Some(session)).collect();
    let supervisor = agents.iter().find(|agent| agent.role == AgentRole::Supervisor)
        .map_or_else(|| "the supervisor".to_string(), |agent| agent.name.clone());
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
    // cas-c1fb: historical local assignees do not make a peer's release
    // train actionable here. Only structured ownership and Git proof count.
    let peer_tasks: HashSet<_> = crate::cloud::epic_split::current_epic_splits(cas_dir).ok()?
        .into_iter().filter(|split| split.is_binding(now)).flat_map(|split| split.theirs).collect();
    let peer_branches: HashSet<_> = peer_tasks.iter().filter_map(|id| store.get(id).ok())
        .filter(|task| task.task_type == cas_types::TaskType::Epic)
        .filter_map(|task| task.effective_epic_branch().map(str::to_string)).collect();
    tasks.retain(|task| {
        // cas-e239: a blocked operator check is not locally actionable.
        // Inspect only the current blocker, never historical operator waits.
        if task.status == TaskStatus::Blocked {
            use crate::mcp::tools::core::task::block_hold;
            let reason = block_hold::latest_blocker_reason(&task.notes)
                .or_else(|| block_hold::read(store.as_ref(), &task.id).ok().flatten().map(|hold| hold.reason));
            if reason.is_some_and(|reason| {
                let words: Vec<_> = reason.split(|c: char| !c.is_alphanumeric())
                    .filter(|word| !word.is_empty()).map(str::to_ascii_lowercase).collect();
                // An explicit owner wins over incidental mentions elsewhere.
                words.windows(2).find(|pair| pair[0] == "owner")
                    .map_or_else(|| words.iter().any(|word| word == "operator"), |pair| pair[1] == "operator")
            }) {
                return false;
            }
        }
        // An old delivery anchor cannot hide a worker's new local work.
        if task.status != TaskStatus::AwaitingMerge {
            return true;
        }
        // cas-e239: a delivery the supervisor deliberately holds for a later
        // train waits on that decision, not on anyone here.
        if is_held(&task.notes) {
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
        !peer_branches.contains(target) && !cas_dir.parent()
            .is_some_and(|repo| delivery_is_contained(repo, task, target))
    });
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    let mut fingerprint: Vec<String> = tasks.iter()
        .map(|task| format!("{} {} {}", task.id, task.status, task.updated_at.to_rfc3339())).collect();
    let waiting = tasks.iter().map(|task| {
        let title: String = task.title.chars().take(70).collect();
        format!("- {} {} ({}): {title}", task.id, task.status, task.assignee.as_deref().unwrap_or_default())
    }).collect();
    let latest = crate::store::open_prompt_queue_store(cas_dir).ok()?.latest_agent_message_id(&names, session).ok()?;
    fingerprint.push(format!("message {}", latest.unwrap_or(0)));
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
    Some((fingerprint.join("\n"), waiting, supervisor))
}

/// Whether the newest decision in `notes` is a `DECISION HOLD`; any later
/// decision lifts it. Entries start at a `[YYYY-MM-DD HH:MM] ` line.
fn is_held(notes: &str) -> bool {
    let mut held = false;
    for entry in notes.split("\n[") {
        if let Some((_, rest)) = entry.split_once("DECISION") {
            held = rest.trim_start().starts_with("HOLD");
        }
    }
    held
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
pub(crate) fn sweep(cas_dir: &Path, session: &str, now: DateTime<Utc>) {
    let Some((fingerprint, waiting, supervisor)) = observe(cas_dir, session, now) else { return };
    let mut episodes = EPISODES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let episodes = episodes.get_or_insert_with(HashMap::new);
    // A project hold is the operator's own stop, not a stall.
    let open = !waiting.is_empty() && !cas_store::project_hold::is_held(cas_dir);
    let episode = episodes.entry(session.to_string()).or_insert_with(|| (fingerprint.clone(), now, false, false));
    if !open || episode.0 != fingerprint {
        *episode = (fingerprint, now, false, false);
        return;
    }
    let quiet = (now - episode.1).num_seconds();
    let (minutes, start) = (quiet / 60, episode.1.to_rfc3339());
    let waiting = waiting.join("\n");
    if quiet >= QUIET_SECS && !episode.2 {
        let detail = format!(
            "Nothing has moved in this factory for {minutes} min with work open: no task change, lane commit, merge or message from any agent here. Heartbeats do not count.\nWaiting:\n{waiting}\nFind who owes the next step and chase it, take it over, or tell the operator. If nothing moves in another {} min, the operator is paged.",
            QUIET_SECS / 60
        );
        let key = format!("progress-stall:{start}");
        episode.2 = matches!(
            super::lifecycle::enqueue_worker_attention_relay_detail_with_key(cas_dir, "progress_stalled", "supervisor",
                None, Some(quiet as u64), |_| detail.clone(), &start, Some(&key)),
            super::lifecycle::WorkerAttentionRelayOutcome::Persisted { .. }
        );
    }
    if quiet >= 2 * QUIET_SECS && !episode.3 {
        let message = format!(
            "Nothing has moved in factory session {session} for {minutes} min, though work is open and heartbeats look healthy. Supervisor {supervisor} was woken at {} min and nothing has moved since.\nWaiting:\n{waiting}\nCheck the supervisor's pane, chase the peer, or take the work over.",
            QUIET_SECS / 60
        );
        // The dedupe key names the episode, so a retry cannot page twice.
        episode.3 = super::lifecycle::record_operator_page(cas_dir, "progress-watchdog",
            &format!("Factory {session}: nothing moved for {minutes} min"), message,
            &format!("progress-stall-page:{session}:{start}"));
    }
}

#[cfg(test)]
mod tests {
    use cas_types::{Agent, AgentRole, AgentStatus, Task, TaskStatus};

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
        epic.notes = "[2026-11-06 18:00] 🚫 BLOCKER owner: operator; operator-only desktop verification remains.".into();
        tasks.add(&epic).unwrap();
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
        for minutes in [181, 226, 271, 316] {
            super::sweep(&cas_dir, SESSION, now + chrono::Duration::minutes(minutes));
        }
        let rows = queue.peek_all(50).unwrap();
        assert_eq!(rows.iter().filter(|row| row.target == "supervisor").count(), 1,
            "a genuine local stall must wake the supervisor once: {rows:?}");
        assert_eq!(rows.iter().filter(|row| row.target == "operator").count(), 1,
            "a genuine local stall must still page once: {rows:?}");
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
            assert_eq!(wakes, usize::from(page), "{case}: wrong local stall wake");
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
        assert_eq!(rows.iter().filter(|row| row.prompt.contains("progress_stalled")).count(), 1, "{rows:?}");
        assert_eq!(rows.iter().filter(|row| row.target == "operator").count(), 1, "{rows:?}");
    }

    /// GH #1500 / #1302: an overnight with a live, heartbeating but silent
    /// peer and an idle supervisor whose only wake is event-based. The
    /// supervisor is woken once at 45 min, the operator is paged once at
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
        let rows = |target: &str| queue.peek_all(50).unwrap().into_iter().filter(|r| r.target == target).collect::<Vec<_>>();
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
        // pin: the wake names the waiting task and the quiet span
        assert!(woke[0].prompt.contains("cas-gate") && woke[0].prompt.contains("45 min"), "{}", woke[0].prompt);
        assert!(crate::prompt_revalidation::parse_blocker_envelope(&woke[0].prompt).is_some());
        sweep(60);
        assert!(rows("operator").is_empty(), "the wake itself is not progress");
        sweep(90);
        sweep(7 * 60);
        let paged = rows("operator");
        assert_eq!(paged.len(), 1, "one page per episode: {paged:?}");
        // pin: the page names the waiting task and the woken supervisor
        assert!(paged[0].prompt.contains("cas-gate") && paged[0].prompt.contains("supervisor"), "{}", paged[0].prompt);
        assert_eq!(rows("supervisor").len(), 1, "one wake per episode");

        // The peer finally answers, which is not this session's progress;
        // the supervisor then acts, which starts a new episode.
        queue.enqueue_with_session("lively-cheetah-57", "supervisor", "approved", "peer-session").unwrap();
        queue.enqueue_with_session("supervisor", "busy-otter", "merging now", SESSION).unwrap();
        sweep(7 * 60 + 1);
        sweep(7 * 60 + 45);
        assert_eq!(rows("supervisor").len(), 2, "movement starts a new episode");
    }
}
