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

/// The explicit target an epic create adopts as its lane, if any.
pub(crate) fn adopted_epic_lane(target_branch: Option<&str>) -> Option<&str> {
    target_branch
        .map(str::trim)
        .filter(|branch| branch.len() > "epic/".len() && branch.starts_with("epic/"))
}

/// The receipt note for an explicit target that is the epic's base rather
/// than its lane.
pub(crate) fn explicit_base_note(target_branch: Option<&str>, lane: &str) -> String {
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
            "local-only; it will be published on first focus or spawn"
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
            "local-only (CAS_PUSH_EPIC_BRANCH is disabled); it will be published on first focus or spawn"
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

/// cas-1630: the open epic a new epic follows, from its `blocked_by` ids:
/// the first non-terminal epic that already has a branch. A successor epic
/// stacks on that branch so follow-ups on unreleased code can start.
pub(crate) fn open_predecessor_epic(
    store: &dyn cas_store::TaskStore,
    blocked_by: &[String],
) -> Option<(String, String)> {
    blocked_by.iter().find_map(|id| {
        let task = store.get(id).ok()?;
        let branch = task.branch.clone().filter(|branch| !branch.trim().is_empty())?;
        (task.task_type == cas_types::TaskType::Epic && !task.is_terminal())
            .then(|| (task.id, branch))
    })
}

fn git_ok(repo: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .current_dir(repo)
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Whether `commit` is on `branch`, locally or as last fetched from origin.
fn on_branch(repo: &Path, commit: &str, branch: &str) -> bool {
    [branch.to_string(), format!("origin/{branch}")].iter().any(|candidate| {
        git_ok(repo, &["merge-base", "--is-ancestor", commit, candidate])
    })
}

fn git_stdout(repo: &Path, args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .current_dir(repo)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// cas-fd4c: a successor epic stacked on an open predecessor epic goes stale
/// when the predecessor advances. Name the predecessor commit this epic's
/// `branch` lacks, and whether merging it in is clean or would conflict.
/// Read-only: nothing is merged here, and a conflict is refused with the
/// files it would touch. Empty when the successor already has every commit.
pub(crate) fn stale_base_note(
    repo: &Path,
    branch: &str,
    predecessor_id: &str,
    predecessor_branch: &str,
) -> String {
    let resolve = |name: &str| git_stdout(repo, &["rev-parse", "--verify", "--quiet", &format!("{name}^{{commit}}")]);
    let Some(successor) = resolve(branch).or_else(|| resolve(&format!("origin/{branch}"))) else {
        return String::new();
    };
    // The predecessor as this clone last saw it, locally or on origin.
    let Some((source, tip)) = [predecessor_branch.to_string(), format!("origin/{predecessor_branch}")]
        .into_iter()
        .filter_map(|name| resolve(&name).map(|tip| (name, tip)))
        .find(|(_, tip)| !git_ok(repo, &["merge-base", "--is-ancestor", tip, &successor]))
    else {
        return String::new();
    };
    let short = &tip[..tip.len().min(12)];
    let lacking = git_stdout(repo, &["rev-list", "--count", &format!("{successor}..{tip}")])
        .unwrap_or_else(|| "some".to_string());
    let probe = std::process::Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["merge-tree", "--write-tree", "--name-only", "--no-messages", &successor, &tip])
        .current_dir(repo)
        .output();
    match probe {
        Ok(output) if output.status.code() == Some(0) => format!(
            "\n⚠️ STALE BASE (cas-fd4c): stacked on open epic {predecessor_id} ({predecessor_branch}), \
             which has advanced to {short}; {branch} lacks {lacking} of its commit(s). It merges \
             cleanly: run `git merge --no-ff {source}` on {branch} and push, so work here builds \
             on it.\n"
        ),
        Ok(output) if output.status.code() == Some(1) => {
            let files: Vec<String> = String::from_utf8_lossy(&output.stdout)
                .lines()
                .skip(1)
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect();
            format!(
                "\n⛔ STALE BASE, MERGE REFUSED (cas-fd4c): stacked on open epic {predecessor_id} \
                 ({predecessor_branch}) @ {short}, which {branch} lacks ({lacking} commit(s)). \
                 Merging it would conflict in: {}. Nothing was merged. Resolve that merge by hand \
                 on {branch}, or land {predecessor_id} first.\n",
                files.join(", ")
            )
        }
        _ => format!(
            "\n⚠️ STALE BASE (cas-fd4c): stacked on open epic {predecessor_id} ({predecessor_branch}), \
             which has advanced to {short}; {branch} lacks {lacking} of its commit(s). Cassy could \
             not check whether merging it is clean.\n"
        ),
    }
}

/// cas-1630: a follow-up created in `epic` that cites a commit its branch
/// lacks cannot start until that code reaches the branch. Name each such
/// commit, and the open epic that has it, so the base is fixed at create.
pub(crate) fn cited_commit_warning(
    repo: &Path,
    store: &dyn cas_store::TaskStore,
    epic: &cas_types::Task,
    text: &str,
) -> String {
    let Some(branch) = epic.branch.as_deref().filter(|branch| !branch.trim().is_empty()) else {
        return String::new();
    };
    let Ok(pattern) = regex::Regex::new(r"\b[0-9a-f]{7,40}\b") else {
        return String::new();
    };
    // Most tasks cite no commit; skip the task scan for them.
    if !pattern
        .find_iter(text)
        .any(|m| m.as_str().bytes().any(|byte| byte.is_ascii_alphabetic()))
    {
        return String::new();
    }
    let mut seen = std::collections::BTreeSet::new();
    let others: Vec<(String, String)> = store
        .list(None)
        .unwrap_or_default()
        .into_iter()
        .filter(|task| {
            task.task_type == cas_types::TaskType::Epic && !task.is_terminal() && task.id != epic.id
        })
        .filter_map(|task| task.branch.clone().map(|branch| (task.id, branch)))
        .collect();
    let mut lines = Vec::new();
    for found in pattern.find_iter(text).map(|m| m.as_str()) {
        if !found.bytes().any(|byte| byte.is_ascii_alphabetic()) || !seen.insert(found) || seen.len() > 10 {
            continue;
        }
        if !git_ok(repo, &["rev-parse", "--verify", "--quiet", &format!("{found}^{{commit}}")])
            || on_branch(repo, found, branch)
        {
            continue;
        }
        let holder = others
            .iter()
            .find(|(_, other)| on_branch(repo, found, other))
            .map(|(id, other)| format!(" it is on open epic {id} ({other})"))
            .unwrap_or_else(|| " no open epic has it".to_string());
        lines.push(format!("{found} is not on {branch};{holder}"));
    }
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "\n\n⚠️ CITED CODE NOT ON THIS EPIC (cas-1630): {}. The task cannot start until that code reaches {branch}: merge the epic that has it into {branch}, or file the task under that epic.",
        lines.join("; ")
    )
}
