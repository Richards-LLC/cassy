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
    record_commander_mirror_config_boundary(path, contents)?;
    atomic_replace_project_config_via(path, contents, &temp_path, |from, to| fs::rename(from, to))
}

/// Read a durable policy timestamp; malformed state fails closed.
fn mirror_policy_time(path: &Path) -> std::io::Result<Option<chrono::DateTime<chrono::Utc>>> {
    match fs::read_to_string(path) {
        Ok(at) => chrono::DateTime::parse_from_rfc3339(at.trim())
            .map(|at| Some(at.with_timezone(&chrono::Utc)))
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Shared by every session in the project, including daemons that were down
/// throughout an operator's false -> true toggle.
pub(crate) fn commander_mirror_resume_floor(
    root: &Path,
) -> std::io::Result<Option<chrono::DateTime<chrono::Utc>>> {
    let policy = root.join("commander-mirror");
    let disabled = mirror_policy_time(&policy.join("policy.disabled"))?;
    let resumed = mirror_policy_time(&policy.join("policy.resumed"))?;
    if disabled > resumed {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "supervisor reply mirroring is disabled",
        ));
    }
    Ok(resumed)
}

fn mirror_enabled_in_document(contents: &str) -> Option<bool> {
    let doc: toml::Value = toml::from_str(contents).ok()?;
    Some(
        doc.get("factory")
            .and_then(|factory| factory.get("commander_mirror_enabled"))
            .and_then(toml::Value::as_bool)
            .unwrap_or(true),
    )
}

