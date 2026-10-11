//! Host project discovery and launch-target resolution for Commander.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use cas_store::{KnownRepo, KnownRepoStore};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{DaemonLiveness, HubSession};
use crate::config::Config;
use crate::store::known_repos::{host_cas_dir, open_host_known_repo_store, registry_skip};

const MAX_BROWSE_DEPTH: usize = 6;
const MAX_BROWSE_ENTRIES: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LaunchTarget {
    Project { id: String },
    Browse { root_id: String, path: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRoot {
    pub path: PathBuf,
}

/// Resolve a UI selection against current host state. IDs never authorize a
/// path by themselves: both target kinds are revalidated on every use.
pub fn resolve_launch_target(target: &LaunchTarget) -> Result<LaunchRoot> {
    let path = match target {
        LaunchTarget::Project { id } => list_projects(&[])?
            .into_iter()
            .find(|project| &project.id == id)
            .map(|project| project.path)
            .context("project is not available")?,
        LaunchTarget::Browse { root_id, path } => {
            let roots = configured_launch_roots()?;
            let root = roots
                .into_iter()
                .find(|root| &root.id == root_id)
                .context("browse root is not available")?;
            resolve_browse_path(&root.path, path)?
        }
    };
    let main = canonical_main_repo(&path).context("launch target is not a repository")?;
    ensure!(main == path, "linked worktrees are not launchable");
    Ok(LaunchRoot { path: main })
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectEntry {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub last_touched_at: DateTime<Utc>,
    pub touch_count: u64,
    pub running_session: Option<String>,
    pub target: LaunchTarget,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowseRoot {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowseEntry {
    pub name: String,
    pub path: String,
    pub launchable: bool,
    pub project_id: Option<String>,
    pub target: Option<LaunchTarget>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowseResult {
    pub root: BrowseRoot,
    pub path: String,
    pub entries: Vec<BrowseEntry>,
    pub truncated: bool,
}

fn stable_id(prefix: &str, path: &Path) -> String {
    let digest = Sha256::digest(path.to_string_lossy().as_bytes());
    format!("{prefix}-{}", hex::encode(&digest[..16]))
}

fn git_output(path: &Path, args: &[&str]) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = std::str::from_utf8(&output.stdout).ok()?.trim();
    if value.is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

/// A registered linked checkout resolves to its main root. A nested folder,
/// bare repository, missing path, or separate gitdir layout is not selectable.
fn canonical_main_repo(path: &Path) -> Option<PathBuf> {
    let candidate = path.canonicalize().ok()?;
    if !candidate.is_dir() {
        return None;
    }
    let checkout = git_output(&candidate, &["rev-parse", "--show-toplevel"])?
        .canonicalize()
        .ok()?;
    if checkout != candidate {
        return None;
    }
    let common = git_output(&candidate, &["rev-parse", "--git-common-dir"])?;
    let common = if common.is_absolute() {
        common
    } else {
        candidate.join(common)
    };
    let common = common.canonicalize().ok()?;
    if common.file_name()? != ".git" {
        return None;
    }
    let main = common.parent()?.canonicalize().ok()?;
    if !main.join(".git").is_dir() || main.join(".git").canonicalize().ok()? != common {
        return None;
    }
    Some(main)
}

fn registry_rows() -> Result<Vec<KnownRepo>> {
    if !host_cas_dir().join("cas.db").exists() {
        return Ok(Vec::new());
    }
    open_host_known_repo_store()?.list().map_err(Into::into)
}

pub fn list_projects(sessions: &[HubSession]) -> Result<Vec<ProjectEntry>> {
    Ok(projects_from_rows(registry_rows()?, sessions, true))
}

fn projects_from_rows(
    rows: Vec<KnownRepo>,
    sessions: &[HubSession],
    skip_disposable: bool,
) -> Vec<ProjectEntry> {
    let mut projects: BTreeMap<PathBuf, ProjectEntry> = BTreeMap::new();
    for row in rows {
        let Some(main) = canonical_main_repo(&row.path) else {
            continue;
        };
        if skip_disposable
            && registry_skip(&main).is_some_and(|skip| {
                !matches!(
                    skip,
                    crate::store::known_repos::RegistrySkip::BareFolderIdentity(_)
                )
            })
        {
            continue;
        }
        let entry = projects
            .entry(main.clone())
            .or_insert_with(|| ProjectEntry {
                id: stable_id("project", &main),
                name: main
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path: main.clone(),
                last_touched_at: row.last_touched_at,
                touch_count: 0,
                running_session: None,
                target: LaunchTarget::Project {
                    id: stable_id("project", &main),
                },
            });
        entry.last_touched_at = entry.last_touched_at.max(row.last_touched_at);
        entry.touch_count = entry.touch_count.saturating_add(row.touch_count);
    }
    for session in sessions
        .iter()
        .filter(|s| s.liveness == DaemonLiveness::Live)
    {
        let Some(project_dir) = session.project_dir.as_deref() else {
            continue;
        };
        if let Some(main) = canonical_main_repo(Path::new(project_dir)) {
            if let Some(project) = projects.get_mut(&main) {
                project
                    .running_session
                    .get_or_insert_with(|| session.name.clone());
            }
        }
    }
    let mut result: Vec<_> = projects.into_values().collect();
    result.sort_by(|a, b| {
        b.last_touched_at
            .cmp(&a.last_touched_at)
            .then_with(|| b.touch_count.cmp(&a.touch_count))
            .then_with(|| a.name.cmp(&b.name))
    });
    result
}

/// Host-scoped configuration. An absent key defaults to ~/Petrastella only
/// when it exists; an explicit empty list disables browsing.
pub fn configured_launch_roots() -> Result<Vec<BrowseRoot>> {
    let home = dirs::home_dir().context("home directory unavailable")?;
    let config = match Config::load(&host_cas_dir()) {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(%error, "host config unavailable; Commander browsing disabled");
            return Ok(Vec::new());
        }
    };
    let configured = config.hub.and_then(|hub| hub.launch_roots);
    let paths = configured.unwrap_or_else(|| {
        let default = home.join("Petrastella");
        if default.is_dir() {
            vec![default.to_string_lossy().into_owned()]
        } else {
            vec![]
        }
    });
    Ok(launch_roots_from_paths(&home, paths))
}

fn launch_roots_from_paths(home: &Path, paths: Vec<String>) -> Vec<BrowseRoot> {
    let mut roots = BTreeMap::new();
    for raw in paths {
        let path = if raw == "~" {
            home.to_path_buf()
        } else if let Some(tail) = raw.strip_prefix("~/") {
            home.join(tail)
        } else {
            PathBuf::from(raw)
        };
        let path = match path.canonicalize() {
            Ok(path) if path.is_dir() => path,
            Ok(path) => {
                tracing::warn!(path = %path.display(), "configured launch root is not a directory");
                continue;
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "configured launch root is unavailable");
                continue;
            }
        };
        roots.entry(path.clone()).or_insert_with(|| BrowseRoot {
            id: stable_id("root", &path),
            name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            path,
        });
    }
    roots.into_values().collect()
}

fn resolve_browse_path(root: &Path, relative: &str) -> Result<PathBuf> {
    ensure!(
        !Path::new(relative).is_absolute(),
        "browse path must be relative"
    );
    let mut depth = 0;
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => bail!("browse path contains traversal"),
        }
    }
    ensure!(depth <= MAX_BROWSE_DEPTH, "browse depth exceeded");
    let path = root
        .join(relative)
        .canonicalize()
        .context("browse path is unavailable")?;
    ensure!(
        path.starts_with(root),
        "browse path escapes configured root"
    );
    ensure!(path.is_dir(), "browse path is not a directory");
    Ok(path)
}

pub fn browse(root_id: &str, path: &str) -> Result<BrowseResult> {
    let root = configured_launch_roots()?
        .into_iter()
        .find(|root| root.id == root_id)
        .context("browse root is not available")?;
    browse_in(root, path)
}

fn browse_in(root: BrowseRoot, path: &str) -> Result<BrowseResult> {
    let current = resolve_browse_path(&root.path, path)?;
    let mut entries = Vec::new();
    const MAX_SCAN_ENTRIES: usize = 5_000;
    let mut scanned = 0;
    let mut scan_limited = false;
    for item in fs::read_dir(&current)? {
        if scanned == MAX_SCAN_ENTRIES {
            scan_limited = true;
            break;
        }
        scanned += 1;
        let Ok(item) = item else { continue };
        let Ok(file_type) = item.file_type() else {
            continue;
        };
        if file_type.is_symlink() || !file_type.is_dir() {
            continue;
        }
        let child = match item.path().canonicalize() {
            Ok(path) => path,
            Err(_) => continue,
        };
        if !child.starts_with(&root.path) {
            continue;
        }
        let relative = child
            .strip_prefix(&root.path)?
            .to_string_lossy()
            .into_owned();
        if relative.split(std::path::MAIN_SEPARATOR).count() > MAX_BROWSE_DEPTH {
            continue;
        }
        let launchable = canonical_main_repo(&child).is_some_and(|main| main == child);
        entries.push(BrowseEntry {
            name: item.file_name().to_string_lossy().into_owned(),
            path: relative.clone(),
            launchable,
            project_id: launchable.then(|| stable_id("project", &child)),
            target: launchable.then(|| LaunchTarget::Browse {
                root_id: root.id.clone(),
                path: relative.clone(),
            }),
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let truncated = scan_limited || entries.len() > MAX_BROWSE_ENTRIES;
    entries.truncate(MAX_BROWSE_ENTRIES);
    let relative = current
        .strip_prefix(&root.path)?
        .to_string_lossy()
        .into_owned();
    Ok(BrowseResult {
        root,
        path: relative,
        entries,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn git(dir: &Path, args: &[&str]) {
        let result = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&result.stderr)
        );
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir_in(crate::test_paths::runtime_fixture_parent()).unwrap();
        let main = temp.path().join("main");
        let linked = temp.path().join("linked");
        fs::create_dir(&main).unwrap();
        git(&main, &["init"]);
        git(
            &main,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
        );
        git(
            &main,
            &["worktree", "add", "-b", "linked", linked.to_str().unwrap()],
        );
        (temp, main, linked)
    }

    #[test]
    fn registered_worktrees_collapse_to_one_main_project_and_bind_live_session() {
        let (_temp, main, linked) = fixture();
        let now = Utc::now();
        let rows = vec![
            KnownRepo {
                path: linked.clone(),
                first_seen_at: now,
                last_touched_at: now,
                touch_count: 2,
            },
            KnownRepo {
                path: main.clone(),
                first_seen_at: now,
                last_touched_at: now + Duration::seconds(1),
                touch_count: 3,
            },
            KnownRepo {
                path: main.join("MISSING"),
                first_seen_at: now,
                last_touched_at: now,
                touch_count: 99,
            },
        ];
        let sessions = vec![HubSession {
            name: "factory-main".into(),
            project_dir: Some(linked.to_string_lossy().into_owned()),
            supervisor: "supervisor".into(),
            workers: vec![],
            epic_id: None,
            ws_port: Some(1),
            liveness: DaemonLiveness::Live,
            dormant: false,
            last_activity_at: None,
            last_activity: None,
            started_at: None,
            cloud_project_id: None,
            daemon_identity: None,
        }];
        let projects = projects_from_rows(rows, &sessions, false);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].path, main);
        assert_eq!(projects[0].touch_count, 5);
        assert_eq!(projects[0].running_session.as_deref(), Some("factory-main"));
    }

    #[test]
    fn browse_is_bounded_and_only_main_checkout_is_launchable() {
        let (temp, main, linked) = fixture();
        let root = BrowseRoot {
            id: stable_id("root", temp.path()),
            name: "root".into(),
            path: temp.path().to_path_buf(),
        };
        let output = browse_in(root.clone(), "").unwrap();
        let main_entry = output
            .entries
            .iter()
            .find(|entry| entry.name == "main")
            .unwrap();
        assert!(main_entry.launchable);
        assert_eq!(
            main_entry.target,
            Some(LaunchTarget::Browse {
                root_id: root.id.clone(),
                path: "main".into()
            })
        );
        assert!(
            !output
                .entries
                .iter()
                .find(|entry| entry.name == "linked")
                .unwrap()
                .launchable
        );
        assert_eq!(
            resolve_browse_path(&root.path, "../")
                .unwrap_err()
                .to_string(),
            "browse path contains traversal"
        );
        assert_eq!(canonical_main_repo(&linked), Some(main));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/", temp.path().join("escape")).unwrap();
            assert!(resolve_browse_path(&root.path, "escape").is_err());
        }
    }

    #[test]
    fn launch_target_wire_shape_and_explicit_browse_disable() {
        let project = LaunchTarget::Project {
            id: "project-123".into(),
        };
        assert_eq!(
            serde_json::to_value(&project).unwrap(),
            serde_json::json!({
                "kind": "project", "id": "project-123"
            })
        );
        let browse: LaunchTarget = serde_json::from_value(serde_json::json!({
            "kind": "browse", "root_id": "root-123", "path": "team/repo"
        }))
        .unwrap();
        assert_eq!(
            browse,
            LaunchTarget::Browse {
                root_id: "root-123".into(),
                path: "team/repo".into()
            }
        );
        assert!(
            serde_json::from_value::<LaunchTarget>(serde_json::json!({
                "kind": "browse", "root_id": "root-123", "path": "repo", "extra": true
            }))
            .is_err()
        );
        let config: Config = toml::from_str("[hub]\nlaunch_roots = []\n").unwrap();
        assert_eq!(config.hub.unwrap().launch_roots, Some(vec![]));
    }

    #[test]
    fn missing_configured_root_is_skipped_and_known_projects_remain() {
        let (temp, main, _linked) = fixture();
        let missing = temp.path().join("MISSING");
        let roots = launch_roots_from_paths(
            temp.path(),
            vec![
                missing.to_string_lossy().into_owned(),
                temp.path().to_string_lossy().into_owned(),
            ],
        );
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].path, temp.path());
        let now = Utc::now();
        let projects = projects_from_rows(
            vec![KnownRepo {
                path: main.clone(),
                first_seen_at: now,
                last_touched_at: now,
                touch_count: 1,
            }],
            &[],
            false,
        );
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].path, main);
    }

    #[test]
    fn browse_filters_files_before_sorting_and_truncating_directories() {
        let temp = tempfile::tempdir_in(crate::test_paths::runtime_fixture_parent()).unwrap();
        for index in 0..150 {
            fs::write(temp.path().join(format!("file-{index:03}")), b"fixture").unwrap();
        }
        let repo = temp.path().join("repo-after-files");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init"]);
        let root = BrowseRoot {
            id: stable_id("root", temp.path()),
            name: "root".into(),
            path: temp.path().to_path_buf(),
        };
        let result = browse_in(root, "").unwrap();
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].name, "repo-after-files");
        assert!(result.entries[0].launchable);
        assert!(!result.truncated);
    }

    #[test]
    fn unreadable_host_config_disables_browse_without_an_error() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let host = home.join(".cas");
            fs::create_dir_all(&host).unwrap();
            fs::write(host.join("config.toml"), "[hub\ninvalid toml").unwrap();
            assert!(configured_launch_roots().unwrap().is_empty());
        });
    }
}
