//! Disposable task/dispatch-owned proof checkouts shared by reviewers and GC.
#[cfg(unix)]
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

const MIN_AGE_SECS: i64 = 2 * 3600;
const LARGE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_RECLAIMS_PER_PASS: usize = 16;

#[derive(Serialize, Deserialize)]
struct Owner {
    schema_version: u32,
    cas_root: PathBuf,
    task_id: String,
    dispatch_id: Option<String>,
    head: String,
    created_at: DateTime<Utc>,
    #[serde(default)]
    last_used_at: Option<DateTime<Utc>>,
}

/// The lease spans admission and the capped test; native process probes protect
/// any surviving descendants after a reviewer exits.
pub struct Checkout {
    pub path: PathBuf,
    _lease: File,
}

fn base(cas: &Path) -> PathBuf {
    crate::factory_scratch::configured_scratch_root(cas)
        .map(|root| cas_types::factory_scratch::project_scratch_dir(&root, cas).join("verify"))
        .unwrap_or_else(|| cas.join("review-scratch"))
}

fn component(value: &str) -> bool {
    let mut parts = Path::new(value).components();
    matches!(parts.next(), Some(Component::Normal(_))) && parts.next().is_none()
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .output()?;
    ensure!(
        output.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn safe_path(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => ensure!(
                !metadata.file_type().is_symlink(),
                "scratch path contains a symlink"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn private_dir(path: &Path) -> Result<()> {
    safe_path(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path)?;
    Ok(())
}

fn lease(path: &Path, create: bool) -> Result<File> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    }
    let file = options.open(path.join("lease"))?;
    file.try_lock_exclusive()
        .context("review checkout is leased")?;
    Ok(file)
}

fn write_owner(directory: &Path, record: &Owner) -> Result<()> {
    write_owner_with_directory_sync(directory, record, |directory| {
        File::open(directory)?.sync_all()
    })
}

fn write_owner_with_directory_sync(
    directory: &Path,
    record: &Owner,
    sync_directory: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(file.as_file_mut(), record)?;
    file.as_file().sync_all()?;
    file.persist(directory.join("owner.json"))?;
    // The synced payload alone does not make the renamed directory entry durable.
    sync_directory(directory)?;
    Ok(())
}

fn write_new_checkout_owner_with_parent_sync(
    directory: &Path,
    record: &Owner,
    mut sync_parent: impl FnMut(&Path) -> std::io::Result<()>,
) -> Result<()> {
    write_owner(directory, record)?;
    // The owner's child sync makes its contents durable; the store parent
    // sync publishes the checkout, and the scratch base sync publishes its task.
    let parent = directory.parent().context("review store parent missing")?;
    let scratch_base = parent.parent().context("review scratch base missing")?;
    if let Err(error) = sync_parent(parent).and_then(|()| sync_parent(scratch_base)) {
        // Only fresh checkouts reach this helper, before Git creates a worktree.
        // Removal keeps a retry on the fresh path so it repeats both syncs.
        fs::remove_dir_all(directory).with_context(|| {
            format!("remove incomplete review checkout after parent sync failure: {error}")
        })?;
        return Err(error.into());
    }
    Ok(())
}

fn stale_owner_temp(path: &Path) -> Result<bool> {
    let Some(suffix) = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(".tmp"))
    else {
        return Ok(false);
    };
    if suffix.len() != 6 || !suffix.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Ok(false);
    }
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() < 64 * 1024,
        "invalid review ownership temp file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let parent = fs::metadata(path.parent().context("owner temp parent missing")?)?;
        ensure!(
            metadata.uid() == parent.uid() && metadata.nlink() == 1,
            "review ownership temp file has another owner"
        );
    }
    Ok(true)
}

