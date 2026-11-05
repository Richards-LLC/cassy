//! Preserve a worker's unpushed or uncommitted work before a teardown
//! (cas-2879).
//!
//! QA cas-881f: a forced shutdown and the later worktree removal deleted a
//! worker branch whose commit was on no remote, and the next session found
//! only stranded objects. Every teardown that can lose work first writes the
//! work to `refs/cas/rescue/<worker>/<timestamp>-<sha>` and pushes that ref to
//! `origin` when one exists. The worktree, its index and its branch are never
//! modified here.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::worktree::git::{GitError, GitOperations, Result};

const RESCUE_PUSH_TIMEOUT: Duration = Duration::from_secs(30);

/// Work written to a rescue ref before a teardown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RescuedWork {
    pub refname: String,
    /// Commit the rescue ref points at. With dirty files it is a commit on
    /// top of HEAD holding them; otherwise HEAD itself.
    pub sha: String,
    pub unpushed_commits: usize,
    pub dirty_files: usize,
    /// `Ok(true)` pushed to origin, `Ok(false)` no origin remote,
    /// `Err` push failed and the ref is local only.
    pub pushed: std::result::Result<bool, String>,
}

impl RescuedWork {
    /// One line naming what was kept, where, and how to get it back.
    pub fn describe(&self) -> String {
        let place = match &self.pushed {
            Ok(true) => "pushed to origin".to_string(),
            Ok(false) => "local only: no origin remote".to_string(),
            Err(error) => format!("local only: push to origin failed ({error})"),
        };
        format!(
            "preserved {} unpushed commit(s) and {} uncommitted file(s) at {} ({}, {place}); \
             restore with `git switch -c <branch> {}`",
            self.unpushed_commits,
            self.dirty_files,
            self.refname,
            &self.sha[..self.sha.len().min(8)],
            self.refname,
        )
    }
}

