//! Factory MCP Tool Integration Tests
//!
//! Tests the factory MCP tool handlers (`mcp__cas__coordination`) by constructing
//! a `CasService` with a temp CAS directory and calling `factory()` directly.
//! Verifies input validation, queue side effects, and response formatting.
//!
//! # Running
//! Some tests modify process-global environment variables (`CAS_AGENT_ROLE`,
//! `CAS_FACTORY_WORKER_NAMES`). Those tests use a process-wide, poison-tolerant
//! lock, so the target is safe to run with Cargo's default parallelism:
//! ```bash
//! cargo test --test factory_mcp_ops_test -- --nocapture
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use cas::mcp::{CasCore, CasService};
use cas::store::{
    AgentStore, EventStore, PromptQueueStore, SpawnQueueStore, TaskStore, init_cas_dir,
    open_agent_store, open_event_store, open_prompt_queue_store, open_reminder_store,
    open_spawn_queue_store, open_store, open_task_store,
};
use cas::types::{
    Agent, AgentStatus, Entry, Event, EventEntityType, EventType, Task, TaskDepth, TaskStatus,
    TaskType, WorkTarget,
};
use cas_mcp::types::{CoordinationRequest, FactoryRequest};
use cas_mux::{Mux, MuxConfig, SupervisorCli};
use cas_types::AgentRole;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::RawContent;
use tempfile::TempDir;

#[path = "../src/test_env_guard.rs"]
mod test_env_guard;
use test_env_guard::TestEnvGuard;

// =============================================================================
// Test Fixture
// =============================================================================

struct FactoryTestEnv {
    _temp: TempDir,
    cas_root: PathBuf,
    service: CasService,
    // Keep process-global HOME isolation alive for the full fixture lifetime.
    // `None` means an explicit EnvGuard already owns that isolation, or an
    // isolated child supplied HOME/PATH directly on its Command.
    _env_guard: Option<EnvGuard>,
}

impl FactoryTestEnv {
    fn new() -> Self {
        Self::with_agent_id("test-agent-id")
    }

    fn with_agent_id(agent_id: &str) -> Self {
        Self::with_agent_id_and_env(agent_id, EnvGuard::ensure_isolated_home())
    }

    fn with_agent_id_and_env(agent_id: &str, env_guard: Option<EnvGuard>) -> Self {
        let temp = TempDir::new().expect("Failed to create temp dir");
        let cas_root = init_cas_dir(temp.path()).expect("Failed to init CAS dir");

        let core = CasCore::with_daemon(cas_root.clone(), None, None);
        core.set_agent_id_for_testing(agent_id.to_string());
        let service = CasService::new(core, None);

        Self {
            _temp: temp,
            cas_root,
            service,
            _env_guard: env_guard,
        }
    }

    fn without_agent_id() -> Self {
        let env_guard = EnvGuard::ensure_isolated_home();
        let temp = TempDir::new().expect("Failed to create temp dir");
        let cas_root = init_cas_dir(temp.path()).expect("Failed to init CAS dir");
        let core = CasCore::with_daemon(cas_root.clone(), None, None);
        let service = CasService::new(core, None);
        Self {
            _temp: temp,
            cas_root,
            service,
            _env_guard: env_guard,
        }
    }

    /// Build a service whose privileged role was registered independently of
    /// caller-controlled environment/request hints.
    fn with_server_supervisor() -> Self {
        let env = Self::with_agent_id("test-supervisor-id");
        let mut supervisor = Agent::new("test-supervisor-id".to_string(), "supervisor".to_string());
        supervisor.role = AgentRole::Supervisor;
        env.agent_store()
            .register(&supervisor)
            .expect("register server-created supervisor");
        env
    }

    fn create_epic(&self, title: &str) -> String {
        let store = self.task_store();
        let id = store.generate_id().expect("generate_id");
        let mut task = Task::new(id.clone(), title.to_string());
        task.task_type = TaskType::Epic;
        store.add(&task).expect("add epic");
        id
    }

    /// Create a task parked in `AwaitingMerge` assigned to `assignee`
    /// (cas-126b fixtures). Returns the generated task id.
    fn create_awaiting_merge_task(&self, title: &str, assignee: &str) -> String {
        let store = self.task_store();
        let id = store.generate_id().expect("generate_id");
        let mut task = Task::new(id.clone(), title.to_string());
        task.status = TaskStatus::AwaitingMerge;
        task.assignee = Some(assignee.to_string());
        store.add(&task).expect("add awaiting-merge task");
        id
    }

    /// Read a registered worker's `halt_task_work` metadata flag by name.
    fn worker_halted(&self, name: &str) -> bool {
        let agents = self.agent_store().list(None).expect("list agents");
        agents
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name) && a.role == AgentRole::Worker)
            .map(|a| {
                a.metadata
                    .get("halt_task_work")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    fn register_worker(&self, name: &str) -> String {
        let store = self.agent_store();
        let id = Agent::generate_fallback_id();
        let mut agent = Agent::new(id.clone(), name.to_string());
        agent.role = AgentRole::Worker;
        store.register(&agent).expect("register worker");
        id
    }

    fn register_worker_with_id(&self, id: &str, name: &str, factory_session: Option<&str>) {
        let store = self.agent_store();
        let mut agent = Agent::new(id.to_string(), name.to_string());
        agent.role = AgentRole::Worker;
        agent.factory_session = factory_session.map(str::to_string);
        store
            .register(&agent)
            .expect("register worker with fixed id");
    }

    fn register_worker_in_session(&self, name: &str, factory_session: &str) -> String {
        let store = self.agent_store();
        let id = Agent::generate_fallback_id();
        let mut agent = Agent::new(id.clone(), name.to_string());
        agent.role = AgentRole::Worker;
        agent.factory_session = Some(factory_session.to_string());
        store.register(&agent).expect("register worker in session");
        id
    }

    fn register_supervisor_in_session(&self, name: &str, factory_session: &str) -> String {
        let store = self.agent_store();
        let id = Agent::generate_fallback_id();
        let mut agent = Agent::new(id.clone(), name.to_string());
        agent.role = AgentRole::Supervisor;
        agent.factory_session = Some(factory_session.to_string());
        store
            .register(&agent)
            .expect("register supervisor in session");
        id
    }

    fn record_worker_file_event(&self, worker_id: &str, summary: &str) {
        let store = self.event_store();
        let event = Event::new(
            EventType::WorkerFileEdited,
            EventEntityType::Agent,
            worker_id,
            summary.to_string(),
        )
        .with_session(worker_id.to_string());
        store.record(&event).expect("record worker activity");
    }

    fn register_worker_with_metadata(
        &self,
        name: &str,
        metadata: HashMap<String, String>,
    ) -> String {
        let store = self.agent_store();
        let id = Agent::generate_fallback_id();
        let mut agent = Agent::new(id.clone(), name.to_string());
        agent.role = AgentRole::Worker;
        agent.metadata = metadata;
        store.register(&agent).expect("register worker");
        id
    }

    /// Register a worker with its `last_heartbeat` backdated so
    /// `factory_worker_status` classifies it as DEAD (elapsed > 30s).
    ///
    /// Used by the cas-5b1c worker_status integration test to drive the
    /// `[DEAD]` label + transcript-path surfacing branch without waiting
    /// 30 seconds of real time.
    fn register_stale_worker_with_clone_path(
        &self,
        name: &str,
        clone_path: &str,
        stale_secs: i64,
    ) -> String {
        let store = self.agent_store();
        let id = Agent::generate_fallback_id();
        let mut agent = Agent::new(id.clone(), name.to_string());
        agent.role = AgentRole::Worker;
        agent
            .metadata
            .insert("clone_path".to_string(), clone_path.to_string());
        // Backdate BOTH last_heartbeat and registered_at so this fixture
        // survives any future change that adds `registered_at` to the
        // stale-criteria set (adversarial cas-5b1c review A5). Current
        // `list_stale(threshold_secs)` keys on last_heartbeat only, but the
        // fixture is a test-stability anchor — backdating both is cheap
        // insurance against silent regression of the prune criteria.
        let staleness = chrono::Duration::seconds(stale_secs);
        agent.last_heartbeat = chrono::Utc::now() - staleness;
        agent.registered_at = chrono::Utc::now() - staleness;
        store.register(&agent).expect("register stale worker");
        id
    }

    fn register_supervisor(&self, name: &str) -> String {
        let store = self.agent_store();
        let id = Agent::generate_fallback_id();
        let mut agent = Agent::new(id.clone(), name.to_string());
        agent.role = AgentRole::Supervisor;
        store.register(&agent).expect("register supervisor");
        id
    }

    fn register_worker_with_status(&self, name: &str, status: AgentStatus) -> String {
        let store = self.agent_store();
        let id = Agent::generate_fallback_id();
        let mut agent = Agent::new(id.clone(), name.to_string());
        agent.role = AgentRole::Worker;
        agent.status = status;
        store.register(&agent).expect("register worker with status");
        id
    }

    fn agent_store(&self) -> Arc<dyn AgentStore> {
        open_agent_store(&self.cas_root).expect("open agent store")
    }

    fn task_store(&self) -> Arc<dyn TaskStore> {
        open_task_store(&self.cas_root).expect("open task store")
    }

    fn event_store(&self) -> Arc<dyn EventStore> {
        open_event_store(&self.cas_root).expect("open event store")
    }

    fn spawn_queue(&self) -> Arc<dyn SpawnQueueStore> {
        open_spawn_queue_store(&self.cas_root).expect("open spawn queue")
    }

    fn prompt_queue(&self) -> Arc<dyn PromptQueueStore> {
        open_prompt_queue_store(&self.cas_root).expect("open prompt queue")
    }
}

thread_local! {
    /// Existing tests sometimes apply request-specific environment overrides
    /// before constructing `FactoryTestEnv`. Let the fixture reuse that guard
    /// instead of attempting to nest the canonical non-reentrant lock.
    static FACTORY_ENV_ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Factory-specific wrapper around the suite's canonical process-state guard.
///
/// Every `FactoryTestEnv` gets a temporary HOME, making Codex deterministically
/// unavailable without mutating process-global PATH. Tests that explicitly
/// exercise Codex availability run in isolated children whose `Command`
/// supplies both HOME and PATH.
struct EnvGuard {
    _guard: TestEnvGuard,
}

impl EnvGuard {
    fn set(vars: &[(&str, &str)]) -> Self {
        let mut guard = Self::begin();
        for (key, value) in vars {
            guard.set(*key, *value);
        }
        Self { _guard: guard }
    }

    fn set_optional(vars: &[(&str, Option<&str>)]) -> Self {
        let mut guard = Self::begin();
        for (key, value) in vars {
            match value {
                Some(value) => guard.set(*key, *value),
                None => guard.remove(*key),
            }
        }
        Self { _guard: guard }
    }

    fn ensure_isolated_home() -> Option<Self> {
        if FACTORY_ENV_ACTIVE.with(std::cell::Cell::get) {
            None
        } else {
            Some(Self::set(&[]))
        }
    }

    fn begin() -> TestEnvGuard {
        FACTORY_ENV_ACTIVE.with(|active| {
            assert!(
                !active.replace(true),
                "nested factory environment guard; reuse the active fixture guard"
            );
        });
        TestEnvGuard::temp_home()
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        FACTORY_ENV_ACTIVE.with(|active| active.set(false));
    }
}

#[derive(Clone, Copy)]
enum IsolatedCodexState {
    Available,
    Unavailable,
}

/// Run one availability-sensitive integration test in its own process.
///
/// The production probe reads process HOME/PATH internally, so a per-Command
/// environment can reach it only when the whole service call runs in this
/// child. The parent test process never mutates PATH.
fn run_isolated_codex_test(child_test: &str, state: IsolatedCodexState) {
    use std::os::unix::fs::PermissionsExt;

    let home = TempDir::new().expect("isolated Codex child HOME");
    let bin_dir = home.path().join(match state {
        IsolatedCodexState::Available => "fake-bin",
        IsolatedCodexState::Unavailable => "empty-path",
    });
    std::fs::create_dir(&bin_dir).expect("create isolated child PATH");

    if matches!(state, IsolatedCodexState::Available) {
        let codex = bin_dir.join("codex");
        std::fs::write(&codex, "#!/bin/sh\nprintf 'codex-cli 0.0.0-test\\n'\n")
            .expect("write fake codex executable");
        let mut permissions = std::fs::metadata(&codex)
            .expect("stat fake codex executable")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&codex, permissions).expect("chmod fake codex executable");

        let auth = home.path().join(".codex/auth.json");
        std::fs::create_dir_all(auth.parent().expect("auth parent"))
            .expect("create fake codex auth directory");
        std::fs::write(auth, "{}").expect("write fake codex auth marker");
    }

    let output = std::process::Command::new(
        std::env::current_exe().expect("current integration-test executable"),
    )
    .args(["--exact", child_test, "--ignored", "--nocapture"])
    .env("CAS_FACTORY_CODEX_ISOLATED_CHILD", child_test)
    .env("HOME", home.path())
    .env("PATH", &bin_dir)
    .output()
    .expect("spawn isolated Codex integration test");

    assert!(
        output.status.success(),
        "isolated Codex child {child_test} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(child_test) && stdout.contains("test result: ok"),
        "isolated helper did not execute {child_test}:\n{stdout}"
    );
}

fn factory_env_in_isolated_codex_child(child_test: &str) -> FactoryTestEnv {
    assert_eq!(
        std::env::var("CAS_FACTORY_CODEX_ISOLATED_CHILD").as_deref(),
        Ok(child_test),
        "isolated helper must run only through its matching parent test"
    );
    FactoryTestEnv::with_agent_id_and_env("test-agent-id", None)
}

/// cas-5270: the lib-test invariant in `src/lib.rs` covers `src/`; this
/// companion covers every integration test source. A guarded PATH writer is
/// still observable by an unguarded subprocess spawn in another parallel test,
/// so integration tests must use `Command::env` instead of process mutation.
#[test]
fn integration_test_process_path_mutation_is_isolated() {
    fn visit(dir: &std::path::Path, hits: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("read integration-test directory") {
            let path = entry.expect("integration-test entry").path();
            if path.is_dir() {
                visit(&path, hits);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let source = std::fs::read_to_string(&path).expect("read integration-test source");
                for (line_index, line) in source.lines().enumerate() {
                    let mutates_process_path = line.contains("set_var(\"PATH\"")
                        || line.contains("remove_var(\"PATH\"")
                        || line.contains(".set(\"PATH\"")
                        || line.contains(".remove(\"PATH\"")
                        || (line.contains("(\"PATH\",") && !line.contains(".env(\"PATH\","));
                    if mutates_process_path {
                        hits.push(format!("{}:{}", path.display(), line_index + 1));
                    }
                }
            }
        }
    }

    let mut hits = Vec::new();
    visit(
        cas::test_paths::crate_root().join("tests").as_path(),
        &mut hits,
    );
    assert!(
        hits.is_empty(),
        "integration tests must not mutate process-global PATH; use per-Command environment: \
         {hits:?}"
    );
}

fn factory_req(action: &str) -> FactoryRequest {
    FactoryRequest {
        action: action.to_string(),
        id: None,
        count: None,
        worker_names: None,
        task_id: None,
        delivery_mode: None,
        target: None,
        message: None,
        force: None,
        dry_run: None,
        // allow_trunk is CoordinationRequest/worktree_merge only — not FactoryRequest
        clear: None,
        branch: None,
        older_than_secs: None,
        isolate: None,
        remind_message: None,
        remind_delay_secs: None,
        remind_event: None,
        remind_filter: None,
        remind_id: None,
        remind_ttl_secs: None,
        cross_session: None,
        lane: None,
        cli: None,
        model: None,
        effort: None,
        config_dir: None,
        workers: None,
        command: None,
        cwd: None,
        port: None,
        shared: None,
    }
}

fn coord_req(action: &str) -> CoordinationRequest {
    CoordinationRequest {
        action: action.to_string(),
        id: None,
        task_id: None,
        delivery_mode: None,
        merge_request: None,
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

/// cas-e0ab: `session_end` is dispatched on the MCP Tokio runtime, while the
/// hook path retains synchronous title generation that owns another runtime.
/// A session observation exercises that title path; this must return a normal
/// receipt instead of panicking with nested `block_on`.
#[tokio::test]
async fn coordination_session_end_runs_hook_path_off_dispatch_runtime_cas_e0ab() {
    let env = FactoryTestEnv::new();
    let session_id = "session-end-runtime-shape";
    env.register_worker_with_id(session_id, "ending-worker", None);

    let entries = open_store(&env.cas_root).expect("open entry store");
    let entry_id = entries.generate_id().expect("generate entry id");
    let mut entry = Entry::new(entry_id, "session observation".to_string());
    entry.session_id = Some(session_id.to_string());
    entries.add(&entry).expect("add session observation");

    let mut request = coord_req("session_end");
    request.session_id = Some(session_id.to_string());
    let result = env
        .service
        .coordination(Parameters(request))
        .await
        .expect("session_end must return a receipt from MCP dispatch");

    assert!(
        get_text(&result).contains("Session ended: session-end-runtime-shape"),
        "unexpected session_end receipt: {}",
        get_text(&result)
    );
    assert!(
        env.agent_store().get(session_id).is_err(),
        "session_end must unregister the ending agent"
    );
}

/// GH #276: existing callers did not supply task_id. With exactly one active
/// issuer task, remind must infer that context so close quarantines it.
#[tokio::test]
async fn remind_auto_binds_issuer_single_in_progress_task_and_close_quarantines_it() {
    let env = FactoryTestEnv::new();
    env.register_worker_with_id("test-agent-id", "reminder-worker", None);

    let task_store = env.task_store();
    let task_id = task_store.generate_id().unwrap();
    let mut task = Task::new(task_id.clone(), "reminder context".to_string());
    task.assignee = Some("test-agent-id".to_string());
    task.status = TaskStatus::InProgress;
    task.depth = TaskDepth::Light;
    task_store.add(&task).unwrap();

    let mut remind = factory_req("remind");
    remind.remind_message = Some("follow up on discarded draft".to_string());
    remind.remind_delay_secs = Some(300);
    env.service
        .factory(Parameters(remind))
        .await
        .expect("remind should infer the issuer task");

    let reminders = open_reminder_store(&env.cas_root).unwrap();
    let pending = reminders.list_pending("test-agent-id").unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].task_id.as_deref(), Some(task_id.as_str()));

    let close: cas_mcp::types::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "close", "id": task_id, "reason": "done"
    }))
    .unwrap();
    env.service
        .task(Parameters(close))
        .await
        .expect("light task close should succeed");
    assert!(
        reminders.list_pending("test-agent-id").unwrap().is_empty(),
        "task close must quarantine the inferred reminder"
    );
}

/// GH #624: an external reminder is a durable event row, not a one-hour timer
/// tied to the registering factory session.
#[tokio::test]
async fn remind_external_condition_defaults_to_non_expiring_cross_session_row() {
    let env = FactoryTestEnv::new();
    let mut remind = factory_req("remind");
    remind.remind_message = Some("inspect the landed delivery".to_string());
    remind.remind_event = Some("tag_exists".to_string());
    remind.remind_filter = Some(r#"{"tag":"v3.6.0"}"#.to_string());
    remind.cross_session = Some(true);

    let result = env
        .service
        .factory(Parameters(remind))
        .await
        .expect("external reminder should be accepted");
    assert!(get_text(&result).contains("event-based, fires on tag_exists"));

    let reminders = open_reminder_store(&env.cas_root).unwrap();
    let pending = reminders.list_pending("test-agent-id").unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].trigger_event.as_deref(), Some("tag_exists"));
    assert_eq!(pending[0].ttl_secs, 0);
    assert!(pending[0].cross_session);
    assert_eq!(
        pending[0].trigger_filter,
        Some(serde_json::json!({"tag": "v3.6.0"}))
    );
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

fn write_session_metadata(session_name: &str, epic_id: Option<&str>) {
    write_session_metadata_for_project(session_name, epic_id, "/tmp/project");
}

fn write_session_metadata_for_project(
    session_name: &str,
    epic_id: Option<&str>,
    project_dir: &str,
) {
    let path = cas::ui::factory::metadata_path(session_name);
    std::fs::create_dir_all(path.parent().expect("metadata parent")).unwrap();
    let metadata = cas::ui::factory::create_metadata(
        session_name,
        12345,
        "supervisor",
        &[],
        epic_id,
        Some(project_dir),
        None,
    );
    std::fs::write(path, serde_json::to_string_pretty(&metadata).unwrap()).unwrap();
}

fn add_epic_with_id(env: &FactoryTestEnv, id: &str, status: TaskStatus, branch: &str) {
    let mut epic = Task::new(id.to_string(), id.to_string());
    epic.task_type = TaskType::Epic;
    epic.status = status;
    epic.branch = Some(branch.to_string());
    env.task_store().add(&epic).expect("add epic fixture");
}

