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
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use cas_store::{EnqueueIdempotentResult, NotificationPriority, QueueOrigin};

use crate::cloud::peers::Peer;
use crate::store::{AgentStore, PromptQueueStore};
use crate::types::{AgentRole, AgentStatus};

mod outbox;
mod runtime;
pub use outbox::{OUTBOX_TTL_SECS, PeerSendError, outbox_status_text, retry_peer_send};
pub use runtime::{PeerMailboxPoller, PeerMailboxRuntime, mailbox_status_section};

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
/// Cloud messages expire after seven days; older retirement identities cannot
/// recover a message sent during that restart window.
const RETIREMENT_LOOKBACK_DAYS: i64 = 7;
/// Keep current recipients independent of bounded historical recovery work.
const RETIRED_RECIPIENT_MAX: usize = 100;
const RETIREMENT_CAP_WARNING: &str =
    "retired peer recipients capped at 100; older addresses were not checked";

/// One outbound peer message (`POST /api/peer-messages`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The sender's machine label, resolved from its registry row
    /// (`metadata.hostname`) when the message is claimed (cas-c7c7). Shown
    /// and addressed in place of `sender_machine_id`, the stable identity,
    /// which stays the fallback for a sender the registry does not list.
    #[serde(default)]
    pub sender_label: Option<String>,
}

impl ClaimedPeerMessage {
    /// The sender's machine as people see and type it.
    pub fn sender_machine(&self) -> String {
        self.sender_label
            .clone()
            .or_else(|| self.sender_machine_id.clone())
            .filter(|machine| !machine.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    }
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
    /// Claim warnings must remain visible even if some projects succeeded.
    fn claim_report(
        &self,
        project_ids: &[String],
        recipient_agent_ids: &[String],
        consumer_id: &str,
    ) -> Result<PeerClaim, String> {
        self.claim(project_ids, recipient_agent_ids, consumer_id)
            .map(|messages| PeerClaim {
                messages,
                denied: Vec::new(),
                errors: Vec::new(),
            })
    }
    fn ack(&self, consumer_id: &str, acks: &[(String, PeerAckOutcome)]) -> Result<(), String>;
    fn status(&self, id: &str) -> Result<PeerMessageStatus, String>;
    /// The machine labels (`metadata.hostname`) of `sender_ids`, from the
    /// agent registry of `project_id` (cas-c7c7). Senders it cannot find are
    /// left out, so their messages fall back to `sender_machine_id`.
    fn sender_labels(
        &self,
        _project_id: &str,
        _sender_ids: &[String],
    ) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::new()
    }
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
    pub in_reply_to: Option<String>,
    pub kind: Option<PeerMessageKind>,
}

/// A sender-selected action, independent of Cloud's authenticated identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerMessageKind {
    Ask,
    Blocker,
}

impl PeerMessageKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Blocker => "blocker",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "ask" => Some(Self::Ask),
            "blocker" => Some(Self::Blocker),
            _ => None,
        }
    }
}

impl PeerEnvelope {
    pub fn wakes_idle(&self) -> bool {
        self.in_reply_to
            .as_deref()
            .is_some_and(|id| !id.trim().is_empty())
            || self.kind.is_some()
    }
}

/// Carry action intent through the existing Cloud body contract. This is
/// routing intent, never authority: only an admitted, daemon-stamped peer
/// envelope may use it to wake a supervisor.
pub fn attach_message_intent(body: &str, kind: Option<&str>, blocker: bool) -> String {
    let kind = if blocker {
        Some(PeerMessageKind::Blocker)
    } else {
        kind.and_then(PeerMessageKind::parse)
    };
    match kind {
        Some(kind) => format!("<cas-peer-intent kind=\"{}\"/>\n{body}", kind.as_str()),
        None => body.to_string(),
    }
}

