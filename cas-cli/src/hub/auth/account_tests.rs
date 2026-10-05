//! Account enrollment binding against a real AuthStore (cas-4634).

use super::*;

const ORIGIN: &str = "https://hub.petrastella.io";
const HUB: &str = "test-hub";

struct Paired {
    _root: tempfile::TempDir,
    store: AuthStore,
    credential: DeviceCredential,
    context: AuthContext,
    jkt: String,
}

fn paired() -> Paired {
    let root = tempfile::tempdir().unwrap();
    let store = AuthStore::open(root.path().join("hub"), HUB).unwrap();
    let now = Utc::now();
    let invitation = store
        .mint_pairing(ORIGIN, Scope::default_read_only(), now)
        .unwrap();
    let exchange =
        PairingExchange::test_fixture(invitation.token, HUB, ORIGIN, Scope::default_read_only());
    let credential = store.exchange_pairing(exchange, now).unwrap();
    let context = AuthContext {
        device_id: credential.device_id.clone(),
        credential_id: credential.credential_id.clone(),
        device_label: "test device".into(),
        operator_label: "test operator".into(),
        controller_origin: ORIGIN.into(),
        scopes: credential.scopes.clone(),
        request_id: "test".into(),
    };
    let jkt = store.list_devices().unwrap()[0].key_fingerprint.clone();
    Paired {
        _root: root,
        store,
        credential,
        context,
        jkt,
    }
}

fn assertion(paired: &Paired, challenge: &str) -> EnrollmentAssertion {
    EnrollmentAssertion {
        issuer_kid: "op-2026-10-05".into(),
        aud: format!("cas-hub:{HUB}"),
        hub: HUB.into(),
        account_id: "acct-1".into(),
        relay_device_id: "dev-relay-1".into(),
        origin: ORIGIN.into(),
        installation_jkt: paired.jkt.clone(),
        challenge: challenge.into(),
        grant_generation: "1".into(),
        feed_generation: "1".into(),
        epoch: "4".into(),
        expires_at: Utc::now() + Duration::seconds(300),
    }
}

fn enrollment(paired: &Paired) -> AccountEnrollment {
    paired.store.list_devices().unwrap()[0]
        .account_enrollment
        .clone()
}

#[test]
fn a_valid_assertion_enrolls_exactly_this_installation_and_survives_refresh() {
    let paired = paired();
    assert_eq!(enrollment(&paired), AccountEnrollment::Unenrolled);
    let (challenge, expires) = paired
        .store
        .issue_account_challenge(&paired.context, Utc::now())
        .unwrap();
    assert!(expires <= Utc::now() + Duration::seconds(CHALLENGE_TTL_SECONDS));
    let bound = paired
        .store
        .bind_account(
            &paired.context,
            &assertion(&paired, &challenge),
            Some("acct-1"),
            Utc::now(),
        )
        .unwrap()
        .unwrap();
    assert!(matches!(
        &bound,
        AccountEnrollment::Enrolled { account_id, relay_device_id, epoch, .. }
            if account_id == "acct-1" && relay_device_id == "dev-relay-1" && epoch == "4"
    ));
    assert_eq!(enrollment(&paired), bound);
    let wire = serde_json::to_value(&bound).unwrap();
    assert_eq!(wire["state"], "enrolled");
    assert_eq!(wire["account_id"], "acct-1");

    // Durable: another process opening the same state sees the binding.
    let reopened = AuthStore::open(paired.store.state_dir(), HUB).unwrap();
    assert_eq!(
        reopened.list_devices().unwrap()[0].account_enrollment,
        bound
    );

    // A revoke ends it.
    paired
        .store
        .revoke_device(&paired.credential.device_id, Utc::now())
        .unwrap();
    assert_eq!(enrollment(&paired), AccountEnrollment::Unenrolled);
}

