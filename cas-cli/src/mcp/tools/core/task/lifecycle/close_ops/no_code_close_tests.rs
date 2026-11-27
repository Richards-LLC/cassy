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

/// cas-3d72: throwaway native-proof workflows are explicitly non-delivery;
/// neither their stale anchor nor the worker's next task may become a merge.
#[tokio::test]
async fn never_merge_workflow_proof_closes_without_foreign_delivery_cas_3d72() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let repo = f.dir.path();
    let id = "cas-3d72";
    let proof_branch = "factory/native-macos-proof";
    let standby_branch = "factory/native-macos-standby";
    // Allocating this task's lane inherits the preceding task's content;
    // its only unmerged subject must remain cas-a0b1, never this task's.
    let task_lane = format!("{LANE}-{id}");
    git(repo, &["branch", &task_lane, LANE]);
    assert_eq!(git(repo, &["log", "--format=%s", &format!("main..{task_lane}")]), OTHER);
    git(repo, &["checkout", "-q", "-b", proof_branch, "main"]);
    std::fs::create_dir_all(repo.join(".github/workflows")).unwrap();
    std::fs::write(repo.join(".github/workflows/proof.yml"), "name: native proof\n").unwrap();
    git(repo, &["add", ".github/workflows/proof.yml"]);
    git(repo, &["commit", "-q", "-m", "test(cas-3d72): throwaway native proof"]);
    let proof_tip = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["push", "-q", "origin", proof_branch]);
    git(repo, &["checkout", "-q", "-b", standby_branch, "main"]);
    std::fs::create_dir_all(repo.join(".github/workflows")).unwrap();
    std::fs::write(repo.join(".github/workflows/proof.yml"), "name: unused standby\n").unwrap();
    git(repo, &["add", ".github/workflows/proof.yml"]);
    git(repo, &["commit", "-q", "-m", "test(cas-3d72): unused proof standby"]);
    let stale_anchor = git(repo, &["rev-parse", "HEAD"]);
    let product_branch = "factory/contaminated-proof";
    git(repo, &["checkout", "-q", "-b", product_branch, proof_branch]);
    std::fs::write(repo.join("product.rs"), "pub fn product() {}\n").unwrap();
    git(repo, &["add", "product.rs"]);
    git(repo, &["commit", "-q", "-m", "fix(cas-3d72): product content"]);
    // Never-merge workflow proofs still inspect intermediate commits, even
    // though ordinary evidence deliveries are judged by their net diff.
    git(repo, &["rm", "product.rs"]);
    git(repo, &["commit", "-q", "-m", "revert(cas-3d72): product content"]);
    git(repo, &["checkout", "-q", "main"]);

    let mut task = assigned(id, TaskType::Task, "main");
    task.status = TaskStatus::Blocked;
    task.deliverables.factory_branch_anchor = Some(stale_anchor.clone());
    f.put(&task);
    claim(&f, id);
    let mut foreign = assigned("cas-a0b1", TaskType::Bug, "main");
    foreign.status = TaskStatus::AwaitingMerge;
    foreign.deliverables.factory_branch_anchor = Some(f.foreign_tip.clone());
    f.put(&foreign);
    let artifact = f.artifact(id);
    let request = serde_json::json!({
        "action": "close", "id": id,
        "reason": "Native proof accepted; workflow branches are never merged",
        "evidence_only": true, "evidence_only_artifact_path": artifact,
        "evidence_only_reference": format!("never-merge:branch:{proof_branch}"),
    });
    let refused = call(&f.worker, request.clone()).await;
    assert!(refused.contains("EVIDENCE ONLY CLOSE REJECTED"), "{refused}");
    assert!(refused.contains("supervisor"), "{refused}");
    assert_eq!(f.task(id).status, TaskStatus::Blocked, "{refused}");

    // Exercise the ordinary close handler too: its hook must not advertise
    // this shared checkout's other-task HEAD as a recovery commit receipt.
    let refused = call(&f.worker, serde_json::json!({
        "action": "close", "id": id, "reason": "Native proof recorded",
    })).await;
    assert!(refused.contains("PRE-CLOSE HOOK CONTEXT REJECTED"), "{refused}");
    assert!(
        !refused.contains(&format!("commit_receipt={}", f.foreign_tip)),
        "a foreign shared HEAD must not be offered as this task's receipt: {refused}"
    );

    task.execution_note = Some("no-code".into());
    task.external_ref = Some(PROOF.into());
    f.put(&task);
    let mut contaminated = request.clone();
    contaminated["evidence_only_reference"] =
        format!("never-merge:branch:{product_branch}").into();
    let refused = call(&f.supervisor, contaminated).await;
    assert!(refused.contains("EVIDENCE ONLY CLOSE REJECTED"), "{refused}");
    assert!(refused.contains("product.rs"), "{refused}");
    assert_eq!(f.task(id).status, TaskStatus::Blocked, "{refused}");
    let response = call(&f.supervisor, request).await;
    assert_closed_without_lane(&f, id, &response);
    let closed = f.task(id);
    let evidence = closed.deliverables.evidence_only.as_ref().expect("evidence recorded");
    assert!(evidence.paths.is_empty(), "{:?}", evidence.paths);
    assert_eq!(evidence.commit_sha, git(repo, &["rev-parse", "origin/main"]));
    assert_eq!(evidence.base_sha, evidence.commit_sha);
    assert!(evidence.reference.starts_with("never-merge:branch:"));
    assert!(closed.deliverables.factory_branch_anchor.is_none());
    assert_eq!(closed.assignee.as_deref(), Some(WORKER));
    assert!(closed.notes.contains(&proof_tip), "{}", closed.notes);
    assert!(closed.notes.contains(&stale_anchor), "{}", closed.notes);
    assert_eq!(git(repo, &["rev-parse", LANE]), f.foreign_tip);
    let unchanged = f.task(&foreign.id);
    assert_eq!(unchanged.status, foreign.status);
    assert_eq!(unchanged.assignee, foreign.assignee);
    assert_eq!(unchanged.deliverables.factory_branch_anchor, foreign.deliverables.factory_branch_anchor);
}

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
        .filter_map(|content| match content {
            rmcp::model::ContentBlock::Text(text) => Some(text.text),
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
            "[project]\ncanonical_id=\"{PROJECT}\"\n[factory]\nartifacts_root={:?}\n[verification]\nenabled=false\n[qa]\nmode='per_task'\n",
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

/// GH #1577/#1430: losing an implementer cannot waive a task's review,
/// and an explicit waiver cannot replace an outstanding proof cycle.
#[tokio::test]
async fn retired_assignee_keeps_required_verification_cas_8826() {
    use cas_store::VerificationStore;
    use cas_types::{Verification, VerificationProvenance, VerificationStatus};
    let mut env = TestEnvGuard::temp_home();
    for identity in ["unregistered", "inactive", "unassigned", "active"] {
        for (prior, override_requested) in [("none", false), ("none", true), ("pending", true), ("error", true), ("legacy-error", true)] {
            let f = fixture(&mut env, "main");
            let mut config = crate::config::Config::load(&f.cas_dir()).unwrap();
            config.set("verification.enabled", "true").unwrap();
            config.save(&f.cas_dir()).unwrap();
            env.set("CAS_AGENT_ROLE", "supervisor");
            let id = "cas-8826";
            let mut task = assigned(id, TaskType::Bug, "main");
            task.priority = cas_types::Priority(0);
            task.execution_note = Some("no-code".into());
            task.external_ref = Some(PROOF.into());
            f.put(&task);
            let verifications = cas_store::SqliteVerificationStore::open(&f.cas_dir()).unwrap();
            if prior == "legacy-error" {
                let mut error = Verification::new("ver-prior-error".into(), id.into());
                error.status = VerificationStatus::Error;
                error.summary = "Legacy verifier requires supervisor recovery".into();
                error.provenance = VerificationProvenance::System;
                cas_store::add_system_verification(&f.cas_dir(), &error).unwrap();
            } else if prior != "none" {
                let dispatch = cas_store::create_verification_dispatch_bound(
                    &f.cas_dir(), id, "nocode-supervisor-session", "nocode-supervisor-session",
                    &cas_types::VerificationProofBoundary::task(),
                    chrono::Utc::now() + chrono::Duration::minutes(10), false,
                ).unwrap();
                if prior == "error" {
                    let mut error = Verification::new("ver-prior-error".into(), id.into());
                    error.status = VerificationStatus::Error;
                    error.summary = "Verifier could not establish the delivery boundary".into();
                    error.provenance = VerificationProvenance::SupervisorDirect;
                    error.agent_id = Some("nocode-supervisor-session".into());
                    error.issuer_agent_id = error.agent_id.clone();
                    error.dispatch_id = Some(dispatch.id.clone());
                    verifications.add(&error).unwrap();
                    let conn = rusqlite::Connection::open(f.cas_dir().join("cas.db")).unwrap();
                    cas_store::resolve_verification_dispatch_with_conn(
                        &conn, &dispatch.id, "nocode-supervisor-session", None, true,
                    ).unwrap();
                }
            }
            let agents = open_agent_store(&f.cas_dir()).unwrap();
            match identity {
                "unregistered" => agents.unregister("nocode-worker-session").unwrap(),
                "inactive" => {
                    let mut worker = agents.get("nocode-worker-session").unwrap();
                    worker.status = cas_types::AgentStatus::Shutdown;
                    agents.update(&worker).unwrap();
                }
                "unassigned" => { task.assignee = None; f.put(&task); }
                _ => {}
            }
            let identity_request = serde_json::from_value(serde_json::json!({"id": id})).unwrap();
            let state = f.supervisor.inner.compute_verification_skip_reason(&task, &identity_request);
            match identity {
                "unregistered" => assert_eq!(state, VerificationSkipReason::AssigneeUnregistered { assignee: WORKER.into() }),
                "inactive" => assert!(matches!(state, VerificationSkipReason::AssigneeInactive { minutes_stale: Some(_) })),
                "unassigned" => assert_eq!(state, VerificationSkipReason::NoAssignee),
                _ => assert_eq!(state, VerificationSkipReason::None),
            }
            assert!(!state.is_skip(), "identity alone is not waiver authority: {state:?}");
            let response = call(&f.supervisor, serde_json::json!({
                "action":"close", "id":id, "reason":"External repair measured",
                "supervisor_override":override_requested,
            })).await;
            let closed = f.task(id);
            let latest = verifications.get_latest_for_task(id).unwrap();
            let waived = override_requested && prior == "none";
            assert_eq!(closed.status == TaskStatus::Closed, waived,
                "identity={identity} prior={prior} override={override_requested}: {response}");
            if waived {
                let row = latest.expect("explicit waiver is durable");
                assert_eq!(row.status, VerificationStatus::Skipped);
                assert_eq!(row.agent_id.as_deref(), Some("nocode-supervisor-session"));
            } else {
                assert!(latest.is_none_or(|row| row.status != VerificationStatus::Skipped), "{response}");
                assert!(cas_store::get_latest_verification_dispatch(&f.cas_dir(), id).unwrap().is_some(), "{response}");
            }
            if prior == "error" || prior == "legacy-error" {
                assert_eq!(verifications.get("ver-prior-error").unwrap().status, VerificationStatus::Error);
            }
        }
    }
}

fn claim(f: &Fixture, id: &str) {
    open_agent_store(&f.cas_dir())
        .unwrap()
        .try_claim(id, "nocode-worker-session", 1800, Some("assigned"))
        .unwrap();
}

fn issue_delivery(f: &Fixture, id: &str, target: &str) -> (PathBuf, String) {
    let checkout = f.cas_dir().join("worktrees").join(WORKER);
    let lane = format!("{LANE}-{id}");
    git(&checkout, &["checkout", "-q", "-b", &lane, target]);
    let mut task = assigned(id, TaskType::Bug, target);
    task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
        work_target: task.deliverables.work_target.clone().unwrap(),
        assignee: WORKER.into(), branch: lane.clone(),
        base_sha: git(&checkout, &["rev-parse", "HEAD"]),
    });
    f.put(&task);
    claim(f, id);
    let agents = open_agent_store(&f.cas_dir()).unwrap();
    let mut agent = agents.get("nocode-worker-session").unwrap();
    agent.metadata.insert("clone_path".into(), checkout.display().to_string());
    agents.update(&agent).unwrap();
    (checkout, lane)
}

async fn park_for_repeat_review_gh2373(f: &Fixture, id: &str, target: &str, train: Option<&Task>) {
    let (checkout, lane) = issue_delivery(f, id, target);
    if let Some(train) = train {
        open_task_store(&f.cas_dir()).unwrap().add_dependency(&Dependency::new(
            id.into(), train.id.clone(), DependencyType::ParentChild,
        )).unwrap();
    }
    std::fs::write(checkout.join("feature.rs"), "pub fn feature() {}\n").unwrap();
    git(&checkout, &["add", "feature.rs"]);
    git(&checkout, &["commit", "-qm", &format!("feat({id}): implement transition")]);
    git(&checkout, &["push", "-q", "origin", &lane]);
    let response = call(&f.worker, serde_json::json!({
        "action":"close", "id":id, "reason":"Transition implemented",
    })).await;
    assert_eq!(f.task(id).status, TaskStatus::AwaitingMerge, "{response}");
}

async fn reject_repeat_review_gh2373(f: &Fixture, id: &str, finding: &str) -> serde_json::Value {
    let owner = "nocode-supervisor-session";
    let dispatch = cas_store::create_verification_dispatch_bound(
        &f.cas_dir(), id, owner, owner, &cas_types::VerificationProofBoundary::task(),
        chrono::Utc::now() + chrono::Duration::minutes(10), false,
    ).unwrap();
    let request = serde_json::json!({
        "action":"add", "task_id":id, "dispatch_id":dispatch.id, "status":"rejected", "summary":finding,
    });
    let response = f.supervisor.verification(Parameters(serde_json::from_value(request.clone()).unwrap())).await.unwrap();
    assert_ne!(response.is_error, Some(true), "{response:?}");
    let verdict = cas_store::get_verification_for_dispatch(&f.cas_dir(), &dispatch.id).unwrap().unwrap();
    assert_eq!(verdict.status, cas_types::VerificationStatus::Rejected);
    request
}

async fn link_repeat_review_gh2373(f: &Fixture, id: &str, path: &Path) -> String {
    f.supervisor.artifact(Parameters(serde_json::from_value(serde_json::json!({
        "action":"publish", "task_id":id, "path":path,
    })).unwrap())).await.unwrap();
    let artifact = cas_store::SqliteArtifactStore::open(&f.cas_dir()).unwrap()
        .list_for_task(id).unwrap().into_iter().find(|artifact|
            Some(artifact.name.as_str()) == path.file_name().and_then(std::ffi::OsStr::to_str)
        ).unwrap();
    call(&f.supervisor, serde_json::json!({"action":"notes", "id":id, "note_type":"decision",
        "notes":format!("whole-flow review: {}", artifact.id)})).await;
    artifact.id
}

#[tokio::test]
async fn repeat_review_every_entry_refuses_g172_gh2373() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let id = "cas-2373";
    env.set("CAS_AGENT_ROLE", "worker");
    park_for_repeat_review_gh2373(&f, id, "main", None).await;
    env.set("CAS_AGENT_ROLE", "supervisor");
    reject_repeat_review_gh2373(&f, id, "The abort transition is unreachable").await;
    let agents = open_agent_store(&f.cas_dir()).unwrap();
    let session = "repeat-review-queue";
    for actor in ["nocode-supervisor-session", "nocode-worker-session"] {
        let mut registration = agents.get(actor).unwrap();
        registration.factory_session = Some(session.into());
        registration.metadata.insert("clone_path".into(), f.dir.path().display().to_string());
        agents.update(&registration).unwrap();
    }
    let mut predecessor = assigned("cas-2373-predecessor", TaskType::Feature, "main");
    f.put(&predecessor);
    let core = CasCore::with_daemon(f.cas_dir(), None, None);
    core.set_agent_id_for_testing("nocode-supervisor-session".into());
    reject_repeat_review_gh2373(&f, id, "A stale queue can claim the successor").await;
    let legacy = cas_store::worker_task_queue_store::SqliteWorkerTaskQueueStore::open(&f.cas_dir()).unwrap();
    let mut cursor = legacy.get(session, WORKER).unwrap();
    cursor.current_task = Some(predecessor.id.clone());
    cursor.current_started = true;
    cursor.pending = vec![id.into()];
    legacy.save(session, WORKER, &mut cursor).unwrap();
    predecessor.status = TaskStatus::AwaitingMerge;
    f.put(&predecessor);
    open_agent_store(&f.cas_dir()).unwrap().register(&Agent::new_with_role(
        "replacement-session".into(), "replacement".into(), AgentRole::Worker,
    )).unwrap();
    let request = serde_json::json!({"action":"request_changes", "id":id, "reason":"Correct the mismatches"});
    let assignment = serde_json::json!({"action":"update", "id":id, "assignee":"replacement"});
    let transfer = serde_json::json!({"action":"transfer", "id":id, "to_agent":"replacement", "supervisor_override":true});
    let parked = f.task(id);
    let mut failures = Vec::new();
    for (role, status, attempt) in [
        ("supervisor", TaskStatus::AwaitingMerge, request.clone()),
        ("supervisor", TaskStatus::AwaitingMerge, assignment),
        ("supervisor", TaskStatus::AwaitingMerge, transfer),
        ("worker", TaskStatus::Open, serde_json::json!({"action":"claim", "id":id})),
        ("worker", TaskStatus::Open, serde_json::json!({"action":"start", "id":id})),
        ("supervisor", TaskStatus::Blocked, serde_json::json!({"action":"reopen", "id":id, "reason":"Reopen repair"})),
        ("supervisor", TaskStatus::AwaitingMerge, serde_json::json!({"action":"update", "id":id, "proof_scope_fix":true, "proof_targets":["transition suite"], "reason":"Widen proof"})),
        ("supervisor", TaskStatus::Blocked, serde_json::json!({"action":"update", "id":id, "status":"open"})),
        ("supervisor", TaskStatus::Open, serde_json::json!({"action":"update", "id":id, "status":"in_progress"})),
    ] {
        let mut before = parked.clone();
        before.status = status;
        f.put(&before);
        env.set("CAS_AGENT_ROLE", role);
        let service = if role == "worker" { &f.worker } else { &f.supervisor };
        let result = service.task(Parameters(serde_json::from_value(attempt.clone()).unwrap())).await;
        let after = f.task(id);
        if !result.as_ref().is_err_and(|error| crate::gates::codes_in(&error.message).contains(&"G-172"))
            || after.status != before.status || after.assignee != before.assignee
        {
            failures.push(format!("{attempt}: {result:?}; state={:?}/{:?}", after.status, after.assignee));
        }
    }
    env.set("CAS_AGENT_ROLE", "supervisor");
    let mut queued = parked.clone();
    queued.status = TaskStatus::Open;
    queued.assignee = None;
    queued.execution_note = Some("no-code".into());
    queued.deliverables.factory_branch_start = None;
    f.put(&queued);
    let queues = cas_store::worker_task_queue_store::SqliteWorkerTaskQueueStore::open(&f.cas_dir()).unwrap();
    let cursor = queues.get(session, WORKER).unwrap();
    assert_eq!(cursor.current_task.as_deref(), Some(predecessor.id.as_str()));
    assert_eq!(cursor.pending, vec![id.to_string()]);
    let lease = || agents.get_lease(id).unwrap().map(|lease| (lease.agent_id, lease.epoch, lease.status));
    let before_lease = lease();
    let repeat_hold = |error: &McpError| {
        error.data.as_ref().is_some_and(|data| data["rule_id"] == "repeat_review_required")
            && crate::gates::codes_in(&error.message).contains(&"G-172")
    };
    let queue_result = core.advance_queue_after_delivery(&predecessor.id).await;
    let after = f.task(id);
    if !queue_result.as_ref().is_err_and(repeat_hold)
        || after.status != queued.status || after.assignee != queued.assignee
        || queues.get(session, WORKER).unwrap() != cursor || lease() != before_lease
    {
        failures.push(format!("ordered queue: {queue_result:?}; state={:?}/{:?}", after.status, after.assignee));
    }

    queued.status = TaskStatus::InProgress;
    queued.assignee = Some(WORKER.into());
    f.put(&queued);
    let mut recovery_cursor = queues.get(session, WORKER).unwrap();
    recovery_cursor.current_task = Some(id.into());
    recovery_cursor.current_started = false;
    recovery_cursor.pending.clear();
    recovery_cursor.dispatch_from = Some(predecessor.id.clone());
    recovery_cursor.starting = true;
    recovery_cursor.dispatch_revision = recovery_cursor.revision + 1;
    queues.save(session, WORKER, &mut recovery_cursor).unwrap();
    let before_retry = lease();
    let retry = core.advance_queue_after_delivery(&predecessor.id).await;
    let after_retry = lease();
    if !retry.as_ref().is_err_and(repeat_hold)
        || after_retry != before_retry || queues.get(session, WORKER).unwrap() != recovery_cursor
    {
        failures.push(format!("queue retry: {retry:?}; lease {before_retry:?} -> {after_retry:?}"));
    }
    if let Some((holder, _, cas_types::LeaseStatus::Active)) = after_retry {
        agents.release_lease(id, &holder).unwrap();
    }

    env.set("CAS_AGENT_ROLE", "worker");
    let before_claim = lease();
    let before_cursor = queues.get(session, WORKER).unwrap();
    let claim = f.worker.task(Parameters(serde_json::from_value(serde_json::json!({
        "action":"claim", "id":id,
    })).unwrap())).await;
    if !claim.as_ref().is_err_and(repeat_hold)
        || lease() != before_claim || queues.get(session, WORKER).unwrap() != before_cursor
    {
        failures.push(format!("manual claim: {claim:?}; lease {before_claim:?} -> {:?}", lease()));
    }
    assert!(failures.is_empty(), "{failures:#?}");
    let mut cursor = cursor;
    cursor.revision = before_cursor.revision;
    queues.save(session, WORKER, &mut cursor).unwrap();
    f.put(&parked);

    env.set("CAS_AGENT_ROLE", "supervisor");
    call(&f.supervisor, serde_json::json!({"action":"notes", "id":id, "note_type":"decision",
        "notes":"whole-flow review: art-missing"})).await;
    let refusal = f.supervisor.task(Parameters(serde_json::from_value(request.clone()).unwrap()))
        .await.unwrap_err();
    assert!(crate::gates::codes_in(&refusal.message).contains(&"G-172"), "{refusal:?}");
    assert_eq!(refusal.data.as_ref().unwrap()["rule_id"], "repeat_review_required");

    let receipt = f.artifact(id);
    link_repeat_review_gh2373(&f, id, &receipt).await;
    let refusal = f.supervisor.task(Parameters(serde_json::from_value(request.clone()).unwrap()))
        .await.unwrap_err();
    assert!(crate::gates::codes_in(&refusal.message).contains(&"G-172"), "{refusal:?}");

    let path = receipt.with_file_name("whole-flow-review.md");
    std::fs::write(&path, "| Actor | Operation | Design | Observed |\n| --- | --- | --- | --- |\n| Owner | Abort | Can abort an open run | Unreachable |\n| Queue | Claim | Only current generation | Stale generation admitted |\n\n## Mismatches\n\n- Owner cannot abort an open run.\n- Queue admits a stale generation.\n").unwrap();
    let review = link_repeat_review_gh2373(&f, id, &path).await;
    call(&f.supervisor, request).await;
    assert_eq!(f.task(id).status, TaskStatus::Open);
    let checkout = f.cas_dir().join("worktrees").join(WORKER);
    let mut registration = agents.get("nocode-worker-session").unwrap();
    registration.metadata.insert("clone_path".into(), checkout.display().to_string());
    agents.update(&registration).unwrap();
    env.set("CAS_AGENT_ROLE", "worker");
    f.worker.coordination(Parameters(serde_json::from_value(serde_json::json!({
        "action":"inbox_poll",
    })).unwrap())).await.unwrap();
    let started = call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
    assert_eq!(f.task(id).status, TaskStatus::InProgress);
    assert!(started.contains(&review));
}

#[tokio::test]
async fn repeat_rejection_holds_automatic_conflict_return_gh2373() {
    let mut env = TestEnvGuard::temp_home();
    let target = "epic/conflict";
    let f = fixture(&mut env, target);
    let id = "cas-2373-conflict";
    env.set("CAS_AGENT_ROLE", "worker");
    park_for_repeat_review_gh2373(&f, id, target, None).await;
    env.set("CAS_AGENT_ROLE", "supervisor");
    reject_repeat_review_gh2373(&f, id, "The abort transition is unreachable").await;
    reject_repeat_review_gh2373(&f, id, "A stale queue can claim the successor").await;

    let parked = f.task(id);
    assert!(!parked.deliverables.merge_conflicted);
    let agents = open_agent_store(&f.cas_dir()).unwrap();
    let lease_snapshot = || serde_json::to_value(agents.get_lease(id).unwrap()).unwrap();
    let parked_lease = lease_snapshot();
    let start = || f.worker.task(Parameters(serde_json::from_value(serde_json::json!({
        "action":"start", "id":id,
    })).unwrap()));
    env.set("CAS_AGENT_ROLE", "worker");
    let clean_refusal = start().await.unwrap_err();
    assert!(crate::gates::codes_in(&clean_refusal.message).contains(&"G-05"), "{clean_refusal:?}");
    env.set("CAS_AGENT_ROLE", "supervisor");
    let repo = f.dir.path();
    git(repo, &["checkout", "-q", target]);
    std::fs::write(repo.join("feature.rs"), "pub fn different_feature() {}\n").unwrap();
    git(repo, &["add", "feature.rs"]);
    git(repo, &["commit", "-qm", "conflicting target change"]);
    git(repo, &["push", "-q", "origin", target]);
    let refusal = f.supervisor.factory(Parameters(serde_json::from_value(serde_json::json!({
        "action":"worktree_merge", "id":format!("{LANE}-{id}"), "task_id":id,
    })).unwrap())).await.unwrap_err();
    assert!(crate::gates::codes_in(&refusal.message).contains(&"G-172"), "{refusal:?}");
    assert_eq!(refusal.data.as_ref().and_then(|data| data["rule_id"].as_str()),
        Some("repeat_review_required"), "{refusal:?}");
    let held = f.task(id);
    assert_eq!(held.status, TaskStatus::AwaitingMerge);
    assert_eq!(held.deliverables.factory_branch_anchor, parked.deliverables.factory_branch_anchor);
    assert!(!held.deliverables.merge_conflicted);

    env.set("CAS_AGENT_ROLE", "worker");
    for stored_conflict in [false, true] {
        let mut before = held.clone();
        if stored_conflict {
            before.deliverables.merge_conflicted = true;
            f.put(&before);
        }
        let before = serde_json::to_value(f.task(id)).unwrap();
        let result = start().await;
        let after = serde_json::to_value(f.task(id)).unwrap();
        assert!(result.as_ref().is_err_and(|refusal| {
            crate::gates::codes_in(&refusal.message).contains(&"G-172")
                && refusal.data.as_ref().and_then(|data| data["rule_id"].as_str())
                    == Some("repeat_review_required")
        }), "stored_conflict={stored_conflict}: {result:?}");
        assert_eq!(after, before);
        assert_eq!(lease_snapshot(), parked_lease);
    }
}

