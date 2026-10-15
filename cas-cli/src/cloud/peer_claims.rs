//! cas-5f28: cross-machine claim awareness.
//!
//! A task lease lives in one Cassy database, so two supervisors of the same
//! repository on two machines (or two clones on one machine) could both start
//! the same task. When the project is logged in to Cassy Cloud, the lease is
//! mirrored as a cloud task claim through [`CloudCoordinator`]: `task start`
//! claims it, the daemon heartbeat renews it, and every local release path
//! releases it.
//!
//! Cloud claims are scoped to the logged-in user and keyed by an opaque id.
//! The key carries the repository (`<task id>~<8 hex of the canonical id>`),
//! so the same user's tasks in two repositories never collide. The claim
//! reason carries the holder's identity as JSON, so a refused peer can say who
//! holds the task and on which machine without another request.
//!
//! The cloud is advisory infrastructure: when it is unreachable, the local
//! lease still decides and the caller is told that peers were not checked.
//!
//! cas-0078: the local lease decides between agents of one database, so a
//! cloud claim only moves between them after the local lease has a winner
//! ([`PeerClaims::take_from_local`]). Every network call made for an async
//! MCP handler runs off the runtime under one [`PeerBudget`].

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cloud::{CloudConfig, CloudCoordinator};
use crate::error::CasError;
use crate::types::{ClaimResult, TaskLease};

/// What Cassy Cloud's agent list says about a claim holder.
enum Listed {
    /// Listed, with a heartbeat inside the live window.
    Live(ClaimHolder),
    /// Explicitly shut down. Unlike late presence, this overrides renewal.
    Shutdown(ClaimHolder),
    /// Listed, but stale or silent past the live window.
    Stale(ClaimHolder),
    /// The list was read and the holder is not in it.
    Missing,
    /// The list could not be read.
    Unknown,
}

/// Bounded wait for a claim round trip, so an unreachable cloud costs a
/// start seconds, not the coordinator's default thirty.
pub const CLAIM_TIMEOUT: Duration = Duration::from_secs(5);

/// The total time one MCP request may spend asking Cassy Cloud about peers.
/// Past it, the request proceeds on the local lease and says peers were not
/// checked.
pub const PEER_BUDGET: Duration = Duration::from_secs(8);

/// Agent metadata key naming the epic whose focus this agent claims in the
/// cloud (a supervisor's `focus_epic`). Renewal keeps that claim alive;
/// clearing the focus releases it.
pub const META_FOCUS_CLAIM: &str = "cloud_focus_epic";

/// One request's bounded allowance of cloud round trips (cas-0078). Each
/// call runs on the blocking pool, so a slow cloud never stalls the async
/// runtime, and all calls of one request share one deadline.
#[derive(Debug, Clone, Copy)]
pub struct PeerBudget {
    deadline: Instant,
}

impl Default for PeerBudget {
    fn default() -> Self {
        Self::new(PEER_BUDGET)
    }
}

impl PeerBudget {
    pub fn new(total: Duration) -> Self {
        Self { deadline: Instant::now() + total }
    }

    /// Time left for another Cloud call; zero forbids starting it.
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// Run `work` off the async runtime within what is left of the budget.
    /// `Err` says why it gave no answer (the budget ran out, or it panicked).
    pub async fn run<T, F>(&self, work: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let left = self.remaining();
        if left.is_zero() {
            return Err(format!("no answer within {}s", PEER_BUDGET.as_secs()));
        }
        match tokio::time::timeout(left, tokio::task::spawn_blocking(work)).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(format!("the peer check failed: {error}")),
            Err(_) => Err(format!("no answer within {}s", PEER_BUDGET.as_secs())),
        }
    }
}

/// The ids of every agent registered in this database, whatever its status:
/// an owned stand-in for an `is_local` lookup that can cross into
/// [`PeerBudget::run`].
pub fn local_agent_ids(agents: &dyn crate::store::AgentStore) -> HashSet<String> {
    agents
        .list(None)
        .map(|agents| agents.into_iter().map(|agent| agent.id).collect())
        .unwrap_or_default()
}

/// What a claim is for. Recorded in the claim reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimKind {
    Task,
    Epic,
}

impl ClaimKind {
    fn as_str(self) -> &'static str {
        match self {
            ClaimKind::Task => "task",
            ClaimKind::Epic => "epic",
        }
    }
}

/// The holder identity carried in a claim's reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimHolder {
    /// Wire version of this record.
    pub v: u8,
    pub kind: String,
    /// Agent name, e.g. a supervisor's codename.
    pub name: String,
    /// Host name of the machine running that agent.
    pub machine: String,
    /// Canonical repository id the claim belongs to.
    pub project: String,
}

impl ClaimHolder {
    pub fn new(kind: ClaimKind, name: &str, project: &str) -> Self {
        Self {
            v: 1,
            kind: kind.as_str().to_string(),
            name: name.to_string(),
            // cas-f707: the label discovery shows for this machine too.
            machine: crate::cloud::peers::machine_label(),
            project: project.to_string(),
        }
    }

    /// The holder on the machine labelled `machine` (a project's own
    /// label, [`crate::cloud::peers::machine_label_for`]).
    pub fn on(mut self, machine: &str) -> Self {
        self.machine = machine.to_string();
        self
    }

    fn reason(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    fn from_reason(reason: Option<&str>) -> Option<Self> {
        serde_json::from_str(reason?).ok()
    }
}

/// A claim held by an agent that is not registered in this database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerHold {
    pub agent_id: String,
    pub holder: Option<ClaimHolder>,
    pub expires_at: Option<DateTime<Utc>>,
    /// Explicit shutdown is dead. Otherwise either a recent heartbeat or a
    /// recent renewal keeps a holder live; late presence alone cannot authorize
    /// taking a claim. `None` means no conclusive signal was available.
    pub live: Option<bool>,
}

impl PeerHold {
    /// The holder's name, falling back to its agent id.
    pub fn name(&self) -> &str {
        self.holder
            .as_ref()
            .map(|holder| holder.name.as_str())
            .unwrap_or(self.agent_id.as_str())
    }

