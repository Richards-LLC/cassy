//! Public coordination-surface tests for the director worker-hold gate.
//!
//! Kept in a dedicated test process because session metadata is rooted under
//! `HOME`; changing that process-global variable inside the broad factory MCP
//! test binary would race unrelated Codex-availability tests.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use cas::mcp::{CasCore, CasService};
use cas::store::{init_cas_dir, open_agent_store};
use cas::types::Agent;
use cas_mcp::types::{CoordinationRequest, FactoryRequest};
use cas_types::AgentRole;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::RawContent;
use tempfile::TempDir;

struct TestEnv {
    _temp: TempDir,
    cas_root: PathBuf,
    service: CasService,
}

impl TestEnv {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let cas_root = init_cas_dir(temp.path()).unwrap();
        let core = CasCore::with_daemon(cas_root.clone(), None, None);
        core.set_agent_id_for_testing("hold-test-supervisor".to_string());
        let service = CasService::new(core, None);
        Self {
            _temp: temp,
            cas_root,
            service,
        }
    }

    fn register_worker(&self, name: &str, factory_session: &str) {
        let store = open_agent_store(&self.cas_root).unwrap();
        let mut agent = Agent::new(Agent::generate_fallback_id(), name.to_string());
        agent.role = AgentRole::Worker;
        agent.factory_session = Some(factory_session.to_string());
        store.register(&agent).unwrap();
    }
}

fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn set(vars: &[(&str, &str)]) -> Self {
        let lock = env_lock();
        let mut saved = Vec::new();
        for (key, value) in vars {
            let key = (*key).to_string();
            saved.push((key.clone(), std::env::var(&key).ok()));
            unsafe { std::env::set_var(key, value) };
        }
        Self { saved, _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}

fn request(action: &str, target: &str) -> CoordinationRequest {
    serde_json::from_value(serde_json::json!({
        "action": action,
        "target": target,
    }))
    .unwrap()
}

fn factory_request(action: &str) -> FactoryRequest {
    serde_json::from_value(serde_json::json!({ "action": action })).unwrap()
}

fn result_text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| match &content.raw {
            RawContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_metadata(env: &TestEnv, session: &str, workers: &[String]) -> PathBuf {
    let path = cas::ui::factory::metadata_path(session);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let metadata = cas::ui::factory::create_metadata(
        session,
        12345,
        "supervisor",
        workers,
        None,
        env.cas_root.parent().and_then(std::path::Path::to_str),
        None,
    );
    std::fs::write(&path, serde_json::to_string_pretty(&metadata).unwrap()).unwrap();
    path
}

#[tokio::test]
async fn hold_and_release_update_session_state_and_worker_status_cas_60dd() {
    let home = TempDir::new().unwrap();
    let session = "session-worker-hold";
    let worker = "lively-crow";
    let _guard = EnvGuard::set(&[
        ("HOME", home.path().to_str().unwrap()),
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_FACTORY_SESSION", session),
    ]);
    let env = TestEnv::new();
    env.register_worker(worker, session);
    let path = write_metadata(&env, session, &[worker.to_string()]);

    let result = env
        .service
        .coordination(Parameters(request("hold_worker", worker)))
        .await
        .expect("public hold action");
    assert!(result_text(&result).contains(worker));
    let held: cas::ui::factory::SessionMetadata =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(held.held_workers, vec![worker.to_string()]);

    let status = env
        .service
        .factory(Parameters(factory_request("worker_status")))
        .await
        .unwrap();
    let status = result_text(&status);
    assert!(status.contains(worker) && status.contains("[HELD]"), "{status}");

    env.service
        .coordination(Parameters(request("release_worker", worker)))
        .await
        .expect("public release action");
    let released: cas::ui::factory::SessionMetadata =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(released.held_workers.is_empty());
}

#[tokio::test]
async fn worker_cannot_change_hold_state_cas_60dd() {
    let home = TempDir::new().unwrap();
    let session = "session-worker-hold-policy";
    let worker = "lively-crow";
    let _guard = EnvGuard::set(&[
        ("HOME", home.path().to_str().unwrap()),
        ("CAS_AGENT_ROLE", "worker"),
        ("CAS_FACTORY_SESSION", session),
    ]);
    let env = TestEnv::new();
    env.register_worker(worker, session);
    let path = write_metadata(&env, session, &[worker.to_string()]);

    let error = env
        .service
        .coordination(Parameters(request("hold_worker", worker)))
        .await
        .expect_err("worker role must be rejected");
    assert!(error.to_string().contains("only supervisors"), "{error}");
    let metadata: cas::ui::factory::SessionMetadata =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(metadata.held_workers.is_empty());
}

#[tokio::test]
async fn supervisor_cannot_hold_foreign_session_worker_cas_60dd() {
    let home = TempDir::new().unwrap();
    let session = "session-worker-hold-scope";
    let worker = "foreign-crow";
    let _guard = EnvGuard::set(&[
        ("HOME", home.path().to_str().unwrap()),
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_FACTORY_SESSION", session),
    ]);
    let env = TestEnv::new();
    env.register_worker(worker, "another-factory-session");
    let path = write_metadata(&env, session, &[]);

    let error = env
        .service
        .coordination(Parameters(request("hold_worker", worker)))
        .await
        .expect_err("foreign-session worker must be rejected");
    assert!(error.to_string().contains("not a live member"), "{error}");
    let metadata: cas::ui::factory::SessionMetadata =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(metadata.held_workers.is_empty());
}
