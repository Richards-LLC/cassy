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

use cas::mcp::tools::{
    AgentRegisterRequest, SessionStartRequest, TaskCloseRequest, TaskUpdateRequest,
    VerificationAddRequest,
};
use cas::mcp::{CasCore, CasService};
use cas::store::{open_agent_store, open_task_store, open_verification_store, open_worktree_store};
use cas::types::{
    Agent, AgentRole, AgentType, Task, TaskDepth, TaskStatus, TaskType, Verification,
    VerificationStatus, WorkTarget, WorkerCompletionReceiptInput, WorkerDeliveryState, Worktree,
};
use cas_mcp::types::{CoordinationRequest, TaskRequest, VerificationRequest};
use cas_store::KnownRepoStore;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::RawContent;
use tempfile::TempDir;

#[path = "../src/test_env_guard.rs"]
mod test_env_guard;
use test_env_guard::TestEnvGuard;

/// cas-6a30: every test in this integration binary shares one process. Direct
/// HOME/XDG_CONFIG_HOME mutation bypasses the canonical guard and can move the
/// host registry between schema installation and strict store open.
#[test]
fn process_home_mutation_uses_the_canonical_test_env_guard() {
    let source = include_str!("worktree_surface_test.rs");
    let offenders = source
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            let mutates_env =
                line.contains("std::env::set_var") || line.contains("std::env::remove_var");
            let mutates_home = line.contains("\"HOME\"") || line.contains("\"XDG_CONFIG_HOME\"");
            mutates_env && mutates_home
        })
        .map(|(index, line)| format!("{}: {}", index + 1, line.trim()))
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "HOME/XDG_CONFIG_HOME must be mutated through TestEnvGuard: {offenders:?}"
    );
}

/// cas-f211: integration-test binaries inherit the factory session that ran
/// `cargo test`. Prove the public authority boundary has the same result when
/// its parent is a live supervisor as it does in a clean shell.
#[test]
fn authority_boundary_test_is_hermetic_against_inherited_factory_env() {
    let output = Command::new(std::env::current_exe().expect("current test executable"))
        .args([
            "--exact",
            "public_registration_cannot_mint_or_capture_supervisor_verification_authority",
            "--nocapture",
        ])
        .env("CAS_AGENT_ROLE", "supervisor")
        .env("CAS_AGENT_NAME", "inherited-supervisor")
        .env("CAS_SESSION_ID", "inherited-session")
        .env("CAS_FACTORY_SESSION", "inherited-factory")
        .output()
        .expect("run authority-boundary test under inherited factory env");

    assert!(
        output.status.success(),
        "authority-boundary test must be hermetic under inherited CAS_* env\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Initialize a project fixture while keeping the host-level known-repo
/// registry inside this test binary's private HOME. Several System-A tests
/// intentionally exercise registry resolution, so simply suppressing the
/// registration would make the fixture less realistic.
fn test_env() -> TestEnvGuard {
    use std::sync::OnceLock;

    static TEST_HOME: OnceLock<TempDir> = OnceLock::new();
    let home = TEST_HOME.get_or_init(|| TempDir::new().expect("isolated worktree test HOME"));
    let home = home
        .path()
        .canonicalize()
        .expect("canonical isolated worktree test HOME");
    let xdg = home.join("xdg");
    TestEnvGuard::with_optional_vars(&[
        ("HOME", home.to_str()),
        ("XDG_CONFIG_HOME", xdg.to_str()),
        // Close-time verification policy derives the factory harness from the
        // ambient environment (harness_policy::worker_harness_from_env): under
        // an inherited CAS_FACTORY_WORKER_CLI=codex, task verification is
        // Bypassed (codex has no subagent support) and close outcomes flip —
        // e.g. resolved_task_proof_freezes_scope_until_supervisor_starts_a_
        // fresh_cycle expects VERIFICATION REQUIRED but sees a successful
        // close. Pin both halves of the policy so results do not depend on
        // which harness launched `cargo test`.
        ("CAS_FACTORY_WORKER_CLI", Some("claude")),
        ("CAS_FACTORY_SUPERVISOR_CLI", Some("claude")),
        // Authority-boundary fixtures must begin without a caller identity.
        // A live factory supervisor exports these into `cargo test`; leaving
        // them ambient lets public registration inherit privileged authority
        // and makes clean-shell and factory-shell results disagree.
        ("CAS_AGENT_ROLE", None),
        ("CAS_AGENT_NAME", None),
        ("CAS_SESSION_ID", None),
        ("CAS_FACTORY_SESSION", None),
        ("CAS_FACTORY_MODE", None),
    ])
}

fn init_cas_dir(path: &Path, _env: &mut TestEnvGuard) -> anyhow::Result<PathBuf> {
    let cas_root = cas::store::init_cas_dir(path)?;
    cas::store::known_repos::ensure_host_schema()?;
    cas::store::known_repos::register_repo_strict(path)?;
    Ok(cas_root)
}

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
        let root = temp.path().canonicalize().expect("canonical repo fixture");

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
            .args(["worktree", "add", "-b", branch, path.to_str().unwrap()])
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
        delivery_mode: None,
        merge_request: None,
        blocker: None,
        in_reply_to: None,
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
        lane: None,
        branch: None,
        older_than_secs: None,
        isolate: None,
        cli: None,
        model: None,
        effort: None,
        config_dir: None,
        workers: None,
        remind_message: None,
        remind_delay_secs: None,
        remind_event: None,
        remind_filter: None,
        remind_id: None,
        remind_ttl_secs: None,
        cross_session: None,
        all: None,
        status: None,
        orphans: None,
        dry_run: None,
        command: None,
        cwd: None,
        port: None,
        shared: None,
    }
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

fn close_update_request(id: String) -> TaskUpdateRequest {
    TaskUpdateRequest {
        blocked_by: None,
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
        origin_project: None,
        epic_verification_owner: None,
        depth: None,
    }
}

fn durable_close_snapshot(cas_root: &Path) -> Vec<(String, Vec<Vec<String>>)> {
    const TABLES: &[&str] = &[
        "agents",
        "tasks",
        "worker_completion_receipts",
        "worker_delivery_transactions",
        "worker_delivery_events",
        "verification_dispatches",
        "verifications",
        "verification_issues",
        "events",
        "task_leases",
        "task_lease_history",
        "supervisor_queue",
        "prompt_queue",
    ];
    let conn = rusqlite::Connection::open(cas_root.join("cas.db")).unwrap();
    TABLES
        .iter()
        .map(|table| {
            let exists = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                    [table],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap();
            if !exists {
                return ((*table).to_string(), Vec::new());
            }
            let mut statement = conn
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .unwrap();
            let column_count = statement.column_count();
            let rows = statement
                .query_map([], |row| {
                    (0..column_count)
                        .map(|index| {
                            use rusqlite::types::ValueRef;
                            Ok(match row.get_ref(index)? {
                                ValueRef::Null => "null".to_string(),
                                ValueRef::Integer(value) => format!("i:{value}"),
                                ValueRef::Real(value) => format!("f:{:016x}", value.to_bits()),
                                ValueRef::Text(value) => {
                                    format!("t:{}", String::from_utf8_lossy(value))
                                }
                                ValueRef::Blob(value) => format!(
                                    "b:{}",
                                    value
                                        .iter()
                                        .map(|byte| format!("{byte:02x}"))
                                        .collect::<String>()
                                ),
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

fn seed_direct_close_delivery(
    cas_root: &Path,
    task_id: &str,
    state: WorkerDeliveryState,
    commit_sha: &str,
) {
    let input = WorkerCompletionReceiptInput {
        task_id: task_id.to_string(),
        worker_agent_id: "delivery-worker-session".to_string(),
        repo_selector: "remote:github.com/org/direct-close".to_string(),
        source_branch: "factory/delivery-worker".to_string(),
        commit_sha: commit_sha.to_string(),
        merge_base_sha: commit_sha.to_string(),
        target_branch: "main".to_string(),
        target_sha: commit_sha.to_string(),
        proof_reference: "proof:direct-close-state".to_string(),
        scope_summary: "direct close delivery-state fixture".to_string(),
        artifact_path: None,
    };
    let receipt =
        cas_store::build_worker_completion_receipt(&input, "delivery-worker", chrono::Utc::now());
    let (mut transaction, dispatch) = cas_store::create_worker_delivery_with_dispatch(
        cas_root,
        &receipt,
        WorkerDeliveryState::AwaitingVerification,
        "delivery-worker-session",
        "delivery-supervisor-session",
        chrono::Utc::now() + chrono::Duration::minutes(10),
    )
    .expect("seed delivery");
    if state == WorkerDeliveryState::AwaitingVerification {
        return;
    }

    let mut supervisor = Agent::new(
        "delivery-supervisor-session".to_string(),
        "delivery-supervisor".to_string(),
    );
    supervisor.role = AgentRole::Supervisor;
    let agent_store = open_agent_store(cas_root).expect("agent store");
    agent_store
        .register(&supervisor)
        .expect("register delivery supervisor");

    let mut verification = Verification::new(format!("ver-{task_id}"), task_id.to_string());
    verification.provenance = cas::types::VerificationProvenance::SupervisorDirect;
    verification.agent_id = Some(supervisor.id.clone());
    verification.issuer_agent_id = Some(supervisor.id.clone());
    verification.dispatch_id = Some(dispatch.id.clone());
    verification.status = if state == WorkerDeliveryState::VerificationFailed {
        VerificationStatus::Rejected
    } else {
        VerificationStatus::Approved
    };
    verification.summary = format!("fixture verdict for {state}");
    open_verification_store(cas_root)
        .unwrap()
        .add(&verification)
        .expect("seed verdict");
    let conn = rusqlite::Connection::open(cas_root.join("cas.db")).expect("db");
    cas_store::resolve_verification_dispatch_with_conn(
        &conn,
        &dispatch.id,
        &supervisor.id,
        None,
        true,
    )
    .expect("resolve seeded delivery dispatch");
    agent_store
        .unregister(&supervisor.id)
        .expect("fixture supervisor goes offline after verdict");

    if state == WorkerDeliveryState::VerificationFailed {
        cas_store::transition_worker_delivery(
            cas_root,
            &transaction.id,
            &[WorkerDeliveryState::AwaitingVerification],
            state,
            "delivery-supervisor-session",
            Some("delivery-supervisor-session"),
            Some(&verification.id),
            None,
            Some(("verification_failed", "fixture rejected")),
        )
        .expect("seed failed delivery");
        return;
    }

    for next in [
        WorkerDeliveryState::AwaitingMerge,
        WorkerDeliveryState::MergeAuthorized,
        WorkerDeliveryState::Merged,
        WorkerDeliveryState::CloseReady,
        WorkerDeliveryState::Delivered,
    ] {
        let current = transaction.state;
        let merge_sha = "dddddddddddddddddddddddddddddddddddddddd";
        transaction = cas_store::transition_worker_delivery(
            cas_root,
            &transaction.id,
            &[current],
            next,
            "delivery-supervisor-session",
            Some("delivery-supervisor-session"),
            Some(&verification.id),
            (next == WorkerDeliveryState::Merged).then_some(merge_sha),
            None,
        )
        .expect("advance delivery fixture");
        if next == state {
            return;
        }
    }
    panic!("unsupported direct-close delivery fixture state {state}");
}

async fn exercise_direct_close_delivery_state(state: WorkerDeliveryState, expect_close: bool) {
    let project = GitRepo::new();
    run_git(&["branch", "factory/delivery-worker"], &project.root);
    let commit_sha = git_stdout(&project.root, &["rev-parse", "main"]);
    let mut env = test_env();
    let cas_root = init_cas_dir(&project.root, &mut env).expect("init direct-close CAS");
    let task_store = open_task_store(&cas_root).expect("task store");
    let task_id = format!("direct-close-{state}");
    let mut task = Task::new(task_id.clone(), format!("Direct close in {state}"));
    task.status = TaskStatus::InProgress;
    task.depth = TaskDepth::Light;
    task.assignee = Some("delivery-worker".to_string());
    if state == WorkerDeliveryState::Delivered {
        task.deliverables.factory_branch_anchor = Some(commit_sha.clone());
    }
    task_store.add(&task).expect("add direct-close task");
    register_delivery_agent(
        &cas_root,
        "delivery-worker-session",
        "delivery-worker",
        AgentRole::Worker,
        "direct-close-factory",
    );
    let agent_store = open_agent_store(&cas_root).expect("agent store");
    agent_store
        .try_claim(
            &task.id,
            "delivery-worker-session",
            600,
            Some("direct-close fixture"),
        )
        .expect("seed active task lease");
    seed_direct_close_delivery(&cas_root, &task.id, state, &commit_sha);

    let task_before = task_store.get(&task.id).expect("task before close");
    let snapshot_before = durable_close_snapshot(&cas_root);
    let core = CasCore::with_daemon(cas_root.clone(), None, None);
    core.set_agent_id_for_testing("delivery-worker-session".to_string());
    let result = core
        .cas_task_update(Parameters(close_update_request(task.id.clone())))
        .await;

    if !expect_close {
        let text = result
            .expect_err("non-Delivered direct close must fail")
            .message
            .to_string();
        assert!(
            text.contains("DELIVERY CLOSE BLOCKED")
                && text.contains(&state.to_string())
                && text.contains("Remediation:"),
            "non-Delivered state must return typed actionable guidance:\n{text}"
        );
        assert_eq!(
            durable_close_snapshot(&cas_root),
            snapshot_before,
            "rejected direct close in {state} mutated task, receipt/transaction/events, dispatch/verdict, hook, lease, or lifecycle outbox"
        );
        return;
    }

    let text = get_text(&result.expect("Delivered direct close response"));
    assert!(text.contains("Updated task"), "{text}");
    let task_after = task_store.get(&task.id).expect("closed task");
    assert_eq!(task_after.status, TaskStatus::Closed);
    assert_eq!(
        serde_json::to_value(&task_after.deliverables).unwrap(),
        serde_json::to_value(&task_before.deliverables).unwrap()
    );
    assert_eq!(task_after.assignee, task_before.assignee);
    let snapshot_after = durable_close_snapshot(&cas_root);
    for (table, before_rows) in &snapshot_before {
        if matches!(table.as_str(), "tasks" | "events") {
            continue;
        }
        let after_rows = snapshot_after
            .iter()
            .find_map(|(after_table, rows)| (after_table == table).then_some(rows))
            .unwrap();
        assert_eq!(
            after_rows, before_rows,
            "Delivered direct close unexpectedly mutated {table}"
        );
    }
    let events_before = snapshot_before
        .iter()
        .find_map(|(table, rows)| (table == "events").then_some(rows))
        .unwrap();
    let events_after = snapshot_after
        .iter()
        .find_map(|(table, rows)| (table == "events").then_some(rows))
        .unwrap();
    assert_eq!(events_after.len(), events_before.len() + 1);
    assert!(
        events_after
            .last()
            .unwrap()
            .iter()
            .any(|value| value == "t:task_completed"),
        "Delivered direct close must emit only the normal task completion event"
    );
    let (_, transaction) = cas_store::get_latest_worker_delivery(&cas_root, &task.id)
        .unwrap()
        .unwrap();
    assert_eq!(transaction.state, WorkerDeliveryState::Delivered);
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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");

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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");

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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
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

/// Scoped override of one env var, restored on drop (including unwind).
///
/// The `_env` witness is the binary's canonical `TestEnvGuard`: it proves the
/// caller holds the process-wide env lock for the whole test body, so the
/// otherwise-unlocked mutation below cannot race a concurrently running test.
/// Never construct one without it.
struct VarGuard {
    key: &'static str,
    original: Option<std::ffi::OsString>,
}

impl VarGuard {
    fn set(_env: &TestEnvGuard, key: &'static str, value: &str) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: `_env` holds the process-wide test env lock until after this
        // guard's Drop has restored the original value.
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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);

    let wt_path = cas_root.join("worktrees").join("alice");
    repo.add_worktree(&wt_path, "factory/alice");

    // Give the worker branch real content to merge, not just an empty commit.
    std::fs::write(wt_path.join("alice-work.txt"), "alice's work").unwrap();
    run_git(&["add", "."], &wt_path);
    run_git(&["commit", "-m", "alice work"], &wt_path);

    env.set_current_dir(&repo.root);

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
    assert!(
        text.contains("Merge policy: merge proceeded because CI is advisory."),
        "successful worktree_merge must state the advisory CI policy.\nGot:\n{text}"
    );
    assert!(
        text.contains("gh endpoint queried:") && text.contains("CI SHA:"),
        "worktree_merge CI diagnostics must name the endpoint and source SHA.\nGot:\n{text}"
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
    let mut env = test_env();
    let home = TempDir::new().expect("temp HOME");
    env.set("HOME", home.path());

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
    let cas_root_a = init_cas_dir(&repo_a.root, &mut env).expect("init repo A CAS");
    let cas_root_b = init_cas_dir(&repo_b.root, &mut env).expect("init repo B CAS");
    disable_system_a(&cas_root_a);

    let task_store = open_task_store(&cas_root_a).expect("open repo A task store");
    let mut task = Task::new(
        "cross-repo-task".to_string(),
        "Cross repo merge".to_string(),
    );
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
    env.set_current_dir(&repo_a.root);

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
async fn public_create_update_and_close_reuse_duplicate_selector_binding() {
    let mut env = test_env();
    let home = TempDir::new().expect("temp HOME");
    env.set("HOME", home.path());
    let repo_a = GitRepo::new();
    let repo_b = GitRepo::new();
    for repo in [&repo_a, &repo_b] {
        run_git(
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:org/shared-lifecycle.git",
            ],
            &repo.root,
        );
    }
    let cas_root_a = init_cas_dir(&repo_a.root, &mut env).expect("init clone A CAS");
    init_cas_dir(&repo_b.root, &mut env).expect("init clone B CAS");
    let store = cas::store::known_repos::open_host_known_repo_store().unwrap();
    store
        .bind(
            "remote:github.com/org/shared-lifecycle",
            &repo_b.root,
            &repo_b.root.join(".git").canonicalize().unwrap(),
        )
        .unwrap();
    drop(store);

    let svc = make_service(cas_root_a.clone());
    let create = svc
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "Bound lifecycle",
            "depth": "light",
            "target_repo": repo_b.root,
            "target_branch": "main",
        }))))
        .await
        .expect("public create with explicit target");
    let create_text = get_text(&create);
    let task_id = create_text
        .split("Created task: ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .expect("created task id")
        .to_string();

    let update = svc
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": task_id,
            "title": "Bound lifecycle updated",
        }))))
        .await
        .expect("public update resolves existing binding");
    assert!(
        get_text(&update).contains("Updated task"),
        "{}",
        get_text(&update)
    );

    std::fs::write(repo_b.root.join("lifecycle-proof.txt"), "bound clone B").unwrap();
    run_git(&["add", "lifecycle-proof.txt"], &repo_b.root);
    run_git(&["commit", "-m", "bound lifecycle proof"], &repo_b.root);
    let task_store = open_task_store(&cas_root_a).unwrap();
    let mut task = task_store.get(&task_id).unwrap();
    task.status = TaskStatus::InProgress;
    task.deliverables.factory_branch_anchor =
        Some(git_stdout(&repo_b.root, &["rev-parse", "HEAD"]));
    task_store.update(&task).unwrap();

    let close = svc
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": task_id,
            "reason": "binding lifecycle proof",
        }))))
        .await
        .expect("public close resolves existing binding");
    let close_text = get_text(&close);
    assert!(
        close_text.contains("Closed task"),
        "normal close must resolve the intended bound clone: {close_text}"
    );
    assert!(!close_text.contains("AMBIGUOUS WORK TARGET"));
    assert!(!close_text.contains(home.path().to_string_lossy().as_ref()));

    let persisted = task_store.get(&task_id).unwrap();
    let portable = serde_json::to_string(&persisted.deliverables).unwrap();
    assert!(!portable.contains(home.path().to_string_lossy().as_ref()));
    assert!(!portable.contains("repo_root"));
    assert!(!portable.contains("git_common_dir"));
}