    /// "alpha-sup on host-a", or the agent id when no identity was recorded.
    pub fn describe(&self) -> String {
        match &self.holder {
            Some(holder) => format!("{} on {}", holder.name, holder.machine),
            None => format!("agent {}", self.agent_id),
        }
    }

    pub fn until(&self) -> String {
        self.expires_at
            .map(|at| at.format("%H:%M UTC").to_string())
            .unwrap_or_else(|| "an unknown time".to_string())
    }

    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|at| at <= now)
    }

    /// A claim nobody is keeping: it ran out, or its holder is not a live
    /// agent. Acquisition rechecks and recovers it with an owner-scoped release;
    /// a stale observation alone never means a live competing worker.
    pub fn is_stale(&self, now: DateTime<Utc>) -> bool {
        self.is_expired(now) || self.live == Some(false)
    }

    /// Why a stale claim is stale, for the warning.
    pub fn stale_reason(&self, now: DateTime<Utc>) -> String {
        if self.is_expired(now) {
            format!("expired at {} without being released", self.until())
        } else {
            format!(
                "belongs to an agent Cassy Cloud has not seen alive in the last {} minutes",
                crate::cloud::peers::PEER_LIVE_SECS / 60
            )
        }
    }

    /// Advisory when this peer also works `epic_id`. `prefix` is the
    /// caller's tool prefix (e.g. `mcp__cas__`).
    pub fn shared_epic_note(&self, epic_id: &str, prefix: &str) -> String {
        format!(
            "\n\n👥 SHARED EPIC — {} also works epic {epic_id} (claimed until {}). Agree who takes which children before both of you start the same one: {prefix}factory action=epic_split id={epic_id} target={} mine=<ids> theirs=<ids>, or {prefix}coordination action=message target={} summary=\"{epic_id}\" message=\"...\"",
            self.describe(),
            self.until(),
            self.name(),
            self.name(),
        )
    }
}

/// The outcome of mirroring a lease into the cloud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Acquire {
    /// This agent now holds (or already held) the cloud claim.
    Claimed,
    /// Another agent of this database holds the cloud claim. That is not a
    /// peer: the local lease decides between the two, and only its winner
    /// moves the claim, with [`PeerClaims::take_from_local`].
    LocalHolder(String),
    /// A live peer on another machine holds it.
    PeerHolds(PeerHold),
    /// A stale holder observed during recovery (for example another stale
    /// owner won a race). Retry recovery later; never emit a live-conflict alarm.
    StalePeer(PeerHold),
    /// The cloud could not be asked; only the local lease applies.
    Unavailable(String),
}

/// Durable local SessionEnd evidence survives deletion of the registry row.
#[derive(Clone)]
pub(crate) struct RetiredLocalAgent {
    pub name: String,
    pub ended_at: DateTime<Utc>,
}

pub(crate) fn retired_local_agents(cas_root: &Path) -> HashMap<String, RetiredLocalAgent> {
    let events = crate::store::open_event_store(cas_root).ok()
        .and_then(|store| store.list_by_type(crate::types::EventType::AgentShutdown, 2000).ok())
        .unwrap_or_default();
    let mut retired = HashMap::new();
    for event in events {
        if event.entity_type == crate::types::EventEntityType::Agent
            && let Some(name) = event.summary.strip_prefix("Agent unregistered: ")
        {
            // Events are newest first. Never replace the latest exit by an old one.
            retired.entry(event.entity_id).or_insert_with(|| RetiredLocalAgent {
                name: name.to_string(), ended_at: event.created_at,
            });
        }
    }
    retired
}

/// Cloud claims for one repository.
#[derive(Clone)]
pub struct PeerClaims {
    config: CloudConfig,
    project: String,
    /// This machine's label, recorded on every claim it makes (cas-c7c7).
    label: String,
    retired_local: HashMap<String, RetiredLocalAgent>,
    budget: Option<PeerBudget>,
}

/// `<task id>~<first 8 hex of sha256(canonical id)>`.
pub fn claim_key(project: &str, task_id: &str) -> String {
    let digest = Sha256::digest(project.as_bytes());
    let hex: String = digest.iter().take(4).map(|byte| format!("{byte:02x}")).collect();
    format!("{task_id}~{hex}")
}