#[test]
fn a_challenge_is_one_use_even_when_the_assertion_is_refused() {
    let paired = paired();
    let (challenge, _) = paired
        .store
        .issue_account_challenge(&paired.context, Utc::now())
        .unwrap();
    let mut wrong = assertion(&paired, &challenge);
    wrong.account_id = "acct-other".into();
    assert_eq!(
        paired
            .store
            .bind_account(&paired.context, &wrong, Some("acct-1"), Utc::now())
            .unwrap(),
        Err(EnrollmentRefusal::AccountMismatch)
    );
    assert_eq!(
        paired
            .store
            .bind_account(
                &paired.context,
                &assertion(&paired, &challenge),
                Some("acct-1"),
                Utc::now()
            )
            .unwrap(),
        Err(EnrollmentRefusal::ChallengeUnknown),
        "the refused attempt spent the challenge"
    );
    assert_eq!(enrollment(&paired), AccountEnrollment::Unenrolled);
}

#[test]
fn every_binding_check_refuses_with_its_own_reason() {
    type Tamper = fn(&mut EnrollmentAssertion);
    let cases: [(Tamper, EnrollmentRefusal); 5] = [
        (
            |a| a.aud = "cas-hub:other-hub".into(),
            EnrollmentRefusal::WrongHub,
        ),
        (|a| a.hub = "other-hub".into(), EnrollmentRefusal::WrongHub),
        (
            |a| a.expires_at = Utc::now() - Duration::seconds(1),
            EnrollmentRefusal::AssertionExpired,
        ),
        (
            |a| a.installation_jkt = "another-installation-key".into(),
            EnrollmentRefusal::InstallationMismatch,
        ),
        (
            |a| a.origin = "https://evil.example".into(),
            EnrollmentRefusal::OriginMismatch,
        ),
    ];
    for (tamper, expected) in cases {
        let paired = paired();
        let (challenge, _) = paired
            .store
            .issue_account_challenge(&paired.context, Utc::now())
            .unwrap();
        let mut claims = assertion(&paired, &challenge);
        tamper(&mut claims);
        assert_eq!(
            paired
                .store
                .bind_account(&paired.context, &claims, Some("acct-1"), Utc::now())
                .unwrap(),
            Err(expected)
        );
        assert_eq!(enrollment(&paired), AccountEnrollment::Unenrolled);
    }

    // An unenrolled hub can never bind; an expired challenge is refused.
    let paired = paired();
    let (challenge, _) = paired
        .store
        .issue_account_challenge(&paired.context, Utc::now())
        .unwrap();
    assert_eq!(
        paired
            .store
            .bind_account(
                &paired.context,
                &assertion(&paired, &challenge),
                None,
                Utc::now()
            )
            .unwrap(),
        Err(EnrollmentRefusal::HubNotEnrolled)
    );
    let (late, _) = paired
        .store
        .issue_account_challenge(&paired.context, Utc::now())
        .unwrap();
    assert_eq!(
        paired
            .store
            .bind_account(
                &paired.context,
                &assertion(&paired, &late),
                Some("acct-1"),
                Utc::now() + Duration::seconds(CHALLENGE_TTL_SECONDS + 1)
            )
            .unwrap(),
        Err(EnrollmentRefusal::ChallengeExpired)
    );
}

#[test]
fn a_challenge_issued_to_another_device_cannot_be_used() {
    let paired = paired();
    let (challenge, _) = paired
        .store
        .issue_account_challenge(&paired.context, Utc::now())
        .unwrap();
    let mut other = paired.context.clone();
    other.credential_id = "another-credential".into();
    // The other context is not an active session at all.
    assert!(
        paired
            .store
            .bind_account(
                &other,
                &assertion(&paired, &challenge),
                Some("acct-1"),
                Utc::now()
            )
            .is_err()
    );
    // The challenge was not consumed by the refused session.
    assert!(
        paired
            .store
            .bind_account(
                &paired.context,
                &assertion(&paired, &challenge),
                Some("acct-1"),
                Utc::now()
            )
            .unwrap()
            .is_ok()
    );
}
