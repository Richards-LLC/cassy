//! cas-0a21: compare-and-swap the target ref for transactional delivery merge.
//!
//! `worktree_merge` checked `receipt.target_sha` against the live target tip in
//! preflight, then authorized and ran the Git merge with nothing holding the
//! target ref in between. A concurrent commit landing on the target in that
//! window was silently accepted: the post-merge gate only asserted that the
//! receipt commit was an *ancestor* of the new tip, which stays true precisely
//! because the merge swept the drifted commits in. CAS then projected
//! Merged -> CloseReady -> Delivered even though the merge's first parent was
//! not the reviewed `target_sha`.
//!
//! These regressions drive the real production path (`factory
//! action=worktree_merge`) and inject drift deterministically:
//!
//!  * before merge  — a plain commit on the target after receipt approval;
//!  * during merge  — a real `post-checkout` git hook, which fires inside
//!    `merge_preserving_worktree` between `checkout(parent)` and
//!    `merge_branch(..)`, i.e. exactly the preflight->merge window. Using a
//!    genuine git hook keeps the race deterministic without compiling any
//!    test-only injection seam into production code.

use std::path::{Path, PathBuf};
use std::process::Command;

use cas::mcp::{CasCore, CasService};
use cas::store::{init_cas_dir, open_agent_store, open_task_store};
use cas::types::{
    Agent, AgentRole, AgentType, Task, TaskDepth, TaskStatus, TaskType, WorkTarget,
    WorkerCompletionReceiptInput, WorkerDeliveryState,
};
use cas_mcp::types::{CoordinationRequest, TaskRequest, VerificationRequest};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::RawContent;
use tempfile::TempDir;

use crate::test_env_guard::TestEnvGuard;

#[path = "fixtures/ci_check_run.rs"]
mod ci_check_run;

// =============================================================================
// Fixtures (deliberately self-contained: cas-0a21 must not couple to the
// shared worktree_surface_test helpers while cas-59c0 is editing that file)
// =============================================================================

fn run_git(args: &[&str], dir: &Path) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_stdout(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct GitRepo {
    _temp: TempDir,
    root: PathBuf,
}

impl GitRepo {
    fn new() -> Self {
        let temp = TempDir::new().expect("TempDir");
        let root = temp.path().to_path_buf();
        run_git(&["init", "-b", "main"], &root);
        run_git(&["config", "user.email", "test@test.com"], &root);
        run_git(&["config", "user.name", "Test"], &root);
        std::fs::write(root.join("README.md"), "test").unwrap();
        run_git(&["add", "."], &root);
        run_git(&["commit", "-m", "init"], &root);
        Self { _temp: temp, root }
    }

    fn add_worktree(&self, path: &Path, branch: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        run_git(
            &["worktree", "add", "-b", branch, path.to_str().unwrap()],
            &self.root,
        );
    }

    /// Install a `post-checkout` hook that lands exactly one commit on
    /// `branch`, simulating a concurrent actor.
    ///
    /// cas-4702: `merge_preserving_worktree` no longer checks the target out
    /// in the shared checkout (that flipped the operator's HEAD — GH #68); it
    /// creates an ephemeral detached worktree instead. `git worktree add`
    /// still fires `post-checkout`, so this hook still lands inside the
    /// preflight -> merge window on the real production path. It advances the
    /// branch ref with plumbing rather than committing on HEAD, so it drifts
    /// the target from wherever it happens to run.
    fn arm_post_checkout_drift(&self, marker: &str, branch: &str) {
        let hooks = self.root.join(".git").join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("post-checkout");
        // The marker lives under .git/ so firing the hook never dirties any
        // work tree the merge is about to operate on.
        cas::test_paths::warm_stub(
            &hook,
            &format!(
                "#!/bin/sh\n\
                 set -e\n\
                 marker=\"$(git rev-parse --git-common-dir)/{marker}\"\n\
                 [ -e \"$marker\" ] && exit 0\n\
                 : > \"$marker\"\n\
                 tip=\"$(git rev-parse refs/heads/{branch})\"\n\
                 tree=\"$(git rev-parse refs/heads/{branch}^{{tree}})\"\n\
                 new=\"$(git -c user.email=drift@test.com -c user.name=Drift \\\n\
                     commit-tree \"$tree\" -p \"$tip\" -m 'concurrent target commit')\"\n\
                 git update-ref refs/heads/{branch} \"$new\" \"$tip\"\n\
                 exit 0\n"
            ),
        );
    }
}

fn coord_req(action: &str) -> CoordinationRequest {
    serde_json::from_value(serde_json::json!({ "action": action })).expect("CoordinationRequest")
}

fn task_req(value: serde_json::Value) -> TaskRequest {
    serde_json::from_value(value).expect("TaskRequest")
}

fn verification_req(value: serde_json::Value) -> VerificationRequest {
    serde_json::from_value(value).expect("VerificationRequest")
}

fn get_text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match &c.raw {
            RawContent::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn register_delivery_agent(
    cas_root: &Path,
    id: &str,
    name: &str,
    role: AgentRole,
    factory_session: &str,
) {
    let agent_store = open_agent_store(cas_root).expect("agent store");
    let mut agent = Agent::new(id.to_string(), name.to_string());
    agent.agent_type = if role == AgentRole::Worker {
        AgentType::Worker
    } else {
        AgentType::Primary
    };
    agent.role = role;
    agent.factory_session = Some(factory_session.to_string());
    agent.heartbeat();
    agent_store.register(&agent).expect("register agent");
}

fn delivery_service(cas_root: &Path, agent_id: &str) -> CasService {
    let core = CasCore::with_daemon(cas_root.to_path_buf(), None, None);
    core.set_agent_id_for_testing(agent_id.to_string());
    CasService::new(core, None)
}

/// One fully-armed transactional delivery: repo + worktree + task + approved
/// receipt, parked at AwaitingMerge and ready for `worktree_merge`.
struct DeliveryFixture {
    repo: GitRepo,
    cas_root: PathBuf,
    task_id: String,
    supervisor_id: String,
    receipt: WorkerCompletionReceiptInput,
}

async fn arm_delivery(slug: &str, repo_host: &str, env: &mut TestEnvGuard) -> DeliveryFixture {
    arm_delivery_with_selector(
        slug,
        repo_host,
        env,
        &format!("project:github.com/org/{repo_host}"),
    )
    .await
}

async fn arm_delivery_with_selector(
    slug: &str,
    repo_host: &str,
    env: &mut TestEnvGuard,
    repo_selector: &str,
) -> DeliveryFixture {
    let repo = GitRepo::new();
    run_git(
        &[
            "remote",
            "add",
            "origin",
            &format!("git@github.com:org/{repo_host}.git"),
        ],
        &repo.root,
    );
    // The candidate gate must confirm publication. Keep Git hermetic while
    // the pinned project identity and GH double still address the intended repo.
    let origin = repo.root.join(".git/test-origin.git");
    run_git(&["init", "-q", "--bare", origin.to_str().unwrap()], &repo.root);
    run_git(&["config", &format!("url.{}.insteadOf", origin.display()),
        &format!("git@github.com:org/{repo_host}.git")], &repo.root);
    let cas_root = init_cas_dir(&repo.root).expect("init CAS");
    // Keep deliberate CI doubles (notably the red-override regression).
    // Other target-CAS fixtures need explicit successful code validation.
    if std::env::var_os(cas::github_issue_attach::GH_BIN_ENV)
        .is_none_or(|binary| !Path::new(&binary).is_file())
    {
        env.set(cas::github_issue_attach::GH_BIN_ENV, ci_check_run::green_ci(&cas_root));
    }
    let artifact_root = cas_root.join("durable-artifacts");
    std::fs::write(
        cas_root.join("config.toml"),
        format!(
            "{}[worktrees]\nenabled = false\n\n[factory]\nartifacts_root = {:?}\n",
            if repo_selector.starts_with("project:") {
                format!("[project]\ncanonical_id = \"github.com/org/{repo_host}\"\n")
            } else {
                String::new()
            },
            artifact_root.display().to_string()
        ),
    )
    .expect("write config");

    let factory_session = format!("{slug}-factory");
    let worker_id = format!("{slug}-worker-session");
    let supervisor_id = format!("{slug}-supervisor-session");
    register_delivery_agent(
        &cas_root,
        &worker_id,
        slug,
        AgentRole::Worker,
        &factory_session,
    );
    register_delivery_agent(
        &cas_root,
        &supervisor_id,
        &format!("{slug}-supervisor"),
        AgentRole::Supervisor,
        &factory_session,
    );

    let worker_path = cas_root.join("worktrees").join(slug);
    repo.add_worktree(&worker_path, &format!("factory/{slug}"));
    std::fs::write(worker_path.join("delivered.txt"), "worker change\n").unwrap();
    run_git(&["add", "delivered.txt"], &worker_path);
    run_git(&["commit", "-m", "worker delivery commit"], &worker_path);

    let task_store = open_task_store(&cas_root).expect("task store");
    let mut task = Task::new(
        format!("cas-{slug}-target-cas"),
        "Transactional delivery target CAS".to_string(),
    );
    task.task_type = TaskType::Task;
    task.status = TaskStatus::InProgress;
    task.depth = TaskDepth::Deep;
    task.assignee = Some(slug.to_string());
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: repo_selector.to_string(),
        target_branch: "main".to_string(),
    });
    task_store.add(&task).expect("add task");
    let artifact = artifact_root.join(&task.id).join("proof.json");
    std::fs::create_dir_all(artifact.parent().unwrap()).expect("create durable artifact directory");
    std::fs::write(&artifact, "transactional delivery proof\n").expect("write durable artifact");
    assert!(matches!(
        open_agent_store(&cas_root)
            .expect("agent store")
            .try_claim(
                &task.id,
                &worker_id,
                600,
                Some("transactional delivery target CAS"),
            )
            .expect("claim delivery task"),
        cas::types::ClaimResult::Success(_)
    ));

    let receipt = WorkerCompletionReceiptInput {
        task_id: task.id.clone(),
        worker_agent_id: worker_id.clone(),
        repo_selector: repo_selector.to_string(),
        source_branch: format!("factory/{slug}"),
        commit_sha: git_stdout(&repo.root, &["rev-parse", &format!("factory/{slug}")]),
        merge_base_sha: git_stdout(
            &repo.root,
            &["merge-base", &format!("factory/{slug}"), "main"],
        ),
        target_branch: "main".to_string(),
        target_sha: git_stdout(&repo.root, &["rev-parse", "main"]),
        proof_reference: format!("proof:{slug}"),
        scope_summary: "transactional delivery target CAS".to_string(),
        artifact_path: Some(artifact.display().to_string()),
    };

    // cas-c744 / cas-e051: review the unpublished candidate through the real
    // receipt path. The retired #588 gate required publication first; seeding
    // its delivery/dispatch rows bypassed the pre-merge proof boundary.
    let worker_service = delivery_service(&cas_root, &worker_id);
    let close = worker_service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": task.id,
            "reason": "worker handoff",
            "completion_receipt": serde_json::to_string(&receipt).unwrap(),
        }))))
        .await
        .expect("receipt submission");
    assert!(get_text(&close).contains("State: awaiting_verification"), "{}", get_text(&close));
    assert!(!cas_git_is_ancestor(&repo.root, &receipt.source_branch, "main"));
    assert_eq!(
        cas_store::get_latest_worker_delivery(&cas_root, &task.id).unwrap().unwrap().1.state,
        WorkerDeliveryState::AwaitingVerification,
    );
    assert!(open_agent_store(&cas_root).unwrap().get_lease(&task.id).unwrap().is_none(),
        "the accepted receipt releases its exact worker lease");
    let pending = task_store.get(&task.id).unwrap();
    assert!(pending.pending_verification);
    assert_ne!(pending.status, TaskStatus::Closed);

    let dispatch = cas_store::get_latest_verification_dispatch(&cas_root, &task.id)
        .expect("dispatch lookup")
        .expect("receipt-bound dispatch");
    let supervisor_service = delivery_service(&cas_root, &supervisor_id);
    let verification = supervisor_service
        .verification(Parameters(verification_req(serde_json::json!({
            "action": "add",
            "task_id": task.id,
            "status": "approved",
            "summary": "delivery proof approved",
            "confidence": 1.0,
            "dispatch_id": dispatch.id,
        }))))
        .await
        .expect("verification add");
    assert!(get_text(&verification).contains("approved"));

    DeliveryFixture {
        repo,
        cas_root,
        task_id: task.id,
        supervisor_id,
        receipt,
    }
}

