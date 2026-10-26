//! Retention for durable per-task artifact directories (GH #196, cas-75a8 / GH #1295).
//!
//! One policy serves both `factory action=gc_cleanup artifacts=true` and the
//! session-free `cas artifact prune`: a closed task's directory is reclaimable
//! only after the retention window since close, and only when no open task,
//! and no task updated inside the window, cites the task or one of its
//! published artifact aliases. Unknown close dates and unreadable evidence
//! preserve everything.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};

/// The shortest retention after close that either surface accepts.
pub const MIN_RETENTION_DAYS: i64 = 7;

/// Bytes under a directory, and the regenerable build outputs it stores.
#[derive(Default)]
pub struct ArtifactRootUsage {
    pub bytes: u64,
    pub regenerable: Vec<PathBuf>,
    pub error: Option<String>,
}

/// Measure `root` without following links.
pub fn artifact_root_usage(root: &Path) -> ArtifactRootUsage {
    artifact_root_usage_until(root, None)
}

pub(crate) fn artifact_root_usage_until(root: &Path,
    deadline: Option<std::time::Instant>) -> ArtifactRootUsage {
    let mut usage = ArtifactRootUsage::default();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
            usage.error = Some("report scan budget exhausted; byte inventory incomplete".into());
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                usage.error = Some(error.to_string());
                continue;
            }
        };
        if entry.file_type().is_file() {
            match entry.metadata() {
                Ok(metadata) => usage.bytes = usage.bytes.saturating_add(metadata.len()),
                Err(error) => usage.error = Some(error.to_string()),
            }
        } else if entry.file_type().is_dir()
            && (entry.file_name() == "DerivedData" || entry.file_name() == "target")
        {
            usage.regenerable.push(entry.path().to_path_buf());
        }
    }
    usage.regenerable.sort();
    usage
}

/// Closed tasks whose artifacts are past `retention_days` since close and
/// cited by no open or recently updated task (by id or published alias).
pub fn eligible_artifact_tasks(
    cas_root: &Path,
    tasks: &[cas_types::Task],
    now: DateTime<Utc>,
    retention_days: i64,
) -> Result<BTreeSet<String>, String> {
    let cutoff = now - Duration::days(retention_days.max(MIN_RETENTION_DAYS));
    let references = serde_json::to_string(
        &tasks
            .iter()
            .filter(|task| !task.is_terminal() || task.updated_at >= cutoff)
            .collect::<Vec<_>>(),
    )
    .map_err(|error| error.to_string())?;
    let artifacts =
        cas_store::SqliteArtifactStore::open(cas_root).map_err(|error| error.to_string())?;
    let mut eligible = BTreeSet::new();
    for task in tasks.iter().filter(|task| {
        task.status == cas_types::TaskStatus::Closed
            && task.closed_at.is_some_and(|closed| closed <= cutoff)
    }) {
        if references.contains(&task.id) {
            continue;
        }
        let published = artifacts
            .list_for_task(&task.id)
            .map_err(|error| error.to_string())?;
        let referenced = published.iter().any(|artifact| {
            std::iter::once(Some(artifact.id.as_str()))
                .chain([
                    artifact.cloud_artifact_id.as_deref(),
                    artifact.cloud_url.as_deref(),
                    artifact.slack_permalink.as_deref(),
                ])
                .flatten()
                .any(|alias| !alias.is_empty() && references.contains(alias))
        });
        if !referenced {
            eligible.insert(task.id.clone());
        }
    }
    Ok(eligible)
}

/// Why a closed task's directory is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kept {
    /// Closed inside the retention window (or with no close date).
    Recent,
    /// An open or recently updated task cites it or its published artifacts.
    Referenced,
    /// cas-6162: a legacy top-level directory whose task id cannot be proven
    /// to be this project's (the reason says why).
    Unproven(String),
}

/// One task directory under the project's artifact namespace.
#[derive(Debug, Clone)]
pub struct TaskArtifactDir {
    pub task_id: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub closed_at: Option<DateTime<Utc>>,
    /// cas-6162: a legacy directory directly under the artifacts root, not
    /// under this project's key.
    pub legacy: bool,
}

/// What `cas artifact prune` would remove and what it keeps.
#[derive(Debug, Default)]
pub struct PrunePlan {
    pub root: PathBuf,
    pub retention_days: i64,
    pub reclaimable: Vec<TaskArtifactDir>,
    pub kept: Vec<(TaskArtifactDir, Kept)>,
    /// Directories that are not a closed task of this project: open tasks,
    /// other projects' ids, or strays. Never deleted.
    pub untouched: usize,
}

impl PrunePlan {
    pub fn reclaimable_bytes(&self) -> u64 {
        self.reclaimable.iter().map(|dir| dir.bytes).sum()
    }
}

