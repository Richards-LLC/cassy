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
        let addressable = if body["scope"] == "user" {
            sender["user_id"].as_str().is_some()
                && sender["user_id"] == recipient["user_id"]
                && recipient["metadata"]["role"] == "supervisor"
        } else {
            cas::cloud::project_ids_match_with_aliases(recipient_repo, project, &[])
        };
        if !addressable {
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
            "scope": body.get("scope").cloned().unwrap_or_else(|| json!("project")),
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
            None,
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
            scope: Default::default(),
            recipient_project_id: None,
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
            scope: Default::default(),
            recipient_project_id: None,
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
    // Untyped peer FYIs default to receipts; explicit actions still wake.
    for (kind, blocker, wakes) in [
        (None, false, false),
        (Some("blocker"), false, true), (None, true, true), (Some("status"), false, true),
        (Some("receipt"), false, false), (Some("quiet"), false, false),
    ] {
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

#[tokio::test]
async fn ct2_p3_overcap_peer_body_readable() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, _registry, mailbox) = fake_cloud_with_mailbox().await;
    let alpha = TestMachine::new("machine-alpha");
    let beta = TestMachine::new("machine-beta");
    seed_project(&alpha, &server.uri(), "github.com/acme/widgets");
    seed_project(&beta, &server.uri(), "github.com/acme/widgets");
    let (a, b) = (alpha.cas_dir.clone(), beta.cas_dir.clone());
    let (aa, bb) = (a.clone(), b.clone());
    tokio::task::spawn_blocking(move || {
        supervisor_at(&aa, "machine-alpha", "a");
        supervisor_at(&bb, "machine-beta", "b");
    }).await.unwrap();
    let (sender, receiver) = (service_for(&a, "a"), service_for(&b, "b"));
    let body = format!("contract-v2 over-cap body\n{}\nend-of-body", "readable 体\n".repeat(400));
    assert!(body.chars().count() > cas::config::Config::default().factory().message_max_chars);
    coordinate(&sender, json!({"action":"message", "target":"sup-b@machine-beta", "summary":"long peer body", "message":body})).await.unwrap();
    assert_eq!(mailbox.lock().unwrap()[0]["body"].as_str(), Some(body.as_str()));
    let rows = tokio::task::spawn_blocking(move || { tick(&b, "b"); inbox(&b) }).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].prompt.ends_with(&body));
    let reply = format!("reply\n{body}");
    coordinate(&receiver, json!({"action":"message", "target":"sup-a@machine-alpha", "summary":"long peer reply", "message":reply, "in_reply_to":rows[0].id})).await.unwrap();
    assert_eq!(mailbox.lock().unwrap()[1]["body"].as_str(), Some(reply.as_str()));
    let rows = tokio::task::spawn_blocking(move || { tick(&a, "a"); inbox(&a) }).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].prompt.ends_with(&reply));
}


