//! The enrolled drain against an in-process cloud that really opens each
//! sealed event with the epoch key and its AAD (so a wrong routing binding
//! fails), keeps one row per `(account, event_id)`, refuses novel events
//! under a retired epoch, and can lose the append response after storing.
//! PoP itself is covered by machine_tests.

use super::*;
use crate::hub::operator_inbox::jws::test_issuer::TestIssuer;
use crate::hub::operator_inbox::machine::{HttpClient, HttpResponse};
use cas_store::{
    OperatorFeedBinding, OperatorTurn, OperatorTurnMetadata, PromptQueueStore, QueueOrigin,
    SqlitePromptQueueStore,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const ORIGIN: &str = "https://psc.test";

struct Epoch {
    secret: Vec<u8>,
    public: Vec<u8>,
    active: bool,
}

#[derive(Default)]
struct State {
    epochs: Vec<Epoch>,
    policy_version: u64,
    frozen: bool,
    /// event_id → (digest, sequence, epoch, plaintext)
    events: HashMap<String, (String, u64, usize, Value)>,
    head: u64,
    lose_next_response: bool,
}

struct Cloud {
    issuer: TestIssuer,
    state: Mutex<State>,
}

impl Cloud {
    fn new() -> Arc<Self> {
        let cloud = Arc::new(Self {
            issuer: TestIssuer::new("op-test-1"),
            state: Mutex::new(State::default()),
        });
        cloud.rotate();
        cloud
    }

    fn rotate(&self) {
        let mut state = self.state.lock().unwrap();
        for epoch in &mut state.epochs {
            epoch.active = false;
        }
        let pair = cas_operator_crypto::generate_key_pair();
        state.epochs.push(Epoch {
            secret: pair.secret.to_vec(),
            public: pair.public.to_vec(),
            active: true,
        });
        state.policy_version += 1;
    }

    fn active(state: &State) -> usize {
        state.epochs.iter().position(|epoch| epoch.active).unwrap() + 1
    }

    fn reply(status: u16, body: Value) -> HttpResponse {
        HttpResponse {
            status,
            body: serde_json::to_vec(&body).unwrap(),
            date: None,
            retry_after_s: None,
        }
    }

    fn handle(&self, method: &str, path: &str, body: &[u8]) -> HttpResponse {
        let mut state = self.state.lock().unwrap();
        match (method, path) {
            ("GET", "/api/operator/jwks") => Self::reply(200, self.issuer.jwks()),
            ("GET", "/api/operator/keys/epoch") => {
                let epoch = Self::active(&state);
                let now = Utc::now().timestamp();
                let manifest = self.issuer.sign(
                    TYP_EPOCH_MANIFEST,
                    json!({"acct": "acct-1", "fgen": "1", "epoch": epoch.to_string(), "status": "active",
                        "suite": {"kem": 16, "kdf": 1, "aead": 2},
                        "pk": URL_SAFE_NO_PAD.encode(&state.epochs[epoch - 1].public),
                        "policy_version": state.policy_version.to_string(),
                        "upload_state": if state.frozen { "frozen" } else { "open" },
                        "iat": now, "exp": now + 86_400}),
                );
                Self::reply(
                    200,
                    json!({"wire_version": 1, "feed_generation": "1", "active_epoch": epoch.to_string(),
                        "policy_version": state.policy_version.to_string(),
                        "upload_state": if state.frozen { "frozen" } else { "open" }, "manifest": manifest}),
                )
            }
            ("POST", "/api/operator/feed/events") => {
                let request: Value = serde_json::from_slice(body).unwrap();
                let mut rows = Vec::new();
                for event in request["events"].as_array().unwrap() {
                    let id = event["event_id"].as_str().unwrap().to_owned();
                    let digest = event["digest"].as_str().unwrap().to_owned();
                    let epoch: usize = event["key_epoch"].as_str().unwrap().parse().unwrap();
                    if let Some((stored, sequence, stored_epoch, _)) = state.events.get(&id) {
                        rows.push(if *stored == digest {
                            json!({"event_id": id, "outcome": "duplicate", "sequence": sequence.to_string(),
                                "digest": digest, "key_epoch": stored_epoch.to_string(),
                                "stored_at": "2026-10-05T19:00:00Z", "expires_at": "2027-01-03T19:00:00Z"})
                        } else {
                            json!({"event_id": id, "outcome": "rejected", "error": "event_conflict"})
                        });
                        continue;
                    }
                    if !state.epochs[epoch - 1].active {
                        rows.push(json!({"event_id": id, "outcome": "rejected", "error": "epoch_retired",
                            "active_epoch": Self::active(&state).to_string(), "policy_version": state.policy_version.to_string()}));
                        continue;
                    }
                    let bytes = URL_SAFE_NO_PAD
                        .decode(event["ciphertext"].as_str().unwrap())
                        .unwrap();
                    let plain = cas_operator_crypto::open_event(
                        &state.epochs[epoch - 1].secret,
                        &bytes,
                        &cas_operator_crypto::EventIds {
                            account_id: "acct-1",
                            feed_generation: "1",
                            key_epoch: &epoch.to_string(),
                            event_id: &id,
                            hub_id: event["hub_id"].as_str().unwrap(),
                            project_id: event["project_id"].as_str().unwrap(),
                            session_id: event["session_id"].as_str().unwrap(),
                        },
                    )
                    .expect("the cloud double opens what the hub sealed");
                    state.head += 1;
                    let sequence = state.head;
                    state.events.insert(
                        id.clone(),
                        (
                            digest.clone(),
                            sequence,
                            epoch,
                            serde_json::from_slice(&plain).unwrap(),
                        ),
                    );
                    rows.push(json!({"event_id": id, "outcome": "stored", "sequence": sequence.to_string(),
                        "digest": digest, "key_epoch": epoch.to_string(),
                        "stored_at": "2026-10-05T19:00:00Z", "expires_at": "2027-01-03T19:00:00Z"}));
                }
                if std::mem::take(&mut state.lose_next_response) {
                    return Self::reply(503, json!({"error": "temporarily_unavailable"}));
                }
                Self::reply(
                    200,
                    json!({"wire_version": 1, "feed_generation": "1", "rows": rows}),
                )
            }
            _ => Self::reply(404, json!({"error": "not_found"})),
        }
    }
}

struct Http(Arc<Cloud>);

impl HttpClient for Http {
    fn send(
        &self,
        method: &str,
        url: &str,
        _headers: &[(&str, String)],
        body: &[u8],
    ) -> Result<HttpResponse, Failure> {
        Ok(self
            .0
            .handle(method, url.strip_prefix(ORIGIN).unwrap(), body))
    }
}

fn principal() -> MachinePrincipal {
    let relay = super::super::jws::RelayKey::generate();
    let command = cas_operator_crypto::generate_key_pair();
    MachinePrincipal {
        wire_version: 1,
        cloud_origin: ORIGIN.into(),
        account_id: "acct-1".into(),
        machine_id: "mch-1".into(),
        hub_id: "hub-1".into(),
        grant_id: "mch-1".into(),
        grant_generation: "1".into(),
        feed_generation: "1".into(),
        active_epoch: "1".into(),
        label: "soundwave".into(),
        projects: vec!["github.com/richards-llc/cassy".into()],
        capabilities: vec![],
        signing_secret: URL_SAFE_NO_PAD.encode(&relay.secret()[..]),
        command_secret: URL_SAFE_NO_PAD.encode(&command.secret[..]),
        command_public: URL_SAFE_NO_PAD.encode(command.public),
        command_key_id: "kid-1".into(),
        policy_version: None,
        enrolled_at: "2026-10-05T19:00:00Z".into(),
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    store: SqlitePromptQueueStore,
    cloud: Arc<Cloud>,
    transport: MachineTransport,
    issuer: IssuerKeys,
    principal: MachinePrincipal,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let store = SqlitePromptQueueStore::open(dir.path()).unwrap();
    store.init().unwrap();
    let principal = principal();
    store
        .bind_operator_feed(&OperatorFeedBinding {
            account_id: principal.account_id.clone(),
            machine_id: principal.machine_id.clone(),
            hub_id: principal.hub_id.clone(),
            project_id: principal.projects[0].clone(),
            bound_at: "2026-10-05T19:00:00Z".into(),
        })
        .unwrap();
    let cloud = Cloud::new();
    let http: Arc<dyn HttpClient> = Arc::new(Http(Arc::clone(&cloud)));
    let issuer = super::super::machine::issuer_keys(Arc::clone(&http), ORIGIN);
    let transport = MachineTransport::new(http, principal.clone(), None).unwrap();
    Rig {
        _dir: dir,
        store,
        cloud,
        transport,
        issuer,
        principal,
    }
}

fn record(store: &SqlitePromptQueueStore, prompt: &str) {
    static DAEMON: QueueOrigin = QueueOrigin::Daemon;
    store
        .record_operator_turn(&OperatorTurn {
            source: "supervisor",
            target: "operator",
            prompt,
            factory_session: Some("cas-src-quiet hawk ✦"),
            metadata: OperatorTurnMetadata {
                origin: Some(&DAEMON),
                ..Default::default()
            },
        })
        .unwrap();
}

impl Rig {
    async fn drain(&self) -> Result<DrainReport, DrainError> {
        let policy = fetch_epoch_policy(&self.transport, &self.issuer, &self.principal)?;
        let relay = MachineRelay::new(self.transport.clone());
        drain_project(&self.store, &relay, &policy, &self.principal).await
    }
}

#[tokio::test]
async fn bound_turns_are_sealed_once_and_the_cloud_opens_them() {
    let rig = rig();
    for prompt in ["one", "two", "three"] {
        record(&rig.store, prompt);
    }
    let report = rig.drain().await.unwrap();
    assert_eq!((report.claimed, report.sealed, report.stored), (3, 3, 3));
    let state = rig.cloud.state.lock().unwrap();
    let mut prompts: Vec<(u64, String)> = state
        .events
        .values()
        .map(|(_, sequence, _, plain)| {
            assert_eq!(plain["type"], "cas.operator.turn");
            assert_eq!(plain["session_name"], "cas-src-quiet hawk ✦");
            (
                *sequence,
                plain["snapshot"]["prompt"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    prompts.sort();
    assert_eq!(
        prompts.into_iter().map(|(_, p)| p).collect::<Vec<_>>(),
        ["one", "two", "three"]
    );
    drop(state);
    assert_eq!(rig.store.operator_cloud_backlog().unwrap().delivered, 3);
    assert_eq!(rig.drain().await.unwrap().claimed, 0);
}

#[tokio::test]
async fn a_lost_append_response_retries_the_same_bytes_without_duplicating() {
    let rig = rig();
    record(&rig.store, "kept once");
    rig.cloud.state.lock().unwrap().lose_next_response = true;
    assert!(matches!(
        rig.drain().await,
        Err(DrainError::Transport(Failure::Http { status: 503, .. }))
    ));
    let first_digest = rig
        .cloud
        .state
        .lock()
        .unwrap()
        .events
        .values()
        .next()
        .unwrap()
        .0
        .clone();
    // The retry waits out its backoff (1 s plus at most 1 s of jitter).
    tokio::time::sleep(std::time::Duration::from_millis(2_100)).await;
    let report = rig.drain().await.unwrap();
    assert_eq!(
        (report.sealed, report.stored),
        (0, 1),
        "no re-seal on retry"
    );
    let state = rig.cloud.state.lock().unwrap();
    assert_eq!(state.events.len(), 1);
    assert_eq!(state.events.values().next().unwrap().0, first_digest);
}

#[tokio::test]
async fn a_novel_event_under_a_retired_epoch_is_resealed_with_its_event_id() {
    let rig = rig();
    record(&rig.store, "rotated");
    let stale = fetch_epoch_policy(&rig.transport, &rig.issuer, &rig.principal).unwrap();
    rig.cloud.rotate();
    let relay = MachineRelay::new(rig.transport.clone());
    let report = drain_project(&rig.store, &relay, &stale, &rig.principal)
        .await
        .unwrap();
    assert_eq!(report.resealed, 1);
    assert!(rig.cloud.state.lock().unwrap().events.is_empty());
    let report = rig.drain().await.unwrap();
    assert_eq!((report.sealed, report.stored), (1, 1));
    let state = rig.cloud.state.lock().unwrap();
    let (_, _, epoch, _) = state.events.values().next().unwrap();
    assert_eq!(*epoch, 2);
}

#[tokio::test]
async fn frozen_uploads_hold_the_lane_and_a_policy_regression_is_refused() {
    let rig = rig();
    record(&rig.store, "held");
    rig.cloud.state.lock().unwrap().frozen = true;
    let report = rig.drain().await.unwrap();
    assert!(report.frozen);
    assert_eq!(rig.store.operator_cloud_backlog().unwrap().pending, 1);

    let mut principal = rig.principal.clone();
    principal.policy_version = Some("99".into());
    assert_eq!(
        fetch_epoch_policy(&rig.transport, &rig.issuer, &principal).err(),
        Some(DrainError::Policy("policy version went backwards"))
    );
}

#[tokio::test]
async fn the_plaintext_carries_names_and_routing_ids_stay_opaque() {
    let rig = rig();
    record(&rig.store, "routing");
    rig.drain().await.unwrap();
    let claims_seen = rig.cloud.state.lock().unwrap().events.len();
    assert_eq!(claims_seen, 1);
    let backlog = rig.store.operator_cloud_backlog().unwrap();
    assert_eq!(backlog.delivered, 1);
    assert!(cas_store::is_routing_id(&cas_store::session_routing_id(
        "cas-src-quiet hawk ✦"
    )));
}
