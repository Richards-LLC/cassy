//! Machine enrollment and the PSC-PoP relay transport against an in-process
//! cloud that checks the §4.2 and §5.4 rules the hub depends on: bearer on
//! account routes, PoP signature/kid/aud/htm/htp/bdg/gid/gen, the one-use
//! enrollment check, and a machine binding signed by a test issuer. It is a
//! protocol double; the deployed-cloud journey is the acceptance gate (S5).

use super::*;
use crate::hub::operator_inbox::jws::test_issuer::TestIssuer;
use crate::hub::operator_inbox::{MachineRelay, Position, SealedEvent};
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use std::collections::HashSet;

const BEARER: &str = "psc_k_test_bearer_never_real";
const ORIGIN: &str = "https://psc.test";

#[derive(Default)]
struct CloudState {
    enrollment: Option<(String, Value, Vec<u8>, String)>,
    machine_jwk: Option<Value>,
    machine_id: String,
    generation: u64,
    nonces: HashSet<String>,
    appended: Vec<Value>,
    skew_seconds: i64,
    approvals: Vec<Value>,
    log: Vec<(String, String, u16)>,
}

struct FakeCloud {
    issuer: TestIssuer,
    state: Mutex<CloudState>,
    /// Bind the issued binding to this command key instead (substitution test).
    substitute_cmd_pk: Option<String>,
}

fn reply(status: u16, body: Value) -> HttpResponse {
    HttpResponse {
        status,
        body: serde_json::to_vec(&body).unwrap(),
        date: Some(chrono::Utc::now().to_rfc2822()),
        retry_after_s: None,
    }
}