async fn restart_peer_mail(unregister: bool, legacy: bool, fail_notice: bool) {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, registry, mailbox) = fake_cloud_with_mailbox().await;
    if fail_notice {
        Mock::given(method("POST"))
            .and(path("/api/peer-messages"))
            .and(wiremock::matchers::body_partial_json(
                json!({"sender_agent_id":"b-new"}),
            ))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
    }
    let endpoint = server.uri();
    let alpha = TestMachine::new("machine-alpha");
    let beta = TestMachine::new("machine-beta");
    tokio::task::spawn_blocking(move || {
        seed_project(&alpha, &endpoint, "github.com/acme/widgets");
        seed_project(&beta, &endpoint, "github.com/acme/widgets");
        supervisor(&alpha, "a");
        let register = |id: &str| {
            let mut agent = Agent::new(id.into(), "peer-beta".into());
            agent.role = AgentRole::Supervisor;
            agent.machine_id = Some("machine-beta".into());
            // The named factory survives a full daemon restart; only its
            // supervisor agent ID/process lifetime changes.
            agent.factory_session = Some("factory-b-new".into());
            let config = cas::cloud::CloudConfig::load_from_cas_dir(&beta.cas_dir).unwrap();
            cas::cloud::CloudCoordinator::new(config)
                .unwrap()
                .with_canonical_id(Some("github.com/acme/widgets".into()))
                .register_with_focus(&agent, Some("cas-571d"))
                .unwrap();
            open_agent_store(&beta.cas_dir)
                .unwrap()
                .register(&agent)
                .unwrap();
        };
        register("b-old");
        let agents = open_agent_store(&beta.cas_dir).unwrap();
        if unregister {
            agents.unregister("b-old").unwrap();
            if legacy {
                // A pre-upgrade SessionEnd row had no typed retirement metadata.
                rusqlite::Connection::open(beta.cas_dir.join("cas.db"))
                    .unwrap()
                    .execute(
                        "UPDATE events SET metadata = NULL WHERE entity_id = 'b-old'",
                        [],
                    )
                    .unwrap();
            }
        } else {
            let mut old = agents.get("b-old").unwrap();
            old.status = cas::types::AgentStatus::Shutdown;
            agents.update(&old).unwrap();
        }
        // Cloud presence can lag a locally ended session (including a
        // missed shutdown). The stale address is still a valid mailbox.
        registry.lock().unwrap().get_mut("b-old").unwrap()["status"] = json!("stale");
        let peers = peers_of(&alpha.cas_dir, "a");
        let old_peer = resolve_peer_target(&peers, "peer-beta@machine-beta")
            .unwrap()
            .unwrap();
        let receipt =
            send_to_peer(&alpha.cas_dir, "a", old_peer, "mail while down", None, None).unwrap();
        assert_eq!(mailbox.lock().unwrap()[0]["recipient_agent_id"], "b-old");
        assert!(inbox(&beta.cas_dir).is_empty());
        register("b-new");
        let mut report = tick(&beta.cas_dir, "b-new");
        if fail_notice {
            assert_eq!((report.delivered, report.rejected), (0, 0), "{report:?}");
            assert!(!report.errors.is_empty());
            assert_eq!(
                mailbox.lock().unwrap()[0]["status"],
                "leased",
                "failed feedback must not terminally ACK original"
            );
            assert!(inbox(&beta.cas_dir).is_empty());
            // Simulate the claim lease expiring; the body remains in Cloud.
            mailbox.lock().unwrap()[0]["status"] = json!("queued");
            report = tick(&beta.cas_dir, "b-new");
        }
        if legacy {
            assert_eq!((report.delivered, report.rejected), (0, 1), "{report:?}");
            assert!(
                inbox(&beta.cas_dir).is_empty(),
                "unverified body is never admitted"
            );
            assert_eq!(mailbox.lock().unwrap()[0]["status"], "rejected");
            assert_eq!(tick(&alpha.cas_dir, "a").delivered, 1);
            let notices = inbox(&alpha.cas_dir);
            assert_eq!(notices.len(), 1);
            assert_eq!(
                parse_envelope(&notices[0].prompt)
                    .unwrap()
                    .in_reply_to
                    .as_deref(),
                Some(receipt.id.as_str())
            );
            assert!(notices[0].prompt.contains("b-new"));
            assert!(notices[0].prompt.to_ascii_lowercase().contains("resend"));
            // pin: a routing failure notice must not forward the unverified original body.
            assert!(!notices[0].prompt.contains("mail while down"));
            // A second successor must reuse the original message's feedback
            // key rather than manufacture another notice under its new ID.
            register("b-next");
            mailbox.lock().unwrap()[0]["status"] = json!("queued");
            let replay = cas::cloud::peer_mailbox::PeerMailboxRuntime::start(
                &beta.cas_dir,
                "factory-b-new",
            )
            .unwrap()
            .tick_blocking();
            assert_eq!(replay.rejected, 1);
            assert_eq!(
                mailbox.lock().unwrap().len(),
                2,
                "notice send is idempotent on redelivery"
            );
            assert_eq!(tick(&alpha.cas_dir, "a").claimed, 0);
            assert_eq!(inbox(&alpha.cas_dir).len(), 1);
            return;
        }
        assert_eq!(
            (report.claimed, report.delivered, report.rejected),
            (1, 1, 0),
            "unregister={unregister}: {report:?}"
        );
        let rows = inbox(&beta.cas_dir);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, "peer-beta");
        assert_eq!(rows[0].factory_session.as_deref(), Some("factory-b-new"));
        assert_eq!(parse_envelope(&rows[0].prompt).unwrap().id, receipt.id);
        // pin: restart recovery must preserve the original cross-machine body.
        assert!(rows[0].prompt.ends_with("mail while down"));
        assert_eq!(tick(&beta.cas_dir, "b-new").claimed, 0);
        assert_eq!(
            http_mailbox(&alpha.cas_dir)
                .unwrap()
                .status(&receipt.id)
                .unwrap()
                .status,
            "delivered"
        );
        // Model redelivery after an uncertain ACK. The durable Cloud ID
        // remains the queue idempotency key, including across restarts.
        mailbox.lock().unwrap()[0]["status"] = json!("queued");
        let replay = tick(&beta.cas_dir, "b-new");
        assert_eq!((replay.delivered, replay.duplicates), (0, 1), "{replay:?}");
        assert_eq!(inbox(&beta.cas_dir).len(), 1);
        let peers = peers_of(&alpha.cas_dir, "a");
        assert_eq!(
            resolve_peer_target(&peers, "peer-beta@machine-beta")
                .unwrap()
                .unwrap()
                .agent_id,
            "b-new"
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn ct2_p3_peer_mail_survives_restart() {
    restart_peer_mail(false, false, false).await;
}

#[tokio::test]
async fn cas_8063_peer_mail_survives_unregister_restart() {
    restart_peer_mail(true, false, false).await;
}

#[tokio::test]
async fn cas_8063_legacy_deleted_address_notifies_sender_to_resend_without_forwarding_body() {
    restart_peer_mail(true, true, false).await;
}

#[tokio::test]
async fn cas_8063_failed_legacy_feedback_keeps_original_retryable() {
    restart_peer_mail(true, true, true).await;
}

#[tokio::test]
async fn cas_8063_repair_claim_batch_failure_preserves_other_batches() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/peer-messages/claim"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let first = body["recipient_agent_ids"][0].as_str().unwrap();
            if first == "r050" { return ResponseTemplate::new(503); }
            ResponseTemplate::new(200).set_body_json(json!({"messages":[{
                "id":first, "project_id":"github.com/acme/widgets", "recipient_agent_id":first,
                "sender_agent_id":"sender", "sender_name":"sender", "body":"body",
                "created_at":chrono::Utc::now().to_rfc3339()
            }],"denied":[]}))
        })
        .mount(&server).await;
    let endpoint = server.uri();
    tokio::task::spawn_blocking(move || {
        let mailbox = cas::cloud::peer_mailbox::HttpPeerMailbox::new(&endpoint, "test-token");
        let recipients: Vec<_> = (0..101).map(|i| format!("r{i:03}")).collect();
        let report = mailbox.claim_report(&["github.com/acme/widgets".into()], &recipients, "consumer")
            .expect("one failed claim must not discard successful batches");
        let ids: Vec<_> = report.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["r000", "r100"]);
        assert_eq!(report.errors.len(), 1, "partial failure remains visible");
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn cas_8063_repair_history_warning_keeps_mailbox_receiving() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, _, _) = fake_cloud_with_mailbox().await;
    let endpoint = server.uri();
    tokio::task::spawn_blocking(move || {
        let machine = TestMachine::new("machine-beta");
        seed_project(&machine, &endpoint, "github.com/acme/widgets");
        supervisor(&machine, "b");
        let events = cas::store::open_event_store(&machine.cas_dir).unwrap();
        // Mix typed and pre-metadata retirements: both must share the same
        // budget, including the legacy-name backlog seen in the incident.
        for i in 0..338 {
            let mut event = cas::types::Event::new(
                cas::types::EventType::AgentShutdown,
                cas::types::EventEntityType::Agent,
                format!("retired-{i}"),
                "Agent unregistered: sup-b",
            )
            .with_metadata(json!({"retired_agent": {
                "name":"sup-b", "role":"supervisor", "machine_id":"machine-beta",
                "factory_session":"factory-b",
                "registered_at":(chrono::Utc::now()-chrono::Duration::days(1)).to_rfc3339()
            }}));
            if i % 2 == 1 { event.metadata = None; }
            events.record(&event).unwrap();
        }
        let report = tick(&machine.cas_dir, "b");
        assert!(report.errors.is_empty(), "{report:?}");
        assert!(
            !report.warnings.is_empty(),
            "truncation must remain visible"
        );
        let text =
            cas::cloud::peer_mailbox::mailbox_status_section(&machine.cas_dir, Some("factory-b"));
        assert!(text.contains("Peer mailbox: receiving."), "{text}");
        assert!(text.contains("Warning:"), "{text}");
    })
    .await
    .unwrap();
    let requests = server.received_requests().await.unwrap();
    let recipients: Vec<String> = requests
        .iter()
        .filter(|r| r.url.path().ends_with("/claim"))
        .flat_map(|r| {
            serde_json::from_slice::<Value>(&r.body).unwrap()["recipient_agent_ids"]
                .as_array()
                .unwrap()
                .iter().map(|id| id.as_str().unwrap().to_string()).collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(recipients.len(), 101, "100 retired recipients plus current");
    assert!(recipients.iter().any(|id| id == "b"));
    assert!(recipients.iter().any(|id| id == "retired-337"), "newest retiree is included");
    assert!(!recipients.iter().any(|id| id == "retired-218"), "120th newest retiree is excluded");
}

#[tokio::test]
async fn cas_4fe2_partial_claim_survivors_are_admitted_and_acked_once() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, _, cloud_mail) = fake_cloud_with_mailbox().await;
    Mock::given(method("POST")).and(path("/api/peer-messages/claim"))
        .and(wiremock::matchers::body_partial_json(json!({"recipient_agent_ids": (50..100).map(|i| format!("r{i:03}")).collect::<Vec<_>>()})))
        .respond_with(ResponseTemplate::new(503)).with_priority(1).mount(&server).await;
    let endpoint = server.uri();
    tokio::task::spawn_blocking(move || {
        let machine = TestMachine::new("machine-beta");
        seed_project(&machine, &endpoint, "github.com/acme/widgets");
        let agents = open_agent_store(&machine.cas_dir).unwrap();
        // Two current IDs plus99 retirements keeps this independent of the
        // cap-size negative control while still crossing three HTTP chunks.
        for i in 2..=100 {
            let mut old = Agent::new(format!("r{i:03}"), "sup-b".into());
            old.role = AgentRole::Supervisor;
            old.status = cas::types::AgentStatus::Shutdown;
            old.factory_session = Some("factory-b".into());
            old.machine_id = Some("machine-beta".into());
            agents.register(&old).unwrap();
        }
        let mut current = Agent::new("r000".into(), "sup-b".into());
        current.role = AgentRole::Supervisor;
        current.factory_session = Some("factory-b".into());
        current.machine_id = Some("machine-beta".into());
        agents.register(&current).unwrap();
        let mut direct = current.clone();
        direct.id = "r001".into();
        direct.name = "direct-first".into();
        agents.register(&direct).unwrap();
        for id in ["r001", "r050", "r100"] {
            cloud_mail.lock().unwrap().push(json!({
                "id":format!("mail-{id}"), "project_id":"github.com/acme/widgets",
                "recipient_agent_id":id, "sender_agent_id":"remote", "sender_name":"remote",
                "body":format!("body-{id}"), "created_at":chrono::Utc::now().to_rfc3339(),
                "status":"queued", "attempts":0
            }));
        }
        let runtime = cas::cloud::peer_mailbox::PeerMailboxRuntime::start(&machine.cas_dir, "factory-b").unwrap();
        let report = runtime.tick_blocking();
        assert_eq!((report.claimed, report.delivered), (2, 2), "{report:?}");
        assert!(!report.errors.is_empty(), "failed middle batch stays visible");
        let rows = inbox(&machine.cas_dir);
        let ids: Vec<_> = rows.iter().map(|r| parse_envelope(&r.prompt).unwrap().id).collect();
        assert_eq!(ids, ["mail-r001", "mail-r100"]);
        assert_eq!(rows.iter().map(|row| row.target.as_str()).collect::<Vec<_>>(), ["direct-first", "sup-b"]);
        for row in &rows {
            assert_eq!(row.factory_session.as_deref(), Some("factory-b"));
        }
        let mut mail = cloud_mail.lock().unwrap();
        assert_eq!(mail[0]["status"], "delivered");
        assert_eq!(mail[1]["status"], "queued", "failed chunk is neither claimed nor ACKed");
        assert_eq!(mail[2]["status"], "delivered");
        mail[0]["status"] = json!("queued");
        mail[2]["status"] = json!("queued");
        drop(mail);
        let replay = runtime.tick_blocking();
        assert_eq!((replay.delivered, replay.duplicates), (0, 2), "{replay:?}");
        assert_eq!(inbox(&machine.cas_dir).len(), 2);
        let mail = cloud_mail.lock().unwrap();
        assert_eq!(mail[0]["status"], "delivered");
        assert_eq!(mail[1]["status"], "queued");
        assert_eq!(mail[2]["status"], "delivered");
    }).await.unwrap();
}

