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
//! child the other side took is refused unless forced. The gate reads only
//! this database, so it holds even when the cloud is unreachable.
//!
//! Authority (cas-604d): a split line is honoured only in a message the cloud
//! delivered to a local supervisor, whose sender identity the cloud stamped;
//! an answer or cancel changes only a split whose peer is that sender. No
//! side ever claims, releases or acts for the other.

use std::path::Path;

use chrono::{DateTime, Utc};

use cas_store::{
    EpicSplit, SplitDirection, SplitState, decide_epic_split, get_epic_split, insert_epic_split,
    list_epic_splits, open_epic_splits,
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
}

impl SplitKind {
    fn as_str(self) -> &'static str {
        match self {
            SplitKind::Proposal => "proposal",
            SplitKind::Accept => "accept",
            SplitKind::Reject => "reject",
            SplitKind::Cancel => "cancel",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "proposal" => Some(SplitKind::Proposal),
            "accept" => Some(SplitKind::Accept),
            "reject" => Some(SplitKind::Reject),
            "cancel" => Some(SplitKind::Cancel),
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
             To take this child anyway: {prefix}task action=start id={task_id} force=true",
            self.describe(),
            self.split_id,
            self.epic_id,
            self.state,
            self.describe(),
            self.split_id,
        )
    }

    /// Note for a forced start of the peer's child.
    pub fn override_note(&self, task_id: &str, prefix: &str) -> String {
        format!(
            "\n\n⚠️  SPLIT OVERRIDE — {task_id} belongs to {} under epic split {}; started anyway (force=true). Tell them: {prefix}coordination action=message target={} summary=\"took {task_id}\" message=\"...\"",
            self.describe(),
            self.split_id,
            self.describe(),
        )
    }
}

/// Read paths never create a database where there is none.
fn has_store(cas_root: &Path) -> bool {
    cas_root.join("cas.db").exists()
}

