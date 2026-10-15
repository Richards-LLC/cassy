//! cas-f9c7: cross-machine supervisor messaging through a fake Cassy Cloud
//! peer mailbox, between "machines" (separate CAS directories).
//!
//! The fake implements the Richards-LLC/petra-stella-cloud#152 contract on
//! top of the cas-e477 fake agent registry: send (sender and recipient must
//! be registered, the recipient under the same repo; sender identity is
//! stamped from the registry; `dedupe_key` collapses resends), claim (only
//! queued messages of the named recipients, in send order), ack and status.

use std::path::Path;
use std::sync::{Arc, Mutex};

use cas::cloud::peer_mailbox::{
    DeliveryReport, PeerMailbox, PeerSend, http_mailbox, parse_envelope, reply_to_peer_row,
    resolve_peer_target, send_to_peer,
};
use cas::store::{open_agent_store, open_prompt_queue_store};
use cas::types::{Agent, AgentRole};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::TestMachine;
use super::peers::{Registry, fake_cloud, peers_of, register_supervisor, seed_project};

type Mailbox = Arc<Mutex<Vec<Value>>>;

struct Send(Registry, Mailbox);
impl Respond for Send {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let registry = self.0.lock().unwrap();
        let (Some(sender), Some(recipient)) = (
            registry.get(body["sender_agent_id"].as_str().unwrap()),
            registry.get(body["recipient_agent_id"].as_str().unwrap()),
        ) else {
            return ResponseTemplate::new(404).set_body_json(json!({"error": "unknown agent"}));
        };
        let project = body["project_id"].as_str().unwrap();
        let recipient_repo = recipient["metadata"]["canonical_id"].as_str().unwrap_or("");
        if !cas::cloud::project_ids_match_with_aliases(recipient_repo, project, &[]) {
            return ResponseTemplate::new(403)
                .set_body_json(json!({"error": "recipient is not in this project"}));
        }
        let mut mailbox = self.1.lock().unwrap();
        if let Some(existing) = mailbox
            .iter()
            .find(|m| m["dedupe_key"] == body["dedupe_key"])
        {
            return ResponseTemplate::new(200)
                .set_body_json(json!({"id": existing["id"], "status": "duplicate"}));
        }
        let id = format!("pm_{}", mailbox.len() + 1);
        mailbox.push(json!({
            "id": id,
            "project_id": project,
            "recipient_agent_id": body["recipient_agent_id"],
            "sender_agent_id": body["sender_agent_id"],
            "sender_name": sender["name"],
            "sender_machine_id": sender["machine_id"],
            "sender_session": sender["metadata"]["factory_session"],
            "body": body["body"],
            "summary": body["summary"],
            "in_reply_to": body["in_reply_to"],
            "dedupe_key": body["dedupe_key"],
            "created_at": chrono::Utc::now().to_rfc3339(),
            "attempts": 0,
            "status": "queued",
            "delivered_at": null,
        }));
        ResponseTemplate::new(200).set_body_json(json!({"id": id, "status": "queued"}))
    }
}

struct Claim(Mailbox);
impl Respond for Claim {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let recipients: Vec<&str> = body["recipient_agent_ids"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let mut mailbox = self.0.lock().unwrap();
        let mut messages = Vec::new();
        for message in mailbox.iter_mut() {
            let mine = recipients.contains(&message["recipient_agent_id"].as_str().unwrap());
            if mine && message["status"] == "queued" {
                message["status"] = json!("leased");
                message["attempts"] = json!(message["attempts"].as_u64().unwrap() + 1);
                messages.push(message.clone());
            }
        }
        ResponseTemplate::new(200).set_body_json(json!({"messages": messages,"denied":[]} ))
    }
}

struct Ack(Mailbox);
impl Respond for Ack {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let mut mailbox = self.0.lock().unwrap();
        for ack in body["acks"].as_array().unwrap() {
            if let Some(message) = mailbox.iter_mut().find(|m| m["id"] == ack["id"]) {
                message["status"] = ack["outcome"].clone();
                message["delivered_at"] = json!(chrono::Utc::now().to_rfc3339());
            }
        }
        let results: Vec<_> = body["acks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|ack| json!({"id":ack["id"],"status":200}))
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({"results":results}))
    }
}

struct Status(Mailbox);
impl Respond for Status {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let id = request.url.path().rsplit('/').next().unwrap();
        match self.0.lock().unwrap().iter().find(|m| m["id"] == id) {
            Some(message) => ResponseTemplate::new(200).set_body_json(json!({
                "id": message["id"],
                "status": message["status"],
                "delivered_at": message["delivered_at"],
                "attempts": message["attempts"],
            })),
            None => ResponseTemplate::new(404),
        }
    }
}

