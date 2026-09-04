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
/// `anchor_commits` binds the delivered commit identity this cycle is about.
/// cas-5c33: without anchors the proof is pinned to the branch tip, so a
/// worker that fast-forwards to the epic tip for its next task invalidates the
/// verdict on work that is already merged and unchanged. Pass an empty list to
/// keep the strict whole-boundary contract.
pub(crate) fn capture_repository_proof_with_anchors(
    repository_root: &Path,
    worktree_root: &Path,
    anchor_commits: Vec<String>,
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
        anchor_commits,
    })
}

/// Commits whose content this close is really delivering.
///
/// The resolved commit receipt always counts. The worktree tip counts only
/// when it carries work beyond the integration base — a close with nothing
/// delivered yet returns an empty list and keeps the strict whole-boundary
/// contract, which is what pins "the verifier looked at exactly this tree".
pub(crate) fn delivered_anchor_commits(
    worktree_root: &Path,
    parent_branch: Option<&str>,
    commit_receipt: Option<&str>,
) -> Vec<String> {
    let mut anchors: Vec<String> = Vec::new();
    if let Some(receipt) = commit_receipt
        && let Ok(sha) = rev_parse(worktree_root, receipt)
    {
        anchors.push(sha);
    }
    if let Ok(head) = rev_parse(worktree_root, "HEAD")
        && head_is_ahead_of_integration_base(worktree_root, parent_branch, &head)
        && !anchors.iter().any(|anchor| anchor == &head)
    {
        anchors.push(head);
    }
    anchors
}

fn rev_parse(worktree_root: &Path, revision: &str) -> Result<String, String> {
    let output = git_output(worktree_root, &["rev-parse", "--verify", revision])?;
    Ok(String::from_utf8_lossy(&output).trim().to_string())
}

/// True when HEAD carries commits the integration branch does not already
/// have. Without a resolvable parent branch we answer false: an unknown
/// integration point must not silently widen the proof's tolerance.
fn head_is_ahead_of_integration_base(
    worktree_root: &Path,
    parent_branch: Option<&str>,
    head: &str,
) -> bool {
    let Some(parent_branch) = parent_branch.map(str::trim).filter(|name| !name.is_empty()) else {
        return false;
    };
    // Mirror close_ops' refname guard: a leading dash would be parsed as a
    // git option, so an unsafe name fails closed (no anchor, strict proof).
    if parent_branch.starts_with('-') {
        return false;
    }
    let Ok(base) = git_output(worktree_root, &["merge-base", parent_branch, head]) else {
        return false;
    };
    let base = String::from_utf8_lossy(&base).trim().to_string();
    !base.is_empty() && base != head
}