/// Worker worktrees inherit the main checkout's Cargo config in addition to
/// their own copy. Review checkouts live outside that tree. Restore inherited
/// flag arrays outside Git so Cargo keeps the same artifact identity, including
/// its own target/cfg selection and environment override precedence.
fn inherit_worker_rustflags(cas: &Path, directory: &Path) -> Result<()> {
    let source = cas
        .parent()
        .context("project root missing")?
        .join(".cargo/config.toml");
    let mut inherited = toml::Table::new();
    match fs::read_to_string(source) {
        Ok(text) => {
            let config: toml::Table = toml::from_str(&text)?;
            for section in ["build", "target"] {
                let Some(table) = config.get(section).and_then(toml::Value::as_table) else {
                    continue;
                };
                let flags = |table: &toml::Table| -> toml::Table {
                    ["rustflags", "rustdocflags"]
                        .into_iter()
                        .filter_map(|key| {
                            table.get(key).map(|value| (key.to_owned(), value.clone()))
                        })
                        .collect()
                };
                let selected: toml::Table = if section == "build" {
                    flags(table)
                } else {
                    table
                        .iter()
                        .filter_map(|(target, value)| {
                            let selected = flags(value.as_table()?);
                            (!selected.is_empty())
                                .then(|| (target.clone(), toml::Value::Table(selected)))
                        })
                        .collect()
                };
                if !selected.is_empty() {
                    inherited.insert(section.to_owned(), toml::Value::Table(selected));
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let config_dir = directory.join(".cargo");
    let destination = config_dir.join("config.toml");
    safe_path(&destination)?;
    if inherited.is_empty() {
        if destination.exists() {
            fs::remove_file(destination)?;
        }
    } else {
        private_dir(&config_dir)?;
        fs::write(destination, toml::to_string(&inherited)?)?;
    }
    Ok(())
}

/// Create or reuse one detached, disposable checkout per task/dispatch/head.
/// No legacy directory is adopted, and changed source is never reset.
/// Callers retain this value while running tests; standalone creation releases
/// the lease on return, and GC also checks open files, cwd, mappings and argv.
pub fn create(
    cas: &Path,
    repo: &Path,
    task_id: &str,
    dispatch_id: Option<&str>,
    head: &str,
) -> Result<Checkout> {
    let cas = cas.canonicalize()?;
    ensure!(component(task_id), "invalid scratch task id");
    crate::store::open_task_store(&cas)?.get(task_id)?;
    ensure!(
        matches!(head.len(), 40 | 64) && head.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "scratch checkout needs a full commit SHA"
    );
    git(
        repo,
        &["rev-parse", "--verify", &format!("{head}^{{commit}}")],
    )?;
    let common = git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    ensure!(
        Path::new(&common).canonicalize()?
            == Path::new(&git(
                cas.parent().context("project root missing")?,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"]
            )?)
            .canonicalize()?,
        "checkout must belong to the task's repository"
    );
    if let Some(id) = dispatch_id {
        let dispatch = cas_store::get_verification_dispatch(&cas, id)?;
        ensure!(
            dispatch.task_id == task_id,
            "dispatch belongs to a different task"
        );
    }
    let run = dispatch_id.unwrap_or("walk");
    ensure!(component(run), "invalid scratch dispatch id");
    let parent = base(&cas).join(task_id);
    private_dir(&parent)?;
    // Repeated admission refusals must not multiply full source checkouts.
    let directory = parent.join(format!("{run}-{head}"));
    safe_path(&directory)?;
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    let fresh = match builder.create(&directory) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(error.into()),
    };
    let lock = lease(&directory, fresh)?;
    if fresh {
        // Write ownership before Git, so interrupted creation stays visible.
        write_new_checkout_owner_with_parent_sync(
            &directory,
            &Owner {
                schema_version: 1,
                cas_root: cas.clone(),
                task_id: task_id.into(),
                dispatch_id: dispatch_id.map(str::to_owned),
                head: head.into(),
                created_at: Utc::now(),
                last_used_at: Some(Utc::now()),
            },
            |parent| File::open(parent)?.sync_all(),
        )?;
    } else {
        let record = owner(&cas, &directory)?;
        ensure!(
            record.head == head
                && record.task_id == task_id
                && record.dispatch_id.as_deref() == dispatch_id,
            "existing review checkout has another owner; it was not adopted"
        );
    }
    let path = directory.join("repo");
    if path.exists() {
        ensure!(
            is_managed_checkout(&cas, &path),
            "review checkout head or ownership changed"
        );
        discard_legacy_seed_stage(&directory, &path, &lock)?;
        ensure!(
            git(&path, &["status", "--porcelain", "--untracked-files=all"])?.is_empty(),
            "review checkout has source changes; preserve them before retrying"
        );
    } else {
        git(
            repo,
            &[
                "worktree",
                "add",
                "--detach",
                path.to_str().context("non-UTF-8 scratch path")?,
                head,
            ],
        )?;
    }
    inherit_worker_rustflags(&cas, &directory)?;
    // Successful reuse starts a new idle grace period; admission retries can
    // return to a checkout long after its original creation.
    let mut record = owner(&cas, &directory)?;
    record.last_used_at = Some(Utc::now());
    write_owner(&directory, &record)?;
    Ok(Checkout { path, _lease: lock })
}

fn owner(cas: &Path, directory: &Path) -> Result<Owner> {
    let record = read_owner(cas, directory)?;
    ensure!(
        directory.parent().and_then(Path::file_name) == Some(std::ffi::OsStr::new(&record.task_id)),
        "review task does not match directory"
    );
    Ok(record)
}

fn read_owner(cas: &Path, directory: &Path) -> Result<Owner> {
    safe_path(directory)?;
    let marker = directory.join("owner.json");
    let metadata = fs::symlink_metadata(&marker)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() < 64 * 1024,
        "invalid review ownership record"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let parent = fs::metadata(directory)?;
        // SAFETY: geteuid has no preconditions and only returns this process's effective user ID.
        let effective_uid = unsafe { libc::geteuid() };
        ensure!(
            parent.uid() == effective_uid
                && parent.mode() & 0o077 == 0
                && metadata.uid() == parent.uid()
                && metadata.nlink() == 1,
            "review directory must be private and owned by this user"
        );
    }
    let owner: Owner = serde_json::from_slice(&fs::read(marker)?)?;
    ensure!(
        owner.schema_version == 1
            && owner.cas_root == cas.canonicalize()?
            && component(&owner.task_id),
        "review owner does not match project"
    );
    Ok(owner)
}

/// Only marker-verified review paths widen capped admission beyond worker lanes.
pub(crate) fn is_managed_checkout(cas: &Path, repo: &Path) -> bool {
    // Match create's scratch base even when the caller uses an aliased root.
    let Ok(cas) = cas.canonicalize() else {
        return false;
    };
    let Some(directory) = repo.parent() else {
        return false;
    };
    if directory
        .strip_prefix(base(&cas))
        .ok()
        .is_none_or(|path| path.components().count() != 2)
        || repo.file_name() != Some(std::ffi::OsStr::new("repo"))
        || safe_path(repo).is_err()
    {
        return false;
    }
    owner(&cas, directory)
        .is_ok_and(|record| git(repo, &["rev-parse", "HEAD"]).is_ok_and(|head| head == record.head))
}

fn discard_legacy_seed_stage(directory: &Path, checkout: &Path, lock: &File) -> Result<()> {
    let staging = checkout.join(".target-seed-in-progress");
    let metadata = match fs::symlink_metadata(&staging) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "legacy review seed stage is not a directory"
    );
    ensure!(
        !crate::factory_target_cache::review_output_in_use(directory, lock),
        "legacy review seed stage may still be in use"
    );
    fs::remove_dir_all(staging)?;
    Ok(())
}

/// Resolve the task's assigned worker rather than another dispatch's output.
/// Called only inside the review lane's existing build admission and lease.
pub(crate) fn seed_target_from_worker(cas: &Path, checkout: &Path) -> Result<()> {
    if !is_managed_checkout(cas, checkout) || checkout.join("target/debug").exists() {
        return Ok(());
    }
    let record = owner(cas, checkout.parent().context("review directory missing")?)?;
    let task = crate::store::open_task_store(cas)?.get(&record.task_id)?;
    let agents = crate::store::open_agent_store(cas)?;
    let worker = task
        .assignee
        .as_deref()
        .and_then(|assignee| {
            crate::mcp::tools::core::task::resolve_agent_identity(agents.as_ref(), assignee)
        })
        .filter(|agent| agent.role == cas_types::AgentRole::Worker);
    if let Some(worker) = worker {
        let source = worker
            .metadata
            .get("clone_path")
            .map(PathBuf::from)
            .unwrap_or_else(|| cas.join("worktrees").join(&worker.name));
        if let Some(stats) =
            crate::ui::factory::seed_review_target_from_worker(cas, &source, checkout)?
        {
            tracing::info!(source=%stats.snapshot, files=stats.files, bytes=stats.bytes, "review target seeded from assigned worker");
        }
    }
    Ok(())
}

fn eligible(cas: &Path, record: &Owner) -> Result<()> {
    let task = crate::store::open_task_store(cas)?.get(&record.task_id)?;
    let finished = match &record.dispatch_id {
        Some(id) => {
            let dispatch = cas_store::get_verification_dispatch(cas, id)?;
            ensure!(dispatch.task_id == task.id, "dispatch task mismatch");
            ensure!(
                !matches!(
                    dispatch.state,
                    cas_types::VerificationDispatchState::Pending
                        | cas_types::VerificationDispatchState::Claimed
                ),
                "dispatch still active"
            );
            true
        }
        None => false,
    };
    ensure!(
        task.is_terminal() || finished,
        "task and dispatch still active"
    );
    ensure!(
        !cas_store::has_active_verification_dispatch(cas, &task.id)?,
        "task has an active verification dispatch"
    );
    Ok(())
}

/// A durable queue outside Git, still under the checkout's private ownership.
/// Its marker precedes the rename, so a crashed handoff is recoverable by GC.
fn trash(cas: &Path, directory: &Path) -> Result<Option<PathBuf>> {
    let path = directory.join("trash");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    safe_path(&path)?;
    ensure!(metadata.is_dir(), "review trash is not a directory");
    let source = owner(cas, directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions.
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0,
            "review trash must be private and owned by this user"
        );
    }
    match fs::symlink_metadata(path.join("owner.json")) {
        Ok(_) => {
            let queued = read_owner(cas, &path)?;
            ensure!(
                queued.task_id == source.task_id
                    && queued.dispatch_id == source.dispatch_id
                    && queued.head == source.head,
                "trash owner does not match checkout"
            );
        }
        // Older collectors could remove the marker before their process exited.
        // The private parent still proves ownership; corrupt markers never do.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    for entry in fs::read_dir(&path)? {
        let entry = entry?;
        let name = entry.file_name();
        ensure!(
            name == "owner.json" || name == "target" || stale_owner_temp(&entry.path())?,
            "unexpected file in review trash"
        );
    }
    safe_path(&path.join("target"))?;
    if path.join("target").exists() {
        ensure!(
            fs::symlink_metadata(path.join("target"))?.is_dir(),
            "trash target is not a directory"
        );
    }
    Ok(Some(path))
}

