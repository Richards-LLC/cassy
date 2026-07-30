//! Tests for A3: Truthful worktree status surface (cas-af86)
//!
//! Verifies that `worktree_list` and `worktree_status` accurately report factory
//! (System B) worktrees even when the CAS experimental worktree system (System A)
//! is disabled via config (`worktrees.enabled = false`).
//!
//! Prior to the fix, the gate in `mcp/tools/service/mod.rs` short-circuited
//! `worktree_list` with a misleading "experimental and disabled by default"
//! message whenever System A was off — even though factory workers were running
//! in real git worktrees under `.cas/worktrees/<name>`.

use std::path::{Path, PathBuf};
use std::process::Command;

use cas::mcp::{CasCore, CasService};
use cas::mcp::tools::{TaskCloseRequest, TaskUpdateRequest};
use cas::store::{init_cas_dir, open_agent_store, open_task_store};
use cas::types::{Agent, AgentType, Task, TaskType, WorkTarget};
use cas_mcp::types::CoordinationRequest;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::RawContent;
use tempfile::TempDir;

// =============================================================================
// Test fixtures
// =============================================================================

struct GitRepo {
    _temp: TempDir,
    pub root: PathBuf,
}

impl GitRepo {
    fn new() -> Self {
        let temp = TempDir::new().expect("TempDir");
        let root = temp.path().to_path_buf();

        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };

        run(&["init", "-b", "main"]);
        run(&["config", "user.email", "test@test.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(root.join("README.md"), "test").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "init"]);

        Self { _temp: temp, root }
    }

    /// Create a git worktree at `path` on a new branch `branch`.
    /// The parent directory of `path` is created if needed; git creates `path` itself.
    fn add_worktree(&self, path: &Path, branch: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let out = Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                branch,
                path.to_str().unwrap(),
            ])
            .current_dir(&self.root)
            .output()
            .expect("git worktree add");
        assert!(
            out.status.success(),
            "git worktree add failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn make_service(cas_root: PathBuf) -> CasService {
    let core = CasCore::with_daemon(cas_root, None, None);
    CasService::new(core, None)
}

/// Overwrite the config with `[worktrees] enabled = false` to simulate
/// a deployment where System A (experimental) is explicitly off.
fn disable_system_a(cas_root: &Path) {
    std::fs::write(
        cas_root.join("config.toml"),
        "[worktrees]\nenabled = false\n",
    )
    .unwrap();
}

/// Build a minimal CoordinationRequest with only `action` set.
fn coord_req(action: &str) -> CoordinationRequest {
    CoordinationRequest {
        action: action.to_string(),
        id: None,
        task_id: None,
        target: None,
        message: None,
        summary: None,
        urgent: None,
        force: None,
        allow_trunk: None,
        cleanup: None,
        clear: None,
        limit: None,
        name: None,
        agent_type: None,
        parent_id: None,
        session_id: None,
        prompt: None,
        max_iterations: None,
        completion_promise: None,
        reason: None,
        stale_threshold_secs: None,
        supervisor_id: None,
        event_type: None,
        payload: None,
        priority: None,
        notification_id: None,
        count: None,
        worker_names: None,
        branch: None,
        older_than_secs: None,
        isolate: None,
        cli: None,
        model: None,
        effort: None,
        remind_message: None,
        remind_delay_secs: None,
        remind_event: None,
        remind_filter: None,
        remind_id: None,
        remind_ttl_secs: None,
        all: None,
        status: None,
        orphans: None,
        dry_run: None,
    }
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

fn close_update_request(id: String) -> TaskUpdateRequest {
    TaskUpdateRequest {
        id,
        title: None,
        notes: None,
        priority: None,
        labels: None,
        description: None,
        design: None,
        acceptance_criteria: None,
        demo_statement: None,
        execution_note: None,
        external_ref: None,
        assignee: None,
        status: Some("closed".to_string()),
        epic: None,
        epic_verification_owner: None,
        depth: None,
    }
}

// =============================================================================
// Tests
// =============================================================================

/// AC1 + AC2: `worktree_list` returns the factory (System B) worktrees and labels
/// them `[factory]`, rather than returning the "experimental and disabled" gate
/// message, when System A is off but a real factory worktree is present.
#[tokio::test]
async fn test_worktree_list_shows_factory_worktrees_when_system_a_disabled() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");

    // System A explicitly off
    disable_system_a(&cas_root);

    // Create a factory (System B) worktree at the standard location
    let wt_path = cas_root.join("worktrees").join("alice");
    repo.add_worktree(&wt_path, "factory/alice");

    let svc = make_service(cas_root);
    let result = svc
        .coordination(Parameters(coord_req("worktree_list")))
        .await
        .expect("coordination call should succeed");

    let text = get_text(&result);

    // Must NOT show the misleading disabled-gate message
    assert!(
        !text.contains("experimental and disabled"),
        "worktree_list must not return the 'disabled' gate message when factory worktrees \
         exist (System A off, System B active).\nGot:\n{text}"
    );

    // Must include the factory worktree's branch name
    assert!(
        text.contains("factory/alice"),
        "worktree_list must list the factory/alice branch.\nGot:\n{text}"
    );

    // AC2: output must distinguish factory (System B) worktrees
    assert!(
        text.contains("[factory]") || text.to_lowercase().contains("factory"),
        "worktree_list output must label the worktree as factory (System B).\nGot:\n{text}"
    );
}

/// AC4 (regression): when NO worktrees exist and System A is off, `worktree_list`
/// returns an informational "No worktrees" message — not the misleading
/// "experimental and disabled" gate message.
#[tokio::test]
async fn test_worktree_list_no_disabled_message_when_no_factory_worktrees() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");

    // System A off, no factory worktrees at all
    disable_system_a(&cas_root);

    let svc = make_service(cas_root);
    let result = svc
        .coordination(Parameters(coord_req("worktree_list")))
        .await
        .expect("coordination call should succeed");

    let text = get_text(&result);

    // Gate must not block with the 'disabled' message
    assert!(
        !text.contains("experimental and disabled"),
        "worktree_list must not show the misleading 'disabled' gate message.\nGot:\n{text}"
    );

    // Should return the natural empty-list response
    assert!(
        text.contains("No worktrees"),
        "worktree_list should say 'No worktrees' when none exist.\nGot:\n{text}"
    );
}

// =============================================================================
// cas-d1a0: project-scoped git reconcile — sibling-session worktrees must
// appear in worktree_list even with no WorktreeStore row (System B never
// registers; epic worktrees often live outside .cas/worktrees).
// =============================================================================