#[tokio::test]
async fn task_bound_merge_uses_persisted_binding_for_duplicate_live_clones() {
    let mut env = test_env();
    let home = TempDir::new().expect("temp HOME");
    env.set("HOME", home.path());

    let repo_a = GitRepo::new();
    let repo_b = GitRepo::new();
    for repo in [&repo_a, &repo_b] {
        run_git(
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:org/shared-work.git",
            ],
            &repo.root,
        );
    }
    let cas_root_a = init_cas_dir(&repo_a.root, &mut env).expect("init clone A CAS");
    let cas_root_b = init_cas_dir(&repo_b.root, &mut env).expect("init clone B CAS");
    disable_system_a(&cas_root_a);

    let store = cas::store::known_repos::open_host_known_repo_store().unwrap();
    let common_b = repo_b.root.join(".git").canonicalize().unwrap();
    store
        .bind("remote:github.com/org/shared-work", &repo_b.root, &common_b)
        .unwrap();
    drop(store);

    let task_store = open_task_store(&cas_root_a).expect("open clone A task store");
    let mut task = Task::new(
        "duplicate-selector-task".to_string(),
        "Bound duplicate selector merge".to_string(),
    );
    task.assignee = Some("alice".to_string());
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/org/shared-work".to_string(),
        target_branch: "main".to_string(),
    });
    task_store.add(&task).expect("add task binding");

    let wt_path = cas_root_b.join("worktrees").join("alice");
    repo_b.add_worktree(&wt_path, "factory/alice");
    std::fs::write(wt_path.join("bound-repo-work.txt"), "clone B only").unwrap();
    run_git(&["add", "bound-repo-work.txt"], &wt_path);
    run_git(&["commit", "-m", "bound clone B work"], &wt_path);

    let a_head_before = git_stdout(&repo_a.root, &["rev-parse", "HEAD"]);
    let a_index_before = git_stdout(&repo_a.root, &["write-tree"]);
    let a_status_before = git_stdout(&repo_a.root, &["status", "--porcelain=v1"]);
    env.set_current_dir(&repo_a.root);

    // Constructing a fresh service after the binding write models process
    // restart: resolution must come from host persistence, never cwd/recency.
    let svc = make_service(cas_root_a);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/alice".to_string());
    req.task_id = Some(task.id);
    req.allow_trunk = Some(true);
    req.cleanup = Some(false);
    let result = svc
        .coordination(Parameters(req))
        .await
        .expect("task-bound duplicate-selector merge call");
    let text = get_text(&result);
    assert!(
        text.contains("Merged worktree") && text.contains("task WorkTarget"),
        "public manager construction and merge must use the persisted binding.\nGot:\n{text}"
    );

    assert!(repo_b.root.join("bound-repo-work.txt").exists());
    assert!(!repo_a.root.join("bound-repo-work.txt").exists());
    assert_eq!(
        git_stdout(&repo_a.root, &["rev-parse", "HEAD"]),
        a_head_before
    );
    assert_eq!(git_stdout(&repo_a.root, &["write-tree"]), a_index_before);
    assert_eq!(
        git_stdout(&repo_a.root, &["status", "--porcelain=v1"]),
        a_status_before
    );
}

#[tokio::test]
async fn task_bound_system_a_merge_rejects_same_selector_worktree_from_wrong_clone() {
    let mut env = test_env();
    let home = TempDir::new().expect("temp HOME");
    env.set("HOME", home.path());

    let repo_a = GitRepo::new();
    let repo_b = GitRepo::new();
    for repo in [&repo_a, &repo_b] {
        run_git(
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:org/shared-system-a.git",
            ],
            &repo.root,
        );
    }
    let cas_root_a = init_cas_dir(&repo_a.root, &mut env).expect("init clone A CAS");
    let cas_root_b = init_cas_dir(&repo_b.root, &mut env).expect("init clone B CAS");

    let selector = "remote:github.com/org/shared-system-a";
    let known_repos = cas::store::known_repos::open_host_known_repo_store().unwrap();
    known_repos
        .bind(
            selector,
            &repo_b.root,
            &repo_b.root.join(".git").canonicalize().unwrap(),
        )
        .unwrap();
    drop(known_repos);

    let wrong_path = cas_root_a.join("worktrees").join("wrong-alice");
    repo_a.add_worktree(&wrong_path, "factory/alice");
    std::fs::write(wrong_path.join("clone-a-only.txt"), "wrong clone").unwrap();
    run_git(&["add", "clone-a-only.txt"], &wrong_path);
    run_git(&["commit", "-m", "wrong clone work"], &wrong_path);

    let intended_path = cas_root_b.join("worktrees").join("alice");
    repo_b.add_worktree(&intended_path, "factory/alice");
    std::fs::write(
        intended_path.join("clone-b-only.txt"),
        "declared clone work",
    )
    .unwrap();
    run_git(&["add", "clone-b-only.txt"], &intended_path);
    run_git(&["commit", "-m", "declared clone work"], &intended_path);

    let worktree_store = open_worktree_store(&cas_root_a).expect("worktree store");
    worktree_store.init().expect("initialize worktree store");
    let worktree_id = "system-a-wrong-clone".to_string();
    let wrong_worktree = Worktree::new(
        worktree_id.clone(),
        "factory/alice".to_string(),
        "main".to_string(),
        wrong_path.clone(),
    );
    worktree_store
        .add(&wrong_worktree)
        .expect("record wrong-clone System-A worktree");

    let task_store = open_task_store(&cas_root_a).expect("task store");
    let mut task = Task::new(
        "same-selector-wrong-clone".to_string(),
        "Reject wrong clone before merge".to_string(),
    );
    task.assignee = Some("alice".to_string());
    task.worktree_id = Some(worktree_id.clone());
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: selector.to_string(),
        target_branch: "main".to_string(),
    });
    task_store.add(&task).expect("add bound task");

    let repo_snapshot = |repo: &Path| {
        (
            git_stdout(repo, &["rev-parse", "HEAD"]),
            git_stdout(repo, &["write-tree"]),
            git_stdout(repo, &["status", "--porcelain=v1"]),
            git_stdout(repo, &["worktree", "list", "--porcelain"]),
        )
    };
    let a_before = repo_snapshot(&repo_a.root);
    let b_before = repo_snapshot(&repo_b.root);
    let wrong_before = repo_snapshot(&wrong_path);
    let intended_before = repo_snapshot(&intended_path);
    let task_before = serde_json::to_value(task_store.get(&task.id).unwrap()).unwrap();
    let worktree_before = serde_json::to_value(worktree_store.get(&worktree_id).unwrap()).unwrap();
    assert!(
        cas_store::get_latest_worker_delivery(&cas_root_a, &task.id)
            .unwrap()
            .is_none()
    );

    let svc = make_service(cas_root_a.clone());
    let mut req = coord_req("worktree_merge");
    req.id = Some(worktree_id.clone());
    req.task_id = Some(task.id.clone());
    req.allow_trunk = Some(true);
    req.force = Some(true);
    req.cleanup = Some(true);
    let error = svc
        .coordination(Parameters(req))
        .await
        .expect_err("same-selector wrong-clone worktree must fail closed")
        .to_string();
    assert!(
        error.contains("WORKTREE REPOSITORY MISMATCH")
            && error.contains("before merge/reachability checks"),
        "wrong clone must be rejected at the identity boundary, got:\n{error}"
    );

    assert_eq!(repo_snapshot(&repo_a.root), a_before, "repo A changed");
    assert_eq!(repo_snapshot(&repo_b.root), b_before, "repo B changed");
    assert_eq!(
        repo_snapshot(&wrong_path),
        wrong_before,
        "wrong-clone worker worktree changed"
    );
    assert_eq!(
        repo_snapshot(&intended_path),
        intended_before,
        "declared-clone worker worktree changed"
    );
    assert!(wrong_path.exists());
    assert!(intended_path.exists());
    assert_eq!(
        serde_json::to_value(task_store.get(&task.id).unwrap()).unwrap(),
        task_before,
        "task changed before rejection"
    );
    assert_eq!(
        serde_json::to_value(worktree_store.get(&worktree_id).unwrap()).unwrap(),
        worktree_before,
        "System-A record changed before rejection"
    );
    assert!(
        cas_store::get_latest_worker_delivery(&cas_root_a, &task.id)
            .unwrap()
            .is_none(),
        "rejection created delivery state"
    );
    let portable = serde_json::to_string(&task_store.get(&task.id).unwrap().deliverables).unwrap();
    assert!(!portable.contains(home.path().to_string_lossy().as_ref()));
    assert!(!portable.contains("repo_root"));
    assert!(!portable.contains("git_common_dir"));
}

#[tokio::test]
async fn update_to_closed_rejects_unmerged_task_without_mutation_then_closes_after_merge() {
    let mut env = test_env();
    let home = TempDir::new().expect("temp HOME");
    env.set("HOME", home.path());
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
    let cas_root_a = init_cas_dir(&repo_a.root, &mut env).expect("init repo A CAS");
    let cas_root_b = init_cas_dir(&repo_b.root, &mut env).expect("init repo B CAS");

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
    run_git(
        &["commit", "-m", "newer unrelated backend work"],
        &unrelated_path,
    );
    std::fs::write(unrelated_path.join("dirty.tmp"), "newest dirty worktree").unwrap();

    let task_store = open_task_store(&cas_root_a).expect("task store");
    let mut task = Task::new("close-via-update".to_string(), "Frontend task".to_string());
    task.depth = TaskDepth::Light;
    task.assignee = Some("frontend".to_string());
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/org/work-b".to_string(),
        target_branch: "main".to_string(),
    });
    task_store.add(&task).expect("add task");

    let task_before = serde_json::to_value(task_store.get(&task.id).unwrap()).unwrap();
    let durable_counts = || {
        let conn = rusqlite::Connection::open(cas_root_a.join("cas.db")).unwrap();
        [
            "worker_completion_receipts",
            "worker_delivery_transactions",
            "worker_delivery_events",
            "verification_dispatches",
            "verifications",
            "events",
            "supervisor_queue",
        ]
        .map(|table| {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
        })
    };
    let counts_before = durable_counts();
    let core = CasCore::with_daemon(cas_root_a.clone(), None, None);
    let rejected = core
        .cas_task_update(Parameters(close_update_request(task.id.clone())))
        .await
        .expect("merge-gated update response");
    let rejected_text = get_text(&rejected);
    assert!(
        rejected_text.contains("MERGE REQUIRED"),
        "update-to-closed must enforce the normal task merge gate:\n{rejected_text}"
    );
    assert_eq!(
        serde_json::to_value(task_store.get(&task.id).unwrap()).unwrap(),
        task_before,
        "failed direct close must not park, stamp hook evidence, change status/anchor, or update timestamps"
    );
    assert_eq!(
        durable_counts(),
        counts_before,
        "failed direct close must not create delivery, verification, event, or lifecycle-outbox state"
    );

    run_git(&["merge", "--no-ff", "factory/frontend"], &repo_b.root);
    core.cas_task_update(Parameters(close_update_request(task.id.clone())))
        .await
        .expect("merged update-to-closed must use frontend context");

    let persisted = task_store.get(&task.id).expect("persisted task");
    assert_eq!(persisted.status, cas::types::TaskStatus::Closed);
    let evidence = persisted
        .deliverables
        .pre_close_hook
        .as_ref()
        .expect("portable hook evidence");
    assert_eq!(evidence.repo_selector, "remote:github.com/org/work-b");
    assert_eq!(
        evidence.worktree_branch.as_deref(),
        Some("factory/frontend")
    );
    assert_eq!(evidence.task_tip.as_deref(), Some(own_tip.as_str()));
    let json = serde_json::to_string(&persisted.deliverables).unwrap();
    assert!(!json.contains(home.path().to_string_lossy().as_ref()));
    assert!(!json.contains(unrelated_path.to_string_lossy().as_ref()));
}

