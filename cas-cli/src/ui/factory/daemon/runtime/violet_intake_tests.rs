use super::*;
use cas_store::{PromptQueueStore, SqlitePromptQueueStore};
use std::cell::RefCell;

fn claim_fixture() -> Value {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../../tests/fixtures/coordination_contract/v1/violet_activity--claim_v2.json"
    ))
    .unwrap();
    fixture["cases"][0]["response"]["payload"].clone()
}

struct Relay {
    response: RefCell<Value>,
    acks: RefCell<Vec<Value>>,
    fail_ack: bool,
    database: Option<std::path::PathBuf>,
    ack_response: Option<Value>,
}
impl ActivityDeliveryPort for Relay {
    fn claim(&self, request: &Value, timeout: Duration) -> Result<Value, RemoteError> {
        assert_eq!(request["schema"], CONTRACT_SCHEMA);
        assert!(!timeout.is_zero() && timeout <= CALL_REMOTE_BUDGET);
        Ok(self.response.borrow().clone())
    }
    fn acknowledge(&self, request: &Value, _: Duration) -> Result<Value, RemoteError> {
        if let Some(path) = &self.database {
            let db = rusqlite::Connection::open(path).unwrap();
            for ack in request["acks"].as_array().unwrap() {
                let (admissions, cursors): (i64, i64) = db.query_row(
                    "SELECT (SELECT COUNT(*) FROM violet_admissions a JOIN prompt_queue q ON q.id=a.prompt_id
                     WHERE a.delivery_id=?1), (SELECT COUNT(*) FROM violet_admission_cursors)",
                    [ack["delivery_id"].as_str().unwrap()], |row| Ok((row.get(0)?, row.get(1)?))
                ).unwrap();
                assert_eq!(
                    admissions, 1,
                    "ack must follow actual queue+admission commit"
                );
                assert!(cursors > 0, "cursor must be durable before ack");
            }
        }
        self.acks.borrow_mut().push(request.clone());
        if self.fail_ack {
            return Err(RemoteError::timeout("lost ack response"));
        }
        Ok(self.ack_response.clone().unwrap_or_else(|| json!({
            "schema": CONTRACT_SCHEMA, "code": "completed",
            "results": request["acks"].as_array().unwrap().iter().map(|ack|
                json!({"delivery_id": ack["delivery_id"], "code": "completed", "status": "admitted"})
            ).collect::<Vec<_>>()
        })))
    }
}
fn context() -> OperationContext {
    OperationContext {
        project_id: "github.com/richards-llc/cassy".into(),
        registration_id: "reg-supervisor-alpha-01".into(),
    }
}