/// Factory worktree under a *customized* `worktrees.base_path` (not the
/// hardcoded `<cas_root>/worktrees` layout) must still appear in
/// `worktree_list` — same path resolution spawn / worktree_merge use.
#[tokio::test]
async fn test_worktree_list_honors_configured_base_path() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    // Unique name: relative base_path resolves under the project parent
    // (often /tmp), so a fixed name collides across tests/processes.
    let base_name = format!(
        "cas-d1a0-list-base-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    std::fs::write(
        cas_root.join("config.toml"),
        format!("[worktrees]\nenabled = false\nbase_path = \"{base_name}\"\n"),
    )
    .unwrap();

    // Mirrors WorktreeManager::worktree_root for relative non-{project} base_path:
    // repo_root.parent().join(base_path)/<worker>
    let base_root = repo.root.parent().unwrap().join(&base_name);
    let wt_path = base_root.join("erin");
    repo.add_worktree(&wt_path, "factory/erin");
    assert_ne!(wt_path, cas_root.join("worktrees").join("erin"));

    let svc = make_service(cas_root);
    let result = svc
        .coordination(Parameters(coord_req("worktree_list")))
        .await
        .expect("coordination call should succeed");
    let text = get_text(&result);

    // Reclaim external worktree before asserts so a failure still cleans up.
    let _ = Command::new("git")
        .args(["worktree", "remove", "--force", wt_path.to_str().unwrap()])
        .current_dir(&repo.root)
        .output();
    let _ = std::fs::remove_dir_all(&base_root);

    assert!(
        text.contains("factory/erin"),
        "worktree_list must surface factory worktrees under configured base_path.\nGot:\n{text}"
    );
    assert!(
        text.contains("[factory]"),
        "custom-base factory worktree must be labeled [factory].\nGot:\n{text}"
    );
}
/// Epic worktree outside `.cas/worktrees` (e.g. director `/tmp/…-epic-…`)
/// with an `epic/*` branch must appear as untracked so a sibling session
/// can see it for merge/cleanup (BUG report cas-d1a0).
#[tokio::test]
async fn test_worktree_list_surfaces_unregistered_epic_worktree_outside_cas_dir() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    // No SQLite WorktreeStore row — simulates a worktree created by a
    // sibling/predecessor session that never registered System A.
    let tmp = TempDir::new().expect("TempDir for epic worktree");
    let epic_path = tmp.path().join("ozer-epic-ea3e-hv");
    repo.add_worktree(&epic_path, "epic/integrate-cas-ea3e");

    let svc = make_service(cas_root);
    let result = svc
        .coordination(Parameters(coord_req("worktree_list")))
        .await
        .expect("coordination call should succeed");
    let text = get_text(&result);

    assert!(
        text.contains("epic/integrate-cas-ea3e"),
        "unregistered epic/* worktree outside .cas/worktrees must appear in list.\nGot:\n{text}"
    );
    assert!(
        text.contains("[untracked]"),
        "CAS-pattern worktree with no store row must be labeled [untracked].\nGot:\n{text}"
    );
    // Keep the temp dir alive until after the list call (git worktree still present).
    drop(tmp);
}

/// Non-CAS user worktrees (arbitrary branch outside CAS layouts) must NOT
/// pollute worktree_list — only CAS-pattern paths/branches are reconciled.
#[tokio::test]
async fn test_worktree_list_ignores_unrelated_git_worktrees() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    let tmp = TempDir::new().expect("TempDir for unrelated worktree");
    let other_path = tmp.path().join("hand-made-wt");
    repo.add_worktree(&other_path, "feature/hand-made");

    let svc = make_service(cas_root);
    let result = svc
        .coordination(Parameters(coord_req("worktree_list")))
        .await
        .expect("coordination call should succeed");
    let text = get_text(&result);

    assert!(
        !text.contains("feature/hand-made"),
        "unrelated user worktrees must not appear in worktree_list.\nGot:\n{text}"
    );
    assert!(
        text.contains("No worktrees"),
        "only non-CAS worktrees present → empty list message.\nGot:\n{text}"
    );
    drop(tmp);
}

/// Sibling-session factory worker under the default `.cas/worktrees/<name>`
/// path with no store row must still list (git reconcile is the project-
/// scoped source of truth for System B).
#[tokio::test]
async fn test_worktree_list_shows_sibling_session_factory_worktree_without_store_row() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    // Session A created this; session B only has the git worktree + shared .cas.
    let wt_path = cas_root.join("worktrees").join("hv-food-qa");
    repo.add_worktree(&wt_path, "factory/hv-food-qa");

    let svc = make_service(cas_root);
    let result = svc
        .coordination(Parameters(coord_req("worktree_list")))
        .await
        .expect("coordination call should succeed");
    let text = get_text(&result);

    assert!(
        text.contains("factory/hv-food-qa"),
        "director-spawned factory worktree must be visible to another session's worktree_list.\nGot:\n{text}"
    );
    assert!(
        text.contains("[factory]"),
        "expected [factory] label for System B reconcile entry.\nGot:\n{text}"
    );
}

// =============================================================================
// cas-1d11: worktree_merge must agree with spawn isolate=true on
// worktrees.enabled — a factory (System B) worktree must be mergeable
// even when System A is off, since spawn never checked that flag either.
// =============================================================================

/// `worktree_merge`'s handler resolves the repo root from the *process*
/// current directory (`std::env::current_dir()`), not from `cas_root` —
/// a pre-existing quirk shared by `worktree_create` too, unrelated to this
/// fix. Since cwd is process-global, tests that exercise `worktree_merge`
/// must serialize around changing it. All such tests live in this file and
/// take this lock for their full duration; no other test file is affected
/// (`cargo test` runs each integration-test file as its own process).
fn merge_cwd_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// RAII guard: switches the process cwd to `dir` on construction, restores
/// the original cwd on drop (including on panic/early return).
struct CwdGuard {
    original: PathBuf,
}

struct HomeGuard {
    original: Option<std::ffi::OsString>,
}

struct VarGuard {
    key: &'static str,
    original: Option<std::ffi::OsString>,
}

impl VarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let original = std::env::var_os(key);
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for VarGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.original {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

impl HomeGuard {
    fn enter(path: &Path) -> Self {
        let original = std::env::var_os("HOME");
        // SAFETY: every test in this integration-test process that mutates
        // process state holds merge_cwd_lock for the full mutation lifetime.
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

/// AC: spawn isolate=true creates a real factory worktree regardless of
/// `worktrees.enabled`; `worktree_merge` must actually merge it instead of
/// refusing with the "disabled by default" message.
#[tokio::test]
async fn test_worktree_merge_succeeds_for_factory_worktree_when_system_a_disabled() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    let wt_path = cas_root.join("worktrees").join("alice");
    repo.add_worktree(&wt_path, "factory/alice");

    // Give the worker branch real content to merge, not just an empty commit.
    std::fs::write(wt_path.join("alice-work.txt"), "alice's work").unwrap();
    run_git(&["add", "."], &wt_path);
    run_git(&["commit", "-m", "alice work"], &wt_path);

    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);

    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/alice".to_string());
    // No epic context — allow_trunk authorizes trunk; force stays false so dirty protection remains.
    req.allow_trunk = Some(true);
    req.cleanup = Some(true);
    let result = svc
        .coordination(Parameters(req))
        .await
        .expect("coordination call should succeed");

    let text = get_text(&result);

    assert!(
        !text.contains("experimental and disabled"),
        "worktree_merge must not refuse a real factory (System B) worktree just \
         because System A's flag is off — spawn never checked that flag either.\nGot:\n{text}"
    );
    assert!(
        text.contains("Merged worktree"),
        "worktree_merge should report a successful merge.\nGot:\n{text}"
    );

    // The merge actually landed: content reachable from the checked-out repo.
    assert!(
        repo.root.join("alice-work.txt").exists(),
        "merged content must land on the parent branch's working tree"
    );
    // The request opted into cleanup, so the worktree directory is reclaimed.
    assert!(
        !wt_path.exists(),
        "worktree directory should be cleaned up after a successful merge"
    );
}

#[tokio::test]
async fn task_bound_cross_repo_merge_mutates_only_declared_repo() {
    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let home = TempDir::new().expect("temp HOME");
    let _home = HomeGuard::enter(home.path());

    let repo_a = GitRepo::new();
    let repo_b = GitRepo::new();
    run_git(
        &["remote", "add", "origin", "git@github.com:org/spawn-a.git"],
        &repo_a.root,
    );
    run_git(
        &["remote", "add", "origin", "git@github.com:org/work-b.git"],
        &repo_b.root,
    );
    let cas_root_a = init_cas_dir(&repo_a.root).expect("init repo A CAS");
    let cas_root_b = init_cas_dir(&repo_b.root).expect("init repo B CAS");
    disable_system_a(&cas_root_a);

    let task_store = open_task_store(&cas_root_a).expect("open repo A task store");
    let mut task = Task::new("cross-repo-task".to_string(), "Cross repo merge".to_string());
    task.assignee = Some("alice".to_string());
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/org/work-b".to_string(),
        target_branch: "main".to_string(),
    });
    task_store.add(&task).expect("add task binding");

    let wt_path = cas_root_b.join("worktrees").join("alice");
    repo_b.add_worktree(&wt_path, "factory/alice");
    std::fs::write(wt_path.join("repo-b-work.txt"), "repo B only").unwrap();
    run_git(&["add", "repo-b-work.txt"], &wt_path);
    run_git(&["commit", "-m", "repo B work"], &wt_path);

    let a_head_before = git_stdout(&repo_a.root, &["rev-parse", "HEAD"]);
    let a_index_before = git_stdout(&repo_a.root, &["write-tree"]);
    let a_status_before = git_stdout(&repo_a.root, &["status", "--porcelain=v1"]);
    let _cwd = CwdGuard::enter(&repo_a.root);

    let svc = make_service(cas_root_a);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/alice".to_string());
    req.task_id = Some(task.id);
    req.allow_trunk = Some(true);
    req.cleanup = Some(false);
    let result = svc
        .coordination(Parameters(req))
        .await
        .expect("task-bound cross-repo merge call");
    let text = get_text(&result);
    assert!(
        text.contains("Merged worktree") && text.contains("task WorkTarget"),
        "public merge path must use the declared WorkTarget.\nGot:\n{text}"
    );

    assert!(
        repo_b.root.join("repo-b-work.txt").exists(),
        "repo B main checkout must receive the worker change"
    );
    assert_eq!(
        git_stdout(&repo_b.root, &["branch", "--show-current"]),
        "main"
    );
    assert!(
        !repo_a.root.join("repo-b-work.txt").exists(),
        "repo A worktree must not receive repo B content"
    );
    assert_eq!(
        git_stdout(&repo_a.root, &["rev-parse", "HEAD"]),
        a_head_before,
        "repo A HEAD must remain unchanged"
    );
    assert_eq!(
        git_stdout(&repo_a.root, &["write-tree"]),
        a_index_before,
        "repo A index must remain unchanged"
    );
    assert_eq!(
        git_stdout(&repo_a.root, &["status", "--porcelain=v1"]),
        a_status_before,
        "repo A working tree must remain unchanged"
    );
}