#[tokio::test]
async fn update_to_closed_preserves_legacy_non_git_non_factory_path() {
    let mut env = test_env();
    let home = TempDir::new().expect("temp HOME");
    env.set("HOME", home.path());
    let project = TempDir::new().expect("non-git project");
    let cas_root = init_cas_dir(project.path(), &mut env).expect("init non-git CAS");
    let task_store = open_task_store(&cas_root).expect("task store");
    let mut task = Task::new(
        "legacy-update-close".to_string(),
        "Legacy non-git close".to_string(),
    );
    task.depth = TaskDepth::Light;
    task.assignee = Some("legacy-worker".to_string());
    task_store.add(&task).expect("add legacy task");

    let core = CasCore::with_daemon(cas_root, None, None);
    let result = core
        .cas_task_update(Parameters(close_update_request(task.id.clone())))
        .await
        .expect("legacy direct close");
    assert!(
        get_text(&result).contains("Updated task"),
        "{}",
        get_text(&result)
    );
    let persisted = task_store.get(&task.id).expect("persisted legacy task");
    assert_eq!(persisted.status, TaskStatus::Closed);
    assert!(
        persisted.deliverables.pre_close_hook.is_none(),
        "legacy task without WorkTarget keeps the existing no-hook path"
    );
}

#[tokio::test]
async fn update_to_closed_rejects_every_incomplete_or_failed_delivery_without_mutation() {
    for state in [
        WorkerDeliveryState::AwaitingVerification,
        WorkerDeliveryState::CloseReady,
        WorkerDeliveryState::VerificationFailed,
    ] {
        exercise_direct_close_delivery_state(state, false).await;
    }
}

#[tokio::test]
async fn update_to_closed_accepts_delivered_transaction_without_replaying_delivery() {
    exercise_direct_close_delivery_state(WorkerDeliveryState::Delivered, true).await;
}

#[tokio::test]
async fn update_to_closed_fails_closed_when_declared_task_worktree_branch_is_ambiguous() {
    let mut env = test_env();
    let home = TempDir::new().expect("temp HOME");
    env.set("HOME", home.path());
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
    let cas_root_a = init_cas_dir(&repo_a.root, &mut env).expect("init repo A CAS");
    let cas_root_b = init_cas_dir(&repo_b.root, &mut env).expect("init repo B CAS");

    let misleading_path = cas_root_b.join("worktrees").join("frontend");
    repo_b.add_worktree(&misleading_path, "factory/other-worker");

    let task_store = open_task_store(&cas_root_a).expect("task store");
    let mut task = Task::new("ambiguous-close".to_string(), "Frontend task".to_string());
    task.depth = TaskDepth::Light;
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
    let mut env = test_env();
    let home = TempDir::new().expect("temp HOME");
    env.set("HOME", home.path());
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
    let cas_root_a = init_cas_dir(&repo_a.root, &mut env).expect("init repo A CAS");
    let cas_root_b = init_cas_dir(&repo_b.root, &mut env).expect("init repo B CAS");

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
    task.depth = TaskDepth::Light;
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
async fn normal_close_uses_task_anchor_not_newer_same_worker_or_unrelated_worktree() {
    let mut env = test_env();
    let home = TempDir::new().expect("temp HOME");
    env.set("HOME", home.path());
    let _role = VarGuard::set(&env, "CAS_AGENT_ROLE", "worker");
    let _factory = VarGuard::set(&env, "CAS_FACTORY_MODE", "1");
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
    let cas_root_a = init_cas_dir(&repo_a.root, &mut env).expect("init repo A CAS");
    let cas_root_b = init_cas_dir(&repo_b.root, &mut env).expect("init repo B CAS");
    std::fs::write(
        cas_root_a.join("config.toml"),
        "[verification]\nenabled = false\n",
    )
    .expect("disable verification for the ancestry-gate fixture");

    let own_path = cas_root_b.join("worktrees").join("frontend");
    repo_b.add_worktree(&own_path, "factory/frontend");
    std::fs::write(own_path.join("task-a.rs"), "pub fn task_a() {}\n").unwrap();
    run_git(&["add", "task-a.rs"], &own_path);
    run_git(&["commit", "-m", "task A"], &own_path);
    let task_a_tip = git_stdout(&own_path, &["rev-parse", "HEAD"]);
    run_git(
        &["merge", "--no-ff", "factory/frontend", "-m", "merge task A"],
        &repo_b.root,
    );

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

    // A real worker reaches close with a registered SessionStart identity.
    // Seed that identity explicitly so a clean CI process with no ambient
    // CAS_SESSION_ID exercises close/lint behavior instead of failing during
    // agent lookup (the same rebuilt-CasCore fixture rule as cas-48e6).
    let agent_id = register_worker_agent(&cas_root_a, "frontend", None);
    let core = CasCore::with_daemon(cas_root_a, None, None);
    core.set_agent_id_for_testing(agent_id);
    let result = core
        .cas_task_close(Parameters(TaskCloseRequest {
            stranded_branch_override: None,
            id: task.id.clone(),
            reason: Some("task A complete".to_string()),
            supervisor_override: None,
            legacy_bypass_code_review: None,
            search_manifest: None,
            commit_receipt: None,
        }))
        .await
        .expect("normal close call");
    let text = get_text(&result);
    assert!(
        text.contains("Closed task: normal-close"),
        "the task-owned anchor must allow A to close without queueing a review for unrelated B: {text}"
    );
    let persisted = task_store.get(&task.id).expect("persisted task");
    assert_eq!(
        persisted.status,
        cas::types::TaskStatus::Closed,
        "the task-owned anchor must leave task A closed after its delivery is integrated"
    );
}

/// Negative case: when neither System A nor System B has a matching
/// worktree, `worktree_merge` must report an accurate "not found" — never
/// silently succeed, and never fall back to the misleading "disabled"
/// message (that message implies the feature is off, not that the target
/// doesn't exist).
#[tokio::test]
async fn test_worktree_merge_reports_not_found_not_disabled_when_nothing_matches() {
    let repo = GitRepo::new();
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);
    // No worktree created for "bob" in either system.

    env.set_current_dir(&repo.root);

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
fn register_worker_agent(cas_root: &Path, name: &str, factory_session: Option<&str>) -> String {
    let agent_store = open_agent_store(cas_root).expect("open_agent_store");
    let id = Agent::generate_fallback_id();
    let mut agent = Agent::new(id.clone(), name.to_string());
    agent.agent_type = AgentType::Worker;
    agent.role = AgentRole::Worker;
    agent.factory_session = factory_session.map(|s| s.to_string());
    agent_store.register(&agent).expect("register worker agent");
    id
}

#[tokio::test]
async fn test_worktree_merge_targets_epic_branch_when_task_id_given() {
    let repo = GitRepo::new();
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
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

    env.set_current_dir(&repo.root);

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
async fn test_worktree_merge_refuses_explicit_task_whose_parent_epic_is_closed() {
    let repo = GitRepo::new();
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);

    Command::new("git")
        .args(["branch", "epic/already-closed"])
        .current_dir(&repo.root)
        .output()
        .unwrap();
    let (epic_id, worker_task_id) =
        create_epic_and_worker_task(&cas_root, "epic/already-closed", Some("closed-worker"));
    let task_store = open_task_store(&cas_root).expect("open_task_store");
    let mut epic = task_store.get(&epic_id).expect("get epic");
    epic.status = TaskStatus::Closed;
    task_store.update(&epic).expect("close epic");

    let wt_path = cas_root.join("worktrees").join("closed-worker");
    repo.add_worktree(&wt_path, "factory/closed-worker");
    std::fs::write(wt_path.join("late-work.txt"), "must not merge").unwrap();
    run_git(&["add", "."], &wt_path);
    run_git(&["commit", "-m", "late work"], &wt_path);

    env.set_current_dir(&repo.root);
    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/closed-worker".to_string());
    req.task_id = Some(worker_task_id);
    let result = svc.coordination(Parameters(req)).await;

    assert!(result.is_err(), "closed parent epic must reject the merge");
    let message = format!("{:?}", result.unwrap_err());
    assert!(
        message.contains("Closed") && message.contains("close receipt"),
        "refusal must explain closed-epic integrity. Got: {message}"
    );
    let epic_tree = Command::new("git")
        .args(["ls-tree", "-r", "--name-only", "epic/already-closed"])
        .current_dir(&repo.root)
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&epic_tree.stdout).contains("late-work.txt"),
        "closed epic branch must remain unchanged"
    );
}

#[tokio::test]
async fn standalone_non_trunk_work_target_does_not_require_allow_trunk_cas_84df() {
    let repo = GitRepo::new();
    let mut env = test_env();
    run_git(
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:org/staging-target.git",
        ],
        &repo.root,
    );
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);
    run_git(&["branch", "staging"], &repo.root);

    let task_store = open_task_store(&cas_root).expect("open_task_store");
    let mut standalone_task = Task::new(
        "standalone-staging".to_string(),
        "Standalone staging task".to_string(),
    );
    standalone_task.assignee = Some("alice".to_string());
    standalone_task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/org/staging-target".to_string(),
        target_branch: "staging".to_string(),
    });
    task_store
        .add(&standalone_task)
        .expect("add standalone task");

    let wt_path = cas_root.join("worktrees").join("alice");
    repo.add_worktree(&wt_path, "factory/alice");
    std::fs::write(wt_path.join("staging-only.txt"), "staging delivery").unwrap();
    run_git(&["add", "staging-only.txt"], &wt_path);
    run_git(&["commit", "-m", "staging delivery"], &wt_path);

    env.set_current_dir(&repo.root);

    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/alice".to_string());
    req.task_id = Some(standalone_task.id);
    req.cleanup = Some(false);
    let result = svc
        .coordination(Parameters(req))
        .await
        .expect("a declared non-trunk WorkTarget must not require allow_trunk");
    let text = get_text(&result);
    assert!(
        text.contains("to staging") && text.contains("task WorkTarget"),
        "success must name the declared staging destination:\n{text}"
    );
    assert!(
        git_stdout(&repo.root, &["ls-tree", "-r", "--name-only", "staging"])
            .contains("staging-only.txt"),
        "the WorkTarget branch must receive the standalone task"
    );
    assert!(
        !git_stdout(&repo.root, &["ls-tree", "-r", "--name-only", "main"])
            .contains("staging-only.txt"),
        "main must remain untouched"
    );
}

#[tokio::test]
async fn missing_work_target_refusal_names_resolved_trunk_cas_84df() {
    let repo = GitRepo::new();
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);

    let task_store = open_task_store(&cas_root).expect("open_task_store");
    let mut standalone_task = Task::new(
        "standalone-missing-target".to_string(),
        "Standalone task missing WorkTarget".to_string(),
    );
    standalone_task.assignee = Some("bob".to_string());
    task_store
        .add(&standalone_task)
        .expect("add standalone task");

    let wt_path = cas_root.join("worktrees").join("bob");
    repo.add_worktree(&wt_path, "factory/bob");
    env.set_current_dir(&repo.root);

    let svc = make_service(cas_root);

    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/bob".to_string());
    req.task_id = Some(standalone_task.id);
    let refused = svc
        .coordination(Parameters(req))
        .await
        .expect_err("missing WorkTarget must require explicit trunk authorization");
    assert!(
        refused.message.contains("would merge to: main"),
        "refusal must reveal the exact fallback destination before authorization:\n{}",
        refused.message
    );
}

#[tokio::test]
async fn authorized_trunk_fallback_push_is_loud_cas_84df() {
    let repo = GitRepo::new();
    let remote = TempDir::new().expect("bare origin tempdir");
    let output = Command::new("git")
        .args(["init", "--bare", "--initial-branch=main"])
        .arg(remote.path())
        .output()
        .expect("initialize bare origin");
    assert!(
        output.status.success(),
        "git init --bare failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    run_git(
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
        &repo.root,
    );
    run_git(&["push", "-u", "origin", "main"], &repo.root);

    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);

    let task_store = open_task_store(&cas_root).expect("open_task_store");
    let mut standalone_task = Task::new(
        "standalone-trunk".to_string(),
        "Explicit trunk fallback".to_string(),
    );
    standalone_task.assignee = Some("bob".to_string());
    task_store
        .add(&standalone_task)
        .expect("add standalone task");

    let wt_path = cas_root.join("worktrees").join("bob");
    repo.add_worktree(&wt_path, "factory/bob");
    std::fs::write(wt_path.join("production.txt"), "published to trunk").unwrap();
    run_git(&["add", "production.txt"], &wt_path);
    run_git(&["commit", "-m", "production delivery"], &wt_path);
    env.set_current_dir(&repo.root);

    let svc = make_service(cas_root);

    let mut trunk_ok = coord_req("worktree_merge");
    trunk_ok.id = Some("factory/bob".to_string());
    trunk_ok.task_id = Some(standalone_task.id);
    trunk_ok.allow_trunk = Some(true);
    let result = svc
        .coordination(Parameters(trunk_ok))
        .await
        .expect("allow_trunk=true must authorize a genuine trunk fallback");
    let text = get_text(&result);
    assert!(
        text.contains("⚠️ TRUNK PUSH COMPLETE")
            && text.contains("allow_trunk=true")
            && text.contains("main"),
        "the destructive-adjacent trunk push must be unmistakable:\n{text}"
    );
    assert!(
        git_stdout(remote.path(), &["ls-tree", "-r", "--name-only", "main"])
            .contains("production.txt"),
        "origin/main must contain the authorized trunk delivery"
    );
}

#[tokio::test]
async fn test_worktree_merge_refuses_when_task_id_does_not_exist() {
    let repo = GitRepo::new();
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);

    let wt_path = cas_root.join("worktrees").join("carol");
    repo.add_worktree(&wt_path, "factory/carol");

    env.set_current_dir(&repo.root);

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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);

    let wt_path = cas_root.join("worktrees").join("dave");
    repo.add_worktree(&wt_path, "factory/dave");

    env.set_current_dir(&repo.root);

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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
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
    epic.branch =
        Some("epic/epic-triage-and-fix-jul-9-11-docs-requests-factory-cas-0e22".to_string());
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

    env.set_current_dir(&repo.root);

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

/// cas-b86e: a stale session focus is a TUI attention hint, never merge
/// authority. Reproduce the live incident: the worker finished a task in epic
/// A, was reassigned to a standalone task, epic A was closed, and the session
/// remained focused on A. Explicit `allow_trunk=true` must merge the worker's
/// new commits to trunk rather than contaminating the closed epic branch.
#[tokio::test]
async fn test_worktree_merge_reassignment_ignores_closed_focused_epic_and_honors_trunk() {
    let repo = GitRepo::new();
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);

    Command::new("git")
        .args(["branch", "epic/completed-a"])
        .current_dir(&repo.root)
        .output()
        .unwrap();

    let task_store = open_task_store(&cas_root).expect("open_task_store");
    let mut epic = Task::new("epic-a".to_string(), "Completed epic A".to_string());
    epic.task_type = TaskType::Epic;
    epic.branch = Some("epic/completed-a".to_string());
    task_store.add(&epic).expect("add epic A");

    let mut old_task = Task::new("task-in-a".to_string(), "Earlier epic task".to_string());
    old_task.assignee = Some("reassigned-worker".to_string());
    old_task.status = TaskStatus::Closed;
    task_store
        .create_atomic(&old_task, &[], Some(&epic.id), None)
        .expect("create completed task under epic A");

    epic.status = TaskStatus::Closed;
    task_store.update(&epic).expect("close epic A");

    let mut current_task = Task::new(
        "standalone-current".to_string(),
        "Current standalone task".to_string(),
    );
    current_task.assignee = Some("reassigned-worker".to_string());
    current_task.status = TaskStatus::InProgress;
    task_store
        .add(&current_task)
        .expect("add current standalone task");

    let session = "test-reassignment-focus-b86e";
    let home = TempDir::new().expect("home");
    env.set_current_dir(&repo.root);
    let _session_env = VarGuard::set(&env, "CAS_FACTORY_SESSION", session);
    env.set("HOME", home.path());
    let meta_path = cas::ui::factory::metadata_path(session);
    std::fs::create_dir_all(meta_path.parent().expect("metadata parent")).unwrap();
    let workers = vec!["reassigned-worker".to_string()];
    let mut meta = cas::ui::factory::create_metadata(
        session,
        1,
        "supervisor",
        &workers,
        None,
        Some(repo.root.to_str().unwrap()),
        None,
    );
    // Seed directly to model a focus that was valid before epic A closed.
    meta.pinned_epic_id = Some(epic.id.clone());
    std::fs::write(
        &meta_path,
        serde_json::to_string_pretty(&meta).expect("serialize metadata"),
    )
    .expect("write stale session focus");

    let wt_path = cas_root.join("worktrees").join("reassigned-worker");
    repo.add_worktree(&wt_path, "factory/reassigned-worker");
    std::fs::write(wt_path.join("new-task-work.txt"), "new task work").unwrap();
    run_git(&["add", "."], &wt_path);
    run_git(&["commit", "-m", "new standalone task work"], &wt_path);

    let svc = make_service(cas_root);
    let mut req = coord_req("worktree_merge");
    req.id = Some("factory/reassigned-worker".to_string());
    req.allow_trunk = Some(true);
    let result = svc.coordination(Parameters(req)).await;

    let text = get_text(&result.expect("allow_trunk merge should succeed"));
    assert!(
        text.contains("to main") && text.contains("allow_trunk=true"),
        "standalone task must resolve to explicitly-authorized trunk. Got:\n{text}"
    );
    assert!(
        text.contains(&current_task.id),
        "resolution reason must identify the current assignee task. Got:\n{text}"
    );

    let main_tree = Command::new("git")
        .args(["ls-tree", "-r", "--name-only", "main"])
        .current_dir(&repo.root)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&main_tree.stdout).contains("new-task-work.txt"),
        "trunk must contain the new task's commit"
    );
    let closed_epic_tree = Command::new("git")
        .args(["ls-tree", "-r", "--name-only", "epic/completed-a"])
        .current_dir(&repo.root)
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&closed_epic_tree.stdout).contains("new-task-work.txt"),
        "closed epic A must not receive the reassigned worker's new commits"
    );
}

