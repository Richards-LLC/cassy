//! Shared Git evidence probes (cas-269ab, from the cas-c6ef review).
//!
//! Ref resolution, ancestry, merge-base, unmerged counts, tree blobs and the
//! bounded target fetch were private to the task close gate, so the Director
//! and worktree code reached into MCP lifecycle internals for them. They live
//! here now; `close_ops` re-exports them unchanged. Every probe that ran under
//! the epic-measurement deadline still does, through [`measurement`].

pub(crate) mod measurement;

use measurement::CommandExt as _;

/// cas-cf64 (P3, option-injection hardening): `true` when `name` is safe to
/// pass as a git ref/branch-name argument to a git subcommand.
///
/// None of the git subcommands this module shells out to
/// (`fetch`, `merge-base`, `rev-list`, `rev-parse --verify`) offer a safe
/// `--` end-of-options marker at every position a branch name is passed —
/// `git fetch <remote> <refspec>` in particular has none — so callers
/// validate at the source instead of trying to escape per call site. A
/// name starting with `-` would otherwise be parsed as a command-line
/// option (by git itself, or by ssh/git's transport helpers) rather than a
/// ref name.
pub(crate) fn is_safe_git_refname(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('-')
}

/// Return `true` if `refname` resolves to an existing commit object in the
/// git repository at `repo_path`.
///
/// `rev-parse --verify` alone accepts a syntactically valid full object ID even
/// when that object is absent. Every caller feeds the result to a commit-history
/// operation, so verify both object existence and commit shape with
/// `git cat-file -e <refname>^{commit}`.
pub(crate) fn git_ref_exists(repo_path: &std::path::Path, refname: &str) -> bool {
    use std::process::Command;
    Command::new("git")
        .args(["cat-file", "-e", "--", &format!("{refname}^{{commit}}")])
        .current_dir(repo_path)
        .measurement_output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Resolve `refname` to its full commit sha in the git repository at
/// `repo_path` (equivalent to `git rev-parse --verify <refname>`, returning
/// the trimmed stdout instead of just a boolean). Returns `None` on any
/// failure — used by the cas-4b3f factory-branch-anchor snapshot, where an
/// unresolvable ref simply means "nothing to anchor yet", not an error.
pub(crate) fn resolve_branch_sha(repo_path: &std::path::Path, refname: &str) -> Option<String> {
    use std::process::Command;
    let out = Command::new("git")
        .args(["rev-parse", "--verify", refname])
        .current_dir(repo_path)
        .measurement_output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

/// Immutable commit ID currently named by `reference`, or `None` when the ref
/// is unsafe, missing, or does not resolve to a commit.
///
/// Director merge-alert classification resolves all movable refs through this
/// helper once, then performs every merge-base/count operation against these
/// immutable IDs so concurrent ref updates cannot produce a mixed snapshot.
pub(crate) fn resolve_ref_commit_sha(
    repo_path: &std::path::Path,
    reference: &str,
) -> Option<String> {
    use std::process::Command;

    if !is_safe_git_refname(reference) {
        return None;
    }

    let commit = format!("{reference}^{{commit}}");
    let out = Command::new("git")
        .args(["rev-parse", "--verify", &commit])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

pub(crate) fn git_merge_base(
    repo_path: &std::path::Path,
    left: &str,
    right: &str,
) -> Option<String> {
    use std::process::Command;
    let out = Command::new("git")
        .args(["merge-base", left, right])
        .current_dir(repo_path)
        .measurement_output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(sha)
    } else {
        None
    }
}

pub(crate) fn git_commit_is_ancestor(
    repo_path: &std::path::Path,
    commit: &str,
    descendant: &str,
) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["merge-base", "--is-ancestor", commit, descendant])
        .measurement_status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Number of parents of `commit`, or 0 when git cannot answer.
pub(crate) fn git_commit_parent_count(repo_path: &std::path::Path, commit: &str) -> usize {
    let out = match std::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["rev-list", "--parents", "-n", "1", commit])
        .output()
    {
        Ok(out) if out.status.success() => out,
        _ => return 0,
    };
    // Output is "<commit> <parent1> <parent2> ..."
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .count()
        .saturating_sub(1)
}

