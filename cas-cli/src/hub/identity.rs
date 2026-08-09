use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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

pub(crate) fn ensure_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
