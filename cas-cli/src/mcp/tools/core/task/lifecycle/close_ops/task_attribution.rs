//! Shared task delivery attribution for close gates and their receipt display.
use super::*;

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

#[derive(Debug)]
struct DeliveryRange {
    base: String,
    tip: String,
}

fn git_text(repo: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Current-cycle commits and durably identified earlier-cycle commits share
/// the same epoch contract in selection and receipt validation.
pub(super) fn in_work_window(window: &TaskCommitReceiptWindow, epoch: i64, owned: bool) -> bool {
    epoch
        >= window
            .not_before
            .timestamp()
            .saturating_sub(COMMIT_RECEIPT_CLOCK_SKEW_SECS)
        || (owned
            && epoch
                >= window
                    .task_floor
                    .timestamp()
                    .saturating_sub(COMMIT_RECEIPT_CLOCK_SKEW_SECS))
}

/// Select first-parent delivery segments once for every close consumer.
/// Target-reachable history needs durable task identity. Unmerged history may
/// also use the lease window. A receipt expands through unnamed predecessors
/// in that window, stopping at another task or the task's time boundary.
fn task_delivery_ranges(
    repo: &Path,
    parent: &str,
    window: &TaskCommitReceiptWindow,
    tip: Option<&str>,
) -> Option<Vec<DeliveryRange>> {
    if !is_safe_git_refname(parent) {
        return None;
    }
    let target = preferred_diff_target_ref(repo, parent);
    let tip = tip.unwrap_or("HEAD");
    let base = git_text(repo, &["merge-base", tip, &target])?;
    let unmerged: std::collections::HashSet<String> = git_text(
        repo,
        &["rev-list", "--first-parent", &format!("{base}..{tip}")],
    )?
    .lines()
    .map(str::to_owned)
    .collect();
    let mut history_args = vec![
        "log".to_string(),
        "--first-parent".into(),
        "--reverse".into(),
        "--format=%H%x1f%P%x1f%ct%x1f%B%x1e".into(),
    ];
    if let Some(since) = task_commit_receipt_since(window.task_floor) {
        history_args.push(format!("--since-as-filter={since}"));
    }
    history_args.push(tip.into());
    let history = git_text(
        repo,
        &history_args.iter().map(String::as_str).collect::<Vec<_>>(),
    )?;
    struct Commit {
        sha: String,
        parent: String,
        epoch: i64,
        owned: bool,
        foreign: bool,
        merge: bool,
    }
    let commits: Vec<Commit> = history
        .split('\u{1e}')
        .filter_map(|record| {
            let fields: Vec<_> = record.trim().splitn(4, '\u{1f}').collect();
            if fields.len() != 4 {
                return None;
            }
            let sha = fields[0].to_string();
            let message = fields[3];
            let owned = window.identity.matches_known_commit(&sha)
                || window
                    .identity
                    .task_id
                    .as_deref()
                    .is_some_and(|id| message_references_task(message, id));
            let foreign = !owned
                && message
                    .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                    .any(|word| {
                        // TaskStore::generate_hash_id emits hexadecimal IDs.
                        // Crate/path words such as cas-cli are not task claims.
                        word.get(..4)
                            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cas-"))
                            && word.get(4..).is_some_and(|id| {
                                (4..=8).contains(&id.len())
                                    && id.bytes().all(|byte| byte.is_ascii_hexdigit())
                            })
                    });
            Some(Commit {
                sha,
                parent: fields[1].split_whitespace().next().unwrap_or("").into(),
                epoch: fields[2].parse().ok()?,
                owned,
                foreign,
                merge: fields[1].split_whitespace().count() > 1,
            })
        })
        .collect();
    let floor = window
        .task_floor
        .timestamp()
        .saturating_sub(COMMIT_RECEIPT_CLOCK_SKEW_SECS);
    let mut selected: Vec<bool> = commits
        .iter()
        .map(|c| {
            !c.parent.is_empty()
                && !c.foreign
                && (c.owned || unmerged.contains(&c.sha))
                && in_work_window(window, c.epoch, c.owned)
        })
        .collect();
    // The end of a delivery is an upper boundary, not its only commit.
    for i in (0..commits.len()).rev() {
        if !selected[i]
            || commits[i].merge
            || !window.identity.matches_known_commit(&commits[i].sha)
        {
            continue;
        }
        let mut j = i;
        while j > 0 {
            let previous = &commits[j - 1];
            if previous.parent.is_empty()
                || previous.foreign
                || previous.merge
                || previous.epoch < floor
            {
                break;
            }
            selected[j - 1] = true;
            j -= 1;
        }
    }
    let mut ranges: Vec<DeliveryRange> = vec![];
    let mut contiguous = false;
    for (commit, selected) in commits.into_iter().zip(selected) {
        if selected {
            if contiguous {
                ranges.last_mut()?.tip = commit.sha;
            } else {
                ranges.push(DeliveryRange {
                    base: commit.parent,
                    tip: commit.sha,
                });
            }
        }
        contiguous = selected;
    }
    Some(ranges)
}

pub(super) fn reviewable(
    repo: &Path,
    target: &str,
    window: &TaskCommitReceiptWindow,
) -> Option<bool> {
    let ranges = task_delivery_ranges(repo, target, window, None)?;
    for range in ranges {
        let paths = git_text(
            repo,
            &["diff", "--name-only", &range.base, &range.tip, "--"],
        )?;
        if paths.lines().any(is_reviewable_path) {
            return Some(true);
        }
    }
    Some(false)
}

pub(super) fn violations(
    repo: &Path,
    target: &str,
    window: &TaskCommitReceiptWindow,
    is_violation: &impl Fn(char) -> bool,
) -> Option<Vec<AdditiveOnlyViolation>> {
    let ranges = task_delivery_ranges(repo, target, window, None)?;
    let mut violations = vec![];
    for range in ranges {
        let statuses = git_text(
            repo,
            &["diff", "--name-status", &range.base, &range.tip, "--"],
        )?;
        violations.extend(retain_foreign_violations(
            repo,
            &range.base,
            &window.identity,
            parse_name_status_filtered(&statuses, is_violation),
        ));
    }
    Some(violations)
}

pub(super) fn diff_stat(
    repo: &Path,
    target: &str,
    window: &TaskCommitReceiptWindow,
    receipt: Option<&str>,
) -> Option<TaskAttributedDiffStat> {
    let receipt = receipt
        .map(|receipt| resolve_task_commit_receipt_sha(repo, receipt))
        .transpose()
        .ok()?;
    let ranges = task_delivery_ranges(repo, target, window, receipt.as_deref())?;
    Some(TaskAttributedDiffStat {
        stat: ranges
            .iter()
            .map(|r| get_diff_stat_for_range(repo, &r.base, &r.tip))
            .filter(|stat| !stat.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        target_ref: preferred_diff_target_ref(repo, target),
        basis: "task delivery commits bounded by target and work window",
    })
}

/// Return the paths carried by the same task-attributed delivery ranges used
/// by the close diff stat. Risk proof must not fall back to an unrelated
/// branch-wide diff: a proof narrower than this set is a hard close failure.
pub(super) fn paths(
    repo: &Path,
    target: &str,
    window: &TaskCommitReceiptWindow,
    receipt: Option<&str>,
) -> Option<Vec<String>> {
    let receipt = receipt
        .map(|receipt| resolve_task_commit_receipt_sha(repo, receipt))
        .transpose()
        .ok()?;
    let ranges = task_delivery_ranges(repo, target, window, receipt.as_deref())?;
    let mut paths = Vec::new();
    for range in ranges {
        let changed = git_text(repo, &["diff", "--name-only", &range.base, &range.tip, "--"])?;
        paths.extend(
            changed
                .lines()
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(ToOwned::to_owned),
        );
    }
    paths.sort();
    paths.dedup();
    Some(paths)
}

/// Prove the content of a merge-tip delivery from the task's first-parent
/// commits rather than from the merge tip itself.
///
/// A worker commonly merges the current integration target into its factory
/// branch before handing it back to the supervisor. That merge can have no
/// first-parent tree effect even though the worker's earlier task commits are
/// present on the merge's first-parent side. The merge tip is therefore an
/// attribution boundary, not delivery content. Reuse the task-window
/// selection above and validate every selected non-merge commit individually
/// against the authoritative target.
///
/// An explicit receipt is accepted as an additional candidate only when it is
/// a non-merge commit on the merge tip's first-parent history. The close path
/// validates that receipt's normal topology, timestamp, diff, and target
/// content predicates before calling this helper.
pub(super) fn merge_tip_content_presence(
    repo: &Path,
    target: &str,
    merge_tip: &str,
    window: Option<&TaskCommitReceiptWindow>,
    identity: &TaskCommitIdentity,
    validated_receipt: Option<&str>,
) -> Option<DeliveryContentPresence> {
    let fallback_window = TaskCommitReceiptWindow {
        supervisor_override_reason: None,
        not_before: chrono::DateTime::from_timestamp(0, 0)?,
        task_floor: chrono::DateTime::from_timestamp(0, 0)?,
        basis: "task identity fallback",
        identity: identity.clone(),
    };
    let window = window.unwrap_or(&fallback_window);
    let ranges = task_delivery_ranges(repo, target, window, Some(merge_tip))?;

    let first_parent_commits = git_text(
        repo,
        &["rev-list", "--first-parent", "--reverse", merge_tip],
    )?;
    let first_parent_commits = first_parent_commits
        .lines()
        .filter(|commit| !commit.is_empty())
        .collect::<Vec<_>>();

    let mut commits = Vec::new();
    for range in ranges {
        let range = format!("{}..{}", range.base, range.tip);
        let range_commits = git_text(repo, &["rev-list", "--first-parent", "--reverse", &range])?;
        for commit in range_commits.lines().filter(|commit| !commit.is_empty()) {
            if !commits.iter().any(|known| known == commit) && !is_merge_commit(repo, commit) {
                commits.push(commit.to_string());
            }
        }
    }

    if let Some(receipt) = validated_receipt
        .and_then(|receipt| super::resolve_task_commit_receipt_sha(repo, receipt).ok())
        .filter(|receipt| {
            first_parent_commits
                .iter()
                .any(|commit| *commit == receipt.as_str())
        })
        .filter(|receipt| !is_merge_commit(repo, receipt))
        && !commits.iter().any(|commit| commit == &receipt)
    {
        commits.push(receipt);
    }

    if commits.is_empty() {
        return None;
    }

    // GH #840: a worker-owned merge can replace an earlier task hunk before
    // the final merge tip is integrated. When the tip and target agree on a
    // delivered path, preserve that final tree effect instead of requiring
    // the original non-merge patch to remain byte-for-byte applicable.
    let merge_tip_paths = merge_tip_tree_effect_paths(repo, &target, merge_tip, &commits)?;
    let mut present_paths = Vec::new();
    let mut superseded_paths = Vec::new();
    let mut superseding_commits = Vec::new();
    let mut dropped_paths = Vec::new();
    let mut unknown_reason = None;
    for commit in commits {
        match super::delivery_content_presence_in_parent(repo, &commit, target) {
            DeliveryContentPresence::Present { paths } => {
                append_unique(&mut present_paths, paths);
            }
            DeliveryContentPresence::Superseded { paths, commits } => {
                append_unique(&mut superseded_paths, paths);
                append_unique(&mut superseding_commits, commits);
            }
            DeliveryContentPresence::Dropped { paths } => {
                append_unique(
                    &mut dropped_paths,
                    paths
                        .into_iter()
                        .filter(|path| !merge_tip_paths.contains(path))
                        .collect(),
                );
            }
            DeliveryContentPresence::Unknown { reason } => {
                unknown_reason.get_or_insert(reason);
            }
        }
    }

    if !dropped_paths.is_empty() {
        Some(DeliveryContentPresence::Dropped {
            paths: dropped_paths,
        })
    } else if let Some(reason) = unknown_reason {
        Some(DeliveryContentPresence::Unknown { reason })
    } else if !superseded_paths.is_empty() {
        Some(DeliveryContentPresence::Superseded {
            paths: superseded_paths,
            commits: superseding_commits,
        })
    } else {
        Some(DeliveryContentPresence::Present {
            paths: present_paths,
        })
    }
}

fn is_merge_commit(repo: &Path, commit: &str) -> bool {
    git_text(repo, &["rev-list", "--parents", "-n", "1", commit])
        .is_some_and(|parents| parents.split_whitespace().count() > 2)
}

fn append_unique(values: &mut Vec<String>, additions: Vec<String>) {
    for value in additions {
        if !values.contains(&value) {
            values.push(value);
        }
    }
}

/// Paths whose final merge-tip tree effect is the task delivery, even when an
/// earlier non-merge task commit's exact patch no longer applies.
///
/// A worker can resolve a target-sync merge on top of an earlier task commit,
/// replacing its hunk while retaining the intended path. The old attribution
/// check only inspected each non-merge commit against the target and therefore
/// reported that earlier hunk as dropped. Compare the merge tip to the
/// authoritative target per delivered path, but require the merge tip to
/// differ from at least one task commit's first parent. That last predicate
/// preserves the genuinely-empty/reverted-delivery rejection.
fn merge_tip_tree_effect_paths(
    repo: &Path,
    target: &str,
    merge_tip: &str,
    commits: &[String],
) -> Option<HashSet<String>> {
    let mut paths = HashSet::new();
    for commit in commits {
        let parent_ref = format!("{commit}^1");
        let parent = git_text(repo, &["rev-parse", &parent_ref])?;
        let changed = git_text(
            repo,
            &["diff", "--name-only", "--no-renames", &parent, commit, "--"],
        )?;
        for path in changed.lines().filter(|path| !path.is_empty()) {
            let anchor_path = format!("{merge_tip}:{path}");
            let anchor_exists = Command::new("git")
                .args(["cat-file", "-e", &anchor_path])
                .current_dir(repo)
                .status()
                .ok()?
                .success();
            if !anchor_exists {
                continue;
            }
            let anchor_matches_target = git_diff_is_empty(repo, merge_tip, target, path)?;
            let anchor_differs_from_parent = !git_diff_is_empty(repo, &parent, merge_tip, path)?;
            if anchor_matches_target && anchor_differs_from_parent {
                paths.insert(path.to_string());
            }
        }
    }
    Some(paths)
}

fn git_diff_is_empty(repo: &Path, left: &str, right: &str, path: &str) -> Option<bool> {
    let status = Command::new("git")
        .args(["diff", "--quiet", "--no-renames", left, right, "--", path])
        .current_dir(repo)
        .status()
        .ok()?;
    match status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    fn git(repo: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
            .env("GIT_COMMITTER_NAME", "fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
            .env("GIT_AUTHOR_DATE", "2026-09-01T12:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-09-01T12:00:00Z")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
    fn commit(repo: &Path, file: &str, contents: &str, message: &str) -> String {
        std::fs::write(repo.join(file), contents).unwrap();
        git(repo, &["add", file]);
        git(repo, &["commit", "-qm", message]);
        git(repo, &["rev-parse", "HEAD"])
    }
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        commit(dir.path(), "copy.txt", "old\n", "baseline");
        git(dir.path(), &["checkout", "-qb", "factory/worker"]);
        dir
    }
    fn window() -> TaskCommitReceiptWindow {
        let floor = chrono::DateTime::parse_from_rfc3339("2026-09-01T11:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        TaskCommitReceiptWindow {
            supervisor_override_reason: None,
            not_before: floor,
            task_floor: floor,
            basis: "fixture",
            identity: TaskCommitIdentity {
                task_id: Some("cas-taskb".into()),
                known_commits: vec![],
            },
        }
    }
    #[test]
    fn crate_names_are_not_foreign_task_ids() {
        let dir = fixture();
        let p = dir.path();
        commit(
            p,
            "crate_change.rs",
            "own\n",
            "fix cas-cli build and cas-core integration",
        );
        commit(p, "foreign.rs", "foreign\n", "cas-a1b2: unrelated task");
        commit(
            p,
            "explicit.rs",
            "explicit\n",
            "cas-taskb: follow up on cas-a1b2",
        );
        let known = commit(p, "recorded.rs", "known\n", "cas-cafe: recorded delivery");
        let mut scope = window();
        scope.identity.known_commits.push(known);
        let stat = diff_stat(p, "main", &scope, None).unwrap().stat;
        assert!(
            stat.contains("crate_change.rs"),
            "crate wording must remain attributed: {stat}"
        );
        assert!(
            !stat.contains("foreign.rs"),
            "other task IDs must remain excluded: {stat}"
        );
        assert!(
            stat.contains("explicit.rs") && stat.contains("recorded.rs"),
            "own task ID and known commit must take precedence: {stat}"
        );
    }

    #[test]
    fn historical_record_receipt_requires_audited_supervisor_exception() {
        let dir = fixture();
        let p = dir.path();
        commit(
            p,
            "hotfix.rs",
            "pub fn fixed() {}\n",
            "hotfix delivered before record task",
        );
        git(p, &["checkout", "main"]);
        git(
            p,
            &["merge", "--no-ff", "factory/worker", "-m", "hotfix merge"],
        );
        let receipt = git(p, &["rev-parse", "HEAD"]);
        let mut scope = window();
        scope.not_before += chrono::Duration::days(2);
        scope.task_floor = scope.not_before;
        assert!(
            validate_task_commit_receipt(p, &receipt, "main", &scope)
                .unwrap_err()
                .contains("predates")
        );
        scope.supervisor_override_reason = Some("record previously delivered hotfix".into());
        let note = validate_task_commit_receipt(p, &receipt, "main", &scope).unwrap();
        assert!(
            note.contains("Registered supervisor override")
                && note.contains("record previously delivered hotfix")
        );
        let outcome = check_zero_commit_close(
            p,
            "main",
            "cas-taskb",
            &TaskType::Bug,
            None,
            false,
            None,
            Some(&receipt),
            Some(&scope),
        );
        assert!(
            matches!(outcome, ZeroCommitCloseOutcome::ProceedWithReceipt(ref decision)
            if decision.contains("Registered supervisor override")),
            "{outcome:?}"
        );
        // An epoch exception does not authorize an unmerged receipt.
        git(p, &["checkout", "factory/worker"]);
        let unmerged = commit(p, "other.rs", "unmerged\n", "another change");
        assert!(
            validate_task_commit_receipt(p, &unmerged, "main", &scope)
                .unwrap_err()
                .contains("not an ancestor")
        );
    }

    #[test]
    fn receipt_upper_boundary_excludes_later_sibling_commit() {
        let dir = fixture();
        let p = dir.path();
        commit(p, "first.md", "first\n", "first part");
        let receipt = commit(p, "second.md", "second\n", "second part");
        commit(p, "sibling.rs", "foreign\n", "cas-sibling: other task");
        let stat = render_close_diff_stat(p, "main", Some(&window()), Some(&receipt));
        assert!(
            stat.contains("first.md") && stat.contains("second.md") && !stat.contains("sibling.rs"),
            "{stat}"
        );
    }

    #[test]
    fn value_only_ignores_earlier_task_merged_on_remote_target() {
        let dir = fixture();
        let p = dir.path();
        let prior = commit(
            p,
            "prior.rs",
            "pub fn prior() {}\n",
            "cas-taska: prior delivery",
        );
        git(p, &["update-ref", "refs/remotes/origin/main", &prior]);
        commit(p, "copy.txt", "new\n", "cas-taskb: edit value");
        assert!(check_value_only_branch_violations(p, "main", None, &window().identity).is_empty());
        assert!(
            violations(p, "main", &window(), &|status| status != 'M')
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn receipt_diff_includes_unnamed_predecessor_of_two_commit_delivery() {
        let dir = fixture();
        let p = dir.path();
        commit(p, "first.md", "first\n", "first part");
        let receipt = commit(p, "second.md", "second\n", "second part");
        git(p, &["update-ref", "refs/remotes/origin/main", &receipt]);
        let stat = render_close_diff_stat(p, "main", Some(&window()), Some(&receipt));
        assert!(
            stat.contains("first.md") && stat.contains("second.md"),
            "{stat}"
        );
        let mut restarted = window();
        restarted.not_before += chrono::Duration::days(1);
        let stat = render_close_diff_stat(p, "main", Some(&restarted), Some(&receipt));
        assert!(
            stat.contains("first.md") && stat.contains("second.md"),
            "{stat}"
        );
    }
    #[test]
    fn no_code_after_reset_to_remote_trunk_ignores_inherited_code() {
        let dir = fixture();
        let p = dir.path();
        let release = commit(
            p,
            "release.rs",
            "pub fn released() {}\n",
            "merged epic release",
        );
        git(p, &["update-ref", "refs/remotes/origin/main", &release]);
        git(p, &["reset", "--hard", "origin/main"]);
        commit(p, "NOTES.md", "release notes\n", "cas-taskb: documentation");
        assert_eq!(
            has_task_attributable_reviewable_changes(p, "main", &window()),
            Some(false)
        );
    }
}