/// cas-b86e: even a valid, open focused epic is not merge authority. Without
/// task/assignee binding or explicit trunk authorization, merge must refuse.
#[tokio::test]
async fn test_worktree_merge_ignores_focused_epic_without_task_authority() {
    let repo = GitRepo::new();
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
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
    env.set_current_dir(&repo.root);
    let _session_env = VarGuard::set(&env, "CAS_FACTORY_SESSION", session);
    env.set("HOME", home.path());
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

    let error = result.expect_err("focused epic alone must not authorize a merge");
    let text = format!("{error:?}");
    assert!(
        text.contains("Session focus is not merge authority")
            && text.contains("allow_trunk was not set"),
        "refusal must explain authoritative inputs. Got:\n{text}"
    );
}

/// cas-0b32 review P1: focused epic with mismatched project_dir is ignored
/// (cross-project / stale) — refuse silent trunk without allow_trunk.
#[tokio::test]
async fn test_worktree_merge_rejects_cross_project_focused_epic_cas_0b32() {
    let repo = GitRepo::new();
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
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
    env.set_current_dir(&repo.root);
    let _session_env = VarGuard::set(&env, "CAS_FACTORY_SESSION", session);
    env.set("HOME", home.path());
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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
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
    env.set_current_dir(&repo.root);
    let _session_env = VarGuard::set(&env, "CAS_FACTORY_SESSION", session);
    env.set("HOME", home.path());
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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
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

    env.set_current_dir(&repo.root);
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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
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

    env.set_current_dir(&repo.root);
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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
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

    env.set_current_dir(&repo.root);

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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    // Unique base_path under the temp parent so parallel/rerun tests don't
    // collide on a shared /tmp/custom-worktree-loc path. GH #704: owned by a
    // TempDir so the directory is removed when the test ends — the hand-named
    // variant leaked 63 `custom-wt-*` directories into the operator's tmpfs.
    let base_parent = repo.root.parent().expect("temp repo has a parent");
    let base_dir = tempfile::Builder::new()
        .prefix("custom-wt-")
        .tempdir_in(base_parent)
        .expect("create custom base_path under the temp parent");
    let unique = base_dir
        .path()
        .file_name()
        .expect("tempdir has a name")
        .to_string_lossy()
        .into_owned();
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

    env.set_current_dir(&repo.root);

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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);

    Command::new("git")
        .args(["branch", "epic/match"])
        .current_dir(&repo.root)
        .output()
        .unwrap();

    let (_epic_id, task_id) = create_epic_and_worker_task(&cas_root, "epic/match", Some("alice"));

    let wt_path = cas_root.join("worktrees").join("alice");
    repo.add_worktree(&wt_path, "factory/alice");
    std::fs::write(wt_path.join("match.txt"), "ok").unwrap();
    run_git(&["add", "."], &wt_path);
    run_git(&["commit", "-m", "match work"], &wt_path);

    env.set_current_dir(&repo.root);

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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
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

    env.set_current_dir(&repo.root);

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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);

    Command::new("git")
        .args(["branch", "epic/orphan"])
        .current_dir(&repo.root)
        .output()
        .unwrap();

    // Intentionally no assignee — pre-cas-bd5f this would still merge to epic.
    let (_epic_id, orphan_task_id) = create_epic_and_worker_task(&cas_root, "epic/orphan", None);

    let wt_path = cas_root.join("worktrees").join("carol");
    repo.add_worktree(&wt_path, "factory/carol");

    env.set_current_dir(&repo.root);

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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);

    Command::new("git")
        .args(["branch", "epic/xsession"])
        .current_dir(&repo.root)
        .output()
        .unwrap();

    // Task has no assignee field but an active lease held by bob in session-B.
    let (_epic_id, task_id) = create_epic_and_worker_task(&cas_root, "epic/xsession", None);

    let bob_id = register_worker_agent(&cas_root, "bob", Some("session-b"));
    let agent_store = open_agent_store(&cas_root).expect("open_agent_store");
    agent_store
        .try_claim(&task_id, &bob_id, 600, Some("bob owns this"))
        .expect("bob claims task");

    // alice is a separate session agent — name does not match bob's lease.
    let _alice_id = register_worker_agent(&cas_root, "alice", Some("session-a"));

    let wt_path = cas_root.join("worktrees").join("alice");
    repo.add_worktree(&wt_path, "factory/alice");

    env.set_current_dir(&repo.root);

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
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);

    Command::new("git")
        .args(["branch", "epic/lease-ok"])
        .current_dir(&repo.root)
        .output()
        .unwrap();

    let (_epic_id, task_id) = create_epic_and_worker_task(&cas_root, "epic/lease-ok", None);

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

    env.set_current_dir(&repo.root);

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
    agent_store
        .register(&agent)
        .expect("register delivery agent");
}

fn delivery_service(cas_root: &Path, agent_id: &str) -> CasService {
    let core = CasCore::with_daemon(cas_root.to_path_buf(), None, None);
    core.set_agent_id_for_testing(agent_id.to_string());
    CasService::new(core, None)
}

fn delivery_receipt(
    task_id: &str,
    worker_id: &str,
    repo: &GitRepo,
    worker: &str,
) -> WorkerCompletionReceiptInput {
    WorkerCompletionReceiptInput {
        task_id: task_id.to_string(),
        worker_agent_id: worker_id.to_string(),
        repo_selector: "remote:github.com/org/delivery".to_string(),
        source_branch: format!("factory/{worker}"),
        commit_sha: git_stdout(&repo.root, &["rev-parse", &format!("factory/{worker}")]),
        merge_base_sha: git_stdout(
            &repo.root,
            &["merge-base", &format!("factory/{worker}"), "main"],
        ),
        target_branch: "main".to_string(),
        target_sha: git_stdout(&repo.root, &["rev-parse", "main"]),
        proof_reference: "proof:serialized-workspace-1".to_string(),
        scope_summary: "transactional delivery integration".to_string(),
        artifact_path: None,
    }
}

async fn submit_and_verify_delivery(
    cas_root: &Path,
    task_id: &str,
    worker_id: &str,
    supervisor_id: &str,
    receipt: &WorkerCompletionReceiptInput,
) {
    let agent_store = open_agent_store(cas_root).expect("delivery agent store");
    if agent_store.get_lease(task_id).unwrap().is_none() {
        assert!(matches!(
            agent_store
                .try_claim(task_id, worker_id, 600, Some("delivery fixture lease"))
                .unwrap(),
            cas::types::ClaimResult::Success(_)
        ));
    }
    let lease = agent_store
        .get_lease(task_id)
        .expect("delivery fixture lease lookup")
        .expect("delivery fixture must retain its worker lease");
    let worker_service = delivery_service(cas_root, worker_id);
    let close = worker_service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": task_id,
            "reason": "worker handoff",
            "completion_receipt": serde_json::to_string(receipt).unwrap(),
        }))))
        .await
        .expect("public task close receipt submission");
    assert!(
        get_text(&close).contains("DELIVERY RECEIPT REJECTED")
            && get_text(&close).contains("current factory branch tip"),
        "{}",
        get_text(&close)
    );

    // Seed the state after the explicit #588 rejection so these tests retain
    // their coverage of the supervisor's merge/reconcile path. The public
    // receipt path cannot create this boundary until the source tip is merged.
    let worker = agent_store.get(worker_id).expect("delivery worker");
    let durable_receipt =
        cas_store::build_worker_completion_receipt(receipt, &worker.name, chrono::Utc::now());
    cas_store::create_worker_delivery_with_dispatch_for_lease(
        cas_root,
        &durable_receipt,
        WorkerDeliveryState::AwaitingVerification,
        worker_id,
        lease.epoch,
        supervisor_id,
        chrono::Utc::now() + chrono::Duration::minutes(10),
    )
    .expect("seed delivery boundary after explicit merge-gate rejection");
    let mut pending = open_task_store(cas_root)
        .expect("task store")
        .get(task_id)
        .expect("task after receipt rejection");
    pending.status = TaskStatus::AwaitingMerge;
    pending.pending_verification = true;
    pending.close_reason = Some("worker handoff".to_string());
    pending.deliverables.factory_branch_anchor = Some(receipt.commit_sha.clone());
    open_task_store(cas_root)
        .expect("task store")
        .update(&pending)
        .expect("project seeded delivery boundary as review-pending");
    agent_store
        .release_lease_if_owner_epoch(
            task_id,
            worker_id,
            lease.epoch,
            "Fixture receipt handoff after explicit merge-gate rejection",
        )
        .expect("release fixture lease");
    let dispatch = cas_store::get_latest_verification_dispatch(cas_root, task_id)
        .expect("verification dispatch lookup")
        .expect("receipt-bound verification dispatch");

    let supervisor_service = delivery_service(cas_root, supervisor_id);
    let verification = supervisor_service
        .verification(Parameters(verification_req(serde_json::json!({
            "action": "add",
            "task_id": task_id,
            "status": "approved",
            "summary": "fresh external delivery proof approved",
            "confidence": 1.0,
            "dispatch_id": dispatch.id,
        }))))
        .await
        .expect("public verification add");
    assert!(get_text(&verification).contains("approved"));
}

