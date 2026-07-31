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
//! These regressions drive the real production path (`coordination
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

// =============================================================================
// Fixtures (deliberately self-contained: cas-0a21 must not couple to the
// shared worktree_surface_test helpers while cas-59c0 is editing that file)
// =============================================================================

/// Serializes every test in this process that mutates cwd / HOME.
fn merge_cwd_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

struct CwdGuard {
    original: PathBuf,
}

impl CwdGuard {
    fn enter(dir: &Path) -> Self {
        let original = std::env::current_dir().expect("current_dir");
        std::env::set_current_dir(dir).expect("set_current_dir");
        Self { original }
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
    }
}

struct HomeGuard {
    original: Option<std::ffi::OsString>,
}

impl HomeGuard {
    fn enter(path: &Path) -> Self {
        let original = std::env::var_os("HOME");
        // SAFETY: every test here holds merge_cwd_lock for the full mutation
        // lifetime, so no other thread observes the mutated environment.
        unsafe { std::env::set_var("HOME", path) };
        Self { original }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.original {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

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

    /// Install a `post-checkout` hook that lands exactly one commit on the
    /// target branch. `merge_preserving_worktree` runs `checkout(parent)`
    /// immediately before `merge_branch`, so this fires inside the
    /// preflight -> merge window on the real production path.
    fn arm_post_checkout_drift(&self, marker: &str) {
        let hooks = self.root.join(".git").join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("post-checkout");
        // The marker lives under .git/ so firing the hook never dirties the
        // work tree that the merge is about to operate on.
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\n\
                 set -e\n\
                 marker=\"$(git rev-parse --git-common-dir)/{marker}\"\n\
                 [ -e \"$marker\" ] && exit 0\n\
                 : > \"$marker\"\n\
                 root=\"$(git rev-parse --show-toplevel)\"\n\
                 printf 'concurrent\\n' > \"$root/concurrent-{marker}.txt\"\n\
                 git add \"$root/concurrent-{marker}.txt\"\n\
                 git -c user.email=drift@test.com -c user.name=Drift \\\n\
                     commit --no-verify -q -m 'concurrent target commit'\n\
                 exit 0\n"
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
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

async fn arm_delivery(slug: &str, repo_host: &str) -> DeliveryFixture {
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
    let cas_root = init_cas_dir(&repo.root).expect("init CAS");
    std::fs::write(
        cas_root.join("config.toml"),
        "[worktrees]\nenabled = false\n",
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
        repo_selector: format!("remote:github.com/org/{repo_host}"),
        target_branch: "main".to_string(),
    });
    task_store.add(&task).expect("add task");

    let receipt = WorkerCompletionReceiptInput {
        task_id: task.id.clone(),
        worker_agent_id: worker_id.clone(),
        repo_selector: format!("remote:github.com/org/{repo_host}"),
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
    };

    // Worker submits the immutable receipt, supervisor approves it.
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
    assert!(
        get_text(&close).contains("Worker delivery receipt accepted idempotently"),
        "{}",
        get_text(&close)
    );

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
    match supervisor_service.coordination(Parameters(merge)).await {
        Ok(result) => get_text(&result),
        Err(error) => format!("MCP_ERROR: {error}"),
    }
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

// =============================================================================
// Regressions
// =============================================================================

/// Drift that lands *before* the merge is entered must be refused as the
/// typed, recoverable `TipChanged` — target drift is a tip change, and the
/// supervisor needs `is_recoverable_failure()` to offer recovery.
#[tokio::test]
async fn delivery_merge_refuses_target_drift_before_merge_as_recoverable_tip_changed() {
    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let home = TempDir::new().expect("temp HOME");
    let _home = HomeGuard::enter(home.path());
    let fixture = arm_delivery("driftbefore", "drift-before").await;

    // A concurrent actor commits on the reviewed target after approval.
    std::fs::write(fixture.repo.root.join("concurrent.txt"), "concurrent\n").unwrap();
    run_git(&["add", "concurrent.txt"], &fixture.repo.root);
    run_git(&["commit", "-m", "concurrent target commit"], &fixture.repo.root);
    let drifted = git_stdout(&fixture.repo.root, &["rev-parse", "main"]);
    assert_ne!(drifted, fixture.receipt.target_sha);

    let _cwd = CwdGuard::enter(&fixture.repo.root);
    let output = run_merge(&fixture).await;
    drop(_cwd);

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
    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let home = TempDir::new().expect("temp HOME");
    let _home = HomeGuard::enter(home.path());
    let fixture = arm_delivery("driftduring", "drift-during").await;

    let reviewed = fixture.receipt.target_sha.clone();
    assert_eq!(git_stdout(&fixture.repo.root, &["rev-parse", "main"]), reviewed);
    fixture.repo.arm_post_checkout_drift("cas0a21drift");

    let _cwd = CwdGuard::enter(&fixture.repo.root);
    let output = run_merge(&fixture).await;
    drop(_cwd);

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
    let parents = git_stdout(&fixture.repo.root, &["rev-list", "--parents", "-n", "1", "main"]);
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
    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let home = TempDir::new().expect("temp HOME");
    let _home = HomeGuard::enter(home.path());

    let first = arm_delivery("indepone", "independent-one").await;
    let second = arm_delivery("indeptwo", "independent-two").await;

    // Drift only the FIRST repository's target.
    std::fs::write(first.repo.root.join("concurrent.txt"), "concurrent\n").unwrap();
    run_git(&["add", "concurrent.txt"], &first.repo.root);
    run_git(&["commit", "-m", "concurrent target commit"], &first.repo.root);

    let cwd = CwdGuard::enter(&first.repo.root);
    let first_output = run_merge(&first).await;
    drop(cwd);
    let cwd = CwdGuard::enter(&second.repo.root);
    let second_output = run_merge(&second).await;
    drop(cwd);

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