#[tokio::test]
async fn detached_task_keeps_recorded_start_gh2373() {
    let mut env = TestEnvGuard::temp_home();
    for (keep, fail_first_notice) in [(false, true), (false, false), (true, false)] {
        let target = "epic/release-train";
        let f = fixture(&mut env, target);
        let mut config = crate::config::Config::load(&f.cas_dir()).unwrap();
        config.set("factory.release_train", "true").unwrap();
        config.save(&f.cas_dir()).unwrap();
        let store = open_task_store(&f.cas_dir()).unwrap();
        let mut train = Task::new("cas-2374-train".into(), "Release train".into());
        train.task_type = TaskType::Epic;
        train.labels.push("release-train".into());
        train.branch = Some(target.into());
        train.deliverables.work_target = assigned("unused", TaskType::Task, "main").deliverables.work_target;
        f.put(&train);
        let id = "cas-2374";
        env.set("CAS_AGENT_ROLE", "worker");
        park_for_repeat_review_gh2373(&f, id, target, Some(&train)).await;
        env.set("CAS_AGENT_ROLE", "supervisor");
        reject_repeat_review_gh2373(&f, id, "The abort transition is unreachable").await;
        assert_eq!(store.get_parent_epic(id).unwrap().unwrap().id, train.id);
        if keep {
            call(&f.supervisor, serde_json::json!({"action":"notes", "id":id, "note_type":"decision",
                "notes":format!("keep release train: {}: Required correction for this cut", train.id)})).await;
        }
        let queue = crate::store::open_prompt_queue_store(&f.cas_dir()).unwrap();
        let conn = rusqlite::Connection::open(f.cas_dir().join("cas.db")).unwrap();
        if fail_first_notice {
            conn.execute_batch("CREATE TRIGGER fail_repeat_notice BEFORE INSERT ON prompt_queue
                WHEN NEW.source LIKE 'supervisor-notice:%' AND
                (NEW.summary = 'repeat-review-train-detached' OR instr(NEW.prompt, 'G-172') > 0)
                BEGIN SELECT RAISE(ABORT, 'injected repeat notice delivery error'); END;").unwrap();
        }
        let verdict = reject_repeat_review_gh2373(&f, id, "A stale queue can claim the successor").await;
        assert_eq!(store.get_parent_epic(id).unwrap().map(|parent| parent.id), keep.then(|| train.id.clone()));
        assert_eq!(f.task(id).deliverables.work_target.unwrap().target_branch, if keep { target } else { "main" });
        let detach_notices = || queue.peek_all(50).unwrap().into_iter().filter(|row|
            row.summary.as_deref() == Some("repeat-review-train-detached") && row.target == "supervisor").collect::<Vec<_>>();
        let assert_notices = || {
            let notices = detach_notices();
            assert_eq!(notices.len(), usize::from(!keep));
            assert!(notices.iter().all(|row| crate::supervisor_notices::is_action_envelope(&row.prompt)));
            let notice = crate::store::open_supervisor_queue_store(&f.cas_dir()).unwrap()
                .get_by_transition_key(&format!("repeat-review:{id}:{}",
                    cas_store::SqliteVerificationStore::open(&f.cas_dir()).unwrap()
                        .latest_repeated_rejection(id).unwrap().unwrap().0)).unwrap().unwrap();
            assert!(notice.prompt_delivered_at.is_some());
            let payload: serde_json::Value = serde_json::from_str(&notice.payload).unwrap();
            assert_eq!(payload["context"]["train_id"].as_str(), (!keep).then_some(train.id.as_str()));
        };
        if fail_first_notice {
            assert!(detach_notices().is_empty());
            conn.execute_batch("DROP TRIGGER fail_repeat_notice").unwrap();
            for _ in 0..2 {
                let retry = f.supervisor.verification(Parameters(serde_json::from_value(verdict.clone()).unwrap())).await.unwrap();
                assert_ne!(retry.is_error, Some(true), "{retry:?}");
                assert_notices();
            }
        } else {
            assert_notices();
        }
        let parked = f.task(id);
        let start = parked.deliverables.factory_branch_start.unwrap();
        let work_target = parked.deliverables.work_target.unwrap();
        assert_eq!(start.work_target, work_target);
        if !keep {
            assert!(parked.deliverables.work_target_pinned);
            assert!(parked.deliverables.prior_work_targets.iter().any(|previous| previous.target_branch == target));
            let path = f.artifact(id).with_file_name("whole-flow-review.md");
            std::fs::write(&path, "| Actor | Operation | Design | Observed |\n| --- | --- | --- | --- |\n| Owner | Abort | Allowed | Unreachable |\n\n## Mismatches\n- Abort is unreachable.\n").unwrap();
            link_repeat_review_gh2373(&f, id, &path).await;
            call(&f.supervisor, serde_json::json!({"action":"request_changes", "id":id, "reason":"Correct the mismatches"})).await;
            let restarted = CasCore::with_daemon(f.cas_dir(), None, None);
            let returned = f.task(id);
            assert_eq!(returned.status, TaskStatus::Open);
            assert!(restarted.test_started_task_merge_source(f.dir.path(), &returned, &start.branch).unwrap());
        }
    }
}

/// Ordinary parks must make all close-reason proof portable before handing
/// the delivery to a verifier on another machine (cas-dae1).
#[tokio::test(flavor = "multi_thread")]
async fn ordinary_park_requires_every_cited_local_proof_cas_dae1() {
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use wiremock::matchers::{body_partial_json, method, path};
    let mut env = TestEnvGuard::temp_home();
    for state in ["upload-failed", "missing", "published", "cited-copy"] {
        let f = fixture(&mut env, "main");
        let server = MockServer::start().await;
        std::fs::write(f.cas_dir().join("cloud.json"), serde_json::json!({
            "endpoint": server.uri(), "token": "fixture-token"
        }).to_string()).unwrap();
        let id = "cas-dae1";
        let (checkout, lane) = issue_delivery(&f, id, "main");
        std::fs::write(checkout.join(".gitignore"), ".cas/\ndurable-artifacts/\ntarget/\n").unwrap();
        std::fs::write(checkout.join("own.md"), "delivery\n").unwrap();
        git(&checkout, &["add", ".gitignore", "own.md"]);
        git(&checkout, &["commit", "-qm", "docs(cas-dae1): owned delivery"]);
        git(&checkout, &["push", "-q", "origin", &lane]);
        let capture = crate::mcp::tools::core::task::lifecycle::qa_evidence_gate::task_evidence_dir(
            &f.cas_dir(), &f.task(id),
        ).unwrap().join("capture.txt");
        let log = checkout.join("target/run.log");
        let mut files = vec![capture.clone(), log.clone()];
        if state == "cited-copy" {
            // The capture is not in the ledger, but the close reason cites it.
            // Its presence in the directory scan must not leave it optional.
            let root = capture.parent().unwrap();
            files.extend([root.join("LEDGER.md"), root.join("base.txt")]);
        }
        for (index, file) in files.iter().enumerate() {
            if state != "missing" {
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                let body = if file.file_name().unwrap() == "LEDGER.md" {
                    "schema: 2\ncovers: cas-dae1\ncells=1; PASS=1; FAIL=0; NOT EXERCISED=0; NOT A ROUTE=0\nlabels: fixture=1\n\n| id | cell | expected | observed | verdict | label | evidence path | defect task |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n| M01 | probe | ok [source: own.md:1] | terminal: ok | PASS | fixture | base.txt | - |\n".to_string()
                } else { format!("proof {index}\n") };
                std::fs::write(file, body).unwrap();
            }
            let name = file.file_name().unwrap().to_str().unwrap();
            let response = if index < 2 && matches!(state, "upload-failed" | "cited-copy") {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(201).set_body_json(serde_json::json!({
                    "artifact_id": format!("cloud-{index}"),
                    "upload_url": format!("{}/blob/{index}", server.uri()), "required_headers": {}
                }))
            };
            Mock::given(method("POST")).and(path("/api/artifacts/begin"))
                .and(body_partial_json(serde_json::json!({"name": name})))
                .respond_with(response).mount(&server).await;
            Mock::given(method("PUT")).and(path(format!("/blob/{index}")))
                .respond_with(ResponseTemplate::new(200)).mount(&server).await;
            Mock::given(method("POST")).and(path(format!("/api/artifacts/cloud-{index}/complete")))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"url": null})))
                .mount(&server).await;
        }
        let reply = call(&f.worker, serde_json::json!({
            "action": "close", "id": id,
            "reason": format!("PASS proof=`{}` log=target/run.log", capture.display()),
        })).await;
        let task = f.task(id);
        if state == "published" {
            assert_eq!(task.status, TaskStatus::AwaitingMerge, "{reply}");
            let records = cas_store::SqliteArtifactStore::open(&f.cas_dir())
                .unwrap().list_for_task(id).unwrap();
            assert_eq!(records.len(), 2);
            assert!(records.iter().all(|artifact| artifact.status == "committed"));
            let manifest: serde_json::Value = task.notes.lines().find_map(|line| {
                serde_json::from_str(line.strip_prefix("EVIDENCE_SYNC: ")?).ok()
            }).expect("portable paths recorded for the peer");
            assert_eq!(manifest["files"].as_array().unwrap().len(), 2);
        } else {
            assert_eq!(task.status, TaskStatus::InProgress, "{state}: {reply}");
            assert!(reply.contains("EVIDENCE PUBLICATION INCOMPLETE"), "{reply}");
            for name in ["capture.txt", "run.log"] {
                assert!(reply.contains(name), "unpublished {name}: {reply}");
            }
            assert!(task.deliverables.parked_branch.is_none());
        }
    }
}

/// cas-7c35 (GH #2322, #2371): a re-park through the real close handler on a
/// task that already carries an older evidence summary replaces that summary,
/// keeps unrelated history, and never requires a file for prose naming target.
#[tokio::test(flavor = "multi_thread")]
async fn repark_replaces_older_evidence_summary_cas_7c35() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let id = "cas-7c35";
    let (checkout, lane) = issue_delivery(&f, id, "main");
    std::fs::write(checkout.join("own.md"), "delivery\n").unwrap();
    git(&checkout, &["add", "own.md"]);
    git(&checkout, &["commit", "-qm", "docs(cas-7c35): owned delivery"]);
    git(&checkout, &["push", "-q", "origin", &lane]);
    let dir = crate::mcp::tools::core::task::lifecycle::qa_evidence_gate::task_evidence_dir(
        &f.cas_dir(), &f.task(id),
    ).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("capture.txt"), b"").unwrap();
    let mut task = f.task(id);
    task.notes = format!("{}\n\n[2026-11-23 23:00] keep this decision\n\n[2026-11-23 23:01] Evidence synced for review on the merge owner\nEVIDENCE_SYNC: {{\"machine_id\":\"m\",\"root\":\"/old-root\",\"files\":[],\"strict\":false}}\nFiles: 0; optional not published: 0; required not published: 0; excluded trees: 0. Full report: evidence-sync-report.json", task.notes);
    f.put(&task);
    assert!(!checkout.join("target").exists());
    let reply = call(&f.worker, serde_json::json!({
        "action": "close", "id": id,
        "reason": "PASS docs only: merged epic target into the delivery; target/owner unchanged.",
    })).await;
    let task = f.task(id);
    assert_eq!(task.status, TaskStatus::AwaitingMerge, "{reply}");
    assert_eq!(task.notes.lines().filter(|line| line.starts_with("EVIDENCE_SYNC: ")).count(), 1, "{}", task.notes);
    assert!(!task.notes.contains("/old-root"), "{}", task.notes);
    assert!(task.notes.contains("keep this decision"), "{}", task.notes);
}

/// cas-fd1c (GH #2212): a no-code delivery with a portable external_ref, whose
/// bundle files an earlier publication already receipted, parks through the
/// real close handler without uploading those files again, and the nested raw
/// and private request tree nothing cites is listed, never published.
#[tokio::test(flavor = "multi_thread")]
async fn no_code_park_reuses_published_bundle_and_skips_nested_raw_files_gh2212() {
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let server = MockServer::start().await;
    std::fs::write(f.cas_dir().join("cloud.json"), serde_json::json!({
        "endpoint": server.uri(), "token": "fixture-token"
    }).to_string()).unwrap();
    let id = "cas-2212";
    let (checkout, lane) = issue_delivery(&f, id, "main");
    // The no-code declaration and its portable proof are recorded on the task.
    let mut declared = f.task(id);
    declared.execution_note = Some("no-code".into());
    declared.external_ref = Some(PROOF.into());
    f.put(&declared);
    std::fs::write(checkout.join(".gitignore"), ".cas/\ndurable-artifacts/\ntarget/\n").unwrap();
    std::fs::write(checkout.join("runbook.md"), "deploy runbook\n").unwrap();
    git(&checkout, &["add", ".gitignore", "runbook.md"]);
    git(&checkout, &["commit", "-qm", "docs(cas-2212): deploy runbook"]);
    git(&checkout, &["push", "-q", "origin", &lane]);

    let dir = crate::mcp::tools::core::task::lifecycle::qa_evidence_gate::task_evidence_dir(
        &f.cas_dir(), &f.task(id),
    ).unwrap();
    let write = |name: &str, body: &str| {
        let file = dir.join(name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, body).unwrap();
        file
    };
    let published = [write("captures.log", "captures: ok\n"), write("private-map.json", "{\"map\":1}\n")];
    let summary = write("summary.md", "deployed\n");
    for index in 0..40 {
        write(&format!("raw/requests/{index:04}.json"), &format!("{{\"request\":{index}}}\n"));
    }
    write("raw/private/map.json", "{\"private\":true}\n");

    // Cloud accepts each file once by name; the bundle files are published
    // before the close, so durable committed receipts exist for them.
    for (index, name) in ["captures.log", "private-map.json", "summary.md"].into_iter().enumerate() {
        Mock::given(method("POST")).and(path("/api/artifacts/begin"))
            .and(body_partial_json(serde_json::json!({"name": name})))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "artifact_id": format!("cloud-{index}"),
                "upload_url": format!("{}/blob/{index}", server.uri()), "required_headers": {}
            }))).mount(&server).await;
        Mock::given(method("PUT")).and(path(format!("/blob/{index}")))
            .respond_with(ResponseTemplate::new(200)).mount(&server).await;
        Mock::given(method("POST")).and(path(format!("/api/artifacts/cloud-{index}/complete")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"url": null})))
            .mount(&server).await;
    }
    let root = f.cas_dir();
    let (task_id, bundle) = (id.to_string(), published.to_vec());
    let receipts: Vec<String> = tokio::task::spawn_blocking(move || {
        let config = crate::config::Config::load(&root).unwrap_or_default();
        let base = crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
        let store = cas_store::SqliteArtifactStore::open(&root).unwrap();
        let context = crate::artifacts::PublishContext {
            store: &store,
            roots: crate::artifacts::paths::PublishRoots::new(&root, &base, &task_id),
            task_id: task_id.clone(), cloud: crate::artifacts::cloud_client(&root),
        };
        bundle.iter().map(|file| {
            let artifact = crate::artifacts::publish(&context, file).unwrap().artifact;
            assert_eq!(artifact.status, "committed");
            artifact.id
        }).collect()
    }).await.unwrap();
    let before = server.received_requests().await.unwrap().len();

    let reply = call(&f.worker, serde_json::json!({
        "action": "close", "id": id,
        "reason": format!("PASS bundle `{}` summary `{}`", published[0].display(), summary.display()),
    })).await;
    let task = f.task(id);
    assert_eq!(task.status, TaskStatus::AwaitingMerge, "{reply}");

    let uploads: Vec<serde_json::Value> = server.received_requests().await.unwrap()[before..].iter()
        .filter(|request| request.url.path() == "/api/artifacts/begin")
        .map(|request| serde_json::from_slice(&request.body).unwrap()).collect();
    assert_eq!(uploads.len(), 1, "only the unpublished summary uploads: {uploads:?}");
    assert_eq!(uploads[0]["name"], "summary.md");
    let store = cas_store::SqliteArtifactStore::open(&f.cas_dir()).unwrap();
    let records = store.list_for_task(id).unwrap();
    assert_eq!(records.len(), 3, "the bundle receipts are reused, the raw tree is not published");
    for (file, receipt) in published.iter().zip(&receipts) {
        let name = file.file_name().unwrap().to_str().unwrap();
        assert_eq!(records.iter().filter(|record| record.name == name).map(|record| &record.id).collect::<Vec<_>>(), [receipt]);
    }
    assert!(records.iter().all(|record| !record.name.ends_with(".json") || record.name == "private-map.json"));
    let manifest: serde_json::Value = task.notes.lines().find_map(|line| {
        serde_json::from_str(line.strip_prefix("EVIDENCE_SYNC: ")?).ok()
    }).expect("portable paths recorded for the peer");
    let mut files: Vec<&str> = manifest["files"].as_array().unwrap().iter()
        .map(|file| file["name"].as_str().unwrap()).collect();
    files.sort();
    assert_eq!(files, ["captures.log", "private-map.json", "summary.md"]);
}

#[tokio::test]
async fn own_contextual_task_mention_can_park_gh1496() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let id = "cas-eddf";
    let (checkout, lane) = issue_delivery(&f, id, "main");
    std::fs::write(checkout.join("own.rs"), "pub fn own() {}\n// collection exemption\n").unwrap();
    git(&checkout, &["add", "own.rs"]);
    git(&checkout, &["commit", "-qm", "fix(cas-eddf): own delivery"]);
    std::fs::write(checkout.join("own.rs"), "pub fn own() {}\n").unwrap();
    git(&checkout, &["commit", "-qam", "chore: leave cas-3094 typed collection exemptions to epic owner"]);
    git(&checkout, &["push", "-q", "origin", &lane]);
    let response = call(&f.worker, serde_json::json!({
        "action": "close", "id": id, "reason": "Own delivery ready",
    })).await;
    assert_eq!(f.task(id).status, TaskStatus::AwaitingMerge, "{response}");
    assert!(!response.contains("foreign commit"), "{response}");
}

#[tokio::test]
async fn integrated_squash_receipt_survives_large_source_history_gh1454() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let id = "cas-5a43";
    let (checkout, lane) = issue_delivery(&f, id, "main");
    for _ in 0..201 {
        git(&checkout, &["commit", "-q", "--allow-empty", "-m", "inherited history"]);
    }
    std::fs::write(checkout.join("own.rs"), "pub fn own() {}\n").unwrap();
    git(&checkout, &["add", "own.rs"]);
    git(&checkout, &["commit", "-qm", "fix(cas-5a43): own delivery"]);
    let anchor = git(&checkout, &["rev-parse", "HEAD"]);
    git(&checkout, &["push", "-q", "origin", &lane]);
    let repo = f.dir.path();
    git(repo, &["merge", "-q", "--squash", &lane]);
    git(repo, &["commit", "-qm", "fix(cas-5a43): integrated squash"]);
    let receipt = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["push", "-q", "origin", "main"]);
    let mut task = f.task(id);
    task.status = TaskStatus::AwaitingMerge;
    task.deliverables.parked_branch = Some(lane);
    task.deliverables.factory_branch_anchor = Some(anchor);
    f.put(&task);
    let response = call(&f.supervisor, serde_json::json!({
        "action": "close", "id": id, "reason": "Recover integrated squash",
        "commit_receipt": receipt,
    })).await;
    assert_eq!(f.task(id).status, TaskStatus::Closed, "{response}");
}

#[tokio::test]
async fn explicit_staging_scope_fix_survives_epic_child_close_gh1452() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "staging");
    let repo = f.dir.path();
    git(repo, &["branch", "epic/child-target", "staging"]);
    git(repo, &["push", "-q", "origin", "epic/child-target"]);
    let id = "cas-ea89";
    let (checkout, lane) = issue_delivery(&f, id, "epic/child-target");
    let mut epic = assigned("cas-cca7", TaskType::Epic, "staging");
    epic.assignee = None;
    epic.branch = Some("epic/child-target".into());
    f.put(&epic);
    open_task_store(&f.cas_dir()).unwrap().add_dependency(&Dependency::new(
        id.into(), epic.id, DependencyType::ParentChild,
    )).unwrap();
    std::fs::write(checkout.join("own.rs"), "pub fn own() {}\n").unwrap();
    git(&checkout, &["add", "own.rs"]);
    git(&checkout, &["commit", "-qm", "fix(cas-ea89): own delivery"]);
    git(&checkout, &["push", "-q", "origin", &lane]);
    git(repo, &["checkout", "-q", "staging"]);
    git(repo, &["merge", "-q", "--squash", &lane]);
    git(repo, &["commit", "-qm", "fix(cas-ea89): integrated squash"]);
    let receipt = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["push", "-q", "origin", "staging"]);
    let updated = call(&f.supervisor, serde_json::json!({
        "action": "update", "id": id, "proof_scope_fix": true,
        "target_branch": "staging", "reason": "Delivery intentionally integrated into staging",
    })).await;
    assert!(updated.contains("staging"), "{updated}");
    let response = call(&f.worker, serde_json::json!({
        "action": "close", "id": id, "reason": "Delivery integrated into chosen target",
        "commit_receipt": receipt,
    })).await;
    assert_eq!(f.task(id).status, TaskStatus::Closed, "{response}");
}

/// The task closed on its proof and nothing of the lane was attributed to it.
fn assert_closed_without_lane(f: &Fixture, id: &str, response: &str) {
    let task = f.task(id);
    assert_eq!(task.status, TaskStatus::Closed, "{response}");
    assert!(
        !response.lines().any(crate::mcp::tools::core::task::lifecycle::close_ops::gate_preflight::is_merge_handoff) && !response.contains("DELIVERY BRANCH UNRESOLVED"),
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

/// cas-255b: a worker-created helper never allocated a lane or delivered code.
/// HEAD happens to be a previous UI merge; that is not the helper's delivery.
#[tokio::test]
async fn empty_helper_closes_without_qa_or_factory_branch_cas_255b() {
    let mut env = TestEnvGuard::temp_home();
    for lane_exists in [true, false] {
        let f = fixture(&mut env, "main");
        let repo = f.dir.path();
        let worker_path = f.cas_dir().join("worktrees").join(WORKER);
        std::fs::write(worker_path.join("app.css"), "body { color: red; }\n").unwrap();
        git(&worker_path, &["add", "app.css"]);
        git(&worker_path, &["commit", "-q", "-m", OTHER]);
        git(repo, &["merge", "-q", "--no-ff", LANE, "-m", "Merge earlier UI delivery"]);
        git(repo, &["push", "-q", "origin", "main"]);
        if !lane_exists {
            git(repo, &["worktree", "remove", "--force", worker_path.to_str().unwrap()]);
            git(repo, &["branch", "-D", LANE]);
            git(repo, &["push", "-q", "origin", "--delete", LANE]);
        }
        let id = "cas-255b";
        let mut helper = assigned(id, TaskType::Task, "main");
        helper.title = "Own the iOS simulator".into();
        f.put(&helper);
        claim(&f, id);
        let mut config = crate::config::Config::load(&f.cas_dir()).unwrap();
        config.set("verification.enabled", "true").unwrap();
        config.save(&f.cas_dir()).unwrap();
        // A real exact-task verdict still binds this undeclared helper;
        // only a fresh no-delivery declaration may waive its delivery gates.
        let dispatch = cas_store::create_verification_dispatch_bound(
            &f.cas_dir(), id, "nocode-worker-session", "nocode-supervisor-session",
            &cas_types::VerificationProofBoundary::task(),
            chrono::Utc::now() + chrono::Duration::minutes(10), false,
        ).unwrap();
        let pending = call(
            &f.worker,
            serde_json::json!({
                "action": "close", "id": id, "reason": "Simulator released",
                "no_delivery": true,
            }),
        ).await;
        assert!(pending.contains("VERIFICATION"), "{pending}");
        assert_eq!(f.task(id).status, TaskStatus::InProgress, "{pending}");
        let mut verdict = cas_types::Verification::approved(
            "ver-empty-helper".into(), id.into(), "Simulator released; no code delivered".into(),
        );
        verdict.provenance = cas_types::VerificationProvenance::SupervisorDirect;
        verdict.agent_id = Some("nocode-supervisor-session".into());
        verdict.issuer_agent_id = verdict.agent_id.clone();
        verdict.dispatch_id = Some(dispatch.id.clone());
        use cas_store::VerificationStore;
        cas_store::SqliteVerificationStore::open(&f.cas_dir()).unwrap().add(&verdict).unwrap();
        let conn = rusqlite::Connection::open(f.cas_dir().join("cas.db")).unwrap();
        cas_store::resolve_verification_dispatch_with_conn(
            &conn, &dispatch.id, "nocode-supervisor-session", None, true,
        ).unwrap();
        let ordinary = call(
            &f.worker,
            serde_json::json!({"action": "close", "id": id, "reason": "Simulator released"}),
        ).await;
        assert_ne!(
            f.task(id).status, TaskStatus::Closed,
            "empty measurements and an earlier declaration must not waive an ordinary close: {ordinary}"
        );
        let missing_reason = call(
            &f.worker,
            serde_json::json!({"action": "close", "id": id, "no_delivery": true}),
        ).await;

        assert_ne!(f.task(id).status, TaskStatus::Closed, "{missing_reason}");
        let conflict = call(&f.worker, serde_json::json!({
            "action": "close", "id": id, "reason": "Simulator released",
            "no_delivery": true, "execution_note": "no-code", "external_ref": PROOF,
        })).await;

        assert!(f.task(id).execution_note.is_none());

        // Deleted branches or an empty current range cannot erase a prior
        // delivery. Restore the original row so the valid retry is unchanged.
        let original = f.task(id);
        let mut historical = original.clone();
        historical.deliverables.historical_factory_branch_anchors.push(f.foreign_tip.clone());
        f.put(&historical);
        let recorded = call(&f.worker, serde_json::json!({
            "action": "close", "id": id, "reason": "Simulator released", "no_delivery": true,
        })).await;
        assert_ne!(f.task(id).status, TaskStatus::Closed, "{recorded}");
        f.put(&original);
        let broken_ref = repo.join(".git/refs/heads").join(format!("{LANE}-{id}"));
        std::fs::create_dir_all(broken_ref.parent().unwrap()).unwrap();
        std::fs::write(&broken_ref, "unreadable-object\n").unwrap();
        let unknown = call(&f.worker, serde_json::json!({
            "action": "close", "id": id, "reason": "Simulator released", "no_delivery": true,
        })).await;
        assert_ne!(f.task(id).status, TaskStatus::Closed, "unknown Git state is not absence: {unknown}");
        std::fs::remove_file(broken_ref).unwrap();
        if lane_exists {
            std::fs::write(worker_path.join("app.css"), "body { color: purple; }\n").unwrap();
            let dirty = call(&f.worker, serde_json::json!({
                "action": "close", "id": id, "reason": "Simulator released", "no_delivery": true,
            })).await;
            assert!(dirty.contains("UNCOMMITTED WORK"), "{dirty}");
            assert_ne!(f.task(id).status, TaskStatus::Closed, "{dirty}");
            git(&worker_path, &["checkout", "--", "app.css"]);
        }
        let response = call(
            &f.worker,
            serde_json::json!({
                "action": "close", "id": id, "reason": "Simulator released",
                "no_delivery": true,
            }),
        ).await;
        assert_closed_without_lane(&f, id, &response);
        assert!(f.task(id).deliverables.factory_branch_anchor.is_none());
        assert!(f.task(id).execution_note.is_none());
        assert!(f.task(id).external_ref.is_none());
        assert!(f.task(id).notes.contains("No delivery declared"));
        assert!(f.task(id).notes.contains("nocode-worker-session"));
        assert!(cas_store::list_qa_passes(&f.cas_dir(), id).unwrap().is_empty());

        // An integrated own commit is still delivery even with an empty tree
        // delta. Put it outside the recent-50 subject scan to exercise the
        // full task-life attribution rather than just recent branch claims.
        if lane_exists {
            let empty_id = "cas-a94f-empty";
            f.put(&assigned(empty_id, TaskType::Task, "main"));
            claim(&f, empty_id);
            git(&worker_path, &["commit", "-q", "--allow-empty", "-m", "fix(cas-a94f-empty): task work"]);
            for _ in 0..51 {
                git(&worker_path, &["commit", "-q", "--allow-empty", "-m", OTHER]);
            }
            git(repo, &["merge", "-q", "--no-ff", LANE, "-m", "Merge empty helper delivery"]);
            git(repo, &["push", "-q", "origin", "main"]);
            let empty = call(&f.worker, serde_json::json!({
                "action": "close", "id": empty_id, "reason": "Helper finished", "no_delivery": true,
            })).await;
            assert!(empty.contains("NO DELIVERY"), "{empty}");
            assert_eq!(f.task(empty_id).status, TaskStatus::InProgress, "{empty}");
        }

        if lane_exists {
            // An unnamed unmerged commit is possible task work, even before any
            // delivery metadata is recorded. It must still owe its UI bundle.
            let code_id = "cas-255c";
            f.put(&assigned(code_id, TaskType::Task, "main"));
            claim(&f, code_id);
            std::fs::write(worker_path.join("app.css"), "body { color: blue; }\n").unwrap();
            git(&worker_path, &["add", "app.css"]);
            git(&worker_path, &["commit", "-q", "-m", "Change stylesheet"]);
            // Identify the unnamed work by its per-task lane so ordinary
            // close reaches QA instead of the unrelated branch-identity gate.
            let code_lane = format!("{LANE}-{code_id}");
            git(repo, &["branch", &code_lane, LANE]);
            let declared = call(
                &f.worker,
                serde_json::json!({
                    "action": "close", "id": code_id, "reason": "Stylesheet changed",
                    "no_delivery": true,
                }),
            ).await;
            assert!(declared.contains("NO DELIVERY"), "{declared}");
            assert_eq!(f.task(code_id).status, TaskStatus::InProgress, "{declared}");
            let refused = call(
                &f.worker,
                serde_json::json!({"action": "close", "id": code_id, "reason": "Stylesheet changed"}),
            ).await;
            assert!(refused.contains("QA evidence bundle"), "{refused}");
            assert_eq!(f.task(code_id).status, TaskStatus::InProgress, "{refused}");

            // A clean, merged local lane must not hide the remote's newer
            // delivery. Keep the real pushed commit while restoring local.
            git(&worker_path, &["push", "-q", "origin", LANE]);
            git(&worker_path, &["reset", "--hard", "main"]);
            git(repo, &["branch", "-f", &code_lane, "main"]);
            let remote = call(&f.worker, serde_json::json!({
                "action": "close", "id": code_id, "reason": "Stylesheet changed", "no_delivery": true,
            })).await;
            assert!(remote.contains("NO DELIVERY"), "{remote}");
            assert_ne!(f.task(code_id).status, TaskStatus::Closed, "{remote}");
        }

        // A declaration cannot silently retire an existing independent QA
        // round, even if no code evidence is currently available.
        let qa_id = "cas-a94f-qa";
        f.put(&assigned(qa_id, TaskType::Task, "main"));
        claim(&f, qa_id);
        let now = chrono::Utc::now();
        cas_store::open_qa_pass(&f.cas_dir(), &cas_store::NewQaPass {
            task_id: qa_id, implementer_agent_id: "nocode-worker-session",
            branch: LANE, bound_head: &f.foreign_tip,
            deadline_at: now + chrono::Duration::minutes(10), max_rounds: 2,
        }, now).unwrap();
        let qa = call(&f.worker, serde_json::json!({
            "action": "close", "id": qa_id, "reason": "Simulator released", "no_delivery": true,
        })).await;
        assert!(qa.contains("NO DELIVERY") && qa.contains("QA round"), "{qa}");
        assert_eq!(f.task(qa_id).status, TaskStatus::InProgress, "{qa}");
    }
}

/// GH #1370: held A and later integrated C belong to separate deliveries;
/// neither can turn B's empty task lane into a code delivery.
#[tokio::test]
async fn no_delivery_isolates_held_and_integrated_sibling_lanes_gh1370() {
    let mut env = TestEnvGuard::temp_home();
    for (own_commit, legacy_start) in [(false, false), (false, true), (true, false)] {
        let f = fixture(&mut env, "main");
        let repo = f.dir.path();
        let worker_path = f.cas_dir().join("worktrees").join(WORKER);
        let mut held = assigned("cas-a0b1", TaskType::Bug, "main");
        held.status = TaskStatus::AwaitingMerge;
        held.deliverables.parked_branch = Some(LANE.into());
        held.deliverables.factory_branch_anchor = Some(f.foreign_tip.clone());
        f.put(&held);

        let id = "cas-b3d4";
        let own_lane = format!("{LANE}-{id}");
        git(&worker_path, &["checkout", "-q", "-b", &own_lane, "main"]);
        let agents = open_agent_store(&f.cas_dir()).unwrap();
        let mut agent = agents.get("nocode-worker-session").unwrap();
        agent.metadata.insert("clone_path".into(), worker_path.display().to_string());
        agents.update(&agent).unwrap();
        let mut helper = assigned(id, TaskType::Task, "main");
        helper.status = TaskStatus::Open;
        f.put(&helper);
        let started = call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
        assert_eq!(f.task(id).status, TaskStatus::InProgress, "{started}");
        assert_eq!(f.task(id).deliverables.factory_branch_start.as_ref().unwrap().branch, own_lane);
        if legacy_start {
            // Older starts recorded the reusable lane before isolation.
            // The allocated task lane still defines B's actual delivery.
            let mut recorded = f.task(id);
            recorded.deliverables.factory_branch_start.as_mut().unwrap().branch = LANE.into();
            f.put(&recorded);
        }

        let c_lane = format!("{LANE}-cas-c3d4");
        git(repo, &["checkout", "-q", "-b", &c_lane, "main"]);
        std::fs::write(repo.join("sibling.rs"), "pub fn sibling() {}\n").unwrap();
        git(repo, &["add", "sibling.rs"]);
        git(repo, &["commit", "-q", "-m", "fix(cas-c3d4): separate delivery"]);
        let c_tip = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["push", "-q", "origin", &c_lane]);
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["merge", "-q", "--no-ff", &c_lane, "-m", "Integrate C"]);
        git(repo, &["push", "-q", "origin", "main"]);
        let mut delivered = assigned("cas-c3d4", TaskType::Bug, "main");
        delivered.status = TaskStatus::Closed;
        delivered.deliverables.parked_branch = Some(c_lane.clone());
        delivered.deliverables.factory_branch_anchor = Some(c_tip.clone());
        f.put(&delivered);
        if own_commit {
            std::fs::write(worker_path.join("own.rs"), "pub fn own() {}\n").unwrap();
            git(&worker_path, &["add", "own.rs"]);
            git(&worker_path, &["commit", "-q", "-m", "fix(cas-b3d4): own delivery"]);
        }
        let response = call(&f.worker, serde_json::json!({
            "action":"close", "id":id, "reason":"Diagnosis finished", "no_delivery":true,
        })).await;
        if own_commit {
            assert!(response.contains("NO DELIVERY CLOSE REJECTED"), "{response}");
            assert_eq!(f.task(id).status, TaskStatus::InProgress, "{response}");
        } else {
            assert_closed_without_lane(&f, id, &response);
            assert!(f.task(id).deliverables.factory_branch_anchor.is_none());
        }
        assert_eq!(f.task(&held.id).status, TaskStatus::AwaitingMerge);
        assert_eq!(git(repo, &["rev-parse", LANE]), f.foreign_tip);
        assert_eq!(git(repo, &["rev-parse", &c_lane]), c_tip);
        assert_eq!(f.task(&delivered.id).status, TaskStatus::Closed);
    }
}

