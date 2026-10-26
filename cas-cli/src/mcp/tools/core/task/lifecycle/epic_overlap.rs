//! Pre-write epic admission (GH #1316). A peer's claim must never hide its
//! plan from another supervisor. Read Cloud's complete project inventory, not
//! the assignable-task view, and leave both inventories untouched.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use cas_store::TaskStore;

use crate::cloud::{CloudConfig, CloudSyncer, CloudSyncerConfig, SyncQueue};
use crate::mcp::server::CasCore;
use crate::types::{DependencyType, Scope, Task, TaskStatus, TaskType};

// Unlike individual tasks, epics describe outcomes in prose. Require at least
// five shared content terms and 45% Jaccard overlap; generic planning language
// and a shared project name alone cannot carry the warning. Exact/contained
// titles retain the existing title threshold, including short epic titles.
const SCOPE_THRESHOLD: f64 = 0.45;
const MIN_SHARED_TERMS: usize = 5;

fn scope_terms(title: &str, description: &str) -> BTreeSet<String> {
    format!("{title}\n{description}")
        .split_whitespace()
        // Apply recall's noise floor per token, without its 16-term query cap:
        // an epic's substantive scope may follow a long introductory title.
        .flat_map(crate::ambient_recall::content_terms)
        .filter(|term| {
            !super::is_generic_task_token(term)
                && !matches!(
                    term.as_str(),
                    "epic" | "plan" | "planning" | "scope" | "work" | "implementation"
                )
                && !term.chars().all(|ch| ch.is_ascii_digit())
        })
        .collect()
}

fn similarity(title: &str, description: &str, epic: &Task) -> Option<f64> {
    let title_score = super::title_similarity(title, &epic.title);
    if title_score >= super::DUPLICATE_TITLE_SIMILARITY_THRESHOLD {
        return Some(title_score);
    }
    let left = scope_terms(title, description);
    let right = scope_terms(&epic.title, &epic.description);
    let shared = left.intersection(&right).count();
    if shared < MIN_SHARED_TERMS {
        return None;
    }
    let score = shared as f64 / left.union(&right).count() as f64;
    (score >= SCOPE_THRESHOLD).then_some(score)
}

fn open(task: &Task) -> bool {
    !matches!(task.status, TaskStatus::Closed | TaskStatus::Cancelled)
}

#[derive(Default)]
struct CloudInventory {
    tasks: Vec<Task>,
    edges: Vec<serde_json::Value>,
}

async fn cloud_inventory(
    root: &std::path::Path,
    project: Option<&str>,
) -> Result<CloudInventory, String> {
    let config = CloudConfig::load_from_cas_dir(root).map_err(|_| inventory_warning())?;
    if !config.is_logged_in() {
        return Ok(CloudInventory::default());
    }
    config
        .validate_team_only()
        .map_err(|_| inventory_warning())?;
    let project = project.ok_or_else(inventory_warning)?.to_owned();
    let root = root.to_owned();
    let budget = crate::cloud::peer_claims::PeerBudget::new(Duration::from_secs(5));
    let (project, remote_tasks, edges) = budget
        .run(move || {
            let queue = Arc::new(SyncQueue::open_read_only(&root)?);
            let team = config.active_team_id();
            let syncer = CloudSyncer::new(
                queue,
                config,
                CloudSyncerConfig {
                    timeout: Duration::from_secs(2),
                    ..Default::default()
                },
            );
            let tasks = syncer.fetch_task_snapshot(team.as_deref(), &project)?;
            let edges = syncer.fetch_dependency_snapshot(team.as_deref(), &project)?;
            Ok::<_, crate::error::CasError>((project, tasks, edges))
        })
        .await
        .map_err(|_| inventory_warning())?
        .map_err(|_| inventory_warning())?;
    let mut tasks = Vec::new();
    for raw in remote_tasks {
        if !crate::cloud::entity_matches_project(&raw, &project, "task") {
            continue;
        }
        let task: Task = crate::cloud::syncer::pull::deserialize_pulled_entity(raw, "task")
            .map_err(|_| inventory_warning())?;
        if task.scope == Scope::Project {
            tasks.push(task);
        }
    }
    Ok(CloudInventory { tasks, edges })
}