fn split_message_intent(body: &str) -> (Option<PeerMessageKind>, &str) {
    let Some((line, rest)) = body.split_once('\n') else {
        return (None, body);
    };
    let kind = line
        .strip_prefix("<cas-peer-intent kind=\"")
        .and_then(|line| line.strip_suffix("\"/>"))
        .and_then(PeerMessageKind::parse);
    if kind.is_some() {
        (kind, rest)
    } else {
        (None, body)
    }
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
    if let Some(reply) = &envelope.in_reply_to {
        attribute("in_reply_to", reply);
    }
    if let Some(kind) = envelope.kind {
        attribute("kind", kind.as_str());
    }
    line.push_str("/>");
    line
}

/// Percent-escape the characters that could end an attribute or the line.
pub(crate) fn escape(value: &str) -> String {
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

pub(crate) fn unescape(value: &str) -> String {
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
        in_reply_to: take("in_reply_to"),
        kind: take("kind").as_deref().and_then(PeerMessageKind::parse),
    })
}

/// The local prompt for a claimed message: envelope, a readable header and
/// the body.
pub fn render_prompt(message: &ClaimedPeerMessage) -> String {
    let machine = machine_of(message);
    let (kind, body) = split_message_intent(&message.body);
    // Existing first-line split proposals are typed asks too. Preserve their
    // protocol body for epic_split::ingest instead of wrapping it in a new tag.
    let kind = kind.or_else(|| {
        crate::cloud::epic_split::SplitMessage::parse(body)
            .filter(|split| split.kind == crate::cloud::epic_split::SplitKind::Proposal)
            .map(|_| PeerMessageKind::Ask)
    });
    let envelope = render_envelope(&PeerEnvelope {
        id: message.id.clone(),
        sender_name: message.sender_name.clone(),
        sender_agent_id: message.sender_agent_id.clone(),
        machine: machine.clone(),
        session: message.sender_session.clone(),
        repo: message.project_id.clone(),
        in_reply_to: message.in_reply_to.clone(),
        kind,
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
        body = body,
    )
}

fn machine_of(message: &ClaimedPeerMessage) -> String {
    message.sender_machine()
}

/// Resolve `target` (`name` or `name@machine`) to one peer.
///
/// `Ok(None)` when nothing matches; `Err` when the name is ambiguous.
pub fn resolve_peer_target<'a>(
    peers: &'a [Peer],
    target: &str,
) -> Result<Option<&'a Peer>, String> {
    let target = target.trim();
    if let Some(peer) = peers
        .iter()
        .find(|peer| peer.agent_id.eq_ignore_ascii_case(target))
    {
        return Ok(Some(peer));
    }
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
    let mut addresses: BTreeMap<_, Vec<&Peer>> = BTreeMap::new();
    for peer in matches {
        let machine = peer
            .machine_id
            .as_deref()
            .unwrap_or(&peer.machine)
            .to_ascii_lowercase();
        let project = peer
            .canonical_id
            .as_deref()
            .and_then(crate::cloud::normalize_project_canonical_id);
        addresses
            .entry((
                peer.name.to_ascii_lowercase(),
                machine,
                project,
                peer.session.as_deref(),
            ))
            .or_default()
            .push(peer);
    }
    let mut matches = Vec::new();
    for group in addresses.values() {
        let live: Vec<_> = group.iter().copied().filter(|peer| peer.live).collect();
        if live.is_empty() {
            // During downtime, queue to the most recently seen identity. Its
            // successor claims this address when it returns.
            if let Some(peer) = group
                .iter()
                .max_by_key(|peer| (peer.last_heartbeat, &peer.agent_id))
            {
                matches.push(*peer);
            }
        } else {
            matches.extend(live);
        }
    }
    matches.sort_by_key(|peer| &peer.agent_id);
    matches.dedup_by_key(|peer| &peer.agent_id);
    match matches.as_slice() {
        [] => Ok(None),
        [peer] => Ok(Some(peer)),
        many => Err(format!(
            "'{target}' names {} peer supervisors; address one as {}",
            many.len(),
            many.iter()
                .map(|peer| format!(
                    "{} ({}@{}, session {})",
                    peer.agent_id,
                    peer.name,
                    peer.machine,
                    peer.session.as_deref().unwrap_or("unknown")
                ))
                .collect::<Vec<_>>()
                .join(" or ")
        )),
    }
}