/// GH #1939 (cas-aac3, cas-f45f): both retarget paths, `task update epic=`
/// and a `dep_add` parent link (cas-361e's case), carry an unchanged empty
/// source to the new target, so its no-delivery close is not refused G-47.
#[tokio::test]
async fn epic_retarget_rebinds_unchanged_started_source_before_no_delivery_close() {
    let mut env = TestEnvGuard::temp_home();
    for via_dep_add in [false, true] {
    let f = fixture(&mut env, "main");
    let repo = f.dir.path();
    git(repo, &["checkout", "-q", "-b", "epic/next"]);
    git(repo, &["push", "-q", "origin", "epic/next"]);
    git(repo, &["checkout", "-q", "main"]);

    let tasks = open_task_store(&f.cas_dir()).unwrap();
    let target = |branch: &str| WorkTarget {
        repo_selector: format!("project:{PROJECT}"),
        target_branch: branch.into(),
    };
    for (id, branch) in [("cas-aac3-old", "main"), ("cas-aac3-new", "epic/next")] {
        let mut epic = Task::new(id.into(), format!("train {branch}"));
        epic.task_type = TaskType::Epic;
        epic.branch = Some(branch.into());
        epic.deliverables.work_target = Some(target("main"));
        tasks.add(&epic).unwrap();
    }
    let id = "cas-aac3-child";
    let mut helper = assigned(id, TaskType::Task, "main");
    helper.status = TaskStatus::Open;
    f.put(&helper);
    if !via_dep_add {
        tasks.add_dependency(&Dependency::new(
            id.into(), "cas-aac3-old".into(), DependencyType::ParentChild,
        )).unwrap();
    }
    let retarget = if via_dep_add {
        serde_json::json!({"action":"dep_add", "id":id, "to_id":"cas-aac3-new", "dep_type":"parent"})
    } else {
        serde_json::json!({"action":"update", "id":id, "epic":"cas-aac3-new"})
    };
    let agents = open_agent_store(&f.cas_dir()).unwrap();
    let mut worker = agents.get("nocode-worker-session").unwrap();
    worker.metadata.insert(
        "clone_path".into(),
        f.cas_dir().join("worktrees").join(WORKER).display().to_string(),
    );
    agents.update(&worker).unwrap();

    let started = call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
    let original = f.task(id).deliverables.factory_branch_start.unwrap();
    assert_eq!(original.work_target, target("main"), "{started}");
    let mut reassigned = f.task(id);
    reassigned.assignee = Some("another-worker".into());
    f.put(&reassigned);
    let refused = f.supervisor.task(Parameters(serde_json::from_value(retarget.clone()).unwrap())).await;
    assert!(refused.is_err(), "mismatched assignee must refuse the retarget");
    assert!(
        refused.as_ref().unwrap_err().message.contains("rebind_branch"),
        "mismatched source guidance names the supervisor recovery action: {refused:?}"
    );
    assert_ne!(tasks.get_parent_epic(id).unwrap().map(|epic| epic.id).as_deref(), Some("cas-aac3-new"), "{refused:?}");
    assert_eq!(f.task(id).deliverables.work_target, Some(target("main")), "{refused:?}");
    let mut restored = f.task(id);
    restored.assignee = Some(WORKER.into());
    f.put(&restored);

    let moved = call(&f.supervisor, retarget).await;
    let rebound = f.task(id);
    assert_eq!(rebound.deliverables.work_target, Some(target("epic/next")), "{moved}");
    assert_eq!(rebound.deliverables.factory_branch_start.as_ref().unwrap().work_target, target("epic/next"), "{moved}");
    assert!(rebound.notes.contains("DELIVERY BRANCH REBOUND"), "{moved}");

    let restarted = call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
    assert_eq!(f.task(id).deliverables.factory_branch_start.as_ref().unwrap().work_target, target("epic/next"), "{restarted}");
    let closed = call(&f.worker, serde_json::json!({
        "action":"close", "id":id, "reason":"No code was needed", "no_delivery":true,
    })).await;
    assert_closed_without_lane(&f, id, &closed);
    }
}

/// A derived empty ref cannot erase work on B's publicly recorded source.
#[tokio::test]
async fn no_delivery_keeps_generic_source_work_despite_empty_task_ref_gh1370() {
    let mut env = TestEnvGuard::temp_home();
    for remote_only in [false, true] {
        let f = fixture(&mut env, "main");
        let repo = f.dir.path();
        let worker_path = f.cas_dir().join("worktrees").join(WORKER);
        git(repo, &["merge", "-q", "--ff-only", LANE]);
        git(repo, &["push", "-q", "origin", "main"]);
        let agents = open_agent_store(&f.cas_dir()).unwrap();
        let mut agent = agents.get("nocode-worker-session").unwrap();
        agent.metadata.insert("clone_path".into(), worker_path.display().to_string());
        agents.update(&agent).unwrap();
        let id = "cas-b137";
        let mut task = assigned(id, TaskType::Task, "main");
        task.status = TaskStatus::Open;
        f.put(&task);
        let started = call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
        let source = f.task(id).deliverables.factory_branch_start.unwrap();
        assert_eq!(source.branch, LANE, "{started}");
        git(repo, &["branch", &format!("{LANE}-{id}"), &source.base_sha]);
        std::fs::write(worker_path.join("own.rs"), "pub fn own() {}\n").unwrap();
        git(&worker_path, &["add", "own.rs"]);
        git(&worker_path, &["commit", "-q", "-m", "fix(cas-b137): real generic-source work"]);
        git(&worker_path, &["push", "-q", "origin", LANE]);
        if remote_only {
            git(&worker_path, &["reset", "-q", "--hard", &source.base_sha]);
        }
        assert!(f.task(id).deliverables.factory_branch_anchor.is_none());
        let response = call(&f.worker, serde_json::json!({
            "action":"close", "id":id, "reason":"Claims no delivery", "no_delivery":true,
        })).await;
        assert!(response.contains("NO DELIVERY CLOSE REJECTED"), "remote_only={remote_only}: {response}");
        assert_eq!(f.task(id).status, TaskStatus::InProgress, "{response}");
        assert_eq!(f.task(id).deliverables.factory_branch_start.as_ref(), Some(&source));
    }
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

/// Every proof correction and reopen must survive both terminal race windows.
#[tokio::test]
async fn close_refuses_terminal_write_over_a_concurrent_proof_scope_fix_cas_c202() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    for after_read in [false, true] {
        for field in ["reopen", "proof_targets", "risk", "execution_note", "work_target"] {
            let id = format!("cas-c202-{field}-{after_read}");
            f.put(&assigned(&id, TaskType::Task, "main"));
            claim(&f, &id);
            let cas_dir = f.cas_dir();
            let changed_id = id.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            let hook = move || {
                let store = open_task_store(&cas_dir).unwrap();
                let loaded = store.get(&changed_id).unwrap();
                let mut corrected = loaded.clone();
                match field {
                    "reopen" => corrected.status = TaskStatus::Open,
                    "proof_targets" => corrected.proof_targets = vec!["pre_tool".into()],
                    "risk" => corrected.risk = vec![TaskRisk::Concurrency],
                    "execution_note" => corrected.execution_note = Some("test-first".into()),
                    "work_target" => corrected.deliverables.work_target.as_mut().unwrap().target_branch = "corrected".into(),
                    _ => unreachable!(),
                }
                corrected.notes.push_str("\n\nConcurrent correction");
                store.update_if_current(&corrected, &loaded).unwrap();
                tx.send(corrected).unwrap();
            };
            if after_read { lifecycle_race_hook::before_terminal_cas(&id, hook); }
            else { lifecycle_race_hook::before_terminal_write(&id, hook); }
            let response = call(&f.worker, serde_json::json!({
                "action": "close", "id": id, "reason": "Deployed the monthly release",
                "execution_note": "no-code", "external_ref": PROOF,
            })).await;
            let corrected = rx.try_recv().expect("race hook ran");
            let task = f.task(&id);
            assert_eq!(task.status, corrected.status, "{field}, after_read={after_read}: {response}");
            assert_eq!(task.proof_targets, corrected.proof_targets);
            assert_eq!(task.risk, corrected.risk);
            assert_eq!(task.execution_note, corrected.execution_note);
            assert_eq!(task.deliverables.work_target, corrected.deliverables.work_target);
            assert!(task.notes.contains("Concurrent correction"));
            assert!(response.contains("CLOSE REFUSED") && response.contains("Reload"), "{response}");
        }
    }
}

async fn note_only_close_race(rewrite: bool, after_read: bool) {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let id = "cas-3f81-race";
    let mut original = assigned(id, TaskType::Task, "main");
    original.notes = "[2026-10-21 14:17] Initial progress\n\nEarlier observation".into();
    f.put(&original);
    claim(&f, id);
    let cas_dir = f.cas_dir();
    let (notes_tx, notes_rx) = std::sync::mpsc::channel();
    let hook = move || {
        let store = open_task_store(&cas_dir).unwrap();
        if rewrite {
            let loaded = store.get(id).unwrap();
            let mut replaced = loaded.clone();
            replaced.notes = replaced.notes.replace("Initial progress", "Verified provenance");
            store.update_if_current(&replaced, &loaded).unwrap();
        }
        store.append_note(id, "[2026-10-21 14:18] Supervisor update\n\nSecond paragraph").unwrap();
        for _ in 0..2 {
            store.append_note(id, "[2026-10-21 14:19] Repeated observation").unwrap();
        }
        notes_tx.send(store.get(id).unwrap().notes).unwrap();
    };
    if after_read { lifecycle_race_hook::before_terminal_cas(id, hook); }
    else { lifecycle_race_hook::before_terminal_write(id, hook); }
    let response = call(&f.worker, serde_json::json!({
        "action": "close", "id": id, "reason": "Deployment independently verified",
        "execution_note": "no-code", "external_ref": PROOF,
    })).await;
    let closed = f.task(id);
    let persisted_notes = notes_rx.try_recv().expect("terminal race hook ran");
    assert_eq!(closed.status, TaskStatus::Closed, "note-only changes allow close: {response}");
    assert!(closed.notes.starts_with(&persisted_notes), "concurrent timeline lost or reordered: {}", closed.notes);
    assert_eq!(closed.notes.matches("Initial progress").count(), usize::from(!rewrite));
    assert_eq!(closed.notes.matches("Earlier observation").count(), 1);
    assert_eq!(closed.notes.matches("Supervisor update").count(), 1);
    assert_eq!(closed.notes.matches("Repeated observation").count(), 2);
    assert!(closed.notes[persisted_notes.len()..].contains("Deployment independently verified"), "close audit follows concurrent notes: {}", closed.notes);
}

#[tokio::test]
async fn close_preserves_note_rewrites_without_duplicate_history_cas_90ea() {
    for rewrite in [false, true] { note_only_close_race(rewrite, false).await; }
}

#[tokio::test]
async fn close_retries_a_note_in_the_terminal_write_gap_cas_90ea() {
    note_only_close_race(false, true).await;
}

/// GH #2277: a live pinned owner reviews a worker's artifact-only child;
/// the worker cannot finalize it and another supervisor cannot take it over.
/// GH #2284: the same handoff preserves a commitless no_delivery spike
/// without inventing a worker lane or asking for another verification.
#[tokio::test]
async fn worker_no_code_train_child_parks_for_live_owner_cas_141a() {
    let mut env = TestEnvGuard::temp_home();
    for no_delivery in [false, true] {
    let target = "epic/owner-review";
    let f = fixture(&mut env, target);
    let owner_id = "nocode-supervisor-session";
    let agents = open_agent_store(&f.cas_dir()).unwrap();
    let mut owner = agents.get(owner_id).unwrap();
    owner.pid = Some(std::process::id());
    owner.cc_session_id = Some("owner-session-preserved-after-clear".into());
    agents.update(&owner).unwrap();
    let mut epic = Task::new("cas-141a-train".into(), "owned train".into());
    epic.task_type = TaskType::Epic;
    epic.status = TaskStatus::InProgress;
    epic.branch = Some(target.into());
    epic.epic_verification_owner = Some(owner_id.into());
    f.put(&epic);
    let id = "cas-141a-child";
    let mut child = assigned(id, if no_delivery { TaskType::Spike } else { TaskType::Chore }, target);
    if no_delivery {
        let checkout = f.cas_dir().join("worktrees").join(WORKER);
        let source = format!("{LANE}-{id}");
        git(&checkout, &["checkout", "-q", "-b", &source, target]);
        git(f.dir.path(), &["branch", "-D", LANE]);
        git(f.dir.path(), &["push", "-q", "origin", "--delete", LANE]);
        child.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
            work_target: child.deliverables.work_target.clone().unwrap(),
            assignee: WORKER.into(), branch: source,
            base_sha: git(f.dir.path(), &["rev-parse", target]),
        });
        let mut config = crate::config::Config::load(&f.cas_dir()).unwrap();
        config.set("verification.enabled", "true").unwrap();
        config.save(&f.cas_dir()).unwrap();
        env.set("CAS_FACTORY_SUPERVISOR_CLI", "codex");
    } else {
        child.execution_note = Some("no-code".into());
    }
    f.put(&child);
    open_task_store(&f.cas_dir()).unwrap().add_dependency(&Dependency::new(
        id.into(), epic.id.clone(), DependencyType::ParentChild,
    )).unwrap();
    claim(&f, id);
    let mut request = serde_json::json!({
        "action": "close", "id": id, "reason": "Measurement recorded",
        "external_ref": PROOF,
    });
    let reviewed = if no_delivery {
        request.as_object_mut().unwrap().remove("external_ref");
        request["no_delivery"] = true.into();
        let pending = call(&f.worker, request.clone()).await;
        let dispatch = cas_store::get_latest_verification_dispatch(&f.cas_dir(), id)
            .unwrap().unwrap_or_else(|| panic!("{pending}"));
        assert_eq!(f.task(id).status, TaskStatus::InProgress, "{pending}");
        let mut verdict = cas_types::Verification::approved(
            "ver-2284".into(), id.into(), "Commitless spike reviewed".into(),
        );
        verdict.provenance = cas_types::VerificationProvenance::SupervisorDirect;
        verdict.agent_id = Some(owner_id.into());
        verdict.issuer_agent_id = verdict.agent_id.clone();
        verdict.dispatch_id = Some(dispatch.id.clone());
        use cas_store::VerificationStore;
        cas_store::SqliteVerificationStore::open(&f.cas_dir()).unwrap().add(&verdict).unwrap();
        let conn = rusqlite::Connection::open(f.cas_dir().join("cas.db")).unwrap();
        cas_store::resolve_verification_dispatch_with_conn(&conn, &dispatch.id, owner_id, None, true).unwrap();
        Some(dispatch.id)
    } else { None };
    let response = f.worker.task(Parameters(serde_json::from_value(request.clone()).unwrap()))
        .await.expect("worker close must park for the live train owner");
    assert_ne!(response.is_error, Some(true), "{response:?}");
    let parked = f.task(id);
    assert_eq!(parked.status, TaskStatus::AwaitingMerge);
    assert!(parked.closed_at.is_none());
    assert!(parked.terminal_outcome.is_none());
    if !no_delivery { assert_eq!(parked.external_ref.as_deref(), Some(PROOF)); }
    assert!(agents.get_lease(id).unwrap().is_none());
    assert_eq!(agents.get(owner_id).unwrap().cc_session_id, owner.cc_session_id);

    // The owner's ordinary close consumes the same approved proof. It must
    // not require a methodology edit, a new declaration or another review.
    if no_delivery { request.as_object_mut().unwrap().remove("no_delivery"); }
    agents.register(&Agent::new_with_role(
        "other-supervisor".into(), "other-supervisor".into(), AgentRole::Supervisor,
    )).unwrap();
    let other = CasCore::with_daemon(f.cas_dir(), None, None);
    other.set_agent_id_for_testing("other-supervisor".into());
    let other = CasService::new(other, None);
    let error = other.task(Parameters(serde_json::from_value(request.clone()).unwrap()))
        .await.unwrap_err();
    assert!(error.message.contains("[G-113]"), "{error:?}");
    assert_eq!(f.task(id).status, TaskStatus::AwaitingMerge);
    let accepted = f.supervisor.task(Parameters(serde_json::from_value(request).unwrap()))
        .await.unwrap();
    assert_ne!(accepted.is_error, Some(true), "{accepted:?}");
    let closed = f.task(id);
    assert_eq!(closed.status, TaskStatus::Closed);
    assert!(parked.deliverables.factory_branch_anchor.is_none());
    assert!(parked.deliverables.historical_factory_branch_anchors.is_empty());
    assert!(parked.deliverables.parked_branch.is_none());
    assert!(closed.deliverables.factory_branch_anchor.is_none());
    assert!(closed.deliverables.parked_branch.is_none());
    if let Some(dispatch) = reviewed {
        assert_eq!(cas_store::get_latest_verification_dispatch(&f.cas_dir(), id).unwrap().unwrap().id, dispatch);
        assert!(closed.execution_note.is_none());
        assert_eq!(git(f.dir.path(), &["rev-parse", &child.deliverables.factory_branch_start.unwrap().branch]),
            git(f.dir.path(), &["rev-parse", target]));
    }
    }
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
    // cas-3b42: the clear is recorded, so the target is absent by review.
    assert!(f.task(id).deliverables.work_target_cleared, "{cleared}");
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
            response.lines().any(crate::mcp::tools::core::task::lifecycle::close_ops::gate_preflight::is_merge_handoff) || response.contains("DELIVERY BRANCH UNRESOLVED"),
            "{subject}: {response}"
        );
        assert_ne!(f.task(id).status, TaskStatus::Closed, "{subject}");
    }
}

/// GH #1167: put an unnamed commit under the worker lane's foreign tip, the
/// way a lane carries a sync or an untagged commit from an earlier task. Under
/// cas-2387 that commit might be this task's work, so the worker's own close
/// keeps the delivery gates; the supervisor's review paths decide it.
fn bury_unnamed_lane_commit(f: &mut Fixture) {
    let worker_path = f.cas_dir().join("worktrees").join(WORKER);
    git(&worker_path, &["reset", "-q", "--hard", "HEAD~1"]);
    std::fs::write(worker_path.join("shared.txt"), "shared fixture\n").unwrap();
    git(&worker_path, &["add", "shared.txt"]);
    git(&worker_path, &["commit", "-q", "-m", "sync shared fixtures"]);
    std::fs::write(worker_path.join("other.rs"), "pub fn other() {}\n").unwrap();
    git(&worker_path, &["add", "other.rs"]);
    git(&worker_path, &["commit", "-q", "-m", OTHER]);
    f.foreign_tip = git(&worker_path, &["rev-parse", "HEAD"]);
    git(&worker_path, &["push", "-q", "-f", "origin", LANE]);
}

fn no_code_spike(id: &str, target: &str) -> Task {
    let mut spike = assigned(id, TaskType::Spike, target);
    spike.execution_note = Some("no-code".into());
    spike.external_ref = Some("art-c203".into());
    spike
}

/// GH #1167 (1): a no-code spike whose target branch exists only on origin
/// (the main checkout tracks it remotely) closes on its proof. The lane's
/// commits all belong to another task; an unreadable local target must not
/// make them look like this task's.
#[tokio::test]
async fn no_code_spike_closes_when_its_target_exists_only_on_origin_gh1167() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "staging");
    git(f.dir.path(), &["branch", "-D", "staging"]);
    let id = "cas-c203";
    f.put(&no_code_spike(id, "staging"));
    claim(&f, id);
    let response = call(
        &f.worker,
        serde_json::json!({
            "action": "close", "id": id, "reason": "Report delivered as an artifact",
            "execution_note": "no-code", "external_ref": "art-c203",
        }),
    )
    .await;
    assert_closed_without_lane(&f, id, &response);
}

/// cas-afd2: a dead verifier must not permanently jail either a source
/// delivery or a no-code report. An active cycle remains immutable.
#[tokio::test]
async fn expired_task_close_renews_verification_with_corrected_proof_cas_afd2() {
    use cas_types::{VerificationDispatchState as State, VerificationProofBoundary};
    for (no_code, recorded) in [(false, false), (true, false), (true, true)] {
        for state in ["due", "timed-out", "live"] {
            let mut env = TestEnvGuard::temp_home();
            let f = fixture(&mut env, "main");
            if recorded {
                let mut config = crate::config::Config::load(&f.cas_dir()).unwrap();
                config.set("verification.enabled", "true").unwrap();
                config.save(&f.cas_dir()).unwrap();
            }
            env.set("CAS_AGENT_ROLE", "worker");
            let id = "cas-afd2";
            if no_code {
                let mut task = no_code_spike(id, "main");
                task.external_ref = Some(PROOF.into());
                f.put(&task);
                claim(&f, id);
            } else {
                let (checkout, lane) = issue_delivery(&f, id, "main");
                let mut task = f.task(id);
                task.task_type = TaskType::Task;
                f.put(&task);
                std::fs::write(checkout.join("own.txt"), "own delivery\n").unwrap();
                git(&checkout, &["add", "own.txt"]);
                git(&checkout, &["commit", "-qm", "fix(cas-afd2): own delivery"]);
                git(&checkout, &["push", "-q", "origin", &lane]);
            }
            // The explicit dispatch still requires review if config changes.
            let deadline = chrono::Utc::now() + chrono::Duration::minutes(if state == "live" { 10 } else { -1 });
            let reviewed = recorded.then(|| {
                let report = f.task(id);
                crate::mcp::tools::core::task::lifecycle::report_proof::require_report_verdict(
                    &f.cas_dir(), true, &report, &report,
                ).unwrap().proof_to_record().unwrap()
            });
            cas_store::SqliteVerificationStore::open(&f.cas_dir()).unwrap();
            let old = cas_store::create_verification_dispatch_reviewing_with_conn(
                &rusqlite::Connection::open(f.cas_dir().join("cas.db")).unwrap(),
                id, "nocode-worker-session", "nocode-supervisor-session",
                &VerificationProofBoundary::task(), reviewed.as_deref(), deadline, false,
            ).unwrap();
            if state == "timed-out" {
                cas_store::timeout_verification_dispatch(&f.cas_dir(), id, chrono::Utc::now()).unwrap().unwrap();
            }
            let mut task = f.task(id);
            task.pending_verification = true;
            f.put(&task);
            let corrected = "https://example.test/deploys/corrected";
            let mut request = serde_json::json!({"action":"close", "id":id, "reason":"Corrected evidence recorded"});
            let corrects = no_code && !recorded;
            if corrects { request["external_ref"] = corrected.into(); }
            if corrects && state == "due" {
                let mut invalid = request.clone();
                invalid["external_ref"] = "/tmp/corrected-report.md".into();
                call(&f.worker, invalid).await;
                assert_eq!(cas_store::get_latest_verification_dispatch(&f.cas_dir(), id).unwrap().unwrap(), old);
                assert_eq!(f.task(id).external_ref.as_deref(), Some(PROOF));
            }
            let response = call(&f.worker, request.clone()).await;
            let current = cas_store::get_latest_verification_dispatch(&f.cas_dir(), id).unwrap().unwrap();
            assert_ne!(f.task(id).status, TaskStatus::Closed, "{no_code}/{recorded}/{state}: {response}");
            if state == "live" {
                assert_eq!(current.id, old.id, "{response}");
                if no_code { assert_eq!(f.task(id).external_ref.as_deref(), Some(PROOF)); }
                continue;
            }
            assert_ne!(current.id, old.id, "{no_code}/{recorded}/{state}: {response}");
            if recorded {
                use crate::mcp::tools::core::task::lifecycle::report_proof::ReportProof;
                let kept = ReportProof::recorded(&f.cas_dir(), &current.id).unwrap();
                assert!(kept.is_some(), "{state}: the replacement dispatch records no report proof: {response}");
                assert_eq!(kept, ReportProof::recorded(&f.cas_dir(), &old.id).unwrap(), "{state}: {response}");
            }
            assert_eq!(current.state, State::Pending, "{response}");
            assert!(current.deadline_at > chrono::Utc::now());
            assert!(f.task(id).pending_verification);
            assert_eq!(cas_store::get_verification_dispatch(&f.cas_dir(), &old.id).unwrap().state, State::TimedOut);
            if corrects { assert_eq!(f.task(id).external_ref.as_deref(), Some(corrected)); }
            // A retry must retain the fresh cycle, not dispatch again or close.
            let again = call(&f.worker, request).await;
            assert_eq!(cas_store::get_latest_verification_dispatch(&f.cas_dir(), id).unwrap().unwrap().id, current.id, "{again}");
            assert_ne!(f.task(id).status, TaskStatus::Closed, "{again}");
        }
    }
}