impl PeerClaims {
    /// Claims for the project rooted at `cas_root`, or `None` when it is not
    /// logged in to Cassy Cloud or has no canonical repository id. Then there
    /// is no peer to see, and nothing is said.
    pub fn for_project(cas_root: &Path) -> Option<Self> {
        // The project's own cloud config, as peer discovery (cas-e477) and the
        // daemon's coordinator load it: a project that does not sync never
        // reaches the cloud through the machine-wide login.
        let config = CloudConfig::load_from_cas_dir(cas_root).ok()?;
        if !config.is_logged_in() {
            return None;
        }
        let project = crate::cloud::resolve_canonical_id(cas_root)?;
        let label = crate::cloud::peers::machine_label_for(cas_root);
        let mut retired_local = retired_local_agents(cas_root);
        match crate::store::open_agent_store(cas_root).and_then(|store|
            store.list(None).map_err(crate::error::CasError::from))
        {
            Ok(agents) => retired_local.retain(|id, _| !agents.iter().any(|agent| &agent.id == id)),
            Err(_) => retired_local.clear(), // Unreadable registry cannot authorize a handoff.
        }
        Some(Self { config, project, label, retired_local, budget: None })
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    fn key(&self, task_id: &str) -> String {
        claim_key(&self.project, task_id)
    }

    /// Keep blocking Cloud calls within the caller's async request deadline.
    pub fn with_budget(mut self, budget: PeerBudget) -> Self {
        self.budget = Some(match self.budget {
            Some(existing) if existing.deadline < budget.deadline => existing,
            _ => budget,
        });
        self
    }

    fn remaining(&self) -> Result<Duration, CasError> {
        let left = self
            .budget
            .map(|budget| budget.remaining())
            .unwrap_or(CLAIM_TIMEOUT)
            .min(CLAIM_TIMEOUT);
        if left.is_zero() {
            Err(CasError::Other("peer check budget exhausted".into()))
        } else {
            Ok(left)
        }
    }

    fn coordinator(&self, agent_id: &str) -> Result<CloudCoordinator, CasError> {
        let left = self.remaining()?;
        Ok(CloudCoordinator::new(self.config.clone())?
            .with_timeout(left)
            .with_agent_id(agent_id)
            // cas-f707: claims carry the project scope (petra-stella-cloud#151).
            .with_canonical_id(Some(self.project.clone())))
    }

    /// Claim `task_id` for `agent_id`. A claim held by another agent of this
    /// database (`is_local`) is reported as [`Acquire::LocalHolder`] and left
    /// where it is: this call runs before the local lease decides, so moving
    /// it here would take it from an agent that may keep its lease.
    pub fn acquire(
        &self,
        task_id: &str,
        kind: ClaimKind,
        agent_id: &str,
        agent_name: &str,
        duration_secs: u32,
        is_local: &dyn Fn(&str) -> bool,
    ) -> Acquire {
        let claims = self.clone().with_budget(PeerBudget::default());
        claims.acquire_bounded(task_id, kind, agent_id, agent_name, duration_secs, is_local)
    }

    fn acquire_bounded(
        &self, task_id: &str, kind: ClaimKind, agent_id: &str, agent_name: &str,
        duration_secs: u32, is_local: &dyn Fn(&str) -> bool,
    ) -> Acquire {
        let deadline = self.budget.expect("acquire has a budget").deadline;
        let key = self.key(task_id);
        let reason = ClaimHolder::new(kind, agent_name, &self.project).on(&self.label).reason();
        let coordinator = match self.coordinator(agent_id) {
            Ok(coordinator) => coordinator,
            Err(error) => return Acquire::Unavailable(error.to_string()),
        };
        let (held_by, expires_at) = match coordinator.claim(&key, duration_secs, Some(&reason)) {
            Ok(ClaimResult::Success(_)) => return Acquire::Claimed,
            Ok(ClaimResult::AlreadyClaimed { held_by, expires_at, .. }) => (held_by, expires_at),
            Ok(other) => return Acquire::Unavailable(format!("unexpected claim answer: {other:?}")),
            Err(error) => return Acquire::Unavailable(error.to_string()),
        };
        // A refusal that names no holder (an older cloud, or a body that did
        // not parse) is resolved from the lock, so a local holder is never
        // mistaken for a peer.
        let (held_by, expires_at) = if held_by.is_empty() {
            match self.coordinator(agent_id).and_then(|c| c.get_lock(&key)) {
                Ok(Some(lock)) => (lock.agent_id, lock.expires_at),
                Ok(None) => return Acquire::Unavailable("the claim was refused without a holder".into()),
                Err(error) => return Acquire::Unavailable(error.to_string()),
            }
        } else {
            (held_by, expires_at)
        };
        if held_by == agent_id {
            return Acquire::Claimed;
        }
        if is_local(&held_by) {
            return Acquire::LocalHolder(held_by);
        }
        if self.retired_local.contains_key(&held_by)
            && let Ok(Some(lock)) = self.coordinator(agent_id).and_then(|c| c.get_lock(&key))
            && lock.agent_id == held_by
            && self.retired_local.get(&held_by).is_some_and(|ended| lock.renewed_at <= ended.ended_at)
        {
            // Still arbitrate locally before moving this pre-exit claim.
            return Acquire::LocalHolder(held_by);
        }
        let answer = self.conflict_answer(&key, &held_by, expires_at);
        match answer {
            Acquire::StalePeer(hold) => self.take_stale_peer(
                task_id, kind, &hold, agent_id, agent_name, duration_secs, deadline,
            ),
            answer => answer,
        }
    }

    /// A stale observation is not permission to release a later owner's claim.
    /// Re-read ownership and liveness, then use the wire protocol's owner-scoped
    /// release. A raced live holder is reported rather than overwritten.
    fn take_stale_peer(
        &self,
        task_id: &str,
        kind: ClaimKind,
        observed: &PeerHold,
        agent_id: &str,
        agent_name: &str,
        duration_secs: u32,
        deadline: Instant,
    ) -> Acquire {
        let claims = self.clone().with_budget(PeerBudget { deadline });
        let key = claims.key(task_id);
        let lock = match claims.coordinator(agent_id).and_then(|c| c.get_lock(&key)) {
            Ok(Some(lock)) => lock,
            Ok(None) => {
                return Acquire::Unavailable("the stale claim disappeared; retry claiming".into());
            }
            Err(error) => return Acquire::Unavailable(error.to_string()),
        };
        let listed = claims.listed_holder(&lock.agent_id);
        let current = Self::assemble(lock.agent_id.clone(), Some(&lock), None, listed);
        if current.agent_id != observed.agent_id || !current.is_stale(Utc::now()) {
            return Self::answer(current);
        }
        // The release validates this exact snapshot again, including same-ID
        // renewal/reacquisition, after the final presence observation.
        claims.move_claim(
            task_id,
            kind,
            &current.agent_id,
            agent_id,
            agent_name,
            duration_secs,
            deadline,
            Some(&lock),
        )
    }

    /// A peer's hold from a refused claim. The 409 names the holder and when
    /// its lock runs out, and the agent list names its machine and says
    /// whether it is live. Late or missing presence also reads the lock: a
    /// recent renewal protects the holder even when presence has backed off.
    fn conflict_answer(&self, key: &str, held_by: &str, expires_at: DateTime<Utc>) -> Acquire {
        let now = Utc::now();
        if held_by.is_empty() || expires_at <= now {
            return self.peer_answer(key, held_by);
        }
        let listed = self.listed_holder(held_by);
        let lock = match &listed {
            Listed::Live(..) | Listed::Shutdown(..) => None,
            Listed::Stale(..) | Listed::Missing | Listed::Unknown => self
                .coordinator(held_by)
                .and_then(|coordinator| coordinator.get_lock(key))
                .ok()
                .flatten(),
        };
        Self::answer(Self::assemble(held_by.to_string(), lock.as_ref(), Some(expires_at), listed))
    }

    /// Move the cloud claim on `task_id` from `from`, another agent of this
    /// database, to `agent_id`. Only for the winner of the local lease.
    pub fn take_from_local(
        &self,
        task_id: &str,
        kind: ClaimKind,
        from: &str,
        agent_id: &str,
        agent_name: &str,
        duration_secs: u32,
    ) -> Acquire {
        self.move_claim(task_id, kind, from, agent_id, agent_name, duration_secs,
            Instant::now() + CLAIM_TIMEOUT, None)
    }

    fn move_claim(
        &self,
        task_id: &str,
        kind: ClaimKind,
        from: &str,
        agent_id: &str,
        agent_name: &str,
        duration_secs: u32,
        deadline: Instant,
        expected: Option<&TaskLease>,
    ) -> Acquire {
        let claims = self.clone().with_budget(PeerBudget { deadline });
        let key = self.key(task_id);
        let reason = ClaimHolder::new(kind, agent_name, &self.project)
            .on(&self.label)
            .reason();
        if let Some(expected) = expected {
            match claims.release_if_unchanged(&key, expected) {
                Ok(true) => {}
                Ok(false) => {
                    return Acquire::PeerHolds(PeerHold {
                        agent_id: expected.agent_id.clone(),
                        holder: ClaimHolder::from_reason(expected.claim_reason.as_deref()),
                        expires_at: Some(expected.expires_at),
                        live: None,
                    });
                }
                Err(error) => return Acquire::Unavailable(error.to_string()),
            }
        } else if let Err(error) = claims.release_with_reserve(&key, from) {
            return Acquire::Unavailable(error.to_string());
        }
        let moved = claims
            .coordinator(agent_id)
            .and_then(|coordinator| coordinator.claim(&key, duration_secs, Some(&reason)));
        match moved {
            Ok(ClaimResult::Success(_)) => Acquire::Claimed,
            Ok(ClaimResult::AlreadyClaimed { held_by, .. }) if held_by == agent_id => {
                Acquire::Claimed
            }
            // Someone else claimed it between the release and the claim.
            Ok(ClaimResult::AlreadyClaimed { held_by, .. }) if !held_by.is_empty() => {
                claims.peer_answer(&key, &held_by)
            }
            Ok(other) => Acquire::Unavailable(format!("could not move the claim: {other:?}")),
            Err(error) => Acquire::Unavailable(error.to_string()),
        }
    }

    /// Spend at most half the remaining allowance releasing, leaving time to
    /// claim. A blocking continuation cannot start a call after the deadline.
    fn release_with_reserve(&self, key: &str, agent_id: &str) -> Result<(), CasError> {
        let left = self.remaining()? / 2;
        if left.is_zero() {
            return Err(CasError::Other("peer check budget exhausted".into()));
        }
        self.coordinator(agent_id)?.with_timeout(left).release(key)
    }

    fn release_if_unchanged(&self, key: &str, expected: &TaskLease) -> Result<bool, CasError> {
        let current = self.coordinator(&expected.agent_id)?.get_lock(key)?;
        let unchanged = current.is_some_and(|current| {
            current.agent_id == expected.agent_id
                && current.epoch == expected.epoch
                && current.renewed_at == expected.renewed_at
                && current.status == expected.status
        });
        if !unchanged {
            return Ok(false);
        }
        self.release_with_reserve(key, &expected.agent_id)?;
        Ok(true)
    }

    /// A peer's hold on `key`, live or stale, read from the lock.
    fn peer_answer(&self, key: &str, held_by: &str) -> Acquire {
        Self::answer(self.hold(key, held_by))
    }

    fn answer(hold: PeerHold) -> Acquire {
        if hold.is_stale(Utc::now()) {
            Acquire::StalePeer(hold)
        } else {
            Acquire::PeerHolds(hold)
        }
    }

    /// Release the cloud claim on `task_id` only when `agent_id` holds it.
    /// Returns whether a claim was released.
    pub fn release_own(&self, task_id: &str, agent_id: &str) -> Result<bool, CasError> {
        let key = self.key(task_id);
        let Some(lock) = self.coordinator(agent_id)?.get_lock(&key)? else {
            return Ok(false);
        };
        if lock.agent_id != agent_id {
            return Ok(false);
        }
        self.coordinator(agent_id)?.release(&key)?;
        Ok(true)
    }

    /// The holder as Cassy Cloud lists it: whether it is live, and its
    /// name and machine from registration.
    fn listed_holder(&self, held_by: &str) -> Listed {
        let Some(agents) = self
            .coordinator(held_by)
            .ok()
            .and_then(|coordinator| coordinator.list_agent_infos().ok())
        else {
            return Listed::Unknown;
        };
        let Some(agent) = agents.into_iter().find(|agent| agent.id == held_by) else {
            return Listed::Missing;
        };
        let age = DateTime::parse_from_rfc3339(&agent.last_heartbeat)
            .map(|at| (Utc::now() - at.with_timezone(&Utc)).num_seconds())
            .unwrap_or(i64::MAX);
        let live = !matches!(agent.status.as_str(), "stale" | "shutdown")
            && age <= crate::cloud::peers::PEER_LIVE_SECS;
        let machine = agent
            .metadata
            .get(crate::cloud::peers::META_HOSTNAME)
            .cloned()
            .or(agent.machine_id)
            .unwrap_or_else(|| "an unknown machine".to_string());
        let holder = ClaimHolder {
            v: 1,
            kind: String::new(),
            name: agent.name,
            machine,
            project: agent
                .metadata
                .get(crate::cloud::peers::META_CANONICAL_ID)
                .cloned()
                .unwrap_or_default(),
        };
        if agent.status == "shutdown" { Listed::Shutdown(holder) }
        else if live { Listed::Live(holder) } else { Listed::Stale(holder) }
    }

    /// Who holds `key`, read from the lock itself.
    fn hold(&self, key: &str, held_by: &str) -> PeerHold {
        let claims = self.clone().with_budget(PeerBudget::default());
        let lock = claims
            .coordinator(held_by)
            .and_then(|coordinator| coordinator.get_lock(key))
            .ok()
            .flatten();
        let agent_id = lock
            .as_ref()
            .map(|lock| lock.agent_id.clone())
            .unwrap_or_else(|| held_by.to_string());
        let listed = claims.listed_holder(&agent_id);
        Self::assemble(agent_id, lock.as_ref(), None, listed)
    }

    /// One hold from whatever was learned: the lock (reason, expiry, last
    /// renewal), the 409's expiry, and the agent list.
    ///
    /// Only explicit shutdown overrides renewal. Presence can lag independently
    /// of renewals; either fresh signal protects the claim. Missing evidence
    /// cannot authorize recovery.
    fn assemble(
        agent_id: String,
        lock: Option<&TaskLease>,
        expires_at: Option<DateTime<Utc>>,
        listed: Listed,
    ) -> PeerHold {
        let renewed_recently = lock.map(|lock| {
            (Utc::now() - lock.renewed_at).num_seconds() <= crate::cloud::peers::PEER_LIVE_SECS
        });
        let (live, listed_holder) = match listed {
            Listed::Shutdown(holder) => (Some(false), Some(holder)),
            Listed::Live(holder) => (Some(true), Some(holder)),
            Listed::Stale(holder) => (renewed_recently, Some(holder)),
            Listed::Missing | Listed::Unknown => (renewed_recently, None),
        };
        PeerHold {
            holder: lock
                .and_then(|lock| ClaimHolder::from_reason(lock.claim_reason.as_deref()))
                .or(listed_holder),
            live,
            expires_at: expires_at.or(lock.map(|lock| lock.expires_at)),
            agent_id,
        }
    }

    /// A live claim on `task_id` held by a peer (not an agent of this
    /// database), for advisories that warn without claiming.
    pub fn peer_hold(
        &self,
        task_id: &str,
        is_local: &dyn Fn(&str) -> bool,
    ) -> Result<Option<PeerHold>, CasError> {
        let claims = self.clone().with_budget(PeerBudget::default());
        let key = claims.key(task_id);
        let lock = claims.coordinator("peer-claims-reader")?.get_lock(&key)?;
        Ok(lock
            .filter(|lock| !is_local(&lock.agent_id)
                && !self.retired_local.get(&lock.agent_id)
                    .is_some_and(|ended| lock.renewed_at <= ended.ended_at))
            .map(|lock| claims.hold(&key, &lock.agent_id))
            .filter(|hold| !hold.is_stale(Utc::now())))
    }

    /// Release the cloud claim on `task_id` when an agent of this database
    /// holds it. Returns whether a claim was released.
    pub fn release_local(
        &self,
        task_id: &str,
        is_local: &dyn Fn(&str) -> bool,
    ) -> Result<bool, CasError> {
        let key = self.key(task_id);
        let Some(lock) = self.coordinator("peer-claims-reader")?.get_lock(&key)? else {
            return Ok(false);
        };
        if !is_local(&lock.agent_id) {
            return Ok(false);
        }
        self.coordinator(&lock.agent_id)?.release(&key)?;
        Ok(true)
    }

    pub(crate) fn release_reset_holder(
        &self, task_id: &str, name: &str, registered_ids: &HashSet<String>,
    ) -> Result<bool, CasError> {
        let claims = self.clone().with_budget(PeerBudget::default());
        let key = claims.key(task_id);
        let Some(lock) = claims.coordinator("peer-claims-reader")?.get_lock(&key)? else {
            return Ok(false);
        };
        let ended = self.retired_local.get(&lock.agent_id).is_some_and(|ended|
            ended.name == name && lock.renewed_at <= ended.ended_at);
        if !registered_ids.contains(&lock.agent_id) && !ended { return Ok(false); }
        claims.release_if_unchanged(&key, &lock)
    }

    /// The agent holding the active claim on `task_id`, if any.
    pub fn holder(&self, task_id: &str) -> Result<Option<String>, CasError> {
        Ok(self
            .coordinator("peer-claims-reader")?
            .get_lock(&self.key(task_id))?
            .map(|lock| lock.agent_id))
    }

    pub fn renew(&self, task_id: &str, agent_id: &str, duration_secs: u32) -> Result<(), CasError> {
        self.coordinator(agent_id)?
            .renew(&self.key(task_id), duration_secs)
            .map(|_| ())
    }
}

/// What one renewal pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenewReport {
    pub renewed: usize,
    /// Claims that were lost (released or expired in the cloud) and taken
    /// again.
    pub reacquired: usize,
    pub failed: usize,
    /// Claims a peer took while this agent's was lost: the task and its new
    /// holder.
    pub conflicts: Vec<(String, PeerHold)>,
}

