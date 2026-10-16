//! Known-admission recovery. Discovery on a fresh machine needs Cloud #167.
use super::*;
use crate::coordination::external_work::{CloudCoordinationPort, SqliteRequestJournal};
use cas_store::{VioletAdmissionRebind, VioletConsumption, VioletRecoveryReceipt};

pub(crate) struct KnownRebind {
    pub request_id: String,
    pub delivery_id: String,
    pub occurrence_id: String,
    pub reader_slot_id: String,
    pub expected_generation: i64,
    pub factory_session: String,
    pub recipient: String,
}

pub(crate) fn rebind_known(
    cloud: &dyn CloudCoordinationPort,
    journal: &SqliteRequestJournal,
    store: &SqliteVioletAdmissionStore,
    context: &OperationContext,
    request: &KnownRebind,
) -> Result<VioletRecoveryReceipt, String> {
    // UUIDs are path inputs, not caller-controlled endpoint fragments.
    for id in [&request.request_id, &request.delivery_id] {
        uuid::Uuid::parse_str(id).map_err(|_| "invalid rebind request/delivery UUID")?;
    }
    if request.expected_generation < 1 {
        return Err("invalid rebind expected generation".into());
    }
    let started = Instant::now();
    let timeout = || {
        let remaining = TOTAL_REMOTE_BUDGET
            .saturating_sub(started.elapsed())
            .min(CALL_REMOTE_BUDGET);
        if remaining.is_zero() {
            Err("rebind remote budget exhausted".to_string())
        } else {
            Ok(remaining)
        }
    };
    let actor = json!({"registration_id":context.registration_id});
    let wire = json!({"schema":CONTRACT_SCHEMA, "request_id":request.request_id,
        "actor":actor, "expected_delivery_generation":request.expected_generation});
    // Local routing is part of the immutable logical input, but never sent to
    // Cloud as authority or a caller-provided reader list.
    let logical = json!({"actor":actor,"project_id":context.project_id,"wire":wire,
        "delivery_id":request.delivery_id,"occurrence_id":request.occurrence_id,
        "reader_slot_id":request.reader_slot_id,"factory_session":request.factory_session,
        "recipient":request.recipient});
    let (operation, _) = journal
        .prepare(&request.request_id, "delivery_rebind", &logical)
        .map_err(|error| error.to_string())?;
    let response: Value = if operation.stage == "terminal" {
        serde_json::from_str(
            operation
                .response_json
                .as_deref()
                .ok_or("rebind terminal receipt is missing")?,
        )
        .map_err(|error| format!("invalid rebind journal receipt: {error}"))?
    } else {
        let first_dispatch = matches!(operation.stage.as_str(), "prepared" | "ready");
        if first_dispatch {
            journal
                .set_wire_request(&request.request_id, &wire)
                .map_err(|error| error.to_string())?;
        }
        let budget = timeout()?;
        journal
            .begin_attempt(&request.request_id)
            .map_err(|error| error.to_string())?;
        let remote = if first_dispatch {
            cloud.post(
                &format!(
                    "/api/coordination/activity/deliveries/{}/rebind",
                    request.delivery_id
                ),
                &wire,
                budget,
            )
        } else {
            // Dispatch might have committed. Reconcile its original identity;
            // never issue another rebind or invent a new request ID.
            cloud.lookup(&request.request_id, &context.registration_id, budget)
        };
        let response = remote.unwrap_or_else(|error| {
            json!({"schema":CONTRACT_SCHEMA,
            "code":"outcome_unknown","guidance":error.message,
            "receipt":{"operation_id":null,"request_id":request.request_id,
                "resource":null,"visible_owner":null,"generation":null,"expires_at":null,
                "observed_at":chrono::Utc::now().to_rfc3339(),"next_action":"reconcile"}})
        });
        journal
            .record_result(&request.request_id, &response, Some(Duration::from_secs(2)))
            .map_err(|error| error.to_string())?;
        response
    };
    require_completed(&response)?;
    if response["receipt"]["request_id"].as_str() != Some(request.request_id.as_str()) {
        return Err("rebind receipt request identity mismatch".into());
    }
    let delivery: ReaderDelivery = serde_json::from_value(response["receipt"]["delivery"].clone())
        .map_err(|error| format!("invalid rebind delivery identity: {error}"))?;
    if delivery.id != request.delivery_id
        || delivery.occurrence_id != request.occurrence_id
        || delivery.reader_slot_id != request.reader_slot_id
        || delivery.registration_id != context.registration_id
        || delivery.status != "admitted"
    {
        return Err("rebind receipt delivery/reader/registration identity mismatch".into());
    }
    if !matches!(delivery.consumption.as_str(), "unread" | "queued") {
        return Err("rebind consumption requires recovery; never replay the ask".into());
    }
    store
        .adopt_unread(&VioletAdmissionRebind {
            delivery_id: delivery.id,
            occurrence_id: delivery.occurrence_id,
            reader_slot_id: delivery.reader_slot_id,
            project_id: context.project_id.clone(),
            registration_id: context.registration_id.clone(),
            expected_generation: request.expected_generation,
            delivery_generation: delivery.delivery_generation,
            factory_session: request.factory_session.clone(),
            recipient: request.recipient.clone(),
        })
        .map(|admission| VioletRecoveryReceipt { admission, consumption: VioletConsumption::Queued, recovery_prompt_id: None })
        .map_err(|error| error.to_string())
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
            assert!(path.ends_with(&format!(
                    "/{}/rebind",
                    body["delivery_id"]
                        .as_str()
                        .unwrap_or("0000de1b-1111-4111-8111-00000000de1b")
                )));
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
        assert!(rebind_known(&cloud, &journal, &store, &context, &request).is_err());
        drop(journal);
        drop(store);
        let journal = SqliteRequestJournal::open(root.path()).unwrap();
        let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
        let adopted = rebind_known(&cloud, &journal, &store, &context, &request).unwrap();
        assert_eq!(adopted.admission.prompt_id, original_prompt);
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
            rebind_known(&cloud, &journal, &store, &context, &request).unwrap(),
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
        let error = rebind_known(&cloud, &journal, &store, &context, &request).unwrap_err();
        assert!(error.contains("identity"), "{error}");
        let db = rusqlite::Connection::open(root.path().join("cas.db")).unwrap();
        let recipient: String = db
            .query_row("SELECT target FROM prompt_queue", [], |r| r.get(0))
            .unwrap();
        assert_eq!(recipient, "old-supervisor");
    }

    #[test]
    fn cas_c423_known_rebind_remote_consumption_recovers_reference_without_ask() {
        use cas_store::PromptQueueStore;
        for (remote, expected, notice_count) in [
            ("dispatch_uncertain", VioletConsumption::DispatchUncertain, 1),
            ("consumed", VioletConsumption::Consumed, 0),
        ] {
            let root = tempfile::tempdir().unwrap();
            let (request, context, original_prompt) = setup(root.path());
            let mut response = fixture();
            response["receipt"]["delivery"]["consumption"] = json!(remote);
            let cloud = Cloud { response, db: root.path().join("cas.db"),
                lose_response: AtomicBool::new(true), posts: Mutex::new(vec![]), lookups: Mutex::new(vec![]) };
            let journal = SqliteRequestJournal::open(root.path()).unwrap();
            let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
            assert!(rebind_known(&cloud, &journal, &store, &context, &request).is_err());
            let recovered = rebind_known(&cloud, &journal, &store, &context, &request).unwrap();
            assert_eq!(recovered.admission.prompt_id, original_prompt);
            assert_eq!(recovered.consumption, expected);
            let queue = cas_store::SqlitePromptQueueStore::open(root.path()).unwrap();
            let pending = queue.peek_unseen_for_recipient(&request.recipient, Some(&request.factory_session), 10).unwrap();
            assert_eq!(pending.len(), notice_count);
            if let Some(notice) = pending.first() {
                assert_eq!(Some(notice.id), recovered.recovery_prompt_id);
                assert!(notice.prompt.contains(&request.delivery_id));
                assert!(!notice.prompt.contains("<cas-violet-activity"));
            }
            assert_eq!(rebind_known(&cloud, &journal, &store, &context, &request).unwrap(), recovered);
            assert_eq!(cloud.posts.lock().unwrap().len(), 1);
            assert_eq!(cloud.lookups.lock().unwrap().len(), 1);
        }
    }

    #[test]
    fn cas_c423_known_rebind_local_transport_evidence_overrides_remote_unread() {
        use cas_store::PromptQueueStore;
        let root = tempfile::tempdir().unwrap();
        let (request, context, original_prompt) = setup(root.path());
        let queue = cas_store::SqlitePromptQueueStore::open(root.path()).unwrap();
        queue.mark_transport_delivered(original_prompt).unwrap();
        let cloud = Cloud { response: fixture(), db: root.path().join("cas.db"),
            lose_response: AtomicBool::new(false), posts: Mutex::new(vec![]), lookups: Mutex::new(vec![]) };
        let recovered = rebind_known(&cloud, &SqliteRequestJournal::open(root.path()).unwrap(),
            &SqliteVioletAdmissionStore::open(root.path()).unwrap(), &context, &request).unwrap();
        assert_eq!(recovered.consumption, VioletConsumption::DispatchUncertain);
        let pending = queue.peek_unseen_for_recipient(&request.recipient, Some(&request.factory_session), 10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_ne!(pending[0].id, original_prompt);
    }
}