#[test]
fn cas_c423_duplicate_poll_lost_ack_and_two_readers_keep_one_admission_each() {
    let root = tempfile::tempdir().unwrap();
    let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
    let relay = Relay {
        response: RefCell::new(claim_fixture()),
        acks: RefCell::new(vec![]),
        fail_ack: true,
        database: Some(root.path().join("cas.db")),
        ack_response: None,
    };
    let first = poll_once(&relay, &store, &context(), "factory-a", "supervisor-a");
    assert_eq!(first.admitted, 1, "{first:?}");
    assert_eq!(first.created, 1);
    assert_eq!(relay.acks.borrow().len(), 1);
    drop(store); // crash after durable admission, before a successful ack
    let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
    let retry = poll_once(&relay, &store, &context(), "factory-a", "supervisor-a");
    assert_eq!(retry.admitted, 1);
    assert_eq!(retry.created, 0);
    let mut second = context();
    second.registration_id = "reg-supervisor-beta-07".into();
    {
        let mut response = relay.response.borrow_mut();
        let row = &mut response["deliveries"][0]["delivery"];
        row["id"] = json!("0000de2b-1111-4111-8111-00000000de2b");
        row["reader_slot_id"] = json!("b2b2b2b2-0000-4000-8000-00000000000b");
        row["registration_id"] = json!(second.registration_id);
        row["route_kind"] = json!("unclaimed_fanout");
        row["route_generation"] = json!(0);
    }
    assert_eq!(
        poll_once(&relay, &store, &second, "factory-b", "supervisor-b").created,
        1
    );
    let queue = SqlitePromptQueueStore::open(root.path()).unwrap();
    assert_eq!(queue.peek_all(20).unwrap().len(), 2);
    let db = rusqlite::Connection::open(root.path().join("cas.db")).unwrap();
    let counts: (i64, i64) = db
        .query_row(
            "SELECT (SELECT COUNT(*) FROM violet_admissions), \
        (SELECT COUNT(*) FROM violet_admission_cursors)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(counts, (2, 2));
}

#[test]
fn cas_c423_failed_sqlite_cursor_commit_never_acks_or_leaks_an_admission() {
    let root = tempfile::tempdir().unwrap();
    let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
    let db = rusqlite::Connection::open(root.path().join("cas.db")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail_cursor BEFORE INSERT ON violet_admission_cursors
        BEGIN SELECT RAISE(ABORT, 'disk-full simulation'); END;",
    )
    .unwrap();
    let relay = Relay {
        response: RefCell::new(claim_fixture()),
        acks: RefCell::new(vec![]),
        fail_ack: false,
        database: Some(root.path().join("cas.db")),
        ack_response: None,
    };
    let result = poll_once(&relay, &store, &context(), "factory-a", "supervisor-a");
    assert_eq!(result.admitted, 0);
    assert!(
        relay.acks.borrow().is_empty(),
        "failed commit cannot be acknowledged"
    );
    assert!(
        result
            .errors
            .iter()
            .any(|error| error.contains("disk-full simulation")),
        "{result:?}"
    );
    let counts: (i64, i64, i64) = db
        .query_row(
            "SELECT (SELECT COUNT(*) FROM prompt_queue),
        (SELECT COUNT(*) FROM violet_admissions), (SELECT COUNT(*) FROM violet_admission_cursors)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        counts,
        (0, 0, 0),
        "one SQLite transaction must roll back all three writes"
    );
}

#[test]
fn cas_c423_ack_retries_durable_admission_when_cloud_no_longer_leases_it() {
    let root = tempfile::tempdir().unwrap();
    let relay = Relay {
        response: RefCell::new(claim_fixture()),
        acks: RefCell::new(vec![]),
        fail_ack: true,
        database: Some(root.path().join("cas.db")),
        ack_response: None,
    };
    let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
    let first = poll_once(&relay, &store, &context(), "factory-a", "supervisor-a");
    assert_eq!(first.created, 1);
    drop(store);
    // Cloud committed the first ack, then its response was lost. Admitted rows
    // are no longer claimable, but the local pending ack still needs settlement.
    let mut empty = claim_fixture();
    empty["deliveries"] = json!([]);
    let relay = Relay {
        response: RefCell::new(empty),
        acks: RefCell::new(vec![]),
        fail_ack: false,
        database: Some(root.path().join("cas.db")),
        ack_response: None,
    };
    let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
    let resumed = poll_once(&relay, &store, &context(), "factory-a", "supervisor-a");
    assert!(resumed.errors.is_empty(), "{resumed:?}");
    assert_eq!(
        relay.acks.borrow().len(),
        1,
        "retry durable pending ack even without a fresh lease"
    );
    let db = rusqlite::Connection::open(root.path().join("cas.db")).unwrap();
    let (pending, prompts): (i64,i64) = db.query_row(
        "SELECT (SELECT SUM(ack_pending) FROM violet_admissions), (SELECT COUNT(*) FROM prompt_queue)",
        [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!((pending, prompts), (0, 1));
    poll_once(&relay, &store, &context(), "factory-a", "supervisor-a");
    assert_eq!(
        relay.acks.borrow().len(),
        1,
        "settled acknowledgement stops retrying"
    );
}

#[test]
fn cas_c423_ack_checks_each_delivery_result_and_retains_failures() {
    let root = tempfile::tempdir().unwrap();
    let mut claim = claim_fixture();
    let mut second = claim["deliveries"][0].clone();
    second["delivery"]["id"] = json!("0000de1b-1111-4111-8111-00000000de1b");
    second["delivery"]["occurrence_id"] = json!("another-occurrence");
    claim["deliveries"].as_array_mut().unwrap().push(second);
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../../tests/fixtures/coordination_contract/v1/violet_activity--ack_v2.json"
    ))
    .unwrap();
    let relay = Relay {
        response: RefCell::new(claim),
        acks: RefCell::new(vec![]),
        fail_ack: false,
        database: Some(root.path().join("cas.db")),
        ack_response: Some(fixture["cases"][0]["response"]["payload"].clone()),
    };
    let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
    let report = poll_once(&relay, &store, &context(), "factory-a", "supervisor-a");
    assert_eq!(report.created, 2);
    let db = rusqlite::Connection::open(root.path().join("cas.db")).unwrap();
    let mut statement = db
        .prepare("SELECT delivery_id,ack_pending FROM violet_admissions ORDER BY delivery_id")
        .unwrap();
    let rows: Vec<(String, i64)> = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            ("0000de1b-1111-4111-8111-00000000de1b".into(), 1),
            ("0000de2a-1111-4111-8111-00000000de2a".into(), 0)
        ]
    );
    assert!(
        report.errors.iter().any(|e| e.contains("stale_generation")),
        "{report:?}"
    );
}
