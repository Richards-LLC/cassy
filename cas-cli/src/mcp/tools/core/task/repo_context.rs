use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use cas_store::KnownRepoStore;
use cas_types::WorkTarget;

use crate::bounded_process::{BoundedCommandError, Deadline, run_command};

/// Host-local repository evidence resolved fresh for one lifecycle mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoContext {
    pub repo_selector: String,
    pub repo_root: PathBuf,
    pub git_common_dir: PathBuf,
    pub target_branch: String,
}

/// Preflight-only repository probe. Lifecycle mutations retain their existing
/// resolver; preflight supplies one absolute deadline across every Git call.
#[derive(Debug, Clone)]
pub(crate) struct BoundedRepoProbe {
    deadline: Deadline,
    per_command_cap: Duration,
    candidate_limit: usize,
    git_program: PathBuf,
}

impl BoundedRepoProbe {
    pub(crate) fn new(
        deadline: Deadline,
        per_command_cap: Duration,
        candidate_limit: usize,
    ) -> Self {
        Self {
            deadline,
            per_command_cap,
            candidate_limit,
            git_program: PathBuf::from("git"),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_git_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.git_program = program.into();
        self
    }

    pub(crate) fn output(&self, path: &Path, args: &[&str]) -> Result<String, BoundedRepoError> {
        let output = run_command(
            Command::new(&self.git_program)
                .arg("-C")
                .arg(path)
                .args(args),
            self.deadline,
            self.per_command_cap,
        )
        .map_err(|error| match error {
            BoundedCommandError::TimedOut => BoundedRepoError::TimedOut,
            BoundedCommandError::Io => BoundedRepoError::Unavailable,
        })?;
        if !output.status.success() {
            return Err(BoundedRepoError::Unavailable);
        }
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if value.is_empty() {
            Err(BoundedRepoError::Unavailable)
        } else {
            Ok(value)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundedRepoError {
    TimedOut,
    CandidateLimit,
    Unavailable,
    Ambiguous,
}

fn git_output(path: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        Err("git returned an empty value".to_string())
    } else {
        Ok(value)
    }
}

fn canonical(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn git_layout(path: &Path) -> Result<(PathBuf, PathBuf), String> {
    let checkout_root = PathBuf::from(git_output(path, &["rev-parse", "--show-toplevel"])?);
    let common_raw = PathBuf::from(git_output(path, &["rev-parse", "--git-common-dir"])?);
    let common_dir = canonical(if common_raw.is_absolute() {
        common_raw
    } else {
        // `rev-parse --git-common-dir` is relative to Git's `-C <path>`,
        // not necessarily to the checkout root. Preflight intentionally
        // resolves from `<project>/.cas`; joining `../.git` to the checkout
        // root would escape to the parent repository and fabricate a mismatch.
        path.join(common_raw)
    });
    // Linked worktrees share `<main>/.git`; use its parent as the durable
    // host-local root. Ordinary repositories take the same path.
    let repo_root = common_dir
        .file_name()
        .filter(|name| *name == ".git")
        .and_then(|_| common_dir.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| canonical(checkout_root));
    Ok((canonical(repo_root), common_dir))
}

fn bounded_git_layout(
    path: &Path,
    probe: &BoundedRepoProbe,
) -> Result<(PathBuf, PathBuf), BoundedRepoError> {
    let checkout_root = PathBuf::from(probe.output(path, &["rev-parse", "--show-toplevel"])?);
    let common_raw = PathBuf::from(probe.output(path, &["rev-parse", "--git-common-dir"])?);
    let common_dir = canonical(if common_raw.is_absolute() {
        common_raw
    } else {
        path.join(common_raw)
    });
    let repo_root = common_dir
        .file_name()
        .filter(|name| *name == ".git")
        .and_then(|_| common_dir.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| canonical(checkout_root));
    Ok((canonical(repo_root), common_dir))
}

fn selector_for_repo(repo_root: &Path) -> Result<String, String> {
    let cas_root = repo_root.join(".cas");
    if let Some(project_id) = crate::cloud::canonical_id_from_config_toml(&cas_root)
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
    {
        return Ok(format!("project:{project_id}"));
    }
    crate::cloud::derive_canonical_id_from_git_remote(repo_root)
        .map(|remote| format!("remote:{remote}"))
        .ok_or_else(|| {
            format!(
                "repository {} has neither [project].canonical_id nor a normalizable origin URL",
                repo_root.display()
            )
        })
}

fn bounded_selector_for_repo(
    repo_root: &Path,
    probe: &BoundedRepoProbe,
) -> Result<String, BoundedRepoError> {
    let cas_root = repo_root.join(".cas");
    if let Some(project_id) = crate::cloud::canonical_id_from_config_toml(&cas_root)
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
    {
        return Ok(format!("project:{project_id}"));
    }
    let remote = probe.output(repo_root, &["remote", "get-url", "origin"])?;
    crate::cloud::normalize_git_remote_url(&remote)
        .map(|remote| format!("remote:{remote}"))
        .ok_or(BoundedRepoError::Unavailable)
}

pub(crate) fn resolve_default_branch(repo_root: &Path) -> Result<String, String> {
    if let Ok(reference) = git_output(repo_root, &["symbolic-ref", "refs/remotes/origin/HEAD"])
        && let Some(branch) = reference.strip_prefix("refs/remotes/origin/")
        && !branch.is_empty()
        && let Ok(branch) = validate_target_branch(repo_root, branch)
    {
        return Ok(branch);
    }
    for candidate in ["main", "master"] {
        if Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{candidate}"),
            ])
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
        {
            return Ok(candidate.to_string());
        }
    }
    Err(format!(
        "cannot resolve a default branch for {} (origin/HEAD, main, and master are absent)",
        repo_root.display()
    ))
}

/// Validate and normalize a branch name using Git's own branch grammar.
pub(crate) fn validate_target_branch(repo_root: &Path, branch: &str) -> Result<String, String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("WORK TARGET REJECTED: target branch is empty".to_string());
    }
    git_output(repo_root, &["check-ref-format", "--branch", branch]).map_err(|reason| {
        format!("WORK TARGET REJECTED: invalid target branch `{branch}`: {reason}")
    })
}

