//! cas-146e: the v2 reader against Cloud's wire fixtures
//! (`tests/contracts/coordination/v1`, vendored at petra-stella-cloud 00d0675).

use super::*;
use crate::coordination::external_work::RemoteError;
use std::sync::Mutex;
use std::time::Duration;

const CLAIM: &str = include_str!("../../../../../tests/fixtures/coordination_contract/v1/violet_activity--claim_v2.json");
const ACK: &str = include_str!("../../../../../tests/fixtures/coordination_contract/v1/violet_activity--ack_v2.json");
const RECEIPTS: &str = include_str!("../../../../../tests/fixtures/coordination_contract/v1/deliveries--receipts.json");
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
    log: Mutex<Vec<String>>,
}

impl FixtureCloud {
    fn new(claim_case: &str) -> Self {
        Self { claim: fixture(CLAIM, claim_case), refuse_dispatch: false, log: Mutex::new(Vec::new()) }
    }
    fn push(&self, step: String) {
        self.log.lock().unwrap().push(step);
    }
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl CloudCoordinationPort for FixtureCloud {
    fn post(&self, path: &str, payload: &Value, _: Duration) -> Result<Value, RemoteError> {
        assert_eq!(payload["schema"], CONTRACT_SCHEMA, "{path}");
        assert_eq!(payload["actor"]["registration_id"], REGISTRATION, "{path}");
        if path == CLAIM_PATH {
            return Ok(self.claim.clone());
        }
        if path == ACK_PATH {
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
        let id = path
            .strip_prefix("/api/coordination/activity/deliveries/")
            .and_then(|rest| rest.strip_suffix("/receipts"))
            .unwrap_or_else(|| panic!("unexpected route {path}"));
        let state = payload["state"].as_str().unwrap();
        assert_eq!(payload["delivery_generation"], 1);
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
}

struct Body;
impl BodySource for Body {
    fn fetch_body(&self, target: &Value) -> Result<String, NamedRefusal> {
        Ok(format!("body for {}", target["message_ts"].as_str().unwrap()))
    }
}

struct Typer<'a>(&'a FixtureCloud);
impl SessionTyper for Typer<'_> {
    fn type_into(&self, session: &str, delivery: &ClaimedDelivery, body: &str) -> Result<(), NamedRefusal> {
        self.0.push(format!("typed {} into {session}: {body}", delivery.id));
        Ok(())
    }
}

fn tick(reader: &mut VioletReader, cloud: &FixtureCloud) -> ClaimOutcome {
    reader.tick(cloud, REGISTRATION, &Body, &Typer(cloud), Utc::now())
}

/// Reader order is ack admitted → dispatch_uncertain → type → consumed, and a
/// delivery Cloud hands out again is typed once by this process.
#[test]
fn reader_walks_cloud_order_and_types_a_redelivered_id_once() {
    let cloud = FixtureCloud::new("leases this reader's deliveries");
    let mut reader = VioletReader::new("factory-session");
    for _ in 0..2 {
        assert!(matches!(tick(&mut reader, &cloud), ClaimOutcome::Read(_)));
    }
    assert_eq!(
        cloud.log(),
        [
            format!("ack {DELIVERY} admitted"),
            format!("receipt {DELIVERY} dispatch_uncertain"),
            format!("typed {DELIVERY} into factory-session: body for 1760616042.000200"),
            format!("receipt {DELIVERY} consumed"),
            format!("ack {DELIVERY} admitted"),
        ]
    );
    assert!(reader.managed(), "a completed v2 claim turns the legacy sweep off");
}

/// A refused write-ahead receipt fences the reader: nothing is typed.
#[test]
fn refused_dispatch_uncertain_types_nothing() {
    let mut cloud = FixtureCloud::new("leases this reader's deliveries");
    cloud.refuse_dispatch = true;
    let mut reader = VioletReader::new("factory-session");
    let ClaimOutcome::Read(report) = tick(&mut reader, &cloud) else { panic!("claim refused") };
    assert_eq!(
        cloud.log(),
        [format!("ack {DELIVERY} admitted"), format!("receipt {DELIVERY} dispatch_uncertain refused")]
    );
    assert_eq!((report.typed, report.not_typed[0].1.code.as_str()), (0, "stale_generation"));
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
