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
    for exchange in production_order() {
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