/// Plan a prune of `root` (this project's artifact namespace) and, when
/// given, of the legacy task directories directly under `legacy_root`
/// (cas-6162). A legacy directory is a candidate only when its name is
/// exactly a task id that is closed in this project and held by no other
/// known project; otherwise it is kept and reported as unproven.
pub fn plan_prune(
    cas_root: &Path,
    root: &Path,
    legacy_root: Option<&Path>,
    others: &dyn crate::task_ownership::OtherProjects,
    tasks: &[cas_types::Task],
    now: DateTime<Utc>,
    retention_days: i64,
) -> Result<PrunePlan, String> {
    let retention_days = retention_days.max(MIN_RETENTION_DAYS);
    let mut plan = PrunePlan {
        root: root.to_path_buf(),
        retention_days,
        ..Default::default()
    };
    if std::fs::symlink_metadata(root).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err("project artifact namespace must not be a symlink".into());
    }
    // A project with no keyed directory yet may still own legacy ones.
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.to_string()),
    };
    let eligible = eligible_artifact_tasks(cas_root, tasks, now, retention_days)?;
    let cutoff = now - Duration::days(retention_days);
    let mut dirs: Vec<_> = entries
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .collect();
    dirs.sort_by_key(|entry| entry.file_name());
    for entry in dirs {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(task) = tasks
            .iter()
            .find(|task| task.id == name && task.status == cas_types::TaskStatus::Closed)
        else {
            plan.untouched += 1;
            continue;
        };
        let dir = TaskArtifactDir {
            task_id: name,
            bytes: artifact_root_usage(&entry.path()).bytes,
            path: entry.path(),
            closed_at: task.closed_at,
            legacy: false,
        };
        classify_dir(&mut plan, dir, task, &eligible, cutoff);
    }
    if let Some(legacy_root) = legacy_root {
        plan_legacy(&mut plan, legacy_root, others, tasks, &eligible, cutoff)?;
    }
    Ok(plan)
}

fn classify_dir(
    plan: &mut PrunePlan,
    dir: TaskArtifactDir,
    task: &cas_types::Task,
    eligible: &BTreeSet<String>,
    cutoff: DateTime<Utc>,
) {
    if eligible.contains(&dir.task_id) {
        plan.reclaimable.push(dir);
    } else if task.closed_at.is_some_and(|closed| closed <= cutoff) {
        plan.kept.push((dir, Kept::Referenced));
    } else {
        plan.kept.push((dir, Kept::Recent));
    }
}

/// Legacy `<artifacts_root>/cas-<id>/` directories (cas-6162).
fn plan_legacy(
    plan: &mut PrunePlan,
    legacy_root: &Path,
    others: &dyn crate::task_ownership::OtherProjects,
    tasks: &[cas_types::Task],
    eligible: &BTreeSet<String>,
    cutoff: DateTime<Utc>,
) -> Result<(), String> {
    if std::fs::symlink_metadata(legacy_root).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err("artifacts root must not be a symlink".into());
    }
    let entries = match std::fs::read_dir(legacy_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let mut dirs: Vec<_> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| crate::task_ownership::is_task_id(&entry.file_name().to_string_lossy()))
        .collect();
    dirs.sort_by_key(|entry| entry.file_name());
    // Other projects' stores are read once, at the first ownership lookup.
    for entry in dirs {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(task) = tasks
            .iter()
            .find(|task| task.id == name && task.status == cas_types::TaskStatus::Closed)
        else {
            plan.untouched += 1;
            continue;
        };
        let dir = TaskArtifactDir {
            task_id: name,
            bytes: artifact_root_usage(&entry.path()).bytes,
            path: entry.path(),
            closed_at: task.closed_at,
            legacy: true,
        };
        match others.holders(&dir.task_id) {
            Ok(holders) if holders.is_empty() => classify_dir(plan, dir, task, eligible, cutoff),
            Ok(holders) => {
                let why = format!("task id also exists in {}", holders.join(", "));
                plan.kept.push((dir, Kept::Unproven(why)));
            }
            Err(error) => plan.kept.push((dir, Kept::Unproven(error))),
        }
    }
    Ok(())
}

/// Remove every reclaimable directory; returns (removed, bytes freed).
pub fn apply_prune(plan: &PrunePlan) -> (usize, u64, Vec<String>) {
    let mut removed = (0, 0, Vec::new());
    for dir in &plan.reclaimable {
        match std::fs::remove_dir_all(&dir.path) {
            Ok(()) => {
                removed.0 += 1;
                removed.1 += dir.bytes;
            }
            Err(error) => removed.2.push(format!("{}: {error}", dir.path.display())),
        }
    }
    removed
}
