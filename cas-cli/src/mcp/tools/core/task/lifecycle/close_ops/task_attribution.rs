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

use crate::git_evidence::git_text;

fn references_foreign_task(message: &str, identity: &TaskCommitIdentity) -> bool {
    message
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .any(|word| {
            word.get(..4)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cas-"))
                && word.get(4..).is_some_and(|id| {
                    (4..=8).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                && identity.task_id.as_deref() != Some(word)
        })
}

/// cas-f38ca / cas-f2eb: the subject line names the task that owns a
/// commit. One whose subject claims another task, and not this one, is that
/// task's work even when its body mentions this task as context ("workers on
/// cas-940f base on this").
fn subject_claims_another_task(message: &str, identity: &TaskCommitIdentity) -> bool {
    let subject = message.trim().lines().next().unwrap_or("");
    !identity
        .task_id
        .as_deref()
        .is_some_and(|id| message_references_task(subject, id))
        && references_foreign_task(subject, identity)
}

/// GH #1151: whether `commit` is another task's work under the subject rule
/// above. An unreadable commit is not shown to be foreign.
pub(super) fn commit_claims_another_task(
    repo: &Path,
    commit: &str,
    identity: &TaskCommitIdentity,
) -> bool {
    is_safe_git_refname(commit)
        && git_text(repo, &["log", "-1", "--format=%B", commit, "--"])
            .is_some_and(|message| subject_claims_another_task(&message, identity))
}

/// GH #1133, #1151: whether every commit `lane` holds beyond `target` (and
/// beyond `origin/<target>` when that exists) claims another task. An unnamed
/// commit may be this task's own work (cas-2387), so it answers `false`.
/// `None` when Git cannot read the range or it is too large to judge.
pub(super) fn lane_commits_all_claim_other_tasks(
    repo: &Path,
    lane: &str,
    target: &str,
    identity: &TaskCommitIdentity,
) -> Option<bool> {
    const LIMIT: usize = 200;
    if !is_safe_git_refname(lane) || !is_safe_git_refname(target) {
        return None;
    }
    let limit = (LIMIT + 1).to_string();
    let mut args = vec![
        "log".to_string(),
        "--no-merges".to_string(),
        "-n".to_string(),
        limit,
        "--format=%H%x1f%B%x1e".to_string(),
        lane.to_string(),
        format!("^{target}"),
    ];
    let origin_target = format!("origin/{target}");
    if git_ref_exists(repo, &origin_target) {
        args.push(format!("^{origin_target}"));
    }
    args.push("--".to_string());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let history = git_text(repo, &args)?;
    let records: Vec<&str> = history
        .split('\u{1e}')
        .map(str::trim)
        .filter(|record| !record.is_empty())
        .collect();
    if records.len() > LIMIT {
        return None;
    }
    Some(records.iter().all(|record| {
        record
            .split_once('\u{1f}')
            .is_some_and(|(_, message)| subject_claims_another_task(message, identity))
    }))
}

/// cas-93db: what `branch`'s recent first-parent work claims, under the
/// subject rule above. Returns the first commit claiming this task, if any,
/// and the newest commit claiming only another task (its subject line), if
/// any, among the last 50 non-merge first-parent commits. `None` when Git
/// cannot read the branch.
pub(super) fn branch_task_claims(
    repo: &Path,
    branch: &str,
    identity: &TaskCommitIdentity,
) -> Option<(Option<String>, Option<String>)> {
    let history = git_text(
        repo,
        &[
            "log",
            "--first-parent",
            "--no-merges",
            "-n",
            "50",
            "--format=%H%x1f%B%x1e",
            branch,
            "--",
        ],
    )?;
    let mut own = None;
    let mut foreign = None;
    for record in history.split('\u{1e}') {
        let Some((sha, message)) = record.trim().split_once('\u{1f}') else {
            continue;
        };
        if subject_claims_another_task(message, identity) {
            if foreign.is_none() {
                let subject = message.trim().lines().next().unwrap_or("").trim();
                foreign = Some(format!("{} {subject}", &sha[..sha.len().min(9)]));
            }
        } else if own.is_none()
            && (identity.matches_known_commit(sha)
                || identity
                    .task_id
                    .as_deref()
                    .is_some_and(|id| message_references_task(message, id)))
        {
            own = Some(sha.to_string());
        }
    }
    Some((own, foreign))
}

/// cas-f2eb: a lane merge is task delivery only when everything it brings in
/// is this task's work. A merge that brings a commit claimed by another task,
/// or another lane's commit already on the target, imports someone else's
/// content: its first-parent diff is theirs. Unnamed commits off the target
/// stay attributable (a worker's own side branch). Unknowable Git state keeps
/// the merge, as before.
fn merge_brings_foreign_work(
    repo: &Path,
    first_parent: &str,
    merged_parents: &[String],
    target: &str,
    identity: &TaskCommitIdentity,
) -> bool {
    for parent in merged_parents {
        let exclude_first = format!("^{first_parent}");
        let exclude_target = format!("^{target}");
        let Some(brought) = git_text(
            repo,
            &[
                "log",
                "--no-merges",
                "--format=%H%x1f%B%x1e",
                parent,
                &exclude_first,
            ],
        ) else {
            return false;
        };
        let Some(off_target) = git_text(
            repo,
            &["rev-list", "--no-merges", parent, &exclude_first, &exclude_target],
        ) else {
            return false;
        };
        let off_target: HashSet<&str> = off_target.lines().collect();
        for record in brought.split('\u{1e}') {
            let Some((sha, message)) = record.trim().split_once('\u{1f}') else {
                continue;
            };
            if identity.matches_known_commit(sha) {
                continue;
            }
            if subject_claims_another_task(message, identity) {
                return true;
            }
            let named = identity
                .task_id
                .as_deref()
                .is_some_and(|id| message_references_task(message, id));
            if !named && !off_target.contains(sha) {
                return true;
            }
        }
    }
    false
}

fn is_target_sync_merge(repo: &Path, merged_parents: &[String], target: &str) -> bool {
    !merged_parents.is_empty()
        && merged_parents.iter().all(|parent| {
            Command::new("git")
                .args(["merge-base", "--is-ancestor", parent, target])
                .current_dir(repo)
                .status()
                .is_ok_and(|status| status.success())
        })
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
    task_delivery_selection(repo, parent, window, tip, true).map(|selection| selection.ranges)
}

/// The selected delivery ranges, plus whether a target-sync merge that would
/// otherwise have been selected was excluded (cas-2664 defect 7), or a merge
/// that brought another task's work (cas-f2eb). Empty
/// ranges with that flag set are a positive finding, "this task only brought
/// the target into its lane", not an attribution failure.
struct DeliverySelection {
    ranges: Vec<DeliveryRange>,
    excluded_target_sync: bool,
}

fn task_delivery_selection(
    repo: &Path,
    parent: &str,
    window: &TaskCommitReceiptWindow,
    tip: Option<&str>,
    include_unowned_unmerged: bool,
) -> Option<DeliverySelection> {
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
        /// Non-first parents of a merge commit.
        merged_parents: Vec<String>,
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
                || (window
                    .identity
                    .task_id
                    .as_deref()
                    .is_some_and(|id| message_references_task(message, id))
                    && !subject_claims_another_task(message, &window.identity));
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
            let parents: Vec<&str> = fields[1].split_whitespace().collect();
            Some(Commit {
                sha,
                parent: parents.first().copied().unwrap_or("").into(),
                epoch: fields[2].parse().ok()?,
                owned,
                foreign,
                merge: parents.len() > 1,
                merged_parents: parents.iter().skip(1).map(|parent| parent.to_string()).collect(),
            })
        })
        .collect();
    let floor = window
        .task_floor
        .timestamp()
        .saturating_sub(COMMIT_RECEIPT_CLOCK_SKEW_SECS);
    let mut excluded_target_sync = false;
    let mut selected: Vec<bool> = commits
        .iter()
        .map(|c| {
            let candidate = !c.parent.is_empty()
                && !c.foreign
                && (c.owned || (include_unowned_unmerged && unmerged.contains(&c.sha)))
                && (in_work_window(window, c.epoch, c.owned) || (historical_receipt && c.owned));
            // cas-2664 (7): a merge whose every non-first parent is already
            // on the target only brings the target into the lane; its tree
            // effect is other tasks' delivered content. Deselecting it also
            // splits the ranges, so no range spans the target content it
            // brought in. Only otherwise-selected merges cost a Git call.
            // cas-f2eb: likewise a merge that brings another task's work
            // (an epic tip the target does not hold yet, or main) imports
            // that work; only the task's own commits around it are delivery.
            if candidate
                && c.merge
                && (is_target_sync_merge(repo, &c.merged_parents, &target)
                    || merge_brings_foreign_work(
                        repo,
                        &c.parent,
                        &c.merged_parents,
                        &target,
                        &window.identity,
                    ))
            {
                excluded_target_sync = true;
                return false;
            }
            candidate
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
    Some(DeliverySelection {
        ranges,
        excluded_target_sync,
    })
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
    paths_from_ranges(repo, ranges)
}

/// cas-2d27: without a task delivery tip, a no-code supervisor close must
/// not adopt unclaimed work from the closing checkout merely because it is
/// unmerged and inside the task's clock window. Named/recorded task commits
/// remain visible, so a no-code declaration cannot erase real delivery.
pub(super) fn identified_paths(
    repo: &Path,
    target: &str,
    window: &TaskCommitReceiptWindow,
) -> Option<Vec<String>> {
    let selection = task_delivery_selection(repo, target, window, None, false)?;
    paths_from_ranges(repo, selection.ranges)
}

fn paths_from_ranges(repo: &Path, ranges: Vec<DeliveryRange>) -> Option<Vec<String>> {
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
    let selection = task_delivery_selection(repo, target, window, receipt.as_deref(), true)?;
    let ranges = selection.ranges;
    if ranges.is_empty() {
        // GH #1037 / cas-2664: a task whose only candidate commit was a
        // target-sync merge delivered no reviewable content of its own. That
        // is an authoritative empty set; `None` would make the QA gate fall
        // back to the lane's whole range and charge the incoming files.
        return selection.excluded_target_sync.then(Vec::new);
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

/// Shared candidate selection for whole-delivery and final-snapshot proof.
/// Preserve side-parent identity, foreign-subject and work-window rules.
fn attributed_content_commits(
    repo: &Path,
    target: &str,
    tip: &str,
    window: &TaskCommitReceiptWindow,
    identity: &TaskCommitIdentity,
) -> Option<Vec<String>> {
    let ranges = task_delivery_ranges(repo, target, window, Some(tip))?;

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
        args.push(tip.into());
        let history = git_text(repo, &args.iter().map(String::as_str).collect::<Vec<_>>())?;
        for record in history.split('\u{1e}') {
            let fields: Vec<_> = record.trim().splitn(3, '\u{1f}').collect();
            if fields.len() != 3
                || !message_references_task(fields[2], id)
                || delivery_evolution::is_revert_message(fields[2])
            {
                continue;
            }
            // cas-f38ca: another lane's commit can mention this task as
            // context ("dep_add no longer strands cas-940f") and reach the
            // tip through a merge of the epic. Its subject names the task
            // that owns it; a body mention does not make its lines ours.
            if !identity.matches_known_commit(fields[0])
                && subject_claims_another_task(fields[2], identity)
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

    Some(commits)
}

/// Exact snapshot recovery needs a surviving task-attributed path effect,
/// including side-parent authors, not an imported baseline. Delivery ranges
/// split at target-sync/foreign merges;
/// record their path baselines before crediting ordinary task-authored blobs.
/// Returning to any imported baseline removes the task effect even when that
/// baseline differs from the task's earliest spawn tree (cas-5f0b).
/// `None` is undecidable Git evidence and never authorizes recovery.
pub(super) fn final_path_snapshot_proven(
    repo: &Path,
    target: &str,
    window: &TaskCommitReceiptWindow,
    tip: &str,
    measured_target: &str,
    path: &str,
) -> Option<bool> {
    let snapshot = tree_path_blob(repo, tip, path)?;
    let ranges = task_delivery_ranges(repo, target, window, Some(tip))?;
    let mut authored_blobs = Vec::new();
    let mut imported_baselines = Vec::new();
    for range in ranges {
        let history = git_text(repo, &["rev-list", "--first-parent", "--reverse", &format!("{}..{}", range.base, range.tip)])?;
        let mut path_commits = Vec::new();
        for commit in history.lines().filter(|commit| !commit.is_empty()) {
            if super::commit_changes_path(repo, commit, path).ok()? {
                path_commits.push(commit);
            }
        }
        if path_commits.is_empty() {
            continue;
        }
        let baseline = tree_path_blob(repo, &range.base, path)?;
        if !authored_blobs.contains(&baseline) && !imported_baselines.contains(&baseline) {
            imported_baselines.push(baseline);
        }
        for commit in path_commits {
            let message = git_text(repo, &["show", "-s", "--format=%B", commit])?;
            let ordinary_authored = !is_merge_commit(repo, commit)
                && !delivery_evolution::is_revert_message(&message);
            let blob = tree_path_blob(repo, commit, path)?;
            // Imported/reverted states never become task-owned baselines.
            if ordinary_authored && !imported_baselines.contains(&blob) && !authored_blobs.contains(&blob) {
                authored_blobs.push(blob.clone());
            }
        }
    }
    if imported_baselines.contains(&snapshot) {
        return Some(false);
    }
    for commit in attributed_content_commits(repo, target, tip, window, &window.identity)? {
        let message = git_text(repo, &["show", "-s", "--format=%B", &commit])?;
        if delivery_evolution::is_revert_message(&message)
            || !super::commit_changes_path(repo, &commit, path).ok()? {
            continue;
        }
        let parent = git_text(repo, &["rev-parse", &format!("{commit}^1")])?;
        match delivery_evolution::surviving_line_content(repo, &parent, &commit, measured_target, path).ok()? {
            Some(DeliveryContentPresence::Present { .. } | DeliveryContentPresence::Superseded { .. }) => return Some(true),
            None => {
                // Binary/deletion-only effects use the existing reverse-patch
                // proof, still bound to an attributed commit and final tree.
                if tree_path_blob(repo, &commit, path)? == snapshot
                    && super::reverse_delivery_path_applies_to_tree(repo, &parent, &commit, measured_target, path).ok()? {
                    return Some(true);
                }
            }
            _ => {},
        }
    }
    Some(false)
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
    let first_parent_commits = git_text(
        repo,
        &["rev-list", "--first-parent", "--reverse", merge_tip],
    )?;
    let first_parent_commits = first_parent_commits
        .lines()
        .filter(|commit| !commit.is_empty())
        .collect::<Vec<_>>();

    let mut commits = attributed_content_commits(repo, target, merge_tip, window, identity)?;

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
    let commits: Vec<String> = order
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
    // A worker may resolve a conflict before its final target-sync merge.
    // Bind unnamed earlier resolutions to selected task content on their
    // first-parent delivery side, rather than granting every merge ownership.
    let mut resolutions = Vec::new();
    let delivery_history: HashSet<_> = first_parent_commits
        .iter()
        .copied()
        .skip_while(|prior| !commits.iter().any(|owned| owned.as_str() == *prior))
        .collect();
    let merge_history = git_text(
        repo,
        &[
            "log",
            "--first-parent",
            "--merges",
            "--reverse",
            "--format=%H%x1f%P%x1f%ct%x1f%B%x1e",
            merge_tip,
        ],
    )?;
    for record in merge_history.split('\u{1e}') {
        let fields: Vec<_> = record.trim().splitn(4, '\u{1f}').collect();
        if fields.len() != 4 {
            continue;
        }
        let resolution = fields[0];
        let parents: Vec<_> = fields[1].split_whitespace().collect();
        let message = fields[3];
        let owned = identity.matches_known_commit(resolution)
            || identity
                .task_id
                .as_deref()
                .is_some_and(|id| message_references_task(message, id));
        let epoch = fields[2].parse::<i64>().ok()?;
        if parents.len() != 2
            || (!owned
                && (!delivery_history.contains(resolution)
                    || !in_work_window(window, epoch, false)))
            || delivery_evolution::is_revert_message(message)
            // An explicitly owned QA receipt can discuss other tasks whose
            // content it integrates. Foreign references only disqualify a
            // resolution inferred from an otherwise unnamed work window.
            || (!owned && references_foreign_task(message, identity))
        {
            continue;
        }
        let mut resolution_identity = identity.clone();
        if !owned {
            let base = git_text(repo, &["merge-base", parents[0], parents[1]])?;
            let delivery_side = git_text(
                repo,
                &[
                    "rev-list",
                    "--first-parent",
                    &format!("{base}..{}", parents[0]),
                ],
            )?;
            if delivery_side
                .lines()
                .any(|prior| commits.iter().any(|owned| owned == prior))
            {
                resolution_identity
                    .known_commits
                    .push(resolution.to_string());
            } else {
                continue;
            }
        }
        let paths = merge_resolution_paths(repo, resolution, &resolution_identity)?;
        if !paths.is_empty() {
            resolutions.push((resolution.to_string(), paths));
        }
    }
    // This task's own first-parent cycle ends at the handoff. Contextual
    // mentions do not disown explicit receipts; unnamed foreign work and
    // every revert remain outside the retirement window.
    let mut cycle = Vec::new();
    for commit in first_parent_commits
        .iter()
        .skip_while(|commit| !commits.iter().any(|owned| owned.as_str() == **commit))
    {
        let message = git_text(repo, &["show", "-s", "--format=%B", commit])?;
        let owned = identity.matches_known_commit(commit)
            || identity
                .task_id
                .as_deref()
                .is_some_and(|id| message_references_task(&message, id));
        let epoch = git_text(repo, &["show", "-s", "--format=%ct", commit])?
            .parse::<i64>()
            .ok()?;
        if !delivery_evolution::is_revert_message(&message)
            && (owned || !references_foreign_task(&message, identity))
            && in_work_window(window, epoch, owned)
        {
            cycle.push(commit.to_string());
        }
    }
    if commits.is_empty() && resolutions.is_empty() {
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
    let mut proven_resolutions = Vec::new();
    let carries_source = std::cell::OnceCell::new();
    // Mixed proofs list only evolved paths in Superseded.paths. Measure
    // independently to preserve authorization for every Present path.
    let mut plan = Vec::new();
    for (resolution, resolution_paths) in resolutions {
        for path in resolution_paths {
            // cas-bdd2: as in the later-commit loop (cas-24d8), a regenerable
            // artifact is left to cas-baf7's rule instead of walking a
            // minified bundle through every rebuilt epic commit. In the
            // cas-cee5 replay this loop spent 295 bundle diffs and 77 blames
            // reaching the same "dropped" the rule decides on.
            let artifact =
                super::artifact_left_to_regeneration_rule(repo, merge_tip, &path, &carries_source);
            plan.push((resolution.clone(), path, artifact));
        }
    }
    // cas-bdd2: each path's proof is independent; measure them concurrently
    // and decide in the original order.
    let proofs = super::measure_in_parallel(&plan, |(resolution, path, artifact)| {
        (!artifact).then(|| {
            super::delivery_content_presence_in_parent_for_paths(
                repo,
                resolution,
                target,
                Some(std::slice::from_ref(path)),
                true,
            )
        })
    });
    for ((resolution, path, _), proof) in plan.into_iter().zip(proofs) {
        match proof {
            None => append_unique(&mut dropped_paths, vec![path]),
            Some(DeliveryContentPresence::Present { paths }) => {
                proven_resolutions.push((resolution.clone(), paths.clone()));
                append_unique(&mut present_paths, paths);
            }
            Some(DeliveryContentPresence::Superseded { paths, commits }) => {
                proven_resolutions.push((resolution.clone(), paths.clone()));
                append_unique(&mut superseded_paths, paths);
                append_unique(&mut superseding_commits, commits);
            }
            Some(DeliveryContentPresence::Dropped { paths }) => {
                append_unique(&mut dropped_paths, paths)
            }
            Some(DeliveryContentPresence::Unknown { reason }) => {
                unknown_reason.get_or_insert(reason);
            }
        }
    }
    for commit in &commits {
        // cas-bdd2: the commit's rebuilt bundles are left to the regeneration
        // rule below without first walking them through the epic history.
        match super::delivery_content_presence_in_parent_leaving_artifacts(
            repo,
            commit,
            target,
            merge_tip,
            &carries_source,
        ) {
            DeliveryContentPresence::Present { paths } => append_unique(&mut present_paths, paths),
            DeliveryContentPresence::Superseded { paths, commits } => {
                append_unique(&mut superseded_paths, paths);
                append_unique(&mut superseding_commits, commits);
            }
            DeliveryContentPresence::Dropped { paths } => {
                for path in paths {
                    if super::artifact_left_to_regeneration_rule(
                        repo,
                        merge_tip,
                        &path,
                        &carries_source,
                    ) {
                        append_unique(&mut dropped_paths, vec![path]);
                        continue;
                    }
                    // A resolution may replace only the owned lines in its
                    // novel hunks, on a path whose final effect was proven.
                    // It never enters the later-commit list for other paths.
                    let authorized: Vec<_> = proven_resolutions
                        .iter()
                        .filter(|(_, paths)| paths.contains(&path))
                        .map(|(resolution, _)| resolution.clone())
                        .collect();
                    // Retirement alone is never evidence. Bind it to this
                    // path's final task-authored handoff content, measured on
                    // target before replaying any obsolete draft owner.
                    let final_resolution_proven =
                        proven_resolutions.iter().any(|(resolution, paths)| {
                            resolution == merge_tip && paths.contains(&path)
                        });
                    let mut final_ordinary_proven = false;
                    if !final_resolution_proven {
                        for later in commits.iter().rev().filter(|later| cycle.contains(later)) {
                            let parent = git_text(repo, &["rev-parse", &format!("{later}^1")])?;
                            if git_diff_is_empty(repo, &parent, later, &path)? {
                                continue;
                            }
                            final_ordinary_proven = matches!(
                                delivery_evolution::line_content_presence(
                                    repo,
                                    &parent,
                                    later,
                                    &target_ref,
                                    &path
                                ),
                                Ok(Some(
                                    DeliveryContentPresence::Present { .. }
                                        | DeliveryContentPresence::Superseded { .. }
                                ))
                            );
                            break;
                        }
                    }
                    let proof = if !authorized.is_empty() || final_ordinary_proven {
                        let parent = git_text(repo, &["rev-parse", &format!("{commit}^1")])?;
                        if (final_resolution_proven || final_ordinary_proven)
                            && cycle.contains(commit)
                        {
                            delivery_evolution::line_content_presence_with_task_cycle(
                                repo,
                                &parent,
                                commit,
                                &target_ref,
                                &path,
                                &authorized,
                                &cycle,
                            )
                        } else {
                            delivery_evolution::line_content_presence_with_resolutions(
                                repo,
                                &parent,
                                commit,
                                &target_ref,
                                &path,
                                &authorized,
                            )
                        }
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

/// A non-merge receipt can be integrated with a later task-owned QA
/// resolution. Credit each path independently; a foreign resolution cannot
/// enter this list and still requires the reviewed-drop exit.
pub(super) fn ordinary_anchor_content_presence(
    repo: &Path,
    target: &str,
    anchor: &str,
    identity: &TaskCommitIdentity,
) -> DeliveryContentPresence {
    // cas-bdd2: as in the per-path loop below, a regenerable artifact is left
    // to cas-baf7's rule instead of being walked through the epic history.
    let carries_source = std::cell::OnceCell::new();
    let original = super::delivery_content_presence_in_parent_leaving_artifacts(
        repo,
        anchor,
        target,
        anchor,
        &carries_source,
    );
    let DeliveryContentPresence::Dropped { paths } = &original else {
        return original;
    };
    let target_ref = preferred_diff_target_ref(repo, target);
    let inspect = || -> Option<DeliveryContentPresence> {
        let anchor = git_text(repo, &["rev-parse", &format!("{anchor}^{{commit}}")])?;
        let history = git_text(
            repo,
            &[
                "log",
                "--ancestry-path",
                "--merges",
                "--reverse",
                "--format=%H%x1f%B%x1e",
                &format!("{anchor}..{target_ref}"),
            ],
        )?;
        let mut resolutions = Vec::new();
        for record in history.split('\u{1e}') {
            let Some((commit, message)) = record.trim().split_once('\u{1f}') else {
                continue;
            };
            let owned = identity.matches_known_commit(commit)
                || identity
                    .task_id
                    .as_deref()
                    .is_some_and(|id| message_references_task(message, id));
            if !owned || delivery_evolution::is_revert_message(message) {
                continue;
            }
            for path in merge_resolution_paths(repo, commit, identity)? {
                if !paths.contains(&path) {
                    continue;
                }
                let selected = vec![path.clone()];
                if matches!(
                    super::delivery_content_presence_in_parent_for_paths(
                        repo,
                        commit,
                        target,
                        Some(&selected),
                        true
                    ),
                    DeliveryContentPresence::Present { .. }
                        | DeliveryContentPresence::Superseded { .. }
                ) {
                    resolutions.push((commit.to_string(), path));
                }
            }
        }
        let parent = git_text(repo, &["rev-parse", &format!("{anchor}^1")])?;
        let mut dropped = Vec::new();
        let mut proven_paths = Vec::new();
        let mut commits = Vec::new();
        let mut plan = Vec::new();
        for path in paths {
            let artifact =
                super::artifact_left_to_regeneration_rule(repo, &anchor, path, &carries_source);
            let authorized: Vec<_> = resolutions
                .iter()
                .filter(|(_, resolved)| resolved == path)
                .map(|(commit, _)| commit.clone())
                .collect();
            plan.push((path.clone(), authorized, artifact));
        }
        // cas-bdd2: one ownership walk per path, each independent of the
        // others; run them concurrently, then decide in the original order.
        let walks = super::measure_in_parallel(&plan, |(path, authorized, artifact)| {
            (!artifact).then(|| {
                delivery_evolution::line_content_presence_with_resolutions(
                    repo,
                    &parent,
                    &anchor,
                    &target_ref,
                    path,
                    authorized,
                )
            })
        });
        for ((path, _, _), walk) in plan.into_iter().zip(walks) {
            let Some(walk) = walk else {
                dropped.push(path);
                continue;
            };
            match walk.ok()? {
                Some(DeliveryContentPresence::Superseded {
                    paths,
                    commits: authors,
                }) => {
                    append_unique(&mut proven_paths, paths);
                    append_unique(&mut commits, authors);
                }
                Some(DeliveryContentPresence::Present { paths }) => {
                    append_unique(&mut proven_paths, paths)
                }
                _ => dropped.push(path),
            }
        }
        Some(if dropped.is_empty() {
            DeliveryContentPresence::Superseded {
                paths: proven_paths,
                commits,
            }
        } else {
            DeliveryContentPresence::Dropped { paths: dropped }
        })
    };
    inspect().unwrap_or(original)
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

    fn parallel_merge_fixture(
        base: &str,
        delivered: &str,
        sibling: &str,
        merged: &str,
    ) -> (tempfile::TempDir, String) {
        let dir = fixture();
        let repo = dir.path();
        commit(repo, "copy.txt", base, "list baseline");
        git(repo, &["branch", "-f", "main", "HEAD"]);
        let delivery = commit(repo, "copy.txt", delivered, "cas-taskb: delivery list");
        git(repo, &["checkout", "main"]);
        commit(repo, "copy.txt", sibling, "sibling list");
        git(
            repo,
            &[
                "merge",
                "--no-ff",
                "--no-commit",
                "-s",
                "ours",
                "factory/worker",
            ],
        );
        commit(repo, "copy.txt", merged, "integrate list resolution");
        (dir, delivery)
    }

    fn draft_handoff_fixture(
        delete_draft: bool,
        foreign_delete: bool,
        lose_after: bool,
        restore_baseline: bool,
    ) -> (tempfile::TempDir, String) {
        let dir = fixture();
        let repo = dir.path();
        commit(repo, "copy.txt", "base();\nkept();\n", "baseline");
        git(repo, &["branch", "-f", "main", "HEAD"]);
        commit(
            repo,
            "copy.txt",
            "draft();\nkept();\n",
            "cas-taskb: initial draft",
        );
        if delete_draft {
            commit(
                repo,
                "copy.txt",
                "kept();\n",
                if foreign_delete {
                    "cas-aaaa: remove foreign work"
                } else {
                    "cas-taskb: retire obsolete draft"
                },
            );
        }
        commit(
            repo,
            "copy.txt",
            "final();\nkept();\n",
            "cas-taskb: revised delivery",
        );
        git(repo, &["checkout", "main"]);
        commit(repo, "copy.txt", "target();\nkept();\n", "target work");
        git(repo, &["checkout", "factory/worker"]);
        git(
            repo,
            &["merge", "--no-ff", "--no-commit", "-s", "ours", "main"],
        );
        let handoff = commit(
            repo,
            "copy.txt",
            if restore_baseline {
                "base();\nnovel_neighbor();\nkept();\n"
            } else {
                "handoff();\nkept();\n"
            },
            "cas-taskb: QA resolution",
        );
        git(repo, &["checkout", "main"]);
        if lose_after {
            git(
                repo,
                &[
                    "merge",
                    "--no-ff",
                    "-s",
                    "ours",
                    "factory/worker",
                    "-m",
                    "lose handoff",
                ],
            );
        } else {
            git(
                repo,
                &["merge", "--no-ff", "factory/worker", "-m", "integrate"],
            );
        }
        (dir, handoff)
    }

    #[test]
    fn task_internal_replacement_proves_final_handoff_cas_0930() {
        let (dir, handoff) = draft_handoff_fixture(false, false, false, false);
        let mut window = window();
        window.identity.known_commits.push(handoff.clone());
        assert!(
            matches!(merge_tip_content_presence(dir.path(), "main", &handoff,
            Some(&window), &window.identity, None), Some(DeliveryContentPresence::Superseded { commits, .. })
            if commits.contains(&handoff))
        );
    }

    fn mixed_handoff_fixture(drop_present_path: bool) -> (tempfile::TempDir, String, String) {
        let dir = fixture();
        let repo = dir.path();
        commit(repo, "copy.txt", "base();\n", "baseline copy");
        commit(repo, "other.txt", "base_other();\n", "baseline other");
        git(repo, &["branch", "-f", "main", "HEAD"]);
        commit(repo, "copy.txt", "draft();\n", "cas-taskb: draft copy");
        commit(
            repo,
            "other.txt",
            "draft_other();\n",
            "cas-taskb: draft other",
        );
        git(repo, &["checkout", "main"]);
        commit(repo, "copy.txt", "target();\n", "target copy");
        commit(repo, "other.txt", "target_other();\n", "target other");
        git(repo, &["checkout", "factory/worker"]);
        git(
            repo,
            &["merge", "--no-ff", "--no-commit", "-s", "ours", "main"],
        );
        std::fs::write(repo.join("copy.txt"), "handoff();\n").unwrap();
        git(repo, &["add", "copy.txt"]);
        let handoff = commit(
            repo,
            "other.txt",
            "handoff_other();\n",
            "cas-taskb: QA resolution",
        );
        git(repo, &["checkout", "main"]);
        git(
            repo,
            &["merge", "--no-ff", "factory/worker", "-m", "integrate"],
        );
        let evolution = commit(repo, "other.txt", "evolved_other();\n", "evolve other");
        if drop_present_path {
            commit(repo, "copy.txt", "", "delete copy after handoff");
        }
        (dir, handoff, evolution)
    }

    #[test]
    fn mixed_resolution_preserves_present_path_authorization_cas_0930() {
        let (dir, handoff, evolution) = mixed_handoff_fixture(false);
        let mut window = window();
        window.identity.known_commits.push(handoff.clone());
        let measured = merge_tip_content_presence(
            dir.path(),
            "main",
            &handoff,
            Some(&window),
            &window.identity,
            None,
        );
        assert!(
            matches!(&measured, Some(DeliveryContentPresence::Superseded { paths, commits })
            if paths.contains(&"copy.txt".to_string()) && paths.contains(&"other.txt".to_string())
            && commits.contains(&handoff) && commits.contains(&evolution)),
            "{measured:?}"
        );
    }

    #[test]
    fn mixed_resolution_cannot_authorize_dropped_neighbour_cas_0930() {
        let (dir, handoff, _) = mixed_handoff_fixture(true);
        let mut window = window();
        window.identity.known_commits.push(handoff.clone());
        assert_eq!(
            merge_tip_content_presence(
                dir.path(),
                "main",
                &handoff,
                Some(&window),
                &window.identity,
                None
            ),
            Some(DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()],
            })
        );
    }

    #[test]
    fn deletion_of_baseline_is_not_retired_draft_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        commit(
            repo,
            "copy.txt",
            "base();\nold_baseline();\nkept();\n",
            "baseline",
        );
        git(repo, &["branch", "-f", "main", "HEAD"]);
        commit(
            repo,
            "copy.txt",
            "draft();\nold_baseline();\nkept();\n",
            "cas-taskb: draft",
        );
        commit(
            repo,
            "copy.txt",
            "draft();\nkept();\n",
            "cas-taskb: delete baseline",
        );
        git(repo, &["checkout", "main"]);
        commit(
            repo,
            "copy.txt",
            "target();\nold_baseline();\nkept();\n",
            "target",
        );
        git(repo, &["checkout", "factory/worker"]);
        git(
            repo,
            &["merge", "--no-ff", "--no-commit", "-s", "ours", "main"],
        );
        let handoff = commit(
            repo,
            "copy.txt",
            "handoff();\nkept();\n",
            "cas-taskb: QA resolution",
        );
        git(repo, &["checkout", "main"]);
        git(
            repo,
            &["merge", "--no-ff", "factory/worker", "-m", "integrate"],
        );
        commit(
            repo,
            "copy.txt",
            "handoff();\nold_baseline();\nkept();\n",
            "restore deleted baseline",
        );
        let mut window = window();
        window.identity.known_commits.push(handoff.clone());
        assert_eq!(
            merge_tip_content_presence(
                repo,
                "main",
                &handoff,
                Some(&window),
                &window.identity,
                None
            ),
            Some(DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()],
            })
        );
    }

    #[test]
    fn retired_draft_requires_surviving_handoff_cas_0930() {
        let (dir, handoff) = draft_handoff_fixture(true, false, false, false);
        let mut window = window();
        window.identity.known_commits.push(handoff.clone());
        let retirement = git(dir.path(), &["rev-parse", &format!("{handoff}^1^1")]);
        let observed = merge_tip_content_presence(
            dir.path(),
            "main",
            &handoff,
            Some(&window),
            &window.identity,
            None,
        );
        assert!(
            matches!(&observed, Some(DeliveryContentPresence::Superseded { commits, .. })
            if commits.contains(&retirement) && commits.contains(&handoff)),
            "retirement={retirement} handoff={handoff} observed={observed:?}"
        );
    }

    #[test]
    fn post_handoff_merge_loss_cannot_retire_draft_cas_0930() {
        let (dir, handoff) = draft_handoff_fixture(false, false, true, false);
        let mut window = window();
        window.identity.known_commits.push(handoff.clone());
        assert_eq!(
            merge_tip_content_presence(
                dir.path(),
                "main",
                &handoff,
                Some(&window),
                &window.identity,
                None
            ),
            Some(DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            })
        );
    }

    #[test]
    fn draft_retirement_cannot_prove_missing_final_path_cas_0930() {
        let (dir, handoff) = draft_handoff_fixture(true, false, true, false);
        let mut window = window();
        window.identity.known_commits.push(handoff.clone());
        assert_eq!(
            merge_tip_content_presence(
                dir.path(),
                "main",
                &handoff,
                Some(&window),
                &window.identity,
                None
            ),
            Some(DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            })
        );
    }

    #[test]
    fn foreign_pre_handoff_deletion_cannot_retire_owner_cas_0930() {
        let (dir, handoff) = draft_handoff_fixture(true, true, false, false);
        let mut window = window();
        window.identity.known_commits.push(handoff.clone());
        assert_eq!(
            merge_tip_content_presence(
                dir.path(),
                "main",
                &handoff,
                Some(&window),
                &window.identity,
                None
            ),
            Some(DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            })
        );
    }

    #[test]
    fn retired_draft_baseline_restore_with_novel_neighbor_rejects_cas_0930() {
        let (dir, handoff) = draft_handoff_fixture(true, false, false, true);
        let mut window = window();
        window.identity.known_commits.push(handoff.clone());
        assert_eq!(
            merge_tip_content_presence(
                dir.path(),
                "main",
                &handoff,
                Some(&window),
                &window.identity,
                None
            ),
            Some(DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            })
        );
    }

    #[test]
    fn ordinary_anchor_gets_task_owned_integration_resolution_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        commit(repo, "copy.txt", "6. original item\n", "baseline");
        git(repo, &["branch", "-f", "main", "HEAD"]);
        let delivery = commit(repo, "copy.txt", "6. delivered item\n", "cas-taskb: docs");
        git(repo, &["checkout", "main"]);
        commit(repo, "copy.txt", "6. other item\n", "target item");
        git(
            repo,
            &[
                "merge",
                "--no-ff",
                "--no-commit",
                "-s",
                "ours",
                "factory/worker",
            ],
        );
        let integration = commit(
            repo,
            "copy.txt",
            "6. other item\n7. delivered item\n",
            "cas-taskb: integrate docs",
        );
        assert_eq!(
            ordinary_anchor_content_presence(repo, "main", &delivery, &window().identity),
            DeliveryContentPresence::Superseded {
                paths: vec!["copy.txt".into()],
                commits: vec![integration]
            }
        );
    }

    #[test]
    fn parallel_ordinary_edit_records_actual_side_commit_cas_0930() {
        let (dir, delivery) = parallel_merge_fixture(
            "render(title, disabled);\n",
            "render(delivered_title, disabled);\n",
            "render(title, described);\n",
            "render(delivered_title, described);\n",
        );
        let side = git(dir.path(), &["rev-parse", "main^1"]);
        assert_eq!(
            delivery_content_presence_on_target(dir.path(), &delivery, "main"),
            DeliveryContentPresence::Superseded {
                paths: vec!["copy.txt".into()],
                commits: vec![side]
            }
        );
    }

    #[test]
    fn conflicting_parallel_edits_cannot_certify_merge_cas_0930() {
        let (dir, delivery) = parallel_merge_fixture(
            "render(title, disabled);\n",
            "render(delivered_title, disabled);\n",
            "render(replaced_title, disabled);\n",
            "render(delivered_title, described);\n",
        );
        assert_eq!(
            delivery_content_presence_on_target(dir.path(), &delivery, "main"),
            DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            }
        );
    }

    #[test]
    fn parallel_import_union_preserves_every_parent_name_cas_0930() {
        let (dir, delivery) = parallel_merge_fixture(
            "import { Alpha, Beta } from \"./api\";\n",
            "import { Beta, Delivery, Alpha } from \"./api\";\n",
            "import { Alpha, Side, Beta } from \"./api\";\n",
            "import { Beta, Delivery, Side, Alpha } from \"./api\";\n",
        );
        let side = git(dir.path(), &["rev-parse", "main^1"]);
        assert_eq!(
            delivery_content_presence_on_target(dir.path(), &delivery, "main"),
            DeliveryContentPresence::Superseded {
                paths: vec!["copy.txt".into()],
                commits: vec![side]
            }
        );
    }

    #[test]
    fn parallel_import_union_cannot_restore_removed_name_cas_0930() {
        let (dir, delivery) = parallel_merge_fixture(
            "import { Alpha, ADMIN, Beta } from \"./api\";\n",
            "import { Alpha, Delivery, Beta } from \"./api\";\n",
            "import { Alpha, ADMIN, Side, Beta } from \"./api\";\n",
            "import { Alpha, Delivery, ADMIN, Side, Beta } from \"./api\";\n",
        );
        assert_eq!(
            delivery_content_presence_on_target(dir.path(), &delivery, "main"),
            DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            }
        );
    }

    #[test]
    fn parallel_edit_cannot_restore_removed_privilege_cas_0930() {
        let (dir, delivery) = parallel_merge_fixture(
            "grant(user, ADMIN); render(title);\n",
            "grant(user); render(title);\n",
            "grant(user, ADMIN); render(described);\n",
            "grant(user, ADMIN); render(described);\n",
        );
        assert_eq!(
            delivery_content_presence_on_target(dir.path(), &delivery, "main"),
            DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            }
        );
    }

    #[test]
    fn parallel_edit_requires_final_survival_cas_0930() {
        let (dir, delivery) = parallel_merge_fixture(
            "render(title, disabled);\n",
            "render(delivered_title, disabled);\n",
            "render(title, described);\n",
            "render(delivered_title, described);\n",
        );
        commit(dir.path(), "copy.txt", "", "delete combined delivery");
        assert_eq!(
            delivery_content_presence_on_target(dir.path(), &delivery, "main"),
            DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            }
        );
    }

    #[test]
    fn parallel_complete_block_evolution_records_side_commit_cas_0930() {
        let (dir, delivery) = parallel_merge_fixture(
            "base();\ntimeout(10);\n",
            "delivered();\ntimeout(10);\n",
            "base();\ntimeout(30);\nretry();\n",
            "delivered();\ntimeout(30);\nretry();\n",
        );
        let side = git(dir.path(), &["rev-parse", "main^1"]);
        assert_eq!(
            delivery_content_presence_on_target(dir.path(), &delivery, "main"),
            DeliveryContentPresence::Present {
                paths: vec!["copy.txt".into()]
            }
        );
        // Anchor ownership in the timeout before the side fork, then force
        // its actual edit through a merge where the sibling route is lost.
        let base = git(dir.path(), &["rev-parse", &format!("{delivery}^1")]);
        assert_eq!(
            delivery_evolution::line_content_presence(
                dir.path(),
                &format!("{base}^1"),
                &base,
                "main",
                "copy.txt"
            )
            .unwrap(),
            Some(DeliveryContentPresence::Superseded {
                paths: vec!["copy.txt".into()],
                commits: vec![delivery, side]
            })
        );
    }

    #[test]
    fn parallel_block_partial_deletion_rejects_cas_0930() {
        let (dir, delivery) = parallel_merge_fixture(
            "base();\ntimeout(10);\n",
            "delivered();\ntimeout(10);\n",
            "base();\ntimeout(30);\nretry();\n",
            "delivered();\ntimeout(30);\nretry();\n",
        );
        let base = git(dir.path(), &["rev-parse", &format!("{delivery}^1")]);
        commit(
            dir.path(),
            "copy.txt",
            "delivered();\ntimeout(30);\n",
            "lose retry",
        );
        assert_eq!(
            delivery_evolution::line_content_presence(
                dir.path(),
                &format!("{base}^1"),
                &base,
                "main",
                "copy.txt"
            )
            .unwrap(),
            Some(DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            })
        );
    }

    #[test]
    fn ordinary_loop_rewrite_spanning_closing_brace_supersedes_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        commit(
            repo,
            "copy.txt",
            "function labels() {\nfor (const item of group) {\nprepare();\n}\nbase();\nreturn labels;\n}\n",
            "baseline",
        );
        let delivery = commit(
            repo,
            "copy.txt",
            "function labels() {\nfor (const item of group) {\nprepare();\n}\ngroup.forEach((item) => {\nlabels.set(item, oldTag);\n});\nreturn labels;\n}\n",
            "cas-taskb: labels",
        );
        let edit = commit(
            repo,
            "copy.txt",
            "function labels() {\nfor (const item of group) {\nlabels.set(item, newTag);\n}\nreturn labels;\n}\n",
            "ordinary labels rewrite",
        );
        assert_eq!(
            delivery_content_presence_on_target(repo, &delivery, "HEAD"),
            DeliveryContentPresence::Superseded {
                paths: vec!["copy.txt".into()],
                commits: vec![edit]
            }
        );
        commit(
            repo,
            "copy.txt",
            "function labels() {\nfor (const item of group) {\n}\nreturn labels;\n}\n",
            "delete replacement",
        );
        assert_eq!(
            delivery_content_presence_on_target(repo, &delivery, "HEAD"),
            DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            }
        );
    }

    #[test]
    fn merge_invention_without_ordinary_side_patch_rejects_cas_0930() {
        let (dir, delivery) = parallel_merge_fixture(
            "render(title, disabled);\nneighbor();\n",
            "render(delivered_title, disabled);\nneighbor();\n",
            "render(title, disabled);\nother_neighbor();\n",
            "render(delivered_title, invented);\nother_neighbor();\n",
        );
        assert_eq!(
            delivery_content_presence_on_target(dir.path(), &delivery, "main"),
            DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            }
        );
    }

    #[test]
    fn resolution_owns_novel_lines_not_imported_side_lines_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        commit(
            repo,
            "copy.txt",
            "base_owned();\nbase_inherited();\n",
            "baseline",
        );
        git(repo, &["branch", "-f", "main", "HEAD"]);
        commit(
            repo,
            "copy.txt",
            "worker();\nbase_inherited();\n",
            "cas-taskb: delivery",
        );
        git(repo, &["checkout", "main"]);
        commit(
            repo,
            "copy.txt",
            "target();\ninherited();\n",
            "target changes",
        );
        git(repo, &["checkout", "factory/worker"]);
        git(
            repo,
            &["merge", "--no-ff", "--no-commit", "-s", "ours", "main"],
        );
        let tip = commit(
            repo,
            "copy.txt",
            "resolved();\ninherited();\n",
            "cas-taskb: QA resolution",
        );
        git(repo, &["checkout", "main"]);
        git(
            repo,
            &["merge", "--no-ff", "factory/worker", "-m", "integrate"],
        );
        commit(
            repo,
            "copy.txt",
            "resolved();\n",
            "delete unrelated imported side line",
        );
        let mut window = window();
        window.identity.known_commits.push(tip.clone());
        assert_eq!(
            merge_tip_content_presence(repo, "main", &tip, Some(&window), &window.identity, None),
            Some(DeliveryContentPresence::Superseded {
                paths: vec!["copy.txt".into()],
                commits: vec![tip]
            })
        );
    }

    #[test]
    fn explicit_resolution_receipt_may_discuss_other_tasks_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        commit(
            repo,
            "copy.txt",
            "base_owned();\nbase_inherited();\n",
            "baseline",
        );
        git(repo, &["branch", "-f", "main", "HEAD"]);
        commit(
            repo,
            "copy.txt",
            "worker();\nbase_inherited();\n",
            "cas-taskb: delivery",
        );
        git(repo, &["checkout", "main"]);
        commit(
            repo,
            "copy.txt",
            "target();\ninherited();\n",
            "target changes",
        );
        git(repo, &["checkout", "factory/worker"]);
        git(
            repo,
            &["merge", "--no-ff", "--no-commit", "-s", "ours", "main"],
        );
        let tip = commit(
            repo,
            "copy.txt",
            "resolved();\ninherited();\n",
            "cas-taskb: QA resolution\n\nIntegrates cas-aaaa and preserves cas-bbbb behaviour.",
        );
        git(repo, &["checkout", "main"]);
        git(
            repo,
            &["merge", "--no-ff", "factory/worker", "-m", "integrate"],
        );
        commit(
            repo,
            "copy.txt",
            "resolved();\n",
            "delete unrelated imported side line",
        );
        let mut window = window();
        window.identity.known_commits.push(tip.clone());
        assert_eq!(
            merge_tip_content_presence(repo, "main", &tip, Some(&window), &window.identity, None),
            Some(DeliveryContentPresence::Superseded {
                paths: vec!["copy.txt".into()],
                commits: vec![tip]
            })
        );
    }

    #[test]
    fn union_cannot_restore_removed_baseline_list_member_cas_0930() {
        let (dir, delivery) = parallel_merge_fixture(
            "Alpha, ADMIN, Beta,\nneighbor();\n",
            "Alpha, DeliveryProbe, Beta,\nneighbor();\n",
            "Alpha, ADMIN, SiblingProbe, Beta,\nneighbor();\n",
            "Alpha, ADMIN, DeliveryProbe, SiblingProbe, Beta,\nnew_neighbor();\n",
        );
        assert_eq!(
            delivery_content_presence_on_target(dir.path(), &delivery, "main"),
            DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            }
        );
    }

    #[test]
    fn union_cannot_introduce_items_absent_from_both_parents_cas_0930() {
        let (dir, delivery) = parallel_merge_fixture(
            "Alpha, Beta,\n",
            "Alpha, DeliveryProbe, Beta,\n",
            "Alpha, SiblingProbe, Beta,\n",
            "Alpha, DeliveryProbe, SiblingProbe, Invented, Beta,\n",
        );
        assert_eq!(
            delivery_content_presence_on_target(dir.path(), &delivery, "main"),
            DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            }
        );
    }

    #[test]
    fn union_cannot_ignore_sibling_removal_of_baseline_item_cas_0930() {
        let (dir, delivery) = parallel_merge_fixture(
            "Alpha, Beta,\n",
            "Alpha, DeliveryProbe, Beta,\n",
            "Alpha, SiblingProbe,\n",
            "Alpha, DeliveryProbe, SiblingProbe, Beta,\n",
        );
        assert_eq!(
            delivery_content_presence_on_target(dir.path(), &delivery, "main"),
            DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            }
        );
    }

    #[test]
    fn union_transport_still_requires_later_line_survival_cas_0930() {
        let (dir, delivery) = parallel_merge_fixture(
            "Alpha, Beta,\n",
            "Alpha, DeliveryProbe, Beta,\n",
            "Alpha, SiblingProbe, Beta,\n",
            "Alpha, DeliveryProbe, SiblingProbe, Beta,\n",
        );
        assert_eq!(
            delivery_content_presence_on_target(dir.path(), &delivery, "main"),
            DeliveryContentPresence::Present {
                paths: vec!["copy.txt".into()]
            }
        );
        commit(dir.path(), "copy.txt", "", "delete union line");
        assert_eq!(
            delivery_content_presence_on_target(dir.path(), &delivery, "main"),
            DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            }
        );
    }

    #[test]
    fn earlier_worker_resolution_cannot_revive_deleted_content_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        commit(repo, "copy.txt", "worker();\n", "cas-taskb: delivery");
        git(repo, &["checkout", "main"]);
        commit(repo, "copy.txt", "target();\n", "target edit");
        git(repo, &["checkout", "factory/worker"]);
        git(
            repo,
            &["merge", "--no-ff", "--no-commit", "-s", "ours", "main"],
        );
        commit(
            repo,
            "copy.txt",
            "resolved();\n",
            "unnamed worker resolution",
        );
        commit(
            repo,
            "follow-up.rs",
            "follow_up();\n",
            "cas-taskb: follow up",
        );
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
        commit(
            repo,
            "copy.txt",
            "",
            "delete resolution without replacement",
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
    fn stale_merge_restoring_pre_delivery_superset_rejects_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        commit(
            repo,
            "copy.txt",
            "grant(user, ADMIN);\nneighbor();\n",
            "baseline authorization",
        );
        git(repo, &["branch", "-f", "main", "HEAD"]);
        let delivery = commit(
            repo,
            "copy.txt",
            "grant(user);\nneighbor();\n",
            "cas-taskb: restrict grant",
        );
        git(repo, &["checkout", "main"]);
        commit(
            repo,
            "copy.txt",
            "grant(user, ADMIN);\nstale_neighbor();\n",
            "stale branch edit",
        );
        git(
            repo,
            &[
                "merge",
                "--no-ff",
                "-s",
                "ours",
                "factory/worker",
                "-m",
                "stale integration",
            ],
        );
        // The neighbor changes in the same hunk, defeating an exact inverse-block check.
        assert_eq!(
            delivery_content_presence_on_target(repo, &delivery, "main"),
            DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            }
        );
    }

    #[test]
    fn task_owned_resolution_cannot_restore_stale_superset_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        commit(
            repo,
            "copy.txt",
            "grant(user, ADMIN);\nneighbor();\n",
            "baseline authorization",
        );
        git(repo, &["branch", "-f", "main", "HEAD"]);
        commit(
            repo,
            "copy.txt",
            "grant(user);\nneighbor();\n",
            "cas-taskb: restrict grant",
        );
        git(repo, &["checkout", "main"]);
        commit(
            repo,
            "copy.txt",
            "grant(user, ADMIN);\nstale_neighbor();\n",
            "stale branch edit",
        );
        git(repo, &["checkout", "factory/worker"]);
        git(
            repo,
            &["merge", "--no-ff", "--no-commit", "-s", "ours", "main"],
        );
        let tip = commit(
            repo,
            "copy.txt",
            "grant(user, ADMIN);\nnovel_neighbor();\n",
            "cas-taskb: stale resolution",
        );
        git(repo, &["checkout", "main"]);
        git(
            repo,
            &["merge", "--no-ff", "factory/worker", "-m", "integrate"],
        );
        let mut window = window();
        window.identity.known_commits.push(tip.clone());
        assert_eq!(
            merge_resolution_paths(repo, &tip, &window.identity).unwrap(),
            vec!["copy.txt"]
        );
        assert_eq!(
            merge_tip_content_presence(repo, "main", &tip, Some(&window), &window.identity, None),
            Some(DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            })
        );
    }

    fn expanded_grant_resolution_fixture(
        adjacent: bool,
        novel_grant: bool,
    ) -> (tempfile::TempDir, String) {
        let dir = fixture();
        let repo = dir.path();
        let separation = if adjacent {
            ""
        } else {
            "kept_one();\nkept_two();\n"
        };
        commit(
            repo,
            "copy.txt",
            &format!("grant(user, ADMIN);\n{separation}old_qa();\n"),
            "baseline authorization",
        );
        git(repo, &["branch", "-f", "main", "HEAD"]);
        commit(
            repo,
            "copy.txt",
            &format!("grant(user);\n{separation}old_qa();\n"),
            "cas-taskb: restrict grant",
        );
        git(repo, &["checkout", "main"]);
        commit(
            repo,
            "copy.txt",
            &format!("grant(user, ADMIN, audit);\n{separation}foreign_qa();\n"),
            "expand baseline grant",
        );
        git(repo, &["checkout", "factory/worker"]);
        git(
            repo,
            &["merge", "--no-ff", "--no-commit", "-s", "ours", "main"],
        );
        let grant = if novel_grant {
            "grant(user, audit);"
        } else {
            "grant(user, ADMIN, audit);"
        };
        let handoff = commit(
            repo,
            "copy.txt",
            &format!("{grant}\n{separation}novel_qa();\n"),
            "cas-taskb: QA resolution",
        );
        git(repo, &["checkout", "main"]);
        git(
            repo,
            &["merge", "--no-ff", "factory/worker", "-m", "integrate"],
        );
        (dir, handoff)
    }

    #[test]
    fn task_owned_resolution_cannot_import_expanded_stale_grant_cas_0930() {
        let (dir, handoff) = expanded_grant_resolution_fixture(false, false);
        let mut window = window();
        window.identity.known_commits.push(handoff.clone());
        assert_eq!(
            merge_resolution_paths(dir.path(), &handoff, &window.identity).unwrap(),
            vec!["copy.txt"]
        );
        assert_eq!(
            merge_tip_content_presence(
                dir.path(),
                "main",
                &handoff,
                Some(&window),
                &window.identity,
                None
            ),
            Some(DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()],
            })
        );
    }

    #[test]
    fn adjacent_novel_resolution_cannot_own_imported_stale_grant_cas_0930() {
        let (dir, handoff) = expanded_grant_resolution_fixture(true, false);
        let mut window = window();
        window.identity.known_commits.push(handoff.clone());
        assert_eq!(
            merge_resolution_paths(dir.path(), &handoff, &window.identity).unwrap(),
            vec!["copy.txt"]
        );
        assert_eq!(
            merge_tip_content_presence(
                dir.path(),
                "main",
                &handoff,
                Some(&window),
                &window.identity,
                None
            ),
            Some(DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()],
            })
        );
    }

    #[test]
    fn task_owned_novel_grant_resolution_records_supersession_cas_0930() {
        let (dir, handoff) = expanded_grant_resolution_fixture(false, true);
        let mut window = window();
        window.identity.known_commits.push(handoff.clone());
        assert_eq!(
            merge_tip_content_presence(
                dir.path(),
                "main",
                &handoff,
                Some(&window),
                &window.identity,
                None
            ),
            Some(DeliveryContentPresence::Superseded {
                paths: vec!["copy.txt".into()],
                commits: vec![handoff],
            })
        );
    }

    #[test]
    fn commented_out_delivered_line_records_superseding_commit_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        let delivery = commit(
            repo,
            "copy.txt",
            "check_auth(user)?;\n",
            "cas-taskb: check authorization",
        );
        git(repo, &["checkout", "main"]);
        git(
            repo,
            &["merge", "--no-ff", "factory/worker", "-m", "integrate"],
        );
        let commenting = commit(
            repo,
            "copy.txt",
            "// check_auth(user)?;\n",
            "disable authorization check",
        );
        assert_eq!(
            delivery_content_presence_on_target(repo, &delivery, "main"),
            DeliveryContentPresence::Superseded {
                paths: vec!["copy.txt".into()],
                commits: vec![commenting],
            }
        );
    }

    #[test]
    fn ordinary_line_extension_records_superseding_commit_cas_0930() {
        let dir = fixture();
        let repo = dir.path();
        let delivery = commit(repo, "copy.txt", "grant(user);\n", "cas-taskb: grant");
        git(repo, &["checkout", "main"]);
        git(
            repo,
            &["merge", "--no-ff", "factory/worker", "-m", "integrate"],
        );
        let extension = commit(repo, "copy.txt", "grant(user, ADMIN);\n", "extend grant");
        assert_eq!(
            delivery_content_presence_on_target(repo, &delivery, "main"),
            DeliveryContentPresence::Superseded {
                paths: vec!["copy.txt".into()],
                commits: vec![extension],
            }
        );
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
        // GH #1037 and cas-2664 (7) state the same fact from two gates:
        // `incoming.vue` reached the lane through a merge of the target, so
        // it is target content, not this task's delivery. QA must not demand
        // evidence for it (#1037), and proof/attribution must not charge it
        // to the task (cas-2664). This assertion used to pin the raw-path
        // behaviour that predates cas-2664; both views now exclude it.
        assert!(
            !paths(repo, "main", &window(), Some(&merge_tip))
                .unwrap()
                .contains(&"incoming.vue".into())
        );
        // The empty QA set stays authoritative (`Some`), not "unattributable"
        // (`None`), so the QA gate cannot fall back to the merge's whole diff.
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

    /// cas-24d8: the ownership walk skips edges that leave the path's blob
    /// unchanged (a long run of unrelated commits, a side merge that never
    /// touches the file) without losing the edge that does change it.
    #[test]
    fn unrelated_history_is_skipped_but_a_later_deletion_is_still_caught_cas_24d8() {
        let dir = fixture();
        let repo = dir.path();
        let base = git(repo, &["rev-parse", "HEAD"]);
        let delivery = commit(
            repo,
            "copy.txt",
            "old\ndelivered();\n",
            "cas-taskb: delivery",
        );
        for index in 0..40 {
            commit(
                repo,
                "other.txt",
                &format!("{index}\n"),
                &format!("unrelated {index}"),
            );
        }
        git(repo, &["checkout", "-qb", "side"]);
        commit(repo, "side.txt", "side\n", "side work");
        git(repo, &["checkout", "-q", "factory/worker"]);
        git(
            repo,
            &["merge", "-q", "--no-ff", "-m", "merge side", "side"],
        );
        git(repo, &["branch", "-f", "main", "HEAD"]);
        assert_eq!(
            delivery_evolution::line_content_presence(repo, &base, &delivery, "main", "copy.txt")
                .unwrap(),
            Some(DeliveryContentPresence::Present {
                paths: vec!["copy.txt".into()]
            })
        );
        commit(repo, "copy.txt", "old\n", "drop the delivered line");
        git(repo, &["branch", "-f", "main", "HEAD"]);
        assert_eq!(
            delivery_evolution::line_content_presence(repo, &base, &delivery, "main", "copy.txt")
                .unwrap(),
            Some(DeliveryContentPresence::Dropped {
                paths: vec!["copy.txt".into()]
            })
        );
    }
}
