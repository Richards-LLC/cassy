//! Recover the same authoritative admission on existing and fresh machines.
use super::cloud::ActivityRecoveryPort;
use super::*;
use crate::coordination::external_work::{CloudCoordinationPort, SqliteRequestJournal};
use cas_store::{VioletAdmissionRebind, VioletConsumption, VioletRecoveryReceipt};

pub(crate) struct ReaderDestination<'a> {
    pub context: &'a OperationContext,
    pub factory_session: &'a str,
    pub recipient: &'a str,
}

#[derive(Debug, Default)]
pub(crate) struct RecoveryReport {
    pub restored: usize,
    pub next_cursor: Option<String>,
    pub errors: Vec<String>,
}

pub(crate) fn recover_page(
    cloud: &dyn ActivityRecoveryPort,
    journal: &SqliteRequestJournal,
    store: &SqliteVioletAdmissionStore,
    destination: &ReaderDestination<'_>,
    evidence: &activation::BindingEvidence,
    cursor: Option<&str>,
    deadline: Instant,
) -> RecoveryReport {
    let context = destination.context;
    let mut report = RecoveryReport {
        next_cursor: cursor.map(str::to_owned),
        ..Default::default()
    };
    let result = (|| -> Result<(), String> {
        // Finish the original operation before consulting a new snapshot. A
        // commit-then-timeout may already have changed rebind_required to false.
        if journal
            .has_unapplied_rebind(context)
            .map_err(|e| e.to_string())?
        {
            for operation in journal
                .lease_rebind_due(chrono::Utc::now().timestamp_millis(), context)
                .map_err(|e| e.to_string())?
            {
                let input: Value =
                    serde_json::from_str(&operation.input_json).map_err(|e| e.to_string())?;
                let text = |key: &str| {
                    input[key]
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| format!("rebind journal has no {key}"))
                };
                let request = KnownRebind {
                    request_id: operation.request_id.clone(),
                    delivery_id: text("delivery_id")?,
                    occurrence_id: text("occurrence_id")?,
                    reader_slot_id: text("reader_slot_id")?,
                    expected_generation: input["wire"]["expected_delivery_generation"]
                        .as_i64()
                        .ok_or("rebind journal has no generation")?,
                    factory_session: text("factory_session")?,
                    recipient: text("recipient")?,
                };
                let applied =
                    restore_rebound(cloud, journal, store, context, &request, evidence, deadline);
                if applied.is_ok() {
                    journal
                        .mark_rebind_applied(&request.request_id, context)
                        .map_err(|e| e.to_string())?;
                    report.restored += 1;
                } else {
                    // Terminal refusals authorize no local effect. Retire their
                    // job so a later fresh discovery can observe a new generation.
                    if journal
                        .get(&request.request_id)
                        .map_err(|e| e.to_string())?
                        .is_some_and(|op| {
                            op.stage == "terminal"
                                && op
                                    .response_json
                                    .as_deref()
                                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                                    .is_some_and(|v| v["code"] != "completed")
                        })
                    {
                        journal
                            .mark_rebind_applied(&request.request_id, context)
                            .map_err(|e| e.to_string())?;
                    } else {
                        journal
                            .clear_lease(&request.request_id)
                            .map_err(|e| e.to_string())?;
                    }
                    applied?;
                }
            }
            return Ok(());
        }
        let response = cloud
            .recover(context, cursor, remaining(deadline)?)
            .map_err(|e| e.message)?;
        require_completed(&response)?;
        let admissions: Vec<RecoverableAdmission> =
            serde_json::from_value(response["admissions"].clone()).map_err(|e| e.to_string())?;
        if admissions.len() > 25 {
            return Err("recovery page exceeds its bounded admission limit".into());
        }
        for admission in admissions {
            validate_admission(&admission, context, evidence)?;
            if admission.recovery.rebind_required {
                let d = &admission.delivery;
                let identity = serde_json::to_vec(&json!([
                    "delivery_rebind",
                    context.project_id,
                    context.registration_id,
                    d.id,
                    d.delivery_generation,
                    destination.factory_session,
                    destination.recipient
                ]))
                .map_err(|e| e.to_string())?;
                use sha2::Digest;
                let digest = sha2::Sha256::digest(identity);
                let mut bytes = [0; 16];
                bytes.copy_from_slice(&digest[..16]);
                // A stable UUID is an operation key, never a reader identity.
                let request = KnownRebind {
                    request_id: uuid::Builder::from_random_bytes(bytes)
                        .into_uuid()
                        .to_string(),
                    delivery_id: d.id.clone(),
                    occurrence_id: d.occurrence_id.clone(),
                    reader_slot_id: d.reader_slot_id.clone(),
                    expected_generation: d.delivery_generation,
                    factory_session: destination.factory_session.into(),
                    recipient: destination.recipient.into(),
                };
                restore_rebound(cloud, journal, store, context, &request, evidence, deadline)?;
                journal
                    .mark_rebind_applied(&request.request_id, context)
                    .map_err(|e| e.to_string())?;
            } else {
                let response = cloud
                    .delivery(context, &admission.delivery.id, remaining(deadline)?)
                    .map_err(|e| e.message)?;
                let fresh = decode_lookup(&response, context, evidence)?;
                if fresh.delivery.id != admission.delivery.id
                    || fresh.delivery.occurrence_id != admission.delivery.occurrence_id
                    || fresh.delivery.reader_slot_id != admission.delivery.reader_slot_id
                    || fresh.target != admission.target
                {
                    return Err(
                        "delivery lookup changed immutable admission identity or target".into(),
                    );
                }
                if fresh.delivery.delivery_generation != admission.delivery.delivery_generation
                    || fresh.delivery.route_generation != admission.delivery.route_generation
                    || fresh.delivery.route_kind != admission.delivery.route_kind
                    || consumption(&fresh.delivery.consumption)?
                        < consumption(&admission.delivery.consumption)?
                {
                    return Err("delivery lookup generation or consumption regressed".into());
                }
                restore_lookup(
                    store,
                    context,
                    destination.factory_session,
                    destination.recipient,
                    admission.delivery.delivery_generation,
                    &fresh,
                )?;
            }
            report.restored += 1;
        }
        report.next_cursor = match &response["next_cursor"] {
            Value::Null => None,
            Value::String(cursor) if uuid::Uuid::parse_str(cursor).is_ok() => Some(cursor.clone()),
            _ => return Err("invalid recovery page cursor".into()),
        };
        Ok(())
    })();
    if let Err(error) = result {
        report.errors.push(error);
    }
    report
}

