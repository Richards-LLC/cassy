use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use super::ensure_private_dir;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HubProcessRecord {
    pub pid: u32,
    /// Session and process-group identity prove that the hub does not share
    /// the short-lived shell or worker pane that launched it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pgid: Option<u32>,
    pub bind: String,
    pub port: u16,
    pub version: String,
    pub started_at: String,
    /// The cgroup scope that contains the hub when it was launched from a
    /// factory worker. Shared hub scopes are siblings of worker scopes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cgroup: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launched_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launched_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailscale_serve_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailscale_cli: Option<String>,
    /// The ephemeral loopback listener that Tailscale Serve proxies to.
    /// Older records omit this field and are intentionally treated as unable
    /// to prove that the current hub owns the Serve route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailscale_serve_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_warning: Option<String>,
}

/// The process that currently owns hub.lock.
///
/// This is deliberately separate from HubProcessRecord. Startup writes it
/// before doing any work that can block, so lifecycle commands can identify a
/// wedged hub serve even when the HTTP runtime record does not exist yet.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HubLockOwner {
    pub pid: u32,
    pub acquired_at: String,
    /// starting, running, or stopping. Missing means an older binary owns the
    /// lock and cannot prove which lifecycle phase it reached.
    #[serde(default)]
    pub phase: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubLockHolder {
    pub pid: u32,
    pub age: Option<Duration>,
    pub phase: Option<String>,
    pub command: Option<String>,
}

impl HubLockHolder {
    pub fn age_label(&self) -> String {
        self.age
            .map(format_duration)
            .unwrap_or_else(|| "unknown age".to_owned())
    }

    pub fn is_stopping(&self) -> bool {
        self.phase.as_deref() == Some("stopping")
    }
}

#[derive(Debug, Clone)]
pub struct HubRuntimePaths {
    root: PathBuf,
}