/// GH #1766, #1817: a no-code spike's audit report is named LEDGER.md. Start
/// announces how close classifies that file; close accepts the report unless
/// it declares the canonical QA matrix, which stays validated.
#[tokio::test]
async fn no_code_audit_ledger_closes_unless_it_declares_the_qa_matrix_gh1766() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let worker_path = f.cas_dir().join("worktrees").join(WORKER);
    let agents = open_agent_store(&f.cas_dir()).unwrap();
    let mut agent = agents.get("nocode-worker-session").unwrap();
    agent.metadata.insert("clone_path".into(), worker_path.display().to_string());
    agents.update(&agent).unwrap();
    let mut config = crate::config::Config::load(&f.cas_dir()).unwrap();
    config.set("verification.enabled", "true").unwrap();
    config.save(&f.cas_dir()).unwrap();
    let id = "cas-c766";
    let mut spike = no_code_spike(id, "main");
    spike.status = TaskStatus::Open;
    f.put(&spike);
    let started = call(&f.worker, serde_json::json!({"action": "start", "id": id})).await;
    for marker in crate::qa_ledger::DECLARATION_MARKERS {
        assert!(started.contains(marker), "start must announce the QA declaration: {started}");
    }
    let config = crate::config::Config::load(&f.cas_dir()).unwrap();
    let dir = &crate::config::resolved_factory_artifact_paths(
        &f.cas_dir(), config.factory().artifacts_root.as_deref(),
    ).task_dirs(id)[0];
    std::fs::create_dir_all(dir).unwrap();
    // The two original report shapes: #1766's cost tables with source links
    // and #1817's id-keyed audit rows (`L2`).
    let cost_report = "# Test audit\n\n| item | cost |\n|---|---|\n| [a.rs:1](https://example.test/a.rs) | 3m |\n";
    let value_report = "# Test audit\n\n| layer | cost | where |\n|---|---|---|\n| unit | 3m | ci |\n\n\
        | id | item | verdict | tests | evidence | fix | covering | confidence |\n\
        |---|---|---|---|---|---|---|---|\n| L2 | a.rs:1 | dup | t | e | f | none | high |\n";
    let foreign = crate::qa_ledger::scaffold(&no_code_spike("cas-other", "main"), &[], "deadbeef");
    let request = serde_json::json!({
        "action": "close", "id": id, "reason": "Audit report published",
        "execution_note": "no-code", "external_ref": "art-c766",
    });
    // Canonical declarations stay validated: a declared-but-invalid ledger,
    // and a well-formed canonical ledger written for another task.
    for (case, body) in [
        ("declared and invalid", format!("schema: 2\n{value_report}")),
        ("foreign canonical", foreign),
    ] {
        std::fs::write(dir.join("LEDGER.md"), body).unwrap();
        let refused = call(&f.worker, request.clone()).await;
        assert!(refused.contains("QA LEDGER REJECTED"), "{case}: {refused}");
        assert_ne!(f.task(id).status, TaskStatus::Closed, "{case}");
    }
    for report in [cost_report, value_report] {
        std::fs::write(dir.join("LEDGER.md"), report).unwrap();
        let response = call(&f.worker, request.clone()).await;
        assert!(!response.contains("QA LEDGER REJECTED"), "{response}");
        assert!(response.contains("VERIFICATION"), "the report reaches ordinary verification: {response}");
    }
}

/// GH #1167 (2): the supervisor's evidence_only close of an artifact-only
/// no-code spike succeeds even when the worker lane holds an unnamed commit.
/// The supervisor reviewed the artifact; the lane is not measured.
#[tokio::test]
async fn evidence_only_closes_an_artifact_only_no_code_spike_gh1167() {
    let mut env = TestEnvGuard::temp_home();
    let mut f = fixture(&mut env, "main");
    bury_unnamed_lane_commit(&mut f);
    let id = "cas-c204";
    f.put(&no_code_spike(id, "main"));
    claim(&f, id);
    let proof = f.artifact(id);
    let response = call(
        &f.supervisor,
        serde_json::json!({
            "action": "close", "id": id, "reason": "Spike report retained as an artifact",
            "evidence_only": true, "evidence_only_artifact_path": proof,
            "evidence_only_reference": format!("branch:{LANE}"),
        }),
    )
    .await;
    assert_closed_without_lane(&f, id, &response);
    let evidence = f.task(id).deliverables.evidence_only.expect("evidence recorded");
    assert!(evidence.paths.is_empty(), "{:?}", evidence.paths);
}

/// GH #1167 (3): after the supervisor clears the code target
/// (proof_scope_fix target_repo=""), the no-code spike closes on its proof
/// with no branch resolution and no merge-tree preflight, even though the
/// lane holds an unnamed commit. (A retained delivery anchor would stay
/// binding: cas-3067.)
#[tokio::test]
async fn cleared_target_no_code_spike_closes_without_branch_or_merge_tree_gh1167() {
    let mut env = TestEnvGuard::temp_home();
    let mut f = fixture(&mut env, "main");
    bury_unnamed_lane_commit(&mut f);
    let id = "cas-c205";
    let mut spike = no_code_spike(id, "main");
    spike.status = TaskStatus::AwaitingMerge;
    spike.deliverables.parked_branch = Some(LANE.into());
    f.put(&spike);
    claim(&f, id);
    let cleared = call(
        &f.supervisor,
        serde_json::json!({
            "action": "update", "id": id, "proof_scope_fix": true, "target_repo": "",
            "reason": "report-only spike: no code target",
        }),
    )
    .await;
    assert!(f.task(id).deliverables.work_target.is_none(), "{cleared}");
    // cas-3b42: the clear is recorded, so the target is absent by review.
    assert!(f.task(id).deliverables.work_target_cleared, "{cleared}");
    let response = call(
        &f.worker,
        serde_json::json!({
            "action": "close", "id": id, "reason": "Report delivered",
            "execution_note": "no-code", "external_ref": "art-c203",
        }),
    )
    .await;
    assert!(!response.contains("merge-tree"), "{response}");
    let task = f.task(id);
    assert_eq!(task.status, TaskStatus::Closed, "{response}");
    assert!(
        !response.lines().any(crate::mcp::tools::core::task::lifecycle::close_ops::gate_preflight::is_merge_handoff) && !response.contains("DELIVERY BRANCH UNRESOLVED"),
        "{response}"
    );
}

/// GH #1901: a completed documentation delivery is not the current artifact.
#[tokio::test]
async fn artifact_scope_fix_retires_parked_docs_and_refuses_code_cas_3ff6() {
    for (code, already_cleared) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut env = TestEnvGuard::temp_home();
        let f = fixture(&mut env, "main");
        let id = "cas-8156";
        let (checkout, lane) = issue_delivery(&f, id, "main");
        let path = if code { "product.rs" } else { "docs/design/proposal.brief.md" };
        std::fs::create_dir_all(checkout.join("docs/design")).unwrap();
        std::fs::write(checkout.join(path), "previous delivery\n").unwrap();
        git(&checkout, &["add", path]);
        git(&checkout, &["commit", "-qm", "docs(cas-8156): previous design"]);
        let old_tip = git(&checkout, &["rev-parse", "HEAD"]);
        git(&checkout, &["push", "-q", "origin", &lane]);
        git(f.dir.path(), &["merge", "-q", "--ff-only", &lane]);
        git(f.dir.path(), &["push", "-q", "origin", "main"]);
        // Persist the previous park, then resume with a new portable artifact.
        let mut task = f.task(id);
        task.status = TaskStatus::AwaitingMerge;
        task.task_type = TaskType::Spike;
        task.execution_note = Some("no-code".into());
        task.external_ref = Some("artifact:art-current-design".into());
        task.deliverables.factory_branch_anchor = Some(old_tip.clone());
        task.deliverables.historical_factory_branch_anchors.push(old_tip.clone());
        task.deliverables.parked_branch = Some(lane.clone());
        task.deliverables.files_changed = vec![path.into()];
        if already_cleared {
            task.status = TaskStatus::Open;
            task.deliverables.work_target = None;
            task.deliverables.work_target_cleared = true;
        }
        let prior_target = task.deliverables.work_target.clone();
        f.put(&task);
        let mut config = crate::config::Config::load(&f.cas_dir()).unwrap();
        config.set("verification.enabled", "true").unwrap();
        config.set("qa.mode", "batch").unwrap();
        config.save(&f.cas_dir()).unwrap();
        let corrected = f.supervisor.task(Parameters(serde_json::from_value(serde_json::json!({
            "action":"update", "id":id, "proof_scope_fix":true,
            "target_repo":"", "reason":"Published design replaces the old documentation delivery",
        })).unwrap())).await;
        if code {
            assert!(corrected.is_err(), "{corrected:?}");
            assert_eq!(f.task(id).deliverables.factory_branch_anchor.as_deref(), Some(old_tip.as_str()));
            assert_eq!(f.task(id).deliverables.work_target, prior_target);
            continue;
        }
        assert!(corrected.is_ok(), "{corrected:?}");
        let response = call(&f.worker, serde_json::json!({
            "action":"close", "id":id, "reason":"Current design artifact delivered",
        })).await;
        assert!(!response.contains("[G-67]"), "{response}");
        let current = f.task(id);
        assert!(current.deliverables.factory_branch_anchor.is_none(), "{response}");
        assert!(current.deliverables.historical_factory_branch_anchors.is_empty());
        assert!(current.deliverables.factory_branch_start.is_none());
        assert!(current.deliverables.parked_branch.is_none());
        assert!(current.deliverables.files_changed.is_empty());
        assert_eq!(current.external_ref.as_deref(), Some("artifact:art-current-design"));
        if current.status != TaskStatus::Closed {
            let dispatch = cas_store::get_latest_verification_dispatch(&f.cas_dir(), id).unwrap().unwrap();
            assert!(dispatch.repository.is_none_or(|repository| repository.anchor_commits.is_empty()));
        }
    }
}

/// GH #1167 (4): unnamed lane work still keeps ordinary delivery gates.
#[tokio::test]
async fn unnamed_lane_commit_still_gates_code_and_worker_no_code_closes_gh1167() {
    for (id, no_code) in [("cas-c206", false), ("cas-c207", true)] {
        let mut env = TestEnvGuard::temp_home();
        let mut f = fixture(&mut env, "main");
        bury_unnamed_lane_commit(&mut f);
        let task = if no_code {
            no_code_spike(id, "main")
        } else {
            assigned(id, TaskType::Task, "main")
        };
        f.put(&task);
        claim(&f, id);
        let mut request = serde_json::json!({"action": "close", "id": id, "reason": "Done"});
        if no_code {
            request["execution_note"] = "no-code".into();
            request["external_ref"] = "art-c203".into();
        }
        let response = call(&f.worker, request).await;
        assert!(
            response.lines().any(crate::mcp::tools::core::task::lifecycle::close_ops::gate_preflight::is_merge_handoff) || response.contains("DELIVERY BRANCH UNRESOLVED"),
            "{id}: {response}"
        );
        assert_ne!(f.task(id).status, TaskStatus::Closed, "{id}");
    }
}

/// What an operations task looks like after its first close parked it
/// (cas-3507): no execution_note, no commit of its own, a parked task branch
/// that was never pushed, and the conflict preflight flag that came with it.
fn parked_ops_task(id: &str) -> Task {
    let mut task = assigned(id, TaskType::Task, "main");
    task.status = TaskStatus::AwaitingMerge;
    task.deliverables.parked_branch = Some(format!("{LANE}-{id}"));
    task.deliverables.merge_conflicted = true;
    task
}

const PR: &str = "https://github.com/example/cloud/pull/149";

/// cas-b38a: a parked operations task with no commit of its own closes when
/// the supervisor declares it no-code on the merged pull request that did
/// the work. Nothing of the lane is attributed to it.
#[tokio::test]
async fn parked_ops_task_closes_no_code_on_its_pull_request_cas_b38a() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let id = "cas-nc07";
    f.put(&parked_ops_task(id));
    let response = call(
        &f.supervisor,
        serde_json::json!({
            "action": "close", "id": id, "reason": "Promotion merged and deployed",
            "execution_note": "no-code", "external_ref": PR,
        }),
    )
    .await;
    assert_closed_without_lane(&f, id, &response);
    let task = f.task(id);
    assert_eq!(task.execution_note.as_deref(), Some("no-code"));
    assert_eq!(task.external_ref.as_deref(), Some(PR));
}

/// cas-b38a: the same parked operations task closes by the supervisor's
/// evidence_only review, citing the pull request, with no paths measured.
#[tokio::test]
async fn parked_ops_task_closes_by_evidence_on_its_pull_request_cas_b38a() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let id = "cas-nc08";
    f.put(&parked_ops_task(id));
    let proof = f.artifact(id);
    let response = call(
        &f.supervisor,
        serde_json::json!({
            "action": "close", "id": id, "reason": "Promotion merged and deployed",
            "evidence_only": true, "evidence_only_artifact_path": proof,
            "evidence_only_reference": PR,
        }),
    )
    .await;
    assert_closed_without_lane(&f, id, &response);
    let closed = f.task(id);
    assert!(!closed.deliverables.merge_conflicted);
    let evidence = closed
        .deliverables
        .evidence_only
        .clone()
        .expect("evidence recorded");
    assert!(evidence.paths.is_empty(), "{:?}", evidence.paths);
    assert!(
        closed
            .notes
            .contains("evidence-only close retired lane records"),
        "{}",
        closed.notes
    );
}

async fn fixture_cloud(f: &Fixture) -> wiremock::MockServer {
    let cloud = wiremock::MockServer::start().await;
    std::fs::write(f.cas_dir().join("cloud.json"), serde_json::json!({
        "endpoint": cloud.uri(), "token": "fixture-token"
    }).to_string()).unwrap();
    cloud
}

async fn publish_report(f: &Fixture, cloud: &wiremock::MockServer, task_id: &str, body: &str) -> cas_store::PublishedArtifact {
    let publish = stage_report(f, cloud, task_id, body).await;
    tokio::task::spawn_blocking(publish).await.unwrap()
}

async fn stage_report(
    f: &Fixture, cloud: &wiremock::MockServer, task_id: &str, body: &str,
) -> Box<dyn FnOnce() -> cas_store::PublishedArtifact + Send> {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};
    let cloud_id = format!("cloud-{}", cloud.received_requests().await.unwrap().len());
    Mock::given(method("POST")).and(path("/api/artifacts/begin"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "artifact_id": cloud_id, "upload_url": format!("{}/blob/{cloud_id}", cloud.uri()), "required_headers": {}
        }))).up_to_n_times(1).mount(cloud).await;
    Mock::given(method("PUT")).and(path(format!("/blob/{cloud_id}")))
        .respond_with(ResponseTemplate::new(200)).mount(cloud).await;
    Mock::given(method("GET")).and(path(format!("/api/artifacts/{cloud_id}/url")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "artifact_id": cloud_id, "url": format!("{}/blob/{cloud_id}", cloud.uri()), "size_bytes": body.len()
        }))).mount(cloud).await;
    Mock::given(method("GET")).and(path(format!("/blob/{cloud_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.as_bytes().to_vec())).mount(cloud).await;
    Mock::given(method("POST")).and(path(format!("/api/artifacts/{cloud_id}/complete")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"url": null})))
        .mount(cloud).await;
    let root = f.cas_dir();
    let report = crate::mcp::tools::core::task::lifecycle::qa_evidence_gate::task_evidence_dir(&root, &f.task(task_id))
        .unwrap().join("report.md");
    let (task_id, body) = (task_id.to_string(), body.to_string());
    Box::new(move || {
        std::fs::create_dir_all(report.parent().unwrap()).unwrap();
        std::fs::write(&report, body).unwrap();
        let config = crate::config::Config::load(&root).unwrap_or_default();
        let base = crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
        let store = cas_store::SqliteArtifactStore::open(&root).unwrap();
        let context = crate::artifacts::PublishContext {
            store: &store,
            roots: crate::artifacts::paths::PublishRoots::new(&root, &base, &task_id),
            task_id: task_id.clone(), cloud: crate::artifacts::cloud_client(&root),
        };
        let artifact = crate::artifacts::publish(&context, &report).unwrap().artifact;
        assert_eq!(artifact.status, "committed");
        artifact
    })
}

fn open_rapid_window(f: &Fixture, id: &str) {
    assert!(!crate::rapid_response::verification_sampled(id));
    let tasks = open_task_store(&f.cas_dir()).unwrap();
    let mut epic = Task::new("cas-0ff6-rapid".into(), "Rapid-response epic".into());
    epic.task_type = TaskType::Epic;
    tasks.add(&epic).unwrap();
    tasks.add_dependency(&Dependency::new(id.into(), epic.id.clone(), DependencyType::ParentChild)).unwrap();
    crate::rapid_response::enable(
        &f.cas_dir(), tasks.as_ref(), &epic.id, chrono::Duration::hours(1), "Incident", "Revert the release",
        chrono::Utc::now(),
    ).unwrap();
    assert!(crate::rapid_response::active_for_task_at(&f.cas_dir(), &f.task(id), None).is_some());
}