fn init_sync_repo(env: &FactoryTestEnv, worker: &str) -> PathBuf {
    use std::process::Command;

    fn git(project: &std::path::Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(project)
            .output()
            .expect("run git fixture command");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let project = env.cas_root.parent().expect("project root");
    git(project, &["init", "-b", "main"]);
    git(project, &["config", "user.email", "test@cas"]);
    git(project, &["config", "user.name", "CAS Test"]);
    std::fs::write(project.join("README"), "initial\n").unwrap();
    git(project, &["add", "README"]);
    git(project, &["commit", "-m", "initial"]);

    git(project, &["checkout", "-b", "epic/foreign"]);
    std::fs::write(project.join("foreign.txt"), "foreign\n").unwrap();
    git(project, &["add", "foreign.txt"]);
    git(project, &["commit", "-m", "foreign epic"]);
    git(project, &["checkout", "main"]);

    git(project, &["checkout", "-b", "epic/requested"]);
    std::fs::write(project.join("requested.txt"), "requested\n").unwrap();
    git(project, &["add", "requested.txt"]);
    git(project, &["commit", "-m", "requested epic"]);
    git(project, &["checkout", "main"]);

    let worker_path = env.cas_root.join("worktrees").join(worker);
    std::fs::create_dir_all(worker_path.parent().unwrap()).unwrap();
    git(
        project,
        &[
            "worktree",
            "add",
            "-b",
            &format!("factory/{worker}"),
            worker_path.to_str().unwrap(),
            "main",
        ],
    );
    worker_path
}

fn git_stdout(path: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .expect("run git query");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn read_session_metadata(session_name: &str) -> cas::ui::factory::SessionMetadata {
    let path = cas::ui::factory::metadata_path(session_name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[tokio::test]
async fn test_sync_all_workers_explicit_id_beats_unrelated_in_progress_epic_cas_bfa5() {
    let home = TempDir::new().expect("home tempdir");
    let _guard = EnvGuard::set(&[
        ("CAS_FACTORY_SESSION", "session-sync-explicit"),
        ("HOME", home.path().to_str().unwrap()),
    ]);
    let env = FactoryTestEnv::new();
    let worker = "sync-explicit-worker";
    let worker_path = init_sync_repo(&env, worker);
    env.register_worker_in_session(worker, "session-sync-explicit");
    add_epic_with_id(&env, "cas-3648", TaskStatus::InProgress, "epic/foreign");
    add_epic_with_id(&env, "cas-3b7c", TaskStatus::Open, "epic/requested");
    write_session_metadata_for_project(
        "session-sync-explicit",
        Some("cas-3b7c"),
        env.cas_root.parent().unwrap().to_str().unwrap(),
    );

    let mut req = factory_req("sync_all_workers");
    req.id = Some("cas-3b7c".to_string());
    let result = env
        .service
        .factory(Parameters(req))
        .await
        .expect("explicit requested epic should resolve");
    let text = get_text(&result);

    assert!(text.contains("Sync target: epic/requested"), "{text}");
    assert!(worker_path.join("requested.txt").exists());
    assert!(
        !worker_path.join("foreign.txt").exists(),
        "foreign in-progress epic must not become the sync target"
    );
}

#[tokio::test]
async fn test_sync_all_workers_explicit_epic_uses_recorded_parent_branch_cas_580e() {
    let home = TempDir::new().expect("home tempdir");
    let _guard = EnvGuard::set(&[
        ("CAS_FACTORY_SESSION", "session-sync-recorded-parent"),
        ("HOME", home.path().to_str().unwrap()),
    ]);
    let env = FactoryTestEnv::new();
    let worker = "sync-recorded-parent-worker";
    let _worker_path = init_sync_repo(&env, worker);
    env.register_worker_in_session(worker, "session-sync-recorded-parent");
    add_epic_with_id(&env, "cas-580e-parent", TaskStatus::Open, "epic/requested");
    let store = env.task_store();
    let mut epic = store.get("cas-580e-parent").expect("get epic fixture");
    epic.deliverables.work_target = Some(WorkTarget {
        repo_selector: "project:active-project".to_string(),
        target_branch: "main".to_string(),
    });
    store.update(&epic).expect("record integration parent");

    let mut req = factory_req("sync_all_workers");
    req.id = Some(epic.id);
    let result = env
        .service
        .factory(Parameters(req))
        .await
        .expect("explicit epic should resolve its recorded parent");
    let text = get_text(&result);
    assert!(
        text.contains("Sync target: main"),
        "sync must target the epic's recorded parent, not epic/requested: {text}"
    );
}

#[tokio::test]
async fn test_sync_all_workers_invalid_explicit_id_has_zero_worker_mutations_cas_bfa5() {
    let home = TempDir::new().expect("home tempdir");
    let _guard = EnvGuard::set(&[
        ("CAS_FACTORY_SESSION", "session-sync-invalid"),
        ("HOME", home.path().to_str().unwrap()),
    ]);
    let env = FactoryTestEnv::new();
    let worker = "sync-invalid-worker";
    let worker_path = init_sync_repo(&env, worker);
    env.register_worker_in_session(worker, "session-sync-invalid");
    add_epic_with_id(&env, "cas-3648", TaskStatus::InProgress, "epic/foreign");
    write_session_metadata_for_project(
        "session-sync-invalid",
        Some("cas-3648"),
        env.cas_root.parent().unwrap().to_str().unwrap(),
    );
    std::fs::write(worker_path.join("dirty.txt"), "must survive untouched\n").unwrap();
    let head_before = git_stdout(&worker_path, &["rev-parse", "HEAD"]);
    let status_before = git_stdout(&worker_path, &["status", "--porcelain"]);
    let reflog_before = git_stdout(&worker_path, &["reflog", "show", "--format=%H %gs"]);

    let mut req = factory_req("sync_all_workers");
    req.id = Some("cas-does-not-exist".to_string());
    let err = env
        .service
        .factory(Parameters(req))
        .await
        .expect_err("invalid explicit id must fail closed");
    let err_text = err.to_string();

    assert!(err_text.contains("cas-does-not-exist"), "{err_text}");
    assert_eq!(
        git_stdout(&worker_path, &["rev-parse", "HEAD"]),
        head_before
    );
    assert_eq!(
        git_stdout(&worker_path, &["status", "--porcelain"]),
        status_before
    );
    assert_eq!(
        git_stdout(&worker_path, &["reflog", "show", "--format=%H %gs"]),
        reflog_before,
        "resolution failure must occur before stash/fetch/rebase"
    );
    assert!(!worker_path.join("foreign.txt").exists());
}

#[tokio::test]
async fn test_sync_all_workers_rejects_cross_project_session_focus_cas_bfa5() {
    let home = TempDir::new().expect("home tempdir");
    let _guard = EnvGuard::set(&[
        ("CAS_FACTORY_SESSION", "session-sync-cross-project"),
        ("HOME", home.path().to_str().unwrap()),
    ]);
    let env = FactoryTestEnv::new();
    let worker = "sync-cross-project-worker";
    let worker_path = init_sync_repo(&env, worker);
    env.register_worker_in_session(worker, "session-sync-cross-project");
    add_epic_with_id(&env, "cas-3648", TaskStatus::InProgress, "epic/foreign");
    let other_project = home.path().join("roark-realty");
    std::fs::create_dir_all(&other_project).unwrap();
    write_session_metadata_for_project(
        "session-sync-cross-project",
        Some("cas-3648"),
        other_project.to_str().unwrap(),
    );
    let head_before = git_stdout(&worker_path, &["rev-parse", "HEAD"]);

    let req = factory_req("sync_all_workers");
    let err = env
        .service
        .factory(Parameters(req))
        .await
        .expect_err("cross-project focus must fail closed");
    let err_text = err.to_string();

    assert!(err_text.contains("cross-project"), "{err_text}");
    assert_eq!(
        git_stdout(&worker_path, &["rev-parse", "HEAD"]),
        head_before
    );
    assert!(!worker_path.join("foreign.txt").exists());
}

#[tokio::test]
async fn test_focus_epic_pins_valid_epic_and_records_activity() {
    let home = TempDir::new().expect("home tempdir");
    let home_path = home.path().to_str().unwrap();
    let _guard = EnvGuard::set(&[
        ("CAS_FACTORY_SESSION", "session-focus-pin"),
        ("HOME", home_path),
    ]);
    let env = FactoryTestEnv::new();
    let epic_id = env.create_epic("Focused Epic");
    write_session_metadata("session-focus-pin", Some("cas-session"));

    let mut req = factory_req("focus_epic");
    req.id = Some(epic_id.clone());
    let result = env
        .service
        .factory(Parameters(req))
        .await
        .expect("focus_epic should succeed");

    let text = get_text(&result);
    assert!(
        text.contains(&epic_id),
        "response should name pinned epic: {text}"
    );
    let metadata = read_session_metadata("session-focus-pin");
    assert_eq!(metadata.epic_id, Some("cas-session".to_string()));
    assert_eq!(metadata.pinned_epic_id, Some(epic_id.clone()));

    let events = env.event_store().list_recent(10).unwrap();
    assert!(
        events.iter().any(|event| {
            event.event_type == EventType::SupervisorInjected
                && event.entity_id == epic_id
                && event.session_id.as_deref() == Some("session-focus-pin")
        }),
        "focus_epic should record an activity event"
    );
}

#[tokio::test]
async fn test_focus_epic_rejects_missing_and_non_epic_without_mutation() {
    let home = TempDir::new().expect("home tempdir");
    let home_path = home.path().to_str().unwrap();
    let _guard = EnvGuard::set(&[
        ("CAS_FACTORY_SESSION", "session-focus-invalid"),
        ("HOME", home_path),
    ]);
    let env = FactoryTestEnv::new();
    write_session_metadata("session-focus-invalid", Some("cas-session"));

    let mut missing = factory_req("focus_epic");
    missing.id = None;
    assert!(
        env.service.factory(Parameters(missing)).await.is_err(),
        "missing id without clear=true should fail"
    );
    assert_eq!(
        read_session_metadata("session-focus-invalid").pinned_epic_id,
        None
    );

    let mut nonexistent = factory_req("focus_epic");
    nonexistent.id = Some("cas-does-not-exist".to_string());
    assert!(
        env.service.factory(Parameters(nonexistent)).await.is_err(),
        "nonexistent id should fail"
    );
    assert_eq!(
        read_session_metadata("session-focus-invalid").pinned_epic_id,
        None
    );

    let store = env.task_store();
    let task_id = store.generate_id().expect("generate_id");
    let task = Task::new(task_id.clone(), "Regular Task".to_string());
    store.add(&task).expect("add task");

    let mut non_epic = factory_req("focus_epic");
    non_epic.id = Some(task_id);
    assert!(
        env.service.factory(Parameters(non_epic)).await.is_err(),
        "non-epic id should fail"
    );
    assert_eq!(
        read_session_metadata("session-focus-invalid").pinned_epic_id,
        None
    );
}

#[tokio::test]
async fn test_focus_epic_rejects_closed_epic_without_mutation() {
    let home = TempDir::new().expect("home tempdir");
    let home_path = home.path().to_str().unwrap();
    let _guard = EnvGuard::set(&[
        ("CAS_FACTORY_SESSION", "session-focus-closed"),
        ("HOME", home_path),
    ]);
    let env = FactoryTestEnv::new();
    let epic_id = env.create_epic("Closed Epic");
    let store = env.task_store();
    let mut epic = store.get(&epic_id).expect("get epic");
    epic.status = TaskStatus::Closed;
    store.update(&epic).expect("close epic");
    write_session_metadata("session-focus-closed", Some("cas-session"));

    let mut req = factory_req("focus_epic");
    req.id = Some(epic_id);
    assert!(
        env.service.factory(Parameters(req)).await.is_err(),
        "closed epic id should fail"
    );

    let metadata = read_session_metadata("session-focus-closed");
    assert_eq!(metadata.epic_id, Some("cas-session".to_string()));
    assert_eq!(metadata.pinned_epic_id, None);
}

#[tokio::test]
async fn test_focus_epic_clear_removes_pin_and_preserves_session_default() {
    let home = TempDir::new().expect("home tempdir");
    let home_path = home.path().to_str().unwrap();
    let _guard = EnvGuard::set(&[
        ("CAS_FACTORY_SESSION", "session-focus-clear"),
        ("HOME", home_path),
    ]);
    let env = FactoryTestEnv::new();
    let epic_id = env.create_epic("Focused Epic");
    write_session_metadata("session-focus-clear", Some("cas-session"));

    let mut pin = factory_req("focus_epic");
    pin.id = Some(epic_id);
    env.service
        .factory(Parameters(pin))
        .await
        .expect("pin should succeed");

    let mut clear = factory_req("focus_epic");
    clear.clear = Some(true);
    env.service
        .factory(Parameters(clear))
        .await
        .expect("clear should succeed");

    let metadata = read_session_metadata("session-focus-clear");
    assert_eq!(metadata.epic_id, Some("cas-session".to_string()));
    assert_eq!(metadata.pinned_epic_id, None);

    let events = env.event_store().list_recent(10).unwrap();
    assert!(
        events.iter().any(|event| {
            event.event_type == EventType::SupervisorInjected
                && event.entity_id == "session-focus-clear"
                && event.session_id.as_deref() == Some("session-focus-clear")
        }),
        "clear=true should record a supervisor activity event"
    );
}

#[tokio::test]
async fn test_coordination_focus_epic_routes_clear_field() {
    let home = TempDir::new().expect("home tempdir");
    let home_path = home.path().to_str().unwrap();
    let _guard = EnvGuard::set(&[
        ("CAS_FACTORY_SESSION", "session-focus-coordination"),
        ("HOME", home_path),
    ]);
    let env = FactoryTestEnv::new();
    let epic_id = env.create_epic("Coordination Epic");
    write_session_metadata("session-focus-coordination", Some("cas-session"));

    let mut pin = coord_req("focus_epic");
    pin.id = Some(epic_id.clone());
    env.service
        .coordination(Parameters(pin))
        .await
        .expect("coordination focus_epic should pin");
    assert_eq!(
        read_session_metadata("session-focus-coordination").pinned_epic_id,
        Some(epic_id)
    );

    let mut clear = coord_req("focus_epic");
    clear.clear = Some(true);
    env.service
        .coordination(Parameters(clear))
        .await
        .expect("coordination focus_epic should forward clear=true");
    let metadata = read_session_metadata("session-focus-coordination");
    assert_eq!(metadata.epic_id, Some("cas-session".to_string()));
    assert_eq!(metadata.pinned_epic_id, None);
}

// =============================================================================
// spawn_workers tests
// =============================================================================

#[tokio::test]
async fn test_spawn_workers_requires_epic() {
    let env = FactoryTestEnv::new();

    let req = factory_req("spawn_workers");
    let result = env.service.factory(Parameters(req)).await;

    assert!(result.is_err(), "Should fail without epic");
    let err = result.unwrap_err();
    assert!(
        err.message.contains("No active EPIC"),
        "Error should mention missing EPIC: {}",
        err.message
    );
    // cas-549c (GH #96): the error must point at the no-epic escape hatch
    // instead of only demanding an epic.
    assert!(
        err.message.contains("task_id=<task-id>"),
        "Error should offer the task_id path for standalone work: {}",
        err.message
    );
}

#[test]
fn test_spawn_workers_enqueues_with_epic() {
    run_isolated_codex_test(
        "test_spawn_workers_enqueues_with_epic_in_isolated_child",
        IsolatedCodexState::Available,
    );
}

#[tokio::test]
#[ignore = "subprocess helper for deterministic available-Codex probe"]
async fn test_spawn_workers_enqueues_with_epic_in_isolated_child() {
    let env = factory_env_in_isolated_codex_child(
        "test_spawn_workers_enqueues_with_epic_in_isolated_child",
    );
    env.create_epic("Test Epic");

    let mut req = factory_req("spawn_workers");
    req.count = Some(3);
    req.worker_names = Some("alpha,beta,gamma".to_string());

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok(), "Should succeed with epic");

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("alpha, beta, gamma"),
        "Should list worker names: {text}"
    );

    // Verify queue
    let entries = env.spawn_queue().peek(10).expect("peek");
    assert_eq!(entries.len(), 1, "Should have 1 spawn queue entry");
    assert_eq!(entries[0].action, cas_store::SpawnAction::Spawn);
    assert_eq!(entries[0].worker_names, vec!["alpha", "beta", "gamma"]);
}

#[test]
fn test_spawn_workers_isolate_flag() {
    run_isolated_codex_test(
        "test_spawn_workers_isolate_flag_in_isolated_child",
        IsolatedCodexState::Available,
    );
}

#[tokio::test]
#[ignore = "subprocess helper for deterministic available-Codex probe"]
async fn test_spawn_workers_isolate_flag_in_isolated_child() {
    let env =
        factory_env_in_isolated_codex_child("test_spawn_workers_isolate_flag_in_isolated_child");
    env.create_epic("Test Epic");

    let mut req = factory_req("spawn_workers");
    req.count = Some(2);
    req.isolate = Some(true);

    let result = env
        .service
        .factory(Parameters(req))
        .await
        .expect("stock spawn should succeed");
    let text = get_text(&result);
    assert!(
        text.contains("policy default codex/gpt-5.6-luna/xhigh"),
        "caller-facing response must name the resolved policy fallback: {text}"
    );

    let entries = env.spawn_queue().peek(10).expect("peek");
    assert_eq!(entries.len(), 1);
    assert!(entries[0].isolate, "Should have isolate=true");
}

/// cas-549c (GH #96): after an epic is verified and closed, a standalone
/// follow-on task must be spawnable without inventing a ceremonial
/// single-child epic. The epic gate exists to stop *unscoped* spawning; a
/// concrete open task_id already states the work.
#[test]
fn test_spawn_workers_with_task_id_succeeds_after_epic_closed() {
    run_isolated_codex_test(
        "test_spawn_workers_with_task_id_succeeds_after_epic_closed_in_isolated_child",
        IsolatedCodexState::Available,
    );
}

#[tokio::test]
#[ignore = "subprocess helper for deterministic available-Codex probe"]
async fn test_spawn_workers_with_task_id_succeeds_after_epic_closed_in_isolated_child() {
    let env = factory_env_in_isolated_codex_child(
        "test_spawn_workers_with_task_id_succeeds_after_epic_closed_in_isolated_child",
    );
    let task_store = env.task_store();

    // The exact reported sequence: an epic existed, was completed, and closed.
    let epic_id = env.create_epic("Finished Epic");
    let mut epic = task_store.get(&epic_id).expect("get epic");
    epic.status = TaskStatus::Closed;
    task_store.update(&epic).expect("close epic");

    // New evidence arrives; a standalone follow-on task is created.
    let task_id = task_store.generate_id().expect("generate_id");
    task_store
        .add(&Task::new(
            task_id.clone(),
            "Post-epic follow-up".to_string(),
        ))
        .expect("add follow-up task");

    let mut req = factory_req("spawn_workers");
    req.count = Some(1);
    req.task_id = Some(task_id.clone());

    let result = env.service.factory(Parameters(req)).await;
    assert!(
        result.is_ok(),
        "spawn_workers with a concrete open task_id must not require an epic: {result:?}"
    );

    let entries = env.spawn_queue().peek(10).expect("peek");
    assert_eq!(entries.len(), 1, "the spawn must actually be queued");
    assert_eq!(entries[0].task_id.as_deref(), Some(task_id.as_str()));
}

/// cas-549c: the same relaxation must hold when no epic has ever existed —
/// a fresh project with one task, not just a post-close one.
#[tokio::test]
async fn test_spawn_workers_with_task_id_succeeds_with_no_epic_at_all() {
    let env = FactoryTestEnv::new();
    let task_store = env.task_store();
    let task_id = task_store.generate_id().expect("generate_id");
    task_store
        .add(&Task::new(task_id.clone(), "Standalone".to_string()))
        .expect("add task");

    let mut req = factory_req("spawn_workers");
    req.worker_names = Some("swift-fox".to_string());
    req.task_id = Some(task_id.clone());
    req.cli = Some("claude".to_string());

    let result = env.service.factory(Parameters(req)).await;
    assert!(
        result.is_ok(),
        "a single named worker for an open task needs no epic: {result:?}"
    );
    assert_eq!(env.spawn_queue().peek(10).expect("peek").len(), 1);
}

/// cas-549c review follow-up: standing in for an EPIC is a stronger claim
/// than being a legal pre-assignment target. A task that a newly spawned
/// worker cannot actually pick up — owned by a *live* worker, or
/// parked awaiting the supervisor — must NOT authorize an epic-free spawn,
/// or the factory boots a pane and worktree for a worker that then sits
/// permanently idle (assign_task_to_new_worker refuses to steal a live
/// assignee). A missing/dead holder is intentionally dispatchable under
/// cas-2327's stale-holder reset contract.
#[tokio::test]
async fn test_spawn_workers_undispatchable_task_id_is_rejected_without_epic() {
    let undispatchable = [
        (
            TaskStatus::AwaitingMerge,
            None,
            "not work a newly spawned worker",
        ),
        (TaskStatus::Blocked, None, "not work a newly spawned worker"),
        (
            TaskStatus::Open,
            Some("alpha"),
            "already assigned to live worker 'alpha'",
        ),
        (
            TaskStatus::InProgress,
            Some("alpha"),
            "already assigned to live worker 'alpha'",
        ),
    ];

    for (status, assignee, expected) in undispatchable {
        let env = FactoryTestEnv::new();
        if let Some(holder) = assignee {
            // cas-2327 makes an unregistered holder stale and resettable, so
            // register this fixture holder to retain its live-owner contract.
            let mut agent = Agent::new("alpha-agent-id".to_string(), holder.to_string());
            agent.role = AgentRole::Worker;
            env.agent_store()
                .register(&agent)
                .expect("register live holder");
        }
        let task_store = env.task_store();
        let id = task_store.generate_id().expect("generate_id");
        let mut task = Task::new(id.clone(), format!("{status:?} task"));
        task.status = status;
        task.assignee = assignee.map(str::to_string);
        task_store.add(&task).expect("add task");

        let mut req = factory_req("spawn_workers");
        req.count = Some(1);
        req.task_id = Some(id.clone());
        let err = env
            .service
            .factory(Parameters(req))
            .await
            .expect_err(&format!(
                "{status:?} (assignee={assignee:?}) must not authorize an epic-free spawn"
            ));
        assert!(
            err.message.contains(expected),
            "{status:?} (assignee={assignee:?}) error should say why: {}",
            err.message
        );
        assert!(
            env.spawn_queue().peek(10).expect("peek").is_empty(),
            "{status:?} must not enqueue a spawn"
        );
    }
}

/// cas-549c review follow-up: the SAME statuses must still be accepted when
/// an epic is open — the tightening only ever withholds the epic bypass, it
/// must not change pre-assignment rules for an epic-backed factory.
#[test]
fn test_undispatchable_task_id_still_allowed_when_an_epic_is_open() {
    run_isolated_codex_test(
        "test_undispatchable_task_id_still_allowed_when_an_epic_is_open_in_isolated_child",
        IsolatedCodexState::Available,
    );
}

#[tokio::test]
#[ignore = "subprocess helper for deterministic available-Codex probe"]
async fn test_undispatchable_task_id_still_allowed_when_an_epic_is_open_in_isolated_child() {
    for (status, assignee) in [
        (TaskStatus::AwaitingMerge, None),
        (TaskStatus::Open, Some("alpha")),
        (TaskStatus::InProgress, Some("alpha")),
    ] {
        let env = factory_env_in_isolated_codex_child(
            "test_undispatchable_task_id_still_allowed_when_an_epic_is_open_in_isolated_child",
        );
        env.create_epic("Live Epic");
        let task_store = env.task_store();
        let id = task_store.generate_id().expect("generate_id");
        let mut task = Task::new(id.clone(), format!("{status:?} task"));
        task.status = status;
        task.assignee = assignee.map(str::to_string);
        task_store.add(&task).expect("add task");

        let mut req = factory_req("spawn_workers");
        req.count = Some(1);
        req.task_id = Some(id.clone());
        assert!(
            env.service.factory(Parameters(req)).await.is_ok(),
            "with an open epic, {status:?} (assignee={assignee:?}) must behave exactly as before"
        );
        assert_eq!(env.spawn_queue().peek(10).expect("peek").len(), 1);
    }
}

/// cas-549c review follow-up: one task authorizes ONE spawn. Nothing in the
/// MCP call mutates the task (binding happens at worker registration), so
/// without a duplicate guard a single open task_id could authorize an
/// unbounded burst of epic-free spawns where only the first worker binds.
#[tokio::test]
async fn test_task_id_authorizes_only_one_epic_free_spawn() {
    let env = FactoryTestEnv::new();
    let task_store = env.task_store();
    let task_id = task_store.generate_id().expect("generate_id");
    task_store
        .add(&Task::new(task_id.clone(), "Only once".to_string()))
        .expect("add task");

    let mut first = factory_req("spawn_workers");
    first.count = Some(1);
    first.task_id = Some(task_id.clone());
    first.cli = Some("claude".to_string());
    env.service
        .factory(Parameters(first))
        .await
        .expect("first epic-free spawn should be authorized");

    let mut second = factory_req("spawn_workers");
    second.count = Some(1);
    second.task_id = Some(task_id.clone());
    let err = env
        .service
        .factory(Parameters(second))
        .await
        .expect_err("a second spawn for the same queued task must be refused");
    assert!(
        err.message.contains("already queued"),
        "error should name the pending spawn: {}",
        err.message
    );
    assert_eq!(
        env.spawn_queue().peek(10).expect("peek").len(),
        1,
        "the duplicate must not enqueue a second row"
    );
}

/// cas-549c: the relaxation is scoped to a *valid* task_id. A closed task,
/// a nonexistent one, or an ambiguous multi-worker request must still be
/// rejected with no epic present — otherwise task_id becomes a blanket
/// bypass of the unscoped-spawn guard.
#[tokio::test]
async fn test_spawn_workers_task_id_bypass_requires_a_valid_open_task() {
    let env = FactoryTestEnv::new();
    let task_store = env.task_store();

    let closed_id = task_store.generate_id().expect("generate_id");
    let mut closed = Task::new(closed_id.clone(), "Already done".to_string());
    closed.status = TaskStatus::Closed;
    task_store.add(&closed).expect("add closed task");

    let open_id = task_store.generate_id().expect("generate_id");
    task_store
        .add(&Task::new(open_id.clone(), "Open".to_string()))
        .expect("add open task");

    // Closed task: rejected on the task, not waved through as "authorized".
    let mut req = factory_req("spawn_workers");
    req.count = Some(1);
    req.task_id = Some(closed_id.clone());
    let err = env
        .service
        .factory(Parameters(req))
        .await
        .expect_err("a closed task must not authorize a spawn");
    assert!(
        err.message.contains(&closed_id) && err.message.contains("terminal (closed)"),
        "error should name the closed terminal task and status: {}",
        err.message
    );

    // Unknown task id.
    let mut req = factory_req("spawn_workers");
    req.count = Some(1);
    req.task_id = Some("cas-doesnotexist".to_string());
    let err = env
        .service
        .factory(Parameters(req))
        .await
        .expect_err("an unknown task must not authorize a spawn");
    assert!(
        err.message.contains("no such task"),
        "error should say the task is unknown: {}",
        err.message
    );

    // Ambiguous multi-worker request keeps its own error even with no epic.
    let mut req = factory_req("spawn_workers");
    req.count = Some(3);
    req.task_id = Some(open_id);
    let err = env
        .service
        .factory(Parameters(req))
        .await
        .expect_err("multi-worker + task_id stays ambiguous");
    assert!(
        err.message.contains("single-worker"),
        "error should explain the single-worker requirement: {}",
        err.message
    );

    assert!(
        env.spawn_queue().peek(10).expect("peek").is_empty(),
        "no rejected request may enqueue anything"
    );
}

/// cas-6913 AC3: `task_id` on a single-worker spawn request must carry
/// through to the queued `SpawnRequest`, ready for `finish_worker_spawn` to
/// pick up once the daemon actually spawns the worker (unit-tested
/// separately in epic_workers.rs — this test covers the MCP-to-queue leg).
#[tokio::test]
async fn test_spawn_workers_task_id_enqueues_for_single_worker() {
    let env = FactoryTestEnv::new();
    env.create_epic("Test Epic");
    let task_store = env.task_store();
    let task_id = task_store.generate_id().expect("generate_id");
    task_store
        .add(&Task::new(task_id.clone(), "Pre-assign me".to_string()))
        .expect("add task");

    let mut req = factory_req("spawn_workers");
    req.count = Some(1);
    req.task_id = Some(task_id.clone());
    req.cli = Some("claude".to_string());

    let result = env.service.factory(Parameters(req)).await;
    assert!(
        result.is_ok(),
        "single-worker spawn with task_id should succeed: {result:?}"
    );
    let text = get_text(&result.unwrap());
    assert!(
        text.contains(&task_id),
        "response should mention the pre-assigned task: {text}"
    );

    let entries = env.spawn_queue().peek(10).expect("peek");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].task_id.as_deref(), Some(task_id.as_str()));
}

/// GH #170: a task held by a vanished session remains dispatchable. The queue
/// receipt makes the pending reset explicit; registration performs the reset
/// before the new worker is bound (covered in queue_and_events).
#[tokio::test]
async fn test_spawn_workers_task_id_accepts_stale_assignee_for_reset_preassign() {
    let env = FactoryTestEnv::new();
    let task_id = env.task_store().generate_id().expect("generate_id");
    let mut task = Task::new(task_id.clone(), "Orphaned but pushed work".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("dead-session-worker".to_string());
    task.notes = "Keep this history".to_string();
    task.branch = Some("factory/dead-session-worker".to_string());
    env.task_store().add(&task).expect("add stale task");

    let mut req = factory_req("spawn_workers");
    req.task_id = Some(task_id.clone());
    req.cli = Some("claude".to_string());
    let text = get_text(
        &env.service
            .factory(Parameters(req))
            .await
            .expect("stale holder must not strand a replacement spawn"),
    );

    assert!(text.contains("dead-session-worker"), "{text}");
    assert!(text.contains("force-released"), "{text}");
    assert_eq!(
        env.spawn_queue().peek(10).unwrap()[0].task_id.as_deref(),
        Some(task_id.as_str())
    );
}

/// A fresh heartbeat is an authoritative live-holder signal even without a
/// recorded harness PID. It must fail before anything reaches the spawn queue.
#[tokio::test]
async fn test_spawn_workers_task_id_refuses_fresh_heartbeat_holder() {
    let env = FactoryTestEnv::new();
    let holder = "fresh-heartbeat-worker";
    env.register_worker(holder);
    let task_id = env.task_store().generate_id().expect("generate_id");
    let mut task = Task::new(task_id.clone(), "Live holder".to_string());
    task.assignee = Some(holder.to_string());
    env.task_store().add(&task).expect("add held task");

    let mut req = factory_req("spawn_workers");
    req.task_id = Some(task_id);
    let error = env
        .service
        .factory(Parameters(req))
        .await
        .expect_err("fresh holder must reject replacement spawn");

    assert!(error.message.contains(holder), "{}", error.message);
    assert!(env.spawn_queue().peek(10).unwrap().is_empty());
}

/// cas-6913 AC3: task_id with a single explicit worker_names entry is also
/// a valid "single worker" request (not just count=1).
#[tokio::test]
async fn test_spawn_workers_task_id_enqueues_for_single_named_worker() {
    let env = FactoryTestEnv::new();
    env.create_epic("Test Epic");
    let task_store = env.task_store();
    let task_id = task_store.generate_id().expect("generate_id");
    task_store
        .add(&Task::new(task_id.clone(), "Pre-assign me".to_string()))
        .expect("add task");

    let mut req = factory_req("spawn_workers");
    req.worker_names = Some("swift-fox".to_string());
    req.task_id = Some(task_id.clone());
    req.cli = Some("claude".to_string());

    let result = env.service.factory(Parameters(req)).await;
    assert!(
        result.is_ok(),
        "single named-worker spawn with task_id should succeed: {result:?}"
    );

    let entries = env.spawn_queue().peek(10).expect("peek");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].task_id.as_deref(), Some(task_id.as_str()));
}

/// cas-6913: task_id must be rejected (not silently ignored or applied to
/// only one of several) when the spawn request is ambiguous about which
/// worker "the" spawned worker is.
#[tokio::test]
async fn test_spawn_workers_task_id_rejects_multi_worker_count() {
    let env = FactoryTestEnv::new();
    env.create_epic("Test Epic");
    let task_store = env.task_store();
    let task_id = task_store.generate_id().expect("generate_id");
    task_store
        .add(&Task::new(task_id.clone(), "Ambiguous".to_string()))
        .expect("add task");

    let mut req = factory_req("spawn_workers");
    req.count = Some(3);
    req.task_id = Some(task_id);

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_err(), "task_id with count>1 must be rejected");
    let err = result.unwrap_err();
    assert!(
        err.message.contains("single-worker"),
        "error should explain the single-worker requirement: {}",
        err.message
    );

    assert!(
        env.spawn_queue().peek(10).expect("peek").is_empty(),
        "rejected request must not enqueue anything"
    );
}

/// cas-6913: same ambiguity guard, via worker_names listing more than one name.
#[tokio::test]
async fn test_spawn_workers_task_id_rejects_multi_worker_names() {
    let env = FactoryTestEnv::new();
    env.create_epic("Test Epic");
    let task_store = env.task_store();
    let task_id = task_store.generate_id().expect("generate_id");
    task_store
        .add(&Task::new(task_id.clone(), "Ambiguous".to_string()))
        .expect("add task");

    let mut req = factory_req("spawn_workers");
    req.worker_names = Some("alpha,beta".to_string());
    req.task_id = Some(task_id);

    let result = env.service.factory(Parameters(req)).await;
    assert!(
        result.is_err(),
        "task_id with 2 worker_names must be rejected"
    );
}

/// cas-6913: task_id referencing a task that doesn't exist must fail fast
/// with a clear error, not silently queue a spawn request that can never
/// resolve the assignment.
#[tokio::test]
async fn test_spawn_workers_task_id_rejects_unknown_task() {
    let env = FactoryTestEnv::new();
    env.create_epic("Test Epic");

    let mut req = factory_req("spawn_workers");
    req.count = Some(1);
    req.task_id = Some("cas-doesnotexist".to_string());

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_err(), "unknown task_id must be rejected");
    let err = result.unwrap_err();
    assert!(
        err.message.contains("not found"),
        "error should say the task wasn't found: {}",
        err.message
    );
}

/// cas-6913: task_id referencing an already-closed task must be rejected —
/// pre-assigning a spawned worker to dead work is never useful.
#[tokio::test]
async fn test_spawn_workers_task_id_rejects_closed_task() {
    let env = FactoryTestEnv::new();
    env.create_epic("Test Epic");
    let task_store = env.task_store();
    let task_id = task_store.generate_id().expect("generate_id");
    let mut task = Task::new(task_id.clone(), "Already done".to_string());
    task.status = TaskStatus::Closed;
    task_store.add(&task).expect("add closed task");

    let mut req = factory_req("spawn_workers");
    req.count = Some(1);
    req.task_id = Some(task_id);

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_err(), "closed task_id must be rejected");
    let err = result.unwrap_err();
    assert!(
        err.message.contains("closed"),
        "error should say the task is closed: {}",
        err.message
    );
}

#[tokio::test]
async fn test_spawn_workers_closed_epic_not_counted() {
    let env = FactoryTestEnv::new();

    // Create an epic and close it
    let epic_id = env.create_epic("Closed Epic");
    let store = env.task_store();
    let mut task = store.get(&epic_id).expect("get epic");
    task.status = TaskStatus::Closed;
    store.update(&task).expect("close epic");

    let req = factory_req("spawn_workers");
    let result = env.service.factory(Parameters(req)).await;

    assert!(result.is_err(), "Closed epic should not count as active");
}

// cas-2992: spawn_workers with cli/model/effort overrides
#[test]
fn test_spawn_workers_codex_available_enqueues_codex_spec() {
    run_isolated_codex_test(
        "test_spawn_workers_codex_available_enqueues_codex_spec_in_isolated_child",
        IsolatedCodexState::Available,
    );
}

#[tokio::test]
#[ignore = "subprocess helper for deterministic available-Codex probe"]
async fn test_spawn_workers_codex_available_enqueues_codex_spec_in_isolated_child() {
    // The child provides both availability signals hermetically: a fake
    // `codex --version` executable and a temp-HOME auth marker. The queued
    // SpawnRequest.worker_spec must therefore preserve the requested harness.
    let env = factory_env_in_isolated_codex_child(
        "test_spawn_workers_codex_available_enqueues_codex_spec_in_isolated_child",
    );
    env.create_epic("Test Epic");

    let fake_version = std::process::Command::new("codex")
        .arg("--version")
        .output()
        .expect("controlled fake codex must resolve");
    assert_eq!(
        String::from_utf8_lossy(&fake_version.stdout),
        "codex-cli 0.0.0-test\n",
        "the fixture must resolve its fake codex, never the host binary"
    );
    let auth =
        PathBuf::from(std::env::var_os("HOME").expect("controlled HOME")).join(".codex/auth.json");
    assert!(auth.is_file(), "controlled auth marker must exist");

    let mut req = factory_req("spawn_workers");
    req.count = Some(1);
    req.cli = Some("codex".to_string());

    let result = env.service.factory(Parameters(req)).await;
    assert!(
        result.is_ok(),
        "spawn_workers with cli=codex should succeed"
    );

    let entries = env.spawn_queue().peek(10).expect("peek");
    assert_eq!(entries.len(), 1, "should have one queue entry");

    let spec_json = entries[0]
        .worker_spec
        .as_deref()
        .expect("worker_spec should be set when cli override given");
    assert!(
        spec_json.contains("codex"),
        "spec JSON should mention 'codex': {spec_json}"
    );
}

#[test]
fn test_spawn_workers_codex_unavailable_fails_loudly() {
    run_isolated_codex_test(
        "test_spawn_workers_codex_unavailable_fails_loudly_in_isolated_child",
        IsolatedCodexState::Unavailable,
    );
}

#[tokio::test]
#[ignore = "subprocess helper for deterministic unavailable-Codex probe"]
async fn test_spawn_workers_codex_unavailable_fails_loudly_in_isolated_child() {
    // The child removes BOTH independent availability signals: PATH has no
    // codex executable and temp HOME has no auth marker. This deliberately
    // exercises the real public probe + fallback composition.
    let env = factory_env_in_isolated_codex_child(
        "test_spawn_workers_codex_unavailable_fails_loudly_in_isolated_child",
    );
    env.create_epic("Test Epic");

    let binary_error = std::process::Command::new("codex")
        .arg("--version")
        .output()
        .expect_err("controlled unavailable PATH must not resolve host codex");
    assert_eq!(binary_error.kind(), std::io::ErrorKind::NotFound);
    let auth =
        PathBuf::from(std::env::var_os("HOME").expect("controlled HOME")).join(".codex/auth.json");
    assert!(
        !auth.is_file(),
        "controlled unavailable HOME must not contain auth"
    );

    let mut req = factory_req("spawn_workers");
    req.count = Some(1);
    req.cli = Some("codex".to_string());

    let error = env
        .service
        .factory(Parameters(req))
        .await
        .expect_err("unavailable Codex must refuse rather than substitute Claude");
    assert!(
        error.message.contains("codex is unavailable")
            && error.message.contains("refusing to silently fall back"),
        "caller must see the loud refusal: {}",
        error.message
    );
    assert!(env.spawn_queue().peek(10).expect("peek").is_empty());
}

#[tokio::test]
async fn test_spawn_workers_invalid_cli_returns_error() {
    // An unrecognised cli value should return an MCP error, not silently use defaults.
    let env = FactoryTestEnv::new();
    env.create_epic("Test Epic");

    let mut req = factory_req("spawn_workers");
    req.count = Some(1);
    req.cli = Some("openai".to_string()); // invalid

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_err(), "invalid cli should return error");
    let err = result.unwrap_err();
    assert!(
        err.message.contains("openai") || err.message.contains("cli"),
        "error should mention the invalid value: {}",
        err.message
    );
}

#[test]
fn test_spawn_workers_no_cli_override_queues_safe_worker_spec() {
    run_isolated_codex_test(
        "test_spawn_workers_no_cli_override_queues_safe_worker_spec_in_isolated_child",
        IsolatedCodexState::Available,
    );
}

#[tokio::test]
#[ignore = "subprocess helper for deterministic available-Codex probe"]
async fn test_spawn_workers_no_cli_override_queues_safe_worker_spec_in_isolated_child() {
    // Without cli/model/effort fields, worker_spec resolves to the safe worker
    // floor instead of inheriting the supervisor session defaults.
    let env = factory_env_in_isolated_codex_child(
        "test_spawn_workers_no_cli_override_queues_safe_worker_spec_in_isolated_child",
    );
    env.create_epic("Test Epic");

    let mut req = factory_req("spawn_workers");
    req.count = Some(2);

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let entries = env.spawn_queue().peek(10).expect("peek");
    assert_eq!(entries.len(), 1);
    let spec_json = entries[0]
        .worker_spec
        .as_deref()
        .expect("no cli/model/effort should still queue a resolved worker_spec");
    let spec: cas_mux::WorkerSpec = serde_json::from_str(spec_json).expect("valid WorkerSpec");
    assert_eq!(spec.cli, cas_mux::SupervisorCli::Codex);
    assert_eq!(spec.model.as_deref(), Some(cas::config::STOCK_WORKER_MODEL));
    assert_eq!(spec.effort, Some(cas_mux::Effort::XHigh));
}

// =============================================================================
// shutdown_workers tests
// =============================================================================

#[tokio::test]
async fn test_shutdown_workers_rejects_unsupported_known_param_without_queueing() {
    let env = FactoryTestEnv::new();
    env.register_worker("alice");

    let mut req = coord_req("shutdown_workers");
    req.worker_names = Some("alice".to_string());
    // task_id is a valid field in the unified request, but not for this
    // destructive action. It must not disappear during domain conversion.
    req.task_id = Some("cas-wrong-field".to_string());

    let result = env.service.coordination(Parameters(req)).await;
    let err = result.expect_err("unsupported shutdown field must hard-error");
    assert!(err.message.contains("task_id"), "unexpected error: {err:?}");
    assert!(
        env.spawn_queue().peek(10).expect("peek").is_empty(),
        "a rejected destructive request must queue nothing"
    );
}

#[tokio::test]
async fn test_shutdown_workers_id_targets_exact_worker() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    env.register_worker("alice");
    env.register_worker("bob");

    let mut req = coord_req("shutdown_workers");
    // GH #197's incident shape: id carried the display name. Before the fix
    // this field was ignored and the empty selector expanded to ALL.
    req.id = Some("alice".to_string());

    let result = env
        .service
        .coordination(Parameters(req))
        .await
        .expect("id target should be accepted");
    let text = get_text(&result);
    assert!(text.contains("alice"), "receipt must name target: {text}");
    assert!(
        text.contains("tasks=[none]"),
        "receipt must show task state: {text}"
    );

    let entries = env.spawn_queue().peek(10).expect("peek");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].worker_names, vec!["alice"]);
    assert!(!entries[0].worker_names.contains(&"bob".to_string()));
}

#[tokio::test]
async fn test_shutdown_workers_mid_task_requires_force_and_receipt_enumerates_state() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    env.register_worker("alice");
    let mut task = Task::new("cas-active".to_string(), "active work".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("alice".to_string());
    env.task_store().add(&task).expect("add task");

    let mut req = factory_req("shutdown_workers");
    req.worker_names = Some("alice".to_string());
    let err = env
        .service
        .factory(Parameters(req.clone()))
        .await
        .expect_err("mid-task shutdown must require force");
    assert!(
        err.message.contains("force=true"),
        "unexpected error: {err:?}"
    );
    assert!(
        err.message.contains("cas-active"),
        "task state missing: {err:?}"
    );
    assert!(env.spawn_queue().peek(10).expect("peek").is_empty());

    req.force = Some(true);
    let result = env
        .service
        .factory(Parameters(req))
        .await
        .expect("force should authorize exact target");
    let text = get_text(&result);
    assert!(
        text.contains("alice"),
        "worker missing from receipt: {text}"
    );
    assert!(
        text.contains("cas-active [in_progress]"),
        "task missing: {text}"
    );
    assert!(text.contains("worktree="), "worktree state missing: {text}");
}

#[tokio::test]
async fn test_shutdown_workers_dirty_or_unpushed_worktree_requires_force() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    let worker_path = init_sync_repo(&env, "alice");
    let mut metadata = HashMap::new();
    metadata.insert("clone_path".to_string(), worker_path.display().to_string());
    env.register_worker_with_metadata("alice", metadata);
    std::fs::write(worker_path.join("uncommitted.txt"), "live WIP\n").unwrap();

    let mut req = factory_req("shutdown_workers");
    req.worker_names = Some("alice".to_string());
    let err = env
        .service
        .factory(Parameters(req))
        .await
        .expect_err("dirty/unpushed shutdown must require force");
    assert!(
        err.message.contains("force=true"),
        "unexpected error: {err:?}"
    );
    assert!(
        err.message.contains("dirty_files=1"),
        "dirty state missing: {err:?}"
    );
    assert!(
        err.message.contains("unpushed_commits="),
        "unpushed state missing: {err:?}"
    );
    assert!(env.spawn_queue().peek(10).expect("peek").is_empty());
}

#[tokio::test]
async fn test_shutdown_workers_validates_existence() {
    let env = FactoryTestEnv::new();
    env.register_worker("alice");

    let mut req = factory_req("shutdown_workers");
    req.worker_names = Some("alice,charlie".to_string());

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_err(), "Should fail for nonexistent worker");

    let err = result.unwrap_err();
    assert!(
        err.message.contains("charlie"),
        "Error should mention missing worker: {}",
        err.message
    );
}

#[tokio::test]
async fn test_shutdown_workers_enqueues() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    env.register_worker("alice");
    env.register_worker("bob");

    let mut req = factory_req("shutdown_workers");
    req.worker_names = Some("alice,bob".to_string());

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(text.contains("alice, bob"), "Should list workers: {text}");

    let entries = env.spawn_queue().peek(10).expect("peek");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].action, cas_store::SpawnAction::Shutdown);
    assert!(entries[0].worker_names.contains(&"alice".to_string()));
    assert!(entries[0].worker_names.contains(&"bob".to_string()));
}

#[tokio::test]
async fn test_shutdown_workers_all() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    env.register_worker("alice");

    let mut req = factory_req("shutdown_workers");
    req.count = Some(0);

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(text.contains("ALL workers"), "Should say ALL: {text}");
}

#[tokio::test]
async fn test_shutdown_workers_supervisor_scoping() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_FACTORY_WORKER_NAMES", "owned-1"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_worker("owned-1");
    env.register_worker("other-1");

    // Empty worker_names should auto-scope to owned workers
    let req = factory_req("shutdown_workers");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let entries = env.spawn_queue().peek(10).expect("peek");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].worker_names, vec!["owned-1"]);
}

// =============================================================================
// worker_status tests
// =============================================================================

#[tokio::test]
async fn test_worker_status_empty() {
    let env = FactoryTestEnv::new();

    let req = factory_req("worker_status");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("No active agents"),
        "Should report no agents: {text}"
    );
}

#[tokio::test]
async fn cas_a736_worker_status_reconciles_the_full_terminal_relay_backlog() {
    let env = FactoryTestEnv::new();
    let task_id = "cas-terminal-relay-backlog";
    let mut task = Task::new(task_id.to_string(), "terminal relay backlog".to_string());
    env.task_store().add(&task).expect("add terminal task");
    task.status = TaskStatus::Closed;
    task.closed_at = Some(chrono::Utc::now());
    env.task_store().update(&task).expect("close task");

    let queue = env.prompt_queue();
    for index in 0..12 {
        let prompt_id = queue
            .enqueue_full(
                &format!("lifecycle-wake:{}", 6000 + index),
                "supervisor",
                "<task-lifecycle transition=\"task_awaiting_merge\">",
                Some("historical-session"),
                Some(&format!("task_awaiting_merge: {task_id} ({index})")),
                None,
            )
            .expect("enqueue historical relay");
        queue
            .mark_suppressed(prompt_id, Some("historical lifecycle occurrence expired"))
            .expect("suppress historical relay");
    }
    assert_eq!(
        queue.list_undelivered_lifecycle_relays(10).unwrap().len(),
        10
    );

    let text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("worker_status"),
    );
    assert!(
        !text.contains("UNDELIVERED SUPERVISOR RELAY"),
        "the terminal backlog must fully self-reconcile rather than leave a banner: {text}"
    );
    assert!(
        queue
            .list_undelivered_lifecycle_relays(10)
            .unwrap()
            .is_empty(),
        "a second status read must not reintroduce an acknowledged terminal relay"
    );
}

