use std::path::{Path, PathBuf};
use std::process::Command;

use cas::mcp::{CasCore, CasService};
use cas::store::{init_cas_dir, open_task_store};
use cas::types::{Task, TaskDepth, TaskStatus, WorkTarget};
use cas_mcp::types::TaskRequest;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::RawContent;
use tempfile::TempDir;

#[path = "../src/test_env_guard.rs"]
mod test_env_guard;
use test_env_guard::TestEnvGuard;

struct GitRepo {
    _temp: TempDir,
    root: PathBuf,
}

impl GitRepo {
    fn new() -> Self {
        let temp = TempDir::new().expect("temporary repository");
        let root = temp.path().to_path_buf();
        run_git(&root, &["init", "-b", "main"]);
        run_git(&root, &["config", "user.email", "test@test.com"]);
        run_git(&root, &["config", "user.name", "Test"]);
        std::fs::write(root.join("README.md"), "initial\n").unwrap();
        run_git(&root, &["add", "README.md"]);
        run_git(&root, &["commit", "-m", "initial"]);
        run_git(
            &root,
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:org/updated-target.git",
            ],
        );
        Self { _temp: temp, root }
    }
}

fn run_git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git command");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git command");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn task_request(value: serde_json::Value) -> TaskRequest {
    serde_json::from_value(value).expect("valid public task request")
}

