//! `cas open` — interactive TUI project picker
//!
//! Scans ~/projects/ for git repositories, shows a picker with session status,
//! and launches or attaches to a factory session for the selected project.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::Args;
use crossterm::style::Color;

use crate::cli::interactive::{PickerItem, PickerTag};
use crate::ui::factory::SessionManager;

/// Arguments for `cas open`
#[derive(Args, Debug, Clone)]
pub struct OpenArgs {
    /// Directory to scan for projects (default: ~/projects/)
    #[arg(long)]
    pub dir: Option<PathBuf>,
}

/// A discovered project directory with its session status.
#[derive(Debug, Clone)]
pub struct ProjectEntry {
    /// Project directory name (last component of path)
    pub name: String,
    /// Full path to the project
    pub path: PathBuf,
    /// Whether a Cassy factory session is currently running for this project
    pub has_running_session: bool,
}

/// Scan a directory for git repositories (non-recursive, one level deep).
pub fn scan_projects(base_dir: &Path) -> Result<Vec<ProjectEntry>> {
    if !base_dir.exists() {
        return Ok(Vec::new());
    }

    let session_manager = SessionManager::new();
    let sessions = session_manager.list_sessions().unwrap_or_default();

    let mut entries = Vec::new();

    let mut read_dir: Vec<_> = std::fs::read_dir(base_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .collect();

    // Sort by name for stable ordering
    read_dir.sort_by_key(|e| e.file_name());

    for entry in read_dir {
        let path = entry.path();

        // Only include directories that contain a .git (are git repos)
        if !path.join(".git").exists() {
            continue;
        }

        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let path_str = path.to_string_lossy().to_string();
        let has_running_session = sessions.iter().any(|s| {
            s.is_running
                && s.metadata
                    .project_dir
                    .as_ref()
                    .is_some_and(|p| p == &path_str)
        });

        entries.push(ProjectEntry {
            name,
            path,
            has_running_session,
        });
    }

    Ok(entries)
}

/// Convert project entries into picker items for the interactive TUI.
pub fn projects_to_picker_items(projects: &[ProjectEntry]) -> Vec<PickerItem> {
    projects
        .iter()
        .map(|p| {
            let status_tag = if p.has_running_session {
                PickerTag {
                    text: "running".to_string(),
                    color: Color::Green,
                }
            } else {
                PickerTag {
                    text: "stopped".to_string(),
                    color: Color::DarkGrey,
                }
            };

            PickerItem {
                label: p.name.clone(),
                tags: vec![status_tag],
            }
        })
        .collect()
}

/// Default projects directory: ~/projects/
fn default_projects_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("projects")
}

/// Execute the `cas open` command.
pub fn execute(args: &OpenArgs) -> Result<()> {
    let base_dir = args.dir.clone().unwrap_or_else(default_projects_dir);

    if !base_dir.exists() {
        bail!(
            "Projects directory does not exist: {}\n\n\
             Create it or specify a different directory with --dir",
            base_dir.display()
        );
    }

    let projects = scan_projects(&base_dir)?;

    if projects.is_empty() {
        bail!(
            "No git repositories found in {}\n\n\
             Clone or create projects there first.",
            base_dir.display()
        );
    }

    let items = projects_to_picker_items(&projects);

    let selected = crate::cli::interactive::pick("Open project", &items)?;

    let Some(idx) = selected else {
        return Ok(());
    };

    let project = &projects[idx];

    // Change to the project directory before launching
    std::env::set_current_dir(&project.path)?;

    if project.has_running_session {
        // Attach to existing session
        let session_manager = SessionManager::new();
        let project_dir_str = project.path.to_string_lossy().to_string();
        if let Some(session) = session_manager
            .find_session_for_project(None, &project_dir_str)?
            .filter(|s| s.can_attach())
        {
            crate::ui::factory::attach(Some(session.name))
        } else {
            // Session was running but can't attach — start fresh
            launch_factory_in_project()
        }
    } else {
        launch_factory_in_project()
    }
}

/// Launch a new factory session in the current directory (already cd'd into project).
fn launch_factory_in_project() -> Result<()> {
    use crate::cli::factory::FactoryArgs;
    use crate::cli::Cli;

    let factory_args = FactoryArgs::default();
    let cli = Cli {
        json: false,
        full: false,
        verbose: false,
        command: None,
    };

    // find_cas_root will pick up the .cas in the new cwd
    let cas_root = crate::store::find_cas_root().ok();
    super::factory::execute(&factory_args, &cli, cas_root.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_scan_empty_directory() {
        let tmp = TempDir::new().unwrap();
        let projects = scan_projects(tmp.path()).unwrap();
        assert!(projects.is_empty());
    }

    #[test]
    fn test_scan_nonexistent_directory() {
        let projects = scan_projects(Path::new("/nonexistent/path/abc123")).unwrap();
        assert!(projects.is_empty());
    }

    #[test]
    fn test_scan_finds_git_repos() {
        let tmp = TempDir::new().unwrap();

        // Create a git repo
        let repo_a = tmp.path().join("project-a");
        std::fs::create_dir_all(repo_a.join(".git")).unwrap();

        // Create a non-git directory (should be excluded)
        let non_repo = tmp.path().join("not-a-repo");
        std::fs::create_dir_all(&non_repo).unwrap();

        // Create another git repo
        let repo_b = tmp.path().join("project-b");
        std::fs::create_dir_all(repo_b.join(".git")).unwrap();

        let projects = scan_projects(tmp.path()).unwrap();
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].name, "project-a");
        assert_eq!(projects[1].name, "project-b");
        // No running sessions in test env
        assert!(!projects[0].has_running_session);
        assert!(!projects[1].has_running_session);
    }

    #[test]
    fn test_scan_ignores_files() {
        let tmp = TempDir::new().unwrap();

        // Create a file (not a directory)
        std::fs::write(tmp.path().join("some-file.txt"), "hello").unwrap();

        // Create a git repo
        let repo = tmp.path().join("real-project");
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        let projects = scan_projects(tmp.path()).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "real-project");
    }

    #[test]
    fn test_picker_items_stopped() {
        let projects = vec![ProjectEntry {
            name: "my-project".to_string(),
            path: PathBuf::from("/home/user/projects/my-project"),
            has_running_session: false,
        }];

        let items = projects_to_picker_items(&projects);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "my-project");
        assert_eq!(items[0].tags[0].text, "stopped");
    }

    #[test]
    fn test_picker_items_running() {
        let projects = vec![ProjectEntry {
            name: "active-project".to_string(),
            path: PathBuf::from("/home/user/projects/active-project"),
            has_running_session: true,
        }];

        let items = projects_to_picker_items(&projects);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "active-project");
        assert_eq!(items[0].tags[0].text, "running");
    }

    #[test]
    fn test_projects_sorted_alphabetically() {
        let tmp = TempDir::new().unwrap();

        // Create repos in non-alphabetical order
        for name in &["zebra", "apple", "mango"] {
            std::fs::create_dir_all(tmp.path().join(name).join(".git")).unwrap();
        }

        let projects = scan_projects(tmp.path()).unwrap();
        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["apple", "mango", "zebra"]);
    }
}
