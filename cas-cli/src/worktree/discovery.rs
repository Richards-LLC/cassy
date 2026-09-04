//! Cross-repo discovery for sweep tooling.
//!
//! Wraps the host-scoped [`crate::store::known_repos`] helpers with a
//! disk-health check so callers (`cas worktree sweep --all-repos`,
//! `cas sweep-all`, Unit 3's opportunistic trigger) get a filtered list of
//! repos they can actually act on. Also exposes the `seed` fallback used by
//! `cas known-repos seed` for hosts that adopted the registry after their
//! repos were already in use.

use std::path::{Path, PathBuf};

use anyhow::Result;
use tracing::debug;

use crate::store::KnownRepoStore;
use crate::store::known_repos::{host_cas_dir, open_host_known_repo_store};

/// A repository discovered via the host registry.
#[derive(Debug, Clone)]
pub struct DiscoveredRepo {
    /// Canonical absolute path to the repo root.
    pub path: PathBuf,
    /// `true` if `<path>/.cas/` exists on disk right now.
    pub healthy: bool,
    /// Number of registry upserts recorded for this path.
    pub touch_count: u64,
}

/// List every known repo, tagging each with a health flag (`.cas/` exists
/// on disk). Callers can filter on `healthy` to skip repos that have been
/// deleted or moved.
///
/// Returns repos in `last_touched_at DESC` order (most-recently-used first),
/// which matches how `cas sweep-all` wants to process them.
pub fn list_tracked_repos() -> Result<Vec<DiscoveredRepo>> {
    let store = open_host_known_repo_store()?;
    let records = store.list()?;
    Ok(records
        .into_iter()
        .map(|r| DiscoveredRepo {
            healthy: r.path.join(".cas").is_dir(),
            path: r.path,
            touch_count: r.touch_count,
        })
        .collect())
}

/// Result of a seed run.
#[derive(Debug, Default, Clone)]
pub struct SeedReport {
    /// Paths that were upserted for the first time.
    pub new: Vec<PathBuf>,
    /// Paths already present (still touched).
    pub existing: Vec<PathBuf>,
    /// Paths rejected because `<path>/.cas/` is not a directory.
    pub skipped_missing: Vec<PathBuf>,
    /// Paths rejected as disposable roots (factory artifacts, `~/.cas/scratch`,
    /// a temp root or an unpinned no-remote container), paired with the reason. These have a real `.cas/`
    /// — they are copies of a project, not projects (cas-647c).
    pub skipped_disposable: Vec<(PathBuf, String)>,
}

impl SeedReport {
    pub fn total_considered(&self) -> usize {
        self.new.len()
            + self.existing.len()
            + self.skipped_missing.len()
            + self.skipped_disposable.len()
    }
}

/// Seed the registry from existing host state.
///
/// Sources, unioned and deduplicated by canonical path:
/// 1. `sessions.cwd` on the host `~/.cas/cas.db` (written by Claude Code
///    hooks on every session start).
/// 2. `project_dir` from every `~/.cas/sessions/*.json` factory summary.
///
/// Only paths where `<path>/.cas/` is a real directory are inserted; the
/// rest are reported in [`SeedReport::skipped_missing`].
///
/// If `include_home_scan` is `true`, additionally scan `$HOME` up to 5
/// levels deep for `.cas` directories. This is slow (seconds on a busy
/// home dir) and must be gated behind an explicit CLI flag.
pub fn seed(include_home_scan: bool) -> Result<SeedReport> {
    let store = open_host_known_repo_store()?;
    let already: std::collections::HashSet<PathBuf> =
        store.list()?.into_iter().map(|r| r.path).collect();

    let mut candidates: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();

    // Source 1: sessions.cwd on host DB.
    for p in session_cwds_from_host_db()? {
        candidates.insert(p);
    }
    // Source 2: factory SessionSummary JSON.
    for p in project_dirs_from_session_json()? {
        candidates.insert(p);
    }
    // Source 3 (opt-in): filesystem scan.
    if include_home_scan {
        for p in home_cas_scan() {
            candidates.insert(p);
        }
    }

    let mut report = SeedReport::default();
    for cand in candidates {
        if !cand.join(".cas").is_dir() {
            report.skipped_missing.push(cand);
            continue;
        }
        if let Some(skip) = crate::store::known_repos::registry_skip(&cand) {
            debug!(
                path = %cand.display(),
                reason = %skip.reason(),
                "seed skipped a disposable root",
            );
            report.skipped_disposable.push((cand, skip.reason()));
            continue;
        }
        let canonical = cand.canonicalize().unwrap_or(cand.clone());
        let is_new = !already.contains(&canonical);
        store.upsert(&cand)?;
        if is_new {
            report.new.push(canonical);
        } else {
            report.existing.push(canonical);
        }
    }
    debug!(
        new = report.new.len(),
        existing = report.existing.len(),
        skipped = report.skipped_missing.len(),
        skipped_disposable = report.skipped_disposable.len(),
        "known_repos seed complete",
    );
    Ok(report)
}