#[tokio::test]
async fn test_worker_status_shows_agents() {
    // Acquire env mutex to prevent concurrent tests from setting CAS_AGENT_ROLE=supervisor
    // which would activate supervisor scoping and filter out our test workers.
    let _guard = EnvGuard::set(&[]);

    let env = FactoryTestEnv::new();
    env.register_supervisor("sup-1");

    let mut meta = HashMap::new();
    meta.insert("clone_path".to_string(), "/tmp/worktree/wolf".to_string());
    meta.insert("worker_model".to_string(), "sonnet".to_string());
    meta.insert("worker_effort".to_string(), "high".to_string());
    env.register_worker_with_metadata("wolf", meta);
    env.register_worker("fox");

    let req = factory_req("worker_status");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("Workers (2)"),
        "Should show 2 workers: {text}"
    );
    assert!(text.contains("wolf"), "Should list wolf: {text}");
    assert!(text.contains("fox"), "Should list fox: {text}");
    assert!(
        text.contains("/tmp/worktree/wolf"),
        "Should show clone path: {text}"
    );
    assert!(text.contains("model: sonnet"), "Should show model: {text}");
    assert!(text.contains("effort: high"), "Should show effort: {text}");
}

/// cas-fa38: a headless knowledge-build child used to inherit the parent
/// worker name/factory session and create another Active row. Even after the
/// worktree disappeared, every row rendered independently with a fresh-looking
/// heartbeat. Status must expose one authoritative identity.
#[tokio::test]
async fn test_worker_status_dedupes_nested_identity_with_missing_worktree() {
    let _guard = EnvGuard::set(&[("CAS_FACTORY_SESSION", "session-fa38")]);
    let env = FactoryTestEnv::new();
    let missing = env.cas_root.join("worktrees/knowledge-worker");
    let store = env.agent_store();

    let mut parent = Agent::new("parent-factory-id".into(), "knowledge-worker".into());
    parent.role = AgentRole::Worker;
    parent.factory_session = Some("session-fa38".into());
    parent.registered_at = chrono::Utc::now() - chrono::Duration::minutes(1);
    parent
        .metadata
        .insert("clone_path".into(), missing.to_string_lossy().into_owned());
    store.register(&parent).unwrap();

    let mut nested = parent.clone();
    nested.id = "nested-knowledge-id".into();
    nested.cc_session_id = Some("nested-transcript-id".into());
    nested.registered_at = chrono::Utc::now();
    store.register(&nested).unwrap();

    let text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("worker_status"),
    );
    assert!(text.contains("Workers (1)"), "{text}");
    assert_eq!(text.matches("• knowledge-worker").count(), 1, "{text}");
    assert!(
        text.contains("Collapsed 1 superseded registry row(s) for: knowledge-worker"),
        "{text}"
    );
    assert!(text.contains("[missing-worktree]"), "{text}");
}

/// cas-e728 (GH #105) defect 1 — stale task attribution.
///
/// A lease is a fixed-duration row that nothing renews and that not every
/// close path releases (a direct status update, a supervisor-side close, a
/// crashed worker all leave it). While it lingered, `worker_status` counted
/// the worker as holding an in-progress task for the rest of the lease
/// (default 30 minutes) — reporting work that was already finished, and
/// rendering `⚠ STALLED ... while task in progress` at a worker with nothing
/// assigned. Every task list must be read at render time and the lease must
/// only corroborate a task that is STILL in progress.
#[tokio::test]
async fn test_worker_status_task_state_is_fresh_for_a_just_closed_task() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    env.register_supervisor("sup-1");
    let worker_id = env.register_worker("wolf");

    let task_store = env.task_store();
    let id = task_store.generate_id().expect("id");
    let mut task = Task::new(id.clone(), "Do the thing".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("wolf".to_string());
    task_store.add(&task).expect("add");
    // A live lease, exactly as `try_claim` leaves one during real work.
    env.agent_store()
        .try_claim(&id, &worker_id, 1800, Some("working"))
        .expect("claim");

    let before = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("status"),
    );
    assert!(
        before.contains(&format!("task: {id} (in progress)")),
        "while genuinely in progress the task must be named: {before}"
    );

    // The task closes. The lease is deliberately NOT released — that is the
    // reported production state.
    let mut closed = task_store.get(&id).expect("get");
    closed.status = TaskStatus::Closed;
    task_store.update(&closed).expect("close");
    assert!(
        env.agent_store()
            .get_lease(&id)
            .expect("get_lease")
            .is_some(),
        "fixture precondition: the stale lease must still exist"
    );

    let after = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("status"),
    );
    assert!(
        after.contains("task: none assigned"),
        "a closed task must not read as assigned at render time: {after}"
    );
    assert!(
        !after.contains("in progress"),
        "a closed task must never render as in progress: {after}"
    );
    assert!(
        !after.contains("STALLED"),
        "a stale lease alone must not produce a stall accusation: {after}"
    );
}

/// cas-e728 (GH #105) defect 1, the load-bearing regression barrier.
///
/// Uses a CODEX worker deliberately: on a turn-observable harness the ⚠ STALLED
/// verdict is unchanged, so this test isolates the lease cross-check itself.
/// With the cross-check reverted this renders
/// `⚠ STALLED (no activity ≥0s while task in progress)` beside
/// `task: none assigned` — the reported defect, verbatim.
#[tokio::test]
async fn test_worker_status_stale_lease_alone_does_not_assert_work_in_progress() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    std::fs::write(
        env.cas_root.join("config.toml"),
        "[factory]\nstall_threshold_secs = 0\n",
    )
    .expect("write config.toml");
    env.register_supervisor("sup-1");
    let mut codex_meta = HashMap::new();
    codex_meta.insert("worker_cli".to_string(), "codex".to_string());
    let worker_id = env.register_worker_with_metadata("badger", codex_meta);

    let task_store = env.task_store();
    let id = task_store.generate_id().expect("id");
    let mut task = Task::new(id.clone(), "Finished work".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("badger".to_string());
    task_store.add(&task).expect("add");
    env.agent_store()
        .try_claim(&id, &worker_id, 1800, Some("working"))
        .expect("claim");

    let mut closed = task_store.get(&id).expect("get");
    closed.status = TaskStatus::Closed;
    closed.assignee = None;
    task_store.update(&closed).expect("close");
    assert!(
        env.agent_store()
            .get_lease(&id)
            .expect("get_lease")
            .is_some(),
        "fixture precondition: the lease must outlive the close"
    );

    let text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("status"),
    );
    let row = text
        .split("• ")
        .find(|block| block.starts_with("badger"))
        .expect("badger row");
    assert!(
        !row.contains("STALLED"),
        "a lease outliving its closed task must not assert work in progress: {row}"
    );
    assert!(
        row.contains("task: none assigned"),
        "the closed task must not be attributed: {row}"
    );
}

/// cas-e728 (GH #105) defect 1, second half — finished-awaiting-merge was
/// invisible. It rendered as "task: none assigned", identical to a worker
/// with nothing to do, so the one state that genuinely needs supervisor
/// action looked like the one that needs none.
#[tokio::test]
async fn test_worker_status_names_finished_awaiting_merge_as_waiting_on_supervisor() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    env.register_supervisor("sup-1");
    env.register_worker("wolf");

    let task_store = env.task_store();
    for (status, expected) in [
        (TaskStatus::AwaitingMerge, "awaiting merge"),
    ] {
        let id = task_store.generate_id().expect("id");
        let mut task = Task::new(id.clone(), "Finished work".to_string());
        task.status = status;
        task.assignee = Some("wolf".to_string());
        task_store.add(&task).expect("add");

        let text = get_text(
            &env.service
                .factory(Parameters(factory_req("worker_status")))
                .await
                .expect("status"),
        );
        assert!(
            text.contains(&id) && text.contains(expected),
            "{status:?} must be named on the row: {text}"
        );
        assert!(
            text.contains("WAITING ON YOU"),
            "{status:?} must say the supervisor is the blocker: {text}"
        );
        assert!(
            !text.contains("task: none assigned"),
            "{status:?} must not read as an idle worker: {text}"
        );

        let mut done = task_store.get(&id).expect("get");
        done.status = TaskStatus::Closed;
        done.assignee = None;
        task_store.update(&done).expect("clear");
    }
}

/// cas-9fd4 (GH #341): the normal AwaitingMerge advice becomes stale after a
/// supervisor has already landed the parked factory branch. The worker still
/// has to re-close, so status must name that real blocker instead of asking
/// the supervisor to merge the same branch again.
#[tokio::test]
async fn test_worker_status_names_delivered_merge_awaiting_worker_reclose_cas_9fd4() {
    use std::process::Command;

    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    env.register_supervisor("sup-1");
    let worker = "cas-9fd4-wolf";
    let worker_path = init_sync_repo(&env, worker);
    let mut metadata = HashMap::new();
    metadata.insert("clone_path".to_string(), worker_path.display().to_string());
    env.register_worker_with_metadata(worker, metadata);

    let git = |dir: &std::path::Path, args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("run git fixture command");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    std::fs::write(worker_path.join("delivered.txt"), "delivered\n").expect("write delivery");
    git(&worker_path, &["add", "delivered.txt"]);
    git(&worker_path, &["commit", "-m", "deliver cas-9fd4"]);

    let project = env.cas_root.parent().expect("project root");
    git(project, &["checkout", "epic/requested"]);
    git(
        project,
        &[
            "merge",
            "--no-ff",
            &format!("factory/{worker}"),
            "-m",
            "merge delivered worker branch",
        ],
    );
    git(project, &["checkout", "main"]);

    let task_store = env.task_store();
    let id = task_store.generate_id().expect("id");
    let mut task = Task::new(id.clone(), "Delivered task awaiting re-close".to_string());
    task.status = TaskStatus::AwaitingMerge;
    task.assignee = Some(worker.to_string());
    task.deliverables.parked_branch = Some(format!("factory/{worker}"));
    task.deliverables.work_target = Some(WorkTarget {
        repo_selector: "project:active-project".to_string(),
        target_branch: "epic/requested".to_string(),
    });
    task_store.add(&task).expect("add task");

    let text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("status"),
    );
    assert!(
        text.contains("delivered-and-merged, awaiting worker re-close"),
        "status must report the delivered state, not stale merge work: {text}"
    );
    assert!(
        text.contains("WAITING ON WORKER") && text.contains("retry task close"),
        "status must name the worker re-close as the blocker: {text}"
    );
    assert!(
        !text.contains("merge its branch, then it can close"),
        "already-integrated branch must not receive stale merge advice: {text}"
    );
}

/// cas-e728 review follow-up: `all_workers` broadcasts are real inbox items
/// with per-recipient read state. Counting only name-targeted rows made a
/// broadcast that nobody acted on render as "inbox empty" on every row — the
/// status line affirming that nothing was waiting on workers that had all been
/// asked to report.
#[tokio::test]
async fn test_worker_status_counts_broadcast_messages_in_worker_inboxes() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    env.register_supervisor("sup-1");
    env.register_worker("wolf");

    env.prompt_queue()
        .enqueue("sup-1", "all_workers", "everyone report status")
        .expect("broadcast");

    let text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("status"),
    );
    assert!(
        text.contains("inbox: 1 unread message"),
        "a pending broadcast must count toward every worker's inbox: {text}"
    );
}

/// cas-e728 review follow-up: the inbox line must render for workers that are
/// not "stalled" at all. The commonest handed-work-and-asleep shape is a worker
/// with a parked or freshly assigned task, which never trips the stall path —
/// gating the count on the alert hid it exactly where it mattered.
#[tokio::test]
async fn test_worker_status_shows_inbox_depth_even_when_not_stalled() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    env.register_supervisor("sup-1");
    env.register_worker("wolf");
    env.register_worker("fox");

    let task_store = env.task_store();
    let id = task_store.generate_id().expect("id");
    let mut task = Task::new(id.clone(), "Finished work".to_string());
    task.status = TaskStatus::AwaitingMerge;
    task.assignee = Some("wolf".to_string());
    task_store.add(&task).expect("add");

    env.prompt_queue()
        .enqueue("sup-1", "wolf", "next task for you")
        .expect("enqueue");

    let text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("status"),
    );
    let wolf = text
        .split("• ")
        .find(|b| b.starts_with("wolf"))
        .expect("wolf row");
    let fox = text
        .split("• ")
        .find(|b| b.starts_with("fox"))
        .expect("fox row");
    assert!(
        wolf.contains("inbox: 1 unread message"),
        "a parked worker with mail must still show its inbox: {wolf}"
    );
    assert!(
        !fox.contains("inbox:"),
        "a worker with no mail must not get an inbox line: {fox}"
    );
}

/// cas-e728 review follow-up: a task assigned by agent ID (not name) must be
/// named too — narrowing that lookup would silently restore the original
/// "none assigned" defect for uuid-assigned work.
#[tokio::test]
async fn test_worker_status_names_parked_task_assigned_by_agent_id() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    env.register_supervisor("sup-1");
    let worker_id = env.register_worker("wolf");

    let task_store = env.task_store();
    let id = task_store.generate_id().expect("id");
    let mut task = Task::new(id.clone(), "Finished work".to_string());
    task.status = TaskStatus::AwaitingMerge;
    task.assignee = Some(worker_id);
    task_store.add(&task).expect("add");

    let text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("status"),
    );
    assert!(
        text.contains(&id) && text.contains("awaiting merge"),
        "an id-assigned parked task must be named: {text}"
    );
}

/// cas-e728 review follow-up: a Blocked task is not in progress and not parked
/// for the supervisor, but it must still be named — it is the one status that
/// literally means "waiting on something".
#[tokio::test]
async fn test_worker_status_names_a_blocked_task() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    env.register_supervisor("sup-1");
    env.register_worker("wolf");

    let task_store = env.task_store();
    let id = task_store.generate_id().expect("id");
    let mut task = Task::new(id.clone(), "Stuck work".to_string());
    task.status = TaskStatus::Blocked;
    task.assignee = Some("wolf".to_string());
    task_store.add(&task).expect("add");

    let text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("status"),
    );
    assert!(text.contains(&id), "blocked task must be named: {text}");
    assert!(text.contains("blocked"), "{text}");
    assert!(
        !text.contains("task: none assigned"),
        "a blocked worker must not read as idle: {text}"
    );
}

/// cas-e728 review follow-up: a live in-progress task outranks an older parked
/// one on the same worker — the normal end-of-task shape (previous task
/// awaiting merge, new task started).
#[tokio::test]
async fn test_worker_status_prefers_live_task_over_parked_one() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    env.register_supervisor("sup-1");
    env.register_worker("wolf");

    let task_store = env.task_store();
    let parked_id = task_store.generate_id().expect("id");
    let mut parked = Task::new(parked_id.clone(), "Old work".to_string());
    parked.status = TaskStatus::AwaitingMerge;
    parked.assignee = Some("wolf".to_string());
    task_store.add(&parked).expect("add parked");

    let live_id = task_store.generate_id().expect("id");
    let mut live = Task::new(live_id.clone(), "Current work".to_string());
    live.status = TaskStatus::InProgress;
    live.assignee = Some("wolf".to_string());
    task_store.add(&live).expect("add live");

    let text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("status"),
    );
    assert!(
        text.contains(&format!("task: {live_id} (in progress)")),
        "the live task must win: {text}"
    );
    assert!(
        !text.contains(&format!("task: {parked_id}")),
        "the parked task must not also claim the row: {text}"
    );
}

/// cas-e728 (GH #105) defect 2 — the stall heuristic assumed continuous
/// execution. A Claude worker only runs when a message grants it a turn; a
/// healthy turn ends with commit/push/note and then legitimate silence. The
/// old flag fired dozens of times in one session with zero true positives.
/// A heartbeating worker on a harness with no turn-start artifact must get
/// the honest between-turns line plus the one actionable fact — unread mail.
#[tokio::test]
async fn test_worker_status_reports_between_turns_not_stalled_for_claude_worker() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    // Threshold 0: any quiet moment counts as "past the stall threshold", so
    // the old code would unconditionally render ⚠ STALLED here.
    std::fs::write(
        env.cas_root.join("config.toml"),
        "[factory]\nstall_threshold_secs = 0\n",
    )
    .expect("write config.toml");
    env.register_supervisor("sup-1");
    env.register_worker("wolf");

    let task_store = env.task_store();
    let id = task_store.generate_id().expect("id");
    let mut task = Task::new(id.clone(), "Long running work".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("wolf".to_string());
    task_store.add(&task).expect("add");

    let text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("status"),
    );
    assert!(
        !text.contains("STALLED"),
        "a heartbeating turn-based worker must not be accused of stalling: {text}"
    );
    assert!(
        text.contains("between turns"),
        "the row must state the between-turns reality: {text}"
    );
    assert!(
        text.contains("inbox empty"),
        "with no queued work the row must say so: {text}"
    );
}

/// cas-e728: the actionable half — quiet WITH undelivered mail means the
/// worker was handed work and has not woken. The count must be surfaced, and
/// reading status must never consume the worker's inbox.
#[tokio::test]
async fn test_worker_status_surfaces_unread_inbox_without_consuming_it() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    std::fs::write(
        env.cas_root.join("config.toml"),
        "[factory]\nstall_threshold_secs = 0\n",
    )
    .expect("write config.toml");
    env.register_supervisor("sup-1");
    env.register_worker("wolf");

    let task_store = env.task_store();
    let id = task_store.generate_id().expect("id");
    let mut task = Task::new(id.clone(), "Queued work".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("wolf".to_string());
    task_store.add(&task).expect("add");

    let queue = env.prompt_queue();
    queue.enqueue("sup-1", "wolf", "please start").expect("q1");
    queue.enqueue("sup-1", "wolf", "and this too").expect("q2");

    let text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("status"),
    );
    assert!(
        text.contains("inbox: 2 unread messages"),
        "the unread count must be on the row: {text}"
    );

    // Reading status must not mark the worker's mail as seen.
    let still_unread = queue
        .poll_unseen_for_recipient("wolf", None, 10)
        .expect("poll");
    assert_eq!(
        still_unread.len(),
        2,
        "worker_status must PEEK the inbox, never consume it"
    );
}

#[tokio::test]
async fn test_worker_status_scopes_agents_to_factory_session() {
    let _guard = EnvGuard::set_optional(&[("CAS_FACTORY_SESSION", None)]);
    let env = FactoryTestEnv::new();

    env.register_supervisor_in_session("sup-a", "session-a");
    env.register_worker_in_session("worker-a", "session-a");
    env.register_worker_in_session("worker-b", "session-b");

    let mut plain = Agent::new("plain-agent".to_string(), "plain-worker".to_string());
    plain.role = AgentRole::Worker;
    plain.factory_session = None;
    let agent_store = env.agent_store();
    agent_store.register(&plain).expect("register plain worker");

    unsafe { std::env::set_var("CAS_FACTORY_SESSION", "session-a") };
    let req = factory_req("worker_status");
    let result = env
        .service
        .factory(Parameters(req))
        .await
        .expect("worker_status should succeed");

    let text = get_text(&result);
    assert!(
        text.contains("worker-a"),
        "same-session worker visible: {text}"
    );
    assert!(
        !text.contains("worker-b"),
        "other-session worker must be hidden: {text}"
    );
    assert!(
        !text.contains("plain-worker"),
        "NULL-session plain CC worker must be hidden from factory director: {text}"
    );
}

/// cas-e98e AC2: for the same registry state, the set of live factory worker
/// identities from `worker_status` must agree with `agent_list` effective
/// liveness labels (active / active,alive-heartbeat-stale).
#[tokio::test]
async fn test_worker_status_and_agent_list_agree_on_live_workers() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();

    env.register_worker("live-fresh");
    let store = env.agent_store();

    // Heartbeat-stale + process-alive (this test process).
    let id_alive = Agent::generate_fallback_id();
    let mut alive = Agent::new(id_alive.clone(), "live-stale-hb".to_string());
    alive.role = AgentRole::Worker;
    let staleness = chrono::Duration::seconds(40);
    alive.last_heartbeat = chrono::Utc::now() - staleness;
    alive.registered_at = chrono::Utc::now() - staleness;
    alive.pid = Some(std::process::id());
    store
        .register(&alive)
        .expect("register process-alive stale-hb worker");

    // Truly dead stale (no pid) — must not appear as live.
    env.register_stale_worker_with_clone_path("dead-stale", "/tmp/cas-wt-dead", 40);

    let status_text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("worker_status"),
    );
    let list_req: CoordinationRequest = serde_json::from_value(serde_json::json!({
        "action": "agent_list"
    }))
    .expect("CoordinationRequest");
    let list_text = get_text(
        &env.service
            .coordination(Parameters(list_req))
            .await
            .expect("agent_list"),
    );

    // Live identities from worker_status (named bullets) — AC2 count + identity.
    assert!(
        status_text.contains("Workers (2)"),
        "worker_status operational count must be 2. Got:\n{status_text}"
    );
    for name in ["live-fresh", "live-stale-hb"] {
        assert!(
            status_text.contains(name),
            "worker_status must list {name}. Got:\n{status_text}"
        );
        assert!(
            list_text.contains(name),
            "agent_list must list {name}. Got:\n{list_text}"
        );
    }
    assert!(
        list_text.contains("Live factory workers (authoritative): 2"),
        "agent_list live footer must agree on count 2. Got:\n{list_text}"
    );
    assert!(
        list_text.contains("active,alive-heartbeat-stale")
            || (status_text.contains("alive") && status_text.contains("heartbeat stale")),
        "dual-signal must appear for process-alive stale-hb worker.\nstatus:\n{status_text}\nlist:\n{list_text}"
    );
    assert!(
        !status_text.contains("dead-stale"),
        "dead worker must not be in worker_status Active roster. Got:\n{status_text}"
    );
}

/// cas-3e56: heartbeat past WORKER_STALE_SECS but registered harness PID still
/// alive → worker_status must keep the worker listed as active with the
/// "[alive — heartbeat stale]" dual-signal, never omit as "None active".
///
/// This is the supervision-truth residual after Grok liveness work: false
/// "None active" while a Grok worker is mid-turn nearly caused a re-spawn.
#[tokio::test]
async fn test_worker_status_keeps_heartbeat_stale_process_alive_worker() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();

    let store = env.agent_store();
    let id = Agent::generate_fallback_id();
    let mut agent = Agent::new(id.clone(), "mid-turn-grok".to_string());
    agent.role = AgentRole::Worker;
    agent
        .metadata
        .insert("worker_cli".to_string(), "grok".to_string());
    // Heartbeat is "stale" by the 30s prune threshold.
    let staleness = chrono::Duration::seconds(40);
    agent.last_heartbeat = chrono::Utc::now() - staleness;
    agent.registered_at = chrono::Utc::now() - staleness;
    // Registered PID = this test process (alive). No fingerprint → pid-only
    // liveness (kill 0) still proves the process is up.
    agent.pid = Some(std::process::id());
    store
        .register(&agent)
        .expect("register heartbeat-stale process-alive worker");

    let req = factory_req("worker_status");
    let result = env
        .service
        .factory(Parameters(req))
        .await
        .expect("worker_status call should succeed");
    let text = get_text(&result);

    assert!(
        text.contains("mid-turn-grok"),
        "process-alive worker must stay in Active listing despite stale heartbeat. Got:\n{text}"
    );
    assert!(
        !text.contains("Workers: None active"),
        "must not report empty roster when a process-alive worker exists. Got:\n{text}"
    );
    assert!(
        text.contains("alive") && text.contains("heartbeat stale"),
        "must surface dual-signal '[alive — heartbeat stale]'. Got:\n{text}"
    );
    assert!(
        !text.contains("Filtered stale agent record(s)"),
        "process-alive worker must not be pruned. Got:\n{text}"
    );
}

/// cas-5b1c integration coverage: a worker whose heartbeat is older than
/// `WORKER_STALE_SECS` (30s) is pruned out of the Active listing on the
/// next `factory_worker_status` call and reported in the "Filtered stale
/// agent record(s)" footer, while a live worker from the same call stays
/// visible. This pins the supervisor-facing UX contract that stale
/// workers disappear promptly once past the threshold.
///
/// Implementation note: `factory_worker_status` does its opportunistic
/// prune BEFORE rendering the Active list, so in the common path a
/// stale Worker transitions out of Active and never hits the `[DEAD]`
/// label / transcript-path render branch. The render-time DEAD branch
/// only fires when `mark_stale` fails (DB lock, etc.) — that code path
/// is cheap unit coverage at the `resolve_transcript` / `render_transcript_block`
/// level (see the `mcp::tools::service::factory_ops::tests` module),
/// now with glob-based resolution landed via cas-900b. Here we test the
/// prune-success integration.
#[tokio::test]
async fn test_worker_status_prunes_stale_worker_and_keeps_live_one() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();

    // Live worker: default heartbeat = now, stays in Active.
    env.register_worker("live-fox");
    // Stale worker: heartbeat backdated 40s so list_stale(30) catches it.
    let stale_id =
        env.register_stale_worker_with_clone_path("dead-wolf", "/tmp/cas-worktrees/dead-wolf", 40);

    let req = factory_req("worker_status");
    let result = env
        .service
        .factory(Parameters(req))
        .await
        .expect("worker_status call should succeed");
    let text = get_text(&result);

    // Live worker must appear; stale must not appear as an Active bullet.
    // cas-2e81: stale workers that held no lease may be absent entirely;
    // those that held a lease can appear under "Recently died while leased".
    assert!(
        text.contains("live-fox"),
        "live worker must appear in Active listing. Got:\n{text}"
    );
    let active_section = text
        .split("Recently died while leased")
        .next()
        .unwrap_or(&text);
    assert!(
        !active_section
            .lines()
            .any(|l| l.contains("• dead-wolf") || l.contains(&format!("• {stale_id}"))),
        "stale worker must be pruned out of the Active listing. Got:\n{text}"
    );

    // The footer must account for the prune so operators can see the
    // pruned count at a glance.
    assert!(
        text.contains("Filtered stale agent record(s): 1"),
        "prune summary must report exactly 1 stale record filtered. Got:\n{text}"
    );
    assert!(
        text.contains("30s heartbeat age"),
        "footer must reference the 30s worker threshold. Got:\n{text}"
    );
}

#[tokio::test]
async fn test_worker_status_prune_skips_stale_workers_in_other_factory_sessions() {
    let _guard = EnvGuard::set(&[("CAS_FACTORY_SESSION", "session-a")]);
    let env = FactoryTestEnv::new();

    env.register_worker_in_session("live-a", "session-a");

    let store = env.agent_store();
    let stale_b_id = Agent::generate_fallback_id();
    let mut stale_b = Agent::new(stale_b_id.clone(), "stale-b".to_string());
    stale_b.role = AgentRole::Worker;
    stale_b.status = AgentStatus::Active;
    stale_b.factory_session = Some("session-b".to_string());
    let staleness = chrono::Duration::seconds(40);
    stale_b.last_heartbeat = chrono::Utc::now() - staleness;
    stale_b.registered_at = chrono::Utc::now() - staleness;
    store
        .register(&stale_b)
        .expect("register stale session-b worker");

    let req = factory_req("worker_status");
    let result = env
        .service
        .factory(Parameters(req))
        .await
        .expect("worker_status call should succeed");
    let text = get_text(&result);

    assert!(
        text.contains("live-a"),
        "same-session live worker should appear: {text}"
    );
    assert!(
        !text.contains("stale-b"),
        "other-session stale worker should remain hidden: {text}"
    );
    let stale_after = store
        .get(&stale_b_id)
        .expect("session-b stale worker should still exist");
    assert_eq!(
        stale_after.status,
        AgentStatus::Active,
        "session-a worker_status prune must not mark stale workers in session-b"
    );
}

/// cas-9829: a worker holding an in-progress task lease whose last observed
/// activity is at/past the configured `stall_threshold_secs` must render
/// `⚠ STALLED`, not the soft "may be investigating or idle" hedge — that
/// hedge is exactly what let a genuinely stalled worker go unnoticed in the
/// reported bug (worker printed a plan, then produced nothing for 10+
/// minutes while heartbeating fine). A worker with NO claimed task must
/// never be marked STALLED — that's the pre-existing WorkerIdle state, a
/// different signal entirely.
///
/// `stall_threshold_secs` is set to `0` via `.cas/config.toml` so the
/// claim's own registration-time activity event (which is necessarily
/// "0s ago" in a synchronous test) already counts as past-threshold —
/// this deterministically exercises the render wiring without needing to
/// fabricate a real time gap.
#[tokio::test]
async fn test_9829_worker_status_marks_stalled_worker_with_in_progress_task() {
    let codex_home = tempfile::tempdir().expect("codex home");
    let _guard = EnvGuard::set_optional(&[
        ("CAS_FACTORY_SESSION", None),
        (
            "CODEX_HOME",
            Some(codex_home.path().to_str().expect("utf-8 codex home")),
        ),
    ]);
    let env = FactoryTestEnv::new();
    std::fs::write(
        env.cas_root.join("config.toml"),
        "[factory]\nstall_threshold_secs = 0\n",
    )
    .expect("write config.toml");

    // cas-e728 (GH #105): the ⚠ STALLED verdict now belongs to harnesses that
    // publish an authoritative turn-start artifact. Codex does, so this row
    // keeps the original contract verbatim; the Claude case is covered by
    // test_worker_status_reports_between_turns_not_stalled_for_claude_worker.
    let clone_path = "/tmp/cas-9829-busy-badger";
    let rollout = codex_home
        .path()
        .join("sessions/2026/08/13/rollout-2026-08-13T01-00-00-stalled.jsonl");
    std::fs::create_dir_all(rollout.parent().expect("rollout parent"))
        .expect("create rollout parent");
    std::fs::write(
        &rollout,
        format!(
            "{}\n{}\n",
            serde_json::json!({
                "type": "session_meta",
                "payload": {
                    "cwd": clone_path,
                    "originator": "codex-tui",
                    "source": "cli"
                }
            }),
            serde_json::json!({
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "type": "event_msg",
                "payload": {"type": "task_started", "turn_id": "stalled-turn"}
            })
        ),
    )
    .expect("write authoritative Codex turn artifact");

    let mut codex_meta = HashMap::new();
    codex_meta.insert("worker_cli".to_string(), "codex".to_string());
    codex_meta.insert("clone_path".to_string(), clone_path.to_string());
    let busy_id = env.register_worker_with_metadata("busy-badger", codex_meta);
    let task_store = env.task_store();
    let task = Task::new("cas-0b7d".to_string(), "Stalled task".to_string());
    task_store.add(&task).expect("add task");
    env.agent_store()
        .try_claim("cas-0b7d", &busy_id, 600, None)
        .expect("claim task")
        .is_success();

    env.register_worker("idle-ibis"); // no claimed task — must stay soft-worded

    let req = factory_req("worker_status");
    let result = env
        .service
        .factory(Parameters(req))
        .await
        .expect("worker_status call should succeed");
    let text = get_text(&result);

    assert!(
        text.contains("busy-badger"),
        "busy worker must appear in the listing. Got:\n{text}"
    );
    // Find the busy-badger's own row/block for a precise assertion (avoid a
    // STALLED marker from the wrong worker satisfying the check).
    let badger_block = text
        .split("• ")
        .find(|block| block.starts_with("busy-badger"))
        .expect("busy-badger row must be present");
    assert!(
        badger_block.contains("⚠ STALLED"),
        "worker with an in-progress task past the stall threshold must be marked STALLED. Got:\n{badger_block}"
    );

    // A worker with no claimed task is never "stalled" in this sense — an
    // idle worker with no task is the pre-existing WorkerIdle state, not a
    // stall, regardless of how fresh/stale its activity looks.
    let ibis_block = text
        .split("• ")
        .find(|block| block.starts_with("idle-ibis"))
        .expect("idle-ibis row must be present");
    assert!(
        !ibis_block.contains("⚠ STALLED"),
        "a worker with no claimed task must never be marked STALLED. Got:\n{ibis_block}"
    );
}

// =============================================================================
// cas-2e81: orphan InProgress + death/lease-expiry signal
// =============================================================================

/// Simulated kill of a lease holder after start: worker_status prune must
/// park the InProgress task Open with an audit note (non-silent recovery).
#[tokio::test]
async fn test_2e81_worker_status_parks_orphaned_inprogress_on_stale_prune() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    let sup_id = env.register_supervisor("sup-orphan");

    let worker_id =
        env.register_stale_worker_with_clone_path("gone-worker", "/tmp/cas-worktrees/gone", 40);
    let task_store = env.task_store();
    let mut task = Task::new("cas-orphan1".to_string(), "Orphan mid-task".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("gone-worker".to_string());
    task_store.add(&task).expect("add task");
    env.agent_store()
        .try_claim("cas-orphan1", &worker_id, 600, Some("started"))
        .expect("claim")
        .is_success();

    let req = factory_req("worker_status");
    let result = env
        .service
        .factory(Parameters(req))
        .await
        .expect("worker_status");
    let text = get_text(&result);

    let recovered = task_store.get("cas-orphan1").expect("task still exists");
    assert_eq!(
        recovered.status,
        TaskStatus::Open,
        "orphaned InProgress must park to Open. Got notes: {}",
        recovered.notes
    );
    assert!(
        recovered.assignee.is_none(),
        "assignee must clear on orphan recovery"
    );
    assert!(
        recovered.notes.contains("orphaned")
            || recovered.notes.contains("worker vanished")
            || recovered.notes.contains("lease"),
        "audit note required. notes={}",
        recovered.notes
    );

    // Recovery signal also visible on worker_status (died-while-leased section).
    assert!(
        text.contains("Recently died while leased") || text.contains("gone-worker"),
        "worker_status must surface death/orphan signal. Got:\n{text}"
    );
    assert!(
        text.contains("cas-orphan1"),
        "held task id must appear in death signal. Got:\n{text}"
    );

    // worker_died queue notification for the supervising session.
    let queue = cas::store::open_supervisor_queue_store(&env.cas_root).expect("queue");
    let pending = queue.peek(&sup_id, 20).expect("peek");
    assert!(
        pending.iter().any(|n| n.event_type == "worker_died"),
        "worker_died must be queued for supervisor. pending={pending:?}"
    );
    let _ = sup_id;
}

