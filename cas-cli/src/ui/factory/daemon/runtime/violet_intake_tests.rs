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
}
impl ActivityDeliveryPort for Relay {
    fn claim(&self, request: &Value, timeout: Duration) -> Result<Value, RemoteError> {
        assert_eq!(request["schema"], CONTRACT_SCHEMA);
        assert!(!timeout.is_zero() && timeout <= CALL_REMOTE_BUDGET);
        Ok(self.response.borrow().clone())
    }
    fn acknowledge(&self, request: &Value, _: Duration) -> Result<Value, RemoteError> {
        self.acks.borrow_mut().push(request.clone());
        if self.fail_ack {
            return Err(RemoteError::timeout("lost ack response"));
        }
        Ok(json!({"schema": CONTRACT_SCHEMA, "code": "completed", "results": []}))
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
