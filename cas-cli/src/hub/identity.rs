use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::ensure_private_dir;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MachineIdentity {
    pub id: String,
}

#[derive(Debug, Clone)]
pub struct MachineIdentityStore {
    state_dir: PathBuf,
}

impl MachineIdentityStore {
    pub fn new(state_dir: impl AsRef<Path>) -> Self {
        Self {
            state_dir: state_dir.as_ref().to_path_buf(),
        }
    }

    pub fn load_or_create(&self) -> Result<MachineIdentity> {
        ensure_private_dir(&self.state_dir)?;
        let path = self.state_dir.join("machine-id");
        if path.exists() {
            ensure_private_file(&path)?;
            let id = fs::read_to_string(&path)
                .with_context(|| format!("read hub machine identity at {}", path.display()))?;
            let id = id.trim().to_owned();
            anyhow::ensure!(!id.is_empty(), "hub machine identity is empty");
            return Ok(MachineIdentity { id });
        }

        let id = uuid::Uuid::new_v4().to_string();
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                file.write_all(id.as_bytes())?;
                file.sync_all()?;
                Ok(MachineIdentity { id })
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                self.load_or_create()
            }
            Err(error) => Err(error)
                .with_context(|| format!("create hub machine identity at {}", path.display())),
        }
    }
}

fn ensure_private_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "hub machine identity is not a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            metadata.mode() & 0o777 == 0o600,
            "hub machine identity must have mode 0600"
        );
        anyhow::ensure!(
            metadata.uid() == unsafe { libc::geteuid() },
            "hub machine identity has the wrong owner"
        );
    }
    Ok(())
}
