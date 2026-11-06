//! Same-PID daemon replacement. Inherited originals keep custody until every
//! pane has been staged; errors before that point exec the pinned old image.
use super::*;
use crate::ui::factory::app::handoff::AppHandoff;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const HANDOFF_SCHEMA: u32 = 1;
const HANDOFF_ENV: &str = "CAS_DAEMON_HANDOFF_FD";
const FALLBACK_ENV: &str = "CAS_DAEMON_HANDOFF_FALLBACK";
const ROLLBACK_ENV: &str = "CAS_DAEMON_ROLLBACK_BINARY";

#[derive(Serialize, Deserialize)]
pub(super) struct Handoff {
    schema: u32,
    pub(super) session: String,
    old_version: String,
    nonce: String,
    rollback: PathBuf,
    pub(super) listener: i32,
    pub(super) gui_listener: i32,
    pub(super) ws_listener: Option<i32>,
    pub(super) teams_present: bool,
    reply: Option<i32>,
    pub(super) app: AppHandoff,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReexecReceipt {
    pub session: String,
    pub nonce: String,
    pub pid: u32,
    pub old_version: String,
    pub version: String,
    pub build: String,
    pub panes_adopted: usize,
    pub fallback: bool,
    pub error: Option<String>,
    pub at: chrono::DateTime<chrono::Utc>,
}

fn running_version() -> String {
    #[cfg(test)]
    if let Ok(version) = std::env::var("CAS_REEXEC_TEST_VERSION") {
        return version;
    }
    env!("CARGO_PKG_VERSION").to_owned()
}

pub fn handoff_check(schema: u32) -> Result<serde_json::Value> {
    ensure!(
        schema == HANDOFF_SCHEMA,
        "Unsupported daemon handoff schema {schema}; supported: {HANDOFF_SCHEMA}"
    );
    Ok(serde_json::json!({ "supported_schemas": [HANDOFF_SCHEMA] }))
}

async fn preflight(binary: &Path) -> Result<()> {
    let mut command = tokio::process::Command::new(binary);
    command
        .args([
            "factory",
            "handoff-check",
            "--schema",
            &HANDOFF_SCHEMA.to_string(),
        ])
        .env_remove(HANDOFF_ENV)
        .env_remove(FALLBACK_ENV)
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(5), command.output())
        .await
        .context("Daemon reexec preflight timed out; session untouched")?
        .context("Daemon reexec preflight could not run; session untouched")?;
    ensure!(
        output.status.success(),
        "Daemon reexec preflight refused ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("Daemon reexec preflight returned an invalid schema response")?;
    ensure!(
        reply["supported_schemas"]
            .as_array()
            .is_some_and(|schemas| schemas
                .iter()
                .any(|schema| schema.as_u64() == Some(HANDOFF_SCHEMA as u64))),
        "Daemon reexec preflight does not accept handoff schema {HANDOFF_SCHEMA}"
    );
    Ok(())
}

impl FactoryDaemon {
    pub(crate) async fn reexec(
        &mut self,
        binary: &Path,
        _version: &str,
        reply: Option<RawFd>,
        nonce: &str,
    ) -> Result<()> {
        // This must be first: an unsupported candidate cannot change input,
        // readers, queues, files, socket flags, metadata or the running image.
        preflight(binary).await?;
        ensure!(
            self.spawn_task.is_none()
                && self.pending_spawns.is_empty()
                && self.app.mux.handoff_ready(),
            "Worker provisioning or pane injection is still active; retry reexec"
        );
        self.queue_jobs
            .flush_for_reexec(self.app.cas_dir(), Duration::from_secs(5))
            .await?;
        ensure!(
            runtime::store_worker::global().wait_idle(Duration::from_secs(5)),
            "Durable stores did not flush before reexec"
        );
        let rollback = pin_rollback(&self.session_name)?;
        let prepared = self.app.prepare_handoff().await;
        let app = match prepared {
            Ok(app) => app,
            Err(error) => {
                self.resume_after_failed_reexec().await;
                return Err(error);
            }
        };
        let manifest = Handoff {
            schema: HANDOFF_SCHEMA,
            session: self.session_name.clone(),
            old_version: running_version(),
            nonce: nonce.to_owned(),
            rollback: rollback.to_path_buf(),
            listener: self.listener.as_raw_fd(),
            gui_listener: self.gui_listener.as_raw_fd(),
            ws_listener: self.ws_listener.as_ref().map(AsRawFd::as_raw_fd),
            teams_present: self.teams.is_some(),
            reply,
            app,
        };
        let result = (|| {
            let file = manifest_file(&self.session_name, &manifest)?;
            let keep = manifest.fds(file.as_raw_fd());
            let flags = inherit_only(&keep)?;
            let error = exec_image(binary, file.as_raw_fd(), false, &manifest.rollback);
            restore_flags(&flags);
            Err(anyhow::anyhow!(error).context("Daemon exec failed; resumed current image"))
        })();
        self.resume_after_failed_reexec().await;
        result
    }

