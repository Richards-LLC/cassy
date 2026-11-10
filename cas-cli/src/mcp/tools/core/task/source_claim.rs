//! Stakeholder-source ownership uses the existing Slack and advisory task claims.
use crate::cloud::peer_claims::github_source_key;
use crate::cloud::peer_claims::{Acquire, ClaimKind, PeerBudget, PeerClaims};
use crate::mcp::CasCore;
use crate::types::AgentRole;
use rmcp::{ErrorData as McpError, model::ErrorCode};

#[derive(Default)]
pub(crate) struct SourceAdmission {
    pub notice: String,
    pub override_grant: Option<cas_types::SourceClaimOverride>,
    // Retained through local publication, like the existing task-start fence.
    _fence: Option<crate::cloud::peer_claims::TaskClaimFence>,
}

impl From<String> for SourceAdmission {
    fn from(notice: String) -> Self {
        Self {
            notice,
            ..Default::default()
        }
    }
}

pub(crate) fn is_slack_source(source: &str) -> bool {
    url::Url::parse(source.trim()).is_ok_and(|url| {
        url.scheme() == "https"
            && url
                .host_str()
                .is_some_and(|host| host.ends_with(".slack.com"))
            && url.path().starts_with("/archives/")
    })
}

impl CasCore {
    /// An accepted owner-issued share grants work to its local supervisor
    /// and workers. Keep the task's source link intact; this grants no reply.
    pub(crate) fn source_split(&self, id: &str, source: Option<&str>) -> Option<String> {
        let task = self.open_task_store().ok()?.get(id).ok()?;
        if task.external_ref.as_deref() != source { return None; }
        crate::cloud::epic_split::current_epic_splits(&self.cas_root).ok()?
            .into_iter().find(|split| {
                split.state == cas_store::SplitState::Accepted
                    && split.mine.iter().any(|child| child == id)
                    // Resolving the actor may register it. A task without a
                    // matching split must leave the caller's identity alone.
                    && self.source_claim_actor()
                        .is_some_and(|actor| split.local_agent_id == actor.id)
            }).map(|split| split.id)
    }

    pub(crate) async fn task_source_admission_for(
        &self,
        task: &crate::types::Task,
        override_reason: Option<&str>,
    ) -> Result<SourceAdmission, McpError> {
        if task.execution_note.as_deref() == Some("no-code") {
            return Ok(SourceAdmission::default());
        }
        if let Some(split) = self.source_split(&task.id, task.external_ref.as_deref()) {
            return Ok(format!(
                "\nSource ownership: task {} is governed by epic split {split}; its delegation and takeover gates apply.",
                task.id
            ).into());
        }
        if let Some(grant) = task.deliverables.source_claim_override.as_ref()
            && task
                .external_ref
                .as_deref()
                .and_then(github_source_key)
                .as_deref()
                == Some(grant.source_key.as_str())
            && !grant.supervisor_id.is_empty()
            && !grant.reason.trim().is_empty()
        {
            return Ok(format!(
                "\nSource override authorized by {} for task {}: {}",
                grant.supervisor_id, task.id, grant.reason
            )
            .into());
        }
        self.task_source_admission(task.external_ref.as_deref(), &task.id, override_reason)
            .await
    }
    /// A worker acts under its registered supervisor's source claim. This does
    /// not authorize Slack mutations: that service still authenticates its caller.
    pub(crate) fn source_claim_actor(&self) -> Option<crate::types::Agent> {
        let agents = self.open_agent_store().ok()?;
        let actor = agents.get(&self.get_agent_id().ok()?).ok()?;
        if actor.role == AgentRole::Worker {
            let Some(session) = actor.factory_session.as_deref() else {
                return Some(actor);
            };
            let supervisors = agents
                .list(None)
                .ok()?
                .into_iter()
                .filter(|agent| {
                    agent.role == AgentRole::Supervisor
                        && agent.factory_session.as_deref() == Some(session)
                        && matches!(
                            agent.status,
                            crate::types::AgentStatus::Active | crate::types::AgentStatus::Idle
                        )
                })
                .collect::<Vec<_>>();
            if supervisors.len() == 1 {
                return Some(supervisors[0].clone());
            }
        }
        Some(actor)
    }

