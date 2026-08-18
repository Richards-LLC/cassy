use cas_types::RepositoryProofBoundary;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::TaskLifecycleGateError;

fn git_output(worktree: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(args)
        .output()
        .map_err(|error| format!("repository proof could not run git: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "repository proof git command failed: {}",
            stderr.trim()
        ));
    }
    Ok(output.stdout)
}

fn canonical(path: &Path, label: &str) -> Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|error| format!("repository proof cannot resolve {label}: {error}"))
}

pub(crate) fn is_git_worktree(path: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .is_ok_and(|output| output.status.success() && output.stdout.starts_with(b"true"))
}

/// Capture the exact Git/worktree state a legacy task verifier is asked to inspect.
///
/// Cassy metadata is excluded because verifier lifecycle writes update `.cas/cas.db`
/// themselves. Tracked changes (staged and unstaged) and untracked file contents
/// are all included, so any operator-authored repository mutation changes the proof.
pub(crate) fn capture_repository_proof(
    repository_root: &Path,
    worktree_root: &Path,
) -> Result<RepositoryProofBoundary, String> {
    let repository_root = canonical(repository_root, "repository root")?;
    let worktree_root = canonical(worktree_root, "worktree root")?;
    let actual_worktree = git_output(&worktree_root, &["rev-parse", "--show-toplevel"])?;
    let actual_worktree = PathBuf::from(
        String::from_utf8(actual_worktree)
            .map_err(|_| "repository proof worktree path is not UTF-8".to_string())?
            .trim(),
    );
    if canonical(&actual_worktree, "Git worktree")? != worktree_root {
        return Err("repository proof path is not the Git worktree root".to_string());
    }

    let head = git_output(&worktree_root, &["rev-parse", "--verify", "HEAD"])?;
    let head_commit = String::from_utf8(head)
        .map_err(|_| "repository proof HEAD is not UTF-8".to_string())?
        .trim()
        .to_string();
    let tracked = git_output(
        &worktree_root,
        &[
            "diff",
            "--no-ext-diff",
            "--binary",
            "--full-index",
            "HEAD",
            "--",
            ".",
            ":(exclude).cas",
            ":(exclude).cas/**",
        ],
    )?;
    let untracked = git_output(
        &worktree_root,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            ".",
            ":(exclude).cas",
            ":(exclude).cas/**",
        ],
    )?;

    let mut hasher = Sha256::new();
    hasher.update(b"cas-verification-repository-proof-v1\0");
    hasher.update(head_commit.as_bytes());
    hasher.update(b"\0tracked\0");
    hasher.update(&tracked);
    hasher.update(b"\0untracked\0");
    for raw_path in untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let relative = std::str::from_utf8(raw_path)
            .map_err(|_| "repository proof cannot bind a non-UTF-8 untracked path".to_string())?;
        let path = worktree_root.join(relative);
        hasher.update((raw_path.len() as u64).to_le_bytes());
        hasher.update(raw_path);
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            format!("repository proof cannot inspect untracked file {relative}: {error}")
        })?;
        if metadata.file_type().is_symlink() {
            hasher.update(b"symlink\0");
            let target = std::fs::read_link(&path).map_err(|error| {
                format!("repository proof cannot read symlink {relative}: {error}")
            })?;
            hasher.update(target.to_string_lossy().as_bytes());
        } else if metadata.is_file() {
            hasher.update(b"file\0");
            let contents = std::fs::read(&path).map_err(|error| {
                format!("repository proof cannot read untracked file {relative}: {error}")
            })?;
            hasher.update((contents.len() as u64).to_le_bytes());
            hasher.update(contents);
        } else {
            return Err(format!(
                "repository proof does not support untracked special file {relative}"
            ));
        }
    }

    Ok(RepositoryProofBoundary {
        repository_root: repository_root.to_string_lossy().into_owned(),
        worktree_root: worktree_root.to_string_lossy().into_owned(),
        head_commit,
        state_digest: format!("{:x}", hasher.finalize()),
    })
}

pub(crate) fn verify_repository_proof(
    proof: &RepositoryProofBoundary,
) -> Result<(), TaskLifecycleGateError> {
    let current = capture_repository_proof(
        Path::new(&proof.repository_root),
        Path::new(&proof.worktree_root),
    )
    .map_err(|message| TaskLifecycleGateError::RepositoryProof { message })?;
    if &current != proof {
        return Err(TaskLifecycleGateError::RepositoryProof {
            message: "repository proof changed after dispatch; request a fresh verification cycle"
                .to_string(),
        });
    }
    Ok(())
}