#[tokio::test]
async fn supervisor_close_of_parked_no_code_spike_needs_a_current_verdict_gh2417() {
    use cas_store::VerificationStore;
    let mut env = TestEnvGuard::temp_home();
    let mut wrong = Vec::new();
    for prior in [
        "never-verified", "light-depth", "rejected-earlier-report", "approval-unreadable",
        "legacy-approved", "legacy-skipped", "legacy-approved-update-route",
        "inventory-unreadable", "rapid-unsampled", "release-tagged", "opencode-worker",
    ] {
        let f = fixture(&mut env, "main");
        let mut config = crate::config::Config::load(&f.cas_dir()).unwrap();
        config.set("verification.enabled", "true").unwrap();
        config.save(&f.cas_dir()).unwrap();
        env.set("CAS_AGENT_ROLE", "supervisor");
        if prior == "opencode-worker" {
            env.set("CAS_FACTORY_WORKER_CLI", "opencode");
        }
        let id = "cas-2417";
        let (checkout, lane) = issue_delivery(&f, id, "main");
        std::fs::create_dir_all(checkout.join("docs")).unwrap();
        std::fs::write(checkout.join("docs/spike-report.md"), "# Spike report\n").unwrap();
        git(&checkout, &["add", "docs/spike-report.md"]);
        git(&checkout, &["commit", "-qm", "docs(cas-2417): spike report"]);
        git(&checkout, &["push", "-q", "origin", &lane]);
        let anchor = git(&checkout, &["rev-parse", "HEAD"]);
        git(f.dir.path(), &["merge", "-q", "--ff-only", &lane]);
        git(f.dir.path(), &["push", "-q", "origin", "main"]);
        let mut spike = f.task(id);
        spike.task_type = TaskType::Spike;
        spike.execution_note = Some("no-code".into());
        spike.external_ref = Some(PR.into());
        spike.status = TaskStatus::AwaitingMerge;
        spike.deliverables.factory_branch_anchor = Some(anchor);
        spike.deliverables.parked_branch = Some(lane);
        if prior == "light-depth" {
            spike.depth = cas_types::TaskDepth::Light;
        }
        f.put(&spike);
        match prior {
            "inventory-unreadable" => {
                cas_store::SqliteArtifactStore::open(&f.cas_dir()).unwrap().record_local(&cas_store::NewArtifact {
                    id: "art-0ff6c0de".into(), task_id: id.into(), name: "report.md".into(),
                    mime: "text/markdown".into(), size_bytes: 9, sha256: "e".repeat(64),
                }).unwrap();
                rusqlite::Connection::open(f.cas_dir().join("cas.db")).unwrap()
                    .execute("UPDATE artifacts SET size_bytes = 'unreadable'", []).unwrap();
            }
            "rapid-unsampled" => open_rapid_window(&f, id),
            "release-tagged" => {
                git(f.dir.path(), &["tag", "v1.0.0"]);
                git(f.dir.path(), &["push", "-q", "origin", "v1.0.0"]);
            }
            _ => {}
        }
        let mut seeded = None;
        if matches!(prior, "rejected-earlier-report" | "approval-unreadable") {
            let reviewed = (prior == "approval-unreadable").then(|| {
                let report = f.task(id);
                crate::mcp::tools::core::task::lifecycle::report_proof::require_report_verdict(
                    &f.cas_dir(), true, &report, &report,
                ).unwrap().proof_to_record().unwrap()
            });
            cas_store::SqliteVerificationStore::open(&f.cas_dir()).unwrap();
            let conn = rusqlite::Connection::open(f.cas_dir().join("cas.db")).unwrap();
            let dispatch = cas_store::create_verification_dispatch_reviewing_with_conn(
                &conn, id, "nocode-supervisor-session", "nocode-supervisor-session",
                &cas_types::VerificationProofBoundary::task(), reviewed.as_deref(),
                chrono::Utc::now() + chrono::Duration::minutes(10), false,
            ).unwrap();
            seeded = reviewed.map(|_| dispatch.id.clone());
            let mut verdict = cas_types::Verification::new("ver-2417-rejected".into(), id.into());
            verdict.status = if prior == "approval-unreadable" {
                cas_types::VerificationStatus::Approved
            } else {
                cas_types::VerificationStatus::Rejected
            };
            verdict.summary = "Earlier report version reviewed".into();
            verdict.provenance = cas_types::VerificationProvenance::SupervisorDirect;
            verdict.agent_id = Some("nocode-supervisor-session".into());
            verdict.issuer_agent_id = verdict.agent_id.clone();
            verdict.dispatch_id = Some(dispatch.id.clone());
            cas_store::SqliteVerificationStore::open(&f.cas_dir()).unwrap().add(&verdict).unwrap();
            cas_store::resolve_verification_dispatch_with_conn(
                &conn, &dispatch.id, "nocode-supervisor-session", None, true,
            ).unwrap();
            if prior == "approval-unreadable" {
                conn.execute("UPDATE verifications SET confidence = 'unreadable' WHERE id = ?1", [&verdict.id]).unwrap();
            }
        }
        if prior.starts_with("legacy-") {
            let mut without_dispatch = cas_types::Verification::new(format!("ver-2417-{prior}"), id.into());
            without_dispatch.status = if prior == "legacy-skipped" {
                cas_types::VerificationStatus::Skipped
            } else {
                cas_types::VerificationStatus::Approved
            };
            without_dispatch.summary = "No dispatch recorded".into();
            cas_store::SqliteVerificationStore::open(&f.cas_dir()).unwrap().add(&without_dispatch).unwrap();
        }
        let by_update = prior == "legacy-approved-update-route";
        let response = call(&f.supervisor, if by_update {
            serde_json::json!({"action": "update", "id": id, "status": "closed"})
        } else {
            serde_json::json!({"action": "close", "id": id, "reason": "Report accepted"})
        }).await;
        let latest = cas_store::get_latest_verification_dispatch(&f.cas_dir(), id).unwrap().map(|dispatch| dispatch.id);
        let dispatched = latest.is_some();
        if f.task(id).status == TaskStatus::Closed
            || (by_update || seeded.is_some() || prior == "inventory-unreadable") && !response.contains("G-68")
            || prior == "inventory-unreadable" && dispatched
            || seeded.is_some() && latest != seeded
        {
            wrong.push(format!("{prior}: dispatched={dispatched}: {response}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[tokio::test]
async fn premerge_approval_of_a_no_code_spike_covers_the_artifacts_it_reviewed_gh2417() {
    let mut env = TestEnvGuard::temp_home();
    let mut wrong = Vec::new();
    for case in [
        "unchanged", "light-depth", "rapid-unsampled", "release-tagged", "republished",
        "revoked-before-launch", "peer-report-revoked-before-launch", "bytes-changed-before-launch",
        "size-changed-before-launch", "cloud-unavailable-at-launch", "republished-while-merge-waits",
        "opencode-worker",
    ] {
        let f = fixture(&mut env, "main");
        let mut config = crate::config::Config::load(&f.cas_dir()).unwrap();
        config.set("verification.enabled", "true").unwrap();
        config.save(&f.cas_dir()).unwrap();
        env.set("CAS_AGENT_ROLE", "worker");
        if case == "opencode-worker" {
            env.set("CAS_FACTORY_WORKER_CLI", "opencode");
        }
        let cloud = fixture_cloud(&f).await;
        let id = "cas-2418";
        let (checkout, lane) = issue_delivery(&f, id, "main");
        let mut spike = f.task(id);
        spike.task_type = TaskType::Spike;
        spike.execution_note = Some("no-code".into());
        spike.external_ref = Some(PROOF.into());
        if case == "light-depth" {
            spike.depth = cas_types::TaskDepth::Light;
        }
        f.put(&spike);
        std::fs::create_dir_all(checkout.join("docs")).unwrap();
        std::fs::write(checkout.join("docs/spike-report.md"), "# Spike report\n").unwrap();
        git(&checkout, &["add", "docs/spike-report.md"]);
        git(&checkout, &["commit", "-qm", "docs(cas-2418): spike report"]);
        git(&checkout, &["push", "-q", "origin", &lane]);
        let published = if case == "peer-report-revoked-before-launch" {
            let peer_report = cas_store::PublishedArtifact {
                id: "art-0ff62418".into(), task_id: id.into(), name: "report.md".into(),
                mime: "text/markdown".into(), size_bytes: 12, sha256: "c".repeat(64),
                cloud_artifact_id: Some("cloud-peer".into()), status: "committed".into(),
                cloud_url: None, slack_permalink: None, created_at: chrono::Utc::now().to_rfc3339(),
                revocation: None,
            };
            crate::artifacts::proof::record_published(&f.cas_dir(), &peer_report).unwrap();
            peer_report
        } else {
            publish_report(&f, &cloud, id, "report v1\n").await
        };
        let parked = call(&f.worker, serde_json::json!({"action": "close", "id": id, "reason": "Report published"})).await;
        assert_eq!(f.task(id).status, TaskStatus::AwaitingMerge, "{case}: {parked}");
        match case {
            "rapid-unsampled" => open_rapid_window(&f, id),
            "release-tagged" => {
                git(&checkout, &["tag", "v1.0.0"]);
                git(&checkout, &["push", "-q", "origin", "v1.0.0"]);
            }
            _ => {}
        }

        let repo = f.dir.path();
        git(repo, &["config", "user.name", "CAS Test"]);
        git(repo, &["config", "user.email", "cas@example.test"]);
        env.set("CAS_AGENT_ROLE", "supervisor");
        let train_owner = CasCore::with_daemon(f.cas_dir(), None, None);
        train_owner.set_agent_id_for_testing("nocode-supervisor-session".into());
        let held = train_owner.prepare_premerge_verification(&f.task(id), repo, &lane, "main");
        let Some(reviewed) = cas_store::get_latest_verification_dispatch(&f.cas_dir(), id).unwrap()
            .filter(|_| held.is_err())
        else {
            wrong.push(format!("{case}: no review requested before the merge: {held:?}"));
            continue;
        };
        if case.ends_with("-launch") {
            use cas_types::VerificationDispatchState::{Invalidated, Pending};
            use wiremock::matchers::{any, method, path};
            use wiremock::{Mock, ResponseTemplate};
            let cloud_id = published.cloud_artifact_id.clone().unwrap();
            let view = format!("/api/artifacts/{cloud_id}/url");
            cloud.reset().await;
            match case {
                "cloud-unavailable-at-launch" => {
                    Mock::given(any()).respond_with(ResponseTemplate::new(503)).mount(&cloud).await;
                }
                "bytes-changed-before-launch" | "size-changed-before-launch" => {
                    let served: &[u8] = if case == "size-changed-before-launch" { b"report v10\n" } else { b"report v9\n" };
                    Mock::given(method("GET")).and(path(view.clone()))
                        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                            "artifact_id": cloud_id, "url": format!("{}/blob/{cloud_id}", cloud.uri()),
                            "size_bytes": published.size_bytes,
                        }))).mount(&cloud).await;
                    Mock::given(method("GET")).and(path(format!("/blob/{cloud_id}")))
                        .respond_with(ResponseTemplate::new(200).set_body_bytes(served.to_vec()))
                        .mount(&cloud).await;
                }
                _ => {
                    Mock::given(method("GET")).and(path(view.clone()))
                        .respond_with(ResponseTemplate::new(409).set_body_json(serde_json::json!({
                            "error": "artifact_revoked",
                        }))).mount(&cloud).await;
                }
            }
            let (core, task, dispatch) = (train_owner.clone(), f.task(id), reviewed.clone());
            let output = f.cas_dir().join("fixture-verdict.json");
            tokio::task::spawn_blocking(move || {
                crate::mcp::tools::core::workflow::premerge_verification::launch_verifier_with(
                    &core, &task, &dispatch, output, |_| Err("stop before the fixture verifier".into()),
                ).unwrap().unwrap().join().unwrap()
            }).await.unwrap();
            let asked = cloud.received_requests().await.unwrap().iter().any(|request| request.url.path() == view);
            let launched = cas_store::get_verification_dispatch(&f.cas_dir(), &reviewed.id).unwrap().state;
            let retried = (case != "cloud-unavailable-at-launch")
                .then(|| train_owner.prepare_premerge_verification(&f.task(id), repo, &lane, "main"));
            let latest = cas_store::get_latest_verification_dispatch(&f.cas_dir(), id).unwrap().unwrap();
            let recorded = crate::mcp::tools::core::task::lifecycle::report_proof::ReportProof::recorded(
                &f.cas_dir(), &latest.id,
            ).unwrap();
            let replaced = launched == Invalidated && latest.id != reviewed.id && latest.state == Pending;
            let expected = asked && match case {
                "cloud-unavailable-at-launch" => launched == Pending && latest.id == reviewed.id,
                "bytes-changed-before-launch" | "size-changed-before-launch" => replaced,
                _ => replaced && recorded.as_ref().is_some_and(|proof| {
                    proof.artifacts.iter().all(|(artifact, _)| artifact != &published.id)
                }),
            };
            if !expected {
                wrong.push(format!(
                    "{case}: Cloud asked={asked}; launched dispatch is {launched:?}; latest {latest:?} records {recorded:?}; retry {retried:?}",
                ));
            }
            continue;
        }
        f.supervisor.verification(Parameters(serde_json::from_value(serde_json::json!({
            "action": "add", "task_id": id, "dispatch_id": reviewed.id,
            "status": "approved", "summary": "Report reviewed", "confidence": 1.0,
        })).unwrap())).await.unwrap();
        if case == "republished-while-merge-waits" {
            let approval = match train_owner.prepare_premerge_verification(&f.task(id), repo, &lane, "main") {
                Ok(Some(approval)) => approval,
                other => {
                    wrong.push(format!("{case}: no approval to admit: {other:?}"));
                    continue;
                }
            };
            let before = approval.validate_for_merge(&f.cas_dir(), &f.task(id));
            publish_report(&f, &cloud, id, "report v2\n").await;
            let after = approval.validate_for_merge(&f.cas_dir(), &f.task(id));
            if before.is_err() || after.is_ok() {
                wrong.push(format!("{case}: admitted before republication {before:?}, after {after:?}"));
            }
            continue;
        }
        git(repo, &["merge", "-q", "--ff-only", &lane]);
        git(repo, &["push", "-q", "origin", "main"]);
        if case == "republished" {
            publish_report(&f, &cloud, id, "report v2\n").await;
        }
        let response = call(&f.supervisor, serde_json::json!({
            "action": "close", "id": id, "reason": "Report accepted",
        })).await;
        let closed = f.task(id).status == TaskStatus::Closed;
        if closed == (case == "republished")
            || case == "republished" && (!response.contains("G-70") || response.contains("supervisor_override=true"))
        {
            wrong.push(format!("{case}: closed={closed}: {response}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[tokio::test]
async fn revised_no_code_report_needs_a_new_verdict_before_close_gh2417() {
    use cas_store::VerificationStore;
    let mut env = TestEnvGuard::temp_home();
    let mut wrong = Vec::new();
    for (case, task_type) in [
        ("unchanged", TaskType::Spike), ("unpublished-draft", TaskType::Spike),
        ("peer-report-imported", TaskType::Spike), ("approved", TaskType::Spike),
        ("rejected", TaskType::Spike), ("chore", TaskType::Chore), ("demo-feature", TaskType::Feature),
        ("report-republished", TaskType::Spike), ("notes-marker", TaskType::Spike),
        ("published-before-launch", TaskType::Spike), ("peer-report-importing", TaskType::Spike),
        ("report-revoked", TaskType::Spike), ("reference-revised-before-launch", TaskType::Spike),
        ("active-dispatch-without-recorded-proof", TaskType::Spike), ("legacy-mcp-close", TaskType::Spike),
        ("update-route", TaskType::Spike),
        ("reference-revised-during-close", TaskType::Spike), ("republished-during-final-write", TaskType::Spike),
        ("revoked-during-update-close", TaskType::Spike), ("withdrawn-after-close-then-replayed", TaskType::Spike),
    ] {
        let f = fixture(&mut env, "main");
        let mut config = crate::config::Config::load(&f.cas_dir()).unwrap();
        config.set("verification.enabled", "true").unwrap();
        config.save(&f.cas_dir()).unwrap();
        env.set("CAS_FACTORY_SUPERVISOR_CLI", "codex");
        let cloud = fixture_cloud(&f).await;
        let id = "cas-2372";
        let mut spike = assigned(id, task_type, "main");
        spike.execution_note = Some("no-code".into());
        spike.external_ref = Some(PROOF.into());
        if case == "demo-feature" {
            spike.demo_statement = "Operators read the published report".into();
        }
        f.put(&spike);
        claim(&f, id);
        if case == "demo-feature" {
            let dir = crate::mcp::tools::core::task::lifecycle::qa_evidence_gate::task_evidence_dir(
                &f.cas_dir(), &f.task(id),
            ).unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            let audit_ledger_without_qa_matrix = "# Report notes\n";
            std::fs::write(dir.join("LEDGER.md"), audit_ledger_without_qa_matrix).unwrap();
        }
        let peer_report = cas_store::PublishedArtifact {
            id: "art-0ff62372".into(), task_id: id.into(), name: "report.md".into(),
            mime: "text/markdown".into(), size_bytes: 12, sha256: "c".repeat(64),
            cloud_artifact_id: Some("cloud-peer".into()), status: "committed".into(),
            cloud_url: None, slack_permalink: None, created_at: chrono::Utc::now().to_rfc3339(),
            revocation: None,
        };
        let published = if case.starts_with("peer-report-import") {
            crate::artifacts::proof::record_published(&f.cas_dir(), &peer_report).unwrap();
            peer_report.clone()
        } else {
            publish_report(&f, &cloud, id, "report v1\n").await
        };
        let unrecorded = matches!(case, "active-dispatch-without-recorded-proof" | "legacy-mcp-close").then(|| {
            cas_store::create_verification_dispatch_bound(
                &f.cas_dir(), id, "nocode-supervisor-session", "nocode-supervisor-session",
                &cas_types::VerificationProofBoundary::task(),
                chrono::Utc::now() + chrono::Duration::minutes(10), false,
            ).unwrap()
        });
        if case == "legacy-mcp-close" {
            let (core, task, dispatch) = (CasCore::with_daemon(f.cas_dir(), None, None), f.task(id), unrecorded.clone().unwrap());
            let output = f.cas_dir().join("legacy-verdict.json");
            tokio::task::spawn_blocking(move || {
                crate::mcp::tools::core::workflow::premerge_verification::launch_verifier_with(
                    &core, &task, &dispatch, output, |_| panic!("proofless dispatch reached verifier preparation"),
                ).unwrap().unwrap().join().unwrap();
            }).await.unwrap();
            let queue = crate::store::open_prompt_queue_store(&f.cas_dir()).unwrap();
            let notices = queue.peek_all(50).unwrap();
            if !notices.iter().any(|row| row.prompt.contains("verifier_dispatch_report_proof_missing:")) {
                wrong.push(format!("{case}: missing recovery code in {} notice(s)", notices.len()));
            }
            let legacy_id = &unrecorded.as_ref().unwrap().id;
            assert_eq!(
                cas_store::get_verification_dispatch(&f.cas_dir(), legacy_id).unwrap().state,
                cas_types::VerificationDispatchState::Invalidated,
            );
            assert!(cas_store::get_verification_for_dispatch(&f.cas_dir(), legacy_id).unwrap().is_none());
        }
        let request = serde_json::json!({"action": "close", "id": id, "reason": "Report published"});
        let pending = call(&f.worker, request.clone()).await;
        let reviewed = cas_store::get_latest_verification_dispatch(&f.cas_dir(), id).unwrap()
            .unwrap_or_else(|| panic!("{case}: {pending}"));
        if let Some(unrecorded) = unrecorded {
            let retired = cas_store::get_verification_dispatch(&f.cas_dir(), &unrecorded.id).unwrap().state;
            let recorded = crate::mcp::tools::core::task::lifecycle::report_proof::ReportProof::recorded(
                &f.cas_dir(), &reviewed.id,
            ).unwrap();
            let expected = crate::mcp::tools::core::task::lifecycle::report_proof::ReportProof {
                external_ref: Some(PROOF.into()), artifacts: vec![(published.id.clone(), published.sha256.clone())],
            };
            if retired != cas_types::VerificationDispatchState::Invalidated
                || reviewed.id == unrecorded.id || reviewed.state != cas_types::VerificationDispatchState::Pending
                || recorded.as_ref() != Some(&expected) || !pending.contains("G-68")
            {
                wrong.push(format!(
                    "{case}: unrecorded dispatch is {retired:?}; review {} records {recorded:?}: {pending}", reviewed.id,
                ));
                continue;
            }
        }
        if case == "reference-revised-before-launch" {
            let mut revised = f.task(id);
            revised.external_ref = Some("https://example.test/deploys/revised".into());
            f.put(&revised);
        }
        if matches!(case, "published-before-launch" | "reference-revised-before-launch" | "legacy-mcp-close") {
            let later = if case == "published-before-launch" {
                publish_report(&f, &cloud, id, "report v2\n").await.id
            } else {
                String::new()
            };
            let (core, task, dispatch) = (CasCore::with_daemon(f.cas_dir(), None, None), f.task(id), reviewed.clone());
            let input = tokio::task::spawn_blocking(move || {
                crate::mcp::tools::core::workflow::premerge_verification::verifier_prompt(&core, &task, &dispatch)
            }).await.unwrap().unwrap_or_else(|error| panic!("{case}: {error}"));
            let inputs: serde_json::Value = input.lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .find(|value: &serde_json::Value| value.get("artifacts").is_some()).unwrap();
            let shown: Vec<_> = inputs["artifacts"].as_array().unwrap().iter()
                .filter_map(|artifact| artifact["artifact_id"].as_str()).collect();
            if shown != [published.id.as_str()] || inputs["task"]["external_ref"] != PROOF {
                wrong.push(format!(
                    "{case}: verifier shown {shown:?} at {}, dispatched {} at {PROOF} (later {later})",
                    inputs["task"]["external_ref"], published.id,
                ));
            }
        }
        let mut verdict = cas_types::Verification::new(format!("ver-2372-{case}"), id.into());
        verdict.status = if case == "rejected" {
            cas_types::VerificationStatus::Rejected
        } else {
            cas_types::VerificationStatus::Approved
        };
        verdict.summary = "Original report reviewed".into();
        verdict.provenance = cas_types::VerificationProvenance::SupervisorDirect;
        verdict.agent_id = Some("nocode-supervisor-session".into());
        verdict.issuer_agent_id = verdict.agent_id.clone();
        verdict.dispatch_id = Some(reviewed.id.clone());
        cas_store::SqliteVerificationStore::open(&f.cas_dir()).unwrap().add(&verdict).unwrap();
        let conn = rusqlite::Connection::open(f.cas_dir().join("cas.db")).unwrap();
        cas_store::resolve_verification_dispatch_with_conn(
            &conn, &reviewed.id, "nocode-supervisor-session", None, true,
        ).unwrap();
        let artifacts = cas_store::SqliteArtifactStore::open(&f.cas_dir()).unwrap();
        match case {
            "unpublished-draft" => {
                artifacts.record_local(&cas_store::NewArtifact {
                    id: "art-0ff6d4af".into(), task_id: id.into(), name: "draft.md".into(),
                    mime: "text/markdown".into(), size_bytes: 9, sha256: "d".repeat(64),
                }).unwrap();
            }
            "peer-report-imported" | "peer-report-importing" => {
                artifacts.record_local(&cas_store::NewArtifact {
                    id: peer_report.id.clone(), task_id: id.into(), name: peer_report.name.clone(),
                    mime: peer_report.mime.clone(), size_bytes: peer_report.size_bytes, sha256: peer_report.sha256.clone(),
                }).unwrap();
                if case == "peer-report-imported" {
                    artifacts.mark_uploaded(&peer_report.id, "cloud-peer").unwrap();
                    artifacts.mark_committed(&peer_report.id, None).unwrap();
                }
            }
            "report-revoked" => {
                artifacts.revoke(&published.id, "nocode-supervisor-session", "Report withdrawn").unwrap();
            }
            "report-republished" | "notes-marker" => {
                let republished = publish_report(&f, &cloud, id, "report v2\n").await;
                if case == "notes-marker" {
                    let marker = format!(
                        "NO-CODE REVIEW BOUND dispatch {}: {PROOF} artifacts={}:{}",
                        reviewed.id, republished.id, republished.sha256,
                    );
                    let noted = call(&f.worker, serde_json::json!({
                        "action": "notes", "id": id, "note_type": "progress", "notes": marker,
                    })).await;
                    assert!(f.task(id).notes.contains(&marker), "{case}: {noted}");
                }
            }
            _ => {}
        }
        let revised = "https://example.test/deploys/revised";
        let root = f.cas_dir();
        match case {
            "reference-revised-during-close" => lifecycle_race_hook::before_terminal_write(id, move || {
                let store = open_task_store(&root).unwrap();
                let loaded = store.get(id).unwrap();
                let mut edited = loaded.clone();
                edited.external_ref = Some(revised.into());
                store.update_if_current(&edited, &loaded).unwrap();
            }),
            "republished-during-final-write" => {
                let republish = stage_report(&f, &cloud, id, "report v2\n").await;
                lifecycle_race_hook::before_terminal_cas(id, move || { republish(); });
            }
            "revoked-during-update-close" => {
                let report = published.id.clone();
                lifecycle_race_hook::before_terminal_write(id, move || {
                    cas_store::SqliteArtifactStore::open(&root).unwrap()
                        .revoke(&report, "nocode-supervisor-session", "Report withdrawn").unwrap();
                });
            }
            _ => {}
        }
        let raced = matches!(
            case, "reference-revised-during-close" | "republished-during-final-write" | "revoked-during-update-close"
        );
        let request = if matches!(case, "update-route" | "revoked-during-update-close") {
            serde_json::json!({"action": "update", "id": id, "status": "closed"})
        } else {
            request
        };
        let proof_reference_revised = matches!(
            case, "approved" | "rejected" | "chore" | "demo-feature" | "reference-revised-before-launch"
        );
        let mut parked = f.task(id);
        if proof_reference_revised {
            parked.external_ref = Some("https://example.test/deploys/revised".into());
        }
        parked.status = TaskStatus::AwaitingMerge;
        f.put(&parked);
        let response = call(&f.supervisor, request).await;
        let closed = f.task(id).status == TaskStatus::Closed;
        let approval_still_covers = matches!(
            case, "unchanged" | "unpublished-draft" | "peer-report-imported" | "peer-report-importing" | "chore"
                | "active-dispatch-without-recorded-proof" | "legacy-mcp-close" | "update-route" | "withdrawn-after-close-then-replayed"
        );
        if closed != approval_still_covers {
            wrong.push(format!("{case}: closed={closed}: {response}"));
        } else if case == "reference-revised-during-close" && f.task(id).external_ref.as_deref() != Some(revised) {
            wrong.push(format!("{case}: the concurrent reference was overwritten: {response}"));
        } else if case == "withdrawn-after-close-then-replayed" {
            artifacts.revoke(&published.id, "nocode-supervisor-session", "Report withdrawn").unwrap();
            let replay = call(&f.supervisor, serde_json::json!({"action": "close", "id": id, "reason": "Report published"})).await;
            let review = cas_store::get_verification_dispatch(&f.cas_dir(), &reviewed.id).unwrap().state;
            if review != cas_types::VerificationDispatchState::Resolved || f.task(id).status != TaskStatus::Closed {
                wrong.push(format!("{case}: replay left the review {review:?}: {replay}"));
            }
        } else if !closed && !raced && case != "rejected"
            && cas_store::get_latest_verification_dispatch(&f.cas_dir(), id).unwrap().unwrap().id == reviewed.id
        {
            wrong.push(format!("{case}: no new review dispatched: {response}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// cas-b38a: a code methodology keeps the evidence measurement. A test-first
/// task with no commit of its own is not closed on evidence alone.
#[tokio::test]
async fn evidence_does_not_close_a_code_task_without_its_commits_cas_b38a() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let id = "cas-nc09";
    let mut task = parked_ops_task(id);
    task.execution_note = Some("test-first".into());
    f.put(&task);
    let proof = f.artifact(id);
    let response = call(
        &f.supervisor,
        serde_json::json!({
            "action": "close", "id": id, "reason": "Promotion merged and deployed",
            "evidence_only": true, "evidence_only_artifact_path": proof,
            "evidence_only_reference": PR,
        }),
    )
    .await;
    assert!(
        response.contains("EVIDENCE ONLY CLOSE REJECTED"),
        "{response}"
    );
    assert_eq!(f.task(id).status, TaskStatus::AwaitingMerge);
}

/// Park accepts a pushed delivery without local fast-row receipts; CI owns
/// those checks. The normal merge requirement still applies.
#[tokio::test]
async fn park_accepts_delivery_without_fast_rows_receipt_cas_51ff() {
    for union_during_park in [false, true] {
        let mut env = TestEnvGuard::temp_home();
        let f = fixture(&mut env, "main");
        let worker_path = f.cas_dir().join("worktrees").join(WORKER);
        std::fs::create_dir_all(worker_path.join("scripts")).unwrap();
        std::fs::write(worker_path.join("scripts/fast-rows-pre-push.sh"), "exit 1\n").unwrap();
        std::fs::write(worker_path.join("feature.rs"), "pub fn feature() {}\n").unwrap();
        git(&worker_path, &["add", "."]);
        git(&worker_path, &["commit", "-q", "-m", "feat(cas-fr02): the feature"]);
        git(&worker_path, &["push", "-q", "--no-verify", "origin", LANE]);
        let id = "cas-fr02";
        let mut task = assigned(id, TaskType::Task, "main");
        task.notes = "Unstamped original context\n\n[2026-01-02 00:00] Worker progress".into();
        f.put(&task);
        claim(&f, id);
        if union_during_park {
            let root = f.cas_dir();
            lifecycle_race_hook::before_park(id, move || {
                let store = crate::store::open_task_store_local(&root).unwrap();
                let before = store.get(id).unwrap();
                let mut merged = before.clone();
                merged.notes = crate::cloud::syncer::pull::merge_task_notes(
                    &before.notes, "[2026-01-01 00:00] Receipt imported from another machine",
                );
                merged.updated_at = chrono::Utc::now();
                assert!(store.update_from_sync(&merged, &before).unwrap().is_some());
                let current = serde_json::to_value(store.get(id).unwrap()).unwrap();
                let before = serde_json::to_value(before).unwrap();
                let changed: Vec<_> = before.as_object().unwrap().iter()
                    .filter_map(|(key, value)| (current.get(key) != Some(value)).then_some(key.as_str()))
                    .collect();
                assert_eq!(changed, ["notes", "updated_at"], "audit the actual sync write");
            });
        }
        let close = serde_json::json!({"action": "close", "id": id, "reason": "Delivered"});

        let parked = call(&f.worker, close.clone()).await;
        assert!(parked.lines().any(crate::mcp::tools::core::task::lifecycle::close_ops::gate_preflight::is_merge_handoff), "{parked}");
        assert_eq!(f.task(id).status, TaskStatus::AwaitingMerge, "{parked}");
        if union_during_park {
            assert!(f.task(id).notes.contains("Receipt imported from another machine"));
            assert!(f.task(id).notes.contains("Unstamped original context"));
        }

        let again = call(&f.worker, close).await;
        assert!(again.lines().any(crate::mcp::tools::core::task::lifecycle::close_ops::gate_preflight::is_merge_handoff), "unmerged work must not close: {again}");
        assert_ne!(f.task(id).status, TaskStatus::Closed, "{again}");
    }
}

/// Release preparation used a suffixed promotion branch, while the worker's
/// plain lane still holds another task. The canonical task branch never existed.
fn promotion_receipt(f: &Fixture, id: &str) -> (String, String) {
    let mut task = assigned(id, TaskType::Chore, "main");
    task.execution_note = Some("no-code".into());
    task.external_ref = Some(PROOF.into());
    f.put(&task);
    claim(f, id);
    let promo = format!("{LANE}-{id}-promo");
    git(f.dir.path(), &["checkout", "-q", "-b", &promo, "main"]);
    std::fs::write(f.dir.path().join("release.md"), "promotion receipt\n").unwrap();
    git(f.dir.path(), &["add", "release.md"]);
    git(f.dir.path(), &["commit", "-q", "-m", &format!("chore({id}): prepare promotion")]);
    git(f.dir.path(), &["push", "-q", "origin", &promo]);
    let receipt = git(f.dir.path(), &["rev-parse", "HEAD"]);
    git(f.dir.path(), &["checkout", "-q", "main"]);
    git(f.dir.path(), &["merge", "-q", "--no-ff", &promo, "-m", &format!("Merge promotion {id}")]);
    git(f.dir.path(), &["push", "-q", "origin", "main"]);
    (receipt, promo)
}

#[tokio::test]
async fn merged_promotion_receipt_closes_without_canonical_branch_gh1202() {
    for remove_promo in [false, true] {
        let mut env = TestEnvGuard::temp_home();
        let f = fixture(&mut env, "main");
        let id = "cas-b302";
        let (receipt, promo) = promotion_receipt(&f, id);
        if remove_promo {
            git(f.dir.path(), &["branch", "-D", &promo]);
            git(f.dir.path(), &["push", "-q", "origin", "--delete", &promo]);
        }
        let response = call(&f.worker, serde_json::json!({
            "action": "close", "id": id, "reason": "Promotion shipped",
            "commit_receipt": receipt, "merged_into": "main", "external_ref": PROOF,
        })).await;
        assert_closed_without_lane(&f, id, &response);
        assert!(!f.task(id).deliverables.merge_conflicted, "{response}");
    }
}

#[tokio::test]
async fn wrong_target_promotion_receipt_refuses_before_parking_gh1202() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "staging");
    let id = "cas-b303";
    let (receipt, promo) = promotion_receipt(&f, id);
    git(f.dir.path(), &["branch", "-D", &promo]);
    git(f.dir.path(), &["push", "-q", "origin", "--delete", &promo]);
    let mut task = f.task(id);
    task.deliverables.work_target.as_mut().unwrap().target_branch = "staging".into();
    f.put(&task);
    let response = call(&f.worker, serde_json::json!({
        "action": "close", "id": id, "reason": "Promotion shipped to main",
        "commit_receipt": receipt, "merged_into": "main", "external_ref": PROOF,
    })).await;
    assert!(response.contains("not an ancestor of staging"), "{response}");
    let rejected = f.task(id);
    assert_eq!(rejected.status, TaskStatus::InProgress, "{response}");
    assert!(rejected.deliverables.parked_branch.is_none(), "{response}");
    assert!(rejected.deliverables.factory_branch_anchor.is_none(), "{response}");
    assert!(!rejected.deliverables.merge_conflicted, "{response}");
    assert!(!response.contains("merge-tree"), "{response}");
}

#[tokio::test]
async fn empty_promotion_receipt_refuses_before_parking_gh1202() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let id = "cas-b304";
    let (_, promo) = promotion_receipt(&f, id);
    git(f.dir.path(), &["branch", "-D", &promo]);
    git(f.dir.path(), &["push", "-q", "origin", "--delete", &promo]);
    git(f.dir.path(), &["commit", "-q", "--allow-empty", "-m", "empty receipt"]);
    let receipt = git(f.dir.path(), &["rev-parse", "HEAD"]);
    git(f.dir.path(), &["push", "-q", "origin", "main"]);
    let response = call(&f.worker, serde_json::json!({
        "action": "close", "id": id, "reason": "Promotion shipped",
        "commit_receipt": receipt, "merged_into": "main", "external_ref": PROOF,
    })).await;
    assert!(response.contains("empty file diff"), "{response}");
    let rejected = f.task(id);
    assert_eq!(rejected.status, TaskStatus::InProgress, "{response}");
    assert!(rejected.deliverables.parked_branch.is_none(), "{response}");
    assert!(rejected.deliverables.factory_branch_anchor.is_none(), "{response}");
    assert!(!rejected.deliverables.merge_conflicted, "{response}");
}

#[tokio::test]
async fn unresolved_delivery_without_promotion_receipt_does_not_park_gh1202() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let id = "cas-b305";
    f.put(&assigned(id, TaskType::Task, "main"));
    claim(&f, id);
    let response = call(&f.worker, serde_json::json!({
        "action": "close", "id": id, "reason": "Missing delivery",
    })).await;
    assert!(response.contains("DELIVERY BRANCH UNRESOLVED"), "{response}");
    let rejected = f.task(id);
    assert_eq!(rejected.status, TaskStatus::InProgress, "{response}");
    assert!(rejected.deliverables.parked_branch.is_none(), "{response}");
    assert!(rejected.deliverables.factory_branch_anchor.is_none(), "{response}");
    assert!(!rejected.deliverables.merge_conflicted, "{response}");
    assert!(!response.contains("merge-tree"), "{response}");
}

/// Starting read-only verification can allocate a task branch from a reused
/// lane. That branch still contains only another task's commits.
#[tokio::test]
async fn no_code_qa_on_foreign_task_branch_closes_on_portable_proof_gh1177() {
    for (target, reference, parked) in [
        ("main", PR, false),
        ("staging", "art-3ddfce77", false),
        ("staging", PR, true),
    ] {
        let mut env = TestEnvGuard::temp_home();
        let f = fixture(&mut env, target);
        let id = "cas-f177";
        let branch = format!("{LANE}-{id}");
        git(f.dir.path(), &["branch", &branch, LANE]);
        git(f.dir.path(), &["push", "-q", "origin", &branch]);
        let mut task = assigned(id, TaskType::Task, target);
        task.execution_note = Some("no-code".into());
        task.external_ref = Some(reference.into());
        if parked {
            task.status = TaskStatus::AwaitingMerge;
            task.deliverables.parked_branch = Some(branch);
            task.deliverables.factory_branch_anchor = Some(f.foreign_tip.clone());
            task.deliverables.merge_conflicted = true;
        }
        f.put(&task);
        claim(&f, id);
        let response = call(&f.worker, serde_json::json!({
            "action": "close", "id": id, "reason": "Read-only functional verification passed",
        })).await;
        assert_closed_without_lane(&f, id, &response);
        let closed = f.task(id);
        assert_eq!(closed.external_ref.as_deref(), Some(reference), "{response}");
        assert!(!closed.deliverables.merge_conflicted, "{response}");
    }
}

/// GH #2247: a supplemental read-only wait must resume without replacing the
/// exact proof already submitted for the no-code delivery.
#[tokio::test]
async fn blocked_supplemental_work_resumes_without_invalidating_proof_gh2247() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let mut config = crate::config::Config::load(&f.cas_dir()).unwrap();
    config.set("verification.enabled", "true").unwrap();
    config.save(&f.cas_dir()).unwrap();
    let id = "cas-bd3d";
    let mut task = assigned(id, TaskType::Task, "main");
    task.priority = cas_types::Priority(0);
    task.execution_note = Some("no-code".into());
    task.external_ref = Some(PROOF.into());
    f.put(&task);
    claim(&f, id);
    call(&f.worker, serde_json::json!({
        "action":"close", "id":id, "reason":"Read-only delivery submitted for review",
    })).await;
    let dispatch = cas_store::get_latest_verification_dispatch(&f.cas_dir(), id)
        .unwrap().expect("close must submit an exact verification boundary");
    let delivery = cas_store::get_latest_worker_delivery(&f.cas_dir(), id).unwrap();
    let before = f.task(id);
    call(&f.worker, serde_json::json!({"action":"update", "id":id, "status":"blocked"})).await;
    assert_eq!(f.task(id).status, TaskStatus::Blocked);

    // An operational resume must not reopen or change the reviewed scope.
    for update in [
        serde_json::json!({"action":"update", "id":id, "status":"open"}),
        serde_json::json!({"action":"update", "id":id, "status":"blocked", "title":"New scope"}),
    ] {
        let error = f.worker.task(Parameters(serde_json::from_value(update).unwrap())).await.unwrap_err();
        assert!(error.message.contains("[G-29]"), "{error:?}");
        assert_eq!(f.task(id).status, TaskStatus::Blocked);
    }
    // After the supervisor approves supplemental work, start resumes it
    // directly; findings stay in notes rather than replacing the proof.
    call(&f.supervisor, serde_json::json!({
        "action":"notes", "id":id, "note_type":"decision",
        "notes":"Supplemental read-only fixture approved within the existing scope",
    })).await;
    call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
    let resumed = f.task(id);
    assert_eq!(resumed.status, TaskStatus::InProgress);
    assert_eq!(resumed.external_ref, before.external_ref);
    assert_eq!(resumed.title, before.title);
    assert_eq!(serde_json::to_value(resumed.deliverables).unwrap(),
        serde_json::to_value(before.deliverables).unwrap());
    assert_eq!(serde_json::to_value(cas_store::get_latest_verification_dispatch(&f.cas_dir(), id).unwrap()).unwrap(),
        serde_json::to_value(Some(dispatch)).unwrap());
    assert_eq!(serde_json::to_value(cas_store::get_latest_worker_delivery(&f.cas_dir(), id).unwrap()).unwrap(),
        serde_json::to_value(delivery).unwrap());
}

#[tokio::test]
async fn no_code_qa_evidence_on_foreign_task_branch_measures_no_code_paths_gh1177() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "staging");
    let id = "cas-f178";
    let branch = format!("{LANE}-{id}");
    git(f.dir.path(), &["branch", &branch, LANE]);
    let mut task = assigned(id, TaskType::Task, "staging");
    task.execution_note = Some("no-code".into());
    task.external_ref = Some(PR.into());
    f.put(&task);
    let proof = f.artifact(id);
    let response = call(&f.supervisor, serde_json::json!({
        "action": "close", "id": id, "reason": "Read-only diagnosis retained as an artifact",
        "evidence_only": true, "evidence_only_artifact_path": proof,
        "evidence_only_reference": PR,
    })).await;
    assert_closed_without_lane(&f, id, &response);
    let evidence = f.task(id).deliverables.evidence_only.expect("evidence recorded");
    assert!(evidence.paths.is_empty(), "{:?}", evidence.paths);
}

#[tokio::test]
async fn no_code_task_with_own_task_branch_commit_still_refuses_gh1177() {
    for subject in ["fix(cas-f179): own delivery", "unnamed worker change"] {
        let mut env = TestEnvGuard::temp_home();
        let f = fixture(&mut env, "main");
        let id = "cas-f179";
        let branch = format!("{LANE}-{id}");
        git(f.dir.path(), &["checkout", "-q", "-b", &branch, LANE]);
        std::fs::write(f.dir.path().join("own.rs"), "pub fn own() {}\n").unwrap();
        git(f.dir.path(), &["add", "own.rs"]);
        git(f.dir.path(), &["commit", "-q", "-m", subject]);
        git(f.dir.path(), &["push", "-q", "origin", &branch]);
        git(f.dir.path(), &["checkout", "-q", "main"]);
        let mut task = assigned(id, TaskType::Task, "main");
        task.execution_note = Some("no-code".into());
        task.external_ref = Some(PR.into());
        f.put(&task);
        claim(&f, id);
        let response = call(&f.worker, serde_json::json!({
            "action": "close", "id": id, "reason": "Claims read-only verification",
        })).await;
        assert_ne!(f.task(id).status, TaskStatus::Closed, "{subject}: {response}");
        assert!(response.lines().any(crate::mcp::tools::core::task::lifecycle::close_ops::gate_preflight::is_merge_handoff), "{subject}: {response}");
    }
}

/// GH #1930: the real close dispatch's approved verdict must release the
/// no-code worker without a second close or a supervisor recovery turn.
#[tokio::test]
async fn approved_no_code_dispatch_closes_and_releases_claim_gh1930() {
    let mut env = TestEnvGuard::temp_home();
    for case in ["approved", "inline", "rejected", "foreign", "stale", "changed-proof"] {
        let f = fixture(&mut env, "main");
        let mut config = crate::config::Config::load(&f.cas_dir()).unwrap();
        config.set("verification.enabled", "true").unwrap();
        config.save(&f.cas_dir()).unwrap();
        env.set("CAS_FACTORY_SUPERVISOR_CLI", "codex");
        let id = "cas-ea5d";
        let mut task = assigned(id, TaskType::Spike, "main");
        if case != "inline" {
            task.execution_note = Some("no-code".into());
            task.external_ref = Some(PROOF.into());
        }
        f.put(&task);
        claim(&f, id);
        let mut request = serde_json::json!({
            "action":"close", "id":id, "reason":"Portable investigation report published",
        });
        if case == "inline" {
            request["execution_note"] = "no-code".into();
            request["external_ref"] = PROOF.into();
        }
        let pending = call(&f.worker, request.clone()).await;
        let dispatch = cas_store::get_latest_verification_dispatch(&f.cas_dir(), id)
            .unwrap().unwrap_or_else(|| panic!("{case}: {pending}"));
        assert_eq!(f.task(id).status, TaskStatus::InProgress, "{case}: {pending}");
        let agents = open_agent_store(&f.cas_dir()).unwrap();
        assert!(agents.get_lease(id).unwrap().is_some());
        let foreign = assigned("cas-other", TaskType::Spike, "main");
        f.put(&foreign);
        let foreign = f.task(&foreign.id);
        if case == "stale" {
            cas_store::timeout_verification_dispatch(&f.cas_dir(), id, dispatch.deadline_at).unwrap();
            let renewed = call(&f.worker, request).await;
            assert_ne!(cas_store::get_latest_verification_dispatch(&f.cas_dir(), id).unwrap().unwrap().id,
                dispatch.id, "{renewed}");
        } else if case == "changed-proof" {
            let mut changed = f.task(id);
            changed.external_ref = Some("artifact:replacement-report".into());
            f.put(&changed);
        }
        let latest = cas_store::get_latest_verification_dispatch(&f.cas_dir(), id).unwrap().unwrap();
        let approval = serde_json::json!({
            "action":"add", "task_id":if case == "foreign" { "cas-other" } else { id },
            "dispatch_id":dispatch.id,
            "status":if case == "rejected" { "rejected" } else { "approved" },
            "summary":"Portable report reviewed",
        });
        let result = f.supervisor.verification(Parameters(serde_json::from_value(approval.clone()).unwrap())).await;
        let closed = f.task(id);
        if case == "foreign" {
            assert!(result.is_err() || result.as_ref().unwrap().is_error == Some(true), "{case}: {result:?}");
        } else {
            assert!(!result.as_ref().unwrap().is_error.unwrap_or(false), "{case}: {result:?}");
        }
        if matches!(case, "foreign" | "stale") {
            assert_eq!(cas_store::get_latest_verification_dispatch(&f.cas_dir(), id).unwrap().unwrap(), latest);
            assert!(closed.pending_verification);
            if case == "stale" {
                // Historical review remains auditable without spending the
                // newer dispatch, clearing its pending flag or closing its task.
                let historical = cas_store::get_verification_for_dispatch(&f.cas_dir(), &dispatch.id).unwrap().unwrap();
                assert_eq!(historical.dispatch_id.as_deref(), Some(dispatch.id.as_str()));
                assert_eq!(historical.status, cas_types::VerificationStatus::Approved);
            }
        }
        if matches!(case, "approved" | "inline") {
            assert_eq!(closed.status, TaskStatus::Closed, "{case}: {result:?}");
            assert!(!closed.pending_verification);
            assert!(agents.get_lease(id).unwrap().is_none());
            assert!(closed.deliverables.no_delivery_close_intent.is_none());
            let history = agents.get_lease_history(id, None).unwrap();
            let queue = crate::store::open_prompt_queue_store(&f.cas_dir()).unwrap();
            let notices = queue.peek_all(50).unwrap().len();
            f.supervisor.verification(Parameters(serde_json::from_value(approval).unwrap())).await.unwrap();
            assert_eq!(f.task(id).closed_at, closed.closed_at);
            assert_eq!(agents.get_lease_history(id, None).unwrap().len(), history.len());
            assert_eq!(queue.peek_all(50).unwrap().len(), notices);
        } else {
            assert_ne!(closed.status, TaskStatus::Closed, "{case}: {result:?}");
            assert!(agents.get_lease(id).unwrap().is_some());
        }
        assert!(closed.deliverables.factory_branch_anchor.is_none());
        assert_eq!(serde_json::to_value(f.task("cas-other")).unwrap(), serde_json::to_value(&foreign).unwrap());
        assert_eq!(git(f.dir.path(), &["rev-parse", LANE]), f.foreign_tip);
    }
}

/// GH #2256: a peer review keeps its original empty source when a new
/// supervisor owns the Open task. Approval never erases reachable delivery.
#[tokio::test]
async fn approved_peer_review_no_delivery_missing_supervisor_lane_cas_e0ee() {
    let mut env = TestEnvGuard::temp_home();
    for case in ["unchanged", "missing", "foreign-tip", "remote-own", "bound-own"] {
        let f = fixture(&mut env, "main");
        let repo = f.dir.path();
        let id = "cas-e0ee";
        let source = if case == "foreign-tip" {
            "factory/nocode-worker-cas-e0ee"
        } else { "factory/peer-reviewer" };
        let side = "factory/earlier-review-work";
        let base = git(repo, &["rev-parse", "main"]);
        if case == "foreign-tip" {
            // cas-f8f8: the per-task source is gone, while the reusable
            // worker lane belongs to another task and must remain untouched.
            assert!(git(repo, &["for-each-ref", "--format=%(refname)",
                &format!("refs/heads/{source}"), &format!("refs/remotes/origin/{source}")]).is_empty());
            assert_eq!(git(repo, &["log", "-1", "--format=%s", LANE]), OTHER);
        } else {
            git(repo, &["branch", source, "main"]);
        }
        let own = if case.ends_with("own") {
            let checkout = f.cas_dir().join("worktrees").join(WORKER);
            git(&checkout, &["checkout", "-q", "-b", side, "main"]);
            git(&checkout, &["commit", "-q", "--allow-empty", "-m", "fix(cas-e0ee): earlier delivery"]);
            let own = git(&checkout, &["rev-parse", "HEAD"]);
            git(&checkout, &["push", "-q", "origin", side]);
            git(&checkout, &["checkout", "-q", LANE]);
            git(repo, &["branch", "-D", side]);
            Some(own)
        } else { None };
        if case == "missing" {
            git(repo, &["branch", "-D", source]);
        }
        let mut task = assigned(id, TaskType::Task, "main");
        task.status = TaskStatus::Open;
        let source_assignee = if case == "foreign-tip" { WORKER } else { "peer-reviewer" };
        task.assignee = Some(if case == "foreign-tip" { WORKER } else { "nocode-supervisor-session" }.into());
        task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
            work_target: task.deliverables.work_target.clone().unwrap(),
            assignee: source_assignee.into(), branch: source.into(), base_sha: base,
        });
        let original_start = task.deliverables.factory_branch_start.clone();
        f.put(&task);
        let mut config = crate::config::Config::load(&f.cas_dir()).unwrap();
        config.set("verification.enabled", "true").unwrap();
        config.save(&f.cas_dir()).unwrap();
        let boundary = if case == "bound-own" {
            cas_types::VerificationProofBoundary::task_at(
                super::super::repository_proof::capture_repository_proof_with_anchors(
                    repo, repo, vec![own.clone().unwrap()],
                ).unwrap(),
            )
        } else { cas_types::VerificationProofBoundary::task() };
        let dispatch = cas_store::create_verification_dispatch_bound(
            &f.cas_dir(), id, "nocode-supervisor-session", "nocode-supervisor-session",
            &boundary, chrono::Utc::now() + chrono::Duration::minutes(10), false,
        ).unwrap();
        let mut verdict = cas_types::Verification::approved(
            format!("ver-review-{case}"), id.into(), "Read-only review approved".into(),
        );
        verdict.provenance = cas_types::VerificationProvenance::SupervisorDirect;
        verdict.agent_id = Some("nocode-supervisor-session".into());
        verdict.issuer_agent_id = verdict.agent_id.clone();
        verdict.dispatch_id = Some(dispatch.id.clone());
        use cas_store::VerificationStore;
        cas_store::SqliteVerificationStore::open(&f.cas_dir()).unwrap().add(&verdict).unwrap();
        let conn = rusqlite::Connection::open(f.cas_dir().join("cas.db")).unwrap();
        cas_store::resolve_verification_dispatch_with_conn(
            &conn, &dispatch.id, "nocode-supervisor-session", None, true,
        ).unwrap();
        let response = call(&f.supervisor, serde_json::json!({
            "action":"close", "id":id, "no_delivery":true,
            "reason":"Peer review completed without code delivery",
        })).await;
        if let Some(own) = own {
            assert_eq!(f.task(id).status, TaskStatus::Open, "{case}: {response}");
            assert!(response.contains("G-47"), "{case}: {response}");
            // SHA and ref are actionable delivery data, independent of prose.
            assert!(response.contains(&own), "{case}: {response}");
            assert!(response.contains(&format!("refs/remotes/origin/{side}")), "{case}: {response}");
        } else {
            assert_closed_without_lane(&f, id, &response);
            assert_eq!(f.task(id).deliverables.factory_branch_start, original_start);
            assert!(f.task(id).deliverables.factory_branch_anchor.is_none());
            if case == "foreign-tip" {
                assert_eq!(git(repo, &["rev-parse", LANE]), f.foreign_tip);
                assert_eq!(git(repo, &["rev-parse", &format!("origin/{LANE}")]), f.foreign_tip);
            }
        }
    }
}

/// A reset starts at the target tip, without inheriting target history as
/// delivery evidence when the no-delivery verdict is consumed.
#[tokio::test]
async fn reset_reused_lane_no_delivery_closes_after_approval_cas_d4d8() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let repo = f.dir.path();
    let worker_path = f.cas_dir().join("worktrees").join(WORKER);
    git(repo, &["merge", "-q", "--ff-only", LANE]);
    git(repo, &["push", "-q", "origin", "main"]);
    let agents = open_agent_store(&f.cas_dir()).unwrap();
    let mut worker = agents.get("nocode-worker-session").unwrap();
    worker.metadata.insert("clone_path".into(), worker_path.display().to_string());
    agents.update(&worker).unwrap();
    let mut config = crate::config::Config::load(&f.cas_dir()).unwrap();
    config.set("verification.enabled", "true").unwrap();
    config.save(&f.cas_dir()).unwrap();
    env.set("CAS_FACTORY_SUPERVISOR_CLI", "codex");
    let id = "cas-d4d8";
    let mut task = assigned(id, TaskType::Task, "main");
    task.status = TaskStatus::Open;
    f.put(&task);
    let started = call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
    assert_eq!(f.task(id).status, TaskStatus::InProgress, "{started}");

    // A hold and target merge happen without any delivery from this task.
    let original = f.task(id).deliverables.factory_branch_start.unwrap();
    let mut held = f.task(id);
    held.status = TaskStatus::Open;
    f.put(&held);
    std::fs::write(repo.join("integrated.rs"), "pub fn integrated() {}\n").unwrap();
    git(repo, &["add", "integrated.rs"]);
    git(repo, &["commit", "-q", "-m", "fix(cas-a0b1): integrated sibling delivery"]);
    git(repo, &["push", "-q", "origin", "main"]);
    let target = git(repo, &["rev-parse", "HEAD"]);
    assert_ne!(original.base_sha, target);
    // The incident's target checkout lagged origin. Start resets the lane to
    // origin/main, while dispatch must not treat that integrated tip as work.
    git(repo, &["reset", "--hard", &original.base_sha]);
    assert_eq!(git(repo, &["rev-parse", "main"]), original.base_sha);
    assert_eq!(git(repo, &["rev-parse", "origin/main"]), target);
    let restarted = call(&f.worker, serde_json::json!({"action":"start", "id":id, "full":true})).await;
    assert!(restarted.contains("REUSED FACTORY BRANCH RESET"), "{restarted}");
    assert_eq!(git(&worker_path, &["rev-parse", "HEAD"]), target);

    let request = serde_json::json!({
        "action":"close", "id":id, "no_delivery":true,
        "reason":"Read-only diagnosis completed without delivery",
    });
    let pending = call(&f.worker, request.clone()).await;
    assert!(pending.contains("VERIFICATION REQUIRED"), "{pending}");
    let dispatch = cas_store::get_latest_verification_dispatch(&f.cas_dir(), id).unwrap().unwrap();
    let mut verdict = cas_types::Verification::approved(
        "ver-reset-no-delivery".into(), id.into(), "Read-only diagnosis approved".into(),
    );
    verdict.provenance = cas_types::VerificationProvenance::SupervisorDirect;
    verdict.agent_id = Some("nocode-supervisor-session".into());
    verdict.issuer_agent_id = verdict.agent_id.clone();
    verdict.dispatch_id = Some(dispatch.id.clone());
    use cas_store::VerificationStore;
    cas_store::SqliteVerificationStore::open(&f.cas_dir()).unwrap().add(&verdict).unwrap();
    let conn = rusqlite::Connection::open(f.cas_dir().join("cas.db")).unwrap();
    cas_store::resolve_verification_dispatch_with_conn(
        &conn, &dispatch.id, "nocode-supervisor-session", None, true,
    ).unwrap();
    let closed = call(&f.worker, request).await;
    assert_closed_without_lane(&f, id, &closed);
    assert_eq!(f.task(id).deliverables.factory_branch_start.unwrap().base_sha, target);
    assert!(dispatch.repository.unwrap().anchor_commits.is_empty());
}

