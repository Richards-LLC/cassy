//! Shared CLI/MCP transfer handler. Caller and successor fingerprints come
//! from registration; task ownership is repaired only after Git acceptance.
use std::{path::Path, time::Duration};

use cas_mcp::types::{TrainOwnerTransferAction as Action, TrainOwnerTransferRequest as Request};
use serde_json::{Value, json};

use crate::{mcp::CasCore, types::{Agent, AgentRole, Task}};
use crate::worktree::git::train_authority::{Identity, Phase, Snapshot, TransferChecks};

pub(crate) const REMOTE_IDENTITY: &str = "train_owner_identity_v1";

fn refusal(message: impl AsRef<str>) -> String {
    crate::gates::tag("G-113", message.as_ref())
}

/// Authenticates a transfer participant even when the task projection still
/// names the prior owner. Only the existing verified predecessor chain can
/// replace the recorded process; matching a name or reusing its id cannot.
fn authenticate(root: &Path, repo: &Path, actor: &Agent, snapshot: Snapshot, expected: &Identity) -> Result<(Snapshot, Identity), String> {
    if actor.role != AgentRole::Supervisor || !actor.is_alive()
        || !crate::cloud::peer_mailbox::owns_recipient(root, actor, &expected.owner)
    {
        return Err(refusal("TRAIN_OWNER_GENERATION_CHANGED: caller does not own the registered participant"));
    }
    let identity = super::identity(actor, &expected.owner)?;
    if identity == *expected {
        return Ok((snapshot, identity));
    }
    if actor.id == expected.owner {
        return Err(refusal("TRAIN_OWNER_GENERATION_CHANGED: process incarnation lacks a predecessor claim"));
    }
    // A successor named by an in-flight transfer cannot be silently rebound.
    // After accept the normal authenticated same-owner restart path applies.
    if snapshot.record.identity.owner != expected.owner {
        return Err(refusal("TRAIN_TRANSFER_TARGET_MISMATCH: prepare again for the registered successor incarnation"));
    }
    let refreshed = snapshot.refresh_incarnation(repo, expected, &identity)?;
    Ok((refreshed, identity))
}

pub(super) fn resolve_successor(root: &Path, caller: Option<&Agent>, id: &str) -> Result<Identity, String> {
    let machine = caller.and_then(|actor| actor.machine_id.clone())
        .unwrap_or_else(Agent::get_or_generate_machine_id);
    let agents = crate::store::open_agent_store(root).map_err(|e| e.to_string())?;
    if let Ok(agent) = agents.get(id) {
        if agent.machine_id.as_deref() == Some(machine.as_str()) && agent.role == AgentRole::Supervisor && agent.is_alive() {
            return super::identity(&agent, id);
        }
        // A locally registered id must not fall through to an unrelated cloud
        // row when its role, machine or liveness disagrees.
        return Err(refusal("TRAIN_TRANSFER_TARGET_MISMATCH: successor registration is not live on its recorded machine"));
    }
    let project = crate::cloud::resolve_canonical_id(root)
        .ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: canonical project missing")?;
    let config = crate::cloud::CloudConfig::load_from_cas_dir(root).map_err(|e| e.to_string())?;
    let rows = crate::cloud::CloudCoordinator::new(config).map_err(|e| e.to_string())?
        .with_timeout(Duration::from_secs(15)).with_canonical_id(Some(project.clone()))
        .list_agent_infos().map_err(|e| e.to_string())?;
    registered_remote_identity(&rows, &project, caller.map_or("", |actor| actor.id.as_str()), id)
}

