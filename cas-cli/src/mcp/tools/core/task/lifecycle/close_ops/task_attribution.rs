//! Shared task delivery attribution for close gates and their receipt display.
use super::*;

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
                    .any(|word| word.starts_with("cas-") && word.len() > 4);
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