/// Renew the cloud claims of every task and epic `agent_id` holds a local
/// lease on, and of the epic its focus claims ([`META_FOCUS_CLAIM`]), for the
/// configured lease duration. Called from the daemon heartbeat, so claims
/// stay alive while the agent works and run out after it stops.
///
/// cas-0078: a claim that can no longer be renewed (it was released or
/// expired in the cloud) is claimed again. When a peer took it meanwhile, the
/// report names the peer as a conflict instead.
pub fn renew_agent_claims(cas_root: &Path, agent_id: &str) -> RenewReport {
    let mut report = RenewReport::default();
    let Some(claims) = PeerClaims::for_project(cas_root) else {
        return report;
    };
    let Ok(agents) = crate::store::open_agent_store(cas_root) else {
        return report;
    };
    let Ok(leases) = agents.list_agent_leases(agent_id) else {
        return report;
    };
    let me = agents.get(agent_id).ok();
    let name = me
        .as_ref()
        .map(|agent| agent.name.clone())
        .unwrap_or_else(|| agent_id.to_string());
    // (claim id, backed by a local lease)
    let mut targets: Vec<(String, bool)> = leases
        .into_iter()
        .filter(|lease| lease.is_valid())
        .map(|lease| (lease.task_id, true))
        .collect();
    if let Some(focus) = me
        .as_ref()
        .and_then(|agent| agent.metadata.get(META_FOCUS_CLAIM))
        .filter(|focus| !focus.trim().is_empty())
        && !targets.iter().any(|(id, _)| id == focus)
    {
        targets.push((focus.clone(), false));
    }
    let tasks = crate::store::open_task_store(cas_root).ok();
    let kind_of = |id: &str| match tasks.as_ref().and_then(|tasks| tasks.get(id).ok()) {
        Some(task) if task.task_type == crate::types::TaskType::Epic => ClaimKind::Epic,
        _ => ClaimKind::Task,
    };
    let is_local = |id: &str| agents.get(id).is_ok();
    let duration = lease_duration_secs(cas_root);
    for (task_id, lease_backed) in targets {
        match claims.renew(&task_id, agent_id, duration) {
            Ok(()) => {
                report.renewed += 1;
                continue;
            }
            Err(error) => {
                tracing::debug!(task = %task_id, %error, "cloud claim renewal failed; claiming again");
            }
        }
        let kind = kind_of(&task_id);
        let mut answer = claims.acquire(&task_id, kind, agent_id, &name, duration, &is_local);
        // This agent's local lease already won, so the claim belongs here.
        if lease_backed && let Acquire::LocalHolder(from) = &answer {
            answer = claims.take_from_local(&task_id, kind, from, agent_id, &name, duration);
        }
        match answer {
            Acquire::Claimed => report.reacquired += 1,
            // A focus another agent of this database also claims: the epic is
            // still claimed for this repository, and nothing is lost.
            Acquire::LocalHolder(_) => {}
            Acquire::StalePeer(_) => {
                // Staleness is not a competing worker. Retry recovery next beat.
                report.failed += 1;
            }
            Acquire::PeerHolds(hold) => {
                notify_claim_lost(cas_root, agents.as_ref(), me.as_ref(), agent_id, &task_id, &hold);
                report.conflicts.push((task_id, hold));
            }
            Acquire::Unavailable(error) => {
                report.failed += 1;
                tracing::debug!(task = %task_id, %error, "cloud claim could not be taken again");
            }
        }
    }
    // cas-4736: a supervisor's share of a binding epic split is claimed by
    // the supervisor itself, without a local lease.
    report.renewed += crate::cloud::epic_split::renew_split_claims(cas_root, &claims, agent_id, duration);
    report
}