fn record_commander_mirror_config_boundary(path: &Path, contents: &str) -> std::io::Result<()> {
    let Some(enabled) = mirror_enabled_in_document(contents) else {
        return Ok(());
    };
    let Some(root) = path.parent() else {
        return Ok(());
    };
    let policy = root.join("commander-mirror");
    let disabled = mirror_policy_time(&policy.join("policy.disabled"))?;
    let resumed = mirror_policy_time(&policy.join("policy.resumed"))?;
    let was_enabled = fs::read_to_string(path)
        .ok()
        .and_then(|before| mirror_enabled_in_document(&before))
        .unwrap_or(true);
    let marker = if !enabled {
        policy.join("policy.disabled")
    } else if !was_enabled || disabled > resumed {
        policy.join("policy.resumed")
    } else {
        return Ok(());
    };
    fs::create_dir_all(&policy)?;
    let sequence = PROJECT_CONFIG_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = policy.join(format!(".policy.{}.{sequence}.tmp", std::process::id()));
    // Write before replacing config: a failed save may conservatively suppress
    // replies, but can never publish the disabled interval. Same config lock.
    atomic_replace_project_config_via(
        &marker,
        &chrono::Utc::now().to_rfc3339(),
        &temporary,
        |from, to| fs::rename(from, to),
    )
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

/// Shown when a config file still has the `[jev]` section, which was removed with Jev (cas-4971).
const RETIRED_JEV_SECTION_NOTE: &str =
    "Note: the [jev] config section is retired and ignored; remove it from config.toml.";

fn empty_qa_table() -> toml::Value {
    toml::Value::Table(Default::default())
}

fn qa_value(qa: Option<&QaConfig>) -> toml::Value {
    qa.map(|qa| toml::Value::try_from(qa).expect("QA config serializes"))
        .unwrap_or_else(empty_qa_table)
}

fn read_qa_source(dir: &Path) -> Result<toml::Value, MemError> {
    let toml_path = dir.join("config.toml");
    let yaml_path = dir.join("config.yaml");
    let document: toml::Value = if toml_path.exists() {
        toml::from_str(&fs::read_to_string(toml_path)?)
            .map_err(|error| MemError::Parse(format!("Invalid user/project QA config at {}: {error}", dir.display())))?
    } else if yaml_path.exists() {
        serde_yaml::from_str(&fs::read_to_string(yaml_path)?)?
    } else {
        return Ok(empty_qa_table());
    };
    let qa = document.get("qa").cloned().unwrap_or_else(empty_qa_table);
    // Fail on invalid user policy rather than silently changing its QA mode.
    let _: QaConfig = qa.clone().try_into().map_err(|error| MemError::Parse(format!("Invalid [qa] at {}: {error}", dir.display())))?;
    Ok(qa)
}

fn overlay_qa(base: &mut toml::Value, overrides: &toml::Value) {
    if let (Some(base), Some(overrides)) = (base.as_table_mut(), overrides.as_table()) {
        for (key, value) in overrides {
            if let Some(existing) = base.get_mut(key) {
                overlay_qa(existing, value);
            } else {
                base.insert(key.clone(), value.clone());
            }
        }
    } else {
        *base = overrides.clone();
    }
}

fn apply_qa_changes(local: &mut toml::Value, before: &toml::Value, after: &toml::Value) {
    let (Some(before), Some(after), Some(local)) = (before.as_table(), after.as_table(), local.as_table_mut()) else {
        if before != after { *local = after.clone(); }
        return;
    };
    for key in before.keys().chain(after.keys()) {
        match (before.get(key), after.get(key)) {
            (Some(old), Some(new)) if old != new => {
                apply_qa_changes(local.entry(key.clone()).or_insert_with(empty_qa_table), old, new);
            }
            (None, Some(new)) => { local.insert(key.clone(), new.clone()); }
            (Some(_), None) => { local.remove(key); }
            _ => {}
        }
    }
}

fn copy_qa_key(local: &mut toml::Value, current: &toml::Value, path: &[&str]) {
    let (key, rest) = path.split_first().expect("QA key");
    let table = local.as_table_mut().expect("QA table");
    if rest.is_empty() {
        if let Some(value) = current.get(*key) {
            table.insert((*key).into(), value.clone());
        } else if matches!(*key, "telemetry_sweep" | "preflight_hook") {
            // TOML has no null; an explicit empty command disables a user default.
            table.insert((*key).into(), toml::Value::String(String::new()));
        } else {
            table.remove(*key);
        }
    } else if let Some(current) = current.get(*key) {
        copy_qa_key(table.entry((*key).to_owned()).or_insert_with(empty_qa_table), current, rest);
    }
}

impl Config {
    /// Resolve user QA defaults without copying them into project serialization.
    pub(crate) fn resolved_qa(&self) -> QaConfig {
        let Some(layers) = &self.qa_layers else {
            return self.qa.clone().unwrap_or_default();
        };
        let mut effective = layers.user.clone();
        overlay_qa(&mut effective, &self.local_qa_value());
        let mut qa: QaConfig = effective.try_into().expect("validated QA sources and typed updates");
        qa.telemetry_sweep = qa.telemetry_sweep.filter(|value| !value.trim().is_empty());
        qa.preflight_hook = qa.preflight_hook.filter(|value| !value.trim().is_empty());
        qa
    }

    fn local_qa_value(&self) -> toml::Value {
        let Some(layers) = &self.qa_layers else { return qa_value(self.qa.as_ref()); };
        let mut local = layers.local.clone();
        apply_qa_changes(&mut local, &layers.baseline, &qa_value(self.qa.as_ref()));
        local
    }

    /// Fold direct field mutations into local policy before a single-key update.
    pub(crate) fn prepare_qa_set(&mut self) {
        let local = self.local_qa_value();
        let baseline = qa_value(self.qa.as_ref());
        if let Some(layers) = &mut self.qa_layers {
            layers.local = local;
            layers.baseline = baseline;
        }
    }

    /// Preserve explicit default-valued setters as well as typed field changes.
    pub(crate) fn record_qa_key(&mut self, key: &str) {
        let current = qa_value(self.qa.as_ref());
        if let Some(layers) = &mut self.qa_layers {
            copy_qa_key(&mut layers.local, &current, &key[3..].split('.').collect::<Vec<_>>());
            layers.baseline = current;
        }
    }

    /// Load configuration from .cas directory
    ///
    /// Tries TOML first (config.toml), falls back to YAML (config.yaml),
    /// and auto-migrates YAML to TOML on first load.
    ///
    /// When both files exist, merges any YAML-only settings into the TOML
    /// config (covers the case where something wrote to config.yaml while
    /// config.toml already existed).
    pub fn load(cas_dir: &std::path::Path) -> Result<Self, MemError> {
        Self::load_with_notice_root(cas_dir, cas_dir)
    }

    /// An index-side copy has the same settings but still belongs to its
    /// source project. Attribute notices there, without changing read/write
    /// paths or guessing ownership from a temporary directory's spelling.
    pub(crate) fn load_with_notice_root(
        cas_dir: &Path,
        notice_root: &Path,
    ) -> Result<Self, MemError> {
        let mut config = Self::load_local_with_notice_root(cas_dir, notice_root)?;
        let host = dirs::home_dir().map(|home| home.join(".cas"));
        let user = if let Some(host) = host.filter(|host| {
            host != cas_dir && host.canonicalize().ok() != cas_dir.canonicalize().ok()
        }) {
            read_qa_source(&host).unwrap_or_else(|_| {
                // A broken optional user layer must not disable every project.
                // Local project errors are still returned by the loader above.
                static WARNED: std::sync::Once = std::sync::Once::new();
                WARNED.call_once(|| {
                    let path = if host.join("config.toml").exists() {
                        host.join("config.toml")
                    } else { host.join("config.yaml") };
                    eprintln!("Warning: unreadable or invalid user QA defaults in {}\n  User defaults ignored; project and built-in settings apply.\n  Repair this file to restore inheritance.", path.display());
                });
                empty_qa_table()
            })
        } else {
            empty_qa_table()
        };
        if let Some(layers) = &mut config.qa_layers {
            layers.user = user;
        } else {
            config.qa_layers = Some(QaLayers {
                user,
                local: empty_qa_table(),
                baseline: qa_value(config.qa.as_ref()),
            });
        }
        Ok(config)
    }

    fn load_local_with_notice_root(cas_dir: &Path, notice_root: &Path) -> Result<Self, MemError> {
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
            config.qa_layers = Some(QaLayers {
                user: empty_qa_table(),
                local: toml::from_str::<toml::Value>(&content)
                    .expect("same validated TOML document").get("qa").cloned().unwrap_or_else(empty_qa_table),
                baseline: qa_value(config.qa.as_ref()),
            });
            config.note_retired_sections(notice_root);

            // If YAML also exists, merge any settings that are missing from TOML.
            // This handles the case where something wrote to config.yaml after
            // config.toml was already created (e.g. theme variant).
            if yaml_path.exists() {
                if let Ok(yaml_content) = std::fs::read_to_string(&yaml_path) {
                    if let Ok(yaml_config) = serde_yaml::from_str::<Self>(&yaml_content) {
                        let missing_qa = config.qa.is_none();
                        let changed = config.merge_missing(&yaml_config);
                        if missing_qa && config.qa.is_some() {
                            let yaml: toml::Value = serde_yaml::from_str(&yaml_content)?;
                            config.qa_layers = Some(QaLayers {
                                user: empty_qa_table(),
                                local: yaml.get("qa").cloned().unwrap_or_else(empty_qa_table),
                                baseline: qa_value(config.qa.as_ref()),
                            });
                        }
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
            let mut config: Self = serde_yaml::from_str(&content)?;
            config.qa_layers = Some(QaLayers {
                user: empty_qa_table(),
                local: serde_yaml::from_str::<toml::Value>(&content)?
                    .get("qa").cloned().unwrap_or_else(empty_qa_table),
                baseline: qa_value(config.qa.as_ref()),
            });
            config.note_retired_sections(notice_root);

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

    /// Print a one-line note, once per process, when the loaded file still
    /// carries a retired section. The section itself is ignored on load and
    /// dropped on the next save.
    fn note_retired_sections(&self, cas_dir: &Path) {
        if super::notices::defer_retired_config(cas_dir, self) {
            return;
        }
        static NOTED: std::sync::Once = std::sync::Once::new();
        if self.retired_jev.is_some() {
            NOTED.call_once(|| eprintln!("{RETIRED_JEV_SECTION_NOTE}"));
        }
        // cas-fd0a: one line naming the retired [factory] sweep keys.
        static NOTED_FACTORY: std::sync::Once = std::sync::Once::new();
        let retired = self
            .factory
            .as_ref()
            .map(FactoryConfig::retired_keys)
            .unwrap_or_default();
        if !retired.is_empty() {
            NOTED_FACTORY.call_once(|| {
                eprintln!(
                    "note: config.toml [factory] {} retired (the daemon no longer sweeps after merges; the merge queue is the integration proof); ignored and dropped on the next save.",
                    retired.join(", ")
                )
            });
        }
    }

    /// Load project config with host-level `~/.cas/config.toml` staging defaults.
    ///
    /// QA keys inherit user defaults in `load`. Project config wins when it
    /// sets `[staging]`; other sections remain project-local to avoid
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
        let mut document = toml::Value::try_from(self)
            .map_err(|e| MemError::Parse(format!("Failed to serialize config to TOML: {e}")))?;
        if self.qa_layers.is_some() {
            let local = self.local_qa_value();
            if local.as_table().is_some_and(|table| table.is_empty()) && self.qa.is_none() {
                document.as_table_mut().unwrap().remove("qa");
            } else {
                document.as_table_mut().unwrap().insert("qa".into(), local);
            }
        }
        let content = toml::to_string_pretty(&document)
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