#[tokio::test]
async fn update_to_closed_runs_hook_in_declared_task_worktree_and_records_path_free_evidence() {
    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let home = TempDir::new().expect("temp HOME");
    let _home = HomeGuard::enter(home.path());
    let repo_a = GitRepo::new();
    let repo_b = GitRepo::new();
    run_git(
        &["remote", "add", "origin", "git@github.com:org/spawn-a.git"],
        &repo_a.root,
    );
    run_git(
        &["remote", "add", "origin", "git@github.com:org/work-b.git"],
        &repo_b.root,
    );
    let cas_root_a = init_cas_dir(&repo_a.root).expect("init repo A CAS");
    let cas_root_b = init_cas_dir(&repo_b.root).expect("init repo B CAS");

    let own_path = cas_root_b.join("worktrees").join("frontend");
    repo_b.add_worktree(&own_path, "factory/frontend");
    std::fs::write(own_path.join("frontend.rs"), "pub fn done() {}\n").unwrap();
    run_git(&["add", "frontend.rs"], &own_path);
    run_git(&["commit", "-m", "frontend task"], &own_path);
    let own_tip = git_stdout(&own_path, &["rev-parse", "HEAD"]);

    let unrelated_path = cas_root_b.join("worktrees").join("worker-pulse");
    repo_b.add_worktree(&unrelated_path, "factory/worker-pulse");
    std::fs::write(
        unrelated_path.join("backend.rs"),
        "pub fn transient() { todo!(\"mid-edit\") }\n",
    )
    .unwrap();
    run_git(&["add", "backend.rs"], &unrelated_path);
    run_git(&["commit", "-m", "newer unrelated backend work"], &unrelated_path);
    std::fs::write(unrelated_path.join("dirty.tmp"), "newest dirty worktree").unwrap();

    let task_store = open_task_store(&cas_root_a).expect("task store");
    let mut task = Task::new("close-via-update".to_string(), "Frontend task".to_string());
    task.assignee = Some("frontend".to_string());
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/org/work-b".to_string(),
        target_branch: "main".to_string(),
    });
    task_store.add(&task).expect("add task");

    let core = CasCore::with_daemon(cas_root_a, None, None);
    core.cas_task_update(Parameters(close_update_request(task.id.clone())))
    .await
    .expect("update-to-closed must use frontend context");

    let persisted = task_store.get(&task.id).expect("persisted task");
    assert_eq!(persisted.status, cas::types::TaskStatus::Closed);
    let evidence = persisted
        .deliverables
        .pre_close_hook
        .as_ref()
        .expect("portable hook evidence");
    assert_eq!(evidence.repo_selector, "remote:github.com/org/work-b");
    assert_eq!(evidence.worktree_branch.as_deref(), Some("factory/frontend"));
    assert_eq!(evidence.task_tip.as_deref(), Some(own_tip.as_str()));
    let json = serde_json::to_string(&persisted.deliverables).unwrap();
    assert!(!json.contains(home.path().to_string_lossy().as_ref()));
    assert!(!json.contains(unrelated_path.to_string_lossy().as_ref()));
}

#[tokio::test]
async fn update_to_closed_fails_closed_when_declared_task_worktree_branch_is_ambiguous() {
    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let home = TempDir::new().expect("temp HOME");
    let _home = HomeGuard::enter(home.path());
    let repo_a = GitRepo::new();
    let repo_b = GitRepo::new();
    run_git(
        &["remote", "add", "origin", "git@github.com:org/spawn-a.git"],
        &repo_a.root,
    );
    run_git(
        &["remote", "add", "origin", "git@github.com:org/work-b.git"],
        &repo_b.root,
    );
    let cas_root_a = init_cas_dir(&repo_a.root).expect("init repo A CAS");
    let cas_root_b = init_cas_dir(&repo_b.root).expect("init repo B CAS");

    let misleading_path = cas_root_b.join("worktrees").join("frontend");
    repo_b.add_worktree(&misleading_path, "factory/other-worker");

    let task_store = open_task_store(&cas_root_a).expect("task store");
    let mut task = Task::new("ambiguous-close".to_string(), "Frontend task".to_string());
    task.assignee = Some("frontend".to_string());
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/org/work-b".to_string(),
        target_branch: "main".to_string(),
    });
    task_store.add(&task).expect("add task");

    let core = CasCore::with_daemon(cas_root_a, None, None);
    let error = core
        .cas_task_update(Parameters(close_update_request(task.id.clone())))
        .await
        .expect_err("mismatched task branch must fail closed");
    assert!(
        error.message.contains("expected task worktree branch"),
        "unexpected rejection: {}",
        error.message
    );
    let persisted = task_store.get(&task.id).expect("persisted task");
    assert_eq!(persisted.status, cas::types::TaskStatus::Open);
    assert!(persisted.deliverables.pre_close_hook.is_none());
}

