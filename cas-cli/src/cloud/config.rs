//! Cloud configuration management
//!
//! Stores cloud authentication and sync state in `.cas/cloud.json`.
//!
//! # Integration Status
//! Methods ready for cloud sync feature when enabled.

// #![allow(dead_code)] // Check unused

use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use url::Url;

use crate::error::CasError;
use crate::store::find_cas_root;

// `dirs` used by `user_config_path()` / `load_user()` / `save_user()`

/// Cached alias class of the current project (canonical id + the server's
/// `aliases` record as mirrored into `.cas/config.toml`). `None` means "not
/// resolved yet"; an empty `Vec` is a real answer and is cached, because a
/// project with no registered aliases must not re-read config.toml per row.
static CACHED_PROJECT_ALIAS_CLASS: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// Get the canonical project ID for the current Cassy project.
///
/// See [`resolve_canonical_id`] for the full read chain: explicit
/// `config.toml` pin, then the `origin` git remote, then the project folder
/// name, then a path hash.
///
/// Examples:
/// - a checkout of `git@github.com:acme/ledger.git` → `github.com/acme/ledger`
/// - `/home/user/gabber-studio/.cas/` with no git remote → `gabber-studio`
///
/// If the folder name cannot be derived (e.g. `.cas/` lives at the filesystem root
/// and its parent has no file name), falls back to a deterministic `local:<sha256>`
/// hash of the canonicalized project path. This guarantees every valid Cassy project
/// has a stable, unique `project_id` for cloud sync scoping.
///
/// Returns `None` only if not inside a Cassy project directory at all.
/// This compatibility helper resolves the process's current root on every
/// call. Root-owning code must use [`resolve_canonical_id`] with its explicit
/// root instead; a process-wide identity cannot be correct while one process
/// refreshes several projects.
pub fn get_project_canonical_id() -> Option<String> {
    find_cas_root()
        .ok()
        .and_then(|root| resolve_canonical_id(&root))
}

/// Retained for API compatibility. Canonical ids are no longer cached, so
/// there is nothing to invalidate after writing a new project pin.
pub fn invalidate_cached_project_id() {
}

/// Where a resolved canonical id came from. Reported by
/// [`resolve_canonical_id_with_source`] so diagnostics (`cas doctor`) can
/// explain *why* a project maps to the cloud bucket it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalIdSource {
    /// `.cas/config.toml [project] canonical_id` — explicit pin.
    ConfigToml,
    /// Derived from `git remote get-url origin` (cas-f699).
    GitRemote,
    /// Parent-directory folder name.
    FolderName,
    /// `local:<sha256>` hash of the project path.
    PathHash,
}

impl CanonicalIdSource {
    /// Short human label used in diagnostics output.
    pub fn label(self) -> &'static str {
        match self {
            CanonicalIdSource::ConfigToml => "config.toml pin",
            CanonicalIdSource::GitRemote => "git remote origin",
            CanonicalIdSource::FolderName => "folder name",
            CanonicalIdSource::PathHash => "path hash",
        }
    }
}

/// Pure composition of the canonical-id resolution chain.
/// Extracted from `get_project_canonical_id` so the chain is testable without
/// consulting the process's current working directory.
///
/// Resolution order (highest priority first):
///  1. `.cas/config.toml [project] canonical_id` — explicit source of truth,
///     set eagerly by `cas cloud team set` or manually via
///     `cas cloud project set` (cas-1ced).
///  2. `git remote get-url origin`, normalized to `<host>/<owner>/<repo>`
///     (cas-f699). The remote identifies the *repository*, so two unrelated
///     checkouts can never collide, and two clones of the same repo agree.
///  3. Parent-directory folder name — for non-git projects and repos with no
///     `origin` remote. This is the step that used to run second and merged
///     two different repos that happened to share a parent-folder name into
///     one cloud bucket (GH #134).
///  4. Path-hash fallback — for the `.cas/` at filesystem root edge case.
pub fn resolve_canonical_id(cas_root: &Path) -> Option<String> {
    resolve_canonical_id_with_source(cas_root).map(|(id, _)| id)
}

/// Resolve and validate the identity used by a sync rooted at `cas_root`.
///
/// An explicit config pin remains authoritative, including the supported
/// legacy bare-repository alias. A remote-shaped pin for a different
/// repository is an unsafe split-brain configuration: the caller must refuse
/// network sync rather than silently sending rows to the pinned bucket. Bare
/// slug pins are operator-chosen cloud bucket names and must not be compared
/// to the remote repository's final path segment. This check is deliberately
/// rooted in the supplied path and never consults the process cwd.
pub fn resolve_canonical_id_for_sync(cas_root: &Path) -> Result<String, CasError> {
    let resolved = resolve_canonical_id(cas_root).ok_or_else(|| {
        CasError::Other(format!(
            "Cannot sync `{}`: no canonical project id could be resolved",
            cas_root.display()
        ))
    })?;

    if let Some(pin) = canonical_id_from_config_toml(cas_root)
        && pin.matches('/').count() >= 2
        && let Some(remote) = normalized_git_remote_for_push(cas_root)
        && remote != pin
    {
        return Err(CasError::Other(format!(
            "Cannot sync `{}`: resolved identity `{remote}` disagrees with pinned \
             [project] canonical_id `{pin}`; run `cas cloud project set {remote}`",
            cas_root.display()
        )));
    }

    Ok(resolved)
}

/// [`resolve_canonical_id`] plus the step that produced the value.
pub fn resolve_canonical_id_with_source(cas_root: &Path) -> Option<(String, CanonicalIdSource)> {
    if let Some(id) = canonical_id_from_config_toml(cas_root) {
        // A config pin is authoritative. Registration reconciles an unpinned
        // remote-derived id with the server and writes the server-resolved
        // bucket here; rewriting that pin afterwards disconnects the client
        // from legacy bare-slug buckets because team pull matches verbatim.
        return Some((id, CanonicalIdSource::ConfigToml));
    }
    if let Some(id) =
        derive_canonical_id_from_git_remote(cas_root).and_then(|id| canonical_project_id(&id))
    {
        return Some((id, CanonicalIdSource::GitRemote));
    }
    if let Some(id) = canonical_id_from_cas_root(cas_root).and_then(|id| canonical_project_id(&id))
    {
        return Some((id, CanonicalIdSource::FolderName));
    }
    fallback_project_id_from_path(cas_root).map(|id| (id, CanonicalIdSource::PathHash))
}

/// Read `[project] canonical_id` from `<cas_root>/config.toml`. Returns
/// `None` when the file is missing, parse fails, the `[project]` block is
/// absent, or `canonical_id` is unset. This is a best-effort read — any
/// failure falls through to the next resolution step.
pub fn canonical_id_from_config_toml(cas_root: &Path) -> Option<String> {
    let toml_path = cas_root.join("config.toml");
    let content = std::fs::read_to_string(&toml_path).ok()?;
    let parsed: toml::Value = toml::from_str(&content).ok()?;
    parsed
        .get("project")?
        .get("canonical_id")?
        .as_str()
        .and_then(canonical_project_id)
}

/// Resolve one project identity to the single wire form used by registration,
/// push, pull, and local ownership checks. Remote-shaped values are reduced to
/// `host/owner/repository`; host, owner, and legacy bare slugs are folded to
/// lowercase so URL, SSH, and server-slug spellings cannot fork a project.
///
/// This is deliberately the one-argument normalizer used at every cloud
/// boundary. [`canonical_project_id_with_pin`] adds the explicit-pin alias
/// rule for callers that are comparing a stored identity to the current
/// project.
///
/// # Contract with the cloud (GH #669)
///
/// This function is the byte-for-byte twin of the server's
/// `canonicalizeProjectIdentity` (`petra-stella-cloud`
/// `lib/project-identity.ts`) and of its PostgreSQL projection
/// `canonicalProjectIdentitySql`, which is what the alias-merge migration
/// rewrote stored identities with. The steps below are transcribed in the
/// server's order; changing one without the other forks every bucket.
///
/// Two of those steps used to be client-only approximations, and both are now
/// fixed here:
///  - `.git` is stripped **case-insensitively** (`Repo.GIT` is `repo`), and
///  - `.git` is stripped from **every** shape, not only from a recognized
///    remote URL. A bare `gabber-studio.git` and a `git://…/repo.git` (whose
///    scheme deliberately survives, because `git://` is not a recognized
///    transport on either side) both lose the suffix.
pub fn canonical_project_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    // `.replace(/^https?:\/\//i, "")`
    let mut value = strip_prefix_ascii_case_insensitive(trimmed, "https://")
        .or_else(|| strip_prefix_ascii_case_insensitive(trimmed, "http://"))
        .unwrap_or(trimmed)
        .to_string();

    // `.replace(/^ssh:\/\/git@/i, "")`
    if let Some(rest) = strip_prefix_ascii_case_insensitive(&value, "ssh://git@") {
        value = rest.to_string();
    }

    // `.replace(/^git@([^:]+):/i, "$1/")` — the host run is `[^:]+`, so a
    // `git@` with no colon is left alone exactly as the regex leaves it.
    if let Some(rest) = strip_prefix_ascii_case_insensitive(&value, "git@")
        && let Some((host, path)) = rest.split_once(':')
        && !host.is_empty()
    {
        value = format!("{host}/{path}");
    }

    // `.replace(/\/+$/, "").replace(/\.git$/i, "").replace(/\/+$/, "")`
    let trimmed_tail = value.trim_end_matches('/');
    let without_dot_git = if trimmed_tail.len() >= 4
        && trimmed_tail.is_char_boundary(trimmed_tail.len() - 4)
        && trimmed_tail[trimmed_tail.len() - 4..].eq_ignore_ascii_case(".git")
    {
        &trimmed_tail[..trimmed_tail.len() - 4]
    } else {
        trimmed_tail
    };

    // `.replace(/^\/+|\/+$/g, "").toLowerCase()`
    let cleaned = without_dot_git.trim_matches('/').to_ascii_lowercase();
    if cleaned.is_empty() { None } else { Some(cleaned) }
}

/// `str::strip_prefix` that ignores ASCII case, so the server's `/…/i` regex
/// anchors can be transcribed literally.
fn strip_prefix_ascii_case_insensitive<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    if value.len() >= prefix.len()
        && value.is_char_boundary(prefix.len())
        && value[..prefix.len()].eq_ignore_ascii_case(prefix)
    {
        Some(&value[prefix.len()..])
    } else {
        None
    }
}

/// Normalize an identity while treating an explicit project pin as the
/// authoritative alias target. A remote form whose repository name is the
/// pinned bare slug is therefore represented by the pin,
/// preserving the legacy server bucket instead of creating a remote-shaped
/// sibling bucket.
pub fn canonical_project_id_with_pin(value: &str, pinned_id: Option<&str>) -> Option<String> {
    let normalized = canonical_project_id(value)?;
    let Some(pin) = pinned_id.and_then(canonical_project_id) else {
        return Some(normalized);
    };

    if normalized == pin || project_id_aliases(&normalized, &pin) {
        Some(pin)
    } else {
        Some(normalized)
    }
}

/// Compare two project identities after normalization, accepting the legacy
/// bare-repository-name alias in either direction. This is intentionally
/// separate from [`canonical_project_id_with_pin`]: a pin determines which
/// spelling to emit, while ownership checks must recognize both spellings
/// without changing the current project's selected identity.
pub fn project_ids_match(candidate: &str, current: &str) -> bool {
    project_ids_match_with_aliases(candidate, current, &cached_project_alias_class())
}

/// [`project_ids_match`] with the registered alias class supplied explicitly.
///
/// `alias_class` is the complete set of spellings the cloud registry folds into
/// **one** project — its canonical id plus every active row of the server's
/// `project_aliases` record (GH #669). Two identities match when they are both
/// members of that class, which is the only way `ozer-health` and `ozer` can
/// ever be recognized as the same project: no normalizer folds them, because
/// the fold is registry data, not syntax.
///
/// Membership is the *conjunction* of both sides, so an alias record can only
/// ever merge spellings of the project it belongs to. It can never make two
/// different projects match, and an identity the migration deliberately left
/// unmapped (`penguinz`, `pippenz`) stays foreign because it is in no class.
pub fn project_ids_match_with_aliases(
    candidate: &str,
    current: &str,
    alias_class: &[String],
) -> bool {
    let Some(candidate) = canonical_project_id(candidate) else {
        return false;
    };
    let Some(current) = canonical_project_id(current) else {
        return false;
    };

    if candidate == current
        || project_id_aliases(&candidate, &current)
        || project_id_aliases(&current, &candidate)
    {
        return true;
    }

    let in_class = |value: &str| {
        alias_class
            .iter()
            .filter_map(|alias| canonical_project_id(alias))
            .any(|alias| alias == value)
    };
    in_class(&candidate) && in_class(&current)
}

/// Backwards-compatible name retained for cloud callers outside this module.
/// New identity code should call [`canonical_project_id`] directly.
pub fn normalize_project_canonical_id(value: &str) -> Option<String> {
    canonical_project_id(value)
}

fn project_id_aliases(candidate: &str, pinned: &str) -> bool {
    fn final_segment(value: &str) -> &str {
        value.rsplit('/').next().unwrap_or(value)
    }
    let candidate_is_remote = candidate.matches('/').count() >= 2;
    let pinned_is_remote = pinned.matches('/').count() >= 2;

    candidate_is_remote && !pinned_is_remote && final_segment(candidate) == pinned
}

/// Read `[project] aliases` from `<cas_root>/config.toml` — the locally cached
/// copy of the server's per-project `aliases` record (GH #669). Best-effort:
/// a missing file, a parse failure, or a non-array value yields an empty list,
/// which degrades to the pre-#669 syntax-only matching rather than to a wrong
/// attribution.
pub fn project_aliases_from_config_toml(cas_root: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(cas_root.join("config.toml")) else {
        return Vec::new();
    };
    let Ok(parsed) = toml::from_str::<toml::Value>(&content) else {
        return Vec::new();
    };
    parsed
        .get("project")
        .and_then(|project| project.get("aliases"))
        .and_then(toml::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(toml::Value::as_str)
                .filter_map(canonical_project_id)
                .collect()
        })
        .unwrap_or_default()
}

/// Salt same-process temp paths; the PID separates independent MCP servers.
static PROJECT_CONFIG_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

struct ProjectConfigWriteLock {
    file: fs::File,
}

impl Drop for ProjectConfigWriteLock {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            tracing::error!(%error, "failed to release project config write lock");
        }
    }
}

fn lock_project_config(cas_root: &Path) -> Result<ProjectConfigWriteLock, CasError> {
    fs::create_dir_all(cas_root).map_err(|error| {
        CasError::Other(format!(
            "Failed to create project config directory {cas_root:?}: {error}"
        ))
    })?;
    // The lock needs a stable inode of its own. Locking config.toml itself is
    // ineffective after an atomic rename replaces that file's inode.
    let lock_path = cas_root.join(".config.toml.cas-write.lock");
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| {
            CasError::Other(format!(
                "Failed to open project config lock {lock_path:?}: {error}"
            ))
        })?;
    file.lock_exclusive().map_err(|error| {
        CasError::Other(format!(
            "Failed to lock project config {lock_path:?}: {error}"
        ))
    })?;
    Ok(ProjectConfigWriteLock { file })
}

fn atomic_replace_project_config(path: &Path, contents: &str) -> Result<(), CasError> {
    let parent = path.parent().ok_or_else(|| {
        CasError::Other(format!(
            "Cannot atomically write project config without a parent: {path:?}"
        ))
    })?;
    let sequence = PROJECT_CONFIG_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp_path = parent.join(format!(
        ".config.toml.cas-write.{}.{sequence}.tmp",
        std::process::id()
    ));
    atomic_replace_project_config_via(path, contents, &temp_path, |from, to| {
        fs::rename(from, to)
    })
}

/// Write, sync, and atomically rename a same-directory temp into place. Until
/// `commit` succeeds, the existing config is untouched; every error removes
/// only the uniquely-created temp owned by this call.
fn atomic_replace_project_config_via<F>(
    path: &Path,
    contents: &str,
    temp_path: &Path,
    commit: F,
) -> Result<(), CasError>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    let permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let mut temp = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(temp_path)
        .map_err(|error| {
            CasError::Other(format!(
                "Failed to create temporary project config {temp_path:?}: {error}"
            ))
        })?;

    let result = (|| -> std::io::Result<()> {
        temp.write_all(contents.as_bytes())?;
        temp.flush()?;
        if let Some(permissions) = permissions {
            temp.set_permissions(permissions)?;
        }
        temp.sync_all()?;
        drop(temp);
        commit(temp_path, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(temp_path);
        return Err(CasError::Other(format!(
            "Failed to atomically write project config {path:?}: {error}"
        )));
    }
    Ok(())
}

fn update_project_config_toml_with<F, H>(
    cas_root: &Path,
    update: F,
    before_commit: H,
) -> Result<(), CasError>
where
    F: FnOnce(&mut toml::value::Table) -> Result<bool, CasError>,
    H: FnOnce(),
{
    let _lock = lock_project_config(cas_root)?;
    let toml_path = cas_root.join("config.toml");
    let mut doc: toml::Value = match std::fs::read_to_string(&toml_path) {
        Ok(content) => toml::from_str(&content)
            .map_err(|e| CasError::Other(format!("Failed to parse config.toml: {e}")))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            toml::Value::Table(toml::value::Table::new())
        }
        Err(e) => return Err(CasError::Other(format!("Failed to read config.toml: {e}"))),
    };
    let table = doc
        .as_table_mut()
        .ok_or_else(|| CasError::Other("config.toml root is not a table".to_string()))?;
    if !update(table)? {
        return Ok(());
    }

    let serialized = toml::to_string_pretty(&doc)
        .map_err(|e| CasError::Other(format!("Failed to serialize config.toml: {e}")))?;
    before_commit();
    atomic_replace_project_config(&toml_path, &serialized)
}

