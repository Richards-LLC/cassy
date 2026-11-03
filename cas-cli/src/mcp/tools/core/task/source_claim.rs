//! Stakeholder-source ownership uses the existing Slack and advisory task claims.
use crate::cloud::peer_claims::{Acquire, ClaimKind, PeerBudget, PeerClaims};
use crate::mcp::CasCore;
use crate::types::AgentRole;
use rmcp::{ErrorData as McpError, model::ErrorCode};
use sha2::{Digest, Sha256};

/// GitHub issues and PRs share one numeric namespace, including their subpages.
/// Other external refs (including no-code proof artifacts) are not ask claims.
pub(crate) fn github_source_key(source: &str) -> Option<String> {
    let url = url::Url::parse(source.trim()).ok()?;
    if url.scheme() != "https" || url.host_str()? != "github.com" || !url.username().is_empty() {
        return None;
    }
    let parts = url.path_segments()?.collect::<Vec<_>>();
    if parts.len() < 4 || !matches!(parts[2], "issues" | "pull") {
        return None;
    }
    let number = parts[3].parse::<u64>().ok().filter(|number| *number > 0)?;
    if parts[0].is_empty() || parts[1].is_empty() { return None; }
    let identity = format!("github.com/{}/{}/{number}", parts[0].to_ascii_lowercase(), parts[1].to_ascii_lowercase());
    let digest = Sha256::digest(identity.as_bytes());
    Some(format!("ask-{}", digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>()))
}

pub(crate) fn is_slack_source(source: &str) -> bool {
    url::Url::parse(source.trim()).is_ok_and(|url| {
        url.scheme() == "https" && url.host_str().is_some_and(|host| host.ends_with(".slack.com"))
            && url.path().starts_with("/archives/")
    })
}

impl CasCore {
    /// A worker acts under its registered supervisor's source claim. This does
    /// not authorize Slack mutations: that service still authenticates its caller.
    pub(crate) fn source_claim_actor(&self) -> Option<crate::types::Agent> {
        let agents = self.open_agent_store().ok()?;
        let actor = agents.get(&self.get_agent_id().ok()?).ok()?;
        if actor.role == AgentRole::Worker {
            let Some(session) = actor.factory_session.as_deref() else { return Some(actor); };
            let supervisors = agents.list(None).ok()?.into_iter().filter(|agent| {
                agent.role == AgentRole::Supervisor && agent.factory_session.as_deref() == Some(session)
                    && matches!(agent.status, crate::types::AgentStatus::Active | crate::types::AgentStatus::Idle)
            }).collect::<Vec<_>>();
            if supervisors.len() == 1 { return Some(supervisors[0].clone()); }
        }
        Some(actor)
    }

    /// Refuse a known competing owner before local task or staffing writes.
    /// Missing advisory infrastructure is visible, and never grants Slack authority.
    pub(crate) async fn task_source_notice(&self, source: Option<&str>) -> Result<String, McpError> {
        let Some(source) = source.filter(|source| is_slack_source(source) || github_source_key(source).is_some()) else {
            return Ok(String::new());
        };
        let unchecked = |why: &str| format!("\n⚠️ SOURCE OWNERSHIP NOT CHECKED — {source}: {why}. Coordinate with the peer supervisor before staffing this ask.");
        let Some(actor) = self.source_claim_actor() else { return Ok(unchecked("no registered source owner")); };
        if is_slack_source(source) {
            let journal = crate::coordination::external_work::SqliteRequestJournal::open(&self.cas_root)
                .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
            let project = crate::cloud::resolve_canonical_id(&self.cas_root).unwrap_or_default();
            let receipt = journal.task_source_receipt(source, &project)
                .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
            if let Some(receipt) = receipt {
                let owner = &receipt["visible_owner"];
                if owner["registration_id"].as_str() == Some(actor.id.as_str()) {
                    return Ok(String::new());
                }
                let label = owner["label"].as_str().or(owner["registration_id"].as_str()).unwrap_or("another supervisor");
                return Err(Self::error(ErrorCode::INVALID_REQUEST, format!(
                    "SOURCE HELD BY A PEER — {source} is owned by {label}. Coordinate with that owner or use the existing claim handoff before creating or staffing this ask."
                )));
            }
            return Ok(unchecked("no current Slack claim receipt; the supervisor must use coordination action=claim external_ref=<source>"));
        }
        let Some(claims) = PeerClaims::for_project(&self.cas_root) else {
            return Ok(unchecked("Cassy Cloud is not configured for this project"));
        };
        let key = github_source_key(source).expect("recognized GitHub source");
        let local = self.open_agent_store().map(|agents| crate::cloud::peer_claims::local_agent_ids(agents.as_ref()))?;
        let root = self.cas_root.clone();
        let budget = PeerBudget::default();
        let answer = budget.run(move || {
            let answer = claims.with_budget(budget).acquire(&key, ClaimKind::Task, &actor.id, &actor.name,
                crate::cloud::peer_claims::lease_duration_secs(&root), &|id| local.contains(id));
            if answer == Acquire::Claimed {
                // Existing agent metadata projects these claims into the usual
                // heartbeat; no independent source lease table or protocol.
                if let Ok(agents) = crate::store::open_agent_store(&root)
                    && let Ok(mut agent) = agents.get(&actor.id)
                {
                    let mut keys: Vec<String> = agent.metadata.get(crate::cloud::peer_claims::META_SOURCE_CLAIMS)
                        .and_then(|value| serde_json::from_str(value).ok()).unwrap_or_default();
                    if !keys.contains(&key) {
                        keys.push(key);
                        agent.metadata.insert(crate::cloud::peer_claims::META_SOURCE_CLAIMS.into(), serde_json::to_string(&keys).unwrap_or_default());
                        let _ = agents.update(&agent);
                    }
                }
            }
            answer
        }).await.unwrap_or_else(Acquire::Unavailable);
        match answer {
            Acquire::Claimed | Acquire::LocalHolder(_) => Ok(String::new()),
            Acquire::PeerHolds(hold) => Err(Self::error(ErrorCode::INVALID_REQUEST, format!(
                "SOURCE HELD BY A PEER — {source} is owned by {} until {}. Coordinate with {} before creating or staffing this ask.", hold.describe(), hold.until(), hold.name()
            ))),
            Acquire::StalePeer(hold) => Ok(unchecked(&format!("stale source claim for {}; retry the ownership check", hold.describe()))),
            Acquire::Unavailable(why) => Ok(unchecked(&why)),
        }
    }
}
