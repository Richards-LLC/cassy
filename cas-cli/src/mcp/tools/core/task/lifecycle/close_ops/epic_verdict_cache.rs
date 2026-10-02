//! Persisted per-child epic-close verdicts (cas-b412).
//!
//! The epic close gate proves every child's delivery against the target
//! within [`super::EPIC_CLOSE_GATE_BUDGET`]. Content proofs cost a few hundred
//! milliseconds per child, so an epic with 140 children cannot be evaluated in
//! one call. Before this cache every retry restarted at child zero and stopped
//! at the same place, so a shipped epic could never close.
//!
//! A child's verdict is a pure function of the Git objects its proofs read:
//! the child's delivery target (local and `origin/`), each branch the child
//! is measured on (local and `origin/`), and its immutable recorded anchor.
//! The cache key is a digest of exactly those inputs, taken from the epic's
//! one-shot ref snapshot, plus the task fields that select them. A hit is
//! therefore the verdict the gate would compute again; any moved ref, changed
//! anchor, retarget or reassignment produces a different key and a fresh
//! proof. Each retry reuses the verdicts already proven and spends its budget
//! on the children it has not yet reached, so the check resumes across calls.
//!
//! Only stable verdicts are stored: terminal children (whose refs no longer
//! move under an active worker) without a content-proof error or an Unknown
//! branch measurement, both of which can be transient. The file lives in the
//! repository's common Git directory, so worktrees of one repository share it
//! and nothing appears in the working tree. Cache I/O failures never change a
//! verdict; they only cost a recomputation.

use super::{BranchContentDirection, EpicChildBranchStatus, EpicGitSnapshot};
use cas_types::Task;
use super::epic_measurement::CommandExt as _;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Bump when the verdict semantics or the key inputs change.
const CACHE_VERSION: u32 = 1;
/// Bound on stored verdicts; the oldest are evicted first.
const MAX_ENTRIES: usize = 8192;
const CACHE_DIR: &str = "cas";
const CACHE_FILE: &str = "epic-close-verdicts.json";

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct CacheFile {
    version: u32,
    entries: BTreeMap<String, CachedVerdict>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CachedVerdict {
    stored_unix: i64,
    status: EpicChildBranchStatus,
}

/// Verdicts loaded for one epic-status collection, plus the ones it proved.
#[derive(Debug, Default)]
pub(super) struct EpicVerdictCache {
    path: Option<PathBuf>,
    entries: BTreeMap<String, CachedVerdict>,
    proven: BTreeMap<String, CachedVerdict>,
}

impl EpicVerdictCache {
    /// A cache that never stores or returns anything (summary views).
    pub(super) fn disabled() -> Self {
        Self::default()
    }

    pub(super) fn load(repo_path: &Path) -> Self {
        let Some(path) = cache_path(repo_path) else {
            return Self::disabled();
        };
        let entries = read_entries(&path);
        Self {
            path: Some(path),
            entries,
            proven: BTreeMap::new(),
        }
    }

    pub(super) fn get(&self, key: &str) -> Option<EpicChildBranchStatus> {
        self.path.as_ref()?;
        self.proven
            .get(key)
            .or_else(|| self.entries.get(key))
            .map(|entry| entry.status.clone())
    }

    /// Record a freshly proven verdict when it is stable enough to reuse.
    pub(super) fn record(&mut self, key: String, child: &Task, status: &EpicChildBranchStatus) {
        if self.path.is_none() || !verdict_is_cacheable(child, status) {
            return;
        }
        self.proven.insert(
            key,
            CachedVerdict {
                stored_unix: chrono::Utc::now().timestamp(),
                status: status.clone(),
            },
        );
    }

    /// Merge the verdicts proven by this collection into the file. The file
    /// is re-read first so concurrent collectors lose at most each other's
    /// newest entries, never the file, and the write is an atomic rename.
    pub(super) fn persist(self) {
        let Some(path) = self.path else {
            return;
        };
        if self.proven.is_empty() {
            return;
        }
        let mut entries = read_entries(&path);
        entries.extend(self.proven);
        if entries.len() > MAX_ENTRIES {
            let mut by_age: Vec<(i64, String)> = entries
                .iter()
                .map(|(key, entry)| (entry.stored_unix, key.clone()))
                .collect();
            by_age.sort();
            for (_, key) in by_age.into_iter().take(entries.len() - MAX_ENTRIES) {
                entries.remove(&key);
            }
        }
        let _ = write_entries(&path, entries);
    }
}

/// Digest of every input the child's verdict depends on. `branches` are the
/// branch names the collector measures for this child, in collector order.
pub(super) fn verdict_key(
    child: &Task,
    target_branch: &str,
    has_delivery: bool,
    recorded_anchor: Option<&str>,
    anchor_resolved: bool,
    branches: &[String],
    snapshot: &EpicGitSnapshot,
) -> String {
    use sha2::Digest;
    use std::fmt::Write as _;

    let commit = |refname: &str| {
        snapshot
            .ref_info(refname)
            .map(|info| info.commit.clone())
            .unwrap_or_else(|| "-".to_string())
    };
    let mut material = String::new();
    let _ = writeln!(material, "v{CACHE_VERSION}");
    let _ = writeln!(material, "id={}", child.id);
    let _ = writeln!(material, "status={:?}", child.status);
    let _ = writeln!(material, "assignee={:?}", child.assignee);
    let _ = writeln!(material, "delivery={has_delivery}");
    let _ = writeln!(
        material,
        "target={target_branch} {} {}",
        commit(target_branch),
        commit(&format!("origin/{target_branch}"))
    );
    let _ = writeln!(material, "anchor={recorded_anchor:?} resolved={anchor_resolved}");
    for branch in branches {
        let _ = writeln!(
            material,
            "branch={branch} {} {}",
            commit(branch),
            commit(&format!("origin/{branch}"))
        );
    }
    let digest = sha2::Sha256::digest(material.as_bytes());
    let mut key = String::with_capacity(64);
    for byte in digest {
        let _ = write!(key, "{byte:02x}");
    }
    key
}

fn verdict_is_cacheable(child: &Task, status: &EpicChildBranchStatus) -> bool {
    child.is_terminal()
        && status.content_check_error.is_none()
        && !status
            .content_directions
            .iter()
            .any(|(_, direction)| matches!(direction, BranchContentDirection::Unknown { .. }))
}

fn cache_path(repo_path: &Path) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(repo_path)
        .measurement_output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let common = String::from_utf8(output.stdout).ok()?;
    let common = common.trim();
    if common.is_empty() {
        return None;
    }
    let common = Path::new(common);
    let common = if common.is_absolute() {
        common.to_path_buf()
    } else {
        repo_path.join(common)
    };
    Some(common.join(CACHE_DIR).join(CACHE_FILE))
}

fn read_entries(path: &Path) -> BTreeMap<String, CachedVerdict> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CacheFile>(&bytes).ok())
        .filter(|file| file.version == CACHE_VERSION)
        .map(|file| file.entries)
        .unwrap_or_default()
}

fn write_entries(path: &Path, entries: BTreeMap<String, CachedVerdict>) -> std::io::Result<()> {
    let dir = path.parent().ok_or(std::io::ErrorKind::NotFound)?;
    std::fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec(&CacheFile {
        version: CACHE_VERSION,
        entries,
    })
    .map_err(std::io::Error::other)?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    std::io::Write::write_all(&mut temp, &bytes)?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}