/// Local retirement identity is not a remote roster entry. It only allows an
/// authenticated mailbox claim for a predecessor of this project's supervisor.
#[derive(Clone, Deserialize)]
struct RetiredRecipient {
    #[serde(default)]
    id: String,
    name: String,
    role: AgentRole,
    machine_id: Option<String>,
    factory_session: Option<String>,
    registered_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip, default = "chrono::Utc::now")]
    retired_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone)]
struct UnverifiedRetirement {
    id: String,
    name: String,
    retired_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Default)]
struct RecipientHistory {
    verified: Vec<RetiredRecipient>,
    unverified: Vec<UnverifiedRetirement>,
    warning: Option<String>,
}

fn retired_recipients(cas_root: &Path, session: &str) -> Result<RecipientHistory, String> {
    let events = cas_store::SqliteEventStore::open(cas_root)
        .map_err(|e| e.to_string())?
        .list_retired_supervisors_since(
            chrono::Utc::now() - chrono::Duration::days(RETIREMENT_LOOKBACK_DAYS),
            session,
            RETIRED_RECIPIENT_MAX + 1,
        )
        .map_err(|e| e.to_string())?;
    let mut history = RecipientHistory::default();
    if events.len() > RETIRED_RECIPIENT_MAX {
        history.warning = Some(RETIREMENT_CAP_WARNING.into());
    }
    for event in events {
        if event.entity_type != crate::types::EventEntityType::Agent {
            continue;
        }
        let identity = event
            .metadata
            .as_ref()
            .and_then(|v| v.get("retired_agent"))
            .and_then(|v| serde_json::from_value::<RetiredRecipient>(v.clone()).ok());
        if let Some(mut identity) = identity {
            identity.id = event.entity_id;
            identity.retired_at = event.created_at;
            history.verified.push(identity);
        } else if let Some(name) = event
            .summary
            .strip_prefix("Agent unregistered: ")
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            // This proves only that a local ID was retired, not its role or
            // machine. It authorizes inspection/feedback, never body admission.
            history.unverified.push(UnverifiedRetirement {
                id: event.entity_id,
                name: name.into(),
                retired_at: event.created_at,
            });
        }
    }
    Ok(history)
}

fn send_unverified_address_notice(
    mailbox: &dyn PeerMailbox,
    author: &crate::types::Agent,
    message: &ClaimedPeerMessage,
) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let machine = author.machine_id.as_deref().unwrap_or("unknown");
    let body = format!(
        "PEER DELIVERY NEEDS RESEND: message {} targeted retired ID {}, whose role and machine cannot be verified. Its body was not admitted. If you intended supervisor {}@{}, resend the original message to current supervisor ID {} using coordination action=message target={} summary=\"resend\" message=\"<original message>\".",
        message.id, message.recipient_agent_id, author.name, machine, author.id, author.id
    );
    let dedupe_key = format!(
        "{:x}",
        Sha256::digest(format!("unverified-recipient:{}", message.id))
    );
    mailbox
        .send(&PeerSend {
            project_id: message.project_id.clone(),
            sender_agent_id: author.id.clone(),
            recipient_agent_id: message.sender_agent_id.clone(),
            body,
            summary: Some("Retired peer address: resend to current supervisor ID".into()),
            in_reply_to: Some(message.id.clone()),
            dedupe_key,
        })
        .map(|_| ())
        .map_err(|error| {
            format!(
                "cannot notify sender to resend message {} to {}: {error}",
                message.id, author.id
            )
        })
}

fn live_supervisor(agent: &crate::types::Agent) -> bool {
    agent.role == AgentRole::Supervisor
        && matches!(agent.status, AgentStatus::Active | AgentStatus::Idle)
        && crate::mcp::tools::service::agent_liveness::evaluate_supervision_liveness_with(
            agent,
            crate::mcp::tools::service::agent_liveness::agent_process_is_alive(agent),
            crate::cloud::peers::PEER_LIVE_SECS,
        )
        .is_live()
}

