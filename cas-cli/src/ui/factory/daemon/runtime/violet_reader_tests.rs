//! cas-146e: the v2 reader against Cloud's wire fixtures
//! (`tests/contracts/coordination/v1`, vendored at petra-stella-cloud 00d0675).

use super::*;
use crate::coordination::external_work::RemoteError;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const CLAIM: &str = include_str!("../../../../../tests/fixtures/coordination_contract/v1/violet_activity--claim_v2.json");
const ACK: &str = include_str!("../../../../../tests/fixtures/coordination_contract/v1/violet_activity--ack_v2.json");
const RECEIPTS: &str = include_str!("../../../../../tests/fixtures/coordination_contract/v1/deliveries--receipts.json");
const RECOVER: &str = include_str!("../../../../../tests/fixtures/coordination_contract/v1/deliveries--recover.json");
const REBIND: &str = include_str!("../../../../../tests/fixtures/coordination_contract/v1/deliveries--rebind.json");
const REGISTRATION: &str = "reg-supervisor-alpha-01";
const DELIVERY: &str = "0000de2a-1111-4111-8111-00000000de2a";

/// The response payload of the fixture case whose name contains `case`.
fn fixture(doc: &str, case: &str) -> Value {
    let doc: Value = serde_json::from_str(doc).unwrap();
    doc["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"].as_str().unwrap().contains(case))
        .unwrap_or_else(|| panic!("no fixture case {case}"))["response"]["payload"]
        .clone()
}

/// Answers each route from Cloud's fixtures and logs every step, shared with
/// the typer, so the test sees one ordered timeline.
struct FixtureCloud {
    claim: Value,
    refuse_dispatch: bool,
    ack_response: Option<Value>,
    registration_id: &'static str,
    recover: Value,
    generations: Mutex<std::collections::HashMap<String, u64>>,
    claim_calls: AtomicUsize,
    log: Mutex<Vec<String>>,
}

impl FixtureCloud {
    fn new(claim_case: &str) -> Self {
        Self {
            claim: fixture(CLAIM, claim_case), refuse_dispatch: false,
            ack_response: None, registration_id: REGISTRATION,
            recover: fixture(RECOVER, "nothing left to recover"), generations: Mutex::default(),
            claim_calls: AtomicUsize::new(0), log: Mutex::new(Vec::new()),
        }
    }
    fn push(&self, step: String) {
        self.log.lock().unwrap().push(step);
    }
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
    fn generation(&self, id: &str) -> u64 {
        *self.generations.lock().unwrap().get(id).unwrap_or(&1)
    }
}

impl CloudCoordinationPort for FixtureCloud {
    fn post(&self, path: &str, payload: &Value, _: Duration) -> Result<Value, RemoteError> {
        assert_eq!(payload["schema"], CONTRACT_SCHEMA, "{path}");
        assert_eq!(payload["actor"]["registration_id"], self.registration_id, "{path}");
        if path == CLAIM_PATH {
            self.claim_calls.fetch_add(1, Ordering::Relaxed);
            return Ok(self.claim.clone());
        }
        if path == ACK_PATH {
            if let Some(response) = &self.ack_response {
                return Ok(response.clone());
            }
            let mut results = Vec::new();
            for ack in payload["acks"].as_array().unwrap() {
                assert_eq!(ack["lease_token"], "0001ea5e-1111-4111-8111-00000001ea5e");
                assert_eq!(ack["delivery_generation"], 1);
                self.push(format!("ack {} {}", ack["delivery_id"].as_str().unwrap(), ack["outcome"].as_str().unwrap()));
                results.push(fixture(ACK, "durable admission")["results"][0].clone());
            }
            let mut response = fixture(ACK, "durable admission");
            response["results"] = Value::Array(results);
            return Ok(response);
        }
        let rest = path.strip_prefix("/api/coordination/activity/deliveries/").unwrap_or_else(|| panic!("{path}"));
        if let Some(id) = rest.strip_suffix("/rebind") {
            assert_eq!(payload["expected_delivery_generation"], self.generation(id));
            let generation = self.generation(id) + 1;
            self.generations.lock().unwrap().insert(id.to_string(), generation);
            self.push(format!("rebind {id} {generation}"));
            let mut rebound = fixture(REBIND, "successor adopts");
            rebound["receipt"]["delivery"]["id"] = json!(id);
            rebound["receipt"]["delivery"]["delivery_generation"] = json!(generation);
            return Ok(rebound);
        }
        let id = rest.strip_suffix("/receipts").unwrap_or_else(|| panic!("unexpected route {path}"));
        let state = payload["state"].as_str().unwrap();
        assert_eq!(payload["delivery_generation"], self.generation(id));
        assert!(payload["request_id"].as_str().is_some_and(|r| uuid::Uuid::parse_str(r).is_ok()));
        if state == "dispatch_uncertain" && self.refuse_dispatch {
            // A superseded reader's receipt: Cloud answers 409 stale_generation.
            self.push(format!("receipt {id} {state} refused"));
            let mut refused = fixture(ACK, "durable admission");
            refused["code"] = json!("stale_generation");
            return Ok(refused);
        }
        self.push(format!("receipt {id} {state}"));
        Ok(fixture(RECEIPTS, if state == "consumed" { "consumed" } else { "dispatch uncertain" }))
    }

