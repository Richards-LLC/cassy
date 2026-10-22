//! Splitting an epic between peer supervisors (cas-4736).
//!
//! Two supervisors of one repository (on two machines, or two clones) agree
//! who takes which children of an epic:
//!
//! 1. The proposer claims its own share in Cassy Cloud (all or nothing),
//!    records an outgoing pending split, and sends the peer a structured
//!    proposal through the peer mailbox (cas-f9c7).
//! 2. The peer's factory daemon records the proposal as incoming. Recording
//!    claims, spawns and directs nothing.
//! 3. The peer accepts, claiming only its own share, or rejects. Its answer
//!    flows back through the mailbox and the proposer's daemon records it.
//! 4. Either side may cancel. The canceller releases only the claims it
//!    holds itself; the other side's daemon marks the split cancelled and
//!    leaves that side's claims alone.
//!
//! While a split binds a side, a start, claim or pre-assigned spawn of a
//! child the other side took is refused unless a supervisor authorizes an
//! assignment to a specific worker with a reason. The gate reads only
//! this database, so it holds even when the cloud is unreachable.
//!
//! Authority (cas-604d): a split line is honoured only in a message the cloud
//! delivered to a local supervisor, whose sender identity the cloud stamped;
//! an answer or cancel changes only a split whose peer was that sender. Normal
//! cancellation releases only the caller's claims. A live registered
//! supervisor may supersede an accepted split only after its registered owner
//! has no live process and its heartbeat is past the stale threshold; the
//! cancellation reason records both supervisors.

use std::path::Path;

use chrono::{DateTime, Utc};

use cas_store::{
    EpicSplit, SplitDirection, SplitState, decide_epic_split, get_epic_split, insert_epic_split,
    list_epic_splits, decide_epic_split_by, finish_epic_split_work,
    list_epic_split_child_releases, mark_epic_split_child_released,
    pending_epic_split_work,
};

use crate::cloud::peer_claims::{Acquire, ClaimKind, PeerClaims};
use crate::cloud::peer_mailbox::{self, ClaimedPeerMessage, escape, unescape};
use crate::cloud::peers::{PeerDiscovery, discover_peers};
use crate::types::{Agent, DependencyType, TaskStatus, TaskType};

/// Tag of the machine-readable first line of a split message.
const SPLIT_TAG: &str = "<cas-epic-split ";

/// How long an unanswered proposal stays open by default.
pub const DEFAULT_TTL_SECS: i64 = 30 * 60;
/// Longest accepted proposal lifetime.
pub const MAX_TTL_SECS: i64 = 24 * 60 * 60;

/// What a split message says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitKind {
    Proposal,
    Accept,
    Reject,
    Cancel,
    Expired,
}

impl SplitKind {
    fn as_str(self) -> &'static str {
        match self {
            SplitKind::Proposal => "proposal",
            SplitKind::Accept => "accept",
            SplitKind::Reject => "reject",
            SplitKind::Cancel => "cancel",
            SplitKind::Expired => "expired",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "proposal" => Some(SplitKind::Proposal),
            "accept" => Some(SplitKind::Accept),
            "reject" => Some(SplitKind::Reject),
            "cancel" => Some(SplitKind::Cancel),
            "expired" => Some(SplitKind::Expired),
            _ => None,
        }
    }
}

/// The machine-readable line of a split message. Shares are named from the
/// proposer's side, so both databases read the same line the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitMessage {
    pub kind: SplitKind,
    /// Proposal id.
    pub id: String,
    pub epic: String,
    pub proposer_takes: Vec<String>,
    pub peer_takes: Vec<String>,
    /// When an unanswered proposal lapses.
    pub expires_at: Option<DateTime<Utc>>,
    /// When the sender wrote it.
    pub at: DateTime<Utc>,
    pub reason: Option<String>,
}

impl SplitMessage {
    /// The line, without a trailing newline.
    pub fn render(&self) -> String {
        let mut line = format!("{SPLIT_TAG}v=\"1\"");
        let mut attribute = |key: &str, value: &str| {
            line.push_str(&format!(" {key}=\"{}\"", escape(value)));
        };
        attribute("kind", self.kind.as_str());
        attribute("id", &self.id);
        attribute("epic", &self.epic);
        attribute("proposer_takes", &self.proposer_takes.join(","));
        attribute("peer_takes", &self.peer_takes.join(","));
        if let Some(expires_at) = self.expires_at {
            attribute("expires_at", &expires_at.to_rfc3339());
        }
        attribute("at", &self.at.to_rfc3339());
        if let Some(reason) = &self.reason {
            attribute("reason", reason);
        }
        line.push_str("/>");
        line
    }

    /// Parse the split line from the first line of `body`, and only there.
    pub fn parse(body: &str) -> Option<Self> {
        let line = body.lines().next()?.trim_end();
        let mut rest = line.strip_prefix(SPLIT_TAG)?.strip_suffix("/>")?;
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
        let time = |value: Option<&String>| {
            value
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|at| at.with_timezone(&Utc))
        };
        let ids = |value: Option<&String>| parse_ids(value.map(String::as_str).unwrap_or(""));
        let id = attributes.get("id").filter(|id| !id.is_empty())?.clone();
        Some(Self {
            kind: SplitKind::parse(attributes.get("kind")?)?,
            id,
            epic: attributes.get("epic").cloned().unwrap_or_default(),
            proposer_takes: ids(attributes.get("proposer_takes")),
            peer_takes: ids(attributes.get("peer_takes")),
            expires_at: time(attributes.get("expires_at")),
            at: time(attributes.get("at"))?,
            reason: attributes.get("reason").filter(|r| !r.is_empty()).cloned(),
        })
    }
}

/// Comma- or whitespace-separated task ids, trimmed and deduplicated in order.
pub fn parse_ids(value: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for id in value.split(|c: char| c == ',' || c.is_whitespace()) {
        let id = id.trim();
        if !id.is_empty() && !ids.iter().any(|seen| seen == id) {
            ids.push(id.to_string());
        }
    }
    ids
}

/// A binding split that gives `task_id` to the peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitHold {
    pub split_id: String,
    pub epic_id: String,
    pub peer_name: String,
    pub peer_machine: String,
    /// `accepted`, or `pending` for this side's own open proposal.
    pub state: &'static str,
}

impl SplitHold {
    fn describe(&self) -> String {
        format!("{}@{}", self.peer_name, self.peer_machine)
    }

    /// Refusal for a start, claim or spawn of the peer's child. `prefix` is
    /// the caller's tool prefix.
    pub fn refusal(&self, task_id: &str, prefix: &str) -> String {
        format!(
            "⛔ SPLIT TO A PEER — {task_id} belongs to peer supervisor {} under epic split {} of {} ({}).\n\
             Starting it here would duplicate their work. Ask them first: \
             {prefix}coordination action=message target={} summary=\"{task_id}\" message=\"...\"\n\
             To end the split: {prefix}factory action=epic_split id={} decision=cancel. \
             To redirect it, cancel the split above. To authorize one worker's start: \
             {prefix}task action=update id={task_id} assignee=<worker> supervisor_override=true reason=\"<agreed takeover reason>\". \
             Only a live registered supervisor may authorize this; force=true does not bypass a split.",
            self.describe(),
            self.split_id,
            self.epic_id,
            self.state,
            self.describe(),
            self.split_id,
        )
    }

    /// Receipt for the worker consuming a supervisor-authorized takeover.
    pub fn override_note(&self, task_id: &str, authorization: &cas_types::SplitTakeoverAuthorization) -> String {
        format!(
            "\n\nSPLIT OVERRIDE — {task_id} belongs to {} under epic split {}; worker {} consumed supervisor {}'s authorization. Reason: {}",
            self.describe(),
            self.split_id,
            authorization.worker_id,
            authorization.supervisor_id,
            authorization.reason.replace(['\n', '\r'], " "),
        )
    }
}

/// Read paths never create a database where there is none.
fn has_store(cas_root: &Path) -> bool {
    cas_root.join("cas.db").exists()
}

/// Shares that can still reserve children in their original epic. Keep raw
/// list/get and cleanup reads historical: this projection never cancels a
/// split or releases a claim. Callers still apply their split-state policy.
pub(crate) fn current_epic_splits(cas_root: &Path) -> Result<Vec<EpicSplit>, String> {
    projected_epic_splits(cas_root, false)
}

