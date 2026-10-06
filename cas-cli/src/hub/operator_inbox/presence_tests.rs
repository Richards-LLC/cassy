use super::*;

fn reporter() -> Reporter {
    Reporter::new(
        "machine".into(),
        "boot".into(),
        "process".into(),
        "0".into(),
    )
}

fn activation(epoch: &str, generation: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "wire_version": 1, "machine_id": "machine", "reporter_epoch": epoch, "monitoring_generation": generation,
        "outcome": "activated", "heartbeat_interval_s": 60, "heartbeat_jitter_s": 10,
        "lease_s": 180, "grace_s": 60, "max_silence_s": 86400,
    }))
    .unwrap()
}

fn activated() -> Reporter {
    let mut r = reporter();
    r.prepare(vec![], None).unwrap();
    assert_eq!(r.receive(200, &activation("1", "1")), Some("1".into()));
    r
}

fn response(r: &mut Reporter, error: &str) {
    r.receive(409, &serde_json::to_vec(&json!({"error":error})).unwrap());
}

#[test]
fn presence_unanswered_report_reuses_identical_body_and_sequence() {
    let mut r = activated();
    let original = r
        .prepare(vec![], Silence::new(SilenceKind::Sleep, 600))
        .unwrap()
        .body
        .clone();
    r.receive(503, b"not JSON");
    let retry = r
        .prepare(
            vec![Component {
                component: ComponentName::Hub,
                state: ComponentState::Down,
                age_s: 0,
            }],
            None,
        )
        .unwrap();
    assert_eq!(retry.body, original);
    r.receive(
        200,
        br#"{"wire_version":1,"receipt":{"reporter_epoch":"1","seq":"1","outcome":"duplicate"}}"#,
    );
    let fresh: Value = serde_json::from_slice(&r.prepare(vec![], None).unwrap().body).unwrap();
    assert_eq!(fresh["seq"], "2");
    assert!(fresh.get("silence").is_none());
}

#[test]
fn presence_activation_conflict_retries_once_but_fenced_heartbeat_never_activates() {
    let mut r = reporter();
    r.prepare(vec![], None);
    r.receive(
        409,
        br#"{"error":"reporter_epoch_conflict","current_reporter_epoch":"7"}"#,
    );
    let next: Value = serde_json::from_slice(&r.prepare(vec![], None).unwrap().body).unwrap();
    assert_eq!(next["previous_reporter_epoch"], "7");
    r.receive(
        409,
        br#"{"error":"reporter_epoch_conflict","current_reporter_epoch":"8"}"#,
    );
    assert!(r.prepare(vec![], None).is_none());

    let mut r = activated();
    r.prepare(vec![], None);
    response(&mut r, "reporter_epoch_conflict");
    assert!(r.prepare(vec![], None).is_none());
    assert!(matches!(r.phase, Phase::Stopped("reporter_fenced")));
}

#[test]
fn presence_generation_change_activates_but_preserves_seq_for_existing_epoch() {
    let mut r = activated();
    r.prepare(vec![], None);
    response(&mut r, "monitoring_generation_conflict");
    assert_eq!(r.prepare(vec![], None).unwrap().path, ACTIVATE);
    assert_eq!(r.receive(200, &activation("1", "3")), Some("1".into()));
    let next: Value = serde_json::from_slice(&r.prepare(vec![], None).unwrap().body).unwrap();
    assert_eq!(next["seq"], "2");
    assert_eq!(next["monitoring_generation"], "3");
}

#[test]
fn presence_stale_report_advances_seq_with_fresh_observations() {
    let mut r = activated();
    r.prepare(vec![], None);
    response(&mut r, "presence_report_stale");
    let next: Value = serde_json::from_slice(
        &r.prepare(
            vec![Component {
                component: ComponentName::Serve,
                state: ComponentState::Degraded,
                age_s: 1,
            }],
            None,
        )
        .unwrap()
        .body,
    )
    .unwrap();
    assert_eq!(next["seq"], "2");
    assert_eq!(
        next["components"][0],
        json!({"component":"serve","state":"degraded","age_s":1})
    );
}

#[test]
fn presence_disable_revoke_and_unknown_refusal_stop_this_process() {
    for (status, body) in [
        (409, br#"{"error":"monitoring_disabled"}"#.as_slice()),
        (401, br#"{"error":"grant_revoked"}"#.as_slice()),
        (403, br#"{"error":"capability_required"}"#.as_slice()),
        (409, br#"{"error":"presence_sequence_conflict"}"#.as_slice()),
    ] {
        let mut r = activated();
        r.prepare(vec![], None);
        r.receive(status, body);
        assert!(r.prepare(vec![], None).is_none());
    }
}

#[test]
fn presence_receipt_must_bind_epoch_seq_version_and_policy() {
    let mut r = activated();
    r.prepare(vec![], None);
    r.receive(
        200,
        br#"{"wire_version":1,"receipt":{"reporter_epoch":"2","seq":"1","outcome":"accepted"}}"#,
    );
    assert!(r.prepare(vec![], None).is_none());
    let mut r = reporter();
    r.prepare(vec![], None);
    let mut invalid: Value = serde_json::from_slice(&activation("1", "1")).unwrap();
    invalid["lease_s"] = json!(0);
    assert_eq!(r.receive(200, &serde_json::to_vec(&invalid).unwrap()), None);
    assert!(r.prepare(vec![], None).is_none());
}

#[test]
fn presence_silence_and_positions_are_bounded_closed_values() {
    assert!(Silence::new(SilenceKind::Maintenance, 86_400).is_some());
    assert!(Silence::new(SilenceKind::Reboot, 0).is_none());
    assert!(Silence::new(SilenceKind::Sleep, 86_401).is_none());
    for value in [
        json!("01"),
        json!("-1"),
        json!("9223372036854775808"),
        json!(1),
    ] {
        assert!(position(&value).is_none());
    }
    assert_eq!(position(&json!("0")), Some("0".into()));
}

#[test]
fn presence_epoch_is_atomic_and_bound_to_cloud_account_and_machine() {
    let root = tempfile::tempdir().unwrap();
    let mut principal: MachinePrincipal = serde_json::from_value(json!({
        "wire_version":1, "cloud_origin":"https://cloud.example", "account_id":"account", "machine_id":"machine",
        "hub_id":"hub", "grant_id":"grant", "grant_generation":"1", "feed_generation":"1", "active_epoch":"1",
        "label":"Atlas", "projects":[], "capabilities":["presence:report"], "signing_secret":"", "command_secret":"",
        "command_public":"", "command_key_id":"", "enrolled_at":"2026-10-06T00:00:00Z",
    })).unwrap();
    assert_eq!(load_epoch(root.path(), &principal).unwrap(), "0");
    save_epoch(root.path(), &principal, "7".into()).unwrap();
    assert_eq!(load_epoch(root.path(), &principal).unwrap(), "7");
    principal.machine_id = "replacement".into();
    assert_eq!(load_epoch(root.path(), &principal).unwrap(), "0");
    assert_eq!(
        fs::read_dir(root.path().join("operator-inbox"))
            .unwrap()
            .count(),
        1
    );
    fs::write(
        root.path().join("operator-inbox").join(EPOCH_FILE),
        b"truncated",
    )
    .unwrap();
    assert!(load_epoch(root.path(), &principal).is_err());
}
