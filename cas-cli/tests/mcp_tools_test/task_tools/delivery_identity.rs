//! cas-faa9: Nov 23's delivery-identity refusals replayed through the real
//! create/start/update/transfer/close paths, not the attribution helper.
use crate::support::*;
use cas::mcp::CasService;
use cas::store::{open_agent_store, open_task_store};
use cas::types::{Agent, AgentRole, Task, TaskStatus, TaskType};
use cas_mcp::types::TaskRequest;
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

fn commit(repo: &Path, file: &str, message: &str) -> String {
    std::fs::write(repo.join(file), format!("{file}\n")).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-qm", message]);
    git(repo, &["rev-parse", "HEAD"])
}

async fn call(service: &CasService, request: serde_json::Value) -> String {
    let request: TaskRequest = serde_json::from_value(request).unwrap();
    match service.task(Parameters(request)).await {
        Ok(out) => extract_text(out),
        Err(error) => error.message.to_string(),
    }
}

struct Fixture {
    _env: TestEnvGuard,
    temp: tempfile::TempDir,
    _origin: tempfile::TempDir,
    worker: CasService,
    supervisor: CasService,
}

impl Fixture {
    fn repo(&self) -> &Path { self.temp.path() }
    fn cas_dir(&self) -> std::path::PathBuf { self.repo().join(".cas") }
}

/// A repo with `main`, two trains cut from it, an origin, a registered
/// supervisor, and the worker on `factory/test-agent`. `epic_owns_train_a`
/// stores an epic owning `epic/train-a`.
fn fixture(epic_owns_train_a: bool) -> Fixture {
    let mut env = TestEnvGuard::temp_home();
    let (temp, core) = setup_cas_as(&mut env, AgentRole::Worker);
    let repo = temp.path();
    let cas_dir = repo.join(".cas");
    let origin = tempfile::TempDir::new().unwrap();
    git(origin.path(), &["init", "-q", "--bare"]);
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["remote", "add", "origin", origin.path().to_str().unwrap()]);
    std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
    git(repo, &["add", ".gitignore"]);
    git(repo, &["commit", "-qm", "seed"]);
    git(repo, &["branch", "epic/train-a"]);
    git(repo, &["branch", "epic/train-b"]);
    git(repo, &["push", "-q", "origin", "main", "epic/train-a", "epic/train-b"]);
    std::fs::write(
        cas_dir.join("config.toml"),
        "[project]\ncanonical_id='cas-faa9-fixture'\n[verification]\nenabled=false\n[qa]\nindependent_pass=false\n",
    )
    .unwrap();
    cas::store::known_repos::ensure_host_schema().unwrap();
    let agents = open_agent_store(&cas_dir).unwrap();
    let mut source = agents.get(&format!("test-session-{}", std::process::id())).unwrap();
    source.metadata.insert("clone_path".into(), repo.display().to_string());
    agents.update(&source).unwrap();
    agents
        .register(&Agent::new_with_role("faa9-supervisor".into(), "faa9-supervisor".into(), AgentRole::Supervisor))
        .unwrap();
    if epic_owns_train_a {
        let mut epic = Task::new("cas-e0a1".into(), "Train A".into());
        epic.task_type = TaskType::Epic;
        epic.branch = Some("epic/train-a".into());
        open_task_store(&cas_dir).unwrap().add(&epic).unwrap();
    }
    git(repo, &["checkout", "-qb", "factory/test-agent"]);
    let supervisor = cas::mcp::CasCore::with_daemon(cas_dir.clone(), None, None);
    supervisor.set_agent_id_for_testing("faa9-supervisor".into());
    Fixture {
        _env: env,
        temp,
        _origin: origin,
        worker: CasService::new(core, None),
        supervisor: CasService::new(supervisor, None),
    }
}

impl Fixture {
    async fn as_supervisor(&mut self, request: serde_json::Value) -> String {
        self._env.set("CAS_AGENT_ROLE", "supervisor");
        let out = call(&self.supervisor, request).await;
        self._env.remove("CAS_AGENT_ROLE");
        out
    }

