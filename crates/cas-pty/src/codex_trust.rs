//! Pre-trust a working directory for the Codex CLI (cas-28a49, GH #97).
//!
//! # Why this exists
//!
//! Codex CLI refuses to start in a directory that is absent from the
//! `[projects]` trust table in `$CODEX_HOME/config.toml` (default
//! `~/.codex/config.toml`). It parks on its interactive "do you trust the files
//! in this folder?" onboarding screen **before** it renders a TUI, before it
//! writes a session file, and before it starts any MCP server — so a factory
//! worker launched there never registers with CAS and the spawn dies at
//! `stage=register` with a generic 60s timeout.
//!
//! Verified against codex-cli 0.146.0 on Linux by launching `codex --yolo
//! --no-alt-screen` under a real PTY in three configurations:
//!
//! | workdir state | bytes of TUI output in 12s |
//! |---|---|
//! | listed in `[projects]` as `trusted` | ~27 000 (normal startup) |
//! | absent from `[projects]` | 115 (terminal capability queries only — wedged) |
//! | absent, launched with `-c 'projects."<dir>".trust_level="trusted"'` | 115 (wedged) |
//!
//! The third row is the important one: the CLI `-c` override does **not**
//! satisfy the trust check, so the entry has to exist on disk before launch.
//! That is what [`ensure_project_trusted`] does.
//!
//! The write is deliberately conservative — corrupting this file would stop
//! Codex from starting *anywhere*, which is strictly worse than the wedge being
//! fixed:
//! - it only ever **appends** a `[projects."<abs path>"]` block; it never
//!   rewrites, reformats, or reorders the operator's config;
//! - presence is decided by parsing the TOML, not by matching lines, so an
//!   existing entry in any spelling (`[projects."x"] # comment`, an inline
//!   `[projects]` entry, a dotted `projects."x".trust_level = …`) is left
//!   completely alone, whatever its `trust_level` — an operator who explicitly
//!   untrusted a directory keeps that decision, and no duplicate table is ever
//!   created;
//! - a config that does not parse, a result that would not parse, and a path
//!   that cannot be a TOML key (control characters, non-UTF-8) all abort the
//!   write instead of guessing;
//! - the read-modify-write/read-back transaction is guarded by a blocking OS
//!   advisory lock so a concurrent factory process (or another CAS-owned
//!   Codex registration) cannot lose an update or launch before its state is
//!   verified;
//! - the file is replaced via write-temp-then-rename, through any symlink and
//!   preserving the original mode, then the file and its parent directory are
//!   synced and parsed back before launch; a concurrently reading Codex process
//!   never observes a half-written config and a `0600` config stays `0600`.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use fs2::FileExt;
use sha2::{Digest, Sha256};

/// What [`ensure_project_trusted`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexTrustOutcome {
    /// Every candidate path already had a `[projects."..."]` entry.
    AlreadyPresent,
    /// One or more trust entries were appended to the config file.
    Added(Vec<String>),
    /// Nothing was attempted; the reason is operator-facing.
    Skipped(&'static str),
}

/// Resolve the directory Codex reads its `config.toml` from.
///
/// `$CODEX_HOME` wins when set (that is Codex's own override), otherwise
/// `~/.codex`.
pub fn codex_home() -> Option<PathBuf> {
    match std::env::var_os("CODEX_HOME") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => dirs::home_dir().map(|home| home.join(".codex")),
    }
}

/// Escape a path for use inside a TOML basic string key.
fn escape_toml_basic(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out
}

