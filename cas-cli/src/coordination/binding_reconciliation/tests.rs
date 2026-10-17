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
    repeat["latest_attempt"]["settled"] =
        json!({"migrated_v1_rows": 0, "retired_v1_rows": 0, "invalidated_v1_leases": 0});
    repeat["settled"] = json!({"migrated": 0, "retired": 0, "invalidated_leases": 0});
    // Managed repeats settle strays without repeating the original quiet gate.
    repeat["latest_attempt"]["quiescence"]["blocking_v1_consumers"] = json!(["peer:recent-poll"]);
    let repeated = parse(&repeat, 200).expect("repeat retains independent original evidence");
    assert_eq!(repeated.settled.as_ref().unwrap().migrated, 0);
    assert_ne!(
        repeated.activation_id.as_ref(),
        repeated.fence_receipt.as_ref().map(|r| &r.id)
    );
    assert_eq!(first.fence_receipt, repeated.fence_receipt);

    for (pointer, replacement) in [
        ("/schema", json!("cassy.binding-reconciliation/v2")),
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
        ("/fence_receipt/prior_mode", json!("managed")),
        ("/fence_receipt/quiescence/blocking_v1_leases", json!(1)),
        (
            "/fence_receipt/quiescence/blocking_v1_consumers",
            json!(["peer:old-reader"]),
        ),
        ("/fence_receipt/fence_generation", json!(8)),
        (
            "/binding/fence_generation",
            json!(9_007_199_254_740_992_u64),
        ),
        ("/settled/migrated", json!(-1)),
        (
            "/binding/activated_at",
            json!("2026-10-17T18:00:00.000+01:00"),
        ),
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

    for field in [
        "scope",
        "binding",
        "strict_activation",
        "activation_id",
        "fence_receipt",
        "latest_attempt",
        "settled",
        "blockers",
        "retry_after_secs",
    ] {
        let mut missing = original.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            parse(&missing, 200).is_err(),
            "missing nullable field {field} was accepted"
        );
    }
    for pointer in ["", "/scope", "/fence_receipt/quiescence"] {
        let mut extra = original.clone();
        extra
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("trusted".into(), json!(true));
        assert!(
            parse(&extra, 200).is_err(),
            "unknown field accepted at {pointer}"
        );
    }
    assert!(
        parse(&original, 201).is_err(),
        "only HTTP200 completes this operation"
    );
    assert!(target().parse_response(204, b"").is_err());
    assert!(
        target()
            .parse_response(200, b"<html>upstream failure</html>")
            .is_err()
    );
}

#[test]
fn request_serializes_only_valid_identity_without_authority_claims() {
    // pin: these are spec-b489's complete caller fields; no trusted/exemption/admin input.
    assert_eq!(
        serde_json::to_value(target()).unwrap(),
        json!({
            "schema": "cassy.binding-reconciliation/v1", "team_id": "team-fixture",
            "project_id": "github.com/example/project", "channel_id": "C0123456789"
        })
    );
    for (team, project, channel) in [
        ("", "project", "C012"),
        ("team", " \t", "C012"),
        ("te\0am", "project", "C012"),
        ("team", "pro\0ject", "C012"),
        ("team", "project", "D012"),
        ("team", "project", "C12lower"),
        ("team", "project", "C1"),
    ] {
        assert!(
            ReconciliationRequest::new(team, project, channel).is_err(),
            "invalid target {team:?}/{project:?}/{channel:?}"
        );
    }
    assert!(ReconciliationRequest::new(&"a".repeat(257), "project", "C012").is_err());
    assert!(ReconciliationRequest::new("team", &"a".repeat(513), "C012").is_err());
    assert!(
        ReconciliationRequest::new("team", "project", &format!("C{}", "1".repeat(31))).is_err()
    );
    // Cloud's existing validators measure UTF16 lengths, without normalizing identities.
    assert!(ReconciliationRequest::new(&"😀".repeat(128), &"😀".repeat(256), "G012").is_ok());
    assert!(ReconciliationRequest::new(&"😀".repeat(129), "project", "C012").is_err());
}

#[test]
fn refusals_preserve_blockers_and_keep_unknown_attempts_unresolved() {
    let refusal: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/binding_reconciliation/v1/legacy-intake-active.json"
    ))
    .unwrap();
    let parsed = parse(&refusal, 409).expect("supported legacy quiet-window refusal");
    assert_eq!(parsed.code, ReconciliationCode::LegacyIntakeActive);
    assert_eq!(parsed.next_action, NextAction::StopLegacyIntake);
    assert_eq!(parsed.retry_after_secs, Some(300));
    let blockers = parsed.blockers.unwrap();
    assert_eq!(blockers.unexpired_v1_leases, 2);
    assert_eq!(blockers.live_v1_consumers, ["peer:old-reader"]);
    assert!(!blockers.consumers_truncated);

    let mut unknown = refusal.clone();
    for field in [
        "binding",
        "strict_activation",
        "activation_id",
        "fence_receipt",
        "latest_attempt",
        "settled",
        "blockers",
        "retry_after_secs",
    ] {
        unknown[field] = Value::Null;
    }
    unknown["code"] = json!("authority_unavailable");
    unknown["reason"] = json!("activation_outcome_unknown");
    unknown["next_action"] = json!("retry_later");
    let unresolved = parse(&unknown, 503).expect("valid unknown mutation outcome");
    assert_eq!(unresolved.code, ReconciliationCode::AuthorityUnavailable);
    assert_eq!(
        serde_json::to_value(unresolved).unwrap()["reason"],
        "activation_outcome_unknown"
    );
    assert!(
        parse(&unknown, 200).is_err(),
        "HTTP/code mismatch cannot become completion"
    );

    let mut unauthenticated = unknown.clone();
    unauthenticated["scope"] = Value::Null;
    unauthenticated["code"] = json!("unauthorized");
    unauthenticated["reason"] = json!("invalid_credential");
    unauthenticated["next_action"] = json!("authenticate");
    assert_eq!(
        parse(&unauthenticated, 401).unwrap().next_action,
        NextAction::Authenticate
    );

    for (pointer, replacement) in [
        ("/reason", json!("new_unknown_reason")),
        ("/code", json!("completed")),
        ("/next_action", json!("none")),
        ("/blockers", Value::Null),
        ("/blockers/live_v1_consumers_count", json!(0)),
        ("/blockers/consumers_truncated", json!(true)),
        ("/latest_attempt/outcome", json!("activated")),
    ] {
        let mut corrupted = refusal.clone();
        *corrupted.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            parse(&corrupted, 409).is_err(),
            "accepted malformed refusal at {pointer}"
        );
    }
}
