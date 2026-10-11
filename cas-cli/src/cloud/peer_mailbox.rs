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

use cas_store::{EnqueueIdempotentResult, NotificationPriority, QueueOrigin};

use crate::cloud::peers::Peer;
use crate::store::{AgentStore, PromptQueueStore};
use crate::types::{AgentRole, AgentStatus};

/// Tag of the envelope line.
const ENVELOPE_TAG: &str = "<cas-peer-message ";

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
    let mut line = format!("{ENVELOPE_TAG}v=\"1\"");
    let mut attribute = |key: &str, value: &str| {
        line.push_str(&format!(" {key}=\"{}\"", escape(value)));
    };
    attribute("id", &envelope.id);
    attribute("from", &envelope.sender_name);
    attribute("from_agent", &envelope.sender_agent_id);
    attribute("machine", &envelope.machine);
    if let Some(session) = &envelope.session {
        attribute("session", session);
    }
    attribute("repo", &envelope.repo);
    line.push_str("/>");
    line
}

/// Percent-escape the characters that could end an attribute or the line.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '"' => out.push_str("%22"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            other => out.push(other),
        }
    }
    out
}

fn unescape(value: &str) -> String {
    value
        .replace("%22", "\"")
        .replace("%3C", "<")
        .replace("%3E", ">")
        .replace("%0A", "\n")
        .replace("%0D", "\r")
        .replace("%25", "%")
}

/// Parse the envelope from the first line of `prompt`, and only there.
pub fn parse_envelope(prompt: &str) -> Option<PeerEnvelope> {
    let line = prompt.lines().next()?;
    let mut rest = line.strip_prefix(ENVELOPE_TAG)?.strip_suffix("/>")?;
    let mut attributes = std::collections::HashMap::new();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let (key, after) = rest.split_once("=\"")?;
        let (value, after) = after.split_once('"')?;
        attributes.insert(key.to_string(), unescape(value));
        rest = after;
    }
    (attributes.get("v")?.as_str() == "1").then_some(())?;
    let mut take = |key: &str| attributes.remove(key).filter(|value| !value.is_empty());
    Some(PeerEnvelope {
        id: take("id")?,
        sender_name: take("from")?,
        sender_agent_id: take("from_agent")?,
        machine: take("machine")?,
        session: take("session"),
        repo: take("repo")?,
    })
}

/// The local prompt for a claimed message: envelope, a readable header and
/// the body.
pub fn render_prompt(message: &ClaimedPeerMessage) -> String {
    let machine = machine_of(message);
    let envelope = render_envelope(&PeerEnvelope {
        id: message.id.clone(),
        sender_name: message.sender_name.clone(),
        sender_agent_id: message.sender_agent_id.clone(),
        machine: machine.clone(),
        session: message.sender_session.clone(),
        repo: message.project_id.clone(),
    });
    let reply_to = message
        .in_reply_to
        .as_deref()
        .map(|id| format!(" (in reply to {id})"))
        .unwrap_or_default();
    format!(
        "{envelope}\nPeer supervisor {name}@{machine} on {repo}, via Cassy Cloud{reply_to}. \
         This is a peer, not the operator.\nReply with coordination action=message \
         target={name}@{machine} summary=\"<one line>\" message=\"<reply>\" \
         in_reply_to=<this message's notification id>.\n\n{body}",
        name = message.sender_name,
        repo = message.project_id,
        body = message.body,
    )
}