/// The blob a commit's tree holds at `path`: `Some(None)` when the path is
/// absent, `None` when Git cannot answer (which never proves anything).
pub(crate) fn tree_path_blob(repo_path: &std::path::Path, commit: &str, path: &str) -> Option<Option<String>> {
    let output = std::process::Command::new("git")
        .args(["ls-tree", "-z", commit, "--", path])
        .current_dir(repo_path)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let listing = String::from_utf8(output.stdout).ok()?;
    let Some(entry) = listing.split('\0').find(|entry| !entry.is_empty()) else {
        return Some(None);
    };
    let (meta, listed) = entry.split_once('\t')?;
    let blob = meta.split_whitespace().nth(2)?;
    (listed == path).then(|| Some(blob.to_string()))
}

/// cas-38e2 / cas-cf64 (P3, bounded + validated): best-effort
/// `git fetch origin <parent_branch>` inside `repo_path`, refreshing the
/// `origin/<parent_branch>` remote-tracking ref before
/// [`run_factory_branch_merge_gate`] consults it as a fallback.
///
/// Deliberately fire-and-forget:
/// - No `origin` remote configured (common for local-only dev repos, or the
///   `epic/<slug>` local-only-branch convention) → the command fails fast
///   and is ignored; the caller falls back to whatever `origin/<parent_branch>`
///   already resolves to (nothing, if it never existed).
/// - Offline / unreachable remote → same graceful ignore.
/// - `GIT_TERMINAL_PROMPT=0` prevents a credential prompt from hanging the
///   close call indefinitely on a private remote with no cached credentials.
///
/// cas-cf64 (P3) hardening on top of the original cas-38e2 version:
/// - **Bounded**: this fires on the reject path on EVERY retry (there is no
///   fetch-once-per-park cache), so a worker looping `close` on a parked
///   task against a slow/blackholed `origin` would otherwise re-hang the
///   synchronous MCP handler each attempt. The child process is killed if
///   it hasn't finished within [`FETCH_TIMEOUT`], bounding worst-case
///   added latency per close attempt regardless of transport (SSH/HTTP/
///   filesystem) or how the remote fails.
/// - **Validated**: `parent_branch` is checked with [`is_safe_git_refname`]
///   before ever reaching the shell-out. `git fetch <remote> <refspec>` has
///   no safe `--` end-of-options marker, so a `parent_branch` value
///   starting with `-` would otherwise be parsed as a git option instead of
///   a ref name.
pub(crate) const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub(crate) fn fetch_parent_branch_best_effort(
    repo_path: &std::path::Path,
    parent_branch: &str,
) -> bool {
    if !is_safe_git_refname(parent_branch) {
        return false;
    }

    // Force-update the exact remote-tracking ref. The leading `+` is
    // intentional: a remote epic may have been rebased/force-pushed, and the
    // caller needs the authoritative remote state rather than a fetch rejected
    // as non-fast-forward.
    let refspec = format!("+refs/heads/{parent_branch}:refs/remotes/origin/{parent_branch}");
    // Finished (success or failure) or timed out — the caller doesn't care.
    !matches!(
        run_bounded_git_fetch(repo_path, &["fetch", "--quiet", "origin", &refspec]),
        BoundedFetch::NotStarted(_)
    )
}

/// How one bounded `git fetch` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BoundedFetch {
    Succeeded,
    Failed,
    TimedOut,
    NotStarted(String),
}

