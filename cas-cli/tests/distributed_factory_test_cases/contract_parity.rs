//! cas-c7c7: the in-process fake cloud answers every recorded production
//! exchange (cas-2911 fixtures) with the same status, keys and literals.

use super::cloud_contract_replay::{Exchange, Replay, fixtures};
use super::fake_cloud::FakeCloud;

/// The capture's own order: agents, claims and peer messages, then the
/// five-minute-later stale listing, then teardown.
fn production_order() -> Vec<Exchange> {
    let agents = fixtures("agents");
    let (stale, agents): (Vec<_>, Vec<_>) =
        agents.into_iter().partition(|exchange| exchange.name == "list_project_stale");
    agents
        .into_iter()
        .chain(fixtures("claims"))
        .chain(fixtures("peer_messages"))
        .chain(stale)
        .chain(fixtures("teardown"))
        .collect()
}

#[test]
fn contract_parity_fake_cloud_matches_production() {
    let cloud = FakeCloud::start();
    let mut replay = Replay::new(&cloud.url, "0c7c7a11");
    for mut exchange in production_order() {
        // The immutable PR #153 capture predates migration 0036 (#155).
        // Overlay only its documented additive owner/scope fields; preserve
        // the recorded keys and literals for the rest of the protocol.
        amend_team_lock_shape(&mut exchange.response);
        match exchange.name.as_str() {
            // The capture waited five minutes, then listed stale agents.
            "list_project_stale" => cloud.advance_clock(310),
            // The capture claimed for one second and waited for it to run out.
            "claim_takeover_after_expiry" => cloud.advance_clock(3),
            _ => {}
        }
        replay.check(&exchange);
    }
    replay.assert_clean();
}

fn amend_team_lock_shape(value: &mut serde_json::Value) {
    use serde_json::{Value, json};
    match value {
        Value::Object(object) => {
            if object.contains_key("task_id") && object.contains_key("claim_reason") {
                object.insert("user_id".into(), json!("<user>"));
                object.insert("project_id".into(), json!("<project>"));
                object.insert("team_id".into(), Value::Null);
            }
            if object.get("error").and_then(Value::as_str) == Some("already_claimed") {
                object.insert("user_id".into(), json!("<user>"));
            }
            for value in object.values_mut() {
                amend_team_lock_shape(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                amend_team_lock_shape(value);
            }
        }
        _ => {}
    }
}
