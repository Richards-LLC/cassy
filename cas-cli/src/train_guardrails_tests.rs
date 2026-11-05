//! cas-dcc9: dropping one child keeps the rest of the train, and the second
//! ejection goes to the operator; one cut notice per train state.
use super::*;
use crate::store::{open_agent_store, open_supervisor_queue_store, open_task_store};
use crate::test_support::TestEnvGuard;
use cas_types::{Agent, AgentRole, Dependency, DependencyType};
use std::sync::Mutex;

const EPIC: &str = "cas-tr00";
const TRAIN: &str = "epic/release-train-cas-tr00";

fn run(repo: &Path, args: &[&str]) -> String {
    git(repo, args).unwrap()
}

#[derive(Default)]
struct FakeQueue(Mutex<Vec<u64>>);

impl TrainQueue for FakeQueue {
    fn open_pr(&self, branch: &str) -> Result<Option<u64>> {
        Ok((branch == TRAIN).then_some(42))
    }
    fn enqueue(&self, pr: u64) -> Result<()> {
        self.0.lock().unwrap().push(pr);
        Ok(())
    }
}

/// A train epic with `children` (id, status); returns the Cassy dir.
fn train(repo: &Path, children: &[(&str, TaskStatus)]) -> PathBuf {
    let cas_dir = crate::store::init_cas_dir(repo).unwrap();
    let tasks = open_task_store(&cas_dir).unwrap();
    let agents = open_agent_store(&cas_dir).unwrap();
    let mut supervisor = Agent::new_with_role("sup-id".into(), "supervisor".into(), AgentRole::Supervisor);
    supervisor.factory_session = Some("factory-session".into());
    agents.register(&supervisor).unwrap();
    let mut epic = Task::new(EPIC.into(), "Release train vNEXT".into());
    epic.task_type = TaskType::Epic;
    epic.labels = vec![RELEASE_TRAIN_LABEL.into()];
    epic.branch = Some(TRAIN.into());
    epic.epic_verification_owner = Some("sup-id".into());
    tasks.add(&epic).unwrap();
    for (id, status) in children {
        let worker = format!("worker-{id}");
        agents.register(&Agent::new_with_role(format!("{worker}-id"), worker.clone(), AgentRole::Worker)).unwrap();
        let mut task = Task::new((*id).into(), format!("fix {id}"));
        task.status = *status;
        task.assignee = Some(worker);
        tasks.add(&task).unwrap();
        tasks.add_dependency(&Dependency::new((*id).into(), EPIC.into(), DependencyType::ParentChild)).unwrap();
    }
    cas_dir
}

/// Merge `factory/<id>`, which adds `<id>.txt`, into the train and record it.
fn merge_child(repo: &Path, cas_dir: &Path, id: &str) {
    let branch = format!("factory/{id}");
    run(repo, &["checkout", "-q", "-b", &branch, TRAIN]);
    std::fs::write(repo.join(format!("{id}.txt")), id).unwrap();
    run(repo, &["add", "."]);
    run(repo, &["commit", "-q", "-m", id]);
    let source = run(repo, &["rev-parse", "HEAD"]);
    run(repo, &["checkout", "-q", TRAIN]);
    run(repo, &["merge", "-q", "--no-ff", "-m", &format!("merge {id}"), &branch]);
    let merge = run(repo, &["rev-parse", "HEAD"]);
    cas_store::record_observed_delivery_merge(cas_dir, id, &branch, TRAIN, &source, &merge, "sup-id").unwrap();
}

fn repo_with_origin() -> (tempfile::TempDir, tempfile::TempDir) {
    let origin = tempfile::tempdir().unwrap();
    run(origin.path(), &["init", "-q", "--bare"]);
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    run(repo, &["init", "-q", "-b", "main"]);
    run(repo, &["config", "user.name", "CAS Test"]);
    run(repo, &["config", "user.email", "cas@example.test"]);
    std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
    run(repo, &["add", "."]);
    run(repo, &["commit", "-q", "-m", "seed"]);
    run(repo, &["remote", "add", "origin", origin.path().to_str().unwrap()]);
    run(repo, &["checkout", "-q", "-b", TRAIN]);
    (dir, origin)
}

