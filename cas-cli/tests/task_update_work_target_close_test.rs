use std::path::{Path, PathBuf};
use std::process::Command;

use cas::mcp::{CasCore, CasService};
use cas::store::{init_cas_dir, open_task_store};
use cas::types::{Task, TaskDepth, TaskStatus, WorkTarget};
use cas_mcp::types::TaskRequest;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::RawContent;
use tempfile::TempDir;

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

#[tokio::test]
async fn combined_work_target_update_and_close_uses_the_updated_branch() {
    let home = TempDir::new().expect("temporary HOME");
    let original_home = std::env::var_os("HOME");
    // SAFETY: this integration test is the only test in its process.
    unsafe { std::env::set_var("HOME", home.path()) };

    let repo = GitRepo::new();
    run_git(&repo.root, &["branch", "alternate"]);
    run_git(&repo.root, &["checkout", "-b", "factory/alice"]);
    std::fs::write(repo.root.join("worker.rs"), "pub fn delivered() {}\n").unwrap();
    run_git(&repo.root, &["add", "worker.rs"]);
    run_git(&repo.root, &["commit", "-m", "worker change"]);
    run_git(&repo.root, &["checkout", "main"]);
    run_git(&repo.root, &["merge", "--ff-only", "factory/alice"]);

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
    task.deliverables.factory_branch_anchor =
        Some(git_stdout(&repo.root, &["rev-parse", "factory/alice"]));
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
    assert!(
        text.contains("PRE-CLOSE HOOK CONTEXT REJECTED")
            && text.contains("not reachable from the declared target branch"),
        "the worker commit is not merged into the updated alternate target; got:\n{text}"
    );
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

    unsafe {
        match original_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }
}