/// Create the portable durable target. An omitted target remains `None` for
/// legacy/non-git task stores; an explicitly supplied path always fails closed.
pub(crate) fn declare_work_target(
    cas_root: &Path,
    target_repo: Option<&str>,
    target_branch: Option<&str>,
) -> Result<Option<WorkTarget>, String> {
    if target_repo.is_none() && target_branch.is_none() {
        return Ok(None);
    }
    let input = target_repo
        .map(PathBuf::from)
        .unwrap_or_else(|| cas_root.to_path_buf());
    let (repo_root, _) = git_layout(&input).map_err(|reason| {
        format!(
            "WORK TARGET REJECTED: cannot resolve target repository {}: {reason}",
            input.display()
        )
    })?;
    let repo_selector = selector_for_repo(&repo_root)
        .map_err(|reason| format!("WORK TARGET REJECTED: {reason}"))?;
    let target_branch = match target_branch {
        Some(branch) => validate_target_branch(&repo_root, branch)?,
        None => resolve_default_branch(&repo_root)
            .map_err(|reason| format!("WORK TARGET REJECTED: {reason}"))?,
    };
    crate::store::known_repos::register_repo_strict(&repo_root).map_err(|error| {
        format!(
            "WORK TARGET REJECTED: failed to register {} in the host known-repo registry: {error}",
            repo_root.display()
        )
    })?;
    Ok(Some(WorkTarget {
        repo_selector,
        target_branch,
    }))
}