#[tokio::test]
async fn completion_receipt_authority_is_exact_active_lease_session() {
    let mut env = test_env();
    let home = TempDir::new().expect("temp HOME");
    env.set("HOME", home.path());
    let repo = GitRepo::new();
    run_git(
        &["remote", "add", "origin", "git@github.com:org/delivery.git"],
        &repo.root,
    );
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init CAS");
    disable_system_a(&cas_root);
    std::fs::write(
        cas_root.join("config.toml"),
        "[worktrees]\nenabled = false\n[verification]\nenabled = true\n",
    )
    .unwrap();

    let factory_session = "receipt-authority-factory";
    let owner_id = "receipt-owner-session";
    let duplicate_id = "receipt-duplicate-session";
    let wrong_id = "receipt-wrong-session";
    let dead_id = "receipt-dead-session";
    let unleased_id = "receipt-unleased-session";
    let supervisor_id = "receipt-supervisor-session";
    register_delivery_agent(
        &cas_root,
        owner_id,
        "alice",
        AgentRole::Worker,
        factory_session,
    );
    register_delivery_agent(
        &cas_root,
        duplicate_id,
        "alice",
        AgentRole::Worker,
        factory_session,
    );
    register_delivery_agent(
        &cas_root,
        wrong_id,
        "mallory",
        AgentRole::Worker,
        factory_session,
    );
    register_delivery_agent(
        &cas_root,
        dead_id,
        "alice",
        AgentRole::Worker,
        factory_session,
    );
    register_delivery_agent(
        &cas_root,
        unleased_id,
        "orphan",
        AgentRole::Worker,
        factory_session,
    );
    register_delivery_agent(
        &cas_root,
        supervisor_id,
        "supervisor",
        AgentRole::Supervisor,
        factory_session,
    );
    let worker_path = cas_root.join("worktrees").join("alice");
    repo.add_worktree(&worker_path, "factory/alice");
    std::fs::write(worker_path.join("authority.rs"), "pub fn authority() {}\n").unwrap();
    run_git(&["add", "authority.rs"], &worker_path);
    run_git(&["commit", "-m", "receipt authority fixture"], &worker_path);
    // This test exercises exact lease-session authority after receipt
    // validation. Keep the source tip in the already-merged/published state
    // required by the #588 decision-time ancestry and B2 reality gates.
    run_git(
        &[
            "merge",
            "--no-ff",
            "factory/alice",
            "-m",
            "merge receipt authority",
        ],
        &repo.root,
    );
    run_git(
        &[
            "update-ref",
            "refs/remotes/origin/factory/alice",
            "factory/alice",
        ],
        &repo.root,
    );

    let task_store = open_task_store(&cas_root).expect("task store");
    let agent_store = open_agent_store(&cas_root).expect("agent store");
    let mut task = Task::new(
        "cas-receipt-authority".to_string(),
        "Exact receipt lease authority".to_string(),
    );
    task.status = TaskStatus::InProgress;
    task.depth = TaskDepth::Deep;
    task.assignee = Some("alice".to_string());
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/org/delivery".to_string(),
        target_branch: "main".to_string(),
    });
    task_store.add(&task).expect("add receipt task");
    assert!(matches!(
        agent_store
            .try_claim(&task.id, owner_id, 600, Some("exact receipt owner"))
            .unwrap(),
        cas::types::ClaimResult::Success(_)
    ));

    let owner_receipt = delivery_receipt(&task.id, owner_id, &repo, "alice");
    for (caller_id, mut claimed_receipt, label) in [
        (
            duplicate_id,
            delivery_receipt(&task.id, duplicate_id, &repo, "alice"),
            "duplicate-name peer",
        ),
        (
            wrong_id,
            delivery_receipt(&task.id, wrong_id, &repo, "alice"),
            "wrong worker",
        ),
        (
            dead_id,
            delivery_receipt(&task.id, dead_id, &repo, "alice"),
            "dead worker",
        ),
    ] {
        // Keep every field otherwise production-valid. A self-declared worker
        // id is consistency metadata, never the authority source.
        claimed_receipt.worker_agent_id = caller_id.to_string();
        if caller_id == dead_id {
            open_agent_store(&cas_root)
                .unwrap()
                .mark_stale(dead_id)
                .expect("mark receipt caller stale before a fresh MCP request");
        }
        // Completion proof authentication is deliberately read-only: unlike
        // normal MCP activity, it must not revive a stale session.
        let service = delivery_service(&cas_root, caller_id);
        let before = durable_close_snapshot(&cas_root);
        let result = service
            .task(Parameters(task_req(serde_json::json!({
                "action": "close",
                "id": task.id,
                "reason": label,
                "completion_receipt": serde_json::to_string(&claimed_receipt).unwrap(),
            }))))
            .await
            .expect("authority rejection is a typed tool result");
        let result_text = get_text(&result);
        assert!(
            result_text.contains("DELIVERY RECEIPT REJECTED")
                && result_text.contains("exact active task lease"),
            "{label} needs lease-authority remediation, got:\n{result_text}"
        );
        assert_eq!(
            durable_close_snapshot(&cas_root),
            before,
            "{label} mutated task/agent/lease/delivery/dispatch/verdict/event/outbox state"
        );
    }

    let before_malformed = durable_close_snapshot(&cas_root);
    let malformed = delivery_service(&cas_root, duplicate_id)
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": task.id,
            "reason": "authority must precede receipt parsing",
            "completion_receipt": "not-json",
        }))))
        .await
        .expect("unauthorized malformed receipt returns a typed rejection");
    let malformed_text = get_text(&malformed);
    assert!(malformed_text.contains("exact active task lease"));
    assert!(!malformed_text.contains("valid WorkerCompletionReceiptInput JSON"));
    assert_eq!(durable_close_snapshot(&cas_root), before_malformed);

    let mut unleased_task = Task::new(
        "cas-receipt-unleased".to_string(),
        "Unleased receipt caller".to_string(),
    );
    unleased_task.status = TaskStatus::InProgress;
    unleased_task.depth = TaskDepth::Deep;
    unleased_task.assignee = Some("orphan".to_string());
    unleased_task.deliverables.work_target = task.deliverables.work_target.clone();
    task_store.add(&unleased_task).unwrap();
    let unleased_receipt = delivery_receipt(&unleased_task.id, unleased_id, &repo, "alice");
    let before_unleased = durable_close_snapshot(&cas_root);
    let unleased = delivery_service(&cas_root, unleased_id)
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": unleased_task.id,
            "reason": "no lease is not authority",
            "completion_receipt": serde_json::to_string(&unleased_receipt).unwrap(),
        }))))
        .await
        .expect("unleased receipt returns a typed rejection");
    assert!(get_text(&unleased).contains("exact active task lease"));
    assert_eq!(durable_close_snapshot(&cas_root), before_unleased);

    // A receipt may establish its immutable recovery boundary before task
    // projection, but it must never claim a clean handoff while the exact
    // worker lease is still active. Inject a real SQLite failure into the
    // public close path, then prove an exact retry reconciles without
    // duplicating delivery state or touching an unrelated lease.
    let mut release_failure_task = Task::new(
        "cas-receipt-release-failure".to_string(),
        "Receipt lease-release recovery".to_string(),
    );
    release_failure_task.status = TaskStatus::InProgress;
    release_failure_task.depth = TaskDepth::Deep;
    release_failure_task.assignee = Some("alice".to_string());
    release_failure_task.deliverables.work_target = task.deliverables.work_target.clone();
    task_store.add(&release_failure_task).unwrap();
    assert!(matches!(
        agent_store
            .try_claim(
                &release_failure_task.id,
                owner_id,
                600,
                Some("receipt release failure fixture"),
            )
            .unwrap(),
        cas::types::ClaimResult::Success(_)
    ));
    let unrelated_before = agent_store
        .get_lease(&task.id)
        .unwrap()
        .expect("unrelated original task lease");
    let release_failure_receipt =
        delivery_receipt(&release_failure_task.id, owner_id, &repo, "alice");
    {
        let conn = rusqlite::Connection::open(cas_root.join("cas.db")).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_completion_receipt_lease_release
             BEFORE UPDATE OF status ON task_leases
             WHEN OLD.task_id = 'cas-receipt-release-failure'
              AND OLD.status = 'active' AND NEW.status = 'released'
             BEGIN
               SELECT RAISE(ABORT, 'forced completion receipt lease release failure');
             END;",
        )
        .unwrap();
    }
    let release_failure = delivery_service(&cas_root, owner_id)
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": release_failure_task.id,
            "reason": "injected lease release failure",
            "completion_receipt": serde_json::to_string(&release_failure_receipt).unwrap(),
        }))))
        .await
        .expect("lease-release failure is a typed public result");
    let release_failure_text = get_text(&release_failure);
    assert!(
        release_failure_text.contains("DELIVERY RECEIPT HANDOFF INCOMPLETE")
            && release_failure_text.contains("lease remains active")
            && release_failure_text.contains("retry the exact same completion_receipt")
            && !release_failure_text.contains("accepted idempotently"),
        "failure must report an honest actionable recovery state:\n{release_failure_text}"
    );
    let failed_projection = task_store.get(&release_failure_task.id).unwrap();
    assert_eq!(failed_projection.status, TaskStatus::InProgress);
    assert!(!failed_projection.pending_verification);
    assert!(
        failed_projection
            .deliverables
            .factory_branch_anchor
            .is_none()
    );
    let active_failed_lease = agent_store
        .get_lease(&release_failure_task.id)
        .unwrap()
        .expect("failed cleanup must leave its exact lease visibly active");
    assert_eq!(active_failed_lease.agent_id, owner_id);
    let unrelated_after_failure = agent_store
        .get_lease(&task.id)
        .unwrap()
        .expect("unrelated lease survives failure");
    assert_eq!(unrelated_after_failure.agent_id, unrelated_before.agent_id);
    assert_eq!(unrelated_after_failure.epoch, unrelated_before.epoch);
    assert_eq!(
        unrelated_after_failure.expires_at,
        unrelated_before.expires_at
    );
    let (_, failed_transaction) =
        cas_store::get_latest_worker_delivery(&cas_root, &release_failure_task.id)
            .unwrap()
            .expect("immutable receipt boundary remains available for reconciliation");
    assert_eq!(
        failed_transaction.state,
        WorkerDeliveryState::AwaitingVerification
    );
    assert_eq!(
        cas_store::list_worker_delivery_events(&cas_root, &failed_transaction.id)
            .unwrap()
            .len(),
        1
    );

    {
        let conn = rusqlite::Connection::open(cas_root.join("cas.db")).unwrap();
        conn.execute_batch("DROP TRIGGER fail_completion_receipt_lease_release;")
            .unwrap();
    }
    let recovered = delivery_service(&cas_root, owner_id)
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": release_failure_task.id,
            "reason": "exact receipt cleanup retry",
            "completion_receipt": serde_json::to_string(&release_failure_receipt).unwrap(),
        }))))
        .await
        .expect("exact retry reconciles lease cleanup");
    assert!(
        get_text(&recovered).contains("Worker delivery receipt accepted idempotently"),
        "{}",
        get_text(&recovered)
    );
    assert!(
        agent_store
            .get_lease(&release_failure_task.id)
            .unwrap()
            .is_none(),
        "successful reconciliation releases the exact lease"
    );
    let recovered_task = task_store.get(&release_failure_task.id).unwrap();
    assert_eq!(recovered_task.status, TaskStatus::InProgress);
    assert!(recovered_task.pending_verification);
    let unrelated_after_recovery = agent_store
        .get_lease(&task.id)
        .unwrap()
        .expect("unrelated lease survives reconciliation");
    assert_eq!(unrelated_after_recovery.agent_id, unrelated_before.agent_id);
    assert_eq!(unrelated_after_recovery.epoch, unrelated_before.epoch);
    assert_eq!(
        unrelated_after_recovery.expires_at,
        unrelated_before.expires_at
    );
    assert_eq!(
        cas_store::list_worker_delivery_events(&cas_root, &failed_transaction.id)
            .unwrap()
            .len(),
        1,
        "reconciliation must not duplicate the delivery event"
    );
    let conn = rusqlite::Connection::open(cas_root.join("cas.db")).unwrap();
    let boundary_counts: (i64, i64, i64) = conn
        .query_row(
            "SELECT
               (SELECT COUNT(*) FROM worker_completion_receipts WHERE task_id = ?1),
               (SELECT COUNT(*) FROM worker_delivery_transactions WHERE task_id = ?1),
               (SELECT COUNT(*) FROM verification_dispatches WHERE task_id = ?1)",
            [&release_failure_task.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        boundary_counts,
        (1, 1, 1),
        "reconciliation must not duplicate receipt, delivery, or dispatch"
    );

    let owner_service = delivery_service(&cas_root, owner_id);
    let concurrent_owner_service = delivery_service(&cas_root, owner_id);
    let accepted_a = owner_service.task(Parameters(task_req(serde_json::json!({
        "action": "close",
        "id": task.id,
        "reason": "concurrent exact lease owner handoff a",
        "completion_receipt": serde_json::to_string(&owner_receipt).unwrap(),
    }))));
    let accepted_b = concurrent_owner_service.task(Parameters(task_req(serde_json::json!({
        "action": "close",
        "id": task.id,
        "reason": "concurrent exact lease owner handoff b",
        "completion_receipt": serde_json::to_string(&owner_receipt).unwrap(),
    }))));
    let (accepted_a, accepted_b) = tokio::join!(accepted_a, accepted_b);
    let accepted_a = accepted_a.expect("first concurrent exact lease owner receipt");
    let accepted_b = accepted_b.expect("second concurrent exact lease owner receipt");
    for accepted in [&accepted_a, &accepted_b] {
        assert!(
            get_text(accepted).contains("Worker delivery receipt accepted idempotently"),
            "{}",
            get_text(accepted)
        );
    }
    let (stored_receipt, transaction) = cas_store::get_latest_worker_delivery(&cas_root, &task.id)
        .unwrap()
        .expect("lease-authorized delivery");
    assert_eq!(stored_receipt.worker_agent_id, owner_id);
    assert_eq!(transaction.state, WorkerDeliveryState::AwaitingVerification);
    let events = cas_store::list_worker_delivery_events(&cas_root, &transaction.id).unwrap();
    assert_eq!(events.len(), 1, "concurrent creation must emit one event");
    assert_eq!(events[0].state, WorkerDeliveryState::AwaitingVerification);
    let dispatch = cas_store::get_latest_verification_dispatch(&cas_root, &task.id)
        .unwrap()
        .expect("receipt dispatch");
    assert_eq!(
        dispatch.owner_agent_id, supervisor_id,
        "exact lease guard must preserve supervisor-owned verification routing"
    );
    assert!(
        open_agent_store(&cas_root)
            .unwrap()
            .get_lease(&task.id)
            .unwrap()
            .is_none(),
        "accepted handoff releases the exact original lease"
    );

    let before_retry = durable_close_snapshot(&cas_root);
    let retry_service = delivery_service(&cas_root, owner_id);
    let retry_a = owner_service.task(Parameters(task_req(serde_json::json!({
        "action": "close",
        "id": task.id,
        "reason": "concurrent same-session exact-cycle retry a",
        "completion_receipt": serde_json::to_string(&owner_receipt).unwrap(),
    }))));
    let retry_b = retry_service.task(Parameters(task_req(serde_json::json!({
        "action": "close",
        "id": task.id,
        "reason": "concurrent same-session exact-cycle retry b",
        "completion_receipt": serde_json::to_string(&owner_receipt).unwrap(),
    }))));
    let (retry_a, retry_b) = tokio::join!(retry_a, retry_b);
    let retry_a = retry_a.expect("first concurrent exact receipt retry");
    let retry_b = retry_b.expect("second concurrent exact receipt retry");
    assert!(get_text(&retry_a).contains("accepted idempotently"));
    assert!(get_text(&retry_b).contains("accepted idempotently"));
    assert_eq!(
        durable_close_snapshot(&cas_root),
        before_retry,
        "concurrent same-session exact-cycle retries must be durable no-ops"
    );

    let supervisor_service = delivery_service(&cas_root, supervisor_id);
    let verification = supervisor_service
        .verification(Parameters(verification_req(serde_json::json!({
            "action": "add",
            "task_id": task.id,
            "status": "approved",
            "summary": "receipt A exact delivery proof approved",
            "confidence": 1.0,
            "dispatch_id": dispatch.id,
        }))))
        .await
        .expect("resolve receipt A through the public verification boundary");
    assert!(get_text(&verification).contains("approved"));
    let (_, awaiting_merge) = cas_store::get_latest_worker_delivery(&cas_root, &task.id)
        .unwrap()
        .expect("receipt A delivery after verdict");
    assert_eq!(awaiting_merge.state, WorkerDeliveryState::AwaitingMerge);

    let before_awaiting_merge_retry = durable_close_snapshot(&cas_root);
    let awaiting_merge_retry = owner_service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": task.id,
            "reason": "exact awaiting-merge retry",
            "completion_receipt": serde_json::to_string(&owner_receipt).unwrap(),
        }))))
        .await
        .expect("exact receipt A retry while AwaitingMerge");
    assert!(
        get_text(&awaiting_merge_retry).contains("accepted idempotently")
            && get_text(&awaiting_merge_retry).contains("State: awaiting_merge")
    );
    assert_eq!(
        durable_close_snapshot(&cas_root),
        before_awaiting_merge_retry,
        "same receipt retry must remain a durable no-op in the coherent active cycle"
    );

    assert!(matches!(
        open_agent_store(&cas_root)
            .unwrap()
            .try_claim(
                &task.id,
                owner_id,
                600,
                Some("attempt replacement receipt B")
            )
            .unwrap(),
        cas::types::ClaimResult::Success(_)
    ));
    let mut replacement_receipt = owner_receipt.clone();
    replacement_receipt.proof_reference = "proof:replacement-receipt-b".to_string();
    replacement_receipt.scope_summary = "distinct replacement proof B".to_string();
    let before_replacement = durable_close_snapshot(&cas_root);
    let replacement = owner_service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": task.id,
            "reason": "receipt B must not replace active receipt A",
            "completion_receipt": serde_json::to_string(&replacement_receipt).unwrap(),
        }))))
        .await
        .expect("replacement rejection is a typed public result");
    let replacement_text = get_text(&replacement);
    assert!(
        replacement_text.contains("DELIVERY RECEIPT REJECTED")
            && replacement_text.contains("distinct proof boundary")
            && replacement_text.contains("awaiting_merge"),
        "public receipt B must reject against active receipt A, got:\n{replacement_text}"
    );
    assert_eq!(
        durable_close_snapshot(&cas_root),
        before_replacement,
        "replacement receipt B must not mutate task/deliverables/lease/receipt/transaction/event/dispatch/verdict/outbox state"
    );
}

