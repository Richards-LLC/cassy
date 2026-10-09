//! GH #1133, #1147, #1151: a no-code task with no commit of its own closes on
//! its portable proof. The worker's lane may end in another task's delivery;
//! that delivery is never measured, parked or anchored as this task's.
use super::*;
use crate::mcp::CasService;
use crate::store::{
    open_agent_store, open_rule_store, open_skill_store, open_store, open_task_store,
    open_worktree_store,
};
use crate::test_support::TestEnvGuard;
use cas_types::{Agent, AgentRole, Dependency, DependencyType, TaskRisk, WorkTarget};
use std::path::{Path, PathBuf};

const PROJECT: &str = "cas-no-code-fixture";
const WORKER: &str = "nocode-worker";
const LANE: &str = "factory/nocode-worker";
/// Another task's delivery, named by its task id as factory commits are.
const OTHER: &str = "fix(cas-a0b1): another task's delivery";
const PROOF: &str = "https://example.test/deploys/month";

fn git(repo: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "CAS Test")
        .env("GIT_AUTHOR_EMAIL", "cas@example.test")
        .env("GIT_COMMITTER_NAME", "CAS Test")
        .env("GIT_COMMITTER_EMAIL", "cas@example.test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

struct Fixture {
    dir: tempfile::TempDir,
    _origin: tempfile::TempDir,
    worker: CasService,
    supervisor: CasService,
    /// The worker lane's tip: another task's unmerged delivery.
    foreign_tip: String,
}

impl Fixture {
    fn cas_dir(&self) -> PathBuf {
        self.dir.path().join(".cas")
    }

    fn task(&self, id: &str) -> Task {
        open_task_store(&self.cas_dir()).unwrap().get(id).unwrap()
    }

    fn put(&self, task: &Task) {
        let store = open_task_store(&self.cas_dir()).unwrap();
        if store.get(&task.id).is_ok() {
            store.update(task).unwrap();
        } else {
            store.add(task).unwrap();
        }
    }

    fn artifact(&self, task_id: &str) -> PathBuf {
        let path = self
            .dir
            .path()
            .join("durable-artifacts")
            .join(task_id)
            .join("receipt.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "deploy receipt\n").unwrap();
        path
    }
}

async fn call(service: &CasService, request: serde_json::Value) -> String {
    let request = serde_json::from_value(request).unwrap();
    service
        .task(Parameters(request))
        .await
        .unwrap()
        .content
        .into_iter()
        .filter_map(|content| match content.raw {
            rmcp::model::RawContent::Text(text) => Some(text.text),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A repository whose `target` is on origin and whose worker lane ends in
/// another task's delivery, pushed but not merged; the worker works in a
/// linked worktree on that lane, as a factory worker does.
fn fixture(env: &mut TestEnvGuard, target: &str) -> Fixture {
    let origin = tempfile::tempdir().unwrap();
    git(origin.path(), &["init", "-q", "--bare"]);
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join(".gitignore"), ".cas/\ndurable-artifacts/\n").unwrap();
    git(repo, &["add", ".gitignore"]);
    git(repo, &["commit", "-q", "-m", "seed"]);
    git(repo, &["remote", "add", "origin", origin.path().to_str().unwrap()]);
    git(repo, &["push", "-q", "origin", "main"]);
    if target != "main" {
        git(repo, &["checkout", "-q", "-b", target]);
        git(repo, &["push", "-q", "origin", target]);
    }
    git(repo, &["checkout", "-q", "-b", LANE, target]);
    std::fs::write(repo.join("other.rs"), "pub fn other() {}\n").unwrap();
    git(repo, &["add", "other.rs"]);
    git(repo, &["commit", "-q", "-m", OTHER]);
    let foreign_tip = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["push", "-q", "origin", LANE]);
    git(repo, &["checkout", "-q", "main"]);

    let cas_dir = repo.join(".cas");
    std::fs::create_dir_all(&cas_dir).unwrap();
    env.set("CAS_ROOT", &cas_dir);
    env.set("CAS_FACTORY_MODE", "1");
    env.set("XDG_CONFIG_HOME", env.home().join(".config"));
    std::fs::write(
        cas_dir.join("config.toml"),
        format!(
            "[project]\ncanonical_id=\"{PROJECT}\"\n[factory]\nartifacts_root={:?}\n[verification]\nenabled=false\n",
            repo.join("durable-artifacts").display().to_string()
        ),
    )
    .unwrap();
    open_store(&cas_dir).unwrap().init().unwrap();
    open_rule_store(&cas_dir).unwrap().init().unwrap();
    open_skill_store(&cas_dir).unwrap().init().unwrap();
    open_worktree_store(&cas_dir).unwrap().init().unwrap();
    open_task_store(&cas_dir).unwrap().init().unwrap();
    let worker_path = cas_dir.join("worktrees").join(WORKER);
    std::fs::create_dir_all(worker_path.parent().unwrap()).unwrap();
    git(repo, &["worktree", "add", "-q", worker_path.to_str().unwrap(), LANE]);
    std::fs::create_dir_all(worker_path.join(".cas")).unwrap();
    std::fs::write(
        worker_path.join(".cas/config.toml"),
        format!("[project]\ncanonical_id=\"{PROJECT}\"\n"),
    )
    .unwrap();
    let agents = open_agent_store(&cas_dir).unwrap();
    agents.init().unwrap();
    agents
        .register(&Agent::new_with_role(
            "nocode-worker-session".into(),
            WORKER.into(),
            AgentRole::Worker,
        ))
        .unwrap();
    agents
        .register(&Agent::new_with_role(
            "nocode-supervisor-session".into(),
            "supervisor".into(),
            AgentRole::Supervisor,
        ))
        .unwrap();
    let worker = CasCore::with_daemon(cas_dir.clone(), None, None);
    worker.set_agent_id_for_testing("nocode-worker-session".into());
    let supervisor = CasCore::with_daemon(cas_dir, None, None);
    supervisor.set_agent_id_for_testing("nocode-supervisor-session".into());
    Fixture {
        dir,
        _origin: origin,
        worker: CasService::new(worker, None),
        supervisor: CasService::new(supervisor, None),
        foreign_tip,
    }
}

fn assigned(id: &str, task_type: TaskType, target: &str) -> Task {
    let mut task = Task::new(id.into(), format!("no-code {id}"));
    task.task_type = task_type;
    task.status = TaskStatus::InProgress;
    task.assignee = Some(WORKER.into());
    task.risk = vec![TaskRisk::None];
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: format!("project:{PROJECT}"),
        target_branch: target.into(),
    });
    task
}

fn claim(f: &Fixture, id: &str) {
    open_agent_store(&f.cas_dir())
        .unwrap()
        .try_claim(id, "nocode-worker-session", 1800, Some("assigned"))
        .unwrap();
}

/// The task closed on its proof and nothing of the lane was attributed to it.
fn assert_closed_without_lane(f: &Fixture, id: &str, response: &str) {
    let task = f.task(id);
    assert_eq!(task.status, TaskStatus::Closed, "{response}");
    assert!(
        !response.contains("MERGE REQUIRED") && !response.contains("DELIVERY BRANCH UNRESOLVED"),
        "{response}"
    );
    assert_ne!(
        task.deliverables.factory_branch_anchor.as_deref(),
        Some(f.foreign_tip.as_str()),
        "another task's delivery must never become this task's anchor"
    );
    assert!(
        task.deliverables.parked_branch.is_none(),
        "{:?}",
        task.deliverables.parked_branch
    );
}

/// GH #1133: a deploy-only task targeting `main`, started on a lane whose tip
/// is another task's delivery. The worker declares no-code at close.
#[tokio::test]
async fn deploy_task_closes_on_no_code_proof_over_a_foreign_lane_gh1133() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let id = "cas-nc33";
    f.put(&assigned(id, TaskType::Task, "main"));
    claim(&f, id);
    let response = call(
        &f.worker,
        serde_json::json!({
            "action": "close", "id": id, "reason": "Deployed the monthly release",
            "execution_note": "no-code", "external_ref": PROOF,
        }),
    )
    .await;
    assert_closed_without_lane(&f, id, &response);
    let task = f.task(id);
    assert_eq!(task.execution_note.as_deref(), Some("no-code"));
    assert_eq!(task.external_ref.as_deref(), Some(PROOF));
    assert!(task.notes.contains(PROOF), "{}", task.notes);
}