fn candidate_paths(cas_root: &Path) -> Vec<PathBuf> {
    let mut raw = Vec::new();
    if let Ok(store) = crate::store::known_repos::open_host_known_repo_store()
        && let Ok(known) = store.list()
    {
        raw.extend(known.into_iter().map(|repo| repo.path));
    }
    raw.push(cas_root.to_path_buf());
    let mut seen = HashSet::new();
    raw.into_iter()
        // The host registry intentionally retains historical checkouts. Avoid
        // one failed `git` process per deleted TempDir/stale path during
        // bounded preflight and lifecycle resolution.
        .filter(|path| path.exists())
        .filter_map(|path| git_layout(&path).ok().map(|(root, _)| root))
        .filter(|root| seen.insert(root.clone()))
        .collect()
}

fn bounded_registry_paths(candidate_limit: usize) -> Result<Vec<PathBuf>, BoundedRepoError> {
    let db_path = crate::store::known_repos::host_cas_dir().join("cas.db");
    if !db_path.is_file() {
        return Ok(Vec::new());
    }
    let connection = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| BoundedRepoError::Unavailable)?;
    connection
        .busy_timeout(Duration::from_millis(50))
        .map_err(|_| BoundedRepoError::Unavailable)?;
    let mut statement = match connection
        .prepare("SELECT path FROM known_repos ORDER BY last_touched_at DESC LIMIT ?1")
    {
        Ok(statement) => statement,
        // An uninitialized host registry is equivalent to no known repos.
        Err(_) => return Ok(Vec::new()),
    };
    let query_limit = candidate_limit.saturating_add(1) as i64;
    let paths = statement
        .query_map([query_limit], |row| row.get::<_, String>(0))
        .map_err(|_| BoundedRepoError::Unavailable)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| BoundedRepoError::Unavailable)?;
    if paths.len() > candidate_limit {
        return Err(BoundedRepoError::CandidateLimit);
    }
    Ok(paths.into_iter().map(PathBuf::from).collect())
}