/// Empty fleet with no deaths vs died-while-leased must read differently.
#[tokio::test]
async fn test_2e81_worker_status_distinguishes_empty_fleet_vs_died_while_leased() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    env.register_supervisor("sup-empty");

    // Empty fleet (only supervisor) — no died-while-leased section.
    let empty_text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("worker_status empty"),
    );
    assert!(
        empty_text.contains("None active") || empty_text.contains("No active"),
        "empty fleet must say no active workers. Got:\n{empty_text}"
    );
    assert!(
        !empty_text.contains("Recently died while leased"),
        "empty fleet must NOT claim died-while-leased. Got:\n{empty_text}"
    );

    // Kill a lease holder mid-task.
    let worker_id =
        env.register_stale_worker_with_clone_path("crash-worker", "/tmp/cas-worktrees/crash", 45);
    let task_store = env.task_store();
    let mut task = Task::new("cas-crash1".to_string(), "Crash mid-task".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("crash-worker".to_string());
    task_store.add(&task).expect("add");
    env.agent_store()
        .try_claim("cas-crash1", &worker_id, 600, None)
        .expect("claim")
        .is_success();

    let died_text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("worker_status after death"),
    );
    assert!(
        died_text.contains("Recently died while leased"),
        "died-while-leased must be explicit. Got:\n{died_text}"
    );
    assert!(
        died_text.contains("crash-worker") && died_text.contains("cas-crash1"),
        "must name dead worker + held task. Got:\n{died_text}"
    );
}

/// agent_cleanup path: lease reclaim + stale mark also parks orphaned tasks
/// and emits worker_died (not only worker_status).
#[tokio::test]
async fn test_2e81_agent_cleanup_parks_orphan_and_emits_worker_died() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    let sup_id = env.register_supervisor("sup-cleanup");

    let worker_id =
        env.register_stale_worker_with_clone_path("cleanup-dead", "/tmp/cas-worktrees/cd", 200);
    let task_store = env.task_store();
    let mut task = Task::new("cas-clean1".to_string(), "Cleanup orphan".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("cleanup-dead".to_string());
    task_store.add(&task).expect("add");
    env.agent_store()
        .try_claim("cas-clean1", &worker_id, 600, None)
        .expect("claim")
        .is_success();

    let mut req = coord_req("agent_cleanup");
    req.stale_threshold_secs = Some(30);
    let result = env
        .service
        .coordination(Parameters(req))
        .await
        .expect("agent_cleanup");
    let text = get_text(&result);
    assert!(
        text.contains("Cleanup complete") || text.contains("Stale"),
        "cleanup should succeed: {text}"
    );

    let recovered = task_store.get("cas-clean1").expect("task");
    assert_eq!(
        recovered.status,
        TaskStatus::Open,
        "agent_cleanup must park orphaned InProgress. notes={}",
        recovered.notes
    );

    let queue = cas::store::open_supervisor_queue_store(&env.cas_root).expect("queue");
    let pending = queue.peek(&sup_id, 20).expect("peek");
    assert!(
        pending.iter().any(|n| n.event_type == "worker_died"),
        "agent_cleanup must queue worker_died. pending={pending:?}"
    );
}

/// AwaitingMerge tasks must not be reset to Open by orphan recovery.
#[tokio::test]
async fn test_2e81_orphan_recovery_skips_awaiting_merge_tasks() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    env.register_supervisor("sup-psr");

    let worker_id =
        env.register_stale_worker_with_clone_path("psr-worker", "/tmp/cas-worktrees/psr", 40);
    let task_store = env.task_store();
    let mut task = Task::new("cas-awaiting-merge1".to_string(), "Awaiting merge task".to_string());
    task.status = TaskStatus::AwaitingMerge;
    task.assignee = Some("awaiting-merge-worker".to_string());
    task_store.add(&task).expect("add");
    // Still hold a lease so mark_stale revokes something — recovery must skip status flip.
    env.agent_store()
        .try_claim("cas-awaiting-merge1", &worker_id, 600, None)
        .expect("claim")
        .is_success();
    // Force AwaitingMerge again in case claim path rewrote status.
    let mut t = task_store.get("cas-awaiting-merge1").unwrap();
    t.status = TaskStatus::AwaitingMerge;
    task_store.update(&t).unwrap();

    let _ = env
        .service
        .factory(Parameters(factory_req("worker_status")))
        .await
        .expect("worker_status");

    let after = task_store.get("cas-awaiting-merge1").unwrap();
    assert_eq!(
        after.status,
        TaskStatus::AwaitingMerge,
        "AwaitingMerge must not be auto-reset to Open by orphan recovery"
    );
}

// cas-3dcb (GH #168): worker death reaches the supervisor's PROMPT path,
// exactly once per death incident
// =============================================================================

/// All prompt-queue rows that are worker-death relays.
fn worker_died_prompt_rows(cas_root: &std::path::Path) -> Vec<cas_store::QueuedPrompt> {
    cas::store::open_prompt_queue_store(cas_root)
        .expect("prompt queue")
        .peek_all(200)
        .expect("peek prompt queue")
        .into_iter()
        .filter(|row| row.prompt.starts_with("<worker-died "))
        .collect()
}

/// cas-20ac: lifecycle relays print the durable `supervisor_queue` ID, while
/// replay is driven by a separately numbered `prompt_queue` row. Both public
/// acknowledgement actions must resolve that link instead of blindly applying
/// the visible integer to an unrelated prompt row with the same ID.
#[tokio::test]
async fn test_20ac_lifecycle_ack_actions_terminate_both_queue_lanes_without_replay() {
    let _guard = EnvGuard::set(&[]);

    for action in ["message_ack", "queue_ack"] {
        let env = FactoryTestEnv::new();
        let prompt_queue = env.prompt_queue();
        let durable_queue =
            cas::store::open_supervisor_queue_store(&env.cas_root).expect("durable queue");

        // Reproduce the live ambiguity: visible durable notification 1 exists
        // alongside unrelated prompt 1; its actual relay is prompt 2.
        let unrelated = prompt_queue
            .enqueue("worker", "supervisor", "unrelated ordinary message")
            .expect("unrelated prompt");
        assert_eq!(unrelated, 1);
        let durable_id = durable_queue
            .notify(
                "supervisor-id",
                "worker_died",
                r#"{"worker_name":"duplicate-worker"}"#,
                cas_store::NotificationPriority::Critical,
            )
            .expect("durable worker death");
        assert_eq!(durable_id, 1);
        let linked_prompt = match prompt_queue
            .enqueue_idempotent(
                "lifecycle-wake:worker-died:1",
                "supervisor",
                "<worker-died worker_id=\"registration-1\" worker_name=\"duplicate-worker\" incident=\"incident-1\" notification_id=\"1\">\nHeld at death: none\nParked back to Open: none\n</worker-died>",
                None,
                Some("worker died: duplicate-worker"),
                Some(cas_store::NotificationPriority::Critical),
                "worker-died-outbox:1",
            )
            .expect("linked relay")
        {
            cas_store::EnqueueIdempotentResult::Created(id)
            | cas_store::EnqueueIdempotentResult::AlreadyExists(id) => id,
        };
        assert_eq!(linked_prompt, 2);
        prompt_queue
            .mark_transport_delivered(linked_prompt)
            .expect("simulate first injection");

        let mut request = coord_req(action);
        request.notification_id = Some(durable_id);
        let result = env
            .service
            .coordination(Parameters(request))
            .await
            .expect("public lifecycle acknowledgement");
        let text = get_text(&result);
        assert!(
            text.contains("across durable and prompt queues"),
            "{action} must describe the unified lifecycle acknowledgement: {text}"
        );

        assert!(
            durable_queue
                .get(durable_id)
                .expect("durable lookup")
                .expect("durable row")
                .processed_at
                .is_some(),
            "{action} must terminalize the durable row"
        );
        let linked_report = prompt_queue
            .message_delivery_report(linked_prompt)
            .expect("linked report")
            .expect("linked row");
        assert_eq!(
            linked_report.stage,
            cas_store::DeliveryStage::Confirmed,
            "{action} must terminalize the exact replay-driving prompt"
        );
        let unrelated_report = prompt_queue
            .message_delivery_report(unrelated)
            .expect("unrelated report")
            .expect("unrelated row");
        assert!(
            unrelated_report.confirmed_at.is_none(),
            "{action} must not acknowledge a numerically colliding ordinary prompt"
        );

        let replay = prompt_queue
            .poll_unseen_for_recipient("supervisor", None, 10)
            .expect("replay poll");
        assert!(
            replay.iter().all(|row| row.id != linked_prompt),
            "{action} allowed exact notification {durable_id} to re-inject: {replay:?}"
        );
        assert!(
            durable_queue
                .peek("supervisor-id", 10)
                .expect("durable replay poll")
                .is_empty(),
            "{action} left the durable notification eligible for replay"
        );
    }
}

/// Exact live 3764/3765 shape: the old queue_ack processed the durable row,
/// but the already-delivered prompt row survived and injected the same turn
/// again. A repeated queue_ack must now resolve the processed durable row and
/// terminalize its still-pending linked prompt instead of returning "not found
/// or already processed" while replay remains armed.
#[tokio::test]
async fn test_20ac_queue_ack_repairs_processed_durable_row_with_replay_pending() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    let prompt_queue = env.prompt_queue();
    let durable_queue =
        cas::store::open_supervisor_queue_store(&env.cas_root).expect("durable queue");

    let durable_id = durable_queue
        .notify(
            "supervisor-id",
            "worker_died",
            r#"{"worker_name":"duplicate-worker"}"#,
            cas_store::NotificationPriority::Critical,
        )
        .expect("durable worker death");
    let linked_prompt = match prompt_queue
        .enqueue_idempotent(
            &format!("lifecycle-wake:worker-died:{durable_id}"),
            "supervisor",
            &format!(
                "<worker-died worker_id=\"registration-3765\" worker_name=\"duplicate-worker\" incident=\"incident-3765\" notification_id=\"{durable_id}\">\nHeld at death: none\nParked back to Open: none\n</worker-died>"
            ),
            None,
            Some("worker died: duplicate-worker"),
            Some(cas_store::NotificationPriority::Critical),
            &format!("worker-died-outbox:{durable_id}"),
        )
        .expect("linked relay")
    {
        cas_store::EnqueueIdempotentResult::Created(id)
        | cas_store::EnqueueIdempotentResult::AlreadyExists(id) => id,
    };
    prompt_queue
        .mark_transport_delivered(linked_prompt)
        .expect("first turn injection");

    // Reproduce the old successful queue_ack: only the durable lane became
    // terminal, so the exact prompt remained eligible for another user turn.
    durable_queue
        .ack(durable_id)
        .expect("legacy durable-only queue_ack");
    let reinjected = prompt_queue
        .poll_unseen_for_recipient("supervisor", None, 10)
        .expect("historical reinjection");
    assert!(
        reinjected.iter().any(|row| row.id == linked_prompt),
        "fixture must reproduce processed durable row + exact prompt reinjection"
    );

    let mut request = coord_req("queue_ack");
    request.notification_id = Some(durable_id);
    let result = env
        .service
        .coordination(Parameters(request))
        .await
        .expect("repeated queue_ack bridges processed durable row");
    assert!(
        get_text(&result).contains("across durable and prompt queues"),
        "repeated queue_ack must report the repaired cross-queue acknowledgement"
    );
    assert_eq!(
        prompt_queue
            .message_delivery_report(linked_prompt)
            .expect("linked report")
            .expect("linked row")
            .stage,
        cas_store::DeliveryStage::Confirmed
    );
    let after = prompt_queue
        .poll_unseen_for_recipient("supervisor", None, 10)
        .expect("post-ack replay poll");
    assert!(
        after.iter().all(|row| row.id != linked_prompt),
        "processed-row repair still allowed notification {durable_id} to re-inject: {after:?}"
    );
}

/// The reported defect: 2,044 death notices, 100% never injected into any
/// supervisor turn, because the emitter wrote to `supervisor_queue` only.
/// A death must now land on the prompt path — the one the supervisor reads.
#[tokio::test]
async fn test_3dcb_worker_death_reaches_the_prompt_path() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    let sup_id = env.register_supervisor("sup-prompt-path");

    let worker_id =
        env.register_stale_worker_with_clone_path("died-loud", "/tmp/cas-worktrees/loud", 90);
    let task_store = env.task_store();
    let mut task = Task::new("cas-loud1".to_string(), "Held at death".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("died-loud".to_string());
    task_store.add(&task).expect("add task");
    env.agent_store()
        .try_claim("cas-loud1", &worker_id, 600, None)
        .expect("claim")
        .is_success();

    env.service
        .factory(Parameters(factory_req("worker_status")))
        .await
        .expect("worker_status");

    let rows = worker_died_prompt_rows(&env.cas_root);
    assert_eq!(
        rows.len(),
        1,
        "a worker death must enqueue exactly one prompt-path relay. rows={rows:?}"
    );
    let row = &rows[0];
    assert_eq!(
        row.target, "supervisor",
        "the death relay must target the supervisor pane. row={row:?}"
    );
    assert!(
        row.source.starts_with("lifecycle-wake:"),
        "the relay needs a wake-eligible source or the daemon will neither wake an idle \
         supervisor nor report it as a lost relay. source={}",
        row.source
    );
    assert!(
        !row.source.contains("died-loud"),
        "the source must not be the dead worker's name — `is_dead_worker_source` drops those. \
         source={}",
        row.source
    );
    assert!(
        row.prompt.contains("died-loud") && row.prompt.contains("cas-loud1"),
        "the relay must name the dead worker and the work it held. prompt={}",
        row.prompt
    );

    // The durable row survives for existing consumers, and is now stamped as
    // handed off to the prompt path.
    let queue = cas::store::open_supervisor_queue_store(&env.cas_root).expect("queue");
    let pending = queue.peek(&sup_id, 20).expect("peek");
    let died: Vec<_> = pending
        .iter()
        .filter(|n| n.event_type == "worker_died")
        .collect();
    assert_eq!(
        died.len(),
        1,
        "supervisor_queue consumers must still see exactly one worker_died. pending={pending:?}"
    );
    assert!(
        died[0].prompt_delivered_at.is_some(),
        "the durable row must be stamped once its prompt was handed off. row={:?}",
        died[0]
    );
}

/// Dedup is keyed on the death INCIDENT, not on the agent — a worker that
/// revives, heartbeats, and dies again is a new fact the supervisor must hear.
#[tokio::test]
async fn test_3dcb_a_second_genuine_death_is_reported_again() {
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();
    env.register_supervisor("sup-second-death");

    let worker_id =
        env.register_stale_worker_with_clone_path("twice-dead", "/tmp/cas-worktrees/twice", 90);
    let task_store = env.task_store();
    let mut task = Task::new("cas-twice1".to_string(), "First life".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("twice-dead".to_string());
    task_store.add(&task).expect("add task");
    env.agent_store()
        .try_claim("cas-twice1", &worker_id, 600, None)
        .expect("claim")
        .is_success();

    env.service
        .factory(Parameters(factory_req("worker_status")))
        .await
        .expect("first death");
    assert_eq!(worker_died_prompt_rows(&env.cas_root).len(), 1);

    // Revive: a fresh heartbeat is what makes the next death a new incident.
    let agent_store = env.agent_store();
    agent_store.revive(&worker_id).expect("revive");
    agent_store.heartbeat(&worker_id).expect("heartbeat");
    let mut revived = agent_store.get(&worker_id).expect("get agent");
    revived.last_heartbeat = chrono::Utc::now() - chrono::Duration::seconds(90);
    agent_store.update(&revived).expect("backdate heartbeat");

    let mut task = task_store.get("cas-twice1").expect("task");
    task.status = TaskStatus::InProgress;
    task.assignee = Some("twice-dead".to_string());
    task_store.update(&task).expect("re-assign");
    agent_store
        .try_claim("cas-twice1", &worker_id, 600, None)
        .expect("re-claim")
        .is_success();

    env.service
        .factory(Parameters(factory_req("worker_status")))
        .await
        .expect("second death");

    assert_eq!(
        worker_died_prompt_rows(&env.cas_root).len(),
        2,
        "a genuinely separate death must be reported again — dedup keys the incident, \
         not the agent"
    );
}

// =============================================================================
// worker_activity tests
// =============================================================================

#[tokio::test]
async fn test_worker_activity_empty() {
    let env = FactoryTestEnv::new();

    let req = factory_req("worker_activity");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("No recent worker activity"),
        "Should report no activity: {text}"
    );
}

#[tokio::test]
async fn test_worker_activity_scopes_session_and_honors_target_filter() {
    let _guard = EnvGuard::set(&[("CAS_FACTORY_SESSION", "session-a")]);
    let env = FactoryTestEnv::new();

    let worker_a = env.register_worker_in_session("worker-a", "session-a");
    let worker_b = env.register_worker_in_session("worker-b", "session-b");
    env.record_worker_file_event(&worker_a, "worker-a edited src/lib.rs");
    env.record_worker_file_event(&worker_b, "worker-b edited src/lib.rs");

    let mut req = factory_req("worker_activity");
    req.target = Some("worker-a".to_string());
    let result = env
        .service
        .factory(Parameters(req))
        .await
        .expect("worker_activity should succeed");

    let text = get_text(&result);
    assert!(
        text.contains("worker-a edited"),
        "targeted same-session activity should be visible: {text}"
    );
    assert!(
        !text.contains("worker-b edited"),
        "other-session activity must be hidden: {text}"
    );

    let mut req = factory_req("worker_activity");
    req.target = Some("worker-b".to_string());
    let result = env
        .service
        .factory(Parameters(req))
        .await
        .expect("worker_activity should succeed for hidden target");
    let text = get_text(&result);
    assert!(
        text.contains("No recent worker activity"),
        "target outside caller session should produce no activity: {text}"
    );
}

#[tokio::test]
async fn test_worker_activity_includes_idle_workers() {
    let _guard = EnvGuard::set(&[("CAS_FACTORY_SESSION", "session-a")]);
    let env = FactoryTestEnv::new();

    let store = env.agent_store();
    let idle_id = Agent::generate_fallback_id();
    let mut idle_worker = Agent::new(idle_id.clone(), "idle-worker".to_string());
    idle_worker.role = AgentRole::Worker;
    idle_worker.status = AgentStatus::Idle;
    idle_worker.factory_session = Some("session-a".to_string());
    store
        .register(&idle_worker)
        .expect("register idle worker in session");
    env.record_worker_file_event(&idle_id, "idle-worker edited src/lib.rs");

    let mut req = factory_req("worker_activity");
    req.target = Some("idle-worker".to_string());
    let result = env
        .service
        .factory(Parameters(req))
        .await
        .expect("worker_activity should include idle worker");

    let text = get_text(&result);
    assert!(
        text.contains("idle-worker edited"),
        "Idle workers with recent events should still report activity: {text}"
    );
}

#[tokio::test]
async fn test_worker_activity_uses_worker_status_signal_full_names_and_suppresses_closed_tasks() {
    let env = FactoryTestEnv::new();
    let worker_name = "codex-current-worker-255";
    let worker_id = env.register_worker(worker_name);

    let mut closed_task = Task::new(
        "cas-terminal-activity".to_string(),
        "Terminal activity".to_string(),
    );
    closed_task.status = TaskStatus::Closed;
    closed_task.assignee = Some(worker_name.to_string());
    env.task_store().add(&closed_task).expect("add closed task");

    let mut terminal_event = Event::new(
        EventType::WorkerGitCommit,
        EventEntityType::Task,
        &closed_task.id,
        "stale closed-task activity",
    )
    .with_session(worker_id.clone());
    terminal_event.created_at = chrono::Utc::now() - chrono::Duration::minutes(8);
    env.event_store()
        .record(&terminal_event)
        .expect("record terminal activity");

    // TaskNoteAdded is part of worker_status's last-activity input but was
    // previously discarded from worker_activity's hook-only event subset.
    let fresh_signal = Event::new(
        EventType::TaskNoteAdded,
        EventEntityType::Task,
        "cas-live-activity",
        "fresh progress-note activity",
    )
    .with_session(worker_id);
    env.event_store()
        .record(&fresh_signal)
        .expect("record fresh worker-status signal");

    let result = env
        .service
        .factory(Parameters(factory_req("worker_activity")))
        .await
        .expect("worker_activity should succeed");
    let text = get_text(&result);

    assert!(
        text.contains("• codex-current-worker-255 - fresh progress-note activity"),
        "fresh worker_status activity must use the full registered worker name: {text}"
    );
    assert!(
        text.contains("1 terminal-task activity row suppressed"),
        "terminal task events must collapse to one operator-facing count: {text}"
    );
    assert!(
        !text.contains("stale closed-task activity"),
        "closed-task activity must not be presented as live work: {text}"
    );
}

#[tokio::test]
async fn test_worker_activity_codex_tool_call_uses_worker_status_rollout_signal() {
    use std::io::Write;

    let codex_home = tempfile::tempdir().expect("codex home");
    let clone_path = "/tmp/cas-a568-codex-worker";
    let rollout = codex_home
        .path()
        .join("sessions/2026/07/28/rollout-2026-07-28T12-00-00-live.jsonl");
    std::fs::create_dir_all(rollout.parent().unwrap()).unwrap();
    let mut file = std::fs::File::create(&rollout).unwrap();
    writeln!(
        file,
        "{}",
        serde_json::json!({
            "type": "session_meta",
            "payload": {
                "session_id": "019fa8b8-activity-feed",
                "cwd": clone_path,
                "originator": "codex-tui",
                "source": "cli"
            }
        })
    )
    .unwrap();
    writeln!(
        file,
        r#"{{"type":"response_item","payload":{{"type":"function_call","call_id":"call-live","name":"apply_patch"}}}}"#
    )
    .unwrap();
    drop(file);

    let _guard = EnvGuard::set(&[(
        "CODEX_HOME",
        codex_home.path().to_str().expect("utf-8 temp path"),
    )]);
    let env = FactoryTestEnv::new();
    let mut metadata = HashMap::new();
    metadata.insert("worker_cli".to_string(), "codex".to_string());
    metadata.insert("clone_path".to_string(), clone_path.to_string());
    env.register_worker_with_metadata("codex-worker", metadata);

    let mut req = factory_req("worker_activity");
    req.target = Some("codex-worker".to_string());
    let result = env
        .service
        .factory(Parameters(req))
        .await
        .expect("worker_activity should consume the resolved Codex rollout");
    let text = get_text(&result);

    assert!(
        text.contains("codex-worker - in-flight tool call"),
        "the same rollout fixture that refutes worker_status STALLED must appear here: {text}"
    );
    assert!(
        text.contains("transcript-backed"),
        "the feed must distinguish rollout freshness from CAS event rows: {text}"
    );
    assert!(
        !text.contains("No recent worker activity"),
        "an active Codex tool call must not produce the empty feed: {text}"
    );
}

// =============================================================================
// clear_context tests
// =============================================================================

/// Fixture for a Claude worker whose transcripts CAS can locate: a temp Claude
/// config dir with `projects/<cwd-slug>/`, plus a registered worker agent
/// carrying that cwd as its `clone_path`.
struct ClearContextFixture {
    _config: TempDir,
    _clone: TempDir,
    projects: PathBuf,
    worker: Agent,
}

fn clear_context_fixture(
    worker_name: &str,
    cli: &str,
    timeout_secs: &str,
) -> (EnvGuard, ClearContextFixture) {
    let config = TempDir::new().expect("config dir");
    let clone = TempDir::new().expect("clone dir");
    let clone_path = clone.path().to_string_lossy().to_string();
    let slug: String = clone_path
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect();
    let projects = config.path().join("projects").join(&slug);
    std::fs::create_dir_all(&projects).expect("projects dir");

    let guard = EnvGuard::set(&[
        ("CLAUDE_CONFIG_DIR", &config.path().to_string_lossy()),
        ("CAS_CONTEXT_RESET_TIMEOUT_SECS", timeout_secs),
    ]);

    let mut worker = Agent::new(format!("{worker_name}-session"), worker_name.to_string());
    worker.role = AgentRole::Worker;
    worker
        .metadata
        .insert("clone_path".to_string(), clone_path.clone());
    worker
        .metadata
        .insert("worker_cli".to_string(), cli.to_string());

    (
        guard,
        ClearContextFixture {
            _config: config,
            _clone: clone,
            projects,
            worker,
        },
    )
}

/// cas-dffe (GH #145), the regression this task exists for, pinned dead: the
/// queued row must be a CONTROL command, never the literal text `/clear`.
///
/// The old implementation queued the four characters `/clear` as an ordinary
/// message, which the daemon routes to a Claude worker's team inbox — the
/// worker read it as a teammate note and kept its whole conversation. If this
/// assertion ever flips back to `"/clear"`, the bug is back.
#[tokio::test]
async fn test_clear_context_queues_a_control_command_not_message_text() {
    let (guard, fixture) = clear_context_fixture("wolf", "claude", "0");
    let env = FactoryTestEnv::with_agent_id_and_env("test-sup", Some(guard));

    let store = env.agent_store();
    store
        .register(&Agent::new(
            "test-sup".to_string(),
            "supervisor".to_string(),
        ))
        .expect("register supervisor");
    store.register(&fixture.worker).expect("register worker");

    let mut req = factory_req("clear_context");
    req.target = Some("wolf".to_string());
    let result = env.service.factory(Parameters(req)).await;

    // No daemon is running in this fixture, so nothing types the command into a
    // pane and no post-reset transcript ever appears. The call must therefore
    // FAIL — reporting "queued" as success is precisely the reported bug.
    let error = result.expect_err("an unconfirmed reset must not be reported as success");
    let message = error.message.to_string();
    assert!(
        message.contains("UNCONFIRMED"),
        "failure must name the missing post-condition: {message}"
    );

    let prompts = env.prompt_queue().peek_all(10).expect("peek");
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].target, "wolf");
    assert_ne!(
        prompts[0].prompt, "/clear",
        "a context reset must never be queued as readable message text"
    );
    assert_eq!(
        prompts[0].prompt,
        cas::factory_context_reset::CONTEXT_RESET_CONTROL
    );
    assert!(
        prompts[0].urgent,
        "the reset takes the interrupt-and-inject lane so a mid-turn worker is reset too"
    );
    drop(fixture);
}

/// cas-dffe AC1/AC2/AC4: when the post-condition actually appears — a NEW
/// session transcript recording the `/clear` — the call succeeds, reports the
/// old→new session ids, and points CAS's transcript resolution at the live
/// session so `worker_status` reflects the reset.
#[tokio::test]
async fn test_clear_context_confirms_reset_from_new_session_transcript() {
    let (guard, fixture) = clear_context_fixture("otter", "claude", "10");
    let env = FactoryTestEnv::with_agent_id_and_env("test-sup", Some(guard));

    let store = env.agent_store();
    store
        .register(&Agent::new(
            "test-sup".to_string(),
            "supervisor".to_string(),
        ))
        .expect("register supervisor");
    store.register(&fixture.worker).expect("register worker");

    // Stand in for the daemon + harness: shortly after the control command is
    // queued, Claude Code starts a new session and writes its transcript.
    let projects = fixture.projects.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(600));
        std::fs::write(
            projects.join("11112222-3333-4444-5555-666677778888.jsonl"),
            format!(
                "{{\"type\":\"mode\"}}\n{{\"content\":\"{}\"}}\n",
                cas::factory_context_reset::CLEAR_COMMAND_MARKER
            ),
        )
        .expect("write post-clear transcript");
    });

    let mut req = factory_req("clear_context");
    req.target = Some("otter".to_string());
    let result = env.service.factory(Parameters(req)).await;

    let text = get_text(&result.expect("a confirmed reset must succeed"));
    assert!(text.contains("CONFIRMED"), "{text}");
    assert!(
        text.contains("11112222"),
        "the new conversation id is the post-condition: {text}"
    );
    assert!(
        text.contains("otter-se"),
        "the pre-reset session must be named so the change is checkable: {text}"
    );

    let reloaded = store.get(&fixture.worker.id).expect("reload worker");
    assert_eq!(
        reloaded.cc_session_id.as_deref(),
        Some("11112222-3333-4444-5555-666677778888"),
        "worker_status resolves transcripts by cc_session_id — it must follow the new session"
    );
}

/// cas-dffe AC2: a harness with no verified in-place reset is refused up front.
/// Nothing is queued, and the error names the harness and the alternative.
#[tokio::test]
async fn test_clear_context_refuses_harness_without_verified_reset() {
    let (guard, fixture) = clear_context_fixture("badger", "codex", "0");
    let env = FactoryTestEnv::with_agent_id_and_env("test-sup", Some(guard));

    let store = env.agent_store();
    store
        .register(&Agent::new(
            "test-sup".to_string(),
            "supervisor".to_string(),
        ))
        .expect("register supervisor");
    store.register(&fixture.worker).expect("register worker");

    let mut req = factory_req("clear_context");
    req.target = Some("badger".to_string());
    let error = env
        .service
        .factory(Parameters(req))
        .await
        .expect_err("an impossible reset must not report success");
    let message = error.message.to_string();
    assert!(message.contains("codex"), "{message}");
    assert!(
        message.contains("unsupported"),
        "the explicit fail-closed outcome must name Codex reset as unsupported: {message}"
    );
    assert!(message.contains("shutdown_workers"), "{message}");
    assert!(message.contains("spawn_workers"), "{message}");

    assert!(
        env.prompt_queue().peek_all(10).expect("peek").is_empty(),
        "nothing may be queued for a harness that cannot be reset"
    );
}

/// cas-dffe live measurement, codified: does typing the production reset
/// command into a REAL `claude` produce the production post-condition?
///
/// `#[ignore]` — spawns a real `claude` attached to a real PTY, so it is
/// excluded from the default gate per this repo's live/e2e convention. Run it
/// explicitly when touching `factory_context_reset` or the reset delivery path:
///
/// ```bash
/// cargo test -p cas --test factory_mcp_ops_test -- --ignored --nocapture \
///     clear_context_command_really_resets_a_live_claude
/// ```
///
/// This exists because of the cas-5fff lesson recorded in
/// `crates/cas-mux/tests/idle_pty_injection_runtime.rs`: a claim about a
/// harness's behavior is only valid for the harness it was measured against.
/// Everything else in this task's coverage is fixture-driven — the *only*
/// evidence that `/clear` typed over a PTY is a real command channel, and that
/// Claude Code answers it with a new session transcript recording the clear, is
/// this measurement. It deliberately uses the production helpers
/// (`context_reset_command` + `detect_context_reset`), not hand-written
/// equivalents, so a drift in either is caught here.
///
/// Note (measured the hard way): a child inheriting `CLAUDE_CODE_CHILD_SESSION`
/// runs with "Transcript saving is off" and writes NO transcript at all, which
/// looks exactly like a failed reset. The marker vars are stripped below.
#[test]
#[ignore = "spawns a real `claude` on a real PTY — run explicitly, see doc comment"]
fn clear_context_command_really_resets_a_live_claude() {
    use cas::factory_context_reset as reset;
    use cas_mux::{Pane, PaneKind, Pty, PtyConfig};

    if which_claude().is_none() {
        eprintln!("SKIP: `claude` is not on PATH");
        return;
    }
    // A directory Claude Code already trusts: the repo root this test is built
    // from. A fresh temp dir would stall on the trust dialog instead.
    let cwd = cas::test_paths::workspace_root();

    let stripped: Vec<(String, String)> = std::env::vars()
        .filter(|(key, _)| key.starts_with("CLAUDE_CODE"))
        .collect();
    for (key, _) in &stripped {
        unsafe { std::env::remove_var(key) };
    }

    let dirs = reset::transcript_dirs_for(&cwd.to_string_lossy());
    assert!(
        !dirs.is_empty(),
        "no Claude project directory for {} — run `claude` there once first",
        cwd.display()
    );
    let before = reset::snapshot_transcripts(&dirs);

    let pty = Pty::spawn(
        "cas-dffe-live",
        PtyConfig {
            command: "claude".to_string(),
            args: vec![],
            cwd: Some(cwd.clone()),
            env: vec![("TERM".to_string(), "xterm-256color".to_string())],
            env_remove: vec![],
            rows: 24,
            cols: 80,
        },
    )
    .expect("spawn claude on a pty");
    let mut pane = Pane::with_pty(
        "cas-dffe-live",
        PaneKind::Shell,
        pty,
        24,
        80,
        SupervisorCli::Claude,
    )
    .expect("wrap pty in pane");

    // Let the TUI boot; the readiness gate the daemon uses needs output + 5s.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        let _ = pane.drain_output();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(pane.ready_for_injection(), "claude never became injectable");

    let command = reset::context_reset_command(SupervisorCli::Claude)
        .expect("claude has a verified reset command");
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(pane.inject_prompt(command))
        .expect("inject the reset command");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut evidence = None;
    while std::time::Instant::now() < deadline {
        let _ = pane.drain_output();
        evidence = reset::detect_context_reset(&dirs, &before);
        if evidence.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }

    for (key, value) in stripped {
        unsafe { std::env::set_var(key, value) };
    }

    let evidence = evidence.expect(
        "typing the reset command into a live claude must start a NEW session whose transcript \
         records the /clear — that post-condition is the entire contract of clear_context",
    );
    eprintln!(
        "context reset confirmed: session {} at {}",
        evidence.session_id,
        evidence.transcript.display()
    );
}

fn which_claude() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("claude"))
            .find(|candidate| candidate.is_file())
    })
}

#[tokio::test]
async fn test_clear_context_all_workers_fans_out_to_live_workers() {
    let (guard, fixture) = clear_context_fixture("wolf", "claude", "0");
    let env = FactoryTestEnv::with_agent_id_and_env("test-sup", Some(guard));

    let store = env.agent_store();
    store
        .register(&Agent::new(
            "test-sup".to_string(),
            "supervisor".to_string(),
        ))
        .expect("register supervisor");
    store.register(&fixture.worker).expect("register worker");

    let mut req = factory_req("clear_context");
    req.target = Some("all_workers".to_string());
    let result = env.service.factory(Parameters(req)).await;
    assert!(
        result.is_err(),
        "no daemon is running, so no reset can be confirmed"
    );

    // `all_workers` resolves to concrete recipients: a control command is
    // addressed to each worker's pane, never to the un-routable literal
    // "all_workers" target.
    let prompts = env.prompt_queue().peek_all(10).expect("peek");
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].target, "wolf");
    assert_eq!(
        prompts[0].prompt,
        cas::factory_context_reset::CONTEXT_RESET_CONTROL
    );
}

// =============================================================================
// my_context tests
// =============================================================================

#[tokio::test]
async fn test_my_context_shows_agent_info() {
    let env = FactoryTestEnv::with_agent_id("ctx-agent-id");

    let store = env.agent_store();
    let mut agent = Agent::new("ctx-agent-id".to_string(), "ctx-supervisor".to_string());
    agent.role = AgentRole::Supervisor;
    store.register(&agent).expect("register");

    let req = factory_req("my_context");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(text.contains("ctx-supervisor"), "Should show name: {text}");
    assert!(text.contains("Supervisor"), "Should show role: {text}");
    assert!(text.contains("ctx-agent-id"), "Should show ID: {text}");
    assert!(text.contains("None (idle)"), "Should show no tasks: {text}");
}

// =============================================================================
// gc_report tests
// =============================================================================

