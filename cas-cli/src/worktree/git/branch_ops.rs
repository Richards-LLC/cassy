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
    /// rather than the current HEAD. Pass the configured trunk (e.g. "main") so
    /// epic and worker branches are always anchored to the correct base.
    ///
    /// Returns true if the branch was created, false if it already existed.
    pub fn create_branch_from(&self, branch: &str, base: &str) -> Result<bool> {
        if self.branch_exists(branch)? {
            return Ok(false);
        }

        let output = Command::new("git")
            .args(["branch", branch, base])
            .current_dir(&self.repo_root)
            .output()?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
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
                    " STACK DEPTH {}: '{head}' is itself based on unlanded epic branch(es) {}. \
                     Everything above lands with it — merge order is {} → '{base_ref}'.",
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
    /// Ordering is by distance from trunk ascending, so the returned list reads
    /// in the order the branches must land. Ties (independent branches at the
    /// same distance) keep a stable name order.
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
        chain.into_iter().map(|(_, name)| name).collect()
    }

    /// True when `ancestor` is reachable from `descendant`.
    ///
    /// A git failure answers `false`: callers use this to decide whether to
    /// *add* a warning, so an unknown must not manufacture one.
    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> bool {
        Command::new("git")
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .current_dir(&self.repo_root)
            .status()
            .map(|status| status.code() == Some(0))
            .unwrap_or(false)
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

    /// Mark .claude/, CLAUDE.md, and .mcp.json as skip-worktree in a worktree
    ///
    /// This prevents workers from accidentally staging and committing CAS-synced
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
                "[CAS] Warning: git fetch failed: {}",
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