#[tokio::test]
async fn update_to_closed_failed_hook_leaves_status_and_evidence_unchanged() {
    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let home = TempDir::new().expect("temp HOME");
    let _home = HomeGuard::enter(home.path());
    let repo_a = GitRepo::new();
    let repo_b = GitRepo::new();
    run_git(
        &["remote", "add", "origin", "git@github.com:org/spawn-a.git"],
        &repo_a.root,
    );
    run_git(
        &["remote", "add", "origin", "git@github.com:org/work-b.git"],
        &repo_b.root,
    );
    let cas_root_a = init_cas_dir(&repo_a.root).expect("init repo A CAS");
    let cas_root_b = init_cas_dir(&repo_b.root).expect("init repo B CAS");

    let own_path = cas_root_b.join("worktrees").join("frontend");
    repo_b.add_worktree(&own_path, "factory/frontend");
    std::fs::write(
        own_path.join("unfinished.rs"),
        "pub fn unfinished() { todo!(\"not done\") }\n",
    )
    .unwrap();
    run_git(&["add", "unfinished.rs"], &own_path);
    run_git(&["commit", "-m", "unfinished frontend task"], &own_path);

    let task_store = open_task_store(&cas_root_a).expect("task store");
    let mut task = Task::new("failed-hook-close".to_string(), "Frontend task".to_string());
    task.assignee = Some("frontend".to_string());
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/org/work-b".to_string(),
        target_branch: "main".to_string(),
    });
    task.deliverables.pre_close_hook = Some(cas::types::PreCloseHookEvidence {
        repo_selector: "remote:github.com/org/work-b".to_string(),
        target_branch: "main".to_string(),
        worktree_branch: Some("factory/frontend".to_string()),
        task_tip: Some("prior-success".to_string()),
    });
    task_store.add(&task).expect("add task");

    let core = CasCore::with_daemon(cas_root_a, None, None);
    let error = core
        .cas_task_update(Parameters(close_update_request(task.id.clone())))
        .await
        .expect_err("lint failure must reject update-to-closed");
    assert!(
        error.message.contains("Lightweight structural lint found"),
        "unexpected hook failure: {}",
        error.message
    );
    let persisted = task_store.get(&task.id).expect("persisted task");
    assert_eq!(persisted.status, cas::types::TaskStatus::Open);
    assert_eq!(
        persisted
            .deliverables
            .pre_close_hook
            .as_ref()
            .and_then(|evidence| evidence.task_tip.as_deref()),
        Some("prior-success"),
        "failed hook must not overwrite prior portable evidence"
    );
}

#[tokio::test]
async fn normal_close_lints_task_anchor_not_newer_same_worker_or_unrelated_worktree() {
    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let home = TempDir::new().expect("temp HOME");
    let _home = HomeGuard::enter(home.path());
    let _role = VarGuard::set("CAS_AGENT_ROLE", "worker");
    let _factory = VarGuard::set("CAS_FACTORY_MODE", "1");
    let repo_a = GitRepo::new();
    let repo_b = GitRepo::new();
    run_git(
        &["remote", "add", "origin", "git@github.com:org/spawn-a.git"],
        &repo_a.root,
    );
    run_git(
        &["remote", "add", "origin", "git@github.com:org/work-b.git"],
        &repo_b.root,
    );
    let cas_root_a = init_cas_dir(&repo_a.root).expect("init repo A CAS");
    let cas_root_b = init_cas_dir(&repo_b.root).expect("init repo B CAS");

    let own_path = cas_root_b.join("worktrees").join("frontend");
    repo_b.add_worktree(&own_path, "factory/frontend");
    std::fs::write(own_path.join("task-a.rs"), "pub fn task_a() {}\n").unwrap();
    run_git(&["add", "task-a.rs"], &own_path);
    run_git(&["commit", "-m", "task A"], &own_path);
    let task_a_tip = git_stdout(&own_path, &["rev-parse", "HEAD"]);
    run_git(&["merge", "--no-ff", "factory/frontend", "-m", "merge task A"], &repo_b.root);

    // Same worker starts task B after A was merged. Its bad lint must not be
    // attributed to A; A's recorded anchor is the task-owned proof scope.
    std::fs::write(
        own_path.join("task-b.rs"),
        "pub fn task_b() { todo!(\"later task\") }\n",
    )
    .unwrap();
    run_git(&["add", "task-b.rs"], &own_path);
    run_git(&["commit", "-m", "later task B"], &own_path);

    let unrelated_path = cas_root_b.join("worktrees").join("worker-pulse");
    repo_b.add_worktree(&unrelated_path, "factory/worker-pulse");
    std::fs::write(unrelated_path.join("dirty.ts"), "transient type error").unwrap();

    let task_store = open_task_store(&cas_root_a).expect("task store");
    let mut task = Task::new("normal-close".to_string(), "Task A".to_string());
    task.status = cas::types::TaskStatus::AwaitingMerge;
    task.assignee = Some("frontend".to_string());
    task.deliverables.factory_branch_anchor = Some(task_a_tip.clone());
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/org/work-b".to_string(),
        target_branch: "main".to_string(),
    });
    task_store.add(&task).expect("add task");

    let core = CasCore::with_daemon(cas_root_a, None, None);
    let result = core
        .cas_task_close(Parameters(TaskCloseRequest {
            id: task.id.clone(),
            reason: Some("task A complete".to_string()),
            bypass_code_review: None,
            code_review_findings: None,
            search_manifest: None,
            commit_receipt: None,
        }))
        .await
        .expect("normal close call");
    let text = get_text(&result);
    assert!(
        text.contains("queued for supervisor review"),
        "task A anchor must pass despite later bad task B and dirty pulse worktree: {text}"
    );
    let persisted = task_store.get(&task.id).expect("persisted task");
    let evidence = persisted
        .deliverables
        .pre_close_hook
        .as_ref()
        .expect("hook evidence");
    assert_eq!(evidence.worktree_branch.as_deref(), Some("factory/frontend"));
    assert_eq!(evidence.task_tip.as_deref(), Some(task_a_tip.as_str()));
}

/// Negative case: when neither System A nor System B has a matching
/// worktree, `worktree_merge` must report an accurate "not found" — never
/// silently succeed, and never fall back to the misleading "disabled"
/// message (that message implies the feature is off, not that the target
/// doesn't exist).
#[tokio::test]
async fn test_worktree_merge_reports_not_found_not_disabled_when_nothing_matches() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);
    // No worktree created for "bob" in either system.

    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);

    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/bob".to_string());
    let result = svc.coordination(Parameters(req)).await;

    let (not_disabled, contains_not_found) = match &result {
        Ok(r) => {
            let text = get_text(r);
            (
                !text.contains("experimental and disabled"),
                text.to_lowercase().contains("not found"),
            )
        }
        Err(e) => {
            let msg = format!("{e:?}");
            (
                !msg.contains("experimental and disabled"),
                msg.to_lowercase().contains("not found"),
            )
        }
    };

    assert!(
        not_disabled,
        "a missing worktree must never be reported as 'disabled' — that implies \
         the feature is off, not that the target doesn't exist. Got: {result:?}"
    );
    assert!(
        contains_not_found,
        "a missing worktree should be reported as not found. Got: {result:?}"
    );
}

// =============================================================================
// cas-0938: worktree_merge's System-B fallback must target the worker's
// TASK'S EPIC branch, not the repo trunk — merging an epic worker's commits
// to trunk (then deleting the branch via cleanup_on_close) is a silent
// wrong-target class of bug: worse than cas-1d11's pre-fix refusal, because
// the close-gate still rejects AND unreviewed code now sits on trunk with
// the only copy of it gone.
// =============================================================================

fn create_epic_and_worker_task(
    cas_root: &Path,
    epic_branch: &str,
    assignee: Option<&str>,
) -> (String, String) {
    let task_store = open_task_store(cas_root).expect("open_task_store");

    let mut epic = Task::new("epic-1".to_string(), "Test epic".to_string());
    epic.task_type = TaskType::Epic;
    epic.branch = Some(epic_branch.to_string());
    task_store.add(&epic).expect("add epic task");

    let mut worker_task = Task::new("worker-task-1".to_string(), "Worker task".to_string());
    // cas-bd5f: explicit task_id merges require assignee/lease belonging to the worker.
    if let Some(name) = assignee {
        worker_task.assignee = Some(name.to_string());
    }
    task_store
        .create_atomic(&worker_task, &[], Some(&epic.id), None)
        .expect("create worker task under epic");

    (epic.id, worker_task.id)
}

