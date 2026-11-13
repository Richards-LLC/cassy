//! Bounded host retention. Unknown ownership, references or Git state retain bytes.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, ensure};
use cas_store::{AgentStore, KnownRepoStore, TaskStore};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

const BUDGET: Duration = Duration::from_secs(20);
const ARTIFACT_DAYS: i64 = 14;
pub(crate) const PRESSURE_PERCENT: u8 = 80;

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct SweepReport {
    pub at: i64,
    pub applied: bool,
    pub used_percent: u8,
    pub incomplete: bool,
    pub rows: Vec<Row>,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Row {
    pub project: PathBuf,
    pub category: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub reclaimed_bytes: u64,
    pub disposition: String,
}

impl SweepReport {
    pub fn reclaimed_bytes(&self) -> u64 {
        self.rows.iter().map(|row| row.reclaimed_bytes).sum()
    }

    pub fn render(&self) -> String {
        let mut categories = BTreeMap::<(&Path, &str), (u64, u64)>::new();
        for row in &self.rows {
            let total = categories.entry((&row.project, &row.category)).or_default();
            total.0 += row.bytes;
            total.1 += row.reclaimed_bytes;
        }
        let mut out = format!("\nHost retention: reclaimed_bytes={} dry_run={} incomplete={}\n",
            self.reclaimed_bytes(), !self.applied, self.incomplete);
        for ((project, category), (bytes, reclaimed)) in categories {
            out.push_str(&format!("  {} {category}: bytes={bytes} reclaimed_bytes={reclaimed}\n", project.display()));
        }
        for error in &self.errors { out.push_str(&format!("  Preserved: {error}\n")); }
        out.push_str(&format!("HOST_RETENTION_STATUS_JSON={}\n", serde_json::to_string(self).unwrap()));
        out
    }
}

struct Project {
    root: PathBuf,
    tasks: Vec<cas_types::Task>,
    agents: Vec<cas_types::Agent>,
    artifacts: PathBuf,
}

#[derive(Default)]
struct References(String);
impl References {
    fn add(&mut self, value: impl Serialize) -> Result<()> {
        self.0.push_str(&serde_json::to_string(&value)?);
        self.0.push('\n');
        Ok(())
    }
    fn cites(&self, key: &str) -> bool { !key.is_empty() && self.0.contains(key) }
}

fn state_dir() -> PathBuf { crate::store::known_repos::host_cas_dir().join("retention") }

pub(crate) fn last_report() -> Option<SweepReport> {
    let path = state_dir().join("last-sweep.json");
    (fs::metadata(&path).ok()?.len() <= 2 * 1024 * 1024)
        .then(|| fs::read(path).ok()).flatten()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

pub(crate) fn disk_percent(path: &Path) -> Option<u8> {
    let space = crate::fs_space::fs_space(path).ok()?;
    (space.total_bytes > 0).then(|| ((u128::from(space.used_bytes()) * 100) / u128::from(space.total_bytes)).min(100) as u8)
}

pub(crate) fn automatic_due(last: Option<&SweepReport>, percent: u8, idle: bool, now: i64) -> bool {
    if percent <= PRESSURE_PERCENT && !idle { return false; }
    last.is_none_or(|last| !last.applied
        || (percent > PRESSURE_PERCENT && last.used_percent <= PRESSURE_PERCENT)
        || now.saturating_sub(last.at) >= if percent > PRESSURE_PERCENT { 60 } else { 300 })
}

/// Called on the daemon's heartbeat as well as idle maintenance. The host lock
/// serializes different projects; the persisted timestamp prevents scan storms.
pub(crate) fn automatic(cas: &Path, idle: bool) {
    // Unit tests invoke maintenance with arbitrary roots; only explicit isolated
    // sweep fixtures may delete data in the test process.
    if cfg!(test) { return; }
    let percent = disk_percent(cas).unwrap_or(0).max(disk_percent(&crate::store::known_repos::host_cas_dir()).unwrap_or(0));
    if automatic_due(last_report().as_ref(), percent, idle, Utc::now().timestamp()) {
        let _ = sweep(cas, true);
    }
}

fn load_projects(cas: &Path, deadline: Instant) -> Result<(Vec<Project>, References)> {
    let registry = crate::store::known_repos::open_host_known_repo_store()?;
    let mut roots: BTreeSet<_> = registry.list()?.into_iter().map(|repo| repo.path.join(".cas")).collect();
    roots.insert(cas.to_path_buf());
    if let Some(global) = dirs::home_dir().map(|home| home.join(".cas"))
        && global.join("cas.db").is_file() { roots.insert(global); }
    ensure!(roots.len() <= 256, "registered-project inventory exceeds 256 stores");
    let mut projects = Vec::new();
    let mut refs = References::default();
    let mut seen = BTreeSet::new();
    for root in roots {
        ensure!(Instant::now() < deadline, "reference inventory budget exhausted; nothing deleted");
        no_symlink_ancestors(&root)?;
        let root = root.canonicalize().with_context(|| format!("unreadable registered store {}", root.display()))?;
        if !seen.insert(root.clone()) { continue; }
        ensure!(root.join("cas.db").is_file(), "registered store has no database: {}", root.display());
        let tasks = cas_store::SqliteTaskStore::open(&root)?.list(None)?;
        for task in tasks.iter().filter(|task| !task.is_terminal()) { refs.add(task)?; }
        // Publication events and their task records carry portable references.
        for release in cas_store::quality_release_events(&root)?.into_iter().take(3) {
            refs.add((&release.task_id, &release.release_version, &release.local_note, &release.title, &release.link))?;
            if let Some(id) = &release.task_id
                && let Some(task) = tasks.iter().find(|task| &task.id == id) { refs.add(task)?; }
        }
        // Immutable release closeouts include no-code proofs and reviewed ledgers.
        let db = cas_store::shared_db::shared_connection(&root.join("cas.db"))?;
        let conn = db.try_lock().map_err(|_| anyhow::anyhow!("release evidence busy at {}", root.display()))?;
        for (table, order) in [("release_reconcile_operations", "rowid"), ("qa_batch_receipts", "recorded_at")] {
            let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)", [table], |row| row.get(0))?;
            if exists {
                let mut statement = conn.prepare(&format!("SELECT receipt_json FROM {table} ORDER BY {order} DESC LIMIT 3"))?;
                for value in statement.query_map([], |row| row.get::<_, String>(0))? { refs.0.push_str(&value?); }
            }
        }
        drop(conn);
        let config = crate::config::Config::load(&root)?;
        let base = crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
        let artifacts = crate::config::project_factory_artifacts_root(&root, &base);
        let agents = cas_store::SqliteAgentStore::open(&root)?.list(None)?;
        projects.push(Project { root, tasks, agents, artifacts });
    }
    // Close citations transitively: live task -> old task -> published alias ->
    // another task. A terminal intermediary must not sever evidence protection.
    let mut evidence = Vec::new();
    for project in &projects {
        let store = cas_store::SqliteArtifactStore::open(&project.root)?;
        for task in &project.tasks {
            let mut keys = vec![task.id.clone()];
            for artifact in store.list_for_task(&task.id)? {
                keys.push(artifact.id);
                keys.extend([artifact.cloud_artifact_id, artifact.cloud_url, artifact.slack_permalink].into_iter().flatten());
            }
            evidence.push((keys, task));
        }
    }
    let mut protected = BTreeSet::new();
    loop {
        ensure!(Instant::now() < deadline, "reference closure budget exhausted; nothing deleted");
        let before = protected.len();
        for (index, (keys, task)) in evidence.iter().enumerate() {
            if !protected.contains(&index) && keys.iter().any(|key| refs.cites(key)) {
                refs.add(task)?;
                protected.insert(index);
            }
        }
        if before == protected.len() { break; }
    }
    Ok((projects, refs))
}

#[derive(Clone, Serialize, Deserialize)]
struct Candidate {
    project: PathBuf,
    category: String,
    path: PathBuf,
    keys: Vec<String>,
    #[serde(default)]
    preserve: Option<String>,
}

fn cited(candidate: &Candidate, refs: &References) -> bool {
    refs.cites(&candidate.path.to_string_lossy()) || candidate.keys.iter().any(|key| refs.cites(key))
}

fn no_symlink_ancestors(path: &Path) -> Result<()> {
    ensure!(path.is_absolute() && !path.components().any(|part| matches!(part, std::path::Component::ParentDir)), "non-canonical retention path");
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => ensure!(!metadata.file_type().is_symlink(), "symlinked retention path: {}", ancestor.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn children(path: &Path) -> Result<Vec<PathBuf>> {
    match fs::read_dir(path) {
        Ok(entries) => entries.map(|entry| Ok(entry?.path())).collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

fn git(path: &Path, args: &[&str], deadline: Instant) -> Result<String> {
    let remaining = deadline.saturating_duration_since(Instant::now()).min(Duration::from_secs(1));
    ensure!(!remaining.is_zero(), "Git probe budget exhausted");
    let output = crate::bounded_process::run_command(
        Command::new("git").arg("-C").arg(path).args(args).env("GIT_OPTIONAL_LOCKS", "0")
            .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE").env_remove("GIT_COMMON_DIR"),
        crate::bounded_process::Deadline::after(remaining), remaining,
    ).map_err(|error| anyhow::anyhow!("Git state unavailable: {error:?}"))?;
    ensure!(output.status.success(), "Git state unavailable: {}", String::from_utf8_lossy(&output.stderr));
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

struct Tree { bytes: u64, newest: SystemTime, repositories: Vec<PathBuf> }
fn inspect_tree(path: &Path, deadline: Instant, exclude: Option<&Path>) -> Result<Tree> {
    no_symlink_ancestors(path)?;
    let mut tree = Tree { bytes: 0, newest: SystemTime::UNIX_EPOCH, repositories: Vec::new() };
    let mut entries = walkdir::WalkDir::new(path).follow_links(false).into_iter();
    while let Some(entry) = entries.next() {
        ensure!(Instant::now() < deadline, "tree inventory budget exhausted");
        let entry = entry?;
        if exclude == Some(entry.path()) { entries.skip_current_dir(); continue; }
        // Nested links are retained rather than trusting an age from a target.
        ensure!(!entry.file_type().is_symlink(), "symlink inside retention tree");
        let metadata = entry.metadata()?;
        tree.newest = tree.newest.max(metadata.modified()?);
        if entry.file_name() == ".git" {
            let repo = entry.path().parent().context("Git parent missing")?;
            ensure!(git(repo, &["rev-list", "--count", "HEAD", "--not", "--remotes"], deadline)? == "0", "unpushed commits");
            ensure!(git(repo, &["status", "--porcelain=v1", "--untracked-files=all"], deadline)?.is_empty(), "dirty source");
            if entry.file_type().is_file() { tree.repositories.push(repo.to_path_buf()); }
            if entry.file_type().is_dir() { entries.skip_current_dir(); }
        } else if metadata.is_file() { tree.bytes = tree.bytes.saturating_add(metadata.len()); }
    }
    Ok(tree)
}

fn candidates(project: &Project, refs: &References, now: DateTime<Utc>, deadline: Instant) -> Result<Vec<(Candidate, Duration)>> {
    let mut found = Vec::new();
    let artifacts = cas_store::SqliteArtifactStore::open(&project.root)?;
    for task in &project.tasks {
        ensure!(Path::new(&task.id).components().count() == 1 && !task.id.starts_with('.'), "invalid task directory");
        let mut keys = vec![task.id.clone()];
        for artifact in artifacts.list_for_task(&task.id)? {
            keys.push(artifact.id);
            keys.extend([artifact.cloud_artifact_id, artifact.cloud_url, artifact.slack_permalink].into_iter().flatten());
        }
        found.push((Candidate { project: project.root.clone(), category: "artifacts".into(), path: project.artifacts.join(&task.id), keys,
            preserve: (!(task.is_terminal() && task.closed_at.is_some_and(|closed| now.signed_duration_since(closed).num_days() >= ARTIFACT_DAYS)))
                .then(|| "task is live, recently closed or has unknown close date".into()) }, Duration::from_secs(14 * 86400)));
    }
    for category in ["scratch", "review-scratch"] {
        for path in children(&project.root.join(category))? {
            let mut keys = Vec::new();
            if let Some(id) = path.file_name().and_then(|name| name.to_str()) {
                keys.extend(project.tasks.iter().filter(|task| id.contains(&task.id)).map(|task| task.id.clone()));
            }
            found.push((Candidate { project: project.root.clone(), category: category.into(), path, keys, preserve: None }, Duration::from_secs(86400)));
        }
    }
    for path in children(&project.root.join("worktrees"))? {
        if Instant::now() >= deadline { break; }
        if !path.join(".git").exists() { continue; }
        let worker = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        let branch = git(&path, &["branch", "--show-current"], deadline)?;
        let owners: Vec<_> = project.agents.iter().filter(|agent| agent.name == worker
            || agent.metadata.get("clone_path").is_some_and(|value| Path::new(value) == path)).collect();
        // Missing old registry rows are retired only for an actual factory branch.
        if (owners.is_empty() && !branch.starts_with("factory/")) || owners.iter().any(|agent|
            crate::mcp::tools::service::agent_liveness::evaluate_supervision_liveness(agent).is_live()) { continue; }
        let keys = vec![worker.to_string(), branch];
        let source = Candidate { project: project.root.clone(), category: "worktrees".into(), path: path.clone(), keys: keys.clone(), preserve: None };
        if cited(&source, refs) { continue; }
        // Build output is regenerable even when an unpushed source branch is kept.
        found.push((Candidate { project: project.root.clone(), category: "targets".into(), path: crate::factory_scratch::worker_target_dir(&path), keys, preserve: None }, Duration::ZERO));
        found.push((source, Duration::ZERO));
    }
    Ok(found)
}

fn drain(directory: &Path, candidate: &Candidate, refs: &References, lock: &File, deadline: Instant, row: &mut Row) -> Result<()> {
    no_symlink_ancestors(directory)?;
    let data = directory.join("data");
    ensure!(!fs::symlink_metadata(&data)?.file_type().is_symlink(), "symlinked trash data");
    ensure!(!cited(candidate, refs), "cited by live work or retained releases");
    ensure!(!crate::factory_target_cache::review_output_in_use(directory, lock), "live or unverifiable native process");
    // Each successful unlink is accounted even if a later unlink fails.
    row.disposition = "reclaimed".into();
    for entry in walkdir::WalkDir::new(&data).follow_links(false).contents_first(true) {
        if Instant::now() >= deadline { row.disposition = "pending (budget exhausted)".into(); break; }
        let entry = entry?;
        let bytes = if entry.file_type().is_file() { entry.metadata()?.len() } else { 0 };
        if entry.file_type().is_dir() { fs::remove_dir(entry.path())?; }
        else { fs::remove_file(entry.path())?; }
        row.reclaimed_bytes = row.reclaimed_bytes.saturating_add(bytes);
    }
    if !data.exists() {
        fs::remove_file(directory.join("owner.json"))?;
        fs::remove_dir(directory)?;
    }
    Ok(())
}

fn validate_candidate(candidate: &Candidate, projects: &[Project]) -> Result<()> {
    let project = projects.iter().find(|project| project.root == candidate.project).context("unregistered trash owner")?;
    let valid = match candidate.category.as_str() {
        "artifacts" => candidate.path.parent() == Some(project.artifacts.as_path()),
        "scratch" | "review-scratch" | "worktrees" => candidate.path.parent() == Some(project.root.join(&candidate.category).as_path()),
        "targets" => candidate.keys.first().is_some_and(|worker| {
            Path::new(worker).components().count() == 1
                && !worker.starts_with('.')
                && crate::factory_scratch::worker_target_dir(&project.root.join("worktrees").join(worker)) == candidate.path
        }),
        _ => false,
    };
    ensure!(valid, "trash candidate escapes its category");
    no_symlink_ancestors(&candidate.path)
}

pub(crate) fn sweep(cas: &Path, apply: bool) -> Result<SweepReport> {
    sweep_until(cas, apply, Instant::now() + BUDGET)
}

pub(crate) fn preview(cas: &Path, deadline: Instant) -> Result<SweepReport> {
    sweep_until(cas, false, deadline)
}

fn sweep_until(cas: &Path, apply: bool, deadline: Instant) -> Result<SweepReport> {
    let state = state_dir();
    no_symlink_ancestors(&state)?;
    // Preview uses a temporary lease descriptor and never creates host state.
    if apply { fs::create_dir_all(&state)?; }
    no_symlink_ancestors(&state.join("sweep.lock"))?;
    let lock = if apply {
        let lock = fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(state.join("sweep.lock"))?;
        lock.try_lock_exclusive().context("host retention already running")?;
        lock
    } else { tempfile::tempfile()? };
    let mut report = SweepReport { at: Utc::now().timestamp(), applied: apply, used_percent: disk_percent(cas).unwrap_or(0).max(disk_percent(&crate::store::known_repos::host_cas_dir()).unwrap_or(0)), ..Default::default() };
    let (projects, refs) = match load_projects(cas, deadline) {
        Ok(snapshot) => snapshot,
        Err(error) => { report.errors.push(error.to_string()); report.incomplete = true; if apply { persist(&report)?; } return Ok(report); }
    };
    let trash = state.join("trash");
    if apply {
        for directory in children(&trash)? {
            if Instant::now() >= deadline { report.incomplete = true; break; }
            let attempt = (|| -> Result<()> {
                no_symlink_ancestors(&directory)?;
                let marker = directory.join("owner.json");
                no_symlink_ancestors(&marker)?;
                ensure!(fs::metadata(&marker)?.len() <= 64 * 1024, "oversized trash marker");
                let candidate: Candidate = serde_json::from_slice(&fs::read(marker)?)?;
                validate_candidate(&candidate, &projects)?;
                let mut row = Row { project: candidate.project.clone(), category: candidate.category.clone(), path: candidate.path.clone(), bytes: 0, reclaimed_bytes: 0, disposition: String::new() };
                if let Err(error) = drain(&directory, &candidate, &refs, &lock, deadline, &mut row) { row.disposition = format!("retained: {error}"); }
                row.bytes = row.reclaimed_bytes;
                report.rows.push(row);
                Ok(())
            })();
            if let Err(error) = attempt { report.errors.push(format!("{}: {error}", directory.display())); }
        }
    }
    for project in &projects {
        if Instant::now() >= deadline { report.incomplete = true; break; }
        let found = match candidates(project, &refs, Utc::now(), deadline) {
            Ok(found) => found,
            Err(error) => { report.errors.push(format!("{}: {error}", project.root.display())); continue; }
        };
        for (candidate, age) in found {
            if report.rows.len() >= 1024 || Instant::now() >= deadline { report.incomplete = true; break; }
            if !candidate.path.exists() { continue; }
            let mut row = Row { project: candidate.project.clone(), category: candidate.category.clone(), path: candidate.path.clone(), bytes: 0, reclaimed_bytes: 0, disposition: String::new() };
            let attempt = (|| -> Result<()> {
                validate_candidate(&candidate, &projects)?;
                let excluded = (candidate.category == "worktrees").then(|| crate::factory_scratch::worker_target_dir(&candidate.path));
                let tree = inspect_tree(&candidate.path, deadline, excluded.as_deref())?;
                row.bytes = tree.bytes;
                ensure!(!cited(&candidate, &refs), "cited by live work or retained releases");
                ensure!(candidate.preserve.is_none(), "{}", candidate.preserve.as_deref().unwrap_or_default());
                ensure!(SystemTime::now().duration_since(tree.newest).unwrap_or_default() >= age, "inside retention window");
                ensure!(!crate::factory_target_cache::review_output_in_use(&candidate.path, &lock), "live or unverifiable native process");
                if apply {
                    let directory = trash.join(uuid::Uuid::new_v4().to_string());
                    no_symlink_ancestors(&directory)?;
                    fs::create_dir_all(&directory)?;
                    let mut marker = File::create(directory.join("owner.json"))?;
                    serde_json::to_writer(&mut marker, &candidate)?;
                    marker.sync_all()?;
                    fs::rename(&candidate.path, directory.join("data"))?;
                    for repo in tree.repositories {
                        // Only vanished exact registrations are force-removed; source
                        // was already proven clean and fully pushed before the rename.
                        let _ = git(project.root.parent().context("project parent")?,
                            &["worktree", "remove", "--force", repo.to_str().context("non-UTF-8 checkout")?], deadline);
                    }
                    drain(&directory, &candidate, &refs, &lock, deadline, &mut row)?;
                } else { row.disposition = "reclaimable".into(); }
                Ok(())
            })();
            if let Err(error) = attempt { row.disposition = format!("retained: {error}"); }
            report.rows.push(row);
        }
    }
    report.incomplete |= Instant::now() >= deadline;
    if apply { persist(&report)?; }
    Ok(report)
}

fn persist(report: &SweepReport) -> Result<()> {
    let state = state_dir();
    let bytes = serde_json::to_vec(report)?;
    no_symlink_ancestors(&state.join("sweeps.jsonl"))?;
    no_symlink_ancestors(&state.join("last-sweep.json"))?;
    let mut log = fs::OpenOptions::new().create(true).append(true).open(state.join("sweeps.jsonl"))?;
    log.write_all(&bytes)?;
    log.write_all(b"\n")?;
    log.sync_data()?;
    if report.applied {
        let mut file = tempfile::NamedTempFile::new_in(&state)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(state.join("last-sweep.json"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_retention_replays_inventory_cas_d740() {
        let env = crate::test_support::TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().unwrap();
        let gib = 1024_u64.pow(3);
        let old = SystemTime::now() - Duration::from_secs(30 * 86400);
        let sparse = |path: &Path, bytes| {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let file = File::create(path).unwrap();
            file.set_len(bytes).unwrap();
            file.set_times(fs::FileTimes::new().set_modified(old)).unwrap();
            File::open(path.parent().unwrap()).unwrap().set_times(fs::FileTimes::new().set_modified(old)).unwrap();
        };
        let mut roots = Vec::new();
        for (name, size) in [("cassy", 40), ("gabber", 42)] {
            let project = env.home().join(name);
            let root = crate::store::init_cas_dir(&project).unwrap();
            fs::write(root.join("config.toml"), format!("[project]\ncanonical_id = {name:?}\n")).unwrap();
            crate::store::known_repos::register_repo_strict(&project).unwrap();
            let mut task = cas_types::Task::new(format!("cas-{name}-old"), "Inventory fixture".into());
            task.status = cas_types::TaskStatus::Closed;
            task.closed_at = Some(Utc::now() - chrono::Duration::days(30));
            cas_store::SqliteTaskStore::open(&root).unwrap().add(&task).unwrap();
            let artifacts = crate::config::project_factory_artifacts_root(&root, &crate::config::resolved_factory_artifacts_root(None));
            sparse(&artifacts.join(&task.id).join("inventory"), size * gib);
            roots.push(root);
        }
        sparse(&roots[0].join("scratch/old/inventory"), 6 * gib);
        sparse(&roots[0].join("review-scratch/old/inventory"), 32 * gib);
        // Source and target sizes are accounted separately, without hashing or
        // allocating the reported 73 GiB of ignored checkout scratch.
        let worktree = roots[0].join("worktrees/retired");
        fs::create_dir_all(&worktree).unwrap();
        let deadline = Instant::now() + BUDGET;
        git(&worktree, &["init", "-b", "factory/retired"], deadline).unwrap();
        git(&worktree, &["-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", "commit", "--allow-empty", "-m", "fixture"], deadline).unwrap();
        git(&worktree, &["update-ref", "refs/remotes/origin/factory/retired", "HEAD"], deadline).unwrap();
        fs::write(worktree.join(".git/info/exclude"), "inventory\ntarget/\n").unwrap();
        sparse(&worktree.join("inventory"), 73 * gib);
        sparse(&crate::factory_scratch::worker_target_dir(&worktree).join("inventory"), gib);
        let started = Instant::now();
        let report = sweep(&roots[0], false).unwrap();
        assert!(!report.incomplete, "{report:?}");
        assert!(started.elapsed() < BUDGET);
        assert_eq!(report.reclaimed_bytes(), 0);
        for (category, size) in [("artifacts", 82), ("scratch", 6), ("review-scratch", 32), ("worktrees", 73), ("targets", 1)] {
            let bytes: u64 = report.rows.iter().filter(|row| row.category == category && row.disposition == "reclaimable").map(|row| row.bytes).sum();
            assert_eq!(bytes, size * gib, "{category}: {report:?}");
        }
        assert_eq!(fs::metadata(worktree.join("inventory")).unwrap().len(), 73 * gib);
        assert!(!state_dir().join("last-sweep.json").exists());
        eprintln!("fixture inventory: cassy artifacts=40GiB gabber artifacts=42GiB worktrees=73GiB review-scratch=32GiB scratch=6GiB targets=1GiB; input disk=84%; preview reclaimed_bytes=0 elapsed_ms={}", started.elapsed().as_millis());
    }
}