#[tokio::test]
async fn test_gc_report_empty() {
    let env = FactoryTestEnv::new();

    let req = factory_req("gc_report");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("Stale agents: 0"),
        "Should show 0 stale: {text}"
    );
    assert!(
        text.contains("Pending prompts: 0"),
        "Should show 0 prompts: {text}"
    );
    assert!(
        text.contains("Orphan worker process groups: 0"),
        "Should expose the process-group GC surface: {text}"
    );
}

#[tokio::test]
async fn test_gc_report_shows_pending_prompts() {
    let env = FactoryTestEnv::new();

    // Add some pending prompts
    let pq = env.prompt_queue();
    pq.enqueue("src", "wolf", "do stuff").expect("enqueue");
    pq.enqueue("src", "fox", "do other stuff").expect("enqueue");

    let req = factory_req("gc_report");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("Pending prompts: 2"),
        "Should show 2 prompts: {text}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_gc_report_names_dangling_primary_node_modules_link() {
    let env = FactoryTestEnv::new();
    let repo = env.cas_root.parent().expect("CAS root has checkout parent");
    std::fs::write(repo.join("package.json"), "{}").unwrap();
    let link = repo.join("node_modules/backend");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink("/deleted/cassy-worktree/node_modules/backend", &link).unwrap();

    let report = env
        .service
        .factory(Parameters(factory_req("gc_report")))
        .await
        .expect("gc report");
    let text = get_text(&report);
    assert!(
        text.contains("Dangling primary-checkout node_modules symlinks"),
        "{text}"
    );
    assert!(text.contains(&link.display().to_string()), "{text}");
    assert!(text.contains("package-manager install"), "{text}");
}

/// GH #704: the operator had no way to see 1,242 leaked `cas-probe-comm-*`
/// roots before `/tmp` hit 100%. `gc_report` now names the stale Cassy-shaped
/// temp roots (read-only; it never deletes them).
#[tokio::test]
async fn test_gc_report_lists_stale_temp_roots_without_deleting_them() {
    let env = FactoryTestEnv::new();

    let report = env
        .service
        .factory(Parameters(factory_req("gc_report")))
        .await
        .expect("gc report");
    let text = get_text(&report);
    assert!(
        text.contains("Stale Cassy temp roots under"),
        "gc_report must surface the $TMPDIR inventory: {text}"
    );
    assert!(
        text.contains(&std::env::temp_dir().display().to_string()),
        "the inventory must name the directory it scanned: {text}"
    );
}

#[tokio::test]
async fn test_gc_artifacts_are_lifecycle_keyed_and_strays_are_review_only() {
    let env = FactoryTestEnv::new();
    let root = env.cas_root.join("durable-artifacts");
    let task_store = env.task_store();
    let closed_id = task_store.generate_id().expect("generate closed task id");
    let mut closed = Task::new(closed_id.clone(), "closed artifact owner".to_string());
    closed.status = TaskStatus::Closed;
    task_store.add(&closed).expect("add closed task");
    let open_id = task_store.generate_id().expect("generate open task id");
    task_store
        .add(&Task::new(
            open_id.clone(),
            "open artifact owner".to_string(),
        ))
        .expect("add open task");
    std::fs::create_dir_all(root.join(&closed_id)).unwrap();
    std::fs::create_dir_all(root.join(&open_id)).unwrap();
    std::fs::create_dir_all(root.join("operator-review-stray")).unwrap();
    std::fs::write(root.join(&closed_id).join("proof.txt"), "durable proof").unwrap();
    std::fs::write(
        env.cas_root.join("config.toml"),
        format!(
            "[factory]\nartifacts_root = {:?}\n",
            root.display().to_string()
        ),
    )
    .unwrap();

    let report = env
        .service
        .factory(Parameters(factory_req("gc_report")))
        .await
        .unwrap();
    let report_text = get_text(&report);
    assert!(
        report_text.contains("closed-task candidates=1"),
        "{report_text}"
    );
    assert!(
        report_text.contains("review-only stray artifact"),
        "{report_text}"
    );

    let mut preview = factory_req("gc_cleanup");
    preview.force = Some(true);
    let preview = env.service.factory(Parameters(preview)).await.unwrap();
    assert!(get_text(&preview).contains("mode=review-only"));
    assert!(
        root.join(&closed_id).exists(),
        "force alone must not delete durable proof"
    );

    let mut cleanup = factory_req("gc_cleanup");
    cleanup.force = Some(true);
    cleanup.dry_run = Some(false);
    let cleanup = env.service.factory(Parameters(cleanup)).await.unwrap();
    let cleanup_text = get_text(&cleanup);
    assert!(
        cleanup_text.contains("Closed-task artifact directories removed: 1"),
        "{cleanup_text}"
    );
    assert!(
        !root.join(&closed_id).exists(),
        "closed-task artifact should be GC'd"
    );
    assert!(
        root.join(&open_id).exists(),
        "open task artifact must be preserved"
    );
    assert!(
        root.join("operator-review-stray").exists(),
        "stray inventory must never delete"
    );
}

// Target-cache process liveness is implemented with Linux `/proc`; other
// platforms intentionally fail closed and cannot select a cache for cleanup.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn test_target_cache_gc_public_dry_run_and_explicit_cleanup() {
    let env = FactoryTestEnv::new();
    let worker = env.cas_root.join("worktrees/dead-cache-worker");
    let target = worker.join("target/deps");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("artifact.rlib"), vec![0u8; 64]).unwrap();
    std::fs::write(worker.join("source.rs"), b"source").unwrap();
    std::fs::write(
        env.cas_root.join("config.toml"),
        "[factory]\ntarget_cache_high_watermark_percent = 1\ntarget_cache_low_watermark_percent = 0\ntarget_cache_min_idle_secs = 0\ntarget_cache_retention_count = 0\n",
    )
    .unwrap();

    let report = env
        .service
        .factory(Parameters(factory_req("gc_report")))
        .await
        .unwrap();
    let report_text = get_text(&report);
    assert!(
        report_text.contains("TARGET_CACHE_STATUS_JSON="),
        "{report_text}"
    );
    let machine = report_text
        .lines()
        .find_map(|line| line.strip_prefix("TARGET_CACHE_STATUS_JSON="))
        .expect("machine-readable target-cache status line");
    let machine: serde_json::Value = serde_json::from_str(machine).expect("valid status JSON");
    assert_eq!(machine["schema_version"], 1);
    assert_eq!(machine["dry_run"], true);
    assert!(
        report_text.contains(&worker.join("target").display().to_string()),
        "dry-run must report the exact cache path: {report_text}"
    );
    assert!(report_text.contains("bytes=64"), "{report_text}");

    let mut preview = factory_req("gc_cleanup");
    preview.force = Some(true);
    let preview = env.service.factory(Parameters(preview)).await.unwrap();
    let preview_text = get_text(&preview);
    assert!(preview_text.contains("mode=dry-run"), "{preview_text}");
    assert!(
        worker.join("target").exists(),
        "omitted dry_run must not delete"
    );

    let mut cleanup = factory_req("gc_cleanup");
    cleanup.force = Some(true);
    cleanup.dry_run = Some(false);
    let cleanup = env.service.factory(Parameters(cleanup)).await.unwrap();
    let cleanup_text = get_text(&cleanup);
    assert!(
        cleanup_text.contains("reclaimed_bytes=64"),
        "{cleanup_text}"
    );
    assert!(!worker.join("target").exists());
    assert_eq!(std::fs::read(worker.join("source.rs")).unwrap(), b"source");
}

// =============================================================================
// gc_cleanup tests
// =============================================================================

#[tokio::test]
async fn test_gc_cleanup_without_force() {
    let env = FactoryTestEnv::new();

    // Add pending prompts
    let pq = env.prompt_queue();
    pq.enqueue("src", "wolf", "test").expect("enqueue");

    let req = factory_req("gc_cleanup");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("Prompt queue entries cleared: 0"),
        "Should NOT clear prompts without force: {text}"
    );
    assert!(
        text.contains("Orphan worker process groups reaped: 0"),
        "Should report process-group cleanup outcome: {text}"
    );

    // Prompts should still be pending
    assert_eq!(pq.pending_count().expect("count"), 1);
}

#[tokio::test]
async fn test_gc_cleanup_removes_only_stale_skill_markers_and_invalid_bare_marker() {
    let env = FactoryTestEnv::new();
    let stale = env.cas_root.join("session_skills_seen_old-session");
    let current = env.cas_root.join("session_skills_seen_current-session");
    let bare = env.cas_root.join("session_skills_seen_");
    std::fs::write(&stale, "old").unwrap();
    std::fs::write(&current, "current").unwrap();
    std::fs::write(&bare, "invalid").unwrap();
    filetime::set_file_mtime(&stale, filetime::FileTime::from_unix_time(1, 0)).unwrap();

    let result = env
        .service
        .factory(Parameters(factory_req("gc_cleanup")))
        .await
        .unwrap();
    let text = get_text(&result);

    assert!(text.contains("Stale skill markers removed: 2"), "{text}");
    assert!(!stale.exists(), "old marker should be removed");
    assert!(
        !bare.exists(),
        "invalid empty-session marker should be removed"
    );
    assert!(current.exists(), "live session marker must be preserved");
}

#[tokio::test]
async fn test_gc_cleanup_with_force() {
    let env = FactoryTestEnv::new();

    let pq = env.prompt_queue();
    pq.enqueue("src", "wolf", "test1").expect("enqueue");
    pq.enqueue("src", "fox", "test2").expect("enqueue");

    let mut req = factory_req("gc_cleanup");
    req.force = Some(true);

    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("Prompt queue entries cleared: 2"),
        "Should clear prompts with force: {text}"
    );

    assert_eq!(pq.pending_count().expect("count"), 0);
}

#[tokio::test]
async fn test_gc_cleanup_force_with_age_expires_without_deleting_prompt_rows() {
    let env = FactoryTestEnv::new();
    let pq = env.prompt_queue();
    let id = pq.enqueue("src", "dead-worker", "poison").expect("enqueue");

    let mut req = factory_req("gc_cleanup");
    req.force = Some(true);
    req.older_than_secs = Some(0);
    let result = env.service.factory(Parameters(req)).await.unwrap();
    let text = get_text(&result);

    assert!(
        text.contains("Prompt queue entries expired: 1"),
        "targeted remediation must report terminalized rows: {text}"
    );
    assert!(
        text.contains("Prompt queue entries cleared: 0"),
        "age-targeted remediation must preserve history: {text}"
    );
    assert_eq!(pq.pending_count().unwrap(), 0);
    let report = pq
        .message_delivery_report(id)
        .unwrap()
        .expect("expired row remains queryable");
    assert_eq!(report.stage, cas_store::DeliveryStage::Abandoned);
    assert!(report.delivered_at.is_none());
}

#[tokio::test]
async fn test_gc_cleanup_purges_stale_and_shutdown_worker_records() {
    let env = FactoryTestEnv::new();

    let stale_id = env.register_worker_with_status("stale-wolf", AgentStatus::Stale);
    let shutdown_id = env.register_worker_with_status("shutdown-fox", AgentStatus::Shutdown);

    let req = factory_req("gc_cleanup");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("Dead agent records purged: 2"),
        "Should report purged dead agent records: {text}"
    );

    let store = env.agent_store();
    assert!(
        store.get(&stale_id).is_err(),
        "stale worker should be purged"
    );
    assert!(
        store.get(&shutdown_id).is_err(),
        "shutdown worker should be purged"
    );
}

#[tokio::test]
async fn test_gc_cleanup_preserves_stale_supervisors() {
    let env = FactoryTestEnv::new();

    let supervisor_id = env.register_supervisor("stale-supervisor");
    let store = env.agent_store();
    let mut supervisor = store.get(&supervisor_id).expect("get supervisor");
    supervisor.status = AgentStatus::Stale;
    store.update(&supervisor).expect("mark supervisor stale");

    let req = factory_req("gc_cleanup");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    let text = get_text(&result.unwrap());
    assert!(
        text.contains("Dead agent records purged: 0"),
        "Should not purge supervisor/director records: {text}"
    );
    assert!(
        store.get(&supervisor_id).is_ok(),
        "stale supervisor record should be preserved"
    );
}

// =============================================================================
// Sequence tests
// =============================================================================

#[tokio::test]
async fn test_spawn_then_shutdown_sequence() {
    let _guard = EnvGuard::set_optional(&[("CAS_FACTORY_SESSION", None)]);
    let env = FactoryTestEnv::new();
    env.create_epic("Sequence Epic");
    env.register_worker("alpha");

    // Spawn
    let mut req = factory_req("spawn_workers");
    req.count = Some(2);
    req.cli = Some("claude".to_string());
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    // Shutdown
    let mut req = factory_req("shutdown_workers");
    req.worker_names = Some("alpha".to_string());
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_ok());

    // Both should be in queue
    let entries = env.spawn_queue().peek(10).expect("peek");
    assert_eq!(entries.len(), 2, "Should have 2 queue entries");
    assert_eq!(entries[0].action, cas_store::SpawnAction::Spawn);
    assert_eq!(entries[1].action, cas_store::SpawnAction::Shutdown);
}

#[tokio::test]
async fn test_unknown_action() {
    let env = FactoryTestEnv::new();

    let req = factory_req("invalid_action");
    let result = env.service.factory(Parameters(req)).await;
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert!(
        err.message.contains("Unknown factory action"),
        "Should report unknown action: {}",
        err.message
    );
}

// =============================================================================
// cas-337e: worker-side inbox polling handler
// =============================================================================

#[tokio::test]
async fn inbox_poll_uses_registered_identity_and_session_and_claims_processed_unacked_rows() {
    let _guard = EnvGuard::set_optional(&[
        ("CAS_AGENT_NAME", Some("wrong-env-name")),
        ("CAS_SESSION_ID", None),
        ("CAS_FACTORY_SESSION", None),
    ]);
    let env = FactoryTestEnv::with_agent_id("registered-worker-id");
    env.register_worker_with_id(
        "registered-worker-id",
        "registered-worker",
        Some("session-a"),
    );
    let queue = env.prompt_queue();
    let processed = queue
        .enqueue_with_session(
            "supervisor",
            "registered-worker",
            "processed but unacked",
            "session-a",
        )
        .unwrap();
    queue.mark_processed(processed).unwrap();
    queue
        .enqueue_with_session(
            "supervisor",
            "registered-worker",
            "wrong session",
            "session-b",
        )
        .unwrap();
    queue
        .enqueue("supervisor", "registered-worker", "legacy message")
        .unwrap();

    let first = env
        .service
        .coordination(Parameters(coord_req("inbox_poll")))
        .await
        .expect("registered inbox poll");
    let text = get_text(&first);
    assert!(text.contains("for registered-worker"), "{text}");
    assert!(text.contains("processed but unacked"), "{text}");
    assert!(text.contains("legacy message"), "{text}");
    assert!(!text.contains("wrong session"), "{text}");
    assert!(text.contains("at-most-once inbox claim"), "{text}");

    let second = env
        .service
        .coordination(Parameters(coord_req("inbox_poll")))
        .await
        .expect("second registered inbox poll");
    assert_eq!(
        get_text(&second),
        "No unread messages for registered-worker"
    );
}

/// cas-53a7: the MCP reader must mirror its real receipt across every alias a
/// supervisor answers to.  A broadcast reaches the pane-name alias first; if
/// this reader drops `mirror_receipts_across_aliases`, the logical
/// `supervisor` alias remains unread and this assertion fails without any
/// unit mock of the helper.
#[tokio::test]
async fn inbox_poll_writes_receipts_for_every_supervisor_alias() {
    const SUPERVISOR: &str = "receipt-supervisor";

    let _guard = EnvGuard::set_optional(&[
        ("CAS_AGENT_NAME", Some(SUPERVISOR)),
        ("CAS_SESSION_ID", None),
        ("CAS_FACTORY_SESSION", None),
    ]);
    let env = FactoryTestEnv::with_agent_id("receipt-supervisor-id");
    let store = env.agent_store();
    let mut supervisor = Agent::new("receipt-supervisor-id".to_string(), SUPERVISOR.to_string());
    supervisor.role = AgentRole::Supervisor;
    store
        .register(&supervisor)
        .expect("register calling supervisor");

    let aliases = cas::harness_policy::inbox_aliases(SUPERVISOR, true);
    assert!(
        aliases.len() > 1,
        "precondition: this must exercise the supervisor's full alias set"
    );
    let queue = env.prompt_queue();
    // Fill the first alias's poll limit. The remaining alias is not polled in
    // this response, so only the production mirror call can receipt it.
    for index in 0..10 {
        queue
            .enqueue(
                "director",
                "all_workers",
                &format!("receipt must mirror through MCP inbox_poll #{index}"),
            )
            .expect("enqueue broadcast");
    }

    let text = get_text(
        &env.service
            .coordination(Parameters(coord_req("inbox_poll")))
            .await
            .expect("inbox_poll"),
    );
    assert!(
        text.contains("receipt must mirror through MCP inbox_poll #0"),
        "precondition: the real MCP handler must return the broadcast: {text}"
    );
    for alias in aliases {
        assert_eq!(
            queue.count_unseen_for_recipient(&alias, None).unwrap(),
            0,
            "inbox_poll must write a receipt for supervisor alias {alias}"
        );
    }
}

#[tokio::test]
async fn inbox_poll_sessionless_registered_agent_only_reads_legacy_rows() {
    let _guard = EnvGuard::set_optional(&[
        ("CAS_AGENT_NAME", Some("wrong-env-name")),
        ("CAS_SESSION_ID", None),
        ("CAS_FACTORY_SESSION", None),
    ]);
    let env = FactoryTestEnv::with_agent_id("sessionless-worker-id");
    env.register_worker_with_id("sessionless-worker-id", "sessionless-worker", None);
    let queue = env.prompt_queue();
    queue
        .enqueue("supervisor", "sessionless-worker", "legacy visible")
        .unwrap();
    queue
        .enqueue_with_session(
            "supervisor",
            "sessionless-worker",
            "session hidden",
            "session-a",
        )
        .unwrap();

    let result = env
        .service
        .coordination(Parameters(coord_req("inbox_poll")))
        .await
        .expect("sessionless registered inbox poll");
    let text = get_text(&result);
    assert!(text.contains("for sessionless-worker"), "{text}");
    assert!(text.contains("legacy visible"), "{text}");
    assert!(!text.contains("session hidden"), "{text}");
}

#[tokio::test]
async fn inbox_poll_applies_default_limit_and_hard_cap() {
    let _guard = EnvGuard::set_optional(&[
        ("CAS_AGENT_NAME", Some("env-worker")),
        ("CAS_SESSION_ID", None),
        ("CAS_FACTORY_SESSION", None),
    ]);
    let env = FactoryTestEnv::without_agent_id();
    let queue = env.prompt_queue();
    for index in 0..115 {
        queue
            .enqueue("supervisor", "env-worker", &format!("message-{index:03}"))
            .unwrap();
    }

    let default_result = env
        .service
        .coordination(Parameters(coord_req("inbox_poll")))
        .await
        .expect("default-limit inbox poll");
    assert!(
        get_text(&default_result).starts_with("Pulled 10 unread message(s)"),
        "{}",
        get_text(&default_result)
    );

    let mut capped_req = coord_req("inbox_poll");
    capped_req.limit = Some(usize::MAX);
    let capped_result = env
        .service
        .coordination(Parameters(capped_req))
        .await
        .expect("capped inbox poll");
    assert!(
        get_text(&capped_result).starts_with("Pulled 100 unread message(s)"),
        "{}",
        get_text(&capped_result)
    );

    let final_result = env
        .service
        .coordination(Parameters(coord_req("inbox_poll")))
        .await
        .expect("remaining inbox poll");
    assert!(
        get_text(&final_result).starts_with("Pulled 5 unread message(s)"),
        "{}",
        get_text(&final_result)
    );
}

// =============================================================================
// cas-c931: urgent (interrupt-and-redirect) message routing
// =============================================================================

/// Minimal CoordinationRequest for the `message`/`interrupt` actions.
fn coord_msg(
    action: &str,
    target: &str,
    message: &str,
    urgent: Option<bool>,
) -> CoordinationRequest {
    CoordinationRequest {
        action: action.to_string(),
        id: None,
        task_id: None,
        delivery_mode: None,
        merge_request: None,
        in_reply_to: None,
        target: Some(target.to_string()),
        message: Some(message.to_string()),
        summary: Some("test".to_string()),
        urgent,
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

/// Default (non-urgent) coordination message must enqueue with urgent=false —
/// the unchanged inbox/queue delivery path. Regression guard.
#[tokio::test]
async fn test_coordination_message_default_is_not_urgent() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "supervisor"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_worker("swift-fox");

    let req = coord_msg("message", "swift-fox", "FYI: status update", None);
    let result = env.service.coordination(Parameters(req)).await;
    assert!(result.is_ok(), "message should succeed: {result:?}");

    let prompts = env.prompt_queue().peek_all(10).expect("peek");
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].target, "swift-fox");
    assert!(!prompts[0].urgent, "default message must not be urgent");
}

/// `urgent=true` to a Claude teammate must not report success in a hermetic
/// environment where no daemon can provide a recipient-side transport stamp.
/// The urgent + Critical row remains durable for the daemon to retry later.
/// Paused Tokio time keeps the production eight-second confirmation contract
/// while making this negative-path integration test complete immediately.
#[tokio::test(start_paused = true)]
async fn test_coordination_message_urgent_flag_enqueues_urgent() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "supervisor"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_worker("swift-fox");

    let req = coord_msg("message", "swift-fox", "STOP — wrong file", Some(true));
    let result = env.service.coordination(Parameters(req)).await;
    let error = result.expect_err("unobserved Claude interrupt must fail explicitly");
    assert!(
        error
            .message
            .contains("Could not confirm Claude interrupt delivery"),
        "error must name the unconfirmed interrupt: {}",
        error.message
    );
    assert!(
        error.message.contains(
            "within 8s (stage=enqueued, wake_attempt=nudge_not_attempted, recipient_transport=unobserved)"
        ),
        "error must expose the exact unobserved delivery state: {}",
        error.message
    );
    assert!(
        error
            .message
            .contains("queue row remains durable for retry"),
        "error must explain the durable retry contract: {}",
        error.message
    );

    let prompts = env.prompt_queue().peek_all(10).expect("peek");
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].urgent,
        "urgent=true must persist on the queue row"
    );
    assert_eq!(
        prompts[0].priority,
        cas_store::NotificationPriority::Critical,
        "urgent with no explicit priority defaults to Critical so it jumps the queue"
    );
}

/// When the daemon records the recipient-side transport stamp inside the
/// confirmation window, the same Claude urgent call succeeds.
#[tokio::test]
async fn test_coordination_message_urgent_succeeds_after_observed_delivery() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "supervisor"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_worker("swift-fox");

    let queue = env.prompt_queue();
    let observe_delivery = async {
        for _ in 0..100 {
            if let Some(prompt) = queue
                .peek_all(10)
                .expect("peek")
                .into_iter()
                .find(|prompt| prompt.target == "swift-fox")
            {
                queue
                    .mark_transport_delivered(prompt.id)
                    .expect("record recipient-side transport delivery");
                return prompt.id;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        panic!("urgent prompt was not enqueued for delivery observation");
    };

    let req = coord_msg("message", "swift-fox", "STOP — wrong file", Some(true));
    let (result, message_id) =
        tokio::join!(env.service.coordination(Parameters(req)), observe_delivery);
    let result = result.expect("observed Claude interrupt should succeed");
    let text = get_text(&result);
    assert!(
        text.contains("URGENT"),
        "response should mark URGENT: {text}"
    );

    let report = queue
        .message_delivery_report(message_id)
        .expect("read delivery report")
        .expect("delivery report exists");
    assert_eq!(report.stage, cas_store::DeliveryStage::Delivered);
    assert!(
        report.recipient_transport_at.is_some(),
        "success must be backed by recipient-side transport evidence"
    );
}

/// cas-126b: an urgent "MERGE DONE → re-close now" hand-off to the worker that
/// OWNS the parked task must NOT arm halt_task_work — otherwise the re-close it
/// asks for deadlocks (WORK HALTED). The urgent is still enqueued (worker wakes).
#[tokio::test(start_paused = true)]
async fn test_126b_target_awaiting_merge_reclose_urgent_does_not_halt() {
    let _guard = EnvGuard::set_optional(&[
        ("CAS_AGENT_ROLE", Some("supervisor")),
        ("CAS_AGENT_NAME", Some("supervisor")),
        // No factory session → halt fan-out is unfiltered (worker in scope).
        ("CAS_FACTORY_SESSION", None),
    ]);
    let env = FactoryTestEnv::with_server_supervisor();
    env.register_worker("swift-fox");
    let task_id = env.create_awaiting_merge_task("parked work", "swift-fox");

    let msg = format!(
        "MERGE DONE: factory/swift-fox merged to epic. \
         Re-close now: task action=close id={task_id} reason=\"merged\""
    );
    let req = coord_msg("message", "swift-fox", &msg, Some(true));
    let result = env.service.coordination(Parameters(req)).await;
    let error = result.expect_err("unobserved Claude urgent must fail explicitly");
    assert!(
        error
            .message
            .contains("Could not confirm Claude interrupt delivery"),
        "unexpected urgent error: {}",
        error.message
    );

    // Still enqueued as an urgent so the worker wakes.
    let prompts = env.prompt_queue().peek_all(10).expect("peek");
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].urgent,
        "merge re-close must still enqueue urgent"
    );
    // But halt must NOT be armed on the owning worker.
    assert!(
        !env.worker_halted("swift-fox"),
        "merge-complete re-close to the owning worker must not arm halt_task_work"
    );
}

/// cas-126b scope/authorization gate: an urgent close-guidance message to worker
/// B that merely NAMES worker A's parked task must STILL halt B — the exemption
/// binds to the target's OWN AwaitingMerge task, not any AwaitingMerge task.
#[tokio::test(start_paused = true)]
async fn test_126b_other_workers_task_does_not_exempt_target() {
    let _guard = EnvGuard::set_optional(&[
        ("CAS_AGENT_ROLE", Some("supervisor")),
        ("CAS_AGENT_NAME", Some("supervisor")),
        ("CAS_FACTORY_SESSION", None),
    ]);
    let env = FactoryTestEnv::with_server_supervisor();
    env.register_worker("swift-fox"); // target B
    env.register_worker("brave-otter"); // owner A
    // Parked task belongs to A, not to the target B.
    let a_task = env.create_awaiting_merge_task("A's parked work", "brave-otter");

    let msg = format!("MERGE DONE — task action=close id={a_task} reason=\"merged\"");
    let req = coord_msg("message", "swift-fox", &msg, Some(true));
    let result = env.service.coordination(Parameters(req)).await;
    let error = result.expect_err("unobserved Claude urgent must fail explicitly");
    assert!(
        error
            .message
            .contains("Could not confirm Claude interrupt delivery"),
        "unexpected urgent error: {}",
        error.message
    );

    assert!(
        env.worker_halted("swift-fox"),
        "close guidance naming another worker's parked task must NOT exempt the \
         target — halt must still fire"
    );
}

/// cas-126b: an ordinary urgent stop/redirect (not close guidance) must STILL
/// arm halt_task_work even if the target owns a parked AwaitingMerge task.
#[tokio::test(start_paused = true)]
async fn test_126b_ordinary_urgent_still_halts_even_with_parked_task() {
    let _guard = EnvGuard::set_optional(&[
        ("CAS_AGENT_ROLE", Some("supervisor")),
        ("CAS_AGENT_NAME", Some("supervisor")),
        ("CAS_FACTORY_SESSION", None),
    ]);
    let env = FactoryTestEnv::with_server_supervisor();
    env.register_worker("swift-fox");
    let _parked = env.create_awaiting_merge_task("parked work", "swift-fox");

    let req = coord_msg(
        "message",
        "swift-fox",
        "STOP — you are on the wrong file",
        Some(true),
    );
    let result = env.service.coordination(Parameters(req)).await;
    let error = result.expect_err("unobserved Claude urgent must fail explicitly");
    assert!(
        error
            .message
            .contains("Could not confirm Claude interrupt delivery"),
        "unexpected urgent error: {}",
        error.message
    );

    assert!(
        env.worker_halted("swift-fox"),
        "ordinary urgent stop must still arm halt_task_work (stale-work protection)"
    );
}

/// cas-126b fail-closed: an urgent close-guidance message with no positive
/// AwaitingMerge evidence for the target (no matching task in the store — the
/// same net branch as a task-store read error) must STILL halt. The exemption
/// only fires on positive, target-bound AwaitingMerge evidence.
#[tokio::test(start_paused = true)]
async fn test_126b_close_guidance_without_target_awaiting_task_fails_closed_to_halt() {
    let _guard = EnvGuard::set_optional(&[
        ("CAS_AGENT_ROLE", Some("supervisor")),
        ("CAS_AGENT_NAME", Some("supervisor")),
        ("CAS_FACTORY_SESSION", None),
    ]);
    let env = FactoryTestEnv::with_server_supervisor();
    env.register_worker("swift-fox");
    // No AwaitingMerge task exists for swift-fox (store returns none → no
    // positive evidence; identical safe outcome to a store read error).

    let req = coord_msg(
        "message",
        "swift-fox",
        "MERGE DONE — task action=close id=cas-9999 reason=\"merged\"",
        Some(true),
    );
    let result = env.service.coordination(Parameters(req)).await;
    let error = result.expect_err("unobserved Claude urgent must fail explicitly");
    assert!(
        error
            .message
            .contains("Could not confirm Claude interrupt delivery"),
        "unexpected urgent error: {}",
        error.message
    );

    assert!(
        env.worker_halted("swift-fox"),
        "close guidance without a target-owned AwaitingMerge task must fail closed \
         (halt still armed)"
    );
}

/// cas-6913 AC2: a message to a target that IS registered must say so
/// honestly — the response must confirm registration ("target is
/// registered") and (cas-893c) must not claim delivery for a merely
/// enqueued message. Regression guard against re-collapsing the two cases.
#[tokio::test]
async fn test_coordination_message_to_registered_target_reports_delivery_status() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "supervisor"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_worker("swift-fox");

    let req = coord_msg("message", "swift-fox", "status update", None);
    let result = env.service.coordination(Parameters(req)).await;
    assert!(result.is_ok(), "message should succeed: {result:?}");
    let text = get_text(&result.unwrap());
    assert!(
        text.contains("target is registered"),
        "response should confirm the target is registered: {text}"
    );
    assert!(
        !text.contains("not yet registered"),
        "a registered target must not read as unregistered: {text}"
    );
}

/// cas-893c AC2: a non-urgent message to a registered target must NOT read
/// as delivered. "queued for next poll" previously implied success; the
/// response must now say it's merely enqueued and point at `message_status`
/// to check the real state before escalating.
#[tokio::test]
async fn test_coordination_message_non_urgent_does_not_claim_delivery() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "supervisor"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_worker("swift-fox");

    let req = coord_msg("message", "swift-fox", "status update", None);
    let result = env.service.coordination(Parameters(req)).await;
    assert!(result.is_ok(), "message should succeed: {result:?}");
    let text = get_text(&result.unwrap());
    assert!(
        !text.contains("queued for next poll"),
        "must not use the old delivery-implying phrasing: {text}"
    );
    assert!(
        text.contains("not yet confirmed delivered"),
        "must honestly state delivery is not confirmed: {text}"
    );
    assert!(
        text.contains("message_status"),
        "must point the sender at message_status to check the real state: {text}"
    );
}

/// cas-6ad2: a worker response after the supervisor message that prompted the
/// work is meaningful activity evidence. Keep the original test intent — the
/// reply is correlated with the prior delivered row — without overstating that
/// correlation as an explicit acknowledgement.
///
/// cas-dcf2 (GH #390) deliberately stages even a post-handoff reply with a
/// surfacing receipt as `AssumedSeen`: only `message_ack` may confirm receipt.
/// The no-receipt variant remains covered by
/// `cas99d2_status_keeps_counting_when_a_reply_lacks_a_surfacing_receipt_gh126`.
#[tokio::test]
async fn test_worker_response_confirms_consumed_supervisor_message() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_AGENT_NAME", "swift-fox"),
        ("CAS_SUPERVISOR_NAME", "cosmic-bear-43"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_supervisor("cosmic-bear-43");
    let instruction = env
        .prompt_queue()
        .enqueue_urgent(
            "supervisor",
            "swift-fox",
            "start cas-6ad2",
            None,
            Some("assignment"),
            None,
            false,
        )
        .expect("enqueue supervisor instruction");
    env.prompt_queue()
        .mark_transport_delivered(instruction)
        .expect("deliver supervisor instruction");
    // cas-99d2: the surfacing receipt — the worker pulled the row through its
    // own inbox, so CAS observed the content reaching it.
    env.prompt_queue()
        .poll_unseen_for_recipient("swift-fox", None, 10)
        .expect("worker inbox drain");

    let reply = coord_msg(
        "message",
        "supervisor",
        "cas-6ad2 characterization reproduced",
        None,
    );
    let result = env.service.coordination(Parameters(reply)).await;
    assert!(result.is_ok(), "worker response should succeed: {result:?}");

    let report = env
        .prompt_queue()
        .message_delivery_report(instruction)
        .expect("delivery report")
        .expect("instruction exists");
    assert_eq!(
        report.stage,
        cas_store::DeliveryStage::AssumedSeen,
        "a correlated reply is useful activity evidence, not an acknowledgement"
    );
    assert!(
        report.confirmed_at.is_none(),
        "only explicit message_ack may mark a message confirmed"
    );
    assert!(
        report.assumed_seen_at.is_some(),
        "the reply should still record the weaker assumed-seen stage"
    );
}

