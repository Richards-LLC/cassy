//! Opportunistic cross-repo sweep — the Unit 3 keystone.
//!
//! Runs at MCP server startup and factory daemon startup (the only remaining
//! "always-fires" lifecycle points in the Cassy architecture since the
//! standalone daemon was removed; see `cas-cli/src/daemon/mod.rs:11-13`).
//! Debounced via `~/.cas/last_global_sweep` mtime vs
//! `worktrees.global_sweep_debounce_secs` (default 3600s).
//!
//! **Semantics differ from `cas worktree sweep` (Unit 4).** Unit 4 is
//! user-invoked and uses merge status as the "safe to remove" signal.
//! This module runs unattended and uses **TTL-based reclaim** — a worktree
//! directory whose mtime is older than `worktrees.abandon_ttl_hours` is
//! considered abandoned regardless of merge status. Dirty worktrees are
//! captured with [`crate::worktree::salvage::salvage`] before removal so
//! no WIP is lost.
//!
//! Only `<repo>/.cas/worktrees/*` and (when
//! `worktrees.sweep_claude_agent_dirs`) `<repo>/.claude/worktrees/agent-*`
//! are touched. User-created worktrees anywhere else are never reclaimed.

use std::fs;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use tracing::{debug, info, warn};

use crate::config::WorktreesConfig;
use crate::store::known_repos::host_cas_dir;
use crate::worktree::discovery::list_tracked_repos;
use crate::worktree::{ExternalSymlink, salvage, scan_project_node_modules_symlinks_into};

/// Disposition for a single worktree encountered by the opportunistic
/// sweep. Narrower than `sweep::Disposition` because the TTL path has
/// fewer outcomes (no merge-status branches).
#[derive(Debug, Clone)]
pub enum OpportunisticOutcome {
    /// Directory age below TTL — preserved.
    Young { age_secs: u64 },
    /// Directory age above TTL, clean → removed.
    Reclaimed { bytes_freed: u64 },
    /// Directory age above TTL, dirty → salvaged + removed.
    Salvaged {
        patch_path: PathBuf,
        bytes_freed: u64,
    },
    /// Path was a symlink — refused.
    RefusedSymlink,
    /// A primary-checkout JavaScript dependency link resolves into this
    /// disposable worktree. Preserve it until the package-manager install is
    /// repaired rather than creating a dangling virtual-store entry.
    InboundSymlinksBlocked { links: Vec<ExternalSymlink> },
    /// Something blew up partway through; string captures the reason.
    Error { reason: String },
}

