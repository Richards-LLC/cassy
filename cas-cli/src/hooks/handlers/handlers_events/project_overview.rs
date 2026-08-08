//! Project-overview freshness detection.
//!
//! Mirrors `codemap.rs` but targets `docs/PRODUCT_OVERVIEW.md` and filters changes
//! through a configurable `[project_overview]` section in `.cas/config.toml`
//! (`domain_paths` = any-change triggers, `watch_dirs` = structural-only triggers).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Represents a project-overview-relevant file change detected from git.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectOverviewChange {
    /// "A" (added), "D" (deleted), "R" (renamed), "M" (modified — only for
    /// files listed in `domain_paths`).
    #[serde(rename = "type")]
    pub change_type: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

/// A single pending entry (JSONL — one JSON object per line).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectOverviewPending {
    pub changes: Vec<ProjectOverviewChange>,
    pub commit: String,
    pub recorded_at: String,
}

/// Pending-file path, relative to `.cas/`.
const PENDING_FILE: &str = "project-overview-pending.json";

/// Path to the target doc, relative to project root.
pub(crate) const DOC_PATH: &str = "docs/PRODUCT_OVERVIEW.md";

/// Stale at or above this change count → `SignificantlyStale`.
/// Lower than codemap's 10 because schema/layout changes are more impactful.
const SIGNIFICANT_STALENESS_THRESHOLD: usize = 5;

// ---------------------------------------------------------------------------
// Watch-target configuration
// ---------------------------------------------------------------------------

/// Files and directory globs that should trigger project-overview staleness.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchTargets {
    /// Exact file paths where any change (M/A/D/R) counts.
    pub domain_paths: Vec<String>,
    /// Directory globs where only structural changes (A/D/R) count.
    pub watch_dirs: Vec<String>,
}

impl WatchTargets {
    fn defaults() -> Self {
        Self {
            domain_paths: vec![
                "apps/backend/prisma/schema.prisma".to_string(),
                "prisma/schema.prisma".to_string(),
            ],
            watch_dirs: vec![
                "apps/*/src/components".to_string(),
                "apps/*/src/pages".to_string(),
                "app".to_string(),
                "components/dashboard".to_string(),
            ],
        }
    }

    /// True when `path` matches a `domain_paths` entry (exact string match).
    pub fn is_domain_path(&self, path: &str) -> bool {
        self.domain_paths.iter().any(|p| p == path)
    }

    /// True when `path` sits inside any `watch_dirs` glob.
    pub fn is_in_watch_dir(&self, path: &str) -> bool {
        for pat in &self.watch_dirs {
            if dir_glob_matches(pat, path) {
                return true;
            }
        }
        false
    }
}

#[derive(Debug, serde::Deserialize, Default)]
struct RawConfig {
    project_overview: Option<RawProjectOverview>,
}

#[derive(Debug, serde::Deserialize)]
struct RawProjectOverview {
    domain_paths: Option<Vec<String>>,
    watch_dirs: Option<Vec<String>>,
}

/// Load watch targets from `<repo_root>/.cas/config.toml`, falling back to defaults.
pub fn watch_targets(repo_root: &Path) -> WatchTargets {
    let config_path = repo_root.join(".cas").join("config.toml");
    let Ok(content) = std::fs::read_to_string(&config_path) else {
        return WatchTargets::defaults();
    };
    let Ok(raw): Result<RawConfig, _> = toml::from_str(&content) else {
        return WatchTargets::defaults();
    };
    let Some(po) = raw.project_overview else {
        return WatchTargets::defaults();
    };
    let defaults = WatchTargets::defaults();
    WatchTargets {
        domain_paths: po.domain_paths.unwrap_or(defaults.domain_paths),
        watch_dirs: po
            .watch_dirs
            .map(sanitize_watch_dirs)
            .unwrap_or(defaults.watch_dirs),
    }
}

/// Strip degenerate globs like `*` / `**` / empty strings that would match every
/// path in the repo and turn freshness detection into a full-scan DoS on every
/// SessionStart. Silent — the caller falls back to the remaining patterns.
fn sanitize_watch_dirs(patterns: Vec<String>) -> Vec<String> {
    patterns
        .into_iter()
        .filter(|p| {
            let trimmed = p.trim();
            !trimmed.is_empty() && trimmed != "*" && trimmed != "**"
        })
        .collect()
}