/// Overlap also reserves the original shares while claim cleanup is pending.
/// Keep that historical coverage out of ordinary ownership/renewal projections.
fn projected_epic_splits(cas_root: &Path, include_cleanup: bool) -> Result<Vec<EpicSplit>, String> {
    let splits = list_epic_splits(cas_root)
        .map_err(|error| format!("epic split store: {error}"))?;
    if splits.is_empty() {
        return Ok(splits);
    }
    let released: std::collections::HashSet<(String, String)> =
        list_epic_split_child_releases(cas_root)
            .map_err(|error| format!("epic split release store: {error}"))?
            .into_iter()
            .collect();
    // Ownership reads must not replay a failed outbox repair and lose a live
    // binding while local task/parent rows remain perfectly readable.
    let tasks = crate::store::open_task_store_local(cas_root)
        .map_err(|error| format!("could not open the task store: {error}"))?;
    let mut terminal_epics = std::collections::HashMap::new();
    let mut parents = std::collections::HashMap::new();
    let mut current = Vec::new();
    for mut split in splits {
        let split_id = split.id.clone();
        split.mine.retain(|id| !released.contains(&(split_id.clone(), id.clone())));
        split.theirs.retain(|id| !released.contains(&(split_id.clone(), id.clone())));
        if split.mine.is_empty() && split.theirs.is_empty() {
            continue;
        }
        if include_cleanup && split.cleanup_pending {
            current.push(split);
            continue;
        }
        let terminal = match terminal_epics.get(&split.epic_id) {
            Some(terminal) => *terminal,
            None => {
                let terminal = match tasks.get(&split.epic_id) {
                    Ok(epic) => epic.is_terminal(),
                    // An incomplete sync is not proof that ownership ended.
                    Err(cas_store::StoreError::TaskNotFound(_)) => false,
                    Err(error) => return Err(format!("could not read split epic: {error}")),
                };
                terminal_epics.insert(split.epic_id.clone(), terminal);
                terminal
            }
        };
        if terminal {
            continue;
        }
        for id in split.mine.iter().chain(&split.theirs) {
            if !parents.contains_key(id) {
                let parent = tasks.get_parent_epic(id)
                    .map_err(|error| format!("could not read split child parent: {error}"))?
                    .map(|epic| epic.id);
                parents.insert(id.clone(), parent);
            }
        }
        let still_child = |id: &String| {
            parents[id].as_deref().is_none_or(|parent| parent == split.epic_id)
        };
        split.mine.retain(still_child);
        split.theirs.retain(still_child);
        if !split.mine.is_empty() || !split.theirs.is_empty() {
            current.push(split);
        }
    }
    Ok(current)
}

/// The binding split, if any, under which `task_id` is the peer's.
pub fn split_hold_for(cas_root: &Path, task_id: &str) -> Option<SplitHold> {
    if !has_store(cas_root) {
        return None;
    }
    let splits = match current_epic_splits(cas_root) {
        Ok(splits) => splits,
        Err(error) => {
            tracing::debug!(%error, "could not read epic splits");
            return None;
        }
    };
    let now = Utc::now();
    splits
        .into_iter()
        .filter(|split| split.is_binding(now))
        .find(|split| split.theirs.iter().any(|id| id == task_id))
        .map(|split| SplitHold {
            state: if split.state == SplitState::Accepted {
                "accepted"
            } else {
                "pending"
            },
            split_id: split.id,
            epic_id: split.epic_id,
            peer_name: split.peer_name,
            peer_machine: split.peer_machine,
        })
}

/// What ingesting one delivered peer message did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ingested {
    /// Not a split message.
    NotASplit,
    /// An incoming proposal was recorded (or already was).
    Proposal(String),
    /// A split moved to a new state.
    Decided { id: String, state: SplitState },
    /// A split message that changes nothing here, and why.
    Ignored(String),
}

/// Record the split line of a peer message the cloud delivered to local
/// supervisor `recipient`. Idempotent. Rejected/expired decisions release
/// only this database's own split claims before the mailbox acknowledges them.
pub fn ingest(
    cas_root: &Path,
    recipient: &Agent,
    message: &ClaimedPeerMessage,
) -> Result<Ingested, String> {
    let Some(line) = SplitMessage::parse(&message.body) else {
        return Ok(Ingested::NotASplit);
    };
    let store_error = |error: cas_store::StoreError| format!("epic split store: {error}");
    if line.kind == SplitKind::Proposal {
        let Some(expires_at) = line.expires_at else {
            return Ok(Ingested::Ignored(format!("proposal {} has no expiry", line.id)));
        };
        if line.epic.is_empty() || (line.proposer_takes.is_empty() && line.peer_takes.is_empty()) {
            return Ok(Ingested::Ignored(format!("proposal {} names no share", line.id)));
        }
        if let Some(existing) = get_epic_split(cas_root, &line.id).map_err(store_error)?
            && existing.peer_agent_id != message.sender_agent_id
        {
            return Ok(Ingested::Ignored(format!(
                "proposal id {} is already recorded for another peer",
                line.id
            )));
        }
        insert_epic_split(
            cas_root,
            &EpicSplit {
                id: line.id.clone(),
                epic_id: line.epic.clone(),
                local_agent_id: recipient.id.clone(),
                peer_agent_id: message.sender_agent_id.clone(),
                peer_name: message.sender_name.clone(),
                // cas-c7c7: the sender's label, as everywhere else.
                peer_machine: message.sender_machine(),
                direction: SplitDirection::Incoming,
                mine: line.peer_takes.clone(),
                theirs: line.proposer_takes.clone(),
                state: SplitState::Pending,
                reason: None,
                message_id: Some(message.id.clone()),
                expires_at,
                created_at: Utc::now(),
                decided_at: None,
                decided_by: None,
                cleanup_pending: false,
                expiry_notice_pending: false,
            },
        )
        .map_err(store_error)?;
        return Ok(Ingested::Proposal(line.id));
    }

    let Some(split) = get_epic_split(cas_root, &line.id).map_err(store_error)? else {
        return Ok(Ingested::Ignored(format!("no split {} recorded here", line.id)));
    };
    if split.peer_agent_id != message.sender_agent_id || split.local_agent_id != recipient.id {
        return Ok(Ingested::Ignored(format!(
            "split {} is not between {} and {}",
            line.id, recipient.name, message.sender_name
        )));
    }
    let (from, to): (&[SplitState], SplitState) = match line.kind {
        SplitKind::Accept | SplitKind::Reject if split.direction != SplitDirection::Outgoing => {
            return Ok(Ingested::Ignored(format!(
                "only the proposer's side records an answer to {}",
                line.id
            )));
        }
        SplitKind::Accept => {
            if split.expires_at < line.at {
                return Ok(Ingested::Ignored(format!(
                    "acceptance of {} is dated after its expiry",
                    line.id
                )));
            }
            (&[SplitState::Pending], SplitState::Accepted)
        }
        SplitKind::Reject => (&[SplitState::Pending], SplitState::Rejected),
        SplitKind::Cancel => (
            &[SplitState::Pending, SplitState::Accepted],
            SplitState::Cancelled,
        ),
        SplitKind::Expired => {
            if line.at < split.expires_at {
                return Ok(Ingested::Ignored(format!("premature expiry of {}", line.id)));
            }
            // A persisted expiry cannot be resurrected by a delayed acceptance.
            (&[SplitState::Pending, SplitState::Accepted], SplitState::Expired)
        }
        SplitKind::Proposal => unreachable!("handled above"),
    };
    let changed = decide_epic_split_by(cas_root, &line.id, from, to, line.reason.as_deref(), line.at,
        Some(&message.sender_agent_id))
        .map_err(store_error)?;
    if let Some(current) = get_epic_split(cas_root, &line.id).map_err(store_error)? {
        cleanup_decision(cas_root, &current)?;
    }
    Ok(if changed {
        Ingested::Decided { id: line.id, state: to }
    } else {
        Ingested::Ignored(format!("split {} is already {}", line.id, split.state.as_str()))
    })
}

/// How a split message left this side.
enum Sent {
    /// Cassy Cloud accepted it: `(peer message id, status)`.
    Now(String, String),
    /// The cloud did not answer, but the message is in the peer outbox and
    /// the factory daemon retries it with the same key. It will arrive, so
    /// the split stands.
    Queued(String),
}

impl Sent {
    fn describe(&self) -> String {
        match self {
            Sent::Now(id, status) => format!("peer message {id}, {status}"),
            Sent::Queued(why) => format!("not sent yet: {why}"),
        }
    }
}