fn bounded_candidate_paths(
    cas_root: &Path,
    probe: &BoundedRepoProbe,
) -> Result<Vec<(PathBuf, PathBuf)>, BoundedRepoError> {
    let mut raw = vec![cas_root.to_path_buf()];
    raw.extend(bounded_registry_paths(probe.candidate_limit)?);
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for path in raw.into_iter().filter(|path| path.exists()) {
        match bounded_git_layout(&path, probe) {
            Ok((root, common)) if seen.insert(root.clone()) => {
                candidates.push((root, common));
            }
            Ok(_) | Err(BoundedRepoError::Unavailable) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(candidates)
}

/// Resolve and identity-check a declared work target for one mutation.
pub(crate) fn resolve_repo_context(
    cas_root: &Path,
    target: &WorkTarget,
) -> Result<RepoContext, String> {
    let mut matches = Vec::new();
    for candidate in candidate_paths(cas_root) {
        if selector_for_repo(&candidate).ok().as_deref() == Some(&target.repo_selector)
            && let Ok((repo_root, git_common_dir)) = git_layout(&candidate)
        {
            matches.push((repo_root, git_common_dir));
        }
    }
    matches.sort();
    matches.dedup();
    match matches.as_slice() {
        [(repo_root, common_dir)] => {
            let target_branch = validate_target_branch(repo_root, &target.target_branch)?;
            Ok(RepoContext {
                repo_selector: target.repo_selector.clone(),
                repo_root: repo_root.clone(),
                git_common_dir: common_dir.clone(),
                target_branch,
            })
        }
        [] => Err(format!(
            "⚠️ WORK TARGET REPOSITORY MISMATCH\n\n\
             Task targets `{}`, but no current-host known repository or verified \
             path hint resolves to that selector. Register/open the target repo \
             with CAS, then retry. No git merge/reachability check was run.",
            target.repo_selector
        )),
        many => Err(format!(
            "⚠️ AMBIGUOUS WORK TARGET\n\n\
             Task selector `{}` matched {} repositories on this host. Refusing \
             lifecycle mutation before git merge/reachability checks.",
            target.repo_selector,
            many.len()
        )),
    }
}

pub(crate) fn resolve_repo_context_bounded(
    cas_root: &Path,
    target: &WorkTarget,
    probe: &BoundedRepoProbe,
) -> Result<RepoContext, BoundedRepoError> {
    let mut matches = Vec::new();
    for (repo_root, git_common_dir) in bounded_candidate_paths(cas_root, probe)? {
        match bounded_selector_for_repo(&repo_root, probe) {
            Ok(selector) if selector == target.repo_selector => {
                matches.push((repo_root, git_common_dir));
            }
            Ok(_) | Err(BoundedRepoError::Unavailable) => {}
            Err(error) => return Err(error),
        }
    }
    matches.sort();
    matches.dedup();
    match matches.as_slice() {
        [(repo_root, common_dir)] => probe
            .output(
                repo_root,
                &["check-ref-format", "--branch", &target.target_branch],
            )
            .map(|target_branch| RepoContext {
                repo_selector: target.repo_selector.clone(),
                repo_root: repo_root.clone(),
                git_common_dir: common_dir.clone(),
                target_branch,
            }),
        [] => Err(BoundedRepoError::Unavailable),
        _ => Err(BoundedRepoError::Ambiguous),
    }
}

pub(crate) fn resolve_path_context(
    path: &Path,
    target_branch: &str,
) -> Result<RepoContext, String> {
    let (repo_root, git_common_dir) = git_layout(path)?;
    Ok(RepoContext {
        repo_selector: selector_for_repo(&repo_root)?,
        repo_root,
        git_common_dir,
        target_branch: target_branch.to_string(),
    })
}

pub(crate) fn resolve_path_context_bounded(
    path: &Path,
    target_branch: &str,
    probe: &BoundedRepoProbe,
) -> Result<RepoContext, BoundedRepoError> {
    let (repo_root, git_common_dir) = bounded_git_layout(path, probe)?;
    Ok(RepoContext {
        repo_selector: bounded_selector_for_repo(&repo_root, probe)?,
        repo_root,
        git_common_dir,
        target_branch: target_branch.to_string(),
    })
}

pub(crate) fn validate_worktree_binding(
    task_id: &str,
    expected: &RepoContext,
    actual: &RepoContext,
    actual_branch: &str,
    worktree_path: &Path,
) -> Result<(), String> {
    if actual.repo_selector == expected.repo_selector && actual_branch == expected.target_branch {
        return Ok(());
    }
    Err(format!(
        "⚠️ WORKTREE REPOSITORY MISMATCH\n\n\
         Task {task_id} targets repository `{}` branch `{}`, but worktree {} \
         resolves to repository `{}` branch `{actual_branch}`. Refusing before \
         merge/reachability checks.",
        expected.repo_selector,
        expected.target_branch,
        worktree_path.display(),
        actual.repo_selector,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnvGuard;

    fn git(repo: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .status()
                .unwrap()
                .success()
        );
    }

    #[cfg(unix)]
    #[test]
    fn bounded_git_probe_returns_typed_timeout_without_command_details() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let fake_git = dir.path().join("slow-git");
        std::fs::write(&fake_git, "#!/bin/sh\nsleep 10\n").unwrap();
        std::fs::set_permissions(&fake_git, std::fs::Permissions::from_mode(0o700)).unwrap();
        let probe = BoundedRepoProbe::new(
            Deadline::after(Duration::from_millis(75)),
            Duration::from_millis(75),
            8,
        )
        .with_git_program(fake_git);
        let started = std::time::Instant::now();
        assert_eq!(
            probe.output(dir.path(), &["rev-parse", "HEAD"]),
            Err(BoundedRepoError::TimedOut)
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn bounded_registry_fails_closed_before_scanning_too_many_host_repos() {
        TestEnvGuard::run_with_temp_home(|home| {
            crate::store::known_repos::ensure_host_schema().unwrap();
            for index in 0..3 {
                let repo = home.join(format!("repo-{index}"));
                std::fs::create_dir(&repo).unwrap();
                crate::store::known_repos::register_repo_strict(&repo).unwrap();
            }
            assert_eq!(
                bounded_registry_paths(2),
                Err(BoundedRepoError::CandidateLimit)
            );
        });
    }

    #[test]
    fn linked_worktree_and_symlink_share_selector_and_common_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let main = dir.path().join("main");
        std::fs::create_dir(&main).unwrap();
        git(&main, &["init", "-q", "-b", "master"]);
        git(
            &main,
            &["remote", "add", "origin", "git@github.com:org/repo.git"],
        );
        std::fs::write(main.join("a"), "a").unwrap();
        git(&main, &["add", "a"]);
        git(
            &main,
            &[
                "-c",
                "user.name=CAS",
                "-c",
                "user.email=cas@example.com",
                "commit",
                "-q",
                "-m",
                "base",
            ],
        );
        let linked = dir.path().join("linked");
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "factory/w",
                linked.to_str().unwrap(),
            ],
        );
        #[cfg(unix)]
        std::os::unix::fs::symlink(&linked, dir.path().join("alias")).unwrap();

        let a = resolve_path_context(&main, "master").unwrap();
        let b = resolve_path_context(&linked, "master").unwrap();
        assert_eq!(a.repo_selector, b.repo_selector);
        assert_eq!(a.git_common_dir, b.git_common_dir);
        #[cfg(unix)]
        {
            let c = resolve_path_context(&dir.path().join("alias"), "master").unwrap();
            assert_eq!(a.git_common_dir, c.git_common_dir);
        }
    }

    #[test]
    fn nonexistent_known_repo_is_skipped_before_git_resolution() {
        TestEnvGuard::run_with_temp_home(|home| {
            crate::store::known_repos::ensure_host_schema().unwrap();
            let stale = home.join("deleted-checkout");
            std::fs::create_dir_all(&stale).unwrap();
            crate::store::known_repos::register_repo_strict(&stale).unwrap();
            std::fs::remove_dir(&stale).unwrap();

            let project = home.join("active-project");
            std::fs::create_dir_all(&project).unwrap();
            git(&project, &["init", "-q", "-b", "main"]);
            git(
                &project,
                &[
                    "remote",
                    "add",
                    "origin",
                    "git@example.invalid:org/active-project.git",
                ],
            );
            std::fs::create_dir(project.join(".cas")).unwrap();
            std::fs::write(
                project.join(".cas/config.toml"),
                "[project]\ncanonical_id = \"active-project\"\n",
            )
            .unwrap();

            let target = WorkTarget {
                repo_selector: "project:active-project".to_string(),
                target_branch: "main".to_string(),
            };
            let resolved =
                resolve_repo_context(&project.join(".cas"), &target).expect("active repo resolves");
            assert_eq!(resolved.repo_selector, target.repo_selector);
            assert_eq!(resolved.target_branch, "main");
            assert!(
                !stale.exists(),
                "the stale registry entry must remain nonexistent"
            );
        });
    }

    #[test]
    fn explicit_target_is_normalized_registered_and_path_free_when_serialized() {
        TestEnvGuard::run_with_temp_home(|home| {
            crate::store::known_repos::ensure_host_schema().unwrap();
            let repo = home.join("checkout");
            std::fs::create_dir(&repo).unwrap();
            git(&repo, &["init", "-q", "-b", "main"]);
            git(
                &repo,
                &["remote", "add", "origin", "https://github.com/Org/Repo.git"],
            );
            std::fs::write(repo.join("base"), "base").unwrap();
            git(&repo, &["add", "base"]);
            git(
                &repo,
                &[
                    "-c",
                    "user.name=CAS",
                    "-c",
                    "user.email=cas@example.com",
                    "commit",
                    "-q",
                    "-m",
                    "base",
                ],
            );
            std::fs::create_dir(repo.join(".cas")).unwrap();

            let target =
                declare_work_target(&repo.join(".cas"), Some(repo.to_str().unwrap()), None)
                    .unwrap()
                    .unwrap();
            assert_eq!(target.repo_selector, "remote:github.com/Org/Repo");
            assert_eq!(target.target_branch, "main");

            let json = serde_json::to_string(&target).unwrap();
            assert!(!json.contains(home.to_string_lossy().as_ref()));
            assert!(!json.contains("repo_root"));
            assert!(!json.contains("git_common_dir"));

            let known = crate::store::known_repos::open_host_known_repo_store()
                .unwrap()
                .list()
                .unwrap();
            assert_eq!(known.len(), 1);
            assert_eq!(known[0].path, repo.canonicalize().unwrap());
        });
    }

    #[test]
    fn declared_repo_b_resolves_from_repo_a_spawn_context() {
        TestEnvGuard::run_with_temp_home(|home| {
            crate::store::known_repos::ensure_host_schema().unwrap();
            let repo_a = home.join("spawn-a");
            let repo_b = home.join("work-b");
            for (repo, remote) in [
                (&repo_a, "git@github.com:org/spawn-a.git"),
                (&repo_b, "git@github.com:org/work-b.git"),
            ] {
                std::fs::create_dir(repo).unwrap();
                git(repo, &["init", "-q", "-b", "main"]);
                git(repo, &["remote", "add", "origin", remote]);
                std::fs::create_dir(repo.join(".cas")).unwrap();
            }
            let target = declare_work_target(
                &repo_a.join(".cas"),
                Some(repo_b.to_str().unwrap()),
                Some("main"),
            )
            .unwrap()
            .unwrap();
            let context = resolve_repo_context(&repo_a.join(".cas"), &target).unwrap();
            assert_eq!(context.repo_root, repo_b.canonicalize().unwrap());
            assert_eq!(context.repo_selector, "remote:github.com/org/work-b");
        });
    }

    #[test]
    fn unknown_selector_fails_before_git_evidence_is_consulted() {
        TestEnvGuard::run_with_temp_home(|_| {
            crate::store::known_repos::ensure_host_schema().unwrap();
            let dir = tempfile::TempDir::new().unwrap();
            let target = WorkTarget {
                repo_selector: "remote:github.com/missing/repo".to_string(),
                target_branch: "main".to_string(),
            };
            let error = resolve_repo_context(dir.path(), &target).unwrap_err();
            assert!(error.contains("no current-host known repository"));
            assert!(error.contains("No git merge/reachability check was run"));
        });
    }

    #[test]
    fn default_branch_master_survives_detached_head() {
        let dir = tempfile::TempDir::new().unwrap();
        git(dir.path(), &["init", "-q", "-b", "master"]);
        std::fs::write(dir.path().join("a"), "a").unwrap();
        git(dir.path(), &["add", "a"]);
        git(
            dir.path(),
            &[
                "-c",
                "user.name=CAS",
                "-c",
                "user.email=cas@example.com",
                "commit",
                "-q",
                "-m",
                "base",
            ],
        );
        git(dir.path(), &["checkout", "-q", "--detach"]);
        assert_eq!(resolve_default_branch(dir.path()).unwrap(), "master");
    }

    #[test]
    fn branch_validation_uses_git_ref_grammar() {
        let dir = tempfile::TempDir::new().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        assert_eq!(
            validate_target_branch(dir.path(), "feature/valid").unwrap(),
            "feature/valid"
        );
        for invalid in ["-option", "bad..name", "bad name", "trailing."] {
            assert!(
                validate_target_branch(dir.path(), invalid).is_err(),
                "Git must reject invalid branch {invalid:?}"
            );
        }
    }

    #[test]
    fn epic_branch_scan_uses_declared_repo_not_process_cwd() {
        let mut guard = TestEnvGuard::temp_home();
        let repo_a = guard.home().join("spawn-a");
        let repo_b = guard.home().join("work-b");
        for repo in [&repo_a, &repo_b] {
            std::fs::create_dir(repo).unwrap();
            git(repo, &["init", "-q", "-b", "main"]);
            std::fs::write(repo.join("base"), "base").unwrap();
            git(repo, &["add", "base"]);
            git(
                repo,
                &[
                    "-c",
                    "user.name=CAS",
                    "-c",
                    "user.email=cas@example.com",
                    "commit",
                    "-q",
                    "-m",
                    "base",
                ],
            );
        }

        git(&repo_a, &["checkout", "-q", "-b", "cas-epic/noise"]);
        std::fs::write(repo_a.join("noise"), "repo a only").unwrap();
        git(&repo_a, &["add", "noise"]);
        git(
            &repo_a,
            &[
                "-c",
                "user.name=CAS",
                "-c",
                "user.email=cas@example.com",
                "commit",
                "-q",
                "-m",
                "noise",
            ],
        );
        guard.set_current_dir(&repo_a);

        assert!(
            crate::mcp::tools::check_unmerged_epic_branches(&repo_b, "cas-epic", "main").is_empty(),
            "repo A epic branch noise must not contaminate repo B close"
        );

        git(&repo_b, &["checkout", "-q", "-b", "cas-epic/real"]);
        std::fs::write(repo_b.join("work"), "repo b").unwrap();
        git(&repo_b, &["add", "work"]);
        git(
            &repo_b,
            &[
                "-c",
                "user.name=CAS",
                "-c",
                "user.email=cas@example.com",
                "commit",
                "-q",
                "-m",
                "work",
            ],
        );
        git(&repo_b, &["checkout", "-q", "main"]);
        assert_eq!(
            crate::mcp::tools::check_unmerged_epic_branches(&repo_b, "cas-epic", "main"),
            vec!["cas-epic/real"]
        );
    }

    #[test]
    fn wrong_spawn_repo_is_not_used_for_declared_repo_close_gate() {
        use crate::mcp::tools::TaskCloseRequest;
        use crate::mcp::tools::core::task::lifecycle::close_ops::{
            MergeStateGateOutcome, TaskCommitReceiptWindow, count_unmerged_factory_commits,
            run_factory_branch_merge_gate, validate_task_commit_receipt,
        };
        use crate::types::{Task, TaskStatus};

        TestEnvGuard::run_with_temp_home(|home| {
            crate::store::known_repos::ensure_host_schema().unwrap();
            let repo_a = home.join("spawn-a");
            let repo_b = home.join("work-b");
            for (repo, remote) in [
                (&repo_a, "git@github.com:org/spawn-a.git"),
                (&repo_b, "git@github.com:org/work-b.git"),
            ] {
                std::fs::create_dir(repo).unwrap();
                git(repo, &["init", "-q", "-b", "master"]);
                git(repo, &["remote", "add", "origin", remote]);
                std::fs::write(repo.join("base"), "base").unwrap();
                git(repo, &["add", "base"]);
                git(
                    repo,
                    &[
                        "-c",
                        "user.name=CAS",
                        "-c",
                        "user.email=cas@example.com",
                        "commit",
                        "-q",
                        "-m",
                        "base",
                    ],
                );
                std::fs::create_dir(repo.join(".cas")).unwrap();
            }

            // Spawn repo A carries inherited epic history not on its trunk.
            git(&repo_a, &["checkout", "-q", "-b", "epic/x"]);
            std::fs::write(repo_a.join("epic"), "inherited").unwrap();
            git(&repo_a, &["add", "epic"]);
            git(
                &repo_a,
                &[
                    "-c",
                    "user.name=CAS",
                    "-c",
                    "user.email=cas@example.com",
                    "commit",
                    "-q",
                    "-m",
                    "epic",
                ],
            );
            git(&repo_a, &["checkout", "-q", "-b", "factory/worker"]);

            // Actual work is in B and has already landed on B/master.
            git(&repo_b, &["checkout", "-q", "-b", "factory/worker"]);
            std::fs::write(repo_b.join("feature"), "done").unwrap();
            git(&repo_b, &["add", "feature"]);
            git(
                &repo_b,
                &[
                    "-c",
                    "user.name=CAS",
                    "-c",
                    "user.email=cas@example.com",
                    "commit",
                    "-q",
                    "-m",
                    "feature",
                ],
            );
            let receipt = git_output(&repo_b, &["rev-parse", "HEAD"]).unwrap();
            git(&repo_b, &["checkout", "-q", "master"]);
            git(
                &repo_b,
                &[
                    "-c",
                    "user.name=CAS",
                    "-c",
                    "user.email=cas@example.com",
                    "merge",
                    "-q",
                    "--no-ff",
                    "factory/worker",
                ],
            );

            let target = declare_work_target(
                &repo_a.join(".cas"),
                Some(repo_b.to_str().unwrap()),
                Some("master"),
            )
            .unwrap()
            .unwrap();
            let context = resolve_repo_context(&repo_a.join(".cas"), &target).unwrap();
            let mut task = Task::new("cas-cross".to_string(), "cross repo".to_string());
            task.assignee = Some("worker".to_string());
            task.status = TaskStatus::InProgress;
            task.deliverables.work_target = Some(target);
            let request = TaskCloseRequest {
                id: task.id.clone(),
                reason: None,
                bypass_code_review: Some(true),
                code_review_findings: None,
                search_manifest: None,
                commit_receipt: None,
            };

            assert_eq!(
                count_unmerged_factory_commits(&repo_a, "factory/worker", "master"),
                1,
                "precondition: spawn repo would falsely report inherited epic history"
            );
            assert!(matches!(
                run_factory_branch_merge_gate(
                    &task,
                    &request,
                    &context.target_branch,
                    &context.repo_root
                ),
                MergeStateGateOutcome::Proceed
            ));

            let window = TaskCommitReceiptWindow {
                not_before: chrono::DateTime::from_timestamp(0, 0).unwrap(),
                basis: "test task creation",
            };
            assert!(
                validate_task_commit_receipt(&repo_a, &receipt, "master", &window).is_err(),
                "receipt absent from spawn repo must never validate"
            );
            assert!(
                validate_task_commit_receipt(
                    &context.repo_root,
                    &receipt,
                    &context.target_branch,
                    &window
                )
                .is_ok(),
                "receipt must validate in the declared work repository"
            );
        });
    }

    #[test]
    fn worktree_binding_rejects_repo_or_branch_mismatch_before_merge() {
        let expected = RepoContext {
            repo_selector: "remote:github.com/org/work".to_string(),
            repo_root: PathBuf::from("/runtime/work"),
            git_common_dir: PathBuf::from("/runtime/work/.git"),
            target_branch: "master".to_string(),
        };
        let wrong_repo = RepoContext {
            repo_selector: "remote:github.com/org/spawn".to_string(),
            repo_root: PathBuf::from("/runtime/spawn"),
            git_common_dir: PathBuf::from("/runtime/spawn/.git"),
            target_branch: "master".to_string(),
        };
        let error = validate_worktree_binding(
            "cas-x",
            &expected,
            &wrong_repo,
            "master",
            Path::new("/runtime/spawn/wt"),
        )
        .unwrap_err();
        assert!(error.contains("before merge/reachability checks"));

        assert!(
            validate_worktree_binding(
                "cas-x",
                &expected,
                &expected,
                "main",
                Path::new("/runtime/work/wt")
            )
            .is_err()
        );
        assert!(
            validate_worktree_binding(
                "cas-x",
                &expected,
                &expected,
                "master",
                Path::new("/runtime/work/wt")
            )
            .is_ok()
        );
    }
}
