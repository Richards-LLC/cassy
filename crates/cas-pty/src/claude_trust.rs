//! Claude Code workspace trust for factory agents (cas-0f5b).
//!
//! Claude Code runs no hooks in a workspace it has not trusted: not
//! SessionStart, and not PreToolUse from `--settings`, project or user
//! settings. Factory agents launch with `IS_DEMO=true`, which skips the
//! interactive trust dialog without trusting the workspace, so every Claude
//! worker ran with no CAS guard at all: no capped cargo runner, no
//! worker-memory admission, no Slack/publication/browser guards. Measured in
//! cas-0f5b probe round 2: same launch, no trust entry → the canary hook never
//! ran; with `projects[<cwd>].hasTrustDialogAccepted = true` → it ran and its
//! deny held.
//!
//! [`ensure_claude_project_trusted_in`] records that one fact before spawn:
//!
//! - it merges only `projects["<cwd>"].hasTrustDialogAccepted = true` into
//!   Claude's global config (`$CLAUDE_CONFIG_DIR/.claude.json`, or
//!   `~/.claude.json`), keeping every other key and value;
//! - the read-modify-write/read-back runs under an exclusive advisory lock
//!   (`.claude.json.cas-lock`) shared by every CAS writer, through a temp file,
//!   fsync and rename, preserving the file's mode and any symlink;
//! - live Claude sessions rewrite the same file without that lock, so a
//!   write can be lost between rename and read-back; the transaction is
//!   retried once before the launch is refused.
//!
//! - serde_json is built without `preserve_order` (enabling it would unify
//!   onto every crate in the binary), so a rewrite re-sorts object keys.
//!   Values are unchanged and Claude does not depend on key order; the diff
//!   is noisy only.
//!
//! Trust is necessary but not sufficient: the factory's launch canary (the
//! SessionStart marker) is what proves hooks actually run.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use fs2::FileExt;
use serde_json::{Map, Value};

/// The single field this module ever writes.
pub const CLAUDE_TRUST_FIELD: &str = "hasTrustDialogAccepted";

/// Outcome of [`ensure_claude_project_trusted_in`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeTrustOutcome {
    /// The trust flag was written for these keys.
    Added(Vec<String>),
    /// Every key was already trusted; the file was not rewritten.
    AlreadyPresent,
    /// Refused without touching the file; the caller must not launch.
    Skipped(&'static str),
}

/// Claude Code's global config for an agent: `<config_dir>/.claude.json` when
/// the agent runs with `CLAUDE_CONFIG_DIR`, else the daemon's own
/// `CLAUDE_CONFIG_DIR`, else `~/.claude.json`.
pub fn claude_global_config_path(config_dir: Option<&str>) -> Option<PathBuf> {
    let explicit = config_dir
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("CLAUDE_CONFIG_DIR")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from)
        });
    match explicit {
        Some(dir) => Some(dir.join(".claude.json")),
        None => std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(|home| PathBuf::from(home).join(".claude.json")),
    }
}

/// The cwd as written and as canonicalized; Claude keys projects by the
/// absolute working directory it starts in.
fn candidate_keys(workdir: &Path) -> Vec<String> {
    let mut keys = vec![workdir.to_string_lossy().to_string()];
    if let Ok(canonical) = std::fs::canonicalize(workdir) {
        let canonical = canonical.to_string_lossy().to_string();
        if !keys.contains(&canonical) {
            keys.push(canonical);
        }
    }
    keys
}

fn is_trusted(config: &Value, key: &str) -> bool {
    config
        .get("projects")
        .and_then(|projects| projects.get(key))
        .and_then(|project| project.get(CLAUDE_TRUST_FIELD))
        .and_then(Value::as_bool)
        == Some(true)
}

/// Parse an existing config. Empty or whitespace-only contents are refused, not
/// treated as `{}`: a live Claude session can leave the file truncated
/// mid-write, and renaming `{"projects":…}` over it would wipe the account's
/// whole config. Only a genuinely missing file starts from `{}`.
fn parse_config(contents: &str, path: &Path) -> io::Result<Value> {
    if contents.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Claude config {} exists but is empty (possibly mid-write by a live session); \
                 refusing to rewrite it",
                path.display()
            ),
        ));
    }
    let value: Value = serde_json::from_str(contents).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Claude config {} does not parse as JSON: {error}", path.display()),
        )
    })?;
    if !value.is_object() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Claude config {} is not a JSON object", path.display()),
        ));
    }
    Ok(value)
}