/// Raw-SQL read of distinct `sessions.cwd` from the host DB. Routes through
/// the shared_db pool so we inherit the 5s busy_timeout + WAL pragmas —
/// without it, a seed invoked while a factory daemon holds a write lock
/// would return `SQLITE_BUSY` immediately and silently report zero rows.
/// Schema may have evolved; a missing `sessions` table is treated as empty.
fn session_cwds_from_host_db() -> Result<Vec<PathBuf>> {
    let db_path = host_cas_dir().join("cas.db");
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let conn = cas_store::shared_db::shared_connection(&db_path)?;
    let conn = conn.lock().unwrap_or_else(|p| p.into_inner());
    // sessions table may not exist on very fresh hosts.
    let has_sessions: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sessions'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if has_sessions == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare("SELECT DISTINCT cwd FROM sessions WHERE cwd IS NOT NULL")?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .collect();
    Ok(rows)
}

/// Parse `project_dir` out of every `~/.cas/sessions/*.json`.
fn project_dirs_from_session_json() -> Result<Vec<PathBuf>> {
    let sessions_dir = host_cas_dir().join("sessions");
    if !sessions_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&sessions_dir)? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if let Some(dir) = value.get("project_dir").and_then(|v| v.as_str()) {
            let pb = PathBuf::from(dir);
            if pb.is_absolute() {
                out.push(pb);
            }
        }
    }
    Ok(out)
}

/// Scan `$HOME` up to 5 levels for `.cas/` directories. Slow; gated behind
/// an explicit flag in the CLI.
fn home_cas_scan() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    walk(&home, 0, 5, &mut found);
    found
}

