//! Say so when merged deliveries wait too long for a batch QA round (cas-9e4e).
//!
//! Batch QA is hourly at the epic tip, but nothing noticed when a round did not
//! happen: one was held for over two hours while 24 merged tasks waited. Every ten
//! minutes the daemon looks for merged deliveries with no batch receipt and tells
//! every supervisor once an hour, on this machine and through the peer mailbox.
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeZone, Utc};

use crate::cloud::peers::{Peer, PeerDiscovery};
use crate::store::{open_agent_store, open_task_store};
use crate::supervisor_notices::{Recipient, enqueue, is_live};
use cas_types::{AgentRole, Task, TaskStatus};

const CHECK_INTERVAL: Duration = Duration::from_secs(10 * 60);
const OVERDUE_MINUTES: i64 = 60;
/// A running round suppresses the notice, so it must be recognizable. A round is
/// a task with this title prefix, as the cas-supervisor Batch QA reference says.
const ROUND_TITLE_PREFIX: &str = "batch qa round";
const NOTICE_KIND: &str = "qa_batch_overdue";

struct Overdue {
    epic: String,
    waiting: usize,
    oldest: DateTime<Utc>,
}

impl Overdue {
    /// One notice per hour of waiting, counted from the oldest merge.
    fn occurrence(&self, now: DateTime<Utc>) -> String {
        let hours = (now - self.oldest).num_minutes() / OVERDUE_MINUTES;
        format!("{}:{}:{hours}", self.epic, self.oldest.timestamp())
    }

    fn detail(&self, now: DateTime<Utc>) -> String {
        let tasks = if self.waiting == 1 { "task" } else { "tasks" };
        format!(
            "QA batch overdue on {}: {} merged {tasks} waiting, oldest {}m. Start a round at the epic tip now; lanes still in review go in the next round.",
            self.epic,
            self.waiting,
            (now - self.oldest).num_minutes(),
        )
    }
}

/// When `anchor` landed on `target`: the first merge after it, or its own commit
/// time for a fast-forward. `None` while it has not landed.
fn merged_at(repo: &Path, anchor: &str, target: &str) -> Option<DateTime<Utc>> {
    let landed = [target.to_string(), format!("origin/{target}")]
        .into_iter()
        .find(|branch| crate::qa_batch::ancestor(repo, anchor, branch) == Some(true))?;
    let seconds = |args: &[&str]| {
        let output = Command::new("git").args(args).current_dir(repo).output().ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.lines().next()?.trim().parse::<i64>().ok()
    };
    let range = format!("{anchor}..{landed}");
    let at = seconds(&["log", "--ancestry-path", "--merges", "--reverse", "--format=%ct", &range])
        .or_else(|| seconds(&["log", "-1", "--format=%ct", anchor]))?;
    Utc.timestamp_opt(at, 0).single()
}

fn round_running(children: &[Task], now: DateTime<Utc>) -> bool {
    children.iter().any(|task| {
        task.status == TaskStatus::InProgress
            && task.title.to_ascii_lowercase().starts_with(ROUND_TITLE_PREFIX)
            && (now - task.created_at).num_minutes() < OVERDUE_MINUTES
    })
}

/// The epics this machine should raise. An owned epic is raised only where its
/// owner is registered, so two machines do not each tell the other.
fn overdue_epics(root: &Path, now: DateTime<Utc>) -> Result<Vec<Overdue>, crate::error::CasError> {
    let tasks = open_task_store(root)?;
    let agents = open_agent_store(root)?.list(None)?;
    let project = crate::cloud::resolve_canonical_id(root);
    let mut waiting: BTreeMap<String, Vec<DateTime<Utc>>> = BTreeMap::new();
    for id in cas_store::pending_qa_batch_deferrals(root)? {
        let Ok(task) = tasks.get(&id) else { continue };
        let Some(epic) = tasks.get_parent_epic(&id)? else { continue };
        let target = task
            .deliverables
            .work_target
            .as_ref()
            .map(|target| target.target_branch.clone())
            .or_else(|| epic.branch.clone());
        let repo = crate::qa_pass::github_gate::task_repo(root, &task);
        let Some(anchor) = crate::qa_batch::delivery_anchor(&repo, &task) else { continue };
        let Some(merged) = target.and_then(|target| merged_at(&repo, &anchor, &target)) else {
            continue;
        };
        let receipted = project.as_ref().is_some_and(|project| {
            cas_store::qa_batch_receipts_for_task(root, project, &id).is_ok_and(|receipts| {
                receipts.iter().any(|receipt| receipt.task_heads.get(&id) == Some(&anchor))
            })
        });
        if !receipted {
            waiting.entry(epic.id).or_default().push(merged);
        }
    }
    let mut overdue = Vec::new();
    for (epic_id, merged) in waiting {
        let Some(&oldest) = merged.iter().min() else { continue };
        if (now - oldest).num_minutes() < OVERDUE_MINUTES
            || round_running(&tasks.get_subtasks(&epic_id)?, now)
        {
            continue;
        }
        let owner = tasks.get(&epic_id)?.epic_verification_owner;
        let owned_here = owner.as_deref().is_none_or(|owner| {
            agents.iter().any(|agent| {
                agent.role == AgentRole::Supervisor && (agent.id == owner || agent.name == owner)
            })
        });
        if owned_here {
            overdue.push(Overdue { epic: epic_id, waiting: merged.len(), oldest });
        }
    }
    Ok(overdue)
}