/// GH #1673: retargeting an unmodified task lane must allow a supervisor to
/// bind its clean replacement before the worker has to commit any delivery.
#[tokio::test]
async fn retarget_empty_branch_rebind_then_start_gh1673() {
    for obstruction in ["old-commit", "historical-anchor", "dirty", "stale-target", "none"] {
        let mut env = TestEnvGuard::temp_home();
        let f = fixture(&mut env, "main");
        let repo = f.dir.path();
        let id = "cas-a4a7";
        let checkout = f.cas_dir().join("worktrees").join(WORKER);
        let lane = format!("{LANE}-{id}");
        git(&checkout, &["checkout", "-q", "-b", &lane, "main"]);
        let agents = open_agent_store(&f.cas_dir()).unwrap();
        let mut agent = agents.get("nocode-worker-session").unwrap();
        agent.metadata.insert("clone_path".into(), checkout.display().to_string());
        agents.update(&agent).unwrap();
        let mut task = assigned(id, TaskType::Bug, "main");
        task.status = TaskStatus::Open;
        f.put(&task);
        let started = call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
        assert_eq!(f.task(id).status, TaskStatus::InProgress, "{started}");
        let original = f.task(id).deliverables.factory_branch_start.unwrap();
        if obstruction == "old-commit" {
            std::fs::write(checkout.join("own.rs"), "pub fn own() {}\n").unwrap();
            git(&checkout, &["add", "own.rs"]);
            git(&checkout, &["commit", "-qm", "fix(cas-a4a7): existing work"]);
        }
        let old_tip = git(&checkout, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "-q", "-b", "new-target"]);
        std::fs::write(repo.join("target.rs"), "pub fn target() {}\n").unwrap();
        git(repo, &["add", "target.rs"]);
        git(repo, &["commit", "-qm", "target history"]);
        git(repo, &["push", "-q", "origin", "new-target"]);
        let target_tip = git(repo, &["rev-parse", "HEAD"]);
        let updated = call(&f.supervisor, serde_json::json!({
            "action":"update", "id":id, "proof_scope_fix":true,
            "target_branch":"new-target", "reason":"Move the task to its new release train",
        })).await;
        assert_eq!(f.task(id).status, TaskStatus::Open, "{updated}");
        let restarted = call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
        assert_eq!(f.task(id).status, TaskStatus::InProgress, "{restarted}");
        assert_eq!(f.task(id).deliverables.factory_branch_start.as_ref(), Some(&original));

        let replacement = format!("{lane}-retarget");
        let base = if obstruction == "stale-target" { "main" } else { "new-target" };
        git(&checkout, &["checkout", "-q", "-b", &replacement, base]);
        if obstruction == "dirty" {
            std::fs::write(checkout.join("target.rs"), "uncommitted work\n").unwrap();
        }
        if obstruction == "historical-anchor" {
            let mut task = f.task(id);
            task.deliverables.historical_factory_branch_anchors.push(old_tip.clone());
            f.put(&task);
        }
        let before = f.task(id);
        let rebound = f.supervisor.task(Parameters(serde_json::from_value(serde_json::json!({
            "action":"rebind_branch", "id":id, "branch":replacement,
            "reason":"Approve the empty replacement from the new target",
        })).unwrap())).await;
        if obstruction != "none" {
            assert!(rebound.is_err(), "{obstruction}: {rebound:?}");
            assert_eq!(serde_json::to_value(f.task(id).deliverables).unwrap(),
                serde_json::to_value(before.deliverables).unwrap(), "{obstruction}");
        } else {
            assert!(rebound.is_ok(), "{rebound:?}");
            let restarted = call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
            let task = f.task(id);
            assert_eq!(task.status, TaskStatus::InProgress, "{restarted}");
            let start = task.deliverables.factory_branch_start.unwrap();
            assert_eq!(start.branch, replacement);
            assert_eq!(start.base_sha, target_tip);
            assert_eq!(Some(start.work_target), task.deliverables.work_target);
            assert_eq!(start.assignee, WORKER);
            assert!(task.deliverables.factory_branch_anchor.is_none());
            assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), target_tip);
            // cas-db91 (GH #1698): close judges the rebound lane, not the original.
            // The approved empty replacement has no commits and is unpushed, so G-81
            // refuses it by name, where the original lane would be named instead.
            assert_eq!(close_measured_factory_branch(repo, &f.task(id), WORKER).trim_start_matches("origin/"), replacement);
            let MergeRealityOutcome::Refuse(message) = check_close_merge_reality(repo, &f.task(id), WORKER, "new-target") else {
                panic!("G-81 must judge the empty rebound lane");
            };
            assert!(message.contains(&replacement), "{message}");
        }
        assert_eq!(git(repo, &["rev-parse", &lane]), old_tip, "{obstruction}");
    }
}

/// GH #2157: reject a divergent base migration before the worker resets its
/// old lane, then execute the supported recovery and park its scoped delivery.
#[tokio::test]
async fn divergent_unparked_retarget_has_executable_recovery_gh2157() {
    let mut env = TestEnvGuard::temp_home();
    for obstruction in ["open", "clean", "dirty", "foreign"] {
        let f = fixture(&mut env, "main");
        let repo = f.dir.path();
        let fork = git(repo, &["rev-parse", "main"]);
        std::fs::write(repo.join("old.md"), "old integration history\n").unwrap();
        git(repo, &["add", "old.md"]);
        git(repo, &["commit", "-qm", "old integration"]);
        git(repo, &["push", "-q", "origin", "main"]);
        let checkout = f.cas_dir().join("worktrees").join(WORKER);
        let id = "cas-9122";
        let lane = format!("{LANE}-{id}");
        git(&checkout, &["checkout", "-q", "-b", &lane, "main"]);
        let agents = open_agent_store(&f.cas_dir()).unwrap();
        let mut worker = agents.get("nocode-worker-session").unwrap();
        worker.metadata.insert("clone_path".into(), checkout.display().to_string());
        agents.update(&worker).unwrap();
        let mut task = assigned(id, TaskType::Bug, "main");
        task.status = TaskStatus::Open;
        f.put(&task);
        call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
        if obstruction == "open" {
            call(&f.supervisor, serde_json::json!({"action":"update", "id":id, "status":"open"})).await;
            assert_eq!(f.task(id).status, TaskStatus::Open);
            assert!(f.task(id).deliverables.factory_branch_start.is_some());
        }
        if obstruction == "dirty" {
            std::fs::write(checkout.join("local.txt"), "unsaved work\n").unwrap();
        } else if obstruction == "foreign" {
            std::fs::write(checkout.join("foreign.txt"), "another task\n").unwrap();
            git(&checkout, &["add", "foreign.txt"]);
            git(&checkout, &["commit", "-qm", "fix(cas-dead): foreign work"]);
        }
        let old_tip = git(&checkout, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "-q", "-b", "develop", &fork]);
        std::fs::write(repo.join("new.md"), "new integration history\n").unwrap();
        git(repo, &["add", "new.md"]);
        git(repo, &["commit", "-qm", "new integration"]);
        git(repo, &["push", "-q", "origin", "develop"]);
        let target_tip = git(repo, &["rev-parse", "HEAD"]);
        let before = f.task(id);
        let result = f.supervisor.task(Parameters(serde_json::from_value(serde_json::json!({
            "action":"update", "id":id, "target_branch":"develop",
            "reason":"Supervisor authorizes resetting the clean lane onto develop",
        })).unwrap())).await;
        let recovery = if obstruction == "foreign" {
            // Nonempty retargets keep the old boundary and rely on explicit
            // rebind attribution; a metadata edit must not erase that work.
            result.unwrap();
            assert_eq!(f.task(id).deliverables.factory_branch_start, before.deliverables.factory_branch_start);
            serde_json::json!({
                "branch":format!("{lane}-retarget"), "base":"origin/develop",
                "rebind":{"action":"rebind_branch", "id":id, "branch":format!("{lane}-retarget"),
                    "reason":"Approve replacement without discarding foreign delivery"},
            })
        } else {
            assert!(result.is_err(), "divergent target must not leave a stale start: {result:?}");
            let error = result.unwrap_err();
            assert_eq!(error.data.as_ref().unwrap()["rule_id"], "unparked_target_base_migration");
            assert_eq!(serde_json::to_value(f.task(id).deliverables).unwrap(),
                serde_json::to_value(&before.deliverables).unwrap());
            error.data.as_ref().unwrap()["recovery"].clone()
        };
        if let Some(start) = recovery.get("start") {
            call(&f.worker, start.clone()).await;
            assert_eq!(f.task(id).status, TaskStatus::InProgress);
        }
        if let Some(target_update) = recovery.get("target_update") {
            call(&f.supervisor, target_update.clone()).await;
        }
        let replacement = recovery["branch"].as_str().unwrap();
        git(&checkout, &["checkout", "-q", "-b", replacement, recovery["base"].as_str().unwrap()]);
        let rebound = f.supervisor.task(Parameters(serde_json::from_value(recovery["rebind"].clone()).unwrap())).await;
        if matches!(obstruction, "dirty" | "foreign") {
            assert!(rebound.is_err(), "{obstruction}: {rebound:?}");
            assert_eq!(f.task(id).deliverables.factory_branch_start, before.deliverables.factory_branch_start);
            if obstruction == "dirty" {
                assert_eq!(std::fs::read(checkout.join("local.txt")).unwrap(), b"unsaved work\n");
            }
        } else {
            rebound.unwrap();
            call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
            let start = f.task(id).deliverables.factory_branch_start.unwrap();
            assert_eq!(start.base_sha, target_tip);
            assert_eq!(start.work_target.target_branch, "develop");
            std::fs::write(checkout.join("README.md"), "Scoped documentation delivery\n").unwrap();
            git(&checkout, &["add", "README.md"]);
            git(&checkout, &["commit", "-qm", "docs(cas-9122): scoped delivery"]);
            git(&checkout, &["push", "-q", "origin", replacement]);
            let closed = call(&f.worker, serde_json::json!({
                "action":"close", "id":id, "reason":"Scoped documentation verified",
            })).await;
            assert_eq!(f.task(id).status, TaskStatus::AwaitingMerge, "{closed}");
        }
        assert_eq!(git(repo, &["rev-parse", &lane]), old_tip, "old lane must be retained");
    }
}

/// Diagnostics expose the immutable source through the real task dispatcher.
#[tokio::test]
async fn start_and_show_report_recorded_source_without_rebinding_gh1407() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let repo = f.dir.path();
    let worker_path = f.cas_dir().join("worktrees").join(WORKER);
    git(repo, &["merge", "-q", "--ff-only", LANE]);
    git(repo, &["push", "-q", "origin", "main"]);
    let agents = open_agent_store(&f.cas_dir()).unwrap();
    let mut agent = agents.get("nocode-worker-session").unwrap();
    agent.metadata.insert("clone_path".into(), worker_path.display().to_string());
    agents.update(&agent).unwrap();
    let id = "cas-a1c3";
    let mut task = assigned(id, TaskType::Bug, "main");
    task.status = TaskStatus::Open;
    f.put(&task);
    let started = call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
    let recorded = f.task(id).deliverables.factory_branch_start.unwrap();
    assert!(started.contains(&format!("Recorded source branch: {LANE}")), "{started}");
    assert!(started.contains(&format!("Start base: {}", recorded.base_sha)), "{started}");
    assert!(!started.contains("rebind_branch"), "matching checkout needs no recovery: {started}");

    let replacement = format!("{LANE}-{id}");
    git(&worker_path, &["checkout", "-q", "-b", &replacement]);
    for full in [false, true] {
        let started = call(&f.worker, serde_json::json!({
            "action":"start", "id":id, "brief":!full,
        })).await;
        assert!(started.contains(&format!("Recorded source branch: {LANE}")), "{started}");
        assert!(started.contains(&format!("Start base: {}", recorded.base_sha)), "{started}");
        assert!(started.contains("WARNING") && started.contains(&replacement)
            && started.contains("supervisor") && started.contains("rebind_branch"), "{started}");
        let shown = call(&f.worker, serde_json::json!({"action":"show", "id":id, "full":full})).await;
        assert!(shown.contains(&format!("Recorded source branch: {LANE}")), "{shown}");
        assert!(shown.contains(&format!("Start base: {}", recorded.base_sha)), "{shown}");
        assert_eq!(f.task(id).deliverables.factory_branch_start.as_ref(), Some(&recorded));
        assert_eq!(git(&worker_path, &["branch", "--show-current"]), replacement);
    }
    let mut task = f.task(id);
    task.status = TaskStatus::AwaitingMerge;
    task.deliverables.parked_branch = Some(replacement.clone());
    f.put(&task);
    for full in [false, true] {
        let shown = call(&f.worker, serde_json::json!({"action":"show", "id":id, "full":full})).await;
        assert!(shown.contains(&format!("Recorded source branch: {LANE}")), "{shown}");
        assert!(shown.contains(&format!("Parked branch: {replacement}")), "{shown}");
    }
}

