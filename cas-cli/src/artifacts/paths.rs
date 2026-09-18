//! Where a publishable file may live, and how that is proven.
//!
//! The guard is a resolution, not a string comparison: the candidate and both
//! permitted roots are canonicalised, so a symlink pointing out of the task's
//! artifacts directory resolves to its real location and fails the
//! containment check. Comparing the lexical path would accept
//! `artifacts/cas-b72a/escape -> /etc/shadow`.

use std::path::{Path, PathBuf};

/// The two places a worker may publish from.
#[derive(Debug, Clone)]
pub struct PublishRoots {
    /// `[factory] artifacts_root/<task-id>` — durable per-task evidence.
    pub task_artifacts_dir: PathBuf,
    /// The project checkout: the parent of the `.cas` directory.
    pub project_root: PathBuf,
}

impl PublishRoots {
    /// Derive both roots from the resolved `.cas` directory and the task.
    pub fn new(cas_root: &Path, artifacts_root: &Path, task_id: &str) -> Self {
        Self {
            task_artifacts_dir: artifacts_root.join(task_id),
            project_root: cas_root
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| cas_root.to_path_buf()),
        }
    }

    /// Directories inside the project that hold credentials or history and are
    /// never publishable, however the caller reached them.
    fn forbidden_subdirectories(&self) -> [PathBuf; 2] {
        [
            self.project_root.join(".cas"),
            self.project_root.join(".git"),
        ]
    }
}

/// Why a path may not be published. Each variant names what the operator can
/// do about it; a refusal that only says "invalid path" costs a round trip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathRefusal {
    Missing(String),
    NotAFile(String),
    Unreadable(String),
    OutsideRoots {
        resolved: String,
        task_artifacts_dir: String,
        project_root: String,
    },
    SensitiveDirectory {
        resolved: String,
        directory: String,
    },
}

impl std::fmt::Display for PathRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathRefusal::Missing(path) => write!(f, "no file at {path}"),
            PathRefusal::NotAFile(path) => {
                write!(
                    f,
                    "{path} is not a regular file; publish one file at a time"
                )
            }
            PathRefusal::Unreadable(detail) => write!(f, "cannot resolve the path: {detail}"),
            PathRefusal::OutsideRoots {
                resolved,
                task_artifacts_dir,
                project_root,
            } => write!(
                f,
                "{resolved} is outside this task's publishable roots.\n  \
                 Publish from {task_artifacts_dir} (durable task evidence) or from {project_root} (the checkout)."
            ),
            PathRefusal::SensitiveDirectory {
                resolved,
                directory,
            } => write!(
                f,
                "{resolved} is inside {directory}, which holds credentials and repository internals; \
                 it is never publishable. Copy what you meant to share into this task's artifacts directory first."
            ),
        }
    }
}

impl std::error::Error for PathRefusal {}