fn registered_remote_identity(rows: &[crate::cloud::AgentInfo], project: &str, caller: &str, id: &str) -> Result<Identity, String> {
    let user = crate::cloud::peers::caller_user_id(rows, Some(caller))?;
    let peers = crate::cloud::peers::repo_peers(rows, project, &[], None, chrono::Utc::now());
    let row = rows.iter().find(|row| row.id == id)
        .ok_or("TRAIN_TRANSFER_TARGET_MISMATCH: successor is not registered")?;
    if !peers.iter().any(|peer| peer.agent_id == id && peer.live && peer.user_id.as_deref() == Some(&user)) {
        return Err(refusal("TRAIN_TRANSFER_TARGET_MISMATCH: successor account, project or live registration disagrees"));
    }
    let encoded = row.metadata.get(REMOTE_IDENTITY)
        .ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: successor runtime has no registered fingerprint")?;
    let (agent, machine, session, harness, pid, start): (String, String, String, Option<String>, u32, u64) =
        serde_json::from_str(encoded).map_err(|_| "TRAIN_TRANSFER_FENCE_UNAVAILABLE: invalid successor fingerprint")?;
    if agent != id || Some(machine.as_str()) != row.machine_id.as_deref()
        || Some(session.as_str()) != row.metadata.get(crate::cloud::peers::META_FACTORY_SESSION).map(String::as_str)
        || Some(pid) != row.pid || pid <= 1 || start == 0 || machine.is_empty() || session.is_empty()
        || harness.as_deref().is_none_or(str::is_empty)
    {
        return Err(refusal("TRAIN_TRANSFER_TARGET_MISMATCH: registered successor fingerprint disagrees"));
    }
    Ok(Identity { owner: id.into(), incarnation: encoded.clone() })
}

impl CasCore {
    pub(crate) fn train_owner_transfer(&self, train_id: &str, request: &Request) -> Result<String, String> {
        let tasks = self.open_task_store().map_err(|e| e.to_string())?;
        let train = tasks.get(train_id).map_err(|e| e.to_string())?;
        let repo = crate::train_guardrails::train_repo_root(&self.cas_root, &train).map_err(|e| e.to_string())?;
        let checks = super::provider::GitHubChecks::new(&repo)?;
        self.train_owner_transfer_checked(&train, request, &checks)
            .and_then(|receipt| serde_json::to_string(&receipt).map_err(|e| e.to_string()))
    }

    // The provider seam replaces network I/O in tests, never registration,
    // Git, projection, generation checks or the provider's state predicates.
    pub(crate) fn train_owner_transfer_checked(&self, train: &Task, request: &Request, checks: &impl TransferChecks) -> Result<Value, String> {
        let actor = self.resolve_live_supervisor_authority().map_err(|e| refusal(format!("TRAIN_OWNER_GENERATION_CHANGED: {e:?}")))?;
        if !crate::mcp::tools::core::task::release_train::is_release_train(train) || train.is_terminal() {
            return Err(refusal("TRAIN_TRANSFER_CONFLICT: transfer requires an open release train"));
        }
        if [&request.reason, &request.expected_authority, &request.expected_generation, &request.transfer_id]
            .iter().any(|s| s.trim().is_empty()) || uuid::Uuid::parse_str(&request.request_id).is_err()
            || (request.operation == Action::Prepare) != request.successor.is_some()
        {
            return Err(refusal("TRAIN_TRANSFER_CONFLICT: incomplete transfer scope or audit data"));
        }
        let repo = crate::train_guardrails::train_repo_root(&self.cas_root, train).map_err(|e| e.to_string())?;
        let mut snapshot = super::authority(&self.cas_root, train)?
            .ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: enroll the train before transferring it")?;
        let accepted_retry = request.operation == Action::Accept && snapshot.record.phase == Phase::Active
            && snapshot.record.accepted.as_ref().is_some_and(|a| a.transfer_id == request.transfer_id
                && a.prior_generation == request.expected_generation && a.generation == snapshot.record.generation);
        let prepared_retry = request.operation == Action::Prepare
            && matches!(snapshot.record.phase, Phase::Draining | Phase::Quiesced)
            && snapshot.record.transfer.as_ref().is_some_and(|t| t.id == request.transfer_id
                && Some(t.successor.owner.as_str()) == request.successor.as_deref());
        let git = crate::worktree::GitOperations::new(repo.clone());
        if (!accepted_retry && snapshot.record.generation != request.expected_generation)
            || (snapshot.tip != request.expected_authority
                && (!(accepted_retry || prepared_retry) || !git.is_ancestor(&request.expected_authority, &snapshot.tip)))
        {
            return Err(refusal("TRAIN_TRANSFER_CONFLICT: expected authority or generation changed"));
        }
        let expected_actor = if request.operation == Action::Accept && !accepted_retry {
            snapshot.record.transfer.as_ref().filter(|t| t.id == request.transfer_id)
                .map(|t| t.successor.clone()).ok_or("TRAIN_TRANSFER_CONFLICT: transfer id changed")?
        } else { snapshot.record.identity.clone() };
        let (authenticated, identity) = authenticate(&self.cas_root, &repo, &actor, snapshot, &expected_actor)?;
        snapshot = authenticated.audited(json!({
            "request_id": request.request_id, "reason": request.reason, "actor": actor.id,
            "incarnation": identity.incarnation, "train": train.id, "project": authenticated_project(&self.cas_root)?,
            "expected_authority": request.expected_authority, "expected_generation": request.expected_generation,
            "operation": request.operation, "observed_at": chrono::Utc::now(),
        }));
        // authenticate() intentionally precedes the ordinary projected-owner
        // check. B may have accepted remotely and crashed before projection.
        let result = match request.operation {
            Action::Prepare => {
                let successor = resolve_successor(&self.cas_root, Some(&actor), request.successor.as_deref().unwrap())?;
                snapshot.prepare(&repo, &identity, &successor, &request.transfer_id)?
            }
            Action::Quiesce | Action::Abort => {
                if !snapshot.record.transfer.as_ref().is_some_and(|t| t.id == request.transfer_id) {
                    return Err(refusal("TRAIN_TRANSFER_CONFLICT: transfer id changed"));
                }
                if request.operation == Action::Quiesce { snapshot.quiesce(&repo, &identity, checks)? }
                else { snapshot.abort(&repo, &identity, checks)? }
            }
            Action::Accept => snapshot.accept(&repo, &identity, &request.transfer_id, &request.expected_generation, checks)?,
        };
        let tasks = self.open_task_store().map_err(|e| e.to_string())?;
        let current = tasks.get(&train.id).map_err(|e| e.to_string())?;
        super::repair_projection(&self.cas_root, tasks.as_ref(), &current, &result)?;
        Ok(json!({"train": train.id, "authority": result.tip, "generation": result.record.generation,
            "phase": result.record.phase, "owner": result.record.identity.owner, "transfer_id": request.transfer_id}))
    }
}