    async fn resume_after_failed_reexec(&mut self) {
        for pane in self.app.mux.panes_mut() {
            if let Err(error) = pane.resume_recording_after_handoff().await {
                tracing::error!(%error, "Could not resume pane recording after failed reexec");
            }
            if let Err(error) = pane.resume_after_handoff() {
                tracing::error!(%error, "Could not resume pane reader after failed reexec");
            }
        }
    }
}

impl Handoff {
    fn fds(&self, manifest: RawFd) -> Vec<RawFd> {
        let mut fds = vec![manifest, self.listener, self.gui_listener];
        fds.extend(self.ws_listener);
        fds.extend(self.reply);
        fds.extend(
            self.app
                .mux
                .panes
                .iter()
                .filter_map(|pane| pane.pty.as_ref().map(|pty| pty.master_fd)),
        );
        fds.sort_unstable();
        fds.dedup();
        fds
    }
}

fn pin_rollback(session: &str) -> Result<tempfile::TempPath> {
    let root = dirs::home_dir()
        .context("No home for daemon rollback pin")?
        .join(".local/lib/cassy/hub/versions");
    std::fs::create_dir_all(&root)?;
    let path = tempfile::NamedTempFile::new_in(&root)?.into_temp_path();
    #[cfg(target_os = "linux")]
    let source = PathBuf::from("/proc/self/exe");
    #[cfg(not(target_os = "linux"))]
    let source = crate::ui::factory::provisioner_pin_path(session);
    #[cfg(test)]
    let fixture = std::env::var_os("CAS_REEXEC_TEST_ROOT").is_some();
    #[cfg(not(test))]
    let fixture = false;
    if fixture {
        // The fixture's stable test image can be linked. Copying a gigabyte
        // debug image per exec would exhaust a RAM-backed test HOME.
        std::fs::remove_file(&path)?;
        std::os::unix::fs::symlink(std::env::current_exe()?, &path)
            .context("Could not link isolated rollback test image")?;
    } else {
        std::fs::copy(source, &path).context("Could not preserve old daemon for rollback")?;
    }
    if !fixture {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    }
    File::open(&path)?.sync_all()?;
    Ok(path)
}

fn manifest_file(session: &str, manifest: &Handoff) -> Result<File> {
    #[cfg(target_os = "linux")]
    let mut file = {
        let fd = unsafe {
            libc::memfd_create(
                c"cas-daemon-handoff".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        ensure!(
            fd >= 0,
            "Could not create handoff memfd: {}",
            std::io::Error::last_os_error()
        );
        unsafe { File::from_raw_fd(fd) }
    };
    #[cfg(not(target_os = "linux"))]
    let mut file = {
        let root = crate::ui::factory::session::session_log_dir(session);
        std::fs::create_dir_all(&root)?;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
        tempfile::tempfile_in(root)? // unlinked immediately; only our fd survives.
    };
    #[cfg(target_os = "linux")]
    let _ = session;
    serde_json::to_writer(&mut file, manifest)?;
    file.flush()?;
    file.sync_all()?;
    file.rewind()?;
    #[cfg(target_os = "linux")]
    ensure!(
        unsafe {
            libc::fcntl(
                file.as_raw_fd(),
                libc::F_ADD_SEALS,
                libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL,
            )
        } == 0,
        "Could not seal daemon handoff: {}",
        std::io::Error::last_os_error()
    );
    Ok(file)
}

fn open_fds() -> Result<Vec<RawFd>> {
    #[cfg(target_os = "linux")]
    {
        Ok(std::fs::read_dir("/proc/self/fd")?
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<i32>().ok())
            .filter(|fd| *fd > 2)
            .collect())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let max = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
        ensure!(max > 0, "Could not enumerate descriptors before reexec");
        Ok((3..max as i32)
            .filter(|fd| unsafe { libc::fcntl(*fd, libc::F_GETFD) } >= 0)
            .collect())
    }
}

fn inherit_only(keep: &[RawFd]) -> Result<Vec<(RawFd, i32)>> {
    let mut originals = Vec::new();
    for fd in open_fds()? {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 {
            continue;
        } // the enumeration's own fd has closed.
        let next = if keep.contains(&fd) {
            flags & !libc::FD_CLOEXEC
        } else {
            flags | libc::FD_CLOEXEC
        };
        if next != flags {
            originals.push((fd, flags));
            if unsafe { libc::fcntl(fd, libc::F_SETFD, next) } < 0 {
                restore_flags(&originals);
                bail!(
                    "Could not prepare inherited descriptor {fd}: {}",
                    std::io::Error::last_os_error()
                );
            }
        }
    }
    Ok(originals)
}
fn restore_flags(originals: &[(RawFd, i32)]) {
    for (fd, flags) in originals {
        unsafe {
            libc::fcntl(*fd, libc::F_SETFD, *flags);
        }
    }
}
fn exec_image(binary: &Path, manifest: RawFd, fallback: bool, rollback: &Path) -> std::io::Error {
    std::process::Command::new(binary)
        .arg0(std::env::args_os().next().unwrap_or_default())
        .args(std::env::args_os().skip(1))
        .env(HANDOFF_ENV, manifest.to_string())
        .env(FALLBACK_ENV, if fallback { "1" } else { "0" })
        .env(ROLLBACK_ENV, rollback)
        .exec()
}

pub(super) fn duplicate(fd: RawFd) -> Result<RawFd> {
    let clone = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    ensure!(
        clone >= 0,
        "Inherited descriptor {fd} is unavailable: {}",
        std::io::Error::last_os_error()
    );
    Ok(clone)
}

/// Called before ordinary CLI startup so the original argv never forks again.
pub fn resume_inherited_daemon() -> Option<std::process::ExitCode> {
    let fd = std::env::var(HANDOFF_ENV).ok()?;
    let fallback = std::env::var(FALLBACK_ENV).ok().as_deref() == Some("1");
    let rollback = std::env::var_os(ROLLBACK_ENV).map(PathBuf::from);
    // SAFETY: main calls this before creating threads. Children must not
    // inherit stale handoff markers and enter the daemon adoption path.
    unsafe {
        std::env::remove_var(HANDOFF_ENV);
        std::env::remove_var(FALLBACK_ENV);
        std::env::remove_var(ROLLBACK_ENV);
    }
    let adopted = std::cell::Cell::new(false);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        resume(fd.clone(), fallback, &adopted)
    }));
    let result = match result {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!("panic before daemon adoption completed")),
    };
    if result.is_err() && !adopted.get() && !fallback {
        if let (Some(rollback), Ok(fd)) = (rollback, fd.parse::<RawFd>()) {
            // Even schema decoding and runtime construction failures reach this
            // one-shot fallback. The original inherited fds still hold custody.
            eprintln!(
                "Daemon reexec refused adoption: {}; restoring previous image",
                result.as_ref().unwrap_err()
            );
            let error = exec_image(&rollback, fd, true, &rollback);
            eprintln!("Daemon rollback exec failed: {error}");
        }
    }
    Some(match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Daemon reexec failed: {error:#}");
            std::process::ExitCode::FAILURE
        }
    })
}

