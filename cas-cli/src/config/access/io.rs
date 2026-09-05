use crate::config::*;
use fs2::FileExt;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Salt same-process temp paths; the PID separates independent Cassy
/// processes. The lock file itself is stable across atomic replacements.
static PROJECT_CONFIG_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(crate) struct ProjectConfigWriteLock {
    file: fs::File,
}

impl Drop for ProjectConfigWriteLock {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            tracing::error!(%error, "failed to release project config write lock");
        }
    }
}

/// Lock the project config's stable sidecar inode. Locking config.toml itself
/// would not serialize writers after an atomic rename replaces its inode.
pub(crate) fn lock_project_config(cas_dir: &Path) -> std::io::Result<ProjectConfigWriteLock> {
    fs::create_dir_all(cas_dir)?;
    let lock_path = cas_dir.join(".config.toml.cas-write.lock");
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)?;
    file.lock_exclusive()?;
    Ok(ProjectConfigWriteLock { file })
}

/// Write a complete TOML document through a same-directory temp file, fsync,
/// and atomic rename. The destination is never opened for in-place writes.
pub(crate) fn write_project_config_toml(cas_dir: &Path, contents: &str) -> std::io::Result<()> {
    let _lock = lock_project_config(cas_dir)?;
    let path = cas_dir.join("config.toml");
    atomic_replace_project_config(&path, contents)
}

pub(crate) fn atomic_replace_project_config(path: &Path, contents: &str) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("cannot atomically write project config without a parent: {path:?}"),
        )
    })?;
    let sequence = PROJECT_CONFIG_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp_path = parent.join(format!(
        ".config.toml.cas-write.{}.{sequence}.tmp",
        std::process::id()
    ));
    atomic_replace_project_config_via(path, contents, &temp_path, |from, to| fs::rename(from, to))
}

/// Testable implementation of the complete-document replacement contract.
pub(crate) fn atomic_replace_project_config_via<F>(
    path: &Path,
    contents: &str,
    temp_path: &Path,
    commit: F,
) -> std::io::Result<()>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    atomic_replace_project_config_via_with_created_hook(
        path,
        contents,
        temp_path,
        |_| Ok(()),
        commit,
    )
}