/// GH #1147: a no-code spike created under a code epic inherits the epic's
/// WorkTarget. The worker's close must not park it for the epic branch.
#[tokio::test]
async fn no_code_spike_under_a_code_epic_closes_without_parking_gh1147() {
    let mut env = TestEnvGuard::temp_home();
    let epic_branch = "epic/nocode-spikes";
    let f = fixture(&mut env, epic_branch);
    let mut epic = Task::new("cas-nc47-epic".into(), "code epic".into());
    epic.task_type = TaskType::Epic;
    epic.status = TaskStatus::InProgress;
    epic.branch = Some(epic_branch.into());
    epic.risk = vec![TaskRisk::None];
    epic.deliverables.work_target = Some(WorkTarget {
        repo_selector: format!("project:{PROJECT}"),
        target_branch: epic_branch.into(),
    });
    f.put(&epic);
    let id = "cas-nc47";
    let mut spike = assigned(id, TaskType::Spike, epic_branch);
    spike.execution_note = Some("no-code".into());
    spike.external_ref = Some("artifact:spike-findings".into());
    f.put(&spike);
    open_task_store(&f.cas_dir())
        .unwrap()
        .add_dependency(&Dependency::new(
            id.into(),
            epic.id.clone(),
            DependencyType::ParentChild,
        ))
        .unwrap();
    claim(&f, id);
    let response = call(
        &f.worker,
        serde_json::json!({"action": "close", "id": id, "reason": "Findings recorded"}),
    )
    .await;
    assert_closed_without_lane(&f, id, &response);
}

