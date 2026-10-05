//! Killable worker provisioning. Every store, Git and filesystem operation
//! runs in a separate process group, including base resolution before checkout.

use super::{WorkerSpawnContext, WorkerSpawnResult, WorktreePrep};
use anyhow::Context;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct ProvisionedWorker {
    pub(crate) result: WorkerSpawnResult,
    pub(crate) warnings: Vec<String>,
    pub(crate) base_provenance: Option<String>,
    pub(crate) receipt: String,
    #[serde(skip)]
    retirement: Option<RetirementGuard>,
}

impl ProvisionedWorker {
    pub(crate) fn retire(&self) {
        if let Some(guard) = &self.retirement {
            guard.cancellation.cancel();
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ProvisionerInput {
    build_hash: String,
    // Decode this only after checking the build, including after a macOS update.
    context: serde_json::Value,
}

fn build_hash() -> &'static str {
    option_env!("CAS_GIT_HASH").unwrap_or("unknown")
}

fn strip_deleted_suffix(path: PathBuf) -> PathBuf {
    path.to_str()
        .and_then(|s| s.strip_suffix(" (deleted)"))
        .map(PathBuf::from)
        .unwrap_or(path)
}

fn provisioner_executable() -> anyhow::Result<PathBuf> {
    #[cfg(test)]
    {
        Ok(crate::test_paths::cas_binary())
    }
    #[cfg(not(test))]
    {
        // Linux keeps the old executable inode here after an atomic update.
        #[cfg(target_os = "linux")]
        if Path::new("/proc/self/exe").exists() {
            return Ok(PathBuf::from("/proc/self/exe"));
        }
        Ok(strip_deleted_suffix(std::env::current_exe()?))
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CleanupPlan {
    repo: PathBuf,
    path: PathBuf,
    branch: String,
    common_dir: PathBuf,
    previous_admin_dirs: Vec<PathBuf>,
}

impl CleanupPlan {
    fn capture(wt: &WorktreePrep) -> anyhow::Result<Option<Self>> {
        // Never retire a reused checkout or a branch owned before this spawn.
        if wt.worktree_path.exists()
            || crate::worktree::GitOperations::new(wt.repo_root.clone())
                .branch_exists(&wt.branch_name)?
        {
            return Ok(None);
        }
        let output = Command::new("git")
            .arg("-C")
            .arg(&wt.repo_root)
            .args(["rev-parse", "--git-common-dir"])
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "cannot resolve provisioning Git directory"
        );
        let common_dir = wt
            .repo_root
            .join(String::from_utf8(output.stdout)?.trim())
            .canonicalize()?;
        let previous_admin_dirs = std::fs::read_dir(common_dir.join("worktrees"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        Ok(Some(Self {
            repo: wt.repo_root.clone(),
            path: wt.worktree_path.clone(),
            branch: wt.branch_name.clone(),
            common_dir,
            previous_admin_dirs,
        }))
    }

    fn cleanup(self) -> String {
        // Only locks belonging to this fresh worker ref/admin directory may be removed.
        // Never touch shared index.lock, packed-refs.lock, or other workers' refs.
        if self.branch.starts_with("factory/")
            && Path::new(&self.branch)
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
        {
            let _ = std::fs::remove_file(
                self.common_dir
                    .join("refs/heads")
                    .join(format!("{}.lock", self.branch)),
            );
        }
        let mut owned_admin = Vec::new();
        for entry in std::fs::read_dir(self.common_dir.join("worktrees"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            let admin = entry.path();
            if self.previous_admin_dirs.contains(&admin) {
                continue;
            }
            let binding = std::fs::read_to_string(admin.join("gitdir")).ok();
            if binding
                .as_ref()
                .is_some_and(|s| Path::new(s.trim()) == self.path.join(".git"))
                || (binding.is_none() && admin.file_name() == self.path.file_name())
            {
                let _ = std::fs::remove_file(admin.join("index.lock"));
                let _ = std::fs::remove_file(admin.join("HEAD.lock"));
                owned_admin.push(admin);
            }
        }
        let run = |args: &[&std::ffi::OsStr]| -> anyhow::Result<()> {
            let mut command = Command::new("git");
            command
                .arg("-C")
                .arg(&self.repo)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            run_command(
                command,
                &ProvisioningCancellation::default(),
                Instant::now(),
                Duration::from_secs(2),
            )
        };
        let remove = run(&[
            "worktree".as_ref(),
            "remove".as_ref(),
            "--force".as_ref(),
            self.path.as_os_str(),
        ]);
        if self.path.exists() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
        for admin in owned_admin {
            let _ = std::fs::remove_dir_all(admin);
        }
        let branch = run(&["branch".as_ref(), "-D".as_ref(), self.branch.as_ref()]);
        format!(
            "best-effort spawn cleanup: worktree={}, branch={}, path_remaining={}",
            if remove.is_ok() {
                "removed"
            } else {
                "pruned partial checkout"
            },
            if branch.is_ok() {
                "removed"
            } else {
                "absent or removal failed"
            },
            self.path.exists()
        )
    }
}

/// A completed result may be dropped by a queue reset before its consumer runs.
/// Keep cancellation ownership with that result so this race still retires it.
struct RetirementGuard {
    cancellation: Arc<ProvisioningCancellation>,
    plan: Option<CleanupPlan>,
    lease: Option<std::fs::File>,
}
impl Drop for RetirementGuard {
    fn drop(&mut self) {
        if self.cancellation.cancelled.load(Ordering::SeqCst)
            && let Some(plan) = self.plan.take()
        {
            let lease = self.lease.take();
            let _ = std::thread::Builder::new()
                .name("factory-spawn-cleanup".into())
                .spawn(move || {
                    // Event arguments are lazy: cleanup must run even when WARN is disabled.
                    let detail = plan.cleanup();
                    tracing::warn!(%detail, "retired completed provisioner");
                    drop(lease);
                });
        }
    }
}

fn stderr_tail(path: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(4096)));
    let mut bytes = Vec::new();
    let _ = file.take(4096).read_to_end(&mut bytes);
    let tail = String::from_utf8_lossy(&bytes).trim().to_string();
    if tail.is_empty() {
        String::new()
    } else {
        format!("; provisioner stderr tail: {tail}")
    }
}

/// Cancellation targets only the process group owned by this generation.
/// Aborting a started spawn_blocking handle alone cannot stop its writes.
#[derive(Default)]
pub(crate) struct ProvisioningCancellation {
    cancelled: AtomicBool,
    child_pid: Mutex<Option<u32>>,
}

impl ProvisioningCancellation {
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let pid = self.child_pid.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pid) = *pid {
            kill_group(pid);
        }
    }
}

fn kill_group(pid: u32) {
    #[cfg(unix)]
    unsafe {
        // The child was launched with process_group(0). Never signal the
        // daemon's group, which also owns live worker harnesses.
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

/// Entry used before ordinary CLI initialization, without a PTY or daemon.
/// Input/output contain paths and spawn metadata, never credential values.
pub fn run_internal_provisioner(input: &Path, output: &Path) -> anyhow::Result<()> {
    let input_data: ProvisionerInput = serde_json::from_reader(std::fs::File::open(input)?)?;
    anyhow::ensure!(
        input_data.build_hash == build_hash(),
        "provision_build_mismatch: daemon build {} differs from provisioner build {}; restart the factory after update",
        input_data.build_hash,
        build_hash()
    );
    let context: WorkerSpawnContext = serde_json::from_value(input_data.context)?;
    let outcome = (|| {
        let prep = context.resolve()?;
        serde_json::to_writer(
            std::fs::File::create(input.with_file_name("prepared.json"))?,
            &prep.worktree_info.is_some(),
        )?;
        if let Some(wt) = &prep.worktree_info {
            let plan = CleanupPlan::capture(wt)?;
            serde_json::to_writer(
                std::fs::File::create(input.with_file_name("cleanup.json"))?,
                &plan,
            )?;
        }
        let receipt = super::render_and_ops::epic_workers::spawn_provision_receipt(&prep);
        let warnings = prep.warnings.clone();
        let base_provenance = prep.base_provenance.clone();
        Ok::<_, anyhow::Error>(ProvisionedWorker {
            result: prep.run()?,
            warnings,
            base_provenance,
            receipt,
            retirement: None,
        })
    })()
    .map_err(|error| format!("{error:#}"));
    serde_json::to_writer(std::fs::File::create(output)?, &outcome)?;
    cas_store::shared_db::close_idle_connections();
    Ok(())
}

pub(crate) fn launch(
    context: WorkerSpawnContext,
    timeout: Duration,
) -> (
    tokio::task::JoinHandle<anyhow::Result<ProvisionedWorker>>,
    Arc<ProvisioningCancellation>,
) {
    let cancellation = Arc::new(ProvisioningCancellation::default());
    let stop = cancellation.clone();
    let task = tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        if stop.cancelled.load(Ordering::SeqCst) {
            anyhow::bail!("provision_cancelled: spawn generation retired before preparation");
        }
        let jobs = context.cas_dir.join("provisioning");
        std::fs::create_dir_all(&jobs)?;
        let jobs = jobs.canonicalize()?;
        // Hold this through completion/retirement: a reset may immediately queue
        // the same name, but its new generation must not race old cleanup.
        let lease = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(jobs.join(format!(
                "{}.lock",
                hex::encode(context.worker_name.as_bytes())
            )))?;
        loop {
            match fs2::FileExt::try_lock_exclusive(&lease) {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    anyhow::ensure!(
                        !stop.cancelled.load(Ordering::SeqCst),
                        "provision_cancelled: waiting for retired generation cleanup"
                    );
                    anyhow::ensure!(
                        started.elapsed() < timeout,
                        "provision_timeout: waiting for retired generation cleanup"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error.into()),
            }
        }
        let job = tempfile::tempdir_in(&jobs)?;
        let input = job.path().join("input.json");
        let output = job.path().join("output.json");
        let project_path = context.project_path.clone();
        let spawn_type = context.spawn_type.clone();
        let envelope = ProvisionerInput {
            build_hash: build_hash().into(),
            context: serde_json::to_value(context)?,
        };
        serde_json::to_writer(std::fs::File::create(&input)?, &envelope)?;
        let executable = provisioner_executable()?;
        let mut command = Command::new(executable);
        command
            .current_dir(project_path)
            .arg("--internal-factory-provisioner")
            .arg(input)
            .arg(&output)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(job.path().join("stderr.log"))?);
        let outcome = (|| {
            run_command(command, &stop, started, timeout)?;
            let outcome: Result<ProvisionedWorker, String> =
                serde_json::from_reader(std::fs::File::open(&output)?)
                    .context("provisioner did not return a valid spawn result")?;
            outcome.map_err(anyhow::Error::msg)
        })();
        // Resolution is a prepared event even if checkout later fails. Emit on
        // the daemon process: the hidden child does not initialize telemetry.
        if let Some(worktrees_enabled) = std::fs::File::open(job.path().join("prepared.json"))
            .ok()
            .and_then(|file| serde_json::from_reader::<_, bool>(file).ok())
        {
            crate::telemetry::track(
                "factory_worker_spawn_prepared",
                vec![
                    ("spawn_type", spawn_type.as_str()),
                    (
                        "worktrees_enabled",
                        if worktrees_enabled { "true" } else { "false" },
                    ),
                ],
            );
        }
        let plan: Option<CleanupPlan> = std::fs::File::open(job.path().join("cleanup.json"))
            .ok()
            .and_then(|file| serde_json::from_reader(file).ok())
            .flatten();
        match outcome {
            Ok(mut worker) => {
                worker.retirement = Some(RetirementGuard {
                    cancellation: stop,
                    plan,
                    lease: Some(lease),
                });
                Ok(worker)
            }
            Err(error) => {
                let cleanup = if let Some(plan) = plan {
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::Builder::new()
                        .name("factory-spawn-cleanup".into())
                        .spawn(move || {
                            let detail = plan.cleanup();
                            tracing::warn!(%detail, "failed provisioner cleanup");
                            drop(lease);
                            let _ = tx.send(detail);
                        })?;
                    rx.recv_timeout(Duration::from_millis(250))
                        .unwrap_or_else(|_| "best-effort spawn cleanup continuing off loop".into())
                } else {
                    "no new worktree to clean".into()
                };
                Err(anyhow::anyhow!(
                    "{error:#}{}; {cleanup}",
                    stderr_tail(&job.path().join("stderr.log"))
                ))
            }
        }
    });
    (task, cancellation)
}

/// A kernel-stuck child must not make Tokio wait forever for spawn_blocking
/// during daemon shutdown. Poll reaping for a bounded interval, then transfer
/// only the reap to an ordinary detached OS thread (never the Tokio pool).
fn wait_for_exit(
    timeout: Duration,
    mut poll: impl FnMut() -> std::io::Result<Option<ExitStatus>>,
) -> bool {
    let started = Instant::now();
    loop {
        match poll() {
            Ok(Some(_)) => return true,
            Err(_) => return false,
            Ok(None) if started.elapsed() >= timeout => return false,
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn reap_bounded(mut child: Child) {
    if !wait_for_exit(Duration::from_millis(250), || child.try_wait()) {
        let _ = std::thread::Builder::new()
            .name("factory-provisioner-reaper".into())
            .spawn(move || {
                let _ = child.wait();
            });
    }
}

/// Shared production/test seam: the command may stall anywhere in preparation.
pub(crate) fn run_command(
    mut command: Command,
    cancellation: &ProvisioningCancellation,
    started: Instant,
    timeout: Duration,
) -> anyhow::Result<()> {
    if cancellation.cancelled.load(Ordering::SeqCst) {
        anyhow::bail!("provision_cancelled: spawn generation retired before launch");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    {
        let mut pid = cancellation
            .child_pid
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *pid = Some(child.id());
        if cancellation.cancelled.load(Ordering::SeqCst) {
            kill_group(child.id());
        }
    }
    loop {
        let cancelled = cancellation.cancelled.load(Ordering::SeqCst);
        let timed_out = started.elapsed() >= timeout;
        let mut pid = cancellation
            .child_pid
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if cancelled || timed_out {
            kill_group(child.id());
            let _ = child.kill();
            *pid = None;
            drop(pid);
            // Reap outside the daemon loop. Even a kernel-stuck child cannot
            // hold up shutdowns, wake delivery or the reset consumer.
            reap_bounded(child);
            if cancelled {
                anyhow::bail!(
                    "provision_cancelled: spawn generation retired; provisioner group killed"
                );
            }
            anyhow::bail!(
                "provision_timeout: worktree preparation exceeded {} seconds; provisioner group killed",
                timeout.as_secs()
            );
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                *pid = None;
                // A successful result retains its cleanup guard. Let the daemon's
                // generation marker discard it, even when cancel races completion.
                anyhow::ensure!(status.success(), "provisioner exited with {status}");
                return Ok(());
            }
            Ok(None) => {}
            Err(error) => {
                kill_group(child.id());
                let _ = child.kill();
                *pid = None;
                drop(pid);
                reap_bounded(child);
                return Err(error.into());
            }
        }
        drop(pid);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
pub(crate) fn launch_stalled_for_test(
    command: Command,
    timeout: Duration,
) -> (
    tokio::task::JoinHandle<anyhow::Result<ProvisionedWorker>>,
    Arc<ProvisioningCancellation>,
) {
    let cancellation = Arc::new(ProvisioningCancellation::default());
    let stop = cancellation.clone();
    let task = tokio::task::spawn_blocking(move || {
        run_command(command, &stop, Instant::now(), timeout)?;
        anyhow::bail!("test provisioner exited without a result")
    });
    (task, cancellation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provisioner_build_mismatch_precedes_context_decode() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.json");
        let output = dir.path().join("output.json");
        std::fs::write(
            &input,
            r#"{"build_hash":"different-fixture-build","context":null}"#,
        )
        .unwrap();
        let error = run_internal_provisioner(&input, &output).unwrap_err();
        assert!(error.to_string().contains("provision_build_mismatch"));
        assert!(!output.exists());
        assert_eq!(
            strip_deleted_suffix(PathBuf::from("/bin/cas (deleted)")),
            PathBuf::from("/bin/cas")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn internal_provisioner_main_dispatch_round_trips_success() {
        let _env = crate::test_env_guard::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let cas_dir = crate::store::init_cas_dir(dir.path()).unwrap();
        std::fs::write(
            cas_dir.join("config.toml"),
            "[factory]\nspawn_min_free_gib = 0\n",
        )
        .unwrap();
        let context = WorkerSpawnContext {
            worker_name: "dispatch-fixture".into(),
            spawn_type: "named".into(),
            isolate: false,
            task_id: None,
            project_path: dir.path().to_path_buf(),
            cas_dir,
            worktree_repo_root: None,
            worktree_root: None,
            epic_branch: None,
            current_epic_id: None,
            factory_session: None,
        };
        // launch invokes the real cas binary, hidden main.rs dispatch and JSON envelope.
        let (handle, _) = launch(context, Duration::from_secs(10));
        let worker = handle.await.unwrap().unwrap();
        assert_eq!(worker.result.worker_name, "dispatch-fixture");
        assert_eq!(worker.result.cwd, dir.path());
        assert!(worker.result.worktree.is_none());
        assert!(!worker.receipt.is_empty());
    }

    #[test]
    fn stderr_tail_is_bounded_and_keeps_failure_reason() {
        let dir = tempfile::tempdir().unwrap();
        let stderr = dir.path().join("stderr.log");
        std::fs::write(
            &stderr,
            format!("{}\nfatal: fixture git checkout failed", "x".repeat(8192)),
        )
        .unwrap();
        let tail = stderr_tail(&stderr);
        assert!(tail.ends_with("fatal: fixture git checkout failed"));
        assert!(tail.len() < 4200);
    }

    #[cfg(unix)]
    #[test]
    fn cancelled_completed_and_partial_checkouts_remove_only_owned_git_artifacts() {
        let _env = crate::test_env_guard::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git_command = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        };
        git_command(&["init", "-b", "main"]);
        git_command(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ]);
        let git = crate::worktree::GitOperations::new(repo.clone());
        let survivor = dir.path().join("survivor");
        git.create_worktree(&survivor, "factory/survivor", Some("main"))
            .unwrap();
        let unrelated_lock = repo.join(".git/refs/heads/factory/survivor.lock");
        std::fs::write(&unrelated_lock, b"another writer").unwrap();
        let wt = WorktreePrep {
            worktree_path: dir.path().join("retired"),
            branch_name: "factory/retired".into(),
            parent_branch: "main".into(),
            base_ref: None,
            repo_root: repo.clone(),
            cas_dir: repo.join(".cas"),
        };
        let plan = CleanupPlan::capture(&wt).unwrap().unwrap();
        git.create_worktree(&wt.worktree_path, &wt.branch_name, Some("main"))
            .unwrap();
        assert!(
            CleanupPlan::capture(&wt).unwrap().is_none(),
            "reuse never owns cleanup"
        );
        let own_lock = repo.join(".git/refs/heads/factory/retired.lock");
        std::fs::write(&own_lock, b"killed writer").unwrap();
        std::fs::write(
            repo.join(".git/worktrees/retired/index.lock"),
            b"killed writer",
        )
        .unwrap();
        let stop = Arc::new(ProvisioningCancellation::default());
        let lease = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.path().join("generation.lock"))
            .unwrap();
        fs2::FileExt::lock_exclusive(&lease).unwrap();
        let guard = RetirementGuard {
            cancellation: stop.clone(),
            plan: Some(plan),
            lease: Some(lease),
        };
        // The child already succeeded; reset now drops its unconsumed result.
        stop.cancel();
        drop(guard);
        let started = Instant::now();
        while (wt.worktree_path.exists() || git.branch_exists(&wt.branch_name).unwrap())
            && started.elapsed() < Duration::from_secs(5)
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!wt.worktree_path.exists());
        assert!(!git.branch_exists(&wt.branch_name).unwrap());
        assert!(!own_lock.exists());
        assert!(!repo.join(".git/worktrees/retired").exists());
        assert!(survivor.join(".git").exists());
        assert!(unrelated_lock.exists());
        assert!(git.branch_exists("factory/survivor").unwrap());
        // Simulate SIGKILL after Git reserved an admin directory but before
        // its gitdir binding was written. Prior admin dirs must survive.
        let plan = CleanupPlan::capture(&wt).unwrap().unwrap();
        git_command(&["branch", "factory/retired"]);
        std::fs::create_dir(&wt.worktree_path).unwrap();
        std::fs::create_dir(repo.join(".git/worktrees/retired")).unwrap();
        std::fs::write(&own_lock, b"killed writer").unwrap();
        let report = plan.cleanup();
        assert!(!wt.worktree_path.exists(), "{report}");
        assert!(!own_lock.exists());
        assert!(!repo.join(".git/worktrees/retired").exists());
        assert!(!git.branch_exists(&wt.branch_name).unwrap());
        assert!(survivor.join(".git").exists());
        assert!(unrelated_lock.exists());
    }

    #[test]
    fn stalled_child_reap_has_a_bounded_wait() {
        let started = Instant::now();
        assert!(!wait_for_exit(Duration::from_millis(30), || Ok(None)));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn stalled_provisioner_times_out_without_blocking_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let entered = dir.path().join("entered");
        let mut command = Command::new("sh");
        command
            .args(["-c", r#"touch "$1"; sleep 30 & wait"#, "fixture"])
            .arg(&entered);
        let started = Instant::now();
        let (handle, _) = launch_stalled_for_test(command, Duration::from_secs(1));
        let mut ticks = 0;
        while !handle.is_finished() && started.elapsed() < Duration::from_secs(3) {
            ticks += 1;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            ticks > 1,
            "the loop must complete passes while the provisioner stalls"
        );
        assert!(entered.exists(), "the fixture must enter provisioning");
        let result = tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("provision_timeout")
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_before_launch_prevents_provisioner_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("should-not-exist");
        let stop = ProvisioningCancellation::default();
        stop.cancel();
        let mut command = Command::new("touch");
        command.arg(&path);
        let error =
            run_command(command, &stop, Instant::now(), Duration::from_secs(1)).unwrap_err();
        assert!(error.to_string().contains("provision_cancelled"));
        assert!(!path.exists());
    }
}
