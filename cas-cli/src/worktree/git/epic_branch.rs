//! First-use epic branch readiness and safe local fast-forwarding.

use std::process::Command;

use super::GitOperations;

impl GitOperations {
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