/// cas-85fd: an urgent stop is discharged by the worker's demonstrated reply
/// to that exact urgent prompt. It must not survive as collateral state and
/// veto a later close that has reached its normal lifecycle gates.
#[tokio::test]
async fn cas_85fd_answered_urgent_does_not_block_later_unrelated_close() {
    let mut role_guard = EnvGuard::set_optional(&[
        ("CAS_AGENT_ROLE", Some("supervisor")),
        ("CAS_AGENT_NAME", Some("supervisor")),
        ("CAS_SUPERVISOR_NAME", Some("supervisor")),
        ("CAS_FACTORY_SESSION", None),
    ]);
    let env = FactoryTestEnv::with_server_supervisor();
    let worker_id = env.register_worker("swift-fox");

    let urgent = coord_msg(
        "message",
        "swift-fox",
        "URGENT: stop and report your current status",
        Some(true),
    );
    let error = env
        .service
        .coordination(Parameters(urgent))
        .await
        .expect_err("the fixture has no live Claude pane to confirm");
    assert!(
        error
            .message
            .contains("Could not confirm Claude interrupt delivery"),
        "unexpected urgent error: {}",
        error.message
    );
    let urgent_id = env
        .prompt_queue()
        .peek_all(10)
        .expect("peek")
        .into_iter()
        .next()
        .expect("urgent row")
        .id;
    assert!(env.worker_halted("swift-fox"), "urgent must arm the halt");

    // The worker's inbox drain is the surfacing receipt required before a
    // reply may count as consuming the instruction.
    env.prompt_queue()
        .mark_transport_delivered(urgent_id)
        .expect("deliver urgent");
    env.prompt_queue()
        .poll_unseen_for_recipient("swift-fox", None, 10)
        .expect("surface urgent to worker");

    let worker_core = CasCore::with_daemon(env.cas_root.clone(), None, None);
    worker_core.set_agent_id_for_testing(worker_id);
    let worker_service = CasService::new(worker_core, None);
    role_guard._guard.set("CAS_AGENT_ROLE", "worker");
    role_guard._guard.set("CAS_AGENT_NAME", "swift-fox");
    worker_service
        .coordination(Parameters(coord_msg(
            "message",
            "supervisor",
            "ACK: stopped and awaiting direction",
            None,
        )))
        .await
        .expect("worker reply");
    let urgent_report = env
        .prompt_queue()
        .message_delivery_report(urgent_id)
        .expect("urgent report")
        .expect("urgent exists");
    assert_eq!(
        urgent_report.stage,
        cas_store::DeliveryStage::AssumedSeen,
        "the reply alone must remain weaker than confirmation: {urgent_report:?}"
    );

    // cas-dcf2: keep cas-85fd's important no-collateral-halt contract, but
    // discharge only after the worker explicitly acknowledges this exact
    // urgent. The old reply-only assertion would weaken honest staging.
    let mut acknowledge = coord_msg("message_ack", "unused", "unused", None);
    acknowledge.notification_id = Some(urgent_id);
    worker_service
        .coordination(Parameters(acknowledge))
        .await
        .expect("worker explicitly acknowledges the urgent");
    let urgent_report = env
        .prompt_queue()
        .message_delivery_report(urgent_id)
        .expect("urgent report after explicit acknowledgement")
        .expect("urgent exists");
    assert_eq!(
        urgent_report.stage,
        cas_store::DeliveryStage::Confirmed,
        "the exact explicit acknowledgement must confirm the urgent before it can discharge the halt: {urgent_report:?}"
    );
    assert!(
        !env.worker_halted("swift-fox"),
        "a confirmed response must discharge only the urgent exchange it answered; metadata={:?}",
        env.agent_store()
            .list(None)
            .expect("agents")
            .into_iter()
            .find(|agent| agent.name == "swift-fox")
            .expect("worker")
            .metadata
    );

    // This task was not the subject of the status check. With the exchange
    // discharged, close reaches its ordinary gates rather than returning the
    // stale WORK HALTED veto.
    let task_id = env.task_store().generate_id().expect("task id");
    let mut task = Task::new(task_id.clone(), "unrelated merged work".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("someone-else".to_string());
    env.task_store().add(&task).expect("add unrelated task");
    let close = worker_service
        .inner
        .cas_task_close(Parameters(cas::mcp::tools::TaskCloseRequest {
            stranded_branch_override: None,
            id: task_id.clone(),
            reason: Some("already merged before the urgent status check".to_string()),
            supervisor_override: None,
            legacy_bypass_code_review: None,
            search_manifest: None,
            commit_receipt: None,
        }))
        .await
        .expect("answered urgent must allow the later close to proceed");
    let close_text = get_text(&close);
    assert!(
        !close_text.contains("WORK HALTED"),
        "answered urgent must not leave a collateral halt: {close_text}"
    );
}

/// cas-ae2f AC1/AC2: exercise the real factory spawn configuration through the
/// public coordination handler. This is intentionally wider than a resolver
/// unit test: the supervisor identity must cross the Codex PTY -> MCP env
/// boundary before a worker can address the logical `supervisor` alias.
#[tokio::test]
async fn test_real_factory_codex_worker_can_message_supervisor_alias() {
    let mux_config = MuxConfig {
        cwd: PathBuf::from("/tmp/test"),
        workers: 1,
        worker_names: vec!["swift-fox".to_string()],
        supervisor_name: "cosmic-bear-43".to_string(),
        factory_session: Some("factory-message-e2e".to_string()),
        include_director: false,
        supervisor_cli: SupervisorCli::Codex,
        worker_cli: SupervisorCli::Codex,
        ..MuxConfig::default()
    };
    let configs = Mux::factory_pane_configs(&mux_config);
    let worker_config = &configs
        .iter()
        .find(|(name, _)| name == "swift-fox")
        .expect("spawned worker config")
        .1;
    let supervisor_override = worker_config
        .args
        .iter()
        .find_map(|arg| {
            arg.strip_prefix("mcp_servers.cs.env.CAS_SUPERVISOR_NAME=\"")
                .and_then(|value| value.strip_suffix('"'))
        })
        .expect("factory spawn must inject supervisor identity into Codex cs MCP env");

    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_AGENT_NAME", "swift-fox"),
        ("CAS_FACTORY_SESSION", "factory-message-e2e"),
        ("CAS_SUPERVISOR_NAME", supervisor_override),
    ]);
    let env = FactoryTestEnv::new();
    env.register_supervisor_in_session("cosmic-bear-43", "factory-message-e2e");

    let request = coord_msg(
        "message",
        "supervisor",
        "factory spawn-to-message behavior proof",
        None,
    );
    let result = env
        .service
        .coordination(Parameters(request))
        .await
        .expect("worker must resolve and enqueue to supervisor alias");
    let text = get_text(&result);
    assert!(text.contains("To: cosmic-bear-43"), "{text}");

    let prompts = env.prompt_queue().peek_all(10).expect("peek messages");
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].target, "cosmic-bear-43");
}

#[tokio::test]
async fn test_worker_unresolvable_message_target_fails_without_enqueue() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_AGENT_NAME", "swift-fox"),
        ("CAS_SUPERVISOR_NAME", "cosmic-bear-43"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_supervisor("cosmic-bear-43");

    let request = coord_msg("message", "typo-supervisor", "must not queue", None);
    let error = env
        .service
        .coordination(Parameters(request))
        .await
        .expect_err("unresolvable worker target must fail at call time");
    assert!(
        error
            .message
            .contains("Workers can only message their supervisor"),
        "unexpected error: {error:?}"
    );
    assert!(
        env.prompt_queue().peek_all(10).expect("peek").is_empty(),
        "failed sends must not leave a queued row"
    );
}

/// cas-5068 / GH #335: a worker may warn a same-session peer about a live
/// collision, but CAS persists an inseparable supervisor-visible copy rather
/// than opening an unobserved worker chat channel.
#[tokio::test]
async fn cas_5068_same_session_peer_warning_reaches_peer_and_supervisor_copy() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_AGENT_NAME", "worker-a"),
        ("CAS_FACTORY_SESSION", "collision-session"),
        ("CAS_SUPERVISOR_NAME", "supervisor-a"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_worker_with_id("test-agent-id", "worker-a", Some("collision-session"));
    env.register_worker_in_session("worker-b", "collision-session");
    let supervisor_id = env.register_supervisor_in_session("supervisor-a", "collision-session");

    let result = env
        .service
        .coordination(Parameters(coord_msg(
            "message",
            "worker-b",
            "Stop: I own customer thread #42; do not send a second draft.",
            None,
        )))
        .await
        .expect("same-session peer warning must queue");
    let text = get_text(&result);
    assert!(text.contains("To: worker-b"), "{text}");
    assert!(text.contains("supervisor_copy_notification_id:"), "{text}");

    let rows = env.prompt_queue().peek_all(10).expect("queued rows");
    assert_eq!(rows.len(), 2, "peer warning and supervisor copy: {rows:?}");
    assert!(
        rows.iter()
            .any(|row| row.target == "worker-b" && !row.urgent)
    );
    assert!(rows.iter().any(|row| {
        row.target == "supervisor-a"
            && !row.urgent
            && row
                .prompt
                .contains("Peer worker message copy — from worker-a to worker-b")
            && row.prompt.contains("customer thread #42")
    }));

    let supervisor_core = CasCore::with_daemon(env.cas_root.clone(), None, None);
    supervisor_core.set_agent_id_for_testing(supervisor_id);
    let supervisor_service = CasService::new(supervisor_core, None);
    let supervisor_poll = supervisor_service
        .coordination(Parameters(coord_req("inbox_poll")))
        .await
        .expect("supervisor must retain peer-message visibility");
    let supervisor_text = get_text(&supervisor_poll);
    assert!(
        supervisor_text.contains("Peer worker message copy — from worker-a to worker-b"),
        "supervisor copy must be readable through the public inbox: {supervisor_text}"
    );
}

#[tokio::test]
async fn cas_5068_peer_warning_refuses_cross_session_target_without_enqueue() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_AGENT_NAME", "worker-a"),
        ("CAS_FACTORY_SESSION", "session-a"),
        ("CAS_SUPERVISOR_NAME", "supervisor-a"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_worker_with_id("test-agent-id", "worker-a", Some("session-a"));
    env.register_supervisor_in_session("supervisor-a", "session-a");
    env.register_worker_in_session("worker-b", "session-b");

    let error = env
        .service
        .coordination(Parameters(coord_msg(
            "message",
            "worker-b",
            "do not queue across sessions",
            None,
        )))
        .await
        .expect_err("cross-session worker route must be refused");
    assert!(
        error
            .message
            .contains("registered worker in factory session 'session-a'")
            && error.message.contains("target='supervisor'"),
        "refusal must name the bounded scope and supported alternative: {error:?}"
    );
    assert!(env.prompt_queue().peek_all(10).expect("peek").is_empty());
}

#[tokio::test]
async fn cas_5068_peer_warning_is_rate_limited_per_recipient() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_AGENT_NAME", "worker-a"),
        ("CAS_FACTORY_SESSION", "collision-session"),
        ("CAS_SUPERVISOR_NAME", "supervisor-a"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_worker_with_id("test-agent-id", "worker-a", Some("collision-session"));
    env.register_worker_in_session("worker-b", "collision-session");
    env.register_supervisor_in_session("supervisor-a", "collision-session");

    for warning in 0..cas_store::WORKER_PEER_MESSAGE_BURST_LIMIT {
        env.service
            .coordination(Parameters(coord_msg(
                "message",
                "worker-b",
                &format!("collision warning #{warning}"),
                None,
            )))
            .await
            .expect("warning inside peer burst limit");
    }
    let error = env
        .service
        .coordination(Parameters(coord_msg(
            "message",
            "worker-b",
            "one warning too many",
            None,
        )))
        .await
        .expect_err("sixth one-minute peer warning must be rate limited");
    assert!(error.message.contains("rate limit"), "{error:?}");
    assert_eq!(
        env.prompt_queue().peek_all(20).expect("queued rows").len(),
        (cas_store::WORKER_PEER_MESSAGE_BURST_LIMIT * 2) as usize,
        "each allowed warning must retain its supervisor copy"
    );
}

/// cas-c061: exact-content dedup is an observable send outcome. Reusing the
/// existing row ID must not be reported as a newly queued message.
#[tokio::test]
async fn test_worker_duplicate_send_reports_suppression() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_AGENT_NAME", "swift-fox"),
        ("CAS_SUPERVISOR_NAME", "cosmic-bear-43"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_supervisor("cosmic-bear-43");

    let first = coord_msg("message", "supervisor", "same completion report", None);
    let first_result = env
        .service
        .coordination(Parameters(first))
        .await
        .expect("first send");
    let first_text = get_text(&first_result);
    let first_id = first_text
        .lines()
        .find_map(|line| line.strip_prefix("notification_id: "))
        .expect("first response notification id")
        .parse::<i64>()
        .expect("numeric notification id");
    env.prompt_queue()
        .mark_transport_delivered(first_id)
        .expect("deliver first report");

    let duplicate = coord_msg("message", "supervisor", "same completion report", None);
    let duplicate_result = env
        .service
        .coordination(Parameters(duplicate))
        .await
        .expect("duplicate send");
    let duplicate_text = get_text(&duplicate_result);

    assert!(
        duplicate_text.to_lowercase().contains("suppressed"),
        "dedup must be visible instead of claiming a fresh enqueue: {duplicate_text}"
    );
    assert!(
        !duplicate_text.starts_with("Message queued"),
        "a suppressed duplicate must not impersonate a fresh queue insert: {duplicate_text}"
    );
}

/// cas-c061: responding to a peer proves consumption only of messages from
/// that peer. A display-name route must not broaden the counterparty to the
/// logical `supervisor` alias and confirm unrelated supervisor instructions.
#[tokio::test]
async fn test_worker_peer_message_does_not_confirm_supervisor_instruction() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_AGENT_NAME", "swift-fox"),
        ("CAS_SUPERVISOR_NAME", "supervisor"),
        ("CAS_FACTORY_SESSION", "peer-message-session"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_worker_with_id("test-agent-id", "swift-fox", Some("peer-message-session"));
    env.register_worker_in_session("peer-worker", "peer-message-session");
    env.register_supervisor_in_session("supervisor", "peer-message-session");
    let instruction = env
        .prompt_queue()
        .enqueue_urgent(
            "supervisor",
            "swift-fox",
            "unread supervisor instruction",
            None,
            Some("assignment"),
            None,
            false,
        )
        .expect("enqueue supervisor instruction");
    env.prompt_queue()
        .mark_transport_delivered(instruction)
        .expect("deliver supervisor instruction");

    let peer_message = coord_msg("message", "peer-worker", "peer status", None);
    env.service
        .coordination(Parameters(peer_message))
        .await
        .expect("peer display-name route should send");

    let report = env
        .prompt_queue()
        .message_delivery_report(instruction)
        .expect("delivery report")
        .expect("instruction exists");
    assert_eq!(
        report.stage,
        cas_store::DeliveryStage::Delivered,
        "peer messaging must not confirm an unrelated supervisor instruction"
    );
}

/// cas-0440: the send response must name the same parameter that
/// `message_status` accepts. Drive the caller-visible two-call sequence:
/// send a message, copy the returned notification_id verbatim, then query it.
#[tokio::test]
async fn test_coordination_message_returned_notification_id_drives_status_query() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "supervisor"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_worker("swift-fox");

    let send_req = coord_msg("message", "swift-fox", "status update", None);
    let send_result = env.service.coordination(Parameters(send_req)).await;
    assert!(
        send_result.is_ok(),
        "message should succeed: {send_result:?}"
    );
    let send_text = get_text(&send_result.unwrap());

    let notification_id = send_text
        .lines()
        .find_map(|line| line.strip_prefix("notification_id: "))
        .expect("message response must label the returned prompt queue ID as notification_id")
        .parse::<i64>()
        .expect("returned notification_id must be an integer");
    assert!(
        send_text.contains(&format!(
            "`message_status` with `notification_id={notification_id}`"
        )),
        "response must give the exact parameter spelling for the follow-up call: {send_text}"
    );

    let mut status_req = coord_msg("message_status", "swift-fox", "unused", None);
    status_req.notification_id = Some(notification_id);
    let status_result = env.service.coordination(Parameters(status_req)).await;
    assert!(
        status_result.is_ok(),
        "message_status must accept the ID copied from the send response: {status_result:?}"
    );
    let status_text = get_text(&status_result.unwrap());
    assert!(
        status_text.contains(&format!("Message {notification_id} status:")),
        "status response must describe the same returned ID: {status_text}"
    );
}

/// cas-893c AC2: `message_status` must expose how long a message has been
/// undelivered rather than only a bare stage string. A freshly enqueued,
/// never-acked message must report a non-negative `undelivered_after_secs`
/// (both in the JSON payload and the human-readable line), and the JSON
/// must be well-formed (parseable) so scripted callers can consume it.
#[tokio::test]
async fn test_message_status_exposes_undelivered_after() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "supervisor"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_worker("swift-fox");

    let send_req = coord_msg("message", "swift-fox", "status update", None);
    let send_result = env.service.coordination(Parameters(send_req)).await;
    assert!(
        send_result.is_ok(),
        "message should succeed: {send_result:?}"
    );

    let prompts = env.prompt_queue().peek_all(10).expect("peek");
    assert_eq!(prompts.len(), 1);
    let message_id = prompts[0].id;

    let mut status_req = coord_msg("message_status", "swift-fox", "unused", None);
    status_req.notification_id = Some(message_id);
    let status_result = env.service.coordination(Parameters(status_req)).await;
    assert!(
        status_result.is_ok(),
        "message_status should succeed: {status_result:?}"
    );
    let text = get_text(&status_result.unwrap());

    assert!(
        text.contains("undelivered_after:"),
        "human-readable line must expose undelivered_after: {text}"
    );

    // The JSON body is everything after the human-readable header lines —
    // find the first `{` and parse from there.
    let json_start = text.find('{').expect("response must contain a JSON body");
    let json: serde_json::Value =
        serde_json::from_str(&text[json_start..]).expect("undelivered status JSON must parse");
    let undelivered_after_secs = json
        .get("undelivered_after_secs")
        .expect("JSON must carry undelivered_after_secs");
    assert!(
        undelivered_after_secs.is_number(),
        "a never-acked message must report a numeric undelivered_after_secs, got: {json}"
    );
    assert!(
        undelivered_after_secs.as_i64().unwrap() >= 0,
        "undelivered_after_secs must be non-negative: {json}"
    );
}

/// cas-73c8 AC3: `director` is a permanent team member and the source of
/// inbound teammate messages, but is not an agent_store registration.
/// Outbound `target=director` must report registered (symmetric with inbound).
#[tokio::test]
async fn test_coordination_message_to_director_reports_registered() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "supervisor"),
    ]);
    let env = FactoryTestEnv::new();
    // Deliberately do NOT register "director" in agent_store.

    let req = coord_msg("message", "director", "ack director nudge", None);
    let result = env.service.coordination(Parameters(req)).await;
    assert!(
        result.is_ok(),
        "message to director should succeed: {result:?}"
    );
    let text = get_text(&result.unwrap());
    assert!(
        text.contains("target is registered"),
        "director must resolve as registered: {text}"
    );
    assert!(
        !text.contains("not yet registered"),
        "director must not read as unregistered after inbound teammate traffic: {text}"
    );
}

/// cas-6913 AC2: the defect this task exists to fix — "Message queued" reads
/// as delivery confirmation even when the target name isn't in the agent
/// store yet (the common spawn-then-immediately-assign race). The ack must
/// say so honestly instead of implying success either way.
#[tokio::test]
async fn test_coordination_message_to_unregistered_target_reports_queued_pending_registration() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "supervisor"),
    ]);
    let env = FactoryTestEnv::new();
    // Deliberately do NOT register "not-born-yet" — simulates a message
    // addressed to a worker name the supervisor already knows (e.g. from an
    // explicit spawn_workers worker_names= request) before the daemon has
    // finished spawning it.

    let req = coord_msg("message", "not-born-yet", "start with task cas-abc1", None);
    let result = env.service.coordination(Parameters(req)).await;
    assert!(result.is_ok(), "message should still enqueue: {result:?}");
    let text = get_text(&result.unwrap());
    assert!(
        text.contains("not yet registered"),
        "response must honestly flag the target as not yet registered: {text}"
    );
    assert!(
        !text.contains("target is registered"),
        "an unregistered target must not read as registered: {text}"
    );

    // The message still lands in the queue — this is about honest
    // reporting, not blocking the send. cas-7e20/daemon polling handles
    // eventual delivery once the name is registered.
    let prompts = env.prompt_queue().peek_all(10).expect("peek");
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].target, "not-born-yet");
}

/// cas-6ad2: "queue-before-register -> register consumes it exactly once".
/// A message queued to a worker name before that worker exists in the
/// agent store must be delivered into the worker's OWN prompt loop at
/// registration time (surfaced directly in the register response text —
/// no PTY-injection timing dependency). Once the registration response has
/// carried that message into the recipient's context, the queue row must be
/// terminally confirmed instead of remaining eligible for daemon redelivery.
#[tokio::test]
async fn test_agent_register_surfaces_pending_prompt_queue_mail() {
    let env = FactoryTestEnv::new();

    // Step 1: queue-before-register.
    let message_id = env
        .prompt_queue()
        .enqueue_urgent(
            "supervisor",
            "not-born-yet",
            "start with task cas-abc1",
            None,
            Some("assignment"),
            None,
            false,
        )
        .expect("enqueue pre-registration message");

    // Step 2: register.
    let mut req = coord_req("register");
    req.name = Some("not-born-yet".to_string());
    req.session_id = Some("session-not-born-yet".to_string());
    let result = env.service.coordination(Parameters(req)).await;
    assert!(result.is_ok(), "register should succeed: {result:?}");
    let text = get_text(&result.unwrap());
    assert!(
        text.contains("start with task cas-abc1"),
        "registration response should surface the pending message: {text}"
    );
    assert!(
        text.contains("waiting for you"),
        "response should explain why the message appears: {text}"
    );

    // Step 3: the registration response is the delivery. The daemon must not
    // inject the same message as another fresh turn afterward.
    let still_pending = env
        .prompt_queue()
        .poll_for_target("not-born-yet", 10)
        .expect("poll");
    assert!(
        still_pending.is_empty(),
        "message consumed by registration must not remain pollable: {still_pending:?}"
    );
    let report = env
        .prompt_queue()
        .message_delivery_report(message_id)
        .expect("delivery report")
        .expect("message exists");
    assert_eq!(
        report.stage,
        cas_store::DeliveryStage::Confirmed,
        "recipient consumption must advance message_status to confirmed"
    );
}

/// Codex workers register via `action=session_start`, not `action=register`
/// (see cas-e7c8 / the ToolSearch two-step guidance) — this is the literal
/// path the source bug doc's repro hit ("Worker zealous-hawk-40 (codex
/// CLI)"). Must get the same treatment as the Claude `register` path.
#[tokio::test]
async fn test_agent_session_start_surfaces_pending_prompt_queue_mail() {
    let env = FactoryTestEnv::new();

    env.prompt_queue()
        .enqueue_urgent(
            "supervisor",
            "codex-worker-1",
            "branch base: epic/foo. proof command: cargo test.",
            None,
            Some("assignment"),
            None,
            false,
        )
        .expect("enqueue pre-registration message");

    let mut req = coord_req("session_start");
    req.name = Some("codex-worker-1".to_string());
    req.session_id = Some("session-codex-worker-1".to_string());
    let result = env.service.coordination(Parameters(req)).await;
    assert!(result.is_ok(), "session_start should succeed: {result:?}");
    let text = get_text(&result.unwrap());
    assert!(
        text.contains("branch base: epic/foo"),
        "session_start response should surface the pending message: {text}"
    );
}

/// No pending mail must add no noise — registration stays a clean,
/// unchanged response for the overwhelmingly common case.
#[tokio::test]
async fn test_agent_register_with_no_pending_mail_stays_unchanged() {
    let env = FactoryTestEnv::new();

    let mut req = coord_req("register");
    req.name = Some("fresh-worker".to_string());
    req.session_id = Some("session-fresh-worker".to_string());
    let result = env.service.coordination(Parameters(req)).await;
    assert!(result.is_ok(), "register should succeed: {result:?}");
    let text = get_text(&result.unwrap());
    assert!(
        !text.contains("waiting for you"),
        "no pending mail should mean no pending-mail section: {text}"
    );
}

/// A message queued for a DIFFERENT worker must never leak into this
/// worker's registration response.
#[tokio::test]
async fn test_agent_register_does_not_leak_other_agents_mail() {
    let env = FactoryTestEnv::new();

    env.prompt_queue()
        .enqueue_urgent(
            "supervisor",
            "someone-else",
            "top secret instructions for someone-else",
            None,
            Some("assignment"),
            None,
            false,
        )
        .expect("enqueue message for a different worker");

    let mut req = coord_req("register");
    req.name = Some("fresh-worker".to_string());
    req.session_id = Some("session-fresh-worker".to_string());
    let result = env.service.coordination(Parameters(req)).await;
    assert!(result.is_ok(), "register should succeed: {result:?}");
    let text = get_text(&result.unwrap());
    assert!(
        !text.contains("top secret instructions"),
        "another agent's queued message must not leak into this registration: {text}"
    );
}

/// `action=interrupt` is sugar for `message` with urgent=true.
#[tokio::test(start_paused = true)]
async fn test_coordination_interrupt_action_is_urgent() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "supervisor"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_worker("swift-fox");

    // urgent intentionally None — the action alone must force urgent.
    let req = coord_msg(
        "interrupt",
        "swift-fox",
        "abort and re-read the ticket",
        None,
    );
    let result = env.service.coordination(Parameters(req)).await;
    let error = result.expect_err("unobserved Claude interrupt action must fail explicitly");
    assert!(
        error
            .message
            .contains("Could not confirm Claude interrupt delivery"),
        "unexpected interrupt error: {}",
        error.message
    );

    let prompts = env.prompt_queue().peek_all(10).expect("peek");
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].urgent,
        "action=interrupt must enqueue urgent even when the urgent flag is omitted"
    );
}

// =============================================================================
// cas-efc4: Heterogeneous Claude+Codex smoke regression tests
//
// Covers the surfaces landed by cas-8aaf / cas-a3ca / cas-4491 / cas-dbbb:
// heterogeneous spawn config (AC1+AC2), model/effort spec propagation (AC2),
// and worker_status metadata for both harness types (AC4).
// The prompt-layer heterogeneous tests (AC3, AC5) live in director/prompts.rs.
// =============================================================================

/// cas-efc4 AC1+AC2: Spawning a Codex worker followed by a Claude worker in the
/// same supervisor session must queue two distinct SpawnRequests.  The Codex
/// entry must carry a worker_spec that encodes the harness; the default-Claude
/// entry must have no spec (session defaults apply).
///
/// This pins the spawn-queue contract for heterogeneous sessions so a
/// regression in `build_spawn_spec_json` or the `spawn_workers` handler is
/// caught at test time, not at factory-start time.
#[test]
fn test_efc4_heterogeneous_codex_then_claude_spawn_queued_correctly() {
    run_isolated_codex_test(
        "test_efc4_heterogeneous_codex_then_claude_spawn_queued_correctly_in_isolated_child",
        IsolatedCodexState::Available,
    );
}

#[tokio::test]
#[ignore = "subprocess helper for deterministic available-Codex probe"]
async fn test_efc4_heterogeneous_codex_then_claude_spawn_queued_correctly_in_isolated_child() {
    let env = factory_env_in_isolated_codex_child(
        "test_efc4_heterogeneous_codex_then_claude_spawn_queued_correctly_in_isolated_child",
    );
    env.create_epic("Heterogeneous Smoke Epic");

    // --- Codex worker with model + effort overrides ---
    let mut codex_req = factory_req("spawn_workers");
    codex_req.count = Some(1);
    codex_req.cli = Some("codex".to_string());
    codex_req.model = Some("o3".to_string());
    codex_req.effort = Some("high".to_string());
    codex_req.worker_names = Some("codex-alpha".to_string());
    env.service
        .factory(Parameters(codex_req))
        .await
        .expect("codex spawn should succeed");

    // --- Claude worker with no overrides (session defaults) ---
    let mut claude_req = factory_req("spawn_workers");
    claude_req.count = Some(1);
    claude_req.worker_names = Some("claude-beta".to_string());
    env.service
        .factory(Parameters(claude_req))
        .await
        .expect("claude spawn should succeed");

    let entries = env.spawn_queue().peek(10).expect("peek spawn queue");
    assert_eq!(
        entries.len(),
        2,
        "should have exactly 2 spawn queue entries (one Codex, one Claude)"
    );

    // First entry: Codex with spec
    let codex_entry = &entries[0];
    let spec_json = codex_entry
        .worker_spec
        .as_deref()
        .expect("cas-efc4 AC2: Codex spawn entry must carry a worker_spec");
    assert!(
        spec_json.contains("codex"),
        "cas-efc4 AC1: worker_spec must encode the Codex harness: {spec_json}"
    );
    assert!(
        spec_json.contains("o3"),
        "cas-efc4 AC2: worker_spec must encode the model override: {spec_json}"
    );
    assert!(
        spec_json.contains("high"),
        "cas-efc4 AC2: worker_spec must encode the effort override: {spec_json}"
    );

    // Second entry: omitted overrides now resolves to the safe Codex worker floor
    // instead of inheriting the supervisor/session defaults.
    let claude_entry = &entries[1];
    let spec_json = claude_entry
        .worker_spec
        .as_deref()
        .expect("cas-23dc: omitted overrides must still queue a resolved worker_spec");
    let spec: cas_mux::WorkerSpec = serde_json::from_str(spec_json).expect("valid WorkerSpec");
    assert_eq!(spec.cli, cas_mux::SupervisorCli::Codex);
    assert_eq!(spec.model.as_deref(), Some(cas::config::STOCK_WORKER_MODEL));
    assert_eq!(spec.effort, Some(cas_mux::Effort::XHigh));
}

/// cas-efc4 AC2: Model and effort overrides must reach the spawn-queue spec
/// for both Codex and Claude harnesses.  Tests the cross-product so that a
/// future change to `build_spawn_spec_json` for one harness doesn't silently
/// break the other.
#[test]
fn test_efc4_model_and_effort_reach_spawn_spec_for_each_harness() {
    run_isolated_codex_test(
        "test_efc4_model_and_effort_reach_spawn_spec_for_each_harness_in_isolated_child",
        IsolatedCodexState::Available,
    );
}

#[tokio::test]
#[ignore = "subprocess helper for deterministic available-Codex probe"]
async fn test_efc4_model_and_effort_reach_spawn_spec_for_each_harness_in_isolated_child() {
    let env = factory_env_in_isolated_codex_child(
        "test_efc4_model_and_effort_reach_spawn_spec_for_each_harness_in_isolated_child",
    );
    env.create_epic("Spec Propagation Epic");

    // Codex with model+effort
    let mut codex_req = factory_req("spawn_workers");
    codex_req.count = Some(1);
    codex_req.cli = Some("codex".to_string());
    codex_req.model = Some("o4-mini".to_string());
    codex_req.effort = Some("xhigh".to_string());
    env.service
        .factory(Parameters(codex_req))
        .await
        .expect("codex+model+effort spawn should succeed");

    // Claude with model+effort
    let mut claude_req = factory_req("spawn_workers");
    claude_req.count = Some(1);
    claude_req.cli = Some("claude".to_string());
    claude_req.model = Some("claude-opus-4-5".to_string());
    claude_req.effort = Some("medium".to_string());
    env.service
        .factory(Parameters(claude_req))
        .await
        .expect("claude+model+effort spawn should succeed");

    let entries = env.spawn_queue().peek(10).expect("peek");
    assert_eq!(entries.len(), 2, "expected 2 spec-carrying entries");

    let codex_spec = entries[0]
        .worker_spec
        .as_deref()
        .expect("codex entry must have spec");
    assert!(
        codex_spec.contains("codex"),
        "codex harness in spec: {codex_spec}"
    );
    assert!(
        codex_spec.contains("o4-mini"),
        "codex model in spec: {codex_spec}"
    );
    assert!(
        codex_spec.contains("xhigh"),
        "codex effort in spec: {codex_spec}"
    );

    let claude_spec = entries[1]
        .worker_spec
        .as_deref()
        .expect("claude entry must have spec when cli given");
    assert!(
        claude_spec.contains("claude"),
        "claude harness in spec: {claude_spec}"
    );
    assert!(
        claude_spec.contains("claude-opus-4-5"),
        "claude model in spec: {claude_spec}"
    );
    assert!(
        claude_spec.contains("medium"),
        "claude effort in spec: {claude_spec}"
    );
}

/// cas-efc4 AC4: `worker_status` must surface worktree/git metadata
/// (`clone_path`) for workers of **both** harness types registered in the same
/// session.  Exercises the cas-4491 rendering path across harnesses so that a
/// regression only affecting one type is caught here.
#[tokio::test]
async fn test_efc4_worker_status_shows_clone_path_for_both_harnesses() {
    // Acquire env mutex — prevents concurrent tests that set CAS_AGENT_ROLE
    // from activating supervisor scoping and filtering our test workers out.
    let _guard = EnvGuard::set(&[]);
    let env = FactoryTestEnv::new();

    // Claude worker with clone_path metadata
    let mut claude_meta = HashMap::new();
    claude_meta.insert(
        "clone_path".to_string(),
        "/tmp/cas-worktrees/claude-worker".to_string(),
    );
    env.register_worker_with_metadata("claude-worker", claude_meta);

    // Codex worker with clone_path metadata
    let mut codex_meta = HashMap::new();
    codex_meta.insert(
        "clone_path".to_string(),
        "/tmp/cas-worktrees/codex-worker".to_string(),
    );
    env.register_worker_with_metadata("codex-worker", codex_meta);

    let req = factory_req("worker_status");
    let result = env
        .service
        .factory(Parameters(req))
        .await
        .expect("worker_status should succeed");
    let text = get_text(&result);

    assert!(
        text.contains("Workers (2)"),
        "cas-efc4 AC4: should report 2 active workers: {text}"
    );
    assert!(
        text.contains("claude-worker"),
        "cas-efc4 AC4: Claude worker must appear in status: {text}"
    );
    assert!(
        text.contains("codex-worker"),
        "cas-efc4 AC4: Codex worker must appear in status: {text}"
    );
    assert!(
        text.contains("/tmp/cas-worktrees/claude-worker"),
        "cas-efc4 AC4: Claude worker clone_path must be rendered: {text}"
    );
    assert!(
        text.contains("/tmp/cas-worktrees/codex-worker"),
        "cas-efc4 AC4: Codex worker clone_path must be rendered: {text}"
    );
}

// =============================================================================
// cas-062d: task lifecycle → owning supervisor push
// =============================================================================