/// The two CAS-native Codex handlers whose hash state CAS owns. Codex's trust
/// key also includes a positional matcher-group and handler index, so the
/// values are discovered from the generated `hooks.json` before writing state.
const CAS_CODEX_HOOKS: [(&str, &str, &str); 2] = [
    (
        "PreToolUse",
        "pre_tool_use",
        "CAS_HOOK_HARNESS=codex cas hook PreToolUse",
    ),
    ("PostToolUse", "post_tool_use", "cas hook PostToolUse"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct HookTrustEntry {
    key: String,
    hash: String,
}

/// Return the canonical Codex trust hash for a normalized command hook.
///
/// Captured against Codex CLI 0.147.0 using its `hooks/list` RPC and confirmed
/// against the upstream `NormalizedHookIdentity` implementation: SHA-256 of
/// canonical JSON for `{ event_name, matcher, hooks: [handler] }`.
fn codex_command_hook_hash(
    event_name: &str,
    matcher: &str,
    command: &str,
    timeout: u64,
    runs_async: bool,
) -> String {
    let identity = serde_json::json!({
        "event_name": event_name,
        "matcher": matcher,
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": timeout,
            "async": runs_async,
        }],
    });
    // `serde_json::Map` is a BTreeMap in our configuration, so this emits the
    // recursively sorted canonical form Codex hashes. The regression below pins
    // the two hashes returned by the real CLI.
    let bytes = serde_json::to_vec(&identity).expect("JSON hook identity serializes");
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn cas_hook_trust_entries(hooks_path: &Path) -> io::Result<Vec<HookTrustEntry>> {
    if !hooks_path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Codex hooks path must be absolute to register trust",
        ));
    }
    let source = hooks_path.to_string_lossy().to_string();
    if path_is_unsafe_key(&source) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Codex hooks path contains control characters or invalid UTF-8",
        ));
    }
    let contents = std::fs::read_to_string(hooks_path)?;
    let hooks: serde_json::Value = serde_json::from_str(&contents).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to parse {}: {error}", hooks_path.display()),
        )
    })?;
    let root = hooks
        .get("hooks")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "hooks.json has no hooks object")
        })?;

    let mut entries = Vec::with_capacity(CAS_CODEX_HOOKS.len());
    for (event, event_key, expected_command) in CAS_CODEX_HOOKS {
        let groups = root
            .get(event)
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("hooks.json is missing CAS {event} hook groups"),
                )
            })?;
        let mut found = None;
        for (group_index, group) in groups.iter().enumerate() {
            let matcher = group
                .get("matcher")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let Some(handlers) = group.get("hooks").and_then(serde_json::Value::as_array) else {
                continue;
            };
            for (handler_index, handler) in handlers.iter().enumerate() {
                if handler.get("type").and_then(serde_json::Value::as_str) != Some("command")
                    || handler.get("command").and_then(serde_json::Value::as_str)
                        != Some(expected_command)
                {
                    continue;
                }
                let timeout = handler
                    .get("timeout")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(600)
                    .max(1);
                let runs_async = handler
                    .get("async")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                found = Some(HookTrustEntry {
                    key: format!("{source}:{event_key}:{group_index}:{handler_index}"),
                    hash: codex_command_hook_hash(
                        event_key,
                        matcher,
                        expected_command,
                        timeout,
                        runs_async,
                    ),
                });
                break;
            }
            if found.is_some() {
                break;
            }
        }
        entries.push(found.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("hooks.json does not contain canonical CAS {event} command"),
            )
        })?);
    }
    Ok(entries)
}

fn hook_state_is_trusted(config: &str, entry: &HookTrustEntry) -> Result<bool, toml::de::Error> {
    let parsed: toml::Value = toml::from_str(config)?;
    Ok(parsed
        .get("hooks")
        .and_then(|hooks| hooks.get("state"))
        .and_then(|state| state.get(&entry.key))
        .and_then(|state| state.get("trusted_hash"))
        .and_then(toml::Value::as_str)
        == Some(entry.hash.as_str()))
}

fn hook_state_entry_block(entry: &HookTrustEntry) -> String {
    format!(
        "\n[hooks.state.\"{}\"]\ntrusted_hash = \"{}\"\n",
        escape_toml_basic(&entry.key),
        entry.hash
    )
}