/// Register a System-B style worker agent and optionally claim a task lease.
fn register_worker_agent(
    cas_root: &Path,
    name: &str,
    factory_session: Option<&str>,
) -> String {
    let agent_store = open_agent_store(cas_root).expect("open_agent_store");
    let id = Agent::generate_fallback_id();
    let mut agent = Agent::new(id.clone(), name.to_string());
    agent.agent_type = AgentType::Worker;
    agent.factory_session = factory_session.map(|s| s.to_string());
    agent_store.register(&agent).expect("register worker agent");
    id
}

#[tokio::test]
async fn test_worktree_merge_targets_epic_branch_when_task_id_given() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    // The epic branch is a real branch in this repo (created off main) so
    // merge_and_cleanup can actually check it out.
    Command::new("git")
        .args(["branch", "epic/foo"])
        .current_dir(&repo.root)
        .output()
        .unwrap();

    // cas-bd5f: task must belong to alice (matching worker/task/epic).
    let (_epic_id, worker_task_id) =
        create_epic_and_worker_task(&cas_root, "epic/foo", Some("alice"));

    let wt_path = cas_root.join("worktrees").join("alice");
    repo.add_worktree(&wt_path, "factory/alice");
    std::fs::write(wt_path.join("alice-work.txt"), "alice's work").unwrap();
    run_git(&["add", "."], &wt_path);
    run_git(&["commit", "-m", "alice work"], &wt_path);

    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);

    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/alice".to_string());
    req.task_id = Some(worker_task_id);
    let result = svc
        .coordination(Parameters(req))
        .await
        .expect("coordination call should succeed");
    let text = get_text(&result);

    assert!(
        text.contains("Merged worktree") && text.contains("epic/foo"),
        "must merge into the task's epic branch (epic/foo), not trunk.\nGot:\n{text}"
    );
    assert!(
        !text.contains("Merged worktree system-b-alice to main")
            && !text.contains("Merged worktree system-b-alice to master"),
        "must NOT merge to trunk when the task has a parent epic.\nGot:\n{text}"
    );
    assert!(
        text.contains("[resolved via:"),
        "the resolved target and why must be surfaced in the success message.\nGot:\n{text}"
    );

    // The epic branch itself must now contain the worker's content — proves
    // the merge landed on the right branch, not just that the message says so.
    let epic_tree = Command::new("git")
        .args(["ls-tree", "-r", "--name-only", "epic/foo"])
        .current_dir(&repo.root)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&epic_tree.stdout).contains("alice-work.txt"),
        "epic/foo must contain the merged worker content"
    );
}

#[tokio::test]
async fn test_worktree_merge_standalone_task_requires_force_for_trunk() {
    // cas-0b32 AC4: standalone (no parent epic) trunk merge needs explicit intent.
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    let task_store = open_task_store(&cas_root).expect("open_task_store");
    let mut standalone_task = Task::new("standalone-1".to_string(), "Standalone task".to_string());
    // cas-bd5f: explicit task_id requires worker ownership.
    standalone_task.assignee = Some("bob".to_string());
    task_store.add(&standalone_task).expect("add standalone task");

    let wt_path = cas_root.join("worktrees").join("bob");
    repo.add_worktree(&wt_path, "factory/bob");

    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);

    let svc = make_service(cas_root.clone());

    // Without allow_trunk: refuse silent trunk (force alone must NOT authorize).
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/bob".to_string());
    req.task_id = Some(standalone_task.id.clone());
    req.force = Some(true); // dirty bypass only — must not open trunk
    let refused = svc.coordination(Parameters(req)).await;
    assert!(
        refused.is_err(),
        "standalone task with force but without allow_trunk must refuse trunk"
    );
    let msg = format!("{:?}", refused.unwrap_err());
    assert!(
        msg.contains("no parent epic") || msg.contains("refusing") || msg.contains("allow_trunk"),
        "refusal must explain missing parent epic / allow_trunk. Got: {msg}"
    );

    // allow_trunk=true (force=false): trunk authorized without dirty bypass.
    let mut trunk_ok = coord_req("worktree_merge");
    trunk_ok.id = Some("factory/bob".to_string());
    trunk_ok.task_id = Some(standalone_task.id.clone());
    trunk_ok.allow_trunk = Some(true);
    trunk_ok.force = Some(false);
    let result = svc
        .coordination(Parameters(trunk_ok))
        .await
        .expect("allow_trunk=true standalone merge should succeed");
    let text = get_text(&result);
    assert!(
        text.contains("Merged worktree"),
        "allow_trunk=true must allow trunk for standalone task.\nGot:\n{text}"
    );
    assert!(
        text.contains("allow_trunk=true") || text.contains("no parent epic"),
        "reason must cite allow_trunk / no parent epic.\nGot:\n{text}"
    );
}

#[tokio::test]
async fn test_worktree_merge_refuses_when_task_id_does_not_exist() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    let wt_path = cas_root.join("worktrees").join("carol");
    repo.add_worktree(&wt_path, "factory/carol");

    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);

    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/carol".to_string());
    req.task_id = Some("cas-does-not-exist".to_string());
    let result = svc.coordination(Parameters(req)).await;

    // A caller-asserted task_id we can't verify must refuse — never guess a
    // merge target (that's exactly how the original wrong-target-to-trunk
    // defect happened) and never silently merge to trunk instead.
    assert!(
        result.is_err(),
        "an unresolvable task_id must be refused, not silently fall back to trunk"
    );
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("not found") || msg.to_lowercase().contains("not found"),
        "the refusal should explain the task_id couldn't be resolved. Got: {msg}"
    );

    // The worktree must survive the refused merge — not silently deleted.
    assert!(
        wt_path.exists(),
        "a refused merge must not clean up / delete the worktree"
    );
}

#[tokio::test]
async fn test_worktree_merge_refuses_silent_trunk_when_no_task_id_and_no_epic_context() {
    // cas-0b32: the old cas-1d11/cas-0938 "no task_id → trunk" path is the
    // hv-director→main incident. Without epic context, refuse unless force.
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    let wt_path = cas_root.join("worktrees").join("dave");
    repo.add_worktree(&wt_path, "factory/dave");

    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);

    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/dave".to_string());
    let result = svc.coordination(Parameters(req)).await;
    assert!(
        result.is_err(),
        "no task_id / no epic / no focus must refuse silent trunk (cas-0b32)"
    );
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("refusing silent trunk")
            || msg.contains("cas-0b32")
            || msg.contains("Remediation"),
        "refusal must explain silent-trunk ban + remediation. Got: {msg}"
    );
    assert!(
        wt_path.exists(),
        "refused merge must not delete the worktree"
    );
}