async fn transactional_delivery_cleanup_resume_scenario(system_a: bool) {
    let mut env = test_env();
    let home = TempDir::new().expect("temp HOME");
    env.set("HOME", home.path());
    let repo = GitRepo::new();
    run_git(
        &["remote", "add", "origin", "git@github.com:org/delivery.git"],
        &repo.root,
    );
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init CAS");
    disable_system_a(&cas_root);
    std::fs::write(
        cas_root.join("config.toml"),
        "[verification]\nenabled = true\n",
    )
    .expect("enable exact post-merge verification gate");
    let fixture = if system_a { "system-a" } else { "system-b" };
    let factory_session = format!("delivery-{fixture}-factory");
    let worker_id = format!("delivery-{fixture}-worker-session");
    let supervisor_id = format!("delivery-{fixture}-supervisor-session");
    register_delivery_agent(
        &cas_root,
        &worker_id,
        "alice",
        AgentRole::Worker,
        &factory_session,
    );
    register_delivery_agent(
        &cas_root,
        &supervisor_id,
        "supervisor",
        AgentRole::Supervisor,
        &factory_session,
    );

    let worker_path = cas_root.join("worktrees").join("alice");
    repo.add_worktree(&worker_path, "factory/alice");
    std::fs::write(worker_path.join("delivery.rs"), "pub fn delivered() {}\n").unwrap();
    run_git(&["add", "delivery.rs"], &worker_path);
    run_git(&["commit", "-m", "delivery receipt commit"], &worker_path);

    let worktree_id = format!("{fixture}-delivery-worktree");
    if system_a {
        let worktree_store = open_worktree_store(&cas_root).expect("System-A worktree store");
        worktree_store.init().expect("initialize System-A store");
        worktree_store
            .add(&Worktree::new(
                worktree_id.clone(),
                "factory/alice".to_string(),
                "main".to_string(),
                worker_path.clone(),
            ))
            .expect("register System-A delivery worktree");
    }

    let task_store = open_task_store(&cas_root).expect("task store");
    let mut task = Task::new(
        format!("cas-delivery-{fixture}-integration"),
        format!("Transactional {fixture} delivery integration"),
    );
    task.status = TaskStatus::InProgress;
    task.depth = TaskDepth::Deep;
    task.assignee = Some("alice".to_string());
    if system_a {
        task.worktree_id = Some(worktree_id.clone());
    }
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/org/delivery".to_string(),
        target_branch: "main".to_string(),
    });
    task_store.add(&task).expect("add delivery task");
    let receipt = delivery_receipt(&task.id, &worker_id, &repo, "alice");
    submit_and_verify_delivery(&cas_root, &task.id, &worker_id, &supervisor_id, &receipt).await;

    // Arm a newer exact task proof cycle after the delivery proof is approved.
    // The post-merge internal close must surface this gate verbatim rather
    // than discarding it and claiming that delivery completed.
    let post_merge_dispatch = cas_store::create_verification_dispatch_bound(
        &cas_root,
        &task.id,
        &supervisor_id,
        &supervisor_id,
        &cas_types::VerificationProofBoundary::task(),
        chrono::Utc::now() + chrono::Duration::minutes(10),
        false,
    )
    .expect("post-merge exact verification gate");
    assert_eq!(
        cas_store::get_latest_verification_dispatch(&cas_root, &task.id)
            .unwrap()
            .unwrap()
            .id,
        post_merge_dispatch.id,
        "the new task proof cycle must be authoritative before merge"
    );

    // The production merge must persist CloseReady before cleanup removes
    // the source. The newer close gate then simulates interruption after
    // cleanup but before Delivered.
    env.set_current_dir(&repo.root);
    let supervisor_service = delivery_service(&cas_root, &supervisor_id);
    let mut merge = coord_req("worktree_merge");
    merge.id = Some(if system_a {
        worktree_id.clone()
    } else {
        "factory/alice".to_string()
    });
    merge.task_id = Some(task.id.clone());
    merge.allow_trunk = Some(true);
    merge.cleanup = Some(true);
    let result = supervisor_service
        .coordination(Parameters(merge))
        .await
        .expect("public cleanup-before-close-gate merge");
    let gate_text = get_text(&result);
    assert!(
        gate_text.contains("VERIFICATION") && gate_text.contains(&post_merge_dispatch.id),
        "worktree_merge must return the exact actionable close gate, got:\n{gate_text}"
    );
    assert!(
        !gate_text.contains("Merged worktree"),
        "a remaining close gate must not be reported as generic merge success"
    );
    assert!(
        !worker_path.exists(),
        "cleanup=true must remove the source before the close gate is returned"
    );
    if system_a {
        let persisted_worktree = open_worktree_store(&cas_root)
            .expect("reopen System-A store")
            .get(&worktree_id)
            .expect("cleanup keeps the durable System-A row");
        assert_eq!(
            persisted_worktree.status,
            cas::types::WorktreeStatus::Removed,
            "the retained row must not be mistaken for a live source"
        );
    }
    assert!(
        git_stdout(&repo.root, &["branch", "--list", "factory/alice"]).is_empty(),
        "cleanup=true must remove the source branch"
    );
    assert_eq!(
        git_stdout(
            &repo.root,
            &["merge-base", "--is-ancestor", &receipt.commit_sha, "main"]
        ),
        ""
    );
    let merged_target_tip = git_stdout(&repo.root, &["rev-parse", "main"]);
    let persisted = task_store.get(&task.id).expect("gated task");
    assert_ne!(persisted.status, TaskStatus::Closed);
    let (_, transaction) = cas_store::get_latest_worker_delivery(&cas_root, &task.id)
        .unwrap()
        .unwrap();
    assert_eq!(transaction.state, WorkerDeliveryState::CloseReady);
    let events_before = cas_store::list_worker_delivery_events(&cas_root, &transaction.id).unwrap();
    assert_eq!(
        events_before
            .iter()
            .filter(|event| event.state == WorkerDeliveryState::Merged)
            .count(),
        1,
        "the production merge must persist exactly one Merged transition"
    );
    assert_eq!(
        events_before
            .iter()
            .filter(|event| event.state == WorkerDeliveryState::CloseReady)
            .count(),
        1,
        "the post-merge close gate must leave one durable resumable state"
    );

    let gate_clear = supervisor_service
        .verification(Parameters(verification_req(serde_json::json!({
            "action": "add",
            "task_id": task.id,
            "status": "approved",
            "summary": "post-merge exact close proof approved",
            "confidence": 1.0,
            "dispatch_id": post_merge_dispatch.id,
        }))))
        .await
        .expect("public exact post-merge gate resolution");
    assert!(get_text(&gate_clear).contains("approved"));
    let mut gate_cleared_task = task_store.get(&task.id).expect("gate-cleared task");
    gate_cleared_task.depth = TaskDepth::Light;
    task_store
        .update(&gate_cleared_task)
        .expect("isolate retry from unrelated review gate");

    let mut retry = coord_req("worktree_merge");
    retry.id = Some(if system_a {
        worktree_id
    } else {
        "factory/alice".to_string()
    });
    retry.task_id = Some(task.id.clone());
    retry.allow_trunk = Some(true);
    retry.cleanup = Some(true);
    let retry_result = supervisor_service
        .coordination(Parameters(retry))
        .await
        .expect("source-less idempotent close-ready retry");
    assert!(
        get_text(&retry_result).contains("Merged worktree"),
        "{}",
        get_text(&retry_result)
    );
    assert_eq!(
        task_store.get(&task.id).expect("closed retry").status,
        TaskStatus::Closed
    );
    let (_, delivered) = cas_store::get_latest_worker_delivery(&cas_root, &task.id)
        .unwrap()
        .unwrap();
    assert_eq!(delivered.state, WorkerDeliveryState::Delivered);
    assert_eq!(
        git_stdout(&repo.root, &["rev-parse", "main"]),
        merged_target_tip,
        "source-less retry must not execute a second Git merge"
    );
    let events_after = cas_store::list_worker_delivery_events(&cas_root, &transaction.id).unwrap();
    assert_eq!(events_after.len(), events_before.len() + 1);
    assert_eq!(
        events_after
            .iter()
            .filter(|event| event.state == WorkerDeliveryState::Merged)
            .count(),
        1,
        "resume must not append a second merge event"
    );

    if !system_a {
        let delivered_task = task_store.get(&task.id).expect("delivered task");
        assert_eq!(
            delivered_task.deliverables.factory_branch_anchor.as_deref(),
            Some(receipt.commit_sha.as_str()),
            "the original coherent delivery cycle must retain its exact commit anchor"
        );

        let reopen = {
            let _role = VarGuard::set(&env, "CAS_AGENT_ROLE", "supervisor");
            supervisor_service
                .task(Parameters(task_req(serde_json::json!({
                    "action": "reopen",
                    "id": task.id,
                    "reason": "new proof cycle after delivered rework",
                }))))
                .await
                .expect("public supervisor reopen")
        };
        assert!(
            get_text(&reopen).contains("Reopened task"),
            "{}",
            get_text(&reopen)
        );
        let reopened = task_store.get(&task.id).expect("reopened task");
        assert_eq!(reopened.status, TaskStatus::Open);
        assert!(
            reopened.deliverables.factory_branch_anchor.is_none(),
            "reopen must clear the prior delivery anchor"
        );
        assert_eq!(
            cas_store::get_verification_dispatch(&cas_root, &post_merge_dispatch.id)
                .expect("reopened dispatch")
                .state,
            cas_types::VerificationDispatchState::Invalidated,
            "reopen must invalidate the old resolved proof authority"
        );

        let worker_service = delivery_service(&cas_root, &worker_id);
        let started = worker_service
            .task(Parameters(task_req(serde_json::json!({
                "action": "start",
                "id": task.id,
            }))))
            .await
            .expect("start reopened delivery task");
        assert!(
            get_text(&started).contains("Started task"),
            "{}",
            get_text(&started)
        );
        assert!(
            open_agent_store(&cas_root)
                .expect("agent store")
                .list_active_leases()
                .expect("active leases")
                .iter()
                .any(|lease| lease.task_id == task.id && lease.agent_id == worker_id),
            "the reopened proof cycle must have its own active worker lease"
        );

        let before_replay = durable_close_snapshot(&cas_root);
        let replay = worker_service
            .task(Parameters(task_req(serde_json::json!({
                "action": "close",
                "id": task.id,
                "reason": "stale terminal receipt replay",
                "completion_receipt": serde_json::to_string(&receipt).unwrap(),
            }))))
            .await
            .expect("stale receipt returns a typed tool rejection");
        let replay_text = get_text(&replay);
        assert!(
            replay_text.contains("DELIVERY RECEIPT REJECTED")
                && replay_text.contains("terminal Delivered proof cycle")
                && replay_text.contains("fresh cycle-bound receipt"),
            "reopened receipt replay needs typed actionable guidance:\n{replay_text}"
        );
        assert_eq!(
            durable_close_snapshot(&cas_root),
            before_replay,
            "stale Delivered replay mutated the reopened task, deliverables, lease, receipt/transaction/events, dispatch/verdict, or lifecycle outbox"
        );

        let direct_close = worker_service
            .task(Parameters(task_req(serde_json::json!({
                "action": "update",
                "id": task.id,
                "status": "closed",
            }))))
            .await
            .expect_err("stale Delivered evidence must not authorize a later direct close");
        let direct_close_text = direct_close.message.to_string();
        assert!(
            direct_close_text.contains("DELIVERY CLOSE BLOCKED")
                && direct_close_text.contains("prior proof cycle")
                && direct_close_text.contains("fresh immutable completion receipt"),
            "later close needs cycle-specific remediation:\n{direct_close_text}"
        );
        assert_eq!(
            durable_close_snapshot(&cas_root),
            before_replay,
            "later close reused stale terminal delivery evidence or mutated the new proof cycle"
        );
    }
}

#[tokio::test]
async fn transactional_delivery_public_interrupted_resume_is_ancestry_gated_and_idempotent() {
    transactional_delivery_cleanup_resume_scenario(false).await;
}

#[tokio::test]
async fn transactional_system_a_cleanup_gate_retry_reconciles_without_source_path() {
    transactional_delivery_cleanup_resume_scenario(true).await;
}

#[tokio::test]
async fn transactional_delivery_public_merge_persists_changed_worker_tip_without_git_mutation() {
    let mut env = test_env();
    let home = TempDir::new().expect("temp HOME");
    env.set("HOME", home.path());
    let repo = GitRepo::new();
    run_git(
        &["remote", "add", "origin", "git@github.com:org/delivery.git"],
        &repo.root,
    );
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init CAS");
    disable_system_a(&cas_root);
    let worker_id = "stale-worker-session";
    let supervisor_id = "stale-supervisor-session";
    register_delivery_agent(
        &cas_root,
        worker_id,
        "bob",
        AgentRole::Worker,
        "stale-factory",
    );
    register_delivery_agent(
        &cas_root,
        supervisor_id,
        "supervisor",
        AgentRole::Supervisor,
        "stale-factory",
    );
    let worker_path = cas_root.join("worktrees").join("bob");
    repo.add_worktree(&worker_path, "factory/bob");
    std::fs::write(worker_path.join("first.rs"), "pub fn first() {}\n").unwrap();
    run_git(&["add", "first.rs"], &worker_path);
    run_git(&["commit", "-m", "receipt tip"], &worker_path);

    let task_store = open_task_store(&cas_root).expect("task store");
    let mut task = Task::new(
        "cas-delivery-stale".to_string(),
        "Stale delivery tip".to_string(),
    );
    task.status = TaskStatus::InProgress;
    task.depth = TaskDepth::Light;
    task.assignee = Some("bob".to_string());
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/org/delivery".to_string(),
        target_branch: "main".to_string(),
    });
    task_store.add(&task).expect("add stale task");
    let receipt = delivery_receipt(&task.id, worker_id, &repo, "bob");
    submit_and_verify_delivery(&cas_root, &task.id, worker_id, supervisor_id, &receipt).await;
    std::fs::write(worker_path.join("drift.rs"), "pub fn drift() {}\n").unwrap();
    run_git(&["add", "drift.rs"], &worker_path);
    run_git(&["commit", "-m", "tip drift after receipt"], &worker_path);
    let main_before = git_stdout(&repo.root, &["rev-parse", "main"]);

    env.set_current_dir(&repo.root);
    let supervisor_service = delivery_service(&cas_root, supervisor_id);
    let mut merge = coord_req("worktree_merge");
    merge.id = Some("factory/bob".to_string());
    merge.task_id = Some(task.id.clone());
    merge.allow_trunk = Some(true);
    merge.cleanup = Some(false);
    let result = supervisor_service
        .coordination(Parameters(merge))
        .await
        .expect("public stale-tip merge response");
    assert!(get_text(&result).contains("tip_changed"));
    assert_eq!(git_stdout(&repo.root, &["rev-parse", "main"]), main_before);
    let (_, transaction) = cas_store::get_latest_worker_delivery(&cas_root, &task.id)
        .unwrap()
        .unwrap();
    assert_eq!(transaction.state, WorkerDeliveryState::TipChanged);
    assert_eq!(
        cas_store::list_worker_delivery_events(&cas_root, &transaction.id)
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn pending_delivery_proof_rejects_review_scope_update_but_allows_notes() {
    let mut env = test_env();
    let home = TempDir::new().expect("temp HOME");
    env.set("HOME", home.path());
    let repo = GitRepo::new();
    run_git(
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:org/scope-lock.git",
        ],
        &repo.root,
    );
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init CAS");
    disable_system_a(&cas_root);
    let task_store = open_task_store(&cas_root).expect("task store");

    let mut epic = Task::new("cas-scope-epic".to_string(), "Scope epic".to_string());
    epic.task_type = TaskType::Epic;
    task_store.add(&epic).expect("add epic");

    let mut task = Task::new(
        "cas-scope-locked".to_string(),
        "Immutable review scope".to_string(),
    );
    task.status = TaskStatus::AwaitingMerge;
    task.pending_verification = true;
    task.assignee = Some("alice".to_string());
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/org/scope-lock".to_string(),
        target_branch: "main".to_string(),
    });
    task_store.add(&task).expect("add task");

    let input = WorkerCompletionReceiptInput {
        task_id: task.id.clone(),
        worker_agent_id: "scope-worker".to_string(),
        repo_selector: "remote:github.com/org/scope-lock".to_string(),
        source_branch: "factory/alice".to_string(),
        commit_sha: "a".repeat(40),
        merge_base_sha: "b".repeat(40),
        target_branch: "main".to_string(),
        target_sha: "c".repeat(40),
        proof_reference: "proof:scope-lock".to_string(),
        scope_summary: "immutable review boundary".to_string(),
        artifact_path: None,
    };
    let receipt = cas_store::build_worker_completion_receipt(&input, "alice", chrono::Utc::now());
    let (transaction, dispatch) = cas_store::create_worker_delivery_with_dispatch(
        &cas_root,
        &receipt,
        WorkerDeliveryState::AwaitingVerification,
        "scope-worker",
        "scope-supervisor",
        chrono::Utc::now() + chrono::Duration::minutes(10),
    )
    .expect("active exact delivery boundary");
    let events_before =
        cas_store::list_worker_delivery_events(&cas_root, &transaction.id).expect("events");
    let task_before = serde_json::to_value(task_store.get(&task.id).unwrap()).unwrap();
    let deps_before = serde_json::to_value(task_store.get_dependencies(&task.id).unwrap()).unwrap();

    let service = make_service(cas_root.clone());
    let error = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": task.id,
            "title": "Changed title",
            "notes": "must not append on mixed rejected request",
            "priority": 0,
            "labels": "changed,security",
            "description": "Changed description",
            "design": "Changed design",
            "acceptance_criteria": "Changed acceptance",
            "demo_statement": "Changed demo",
            "execution_note": "additive-only",
            "external_ref": "changed-reference",
            "assignee": "bob",
            "status": "open",
            "epic": epic.id,
            "depth": "light",
            "target_repo": repo.root,
            "target_branch": "review-scope"
        }))))
        .await
        .expect_err("public review-scope update must be rejected");
    let text = error.message.to_string();
    assert!(
        text.contains("DELIVERY PROOF SCOPE LOCKED"),
        "review-relevant task mutation must fail closed, got:\n{text}"
    );
    assert_eq!(
        serde_json::to_value(task_store.get(&task.id).unwrap()).unwrap(),
        task_before,
        "rejected scope update must leave the task byte-for-byte unchanged"
    );
    assert_eq!(
        serde_json::to_value(task_store.get_dependencies(&task.id).unwrap()).unwrap(),
        deps_before
    );
    assert_eq!(
        cas_store::get_latest_verification_dispatch(&cas_root, &task.id)
            .unwrap()
            .unwrap(),
        dispatch
    );
    assert_eq!(
        cas_store::get_latest_worker_delivery(&cas_root, &task.id)
            .unwrap()
            .unwrap()
            .1,
        transaction
    );
    assert_eq!(
        cas_store::list_worker_delivery_events(&cas_root, &transaction.id).unwrap(),
        events_before
    );

    let update_to_closed = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": task.id,
            "status": "closed"
        }))))
        .await
        .expect_err("public update-to-closed must be rejected");
    assert!(
        update_to_closed.message.contains("DELIVERY CLOSE BLOCKED"),
        "update-to-closed must use the more specific early delivery-state guard"
    );
    assert_eq!(
        serde_json::to_value(task_store.get(&task.id).unwrap()).unwrap(),
        task_before
    );

    let note = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": task.id,
            "notes": "harmless progress"
        }))))
        .await
        .expect("notes-only update");
    assert!(get_text(&note).contains("notes"));
    let after_note = task_store.get(&task.id).unwrap();
    assert!(after_note.notes.contains("harmless progress"));
    assert_eq!(after_note.status, TaskStatus::AwaitingMerge);
    assert_eq!(
        cas_store::get_latest_verification_dispatch(&cas_root, &task.id)
            .unwrap()
            .unwrap(),
        dispatch
    );
    assert_eq!(
        cas_store::get_latest_worker_delivery(&cas_root, &task.id)
            .unwrap()
            .unwrap()
            .1,
        transaction
    );
    assert_eq!(
        cas_store::list_worker_delivery_events(&cas_root, &transaction.id).unwrap(),
        events_before
    );

    let progress = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "notes",
            "id": task.id,
            "note_type": "progress",
            "notes": "dedicated progress action remains available"
        }))))
        .await
        .expect("dedicated progress note");
    assert!(get_text(&progress).contains("progress note"));
    let after_progress = task_store.get(&task.id).unwrap();
    assert!(
        after_progress
            .notes
            .contains("dedicated progress action remains available")
    );
    assert_eq!(after_progress.status, TaskStatus::AwaitingMerge);
    assert_eq!(
        cas_store::get_latest_verification_dispatch(&cas_root, &task.id)
            .unwrap()
            .unwrap(),
        dispatch
    );
    assert_eq!(
        cas_store::get_latest_worker_delivery(&cas_root, &task.id)
            .unwrap()
            .unwrap()
            .1,
        transaction
    );
    assert_eq!(
        cas_store::list_worker_delivery_events(&cas_root, &transaction.id).unwrap(),
        events_before
    );
}

