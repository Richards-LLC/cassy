//! cas-b512: Cassy's side of the `cassy.coordination/v1` contract fixtures
//! (spec-022d part (a)). Cloud vendors its own copy and implements the HTTP
//! surfaces against these files; Violet implements the two tool surfaces;
//! Cassy's coordination ports deserialize the stable codes below. The
//! fixtures are embedded at compile time so archive shards need no source
//! tree.

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

const CONTRACT: &str = "cassy.coordination/v1";

macro_rules! fixture {
    ($name:literal) => {
        (
            $name,
            include_str!(concat!(
                "../../tests/fixtures/coordination_contract/v1/",
                $name
            )),
        )
    };
}

/// Every surface fixture, Cloud-vendored and Cassy-authored.
const FIXTURES: &[(&str, &str)] = &[
    fixture!("activity--observe.json"),
    fixture!("deliveries--rebind.json"),
    fixture!("deliveries--receipts.json"),
    fixture!("external_work--claim.json"),
    fixture!("external_work--complete.json"),
    fixture!("external_work--handoff_offer.json"),
    fixture!("external_work--release.json"),
    fixture!("external_work--renew.json"),
    fixture!("external_work--reply_intents.json"),
    fixture!("handoffs--accept.json"),
    fixture!("operations--lookup.json"),
    fixture!("reply_intents--dispatch.json"),
    fixture!("violet_activity--ack_v2.json"),
    fixture!("violet_activity--claim_v2.json"),
    fixture!("violet_post--managed_reply.json"),
    fixture!("violet_read--external_ref.json"),
];

const MANIFEST: &str = include_str!("../../tests/fixtures/coordination_contract/v1/MANIFEST.json");
const CAPTURED: &str =
    include_str!("../../tests/fixtures/coordination_contract/v1/captured/production-2026-10-16.json");

/// The stable machine-readable outcome and error codes (spec §6; Cloud's
/// deployed set). Coordination ports branch on these, never on prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CoordinationCode {
    Acquired,
    Completed,
    Held,
    Rejected,
    Pending,
    OutcomeUnknown,
    StaleGeneration,
    IdempotencyConflict,
    CoordinationScopeRequired,
    StrictActivationRequired,
    AuthorityUnavailable,
    UnsupportedVersion,
    InvalidRequest,
    NotFound,
}

const RECEIPT_FIELDS: &[&str] = &[
    "operation_id",
    "request_id",
    "resource",
    "visible_owner",
    "generation",
    "expires_at",
    "observed_at",
    "next_action",
];

fn parsed() -> Vec<(&'static str, Value)> {
    FIXTURES
        .iter()
        .map(|(name, text)| {
            let value: Value = serde_json::from_str(text)
                .unwrap_or_else(|error| panic!("{name} is not JSON: {error}"));
            (*name, value)
        })
        .collect()
}

fn cases(doc: &Value) -> &Vec<Value> {
    doc["cases"].as_array().expect("cases")
}

/// Every fixture is a `cassy.coordination/v1` surface with cases whose
/// response codes, where present, are stable codes and whose receipts carry
/// every receipt field (§7).
#[test]
fn every_fixture_is_a_v1_surface_with_stable_codes_and_full_receipts() {
    for (name, doc) in parsed() {
        assert_eq!(doc["contract"], CONTRACT, "{name}");
        for key in ["surface", "method", "path", "spec"] {
            assert!(doc[key].as_str().is_some_and(|value| !value.is_empty()), "{name}: {key}");
        }
        assert!(!cases(&doc).is_empty(), "{name}: no cases");
        for case in cases(&doc) {
            let label = format!("{name} / {}", case["name"]);
            assert!(case["name"].as_str().is_some(), "{label}: name");
            assert!(case["request"].is_object(), "{label}: request");
            let status = case["response"]["status"].as_u64().expect("status");
            assert!((200..600).contains(&status), "{label}: status {status}");
            let payload = &case["response"]["payload"];
            let Some(code) = payload.get("code") else {
                continue;
            };
            // pin: the schema string is the wire protocol's version identity.
            assert_eq!(payload["schema"], CONTRACT, "{label}: schema");
            serde_json::from_value::<CoordinationCode>(code.clone())
                .unwrap_or_else(|_| panic!("{label}: unknown code {code}"));
            let receipt = payload["receipt"].as_object().unwrap_or_else(|| panic!("{label}: receipt"));
            for field in RECEIPT_FIELDS {
                assert!(receipt.contains_key(*field), "{label}: receipt lacks {field}");
            }
            if let Some(rejected_as) = case.get("request_rejected_as") {
                assert_eq!(rejected_as, code, "{label}: request_rejected_as");
            }
        }
    }
}

/// Acceptance: the fixtures cover every stable code, including the eight the
/// task names.
#[test]
fn fixtures_cover_every_stable_code() {
    use std::collections::BTreeSet;
    let seen: BTreeSet<CoordinationCode> = parsed()
        .iter()
        .flat_map(|(_, doc)| cases(doc).clone())
        .filter_map(|case| case["response"]["payload"].get("code").cloned())
        .map(|code| serde_json::from_value(code).expect("stable code"))
        .collect();
    let all = [
        CoordinationCode::Acquired,
        CoordinationCode::Completed,
        CoordinationCode::Held,
        CoordinationCode::Rejected,
        CoordinationCode::Pending,
        CoordinationCode::OutcomeUnknown,
        CoordinationCode::StaleGeneration,
        CoordinationCode::IdempotencyConflict,
        CoordinationCode::CoordinationScopeRequired,
        CoordinationCode::StrictActivationRequired,
        CoordinationCode::AuthorityUnavailable,
        CoordinationCode::UnsupportedVersion,
        CoordinationCode::InvalidRequest,
        CoordinationCode::NotFound,
    ];
    let missing: Vec<_> = all.iter().filter(|code| !seen.contains(code)).collect();
    assert!(missing.is_empty(), "codes with no fixture case: {missing:?}");
}