/// Happy path + isolation: start + blocked create durable `task_lifecycle`
/// events only for the owning factory session's supervisor. Replay of the
/// same transition identity does not create a second row.
#[tokio::test]
async fn test_062d_lifecycle_start_and_blocked_push_session_isolated() {
    let _guard = EnvGuard::set(&[("CAS_FACTORY_SESSION", "sess-062d-a")]);
    let env = FactoryTestEnv::with_agent_id("worker-062d");

    // Owning supervisor (session A) vs foreign supervisor (session B).
    let sup_a = env.register_supervisor_in_session("sup-a", "sess-062d-a");
    let sup_b = env.register_supervisor_in_session("sup-b", "sess-062d-b");

    // Register the starting agent so assignee/actor resolve cleanly.
    {
        let store = env.agent_store();
        let mut worker = Agent::new("worker-062d".to_string(), "worker-062d".to_string());
        worker.role = AgentRole::Worker;
        worker.factory_session = Some("sess-062d-a".to_string());
        store.register(&worker).expect("register worker");
    }

    let task_store = env.task_store();
    let mut task = Task::new(
        "cas-062d-start".to_string(),
        "Lifecycle start push".to_string(),
    );
    task.status = TaskStatus::Open;
    task_store.add(&task).expect("add task");

    let start = env
        .service
        .inner
        .cas_task_start(Parameters(cas::mcp::tools::IdRequest {
            id: "cas-062d-start".to_string(),
        }))
        .await
        .expect("start should succeed");
    let start_text = get_text(&start);
    assert!(
        start_text.contains("Started task") || start_text.contains("cas-062d-start"),
        "start response: {start_text}"
    );

    let queue = cas::store::open_supervisor_queue_store(&env.cas_root).expect("queue");
    let pending_a = queue.peek(&sup_a, 20).expect("peek a");
    assert!(
        pending_a.iter().any(|n| {
            n.event_type == "task_lifecycle"
                && n.payload.contains("task_started")
                && n.payload.contains("cas-062d-start")
        }),
        "session-A supervisor must receive task_started. pending={pending_a:?}"
    );
    let pending_b = queue.peek(&sup_b, 20).expect("peek b");
    assert!(
        !pending_b
            .iter()
            .any(|n| n.payload.contains("cas-062d-start")),
        "session-B supervisor must NOT receive session-A lifecycle events. pending={pending_b:?}"
    );
    assert_eq!(
        queue.pending_count(&sup_a).unwrap(),
        1,
        "exactly one start event before blocked"
    );

    // Blocked transition via task update.
    let update = env
        .service
        .inner
        .cas_task_update(Parameters(cas::mcp::tools::TaskUpdateRequest {
            blocked_by: None,
            id: "cas-062d-start".to_string(),
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
            status: Some("blocked".to_string()),
            epic: None,
            origin_project: None,
            epic_verification_owner: None,
            depth: None,
        }))
        .await
        .expect("update blocked");
    let _ = get_text(&update);

    let pending_a2 = queue.peek(&sup_a, 20).expect("peek a after blocked");
    assert!(
        pending_a2.iter().any(|n| {
            n.event_type == "task_lifecycle"
                && n.payload.contains("task_blocked")
                && n.payload.contains("cas-062d-start")
        }),
        "blocked transition must enqueue task_blocked. pending={pending_a2:?}"
    );
    assert_eq!(
        queue.pending_count(&sup_a).unwrap(),
        2,
        "start + blocked = 2 durable events"
    );
    assert_eq!(
        queue.pending_count(&sup_b).unwrap(),
        0,
        "foreign session still empty"
    );
}

/// Reopen (supervisor) emits ReadyReopened / task_ready for the owning session.
#[tokio::test]
async fn test_062d_lifecycle_reopen_pushes_ready() {
    let _guard = EnvGuard::set(&[
        ("CAS_FACTORY_SESSION", "sess-062d-reopen"),
        ("CAS_AGENT_ROLE", "supervisor"),
    ]);
    let env = FactoryTestEnv::with_agent_id("sup-reopen-agent");

    let sup_id = env.register_supervisor_in_session("sup-reopen", "sess-062d-reopen");
    {
        let store = env.agent_store();
        let mut sup = Agent::new("sup-reopen-agent".to_string(), "sup-reopen".to_string());
        sup.role = AgentRole::Supervisor;
        sup.factory_session = Some("sess-062d-reopen".to_string());
        store.register(&sup).expect("register calling supervisor");
    }

    let task_store = env.task_store();
    let mut task = Task::new(
        "cas-062d-reopen".to_string(),
        "Reopen lifecycle".to_string(),
    );
    task.status = TaskStatus::Closed;
    task.closed_at = Some(chrono::Utc::now());
    task_store.add(&task).expect("add closed task");

    let result = env
        .service
        .inner
        .cas_task_reopen(Parameters(cas::mcp::tools::TaskReopenRequest {
            id: "cas-062d-reopen".to_string(),
            reason: Some("new ready cycle after supervisor review".to_string()),
        }))
        .await
        .expect("reopen should succeed");
    let text = get_text(&result);
    assert!(
        text.contains("Reopened") || text.contains("cas-062d-reopen"),
        "reopen response: {text}"
    );

    let after = task_store.get("cas-062d-reopen").expect("task");
    assert_eq!(after.status, TaskStatus::Open);

    let queue = cas::store::open_supervisor_queue_store(&env.cas_root).expect("queue");
    // cas-7787 (GH #160) changed `resolve_owning_supervisor` to break ties on
    // registration recency after liveness, so that a supervisor whose harness
    // session restarts mid-factory-session keeps receiving relays at its
    // SUCCESSOR identity instead of at whichever duplicate row happened to
    // sort first on a UUID. This test registers two Supervisor rows in one
    // session — `sup_id` first, then the calling agent below — so the relay
    // now lands at the caller, which is what the fixture always meant by
    // "register calling supervisor". Assert against the caller, not against
    // the row the old lexicographic coin flip used to pick.
    assert!(
        queue.peek(&sup_id, 20).expect("peek").is_empty(),
        "the superseded identity must not capture the relay"
    );
    let pending = queue.peek("sup-reopen-agent", 20).expect("peek");
    assert!(
        pending.iter().any(|n| {
            n.event_type == "task_lifecycle"
                && n.payload.contains("task_ready")
                && n.payload.contains("cas-062d-reopen")
        }),
        "reopen must push task_ready. pending={pending:?}"
    );
}

/// Supervisor close of an orphaned task pushes task_closed (Closed transition).
#[tokio::test]
async fn test_062d_lifecycle_close_pushes_closed() {
    let _guard = EnvGuard::set(&[
        ("CAS_FACTORY_SESSION", "sess-062d-close"),
        ("CAS_AGENT_ROLE", "supervisor"),
    ]);
    let env = FactoryTestEnv::with_agent_id("sup-close-agent");

    let sup_id = env.register_supervisor_in_session("sup-close", "sess-062d-close");
    {
        let store = env.agent_store();
        let mut sup = Agent::new("sup-close-agent".to_string(), "sup-close".to_string());
        sup.role = AgentRole::Supervisor;
        sup.factory_session = Some("sess-062d-close".to_string());
        store.register(&sup).expect("register calling supervisor");
    }

    let task_store = env.task_store();
    let mut task = Task::new("cas-062d-close".to_string(), "Close lifecycle".to_string());
    // Orphaned InProgress → supervisor bypass skips verification.
    task.status = TaskStatus::InProgress;
    task.assignee = None;
    task_store.add(&task).expect("add task");

    let result = env
        .service
        .inner
        .cas_task_close(Parameters(cas::mcp::tools::TaskCloseRequest {
            stranded_branch_override: None,
            id: "cas-062d-close".to_string(),
            reason: Some("lifecycle close proof".to_string()),
            supervisor_override: Some(true),
            legacy_bypass_code_review: None,
            search_manifest: None,
            commit_receipt: None,
        }))
        .await
        .expect("close should succeed");
    let text = get_text(&result);
    assert!(
        text.contains("Closed") || text.contains("cas-062d-close"),
        "close response: {text}"
    );

    let after = task_store.get("cas-062d-close").expect("task");
    assert_eq!(after.status, TaskStatus::Closed, "notes={}", after.notes);

    let queue = cas::store::open_supervisor_queue_store(&env.cas_root).expect("queue");
    // cas-7787 (GH #160) changed `resolve_owning_supervisor` to break ties on
    // registration recency after liveness, so that a supervisor whose harness
    // session restarts mid-factory-session keeps receiving relays at its
    // SUCCESSOR identity instead of at whichever duplicate row happened to
    // sort first on a UUID. This test registers two Supervisor rows in one
    // session — `sup_id` first, then the calling agent below — so the relay
    // now lands at the caller, which is what the fixture always meant by
    // "register calling supervisor". Assert against the caller, not against
    // the row the old lexicographic coin flip used to pick.
    assert!(
        queue.peek(&sup_id, 20).expect("peek").is_empty(),
        "the superseded identity must not capture the relay"
    );
    let pending = queue.peek("sup-close-agent", 20).expect("peek");
    assert!(
        pending.iter().any(|n| {
            n.event_type == "task_lifecycle"
                && n.payload.contains("task_closed")
                && n.payload.contains("cas-062d-close")
        }),
        "close must push task_closed. pending={pending:?}"
    );
}

// =============================================================================
// cas-a844: awaiting_merge is a dead end when the merge cannot succeed
// =============================================================================

/// AC1: a worker CAN start a conflicted `awaiting_merge` task — it transitions
/// back to `in_progress`, records the rework decision, and invalidates all
/// close-cycle merge state so the eventual re-close evaluates fresh work.
#[tokio::test]
async fn test_a844_worker_can_start_awaiting_merge_task() {
    let _guard = EnvGuard::set_optional(&[
        ("CAS_AGENT_ROLE", Some("worker")),
        ("CAS_FACTORY_MODE", Some("1")),
    ]);
    let env = FactoryTestEnv::with_agent_id("swift-fox");
    {
        let store = env.agent_store();
        let mut worker = Agent::new("swift-fox".to_string(), "swift-fox".to_string());
        worker.role = AgentRole::Worker;
        store.register(&worker).expect("register worker");
    }
    let task_id = env.create_awaiting_merge_task("parked work", "swift-fox");
    {
        let store = env.task_store();
        let mut task = store.get(&task_id).expect("task");
        task.deliverables.merge_conflicted = true;
        task.deliverables.factory_branch_anchor = Some("parked-anchor".to_string());
        task.deliverables.parked_branch = Some("factory/swift-fox".to_string());
        store.update(&task).expect("mark task conflicted");
    }

    let result = env
        .service
        .inner
        .cas_task_start(Parameters(cas::mcp::tools::IdRequest {
            id: task_id.clone(),
        }))
        .await;
    assert!(
        result.is_ok(),
        "starting an awaiting_merge task must now succeed: {result:?}"
    );

    let after = env.task_store().get(&task_id).expect("task");
    assert_eq!(
        after.status,
        TaskStatus::InProgress,
        "awaiting_merge -> start must transition to in_progress, not stay parked"
    );
    assert!(
        after.notes.to_lowercase().contains("merge conflict"),
        "resume decision note should name the merge conflict: {}",
        after.notes
    );
    assert!(
        after.deliverables.factory_branch_anchor.is_none(),
        "conflict rework must invalidate the parked anchor"
    );
    assert!(
        after.deliverables.parked_branch.is_none(),
        "conflict rework must clear the parked branch receipt"
    );
    assert!(
        !after.deliverables.merge_conflicted,
        "conflict rework must clear the prior close cycle's conflict flag"
    );
}

/// AC1 negative control: a cleanly mergeable `awaiting_merge` task is still
/// complete from the worker's perspective. Starting it must keep steering the
/// worker to wait for the supervisor merge while naming the conflict escape.
#[tokio::test]
async fn test_5054_clean_awaiting_merge_still_refuses_start() {
    let _guard = EnvGuard::set_optional(&[
        ("CAS_AGENT_ROLE", Some("worker")),
        ("CAS_FACTORY_MODE", Some("1")),
    ]);
    let env = FactoryTestEnv::with_agent_id("swift-fox");
    {
        let store = env.agent_store();
        let mut worker = Agent::new("swift-fox".to_string(), "swift-fox".to_string());
        worker.role = AgentRole::Worker;
        store.register(&worker).expect("register worker");
    }
    let task_id = env.create_awaiting_merge_task("clean parked work", "swift-fox");

    let result = env
        .service
        .inner
        .cas_task_start(Parameters(cas::mcp::tools::IdRequest {
            id: task_id.clone(),
        }))
        .await
        .expect_err("clean awaiting_merge task must remain parked");
    let text = result.to_string();
    assert!(
        text.contains("wait for the supervisor"),
        "refusal must preserve the normal merge guidance: {text}"
    );
    assert!(
        text.to_lowercase().contains("conflict"),
        "refusal must name the conflict rework path: {text}"
    );

    let after = env.task_store().get(&task_id).expect("task");
    assert_eq!(after.status, TaskStatus::AwaitingMerge);
}

/// AC1 (self-dispatch guard preserved): starting an `awaiting_merge` task must
/// still be refused for a DIFFERENT worker than the recorded assignee — the
/// fix permits the *assigned* worker to resume, not free-for-all self-dispatch.
#[tokio::test]
async fn test_a844_other_worker_still_refused_on_awaiting_merge() {
    let _guard = EnvGuard::set_optional(&[
        ("CAS_AGENT_ROLE", Some("worker")),
        ("CAS_FACTORY_MODE", Some("1")),
    ]);
    let env = FactoryTestEnv::with_agent_id("brave-otter");
    {
        let store = env.agent_store();
        let mut worker = Agent::new("brave-otter".to_string(), "brave-otter".to_string());
        worker.role = AgentRole::Worker;
        store.register(&worker).expect("register worker");
    }
    // Parked under a different assignee ("swift-fox").
    let task_id = env.create_awaiting_merge_task("parked work", "swift-fox");

    let result = env
        .service
        .inner
        .cas_task_start(Parameters(cas::mcp::tools::IdRequest {
            id: task_id.clone(),
        }))
        .await;
    assert!(
        result.is_err(),
        "a worker other than the recorded assignee must still be refused: {result:?}"
    );

    let after = env.task_store().get(&task_id).expect("task");
    assert_eq!(
        after.status,
        TaskStatus::AwaitingMerge,
        "task must remain parked when the wrong worker attempts start"
    );
}

/// AC3: the branch name is recorded on the task the first time it parks
/// (`parked_branch`), independent of the commit-sha anchor, so recovery
/// doesn't depend on a supervisor remembering which branch held the work
/// after the original worker is lost (e.g. a fleet restart).
#[tokio::test]
async fn test_a844_park_records_branch_name_surviving_reassignment() {
    let _guard = EnvGuard::set_optional(&[("CAS_AGENT_ROLE", Some("supervisor"))]);
    let env = FactoryTestEnv::new();
    let task_id = env.create_awaiting_merge_task("parked work", "lost-worker");

    // Simulate what `park_task_awaiting_merge` does at close time.
    {
        let store = env.task_store();
        let mut task = store.get(&task_id).expect("task");
        task.deliverables.parked_branch = Some("factory/lost-worker".to_string());
        store.update(&task).expect("update");
    }

    // The original worker is gone; supervisor reassigns to a fresh worker.
    {
        let store = env.task_store();
        let mut task = store.get(&task_id).expect("task");
        task.assignee = Some("fresh-worker".to_string());
        store.update(&task).expect("reassign");
    }

    let after = env.task_store().get(&task_id).expect("task");
    assert_eq!(
        after.deliverables.parked_branch.as_deref(),
        Some("factory/lost-worker"),
        "parked_branch must survive reassignment — it's the only thing still \
         pointing at the orphaned commits once the original assignee is gone"
    );
}

/// AC2: a conflicted awaiting_merge must be visibly distinguishable from a
/// clean one in task show/list output — not read identically as "done,
/// pending a formality".
#[tokio::test]
async fn test_a844_show_and_list_distinguish_merge_conflict() {
    let _guard = EnvGuard::set_optional(&[("CAS_AGENT_ROLE", Some("supervisor"))]);
    let env = FactoryTestEnv::new();
    let clean_id = env.create_awaiting_merge_task("clean parked work", "swift-fox");
    let conflict_id = env.create_awaiting_merge_task("conflicted parked work", "brave-otter");
    {
        let store = env.task_store();
        let mut task = store.get(&conflict_id).expect("task");
        task.deliverables.merge_conflicted = true;
        task.deliverables.parked_branch = Some("factory/brave-otter".to_string());
        store.update(&task).expect("update");
    }

    let clean_show = env
        .service
        .inner
        .cas_task_show(Parameters(cas::mcp::tools::TaskShowRequest {
            id: clean_id.clone(),
            with_deps: false,
        }))
        .await
        .expect("show clean");
    let clean_text = get_text(&clean_show);
    assert!(
        !clean_text.to_uppercase().contains("MERGE CONFLICT"),
        "a clean awaiting_merge must not be flagged as conflicted: {clean_text}"
    );

    let conflict_show = env
        .service
        .inner
        .cas_task_show(Parameters(cas::mcp::tools::TaskShowRequest {
            id: conflict_id.clone(),
            with_deps: false,
        }))
        .await
        .expect("show conflict");
    let conflict_text = get_text(&conflict_show);
    assert!(
        conflict_text.to_uppercase().contains("MERGE CONFLICT"),
        "a conflicted awaiting_merge must be visibly flagged in show: {conflict_text}"
    );
    assert!(
        conflict_text.contains("factory/brave-otter"),
        "parked branch should be surfaced in show: {conflict_text}"
    );

    let list = env
        .service
        .inner
        .cas_task_list(Parameters(cas::mcp::tools::TaskListRequest {
            limit: None,
            scope: "all".to_string(),
            status: Some("awaiting_merge".to_string()),
            task_type: None,
            label: None,
            assignee: None,
            epic: None,
            sort: None,
            sort_order: None,
            include_foreign: false,
        }))
        .await
        .expect("list");
    let list_text = get_text(&list);
    assert!(
        list_text.contains(&format!("[{conflict_id}]")) && list_text.contains("MERGE CONFLICT"),
        "conflicted task must show a MERGE CONFLICT marker in list: {list_text}"
    );
    // The clean one's line must not carry the marker.
    let clean_line = list_text
        .lines()
        .find(|l| l.contains(&clean_id))
        .unwrap_or("");
    assert!(
        !clean_line.contains("MERGE CONFLICT"),
        "clean awaiting_merge line must not carry the conflict marker: {clean_line}"
    );
}

// ===========================================================================
// cas-0a6f (GH #103): sync_all_workers must not rebase a worktree that is
// dirty or whose assignee is mid-task, and a stranded stash must reach both
// the worker and the supervisor. These drive the real MCP handler against
// real linked worktrees — the pure-unit tests in factory_ops.rs cover the
// decision table, these cover the wiring.
// ===========================================================================

fn sync_env_with_worker(session: &str, worker: &str) -> (FactoryTestEnv, PathBuf, EnvGuard) {
    let home = TempDir::new().expect("home tempdir");
    let guard = EnvGuard::set(&[
        ("CAS_FACTORY_SESSION", session),
        ("HOME", home.path().to_str().unwrap()),
        // A real factory session exports this; blank it so supervisor
        // resolution is decided by the fixture's agent store rather than by
        // whatever session happens to be running the suite.
        ("CAS_SUPERVISOR_NAME", ""),
    ]);
    std::mem::forget(home);
    let env = FactoryTestEnv::new();
    let worker_path = init_sync_repo(&env, worker);
    env.register_worker_in_session(worker, session);
    add_epic_with_id(&env, "cas-3b7c", TaskStatus::Open, "epic/requested");
    write_session_metadata_for_project(
        session,
        Some("cas-3b7c"),
        env.cas_root.parent().unwrap().to_str().unwrap(),
    );
    (env, worker_path, guard)
}

#[tokio::test]
async fn test_sync_all_workers_skips_dirty_worktree_without_force_cas_0a6f() {
    let (env, worker_path, _guard) =
        sync_env_with_worker("session-sync-dirty", "sync-dirty-worker");

    // Live WIP the worker has not committed.
    std::fs::write(worker_path.join("wip.txt"), "precious uncommitted work").unwrap();

    let mut req = factory_req("sync_all_workers");
    req.id = Some("cas-3b7c".to_string());
    let text = get_text(&env.service.factory(Parameters(req)).await.expect("sync"));

    assert!(
        text.contains("Skipped:") && text.contains("uncommitted change(s)"),
        "a dirty worktree must be reported as skipped: {text}"
    );
    assert!(
        !text.contains("Synced:"),
        "nothing should have been rebased: {text}"
    );
    assert_eq!(
        std::fs::read_to_string(worker_path.join("wip.txt")).unwrap(),
        "precious uncommitted work",
        "WIP must be untouched"
    );
    assert!(
        !worker_path.join("requested.txt").exists(),
        "the worktree must not have been rebased onto the epic"
    );
    assert!(
        git_stdout(&worker_path, &["stash", "list"])
            .trim()
            .is_empty(),
        "sync must not have stashed anything"
    );
}

#[tokio::test]
async fn test_sync_all_workers_force_syncs_dirty_worktree_and_restores_wip_cas_0a6f() {
    let (env, worker_path, _guard) =
        sync_env_with_worker("session-sync-force", "sync-force-worker");

    std::fs::write(worker_path.join("wip.txt"), "precious uncommitted work").unwrap();

    let mut req = factory_req("sync_all_workers");
    req.id = Some("cas-3b7c".to_string());
    req.force = Some(true);
    let text = get_text(&env.service.factory(Parameters(req)).await.expect("sync"));

    assert!(
        text.contains("Synced:") && text.contains("stashed + rebased + restored"),
        "force must carry the dirty worktree through: {text}"
    );
    assert!(
        worker_path.join("requested.txt").exists(),
        "the epic commit must have landed: {text}"
    );
    assert_eq!(
        std::fs::read_to_string(worker_path.join("wip.txt")).unwrap(),
        "precious uncommitted work",
        "the worker's WIP must be restored after the rebase"
    );
    assert!(
        git_stdout(&worker_path, &["stash", "list"])
            .trim()
            .is_empty(),
        "a restored stash must not be left behind"
    );
}

#[tokio::test]
async fn test_sync_all_workers_skips_worker_holding_an_in_progress_task_cas_0a6f() {
    let (env, worker_path, _guard) = sync_env_with_worker("session-sync-busy", "sync-busy-worker");

    // Clean worktree, but the worker is actively working a task.
    let store = env.task_store();
    let task_id = store.generate_id().expect("generate_id");
    let mut task = Task::new(task_id.clone(), "Live work".to_string());
    task.status = TaskStatus::InProgress;
    task.assignee = Some("sync-busy-worker".to_string());
    store.add(&task).expect("add in-progress task");

    let mut req = factory_req("sync_all_workers");
    req.id = Some("cas-3b7c".to_string());
    let text = get_text(&env.service.factory(Parameters(req)).await.expect("sync"));

    assert!(
        text.contains("Skipped:") && text.contains(&task_id),
        "the skip must name the task holding the worktree: {text}"
    );
    assert!(
        !worker_path.join("requested.txt").exists(),
        "a mid-task worktree must not be rebased under the worker: {text}"
    );
}

#[tokio::test]
async fn test_sync_all_workers_notifies_worker_and_supervisor_on_stranded_stash_cas_0a6f() {
    let (env, worker_path, _guard) =
        sync_env_with_worker("session-sync-strand", "sync-strand-worker");
    env.register_supervisor("sync-strand-supervisor");

    // Untracked WIP that collides with the file the epic commit introduces:
    // the rebase succeeds, then the stash pop cannot restore it.
    std::fs::write(
        worker_path.join("requested.txt"),
        "local uncommitted version",
    )
    .unwrap();

    let mut req = factory_req("sync_all_workers");
    req.id = Some("cas-3b7c".to_string());
    req.force = Some(true);
    let text = get_text(&env.service.factory(Parameters(req)).await.expect("sync"));

    assert!(
        text.contains("Failed:") && text.contains("WIP IS NOT LOST"),
        "a stranded stash must be reported loudly: {text}"
    );
    assert!(
        text.contains("Incident notifications:"),
        "the report must record that the incident was pushed: {text}"
    );

    let queue = env.prompt_queue();
    for target in ["sync-strand-worker", "sync-strand-supervisor"] {
        let queued = queue.poll_for_target(target, 10).expect("poll queue");
        let incident = queued
            .iter()
            .find(|q| q.prompt.contains("SYNC INCIDENT"))
            .unwrap_or_else(|| panic!("{target} must receive the incident: {queued:?}"));
        // The prose may quote the git command that FAILED ("stash pop
        // failed: ..."); what matters is the command block the operator is
        // told to run. `git stash pop <sha>` would be rejected by git as
        // "not a stash reference", so the instruction must be `apply`.
        let instruction = incident.prompt.split("run:").nth(1).unwrap_or_else(|| {
            panic!(
                "incident must contain an instruction block: {}",
                incident.prompt
            )
        });
        assert!(
            instruction.contains("git stash apply"),
            "the recovery command must be one git accepts for a SHA: {instruction}"
        );
        assert!(
            !instruction.contains("git stash pop"),
            "`stash pop` rejects the SHA we hand out: {instruction}"
        );
        assert!(
            instruction.contains("--include-untracked"),
            "inspection must reveal untracked WIP: {instruction}"
        );
    }

    // The WIP really is recoverable via the instruction that was sent.
    assert!(
        !git_stdout(&worker_path, &["stash", "list"])
            .trim()
            .is_empty(),
        "the stash entry must survive for recovery"
    );
}

// ===========================================================================
// cas-f8bc (GH #106): the circular authorization deadlock.
//
//   worker closes A → produces a standalone fix for new task B on its branch
//   → worktree_merge refuses (task B has no assignee/lease)
//   → assignment refuses ("N commits behind epic")
//   → but it is behind ONLY because the worker's own lane was merged, and the
//     assignment it is refusing is the prerequisite the merge path asked for.
//
// These drive the real assignment gate through the MCP surface.
// ===========================================================================

/// Build the exact post-merge state from the live repro: the worker's own lane
/// is merged into the epic, so the epic is one merge commit "ahead".
fn repo_with_worker_lane_merged(env: &FactoryTestEnv, worker: &str) -> PathBuf {
    let worker_path = init_sync_repo(env, worker);
    let project = env.cas_root.parent().expect("project root");

    let run = |dir: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "CAS Test")
            .env("GIT_AUTHOR_EMAIL", "test@cas")
            .env("GIT_COMMITTER_NAME", "CAS Test")
            .env("GIT_COMMITTER_EMAIL", "test@cas")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };

    // The worker starts current with the epic — the deadlock is about a worker
    // whose ONLY gap is its own landed work, so any pre-existing staleness must
    // be out of the picture first.
    run(&worker_path, &["rebase", "epic/requested"]);

    // Worker does its work on its own branch.
    std::fs::write(worker_path.join("fix.txt"), "worker fix").unwrap();
    run(&worker_path, &["add", "."]);
    run(&worker_path, &["commit", "-m", "worker fix"]);

    // Supervisor merges that lane into the epic branch.
    run(project, &["checkout", "epic/requested"]);
    run(
        project,
        &[
            "merge",
            "--no-ff",
            &format!("factory/{worker}"),
            "-m",
            "Merge worker lane",
        ],
    );
    run(project, &["checkout", "main"]);
    worker_path
}

fn child_task_of_epic(env: &FactoryTestEnv, epic_id: &str, title: &str) -> String {
    let store = env.task_store();
    let id = store.generate_id().expect("generate_id");
    let task = Task::new(id.clone(), title.to_string());
    store.add(&task).expect("add child task");
    store
        .add_dependency(&cas::types::Dependency {
            from_id: id.clone(),
            to_id: epic_id.to_string(),
            dep_type: cas::types::DependencyType::ParentChild,
            created_at: chrono::Utc::now(),
            created_by: Some("test".to_string()),
        })
        .expect("link child to epic");
    id
}

async fn assign(env: &FactoryTestEnv, task_id: &str, assignee: &str) -> Result<String, String> {
    let req: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "update",
        "id": task_id,
        "assignee": assignee,
    }))
    .expect("task request");
    match env.service.task(Parameters(req)).await {
        Ok(result) => Ok(get_text(&result)),
        Err(error) => Err(error.message.to_string()),
    }
}

#[tokio::test]
async fn test_assignment_is_not_blocked_by_the_workers_own_merged_lane_cas_f8bc() {
    let home = TempDir::new().expect("home tempdir");
    let _guard = EnvGuard::set_optional(&[
        ("CAS_FACTORY_MODE", Some("1")),
        ("CAS_FACTORY_SESSION", Some("session-f8bc-own")),
        ("HOME", Some(home.path().to_str().unwrap())),
    ]);
    let env = FactoryTestEnv::new();
    let worker = "f8bc-own-worker";
    let worker_path = repo_with_worker_lane_merged(&env, worker);
    add_epic_with_id(&env, "cas-3b7c", TaskStatus::Open, "epic/requested");

    {
        let store = env.agent_store();
        let mut agent = Agent::new(Agent::generate_fallback_id(), worker.to_string());
        agent.role = AgentRole::Worker;
        agent.factory_session = Some("session-f8bc-own".to_string());
        agent.metadata.insert(
            "clone_path".to_string(),
            worker_path.to_str().unwrap().to_string(),
        );
        store.register(&agent).expect("register worker");
    }

    let task_b = child_task_of_epic(&env, "cas-3b7c", "standalone fix follow-up");
    let outcome = assign(&env, &task_b, worker).await;

    let text = outcome.unwrap_or_else(|error| {
        panic!(
            "assignment must not be refused for the worker's own merged lane \
             — that refusal is the deadlock (GH #106): {error}"
        )
    });
    assert!(
        text.contains("assignee"),
        "the assignment must actually be applied: {text}"
    );
    assert!(
        !text.contains("commit(s) behind"),
        "no staleness warning is warranted for a worker's own landed work: {text}"
    );

    // And the sanctioned merge path is now open: worktree_merge's conservative
    // rule authorizes on assignee match, no lease required.
    let task = env.task_store().get(&task_b).expect("task");
    assert_eq!(task.assignee.as_deref(), Some(worker));
}

#[tokio::test]
async fn test_assignment_still_refuses_a_genuinely_stale_worker_cas_f8bc() {
    let home = TempDir::new().expect("home tempdir");
    let _guard = EnvGuard::set_optional(&[
        ("CAS_FACTORY_MODE", Some("1")),
        ("CAS_FACTORY_SESSION", Some("session-f8bc-stale")),
        ("HOME", Some(home.path().to_str().unwrap())),
    ]);
    let env = FactoryTestEnv::new();
    let worker = "f8bc-stale-worker";
    // init_sync_repo leaves epic/requested one real commit ahead of the
    // worker's branch, and nothing of the worker's has been merged.
    let worker_path = init_sync_repo(&env, worker);
    add_epic_with_id(&env, "cas-3b7c", TaskStatus::Open, "epic/requested");

    {
        let store = env.agent_store();
        let mut agent = Agent::new(Agent::generate_fallback_id(), worker.to_string());
        agent.role = AgentRole::Worker;
        agent.factory_session = Some("session-f8bc-stale".to_string());
        agent.metadata.insert(
            "clone_path".to_string(),
            worker_path.to_str().unwrap().to_string(),
        );
        store.register(&agent).expect("register worker");
    }

    let task_b = child_task_of_epic(&env, "cas-3b7c", "work needing fresh base");
    let error = assign(&env, &task_b, worker)
        .await
        .expect_err("a worker missing real epic commits must still be refused");
    assert!(
        error.contains("commits behind") && error.contains("epic/requested"),
        "the genuine staleness guard must survive the exemption: {error}"
    );
}

// ===========================================================================
// cas-aae6 (GH #110): epic_status must show the chain for a stacked epic.
// The renderer is unit-tested with a hand-built chain; this drives the real
// handler against a real three-deep stack, which is the only thing that can
// catch a mis-wiring (wrong branch, wrong trunk, swapped arguments).
// ===========================================================================

#[tokio::test]
async fn test_epic_status_reports_a_three_deep_stack_end_to_end_cas_aae6() {
    let home = TempDir::new().expect("home tempdir");
    let _guard = EnvGuard::set_optional(&[
        ("CAS_FACTORY_MODE", Some("1")),
        ("HOME", Some(home.path().to_str().unwrap())),
    ]);
    let env = FactoryTestEnv::new();
    let project = env.cas_root.parent().expect("project root").to_path_buf();

    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&project)
            .env("GIT_AUTHOR_NAME", "CAS Test")
            .env("GIT_AUTHOR_EMAIL", "test@cas")
            .env("GIT_COMMITTER_NAME", "CAS Test")
            .env("GIT_COMMITTER_EMAIL", "test@cas")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let commit = |name: &str| {
        std::fs::write(project.join(name), name).unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", name]);
    };

    git(&["init", "-b", "main"]);
    git(&["config", "user.email", "test@cas"]);
    git(&["config", "user.name", "CAS Test"]);
    commit("seed.txt");
    git(&["checkout", "-b", "epic/a"]);
    commit("a.txt");
    git(&["checkout", "-b", "epic/b"]);
    commit("b.txt");
    git(&["checkout", "-b", "epic/c"]);
    commit("c.txt");
    git(&["checkout", "main"]);

    add_epic_with_id(&env, "cas-top", TaskStatus::Open, "epic/c");

    let mut req = factory_req("epic_status");
    req.id = Some("cas-top".to_string());
    let text = get_text(&env.service.factory(Parameters(req)).await.expect("status"));

    assert!(
        text.contains("Stacked on: 2 unlanded epic branch(es) — 'epic/a' → 'epic/b'"),
        "the full chain must reach the supervisor-facing report: {text}"
    );
    assert!(
        text.contains("Landing order: 'epic/a' → 'epic/b' → 'epic/c'"),
        "bottom-up order must be shown: {text}"
    );
}

#[tokio::test]
async fn test_epic_status_omits_stack_lines_for_an_unstacked_epic_cas_aae6() {
    let home = TempDir::new().expect("home tempdir");
    let _guard = EnvGuard::set_optional(&[
        ("CAS_FACTORY_MODE", Some("1")),
        ("HOME", Some(home.path().to_str().unwrap())),
    ]);
    let env = FactoryTestEnv::new();
    let project = env.cas_root.parent().expect("project root").to_path_buf();

    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&project)
            .env("GIT_AUTHOR_NAME", "CAS Test")
            .env("GIT_AUTHOR_EMAIL", "test@cas")
            .env("GIT_COMMITTER_NAME", "CAS Test")
            .env("GIT_COMMITTER_EMAIL", "test@cas")
            .output()
            .expect("git");
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.email", "test@cas"]);
    git(&["config", "user.name", "CAS Test"]);
    std::fs::write(project.join("seed.txt"), "seed").unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "seed"]);
    git(&["branch", "epic/solo"]);

    add_epic_with_id(&env, "cas-solo", TaskStatus::Open, "epic/solo");

    let mut req = factory_req("epic_status");
    req.id = Some("cas-solo".to_string());
    let text = get_text(&env.service.factory(Parameters(req)).await.expect("status"));

    assert!(
        !text.contains("Stacked on"),
        "an epic cut straight from trunk must not claim a stack: {text}"
    );
}