fn result_text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| match &content.raw {
            RawContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn durable_snapshot(cas_root: &Path) -> Vec<(String, Vec<Vec<String>>)> {
    const TABLES: &[&str] = &[
        "tasks",
        "dependencies",
        "worker_completion_receipts",
        "worker_delivery_transactions",
        "worker_delivery_events",
        "verification_dispatches",
        "verifications",
        "events",
        "supervisor_queue",
        "prompt_queue",
    ];
    let connection = rusqlite::Connection::open(cas_root.join("cas.db")).unwrap();
    TABLES
        .iter()
        .map(|table| {
            let exists = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                    [table],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap();
            if !exists {
                return ((*table).to_string(), Vec::new());
            }
            let mut statement = connection
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .unwrap();
            let column_count = statement.column_count();
            let rows = statement
                .query_map([], |row| {
                    (0..column_count)
                        .map(|index| {
                            use rusqlite::types::ValueRef;
                            Ok(match row.get_ref(index)? {
                                ValueRef::Null => "NULL".to_string(),
                                ValueRef::Integer(value) => value.to_string(),
                                ValueRef::Real(value) => value.to_string(),
                                ValueRef::Text(value) => {
                                    String::from_utf8_lossy(value).into_owned()
                                }
                                ValueRef::Blob(value) => format!("{value:?}"),
                            })
                        })
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            ((*table).to_string(), rows)
        })
        .collect()
}

async fn create_task(service: &CasService, request: serde_json::Value) -> String {
    let result = service
        .task(Parameters(task_request(request)))
        .await
        .expect("create task through the public service");
    result_text(&result)
        .split("Created task: ")
        .nth(1)
        .expect("successful task creation")
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn loose_task_parented_to_epic_closes_after_merge_to_epic_only_cas_e258() {
    let _env = TestEnvGuard::temp_home();
    // Public create registers the work target strictly; init_cas_dir only
    // initializes the project store, not this isolated HOME's host registry.
    cas::store::known_repos::ensure_host_schema().expect("fixture host registry schema");
    let repo = GitRepo::new();
    run_git(&repo.root, &["branch", "epic/lane"]);
    let cas_root = init_cas_dir(&repo.root).unwrap();
    std::fs::write(
        cas_root.join("config.toml"),
        "[worktrees]\nenabled = false\n[verification]\nenabled = false\n",
    )
    .unwrap();
    let store = open_task_store(&cas_root).unwrap();
    let service = CasService::new(CasCore::with_daemon(cas_root.clone(), None, None), None);
    let epic_id = create_task(
        &service,
        serde_json::json!({
            "action": "create", "title": "Epic already targets its own lane",
            "task_type": "epic", "depth": "light", "risk": "none",
            "proof_targets": "delivered.txt", "target_branch": "epic/lane"
        }),
    )
    .await;
    let task_id = create_task(
        &service,
        serde_json::json!({
            "action": "create", "title": "Created loose then parented",
            "depth": "light", "risk": "none"
        }),
    )
    .await;
    assert_eq!(
        store
            .get(&task_id)
            .unwrap()
            .deliverables
            .work_target
            .unwrap()
            .target_branch,
        "main"
    );
    service
        .task(Parameters(task_request(serde_json::json!({
            "action": "update", "id": task_id, "epic": epic_id
        }))))
        .await
        .expect("parent before recording any delivery");
    let epic = store.get(&epic_id).unwrap();
    let lane = epic
        .branch
        .expect("public epic creation records the live lane");
    let mut task = store.get(&task_id).unwrap();
    assert_eq!(
        task.deliverables
            .work_target
            .as_ref()
            .unwrap()
            .target_branch,
        lane
    );

    // Deliver only into the epic: merging to main would hide the original bug.
    run_git(&repo.root, &["checkout", "-b", "factory/alice", &lane]);
    std::fs::write(repo.root.join("delivered.txt"), "epic delivery\n").unwrap();
    run_git(&repo.root, &["add", "delivered.txt"]);
    run_git(&repo.root, &["commit", "-m", "deliver to epic only"]);
    let commit = git_stdout(&repo.root, &["rev-parse", "HEAD"]);
    run_git(&repo.root, &["checkout", &lane]);
    run_git(&repo.root, &["merge", "--ff-only", "factory/alice"]);
    assert_ne!(git_stdout(&repo.root, &["rev-parse", "main"]), commit);
    task.status = TaskStatus::InProgress;
    task.assignee = Some("alice".into());
    task.deliverables.factory_branch_anchor = Some(commit);
    store.update(&task).unwrap();
    let result = service
        .task(Parameters(task_request(serde_json::json!({
            "action": "close", "id": task_id, "reason": "delivery merged into epic"
        }))))
        .await
        .expect("close on the inherited epic target without an override");
    assert!(
        result_text(&result).contains("Closed task:"),
        "{}",
        result_text(&result)
    );
    assert_eq!(store.get(&task_id).unwrap().status, TaskStatus::Closed);
}

#[tokio::test]
async fn recorded_delivery_refuses_epic_update_with_exact_supervisor_command_cas_e258() {
    use cas::types::{TaskType, WorkerCompletionReceiptInput, WorkerDeliveryState};

    let _env = TestEnvGuard::temp_home();
    let repo = GitRepo::new();
    run_git(&repo.root, &["branch", "epic/lane"]);
    let cas_root = init_cas_dir(&repo.root).unwrap();
    let store = open_task_store(&cas_root).unwrap();
    let mut epic = Task::new("cas-e258-epic".into(), "epic".into());
    epic.task_type = TaskType::Epic;
    epic.branch = Some("epic/lane".into());
    epic.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/org/updated-target".into(),
        target_branch: "epic/lane".into(),
    });
    store.add(&epic).unwrap();
    let service = CasService::new(CasCore::with_daemon(cas_root.clone(), None, None), None);
    for state in [
        Some(WorkerDeliveryState::AwaitingMerge),
        Some(WorkerDeliveryState::ChangesRequested),
        None,
    ] {
        let suffix = state
            .map(|state| state.to_string())
            .unwrap_or_else(|| "parked-projection".into());
        let mut task = Task::new(format!("cas-e258-{suffix}"), "recorded delivery".into());
        task.status = if state.is_some() {
            TaskStatus::InProgress
        } else {
            TaskStatus::AwaitingMerge
        };
        task.deliverables.work_target = Some(WorkTarget {
            repo_selector: "remote:github.com/org/updated-target".into(),
            target_branch: "main".into(),
        });
        store.add(&task).unwrap();
        if let Some(state) = state {
            let sha = git_stdout(&repo.root, &["rev-parse", "HEAD"]);
            let receipt = cas_store::build_worker_completion_receipt(
                &WorkerCompletionReceiptInput {
                    task_id: task.id.clone(),
                    worker_agent_id: "worker-session".into(),
                    repo_selector: "remote:github.com/org/updated-target".into(),
                    source_branch: "factory/alice".into(),
                    commit_sha: sha.clone(),
                    merge_base_sha: sha.clone(),
                    target_branch: "main".into(),
                    target_sha: sha,
                    proof_reference: "fixture".into(),
                    scope_summary: "recorded scope".into(),
                    artifact_path: None,
                },
                "alice",
                chrono::Utc::now(),
            );
            cas_store::create_worker_delivery(&cas_root, &receipt, state, "worker-session")
                .unwrap();
        }
        let before = durable_snapshot(&cas_root);
        let error = service
            .task(Parameters(task_request(serde_json::json!({
                "action": "update", "id": task.id, "epic": epic.id,
                "title": "must not partially apply"
            }))))
            .await
            .expect_err("every recorded delivery freezes ordinary epic moves");
        let text = error.message.to_string();
        assert!(text.contains("DELIVERY PROOF SCOPE LOCKED"), "{text}");
        assert!(text.contains(&format!("\"id\":\"{}\"", task.id)), "{text}");
        assert!(text.contains("\"proof_scope_fix\":true"), "{text}");
        assert!(text.contains("\"target_branch\":\"epic/lane\""), "{text}");
        assert!(text.contains("registered supervisor"), "{text}");
        assert_eq!(
            durable_snapshot(&cas_root),
            before,
            "rejection must be read-only"
        );
        assert!(store.get_dependencies(&task.id).unwrap().is_empty());
    }
}

#[tokio::test]
async fn configured_standalone_target_and_repo_alias_follow_epic_but_pins_stay_cas_e258() {
    use cas::types::TaskType;

    let _env = TestEnvGuard::temp_home();
    let repo = GitRepo::new();
    run_git(&repo.root, &["branch", "integration"]);
    run_git(&repo.root, &["branch", "epic/lane"]);
    let cas_root = init_cas_dir(&repo.root).unwrap();
    std::fs::write(
        cas_root.join("config.toml"),
        "[project]\ncanonical_id = \"fixture/cas-e258\"\n[factory]\nepic_base_branch = \"integration\"\n[worktrees]\nenabled = false\n",
    )
    .unwrap();
    let store = open_task_store(&cas_root).unwrap();
    let mut epic = Task::new("cas-e258-alias-epic".into(), "epic".into());
    epic.task_type = TaskType::Epic;
    epic.branch = Some("epic/lane".into());
    epic.deliverables.work_target = Some(WorkTarget {
        repo_selector: "project:fixture/cas-e258".into(),
        target_branch: "epic/lane".into(),
    });
    store.add(&epic).unwrap();
    let service = CasService::new(CasCore::with_daemon(cas_root, None, None), None);
    for (id, selector, branch, expected) in [
        (
            "default",
            "remote:github.com/org/updated-target",
            "integration",
            "epic/lane",
        ),
        (
            "pin",
            "remote:github.com/org/updated-target",
            "release/operator",
            "release/operator",
        ),
        (
            "foreign",
            "remote:github.com/other/repo",
            "integration",
            "integration",
        ),
    ] {
        let mut task = Task::new(format!("cas-e258-{id}"), id.into());
        task.deliverables.work_target = Some(WorkTarget {
            repo_selector: selector.into(),
            target_branch: branch.into(),
        });
        store.add(&task).unwrap();
        service
            .task(Parameters(task_request(serde_json::json!({
                "action": "update", "id": task.id, "epic": epic.id
            }))))
            .await
            .unwrap();
        let target = store
            .get(&task.id)
            .unwrap()
            .deliverables
            .work_target
            .unwrap();
        assert_eq!(target.target_branch, expected, "{id}");
        assert_eq!(
            target.repo_selector,
            if id == "default" {
                "project:fixture/cas-e258"
            } else {
                selector
            }
        );
    }
}

#[tokio::test]
async fn combined_work_target_update_and_close_uses_the_updated_branch() {
    let home = TempDir::new().expect("temporary HOME");
    let mut env = TestEnvGuard::new();
    env.set("HOME", home.path());

    let repo = GitRepo::new();
    run_git(&repo.root, &["branch", "alternate"]);
    run_git(&repo.root, &["checkout", "-b", "factory/alice"]);
    std::fs::write(repo.root.join("worker.rs"), "pub fn delivered() {}\n").unwrap();
    run_git(&repo.root, &["add", "worker.rs"]);
    run_git(&repo.root, &["commit", "-m", "worker change"]);
    run_git(&repo.root, &["checkout", "main"]);
    run_git(&repo.root, &["merge", "--ff-only", "factory/alice"]);
    run_git(
        &repo.root,
        &["update-ref", "refs/remotes/origin/main", "main"],
    );
    let worker_commit = git_stdout(&repo.root, &["rev-parse", "factory/alice"]);

    let cas_root = init_cas_dir(&repo.root).expect("initialize CAS");
    std::fs::write(
        cas_root.join("config.toml"),
        "[worktrees]\nenabled = false\n[verification]\nenabled = false\n",
    )
    .unwrap();
    let task_store = open_task_store(&cas_root).expect("task store");
    let mut task = Task::new(
        "cas-updated-target-close".to_string(),
        "Close only against updated target".to_string(),
    );
    task.status = TaskStatus::InProgress;
    task.depth = TaskDepth::Light;
    task.assignee = Some("alice".to_string());
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/org/updated-target".to_string(),
        target_branch: "main".to_string(),
    });
    task.deliverables.factory_branch_anchor = Some(worker_commit.clone());
    task_store.add(&task).expect("add task");

    let before = durable_snapshot(&cas_root);
    let service = CasService::new(CasCore::with_daemon(cas_root.clone(), None, None), None);
    let error = service
        .task(Parameters(task_request(serde_json::json!({
            "action": "update",
            "id": task.id,
            "target_branch": "alternate",
            "status": "closed"
        }))))
        .await
        .expect_err("updated target must reject the close");
    let text = error.message.to_string();
    assert!(text.contains("PRE-CLOSE HOOK CONTEXT REJECTED"));
    assert!(text.contains(&worker_commit));
    assert!(text.contains("live target_branch `alternate`"));
    assert!(text.contains("(local)"));
    assert_eq!(
        durable_snapshot(&cas_root),
        before,
        "a rejected combined target update and close must have zero durable mutation"
    );
    let unchanged = task_store.get(&task.id).unwrap();
    assert_eq!(unchanged.status, TaskStatus::InProgress);
    assert_eq!(
        unchanged.deliverables.work_target.unwrap().target_branch,
        "main"
    );

    let wrong_repo = TempDir::new().expect("explicit wrong repository path");
    let before_wrong_repo = durable_snapshot(&cas_root);
    let wrong_repo_error = service
        .task(Parameters(task_request(serde_json::json!({
            "action": "update",
            "id": task.id,
            "target_repo": wrong_repo.path(),
            "status": "closed"
        }))))
        .await
        .expect_err("an explicit non-repository target must fail closed");
    assert!(wrong_repo_error.message.contains("WORK TARGET REJECTED"));
    assert_eq!(durable_snapshot(&cas_root), before_wrong_repo);

    let legacy_close = service
        .task(Parameters(task_request(serde_json::json!({
            "action": "update",
            "id": task.id,
            "status": "closed"
        }))))
        .await
        .expect("safe direct close against the unchanged main target");
    assert!(result_text(&legacy_close).contains("Updated task"));
    assert_eq!(task_store.get(&task.id).unwrap().status, TaskStatus::Closed);
}