async fn fake_cloud_with_mailbox() -> (MockServer, Registry, Mailbox) {
    let (server, registry) = fake_cloud().await;
    let mailbox: Mailbox = Arc::default();
    Mock::given(method("POST"))
        .and(path("/api/peer-messages"))
        .respond_with(Send(registry.clone(), mailbox.clone()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/peer-messages/claim"))
        .respond_with(Claim(mailbox.clone()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/peer-messages/ack"))
        .respond_with(Ack(mailbox.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/peer-messages/pm_[0-9]+$"))
        .respond_with(Status(mailbox.clone()))
        .mount(&server)
        .await;
    (server, registry, mailbox)
}

/// A supervisor registered both in the cloud and in the machine's own store,
/// as a live factory session does.
fn supervisor(machine: &TestMachine, id: &str) {
    supervisor_at(&machine.cas_dir, &machine.name, id);
}

fn supervisor_at(cas_dir: &Path, machine_name: &str, id: &str) {
    register_supervisor(cas_dir, id, machine_name, "cas-571d");
    let mut agent = Agent::new(id.to_string(), format!("sup-{id}"));
    agent.role = AgentRole::Supervisor;
    agent.machine_id = Some(machine_name.to_string());
    agent.factory_session = Some(format!("factory-{id}"));
    open_agent_store(cas_dir).unwrap().register(&agent).unwrap();
}

/// One recipient-daemon tick for supervisor `id`'s session.
fn tick(cas_dir: &Path, id: &str) -> DeliveryReport {
    cas::cloud::peer_mailbox::PeerMailboxRuntime::start(cas_dir, &format!("factory-{id}"))
        .expect("mailbox runtime survives missing startup credentials")
        .tick_blocking()
}

fn inbox(cas_dir: &Path) -> Vec<cas::store::QueuedPrompt> {
    let mut rows = open_prompt_queue_store(cas_dir)
        .unwrap()
        .peek_all(100)
        .unwrap();
    rows.sort_by_key(|row| row.id);
    rows
}

async fn daemon_poll(
    poller: &mut cas::cloud::peer_mailbox::PeerMailboxPoller,
    now: std::time::Instant,
) -> DeliveryReport {
    for _ in 0..1000 {
        if let Some(report) = poller.poll(now).await {
            return report;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    panic!("daemon mailbox duty did not complete");
}

#[tokio::test]
async fn cas_13c7_daemon_recovers_when_login_arrives_after_startup() {
    use cas::cloud::peer_mailbox::{PeerMailboxPoller, mailbox_status_section};
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, _registry, _mailbox) = fake_cloud_with_mailbox().await;
    let endpoint = server.uri();
    let alpha = TestMachine::new("machine-alpha");
    let beta = TestMachine::new("machine-beta");
    let mut poller = PeerMailboxPoller::new(&beta.cas_dir, "factory-b");
    let start = std::time::Instant::now();
    let unavailable = daemon_poll(&mut poller, start).await;
    assert!(
        unavailable.errors[0].contains("not logged in"),
        "{unavailable:?}"
    );
    assert!(mailbox_status_section(&beta.cas_dir, Some("factory-b")).contains("not logged in"));
    let (a, b) = (alpha.cas_dir.clone(), beta.cas_dir.clone());
    let (_alpha, _beta, receipt) = tokio::task::spawn_blocking(move || {
        seed_project(&alpha, &endpoint, "github.com/acme/widgets");
        seed_project(&beta, &endpoint, "github.com/acme/widgets");
        supervisor(&alpha, "a");
        supervisor(&beta, "b");
        let peers = peers_of(&alpha.cas_dir, "a");
        let receipt = send_to_peer(
            &alpha.cas_dir,
            "a",
            &peers[0],
            "late login delivery",
            None,
            None,
        )
        .unwrap();
        (alpha, beta, receipt)
    })
    .await
    .unwrap();
    // The first retry is due in five seconds, comfortably inside one minute.
    assert!(
        poller
            .poll(start + std::time::Duration::from_secs(4))
            .await
            .is_none()
    );
    let report = daemon_poll(&mut poller, start + std::time::Duration::from_secs(5)).await;
    assert_eq!(report.delivered, 1, "{report:?}");
    // pin: a retried peer delivery must retain its original message body.
    assert!(inbox(&b)[0].prompt.ends_with("late login delivery"));
    assert!(mailbox_status_section(&b, Some("factory-b")).contains(": receiving."));
    let service = service_for(&b, "b");
    let peers_text = coordinate(&service, json!({"action":"peers"}))
        .await
        .unwrap();
    assert!(
        peers_text.contains("Peer mailbox: receiving."),
        "{peers_text}"
    );
    for summary in [false, true] {
        let req =
            serde_json::from_value(json!({"action":"worker_status","summary":summary})).unwrap();
        let result = service
            .factory_request(rmcp::handler::server::wrapper::Parameters(req))
            .await
            .unwrap();
        let text = result
            .content
            .iter()
            .filter_map(|c| match &c.raw {
                rmcp::model::RawContent::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("Peer mailbox: receiving."),
            "summary={summary}: {text}"
        );
    }
    tokio::task::spawn_blocking(move || {
        assert_eq!(
            http_mailbox(&a)
                .unwrap()
                .status(&receipt.id)
                .unwrap()
                .status,
            "delivered"
        );
        let section = cas::cloud::peers::peers_section(&b, Some("b"));
        assert!(section.contains("Peer mailbox: receiving."), "{section}");
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_supervisor_messages_a_peer_on_another_machine_and_gets_the_reply() {
    let (server, _registry, mailbox) = fake_cloud_with_mailbox().await;
    let endpoint = server.uri();
    let alpha = TestMachine::new("machine-alpha");
    let beta = TestMachine::new("machine-beta");
    seed_project(&alpha, &endpoint, "github.com/acme/widgets");
    seed_project(&beta, &endpoint, "git@github.com:acme/widgets.git");
    let (a, b) = (alpha.cas_dir.clone(), beta.cas_dir.clone());

    tokio::task::spawn_blocking(move || {
        supervisor(&alpha, "a");
        supervisor(&beta, "b");

        // A finds B by name and sends twice while B's daemon is offline.
        let peers = peers_of(&a, "a");
        let to_b = resolve_peer_target(&peers, "sup-b")
            .unwrap()
            .expect("B is a peer");
        let first = send_to_peer(
            &a,
            "a",
            to_b,
            "can you take cas-1234?",
            Some("take cas-1234?"),
            None,
        )
        .unwrap();
        assert_eq!(first.status, "queued");
        send_to_peer(
            &a,
            "a",
            to_b,
            "and cas-5678 after",
            Some("then cas-5678"),
            None,
        )
        .unwrap();
        let receipts = http_mailbox(&a).unwrap();
        assert_eq!(receipts.status(&first.id).unwrap().status, "queued");

        // B comes online: both arrive, in order, with sender and machine.
        let report = tick(&b, "b");
        assert_eq!((report.claimed, report.delivered), (2, 2));
        let rows = inbox(&b);
        assert_eq!(rows.len(), 2);
        // pin: the first queued body must remain first after mailbox delivery.
        assert!(rows[0].prompt.ends_with("can you take cas-1234?"));
        // pin: the second queued body must remain second after mailbox delivery.
        assert!(rows[1].prompt.ends_with("and cas-5678 after"));
        let envelope = parse_envelope(&rows[0].prompt).unwrap();
        assert_eq!(envelope.sender_name, "sup-a");
        assert_eq!(envelope.machine, "machine-alpha");
        // pin: the reply address must retain the sender and machine qualification.
        assert!(rows[0].prompt.contains("sup-a@machine-alpha"));
        assert_eq!(rows[0].target, "sup-b");
        assert!(!rows[0].urgent);
        assert_eq!(receipts.status(&first.id).unwrap().status, "delivered");

        // No duplicates on the next tick.
        assert_eq!(tick(&b, "b").claimed, 0);
        assert_eq!(inbox(&b).len(), 2);

        // B replies to the first message; its row is acked locally.
        let queue_b = open_prompt_queue_store(&b).unwrap();
        let reply = reply_to_peer_row(
            &b,
            queue_b.as_ref(),
            rows[0].id,
            "b",
            "sup-b",
            "yes, taking it",
            Some("taking cas-1234"),
        )
        .unwrap()
        .expect("row is a peer message");
        assert_eq!(reply.to, "sup-a@machine-alpha");
        assert!(
            queue_b
                .queued_prompt(rows[0].id)
                .unwrap()
                .unwrap()
                .acked_at
                .is_some()
        );

        // A receives the reply, linked to its message.
        let report = tick(&a, "a");
        assert_eq!(report.delivered, 1);
        let replies = inbox(&a);
        assert_eq!(replies.len(), 1);
        // pin: reply delivery must retain the original reply body.
        assert!(replies[0].prompt.ends_with("yes, taking it"));
        assert!(
            replies[0]
                .prompt
                .contains(&format!("in reply to {}", first.id)),
            "{}",
            replies[0].prompt
        );
        assert_eq!(
            parse_envelope(&replies[0].prompt).unwrap().sender_name,
            "sup-b"
        );
    })
    .await
    .unwrap();

    let stored = mailbox.lock().unwrap();
    assert_eq!(stored.len(), 3);
    assert_eq!(stored[2]["in_reply_to"], stored[0]["id"]);
}

#[tokio::test]
async fn a_peer_message_to_another_repo_is_refused_and_a_resend_is_one_message() {
    let (server, _registry, mailbox) = fake_cloud_with_mailbox().await;
    let endpoint = server.uri();
    let alpha = TestMachine::new("machine-alpha");
    let gamma = TestMachine::new("machine-gamma");
    seed_project(&alpha, &endpoint, "github.com/acme/widgets");
    seed_project(&gamma, &endpoint, "github.com/acme/gadgets");
    let a = alpha.cas_dir.clone();

    tokio::task::spawn_blocking(move || {
        supervisor(&alpha, "a");
        supervisor(&gamma, "g");
        // Discovery never offers another repo's supervisor ...
        assert!(
            resolve_peer_target(&peers_of(&a, "a"), "sup-g")
                .unwrap()
                .is_none()
        );
        // ... and the mailbox refuses an addressed send anyway.
        let mailbox = http_mailbox(&a).unwrap();
        let refused = mailbox.send(&PeerSend {
            project_id: "github.com/acme/widgets".into(),
            sender_agent_id: "a".into(),
            recipient_agent_id: "g".into(),
            body: "hi".into(),
            summary: None,
            in_reply_to: None,
            dedupe_key: "k1".into(),
        });
        assert!(refused.unwrap_err().contains("403"));

        let resend = PeerSend {
            project_id: "github.com/acme/widgets".into(),
            sender_agent_id: "a".into(),
            recipient_agent_id: "a".into(),
            body: "note to self".into(),
            summary: None,
            in_reply_to: None,
            dedupe_key: "k2".into(),
        };
        assert_eq!(mailbox.send(&resend).unwrap().status, "queued");
        assert_eq!(mailbox.send(&resend).unwrap().status, "duplicate");
    })
    .await
    .unwrap();
    assert_eq!(mailbox.lock().unwrap().len(), 1);
}

fn service_for(cas_dir: &Path, agent_id: &str) -> cas::mcp::CasService {
    let core = cas::mcp::CasCore::with_daemon(cas_dir.to_path_buf(), None, None);
    core.set_agent_id_for_testing(agent_id.to_string());
    cas::mcp::CasService::new(core, None)
}

async fn coordinate(service: &cas::mcp::CasService, request: Value) -> Result<String, String> {
    let request: cas_mcp::types::CoordinationRequest = serde_json::from_value(request).unwrap();
    service
        .coordination(rmcp::handler::server::wrapper::Parameters(request))
        .await
        .map(|result| {
            result
                .content
                .iter()
                .filter_map(|content| match &content.raw {
                    rmcp::model::RawContent::Text(text) => Some(text.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .map_err(|error| error.message.to_string())
}

/// The MCP surface: `coordination action=message` to `name@machine` goes
/// through the mailbox, an urgent peer message is refused, the recipient
/// replies with `in_reply_to`, and `message_status id=` reads the receipt.
#[tokio::test]
async fn coordination_message_reaches_a_peer_and_message_status_reads_the_receipt() {
    // The MCP caller identity comes from the registry, never from the runner's
    // ambient CAS_* environment (a factory worker runs these tests).
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, _registry, _mailbox) = fake_cloud_with_mailbox().await;
    let endpoint = server.uri();
    let alpha = TestMachine::new("machine-alpha");
    let beta = TestMachine::new("machine-beta");
    seed_project(&alpha, &endpoint, "github.com/acme/widgets");
    seed_project(&beta, &endpoint, "github.com/acme/widgets");
    // The machines (and their temp directories) live for the whole test.
    let (a, b) = (alpha.cas_dir.clone(), beta.cas_dir.clone());
    let (a2, b2) = (a.clone(), b.clone());
    tokio::task::spawn_blocking(move || {
        supervisor_at(&a2, "machine-alpha", "a");
        supervisor_at(&b2, "machine-beta", "b");
    })
    .await
    .unwrap();
    let (service_a, service_b) = (service_for(&a, "a"), service_for(&b, "b"));

    let urgent = coordinate(
        &service_a,
        json!({"action": "message", "target": "sup-b@machine-beta", "summary": "now", "message": "stop", "urgent": true}),
    )
    .await
    .unwrap_err();
    assert!(urgent.contains("cannot be urgent"), "{urgent}");

    let sent = coordinate(
        &service_a,
        json!({"action": "message", "target": "sup-b@machine-beta", "summary": "take cas-1234?", "message": "can you take cas-1234?", "kind": "ask"}),
    )
    .await
    .unwrap();
    assert!(
        sent.contains("Message sent to peer supervisor sup-b@"),
        "{sent}"
    );
    assert!(sent.contains("peer message pm_1"), "{sent}");

    let bb = b.clone();
    let rows = tokio::task::spawn_blocking(move || {
        tick(&bb, "b");
        inbox(&bb)
    })
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);

    // pin: the receiver's peer envelope carries the typed ask attribute for the wake gate.
    assert!(rows[0].prompt.lines().next().unwrap().contains("kind=\"ask\""), "peer kind must survive the actual Cloud route: {}", rows[0].prompt);

    let replied = coordinate(
        &service_b,
        json!({"action": "message", "target": "sup-a", "summary": "taking it", "message": "yes", "in_reply_to": rows[0].id}),
    )
    .await
    .unwrap();
    assert!(
        replied.contains("Reply sent to peer supervisor sup-a@machine-alpha"),
        "{replied}"
    );

    let status = coordinate(
        &service_a,
        json!({"action": "message_status", "id": "pm_1"}),
    )
    .await
    .unwrap();
    assert!(status.contains("Peer message pm_1: delivered"), "{status}");

    let aa = a.clone();
    let replies = tokio::task::spawn_blocking(move || {
        tick(&aa, "a");
        inbox(&aa)
    })
    .await
    .unwrap();
    assert_eq!(replies.len(), 1);
    assert!(replies[0].prompt.ends_with("yes"));
    // pin: pm_1 is the fixture's Cloud parent id and must survive as reply correlation.
    assert!(replies[0].prompt.lines().next().unwrap().contains("in_reply_to=\"pm_1\""), "reply wake metadata must survive Cloud delivery: {}", replies[0].prompt);
    for (kind, blocker, wakes) in [(Some("blocker"), false, true), (None, true, true), (Some("status"), false, false)] {
        let body = format!("peer intent {kind:?} blocker={blocker}");
        coordinate(&service_a, json!({"action":"message", "target":"sup-b@machine-beta", "summary":"peer intent", "message":body, "kind":kind, "blocker":blocker})).await.unwrap();
        let bb = b.clone();
        let rows = tokio::task::spawn_blocking(move || { tick(&bb, "b"); inbox(&bb) }).await.unwrap();
        let row = rows.iter().find(|row| row.prompt.ends_with(&body)).unwrap();
        let envelope = parse_envelope(&row.prompt).unwrap();
        assert_eq!(envelope.wakes_idle(), wakes, "{envelope:?}");
    }
}

fn public_outbox_status(root: &Path, id: &str) -> String {
    tokio::runtime::Handle::current()
        .block_on(coordinate(
            &service_for(root, "a"),
            json!({"action":"message_status","id":id}),
        ))
        .unwrap()
}

/// The cloud accepted the first send but its receipt was lost. Automatic
/// retry must reuse the local row key; a deliberate identical send is new.
struct AcceptThenFail {
    send: Send,
    first: std::sync::atomic::AtomicBool,
}
impl Respond for AcceptThenFail {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let accepted = self.send.respond(request);
        if self.first.swap(false, std::sync::atomic::Ordering::SeqCst) {
            ResponseTemplate::new(503).set_body_string("receipt lost after acceptance")
        } else {
            accepted
        }
    }
}

#[tokio::test]
async fn cas_13c7_automatic_retry_keeps_logical_send_key_and_identical_sends_distinct() {
    use cas::cloud::peer_mailbox::{PeerMailboxRuntime, outbox_status_text};
    let (server, registry, mailbox) = fake_cloud_with_mailbox().await;
    Mock::given(method("POST"))
        .and(path("/api/peer-messages"))
        .respond_with(AcceptThenFail {
            send: Send(registry, mailbox.clone()),
            first: std::sync::atomic::AtomicBool::new(true),
        })
        .with_priority(1)
        .mount(&server)
        .await;
    let endpoint = server.uri();
    tokio::task::spawn_blocking(move || {
        let alpha = TestMachine::new("machine-alpha");
        let beta = TestMachine::new("machine-beta");
        seed_project(&alpha, &endpoint, "github.com/acme/widgets");
        seed_project(&beta, &endpoint, "github.com/acme/widgets");
        supervisor(&alpha, "a");
        supervisor(&beta, "b");
        let peers = peers_of(&alpha.cas_dir, "a");
        let error = send_to_peer(&alpha.cas_dir, "a", &peers[0], "hold merges", None, None)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("503") && error.contains("queued for automatic retry as peer-outbox:1"),
            "{error}"
        );
        let retrying = outbox_status_text(&alpha.cas_dir, "peer-outbox:1")
            .unwrap()
            .unwrap();
        assert!(
            retrying.contains(": retrying") && retrying.contains("503"),
            "{retrying}"
        );
        assert!(public_outbox_status(&alpha.cas_dir, "peer-outbox:1").contains(": retrying"));
        let second =
            send_to_peer(&alpha.cas_dir, "a", &peers[0], "hold merges", None, None).unwrap();
        assert_ne!(second.id, "pm_1");
        // Simulate restart after local enqueue but before taking the retry lease.
        let conn = rusqlite::Connection::open(alpha.cas_dir.join("cas.db")).unwrap();
        conn.execute(
            "UPDATE peer_message_outbox SET state='pending',next_attempt_at=0 WHERE id=1",
            [],
        )
        .unwrap();
        assert!(
            outbox_status_text(&alpha.cas_dir, "peer-outbox:1")
                .unwrap()
                .unwrap()
                .contains(": pending")
        );
        assert!(public_outbox_status(&alpha.cas_dir, "peer-outbox:1").contains(": pending"));
        // Competing daemon consumers share a project outbox; its atomic row
        // lease permits one HTTP retry even when both observe a due send.
        let mut second_supervisor = Agent::new("second-local".into(), "sup-second".into());
        second_supervisor.role = AgentRole::Supervisor;
        second_supervisor.factory_session = Some("other-session".into());
        open_agent_store(&alpha.cas_dir)
            .unwrap()
            .register(&second_supervisor)
            .unwrap();
        std::thread::scope(|scope| {
            for session in ["factory-a", "other-session"] {
                let root = &alpha.cas_dir;
                scope.spawn(move || {
                    let report = PeerMailboxRuntime::start(root, session)
                        .unwrap()
                        .tick_blocking();
                    assert!(report.errors.is_empty(), "{report:?}");
                });
            }
        });
        assert_eq!(
            conn.query_row(
                "SELECT attempts FROM peer_message_outbox WHERE id=1",
                [],
                |r| r.get::<_, u32>(0)
            )
            .unwrap(),
            2
        );
        let sent = outbox_status_text(&alpha.cas_dir, "peer-outbox:1")
            .unwrap()
            .unwrap();
        assert!(
            sent.contains(": sent") && sent.contains("cloud message pm_1"),
            "{sent}"
        );
        assert!(public_outbox_status(&alpha.cas_dir, "peer-outbox:1").contains(": sent"));
        assert_eq!(tick(&beta.cas_dir, "b").delivered, 2);
        assert_eq!(inbox(&beta.cas_dir).len(), 2);
        assert_eq!(
            mailbox.lock().unwrap().len(),
            2,
            "cloud retry must not duplicate accepted send"
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn cas_13c7_outbox_expires_once_and_notifies_sender_after_restart() {
    use cas::cloud::peer_mailbox::{OUTBOX_TTL_SECS, PeerMailboxRuntime, outbox_status_text};
    let (server, _registry, _mailbox) = fake_cloud_with_mailbox().await;
    Mock::given(method("POST"))
        .and(path("/api/peer-messages"))
        .respond_with(ResponseTemplate::new(503).set_body_string("temporarily unavailable"))
        .with_priority(1)
        .mount(&server)
        .await;
    let endpoint = server.uri();
    tokio::task::spawn_blocking(move || {
        let alpha = TestMachine::new("machine-alpha");
        let beta = TestMachine::new("machine-beta");
        seed_project(&alpha, &endpoint, "github.com/acme/widgets");
        seed_project(&beta, &endpoint, "github.com/acme/widgets");
        supervisor(&alpha, "a");
        supervisor(&beta, "b");
        let peers = peers_of(&alpha.cas_dir, "a");
        assert!(send_to_peer(&alpha.cas_dir, "a", &peers[0], "will expire", None, None).is_err());
        let conn = rusqlite::Connection::open(alpha.cas_dir.join("cas.db")).unwrap();
        conn.execute(
            "UPDATE peer_message_outbox SET created_at=?1,next_attempt_at=0 WHERE id=1",
            [chrono::Utc::now().timestamp() - OUTBOX_TTL_SECS - 1],
        )
        .unwrap();
        let agents = open_agent_store(&alpha.cas_dir).unwrap();
        let mut sender = agents.get("a").unwrap();
        sender.factory_session = Some("restarted-session".into());
        agents.update(&sender).unwrap();
        for _ in 0..2 {
            let report = PeerMailboxRuntime::start(&alpha.cas_dir, "restarted-session")
                .unwrap()
                .tick_blocking();
            assert!(report.errors.is_empty(), "{report:?}");
        }
        let expired = outbox_status_text(&alpha.cas_dir, "peer-outbox:1")
            .unwrap()
            .unwrap();
        assert!(
            expired.contains(": abandoned") && expired.contains("TTL expired"),
            "{expired}"
        );
        assert!(public_outbox_status(&alpha.cas_dir, "peer-outbox:1").contains(": abandoned"));
        let notices = inbox(&alpha.cas_dir);
        assert_eq!(
            notices.len(),
            1,
            "restart must not duplicate sender abandonment notice"
        );
        assert_eq!(notices[0].target, "sup-a");
        // pin: abandonment must identify the durable outbox receipt for status lookup.
        assert!(notices[0].prompt.contains("peer-outbox:1"));
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn cas_13c7_auth_failure_does_not_retry_and_nonfactory_recipient_is_typed_refusal() {
    use cas::cloud::peer_mailbox::{PeerMailboxRuntime, PeerSendError, outbox_status_text};
    let (server, _registry, mailbox) = fake_cloud_with_mailbox().await;
    Mock::given(method("POST"))
        .and(path("/api/peer-messages"))
        .respond_with(ResponseTemplate::new(401).set_body_string("login expired"))
        .with_priority(1)
        .expect(1)
        .mount(&server)
        .await;
    let endpoint = server.uri();
    tokio::task::spawn_blocking(move || {
        let alpha = TestMachine::new("machine-alpha");
        let beta = TestMachine::new("machine-beta");
        seed_project(&alpha, &endpoint, "github.com/acme/widgets");
        seed_project(&beta, &endpoint, "github.com/acme/widgets");
        supervisor(&alpha, "a");
        supervisor(&beta, "b");
        let mut peer = peers_of(&alpha.cas_dir, "a").remove(0);
        peer.session = None;
        assert!(matches!(
            send_to_peer(&alpha.cas_dir, "a", &peer, "hello", None, None),
            Err(PeerSendError::NoReceivingDaemon(_))
        ));
        peer.session = Some("factory-b".into());
        let error = send_to_peer(&alpha.cas_dir, "a", &peer, "hello", None, None)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("401") && !error.contains("queued for automatic retry"),
            "{error}"
        );
        assert!(
            outbox_status_text(&alpha.cas_dir, "peer-outbox:1")
                .unwrap()
                .unwrap()
                .contains(": abandoned")
        );
        PeerMailboxRuntime::start(&alpha.cas_dir, "factory-a")
            .unwrap()
            .tick_blocking();
        assert!(mailbox.lock().unwrap().is_empty());
        std::fs::remove_file(alpha.cas_dir.join("cloud.json")).unwrap();
        assert!(send_to_peer(&alpha.cas_dir, "a", &peer, "logged out", None, None).is_err());
        let conn = rusqlite::Connection::open(alpha.cas_dir.join("cas.db")).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM peer_message_outbox", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1,
            "no retry row while logged out"
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn cas_13c7_ack_checks_each_result_and_claim_denial_is_visible() {
    let (server, _registry, _mailbox) = fake_cloud_with_mailbox().await;
    Mock::given(method("POST"))
        .and(path("/api/peer-messages/ack"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"results":[{"id":"pm_1","status":409}]})),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/peer-messages/pm_1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"pm_1","status":"leased","attempts":2})),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/peer-messages/claim"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"messages":[],"denied":["github.com/acme/widgets"]})),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    let endpoint = server.uri();
    tokio::task::spawn_blocking(move || {
        let machine = TestMachine::new("machine-alpha");
        seed_project(&machine, &endpoint, "github.com/acme/widgets");
        supervisor(&machine, "a");
        let mailbox = http_mailbox(&machine.cas_dir).unwrap();
        let error = mailbox
            .ack(
                "same-consumer",
                &[(
                    "pm_1".into(),
                    cas::cloud::peer_mailbox::PeerAckOutcome::Delivered,
                )],
            )
            .unwrap_err();
        assert!(error.contains("409") && error.contains("leased"), "{error}");
        let report = tick(&machine.cas_dir, "a");
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("denied") && e.contains("github.com/acme/widgets")),
            "{report:?}"
        );
    })
    .await
    .unwrap();
}

/// GH #1186's unqualified target must receive an honest, immediate outcome.
#[tokio::test]
async fn cas_a19b_unknown_target_never_promises_registration() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, _, _) = fake_cloud_with_mailbox().await;
    let machine = TestMachine::new("soundwave");
    seed_project(&machine, &server.uri(), "github.com/acme/widgets");
    let root = machine.cas_dir.clone();
    tokio::task::spawn_blocking(move || supervisor_at(&root, "soundwave", "a"))
        .await
        .unwrap();
    for target in ["calm-marten-67", "calm-marten-67@prowl"] {
        let result = coordinate(
            &service_for(&machine.cas_dir, "a"),
            json!({
                "action": "message", "target": target, "summary": "hold staging",
                "message": "Please hold staging merges while I promote staging to main."
            }),
        )
        .await
        .unwrap_err();
        assert!(
            result.contains(&format!(
                "no supervisor or worker named {target} in this project or its peers"
            )),
            "{result}"
        );
        assert!(!result.contains("will deliver on registration"), "{result}");
        assert!(inbox(&machine.cas_dir).is_empty());
    }
}

#[tokio::test]
async fn cas_a19b_named_remote_target_returns_a_mailbox_receipt() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, registry, mailbox) = fake_cloud_with_mailbox().await;
    let alpha = TestMachine::new("soundwave");
    let beta = TestMachine::new("prowl");
    seed_project(&alpha, &server.uri(), "github.com/acme/widgets");
    seed_project(&beta, &server.uri(), "github.com/acme/widgets");
    let (a, b) = (alpha.cas_dir.clone(), beta.cas_dir.clone());
    tokio::task::spawn_blocking(move || {
        supervisor_at(&a, "soundwave", "a");
        supervisor_at(&b, "prowl", "b");
    })
    .await
    .unwrap();
    registry.lock().unwrap().get_mut("b").unwrap()["name"] = json!("calm-marten-67");
    let sent = coordinate(
        &service_for(&alpha.cas_dir, "a"),
        json!({
            "action": "message", "target": "calm-marten-67", "summary": "hold staging",
            "message": "Please hold staging merges while I promote staging to main."
        }),
    )
    .await
    .unwrap();
    assert!(
        sent.contains("delivered to calm-marten-67@prowl mailbox"),
        "{sent}"
    );
    assert!(sent.contains("pm_1"), "{sent}");
    assert_eq!(mailbox.lock().unwrap().len(), 1);
    assert!(inbox(&alpha.cas_dir).is_empty());
}

#[tokio::test]
async fn cas_a19b_unqualified_target_names_cloud_lookup_failures() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, _, _) = fake_cloud_with_mailbox().await;
    let machine = TestMachine::new("soundwave");
    seed_project(&machine, &server.uri(), "github.com/acme/widgets");
    let root = machine.cas_dir.clone();
    tokio::task::spawn_blocking(move || supervisor_at(&root, "soundwave", "a"))
        .await
        .unwrap();
    let service = service_for(&machine.cas_dir, "a");
    let request = json!({"action": "message", "target": "calm-marten-67", "summary": "hold staging", "message": "hold staging"});
    for status in [401, 503] {
        server.reset().await;
        Mock::given(method("GET"))
            .and(path("/api/agents"))
            .respond_with(ResponseTemplate::new(status))
            .with_priority(1)
            .mount(&server)
            .await;
        let error = coordinate(&service, request.clone()).await.unwrap_err();
        assert!(
            error.contains(&status.to_string()) && error.contains("calm-marten-67"),
            "{error}"
        );
        assert!(inbox(&machine.cas_dir).is_empty());
    }
    let mut config = cas::cloud::CloudConfig::load_from_cas_dir(&machine.cas_dir).unwrap();
    config.token = None;
    config.save_to_cas_dir(&machine.cas_dir).unwrap();
    let error = coordinate(&service, request.clone()).await.unwrap_err();
    assert!(
        error.contains("no token") && error.contains("calm-marten-67"),
        "{error}"
    );
    config.token = Some("test-token".to_string());
    // A closed loopback listener deterministically refuses the connection.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    config.endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    config.save_to_cas_dir(&machine.cas_dir).unwrap();
    let error = coordinate(&service, request).await.unwrap_err();
    assert!(
        error.to_lowercase().contains("network") && error.contains("calm-marten-67"),
        "{error}"
    );
    assert!(inbox(&machine.cas_dir).is_empty());
}

#[tokio::test]
async fn cas_a19b_known_peer_names_mailbox_failure() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, _, _) = fake_cloud_with_mailbox().await;
    let alpha = TestMachine::new("soundwave");
    let beta = TestMachine::new("prowl");
    seed_project(&alpha, &server.uri(), "github.com/acme/widgets");
    seed_project(&beta, &server.uri(), "github.com/acme/widgets");
    let (a, b) = (alpha.cas_dir.clone(), beta.cas_dir.clone());
    tokio::task::spawn_blocking(move || {
        supervisor_at(&a, "soundwave", "a");
        supervisor_at(&b, "prowl", "b");
    })
    .await
    .unwrap();
    for (status, cause) in [
        (401, "unauthorized"),
        (404, "recipient_not_found"),
        (403, "sender_not_owned"),
        (409, "dedupe_key_conflict"),
        (400, "invalid_project"),
        (400, "invalid_in_reply_to"),
        (400, "invalid_request"),
    ] {
        Mock::given(method("POST"))
            .and(path("/api/peer-messages"))
            .respond_with(ResponseTemplate::new(status).set_body_json(json!({"error": cause})))
            .with_priority(1)
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let error = coordinate(&service_for(&alpha.cas_dir, "a"), json!({
        "action": "message", "target": "sup-b", "summary": "hold staging", "message": format!("hold staging ({cause})")
    })).await.unwrap_err();
        assert!(
            error.contains(&status.to_string())
                && error.contains(cause)
                && error.contains("sup-b@prowl"),
            "{error}"
        );
        assert!(inbox(&alpha.cas_dir).is_empty());
    }
}

#[tokio::test]
async fn cas_a19b_status_404_names_visibility_without_claiming_delivery() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, _, _) = fake_cloud_with_mailbox().await;
    let machine = TestMachine::new("soundwave");
    seed_project(&machine, &server.uri(), "github.com/acme/widgets");
    let root = machine.cas_dir.clone();
    tokio::task::spawn_blocking(move || supervisor_at(&root, "soundwave", "a"))
        .await
        .unwrap();
    let error = coordinate(
        &service_for(&machine.cas_dir, "a"),
        json!({
            "action": "message_status", "id": "pm_404"
        }),
    )
    .await
    .unwrap_err();
    assert!(
        error.contains("pm_404") && error.contains("not visible"),
        "{error}"
    );
}

#[tokio::test]
async fn cas_a19b_only_a_pending_local_spawn_can_wait_for_registration() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, _, _) = fake_cloud_with_mailbox().await;
    let machine = TestMachine::new("soundwave");
    let mut agent = Agent::new("a".into(), "sup-a".into());
    agent.role = AgentRole::Supervisor;
    agent.factory_session = Some("factory-a".into());
    open_agent_store(&machine.cas_dir)
        .unwrap()
        .register(&agent)
        .unwrap();
    let spawns = cas::store::open_spawn_queue_store(&machine.cas_dir).unwrap();
    let id = spawns
        .enqueue_spawn(
            1,
            &["new-fox-1".into()],
            true,
            None,
            Some("factory-a"),
            None,
        )
        .unwrap();
    let request =
        json!({"action": "message", "target": "new-fox-1", "summary": "hello", "message": "hello"});
    let service = service_for(&machine.cas_dir, "a");
    let sent = coordinate(&service, request.clone()).await.unwrap();
    let rows = inbox(&machine.cas_dir);
    assert_eq!(rows.len(), 1);
    assert!(
        sent.starts_with(&format!("queued #{} → new-fox-1 (enqueued)\n", rows[0].id)),
        "{sent}"
    );
    assert!(sent.contains("Target is not registered yet"), "{sent}");
    assert!(
        sent.contains("the row will deliver when it registers"),
        "{sent}"
    );
    assert!(
        sent.contains(&format!("message_status notification_id={}", rows[0].id)),
        "{sent}"
    );
    // Once the spawn fails, an empty cloud lookup can establish that the
    // name is unknown both locally and remotely.
    seed_project(&machine, &server.uri(), "github.com/acme/widgets");
    spawns
        .record_spawn_state(
            id,
            cas_store::SpawnLifecycleState::Failed,
            None,
            Some("launch failed"),
        )
        .unwrap();
    let error = coordinate(&service, request).await.unwrap_err();
    assert!(
        error.contains("no supervisor or worker named new-fox-1"),
        "{error}"
    );
    assert_eq!(inbox(&machine.cas_dir).len(), 1);
}

#[test]
fn cas_a19b_abandonment_notifies_sender_once_with_id_and_reason() {
    let machine = TestMachine::new("soundwave");
    let queue = open_prompt_queue_store(&machine.cas_dir).unwrap();
    let id = queue
        .enqueue_with_session("sup-a", "missing-worker", "hello", "factory-a")
        .unwrap();
    queue
        .abandon_ineligible_session_targets(&["sup-a"], "factory-a", -1)
        .unwrap();
    let notices = queue
        .poll_unseen_for_recipient("sup-a", Some("factory-a"), 10)
        .unwrap();
    assert_eq!(notices.len(), 1, "sender receives abandonment notice");
    assert!(
        notices[0].prompt.contains(&id.to_string())
            && notices[0]
                .prompt
                .contains("target no longer belongs to factory session"),
        "{}",
        notices[0].prompt
    );
    assert_eq!(
        queue
            .abandon_ineligible_session_targets(&["sup-a"], "factory-a", -1)
            .unwrap(),
        0
    );
    // Test an unread notice separately: inbox polling above correctly marks
    // delivery, so abandoning that already delivered row would be illegal.
    let another = queue
        .enqueue_with_session("sup-a", "missing-worker", "again", "factory-a")
        .unwrap();
    queue.mark_abandoned(another, Some("target gone")).unwrap();
    let pending = queue.peek_all(10).unwrap();
    assert_eq!(pending.len(), 1);
    queue
        .mark_abandoned(pending[0].id, Some("sender gone"))
        .unwrap();
    assert!(queue.peek_all(10).unwrap().is_empty());
}

#[tokio::test]
async fn cas_a19b_unconfigured_project_fails_immediately() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let machine = TestMachine::new("soundwave");
    let mut agent = Agent::new("a".into(), "sup-a".into());
    agent.role = AgentRole::Supervisor;
    open_agent_store(&machine.cas_dir)
        .unwrap()
        .register(&agent)
        .unwrap();
    for target in ["calm-marten-67", "calm-marten-67@prowl"] {
        let error = coordinate(&service_for(&machine.cas_dir, "a"), json!({
            "action": "message", "target": target, "summary": "hold staging", "message": "hold staging"
        })).await.unwrap_err();
        assert!(
            error.contains("no token") && error.contains(target),
            "{error}"
        );
        assert!(inbox(&machine.cas_dir).is_empty());
    }
}

/// All terminal paths notify the authenticated sender in its own session,
/// including when a supervisor's worker-facing source label is "supervisor".
#[test]
fn cas_a19b_every_abandonment_path_notifies_the_registered_sender() {
    for path in ["retry", "explicit", "stale", "manual"] {
        let machine = TestMachine::new("soundwave");
        let mut sender = Agent::new("sender-id".into(), "calm-puma-54".into());
        sender.role = AgentRole::Supervisor;
        sender.factory_session = Some("sender-session".into());
        open_agent_store(&machine.cas_dir)
            .unwrap()
            .register(&sender)
            .unwrap();
        let queue = open_prompt_queue_store(&machine.cas_dir).unwrap();
        let id = queue
            .enqueue_urgent_with_outcome(
                "supervisor",
                "lost-worker",
                "hold staging",
                Some("recipient-session"),
                None,
                None,
                false,
                Some(&cas_store::QueueOrigin::RegisteredAgent {
                    agent_id: sender.id,
                }),
            )
            .unwrap()
            .id();
        let old = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        rusqlite::Connection::open(machine.cas_dir.join("cas.db"))
            .unwrap()
            .execute(
                "UPDATE prompt_queue SET created_at = ?1, first_attempt_at = ?1 WHERE id = ?2",
                rusqlite::params![old, id],
            )
            .unwrap();
        match path {
            "retry" => {
                queue
                    .record_retry(
                        id,
                        cas_store::PendingReason::AdapterRetryable,
                        Some("network unavailable"),
                    )
                    .unwrap();
            }
            "explicit" => {
                queue.abandon_pending_older_than(900).unwrap();
            }
            "stale" => {
                queue.expire_stale_pending(900).unwrap();
            }
            "manual" => {
                queue
                    .mark_abandoned(id, Some("target disappeared"))
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let report = queue.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(report.stage, cas_store::DeliveryStage::Abandoned, "{path}");
        let notices = queue
            .poll_unseen_for_recipient("calm-puma-54", Some("sender-session"), 10)
            .unwrap();
        assert_eq!(notices.len(), 1, "{path}");
        assert!(
            notices[0]
                .prompt
                .contains(&format!("Message {id} to lost-worker")),
            "{}",
            notices[0].prompt
        );
        assert!(
            notices[0]
                .prompt
                .contains(report.pending_detail.as_deref().unwrap()),
            "{}",
            notices[0].prompt
        );
        assert!(
            queue
                .poll_unseen_for_recipient("supervisor", Some("recipient-session"), 10)
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn cas_a19b_peer_without_a_factory_daemon_is_refused() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, registry, mailbox) = fake_cloud_with_mailbox().await;
    let alpha = TestMachine::new("soundwave");
    let beta = TestMachine::new("prowl");
    seed_project(&alpha, &server.uri(), "github.com/acme/widgets");
    seed_project(&beta, &server.uri(), "github.com/acme/widgets");
    let (a, b) = (alpha.cas_dir.clone(), beta.cas_dir.clone());
    tokio::task::spawn_blocking(move || {
        supervisor_at(&a, "soundwave", "a");
        supervisor_at(&b, "prowl", "b");
    })
    .await
    .unwrap();
    registry.lock().unwrap().get_mut("b").unwrap()["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("factory_session");
    let error = coordinate(&service_for(&alpha.cas_dir, "a"), json!({
        "action": "message", "target": "sup-b", "summary": "hold staging", "message": "hold staging"
    })).await.unwrap_err();
    assert!(
        error.contains(
            "sup-b@prowl is not running a factory daemon, so it cannot receive peer messages"
        ),
        "{error}"
    );
    assert!(mailbox.lock().unwrap().is_empty());
    assert!(inbox(&alpha.cas_dir).is_empty());
}

#[test]
fn cas_a19b_peer_abandonment_survives_an_unregistered_sender() {
    let machine = TestMachine::new("soundwave");
    let agents = open_agent_store(&machine.cas_dir).unwrap();
    let mut sender = Agent::new("old-sender".into(), "calm-puma-54".into());
    sender.role = AgentRole::Supervisor;
    sender.factory_session = Some("ended-session".into());
    agents.register(&sender).unwrap();
    agents.unregister(&sender.id).unwrap();
    let mut successor = Agent::new("new-sender".into(), "new-puma-55".into());
    successor.role = AgentRole::Supervisor;
    successor.factory_session = Some("new-session".into());
    agents.register(&successor).unwrap();
    let queue = open_prompt_queue_store(&machine.cas_dir).unwrap();
    for _ in 0..2 {
        queue
            .notify_peer_send_abandoned(
                &sender.id,
                &sender.name,
                sender.factory_session.as_deref(),
                42,
                "calm-marten-67@prowl",
                "retry deadline expired",
            )
            .unwrap();
    }
    let notices = queue
        .poll_unseen_for_recipient("new-puma-55", Some("new-session"), 10)
        .unwrap();
    assert_eq!(notices.len(), 1);
    assert!(
        // pin: abandonment must retain its durable outbox receipt across sessions.
        notices[0].prompt.contains("peer-outbox:42")
            // pin: the failure reason must survive rerouting to the successor session.
            && notices[0].prompt.contains("retry deadline expired")
            // pin: the notice must name the original sender after that sender exits.
            && notices[0].prompt.contains("calm-puma-54"),
        "{}",
        notices[0].prompt
    );
    // With no live supervisor, the notice remains visible to a future session.
    agents.unregister(&successor.id).unwrap();
    queue
        .notify_peer_send_abandoned(
            &sender.id,
            &sender.name,
            sender.factory_session.as_deref(),
            43,
            "calm-marten-67@prowl",
            "retry deadline expired",
        )
        .unwrap();
    let future = queue
        .poll_unseen_for_recipient("supervisor", Some("future-session"), 10)
        .unwrap();
    assert_eq!(future.len(), 1);
    // pin: the future-session notice must identify its own durable outbox receipt.
    assert!(future[0].prompt.contains("peer-outbox:43"));
}