struct CollectorLock(File);
impl Drop for CollectorLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn collector_lock(cas: &Path, wait: bool) -> Result<Option<CollectorLock>> {
    let slots = cas.join("worker-check-slots");
    private_dir(&slots)?;
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    }
    let lock = options.open(slots.join("review-trash.lock"))?;
    if wait {
        lock.lock_exclusive()?;
    } else if let Err(error) = lock.try_lock_exclusive() {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            return Ok(None);
        }
        return Err(error.into());
    }
    Ok(Some(CollectorLock(lock)))
}

/// Accounting only: queued output is not free space, including while a build
/// lease or collector lock prevents this process from deleting it.
pub(crate) fn pending_reclaim_bytes(cas: &Path) -> u64 {
    pending_reclaim_bytes_until(cas, None).unwrap_or(0)
}

pub(crate) fn pending_reclaim_bytes_until(cas: &Path,
    deadline: Option<std::time::Instant>) -> Option<u64> {
    let mut total = 0u64;
    for directory in entries(&base(cas)).into_iter().flat_map(|task| entries(&task)) {
        if deadline.is_some_and(|end| std::time::Instant::now() >= end) { return None; }
        if let Some(path) = trash(cas, &directory).ok().flatten() {
            total = total.saturating_add(freeable_bytes_until(&path.join("target"), deadline)?);
        }
    }
    Some(total)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReclaimMode {
    Inspect,
    Collect,
    Stage,
}

fn source_is_old(record: &Owner) -> bool {
    Utc::now()
        .signed_duration_since(record.last_used_at.unwrap_or(record.created_at))
        .num_seconds()
        >= MIN_AGE_SECS
}

fn bytes(path: &Path, limit: Option<u64>) -> u64 {
    bytes_until(path, limit, None).unwrap_or(0)
}

fn bytes_until(path: &Path, limit: Option<u64>, deadline: Option<std::time::Instant>) -> Option<u64> {
    let mut size = 0u64;
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if deadline.is_some_and(|end| std::time::Instant::now() >= end) { return None; }
        if !entry.file_type().is_file() { continue; }
        let Ok(metadata) = entry.metadata() else { continue; };
        size = size.saturating_add(metadata.len());
        if limit.is_some_and(|limit| size >= limit) {
            break;
        }
    }
    Some(size)
}