/// Tell the supervisor that owns `agent_id` (the agent itself when it is
/// not a worker) that a peer took its claim on `task_id` (cas-0078). The
/// local lease no longer keeps that peer off the task, so the work must stop
/// or be coordinated. One notice per task, peer and agent: renewal finds the
/// same conflict every pass, and the queue key dedupes it.
fn notify_claim_lost(
    cas_root: &Path,
    agents: &dyn crate::store::AgentStore,
    me: Option<&crate::types::Agent>,
    agent_id: &str,
    task_id: &str,
    hold: &PeerHold,
) {
    let Some(me) = me else {
        return;
    };
    let session = me
        .factory_session
        .clone()
        .filter(|session| !session.trim().is_empty());
    let target = if me.role == crate::types::AgentRole::Worker {
        match crate::mcp::tools::core::task::lifecycle::supervisor_push::resolve_worker_supervisor(
            agents,
            session.as_deref(),
        ) {
            Some(owner) => owner.name,
            None => {
                tracing::warn!(task = %task_id, agent = %agent_id, "claim lost to a peer, and no owning supervisor to tell");
                return;
            }
        }
    } else {
        me.name.clone()
    };
    let machine = hold
        .holder
        .as_ref()
        .map(|holder| holder.machine.as_str())
        .unwrap_or("an unknown machine");
    let prefix = crate::mcp::tools::core::guidance::caller_prefix();
    let prompt = format!(
        "⚠️  PEER CLAIM LOST — claim on {task_id} lost to {peer}@{machine}; stop work or coordinate. \
         {holder} holds the Cassy Cloud claim until {until}, so {mine}'s local lease no longer keeps \
         that session off {task_id}. Ask them: {prefix}coordination action=message target={peer} \
         summary=\"{task_id}\" message=\"...\"",
        peer = hold.name(),
        holder = hold.describe(),
        until = hold.until(),
        mine = me.name,
    );
    let summary = format!("Claim on {task_id} lost to {}", hold.name());
    let key = format!("peer-claim-lost:{task_id}:{}:{agent_id}", hold.agent_id);
    let queue = match crate::store::open_prompt_queue_store(cas_root) {
        Ok(queue) => queue,
        Err(error) => {
            tracing::warn!(task = %task_id, %error, "could not open the queue for the lost-claim notice");
            return;
        }
    };
    match queue.enqueue_idempotent(
        "peer-claims",
        &target,
        &prompt,
        session.as_deref(),
        Some(&summary),
        Some(cas_store::NotificationPriority::High),
        &key,
        Some(&cas_store::QueueOrigin::Daemon),
    ) {
        Ok(cas_store::EnqueueIdempotentResult::Created(_)) => {
            crate::ui::factory::daemon::runtime::delivery::wake_daemon_after_enqueue(cas_root);
        }
        Ok(cas_store::EnqueueIdempotentResult::AlreadyExists(_)) => {}
        Err(error) => {
            tracing::warn!(task = %task_id, %error, "could not queue the lost-claim notice");
        }
    }
}