    fn lookup(&self, _: &str, _: &str, _: Duration) -> Result<Value, RemoteError> {
        unreachable!("the reader never looks up operations")
    }

    fn get(&self, path: &str, query: &[(&str, &str)], _: Duration) -> Result<Value, RemoteError> {
        assert_eq!(path, "/api/coordination/activity/deliveries");
        assert!(query.contains(&("registration_id", self.registration_id)), "{query:?}");
        Ok(self.recover.clone())
    }
}

fn violet_body(refusal: Option<&str>) -> impl BodySource + '_ {
    super::super::violet_body::VioletBodySource(move |args: Value| {
        assert_eq!(args, json!({
            "external_ref": {"workspace_id":"T07ABCDEF", "channel_id":"C08COORD01", "message_ts":"1760616042.000200"},
            "include_threads":true, "include_files":false, "include_channels":false
        }));
        let mut response = json!({"ok":true,"schema_version":1,
            "identity":{"workspace_id":"T07ABCDEF","channel_id":"C08COORD01","root_thread_ts":"1760616000.123456"},
            "messages":[{"message_id":"1760616000.123456","text":"unrelated root"},
                {"message_id":"1760616042.000200","thread_id":"1760616000.123456","text":"body from Violet"}],
            "thread_roots":[{"message_id":"1760616042.000200","text":"truncated context, never body"}]
        });
        match refusal {
            Some("violet_unavailable") => return Err(NamedRefusal::new("violet_unavailable", "Violet read unavailable")),
            Some("violet_refused") => response = json!({"ok":false,"code":"not_member"}),
            Some("identity_mismatch") => response["identity"]["workspace_id"] = json!("TOTHER"),
            Some("message_missing") => response["messages"] = json!([]),
            _ => {}
        }
        Ok(json!({"structuredContent":response}))
    })
}

struct Typer<'a>(&'a FixtureCloud);
impl SessionTyper for Typer<'_> {
    fn type_into(&self, session: &str, delivery: &ClaimedDelivery, body: &str) -> Result<(), NamedRefusal> {
        self.0.push(format!("typed {} into {session}: {body}", delivery.id));
        Ok(())
    }
    fn notice_into(&self, session: &str, delivery: &ClaimedDelivery) -> Result<(), NamedRefusal> {
        self.0.push(format!("notice {} into {session}", delivery.id));
        Ok(())
    }
}

fn tick(reader: &mut VioletReader, cloud: &FixtureCloud) -> ClaimOutcome {
    reader.tick(cloud, REGISTRATION, &violet_body(None), &Typer(cloud), Utc::now())
}