    async fn create_and_start(&self, target: &str) -> String {
        let created = call(&self.worker, serde_json::json!({
            "action": "create", "title": "Identity replay", "risk": "none", "depth": "light",
            "target_repo": self.repo(), "target_branch": target,
        })).await;
        let id = extract_task_id(&created).expect(&created).to_string();
        let started = call(&self.worker, serde_json::json!({"action": "start", "id": id})).await;
        let tasks = open_task_store(&self.cas_dir()).unwrap();
        assert_eq!(tasks.get(&id).unwrap().status, TaskStatus::InProgress, "{started}");
        id
    }
}

#[tokio::test]
async fn delivery_identity_follows_retargets_and_adoption_cas_faa9() {
    struct Row {
        name: &'static str,
        created_on: &'static str,
        epic_owns_train_a: bool,
        // 6456's row predates the recorded prior-target list; it then goes
        // through an explicit rebind_branch before close.
        legacy_row: bool,
        // GH #2307: the successor target is deleted before close.
        delete_successor: bool,
        parks: bool,
    }
    for row in [
        // GH ef8f is the same shape: briefed onto a train, laned off its tip,
        // then G-100 moves the task to the successor cut from main.
        Row { name: "eea8 briefed train, G-100 successor", created_on: "main", epic_owns_train_a: true, legacy_row: false, delete_successor: false, parks: true },
        Row { name: "6456 start recorded on the old train", created_on: "epic/train-a", epic_owns_train_a: true, legacy_row: true, delete_successor: false, parks: true },
        Row { name: "control: a plain branch is not upstream", created_on: "main", epic_owns_train_a: false, legacy_row: false, delete_successor: false, parks: false },
        Row { name: "2307 deleted successor target", created_on: "main", epic_owns_train_a: true, legacy_row: false, delete_successor: true, parks: false },
    ] {
        let mut fx = fixture(row.epic_owns_train_a);
        let repo = fx.repo().to_path_buf();
        let id = fx.create_and_start(row.created_on).await;
        // Another task's reviewed work lands on train A after this start.
        git(&repo, &["checkout", "-q", "epic/train-a"]);
        let foreign = commit(&repo, "train.rs", "chore(cas-12d1): reviewed train work");
        git(&repo, &["push", "-q", "origin", "epic/train-a"]);
        git(&repo, &["checkout", "-q", "factory/test-agent"]);
        if row.created_on != "epic/train-a" {
            let moved = fx.as_supervisor(serde_json::json!({
                "action": "update", "id": id, "target_repo": repo, "target_branch": "epic/train-a",
            })).await;
            assert!(!moved.contains("REJECTED"), "{}: {moved}", row.name);
        }
        git(&repo, &["merge", "-q", "--ff-only", "epic/train-a"]);
        commit(&repo, "own.rs", &format!("fix({id}): own work"));
        git(&repo, &["push", "-q", "origin", "factory/test-agent"]);
        let moved = fx.as_supervisor(serde_json::json!({
            "action": "update", "id": id, "target_repo": repo, "target_branch": "epic/train-b",
        })).await;
        let tasks = open_task_store(&fx.cas_dir()).unwrap();
        assert_eq!(
            tasks.get(&id).unwrap().deliverables.work_target.map(|t| t.target_branch).as_deref(),
            Some("epic/train-b"), "{}: {moved}", row.name,
        );
        if row.legacy_row {
            let conn = rusqlite::Connection::open(fx.cas_dir().join("cas.db")).unwrap();
            conn.execute(
                "UPDATE tasks SET deliverables = json_remove(deliverables, '$.prior_work_targets') WHERE id = ?1",
                [&id],
            ).unwrap();
            assert!(tasks.get(&id).unwrap().deliverables.prior_work_targets.is_empty());
            // Then the supervisor approves a recovery lane: the start that
            // alone names train A is replaced, so the writer must keep it.
            let recovery = format!("factory/test-agent-{id}");
            git(&repo, &["checkout", "-qb", &recovery]);
            git(&repo, &["push", "-q", "origin", &recovery]);
            let rebound = fx.as_supervisor(serde_json::json!({
                "action": "rebind_branch", "id": id, "branch": recovery,
                "reason": "Approve the recovery lane",
            })).await;
            let start = tasks.get(&id).unwrap().deliverables.factory_branch_start.unwrap();
            assert_eq!(start.branch, recovery, "{}: {rebound}", row.name);
        }
        if row.delete_successor {
            git(&repo, &["push", "-q", "origin", "--delete", "epic/train-b"]);
            git(&repo, &["branch", "-qD", "epic/train-b"]);
        }
        let closed = call(&fx.worker, serde_json::json!({
            "action": "close", "id": id, "reason": "identity replay",
        })).await;
        let status = tasks.get(&id).unwrap().status;
        if row.parks {
            assert_eq!(status, TaskStatus::AwaitingMerge, "{}: {closed}", row.name);
        } else {
            // The refusal names what is wrong: the foreign commit, or the
            // missing target rather than an unreadable lane.
            let named = if row.delete_successor { "epic/train-b" } else { foreign.as_str() };
            assert_eq!(status, TaskStatus::InProgress, "{}: {closed}", row.name);
            assert!(closed.contains("[G-42]") && closed.contains(named), "{}: {closed}", row.name);
        }
    }

    // GH #2334 then #2312: the supervisor hands a started delivery to a new
    // worker; the receiver's queued start is accepted on the adopted lane, and
    // its next commit is the anchor its park records.
    let mut fx = fixture(false);
    let repo = fx.repo().to_path_buf();
    let receiver_clone = tempfile::TempDir::new().unwrap();
    git(&repo, &["worktree", "add", "-q", "-b", "factory/new-worker", receiver_clone.path().to_str().unwrap(), "main"]);
    let agents = open_agent_store(&fx.cas_dir()).unwrap();
    let mut receiver = Agent::new_with_role("new-worker-id".into(), "new-worker".into(), AgentRole::Worker);
    receiver.metadata.insert("clone_path".into(), receiver_clone.path().display().to_string());
    agents.register(&receiver).unwrap();
    let id = fx.create_and_start("main").await;
    commit(&repo, "first.rs", &format!("fix({id}): first half"));
    git(&repo, &["push", "-q", "origin", "factory/test-agent"]);
    let adopted = fx.as_supervisor(serde_json::json!({
        "action": "transfer", "id": id, "to_agent": "new-worker", "adopt_branch": true,
        "supervisor_override": true, "reason": "Hand the delivery over",
    })).await;
    let lane = format!("factory/new-worker-{id}");
    assert_eq!(git(receiver_clone.path(), &["branch", "--show-current"]), lane, "{adopted}");
    let receiver_core = cas::mcp::CasCore::with_daemon(fx.cas_dir(), None, None);
    receiver_core.set_agent_id_for_testing("new-worker-id".into());
    let receiver_service = CasService::new(receiver_core, None);
    let started = call(&receiver_service, serde_json::json!({"action": "start", "id": id})).await;
    let tasks = open_task_store(&fx.cas_dir()).unwrap();
    let task = tasks.get(&id).unwrap();
    assert_eq!(
        (task.status, task.deliverables.factory_branch_start.map(|start| start.branch)),
        (TaskStatus::InProgress, Some(lane.clone())), "{started}",
    );
    let tip = commit(receiver_clone.path(), "second.rs", &format!("fix({id}): second half"));
    git(receiver_clone.path(), &["push", "-q", "origin", &lane]);
    let closed = call(&receiver_service, serde_json::json!({
        "action": "close", "id": id, "reason": "adopted delivery",
    })).await;
    let task = tasks.get(&id).unwrap();
    assert_eq!(
        (task.status, task.deliverables.factory_branch_anchor.as_deref()),
        (TaskStatus::AwaitingMerge, Some(tip.as_str())), "{closed}",
    );
}
