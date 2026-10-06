//! Offline commands against an in-process cloud that issues admission JWS
//! like §10.3 and records receipts like §10.4 (PoP is covered elsewhere).

use super::*;
use crate::hub::operator_inbox::jws::test_issuer::TestIssuer;
use crate::hub::operator_inbox::machine::{HttpClient, HttpResponse};
use cas_store::{OperatorFeedBinding, PromptQueueStore};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const ORIGIN: &str = "https://psc.test";
const PROJECT: &str = "github.com/richards-llc/cassy";
const SESSION_NAME: &str = "cas-src-quiet hawk ✦";

struct Command {
    session_id: String,
    ciphertext: Vec<u8>,
    digest: String,
    claim_digest: Option<String>,
    project_id: String,
    status: &'static str,
    receipt: Option<Value>,
}

#[derive(Default)]
struct State {
    commands: HashMap<String, Command>,
    fail_next_receipt: bool,
    receipts_posted: usize,
}

struct Cloud {
    issuer: TestIssuer,
    state: Mutex<State>,
}

impl Cloud {
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
        if (method, path) == ("GET", "/api/operator/jwks") {
            return Self::reply(200, self.issuer.jwks());
        }
        if (method, path) == ("GET", "/api/operator/machine/commands?limit=50") {
            let commands: Vec<Value> = state
                .commands
                .iter()
                .filter(|(_, command)| matches!(command.status, "pending_machine" | "reserved"))
                .map(|(id, command)| {
                    json!({"command_id": id, "hub_id": "hub-1", "project_id": command.project_id,
                        "session_id": command.session_id, "operation": "operator_message",
                        "status": command.status, "created_at": "2026-10-05T19:00:00Z", "expires_at": "2026-10-06T19:00:00Z"})
                })
                .collect();
            return Self::reply(
                200,
                json!({"wire_version": 1, "commands": commands, "poll_after_ms": 15000}),
            );
        }
        let parts: Vec<&str> = path.split('/').collect();
        let id = parts.get(5).copied().unwrap_or_default().to_owned();
        let Some(command) = state.commands.get_mut(&id) else {
            return Self::reply(404, json!({"error": "command_not_found"}));
        };
        match parts.get(6).copied() {
            Some("reserve") => {
                command.status = "reserved";
                let now = Utc::now().timestamp();
                let jws = self.issuer.sign(
                    TYP_COMMAND_ADMISSION,
                    json!({"cmd": id, "acct": "acct-1", "mch": "mch-1", "hub": "hub-1",
                        "proj": command.project_id, "sess": command.session_id, "op": "operator_message",
                        "digest": command.claim_digest.clone().unwrap_or_else(|| command.digest.clone()),
                        "hist": "hist-event-0001-AbCdEfGhIjKl", "dev": "dev-phone", "dev_gen": "1",
                        "submitted_at": "2026-10-05T19:00:00Z", "reserved_at": "2026-10-05T19:05:00Z",
                        "iat": now, "exp": now + 30 * 86_400, "jti": format!("jti-{id}")}),
                );
                Self::reply(
                    200,
                    json!({"wire_version": 1, "command_id": id, "status": "reserved",
                        "machine_ciphertext": URL_SAFE_NO_PAD.encode(&command.ciphertext),
                        "machine_digest": command.digest, "admission_authorization": jws}),
                )
            }
            Some("receipt") => {
                let receipt: Value = serde_json::from_slice(body).unwrap();
                if std::mem::take(&mut state.fail_next_receipt) {
                    return Self::reply(503, json!({"error": "temporarily_unavailable"}));
                }
                let command = state.commands.get_mut(&id).unwrap();
                if let Some(stored) = &command.receipt {
                    return if stored["receipt_id"] == receipt["receipt_id"] {
                        Self::reply(200, json!({"wire_version": 1, "receipt": stored}))
                    } else {
                        Self::reply(409, json!({"error": "receipt_conflict"}))
                    };
                }
                command.status = if receipt["outcome"] == "accepted" {
                    "accepted"
                } else {
                    "rejected_by_machine"
                };
                command.receipt = Some(receipt.clone());
                state.receipts_posted += 1;
                Self::reply(200, json!({"wire_version": 1, "receipt": receipt}))
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
        projects: vec![PROJECT.into()],
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
    root: std::path::PathBuf,
    cloud: Arc<Cloud>,
    transport: MachineTransport,
    issuer: IssuerKeys,
    principal: MachinePrincipal,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let store = SqlitePromptQueueStore::open(&root).unwrap();
    store.init().unwrap();
    let principal = principal();
    store
        .bind_operator_feed(&OperatorFeedBinding {
            account_id: "acct-1".into(),
            machine_id: "mch-1".into(),
            hub_id: "hub-1".into(),
            project_id: PROJECT.into(),
            bound_at: "2026-10-05T19:00:00Z".into(),
        })
        .unwrap();
    let cloud = Arc::new(Cloud {
        issuer: TestIssuer::new("op-test-1"),
        state: Mutex::new(State::default()),
    });
    let http: Arc<dyn HttpClient> = Arc::new(Http(Arc::clone(&cloud)));
    let issuer = super::super::machine::issuer_keys(Arc::clone(&http), ORIGIN);
    let transport = MachineTransport::new(http, principal.clone(), None).unwrap();
    Rig {
        _dir: dir,
        root,
        cloud,
        transport,
        issuer,
        principal,
    }
}

impl Rig {
    /// A device seals and submits a command (the browser does this in S4).
    fn submit(&self, id: &str, session_name: &str, project: &str, body: &str) {
        let session_id = cas_store::session_routing_id(SESSION_NAME);
        let plaintext = serde_json::to_vec(&json!({"type": "cas.operator.command", "v": 1,
            "operation": "operator_message", "session_name": session_name, "body": body}))
        .unwrap();
        let public = URL_SAFE_NO_PAD
            .decode(&self.principal.command_public)
            .unwrap();
        let sealed = cas_operator_crypto::seal_command(
            &public,
            &plaintext,
            &cas_operator_crypto::CommandIds {
                account_id: "acct-1",
                machine_id: "mch-1",
                command_id: id,
                hub_id: "hub-1",
                project_id: project,
                session_id: &session_id,
                operation: "operator_message",
                machine_key_id: "kid-1",
            },
        )
        .unwrap();
        self.cloud.state.lock().unwrap().commands.insert(
            id.into(),
            Command {
                session_id,
                ciphertext: sealed.bytes,
                digest: sealed.digest,
                claim_digest: None,
                project_id: project.into(),
                status: "pending_machine",
                receipt: None,
            },
        );
    }

    fn process(&self) -> CommandReport {
        let root = self.root.clone();
        let queue_for = move |project: &str| {
            let queue = SqlitePromptQueueStore::open(&root).ok()?;
            (queue.operator_feed_binding().ok()??.project_id == project).then_some(queue)
        };
        process_commands(&self.transport, &self.issuer, &self.principal, &queue_for).unwrap()
    }

    fn queued(&self) -> Vec<(String, String)> {
        let conn = rusqlite::Connection::open(self.root.join("cas.db")).unwrap();
        let mut stmt = conn
            .prepare("SELECT target, prompt FROM prompt_queue ORDER BY id")
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn receipt(&self, id: &str) -> Value {
        self.cloud.state.lock().unwrap().commands[id]
            .receipt
            .clone()
            .unwrap_or(Value::Null)
    }
}

const CMD: &str = "cmdFixture0001AbCdEfGhIjKl";

#[test]
fn a_returning_hub_admits_an_offline_message_once_and_reports_acceptance() {
    let rig = rig();
    rig.submit(CMD, SESSION_NAME, PROJECT, "Ship it once soundwave is back");
    let report = rig.process();
    assert_eq!((report.listed, report.admitted), (1, 1));
    assert_eq!(
        rig.queued(),
        vec![(
            "supervisor".to_owned(),
            "Ship it once soundwave is back".to_owned()
        )]
    );
    assert_eq!(rig.receipt(CMD)["outcome"], "accepted");
    // Accepted commands are no longer listed; a second pass does nothing.
    assert_eq!(rig.process().listed, 0);
}

#[test]
fn a_crash_before_the_receipt_replays_the_same_admission() {
    let rig = rig();
    rig.submit(CMD, SESSION_NAME, PROJECT, "exactly once");
    rig.cloud.state.lock().unwrap().fail_next_receipt = true;
    assert_eq!(rig.process().admitted, 1);
    assert_eq!(rig.receipt(CMD), Value::Null);
    let report = rig.process();
    assert_eq!((report.admitted, report.replayed), (0, 1));
    assert_eq!(rig.queued().len(), 1, "one queue row across the replay");
    assert_eq!(rig.receipt(CMD)["outcome"], "accepted");
    assert_eq!(rig.cloud.state.lock().unwrap().receipts_posted, 1);
}

#[test]
fn a_tampered_admission_or_payload_is_rejected_without_a_queue_row() {
    let rig = rig();
    rig.submit(CMD, SESSION_NAME, PROJECT, "tampered");
    rig.cloud
        .state
        .lock()
        .unwrap()
        .commands
        .get_mut(CMD)
        .unwrap()
        .claim_digest = Some(format!("sha256:{}", "0".repeat(64)));
    assert_eq!(rig.process().rejected, 1);
    assert_eq!(rig.receipt(CMD)["reason"], "digest_mismatch");

    let other = "cmdFixture0002AbCdEfGhIjKl";
    rig.submit(other, "a different session", PROJECT, "misrouted");
    assert_eq!(rig.process().rejected, 1);
    assert_eq!(rig.receipt(other)["reason"], "session_mismatch");

    let unbound = "cmdFixture0003AbCdEfGhIjKl";
    rig.submit(
        unbound,
        SESSION_NAME,
        "github.com/other/repo",
        "no project here",
    );
    assert_eq!(rig.process().rejected, 1);
    assert_eq!(rig.receipt(unbound)["reason"], "project_not_bound");
    assert!(rig.queued().is_empty());
}