#[tokio::test]
async fn resolved_task_proof_freezes_scope_until_supervisor_starts_a_fresh_cycle() {
    let mut env = test_env();
    let home = TempDir::new().expect("temp HOME");
    env.set("HOME", home.path());
    let repo = GitRepo::new();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init CAS");
    std::fs::write(
        cas_root.join("config.toml"),
        "[worktrees]\nenabled = false\n[verification]\nenabled = true\n[code_review]\nowner = \"worker\"\n",
    )
    .unwrap();

    let worker_id = "resolved-scope-worker";
    let supervisor_id = "resolved-scope-supervisor";
    register_delivery_agent(
        &cas_root,
        worker_id,
        "alice",
        AgentRole::Worker,
        "resolved-scope-factory",
    );
    register_delivery_agent(
        &cas_root,
        supervisor_id,
        "supervisor",
        AgentRole::Supervisor,
        "resolved-scope-factory",
    );
    let worker = delivery_service(&cas_root, worker_id);
    let supervisor = delivery_service(&cas_root, supervisor_id);
    let task_store = open_task_store(&cas_root).expect("task store");

    async fn approve_task_scope(
        cas_root: &Path,
        supervisor: &CasService,
        task_id: &str,
        supervisor_id: &str,
    ) -> cas_types::VerificationDispatch {
        let dispatch = cas_store::create_verification_dispatch_bound(
            cas_root,
            task_id,
            supervisor_id,
            supervisor_id,
            &cas_types::VerificationProofBoundary::task(),
            chrono::Utc::now() + chrono::Duration::minutes(10),
            false,
        )
        .expect("task-only dispatch");
        supervisor
            .verification(Parameters(verification_req(serde_json::json!({
                "action": "add",
                "task_id": task_id,
                "status": "approved",
                "summary": "approved immutable task scope",
                "confidence": 1.0,
                "dispatch_id": dispatch.id,
            }))))
            .await
            .expect("public supervisor verification");
        cas_store::get_verification_dispatch(cas_root, &dispatch.id).unwrap()
    }

    let mut unchanged = Task::new(
        "cas-resolved-scope-unchanged".to_string(),
        "Reviewed unchanged scope".to_string(),
    );
    unchanged.status = TaskStatus::InProgress;
    unchanged.depth = TaskDepth::Deep;
    unchanged.assignee = Some("alice".to_string());
    unchanged.acceptance_criteria = "original acceptance".to_string();
    task_store.add(&unchanged).expect("add unchanged task");
    let unchanged_dispatch =
        approve_task_scope(&cas_root, &supervisor, &unchanged.id, supervisor_id).await;
    assert_eq!(
        unchanged_dispatch.state,
        cas_types::VerificationDispatchState::Resolved
    );

    let before_rejected_update = durable_close_snapshot(&cas_root);
    let rejected = worker
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": unchanged.id,
            "acceptance_criteria": "scope changed after approval",
        }))))
        .await
        .expect_err("resolved close-authoritative scope must reject semantic mutation");
    assert!(rejected.message.contains("DELIVERY PROOF SCOPE LOCKED"));
    assert!(rejected.message.contains("task action=reopen"));
    assert_eq!(
        durable_close_snapshot(&cas_root),
        before_rejected_update,
        "rejected semantic update must not mutate task, proof, events, or queues"
    );

    worker
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": unchanged.id,
            "notes": "notes remain outside reviewed semantic scope",
        }))))
        .await
        .expect("notes-only update remains supported");
    assert!(
        task_store
            .get(&unchanged.id)
            .unwrap()
            .notes
            .contains("notes remain outside reviewed semantic scope")
    );

    let unchanged_close = worker
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": unchanged.id,
            "status": "closed",
        }))))
        .await
        .expect("the unchanged reviewed scope may close with its exact verdict");
    assert!(get_text(&unchanged_close).contains("Updated task"));
    let closed = task_store.get(&unchanged.id).unwrap();
    assert_eq!(closed.status, TaskStatus::Closed);
    assert_eq!(closed.acceptance_criteria, "original acceptance");

    let mut fresh = Task::new(
        "cas-resolved-scope-fresh".to_string(),
        "Reviewed scope needing rework".to_string(),
    );
    fresh.status = TaskStatus::InProgress;
    fresh.depth = TaskDepth::Deep;
    fresh.assignee = Some("alice".to_string());
    fresh.acceptance_criteria = "first-cycle acceptance".to_string();
    task_store.add(&fresh).expect("add fresh-cycle task");
    let old_dispatch = approve_task_scope(&cas_root, &supervisor, &fresh.id, supervisor_id).await;

    let rejected = worker
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": fresh.id,
            "acceptance_criteria": "second-cycle acceptance",
        }))))
        .await
        .expect_err("approved scope needs explicit recovery before mutation");
    assert!(rejected.message.contains("task action=reopen"));

    let worker_reopen = {
        let _role = VarGuard::set(&env, "CAS_AGENT_ROLE", "worker");
        worker
            .task(Parameters(task_req(serde_json::json!({
                "action": "reopen",
                "id": fresh.id,
                "reason": "worker must not invalidate approved proof",
            }))))
            .await
            .expect_err("worker cannot reset an approved review scope")
    };
    assert!(worker_reopen.message.contains("only supervisors"));
    assert_eq!(
        cas_store::get_verification_dispatch(&cas_root, &old_dispatch.id)
            .unwrap()
            .state,
        cas_types::VerificationDispatchState::Resolved
    );

    let reopen = {
        let _role = VarGuard::set(&env, "CAS_AGENT_ROLE", "supervisor");
        supervisor
            .task(Parameters(task_req(serde_json::json!({
                "action": "reopen",
                "id": fresh.id,
                "reason": "invalidate approved scope before rework",
            }))))
            .await
            .expect("supervisor starts a fresh review scope")
    };
    assert!(get_text(&reopen).contains("fresh verification scope"));
    assert_eq!(
        cas_store::get_verification_dispatch(&cas_root, &old_dispatch.id)
            .unwrap()
            .state,
        cas_types::VerificationDispatchState::Invalidated
    );
    assert_eq!(task_store.get(&fresh.id).unwrap().status, TaskStatus::Open);

    worker
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": fresh.id,
        }))))
        .await
        .expect("worker starts fresh cycle");
    worker
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": fresh.id,
            "acceptance_criteria": "second-cycle acceptance",
        }))))
        .await
        .expect("fresh scope may be updated");

    let close = {
        let _role = VarGuard::set(&env, "CAS_AGENT_ROLE", "worker");
        let _factory = VarGuard::set(&env, "CAS_FACTORY_MODE", "1");
        worker
            .task(Parameters(task_req(serde_json::json!({
                "action": "close",
                "id": fresh.id,
                "reason": "fresh scope requires fresh proof",
            }))))
            .await
            .expect("public close returns fresh verification guidance")
    };
    let close_text = get_text(&close);
    assert!(close_text.contains("VERIFICATION REQUIRED"), "{close_text}");
    let new_dispatch = cas_store::get_latest_verification_dispatch(&cas_root, &fresh.id)
        .unwrap()
        .unwrap();
    assert_ne!(new_dispatch.id, old_dispatch.id);
    assert_eq!(
        new_dispatch.state,
        cas_types::VerificationDispatchState::Pending
    );
    let changed = task_store.get(&fresh.id).unwrap();
    assert_eq!(changed.acceptance_criteria, "second-cycle acceptance");
    assert_ne!(changed.status, TaskStatus::Closed);
}

#[tokio::test]
async fn public_registration_cannot_mint_or_capture_supervisor_verification_authority() {
    let mut env = test_env();
    let home = TempDir::new().expect("temp HOME");
    env.set("HOME", home.path());
    let cas_root = init_cas_dir(home.path(), &mut env).expect("init CAS");
    let agent_store = open_agent_store(&cas_root).expect("agent store");
    let task_store = open_task_store(&cas_root).expect("task store");

    let trusted_supervisor_id = "server-created-supervisor";
    register_delivery_agent(
        &cas_root,
        trusted_supervisor_id,
        "supervisor",
        AgentRole::Supervisor,
        "registration-authority-factory",
    );
    register_delivery_agent(
        &cas_root,
        "worker-owner",
        "worker-owner",
        AgentRole::Worker,
        "registration-authority-factory",
    );

    let mut task = Task::new(
        "cas-public-registration-proof".to_string(),
        "Public registration cannot mint proof authority".to_string(),
    );
    task.status = TaskStatus::InProgress;
    task.assignee = Some("worker-owner".to_string());
    task.pending_verification = true;
    task_store.add(&task).expect("task");
    let dispatch = cas_store::create_verification_dispatch_bound(
        &cas_root,
        &task.id,
        "worker-owner",
        trusted_supervisor_id,
        &cas_types::VerificationProofBoundary::task(),
        chrono::Utc::now() + chrono::Duration::minutes(10),
        false,
    )
    .expect("exact dispatch");

    // The MCP server's launch environment is the factory's durable role
    // contract, so registration must persist it for routing. Identity-source
    // provenance remains separate: a public call still carries zero
    // supervisor_direct verification authority.
    let public = CasCore::with_daemon(cas_root.clone(), None, None);
    {
        let _role = VarGuard::set(&env, "CAS_AGENT_ROLE", "supervisor");
        public
            .cas_agent_register(Parameters(AgentRegisterRequest {
                name: "supervisor".to_string(),
                agent_type: "primary".to_string(),
                session_id: Some("public-env-supervisor".to_string()),
                parent_id: None,
            }))
            .await
            .expect("ordinary public registration remains supported");
    }
    assert_eq!(
        agent_store
            .get("public-env-supervisor")
            .expect("registered public agent")
            .role,
        AgentRole::Supervisor,
        "register must persist the harness-provided supervisor role"
    );

    let before_denial = durable_close_snapshot(&cas_root);
    let denied = public
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: task.id.clone(),
            status: "approved".to_string(),
            summary: "caller-controlled supervisor claim".to_string(),
            confidence: None,
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
            verifier_capability: None,
            dispatch_id: Some(dispatch.id.clone()),
        }))
        .await
        .expect_err("publicly registered identity cannot verify directly");
    assert!(denied.message.contains("authority rejected"));
    assert_eq!(
        durable_close_snapshot(&cas_root),
        before_denial,
        "denied public authority must not mutate task, dispatch, verdict, event, or queue state"
    );

    let mut session_task = Task::new(
        "cas-public-session-start-proof".to_string(),
        "Public session_start cannot mint proof authority".to_string(),
    );
    session_task.status = TaskStatus::InProgress;
    session_task.assignee = Some("worker-owner".to_string());
    session_task.pending_verification = true;
    task_store.add(&session_task).expect("session task");
    let session_dispatch = cas_store::create_verification_dispatch_bound(
        &cas_root,
        &session_task.id,
        "worker-owner",
        trusted_supervisor_id,
        &cas_types::VerificationProofBoundary::task(),
        chrono::Utc::now() + chrono::Duration::minutes(10),
        false,
    )
    .expect("session exact dispatch");
    let public_session = CasCore::with_daemon(cas_root.clone(), None, None);
    {
        let _role = VarGuard::set(&env, "CAS_AGENT_ROLE", "supervisor");
        public_session
            .cas_agent_session_start(Parameters(SessionStartRequest {
                session_id: Some("public-session-env-supervisor".to_string()),
                name: Some("supervisor".to_string()),
                agent_type: Some("primary".to_string()),
                parent_id: None,
                permission_mode: None,
                cwd: None,
                limit: Some(1),
            }))
            .await
            .expect("ordinary public session_start remains supported");
    }
    assert_eq!(
        agent_store
            .get("public-session-env-supervisor")
            .expect("public session agent")
            .role,
        AgentRole::Supervisor,
        "session_start must persist the harness-provided supervisor role"
    );
    let before_session_denial = durable_close_snapshot(&cas_root);
    public_session
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: session_task.id.clone(),
            status: "approved".to_string(),
            summary: "session_start supervisor spoof".to_string(),
            confidence: None,
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
            verifier_capability: None,
            dispatch_id: Some(session_dispatch.id.clone()),
        }))
        .await
        .expect_err("public session_start identity cannot verify directly");
    assert_eq!(
        durable_close_snapshot(&cas_root),
        before_session_denial,
        "denied session_start spoof must not mutate proof state"
    );

    // Re-registration must repair a stale role even though the generic store
    // ON CONFLICT contract preserves authority by default. Public provenance
    // still prevents the repaired row from submitting verification directly.
    register_delivery_agent(
        &cas_root,
        "existing-worker-registration",
        "worker",
        AgentRole::Worker,
        "registration-authority-factory",
    );
    let worker_reregister = CasCore::with_daemon(cas_root.clone(), None, None);
    {
        let _role = VarGuard::set(&env, "CAS_AGENT_ROLE", "supervisor");
        worker_reregister
            .cas_agent_register(Parameters(AgentRegisterRequest {
                name: "supervisor".to_string(),
                agent_type: "primary".to_string(),
                session_id: Some("existing-worker-registration".to_string()),
                parent_id: None,
            }))
            .await
            .expect("worker re-registration is idempotent");
    }
    let worker = agent_store
        .get("existing-worker-registration")
        .expect("re-registered supervisor");
    assert_eq!(worker.role, AgentRole::Supervisor);
    assert_eq!(worker.agent_type, AgentType::Primary);
    let mut worker_task = Task::new(
        "cas-worker-reregister-proof".to_string(),
        "Worker re-registration cannot mint proof authority".to_string(),
    );
    worker_task.status = TaskStatus::InProgress;
    worker_task.assignee = Some("worker-owner".to_string());
    worker_task.pending_verification = true;
    task_store.add(&worker_task).expect("worker proof task");
    let worker_dispatch = cas_store::create_verification_dispatch_bound(
        &cas_root,
        &worker_task.id,
        "worker-owner",
        trusted_supervisor_id,
        &cas_types::VerificationProofBoundary::task(),
        chrono::Utc::now() + chrono::Duration::minutes(10),
        false,
    )
    .expect("worker exact dispatch");
    let before_worker_denial = durable_close_snapshot(&cas_root);
    worker_reregister
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: worker_task.id.clone(),
            status: "approved".to_string(),
            summary: "worker re-registration supervisor spoof".to_string(),
            confidence: None,
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
            verifier_capability: None,
            dispatch_id: Some(worker_dispatch.id.clone()),
        }))
        .await
        .expect_err("re-registered worker cannot verify directly");
    assert_eq!(
        durable_close_snapshot(&cas_root),
        before_worker_denial,
        "denied worker upgrade must not mutate proof state"
    );

    // Explicit privileged request claims fail before any row can be created.
    let explicit_register = CasCore::with_daemon(cas_root.clone(), None, None);
    let error = explicit_register
        .cas_agent_register(Parameters(AgentRegisterRequest {
            name: "supervisor".to_string(),
            agent_type: "supervisor".to_string(),
            session_id: Some("explicit-public-supervisor".to_string()),
            parent_id: None,
        }))
        .await
        .expect_err("public supervisor role claim must be rejected");
    assert!(
        error
            .message
            .contains("cannot request supervisor or director")
    );
    assert!(agent_store.get("explicit-public-supervisor").is_err());

    let explicit_session = CasCore::with_daemon(cas_root.clone(), None, None);
    let error = explicit_session
        .cas_agent_session_start(Parameters(SessionStartRequest {
            session_id: Some("explicit-public-director".to_string()),
            name: Some("director".to_string()),
            agent_type: Some("director".to_string()),
            parent_id: None,
            permission_mode: None,
            cwd: None,
            limit: None,
        }))
        .await
        .expect_err("public director role claim must be rejected");
    assert!(
        error
            .message
            .contains("cannot request supervisor or director")
    );
    assert!(agent_store.get("explicit-public-director").is_err());

    // Public attachment to a pre-existing privileged row is not an
    // authentication mechanism and must be rejected without role drift.
    let impersonator = CasCore::with_daemon(cas_root.clone(), None, None);
    impersonator
        .cas_agent_register(Parameters(AgentRegisterRequest {
            name: "supervisor".to_string(),
            agent_type: "primary".to_string(),
            session_id: Some(trusted_supervisor_id.to_string()),
            parent_id: None,
        }))
        .await
        .expect_err("public caller cannot capture a privileged durable row");
    assert_eq!(
        agent_store
            .get(trusted_supervisor_id)
            .expect("trusted supervisor")
            .role,
        AgentRole::Supervisor
    );

    // The independently server-bound supervisor can still recover the exact
    // dispatch denied above.
    let trusted = delivery_service(&cas_root, trusted_supervisor_id);
    trusted
        .verification(Parameters(verification_req(serde_json::json!({
            "action": "add",
            "task_id": task.id,
            "status": "approved",
            "summary": "server-authenticated supervisor recovery",
            "dispatch_id": dispatch.id,
        }))))
        .await
        .expect("server-created supervisor retains exact recovery");
    assert_eq!(
        cas_store::get_verification_dispatch(&cas_root, &dispatch.id)
            .unwrap()
            .state,
        cas_types::VerificationDispatchState::Resolved
    );
    let verification = open_verification_store(&cas_root)
        .unwrap()
        .get_latest_for_task(&task.id)
        .unwrap()
        .expect("trusted supervisor verdict");
    assert_eq!(
        verification.provenance,
        cas_types::VerificationProvenance::SupervisorDirect
    );
}