/// Reader order is ack admitted → dispatch_uncertain → type → consumed, and a
/// delivery Cloud hands out again is typed once by this process.
#[test]
fn reader_walks_cloud_order_and_types_a_redelivered_id_once() {
    let mut cloud = FixtureCloud::new("leases this reader's deliveries");
    let mut reader = VioletReader::new("factory-session");
    for registration in [REGISTRATION, "successor-registration"] {
        cloud.registration_id = registration;
        let outcome = reader.tick(&cloud, registration, &violet_body(None), &Typer(&cloud), Utc::now());
        assert!(matches!(outcome, ClaimOutcome::Read(_)));
    }
    assert_eq!(
        cloud.log(),
        [
            format!("ack {DELIVERY} admitted"),
            format!("receipt {DELIVERY} dispatch_uncertain"),
            format!("typed {DELIVERY} into factory-session: body from Violet"),
            format!("receipt {DELIVERY} consumed"),
            format!("ack {DELIVERY} admitted"),
        ]
    );
    assert!(reader.managed(), "a completed v2 claim turns the legacy sweep off");
}

/// A refused write-ahead receipt fences the reader: nothing is typed.
#[test]
fn refused_dispatch_uncertain_types_nothing() {
    for ack_stale in [false, true] {
        let mut cloud = FixtureCloud::new("leases this reader's deliveries");
        cloud.refuse_dispatch = !ack_stale;
        if ack_stale {
            let mut ack = fixture(ACK, "durable admission");
            ack["results"][0] = ack["results"][3].clone();
            ack["results"][0]["delivery_id"] = json!(DELIVERY);
            cloud.ack_response = Some(ack);
        }
        let mut reader = VioletReader::new("factory-session");
        let ClaimOutcome::Read(report) = tick(&mut reader, &cloud) else { panic!("claim refused") };
        assert_eq!(
            cloud.log(),
            if ack_stale { vec![] } else {
                vec![format!("ack {DELIVERY} admitted"), format!("receipt {DELIVERY} dispatch_uncertain refused")]
            }
        );
        assert_eq!((report.typed, report.not_typed[0].1.code.as_str()), (0, "stale_generation"));
        assert!(reader.stopped.is_none(), "a stale delivery generation does not revoke the registration");
    }
    for code in ["violet_unavailable", "violet_refused", "identity_mismatch", "message_missing"] {
        let cloud = FixtureCloud::new("leases this reader's deliveries");
        let mut reader = VioletReader::new("factory-session");
        let ClaimOutcome::Read(report) = reader.tick(&cloud, REGISTRATION, &violet_body(Some(code)), &Typer(&cloud), Utc::now()) else { panic!("claim refused") };
        assert_eq!(cloud.log(), [format!("ack {DELIVERY} admitted")], "{code}: no receipt or typing after failed fetch");
        assert_eq!((report.typed, report.not_typed[0].1.code.as_str()), (0, code));
    }
}

#[test]
fn delivery_routes_to_its_mapped_local_session_cas_c186() {
    for mapped in [None, Some("mapped-session")] {
        let temp = tempfile::tempdir().unwrap();
        let queue = cas_store::SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        let cloud = FixtureCloud::new("leases this reader's deliveries");
        let mut reader = VioletReader::new("factory-session");
        if let Some(session) = mapped {
            reader.bind_session(DELIVERY, session);
        }
        let outcome = reader.tick(&cloud, REGISTRATION, &violet_body(None), &PromptQueueTyper(&queue), Utc::now());
        assert!(matches!(outcome, ClaimOutcome::Read(ReaderReport { typed: 1, .. })));
        let queued = queue.peek_all(10).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].factory_session.as_deref(), Some(mapped.unwrap_or("factory-session")));
    }
}

