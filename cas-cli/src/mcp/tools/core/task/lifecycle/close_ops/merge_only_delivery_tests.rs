//! cas-cfa5: a worker whose only commit merges main into the epic.
use super::*;
use std::process::Command;

fn git(dir: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?} failed");
}

fn sha(dir: &std::path::Path, rev: &str) -> String {
    let out = Command::new("git").args(["rev-parse", rev]).current_dir(dir).output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn commit_file(dir: &std::path::Path, name: &str, subject: &str) {
    std::fs::write(dir.join(name), format!("// {name}\n")).unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-q", "-m", subject]);
}

/// The cas-8ef3 shape: `epic/x` carries another task's lane; `main` gained
/// another task's fix; the worker cut `factory/worker` from the epic tip and
/// its only commit merges `main` in (a subject naming no task).
fn merge_only_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    commit_file(p, "seed.txt", "seed");
    git(p, &["checkout", "-q", "-b", "epic/x"]);
    commit_file(p, "lane.rs", "feat: an earlier child's lane (cas-0906)");
    git(p, &["checkout", "-q", "main"]);
    commit_file(p, "hotfix.rs", "fix: a hotfix on main (cas-1111)");
    git(p, &["checkout", "-q", "-b", "factory/worker", "epic/x"]);
    git(p, &["merge", "-q", "--no-ff", "-m", "Merge branch 'main' into factory/worker", "main"]);
    git(p, &["checkout", "-q", "epic/x"]);
    dir
}

fn worker_task() -> Task {
    let mut task = Task {
        id: "cas-test1".to_string(),
        title: "merge main into the epic".to_string(),
        status: TaskStatus::InProgress,
        assignee: Some("worker".to_string()),
        ..Default::default()
    };
    task.deliverables.work_target = Some(cas_types::WorkTarget {
        repo_selector: "project:fixture".into(),
        target_branch: "epic/x".into(),
    });
    task
}

fn req(id: &str) -> TaskCloseRequest {
    TaskCloseRequest {
        no_delivery: None,
        stranded_branch_override: None,
        id: id.to_string(),
        reason: None,
        supervisor_override: None,
        search_manifest: None,
        commit_receipt: None,
    }
}

#[test]
fn cas_cfa5_merge_only_delivery_resolves_the_worker_branch_and_parks() {
    let dir = merge_only_fixture();
    let p = dir.path();
    let task = worker_task();
    assert_eq!(
        resolve_close_delivery_branch(p, &task, "worker"),
        Ok("factory/worker".to_string()),
        "the merge commit is this task's delivery on the worker's real branch"
    );
    assert_eq!(close_measured_factory_branch(p, &task, "worker"), "factory/worker");
    match run_factory_branch_merge_gate(&task, &req(&task.id), "epic/x", p) {
        MergeStateGateOutcome::Reject(message) => {
            assert!(message.contains("MERGE REQUIRED"), "{message}");
            assert!(message.contains("factory/worker"), "{message}");
            assert!(!message.contains("factory/worker-cas-test1"), "{message}");
        }
        other => panic!("an unmerged merge-only delivery parks for merge: {other:?}"),
    }
}

#[test]
fn cas_cfa5_merged_merge_only_delivery_closes() {
    let dir = merge_only_fixture();
    let p = dir.path();
    let tip = sha(p, "factory/worker");
    git(p, &["merge", "-q", "--ff-only", "factory/worker"]);
    let mut task = worker_task();
    task.deliverables.factory_branch_anchor = Some(tip);
    match run_factory_branch_merge_gate(&task, &req(&task.id), "epic/x", p) {
        MergeStateGateOutcome::Proceed | MergeStateGateOutcome::ProceedWithNote(_) => {}
        other => panic!("a merged merge-only delivery closes: {other:?}"),
    }
}

#[test]
fn cas_cfa5_merge_that_changes_nothing_or_sits_on_own_work_is_not_merge_only() {
    let dir = merge_only_fixture();
    let p = dir.path();
    // A merge whose tree equals its first parent delivers nothing.
    git(p, &["checkout", "-q", "-b", "factory/noop", "epic/x"]);
    git(p, &["merge", "-q", "--no-ff", "-s", "ours", "-m", "Merge main (no change)", "main"]);
    assert!(!merge_only_delivery(p, "factory/noop", "epic/x"));
    // A merge on top of a lane commit claiming another task is that lane's.
    git(p, &["checkout", "-q", "-b", "factory/other", "epic/x"]);
    commit_file(p, "other.rs", "feat: another task's work (cas-2222)");
    git(p, &["merge", "-q", "--no-ff", "-m", "Merge main", "main"]);
    assert!(!merge_only_delivery(p, "factory/other", "epic/x"));
    // The real merge-only shape.
    assert!(merge_only_delivery(p, "factory/worker", "epic/x"));
    // Unknown targets and non-merge tips are never merge-only.
    assert!(!merge_only_delivery(p, "factory/worker", "epic/missing"));
    assert!(!merge_only_delivery(p, "epic/x", "main"));
}