#[derive(Debug, Deserialize)]
struct RecoverableAdmission {
    delivery: ReaderDelivery,
    target: ActivityTarget,
    recovery: RecoveryMode,
}
#[derive(Debug, Deserialize)]
struct RecoveryMode {
    rebind_required: bool,
    mode: String,
}

fn validate_admission(
    admission: &RecoverableAdmission,
    context: &OperationContext,
    evidence: &activation::BindingEvidence,
) -> Result<(), String> {
    let d = &admission.delivery;
    let t = &admission.target;
    if t.project_id != context.project_id || t.external_ref.provider != "slack" {
        return Err("recovery target project/provider mismatch".into());
    }
    evidence.require_managed(&t.external_ref.workspace_id, &t.external_ref.channel_id)?;
    for id in [&d.id, &d.occurrence_id, &d.reader_slot_id, &t.resource_id] {
        uuid::Uuid::parse_str(id).map_err(|_| "invalid recovery delivery identity")?;
    }
    let state = consumption(&d.consumption)?;
    let expected_mode = match state {
        VioletConsumption::Queued => "resume",
        VioletConsumption::DispatchUncertain => "recovery_notice",
        VioletConsumption::Consumed => "none",
    };
    if d.status != "admitted"
        || d.registration_id.is_empty()
        || d.delivery_generation < 1
        || d.route_generation < 0
        || !matches!(d.route_kind.as_str(), "owner" | "unclaimed_fanout")
        || admission.recovery.mode != expected_mode
        || (admission.recovery.rebind_required == (d.registration_id == context.registration_id))
    {
        return Err("recovery registration/generation/mode mismatch".into());
    }
    Ok(())
}

fn decode_lookup(
    response: &Value,
    context: &OperationContext,
    evidence: &activation::BindingEvidence,
) -> Result<RecoverableAdmission, String> {
    require_completed(response)?;
    let admission: RecoverableAdmission =
        serde_json::from_value(response["admission"].clone()).map_err(|e| e.to_string())?;
    validate_admission(&admission, context, evidence)?;
    let receipt: ReaderDelivery = serde_json::from_value(response["receipt"]["delivery"].clone())
        .map_err(|e| e.to_string())?;
    if receipt != admission.delivery || admission.recovery.rebind_required {
        return Err("delivery lookup lacks current registration receipt".into());
    }
    Ok(admission)
}

