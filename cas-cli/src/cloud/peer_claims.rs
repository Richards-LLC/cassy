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

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cloud::{CloudConfig, CloudCoordinator};
use crate::error::CasError;
use crate::types::ClaimResult;

/// Bounded wait for a claim round trip, so an unreachable cloud costs a
/// start seconds, not the coordinator's default thirty.
pub const CLAIM_TIMEOUT: Duration = Duration::from_secs(5);

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
            machine: crate::types::Agent::get_or_generate_machine_id(),
            project: project.to_string(),
        }
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

    /// Advisory when this peer also works `epic_id`. `prefix` is the
    /// caller's tool prefix (e.g. `mcp__cas__`).
    pub fn shared_epic_note(&self, epic_id: &str, prefix: &str) -> String {
        format!(
            "\n\n👥 SHARED EPIC — {} also works epic {epic_id} (claimed until {}). Agree who takes which children before both of you start the same one: {prefix}coordination action=message target={} summary=\"{epic_id}\" message=\"...\"",
            self.describe(),
            self.until(),
            self.name(),
        )
    }
}

/// The outcome of mirroring a lease into the cloud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Acquire {
    /// This agent now holds (or already held) the cloud claim.
    Claimed,
    /// A live peer on another machine holds it.
    PeerHolds(PeerHold),
    /// A peer's claim ran out without being released (its holder stopped
    /// renewing). The cloud still reports it held (petra-stella-cloud#150),
    /// so this agent proceeds on its local lease only.
    StalePeer(PeerHold),
    /// The cloud could not be asked; only the local lease applies.
    Unavailable(String),
}

/// Cloud claims for one repository.
#[derive(Clone)]
pub struct PeerClaims {
    config: CloudConfig,
    project: String,
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
        let config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root).ok()?;
        if !config.is_logged_in() {
            return None;
        }
        let project = crate::cloud::resolve_canonical_id(cas_root)?;
        Some(Self { config, project })
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    fn key(&self, task_id: &str) -> String {
        claim_key(&self.project, task_id)
    }

    fn coordinator(&self, agent_id: &str) -> Result<CloudCoordinator, CasError> {
        Ok(CloudCoordinator::new(self.config.clone())?
            .with_timeout(CLAIM_TIMEOUT)
            .with_agent_id(agent_id))
    }

    /// Claim `task_id` for `agent_id`. A claim held by another agent of this
    /// database (`is_local`) is not a peer's: the local lease already decided,
    /// so the claim moves to `agent_id`.
    pub fn acquire(
        &self,
        task_id: &str,
        kind: ClaimKind,
        agent_id: &str,
        agent_name: &str,
        duration_secs: u32,
        is_local: &dyn Fn(&str) -> bool,
    ) -> Acquire {
        let key = self.key(task_id);
        let reason = ClaimHolder::new(kind, agent_name, &self.project).reason();
        let coordinator = match self.coordinator(agent_id) {
            Ok(coordinator) => coordinator,
            Err(error) => return Acquire::Unavailable(error.to_string()),
        };
        let held_by = match coordinator.claim(&key, duration_secs, Some(&reason)) {
            Ok(ClaimResult::Success(_)) => return Acquire::Claimed,
            Ok(ClaimResult::AlreadyClaimed { held_by, .. }) => held_by,
            Ok(other) => return Acquire::Unavailable(format!("unexpected claim answer: {other:?}")),
            Err(error) => return Acquire::Unavailable(error.to_string()),
        };
        if held_by == agent_id {
            return Acquire::Claimed;
        }
        if !held_by.is_empty() && is_local(&held_by) {
            let moved = self
                .coordinator(&held_by)
                .and_then(|holder| holder.release(&key))
                .and_then(|()| coordinator.claim(&key, duration_secs, Some(&reason)));
            return match moved {
                Ok(ClaimResult::Success(_)) => Acquire::Claimed,
                Ok(other) => Acquire::Unavailable(format!("could not move the claim: {other:?}")),
                Err(error) => Acquire::Unavailable(error.to_string()),
            };
        }
        let hold = self.hold(&key, &held_by);
        if hold.is_expired(Utc::now()) {
            Acquire::StalePeer(hold)
        } else {
            Acquire::PeerHolds(hold)
        }
    }

    /// Who holds `key`, read from the lock itself.
    fn hold(&self, key: &str, held_by: &str) -> PeerHold {
        let lock = self
            .coordinator(held_by)
            .and_then(|coordinator| coordinator.get_lock(key))
            .ok()
            .flatten();
        PeerHold {
            agent_id: lock
                .as_ref()
                .map(|lock| lock.agent_id.clone())
                .unwrap_or_else(|| held_by.to_string()),
            holder: lock
                .as_ref()
                .and_then(|lock| ClaimHolder::from_reason(lock.claim_reason.as_deref())),
            expires_at: lock.map(|lock| lock.expires_at),
        }
    }

    /// A live claim on `task_id` held by a peer (not an agent of this
    /// database), for advisories that warn without claiming.
    pub fn peer_hold(
        &self,
        task_id: &str,
        is_local: &dyn Fn(&str) -> bool,
    ) -> Result<Option<PeerHold>, CasError> {
        let key = self.key(task_id);
        let lock = self.coordinator("peer-claims-reader")?.get_lock(&key)?;
        Ok(lock
            .filter(|lock| !is_local(&lock.agent_id))
            .map(|lock| PeerHold {
                holder: ClaimHolder::from_reason(lock.claim_reason.as_deref()),
                agent_id: lock.agent_id,
                expires_at: Some(lock.expires_at),
            })
            .filter(|hold| !hold.is_expired(Utc::now())))
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
    pub failed: usize,
}

