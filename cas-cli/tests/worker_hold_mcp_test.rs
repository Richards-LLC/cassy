//! Public factory-surface tests for the director worker-hold gate.
//!
//! Session metadata is rooted under HOME. Use the grouped harness's canonical
//! environment guard so these fixtures cannot race messaging/status fixtures.

use std::path::PathBuf;
use crate::test_env_guard::TestEnvGuard;

use cas::mcp::{CasCore, CasService};
use cas::store::{ReminderTriggerType, init_cas_dir, open_agent_store, open_reminder_store};
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

    fn register_worker(&self, name: &str, factory_session: &str) -> String {
        let store = open_agent_store(&self.cas_root).unwrap();
        let mut agent = Agent::new(Agent::generate_fallback_id(), name.to_string());
        agent.role = AgentRole::Worker;
        agent.factory_session = Some(factory_session.to_string());
        store.register(&agent).unwrap();
        agent.id
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
    let mut guard = TestEnvGuard::temp_home();
    let session = "session-worker-hold";
    let worker = "lively-crow";
    guard.set("CAS_AGENT_ROLE", "supervisor");
    guard.set("CAS_FACTORY_SESSION", session);
    let env = TestEnv::new();
    let worker_id = env.register_worker(worker, session);
    let reminders = open_reminder_store(&env.cas_root).unwrap();
    reminders
        .create(
            "hold-test-supervisor",
            Some(&worker_id),
            "watch worker after hold",
            ReminderTriggerType::Time,
            Some(chrono::Utc::now() + chrono::Duration::hours(1)),
            None,
            None,
            3600,
            Some(session),
        )
        .unwrap();
    assert_eq!(reminders.list_pending_for_target(&worker_id).unwrap().len(), 1);

    // cas-c5ab: before the session metadata exists the hold cannot be
    // persisted, and the failed hold changes no reminder.
    env.service
        .factory(Parameters(request("hold_worker", worker)))
        .await
        .expect_err("an unpersisted hold fails");
    assert_eq!(reminders.list_pending_for_target(&worker_id).unwrap().len(), 1);
    let path = write_metadata(&env, session, &[worker.to_string()]);

    // cas-5f4b: when suspending fails after the hold is saved, the hold is
    // rolled back and the reminder stays pending.
    let db = rusqlite::Connection::open(env.cas_root.join("cas.db")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER refuse_suspend BEFORE UPDATE OF status ON reminders \
         WHEN NEW.status = 'suspended' BEGIN SELECT RAISE(ABORT, 'suspend refused'); END;",
    )
    .unwrap();
    env.service
        .factory(Parameters(request("hold_worker", worker)))
        .await
        .expect_err("a failed suspend fails the hold");
    let rolled_back: cas::ui::factory::SessionMetadata =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(rolled_back.held_workers.is_empty());
    assert_eq!(reminders.list_pending_for_target(&worker_id).unwrap().len(), 1);
    db.execute_batch("DROP TRIGGER refuse_suspend;").unwrap();

    let result = env
        .service
        .factory(Parameters(request("hold_worker", worker)))
        .await
        .expect("public hold action");
    let hold_result = result_text(&result);
    assert!(hold_result.contains(worker));
    assert!(reminders.list_pending_for_target(&worker_id).unwrap().is_empty());
    // cas-c5ab: the suspended reminder is still listed, labelled held.
    let listed = env
        .service
        .coordination(Parameters(serde_json::from_value(serde_json::json!({"action": "remind_list"})).unwrap()))
        .await
        .expect("remind_list");
    let listed = result_text(&listed);
    assert!(listed.contains("watch worker after hold") && listed.contains("[held"), "{listed}");
    let held: cas::ui::factory::SessionMetadata =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(held.held_workers, vec![worker.to_string()]);

    let status = env
        .service
        .factory_request(Parameters(factory_request("worker_status")))
        .await
        .unwrap();
    let status = result_text(&status);
    assert!(status.contains(worker) && status.contains("[HELD]"), "{status}");

    env.service
        .factory(Parameters(request("release_worker", worker)))
        .await
        .expect("public release action");
    let released: cas::ui::factory::SessionMetadata =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(released.held_workers.is_empty());
    // cas-73b89: release re-arms the suspended reminder.
    assert_eq!(reminders.list_pending_for_target(&worker_id).unwrap().len(), 1);
}


#[tokio::test]
async fn worker_cannot_change_hold_state_cas_60dd() {
    let mut guard = TestEnvGuard::temp_home();
    let session = "session-worker-hold-policy";
    let worker = "lively-crow";
    guard.set("CAS_AGENT_ROLE", "worker");
    guard.set("CAS_FACTORY_SESSION", session);
    let env = TestEnv::new();
    env.register_worker(worker, session);
    let path = write_metadata(&env, session, &[worker.to_string()]);

    let error = env
        .service
        .factory(Parameters(request("hold_worker", worker)))
        .await
        .expect_err("worker role must be rejected");
    assert!(error.to_string().contains("only supervisors"), "{error}");
    let metadata: cas::ui::factory::SessionMetadata =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(metadata.held_workers.is_empty());
}

#[tokio::test]
async fn supervisor_cannot_hold_foreign_session_worker_cas_60dd() {
    let mut guard = TestEnvGuard::temp_home();
    let session = "session-worker-hold-scope";
    let worker = "foreign-crow";
    guard.set("CAS_AGENT_ROLE", "supervisor");
    guard.set("CAS_FACTORY_SESSION", session);
    let env = TestEnv::new();
    env.register_worker(worker, "another-factory-session");
    let path = write_metadata(&env, session, &[]);

    let error = env
        .service
        .factory(Parameters(request("hold_worker", worker)))
        .await
        .expect_err("foreign-session worker must be rejected");
    assert!(error.to_string().contains("not a live member"), "{error}");
    let metadata: cas::ui::factory::SessionMetadata =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(metadata.held_workers.is_empty());
}