/// Update the exact table spelling CAS itself appends, retaining all unrelated
/// comments and configuration bytes. Other valid TOML spellings are deliberately
/// not rewritten: CAS must never reserialize an operator's whole Codex config
/// merely to refresh a hook hash.
fn refresh_owned_hook_state(existing: &str, entry: &HookTrustEntry) -> io::Result<Option<String>> {
    if hook_state_is_trusted(existing, entry)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?
    {
        return Ok(None);
    }
    let parsed: toml::Value = toml::from_str(existing)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let has_state = parsed
        .get("hooks")
        .and_then(|hooks| hooks.get("state"))
        .and_then(|state| state.get(&entry.key))
        .is_some();
    if !has_state {
        let mut updated = existing.to_string();
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str(&hook_state_entry_block(entry));
        return Ok(Some(updated));
    }

    let header = format!("[hooks.state.\"{}\"]", escape_toml_basic(&entry.key));
    let Some(header_start) = existing
        .lines()
        .scan(0usize, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1;
            Some((start, line))
        })
        .find_map(|(start, line)| (line.trim() == header).then_some(start))
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "existing CAS hook trust uses an unsupported TOML spelling; refusing to rewrite it",
        ));
    };
    let after_header = existing[header_start..]
        .find('\n')
        .map(|relative| header_start + relative + 1)
        .unwrap_or(existing.len());
    let section_end = existing[after_header..]
        .find("\n[")
        .map(|relative| after_header + relative + 1)
        .unwrap_or(existing.len());
    let section = &existing[after_header..section_end];
    let Some((line_start, line_end)) = section
        .lines()
        .scan(0usize, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1;
            Some((start, line))
        })
        .find_map(|(start, line)| {
            line.trim_start()
                .starts_with("trusted_hash")
                .then_some((after_header + start, after_header + start + line.len()))
        })
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "existing CAS hook trust lacks trusted_hash; refusing to rewrite it",
        ));
    };
    let indent = existing[line_start..line_end]
        .chars()
        .take_while(|ch| ch.is_whitespace())
        .collect::<String>();
    let mut updated = String::with_capacity(existing.len() + entry.hash.len());
    updated.push_str(&existing[..line_start]);
    updated.push_str(&format!("{indent}trusted_hash = \"{}\"", entry.hash));
    updated.push_str(&existing[line_end..]);
    Ok(Some(updated))
}

/// Record the current CAS hook definitions as trusted in Codex's user config.
///
/// Codex uses this separate state from project trust: without it the project
/// layer loads, but non-managed handlers are silently skipped until someone
/// opens `/hooks`. Uses the same lock/fsync/read-back transaction as the
/// project-trust writer, and only appends CAS-owned state entries. A changed
/// definition deliberately receives a new state key only after the generator
/// has emitted its canonical handler shape.
pub fn ensure_cas_hooks_trusted_in(config_path: &Path, hooks_path: &Path) -> io::Result<bool> {
    let entries = cas_hook_trust_entries(hooks_path)?;
    let entries_for_write = entries.clone();
    update_codex_config_locked(
        config_path,
        move |existing| {
            let mut updated = existing.to_string();
            let mut changed = false;
            for entry in &entries_for_write {
                if let Some(next) = refresh_owned_hook_state(&updated, entry)? {
                    updated = next;
                    changed = true;
                }
            }
            Ok(changed.then_some(updated))
        },
        |read_back| {
            for entry in &entries {
                if !hook_state_is_trusted(read_back, entry).map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
                })? {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "read-back verification failed for CAS hook state {}",
                            entry.key
                        ),
                    ));
                }
            }
            Ok(())
        },
    )
}

/// Resolve the active Codex user config (`CODEX_HOME` or `~/.codex`) and trust
/// the CAS handlers in `hooks_path` without an interactive `/hooks` review.
pub fn ensure_cas_hooks_trusted(hooks_path: &Path) -> io::Result<bool> {
    let home = codex_home().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not resolve CODEX_HOME or a home directory for Codex hook trust",
        )
    })?;
    ensure_cas_hooks_trusted_in(&home.join("config.toml"), hooks_path)
}

/// The literal block appended for a newly trusted path.
pub fn trust_entry_block(path: &str) -> String {
    format!(
        "\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
        escape_toml_basic(path)
    )
}

/// Does `config` already declare a project entry for `path`?
///
/// Parsed with a real TOML parser rather than matched line-by-line, because
/// `[projects."x"]`, `[projects."x"] # comment`, `[ projects."x" ]`,
/// `projects."x".trust_level = ...` and an inline `[projects]` table entry are
/// all the same key. Missing any of those spellings and appending a second
/// `[projects."x"]` table would make the whole file invalid TOML — TOML forbids
/// redeclaring a table — which would stop Codex from starting **anywhere**, a
/// far worse failure than the bug being fixed.
///
/// `Err` means the config does not parse at all; the caller must then refuse to
/// touch it.
///
/// Only the key's presence is checked, never `trust_level`'s value: an existing
/// entry means the operator (or Codex itself) already decided about this
/// directory.
pub fn config_has_project(config: &str, path: &str) -> Result<bool, toml::de::Error> {
    let parsed: toml::Value = toml::from_str(config)?;
    Ok(parsed
        .get("projects")
        .and_then(|projects| projects.get(path))
        .is_some())
}

