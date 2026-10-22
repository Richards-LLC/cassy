//! First-use epic branch readiness and safe local fast-forwarding.

use std::process::Command;
use std::time::Duration;

use super::GitOperations;

/// One network observation, reused by local preparation and publication.
pub(crate) struct EpicPublicationObservation {
    pub remote: Result<Option<bool>, String>,
    pub base_fetched: bool,
    pub previous_remote: bool,
}

pub(crate) struct EpicBranchReadiness {
    pub notice: Option<String>,
    pub can_refresh_parent: bool,
}

impl EpicBranchReadiness {
    fn ready(notice: Option<String>) -> Self {
        Self {
            notice,
            can_refresh_parent: true,
        }
    }
    fn preserved(notice: String) -> Self {
        Self {
            notice: Some(notice),
            can_refresh_parent: false,
        }
    }
}

impl GitOperations {
    /// First use, rather than creation, publishes an epic. An absent remote
    /// is verified by ls-remote, never inferred from a failed fetch/cache miss.
    /// Creation uses an empty expected-old lease; an existing remote is never
    /// overwritten by this operation. No recorded parent (`None`) means
    /// publish the existing local tip as-is; never infer a trunk refresh.
    pub(crate) fn ensure_epic_published(
        &self,
        branch: &str,
        base: Option<&str>,
        origin: Option<&cas_types::EpicBranchOrigin>,
    ) -> Result<EpicBranchReadiness, String> {
        let observation = self.observe_epic_publication(branch, base);
        self.ensure_epic_published_observed(branch, base, &observation, origin)
    }