/// Disk space is released only when a deletion scope owns every link to an
/// inode. Track just multi-link files so ordinary Cargo targets stay cheap.
#[derive(Default)]
struct FreeableInodes {
    single_link_bytes: u64,
    #[cfg(unix)]
    linked: HashMap<(u64, u64), (u64, u64, u64)>, // (dev, ino) -> (size, nlink, seen)
}

impl FreeableInodes {
    fn add(&mut self, metadata: &fs::Metadata) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() > 1 {
                let entry = self
                    .linked
                    .entry((metadata.dev(), metadata.ino()))
                    .or_insert((metadata.len(), metadata.nlink(), 0));
                entry.1 = entry.1.max(metadata.nlink());
                entry.2 = entry.2.saturating_add(1);
                return;
            }
        }
        self.single_link_bytes = self.single_link_bytes.saturating_add(metadata.len());
    }

    fn bytes(&self) -> u64 {
        #[cfg(unix)]
        {
            self.linked
                .values()
                .filter(|(_, nlink, seen)| seen == nlink)
                .fold(self.single_link_bytes, |total, (size, _, _)| {
                    total.saturating_add(*size)
                })
        }
        #[cfg(not(unix))]
        {
            self.single_link_bytes
        }
    }
}

fn freeable_bytes(path: &Path) -> u64 {
    freeable_bytes_until(path, None).unwrap_or(0)
}

fn freeable_bytes_until(path: &Path, deadline: Option<std::time::Instant>) -> Option<u64> {
    let mut inodes = FreeableInodes::default();
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if deadline.is_some_and(|end| std::time::Instant::now() >= end) { return None; }
        if !entry.file_type().is_file() { continue; }
        let Ok(metadata) = entry.metadata() else { continue; };
        inodes.add(&metadata);
    }
    Some(inodes.bytes())
}

#[derive(Default)]
struct ReviewBytes {
    total: u64,
    reclaimable: u64,
    target: u64,
    pending: u64,
}

/// One walk supplies the row size and both disposable payload sizes.
fn review_bytes(directory: &Path) -> ReviewBytes {
    review_bytes_until(directory, None).unwrap_or_default()
}