async fn run_merge(fixture: &DeliveryFixture) -> String {
    let supervisor_service = delivery_service(&fixture.cas_root, &fixture.supervisor_id);
    let mut merge = coord_req("worktree_merge");
    merge.id = Some(fixture.receipt.source_branch.clone());
    merge.task_id = Some(fixture.task_id.clone());
    merge.allow_trunk = Some(true);
    merge.cleanup = Some(false);
    match supervisor_service.factory(Parameters(merge)).await {
        Ok(result) => get_text(&result),
        Err(error) => format!("MCP_ERROR: {error}"),
    }
}

/// A release train owns trunk even when a supervisor explicitly allows it.
/// Drive both stored (System A) and inferred (System B) merge targets.
#[tokio::test]
async fn open_release_train_refuses_trunk_even_with_allow_trunk() {
    let mut env = TestEnvGuard::temp_home();
    let fixture = arm_delivery("train-guard", "train-guard", &mut env).await;
    env.set_current_dir(&fixture.repo.root);
    let tasks = open_task_store(&fixture.cas_root).unwrap();
    let mut train = Task::new("cas-train".into(), "Release train vNEXT".into());
    train.task_type = TaskType::Epic;
    train.labels = vec!["release-train".into()];
    train.branch = Some("epic/train".into());
    train.deliverables.work_target = Some(WorkTarget {
        repo_selector: fixture.receipt.repo_selector.clone(),
        target_branch: "main".into(),
    });
    // cas-ed1d: the prior train stays open while its cut waits to land, but
    // accepts no more merges. Guidance must name the fresh accepting train.
    let mut cut = train.clone();
    cut.id = "cas-cut".into();
    cut.branch = Some("epic/cut-train".into());
    cut.created_at -= chrono::Duration::minutes(1);
    cut.labels.push("release-train-cut".into());
    tasks.add(&cut).unwrap();
    tasks.add(&train).unwrap();
    let service = delivery_service(&fixture.cas_root, &fixture.supervisor_id);
    for system_a in [false, true] {
        if system_a {
            cas::store::open_worktree_store(&fixture.cas_root).unwrap().add(
                &cas::types::Worktree::new(
                    "stored-train-worker".into(), fixture.receipt.source_branch.clone(),
                    "main".into(), fixture.cas_root.join("worktrees/train-guard"),
                ),
            ).unwrap();
        }
        for allow_trunk in [false, true] {
            let mut merge = coord_req("worktree_merge");
            merge.id = Some(fixture.receipt.source_branch.clone());
            merge.task_id = Some(fixture.task_id.clone());
            merge.allow_trunk = Some(allow_trunk);
            merge.cleanup = Some(false);
            let error = service.factory(Parameters(merge)).await
                .expect_err("an open train must refuse direct trunk merges");
            let message = error.to_string();
            assert!(message.contains("RELEASE TRAIN OPEN") && message.contains(&train.id)
                && message.contains("epic/train") && message.contains("scripts/release.sh --hotfix"),
                "System A={system_a}, allow_trunk={allow_trunk}: {message}");
            assert_eq!(git_stdout(&fixture.repo.root, &["rev-parse", "main"]), fixture.receipt.target_sha);
            assert_eq!(delivery_state(&fixture), WorkerDeliveryState::AwaitingMerge);
        }
    }
    train.status = TaskStatus::Closed;
    tasks.update(&train).unwrap();
    let cut_only = run_merge(&fixture).await;
    assert!(cut_only.contains("RELEASE TRAIN OPEN") && cut_only.contains(&cut.id)
        && cut_only.contains("cut and accepts no further merges")
        && cut_only.contains("Wait for it to land or open the next train")
        && !cut_only.contains("scripts/release.sh --hotfix"), "{cut_only}");
    assert_eq!(git_stdout(&fixture.repo.root, &["rev-parse", "main"]), fixture.receipt.target_sha);
    assert_eq!(delivery_state(&fixture), WorkerDeliveryState::AwaitingMerge);
    cut.status = TaskStatus::Closed;
    tasks.update(&cut).unwrap();
    let merged = run_merge(&fixture).await;
    assert!(cas_git_is_ancestor(&fixture.repo.root, &fixture.receipt.commit_sha, "main"),
        "with no train open the existing merge must work: {merged}");
}

fn delivery_state(fixture: &DeliveryFixture) -> WorkerDeliveryState {
    cas_store::get_latest_worker_delivery(&fixture.cas_root, &fixture.task_id)
        .expect("delivery lookup")
        .expect("delivery transaction")
        .1
        .state
}

/// Every state a delivery must NOT reach when the reviewed target drifted.
fn assert_no_delivery_projection(fixture: &DeliveryFixture, state: WorkerDeliveryState) {
    assert!(
        !matches!(
            state,
            WorkerDeliveryState::Merged
                | WorkerDeliveryState::CloseReady
                | WorkerDeliveryState::Delivered
        ),
        "target drift must never project a merged/delivered transaction, got {state}"
    );
    let task = open_task_store(&fixture.cas_root)
        .expect("task store")
        .get(&fixture.task_id)
        .expect("task lookup");
    assert_ne!(
        task.status,
        TaskStatus::Closed,
        "target drift must not close the task"
    );
}

#[tokio::test]
async fn cas8d38_worktree_merge_records_observed_work_target_delivery() {
    // cas-bebc scopes worktree_merge to the caller's CAS_FACTORY_SESSION; the
    // test must not inherit the session of the shell that runs it (cas-31a3).
    let mut env = TestEnvGuard::temp_home();
    let repo = GitRepo::new();
    run_git(&["branch", "integration"], &repo.root);
    run_git(
        &["remote", "add", "origin", "git@github.com:org/cas8d38-observed.git"],
        &repo.root,
    );
    let remote = TempDir::new().unwrap();
    run_git(&["init", "--bare", remote.path().to_str().unwrap()], &repo.root);
    run_git(&["config", &format!("url.{}.insteadOf", remote.path().display()),
        "git@github.com:org/cas8d38-observed.git"], &repo.root);
    run_git(&["push", "origin", "main", "integration"], &repo.root);
    let cas_root = init_cas_dir(&repo.root).expect("init CAS");
    env.set(cas::github_issue_attach::GH_BIN_ENV, ci_check_run::green_ci(&cas_root));
    std::fs::write(cas_root.join("config.toml"), "[worktrees]\nenabled = false\n")
        .expect("write config");
    let supervisor_id = "cas8d38-supervisor-session";
    register_delivery_agent(
        &cas_root,
        supervisor_id,
        "cas8d38-supervisor",
        AgentRole::Supervisor,
        "cas8d38-factory",
    );
    register_delivery_agent(
        &cas_root,
        "cas8d38-worker-session",
        "cas8d38-worker",
        AgentRole::Worker,
        "cas8d38-factory",
    );
    let worker_path = cas_root.join("worktrees").join("cas8d38-worker");
    repo.add_worktree(&worker_path, "factory/cas8d38-worker");
    std::fs::write(worker_path.join("delivered.txt"), "observed merge\n").unwrap();
    run_git(&["add", "delivered.txt"], &worker_path);
    run_git(&["commit", "-m", "observed delivery"], &worker_path);

    let task_store = open_task_store(&cas_root).unwrap();
    let mut task = Task::new("cas-cas8d38-observed".to_string(), "Observed merge correction".to_string());
    task.task_type = TaskType::Task;
    task.status = TaskStatus::InProgress;
    task.depth = TaskDepth::Light;
    task.assignee = Some("cas8d38-worker".to_string());
    task.risk = vec![cas::types::TaskRisk::Platform];
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/org/cas8d38-observed".to_string(),
        target_branch: "integration".to_string(),
    });
    task_store.add(&task).unwrap();

    let service = delivery_service(&cas_root, supervisor_id);
    let mut merge = coord_req("worktree_merge");
    merge.id = Some("factory/cas8d38-worker".to_string());
    merge.task_id = Some(task.id.clone());
    merge.cleanup = Some(false);
    let merged = service.factory(Parameters(merge)).await.expect("merge call");
    assert!(get_text(&merged).contains("Merged"), "{}", get_text(&merged));
    assert!(cas_store::get_latest_worker_delivery(&cas_root, &task.id)
        .unwrap()
        .is_none(), "observation must not fabricate a worker receipt");

    let corrected = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": task.id,
            "risk": "none",
            "proof_scope_fix": true,
            "reason": "Platform risk was declared in error after the observed integration merge."
        }))))
        .await
        .expect("supervisor correction");
    assert!(get_text(&corrected).contains("Corrected proof scope"));
    assert_eq!(task_store.get(&task.id).unwrap().risk, [cas::types::TaskRisk::None]);
}

/// cas-eda0: a parked delivery that `worktree_merge` lands with green CI is
/// closed by Cassy using its pre-merge approval: no worker re-close, the parked
/// close reason and the merge SHA are recorded, and the worker hears once.
/// A rejected verdict prevents publication; a red-CI override never auto-closes. A merge
/// without check runs still starts the close and records its unconfirmed CI.
#[tokio::test]
async fn cassy_closes_green_merged_delivery_on_approval_cas_eda0() {
    premerge_record_then_merge(&[
        // The worker cannot close while its candidate is still under review.
        ("worker-blocked", "success", Some("approved"), true),
        // cas-46ea: merged from a pushed lane tip without a park.
        ("unparked", "success", Some("approved"), true),
        ("green-approved", "success", Some("approved"), true),
        ("green-rejected", "success", Some("rejected"), false),
        ("advisory-no-checks", "no-checks", Some("approved"), true),
        ("advisory-pending-docs", "pending", Some("approved"), true),
        ("red-override", "failure", Some("approved"), false),
        ("fresh-branch", "success", Some("approved"), true),
    ])
    .await;
}

/// cas-62c3: accepting an exact late verdict must never silently discard it
/// on the next real worktree_merge, including explicit timeout recovery.
#[tokio::test]
async fn late_accepted_premerge_verdict_is_reused_cas_62c3() {
    premerge_record_then_merge(&[
        ("late-approved", "success", Some("approved"), true),
        ("late-timedout-approved", "success", Some("approved"), true),
        ("late-moved-target", "success", Some("approved"), true),
        ("late-child", "success", Some("approved"), true),
        ("late-child-moved-target", "success", Some("approved"), true),
        (
            "late-child-target-changed",
            "success",
            Some("approved"),
            false,
        ),
        ("late-child-superseded", "success", Some("approved"), false),
        ("late-child-changed", "success", Some("approved"), false),
        ("late-child-capability-expired", "success", Some("approved"), false),
    ])
    .await;
}