/// Acceptance: every part-(a) surface the spec names has a fixture, and no
/// part-(b) surface (target operations, ownership view) does.
#[test]
fn fixtures_cover_part_a_and_nothing_of_part_b() {
    let surfaces: Vec<String> = parsed()
        .iter()
        .map(|(_, doc)| doc["surface"].as_str().unwrap().to_string())
        .collect();
    for surface in [
        "external_work.claim",
        "external_work.renew",
        "external_work.release",
        "external_work.complete",
        "external_work.handoff_offer",
        "handoffs.accept",
        "operations.lookup",
        "activity.observe",
        "violet_activity.claim_v2",
        "violet_activity.ack_v2",
        "deliveries.rebind",
        "deliveries.receipts",
        "external_work.reply_intents",
        "violet_read.external_ref",
        "violet_post.managed_reply",
    ] {
        assert!(surfaces.iter().any(|seen| seen == surface), "no fixture for {surface}");
    }
    for (name, text) in FIXTURES {
        for part_b in ["target-operations", "target_operation", "/api/coordination/ownership"] {
            assert!(!text.contains(part_b), "{name} names a part (b) surface: {part_b}");
        }
    }
}

/// Cloud's files are adopted as they are: each vendored file still has the
/// sha256 the manifest pinned at the Cloud ref.
#[test]
fn vendored_cloud_fixtures_are_unchanged() {
    let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    let pinned = manifest["vendored"]["files"].as_object().unwrap();
    assert_eq!(pinned.len(), 14, "Cloud ships 14 part-(a) files at this ref");
    for (name, sha) in pinned {
        let (_, text) = FIXTURES
            .iter()
            .find(|(fixture, _)| fixture == name)
            .unwrap_or_else(|| panic!("{name} is pinned but not embedded"));
        let actual = hex::encode(Sha256::digest(text.as_bytes()));
        assert_eq!(&actual, sha.as_str().unwrap(), "{name} differs from Cloud's file");
    }
}

/// The Violet tool fixtures carry what Violet must enforce: a managed reply
/// names its intent, generation and request id, and permalink variants of
/// one thread resolve to one root identity.
#[test]
fn violet_tool_fixtures_bind_replies_and_converge_identities() {
    let docs = parsed();
    let post = &docs.iter().find(|(name, _)| *name == "violet_post--managed_reply.json").unwrap().1;
    let managed: Vec<&Value> = cases(post)
        .iter()
        .filter(|case| case["request"]["payload"].get("coordination_operation_id").is_some())
        .collect();
    assert!(managed.len() >= 4, "managed reply cases");
    for case in &managed {
        let payload = &case["request"]["payload"];
        assert!(payload["expected_generation"].is_u64(), "{}", case["name"]);
        assert!(payload["request_id"].is_string(), "{}", case["name"]);
    }
    let omitted = cases(post)
        .iter()
        .find(|case| case["name"].as_str().unwrap().contains("omitting"))
        .unwrap();
    assert_eq!(omitted["response"]["payload"]["code"], "rejected");

    let read = &docs.iter().find(|(name, _)| *name == "violet_read--external_ref.json").unwrap().1;
    let roots: std::collections::BTreeSet<String> = cases(read)
        .iter()
        .filter_map(|case| case["response"]["payload"].get("thread_identity"))
        .map(|identity| {
            format!(
                "{}/{}/{}",
                identity["workspace_id"], identity["channel_id"], identity["root_ts"]
            )
        })
        .collect();
    assert_eq!(roots.len(), 1, "root and reply permalinks converge: {roots:?}");
}

/// The production capture is the evidence the fixtures were checked against:
/// the coordination routes answer in this contract with a full receipt, and
/// the v1 activity relay is unchanged. It holds no credential.
#[test]
fn production_capture_matches_the_contract_envelope() {
    let captured: Value = serde_json::from_str(CAPTURED).unwrap();
    let probes = captured["probes"].as_object().unwrap();
    let claim = &probes["coordination_external_work_claim_not_deployed"]["response"];
    assert_eq!(claim["body"]["schema"], CONTRACT);
    assert_eq!(claim["body"]["code"], "invalid_request");
    let receipt = claim["body"]["receipt"].as_object().unwrap();
    for field in RECEIPT_FIELDS {
        assert!(receipt.contains_key(*field), "captured receipt lacks {field}");
    }
    let v1 = &probes["activity_claim_scratch_project"]["response"];
    assert!(v1["body"]["events"].is_array() && v1["body"]["denied"].is_array());
    let ack = &probes["activity_ack_unknown_event"]["response"]["body"]["results"][0];
    assert_eq!(ack["status"], 404);
    assert!(!CAPTURED.contains("Bearer "), "no credential in the capture");
}