    /// Refuse a known competing owner before local task or staffing writes.
    /// Missing advisory infrastructure is visible, and never grants Slack authority.
    pub(crate) async fn task_source_admission(
        &self,
        source: Option<&str>,
        task_id: &str,
        override_reason: Option<&str>,
    ) -> Result<SourceAdmission, McpError> {
        let Some(source) =
            source.filter(|source| is_slack_source(source) || github_source_key(source).is_some())
        else {
            return Ok(SourceAdmission::default());
        };
        let unchecked = |why: &str| {
            SourceAdmission::from(format!(
                "\n⚠️ SOURCE OWNERSHIP NOT CHECKED — {source}: {why}. Coordinate with the peer supervisor before staffing this ask."
            ))
        };
        let Some(actor) = self.source_claim_actor() else {
            return Ok(unchecked("no registered source owner"));
        };
        if is_slack_source(source) {
            let journal =
                crate::coordination::external_work::SqliteRequestJournal::open(&self.cas_root)
                    .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
            let project = crate::cloud::resolve_canonical_id(&self.cas_root).unwrap_or_default();
            let receipt = journal
                .task_source_receipt(source, &project)
                .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
            if let Some(receipt) = receipt {
                let owner = &receipt["visible_owner"];
                if owner["registration_id"].as_str() == Some(actor.id.as_str()) {
                    return Ok(SourceAdmission::default());
                }
                let label = owner["label"]
                    .as_str()
                    .or(owner["registration_id"].as_str())
                    .unwrap_or("another supervisor");
                return Err(Self::error(
                    ErrorCode::INVALID_REQUEST,
                    format!(
                        "SOURCE HELD BY A PEER — {source} is owned by {label}. Ask the owner to offer this task via factory action=epic_split id=<epic-or-standalone-task> target=<your-supervisor> theirs={task_id}, then accept it before staffing. Thread reply authority stays with the owner."
                    ),
                ));
            }
            return Ok(unchecked(
                "no current Slack claim receipt; the supervisor must use factory action=claim external_ref=<source>",
            ));
        }
        let Some(claims) = PeerClaims::for_project(&self.cas_root) else {
            return Ok(unchecked("Cassy Cloud is not configured for this project"));
        };
        let key = github_source_key(source).expect("recognized GitHub source");
        let local = self
            .open_agent_store()
            .map(|agents| crate::cloud::peer_claims::local_agent_ids(agents.as_ref()))?;
        let root = self.cas_root.clone();
        let source_key = key.clone();
        let task_name = task_id.to_string();
        let budget = PeerBudget::default();
        let (answer, fence) = budget
            .run(move || {
                let claims = claims.with_budget(budget);
                let (mut answer, fence, _) = claims.acquire_for_local_lease(
                    &root,
                    &key,
                    ClaimKind::Task,
                    &actor.id,
                    &format!("{} (task {task_name})", actor.name),
                    crate::cloud::peer_claims::lease_duration_secs(&root),
                    &|id| local.contains(id),
                );
                // A legacy conflict envelope names only the agent. Source
                // refusals also need the task carried by the lock's reason.
                if matches!(answer, Acquire::PeerHolds(_))
                    && let Ok(Some(hold)) = claims.peer_hold(&key, &|id| local.contains(id))
                {
                    answer = Acquire::PeerHolds(hold);
                }
                if actor.role == AgentRole::Supervisor && actor.is_alive()
                    && let Acquire::PeerHolds(hold) = &answer
                {
                    answer = claims.take_source_for_task(
                        &key, &task_name, hold, &actor.id,
                        &format!("{} (task {task_name})", actor.name),
                        crate::cloud::peer_claims::lease_duration_secs(&root),
                    );
                }
                if answer == Acquire::Claimed {
                    // Existing agent metadata projects these claims into the usual
                    // heartbeat; no independent source lease table or protocol.
                    if let Err(error) =
                        crate::cloud::peer_claims::remember_source_claim(&root, &actor.id, &key)
                    {
                        return (
                            Acquire::Unavailable(format!(
                                "source claim acquired but heartbeat registration failed: {error}"
                            )),
                            fence,
                        );
                    }
                }
                (answer, fence)
            })
            .await
            .unwrap_or_else(|why| (Acquire::Unavailable(why), None));
        match answer {
            Acquire::Claimed | Acquire::LocalHolder(_) => Ok(SourceAdmission {
                _fence: fence,
                ..Default::default()
            }),
            Acquire::PeerHolds(hold) => {
                let conflict = format!(
                    "SOURCE HELD BY A PEER — {source} is owned by {} until {}",
                    hold.describe(),
                    hold.until()
                );
                if let Some(reason) = override_reason {
                    let supervisor = self.resolve_live_supervisor_authority().map_err(|error| {
                        Self::error(
                            ErrorCode::INVALID_REQUEST,
                            format!(
                                "Source override requires a live registered supervisor: {error:?}"
                            ),
                        )
                    })?;
                    let reason = reason.trim();
                    if reason.is_empty() {
                        return Err(Self::error(
                            ErrorCode::INVALID_PARAMS,
                            "Source override requires a non-empty reason.",
                        ));
                    }
                    let notice = format!(
                        "\nDECISION: SOURCE CLAIM OVERRIDE — task {task_id}; {conflict}; authorized by {}. Reason: {reason}",
                        supervisor.id
                    );
                    return Ok(SourceAdmission {
                        notice,
                        override_grant: Some(cas_types::SourceClaimOverride {
                            source_key,
                            supervisor_id: supervisor.id,
                            reason: reason.to_string(),
                        }),
                        _fence: fence,
                    });
                }
                Err(Self::error(
                    ErrorCode::INVALID_REQUEST,
                    format!(
                        "{conflict}. Ask the owner to hand off using factory action=release_claim resource_id=<GitHub URL>. A deliberate second task requires task create/update supervisor_override=true reason=<agreement>; the decision is recorded on that task."
                    ),
                ))
            }
            Acquire::StalePeer(hold) => Ok(unchecked(&format!(
                "stale source claim for {}; retry the ownership check",
                hold.describe()
            ))),
            Acquire::Unavailable(why) => Ok(unchecked(&why)),
        }
    }
}