/// cas-db91 (GH #1698): after a supervisor rebinds a task to its rebuilt
/// `-r2` lane, close measures `-r2`, whether its tip is local or only on
/// origin, never the old per-task lane that still carries the first build.
/// G-81 judges the same lane. The empty rebind is covered through the real
/// handler in `retarget_empty_branch_rebind_then_start_gh1673`.
#[tokio::test]
async fn rebound_r2_lane_is_the_close_measured_branch_cas_db91() {
    for remote_only in [false, true] {
        let mut env = TestEnvGuard::temp_home();
        let f = fixture(&mut env, "main");
        let repo = f.dir.path();
        let checkout = f.cas_dir().join("worktrees").join(WORKER);
        let agents = open_agent_store(&f.cas_dir()).unwrap();
        let mut agent = agents.get("nocode-worker-session").unwrap();
        agent.metadata.insert("clone_path".into(), checkout.display().to_string());
        agents.update(&agent).unwrap();
        let id = "cas-1698";
        let lane = format!("{LANE}-{id}");
        git(&checkout, &["checkout", "-q", "-b", &lane, "main"]);
        let mut task = assigned(id, TaskType::Bug, "main");
        task.status = TaskStatus::Open;
        f.put(&task);
        let started = call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
        assert_eq!(f.task(id).status, TaskStatus::InProgress, "{started}");
        std::fs::write(checkout.join("first.rs"), "pub fn first() {}\n").unwrap();
        git(&checkout, &["add", "first.rs"]);
        git(&checkout, &["commit", "-q", "-m", "fix(cas-1698): first build"]);
        git(&checkout, &["push", "-q", "origin", &lane]);
        let rebuilt = format!("{lane}-r2");
        git(&checkout, &["checkout", "-q", "-b", &rebuilt, "main"]);
        std::fs::write(checkout.join("rebuilt.rs"), "pub fn rebuilt() {}\n").unwrap();
        git(&checkout, &["add", "rebuilt.rs"]);
        git(&checkout, &["commit", "-q", "-m", "fix(cas-1698): rebuilt delivery"]);
        git(&checkout, &["push", "-q", "origin", &rebuilt]);
        let rebound = f.supervisor.task(Parameters(serde_json::from_value(serde_json::json!({
            "action":"rebind_branch", "id":id, "branch":rebuilt, "reason":"Bind the rebuilt delivery",
        })).unwrap())).await;
        assert!(rebound.is_ok(), "{rebound:?}");
        if remote_only {
            git(&checkout, &["checkout", "-q", "--detach"]);
            git(repo, &["branch", "-D", &rebuilt]);
        }
        assert_eq!(close_measured_factory_branch(repo, &f.task(id), WORKER).trim_start_matches("origin/"),
            rebuilt, "remote_only={remote_only}");
        // G-81 judges that same lane: a published rebuilt tip proceeds.
        let reality = check_close_merge_reality(repo, &f.task(id), WORKER, "main");
        assert!(matches!(reality, MergeRealityOutcome::Proceed), "remote_only={remote_only}");
    }
}

/// cas-dae4 (GH #1579): a replacement spawned at the base after its
/// predecessor retired must reach a bound lane without a supervisor override.
/// An untouched predecessor lane (the handoff lives in the artifacts dir) binds
/// at start. A predecessor lane with commits is kept (cas-57df never discards a
/// pending delivery); the replacement starts, delivers, and the supervisor's
/// rebind then succeeds instead of refusing for want of a delivery.
#[tokio::test]
async fn replacement_reaches_a_bound_lane_without_override_cas_dae4() {
    for predecessor in ["untouched", "committed"] {
        let mut env = TestEnvGuard::temp_home();
        let f = fixture(&mut env, "main");
        let repo = f.dir.path();
        let worker_path = f.cas_dir().join("worktrees").join(WORKER);
        let agents = open_agent_store(&f.cas_dir()).unwrap();
        let mut agent = agents.get("nocode-worker-session").unwrap();
        agent.metadata.insert("clone_path".into(), worker_path.display().to_string());
        agents.update(&agent).unwrap();
        let id = "cas-1579";
        let base = git(repo, &["rev-parse", "main"]);
        git(repo, &["branch", "factory/old-worker", "main"]);
        if predecessor == "committed" {
            git(repo, &["checkout", "-q", "factory/old-worker"]);
            std::fs::write(repo.join("HANDOFF.md"), "context checkpoint\n").unwrap();
            git(repo, &["add", "HANDOFF.md"]);
            git(repo, &["commit", "-q", "-m", "chore(cas-1579): handoff"]);
            git(repo, &["checkout", "-q", "main"]);
        }
        let old_tip = git(repo, &["rev-parse", "factory/old-worker"]);
        git(repo, &["push", "-q", "origin", "factory/old-worker"]);
        let replacement = format!("{LANE}-{id}");
        git(&worker_path, &["checkout", "-q", "-b", &replacement, "main"]);
        let mut task = assigned(id, TaskType::Bug, "main");
        task.status = TaskStatus::Open;
        task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
            work_target: task.deliverables.work_target.clone().unwrap(),
            assignee: "old-worker".into(),
            branch: "factory/old-worker".into(),
            base_sha: base,
        });
        f.put(&task);
        let started = call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
        assert_eq!(f.task(id).status, TaskStatus::InProgress, "{predecessor}: {started}");
        let bound = |f: &Fixture| f.task(id).deliverables.factory_branch_start.map(|start| start.branch);
        if predecessor == "untouched" {
            assert_eq!(bound(&f).as_deref(), Some(replacement.as_str()), "{started}");
        } else {
            assert_eq!(bound(&f).as_deref(), Some("factory/old-worker"), "{started}");
            std::fs::write(worker_path.join("fix.rs"), "pub fn fix() {}\n").unwrap();
            git(&worker_path, &["add", "fix.rs"]);
            git(&worker_path, &["commit", "-q", "-m", "fix(cas-1579): replacement delivery"]);
            git(&worker_path, &["push", "-q", "origin", &replacement]);
            let rebound = f.supervisor.task(Parameters(serde_json::from_value(serde_json::json!({
                "action":"rebind_branch", "id":id, "branch":replacement,
                "reason":"Bind the replacement after its first delivery",
            })).unwrap())).await;
            assert!(rebound.is_ok(), "{rebound:?}");
            assert_eq!(bound(&f).as_deref(), Some(replacement.as_str()));
        }
        assert_eq!(git(repo, &["rev-parse", "factory/old-worker"]), old_tip, "{predecessor}: the old lane is kept");
    }
}

/// Start, rather than commit subject conventions, establishes the new task's
/// boundary on a reused lane. A foreign claim after that boundary still refuses.
#[tokio::test]
async fn reused_lane_start_attributes_unnamed_delivery_cas_ead0() {
    let mut env = TestEnvGuard::temp_home();
    for (lane, foreign_after_start, rebase_after_start) in [
        (LANE, false, false),
        (LANE, true, false),
        // cas-ff6c: a per-task lane is this task's own; start refuses
        // another task's per-task branch rather than reusing it.
        ("factory/nocode-worker-cas-ead0", false, false),
        ("factory/nocode-worker-cas-ead0", true, false),
        (LANE, false, true),
    ] {
        let f = fixture(&mut env, "main");
        let repo = f.dir.path();
        let worker_path = f.cas_dir().join("worktrees").join(WORKER);
        git(repo, &["merge", "-q", "--ff-only", LANE]);
        git(repo, &["push", "-q", "origin", "main"]);
        if lane != LANE {
            git(&worker_path, &["checkout", "-q", "-b", lane]);
        }
        let base = git(repo, &["rev-parse", "main"]);
        let agents = open_agent_store(&f.cas_dir()).unwrap();
        let mut agent = agents.get("nocode-worker-session").unwrap();
        agent
            .metadata
            .insert("clone_path".into(), worker_path.display().to_string());
        agents.update(&agent).unwrap();
        let id = "cas-ead0";
        let mut task = assigned(id, TaskType::Bug, "main");
        task.status = TaskStatus::Open;
        f.put(&task);
        let started = call(&f.worker, serde_json::json!({"action": "start", "id": id})).await;
        assert_eq!(f.task(id).status, TaskStatus::InProgress, "{started}");
        assert_eq!(git(&worker_path, &["rev-parse", "HEAD"]), base);
        let boundary = f
            .task(id)
            .deliverables
            .factory_branch_start
            .expect("start records lane boundary");
        assert_eq!(
            (&boundary.base_sha, boundary.branch.as_str()),
            (&base, lane)
        );
        for (file, subject) in [
            ("one.rs", "fix(hub): isolate worker cgroups"),
            ("two.rs", "fix(hub): preserve generation ownership"),
        ] {
            std::fs::write(worker_path.join(file), "pub fn changed() {}\n").unwrap();
            git(&worker_path, &["add", file]);
            git(&worker_path, &["commit", "-q", "-m", subject]);
        }
        // No commit hook runs in this fixture: start must suffice on its own,
        // and an administrative re-start must not move its graph boundary.
        assert!(f.task(id).deliverables.factory_branch_anchor.is_none());
        call(&f.worker, serde_json::json!({"action": "start", "id": id})).await;
        assert_eq!(
            f.task(id).deliverables.factory_branch_start.as_ref(),
            Some(&boundary)
        );
        if rebase_after_start {
            // A target update is already integrated work, even when a rebase
            // puts its ordinary foreign-named commit after our start boundary.
            std::fs::write(repo.join("target.rs"), "pub fn integrated() {}\n").unwrap();
            git(repo, &["add", "target.rs"]);
            git(repo, &["commit", "-q", "-m", "fix(cas-a0b1): target delivery"]);
            git(repo, &["push", "-q", "origin", "main"]);
            git(&worker_path, &["rebase", "origin/main"]);
        }
        if foreign_after_start {
            std::fs::write(worker_path.join("foreign.rs"), "pub fn foreign() {}\n").unwrap();
            git(&worker_path, &["add", "foreign.rs"]);
            git(
                &worker_path,
                &[
                    "commit",
                    "-q",
                    "-m",
                    "fix(cas-a0b1): another delivery\n\nContext: cas-ead0",
                ],
            );
        }
        let tip = git(&worker_path, &["rev-parse", "HEAD"]);
        git(&worker_path, &["push", "-q", "origin", lane]);
        let response = call(
            &f.worker,
            serde_json::json!({
                "action": "close", "id": id, "reason": "Delivery ready for review",
            }),
        )
        .await;
        let parked = f.task(id);
        if foreign_after_start {
            assert!(
                response.contains("DELIVERY BRANCH UNRESOLVED"),
                "{response}"
            );
            assert!(
                response.contains(&tip),
                "refusal must identify foreign commit: {response}"
            );
            assert!(
                parked.deliverables.factory_branch_anchor.is_none(),
                "{response}"
            );
            assert_ne!(parked.status, TaskStatus::Closed, "{response}");
        } else {
            assert!(response.lines().any(crate::mcp::tools::core::task::lifecycle::close_ops::gate_preflight::is_merge_handoff), "{response}");
            assert_eq!(
                parked.deliverables.factory_branch_anchor.as_deref(),
                Some(tip.as_str())
            );
            assert_eq!(parked.deliverables.parked_branch.as_deref(), Some(lane));
            assert_eq!(parked.status, TaskStatus::AwaitingMerge, "{response}");
        }
    }
}

/// cas-aa98: a no-code task delivers no commits, so the worker's next task
/// committing on the same lane is not foreign work in this task's delivery.
/// On 2026-10-27 cas-421d refused "DELIVERY BRANCH UNRESOLVED" until its
/// worker reset the lane, then restored its in-progress work.
#[tokio::test]
async fn no_code_close_survives_the_next_task_on_its_lane_cas_aa98() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let repo = f.dir.path();
    let worker_path = f.cas_dir().join("worktrees").join(WORKER);
    git(repo, &["merge", "-q", "--ff-only", LANE]);
    git(repo, &["push", "-q", "origin", "main"]);
    let agents = open_agent_store(&f.cas_dir()).unwrap();
    let mut agent = agents.get("nocode-worker-session").unwrap();
    agent.metadata.insert("clone_path".into(), worker_path.display().to_string());
    agents.update(&agent).unwrap();
    let id = "cas-aa98";
    let mut task = assigned(id, TaskType::Task, "main");
    task.status = TaskStatus::Open;
    task.execution_note = Some("no-code".into());
    task.external_ref = Some(PR.into());
    f.put(&task);
    let started = call(&f.worker, serde_json::json!({"action": "start", "id": id})).await;
    assert!(f.task(id).deliverables.factory_branch_start.is_some(), "{started}");
    std::fs::write(worker_path.join("next.rs"), "pub fn next() {}\n").unwrap();
    git(&worker_path, &["add", "next.rs"]);
    git(&worker_path, &["commit", "-q", "-m", "feat(cas-b7c0): the worker's next task"]);
    git(&worker_path, &["push", "-q", "origin", LANE]);
    let response = call(&f.worker, serde_json::json!({
        "action": "close", "id": id, "reason": "Read-only QA ledger published",
    })).await;
    assert_closed_without_lane(&f, id, &response);
}

/// GH #1983: syncing released trunk for report tooling is not an artifact-only
/// task's delivery. A subsequent unpublished commit still belongs to its lane.
#[tokio::test]
async fn artifact_only_close_after_released_trunk_sync_gh1983() {
    for own_commit in [false, true] {
        let mut env = TestEnvGuard::temp_home();
        let f = fixture(&mut env, "epic/artifact");
        let repo = f.dir.path();
        let checkout = f.cas_dir().join("worktrees").join(WORKER);
        git(&checkout, &["reset", "-q", "--hard", "epic/artifact"]);
        let agents = open_agent_store(&f.cas_dir()).unwrap();
        let mut worker = agents.get("nocode-worker-session").unwrap();
        worker.metadata.insert("clone_path".into(), checkout.display().to_string());
        agents.update(&worker).unwrap();
        let id = "cas-a1983";
        let mut task = assigned(id, TaskType::Task, "epic/artifact");
        task.status = TaskStatus::Open;
        task.execution_note = Some("no-code".into());
        task.external_ref = Some("artifact:art-1983".into());
        f.put(&task);
        let started = call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
        assert_eq!(f.task(id).status, TaskStatus::InProgress, "{started}");
        git(repo, &["merge", "-q", "--ff-only", &f.foreign_tip]);
        std::fs::write(repo.join("parser.rs"), "pub fn parser() {}\n").unwrap();
        git(repo, &["add", "parser.rs"]);
        git(repo, &["commit", "-qm", "Repair report parser"]);
        git(repo, &["push", "-q", "origin", "main"]);
        git(&checkout, &["merge", "-q", "--ff-only", "origin/main"]);
        if own_commit {
            std::fs::write(checkout.join("own.rs"), "pub fn own() {}\n").unwrap();
            git(&checkout, &["add", "own.rs"]);
            git(&checkout, &["commit", "-qm", "Report tooling changes"]);
        }
        let branch = git(&checkout, &["branch", "--show-current"]);
        let tip = git(&checkout, &["rev-parse", "HEAD"]);
        git(&checkout, &["push", "-q", "origin", &branch]);
        let response = call(&f.worker, serde_json::json!({
            "action":"close", "id":id, "reason":"Report published with portable artifact proof",
        })).await;
        if own_commit {
            assert_ne!(f.task(id).status, TaskStatus::Closed, "{response}");
        } else {
            assert_closed_without_lane(&f, id, &response);
        }
        assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), tip);
    }
}

/// GH #2022: read-only work in artifacts does not own the shared checkout.
/// Starting it must preserve another worker's branch and uncommitted edits.
#[tokio::test]
async fn no_code_start_preserves_the_shared_checkout_gh2022() {
    for (branch, no_code) in [("main", true), ("factory/other-cas-2022", true), ("factory/other-cas-2022", false)] {
        let mut env = TestEnvGuard::temp_home();
        let f = fixture(&mut env, "main");
        let repo = f.dir.path();
        if branch != "main" {
            git(repo, &["switch", "-qc", branch]);
        }
        std::fs::write(repo.join(".gitignore"), ".cas/\ndurable-artifacts/\nshared-edits/\n").unwrap();
        let before = git(repo, &["status", "--porcelain"]);
        let tip = git(repo, &["rev-parse", "HEAD"]);
        let agents = open_agent_store(&f.cas_dir()).unwrap();
        let mut worker = agents.get("nocode-worker-session").unwrap();
        worker.metadata.insert("clone_path".into(), repo.display().to_string());
        agents.update(&worker).unwrap();
        let id = "cas-a2022";
        let mut task = assigned(id, TaskType::Task, "main");
        task.status = TaskStatus::Open;
        if no_code {
            task.execution_note = Some("no-code".into());
            task.external_ref = Some(PROOF.into());
        }
        f.put(&task);
        let result = f.worker.task(Parameters(serde_json::from_value(serde_json::json!({
            "action":"start", "id":id,
        })).unwrap())).await;
        if no_code {
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(f.task(id).status, TaskStatus::InProgress, "{result:?}");
            assert!(f.task(id).deliverables.factory_branch_start.is_none());
        } else {
            assert!(result.is_err(), "{result:?}");
            assert_eq!(f.task(id).status, TaskStatus::Open);
        }
        assert_eq!(git(repo, &["rev-parse", "HEAD"]), tip);
        assert_eq!(git(repo, &["branch", "--show-current"]), branch);
        assert_eq!(git(repo, &["status", "--porcelain"]), before);
    }
}

/// A legacy start recorded the preceding parked task's lane before the worker
/// switched to its own lane. Only a proven integrated delivery can recover it;
/// neither an arbitrary receipt nor supervisor review erases delivery checks.
#[tokio::test]
async fn integrated_receipt_recovers_a_previous_tasks_start_lane_cas_8603() {
    let mut env = TestEnvGuard::temp_home();
    for (supervisor, same_lane) in [(false, false), (true, false), (false, true), (true, true)] {
        let f = fixture(&mut env, "main");
        let repo = f.dir.path();
        let worker_path = f.cas_dir().join("worktrees").join(WORKER);
        let id = "cas-8603";
        let old_lane = format!("{LANE}-cas-a0b1");
        let own_lane = format!("{LANE}-{id}");
        // The old delivery remains an immutable object, but its lane was
        // reset after the next start. Do not repair it by grafting our work.
        git(repo, &["branch", &old_lane, "main"]);
        let old_tip = git(repo, &["rev-parse", &old_lane]);
        git(&worker_path, &["checkout", "-q", "-b", &own_lane, "main"]);
        std::fs::write(worker_path.join("own.rs"), "pub fn own() {}\n").unwrap();
        git(&worker_path, &["add", "own.rs"]);
        git(&worker_path, &["commit", "-q", "-m", "fix(cas-8603): own delivery"]);
        let receipt = git(&worker_path, &["rev-parse", "HEAD"]);
        git(&worker_path, &["push", "-q", "origin", &own_lane]);
        git(repo, &["merge", "-q", "--no-ff", &own_lane, "-m", "Merge own delivery"]);
        git(repo, &["merge", "-q", "--no-ff", LANE, "-m", "Merge preceding delivery"]);
        git(repo, &["commit", "-q", "--allow-empty", "-m", "fix(cas-8603): empty receipt"]);
        let empty = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["push", "-q", "origin", "main"]);
        git(repo, &["checkout", "-q", "-b", "unmerged", "main"]);
        std::fs::write(repo.join("later.rs"), "pub fn later() {}\n").unwrap();
        git(repo, &["add", "later.rs"]);
        git(repo, &["commit", "-q", "-m", "fix(cas-8603): not integrated"]);
        let unmerged = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "-q", "main"]);
        let mut task = assigned(id, TaskType::Bug, "main");
        task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
            work_target: task.deliverables.work_target.clone().unwrap(),
            assignee: WORKER.into(),
            branch: if same_lane { own_lane.clone() } else { old_lane.clone() },
            base_sha: f.foreign_tip.clone(),
        });
        f.put(&task);
        claim(&f, id);
        let caller = if supervisor { &f.supervisor } else { &f.worker };
        for rejected in [None, Some(empty.as_str()), Some(unmerged.as_str()), Some(f.foreign_tip.as_str())] {
            let response = call(caller, serde_json::json!({
                "action": "close", "id": id, "reason": "Reviewed delivery",
                "commit_receipt": rejected, "supervisor_override": supervisor,
            })).await;
            assert_ne!(f.task(id).status, TaskStatus::Closed, "{supervisor}/{rejected:?}: {response}");
            assert_eq!(f.task(id).deliverables.factory_branch_start, task.deliverables.factory_branch_start,
                "a refused receipt must not erase the start record: {response}");
        }
        let response = call(caller, serde_json::json!({
            "action": "close", "id": id, "reason": "Reviewed integrated own delivery",
            "commit_receipt": receipt, "supervisor_override": supervisor,
        })).await;
        if same_lane {
            assert_ne!(f.task(id).status, TaskStatus::Closed, "an own-lane history rewrite still refuses: {response}");
            assert_eq!(f.task(id).deliverables.factory_branch_start, task.deliverables.factory_branch_start);
        } else {
            assert_eq!(f.task(id).status, TaskStatus::Closed, "{supervisor}: {response}");
            assert!(f.task(id).notes.contains(&f.foreign_tip), "the original boundary must remain audited");
        }
        assert_eq!(git(repo, &["rev-parse", &old_lane]), old_tip,
            "recovery must not move the previous task's lane");
    }
}

/// Re-close a delivery after another task reused the bare worker lane, with
/// either its local parked ref or a recorded noncanonical delivery ref.
#[tokio::test]
async fn integrated_parked_receipt_survives_reused_lane_cas_79fc() {
    let mut env = TestEnvGuard::temp_home();
    for (supervisor, recorded, with_receipt, valid_anchor) in [
        (false, false, true, true), (true, false, true, true),
        (false, true, true, true), (true, true, true, true),
        (false, false, false, true), (true, false, false, true),
        (false, true, false, true), (true, true, false, true),
        (false, true, false, false),
    ] {
        let f = fixture(&mut env, "main");
        env.set("CAS_AGENT_ROLE", if supervisor { "supervisor" } else { "worker" });
        let repo = f.dir.path();
        let worker_path = f.cas_dir().join("worktrees").join(WORKER);
        let id = "cas-79fc";
        let parked = if recorded { format!("{LANE}-cas-aabb") } else { format!("{LANE}-{id}") };
        let base = git(repo, &["rev-parse", "main"]);
        git(&worker_path, &["reset", "--hard", "main"]);
        std::fs::write(worker_path.join("own.rs"), "pub fn own() {}\n").unwrap();
        git(&worker_path, &["add", "own.rs"]);
        git(&worker_path, &["commit", "-q", "-m", "fix(cas-79fc): parked delivery"]);
        let receipt = git(&worker_path, &["rev-parse", "HEAD"]);
        git(repo, &["branch", &parked, &receipt]);
        git(repo, &["merge", "-q", "--no-ff", LANE, "-m", "Merge parked delivery"]);
        git(repo, &["push", "-q", "origin", "main"]);
        assert!(!git_ref_exists(repo, &format!("origin/{parked}")), "parked ref was never pushed");
        if recorded {
            // Legacy start used a different task's suffixed name. The
            // unused bare lane is empty and never published afterwards.
            git(&worker_path, &["reset", "--hard", &base]);
            git(repo, &["push", "-q", "origin", "--delete", LANE]);
        } else {
            std::fs::write(worker_path.join("next.rs"), "pub fn next() {}\n").unwrap();
            git(&worker_path, &["add", "next.rs"]);
            git(&worker_path, &["commit", "-q", "-m", "fix(cas-0ddf): next task"]);
        }
        let next_tip = git(&worker_path, &["rev-parse", "HEAD"]);
        let mut task = assigned(id, TaskType::Bug, "main");
        let start_base = if recorded { f.foreign_tip.clone() } else { base.clone() };
        task.deliverables.factory_branch_start = with_receipt.then(|| cas_types::FactoryBranchStart {
            work_target: task.deliverables.work_target.clone().unwrap(),
            assignee: WORKER.into(), branch: LANE.into(), base_sha: start_base.clone(),
        });
        task.deliverables.parked_branch = Some(parked.clone());
        task.deliverables.factory_branch_anchor = Some(if valid_anchor { receipt.clone() } else { base });
        f.put(&task);
        claim(&f, id);
        if recorded && !with_receipt {
            // Without a recovery receipt, the hook still requires the
            // checkout to contain this task's anchor. Keep that guard intact.
            git(&worker_path, &["checkout", "-q", &parked]);
        }
        let caller = if supervisor { &f.supervisor } else { &f.worker };
        let response = call(caller, serde_json::json!({
            "action": "close", "id": id, "reason": "Integrated parked delivery",
            "commit_receipt": with_receipt.then_some(receipt.as_str()), "supervisor_override": supervisor,
        })).await;
        if !valid_anchor {
            assert_ne!(f.task(id).status, TaskStatus::Closed, "an empty anchor is not delivery proof: {response}");
            assert!(response.contains("MERGE REALITY CHECK FAILED"), "worker B2 must still refuse: {response}");
            continue;
        }
        assert_eq!(f.task(id).status, TaskStatus::Closed, "{supervisor}/{recorded}/{with_receipt}: {response}");
        if with_receipt {
            assert!(f.task(id).notes.contains(&start_base), "old base must remain audited");
        }
        assert_eq!(resolve_close_delivery_branch(repo, &f.task(id), WORKER).unwrap(), parked,
            "the recorded anchored delivery must outrank the worker lane");
        assert_eq!(git(repo, &["rev-parse", &parked]), receipt);
        assert_eq!(git(repo, &["rev-parse", LANE]), next_tip, "never move the next task's lane");
    }
}