#[derive(Default)]
struct Alarm {
    /// The local queue dedupes durably. A peer send has no such key, so a daemon
    /// restart may repeat one peer notice within the hour.
    peers_told: HashSet<String>,
}

impl Alarm {
    fn check(
        &mut self,
        root: &Path,
        now: DateTime<Utc>,
        peers: &dyn Fn(Option<&str>) -> Vec<Peer>,
        send: &dyn Fn(&str, &Peer, &str) -> Result<(), String>,
    ) -> Result<(), crate::error::CasError> {
        let overdue = overdue_epics(root, now)?;
        if overdue.is_empty() {
            return Ok(());
        }
        let supervisors: Vec<_> =
            open_agent_store(root)?.list(None)?.into_iter().filter(is_live).collect();
        let sender = supervisors.first().map(|agent| agent.id.as_str());
        let remote = peers(sender);
        for epic in overdue {
            let (occurrence, detail) = (epic.occurrence(now), epic.detail(now));
            for supervisor in &supervisors {
                enqueue(root, &Recipient::agent(supervisor), NOTICE_KIND, &detail, &occurrence)?;
            }
            let Some(sender) = sender else { continue };
            for peer in remote.iter().filter(|peer| peer.live) {
                let key = format!("{occurrence}:{}", peer.agent_id);
                if self.peers_told.contains(&key) {
                    continue;
                }
                match send(sender, peer, &detail) {
                    Ok(()) => {
                        self.peers_told.insert(key);
                    }
                    Err(error) => tracing::warn!(peer = %peer.name, %error, "could not send the QA overdue notice"),
                }
            }
        }
        Ok(())
    }
}