/// How often the daemon heartbeat renews an agent's claims. Claims last a
/// full lease, so a renewal every two minutes keeps them alive with margin
/// and stays far below the cloud's per-user rate limit.
pub const RENEW_EVERY: Duration = Duration::from_secs(120);

/// [`renew_agent_claims`] at most once per [`RENEW_EVERY`] per agent.
pub fn renew_agent_claims_if_due(cas_root: &Path, agent_id: &str) -> Option<RenewReport> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;
    static LAST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    {
        let mut last = LAST.get_or_init(Default::default).lock().ok()?;
        let now = Instant::now();
        if last
            .get(agent_id)
            .is_some_and(|at| now.duration_since(*at) < RENEW_EVERY)
        {
            return None;
        }
        last.insert(agent_id.to_string(), now);
    }
    Some(renew_agent_claims(cas_root, agent_id))
}

/// Release the cloud claim on `task_id` held by any agent of this database.
/// Best-effort: called beside every local lease release.
pub fn release_task_claim(cas_root: &Path, task_id: &str) {
    let Some(claims) = PeerClaims::for_project(cas_root) else {
        return;
    };
    let Ok(agents) = crate::store::open_agent_store(cas_root) else {
        return;
    };
    let is_local = |id: &str| agents.get(id).is_ok();
    if let Err(error) = claims.release_local(task_id, &is_local) {
        tracing::debug!(task = %task_id, %error, "cloud claim release failed");
    }
}

