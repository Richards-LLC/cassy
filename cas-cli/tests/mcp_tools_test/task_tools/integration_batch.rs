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
    let tasks = open_task_store(&repo.join(".cas")).unwrap();
    let mut epic = Task::new("cas-be01".into(), "Integration batch epic".into());
    epic.task_type = cas::types::TaskType::Epic;
    epic.branch = Some("main".into());
    tasks.add(&epic).unwrap();
    let mut task = Task::new(TASK.into(), "Batch delivery".into());
    task.assignee = Some("test-agent".into());
    task.status = TaskStatus::AwaitingMerge;
    task.deliverables.factory_branch_anchor = Some(head.clone());
    task.deliverables.parked_branch = Some("factory/test-agent".into());
    open_task_store(&repo.join(".cas"))
        .unwrap()
        .add(&task)
        .unwrap();
    tasks
        .add_dependency(&cas::types::Dependency::new(
            TASK.into(),
            "cas-be01".into(),
            cas::types::DependencyType::ParentChild,
        ))
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
async fn close(core: CasCore, receipt: Option<&str>) -> String {
    let request = serde_json::from_value(
        serde_json::json!({"action":"close", "id":TASK, "commit_receipt":receipt}),
    )
    .unwrap();
    match CasService::new(core, None).task(Parameters(request)).await {
        Ok(result) => extract_text(result),
        Err(err) => err.to_string(),
    }
}
fn squash(repo: &Path, drop_path: Option<&str>) {
    git(repo, &["checkout", "-q", "main"]);
    // Target advances after the batch was cut; its first parent is not the batch base.
    commit(repo, "later.txt", "target advanced\n");
    git(repo, &["merge", "--squash", "batch/X"]);
    if let Some(path) = drop_path {
        if path == "extra" {
            std::fs::write(repo.join("extra.txt"), "unrecorded squash change\n").unwrap();
            git(repo, &["add", "extra.txt"]);
        } else {
            git(repo, &["rm", "-q", "-f", path]);
        }
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
    for explicit_receipt in [false, true] {
        let (temp, worker, supervisor, head) = fixture(&mut env);
        stage(supervisor.clone(), &head).await.unwrap();
        let service = CasService::new(supervisor, None);
        let shown = extract_text(
            service
                .task(Parameters(
                    serde_json::from_value(serde_json::json!({"action":"show", "id":TASK}))
                        .unwrap(),
                ))
                .await
                .unwrap(),
        );
        assert!(
            shown.contains("Staged in integration batch: batch/X@") && shown.contains(&head),
            "{shown}"
        );
        env.set("CAS_AGENT_ROLE", "supervisor");
        let report = extract_text(
            service
                .factory(Parameters(
                    serde_json::from_value(
                        serde_json::json!({"action":"epic_status", "id":"cas-be01"}),
                    )
                    .unwrap(),
                ))
                .await
                .unwrap(),
        );
        env.remove("CAS_AGENT_ROLE");
        assert!(
            report.contains("Staged integration batch receipts:") && report.contains(&head),
            "{report}"
        );
        let staged = open_task_store(&temp.path().join(".cas"))
            .unwrap()
            .get(TASK)
            .unwrap();
        assert_eq!(
            staged.deliverables.integration_batch.as_ref().unwrap().tip,
            head
        );
        squash(temp.path(), None);
        let squash_tip = git(temp.path(), &["rev-parse", "main"]);
        let text = close(worker, explicit_receipt.then_some(squash_tip.as_str())).await;
        let task = open_task_store(&temp.path().join(".cas"))
            .unwrap()
            .get(TASK)
            .unwrap();
        assert_eq!(task.status, TaskStatus::Closed, "{text}");
        env.set("CAS_AGENT_ROLE", "supervisor");
        let report = extract_text(
            service
                .factory(Parameters(
                    serde_json::from_value(
                        serde_json::json!({"action":"epic_status", "id":"cas-be01"}),
                    )
                    .unwrap(),
                ))
                .await
                .unwrap(),
        );
        env.remove("CAS_AGENT_ROLE");
        assert!(report.contains("integration batch squash"), "{report}");
        assert_eq!(
            task.deliverables.factory_branch_anchor.as_deref(),
            Some(head.as_str())
        );
        if let Some(scope) = task.deliverables.pre_close_hook.as_ref() {
            assert_eq!(scope.task_tip.as_deref(), Some(head.as_str()));
        }
        assert!(
            task.notes.contains("integration batch") && task.notes.contains(&head),
            "{}",
            task.notes
        );
    }
}

#[tokio::test]
async fn cas_4b26f_dropped_batch_path_is_rejected() {
    let mut env = TestEnvGuard::temp_home();
    for dropped in ["src/two.rs", "src/sibling.rs", "extra"] {
        let (temp, worker, supervisor, _) = fixture(&mut env);
        // The batch includes a sibling's work outside this task's own branch.
        // Even when this task's two files landed, partial batch proof must fail.
        git(temp.path(), &["checkout", "-q", "batch/X"]);
        std::fs::write(temp.path().join("src/sibling.rs"), "pub fn sibling() {}\n").unwrap();
        git(temp.path(), &["add", "src/sibling.rs"]);
        git(
            temp.path(),
            &["commit", "-q", "-m", "cas-b402: sibling delivery"],
        );
        let batch_tip = git(temp.path(), &["rev-parse", "HEAD"]);
        git(temp.path(), &["checkout", "-q", "factory/test-agent"]);
        stage(supervisor, &batch_tip).await.unwrap();
        squash(temp.path(), Some(dropped));
        let refused = close(worker, None).await;
        assert!(
            refused.contains("MERGE REQUIRED") && refused.contains("extra or missing paths"),
            "{refused}"
        );
        assert_eq!(
            open_task_store(&temp.path().join(".cas"))
                .unwrap()
                .get(TASK)
                .unwrap()
                .status,
            TaskStatus::AwaitingMerge
        );
    }
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