/// cas-3d57 (cas-985f follow-ups), through the close handler: a worker on an
/// epic-bound lane merges released origin/main (no rebase) and parks; after
/// park it merges a newer origin/main and re-parks, both without override.
/// A local `main` ahead of origin is not released: merging it is refused.
#[tokio::test]
async fn merging_released_main_parks_and_re_parks_but_local_main_is_not_released_cas_3d57() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "epic/next");
    let repo = f.dir.path();
    let worker_path = f.cas_dir().join("worktrees").join(WORKER);
    let id = "cas-3d57";
    let lane = format!("{LANE}-{id}");
    git(&worker_path, &["checkout", "-q", "-b", &lane, "origin/epic/next"]);
    let agents = open_agent_store(&f.cas_dir()).unwrap();
    let mut agent = agents.get("nocode-worker-session").unwrap();
    agent.metadata.insert("clone_path".into(), worker_path.display().to_string());
    agents.update(&agent).unwrap();
    let mut task = assigned(id, TaskType::Bug, "epic/next");
    task.status = TaskStatus::Open;
    f.put(&task);
    let started = call(&f.worker, serde_json::json!({"action": "start", "id": id})).await;
    assert!(f.task(id).deliverables.factory_branch_start.is_some(), "{started}");
    std::fs::write(worker_path.join("own.rs"), "pub fn own() {}\n").unwrap();
    git(&worker_path, &["add", "own.rs"]);
    git(&worker_path, &["commit", "-q", "-m", "fix(hub): own delivery"]);
    let close = serde_json::json!({"action": "close", "id": id, "reason": "Delivery ready"});
    // Another task's work, released on main and merged in as instructed.
    let merge_main = |file: &str, subject: &str, push: bool| {
        std::fs::write(repo.join(file), "pub fn released() {}\n").unwrap();
        git(repo, &["add", file]);
        git(repo, &["commit", "-q", "-m", subject]);
        if push { git(repo, &["push", "-q", "origin", "main"]); }
        git(&worker_path, &["fetch", "-q", "origin"]);
        git(&worker_path, &["merge", "-q", "--no-ff", "--no-edit", if push { "origin/main" } else { "main" }]);
        git(&worker_path, &["push", "-q", "origin", &lane]);
        git(&worker_path, &["rev-parse", "HEAD"])
    };
    for (round, (file, subject)) in [("one.rs", "fix(cas-a0b1): released before park"),
        ("two.rs", "fix(cas-a0b2): released after park")].into_iter().enumerate() {
        let tip = merge_main(file, subject, true);
        let response = call(&f.worker, close.clone()).await;
        let parked = f.task(id);
        assert!(response.lines().any(crate::mcp::tools::core::task::lifecycle::close_ops::gate_preflight::is_merge_handoff), "round {round}: {response}");
        assert_eq!(parked.status, TaskStatus::AwaitingMerge, "round {round}: {response}");
        assert_eq!(parked.deliverables.factory_branch_anchor.as_deref(), Some(tip.as_str()), "round {round}");
    }
    let parked_anchor = f.task(id).deliverables.factory_branch_anchor;
    // An own correction before the foreign merge remains attributable even
    // when a sibling task later becomes in progress on this worker.
    std::fs::write(worker_path.join("own.rs"), "pub fn own() { /* before foreign */ }\n").unwrap();
    git(&worker_path, &["add", "own.rs"]);
    git(&worker_path, &["commit", "-qm", "fix(cas-3d57): correction before next task"]);
    let own_correction = git(&worker_path, &["rev-parse", "HEAD"]);
    let offending = merge_main("three.rs", "fix(cas-0e2b): unreleased, only on local main", false);
    // Keep the offending merge distinct from the ignored current tip.
    std::fs::write(worker_path.join("own.rs"), "pub fn own() { /* correction */ }\n").unwrap();
    git(&worker_path, &["add", "own.rs"]);
    git(&worker_path, &["commit", "-qm", "fix(cas-3d57): final correction"]);
    git(&worker_path, &["push", "-q", "origin", &lane]);
    let ignored_tip = git(&worker_path, &["rev-parse", "HEAD"]);
    let mut unrelated = assigned("cas-dec0", TaskType::Bug, "epic/next");
    unrelated.status = TaskStatus::Open;
    f.put(&unrelated);
    let refused = call(&f.worker, close.clone()).await;
    assert_eq!(f.task(id).deliverables.factory_branch_anchor, parked_anchor,
        "a local main ahead of origin is not released, so the anchor must not advance: {refused}");
    assert!(refused.contains("PARK REFUSED"), "{refused}");
    assert!(refused.contains("request changes") && !refused.contains(&unrelated.id),
        "an unrelated same-worker task must not acquire the foreign advance: {refused}");
    for boundary in [parked_anchor.as_ref().unwrap(), &ignored_tip, &offending] {
        assert!(refused.contains(boundary), "missing {boundary}: {refused}");
    }

    let next = assigned("cas-0e2b", TaskType::Bug, "epic/next");
    f.put(&next);
    // A new target conflict must refresh the parked flag even when the
    // rejected branch continuation returns before ordinary re-park work.
    assert!(!f.task(id).deliverables.merge_conflicted);
    git(repo, &["checkout", "-q", "epic/next"]);
    std::fs::write(repo.join("own.rs"), "pub fn target_version() {}\n").unwrap();
    git(repo, &["add", "own.rs"]);
    git(repo, &["commit", "-qm", "fix: conflicting target delivery"]);
    git(repo, &["push", "-q", "origin", "epic/next"]);
    git(&worker_path, &["fetch", "-q", "origin"]);
    let refused = call(&f.worker, close).await;
    let parked = f.task(id);
    let remedy = refused.contains(&next.id) && refused.contains("own branch")
        && !refused.contains("request changes") && !refused.contains("request_changes");
    let first_foreign = refused.contains(&offending) && !refused.contains(&own_correction);
    assert!(remedy && first_foreign && parked.deliverables.merge_conflicted,
        "next-task remedy={remedy}, first foreign commit={first_foreign}, refreshed conflict={}: {refused}",
        parked.deliverables.merge_conflicted);
    assert_eq!(parked.status, TaskStatus::AwaitingMerge);
    assert_eq!(parked.deliverables.factory_branch_anchor, parked_anchor);
}
#[tokio::test]
async fn bug_family_statement_extension_parks_through_close_cas_f63f() {
    let family = |statement: &str| format!(
        "version: 1\nupdated: '2026-11-12'\ninvariants:\n  am-payment-to-qbo-2: {{statement: Arrived, check: Read state}}\nbug_families:\n  paid-invoice-duplicated-by-manual-sync:\n    title: Paid invoice duplicated by manual sync\n    statement: {statement}\n    tasks: [cas-f63f]\n    invariants: [am-payment-to-qbo-2]\n    journeys: [FLOW]\njourneys:\n  - id: FLOW\n    status: live\n    entry: {{url: /flow, trigger: Open}}\n    steps: [Arrive]\n    owners: ['src/flow.ts']\n    invariants: [am-payment-to-qbo-2]\n"
    );
    let base = "Manual sync never duplicates a paid invoice";
    let extended = format!("{base}, including linked off-platform PENDING bitcoin webhook evidence and typed-discount exclusion");
    let registry = family(&extended);
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let checkout = f.cas_dir().join("worktrees").join(WORKER);
    std::fs::create_dir_all(checkout.join("docs/qa")).unwrap();
    std::fs::write(checkout.join("docs/qa/journeys.yaml"), family(base)).unwrap();
    git(&checkout, &["add", "."]);
    git(&checkout, &["commit", "-qm", "docs: base registry"]);
    git(&checkout, &["push", "-q", "origin", LANE]);
    git(f.dir.path(), &["merge", "-q", "--ff-only", LANE]);
    git(f.dir.path(), &["push", "-q", "origin", "main"]);
    let agents = open_agent_store(&f.cas_dir()).unwrap();
    let mut agent = agents.get("nocode-worker-session").unwrap();
    agent.metadata.insert("clone_path".into(), checkout.display().to_string());
    agents.update(&agent).unwrap();
    let mut config = crate::config::Config::load(&f.cas_dir()).unwrap();
    for (key, value) in [("qa.mode", "batch"), ("qa.evidence_gate", "false"),
        ("qa.independent_pass", "false"), ("qa.registry.enforce", "enforce")] {
        config.set(key, value).unwrap();
    }
    config.save(&f.cas_dir()).unwrap();
    env.set("CAS_AGENT_ROLE", "worker");
    let id = "cas-f63f";
    let mut task = assigned(id, TaskType::Bug, "main");
    task.status = TaskStatus::Open;
    f.put(&task);
    call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
    std::fs::create_dir_all(checkout.join("src")).unwrap();
    std::fs::write(checkout.join("src/flow.ts"), "export const flow = true;\n").unwrap();
    std::fs::write(checkout.join("docs/qa/journeys.yaml"), &registry).unwrap();
    git(&checkout, &["add", "."]);
    git(&checkout, &["commit", "-qm", "fix: manual sync never duplicates a paid invoice"]);
    let tip = git(&checkout, &["rev-parse", "HEAD"]);
    git(&checkout, &["push", "-q", "origin", LANE]);
    let reply = call(&f.worker, serde_json::json!({"action":"close", "id":id, "reason":format!("PASS {tip}")})).await;
    let status = f.task(id).status;
    assert_eq!(status, TaskStatus::AwaitingMerge, "{reply}");
}

/// GH #1596/#1597: refusing A must leave B editable and preserve its owner
/// across the supervisor's correction round; a foreign descendant is refused.
#[tokio::test]
async fn refused_close_keeps_correction_tip_and_restart_cas_56f1() {
    for foreign in [false, true] {
        let mut env = TestEnvGuard::temp_home();
        let f = fixture(&mut env, "main");
        let checkout = f.cas_dir().join("worktrees").join(WORKER);
        // Integrate the fixture's previous delivery before this task starts,
        // keeping every subsequent task push a fast-forward.
        git(f.dir.path(), &["merge", "-q", "--ff-only", LANE]);
        git(f.dir.path(), &["push", "-q", "origin", "main"]);
        let agents = open_agent_store(&f.cas_dir()).unwrap();
        let mut agent = agents.get("nocode-worker-session").unwrap();
        agent.metadata.insert("clone_path".into(), checkout.display().to_string());
        agents.update(&agent).unwrap();
        let mut config = crate::config::Config::load(&f.cas_dir()).unwrap();
        for (key, value) in [("qa.mode", "batch"), ("qa.evidence_gate", "false"),
            ("qa.independent_pass", "false")] {
            config.set(key, value).unwrap();
        }
        config.save(&f.cas_dir()).unwrap();
        env.set("CAS_AGENT_ROLE", "worker");
        let id = "cas-56f1";
        let mut task = assigned(id, TaskType::Task, "main");
        task.status = TaskStatus::Open;
        f.put(&task);
        call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
        let start = f.task(id).deliverables.factory_branch_start.unwrap();
        std::fs::create_dir_all(checkout.join("src")).unwrap();
        std::fs::write(checkout.join("src/flow.ts"), "export const flow = true;\n").unwrap();
        // Keep the restart/ownership regression on a live park refusal;
        // registry upkeep now belongs to the lane lint (cas-4b33).
        std::fs::write(checkout.join("HANDOFF.md"), "fixture checkpoint\n").unwrap();
        git(&checkout, &["add", "."]);
        git(&checkout, &["commit", "-qm", "fix: repair flow"]);
        let a = git(&checkout, &["rev-parse", "HEAD"]);
        git(&checkout, &["push", "-q", "origin", LANE]);
        // The commit hook may retain A as work-cycle identity. It is not a park.
        let mut task = f.task(id);
        task.deliverables.factory_branch_anchor = Some(a.clone());
        f.put(&task);
        let close = |tip: &str| serde_json::json!({"action":"close", "id":id, "reason":format!("PASS {tip}")});
        let refused = call(&f.worker, close(&a)).await;
        assert!(refused.contains("ROOT_HANDOFF_COMMITTED"), "{refused}");
        let after = f.task(id);
        assert_eq!(after.status, TaskStatus::InProgress, "{refused}");
        assert!(after.deliverables.parked_branch.is_none(), "{refused}");
        assert!(!after.pending_verification);
        assert!(cas_store::get_latest_worker_delivery(&f.cas_dir(), id).unwrap().is_none());
        git(&checkout, &["rm", "HANDOFF.md"]);
        git(&checkout, &["commit", "-qm", "docs: remove the root checkpoint"]);
        let b = git(&checkout, &["rev-parse", "HEAD"]);
        git(&checkout, &["push", "-q", "origin", LANE]);
        let parked = call(&f.worker, close(&b)).await;
        assert_eq!(f.task(id).status, TaskStatus::AwaitingMerge, "{parked}");
        assert_eq!(f.task(id).deliverables.factory_branch_anchor.as_deref(), Some(b.as_str()), "{parked}");
        // Reproduce recovery of an old runtime's stale A park while B is
        // already pushed; request_changes must retain this task's start graph.
        let mut legacy = f.task(id);
        legacy.deliverables.factory_branch_anchor = Some(a.clone());
        f.put(&legacy);
        env.set("CAS_AGENT_ROLE", "supervisor");
        call(&f.supervisor, serde_json::json!({"action":"request_changes", "id":id, "reason":"Resume the owned corrected delivery"})).await;
        env.set("CAS_AGENT_ROLE", "worker");
        if foreign {
            std::fs::write(checkout.join("foreign.ts"), "foreign work\n").unwrap();
            git(&checkout, &["add", "foreign.ts"]);
            git(&checkout, &["commit", "-qm", "fix(cas-dead): another task's work"]);
            git(&checkout, &["push", "-q", "origin", LANE]);
        }
        let before = git(&checkout, &["rev-parse", "HEAD"]);
        let request = || Parameters(serde_json::from_value(serde_json::json!({"action":"start", "id":id})).unwrap());
        if foreign {
            assert!(f.worker.task(request()).await.is_err());
            assert_eq!(f.task(id).status, TaskStatus::Open);
        } else {
            let resumed = call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
            assert_eq!(f.task(id).status, TaskStatus::InProgress, "{resumed}");
            assert_eq!(f.task(id).deliverables.factory_branch_start.as_ref(), Some(&start));
        }
        assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), before);
    }
}

/// cas-5dc9e: Lane CI SIGABRT'd a close test ("tokio-runtime-worker has
/// overflowed its stack") after cas-289b added ~20 inline lines. Each wrapper
/// on the blocking-pool path had kept its own copy of the close future,
/// using 1.56 MB of the 2 MiB thread before the close body began. This test
/// measures the stack from the blocking thread's start to the close body.
#[tokio::test]
async fn close_future_frame_stays_under_stack_budget_cas_5dc9e() {
    use crate::async_runtime::stack_probe;
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let id = "cas-5dc9e";
    let (checkout, lane) = issue_delivery(&f, id, "main");
    std::fs::write(checkout.join("own.rs"), "pub fn own() {}\n").unwrap();
    git(&checkout, &["add", "own.rs"]);
    git(&checkout, &["commit", "-qm", "fix(cas-5dc9e): own delivery"]);
    git(&checkout, &["push", "-q", "origin", &lane]);
    let response = call(&f.worker, serde_json::json!({
        "action": "close", "id": id, "reason": "Own delivery ready",
    })).await;
    assert_eq!(f.task(id).status, TaskStatus::AwaitingMerge, "{response}");
    let top = stack_probe::take("blocking_pool").expect("blocking pool probe");
    let body = stack_probe::take("close_body").expect("close body probe");
    // Debug builds measured 1,561,648 B before boxing at the pool entry and
    // 386,944 B after it; the rest of 2 MiB is for the body's own calls.
    const BUDGET: usize = 512 * 1024;
    let used = top - body;
    assert!(used <= BUDGET,
        "close stack budget exceeded: {used} B from the blocking-pool thread start to the close body, budget {BUDGET} B. \
         Box the growing future (crate::async_runtime::heap_future) or move new close-body code into a helper fn; \
         do not raise the thread stack size.");
}

/// GH2022: queue admission reads the declared task before judging a shared
/// checkout. A no-code task never acquires or mutates that checkout.
#[test]
fn shared_queue_checkout_rebinds_only_executable_work_gh2022() {
    let mut env = TestEnvGuard::temp_home();
    for (no_code, dirty, foreign_active) in [(false, false, false), (false, true, false), (false, false, true), (true, true, true)] {
        let f = fixture(&mut env, "main");
        let root = f.cas_dir();
        let checkout = root.join("worktrees").join(WORKER);
        git(f.dir.path(), &["worktree", "remove", checkout.to_str().unwrap()]);
        git(f.dir.path(), &["switch", "-c", "factory/retired-worker"]);
        let agents = open_agent_store(&root).unwrap();
        let mut worker = agents.get("nocode-worker-session").unwrap();
        worker.metadata.insert("clone_path".into(), f.dir.path().display().to_string());
        agents.update(&worker).unwrap();
        let mut task = assigned("cas-shared-queue", TaskType::Task, "main");
        task.status = TaskStatus::Open;
        task.execution_note = no_code.then(|| "no-code".into());
        f.put(&task);
        if foreign_active {
            let mut other = assigned("cas-other-active", TaskType::Task, "main");
            other.assignee = Some("another-worker".into());
            f.put(&other);
        }
        if dirty { std::fs::write(f.dir.path().join("operator-draft.txt"), "unsaved\n").unwrap(); }
        let before = git(f.dir.path(), &["status", "--porcelain"]);
        let result = crate::worker_task_queue::ensure_owned_queue_checkout(&root, &worker, Some(&task.id));
        assert_eq!(result.is_ok(), no_code || (!dirty && !foreign_active), "{result:?}");
        assert_eq!(git(f.dir.path(), &["status", "--porcelain"]), before);
        if no_code || dirty || foreign_active {
            assert_eq!(git(f.dir.path(), &["branch", "--show-current"]), "factory/retired-worker");
        } else {
            assert_eq!(crate::factory_isolation::classify_worker_binding(&worker.name,
                crate::factory_isolation::branch_at(f.dir.path()).as_deref()), crate::factory_isolation::WorkerBinding::Own);
        }
    }
}

/// GH #2250/#2274: the worker may already have parked B, or only started
/// B without a commit, when a relayed review correction is pushed on A's
/// recorded source branch. B's lease must not acquire A's identified repair.
#[tokio::test]
async fn parked_review_fix_keeps_recorded_source_task_gh2250_gh2274() {
    let mut env = TestEnvGuard::temp_home();
    for b_parked in [true, false] {
        let f = fixture(&mut env, "epic/next");
        let repo = f.dir.path();
        let checkout = f.cas_dir().join("worktrees").join(WORKER);
        let id = "cas-a0a0";
        let lane = format!("{LANE}-{id}");
        git(&checkout, &["switch", "-c", &lane, "origin/epic/next"]);
        let agents = open_agent_store(&f.cas_dir()).unwrap();
        let mut worker = agents.get("nocode-worker-session").unwrap();
        worker.metadata.insert("clone_path".into(), checkout.display().to_string());
        agents.update(&worker).unwrap();
        let mut a = assigned(id, TaskType::Bug, "epic/next");
        a.status = TaskStatus::Open;
        f.put(&a);
        call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
        std::fs::write(checkout.join("repair.md"), "initial delivery\n").unwrap();
        git(&checkout, &["add", "repair.md"]);
        git(&checkout, &["commit", "-qm", "fix(cas-a0a0): initial delivery"]);
        git(&checkout, &["push", "-q", "origin", &lane]);
        let close = serde_json::json!({"action":"close", "id":id, "reason":"Delivery ready"});
        let first = call(&f.worker, close.clone()).await;
        assert_eq!(f.task(id).status, TaskStatus::AwaitingMerge, "{first}");
        let recorded = f.task(id).deliverables.factory_branch_anchor.unwrap();

        let mut b = assigned("cas-b0b0", TaskType::Bug, "epic/next");
        b.status = TaskStatus::Open;
        f.put(&b);
        call(&f.worker, serde_json::json!({"action":"start", "id":b.id})).await;
        // Separate the lease clocks explicitly; Git timestamps must not depend
        // on whether two starts happen to fall in the same host clock tick.
        let conn = rusqlite::Connection::open(f.cas_dir().join("cas.db")).unwrap();
        for (task, time) in [(id, "2025-01-01T00:00:00Z"), (b.id.as_str(), "2025-01-02T00:00:00Z")] {
            conn.execute("UPDATE task_lease_history SET timestamp=?1 WHERE task_id=?2", rusqlite::params![time, task]).unwrap();
        }
        if b_parked {
            std::fs::write(checkout.join("b.md"), "task B\n").unwrap();
            git(&checkout, &["add", "b.md"]);
            git(&checkout, &["commit", "-qm", "fix(cas-b0b0): delivery B"]);
            let branch = git(&checkout, &["branch", "--show-current"]);
            git(&checkout, &["push", "-q", "origin", &branch]);
            let response = call(&f.worker, serde_json::json!({"action":"close", "id":b.id, "reason":"Delivery B ready"})).await;
            assert_eq!(f.task(&b.id).status, TaskStatus::AwaitingMerge, "{response}");
        }
        let b_before = f.task(&b.id);
        git(&checkout, &["switch", &lane]);
        for subject in ["fix(cas-b0b0): foreign repair\n\nContext: cas-a0a0", "fix: unnamed repair", "fix(cas-a0a0): restore review correction"] {
            git(&checkout, &["reset", "--hard", &recorded]);
            std::fs::write(checkout.join("repair.md"), format!("{subject}\n")).unwrap();
            git(&checkout, &["add", "repair.md"]);
            git(&checkout, &["commit", "-qm", subject]);
            let own = subject.starts_with("fix(cas-a0a0)");
            if own {
                // GH #2274: an epic sync after the repair imports a sibling's
                // delivery without changing which task owns the repair.
                git(repo, &["switch", "epic/next"]);
                std::fs::write(repo.join("target.md"), "integrated sibling\n").unwrap();
                git(repo, &["add", "target.md"]);
                git(repo, &["commit", "-qm", "fix(cas-c0c0): target progress"]);
                git(repo, &["push", "-q", "origin", "epic/next"]);
                git(&checkout, &["merge", "--no-ff", "-qm", "sync declared target", "origin/epic/next"]);
            }
            git(&checkout, &["push", "-q", "--force", "origin", &lane]);
            let tip = git(&checkout, &["rev-parse", "HEAD"]);
            let response = call(&f.worker, close.clone()).await;
            assert_eq!(f.task(id).status, TaskStatus::AwaitingMerge, "{response}");
            assert_eq!(f.task(id).deliverables.factory_branch_anchor.as_deref(),
                Some(if own { tip.as_str() } else { recorded.as_str() }),
                "b_parked={b_parked}, subject={subject}: {response}");
            assert_eq!(crate::gates::codes_in(&response).contains(&"G-60"), !own, "{response}");
            let b_after = f.task(&b.id);
            assert_eq!(b_after.status, b_before.status);
            assert_eq!(b_after.deliverables.factory_branch_anchor, b_before.deliverables.factory_branch_anchor);
        }
        // Attribution does not grant worker review authority. The supervisor
        // returns A through request_changes; hold unfinished B before restart.
        let start = serde_json::json!({"action":"start", "id":id});
        let error = f.worker.task(Parameters(serde_json::from_value(start.clone()).unwrap())).await.unwrap_err();
        assert!(crate::gates::codes_in(&error.message).contains(&"G-05"));
        env.set("CAS_AGENT_ROLE", "supervisor");
        let returned = call(&f.supervisor, serde_json::json!({"action":"request_changes", "id":id, "reason":"Correct the reviewed delivery"})).await;
        assert_eq!(f.task(id).status, TaskStatus::Open, "{returned}");
        env.set("CAS_AGENT_ROLE", "worker");
        if !b_parked {
            call(&f.worker, serde_json::json!({"action":"update", "id":b.id, "status":"blocked"})).await;
        }
        // First start delivers the unread correction decision without mutation.
        let _ = f.worker.task(Parameters(serde_json::from_value(start.clone()).unwrap())).await;
        let resumed = call(&f.worker, start).await;
        assert_eq!(f.task(id).status, TaskStatus::InProgress, "{resumed}");
        assert_eq!(git(&checkout, &["branch", "--show-current"]), lane);
        assert!(agents.list_active_leases().unwrap().iter()
            .filter(|lease| lease.agent_id == "nocode-worker-session")
            .all(|lease| lease.task_id == id));
    }
}

/// GH #1695: a supervisor reopens an already-landed task for revalidation.
/// The new cycle delivers nothing, so no_delivery is the honest close, and the
/// handed-off verification must not hold the worker's next queued start.
#[tokio::test(flavor = "multi_thread")]
async fn revalidated_landed_task_does_not_reuse_its_worker_gh1695() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let repo = f.dir.path();
    let checkout = f.cas_dir().join("worktrees").join(WORKER);
    env.set("CAS_FACTORY_SESSION", "queue-session");
    env.set("CAS_FACTORY_SUPERVISOR_CLI", "codex");
    let agents = open_agent_store(&f.cas_dir()).unwrap();
    for agent_id in ["nocode-worker-session", "nocode-supervisor-session"] {
        let mut agent = agents.get(agent_id).unwrap();
        agent.factory_session = Some("queue-session".into());
        agent.metadata.insert("clone_path".into(), checkout.display().to_string());
        agents.update(&agent).unwrap();
    }
    let mut config = crate::config::Config::load(&f.cas_dir()).unwrap();
    config.set("verification.enabled", "true").unwrap();
    config.save(&f.cas_dir()).unwrap();

    // The first cycle's delivery landed on the target and the task closed.
    let id = "cas-1695a";
    let lane = format!("{LANE}-{id}");
    git(&checkout, &["checkout", "-q", "-b", &lane, "main"]);
    std::fs::write(checkout.join("landed.rs"), "pub fn landed() {}\n").unwrap();
    git(&checkout, &["add", "landed.rs"]);
    git(&checkout, &["commit", "-q", "-m", "fix(cas-1695a): landed fix"]);
    let landed = git(&checkout, &["rev-parse", "HEAD"]);
    git(&checkout, &["push", "-q", "origin", &lane]);
    let mut task = assigned(id, TaskType::Bug, "main");
    task.status = TaskStatus::Closed;
    task.closed_at = Some(chrono::Utc::now());
    task.deliverables.factory_branch_anchor = Some(landed.clone());
    task.deliverables.parked_branch = Some(lane.clone());
    task.deliverables.green_merge_commit = Some(landed.clone());
    f.put(&task);
    let mut next = assigned("cas-1695b", TaskType::Task, "main");
    next.status = TaskStatus::Open;
    next.assignee = None;
    next.execution_note = Some("no-code".into());
    next.external_ref = Some(PROOF.into());
    f.put(&next);

    env.set("CAS_AGENT_ROLE", "supervisor");
    let reopened = call(&f.supervisor, serde_json::json!({
        "action":"reopen", "id":id, "reason":"Revalidate on the current tree",
    })).await;
    assert_eq!(f.task(id).status, TaskStatus::Open, "{reopened}");
    env.set("CAS_AGENT_ROLE", "worker");
    let started = call(&f.worker, serde_json::json!({"action":"start", "id":id})).await;
    assert_eq!(f.task(id).status, TaskStatus::InProgress, "{started}");
    let close = serde_json::json!({
        "action":"close", "id":id, "no_delivery":true,
        "reason":"Revalidated: the landed fix passes on the current tree",
    });

    // Until the retained tip is on the target, it is still a delivery.
    let refused = call(&f.worker, close.clone()).await;
    assert!(refused.contains("G-47"), "{refused}");
    git(repo, &["merge", "-q", "--ff-only", &lane]);
    git(repo, &["push", "-q", "origin", "main"]);
    let closed = call(&f.worker, close).await;
    assert!(cas_store::get_latest_verification_dispatch(&f.cas_dir(), id).unwrap().is_some(), "{closed}");
    let result = f.worker.task(Parameters(serde_json::from_value(serde_json::json!({
        "action":"start", "id":next.id,
    })).unwrap())).await;
    assert!(result.is_err(), "{result:?}");
    assert_eq!(f.task(&next.id).status, TaskStatus::Open);
}

/// GH #2384: an operations-only epic whose branch is the repository's root
/// commit has no structural range to lint. Its approved epic verification and
/// code-free children close it by the ordinary path; a child code delivery
/// keeps the pre-close hook and its refusal.
#[tokio::test]
async fn ops_only_epic_on_root_commit_closes_by_ordinary_path_gh2384() {
    use cas_store::VerificationStore;
    use cas_types::{Verification, VerificationProvenance, VerificationStatus, VerificationType};
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_AGENT_ROLE", "supervisor");
    let epic_branch = "epic/ops-cas-e2384";
    let f = fixture(&mut env, epic_branch);
    let root = git(f.dir.path(), &["rev-parse", epic_branch]);
    assert!(git(f.dir.path(), &["rev-list", "--parents", "-n1", &root]).split_whitespace().count() == 1);
    let mut epic = assigned("cas-e2384", TaskType::Epic, epic_branch);
    epic.assignee = None;
    epic.branch = Some(epic_branch.into());
    epic.epic_verification_owner = Some("nocode-supervisor-session".into());
    f.put(&epic);
    let mut child = assigned("cas-c2384", TaskType::Chore, epic_branch);
    child.status = TaskStatus::Closed;
    child.assignee = None;
    child.execution_note = Some("no-code".into());
    child.external_ref = Some(PROOF.into());
    child.deliverables.commit_hash = Some(root.clone());
    f.put(&child);
    open_task_store(&f.cas_dir()).unwrap().add_dependency(&Dependency::new(
        child.id.clone(), epic.id.clone(), DependencyType::ParentChild,
    )).unwrap();
    let mut verdict = Verification::new("ver-e2384".into(), epic.id.clone());
    verdict.verification_type = VerificationType::Epic;
    verdict.status = VerificationStatus::Approved;
    verdict.summary = "Operations epic reviewed".into();
    verdict.provenance = VerificationProvenance::SupervisorDirect;
    verdict.agent_id = Some("nocode-supervisor-session".into());
    verdict.issuer_agent_id = verdict.agent_id.clone();
    let dispatch = cas_store::create_verification_dispatch_bound(
        &f.cas_dir(), &epic.id, "nocode-supervisor-session", "nocode-supervisor-session",
        &cas_types::VerificationProofBoundary::task(),
        chrono::Utc::now() + chrono::Duration::minutes(10), false,
    ).unwrap();
    verdict.dispatch_id = Some(dispatch.id.clone());
    cas_store::SqliteVerificationStore::open(&f.cas_dir()).unwrap().add(&verdict).unwrap();
    let conn = rusqlite::Connection::open(f.cas_dir().join("cas.db")).unwrap();
    cas_store::resolve_verification_dispatch_with_conn(
        &conn, &dispatch.id, "nocode-supervisor-session", None, true,
    ).unwrap();
    let close = serde_json::json!({"action":"close", "id":epic.id, "reason":"All operations children verified"});

    let refused = call(&f.supervisor, close.clone()).await;
    assert!(refused.contains("G-41"), "a child code delivery keeps the hook: {refused}");
    assert_ne!(f.task(&epic.id).status, TaskStatus::Closed);

    child.deliverables.commit_hash = None;
    f.put(&child);
    let response = call(&f.supervisor, close).await;
    let closed = f.task(&epic.id);
    assert_eq!(closed.status, TaskStatus::Closed, "{response}");
    assert!(closed.notes.contains("ver-e2384"), "audit names the verdict: {}", closed.notes);
}