/// GH #1151: a no-code chore on a lane ending in another task's delivery,
/// already stuck from an earlier park that anchored that delivery. The
/// supervisor clears the code target and the worker's re-close succeeds; an
/// artifact-only twin closes by supervisor evidence_only with a `branch:`
/// reference and no commits of its own.
#[tokio::test]
async fn no_code_chore_on_a_foreign_lane_closes_after_target_clear_and_by_evidence_gh1151() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");

    let id = "cas-nc51";
    let mut chore = assigned(id, TaskType::Chore, "main");
    chore.execution_note = Some("no-code".into());
    chore.external_ref = Some("art-4b583703".into());
    // What a pre-fix close left behind: parked on the lane, anchored to the
    // other task's delivery.
    chore.status = TaskStatus::AwaitingMerge;
    chore.deliverables.parked_branch = Some(LANE.into());
    chore.deliverables.factory_branch_anchor = Some(f.foreign_tip.clone());
    f.put(&chore);
    claim(&f, id);
    let cleared = call(
        &f.supervisor,
        serde_json::json!({
            "action": "update", "id": id, "proof_scope_fix": true, "target_repo": "",
            "reason": "staging cleanup is external-only work",
        }),
    )
    .await;
    assert!(f.task(id).deliverables.work_target.is_none(), "{cleared}");
    let response = call(
        &f.worker,
        serde_json::json!({"action": "close", "id": id, "reason": "Staging cleaned up"}),
    )
    .await;
    assert_closed_without_lane(&f, id, &response);

    let twin = "cas-nc52";
    let mut spike = assigned(twin, TaskType::Spike, "main");
    spike.execution_note = Some("no-code".into());
    spike.external_ref = Some("art-5597".into());
    f.put(&spike);
    claim(&f, twin);
    let proof = f.artifact(twin);
    let response = call(
        &f.supervisor,
        serde_json::json!({
            "action": "close", "id": twin, "reason": "Spike findings retained as artifacts",
            "evidence_only": true, "evidence_only_artifact_path": proof,
            "evidence_only_reference": format!("branch:{LANE}"),
        }),
    )
    .await;
    assert_closed_without_lane(&f, twin, &response);
    let closed = f.task(twin);
    let evidence = closed.deliverables.evidence_only.as_ref().expect("evidence recorded");
    assert!(evidence.paths.is_empty(), "{:?}", evidence.paths);
    assert_ne!(evidence.commit_sha, f.foreign_tip);
}

/// A no-code declaration does not hide this task's own commits: a lane commit
/// naming the task, or an unnamed one that may be its work (cas-2387), keeps
/// the lane measured, and it must merge.
#[tokio::test]
async fn no_code_task_with_possible_own_lane_commits_still_requires_merge() {
    for (id, subject) in [
        ("cas-nc99", "docs(cas-nc99): deploy runbook"),
        ("cas-nc98", "deploy runbook"),
    ] {
        let mut env = TestEnvGuard::temp_home();
        let f = fixture(&mut env, "main");
        let worker_path = f.cas_dir().join("worktrees").join(WORKER);
        std::fs::write(worker_path.join("runbook.md"), "deploy runbook\n").unwrap();
        git(&worker_path, &["add", "runbook.md"]);
        git(&worker_path, &["commit", "-q", "-m", subject]);
        git(&worker_path, &["push", "-q", "origin", LANE]);
        f.put(&assigned(id, TaskType::Task, "main"));
        claim(&f, id);
        let response = call(
            &f.worker,
            serde_json::json!({
                "action": "close", "id": id, "reason": "Deployed",
                "execution_note": "no-code", "external_ref": PROOF,
            }),
        )
        .await;
        assert!(
            response.contains("MERGE REQUIRED") || response.contains("DELIVERY BRANCH UNRESOLVED"),
            "{subject}: {response}"
        );
        assert_ne!(f.task(id).status, TaskStatus::Closed, "{subject}");
    }
}