/// cas-0b32 AC1/AC5: System-B worker assigned to an epic, merge without
/// task_id (supervisor pattern that previously hit main) → epic branch.
/// Reproduces the hv-director / cas-9fff / cas-0e22 incident shape.
#[tokio::test]
async fn test_worktree_merge_uses_assignee_epic_when_no_task_id_cas_0b32() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    Command::new("git")
        .args([
            "branch",
            "epic/epic-triage-and-fix-jul-9-11-docs-requests-factory-cas-0e22",
        ])
        .current_dir(&repo.root)
        .output()
        .unwrap();

    let task_store = open_task_store(&cas_root).expect("open_task_store");
    let mut epic = Task::new("cas-0e22".to_string(), "EPIC triage".to_string());
    epic.task_type = TaskType::Epic;
    epic.branch = Some(
        "epic/epic-triage-and-fix-jul-9-11-docs-requests-factory-cas-0e22".to_string(),
    );
    task_store.add(&epic).expect("add epic");

    let mut worker_task = Task::new("cas-9fff".to_string(), "Director routing".to_string());
    worker_task.assignee = Some("hv-director".to_string());
    worker_task.status = cas::types::TaskStatus::InProgress;
    task_store
        .create_atomic(&worker_task, &[], Some(&epic.id), None)
        .expect("create child under epic");

    let wt_path = cas_root.join("worktrees").join("hv-director");
    repo.add_worktree(&wt_path, "factory/hv-director");
    std::fs::write(wt_path.join("director-fix.txt"), "work").unwrap();
    run_git(&["add", "."], &wt_path);
    run_git(&["commit", "-m", "director work"], &wt_path);

    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);

    let svc = make_service(cas_root);
    // Incident shape: id only, no task_id.
    let mut req = coord_req("worktree_merge");
    req.id = Some("hv-director".to_string());
    let result = svc
        .coordination(Parameters(req))
        .await
        .expect("assignee epic merge should succeed without task_id");
    let text = get_text(&result);

    assert!(
        text.contains("epic/epic-triage-and-fix-jul-9-11-docs-requests-factory-cas-0e22"),
        "must merge to epic branch, not main. Got:\n{text}"
    );
    assert!(
        !text.contains("to main") && !text.contains("to master"),
        "must never silently land on trunk. Got:\n{text}"
    );
    assert!(
        text.contains("assignee") || text.contains("parent epic"),
        "reason should cite assignee/epic resolution. Got:\n{text}"
    );

    let epic_tree = Command::new("git")
        .args([
            "ls-tree",
            "-r",
            "--name-only",
            "epic/epic-triage-and-fix-jul-9-11-docs-requests-factory-cas-0e22",
        ])
        .current_dir(&repo.root)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&epic_tree.stdout).contains("director-fix.txt"),
        "epic branch must contain merged content"
    );
}

/// cas-0b32 AC2: focused epic is honored when unambiguous (no assignee epic).
#[tokio::test]
async fn test_worktree_merge_uses_focused_epic_when_unambiguous_cas_0b32() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    Command::new("git")
        .args(["branch", "epic/focused"])
        .current_dir(&repo.root)
        .output()
        .unwrap();

    let task_store = open_task_store(&cas_root).expect("open_task_store");
    let mut epic = Task::new("cas-focus".to_string(), "Focused epic".to_string());
    epic.task_type = TaskType::Epic;
    epic.branch = Some("epic/focused".to_string());
    task_store.add(&epic).expect("add epic");

    // Pin focused epic via session metadata (same store focus_epic writes).
    let session = "test-focus-session-0b32";
    let home = TempDir::new().expect("home");
    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);
    let prev_session = std::env::var("CAS_FACTORY_SESSION").ok();
    let prev_home = std::env::var("HOME").ok();
    // SAFETY: exclusive merge_cwd_lock serializes env mutation in this file.
    unsafe {
        std::env::set_var("CAS_FACTORY_SESSION", session);
        std::env::set_var("HOME", home.path());
    }
    let meta_path = cas::ui::factory::metadata_path(session);
    std::fs::create_dir_all(meta_path.parent().expect("metadata parent")).unwrap();
    let workers = vec!["erin".to_string()];
    let mut meta = cas::ui::factory::create_metadata(
        session,
        1,
        "supervisor",
        &workers,
        None,
        Some(repo.root.to_str().unwrap()),
        None,
    );
    meta.pinned_epic_id = Some("cas-focus".to_string());
    std::fs::write(
        &meta_path,
        serde_json::to_string_pretty(&meta).expect("serialize metadata"),
    )
    .expect("write session metadata");

    let wt_path = cas_root.join("worktrees").join("erin");
    repo.add_worktree(&wt_path, "factory/erin");
    std::fs::write(wt_path.join("erin-work.txt"), "work").unwrap();
    run_git(&["add", "."], &wt_path);
    run_git(&["commit", "-m", "erin work"], &wt_path);

    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/erin".to_string());
    let result = svc.coordination(Parameters(req)).await;

    unsafe {
        match prev_session {
            Some(v) => std::env::set_var("CAS_FACTORY_SESSION", v),
            None => std::env::remove_var("CAS_FACTORY_SESSION"),
        }
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    let result = result.expect("focused epic merge should succeed");
    let text = get_text(&result);
    assert!(
        text.contains("epic/focused") && text.contains("focused epic"),
        "must merge via focused epic. Got:\n{text}"
    );
}

/// cas-0b32 review P1: focused epic with mismatched project_dir is ignored
/// (cross-project / stale) — refuse silent trunk without allow_trunk.
#[tokio::test]
async fn test_worktree_merge_rejects_cross_project_focused_epic_cas_0b32() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    Command::new("git")
        .args(["branch", "epic/focused"])
        .current_dir(&repo.root)
        .output()
        .unwrap();
    let task_store = open_task_store(&cas_root).expect("open_task_store");
    let mut epic = Task::new("cas-focus".to_string(), "Focused epic".to_string());
    epic.task_type = TaskType::Epic;
    epic.branch = Some("epic/focused".to_string());
    task_store.add(&epic).unwrap();

    let session = "test-focus-cross-project-0b32";
    let home = TempDir::new().expect("home");
    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);
    let prev_session = std::env::var("CAS_FACTORY_SESSION").ok();
    let prev_home = std::env::var("HOME").ok();
    unsafe {
        std::env::set_var("CAS_FACTORY_SESSION", session);
        std::env::set_var("HOME", home.path());
    }
    let meta_path = cas::ui::factory::metadata_path(session);
    std::fs::create_dir_all(meta_path.parent().unwrap()).unwrap();
    let workers = vec!["erin".to_string()];
    let mut meta = cas::ui::factory::create_metadata(
        session,
        1,
        "supervisor",
        &workers,
        None,
        Some("/tmp/other-project-not-this-repo"), // mismatched project_dir
        None,
    );
    meta.pinned_epic_id = Some("cas-focus".to_string());
    std::fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap()).unwrap();

    let wt_path = cas_root.join("worktrees").join("erin");
    repo.add_worktree(&wt_path, "factory/erin");

    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/erin".to_string());
    let result = svc.coordination(Parameters(req)).await;
    unsafe {
        match prev_session {
            Some(v) => std::env::set_var("CAS_FACTORY_SESSION", v),
            None => std::env::remove_var("CAS_FACTORY_SESSION"),
        }
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
    assert!(
        result.is_err(),
        "cross-project focused epic must not authorize merge to that epic/trunk silently"
    );
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("refusing silent trunk") || msg.contains("Remediation"),
        "must refuse with remediation. Got: {msg}"
    );
}

