use super::*;
use cas_store::{PromptQueueStore, SqlitePromptQueueStore};
use std::cell::{Cell, RefCell};

pub(super) fn claim_fixture() -> Value {
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
    consumption_receipts: RefCell<Vec<Value>>,
    lose_consumption_response: Cell<bool>,
}
impl Relay {
    fn new(root: &std::path::Path) -> Self {
        Self {
            response: RefCell::new(claim_fixture()),
            acks: RefCell::new(vec![]),
            fail_ack: false,
            database: Some(root.join("cas.db")),
            ack_response: None,
            consumption_receipts: RefCell::new(vec![]),
            lose_consumption_response: Cell::new(false),
        }
    }
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
    fn record_consumption(
        &self,
        delivery: &str,
        request: &Value,
        _: Duration,
    ) -> Result<Value, RemoteError> {
        let db = rusqlite::Connection::open(self.database.as_ref().unwrap()).unwrap();
        let (frozen,occurrence,reader): (String,String,String) = db.query_row(
            "SELECT o.input_json,a.occurrence_id,a.reader_slot_id FROM violet_consumption_outbox o
             JOIN violet_admissions a ON a.delivery_id=o.delivery_id WHERE o.request_id=?1",
            [request["request_id"].as_str().unwrap()], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(
            frozen,
            request.to_string(),
            "remote mutation must follow frozen local intent"
        );
        self.consumption_receipts.borrow_mut().push(request.clone());
        if self.lose_consumption_response.replace(false) {
            return Err(RemoteError::timeout("lost consumption response"));
        }
        Ok(
            json!({"schema":CONTRACT_SCHEMA,"code":"completed","receipt":{
            "request_id":request["request_id"],"delivery":{"id":delivery,"occurrence_id":occurrence,
                "reader_slot_id":reader,"registration_id":request["actor"]["registration_id"],
                "delivery_generation":request["delivery_generation"],"status":"admitted","consumption":request["state"]}}}),
        )
    }
}
fn context() -> OperationContext {
    OperationContext {
        project_id: "github.com/richards-llc/cassy".into(),
        registration_id: "reg-supervisor-alpha-01".into(),
    }
}

fn poll_once(
    relay: &dyn ActivityDeliveryPort,
    store: &SqliteVioletAdmissionStore,
    context: &OperationContext,
    session: &str,
    recipient: &str,
) -> IntakeReport {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../../tests/fixtures/coordination_contract/v1/bindings--activation.json"
    ))
    .unwrap();
    let evidence =
        activation::validate_activation(&fixture["cases"][0]["response"]["payload"], context)
            .unwrap();
    super::poll_once(
        relay,
        store,
        context,
        session,
        recipient,
        "test-instance",
        &evidence,
        Instant::now() + TOTAL_REMOTE_BUDGET,
    )
}

