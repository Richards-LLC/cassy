use super::{Axis, AxisRecord, AxisReport, ReviewResult, Round};
use cas_types::RepositoryProofBoundary;
use std::path::Path;
use std::process::Command;

fn output(root: &Path, args: &[&str]) -> ReviewResult<Vec<u8>> {
    let result = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !result.status.success() {
        return Err(format!(
            "shadow Git failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    Ok(result.stdout)
}
fn text(root: &Path, args: &[&str]) -> ReviewResult<String> {
    String::from_utf8(output(root, args)?)
        .map(|s| s.trim().to_string())
        .map_err(|e| e.to_string())
}
pub(super) fn clean(root: &Path) -> ReviewResult<()> {
    if !text(
        root,
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            ".",
            ":(exclude).cas",
            ":(exclude).cas/**",
        ],
    )?
    .is_empty()
    {
        return Err("shadow review/apply requires a clean worktree".into());
    }
    // A clean index can still be in an interrupted cherry-pick/revert/merge.
    for marker in ["CHERRY_PICK_HEAD", "REVERT_HEAD", "MERGE_HEAD"] {
        let path = text(root, &["rev-parse", "--git-path", marker])?;
        if root.join(path).exists() {
            return Err("finish the existing Git operation before shadow review".into());
        }
    }
    Ok(())
}
pub(super) fn head(root: &Path) -> ReviewResult<String> {
    resolve(root, "HEAD")
}
pub(super) fn branch(root: &Path) -> ReviewResult<String> {
    text(root, &["symbolic-ref", "--short", "HEAD"])
}
pub(super) fn resolve(root: &Path, reference: &str) -> ReviewResult<String> {
    if reference.is_empty() || reference.starts_with('-') || reference.len() > 512 {
        return Err("invalid shadow Git reference".into());
    }
    text(
        root,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
    )
}
pub(super) fn ancestor(root: &Path, base: &str, head: &str) -> ReviewResult<()> {
    output(root, &["merge-base", "--is-ancestor", base, head]).map(|_| ())
}
pub(super) fn file_at(root: &Path, head: &str, file: &str) -> ReviewResult<Option<String>> {
    let files = text(root, &["ls-tree", "--name-only", head, "--", file])?;
    if files.is_empty() {
        return Ok(None);
    }
    let value = output(root, &["show", &format!("{head}:{file}")])?;
    if value.len() > 128 * 1024 {
        return Err("CODING_STANDARDS.md exceeds shadow context limit (128 KiB)".into());
    }
    String::from_utf8(value)
        .map(Some)
        .map_err(|e| e.to_string())
}
pub(super) fn create_axis(
    cas_root: &Path,
    task: &str,
    axis: Axis,
    agent_id: String,
    proof: &RepositoryProofBoundary,
) -> ReviewResult<AxisRecord> {
    let side_ref = format!("review/{task}/{}", axis.name());
    let worktree = cas_root
        .join("worktrees")
        .join(format!("review-{task}-{}", axis.name()));
    if worktree.exists() {
        return Err(format!(
            "shadow worktree already exists: {}",
            worktree.display()
        ));
    }
    std::fs::create_dir_all(cas_root.join("worktrees")).map_err(|e| e.to_string())?;
    output(
        Path::new(&proof.worktree_root),
        &[
            "worktree",
            "add",
            "-b",
            &side_ref,
            worktree.to_str().ok_or("non-UTF-8 shadow worktree path")?,
            &proof.head_commit,
        ],
    )?;
    Ok(AxisRecord {
        axis,
        agent_id,
        side_ref,
        worktree,
        report: None,
        reported_tip: None,
        checked_tip: None,
    })
}
pub(super) fn remove_axis(proof: &RepositoryProofBoundary, axis: &AxisRecord) {
    let root = Path::new(&proof.worktree_root);
    if let Some(path) = axis.worktree.to_str() {
        // Roll back only refs/worktrees created by this failed start.
        let _ = output(root, &["worktree", "remove", path]);
        let _ = output(root, &["branch", "-d", &axis.side_ref]);
    }
}
pub(super) fn commits(root: &Path, base: &str, tip: &str) -> ReviewResult<Vec<String>> {
    ancestor(root, base, tip)?;
    Ok(
        text(root, &["rev-list", "--reverse", &format!("{base}..{tip}")])?
            .lines()
            .map(str::to_string)
            .collect(),
    )
}
pub(super) fn validate_fixes(round: &Round, axis: Axis, report: &AxisReport) -> ReviewResult<()> {
    let record = round.axis(axis);
    clean(&record.worktree)?;
    let tip = head(&record.worktree)?;
    if resolve(&record.worktree, &record.side_ref)? != tip {
        return Err("axis worktree must remain on its reserved side ref".into());
    }
    if text(&record.worktree, &["symbolic-ref", "--short", "HEAD"])? != record.side_ref {
        return Err("axis worktree switched away from its reserved side ref".into());
    }
    let commits = commits(&record.worktree, &round.proof.head_commit, &tip)?;
    let reported: Vec<&str> = report
        .findings()
        .filter_map(|f| f.commit.as_deref())
        .collect();
    if reported.len() != commits.len()
        || commits
            .iter()
            .any(|c| reported.iter().filter(|r| **r == c.as_str()).count() != 1)
    {
        return Err(
            "every side-ref commit must be an exact full SHA attached to one certain finding"
                .into(),
        );
    }
    for finding in report.findings().filter(|f| f.commit.is_some()) {
        let commit = finding.commit.as_deref().unwrap();
        let parent_line = text(
            &record.worktree,
            &["rev-list", "--parents", "-n", "1", commit],
        )?;
        if parent_line.split_whitespace().count() != 2 {
            return Err("review fixes must be small non-merge commits".into());
        }
        let subject = text(&record.worktree, &["show", "-s", "--format=%s", commit])?;
        let prefix = format!("review({}): {} ", axis.name(), finding.id);
        if !subject.starts_with(&prefix) {
            return Err(format!("fix subject must start with {prefix:?}"));
        }
        if text(
            &record.worktree,
            &["diff-tree", "--no-commit-id", "--name-only", "-r", commit],
        )?
        .is_empty()
        {
            return Err("empty review fix commits cannot be receipted".into());
        }
    }
    Ok(())
}
pub(super) fn unchanged_axis(record: &AxisRecord) -> ReviewResult<()> {
    clean(&record.worktree)?;
    if branch(&record.worktree)? != record.side_ref {
        return Err("axis worktree switched away from its reserved side ref".into());
    }
    let expected = record
        .checked_tip
        .as_deref()
        .ok_or("axis has no sealed report")?;
    if head(&record.worktree)? != expected
        || resolve(&record.worktree, &record.side_ref)? != expected
    {
        return Err("axis ref changed after report/cross-check; restore its sealed tip".into());
    }
    Ok(())
}
pub(super) fn revert(
    record: &AxisRecord,
    axis: Axis,
    finding: &str,
    commit: &str,
    reason: &str,
    caller_id: &str,
) -> ReviewResult<String> {
    // The bound opposite child authorizes this revert. Preserve the reason and
    // identity on the actual commit as well as in the typed cross-check receipt.
    output(&record.worktree, &["revert", "--no-commit", commit]).inspect_err(|_| {
        let _ = output(&record.worktree, &["revert", "--abort"]);
    })?;
    let message = format!(
        "review({}): {finding} revert {commit}\n\n{reason}\n\nShadow-Reviewer: {caller_id}",
        axis.name()
    );
    if let Err(e) = output(&record.worktree, &["commit", "-m", &message]) {
        // Preserve staged revert if commit hooks fail; never reset away changes.
        return Err(format!(
            "revert is staged but commit failed; recover side ref before retrying: {e}"
        ));
    }
    head(&record.worktree)
}
pub(super) fn apply(root: &Path, commits: &[String]) -> ReviewResult<String> {
    if !commits.is_empty() {
        let mut args = vec!["cherry-pick", "-x", "--"];
        args.extend(commits.iter().map(String::as_str));
        if let Err(e) = output(root, &args) {
            if let Err(abort) = output(root, &["cherry-pick", "--abort"]) {
                return Err(format!(
                    "shadow apply failed and requires manual recovery: {e}; abort: {abort}"
                ));
            }
            return Err(format!("shadow apply conflicted; delivery restored: {e}"));
        }
    }
    head(root)
}