#[tokio::test]
async fn terminal_task_rejects_fresh_completion_receipt_without_any_mutation() {
    let mut env = test_env();
    let home = TempDir::new().expect("temp HOME");
    env.set("HOME", home.path());
    let repo = GitRepo::new();
    run_git(
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:org/terminal-replay.git",
        ],
        &repo.root,
    );
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init CAS");
    disable_system_a(&cas_root);
    let worker_id = "terminal-worker-session";
    register_delivery_agent(
        &cas_root,
        worker_id,
        "alice",
        AgentRole::Worker,
        "terminal-factory",
    );
    let worker_path = cas_root.join("worktrees").join("alice");
    repo.add_worktree(&worker_path, "factory/alice");
    std::fs::write(worker_path.join("replayed.rs"), "pub fn replayed() {}\n").unwrap();
    run_git(&["add", "replayed.rs"], &worker_path);
    run_git(
        &["commit", "-m", "stale post-close worker commit"],
        &worker_path,
    );

    let task_store = open_task_store(&cas_root).expect("task store");
    let mut task = Task::new(
        "cas-terminal-receipt".to_string(),
        "Already delivered task".to_string(),
    );
    task.status = TaskStatus::Closed;
    task.closed_at = Some(chrono::Utc::now());
    task.assignee = Some("alice".to_string());
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/org/terminal-replay".to_string(),
        target_branch: "main".to_string(),
    });
    task_store.add(&task).expect("add closed task");
    let task_before = serde_json::to_value(task_store.get(&task.id).unwrap()).unwrap();
    assert!(
        cas_store::get_latest_worker_delivery(&cas_root, &task.id)
            .unwrap()
            .is_none()
    );
    assert!(
        cas_store::get_latest_verification_dispatch(&cas_root, &task.id)
            .unwrap()
            .is_none()
    );
    let durable_counts = || {
        let conn = rusqlite::Connection::open(cas_root.join("cas.db")).unwrap();
        [
            "worker_completion_receipts",
            "worker_delivery_transactions",
            "worker_delivery_events",
            "verification_dispatches",
            "verifications",
        ]
        .map(|table| {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
        })
    };
    let counts_before = durable_counts();
    let receipt = WorkerCompletionReceiptInput {
        task_id: task.id.clone(),
        worker_agent_id: worker_id.to_string(),
        repo_selector: "remote:github.com/org/terminal-replay".to_string(),
        source_branch: "factory/alice".to_string(),
        commit_sha: git_stdout(&repo.root, &["rev-parse", "factory/alice"]),
        merge_base_sha: git_stdout(&repo.root, &["merge-base", "factory/alice", "main"]),
        target_branch: "main".to_string(),
        target_sha: git_stdout(&repo.root, &["rev-parse", "main"]),
        proof_reference: "proof:terminal-replay".to_string(),
        scope_summary: "stale post-close receipt".to_string(),
        artifact_path: None,
    };
    let receipt_id =
        cas_store::build_worker_completion_receipt(&receipt, "alice", chrono::Utc::now()).id;

    let worker = delivery_service(&cas_root, worker_id);
    let result = worker
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": task.id,
            "reason": "stale terminal replay",
            "completion_receipt": serde_json::to_string(&receipt).unwrap()
        }))))
        .await
        .expect("terminal receipt response");
    let text = get_text(&result);
    assert!(
        text.contains("DELIVERY RECEIPT REJECTED") && text.contains("already Closed"),
        "terminal receipt must fail closed, got:\n{text}"
    );
    assert_eq!(
        serde_json::to_value(task_store.get(&task.id).unwrap()).unwrap(),
        task_before,
        "terminal replay must not update status, timestamps, or deliverables"
    );
    assert!(
        cas_store::get_worker_delivery_by_receipt(&cas_root, &receipt_id)
            .unwrap()
            .is_none(),
        "terminal replay must not create a receipt or transaction"
    );
    assert!(
        cas_store::get_latest_verification_dispatch(&cas_root, &task.id)
            .unwrap()
            .is_none(),
        "terminal replay must not create a verification dispatch"
    );
    assert_eq!(
        durable_counts(),
        counts_before,
        "terminal replay must not create receipts, transactions, events, dispatches, or verdicts"
    );
}

// =============================================================================
// cas-f102 (GH #140): worktree_cleanup gets the cas-1d11 System-B fallback
// =============================================================================

/// Commit something on the worktree's branch and merge it into the repo's
/// default branch, so the factory branch is genuinely reachable from elsewhere
/// — the state a retired worker leaves behind after its merge landed.
fn merge_worktree_branch_into_default(repo: &GitRepo, wt_path: &Path, branch: &str) {
    let run_in = |dir: &Path, args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {:?} in {} failed: {}",
            args,
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    };
    std::fs::write(wt_path.join("worker.txt"), "worker output\n").unwrap();
    run_in(wt_path, &["add", "worker.txt"]);
    run_in(wt_path, &["commit", "-m", "worker work"]);
    run_in(
        &repo.root,
        &["merge", "--no-ff", "-m", "merge worker", branch],
    );
}

/// AC1: a retired worker's System-B worktree — merged branch, no live agent,
/// System A flag off — is removed through CAS, and the reply says what went.
///
/// This is the whole bug: `worktree_merge` on this exact worktree succeeds
/// (cas-1d11 exempted it), while `worktree_cleanup` refused with "experimental
/// and disabled", leaving `git worktree remove` — which bypasses factory
/// tracking — as the only option.
#[tokio::test]
async fn test_worktree_cleanup_removes_retired_system_b_worktree() {
    let repo = GitRepo::new();
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);

    let wt_path = cas_root.join("worktrees").join("retired-fox");
    repo.add_worktree(&wt_path, "factory/retired-fox");
    merge_worktree_branch_into_default(&repo, &wt_path, "factory/retired-fox");

    let svc = make_service(cas_root.clone());
    let mut req = coord_req("worktree_cleanup");
    req.id = Some("retired-fox".to_string());
    let text = get_text(&svc.coordination(Parameters(req)).await.expect("cleanup"));

    assert!(
        !text.contains("experimental and disabled"),
        "the flag-off gate must no longer swallow a System-B cleanup: {text}"
    );
    assert!(
        text.contains("Removed System B worktree"),
        "reply must state what was removed: {text}"
    );
    assert!(
        text.contains("factory/retired-fox"),
        "reply must name the branch it deleted: {text}"
    );
    assert!(
        !wt_path.exists(),
        "the worktree directory must actually be gone"
    );
}

/// AC2: a nonexistent target gets an accurate not-found naming BOTH places
/// that were searched — never the 'disabled' text, which is the misdiagnosis
/// this task exists to kill.
#[tokio::test]
async fn test_worktree_cleanup_unknown_target_reports_not_found_not_disabled() {
    let repo = GitRepo::new();
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);

    let svc = make_service(cas_root.clone());
    let mut req = coord_req("worktree_cleanup");
    req.id = Some("no-such-worker".to_string());
    let err = svc
        .coordination(Parameters(req))
        .await
        .expect_err("unknown target must be an error, not a success message");

    let message = err.message.to_string();
    assert!(
        message.contains("Worktree not found: no-such-worker"),
        "not-found must name the target: {message}"
    );
    assert!(
        message.contains("System A worktree store") && message.contains("System B path"),
        "not-found must name both systems it searched: {message}"
    );
    assert!(
        !message.contains("experimental and disabled"),
        "a genuine absence must never render as the disabled gate: {message}"
    );
}

/// AC2: a branch whose commits exist on no other branch is refused without
/// force. `WorktreeManager::abandon` deletes the branch with `-D`, so without
/// this gate a P3 cleanup would silently destroy unmerged work.
#[tokio::test]
async fn test_worktree_cleanup_refuses_unmerged_branch_without_force() {
    let repo = GitRepo::new();
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);

    let wt_path = cas_root.join("worktrees").join("unmerged-owl");
    repo.add_worktree(&wt_path, "factory/unmerged-owl");
    // Commit on the branch but never merge it.
    let run_in = |dir: &Path, args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git");
    };
    std::fs::write(wt_path.join("wip.txt"), "unmerged\n").unwrap();
    run_in(&wt_path, &["add", "wip.txt"]);
    run_in(&wt_path, &["commit", "-m", "unmerged work"]);

    let svc = make_service(cas_root.clone());
    let mut req = coord_req("worktree_cleanup");
    req.id = Some("unmerged-owl".to_string());
    let text = get_text(&svc.coordination(Parameters(req)).await.expect("cleanup"));

    assert!(
        text.contains("Refused") && text.contains("exist on no other branch"),
        "unmerged commits must block removal: {text}"
    );
    assert!(
        wt_path.exists(),
        "a refused cleanup must leave the worktree in place"
    );
}

/// AC2: a worktree whose assignee is a live agent is refused, and `force` does
/// NOT override it — force is a dirty-tree bypass, not a licence to delete a
/// running worker's working directory.
#[tokio::test]
async fn test_worktree_cleanup_refuses_live_assignee_even_with_force() {
    let repo = GitRepo::new();
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);

    let wt_path = cas_root.join("worktrees").join("busy-lynx");
    repo.add_worktree(&wt_path, "factory/busy-lynx");
    merge_worktree_branch_into_default(&repo, &wt_path, "factory/busy-lynx");

    let agent_store = open_agent_store(&cas_root).unwrap();
    let mut agent = Agent::new("agent-busy-lynx".to_string(), "busy-lynx".to_string());
    agent.role = AgentRole::Worker;
    agent.agent_type = AgentType::Worker;
    agent.status = cas::types::AgentStatus::Active;
    agent_store.register(&agent).unwrap();

    let svc = make_service(cas_root.clone());
    let mut req = coord_req("worktree_cleanup");
    req.id = Some("busy-lynx".to_string());
    req.force = Some(true);
    let text = get_text(&svc.coordination(Parameters(req)).await.expect("cleanup"));

    assert!(
        text.contains("Refused") && text.contains("still a live agent"),
        "a live assignee must block removal: {text}"
    );
    assert!(
        text.contains("does NOT override"),
        "the reply must say force cannot bypass this: {text}"
    );
    assert!(
        wt_path.exists(),
        "a refused cleanup must leave the live worker's cwd in place"
    );
}

/// GH #378 / cas-c2a1: System-A CRUD remains opt-in when its flag is off, but
/// the refusal names that real gate rather than claiming factory worktrees are
/// disabled too. Its exact config snippet must parse as the TOML file it names.
#[tokio::test]
async fn test_system_a_crud_refusal_names_real_gate_and_prints_valid_toml() {
    let repo = GitRepo::new();
    let mut env = test_env();
    let cas_root = init_cas_dir(&repo.root, &mut env).expect("init_cas_dir");
    disable_system_a(&cas_root);

    // The reported incident had a live factory already using CAS-managed
    // worktrees. That is System B, which is deliberately separate from this
    // System-A command's configuration gate.
    let factory_worktree = cas_root.join("worktrees").join("already-isolated");
    repo.add_worktree(&factory_worktree, "factory/already-isolated");

    let svc = make_service(cas_root.clone());
    for action in ["worktree_create", "worktree_show"] {
        let mut req = coord_req(action);
        req.id = Some("anything".to_string());
        req.task_id = Some("anything".to_string());
        let text = get_text(&svc.coordination(Parameters(req)).await.expect(action));
        assert!(
            text.contains("System A worktrees are disabled by `[worktrees].enabled`")
                && text.contains(
                    "Factory isolation worktrees use a separate factory `--worktrees` switch"
                )
                && text.contains("coordination action=spawn_workers isolate=true"),
            "{action} must name the System-A gate and the followable factory alternative: {text}"
        );
        assert!(
            !text.contains("experimental and disabled"),
            "{action} must not misdiagnose active factory isolation as experimental-disabled: {text}"
        );

        let snippet_start = text
            .find("[worktrees]\nenabled = true")
            .expect("refusal must print the TOML snippet");
        let snippet_end = text[snippet_start..]
            .find("\n\nFactory isolation")
            .expect("TOML snippet must end before the factory guidance");
        let snippet = &text[snippet_start..snippet_start + snippet_end];
        let parsed: toml::Value = toml::from_str(snippet)
            .expect("the snippet printed for .cas/config.toml must be valid TOML");
        assert_eq!(
            parsed
                .get("worktrees")
                .and_then(toml::Value::as_table)
                .and_then(|worktrees| worktrees.get("enabled"))
                .and_then(toml::Value::as_bool),
            Some(true),
            "the valid TOML snippet must enable the actual System-A gate"
        );
    }
}
