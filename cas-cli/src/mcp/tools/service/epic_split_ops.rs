//! cas-4736: `factory action=epic_split`, splitting an epic's children with
//! a peer supervisor of this repository on another machine or clone.
//!
//! - propose: `id=<epic> target=<peer> [mine=<ids>] [theirs=<ids>] [ttl_secs]`
//!   At least one share is required; omit the other for a request or grant.
//! - answer: `id=<split> decision=accept|reject [reason]`
//! - cancel: `id=<split> decision=cancel [reason]`
//! - status: `id=<split> [decision=status]`
//! - list: no arguments
//!
//! A registered supervisor may cancel its own split. A live supervisor may
//! also supersede an accepted split when its registered owner is no longer live
//! past the heartbeat TTL; that cancellation records both identities.

use crate::cloud::epic_split::{self, Caller};
use crate::mcp::tools::service::imports::*;

impl CasService {
    pub(super) async fn factory_epic_split(
        &self,
        req: FactoryRequest,
    ) -> Result<CallToolResult, McpError> {
        let cas_root = self.inner.cas_root.clone();
        let agent_id = self.inner.get_agent_id()?;
        let agent = crate::store::open_agent_store(&cas_root)
            .ok()
            .and_then(|store| store.get(&agent_id).ok())
            .filter(|agent| agent.role == cas_types::AgentRole::Supervisor)
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_REQUEST,
                    "epic_split is for a registered supervisor: it splits an epic between \
                     supervisors, never for a worker",
                )
            })?;
        let caller = Caller {
            id: agent.id,
            name: agent.name,
        };
        let trimmed = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let id = trimmed(&req.id);
        let decision = trimmed(&req.decision).map(|d| d.to_ascii_lowercase());
        let target = trimmed(&req.target);
        let reason = trimmed(&req.reason);
        let (mine, theirs) = (
            req.mine.as_deref().map(epic_split::parse_ids),
            req.theirs.as_deref().map(epic_split::parse_ids),
        );
        let ttl_secs = req.ttl_secs;

        let outcome = tokio::task::spawn_blocking(move || -> Result<String, String> {
            match (decision.as_deref(), id) {
                (Some(decision), Some(id)) => match decision {
                    "accept" => epic_split::answer(&cas_root, &caller, &id, true, reason.as_deref()),
                    "reject" => epic_split::answer(&cas_root, &caller, &id, false, reason.as_deref()),
                    "cancel" => epic_split::cancel(&cas_root, &caller, &id, reason.as_deref()),
                    "status" => epic_split::status(&cas_root, &id),
                    other => Err(format!(
                        "decision must be accept, reject or cancel, or status (got '{other}')"
                    )),
                },
                (Some(_), None) => Err("decision needs id=<split id>".to_string()),
                (None, Some(epic)) => {
                    if target.is_none() && epic.starts_with("split-") {
                        return epic_split::status(&cas_root, &epic);
                    }
                    let peer = target.ok_or(
                        "propose an epic split with id=<epic> target=<peer supervisor> \
                         mine=<ids> or theirs=<ids> (or both)",
                    )?;
                    epic_split::propose(
                        &cas_root,
                        &caller,
                        &epic,
                        &peer,
                        &mine.unwrap_or_default(),
                        &theirs.unwrap_or_default(),
                        ttl_secs,
                    )
                }
                (None, None) => {
                    let splits = epic_split::render_splits(&cas_root);
                    Ok(if splits.is_empty() {
                        "No epic splits recorded. Propose one with factory action=epic_split \
                         id=<epic> target=<peer> mine=<ids> or theirs=<ids> (or both)."
                            .to_string()
                    } else {
                        splits
                    })
                }
            }
        })
        .await
        .map_err(|error| Self::error(ErrorCode::INTERNAL_ERROR, error.to_string()))?;
        outcome
            .map(Self::success)
            .map_err(|message| Self::error(ErrorCode::INVALID_PARAMS, message))
    }
}
