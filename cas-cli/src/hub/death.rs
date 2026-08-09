use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{DaemonDeathCause, DaemonDeathDiagnostic, ProcessExit, diagnose_daemon_death};

/// Exact identity of the daemon behind one factory session connection.
///
/// A pid alone is never authoritative: after reuse it may name an unrelated
/// process. Linux supplies the process-start fingerprint; other platforms
/// degrade to no identity and therefore an unknown diagnosis.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonIdentity {
    pub session: String,
    pub pid: u32,
    pub pid_starttime: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonExitReceipt {
    pub identity: DaemonIdentity,
    pub exit: ProcessExit,
    pub core_dumped: Option<bool>,
    pub observed_at: String,
}

/// Durable exit evidence written by the process that actually reaped the
/// daemon. Commander only consumes a receipt whose complete identity matches.
#[derive(Debug, Clone)]
pub struct DaemonExitEvidenceStore {
    root: PathBuf,
}

impl DaemonExitEvidenceStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn default_for_user() -> Option<Self> {
        dirs::home_dir().map(|home| Self::new(home.join(".cas").join("daemon-exits")))
    }

    fn receipt_path(&self, session: &str) -> PathBuf {
        let encoded = session
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        self.root.join(format!("{encoded}.json"))
    }

    pub fn write(&self, receipt: &DaemonExitReceipt) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("create daemon exit store {}", self.root.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))?;
        }
        let target = self.receipt_path(&receipt.identity.session);
        let temporary = self.root.join(format!(
            ".receipt.{}.{}.tmp",
            std::process::id(),
            receipt.identity.pid
        ));
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(receipt)?)?;
        file.sync_all()?;
        fs::rename(temporary, target)?;
        Ok(())
    }

    pub fn read_matching(&self, identity: &DaemonIdentity) -> Option<DaemonExitReceipt> {
        let bytes = fs::read(self.receipt_path(&identity.session)).ok()?;
        let receipt: DaemonExitReceipt = serde_json::from_slice(&bytes).ok()?;
        (receipt.identity == *identity).then_some(receipt)
    }
}

pub(crate) async fn diagnose_disconnect(
    identity: Option<&DaemonIdentity>,
    evidence: Option<&DaemonExitEvidenceStore>,
) -> DaemonDeathDiagnostic {
    let (Some(identity), Some(evidence)) = (identity, evidence) else {
        return diagnose_daemon_death(None, None);
    };

    // The socket usually observes process death just before the reaping parent
    // has persisted wait(2)'s result. Bound the race; never invent a status if
    // the authoritative receipt does not arrive.
    for _ in 0..20 {
        if let Some(receipt) = evidence.read_matching(identity) {
            return diagnose_daemon_death(Some(receipt.exit), receipt.core_dumped);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    if crate::mcp::daemon::pid_matches_fingerprint(identity.pid, identity.pid_starttime) {
        DaemonDeathDiagnostic {
            cause: DaemonDeathCause::TransportLost,
            next_action: "The daemon process is still alive with the expected process-start \
                fingerprint; inspect the local transport and daemon log before reconnecting."
                .into(),
        }
    } else {
        diagnose_daemon_death(None, None)
    }
}

fn identity_for(session: impl Into<String>, pid: u32) -> Option<DaemonIdentity> {
    crate::mcp::daemon::read_pid_starttime(pid).map(|pid_starttime| DaemonIdentity {
        session: session.into(),
        pid,
        pid_starttime,
    })
}

/// Reap a spawned daemon and persist the kernel-provided exit status.
///
/// The returned identity is the value session metadata and Commander must use.
#[cfg(unix)]
pub(crate) fn supervise_spawned_daemon(
    session: impl Into<String>,
    mut child: Child,
    store: DaemonExitEvidenceStore,
) -> Option<(DaemonIdentity, std::thread::JoinHandle<()>)> {
    use std::os::unix::process::ExitStatusExt;

    let identity = identity_for(session, child.id())?;
    let watched = identity.clone();
    let handle = std::thread::spawn(move || {
        let Ok(status) = child.wait() else {
            return;
        };
        let exit = match status.signal() {
            Some(signal) => ProcessExit::Signal(signal),
            None => ProcessExit::Code(status.code().unwrap_or(-1)),
        };
        let receipt = DaemonExitReceipt {
            identity: watched,
            exit,
            core_dumped: status.signal().map(|_| status.core_dumped()),
            observed_at: chrono::Utc::now().to_rfc3339(),
        };
        if let Err(error) = store.write(&receipt) {
            tracing::warn!(%error, "could not persist factory daemon exit receipt");
        }
    });
    Some((identity, handle))
}

/// Reap the child created by the fork-first factory path.
#[cfg(unix)]
pub(crate) struct ForkedDaemonReaper {
    identity: DaemonIdentity,
    completed: std::sync::mpsc::Receiver<()>,
    handle: Option<std::thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl Drop for ForkedDaemonReaper {
    fn drop(&mut self) {
        if process_is_live_non_zombie(&self.identity) {
            return;
        }
        if self
            .completed
            .recv_timeout(Duration::from_millis(750))
            .is_ok()
        {
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn process_is_live_non_zombie(identity: &DaemonIdentity) -> bool {
    if !crate::mcp::daemon::pid_matches_fingerprint(
        identity.pid,
        identity.pid_starttime,
    ) {
        return false;
    }
    fs::read_to_string(format!("/proc/{}/stat", identity.pid))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next().map(str::to_owned))
        })
        .is_some_and(|state| state != "Z")
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_is_live_non_zombie(_identity: &DaemonIdentity) -> bool {
    false
}

#[cfg(unix)]
pub(crate) fn supervise_forked_daemon(
    session: impl Into<String>,
    pid: u32,
) -> Option<ForkedDaemonReaper> {
    use nix::sys::wait::{WaitStatus, waitpid};
    use nix::unistd::Pid;

    let Some(identity) = identity_for(session, pid) else {
        return None;
    };
    let Some(store) = DaemonExitEvidenceStore::default_for_user() else {
        return None;
    };
    let watched = identity.clone();
    let (completed_tx, completed) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let Ok(status) = waitpid(Pid::from_raw(pid as i32), None) else {
            return;
        };
        let (exit, core_dumped) = match status {
            WaitStatus::Exited(_, code) => (ProcessExit::Code(code), None),
            WaitStatus::Signaled(_, signal, core_dumped) => {
                (ProcessExit::Signal(signal as i32), Some(core_dumped))
            }
            _ => return,
        };
        let receipt = DaemonExitReceipt {
            identity: watched,
            exit,
            core_dumped,
            observed_at: chrono::Utc::now().to_rfc3339(),
        };
        if let Err(error) = store.write(&receipt) {
            tracing::warn!(%error, "could not persist forked factory daemon exit receipt");
        }
        let _ = completed_tx.send(());
    });
    Some(ForkedDaemonReaper {
        identity,
        completed,
        handle: Some(handle),
    })
}
