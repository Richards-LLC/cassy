use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use super::ensure_private_dir;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HubProcessRecord {
    pub pid: u32,
    pub bind: String,
    pub port: u16,
    pub version: String,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailscale_serve_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailscale_cli: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_warning: Option<String>,
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

    pub fn acquire_instance_lock(&self) -> Result<HubInstanceLock> {
        self.try_acquire_instance_lock()?.ok_or_else(|| {
            anyhow::anyhow!("another cas hub instance already holds the machine lock")
        })
    }

    pub fn try_acquire_instance_lock(&self) -> Result<Option<HubInstanceLock>> {
        ensure_private_dir(&self.root)?;
        let path = self.root.join("hub.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(HubInstanceLock { file })),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error).context("acquire cas hub machine lock"),
        }
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
}

impl Drop for HubInstanceLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}