#[test]
fn cas_c423_poll_arms_body_only_after_journaled_remote_consumption_receipt() {
    let root = tempfile::tempdir().unwrap();
    let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
    let relay = Relay::new(root.path());
    relay.lose_consumption_response.set(true);
    let report = poll_once(&relay, &store, &context(), "factory-a", "supervisor-a");
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("lost consumption response")),
        "{report:?}"
    );
    let queue = SqlitePromptQueueStore::open(root.path()).unwrap();
    assert!(
        queue
            .poll_unseen_for_recipient("supervisor-a", Some("factory-a"), 10)
            .unwrap()
            .is_empty()
    );
    relay.response.borrow_mut()["deliveries"] = json!([]);
    let db = rusqlite::Connection::open(root.path().join("cas.db")).unwrap();
    db.execute("UPDATE violet_consumption_outbox SET retry_at=0", [])
        .unwrap();
    let report = poll_once(&relay, &store, &context(), "factory-a", "supervisor-a");
    assert!(report.errors.is_empty(), "{report:?}");
    let receipts = relay.consumption_receipts.borrow();
    assert_eq!(receipts.len(), 2);
    assert_eq!(
        receipts[0], receipts[1],
        "lost response retries the same UUID and bytes"
    );
    drop(receipts);
    let body = queue
        .poll_unseen_for_recipient("supervisor-a", Some("factory-a"), 10)
        .unwrap();
    assert_eq!(
        body.len(),
        1,
        "background intake must confirm remote uncertainty before arming the original body"
    );
    let report = poll_once(&relay, &store, &context(), "factory-a", "supervisor-a");
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(
        relay.consumption_receipts.borrow().last().unwrap()["state"],
        "consumed"
    );
    assert!(
        queue
            .poll_unseen_for_recipient("supervisor-a", Some("factory-a"), 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn cas_c423_successor_poll_reuses_unacknowledged_admission_with_new_lease() {
    let root = tempfile::tempdir().unwrap();
    let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
    let mut response = claim_fixture();
    let original: LeasedDelivery =
        serde_json::from_value(response["deliveries"][0].clone()).unwrap();
    let first = store
        .admit(&admission_input(&original, &context(), "old-session", "old-reader").unwrap())
        .unwrap();
    let successor = OperationContext {
        registration_id: "reg-successor".into(),
        ..context()
    };
    response["deliveries"][0]["delivery"]["registration_id"] = json!(successor.registration_id);
    response["deliveries"][0]["lease_token"] = json!("successor-lease");
    let relay = Relay {
        response: RefCell::new(response),
        ..Relay::new(root.path())
    };
    let report = poll_once(&relay, &store, &successor, "new-session", "new-reader");
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!((report.admitted, report.created), (1, 0));
    let acks = relay.acks.borrow();
    assert_eq!(acks.len(), 1);
    assert_eq!(
        acks[0]["actor"]["registration_id"],
        successor.registration_id
    );
    assert_eq!(acks[0]["acks"][0]["lease_token"], "successor-lease");
    let queue = SqlitePromptQueueStore::open(root.path()).unwrap();
    let rows = queue.peek_all(20).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, first.prompt_id);
    assert!(
        store
            .pending_lease_fences(&context().project_id)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn cas_c423_poll_snapshots_lease_fence_before_remote_claim() {
    struct RacingRelay<'a> {
        store: &'a SqliteVioletAdmissionStore,
        old_response: Value,
        successor_input: VioletAdmissionInput,
    }
    impl ActivityDeliveryPort for RacingRelay<'_> {
        fn claim(&self, _: &Value, _: Duration) -> Result<Value, RemoteError> {
            // A second process takes over while the predecessor's request is in
            // flight. The predecessor then receives its delayed old lease.
            let fence = self
                .store
                .pending_lease_fences(&context().project_id)
                .unwrap();
            self.store
                .adopt_lease(&fence[0], &self.successor_input)
                .unwrap();
            Ok(self.old_response.clone())
        }
        fn acknowledge(&self, _: &Value, _: Duration) -> Result<Value, RemoteError> {
            panic!("the old actor must not acknowledge the successor's admission")
        }
        fn record_consumption(
            &self,
            _: &str,
            _: &Value,
            _: Duration,
        ) -> Result<Value, RemoteError> {
            panic!("old actor must not arm the successor's body")
        }
    }
    let root = tempfile::tempdir().unwrap();
    let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
    let old_response = claim_fixture();
    let old: LeasedDelivery =
        serde_json::from_value(old_response["deliveries"][0].clone()).unwrap();
    let mut successor_input =
        admission_input(&old, &context(), "old-session", "old-reader").unwrap();
    let first = store.admit(&successor_input).unwrap();
    successor_input.registration_id = "reg-successor".into();
    successor_input.factory_session = "new-session".into();
    successor_input.recipient = "new-reader".into();
    successor_input.lease_token = "successor-lease".into();
    let relay = RacingRelay {
        store: &store,
        old_response,
        successor_input,
    };
    let report = poll_once(&relay, &store, &context(), "old-session", "old-reader");
    assert_eq!(report.admitted, 0, "{report:?}");
    assert_eq!(report.errors.len(), 1, "{report:?}");
    assert!(
        store
            .pending_acknowledgements(&context().registration_id, &context().project_id)
            .unwrap()
            .is_empty()
    );
    let pending = store
        .pending_acknowledgements("reg-successor", &context().project_id)
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].lease_token, "successor-lease");
    let queue = SqlitePromptQueueStore::open(root.path()).unwrap();
    let rows = queue.peek_all(20).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, first.prompt_id);
}

#[test]
fn cas_c423_duplicate_poll_lost_ack_and_two_readers_keep_one_admission_each() {
    let root = tempfile::tempdir().unwrap();
    let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
    let relay = Relay {
        fail_ack: true,
        ..Relay::new(root.path())
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
    let relay = Relay::new(root.path());
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
        fail_ack: true,
        ..Relay::new(root.path())
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
        ..Relay::new(root.path())
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
        ack_response: Some(fixture["cases"][0]["response"]["payload"].clone()),
        ..Relay::new(root.path())
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