#[tokio::test]
async fn drop_reverts_one_child_keeps_the_rest_and_reenqueues_once_cas_dcc9() {
    let mut env = TestEnvGuard::temp_home();
    let (dir, _origin) = repo_with_origin();
    let repo = dir.path();
    let children = [("cas-a", TaskStatus::Closed), ("cas-b", TaskStatus::Closed), ("cas-c", TaskStatus::Closed)];
    let cas_dir = train(repo, &children);
    for (id, _) in children {
        merge_child(repo, &cas_dir, id);
    }
    // The supervisor's shared checkout stays on main; the train is a branch.
    run(repo, &["checkout", "-q", "main"]);
    run(repo, &["push", "-q", "origin", TRAIN]);
    let before = run(repo, &["rev-parse", TRAIN]);
    env.set("CAS_ROOT", &cas_dir);
    env.set("CAS_AGENT_ROLE", "supervisor");
    let core = CasCore::with_daemon(cas_dir.clone(), None, None);
    core.set_agent_id_for_testing("sup-id".into());
    let queue = FakeQueue::default();

    let receipt = drop_child(&core, "cas-b", "merge_group: Rust suite failed in b", &queue).await.unwrap();

    let files = run(repo, &["ls-tree", "--name-only", TRAIN]);
    assert!(files.contains("cas-a.txt") && files.contains("cas-c.txt") && !files.contains("cas-b.txt"), "{files}");
    assert!(is_ancestor(repo, &before, TRAIN), "a revert commit, never a history rewrite");
    assert_eq!(run(repo, &["rev-parse", TRAIN]), receipt.revert);
    assert_eq!(run(repo, &["ls-remote", "origin", TRAIN]).split_whitespace().next(), Some(receipt.revert.as_str()));
    let tasks = open_task_store(&cas_dir).unwrap();
    let dropped = tasks.get("cas-b").unwrap();
    assert_eq!((dropped.status, dropped.assignee.as_deref()), (TaskStatus::Open, Some("worker-cas-b")));
    assert!(dropped.notes.contains(&format!("git revert {}", receipt.revert)), "{}", dropped.notes);
    assert_eq!(tasks.get("cas-c").unwrap().status, TaskStatus::Closed);
    assert_eq!(*queue.0.lock().unwrap(), vec![42]);
    let sq = open_supervisor_queue_store(&cas_dir).unwrap();
    assert_eq!(sq.peek("worker-cas-b-id", 10).unwrap().len(), 1, "the worker gets the failure");

    // The re-enqueue was the train's one repair cycle (rule-176(5)).
    let error = drop_child(&core, "cas-c", "ejected again", &queue).await.unwrap_err().to_string();
    assert!(error.contains("P0") && error.contains("rule-176(5)"), "{error}");
    assert_eq!(run(repo, &["rev-parse", TRAIN]), receipt.revert, "nothing reverted");
    assert_eq!(*queue.0.lock().unwrap(), vec![42]);
    assert_eq!(sq.peek("operator", 10).unwrap().len(), 1);
}

#[test]
fn cut_notice_is_sent_once_when_ready_and_once_at_max_age_cas_dcc9() {
    let _env = TestEnvGuard::temp_home();
    let (dir, _origin) = repo_with_origin();
    let repo = dir.path();
    let cas_dir = train(repo, &[("cas-a", TaskStatus::Closed), ("cas-b", TaskStatus::InProgress)]);
    merge_child(repo, &cas_dir, "cas-a");
    let sq = open_supervisor_queue_store(&cas_dir).unwrap();
    let notices = || sq.peek("sup-id", 10).unwrap();
    let none = BTreeSet::new();
    let now = Utc::now();

    notify_train_cuts(&cas_dir, "factory-session", &none, now).unwrap();
    assert!(notices().is_empty(), "a child in progress on a young train waits");
    let later = now + chrono::Duration::hours(3);
    for _ in 0..2 {
        notify_train_cuts(&cas_dir, "factory-session", &none, later).unwrap();
    }
    assert_eq!(notices().len(), 1, "one max-age notice");
    assert!(notices()[0].payload.contains("max age"), "{}", notices()[0].payload);

    let tasks = open_task_store(&cas_dir).unwrap();
    let mut b = tasks.get("cas-b").unwrap();
    b.status = TaskStatus::Closed;
    tasks.update(&b).unwrap();
    notify_train_cuts(&cas_dir, "factory-session", &BTreeSet::from([TRAIN.to_string()]), now).unwrap();
    assert_eq!(notices().len(), 1, "a train in the merge queue is already being cut");
    for _ in 0..2 {
        notify_train_cuts(&cas_dir, "factory-session", &none, now).unwrap();
    }
    assert_eq!(notices().len(), 2, "one ready notice");
    assert!(notices()[1].payload.contains("scripts/release.sh"), "{}", notices()[1].payload);
}