#[derive(Debug, Clone, Default)]
pub struct RepoOutcome {
    pub repo_root: PathBuf,
    pub entries: Vec<(PathBuf, OpportunisticOutcome)>,
    pub repo_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SweepSummary {
    pub repos_visited: usize,
    pub reclaimed: usize,
    pub salvaged: usize,
    pub young_preserved: usize,
    pub errors: usize,
    pub bytes_freed: u64,
    pub per_repo: Vec<RepoOutcome>,
}

impl SweepSummary {
    fn record(&mut self, repo: RepoOutcome) {
        self.repos_visited += 1;
        for (_, outcome) in &repo.entries {
            match outcome {
                OpportunisticOutcome::Young { .. } => self.young_preserved += 1,
                OpportunisticOutcome::Reclaimed { bytes_freed } => {
                    self.reclaimed += 1;
                    self.bytes_freed = self.bytes_freed.saturating_add(*bytes_freed);
                }
                OpportunisticOutcome::Salvaged { bytes_freed, .. } => {
                    self.salvaged += 1;
                    self.bytes_freed = self.bytes_freed.saturating_add(*bytes_freed);
                }
                OpportunisticOutcome::RefusedSymlink
                | OpportunisticOutcome::InboundSymlinksBlocked { .. }
                | OpportunisticOutcome::Error { .. } => {
                    self.errors += 1
                }
            }
        }
        self.per_repo.push(repo);
    }
}

/// Absolute path of the debounce marker file.
pub fn debounce_file() -> PathBuf {
    host_cas_dir().join("last_global_sweep")
}

/// Absolute path of the sweep log file.
pub fn log_file() -> PathBuf {
    host_cas_dir().join("logs").join("global-sweep.log")
}

/// Run the opportunistic sweep **if** it is due per the debounce file.
/// Callers (MCP boot, factory daemon boot) invoke this on a detached
/// Tokio task so startup is never blocked. Returns `Ok(None)` when the
/// sweep was skipped (not due).
///
/// The debounce file is touched on completion regardless of partial
/// errors — if we keep firing every boot despite repeated failures, the
/// user loses any startup-latency budget the debounce was meant to
/// protect. Per-repo failures are captured in the summary and logged.
pub fn run_if_due(config: &WorktreesConfig) -> Result<Option<SweepSummary>> {
    if !is_due(config.global_sweep_debounce_secs)? {
        debug!("opportunistic sweep skipped — not yet due");
        return Ok(None);
    }
    info!("opportunistic sweep starting");
    let summary = run_sweep(config)?;
    touch_debounce()?;
    append_log_line(summarize(&summary, config));
    info!(
        repos = summary.repos_visited,
        reclaimed = summary.reclaimed,
        salvaged = summary.salvaged,
        "opportunistic sweep complete",
    );
    Ok(Some(summary))
}

/// Force a sweep regardless of the debounce file. Exposed so tests and
/// Unit 3's Tokio spawn wrapper can exercise the main loop deterministically.
pub fn run_forced(config: &WorktreesConfig) -> Result<SweepSummary> {
    let summary = run_sweep(config)?;
    touch_debounce()?;
    append_log_line(summarize(&summary, config));
    Ok(summary)
}

/// Whether the sweep is due per the debounce marker. Returns `true` when
/// the marker does not exist yet.
pub fn is_due(debounce_secs: u64) -> Result<bool> {
    let path = debounce_file();
    let md = match fs::metadata(&path) {
        Ok(m) => m,
        Err(_) => return Ok(true),
    };
    let mtime = md.modified()?;
    let age = SystemTime::now()
        .duration_since(mtime)
        .unwrap_or(Duration::ZERO);
    Ok(age >= Duration::from_secs(debounce_secs))
}

fn touch_debounce() -> Result<()> {
    let path = debounce_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Re-writing contents is simpler than fiddling with utimensat and is
    // enough for mtime-based debounce; the file stays tiny.
    fs::write(&path, chrono::Utc::now().to_rfc3339())?;
    Ok(())
}

fn run_sweep(config: &WorktreesConfig) -> Result<SweepSummary> {
    let mut summary = SweepSummary::default();
    let repos = list_tracked_repos()?;
    for repo in repos {
        if !repo.healthy {
            let mut out = RepoOutcome::default();
            out.repo_root = repo.path.clone();
            out.repo_error = Some(format!(
                ".cas/ missing at {} — skipping",
                repo.path.display()
            ));
            summary.record(out);
            continue;
        }
        // Per-repo catch_unwind: one repo's panic must not poison the
        // host's entire startup path. Any unwind surfaces as repo_error.
        let cfg = config.clone();
        let path = repo.path.clone();
        let result = catch_unwind(AssertUnwindSafe(|| sweep_repo(&path, &cfg)));
        match result {
            Ok(outcome) => summary.record(outcome),
            Err(panic) => {
                let reason = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "non-string panic payload".to_string());
                warn!(repo = %repo.path.display(), %reason, "opportunistic sweep panicked in repo");
                let mut out = RepoOutcome::default();
                out.repo_root = repo.path.clone();
                out.repo_error = Some(format!("panic: {reason}"));
                summary.record(out);
            }
        }
    }
    Ok(summary)
}

fn sweep_repo(repo_root: &Path, config: &WorktreesConfig) -> RepoOutcome {
    let mut out = RepoOutcome {
        repo_root: repo_root.to_path_buf(),
        ..Default::default()
    };
    let ttl = Duration::from_secs(config.abandon_ttl_hours as u64 * 3600);

    let mut candidates: Vec<PathBuf> = Vec::new();
    let cas_wt_root = repo_root.join(".cas").join("worktrees");
    candidates.extend(list_worktree_dirs(&cas_wt_root, true));
    if config.sweep_claude_agent_dirs {
        let claude_wt_root = repo_root.join(".claude").join("worktrees");
        candidates.extend(
            list_worktree_dirs(&claude_wt_root, false)
                .into_iter()
                .filter(|p| {
                    p.file_name()
                        .and_then(|s| s.to_str())
                        .map(|n| n.starts_with("agent-"))
                        .unwrap_or(false)
                }),
        );
    }

    let mut touched_any = false;
    for path in candidates {
        touched_any = true;
        let outcome = match classify_and_act(repo_root, &path, ttl) {
            Ok(o) => o,
            Err(e) => OpportunisticOutcome::Error {
                reason: e.to_string(),
            },
        };
        out.entries.push((path, outcome));
    }

    if touched_any {
        if let Err(e) = prune_repo(repo_root) {
            warn!(repo = %repo_root.display(), error = %e, "git worktree prune failed");
        }
    }
    out
}