/// What a spawn learns about peers before it queues anything (cas-0078).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpawnPeerCheck {
    /// A live peer claim on the task the spawn would pre-assign.
    pub task: Option<(String, PeerHold)>,
    /// Live peer claims on the epics the spawned worker would work in.
    pub epics: Vec<(String, PeerHold)>,
    /// Why Cassy Cloud could not be asked, when it could not.
    pub unavailable: Option<String>,
}

/// Live peer claims on `task_id` and `epic_ids`, for `spawn_workers`.
/// `None` when the project is not logged in (no peers to see). Read-only.
pub fn spawn_peer_check(
    cas_root: &Path,
    task_id: Option<&str>,
    epic_ids: &[String],
) -> Option<SpawnPeerCheck> {
    let claims = PeerClaims::for_project(cas_root)?;
    let local = crate::store::open_agent_store(cas_root)
        .map(|agents| local_agent_ids(agents.as_ref()))
        .unwrap_or_default();
    let is_local = |id: &str| local.contains(id);
    let mut check = SpawnPeerCheck::default();
    let ids = task_id
        .map(|id| (id, true))
        .into_iter()
        .chain(epic_ids.iter().map(|id| (id.as_str(), false)));
    for (id, is_task) in ids {
        match claims.peer_hold(id, &is_local) {
            Ok(Some(hold)) if is_task => check.task = Some((id.to_string(), hold)),
            Ok(Some(hold)) => check.epics.push((id.to_string(), hold)),
            Ok(None) => {}
            Err(error) => {
                check.unavailable = Some(error.to_string());
                break;
            }
        }
    }
    Some(check)
}

/// Claim the focus on `epic_id` for `agent_id` (a supervisor's
/// `focus_epic`), and record it on the agent so renewal keeps the claim
/// alive while the focus stands. A previous focus's claim is released. The
/// answer says whether a peer already works the epic; the focus is never
/// refused. `None` when the project is not logged in.
pub fn claim_epic_focus(cas_root: &Path, epic_id: &str, agent_id: &str) -> Option<Acquire> {
    let claims = PeerClaims::for_project(cas_root)?;
    let Ok(agents) = crate::store::open_agent_store(cas_root) else {
        return Some(Acquire::Unavailable("the agent registry could not be opened".into()));
    };
    if let Some(previous) = remember_focus(agents.as_ref(), agent_id, Some(epic_id))
        .filter(|previous| previous != epic_id)
    {
        release_focus_claim(&claims, agents.as_ref(), &previous, agent_id);
    }
    let name = agents
        .get(agent_id)
        .map(|agent| agent.name)
        .unwrap_or_else(|_| agent_id.to_string());
    let is_local = |id: &str| agents.get(id).is_ok();
    let duration = lease_duration_secs(cas_root);
    let answer = claims.acquire(epic_id, ClaimKind::Epic, agent_id, &name, duration, &is_local);
    let answer = match answer {
        Acquire::LocalHolder(ref from) if match agents.get(from) {
            Ok(holder) => !crate::mcp::tools::service::agent_liveness::evaluate_supervision_liveness_with(
                &holder,
                crate::mcp::tools::service::agent_liveness::agent_process_is_alive(&holder),
                crate::cloud::peers::PEER_LIVE_SECS,
            ).is_live(),
            Err(cas_store::StoreError::NotFound(_)) => claims.retired_local.contains_key(from),
            Err(_) => false,
        } => claims.take_from_local(epic_id, ClaimKind::Epic, from, agent_id, &name, duration),
        answer => answer,
    };
    Some(answer)
}

/// Forget `agent_id`'s focus claim and release it in the cloud. Returns the
/// epic it was on, if any.
pub fn release_epic_focus(cas_root: &Path, agent_id: &str) -> Option<String> {
    let agents = crate::store::open_agent_store(cas_root).ok()?;
    let previous = remember_focus(agents.as_ref(), agent_id, None)?;
    if let Some(claims) = PeerClaims::for_project(cas_root) {
        release_focus_claim(&claims, agents.as_ref(), &previous, agent_id);
    }
    Some(previous)
}

/// Set (or with `None`, clear) the agent's focus claim; returns the old one.
fn remember_focus(
    agents: &dyn crate::store::AgentStore,
    agent_id: &str,
    epic_id: Option<&str>,
) -> Option<String> {
    let mut agent = agents.get(agent_id).ok()?;
    let previous = agent.metadata.get(META_FOCUS_CLAIM).cloned();
    if previous.as_deref() != epic_id {
        match epic_id {
            Some(epic_id) => {
                agent.metadata.insert(META_FOCUS_CLAIM.to_string(), epic_id.to_string());
            }
            None => {
                agent.metadata.remove(META_FOCUS_CLAIM);
            }
        }
        if let Err(error) = agents.update(&agent) {
            tracing::debug!(%error, "could not record the focus claim");
        }
    }
    previous
}

