//! cas-f823: replay of the v4.21 landing. Main takes the release commit; the
//! next open train must contain it without anyone syncing by hand, and a
//! conflicting main files exactly one sync task.
use super::*;
use crate::store::{open_agent_store, open_supervisor_queue_store, open_task_store};
use crate::test_support::TestEnvGuard;
use cas_types::{Agent, AgentRole};

const EPIC: &str = "cas-nx00";
const LANE: &str = "epic/release-train-cas-nx00";

fn run(repo: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git").arg("-C").arg(repo).args(args).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> bool {
    std::process::Command::new("git").arg("-C").arg(repo)
        .args(["merge-base", "--is-ancestor", ancestor, descendant]).status().unwrap().success()
}

/// A repo with origin, the next train lane published off main, one worker
/// mid-delivery on it. Leaves the checkout on main. Returns the Cassy dir.
fn next_train(repo: &Path, origin: &Path) -> std::path::PathBuf {
    run(origin, &["init", "-q", "--bare"]);
    run(repo, &["init", "-q", "-b", "main"]);
    run(repo, &["config", "user.name", "CAS Test"]);
    run(repo, &["config", "user.email", "cas@example.test"]);
    std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
    std::fs::write(repo.join("shared.txt"), "one\n").unwrap();
    run(repo, &["add", "."]);
    run(repo, &["commit", "-q", "-m", "seed"]);
    run(repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
    run(repo, &["push", "-q", "origin", "main"]);
    run(repo, &["checkout", "-q", "-b", LANE]);
    std::fs::write(repo.join("delivery.txt"), "work\n").unwrap();
    run(repo, &["add", "."]);
    run(repo, &["commit", "-q", "-m", "delivery"]);
    run(repo, &["push", "-q", "origin", LANE]);
    run(repo, &["checkout", "-q", "main"]);

    let cas_dir = crate::store::init_cas_dir(repo).unwrap();
    let agents = open_agent_store(&cas_dir).unwrap();
    agents.register(&Agent::new_with_role("sup-id".into(), "supervisor".into(), AgentRole::Supervisor)).unwrap();
    agents.register(&Agent::new_with_role("w-id".into(), "worker-a".into(), AgentRole::Worker)).unwrap();
    let tasks = open_task_store(&cas_dir).unwrap();
    let mut epic = Task::new(EPIC.into(), "Release train vNEXT".into());
    epic.task_type = TaskType::Epic;
    epic.labels = vec![crate::mcp::tools::core::task::release_train::RELEASE_TRAIN_LABEL.into()];
    epic.branch = Some(LANE.into());
    epic.epic_verification_owner = Some("sup-id".into());
    tasks.add(&epic).unwrap();
    let mut child = Task::new("cas-ch01".into(), "fix".into());
    child.status = TaskStatus::InProgress;
    child.assignee = Some("worker-a".into());
    tasks.add(&child).unwrap();
    tasks.add_dependency(&Dependency::new("cas-ch01".into(), EPIC.into(), DependencyType::ParentChild)).unwrap();
    cas_dir
}

/// The release lands: main gains a commit, published to origin.
fn land_release(repo: &Path, file: &str, body: &str) -> String {
    std::fs::write(repo.join(file), body).unwrap();
    run(repo, &["add", "."]);
    run(repo, &["commit", "-q", "-m", "release"]);
    run(repo, &["push", "-q", "origin", "main"]);
    run(repo, &["rev-parse", "HEAD"])
}

#[test]
fn landed_release_flows_into_the_open_train_and_tells_its_workers() {
    let _env = TestEnvGuard::temp_home();
    let (dir, origin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let cas_dir = next_train(dir.path(), origin.path());
    let release = land_release(dir.path(), "Cargo.lock", "4.22\n");

    assert_eq!(sync_open_trains(&cas_dir).unwrap(), 1);

    assert!(is_ancestor(dir.path(), &release, LANE), "the train contains the release");
    let local = run(dir.path(), &["rev-parse", LANE]);
    assert_eq!(run(origin.path(), &["rev-parse", LANE]), local, "and origin carries it");
    let queue = open_supervisor_queue_store(&cas_dir).unwrap();
    assert_eq!(queue.peek("w-id", 10).unwrap().len(), 1, "the in-flight worker is told");

    assert_eq!(sync_open_trains(&cas_dir).unwrap(), 0, "a synced train is left alone");
    assert_eq!(run(dir.path(), &["rev-parse", LANE]), local);
    assert_eq!(queue.peek("w-id", 10).unwrap().len(), 1);
}

#[test]
fn conflicting_main_files_one_sync_task_naming_the_files_and_changes_nothing() {
    let _env = TestEnvGuard::temp_home();
    let (dir, origin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let repo = dir.path();
    let cas_dir = next_train(repo, origin.path());
    run(repo, &["checkout", "-q", LANE]);
    std::fs::write(repo.join("shared.txt"), "train\n").unwrap();
    run(repo, &["commit", "-qam", "train edit"]);
    run(repo, &["push", "-q", "origin", LANE]);
    run(repo, &["checkout", "-q", "main"]);
    land_release(repo, "shared.txt", "main\n");
    let before = run(repo, &["rev-parse", LANE]);

    for _ in 0..3 {
        assert_eq!(sync_open_trains(&cas_dir).unwrap(), 0);
    }

    assert_eq!(run(repo, &["rev-parse", LANE]), before, "a conflict moves nothing");
    let tasks = open_task_store(&cas_dir).unwrap();
    let filed: Vec<_> = tasks.list(None).unwrap().into_iter()
        .filter(|task| task.labels.iter().any(|label| label == &format!("main-sync:{EPIC}")))
        .collect();
    assert_eq!(filed.len(), 1, "exactly one sync task");
    assert!(filed[0].description.contains("shared.txt"), "{}", filed[0].description);
    assert_eq!(tasks.get_parent_epic(&filed[0].id).unwrap().map(|epic| epic.id), Some(EPIC.to_string()));
    assert_eq!(open_supervisor_queue_store(&cas_dir).unwrap().peek("sup-id", 10).unwrap().len(), 1);
}

#[test]
fn timing_probe_cas_f823() {
    let _env = TestEnvGuard::temp_home();
    let (dir, origin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let repo = dir.path();
    let cas_dir = next_train(repo, origin.path());
    run(repo, &["checkout", "-q", LANE]);
    std::fs::write(repo.join("shared.txt"), "train\n").unwrap();
    run(repo, &["commit", "-qam", "train edit"]);
    run(repo, &["push", "-q", "origin", LANE]);
    run(repo, &["checkout", "-q", "main"]);
    land_release(repo, "shared.txt", "main\n");
    let started = std::time::Instant::now();
    sync_open_trains(&cas_dir).unwrap();
    let elapsed = started.elapsed();
    use std::io::Write;
    let mut out = std::fs::OpenOptions::new().create(true).append(true)
        .open("/home/pippenz/.cas/artifacts/cassy-bbb8e75581403d3edc2689512df64eaf37780d70a032cb0d79850218dfada14a/cas-f823/timing-delivery.txt").unwrap();
    writeln!(out, "{elapsed:?}").unwrap();
}