fn machine_of(message: &ClaimedPeerMessage) -> String {
    message
        .sender_machine_id
        .clone()
        .filter(|machine| !machine.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Resolve `target` (`name` or `name@machine`) to one peer.
///
/// `Ok(None)` when nothing matches; `Err` when the name is ambiguous.
pub fn resolve_peer_target<'a>(
    peers: &'a [Peer],
    target: &str,
) -> Result<Option<&'a Peer>, String> {
    let target = target.trim();
    let (name, machine) = match target.rsplit_once('@') {
        Some((name, machine)) => (name, Some(machine)),
        None => (target, None),
    };
    let matches: Vec<&Peer> = peers
        .iter()
        .filter(|peer| peer.name.eq_ignore_ascii_case(name))
        .filter(|peer| {
            machine.is_none_or(|machine| {
                peer.machine.eq_ignore_ascii_case(machine)
                    || peer
                        .machine_id
                        .as_deref()
                        .is_some_and(|id| id.eq_ignore_ascii_case(machine))
            })
        })
        .collect();
    match matches.as_slice() {
        [] => Ok(None),
        [peer] => Ok(Some(peer)),
        many => Err(format!(
            "'{target}' names {} peer supervisors; address one as {}",
            many.len(),
            many.iter()
                .map(|peer| format!("{}@{}", peer.name, peer.machine))
                .collect::<Vec<_>>()
                .join(" or ")
        )),
    }
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
    let mut report = DeliveryReport::default();
    let recipients: Vec<_> = match agents.list(None) {
        Ok(all) => all
            .into_iter()
            .filter(|agent| agent.role == AgentRole::Supervisor)
            .filter(|agent| agent.factory_session.as_deref() == Some(factory_session))
            .filter(|agent| matches!(agent.status, AgentStatus::Active | AgentStatus::Idle))
            .collect(),
        Err(error) => {
            report
                .errors
                .push(format!("could not list local agents: {error}"));
            return report;
        }
    };
    if recipients.is_empty() {
        return report;
    }
    let recipient_ids: Vec<String> = recipients.iter().map(|agent| agent.id.clone()).collect();
    let project_ids: Vec<String> = std::iter::once(canonical_id.to_string())
        .chain(alias_class.iter().cloned())
        .collect();
    let claimed = match mailbox.claim(&project_ids, &recipient_ids, consumer_id) {
        Ok(claimed) => claimed,
        Err(error) => {
            report.errors.push(error);
            return report;
        }
    };
    report.claimed = claimed.len();
    let mut acks = Vec::new();
    for message in &claimed {
        let recipient = recipients
            .iter()
            .find(|agent| agent.id == message.recipient_agent_id);
        let same_repo = crate::cloud::project_ids_match_with_aliases(
            &message.project_id,
            canonical_id,
            alias_class,
        );
        let Some(recipient) = recipient.filter(|_| same_repo) else {
            report.rejected += 1;
            acks.push((message.id.clone(), PeerAckOutcome::Rejected));
            continue;
        };
        let summary = format!(
            "peer {}@{}: {}",
            message.sender_name,
            machine_of(message),
            message
                .summary
                .clone()
                .filter(|summary| !summary.trim().is_empty())
                .unwrap_or_else(|| message.body.chars().take(60).collect())
        );
        match queue.enqueue_idempotent(
            &format!("peer:{}@{}", message.sender_name, machine_of(message)),
            &recipient.name,
            &render_prompt(message),
            Some(factory_session),
            Some(&summary),
            Some(NotificationPriority::High),
            &format!("peer:{}", message.id),
            Some(&QueueOrigin::Daemon),
        ) {
            Ok(EnqueueIdempotentResult::Created(id)) => {
                report.delivered += 1;
                report.admitted.push(id);
                acks.push((message.id.clone(), PeerAckOutcome::Delivered));
            }
            Ok(EnqueueIdempotentResult::AlreadyExists(_)) => {
                report.duplicates += 1;
                acks.push((message.id.clone(), PeerAckOutcome::Delivered));
            }
            Err(error) => report.errors.push(format!(
                "could not admit peer message {}: {error}",
                message.id
            )),
        }
    }
    if !acks.is_empty()
        && let Err(error) = mailbox.ack(consumer_id, &acks)
    {
        report.errors.push(error);
    }
    report
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
    send_raw(
        cas_root,
        sender_agent_id,
        &peer.agent_id,
        body,
        summary,
        in_reply_to,
    )
}