fn review_bytes_until(directory: &Path, deadline: Option<std::time::Instant>) -> Option<ReviewBytes> {
    let target = directory.join("repo/target");
    let pending = directory.join("trash/target");
    let mut sizes = ReviewBytes::default();
    let mut whole_inodes = FreeableInodes::default();
    let mut target_inodes = FreeableInodes::default();
    let mut pending_inodes = FreeableInodes::default();
    for entry in walkdir::WalkDir::new(directory)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if deadline.is_some_and(|end| std::time::Instant::now() >= end) { return None; }
        if !entry.file_type().is_file() { continue; }
        if let Ok(metadata) = entry.metadata() {
            sizes.total = sizes.total.saturating_add(metadata.len());
            whole_inodes.add(&metadata);
            if entry.path().starts_with(&target) {
                target_inodes.add(&metadata);
            } else if entry.path().starts_with(&pending) {
                pending_inodes.add(&metadata);
            }
        }
    }
    sizes.reclaimable = whole_inodes.bytes();
    sizes.target = target_inodes.bytes();
    sizes.pending = pending_inodes.bytes();
    Some(sizes)
}

fn entries(path: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    paths
}

fn reclaim(
    cas: &Path,
    directory: &Path,
    mode: ReclaimMode,
    targets_only: bool,
    measured: Option<&ReviewBytes>,
) -> Result<(&'static str, u64)> {
    let clean = mode == ReclaimMode::Collect;
    let lock = lease(directory, false)?;
    let record = owner(cas, directory)?;
    eligible(cas, &record)?;
    ensure!(
        !crate::factory_target_cache::review_output_in_use(directory, &lock),
        "live or unverifiable review output"
    );
    let repo = cas.parent().context("project root missing")?;
    let checkout = directory.join("repo");
    let _lane = if checkout.exists() {
        Some(
            crate::factory_worker_check::try_lock_lane(cas, &checkout)?
                .context("review build lane is leased")?,
        )
    } else {
        None
    };
    let registrations = git(repo, &["worktree", "list", "--porcelain", "-z"])?;
    let registered: Vec<_> = registrations
        .split('\0')
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect();
    ensure!(
        !registered
            .iter()
            .any(|path| path.starts_with(directory) && path != &checkout),
        "unexpected nested worktree retained"
    );
    if checkout.exists() {
        ensure!(
            is_managed_checkout(cas, &checkout),
            "review checkout head or ownership changed"
        );
        let expected = git(
            repo,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        ensure!(
            git(
                &checkout,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"]
            )? == expected,
            "review Git registration belongs to another repository"
        );
        if clean {
            discard_legacy_seed_stage(directory, &checkout, &lock)?;
        }
        ensure!(
            git(
                &checkout,
                &["status", "--porcelain", "--untracked-files=all"]
            )?
            .is_empty(),
            "review checkout has source changes"
        );
    }
    // Recheck state and process evidence after inventory, before touching Git.
    eligible(cas, &owner(cas, directory)?)?;
    ensure!(
        !crate::factory_target_cache::review_output_in_use(directory, &lock),
        "review output became live"
    );
    let queued = trash(cas, directory)?;
    if mode == ReclaimMode::Stage {
        // A retry never overwrites an earlier interrupted handoff. The
        // collector/GC can drain it and any later target under the same guards.
        if queued.is_some() {
            return Ok(("target queued; source retained", 0));
        }
        let target = checkout.join("target");
        if !target.exists() {
            return Ok(("source retained (target absent)", 0));
        }
        safe_path(&target)?;
        ensure!(
            fs::symlink_metadata(&target)?.is_dir(),
            "review target is not a directory"
        );
        ensure!(
            git(&checkout, &["ls-files", "--", "target"])?.is_empty(),
            "review target contains tracked source"
        );
        let queue = directory.join("trash");
        private_dir(&queue)?;
        write_owner(&queue, &record)?;
        fs::rename(&target, queue.join("target"))?;
        // Check after rename as well: process descriptors now resolve through
        // trash. Restore when possible if a user appeared during the handoff.
        if eligible(cas, &owner(cas, directory)?).is_err()
            || crate::factory_target_cache::review_output_in_use(directory, &lock)
        {
            let _ = fs::rename(queue.join("target"), &target);
            return Err(anyhow::anyhow!(
                "review output became live after rename; retained"
            ));
        }
        return Ok(("target queued; source retained", 0));
    }
    let queued_bytes = queued.as_ref().map_or(0, |path| {
        measured.map_or_else(
            || freeable_bytes(&path.join("target")),
            |sizes| sizes.pending,
        )
    });
    if clean && let Some(path) = &queued {
        // Keep ownership durable until every payload byte is gone. A verifier
        // process can exit during recursive deletion, or between these steps.
        if let Err(error) = fs::remove_dir_all(path.join("target"))
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error.into());
        }
        if let Err(error) = fs::remove_file(path.join("owner.json"))
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error.into());
        }
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            ensure!(
                stale_owner_temp(&entry.path())?,
                "unexpected file in review trash"
            );
            fs::remove_file(entry.path())?;
        }
        fs::remove_dir(path)?;
    }
    if targets_only || !source_is_old(&record) {
        let target = checkout.join("target");
        if !target.exists() {
            return Ok((
                if queued.is_some() {
                    if clean {
                        "target reclaimed; source retained"
                    } else {
                        "target reclaimable; source retained"
                    }
                } else {
                    "source retained (younger than 2 hours or target-only GC)"
                },
                queued_bytes,
            ));
        }
        safe_path(&target)?;
        ensure!(
            fs::symlink_metadata(&target)?.is_dir(),
            "review target is not a directory"
        );
        ensure!(
            git(&checkout, &["ls-files", "--", "target"])?.is_empty(),
            "review target contains tracked source"
        );
        let size = measured
            .map_or_else(|| freeable_bytes(&target), |sizes| sizes.target)
            .saturating_add(queued_bytes);
        if clean {
            fs::remove_dir_all(&target)?;
        }
        return Ok((
            if clean {
                "target reclaimed; source retained"
            } else {
                "target reclaimable; source retained"
            },
            size,
        ));
    }
    let size = measured.map_or_else(
        || freeable_bytes(directory).saturating_add(if clean { queued_bytes } else { 0 }),
        |sizes| sizes.reclaimable,
    );
    if !clean {
        return Ok(("reclaimable", size));
    }
    if registered.contains(&checkout) {
        let path = checkout.to_str().context("non-UTF-8 checkout")?;
        if checkout.exists() {
            // State/process probes can take time; include untracked files even
            // when local Git configuration hides them from worktree removal.
            ensure!(
                git(
                    &checkout,
                    &["status", "--porcelain", "--untracked-files=all"]
                )?
                .is_empty(),
                "review checkout has source changes"
            );
            // Git rechecks dirtiness itself. Never force removal of existing
            // source: ignored Cargo output is already disposable without it.
            git(repo, &["worktree", "remove", path])?;
        } else {
            // Only a vanished checkout permits forced stale-registration removal.
            git(repo, &["worktree", "remove", "--force", path])?;
        }
    }
    fs::remove_dir_all(directory)?;
    Ok(("reclaimed", size))
}