/// Paths that cannot be written as a TOML basic-string key without corrupting
/// (or, with an embedded newline, *injecting into*) the operator's config.
fn path_is_unsafe_key(path: &str) -> bool {
    path.contains('\u{FFFD}') || path.chars().any(|c| c.is_control())
}

/// Path keys to trust for `workdir`: the path as spawned, plus its canonical
/// form when symlink resolution changes it (Codex matches on the cwd string it
/// resolves, and a factory worktree can be reached through either).
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

/// Serializes config mutations within this process. Across processes,
/// [`ConfigLock`] guards the same transaction.
fn config_write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Cross-process lock over the read-modify-write/read-back window.
///
/// Atomic rename only prevents a *torn read*; it does not prevent a lost
/// update. Two factory processes (or a human answering Codex's own trust prompt
/// in the window) would otherwise have the later rename revert the whole file to
/// the earlier snapshot. This deliberately blocks instead of falling back to an
/// unlocked write: spawning Codex before its entry is durable reintroduces the
/// trust-prompt race this lock exists to prevent. OS advisory locks release when
/// a process exits, including a crash, so there is no stale-lock timeout path.
struct ConfigLock {
    file: File,
}

impl ConfigLock {
    fn acquire(config_path: &Path) -> io::Result<Self> {
        let path = config_path.with_extension("toml.cas-lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)?;
        file.lock_exclusive()?;
        Ok(Self { file })
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn verify_trusted_project_entry(contents: &str, keys: &[String]) -> io::Result<()> {
    let parsed: toml::Value = toml::from_str(contents).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("read-back TOML parse failed while verifying [projects]: {error}"),
        )
    })?;
    let mut trusted = false;
    for key in keys {
        trusted |= parsed
            .get("projects")
            .and_then(|projects| projects.get(key))
            .and_then(|project| project.get("trust_level"))
            .and_then(toml::Value::as_str)
            == Some("trusted");
    }
    if !trusted {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("read-back verification failed: no trusted [projects.{keys:?}] entry"),
        ));
    }
    Ok(())
}

/// Persist the directory entry created by the rename as well as the file's
/// contents. This is a no-op on platforms where opening a directory for fsync
/// is not supported; the read-back still guards the launch ordering there.
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

/// Mutate Codex's user config while holding the single transaction lock used by
/// every CAS-owned Codex registration.
///
/// `mutate` receives the complete current TOML and returns `Some(updated)` only
/// when it needs to replace the file. `verify` receives a freshly read file
/// *after* the temp file and containing directory are synced, while the lock is
/// still held. It must reject any state that would make a caller unsafe to
/// launch Codex. This lets project trust and hook approval state share one
/// read-modify-write/read-back boundary without either feature having to parse
/// or serialize the other's tables.
pub fn update_codex_config_locked<F, V>(
    config_path: &Path,
    mutate: F,
    verify: V,
) -> io::Result<bool>
where
    F: FnOnce(&str) -> io::Result<Option<String>>,
    V: FnOnce(&str) -> io::Result<()>,
{
    let _guard = config_write_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // `read_to_string` follows a symlink but `rename` would replace the link.
    // Resolve first so managed writes preserve a dotfiles-managed link.
    let config_path = std::fs::canonicalize(config_path)
        .ok()
        .unwrap_or_else(|| config_path.to_path_buf());
    let _lock = ConfigLock::acquire(&config_path)?;
    let existing = match std::fs::read_to_string(&config_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };

    let changed = if let Some(updated) = mutate(&existing)? {
        toml::from_str::<toml::Value>(&updated).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "CAS Codex config mutation would produce invalid TOML at {}: {error}",
                    config_path.display()
                ),
            )
        })?;
        let tmp = config_path.with_extension(format!("toml.cas-{}", std::process::id()));
        std::fs::write(&tmp, &updated)?;
        // Rename replaces original metadata; preserve a restrictive config mode.
        if let Ok(meta) = std::fs::metadata(&config_path) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        OpenOptions::new().read(true).open(&tmp)?.sync_all()?;
        if let Err(error) = std::fs::rename(&tmp, &config_path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(error);
        }
        sync_parent_directory(&config_path)?;
        true
    } else {
        false
    };

    let read_back = std::fs::read_to_string(&config_path)?;
    verify(&read_back)?;
    Ok(changed)
}

