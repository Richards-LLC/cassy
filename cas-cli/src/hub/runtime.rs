use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use super::identity::ensure_private_dir;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HubProcessRecord {
    pub pid: u32,
    pub bind: String,
    pub port: u16,
    pub version: String,
    pub started_at: String,
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

    pub fn acquire_instance_lock(&self) -> Result<HubInstanceLock> {
        ensure_private_dir(&self.root)?;
        let path = self.root.join("hub.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)?;
        file.try_lock_exclusive()
            .with_context(|| "another cas hub instance already holds the machine lock")?;
        Ok(HubInstanceLock { file })
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
