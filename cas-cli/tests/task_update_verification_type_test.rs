use std::path::Path;

use cas::mcp::{CasCore, CasService};
use cas::store::{init_cas_dir, open_task_store, open_verification_store};
use cas::types::{
    Task, TaskStatus, TaskType, Verification, VerificationProofBoundary, VerificationProvenance,
    VerificationType,
};
use cas_mcp::types::TaskRequest;
use rmcp::handler::server::wrapper::Parameters;
use tempfile::TempDir;

fn task_request(value: serde_json::Value) -> TaskRequest {
    serde_json::from_value(value).expect("valid public task request")
}

fn add_task(cas_root: &Path, id: &str, task_type: TaskType) -> Task {
    let task_store = open_task_store(cas_root).expect("task store");
    let mut task = Task::new(id.to_string(), format!("{task_type} verification boundary"));
    task.status = TaskStatus::InProgress;
    task.task_type = task_type;
    task_store.add(&task).expect("add task");
    task
}

fn add_exact_verdict(cas_root: &Path, task_id: &str, verification_type: VerificationType) {
    let dispatch = cas_store::create_verification_dispatch_bound(
        cas_root,
        task_id,
        "requester",
        "supervisor",
        &VerificationProofBoundary::task(),
        chrono::Utc::now() + chrono::Duration::minutes(10),
        false,
    )
    .expect("exact dispatch");
    let mut verdict = Verification::approved(
        format!("verdict-{task_id}"),
        task_id.to_string(),
        "approved exact proof".to_string(),
    );
    verdict.verification_type = verification_type;
    verdict.provenance = VerificationProvenance::SupervisorDirect;
    verdict.dispatch_id = Some(dispatch.id.clone());
    open_verification_store(cas_root)
        .unwrap()
        .add(&verdict)
        .expect("add exact verdict");
    let connection = rusqlite::Connection::open(cas_root.join("cas.db")).unwrap();
    cas_store::resolve_verification_dispatch_with_conn(
        &connection,
        &dispatch.id,
        "supervisor",
        None,
        true,
    )
    .expect("resolve exact dispatch");
}

fn add_legacy_verdict(cas_root: &Path, task_id: &str, verification_type: VerificationType) {
    let mut verdict = Verification::approved(
        format!("legacy-{task_id}"),
        task_id.to_string(),
        "approved legacy proof".to_string(),
    );
    verdict.verification_type = verification_type;
    verdict.provenance = VerificationProvenance::Legacy;
    open_verification_store(cas_root)
        .unwrap()
        .add(&verdict)
        .expect("add legacy verdict");
}

fn durable_snapshot(cas_root: &Path) -> Vec<(String, Vec<Vec<String>>)> {
    const TABLES: &[&str] = &[
        "tasks",
        "worker_completion_receipts",
        "worker_delivery_transactions",
        "worker_delivery_events",
        "verification_dispatches",
        "verifications",
        "events",
        "supervisor_queue",
        "prompt_queue",
    ];
    let connection = rusqlite::Connection::open(cas_root.join("cas.db")).unwrap();
    TABLES
        .iter()
        .map(|table| {
            let exists = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                    [table],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap();
            if !exists {
                return ((*table).to_string(), Vec::new());
            }
            let mut statement = connection
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .unwrap();
            let column_count = statement.column_count();
            let rows = statement
                .query_map([], |row| {
                    (0..column_count)
                        .map(|index| {
                            use rusqlite::types::ValueRef;
                            Ok(match row.get_ref(index)? {
                                ValueRef::Null => "NULL".to_string(),
                                ValueRef::Integer(value) => value.to_string(),
                                ValueRef::Real(value) => value.to_string(),
                                ValueRef::Text(value) => {
                                    String::from_utf8_lossy(value).into_owned()
                                }
                                ValueRef::Blob(value) => format!("{value:?}"),
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

async fn update_to_closed(service: &CasService, task_id: &str) -> Result<(), String> {
    service
        .task(Parameters(task_request(serde_json::json!({
            "action": "update",
            "id": task_id,
            "status": "closed"
        }))))
        .await
        .map(|_| ())
        .map_err(|error| error.message.to_string())
}

#[tokio::test]
async fn direct_update_to_closed_requires_the_task_types_verification_type() {
    let root = TempDir::new().expect("temporary CAS root");
    let cas_root = init_cas_dir(root.path()).expect("initialize CAS");
    std::fs::write(
        cas_root.join("config.toml"),
        "[verification]\nenabled = true\n[worktrees]\nenabled = false\n",
    )
    .unwrap();
    let service = CasService::new(CasCore::with_daemon(cas_root.clone(), None, None), None);
    let task_store = open_task_store(&cas_root).expect("task store");

    let wrong_exact = add_task(&cas_root, "cas-epic-wrong-exact", TaskType::Epic);
    add_exact_verdict(&cas_root, &wrong_exact.id, VerificationType::Task);
    let before_wrong_exact = durable_snapshot(&cas_root);
    let error = update_to_closed(&service, &wrong_exact.id)
        .await
        .expect_err("a Task verdict must not close an Epic");
    assert!(error.contains("VERIFICATION REQUIRED"), "{error}");
    assert_eq!(durable_snapshot(&cas_root), before_wrong_exact);
    assert_eq!(
        task_store.get(&wrong_exact.id).unwrap().status,
        TaskStatus::InProgress
    );

    let correct_epic = add_task(&cas_root, "cas-epic-correct-exact", TaskType::Epic);
    add_exact_verdict(&cas_root, &correct_epic.id, VerificationType::Epic);
    update_to_closed(&service, &correct_epic.id)
        .await
        .expect("an exact Epic verdict closes an Epic");
    assert_eq!(
        task_store.get(&correct_epic.id).unwrap().status,
        TaskStatus::Closed
    );

    let correct_task = add_task(&cas_root, "cas-task-correct-exact", TaskType::Task);
    add_exact_verdict(&cas_root, &correct_task.id, VerificationType::Task);
    update_to_closed(&service, &correct_task.id)
        .await
        .expect("an exact Task verdict closes a Task");
    assert_eq!(
        task_store.get(&correct_task.id).unwrap().status,
        TaskStatus::Closed
    );

    let wrong_legacy = add_task(&cas_root, "cas-epic-wrong-legacy", TaskType::Epic);
    add_legacy_verdict(&cas_root, &wrong_legacy.id, VerificationType::Task);
    let before_wrong_legacy = durable_snapshot(&cas_root);
    let error = update_to_closed(&service, &wrong_legacy.id)
        .await
        .expect_err("a legacy Task verdict must not close an Epic");
    assert!(error.contains("VERIFICATION REQUIRED"), "{error}");
    assert_eq!(durable_snapshot(&cas_root), before_wrong_legacy);
    assert_eq!(
        task_store.get(&wrong_legacy.id).unwrap().status,
        TaskStatus::InProgress
    );
}