/// GH #1319 (cas-c922): soundwave's supervisor restarts. Mail prowl sends to
/// the ended identity, as a reply to its message or by its old name, reaches
/// the successor's inbox instead of an unread mailbox.
#[tokio::test]
async fn gh1319_mail_for_an_ended_supervisor_reaches_its_successor() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, registry, mailbox) = fake_cloud_with_mailbox().await;
    let prowl = TestMachine::new("prowl");
    let soundwave = TestMachine::new("soundwave");
    seed_project(&prowl, &server.uri(), "github.com/acme/widgets");
    seed_project(&soundwave, &server.uri(), "github.com/acme/widgets");
    let (p, s) = (prowl.cas_dir.clone(), soundwave.cas_dir.clone());
    let (p2, s2) = (p.clone(), s.clone());
    tokio::task::spawn_blocking(move || {
        supervisor_at(&p2, "prowl", "a");
        supervisor_at(&s2, "soundwave", "old");
    })
    .await
    .unwrap();
    coordinate(
        &service_for(&s, "old"),
        json!({"action": "message", "target": "sup-a@prowl", "summary": "take cas-1?", "message": "can you take cas-1?", "kind": "ask"}),
    )
    .await
    .unwrap();
    let pp = p.clone();
    let asked = tokio::task::spawn_blocking(move || {
        tick(&pp, "a");
        inbox(&pp)
    })
    .await
    .unwrap();
    assert_eq!(asked.len(), 1);

    // The restart: the old session shuts down and a new supervisor takes it.
    registry.lock().unwrap().get_mut("old").unwrap()["status"] = json!("shutdown");
    let s3 = s.clone();
    tokio::task::spawn_blocking(move || supervisor_at(&s3, "soundwave", "new"))
        .await
        .unwrap();
    let service_a = service_for(&p, "a");
    for request in [
        json!({"action": "message", "target": "sup-old@soundwave", "summary": "taking it", "message": "yes, taking cas-1", "in_reply_to": asked[0].id}),
        json!({"action": "message", "target": "sup-old", "summary": "and cas-2", "message": "taking cas-2 as well"}),
    ] {
        let sent = coordinate(&service_a, request).await.unwrap();
        assert!(
            sent.contains("sup-old@soundwave ended")
                && sent.contains("delivered to its successor sup-new@soundwave"),
            "{sent}"
        );
    }
    // cas-2db0: a split offer to the old identity names its successor too.
    let tasks = cas::store::open_task_store(&p).unwrap();
    let mut epic = cas::types::Task::new("cas-e1".into(), "shared epic".into());
    epic.task_type = cas::types::TaskType::Epic;
    tasks.add(&epic).unwrap();
    tasks.add(&cas::types::Task::new("cas-c1".into(), "child".into())).unwrap();
    tasks
        .add_dependency(&cas::types::Dependency::new(
            "cas-c1".into(),
            "cas-e1".into(),
            cas::types::DependencyType::ParentChild,
        ))
        .unwrap();
    let offer: cas_mcp::types::CoordinationRequest = serde_json::from_value(
        json!({"action": "epic_split", "id": "cas-e1", "target": "sup-old", "theirs": "cas-c1"}),
    )
    .unwrap();
    let offered = service_a
        .factory(rmcp::handler::server::wrapper::Parameters(offer))
        .await
        .map(|result| format!("{:?}", result.content))
        .unwrap();
    assert!(offered.contains("delivered to its successor sup-new@soundwave"), "{offered}");
    assert!(
        mailbox.lock().unwrap()[1..]
            .iter()
            .all(|message| message["recipient_agent_id"] == "new"),
        "nothing was left in the ended supervisor's mailbox"
    );
    let ss = s.clone();
    let rows = tokio::task::spawn_blocking(move || {
        tick(&ss, "new");
        inbox(&ss)
    })
    .await
    .unwrap();
    for body in ["yes, taking cas-1", "taking cas-2 as well"] {
        assert!(
            rows.iter().any(|row| row.target == "sup-new" && row.prompt.ends_with(body)),
            "{body}: {rows:?}"
        );
    }
}