pub(crate) fn atomic_replace_project_config_via_with_created_hook<F, H>(
    path: &Path,
    contents: &str,
    temp_path: &Path,
    after_create: H,
    commit: F,
) -> std::io::Result<()>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
    H: FnOnce(&fs::File) -> std::io::Result<()>,
{
    let permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut temp = options.open(temp_path)?;

    let result = (|| -> std::io::Result<()> {
        after_create(&temp)?;
        temp.write_all(contents.as_bytes())?;
        temp.flush()?;
        if let Some(permissions) = permissions {
            temp.set_permissions(permissions)?;
        }
        temp.sync_all()?;
        drop(temp);
        commit(temp_path, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp_path);
    }
    result
}

impl Config {
    /// Load configuration from .cas directory
    ///
    /// Tries TOML first (config.toml), falls back to YAML (config.yaml),
    /// and auto-migrates YAML to TOML on first load.
    ///
    /// When both files exist, merges any YAML-only settings into the TOML
    /// config (covers the case where something wrote to config.yaml while
    /// config.toml already existed).
    pub fn load(cas_dir: &std::path::Path) -> Result<Self, MemError> {
        let toml_path = cas_dir.join("config.toml");
        let yaml_path = cas_dir.join("config.yaml");

        // Try TOML first (preferred format)
        if toml_path.exists() {
            let content = std::fs::read_to_string(&toml_path)?;
            let mut config: Self = toml::from_str(&content).map_err(|e| {
                let line = e
                    .span()
                    .map(|span| content[..span.start].bytes().filter(|b| *b == b'\n').count() + 1)
                    .unwrap_or(1);
                MemError::Parse(format!(
                    "Failed to parse config.toml at line {line}: {e}. Restore a known-good config.toml backup, then rerun `cas doctor`."
                ))
            })?;

            // If YAML also exists, merge any settings that are missing from TOML.
            // This handles the case where something wrote to config.yaml after
            // config.toml was already created (e.g. theme variant).
            if yaml_path.exists() {
                if let Ok(yaml_content) = std::fs::read_to_string(&yaml_path) {
                    if let Ok(yaml_config) = serde_yaml::from_str::<Self>(&yaml_content) {
                        let changed = config.merge_missing(&yaml_config);
                        if changed {
                            // Persist the merged config and clean up stale YAML
                            let _ = config.save_toml(cas_dir);
                        }
                        // Always remove the stale YAML to prevent future confusion
                        let backup_path = cas_dir.join("config.yaml.bak");
                        let _ = std::fs::rename(&yaml_path, &backup_path);
                    }
                }
            }

            return Ok(config);
        }

        // Fall back to YAML and auto-migrate
        if yaml_path.exists() {
            let content = std::fs::read_to_string(&yaml_path)?;
            let config: Self = serde_yaml::from_str(&content)?;

            // Auto-migrate to TOML
            if let Err(e) = config.save_toml(cas_dir) {
                eprintln!("Warning: Failed to migrate config to TOML: {e}");
            } else {
                // Rename old YAML to backup
                let backup_path = cas_dir.join("config.yaml.bak");
                if let Err(e) = std::fs::rename(&yaml_path, &backup_path) {
                    eprintln!("Warning: Failed to backup config.yaml: {e}");
                }
            }

            return Ok(config);
        }

        Ok(Self::default())
    }

    /// Load project config with host-level `~/.cas/config.toml` staging defaults.
    ///
    /// Only the `[staging]` section is host-scoped. Project config wins when it
    /// sets `[staging]`; all other sections remain project-local to avoid
    /// leaking operator-level hooks, telemetry, LLM, or factory settings into
    /// arbitrary repositories.
    pub fn load_with_host_staging_defaults(cas_dir: &std::path::Path) -> Result<Self, MemError> {
        let mut config = Self::load(cas_dir)?;
        let host_dir = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".cas");

        if config.staging.is_none() && host_dir != cas_dir {
            let host_config = Self::load(&host_dir).unwrap_or_default();
            config.staging = host_config.staging;
        }

        Ok(config)
    }

    /// Save configuration to .cas directory as TOML (preferred format)
    pub fn save(&self, cas_dir: &std::path::Path) -> Result<(), MemError> {
        self.save_toml(cas_dir)
    }

    /// Save configuration as TOML
    pub fn save_toml(&self, cas_dir: &std::path::Path) -> Result<(), MemError> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| MemError::Parse(format!("Failed to serialize config to TOML: {e}")))?;
        write_project_config_toml(cas_dir, &content)?;
        Ok(())
    }

    /// Save configuration as YAML (legacy format)
    #[deprecated(note = "YAML config is legacy; use config.toml")]
    pub fn save_yaml(&self, cas_dir: &std::path::Path) -> Result<(), MemError> {
        let _ = cas_dir;
        Err(MemError::Parse(
            "YAML config is deprecated; use config.toml".to_string(),
        ))
    }

    /// Get path to config file (TOML preferred, YAML fallback)
    pub fn config_path(cas_dir: &std::path::Path) -> std::path::PathBuf {
        cas_dir.join("config.toml")
    }

    /// Check if sync is disabled via environment variable
    pub fn is_sync_disabled() -> bool {
        std::env::var("MEM_SYNC_DISABLED")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false)
    }

    /// Resolve `.cas/config.toml` `[factory] epic_base_branch` for the repo
    /// at `repo_root`, or `None` when unset / the config can't be read.
    ///
    /// Shared by epic-branch auto-creation and worker-spawn base resolution
    /// (cas-b082) so both paths agree on the configured trunk before
    /// falling back to `GitOperations::detect_default_branch()`.
    pub fn configured_epic_base_branch(repo_root: &std::path::Path) -> Option<String> {
        Self::load(&repo_root.join(".cas"))
            .ok()?
            .factory()
            .epic_base_branch
    }
}