fn restore_rebound(
    cloud: &dyn ActivityRecoveryPort,
    journal: &SqliteRequestJournal,
    store: &SqliteVioletAdmissionStore,
    context: &OperationContext,
    request: &KnownRebind,
    evidence: &activation::BindingEvidence,
    deadline: Instant,
) -> Result<(), String> {
    let rebound = rebind_remote(cloud, journal, context, request, deadline)?;
    let response = cloud
        .delivery(context, &request.delivery_id, remaining(deadline)?)
        .map_err(|e| e.message)?;
    let fresh = decode_lookup(&response, context, evidence)?;
    if fresh.delivery.id != rebound.id
        || fresh.delivery.occurrence_id != rebound.occurrence_id
        || fresh.delivery.reader_slot_id != rebound.reader_slot_id
        || fresh.delivery.delivery_generation != rebound.delivery_generation
        || fresh.delivery.route_generation != rebound.route_generation
        || fresh.delivery.route_kind != rebound.route_kind
        || consumption(&fresh.delivery.consumption)? < consumption(&rebound.consumption)?
    {
        return Err("rebind lookup identity/generation regressed".into());
    }
    restore_lookup(
        store,
        context,
        &request.factory_session,
        &request.recipient,
        request.expected_generation,
        &fresh,
    )
}