/// The binding split, if any, under which `task_id` is the peer's.
pub fn split_hold_for(cas_root: &Path, task_id: &str) -> Option<SplitHold> {
    if !has_store(cas_root) {
        return None;
    }
    let splits = match open_epic_splits(cas_root) {
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
/// supervisor `recipient`. Idempotent. Never claims, releases or sends.
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
        if line.epic.is_empty() || line.peer_takes.is_empty() {
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
                peer_machine: message
                    .sender_machine_id
                    .clone()
                    .filter(|machine| !machine.trim().is_empty())
                    .unwrap_or_else(|| "unknown".to_string()),
                direction: SplitDirection::Incoming,
                mine: line.peer_takes.clone(),
                theirs: line.proposer_takes.clone(),
                state: SplitState::Pending,
                reason: None,
                message_id: Some(message.id.clone()),
                expires_at,
                created_at: Utc::now(),
                decided_at: None,
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
        SplitKind::Proposal => unreachable!("handled above"),
    };
    let changed = decide_epic_split(cas_root, &line.id, from, to, line.reason.as_deref(), Utc::now())
        .map_err(store_error)?;
    Ok(if changed {
        Ingested::Decided { id: line.id, state: to }
    } else {
        Ingested::Ignored(format!("split {} is already {}", line.id, split.state.as_str()))
    })
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
            Acquire::StalePeer(hold) => warnings.push(format!(
                "{id}: {}'s claim {}; Cassy Cloud still lists them until it expires the claim",
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
                if let Err(error) = claims.release_held_by(id, &caller.id) {
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
        .filter(|id| claims.release_held_by(id, caller_id).unwrap_or(false))
        .count()
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

/// Splits that may still bind (not expired), other than `except`, sharing a
/// child with `ids`: `(split id, child)`.
fn overlapping(cas_root: &Path, ids: &[String], except: Option<&str>) -> Result<Option<(String, String)>, String> {
    let now = Utc::now();
    let splits = open_epic_splits(cas_root).map_err(|error| format!("epic split store: {error}"))?;
    Ok(splits
        .into_iter()
        .filter(|split| Some(split.id.as_str()) != except)
        .filter(|split| !split.is_expired(now))
        .find_map(|split| {
            ids.iter()
                .find(|id| split.mine.contains(id) || split.theirs.contains(id))
                .map(|id| (split.id.clone(), id.clone()))
        }))
}

/// Propose splitting `epic_id` with peer supervisor `peer_target`: claim
/// `mine` now, record the proposal, and send it.
pub fn propose(
    cas_root: &Path,
    caller: &Caller,
    epic_id: &str,
    peer_target: &str,
    mine: &[String],
    theirs: &[String],
    ttl_secs: Option<i64>,
) -> Result<String, String> {
    if mine.is_empty() || theirs.is_empty() {
        return Err("epic_split needs both shares: mine=<ids> and theirs=<ids>".to_string());
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
        return Err(format!(
            "{child} is already in epic split {split}; cancel or reject it first"
        ));
    }

    let peer = match discover_peers(cas_root, Some(&caller.id)) {
        Ok(PeerDiscovery::Found { peers, .. }) => {
            peer_mailbox::resolve_peer_target(&peers, peer_target)?
                .cloned()
                .ok_or_else(|| {
                    format!("no peer supervisor of this repository is named '{peer_target}'")
                })?
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
        mine.join(", "),
        theirs.join(", "),
        split.expires_at.format("%H:%M UTC"),
    );
    let receipt = match peer_mailbox::send_raw(
        cas_root,
        &caller.id,
        &peer.agent_id,
        &body,
        Some(&format!("epic split proposal {id} for {epic_id}")),
        None,
    ) {
        Ok(receipt) => receipt,
        Err(error) => {
            rollback("the proposal could not be sent");
            return Err(format!(
                "epic split not proposed (claims released): could not reach {}@{}: {error}",
                peer.name, peer.machine
            ));
        }
    };
    let mut out = format!(
        "Proposed epic split {id} of {epic_id} to {}@{} (peer message {}, {}).\n\
         You took (claimed in Cassy Cloud): {}\n\
         Offered to {}: {}\n\
         Until they answer, starting their share here is refused. The proposal expires at {}.",
        peer.name,
        peer.machine,
        receipt.id,
        receipt.status,
        mine.join(", "),
        peer.name,
        theirs.join(", "),
        split.expires_at.format("%H:%M UTC"),
    );
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
        let changed = decide_epic_split(cas_root, id, &[SplitState::Pending], SplitState::Rejected, reason, now)
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
        let sent = peer_mailbox::send_raw(
            cas_root,
            &caller.id,
            &split.peer_agent_id,
            &body,
            Some(&format!("rejected epic split {id}")),
            split.message_id.as_deref(),
        );
        return Ok(match sent {
            Ok(receipt) => format!(
                "Rejected epic split {id}; told {}@{} (peer message {}).",
                split.peer_name, split.peer_machine, receipt.id
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
        return Err(format!(
            "{child} is already in epic split {other}; cancel or reject that one first"
        ));
    }
    let claims = peer_claims(cas_root)?;
    let is_local = local_check(cas_root)?;
    let duration = crate::cloud::peer_claims::lease_duration_secs(cas_root);
    let warnings = claim_share(&claims, &split.mine, caller, duration, &is_local)
        .map_err(|failure| format!("epic split {id} not accepted, and nothing claimed: {failure}"))?;
    let changed = decide_epic_split(cas_root, id, &[SplitState::Pending], SplitState::Accepted, None, now)
        .map_err(|error| {
            release_share(&claims, &split.mine, &caller.id);
            store_error(error)
        })?;
    if !changed {
        release_share(&claims, &split.mine, &caller.id);
        return Err(format!("{id} changed while accepting it; check `epic_split`"));
    }
    let body = format!(
        "{}\nAccepted epic split {id} of {}: I take {}; you keep {}.",
        line(SplitKind::Accept).render(),
        split.epic_id,
        split.mine.join(", "),
        split.theirs.join(", "),
    );
    match peer_mailbox::send_raw(
        cas_root,
        &caller.id,
        &split.peer_agent_id,
        &body,
        Some(&format!("accepted epic split {id}")),
        split.message_id.as_deref(),
    ) {
        Ok(receipt) => {
            let mut out = format!(
                "Accepted epic split {id} of {} with {}@{} (peer message {}).\n\
                 You took (claimed in Cassy Cloud): {}\n\
                 {} keeps: {} — starting those here is refused.",
                split.epic_id,
                split.peer_name,
                split.peer_machine,
                receipt.id,
                split.mine.join(", "),
                split.peer_name,
                split.theirs.join(", "),
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

/// Cancel split `id` from this side: release the claims this side's
/// supervisor holds on its own share, and tell the peer.
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
    if split.local_agent_id != caller.id {
        return Err(format!("{id} belongs to another supervisor of this database"));
    }
    match (split.state, split.direction) {
        (SplitState::Pending, SplitDirection::Incoming) => {
            return Err(format!(
                "{id} is {}'s unanswered proposal; decline it with decision=reject",
                split.peer_name
            ));
        }
        (SplitState::Rejected | SplitState::Cancelled, _) => {
            return Err(format!("{id} is already {}", split.state.as_str()));
        }
        _ => {}
    }
    let now = Utc::now();
    let changed = decide_epic_split(
        cas_root,
        id,
        &[SplitState::Pending, SplitState::Accepted],
        SplitState::Cancelled,
        reason,
        now,
    )
    .map_err(store_error)?;
    if !changed {
        return Err(format!("{id} changed while cancelling it; check `epic_split`"));
    }
    let released = peer_claims(cas_root)
        .map(|claims| release_share(&claims, &split.mine, &caller.id))
        .unwrap_or(0);
    let line = SplitMessage {
        kind: SplitKind::Cancel,
        id: id.to_string(),
        epic: split.epic_id.clone(),
        proposer_takes: Vec::new(),
        peer_takes: Vec::new(),
        expires_at: None,
        at: now,
        reason: reason.map(str::to_string),
    };
    let body = format!(
        "{}\nCancelled epic split {id} of {}{}. Your claims are yours; nothing of yours was released.",
        line.render(),
        split.epic_id,
        reason.map(|reason| format!(": {reason}")).unwrap_or_default()
    );
    let told = match peer_mailbox::send_raw(
        cas_root,
        &caller.id,
        &split.peer_agent_id,
        &body,
        Some(&format!("cancelled epic split {id}")),
        split.message_id.as_deref(),
    ) {
        Ok(receipt) => format!("told {}@{} (peer message {})", split.peer_name, split.peer_machine, receipt.id),
        Err(error) => format!(
            "but {}@{} could not be told ({error}); message them, or their side still refuses your share",
            split.peer_name, split.peer_machine
        ),
    };
    Ok(format!(
        "Cancelled epic split {id} of {}; released {released} of your own claim(s); {told}.",
        split.epic_id
    ))
}

/// The splits recorded here, for `epic_split` and the peers view. Empty when
/// there are none.
pub fn render_splits(cas_root: &Path) -> String {
    if !has_store(cas_root) {
        return String::new();
    }
    let Ok(splits) = list_epic_splits(cas_root) else {
        return String::new();
    };
    if splits.is_empty() {
        return String::new();
    }
    let now = Utc::now();
    let mut out = String::from("Epic splits:\n");
    for split in splits.iter().take(20) {
        let state = split.display_state(now);
        let when = match (split.state, split.decided_at) {
            (SplitState::Pending, _) if !split.is_expired(now) => {
                format!(" | expires {}", split.expires_at.format("%H:%M UTC"))
            }
            (_, Some(at)) => format!(" | {}", at.format("%m-%d %H:%M UTC")),
            _ => String::new(),
        };
        let reason = split
            .reason
            .as_deref()
            .map(|reason| format!(" | {}", crate::mcp::tools::truncate_str(reason, 80)))
            .unwrap_or_default();
        out.push_str(&format!(
            "  {} [{state}, {}] epic {} with {}@{} | mine {} | theirs {}{when}{reason}\n",
            split.id,
            split.direction.as_str(),
            split.epic_id,
            split.peer_name,
            split.peer_machine,
            split.mine.join(","),
            split.theirs.join(","),
        ));
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
    let Ok(splits) = open_epic_splits(cas_root) else {
        return 0;
    };
    let now = Utc::now();
    splits
        .iter()
        .filter(|split| split.local_agent_id == agent_id && split.is_binding(now))
        .flat_map(|split| split.mine.iter())
        .filter(|id| claims.holder(id).ok().flatten().as_deref() == Some(agent_id))
        .filter(|id| claims.renew(id, agent_id, duration).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

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
            sender_session: None,
            body,
            summary: None,
            in_reply_to: None,
            created_at: Utc::now().to_rfc3339(),
            attempts: 1,
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