/// GH #1319: an ended supervisor with no successor on its machine is a typed
/// refusal naming why, never a send that reads as delivered.
#[tokio::test]
async fn gh1319_mail_nobody_can_read_is_refused_with_its_reason() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, registry, mailbox) = fake_cloud_with_mailbox().await;
    let prowl = TestMachine::new("prowl");
    let soundwave = TestMachine::new("soundwave");
    seed_project(&prowl, &server.uri(), "github.com/acme/widgets");
    seed_project(&soundwave, &server.uri(), "github.com/acme/widgets");
    let (p, s) = (prowl.cas_dir.clone(), soundwave.cas_dir.clone());
    tokio::task::spawn_blocking(move || {
        supervisor_at(&p, "prowl", "a");
        supervisor_at(&s, "soundwave", "old");
    })
    .await
    .unwrap();
    registry.lock().unwrap().get_mut("old").unwrap()["status"] = json!("shutdown");
    let error = coordinate(
        &service_for(&prowl.cas_dir, "a"),
        json!({"action": "message", "target": "sup-old", "summary": "hold staging", "message": "hold staging"}),
    )
    .await
    .unwrap_err();
    assert!(
        error.contains("sup-old@soundwave ended")
            && error.contains("no live supervisor of widgets runs on soundwave")
            && error.contains("Nothing was sent"),
        "{error}"
    );
    assert!(mailbox.lock().unwrap().is_empty());
}