/// Renew the cloud claims of every task and epic `agent_id` holds a local
/// lease on, for the configured lease duration. Called from the daemon
/// heartbeat, so claims stay alive while the agent works and run out after
/// it stops.
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
    let duration = lease_duration_secs(cas_root);
    for lease in leases.iter().filter(|lease| lease.is_valid()) {
        match claims.renew(&lease.task_id, agent_id, duration) {
            Ok(()) => report.renewed += 1,
            Err(error) => {
                report.failed += 1;
                tracing::debug!(task = %lease.task_id, %error, "cloud claim renewal failed");
            }
        }
    }
    report
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

/// A live claim a peer holds on `task_id`, if the project is logged in and
/// the cloud answers. Read-only: for advisories such as `spawn_workers`.
pub fn peer_hold_for(cas_root: &Path, task_id: &str) -> Option<PeerHold> {
    let claims = PeerClaims::for_project(cas_root)?;
    let agents = crate::store::open_agent_store(cas_root).ok()?;
    let is_local = |id: &str| agents.get(id).is_ok();
    claims.peer_hold(task_id, &is_local).ok().flatten()
}

/// Claim the focus on `epic_id` for `agent_id` (a supervisor's
/// `focus_epic`). Returns the peer that already works it, if any; the focus
/// is never refused.
pub fn claim_epic_focus(cas_root: &Path, epic_id: &str, agent_id: &str) -> Option<PeerHold> {
    let claims = PeerClaims::for_project(cas_root)?;
    let agents = crate::store::open_agent_store(cas_root).ok()?;
    let name = agents
        .get(agent_id)
        .map(|agent| agent.name)
        .unwrap_or_else(|_| agent_id.to_string());
    let is_local = |id: &str| agents.get(id).is_ok();
    match claims.acquire(
        epic_id,
        ClaimKind::Epic,
        agent_id,
        &name,
        lease_duration_secs(cas_root),
        &is_local,
    ) {
        Acquire::PeerHolds(hold) => Some(hold),
        _ => None,
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
        let hold = PeerHold { agent_id: "a1".into(), holder: Some(holder), expires_at: None };
        assert_eq!(hold.describe(), format!("alpha-sup on {}", hold.holder.as_ref().unwrap().machine));
        let anonymous = PeerHold { agent_id: "a1".into(), holder: None, expires_at: None };
        assert_eq!(anonymous.describe(), "agent a1");
        assert_eq!(anonymous.name(), "a1");
    }

    #[test]
    fn a_hold_is_expired_only_past_its_expiry() {
        let now = Utc::now();
        let hold = |at| PeerHold { agent_id: "a".into(), holder: None, expires_at: at };
        assert!(hold(Some(now - chrono::Duration::seconds(1))).is_expired(now));
        assert!(!hold(Some(now + chrono::Duration::seconds(60))).is_expired(now));
        assert!(!hold(None).is_expired(now));
    }
}
