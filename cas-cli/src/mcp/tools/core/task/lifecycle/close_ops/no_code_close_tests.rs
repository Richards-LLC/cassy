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
        assert!(missing_reason.contains("NO DELIVERY"), "{missing_reason}");
        assert_ne!(f.task(id).status, TaskStatus::Closed, "{missing_reason}");
        let conflict = call(&f.worker, serde_json::json!({
            "action": "close", "id": id, "reason": "Simulator released",
            "no_delivery": true, "execution_note": "no-code", "external_ref": PROOF,
        })).await;
        assert!(conflict.contains("NO DELIVERY"), "{conflict}");
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

/// cas-c202: a supervisor proof_scope_fix that lands after this close loaded
/// the task, but before its terminal write, must survive: the close refuses
/// rather than writing its stale copy (status, proof_targets, notes) back.
#[tokio::test]
async fn close_refuses_terminal_write_over_a_concurrent_proof_scope_fix_cas_c202() {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env, "main");
    let id = "cas-c202-race";
    f.put(&assigned(id, TaskType::Task, "main"));
    claim(&f, id);
    let cas_dir = f.cas_dir();
    close_race_hook::before_terminal_write(id, move || {
        let store = open_task_store(&cas_dir).unwrap();
        let loaded = store.get("cas-c202-race").unwrap();
        let mut corrected = loaded.clone();
        corrected.status = TaskStatus::Open;
        corrected.proof_targets = vec!["pre_tool".into(), "pre_tool::shell_effects".into()];
        corrected.notes.push_str("\n\n[2026-10-21 14:18] PROOF-SCOPE FIX widened");
        store.update_if_current(&corrected, &loaded).unwrap();
    });
    let response = call(
        &f.worker,
        serde_json::json!({
            "action": "close", "id": id, "reason": "Deployed the monthly release",
            "execution_note": "no-code", "external_ref": PROOF,
        }),
    )
    .await;
    let task = f.task(id);
    assert_eq!(task.status, TaskStatus::Open, "the correction's status survives: {response}");
    assert_eq!(task.proof_targets, ["pre_tool", "pre_tool::shell_effects"], "{response}");
    assert!(task.notes.contains("PROOF-SCOPE FIX"), "{}", task.notes);
    assert!(
        response.contains("CLOSE REFUSED") && response.contains("Reload"),
        "the refusal tells the worker to reload and retry: {response}"
    );

    // cas-3f81: a note-only append does not invalidate the close's proof.
    // Keep the stored timeline intact, including repeated notes and blank
    // paragraphs, then append close's own audit notes without duplicating it.
    let id = "cas-3f81-race";
    let mut original = assigned(id, TaskType::Task, "main");
    original.notes = "[2026-10-21 14:17] Initial progress".into();
    f.put(&original);
    claim(&f, id);
    let cas_dir = f.cas_dir();
    let (notes_tx, notes_rx) = std::sync::mpsc::channel();
    close_race_hook::before_terminal_write(id, move || {
        let store = open_task_store(&cas_dir).unwrap();
        store.append_note(id, "[2026-10-21 14:18] Supervisor update\n\nSecond paragraph").unwrap();
        for _ in 0..2 {
            store.append_note(id, "[2026-10-21 14:19] Repeated observation").unwrap();
        }
        notes_tx.send(store.get(id).unwrap().notes).unwrap();
    });
    let response = call(
        &f.worker,
        serde_json::json!({
            "action": "close", "id": id, "reason": "Deployment independently verified",
            "execution_note": "no-code", "external_ref": PROOF,
        }),
    ).await;
    let closed = f.task(id);
    let persisted_notes = notes_rx.try_recv().expect("terminal race hook ran");
    assert_eq!(closed.status, TaskStatus::Closed, "note-only changes allow close: {response}");
    assert!(closed.notes.starts_with(&persisted_notes), "concurrent timeline lost or reordered: {}", closed.notes);
    assert_eq!(closed.notes.matches("Initial progress").count(), 1);
    assert_eq!(closed.notes.matches("Supervisor update").count(), 1);
    assert_eq!(closed.notes.matches("Repeated observation").count(), 2);
    assert!(closed.notes[persisted_notes.len()..].contains("Deployment independently verified"), "close audit follows concurrent notes: {}", closed.notes);
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
            response.contains("MERGE REQUIRED") || response.contains("DELIVERY BRANCH UNRESOLVED"),
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
        !response.contains("MERGE REQUIRED") && !response.contains("DELIVERY BRANCH UNRESOLVED"),
        "{response}"
    );
}

/// GH #1167 (4): the same lane still gates a code task and a worker's own
/// no-code close: an unnamed lane commit may be this task's work (cas-2387).
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
            response.contains("MERGE REQUIRED") || response.contains("DELIVERY BRANCH UNRESOLVED"),
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

/// cas-eac2 (D2): the fast rows run once, in the worker's pre-push hook, and
/// lane CI no longer repeats them for factory pushes. A tip that ships the
/// hook but has no PASS receipt (an override or `--no-verify` push) is refused
/// at park, which only reads the receipt; with it, the delivery parks and the
/// core merge gate still refuses to close it before the merge.
#[tokio::test]
async fn park_reads_the_pre_push_fast_rows_receipt_cas_eac2() {
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
    f.put(&assigned(id, TaskType::Task, "main"));
    claim(&f, id);
    let close = serde_json::json!({"action": "close", "id": id, "reason": "Delivered"});

    let refused = call(&f.worker, close.clone()).await;
    assert!(refused.contains("FAST ROWS REQUIRED"), "{refused}");
    assert!(refused.contains("--fast-rows --base origin/main"), "{refused}");
    assert_ne!(f.task(id).status, TaskStatus::AwaitingMerge, "{refused}");

    let tip = git(&worker_path, &["rev-parse", "HEAD"]);
    let receipts = std::path::PathBuf::from(git(&worker_path, &["rev-parse", "--path-format=absolute", "--git-common-dir"]))
        .join("cas/fast-rows");
    std::fs::create_dir_all(&receipts).unwrap();
    std::fs::write(receipts.join(format!("{tip}.pass")), format!("fast-rows: PASS {tip} base=main\n")).unwrap();
    let parked = call(&f.worker, close.clone()).await;
    assert!(parked.contains("MERGE REQUIRED"), "{parked}");
    assert_eq!(f.task(id).status, TaskStatus::AwaitingMerge, "{parked}");

    let again = call(&f.worker, close).await;
    assert!(again.contains("MERGE REQUIRED"), "unmerged work must not close: {again}");
    assert_ne!(f.task(id).status, TaskStatus::Closed, "{again}");
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
        assert!(response.contains("MERGE REQUIRED"), "{subject}: {response}");
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
        ("factory/nocode-worker-cas-a0b1", false, false),
        ("factory/nocode-worker-cas-a0b1", true, false),
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
            assert!(response.contains("MERGE REQUIRED"), "{response}");
            assert_eq!(
                parked.deliverables.factory_branch_anchor.as_deref(),
                Some(tip.as_str())
            );
            assert_eq!(parked.deliverables.parked_branch.as_deref(), Some(lane));
            assert_eq!(parked.status, TaskStatus::AwaitingMerge, "{response}");
        }
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