fn header<'a>(headers: &'a [(&str, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

impl FakeCloud {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            issuer: TestIssuer::new("op-test-1"),
            state: Mutex::new(CloudState {
                generation: 1,
                ..CloudState::default()
            }),
            substitute_cmd_pk: None,
        })
    }

    fn verify_pop(
        &self,
        state: &mut CloudState,
        jwk: &Value,
        method: &str,
        path: &str,
        body: &[u8],
        proof: &str,
    ) -> Result<serde_json::Map<String, Value>, &'static str> {
        let parts: Vec<&str> = proof.split('.').collect();
        if parts.len() != 3 {
            return Err("pop_required");
        }
        let header: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        let claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        let x = jwk["x"].as_str().unwrap();
        let y = jwk["y"].as_str().unwrap();
        if header["typ"] != "psc-op-pop+jwt" || header.get("jwk").is_some() {
            return Err("pop_invalid");
        }
        if header["kid"].as_str()
            != Some(crate::hub::operator_inbox::jws::thumbprint(x, y).as_str())
        {
            return Err("pop_key_mismatch");
        }
        let mut point = vec![4u8];
        point.extend(URL_SAFE_NO_PAD.decode(x).unwrap());
        point.extend(URL_SAFE_NO_PAD.decode(y).unwrap());
        let key = VerifyingKey::from_sec1_bytes(&point).unwrap();
        let signature = Signature::from_slice(&URL_SAFE_NO_PAD.decode(parts[2]).unwrap()).unwrap();
        if key
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .is_err()
        {
            return Err("pop_invalid");
        }
        if claims["aud"] != format!("{ORIGIN}/api/operator")
            || claims["htm"] != method
            || claims["htp"] != path
            || claims["bdg"] != body_digest(body)
        {
            return Err("pop_binding_mismatch");
        }
        let iat = claims["iat"].as_i64().unwrap();
        let server_now = unix_now() + state.skew_seconds;
        if (iat - server_now).abs() > 60 {
            return Err("pop_expired");
        }
        if !state
            .nonces
            .insert(claims["jti"].as_str().unwrap().to_owned())
        {
            return Err("pop_replay");
        }
        Ok(claims.as_object().unwrap().clone())
    }

    fn handle(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, String)],
        body: &[u8],
    ) -> HttpResponse {
        let path = url.strip_prefix(ORIGIN).unwrap();
        let mut state = self.state.lock().unwrap();
        let bearer_ok =
            header(headers, "Authorization") == Some(format!("Bearer {BEARER}").as_str());
        let request: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let response = match (method, path) {
            ("GET", "/api/operator/jwks") => reply(200, self.issuer.jwks()),
            ("POST", "/api/operator/machines/challenges") => {
                if !bearer_ok {
                    reply(401, json!({"error": "unauthorized"}))
                } else {
                    let pk = URL_SAFE_NO_PAD
                        .decode(request["command_encryption_public_key"].as_str().unwrap())
                        .unwrap();
                    let id = "enr-0001".to_owned();
                    let plain = rand::random::<[u8; 32]>().to_vec();
                    let sealed = cas_operator_crypto::seal(
                        &pk,
                        cas_operator_crypto::ENROLLMENT_CHECK_INFO.as_bytes(),
                        &plain,
                        id.as_bytes(),
                    )
                    .unwrap();
                    state.enrollment = Some((
                        id.clone(),
                        request["signing_jwk"].clone(),
                        plain,
                        request["command_encryption_public_key"]
                            .as_str()
                            .unwrap()
                            .to_owned(),
                    ));
                    reply(
                        201,
                        json!({"wire_version": 1, "enrollment_id": id, "expires_at": "2026-10-05T20:00:00Z",
                            "encryption_key_check": {"enc": URL_SAFE_NO_PAD.encode(&sealed.enc), "ct": URL_SAFE_NO_PAD.encode(&sealed.ciphertext)}}),
                    )
                }
            }
            ("POST", "/api/operator/machines") => {
                let Some((id, jwk, plain, cmd_pk)) = state.enrollment.clone() else {
                    return reply(409, json!({"error": "enrollment_not_found"}));
                };
                if !bearer_ok {
                    reply(401, json!({"error": "unauthorized"}))
                } else {
                    match self.verify_pop(
                        &mut state,
                        &jwk,
                        method,
                        path,
                        body,
                        header(headers, "PSC-PoP-Proof").unwrap_or(""),
                    ) {
                        Err(code) => reply(401, json!({"error": code})),
                        Ok(claims) if claims.get("enr") != Some(&json!(id)) => {
                            reply(401, json!({"error": "pop_binding_mismatch"}))
                        }
                        Ok(_)
                            if request["encryption_key_check"].as_str()
                                != Some(URL_SAFE_NO_PAD.encode(&plain).as_str()) =>
                        {
                            reply(403, json!({"error": "encryption_key_check_failed"}))
                        }
                        Ok(_) => {
                            state.machine_jwk = Some(jwk.clone());
                            state.machine_id = "4f1c0a52-5d1e-4b7a-9c43-2a7e0f6b1d01".into();
                            state.enrollment = None;
                            let jkt = crate::hub::operator_inbox::jws::thumbprint(
                                jwk["x"].as_str().unwrap(),
                                jwk["y"].as_str().unwrap(),
                            );
                            let binding = self.issuer.sign(
                                TYP_MACHINE_BINDING,
                                json!({"acct": "acct-1", "mch": state.machine_id, "hub": "hub-1", "sig_jkt": jkt,
                                    "cmd_pk": self.substitute_cmd_pk.clone().unwrap_or(cmd_pk), "cmd_kid": "kid-1",
                                    "projects": request["projects"], "gen": "1", "iat": unix_now(), "exp": unix_now() + 86_400}),
                            );
                            reply(
                                201,
                                json!({"wire_version": 1, "machine_id": state.machine_id, "account_id": "acct-1",
                                    "feed_generation": "1", "active_epoch": "1", "machine_binding": binding,
                                    "grant": {"grant_id": state.machine_id, "kind": "machine", "account_id": "acct-1",
                                        "capabilities": ["feed:append", "commands:receive"], "scopes": [],
                                        "grant_generation": "1", "status": "active", "expires_at": "2027-01-03T00:00:00Z"}}),
                            )
                        }
                    }
                }
            }
            ("POST", "/api/operator/enrollments/approve") => {
                if !bearer_ok {
                    reply(401, json!({"error": "unauthorized"}))
                } else {
                    state.approvals.push(request.clone());
                    reply(200, json!({"wire_version": 1, "status": "approved"}))
                }
            }
            (_, _) => {
                // Relay routes: PSC-PoP grant auth (§4.2 order, abridged).
                let machine_id = state.machine_id.clone();
                if header(headers, "Authorization")
                    != Some(format!("PSC-PoP {machine_id}").as_str())
                {
                    return reply(401, json!({"error": "pop_required"}));
                }
                if header(headers, "Origin").is_some() {
                    return reply(403, json!({"error": "origin_mismatch"}));
                }
                let jwk = state.machine_jwk.clone().unwrap();
                match self.verify_pop(
                    &mut state,
                    &jwk,
                    method,
                    path,
                    body,
                    header(headers, "PSC-PoP-Proof").unwrap_or(""),
                ) {
                    Err(code) => reply(401, json!({"error": code})),
                    Ok(claims) => {
                        let current = state.generation.to_string();
                        if path == "/api/operator/grants/me" {
                            reply(
                                200,
                                json!({"wire_version": 1, "grant": {"grant_id": machine_id, "kind": "machine", "status": "active",
                                    "grant_generation": current, "scopes": [{"project_id": "proj-a"}, {"project_id": "proj-b"}]}}),
                            )
                        } else if claims.get("gen") != Some(&json!(current)) {
                            reply(
                                401,
                                json!({"error": "grant_generation_stale", "current_generation": current}),
                            )
                        } else if (method, path) == ("POST", "/api/operator/feed/events") {
                            let rows: Vec<Value> = request["events"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|event| {
                                    state.appended.push(event.clone());
                                    json!({"event_id": event["event_id"], "outcome": "stored",
                                        "sequence": state.appended.len().to_string(), "digest": event["digest"],
                                        "key_epoch": event["key_epoch"], "stored_at": "2026-10-05T19:00:00Z",
                                        "expires_at": "2027-01-03T19:00:00Z"})
                                })
                                .collect();
                            reply(
                                200,
                                json!({"wire_version": 1, "feed_generation": "1", "rows": rows}),
                            )
                        } else {
                            reply(404, json!({"error": "not_found"}))
                        }
                    }
                }
            }
        };
        state
            .log
            .push((method.to_owned(), path.to_owned(), response.status));
        response
    }
}