impl HubRuntimePaths {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn default_for_user() -> Result<Self> {
        let home = dirs::home_dir().context("cannot determine home directory")?;
        Ok(Self::new(home.join(".cas").join("hub")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn log_path(&self) -> PathBuf {
        self.root.join("hub.log")
    }

    pub fn events_path(&self) -> PathBuf {
        self.root.join("events.json")
    }

    pub fn lock_path(&self) -> PathBuf {
        self.root.join("hub.lock")
    }

    pub fn acquire_instance_lock(&self) -> Result<HubInstanceLock> {
        self.try_acquire_instance_lock()?.ok_or_else(|| {
            anyhow::anyhow!("another cas hub instance already holds the machine lock")
        })
    }

    pub fn try_acquire_instance_lock(&self) -> Result<Option<HubInstanceLock>> {
        ensure_private_dir(&self.root)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(self.lock_path())?;
        match file.try_lock_exclusive() {
            Ok(()) => {
                let mut lock = HubInstanceLock {
                    file,
                    acquired_at: Utc::now().to_rfc3339(),
                };
                if let Err(error) = lock.set_phase("starting") {
                    let _ = FileExt::unlock(&lock.file);
                    return Err(error).context("record cas hub machine-lock owner");
                }
                Ok(Some(lock))
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error).context("acquire cas hub machine lock"),
        }
    }

    pub fn read_lock_owner(&self) -> Option<HubLockOwner> {
        serde_json::from_slice(&fs::read(self.lock_path()).ok()?).ok()
    }

    /// Return processes that can be proven to hold the machine lock.
    ///
    /// The lock metadata covers current binaries and gives a useful phase. The
    /// OS scan is the fallback for a pre-fix macOS/Linux process whose lock
    /// file is empty, which is the exact recovery state reported by GH #804.
    pub fn lock_holders(&self) -> Vec<HubLockHolder> {
        let mut holders = os_lock_holders(&self.lock_path());
        if let Some(owner) = self.read_lock_owner() {
            if let Some(holder) = holders.iter_mut().find(|holder| holder.pid == owner.pid) {
                holder.age = owner_age(&owner.acquired_at).or(holder.age);
                holder.phase = (!owner.phase.is_empty())
                    .then_some(owner.phase.clone())
                    .or_else(|| holder.phase.clone());
            } else if holders.is_empty() && process_is_alive(owner.pid) {
                holders.push(HubLockHolder {
                    pid: owner.pid,
                    age: owner_age(&owner.acquired_at),
                    phase: (!owner.phase.is_empty()).then_some(owner.phase),
                    command: process_command(owner.pid),
                });
            }
        }
        holders.sort_by_key(|holder| holder.pid);
        holders.dedup_by_key(|holder| holder.pid);
        holders
    }

    pub fn wait_for_instance_lock(&self, timeout: Duration) -> Result<HubInstanceLock> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(lock) = self.try_acquire_instance_lock()? {
                return Ok(lock);
            }
            if Instant::now() >= deadline {
                anyhow::bail!(
                    "cas hub machine lock remained held after {:.1}s; the old instance may still be shutting down and no replacement was started",
                    timeout.as_secs_f64()
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn write_process_record(&self, record: &HubProcessRecord) -> Result<()> {
        ensure_private_dir(&self.root)?;
        let target = self.root.join("process.json");
        let temporary = self
            .root
            .join(format!(".process.{}.tmp", std::process::id()));
        let bytes = serde_json::to_vec_pretty(record)?;
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(temporary, target)?;
        Ok(())
    }

    pub fn read_process_record(&self) -> Result<HubProcessRecord> {
        let path = self.root.join("process.json");
        serde_json::from_slice(
            &fs::read(&path)
                .with_context(|| format!("no cas hub runtime record at {}", path.display()))?,
        )
        .context("invalid cas hub runtime record")
    }

    pub fn remove_process_record(&self) -> Result<()> {
        let path = self.root.join("process.json");
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

pub struct HubInstanceLock {
    file: File,
    acquired_at: String,
}

impl HubInstanceLock {
    pub fn set_phase(&mut self, phase: &str) -> Result<()> {
        let owner = HubLockOwner {
            pid: std::process::id(),
            acquired_at: self.acquired_at.clone(),
            phase: phase.to_owned(),
        };
        self.file.seek(SeekFrom::Start(0))?;
        self.file.set_len(0)?;
        self.file.write_all(&serde_json::to_vec(&owner)?)?;
        self.file.sync_all()?;
        Ok(())
    }
}

impl Drop for HubInstanceLock {
    fn drop(&mut self) {
        let _ = self.file.set_len(0);
        let _ = FileExt::unlock(&self.file);
    }
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return if seconds % 60 == 0 {
            format!("{minutes}m")
        } else {
            format!("{minutes}m {}s", seconds % 60)
        };
    }
    let hours = minutes / 60;
    if hours < 24 {
        return if minutes % 60 == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {}m", minutes % 60)
        };
    }
    if hours % 24 == 0 {
        format!("{}d", hours / 24)
    } else {
        format!("{}d {}h", hours / 24, hours % 24)
    }
}

fn owner_age(acquired_at: &str) -> Option<Duration> {
    let acquired_at = acquired_at.parse::<DateTime<Utc>>().ok()?;
    let seconds = (Utc::now() - acquired_at).num_seconds();
    (seconds >= 0).then_some(Duration::from_secs(seconds as u64))
}

fn process_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only performs the kernel existence/permission
        // check and does not deliver a signal.
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        pid == std::process::id()
    }
}

fn process_command(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let bytes = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let command = bytes
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        (!command.is_empty()).then_some(command)
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("ps")
            .args(["-o", "command=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let command = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        (!command.is_empty()).then_some(command)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

fn process_age(pid: u32) -> Option<Duration> {
    #[cfg(target_os = "linux")]
    {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after_comm = stat.rsplit_once(") ")?.1;
        let fields = after_comm.split_whitespace().collect::<Vec<_>>();
        let start_ticks = fields.get(19)?.parse::<u64>().ok()?;
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if hz <= 0 {
            return None;
        }
        let boot = fs::read_to_string("/proc/stat")
            .ok()?
            .lines()
            .find_map(|line| line.strip_prefix("btime "))?
            .trim()
            .parse::<u64>()
            .ok()?;
        let started = std::time::UNIX_EPOCH
            + Duration::from_secs(boot)
            + Duration::from_secs_f64(start_ticks as f64 / hz as f64);
        return std::time::SystemTime::now().duration_since(started).ok();
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("ps")
            .args(["-o", "etime=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        parse_elapsed_time(String::from_utf8_lossy(&output.stdout).trim())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

#[cfg(target_os = "linux")]
fn os_lock_holders(path: &Path) -> Vec<HubLockHolder> {
    let mut holders = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return holders;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse().ok()) else {
            continue;
        };
        let Ok(fds) = fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        if fds
            .flatten()
            .any(|fd| fs::read_link(fd.path()).is_ok_and(|target| target == path))
        {
            holders.push(HubLockHolder {
                pid,
                age: process_age(pid),
                phase: None,
                command: process_command(pid),
            });
        }
    }
    holders
}

#[cfg(target_os = "macos")]
fn os_lock_holders(path: &Path) -> Vec<HubLockHolder> {
    let mut holders = Vec::new();
    let path = path.to_string_lossy();
    for executable in ["/usr/sbin/lsof", "/usr/bin/lsof", "lsof"] {
        let Ok(output) = Command::new(executable)
            .args(["-t", "-n", "-P", path.as_ref()])
            .output()
        else {
            continue;
        };
        for pid in String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.trim().parse::<u32>().ok())
        {
            holders.push(HubLockHolder {
                pid,
                age: process_age(pid),
                phase: None,
                command: process_command(pid),
            });
        }
        if !holders.is_empty() {
            break;
        }
    }
    holders
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn os_lock_holders(_path: &Path) -> Vec<HubLockHolder> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn parse_elapsed_time(value: &str) -> Option<Duration> {
    let fields = value.split(':').collect::<Vec<_>>();
    let (days, hours, minutes, seconds) = match fields.as_slice() {
        [minutes, seconds] => (0, 0, minutes.parse().ok()?, seconds.parse().ok()?),
        [hours, minutes, seconds] => {
            (0, hours.parse().ok()?, minutes.parse().ok()?, seconds.parse().ok()?)
        }
        [days, hours, minutes, seconds] => (
            days.parse().ok()?,
            hours.parse().ok()?,
            minutes.parse().ok()?,
            seconds.parse().ok()?,
        ),
        _ => return None,
    };
    Some(Duration::from_secs(
        days * 86_400 + hours * 3_600 + minutes * 60 + seconds,
    ))
}