fn resume(fd: String, fallback: bool, adopted: &std::cell::Cell<bool>) -> Result<()> {
    let fd = fd
        .parse::<RawFd>()
        .context("Invalid daemon handoff descriptor")?;
    ensure!(fd > 2, "Invalid daemon handoff descriptor");
    let mut file = unsafe { File::from_raw_fd(duplicate(fd)?) };
    file.rewind()?;
    let mut data = Vec::new();
    file.take(256 * 1024 * 1024).read_to_end(&mut data)?;
    let manifest: Handoff =
        serde_json::from_slice(&data).context("Invalid daemon handoff manifest")?;
    let staged = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<_> {
        ensure!(
            manifest.schema == HANDOFF_SCHEMA,
            "Unsupported inherited daemon schema {}",
            manifest.schema
        );
        #[cfg(test)]
        if !fallback && std::env::var_os("CAS_REEXEC_TEST_PANIC").is_some() {
            panic!("injected adopt failure");
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let daemon = runtime.block_on(async {
            let app = FactoryApp::from_handoff(&manifest.app)?;
            let listener = unsafe { UnixListener::from_raw_fd(duplicate(manifest.listener)?) };
            let daemon = process::from_app_after_fork(
                app,
                listener,
                manifest.session.clone(),
                Some(&manifest),
            )?;
            crate::ui::factory::pin_daemon_provisioner(&manifest.session)?;
            Result::<_>::Ok(daemon)
        })?;
        Ok((runtime, daemon))
    }));
    let (runtime, mut daemon) = match staged {
        Ok(Ok(adopted)) => adopted,
        failure => {
            let reason = match failure {
                Ok(Err(error)) => format!("{error:#}"),
                _ => "panic before pane adoption completed".to_owned(),
            };
            crate::ui::factory::record_daemon_lifecycle(
                &manifest.session,
                "daemon_reexec_fallback",
                &reason,
            );
            bail!("Daemon adoption failed: {reason}");
        }
    };
    // All terminals and handles now exist. Commit custody before readers start;
    // no fallback can replay consumed PTY output after this point.
    runtime.block_on(async {
        for pane in daemon.app.mux.panes_mut() {
            pane.resume_recording_after_handoff().await?;
        }
        for pane in daemon.app.mux.panes_mut() {
            if let Some(state) = manifest
                .app
                .mux
                .panes
                .iter()
                .find(|state| state.id == pane.id())
            {
                pane.commit_startup_brief(state);
            }
        }
        adopted.set(true);
        daemon.app.mux.resume_after_handoff()?;
        Result::<()>::Ok(())
    })?;
    let _ = std::fs::remove_file(&manifest.rollback);
    let receipt = ReexecReceipt {
        session: manifest.session.clone(),
        nonce: manifest.nonce.clone(),
        pid: std::process::id(),
        old_version: manifest.old_version.clone(),
        version: if fallback {
            manifest.old_version.clone()
        } else {
            running_version()
        },
        build: option_env!("CAS_GIT_HASH").unwrap_or("unknown").into(),
        panes_adopted: manifest.app.mux.panes.len(),
        fallback,
        error: fallback.then(|| "Candidate failed before adoption; restored previous image".into()),
        at: chrono::Utc::now(),
    };
    crate::ui::factory::record_daemon_lifecycle(
        &manifest.session,
        "daemon_reexec",
        &serde_json::to_string(&receipt)?,
    );
    if let Some(reply) = manifest.reply {
        let send = (|| -> Result<()> {
            let mut stream = unsafe { UnixStream::from_raw_fd(duplicate(reply)?) };
            stream.set_nonblocking(false)?;
            stream.set_write_timeout(Some(Duration::from_secs(5)))?;
            stream.write_all(&crate::ui::factory::protocol::encode_message(
                &crate::ui::factory::protocol::DaemonMessage::ReexecResult {
                    receipt: receipt.clone(),
                },
            )?)?;
            Ok(())
        })();
        if let Err(error) = send {
            tracing::warn!(%error, "Reexec succeeded but its requester disconnected");
        }
    }
    for inherited in manifest.fds(fd) {
        unsafe {
            libc::close(inherited);
        }
    }
    #[cfg(test)]
    if std::env::var_os("CAS_REEXEC_TEST_ROOT").is_some() {
        return runtime.block_on(runtime::reexec_tests::finish_exec_probe(
            &mut daemon,
            &receipt,
        ));
    }
    runtime.block_on(daemon.run())
}
