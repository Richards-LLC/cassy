//! cas-ddae (GH #1500): the factory progress watchdog. One rule: a session
//! with open work and no task, lane or message change for 45 minutes wakes
//! its supervisor once with what is waiting on whom; still nothing after
//! another 45, it pages the operator once. Heartbeats and liveness never
//! count, nor do the daemon's own rows (relays, fired reminders).

use std::collections::HashMap;
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
fn observe(cas_dir: &Path, session: &str) -> Option<(String, Vec<String>, String)> {
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
    let lanes: Vec<String> = agents.iter().filter(|agent| agent.role == AgentRole::Worker)
        .flat_map(|agent| [format!("refs/heads/factory/{}", agent.name), format!("refs/heads/factory/{}-*", agent.name)])
        .collect();
    if let Some(repo) = cas_dir.parent().filter(|repo| !lanes.is_empty() && repo.join(".git").exists())
        && let Ok(output) = std::process::Command::new("git").arg("-C").arg(repo)
            .args(["for-each-ref", "--format=%(refname) %(objectname)"]).args(&lanes).output()
    {
        fingerprint.push(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Some((fingerprint.join("\n"), waiting, supervisor))
}

/// One watchdog sample for `session` at `now`.
pub(crate) fn sweep(cas_dir: &Path, session: &str, now: DateTime<Utc>) {
    let Some((fingerprint, waiting, supervisor)) = observe(cas_dir, session) else { return };
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

        // Replay the peer release wait with zero live workers, then prove
        // that an unmerged delivery and a silent local worker still page.
        for (case, target, status, page) in [
            ("contained", "epic/train", TaskStatus::AwaitingMerge, false),
            ("origin-contained", "epic/peer-train", TaskStatus::AwaitingMerge, false),
            ("held", "main", TaskStatus::AwaitingMerge, false),
            ("peer-handoff", "main", TaskStatus::AwaitingMerge, false),
            ("unmerged", "main", TaskStatus::AwaitingMerge, true),
            ("missing-target", "epic/missing", TaskStatus::AwaitingMerge, true),
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
            if case == "held" {
                task.notes = "[2026-11-06 18:00] ✅ DECISION hold the merge for the peer release train".into();
            }
            tasks.add(&task).unwrap();
            if case == "peer-handoff" {
                cas_store::insert_epic_split(&cas_dir, &EpicSplit {
                    id: "split-c1fb".into(), epic_id: "cas-peer-epic".into(),
                    local_agent_id: supervisor.id.clone(), peer_agent_id: "peer-supervisor".into(),
                    peer_name: "peer".into(), peer_machine: "other-host".into(),
                    direction: SplitDirection::Outgoing, mine: vec![], theirs: vec![task.id.clone()],
                    state: SplitState::Accepted, reason: None, message_id: None,
                    expires_at: now + chrono::Duration::days(1), created_at: now,
                    decided_at: Some(now), decided_by: Some(supervisor.id.clone()),
                    cleanup_pending: false, expiry_notice_pending: false,
                }).unwrap();
            }
            for minutes in [0, 45, 90] {
                super::sweep(&cas_dir, &session, now + chrono::Duration::minutes(minutes));
            }
            let rows = queue.peek_all(100).unwrap();
            let pages = rows.iter().filter(|row| row.target == "operator" && row.prompt.contains(&task.id)).count();
            assert_eq!(pages, usize::from(page), "{case}: wrong operator escalation");
            let wakes = rows.iter().filter(|row| row.target == "supervisor" && row.prompt.contains(&task.id)).count();
            assert_eq!(wakes, usize::from(page), "{case}: wrong local stall wake");
        }
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
