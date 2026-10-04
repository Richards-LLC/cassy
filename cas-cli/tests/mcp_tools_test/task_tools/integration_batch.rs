//! GH1097: a supervisor stages a pinned batch, then ordinary close recognizes its squash.
use crate::support::*;
use cas::mcp::{CasCore, CasService};
use cas::store::open_task_store;
use cas::types::{Agent, AgentRole, Task, TaskStatus};
use rmcp::handler::server::wrapper::Parameters;
use std::path::Path;
use std::process::Command;

const TASK: &str = "cas-b401";
fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "Batch QA")
        .env("GIT_AUTHOR_EMAIL", "batch@example.test")
        .env("GIT_COMMITTER_NAME", "Batch QA")
        .env("GIT_COMMITTER_EMAIL", "batch@example.test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().into()
}
fn commit(repo: &Path, path: &str, body: &str) -> String {
    let file = repo.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, body).unwrap();
    git(repo, &["add", path]);
    git(repo, &["commit", "-q", "-m", &format!("{TASK}: {path}")]);
    git(repo, &["rev-parse", "HEAD"])
}
fn fixture(env: &mut TestEnvGuard) -> (tempfile::TempDir, CasCore, CasCore, String) {
    let (temp, worker) = setup_cas(env);
    let repo = temp.path();
    std::fs::write(
        repo.join(".cas/config.toml"),
        "[verification]\nenabled=false\n[qa]\nindependent_pass=false\n",
    )
    .unwrap();
    git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "README.md", "seed\n");
    git(repo, &["checkout", "-q", "-b", "factory/test-agent"]);
    commit(repo, "src/one.rs", "pub fn one() {}\n");
    let head = commit(repo, "src/two.rs", "pub fn two() {}\n");
    let mut task = Task::new(TASK.into(), "Batch delivery".into());
    task.assignee = Some("test-agent".into());
    task.status = TaskStatus::AwaitingMerge;
    task.deliverables.factory_branch_anchor = Some(head.clone());
    task.deliverables.parked_branch = Some("factory/test-agent".into());
    open_task_store(&repo.join(".cas"))
        .unwrap()
        .add(&task)
        .unwrap();
    git(repo, &["branch", "batch/X", &head]);
    let id = format!("batch-supervisor-{}", std::process::id());
    cas::store::open_agent_store(&repo.join(".cas"))
        .unwrap()
        .register(&Agent::new_with_role(
            id.clone(),
            "batch-supervisor".into(),
            AgentRole::Supervisor,
        ))
        .unwrap();
    let supervisor = CasCore::with_daemon(repo.join(".cas"), None, None);
    supervisor.set_agent_id_for_testing(id);
    (temp, worker, supervisor, head)
}
async fn stage(core: CasCore, head: &str) -> Result<String, String> {
    let request = serde_json::from_value(
        serde_json::json!({"action":"update", "id":TASK, "merged_into":format!("batch/X@{head}")}),
    )
    .map_err(|err| err.to_string())?;
    CasService::new(core, None)
        .task(Parameters(request))
        .await
        .map(extract_text)
        .map_err(|err| err.to_string())
}
async fn close(core: CasCore) -> String {
    let request = serde_json::from_value(serde_json::json!({"action":"close", "id":TASK})).unwrap();
    match CasService::new(core, None).task(Parameters(request)).await {
        Ok(result) => extract_text(result),
        Err(err) => err.to_string(),
    }
}
fn squash(repo: &Path, drop_path: bool) {
    git(repo, &["checkout", "-q", "main"]);
    // Target advances after the batch was cut; its first parent is not the batch base.
    commit(repo, "later.txt", "target advanced\n");
    git(repo, &["merge", "--squash", "batch/X"]);
    if drop_path {
        git(repo, &["rm", "-q", "src/two.rs"]);
    }
    git(
        repo,
        &["commit", "-q", "-m", "Squashed integration batch X"],
    );
    git(repo, &["checkout", "-q", "factory/test-agent"]);
}

#[tokio::test]
async fn cas_4b26f_matching_batch_squash_auto_closes() {
    let mut env = TestEnvGuard::temp_home();
    let (temp, worker, supervisor, head) = fixture(&mut env);
    stage(supervisor, &head).await.unwrap();
    squash(temp.path(), false);
    let text = close(worker).await;
    assert_eq!(
        open_task_store(&temp.path().join(".cas"))
            .unwrap()
            .get(TASK)
            .unwrap()
            .status,
        TaskStatus::Closed,
        "{text}"
    );
    let task = open_task_store(&temp.path().join(".cas"))
        .unwrap()
        .get(TASK)
        .unwrap();
    assert!(
        task.notes.contains("integration batch") && task.notes.contains(&head),
        "{}",
        task.notes
    );
}

#[tokio::test]
async fn cas_4b26f_dropped_batch_path_is_rejected() {
    let mut env = TestEnvGuard::temp_home();
    let (temp, worker, supervisor, head) = fixture(&mut env);
    stage(supervisor, &head).await.unwrap();
    squash(temp.path(), true);
    assert!(close(worker).await.contains("MERGE REQUIRED"));
    assert_eq!(
        open_task_store(&temp.path().join(".cas"))
            .unwrap()
            .get(TASK)
            .unwrap()
            .status,
        TaskStatus::AwaitingMerge
    );
}

#[tokio::test]
async fn cas_4b26f_non_supervisor_cannot_stage_batch() {
    let mut env = TestEnvGuard::temp_home();
    let (temp, worker, _, head) = fixture(&mut env);
    let refused = stage(worker, &head).await.unwrap_err();
    assert!(refused.contains("registered supervisor"), "{refused}");
    let task = open_task_store(&temp.path().join(".cas"))
        .unwrap()
        .get(TASK)
        .unwrap();
    assert!(!task.notes.contains("integration batch"));
}