fn restore_lookup(
    store: &SqliteVioletAdmissionStore,
    context: &OperationContext,
    session: &str,
    recipient: &str,
    expected_generation: i64,
    admission: &RecoverableAdmission,
) -> Result<(), String> {
    let leased = LeasedDelivery {
        delivery: admission.delivery.clone(),
        target: admission.target.clone(),
        lease_token: String::new(),
    };
    let target = &admission.target;
    let delivery = &admission.delivery;
    let input = VioletAdmissionInput {
        delivery_id: delivery.id.clone(),
        occurrence_id: delivery.occurrence_id.clone(),
        reader_slot_id: delivery.reader_slot_id.clone(),
        registration_id: context.registration_id.clone(),
        delivery_generation: delivery.delivery_generation,
        route_generation: delivery.route_generation,
        project_id: context.project_id.clone(),
        target_json: serde_json::to_string(target).map_err(|e| e.to_string())?,
        workspace_id: target.external_ref.workspace_id.clone(),
        channel_id: target.external_ref.channel_id.clone(),
        message_ts: target.message_ts.clone(),
        factory_session: session.into(),
        recipient: recipient.into(),
        prompt: managed_activity_envelope(&leased),
        lease_token: String::new(),
    };
    store
        .restore_admitted(
            &input,
            expected_generation,
            consumption(&delivery.consumption)?,
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

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
    let delivery = rebind_remote(
        cloud,
        journal,
        context,
        request,
        Instant::now() + TOTAL_REMOTE_BUDGET,
    )?;
    store
        .recover_known(
            &binding(&delivery, context, request),
            consumption(&delivery.consumption)?,
        )
        .map_err(|error| error.to_string())
}

fn binding(
    delivery: &ReaderDelivery,
    context: &OperationContext,
    request: &KnownRebind,
) -> VioletAdmissionRebind {
    VioletAdmissionRebind {
        delivery_id: delivery.id.clone(),
        occurrence_id: delivery.occurrence_id.clone(),
        reader_slot_id: delivery.reader_slot_id.clone(),
        project_id: context.project_id.clone(),
        registration_id: context.registration_id.clone(),
        expected_generation: request.expected_generation,
        delivery_generation: delivery.delivery_generation,
        factory_session: request.factory_session.clone(),
        recipient: request.recipient.clone(),
    }
}

fn consumption(state: &str) -> Result<VioletConsumption, String> {
    match state {
        "unread" | "queued" => Ok(VioletConsumption::Queued),
        "dispatch_uncertain" => Ok(VioletConsumption::DispatchUncertain),
        "consumed" => Ok(VioletConsumption::Consumed),
        _ => Err("unknown rebind consumption; never replay the ask".into()),
    }
}

fn remaining(deadline: Instant) -> Result<Duration, String> {
    let timeout = deadline
        .saturating_duration_since(Instant::now())
        .min(CALL_REMOTE_BUDGET);
    if timeout.is_zero() {
        Err("recovery remote budget exhausted".into())
    } else {
        Ok(timeout)
    }
}

fn rebind_remote(
    cloud: &dyn CloudCoordinationPort,
    journal: &SqliteRequestJournal,
    context: &OperationContext,
    request: &KnownRebind,
    deadline: Instant,
) -> Result<ReaderDelivery, String> {
    // UUIDs are path inputs, not caller-controlled endpoint fragments.
    for id in [&request.request_id, &request.delivery_id] {
        uuid::Uuid::parse_str(id).map_err(|_| "invalid rebind request/delivery UUID")?;
    }
    if request.expected_generation < 1 {
        return Err("invalid rebind expected generation".into());
    }
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
    let response: Value = if matches!(operation.stage.as_str(), "terminal" | "applied") {
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
        let budget = remaining(deadline)?;
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
            let lookup = cloud.lookup(&request.request_id, &context.registration_id, budget);
            match lookup {
                Ok(response)
                    if response["schema"] == CONTRACT_SCHEMA
                        && response["code"] == "not_found"
                        && response["reason"] == "unknown_request" =>
                {
                    // A crash may precede the actual HTTP call. Cloud's
                    // authoritative absence permits the SAME frozen request,
                    // never a replacement UUID or changed input.
                    cloud.post(
                        &format!(
                            "/api/coordination/activity/deliveries/{}/rebind",
                            request.delivery_id
                        ),
                        &wire,
                        remaining(deadline)?,
                    )
                }
                other => other,
            }
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
    consumption(&delivery.consumption)?;
    Ok(delivery)
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
            (
                "dispatch_uncertain",
                VioletConsumption::DispatchUncertain,
                1,
            ),
            ("consumed", VioletConsumption::Consumed, 0),
        ] {
            let root = tempfile::tempdir().unwrap();
            let (request, context, original_prompt) = setup(root.path());
            let mut response = fixture();
            response["receipt"]["delivery"]["consumption"] = json!(remote);
            let cloud = Cloud {
                response,
                db: root.path().join("cas.db"),
                lose_response: AtomicBool::new(true),
                posts: Mutex::new(vec![]),
                lookups: Mutex::new(vec![]),
            };
            let journal = SqliteRequestJournal::open(root.path()).unwrap();
            let store = SqliteVioletAdmissionStore::open(root.path()).unwrap();
            assert!(rebind_known(&cloud, &journal, &store, &context, &request).is_err());
            let recovered = rebind_known(&cloud, &journal, &store, &context, &request).unwrap();
            assert_eq!(recovered.admission.prompt_id, original_prompt);
            assert_eq!(recovered.consumption, expected);
            let queue = cas_store::SqlitePromptQueueStore::open(root.path()).unwrap();
            let pending = queue
                .peek_unseen_for_recipient(&request.recipient, Some(&request.factory_session), 10)
                .unwrap();
            assert_eq!(pending.len(), notice_count);
            if let Some(notice) = pending.first() {
                assert_eq!(Some(notice.id), recovered.recovery_prompt_id);
                assert!(notice.prompt.contains(&request.delivery_id));
                assert!(!notice.prompt.contains("<cas-violet-activity"));
            }
            assert_eq!(
                rebind_known(&cloud, &journal, &store, &context, &request).unwrap(),
                recovered
            );
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
        let cloud = Cloud {
            response: fixture(),
            db: root.path().join("cas.db"),
            lose_response: AtomicBool::new(false),
            posts: Mutex::new(vec![]),
            lookups: Mutex::new(vec![]),
        };
        let recovered = rebind_known(
            &cloud,
            &SqliteRequestJournal::open(root.path()).unwrap(),
            &SqliteVioletAdmissionStore::open(root.path()).unwrap(),
            &context,
            &request,
        )
        .unwrap();
        assert_eq!(recovered.consumption, VioletConsumption::DispatchUncertain);
        let pending = queue
            .peek_unseen_for_recipient(&request.recipient, Some(&request.factory_session), 10)
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_ne!(pending[0].id, original_prompt);
    }
}

#[cfg(test)]
#[path = "violet_discovery_tests.rs"]
mod discovery_tests;
