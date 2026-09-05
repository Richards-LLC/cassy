use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use crate::worktree::git::{GitError, GitOperations, ResolvedBase, Result};

/// Wall-clock bound for `fetch_branch` (cas-0938). Protects a synchronous
/// supervisor/UI action (`create_epic_branch` calls `resolve_fresh_base` calls
/// `fetch_branch`) from blocking indefinitely on a configured-but-unreachable
/// remote (VPN down, dead SSH host) — those hang rather than fail fast, and
/// git has no single config knob that bounds connect time across both
/// http(s) and ssh transports, so this is enforced at the process level.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Wall-clock bound for [`GitOperations::publish_branch_to_origin`] (cas-5ee0).
/// Same reasoning as [`FETCH_TIMEOUT`]: a synchronous MCP handler must never
/// hang on a configured-but-unreachable remote. Slightly larger than the fetch
/// bound because a push uploads objects.
const PUSH_TIMEOUT: Duration = Duration::from_secs(30);

/// What happened when a merge target ref was published to its remote
/// (cas-5ee0 / GH #137).
///
/// The merge receipt and the task-close merge-state guard must agree on which
/// ref is authoritative. The guard consults BOTH `<parent>` and
/// `origin/<parent>`, so a merge that only moved the local ref is *invisible*
/// to any checkout that reads origin — which is what produced a guaranteed
/// close-rejection loop. This type is the honest answer to "did the merge
/// actually become visible to everyone else?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetPushOutcome {
    /// No `origin` remote is configured. Nothing to publish, and nothing
    /// downstream can consult a remote ref either — the local ref *is*
    /// authoritative. Not a warning.
    NoRemote,
    /// An `origin` remote exists but `origin/<branch>` does not — the target
    /// branch was never published (a local-only epic branch, for instance).
    /// Nothing downstream consults a ref that does not exist: the close
    /// merge-state guard skips its `origin/<parent>` check entirely when the
    /// ref is absent, so the local ref is authoritative and creating a remote
    /// branch as a side effect of a merge is not this tool's call.
    RemoteBranchAbsent,
    /// `origin/<branch>` already resolves to the local tip (typically a
    /// resumed/reconciled merge that was published on an earlier attempt).
    AlreadyCurrent { sha: String },
    /// The push ran and succeeded; `origin/<branch>` now carries the tip.
    Pushed { sha: String },
    /// GitHub rejected a push to the repository's default branch because an
    /// active repository ruleset requires the change to land through a pull
    /// request. This is distinct from an ordinary push failure: retrying the
    /// same `git push` cannot succeed, and the caller must preserve the source
    /// branch for the PR route.
    ProtectedDefaultBranch {
        /// Local tip that the rejected push attempted to publish.
        sha: String,
        /// Remote default-branch tip observed before the rejected push.
        remote_sha: Option<String>,
        /// GitHub's GH013 diagnostic, retained as evidence.
        reason: String,
    },
    /// The push was rejected because `origin/<branch>` has commits the local
    /// branch does not: the remote moved under this merge.
    ///
    /// Distinct from `NotPushed` because the remedy is different in kind —
    /// repeating the push cannot work, and the operator must reconcile with
    /// the remote first. Reporting this as a generic failure is what produced
    /// the "origin is BEHIND / just push again" advice in GH #703, which was
    /// the exact inverse of the real state.
    NonFastForward {
        /// Local tip that the rejected push attempted to publish.
        sha: String,
        /// Remote tip that the local branch does not contain.
        remote_sha: Option<String>,
        /// git's own rejection text, retained as evidence.
        reason: String,
    },
    /// The merge is LOCAL ONLY. `reason` says why the push did not land.
    NotPushed {
        /// Local tip of the target branch, or `None` if it did not resolve.
        sha: Option<String>,
        /// Remote tip as last observed, or `None` if `origin/<branch>` does
        /// not exist locally.
        remote_sha: Option<String>,
        reason: String,
    },
}

impl TargetPushOutcome {
    /// True when the target ref is known to be visible on its remote (or
    /// there is no remote for it to be visible on).
    pub fn is_published(&self) -> bool {
        matches!(
            self,
            Self::NoRemote
                | Self::RemoteBranchAbsent
                | Self::AlreadyCurrent { .. }
                | Self::Pushed { .. }
        )
    }
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no diagnostic output")
        .to_string()
}