fn candidates(cas: &Path, task_id: Option<&str>) -> Vec<PathBuf> {
    let root = base(cas);
    let tasks = match task_id {
        Some(id) if component(id) => vec![root.join(id)],
        Some(_) => return Vec::new(),
        None => entries(&root),
    };
    tasks
        .into_iter()
        .flat_map(|task| entries(&task))
        .filter(|directory| {
            directory.join("repo/target").exists() || directory.join("trash").exists()
        })
        .filter(|directory| {
            owner(cas, directory)
                .and_then(|record| eligible(cas, &record))
                .is_ok()
        })
        .take(MAX_RECLAIMS_PER_PASS)
        .collect()
}

fn collect_targets(cas: &Path, task_id: Option<&str>) -> u64 {
    candidates(cas, task_id)
        .into_iter()
        .filter_map(|directory| reclaim(cas, &directory, ReclaimMode::Collect, true, None).ok())
        .map(|(_, size)| size)
        .sum()
}

/// Admission/GC can recover interrupted handoffs. A busy collector leaves the
/// queued bytes visible; only actual unlinking contributes to reclaimed bytes.
pub(crate) fn reclaim_targets(cas: &Path, task_id: Option<&str>) -> u64 {
    let Ok(cas) = cas.canonicalize() else {
        return 0;
    };
    let Ok(Some(_collector)) = collector_lock(&cas, false) else {
        return 0;
    };
    collect_targets(&cas, task_id)
}

/// Await bounded guarded renames, never recursive measurement or deletion.
/// The detached collector holds no build lane while waiting its global turn.
pub(crate) async fn reclaim_task_targets(cas: &Path, task_id: &str) {
    let Ok(cas) = cas.canonicalize() else {
        return;
    };
    let task_id = task_id.to_owned();
    let staging_root = cas.clone();
    let staged = tokio::task::spawn_blocking(move || {
        candidates(&staging_root, Some(&task_id))
            .into_iter()
            .filter(|directory| match reclaim(&staging_root, directory, ReclaimMode::Stage, true, None) {
                Ok(_) => true,
                Err(error) => {
                    tracing::warn!(%error, path=%directory.display(), "review target handoff deferred to GC");
                    false
                }
            })
            .count()
    })
    .await;
    match staged {
        Ok(0) => {}
        Ok(_) => {
            // A short-lived CLI runtime must not wait for bulk unlinking on
            // Tokio shutdown. Process exit may interrupt this thread; GC owns
            // recovery from the durable, ownership-checked queue.
            if let Err(error) = std::thread::Builder::new()
                .name("review-trash".into())
                .spawn(move || match collector_lock(&cas, true) {
                    Ok(Some(_collector)) => {
                        collect_targets(&cas, None);
                    }
                    Ok(None) => {}
                    Err(error) => tracing::warn!(%error, "review trash cleanup deferred to GC"),
                })
            {
                tracing::warn!(%error, "review trash worker unavailable; deferred to GC");
            }
        }
        Err(error) => tracing::warn!(%error, "review target handoff deferred to GC"),
    }
}

/// Shared public GC path. Unknown/malformed owners are retained and reported.
pub(crate) fn render(cas: &Path, clean: bool) -> String {
    render_until(cas, clean, None)
}

