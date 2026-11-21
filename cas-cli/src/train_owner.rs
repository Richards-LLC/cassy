//! Authenticated access to the shared train authority. Task fields are a
//! projection; observing a peer or a stale heartbeat never grants ownership.
use std::path::Path;

use crate::types::{Agent, AgentRole, Task};
use crate::worktree::git::train_authority::{Identity, Phase, Snapshot};

pub(crate) const ENROLLED: &str = "train-owner-enrolled-v1";

/// Called after authenticating the accepted successor (or by sync after
/// fetching the shared authority). Never infer an accepted owner from task
/// clocks, notes or model-supplied state.
pub(crate) fn repair_projection(
    root: &Path,
    store: &dyn crate::store::TaskStore,
    train: &Task,
    snapshot: &Snapshot,
) -> Result<Task, String> {
    let live = authority(root, train)?.ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: authority missing")?;
    if live.tip != snapshot.tip {
        return Err("TRAIN_TRANSFER_CONFLICT: authority changed before projection".into());
    }
    let repo = crate::train_guardrails::train_repo_root(root, train).map_err(|e| e.to_string())?;
    if live.observed_target(&repo)? != live.record.target_sha {
        return Err("TRAIN_TRANSFER_TARGET_DIVERGED: projection cannot adopt target changes".into());
    }
    let record = &live.record;
    let projection = cas_types::TrainOwnerProjection {
        train: record.train.clone(), project: record.project.clone(), target: record.target.clone(),
        authority: live.tip.clone(), owner: record.identity.owner.clone(), incarnation: record.identity.incarnation.clone(),
        epoch: record.epoch, generation: record.generation.clone(),
        phase: match record.phase {
            Phase::Active => "active", Phase::Draining => "draining", Phase::Quiesced => "quiesced", Phase::RecoveryHeld => "recovery_held",
        }.into(),
    };
    if !store.project_train_owner(train, &projection).map_err(|e| e.to_string())? {
        return Err("TRAIN_OWNER_SYNC_CONFLICT: task changed before projection".into());
    }
    let confirmed = authority(root, train)?.ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: authority disappeared")?;
    if confirmed.tip != live.tip || confirmed.observed_target(&repo)? != confirmed.record.target_sha {
        return Err("TRAIN_TRANSFER_CONFLICT: authority changed during projection; repair from its current record".into());
    }
    store.get(&train.id).map_err(|e| e.to_string())
}

/// Sync cannot import an owner merely because its whole-row clock won LWW.
pub(crate) fn project_synced_owner(
    root: &Path,
    store: &dyn crate::store::TaskStore,
    incoming: &Task,
    local: &Task,
) -> Result<Task, String> {
    let snapshot = synced_owner_authority(root, incoming, local)?;
    repair_projection(root, store, local, &snapshot)
}

pub(crate) fn synced_owner_authority(root: &Path, incoming: &Task, local: &Task) -> Result<Snapshot, String> {
    let snapshot = authority(root, local)?.ok_or("TRAIN_OWNER_SYNC_CONFLICT: synced owner has no verified authority")?;
    if incoming.epic_verification_owner.as_deref() != Some(snapshot.record.identity.owner.as_str())
        || incoming.effective_epic_branch() != Some(snapshot.record.target.as_str())
        || !incoming.labels.iter().any(|label| label == ENROLLED)
        || !crate::mcp::tools::core::task::release_train::is_release_train(incoming)
        || incoming.deliverables.work_target != local.deliverables.work_target
    {
        return Err("TRAIN_OWNER_SYNC_CONFLICT: incoming owner or train scope disagrees with authority".into());
    }
    Ok(snapshot)
}