/// cas-0b32 review P1: focused epic with matching project but worker not in
/// session membership → ignore focus, refuse trunk.
#[tokio::test]
async fn test_worktree_merge_rejects_focused_epic_for_non_member_worker_cas_0b32() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    Command::new("git")
        .args(["branch", "epic/focused"])
        .current_dir(&repo.root)
        .output()
        .unwrap();
    let task_store = open_task_store(&cas_root).expect("open_task_store");
    let mut epic = Task::new("cas-focus".to_string(), "Focused epic".to_string());
    epic.task_type = TaskType::Epic;
    epic.branch = Some("epic/focused".to_string());
    task_store.add(&epic).unwrap();

    let session = "test-focus-non-member-0b32";
    let home = TempDir::new().expect("home");
    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);
    let prev_session = std::env::var("CAS_FACTORY_SESSION").ok();
    let prev_home = std::env::var("HOME").ok();
    unsafe {
        std::env::set_var("CAS_FACTORY_SESSION", session);
        std::env::set_var("HOME", home.path());
    }
    let meta_path = cas::ui::factory::metadata_path(session);
    std::fs::create_dir_all(meta_path.parent().unwrap()).unwrap();
    // Session workers list does NOT include "stranger".
    let workers = vec!["other-worker".to_string()];
    let mut meta = cas::ui::factory::create_metadata(
        session,
        1,
        "supervisor",
        &workers,
        None,
        Some(repo.root.to_str().unwrap()),
        None,
    );
    meta.pinned_epic_id = Some("cas-focus".to_string());
    std::fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap()).unwrap();

    let wt_path = cas_root.join("worktrees").join("stranger");
    repo.add_worktree(&wt_path, "factory/stranger");

    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/stranger".to_string());
    let result = svc.coordination(Parameters(req)).await;
    unsafe {
        match prev_session {
            Some(v) => std::env::set_var("CAS_FACTORY_SESSION", v),
            None => std::env::remove_var("CAS_FACTORY_SESSION"),
        }
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
    assert!(
        result.is_err(),
        "non-member worker must not inherit focused epic merge target"
    );
}

/// cas-0b32 second review: one branchful + one branchless active parent must
/// reject (must not silently pick the branchful epic).
#[tokio::test]
async fn test_worktree_merge_rejects_mixed_branchful_and_branchless_parent_epics_cas_0b32() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    Command::new("git")
        .args(["branch", "epic/with-branch"])
        .current_dir(&repo.root)
        .output()
        .unwrap();

    let task_store = open_task_store(&cas_root).expect("open_task_store");
    let mut epic_ok = Task::new("epic-ok".to_string(), "Has branch".to_string());
    epic_ok.task_type = TaskType::Epic;
    epic_ok.branch = Some("epic/with-branch".to_string());
    task_store.add(&epic_ok).unwrap();
    let mut epic_nb = Task::new("epic-nb".to_string(), "No branch".to_string());
    epic_nb.task_type = TaskType::Epic;
    epic_nb.branch = None;
    task_store.add(&epic_nb).unwrap();

    let mut t1 = Task::new("t-ok".to_string(), "Under branchful".to_string());
    t1.assignee = Some("mixed".to_string());
    t1.status = cas::types::TaskStatus::InProgress;
    task_store
        .create_atomic(&t1, &[], Some("epic-ok"), None)
        .unwrap();
    let mut t2 = Task::new("t-nb".to_string(), "Under branchless".to_string());
    t2.assignee = Some("mixed".to_string());
    t2.status = cas::types::TaskStatus::InProgress;
    task_store
        .create_atomic(&t2, &[], Some("epic-nb"), None)
        .unwrap();

    let wt_path = cas_root.join("worktrees").join("mixed");
    repo.add_worktree(&wt_path, "factory/mixed");

    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);
    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/mixed".to_string());
    let result = svc.coordination(Parameters(req)).await;
    assert!(
        result.is_err(),
        "mixed branchful+branchless parent epics must reject, not pick branchful"
    );
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        (msg.contains("no branch") || msg.contains("branch field"))
            && (msg.contains("epic-nb") || msg.contains("branchless") || msg.contains("epic-ok")),
        "must cite branchless parent and not silently merge. Got: {msg}"
    );
    assert!(
        !msg.contains("Merged worktree"),
        "must not have merged. Got: {msg}"
    );
}

/// cas-0b32 review P2: branchless parent epic rejects (no fall-through).
#[tokio::test]
async fn test_worktree_merge_rejects_branchless_assignee_parent_epic_cas_0b32() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    let task_store = open_task_store(&cas_root).expect("open_task_store");
    let mut epic = Task::new("epic-nobranch".to_string(), "No branch epic".to_string());
    epic.task_type = TaskType::Epic;
    epic.branch = None;
    task_store.add(&epic).unwrap();
    let mut t = Task::new("t-nobranch".to_string(), "Child".to_string());
    t.assignee = Some("nb".to_string());
    t.status = cas::types::TaskStatus::InProgress;
    task_store
        .create_atomic(&t, &[], Some("epic-nobranch"), None)
        .unwrap();

    let wt_path = cas_root.join("worktrees").join("nb");
    repo.add_worktree(&wt_path, "factory/nb");

    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);
    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/nb".to_string());
    let result = svc.coordination(Parameters(req)).await;
    assert!(result.is_err(), "branchless parent epic must reject");
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("no branch") || msg.contains("branch field"),
        "must cite missing branch. Got: {msg}"
    );
}

/// cas-0b32 AC3: multiple assignee epics → reject with remediation.
#[tokio::test]
async fn test_worktree_merge_rejects_ambiguous_assignee_epics_cas_0b32() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    for b in ["epic/a", "epic/b"] {
        Command::new("git")
            .args(["branch", b])
            .current_dir(&repo.root)
            .output()
            .unwrap();
    }

    let task_store = open_task_store(&cas_root).expect("open_task_store");
    let mut epic_a = Task::new("epic-a".to_string(), "Epic A".to_string());
    epic_a.task_type = TaskType::Epic;
    epic_a.branch = Some("epic/a".to_string());
    task_store.add(&epic_a).unwrap();
    let mut epic_b = Task::new("epic-b".to_string(), "Epic B".to_string());
    epic_b.task_type = TaskType::Epic;
    epic_b.branch = Some("epic/b".to_string());
    task_store.add(&epic_b).unwrap();

    let mut t1 = Task::new("t1".to_string(), "T1".to_string());
    t1.assignee = Some("multi".to_string());
    t1.status = cas::types::TaskStatus::InProgress;
    task_store
        .create_atomic(&t1, &[], Some("epic-a"), None)
        .unwrap();
    let mut t2 = Task::new("t2".to_string(), "T2".to_string());
    t2.assignee = Some("multi".to_string());
    t2.status = cas::types::TaskStatus::InProgress;
    task_store
        .create_atomic(&t2, &[], Some("epic-b"), None)
        .unwrap();

    let wt_path = cas_root.join("worktrees").join("multi");
    repo.add_worktree(&wt_path, "factory/multi");

    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);

    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/multi".to_string());
    let result = svc.coordination(Parameters(req)).await;
    assert!(result.is_err(), "ambiguous assignee epics must reject");
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("ambiguous") && msg.contains("Remediation"),
        "must explain ambiguity + remediation. Got: {msg}"
    );
}

/// cas-0938 P3: System-B path resolution must honor a customized
/// `worktrees.base_path`, not the hardcoded `<cas_root>/worktrees/<assignee>`
/// convention — `spawn_workers isolate=true` itself resolves paths via
/// `WorktreeManager::worktree_path_for_worker`, which respects this config,
/// so a hardcoded path in `worktree_merge` would false-not-found any worker
/// spawned under a non-default layout.
#[tokio::test]
async fn test_worktree_merge_honors_configured_base_path_not_hardcoded_convention() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    // Unique base_path under the temp parent so parallel/rerun tests don't
    // collide on a shared /tmp/custom-worktree-loc path.
    let unique = format!(
        "custom-wt-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    std::fs::write(
        cas_root.join("config.toml"),
        format!("[worktrees]\nenabled = false\nbase_path = \"{unique}\"\n"),
    )
    .unwrap();

    // Mirrors WorktreeManager::worktree_root()'s resolution for a relative,
    // non-{project} base_path: repo_root.parent().join(base_path).
    let expected_root = repo.root.parent().unwrap().join(&unique).join("erin");
    repo.add_worktree(&expected_root, "factory/erin");

    // Sanity: this is NOT where the old hardcoded convention would look.
    assert_ne!(expected_root, cas_root.join("worktrees").join("erin"));

    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);

    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/erin".to_string());
    // Path-resolution fixture has no epic context — allow_trunk (not force).
    req.allow_trunk = Some(true);
    let result = svc
        .coordination(Parameters(req))
        .await
        .expect("coordination call should succeed");
    let text = get_text(&result);

    assert!(
        text.contains("Merged worktree"),
        "must find and merge the worker worktree at its CONFIGURED location, \
         not the hardcoded default.\nGot:\n{text}"
    );
    assert!(
        !text.contains("Worktree not found"),
        "must not false-not-found a worker under a customized base_path.\nGot:\n{text}"
    );
}

