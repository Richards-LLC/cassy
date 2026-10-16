//! Known-admission recovery. Discovery on a fresh machine needs Cloud #167.
use super::*;
use crate::coordination::external_work::{CloudCoordinationPort, SqliteRequestJournal};
use cas_store::{VioletAdmissionRebind, VioletAdmissionReceipt};

pub(crate) struct KnownRebind {
    pub request_id: String,
    pub delivery_id: String,
    pub occurrence_id: String,
    pub reader_slot_id: String,
    pub expected_generation: i64,
    pub factory_session: String,
    pub recipient: String,
}

pub(crate) fn rebind_known_unread(
    _cloud: &dyn CloudCoordinationPort,
    _journal: &SqliteRequestJournal,
    _store: &SqliteVioletAdmissionStore,
    _context: &OperationContext,
    _request: &KnownRebind,
) -> Result<VioletAdmissionReceipt, String> {
    Err("known rebind is not implemented".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };

    fn fixture() -> Value {
        let value: Value = serde_json::from_str(include_str!(
            "../../../../../tests/fixtures/coordination_contract/v1/deliveries--rebind.json"
        ))
        .unwrap();
        value["cases"][0]["response"]["payload"].clone()
    }

    struct Cloud {
        response: Value,
        db: std::path::PathBuf,
        lose_response: AtomicBool,
        posts: Mutex<Vec<Value>>,
        lookups: Mutex<Vec<String>>,
    }
    impl CloudCoordinationPort for Cloud {
        fn post(&self, path: &str, body: &Value, timeout: Duration) -> Result<Value, RemoteError> {
            assert!(
                path.ends_with(&format!(
                    "/{}/rebind",
                    body["delivery_id"]
                        .as_str()
                        .unwrap_or("0000de1b-1111-4111-8111-00000000de1b")
                ))
            );
            assert!(!timeout.is_zero() && timeout <= CALL_REMOTE_BUDGET);
            let db = rusqlite::Connection::open(&self.db).unwrap();
            let (stage, wire): (String, Option<String>) = db
                .query_row(
                    "SELECT stage,wire_json FROM external_work_operations WHERE request_id=?1",
                    [body["request_id"].as_str().unwrap()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(stage, "dispatching", "journal must precede Cloud effect");
            assert_eq!(
                serde_json::from_str::<Value>(&wire.unwrap()).unwrap(),
                *body
            );
            self.posts.lock().unwrap().push(body.clone());
            if self.lose_response.swap(false, Ordering::SeqCst) {
                Err(RemoteError::timeout("rebind committed, response lost"))
            } else {
                Ok(self.response.clone())
            }
        }
        fn lookup(
            &self,
            id: &str,
            registration: &str,
            timeout: Duration,
        ) -> Result<Value, RemoteError> {
            assert_eq!(
                registration,
                self.response["receipt"]["delivery"]["registration_id"]
            );
            assert!(!timeout.is_zero() && timeout <= CALL_REMOTE_BUDGET);
            self.lookups.lock().unwrap().push(id.into());
            Ok(self.response.clone())
        }
    }

    fn setup(root: &std::path::Path) -> (KnownRebind, OperationContext, i64) {
        let response = fixture();
        let d = &response["receipt"]["delivery"];
        let mut old = super::super::tests::claim_fixture();
        let delivery = &mut old["deliveries"][0]["delivery"];
        for key in [
            "id",
            "occurrence_id",
            "reader_slot_id",
            "route_kind",
            "route_generation",
        ] {
            delivery[key] = d[key].clone();
        }
        delivery["registration_id"] = json!("reg-predecessor");
        let old_context = OperationContext {
            project_id: "github.com/richards-llc/cassy".into(),
            registration_id: "reg-predecessor".into(),
        };
        let leased: LeasedDelivery = serde_json::from_value(old["deliveries"][0].clone()).unwrap();
        let input =
            admission_input(&leased, &old_context, "old-factory", "old-supervisor").unwrap();
        let first = SqliteVioletAdmissionStore::open(root)
            .unwrap()
            .admit(&input)
            .unwrap();
        (
            KnownRebind {
                request_id: response["receipt"]["request_id"].as_str().unwrap().into(),
                delivery_id: input.delivery_id,
                occurrence_id: input.occurrence_id,
                reader_slot_id: input.reader_slot_id,
                expected_generation: 1,
                factory_session: "successor-factory".into(),
                recipient: "successor-supervisor".into(),
            },
            OperationContext {
                project_id: old_context.project_id,
                registration_id: d["registration_id"].as_str().unwrap().into(),
            },
            first.prompt_id,
        )
    }

    #[test]
    fn cas_c423_known_rebind_reconciles_original_request_after_timeout_and_restart() {
        let root = tempfile::tempdir().unwrap();
        let (request, context, original_prompt) = setup(root.path());
        let cloud = Cloud {
            response: fixture(),
            db: root.path().join("cas.db"),
            lose_response: AtomicBool::new(true),
            posts: Mutex::new(vec![]),
            lookups: Mutex::new(vec![]),
        };
        let journal = SqliteRequestJournal::open(root.path()).unwrap();
        let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        assert!(rebind_known_unread(&cloud, &journal, &store, &context, &request).is_err());
        drop(journal);
        drop(store);
        let journal = SqliteRequestJournal::open(root.path()).unwrap();
        let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        let adopted = rebind_known_unread(&cloud, &journal, &store, &context, &request).unwrap();
        assert_eq!(adopted.prompt_id, original_prompt);
        assert_eq!(
            cloud.posts.lock().unwrap().len(),
            1,
            "never dispatch a second mutation after uncertainty"
        );
        assert_eq!(
            *cloud.lookups.lock().unwrap(),
            vec![request.request_id.clone()]
        );
        assert_eq!(
            rebind_known_unread(&cloud, &journal, &store, &context, &request).unwrap(),
            adopted
        );
        assert_eq!(
            cloud.lookups.lock().unwrap().len(),
            1,
            "terminal local receipt is reusable"
        );
        let queue = cas_store::SqlitePromptQueueStore::open(root.path()).unwrap();
        use cas_store::PromptQueueStore;
        assert!(
            queue
                .peek_unseen_for_recipient("old-supervisor", Some("old-factory"), 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            queue
                .peek_unseen_for_recipient(&request.recipient, Some(&request.factory_session), 10)
                .unwrap()[0]
                .id,
            original_prompt
        );
    }

    #[test]
    fn cas_c423_known_rebind_refuses_mismatched_remote_identity_without_moving_queue() {
        let root = tempfile::tempdir().unwrap();
        let (request, context, _) = setup(root.path());
        let mut response = fixture();
        response["receipt"]["delivery"]["reader_slot_id"] = json!("foreign-reader");
        let cloud = Cloud {
            response,
            db: root.path().join("cas.db"),
            lose_response: AtomicBool::new(false),
            posts: Mutex::new(vec![]),
            lookups: Mutex::new(vec![]),
        };
        let journal = SqliteRequestJournal::open(root.path()).unwrap();
        let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        let error = rebind_known_unread(&cloud, &journal, &store, &context, &request).unwrap_err();
        assert!(error.contains("identity"), "{error}");
        let db = rusqlite::Connection::open(root.path().join("cas.db")).unwrap();
        let recipient: String = db
            .query_row("SELECT target FROM prompt_queue", [], |r| r.get(0))
            .unwrap();
        assert_eq!(recipient, "old-supervisor");
    }
}
