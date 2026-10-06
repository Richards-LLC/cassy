use super::*;
use crate::hub::operator_inbox::jws::test_issuer::TestIssuer;
use crate::hub::operator_inbox::jws::{TYP_MACHINE_BINDING, claim_str};
use serde_json::json;

fn keys(issuer: &Arc<TestIssuer>) -> IssuerKeys {
    let issuer = Arc::clone(issuer);
    IssuerKeys::new(Box::new(move || Ok(issuer.jwks())))
}

fn claims(now: i64) -> Value {
    json!({
        "iss": "https://psc.test", "aud": "cas-hub:hub-1", "sub": "dev-relay-1", "acct": "acct-1",
        "hub": "hub-1", "projects": ["p1"], "cmd_scopes": [], "origin": "https://hub.petrastella.io",
        "ins_jkt": "installation-jkt", "dev_jkt": "relay-jkt", "gen": "2", "cak": "op-test-1",
        "fgen": "1", "epoch": "4", "chl": "challenge-1", "iat": now, "exp": now + 300, "jti": "j1"
    })
}

#[test]
fn decodes_a_signed_enrollment_assertion() {
    let issuer = Arc::new(TestIssuer::new("op-test-1"));
    let now = Utc::now();
    let token = issuer.sign(TYP_ENROLLMENT, claims(now.timestamp()));
    let assertion = verify_enrollment_assertion(&keys(&issuer), &token, now).unwrap();
    assert_eq!(assertion.aud, "cas-hub:hub-1");
    assert_eq!(assertion.account_id, "acct-1");
    assert_eq!(assertion.relay_device_id, "dev-relay-1");
    assert_eq!(assertion.installation_jkt, "installation-jkt");
    assert_eq!(assertion.challenge, "challenge-1");
    assert_eq!(
        (
            assertion.grant_generation.as_str(),
            assertion.epoch.as_str()
        ),
        ("2", "4")
    );
    assert_eq!(assertion.issuer_kid, "op-test-1");
}

#[test]
fn refuses_another_token_type_an_expired_token_and_a_foreign_cak() {
    let issuer = Arc::new(TestIssuer::new("op-test-1"));
    let now = Utc::now();
    let binding = issuer.sign(TYP_MACHINE_BINDING, claims(now.timestamp()));
    assert!(matches!(
        verify_enrollment_assertion(&keys(&issuer), &binding, now),
        Err(JwsError::WrongType(_))
    ));
    let expired = issuer.sign(TYP_ENROLLMENT, claims(now.timestamp() - 600));
    assert_eq!(
        verify_enrollment_assertion(&keys(&issuer), &expired, now).err(),
        Some(JwsError::Expired)
    );
    let mut foreign = claims(now.timestamp());
    foreign["cak"] = json!("op-other");
    let token = issuer.sign(TYP_ENROLLMENT, foreign);
    assert_eq!(
        verify_enrollment_assertion(&keys(&issuer), &token, now).err(),
        Some(JwsError::Malformed("cak"))
    );
    let mut missing = claims(now.timestamp());
    missing.as_object_mut().unwrap().remove("ins_jkt");
    let token = issuer.sign(TYP_ENROLLMENT, missing);
    assert_eq!(
        verify_enrollment_assertion(&keys(&issuer), &token, now).err(),
        Some(JwsError::Malformed("ins_jkt"))
    );
    let stranger = TestIssuer::new("op-test-1");
    let forged = stranger.sign(TYP_ENROLLMENT, claims(now.timestamp()));
    assert_eq!(
        verify_enrollment_assertion(&keys(&issuer), &forged, now).err(),
        Some(JwsError::BadSignature)
    );
    // The claims helper still reads a verified binding's claims.
    let verified = keys(&issuer)
        .verify(&binding, TYP_MACHINE_BINDING, now.timestamp())
        .unwrap();
    assert_eq!(claim_str(&verified, "acct"), Ok("acct-1"));
}

#[test]
fn an_unenrolled_hub_verifies_nothing() {
    struct NoNetwork;
    impl HttpClient for NoNetwork {
        fn send(
            &self,
            _method: &str,
            _url: &str,
            _headers: &[(&str, String)],
            _body: &[u8],
        ) -> Result<super::super::machine::HttpResponse, super::super::Failure> {
            panic!("an unenrolled hub must not fetch a JWKS");
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let verifier = HubVerifier::new(dir.path(), Arc::new(NoNetwork));
    assert_eq!(
        verifier.verify("a.b.c", Utc::now()).err(),
        Some(AssertionError::HubNotEnrolled)
    );
    assert_eq!(AssertionError::HubNotEnrolled.code(), "hub_not_enrolled");
}