/// Ensure `workdir` is trusted in the Codex config at `config_path`.
///
/// Creates the file (and its parent directory) when missing. Refuses to write
/// when the existing config does not parse, when the result would not parse, or
/// when the path cannot be represented as a TOML key — corrupting
/// `config.toml` would break every Codex launch on the machine, which is
/// strictly worse than the wedge this function prevents.
pub fn ensure_project_trusted_in(
    config_path: &Path,
    workdir: &Path,
) -> io::Result<CodexTrustOutcome> {
    if !workdir.is_absolute() {
        return Ok(CodexTrustOutcome::Skipped(
            "worker cwd is not absolute; cannot key a Codex trust entry",
        ));
    }
    let keys = candidate_keys(workdir);
    if keys.iter().any(|key| path_is_unsafe_key(key)) {
        return Ok(CodexTrustOutcome::Skipped(
            "worker cwd contains control characters or is not valid UTF-8; refusing to write a \
             Codex trust entry for it",
        ));
    }

    let keys_for_write = keys.clone();
    let changed = match update_codex_config_locked(
        config_path,
        move |existing| {
            let mut already_present = false;
            for key in &keys_for_write {
                match config_has_project(existing, key) {
                    Ok(false) => {}
                    Ok(true) => already_present = true,
                    Err(error) => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("Codex config does not parse as TOML: {error}"),
                        ));
                    }
                }
            }
            if already_present {
                return Ok(None);
            }
            let mut updated = existing.to_string();
            if !updated.is_empty() && !updated.ends_with('\n') {
                updated.push('\n');
            }
            for key in &keys_for_write {
                updated.push_str(&trust_entry_block(key));
            }
            Ok(Some(updated))
        },
        |read_back| verify_trusted_project_entry(read_back, &keys),
    ) {
        Ok(changed) => changed,
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            tracing::warn!(
                config = %config_path.display(),
                error = %error,
                "cas-3603: Codex config mutation/read-back verification failed; leaving launch blocked"
            );
            return Ok(CodexTrustOutcome::Skipped(
                "the Codex config could not be verified after mutation; refusing to launch",
            ));
        }
        Err(error) => return Err(error),
    };
    Ok(if changed {
        CodexTrustOutcome::Added(keys)
    } else {
        CodexTrustOutcome::AlreadyPresent
    })
}

/// Ensure `workdir` is trusted in the resolved Codex config
/// (`$CODEX_HOME/config.toml`, else `~/.codex/config.toml`).
///
/// The caller must treat any error or `Skipped` result as a failed precondition
/// and must not launch Codex. Starting first would let Codex cache a missing
/// trust entry and park at its interactive prompt even if a later write lands.
pub fn ensure_project_trusted(workdir: &Path) -> io::Result<CodexTrustOutcome> {
    let Some(home) = codex_home() else {
        return Ok(CodexTrustOutcome::Skipped(
            "could not resolve CODEX_HOME or a home directory for ~/.codex",
        ));
    };
    let config_path = home.join("config.toml");
    match ensure_project_trusted_in(&config_path, workdir) {
        Ok(CodexTrustOutcome::Added(keys)) => {
            tracing::info!(
                config = %config_path.display(),
                paths = ?keys,
                "cas-28a49: pre-trusted workdir for Codex; without this the CLI parks on its \
                 interactive trust prompt and the worker never registers"
            );
            Ok(CodexTrustOutcome::Added(keys))
        }
        Ok(other) => Ok(other),
        Err(e) => {
            tracing::warn!(
                config = %config_path.display(),
                workdir = %workdir.display(),
                error = %e,
                "cas-28a49: could not pre-trust workdir for Codex; if the worker never registers, \
                 add [projects.\"<workdir>\"] trust_level = \"trusted\" to the Codex config"
            );
            Err(e)
        }
    }
}