fn recipient_routes<'a>(
    all: &'a [crate::types::Agent],
    live: &[&'a crate::types::Agent],
    retired: &[RetiredRecipient],
    session: &str,
) -> (BTreeMap<String, &'a crate::types::Agent>, Vec<String>) {
    let live_ids: BTreeSet<_> = live.iter().map(|a| a.id.as_str()).collect();
    let mut routes: BTreeMap<_, _> = live
        .iter()
        .copied()
        .filter(|a| a.factory_session.as_deref() == Some(session))
        .map(|a| (a.id.clone(), a))
        .collect();
    let mut warnings = BTreeSet::new();
    let rows = all
        .iter()
        .filter(|a| !live_ids.contains(a.id.as_str()))
        .map(|a| RetiredRecipient {
            id: a.id.clone(),
            name: a.name.clone(),
            role: a.role,
            machine_id: a.machine_id.clone(),
            factory_session: a.factory_session.clone(),
            registered_at: a.registered_at,
            retired_at: a.last_heartbeat,
        });
    let mut old_rows: Vec<_> = rows
        .chain(
            retired
                .iter()
                .filter(|old| !all.iter().any(|a| a.id == old.id))
                .cloned(),
        )
        .collect();
    old_rows.sort_by(|a, b| {
        b.retired_at
            .cmp(&a.retired_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let now = chrono::Utc::now();
    let since = now - chrono::Duration::days(RETIREMENT_LOOKBACK_DAYS);
    let mut retired_count = 0;
    let mut seen = BTreeSet::new();
    for old in old_rows {
        if !seen.insert(old.id.clone())
            || old.role != AgentRole::Supervisor
            || old.name.trim().is_empty()
            || session.trim().is_empty()
            || old.factory_session.as_deref() != Some(session)
            || old.retired_at < since
            || old.retired_at > now
        {
            continue;
        }
        let Some(machine) = old.machine_id.as_deref().filter(|s| !s.trim().is_empty()) else {
            continue;
        };
        let successors: Vec<_> = live
            .iter()
            .copied()
            .filter(|a| {
                a.name.eq_ignore_ascii_case(&old.name)
                    && a.machine_id.as_deref() == Some(machine)
                    && a.factory_session.as_deref() == old.factory_session.as_deref()
                    && a.registered_at >= old.registered_at
            })
            .collect();
        match successors.as_slice() {
            [successor] => {
                if retired_count < RETIRED_RECIPIENT_MAX {
                    routes.insert(old.id, *successor);
                    retired_count += 1;
                } else {
                    warnings.insert(RETIREMENT_CAP_WARNING.into());
                }
            }
            many if many.len() > 1 => {
                warnings.insert(format!(
                    "ambiguous live successors for {}@{} in factory {}: {}",
                    old.name,
                    machine,
                    session,
                    many.iter()
                        .map(|a| a.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            _ => {}
        }
    }
    (routes, warnings.into_iter().collect())
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
    /// Bounded historical recovery can warn without disabling current mail.
    pub warnings: Vec<String>,
    /// Server-directed delay that outlives this tick's bounded HTTP retries.
    pub retry_after: Option<Duration>,
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
    deliver_claimed_with(
        mailbox,
        queue,
        agents,
        canonical_id,
        alias_class,
        factory_session,
        consumer_id,
        &|_, _| Ok(()),
    )
}

/// [`deliver_claimed`], running `on_admitted` for each message once it is in
/// the local queue (newly or already) and before it is acked. An error leaves
/// the message unacked, so the cloud redelivers it and `on_admitted` runs
/// again; it must be idempotent. The factory daemon records epic splits here
/// (cas-4736).
#[allow(clippy::too_many_arguments)]
pub fn deliver_claimed_with(
    mailbox: &dyn PeerMailbox,
    queue: &dyn PromptQueueStore,
    agents: &dyn AgentStore,
    canonical_id: &str,
    alias_class: &[String],
    factory_session: &str,
    consumer_id: &str,
    on_admitted: &dyn Fn(&crate::types::Agent, &ClaimedPeerMessage) -> Result<(), String>,
) -> DeliveryReport {
    deliver_claimed_with_history(
        mailbox,
        queue,
        agents,
        canonical_id,
        alias_class,
        factory_session,
        consumer_id,
        &RecipientHistory::default(),
        on_admitted,
    )
}

#[allow(clippy::too_many_arguments)]
fn deliver_claimed_with_history(
    mailbox: &dyn PeerMailbox,
    queue: &dyn PromptQueueStore,
    agents: &dyn AgentStore,
    canonical_id: &str,
    alias_class: &[String],
    factory_session: &str,
    consumer_id: &str,
    history: &RecipientHistory,
    on_admitted: &dyn Fn(&crate::types::Agent, &ClaimedPeerMessage) -> Result<(), String>,
) -> DeliveryReport {
    let mut report = DeliveryReport::default();
    let all = match agents.list(None) {
        Ok(all) => all,
        Err(error) => {
            report
                .errors
                .push(format!("could not list local agents: {error}"));
            return report;
        }
    };
    // Evaluate process/heartbeat liveness once per registry row, never per
    // historical ID. Both verified routing and legacy feedback reuse this set.
    let live: Vec<_> = all.iter().filter(|a| live_supervisor(a)).collect();
    if !live
        .iter()
        .any(|a| a.factory_session.as_deref() == Some(factory_session))
    {
        report
            .errors
            .push("no live supervisor in this factory session".into());
        return report;
    }
    let (routes, warnings) = recipient_routes(&all, &live, &history.verified, factory_session);
    report.warnings.extend(warnings);
    if let Some(warning) = &history.warning {
        report.warnings.push(warning.clone());
    }
    let current_count = live
        .iter()
        .filter(|a| a.factory_session.as_deref() == Some(factory_session))
        .count();
    let retired_count = routes.len().saturating_sub(current_count);
    // Untyped retirement evidence can authorize feedback only when its name
    // belongs to exactly one live named factory in this project. It cannot
    // establish a lineage between competing factories.
    let mut live_factories: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    for agent in &live {
        if let Some(session) = agent
            .factory_session
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            live_factories
                .entry(agent.name.to_ascii_lowercase())
                .or_default()
                .insert(session);
        }
    }
    let now = chrono::Utc::now();
    let since = now - chrono::Duration::days(RETIREMENT_LOOKBACK_DAYS);
    let mut candidates: Vec<_> = history
        .unverified
        .iter()
        .filter(|old| !all.iter().any(|a| a.id == old.id) && !routes.contains_key(&old.id))
        .filter(|old| old.retired_at >= since && old.retired_at <= now)
        .filter_map(|old| {
            let name = old.name.to_ascii_lowercase();
            let factories = live_factories.get(&name)?;
            if factories.len() != 1 {
                report.warnings.push(format!(
                    "legacy peer feedback skipped for {name}: competing live factories {}; address left unclaimed; resend to a current supervisor ID",
                    factories.iter().copied().collect::<Vec<_>>().join(", ")
                ));
                return None;
            }
            live.iter()
                .copied()
                .filter(|a| {
                    a.factory_session.as_deref() == Some(factory_session)
                        && a.name.eq_ignore_ascii_case(&old.name)
                })
                .max_by_key(|a| (a.registered_at, &a.id))
                .map(|author| (old, author))
        })
        .collect();
    candidates.sort_by(|(a, _), (b, _)| {
        b.retired_at
            .cmp(&a.retired_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut inspections = BTreeMap::new();
    for (old, author) in candidates {
        if inspections.contains_key(&old.id) {
            continue;
        }
        if retired_count + inspections.len() >= RETIRED_RECIPIENT_MAX {
            report.warnings.push(RETIREMENT_CAP_WARNING.into());
            break;
        }
        inspections.insert(old.id.clone(), author);
    }
    report.warnings.sort();
    report.warnings.dedup();
    let recipient_ids: Vec<_> = routes.keys().chain(inspections.keys()).cloned().collect();
    let project_ids: Vec<String> = std::iter::once(canonical_id.to_string())
        .chain(alias_class.iter().cloned())
        .collect();
    let mut claimed = match mailbox.claim_report(&project_ids, &recipient_ids, consumer_id) {
        Ok(batch) => {
            if !batch.denied.is_empty() {
                report.errors.push(format!(
                    "peer message claim denied projects: {}",
                    batch.denied.join(", ")
                ));
            }
            report.errors.extend(batch.errors);
            batch.messages
        }
        Err(error) => {
            report.errors.push(error);
            return report;
        }
    };
    report.claimed = claimed.len();
    // cas-c7c7: show each sender under its machine label, not its id.
    if !claimed.is_empty() {
        let mut senders: Vec<String> = claimed.iter().map(|m| m.sender_agent_id.clone()).collect();
        senders.sort();
        senders.dedup();
        let labels = mailbox.sender_labels(canonical_id, &senders);
        for message in claimed.iter_mut() {
            if let Some(label) = labels.get(&message.sender_agent_id) {
                message.sender_label = Some(label.clone());
            }
        }
    }
    let mut acks = Vec::new();
    for message in &claimed {
        let recipient = routes.get(&message.recipient_agent_id).copied();
        let same_repo = crate::cloud::project_ids_match_with_aliases(
            &message.project_id,
            canonical_id,
            alias_class,
        );
        if same_repo && let Some(author) = inspections.get(&message.recipient_agent_id) {
            // Never forward an untyped legacy body. Confirm actionable,
            // idempotent sender feedback before terminally rejecting it.
            match send_unverified_address_notice(mailbox, author, message) {
                Ok(()) => {
                    report.rejected += 1;
                    acks.push((message.id.clone(), PeerAckOutcome::Rejected));
                }
                Err(error) => report.errors.push(error), // Leave original unacked for retry.
            }
            continue;
        }
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
            Ok(admitted) => {
                if let Err(error) = on_admitted(recipient, message) {
                    report.errors.push(format!(
                        "peer message {} admitted, but recording it failed: {error}",
                        message.id
                    ));
                    continue;
                }
                match admitted {
                    EnqueueIdempotentResult::Created(id) => {
                        report.delivered += 1;
                        report.admitted.push(id);
                    }
                    EnqueueIdempotentResult::AlreadyExists(_) => report.duplicates += 1,
                }
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
) -> Result<SendReceipt, PeerSendError> {
    if peer.session.as_deref().is_none_or(|s| s.trim().is_empty()) {
        return Err(PeerSendError::NoReceivingDaemon(format!(
            "{}@{}",
            peer.name, peer.machine
        )));
    }
    send_raw(
        cas_root,
        sender_agent_id,
        &peer.agent_id,
        body,
        summary,
        in_reply_to,
        &format!("{}@{}", peer.name, peer.machine),
    )
}

pub(crate) fn send_raw(
    cas_root: &Path,
    sender_agent_id: &str,
    recipient_agent_id: &str,
    body: &str,
    summary: Option<&str>,
    in_reply_to: Option<&str>,
    recipient_label: &str,
) -> Result<SendReceipt, PeerSendError> {
    let project_id =
        crate::cloud::resolve_canonical_id(cas_root).ok_or_else(|| PeerSendError::Failed {
            cause: "this project has no canonical id to address peers by".into(),
            retry: String::new(),
        })?;
    outbox::enqueue_and_send(
        cas_root,
        PeerSend {
            project_id,
            sender_agent_id: sender_agent_id.to_string(),
            recipient_agent_id: recipient_agent_id.to_string(),
            body: body.to_string(),
            summary: summary.map(str::to_string),
            in_reply_to: in_reply_to.map(str::to_string),
            dedupe_key: String::new(),
        },
        recipient_label,
    )
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
    if envelope
        .session
        .as_deref()
        .is_none_or(|session| session.trim().is_empty())
    {
        return Err(PeerSendError::NoReceivingDaemon(format!(
            "{}@{}",
            envelope.sender_name, envelope.machine
        ))
        .to_string());
    }
    let receipt = send_raw(
        cas_root,
        sender_agent_id,
        &envelope.sender_agent_id,
        body,
        summary,
        Some(&envelope.id),
        &format!("{}@{}", envelope.sender_name, envelope.machine),
    )
    .map_err(|error| error.to_string())?;
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

/// `PeerMailbox` over Cassy Cloud HTTP.
#[derive(Clone)]
pub struct HttpPeerMailbox {
    endpoint: String,
    token: String,
    retry_at: std::sync::Arc<std::sync::Mutex<Option<chrono::DateTime<chrono::Utc>>>>,
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
            retry_at: Default::default(),
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
            Err(error) => Err(super::rate_limit::Failure::from_http(what, error).cause),
        }
    }

    fn post(&self, suffix: &str, body: serde_json::Value) -> Result<ureq::Response, ureq::Error> {
        self.post_with_timeout(suffix, body, MAILBOX_TIMEOUT)
    }

    /// Later chunks and ACKs share the server cooldown. A local refusal
    /// retains earlier claimed bodies and gives retries the remaining delay.
    fn cooldown(&self) -> Option<ureq::Error> {
        let delay = self.retry_after()?;
        let seconds = delay
            .as_secs()
            .saturating_add(u64::from(delay.subsec_nanos() != 0));
        let response = format!(
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: {seconds}\r\nContent-Length: 0\r\n\r\n"
        )
        .parse::<ureq::Response>()
        .expect("static rate-limit response with integer header");
        Some(ureq::Error::Status(429, response))
    }

    fn observe(
        &self,
        result: Result<ureq::Response, ureq::Error>,
    ) -> Result<ureq::Response, ureq::Error> {
        if let Err(ureq::Error::Status(429, ref response)) = result
            && let Some(delay) = super::rate_limit::retry_after(response)
        {
            let until = chrono::Duration::from_std(delay)
                .ok()
                .and_then(|delay| chrono::Utc::now().checked_add_signed(delay))
                .unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::seconds(
                    super::rate_limit::MAX_RETRY_AFTER.as_secs() as i64));
            let mut current = self.retry_at.lock().expect("mailbox retry clock");
            *current = Some(current.map_or(until, |previous| previous.max(until)));
        }
        result
    }

    fn post_with_timeout(
        &self,
        suffix: &str,
        body: serde_json::Value,
        timeout: Duration,
    ) -> Result<ureq::Response, ureq::Error> {
        if let Some(error) = self.cooldown() {
            return Err(error);
        }
        self.observe(
            ureq::post(&self.url(suffix))
                .timeout(timeout)
                .set("Authorization", &format!("Bearer {}", self.token))
                .send_json(body),
        )
    }

    pub(crate) fn retry_after(&self) -> Option<Duration> {
        self.retry_at
            .lock()
            .expect("mailbox retry clock")
            .and_then(|until| (until - chrono::Utc::now()).to_std().ok())
    }
}

/// `POST /api/peer-messages/claim` response.
#[derive(Deserialize)]
pub struct PeerClaim {
    pub messages: Vec<ClaimedPeerMessage>,
    #[serde(default)]
    pub denied: Vec<String>,
    /// Local transport failures from individual claim batches. Successful
    /// batches remain available for admission and ACK even when another fails.
    #[serde(skip)]
    pub errors: Vec<String>,
}

#[derive(Deserialize)]
struct AckResponse {
    results: Vec<AckResult>,
}
#[derive(Deserialize)]
struct AckResult {
    id: String,
    status: u16,
}

impl PeerMailbox for HttpPeerMailbox {
    fn sender_labels(
        &self,
        project_id: &str,
        sender_ids: &[String],
    ) -> std::collections::HashMap<String, String> {
        if self.retry_after().is_some() {
            return std::collections::HashMap::new();
        }
        let response = ureq::get(&format!("{}/api/agents", self.endpoint))
            .timeout(MAILBOX_TIMEOUT)
            .set("Authorization", &format!("Bearer {}", self.token))
            .query("project", project_id)
            .query("limit", "500")
            .call();
        let Ok(listing) = Self::read::<serde_json::Value>("agent list", response) else {
            return std::collections::HashMap::new();
        };
        listing["agents"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|agent| {
                let id = agent["id"].as_str()?;
                let label = agent["metadata"][crate::cloud::peers::META_HOSTNAME]
                    .as_str()
                    .map(str::trim)
                    .filter(|label| !label.is_empty())?;
                sender_ids
                    .iter()
                    .any(|sender| sender == id)
                    .then(|| (id.to_string(), label.to_string()))
            })
            .collect()
    }

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
        let batch = self.claim_report(project_ids, recipient_agent_ids, consumer_id)?;
        if !batch.errors.is_empty() {
            return Err(batch.errors.join("; "));
        }
        if batch.denied.is_empty() {
            Ok(batch.messages)
        } else {
            Err(format!(
                "peer message claim denied projects: {}",
                batch.denied.join(", ")
            ))
        }
    }

    fn claim_report(
        &self,
        project_ids: &[String],
        recipient_agent_ids: &[String],
        consumer_id: &str,
    ) -> Result<PeerClaim, String> {
        let mut combined = PeerClaim {
            messages: Vec::new(),
            denied: Vec::new(),
            errors: Vec::new(),
        };
        for projects in project_ids.chunks(50) {
            for recipients in recipient_agent_ids.chunks(50) {
                let batch = Self::read::<PeerClaim>("peer message claim", self.post("/claim", serde_json::json!({
                    "project_ids":projects,"recipient_agent_ids":recipients,"consumer_id":consumer_id,
                    "max":CLAIM_MAX,"lease_secs":LEASE_SECS,
                })));
                match batch {
                    Ok(batch) => {
                        combined.messages.extend(batch.messages);
                        combined.denied.extend(batch.denied);
                    }
                    Err(error) => combined.errors.push(error),
                }
            }
        }
        combined.denied.sort();
        combined.denied.dedup();
        Ok(combined)
    }

    fn ack(&self, consumer_id: &str, acks: &[(String, PeerAckOutcome)]) -> Result<(), String> {
        let mut errors = Vec::new();
        for batch in acks.chunks(50) {
            let items: Vec<_> = batch
                .iter()
                .map(|(id, outcome)| serde_json::json!({"id":id,"outcome":outcome}))
                .collect();
            let response: AckResponse = Self::read(
                "peer message ack",
                super::rate_limit::call(MAILBOX_TIMEOUT, |left| {
                    self.post_with_timeout(
                        "/ack",
                        serde_json::json!({"consumer_id":consumer_id,"acks":items}),
                        left,
                    )
                }),
            )?;
            if response.results.len() != batch.len() {
                return Err("peer message ack: incomplete per-item receipt".into());
            }
            for ((id, outcome), result) in batch.iter().zip(response.results) {
                if result.id != *id {
                    errors.push(format!(
                        "peer message ack: expected {id}, received {}",
                        result.id
                    ));
                    continue;
                }
                match result.status {
                    200 => {}
                    409 => {
                        let expected = match outcome {
                            PeerAckOutcome::Delivered => "delivered",
                            PeerAckOutcome::Rejected => "rejected",
                        };
                        match self.status(id) {
                            Ok(status) if status.status==expected=>{},
                            Ok(status)=>errors.push(format!("peer message ack {id} refused (409): status is {}, expected {expected}",status.status)),
                            Err(error)=>errors.push(format!("peer message ack {id} refused (409): {error}")),
                        }
                    }
                    429 => errors.push(format!("peer message ack {id}: rate limited (429)")),
                    code => errors.push(format!("peer message ack {id} refused ({code})")),
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    fn status(&self, id: &str) -> Result<PeerMessageStatus, String> {
        if let Some(error) = self.cooldown() {
            return Err(super::rate_limit::Failure::from_http("peer message status", error).cause);
        }
        Self::read(
            "peer message status",
            self.observe(
                ureq::get(&self.url(&format!("/{id}")))
                    .timeout(MAILBOX_TIMEOUT)
                    .set("Authorization", &format!("Bearer {}", self.token))
                    .call(),
            ),
        )
    }
}

#[cfg(test)]
#[path = "peer_mailbox_tests.rs"]
mod tests;