pub(crate) fn classify_push_rejection(
    branch: &str,
    default_branch: &str,
    local_sha: String,
    remote_sha: Option<String>,
    stderr: &str,
) -> TargetPushOutcome {
    let reason = stderr.trim();
    if branch == default_branch
        && reason.contains("GH013")
        && reason.contains("Repository rule violations")
    {
        return TargetPushOutcome::ProtectedDefaultBranch {
            sha: local_sha,
            remote_sha,
            reason: reason.to_string(),
        };
    }

    // git names this rejection in several equivalent ways depending on version
    // and whether an upstream is configured. Match on its stable markers.
    let lowered = reason.to_ascii_lowercase();
    let non_fast_forward = lowered.contains("non-fast-forward")
        || lowered.contains("fetch first")
        || lowered.contains("the remote contains work that you do")
        || (lowered.contains("[rejected]") && lowered.contains("failed to push"));

    let reason = reason
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("git push failed with no diagnostic output")
        .to_string();
    if non_fast_forward {
        return TargetPushOutcome::NonFastForward {
            sha: local_sha,
            remote_sha,
            reason,
        };
    }
    TargetPushOutcome::NotPushed {
        sha: Some(local_sha),
        remote_sha,
        reason,
    }
}

/// What reconciling the local target with `origin/<target>` did, or could not do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetReconcile {
    /// No `origin` remote, or no `origin/<target>` to reconcile against.
    NoRemote,
    /// Local and remote already name the same commit.
    AlreadyCurrent { sha: String },
    /// The local target was strictly behind and has been advanced.
    FastForwarded {
        from: String,
        to: String,
        commits_gained: u32,
    },
    /// Both sides hold commits the other lacks. The caller must refuse BEFORE
    /// merging: a refusal afterwards would leave a mutated local target for
    /// the operator to unpick.
    Diverged {
        local: String,
        remote: String,
        ahead: u32,
        behind: u32,
    },
    /// The local target holds commits origin does not, and origin holds none
    /// the local target lacks. This is NOT divergence: origin has not moved,
    /// so a later push fast-forwards. There is nothing to reconcile and no
    /// merge to refuse — the caller states the unpublished commits and
    /// proceeds. Kept separate from `Diverged` precisely because the
    /// divergence recovery recipe is a no-op here (cas-26c7).
    AheadOfRemote {
        local: String,
        remote: String,
        ahead: u32,
    },
    /// `origin` could not be reached. The merge proceeds against the local
    /// target — being offline must not block a merge — but the caller has to
    /// say so, because the push may still be rejected.
    FetchFailed {
        local: Option<String>,
        reason: String,
    },
}