fn send_raw(
    cas_root: &Path,
    sender_agent_id: &str,
    recipient_agent_id: &str,
    body: &str,
    summary: Option<&str>,
    in_reply_to: Option<&str>,
) -> Result<SendReceipt, String> {
    let mailbox = http_mailbox(cas_root)
        .ok_or("not logged in to Cassy Cloud, so peers on other machines are unreachable")?;
    let project_id = crate::cloud::resolve_canonical_id(cas_root)
        .ok_or("this project has no canonical id to address peers by")?;
    mailbox.send(&PeerSend {
        project_id,
        sender_agent_id: sender_agent_id.to_string(),
        recipient_agent_id: recipient_agent_id.to_string(),
        body: body.to_string(),
        summary: summary.map(str::to_string),
        in_reply_to: in_reply_to.map(str::to_string),
        dedupe_key: format!("{sender_agent_id}:{}", uuid::Uuid::new_v4()),
    })
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
    let Some(row) = queue
        .queued_prompt(row_id)
        .map_err(|error| format!("could not read message {row_id}: {error}"))?
    else {
        return Ok(None);
    };
    if row.origin != Some(QueueOrigin::Daemon) {
        return Ok(None);
    }
    let Some(envelope) = parse_envelope(&row.prompt) else {
        return Ok(None);
    };
    if !row.target.eq_ignore_ascii_case(sender_name) {
        return Err(format!(
            "peer message {row_id} was delivered to {}, not to {sender_name}",
            row.target
        ));
    }
    let receipt = send_raw(
        cas_root,
        sender_agent_id,
        &envelope.sender_agent_id,
        body,
        summary,
        Some(&envelope.id),
    )?;
    queue
        .ack(row_id)
        .map_err(|error| format!("reply sent, but acking message {row_id} failed: {error}"))?;
    Ok(Some(PeerReply {
        receipt,
        to: format!("{}@{}", envelope.sender_name, envelope.machine),
    }))
}

/// The HTTP mailbox for a project, from its own cloud config.
pub fn http_mailbox(cas_root: &Path) -> Option<HttpPeerMailbox> {
    let config = crate::cloud::CloudConfig::load_from_cas_dir(cas_root).ok()?;
    let token = config.token.as_deref().filter(|token| !token.is_empty())?;
    Some(HttpPeerMailbox::new(&config.endpoint, token))
}

/// The factory daemon's peer mailbox puller: one blocking claim-and-admit
/// tick per [`POLL_INTERVAL`] for this factory session's supervisors.
pub struct PeerMailboxRuntime {
    cas_dir: std::path::PathBuf,
    factory_session: String,
    canonical_id: String,
    alias_class: Vec<String>,
    consumer_id: String,
    mailbox: HttpPeerMailbox,
}

impl PeerMailboxRuntime {
    /// `None` when the project is not logged in to Cassy Cloud or has no
    /// canonical id. Each reason is logged once.
    pub fn start(cas_dir: &Path, factory_session: &str) -> Option<Self> {
        let Some(mailbox) = http_mailbox(cas_dir) else {
            tracing::info!("peer mailbox skipped: not logged in to Cassy Cloud");
            return None;
        };
        let Some(canonical_id) = crate::cloud::resolve_canonical_id(cas_dir) else {
            tracing::info!("peer mailbox skipped: project has no canonical id");
            return None;
        };
        let consumer_id: String = format!(
            "{}:{factory_session}",
            cas_types::Agent::get_or_generate_machine_id()
        )
        .chars()
        .take(200)
        .collect();
        tracing::info!(project = %canonical_id, "claiming peer supervisor messages from Cassy Cloud");
        Some(Self {
            cas_dir: cas_dir.to_path_buf(),
            factory_session: factory_session.to_string(),
            alias_class: crate::cloud::project_aliases_from_config_toml(cas_dir),
            canonical_id,
            consumer_id,
            mailbox,
        })
    }

