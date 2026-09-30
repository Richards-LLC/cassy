//! Attribute missing delivery lines to explicit later edits, including side
//! branches merged into the target. A file touch alone cannot prove evolution.
use std::path::Path;
use std::process::Command;

#[derive(Default)]
struct Hunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    removed: Vec<String>,
    added: Vec<String>,
}

fn text(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .map_err(|error| format!("failed to inspect delivery line history: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Git could not inspect delivery line history (exit {})",
            output.status
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| "delivery line history is not UTF-8".into())
}

fn span(field: &str) -> Option<(usize, usize)> {
    let (start, count) = field[1..].split_once(',').unwrap_or((&field[1..], "1"));
    Some((start.parse().ok()?, count.parse().ok()?))
}

fn hunks(repo: &Path, left: &str, right: &str, path: &str) -> Result<Vec<Hunk>, String> {
    let patch = text(
        repo,
        &[
            "diff",
            "--unified=0",
            "--no-renames",
            left,
            right,
            "--",
            path,
        ],
    )?;
    let mut hunks: Vec<Hunk> = Vec::new();
    for line in patch.lines() {
        if line.starts_with("@@ ") {
            let fields: Vec<_> = line.split_whitespace().collect();
            let (old_start, old_count) = span(fields[1]).ok_or("invalid old hunk range")?;
            let (new_start, new_count) = span(fields[2]).ok_or("invalid new hunk range")?;
            hunks.push(Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
                ..Hunk::default()
            });
        } else if let Some(hunk) = hunks.last_mut() {
            if let Some(line) = line.strip_prefix('-') {
                hunk.removed.push(line.trim().to_string());
            } else if let Some(line) = line.strip_prefix('+') {
                hunk.added.push(line.trim().to_string());
            }
        }
    }
    Ok(hunks)
}

fn meaningful(line: &str) -> bool {
    line.chars().any(|c| c.is_alphanumeric())
}

/// Map a delivered line through a patch. A replacement keeps its line
/// lineage; a deletion ends it, so later recreation cannot hide a real loss.
fn project(line: usize, hunks: &[Hunk]) -> Option<usize> {
    let mut offset = 0isize;
    for hunk in hunks {
        if hunk.old_count > 0 && (hunk.old_start..hunk.old_start + hunk.old_count).contains(&line) {
            return (hunk.new_count > 0).then(|| {
                hunk.new_start + (line - hunk.old_start).min(hunk.new_count.saturating_sub(1))
            });
        }
        let precedes_line = if hunk.old_count == 0 {
            // Zero-length old ranges name the preceding line. An insertion
            // immediately after this line must not move this line's owner.
            hunk.old_start < line
        } else {
            hunk.old_start + hunk.old_count <= line
        };
        if precedes_line {
            offset += hunk.new_count as isize - hunk.old_count as isize;
        }
    }
    line.checked_add_signed(offset)
}

/// All absent, meaningful added lines need either an exact replacement in
/// descendant history or a replacement of their mapped line on the target's
/// first-parent lineage. Merge resolutions can map the line but never certify
/// it themselves: a stale/ours integration still fails without a later edit.
pub(super) fn superseding_commits(
    repo: &Path,
    parent: &str,
    delivery: &str,
    target: &str,
    integration: &str,
    post_integration: &[String],
    path: &str,
) -> Result<Option<Vec<String>>, String> {
    let delivery_hunks = hunks(repo, parent, delivery, path)?;
    // A deleted target path is a real loss. Do not let an older replacement
    // commit bless the later disappearance of the whole file.
    let output = Command::new("git")
        .args(["show", &format!("{target}:{path}")])
        .current_dir(repo)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Ok(None);
    }
    let target_text =
        String::from_utf8(output.stdout).map_err(|_| "target content is not UTF-8")?;
    let mut counts = std::collections::HashMap::<&str, usize>::new();
    for line in target_text.lines().map(str::trim) {
        *counts.entry(line).or_default() += 1;
    }
    let mut missing = Vec::new();
    for hunk in &delivery_hunks {
        for (index, line) in hunk
            .added
            .iter()
            .enumerate()
            .filter(|(_, line)| meaningful(line))
        {
            let count = counts.entry(line.as_str()).or_default();
            if *count > 0 {
                *count -= 1;
            } else {
                missing.push((hunk.new_start + index, line.clone()));
            }
        }
    }
    if missing.is_empty() {
        return Ok(None);
    }
    let mut owners: Vec<Option<String>> = vec![None; missing.len()];
    let history = text(
        repo,
        &[
            "rev-list",
            "--full-history",
            "--ancestry-path",
            "--reverse",
            "--no-merges",
            &format!("{delivery}..{target}"),
            "--",
            path,
        ],
    )?;
    for commit in history.lines() {
        let changes = hunks(repo, &format!("{commit}^1"), commit, path)?;
        for (index, (_, line)) in missing.iter().enumerate() {
            if owners[index].is_none()
                && changes.iter().any(|hunk| {
                    hunk.removed.contains(line) && hunk.added.iter().any(|line| meaningful(line))
                })
            {
                owners[index] = Some(commit.to_string());
            }
        }
        if owners.iter().all(Option::is_some) {
            break;
        }
    }
    if owners.iter().any(Option::is_none) {
        let integration_patch = hunks(repo, delivery, integration, path)?;
        let mut positions: Vec<_> = missing
            .iter()
            .map(|(line, _)| project(*line, &integration_patch))
            .collect();
        for commit in post_integration {
            let changes = hunks(repo, &format!("{commit}^1"), commit, path)?;
            let ordinary = super::git_commit_parent_count(repo, commit) == 1;
            for (index, position) in positions.iter_mut().enumerate() {
                let Some(line) = *position else {
                    continue;
                };
                if owners[index].is_none()
                    && ordinary
                    && changes.iter().any(|hunk| {
                        hunk.old_count > 0
                            && (hunk.old_start..hunk.old_start + hunk.old_count).contains(&line)
                            && hunk.added.iter().any(|line| meaningful(line))
                    })
                {
                    owners[index] = Some(commit.clone());
                }
                *position = project(line, &changes);
            }
        }
    }
    if owners.iter().any(Option::is_none) {
        return Ok(None);
    }
    let mut commits = Vec::new();
    for commit in owners.into_iter().flatten() {
        if !commits.contains(&commit) {
            commits.push(commit);
        }
    }
    Ok(Some(commits))
}