fn walk(dir: &Path, depth: usize, max: usize, out: &mut Vec<PathBuf>) {
    if depth >= max {
        return;
    }
    // Skip obvious speedbumps.
    if let Some(name) = dir.file_name().and_then(|s| s.to_str()) {
        if matches!(
            name,
            "node_modules" | "target" | ".git" | ".venv" | "venv" | ".next" | "dist" | "build"
        ) {
            return;
        }
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.file_name().and_then(|s| s.to_str()) == Some(".cas") {
            if let Some(parent) = path.parent() {
                out.push(parent.to_path_buf());
            }
            continue; // don't descend into .cas itself
        }
        walk(&path, depth + 1, max, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::known_repos::ensure_host_schema;
    use crate::test_support::TestEnvGuard;

    #[test]
    fn list_tracked_flags_healthy_correctly() {
        TestEnvGuard::run_with_temp_home(|home| {
            ensure_host_schema().unwrap();
            let healthy = home.join("healthy");
            let moved = home.join("moved");
            std::fs::create_dir_all(healthy.join(".cas")).unwrap();
            std::fs::create_dir_all(&moved).unwrap(); // no .cas/ → unhealthy

            let store = open_host_known_repo_store().unwrap();
            store.upsert(&healthy).unwrap();
            store.upsert(&moved).unwrap();

            let list = list_tracked_repos().unwrap();
            assert_eq!(list.len(), 2);
            let healthy_entry = list.iter().find(|r| r.path.ends_with("healthy")).unwrap();
            let moved_entry = list.iter().find(|r| r.path.ends_with("moved")).unwrap();
            assert!(healthy_entry.healthy);
            assert!(!moved_entry.healthy);
        });
    }

    #[test]
    fn seed_skips_missing_cas_dir() {
        TestEnvGuard::run_with_temp_home(|home| {
            ensure_host_schema().unwrap();
            // Seed source: a session JSON pointing at a real repo and a fake one.
            let sessions_dir = home.join(".cas/sessions");
            std::fs::create_dir_all(&sessions_dir).unwrap();
            let real = home.join("real-repo");
            std::fs::create_dir_all(real.join(".cas")).unwrap();
            let fake = home.join("fake-repo"); // no .cas/

            std::fs::write(
                sessions_dir.join("a.json"),
                serde_json::json!({ "project_dir": real.to_string_lossy() }).to_string(),
            )
            .unwrap();
            std::fs::write(
                sessions_dir.join("b.json"),
                serde_json::json!({ "project_dir": fake.to_string_lossy() }).to_string(),
            )
            .unwrap();

            let report = seed(false).unwrap();
            assert_eq!(report.new.len(), 1, "only real repo seeded");
            assert_eq!(report.skipped_missing.len(), 1);
            assert!(report.new[0].ends_with("real-repo"));
        });
    }

    /// cas-647c: a factory task that copies a CAS root into
    /// `~/.cas/artifacts/<task>/` leaves a `.cas/` behind, and a session JSON
    /// pointing at it. Seeding must not adopt the copy as a host project — it
    /// is a fixture, and every later host-wide sweep would treat it as live.
    #[test]
    fn seed_skips_disposable_artifact_and_scratch_roots_cas_647c() {
        TestEnvGuard::run_with_temp_home(|home| {
            ensure_host_schema().unwrap();
            let sessions_dir = home.join(".cas/sessions");
            std::fs::create_dir_all(&sessions_dir).unwrap();
            let real = home.join("real-repo");
            let fixture = home.join(".cas/artifacts/cas-1bfb/fresh-proxy");
            let sandbox = home.join(".cas/scratch/probe");
            for root in [&real, &fixture, &sandbox] {
                std::fs::create_dir_all(root.join(".cas")).unwrap();
            }
            for (name, root) in [
                ("a.json", &real),
                ("b.json", &fixture),
                ("c.json", &sandbox),
            ] {
                std::fs::write(
                    sessions_dir.join(name),
                    serde_json::json!({ "project_dir": root.to_string_lossy() }).to_string(),
                )
                .unwrap();
            }

            let report = seed(false).unwrap();

            assert_eq!(report.new.len(), 1, "only the real repo is a project");
            assert!(report.new[0].ends_with("real-repo"));
            assert_eq!(report.skipped_missing.len(), 0);
            assert_eq!(report.skipped_disposable.len(), 2);
            let named: Vec<String> = report
                .skipped_disposable
                .iter()
                .map(|(path, _)| path.display().to_string())
                .collect();
            assert!(named.iter().any(|p| p.contains("fresh-proxy")), "{named:?}");
            assert!(named.iter().any(|p| p.contains("scratch")), "{named:?}");
            assert_eq!(open_host_known_repo_store().unwrap().count().unwrap(), 1);
        });
    }

    #[test]
    fn seed_idempotent_second_run_has_no_new() {
        TestEnvGuard::run_with_temp_home(|home| {
            ensure_host_schema().unwrap();
            let sessions_dir = home.join(".cas/sessions");
            std::fs::create_dir_all(&sessions_dir).unwrap();
            let repo = home.join("repo");
            std::fs::create_dir_all(repo.join(".cas")).unwrap();
            std::fs::write(
                sessions_dir.join("s.json"),
                serde_json::json!({ "project_dir": repo.to_string_lossy() }).to_string(),
            )
            .unwrap();

            let first = seed(false).unwrap();
            assert_eq!(first.new.len(), 1);
            assert_eq!(first.existing.len(), 0);

            let second = seed(false).unwrap();
            assert_eq!(second.new.len(), 0);
            assert_eq!(second.existing.len(), 1);
        });
    }

    #[test]
    fn list_empty_registry_returns_empty() {
        TestEnvGuard::run_with_temp_home(|_| {
            ensure_host_schema().unwrap();
            let list = list_tracked_repos().unwrap();
            assert!(list.is_empty());
        });
    }

    #[test]
    fn seed_ignores_corrupt_session_json() {
        TestEnvGuard::run_with_temp_home(|home| {
            ensure_host_schema().unwrap();
            let sessions_dir = home.join(".cas/sessions");
            std::fs::create_dir_all(&sessions_dir).unwrap();
            std::fs::write(sessions_dir.join("good.json"), "{}").unwrap(); // no project_dir
            std::fs::write(sessions_dir.join("broken.json"), "{not json").unwrap();
            std::fs::write(sessions_dir.join("bad.txt"), "ignored").unwrap();

            // Must not panic; must not error.
            let report = seed(false).unwrap();
            assert_eq!(report.total_considered(), 0);
        });
    }
}