fn commit_exists(worktree_root: &Path, commit: &str) -> bool {
    git_output(worktree_root, &["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_ok()
}

fn commit_is_reachable_from(worktree_root: &Path, commit: &str, head: &str) -> bool {
    commit == head
        || git_output(
            worktree_root,
            &["merge-base", "--is-ancestor", commit, head],
        )
        .is_ok()
}

/// What a bound proof looks like against the repository right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RepositoryProofStatus {
    /// The boundary is byte-for-byte what the verifier was handed.
    Unchanged,
    /// Every delivered anchor is still reachable, but the working tree moved
    /// (new commits for the next task, a merge, a fast-forward, or local WIP).
    /// Reported, not fatal — the delivered content is pinned by commit id.
    DeliveredContentIntact {
        bound_head: String,
        current_head: String,
        bound_digest: String,
        current_digest: String,
    },
}

impl RepositoryProofStatus {
    /// One line naming both digests, for the verdict record (cas-5c33).
    pub(crate) fn drift_note(&self) -> Option<String> {
        match self {
            Self::Unchanged => None,
            Self::DeliveredContentIntact {
                bound_head,
                current_head,
                bound_digest,
                current_digest,
            } => Some(format!(
                "Repository moved after dispatch but every delivered commit is still reachable: \
                 bound head {bound_head} digest {bound_digest}; current head {current_head} \
                 digest {current_digest}."
            )),
        }
    }
}

/// Evaluate a bound proof against the repository as it stands now.
pub(crate) fn evaluate_repository_proof(
    proof: &RepositoryProofBoundary,
) -> Result<RepositoryProofStatus, TaskLifecycleGateError> {
    let worktree_root = PathBuf::from(&proof.worktree_root);
    let current = capture_repository_proof_with_anchors(
        Path::new(&proof.repository_root),
        &worktree_root,
        proof.anchor_commits.clone(),
    )
    .map_err(|message| TaskLifecycleGateError::RepositoryProof { message })?;
    if &current == proof {
        return Ok(RepositoryProofStatus::Unchanged);
    }

    // Nothing delivered was bound, so the whole boundary is the proof and any
    // change ends this cycle — the pre-cas-5c33 contract, unchanged.
    if proof.anchor_commits.is_empty() {
        return Err(TaskLifecycleGateError::RepositoryProof {
            message: format!(
                "repository proof changed after dispatch; request a fresh verification cycle \
                 (dispatch bound head {bound}, current head {current_head})",
                bound = proof.head_commit,
                current_head = current.head_commit,
            ),
        });
    }

    let missing: Vec<&String> = proof
        .anchor_commits
        .iter()
        .filter(|anchor| {
            !commit_exists(&worktree_root, anchor)
                || !commit_is_reachable_from(&worktree_root, anchor, &current.head_commit)
        })
        .collect();
    if !missing.is_empty() {
        let names = missing
            .iter()
            .map(|anchor| anchor.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(TaskLifecycleGateError::RepositoryProof {
            message: format!(
                "repository proof changed after dispatch; request a fresh verification cycle \
                 (dispatch bound head {bound}, current head {current_head}, delivered commit(s) \
                 no longer reachable: {names})",
                bound = proof.head_commit,
                current_head = current.head_commit,
            ),
        });
    }

    Ok(RepositoryProofStatus::DeliveredContentIntact {
        bound_head: proof.head_commit.clone(),
        current_head: current.head_commit,
        bound_digest: proof.state_digest.clone(),
        current_digest: current.state_digest,
    })
}

/// Boolean form for call sites that only need "may this cycle continue".
pub(crate) fn verify_repository_proof(
    proof: &RepositoryProofBoundary,
) -> Result<(), TaskLifecycleGateError> {
    evaluate_repository_proof(proof).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(path: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "CAS Test")
            .env("GIT_AUTHOR_EMAIL", "cas@example.test")
            .env("GIT_COMMITTER_NAME", "CAS Test")
            .env("GIT_COMMITTER_EMAIL", "cas@example.test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// A repo on `main` with one seed commit, plus a `factory/worker` branch
    /// carrying one delivered commit checked out in the working tree.
    fn delivered_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp repo");
        let path = dir.path();
        git(path, &["init", "-q", "-b", "main"]);
        std::fs::write(path.join("seed.txt"), "seed\n").unwrap();
        git(path, &["add", "seed.txt"]);
        git(path, &["commit", "-q", "-m", "seed"]);
        git(path, &["checkout", "-q", "-b", "factory/worker"]);
        std::fs::write(path.join("delivered.txt"), "delivered\n").unwrap();
        git(path, &["add", "delivered.txt"]);
        git(path, &["commit", "-q", "-m", "deliver the work"]);
        dir
    }

    fn head(path: &Path) -> String {
        rev_parse(path, "HEAD").expect("HEAD")
    }

    #[test]
    fn anchors_name_the_delivered_tip_only_when_it_is_ahead_of_the_base() {
        let repo = delivered_repo();
        let path = repo.path();

        let anchors = delivered_anchor_commits(path, Some("main"), None);
        assert_eq!(
            anchors,
            vec![head(path)],
            "a branch with work beyond main delivers its tip"
        );

        git(path, &["checkout", "-q", "main"]);
        assert!(
            delivered_anchor_commits(path, Some("main"), None).is_empty(),
            "nothing delivered beyond the integration base means no anchor"
        );
        assert!(
            delivered_anchor_commits(path, None, None).is_empty(),
            "an unresolvable integration branch must not widen tolerance"
        );
    }

    #[test]
    fn commit_receipt_is_bound_as_an_anchor() {
        let repo = delivered_repo();
        let path = repo.path();
        let delivered = head(path);
        git(path, &["checkout", "-q", "main"]);

        let anchors = delivered_anchor_commits(path, Some("main"), Some(&delivered));
        assert_eq!(anchors, vec![delivered]);
    }

    /// The wedge from GH cas-5c33: the worker fast-forwards its branch to the
    /// integration tip to start the next task. The delivered commit is still
    /// there, so the verdict must survive.
    #[test]
    fn branch_advance_keeps_the_proof_when_delivered_commits_stay_reachable() {
        let repo = delivered_repo();
        let path = repo.path();
        let anchors = delivered_anchor_commits(path, Some("main"), None);
        let proof = capture_repository_proof_with_anchors(path, path, anchors).expect("capture");

        // The supervisor merges the work, the worker moves on: new commits on
        // top, none of which touch the delivered commit.
        std::fs::write(path.join("next-task.txt"), "next\n").unwrap();
        git(path, &["add", "next-task.txt"]);
        git(path, &["commit", "-q", "-m", "start the next task"]);

        let status = evaluate_repository_proof(&proof).expect("delivered content is intact");
        let RepositoryProofStatus::DeliveredContentIntact {
            bound_head,
            current_head,
            bound_digest,
            current_digest,
        } = &status
        else {
            panic!("expected a reported move, got {status:?}");
        };
        assert_eq!(bound_head, &proof.head_commit);
        assert_ne!(bound_head, current_head, "the tip really did move");
        assert_ne!(bound_digest, current_digest);
        let note = status.drift_note().expect("a moved tree must be reported");
        assert!(note.contains(bound_digest), "{note}");
        assert!(note.contains(current_digest), "{note}");
    }

    /// Uncommitted work for the next task is reported, not fatal, once
    /// delivered commits are bound (supervisor ruling on cas-5c33).
    #[test]
    fn uncommitted_work_is_reported_when_anchors_are_bound() {
        let repo = delivered_repo();
        let path = repo.path();
        let anchors = delivered_anchor_commits(path, Some("main"), None);
        let proof = capture_repository_proof_with_anchors(path, path, anchors).expect("capture");

        std::fs::write(path.join("scratch.txt"), "wip for the next task\n").unwrap();

        let status = evaluate_repository_proof(&proof).expect("uncommitted WIP is not fatal");
        assert!(matches!(
            status,
            RepositoryProofStatus::DeliveredContentIntact { .. }
        ));
        assert!(status.drift_note().is_some());
    }

    /// Rewriting or dropping the delivered commit is the case the gate exists
    /// for, and it must still fail — naming both tips and the lost commit.
    #[test]
    fn rewritten_delivery_is_rejected_and_names_both_tips() {
        let repo = delivered_repo();
        let path = repo.path();
        let delivered = head(path);
        let anchors = delivered_anchor_commits(path, Some("main"), None);
        let proof = capture_repository_proof_with_anchors(path, path, anchors).expect("capture");

        git(path, &["reset", "-q", "--hard", "main"]);
        std::fs::write(path.join("replacement.txt"), "different work\n").unwrap();
        git(path, &["add", "replacement.txt"]);
        git(path, &["commit", "-q", "-m", "rewritten delivery"]);

        let error = evaluate_repository_proof(&proof).expect_err("a rewritten delivery must fail");
        let TaskLifecycleGateError::RepositoryProof { message } = error else {
            panic!("expected a repository-proof refusal");
        };
        assert!(message.contains(&proof.head_commit), "bound tip: {message}");
        assert!(message.contains(&head(path)), "current tip: {message}");
        assert!(message.contains(&delivered), "lost delivery: {message}");
    }

    /// With nothing delivered, the pre-cas-5c33 contract is unchanged: any
    /// change to the reviewed tree ends the cycle.
    #[test]
    fn without_anchors_any_change_still_ends_the_cycle() {
        let repo = delivered_repo();
        let path = repo.path();
        git(path, &["checkout", "-q", "main"]);
        let anchors = delivered_anchor_commits(path, Some("main"), None);
        assert!(anchors.is_empty());
        let proof = capture_repository_proof_with_anchors(path, path, anchors).expect("capture");

        assert_eq!(
            evaluate_repository_proof(&proof).expect("unchanged tree"),
            RepositoryProofStatus::Unchanged
        );

        std::fs::write(path.join("seed.txt"), "mutated during review\n").unwrap();
        let error = evaluate_repository_proof(&proof).expect_err("strict mode still rejects");
        let TaskLifecycleGateError::RepositoryProof { message } = error else {
            panic!("expected a repository-proof refusal");
        };
        assert!(message.contains("request a fresh verification cycle"), "{message}");
        assert!(message.contains(&proof.head_commit), "{message}");
    }

    #[test]
    fn legacy_rows_without_anchors_deserialize_into_strict_mode() {
        let legacy = serde_json::json!({
            "repository_root": "/repo",
            "worktree_root": "/repo",
            "head_commit": "abc123",
            "state_digest": "digest",
        });
        let proof: RepositoryProofBoundary =
            serde_json::from_value(legacy).expect("pre-cas-5c33 dispatch rows still parse");
        assert!(proof.anchor_commits.is_empty());
    }
}