/// Run one `git fetch` with no prompt, no output and a hard deadline of
/// [`FETCH_TIMEOUT`]; the shared runner for every close-time fetch.
pub(crate) fn run_bounded_git_fetch(repo_path: &std::path::Path, args: &[&str]) -> BoundedFetch {
    use std::process::{Command, Stdio};
    use std::time::Instant;

    if let Some(deadline) = measurement::deadline() {
        let mut command = Command::new("git");
        command
            .args(args)
            .current_dir(repo_path)
            .env("GIT_TERMINAL_PROMPT", "0");
        return match crate::bounded_process::run_command(&mut command, deadline, FETCH_TIMEOUT) {
            Ok(output) if output.status.success() => BoundedFetch::Succeeded,
            Ok(_) => BoundedFetch::Failed,
            Err(crate::bounded_process::BoundedCommandError::TimedOut) => BoundedFetch::TimedOut,
            Err(crate::bounded_process::BoundedCommandError::Io) => {
                BoundedFetch::NotStarted("Git probe failed".into())
            }
        };
    }

    let mut child = match Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return BoundedFetch::NotStarted(error.to_string()),
    };

    let deadline = Instant::now() + FETCH_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return BoundedFetch::Succeeded,
            Ok(Some(_)) | Err(_) => return BoundedFetch::Failed,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return BoundedFetch::TimedOut;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}

/// Success-bearing unmerged-count for close-integrity **acceptance** paths
/// (cas-2938 live-ref convergence / cas-5485 pre-rebase SHA refresh).
///
/// Unlike [`count_unmerged_factory_commits`], which deliberately fail-opens
/// to `0` when Git history is unknowable (legacy Proceed-friendly posture
/// for the primary ancestry path), this tri-state never maps unknown state
/// to zero. Callers that authorize close must match on [`KnownZero`] only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KnownUnmergedCount {
    /// Refs resolved, merge-base computed, rev-list succeeded with count 0.
    KnownZero,
    /// Refs resolved and rev-list reported a positive stranded count.
    KnownPositive(u32),
    /// Missing ref, failed merge-base, failed/unparseable rev-list, or
    /// unsafe refname — Git state is not evidence of convergence.
    Unknown,
}

/// Explicit success-bearing counterpart to [`count_unmerged_factory_commits`].
///
/// Returns:
/// - [`KnownUnmergedCount::KnownZero`] only when both refs resolve, merge-base
///   succeeds, and `rev-list --count` parses as `0`.
/// - [`KnownUnmergedCount::KnownPositive`] when the count is a known `> 0`
///   (or the cas-cf64 unsafe-refname fail-closed `u32::MAX` case).
/// - [`KnownUnmergedCount::Unknown`] on any resolution/computation failure —
///   never treats "couldn't tell" as "zero ahead".
pub(crate) fn known_unmerged_factory_commits(
    repo_path: &std::path::Path,
    factory_branch: &str,
    parent_branch: &str,
) -> KnownUnmergedCount {
    use std::process::Command;

    if !is_safe_git_refname(factory_branch) || !is_safe_git_refname(parent_branch) {
        // Corrupted/injection input: not KnownZero. Surface as positive so
        // any caller that only checks `== KnownZero` still refuses, and
        // callers that inspect magnitude still see "stranded".
        return KnownUnmergedCount::KnownPositive(u32::MAX);
    }

    // Both tips must resolve — missing factory or parent is Unknown, not zero.
    if !git_ref_exists(repo_path, factory_branch) || !git_ref_exists(repo_path, parent_branch) {
        return KnownUnmergedCount::Unknown;
    }

    let merge_base_out = Command::new("git")
        .args(["merge-base", parent_branch, factory_branch])
        .current_dir(repo_path)
        .measurement_output();
    let merge_base = match merge_base_out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                return KnownUnmergedCount::Unknown;
            }
            s
        }
        _ => return KnownUnmergedCount::Unknown,
    };

    let count_out = Command::new("git")
        .args([
            "rev-list",
            "--count",
            &format!("{merge_base}..{factory_branch}"),
        ])
        .current_dir(repo_path)
        .measurement_output();
    match count_out {
        Ok(o) if o.status.success() => {
            match String::from_utf8_lossy(&o.stdout).trim().parse::<u32>() {
                Ok(0) => KnownUnmergedCount::KnownZero,
                Ok(n) => KnownUnmergedCount::KnownPositive(n),
                // Unparseable count is not evidence of zero.
                Err(_) => KnownUnmergedCount::Unknown,
            }
        }
        _ => KnownUnmergedCount::Unknown,
    }
}

/// Trimmed stdout of `git <args>` in `repo`, or `None` when Git fails.
pub(crate) fn git_text(repo: &std::path::Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}