/// Match a directory glob (like `apps/*/src/components`) against a file path.
///
/// A match requires the path to sit *inside* the pattern's directory — the
/// pattern itself is treated as a prefix.
fn dir_glob_matches(pattern: &str, path: &str) -> bool {
    // Try `pattern/**` (files under the directory).
    let recursive = format!("{pattern}/**");
    if let Ok(p) = glob::Pattern::new(&recursive) {
        if p.matches(path) {
            return true;
        }
    }
    // Also allow the path to equal the pattern itself (directory file entry).
    if let Ok(p) = glob::Pattern::new(pattern) {
        if p.matches(path) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Change-filter helpers
// ---------------------------------------------------------------------------

/// Apply `WatchTargets` to a raw list of changes.
///
/// Rules:
/// * `domain_paths` entries — keep every change type (M/A/D/R).
/// * `watch_dirs` entries — keep only structural changes (A/D/R); drop M.
/// * Anything else — drop.
fn filter_changes(
    changes: Vec<ProjectOverviewChange>,
    targets: &WatchTargets,
) -> Vec<ProjectOverviewChange> {
    changes
        .into_iter()
        .filter(|c| {
            if targets.is_domain_path(&c.path) {
                return true;
            }
            // Also check old_path for renames.
            if let Some(op) = &c.old_path {
                if targets.is_domain_path(op) {
                    return true;
                }
            }
            let structural = matches!(c.change_type.as_str(), "A" | "D")
                || c.change_type.starts_with('R');
            if !structural {
                return false;
            }
            targets.is_in_watch_dir(&c.path)
                || c.old_path
                    .as_deref()
                    .map(|op| targets.is_in_watch_dir(op))
                    .unwrap_or(false)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Git helpers (adapted from codemap.rs)
// ---------------------------------------------------------------------------

fn cas_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(".cas")
}

fn pending_path(repo_root: &Path) -> PathBuf {
    cas_dir(repo_root).join(PENDING_FILE)
}

/// `git rev-parse HEAD` — returns the current commit hash.
fn head_commit(repo_root: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if hash.is_empty() { None } else { Some(hash) }
}

/// `git diff-tree --name-status` for `commit` against its parent (with
/// root-commit fallback).
fn diff_tree_changes(
    repo_root: &Path,
    commit: &str,
) -> Option<Vec<ProjectOverviewChange>> {
    let out = std::process::Command::new("git")
        .args([
            "diff-tree",
            "--name-status",
            "-r",
            "--no-commit-id",
            &format!("{commit}~1"),
            commit,
        ])
        .current_dir(repo_root)
        .output()
        .ok()?;

    if !out.status.success() {
        // Fall back for the initial commit (no parent).
        let out = std::process::Command::new("git")
            .args([
                "diff-tree",
                "--name-status",
                "-r",
                "--no-commit-id",
                "--root",
                commit,
            ])
            .current_dir(repo_root)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        return Some(parse_diff_tree_output(&String::from_utf8_lossy(
            &out.stdout,
        )));
    }

    Some(parse_diff_tree_output(&String::from_utf8_lossy(
        &out.stdout,
    )))
}

/// Parse `git diff-tree --name-status` into raw changes (keeps M/A/D/R; M will
/// be filtered out for non-domain paths later).
fn parse_diff_tree_output(output: &str) -> Vec<ProjectOverviewChange> {
    let mut changes = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }
        let status = parts[0];
        match status {
            "A" | "D" | "M" => {
                changes.push(ProjectOverviewChange {
                    change_type: status.to_string(),
                    path: parts[1].to_string(),
                    old_path: None,
                });
            }
            s if s.starts_with('R') && parts.len() >= 3 => {
                changes.push(ProjectOverviewChange {
                    change_type: "R".to_string(),
                    path: parts[2].to_string(),
                    old_path: Some(parts[1].to_string()),
                });
            }
            _ => {}
        }
    }
    changes
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Detect project-overview-relevant changes in HEAD and append them to the
/// pending JSONL file.
///
/// Intended to be called from PostToolUse after a successful git commit.
/// Errors (missing git, parse failures) surface as `Err`; callers in hook
/// contexts should ignore them silently.
pub fn detect_structural_changes(repo_root: &Path) -> Result<()> {
    let Some(commit) = head_commit(repo_root) else {
        return Ok(()); // no HEAD yet (fresh repo) — nothing to do
    };

    let Some(raw) = diff_tree_changes(repo_root, &commit) else {
        return Ok(());
    };

    let targets = watch_targets(repo_root);
    let filtered = filter_changes(raw, &targets);
    if filtered.is_empty() {
        return Ok(());
    }

    let pending = ProjectOverviewPending {
        changes: filtered,
        commit,
        recorded_at: chrono::Utc::now().to_rfc3339(),
    };

    let line =
        serde_json::to_string(&pending).context("serialize project-overview pending entry")?;
    let line = format!("{line}\n");

    let cas_dir = cas_dir(repo_root);
    std::fs::create_dir_all(&cas_dir).context("create .cas dir for project-overview pending")?;

    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(pending_path(repo_root))
        .context("open project-overview-pending.json")?;
    f.write_all(line.as_bytes())
        .context("append project-overview pending entry")?;
    Ok(())
}

/// Read and parse the pending JSONL file, if present.
pub fn read_pending(repo_root: &Path) -> Result<Vec<ProjectOverviewPending>> {
    let path = pending_path(repo_root);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: ProjectOverviewPending = serde_json::from_str(line)
            .with_context(|| format!("parse pending entry: {line}"))?;
        out.push(entry);
    }
    Ok(out)
}

/// Remove the pending file (idempotent).
pub fn clear_pending(repo_root: &Path) -> Result<()> {
    let path = pending_path(repo_root);
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Staleness
// ---------------------------------------------------------------------------

/// Staleness severity for project-overview freshness checks.
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectOverviewStaleness {
    /// `docs/PRODUCT_OVERVIEW.md` does not exist.
    Missing,
    /// Stale with fewer than `SIGNIFICANT_STALENESS_THRESHOLD` changes.
    Stale {
        total_changes: usize,
        file_list: Vec<String>,
        commit_info: String,
    },
    /// Stale with `>= SIGNIFICANT_STALENESS_THRESHOLD` changes.
    SignificantlyStale {
        total_changes: usize,
        file_list: Vec<String>,
        commit_info: String,
    },
}

impl ProjectOverviewStaleness {
    /// `true` when this level warrants `severity="high"` in the SessionStart
    /// injection (drives "prepend vs append" in the hook).
    #[cfg(test)]
    pub fn is_high_severity(&self, is_supervisor: bool) -> bool {
        match self {
            ProjectOverviewStaleness::Missing => true,
            ProjectOverviewStaleness::SignificantlyStale { .. } => true,
            ProjectOverviewStaleness::Stale { .. } => is_supervisor,
        }
    }

    /// Format as an XML-wrapped injection string.
    pub fn format_injection(&self, is_supervisor: bool) -> String {
        match self {
            ProjectOverviewStaleness::Missing => {
                "<project-overview-freshness severity=\"high\">\n\
                 docs/PRODUCT_OVERVIEW.md is missing. Run `/project-overview` before \
                 planning cross-cutting work — agents waste tokens re-deriving domain \
                 shape without it.\n\
                 </project-overview-freshness>"
                    .to_string()
            }
            ProjectOverviewStaleness::Stale {
                total_changes,
                file_list,
                commit_info,
            } => {
                let files = file_list.join(", ");
                let truncated = if *total_changes > 10 {
                    format!(" (+{} more)", total_changes - 10)
                } else {
                    String::new()
                };
                if is_supervisor {
                    format!(
                        "<project-overview-freshness severity=\"high\">\n\
                         docs/PRODUCT_OVERVIEW.md has {total_changes} relevant change(s){commit_info}: \
                         {files}{truncated}. Run `/project-overview` to refresh before assigning work.\n\
                         </project-overview-freshness>"
                    )
                } else {
                    format!(
                        "<project-overview-freshness severity=\"info\">\n\
                         docs/PRODUCT_OVERVIEW.md has {total_changes} pending relevant change(s){commit_info}: \
                         {files}{truncated}.\n\
                         </project-overview-freshness>"
                    )
                }
            }
            ProjectOverviewStaleness::SignificantlyStale {
                total_changes,
                file_list,
                commit_info,
            } => {
                let files = file_list.join(", ");
                let truncated = if *total_changes > 10 {
                    format!(" (+{} more)", total_changes - 10)
                } else {
                    String::new()
                };
                format!(
                    "<project-overview-freshness severity=\"high\">\n\
                     docs/PRODUCT_OVERVIEW.md is significantly out of date ({total_changes} relevant changes{commit_info}): \
                     {files}{truncated}. Run `/project-overview` to refresh before assigning work.\n\
                     </project-overview-freshness>"
                )
            }
        }
    }
}

fn make_staleness(
    total_changes: usize,
    file_list: Vec<String>,
    commit_info: String,
) -> ProjectOverviewStaleness {
    if total_changes >= SIGNIFICANT_STALENESS_THRESHOLD {
        ProjectOverviewStaleness::SignificantlyStale {
            total_changes,
            file_list,
            commit_info,
        }
    } else {
        ProjectOverviewStaleness::Stale {
            total_changes,
            file_list,
            commit_info,
        }
    }
}

fn change_prefix(c: &ProjectOverviewChange) -> &'static str {
    match c.change_type.as_str() {
        "A" => "+",
        "D" => "-",
        "R" => "~",
        "M" => "*",
        _ => "?",
    }
}

/// Check if PRODUCT_OVERVIEW.md is missing or stale.
///
/// `agent_role` is accepted for API symmetry with downstream hook wiring; the
/// returned staleness is role-agnostic. Callers can tailor rendering through
/// `format_injection`.
pub fn check_freshness(
    repo_root: &Path,
    agent_role: Option<&str>,
) -> Result<Option<ProjectOverviewStaleness>> {
    let _ = agent_role;
    let doc_path = repo_root.join(DOC_PATH);
    if !doc_path.exists() {
        return Ok(Some(ProjectOverviewStaleness::Missing));
    }

    // Supplement: in-session pending file (catches changes from this session).
    if let Some(staleness) = check_staleness_from_pending(repo_root)? {
        return Ok(Some(staleness));
    }

    // Primary: git history since PRODUCT_OVERVIEW.md's last commit, filtered
    // through watch_targets.
    check_staleness_from_git(repo_root)
}

fn check_staleness_from_pending(
    repo_root: &Path,
) -> Result<Option<ProjectOverviewStaleness>> {
    let entries = read_pending(repo_root)?;
    if entries.is_empty() {
        return Ok(None);
    }

    let mut total = 0usize;
    let mut files = Vec::new();
    let first_commit = entries.first().map(|e| e.commit.clone());

    for entry in &entries {
        for change in &entry.changes {
            total += 1;
            if files.len() < 10 {
                files.push(format!("{}{}", change_prefix(change), change.path));
            }
        }
    }

    if total == 0 {
        return Ok(None);
    }

    let commit_info = first_commit
        .map(|c| format!(" since {}", &c[..7.min(c.len())]))
        .unwrap_or_default();
    Ok(Some(make_staleness(total, files, commit_info)))
}

fn check_staleness_from_git(repo_root: &Path) -> Result<Option<ProjectOverviewStaleness>> {
    let Some(ts) = last_commit_timestamp(repo_root, DOC_PATH) else {
        return Ok(None);
    };

    let targets = watch_targets(repo_root);

    // Domain paths — any change since timestamp.
    let mut collected: Vec<ProjectOverviewChange> = Vec::new();
    for dp in &targets.domain_paths {
        collected.extend(changes_for_path_since(repo_root, dp, &ts));
    }
    // Watch dirs — structural only (A/D/R).
    let structural = structural_changes_since(repo_root, &ts).unwrap_or_default();
    for c in structural {
        let hit = targets.is_in_watch_dir(&c.path)
            || c.old_path
                .as_deref()
                .map(|op| targets.is_in_watch_dir(op))
                .unwrap_or(false);
        if hit {
            collected.push(c);
        }
    }

    // Dedupe by (type, path).
    let mut seen = std::collections::HashSet::new();
    collected.retain(|c| seen.insert(format!("{}:{}", c.change_type, c.path)));

    if collected.is_empty() {
        return Ok(None);
    }

    let total = collected.len();
    let files: Vec<String> = collected
        .iter()
        .take(10)
        .map(|c| format!("{}{}", change_prefix(c), c.path))
        .collect();

    Ok(Some(make_staleness(total, files, String::new())))
}

/// Timestamp (RFC3339) of the last commit that touched `path` (relative to
/// `repo_root`).
///
/// Returns `None` if the file has never been committed. The earlier design
/// fell back to file mtime, but an uncommitted mtime can be arbitrarily old
/// (or in the future on skewed clocks), which made `git log --since=<mtime>`
/// flood every SessionStart with unrelated history and triggered a permanent
/// `SignificantlyStale` warning until someone committed the file. Returning
/// `None` lets the pending-file path be the only in-session signal for
/// uncommitted docs.
fn last_commit_timestamp(repo_root: &Path, path: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["log", "-1", "--format=%cI", "--", path])
        .current_dir(repo_root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let ts = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if ts.is_empty() { None } else { Some(ts) }
}

/// All changes (M/A/D/R) to a single path since `since`.
fn changes_for_path_since(
    repo_root: &Path,
    path: &str,
    since: &str,
) -> Vec<ProjectOverviewChange> {
    let Ok(out) = std::process::Command::new("git")
        .args([
            "log",
            &format!("--since={since}"),
            "--name-status",
            "--format=",
            "--no-renames",
            "--",
            path,
        ])
        .current_dir(repo_root)
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    let mut changes = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }
        let status = parts[0];
        let ty = match status {
            "A" => "A",
            "D" => "D",
            "M" => "M",
            _ => continue, // `--no-renames` below means R never appears here
        };
        changes.push(ProjectOverviewChange {
            change_type: ty.to_string(),
            path: parts[1].to_string(),
            old_path: None,
        });
    }
    changes
}