#[tokio::test]
async fn anchored_no_code_task_can_add_parked_proof_and_close_without_code_hook() {
    let home = TempDir::new().expect("temporary HOME");
    let mut env = TestEnvGuard::new();
    env.set("HOME", home.path());

    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("initialize CAS");
    std::fs::write(
        cas_root.join("config.toml"),
        "[worktrees]\nenabled = false\n[verification]\nenabled = false\n",
    )
    .unwrap();
    let task_store = open_task_store(&cas_root).expect("task store");
    let mut task = Task::new(
        "cas-f1f8-no-code-anchor".to_string(),
        "Produce an operations report".to_string(),
    );
    task.status = TaskStatus::AwaitingMerge;
    task.depth = TaskDepth::Light;
    task.execution_note = Some("no-code".to_string());
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/org/updated-target".to_string(),
        target_branch: "main".to_string(),
    });
    task_store.add(&task).expect("add anchored no-code task");

    let service = CasService::new(CasCore::with_daemon(cas_root.clone(), None, None), None);
    let task_type_error = service
        .task(Parameters(task_request(serde_json::json!({
            "action": "update",
            "id": task.id,
            "task_type": "chore"
        }))))
        .await
        .expect_err("task_type updates must be rejected explicitly");
    assert!(task_type_error.message.contains("TASK UPDATE REJECTED"));
    assert!(task_type_error.message.contains("task_type is create-only"));
    assert!(!task_type_error.message.contains("No changes specified"));

    let proof = "artifact:reports/cas-f1f8-no-code-anchor.html";
    let update = service
        .task(Parameters(task_request(serde_json::json!({
            "action": "update",
            "id": task.id,
            "external_ref": proof
        }))))
        .await
        .expect("the first proof reference must remain writable while parked");
    assert!(result_text(&update).contains("external_ref"));
    assert_eq!(
        task_store.get(&task.id).unwrap().external_ref.as_deref(),
        Some(proof)
    );

    let replacement = service
        .task(Parameters(task_request(serde_json::json!({
            "action": "update",
            "id": task.id,
            "external_ref": "artifact:reports/replacement.html"
        }))))
        .await
        .expect_err("an already-recorded parked proof must stay immutable");
    assert!(replacement.message.contains("DELIVERY PROOF SCOPE LOCKED"));
    assert_eq!(
        task_store.get(&task.id).unwrap().external_ref.as_deref(),
        Some(proof),
        "the rejected replacement must preserve the approved proof"
    );

    let close = service
        .task(Parameters(task_request(serde_json::json!({
            "action": "close",
            "id": task.id,
            "reason": "Operations report published"
        }))))
        .await
        .expect("anchored no-code close");
    let close_text = result_text(&close);
    assert!(
        close_text.contains("Closed task:"),
        "no-code must bypass the code-only declared hook: {close_text}"
    );
    assert!(!close_text.contains("PRE-CLOSE HOOK CONTEXT REJECTED"));
    assert_eq!(task_store.get(&task.id).unwrap().status, TaskStatus::Closed);
}