/// The account-wide path must preserve the same successor receipt across
/// project boundaries, and admit the message to that successor's real inbox.
#[tokio::test]
async fn cas_56c4_cross_project_ended_supervisor_reaches_successor() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, registry, mailbox) = fake_cloud_with_mailbox().await;
    let sender = TestMachine::new("prowl");
    let receiver = TestMachine::new("soundwave");
    seed_project(&sender, &server.uri(), "github.com/acme/gabber");
    seed_project(&receiver, &server.uri(), "github.com/acme/widgets");
    let (a, b) = (sender.cas_dir.clone(), receiver.cas_dir.clone());
    tokio::task::spawn_blocking(move || {
        supervisor_at(&a, "prowl", "a");
        supervisor_at(&b, "soundwave", "old");
        supervisor_at(&b, "soundwave", "new");
    }).await.unwrap();
    for row in registry.lock().unwrap().values_mut() {
        row["user_id"] = json!("fixture-owner");
    }
    registry.lock().unwrap().get_mut("old").unwrap()["status"] = json!("shutdown");
    let service = service_for(&sender.cas_dir, "a");
    for target in ["sup-old@soundwave", "supervisor:widgets"] {
        let sent = coordinate(&service, json!({"action":"message", "target":target,
            "summary":"cross-project ask", "message":target})).await.unwrap();
        if target == "sup-old@soundwave" {
            assert!(sent.contains("sup-old@soundwave ended")
                && sent.contains("delivered to its successor sup-new@soundwave"), "{sent}");
        }
    }
    let messages = mailbox.lock().unwrap().clone();
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().all(|m| m["recipient_agent_id"] == "new" && m["scope"] == "user"));
    let b = receiver.cas_dir.clone();
    let rows = tokio::task::spawn_blocking(move || { tick(&b, "new"); inbox(&b) }).await.unwrap();
    assert_eq!(rows.len(), 2);
    for target in ["sup-old@soundwave", "supervisor:widgets"] {
        assert!(rows.iter().any(|row| row.target == "sup-new" && row.prompt.ends_with(target)), "{rows:?}");
    }
}