struct Client(Arc<FakeCloud>);

impl HttpClient for Client {
    fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, String)],
        body: &[u8],
    ) -> Result<HttpResponse, Failure> {
        Ok(self.0.handle(method, url, headers, body))
    }
}

fn enrolled(cloud: &Arc<FakeCloud>) -> MachinePrincipal {
    let http: Arc<dyn HttpClient> = Arc::new(Client(Arc::clone(cloud)));
    let issuer = issuer_keys(Arc::clone(&http), ORIGIN);
    let authority = AccountAuthority::new(ORIGIN, BEARER.into());
    enroll_machine(
        http.as_ref(),
        &authority,
        &issuer,
        &EnrollRequest {
            hub_id: "hub-1",
            label: "soundwave",
            projects: &["proj-a".to_owned()],
            presence: false,
        },
    )
    .expect("machine enrolls")
}

#[test]
fn enrollment_proves_both_keys_and_verifies_the_binding_before_saving() {
    let cloud = FakeCloud::new();
    let principal = enrolled(&cloud);
    assert_eq!(principal.account_id, "acct-1");
    assert_eq!(principal.hub_id, "hub-1");
    assert_eq!(principal.grant_id, principal.machine_id);
    assert_eq!(principal.command_key_id, "kid-1");
    assert_eq!(principal.projects, vec!["proj-a".to_owned()]);
    // The stored keys are the enrolled keys.
    let relay = principal.relay_key().expect("relay key");
    let jwk = cloud.state.lock().unwrap().machine_jwk.clone().unwrap();
    assert_eq!(
        relay.thumbprint(),
        crate::hub::operator_inbox::jws::thumbprint(
            jwk["x"].as_str().unwrap(),
            jwk["y"].as_str().unwrap()
        )
    );
    let command_secret = principal.command_secret().expect("command key");
    assert_eq!(
        URL_SAFE_NO_PAD.encode(cas_operator_crypto::public_key_of(&command_secret).unwrap()),
        principal.command_public
    );

    let dir = tempfile::tempdir().unwrap();
    let store = PrincipalStore::new(dir.path());
    assert!(store.load().unwrap().is_none());
    store.save(&principal).unwrap();
    assert!(store.load().unwrap() == Some(principal.clone()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(store.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // The bearer authorizes the account; it is never written to disk.
    let saved = fs::read_to_string(store.path()).unwrap();
    assert!(!saved.contains(BEARER));
}

#[test]
fn a_binding_to_another_command_key_is_refused() {
    let mut cloud = FakeCloud::new();
    Arc::get_mut(&mut cloud).unwrap().substitute_cmd_pk = Some("not-our-key".into());
    let http: Arc<dyn HttpClient> = Arc::new(Client(Arc::clone(&cloud)));
    let issuer = issuer_keys(Arc::clone(&http), ORIGIN);
    let error = enroll_machine(
        http.as_ref(),
        &AccountAuthority::new(ORIGIN, BEARER.into()),
        &issuer,
        &EnrollRequest {
            hub_id: "hub-1",
            label: "soundwave",
            projects: &[],
            presence: false,
        },
    )
    .err()
    .expect("refused");
    assert!(matches!(error, AuthorityError::Binding(ref detail) if detail.contains("cmd_pk")));
}

#[test]
fn enrollment_without_the_account_bearer_is_refused() {
    let cloud = FakeCloud::new();
    let http: Arc<dyn HttpClient> = Arc::new(Client(Arc::clone(&cloud)));
    let issuer = issuer_keys(Arc::clone(&http), ORIGIN);
    let error = enroll_machine(
        http.as_ref(),
        &AccountAuthority::new(ORIGIN, "psc_k_wrong".into()),
        &issuer,
        &EnrollRequest {
            hub_id: "hub-1",
            label: "soundwave",
            projects: &[],
            presence: false,
        },
    )
    .err()
    .expect("refused");
    assert!(
        matches!(error, AuthorityError::Refused { status: 401, ref code } if code == "unauthorized")
    );
}

fn sealed_event(id: &str) -> SealedEvent {
    let bytes = b"sealed elsewhere";
    SealedEvent {
        event_id: id.into(),
        hub_id: "hub-1".into(),
        project_id: "proj-a".into(),
        session_id: "s_session".into(),
        key_epoch: Position::new(1).unwrap(),
        ciphertext: URL_SAFE_NO_PAD.encode(bytes),
        digest: crate::hub::operator_inbox::wire::digest(bytes),
        attachment_ids: vec![],
    }
}

#[tokio::test]
async fn relay_append_signs_each_request_and_resyncs_a_stale_generation() {
    let cloud = FakeCloud::new();
    let principal = enrolled(&cloud);
    let dir = tempfile::tempdir().unwrap();
    let store = PrincipalStore::new(dir.path());
    store.save(&principal).unwrap();
    let http: Arc<dyn HttpClient> = Arc::new(Client(Arc::clone(&cloud)));
    let transport = MachineTransport::new(http, principal, Some(store.clone())).unwrap();
    let relay = MachineRelay::new(transport);

    let receipt = relay
        .append(
            Position::new(1).unwrap(),
            &[sealed_event("0123456789abcdef0123456789abcdef")],
        )
        .await
        .expect("append");
    assert_eq!(receipt.rows.len(), 1);

    // A scope change on the cloud bumps the generation; the transport learns
    // it from /grants/me, persists it, and the retried proof is accepted.
    cloud.state.lock().unwrap().generation = 2;
    relay
        .append(
            Position::new(1).unwrap(),
            &[sealed_event("fedcba9876543210fedcba9876543210")],
        )
        .await
        .expect("append after resync");
    let saved = store.load().unwrap().unwrap();
    assert_eq!(saved.grant_generation, "2");
    assert_eq!(
        saved.projects,
        vec!["proj-a".to_owned(), "proj-b".to_owned()]
    );
    let log = cloud.state.lock().unwrap().log.clone();
    let statuses: Vec<u16> = log
        .iter()
        .filter(|(_, path, _)| {
            path.starts_with("/api/operator/feed") || path.ends_with("grants/me")
        })
        .map(|(_, _, status)| *status)
        .collect();
    assert_eq!(statuses, vec![200, 401, 200, 200]);
    assert_eq!(cloud.state.lock().unwrap().appended.len(), 2);
}

#[tokio::test]
async fn a_wrong_clock_is_corrected_once_from_the_server_date() {
    let cloud = FakeCloud::new();
    let principal = enrolled(&cloud);
    cloud.state.lock().unwrap().skew_seconds = 600;
    let http: Arc<dyn HttpClient> = Arc::new(Client(Arc::clone(&cloud)));
    let transport = MachineTransport::new(http, principal, None).unwrap();
    // The fake's Date header is real time, so skew learned from it is ~0 and
    // the retry still fails: the transport gives up after one re-sign.
    let response = transport
        .exchange_blocking("GET", "/api/operator/keys/epoch", &[])
        .expect("exchange");
    assert_eq!(response.status, 401);
    let expired = cloud
        .state
        .lock()
        .unwrap()
        .log
        .iter()
        .filter(|(_, path, status)| path.ends_with("keys/epoch") && *status == 401)
        .count();
    assert_eq!(expired, 2, "one original attempt and exactly one re-sign");
}

#[tokio::test]
async fn the_machine_transport_never_carries_a_device_request() {
    let cloud = FakeCloud::new();
    let principal = enrolled(&cloud);
    let http: Arc<dyn HttpClient> = Arc::new(Client(Arc::clone(&cloud)));
    let inbox = crate::hub::operator_inbox::DeviceInbox::new(
        MachineTransport::new(http, principal, None).unwrap(),
    );
    let refused = inbox
        .replay(Position::new(1).unwrap(), Position::new(0).unwrap(), 10)
        .await
        .err();
    assert_eq!(
        refused,
        Some(Failure::Protocol("hub holds no device grant"))
    );
}

#[test]
fn device_approval_sends_the_fixed_custody_consent() {
    let cloud = FakeCloud::new();
    let http = Client(Arc::clone(&cloud));
    decide_device(
        &http,
        &AccountAuthority::new(ORIGIN, BEARER.into()),
        "ABCD-EFGH",
        true,
        false,
        &[CommandScope {
            hub_id: "hub-1".into(),
            project_id: "proj-a".into(),
            session_id: None,
            operations: vec!["operator_message".into()],
        }],
    )
    .expect("approved");
    let approval = cloud.state.lock().unwrap().approvals[0].clone();
    assert_eq!(
        approval["consent"],
        json!({"custody": "cloud_account_permission", "not_end_to_end_acknowledged": true, "retention_days": 90})
    );
    assert_eq!(approval["capabilities"], json!(["feed:read"]));
    assert_eq!(approval["scopes"][0]["session_id"], Value::Null);
}