async fn premerge_record_then_merge(rows: &[(&str, &str, Option<&str>, bool)]) {
    for &(row, conclusion, verdict, closes) in rows {
        let mut env = TestEnvGuard::temp_home();
        let repo = GitRepo::new();
        let slug = format!("eda0-{row}");
        run_git(
            &["remote", "add", "origin", &format!("git@github.com:org/{slug}.git")],
            &repo.root,
        );
        // cas-3937: a local-only target must be confirmed by an origin that
        // answers and lacks the branch. The GitHub URL stays for CI; Git
        // reaches it as an empty bare repository.
        let origin_dir = TempDir::new_in(cas::test_paths::runtime_fixture_parent()).unwrap();
        run_git(&["init", "-q", "--bare", origin_dir.path().to_str().unwrap()], &repo.root);
        run_git(
            &[
                "config",
                &format!("url.{}.insteadOf", origin_dir.path().display()),
                &format!("git@github.com:org/{slug}.git"),
            ],
            &repo.root,
        );
        let cas_root = init_cas_dir(&repo.root).expect("init CAS");
        let gh = if conclusion == "no-checks" {
            let path = cas_root.join("no-checks-ci-gh");
            cas::test_paths::warm_stub(
                &path,
                "#!/bin/sh\nif [ \"$1 $2\" = 'repo view' ]; then printf '{\"nameWithOwner\":\"org/cas\"}\\n'; exit 0; fi\nif [ \"$1 $2\" = 'api graphql' ]; then printf '%s\\n' '{\"data\":{\"repository\":{\"mergeQueue\":null}}}'; exit 0; fi\nprintf '{\"check_runs\":[]}\\n'\n",
            );
            path
        } else if conclusion == "pending" {
            let path = cas_root.join("pending-ci-gh");
            cas::test_paths::warm_stub(
                &path,
                "#!/bin/sh\nif [ \"$1 $2\" = 'repo view' ]; then printf '{\"nameWithOwner\":\"org/cas\"}\\n'; exit 0; fi\nif [ \"$1 $2\" = 'api graphql' ]; then printf '%s\\n' '{\"data\":{\"repository\":{\"mergeQueue\":null}}}'; exit 0; fi\nprintf '{\"check_runs\":[{\"name\":\"Scoped Validation (factory/PR)\",\"status\":\"in_progress\",\"conclusion\":null}]}\\n'\n",
            );
            path
        } else {
            ci_check_run::ci_concluded(&cas_root, conclusion)
        };
        env.set(cas::github_issue_attach::GH_BIN_ENV, &gh);
        // cas-3fc8: the insteadOf rewrite also changes `git remote get-url`,
        // so pin the project identity the fixture means.
        std::fs::write(
            cas_root.join("config.toml"),
            // Per-task QA: these rows follow one delivery's own dispatch and verdict.
            format!("[project]\ncanonical_id = \"github.com/org/{slug}\"\n[worktrees]\nenabled = false\n[qa]\nmode = \"per_task\"\n"),
        )
        .expect("write config");
        let worker = slug.clone();
        let worker_id = format!("{slug}-worker-session");
        let supervisor_id = format!("{slug}-supervisor-session");
        let session = format!("{slug}-factory");
        register_delivery_agent(&cas_root, &worker_id, &worker, AgentRole::Worker, &session);
        register_delivery_agent(
            &cas_root,
            &supervisor_id,
            &format!("{slug}-supervisor"),
            AgentRole::Supervisor,
            &session,
        );
        let worker_path = cas_root.join("worktrees").join(&worker);
        repo.add_worktree(&worker_path, &format!("factory/{worker}"));
        let delivered_path = if conclusion == "pending" { "docs/delivery.md" } else { "delivered.txt" };
        let task_store = open_task_store(&cas_root).unwrap();
        let mut task = Task::new(format!("cas-eda0-{row}"), "Parked delivery".to_string());
        task.task_type = TaskType::Task;
        task.status = TaskStatus::InProgress;
        task.depth = TaskDepth::Deep;
        task.assignee = Some(worker.clone());
        task.risk = vec![cas::types::TaskRisk::None];
        if row == "fresh-branch" {
            task.status = TaskStatus::Open;
            // Managed linked checkouts share the factory's CAS identity/store.
            std::os::unix::fs::symlink(&cas_root, worker_path.join(".cas")).unwrap();
            std::fs::write(repo.root.join(".git/info/exclude"), ".cas\n").unwrap();
            task.deliverables.work_target = Some(WorkTarget {
                repo_selector: format!("project:github.com/org/{slug}"),
                target_branch: "main".into(),
            });
            let agents = open_agent_store(&cas_root).unwrap();
            let mut registration = agents.get(&worker_id).unwrap();
            registration.metadata.insert("clone_path".into(), worker_path.display().to_string());
            agents.update(&registration).unwrap();
        }
        if row == "unparked" {
            task.status = TaskStatus::Open;
        }
        task_store.add(&task).unwrap();

        if row == "fresh-branch" || row == "unparked" {
            env.set("CAS_AGENT_ROLE", "worker");
            delivery_service(&cas_root, &worker_id).task(Parameters(task_req(serde_json::json!({
                "action": "start", "id": task.id,
            })))).await.expect("record the original delivery branch through real start");
        }
        if row.ends_with("moved-target") {
            std::fs::write(
                worker_path.join("sibling.txt"),
                "already reviewed sibling\n",
            )
            .unwrap();
            run_git(&["add", "sibling.txt"], &worker_path);
            run_git(
                &["commit", "-m", "sibling already carried by source"],
                &worker_path,
            );
        }
        std::fs::create_dir_all(worker_path.join("docs")).unwrap();
        std::fs::write(worker_path.join(delivered_path), "parked delivery\n").unwrap();
        run_git(&["add", delivered_path], &worker_path);
        let subject = if row == "fresh-branch" {
            format!("{}: parked delivery", task.id)
        } else {
            "parked delivery".to_string()
        };
        run_git(&["commit", "-m", &subject], &worker_path);

        let mut delivery_branch = format!("factory/{worker}");
        if row == "unparked" {
            run_git(&["push", "origin", &delivery_branch], &worker_path);
        }
        if row == "fresh-branch" {
            let original = task_store.get(&task.id).unwrap();
            let original_tip = git_stdout(&worker_path, &["rev-parse", "HEAD"]);
            assert_eq!(original.deliverables.factory_branch_start.as_ref().unwrap().branch, delivery_branch);
            run_git(&["push", "origin", &delivery_branch], &worker_path);
            delivery_branch = format!("factory/{worker}-{}-reviewed", task.id);
            run_git(&["switch", "-c", &delivery_branch, "main"], &worker_path);
            std::fs::write(worker_path.join(delivered_path), "fresh branch delivery\n").unwrap();
            run_git(&["add", delivered_path], &worker_path);
            run_git(&["commit", "-m", &format!("{}: fresh delivery", task.id)], &worker_path);
            run_git(&["push", "origin", &delivery_branch], &worker_path);
            let rebind = || task_req(serde_json::json!({
                "action": "rebind_branch", "id": task.id, "branch": delivery_branch,
                "reason": "Supervisor reviewed replacement branch after fresh-base recovery",
            }));
            let worker_denial = delivery_service(&cas_root, &worker_id)
                .task(Parameters(rebind())).await.expect_err("workers cannot approve their own rebind");

            assert_eq!(task_store.get(&task.id).unwrap().deliverables.factory_branch_start,
                original.deliverables.factory_branch_start);
            env.set("CAS_AGENT_ROLE", "supervisor");
            let supervisor = delivery_service(&cas_root, &supervisor_id);
            // A failed review keeps its parked delivery until request_changes.
            let mut blocked = original.clone();
            blocked.status = TaskStatus::Blocked;
            blocked.deliverables.parked_branch = Some(format!("factory/{worker}"));
            blocked.deliverables.factory_branch_anchor = Some(original_tip.clone());
            task_store.update(&blocked).unwrap();
            let blocked_snapshot = serde_json::to_value(task_store.get(&task.id).unwrap()).unwrap();
            let refused = supervisor.task(Parameters(rebind())).await
                .expect_err("a blocked parked delivery must be declined before rebinding");
            assert!(refused.message.contains("request_changes"), "{refused}");
            assert_eq!(serde_json::to_value(task_store.get(&task.id).unwrap()).unwrap(), blocked_snapshot,
                "refusing a parked delivery must not mutate its recorded evidence");
            task_store.update(&original).unwrap();
            run_git(&["commit", "--allow-empty", "-m", "cas-dead: foreign task work"], &worker_path);
            let refused = supervisor.task(Parameters(rebind())).await.expect_err("foreign work cannot be hidden by a new base");

            run_git(&["reset", "--hard", "HEAD^"], &worker_path);
            supervisor.task(Parameters(rebind())).await.expect("supervisor rebinds fresh delivery");
            let rebound = task_store.get(&task.id).unwrap();
            let start = rebound.deliverables.factory_branch_start.as_ref().unwrap();
            assert_eq!(start.branch, delivery_branch);
            assert_eq!(start.base_sha, git_stdout(&repo.root, &["rev-parse", "main"]));
            assert_eq!(rebound.assignee, original.assignee);
            assert_eq!(rebound.deliverables.work_target, original.deliverables.work_target);
            assert!(rebound.notes.contains(&original_tip), "old checkpoint tip must remain auditable");
            supervisor.task(Parameters(rebind())).await.expect("same binding is idempotent");
            assert_eq!(task_store.get(&task.id).unwrap().notes, rebound.notes);
        }

        // The worker's own close parks the delivery and keeps its reason.
        let parked_reason = format!("PASS: delivered.txt for {row}");
        env.set("CAS_AGENT_ROLE", "worker");
        let park = if row == "unparked" { String::new() } else { get_text(
            &delivery_service(&cas_root, &worker_id)
                .task(Parameters(task_req(serde_json::json!({
                    "action": "close", "id": task.id, "reason": parked_reason,
                }))))
                .await
                .expect("worker close"),
        ) };
        let parked = task_store.get(&task.id).unwrap();
        let parked_status = if row == "unparked" { TaskStatus::InProgress } else { TaskStatus::AwaitingMerge };
        assert_eq!(parked.status, parked_status, "{row}: {park}");
        if row == "fresh-branch" {
            assert!(park.contains("MERGE REQUIRED"), "{park}");
            assert_eq!(parked.deliverables.parked_branch.as_deref(), Some(delivery_branch.as_str()));
            assert_eq!(parked.deliverables.factory_branch_anchor.as_deref(),
                Some(git_stdout(&worker_path, &["rev-parse", "HEAD"]).as_str()));
        }

        env.set("CAS_AGENT_ROLE", "supervisor");
        let supervisor = delivery_service(&cas_root, &supervisor_id);
        let mut merge = coord_req("worktree_merge");
        merge.id = Some(delivery_branch.clone());
        merge.task_id = Some(task.id.clone());
        merge.allow_trunk = Some(true);
        merge.cleanup = Some(false);
        if conclusion == "failure" {
            merge.supervisor_override = Some(true);
            merge.reason = Some("red run reviewed".to_string());
        }
        let target_before = git_stdout(&repo.root, &["rev-parse", "main"]);
        let held = supervisor.factory(Parameters(merge.clone())).await
            .expect_err("the candidate must be reviewed before publication, even with a CI override");

        assert_eq!(git_stdout(&repo.root, &["rev-parse", "main"]), target_before);
        assert!(!cas_git_is_ancestor(&repo.root, &delivery_branch, "main"));
        let dispatch = cas_store::get_latest_verification_dispatch(&cas_root, &task.id)
            .unwrap().expect("pre-merge dispatch");
        let proof = dispatch.repository.as_ref().expect("candidate repository proof");
        assert_ne!(proof.head_commit, target_before, "{row}: review the candidate tree");
        assert!(cas_git_is_ancestor(&repo.root, &delivery_branch, &proof.head_commit));
        assert_ne!(task_store.get(&task.id).unwrap().status, TaskStatus::Closed);
        if row == "green-approved" {
            // cas-6a8b/cas-e051: the owner receives one typed handoff
            // before publication, independently of any later COMMITTED reply.
            assert_eq!(dispatch.owner_agent_id, supervisor_id);
            assert_eq!(dispatch.requester_agent_id, supervisor_id);
            let handoffs: Vec<_> = cas::store::open_prompt_queue_store(&cas_root)
                .unwrap()
                .peek_all(50)
                .unwrap()
                .into_iter()
                .filter(|prompt| prompt.source == format!("verification-dispatch:{}", dispatch.id))
                .collect();
            assert_eq!(handoffs.len(), 1, "{row}: background dispatch {} must reach its owner before expiry", dispatch.id);
            let handoff = &handoffs[0];
            assert_eq!(handoff.target, format!("{slug}-supervisor"), "the handoff addresses the bound supervisor, not a role alias");
            assert_eq!(handoff.origin, Some(cas_store::QueueOrigin::Daemon));
            assert_eq!(handoff.priority, cas_store::NotificationPriority::High);
            assert!(handoff.created_at < dispatch.deadline_at);
            assert!(chrono::Utc::now() < dispatch.deadline_at);
            // pin: the daemon's wake parser requires this typed XML envelope and its dispatch identity.
            assert!(handoff.prompt.contains(&format!("<cas-verification-dispatch dispatch_id=\"{}\"", dispatch.id)), "{}", handoff.prompt);
            // pin: the wake parser must route the dispatch to its bound supervisor owner.
            assert!(handoff.prompt.contains(&format!("owner=\"{supervisor_id}\"")), "{}", handoff.prompt);
            // pin: the wake parser revalidates the typed dispatch's exact deadline.
            assert!(handoff.prompt.contains(&format!("deadline=\"{}\"", dispatch.deadline_at.to_rfc3339())), "{}", handoff.prompt);
        }
        if row == "worker-blocked" {
            // The removed post-merge worker relay is now a pre-merge hold.
            // A worker retry cannot manufacture approval or publish the lane.
            env.set("CAS_AGENT_ROLE", "worker");
            let retry = delivery_service(&cas_root, &worker_id).task(Parameters(task_req(serde_json::json!({
                "action": "close", "id": task.id, "reason": parked_reason,
            })))).await;
            assert_ne!(task_store.get(&task.id).unwrap().status, TaskStatus::Closed, "{retry:?}");
            assert_eq!(git_stdout(&repo.root, &["rev-parse", "main"]), target_before);
            env.set("CAS_AGENT_ROLE", "supervisor");
        }
        if row.ends_with("moved-target") {
            let source = git_stdout(&repo.root, &["rev-parse", &delivery_branch]);
            let tree = git_stdout(
                &repo.root,
                &["rev-parse", &format!("{}^{{tree}}", proof.head_commit)],
            );
            let sibling = git_stdout(&repo.root, &["rev-parse", &format!("{delivery_branch}^1")]);
            run_git(
                &[
                    "merge",
                    "--no-ff",
                    &sibling,
                    "-m",
                    "queued sibling landing while review runs",
                ],
                &repo.root,
            );
            run_git(&["push", "origin", "main"], &repo.root);
            assert_ne!(
                git_stdout(&repo.root, &["rev-parse", "origin/main"]),
                target_before
            );
            assert_eq!(
                git_stdout(&repo.root, &["rev-parse", &delivery_branch]),
                source
            );
            let recomputed = git_stdout(
                &repo.root,
                &[
                    "merge-tree",
                    "--write-tree",
                    "origin/main",
                    &delivery_branch,
                ],
            );
            assert_eq!(
                recomputed.lines().next(),
                Some(tree.as_str()),
                "moved target keeps the reviewed candidate tree"
            );
        }
        let late_child = if row.starts_with("late-child") {
            let child_id = format!("{slug}-verifier-session");
            let mut child = Agent::new(child_id.clone(), "task-verifier".into());
            child.agent_type = AgentType::SubAgent;
            child.role = AgentRole::Standard;
            child.parent_id = Some(supervisor_id.clone());
            child.factory_session = Some(session.clone());
            child.heartbeat();
            cas_store::issue_server_verifier_handoff(
                &cas_root,
                &task.id,
                &dispatch.id,
                &supervisor_id,
                "late-verifier-tool",
            )
            .unwrap();
            cas_store::bind_server_verifier_handoff_and_register_child(
                &cas_root,
                &supervisor_id,
                &child,
            )
            .unwrap();
            Some(child_id)
        } else {
            None
        };
        if row.starts_with("late-") {
            // Advance the dispatch's clock, never sleep through a deadline.
            let now = chrono::Utc::now();
            let conn = rusqlite::Connection::open(cas_root.join("cas.db")).unwrap();
            conn.execute(
                "UPDATE verification_dispatches SET requested_at=?2,deadline_at=?3 WHERE id=?1",
                rusqlite::params![
                    dispatch.id,
                    (now - chrono::Duration::minutes(12)).to_rfc3339(),
                    (now - chrono::Duration::minutes(2)).to_rfc3339()
                ],
            )
            .unwrap();
            if row.starts_with("late-child") {
                // Move the existing credential's clock by the same 12 minutes:
                // its actual issuance TTL, not a fabricated expiry, must permit late work.
                let issued_expiry: String = conn.query_row(
                    "SELECT expires_at FROM verification_capabilities WHERE dispatch_id=?1",
                    [&dispatch.id], |record| record.get(0),
                ).unwrap();
                let expires_at = if row == "late-child-capability-expired" {
                    now - chrono::Duration::minutes(2)
                } else {
                    chrono::DateTime::parse_from_rfc3339(&issued_expiry).unwrap()
                        .with_timezone(&chrono::Utc) - chrono::Duration::minutes(12)
                };
                conn.execute(
                    "UPDATE verification_capabilities SET issued_at=?2,bound_at=?2,expires_at=?3 WHERE dispatch_id=?1",
                    rusqlite::params![dispatch.id, (now - chrono::Duration::minutes(12)).to_rfc3339(), expires_at.to_rfc3339()],
                ).unwrap();
            }
            drop(conn);
            if row == "late-timedout-approved" || row.starts_with("late-child") {
                let expired = cas_store::timeout_verification_dispatch(&cas_root, &task.id, now)
                    .unwrap()
                    .unwrap();
                assert_eq!(expired.id, dispatch.id);
            }
        }
        if row == "late-child-superseded" {
            cas_store::invalidate_verification_dispatch_for_new_cycle(&cas_root, &task.id).unwrap();
            let next = cas_store::create_verification_dispatch_bound(
                &cas_root,
                &task.id,
                &supervisor_id,
                &supervisor_id,
                &cas::types::VerificationProofBoundary::task_at(proof.clone()),
                chrono::Utc::now() + chrono::Duration::minutes(10),
                false,
            )
            .unwrap();
            assert_ne!(next.id, dispatch.id);
        }
        if row == "late-child-target-changed" {
            std::fs::write(
                repo.root.join("README.md"),
                "unrelated target change during review\n",
            )
            .unwrap();
            run_git(
                &[
                    "commit",
                    "-am",
                    "unrelated target tree changed during review",
                ],
                &repo.root,
            );
            run_git(&["push", "origin", "main"], &repo.root);
        }
        if row == "late-child-changed" {
            std::fs::write(worker_path.join(delivered_path), "changed after review\n").unwrap();
            run_git(
                &["commit", "-am", "changed candidate after verifier started"],
                &worker_path,
            );
        }
        let verdict = verdict.expect("every publication needs a candidate verdict");
        let request = verification_req(serde_json::json!({
            "action": "add", "task_id": task.id, "status": verdict,
            "summary": format!("{verdict} {row}"), "confidence": 1.0,
            "dispatch_id": dispatch.id,
        }));
        let result = if let Some(child_id) = late_child {
            delivery_service(&cas_root, &child_id)
                .verification(Parameters(request))
                .await
        } else {
            supervisor.verification(Parameters(request)).await
        };
        if matches!(
            row,
            "late-child-superseded" | "late-child-changed" | "late-child-target-changed" | "late-child-capability-expired"
        ) {
            let refused =
                result.expect_err("late authority must refuse changed candidates or expired credentials");
            assert!(
                refused.message.contains("superseded") || refused.message.contains("changed")
                    || (row == "late-child-capability-expired"),
                "{row}: {refused}"
            );
            assert!(
                cas_store::get_verification_for_dispatch(&cas_root, &dispatch.id)
                    .unwrap()
                    .is_none()
            );
            if row == "late-child-superseded" {
                assert_ne!(
                    cas_store::get_latest_verification_dispatch(&cas_root, &task.id)
                        .unwrap()
                        .unwrap()
                        .id,
                    dispatch.id
                );
            }
            continue;
        }
        let recorded = get_text(&result.expect("an exact bound late verdict must be accepted"));
        if row.starts_with("late-") {
            let resolved = cas_store::get_latest_verification_dispatch(&cas_root, &task.id)
                .unwrap()
                .unwrap();
            let accepted = cas_store::get_verification_for_dispatch(&cas_root, &dispatch.id)
                .unwrap()
                .unwrap();
            assert_eq!(
                resolved.id, dispatch.id,
                "record keeps the reviewed dispatch"
            );
            assert_eq!(
                resolved.state,
                cas::types::VerificationDispatchState::Resolved
            );
            assert!(
                accepted.created_at > resolved.deadline_at,
                "the record actually crossed the deadline"
            );
            assert!(recorded.contains("approved"), "{recorded}");
        }
        assert_ne!(task_store.get(&task.id).unwrap().status, TaskStatus::Closed,
            "approval alone cannot close an unpublished delivery: {row}: {recorded}");
        if verdict == "rejected" {
            let denied = supervisor.factory(Parameters(merge)).await.expect_err("rejection holds publication");
            assert!(denied.message.contains(&dispatch.id), "{denied}");
            assert_eq!(git_stdout(&repo.root, &["rev-parse", "main"]), target_before);
            assert!(!cas_git_is_ancestor(&repo.root, &delivery_branch, "main"));
            let declined = task_store.get(&task.id).unwrap();
            // Candidate rejection holds this unpublished delivery; the
            // supervisor explicitly returns it for repair (cas-e051).
            assert_eq!(declined.status, TaskStatus::AwaitingMerge, "{row}: {recorded}");
            assert_eq!(declined.assignee.as_deref(), Some(worker.as_str()));
            assert!(declined.deliverables.green_merge_commit.is_none());
            assert_eq!(cas_store::get_verification_for_dispatch(&cas_root, &dispatch.id).unwrap().unwrap().status,
                cas::types::VerificationStatus::Rejected);
            assert_eq!(cas_store::get_latest_verification_dispatch(&cas_root, &task.id).unwrap().unwrap().id,
                dispatch.id, "retries cannot create a post-merge approval");
            let findings_reason = format!("rejected {row}");
            supervisor.task(Parameters(task_req(serde_json::json!({
                "action": "request_changes", "id": task.id, "reason": findings_reason,
            })))).await.expect("supervisor returns the rejected candidate for repair");
            let returned = task_store.get(&task.id).unwrap();
            assert_eq!(returned.status, TaskStatus::Open);
            assert_eq!(returned.assignee.as_deref(), Some(worker.as_str()));
            assert!(returned.notes.contains(&findings_reason));
            // The pre-merge verdict wakes its supervisor; worker findings
            // were emitted by the retired post-merge rejection relay.
            let supervisor_name = format!("{slug}-supervisor");
            let notices = cas::store::open_prompt_queue_store(&cas_root).unwrap().peek_all(50).unwrap()
                .into_iter().filter(|prompt| prompt.target == supervisor_name
                    && prompt.summary.as_deref() == Some("premerge_verification")
                    && prompt.prompt.contains(&dispatch.id)).count();
            assert_eq!(notices, 1, "one supervisor verdict handoff");
            continue;
        }
        let merged = if row == "green-approved" {
            // A close slower than the post-commit grace, deterministically.
            env.set("CAS_TEST_POST_MERGE_DELAY_MS", "300");
            // cas-3937: under the real response budget with no post-commit
            // grace, the published merge answers COMMITTED with its merge
            // and push state, and the close after it finishes in the
            // background, recording its result on the task.
            let answer = get_text(
                &supervisor
                    .factory_with_budget_for_testing(
                        merge,
                        std::time::Duration::from_secs(50),
                        std::time::Duration::ZERO,
                    )
                    .await
                    .expect("merge"),
            );
            // pin: the early answer names the merge and its local-only target (cas-3937).
            assert!(answer.contains("COMMITTED") && answer.contains("local-only target"), "{row}: {answer}");
            let mut notes = String::new();
            for _ in 0..400 {
                notes = task_store.get(&task.id).unwrap().notes;
                if notes.contains("POST_COMMIT_OUTCOME") {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            assert!(notes.contains("POST_COMMIT_OUTCOME"), "{row}: the close's result is kept: {notes}");
            // pin: the note carries the close's own outcome and the task's status (cas-3937 review).
            assert!(
                notes.contains("Cassy closed task") && notes.contains("task status now `closed`"),
                "{row}: {notes}"
            );
            answer
        } else {
            get_text(&supervisor.factory(Parameters(merge)).await.expect("merge"))
        };
        let merge_sha = git_stdout(&repo.root, &["rev-parse", "main"]);
        let after_merge = task_store.get(&task.id).unwrap();
        assert_eq!(after_merge.status == TaskStatus::Closed, closes, "{row}: {merged}");
        let admission = match conclusion {
            "success" => "green",
            "failure" => "override",
            _ => "advisory",
        };
        let stored = serde_json::to_value(&after_merge.deliverables).unwrap();
        assert_eq!(stored["merge_ci_admission"], admission, "{row}: {merged}");
        let roundtrip: cas::types::TaskDeliverables = serde_json::from_value(stored).unwrap();
        assert_eq!(serde_json::to_value(roundtrip).unwrap()["merge_ci_admission"], admission);
        let shown = get_text(&supervisor.task(Parameters(task_req(serde_json::json!({
            "action": "show", "id": task.id,
        })))).await.expect("show merged task"));
        assert!(shown.contains(&format!("CI admission: {admission}")), "{row}: {shown}");

        if admission == "advisory" {
            assert!(after_merge.notes.contains("CI unconfirmed"), "{row}: {merged}");
        }
        assert_eq!(after_merge.deliverables.green_merge_commit.as_deref(), Some(merge_sha.as_str()), "{row}: {merged}");
        let after_dispatch = cas_store::get_latest_verification_dispatch(&cas_root, &task.id).unwrap().unwrap();
        assert_eq!(after_dispatch.id, dispatch.id, "publication reuses the pre-merge verdict");
        assert_eq!(after_dispatch.repository, dispatch.repository, "publication cannot replace the reviewed proof");
        if closes {
            if row == "unparked" {
                assert!(after_merge.close_reason.as_deref().is_some_and(|reason| reason.contains(&merge_sha)), "{row}: {:?}", after_merge.close_reason);
            } else {
                assert_eq!(after_merge.close_reason.as_deref(), Some(parked_reason.as_str()));
            }
            assert_eq!(after_merge.assignee.as_deref(), Some(worker.as_str()), "{row}: attributed to the assignee");
        } else {
            assert_eq!(after_merge.status, TaskStatus::AwaitingMerge, "CI override stays a manual close: {row}: {merged}");
        }

        let worker_prompts: Vec<String> = cas::store::open_prompt_queue_store(&cas_root)
            .unwrap()
            .peek_all(50)
            .unwrap()
            .into_iter()
            .filter(|prompt| prompt.target == worker)
            .map(|prompt| prompt.prompt)
            .collect();
        let expected = usize::from(closes);
        assert_eq!(
            worker_prompts.iter().filter(|p| p.contains("Cassy merged it")).count(),
            expected,
            "{row}: the worker hears exactly once, only when Cassy closed: {worker_prompts:?}"
        );
        assert!(
            !worker_prompts.iter().any(|p| p.contains("Retry task close")),
            "{row}: Cassy closes or reopens; the worker never re-closes: {worker_prompts:?}"
        );
        assert_eq!(
            worker_prompts.iter().filter(|p| p.contains("Cassy parked it")).count(),
            usize::from(row == "unparked"),
            "{row}: a worker parked by the merge hears it once (cas-e9d6): {worker_prompts:?}"
        );
        if closes && admission == "advisory" {
            assert!(worker_prompts.iter().any(|p| p.contains("CI advisory (unconfirmed)")), "{worker_prompts:?}");
            assert!(!worker_prompts.iter().any(|p| p.contains("with green CI")), "{worker_prompts:?}");
        }
    }
}

// =============================================================================
// Regressions
// =============================================================================

/// cas-5a7e: the transactional path records its published merge before its
/// internal close. Stretch that close on the real MCP response path: the
/// caller must receive the merge receipt promptly, then find the close's
/// actual result on the task rather than losing it with the early answer.
#[tokio::test]
async fn transactional_delivery_slow_close_reports_commit_and_close_outcome() {
    let mut env = TestEnvGuard::temp_home();
    let fixture = arm_delivery_with_selector(
        "slowclose",
        "slow-close",
        &mut env,
        "project:github.com/org/slow-close",
    )
    .await;
    env.set("CAS_TEST_POST_MERGE_DELAY_MS", "300");
    let _dir = env.enter_dir(&fixture.repo.root);

    let supervisor = delivery_service(&fixture.cas_root, &fixture.supervisor_id);
    let mut merge = coord_req("worktree_merge");
    merge.id = Some(fixture.receipt.source_branch.clone());
    merge.task_id = Some(fixture.task_id.clone());
    merge.allow_trunk = Some(true);
    merge.cleanup = Some(false);
    let started = std::time::Instant::now();
    let answer = get_text(
        &supervisor
            .factory_with_budget_for_testing(
                merge,
                std::time::Duration::from_secs(20),
                std::time::Duration::ZERO,
            )
            .await
            .expect("transactional merge"),
    );
    assert!(answer.contains("COMMITTED"), "response took {:?}: {answer}", started.elapsed());
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "response exceeded 20 seconds: {answer}");
    assert!(answer.contains("local-only target"), "{answer}");
    assert!(answer.contains(&git_stdout(&fixture.repo.root, &["rev-parse", "main"])), "{answer}");
    assert_eq!(
        delivery_state(&fixture),
        WorkerDeliveryState::MergeAuthorized,
        "early answer must precede the delivery close: {answer}",
    );

    let task_store = open_task_store(&fixture.cas_root).unwrap();
    let mut notes = String::new();
    for _ in 0..400 {
        notes = task_store.get(&fixture.task_id).unwrap().notes;
        if notes.contains("POST_COMMIT_OUTCOME") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(notes.contains("POST_COMMIT_OUTCOME"), "close outcome missing: {notes}");
    assert!(
        notes.contains("task status now `closed`")
            && notes.contains("Close outcome: Closed task: cas-slowclose-target-cas"),
        "{notes}"
    );
    assert_eq!(delivery_state(&fixture), WorkerDeliveryState::Delivered);
}

/// cas-6a57: in a debug build the poll frame of an `async fn` holds a slot for
/// every child future it constructs, so a merge that closes its delivery
/// nested 1.79 MB of frames, overflowed tokio's 2 MiB worker and aborted
/// `cas serve` after the merge had landed. Measure the real path's high-water
/// mark on a worker with room to spare, so a regression reports its size
/// instead of aborting the test binary.
#[cfg(target_os = "linux")]
#[test]
fn supervisor_merge_through_close_stays_under_stack_budget_cas_6a57() {
    /// Half of tokio's default worker stack. Measured on this fixture in a
    /// dev-profile build: 2028440 bytes before the frames moved to the heap,
    /// 782200 after. The headroom covers frames that vary with the operator's
    /// config and environment.
    const STACK_BUDGET: usize = 1024 * 1024;
    const WORKER_STACK: usize = 8 * 1024 * 1024;
    const PAINTED: usize = 7 * 1024 * 1024;
    const PAINT: u64 = 0x5afe_57ac_6a57_5afe;

    /// Fill the unused stack below this frame; returns its bounds.
    #[inline(never)]
    fn paint_stack() -> (usize, usize) {
        let marker = 0u8;
        let top = std::hint::black_box(&marker) as *const u8 as usize & !7;
        let low = top - PAINTED;
        for at in (low..top - 64 * 1024).step_by(8) {
            // SAFETY: the range lies inside this thread's 8 MiB mapping, below
            // every live frame, and only the paint below reads it back.
            unsafe { (at as *mut u64).write_volatile(PAINT) };
        }
        (low, top)
    }

    /// Bytes below `top` that any frame has written since `paint_stack`.
    fn stack_high_water((low, top): (usize, usize)) -> usize {
        let mut at = low;
        // SAFETY: as in `paint_stack`; `at` stays below the caller's frames.
        while at < top && unsafe { (at as *const u64).read_volatile() } == PAINT {
            at += 8;
        }
        top - at
    }

    let mut env = TestEnvGuard::temp_home();
    let repo = GitRepo::new();
    let slug = "stack-budget";
    let remote = format!("git@github.com:org/{slug}.git");
    run_git(&["remote", "add", "origin", &remote], &repo.root);
    let origin = TempDir::new_in(cas::test_paths::runtime_fixture_parent()).unwrap();
    run_git(&["init", "-q", "--bare", origin.path().to_str().unwrap()], &repo.root);
    run_git(&["config", &format!("url.{}.insteadOf", origin.path().display()), &remote], &repo.root);
    let cas_root = init_cas_dir(&repo.root).expect("init CAS");
    env.set(cas::github_issue_attach::GH_BIN_ENV, ci_check_run::ci_concluded(&cas_root, "success"));
    std::fs::write(
        cas_root.join("config.toml"),
        format!("[project]\ncanonical_id = \"github.com/org/{slug}\"\n[worktrees]\nenabled = false\n[verification]\nenabled = true\n"),
    )
    .expect("write config");
    let worker_id = format!("{slug}-worker-session");
    let supervisor_id = format!("{slug}-supervisor-session");
    let session = format!("{slug}-factory");
    register_delivery_agent(&cas_root, &worker_id, slug, AgentRole::Worker, &session);
    register_delivery_agent(&cas_root, &supervisor_id, &format!("{slug}-supervisor"), AgentRole::Supervisor, &session);
    let worker_path = cas_root.join("worktrees").join(slug);
    repo.add_worktree(&worker_path, &format!("factory/{slug}"));
    std::fs::write(worker_path.join("delivered.txt"), "parked delivery\n").unwrap();
    run_git(&["add", "delivered.txt"], &worker_path);
    run_git(&["commit", "-m", "parked delivery"], &worker_path);
    let task_store = open_task_store(&cas_root).unwrap();
    let mut task = Task::new("cas-stack-budget".to_string(), "Parked delivery".to_string());
    task.task_type = TaskType::Task;
    task.status = TaskStatus::InProgress;
    task.depth = TaskDepth::Deep;
    task.assignee = Some(slug.to_string());
    task.risk = vec![cas::types::TaskRisk::None];
    task_store.add(&task).unwrap();

    // One worker keeps every poll of the merge on the painted stack.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_stack_size(WORKER_STACK)
        .enable_all()
        .build()
        .unwrap();
    env.set("CAS_AGENT_ROLE", "worker");
    runtime
        .block_on(delivery_service(&cas_root, &worker_id).task(Parameters(task_req(serde_json::json!({
            "action": "close", "id": task.id, "reason": "PASS: delivered.txt",
        })))))
        .expect("worker close");
    assert_eq!(task_store.get(&task.id).unwrap().status, TaskStatus::AwaitingMerge);

    env.set("CAS_AGENT_ROLE", "supervisor");
    let supervisor = delivery_service(&cas_root, &supervisor_id);
    let mut merge = coord_req("worktree_merge");
    merge.id = Some(format!("factory/{slug}"));
    merge.task_id = Some(task.id.clone());
    merge.allow_trunk = Some(true);
    merge.cleanup = Some(false);
    // cas-c744: candidate review is a separate phase; keep the painted
    // measurement on the approved publication through ordinary close.
    let held = runtime.block_on(supervisor.factory(Parameters(merge.clone())))
        .expect_err("prepare the pre-merge candidate");

    assert!(!cas_git_is_ancestor(&repo.root, &format!("factory/{slug}"), "main"));
    let dispatch = cas_store::get_latest_verification_dispatch(&cas_root, &task.id).unwrap().unwrap();
    runtime.block_on(supervisor.verification(Parameters(verification_req(serde_json::json!({
        "action": "add", "task_id": task.id, "status": "approved",
        "summary": "stack fixture candidate approved before publication", "dispatch_id": dispatch.id,
    }))))).expect("pre-merge approval");
    let (reply, high_water) = runtime
        .block_on(runtime.spawn(async move {
            let painted = paint_stack();
            let reply = supervisor.factory(Parameters(merge)).await;
            (reply, stack_high_water(painted))
        }))
        .unwrap();

    let reply = get_text(&reply.expect("merge replies"));
    assert!(
        cas_git_is_ancestor(&repo.root, &format!("factory/{slug}"), "main"),
        "the delivery landed: {reply}"
    );
    assert!(reply.contains(&format!("Cassy closed task {}", task.id)), "the reply names the close: {reply}");
    assert_eq!(task_store.get(&task.id).unwrap().status, TaskStatus::Closed);
    assert_eq!(cas_store::get_latest_verification_dispatch(&cas_root, &task.id).unwrap().unwrap().id,
        dispatch.id, "the measured close reuses the candidate verdict");
    assert!(
        high_water < STACK_BUDGET,
        "merge through close used {high_water} stack bytes, budget {STACK_BUDGET}"
    );
}

/// cas-96ba: transactional delivery must accept the task binding consumed by
/// worktree target resolution. The destructive-action field guard runs before
/// that resolver, so this proves `task_id` reaches the real merge path rather
/// than being rejected as an unrelated union field.
#[tokio::test]
async fn delivery_merge_accepts_task_id_and_reaches_target_resolution() {
    let home = TempDir::new().expect("temp HOME");
    let mut env = TestEnvGuard::new();
    env.set("HOME", home.path());
    let fixture = arm_delivery("taskidaccepted", "task-id-accepted", &mut env).await;

    assert_eq!(
        cas_store::get_latest_worker_delivery(&fixture.cas_root, &fixture.task_id)
            .expect("delivery lookup")
            .expect("persisted delivery")
            .0
            .artifact_path,
        fixture.receipt.artifact_path,
        "close must retain its configured durable artifact as queryable receipt metadata"
    );

    let _env = env.enter_dir(&fixture.repo.root);
    let output = run_merge(&fixture).await;

    assert!(
        !output.contains("Unsupported parameter(s)"),
        "task_id must reach target resolution, not fail the destructive-action guard:\n{output}"
    );
    assert!(
        matches!(
            delivery_state(&fixture),
            WorkerDeliveryState::Merged
                | WorkerDeliveryState::CloseReady
                | WorkerDeliveryState::Delivered
        ),
        "a task_id-bearing delivery merge must enter the typed merge flow:\n{output}"
    );
    assert_eq!(
        git_stdout(&fixture.repo.root, &["rev-parse", "main~1"]),
        fixture.receipt.target_sha,
        "the task-bound merge must resolve and merge against the reviewed target"
    );
}

/// cas-96ba: accepting the legitimate `task_id` must not broaden the
/// destructive action's union-field surface. `target` belongs to messaging,
/// not worktree_merge, and therefore remains fail-closed before any delivery
/// state can change.
#[tokio::test]
async fn delivery_merge_rejects_unrelated_union_parameter_before_target_resolution() {
    let home = TempDir::new().expect("temp HOME");
    let mut env = TestEnvGuard::new();
    env.set("HOME", home.path());
    let fixture = arm_delivery("taskidrejectsother", "task-id-rejects-other", &mut env).await;
    let supervisor_service = delivery_service(&fixture.cas_root, &fixture.supervisor_id);
    let mut merge = coord_req("worktree_merge");
    merge.id = Some(fixture.receipt.source_branch.clone());
    merge.task_id = Some(fixture.task_id.clone());
    merge.target = Some("not-a-worktree-merge-parameter".to_string());

    let error = supervisor_service
        .factory(Parameters(merge))
        .await
        .expect_err("unrelated worktree_merge parameter must fail closed");

    assert_eq!(
        delivery_state(&fixture),
        WorkerDeliveryState::AwaitingMerge,
        "the rejected request must not reach target resolution or mutate delivery state"
    );
}

/// Drift that lands *before* the merge is entered must be refused as the
/// typed, recoverable `TipChanged` — target drift is a tip change, and the
/// supervisor needs `is_recoverable_failure()` to offer recovery.
/// cas-4150: the red-CI refusal tells the supervisor to retry with
/// `supervisor_override=true`, `task_id` and a non-empty `reason`. Those
/// parameters must pass the destructive-action allowlist, a missing reason is
/// still refused, and an accepted override merges and records its decision.
#[tokio::test]
async fn red_ci_worktree_merge_accepts_supervisor_override_end_to_end_cas_4150() {
    let home = TempDir::new().expect("temp HOME");
    let fake_bin = TempDir::new().expect("fake gh dir");
    let gh = fake_bin.path().join("gh");
    cas::test_paths::warm_stub(&gh, "#!/bin/sh\nif [ \"$1 $2 $3\" = 'repo view org/red-ci-override' ]; then printf '%s\\n' '{\"nameWithOwner\":\"Richards-LLC/cassy\"}'; exit 0; fi\nif [ \"$1 $2\" = 'api graphql' ]; then printf '%s\\n' '{\"data\":{\"repository\":{\"mergeQueue\":null}}}'; exit 0; fi\ncase \"$4\" in repos/Richards-LLC/cassy/commits/*/check-runs) ;; *) echo 'HTTP 422: No commit found for SHA' >&2; exit 1 ;; esac\ncat <<'JSON'\n{\"check_runs\":[{\"name\":\"Fast Validation\",\"status\":\"completed\",\"conclusion\":\"failure\",\"html_url\":\"https://github.com/org/repo/actions/runs/4150\"}]}\nJSON\n");
    let mut env = TestEnvGuard::new();
    env.set("HOME", home.path());
    // cas-9790: select the fake through Cassy's `CAS_GH_BIN` seam; process
    // PATH stays untouched (factory_mcp_ops_test PATH-isolation lint).
    env.set(cas::github_issue_attach::GH_BIN_ENV, &gh);
    let fixture = arm_delivery("redcioverride", "red-ci-override", &mut env).await;
    // cas-28c8: origin maps to a renamed canonical repo; upstream/default must
    // not select CI. The fake refuses every noncanonical endpoint.
    run_git(&["remote", "add", "upstream", "https://github.com/codingagentsystem/cas.git"], &fixture.repo.root);
    env.set("GH_REPO", "codingagentsystem/cas");
    let supervisor_service = delivery_service(&fixture.cas_root, &fixture.supervisor_id);
    let merge_with = |supervisor_override: Option<bool>, reason: Option<&str>| {
        let mut merge = coord_req("worktree_merge");
        merge.id = Some(fixture.receipt.source_branch.clone());
        merge.task_id = Some(fixture.task_id.clone());
        merge.allow_trunk = Some(true);
        merge.cleanup = Some(false);
        merge.supervisor_override = supervisor_override;
        merge.reason = reason.map(str::to_string);
        merge
    };
    let outcome = |result: Result<rmcp::model::CallToolResult, rmcp::ErrorData>| match result {
        Ok(result) => get_text(&result),
        Err(error) => format!("MCP_ERROR: {error}"),
    };

    let refused = outcome(supervisor_service.factory(Parameters(merge_with(None, None))).await);
    assert!(refused.contains("CI RED"), "{refused}");
    assert!(refused.contains(&format!("repos/Richards-LLC/cassy/commits/{}/check-runs", fixture.receipt.commit_sha)), "{refused}");
    assert!(refused.contains("supervisor_override=true"), "{refused}");
    // Refusals happen after merge authorization but before Git: the target
    // is untouched and the delivery has not merged.
    let unmerged = |label: &str| {
        assert_eq!(
            git_stdout(&fixture.repo.root, &["rev-parse", "main"]),
            fixture.receipt.target_sha,
            "{label}: the target must not move"
        );
        assert_eq!(
            delivery_state(&fixture),
            WorkerDeliveryState::AwaitingMerge,
            "{label}: the refused delivery must remain retryable"
        );
    };
    unmerged("red CI without override");

    let no_reason =
        outcome(supervisor_service.factory(Parameters(merge_with(Some(true), None))).await);
    assert!(!no_reason.contains("Unsupported parameter(s)"), "{no_reason}");
    assert!(no_reason.contains("non-empty reason"), "{no_reason}");
    unmerged("override without reason");

    let merged = outcome(
        supervisor_service
            .factory(Parameters(merge_with(
                Some(true),
                Some("red run is the known flaky runner; delivery reviewed"),
            )))
            .await,
    );
    assert!(!merged.contains("Unsupported parameter(s)"), "{merged}");
    assert!(!merged.starts_with("MCP_ERROR"), "{merged}");
    assert!(
        merged.contains("red CI was explicitly overridden by a registered supervisor"),
        "{merged}"
    );
    let overrides = cas_store::quality_interventions_since(
        &fixture.cas_root,
        cas_store::InterventionKind::OverrideMerge,
        chrono::Utc::now() - chrono::Duration::minutes(5),
    )
    .unwrap();
    assert_eq!(overrides.len(), 1, "{overrides:?}");
    assert!(
        overrides[0].local_note.as_deref().is_some_and(|note| note.contains("flaky runner")),
        "{overrides:?}"
    );
    assert!(
        matches!(
            delivery_state(&fixture),
            WorkerDeliveryState::Merged
                | WorkerDeliveryState::CloseReady
                | WorkerDeliveryState::Delivered
        ),
        "an accepted override must merge the delivery:\n{merged}"
    );
    let notes = open_task_store(&fixture.cas_root)
        .expect("task store")
        .get(&fixture.task_id)
        .expect("task")
        .notes;
    assert!(notes.contains("overrode red lane CI"), "{notes}");
    assert!(notes.contains("known flaky runner"), "{notes}");
    assert!(notes.contains("actions/runs/4150"), "{notes}");
}

/// cas-d1eb: refusing red CI must withdraw merge intent without consuming
/// the approved receipt, so the supervisor can leave it parked or retry it.
#[tokio::test]
async fn red_ci_refusal_restores_awaiting_merge_cas_d1eb() {
    let home = TempDir::new().expect("temp HOME");
    let fake_bin = TempDir::new().expect("fake gh dir");
    let gh = fake_bin.path().join("gh");
    cas::test_paths::warm_stub(
        &gh,
        "#!/bin/sh\nif [ \"$1 $2\" = 'repo view' ]; then printf '{\"nameWithOwner\":\"%s\"}\\n' \"$3\"; exit 0; fi\ncat <<'JSON'\n{\"check_runs\":[{\"name\":\"Fast Validation\",\"status\":\"completed\",\"conclusion\":\"failure\"}]}\nJSON\n",
    );
    let mut env = TestEnvGuard::new();
    env.set("HOME", home.path());
    env.set(cas::github_issue_attach::GH_BIN_ENV, &gh);
    let fixture = arm_delivery("redciretry", "red-ci-retry", &mut env).await;
    let (receipt, approved) =
        cas_store::get_latest_worker_delivery(&fixture.cas_root, &fixture.task_id)
            .unwrap()
            .unwrap();
    assert_eq!(approved.state, WorkerDeliveryState::AwaitingMerge);

    // Include a transaction stranded by an earlier runtime, not just a newly
    // authorized merge. Both refusals must leave the same retryable boundary.
    for prior_authorization in [false, true] {
        if prior_authorization {
            cas_store::transition_worker_delivery(
                &fixture.cas_root,
                &approved.id,
                &[WorkerDeliveryState::AwaitingMerge],
                WorkerDeliveryState::MergeAuthorized,
                &fixture.supervisor_id,
                Some(&fixture.supervisor_id),
                None,
                None,
                None,
            )
            .unwrap();
        }
        let refusal = run_merge(&fixture).await;
        assert!(refusal.contains("CI RED"), "{refusal}");
        assert_eq!(
            git_stdout(&fixture.repo.root, &["rev-parse", "main"]),
            fixture.receipt.target_sha,
            "a refused merge must not move the target"
        );
        let (retained_receipt, parked) =
            cas_store::get_latest_worker_delivery(&fixture.cas_root, &fixture.task_id)
                .unwrap()
                .unwrap();
        assert_eq!(parked.state, WorkerDeliveryState::AwaitingMerge);
        assert_eq!(parked.last_error_code.as_deref(), Some("ci_refused"));
        assert!(
            parked
                .last_error_detail
                .as_deref()
                .unwrap()
                .contains("CI RED")
        );
        assert_eq!(
            retained_receipt, receipt,
            "retry retains the reviewed receipt"
        );
        assert_eq!(parked.verification_id, approved.verification_id);
        assert_eq!(parked.merge_commit_sha, None);
        assert_eq!(
            open_task_store(&fixture.cas_root)
                .unwrap()
                .get(&fixture.task_id)
                .unwrap()
                .status,
            TaskStatus::AwaitingMerge
        );
    }

    // A later CI refusal during reconciliation cannot erase a completed merge.
    run_git(
        &[
            "merge",
            "--no-ff",
            "-m",
            "land reviewed delivery",
            &receipt.source_branch,
        ],
        &fixture.repo.root,
    );
    let merged_tip = git_stdout(&fixture.repo.root, &["rev-parse", "main"]);
    cas_store::transition_worker_delivery(
        &fixture.cas_root,
        &approved.id,
        &[WorkerDeliveryState::AwaitingMerge],
        WorkerDeliveryState::Merged,
        &fixture.supervisor_id,
        Some(&fixture.supervisor_id),
        None,
        Some(&merged_tip),
        None,
    )
    .unwrap();
    let refusal = run_merge(&fixture).await;
    assert!(refusal.contains("CI RED"), "{refusal}");
    assert_eq!(delivery_state(&fixture), WorkerDeliveryState::Merged);
    assert_eq!(
        git_stdout(&fixture.repo.root, &["rev-parse", "main"]),
        merged_tip
    );
}

#[tokio::test]
async fn docs_only_code_ci_supervisor_override_is_logged_cas_a9bd() {
    let home = TempDir::new().expect("temp HOME");
    let fake_bin = TempDir::new().expect("fake gh dir");
    let gh = fake_bin.path().join("gh");
    cas::test_paths::warm_stub(&gh, "#!/bin/sh\nif [ \"$1 $2\" = 'repo view' ]; then printf '{\"nameWithOwner\":\"%s\"}\\n' \"$3\"; exit 0; fi\nif [ \"$1 $2\" = 'api graphql' ]; then printf '%s\\n' '{\"data\":{\"repository\":{\"mergeQueue\":null}}}'; exit 0; fi\ncat <<'JSON'\n{\"check_runs\":[{\"name\":\"Docs Lint\",\"status\":\"completed\",\"conclusion\":\"success\",\"html_url\":\"https://github.com/org/repo/actions/runs/4150\"}]}\nJSON\n");
    let mut env = TestEnvGuard::new();
    env.set("HOME", home.path());
    // cas-9790: select the fake through Cassy's `CAS_GH_BIN` seam; process
    // PATH stays untouched (factory_mcp_ops_test PATH-isolation lint).
    env.set(cas::github_issue_attach::GH_BIN_ENV, &gh);
    let fixture = arm_delivery("docscioverride", "docs-ci-override", &mut env).await;
    let supervisor_service = delivery_service(&fixture.cas_root, &fixture.supervisor_id);
    let merge_with = |supervisor_override: Option<bool>, reason: Option<&str>| {
        let mut merge = coord_req("worktree_merge");
        merge.id = Some(fixture.receipt.source_branch.clone());
        merge.task_id = Some(fixture.task_id.clone());
        merge.allow_trunk = Some(true);
        merge.cleanup = Some(false);
        merge.supervisor_override = supervisor_override;
        merge.reason = reason.map(str::to_string);
        merge
    };
    let outcome = |result: Result<rmcp::model::CallToolResult, rmcp::ErrorData>| match result {
        Ok(result) => get_text(&result),
        Err(error) => format!("MCP_ERROR: {error}"),
    };

    let refused = outcome(
        supervisor_service
            .factory(Parameters(merge_with(None, None)))
            .await,
    );
    assert!(refused.contains("CODE CI REQUIRED"), "{refused}");
    assert!(refused.contains("supervisor_override=true"), "{refused}");
    // Refusals happen after merge authorization but before Git: the target
    // is untouched and the delivery has not merged.
    let unmerged = |label: &str| {
        assert_eq!(
            git_stdout(&fixture.repo.root, &["rev-parse", "main"]),
            fixture.receipt.target_sha,
            "{label}: the target must not move"
        );
        assert_eq!(
            delivery_state(&fixture),
            WorkerDeliveryState::AwaitingMerge,
            "{label}: the refused delivery must remain retryable"
        );
    };
    unmerged("docs-only code CI without override");

    let no_reason = outcome(
        supervisor_service
            .factory(Parameters(merge_with(Some(true), None)))
            .await,
    );
    assert!(
        !no_reason.contains("Unsupported parameter(s)"),
        "{no_reason}"
    );
    assert!(no_reason.contains("non-empty reason"), "{no_reason}");
    unmerged("override without reason");

    let merged = outcome(
        supervisor_service
            .factory(Parameters(merge_with(
                Some(true),
                Some("docs-only run skipped validation; supervisor reviewed delivery"),
            )))
            .await,
    );
    assert!(!merged.contains("Unsupported parameter(s)"), "{merged}");
    assert!(!merged.starts_with("MCP_ERROR"), "{merged}");
    assert!(
        merged.contains("unconfirmed code CI was explicitly overridden by a registered supervisor"),
        "{merged}"
    );
    assert!(
        matches!(
            delivery_state(&fixture),
            WorkerDeliveryState::Merged
                | WorkerDeliveryState::CloseReady
                | WorkerDeliveryState::Delivered
        ),
        "an accepted override must merge the delivery:\n{merged}"
    );
    let notes = open_task_store(&fixture.cas_root)
        .expect("task store")
        .get(&fixture.task_id)
        .expect("task")
        .notes;
    assert!(notes.contains("overrode unconfirmed lane code CI"), "{notes}");
    assert!(notes.contains("supervisor reviewed delivery"), "{notes}");
    assert!(notes.contains(&fixture.receipt.commit_sha), "{notes}");
}

#[tokio::test]
async fn delivery_merge_refuses_target_drift_before_merge_as_recoverable_tip_changed() {
    let home = TempDir::new().expect("temp HOME");
    let mut env = TestEnvGuard::new();
    env.set("HOME", home.path());
    let fixture = arm_delivery("driftbefore", "drift-before", &mut env).await;

    // A concurrent actor commits on the reviewed target after approval.
    std::fs::write(fixture.repo.root.join("concurrent.txt"), "concurrent\n").unwrap();
    run_git(&["add", "concurrent.txt"], &fixture.repo.root);
    run_git(
        &["commit", "-m", "concurrent target commit"],
        &fixture.repo.root,
    );
    let drifted = git_stdout(&fixture.repo.root, &["rev-parse", "main"]);
    assert_ne!(drifted, fixture.receipt.target_sha);

    let _env = env.enter_dir(&fixture.repo.root);
    let output = run_merge(&fixture).await;

    let state = delivery_state(&fixture);
    assert_eq!(
        state,
        WorkerDeliveryState::TipChanged,
        "target drift must be typed TipChanged, not a generic stale failure:\n{output}"
    );
    assert!(
        state.is_recoverable_failure(),
        "TipChanged must remain a recoverable failure"
    );
    assert_no_delivery_projection(&fixture, state);
    assert_eq!(
        git_stdout(&fixture.repo.root, &["rev-parse", "main"]),
        drifted,
        "a refused delivery must not touch the target ref"
    );
}

/// The real cas-0a21 race: drift lands *between* preflight and the merge, via
/// a genuine `post-checkout` hook. Ancestry alone cannot catch this — the
/// merge sweeps the drifted commit in, so `receipt.commit_sha` IS an ancestor
/// of the new tip. Only a first-parent (topology-rooted) check rejects it.
#[tokio::test]
async fn delivery_merge_refuses_target_drift_injected_between_preflight_and_merge() {
    let home = TempDir::new().expect("temp HOME");
    let mut env = TestEnvGuard::new();
    env.set("HOME", home.path());
    let fixture = arm_delivery("driftduring", "drift-during", &mut env).await;

    let reviewed = fixture.receipt.target_sha.clone();
    assert_eq!(
        git_stdout(&fixture.repo.root, &["rev-parse", "main"]),
        reviewed
    );
    // cas-4702 / GH #68: the supervisor's shared checkout sits on its own
    // branch, never on the delivery target — that is precisely why the merge
    // must run in an ephemeral worktree. Park HEAD off `main` before arming
    // the drift so the production venue is the one under test.
    run_git(
        &["checkout", "-q", "-b", "supervisor-parking"],
        &fixture.repo.root,
    );
    fixture.repo.arm_post_checkout_drift("cas0a21drift", "main");

    let _env = env.enter_dir(&fixture.repo.root);
    let output = run_merge(&fixture).await;

    // The hook must actually have fired, or this regression proves nothing.
    let final_tip = git_stdout(&fixture.repo.root, &["rev-parse", "main"]);
    assert_ne!(
        final_tip, reviewed,
        "post-checkout drift hook never fired; the race was not exercised:\n{output}"
    );

    let state = delivery_state(&fixture);
    assert_eq!(
        state,
        WorkerDeliveryState::TipChanged,
        "drift between preflight and merge must fail typed TipChanged:\n{output}"
    );
    assert!(state.is_recoverable_failure());
    assert_no_delivery_projection(&fixture, state);

    // The target must be left exactly at the concurrent actor's commit: our
    // merge is rolled back, their work is preserved.
    assert_eq!(
        git_stdout(&fixture.repo.root, &["rev-parse", "main"]),
        git_stdout(&fixture.repo.root, &["rev-parse", "main^{commit}"]),
        "target tip must resolve to a commit"
    );
    let parents = git_stdout(
        &fixture.repo.root,
        &["rev-list", "--parents", "-n", "1", "main"],
    );
    assert_eq!(
        parents.split_whitespace().count(),
        2,
        "the false merge commit must be rolled off the target, leaving the \
         concurrent single-parent commit: {parents}"
    );
    assert!(
        !cas_git_is_ancestor(&fixture.repo.root, &fixture.receipt.commit_sha, "main"),
        "a refused delivery must not leave the receipt commit merged into the target"
    );
    assert_eq!(
        git_stdout(&fixture.repo.root, &["rev-parse", "main~1"]),
        reviewed,
        "the concurrent commit must remain rooted at the reviewed target"
    );
}

/// Serialization must be keyed on repository + target ref, so unrelated
/// deliveries never contend or leak state across repos.
#[tokio::test]
async fn concurrent_deliveries_in_independent_repositories_remain_independent() {
    let home = TempDir::new().expect("temp HOME");
    let mut env = TestEnvGuard::new();
    env.set("HOME", home.path());

    let first = arm_delivery("indepone", "independent-one", &mut env).await;
    let second = arm_delivery("indeptwo", "independent-two", &mut env).await;

    // Drift only the FIRST repository's target.
    std::fs::write(first.repo.root.join("concurrent.txt"), "concurrent\n").unwrap();
    run_git(&["add", "concurrent.txt"], &first.repo.root);
    run_git(
        &["commit", "-m", "concurrent target commit"],
        &first.repo.root,
    );

    let mut env = env.enter_dir(&first.repo.root);
    let first_output = run_merge(&first).await;
    let _env = env.enter_dir(&second.repo.root);
    let second_output = run_merge(&second).await;

    assert_eq!(
        delivery_state(&first),
        WorkerDeliveryState::TipChanged,
        "drifted repository must fail:\n{first_output}"
    );
    // The undrifted repository is completely unaffected by the neighbour's
    // failure: it merges and its topology is rooted at its reviewed target.
    let second_state = delivery_state(&second);
    assert!(
        matches!(
            second_state,
            WorkerDeliveryState::Merged
                | WorkerDeliveryState::CloseReady
                | WorkerDeliveryState::Delivered
        ),
        "an independent repository must not be blocked or failed by another \
         repository's target drift, got {second_state}:\n{second_output}"
    );
    assert_eq!(
        git_stdout(&second.repo.root, &["rev-parse", "main~1"]),
        second.receipt.target_sha,
        "the successful merge must be rooted at its own reviewed target"
    );
}

fn cas_git_is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> bool {
    Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(repo)
        .status()
        .expect("git merge-base")
        .success()
}

/// Drive real admission, persistence and daemon retry with a Git/SQLite fixture.
#[tokio::test]
async fn pending_ci_merge_retry_is_bound_to_parked_sha() {
    for outcome in ["green", "red", "moved", "no-checks", "expired", "green-held", "cancelled", "gh-failure", "green-bad-neighbor", "missing-supervisor", "unknown", "unvalidated", "missing-commit"] {
        let home = TempDir::new().unwrap();
        let fake_bin = TempDir::new().unwrap();
        let gh = fake_bin.path().join("gh");
        let response = fake_bin.path().join("ci.json");
        std::fs::write(&response, r#"{"check_runs":[{"name":"Fast Validation","status":"in_progress","conclusion":null}]}"#).unwrap();
        cas::test_paths::warm_stub(&gh, r#"#!/bin/sh
if [ "$1 $2" = 'repo view' ]; then printf '{"nameWithOwner":"%s"}\n' "$3"; exit 0; fi
if [ "$1 $2" = 'api graphql' ]; then printf '%s\n' '{"data":{"repository":{"mergeQueue":null}}}'; exit 0; fi
if [ "$(cat "$CAS_CI_RETRY_TEST_RESPONSE")" = gh-failure ]; then exit 1; fi
if [ "$(cat "$CAS_CI_RETRY_TEST_RESPONSE")" = missing-commit ]; then echo 'HTTP 422: no commit found' >&2; exit 1; fi
cat "$CAS_CI_RETRY_TEST_RESPONSE"
"#);
        let mut env = TestEnvGuard::new();
        env.set("HOME", home.path());
        env.set(cas::github_issue_attach::GH_BIN_ENV, &gh);
        env.set("CAS_CI_RETRY_TEST_RESPONSE", &response);
        let slug = format!("pendingretry{outcome}");
        let fixture = arm_delivery_with_selector(&slug, &slug, &mut env,
            &format!("project:github.com/org/{slug}")).await;
        run_git(&["push", "origin", "--all"], &fixture.repo.root);
        let refusal = run_merge(&fixture).await;
        assert!(refusal.contains("CI PENDING"), "{outcome}: {refusal}");
        assert_eq!(delivery_state(&fixture), WorkerDeliveryState::AwaitingMerge);
        let request = fixture.cas_root.join("merge-ci-retries").join(format!("{}.json", fixture.task_id));
        assert!(request.exists(), "pending CI refusal must survive a daemon restart: {refusal}");
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&request).unwrap()).unwrap();
        assert_eq!(saved["sha"],fixture.receipt.commit_sha);
        assert_eq!(saved["ci_sha"],fixture.receipt.commit_sha);
        let session = format!("{slug}-factory");
        // A different factory cannot consume the intent; genuinely pending
        // CI retains it without a supervisor notice.
        cas::factory_ci_retry::retry_pending(&fixture.cas_root, "other-factory").await.unwrap();
        cas::factory_ci_retry::retry_pending(&fixture.cas_root, &session).await.unwrap();
        assert!(request.exists());
        assert_eq!(git_stdout(&fixture.repo.root, &["rev-parse", "main"]), fixture.receipt.target_sha);
        if outcome == "expired" {
            let mut old = saved.clone();
            old["armed_at"] = serde_json::json!("2000-01-01T00:00:00Z");
            std::fs::write(&request, serde_json::to_vec(&old).unwrap()).unwrap();
        }
        if outcome == "missing-supervisor" {
            let mut orphan = saved.clone();
            orphan["supervisor"] = serde_json::json!("deleted-supervisor");
            std::fs::write(&request, serde_json::to_vec(&orphan).unwrap()).unwrap();
        }
        if outcome == "green-bad-neighbor" {
            std::fs::write(request.with_file_name("broken.json"), "not json").unwrap();
        }
        let held_dispatch = if outcome == "green-held" {
            Some(cas_store::create_verification_dispatch_bound(
                &fixture.cas_root, &fixture.task_id, &fixture.supervisor_id,
                &fixture.supervisor_id, &cas_types::VerificationProofBoundary::task(),
                chrono::Utc::now() + chrono::Duration::minutes(10), false,
            ).unwrap())
        } else { None };
        std::fs::write(&response, format!(r#"{{"check_runs":[{{"name":"Fast Validation","status":"completed","conclusion":"{}"}}]}}"#,
            if outcome == "red" { "failure" } else { "success" })).unwrap();
        if outcome == "no-checks" { std::fs::write(&response, r#"{"check_runs":[]}"#).unwrap(); }
        if outcome == "unknown" { std::fs::write(&response, "{}").unwrap(); }
        if outcome == "unvalidated" {
            std::fs::write(&response, r#"{"check_runs":[{"name":"Docs Lint","status":"completed","conclusion":"success"}]}"#).unwrap();
        }
        if outcome == "missing-commit" { std::fs::write(&response, "missing-commit").unwrap(); }
        if outcome == "gh-failure" { std::fs::write(&response, "gh-failure").unwrap(); }
        if outcome == "cancelled" {
            std::fs::write(&response, r#"{"check_runs":[{"name":"Fast Validation","status":"completed","conclusion":"cancelled"}]}"#).unwrap();
        }
        if outcome == "expired" {
            std::fs::write(&response, r#"{"check_runs":[{"name":"Fast Validation","status":"in_progress","conclusion":null}]}"#).unwrap();
        }
        if outcome == "moved" {
            let worker = fixture.cas_root.join("worktrees").join(&slug);
            std::fs::write(worker.join("new-work.txt"), "unreviewed work").unwrap();
            run_git(&["add", "new-work.txt"], &worker);
            run_git(&["commit", "-m", "new worker tip"], &worker);
            run_git(&["push", "origin", &fixture.receipt.source_branch], &worker);
        }
        cas::factory_ci_retry::retry_pending(&fixture.cas_root, &session).await.unwrap();
        cas::factory_ci_retry::retry_pending(&fixture.cas_root, &session).await.unwrap();
        assert!(!request.exists(), "{outcome}: settled intent is consumed once");
        // pin: retry notices share the CI watcher's durable lifecycle wake namespace.
        let notices: Vec<_> = cas::store::open_prompt_queue_store(&fixture.cas_root).unwrap()
            .peek_all(100).unwrap().into_iter().filter(|p| p.source.starts_with("lifecycle-wake:ci-red-run:") || p.source.starts_with("lifecycle-wake:merge-ci-retry:")).collect();
        if outcome.starts_with("green") && held_dispatch.is_none() {
            assert!(cas_git_is_ancestor(&fixture.repo.root, &fixture.receipt.commit_sha, "main"),
                "{outcome}: {notices:?}");
            assert!(notices.is_empty(), "green retries must not wake a model: {notices:?}");
            // cas-5d8d8: landing is delivered quietly to its owning supervisor,
            // including a replay after merge committed but before intent cleanup.
            let landed = git_stdout(&fixture.repo.root, &["rev-parse", "main"]);
            let queue = cas::store::open_prompt_queue_store(&fixture.cas_root).unwrap();
            for replay in [false, true] {
                if replay {
                    std::fs::write(&request, serde_json::to_vec(&saved).unwrap()).unwrap();
                    cas::factory_ci_retry::retry_pending(&fixture.cas_root, &session).await.unwrap();
                    assert!(!request.exists());
                }
                let receipts: Vec<_> = queue.peek_all(100).unwrap().into_iter()
                    .filter(|row| row.source.starts_with("merge-ci-retry-landed:")).collect();
                assert_eq!(receipts.len(), 1, "{outcome}, replay={replay}: {receipts:?}");
                let receipt = &receipts[0];
                assert_eq!(receipt.target, format!("{slug}-supervisor"));
                assert_eq!(receipt.factory_session.as_deref(), Some(session.as_str()));
                assert!(receipt.prompt.starts_with(cas_store::QUIET_MESSAGE_MARKER));
                for value in [&fixture.task_id, &landed, &fixture.receipt.target_branch] {
                    assert!(receipt.prompt.contains(value), "missing receipt value {value}: {receipt:?}");
                }
            }
        } else {
            assert_eq!(git_stdout(&fixture.repo.root, &["rev-parse", "main"]), fixture.receipt.target_sha);
            assert_eq!(notices.len(), 1, "{outcome}: stopped lanes each send one notice");
            assert_eq!(notices[0].factory_session.as_deref(), Some(session.as_str()));
            if let Some(dispatch) = held_dispatch {
                // cas-c744: a new pending review holds publication, rather
                // than allowing the old post-merge close-held scenario.
                // pin: this stop notice identifies review admission, not a red CI result.
                assert!(notices[0].prompt.contains("PRE-MERGE VERIFICATION"), "{notices:?}");
                assert_eq!(cas_store::get_latest_verification_dispatch(&fixture.cas_root, &fixture.task_id).unwrap().unwrap().id,
                    dispatch.id, "automatic retry cannot replace the pending review");
                let task = open_task_store(&fixture.cas_root).unwrap().get(&fixture.task_id).unwrap();
                assert_eq!(task.status, TaskStatus::AwaitingMerge);
                assert!(task.deliverables.green_merge_commit.is_none());
            }
        }
    }
}
