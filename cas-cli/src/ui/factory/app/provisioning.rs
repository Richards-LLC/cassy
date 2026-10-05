//! Killable worker provisioning. Every store, Git and filesystem operation
//! runs in a separate process group, including base resolution before checkout.

use super::{WorkerSpawnContext, WorkerSpawnResult};
use anyhow::Context;
use std::path::Path;
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
    let context: WorkerSpawnContext = serde_json::from_reader(std::fs::File::open(input)?)?;
    let outcome = (|| {
        let prep = context.resolve()?;
        let receipt = super::render_and_ops::epic_workers::spawn_provision_receipt(&prep);
        let warnings = prep.warnings.clone();
        let base_provenance = prep.base_provenance.clone();
        Ok::<_, anyhow::Error>(ProvisionedWorker {
            result: prep.run()?,
            warnings,
            base_provenance,
            receipt,
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
        let job = tempfile::tempdir_in(jobs)?;
        let input = job.path().join("input.json");
        let output = job.path().join("output.json");
        serde_json::to_writer(std::fs::File::create(&input)?, &context)?;
        #[cfg(not(test))]
        let executable = std::env::current_exe()?;
        #[cfg(test)]
        let executable = crate::test_paths::cas_binary();
        let mut command = Command::new(executable);
        command
            .arg("--internal-factory-provisioner")
            .arg(input)
            .arg(&output)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(job.path().join("stderr.log"))?);
        run_command(command, &stop, started, timeout)?;
        let outcome: Result<ProvisionedWorker, String> =
            serde_json::from_reader(std::fs::File::open(&output)?)
                .context("provisioner did not return a valid spawn result")?;
        outcome.map_err(anyhow::Error::msg)
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
                if cancellation.cancelled.load(Ordering::SeqCst) {
                    anyhow::bail!(
                        "provision_cancelled: spawn generation retired; provisioner group killed"
                    );
                }
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
