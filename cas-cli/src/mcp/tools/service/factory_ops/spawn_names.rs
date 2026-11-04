//! Reserve generated worker identities before task/queue binding. All Git and
//! store reads run off the MCP executor, never on the daemon's render loop.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

struct Reserved {
    refs: HashMap<String, String>,
    identities: HashSet<String>,
}

fn reserved(root: &Path, task_id: Option<&str>) -> Result<Reserved, String> {
    use cas_store::SpawnQueueStore;
    let session_repo = crate::worktree::GitOperations::detect_repo_root(root)
        .map_err(|error| format!("spawn refused: cannot inspect factory branch names in {}: {error}. Git and a valid repository are required for isolated workers; no worker was queued.", root.display()))?;
    let repo = crate::ui::factory::spawn_worktree_repo_root(root, &session_repo, task_id)?;
    let mut command = Command::new("git");
    command.current_dir(&repo).args([
        "for-each-ref",
        "--format=%(refname)",
        "refs/heads/factory/",
        "refs/remotes/origin/factory/",
    ]);
    let budget = Duration::from_secs(5);
    let output = crate::bounded_process::run_command(
        &mut command,
        crate::bounded_process::Deadline::after(budget),
        budget,
    )
    .map_err(|error| {
        format!(
            "could not inspect worker branch names in {}: {error:?}",
            repo.display()
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "could not inspect worker branch names in {}: {}",
            repo.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let refs = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|reference| {
            reference
                .strip_prefix("refs/heads/factory/")
                .or_else(|| reference.strip_prefix("refs/remotes/origin/factory/"))
                .map(|name| (name.to_string(), reference.to_string()))
        })
        .collect();
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
        identities.extend(pending.worker_names);
    }
    Ok(Reserved { refs, identities })
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
            if let Some(reference) = reserved.refs.get(&name) {
                return Err(format!(
                    "spawn refused: explicit worker name {name:?} collides with existing {reference} (factory/{name}). Choose a fresh worker_names value; no worker was queued. Use recycle_worker for recovery of an existing worker."
                ));
            }
            if chosen.contains(&name) {
                return Err(format!(
                    "spawn refused: worker name {name:?} appears twice in this request; no worker was queued."
                ));
            }
            name
        } else {
            // A finite bound also handles an exhausted or broken name source.
            (0..100).find_map(|_| {
                let candidate = next_name();
                (!reserved.refs.contains_key(&candidate) && !reserved.identities.contains(&candidate)).then_some(candidate)
            }).ok_or_else(|| "spawn refused: no fresh worker name after 100 attempts; supply an unused worker_names value. No worker was queued.".to_string())?
        };
        reserved.identities.insert(candidate.clone());
        chosen.push(candidate);
    }
    Ok(chosen)
}
