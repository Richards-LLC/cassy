use crate::support::*;
use cas::mcp::tools::{TaskReopenRequest, TaskUpdateRequest};
use cas::store::{
    open_agent_store, open_event_store, open_recording_store, open_supervisor_queue_store,
    open_task_store, open_verification_store,
};
use cas::types::{
    AgentRole, Dependency, DependencyType, Task, TaskStatus, Verification,
    VerificationDispatchState, VerificationProofBoundary, VerificationProvenance,
};
use chrono::{Duration, Utc};
use rmcp::handler::server::wrapper::Parameters;
use rusqlite::Connection;

struct EnvGuard(Vec<(&'static str, Option<String>)>);

impl EnvGuard {
    fn set(values: &[(&'static str, &'static str)]) -> Self {
        let previous = values
            .iter()
            .map(|(key, _)| (*key, std::env::var(key).ok()))
            .collect();
        for (key, value) in values {
            // SAFETY: callers hold support::env_test_lock for this guard's lifetime.
            unsafe { std::env::set_var(key, value) };
        }
        Self(previous)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..) {
            // SAFETY: callers hold support::env_test_lock for this guard's lifetime.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

struct ClosedFixture {
    temp: tempfile::TempDir,
    service: cas::mcp::CasCore,
    task_id: String,
    old_epic_id: String,
    new_epic_id: String,
    dispatch_id: String,
}

impl ClosedFixture {
    fn new(suffix: &str) -> Self {
        let (temp, service) = setup_cas();
        let cas_dir = temp.path().join(".cas");
        let task_store = open_task_store(&cas_dir).expect("task store");
        let agent_store = open_agent_store(&cas_dir).expect("agent store");
        let supervisor_id = format!("reopen-supervisor-{suffix}-{}", std::process::id());
        let mut supervisor = cas::types::Agent::new(
            supervisor_id.clone(),
            format!("reopen-supervisor-{suffix}"),
        );
        supervisor.role = AgentRole::Supervisor;
        supervisor.factory_session = Some("atomic-reopen-session".to_string());
        agent_store
            .register(&supervisor)
            .expect("register exact proof supervisor");
        open_supervisor_queue_store(&cas_dir).expect("supervisor outbox schema");
        open_event_store(&cas_dir).expect("event schema");
        open_recording_store(&cas_dir).expect("recording schema");

        let mut old_epic = Task::new(
            format!("cas-old-epic-{suffix}"),
            "Original parent epic".to_string(),
        );
        old_epic.task_type = cas::types::TaskType::Epic;
        task_store.add(&old_epic).expect("old epic");
        let mut new_epic = Task::new(
            format!("cas-new-epic-{suffix}"),
            "Replacement parent epic".to_string(),
        );
        new_epic.task_type = cas::types::TaskType::Epic;
        task_store.add(&new_epic).expect("new epic");

        let mut task = Task::new(
            format!("cas-reopen-{suffix}"),
            "Failure-atomic reopen".to_string(),
        );
        task.status = TaskStatus::Closed;
        task.closed_at = Some(Utc::now());
        task.close_reason = Some("first proof cycle complete".to_string());
        task.deliverables.factory_branch_anchor = Some("a".repeat(40));
        task_store.add(&task).expect("closed task");
        task_store
            .add_dependency(&Dependency {
                from_id: task.id.clone(),
                to_id: old_epic.id.clone(),
                dep_type: DependencyType::ParentChild,
                created_at: Utc::now(),
                created_by: Some("fixture".to_string()),
            })
            .expect("original parent");

        let dispatch = cas_store::create_verification_dispatch_bound(
            &cas_dir,
            &task.id,
            &supervisor_id,
            &supervisor_id,
            &VerificationProofBoundary::task(),
            Utc::now() + Duration::minutes(10),
            false,
        )
        .expect("proof dispatch");
        let verification_store = open_verification_store(&cas_dir).expect("verification store");
        let mut verdict = Verification::approved(
            format!("ver-reopen-{suffix}"),
            task.id.clone(),
            "approved first proof cycle".to_string(),
        );
        verdict.provenance = VerificationProvenance::SupervisorDirect;
        verdict.dispatch_id = Some(dispatch.id.clone());
        verdict.agent_id = Some(supervisor_id.clone());
        verdict.issuer_agent_id = Some(supervisor_id.clone());
        verification_store.add(&verdict).expect("durable verdict");
        let conn = Connection::open(cas_dir.join("cas.db")).expect("db");
        cas_store::resolve_verification_dispatch_with_conn(
            &conn,
            &dispatch.id,
            &supervisor_id,
            None,
            true,
        )
        .expect("resolved proof");

        Self {
            temp,
            service,
            task_id: task.id,
            old_epic_id: old_epic.id,
            new_epic_id: new_epic.id,
            dispatch_id: dispatch.id,
        }
    }

    fn cas_dir(&self) -> std::path::PathBuf {
        self.temp.path().join(".cas")
    }

    fn update_request(&self) -> TaskUpdateRequest {
        TaskUpdateRequest {
            blocked_by: None,
            id: self.task_id.clone(),
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
            status: Some("open".to_string()),
            epic: Some(self.new_epic_id.clone()),
            epic_verification_owner: None,
            depth: None,
        }
    }
}

fn rows(conn: &Connection, sql: &str) -> Vec<Vec<String>> {
    let mut stmt = conn.prepare(sql).expect("snapshot statement");
    let columns = stmt.column_count();
    stmt.query_map([], |row| {
        (0..columns)
            .map(|index| match row.get_ref(index)? {
                rusqlite::types::ValueRef::Null => Ok("<null>".to_string()),
                rusqlite::types::ValueRef::Integer(value) => Ok(value.to_string()),
                rusqlite::types::ValueRef::Real(value) => Ok(value.to_string()),
                rusqlite::types::ValueRef::Text(value) => {
                    Ok(String::from_utf8_lossy(value).into_owned())
                }
                rusqlite::types::ValueRef::Blob(value) => Ok(format!("{value:?}")),
            })
            .collect::<rusqlite::Result<Vec<_>>>()
    })
    .expect("snapshot query")
    .collect::<rusqlite::Result<Vec<_>>>()
    .expect("snapshot rows")
}

fn snapshot(fixture: &ClosedFixture) -> Vec<Vec<Vec<String>>> {
    let conn = Connection::open(fixture.cas_dir().join("cas.db")).expect("snapshot db");
    let id = fixture.task_id.replace("'", "''");
    vec![
        rows(
            &conn,
            &format!("SELECT * FROM tasks WHERE id = '{id}' ORDER BY id"),
        ),
        rows(
            &conn,
            &format!("SELECT * FROM dependencies WHERE from_id = '{id}' ORDER BY to_id"),
        ),
        rows(
            &conn,
            &format!("SELECT * FROM verification_dispatches WHERE task_id = '{id}' ORDER BY id"),
        ),
        rows(
            &conn,
            &format!("SELECT * FROM events WHERE entity_id = '{id}' ORDER BY id"),
        ),
        rows(
            &conn,
            &format!(
                "SELECT supervisor_id,event_type,payload,priority,transition_key,prompt_delivered_at \
                 FROM supervisor_queue WHERE payload LIKE '%{id}%' ORDER BY id"
            ),
        ),
    ]
}

fn install_failure(conn: &Connection, name: &str, body: &str) {
    conn.execute_batch(&format!(
        "CREATE TRIGGER {name} {body} BEGIN SELECT RAISE(FAIL, 'forced {name}'); END;"
    ))
    .expect("failure trigger");
}

#[tokio::test]
async fn update_reopen_rolls_back_every_later_write_and_retries_idempotently() {
    let fixture = ClosedFixture::new("update");
    let _env_lock = env_test_lock();
    let _env = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_FACTORY_SESSION", "atomic-reopen-session"),
    ]);
    let before = snapshot(&fixture);
    let db_path = fixture.cas_dir().join("cas.db");

    let failures = [
        (
            "fail_reopen_dispatch",
            format!(
                "BEFORE UPDATE ON verification_dispatches WHEN OLD.id = '{}'",
                fixture.dispatch_id
            ),
        ),
        (
            "fail_reopen_task",
            format!("BEFORE UPDATE ON tasks WHEN OLD.id = '{}'", fixture.task_id),
        ),
        (
            "fail_reopen_dependency",
            format!(
                "BEFORE DELETE ON dependencies WHEN OLD.from_id = '{}'",
                fixture.task_id
            ),
        ),
        (
            "fail_reopen_event",
            format!(
                "BEFORE INSERT ON events WHEN NEW.entity_id = '{}'",
                fixture.task_id
            ),
        ),
        (
            "fail_reopen_outbox",
            "BEFORE INSERT ON supervisor_queue WHEN NEW.event_type = 'task_lifecycle'".to_string(),
        ),
    ];

    for (name, body) in failures {
        let conn = Connection::open(&db_path).expect("trigger db");
        install_failure(&conn, name, &body);
        let result = fixture
            .service
            .cas_task_update(Parameters(fixture.update_request()))
            .await;
        assert!(result.is_err(), "{name} must surface");
        conn.execute_batch(&format!("DROP TRIGGER {name};"))
            .expect("drop trigger");
        assert_eq!(snapshot(&fixture), before, "{name} crossed atomic boundary");
    }

    fixture
        .service
        .cas_task_update(Parameters(fixture.update_request()))
        .await
        .expect("retry after failures");
    let after = snapshot(&fixture);
    let task_store = open_task_store(&fixture.cas_dir()).expect("task store");
    let task = task_store.get(&fixture.task_id).expect("reopened task");
    assert_eq!(task.status, TaskStatus::Open);
    assert!(task.closed_at.is_none());
    assert!(task.deliverables.factory_branch_anchor.is_none());
    let parents = task_store
        .get_dependencies(&fixture.task_id)
        .expect("parents");
    assert_eq!(parents.len(), 1);
    assert_eq!(parents[0].to_id, fixture.new_epic_id);
    assert_eq!(
        cas_store::get_verification_dispatch(&fixture.cas_dir(), &fixture.dispatch_id)
            .expect("dispatch")
            .state,
        VerificationDispatchState::Invalidated
    );

    let retry = fixture
        .service
        .cas_task_update(Parameters(fixture.update_request()))
        .await
        .expect("exact update retry");
    assert!(extract_text(retry).contains("No changes specified"));
    assert_eq!(
        snapshot(&fixture),
        after,
        "exact retry mutated durable state"
    );
}

#[tokio::test]
async fn dedicated_reopen_is_failure_atomic_and_exact_retry_is_a_noop() {
    let fixture = ClosedFixture::new("dedicated");
    let _env_lock = env_test_lock();
    let _env = EnvGuard::set(&[
        ("CAS_AGENT_ROLE", "supervisor"),
        ("CAS_FACTORY_SESSION", "atomic-reopen-session"),
    ]);
    let before = snapshot(&fixture);
    let db_path = fixture.cas_dir().join("cas.db");
    let conn = Connection::open(&db_path).expect("trigger db");
    install_failure(
        &conn,
        "fail_dedicated_outbox",
        "BEFORE INSERT ON supervisor_queue WHEN NEW.event_type = 'task_lifecycle'",
    );
    let request = || TaskReopenRequest {
        id: fixture.task_id.clone(),
        reason: Some("fresh work requested".to_string()),
    };
    let failed = fixture.service.cas_task_reopen(Parameters(request())).await;
    assert!(failed.is_err());
    conn.execute_batch("DROP TRIGGER fail_dedicated_outbox;")
        .expect("drop trigger");
    assert_eq!(snapshot(&fixture), before);

    fixture
        .service
        .cas_task_reopen(Parameters(request()))
        .await
        .expect("dedicated retry succeeds");
    let after = snapshot(&fixture);
    let retry = fixture
        .service
        .cas_task_reopen(Parameters(request()))
        .await
        .expect("exact dedicated retry");
    assert!(extract_text(retry).contains("idempotently"));
    assert_eq!(
        snapshot(&fixture),
        after,
        "dedicated retry mutated durable state"
    );
    assert_eq!(
        open_task_store(&fixture.cas_dir())
            .expect("task store")
            .get_dependencies(&fixture.task_id)
            .expect("parent")
            .first()
            .map(|dep| dep.to_id.as_str()),
        Some(fixture.old_epic_id.as_str()),
        "dedicated reopen must preserve dependencies"
    );
}