#[test]
fn unknown_registration_stops_consumption_until_successor_cas_c186() {
    for at_ack in [false, true] {
        let mut cloud = FixtureCloud::new("leases this reader's deliveries");
        let refusal = json!({"schema": CONTRACT_SCHEMA, "code": "not_found", "reason": "unknown_registration"});
        if at_ack {
            cloud.ack_response = Some(refusal);
        } else {
            cloud.claim = refusal;
        }
        let mut reader = VioletReader::new("factory-session");
        tick(&mut reader, &cloud);
        assert_eq!(
            tick(&mut reader, &cloud),
            ClaimOutcome::Stopped(NamedRefusal::new("not_found", "unknown_registration")),
        );
        assert_eq!(cloud.claim_calls.load(Ordering::Relaxed), 1, "superseded registration stops polling");
        assert!(cloud.log().is_empty(), "superseded registration types nothing");
        assert!(reader.managed(), "a superseded registration must not fall back to v1");

        // /clear replaces the registration without restarting the daemon.
        cloud.claim = fixture(CLAIM, "leases this reader's deliveries");
        cloud.ack_response = None;
        cloud.registration_id = "successor-registration";
        let outcome = reader.tick(&cloud, cloud.registration_id, &violet_body(None), &Typer(&cloud), Utc::now());
        assert!(matches!(outcome, ClaimOutcome::Read(ReaderReport { typed: 1, .. })));
        assert_eq!(cloud.claim_calls.load(Ordering::Relaxed), 2);
    }
}

/// cas-51aa: after a crash the successor consumes each admission once. A
/// lease never acked returns through claim; an unread admission still bound
/// to the predecessor is rebound and resumed; a dispatch-uncertain one gets
/// one recovery notice, then `consumed`, and its ask is never typed again.
#[test]
fn successor_recovers_each_admission_once_and_never_retypes_an_uncertain_ask() {
    const SUCCESSOR: &str = "reg-supervisor-beta-08";
    let (resumed, uncertain) = ("0000de1b-1111-4111-8111-00000000de1b", "0000de1c-1111-4111-8111-00000000de1c");
    let mut cloud = FixtureCloud::new("leases this reader's deliveries");
    cloud.registration_id = "reg-supervisor-beta-07";
    cloud.ack_response = Some(json!({})); // the predecessor crashes before its ack lands
    tick_as(&mut VioletReader::new("factory-session"), &cloud);

    cloud.registration_id = SUCCESSOR;
    cloud.ack_response = None;
    cloud.recover = fixture(RECOVER, "successor on another machine");
    let mut successor = VioletReader::new("factory-session");
    for _ in 0..2 {
        tick_as(&mut successor, &cloud);
    }
    assert_eq!(
        cloud.log(),
        [
            format!("rebind {resumed} 2"),
            format!("receipt {resumed} dispatch_uncertain"),
            format!("typed {resumed} into factory-session: body for 1760616000.123456"),
            format!("receipt {resumed} consumed"),
            format!("rebind {uncertain} 2"),
            format!("notice {uncertain} into factory-session"),
            format!("receipt {uncertain} consumed"),
            format!("ack {DELIVERY} admitted"),
            format!("receipt {DELIVERY} dispatch_uncertain"),
            format!("typed {DELIVERY} into factory-session: body for 1760616042.000200"),
            format!("receipt {DELIVERY} consumed"),
            format!("ack {DELIVERY} admitted"),
        ]
    );
}

fn tick_as(reader: &mut VioletReader, cloud: &FixtureCloud) -> ClaimOutcome {
    reader.tick(cloud, cloud.registration_id, &AnyTarget, &Typer(cloud), Utc::now())
}

/// Recovery resumes targets other than `violet_body`'s one fixture message;
/// the fetch itself is covered by the Violet adapter tests.
struct AnyTarget;
impl BodySource for AnyTarget {
    fn fetch_body(&self, target: &Value) -> Result<String, NamedRefusal> {
        Ok(format!("body for {}", target["message_ts"].as_str().unwrap()))
    }
}

/// `409 strict_activation_required` is a named refusal on a managed binding;
/// only `binding_unmanaged` leaves the legacy path on.
#[test]
fn strict_activation_refusal_is_named_and_unmanaged_keeps_legacy() {
    let cloud = FixtureCloud::new("without strict activation evidence");
    let mut reader = VioletReader::new("factory-session");
    assert_eq!(
        tick(&mut reader, &cloud),
        ClaimOutcome::Refused(NamedRefusal::new("strict_activation_required", "binding_unevidenced"))
    );
    assert!(cloud.log().is_empty() && reader.managed());

    let cloud = FixtureCloud::new("binding not activated for v2");
    let mut reader = VioletReader::new("factory-session");
    assert_eq!(tick(&mut reader, &cloud), ClaimOutcome::Unmanaged);
    assert!(!reader.managed());
}