fn list_worktree_dirs(root: &Path, require_git_marker: bool) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(root) else {
        return Vec::new();
    };
    rd.flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            if require_git_marker && !path.join(".git").exists() {
                return None;
            }
            Some(path)
        })
        .collect()
}

fn classify_and_act(
    repo_root: &Path,
    worktree_path: &Path,
    ttl: Duration,
) -> std::io::Result<OpportunisticOutcome> {
    let md = fs::symlink_metadata(worktree_path)?;
    if md.file_type().is_symlink() {
        return Ok(OpportunisticOutcome::RefusedSymlink);
    }
    let age = SystemTime::now()
        .duration_since(md.modified()?)
        .unwrap_or(Duration::ZERO);
    if age < ttl {
        return Ok(OpportunisticOutcome::Young {
            age_secs: age.as_secs(),
        });
    }

    let links = scan_project_node_modules_symlinks_into(worktree_path, repo_root);
    if !links.is_empty() {
        return Ok(OpportunisticOutcome::InboundSymlinksBlocked { links });
    }

    let dirty = has_uncommitted_changes(worktree_path)?;
    let bytes = dir_size(worktree_path);
    let worker = worktree_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    if dirty {
        match salvage::salvage(worktree_path, repo_root, worker) {
            Ok(Some(outcome)) => {
                remove_worktree(repo_root, worktree_path)?;
                Ok(OpportunisticOutcome::Salvaged {
                    patch_path: outcome.patch_path,
                    bytes_freed: bytes,
                })
            }
            Ok(None) => {
                // Raced clean between status and salvage — proceed with plain remove.
                remove_worktree(repo_root, worktree_path)?;
                Ok(OpportunisticOutcome::Reclaimed { bytes_freed: bytes })
            }
            Err(e) => Ok(OpportunisticOutcome::Error {
                reason: format!("salvage failed: {e}"),
            }),
        }
    } else {
        remove_worktree(repo_root, worktree_path)?;
        Ok(OpportunisticOutcome::Reclaimed { bytes_freed: bytes })
    }
}

fn has_uncommitted_changes(worktree_path: &Path) -> std::io::Result<bool> {
    let out = Command::new("git")
        .args(["status", "--porcelain=v1"])
        .current_dir(worktree_path)
        .output()?;
    if !out.status.success() {
        // Non-git directory or corrupt worktree — treat as "needs salvage".
        return Ok(true);
    }
    Ok(!out.stdout.is_empty())
}

fn remove_worktree(repo_root: &Path, worktree_path: &Path) -> std::io::Result<()> {
    let out = Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            worktree_path
                .to_str()
                .ok_or_else(|| std::io::Error::other("non-utf8 path"))?,
        ])
        .current_dir(repo_root)
        .output()?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    fs::remove_dir_all(worktree_path).map_err(|e| {
        std::io::Error::other(format!(
            "git worktree remove failed ({stderr}) and fallback rm-rf also failed: {e}"
        ))
    })
}

fn prune_repo(repo_root: &Path) -> std::io::Result<()> {
    let out = Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(repo_root)
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::other(
            String::from_utf8_lossy(&out.stderr).to_string(),
        ));
    }
    Ok(())
}

fn dir_size(path: &Path) -> u64 {
    fn walk(p: &Path) -> u64 {
        let Ok(md) = fs::symlink_metadata(p) else {
            return 0;
        };
        if md.is_symlink() {
            return md.len();
        }
        if md.is_dir() {
            let mut total: u64 = 0;
            if let Ok(rd) = fs::read_dir(p) {
                for e in rd.flatten() {
                    total = total.saturating_add(walk(&e.path()));
                }
            }
            total
        } else {
            md.len()
        }
    }
    walk(path)
}

fn summarize(s: &SweepSummary, config: &WorktreesConfig) -> String {
    format!(
        "{ts} opportunistic-sweep ttl={ttl}h debounce={dbn}s repos={repos} reclaimed={rec} salvaged={sal} young={young} errors={err} bytes={bytes}",
        ts = chrono::Utc::now().to_rfc3339(),
        ttl = config.abandon_ttl_hours,
        dbn = config.global_sweep_debounce_secs,
        repos = s.repos_visited,
        rec = s.reclaimed,
        sal = s.salvaged,
        young = s.young_preserved,
        err = s.errors,
        bytes = s.bytes_freed,
    )
}

fn append_log_line(line: String) {
    let path = log_file();
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            warn!(error = %e, dir = %parent.display(), "could not create log dir");
            return;
        }
    }
    match fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{line}") {
                warn!(error = %e, path = %path.display(), "sweep log write failed");
            }
        }
        Err(e) => warn!(error = %e, path = %path.display(), "sweep log open failed"),
    }
}

#[cfg(test)]
mod tests;