/// Classify a send: a refusal the outbox will not retry is an error.
fn sent(
    result: Result<peer_mailbox::SendReceipt, peer_mailbox::PeerSendError>,
) -> Result<Sent, String> {
    match result {
        Ok(receipt) => Ok(Sent::Now(receipt.id, receipt.status)),
        Err(peer_mailbox::PeerSendError::Failed { cause, retry }) if !retry.is_empty() => {
            Ok(Sent::Queued(format!("{cause}{retry}")))
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Send a split message to the split's peer, by agent id.
fn send_to_split_peer(
    cas_root: &Path,
    caller: &Caller,
    split: &EpicSplit,
    body: &str,
    summary: &str,
) -> Result<Sent, String> {
    sent(peer_mailbox::send_raw(
        cas_root,
        &caller.id,
        &split.peer_agent_id,
        body,
        Some(summary),
        split.message_id.as_deref(),
        &format!("{}@{}", split.peer_name, split.peer_machine),
    ))
}

/// The registered supervisor acting.
#[derive(Debug, Clone)]
pub struct Caller {
    pub id: String,
    pub name: String,
}

/// Claim `ids` in Cassy Cloud for `caller`, all or nothing. A child already
/// claimed by an agent of this database (the caller or one of its workers)
/// counts as held. On failure every claim taken by this call is released.
/// Returns the warnings worth showing.
fn claim_share(
    claims: &PeerClaims,
    ids: &[String],
    caller: &Caller,
    duration: u32,
    is_local: &dyn Fn(&str) -> bool,
) -> Result<Vec<String>, String> {
    let mut taken: Vec<&str> = Vec::new();
    let mut warnings = Vec::new();
    let mut failure = None;
    for id in ids {
        match claims.holder(id) {
            Ok(Some(holder)) if holder == caller.id || is_local(&holder) => continue,
            Ok(_) => {}
            Err(error) => {
                failure = Some(format!("Cassy Cloud could not be asked about {id}: {error}"));
                break;
            }
        }
        match claims.acquire(id, ClaimKind::Task, &caller.id, &caller.name, duration, &|_| false) {
            Acquire::Claimed => taken.push(id),
            // Another agent of this database took it since the read above:
            // still this side's.
            Acquire::LocalHolder(_) => {}
            Acquire::StalePeer(hold) => warnings.push(format!(
                "{id}: {}'s claim {}; Cloud recovery was not confirmed",
                hold.describe(),
                hold.stale_reason(Utc::now())
            )),
            Acquire::PeerHolds(hold) => {
                failure = Some(format!(
                    "{id} is claimed by {} until {}",
                    hold.describe(),
                    hold.until()
                ));
                break;
            }
            Acquire::Unavailable(reason) => {
                failure = Some(format!("Cassy Cloud could not claim {id}: {reason}"));
                break;
            }
        }
    }
    match failure {
        None => Ok(warnings),
        Some(failure) => {
            for id in taken {
                if let Err(error) = claims.release_own(id, &caller.id) {
                    tracing::warn!(task = %id, %error, "could not release a split claim");
                }
            }
            Err(failure)
        }
    }
}

/// Release the claims `caller` itself holds on `ids`. Returns how many.
fn release_share(claims: &PeerClaims, ids: &[String], caller_id: &str) -> usize {
    ids.iter()
        .filter(|id| claims.release_own(id, caller_id).unwrap_or(false))
        .count()
}

fn cleanup_decision(cas_root: &Path, split: &EpicSplit) -> Result<(), String> {
    if !split.cleanup_pending {
        return Ok(());
    }
    if !split.mine.is_empty() {
        let claims = peer_claims(cas_root)?;
        for id in &split.mine {
            claims.release_own(id, &split.local_agent_id)
                .map_err(|error| format!("{} cleanup pending for {id}: {error}", split.id))?;
        }
    }
    finish_epic_split_work(cas_root, split, true).map_err(|error| error.to_string())
}

fn expiry_notice_id(owner: &str, split_id: &str) -> String {
    format!("split-expiry:{owner}:{split_id}")
}

/// The same expiry observed locally and through its authenticated peer is one
/// queue event. Unrelated senders retain their ordinary message identity.
pub(crate) fn expiry_notice_key(
    root: &Path, recipient: &Agent, message: &ClaimedPeerMessage,
) -> Result<Option<String>, String> {
    let Some(line) = SplitMessage::parse(&message.body).filter(|line| line.kind == SplitKind::Expired)
        else { return Ok(None) };
    let split = get_epic_split(root, &line.id).map_err(|error| error.to_string())?;
    Ok(split.filter(|split| split.local_agent_id == recipient.id
        && split.peer_agent_id == message.sender_agent_id && line.at >= split.expires_at)
        .map(|split| expiry_notice_id(&recipient.id, &split.id)))
}

/// Reconcile durable split work on the existing mailbox duty. No Cloud calls
/// hold the SQLite lock. Failed cleanup/expiry delivery remains pending.
pub fn reconcile(cas_root: &Path, session: &str, now: DateTime<Utc>) -> Result<(), String> {
    if !has_store(cas_root) { return Ok(()) }
    let agents = crate::store::open_agent_store(cas_root).map_err(|error| error.to_string())?;
    let owners: std::collections::HashMap<_, _> = agents.list(None).map_err(|error| error.to_string())?
        .into_iter().filter(|owner| owner.factory_session.as_deref() == Some(session))
        .map(|owner| (owner.id.clone(), owner)).collect();
    let owner_ids: Vec<_> = owners.keys().map(String::as_str).collect();
    let mut errors = Vec::new();
    for mut split in pending_epic_split_work(cas_root, now, &owner_ids).map_err(|error| error.to_string())? {
        let owner = &owners[&split.local_agent_id];
        let mut work = || -> Result<(), String> {
            if split.state == SplitState::Pending {
                decide_epic_split_by(cas_root, &split.id, &[SplitState::Pending], SplitState::Expired,
                    Some("unanswered proposal expired"), now, Some(&split.local_agent_id))
                    .map_err(|error| error.to_string())?;
                split = get_epic_split(cas_root, &split.id).map_err(|error| error.to_string())?
                    .ok_or("split disappeared during expiry")?;
            }
            let cleanup_error = cleanup_decision(cas_root, &split).err();
            if split.expiry_notice_pending {
                let line = SplitMessage {
                    kind: SplitKind::Expired, id: split.id.clone(), epic: split.epic_id.clone(),
                    proposer_takes: vec![], peer_takes: vec![], expires_at: Some(split.expires_at),
                    at: split.decided_at.ok_or("expiry has no decision time")?,
                    reason: split.reason.clone(),
                };
                let body = format!("{}\nEpic split {} expired unanswered; its split guard is lifted. Each side releases only its own frozen claims; use decision=status to inspect pending cleanup.", line.render(), split.id);
                // The local notice is authored by this daemon's own supervisor,
                // never presented as a statement from the remote peer.
                let message = ClaimedPeerMessage {
                    id: expiry_notice_id(&owner.id, &split.id),
                    project_id: crate::cloud::resolve_canonical_id(cas_root).ok_or("project has no canonical id")?,
                    recipient_agent_id: owner.id.clone(), sender_agent_id: owner.id.clone(),
                    sender_name: owner.name.clone(), sender_machine_id: owner.machine_id.clone(),
                    sender_label: Some(crate::cloud::peers::machine_label_for(cas_root)),
                    sender_session: owner.factory_session.clone(), body: body.clone(),
                    summary: Some(format!("epic split {} expired", split.id)), in_reply_to: None,
                    created_at: line.at.to_rfc3339(), attempts: 1, scope: Default::default(),
                };
                crate::store::open_prompt_queue_store(cas_root).map_err(|error| error.to_string())?
                    .enqueue_idempotent(&format!("peer:{}@{}", owner.name, message.sender_machine()),
                        &owner.name, &peer_mailbox::render_prompt(&message), Some(session),
                        message.summary.as_deref(), Some(cas_store::NotificationPriority::High),
                        &message.id, Some(&cas_store::QueueOrigin::Daemon))
                    .map_err(|error| error.to_string())?;
                sent(peer_mailbox::send_split_expiry(cas_root, &owner.id, &split.peer_agent_id,
                    &body, &split.id))?;
                finish_epic_split_work(cas_root, &split, false).map_err(|error| error.to_string())?;
            }
            if let Some(error) = cleanup_error { return Err(error) }
            Ok(())
        };
        if let Err(error) = work() { errors.push(error); }
    }
    if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
}

/// Local decision state plus explicitly observed Cloud claims. Unavailable
/// ownership is reported as unavailable rather than as an empty inventory.
pub fn status(cas_root: &Path, id: &str) -> Result<String, String> {
    let split = get_epic_split(cas_root, id).map_err(|error| error.to_string())?
        .ok_or_else(|| format!("no epic split {id} recorded here"))?;
    let expiry_due = split.is_expired(Utc::now());
    let mut out = format!("Epic split {id}: {}\nepic: {}\ndecided_at: {}\ndecider: {}\ncleanup: {}\nexpiry notice: {}\nclaims (Cloud observation; project/account visibility applies):",
        split.display_state(Utc::now()), split.epic_id,
        split.decided_at.map(|at| at.to_rfc3339()).unwrap_or_else(|| "pending".into()),
        split.decided_by.as_deref().unwrap_or("unknown"),
        if split.cleanup_pending || expiry_due { "pending" } else { "settled" },
        if split.expiry_notice_pending || expiry_due { "pending" } else { "settled" });
    let claims = peer_claims(cas_root);
    for (label, ids) in [("mine", &split.mine), ("theirs", &split.theirs)] {
        if ids.is_empty() { out.push_str(&format!("\n  {label}: (none)")); }
        for task in ids {
            let observed = match &claims {
                Ok(claims) => match claims.holder(task) {
                    Ok(Some(holder)) => holder,
                    Ok(None) => "none visible".into(),
                    Err(error) => format!("unavailable ({error})"),
                },
                Err(error) => format!("unavailable ({error})"),
            };
            out.push_str(&format!("\n  {label} {task}: {observed}"));
        }
    }
    Ok(out)
}

/// Persisted, latest decision used by one-shot reminders, including reminders
/// registered after the decision or after a daemon restart.
pub fn decision_data(split: &EpicSplit) -> Option<serde_json::Value> {
    (split.state != SplitState::Pending).then(|| serde_json::json!({
        "split_id": split.id, "epic_id": split.epic_id, "state": split.state.as_str(),
        "decided_at": split.decided_at, "decided_by": split.decided_by,
    }))
}

fn share_label(ids: &[String]) -> String {
    if ids.is_empty() { "(none)".into() } else { ids.join(", ") }
}

fn peer_claims(cas_root: &Path) -> Result<PeerClaims, String> {
    PeerClaims::for_project(cas_root).ok_or_else(|| {
        "epic_split needs Cassy Cloud: this project is not logged in (`cas login`) or has no \
         canonical repository id"
            .to_string()
    })
}

fn local_check(cas_root: &Path) -> Result<impl Fn(&str) -> bool, String> {
    let agents = crate::store::open_agent_store(cas_root)
        .map_err(|error| format!("could not open the agent store: {error}"))?;
    Ok(move |id: &str| agents.get(id).is_ok())
}

/// Splits reserving a current child in `ids`, other than `except`.
/// Outgoing proposals reserve through expiry cleanup; a live successor may
/// cancel an expired orphan after proving its owner is gone. Cleanup work and
/// accepted splits also reserve, while expired incoming proposals do not.
fn overlapping(cas_root: &Path, ids: &[String], except: Option<&str>) -> Result<Option<(EpicSplit, String)>, String> {
    let now = Utc::now();
    let splits = projected_epic_splits(cas_root, true)?;
    Ok(splits
        .into_iter()
        .filter(|split| Some(split.id.as_str()) != except)
        .filter(|split| split.cleanup_pending || split.state == SplitState::Accepted
            || (split.state == SplitState::Pending
                && (split.direction == SplitDirection::Outgoing || !split.is_expired(now))))
        .find_map(|split| {
            let child = ids.iter()
                .find(|id| split.mine.contains(id) || split.theirs.contains(id))
                .cloned()?;
            Some((split, child))
        }))
}

fn overlap_refusal(cas_root: &Path, split: &EpicSplit, child: &str) -> String {
    let epic_state = crate::store::open_task_store(cas_root).ok()
        .and_then(|tasks| tasks.get(&split.epic_id).ok())
        .map(|epic| epic.status.to_string()).unwrap_or_else(|| "unknown".into());
    format!(
        "{child} is already in epic split {} ({}) of epic {} ({}). \
         Settle pending cleanup, or end it with factory action=epic_split id={} decision=cancel \
         reason=\"<why this split should end>\". If its owner is gone, a live registered \
         supervisor can cancel after the stale-owner check.",
        split.id, split.state.as_str(), split.epic_id, epic_state, split.id,
    )
}

/// Propose splitting `epic_id` with peer supervisor `peer_target`: claim
/// `mine` now, record the proposal, and send it. Either share may be empty
/// for a one-sided request or grant, but at least one task must be named.
pub fn propose(
    cas_root: &Path,
    caller: &Caller,
    epic_id: &str,
    peer_target: &str,
    mine: &[String],
    theirs: &[String],
    ttl_secs: Option<i64>,
) -> Result<String, String> {
    if mine.is_empty() && theirs.is_empty() {
        return Err("epic_split needs at least one share: mine=<ids> or theirs=<ids>".to_string());
    }
    if let Some(both) = mine.iter().find(|id| theirs.contains(id)) {
        return Err(format!("{both} is in both mine and theirs; the shares must be disjoint"));
    }
    let ttl = ttl_secs.unwrap_or(DEFAULT_TTL_SECS);
    if !(1..=MAX_TTL_SECS).contains(&ttl) {
        return Err(format!("ttl_secs must be between 1 and {MAX_TTL_SECS}"));
    }

    let tasks = crate::store::open_task_store(cas_root)
        .map_err(|error| format!("could not open the task store: {error}"))?;
    let epic = tasks
        .get(epic_id)
        .map_err(|_| format!("epic {epic_id} not found"))?;
    if epic.task_type != TaskType::Epic {
        return Err(format!("{epic_id} is not an epic"));
    }
    for id in mine.iter().chain(theirs) {
        let task = tasks
            .get(id)
            .map_err(|_| format!("task {id} not found"))?;
        let child = tasks.get_dependencies(id).unwrap_or_default().iter().any(|dep| {
            dep.dep_type == DependencyType::ParentChild && dep.to_id == epic_id
        });
        if !child {
            return Err(format!("{id} is not a child of epic {epic_id}"));
        }
        if task.status == TaskStatus::Closed {
            return Err(format!("{id} is already closed"));
        }
    }
    let all: Vec<String> = mine.iter().chain(theirs).cloned().collect();
    if let Some((split, child)) = overlapping(cas_root, &all, None)? {
        return Err(overlap_refusal(cas_root, &split, &child));
    }

    let (peer, redirect) = match discover_peers(cas_root, Some(&caller.id)) {
        Ok(PeerDiscovery::Found { peers, ended, .. }) => {
            // GH #1319: an offer to a supervisor that ended, or whose harness
            // is logged out, goes to its successor or is refused by name.
            let (addressed, was_ended) = match peer_mailbox::resolve_peer_target(&peers, peer_target)? {
                Some(peer) => (peer.clone(), false),
                None => (
                    crate::cloud::peers::ended_target(&ended, peer_target)
                        .cloned()
                        .ok_or_else(|| {
                            format!("no peer supervisor of this repository is named '{peer_target}'")
                        })?,
                    true,
                ),
            };
            match crate::cloud::peers::slot_holder(&peers, &addressed, was_ended) {
                crate::cloud::peers::SlotHolder::Addressed => (addressed, None),
                // cas-2db0: the proposer is told who received it instead.
                crate::cloud::peers::SlotHolder::Successor { peer, note } => (peer.clone(), Some(note)),
                crate::cloud::peers::SlotHolder::Refused(reason) => return Err(reason),
            }
        }
        Ok(PeerDiscovery::NotLoggedIn) => {
            return Err("this project is not logged in to Cassy Cloud (`cas login`)".to_string());
        }
        Ok(PeerDiscovery::NoProjectIdentity) => {
            return Err("this project has no canonical id to find peers by".to_string());
        }
        Err(error) => return Err(format!("could not look up peer supervisors: {error}")),
    };

    let claims = peer_claims(cas_root)?;
    let is_local = local_check(cas_root)?;
    let duration = crate::cloud::peer_claims::lease_duration_secs(cas_root);
    let warnings = claim_share(&claims, mine, caller, duration, &is_local)
        .map_err(|failure| format!("epic split not proposed, and nothing claimed: {failure}"))?;

    let now = Utc::now();
    let id = format!("split-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let split = EpicSplit {
        id: id.clone(),
        epic_id: epic_id.to_string(),
        local_agent_id: caller.id.clone(),
        peer_agent_id: peer.agent_id.clone(),
        peer_name: peer.name.clone(),
        peer_machine: peer.machine.clone(),
        direction: SplitDirection::Outgoing,
        mine: mine.to_vec(),
        theirs: theirs.to_vec(),
        state: SplitState::Pending,
        reason: None,
        message_id: None,
        expires_at: now + chrono::Duration::seconds(ttl),
        created_at: now,
        decided_at: None,
        decided_by: None,
        cleanup_pending: false,
        expiry_notice_pending: false,
    };
    let rollback = |why: &str| {
        let _ = decide_epic_split(
            cas_root,
            &id,
            &[SplitState::Pending],
            SplitState::Cancelled,
            Some(why),
            Utc::now(),
        );
        release_share(&claims, mine, &caller.id);
    };
    if let Err(error) = insert_epic_split(cas_root, &split) {
        release_share(&claims, mine, &caller.id);
        return Err(format!("epic split not recorded: {error}"));
    }
    let line = SplitMessage {
        kind: SplitKind::Proposal,
        id: id.clone(),
        epic: epic_id.to_string(),
        proposer_takes: mine.to_vec(),
        peer_takes: theirs.to_vec(),
        expires_at: Some(split.expires_at),
        at: now,
        reason: None,
    };
    let body = format!(
        "{}\nEpic split proposal {id} for {epic_id} from {}:\n\
         - I take (claimed): {}\n\
         - You take: {}\n\
         Accept, claiming only your share: factory action=epic_split id={id} decision=accept\n\
         Reject: factory action=epic_split id={id} decision=reject reason=\"...\"\n\
         Expires at {} unless accepted.",
        line.render(),
        caller.name,
        share_label(mine),
        share_label(theirs),
        split.expires_at.format("%H:%M UTC"),
    );
    let delivery = match sent(peer_mailbox::send_to_peer(
        cas_root,
        &caller.id,
        &peer,
        &body,
        Some(&format!("epic split proposal {id} for {epic_id}")),
        None,
    )) {
        Ok(delivery) => delivery,
        Err(error) => {
            rollback("the proposal could not be sent");
            return Err(format!(
                "epic split not proposed (claims released): could not reach {}@{}: {error}",
                peer.name, peer.machine
            ));
        }
    };
    let mut out = format!(
        "Proposed epic split {id} of {epic_id} to {}@{} ({}).\n\
         You took (claimed in Cassy Cloud): {}\n\
         Offered to {}: {}\n\
         Until they answer, starting their share here is refused. The proposal expires at {}.",
        peer.name,
        peer.machine,
        delivery.describe(),
        share_label(mine),
        peer.name,
        share_label(theirs),
        split.expires_at.format("%H:%M UTC"),
    );
    if let Some(note) = redirect {
        out.push_str(&format!("\n{note}"));
    }
    for warning in warnings {
        out.push_str(&format!("\n⚠️  {warning}"));
    }
    Ok(out)
}

/// Answer incoming proposal `id`: accept (claiming only this side's share)
/// or reject.
pub fn answer(
    cas_root: &Path,
    caller: &Caller,
    id: &str,
    accept: bool,
    reason: Option<&str>,
) -> Result<String, String> {
    let store_error = |error: cas_store::StoreError| format!("epic split store: {error}");
    let split = get_epic_split(cas_root, id)
        .map_err(store_error)?
        .ok_or_else(|| format!("no epic split {id} recorded here"))?;
    if split.direction != SplitDirection::Incoming {
        return Err(format!(
            "{id} is your own proposal; only {} can accept or reject it (to withdraw it: decision=cancel)",
            split.peer_name
        ));
    }
    if split.local_agent_id != caller.id {
        return Err(format!("{id} was proposed to another supervisor of this database"));
    }
    let now = Utc::now();
    if split.state != SplitState::Pending {
        return Err(format!("{id} is already {}", split.state.as_str()));
    }
    if split.is_expired(now) {
        return Err(format!(
            "{id} expired at {}; ask {} to propose again",
            split.expires_at.format("%H:%M UTC"),
            split.peer_name
        ));
    }
    let line = |kind| SplitMessage {
        kind,
        id: id.to_string(),
        epic: split.epic_id.clone(),
        proposer_takes: split.theirs.clone(),
        peer_takes: split.mine.clone(),
        expires_at: Some(split.expires_at),
        at: now,
        reason: reason.map(str::to_string),
    };

    if !accept {
        let changed = decide_epic_split_by(cas_root, id, &[SplitState::Pending], SplitState::Rejected, reason, now, Some(&caller.id))
            .map_err(store_error)?;
        if !changed {
            return Err(format!("{id} changed while rejecting it; check `epic_split`"));
        }
        let body = format!(
            "{}\nRejected epic split {id} of {}{}.",
            line(SplitKind::Reject).render(),
            split.epic_id,
            reason.map(|reason| format!(": {reason}")).unwrap_or_default()
        );
        let delivery = send_to_split_peer(
            cas_root,
            caller,
            &split,
            &body,
            &format!("rejected epic split {id}"),
        );
        return Ok(match delivery {
            Ok(delivery) => format!(
                "Rejected epic split {id}; told {}@{} ({}).",
                split.peer_name,
                split.peer_machine,
                delivery.describe()
            ),
            Err(error) => format!(
                "Rejected epic split {id} here, but {}@{} could not be told ({error}); their proposal lapses at {}.",
                split.peer_name,
                split.peer_machine,
                split.expires_at.format("%H:%M UTC")
            ),
        });
    }

    if let Some((other, child)) = overlapping(cas_root, &[split.mine.clone(), split.theirs.clone()].concat(), Some(id))? {
        return Err(overlap_refusal(cas_root, &other, &child));
    }
    let claims = peer_claims(cas_root)?;
    let is_local = local_check(cas_root)?;
    let duration = crate::cloud::peer_claims::lease_duration_secs(cas_root);
    let warnings = claim_share(&claims, &split.mine, caller, duration, &is_local)
        .map_err(|failure| format!("epic split {id} not accepted, and nothing claimed: {failure}"))?;
    let committed_at = Utc::now();
    let changed = decide_epic_split_by(cas_root, id, &[SplitState::Pending], SplitState::Accepted, None, committed_at, Some(&caller.id))
        .map_err(|error| {
            release_share(&claims, &split.mine, &caller.id);
            store_error(error)
        })?;
    if !changed {
        release_share(&claims, &split.mine, &caller.id);
        return Err(format!("{id} changed while accepting it; check `epic_split`"));
    }
    let mut acceptance = line(SplitKind::Accept);
    acceptance.at = committed_at;
    let body = format!(
        "{}\nAccepted epic split {id} of {}: I take {}; you keep {}.",
        acceptance.render(),
        split.epic_id,
        share_label(&split.mine),
        share_label(&split.theirs),
    );
    match send_to_split_peer(
        cas_root,
        caller,
        &split,
        &body,
        &format!("accepted epic split {id}"),
    ) {
        Ok(delivery) => {
            let mut out = format!(
                "Accepted epic split {id} of {} with {}@{} ({}).\n\
                 You took (claimed in Cassy Cloud): {}\n\
                 {} keeps: {} — starting those here is refused.",
                split.epic_id,
                split.peer_name,
                split.peer_machine,
                delivery.describe(),
                share_label(&split.mine),
                split.peer_name,
                share_label(&split.theirs),
            );
            for warning in warnings {
                out.push_str(&format!("\n⚠️  {warning}"));
            }
            Ok(out)
        }
        Err(error) => {
            // The proposer never heard; undo, so the two sides agree.
            let _ = decide_epic_split(cas_root, id, &[SplitState::Accepted], SplitState::Pending, None, Utc::now());
            release_share(&claims, &split.mine, &caller.id);
            Err(format!(
                "epic split {id} not accepted (claims released): could not reach {}@{}: {error}",
                split.peer_name, split.peer_machine
            ))
        }
    }
}

/// Cancel split `id`: normally release this side's supervisor claims and tell
/// the peer. A live supervisor may also supersede an accepted split or an
/// expired outgoing proposal once its registered owner is proven gone past
/// the heartbeat TTL. The override records its actor and reason on the split.
pub fn cancel(
    cas_root: &Path,
    caller: &Caller,
    id: &str,
    reason: Option<&str>,
) -> Result<String, String> {
    let store_error = |error: cas_store::StoreError| format!("epic split store: {error}");
    let split = get_epic_split(cas_root, id)
        .map_err(store_error)?
        .ok_or_else(|| format!("no epic split {id} recorded here"))?;
    let superseded_owner = if split.local_agent_id == caller.id {
        None
    } else {
        let expired_proposal = split.direction == SplitDirection::Outgoing && split.is_expired(Utc::now());
        if split.state != SplitState::Accepted && !expired_proposal {
            return Err(format!("{id} belongs to another supervisor of this database"));
        }
        let agents = crate::store::open_agent_store(cas_root)
            .map_err(|error| format!("could not open the agent store: {error}"))?;
        let registered_caller = agents
            .get(&caller.id)
            .map_err(|_| format!("{id} can only be superseded by a registered supervisor"))?;
        if registered_caller.role != crate::types::AgentRole::Supervisor
            || !crate::mcp::tools::service::agent_liveness::evaluate_supervision_liveness(
                &registered_caller,
            )
            .is_live()
        {
            return Err(format!(
                "{id} can only be superseded by a live registered supervisor of this project"
            ));
        }
        let owner = agents.get(&split.local_agent_id).map_err(|_| {
            format!(
                "{id} still belongs to supervisor {}; its liveness cannot be verified because it is not registered here",
                split.local_agent_id
            )
        })?;
        if owner.role != crate::types::AgentRole::Supervisor {
            return Err(format!(
                "{id} belongs to registered agent {}, which is not a supervisor",
                owner.id
            ));
        }
        let heartbeat_age =
            crate::mcp::tools::service::agent_liveness::agent_heartbeat_age_secs(&owner);
        let owner_live = crate::mcp::tools::service::agent_liveness::
            evaluate_supervision_liveness(&owner)
            .is_live();
        if owner_live {
            return Err(format!(
                "{id} belongs to live supervisor {} ({}); only that supervisor may cancel it",
                owner.name, owner.id
            ));
        }
        let stale_secs = crate::mcp::tools::service::agent_liveness::WORKER_STALE_SECS;
        if heartbeat_age <= stale_secs {
            return Err(format!(
                "{id} cannot be superseded yet: supervisor {}'s heartbeat is {heartbeat_age}s old; it must exceed the {stale_secs}s stale threshold",
                owner.name
            ));
        }
        Some(owner)
    };
    match (split.state, split.direction) {
        (SplitState::Pending, SplitDirection::Incoming) => {
            return Err(format!(
                "{id} is {}'s unanswered proposal; decline it with decision=reject",
                split.peer_name
            ));
        }
        (SplitState::Rejected | SplitState::Cancelled | SplitState::Expired, _) => {
            return Err(format!("{id} is already {}", split.state.as_str()));
        }
        _ => {}
    }
    let supersede_reason = superseded_owner.as_ref().map(|owner| {
        let mut audit = format!(
            "superseded by live supervisor {} ({}) because original supervisor {} ({}) is not live; heartbeat age {}s exceeds the {}s stale threshold",
            caller.name,
            caller.id,
            owner.name,
            owner.id,
            crate::mcp::tools::service::agent_liveness::agent_heartbeat_age_secs(owner),
            crate::mcp::tools::service::agent_liveness::WORKER_STALE_SECS,
        );
        if let Some(reason) = reason {
            audit.push_str(&format!("; reason: {reason}"));
        }
        audit
    });
    let cancellation_reason = supersede_reason.as_deref().or(reason);
    let now = Utc::now();
    let changed = decide_epic_split_by(
        cas_root,
        id,
        &[SplitState::Pending, SplitState::Accepted],
        SplitState::Cancelled,
        cancellation_reason,
        now,
        Some(&caller.id),
    )
    .map_err(store_error)?;
    if !changed {
        return Err(format!("{id} changed while cancelling it; check `epic_split`"));
    }
    let claim_owner_id = superseded_owner
        .as_ref()
        .map(|owner| owner.id.as_str())
        .unwrap_or(&caller.id);
    let released = peer_claims(cas_root)
        .map(|claims| release_share(&claims, &split.mine, claim_owner_id))
        .unwrap_or(0);
    let line = SplitMessage {
        kind: SplitKind::Cancel,
        id: id.to_string(),
        epic: split.epic_id.clone(),
        proposer_takes: Vec::new(),
        peer_takes: Vec::new(),
        expires_at: None,
        at: now,
        reason: cancellation_reason.map(str::to_string),
    };
    let body = format!(
        "{}\nCancelled epic split {id} of {}{}.",
        line.render(),
        split.epic_id,
        cancellation_reason
            .map(|reason| format!(": {reason}"))
            .unwrap_or_default()
    );
    let told = match send_to_split_peer(
        cas_root,
        caller,
        &split,
        &body,
        &format!("cancelled epic split {id}"),
    ) {
        Ok(delivery) => format!(
            "told {}@{} ({})",
            split.peer_name,
            split.peer_machine,
            delivery.describe()
        ),
        Err(error) => format!(
            "but {}@{} could not be told ({error}); message them, or their side still refuses your share",
            split.peer_name, split.peer_machine
        ),
    };
    let release_detail = match superseded_owner {
        Some(owner) => format!(
            "released {released} claim(s) formerly held by gone supervisor {} ({})",
            owner.name, owner.id
        ),
        None => format!("released {released} of your own claim(s)"),
    };
    Ok(format!(
        "Cancelled epic split {id} of {}; {release_detail}; {told}.",
        split.epic_id
    ))
}

/// `text` word-wrapped to the peers view's width, the first line indented
/// by `indent` spaces and continuations by `indent + 2`.
fn wrap(text: &str, indent: usize) -> String {
    let width = crate::cloud::presence::WRAP_COLUMNS;
    let mut out = String::new();
    let mut line = " ".repeat(indent);
    let mut empty = true;
    for word in text.split_whitespace() {
        if !empty && line.chars().count() + 1 + word.chars().count() > width {
            out.push_str(&line);
            out.push('\n');
            line = " ".repeat(indent + 2);
            empty = true;
        }
        if !empty {
            line.push(' ');
        }
        line.push_str(word);
        empty = false;
    }
    if !empty {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// The splits recorded here, for `epic_split` and the peers view (after the
/// peers, before the mailbox footer), within the view's 72 columns. Empty
/// when there are none.
pub fn render_splits(cas_root: &Path) -> String {
    if !has_store(cas_root) {
        return String::new();
    }
    let Ok(splits) = list_epic_splits(cas_root) else {
        return String::new();
    };
    render_split_list(&splits, Utc::now())
}

fn render_split_list(splits: &[EpicSplit], now: DateTime<Utc>) -> String {
    if splits.is_empty() {
        return String::new();
    }
    let mut out = String::from("Epic splits:\n");
    for split in splits.iter().take(20) {
        out.push_str(&wrap(
            &format!(
                "{} [{}, {}] epic {}",
                split.id,
                split.display_state(now),
                split.direction.as_str(),
                split.epic_id
            ),
            2,
        ));
        out.push_str(&wrap(
            &format!("with {}@{}", split.peer_name, split.peer_machine),
            4,
        ));
        out.push_str(&wrap(&format!("mine {}", share_label(&split.mine)), 4));
        out.push_str(&wrap(&format!("theirs {}", share_label(&split.theirs)), 4));
        let when = match (split.state, split.decided_at) {
            (SplitState::Pending, _) if !split.is_expired(now) => {
                Some(format!("expires {}", split.expires_at.format("%H:%M UTC")))
            }
            (_, Some(at)) => Some(format!("decided {}", at.format("%m-%d %H:%M UTC"))),
            _ => None,
        };
        let reason = split
            .reason
            .as_deref()
            .map(|reason| crate::mcp::tools::truncate_str(reason, 120));
        let tail = [when, reason].into_iter().flatten().collect::<Vec<_>>();
        if !tail.is_empty() {
            out.push_str(&wrap(&tail.join(" | "), 4));
        }
    }
    out
}

/// Renew the cloud claims `agent_id` still holds on its share of every
/// binding split it is party to. Renew only: a child a worker took keeps the
/// worker's claim, and a lapsed claim is not re-acquired.
pub fn renew_split_claims(cas_root: &Path, claims: &PeerClaims, agent_id: &str, duration: u32) -> usize {
    if !has_store(cas_root) {
        return 0;
    }
    if let Err(error) = release_obsolete_split_claims(cas_root, claims, None, None, Some(agent_id)) {
        tracing::warn!(%error, agent = %agent_id, "could not settle obsolete epic split claims");
    }
    let Ok(splits) = current_epic_splits(cas_root) else {
        return 0;
    };
    let Ok(agents) = crate::store::open_agent_store(cas_root) else {
        return 0;
    };
    let task_leases = match agents.list_renewable_task_leases(agent_id) {
        Ok(leases) => leases,
        Err(error) => {
            tracing::warn!(%error, agent = %agent_id, "could not read task leases before split renewal");
            return 0;
        }
    };
    let task_leases: std::collections::HashSet<String> = task_leases
        .into_iter()
        .filter(|lease| lease.is_valid())
        .map(|lease| lease.task_id)
        .collect();
    let now = Utc::now();
    splits
        .iter()
        .filter(|split| split.local_agent_id == agent_id && split.is_binding(now))
        .flat_map(|split| split.mine.iter())
        .filter(|id| !task_leases.contains(*id))
        .filter(|id| claims.holder(id).ok().flatten().as_deref() == Some(agent_id))
        .filter(|id| claims.renew(id, agent_id, duration).is_ok())
        .count()
}

/// Release obsolete split-derived Cloud claims after a task's parent changed.
/// Both shares are considered because a database may know the moved child as
/// the peer's share; release is still fenced to the recorded owner in Cloud.
pub(crate) fn release_obsolete_claims_for_task(
    cas_root: &Path,
    task_id: &str,
) -> Result<usize, String> {
    let Some(claims) = PeerClaims::for_project(cas_root) else {
        return Ok(0);
    };
    release_obsolete_split_claims(cas_root, &claims, Some(task_id), None, None)
}

/// The split owner can settle one moved child without cancelling the rest of
/// the split. The task must be in this supervisor's share and outside the
/// split's epic; a current alternate split or task lease keeps its Cloud claim.
pub fn release_moved_child(
    cas_root: &Path,
    caller: &Caller,
    split_id: &str,
    task_id: &str,
) -> Result<String, String> {
    let split = get_epic_split(cas_root, split_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("no epic split {split_id} recorded here"))?;
    if split.local_agent_id != caller.id {
        return Err(format!("only split owner {} can release its child claim", split.local_agent_id));
    }
    if !split.mine.iter().any(|id| id == task_id) {
        return Err(format!("{task_id} is not in your share of epic split {split_id}"));
    }
    let tasks = crate::store::open_task_store_local(cas_root)
        .map_err(|error| format!("could not open the task store: {error}"))?;
    if tasks.get_parent_epic(task_id).map_err(|error| error.to_string())?
        .is_some_and(|parent| parent.id == split.epic_id)
    {
        return Err(format!("{task_id} is still a child of epic {}; move it before releasing this split claim", split.epic_id));
    }
    let claims = peer_claims(cas_root)?;
    let released = release_obsolete_split_claims(
        cas_root, &claims, Some(task_id), Some(split_id), Some(&caller.id),
    )?;
    Ok(format!(
        "Released {released} obsolete split-derived Cloud claim(s) for {task_id} from epic split {split_id}; the split owner's share is marked released."
    ))
}

fn split_claim_owner<'a>(split: &'a EpicSplit, task_id: &str) -> Option<&'a str> {
    if split.mine.iter().any(|id| id == task_id) {
        Some(&split.local_agent_id)
    } else if split.theirs.iter().any(|id| id == task_id) {
        Some(&split.peer_agent_id)
    } else {
        None
    }
}

/// Release only when the task is no longer justified by this split. A newer
/// binding split for the same owner, or that owner's live task lease, keeps
/// the shared Cloud claim. Renewal calls scope candidates to its own `mine`.
fn release_obsolete_split_claims(
    cas_root: &Path,
    claims: &PeerClaims,
    only_task: Option<&str>,
    only_split: Option<&str>,
    renewing_owner: Option<&str>,
) -> Result<usize, String> {
    let splits = list_epic_splits(cas_root).map_err(|error| format!("epic split store: {error}"))?;
    if splits.is_empty() {
        return Ok(0);
    }
    let released: std::collections::HashSet<(String, String)> =
        list_epic_split_child_releases(cas_root)
            .map_err(|error| format!("epic split release store: {error}"))?
            .into_iter()
            .collect();
    let tasks = crate::store::open_task_store_local(cas_root)
        .map_err(|error| format!("could not open the task store: {error}"))?;
    let agents = crate::store::open_agent_store(cas_root)
        .map_err(|error| format!("could not open the agent store: {error}"))?;
    let now = Utc::now();
    let mut cloud_released = 0;
    let mut errors = Vec::new();

    for split in &splits {
        if only_split.is_some_and(|id| id != split.id) {
            continue;
        }
        if renewing_owner.is_some_and(|owner| split.local_agent_id != owner) {
            continue;
        }
        let candidates: Vec<(&String, &str)> = if renewing_owner.is_some() {
            split.mine.iter().map(|task| (task, split.local_agent_id.as_str())).collect()
        } else {
            split.mine.iter().map(|task| (task, split.local_agent_id.as_str()))
                .chain(split.theirs.iter().map(|task| (task, split.peer_agent_id.as_str())))
                .collect()
        };
        for (task_id, owner_id) in candidates {
            if only_task.is_some_and(|id| id != task_id)
                || released.contains(&(split.id.clone(), task_id.clone()))
            {
                continue;
            }
            let parent = tasks.get_parent_epic(task_id)
                .map_err(|error| format!("could not read split child parent for {task_id}: {error}"))?
                .map(|epic| epic.id);
            // A missing parent in a background scan may mean partial sync. The
            // task-update path is explicit and may settle a removed parent.
            let moved = match parent.as_deref() {
                Some(parent) => parent != split.epic_id,
                None => only_task.is_some(),
            };
            if split.is_binding(now) && !moved {
                continue;
            }

            let justified_elsewhere = parent.as_deref().is_some_and(|parent| {
                splits.iter().any(|other| {
                    other.id != split.id
                        && other.is_binding(now)
                        && other.epic_id == parent
                        && split_claim_owner(other, task_id) == Some(owner_id)
                        && !released.contains(&(other.id.clone(), task_id.clone()))
                })
            });
            let has_owner_lease = agents.get_lease(task_id)
                .map_err(|error| format!("could not inspect task lease for {task_id}: {error}"))?
                .is_some_and(|lease| lease.is_valid() && lease.agent_id == owner_id);

            if !justified_elsewhere && !has_owner_lease {
                match claims.release_own(task_id, owner_id) {
                    Ok(true) => cloud_released += 1,
                    Ok(false) => {}
                    Err(error) => {
                        errors.push(format!("{task_id} ({owner_id}): {error}"));
                        continue;
                    }
                }
            }
            if let Err(error) = mark_epic_split_child_released(cas_root, &split.id, task_id, now) {
                errors.push(format!("{task_id} ({}): could not mark split share released: {error}", split.id));
            }
        }
    }
    if errors.is_empty() {
        Ok(cloud_released)
    } else {
        Err(format!("split claim cleanup failed for {}", errors.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_supervisor(root: &Path, id: &str, name: &str, heartbeat_age_secs: i64) -> Agent {
        let mut agent = Agent::new_with_role(
            id.to_string(),
            name.to_string(),
            crate::types::AgentRole::Supervisor,
        );
        agent.last_heartbeat = Utc::now() - chrono::Duration::seconds(heartbeat_age_secs);
        crate::store::open_agent_store(root)
            .unwrap()
            .register(&agent)
            .unwrap();
        agent
    }

    async fn claim_fixture(
        root: &Path,
        task_id: &str,
        owner_id: &str,
    ) -> (
        wiremock::MockServer,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let project = "github.com/acme/split-cleanup";
        std::fs::write(
            root.join("config.toml"),
            format!("[project]\ncanonical_id = {project:?}\n"),
        )
        .unwrap();
        let server = MockServer::start().await;
        crate::cloud::CloudConfig {
            endpoint: server.uri(),
            token: Some("fixture-token".into()),
            ..Default::default()
        }
        .save_to_cas_dir(root)
        .unwrap();
        let key = crate::cloud::peer_claims::claim_key(project, task_id);
        let lock = crate::types::TaskLease::new(key.clone(), owner_id.to_string(), 600);
        let renew_lock = lock.clone();
        Mock::given(method("GET"))
            .and(path(format!("/api/agents/tasks/{key}/lock")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "lock": lock,
            })))
            .mount(&server)
            .await;
        let renewals = Arc::new(AtomicUsize::new(0));
        let seen_renewals = renewals.clone();
        Mock::given(method("POST"))
            .and(path(format!("/api/agents/tasks/{key}/renew")))
            .respond_with(move |_: &wiremock::Request| {
                seen_renewals.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"lock": renew_lock}))
            })
            .mount(&server)
            .await;
        let releases = Arc::new(AtomicUsize::new(0));
        let seen = releases.clone();
        Mock::given(method("POST"))
            .and(path(format!("/api/agents/tasks/{key}/release")))
            .respond_with(move |_: &wiremock::Request| {
                seen.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "status": "success" }))
            })
            .mount(&server)
            .await;
        (server, releases, renewals)
    }

    fn split_task_graph(root: &Path, current_parent: &str) {
        let tasks = crate::store::open_task_store(root).unwrap();
        tasks.init().unwrap();
        for (id, title) in [("cas-8503-old", "Old epic"), ("cas-8503-new", "New epic")] {
            let mut epic = crate::types::Task::new(id.into(), title.into());
            epic.task_type = TaskType::Epic;
            tasks.add(&epic).unwrap();
        }
        let child = crate::types::Task::new("cas-8503-child".into(), "Moved child".into());
        tasks.add(&child).unwrap();
        tasks.add_dependency(&crate::types::Dependency::new(
            child.id.clone(), current_parent.to_string(), DependencyType::ParentChild,
        )).unwrap();
    }

    fn split_for_child(
        id: &str,
        epic_id: &str,
        owner_id: &str,
        side: &str,
        state: SplitState,
    ) -> EpicSplit {
        let now = Utc::now();
        EpicSplit {
            id: id.into(),
            epic_id: epic_id.into(),
            local_agent_id: if side == "mine" { owner_id } else { "local-peer" }.into(),
            peer_agent_id: if side == "mine" { "remote-peer" } else { owner_id }.into(),
            peer_name: "peer".into(),
            peer_machine: "other-host".into(),
            direction: SplitDirection::Outgoing,
            mine: if side == "mine" { vec!["cas-8503-child".into()] } else { vec![] },
            theirs: if side == "theirs" { vec!["cas-8503-child".into()] } else { vec![] },
            state,
            reason: None,
            message_id: None,
            expires_at: now + chrono::Duration::hours(1),
            created_at: now,
            decided_at: Some(now),
            decided_by: Some(owner_id.into()),
            cleanup_pending: false,
            expiry_notice_pending: false,
        }
    }

    fn insert_child_split(root: &Path, split: &EpicSplit) {
        insert_epic_split(root, split).unwrap();
    }

    #[tokio::test]
    async fn reproposal_by_the_same_owner_keeps_the_live_claim() {
        use std::sync::atomic::Ordering;

        let dir = tempfile::TempDir::new().unwrap();
        split_task_graph(dir.path(), "cas-8503-old");
        let (_server, releases, renewals) = claim_fixture(dir.path(), "cas-8503-child", "split-owner").await;
        insert_child_split(dir.path(), &split_for_child(
            "split-old", "cas-8503-old", "split-owner", "mine", SplitState::Cancelled,
        ));
        insert_child_split(dir.path(), &split_for_child(
            "split-new", "cas-8503-old", "split-owner", "mine", SplitState::Accepted,
        ));

        let claims = PeerClaims::for_project(dir.path()).unwrap();
        assert_eq!(renew_split_claims(dir.path(), &claims, "split-owner", 600), 1);
        assert_eq!(releases.load(Ordering::SeqCst), 0, "the replacement split still justifies the claim");
        assert_eq!(renewals.load(Ordering::SeqCst), 1);
        assert!(list_epic_split_child_releases(dir.path()).unwrap().contains(&("split-old".into(), "cas-8503-child".into())));
    }

    #[tokio::test]
    async fn resplit_after_retarget_keeps_the_same_owners_claim() {
        use std::sync::atomic::Ordering;

        let dir = tempfile::TempDir::new().unwrap();
        split_task_graph(dir.path(), "cas-8503-new");
        let (_server, releases, _renewals) = claim_fixture(dir.path(), "cas-8503-child", "split-owner").await;
        insert_child_split(dir.path(), &split_for_child(
            "split-old", "cas-8503-old", "split-owner", "mine", SplitState::Accepted,
        ));
        insert_child_split(dir.path(), &split_for_child(
            "split-new", "cas-8503-new", "split-owner", "theirs", SplitState::Accepted,
        ));

        let claims = PeerClaims::for_project(dir.path()).unwrap();
        assert_eq!(release_obsolete_split_claims(dir.path(), &claims, None, None, Some("split-owner")).unwrap(), 0);
        assert_eq!(releases.load(Ordering::SeqCst), 0, "the new-epic split still justifies the claim");
        assert!(list_epic_split_child_releases(dir.path()).unwrap().contains(&("split-old".into(), "cas-8503-child".into())));
        assert!(split_hold_for(dir.path(), "cas-8503-child").is_some(), "the current peer share remains an admission hold");
    }

    #[tokio::test]
    async fn supervisors_task_lease_keeps_its_claim_when_old_split_ends() {
        use std::sync::atomic::Ordering;

        let dir = tempfile::TempDir::new().unwrap();
        split_task_graph(dir.path(), "cas-8503-old");
        let (_server, releases, renewals) = claim_fixture(dir.path(), "cas-8503-child", "split-owner").await;
        let owner = test_supervisor(dir.path(), "split-owner", "split-owner", 0);
        assert!(crate::store::open_agent_store(dir.path()).unwrap()
            .try_claim("cas-8503-child", &owner.id, 600, None).unwrap().is_success());
        insert_child_split(dir.path(), &split_for_child(
            "split-old", "cas-8503-old", "split-owner", "mine", SplitState::Accepted,
        ));

        let claims = PeerClaims::for_project(dir.path()).unwrap();
        assert_eq!(renew_split_claims(dir.path(), &claims, "split-owner", 600), 0);
        assert_eq!(releases.load(Ordering::SeqCst), 0, "the task lease, not the old split, owns this claim");
        assert_eq!(renewals.load(Ordering::SeqCst), 0, "the worker lease already renews this claim");
        assert!(!list_epic_split_child_releases(dir.path()).unwrap().contains(&("split-old".into(), "cas-8503-child".into())));
    }

    #[tokio::test]
    async fn a_moved_peer_share_releases_the_remote_owners_claim() {
        use std::sync::atomic::Ordering;

        let dir = tempfile::TempDir::new().unwrap();
        split_task_graph(dir.path(), "cas-8503-new");
        let (_server, releases, _renewals) = claim_fixture(dir.path(), "cas-8503-child", "remote-owner").await;
        insert_child_split(dir.path(), &split_for_child(
            "split-remote", "cas-8503-old", "remote-owner", "theirs", SplitState::Accepted,
        ));

        let released = release_obsolete_claims_for_task(dir.path(), "cas-8503-child").unwrap();
        assert_eq!(released, 1);
        assert_eq!(releases.load(Ordering::SeqCst), 1, "Cloud must release the peer-side owner too");
        assert!(list_epic_split_child_releases(dir.path()).unwrap().contains(&("split-remote".into(), "cas-8503-child".into())));
    }

    #[tokio::test]
    async fn split_owner_can_release_one_moved_child_without_cancelling_the_split() {
        let dir = tempfile::TempDir::new().unwrap();
        split_task_graph(dir.path(), "cas-8503-new");
        let (_server, releases, _renewals) = claim_fixture(dir.path(), "cas-8503-child", "split-owner").await;
        let owner = test_supervisor(dir.path(), "split-owner", "split-owner", 0);
        insert_child_split(dir.path(), &split_for_child(
            "split-owner", "cas-8503-old", &owner.id, "mine", SplitState::Accepted,
        ));

        let result = release_moved_child(
            dir.path(), &Caller { id: owner.id.clone(), name: owner.name.clone() },
            "split-owner", "cas-8503-child",
        ).unwrap();
        assert!(result.contains("marked released"), "{result}");
        assert_eq!(releases.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(get_epic_split(dir.path(), "split-owner").unwrap().unwrap().state, SplitState::Accepted);
    }

    fn accepted_split(root: &Path, owner_id: &str) {
        let now = Utc::now();
        insert_epic_split(
            root,
            &EpicSplit {
                id: "split-2c91dc59".into(),
                epic_id: "cas-5c13".into(),
                local_agent_id: owner_id.into(),
                peer_agent_id: "true-leopard-45".into(),
                peer_name: "true-leopard-45".into(),
                peer_machine: "soundwave".into(),
                direction: SplitDirection::Outgoing,
                mine: vec!["cas-b77a".into()],
                theirs: vec!["cas-bdfc".into()],
                state: SplitState::Accepted,
                reason: None,
                message_id: None,
                expires_at: now + chrono::Duration::days(1),
                created_at: now,
                decided_at: Some(now),
                decided_by: None,
                cleanup_pending: false,
                expiry_notice_pending: false,
            },
        )
        .unwrap();
    }

    #[test]
    fn a_live_supervisor_can_release_an_accepted_split_of_a_gone_supervisor() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let owner = test_supervisor(root, "daring-leopard-21", "gone-owner-probe-cas-59cf", 120);
        let caller = test_supervisor(root, "swift-heron-11", "swift-heron-11", 0);
        accepted_split(root, &owner.id);

        let result = cancel(
            root,
            &Caller {
                id: caller.id,
                name: caller.name,
            },
            "split-2c91dc59",
            Some("epic cas-5c13 is closed; release cas-bdfc"),
        )
        .unwrap();

        assert!(result.contains("released"), "{result}");
        assert!(result.contains("daring-leopard-21"), "{result}");
        let split = get_epic_split(root, "split-2c91dc59").unwrap().unwrap();
        assert_eq!(split.state, SplitState::Cancelled);
        let audit = split.reason.unwrap();
        assert!(audit.contains("swift-heron-11"), "{audit}");
        assert!(audit.contains("daring-leopard-21"), "{audit}");
        assert!(audit.contains("epic cas-5c13 is closed"), "{audit}");
        assert!(split_hold_for(root, "cas-bdfc").is_none());
    }

    #[test]
    fn a_live_supervisor_cannot_release_another_live_supervisors_split() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let owner = test_supervisor(root, "daring-leopard-21", "daring-leopard-21", 0);
        let caller = test_supervisor(root, "swift-heron-11", "swift-heron-11", 0);
        accepted_split(root, &owner.id);

        let error = cancel(
            root,
            &Caller {
                id: caller.id,
                name: caller.name,
            },
            "split-2c91dc59",
            Some("attempted takeover"),
        )
        .unwrap_err();

        assert!(error.contains("live supervisor"), "{error}");
        let split = get_epic_split(root, "split-2c91dc59").unwrap().unwrap();
        assert_eq!(split.state, SplitState::Accepted);
        assert_eq!(split.reason, None);
        assert!(split_hold_for(root, "cas-bdfc").is_some());
    }

    #[test]
    fn the_split_line_round_trips_and_is_read_only_from_the_first_line() {
        let line = SplitMessage {
            kind: SplitKind::Proposal,
            id: "split-1".into(),
            epic: "cas-e1".into(),
            proposer_takes: vec!["cas-1".into(), "cas-2".into()],
            peer_takes: vec!["cas-3".into()],
            expires_at: Some(Utc::now()),
            at: Utc::now(),
            reason: Some("needs \"quotes\" <and> 100%".into()),
        };
        let rendered = line.render();
        let parsed = SplitMessage::parse(&format!("{rendered}\nprose")).unwrap();
        assert_eq!(parsed.id, line.id);
        assert_eq!(parsed.proposer_takes, line.proposer_takes);
        assert_eq!(parsed.peer_takes, line.peer_takes);
        assert_eq!(parsed.reason, line.reason);
        assert_eq!(parsed.kind, SplitKind::Proposal);
        assert!(SplitMessage::parse(&format!("hello\n{rendered}")).is_none());
        assert!(SplitMessage::parse("<cas-epic-split v=\"2\" kind=\"accept\" id=\"x\"/>").is_none());
    }

    #[test]
    fn the_split_list_fits_the_peers_view_width() {
        let now = Utc::now();
        let many: Vec<String> = (0..12).map(|i| format!("cas-{i:04x}")).collect();
        let split = EpicSplit {
            id: "split-0a1b2c3d".into(),
            epic_id: "cas-e100".into(),
            local_agent_id: "a".into(),
            peer_agent_id: "b".into(),
            peer_name: "a-rather-long-supervisor-name".into(),
            peer_machine: "a-long-hostname.example.internal".into(),
            direction: SplitDirection::Outgoing,
            mine: many.clone(),
            theirs: many,
            state: SplitState::Cancelled,
            reason: Some("the operator reassigned this supervisor to another epic for the rest of the week".into()),
            message_id: None,
            expires_at: now,
            created_at: now,
            decided_at: Some(now),
            decided_by: None,
            cleanup_pending: false,
            expiry_notice_pending: false,
        };
        let text = render_split_list(&[split], now);
        assert!(text.starts_with("Epic splits:\n  split-0a1b2c3d [cancelled, outgoing] epic cas-e100\n"), "{text}");
        for line in text.lines() {
            assert!(line.chars().count() <= 72, "{line:?} is {} columns", line.chars().count());
        }
        assert!(text.contains("    mine cas-0000,"), "{text}");
        assert!(text.contains("    theirs cas-0000,"), "{text}");
        assert!(render_split_list(&[], now).is_empty());
    }

    #[test]
    fn ids_are_trimmed_and_deduplicated() {
        assert_eq!(parse_ids(" cas-1, cas-2 cas-1,,"), vec!["cas-1", "cas-2"]);
        assert!(parse_ids("").is_empty());
    }

    fn message(sender: &str, body: String) -> ClaimedPeerMessage {
        ClaimedPeerMessage {
            id: "pm_1".into(),
            project_id: "github.com/acme/widgets".into(),
            recipient_agent_id: "a".into(),
            sender_agent_id: sender.into(),
            sender_name: format!("sup-{sender}"),
            sender_machine_id: Some(format!("machine-{sender}")),
            sender_label: None,
            sender_session: None,
            body,
            summary: None,
            in_reply_to: None,
            created_at: Utc::now().to_rfc3339(),
            attempts: 1,
            scope: Default::default(),
        }
    }

    /// cas-604d: only the proposal's own peer can answer or cancel it, and
    /// only the proposer's side records an answer.
    #[test]
    fn a_third_party_cannot_answer_or_cancel_a_split() {
        let dir = tempfile::TempDir::new().unwrap();
        let now = Utc::now();
        insert_epic_split(
            dir.path(),
            &EpicSplit {
                id: "split-1".into(),
                epic_id: "cas-e1".into(),
                local_agent_id: "a".into(),
                peer_agent_id: "b".into(),
                peer_name: "sup-b".into(),
                peer_machine: "machine-b".into(),
                direction: SplitDirection::Outgoing,
                mine: vec!["cas-1".into()],
                theirs: vec!["cas-2".into()],
                state: SplitState::Pending,
                reason: None,
                message_id: None,
                expires_at: now + chrono::Duration::minutes(30),
                created_at: now,
                decided_at: None,
                decided_by: None,
                cleanup_pending: false,
                expiry_notice_pending: false,
            },
        )
        .unwrap();
        let recipient = Agent::new("a".into(), "sup-a".into());
        let answer = |kind| SplitMessage {
            kind,
            id: "split-1".into(),
            epic: "cas-e1".into(),
            proposer_takes: vec![],
            peer_takes: vec![],
            expires_at: None,
            at: Utc::now(),
            reason: None,
        }
        .render();
        for kind in [SplitKind::Accept, SplitKind::Cancel] {
            let forged = ingest(dir.path(), &recipient, &message("c", answer(kind))).unwrap();
            assert!(matches!(forged, Ingested::Ignored(_)), "{forged:?}");
        }
        // A forged proposal reusing the id cannot rebind it to another peer.
        let reused = SplitMessage {
            kind: SplitKind::Proposal,
            id: "split-1".into(),
            epic: "cas-e1".into(),
            proposer_takes: vec!["cas-2".into()],
            peer_takes: vec!["cas-1".into()],
            expires_at: Some(now + chrono::Duration::minutes(5)),
            at: now,
            reason: None,
        };
        let forged = ingest(dir.path(), &recipient, &message("c", reused.render())).unwrap();
        assert!(matches!(forged, Ingested::Ignored(_)), "{forged:?}");
        assert_eq!(
            get_epic_split(dir.path(), "split-1").unwrap().unwrap().state,
            SplitState::Pending
        );
        let real = ingest(dir.path(), &recipient, &message("b", answer(SplitKind::Accept))).unwrap();
        assert_eq!(real, Ingested::Decided { id: "split-1".into(), state: SplitState::Accepted });
        assert_eq!(
            ingest(dir.path(), &recipient, &message("b", "plain words".into())).unwrap(),
            Ingested::NotASplit
        );
    }
}