pub(crate) fn authority(root: &Path, train: &Task) -> Result<Option<Snapshot>, String> {
    let enrolled = train.labels.iter().any(|label| label == ENROLLED);
    if !enrolled && !crate::mcp::tools::core::task::release_train::is_release_train(train) {
        return Ok(None);
    }
    let repo = crate::train_guardrails::train_repo_root(root, train).map_err(|e| e.to_string())?;
    let authority = Snapshot::read_optional(&repo, &train.id)?;
    if enrolled && authority.is_none() {
        return Err("TRAIN_TRANSFER_FENCE_UNAVAILABLE: enrolled authority is missing".into());
    }
    if let Some(authority) = &authority {
        let project = crate::cloud::resolve_canonical_id(root)
            .ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: canonical project is unavailable")?;
        if authority.record.project != project
            || Some(authority.record.target.as_str()) != train.effective_epic_branch()
        {
            return Err(
                "TRAIN_TRANSFER_FENCE_UNAVAILABLE: authority project or target mismatch".into(),
            );
        }
    }
    Ok(authority)
}

pub(crate) fn identity(actor: &Agent, logical_owner: &str) -> Result<Identity, String> {
    let machine = actor
        .machine_id
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: actor machine is unknown")?;
    let session = actor
        .factory_session
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: actor factory session is unknown")?;
    let pid = actor
        .pid
        .filter(|pid| *pid > 1)
        .ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: actor process is unknown")?;
    let start = actor
        .pid_starttime
        .filter(|stamp| *stamp > 0)
        .ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: actor process fingerprint is unknown")?;
    Ok(Identity {
        owner: logical_owner.into(),
        incarnation: serde_json::to_string(&(
            &actor.id,
            machine,
            session,
            &actor.cc_session_id,
            pid,
            start,
        ))
        .map_err(|e| e.to_string())?,
    })
}

pub(crate) fn require_owner(
    root: &Path,
    train: &Task,
    actor: &Agent,
    snapshot: &Snapshot,
) -> Result<String, String> {
    let owner = &snapshot.record.identity.owner;
    if actor.role != AgentRole::Supervisor
        || !crate::cloud::peer_mailbox::owns_recipient(root, actor, owner)
    {
        return Err(crate::gates::tag(
            "G-113",
            "TRAIN_OWNER_GENERATION_CHANGED: caller is not the authenticated train owner",
        ));
    }
    if train.epic_verification_owner.as_deref() != Some(owner.as_str()) {
        return Err(crate::gates::tag(
            "G-113",
            "TRAIN_OWNER_SYNC_CONFLICT: repair the owner projection from the accepted transfer",
        ));
    }
    let identity = identity(actor, owner)?;
    let mut live = snapshot.clone();
    if live.record.identity != identity {
        // A different registered id must already own the predecessor's mailbox
        // through its verified retirement chain. Reusing an id/fingerprint or
        // matching a name cannot refresh a process incarnation.
        if actor.id == *owner {
            return Err(crate::gates::tag(
                "G-113",
                "TRAIN_OWNER_GENERATION_CHANGED: actor incarnation changed without a predecessor claim",
            ));
        }
        let repo =
            crate::train_guardrails::train_repo_root(root, train).map_err(|e| e.to_string())?;
        live = live.refresh_incarnation(&repo, &live.record.identity, &identity)?;
    }
    if live.record.phase != Phase::Active {
        return Err(crate::gates::tag(
            "G-113",
            "TRAIN_OWNER_DRAINING: admissions are held",
        ));
    }
    let repo = crate::train_guardrails::train_repo_root(root, train).map_err(|e| e.to_string())?;
    live.validate_admission(&repo, &identity)?;
    Ok(live.record.generation)
}

pub(crate) fn generation_matches(
    root: &Path,
    train: &Task,
    queued: &str,
    current: &str,
) -> Result<bool, String> {
    Ok(queued == current || (queued.is_empty() && authority(root, train)?.is_none()))
}

pub(crate) fn require_active(root: &Path, train: &Task) -> Result<(), String> {
    if let Some(snapshot) = authority(root, train)? {
        let repo =
            crate::train_guardrails::train_repo_root(root, train).map_err(|e| e.to_string())?;
        snapshot.validate_admission(&repo, &snapshot.record.identity)?;
    }
    Ok(())
}
