//! Reserve generated worker identities before task/queue binding. Names keep
//! N1 (cas-dc08): no new worker shares another's `factory/` namespace. All Git and
//! store reads run off the MCP executor, never on the daemon's render loop.
use std::collections::HashSet;
use std::path::{Path, PathBuf};

struct Reserved {
    names: crate::factory_isolation::WorkerNameRegistry,
    identities: HashSet<String>,
}

fn reserved(root: &Path, task_id: Option<&str>) -> Result<Reserved, String> {
    use cas_store::SpawnQueueStore;
    let session_repo = crate::worktree::GitOperations::detect_repo_root(root)
        .map_err(|error| format!("spawn refused: cannot inspect factory branch names in {}: {error}. Git and a valid repository are required for isolated workers; no worker was queued.", root.display()))?;
    let repo = crate::ui::factory::spawn_worktree_repo_root(root, &session_repo, task_id)?;
    let mut names = crate::factory_isolation::WorkerNameRegistry::load(root, &repo)?;
    let agents = crate::store::open_agent_store(root).map_err(|error| error.to_string())?;
    let mut identities: HashSet<String> = agents
        .list(None)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|agent| agent.name)
        .collect();
    let queue = crate::store::open_spawn_queue_store(root).map_err(|error| error.to_string())?;
    for pending in queue
        .peek(super::SPAWN_QUEUE_DUPLICATE_SCAN)
        .map_err(|error| error.to_string())?
    {
        names.workers.extend(pending.worker_names.iter().cloned());
        identities.extend(pending.worker_names);
    }
    Ok(Reserved { names, identities })
}

pub(super) async fn allocate(
    root: PathBuf,
    task_id: Option<String>,
    requested: Vec<Option<String>>,
    next_name: &mut (dyn FnMut() -> String + Send),
) -> Result<Vec<String>, String> {
    let mut reserved = tokio::task::spawn_blocking(move || reserved(&root, task_id.as_deref()))
        .await
        .map_err(|error| format!("worker name preflight stopped: {error}"))??;
    let mut chosen = Vec::with_capacity(requested.len());
    for name in requested {
        let candidate = if let Some(name) = name {
            if let Some(reference) = reserved.names.refs.get(&name) {
                return Err(format!(
                    "spawn refused: explicit worker name {name:?} collides with existing {reference} (factory/{name}). Choose a fresh worker_names value; no worker was queued. Use recycle_worker for recovery of an existing worker."
                ));
            }
            if chosen.contains(&name) {
                return Err(format!(
                    "spawn refused: worker name {name:?} appears twice in this request; no worker was queued."
                ));
            }
            if let Some(overlap) = reserved.names.overlap(&name) {
                return Err(format!("spawn refused: {overlap} No worker was queued."));
            }
            name
        } else {
            // A finite bound also handles an exhausted or broken name source.
            (0..100).find_map(|_| {
                let candidate = next_name();
                (!reserved.names.refs.contains_key(&candidate)
                    && !reserved.identities.contains(&candidate)
                    && reserved.names.overlap(&candidate).is_none())
                    .then_some(candidate)
            }).ok_or_else(|| "spawn refused: no fresh worker name after 100 attempts; supply an unused worker_names value. No worker was queued.".to_string())?
        };
        reserved.identities.insert(candidate.clone());
        reserved.names.workers.push(candidate.clone());
        chosen.push(candidate);
    }
    Ok(chosen)
}
