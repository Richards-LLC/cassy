//! First-use epic branch readiness and safe local fast-forwarding.

use std::process::Command;
use std::time::Duration;

use super::GitOperations;

impl GitOperations {
    /// First use, rather than creation, publishes an epic. An absent remote
    /// is verified by ls-remote, never inferred from a failed fetch/cache miss.
    /// Creation uses an empty expected-old lease; an existing remote is never
    /// overwritten by this operation.
    pub(crate) fn ensure_epic_published(
        &self,
        branch: &str,
        base: &str,
    ) -> Result<Option<String>, String> {
        let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);
        let refname = format!("refs/heads/{branch}");
        let valid = Command::new("git")
            .args(["check-ref-format", &refname])
            .current_dir(&self.repo_root)
            .output()
            .map_err(|e| e.to_string())?;
        if !valid.status.success() || !branch.starts_with("epic/") {
            return Err(format!(
                "'{branch}' is not an epic lane eligible for first-use publication"
            ));
        }
        let setting = std::env::var("CAS_PUSH_EPIC_BRANCH")
            .unwrap_or_else(|_| "unset (disabled at create)".into());
        let mut receipts = Vec::new();
        let remote_present = if self.has_origin_remote() {
            let mut command = Command::new("git");
            command
                .args(["ls-remote", "--heads", "origin", &refname])
                .env("GIT_TERMINAL_PROMPT", "0")
                .current_dir(&self.repo_root);
            let result = Self::run_command_bounded(command, Duration::from_secs(10))
                .map_err(|e| format!("origin presence for '{branch}' is unverified: {e}"))?;
            if !result.status.success() {
                return Err(format!(
                    "origin presence for '{branch}' is unverified: {}",
                    String::from_utf8_lossy(&result.stderr).trim()
                ));
            }
            !result.stdout.is_empty()
        } else {
            return Ok(Some(format!(
                "EPIC BRANCH LOCAL-ONLY: '{branch}'; origin is not configured (CAS_PUSH_EPIC_BRANCH={setting})."
            )));
        };
        if remote_present {
            self.fetch_branch(branch).map_err(|e| e.to_string())?;
            let remote = self
                .resolve_commit(&format!("origin/{branch}"))
                .ok_or_else(|| format!("fetched epic '{branch}' has no tracking commit"))?;
            let Some(local) = self.resolve_commit(&refname) else {
                self.create_branch_from(branch, &remote)
                    .map_err(|e| e.to_string())?;
                return Ok(Some(format!(
                    "EPIC BRANCH MATERIALIZED: '{branch}' at published origin tip {remote}."
                )));
            };
            if local == remote {
                return Ok(None);
            }
            let ahead = self
                .commits_behind(&remote, &local)
                .map_err(|e| e.to_string())?;
            let behind = self
                .commits_behind(&local, &remote)
                .map_err(|e| e.to_string())?;
            if ahead > 0 && behind > 0 {
                return Ok(Some(format!(
                    "EPIC BRANCH DIVERGED: '{branch}' local {local}, origin {remote}; no refs changed or published. Reconcile before relying on this lane."
                )));
            }
            if ahead == 0 {
                self.advance_local_branch(branch, &local, &remote)?;
                return Ok(Some(format!(
                    "SPAWN BASE REFRESHED: local '{branch}' ({local}) is {behind} commit(s) behind 'origin/{branch}' ({remote}); local branch FAST-FORWARDED to the published tip."
                )));
            }
            return Ok(Some(format!(
                "EPIC BRANCH PRESERVED: '{branch}' has {ahead} unpublished unique commit(s); existing origin {remote} was not rewritten."
            )));
        }
        let base = self.resolve_fresh_base(base).map_err(|e| e.to_string())?;
        if base.sha.is_empty() {
            return Err(format!(
                "cannot publish '{branch}': recorded/configured base is unresolved"
            ));
        }
        if let Some(local) = self.resolve_commit(&refname) {
            if local != base.sha {
                let unique = self
                    .commits_behind(&base.sha, &local)
                    .map_err(|e| e.to_string())?;
                if unique == 0 {
                    self.advance_local_branch(branch, &local, &base.sha)?;
                    receipts.push(format!(
                        "FAST-FORWARDED local '{branch}' from {local} to base '{}' at {}",
                        base.branch_ref, base.sha
                    ));
                } else {
                    receipts.push(format!("preserved {unique} unique local commit(s) on '{branch}' at {local} unchanged"));
                }
            }
        } else {
            self.create_branch_from(branch, &base.sha)
                .map_err(|e| e.to_string())?;
            receipts.push(format!(
                "created local '{branch}' from base '{}' at {}",
                base.branch_ref, base.sha
            ));
        }
        let tip = self
            .resolve_commit(&refname)
            .ok_or_else(|| format!("epic '{branch}' no longer resolves"))?;
        let mut command = Command::new("git");
        command
            .args([
                "push",
                &format!("--force-with-lease={refname}:"),
                "origin",
                &format!("{tip}:{refname}"),
            ])
            .env("GIT_TERMINAL_PROMPT", "0")
            .current_dir(&self.repo_root);
        let publication =
            Self::run_command_bounded(command, Duration::from_secs(30)).map_err(|e| {
                format!(
                    "publication of '{branch}' is unverified: {e}; reconcile origin before retrying"
                )
            })?;
        if !publication.status.success() {
            return Ok(Some(format!(
                "EPIC BRANCH UNPUBLISHED: {}; expected-absent publication of '{branch}' refused: {}. No existing remote ref was rewritten (CAS_PUSH_EPIC_BRANCH={setting}).",
                receipts.join("; "),
                String::from_utf8_lossy(&publication.stderr).trim()
            )));
        }
        receipts.push(format!(
            "published '{branch}' to origin at {tip} with an expected-absent lease"
        ));
        Ok(Some(format!(
            "EPIC BRANCH READY: {} (CAS_PUSH_EPIC_BRANCH={setting}; publication deferred until first use when disabled at create).",
            receipts.join("; ")
        )))
    }

    /// Move only an ancestor ref, protecting every checkout that holds it.
    /// The expected old SHA prevents a concurrent writer from being overwritten.
    pub(crate) fn advance_local_branch(
        &self,
        branch: &str,
        from: &str,
        to: &str,
    ) -> Result<(), String> {
        let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);
        let ancestry = Command::new("git")
            .args(["merge-base", "--is-ancestor", from, to])
            .current_dir(&self.repo_root)
            .output()
            .map_err(|error| error.to_string())?;
        if !ancestry.status.success() {
            return Err(format!("'{branch}' is not eligible for fast-forward"));
        }
        let checkouts = self.list_worktrees().map_err(|error| error.to_string())?;
        let checkouts: Vec<_> = checkouts
            .into_iter()
            .filter(|checkout| checkout.branch.as_deref() == Some(branch))
            .collect();
        for checkout in &checkouts {
            let dirty = self
                .classify_dirty_status(&checkout.path)
                .map_err(|error| error.to_string())?;
            if dirty.is_blocked() {
                return Err(format!(
                    "'{branch}' is checked out with tracked changes at {} ({})",
                    checkout.path.display(),
                    dirty.describe_blocking()
                ));
            }
        }
        let update = Command::new("git")
            .args(["update-ref", &format!("refs/heads/{branch}"), to, from])
            .current_dir(&self.repo_root)
            .output()
            .map_err(|error| error.to_string())?;
        if !update.status.success() {
            return Err(String::from_utf8_lossy(&update.stderr).trim().into());
        }
        for checkout in checkouts {
            if let super::CheckoutRefresh::LeftStale { reason } =
                self.refresh_linked_checkout(&checkout.path, branch, from, to)
            {
                return Err(format!(
                    "'{branch}' advanced but checkout {} could not be refreshed: {reason}",
                    checkout.path.display()
                ));
            }
        }
        Ok(())
    }
}