/// A project selector also identifies its ended session when no supervisor
/// is live; the sender must learn why nothing was sent.
#[tokio::test]
async fn cas_56c4_cross_project_without_successor_names_ended_session_and_project() {
    let _env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, registry, mailbox) = fake_cloud_with_mailbox().await;
    let sender = TestMachine::new("prowl");
    let receiver = TestMachine::new("soundwave");
    seed_project(&sender, &server.uri(), "github.com/acme/gabber");
    seed_project(&receiver, &server.uri(), "github.com/acme/widgets");
    let (a, b) = (sender.cas_dir.clone(), receiver.cas_dir.clone());
    tokio::task::spawn_blocking(move || {
        supervisor_at(&a, "prowl", "a");
        supervisor_at(&b, "soundwave", "old");
    }).await.unwrap();
    for row in registry.lock().unwrap().values_mut() {
        row["user_id"] = json!("fixture-owner");
    }
    registry.lock().unwrap().get_mut("old").unwrap()["status"] = json!("shutdown");
    let service = service_for(&sender.cas_dir, "a");
    for target in ["sup-old@soundwave", "supervisor:widgets"] {
        let error = coordinate(&service, json!({"action":"message", "target":target,
            "summary":"cross-project ask", "message":"hold staging"})).await.unwrap_err();
        assert!(error.contains("sup-old@soundwave ended") && error.contains("ago")
            && error.contains("no live supervisor") && error.contains("widgets")
            && !error.contains("no supervisor or worker named"), "{target}: {error}");
    }
    assert!(mailbox.lock().unwrap().is_empty());
    assert!(inbox(&sender.cas_dir).is_empty());
}