pub(crate) fn render_until(cas: &Path, clean: bool, deadline: Option<std::time::Instant>) -> String {
    let cas = cas.canonicalize().unwrap_or_else(|_| cas.to_path_buf());
    let cas = cas.as_path();
    let base = base(cas);
    let mut rows = Vec::new();
    let mut reclaimed = 0u64;
    let mut reclaimable = 0u64;
    let mut pending_total = 0u64;
    let mut cleaned = 0usize;
    let collector = clean
        .then(|| collector_lock(cas, false).ok().flatten())
        .flatten();
    for task in entries(&base) {
        for directory in entries(&task) {
            let Some(sizes) = review_bytes_until(&directory, deadline) else {
                return "\nManaged review scratch: report inventory incomplete (scan budget exhausted; none deleted)\n".into();
            };
            let size = sizes.total;
            let mutate = collector.is_some() && cleaned < MAX_RECLAIMS_PER_PASS;
            let pending = trash(cas, &directory)
                .ok()
                .flatten()
                .map_or(0, |_| sizes.pending);
            let mut candidate_bytes = 0;
            let mode = if mutate {
                ReclaimMode::Collect
            } else {
                ReclaimMode::Inspect
            };
            let disposition = match reclaim(cas, &directory, mode, false, Some(&sizes)) {
                Ok((status, bytes)) => {
                    candidate_bytes = bytes;
                    if mutate && matches!(status, "reclaimed" | "target reclaimed; source retained")
                    {
                        reclaimed += bytes;
                        cleaned += 1;
                    }
                    if !(mutate
                        && matches!(status, "reclaimed" | "target reclaimed; source retained"))
                    {
                        pending_total = pending_total.saturating_add(pending);
                    }
                    status.to_owned()
                }
                Err(error) => {
                    // A failed collection may have unlinked part of the queue;
                    // retain the pre-pass snapshot as a conservative bound.
                    pending_total = pending_total.saturating_add(pending);
                    format!("retained: {error:#}")
                }
            };
            reclaimable += candidate_bytes;
            rows.push(
                serde_json::json!({"path": directory, "bytes": size, "reclaimable_bytes": candidate_bytes, "disposition": disposition}),
            );
        }
    }
    let pending = pending_total;
    let deferred = clean && collector.is_none();
    let deferred_text = if deferred {
        "; deletion deferred (collector busy or unavailable)"
    } else {
        ""
    };
    let mut out = format!(
        "\nManaged review scratch: {} checkouts; reclaimed {reclaimed} bytes\nReclaimable: {reclaimable} bytes; pending deletion: {pending} bytes{deferred_text}\n",
        rows.len()
    );
    for row in &rows {
        out.push_str(&format!(
            "- {}: {} ({} bytes)\n",
            row["path"].as_str().unwrap_or("?"),
            row["disposition"].as_str().unwrap_or("?"),
            row["bytes"]
        ));
    }
    out.push_str(&format!("REVIEW_SCRATCH_STATUS_JSON={}\n", serde_json::json!({"schema_version": 1, "rows": rows, "reclaimable_bytes": reclaimable, "reclaimed_bytes": reclaimed, "pending_reclaim_bytes": pending, "dry_run": !clean, "deferred": deferred})));
    out
}

/// Inventory-only evidence for the ad-hoc siblings that motivated this GC.
pub(crate) fn unmanaged_large(cas: &Path) -> String {
    unmanaged_large_until(cas, None)
}

