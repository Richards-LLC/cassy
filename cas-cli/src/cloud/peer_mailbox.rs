//! Cross-machine supervisor messaging through a Cassy Cloud peer mailbox
//! (cas-f9c7).
//!
//! A supervisor addresses a peer supervisor of the same repo (found by
//! cas-e477 peer discovery) on another machine. The sender pushes the message
//! to the cloud mailbox; the recipient's factory daemon claims it, admits it
//! once into its local prompt queue, and acks it. The cloud contract is
//! Richards-LLC/petra-stella-cloud#152.
//!
//! Authority: a peer message is a Daemon-stamped row whose first line is a
//! CAS-written [`PeerEnvelope`]. It is never urgent, never operator-authored,
//! and only ever addressed to a local supervisor, so it cannot interrupt a
//! pane, act as the operator, or spawn or direct workers (cas-604d).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::cloud::peers::Peer;
use crate::store::{AgentStore, PromptQueueStore};

/// How often the factory daemon claims peer messages.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);
/// Per-request cap for mailbox calls.
pub const MAILBOX_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Most messages claimed per tick.
pub const CLAIM_MAX: u32 = 50;
/// Lease on a claimed message before the cloud redelivers it.
pub const LEASE_SECS: u32 = 120;

/// One outbound peer message (`POST /api/peer-messages`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PeerSend {
    pub project_id: String,
    pub sender_agent_id: String,
    pub recipient_agent_id: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The cloud id of the peer message this answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    pub dedupe_key: String,
}

/// The cloud's answer to a send.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SendReceipt {
    pub id: String,
    /// `queued` or `duplicate`.
    pub status: String,
}

/// A message leased to this consumer by `POST /api/peer-messages/claim`.
/// Sender identity is stamped by the cloud from its agent registry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ClaimedPeerMessage {
    pub id: String,
    pub project_id: String,
    pub recipient_agent_id: String,
    pub sender_agent_id: String,
    pub sender_name: String,
    #[serde(default)]
    pub sender_machine_id: Option<String>,
    #[serde(default)]
    pub sender_session: Option<String>,
    pub body: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub attempts: u32,
}

/// Outcome acked for a claimed message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerAckOutcome {
    /// Admitted into the local prompt queue (or already there).
    Delivered,
    /// Not for this repo; never delivered.
    Rejected,
}

/// Delivery receipt (`GET /api/peer-messages/{id}`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PeerMessageStatus {
    pub id: String,
    /// `queued`, `leased`, `delivered`, `rejected` or `expired`.
    pub status: String,
    #[serde(default)]
    pub delivered_at: Option<String>,
    #[serde(default)]
    pub attempts: u32,
}

/// The cloud peer mailbox, abstracted so tests can drive delivery with an
/// in-memory fake.
pub trait PeerMailbox: Send + Sync {
    fn send(&self, message: &PeerSend) -> Result<SendReceipt, String>;
    fn claim(
        &self,
        project_ids: &[String],
        recipient_agent_ids: &[String],
        consumer_id: &str,
    ) -> Result<Vec<ClaimedPeerMessage>, String>;
    fn ack(&self, consumer_id: &str, acks: &[(String, PeerAckOutcome)]) -> Result<(), String>;
    fn status(&self, id: &str) -> Result<PeerMessageStatus, String>;
}

/// The CAS-written first line of a delivered peer message. Only a
/// Daemon-stamped row carrying it is a peer message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerEnvelope {
    /// Cloud peer message id.
    pub id: String,
    pub sender_name: String,
    pub sender_agent_id: String,
    pub machine: String,
    pub session: Option<String>,
    pub repo: String,
}

/// Render the envelope line.
pub fn render_envelope(envelope: &PeerEnvelope) -> String {
    let _ = envelope;
    String::new()
}

/// Parse the envelope from the first line of `prompt`, and only there.
pub fn parse_envelope(prompt: &str) -> Option<PeerEnvelope> {
    let _ = prompt;
    None
}

/// The local prompt for a claimed message: envelope, a readable header and
/// the body.
pub fn render_prompt(message: &ClaimedPeerMessage) -> String {
    let _ = message;
    String::new()
}

