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

/// Retain the originating host with the lane, not its host-local path or
/// current owner (which may already have moved to a peer supervisor).
pub(crate) fn branch_origin(branch: &str) -> Option<cas_types::EpicBranchOrigin> {
    let device = crate::cloud::device::DeviceConfig::load().ok().flatten();
    let device_id = device.as_ref().map(|device| device.device_id.clone())
        .filter(|id| !id.trim().is_empty());
    let host = crate::cloud::device::DeviceConfig::hostname()
        .or_else(|| device.as_ref().and_then(|device| device.name.clone()))
        .or_else(|| device_id.clone())?;
    Some(cas_types::EpicBranchOrigin {
        branch: branch.to_string(),
        host,
        device_id,
    })
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
pub(crate) fn ensure_adopted_lane(
    repo_root: &Path,
    lane: &str,
) -> (String, Option<cas_types::EpicBranchOrigin>) {
    let header = format!(
        "\n\n🌿 Epic lane adopted: {lane} (the explicit target_branch). Children inherit it and \
         worktree_merge lands on it; no auto lane was created."
    );
    if !GitOperations::is_git_available() {
        return (
            format!("{header}\n   git is unavailable, so the lane was not checked."),
            None,
        );
    }
    let Ok(root) = GitOperations::detect_repo_root(repo_root) else {
        return (
            format!("{header}\n   No git repository at {}.", repo_root.display()),
            None,
        );
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
        // A fetched or pre-existing lane supplies no evidence that this
        // adopter was its originating host.
        return (
            format!(
                "{header}\n   Tip: '{}' @ {sha} ({published}).{state}",
                resolved.branch_ref
            ),
            None,
        );
    }

    let trunk = crate::config::Config::configured_epic_base_branch(&root)
        .unwrap_or_else(|| git.detect_default_branch());
    let base = match git.resolve_fresh_base(&trunk) {
        Ok(base) if !base.sha.is_empty() => base,
        _ => {
            return (
                format!(
                    "{header}\n   {lane} does not exist and its base '{trunk}' could not be \
                 resolved; create {lane} before workers merge."
                ),
                None,
            );
        }
    };
    let created = match git.create_branch_from(lane, &base.sha) {
        Ok(created) => created,
        Err(error) => {
            return (
                format!(
                    "{header}\n   Creating {lane} from '{}' failed: {error}.",
                    base.branch_ref
                ),
                None,
            );
        }
    };
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
    (
        format!(
            "{header}\n   {lane} did not exist; created it from '{}' @ {sha}. Push state: {push_state}.",
            base.branch_ref
        ),
        created.then(|| branch_origin(lane)).flatten(),
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
    let (successor, local_behind) = match (resolve(branch), resolve(&format!("origin/{branch}"))) {
        // A fetched descendant includes everything the local copy has. For
        // divergent copies retain the local view; neither is more advanced.
        (Some(local), Some(remote)) if git_ok(repo, &["merge-base", "--is-ancestor", &local, &remote]) => {
            let local_behind = local != remote;
            (remote, local_behind)
        }
        (Some(local), _) => (local, false),
        (None, Some(remote)) => (remote, false),
        (None, None) => return String::new(),
    };
    // Compare every last-observed predecessor tip against that same successor.
    // max_by_key keeps the last tie, so a matching local ref remains the remedy.
    let Some((source, tip, lacking)) = [format!("origin/{predecessor_branch}"), predecessor_branch.to_string()]
        .into_iter()
        .filter_map(|name| {
            let tip = resolve(&name)?;
            let lacking = git_stdout(repo, &["rev-list", "--count", &format!("{successor}..{tip}")])?
                .parse::<u64>().ok()?;
            (lacking > 0).then_some((name, tip, lacking))
        })
        .max_by_key(|(_, _, lacking)| *lacking)
    else {
        return String::new();
    };
    let short = &tip[..tip.len().min(12)];
    let [merge] =
        match crate::worktree::GitOperations::git_remedies([&["merge", "--no-ff", &source]]) {
            Ok(commands) => commands,
            Err(refusal) => return format!("\nSTALE BASE: {refusal}\n"),
        };
    let branch = crate::worktree::GitOperations::visible_git_label(branch);
    let before_merge = if local_behind {
        format!("fast-forward {branch} first, then ")
    } else {
        String::new()
    };
    let predecessor_branch = crate::worktree::GitOperations::visible_git_label(predecessor_branch);
    let probe = std::process::Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["merge-tree", "--write-tree", "--name-only", "--no-messages", &successor, &tip])
        .current_dir(repo)
        .output();
    match probe {
        Ok(output) if output.status.code() == Some(0) => format!(
            "\n⚠️ STALE BASE (cas-fd4c): stacked on open epic {predecessor_id} ({predecessor_branch}), \
             which has advanced to {short}; {branch} lacks {lacking} of its commit(s). It merges \
             cleanly: {before_merge}run `{merge}` on {branch} and push, so work here builds \
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
            const MAX_CONFLICT_FILES: usize = 8;
            let mut preview = files.iter().take(MAX_CONFLICT_FILES).cloned().collect::<Vec<_>>().join(", ");
            let omitted = files.len().saturating_sub(MAX_CONFLICT_FILES);
            if omitted > 0 {
                preview.push_str(&format!(", +{omitted} more"));
            }
            format!(
                "\n⛔ STALE BASE, MERGE REFUSED (cas-fd4c): stacked on open epic {predecessor_id} \
                 ({predecessor_branch}) @ {short}, which {branch} lacks ({lacking} commit(s)). \
                 Merging it would conflict in: {}. Nothing was merged. Resolve that merge by hand \
                 on {branch}, or land {predecessor_id} first.\n",
                preview
            )
        }
        _ => format!(
            "\n⚠️ STALE BASE (cas-fd4c): stacked on open epic {predecessor_id} ({predecessor_branch}), \
             which has advanced to {short}; {branch} lacks {lacking} of its commit(s). Cassy could \
             not check whether merging it is clean.\n"
        ),
    }
}

/// cas-1630: warn about explicitly required code missing from this epic.
/// Repro/evidence citations and dangling candidates do not declare a base.
pub(crate) fn cited_commit_warning(
    repo: &Path,
    store: &dyn cas_store::TaskStore,
    epic: &cas_types::Task,
    text: &str,
) -> String {
    let Some(branch) = epic.branch.as_deref().filter(|branch| !branch.trim().is_empty()) else {
        return String::new();
    };
    let Ok(pattern) = regex::Regex::new(
        r"(?i)(?:\b(?:depends?\s+on|requires?|needs?|based\s+on|builds?\s+on)\s+|\b(?:base(?:\s+commit)?|dependenc(?:y|ies))\s*[:=]\s*)(?:(?:the|code|fix|commit|in|from|at)\s+)*`?([0-9a-f]{7,40})\b",
    ) else {
        return String::new();
    };
    let Ok(negated) = regex::Regex::new(r"(?i)\b(?:not|never|without|no|doesn't|don't)\s*$") else {
        return String::new();
    };
    let required: Vec<_> = pattern.captures_iter(text)
        .filter(|capture| !negated.is_match(&text[..capture.get(0).unwrap().start()]))
        .filter_map(|capture| capture.get(1).map(|sha| sha.as_str()))
        .filter(|sha| sha.bytes().any(|byte| byte.is_ascii_alphabetic()))
        .collect();
    // Most tasks declare no code dependency; skip the task scan for them.
    if required.is_empty() {
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
    for found in required {
        if !seen.insert(found) || seen.len() > 10 {
            continue;
        }
        if !git_ok(repo, &["rev-parse", "--verify", "--quiet", &format!("{found}^{{commit}}")])
            || on_branch(repo, found, branch)
            || git_stdout(repo, &["for-each-ref", "--count=1", "--format=%(refname)", "--contains", found])
                .is_none_or(|refs| refs.is_empty())
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
        "\n\n⚠️ CITED CODE NOT ON THIS EPIC [CITED_CODE_NOT_ON_EPIC] (cas-1630): {}. The task cannot start until that code reaches {branch}: merge the epic that has it into {branch}, or file the task under that epic.",
        lines.join("; ")
    )
}

/// Staffing is still admitted when an epic lacks published release history.
/// Use publication receipts, not arbitrary tags, and the subject's own repo.
pub(crate) fn published_release_note(
    root: &Path,
    store: &dyn cas_store::TaskStore,
    subject: &cas_types::Task,
) -> String {
    let inspect = || -> Option<String> {
        let epic = if subject.task_type == cas_types::TaskType::Epic {
            subject.clone()
        } else {
            store.get_parent_epic(&subject.id).ok()??
        };
        let branch = super::super::repo_context::effective_epic_branch(&epic)?;
        let repo = crate::train_guardrails::train_repo_root(root, &epic).ok()?;
        if subject.task_type != cas_types::TaskType::Epic
            && let Some(target) = super::super::repo_context::effective_task_work_target(subject, Some(&epic))
            && (target.target_branch.strip_prefix("origin/").unwrap_or(&target.target_branch) != branch
                || !super::super::repo_context::repo_answers_to(&repo, &target.repo_selector))
        {
            return None;
        }
        let release_root = crate::store::find_cas_root_ignoring_env(&repo).ok()?;
        let version = cas_store::quality_release_events(&release_root).ok()?
            .into_iter().find_map(|event| event.release_version)?;
        let mut tag = version.trim().to_string();
        if !git_ok(&repo, &["rev-parse", "--verify", "--quiet", &format!("refs/tags/{tag}^{{commit}}")]) {
            tag = format!("v{}", tag.trim_start_matches('v'));
        }
        let release = git_stdout(&repo, &["rev-parse", "--verify", &format!("refs/tags/{tag}^{{commit}}")])?;
        // Match spawning's fresher-origin choice without fetching or moving refs.
        let resolve = |reference: &str| git_stdout(&repo, &["rev-parse", "--verify", "--quiet", &format!("{reference}^{{commit}}")]);
        let tip = match (resolve(branch), resolve(&format!("origin/{branch}"))) {
            (Some(local), Some(remote)) => {
                if git_ok(&repo, &["merge-base", "--is-ancestor", &local, &remote]) { remote } else { local }
            }
            (Some(tip), None) | (None, Some(tip)) => tip,
            (None, None) => return None,
        };
        let missing: usize = git_stdout(&repo, &["rev-list", "--count", &format!("{tip}..{release}"), "--"])?
            .parse().ok()?;
        if missing == 0 { return None; }
        let target = epic.deliverables.work_target.as_ref()
            .map(|target| target.target_branch.clone())
            .filter(|target| target != branch)
            .unwrap_or_else(|| GitOperations::new(repo).detect_default_branch());
        Some(format!(
            "\n⚠️ [EPIC_BEHIND_RELEASE] epic={} branch={branch} tag={tag} missing_commits={missing}. Sync {target} into {branch} before staffing more workers.",
            epic.id,
        ))
    };
    inspect().unwrap_or_default()
}