/// Release the focus claim on `epic_id`, unless `agent_id` also holds a
/// local lease on the epic (a subtask start claims it): then the claim stays
/// and renews with that lease.
fn release_focus_claim(
    claims: &PeerClaims,
    agents: &dyn crate::store::AgentStore,
    epic_id: &str,
    agent_id: &str,
) {
    let leased = agents
        .list_agent_leases(agent_id)
        .map(|leases| leases.iter().any(|lease| lease.task_id == epic_id && lease.is_valid()))
        .unwrap_or(false);
    if leased {
        return;
    }
    if let Err(error) = claims.release_own(epic_id, agent_id) {
        tracing::debug!(epic = %epic_id, %error, "cloud focus claim release failed");
    }
}

/// The configured lease duration, as a cloud claim duration (the cloud
/// clamps claims to 1–3600 seconds).
pub fn lease_duration_secs(cas_root: &Path) -> u32 {
    let mins = crate::config::Config::load(cas_root)
        .map(|config| config.lease().default_duration_mins)
        .unwrap_or(30);
    (mins.max(1) * 60).min(3600) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cas-0078 (f): cloud work runs on the blocking pool, and one budget
    /// bounds every call of a request: past the deadline the caller gets an
    /// answer at once instead of waiting on the network.
    #[tokio::test(flavor = "current_thread")]
    async fn the_budget_runs_work_off_the_runtime_and_stops_at_its_deadline() {
        let budget = PeerBudget::new(Duration::from_millis(150));
        let worker = budget.run(|| std::thread::current().id()).await.unwrap();
        assert_ne!(worker, std::thread::current().id(), "not on the runtime thread");
        let started = Instant::now();
        let slow = budget.run(|| std::thread::sleep(Duration::from_millis(600))).await;
        assert!(slow.unwrap_err().contains("no answer"));
        assert!(started.elapsed() < Duration::from_millis(500), "{:?}", started.elapsed());
        let spent = Instant::now();
        assert!(budget.run(|| 1).await.is_err(), "a spent budget answers at once");
        assert!(spent.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn cas_50713_stale_recovery_bounds_reads_by_remaining_budget() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let key = claim_key("repo", "cas-slow");
        let mut lock = TaskLease::new(key.clone(), "old".into(), 600);
        lock.renewed_at = Utc::now() - chrono::Duration::seconds(301);
        let mut wire_lock = serde_json::to_value(&lock).unwrap();
        wire_lock["id"] = serde_json::json!("fixture-lock");
        Mock::given(method("GET"))
            .and(path(format!("/api/agents/tasks/{key}/lock")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"lock": wire_lock}))
                    .set_delay(Duration::from_millis(400)),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/agents"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"agents": []})),
            )
            .mount(&server)
            .await;
        let claims = PeerClaims {
            config: CloudConfig {
                endpoint: server.uri(),
                token: Some("test-token".into()),
                ..Default::default()
            },
            project: "repo".into(),
            label: "here".into(),
            retired_local: HashMap::new(),
            budget: None,
        };
        let started = Instant::now();
        let answer = tokio::task::spawn_blocking(move || {
            let observed = PeerHold {
                agent_id: "old".into(),
                holder: None,
                expires_at: None,
                live: Some(false),
            };
            claims.take_stale_peer(
                "cas-slow",
                ClaimKind::Task,
                &observed,
                "new",
                "new",
                600,
                Instant::now() + Duration::from_millis(80),
            )
        })
        .await
        .unwrap();
        assert!(matches!(answer, Acquire::Unavailable(_)), "{answer:?}");
        assert!(
            started.elapsed() < Duration::from_millis(300),
            "{:?}",
            started.elapsed()
        );
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|request| !request.url.path().ends_with("/release"))
        );
    }

    #[test]
    fn claim_keys_are_scoped_to_the_repository() {
        let widget = claim_key("github.com/acme/widget", "cas-1234");
        let other = claim_key("github.com/acme/other", "cas-1234");
        assert!(widget.starts_with("cas-1234~"), "{widget}");
        assert_eq!(widget.len(), "cas-1234~".len() + 8);
        assert_ne!(widget, other);
        assert_eq!(widget, claim_key("github.com/acme/widget", "cas-1234"));
        assert!(widget.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '~'));
    }

    #[test]
    fn the_holder_round_trips_through_the_claim_reason() {
        let holder = ClaimHolder::new(ClaimKind::Epic, "alpha-sup", "github.com/acme/widget");
        assert_eq!(holder.kind, "epic");
        assert_eq!(ClaimHolder::from_reason(Some(&holder.reason())), Some(holder.clone()));
        assert_eq!(ClaimHolder::from_reason(Some("Task started")), None);
        let hold = PeerHold { agent_id: "a1".into(), holder: Some(holder), expires_at: None, live: Some(true) };
        assert_eq!(hold.describe(), format!("alpha-sup on {}", hold.holder.as_ref().unwrap().machine));
        let anonymous = PeerHold { agent_id: "a1".into(), holder: None, expires_at: None, live: None };
        assert_eq!(anonymous.describe(), "agent a1");
        assert_eq!(anonymous.name(), "a1");
    }

    #[test]
    fn a_hold_is_stale_when_it_ran_out_or_its_holder_is_not_live() {
        let now = Utc::now();
        let hold = |at, live| PeerHold { agent_id: "a".into(), holder: None, expires_at: at, live };
        let past = Some(now - chrono::Duration::seconds(1));
        let future = Some(now + chrono::Duration::seconds(60));
        assert!(hold(past, Some(true)).is_expired(now));
        assert!(!hold(future, Some(true)).is_expired(now));
        assert!(!hold(None, Some(true)).is_expired(now));
        // Stale: ran out, or the holder is not a live agent; unknown liveness
        // never makes a live claim stale.
        assert!(hold(past, Some(true)).is_stale(now));
        assert!(hold(future, Some(false)).is_stale(now));
        assert!(!hold(future, None).is_stale(now));
        assert!(!hold(future, Some(true)).is_stale(now));
        assert!(hold(past, None).stale_reason(now).starts_with("expired at"));
        assert!(hold(future, Some(false)).stale_reason(now).contains("not seen alive"));
    }
}
