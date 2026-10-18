//! Read-only effective Git hook authority, independent of the host registry.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::bounded_process::{Deadline, run_command};

use super::{Check, CheckStatus};

/// The store can live in the main checkout while doctor runs in a worker.
/// Report both authorities when cwd belongs to that same Git repository.
pub(super) fn checks(project: &Path) -> Vec<Check> {
    let mut rows: Vec<_> = check(project).into_iter().collect();
    let Ok(cwd) = std::env::current_dir() else {
        return rows;
    };
    if cwd == project {
        return rows;
    }
    let identity = |dir: &Path, argument: &str| {
        let cap = Duration::from_secs(2);
        let mut command = Command::new("git");
        command
            .current_dir(dir)
            .args(["rev-parse", "--path-format=absolute", argument]);
        run_command(&mut command, Deadline::after(cap), cap)
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    };
    if let (Some(project_common), Some(cwd_common)) = (
        identity(project, "--git-common-dir"),
        identity(&cwd, "--git-common-dir"),
    ) && project_common == cwd_common
        && identity(project, "--show-toplevel") != identity(&cwd, "--show-toplevel")
        && let Some(row) = check(&cwd)
    {
        rows.push(row);
    }
    rows
}

fn check(project: &Path) -> Option<Check> {
    const NAME: &str = "Git hooks path";
    let cap = Duration::from_secs(2);
    let deadline = Deadline::after(cap);
    let git = |cwd: &Path, args: &[&str]| {
        let mut command = Command::new("git");
        command.current_dir(cwd).args(args);
        run_command(&mut command, deadline, cap)
    };
    let warning = |message| Check::new(NAME, CheckStatus::Warning, message);
    let configured = match git(project, &["config", "--get", "core.hooksPath"]) {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string(),
        Ok(output) if output.status.code() == Some(1) => return None,
        _ => {
            return Some(warning(
                "Could not inspect effective core.hooksPath".to_string(),
            ));
        }
    };
    // Git runs commit/push hooks from the worktree root, not the caller's
    // subdirectory. Let Git resolve tilde expansion and worktree config too.
    let root = match git(project, &["rev-parse", "--show-toplevel"]) {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string(),
        _ => return None, // No working checkout, hence no checkout hook to diagnose.
    };
    let resolved = match git(
        Path::new(&root),
        &["rev-parse", "--path-format=absolute", "--git-path", "hooks"],
    ) {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string(),
        _ => {
            return Some(warning(format!(
                "Could not resolve core.hooksPath={configured}"
            )));
        }
    };
    let path = Path::new(&resolved);
    let status = match path.metadata() {
        Ok(metadata) if metadata.is_dir() => {
            return Some(Check::new(
                NAME,
                CheckStatus::Ok,
                format!(
                    "checkout {root}: core.hooksPath={configured}: directory exists at {resolved}"
                ),
            ));
        }
        Ok(_) => "Hooks path is not a directory",
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "Hooks directory is missing",
        Err(_) => "Hooks directory could not be inspected",
    };
    Some(warning(format!(
        "{status} at {resolved}; core.hooksPath={configured}; checkout {root}. Run `git config --show-origin --show-scope --get core.hooksPath` in that checkout, then restore the intended hooks with the project's documented installer; factory workers need their own guards refreshed through spawn/reuse. Review authority before changing config"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_resolves_dangling_hooks_path_in_checkout_and_worktree_cas_edb0() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("GIT_CONFIG_NOSYSTEM", "1");
        env.set("GIT_CONFIG_GLOBAL", env.home().join("empty-gitconfig"));
        let repo = env.home().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |cwd: &Path, args: &[&str]| {
            let output = Command::new("git")
                .current_dir(cwd)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&repo, &["init", "-q", "-b", "main"]);
        assert!(check(&repo).is_none());
        git(
            &repo,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-qm",
                "fixture",
            ],
        );
        let subdir = repo.join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        let missing = repo.join("deleted-cas-src/hooks");
        for configured in [missing.to_str().unwrap(), "deleted-cas-src/hooks"] {
            git(&repo, &["config", "core.hooksPath", configured]);
            let before = std::fs::read(repo.join(".git/config")).unwrap();
            let row = check(&subdir).expect("configured hooksPath must be checked");
            let json = super::super::serialize_checks(&[row], &[]);
            assert_eq!(json[0]["group"], "config");
            assert_eq!(json[0]["status"], "warning");
            // pin: default doctor clips at three lines; the cause must lead even when paths are long.
            assert!(
                json[0]["message"]
                    .as_str()
                    .unwrap()
                    .starts_with("Hooks directory is missing")
            );
            assert!(
                json[0]["message"]
                    .as_str()
                    .unwrap()
                    .contains(missing.to_str().unwrap()),
                "{json:?}"
            );
            let remedy = json[0]["remediation"].as_str().unwrap();
            assert!(
                remedy.contains("--show-origin") && remedy.contains("restore"),
                "{remedy}"
            );
            assert_eq!(std::fs::read(repo.join(".git/config")).unwrap(), before);
        }
        let worker = env.home().join("worker");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "factory/fixture",
                worker.to_str().unwrap(),
            ],
        );
        git(&repo, &["config", "extensions.worktreeConfig", "true"]);
        let private = worker.join("private-hooks");
        std::fs::create_dir(&private).unwrap();
        git(
            &worker,
            &["config", "--worktree", "core.hooksPath", "private-hooks"],
        );
        assert!(matches!(check(&worker).unwrap().status, CheckStatus::Ok));
        assert!(matches!(check(&repo).unwrap().status, CheckStatus::Warning));
        env.set_current_dir(&worker);
        let rows = checks(&repo);
        assert_eq!(
            rows.len(),
            2,
            "main store and active worktree must be diagnosed separately"
        );
        assert!(matches!(rows[0].status, CheckStatus::Warning));
        assert!(matches!(rows[1].status, CheckStatus::Ok));
        std::fs::remove_dir(&private).unwrap();
        let before = std::fs::read(repo.join(".git/worktrees/worker/config.worktree")).unwrap();
        let row = check(&worker).unwrap();
        assert!(matches!(row.status, CheckStatus::Warning));
        assert!(
            row.message.contains(private.to_str().unwrap()),
            "{}",
            row.message
        );
        assert_eq!(
            std::fs::read(repo.join(".git/worktrees/worker/config.worktree")).unwrap(),
            before
        );
    }
}