/// Resolve `target` (`name` or `name@machine`) to one peer.
///
/// `Ok(None)` when nothing matches; `Err` when the name is ambiguous.
pub fn resolve_peer_target<'a>(peers: &'a [Peer], target: &str) -> Result<Option<&'a Peer>, String> {
    let _ = (peers, target);
    Ok(None)
}

/// What one delivery tick did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeliveryReport {
    pub claimed: usize,
    pub delivered: usize,
    pub duplicates: usize,
    pub rejected: usize,
    /// Local prompt ids of newly admitted rows, in delivery order.
    pub admitted: Vec<i64>,
    pub errors: Vec<String>,
}

/// Claim this session's peer messages and admit each once.
///
/// Recipients are the live local supervisors of `factory_session`; workers
/// are never recipients. A message for another repo is acked `rejected`.
/// Every admitted (or already admitted) message is acked `delivered`; a
/// message that failed to admit is left unacked for redelivery.
#[allow(clippy::too_many_arguments)]
pub fn deliver_claimed(
    mailbox: &dyn PeerMailbox,
    queue: &dyn PromptQueueStore,
    agents: &dyn AgentStore,
    canonical_id: &str,
    alias_class: &[String],
    factory_session: &str,
    consumer_id: &str,
) -> DeliveryReport {
    let _ = (
        mailbox,
        queue,
        agents,
        canonical_id,
        alias_class,
        factory_session,
        consumer_id,
    );
    DeliveryReport::default()
}

/// Send `body` from local supervisor `sender_agent_id` to `peer`, through
/// the project's cloud mailbox. `in_reply_to` is a cloud peer message id.
pub fn send_to_peer(
    cas_root: &Path,
    sender_agent_id: &str,
    peer: &Peer,
    body: &str,
    summary: Option<&str>,
    in_reply_to: Option<&str>,
) -> Result<SendReceipt, String> {
    let _ = (cas_root, sender_agent_id, peer, body, summary, in_reply_to);
    Err("not implemented".into())
}

/// A reply to a delivered peer message, addressed back to its sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerReply {
    pub receipt: SendReceipt,
    /// The peer the reply went to, as `name@machine`.
    pub to: String,
}

/// Reply to local peer-message row `row_id` from local supervisor
/// `sender_agent_id` / `sender_name`. The row must be a Daemon-stamped peer
/// envelope addressed to that supervisor; the reply goes to the original
/// sender with `in_reply_to` = the original cloud id, and the row is acked.
pub fn reply_to_peer_row(
    cas_root: &Path,
    queue: &dyn PromptQueueStore,
    row_id: i64,
    sender_agent_id: &str,
    sender_name: &str,
    body: &str,
    summary: Option<&str>,
) -> Result<Option<PeerReply>, String> {
    let _ = (cas_root, queue, row_id, sender_agent_id, sender_name, body, summary);
    Ok(None)
}

/// The HTTP mailbox for a project, from its own cloud config.
pub fn http_mailbox(cas_root: &Path) -> Option<HttpPeerMailbox> {
    let _ = cas_root;
    None
}

/// `PeerMailbox` over Cassy Cloud HTTP.
#[derive(Debug, Clone)]
pub struct HttpPeerMailbox {
    endpoint: String,
    token: String,
}

impl HttpPeerMailbox {
    pub fn new(endpoint: &str, token: &str) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            token: token.to_string(),
        }
    }
}

impl PeerMailbox for HttpPeerMailbox {
    fn send(&self, message: &PeerSend) -> Result<SendReceipt, String> {
        let _ = message;
        Err("not implemented".into())
    }
    fn claim(
        &self,
        project_ids: &[String],
        recipient_agent_ids: &[String],
        consumer_id: &str,
    ) -> Result<Vec<ClaimedPeerMessage>, String> {
        let _ = (project_ids, recipient_agent_ids, consumer_id);
        Err("not implemented".into())
    }
    fn ack(&self, consumer_id: &str, acks: &[(String, PeerAckOutcome)]) -> Result<(), String> {
        let _ = (consumer_id, acks);
        Err("not implemented".into())
    }
    fn status(&self, id: &str) -> Result<PeerMessageStatus, String> {
        let _ = id;
        Err("not implemented".into())
    }
}

#[cfg(test)]
#[path = "peer_mailbox_tests.rs"]
mod tests;