/// cas-50fe: projects that land child work directly on their configured
/// integration branch must not be forced to fast-forward the cosmetic epic
/// branch merely to satisfy `epic_status` or the close gate.  The child below
/// is intentionally reachable from `main` but not from `epic/main-only`.
///
/// This is end-to-end because the diagnostic and close path must select the
/// same repository/branch authority; a unit test of the collector alone would
/// miss handler wiring back to `epic.branch`.
#[tokio::test]
async fn test_epic_status_and_close_use_declared_target_branch_cas_50fe() {
    let home = TempDir::new().expect("home tempdir");
    let _guard = EnvGuard::set_optional(&[
        ("CAS_FACTORY_MODE", Some("1")),
        ("HOME", Some(home.path().to_str().unwrap())),
    ]);
    let env = FactoryTestEnv::new();
    let project = env.cas_root.parent().expect("project root").to_path_buf();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&project)
            .env("GIT_AUTHOR_NAME", "CAS Test")
            .env("GIT_AUTHOR_EMAIL", "test@cas")
            .env("GIT_COMMITTER_NAME", "CAS Test")
            .env("GIT_COMMITTER_EMAIL", "test@cas")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };

    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@cas"]);
    git(&["config", "user.name", "CAS Test"]);
    git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/example/main-only.git",
    ]);
    std::fs::write(project.join("seed.rs"), "// seed\n").unwrap();
    git(&["add", "seed.rs"]);
    git(&["commit", "-q", "-m", "seed"]);
    git(&["checkout", "-q", "-b", "epic/main-only"]);
    git(&["checkout", "-q", "main"]);
    git(&["checkout", "-q", "-b", "factory/alpha"]);
    std::fs::write(project.join("delivered.rs"), "// delivered\n").unwrap();
    git(&["add", "delivered.rs"]);
    git(&["commit", "-q", "-m", "deliver directly to main"]);
    git(&["checkout", "-q", "main"]);
    git(&["merge", "-q", "--ff-only", "factory/alpha"]);
    let delivered_sha = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "main"])
            .current_dir(&project)
            .output()
            .expect("resolve delivered tip")
            .stdout,
    )
    .expect("sha utf-8")
    .trim()
    .to_string();

    let store = env.task_store();
    let mut epic = Task::new(
        "cas-50fe-main-only".to_string(),
        "main-only epic".to_string(),
    );
    epic.task_type = TaskType::Epic;
    epic.status = TaskStatus::InProgress;
    epic.branch = Some("epic/main-only".to_string());
    epic.deliverables.work_target = Some(WorkTarget {
        repo_selector: "remote:github.com/example/main-only".to_string(),
        target_branch: "main".to_string(),
    });
    store.add(&epic).expect("add epic");
    let child_id = child_task_of_epic(&env, &epic.id, "direct-main child");
    let mut child = store.get(&child_id).expect("child");
    child.status = TaskStatus::Closed;
    child.assignee = Some("alpha".to_string());
    store.update(&child).expect("set child branch evidence");

    let mut status_req = factory_req("epic_status");
    status_req.id = Some(epic.id.clone());
    let status = get_text(
        &env.service
            .factory(Parameters(status_req))
            .await
            .expect("epic_status"),
    );
    assert!(
        status.contains("Parent branch: main")
            && status.contains("✓ All child factory branches are merged"),
        "status must evaluate the configured integration target, not the cosmetic epic branch: {status}"
    );

    std::fs::write(
        env.cas_root.join("config.toml"),
        "[verification]\nenabled = false\n",
    )
    .expect("disable verification for close-path fixture");
    let close_req: cas_mcp::TaskRequest = serde_json::from_value(serde_json::json!({
        "action": "close",
        "id": epic.id,
        "reason": "all children landed directly on main",
        "commit_receipt": delivered_sha,
    }))
    .expect("close request");
    let close = get_text(
        &env.service
            .task(Parameters(close_req))
            .await
            .expect("epic close against declared target"),
    );
    assert!(
        close.contains("Closed task:"),
        "epic close must share epic_status's target authority: {close}"
    );
}

// ---------------------------------------------------------------------------
// cas-99d2 (GH #126 / GH #127): message-confirmation truth and duplicate
// assignment redelivery. Fixtures reproduce the real records from factory
// session cas-src-noble-salmon-99 (2026-08-06).
// ---------------------------------------------------------------------------

/// cas-99d2 AC3 (GH #126): the false-confirm shape, asserted through the
/// surface a supervisor actually reads.
///
/// Notification 7124's real shape: transport-delivered to a worker, no
/// `prompt_queue_recipient_seen` row, then a reply from that worker to the
/// supervisor 12s later. `message_status` used to answer
/// "undelivered_after: n/a (confirmation_source: inferred_from_reply)",
/// which is what suppressed the supervisor's escalation gate. It must now stay
/// on the counting branch.
#[tokio::test]
async fn cas99d2_status_keeps_counting_when_a_reply_lacks_a_surfacing_receipt_gh126() {
    // Pin BOTH supervisor-resolution sources (message.rs `resolve_supervisor_name`:
    // CAS_SUPERVISOR_NAME first, then an Active/Idle Supervisor in the agent
    // store). `FactoryTestEnv::new()` registers no supervisor and the fixture's
    // cas_root is a fresh TempDir, so without this the worker's reply below can
    // only resolve `target="supervisor"` from an ambient CAS_SUPERVISOR_NAME
    // inherited from the surrounding shell — green inside a factory session,
    // red in clean CI (GH #136).
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_AGENT_NAME", "watchful-koala-20"),
        ("CAS_SUPERVISOR_NAME", "cosmic-bear-43"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_worker("watchful-koala-20");
    env.register_supervisor("cosmic-bear-43");

    // Supervisor -> worker, handed to the transport (no inbox drain by the
    // worker, exactly as in the incident).
    let message_id = env
        .prompt_queue()
        .enqueue_urgent(
            "supervisor",
            "watchful-koala-20",
            "factory/watchful-koala-20 is merged into the epic branch (merge commit 77fec76a). \
             Re-run close now.",
            None,
            Some("merged — re-close now"),
            None,
            false,
        )
        .expect("enqueue supervisor message");
    env.prompt_queue()
        .mark_transport_delivered(message_id)
        .expect("transport handoff");

    // The worker replies to the supervisor. Under the old rule this reply
    // confirmed the message above.
    let reply = coord_msg("message", "supervisor", "closed, standing by", None);
    env.service
        .coordination(Parameters(reply))
        .await
        .expect("worker reply should succeed");

    let report = env
        .prompt_queue()
        .message_delivery_report(message_id)
        .expect("delivery report")
        .expect("message exists");
    assert_eq!(
        report.confirmation_source,
        cas_store::ConfirmationSource::Unconfirmed,
        "a reply with no surfacing receipt must not confirm the message"
    );
    assert_eq!(report.stage, cas_store::DeliveryStage::Delivered);
    assert_eq!(
        report.pending_reason,
        Some(cas_store::PendingReason::AwaitingAck)
    );

    let mut status_req = coord_msg("message_status", "watchful-koala-20", "unused", None);
    status_req.notification_id = Some(message_id);
    let text = get_text(
        &env.service
            .coordination(Parameters(status_req))
            .await
            .expect("message_status"),
    );
    assert!(
        !text.contains("undelivered_after: n/a"),
        "undelivered_after must keep counting, not read n/a: {text}"
    );
    assert!(
        text.contains("undelivered_after:") && text.contains("not yet confirmed received"),
        "status must still report the message as unconfirmed: {text}"
    );
    let json_start = text.find('{').expect("JSON body");
    let json: serde_json::Value =
        serde_json::from_str(&text[json_start..]).expect("status JSON must parse");
    assert!(
        json.get("undelivered_after_secs")
            .expect("undelivered_after_secs")
            .is_number(),
        "undelivered_after_secs must be numeric, not null: {json}"
    );
}

/// cas-dcf2 (GH #390): even an inbox drain plus later activity is not a
/// transcript artifact proving THIS row entered the recipient's next turn.
/// The user-facing status must show the weaker state at its top line and keep
/// the undelivered clock running.
#[tokio::test]
async fn cas_dcf2_reply_after_an_inbox_drain_is_assumed_seen_not_confirmed_gh390() {
    // Same deterministic supervisor pinning as the negative case above: the
    // reply at the end of this test resolves `target="supervisor"`, which
    // otherwise depends on an ambient CAS_SUPERVISOR_NAME (GH #136).
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_AGENT_NAME", "watchful-koala-20"),
        ("CAS_SUPERVISOR_NAME", "cosmic-bear-43"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_worker("watchful-koala-20");
    env.register_supervisor("cosmic-bear-43");

    let message_id = env
        .prompt_queue()
        .enqueue_urgent(
            "supervisor",
            "watchful-koala-20",
            "re-close cas-7587 with a commit receipt",
            None,
            Some("re-close"),
            None,
            false,
        )
        .expect("enqueue");
    env.prompt_queue()
        .mark_transport_delivered(message_id)
        .expect("transport handoff");
    // The surfacing receipt: the worker pulls the row through its inbox.
    env.prompt_queue()
        .poll_unseen_for_recipient("watchful-koala-20", None, 10)
        .expect("inbox drain");

    let reply = coord_msg("message", "supervisor", "on it", None);
    env.service
        .coordination(Parameters(reply))
        .await
        .expect("worker reply");

    let report = env
        .prompt_queue()
        .message_delivery_report(message_id)
        .expect("delivery report")
        .expect("message exists");
    assert_eq!(report.stage, cas_store::DeliveryStage::AssumedSeen);
    assert!(report.confirmed_at.is_none());
    assert!(report.assumed_seen_at.is_some());

    let mut status_req = coord_msg("message_status", "watchful-koala-20", "unused", None);
    status_req.notification_id = Some(message_id);
    let text = get_text(
        &env.service
            .coordination(Parameters(status_req))
            .await
            .expect("message_status"),
    );
    assert!(
        text.contains("stage: assumed_seen"),
        "the top-line stage must expose activity inference as weaker than confirmation: {text}"
    );
    assert!(
        text.contains("undelivered_after:") && !text.contains("undelivered_after: n/a"),
        "activity inference must not clear the escalation clock: {text}"
    );
}

#[tokio::test]
async fn cas_dcf2_wake_starvation_is_top_line_status_not_a_lifecycle_relay_gh390() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "cosmic-bear-43"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_worker("watchful-koala-20");
    env.register_supervisor("cosmic-bear-43");

    let message_id = env
        .prompt_queue()
        .enqueue("supervisor", "watchful-koala-20", "blocking DDL ruling")
        .expect("enqueue");
    for _ in 0..3 {
        env.prompt_queue()
            .record_wake_gate_decline(message_id, "pane has not been silent long enough")
            .expect("record busy wake decline");
    }
    env.prompt_queue()
        .mark_undelivered_after_wake_declines(
            message_id,
            Some("wake gate declined 3 consecutive re-offers while recipient stayed busy"),
        )
        .expect("record terminal wake starvation");

    let mut status_req = coord_msg("message_status", "watchful-koala-20", "unused", None);
    status_req.notification_id = Some(message_id);
    let text = get_text(
        &env.service
            .coordination(Parameters(status_req))
            .await
            .expect("message_status"),
    );
    assert!(
        text.contains("stage: abandoned  pending_reason: undelivered_after_wake_declines"),
        "the terminal state must be visible at the status top line: {text}"
    );
    assert!(
        text.contains("undelivered_after:"),
        "a wake-starved row must retain an observable undelivered clock: {text}"
    );
    assert!(
        text.contains("wake_gate_declines: 3"),
        "the top line must expose the bounded consecutive-decline count: {text}"
    );
}

/// cas-4a27 (GH #334): the field reproduction had both halves at once: a
/// supervisor's real response was indistinguishable from delayed spawn
/// boilerplate, while the worker's escalation remained AwaitingAck because
/// the supervisor's transport had no surfacing receipt. A reply reference is
/// explicit evidence for that ONE escalation, and every delivered row exposes
/// durable provenance so the worker can act on the ordering mechanically.
#[tokio::test]
async fn cas4a27_supervisor_reply_is_linked_and_distinct_from_spawn_replay_gh334() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "cosmic-bear-43"),
    ]);
    let env = FactoryTestEnv::with_agent_id_and_env("cosmic-bear-id", None);
    let mut supervisor = Agent::new("cosmic-bear-id".to_string(), "cosmic-bear-43".to_string());
    supervisor.role = AgentRole::Supervisor;
    env.agent_store()
        .register(&supervisor)
        .expect("register calling supervisor");
    env.register_worker_with_id("koala-agent-id", "watchful-koala-20", None);

    let escalation_id = env
        .prompt_queue()
        .enqueue_urgent(
            "watchful-koala-20",
            "cosmic-bear-43",
            "Blocked: the task needs a policy decision; recommend option A.",
            None,
            Some("blocked escalation"),
            None,
            false,
        )
        .expect("enqueue worker escalation");
    env.prompt_queue()
        .mark_transport_delivered(escalation_id)
        .expect("supervisor transport handoff without a surfacing receipt");

    let spawn_id = env
        .prompt_queue()
        .enqueue_urgent(
            "director",
            "watchful-koala-20",
            "You were spawned for task cas-4a27 — start it now.",
            None,
            Some("Assigned task: cas-4a27"),
            None,
            false,
        )
        .expect("enqueue delayed spawn brief");

    let mut reply = coord_msg(
        "message",
        "watchful-koala-20",
        "I read your escalation, endorse option A, and assigned the follow-up.",
        None,
    );
    reply.summary = Some("escalation acknowledged".to_string());
    reply.in_reply_to = Some(escalation_id);
    env.service
        .coordination(Parameters(reply))
        .await
        .expect("explicit supervisor reply");

    let escalation = env
        .prompt_queue()
        .message_delivery_report(escalation_id)
        .expect("escalation delivery report")
        .expect("escalation exists");
    assert_eq!(
        escalation.confirmation_source,
        cas_store::ConfirmationSource::ExplicitAck,
        "the precise reply reference must resolve the escalation even without a surfacing receipt: {escalation:?}"
    );

    let worker_core = CasCore::with_daemon(env.cas_root.clone(), None, None);
    worker_core.set_agent_id_for_testing("koala-agent-id".to_string());
    let worker_service = CasService::new(worker_core, None);
    let text = get_text(
        &worker_service
            .coordination(Parameters(coord_req("inbox_poll")))
            .await
            .expect("worker inbox poll"),
    );
    assert!(
        text.contains(&format!(
            "notification_id={spawn_id} origin=spawn-boilerplate"
        )) && text.contains("delivery=first-delivery"),
        "the delayed spawn brief needs machine-readable origin, ID, time, and delivery state: {text}"
    );
    assert!(
        text.contains("origin=supervisor-authored")
            && text.contains(&format!("notification_id={escalation_id}")),
        "the actual supervisor reply must be visibly fresh and linked to the escalation: {text}"
    );
    assert!(
        text.contains(&format!(
            "[CAS reply: explicitly acknowledges notification_id={escalation_id}]"
        )),
        "the reply must name the exact escalation it acknowledges: {text}"
    );
}

/// cas-99d2 AC4 (GH #127): notification 7112's shape.
///
/// An assignment was transport-delivered at 18:55:59; the worker started,
/// implemented and parked the task; then at 19:10:42 its inbox drain returned
/// the SAME assignment verbatim, reading as a fresh instruction. With the
/// solicited task-start transition on record for this worker, the poll must
/// withhold the row instead of re-rendering it.
#[tokio::test]
async fn cas99d2_inbox_poll_withholds_a_consumed_assignment_gh127() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_AGENT_NAME", "watchful-koala-20"),
    ]);
    let env = FactoryTestEnv::with_agent_id_and_env("koala-agent-id", None);
    env.register_worker_with_id("koala-agent-id", "watchful-koala-20", None);

    // The task the assignment solicited a start for, already started by this
    // worker — the consumption evidence.
    let task_id = {
        let store = env.task_store();
        let id = store.generate_id().expect("generate_id");
        let mut task = Task::new(id.clone(), "GH #122 spawn base resolution".to_string());
        task.status = TaskStatus::InProgress;
        task.assignee = Some("watchful-koala-20".to_string());
        store.add(&task).expect("add started task");
        id
    };

    let assignment = format!(
        "You are assigned task {task_id} (P2 bug, epic cas-b0c7). Run \
         `mcp__cas__task action=show id={task_id}` then \
         `mcp__cas__task action=start id={task_id}`."
    );
    let assignment_id = env
        .prompt_queue()
        .enqueue_urgent(
            "supervisor",
            "watchful-koala-20",
            &assignment,
            None,
            Some("assignment"),
            None,
            false,
        )
        .expect("enqueue assignment");
    env.prompt_queue()
        .mark_transport_delivered(assignment_id)
        .expect("transport handoff");

    let text = get_text(
        &env.service
            .coordination(Parameters(coord_req("inbox_poll")))
            .await
            .expect("inbox_poll"),
    );
    assert!(
        !text.contains("You are assigned task"),
        "an already-delivered assignment whose task this worker has started must not be \
         re-rendered verbatim: {text}"
    );
    assert!(
        text.contains(&format!("{assignment_id}")) && text.contains("already done"),
        "the withheld row must be named, not silently dropped: {text}"
    );
}

/// cas-8aee (GH #336): a task can reach a terminal state while an assignment
/// or spawn-intro prompt is queued. `inbox_poll` must name each withheld row
/// and say it is already done, but must never repeat its task-start imperative.
#[tokio::test]
async fn cas8aee_inbox_poll_names_terminal_assignment_and_spawn_intro_gh336() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_AGENT_NAME", "watchful-koala-20"),
    ]);
    let env = FactoryTestEnv::with_agent_id_and_env("koala-agent-id", None);
    env.register_worker_with_id("koala-agent-id", "watchful-koala-20", None);

    let (closed_task_id, cancelled_task_id) = {
        let store = env.task_store();
        let closed_task_id = store.generate_id().expect("generate closed id");
        let cancelled_task_id = store.generate_id().expect("generate cancelled id");
        let mut closed = Task::new(closed_task_id.clone(), "already closed".to_string());
        closed.status = TaskStatus::Closed;
        closed.assignee = Some("watchful-koala-20".to_string());
        store.add(&closed).expect("add closed task");
        let mut cancelled = Task::new(cancelled_task_id.clone(), "already cancelled".to_string());
        cancelled.status = TaskStatus::Cancelled;
        cancelled.assignee = Some("watchful-koala-20".to_string());
        store.add(&cancelled).expect("add cancelled task");
        (closed_task_id, cancelled_task_id)
    };

    let assignment_id = env
        .prompt_queue()
        .enqueue_urgent(
            "supervisor",
            "watchful-koala-20",
            &format!(
                "You have been assigned a new task:\nTask ID: {closed_task_id}\nStart working: \
                 mcp__cas__task action=start id={closed_task_id}\nThen send an ACK to supervisor."
            ),
            None,
            Some("terminal assignment"),
            None,
            false,
        )
        .expect("enqueue terminal assignment");
    let spawn_intro_id = env
        .prompt_queue()
        .enqueue_urgent(
            "director",
            "watchful-koala-20",
            &format!(
                "You were spawned for task {cancelled_task_id} — \"already cancelled\" — and it \
                 is assigned to you now.\nStart with `mcp__cas__task action=show \
                 id={cancelled_task_id}`, then `mcp__cas__task action=start \
                 id={cancelled_task_id}` before you change any code."
            ),
            None,
            Some("terminal spawn intro"),
            None,
            false,
        )
        .expect("enqueue terminal spawn intro");

    let text = get_text(
        &env.service
            .coordination(Parameters(coord_req("inbox_poll")))
            .await
            .expect("inbox_poll"),
    );

    for id in [assignment_id, spawn_intro_id] {
        assert!(
            text.contains(&id.to_string()),
            "every withheld notification must remain named: {text}"
        );
    }
    assert!(
        text.contains("already done"),
        "a terminal assignment must explain why it is not actionable: {text}"
    );
    assert!(
        !text.contains("Start working") && !text.contains("action=start"),
        "terminal rows must not re-render task-start guidance: {text}"
    );
}

/// cas-99d2 AC4 (GH #127): a repeat delivery that IS handed over carries an
/// explicit machine-checkable marker, so a recipient can discard duplicates
/// without reasoning about timestamps. A row that was never transport-delivered
/// carries no marker.
#[tokio::test]
async fn cas99d2_inbox_poll_marks_redelivered_rows_gh127() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_AGENT_NAME", "watchful-koala-20"),
    ]);
    let env = FactoryTestEnv::with_agent_id_and_env("koala-agent-id", None);
    env.register_worker_with_id("koala-agent-id", "watchful-koala-20", None);

    let repeat_id = env
        .prompt_queue()
        .enqueue_urgent(
            "supervisor",
            "watchful-koala-20",
            "the merge you are waiting on ALREADY HAPPENED",
            None,
            Some("merge landed"),
            None,
            false,
        )
        .expect("enqueue repeat");
    env.prompt_queue()
        .mark_transport_delivered(repeat_id)
        .expect("transport handoff");
    let fresh_id = env
        .prompt_queue()
        .enqueue_urgent(
            "supervisor",
            "watchful-koala-20",
            "new follow-up instruction",
            None,
            Some("follow-up"),
            None,
            false,
        )
        .expect("enqueue fresh");

    let text = get_text(
        &env.service
            .coordination(Parameters(coord_req("inbox_poll")))
            .await
            .expect("inbox_poll"),
    );

    assert!(
        text.contains("ALREADY HAPPENED") && text.contains("new follow-up instruction"),
        "both rows must still be delivered: {text}"
    );
    let repeat_header = text
        .lines()
        .find(|line| line.contains(&format!("[{repeat_id}]")))
        .expect("repeat row header");
    assert!(
        repeat_header.contains("[redelivery]"),
        "an already-transport-delivered row must be marked: {repeat_header}"
    );
    let fresh_header = text
        .lines()
        .find(|line| line.contains(&format!("[{fresh_id}]")))
        .expect("fresh row header");
    assert!(
        !fresh_header.contains("[redelivery]"),
        "a first delivery must not be marked as a repeat: {fresh_header}"
    );
}

// =============================================================================
// cas-5087 / cas-15f2: cross-session supervisor delivery, end to end
// =============================================================================

/// The acceptance evidence for cas-15f2: two supervisors registered in
/// DIFFERENT factory sessions on ONE clone, and a message from A that actually
/// reaches B.
///
/// The incident this pins: notifications 24382 and 24399 were stamped with the
/// SENDER's session (`cas-src-young-raven-93`) while the target lived in
/// `cas-src-vivid-sparrow-8`. Every downstream filter selects on
/// `factory_session = <observer's own session>`, so no daemon ever selected
/// them; both died at `abandoned_unknown_target` with `delivery_attempts=0`
/// while two supervisors watched `awaiting_delivery` for fifteen minutes.
///
/// This is the deliberate complement of
/// `crates/cas-factory/tests/concurrent_factory_session_isolation.rs`, which
/// asserts that an UNADDRESSED row (a same-name worker, an `all_workers`
/// broadcast) must NOT cross sessions. Both halves are the contract: addressed
/// cross-session delivers, unaddressed cross-session still cannot leak.
#[tokio::test]
async fn cas_5087_a_supervisor_message_crosses_sessions_and_wakes_the_recipient() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "young-raven-93"),
        ("CAS_FACTORY_SESSION", "cas-src-young-raven-93"),
    ]);
    // A is the MCP caller, so its registered row must carry the caller's agent
    // id — that row is what resolves this sender's identity, exactly as a live
    // supervisor's own registration does.
    let env = FactoryTestEnv::with_agent_id("supervisor-a-id");
    let mut supervisor_a = Agent::new("supervisor-a-id".to_string(), "young-raven-93".to_string());
    supervisor_a.role = AgentRole::Supervisor;
    supervisor_a.factory_session = Some("cas-src-young-raven-93".to_string());
    env.agent_store()
        .register(&supervisor_a)
        .expect("register supervisor A");
    env.register_supervisor_in_session("noble-lynx-44", "cas-src-vivid-sparrow-8");

    // (1) A messages B.
    let send = get_text(
        &env.service
            .coordination(Parameters(coord_msg(
                "message",
                "noble-lynx-44",
                "Release gate: hold the merge queue until my epic lands.",
                None,
            )))
            .await
            .expect("a supervisor must be able to message a supervisor in another session"),
    );
    let notification_id = send
        .lines()
        .find_map(|line| line.strip_prefix("notification_id: "))
        .expect("send response must return a notification_id")
        .parse::<i64>()
        .expect("notification_id must be an integer");

    // (2) The row belongs to the RECIPIENT's session. This single stamp is the
    // whole routing fix — every filter below selects on it.
    let rows = env.prompt_queue().peek_all(10).expect("peek");
    let row = rows
        .iter()
        .find(|row| row.id == notification_id)
        .expect("the queued row must be the one the sender was told about");
    assert_eq!(
        row.factory_session.as_deref(),
        Some("cas-src-vivid-sparrow-8"),
        "the row must carry the RECIPIENT's session, not the sender's: {row:?}"
    );

    // B's daemon selects it; A's daemon does not. This is the exact query that
    // returned nothing during the incident.
    let b_targets = ["noble-lynx-44", "supervisor", "all_workers"];
    let for_b = env
        .prompt_queue()
        .peek_for_targets(&b_targets, Some("cas-src-vivid-sparrow-8"), 10)
        .expect("peek for B");
    assert!(
        for_b.iter().any(|queued| queued.id == notification_id),
        "the recipient's daemon must select the row: {for_b:?}"
    );
    let a_targets = ["young-raven-93", "supervisor", "all_workers"];
    let for_a = env
        .prompt_queue()
        .peek_for_targets(&a_targets, Some("cas-src-young-raven-93"), 10)
        .expect("peek for A");
    assert!(
        !for_a.iter().any(|queued| queued.id == notification_id),
        "the sender's own daemon must not also select a row addressed elsewhere: {for_a:?}"
    );

    // (3) The wake gate. Routing alone does not prove the demo — an inbox-only
    // row is found by polling, which is the failure cas-15f2's wake slice
    // exists to end. Both predicates below are the daemon's own, reached
    // through the seam `FactoryDaemon::supervisor_wake_decision` calls.
    let agents = env.agent_store().list(None).expect("unscoped roster");
    assert_eq!(
        row.source, "young-raven-93",
        "the row must name the SENDING supervisor; a collapsed \"supervisor\" source \
         resolves to no roster row and can never wake a pane"
    );
    assert!(
        cas::factory_supervisor_overlap::names_a_registered_supervisor(&agents, &row.source),
        "B's daemon must resolve the sender to a registered supervisor across sessions"
    );
    assert!(
        cas::factory_supervisor_overlap::is_peer_supervisor_message(
            &row.source,
            "noble-lynx-44",
            true
        ),
        "a peer supervisor's row must be wake-eligible on B's pane"
    );

    // (4) B's daemon delivers it and records what its wake nudge did.
    env.prompt_queue()
        .record_selected(notification_id)
        .expect("select");
    env.prompt_queue()
        .mark_transport_delivered(notification_id)
        .expect("transport handoff");
    env.prompt_queue()
        .record_wake_attempt(
            notification_id,
            cas_store::WakeAttempt::Fired,
            Some("supervisor pane is quiet and the row is a peer supervisor message"),
        )
        .expect("record wake attempt");

    let mut status_req = coord_msg("message_status", "noble-lynx-44", "unused", None);
    status_req.notification_id = Some(notification_id);
    let status = get_text(
        &env.service
            .coordination(Parameters(status_req))
            .await
            .expect("message_status"),
    );
    let json: serde_json::Value = serde_json::from_str(
        &status[status.find('{').expect("status must carry a JSON body")..],
    )
    .expect("status JSON must parse");

    assert_eq!(
        json["factory_session"].as_str(),
        Some("cas-src-vivid-sparrow-8"),
        "the report must agree with the row it describes: {status}"
    );
    assert!(
        json["delivered_at"].is_string(),
        "the message must reach delivered_at, not sit at awaiting_delivery: {status}"
    );
    assert_eq!(
        json["wake_attempt"].as_str(),
        Some("fired"),
        "a wake attempt must be recorded, not left at nudge_not_attempted: {status}"
    );
    assert!(
        !status.contains("abandoned"),
        "the incident's terminal state must not reappear: {status}"
    );
}

/// The complement, asserted on the same fixture so the two rules cannot drift:
/// a row A broadcasts to its OWN workers must stay in A's session even though a
/// same-named worker exists in B's. Cross-session delivery is addressed-only.
#[tokio::test]
async fn cas_5087_an_unaddressed_broadcast_still_cannot_cross_sessions() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "young-raven-93"),
        ("CAS_FACTORY_SESSION", "cas-src-young-raven-93"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_supervisor_in_session("young-raven-93", "cas-src-young-raven-93");
    env.register_supervisor_in_session("noble-lynx-44", "cas-src-vivid-sparrow-8");
    env.register_worker_in_session("swift-fox", "cas-src-young-raven-93");
    env.register_worker_in_session("swift-fox", "cas-src-vivid-sparrow-8");

    env.service
        .coordination(Parameters(coord_msg(
            "message",
            "all_workers",
            "stand down for the release gate",
            None,
        )))
        .await
        .expect("broadcast to my own workers");

    let for_b = env
        .prompt_queue()
        .peek_for_targets(
            &["noble-lynx-44", "swift-fox", "all_workers"],
            Some("cas-src-vivid-sparrow-8"),
            10,
        )
        .expect("peek for B");
    assert!(
        for_b.is_empty(),
        "an all_workers broadcast means THIS factory's workers; it must not reach B: {for_b:?}"
    );
}

// =============================================================================
// GH #699: two live supervisor sessions sharing one clone
// =============================================================================

/// The reported hazard: a second supervisor starts on the same checkout, and
/// `worker_status` shows the caller only itself under Supervisors while the
/// incumbent's fleet is one `reset`/`shutdown_workers` away from being reaped.
#[tokio::test]
async fn gh_699_worker_status_names_the_other_live_supervisor_on_this_clone() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "noble-koala-5"),
        ("CAS_FACTORY_SESSION", "gabber-gentle-hawk-71"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_supervisor_in_session("noble-koala-5", "gabber-gentle-hawk-71");
    env.register_supervisor_in_session("gentle-falcon-66", "gabber-witty-panda-98");

    let text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("worker_status"),
    );

    assert!(
        text.contains("gentle-falcon-66"),
        "the other live supervisor must be named, not filtered out by session scoping: {text}"
    );
    assert!(
        text.contains("live supervisors share this clone"),
        "the shared-clone hazard must be stated: {text}"
    );
    assert!(
        text.contains("reap the other's workers"),
        "the consequence must be stated: {text}"
    );
}

/// cas-5087: knowing another supervisor is live is only half the answer before
/// a gate. `worker_status` must name the epic each one is running — including
/// the one in the other session — or the operator still has to go ask.
#[tokio::test]
async fn cas_5087_worker_status_names_each_live_supervisors_epic() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "noble-koala-5"),
        ("CAS_FACTORY_SESSION", "gabber-gentle-hawk-71"),
    ]);
    let env = FactoryTestEnv::new();
    let mine = env.register_supervisor_in_session("noble-koala-5", "gabber-gentle-hawk-71");
    let theirs = env.register_supervisor_in_session("gentle-falcon-66", "gabber-witty-panda-98");

    let store = env.task_store();
    for (owner, title) in [
        (&mine, "EPIC: v3.15.4 update follow-ups"),
        (&theirs, "EPIC: hub transcript rewrite"),
    ] {
        let id = store.generate_id().expect("generate_id");
        let mut epic = Task::new(id, title.to_string());
        epic.task_type = TaskType::Epic;
        epic.status = TaskStatus::InProgress;
        epic.epic_verification_owner = Some(owner.clone());
        store.add(&epic).expect("add epic");
    }

    let text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("worker_status"),
    );

    assert!(
        text.contains("noble-koala-5") && text.contains("epic cas"),
        "the caller's own epic must be named: {text}"
    );
    assert!(
        text.contains("EPIC: v3.15.4 update follow-ups"),
        "the caller's epic title must be named: {text}"
    );
    assert!(
        text.contains("gabber-witty-panda-98/gentle-falcon-66"),
        "the other session's supervisor must still be named: {text}"
    );
    assert!(
        text.contains("EPIC: hub transcript rewrite"),
        "the OTHER supervisor's epic is the point of this row: {text}"
    );
}

/// A supervisor running nothing must render cleanly. An empty field would read
/// as a broken report, and this page is checked before destructive actions.
#[tokio::test]
async fn cas_5087_a_supervisor_with_no_epic_renders_cleanly() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "noble-koala-5"),
        ("CAS_FACTORY_SESSION", "gabber-gentle-hawk-71"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_supervisor_in_session("noble-koala-5", "gabber-gentle-hawk-71");

    let text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("worker_status"),
    );

    assert!(
        text.contains("noble-koala-5") && text.contains("no epic"),
        "an epic-less supervisor must say so rather than trail an empty field: {text}"
    );
    assert!(
        !text.contains("task store unreadable"),
        "a readable store with no epics is not an outage: {text}"
    );
}

/// The ordinary single-supervisor factory must stay quiet — a warning every
/// supervisor sees on every poll is a warning nobody reads.
#[tokio::test]
async fn gh_699_worker_status_stays_quiet_with_one_live_supervisor() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "noble-koala-5"),
        ("CAS_FACTORY_SESSION", "gabber-gentle-hawk-71"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_supervisor_in_session("noble-koala-5", "gabber-gentle-hawk-71");
    env.register_worker_in_session("zen-newt-93", "gabber-gentle-hawk-71");

    let text = get_text(
        &env.service
            .factory(Parameters(factory_req("worker_status")))
            .await
            .expect("worker_status"),
    );

    assert!(
        !text.contains("share this clone"),
        "one supervisor is not an overlap: {text}"
    );
}

/// Spawn preflight says it before the workers exist, rather than after one of
/// them is reaped by the other supervisor.
#[tokio::test]
async fn gh_699_spawn_preflight_warns_when_a_second_supervisor_shares_the_clone() {
    let _guard = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_AGENT_NAME", "noble-koala-5"),
        ("CAS_FACTORY_SESSION", "gabber-gentle-hawk-71"),
    ]);
    let env = FactoryTestEnv::new();
    env.register_supervisor_in_session("noble-koala-5", "gabber-gentle-hawk-71");
    env.register_supervisor_in_session("gentle-falcon-66", "gabber-witty-panda-98");

    let task_store = env.task_store();
    let task_id = task_store.generate_id().expect("generate_id");
    task_store
        .add(&Task::new(task_id.clone(), "Standalone".to_string()))
        .expect("add task");

    let mut req = factory_req("spawn_workers");
    req.worker_names = Some("swift-fox".to_string());
    req.task_id = Some(task_id);
    req.cli = Some("claude".to_string());

    let text = get_text(
        &env.service
            .factory(Parameters(req))
            .await
            .expect("spawn must still be allowed, only warned about"),
    );

    assert!(
        text.contains("SHARED-CLONE SUPERVISOR OVERLAP"),
        "the spawn receipt must name the overlap: {text}"
    );
    assert!(
        text.contains("gabber-witty-panda-98/gentle-falcon-66"),
        "the receipt must name the other supervisor session: {text}"
    );
    assert_eq!(
        env.spawn_queue().peek(10).expect("peek").len(),
        1,
        "the warning must not ground the spawn"
    );
}
