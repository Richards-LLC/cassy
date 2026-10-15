//! GH #1216 (cas-cf8e0): an epic created with an explicit `epic/` target
//! branch adopts that branch as its delivery lane.
//!
//! Before this, `task create task_type=epic target_branch=epic/x` recorded
//! `epic/x` as the epic's WorkTarget but still cut an auto lane
//! (`epic/<title-slug>-<id>`, local-only unless `CAS_PUSH_EPIC_BRANCH`).
//! Children inherit `epic.branch`, so every delivery and `worktree_merge`
//! landed on that local lane while the declared, pushed `epic/x` stayed at
//! trunk.
//!
//! An explicit target under `epic/` is a lane by name: it becomes
//! `epic.branch` and no auto lane is created. Any other explicit target
//! (`main`, `staging`, `release/*`) stays the epic's base and PR target
//! (cas-3afc, GH #298); the create receipt then says where deliveries land.

use std::path::Path;

use crate::worktree::GitOperations;

/// Red-first stub (cas-cf8e0): today's behaviour.
const RED_STUB: bool = true;

/// The explicit target an epic create adopts as its lane, if any.
pub(crate) fn adopted_epic_lane(target_branch: Option<&str>) -> Option<&str> {
    if RED_STUB {
        return None;
    }
    target_branch
        .map(str::trim)
        .filter(|branch| branch.len() > "epic/".len() && branch.starts_with("epic/"))
}

/// The receipt note for an explicit target that is the epic's base rather
/// than its lane.
pub(crate) fn explicit_base_note(target_branch: Option<&str>, lane: &str) -> String {
    if RED_STUB {
        return String::new();
    }
    match target_branch
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
    {
        Some(base) if adopted_epic_lane(Some(base)).is_none() => format!(
            "\n   Deliveries land on {lane}; '{base}' is its base and the epic's PR target. \
             To deliver straight to a lane you created, pass target_branch=epic/<name>."
        ),
        _ => String::new(),
    }
}

fn push_enabled() -> bool {
    std::env::var("CAS_PUSH_EPIC_BRANCH")
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "on"))
        .unwrap_or(false)
}

/// Make sure the adopted `lane` exists in `repo_root` and describe it.
///
/// - On origin: its fresh tip is used, and a missing local branch is created
///   at that tip, so workers start from what was pushed.
/// - Local only: it is used as it is.
/// - Nowhere: it is created from the configured trunk, under the same push
///   rule as an auto lane, and the receipt says so.
pub(crate) fn ensure_adopted_lane(repo_root: &Path, lane: &str) -> String {
    let header = format!(
        "\n\n🌿 Epic lane adopted: {lane} (the explicit target_branch). Children inherit it and \
         worktree_merge lands on it; no auto lane was created."
    );
    if !GitOperations::is_git_available() {
        return format!("{header}\n   git is unavailable, so the lane was not checked.");
    }
    let Ok(root) = GitOperations::detect_repo_root(repo_root) else {
        return format!("{header}\n   No git repository at {}.", repo_root.display());
    };
    let git = GitOperations::new(root.clone());
    let local = git.branch_exists(lane).unwrap_or(false);
    let resolved = git
        .resolve_fresh_base(lane)
        .ok()
        .filter(|resolved| !resolved.sha.is_empty());
    if let Some(resolved) = resolved {
        let sha = &resolved.sha[..resolved.sha.len().min(8)];
        let state = if local {
            String::new()
        } else {
            match git.create_branch_from(lane, &resolved.sha) {
                Ok(_) => format!(" Created the local branch at '{}'.", resolved.branch_ref),
                Err(error) => format!(" Creating the local branch failed: {error}."),
            }
        };
        let published = if resolved.used_remote || resolved.branch_ref.starts_with("origin/") {
            "published on origin"
        } else {
            "local-only; push it so deliveries survive this machine"
        };
        return format!(
            "{header}\n   Tip: '{}' @ {sha} ({published}).{state}",
            resolved.branch_ref
        );
    }

    let trunk = crate::config::Config::configured_epic_base_branch(&root)
        .unwrap_or_else(|| git.detect_default_branch());
    let base = match git.resolve_fresh_base(&trunk) {
        Ok(base) if !base.sha.is_empty() => base,
        _ => {
            return format!(
                "{header}\n   {lane} does not exist and its base '{trunk}' could not be \
                 resolved; create {lane} before workers merge."
            );
        }
    };
    if let Err(error) = git.create_branch_from(lane, &base.sha) {
        return format!(
            "{header}\n   Creating {lane} from '{}' failed: {error}.",
            base.branch_ref
        );
    }
    let sha = &base.sha[..base.sha.len().min(8)];
    let push_state = if !push_enabled() {
        format!(
            "local-only (CAS_PUSH_EPIC_BRANCH is disabled); push it with `git push -u origin {lane}`"
        )
    } else {
        match git.push_branch(lane) {
            Ok(()) => format!("published {lane} to origin"),
            Err(error) => format!("local-only; publishing {lane} to origin failed: {error}"),
        }
    };
    format!(
        "{header}\n   {lane} did not exist; created it from '{}' @ {sha}. Push state: {push_state}.",
        base.branch_ref
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_epic_namespace_target_is_adopted() {
        assert_eq!(
            adopted_epic_lane(Some("epic/violet-log-hygiene")),
            Some("epic/violet-log-hygiene")
        );
        assert_eq!(adopted_epic_lane(Some("  epic/x ")), Some("epic/x"));
        for base in [
            None,
            Some(""),
            Some("main"),
            Some("staging"),
            Some("epic/"),
            Some("release/epic/x"),
        ] {
            assert_eq!(adopted_epic_lane(base), None, "{base:?}");
        }
        assert!(
            explicit_base_note(Some("staging"), "epic/a-cas-1").contains("'staging' is its base")
        );
        assert!(explicit_base_note(Some("epic/x"), "epic/x").is_empty());
        assert!(explicit_base_note(None, "epic/a-cas-1").is_empty());
    }
}