/// Operator-facing hint appended to a Codex worker's registration-timeout
/// diagnostic (cas-28a49, GH #97). Named separately so the daemon and its tests
/// share one string.
pub const CODEX_TRUST_TIMEOUT_HINT: &str = "This worker runs cli=codex: Codex refuses to start in a directory that is not listed in \
     [projects] in $CODEX_HOME/config.toml (default ~/.codex/config.toml), parking on its \
     interactive trust prompt before it can register. CAS pre-trusts the worker's cwd at launch, \
     so check that file for a [projects.\"<worker cwd>\"] entry with trust_level = \"trusted\"; if \
     it is missing, the daemon log records why the write was skipped (unparseable config, or the \
     entry already existed with another trust_level).";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_existing_entry_in_every_toml_spelling() {
        // cas-28a49 review follow-up: a matcher that misses any of these would
        // append a duplicate [projects."x"] table, and TOML forbids redeclaring
        // a table — the config would stop parsing and Codex would fail to start
        // for EVERY directory, not just this one.
        for config in [
            "[projects.\"/home/a/proj\"]\ntrust_level = \"trusted\"\n",
            "[projects.'/home/a/proj']\ntrust_level = \"trusted\"\n",
            "[projects.\"/home/a/proj\"] # main repo\ntrust_level = \"trusted\"\n",
            "[ projects.\"/home/a/proj\" ]\ntrust_level = \"trusted\"\n",
            "[projects]\n\"/home/a/proj\" = { trust_level = \"trusted\" }\n",
            "projects.\"/home/a/proj\".trust_level = \"trusted\"\n",
        ] {
            assert!(
                config_has_project(config, "/home/a/proj").unwrap(),
                "must detect the existing key in: {config}"
            );
            assert!(
                !config_has_project(config, "/home/a/other").unwrap(),
                "must not report an unrelated key present in: {config}"
            );
        }
    }

    #[test]
    fn does_not_match_prefixes_or_other_tables() {
        let config = "[projects.\"/home/a/proj-two\"]\n[mcp_servers.cs]\n";
        assert!(!config_has_project(config, "/home/a/proj").unwrap());
        assert!(!config_has_project(config, "cs").unwrap());
    }

    #[test]
    fn unparseable_config_is_reported_not_guessed() {
        assert!(config_has_project("this is [not( toml", "/home/a/proj").is_err());
    }

    #[test]
    fn escaped_keys_round_trip() {
        let path = "/home/a/we\"ird\\dir";
        let block = trust_entry_block(path);
        assert!(
            config_has_project(&block, path).unwrap(),
            "block was: {block}"
        );
    }

    #[test]
    fn appends_entry_when_missing_and_creates_file() {
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();

        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert!(
            matches!(outcome, CodexTrustOutcome::Added(_)),
            "first call must append an entry, got {outcome:?}"
        );
        let written = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            config_has_project(&written, &workdir.to_string_lossy()).unwrap(),
            "config must contain the trust entry: {written}"
        );
        assert!(written.contains("trust_level = \"trusted\""));
    }

    #[test]
    fn second_call_is_a_noop_and_preserves_operator_content() {
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        std::fs::write(&config_path, "# operator comment\nmodel = \"gpt-5\"\n").unwrap();

        ensure_project_trusted_in(&config_path, &workdir).unwrap();
        let after_first = std::fs::read_to_string(&config_path).unwrap();
        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert_eq!(outcome, CodexTrustOutcome::AlreadyPresent);
        let after_second = std::fs::read_to_string(&config_path).unwrap();
        assert_eq!(after_first, after_second, "second call must not rewrite");
        assert!(after_second.contains("# operator comment"));
        assert!(after_second.contains("model = \"gpt-5\""));
    }

    #[test]
    fn locked_transaction_supports_independent_hook_state_mutation_and_read_back() {
        // cas-3603: hooks-trust registration uses the same transaction as
        // `[projects]`; this proves an unrelated config table is retained and
        // must pass a fresh verifier before the transaction returns.
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        std::fs::write(&config_path, "model = \"gpt-5\"\n").unwrap();
        let hook_state = "[hooks.state.\"/tmp/hooks.json:pre_tool_use:cas:handler\"]\ntrusted_hash = \"sha256:example\"\n";

        let changed = update_codex_config_locked(
            &config_path,
            |existing| Ok(Some(format!("{existing}\n{hook_state}"))),
            |read_back| {
                let parsed: toml::Value = toml::from_str(read_back).map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
                })?;
                let hash = parsed
                    .get("hooks")
                    .and_then(|value| value.get("state"))
                    .and_then(|value| value.get("/tmp/hooks.json:pre_tool_use:cas:handler"))
                    .and_then(|value| value.get("trusted_hash"))
                    .and_then(toml::Value::as_str);
                if hash == Some("sha256:example") {
                    Ok(())
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "hook state was not present after read-back",
                    ))
                }
            },
        )
        .unwrap();
        assert!(changed);
        assert!(
            std::fs::read_to_string(&config_path)
                .unwrap()
                .contains("model = \"gpt-5\"")
        );
    }

    #[test]
    fn codex_hook_hashes_match_the_live_cli_canonical_identities() {
        // Recorded from `codex app-server` hooks/list with codex-cli 0.147.0.
        // Codex normalizes the hook identity before hashing, so matching the
        // generated JSON bytes themselves is not sufficient.
        assert_eq!(
            codex_command_hook_hash(
                "pre_tool_use",
                "^Bash$",
                "CAS_HOOK_HARNESS=codex cas hook PreToolUse",
                3,
                false,
            ),
            "sha256:1acc1fe9f8d33d69fb3a16be30738b902dac8e20155fdb9a6490b2a52545ca77"
        );
        assert_eq!(
            codex_command_hook_hash("post_tool_use", "^Bash$", "cas hook PostToolUse", 3, false),
            "sha256:94c125201a0019ec158d7abb96cd3fc0d8282bb499aa506f202ce794748cdb2b"
        );
    }

    #[test]
    fn hook_trust_registration_uses_actual_indexes_and_is_idempotent() {
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let hooks_path = dir.join("project/.codex/hooks.json");
        std::fs::create_dir_all(hooks_path.parent().unwrap()).unwrap();
        // A non-CAS matcher group ahead of PreToolUse proves we use Codex's
        // positional trust key, rather than assuming every CAS handler is 0:0.
        std::fs::write(
            &hooks_path,
            r#"{
  "hooks": {
    "PreToolUse": [
      {"matcher":"^Read$","hooks":[{"type":"command","command":"operator hook"}]},
      {"matcher":"^Bash$","hooks":[{"type":"command","command":"CAS_HOOK_HARNESS=codex cas hook PreToolUse","timeout":3}]}
    ],
    "PostToolUse": [
      {"matcher":"^Bash$","hooks":[{"type":"command","command":"cas hook PostToolUse","timeout":3}]}
    ]
  }
}"#,
        )
        .unwrap();
        std::fs::write(&config_path, "# operator content\nmodel = \"gpt-5\"\n").unwrap();

        assert!(ensure_cas_hooks_trusted_in(&config_path, &hooks_path).unwrap());
        let after_first = std::fs::read_to_string(&config_path).unwrap();
        let entries = cas_hook_trust_entries(&hooks_path).unwrap();
        assert_eq!(
            entries[0].key,
            format!("{}:pre_tool_use:1:0", hooks_path.display())
        );
        assert_eq!(
            entries[1].key,
            format!("{}:post_tool_use:0:0", hooks_path.display())
        );
        for entry in &entries {
            assert!(hook_state_is_trusted(&after_first, entry).unwrap());
        }
        assert!(after_first.contains("# operator content"));

        assert!(!ensure_cas_hooks_trusted_in(&config_path, &hooks_path).unwrap());
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), after_first);
    }

    #[test]
    fn hook_trust_registration_refreshes_only_its_stale_hash() {
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let hooks_path = dir.join("project/.codex/hooks.json");
        std::fs::create_dir_all(hooks_path.parent().unwrap()).unwrap();
        std::fs::write(
            &hooks_path,
            r#"{"hooks":{"PreToolUse":[{"matcher":"^Bash$","hooks":[{"type":"command","command":"CAS_HOOK_HARNESS=codex cas hook PreToolUse","timeout":3}]}],"PostToolUse":[{"matcher":"^Bash$","hooks":[{"type":"command","command":"cas hook PostToolUse","timeout":3}]}]}}"#,
        )
        .unwrap();
        ensure_cas_hooks_trusted_in(&config_path, &hooks_path).unwrap();
        let entries = cas_hook_trust_entries(&hooks_path).unwrap();
        let original = std::fs::read_to_string(&config_path).unwrap();
        let stale = original.replacen(&entries[0].hash, "sha256:stale", 1);
        std::fs::write(&config_path, stale).unwrap();

        assert!(ensure_cas_hooks_trusted_in(&config_path, &hooks_path).unwrap());
        let refreshed = std::fs::read_to_string(&config_path).unwrap();
        assert!(hook_state_is_trusted(&refreshed, &entries[0]).unwrap());
        assert!(hook_state_is_trusted(&refreshed, &entries[1]).unwrap());
        assert_eq!(
            refreshed.matches(&entries[0].key).count(),
            1,
            "refresh must update the owned block rather than append a duplicate"
        );
    }

    #[test]
    fn inline_projects_table_entry_is_not_duplicated() {
        // The shape Codex's own config can take; appending here would produce a
        // duplicate-table parse error for the whole file.
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        let original = format!(
            "[projects]\n\"{}\" = {{ trust_level = \"trusted\" }}\n",
            workdir.to_string_lossy()
        );
        std::fs::write(&config_path, &original).unwrap();

        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert_eq!(outcome, CodexTrustOutcome::AlreadyPresent);
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original);
    }

    #[test]
    fn unparseable_operator_config_is_left_untouched() {
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        let original = "this is [not( toml\n";
        std::fs::write(&config_path, original).unwrap();

        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert!(
            matches!(outcome, CodexTrustOutcome::Skipped(_)),
            "must refuse to modify an unparseable config, got {outcome:?}"
        );
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original);
    }

    #[test]
    fn preserves_an_explicit_untrusted_decision_and_blocks_launch_precondition() {
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        let original = format!(
            "[projects.\"{}\"]\ntrust_level = \"untrusted\"\n",
            workdir.to_string_lossy()
        );
        std::fs::write(&config_path, &original).unwrap();

        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert!(matches!(outcome, CodexTrustOutcome::Skipped(_)));
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original);
    }

    #[test]
    fn relative_workdir_is_skipped() {
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let outcome = ensure_project_trusted_in(&config_path, Path::new("relative/dir")).unwrap();
        assert!(matches!(outcome, CodexTrustOutcome::Skipped(_)));
        assert!(!config_path.exists());
    }

    #[test]
    fn control_character_paths_are_refused() {
        // A newline in a path is legal on Linux and would otherwise inject a
        // whole table (e.g. [mcp_servers.evil]) into the operator's config.
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj\n[mcp_servers.evil]\ncommand = \"sh\"\n");

        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert!(
            matches!(outcome, CodexTrustOutcome::Skipped(_)),
            "must refuse a control-character path, got {outcome:?}"
        );
        assert!(!config_path.exists());
        assert!(path_is_unsafe_key("/a/b\nc"));
        assert!(!path_is_unsafe_key("/a/b c"));
    }

    #[cfg(unix)]
    #[test]
    fn existing_file_mode_is_preserved() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        std::fs::write(&config_path, "model = \"gpt-5\"\n").unwrap();
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).unwrap();

        ensure_project_trusted_in(&config_path, &workdir).unwrap();
        let mode = std::fs::metadata(&config_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "a 0600 config (it can hold API keys) must not widen on rewrite"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_config_keeps_its_link() {
        let dir = TempDir::new();
        let real = dir.join("dotfiles-config.toml");
        let link = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        std::fs::write(&real, "model = \"gpt-5\"\n").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        ensure_project_trusted_in(&link, &workdir).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "a symlinked config.toml must stay a symlink, not be replaced by a regular file"
        );
        assert!(
            config_has_project(
                &std::fs::read_to_string(&real).unwrap(),
                &workdir.to_string_lossy()
            )
            .unwrap(),
            "the entry must land in the symlink target"
        );
    }

    #[test]
    fn codex_home_prefers_env_override() {
        // Env mutation is process-global: this is the only test in the crate
        // that touches CODEX_HOME, and it restores the previous value.
        let previous = std::env::var_os("CODEX_HOME");
        unsafe { std::env::set_var("CODEX_HOME", "/tmp/cas-codex-home-test") };
        assert_eq!(
            codex_home(),
            Some(PathBuf::from("/tmp/cas-codex-home-test"))
        );
        unsafe {
            match previous {
                Some(v) => std::env::set_var("CODEX_HOME", v),
                None => std::env::remove_var("CODEX_HOME"),
            }
        }
    }

    /// Minimal self-cleaning temp dir (cas-pty has no tempfile dev-dependency).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let dir =
                std::env::temp_dir().join(format!("cas-codex-trust-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