fn git_in(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .envs(envs.iter().copied())
        .current_dir(dir)
        .output()?;
    if !output.status.success() {
        return Err(GitError::CommandFailed(format!(
            "git {}: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn ref_component(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "worker".into()
    } else {
        cleaned
    }
}

impl GitOperations {
    /// Preserve `worker`'s work at `rev` before a teardown. `rev` counts as
    /// unpushed when it has commits on no remote ref; with `deleting_branch`,
    /// a commit on another local branch also counts as safe, since only that
    /// branch is going away. With `worktree`, its uncommitted files are
    /// captured too, through a temporary index. Returns `Ok(None)` when
    /// nothing would be lost.
    pub fn rescue_unpushed_work(
        &self,
        worker: &str,
        deleting_branch: Option<&str>,
        rev: &str,
        worktree: Option<&Path>,
    ) -> Result<Option<RescuedWork>> {
        let dir = worktree.unwrap_or(&self.repo_root);
        let Ok(head) = git_in(
            dir,
            &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
            &[],
        ) else {
            return Ok(None);
        };
        let mut count = vec![
            "rev-list".to_string(),
            "--count".into(),
            head.clone(),
            "--not".into(),
        ];
        if let Some(branch) = deleting_branch {
            count.extend([format!("--exclude={branch}"), "--branches".into()]);
        }
        count.push("--remotes".into());
        let count: Vec<&str> = count.iter().map(String::as_str).collect();
        let unpushed: usize = git_in(dir, &count, &[])?
            .parse()
            .map_err(|error| GitError::CommandFailed(format!("unpushed count: {error}")))?;
        let dirty_files = match worktree {
            Some(path) => self.uncommitted_file_count(path)?,
            None => 0,
        };
        if unpushed == 0 && dirty_files == 0 {
            return Ok(None);
        }

        let sha = if dirty_files == 0 {
            head.clone()
        } else {
            snapshot_dirty_tree(dir, &head, worker)?
        };
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
        let refname = format!(
            "refs/cas/rescue/{}/{stamp}-{}",
            ref_component(worker),
            &sha[..sha.len().min(8)]
        );
        git_in(dir, &["update-ref", &refname, &sha], &[])?;

        let pushed = if !self.has_origin_remote() {
            Ok(false)
        } else {
            let mut push = Command::new("git");
            push.args(["push", "origin", &format!("{refname}:{refname}")])
                .current_dir(dir);
            match Self::run_command_bounded(push, RESCUE_PUSH_TIMEOUT) {
                Ok(output) if output.status.success() => Ok(true),
                Ok(output) => Err(super::branch_ops::first_line(&String::from_utf8_lossy(
                    &output.stderr,
                ))),
                Err(error) => Err(error.to_string()),
            }
        };
        Ok(Some(RescuedWork {
            refname,
            sha,
            unpushed_commits: unpushed,
            dirty_files,
            pushed,
        }))
    }
}

/// Commit the worktree's files on top of `head` without touching its index.
fn snapshot_dirty_tree(dir: &Path, head: &str, worker: &str) -> Result<String> {
    let index = git_in(
        dir,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "cas-rescue-index",
        ],
        &[],
    )?;
    let env = [("GIT_INDEX_FILE", index.as_str())];
    let result = (|| {
        git_in(dir, &["read-tree", head], &env)?;
        git_in(dir, &["add", "-A", "."], &env)?;
        let tree = git_in(dir, &["write-tree"], &env)?;
        let message = format!("cas rescue: uncommitted files of {worker}");
        git_in(
            dir,
            &["commit-tree", &tree, "-p", head, "-m", &message],
            &[
                ("GIT_AUTHOR_NAME", "Cassy rescue"),
                ("GIT_AUTHOR_EMAIL", "rescue@cassy.invalid"),
                ("GIT_COMMITTER_NAME", "Cassy rescue"),
                ("GIT_COMMITTER_EMAIL", "rescue@cassy.invalid"),
            ],
        )
    })();
    let _ = std::fs::remove_file(&index);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        git_in(dir, args, &[]).unwrap_or_else(|error| panic!("git {args:?}: {error}"))
    }

    /// cas-2879: an unpushed commit plus a dirty file are kept on a pushed
    /// rescue ref, the worktree is left exactly as it was, and pushed clean
    /// work produces no rescue at all.
    #[test]
    fn rescue_preserves_unpushed_commit_and_dirty_file_without_touching_worktree() {
        let origin = tempfile::TempDir::new().unwrap();
        git(origin.path(), &["init", "--bare", "-b", "main"]);
        let repo = tempfile::TempDir::new().unwrap();
        let dir = repo.path();
        git(dir, &["init", "-b", "main"]);
        git(dir, &["config", "user.email", "t@cas"]);
        git(dir, &["config", "user.name", "T"]);
        git(
            dir,
            &["remote", "add", "origin", origin.path().to_str().unwrap()],
        );
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-m", "base"]);
        git(dir, &["switch", "-c", "factory/w"]);
        git(dir, &["push", "origin", "main", "factory/w"]);
        let ops = GitOperations::new(dir.to_path_buf());

        assert_eq!(
            ops.rescue_unpushed_work("w", None, "HEAD", Some(dir))
                .unwrap(),
            None
        );

        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-m", "unpushed"]);
        std::fs::write(dir.join("a.txt"), "dirty\n").unwrap();
        std::fs::write(dir.join("new.txt"), "untracked\n").unwrap();
        let status_before = git(dir, &["status", "--porcelain"]);
        let head = git(dir, &["rev-parse", "HEAD"]);

        let rescued = ops
            .rescue_unpushed_work("w", None, "HEAD", Some(dir))
            .unwrap()
            .expect("unpushed and dirty work is rescued");
        assert_eq!((rescued.unpushed_commits, rescued.dirty_files), (1, 2));
        assert_eq!(rescued.pushed, Ok(true));
        assert!(
            rescued.refname.starts_with("refs/cas/rescue/w/"),
            "{}",
            rescued.refname
        );
        assert_eq!(git(dir, &["status", "--porcelain"]), status_before);
        assert_eq!(git(dir, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            git(origin.path(), &["rev-parse", &rescued.refname]),
            rescued.sha
        );
        assert_eq!(
            git(dir, &["show", &format!("{}:a.txt", rescued.sha)]),
            "dirty"
        );
        assert_eq!(
            git(dir, &["show", &format!("{}:new.txt", rescued.sha)]),
            "untracked"
        );
        assert_eq!(git(dir, &["rev-parse", &format!("{}^", rescued.sha)]), head);
        assert!(
            rescued.describe().contains("pushed to origin"),
            "{}",
            rescued.describe()
        );
    }
}
