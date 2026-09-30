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
    let historical_receipt = tip.is_some() && window.supervisor_override_reason.is_some();
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
    // An explicit supervisor override may adopt a delivery made before the
    // task existed. Keep that history visible when an identified receipt is
    // supplied; the receipt itself is still validated by the close gates.
    if !historical_receipt && let Some(since) = task_commit_receipt_since(window.task_floor) {
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
            let owned = (historical_receipt && sha == tip)
                || window.identity.matches_known_commit(&sha)
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
                && (in_work_window(window, c.epoch, c.owned) || (historical_receipt && c.owned))
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
                || (!historical_receipt && previous.epoch < floor)
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
        let changed = git_text(
            repo,
            &["diff", "--name-only", &range.base, &range.tip, "--"],
        )?;
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

/// Reviewable paths from selected task ranges. An empty result is meaningful
/// when the task only merged history or changed whitespace/deletions.
pub(super) fn qa_paths(
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
    if ranges.is_empty() {
        return None;
    }
    let mut paths = Vec::new();
    for range in ranges {
        paths.extend(crate::qa_pass::delivery_content_paths(repo, &range.base, &range.tip).ok()?);
    }
    paths.sort();
    paths.dedup();
    Some(paths)
}

/// The immutable baseline a scoped proof must be measured against: the first
/// parent of the earliest commit in this task's delivery.
///
/// This is deliberately the *same* selection that [`paths`] uses, because the
/// close gate derives the required proof targets from those paths. Computing
/// the base any other way lets the two disagree — which is exactly what
/// happened when the base came from `merge-base(HEAD, target)`: once the
/// supervisor merged the branch into the target before close, that merge-base
/// collapsed onto the delivery commit, the surface diff became empty, and the
/// receipt said `targets=none` while the gate still demanded real targets. No
/// honest run could satisfy both (cas-9c1e).
///
/// Unlike a merge-base against `HEAD`, this depends only on the target, the
/// task window and the receipt, so it does not move when the worktree is
/// rebased, when the branch is merged, or when the gate happens to inspect a
/// checkout parked on another branch.
pub(super) fn delivery_base(
    repo: &Path,
    target: &str,
    window: &TaskCommitReceiptWindow,
    receipt: Option<&str>,
) -> Option<String> {
    let receipt = receipt
        .map(|receipt| resolve_task_commit_receipt_sha(repo, receipt))
        .transpose()
        .ok()?;
    let ranges = task_delivery_ranges(repo, target, window, receipt.as_deref())?;
    let base = ranges.first()?.base.clone();
    (!base.is_empty()).then_some(base)
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
    let full_tip = git_text(repo, &["rev-parse", &format!("{merge_tip}^{{commit}}")])?;
    let merge_tip = full_tip.as_str();
    let has_work_window = window.is_some();
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

    // A supervisor can put the task lane on the merge's second-parent
    // side. Its identified content still belongs to the task; restricting
    // attribution to first-parent history would silently omit that delivery.
    if let Some(id) = identity.task_id.as_deref() {
        let mut args = vec![
            "log".to_string(),
            "--no-merges".into(),
            "--reverse".into(),
            "--format=%H%x1f%ct%x1f%B%x1e".into(),
            "--fixed-strings".into(),
            format!("--grep={id}"),
        ];
        if let Some(since) = task_commit_receipt_since(window.task_floor) {
            args.push(format!("--since-as-filter={since}"));
        }
        args.push(merge_tip.into());
        let history = git_text(repo, &args.iter().map(String::as_str).collect::<Vec<_>>())?;
        for record in history.split('\u{1e}') {
            let fields: Vec<_> = record.trim().splitn(3, '\u{1f}').collect();
            if fields.len() != 3
                || !message_references_task(fields[2], id)
                || delivery_evolution::is_revert_message(fields[2])
            {
                continue;
            }
            let Ok(epoch) = fields[1].parse::<i64>() else {
                continue;
            };
            if in_work_window(window, epoch, true)
                && !commits.iter().any(|known| known == fields[0])
            {
                commits.push(fields[0].to_string());
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

    // GH #1018: once the worker's target-sync merge lands, the ordinary
    // target-relative range is empty. An unnamed content commit on the
    // merge's first-parent side is still task work when it was made inside
    // this work cycle and contributes a path to the merge's tree beyond its
    // target-sync parent. The latter condition keeps an empty sync merge
    // from masquerading as delivery.
    if commits.is_empty() && has_work_window {
        let parents = git_text(repo, &["rev-list", "--parents", "-n", "1", merge_tip])?;
        let parents = parents.split_whitespace().collect::<Vec<_>>();
        if parents.len() >= 3 {
            let first = parents[1];
            let second = parents[2];
            let delivered_paths =
                git_text(repo, &["diff", "--name-only", second, merge_tip, "--"])?
                    .lines()
                    .map(str::to_owned)
                    .collect::<HashSet<_>>();
            if !delivered_paths.is_empty() {
                let base = git_text(repo, &["merge-base", first, second])?;
                let range = format!("{base}..{first}");
                let history = git_text(
                    repo,
                    &[
                        "log",
                        "--first-parent",
                        "--no-merges",
                        "--reverse",
                        "--format=%H%x1f%ct%x1e",
                        &range,
                    ],
                )?;
                for record in history.split('\u{1e}') {
                    let Some((sha, epoch)) = record.trim().split_once('\u{1f}') else {
                        continue;
                    };
                    let Ok(epoch) = epoch.trim().parse::<i64>() else {
                        continue;
                    };
                    // The recorded merge anchor binds this first-parent range
                    // to the task. A later administrative restart may move
                    // not_before past these unnamed commits, but cannot move
                    // the task's creation floor.
                    if epoch
                        < window
                            .task_floor
                            .timestamp()
                            .saturating_sub(COMMIT_RECEIPT_CLOCK_SKEW_SECS)
                    {
                        continue;
                    }
                    let parent = git_text(repo, &["rev-parse", &format!("{sha}^1")])?;
                    let paths = git_text(repo, &["diff", "--name-only", &parent, sha, "--"])?;
                    if paths.lines().any(|path| delivered_paths.contains(path)) {
                        commits.push(sha.to_string());
                    }
                }
            }
        }
    }

    let order = git_text(repo, &["rev-list", "--topo-order", "--reverse", merge_tip])?;
    let selected: HashSet<_> = commits.into_iter().collect();
    let mut commits: Vec<String> = order
        .lines()
        .filter(|commit| selected.contains(*commit))
        .map(str::to_owned)
        .collect();

    // Reverts are negative delivery evidence, never a later task delivery.
    // Apply the exclusion to every selection route, including known receipts.
    let mut content_commits = Vec::new();
    for commit in commits {
        let message = git_text(repo, &["show", "-s", "--format=%B", &commit])?;
        if !delivery_evolution::is_revert_message(&message) {
            content_commits.push(commit);
        }
    }
    let commits = content_commits;
    let resolution_paths = merge_resolution_paths(repo, merge_tip, identity)?;
    if commits.is_empty() && resolution_paths.is_empty() {
        return None;
    }
    let origin = format!("origin/{target}");
    let target_ref =
        if git_ref_exists(repo, &origin) && git_commit_is_ancestor(repo, merge_tip, &origin) {
            origin
        } else {
            target.to_string()
        };
    let mut present_paths = Vec::new();
    let mut superseded_paths = Vec::new();
    let mut superseding_commits = Vec::new();
    let mut dropped_paths = Vec::new();
    let mut unknown_reason = None;
    let mut proven_resolutions = HashSet::new();
    if !resolution_paths.is_empty() {
        match super::delivery_content_presence_in_parent_for_paths(
            repo,
            merge_tip,
            target,
            Some(&resolution_paths),
        ) {
            DeliveryContentPresence::Present { paths } => {
                proven_resolutions.extend(paths.iter().cloned());
                append_unique(&mut present_paths, paths);
            }
            DeliveryContentPresence::Superseded { paths, commits } => {
                proven_resolutions.extend(paths.iter().cloned());
                append_unique(&mut superseded_paths, paths);
                append_unique(&mut superseding_commits, commits);
            }
            DeliveryContentPresence::Dropped { paths } => append_unique(&mut dropped_paths, paths),
            DeliveryContentPresence::Unknown { reason } => {
                unknown_reason.get_or_insert(reason);
            }
        }
    }
    for commit in &commits {
        match super::delivery_content_presence_in_parent(repo, commit, target) {
            DeliveryContentPresence::Present { paths } => append_unique(&mut present_paths, paths),
            DeliveryContentPresence::Superseded { paths, commits } => {
                append_unique(&mut superseded_paths, paths);
                append_unique(&mut superseding_commits, commits);
            }
            DeliveryContentPresence::Dropped { paths } => {
                for path in paths {
                    // A resolution may replace only the owned lines in its
                    // novel hunks, on a path whose final effect was proven.
                    // It never enters the later-commit list for other paths.
                    let proof = if proven_resolutions.contains(&path) {
                        let parent = git_text(repo, &["rev-parse", &format!("{commit}^1")])?;
                        delivery_evolution::line_content_presence_with_resolution(
                            repo,
                            &parent,
                            commit,
                            &target_ref,
                            &path,
                            Some(merge_tip),
                        )
                    } else {
                        Ok(None)
                    };
                    match proof {
                        Ok(Some(DeliveryContentPresence::Superseded { paths, commits })) => {
                            append_unique(&mut superseded_paths, paths);
                            append_unique(&mut superseding_commits, commits);
                        }
                        Ok(Some(DeliveryContentPresence::Present { paths })) => {
                            append_unique(&mut present_paths, paths)
                        }
                        Err(reason) => {
                            unknown_reason.get_or_insert(reason);
                        }
                        _ => append_unique(&mut dropped_paths, vec![path]),
                    }
                }
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

/// A task-owned merge may introduce a QA fix in its resolution itself.
/// Restrict its first-parent effect to paths changed on a contributing side,
/// and require a difference from that side too: merely importing the target
/// is not a resolution delivery. NUL paths preserve unusual filenames.
fn merge_resolution_paths(
    repo: &Path,
    merge_tip: &str,
    identity: &TaskCommitIdentity,
) -> Option<Vec<String>> {
    let message = git_text(repo, &["show", "-s", "--format=%B", merge_tip])?;
    let owned = identity.matches_known_commit(merge_tip)
        || identity
            .task_id
            .as_deref()
            .is_some_and(|id| message_references_task(&message, id));
    if !owned {
        return Some(vec![]);
    }
    let parents = git_text(repo, &["rev-list", "--parents", "-n", "1", merge_tip])?;
    let parents: Vec<_> = parents.split_whitespace().collect();
    if parents.len() != 3 {
        return Some(vec![]);
    }
    let remerge = git_text(
        repo,
        &[
            "show",
            "--remerge-diff",
            "--format=",
            "--name-only",
            "-z",
            merge_tip,
            "--",
        ],
    )?;
    let resolution_effect: HashSet<_> = remerge
        .split('\0')
        .filter(|path| !path.is_empty())
        .collect();
    let first = *parents.get(1)?;
    let first_effect = git_text(
        repo,
        &[
            "diff",
            "--name-only",
            "--no-renames",
            "-z",
            first,
            merge_tip,
            "--",
        ],
    )?;
    let first_effect: HashSet<_> = first_effect
        .split('\0')
        .filter(|path| !path.is_empty())
        .collect();
    let mut paths = Vec::new();
    for second in parents.iter().skip(2) {
        let base = git_text(repo, &["merge-base", first, second])?;
        let side_effect = git_text(
            repo,
            &[
                "diff",
                "--name-only",
                "--no-renames",
                "-z",
                &base,
                second,
                "--",
            ],
        )?;
        for path in side_effect.split('\0').filter(|path| !path.is_empty()) {
            if first_effect.contains(path)
                && resolution_effect.contains(path)
                && !git_diff_is_empty(repo, second, merge_tip, path)?
                && resolution_has_novel_lines(repo, first, second, merge_tip, path)?
                && !paths.iter().any(|known| known == path)
            {
                paths.push(path.to_string());
            }
        }
    }
    Some(paths)
}

fn resolution_has_novel_lines(
    repo: &Path,
    first: &str,
    second: &str,
    tip: &str,
    path: &str,
) -> Option<bool> {
    let mut parent_lines = HashSet::new();
    for parent in [first, second] {
        let output = Command::new("git")
            .args(["show", &format!("{parent}:{path}")])
            .current_dir(repo)
            .output()
            .ok()?;
        if output.status.success() {
            let contents = String::from_utf8(output.stdout).ok()?;
            parent_lines.extend(contents.lines().map(|line| line.trim().to_string()));
        }
    }
    let patch = git_text(
        repo,
        &[
            "diff",
            "--unified=0",
            "--no-renames",
            first,
            tip,
            "--",
            path,
        ],
    )?;
    Some(
        patch
            .lines()
            .filter(|line| !line.starts_with("+++"))
            .filter_map(|line| line.strip_prefix('+'))
            .map(str::trim)
            .any(|line| line.chars().any(|c| c.is_alphanumeric()) && !parent_lines.contains(line)),
    )
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
    fn revert_of_task_commit_rejects_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        let delivery = commit(repo, "copy.txt", "delivery\n", "cas-taskb: delivery");
        git(repo, &["revert", "--no-edit", &delivery]);
        git(repo, &["checkout", "main"]);
        commit(repo, "target.rs", "target();\n", "advance target");
        git(repo, &["checkout", "factory/worker"]);
        git(
            repo,
            &["merge", "--no-ff", "main", "-m", "cas-taskb: target sync"],
        );
        let tip = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "main"]);
        git(
            repo,
            &["merge", "--no-ff", "factory/worker", "-m", "integrate"],
        );
        let mut window = window();
        window.identity.known_commits.push(tip.clone());
        assert_eq!(
            merge_tip_content_presence(repo, "main", &tip, Some(&window), &window.identity, None),
            Some(DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            })
        );
    }

    #[test]
    fn merge_tip_cannot_supersede_non_resolution_path_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        commit(repo, "lost.rs", "delivered();\n", "cas-taskb: delivery");
        commit(repo, "qa.rs", "worker();\n", "cas-taskb: QA path");
        git(repo, &["checkout", "main"]);
        commit(repo, "qa.rs", "target();\n", "advance QA path");
        git(repo, &["checkout", "factory/worker"]);
        git(
            repo,
            &["merge", "--no-ff", "--no-commit", "-s", "ours", "main"],
        );
        std::fs::remove_file(repo.join("lost.rs")).unwrap();
        std::fs::write(repo.join("qa.rs"), "novel_resolution();\n").unwrap();
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-qm", "cas-taskb: QA resolution"]);
        let tip = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "main"]);
        git(
            repo,
            &["merge", "--no-ff", "factory/worker", "-m", "integrate"],
        );
        commit(
            repo,
            "lost.rs",
            "unrelated_later_edit();\n",
            "later main edit",
        );
        let mut window = window();
        window.identity.known_commits.push(tip.clone());
        assert_eq!(
            merge_resolution_paths(repo, &tip, &window.identity).unwrap(),
            vec!["qa.rs"]
        );
        assert_eq!(
            merge_tip_content_presence(repo, "main", &tip, Some(&window), &window.identity, None),
            Some(DeliveryContentPresence::Dropped {
                paths: vec!["lost.rs".into()]
            })
        );
    }

    #[test]
    fn clean_auto_merge_path_is_not_resolution_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        commit(repo, "copy.txt", "worker\nold\n", "unnamed worker change");
        git(repo, &["checkout", "main"]);
        commit(repo, "copy.txt", "old\ntarget\n", "unnamed target change");
        git(repo, &["checkout", "factory/worker"]);
        git(repo, &["merge", "--no-ff", "main", "-m", "cas-taskb: sync"]);
        let tip = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "main"]);
        git(
            repo,
            &["merge", "--no-ff", "factory/worker", "-m", "integrate"],
        );
        let identity = TaskCommitIdentity {
            task_id: Some("cas-taskb".into()),
            known_commits: vec![tip.clone()],
        };
        assert!(
            merge_resolution_paths(repo, &tip, &identity)
                .unwrap()
                .is_empty()
        );
        assert!(merge_tip_content_presence(repo, "main", &tip, None, &identity, None).is_none());
    }

    #[test]
    fn revert_of_edit_then_delete_rejects_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        let delivery = commit(repo, "copy.txt", "delivered\nkeep\n", "cas-taskb: delivery");
        git(repo, &["checkout", "main"]);
        git(
            repo,
            &["merge", "--no-ff", "factory/worker", "-m", "integrate"],
        );
        let edit = commit(repo, "copy.txt", "edited\nkeep\n", "edit delivery");
        git(repo, &["revert", "--no-edit", &edit]);
        commit(repo, "copy.txt", "keep\n", "delete without replacement");
        assert_eq!(
            delivery_content_presence_on_target(repo, &delivery, "main"),
            DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            }
        );
    }

    #[test]
    fn override_naming_dropping_merge_rejects_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        let delivery = commit(repo, "copy.txt", "delivery\n", "cas-taskb: delivery");
        git(repo, &["checkout", "main"]);
        git(
            repo,
            &[
                "merge",
                "--no-ff",
                "-s",
                "ours",
                "factory/worker",
                "-m",
                "drop delivery",
            ],
        );
        let dropping = git(repo, &["rev-parse", "HEAD"]);
        let review =
            format!("reviewed-drop: {dropping} -- should not count its second-parent diff");
        let error = validated_delivery_drop_review(
            repo,
            &delivery,
            "main",
            &["copy.txt".into()],
            Some(&review),
        )
        .unwrap_err();
        assert!(error.contains("does not touch"), "{error}");
    }

    #[test]
    fn deleting_one_duplicate_does_not_transfer_line_ownership_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        commit(
            repo,
            "copy.txt",
            "duplicate();\nseparator();\n",
            "baseline duplicate",
        );
        git(repo, &["branch", "-f", "main", "HEAD"]);
        let delivery = commit(
            repo,
            "copy.txt",
            "duplicate();\nseparator();\nduplicate();\n",
            "cas-taskb: second duplicate",
        );
        git(repo, &["checkout", "main"]);
        git(
            repo,
            &["merge", "--no-ff", "factory/worker", "-m", "integrate"],
        );
        commit(
            repo,
            "copy.txt",
            "duplicate();\nseparator();\n",
            "delete owned duplicate",
        );
        assert_eq!(
            delivery_content_presence_on_target(repo, &delivery, "main"),
            DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            }
        );
    }

    #[test]
    fn unlabeled_inverse_patch_cannot_certify_supersession_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        let delivery = commit(
            repo,
            "copy.txt",
            "modern();\n",
            "cas-taskb: change existing line",
        );
        git(repo, &["checkout", "main"]);
        git(
            repo,
            &["merge", "--no-ff", "factory/worker", "-m", "integrate"],
        );
        commit(
            repo,
            "copy.txt",
            "old\n",
            "restore baseline with an ordinary subject",
        );
        assert_eq!(
            delivery_content_presence_on_target(repo, &delivery, "main"),
            DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            }
        );
    }

    #[test]
    fn merge_resolution_itself_is_task_content_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        commit(repo, "work.rs", "legacy();\n", "cas-taskb: delivery");
        git(repo, &["checkout", "main"]);
        git(repo, &["merge", "--no-ff", "--no-commit", "factory/worker"]);
        let merge = commit(repo, "work.rs", "modern();\n", "cas-taskb: QA resolution");
        let mut window = window();
        window.identity.known_commits.push(merge.clone());
        assert_eq!(
            merge_tip_content_presence(repo, "main", &merge, Some(&window), &window.identity, None),
            Some(DeliveryContentPresence::Superseded {
                paths: vec!["work.rs".into()],
                commits: vec![merge.clone()],
            })
        );
    }

    #[test]
    fn the_delivery_base_is_the_first_parent_of_the_earliest_task_commit() {
        let dir = fixture();
        let p = dir.path();
        let parent = git(p, &["rev-parse", "HEAD"]);
        let tip = commit(p, "work.rs", "own\n", "cas-taskb: the delivery");

        let base = delivery_base(p, "main", &window(), Some(&tip))
            .expect("an attributable delivery must yield a base");
        assert_eq!(
            base, parent,
            "the base is the commit the delivery is measured against, not the delivery itself"
        );
    }

    #[test]
    fn the_delivery_base_survives_a_merge_into_the_target_before_close() {
        // cas-9c1e: the exact shape that made the close gate unsatisfiable.
        // `merge-base(HEAD, main)` becomes the delivery commit once the
        // supervisor merges, which would leave an empty proof surface.
        let dir = fixture();
        let p = dir.path();
        let parent = git(p, &["rev-parse", "HEAD"]);
        let tip = commit(p, "work.rs", "own\n", "cas-taskb: the delivery");

        git(p, &["checkout", "-q", "main"]);
        git(
            p,
            &[
                "merge",
                "-q",
                "--no-ff",
                "-m",
                "merge worker",
                "factory/worker",
            ],
        );
        git(p, &["checkout", "-q", "factory/worker"]);

        assert_eq!(
            git(p, &["merge-base", "HEAD", "main"]),
            tip,
            "precondition: the old merge-base rule would have returned the delivery commit"
        );
        assert_eq!(
            delivery_base(p, "main", &window(), Some(&tip)).as_deref(),
            Some(parent.as_str()),
            "the base must still be the delivery's own first parent after the merge"
        );
        assert!(
            !paths(p, "main", &window(), Some(&tip))
                .expect("paths must resolve")
                .is_empty(),
            "the proof surface must not be empty: that is what made the gate unsatisfiable"
        );
    }

    #[test]
    fn qa_attribution_ignores_incoming_merge_and_whitespace_only_vue_gh_1037() {
        let dir = fixture();
        let repo = dir.path();
        assert_eq!(qa_paths(repo, "main", &window(), None), None);
        git(repo, &["checkout", "-q", "main"]);
        commit(
            repo,
            "incoming.vue",
            "<template><p>Incoming</p></template>\n",
            "staging UI",
        );
        git(repo, &["checkout", "-q", "factory/worker"]);
        git(
            repo,
            &["merge", "-q", "--no-ff", "-m", "sync staging", "main"],
        );
        let merge_tip = git(repo, &["rev-parse", "HEAD"]);
        assert!(
            paths(repo, "main", &window(), Some(&merge_tip))
                .unwrap()
                .contains(&"incoming.vue".into())
        );
        assert_eq!(
            qa_paths(repo, "main", &window(), Some(&merge_tip)),
            Some(vec![])
        );

        commit(
            repo,
            "app.vue",
            "<template><p>Hello</p></template>\n",
            "initial UI",
        );
        let base = git(repo, &["rev-parse", "HEAD"]);
        std::fs::write(
            repo.join("app.vue"),
            "<template> <p>Hello</p> </template>\n",
        )
        .unwrap();
        git(repo, &["add", "app.vue"]);
        git(repo, &["commit", "-qm", "format only"]);
        let formatted = git(repo, &["rev-parse", "HEAD"]);
        assert!(
            crate::qa_pass::delivery_content_paths(repo, &base, &formatted)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn the_delivery_base_does_not_move_when_the_checkout_does() {
        // The second observed failure: with the worker idle, the gate inspected
        // a checkout parked on another branch and demanded proof over that
        // branch's whole diff. The base must depend on the delivery, not on
        // whatever HEAD happens to be.
        let dir = fixture();
        let p = dir.path();
        let parent = git(p, &["rev-parse", "HEAD"]);
        let tip = commit(p, "work.rs", "own\n", "cas-taskb: the delivery");
        let from_worker = delivery_base(p, "main", &window(), Some(&tip));

        git(p, &["checkout", "-q", "main"]);
        assert_eq!(
            delivery_base(p, "main", &window(), Some(&tip)),
            from_worker,
            "the same delivery must resolve the same base from any checkout"
        );
        assert_eq!(from_worker.as_deref(), Some(parent.as_str()));
    }

    #[test]
    fn a_multi_commit_unmerged_delivery_still_bases_on_its_branch_point() {
        // Guards the unchanged case: the base spans the whole delivery, not
        // just its last commit.
        let dir = fixture();
        let p = dir.path();
        let branch_point = git(p, &["rev-parse", "HEAD"]);
        commit(p, "first.rs", "one\n", "cas-taskb: first");
        let tip = commit(p, "second.rs", "two\n", "cas-taskb: second");

        assert_eq!(
            delivery_base(p, "main", &window(), Some(&tip)).as_deref(),
            Some(branch_point.as_str()),
            "an unmerged multi-commit delivery is proven from its branch point"
        );
        let changed = paths(p, "main", &window(), Some(&tip)).unwrap();
        assert!(
            changed.contains(&"first.rs".to_string()) && changed.contains(&"second.rs".to_string()),
            "both commits are in the surface: {changed:?}"
        );
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