/// Resolve `candidate` to a real, publishable file, or say why not.
///
/// Returns the canonical path — callers hash and upload *that*, so the bytes
/// that were checked are the bytes that are sent.
pub fn resolve_publishable_path(
    candidate: &Path,
    roots: &PublishRoots,
) -> Result<PathBuf, PathRefusal> {
    let resolved = candidate.canonicalize().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            PathRefusal::Missing(candidate.display().to_string())
        } else {
            PathRefusal::Unreadable(format!("{}: {error}", candidate.display()))
        }
    })?;

    let metadata = std::fs::metadata(&resolved)
        .map_err(|error| PathRefusal::Unreadable(format!("{}: {error}", resolved.display())))?;
    if !metadata.is_file() {
        return Err(PathRefusal::NotAFile(resolved.display().to_string()));
    }

    // A root that does not exist cannot contain anything; canonicalising it
    // would fail, so fall back to the lexical form, which then simply never
    // matches. That is the safe direction.
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let task_dir = canonical(&roots.task_artifacts_dir);
    let project_root = canonical(&roots.project_root);

    let inside_task_dir = resolved.starts_with(&task_dir);
    let inside_project = resolved.starts_with(&project_root);
    if !inside_task_dir && !inside_project {
        return Err(PathRefusal::OutsideRoots {
            resolved: resolved.display().to_string(),
            task_artifacts_dir: task_dir.display().to_string(),
            project_root: project_root.display().to_string(),
        });
    }

    // The sensitive-directory ban applies to the project root only: a task's
    // artifacts directory legitimately contains agent-authored evidence, while
    // `.cas` holds `cloud.json` (a bearer token) and `.git` holds remote URLs
    // that can embed credentials.
    if inside_project && !inside_task_dir {
        for forbidden in roots.forbidden_subdirectories() {
            let forbidden = canonical(&forbidden);
            if resolved.starts_with(&forbidden) {
                return Err(PathRefusal::SensitiveDirectory {
                    resolved: resolved.display().to_string(),
                    directory: forbidden.display().to_string(),
                });
            }
        }
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    struct Fixture {
        _dir: TempDir,
        roots: PublishRoots,
        project_root: PathBuf,
        task_dir: PathBuf,
        outside: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = TempDir::new().unwrap();
        // Canonicalise up front: on macOS the temp dir is itself a symlink
        // (/var -> /private/var), which would make every containment check
        // fail for reasons that have nothing to do with the guard.
        let base = dir.path().canonicalize().unwrap();
        let project_root = base.join("project");
        let cas_root = project_root.join(".cas");
        let artifacts_root = base.join("artifacts");
        let task_dir = artifacts_root.join("cas-b72a");
        let outside = base.join("outside");
        for path in [&project_root, &cas_root, &task_dir, &outside] {
            fs::create_dir_all(path).unwrap();
        }
        fs::create_dir_all(project_root.join(".git")).unwrap();
        let roots = PublishRoots::new(&cas_root, &artifacts_root, "cas-b72a");
        Fixture {
            _dir: dir,
            roots,
            project_root,
            task_dir,
            outside,
        }
    }

    fn write(path: &Path, body: &str) -> PathBuf {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
        path.to_path_buf()
    }

    #[test]
    fn a_file_in_the_task_artifacts_directory_is_publishable() {
        let f = fixture();
        let file = write(&f.task_dir.join("brief.pdf"), "pdf");
        let resolved = resolve_publishable_path(&file, &f.roots).unwrap();
        assert_eq!(resolved, file);
    }

    #[test]
    fn a_file_in_the_checkout_is_publishable() {
        let f = fixture();
        let file = write(&f.project_root.join("docs").join("report.html"), "html");
        let resolved = resolve_publishable_path(&file, &f.roots).unwrap();
        assert_eq!(resolved, file);
    }

    #[test]
    fn a_file_outside_both_roots_is_refused_and_the_message_names_both() {
        let f = fixture();
        let file = write(&f.outside.join("secrets.txt"), "nope");
        let refusal = resolve_publishable_path(&file, &f.roots).unwrap_err();
        let rendered = refusal.to_string();
        assert!(matches!(refusal, PathRefusal::OutsideRoots { .. }));
        assert!(
            rendered.contains("cas-b72a") && rendered.contains("project"),
            "the refusal must name where publishing IS allowed: {rendered}"
        );
    }

    #[test]
    fn a_symlink_escaping_the_task_directory_is_refused() {
        let f = fixture();
        let target = write(&f.outside.join("escape-target.pdf"), "outside bytes");
        let link = f.task_dir.join("looks-local.pdf");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();
        #[cfg(not(unix))]
        return;

        let refusal = resolve_publishable_path(&link, &f.roots).unwrap_err();
        assert!(
            matches!(refusal, PathRefusal::OutsideRoots { .. }),
            "a lexical check would have accepted this path: {refusal}"
        );
    }

    #[test]
    fn a_relative_traversal_out_of_the_task_directory_is_refused() {
        let f = fixture();
        write(&f.outside.join("escape.pdf"), "outside bytes");
        let traversal = f
            .task_dir
            .join("..")
            .join("..")
            .join("outside")
            .join("escape.pdf");
        let refusal = resolve_publishable_path(&traversal, &f.roots).unwrap_err();
        assert!(
            matches!(refusal, PathRefusal::OutsideRoots { .. }),
            "{refusal}"
        );
    }

    #[test]
    fn a_symlink_that_stays_inside_a_root_is_accepted_at_its_real_path() {
        let f = fixture();
        let target = write(&f.task_dir.join("real.pdf"), "bytes");
        let link = f.task_dir.join("alias.pdf");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();
        #[cfg(not(unix))]
        return;

        let resolved = resolve_publishable_path(&link, &f.roots).unwrap();
        assert_eq!(
            resolved, target,
            "the resolved path is what gets hashed, so it must be the real file"
        );
    }

    #[test]
    fn the_credential_and_git_directories_are_never_publishable() {
        let f = fixture();
        let cloud = write(
            &f.project_root.join(".cas").join("cloud.json"),
            "{\"token\":\"x\"}",
        );
        let refusal = resolve_publishable_path(&cloud, &f.roots).unwrap_err();
        assert!(
            matches!(refusal, PathRefusal::SensitiveDirectory { .. }),
            "publishing the token cache must be refused, not merely discouraged: {refusal}"
        );

        let git_config = write(&f.project_root.join(".git").join("config"), "[remote]");
        assert!(matches!(
            resolve_publishable_path(&git_config, &f.roots).unwrap_err(),
            PathRefusal::SensitiveDirectory { .. }
        ));
    }

    #[test]
    fn a_missing_path_and_a_directory_are_refused_distinctly() {
        let f = fixture();
        assert!(matches!(
            resolve_publishable_path(&f.task_dir.join("absent.pdf"), &f.roots).unwrap_err(),
            PathRefusal::Missing(_)
        ));
        assert!(matches!(
            resolve_publishable_path(&f.task_dir, &f.roots).unwrap_err(),
            PathRefusal::NotAFile(_)
        ));
    }

    #[test]
    fn another_tasks_artifacts_directory_is_not_publishable_from_this_task() {
        let f = fixture();
        let other = f.task_dir.parent().unwrap().join("cas-other");
        fs::create_dir_all(&other).unwrap();
        let file = write(&other.join("theirs.pdf"), "bytes");
        assert!(
            matches!(
                resolve_publishable_path(&file, &f.roots).unwrap_err(),
                PathRefusal::OutsideRoots { .. }
            ),
            "the guard is scoped to THIS task's directory"
        );
    }
}
