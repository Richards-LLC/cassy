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
    DeliveryReport, PeerMailbox, PeerSend, deliver_claimed, http_mailbox, parse_envelope,
    reply_to_peer_row, resolve_peer_target, send_to_peer,
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
            return ResponseTemplate::new(403).set_body_json(json!({"error": "recipient is not in this project"}));
        }
        let mut mailbox = self.1.lock().unwrap();
        if let Some(existing) = mailbox.iter().find(|m| m["dedupe_key"] == body["dedupe_key"]) {
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
        ResponseTemplate::new(200).set_body_json(json!({"messages": messages}))
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
        ResponseTemplate::new(200).set_body_json(json!({"status": "success"}))
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
    register_supervisor(&machine.cas_dir, id, &machine.name, "cas-571d");
    let mut agent = Agent::new(id.to_string(), format!("sup-{id}"));
    agent.role = AgentRole::Supervisor;
    agent.machine_id = Some(machine.name.clone());
    agent.factory_session = Some(format!("factory-{id}"));
    open_agent_store(&machine.cas_dir)
        .unwrap()
        .register(&agent)
        .unwrap();
}

/// One recipient-daemon tick for supervisor `id`'s session.
fn tick(cas_dir: &Path, id: &str) -> DeliveryReport {
    let canonical = cas::cloud::resolve_canonical_id(cas_dir).unwrap();
    deliver_claimed(
        &http_mailbox(cas_dir).expect("logged-in project has a mailbox"),
        open_prompt_queue_store(cas_dir).unwrap().as_ref(),
        open_agent_store(cas_dir).unwrap().as_ref(),
        &canonical,
        &[],
        &format!("factory-{id}"),
        &format!("consumer-{id}"),
    )
}

fn inbox(cas_dir: &Path) -> Vec<cas::store::QueuedPrompt> {
    let mut rows = open_prompt_queue_store(cas_dir)
        .unwrap()
        .peek_all(100)
        .unwrap();
    rows.sort_by_key(|row| row.id);
    rows
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
        let to_b = resolve_peer_target(&peers, "sup-b").unwrap().expect("B is a peer");
        let first = send_to_peer(&a, "a", to_b, "can you take cas-1234?", Some("take cas-1234?"), None).unwrap();
        assert_eq!(first.status, "queued");
        send_to_peer(&a, "a", to_b, "and cas-5678 after", Some("then cas-5678"), None).unwrap();
        let receipts = http_mailbox(&a).unwrap();
        assert_eq!(receipts.status(&first.id).unwrap().status, "queued");

        // B comes online: both arrive, in order, with sender and machine.
        let report = tick(&b, "b");
        assert_eq!((report.claimed, report.delivered), (2, 2));
        let rows = inbox(&b);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].prompt.ends_with("can you take cas-1234?"));
        assert!(rows[1].prompt.ends_with("and cas-5678 after"));
        let envelope = parse_envelope(&rows[0].prompt).unwrap();
        assert_eq!(envelope.sender_name, "sup-a");
        assert_eq!(envelope.machine, "machine-alpha");
        assert!(rows[0].prompt.contains("sup-a@machine-alpha"));
        assert_eq!(rows[0].target, "sup-b");
        assert!(!rows[0].urgent);
        assert_eq!(receipts.status(&first.id).unwrap().status, "delivered");

        // No duplicates on the next tick.
        assert_eq!(tick(&b, "b").claimed, 0);
        assert_eq!(inbox(&b).len(), 2);

        // B replies to the first message; its row is acked locally.
        let queue_b = open_prompt_queue_store(&b).unwrap();
        let reply = reply_to_peer_row(&b, queue_b.as_ref(), rows[0].id, "b", "sup-b", "yes, taking it", Some("taking cas-1234"))
            .unwrap()
            .expect("row is a peer message");
        assert_eq!(reply.to, "sup-a@machine-alpha");
        assert!(queue_b.queued_prompt(rows[0].id).unwrap().unwrap().acked_at.is_some());

        // A receives the reply, linked to its message.
        let report = tick(&a, "a");
        assert_eq!(report.delivered, 1);
        let replies = inbox(&a);
        assert_eq!(replies.len(), 1);
        assert!(replies[0].prompt.ends_with("yes, taking it"));
        assert!(replies[0].prompt.contains(&format!("in reply to {}", first.id)), "{}", replies[0].prompt);
        assert_eq!(parse_envelope(&replies[0].prompt).unwrap().sender_name, "sup-b");
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
        assert!(resolve_peer_target(&peers_of(&a, "a"), "sup-g").unwrap().is_none());
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

async fn coordinate(
    service: &cas::mcp::CasService,
    request: Value,
) -> Result<String, String> {
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
    let (server, _registry, _mailbox) = fake_cloud_with_mailbox().await;
    let endpoint = server.uri();
    let alpha = TestMachine::new("machine-alpha");
    let beta = TestMachine::new("machine-beta");
    seed_project(&alpha, &endpoint, "github.com/acme/widgets");
    seed_project(&beta, &endpoint, "github.com/acme/widgets");
    let (a, b) = (alpha.cas_dir.clone(), beta.cas_dir.clone());
    tokio::task::spawn_blocking(move || {
        supervisor(&alpha, "a");
        supervisor(&beta, "b");
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
        json!({"action": "message", "target": "sup-b@machine-beta", "summary": "take cas-1234?", "message": "can you take cas-1234?"}),
    )
    .await
    .unwrap();
    assert!(sent.contains("Message sent to peer supervisor sup-b@"), "{sent}");
    assert!(sent.contains("peer message pm_1"), "{sent}");

    let bb = b.clone();
    let rows = tokio::task::spawn_blocking(move || {
        tick(&bb, "b");
        inbox(&bb)
    })
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);

    let replied = coordinate(
        &service_b,
        json!({"action": "message", "target": "sup-a", "summary": "taking it", "message": "yes", "in_reply_to": rows[0].id}),
    )
    .await
    .unwrap();
    assert!(replied.contains("Reply sent to peer supervisor sup-a@machine-alpha"), "{replied}");

    let status = coordinate(&service_a, json!({"action": "message_status", "id": "pm_1"}))
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
}