fn update_project_config_toml<F>(cas_root: &Path, update: F) -> Result<(), CasError>
where
    F: FnOnce(&mut toml::value::Table) -> Result<bool, CasError>,
{
    update_project_config_toml_with(cas_root, update, || {})
}

/// Persist the server's per-project `aliases` record into
/// `<cas_root>/config.toml` as `[project] aliases`.
///
/// Values are canonicalized and de-duplicated on the way in, and the current
/// project's own canonical id is not written back as an alias of itself. Every
/// other `[project]` key (notably the `canonical_id` pin) is preserved.
///
/// Returns the list that was written.
pub fn set_project_aliases_in_config_toml(
    cas_root: &Path,
    aliases: &[String],
) -> Result<Vec<String>, CasError> {
    let own_id = resolve_canonical_id(cas_root);
    let mut normalized: Vec<String> = Vec::new();
    for alias in aliases {
        let Some(alias) = canonical_project_id(alias) else {
            continue;
        };
        if Some(&alias) == own_id.as_ref() || normalized.contains(&alias) {
            continue;
        }
        normalized.push(alias);
    }
    normalized.sort();

    update_project_config_toml(cas_root, |table| {
        let project = table
            .entry("project".to_string())
            .or_insert_with(|| toml::Value::Table(toml::value::Table::new()))
            .as_table_mut()
            .ok_or_else(|| CasError::Other("config.toml [project] is not a table".to_string()))?;
        let aliases = toml::Value::Array(
            normalized
                .iter()
                .map(|alias| toml::Value::String(alias.clone()))
                .collect(),
        );
        if project.get("aliases") == Some(&aliases) {
            return Ok(false);
        }
        project.insert("aliases".to_string(), aliases);
        Ok(true)
    })?;
    invalidate_cached_project_alias_class();
    Ok(normalized)
}

/// The full alias class of the current project: its canonical id plus every
/// alias cached from the server's record. Cached for the process lifetime the
/// same way [`get_project_canonical_id`] is, because `project_ids_match` runs
/// once per pulled row.
fn cached_project_alias_class() -> Vec<String> {
    let mut cached = CACHED_PROJECT_ALIAS_CLASS
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(class) = cached.as_ref() {
        return class.clone();
    }
    let Ok(cas_root) = find_cas_root() else {
        // Outside a project there is nothing to resolve; retry next call rather
        // than caching an empty class for the process lifetime.
        return Vec::new();
    };
    let mut class = Vec::new();
    if let Some(id) = resolve_canonical_id(&cas_root) {
        class.push(id);
    }
    for alias in project_aliases_from_config_toml(&cas_root) {
        if !class.contains(&alias) {
            class.push(alias);
        }
    }
    *cached = Some(class.clone());
    class
}

/// Drop the cached alias class so the next ownership check re-reads
/// `[project] aliases` from disk. Called after a pull refreshes the record.
pub fn invalidate_cached_project_alias_class() {
    let mut cached = CACHED_PROJECT_ALIAS_CLASS
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *cached = None;
}

/// Write `[project] canonical_id = "<value>"` to `<cas_root>/config.toml`,
/// preserving any other existing sections. Read-modify-write via the `toml`
/// crate so prior `[memory]`, `[code_review]`, etc. blocks survive.
///
/// Returns `Err` only on IO or TOML serialization failure. Callers should
/// surface the error — the value did NOT land if this fails.
pub fn set_canonical_id_in_config_toml(
    cas_root: &Path,
    canonical_id: &str,
) -> Result<(), CasError> {
    let canonical_id = canonical_project_id(canonical_id)
        .ok_or_else(|| CasError::Other("canonical project id must not be empty".to_string()))?;
    update_project_config_toml(cas_root, |table| {
        let project = table
            .entry("project".to_string())
            .or_insert_with(|| toml::Value::Table(toml::value::Table::new()))
            .as_table_mut()
            .ok_or_else(|| CasError::Other("config.toml [project] is not a table".to_string()))?;
        let canonical_id = toml::Value::String(canonical_id);
        if project.get("canonical_id") == Some(&canonical_id) {
            return Ok(false);
        }
        project.insert("canonical_id".to_string(), canonical_id);
        Ok(true)
    })
}

/// Derive the canonical project ID from `git -C <cas_root> remote get-url origin`,
/// normalized to `<host>/<owner>/<repo>` form (strips `https?://` / `git@HOST:`
/// prefix and `.git` suffix). Returns `None` when:
///  - git binary isn't available
///  - cas_root isn't a git repo (or has no `origin` remote)
///  - the URL doesn't match a recognizable form
///
/// Used by `cas cloud team set` (cas-1ced) as the second resolution step
/// after `.cas/config.toml`, and — since cas-f699 / GH #134 — by the main
/// [`resolve_canonical_id`] read chain in the same position, ahead of the
/// folder-name fallback.
///
/// Cost note: this spawns `git`. It only runs when `.cas/config.toml` holds no
/// pin. Root-owned sync paths resolve through their explicit root and retain
/// that identity for the operation.
pub fn derive_canonical_id_from_git_remote(cas_root: &Path) -> Option<String> {
    git_origin_url(cas_root).and_then(|raw| normalize_git_remote_url(&raw))
}

/// Read the configured `origin` URL, whether or not it is one of the
/// host/owner/repository forms Cassy can use as a cloud identity.
pub fn git_origin_url(cas_root: &Path) -> Option<String> {
    use std::process::Command;

    let output = Command::new("git")
        .args(["-C"])
        .arg(cas_root)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    (!raw.trim().is_empty()).then(|| raw.trim().to_string())
}

/// Resolve `origin` to the normalized, lowercased wire identity used by both
/// team and personal push requests. The underlying URL normalizer preserves
/// case for response comparison, while the cloud resolver treats remotes
/// case-insensitively on the wire.
pub fn normalized_git_remote_for_push(cas_root: &Path) -> Option<String> {
    derive_canonical_id_from_git_remote(cas_root).and_then(|remote| canonical_project_id(&remote))
}

/// Normalize a git remote URL to `<host>/<owner>/<repo>` form.
///
/// Recognized inputs:
///  - `https://host/owner/repo[.git]` → `host/owner/repo`
///  - `http://host/owner/repo[.git]` → `host/owner/repo`
///  - `ssh://git@host/owner/repo[.git]` → `host/owner/repo`
///  - `git@host:owner/repo[.git]` → `host/owner/repo`
///
/// Returns `None` for anything else (e.g. local file paths, malformed
/// URLs) so the caller can fall through to the next resolution step
/// rather than persist a non-canonical value.
pub fn normalize_git_remote_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }

    // SSH form: `git@host:owner/repo[.git]`. Replace the `:` with `/` after
    // stripping the user prefix so the parse falls through to the generic
    // `host/owner/repo` extractor below.
    let without_ssh_user = if let Some(rest) = trimmed.strip_prefix("git@") {
        // Find the first `:` — that's the separator between host and path.
        let (host, path) = rest.split_once(':')?;
        format!("{host}/{path}")
    } else if let Some(rest) = trimmed.strip_prefix("ssh://git@") {
        // ssh://git@host/path → strip prefix; rest already uses `/`.
        rest.to_string()
    } else if let Some(rest) = trimmed.strip_prefix("https://") {
        rest.to_string()
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        rest.to_string()
    } else {
        return None;
    };

    // Strip optional trailing slash before `.git` so both `repo.git/` and
    // `repo/` converge on the same identity. A second trim handles a slash
    // after the suffix without making the accepted URL shapes order-sensitive.
    let without_trailing_slash = without_ssh_user.trim_end_matches('/');
    let without_dot_git = without_trailing_slash
        .strip_suffix(".git")
        .unwrap_or(without_trailing_slash);
    let clean = without_dot_git.trim_end_matches('/');

    if clean.is_empty() {
        None
    } else {
        Some(clean.to_string())
    }
}

/// Decide whether to adopt the server-returned canonical id after a team push
/// (cas-8ca5 / cloud contract §5). Returns `Some(new_id)` to adopt and persist,
/// `None` to leave the local pin untouched.
///
/// Adopt only when ALL of these hold:
///  - the server returned a non-empty `canonical_id` and `git_remote`,
///  - we have a local git remote,
///  - the local remote equals the returned `git_remote` (case-insensitive —
///    the server lowercases per its `normalizeGitRemote` rule, while our
///    [`normalize_git_remote_url`] preserves the original case),
///  - no explicit config pin exists. A pin is authoritative, whether set by
///    `cas cloud project set` or by verified registration-time adoption.
///
/// The git-remote equality gate is the safety property: it prevents a shared
/// machine whose `origin` differs from the returned project from being silently
/// re-homed onto someone else's canonical id.
pub fn should_adopt_canonical_id(
    local_remote: Option<&str>,
    resp_git_remote: Option<&str>,
    resp_canonical_id: Option<&str>,
    current_pin: Option<&str>,
) -> Option<String> {
    let local = local_remote?.trim();
    let resp_remote = resp_git_remote?.trim();
    let canonical = resp_canonical_id?.trim();
    if local.is_empty() || resp_remote.is_empty() || canonical.is_empty() {
        return None;
    }
    if canonical_project_id(local) != canonical_project_id(resp_remote) {
        return None;
    }
    if current_pin.is_some() {
        return None;
    }
    canonical_project_id(canonical)
}

/// Derive the canonical project ID from a `.cas` directory path.
///
/// The canonical ID is the folder name of the parent directory (the project root).
/// Returns `None` if the path has no parent or no file name (e.g. filesystem root).
pub fn canonical_id_from_cas_root(cas_root: &Path) -> Option<String> {
    let project_dir = cas_root.parent().unwrap_or(cas_root);
    project_dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
}

/// Fallback project ID derived from a deterministic sha256 hash of the canonical
/// project path. Used when `canonical_id_from_cas_root` cannot produce a folder
/// name (e.g. `.cas/` at the filesystem root).
///
/// Format: `local:<first 16 hex chars of sha256(canonical_path)>` — 8 bytes of
/// entropy, more than enough to avoid collisions on a single machine while staying
/// compact in URLs and logs.
///
/// The input is the parent of `cas_root` (the project directory), canonicalized
/// via `std::fs::canonicalize` when possible so symlinked and renamed paths
/// produce the same ID. Falls back to the lexical path if canonicalization fails
/// (e.g. the directory no longer exists on disk — should not happen in practice
/// since we just resolved it via `find_cas_root`, but we stay defensive).
///
/// Returns `None` only if both the canonical and lexical paths fail to produce
/// any bytes to hash — practically unreachable.
pub fn fallback_project_id_from_path(cas_root: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};

    let project_dir = cas_root.parent().unwrap_or(cas_root);
    let canonical =
        std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    let path_bytes = canonical.as_os_str().as_encoded_bytes();
    if path_bytes.is_empty() {
        return None;
    }

    let mut hasher = Sha256::new();
    hasher.update(path_bytes);
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    Some(format!("local:{hex}"))
}

/// One local Cassy project as seen by the collision detector: where it lives,
/// which cloud bucket it resolves to, and the repository identity (`origin`
/// remote) that distinguishes it from an unrelated project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalRootIdentity {
    /// Project root (the directory containing `.cas/`).
    pub project_root: PathBuf,
    /// Result of [`resolve_canonical_id`] for this root.
    pub canonical_id: String,
    /// Normalized `origin` remote, or `None` for a non-git project / a repo
    /// with no `origin`.
    pub git_remote: Option<String>,
}

/// Two or more local roots that resolve to the same cloud bucket while being
/// different repositories — every `cas cloud sync` merges them into each other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalIdCollision {
    /// The shared canonical id (the contaminated bucket).
    pub canonical_id: String,
    /// The colliding project roots, sorted, deduplicated.
    pub roots: Vec<PathBuf>,
}

/// Find canonical-id collisions among known local roots (GH #134, AC2).
///
/// A group of roots sharing a canonical id is only reported when the roots are
/// **different repositories**. Identity is the normalized `origin` remote; a
/// root with no remote is its own identity (keyed on its path), because two
/// unrelated remote-less checkouts that share a folder name are exactly the
/// reported incident.
///
/// This deliberately stays quiet for the benign cases that would otherwise
/// drown the signal: two clones of one repo, and git worktrees of one repo,
/// share an `origin` and therefore *should* share a bucket.
///
/// Pure — the caller supplies the already-resolved roots.
pub fn detect_canonical_id_collisions(roots: &[LocalRootIdentity]) -> Vec<CanonicalIdCollision> {
    use std::collections::{BTreeMap, BTreeSet};

    let mut by_id: BTreeMap<&str, Vec<&LocalRootIdentity>> = BTreeMap::new();
    let mut seen_roots: BTreeSet<&Path> = BTreeSet::new();
    for root in roots {
        // The registry can hold the same path twice (different spellings are
        // canonicalized by the caller); count each root once.
        if !seen_roots.insert(root.project_root.as_path()) {
            continue;
        }
        by_id
            .entry(root.canonical_id.as_str())
            .or_default()
            .push(root);
    }

    by_id
        .into_iter()
        .filter_map(|(canonical_id, group)| {
            if group.len() < 2 {
                return None;
            }
            let identities: BTreeSet<String> = group
                .iter()
                .map(|r| match &r.git_remote {
                    Some(remote) => format!("remote:{remote}"),
                    None => format!("path:{}", r.project_root.display()),
                })
                .collect();
            if identities.len() < 2 {
                return None; // same repository — sharing a bucket is correct
            }
            let mut paths: Vec<PathBuf> = group.iter().map(|r| r.project_root.clone()).collect();
            paths.sort();
            Some(CanonicalIdCollision {
                canonical_id: canonical_id.to_string(),
                roots: paths,
            })
        })
        .collect()
}

/// A team membership entry returned by `/api/me` and cached in `cloud.json`.
///
/// Mirrors the `TeamInfo` shape promised by petra-stella-cloud's `/api/me`
/// response (RESPONSE-user-team-membership-endpoint.md, 2026-05-15).  Fields
/// are stored as `String` rather than typed UUIDs so the struct survives any
/// future backend representation changes without a migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamInfo {
    /// Opaque team UUID (stable primary key used for API calls)
    pub id: String,

    /// URL-safe slug (human-readable, may change on rename)
    pub slug: String,

    /// Display name shown in the CLI
    pub name: String,

    /// Caller's role in this team: `"owner"`, `"admin"`, `"member"`, or `"viewer"`
    pub role: String,
}

/// Cloud configuration stored in .cas/cloud.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudConfig {
    /// Cloud API endpoint
    #[serde(default = "default_endpoint")]
    pub endpoint: String,

    /// API token for authentication
    pub token: Option<String>,

    /// User email
    pub email: Option<String>,

    /// User plan
    pub plan: Option<String>,

    /// Organization ID (for enterprise users)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,

    /// Organization slug (for display)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_slug: Option<String>,

    /// Team ID (for enterprise users)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,

    /// Team slug (for display)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_slug: Option<String>,

    /// Per-team sync timestamps (team_id -> last sync time)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub team_sync_timestamps: HashMap<String, DateTime<Utc>>,

    /// Per-project team memory sync timestamps (canonical_id -> last pull time)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub team_memory_sync_timestamps: HashMap<String, String>,

    /// Last sync timestamp for entries
    pub last_entry_sync: Option<String>,

    /// Last sync timestamp for tasks
    pub last_task_sync: Option<String>,

    /// Last sync timestamp for rules
    pub last_rule_sync: Option<String>,

    /// Last sync timestamp for skills
    pub last_skill_sync: Option<String>,

    /// Whether the factory daemon should spawn its live-stream WebSocket
    /// client (phone-home / relay / pane-watch).
    ///
    /// Default: `false`. The client targets a Phoenix-framework WebSocket
    /// endpoint (`/socket/websocket`) that the current Next.js cloud backend
    /// does not implement and cannot host on Vercel. Leaving the client off
    /// by default avoids the 10-retry 404 storm (~4 min of log noise per
    /// factory session) that cas-4244 documented.
    ///
    /// Re-enable by setting this field to `true` in `.cas/cloud.json` once a
    /// Phoenix-capable backend is reachable (e.g. when the Hetzner Slack
    /// bridge is re-deployed — see `project_claude_code_account_banned`).
    /// The REST-based cloud syncer (`cas-cli/src/cloud/syncer/`) is
    /// independent of this flag and always runs when logged in.
    #[serde(default)]
    pub factory_cloud_client_enabled: bool,

    /// Per-project team auto-promotion control.
    ///
    /// Three states, each with distinct meaning (cas-f8e3):
    ///
    /// - `None` (default) — **personal project**.  The project has no
    ///   explicit team link.  User-level `default_team_id` / single-team
    ///   auto-pick does **not** promote this project to team scope.
    ///   This is the safe default so that personal side-projects are never
    ///   silently promoted just because the user has a team configured
    ///   elsewhere.
    ///
    /// - `Some(true)` — **explicit team opt-in**.  The project intentionally
    ///   inherits the user-level team preference (`default_team_id` →
    ///   single-team auto-pick).  Set this (together with or instead of
    ///   `team_id`) when you want every project on this machine to follow
    ///   the user's default team without running `cas cloud team set` per
    ///   project.
    ///
    /// - `Some(false)` — **hard kill-switch**.  Team auto-promotion is
    ///   disabled even if `team_id` is set; only explicit `--share team`
    ///   writes reach the team queue.
    ///
    /// See `docs/requests/team-memories-filter-policy.md` Decision 3 and
    /// the `active_team_id_with_user_config` resolution chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_auto_promote: Option<bool>,

    /// Team memberships for the authenticated user, fetched from `/api/me`
    /// and cached here so the resolution chain (T3) can work offline.
    ///
    /// Empty by default; populated by T2 (`cas cloud login` + lazy refresh).
    /// Absent in existing `cloud.json` files → deserialises to empty `Vec`
    /// via `#[serde(default)]`.  Not written to disk when empty via
    /// `skip_serializing_if` so pre-T2 files stay clean.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teams: Vec<TeamInfo>,

    /// The team UUID the user has selected as their default scope.
    ///
    /// Populated either from the `default_team_id` field returned by
    /// `/api/me` (if the server already knows a ranking) or by the user
    /// running `cas cloud team default <slug>` (T4).  `None` means no
    /// default has been set; T3's resolution chain falls back to implicit
    /// single-team detection or personal scope.
    ///
    /// Absent in existing `cloud.json` files → deserialises to `None` via
    /// `#[serde(default)]`.  Not written to disk when `None` via
    /// `skip_serializing_if` so pre-T4 files stay clean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_team_id: Option<String>,

    /// UTC timestamp of the last successful `/api/me` fetch that populated
    /// `teams[]`.  Used by T2's staleness check: when `teams` is non-empty
    /// and this timestamp is within 24 h, the lazy refresh in
    /// `execute_sync` is skipped to avoid an extra HTTP round-trip per
    /// sync cycle.
    ///
    /// `None` means teams have never been fetched (triggers refresh on next
    /// sync).  Absent in existing `cloud.json` → `None` via
    /// `#[serde(default)]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teams_fetched_at: Option<DateTime<Utc>>,

    /// Set to `true` after the first-run backfill notice has been shown
    /// (T6), OR when the user explicitly runs `cas cloud team default
    /// --personal`.
    ///
    /// Guards two things at once:
    /// 1. Prevents the one-time notice from firing more than once.
    /// 2. Prevents `maybe_apply_team_backfill` from overriding an explicit
    ///    `--personal` choice (the `--personal` handler sets this flag before
    ///    saving so a later sync never re-promotes the user to team scope).
    ///
    /// Absent in existing `cloud.json` files → `false` via `#[serde(default)]`.
    /// Not written to disk when `false` so pre-T6 files stay clean.
    #[serde(default, skip_serializing_if = "is_false")]
    pub team_backfill_notified: bool,

    /// Set to `true` after this project has shown the one-time notice that it
    /// is syncing to personal scope while the user has a usable team.
    ///
    /// This is project-local. It never changes `team_id`, `team_slug`, or
    /// `team_auto_promote`; it only suppresses repeated informational notices.
    #[serde(default, skip_serializing_if = "is_false")]
    pub personal_scope_notice_shown: bool,
}

