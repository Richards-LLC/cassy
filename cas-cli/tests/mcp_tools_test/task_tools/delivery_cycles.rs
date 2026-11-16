//! cas-50b2: every production anchor write mints or appends the task's
//! `DeliveryCycle`; only request-changes and reopen start a new cycle.
use crate::support::*;
use cas::hooks::{HookInput, handle_post_tool_use};
use cas::mcp::CasService;
use cas_mcp::types::TaskRequest;
use cas::store::{open_agent_store, open_task_store};
use cas::types::{Agent, AgentRole, TaskStatus};
use rmcp::handler::server::wrapper::Parameters;
use std::path::Path;

fn git(repo: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn cycles(cas_dir: &Path, task: &str) -> Vec<(u32, Vec<String>, bool)> {
    cas_store::delivery_cycles(cas_dir, task)
        .unwrap()
        .into_iter()
        .map(|cycle| (cycle.cycle, cycle.anchors, cycle.ended))
        .collect()
}

fn commit(repo: &Path, task: &str, file: &str) -> String {
    std::fs::write(repo.join(file), format!("{file}\n")).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-qm", &format!("fix({task}): {file}")]);
    git(repo, &["rev-parse", "HEAD"])
}

fn commit_hook(repo: &Path, cas_dir: &Path, session: &str) {
    handle_post_tool_use(
        &HookInput {
            session_id: session.to_string(),
            cwd: repo.display().to_string(),
            hook_event_name: "PostToolUse".to_string(),
            tool_name: Some("Bash".to_string()),
            tool_input: Some(serde_json::json!({"command": "git commit -qm work"})),
            tool_response: Some(serde_json::json!("")),
            agent_role: Some("worker".to_string()),
            ..Default::default()
        },
        Some(cas_dir),
    )
    .expect("post-tool hook");
}

fn task_request(value: serde_json::Value) -> Parameters<TaskRequest> {
    Parameters(serde_json::from_value(value).unwrap())
}

#[tokio::test]
async fn every_anchor_writer_mints_or_appends_its_delivery_cycle_cas_50b2() {
    let mut env = TestEnvGuard::temp_home();
    let (temp, core) = setup_cas_as(&mut env, AgentRole::Worker);
    let repo = temp.path();
    let cas_dir = repo.join(".cas");
    let worker = format!("test-session-{}", std::process::id());
    let origin = tempfile::TempDir::new().unwrap();
    let receiver_clone = tempfile::TempDir::new().unwrap();
    git(origin.path(), &["init", "-q", "--bare"]);
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["remote", "add", "origin", origin.path().to_str().unwrap()]);
    std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
    git(repo, &["add", ".gitignore"]);
    git(repo, &["commit", "-qm", "seed"]);
    git(repo, &["push", "-q", "origin", "main"]);
    git(repo, &["worktree", "add", "-q", "-b", "factory/new-worker",
        receiver_clone.path().to_str().unwrap()]);
    git(repo, &["checkout", "-qb", "factory/test-agent"]);
    let agents = open_agent_store(&cas_dir).unwrap();
    let mut source = agents.get(&worker).unwrap();
    source.metadata.insert("clone_path".into(), repo.display().to_string());
    agents.update(&source).unwrap();
    let mut receiver = Agent::new_with_role("new-worker-id".into(), "new-worker".into(), AgentRole::Worker);
    receiver.metadata.insert("clone_path".into(), receiver_clone.path().display().to_string());
    agents.register(&receiver).unwrap();
    agents
        .register(&Agent::new_with_role("cycle-supervisor".into(), "cycle-supervisor".into(), AgentRole::Supervisor))
        .unwrap();
    let supervisor = cas::mcp::CasCore::with_daemon(cas_dir.clone(), None, None);
    supervisor.set_agent_id_for_testing("cycle-supervisor".into());
    let supervisor = CasService::new(supervisor, None);
    let service = CasService::new(core.clone(), None);
    let tasks = open_task_store(&cas_dir).unwrap();

    let id = extract_task_id(&extract_text(
        service.task(task_request(serde_json::json!({
            "action": "create", "title": "Delivery cycle", "risk": "none",
        }))).await.unwrap(),
    )).unwrap().to_string();
    service.task(task_request(serde_json::json!({"action": "start", "id": id}))).await.unwrap();
    let close = || async {
        let request = task_request(serde_json::json!({
            "action": "close", "id": id, "reason": "delivery cycle fixture",
        }));
        match service.task(request).await {
            Ok(out) => extract_text(out),
            Err(error) => error.message.to_string(),
        }
    };

    // Commit hook: the first anchor write mints cycle 1.
    let a = commit(repo, &id, "a.txt");
    commit_hook(repo, &cas_dir, &worker);
    assert_eq!(cycles(&cas_dir, &id), [(1, vec![a.clone()], false)]);

    // Park keeps the commit-time anchor; re-parking it makes no duplicate.
    git(repo, &["push", "-q", "origin", "factory/test-agent"]);
    let parked = close().await;
    assert_eq!(tasks.get(&id).unwrap().status, TaskStatus::AwaitingMerge, "{parked}");
    let _ = close().await;
    assert_eq!(cycles(&cas_dir, &id), [(1, vec![a.clone()], false)]);

    // An AwaitingMerge retry advances the anchor: one append, same cycle.
    let b = commit(repo, &id, "b.txt");
    git(repo, &["push", "-q", "origin", "factory/test-agent"]);
    let advanced = close().await;
    assert_eq!(tasks.get(&id).unwrap().deliverables.factory_branch_anchor.as_deref(), Some(b.as_str()), "{advanced}");
    assert_eq!(cycles(&cas_dir, &id), [(1, vec![a.clone(), b.clone()], false)]);

    // Request-changes ends the cycle; the corrective commit mints cycle 2.
    env.set("CAS_AGENT_ROLE", "supervisor");
    supervisor.task(task_request(serde_json::json!({
        "action": "request_changes", "id": id, "reason": "Rework; prior commits stand",
    }))).await.unwrap();
    env.remove("CAS_AGENT_ROLE");
    service.task(task_request(serde_json::json!({"action": "start", "id": id}))).await.unwrap();
    let c = commit(repo, &id, "c.txt");
    commit_hook(repo, &cas_dir, &worker);
    let first = (1, vec![a.clone(), b.clone()], true);
    assert_eq!(cycles(&cas_dir, &id), [first.clone(), (2, vec![c.clone()], false)]);

    // Conflict rework retires the parked anchor inside the cycle; a transfer
    // that adopts the reworked branch appends its tip to that same cycle.
    git(repo, &["push", "-q", "origin", "factory/test-agent"]);
    let parked = close().await;
    let mut rework = tasks.get(&id).unwrap();
    assert_eq!(rework.status, TaskStatus::AwaitingMerge, "{parked}");
    rework.status = TaskStatus::InProgress;
    tasks.update(&rework).unwrap();
    assert_eq!(tasks.get(&id).unwrap().deliverables.factory_branch_anchor, None);
    let d = commit(repo, &id, "d.txt");
    git(repo, &["push", "-q", "origin", "factory/test-agent"]);
    env.set("CAS_AGENT_ROLE", "supervisor");
    let adopted = supervisor.task(task_request(serde_json::json!({
        "action": "transfer", "id": id, "to_agent": "new-worker", "adopt_branch": true,
        "supervisor_override": true, "reason": "Hand the reworked branch over",
    }))).await;
    assert!(adopted.is_ok(), "{adopted:?}");
    assert_eq!(tasks.get(&id).unwrap().deliverables.factory_branch_anchor.as_deref(), Some(d.as_str()));
    assert_eq!(cycles(&cas_dir, &id), [first.clone(), (2, vec![c.clone(), d.clone()], false)]);

    // Reopen ends the cycle too; the next anchor write mints cycle 3.
    let mut closed = tasks.get(&id).unwrap();
    closed.status = TaskStatus::Closed;
    closed.closed_at = Some(chrono::Utc::now());
    tasks.update(&closed).unwrap();
    env.set("CAS_AGENT_ROLE", "supervisor");
    supervisor.task(task_request(serde_json::json!({
        "action": "reopen", "id": id, "reason": "Fresh delivery cycle",
    }))).await.unwrap();
    let mut reopened = tasks.get(&id).unwrap();
    reopened.deliverables.mint_or_append(&d);
    tasks.update(&reopened).unwrap();
    assert_eq!(cycles(&cas_dir, &id), [
        first,
        (2, vec![c, d.clone()], true),
        (3, vec![d], false),
    ]);
}
