use super::*;
use serde_json::{Value, json};

fn target() -> ReconciliationRequest {
    ReconciliationRequest::new("team-fixture", "github.com/example/project", "C0123456789").unwrap()
}

fn completed() -> Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/binding_reconciliation/v1/completed.json"
    ))
    .unwrap()
}

fn parse(value: &Value, status: u16) -> Result<ReconciliationResponse, WireError> {
    target().parse_response(status, &serde_json::to_vec(value).unwrap())
}

#[test]
fn completed_requires_matching_original_fence_evidence() {
    let original = completed();
    let first = parse(&original, 200).expect("evidenced ordinary activation");
    assert_eq!(first.code, ReconciliationCode::Completed);
    assert_eq!(first.settled.unwrap().migrated, 2);

    // A repeat has its own attempt, while preserving the original fence receipt.
    let mut repeat = original.clone();
    repeat["reason"] = json!("already_managed");
    repeat["activation_id"] = json!("33333333-3333-4333-8333-333333333333");
    repeat["latest_attempt"]["id"] = repeat["activation_id"].clone();
    repeat["latest_attempt"]["outcome"] = json!("already_managed");
    repeat["latest_attempt"]["prior_mode"] = json!("managed");
    let repeated = parse(&repeat, 200).expect("repeat retains independent original evidence");
    assert_ne!(
        repeated.activation_id.as_ref(),
        repeated.fence_receipt.as_ref().map(|r| &r.id)
    );
    assert_eq!(first.fence_receipt, repeated.fence_receipt);

    for (pointer, replacement) in [
        ("/scope/team_id", json!("foreign-team")),
        ("/binding/project_id", json!("github.com/foreign/project")),
        ("/binding/workspace_id", json!("T9999999999")),
        ("/scope/channel_id", json!("C9999999999")),
        ("/binding/mode", json!("legacy")),
        ("/binding/verified", json!(false)),
        ("/binding/activated_at", Value::Null),
        ("/strict_activation", json!("unevidenced")),
        ("/fence_receipt", Value::Null),
        ("/fence_receipt/outcome", json!("already_managed")),
        ("/fence_receipt/fence_generation", json!(8)),
        (
            "/latest_attempt/id",
            json!("44444444-4444-4444-8444-444444444444"),
        ),
        (
            "/fence_receipt/quiescence/attested_v1_consumers",
            json!(["exempt-reader"]),
        ),
    ] {
        let mut corrupted = original.clone();
        *corrupted.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            parse(&corrupted, 200).is_err(),
            "accepted corrupt evidence at {pointer}"
        );
    }
}