/// Re-observing the source does not make an old delivery or recovery fresh.
#[test]
fn managed_wake_reports_source_age_and_preserves_dedupe_cas_8875() {
    let now = DateTime::parse_from_rfc3339("2026-10-17T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let old = format!("{}.000000", (now - chrono::Duration::days(5)).timestamp());
    let temp = tempfile::tempdir().unwrap();
    let queue = cas_store::SqlitePromptQueueStore::open(temp.path()).unwrap();
    queue.init().unwrap();
    let delivery = ClaimedDelivery {
        id: DELIVERY.to_owned(),
        delivery_generation: 1,
        lease_token: "fixture-lease".into(),
        target: json!({"external_ref":{"channel_id":"C_FIXTURE", "root_ts":old}, "message_ts":old}),
    };
    for (kind, key) in [
        ("delivery", "violet-delivery"),
        ("recovery_notice", "violet-recovery"),
    ] {
        let typer = PromptQueueTyper(&queue);
        typer
            .enqueue_at("fixture-session", &delivery, kind, "", key, now)
            .unwrap();
        typer
            .enqueue_at(
                "fixture-session",
                &delivery,
                kind,
                "",
                key,
                now + chrono::Duration::minutes(3),
            )
            .unwrap();
    }
    let rows = queue.peek_all(10).unwrap();
    assert_eq!(
        rows.len(),
        2,
        "the original delivery/recovery keys still dedupe re-observation"
    );
    for row in rows {
        // pin: source age is trusted agent-facing wake metadata, in seconds.
        assert!(row.prompt.contains("age_secs=432000"), "{}", row.prompt);
        // pin: the original source occurrence/thread identity must survive formatting.
        assert!(
            row.prompt
                .contains(&format!("message_ts={old} thread_ts={old}")),
            "source identity is unchanged"
        );
        assert_eq!(row.factory_session.as_deref(), Some("fixture-session"));
    }
}

#[test]
fn managed_wake_invalid_future_and_fresh_age_policy_cas_8875() {
    let now = DateTime::parse_from_rfc3339("2026-10-17T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let fresh = format!("{}.000000", now.timestamp());
    let future = format!("{}.000001", now.timestamp());
    let invalid_fraction = format!("{}.000000invalid", now.timestamp());
    for (source, expected) in [
        (Some(fresh.as_str()), Some("age_secs=0")),
        (Some(future.as_str()), Some("age_secs=unknown")),
        (Some("invalid"), Some("age_secs=unknown")),
        (Some(invalid_fraction.as_str()), Some("age_secs=unknown")),
        (Some(""), None),
        (None, None),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let queue = cas_store::SqlitePromptQueueStore::open(temp.path()).unwrap();
        queue.init().unwrap();
        let delivery = ClaimedDelivery {
            id: DELIVERY.to_owned(),
            delivery_generation: 1,
            lease_token: "fixture-lease".into(),
            target: json!({"external_ref":{"channel_id":"C_FIXTURE", "root_ts":fresh}, "message_ts":source}),
        };
        let result = PromptQueueTyper(&queue).enqueue_at(
            "fixture-session",
            &delivery,
            "delivery",
            "",
            "violet-delivery",
            now,
        );
        let rows = queue.peek_all(10).unwrap();
        if let Some(age) = expected {
            result.unwrap();
            assert_eq!(rows.len(), 1);
            // pin: unknown source age must differ textually from a legitimate fresh zero.
            assert!(
                rows[0].prompt.contains(age),
                "source={source:?}: {}",
                rows[0].prompt
            );
        } else {
            assert_eq!(result.unwrap_err().code, "invalid_target");
            assert!(
                rows.is_empty(),
                "missing source identity is refused, never displayed as fresh"
            );
        }
    }
}