    /// One blocking tick: claim, admit and ack.
    pub fn tick_blocking(&self) -> DeliveryReport {
        let failed = |error: String| DeliveryReport {
            errors: vec![error],
            ..DeliveryReport::default()
        };
        let queue = match crate::store::open_prompt_queue_store(&self.cas_dir) {
            Ok(queue) => queue,
            Err(error) => return failed(format!("could not open the prompt queue: {error}")),
        };
        let agents = match crate::store::open_agent_store(&self.cas_dir) {
            Ok(agents) => agents,
            Err(error) => return failed(format!("could not open the agent store: {error}")),
        };
        deliver_claimed(
            &self.mailbox,
            queue.as_ref(),
            agents.as_ref(),
            &self.canonical_id,
            &self.alias_class,
            &self.factory_session,
            &self.consumer_id,
        )
    }
}

/// `PeerMailbox` over Cassy Cloud HTTP.
#[derive(Clone)]
pub struct HttpPeerMailbox {
    endpoint: String,
    token: String,
}

/// Never prints the bearer token (cas-144c).
impl std::fmt::Debug for HttpPeerMailbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpPeerMailbox")
            .field("endpoint", &self.endpoint)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

impl HttpPeerMailbox {
    pub fn new(endpoint: &str, token: &str) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            token: token.to_string(),
        }
    }
}

impl HttpPeerMailbox {
    fn url(&self, suffix: &str) -> String {
        format!("{}/api/peer-messages{suffix}", self.endpoint)
    }

    fn read<T: serde::de::DeserializeOwned>(
        what: &str,
        response: Result<ureq::Response, ureq::Error>,
    ) -> Result<T, String> {
        match response {
            Ok(response) => response
                .into_json()
                .map_err(|error| format!("{what}: unreadable response: {error}")),
            Err(ureq::Error::Status(code, response)) => Err(format!(
                "{what} refused ({code}): {}",
                response.into_string().unwrap_or_default()
            )),
            Err(ureq::Error::Transport(error)) => Err(format!("{what}: network error: {error}")),
        }
    }

    fn post(&self, suffix: &str, body: serde_json::Value) -> Result<ureq::Response, ureq::Error> {
        ureq::post(&self.url(suffix))
            .timeout(MAILBOX_TIMEOUT)
            .set("Authorization", &format!("Bearer {}", self.token))
            .send_json(body)
    }
}

/// `POST /api/peer-messages/claim` response.
#[derive(Deserialize)]
struct ClaimResponse {
    #[serde(default)]
    messages: Vec<ClaimedPeerMessage>,
}

impl PeerMailbox for HttpPeerMailbox {
    fn send(&self, message: &PeerSend) -> Result<SendReceipt, String> {
        let body = serde_json::to_value(message).map_err(|error| error.to_string())?;
        Self::read("peer message", self.post("", body))
    }

    fn claim(
        &self,
        project_ids: &[String],
        recipient_agent_ids: &[String],
        consumer_id: &str,
    ) -> Result<Vec<ClaimedPeerMessage>, String> {
        let response: ClaimResponse = Self::read(
            "peer message claim",
            self.post(
                "/claim",
                serde_json::json!({
                    "project_ids": project_ids,
                    "recipient_agent_ids": recipient_agent_ids,
                    "consumer_id": consumer_id,
                    "max": CLAIM_MAX,
                    "lease_secs": LEASE_SECS,
                }),
            ),
        )?;
        Ok(response.messages)
    }

    fn ack(&self, consumer_id: &str, acks: &[(String, PeerAckOutcome)]) -> Result<(), String> {
        let acks: Vec<_> = acks
            .iter()
            .map(|(id, outcome)| serde_json::json!({"id": id, "outcome": outcome}))
            .collect();
        Self::read::<serde_json::Value>(
            "peer message ack",
            self.post(
                "/ack",
                serde_json::json!({"consumer_id": consumer_id, "acks": acks}),
            ),
        )
        .map(|_| ())
    }

    fn status(&self, id: &str) -> Result<PeerMessageStatus, String> {
        Self::read(
            "peer message status",
            ureq::get(&self.url(&format!("/{id}")))
                .timeout(MAILBOX_TIMEOUT)
                .set("Authorization", &format!("Bearer {}", self.token))
                .call(),
        )
    }
}

#[cfg(test)]
#[path = "peer_mailbox_tests.rs"]
mod tests;