/// Display data for the one-time personal-scope notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonalScopeNotice {
    pub team_id: String,
    pub team_slug: String,
    pub team_name: String,
}

impl PersonalScopeNotice {
    pub fn message(&self) -> String {
        format!(
            "This project syncs to personal scope. You're a member of {} ({}). Link it with `cas cloud team set {}` or `cas cloud team auto on`.",
            self.team_name, self.team_slug, self.team_slug
        )
    }
}

/// `skip_serializing_if` predicate for bool fields that default to `false`.
/// Keeps `cloud.json` clean: the field is omitted when it has its zero value.
fn is_false(b: &bool) -> bool {
    !b
}

/// Return true when `url` is a safe endpoint value.
///
/// Accepted: any `https://` URL, or `http://` only when the host is
/// `localhost`, `127.0.0.1`, or `0.0.0.0` (e2e / dev servers).
/// Everything else — `file://`, plain hostnames, arbitrary `http://` — is
/// rejected to prevent an env-var misconfiguration from silently redirecting
/// token exchange to an attacker-controlled server.
pub(crate) fn is_acceptable_endpoint(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    url.starts_with("http://localhost")
        || url.starts_with("http://127.0.0.1")
        || url.starts_with("http://0.0.0.0")
}

pub(crate) fn default_endpoint() -> String {
    std::env::var("CAS_CLOUD_ENDPOINT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .filter(|s| {
            let ok = is_acceptable_endpoint(s);
            if !ok {
                tracing::warn!(
                    endpoint = %s,
                    "CAS_CLOUD_ENDPOINT does not match the allowed scheme; \
                     falling back to default. Allowed: https://* or http://localhost."
                );
            }
            ok
        })
        .unwrap_or_else(|| "https://petra-stella-cloud.vercel.app".to_string())
}

/// Resolve the path to the user-level `~/.cas/cloud.json`.
///
/// In normal operation returns `~/.cas/cloud.json`.
///
/// Test seam: when the `CAS_USER_CLOUD_JSON` environment variable is set to
/// a non-empty value, that path is used instead. This mirrors the
/// `CAS_CLOUD_ENDPOINT` pattern and lets integration tests inject a
/// controlled user-level config without touching the real `~/.cas/`.
pub(crate) fn user_level_cloud_json_path() -> Option<std::path::PathBuf> {
    if let Ok(override_path) = std::env::var("CAS_USER_CLOUD_JSON") {
        if !override_path.trim().is_empty() {
            return Some(std::path::PathBuf::from(override_path));
        }
    }
    dirs::home_dir().map(|h| h.join(".cas").join("cloud.json"))
}