/// GH #1319: a supervisor whose Claude login expired heartbeats like a live
/// one. Its own transcript is published as the reason, and a peer's send to
/// it is refused instead of reported delivered.
#[tokio::test]
async fn gh1319_a_logged_out_supervisor_is_refused_not_delivered() {
    let env = crate::test_env_guard::TestEnvGuard::temp_home();
    let (server, _registry, mailbox) = fake_cloud_with_mailbox().await;
    let prowl = TestMachine::new("prowl");
    let soundwave = TestMachine::new("soundwave");
    seed_project(&prowl, &server.uri(), "github.com/acme/widgets");
    seed_project(&soundwave, &server.uri(), "github.com/acme/widgets");
    let (p, s) = (prowl.cas_dir.clone(), soundwave.cas_dir.clone());
    let home = env.home().to_path_buf();
    tokio::task::spawn_blocking(move || {
        supervisor_at(&s, "soundwave", "a");
        // patient-badger-31's real record: every turn it was given ended so.
        let clone = p.parent().unwrap().to_string_lossy().into_owned();
        let slug: String = clone.chars().map(|c| if c == '/' || c == '.' { '-' } else { c }).collect();
        let dir = home.join(".claude/projects").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("session-b.jsonl"),
            r#"{"type":"assistant","timestamp":"2026-10-20T16:42:04.720Z","message":{"model":"<synthetic>","role":"assistant","content":[{"type":"text","text":"Login expired · Please run /login"}]},"error":"authentication_failed","isApiErrorMessage":true}
"#,
        )
        .unwrap();
        let mut b = Agent::new("b".to_string(), "sup-b".to_string());
        b.role = AgentRole::Supervisor;
        b.machine_id = Some("prowl".to_string());
        b.factory_session = Some("factory-b".to_string());
        b.cc_session_id = Some("session-b".to_string());
        b.metadata.insert("clone_path".to_string(), clone);
        open_agent_store(&p).unwrap().register(&b).unwrap();
        let coordinator = cas::cloud::CloudCoordinator::new(
            cas::cloud::CloudConfig::load_from_cas_dir(&p).unwrap(),
        )
        .unwrap()
        .with_canonical_id(cas::cloud::resolve_canonical_id(&p));
        cas::cloud::presence::register_now(&coordinator, &p, &b, chrono::Utc::now());
    })
    .await
    .unwrap();
    let error = coordinate(
        &service_for(&soundwave.cas_dir, "a"),
        json!({"action": "message", "target": "sup-b", "summary": "take cas-1?", "message": "can you take cas-1?"}),
    )
    .await
    .unwrap_err();
    assert!(
        error.contains("sup-b@prowl's harness is logged out")
            && error.contains("Login expired · Please run /login")
            && error.contains("Nothing was sent"),
        "{error}"
    );
    assert!(mailbox.lock().unwrap().is_empty());
}