/// Return an actionable warning before the caller creates a row or a lane.
/// Cloud is optional, but a configured inventory that cannot be read requires
/// the same explicit override as an overlap; never silently call that checked.
pub(super) async fn warning(
    core: &CasCore,
    store: &dyn TaskStore,
    title: &str,
    description: &str,
) -> Result<Option<String>, String> {
    let project = crate::cloud::resolve_canonical_id(&core.cas_root);
    let mut tasks: BTreeMap<_, _> = store
        .list(None)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|task| task.scope == Scope::Project)
        .filter(|task| {
            project.as_deref().is_none_or(|project| {
                task.origin_project
                    .as_deref()
                    .is_none_or(|origin| crate::cloud::project_ids_match(origin, project))
            })
        })
        .map(|task| (task.id.clone(), task))
        .collect();
    let (remote_edges, outage) = match cloud_inventory(&core.cas_root, project.as_deref()).await {
        Ok(inventory) => {
            for task in inventory.tasks {
                // Keep newer unsynced local changes, including a locally closed
                // epic; otherwise the fresh Cloud state wins over a stale cache.
                if tasks
                    .get(&task.id)
                    .is_none_or(|local| task.updated_at >= local.updated_at)
                {
                    tasks.insert(task.id.clone(), task);
                }
            }
            (inventory.edges, None)
        }
        // An outage is a separate refusal, not an early return: local scope
        // must still be checked and shown before an override is requested.
        Err(warning) => (Vec::new(), Some(warning)),
    };

    let Some((epic, score)) = tasks
        .values()
        .filter(|task| task.task_type == TaskType::Epic && open(task))
        .filter_map(|task| similarity(title, description, task).map(|score| (task, score)))
        .max_by(|(a, x), (b, y)| x.total_cmp(y).then_with(|| b.id.cmp(&a.id)))
    else {
        return Ok(outage);
    };

    let mut children: BTreeSet<String> = store
        .get_dependents(&epic.id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|edge| edge.dep_type == DependencyType::ParentChild)
        .map(|edge| edge.from_id)
        .collect();
    for raw in remote_edges {
        // Endpoints are looked up in the project-filtered inventory below.
        // Cloud tombstones must not resurrect a removed child relationship.
        if !project.as_deref().is_some_and(|project| {
            crate::cloud::syncer::pull::task_dependency_matches_project(&raw, project)
        }) || raw.get("to_id").and_then(|v| v.as_str()) != Some(epic.id.as_str())
            || raw.get("dep_type").and_then(|v| v.as_str()) != Some("parent-child")
        {
            continue;
        }
        if let Some(child) = raw.get("from_id").and_then(|v| v.as_str()) {
            if raw.get("deleted").and_then(|v| v.as_bool()) == Some(true)
                || raw.get("operation").and_then(|v| v.as_str()) == Some("delete")
            {
                children.remove(child);
            } else {
                children.insert(child.to_owned());
            }
        }
    }
    let children: Vec<_> = children
        .iter()
        .filter_map(|id| tasks.get(id))
        .filter(|task| open(task))
        .map(|task| format!("  - {}: {}", task.id, task.title))
        .collect();
    let owner = epic
        .epic_verification_owner
        .as_deref()
        .or(epic.assignee.as_deref())
        .unwrap_or("unrecorded");
    let overlap = format!(
        "DUPLICATE EPIC WARNING: open epic {} ({:?}) overlaps this plan at {:.0}%.\n\
         Owner supervisor: {owner}\nOpen children:\n{}\n\
         Review or reuse that epic and agree scope with its owner. If this is intentional, retry with confirm_warning=true.",
        epic.id,
        epic.title,
        score * 100.0,
        if children.is_empty() {
            "  (none)".into()
        } else {
            children.join("\n")
        },
    );
    Ok(Some(match outage {
        Some(outage) => format!("{outage}\n\n{overlap}"),
        None => overlap,
    }))
}

fn inventory_warning() -> String {
    "EPIC INVENTORY WARNING (CLOUD UNAVAILABLE): could not read the current project's Cloud epic inventory. \
     Local epic scope was still checked; any overlap is reported separately below. No epic was created. \
     Restore Cloud access and retry, or explicitly accept the unchecked peer scope with confirm_warning=true."
        .into()
}