    pub(crate) fn observe_epic_publication(
        &self,
        branch: &str,
        base: Option<&str>,
    ) -> EpicPublicationObservation {
        let previous_remote = self.resolve_commit(&format!("origin/{branch}")).is_some();
        let remote = if !self.has_origin_remote() {
            Ok(None)
        } else {
            let mut command = Command::new("git");
            command
                .args([
                    "ls-remote",
                    "--heads",
                    "origin",
                    &format!("refs/heads/{branch}"),
                ])
                .env("GIT_TERMINAL_PROMPT", "0")
                .current_dir(&self.repo_root);
            match Self::run_command_bounded(command, Duration::from_secs(10)) {
                Ok(output) if output.status.success() => {
                    if output.stdout.is_empty() {
                        Ok(Some(false))
                    } else {
                        self.fetch_branch(branch)
                            .map(|_| Some(true))
                            .map_err(|e| e.to_string())
                    }
                }
                Ok(output) => Err(format!(
                    "origin presence unverified: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )),
                Err(error) => Err(format!("origin presence unverified: {error}")),
            }
        };
        let base_fetched = base.is_none_or(|base| {
            let base_branch = base.strip_prefix("origin/").unwrap_or(base);
            if base_branch == branch {
                matches!(remote, Ok(Some(true)))
            } else {
                self.has_origin_remote() && self.fetch_branch(base_branch).is_ok()
            }
        });
        EpicPublicationObservation {
            remote,
            base_fetched,
            previous_remote,
        }
    }

    /// The same cached local/origin comparison used by legacy spawn preparation.
    /// Fetch is owned by the observation, so retries here add no network work.
    pub(crate) fn shared_parent_ref(&self, branch: &str) -> Result<Option<String>, String> {
        if branch.starts_with("origin/") {
            return Ok(self.resolve_commit(branch).map(|_| branch.into()));
        }
        let local = self.resolve_commit(branch);
        let remote_ref = format!(
            "origin/{}",
            branch.strip_prefix("refs/heads/").unwrap_or(branch)
        );
        let Some(remote) = self.resolve_commit(&remote_ref) else {
            return Ok(local.map(|_| branch.into()));
        };
        let Some(local) = local else {
            return Ok(Some(remote_ref));
        };
        if local == remote {
            return Ok(Some(branch.into()));
        }
        let ahead = self
            .commits_behind(&remote, &local)
            .map_err(|e| e.to_string())?;
        let behind = self
            .commits_behind(&local, &remote)
            .map_err(|e| e.to_string())?;
        if ahead == 0 {
            return Ok(Some(remote_ref));
        }
        if behind == 0 {
            return Err(format!(
                "local parent '{branch}' ({local}) is ahead of shared '{remote_ref}' ({remote}); refusing unpublished local parent"
            ));
        }
        Err(format!(
            "recorded parent '{branch}' and fetched '{remote_ref}' have diverged; refusing to choose a worker base"
        ))
    }

    pub(crate) fn ensure_epic_published_observed(
        &self,
        branch: &str,
        base: Option<&str>,
        observation: &EpicPublicationObservation,
        origin: Option<&cas_types::EpicBranchOrigin>,
    ) -> Result<EpicBranchReadiness, String> {
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
        let initial_tip = self
            .resolve_commit(&refname)
            .unwrap_or_else(|| "missing".into());
        let remote_present = match &observation.remote {
            Ok(Some(present)) => *present,
            Ok(None) => {
                return Ok(EpicBranchReadiness::ready(Some(format!(
                    "EPIC BRANCH LOCAL-ONLY: '{branch}' ({}); origin is not configured; the local tip is unpublished (CAS_PUSH_EPIC_BRANCH={setting}).",
                    self.resolve_commit(&refname)
                        .unwrap_or_else(|| "missing".into())
                ))));
            }
            Err(error) => return Err(error.clone()),
        };
        if remote_present {
            let remote = self
                .resolve_commit(&format!("origin/{branch}"))
                .ok_or_else(|| format!("fetched epic '{branch}' has no tracking commit"))?;
            let Some(local) = self.resolve_commit(&refname) else {
                self.create_branch_from(branch, &remote)
                    .map_err(|e| e.to_string())?;
                return Ok(EpicBranchReadiness::ready(Some(format!(
                    "EPIC BRANCH MATERIALIZED: '{branch}' at published origin tip {remote}."
                ))));
            };
            if local == remote {
                return Ok(EpicBranchReadiness::ready(None));
            }
            let ahead = self
                .commits_behind(&remote, &local)
                .map_err(|e| e.to_string())?;
            let behind = self
                .commits_behind(&local, &remote)
                .map_err(|e| e.to_string())?;
            if ahead > 0 && behind > 0 {
                return Ok(EpicBranchReadiness::preserved(format!(
                    "EPIC BRANCH DIVERGED: '{branch}' local {local}, origin {remote}; no refs changed or published. Reconcile before relying on this lane."
                )));
            }
            if ahead == 0 {
                self.advance_local_branch(branch, &local, &remote)?;
                return Ok(EpicBranchReadiness::ready(Some(format!(
                    "SPAWN BASE REFRESHED: local '{branch}' ({local}) is {behind} commit(s) behind 'origin/{branch}' ({remote}); local branch FAST-FORWARDED to the published tip."
                ))));
            }
            return Ok(EpicBranchReadiness::ready(Some(format!(
                "EPIC BRANCH PRESERVED: '{branch}' ({local}) has {ahead} unpublished unique commit(s); existing origin {remote} was not rewritten."
            ))));
        }
        if observation.previous_remote {
            return Err(format!(
                "previously published '{branch}' is now absent on origin; refusing automatic resurrection"
            ));
        }
        let local = self
            .resolve_commit(&refname)
            .ok_or_else(|| Self::missing_epic_branch_message(branch, origin, false))?;
        if let Some(base) = base {
            if !observation.base_fetched {
                return Err(format!(
                    "base '{base}' fetch failed; publication is unverified, no local fallback was published"
                ));
            }
            let base_ref = self
                .shared_parent_ref(base)?
                .ok_or_else(|| format!("base '{base}' unresolved"))?;
            let base_sha = self
                .resolve_commit(&base_ref)
                .ok_or_else(|| format!("base '{base_ref}' unresolved"))?;
            if local != base_sha {
                let unique = self
                    .commits_behind(&base_sha, &local)
                    .map_err(|e| e.to_string())?;
                if unique == 0 {
                    self.advance_local_branch(branch, &local, &base_sha)?;
                    receipts.push(format!(
                        "FAST-FORWARDED local '{branch}' from {local} to base '{base_ref}' at {base_sha}"
                    ));
                } else {
                    receipts.push(format!("preserved {unique} unique local commit(s) on '{branch}' at {local} unchanged"));
                }
            }
        } else {
            receipts.push(format!(
                "preserved local '{branch}' at {local} unchanged; no recorded parent"
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
            return Ok(EpicBranchReadiness::preserved(format!(
                "EPIC BRANCH UNPUBLISHED: '{branch}' ({initial_tip}); {}expected-absent publication refused: {}. No existing remote ref was rewritten (CAS_PUSH_EPIC_BRANCH={setting}).",
                if receipts.is_empty() {
                    String::new()
                } else {
                    format!("{}; ", receipts.join("; "))
                },
                String::from_utf8_lossy(&publication.stderr).trim()
            )));
        }
        receipts.push(format!(
            "published '{branch}' to origin at {tip} with an expected-absent lease"
        ));
        Ok(EpicBranchReadiness::ready(Some(format!(
            "EPIC BRANCH READY: {} (CAS_PUSH_EPIC_BRANCH={setting}; publication deferred until first use when disabled at create).",
            receipts.join("; ")
        ))))
    }

    /// Missing refs do not prove deletion. Origin metadata only supplies an
    /// actionable host label; it never authorizes recreating or moving a ref.
    pub(crate) fn missing_epic_branch_message(
        branch: &str,
        origin: Option<&cas_types::EpicBranchOrigin>,
        previously_published: bool,
    ) -> String {
        if previously_published {
            return format!(
                "previously published '{branch}' is now absent on origin; refusing automatic resurrection"
            );
        }
        if let Some(origin) =
            origin.filter(|origin| origin.branch == branch && !origin.host.trim().is_empty())
        {
            return format!(
                "spawn refused: epic branch not published by {}\nNo local/origin ref; publish on {}, then retry:\n`git push origin {}`",
                origin.host,
                origin.host,
                shell_words::quote(branch),
            );
        }
        format!(
            "spawn base '{branch}' doesn't resolve locally or on origin\nLane may be unpublished or removed; no lane was recreated.\nAsk the originating host to publish or restore the branch."
        )
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
