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
    /// `[factory] artifacts_root/<project-key>/<task-id>` — new durable evidence.
    pub task_artifacts_dir: PathBuf,
    /// Historical flat evidence is readable/publishable, never a new write root.
    pub legacy_task_artifacts_dir: PathBuf,
    /// The project checkout: the parent of the `.cas` directory.
    pub project_root: PathBuf,
    task_id_is_valid: bool,
}

impl PublishRoots {
    /// Derive both roots from the resolved `.cas` directory and the task.
    pub fn new(cas_root: &Path, artifacts_root: &Path, task_id: &str) -> Self {
        Self {
            task_artifacts_dir: crate::config::project_factory_artifacts_root(cas_root, artifacts_root)
                .join(task_id),
            legacy_task_artifacts_dir: artifacts_root.join(task_id),
            task_id_is_valid: !task_id.is_empty()
                && task_id
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'),
            project_root: cas_root
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| cas_root.to_path_buf()),
        }
    }
}

/// A nested checkout is an exception to its parent's `.cas` ban only when Git
/// registers it in this repository and the candidate actually resolves there.
/// The toplevel check prevents a stale registration redirected by a symlink
/// from treating repository internals as checkout content.
fn registered_checkout_root(candidate: &Path, project_root: &Path) -> Option<PathBuf> {
    let git = |directory: &Path, args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .ok()?;
        output.status.success().then_some(output.stdout)
    };
    let registrations = git(project_root, &["worktree", "list", "--porcelain", "-z"])?;
    let checkout = registrations
        .split(|byte| *byte == 0)
        .filter_map(|line| line.strip_prefix(b"worktree "))
        .filter_map(|path| std::str::from_utf8(path).ok())
        .filter_map(|path| Path::new(path).canonicalize().ok())
        .filter(|path| candidate.starts_with(path))
        .max_by_key(|path| path.components().count())?;
    let toplevel = git(candidate.parent()?, &["rev-parse", "--show-toplevel"])?;
    let toplevel = Path::new(std::str::from_utf8(&toplevel).ok()?.trim())
        .canonicalize()
        .ok()?;
    (toplevel == checkout).then_some(checkout)
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
    let project_root = canonical(&roots.project_root);
    let base = roots.legacy_task_artifacts_dir.parent().unwrap_or(Path::new(""));
    let inside_task_dir = roots.task_id_is_valid && [&roots.task_artifacts_dir, &roots.legacy_task_artifacts_dir]
        .into_iter()
        .filter_map(|dir| crate::config::canonical_factory_task_artifact_dir(base, dir))
        .any(|dir| resolved.starts_with(dir));
    let inside_project = resolved.starts_with(&project_root);
    if !inside_task_dir && !inside_project {
        return Err(PathRefusal::OutsideRoots {
            resolved: resolved.display().to_string(),
            task_artifacts_dir: roots.task_artifacts_dir.display().to_string(),
            project_root: project_root.display().to_string(),
        });
    }

    // The sensitive-directory ban applies to the project root only: a task's
    // artifacts directory legitimately contains agent-authored evidence, while
    // `.cas` holds `cloud.json` (a bearer token) and `.git` holds remote URLs
    // that can embed credentials.
    if inside_project && !inside_task_dir {
        // Factory checkouts live below the primary project's `.cas`. Resolve
        // their registered root before interpreting `.cas` and `.git` as secrets.
        let checkout_root = if resolved.starts_with(project_root.join(".cas")) {
            registered_checkout_root(&resolved, &project_root)
                .unwrap_or_else(|| project_root.clone())
        } else {
            project_root.clone()
        };
        for forbidden in [checkout_root.join(".cas"), checkout_root.join(".git")] {
            let forbidden = canonical(&forbidden);
            // A checkout's .cas may alias the shared parent store. Its ancestor
            // is already checked using the candidate's actual checkout above.
            let shared_ancestor =
                checkout_root != project_root && checkout_root.starts_with(&forbidden);
            if !shared_ancestor && resolved.starts_with(&forbidden) {
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
        let task_dir = crate::config::project_factory_artifacts_root(&cas_root, &artifacts_root).join("cas-b72a");
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

    fn nested_worktree(f: &Fixture) -> PathBuf {
        let git = |args: &[&std::ffi::OsStr]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&f.project_root)
                .args([
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.test",
                ])
                .args(args)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_COMMON_DIR")
                .env_remove("GIT_INDEX_FILE")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init".as_ref()]);
        git(&[
            "commit".as_ref(),
            "--allow-empty".as_ref(),
            "-m".as_ref(),
            "fixture".as_ref(),
        ]);
        let checkout = f.project_root.join(".cas/worktrees/worker");
        git(&[
            "worktree".as_ref(),
            "add".as_ref(),
            "--detach".as_ref(),
            checkout.as_os_str(),
        ]);
        checkout
    }

    #[test]
    fn projects_with_the_same_task_id_cannot_publish_each_others_evidence_cas_6ebf() {
        let temp = TempDir::new().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let a = base.join("one/project/.cas");
        let b = base.join("two/project/.cas");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let artifact_base = base.join("artifacts");
        let a_roots = PublishRoots::new(&a, &artifact_base, "cas-a4b1");
        let b_roots = PublishRoots::new(&b, &artifact_base, "cas-a4b1");
        assert_ne!(a_roots.task_artifacts_dir, b_roots.task_artifacts_dir);
        let a_file = write(&a_roots.task_artifacts_dir.join("proof.md"), "project one");
        let b_file = write(&b_roots.task_artifacts_dir.join("proof.md"), "project two");
        assert_eq!(resolve_publishable_path(&a_file, &a_roots).unwrap(), a_file);
        assert_eq!(resolve_publishable_path(&b_file, &b_roots).unwrap(), b_file);
        assert!(resolve_publishable_path(&b_file, &a_roots).is_err());
        assert!(resolve_publishable_path(&a_file, &b_roots).is_err());
        let invalid = PublishRoots::new(
            &a,
            &artifact_base,
            b_roots.task_artifacts_dir.to_str().unwrap(),
        );
        assert!(resolve_publishable_path(&b_file, &invalid).is_err());
        let legacy = write(
            &a_roots.legacy_task_artifacts_dir.join("historical.md"),
            "old proof",
        );
        assert_eq!(resolve_publishable_path(&legacy, &a_roots).unwrap(), legacy);
        // New layout cannot attribute old flat evidence; it stays explicit,
        // readable, and outside automatic per-project cleanup.
        assert_eq!(fs::read_to_string(&a_file).unwrap(), "project one");
        assert_eq!(fs::read_to_string(&b_file).unwrap(), "project two");
    }

    #[cfg(unix)]
    #[test]
    fn artifact_namespace_aliases_do_not_cross_project_boundaries_cas_6ebf() {
        let f = fixture();
        let other = f
            .task_dir
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("foreign-project");
        fs::create_dir_all(other.join("cas-b72a")).unwrap();
        let file = write(&other.join("cas-b72a/proof.md"), "foreign");
        fs::remove_dir_all(f.task_dir.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&other, f.task_dir.parent().unwrap()).unwrap();
        let alias = f.task_dir.join("proof.md");
        assert!(resolve_publishable_path(&alias, &f.roots).is_err());
        assert!(resolve_publishable_path(&file, &f.roots).is_err());
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
    fn registered_nested_worktree_report_is_publishable_cas_9615() {
        let f = fixture();
        let checkout = nested_worktree(&f);
        let report = write(
            &checkout.join("docs/release-reports/v4.10.0.html"),
            "report",
        );
        assert_eq!(resolve_publishable_path(&report, &f.roots).unwrap(), report);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(f.project_root.join(".cas"), checkout.join(".cas")).unwrap();
            assert_eq!(resolve_publishable_path(&report, &f.roots).unwrap(), report);
        }
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
        // task_dir is <base>/artifacts/<project-key>/<task>; climb to <base>.
        let traversal = f
            .task_dir
            .join("..")
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
        let checkout = nested_worktree(&f);
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

        for path in [
            checkout.join(".cas/cloud.json"),
            checkout.join(".cas/cas.db"),
            checkout.join(".cas/keys/private.pem"),
            checkout.join(".git"),
            f.project_root
                .join(".cas/worktrees/unregistered/docs/report.html"),
        ] {
            if !path.exists() {
                write(&path, "protected");
            }
            assert!(
                matches!(
                    resolve_publishable_path(&path, &f.roots),
                    Err(PathRefusal::SensitiveDirectory { .. })
                ),
                "{}",
                path.display()
            );
        }
        #[cfg(unix)]
        {
            let alias = checkout.join("docs/credentials.json");
            fs::create_dir_all(alias.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&cloud, &alias).unwrap();
            assert!(matches!(
                resolve_publishable_path(&alias, &f.roots),
                Err(PathRefusal::SensitiveDirectory { .. })
            ));

            // A stale worktree registration must not authorize its new target.
            fs::rename(&checkout, f.project_root.join("moved-checkout")).unwrap();
            std::os::unix::fs::symlink(f.project_root.join(".cas"), &checkout).unwrap();
            assert!(matches!(
                resolve_publishable_path(&checkout.join("cloud.json"), &f.roots),
                Err(PathRefusal::SensitiveDirectory { .. })
            ));
        }
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