// =============================================================================
// cas-bd5f: worktree_merge explicit task_id must belong to the worker being merged.
// A foreign task_id must not redirect worker A's branch into another task's epic.
// =============================================================================

/// AC1: Matching worker + assigned task + parent epic resolves normally.
#[tokio::test]
async fn test_worktree_merge_task_id_matching_worker_succeeds_cas_bd5f() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    Command::new("git")
        .args(["branch", "epic/match"])
        .current_dir(&repo.root)
        .output()
        .unwrap();

    let (_epic_id, task_id) =
        create_epic_and_worker_task(&cas_root, "epic/match", Some("alice"));

    let wt_path = cas_root.join("worktrees").join("alice");
    repo.add_worktree(&wt_path, "factory/alice");
    std::fs::write(wt_path.join("match.txt"), "ok").unwrap();
    run_git(&["add", "."], &wt_path);
    run_git(&["commit", "-m", "match work"], &wt_path);

    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);

    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/alice".to_string());
    req.task_id = Some(task_id);
    let result = svc
        .coordination(Parameters(req))
        .await
        .expect("matching worker/task must merge");
    let text = get_text(&result);
    assert!(
        text.contains("Merged worktree") && text.contains("epic/match"),
        "matching worker/task/epic must resolve. Got:\n{text}"
    );
    assert!(
        text.contains("authorized for worker alice"),
        "success reason should note authorization. Got:\n{text}"
    );
}

/// AC2: Worker A + task assigned to worker B rejects — no foreign epic redirect.
#[tokio::test]
async fn test_worktree_merge_rejects_foreign_task_assignee_cas_bd5f() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    Command::new("git")
        .args(["branch", "epic/foreign"])
        .current_dir(&repo.root)
        .output()
        .unwrap();

    // Task belongs to bob; we attempt to merge alice with that task_id.
    let (_epic_id, foreign_task_id) =
        create_epic_and_worker_task(&cas_root, "epic/foreign", Some("bob"));

    let wt_path = cas_root.join("worktrees").join("alice");
    repo.add_worktree(&wt_path, "factory/alice");

    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);

    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/alice".to_string());
    req.task_id = Some(foreign_task_id.clone());
    let result = svc.coordination(Parameters(req)).await;

    assert!(
        result.is_err(),
        "worker A + task assigned to worker B must reject"
    );
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("authorization failed")
            || msg.contains("cas-bd5f")
            || msg.contains("assigned to"),
        "refusal must be audit-ready about ownership mismatch. Got: {msg}"
    );
    assert!(
        msg.contains("alice") && msg.contains("bob"),
        "diagnostics must name both workers. Got: {msg}"
    );
    assert!(
        wt_path.exists(),
        "rejected merge must not delete the worktree"
    );
}

/// AC3: Missing assignee and no active lease → conservative reject.
#[tokio::test]
async fn test_worktree_merge_rejects_task_without_assignee_or_lease_cas_bd5f() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    Command::new("git")
        .args(["branch", "epic/orphan"])
        .current_dir(&repo.root)
        .output()
        .unwrap();

    // Intentionally no assignee — pre-cas-bd5f this would still merge to epic.
    let (_epic_id, orphan_task_id) =
        create_epic_and_worker_task(&cas_root, "epic/orphan", None);

    let wt_path = cas_root.join("worktrees").join("carol");
    repo.add_worktree(&wt_path, "factory/carol");

    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);

    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/carol".to_string());
    req.task_id = Some(orphan_task_id);
    let result = svc.coordination(Parameters(req)).await;

    assert!(
        result.is_err(),
        "missing assignee/lease must conservatively reject"
    );
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("no assignee")
            || msg.contains("conservative")
            || msg.contains("authorization failed"),
        "refusal must cite missing assignee/lease. Got: {msg}"
    );
}

/// AC4: Cross-session — active lease held by a different agent rejects even if
/// the display name could be confused; worker alice must not inherit bob's lease.
#[tokio::test]
async fn test_worktree_merge_rejects_cross_session_lease_mismatch_cas_bd5f() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    Command::new("git")
        .args(["branch", "epic/xsession"])
        .current_dir(&repo.root)
        .output()
        .unwrap();

    // Task has no assignee field but an active lease held by bob in session-B.
    let (_epic_id, task_id) =
        create_epic_and_worker_task(&cas_root, "epic/xsession", None);

    let bob_id = register_worker_agent(&cas_root, "bob", Some("session-b"));
    let agent_store = open_agent_store(&cas_root).expect("open_agent_store");
    agent_store
        .try_claim(&task_id, &bob_id, 600, Some("bob owns this"))
        .expect("bob claims task");

    // alice is a separate session agent — name does not match bob's lease.
    let _alice_id = register_worker_agent(&cas_root, "alice", Some("session-a"));

    let wt_path = cas_root.join("worktrees").join("alice");
    repo.add_worktree(&wt_path, "factory/alice");

    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);

    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/alice".to_string());
    req.task_id = Some(task_id);
    let result = svc.coordination(Parameters(req)).await;

    assert!(
        result.is_err(),
        "cross-session lease holder mismatch must reject"
    );
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("lease") || msg.contains("authorization failed") || msg.contains("cas-bd5f"),
        "refusal must cite lease ownership. Got: {msg}"
    );
    assert!(
        msg.contains("alice"),
        "diagnostics must name the worker being merged. Got: {msg}"
    );
}

/// Lease held by the matching worker authorizes even when assignee field is empty.
#[tokio::test]
async fn test_worktree_merge_task_id_authorized_via_matching_lease_cas_bd5f() {
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root).expect("init_cas_dir");
    disable_system_a(&cas_root);

    Command::new("git")
        .args(["branch", "epic/lease-ok"])
        .current_dir(&repo.root)
        .output()
        .unwrap();

    let (_epic_id, task_id) =
        create_epic_and_worker_task(&cas_root, "epic/lease-ok", None);

    let alice_id = register_worker_agent(&cas_root, "alice", Some("session-a"));
    let agent_store = open_agent_store(&cas_root).expect("open_agent_store");
    agent_store
        .try_claim(&task_id, &alice_id, 600, Some("alice lease"))
        .expect("alice claims task");

    let wt_path = cas_root.join("worktrees").join("alice");
    repo.add_worktree(&wt_path, "factory/alice");
    std::fs::write(wt_path.join("lease-work.txt"), "via lease").unwrap();
    run_git(&["add", "."], &wt_path);
    run_git(&["commit", "-m", "lease work"], &wt_path);

    let _lock = merge_cwd_lock().lock().unwrap_or_else(|p| p.into_inner());
    let _cwd = CwdGuard::enter(&repo.root);

    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/alice".to_string());
    req.task_id = Some(task_id);
    let result = svc
        .coordination(Parameters(req))
        .await
        .expect("matching lease must authorize task_id");
    let text = get_text(&result);
    assert!(
        text.contains("Merged worktree") && text.contains("epic/lease-ok"),
        "lease-authorized merge must target epic. Got:\n{text}"
    );
}