pub(crate) fn unmanaged_large_until(cas: &Path, deadline: Option<std::time::Instant>) -> String {
    let Some(root) = crate::factory_scratch::configured_scratch_root(cas) else {
        return String::new();
    };
    let mut out = String::new();
    for path in entries(&root) {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        // Project namespaces belong to their own store; never classify peers as leaks.
        if name.rsplit_once('-').is_some_and(|(_, key)| {
            key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            continue;
        }
        let Some(size) = bytes_until(&path, Some(LARGE_BYTES), deadline) else {
            out.push_str("\nUnmanaged scratch: report inventory incomplete (scan budget exhausted; none deleted)\n");
            break;
        };
        if size >= LARGE_BYTES {
            if out.is_empty() {
                out.push_str("\nLarge unmanaged scratch directories (review-only; >= 1 GiB):\n");
            }
            out.push_str(&format!(
                "- {}: at least {size} bytes (never auto-deleted)\n",
                path.display()
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[cfg(unix)]
    #[test]
    fn review_bytes_excludes_hardlinks_retained_by_worker_cas_2ffc() {
        let root = tempfile::tempdir().unwrap();
        let review = root.path().join("review");
        let target = review.join("repo/target");
        let pending = review.join("trash/target");
        fs::create_dir_all(&target).unwrap();
        fs::create_dir_all(&pending).unwrap();
        let worker = root.path().join("worker-artifact");
        fs::write(&worker, b"shared-byte").unwrap();
        fs::hard_link(&worker, target.join("shared")).unwrap();
        fs::hard_link(&worker, pending.join("shared")).unwrap();
        fs::write(target.join("private"), b"free").unwrap();
        fs::write(target.join("local"), b"local-pair").unwrap();
        fs::hard_link(target.join("local"), target.join("local-copy")).unwrap();
        let sizes = review_bytes(&review);
        assert_eq!(sizes.total, 46, "logical row size includes every path");
        assert_eq!(sizes.target, 14, "intra-target inode is counted once");
        assert_eq!(
            sizes.pending, 0,
            "shared pending bytes remain in the worker"
        );
        assert_eq!(
            sizes.reclaimable, 14,
            "full review deletion retains worker inode"
        );
        assert_eq!(
            freeable_bytes(&target),
            14,
            "target deletion owns both local links"
        );
        assert_eq!(
            freeable_bytes(&pending),
            0,
            "worker still owns the pending inode"
        );
        assert_eq!(
            freeable_bytes(&review),
            14,
            "whole review counts local inode once"
        );
    }

    #[test]
    fn write_owner_syncs_directory_after_rename_cas_a271() {
        let directory = tempfile::tempdir().unwrap();
        let record = Owner {
            schema_version: 1,
            cas_root: directory.path().to_path_buf(),
            task_id: "cas-owner-sync".into(),
            dispatch_id: None,
            head: "a".repeat(40),
            created_at: Utc::now(),
            last_used_at: None,
        };
        let synced = Cell::new(false);
        let result = write_owner_with_directory_sync(directory.path(), &record, |parent| {
            assert_eq!(parent, directory.path());
            let published: Owner = serde_json::from_slice(
                &fs::read(parent.join("owner.json")).expect("rename must precede directory sync"),
            )
            .unwrap();
            assert_eq!(published.task_id, record.task_id);
            assert_eq!(
                fs::read_dir(parent).unwrap().count(),
                1,
                "temp marker must be renamed"
            );
            File::open(parent)?.sync_all()?;
            synced.set(true);
            Err(std::io::Error::other(
                "injected owner directory sync failure",
            ))
        });
        assert!(
            synced.get(),
            "owner publication must sync its parent directory"
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("injected owner directory sync failure")
        );
    }

    #[test]
    fn new_checkout_syncs_store_parent_and_propagates_failure_cas_34b0() {
        let scratch = tempfile::tempdir().unwrap();
        let scratch_path = scratch.path().canonicalize().unwrap();
        let store = scratch_path.join("cas-checkout-sync");
        private_dir(&store).unwrap();
        let checkout = store.join("checkout");
        fs::create_dir(&checkout).unwrap();
        // Production holds the fresh checkout's lease during ownership publication.
        let lock = lease(&checkout, true).unwrap();
        let record = Owner {
            schema_version: 1,
            cas_root: scratch_path.clone(),
            task_id: "cas-checkout-sync".into(),
            dispatch_id: None,
            head: "a".repeat(40),
            created_at: Utc::now(),
            last_used_at: None,
        };
        let synced = Cell::new(false);
        let result = write_new_checkout_owner_with_parent_sync(&checkout, &record, |parent| {
            assert_eq!(parent, store);
            assert!(checkout.join("owner.json").is_file());
            synced.set(true);
            Err(std::io::Error::other("injected store directory sync failure"))
        });
        assert!(synced.get(), "new checkout must sync its store parent");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("injected store directory sync failure")
        );
        assert!(!checkout.exists(), "failed publication must not be reusable as a durable checkout");
        drop(lock);
        fs::create_dir(&checkout).unwrap();
        let retried = std::cell::RefCell::new(Vec::new());
        write_new_checkout_owner_with_parent_sync(&checkout, &record, |parent| {
            retried.borrow_mut().push(parent.to_path_buf());
            File::open(parent)?.sync_all()
        }).unwrap();
        assert_eq!(*retried.borrow(), [store, scratch_path],
            "retry must sync both directory entries before using the checkout");
    }

    #[test]
    fn first_checkout_syncs_task_entry_in_scratch_base_cas_da0a() {
        let scratch = tempfile::tempdir().unwrap();
        let scratch_path = scratch.path().canonicalize().unwrap();
        let store = scratch_path.join("cas-new-task");
        private_dir(&store).unwrap();
        let checkout = store.join("checkout");
        fs::create_dir(&checkout).unwrap();
        let record = Owner {
            schema_version: 1,
            cas_root: scratch_path.clone(),
            task_id: "cas-new-task".into(),
            dispatch_id: None,
            head: "a".repeat(40),
            created_at: Utc::now(),
            last_used_at: None,
        };
        let synced = std::cell::RefCell::new(Vec::new());
        let result = write_new_checkout_owner_with_parent_sync(&checkout, &record, |parent| {
            assert!(checkout.join("owner.json").is_file());
            synced.borrow_mut().push(parent.to_path_buf());
            if parent == scratch_path {
                Err(std::io::Error::other("injected scratch base sync failure"))
            } else {
                File::open(parent)?.sync_all()
            }
        });
        assert!(result.unwrap_err().to_string().contains("injected scratch base sync failure"));
        assert_eq!(*synced.borrow(), [store, scratch_path]);
        assert!(!checkout.exists(), "an unconfirmed task entry cannot publish a checkout");
    }
}