fn authenticated_project(root: &Path) -> Result<String, String> {
    crate::cloud::resolve_canonical_id(root).ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: project identity missing".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_successor_uses_registered_scope_and_fingerprint_cas_6456() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let mut successor = Agent::new_with_role("successor".into(), "successor".into(), AgentRole::Supervisor);
        successor.machine_id = Some("peer-machine".into());
        successor.factory_session = Some("peer-session".into());
        successor.cc_session_id = Some("peer-harness".into());
        successor.pid = Some(std::process::id());
        crate::mcp::daemon::stamp_pid_fingerprint(&mut successor, std::process::id());
        let row: crate::cloud::AgentInfo = serde_json::from_value(json!({
            "id": successor.id, "name": successor.name, "agent_type": "primary", "status": "active",
            "pid": successor.pid, "session_id": null, "machine_id": successor.machine_id,
            "last_heartbeat": chrono::Utc::now().to_rfc3339(), "active_tasks": 0, "user_id": "account",
            "metadata": crate::cloud::peers::identity_metadata(&successor, Some("fixture"), None, None)
        })).unwrap();
        let mut caller = row.clone(); caller.id = "caller".into();
        let rows = vec![caller.clone(), row.clone()];
        assert_eq!(registered_remote_identity(&rows, "fixture", "caller", "successor").unwrap(),
            super::super::identity(&successor, &successor.id).unwrap());
        for variant in 0..6 {
            let mut invalid = row.clone();
            match variant {
                0 => invalid.user_id = Some("foreign-account".into()),
                1 => { invalid.metadata.insert(crate::cloud::peers::META_CANONICAL_ID.into(), "foreign-project".into()); },
                2 => invalid.machine_id = Some("foreign-machine".into()),
                3 => { invalid.metadata.remove(REMOTE_IDENTITY); },
                4 => invalid.status = "shutdown".into(),
                _ => { invalid.metadata.insert(REMOTE_IDENTITY.into(), "forged".into()); },
            }
            assert!(registered_remote_identity(&[caller.clone(), invalid], "fixture", "caller", "successor").is_err());
        }
    }
}