/// Compare two config paths for identity, tolerating different spellings of
/// the same file (`..`, symlinks) when both exist.
fn paths_equal(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Persist a successful login at user level (`~/.cas/cloud.json`) so every
/// project on the machine is logged in (cas-046d / Ben #3).
///
/// The project-level `.cas/cloud.json` is refreshed too when the caller is
/// inside a project, because the MCP daemon and background syncers read that
/// file directly; it is a cache of the user-level truth, and the write is
/// best-effort so `cas login --token` still succeeds from `$HOME` (Ben #4).
///
/// Returns the project path that was also updated, if any.
pub fn store_login_credentials(
    endpoint: &str,
    token: &str,
    email: Option<&str>,
    plan: Option<&str>,
) -> Result<Option<PathBuf>, CasError> {
    let mut user_config = CloudConfig::load_user().unwrap_or_default();
    user_config.endpoint = endpoint.to_string();
    user_config.token = Some(token.to_string());
    user_config.email = email.map(String::from);
    user_config.plan = plan.map(String::from);
    user_config.save_user()?;

    let Ok(project_path) = CloudConfig::config_path() else {
        return Ok(None);
    };
    if user_level_cloud_json_path().is_some_and(|user| paths_equal(&user, &project_path)) {
        return Ok(None);
    }
    let mut project_config = CloudConfig::load_from(&project_path).unwrap_or_default();
    project_config.endpoint = endpoint.to_string();
    project_config.token = Some(token.to_string());
    project_config.email = email.map(String::from);
    project_config.plan = plan.map(String::from);
    match project_config.save_to(&project_path) {
        Ok(()) => Ok(Some(project_path)),
        Err(error) => {
            tracing::debug!(%error, "logged in, but could not cache credentials in the project cloud.json");
            Ok(None)
        }
    }
}

const TEST_FIXTURE_TOKEN: &str = "test-token";
const EPHEMERAL_PORT_START: u16 = 32_768;

/// Test fixtures must never be able to overwrite a real project's cloud cache.
///
/// `CAS_ROOT` is intentionally supported as a test seam, but it can point at
/// an unrelated checkout when a test inherits the worker's environment. The
/// fixture values below are distinctive enough to identify that accidental
/// write while leaving normal production credentials untouched. Temp
/// directories remain valid destinations for integration-test fixtures.
fn reject_fixture_cloud_write(path: &Path, config: &CloudConfig) -> Result<(), CasError> {
    let is_cloud_json = path.file_name().is_some_and(|name| name == "cloud.json");
    let is_fixture = config.token.as_deref() == Some(TEST_FIXTURE_TOKEN)
        || is_loopback_ephemeral_endpoint(&config.endpoint);

    if is_cloud_json && is_fixture && !path_is_under_system_temp(path) {
        return Err(CasError::Other(format!(
            "refusing to write test-fixture cloud.json outside the system temp directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn is_loopback_ephemeral_endpoint(endpoint: &str) -> bool {
    let Ok(url) = Url::parse(endpoint) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let is_loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    let Some(port) = url.port() else {
        return false;
    };

    is_loopback && (port == 0 || port >= EPHEMERAL_PORT_START)
}

fn path_is_under_system_temp(path: &Path) -> bool {
    let Ok(current_dir) = std::env::current_dir() else {
        return false;
    };
    let absolute_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_dir.join(path)
    };
    let temp_dir = std::env::temp_dir();
    let Ok(canonical_temp_dir) = temp_dir.canonicalize() else {
        return false;
    };
    let canonical_path = match fs::symlink_metadata(&absolute_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => None,
        Ok(_) => absolute_path.canonicalize().ok(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => absolute_path
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .and_then(|parent| absolute_path.file_name().map(|name| parent.join(name))),
        Err(_) => None,
    };

    canonical_path.is_some_and(|candidate| candidate.starts_with(canonical_temp_dir))
}

/// Clear credentials from `~/.cas/cloud.json` and, when inside a project, from
/// that project's cached copy. Non-credential state (teams, sync timestamps)
/// is preserved.
pub fn clear_login_credentials() -> Result<(), CasError> {
    let mut user_config = CloudConfig::load_user().unwrap_or_default();
    user_config.logout();
    user_config.save_user()?;

    if let Ok(project_path) = CloudConfig::config_path()
        && !user_level_cloud_json_path().is_some_and(|user| paths_equal(&user, &project_path))
        && project_path.exists()
        && let Ok(mut project_config) = CloudConfig::load_from(&project_path)
    {
        project_config.logout();
        if let Err(error) = project_config.save_to(&project_path) {
            tracing::debug!(%error, "could not clear project-level cloud credentials");
        }
    }
    Ok(())
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            endpoint: default_endpoint(),
            token: None,
            email: None,
            plan: None,
            org_id: None,
            org_slug: None,
            team_id: None,
            team_slug: None,
            team_sync_timestamps: HashMap::new(),
            team_memory_sync_timestamps: HashMap::new(),
            last_entry_sync: None,
            last_task_sync: None,
            last_rule_sync: None,
            last_skill_sync: None,
            factory_cloud_client_enabled: false,
            team_auto_promote: None,
            teams: Vec::new(),
            default_team_id: None,
            teams_fetched_at: None,
            team_backfill_notified: false,
            personal_scope_notice_shown: false,
        }
    }
}

fn usable_team_from_user_config(user_cfg: &CloudConfig) -> Option<PersonalScopeNotice> {
    if let Some(default_team_id) = user_cfg.default_team_id.as_deref() {
        if let Some(team) = user_cfg.teams.iter().find(|t| t.id == default_team_id) {
            return Some(PersonalScopeNotice {
                team_id: team.id.clone(),
                team_slug: team.slug.clone(),
                team_name: team.name.clone(),
            });
        }
        return Some(PersonalScopeNotice {
            team_id: default_team_id.to_string(),
            team_slug: default_team_id.to_string(),
            team_name: default_team_id.to_string(),
        });
    }

    match user_cfg.teams.as_slice() {
        [team] => Some(PersonalScopeNotice {
            team_id: team.id.clone(),
            team_slug: team.slug.clone(),
            team_name: team.name.clone(),
        }),
        _ => None,
    }
}

/// Return the personal-scope notice data when a project is currently personal
/// but the user has a usable team. Pure helper for unit tests.
pub fn personal_scope_notice_for_configs(
    project_cfg: &CloudConfig,
    user_cfg: &CloudConfig,
) -> Option<PersonalScopeNotice> {
    if project_cfg.personal_scope_notice_shown {
        return None;
    }
    if project_cfg
        .active_team_id_with_user_config(Some(user_cfg))
        .is_some()
    {
        return None;
    }
    usable_team_from_user_config(user_cfg)
}

/// Mark and return the one-time personal-scope notice for this project.
///
/// This intentionally never mutates team scope. The only persisted change is
/// `personal_scope_notice_shown = true` in project `.cas/cloud.json`.
pub fn maybe_mark_personal_scope_notice(
    cas_root: &Path,
) -> Result<Option<PersonalScopeNotice>, CasError> {
    maybe_mark_personal_scope_notice_with_hook(cas_root, || {})
}

fn maybe_mark_personal_scope_notice_with_hook<F>(
    cas_root: &Path,
    before_mark: F,
) -> Result<Option<PersonalScopeNotice>, CasError>
where
    F: FnOnce(),
{
    let project_cfg = CloudConfig::load_from_cas_dir(cas_root)?;
    let user_cfg = user_level_cloud_json_path()
        .and_then(|p| CloudConfig::load_from(&p).ok())
        .unwrap_or_default();

    if personal_scope_notice_for_configs(&project_cfg, &user_cfg).is_none() {
        return Ok(None);
    }

    before_mark();

    let mut fresh_project_cfg = CloudConfig::load_from_cas_dir(cas_root)?;
    let notice = personal_scope_notice_for_configs(&fresh_project_cfg, &user_cfg);
    if notice.is_some() {
        fresh_project_cfg.personal_scope_notice_shown = true;
        fresh_project_cfg.save_to_cas_dir(cas_root)?;
    }
    Ok(notice)
}

/// Outcome of the automatic team-scope adoption `cas cloud sync` performs
/// (cas-c117, operator directive 2026-08-18).
///
/// # Why this exists
///
/// The team identity is already known locally — `/api/me` populates
/// `teams[]` and `maybe_apply_team_backfill` sets `default_team_id` — but
/// [`CloudConfig::active_team_id_with_user_config`] refuses to use it unless
/// the project opted in with `cas cloud team set` or `cas cloud team auto on`
/// (the cas-f8e3 guard). A new clone therefore synced in personal scope and
/// registered nothing with the team until the user ran a command they had no
/// reason to know about. Sync now adopts the resolvable team for the project
/// itself; explicit configuration remains an override, not a prerequisite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeamScopeAdoption {
    /// The project had no team scope and one team resolved — the project is
    /// now opted in (`team_auto_promote = Some(true)`).
    Adopted(PersonalScopeNotice),
    /// The project already resolves a team (explicit `team_id` or a prior
    /// adoption). Nothing changed.
    AlreadyScoped { team_id: String },
    /// `cas cloud team auto off` — the hard kill-switch. Never re-adopted.
    OptedOut,
    /// No token in this project's cloud config; nothing can be resolved.
    NotLoggedIn,
    /// No single team identity resolves. `membership_count` is 0 (no teams)
    /// or ≥ 2 with no user-level default (genuinely ambiguous — Cassy refuses
    /// to guess which team a project belongs to).
    NoResolvableTeam { membership_count: usize },
}

/// Adopt the user's resolvable team for this project. Pure — mutates
/// `project_cfg` in memory only, so the decision table is unit-testable.
///
/// Precedence, highest first:
///  1. not logged in → nothing to resolve;
///  2. `team_auto_promote = Some(false)` → user said personal, forever;
///  3. project already resolves a team → leave it alone (this is what makes
///     `cas cloud team set` an override rather than a competitor);
///  4. exactly one resolvable team (user `default_team_id`, else a sole
///     membership) → opt the project in;
///  5. otherwise → stay personal and report why.
///
/// "Logged in" is machine-wide (cas-046d): a project whose own `cloud.json`
/// has no token still counts when `~/.cas/cloud.json` does, because that is
/// exactly the fresh-clone case this adoption exists for — checking only the
/// project copy would make adoption miss the users who need it most.
pub fn adopt_team_scope_for_configs(
    project_cfg: &mut CloudConfig,
    user_cfg: &CloudConfig,
) -> TeamScopeAdoption {
    if !project_cfg.is_logged_in() && !user_cfg.is_logged_in() {
        return TeamScopeAdoption::NotLoggedIn;
    }
    if matches!(project_cfg.team_auto_promote, Some(false)) {
        return TeamScopeAdoption::OptedOut;
    }
    if let Some(team_id) = project_cfg.active_team_id_with_user_config(Some(user_cfg)) {
        return TeamScopeAdoption::AlreadyScoped { team_id };
    }

    match usable_team_from_user_config(user_cfg) {
        Some(team) => {
            // The same knob `cas cloud team auto on` sets — so adoption is
            // indistinguishable from the user having opted in by hand, and
            // `cas cloud team auto off` reverses it.
            project_cfg.team_auto_promote = Some(true);
            TeamScopeAdoption::Adopted(team)
        }
        None => TeamScopeAdoption::NoResolvableTeam {
            membership_count: user_cfg.teams.len(),
        },
    }
}

/// Disk-backed [`adopt_team_scope_for_configs`]: reads the project config at
/// `cas_root` and the user-level config, and persists the project config only
/// when adoption actually changed it.
pub fn maybe_adopt_team_scope(cas_root: &Path) -> Result<TeamScopeAdoption, CasError> {
    let mut project_cfg = CloudConfig::load_from_cas_dir(cas_root)?;
    let user_cfg = user_level_cloud_json_path()
        .and_then(|p| CloudConfig::load_from(&p).ok())
        .unwrap_or_default();

    let outcome = adopt_team_scope_for_configs(&mut project_cfg, &user_cfg);
    if matches!(outcome, TeamScopeAdoption::Adopted(_)) {
        project_cfg.save_to_cas_dir(cas_root)?;
    }
    Ok(outcome)
}

impl CloudConfig {
    /// Return the path to the user-level `~/.cas/cloud.json`.
    ///
    /// Delegates to [`user_level_cloud_json_path`] so reads (`load_user`) and
    /// writes (`save_user`) always agree, including under the
    /// `CAS_USER_CLOUD_JSON` test seam.
    ///
    /// Returns `None` only when `dirs::home_dir()` fails — practically
    /// unreachable on any supported platform (Linux/macOS).
    pub fn user_config_path() -> Option<PathBuf> {
        user_level_cloud_json_path()
    }

    /// Load the user-level cloud config from `~/.cas/cloud.json`.
    ///
    /// Falls back to `Default::default()` when the file is absent — identical
    /// semantics to `load_from` for a missing file.  This is the user-scope
    /// counterpart to `load()` (project scope).
    pub fn load_user() -> Result<Self, CasError> {
        match user_level_cloud_json_path() {
            Some(path) => Self::load_from(&path),
            None => Ok(Self::default()),
        }
    }

    /// Save the user-level cloud config to `~/.cas/cloud.json`.
    ///
    /// Creates `~/.cas/` if it does not already exist.  This is the
    /// user-scope counterpart to `save()` (project scope).
    pub fn save_user(&self) -> Result<(), CasError> {
        let path = Self::user_config_path()
            .ok_or_else(|| CasError::Other("Cannot determine home directory".to_string()))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        self.save_to(&path)
    }

    /// Load cloud config from `.cas/cloud.json` for the current project,
    /// inheriting the machine-wide login from `~/.cas/cloud.json`.
    ///
    /// Credentials are user-level (cas-046d): `cas login` stores the token in
    /// `~/.cas/cloud.json`, so a project that has never been logged in to picks
    /// it up here instead of reporting "not logged in". When the inheritance
    /// fires inside a real project the credentials are also written through to
    /// the project file, so the direct-file readers
    /// ([`load_from_cas_dir`][Self::load_from_cas_dir] — the MCP daemon and
    /// background syncers) converge without a second `cas login`. That
    /// write-through is best-effort: a read-only checkout still returns the
    /// inherited credentials in memory.
    pub fn load() -> Result<Self, CasError> {
        let path = Self::config_path()?;
        let mut config = Self::load_from(&path)?;
        if config.inherit_credentials_from_user_level(&path) {
            // Cache fill only — never fatal.
            if let Err(error) = config.save_to(&path) {
                tracing::debug!(%error, "could not cache user-level credentials into project cloud.json");
            }
        }
        Ok(config)
    }

    /// The config governing the current context, never failing.
    ///
    /// Inside a Cassy project this is [`load`][Self::load]; outside one (for
    /// example `cas login --token` run from `$HOME`) it is the user-level
    /// `~/.cas/cloud.json` alone. Auth commands use this so they work
    /// everywhere: credentials do not live in a project.
    pub fn load_effective() -> Self {
        match Self::load() {
            Ok(config) => config,
            Err(_) => Self::load_user().unwrap_or_default(),
        }
    }

    /// Copy the machine-wide login into this (project) config when the project
    /// has none of its own. Returns whether anything changed.
    ///
    /// `self_path` is the file `self` was read from; when it *is* the
    /// user-level file there is nothing to inherit.
    fn inherit_credentials_from_user_level(&mut self, self_path: &Path) -> bool {
        let Some(user_path) = user_level_cloud_json_path() else {
            return false;
        };
        if paths_equal(&user_path, self_path) {
            return false;
        }
        let Ok(user_config) = Self::load_from(&user_path) else {
            return false;
        };
        self.inherit_credentials_from(&user_config)
    }

    /// Adopt `user`'s credentials when `self` is not logged in. Returns whether
    /// anything changed. Pure — unit-testable without touching disk.
    pub fn inherit_credentials_from(&mut self, user: &Self) -> bool {
        if self.is_logged_in() || !user.is_logged_in() {
            return false;
        }
        self.token = user.token.clone();
        if self.email.is_none() {
            self.email = user.email.clone();
        }
        if self.plan.is_none() {
            self.plan = user.plan.clone();
        }
        // Only override an endpoint the project never chose for itself.
        if (self.endpoint.trim().is_empty() || self.endpoint == default_endpoint())
            && !user.endpoint.trim().is_empty()
        {
            self.endpoint = user.endpoint.clone();
        }
        true
    }

    /// Load cloud config from a specific path
    pub fn load_from(path: &Path) -> Result<Self, CasError> {
        if path.exists() {
            let content = fs::read_to_string(path)?;
            let config: Self = serde_json::from_str(&content)
                .map_err(|e| CasError::Other(format!("Failed to parse cloud config: {e}")))?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// Load cloud config from a specific cas directory
    pub fn load_from_cas_dir(cas_dir: &Path) -> Result<Self, CasError> {
        let path = cas_dir.join("cloud.json");
        Self::load_from(&path)
    }

    /// Load a project's cloud config and apply the machine-wide login when the
    /// project has no local credentials.
    ///
    /// Unlike [`Self::load`], this is rooted at an explicit `.cas` directory.
    /// Cross-project callers must use it instead of consulting the process
    /// working directory (which may be a factory worktree for another
    /// project). As with `load`, caching inherited credentials is best-effort.
    pub fn load_from_cas_dir_inheriting_user_credentials(cas_dir: &Path) -> Result<Self, CasError> {
        let mut config = Self::load_from_cas_dir(cas_dir)?;
        let changed = Self::load_user()
            .map(|user| config.inherit_credentials_from(&user))
            .unwrap_or(false);
        if changed {
            if let Err(error) = config.save_to_cas_dir(cas_dir) {
                tracing::debug!(%error, cas_dir = %cas_dir.display(), "could not cache inherited user cloud credentials");
            }
        }
        Ok(config)
    }

    /// Save cloud config to .cas/cloud.json
    pub fn save(&self) -> Result<(), CasError> {
        let path = Self::config_path()?;
        self.save_to(&path)
    }

    /// Save cloud config to a specific path
    pub fn save_to(&self, path: &Path) -> Result<(), CasError> {
        reject_fixture_cloud_write(path, self)?;
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| CasError::Other(format!("Failed to serialize cloud config: {e}")))?;
        fs::write(path, content)?;
        Ok(())
    }

    /// Save cloud config to a specific cas directory
    pub fn save_to_cas_dir(&self, cas_dir: &Path) -> Result<(), CasError> {
        let path = cas_dir.join("cloud.json");
        self.save_to(&path)
    }

    /// Get the path to cloud.json
    pub fn config_path() -> Result<PathBuf, CasError> {
        let cas_root = find_cas_root()?;
        Ok(cas_root.join("cloud.json"))
    }

    /// Check if user is logged in (has a valid token)
    pub fn is_logged_in(&self) -> bool {
        self.token.as_ref().is_some_and(|t| !t.is_empty())
    }

    /// Clear authentication (logout)
    pub fn logout(&mut self) {
        self.token = None;
        self.email = None;
        self.plan = None;
        self.org_id = None;
        self.org_slug = None;
        self.team_id = None;
        self.team_slug = None;
    }

    /// Check if user belongs to an organization
    pub fn has_org(&self) -> bool {
        self.org_id.is_some()
    }

    /// Check if user belongs to a team
    pub fn has_team(&self) -> bool {
        self.team_id.is_some()
    }

    /// Core resolution logic for `active_team_id`, split out so unit tests
    /// can inject a controlled user-level config without touching disk.
    ///
    /// Resolution chain (highest priority first):
    ///
    /// 0. **Kill-switch**: `team_auto_promote = Some(false)` → always `None`,
    ///    even if `team_id` is set.
    ///
    /// 1. **Project-level explicit link**: `self.team_id` if `Some` → use it.
    ///    The project was explicitly linked to this team via `cas cloud team
    ///    set`; no further steps needed.
    ///
    /// 1.5. **Explicit opt-in guard** (cas-f8e3): if `team_auto_promote` is
    ///    NOT `Some(true)` at this point, return `None` — the project has no
    ///    explicit team link and has not opted in to user-level auto-promotion.
    ///    Personal projects (no `team_id`, no `team_auto_promote = Some(true)`)
    ///    MUST NOT inherit the user's default team, because that would silently
    ///    promote every Cassy workspace the user touches to team scope, including
    ///    private personal side-projects.
    ///
    /// 2. `user_cfg.default_team_id` if `Some` → user's preferred team.
    ///    Only reached when Step 1.5 passed (i.e. `team_auto_promote = Some(true)`).
    ///
    /// 3. `user_cfg.teams.len() == 1` → implicit single-team auto-pick.
    ///    Only reached when Step 1.5 passed.
    ///
    /// 4. `None` — ambiguous (0 or 2+ teams) or no user config at all.
    pub fn active_team_id_with_user_config(
        &self,
        user_cfg: Option<&CloudConfig>,
    ) -> Option<String> {
        // Step 0 — hard kill-switch.
        if matches!(self.team_auto_promote, Some(false)) {
            return None;
        }
        // Step 1 — project-level explicit link wins.
        if let Some(ref tid) = self.team_id {
            return Some(tid.clone());
        }
        // Step 1.5 — explicit opt-in guard (cas-f8e3).
        //
        // Without `team_id` (Step 1) or `team_auto_promote = Some(true)`, this
        // project is personal.  User-level `default_team_id` / single-team
        // auto-pick must NOT apply, otherwise personal workspaces would be
        // silently promoted to team scope whenever the user has a team
        // configured for their main project.
        //
        // cas-c117 — CONSTRAINT ON THIS GUARD, read before "simplifying" it:
        // the operator reversed the *default* (a logged-in user's project must
        // land in their team without them discovering `cas cloud team set`),
        // but deliberately NOT this guard. `cas cloud sync` calls
        // [`adopt_team_scope_for_configs`], which writes
        // `team_auto_promote = Some(true)` into the project config and prints
        // what it did, so the opt-in still exists as a durable, inspectable,
        // reversible fact on disk. Do not "fix" the UX by making this function
        // fall through to the user-level default on its own: that would make
        // team scope an invisible property of the ambient environment, apply
        // to every non-sync caller (stores, MCP, daemon) with no notice and no
        // record, and remove the `team auto off` kill switch's only anchor.
        if !matches!(self.team_auto_promote, Some(true)) {
            return None;
        }
        // Steps 2–4 — user-level fallback (only reached with opt-in).
        if let Some(user) = user_cfg {
            // Step 2 — user has a default team preference.
            if let Some(ref dtid) = user.default_team_id {
                return Some(dtid.clone());
            }
            // Step 3 — implicit single-team auto-pick.
            if user.teams.len() == 1 {
                return Some(user.teams[0].id.clone());
            }
        }
        // Step 4 — ambiguous or no membership.
        None
    }

    /// Return the team UUID to auto-promote writes to, or `None` if this
    /// project is personal / not explicitly team-linked.
    ///
    /// A project is team-linked when ANY of the following hold:
    ///  - `self.team_id` is set (project was explicitly linked via
    ///    `cas cloud team set`), OR
    ///  - `team_auto_promote = Some(true)` (project opted in to inheriting
    ///    the user-level team preference from `~/.cas/cloud.json`).
    ///
    /// A project WITHOUT `team_id` AND WITHOUT `team_auto_promote = Some(true)`
    /// is always personal — the user-level `default_team_id` / single-team
    /// auto-pick does NOT apply (cas-f8e3 guard).  This prevents personal
    /// side-projects from being silently promoted to team scope.
    ///
    /// The hard kill-switch `team_auto_promote = Some(false)` blocks promotion
    /// even when `team_id` is set.
    ///
    /// For unit-testable access without disk I/O, use
    /// [`active_team_id_with_user_config`][Self::active_team_id_with_user_config]
    /// directly.
    pub fn active_team_id(&self) -> Option<String> {
        let user_cfg = user_level_cloud_json_path().and_then(|p| CloudConfig::load_from(&p).ok());
        self.active_team_id_with_user_config(user_cfg.as_ref())
    }

    /// Set the current team context
    pub fn set_team(&mut self, team_id: &str, team_slug: &str) {
        self.team_id = Some(team_id.to_string());
        self.team_slug = Some(team_slug.to_string());
    }

    /// Clear the current team context, making the project personal.
    ///
    /// Clears `team_id`, `team_slug`, and any explicit `team_auto_promote`
    /// opt-in (`Some(true)`) so that Steps 2/3 of `active_team_id` do not
    /// fire after the clear.  The hard kill-switch (`Some(false)`) is
    /// preserved so an intentional "never team-promote" override survives a
    /// `team clear` command.
    pub fn clear_team(&mut self) {
        self.team_id = None;
        self.team_slug = None;
        // Reset explicit opt-in so the project defaults to personal under the
        // cas-f8e3 guard.  The hard kill-switch (Some(false)) is preserved.
        if matches!(self.team_auto_promote, Some(true)) {
            self.team_auto_promote = None;
        }
    }

    /// Get the last sync timestamp for a specific team
    pub fn get_team_sync_timestamp(&self, team_id: &str) -> Option<DateTime<Utc>> {
        self.team_sync_timestamps.get(team_id).copied()
    }

    /// Set the last sync timestamp for a specific team
    pub fn set_team_sync_timestamp(&mut self, team_id: &str, ts: DateTime<Utc>) {
        self.team_sync_timestamps.insert(team_id.to_string(), ts);
    }

    /// Clear the sync timestamp for a specific team
    pub fn clear_team_sync_timestamp(&mut self, team_id: &str) {
        self.team_sync_timestamps.remove(team_id);
    }

    /// Get the last team memory sync timestamp for a project
    pub fn get_team_memory_sync(&self, canonical_id: &str) -> Option<&str> {
        self.team_memory_sync_timestamps
            .get(canonical_id)
            .map(|s| s.as_str())
    }

    /// Set the last team memory sync timestamp for a project
    pub fn set_team_memory_sync(&mut self, canonical_id: &str, timestamp: &str) {
        self.team_memory_sync_timestamps
            .insert(canonical_id.to_string(), timestamp.to_string());
    }
}

#[cfg(test)]
mod tests {
    use crate::cloud::config::*;
    use crate::test_support::TestEnvGuard;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    #[test]
    fn test_default_config() {
        let _guard = TestEnvGuard::new();
        let config = CloudConfig::default();
        assert_eq!(config.endpoint, "https://petra-stella-cloud.vercel.app");
        assert!(config.token.is_none());
        assert!(!config.is_logged_in());
    }

    #[test]
    fn test_save_and_load() {
        let guard = TestEnvGuard::temp_home();
        let path = guard.home().join("cloud.json");

        let config = CloudConfig {
            token: Some("test_token".to_string()),
            email: Some("test@example.com".to_string()),
            ..Default::default()
        };

        config.save_to(&path).unwrap();

        let loaded = CloudConfig::load_from(&path).unwrap();
        assert_eq!(loaded.token, Some("test_token".to_string()));
        assert_eq!(loaded.email, Some("test@example.com".to_string()));
        assert!(loaded.is_logged_in());
    }

    /// Wire up a fake machine: a user-level `~/.cas/cloud.json` (via the
    /// `CAS_USER_CLOUD_JSON` seam) and a project `.cas/` (via `CAS_ROOT`).
    /// Returns the guard plus both paths.
    fn machine_fixture() -> (TestEnvGuard, TempDir, PathBuf, PathBuf) {
        let guard = TestEnvGuard::temp_home();
        let temp = tempfile::Builder::new()
            .prefix("cas-cloud-fixture-")
            .tempdir_in(guard.home())
            .unwrap();
        let user_path = temp.path().join("home-cas").join("cloud.json");
        std::fs::create_dir_all(user_path.parent().unwrap()).unwrap();
        let project_cas = temp.path().join("project-b").join(".cas");
        std::fs::create_dir_all(&project_cas).unwrap();

        let mut guard = guard;
        guard.set("CAS_USER_CLOUD_JSON", &user_path);
        guard.set("CAS_ROOT", &project_cas);
        let project_path = project_cas.join("cloud.json");
        (guard, temp, user_path, project_path)
    }

    /// Create a disposable project outside the runtime's system temp root.
    ///
    /// Test archives may be built on one machine and executed on another, so
    /// compile-time paths such as `CARGO_MANIFEST_DIR` are not valid fixture
    /// locations. Prefer the runtime working directory, then the runtime home.
    fn project_fixture_outside_system_temp() -> TempDir {
        [std::env::current_dir().ok(), dirs::home_dir()]
            .into_iter()
            .flatten()
            .filter(|parent| !path_is_under_system_temp(parent))
            .find_map(|parent| {
                tempfile::Builder::new()
                    .prefix("cas-11f9-project-")
                    .tempdir_in(parent)
                    .ok()
            })
            .expect("test requires a writable runtime directory outside the system temp root")
    }

    #[test]
    fn login_is_machine_wide_so_a_second_project_is_already_logged_in() {
        // Ben #3 (cas-046d): after `cas login` in one project, a freshly
        // `cas init`-ed second project reported "not logged in".
        let (_guard, _temp, user_path, project_path) = machine_fixture();
        CloudConfig {
            token: Some("user-level-token".to_string()),
            email: Some("ben@example.com".to_string()),
            ..Default::default()
        }
        .save_to(&user_path)
        .unwrap();
        assert!(
            !project_path.exists(),
            "second project has never been logged in to"
        );

        let loaded = CloudConfig::load().unwrap();

        assert!(
            loaded.is_logged_in(),
            "a machine-wide login must serve every project"
        );
        assert_eq!(loaded.token.as_deref(), Some("user-level-token"));
        assert_eq!(loaded.email.as_deref(), Some("ben@example.com"));

        // Write-through so the direct-file readers (MCP daemon, syncers)
        // converge without a second login.
        let cached = CloudConfig::load_from(&project_path).unwrap();
        assert_eq!(cached.token.as_deref(), Some("user-level-token"));
    }

    #[test]
    fn project_credentials_win_over_the_user_level_login() {
        let (_guard, _temp, user_path, project_path) = machine_fixture();
        CloudConfig {
            token: Some("user-level-token".to_string()),
            ..Default::default()
        }
        .save_to(&user_path)
        .unwrap();
        CloudConfig {
            token: Some("project-token".to_string()),
            ..Default::default()
        }
        .save_to(&project_path)
        .unwrap();

        let loaded = CloudConfig::load().unwrap();

        assert_eq!(
            loaded.token.as_deref(),
            Some("project-token"),
            "an explicit project credential must not be overwritten"
        );
    }

    #[test]
    fn explicit_project_team_survives_explicit_root_credential_inheritance() {
        let (_guard, _temp, user_path, project_path) = machine_fixture();
        CloudConfig {
            token: Some("user-level-token".to_string()),
            ..Default::default()
        }
        .save_to(&user_path)
        .unwrap();
        CloudConfig {
            team_id: Some("explicit-team-id".to_string()),
            team_slug: Some("explicit-team".to_string()),
            ..Default::default()
        }
        .save_to(&project_path)
        .unwrap();

        let loaded = CloudConfig::load_from_cas_dir_inheriting_user_credentials(
            project_path.parent().unwrap(),
        )
        .unwrap();

        assert_eq!(loaded.token.as_deref(), Some("user-level-token"));
        assert_eq!(loaded.team_id.as_deref(), Some("explicit-team-id"));
        assert_eq!(loaded.team_slug.as_deref(), Some("explicit-team"));
    }

    #[test]
    fn inheritance_keeps_an_endpoint_the_project_chose_for_itself() {
        let mut project = CloudConfig {
            endpoint: "https://staging.example.com".to_string(),
            ..Default::default()
        };
        let user = CloudConfig {
            endpoint: "https://petra-stella-cloud.vercel.app".to_string(),
            token: Some("t".to_string()),
            ..Default::default()
        };

        assert!(project.inherit_credentials_from(&user));

        assert_eq!(project.token.as_deref(), Some("t"));
        assert_eq!(
            project.endpoint, "https://staging.example.com",
            "a non-default project endpoint survives credential inheritance"
        );
    }

    #[test]
    fn store_login_credentials_writes_user_level_and_project_cache() {
        let (_guard, _temp, user_path, project_path) = machine_fixture();

        let cached = store_login_credentials(
            "https://petra-stella-cloud.vercel.app",
            "fresh-token",
            Some("ben@example.com"),
            Some("pro"),
        )
        .unwrap();

        assert_eq!(cached.as_deref(), Some(project_path.as_path()));
        let user = CloudConfig::load_from(&user_path).unwrap();
        assert_eq!(
            user.token.as_deref(),
            Some("fresh-token"),
            "the login must land in ~/.cas/cloud.json"
        );
        assert_eq!(user.email.as_deref(), Some("ben@example.com"));
        assert_eq!(user.plan.as_deref(), Some("pro"));
        let project = CloudConfig::load_from(&project_path).unwrap();
        assert_eq!(project.token.as_deref(), Some("fresh-token"));
    }

    #[test]
    fn test_fixture_login_does_not_write_cache_to_cas_root_project() {
        // Reproduce the incident's shape without touching a real project:
        // CAS_ROOT points at a project outside the system temp directory while
        // the user-level path is safely injected into TestEnvGuard's temp HOME.
        let mut guard = TestEnvGuard::temp_home();
        let user_path = guard.home().join(".cas/cloud.json");
        std::fs::create_dir_all(user_path.parent().unwrap()).unwrap();

        let project = project_fixture_outside_system_temp();
        let project_cas = project.path().join(".cas");
        std::fs::create_dir_all(&project_cas).unwrap();
        let project_cloud = project_cas.join("cloud.json");
        std::fs::write(&project_cloud, b"{\"token\":\"real-project-token\"}\n").unwrap();
        let before = std::fs::read(&project_cloud).unwrap();

        guard.set("CAS_USER_CLOUD_JSON", &user_path);
        guard.set("CAS_ROOT", &project_cas);

        let cached =
            store_login_credentials("http://127.0.0.1:33749", "test-token", None, None).unwrap();

        assert!(cached.is_none(), "fixture cache writes must be rejected");
        assert_eq!(std::fs::read(&project_cloud).unwrap(), before);
    }

    #[test]
    fn test_fixture_cloud_write_is_rejected_outside_temp_directory() {
        let project = project_fixture_outside_system_temp();
        let cloud_path = project.path().join(".cas/cloud.json");
        std::fs::create_dir_all(cloud_path.parent().unwrap()).unwrap();
        let config = CloudConfig {
            endpoint: "https://petra-stella-cloud.vercel.app".to_string(),
            token: Some(TEST_FIXTURE_TOKEN.to_string()),
            ..Default::default()
        };

        let error = config.save_to(&cloud_path).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("outside the system temp directory")
        );
        assert!(!cloud_path.exists());
    }

    #[test]
    fn loopback_ephemeral_fixture_cloud_write_is_rejected_outside_temp_directory() {
        let project = project_fixture_outside_system_temp();
        let cloud_path = project.path().join(".cas/cloud.json");
        std::fs::create_dir_all(cloud_path.parent().unwrap()).unwrap();
        let config = CloudConfig {
            endpoint: "http://127.0.0.1:33749".to_string(),
            token: Some("non-fixture-token".to_string()),
            ..Default::default()
        };

        let error = config.save_to(&cloud_path).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("outside the system temp directory")
        );
        assert!(!cloud_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_fixture_cloud_write_rejects_dangling_symlink_outside_temp_directory() {
        use std::os::unix::fs::symlink;

        let project = project_fixture_outside_system_temp();
        let external_cloud = project.path().join(".cas/cloud.json");
        std::fs::create_dir_all(external_cloud.parent().unwrap()).unwrap();

        let temp = TempDir::new().unwrap();
        let link = temp.path().join("cloud.json");
        symlink(&external_cloud, &link).unwrap();

        let config = CloudConfig {
            token: Some(TEST_FIXTURE_TOKEN.to_string()),
            ..Default::default()
        };

        let error = config.save_to(&link).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("outside the system temp directory")
        );
        assert!(
            !external_cloud.exists(),
            "a fixture write must not follow a dangling symlink outside temp"
        );
    }

    #[test]
    fn store_login_credentials_works_outside_a_project() {
        // Ben #4 (cas-046d): `cas login --token` from $HOME died with
        // "Cassy not initialized — run cas init".
        let mut guard = TestEnvGuard::temp_home();
        let user_path = guard.home().join(".cas/cloud.json");
        std::fs::create_dir_all(user_path.parent().unwrap()).unwrap();
        let outside = guard.home().join("not-a-cas-project");
        std::fs::create_dir_all(&outside).unwrap();

        guard.set("CAS_USER_CLOUD_JSON", &user_path);
        guard.remove("CAS_ROOT");
        guard.set_current_dir(&outside);

        let cached = store_login_credentials(
            "https://petra-stella-cloud.vercel.app",
            "fresh-token",
            None,
            None,
        )
        .expect("logging in outside a project must succeed");

        assert!(cached.is_none(), "there is no project cache to write");
        let user = CloudConfig::load_from(&user_path).unwrap();
        assert_eq!(user.token.as_deref(), Some("fresh-token"));
        assert!(CloudConfig::load_effective().is_logged_in());
    }

    #[test]
    fn clear_login_credentials_clears_user_level_and_project() {
        let (_guard, _temp, user_path, project_path) = machine_fixture();
        CloudConfig {
            token: Some("t".to_string()),
            team_id: Some("team-123".to_string()),
            ..Default::default()
        }
        .save_to(&user_path)
        .unwrap();
        CloudConfig {
            token: Some("t".to_string()),
            team_id: Some("team-123".to_string()),
            ..Default::default()
        }
        .save_to(&project_path)
        .unwrap();

        clear_login_credentials().unwrap();

        let user = CloudConfig::load_from(&user_path).unwrap();
        assert!(!user.is_logged_in(), "logout is machine-wide");
        let project = CloudConfig::load_from(&project_path).unwrap();
        assert!(
            !project.is_logged_in(),
            "the project credential cache must not outlive logout"
        );
        assert!(
            !CloudConfig::load().unwrap().is_logged_in(),
            "nothing re-inherits a cleared credential"
        );
    }

    #[test]
    fn test_logout() {
        let _guard = TestEnvGuard::new();
        let mut config = CloudConfig {
            token: Some("test_token".to_string()),
            email: Some("test@example.com".to_string()),
            ..Default::default()
        };

        assert!(config.is_logged_in());

        config.logout();

        assert!(!config.is_logged_in());
        assert!(config.token.is_none());
        assert!(config.email.is_none());
    }

    #[test]
    fn test_set_and_clear_team() {
        let _guard = TestEnvGuard::new();
        let mut config = CloudConfig::default();
        assert!(!config.has_team());
        assert!(config.team_id.is_none());
        assert!(config.team_slug.is_none());

        config.set_team("team-123", "my-team");
        assert!(config.has_team());
        assert_eq!(config.team_id, Some("team-123".to_string()));
        assert_eq!(config.team_slug, Some("my-team".to_string()));

        config.clear_team();
        assert!(!config.has_team());
        assert!(config.team_id.is_none());
        assert!(config.team_slug.is_none());
    }

    #[test]
    fn test_active_team_id_returns_none_when_no_team_set() {
        let _guard = TestEnvGuard::new();
        // Ensure no user-level config leaks in from ~/.cas/cloud.json.
        unsafe {
            std::env::set_var("CAS_USER_CLOUD_JSON", "/nonexistent/path/cloud.json");
        }
        let config = CloudConfig::default();
        assert_eq!(config.active_team_id(), None);
        unsafe {
            std::env::remove_var("CAS_USER_CLOUD_JSON");
        }
    }

    #[test]
    fn test_active_team_id_returns_team_when_team_id_explicitly_set() {
        let _guard = TestEnvGuard::new();
        // team_id is explicitly set via `cas cloud team set` → Step 1 returns it
        // regardless of team_auto_promote value (Step 1 precedes Step 1.5 guard).
        unsafe {
            std::env::set_var("CAS_USER_CLOUD_JSON", "/nonexistent/path/cloud.json");
        }
        let mut config = CloudConfig::default();
        config.set_team("team-abc", "my-team");
        assert_eq!(config.active_team_id().as_deref(), Some("team-abc"));
        assert!(config.team_auto_promote.is_none()); // Step 1 fires before Step 1.5
        unsafe {
            std::env::remove_var("CAS_USER_CLOUD_JSON");
        }
    }

    #[test]
    fn test_active_team_id_returns_team_when_auto_promote_is_true() {
        let _guard = TestEnvGuard::new();
        unsafe {
            std::env::set_var("CAS_USER_CLOUD_JSON", "/nonexistent/path/cloud.json");
        }
        let mut config = CloudConfig::default();
        config.set_team("team-abc", "my-team");
        config.team_auto_promote = Some(true);
        assert_eq!(config.active_team_id().as_deref(), Some("team-abc"));
        unsafe {
            std::env::remove_var("CAS_USER_CLOUD_JSON");
        }
    }

    #[test]
    fn test_active_team_id_suppressed_by_auto_promote_false() {
        let _guard = TestEnvGuard::new();
        // The coarse kill-switch from Decision 3 of filter-policy.md —
        // team_id still set, but dual-enqueue is disabled.
        unsafe {
            std::env::set_var("CAS_USER_CLOUD_JSON", "/nonexistent/path/cloud.json");
        }
        let mut config = CloudConfig::default();
        config.set_team("team-abc", "my-team");
        config.team_auto_promote = Some(false);
        assert_eq!(config.active_team_id(), None);
        unsafe {
            std::env::remove_var("CAS_USER_CLOUD_JSON");
        }
    }

    // ── cas-ea2f5: resolution-chain unit tests (test-first, added before impl) ──

    #[test]
    fn test_active_team_id_user_default_team_fallback_requires_opt_in() {
        let _guard = TestEnvGuard::new();
        // cas-f8e3: a project WITHOUT team_id AND WITHOUT team_auto_promote=Some(true)
        // is personal — user-level default_team_id must NOT apply.
        let project_cfg = CloudConfig::default(); // no team_id, team_auto_promote=None
        let mut user_cfg = CloudConfig::default();
        user_cfg.default_team_id = Some("user-default-team".to_string());

        assert_eq!(
            project_cfg.active_team_id_with_user_config(Some(&user_cfg)),
            None,
            "personal project (no team_id, no team_auto_promote=Some(true)) must not \
             be promoted via user-level default_team_id (cas-f8e3)"
        );
    }

    #[test]
    fn test_active_team_id_user_default_team_fallback_with_explicit_opt_in() {
        let _guard = TestEnvGuard::new();
        // team_auto_promote=Some(true) explicitly opts the project in to
        // inheriting the user-level default_team_id.
        let mut project_cfg = CloudConfig::default(); // no team_id
        project_cfg.team_auto_promote = Some(true);
        let mut user_cfg = CloudConfig::default();
        user_cfg.default_team_id = Some("user-default-team".to_string());

        assert_eq!(
            project_cfg
                .active_team_id_with_user_config(Some(&user_cfg))
                .as_deref(),
            Some("user-default-team"),
        );
    }

    #[test]
    fn test_active_team_id_single_team_auto_pick_requires_opt_in() {
        let _guard = TestEnvGuard::new();
        // cas-f8e3: a project WITHOUT team_id AND WITHOUT team_auto_promote=Some(true)
        // is personal — single-team auto-pick must NOT apply.
        let project_cfg = CloudConfig::default();
        let mut user_cfg = CloudConfig::default();
        user_cfg.teams = vec![TeamInfo {
            id: "solo-team-id".to_string(),
            slug: "solo".to_string(),
            name: "Solo".to_string(),
            role: "member".to_string(),
        }];

        assert_eq!(
            project_cfg.active_team_id_with_user_config(Some(&user_cfg)),
            None,
            "personal project must not auto-pick from single-team membership \
             without explicit opt-in (cas-f8e3)"
        );
    }

    #[test]
    fn test_active_team_id_single_team_auto_pick_with_explicit_opt_in() {
        let _guard = TestEnvGuard::new();
        // team_auto_promote=Some(true) opts the project in to single-team auto-pick.
        let mut project_cfg = CloudConfig::default();
        project_cfg.team_auto_promote = Some(true);
        let mut user_cfg = CloudConfig::default();
        user_cfg.teams = vec![TeamInfo {
            id: "solo-team-id".to_string(),
            slug: "solo".to_string(),
            name: "Solo".to_string(),
            role: "member".to_string(),
        }];

        assert_eq!(
            project_cfg
                .active_team_id_with_user_config(Some(&user_cfg))
                .as_deref(),
            Some("solo-team-id"),
        );
    }

    // ── cas-c117: automatic team-scope adoption ────────────────────────────
    //
    // Operator directive: a logged-in user whose team identity is already
    // resolvable must not have to run `cas cloud team set` / `team auto on`
    // before their project is team-scoped. These lock the decision table.

    fn logged_in_project() -> CloudConfig {
        let mut cfg = CloudConfig::default();
        cfg.token = Some("test-token".to_string());
        cfg
    }

    fn user_with_teams(teams: &[(&str, &str, &str)], default_team_id: Option<&str>) -> CloudConfig {
        let mut cfg = CloudConfig::default();
        cfg.teams = teams
            .iter()
            .map(|(id, slug, name)| TeamInfo {
                id: (*id).to_string(),
                slug: (*slug).to_string(),
                name: (*name).to_string(),
                role: "member".to_string(),
            })
            .collect();
        cfg.default_team_id = default_team_id.map(ToString::to_string);
        cfg
    }

    #[test]
    fn adoption_opts_a_fresh_project_into_the_sole_team() {
        let _guard = TestEnvGuard::new();
        let mut project = logged_in_project();
        let user = user_with_teams(&[("solo-team-id", "solo", "Solo Team")], None);

        let outcome = adopt_team_scope_for_configs(&mut project, &user);

        match outcome {
            TeamScopeAdoption::Adopted(team) => {
                assert_eq!(team.team_id, "solo-team-id");
                assert_eq!(team.team_slug, "solo");
                assert_eq!(team.team_name, "Solo Team");
            }
            other => panic!("expected adoption of the sole membership, got {other:?}"),
        }
        assert_eq!(project.team_auto_promote, Some(true));
        assert_eq!(
            project
                .active_team_id_with_user_config(Some(&user))
                .as_deref(),
            Some("solo-team-id"),
            "adoption must make the team actually effective, not just recorded"
        );
    }

    #[test]
    fn adoption_prefers_the_user_default_over_membership_order() {
        let _guard = TestEnvGuard::new();
        let mut project = logged_in_project();
        let user = user_with_teams(
            &[("team-a", "alpha", "Alpha"), ("team-b", "beta", "Beta")],
            Some("team-b"),
        );

        match adopt_team_scope_for_configs(&mut project, &user) {
            TeamScopeAdoption::Adopted(team) => assert_eq!(team.team_id, "team-b"),
            other => panic!("expected the user default to be adopted, got {other:?}"),
        }
    }

    #[test]
    fn adoption_never_overrides_an_explicit_team_set() {
        let _guard = TestEnvGuard::new();
        let mut project = logged_in_project();
        project.set_team("pinned-team", "pinned");
        let user = user_with_teams(&[("other-team", "other", "Other")], Some("other-team"));

        assert_eq!(
            adopt_team_scope_for_configs(&mut project, &user),
            TeamScopeAdoption::AlreadyScoped {
                team_id: "pinned-team".to_string()
            },
            "`cas cloud team set` must remain an override, not a competitor"
        );
        assert_eq!(project.team_id.as_deref(), Some("pinned-team"));
    }

    #[test]
    fn adoption_respects_the_auto_off_kill_switch() {
        let _guard = TestEnvGuard::new();
        let mut project = logged_in_project();
        project.team_auto_promote = Some(false);
        let user = user_with_teams(&[("solo-team-id", "solo", "Solo")], None);

        assert_eq!(
            adopt_team_scope_for_configs(&mut project, &user),
            TeamScopeAdoption::OptedOut,
            "`cas cloud team auto off` must never be undone by adoption"
        );
        assert_eq!(project.team_auto_promote, Some(false));
    }

    #[test]
    fn adoption_refuses_to_guess_between_several_teams() {
        let _guard = TestEnvGuard::new();
        let mut project = logged_in_project();
        let user = user_with_teams(
            &[("team-a", "alpha", "Alpha"), ("team-b", "beta", "Beta")],
            None,
        );

        assert_eq!(
            adopt_team_scope_for_configs(&mut project, &user),
            TeamScopeAdoption::NoResolvableTeam {
                membership_count: 2
            }
        );
        assert_eq!(project.team_auto_promote, None);
    }

    #[test]
    fn adoption_accepts_the_machine_wide_login_of_a_fresh_clone() {
        let _guard = TestEnvGuard::new();
        // cas-046d: `cas login` stores the token in `~/.cas/cloud.json`, so a
        // freshly cloned project has no token of its own. That is precisely
        // the case adoption exists for — it must not read as "not logged in".
        let mut project = CloudConfig::default();
        assert!(!project.is_logged_in());
        let mut user = user_with_teams(&[("solo-team-id", "solo", "Solo")], None);
        user.token = Some("machine-wide-token".to_string());

        match adopt_team_scope_for_configs(&mut project, &user) {
            TeamScopeAdoption::Adopted(team) => assert_eq!(team.team_id, "solo-team-id"),
            other => panic!("a machine-wide login must enable adoption, got {other:?}"),
        }
        assert_eq!(project.team_auto_promote, Some(true));
    }

    #[test]
    fn adoption_is_inert_without_a_token_or_membership() {
        let _guard = TestEnvGuard::new();
        let mut anonymous = CloudConfig::default();
        let user = user_with_teams(&[("solo-team-id", "solo", "Solo")], None);
        assert!(!user.is_logged_in(), "neither config holds a token");
        assert_eq!(
            adopt_team_scope_for_configs(&mut anonymous, &user),
            TeamScopeAdoption::NotLoggedIn
        );
        assert_eq!(anonymous.team_auto_promote, None);

        let mut project = logged_in_project();
        let no_teams = CloudConfig::default();
        assert_eq!(
            adopt_team_scope_for_configs(&mut project, &no_teams),
            TeamScopeAdoption::NoResolvableTeam {
                membership_count: 0
            }
        );
        assert_eq!(project.team_auto_promote, None);
    }

    #[test]
    fn adoption_persists_only_when_it_changed_something() {
        let _guard = TestEnvGuard::new();
        let project_dir = TempDir::new().unwrap();
        let user_dir = TempDir::new().unwrap();

        let user = user_with_teams(&[("solo-team-id", "solo", "Solo")], None);
        user.save_to_cas_dir(user_dir.path()).unwrap();
        // SAFETY: TestEnvGuard serializes env mutation for this test.
        unsafe {
            std::env::set_var(
                "CAS_USER_CLOUD_JSON",
                user_dir.path().join("cloud.json").to_str().unwrap(),
            );
        }

        logged_in_project()
            .save_to_cas_dir(project_dir.path())
            .unwrap();

        let first = maybe_adopt_team_scope(project_dir.path()).unwrap();
        assert!(matches!(first, TeamScopeAdoption::Adopted(_)));
        let persisted = CloudConfig::load_from_cas_dir(project_dir.path()).unwrap();
        assert_eq!(
            persisted.team_auto_promote,
            Some(true),
            "adoption must survive the process that made it"
        );

        // Second run: already scoped, nothing to write.
        let second = maybe_adopt_team_scope(project_dir.path()).unwrap();
        assert_eq!(
            second,
            TeamScopeAdoption::AlreadyScoped {
                team_id: "solo-team-id".to_string()
            }
        );

        unsafe {
            std::env::remove_var("CAS_USER_CLOUD_JSON");
        }
    }

    #[test]
    fn personal_scope_notice_fires_once_for_single_team_user() {
        let _guard = TestEnvGuard::new();
        let project = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();

        let project_cfg = CloudConfig::default();
        project_cfg.save_to_cas_dir(project.path()).unwrap();

        let mut user_cfg = CloudConfig::default();
        user_cfg.teams = vec![TeamInfo {
            id: "solo-team-id".to_string(),
            slug: "solo".to_string(),
            name: "Solo".to_string(),
            role: "member".to_string(),
        }];
        user_cfg.save_to_cas_dir(user.path()).unwrap();

        let user_cloud_json = user.path().join("cloud.json");
        unsafe {
            std::env::set_var("CAS_USER_CLOUD_JSON", &user_cloud_json);
        }

        let first = maybe_mark_personal_scope_notice(project.path())
            .unwrap()
            .expect("single-team personal project should emit notice");
        assert_eq!(first.team_slug, "solo");
        let saved = CloudConfig::load_from_cas_dir(project.path()).unwrap();
        assert!(saved.personal_scope_notice_shown);
        assert!(saved.team_id.is_none());
        assert!(saved.team_auto_promote.is_none());

        let second = maybe_mark_personal_scope_notice(project.path()).unwrap();
        assert!(second.is_none(), "notice must be one-time per project");
        unsafe {
            std::env::remove_var("CAS_USER_CLOUD_JSON");
        }
    }

    #[test]
    fn personal_scope_notice_rechecks_fresh_config_before_marking() {
        let _guard = TestEnvGuard::new();
        let project = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();

        CloudConfig::default()
            .save_to_cas_dir(project.path())
            .unwrap();

        let mut user_cfg = CloudConfig::default();
        user_cfg.teams = vec![TeamInfo {
            id: "solo-team-id".to_string(),
            slug: "solo".to_string(),
            name: "Solo".to_string(),
            role: "member".to_string(),
        }];
        user_cfg.save_to_cas_dir(user.path()).unwrap();

        let user_cloud_json = user.path().join("cloud.json");
        unsafe {
            std::env::set_var("CAS_USER_CLOUD_JSON", &user_cloud_json);
        }

        let notice = maybe_mark_personal_scope_notice_with_hook(project.path(), || {
            let mut concurrent = CloudConfig::load_from_cas_dir(project.path()).unwrap();
            concurrent.set_team("solo-team-id", "solo");
            concurrent.save_to_cas_dir(project.path()).unwrap();
        })
        .unwrap();

        assert!(
            notice.is_none(),
            "fresh re-check should suppress notice after concurrent team link"
        );
        let saved = CloudConfig::load_from_cas_dir(project.path()).unwrap();
        assert_eq!(saved.team_id.as_deref(), Some("solo-team-id"));
        assert_eq!(saved.team_slug.as_deref(), Some("solo"));
        assert!(!saved.personal_scope_notice_shown);
        unsafe {
            std::env::remove_var("CAS_USER_CLOUD_JSON");
        }
    }

    #[test]
    fn personal_scope_notice_suppressed_for_team_linked_project() {
        let _guard = TestEnvGuard::new();
        let mut project_cfg = CloudConfig::default();
        project_cfg.team_auto_promote = Some(true);
        let mut user_cfg = CloudConfig::default();
        user_cfg.default_team_id = Some("team-1".to_string());
        assert!(
            personal_scope_notice_for_configs(&project_cfg, &user_cfg).is_none(),
            "opted-in project resolves to team scope, so no personal-scope notice"
        );
    }

    #[test]
    fn personal_scope_notice_suppressed_for_user_with_no_teams() {
        let _guard = TestEnvGuard::new();
        let project_cfg = CloudConfig::default();
        let user_cfg = CloudConfig::default();
        assert!(personal_scope_notice_for_configs(&project_cfg, &user_cfg).is_none());
    }

    #[test]
    fn test_active_team_id_multi_team_ambiguous_returns_none() {
        let _guard = TestEnvGuard::new();
        // No project-level team_id → personal regardless of user team count.
        // Even with team_auto_promote=Some(true), ambiguous (2+ teams) returns None.
        let mut project_cfg = CloudConfig::default();
        project_cfg.team_auto_promote = Some(true); // opt in to user-level fallback
        let mut user_cfg = CloudConfig::default();
        user_cfg.teams = vec![
            TeamInfo {
                id: "t1".to_string(),
                slug: "a".to_string(),
                name: "A".to_string(),
                role: "member".to_string(),
            },
            TeamInfo {
                id: "t2".to_string(),
                slug: "b".to_string(),
                name: "B".to_string(),
                role: "member".to_string(),
            },
        ];

        assert_eq!(
            project_cfg.active_team_id_with_user_config(Some(&user_cfg)),
            None,
        );
    }

    #[test]
    fn test_active_team_id_project_override_beats_user_default() {
        let _guard = TestEnvGuard::new();
        // Project-level team_id wins over user-level default_team_id.
        let mut project_cfg = CloudConfig::default();
        project_cfg.set_team("project-team", "proj");
        let mut user_cfg = CloudConfig::default();
        user_cfg.default_team_id = Some("user-default-team".to_string());

        assert_eq!(
            project_cfg
                .active_team_id_with_user_config(Some(&user_cfg))
                .as_deref(),
            Some("project-team"),
        );
    }

    #[test]
    fn test_active_team_id_kill_switch_beats_user_config() {
        let _guard = TestEnvGuard::new();
        // team_auto_promote=Some(false) short-circuits to None even when user
        // config would otherwise supply a team.
        let mut project_cfg = CloudConfig::default();
        project_cfg.team_auto_promote = Some(false);
        let mut user_cfg = CloudConfig::default();
        user_cfg.default_team_id = Some("user-default-team".to_string());

        assert_eq!(
            project_cfg.active_team_id_with_user_config(Some(&user_cfg)),
            None,
        );
    }

    #[test]
    fn test_active_team_id_no_user_config_no_project_team() {
        let _guard = TestEnvGuard::new();
        // Neither project nor user config has team info → None.
        let project_cfg = CloudConfig::default();
        assert_eq!(project_cfg.active_team_id_with_user_config(None), None);
    }

    #[test]
    fn test_team_sync_timestamps() {
        let _guard = TestEnvGuard::new();
        let mut config = CloudConfig::default();

        // Initially no timestamps
        assert!(config.get_team_sync_timestamp("team-a").is_none());

        // Set timestamp for team-a
        let ts1 = Utc::now();
        config.set_team_sync_timestamp("team-a", ts1);
        assert_eq!(config.get_team_sync_timestamp("team-a"), Some(ts1));

        // Set timestamp for team-b
        let ts2 = Utc::now();
        config.set_team_sync_timestamp("team-b", ts2);
        assert_eq!(config.get_team_sync_timestamp("team-b"), Some(ts2));

        // team-a still has its timestamp
        assert_eq!(config.get_team_sync_timestamp("team-a"), Some(ts1));

        // Clear team-a timestamp
        config.clear_team_sync_timestamp("team-a");
        assert!(config.get_team_sync_timestamp("team-a").is_none());
        assert_eq!(config.get_team_sync_timestamp("team-b"), Some(ts2));
    }

    #[test]
    fn test_team_memory_sync_timestamps() {
        let _guard = TestEnvGuard::new();
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("cloud.json");

        let mut config = CloudConfig {
            token: Some("t".to_string()),
            ..Default::default()
        };

        // Initially no timestamp
        assert!(config.get_team_memory_sync("github.com/foo/bar").is_none());

        // Set and get
        config.set_team_memory_sync("github.com/foo/bar", "2026-04-02T10:00:00Z");
        assert_eq!(
            config.get_team_memory_sync("github.com/foo/bar"),
            Some("2026-04-02T10:00:00Z")
        );

        // Persists through save/load
        config.save_to(&path).unwrap();
        let loaded = CloudConfig::load_from(&path).unwrap();
        assert_eq!(
            loaded.get_team_memory_sync("github.com/foo/bar"),
            Some("2026-04-02T10:00:00Z")
        );
    }

    #[test]
    fn test_canonical_id_from_cas_root() {
        // Create real temp directories simulating different project layouts
        let temp = TempDir::new().unwrap();

        // Simulate /tmp/.../petra-stella-cloud/.cas
        let project_a = temp.path().join("petra-stella-cloud");
        let cas_a = project_a.join(".cas");
        std::fs::create_dir_all(&cas_a).unwrap();
        assert_eq!(
            canonical_id_from_cas_root(&cas_a),
            Some("petra-stella-cloud".to_string())
        );

        // Simulate /tmp/.../gabber-studio/.cas
        let project_b = temp.path().join("gabber-studio");
        let cas_b = project_b.join(".cas");
        std::fs::create_dir_all(&cas_b).unwrap();
        assert_eq!(
            canonical_id_from_cas_root(&cas_b),
            Some("gabber-studio".to_string())
        );

        // Non-git project works the same way
        let project_c = temp.path().join("local-only-project");
        let cas_c = project_c.join(".cas");
        std::fs::create_dir_all(&cas_c).unwrap();
        assert_eq!(
            canonical_id_from_cas_root(&cas_c),
            Some("local-only-project".to_string())
        );

        // Folder with spaces
        let project_d = temp.path().join("Richards LLC");
        let cas_d = project_d.join(".cas");
        std::fs::create_dir_all(&cas_d).unwrap();
        assert_eq!(
            canonical_id_from_cas_root(&cas_d),
            Some("Richards LLC".to_string())
        );
    }

    #[test]
    fn test_canonical_id_from_filesystem_root() {
        // Edge case: .cas at filesystem root — parent is "/" which has no file_name
        use std::path::Path;
        let root_cas = Path::new("/.cas");
        assert_eq!(canonical_id_from_cas_root(root_cas), None);
    }

    #[test]
    fn test_fallback_project_id_from_path_is_deterministic() {
        // Same input path produces the same hash across repeated invocations,
        // and the format is `local:` + 16 lowercase-hex chars (8 bytes of sha256).
        let temp = TempDir::new().unwrap();
        let project_dir = temp.path().join("some-project");
        let cas_dir = project_dir.join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();

        let first = fallback_project_id_from_path(&cas_dir).unwrap();
        let second = fallback_project_id_from_path(&cas_dir).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("local:"));
        // local: + 16 hex chars = 22 chars total
        assert_eq!(first.len(), 22);
        // Every char after the `local:` prefix must be a lowercase ASCII hex digit.
        let suffix = &first[6..];
        assert!(
            suffix
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "fallback suffix should be lowercase hex, got {suffix:?}"
        );
    }

    #[test]
    fn test_fallback_project_id_from_path_is_unique_per_path() {
        // Different project paths must produce different hashes — otherwise two
        // projects at different locations would still collide.
        let temp = TempDir::new().unwrap();

        let project_a = temp.path().join("project-a");
        let cas_a = project_a.join(".cas");
        std::fs::create_dir_all(&cas_a).unwrap();

        let project_b = temp.path().join("project-b");
        let cas_b = project_b.join(".cas");
        std::fs::create_dir_all(&cas_b).unwrap();

        let id_a = fallback_project_id_from_path(&cas_a).unwrap();
        let id_b = fallback_project_id_from_path(&cas_b).unwrap();
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn test_fallback_project_id_handles_filesystem_root() {
        // The whole point of the fallback: at filesystem root,
        // canonical_id_from_cas_root returns None; fallback must still produce a value.
        use std::path::Path;
        let root_cas = Path::new("/.cas");
        assert_eq!(canonical_id_from_cas_root(root_cas), None);

        let fallback = fallback_project_id_from_path(root_cas);
        assert!(fallback.is_some());
        let id = fallback.unwrap();
        assert!(id.starts_with("local:"));
        assert_eq!(id.len(), 22);
    }

    #[test]
    fn test_resolve_canonical_id_prefers_folder_name() {
        // End-to-end coverage of the chain: with no config pin and no git
        // remote, the folder name is returned unchanged — the path-hash
        // fallback must not fire on the happy path.
        let temp = TempDir::new().unwrap();
        let project_dir = temp.path().join("my-project");
        let cas_dir = project_dir.join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();

        let (id, source) = resolve_canonical_id_with_source(&cas_dir).unwrap();
        assert_eq!(id, "my-project");
        assert_eq!(source, CanonicalIdSource::FolderName);
        assert!(!id.starts_with("local:"));
    }

    // ── cas-f699 / GH #134: git remote ahead of the folder-name fallback ──

    /// Make `<parent>/<name>` a git repo with `origin` pointing at `remote`,
    /// containing a `.cas/` dir. Returns the `.cas` path.
    fn git_project_with_remote(parent: &Path, name: &str, remote: &str) -> PathBuf {
        let project_dir = parent.join(name);
        let cas_dir = project_dir.join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();
        let run = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&project_dir)
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("git must be available for this test");
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["remote", "add", "origin", remote]);
        cas_dir
    }

    #[test]
    fn resolve_canonical_id_prefers_git_remote_over_folder_name() {
        // The GH #134 defect: the folder name decided the cloud bucket even
        // when the repository identity was available. Now the remote wins.
        let temp = TempDir::new().unwrap();
        let cas_dir = git_project_with_remote(
            temp.path(),
            "accounting",
            "git@github.com:client-one/accounting.git",
        );

        let (id, source) = resolve_canonical_id_with_source(&cas_dir).unwrap();
        assert_eq!(id, "github.com/client-one/accounting");
        assert_eq!(source, CanonicalIdSource::GitRemote);
    }

    #[test]
    fn same_folder_name_different_remotes_no_longer_share_a_bucket() {
        // Two different clients' checkouts, both in a folder called
        // `accounting`. Before the fix both resolved to `accounting` and
        // merged into each other on every sync.
        let temp = TempDir::new().unwrap();
        let one = git_project_with_remote(
            &temp.path().join("client-one"),
            "accounting",
            "https://github.com/client-one/accounting.git",
        );
        let two = git_project_with_remote(
            &temp.path().join("client-two"),
            "accounting",
            "git@gitlab.com:client-two/accounting.git",
        );

        assert_eq!(
            canonical_id_from_cas_root(&one),
            canonical_id_from_cas_root(&two)
        );
        assert_ne!(
            resolve_canonical_id(&one),
            resolve_canonical_id(&two),
            "two unrelated repos must not resolve to the same cloud bucket"
        );
    }

    #[test]
    fn config_toml_pin_still_beats_git_remote() {
        // AC: existing pinned-config behaviour is unchanged — an explicit pin
        // remains the source of truth even when a remote is derivable.
        let temp = TempDir::new().unwrap();
        let cas_dir =
            git_project_with_remote(temp.path(), "ledger", "git@github.com:acme/ledger.git");
        set_canonical_id_in_config_toml(&cas_dir, "pinned-id").unwrap();

        let (id, source) = resolve_canonical_id_with_source(&cas_dir).unwrap();
        assert_eq!(id, "pinned-id");
        assert_eq!(source, CanonicalIdSource::ConfigToml);
    }

    #[test]
    fn sync_identity_refuses_a_pin_for_a_different_git_remote() {
        let temp = TempDir::new().unwrap();
        let cas_dir = git_project_with_remote(
            temp.path(),
            "ledger",
            "git@github.com:acme/ledger.git",
        );
        set_canonical_id_in_config_toml(&cas_dir, "github.com/other/other-repo").unwrap();

        let error = resolve_canonical_id_for_sync(&cas_dir)
            .expect_err("a pinned identity for another repository must fail closed")
            .to_string();
        assert!(error.contains("github.com/acme/ledger"), "error: {error}");
        assert!(error.contains("github.com/other/other-repo"), "error: {error}");
        assert!(error.contains("cas cloud project set"), "error: {error}");
    }

    #[test]
    fn sync_identity_accepts_authoritative_cas_src_slug_pin() {
        let temp = TempDir::new().unwrap();
        let cas_dir = git_project_with_remote(
            temp.path(),
            "cas-src",
            "git@github.com:Richards-LLC/cassy.git",
        );
        set_canonical_id_in_config_toml(&cas_dir, "cas-src").unwrap();

        assert_eq!(
            resolve_canonical_id_for_sync(&cas_dir).unwrap(),
            "cas-src",
            "a bare slug pin is the operator-selected cloud bucket",
        );
    }

    #[test]
    fn sync_identity_accepts_authoritative_ozer_slug_pin() {
        let temp = TempDir::new().unwrap();
        let cas_dir = git_project_with_remote(
            temp.path(),
            "ozer",
            "git@github.com:Richards-LLC/ozer-health.git",
        );
        set_canonical_id_in_config_toml(&cas_dir, "ozer").unwrap();

        assert_eq!(
            resolve_canonical_id_for_sync(&cas_dir).unwrap(),
            "ozer",
            "a bare slug pin is the operator-selected cloud bucket",
        );
    }

    #[test]
    fn git_repo_without_origin_falls_through_to_folder_name() {
        // A git repo with no `origin` must not become `local:<hash>` — the
        // folder-name step still runs, so nobody's existing bucket moves.
        let temp = TempDir::new().unwrap();
        let project_dir = temp.path().join("no-remote-project");
        let cas_dir = project_dir.join(".cas");
        std::fs::create_dir_all(&cas_dir).unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(&project_dir)
            .args(["init", "-q"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();

        let (id, source) = resolve_canonical_id_with_source(&cas_dir).unwrap();
        assert_eq!(id, "no-remote-project");
        assert_eq!(source, CanonicalIdSource::FolderName);
    }

    #[test]
    fn unrecognized_remote_url_falls_through_to_folder_name() {
        // A local-path remote (`/srv/git/x.git`) is not normalizable to
        // host/owner/repo; the chain must fall through rather than persist a
        // non-canonical value or skip straight to the path hash.
        let temp = TempDir::new().unwrap();
        let cas_dir = git_project_with_remote(temp.path(), "weird-remote", "/srv/git/mirror.git");

        let (id, source) = resolve_canonical_id_with_source(&cas_dir).unwrap();
        assert_eq!(id, "weird-remote");
        assert_eq!(source, CanonicalIdSource::FolderName);
    }

    // ── cas-f699 AC2: same-slug collision detection ───────────────────────

    fn identity(root: &str, id: &str, remote: Option<&str>) -> LocalRootIdentity {
        LocalRootIdentity {
            project_root: PathBuf::from(root),
            canonical_id: id.to_string(),
            git_remote: remote.map(str::to_string),
        }
    }

    #[test]
    fn collision_detected_for_different_repos_sharing_an_id() {
        let collisions = detect_canonical_id_collisions(&[
            identity(
                "/home/u/client-one/accounting",
                "accounting",
                Some("github.com/client-one/accounting"),
            ),
            identity(
                "/home/u/client-two/accounting",
                "accounting",
                Some("gitlab.com/client-two/accounting"),
            ),
        ]);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].canonical_id, "accounting");
        assert_eq!(
            collisions[0].roots,
            vec![
                PathBuf::from("/home/u/client-one/accounting"),
                PathBuf::from("/home/u/client-two/accounting"),
            ],
        );
    }

    #[test]
    fn two_remote_less_projects_sharing_a_folder_name_collide() {
        // No remotes at all: each root is its own identity, so the shared
        // folder-name id is still a genuine contamination risk.
        let collisions = detect_canonical_id_collisions(&[
            identity("/home/u/a/notes", "notes", None),
            identity("/home/u/b/notes", "notes", None),
        ]);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].roots.len(), 2);
    }

    #[test]
    fn clones_and_worktrees_of_one_repo_do_not_warn() {
        // Same `origin` → same project → sharing a bucket is correct. This is
        // the false-positive that would otherwise fire on every machine with
        // a second checkout or a git worktree.
        let collisions = detect_canonical_id_collisions(&[
            identity(
                "/home/u/cas-src",
                "github.com/acme/cas",
                Some("github.com/acme/cas"),
            ),
            identity(
                "/home/u/cas-src-review",
                "github.com/acme/cas",
                Some("github.com/acme/cas"),
            ),
        ]);
        assert!(collisions.is_empty(), "got {collisions:?}");
    }

    #[test]
    fn distinct_ids_and_single_roots_never_warn() {
        let collisions = detect_canonical_id_collisions(&[
            identity("/home/u/alpha", "alpha", None),
            identity("/home/u/beta", "beta", None),
        ]);
        assert!(collisions.is_empty());
        assert!(detect_canonical_id_collisions(&[]).is_empty());
    }

    #[test]
    fn duplicate_registry_rows_for_one_root_do_not_warn() {
        // The known-repo registry can list the same root twice; a root can
        // never collide with itself.
        let collisions = detect_canonical_id_collisions(&[
            identity("/home/u/notes", "notes", None),
            identity("/home/u/notes", "notes", None),
        ]);
        assert!(collisions.is_empty(), "got {collisions:?}");
    }

    #[test]
    fn pinned_ids_colliding_across_repos_are_reported() {
        // Two explicit pins that happen to be equal are just as contaminating
        // as two folder names — the detector is source-agnostic.
        let collisions = detect_canonical_id_collisions(&[
            identity("/home/u/one", "shared-pin", Some("github.com/a/one")),
            identity("/home/u/two", "shared-pin", Some("github.com/b/two")),
            identity("/home/u/three", "unique", Some("github.com/c/three")),
        ]);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].canonical_id, "shared-pin");
    }

    #[test]
    fn test_resolve_canonical_id_falls_back_at_filesystem_root() {
        // End-to-end: when folder name is unavailable (filesystem root),
        // resolve_canonical_id returns Some("local:...") instead of None.
        // A regression that dropped the `.or_else` would turn this back into None.
        use std::path::Path;
        let root_cas = Path::new("/.cas");
        let id = resolve_canonical_id(root_cas).expect("fallback should fire at fs root");
        assert!(id.starts_with("local:"));
        assert_eq!(id.len(), 22);
    }

    #[test]
    fn test_fallback_lexical_branch_when_canonicalize_fails() {
        // `fallback_project_id_from_path` falls back to the lexical path when
        // `std::fs::canonicalize` fails (e.g., the directory does not exist on
        // disk). Point it at a non-existent path and verify we still get a
        // stable `local:<hex>` value rather than a panic or None.
        let temp = TempDir::new().unwrap();
        let nonexistent_cas = temp.path().join("never-created").join(".cas");
        // Intentionally do NOT create the directory.

        let id = fallback_project_id_from_path(&nonexistent_cas)
            .expect("fallback must tolerate non-canonicalizable paths");
        assert!(id.starts_with("local:"));
        assert_eq!(id.len(), 22);

        // Deterministic: same non-existent path produces the same hash.
        let id2 = fallback_project_id_from_path(&nonexistent_cas).unwrap();
        assert_eq!(id, id2);
    }

    #[cfg(unix)]
    #[test]
    fn test_fallback_resolves_symlinks_to_same_id() {
        // Documented contract: "symlinked and renamed paths produce the same ID"
        // via `std::fs::canonicalize`. Create a real project, symlink to it,
        // and assert both paths produce the same fallback hash.
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let real_project = temp.path().join("real-project");
        let real_cas = real_project.join(".cas");
        std::fs::create_dir_all(&real_cas).unwrap();

        let link_project = temp.path().join("link-to-project");
        symlink(&real_project, &link_project).unwrap();
        let link_cas = link_project.join(".cas");

        let id_real = fallback_project_id_from_path(&real_cas).unwrap();
        let id_link = fallback_project_id_from_path(&link_cas).unwrap();
        assert_eq!(
            id_real, id_link,
            "symlinked and real paths should hash to the same ID after canonicalization"
        );
    }

    /// Regression test for cas-2c77: OnceLock cached None permanently, so a
    /// transient `find_cas_root()` failure during daemon startup locked out
    /// project scoping for the entire process lifetime.
    ///
    /// This test reproduces the exact contract using the same Mutex<Option>
    /// pattern as the production code. We can't safely test the process-global
    /// static (env var mutations race with parallel tests), so we verify the
    /// pattern in isolation: None results are retried, Some results are cached.
    #[test]
    fn test_mutex_cache_retries_none_but_caches_some() {
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicU32, Ordering};

        let cache: Mutex<Option<String>> = Mutex::new(None);
        let call_count = AtomicU32::new(0);

        // Simulate the get_project_canonical_id pattern with a controllable resolver
        let get_id = |resolver: &dyn Fn() -> Option<String>| -> Option<String> {
            let mut cached = cache.lock().unwrap();
            if let Some(ref id) = *cached {
                return Some(id.clone());
            }
            call_count.fetch_add(1, Ordering::SeqCst);
            let result = resolver();
            if result.is_some() {
                *cached = result.clone();
            }
            result
        };

        // First call: resolver returns None (simulates find_cas_root failing)
        let result1 = get_id(&|| None);
        assert_eq!(result1, None);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Second call: resolver still returns None — should retry (not return cached None)
        let result2 = get_id(&|| None);
        assert_eq!(result2, None);
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "None must not be cached — resolver should be called again"
        );

        // Third call: resolver now succeeds (simulates cwd moved into a Cassy project)
        let result3 = get_id(&|| Some("my-project".to_string()));
        assert_eq!(result3, Some("my-project".to_string()));
        assert_eq!(call_count.load(Ordering::SeqCst), 3);

        // Fourth call: should return cached value without calling resolver
        let result4 = get_id(&|| panic!("resolver should not be called when cache has Some"));
        assert_eq!(result4, Some("my-project".to_string()));
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            3,
            "Some must be cached — resolver should not be called again"
        );
    }

    #[test]
    fn test_team_sync_timestamps_persist() {
        let _guard = TestEnvGuard::new();
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("cloud.json");

        let mut config = CloudConfig {
            token: Some("test_token".to_string()),
            ..Default::default()
        };
        config.set_team("team-123", "my-team");
        let ts = Utc::now();
        config.set_team_sync_timestamp("team-123", ts);

        config.save_to(&path).unwrap();

        let loaded = CloudConfig::load_from(&path).unwrap();
        assert_eq!(loaded.team_id, Some("team-123".to_string()));
        assert_eq!(loaded.team_slug, Some("my-team".to_string()));
        // Timestamps are stored with second precision in JSON
        let loaded_ts = loaded.get_team_sync_timestamp("team-123").unwrap();
        assert!((loaded_ts - ts).num_seconds().abs() < 1);
    }

    // cas-1ced: git-remote URL normalizer + config.toml round-trip helpers.

    #[test]
    fn normalize_https_strips_protocol_and_dot_git() {
        assert_eq!(
            normalize_git_remote_url("https://github.com/foo/bar.git").as_deref(),
            Some("github.com/foo/bar"),
        );
    }

    #[test]
    fn normalize_https_handles_missing_dot_git() {
        assert_eq!(
            normalize_git_remote_url("https://github.com/foo/bar").as_deref(),
            Some("github.com/foo/bar"),
        );
    }

    #[test]
    fn normalize_http_strips_protocol_and_dot_git() {
        assert_eq!(
            normalize_git_remote_url("http://gitlab.example.com/g/p.git").as_deref(),
            Some("gitlab.example.com/g/p"),
        );
    }

    #[test]
    fn normalize_ssh_user_form() {
        assert_eq!(
            normalize_git_remote_url("git@github.com:foo/bar.git").as_deref(),
            Some("github.com/foo/bar"),
        );
    }

    #[test]
    fn normalize_ssh_url_form() {
        assert_eq!(
            normalize_git_remote_url("ssh://git@github.com/foo/bar.git").as_deref(),
            Some("github.com/foo/bar"),
        );
    }

    #[test]
    fn normalize_gitlab_subgroup() {
        assert_eq!(
            normalize_git_remote_url("https://gitlab.com/group/subgroup/project.git").as_deref(),
            Some("gitlab.com/group/subgroup/project"),
        );
    }

    #[test]
    fn canonical_project_id_normalizes_case_and_remote_spelling() {
        assert_eq!(
            normalize_project_canonical_id(" Git@GitHub.com:Richards-LLC/gabber-studio.git ")
                .as_deref(),
            Some("github.com/richards-llc/gabber-studio"),
        );
        assert_eq!(
            normalize_project_canonical_id("github.com/Richards-LLC/gabber-studio").as_deref(),
            Some("github.com/richards-llc/gabber-studio"),
        );
    }

    #[test]
    fn canonical_identity_maps_remote_alias_to_explicit_slug_pin() {
        for alias in [
            "gabber-studio",
            "git@GitHub.com:Richards-LLC/gabber-studio.git",
            "https://github.com/richards-llc/gabber-studio/",
        ] {
            assert_eq!(
                canonical_project_id_with_pin(alias, Some("gabber-studio")).as_deref(),
                Some("gabber-studio"),
                "alias {alias} must resolve to the explicit slug pin",
            );
        }
        for alias in [
            "pixel-hive",
            "ssh://git@GitHub.com/Pixel-Hive/pixel-hive.git",
        ] {
            assert_eq!(
                canonical_project_id_with_pin(alias, Some("pixel-hive")).as_deref(),
                Some("pixel-hive"),
                "alias {alias} must resolve to the explicit slug pin",
            );
        }
    }

    #[test]
    fn canonical_identity_keeps_different_repository_foreign_to_slug_pin() {
        assert_eq!(
            canonical_project_id_with_pin(
                "git@github.com:someone-else/other-repo.git",
                Some("gabber-studio"),
            )
            .as_deref(),
            Some("github.com/someone-else/other-repo"),
        );
    }

    #[test]
    fn canonical_identity_matches_bare_alias_to_explicit_remote_pin() {
        for alias in ["gabber-studio", "GABBER-STUDIO"] {
            assert_eq!(
                project_ids_match(
                    alias,
                    "https://GitHub.com/Richards-LLC/gabber-studio.git",
                ),
                true,
                "alias {alias} must match the explicit remote pin",
            );
        }
    }

    #[test]
    fn explicit_pin_wins_even_when_it_equals_the_repo_name() {
        let temp = TempDir::new().unwrap();
        let cas_dir = git_project_with_remote(
            temp.path(),
            "gabber-studio",
            "git@GitHub.com:Richards-LLC/gabber-studio.git",
        );
        set_canonical_id_in_config_toml(&cas_dir, "gabber-studio").unwrap();

        assert_eq!(
            resolve_canonical_id(&cas_dir).as_deref(),
            Some("gabber-studio"),
            "an explicit pin is the source of truth, even when it equals the repo name",
        );
    }

    #[test]
    fn normalize_rejects_local_path() {
        // Local path is not a recognizable URL shape — falls through to None.
        assert_eq!(normalize_git_remote_url("/home/user/repo"), None);
    }

    #[test]
    fn normalize_rejects_empty() {
        assert_eq!(normalize_git_remote_url(""), None);
        assert_eq!(normalize_git_remote_url("   "), None);
    }

    #[test]
    fn config_toml_roundtrip_writes_and_reads_canonical_id() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path();
        assert_eq!(canonical_id_from_config_toml(cas_root), None);
        set_canonical_id_in_config_toml(cas_root, "github.com/foo/bar").unwrap();
        assert_eq!(
            canonical_id_from_config_toml(cas_root).as_deref(),
            Some("github.com/foo/bar"),
        );
    }

    #[test]
    fn config_toml_preserves_other_sections() {
        // Seed config.toml with a pre-existing block that has nothing to do
        // with [project]. The write must NOT clobber it.
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path();
        std::fs::write(
            cas_root.join("config.toml"),
            "[memory]\nsession_learn_auto = true\n",
        )
        .unwrap();

        set_canonical_id_in_config_toml(cas_root, "github.com/foo/bar").unwrap();

        let content = std::fs::read_to_string(cas_root.join("config.toml")).unwrap();
        assert!(
            content.contains("session_learn_auto"),
            "pre-existing [memory] block must survive — got:\n{content}"
        );
        assert!(
            content.contains("github.com/foo/bar"),
            "new canonical_id must be written — got:\n{content}"
        );
    }

    #[test]
    fn alias_refresh_preserves_unrelated_sections_and_canonical_pin() {
        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path();
        std::fs::write(
            cas_root.join("config.toml"),
            "[hooks]\nai_context = false\n\n[project]\ncanonical_id = \"canonical-name\"\n",
        )
        .unwrap();

        let written = set_project_aliases_in_config_toml(
            cas_root,
            &["legacy-name".to_string(), "canonical-name".to_string()],
        )
        .unwrap();

        assert_eq!(written, vec!["legacy-name"]);
        let content = std::fs::read_to_string(cas_root.join("config.toml")).unwrap();
        let parsed: toml::Value = toml::from_str(&content).unwrap();
        assert_eq!(parsed["hooks"]["ai_context"].as_bool(), Some(false));
        assert_eq!(
            parsed["project"]["canonical_id"].as_str(),
            Some("canonical-name")
        );
        assert_eq!(
            parsed["project"]["aliases"].as_array().unwrap(),
            &[toml::Value::String("legacy-name".to_string())]
        );
    }

    #[test]
    fn concurrent_project_config_updates_are_serialized_and_preserve_sections() {
        use std::sync::mpsc;
        use std::time::Duration;

        let temp = tempfile::tempdir().unwrap();
        let cas_root = temp.path().to_path_buf();
        std::fs::write(
            cas_root.join("config.toml"),
            "[hooks]\nai_context = false\n",
        )
        .unwrap();

        let (first_read_tx, first_read_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let first_root = cas_root.clone();
        let first = std::thread::spawn(move || {
            update_project_config_toml_with(
                &first_root,
                |table| {
                    let project = table
                        .entry("project".to_string())
                        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()))
                        .as_table_mut()
                        .unwrap();
                    project.insert(
                        "aliases".to_string(),
                        toml::Value::Array(vec![toml::Value::String("legacy-name".to_string())]),
                    );
                    Ok(true)
                },
                || {
                    first_read_tx.send(()).unwrap();
                    release_first_rx.recv().unwrap();
                },
            )
            .unwrap();
        });
        first_read_rx.recv().unwrap();

        let (second_read_tx, second_read_rx) = mpsc::channel();
        let second_root = cas_root.clone();
        let second = std::thread::spawn(move || {
            update_project_config_toml_with(
                &second_root,
                |table| {
                    let project = table
                        .entry("project".to_string())
                        .or_insert_with(|| toml::Value::Table(toml::value::Table::new()))
                        .as_table_mut()
                        .unwrap();
                    project.insert(
                        "canonical_id".to_string(),
                        toml::Value::String("canonical-name".to_string()),
                    );
                    Ok(true)
                },
                || second_read_tx.send(()).unwrap(),
            )
            .unwrap();
        });

        let second_read_while_first_was_paused = second_read_rx
            .recv_timeout(Duration::from_millis(100))
            .is_ok();
        release_first_tx.send(()).unwrap();
        first.join().unwrap();
        second.join().unwrap();

        assert!(
            !second_read_while_first_was_paused,
            "a second updater reached the commit boundary while the first still held stale state"
        );
        let content = std::fs::read_to_string(cas_root.join("config.toml")).unwrap();
        let parsed: toml::Value = toml::from_str(&content).unwrap();
        assert_eq!(parsed["hooks"]["ai_context"].as_bool(), Some(false));
        assert_eq!(
            parsed["project"]["canonical_id"].as_str(),
            Some("canonical-name")
        );
        assert_eq!(
            parsed["project"]["aliases"].as_array().unwrap(),
            &[toml::Value::String("legacy-name".to_string())]
        );
    }

    #[test]
    fn failed_project_config_commit_leaves_original_valid_and_removes_owned_temp() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.toml");
        let temp_path = temp.path().join(".config.toml.injected.tmp");
        let original = "[hooks]\nai_context = false\n";
        std::fs::write(&config_path, original).unwrap();

        let error = atomic_replace_project_config_via(
            &config_path,
            "[project]\naliases = []\n",
            &temp_path,
            |_temp, _target| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected rename failure",
                ))
            },
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("injected rename failure"), "{error}");
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original);
        assert!(!temp_path.exists(), "owned temp file must be cleaned up");
        toml::from_str::<toml::Value>(original).unwrap();
    }

    // ── default_endpoint env-var tests ──────────────────────────────────────
    // Rust's std::env::set_var is not thread-safe; serialise ALL mutations of
    // CAS_CLOUD_ENDPOINT through the crate-wide test environment guard.
    //
    // Tests that construct CloudConfig::default() (or ..Default::default())
    // also acquire the lock because default_endpoint() now reads the env var.

    #[test]
    fn default_endpoint_uses_env_var_when_set() {
        let mut g = TestEnvGuard::with_optional_vars(&[("CAS_CLOUD_ENDPOINT", None)]);
        g.set("CAS_CLOUD_ENDPOINT", "https://env.example.com");
        assert_eq!(default_endpoint(), "https://env.example.com");
    }

    #[test]
    fn default_endpoint_falls_back_when_env_empty() {
        let mut g = TestEnvGuard::with_optional_vars(&[("CAS_CLOUD_ENDPOINT", None)]);
        g.set("CAS_CLOUD_ENDPOINT", "");
        assert_eq!(
            default_endpoint(),
            "https://petra-stella-cloud.vercel.app",
            "empty CAS_CLOUD_ENDPOINT must not override the hardcoded fallback"
        );
    }

    #[test]
    fn default_endpoint_hardcoded_fallback() {
        let mut g = TestEnvGuard::with_optional_vars(&[("CAS_CLOUD_ENDPOINT", None)]);
        g.remove("CAS_CLOUD_ENDPOINT");
        assert_eq!(
            default_endpoint(),
            "https://petra-stella-cloud.vercel.app",
            "unset CAS_CLOUD_ENDPOINT must yield the Petra Stella default"
        );
    }

    #[test]
    fn default_endpoint_rejects_http_attacker() {
        let mut g = TestEnvGuard::with_optional_vars(&[("CAS_CLOUD_ENDPOINT", None)]);
        g.set("CAS_CLOUD_ENDPOINT", "http://attacker.com");
        assert_eq!(
            default_endpoint(),
            "https://petra-stella-cloud.vercel.app",
            "http://attacker.com must be rejected and fall back to default"
        );
    }

    #[test]
    fn default_endpoint_accepts_http_localhost() {
        let mut g = TestEnvGuard::with_optional_vars(&[("CAS_CLOUD_ENDPOINT", None)]);
        g.set("CAS_CLOUD_ENDPOINT", "http://127.0.0.1:3000");
        assert_eq!(
            default_endpoint(),
            "http://127.0.0.1:3000",
            "http://127.0.0.1 is whitelisted for e2e / dev servers"
        );
    }

    #[test]
    fn default_endpoint_rejects_file_scheme() {
        let mut g = TestEnvGuard::with_optional_vars(&[("CAS_CLOUD_ENDPOINT", None)]);
        g.set("CAS_CLOUD_ENDPOINT", "file:///etc/passwd");
        assert_eq!(
            default_endpoint(),
            "https://petra-stella-cloud.vercel.app",
            "file:// scheme must be rejected"
        );
    }

    #[test]
    fn default_endpoint_trims_whitespace_to_empty() {
        let mut g = TestEnvGuard::with_optional_vars(&[("CAS_CLOUD_ENDPOINT", None)]);
        g.set("CAS_CLOUD_ENDPOINT", "   ");
        assert_eq!(
            default_endpoint(),
            "https://petra-stella-cloud.vercel.app",
            "whitespace-only CAS_CLOUD_ENDPOINT must be treated as empty"
        );
    }

    // ── cas-6462: TeamInfo + CloudConfig.teams / default_team_id ───────────

    #[test]
    fn test_team_info_roundtrip() {
        // TeamInfo serialises and deserialises cleanly.
        let _guard = TestEnvGuard::new();
        let team = TeamInfo {
            id: "tid-abc".to_string(),
            slug: "petra-stella".to_string(),
            name: "Petra Stella".to_string(),
            role: "admin".to_string(),
        };
        let json = serde_json::to_string(&team).unwrap();
        let back: TeamInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back, team);
    }

    #[test]
    fn test_teams_and_default_team_id_roundtrip() {
        // CloudConfig with populated teams[] and default_team_id survives
        // save/load without data loss.
        let _guard = TestEnvGuard::new();
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("cloud.json");

        let mut config = CloudConfig {
            token: Some("tok".to_string()),
            ..Default::default()
        };
        config.teams = vec![
            TeamInfo {
                id: "tid-1".to_string(),
                slug: "team-one".to_string(),
                name: "Team One".to_string(),
                role: "member".to_string(),
            },
            TeamInfo {
                id: "tid-2".to_string(),
                slug: "team-two".to_string(),
                name: "Team Two".to_string(),
                role: "owner".to_string(),
            },
        ];
        config.default_team_id = Some("tid-1".to_string());

        config.save_to(&path).unwrap();
        let loaded = CloudConfig::load_from(&path).unwrap();

        assert_eq!(loaded.teams.len(), 2);
        assert_eq!(loaded.teams[0].id, "tid-1");
        assert_eq!(loaded.teams[0].slug, "team-one");
        assert_eq!(loaded.teams[0].name, "Team One");
        assert_eq!(loaded.teams[0].role, "member");
        assert_eq!(loaded.teams[1].id, "tid-2");
        assert_eq!(loaded.teams[1].role, "owner");
        assert_eq!(loaded.default_team_id, Some("tid-1".to_string()));
    }

    #[test]
    fn test_existing_cloud_json_without_teams_deserialises_to_defaults() {
        // Backwards compat: a cloud.json written before cas-6462 (no `teams`
        // or `default_team_id` keys) must deserialise without error, yielding
        // an empty Vec and None respectively.
        let _guard = TestEnvGuard::new();
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("cloud.json");

        // Simulate a legacy cloud.json with only the fields that existed before T1.
        std::fs::write(
            &path,
            r#"{"endpoint":"https://petra-stella-cloud.vercel.app","token":"old-tok"}"#,
        )
        .unwrap();

        let loaded = CloudConfig::load_from(&path).unwrap();
        assert_eq!(loaded.token, Some("old-tok".to_string()));
        assert!(loaded.teams.is_empty(), "teams must default to empty Vec");
        assert!(
            loaded.default_team_id.is_none(),
            "default_team_id must default to None"
        );
    }

    #[test]
    fn test_empty_teams_not_written_to_disk() {
        // When teams is empty and default_team_id is None, neither key should
        // appear in the serialised JSON — keeping legacy cloud.json files clean.
        let _guard = TestEnvGuard::new();
        let config = CloudConfig {
            token: Some("tok".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(
            !json.contains("\"teams\""),
            "empty teams must not appear in JSON, got: {json}"
        );
        assert!(
            !json.contains("\"default_team_id\""),
            "None default_team_id must not appear in JSON, got: {json}"
        );
    }

    #[test]
    fn resolve_canonical_id_prefers_config_toml_over_folder_name() {
        // Lock in the resolution-order change: config.toml beats folder name.
        let temp = tempfile::tempdir().unwrap();
        // Create the `.cas/` subdir so cas_root looks like a real Cassy root
        // (parent dir name = `quiet-leopard-46` or whatever — irrelevant).
        let cas_root = temp.path().join("project-dir");
        std::fs::create_dir_all(&cas_root).unwrap();
        set_canonical_id_in_config_toml(&cas_root, "github.com/owner/explicit").unwrap();

        assert_eq!(
            resolve_canonical_id(&cas_root).as_deref(),
            Some("github.com/owner/explicit"),
            "config.toml [project] canonical_id must win over folder-name fallback",
        );
    }

    // ── cas-8ca5: canonical-id adoption decision (contract §5) ─────────────

    #[test]
    fn adopt_when_remote_matches_and_id_differs() {
        // Unpinned machine: local remote == returned git_remote, server maps us
        // to the short canonical "ozer" → adopt it.
        assert_eq!(
            should_adopt_canonical_id(
                Some("github.com/richards-llc/ozer-health"),
                Some("github.com/richards-llc/ozer-health"),
                Some("ozer"),
                None,
            )
            .as_deref(),
            Some("ozer"),
        );
    }

    #[test]
    fn adopt_is_case_insensitive_on_remote_when_unpinned() {
        // The server lowercases git remotes; a fresh clone of a mixed-case
        // organization must still adopt its server-resolved bucket.
        assert_eq!(
            should_adopt_canonical_id(
                Some("github.com/Richards-LLC/ozer-health"),
                Some("github.com/richards-llc/ozer-health"),
                Some("ozer"),
                None,
            )
            .as_deref(),
            Some("ozer"),
        );
    }

    #[test]
    fn explicit_pin_blocks_adoption_even_when_remote_matches() {
        // `[project] canonical_id` is authoritative. The later team-push
        // path must not undo a server-resolved legacy bucket pin by re-homing
        // it to the remote-form identity.
        assert_eq!(
            should_adopt_canonical_id(
                Some("github.com/Richards-LLC/ozer-health"),
                Some("github.com/richards-llc/ozer-health"),
                Some("ozer"),
                Some("github.com/Richards-LLC/ozer-health"),
            ),
            None,
        );
    }

    #[test]
    fn no_adopt_when_already_pinned_correctly() {
        // Already on the canonical id → no-op (avoid a redundant config write).
        assert_eq!(
            should_adopt_canonical_id(
                Some("github.com/richards-llc/ozer-health"),
                Some("github.com/richards-llc/ozer-health"),
                Some("ozer"),
                Some("ozer"),
            ),
            None,
        );
    }

    #[test]
    fn no_adopt_when_remotes_differ() {
        // Safety gate: a shared machine whose remote differs from the returned
        // project must NOT be re-homed onto someone else's canonical id.
        assert_eq!(
            should_adopt_canonical_id(
                Some("github.com/someone-else/other-repo"),
                Some("github.com/richards-llc/ozer-health"),
                Some("ozer"),
                None,
            ),
            None,
        );
    }

    #[test]
    fn no_adopt_when_server_omits_fields() {
        // Older cloud build that doesn't return canonical_id / git_remote → skip.
        assert_eq!(
            should_adopt_canonical_id(Some("github.com/r/x"), None, None, None),
            None,
        );
        assert_eq!(
            should_adopt_canonical_id(Some("github.com/r/x"), Some("github.com/r/x"), None, None),
            None,
        );
    }

    #[test]
    fn no_adopt_when_no_local_remote() {
        // Non-git project (no origin) → nothing to match against → skip.
        assert_eq!(
            should_adopt_canonical_id(None, Some("github.com/r/x"), Some("x"), None),
            None,
        );
    }

    #[test]
    fn no_adopt_when_fields_empty_or_whitespace() {
        assert_eq!(
            should_adopt_canonical_id(Some("  "), Some("github.com/r/x"), Some("x"), None),
            None,
        );
        assert_eq!(
            should_adopt_canonical_id(
                Some("github.com/r/x"),
                Some("github.com/r/x"),
                Some("   "),
                None
            ),
            None,
        );
    }

    // ── cas-f8e3: personal-project promotion guard ────────────────────────────
    //
    // Regression coverage: a project without an explicit team link must NOT be
    // promoted to team scope via the user-level `default_team_id` or
    // single-team auto-pick, even when the user has a team configured globally.

    #[test]
    fn f8e3_personal_project_not_promoted_via_user_default_team_id() {
        let _guard = TestEnvGuard::new();
        // Simulates: user has `default_team_id` in ~/.cas/cloud.json but the
        // project's .cas/cloud.json has no team_id and no team_auto_promote.
        // This was the exact path that caused openclaw/penguinz to be promoted.
        let project_cfg = CloudConfig::default(); // no team_id, team_auto_promote=None
        let mut user_cfg = CloudConfig::default();
        user_cfg.default_team_id = Some("2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb".to_string());

        assert_eq!(
            project_cfg.active_team_id_with_user_config(Some(&user_cfg)),
            None,
            "cas-f8e3: personal project (no team_id, no team_auto_promote=Some(true)) \
             must write team_id=NULL even when user has default_team_id set"
        );
    }

    #[test]
    fn f8e3_personal_project_not_promoted_via_single_team_auto_pick() {
        let _guard = TestEnvGuard::new();
        // Simulates: user is a member of exactly 1 team (auto-pick previously
        // fired) but the project is personal.
        let project_cfg = CloudConfig::default(); // no team_id, team_auto_promote=None
        let mut user_cfg = CloudConfig::default();
        user_cfg.teams = vec![TeamInfo {
            id: "petra-stella-team".to_string(),
            slug: "petra-stella".to_string(),
            name: "Petra Stella".to_string(),
            role: "member".to_string(),
        }];

        assert_eq!(
            project_cfg.active_team_id_with_user_config(Some(&user_cfg)),
            None,
            "cas-f8e3: personal project must not be auto-picked into the user's \
             single team membership without team_auto_promote=Some(true)"
        );
    }

    #[test]
    fn f8e3_explicitly_linked_project_still_team_promoted() {
        let _guard = TestEnvGuard::new();
        // Sanity: a project with `team_id` set (via `cas cloud team set`) still
        // works correctly — Step 1 fires before the Step 1.5 guard.
        unsafe {
            std::env::set_var("CAS_USER_CLOUD_JSON", "/nonexistent/path/cloud.json");
        }
        let mut project_cfg = CloudConfig::default();
        project_cfg.set_team("2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb", "petra-stella");
        // No user-level config needed — Step 1 is sufficient.

        assert_eq!(
            project_cfg.active_team_id_with_user_config(None).as_deref(),
            Some("2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb"),
            "cas-f8e3: a project with explicit team_id must still be team-linked (Step 1)"
        );
        unsafe {
            std::env::remove_var("CAS_USER_CLOUD_JSON");
        }
    }

    #[test]
    fn f8e3_team_auto_promote_true_enables_user_level_fallback() {
        let _guard = TestEnvGuard::new();
        // Opt-in path: project sets team_auto_promote=Some(true) to explicitly
        // inherit the user-level team without running `cas cloud team set`.
        let mut project_cfg = CloudConfig::default(); // no team_id
        project_cfg.team_auto_promote = Some(true);
        let mut user_cfg = CloudConfig::default();
        user_cfg.default_team_id = Some("2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb".to_string());

        assert_eq!(
            project_cfg
                .active_team_id_with_user_config(Some(&user_cfg))
                .as_deref(),
            Some("2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb"),
            "cas-f8e3: team_auto_promote=Some(true) is the explicit opt-in for \
             user-level fallback when team_id is not set"
        );
    }

    #[test]
    fn f8e3_clear_team_resets_opt_in_making_project_personal() {
        let _guard = TestEnvGuard::new();
        // `cas cloud team clear` should leave the project in a state where
        // user-level auto-pick no longer fires.
        let mut cfg = CloudConfig::default();
        cfg.set_team("2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb", "petra-stella");
        cfg.team_auto_promote = Some(true); // was explicitly opted in
        cfg.clear_team();

        // After clear: team_id=None, team_auto_promote reset to None.
        assert!(cfg.team_id.is_none());
        assert!(
            !matches!(cfg.team_auto_promote, Some(true)),
            "clear_team must reset the explicit team opt-in so user-level auto-pick does not re-fire"
        );

        // With user-level config having default_team_id, project stays personal post-clear.
        let mut user_cfg = CloudConfig::default();
        user_cfg.default_team_id = Some("2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb".to_string());
        assert_eq!(
            cfg.active_team_id_with_user_config(Some(&user_cfg)),
            None,
            "after clear_team, project must be personal even with user-level default_team_id"
        );
    }
}