/// Structural changes (A/D) anywhere in the repo since `since`. Renames are
/// split to A+D via `--no-renames` (matches codemap behavior).
fn structural_changes_since(
    repo_root: &Path,
    since: &str,
) -> Option<Vec<ProjectOverviewChange>> {
    let out = std::process::Command::new("git")
        .args([
            "log",
            "--diff-filter=ADR",
            "--name-status",
            &format!("--since={since}"),
            "--format=",
            "--no-renames",
            "-z",
        ])
        .current_dir(repo_root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_git_log_nul_output(&String::from_utf8_lossy(
        &out.stdout,
    )))
}

/// Map NUL-delimited `git log --name-status -z` output onto overview changes.
///
/// Wire-format parsing is [`crate::git_log::parse_name_status_z`], shared with
/// codemap and the git-history walker (cas-7a21). Overview-specific policy kept
/// here: only A/D survive (`--no-renames` at the call site means R never
/// arrives), deduplicated across commits.
fn parse_git_log_nul_output(raw: &str) -> Vec<ProjectOverviewChange> {
    let mut changes = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for entry in crate::git_log::parse_name_status_z(raw) {
        if !matches!(entry.status.as_str(), "A" | "D") {
            continue;
        }
        if seen.insert(format!("{}:{}", entry.status, entry.path)) {
            changes.push(ProjectOverviewChange {
                change_type: entry.status,
                path: entry.path,
                old_path: None,
            });
        }
    }

    changes
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- diff-tree parsing -------------------------------------------------

    #[test]
    fn parse_diff_tree_keeps_modified_for_filter_stage() {
        // M is preserved by parse_diff_tree_output — filter_changes is the
        // stage that drops it for non-domain paths.
        let out = "M\tsrc/existing.rs\nA\tsrc/new.rs\n";
        let changes = parse_diff_tree_output(out);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].change_type, "M");
        assert_eq!(changes[1].change_type, "A");
    }

    #[test]
    fn parse_diff_tree_rename() {
        let out = "R100\tsrc/old.rs\tsrc/new.rs\n";
        let changes = parse_diff_tree_output(out);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_type, "R");
        assert_eq!(changes[0].path, "src/new.rs");
        assert_eq!(changes[0].old_path.as_deref(), Some("src/old.rs"));
    }

    #[test]
    fn parse_diff_tree_empty() {
        assert!(parse_diff_tree_output("").is_empty());
    }

    // --- filter_changes ----------------------------------------------------

    fn targets_fixture() -> WatchTargets {
        WatchTargets {
            domain_paths: vec!["schema.prisma".to_string()],
            watch_dirs: vec!["apps/*/src/components".to_string()],
        }
    }

    #[test]
    fn filter_keeps_modified_on_domain_path() {
        let changes = vec![ProjectOverviewChange {
            change_type: "M".to_string(),
            path: "schema.prisma".to_string(),
            old_path: None,
        }];
        let filtered = filter_changes(changes, &targets_fixture());
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn filter_drops_modified_outside_domain() {
        let changes = vec![ProjectOverviewChange {
            change_type: "M".to_string(),
            path: "apps/backend/src/components/Button.tsx".to_string(),
            old_path: None,
        }];
        let filtered = filter_changes(changes, &targets_fixture());
        assert!(filtered.is_empty(), "M under watch_dir must be dropped");
    }

    #[test]
    fn filter_keeps_added_in_watch_dir() {
        let changes = vec![ProjectOverviewChange {
            change_type: "A".to_string(),
            path: "apps/backend/src/components/Button.tsx".to_string(),
            old_path: None,
        }];
        let filtered = filter_changes(changes, &targets_fixture());
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn filter_drops_added_outside_any_target() {
        let changes = vec![ProjectOverviewChange {
            change_type: "A".to_string(),
            path: "docs/unrelated.md".to_string(),
            old_path: None,
        }];
        let filtered = filter_changes(changes, &targets_fixture());
        assert!(filtered.is_empty());
    }

    #[test]
    fn filter_rename_from_domain_path_kept() {
        let changes = vec![ProjectOverviewChange {
            change_type: "R".to_string(),
            path: "new/schema.prisma".to_string(),
            old_path: Some("schema.prisma".to_string()),
        }];
        let filtered = filter_changes(changes, &targets_fixture());
        assert_eq!(filtered.len(), 1);
    }

    // --- dir_glob_matches --------------------------------------------------

    #[test]
    fn glob_matches_nested_wildcard() {
        assert!(dir_glob_matches(
            "apps/*/src/components",
            "apps/backend/src/components/Button.tsx",
        ));
    }

    #[test]
    fn glob_matches_bare_dir() {
        assert!(dir_glob_matches("app", "app/layout.tsx"));
    }

    #[test]
    fn glob_does_not_match_sibling() {
        assert!(!dir_glob_matches(
            "apps/*/src/components",
            "apps/backend/src/pages/index.tsx",
        ));
    }

    // --- watch_targets / config --------------------------------------------

    #[test]
    fn watch_targets_defaults_when_no_config() {
        let tmp = std::env::temp_dir().join(format!(
            "po_test_defaults_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let t = watch_targets(&tmp);
        assert_eq!(t, WatchTargets::defaults());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn watch_targets_defaults_when_section_absent() {
        let tmp = std::env::temp_dir().join(format!(
            "po_test_no_section_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join(".cas")).unwrap();
        std::fs::write(
            tmp.join(".cas/config.toml"),
            "[other]\nfoo = \"bar\"\n",
        )
        .unwrap();

        let t = watch_targets(&tmp);
        assert_eq!(t, WatchTargets::defaults());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn watch_targets_reads_overrides() {
        let tmp = std::env::temp_dir().join(format!(
            "po_test_overrides_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join(".cas")).unwrap();
        std::fs::write(
            tmp.join(".cas/config.toml"),
            r#"
[project_overview]
domain_paths = ["db/schema.sql"]
watch_dirs = ["web/src"]
"#,
        )
        .unwrap();

        let t = watch_targets(&tmp);
        assert_eq!(t.domain_paths, vec!["db/schema.sql".to_string()]);
        assert_eq!(t.watch_dirs, vec!["web/src".to_string()]);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn watch_targets_partial_override_keeps_other_default() {
        let tmp = std::env::temp_dir().join(format!(
            "po_test_partial_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join(".cas")).unwrap();
        std::fs::write(
            tmp.join(".cas/config.toml"),
            r#"
[project_overview]
domain_paths = ["db/schema.sql"]
"#,
        )
        .unwrap();

        let t = watch_targets(&tmp);
        assert_eq!(t.domain_paths, vec!["db/schema.sql".to_string()]);
        assert_eq!(t.watch_dirs, WatchTargets::defaults().watch_dirs);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- pending file I/O --------------------------------------------------

    #[test]
    fn read_pending_empty_when_missing() {
        let tmp = std::env::temp_dir().join(format!(
            "po_read_empty_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let entries = read_pending(&tmp).unwrap();
        assert!(entries.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_pending_parses_jsonl() {
        let tmp = std::env::temp_dir().join(format!(
            "po_read_jsonl_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join(".cas")).unwrap();
        let line1 = r#"{"changes":[{"type":"A","path":"x"}],"commit":"abc","recorded_at":"2026-04-14T00:00:00Z"}"#;
        let line2 = r#"{"changes":[{"type":"M","path":"schema.prisma"}],"commit":"def","recorded_at":"2026-04-14T01:00:00Z"}"#;
        std::fs::write(
            tmp.join(".cas/project-overview-pending.json"),
            format!("{line1}\n{line2}\n"),
        )
        .unwrap();

        let entries = read_pending(&tmp).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].commit, "abc");
        assert_eq!(entries[1].changes[0].change_type, "M");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn clear_pending_is_idempotent() {
        let tmp = std::env::temp_dir().join(format!(
            "po_clear_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join(".cas")).unwrap();
        let p = tmp.join(".cas/project-overview-pending.json");
        std::fs::write(&p, "{}").unwrap();
        assert!(p.exists());
        clear_pending(&tmp).unwrap();
        assert!(!p.exists());
        // Second call on missing file.
        clear_pending(&tmp).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- staleness variant / formatting ------------------------------------

    #[test]
    fn make_staleness_below_threshold_is_stale() {
        let s = make_staleness(3, vec![], String::new());
        assert!(matches!(s, ProjectOverviewStaleness::Stale { .. }));
    }

    #[test]
    fn make_staleness_at_threshold_is_significant() {
        let s = make_staleness(5, vec![], String::new());
        assert!(matches!(
            s,
            ProjectOverviewStaleness::SignificantlyStale { .. }
        ));
    }

    #[test]
    fn high_severity_missing_always() {
        assert!(ProjectOverviewStaleness::Missing.is_high_severity(false));
        assert!(ProjectOverviewStaleness::Missing.is_high_severity(true));
    }

    #[test]
    fn high_severity_stale_only_for_supervisor() {
        let s = ProjectOverviewStaleness::Stale {
            total_changes: 2,
            file_list: vec![],
            commit_info: String::new(),
        };
        assert!(!s.is_high_severity(false));
        assert!(s.is_high_severity(true));
    }

    #[test]
    fn format_injection_wraps_xml_tag() {
        let msg = ProjectOverviewStaleness::Missing.format_injection(false);
        assert!(msg.contains("<project-overview-freshness severity=\"high\">"));
        assert!(msg.contains("docs/PRODUCT_OVERVIEW.md is missing"));
        assert!(msg.contains("/project-overview"));
    }

    #[test]
    fn format_injection_stale_info_for_worker() {
        let s = ProjectOverviewStaleness::Stale {
            total_changes: 2,
            file_list: vec!["+a".to_string()],
            commit_info: String::new(),
        };
        let msg = s.format_injection(false);
        assert!(msg.contains("severity=\"info\""));
        assert!(!msg.contains("Run `/project-overview`"));
    }

    #[test]
    fn format_injection_stale_high_for_supervisor() {
        let s = ProjectOverviewStaleness::Stale {
            total_changes: 2,
            file_list: vec!["+a".to_string()],
            commit_info: String::new(),
        };
        let msg = s.format_injection(true);
        assert!(msg.contains("severity=\"high\""));
        assert!(msg.contains("Run `/project-overview`"));
    }

    #[test]
    fn format_injection_significantly_stale_high_for_all() {
        let s = ProjectOverviewStaleness::SignificantlyStale {
            total_changes: 12,
            file_list: (0..10).map(|i| format!("+f{i}")).collect(),
            commit_info: " since abc1234".to_string(),
        };
        let msg = s.format_injection(false);
        assert!(msg.contains("severity=\"high\""));
        assert!(msg.contains("significantly out of date"));
        assert!(msg.contains("12 relevant changes"));
        assert!(msg.contains("(+2 more)"));
    }

    // --- check_freshness ---------------------------------------------------

    #[test]
    fn check_freshness_returns_missing_when_doc_absent() {
        let tmp = std::env::temp_dir().join(format!(
            "po_cf_missing_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let r = check_freshness(&tmp, None).unwrap();
        assert_eq!(r, Some(ProjectOverviewStaleness::Missing));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn check_freshness_uses_pending_when_doc_present() {
        let tmp = std::env::temp_dir().join(format!(
            "po_cf_pending_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("docs")).unwrap();
        std::fs::create_dir_all(tmp.join(".cas")).unwrap();
        std::fs::write(tmp.join(DOC_PATH), "# overview\n").unwrap();
        // Seed pending with 6 changes → SignificantlyStale.
        let mut lines = String::new();
        for i in 0..6 {
            lines.push_str(&format!(
                r#"{{"changes":[{{"type":"A","path":"f{i}"}}],"commit":"c{i}","recorded_at":"2026-04-14T00:00:00Z"}}
"#
            ));
        }
        std::fs::write(tmp.join(".cas/project-overview-pending.json"), lines).unwrap();

        let r = check_freshness(&tmp, None).unwrap();
        match r {
            Some(ProjectOverviewStaleness::SignificantlyStale {
                total_changes, ..
            }) => assert_eq!(total_changes, 6),
            other => panic!("expected SignificantlyStale, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- sanitize_watch_dirs ----------------------------------------------

    #[test]
    fn sanitize_watch_dirs_strips_stars() {
        let out = sanitize_watch_dirs(vec![
            "**".into(),
            "*".into(),
            " ".into(),
            "".into(),
            "apps/*/src".into(),
        ]);
        assert_eq!(out, vec!["apps/*/src".to_string()]);
    }

    #[test]
    fn watch_targets_sanitizes_dangerous_globs() {
        let tmp = std::env::temp_dir().join(format!(
            "po_sanitize_{}_{}",
            std::process::id(),
            unique_id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join(".cas")).unwrap();
        std::fs::write(
            tmp.join(".cas/config.toml"),
            r#"
[project_overview]
watch_dirs = ["**", "web/src"]
"#,
        )
        .unwrap();

        let t = watch_targets(&tmp);
        assert_eq!(t.watch_dirs, vec!["web/src".to_string()]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- last_commit_timestamp (no-mtime-fallback contract) ---------------

    #[test]
    fn last_commit_timestamp_returns_none_when_not_committed() {
        // Empty tmp dir — no git repo, no file, no history. Must not panic and
        // must not fabricate an mtime.
        let tmp = std::env::temp_dir().join(format!(
            "po_ts_none_{}_{}",
            std::process::id(),
            unique_id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(last_commit_timestamp(&tmp, DOC_PATH).is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- parse_git_log_nul_output rename behavior after --no-renames -----

    #[test]
    fn parse_git_log_nul_skips_rename_status() {
        // `structural_changes_since` always passes `--no-renames`, so R status
        // codes never arrive at this parser. Confirm it silently skips them
        // rather than producing a malformed entry.
        let raw = "R100\0src/old.rs\0src/new.rs\0A\0src/added.rs\0";
        let changes = parse_git_log_nul_output(raw);
        // The 3-token rename is parsed as (R100, src/old.rs) pair [skipped] +
        // leftover (src/new.rs, A) pair — also skipped because "src/new.rs"
        // isn't a recognized status. Then (src/added.rs, <nothing>) is the tail
        // — NUL padding in real git output protects against this. We just
        // verify no panic and no spurious R entry.
        assert!(
            changes.iter().all(|c| c.change_type != "R"),
            "R status must never appear in parse_git_log_nul_output output"
        );
    }

    // --- end-to-end: detect_structural_changes on a real git repo --------

    fn run_git(repo: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn detect_structural_changes_writes_pending_for_watched_file() {
        let tmp = std::env::temp_dir().join(format!(
            "po_detect_{}_{}",
            std::process::id(),
            unique_id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Skip when git isn't on PATH (CI fallback).
        if !run_git(&tmp, &["init", "-q", "-b", "main"]) {
            let _ = std::fs::remove_dir_all(&tmp);
            return;
        }
        // Seed config pointing watch_dirs at `watched/`.
        std::fs::create_dir_all(tmp.join(".cas")).unwrap();
        std::fs::write(
            tmp.join(".cas/config.toml"),
            r#"
[project_overview]
domain_paths = []
watch_dirs = ["watched"]
"#,
        )
        .unwrap();

        // Initial commit (unrelated file) so HEAD~1 exists.
        std::fs::write(tmp.join("seed.txt"), "seed\n").unwrap();
        assert!(run_git(&tmp, &["add", "-A"]));
        assert!(run_git(&tmp, &["commit", "-q", "-m", "seed"]));

        // Add a file inside the watched dir → structural A.
        std::fs::create_dir_all(tmp.join("watched")).unwrap();
        std::fs::write(tmp.join("watched/new.rs"), "pub fn x() {}\n").unwrap();
        assert!(run_git(&tmp, &["add", "-A"]));
        assert!(run_git(&tmp, &["commit", "-q", "-m", "add watched"]));

        // Act.
        detect_structural_changes(&tmp).expect("detect must succeed");

        // Assert: pending JSONL contains one entry with the watched path.
        let entries = read_pending(&tmp).expect("read_pending");
        assert_eq!(entries.len(), 1, "one pending entry expected");
        let entry = &entries[0];
        assert!(!entry.commit.is_empty());
        assert!(
            entry
                .changes
                .iter()
                .any(|c| c.change_type == "A" && c.path == "watched/new.rs"),
            "expected A watched/new.rs in entry, got {:?}",
            entry.changes
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_structural_changes_skips_unwatched_modifications() {
        let tmp = std::env::temp_dir().join(format!(
            "po_detect_skip_{}_{}",
            std::process::id(),
            unique_id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        if !run_git(&tmp, &["init", "-q", "-b", "main"]) {
            let _ = std::fs::remove_dir_all(&tmp);
            return;
        }
        std::fs::create_dir_all(tmp.join(".cas")).unwrap();
        std::fs::write(
            tmp.join(".cas/config.toml"),
            r#"
[project_overview]
domain_paths = ["db/schema.sql"]
watch_dirs = []
"#,
        )
        .unwrap();

        std::fs::write(tmp.join("seed.txt"), "seed\n").unwrap();
        assert!(run_git(&tmp, &["add", "-A"]));
        assert!(run_git(&tmp, &["commit", "-q", "-m", "seed"]));

        // Modify an unrelated file → must not produce a pending entry.
        std::fs::write(tmp.join("seed.txt"), "seed2\n").unwrap();
        assert!(run_git(&tmp, &["add", "-A"]));
        assert!(run_git(&tmp, &["commit", "-q", "-m", "noise"]));

        detect_structural_changes(&tmp).unwrap();

        let entries = read_pending(&tmp).unwrap();
        assert!(
            entries.is_empty(),
            "unrelated change must not write a pending entry; got {entries:?}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Monotonic counter so parallel test threads don't collide on temp paths.
    fn unique_id() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }
}
