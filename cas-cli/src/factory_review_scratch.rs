//! Disposable task/dispatch-owned proof checkouts shared by reviewers and GC.
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

const MIN_AGE_SECS: i64 = 2 * 3600;
const LARGE_BYTES: u64 = 1024 * 1024 * 1024;

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
        fs::write(
            directory.join("owner.json"),
            serde_json::to_vec(&Owner {
                schema_version: 1,
                cas_root: cas.clone(),
                task_id: task_id.into(),
                dispatch_id: dispatch_id.map(str::to_owned),
                head: head.into(),
                created_at: Utc::now(),
                last_used_at: Some(Utc::now()),
            })?,
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
    // Successful reuse starts a new idle grace period; admission retries can
    // return to a checkout long after its original creation.
    let mut record = owner(&cas, &directory)?;
    record.last_used_at = Some(Utc::now());
    fs::write(directory.join("owner.json"), serde_json::to_vec(&record)?)?;
    Ok(Checkout { path, _lease: lock })
}

fn owner(cas: &Path, directory: &Path) -> Result<Owner> {
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
    ensure!(
        directory.parent().and_then(Path::file_name) == Some(std::ffi::OsStr::new(&owner.task_id)),
        "review task does not match directory"
    );
    Ok(owner)
}

/// Only marker-verified review paths widen capped admission beyond worker lanes.
pub(crate) fn is_managed_checkout(cas: &Path, repo: &Path) -> bool {
    let Some(directory) = repo.parent() else {
        return false;
    };
    if directory
        .strip_prefix(base(cas))
        .ok()
        .is_none_or(|path| path.components().count() != 2)
        || repo.file_name() != Some(std::ffi::OsStr::new("repo"))
        || safe_path(repo).is_err()
    {
        return false;
    }
    owner(cas, directory)
        .is_ok_and(|record| git(repo, &["rev-parse", "HEAD"]).is_ok_and(|head| head == record.head))
}

fn eligible(cas: &Path, record: &Owner) -> Result<()> {
    let task = crate::store::open_task_store(cas)?.get(&record.task_id)?;
    let resolved = match &record.dispatch_id {
        Some(id) => {
            let dispatch = cas_store::get_verification_dispatch(cas, id)?;
            ensure!(dispatch.task_id == task.id, "dispatch task mismatch");
            dispatch.state == cas_types::VerificationDispatchState::Resolved
        }
        None => false,
    };
    ensure!(
        task.is_terminal() || resolved,
        "task and dispatch still active"
    );
    ensure!(
        Utc::now()
            .signed_duration_since(record.last_used_at.unwrap_or(record.created_at))
            .num_seconds()
            >= MIN_AGE_SECS,
        "review checkout younger than 2 hours"
    );
    Ok(())
}

fn bytes(path: &Path, limit: Option<u64>) -> u64 {
    let mut size = 0u64;
    for metadata in walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok())
    {
        size = size.saturating_add(metadata.len());
        if limit.is_some_and(|limit| size >= limit) {
            break;
        }
    }
    size
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

fn reclaim(cas: &Path, directory: &Path, clean: bool) -> Result<&'static str> {
    let lock = lease(directory, false)?;
    let record = owner(cas, directory)?;
    eligible(cas, &record)?;
    ensure!(
        !crate::factory_target_cache::review_output_in_use(directory, &lock),
        "live or unverifiable review output"
    );
    let repo = cas.parent().context("project root missing")?;
    let checkout = directory.join("repo");
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
        ensure!(
            git(
                &checkout,
                &["status", "--porcelain", "--untracked-files=all"]
            )?
            .is_empty(),
            "review checkout has source changes"
        );
    }
    if !clean {
        return Ok("reclaimable");
    }
    // Recheck state and process evidence after inventory, before touching Git.
    eligible(cas, &owner(cas, directory)?)?;
    ensure!(
        !crate::factory_target_cache::review_output_in_use(directory, &lock),
        "review output became live"
    );
    if registered.contains(&checkout) {
        let path = checkout.to_str().context("non-UTF-8 checkout")?;
        if checkout.exists() {
            // Git rechecks dirtiness itself. Never force removal of existing
            // source: ignored Cargo output is already disposable without it.
            git(repo, &["worktree", "remove", path])?;
        } else {
            // Only a vanished checkout permits forced stale-registration removal.
            git(repo, &["worktree", "remove", "--force", path])?;
        }
    }
    fs::remove_dir_all(directory)?;
    Ok("reclaimed")
}

/// Shared public GC path. Unknown/malformed owners are retained and reported.
pub(crate) fn render(cas: &Path, clean: bool) -> String {
    let base = base(cas);
    let mut rows = Vec::new();
    let mut reclaimed = 0u64;
    for task in entries(&base) {
        for directory in entries(&task) {
            let size = bytes(&directory, None);
            let disposition = match reclaim(cas, &directory, clean) {
                Ok(status) => {
                    if status == "reclaimed" {
                        reclaimed += size;
                    }
                    status.to_owned()
                }
                Err(error) => format!("retained: {error:#}"),
            };
            rows.push(
                serde_json::json!({"path": directory, "bytes": size, "disposition": disposition}),
            );
        }
    }
    let mut out = format!(
        "\nManaged review scratch: {} checkouts; reclaimed {reclaimed} bytes\n",
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
    out.push_str(&format!("REVIEW_SCRATCH_STATUS_JSON={}\n", serde_json::json!({"schema_version": 1, "rows": rows, "reclaimed_bytes": reclaimed, "dry_run": !clean})));
    out
}

/// Inventory-only evidence for the ad-hoc siblings that motivated this GC.
pub(crate) fn unmanaged_large(cas: &Path) -> String {
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
        let size = bytes(&path, Some(LARGE_BYTES));
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