/// Set the trust flag for `keys` in place, touching nothing else. Returns
/// `Ok(false)` when every key was already trusted.
pub fn merge_claude_trust(config: &mut Value, keys: &[String]) -> io::Result<bool> {
    let root = config.as_object_mut().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "Claude config is not a JSON object")
    })?;
    let projects = root
        .entry("projects")
        .or_insert_with(|| Value::Object(Map::new()));
    let projects = projects.as_object_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Claude config `projects` is not a JSON object",
        )
    })?;
    let mut changed = false;
    for key in keys {
        let project = projects
            .entry(key.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        let project = project.as_object_mut().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Claude config projects[{key:?}] is not a JSON object"),
            )
        })?;
        if project.get(CLAUDE_TRUST_FIELD).and_then(Value::as_bool) != Some(true) {
            project.insert(CLAUDE_TRUST_FIELD.to_string(), Value::Bool(true));
            changed = true;
        }
    }
    Ok(changed)
}

fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct ConfigLock {
    file: File,
}

impl ConfigLock {
    fn acquire(config_path: &Path) -> io::Result<Self> {
        let mut name = config_path.as_os_str().to_owned();
        name.push(".cas-lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(PathBuf::from(name))?;
        file.lock_exclusive()?;
        Ok(Self { file })
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(unix)]
fn sync_parent_directory(config_path: &Path) -> io::Result<()> {
    if let Some(parent) = config_path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent_directory(_config_path: &Path) -> io::Result<()> {
    Ok(())
}

/// One locked transaction: read, merge, write-temp/fsync/rename, read back.
/// Returns `(changed, verified)`.
fn trust_transaction(config_path: &Path, keys: &[String]) -> io::Result<(bool, bool)> {
    let _guard = write_lock().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Follow a managed symlink: `rename` would otherwise replace the link.
    let config_path = std::fs::canonicalize(config_path)
        .ok()
        .unwrap_or_else(|| config_path.to_path_buf());
    let _lock = ConfigLock::acquire(&config_path)?;
    let mut config = match std::fs::read_to_string(&config_path) {
        Ok(contents) => parse_config(&contents, &config_path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(error) => return Err(error),
    };
    let changed = merge_claude_trust(&mut config, keys)?;
    if changed {
        let updated = serde_json::to_string_pretty(&config).map_err(io::Error::other)?;
        let mut tmp_name = config_path.as_os_str().to_owned();
        tmp_name.push(format!(".cas-{}", std::process::id()));
        let tmp = PathBuf::from(tmp_name);
        std::fs::write(&tmp, updated.as_bytes())?;
        if let Ok(meta) = std::fs::metadata(&config_path) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        OpenOptions::new().read(true).open(&tmp)?.sync_all()?;
        if let Err(error) = std::fs::rename(&tmp, &config_path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(error);
        }
        sync_parent_directory(&config_path)?;
    }
    let read_back = parse_config(&std::fs::read_to_string(&config_path)?, &config_path)?;
    let verified = keys.iter().all(|key| is_trusted(&read_back, key));
    Ok((changed, verified))
}

/// Ensure Claude Code trusts `workdir` in the global config at `config_path`.
///
/// Refuses (an error, or [`ClaudeTrustOutcome::Skipped`]) rather than
/// corrupting the file: a malformed `.claude.json` would break every Claude
/// session on the account, which is worse than a refused launch.
pub fn ensure_claude_project_trusted_in(
    config_path: &Path,
    workdir: &Path,
) -> io::Result<ClaudeTrustOutcome> {
    if !workdir.is_absolute() {
        return Ok(ClaudeTrustOutcome::Skipped(
            "agent cwd is not absolute; cannot key a Claude trust entry",
        ));
    }
    let keys = candidate_keys(workdir);
    if keys.iter().any(|key| key.chars().any(char::is_control)) {
        return Ok(ClaudeTrustOutcome::Skipped(
            "agent cwd contains control characters; refusing to write a Claude trust entry",
        ));
    }
    let mut changed_any = false;
    // A live Claude session may rewrite the file between our rename and the
    // read-back (lost update). Retry the whole transaction once.
    for _ in 0..2 {
        let (changed, verified) = trust_transaction(config_path, &keys)?;
        changed_any |= changed;
        if verified {
            return Ok(if changed_any {
                ClaudeTrustOutcome::Added(keys)
            } else {
                ClaudeTrustOutcome::AlreadyPresent
            });
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "Claude trust for {keys:?} did not survive read-back in {} (a concurrent Claude \
             session rewrote it twice)",
            config_path.display()
        ),
    ))
}

#[cfg(test)]
#[path = "claude_trust_tests.rs"]
mod tests;