/// Called every loop pass. All the work runs on its own thread, off the loop.
pub(crate) fn tick(root: &Path) {
    #[derive(Default)]
    struct State {
        last: Option<Instant>,
        alarm: Alarm,
    }
    static STATE: Mutex<Option<State>> = Mutex::new(None);
    let Ok(mut state) = STATE.try_lock() else { return };
    let state = state.get_or_insert_with(State::default);
    let now = Instant::now();
    if now.saturating_duration_since(*state.last.get_or_insert(now)) < CHECK_INTERVAL {
        return;
    }
    state.last = Some(now);
    let root = root.to_path_buf();
    let spawned = std::thread::Builder::new().name("qa-batch-overdue".into()).spawn(move || {
        let Ok(mut state) = STATE.lock() else { return };
        let Some(state) = state.as_mut() else { return };
        let peers = |sender: Option<&str>| match crate::cloud::peers::discover_peers(&root, sender) {
            Ok(PeerDiscovery::Found { peers, .. }) => peers,
            _ => Vec::new(),
        };
        let send = |sender: &str, peer: &Peer, body: &str| {
            crate::cloud::peer_mailbox::send_to_peer(&root, sender, peer, body, Some("QA batch overdue"), None)
                .map(|_| ())
                .map_err(|error| error.to_string())
        };
        if let Err(error) = state.alarm.check(&root, Utc::now(), &peers, &send) {
            tracing::warn!(%error, "QA overdue check failed");
        }
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start the QA overdue check");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_types::{Agent, Dependency, DependencyType, TaskType, WorkTarget};
    use std::cell::RefCell;

    fn git(repo: &Path, date: &str, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(["-c", "user.name=fixture", "-c", "user.email=f@example.test"])
            .args(args)
            .current_dir(repo)
            .env("GIT_COMMITTER_DATE", date)
            .env("GIT_AUTHOR_DATE", date)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn peer() -> Peer {
        Peer {
            agent_id: "peer-id".into(),
            name: "prowl-supervisor".into(),
            machine: "prowl".into(),
            machine_id: None,
            session: Some("peer-session".into()),
            canonical_id: None,
            focus: None,
            focus_title: None,
            last_heartbeat: Utc::now(),
            heartbeat_age_secs: 0,
            live: true,
            user_id: None,
            owner: None,
            branch: None,
            own: None,
            harness_auth_failed: None,
        }
    }

    /// cas-9e4e: a task merged over an hour ago with no round running raises one
    /// notice per epic per hour, locally and to the other machine.
    #[test]
    fn merged_task_waiting_over_an_hour_notifies_once_an_hour_cas_9e4e() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        let repo = dir.path();
        let merged = Utc::now() - chrono::Duration::minutes(30);
        let stamp = merged.to_rfc3339();
        git(repo, &stamp, &["init", "-q", "-b", "main"]);
        git(repo, &stamp, &["commit", "-q", "--allow-empty", "-m", "seed"]);
        git(repo, &stamp, &["switch", "-qc", "epic/board"]);
        git(repo, &stamp, &["switch", "-qc", "factory/lane"]);
        git(repo, &stamp, &["commit", "-q", "--allow-empty", "-m", "delivery"]);
        let anchor = git(repo, &stamp, &["rev-parse", "HEAD"]);
        git(repo, &stamp, &["switch", "-q", "epic/board"]);
        git(repo, &stamp, &["merge", "-q", "--no-ff", "-m", "merge", "factory/lane"]);

        let tasks = open_task_store(&root).unwrap();
        let mut epic = Task::new("cas-ep1c".into(), "Board".into());
        epic.task_type = TaskType::Epic;
        epic.branch = Some("epic/board".into());
        tasks.add(&epic).unwrap();
        let mut delivery = Task::new("cas-de11".into(), "Delivery".into());
        delivery.status = TaskStatus::AwaitingMerge;
        delivery.deliverables.factory_branch_anchor = Some(anchor.clone());
        delivery.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:test".into(),
            target_branch: "epic/board".into(),
        });
        tasks.add(&delivery).unwrap();
        let mut round = Task::new("cas-r0und".into(), "3.92 batch QA round 7".into());
        round.status = TaskStatus::InProgress;
        tasks.add(&round).unwrap();
        for child in [&delivery.id, &round.id] {
            tasks
                .add_dependency(&Dependency::new(child.clone(), epic.id.clone(), DependencyType::ParentChild))
                .unwrap();
        }
        cas_store::defer_qa_batch_evidence(&root, &delivery.id, &anchor).unwrap();
        let mut supervisor = Agent::new("supervisor-id".into(), "calm-swan".into());
        supervisor.role = AgentRole::Supervisor;
        supervisor.factory_session = Some("session".into());
        open_agent_store(&root).unwrap().register(&supervisor).unwrap();

        let sent = RefCell::new(Vec::new());
        let mut alarm = Alarm::default();
        let mut run = |at: DateTime<Utc>| {
            alarm
                .check(&root, at, &|_| vec![peer()], &|sender, peer, body| {
                    sent.borrow_mut().push((sender.to_string(), peer.name.clone(), body.to_string()));
                    Ok(())
                })
                .unwrap();
            let rows = crate::store::open_prompt_queue_store(&root).unwrap().peek_all(50).unwrap();
            rows.into_iter().filter(|row| row.prompt.contains(NOTICE_KIND)).count()
        };
        // Merged 30 minutes ago: not overdue yet.
        assert_eq!(run(merged + chrono::Duration::minutes(30)), 0);
        // Overdue, but a round started recently is running.
        round.created_at = merged + chrono::Duration::minutes(40);
        tasks.update(&round).unwrap();
        assert_eq!(run(merged + chrono::Duration::minutes(70)), 0, "a running round covers it");
        assert!(sent.borrow().is_empty());

        // A round running for over an hour no longer holds the notice back.
        let first = merged + chrono::Duration::minutes(101);
        assert_eq!(run(first), 1);
        assert_eq!(run(first + chrono::Duration::minutes(10)), 1, "one notice an hour");
        assert_eq!(sent.borrow().len(), 1, "one peer notice an hour");
        let (sender, peer_name, body) = sent.borrow()[0].clone();
        assert_eq!((sender, peer_name), (supervisor.id.clone(), peer().name));
        // pin: the count and age are computed; the task specifies the instruction.
        assert!(body.contains("cas-ep1c: 1 merged task waiting, oldest 101m"), "{body}");
        // pin: the task specifies this instruction, which is the point of the notice.
        assert!(body.contains("lanes still in review go in the next round"), "{body}");
        assert_eq!(run(first + chrono::Duration::minutes(60)), 2, "and again an hour later");
        assert_eq!(sent.borrow().len(), 2);
    }
}