impl GitOperations {
    /// Run `cmd` to completion, killing it and returning
    /// `io::ErrorKind::TimedOut` if it hasn't finished within `timeout`.
    ///
    /// Generic and git-agnostic on purpose so it's directly unit-testable
    /// against a deterministic child (e.g. `sleep`) instead of relying on a
    /// real network hang, which would be slow and unreliable in CI.
    pub(crate) fn run_command_bounded(
        mut cmd: Command,
        timeout: Duration,
    ) -> std::io::Result<Output> {
        use std::io::Read;

        let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let deadline = Instant::now() + timeout;

        loop {
            if let Some(status) = child.try_wait()? {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut s) = child.stdout.take() {
                    let _ = s.read_to_end(&mut stdout);
                }
                if let Some(mut s) = child.stderr.take() {
                    let _ = s.read_to_end(&mut stderr);
                }
                return Ok(Output {
                    status,
                    stdout,
                    stderr,
                });
            }

            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("command timed out after {timeout:?}"),
                ));
            }

            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Create a branch from HEAD if it doesn't exist
    ///
    /// Returns true if the branch was created, false if it already existed.
    pub fn create_branch_if_not_exists(&self, branch: &str) -> Result<bool> {
        if self.branch_exists(branch)? {
            return Ok(false);
        }

        let output = Command::new("git")
            .args(["branch", branch])
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(true)
    }

    /// Create a branch from a specific base ref if it doesn't exist.
    ///
    /// Unlike `create_branch_if_not_exists`, this uses an explicit start point
    /// rather than the current HEAD. The start point is resolved to a commit ID
    /// before any ref is written, and the new local branch is read back through
    /// its fully-qualified ref before success is reported.
    ///
    /// Returns true if the branch was created, false if it already existed.
    pub fn create_branch_from(&self, branch: &str, base: &str) -> Result<bool> {
        if self.branch_exists(branch)? {
            return Ok(false);
        }

        let base_sha = self.resolve_commit(base).ok_or_else(|| {
            GitError::CommandFailed(format!(
                "Refusing to create branch '{branch}': start point '{base}' does not resolve to a commit"
            ))
        })?;

        let output = Command::new("git")
            .args(["branch", branch, &base_sha])
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let branch_ref = format!("refs/heads/{branch}");
        let written_sha = self.resolve_commit(&branch_ref).ok_or_else(|| {
            GitError::CommandFailed(format!(
                "Created branch '{branch}', but its ref '{branch_ref}' does not resolve to a commit"
            ))
        })?;
        if written_sha != base_sha {
            return Err(GitError::CommandFailed(format!(
                "Created branch '{branch}', but post-verification found {written_sha} instead of expected {base_sha}"
            )));
        }

        Ok(true)
    }

    /// Fetch a single branch from `origin`, bounded by [`FETCH_TIMEOUT`].
    ///
    /// Best-effort by design: callers should treat an `Err` as "could not
    /// verify freshness" (offline, no remote configured, remote branch
    /// doesn't exist yet, OR the remote didn't answer within the timeout)
    /// rather than a hard failure — local-only repos must keep working, and
    /// an unreachable-but-configured remote must degrade fast to the local
    /// fallback instead of hanging a synchronous caller (cas-0938).
    pub fn fetch_branch(&self, branch: &str) -> Result<()> {
        self.fetch_branch_bounded(branch, FETCH_TIMEOUT)
    }

    /// Same as [`Self::fetch_branch`] with an injectable timeout — the test
    /// seam that lets tests prove the bound actually fires without waiting
    /// out the default 10s.
    pub(crate) fn fetch_branch_bounded(&self, branch: &str, timeout: Duration) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["fetch", "origin", branch])
            // Never block on an interactive credential prompt — fail fast
            // instead of hanging (mirrors cas-38e2's fetch_parent_branch_best_effort).
            .env("GIT_TERMINAL_PROMPT", "0")
            .current_dir(&self.repo_root);

        let output = Self::run_command_bounded(cmd, timeout).map_err(|e| {
            if e.kind() == std::io::ErrorKind::TimedOut {
                GitError::CommandFailed(format!(
                    "git fetch origin {branch} timed out after {timeout:?} — remote unreachable?"
                ))
            } else {
                GitError::Io(e)
            }
        })?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    /// Count commits reachable from `to` but not from `from`
    /// (`git rev-list --count from..to`) — i.e. how far `from` is behind `to`.
    pub fn commits_behind(&self, from: &str, to: &str) -> Result<u32> {
        let output = Command::new("git")
            .args(["rev-list", "--count", &format!("{from}..{to}")])
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u32>()
            .map_err(|e| GitError::CommandFailed(format!("Failed to parse rev-list count: {e}")))
    }

    /// Resolve a branch-creation base against its remote tip (cas-b082 —
    /// BUG-epic-branch-stale-local-base; ahead/divergence handling cas-0938).
    ///
    /// Fetches `origin/<base>` and, when reachable AND local is not ahead
    /// of it, branches from the fetched remote tip rather than the local
    /// `<base>` ref — so a stale local base (the observed failure: local
    /// 30 commits behind origin) can never silently seed a new
    /// epic/worker branch. Logs a loud warning with the exact behind-count
    /// before returning in that case.
    ///
    /// When local carries commits `origin/<base>` lacks (ahead, or truly
    /// diverged — both ahead AND behind), prefers the LOCAL ref instead:
    /// taking `origin/<base>` unconditionally would silently drop the
    /// caller's own unpushed commits, which is worse than the staleness
    /// bug this function originally fixed. On true divergence this still
    /// reports `behind_count` so the caller can see what origin has that
    /// local doesn't, even though local was the one used.
    ///
    /// Falls back to the local `<base>` ref when there is no remote, the
    /// fetch fails (offline), or `origin/<base>` doesn't exist —
    /// local-only repos keep working unchanged.
    pub fn resolve_fresh_base(&self, base: &str) -> Result<ResolvedBase> {
        let remote_ref = format!("origin/{base}");
        let fetch_ok = self.fetch_branch(base).is_ok();

        if fetch_ok && self.branch_exists(&remote_ref).unwrap_or(false) {
            let local_exists = self.branch_exists(base).unwrap_or(false);
            let behind_count = if local_exists {
                self.commits_behind(base, &remote_ref).unwrap_or(0)
            } else {
                0
            };
            let ahead_count = if local_exists {
                self.commits_behind(&remote_ref, base).unwrap_or(0)
            } else {
                0
            };

            if ahead_count > 0 {
                if behind_count > 0 {
                    tracing::warn!(
                        "'{}' and 'origin/{}' have diverged ({} commit(s) local-only, {} \
                         commit(s) origin-only) — basing the new branch on the LOCAL ref to \
                         avoid silently dropping local-only commits. The {} origin-only \
                         commit(s) are NOT included; reconcile manually before relying on \
                         this branch if you need them.",
                        base, base, ahead_count, behind_count, behind_count
                    );
                } else {
                    tracing::info!(
                        "Local '{}' is {} commit(s) ahead of 'origin/{}' (unpushed) — basing \
                         the new branch on the local ref instead of the stale origin tip",
                        base, ahead_count, base
                    );
                }

                let sha = self.ref_sha(base).unwrap_or_default();
                return Ok(ResolvedBase {
                    branch_ref: base.to_string(),
                    sha,
                    behind_count,
                    ahead_count,
                    used_remote: false,
                });
            }

            if behind_count > 0 {
                tracing::warn!(
                    "Local '{}' is {} commit(s) behind 'origin/{}' — basing the new branch \
                     on the fetched remote tip instead of the stale local ref",
                    base,
                    behind_count,
                    base
                );
            }

            let sha = self.ref_sha(&remote_ref).unwrap_or_default();
            return Ok(ResolvedBase {
                branch_ref: remote_ref,
                sha,
                behind_count,
                ahead_count: 0,
                used_remote: true,
            });
        }

        let sha = self.ref_sha(base).unwrap_or_default();
        Ok(ResolvedBase {
            branch_ref: base.to_string(),
            sha,
            behind_count: 0,
            ahead_count: 0,
            used_remote: false,
        })
    }

    /// Choose the start point for a NEW epic branch, given the trunk base
    /// already resolved by the caller (cas-a85e / GH #99).
    ///
    /// Epic branches are anchored to trunk on purpose (cas-dc28) so an
    /// incidental HEAD — a worker branch, a feature branch, a detached
    /// checkout — can never seed one. That rule silently strands work in
    /// exactly one shape: the checkout is on the PREVIOUS epic branch, which
    /// carries commits trunk has never seen, and the follow-on epic is meant
    /// to continue them. Reported as GH #99 with a 36-commit gap.
    ///
    /// Decision table (HEAD vs `base_ref`):
    /// - HEAD is detached, is the base itself, or is not ahead → trunk, silent.
    /// - HEAD is an `epic/*` branch strictly ahead (contains the base) →
    ///   base from HEAD, and say so with the commit count.
    /// - HEAD is an `epic/*` branch that has DIVERGED (ahead and behind) →
    ///   trunk, with a warning naming both counts: auto-stacking would drop
    ///   the base-only commits, so a human has to choose.
    /// - HEAD is any other branch that is ahead → trunk (cas-dc28 holds),
    ///   with a note stating the divergence so it is never silent.
    ///
    /// Never fails: any git error degrades to the plain trunk choice, because
    /// failing epic creation over an advisory comparison would be worse than
    /// the staleness it reports.
    pub fn resolve_epic_base(&self, base_ref: &str) -> crate::worktree::git::EpicBaseChoice {
        use crate::worktree::git::EpicBaseChoice;

        let plain = EpicBaseChoice::plain(base_ref);

        let Ok(head) = self.current_branch() else {
            return plain;
        };
        let head = head.trim().to_string();
        // Detached HEAD ("HEAD"), or the checkout is already on the base.
        if head.is_empty()
            || head == "HEAD"
            || head == base_ref
            || base_ref == format!("origin/{head}")
        {
            return plain;
        }

        let Ok(head_ahead) = self.commits_behind(base_ref, &head) else {
            return plain;
        };
        if head_ahead == 0 {
            return plain;
        }
        let head_behind = self.commits_behind(&head, base_ref).unwrap_or(0);
        let is_epic_head = head.starts_with("epic/");

        if is_epic_head && head_behind == 0 {
            // cas-aae6 (GH #110): name the WHOLE stack, not just the branch
            // being based on. C on B on A used to read as "based on B", and
            // the A→B→C landing order stayed invisible until something failed
            // to merge.
            let ancestry = self.unlanded_epic_ancestry(&head, base_ref);
            let chain_note = if ancestry.is_empty() {
                String::new()
            } else {
                let mut order = ancestry.clone();
                order.push(head.clone());
                format!(
                    " STACK DEPTH {}: '{head}' already contains unlanded epic branch(es) {}. \
                     Landing this epic lands all of them together — they cannot be left behind, \
                     and no separate merge of each is required. Bottom-up they are {} → \
                     '{base_ref}'.",
                    order.len(),
                    ancestry
                        .iter()
                        .map(|b| format!("'{b}'"))
                        .collect::<Vec<_>>()
                        .join(" → "),
                    order
                        .iter()
                        .map(|b| format!("'{b}'"))
                        .collect::<Vec<_>>()
                        .join(" → "),
                )
            };
            return EpicBaseChoice {
                base_ref: head.clone(),
                notice: Some(format!(
                    "Based on the active epic branch '{head}' ({head_ahead} commit(s) ahead of \
                     '{base_ref}') so work already on it is not stranded. This branch therefore \
                     CONTAINS those commits: merging it to '{base_ref}' also merges '{head}', so \
                     land '{head}' first or accept that.{chain_note} Pass an explicit \
                     target_repo/target_branch, or check out '{base_ref}', to start from trunk \
                     instead."
                )),
                stacked_on: ancestry,
                head_branch: Some(head),
                head_ahead,
                head_behind,
                used_head: true,
            };
        }

        let notice = if is_epic_head {
            format!(
                "WARNING: the checkout is on epic branch '{head}', which has DIVERGED from \
                 '{base_ref}' ({head_ahead} commit(s) only on '{head}', {head_behind} only on \
                 '{base_ref}'). The new epic branch was based on '{base_ref}', so the \
                 {head_ahead} commit(s) on '{head}' are NOT included — merge or rebase the two \
                 before workers rely on this branch."
            )
        } else {
            format!(
                "Note: the checkout is on '{head}', {head_ahead} commit(s) ahead of \
                 '{base_ref}'. The new epic branch was based on '{base_ref}' — those commits \
                 are NOT included."
            )
        };

        EpicBaseChoice {
            base_ref: base_ref.to_string(),
            head_branch: Some(head),
            head_ahead,
            head_behind,
            used_head: false,
            // Trunk was chosen, so the new epic inherits no stack.
            stacked_on: Vec::new(),
            notice: Some(notice),
        }
    }

    /// Every unlanded `epic/*` branch contained in `branch`, trunk-first
    /// (cas-aae6 / GH #110).
    ///
    /// Epic stacking is legal and sometimes intended (cas-a85e bases a
    /// follow-on epic on the epic it continues), but it is only safe if the
    /// operator can see it. The cas-a85e notice described one level, so a
    /// three-deep stack — C on B on A — presented as "C is based on B" and the
    /// A→B→C merge order stayed invisible until something failed to land.
    ///
    /// "Contained" is asked of git directly (`merge-base --is-ancestor`), so
    /// the chain is derived from the repository rather than from bookkeeping
    /// that can drift. "Unlanded" means not yet reachable from `trunk`: once an
    /// epic lands, it stops constraining anything and drops out of the chain.
    ///
    /// Ordering is by distance from trunk ascending, i.e. bottom-up. For a true
    /// chain (each contained in the next) that is also the order they must land
    /// in. It is NOT a dependency claim in general: a branch that merges two
    /// independent unlanded epics has both as ancestors while neither contains
    /// the other, and either may land first. Ties keep a stable name order so
    /// the output is deterministic. Callers must not render this as a mandated
    /// sequence — what is always true is that the base contains all of them.
    ///
    /// Never fails: any git error yields an empty chain, because an advisory
    /// display must not be able to break epic creation.
    pub fn unlanded_epic_ancestry(&self, branch: &str, trunk: &str) -> Vec<String> {
        let listed = Command::new("git")
            .args([
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads/epic/",
            ])
            .current_dir(&self.repo_root)
            .output();
        let Ok(listed) = listed else {
            return Vec::new();
        };
        if !listed.status.success() {
            return Vec::new();
        }

        let branch_tip = self.ref_sha(branch).unwrap_or_default();
        let mut chain: Vec<(u32, String)> = Vec::new();

        for candidate in String::from_utf8_lossy(&listed.stdout)
            .lines()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            if candidate == branch {
                continue;
            }
            // Same commit under two names is not a stack, just an alias.
            if !branch_tip.is_empty() && self.ref_sha(candidate).unwrap_or_default() == branch_tip {
                continue;
            }
            if !self.is_ancestor(candidate, branch) {
                continue;
            }
            if self.is_ancestor(candidate, trunk) {
                continue; // already landed — constrains nothing
            }
            let distance = self.commits_behind(trunk, candidate).unwrap_or(0);
            chain.push((distance, candidate.to_string()));
        }

        chain.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        // Two names for the same commit (a rename whose old branch was never
        // deleted) are one rung, not two — otherwise the reported depth and the
        // printed order both inflate. Keeps the first name in sorted order.
        let mut seen_shas = std::collections::HashSet::new();
        chain
            .into_iter()
            .map(|(_, name)| name)
            .filter(|name| {
                let sha = self.ref_sha(name).unwrap_or_default();
                sha.is_empty() || seen_shas.insert(sha)
            })
            .collect()
    }

    /// Local branches whose tip has `rev` in its history, EXCLUDING `rev`'s own
    /// branch name (cas-f102, GH #140).
    ///
    /// This is the "are these commits reachable from anywhere else?" question,
    /// and it is the only safe pre-check before removing a factory worktree:
    /// `WorktreeManager::abandon` deletes the branch with `-D`, so a branch
    /// nothing else contains would take its commits with it.
    ///
    /// Deliberately NOT "is it merged into trunk": factory branches land on
    /// epic branches, so an ancestry test against the default branch
    /// false-negatives every correctly-merged worker.
    ///
    /// A git failure answers "empty" — the caller treats that as "not proven
    /// merged" and refuses without `force`, so an environment problem fails
    /// closed rather than authorising a delete.
    pub fn branches_containing(&self, rev: &str) -> Vec<String> {
        let Ok(output) = Command::new("git")
            .args(["branch", "--format=%(refname:short)", "--contains", rev])
            .current_dir(&self.repo_root)
            .output()
        else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| line.trim().trim_start_matches("* ").trim())
            .filter(|name| !name.is_empty() && !name.starts_with('(') && *name != rev)
            .map(str::to_string)
            .collect()
    }

    /// True when `ancestor` is reachable from `descendant`.
    ///
    /// A git failure answers `false`: callers use this to decide whether to
    /// *add* a warning, so an unknown must not manufacture one.
    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> bool {
        match Command::new("git")
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .current_dir(&self.repo_root)
            .status()
        {
            Ok(status) if status.code() == Some(0) => true,
            // Exit 1 is git's ordinary "no". Anything else (128 for an
            // unresolvable ref, a signal, a spawn failure) is git failing to
            // answer — same `false` result, but it must not look identical to
            // a real negative in the logs, or an environment problem renders
            // as "no stack here".
            Ok(status) if status.code() == Some(1) => false,
            other => {
                tracing::warn!(
                    ancestor = %ancestor,
                    descendant = %descendant,
                    result = ?other,
                    "cas-aae6: `git merge-base --is-ancestor` could not answer; \
                     treating as not-an-ancestor, so a real epic stack may be under-reported"
                );
                false
            }
        }
    }

    /// Resolve the full SHA of a ref (branch name, "HEAD", etc.).
    ///
    /// Returns a 40-character hex SHA, or a GitError if the ref doesn't exist.
    pub fn ref_sha(&self, ref_name: &str) -> Result<String> {
        let output = Command::new("git")
            .args(["rev-parse", ref_name])
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Push a branch to origin
    ///
    /// Pushes the specified branch to the 'origin' remote. If the branch doesn't exist
    /// on origin yet, it will be created. Uses -u to set up tracking.
    pub fn push_branch(&self, branch: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["push", "-u", "origin", branch])
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("Failed to push branch {}: {}", branch, stderr);
            return Err(GitError::CommandFailed(stderr.to_string()));
        }

        Ok(())
    }

    /// True when an `origin` remote is configured for this repository.
    ///
    /// Answers `false` on any git failure: callers use this to decide whether
    /// a *missing* remote ref is expected, and an unknown must not be reported
    /// as "you forgot to push".
    pub fn has_origin_remote(&self) -> bool {
        Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(&self.repo_root)
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    /// Publish `branch` to `origin` so the local ref and the remote-tracking
    /// ref agree (cas-5ee0 / GH #137).
    ///
    /// Called after a successful `worktree_merge`. Never returns `Err`: the
    /// point is to report push state truthfully, and a failed push must
    /// surface as a loud [`TargetPushOutcome::NotPushed`] receipt line rather
    /// than turning a completed merge into an error. The push is a plain
    /// (non-force) push, so git itself refuses to clobber a diverged remote
    /// and the divergence is reported instead.
    /// Bring the local target branch up to `origin/<target>` before a merge.
    ///
    /// GH #703: `worktree_merge` merged into a target whose origin had already
    /// advanced, so the push could only ever be rejected. Reconciling first
    /// turns that into an ordinary successful merge whenever the local target
    /// is merely behind, and into an explicit refusal when it is not.
    ///
    /// The ahead/behind comparison is made AFTER the fetch, against the
    /// freshly-updated `origin/<target>`; comparing first would judge
    /// divergence against a stale remote ref and could call a clean
    /// fast-forward a divergence (or worse, the reverse).
    pub fn reconcile_target_with_origin(&self, target: &str) -> TargetReconcile {
        self.reconcile_target_with_origin_bounded(target, FETCH_TIMEOUT)
    }

    pub(crate) fn reconcile_target_with_origin_bounded(
        &self,
        target: &str,
        timeout: Duration,
    ) -> TargetReconcile {
        if !self.has_origin_remote() {
            return TargetReconcile::NoRemote;
        }
        let local_before = self.resolve_commit(target);

        if let Err(error) = self.fetch_branch_bounded(target, timeout) {
            // Offline, auth-broken or unreachable: a merge must still be
            // possible. The caller reports the degradation rather than
            // pretending the target was verified against origin.
            return TargetReconcile::FetchFailed {
                local: local_before,
                reason: first_line(&error.to_string()),
            };
        }

        let remote_ref = format!("origin/{target}");
        let (Some(local), Some(remote)) =
            (self.resolve_commit(target), self.resolve_commit(&remote_ref))
        else {
            return TargetReconcile::NoRemote;
        };
        if local == remote {
            return TargetReconcile::AlreadyCurrent { sha: local };
        }

        let behind = self.commits_behind(&local, &remote).unwrap_or(0);
        let ahead = self.commits_behind(&remote, &local).unwrap_or(0);
        // cas-26c7: divergence requires BOTH sides to hold commits the other
        // lacks. Testing `ahead > 0` alone reported a target that is merely
        // unpublished as diverged, and the caller then printed a recovery
        // recipe (`git merge origin/<target>`) that is a no-op in that state —
        // an operator who followed it got the identical refusal back, with no
        // exit that did not bypass the tool.
        if ahead > 0 && behind > 0 {
            return TargetReconcile::Diverged {
                local,
                remote,
                ahead,
                behind,
            };
        }
        if ahead > 0 {
            // behind == 0: origin has not moved, so a later push is a
            // fast-forward. Nothing to reconcile — the target simply carries
            // commits that have not been published yet.
            return TargetReconcile::AheadOfRemote {
                local,
                remote,
                ahead,
            };
        }
        if behind == 0 {
            return TargetReconcile::AlreadyCurrent { sha: local };
        }

        // Strictly behind: a fast-forward loses nothing. Where the branch is
        // checked out decides how it moves — the same venue split the merge
        // itself uses, so the shared checkout's working tree stays in sync and
        // is never silently left behind its own branch ref.
        let advanced = if self.branch_is_checked_out_here(target) {
            self.run_git_ok(&["merge", "--ff-only", &remote_ref])
        } else {
            self.run_git_ok(&[
                "update-ref",
                &format!("refs/heads/{target}"),
                &remote,
                &local,
            ])
        };
        if !advanced {
            return TargetReconcile::Diverged {
                local,
                remote,
                ahead,
                behind,
            };
        }
        TargetReconcile::FastForwarded {
            from: local,
            to: remote,
            commits_gained: behind,
        }
    }

    /// Whether the repository root currently has `branch` checked out.
    fn branch_is_checked_out_here(&self, branch: &str) -> bool {
        Command::new("git")
            .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
            .current_dir(&self.repo_root)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim() == branch)
            .unwrap_or(false)
    }

    fn run_git_ok(&self, args: &[&str]) -> bool {
        Command::new("git")
            .args(args)
            .current_dir(&self.repo_root)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    pub fn publish_branch_to_origin(&self, branch: &str) -> TargetPushOutcome {
        self.publish_branch_to_origin_bounded(branch, PUSH_TIMEOUT)
    }

    /// Same as [`Self::publish_branch_to_origin`] with an injectable timeout —
    /// the test seam for proving the bound fires without waiting out 30s.
    pub(crate) fn publish_branch_to_origin_bounded(
        &self,
        branch: &str,
        timeout: Duration,
    ) -> TargetPushOutcome {
        let remote_ref = format!("origin/{branch}");
        let local_sha = self.resolve_commit(branch);

        if !self.has_origin_remote() {
            return TargetPushOutcome::NoRemote;
        }

        let Some(local_sha) = local_sha else {
            return TargetPushOutcome::NotPushed {
                sha: None,
                remote_sha: self.resolve_commit(&remote_ref),
                reason: format!("local ref `{branch}` did not resolve to a commit"),
            };
        };

        match self.resolve_commit(&remote_ref) {
            Some(remote) if remote == local_sha => {
                return TargetPushOutcome::AlreadyCurrent { sha: local_sha };
            }
            Some(_) => {}
            // Never *create* a remote branch as a side effect of a merge.
            None => return TargetPushOutcome::RemoteBranchAbsent,
        }

        let mut cmd = Command::new("git");
        cmd.args([
            "push",
            "origin",
            // Fully-qualified on both sides: a bare branch name lets git's
            // push.default / refspec config decide the destination, which is
            // exactly the kind of ambiguity this receipt is meant to remove.
            &format!("refs/heads/{branch}:refs/heads/{branch}"),
        ])
        // Never block on an interactive credential prompt (mirrors fetch).
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(&self.repo_root);

        match Self::run_command_bounded(cmd, timeout) {
            Ok(output) if output.status.success() => TargetPushOutcome::Pushed { sha: local_sha },
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let remote_sha = self.resolve_commit(&remote_ref);
                let outcome = classify_push_rejection(
                    branch,
                    &self.detect_default_branch(),
                    local_sha,
                    remote_sha,
                    &stderr,
                );
                tracing::warn!(
                    branch = %branch,
                    reason = %stderr.trim(),
                    "cas-5ee0: merge target was not published to origin"
                );
                outcome
            }
            Err(e) => {
                let reason = if e.kind() == std::io::ErrorKind::TimedOut {
                    format!(
                        "git push origin {branch} timed out after {timeout:?} — remote unreachable?"
                    )
                } else {
                    format!("git push origin {branch} could not run: {e}")
                };
                tracing::warn!(
                    branch = %branch,
                    reason = %reason,
                    "cas-5ee0: merge target was not published to origin"
                );
                TargetPushOutcome::NotPushed {
                    sha: Some(local_sha),
                    remote_sha: self.resolve_commit(&remote_ref),
                    reason,
                }
            }
        }
    }

    /// Mark .claude/, CLAUDE.md, and .mcp.json as skip-worktree in a worktree
    ///
    /// This prevents workers from accidentally staging and committing Cassy-synced
    /// changes to these tracked config files. The files remain in the worktree
    /// (Claude Code works normally) but git ignores local modifications.
    pub fn mark_config_skip_worktree(&self, worktree_path: &Path) -> Result<()> {
        let output = Command::new("git")
            .args(["ls-files", ".claude/", "CLAUDE.md", ".mcp.json"])
            .current_dir(worktree_path)
            .output()?;

        if !output.status.success() || output.stdout.is_empty() {
            return Ok(());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let files: Vec<&str> = stdout.lines().filter(|line| !line.is_empty()).collect();

        if files.is_empty() {
            return Ok(());
        }

        let mut args = vec!["update-index", "--skip-worktree"];
        args.extend(files.iter());

        let update_output = Command::new("git")
            .args(&args)
            .current_dir(worktree_path)
            .output()?;

        if !update_output.status.success() {
            tracing::warn!(
                "Failed to set skip-worktree on config files in {}: {}",
                worktree_path.display(),
                String::from_utf8_lossy(&update_output.stderr)
            );
        } else {
            tracing::info!(
                "Marked {} config files as skip-worktree in {}",
                files.len(),
                worktree_path.display()
            );
        }

        Ok(())
    }

    /// Reset a worktree to a specific branch/ref (hard reset)
    ///
    /// This is used to sync a worker's worktree to the latest epic branch.
    pub fn reset_hard_in_dir(&self, dir: &Path, target: &str) -> Result<()> {
        let fetch_output = Command::new("git")
            .args(["fetch", "--all"])
            .current_dir(dir)
            .output()?;

        if !fetch_output.status.success() {
            eprintln!(
                "[Cassy] Warning: git fetch failed: {}",
                String::from_utf8_lossy(&fetch_output.stderr)
            );
        }

        let output = Command::new("git")
            .args(["reset", "--hard", target])
            .current_dir(dir)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod protected_branch_tests {
    use super::{TargetPushOutcome, classify_push_rejection};

    const RECORDED_GH013: &str = "remote: error: GH013: Repository rule violations found for refs/heads/main.\nremote: - 2 of 2 required status checks are expected.\nTo github.com:pippenz/cas.git\n ! [remote rejected] main -> main (push declined due to repository rule violations)";

    #[test]
    fn recorded_gh013_on_default_branch_is_typed_as_pr_required() {
        let outcome = classify_push_rejection(
            "main",
            "main",
            "local-sha".to_string(),
            Some("remote-sha".to_string()),
            RECORDED_GH013,
        );

        assert_eq!(
            outcome,
            TargetPushOutcome::ProtectedDefaultBranch {
                sha: "local-sha".to_string(),
                remote_sha: Some("remote-sha".to_string()),
                reason: RECORDED_GH013.to_string(),
            }
        );
        assert!(!outcome.is_published());
    }

    #[test]
    fn gh013_on_non_default_target_keeps_the_existing_failure_shape() {
        let outcome = classify_push_rejection(
            "epic/release",
            "main",
            "local-sha".to_string(),
            Some("remote-sha".to_string()),
            RECORDED_GH013,
        );

        assert!(matches!(outcome, TargetPushOutcome::NotPushed { .. }));
    }
}
