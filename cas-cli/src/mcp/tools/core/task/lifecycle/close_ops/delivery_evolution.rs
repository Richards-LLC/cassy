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
    if patch.contains("Binary files ") || patch.contains("GIT binary patch") {
        return Err("binary delivery evolution cannot be attributed to line ranges".into());
    }
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

#[derive(Clone)]
struct OwnedLines {
    positions: Vec<usize>,
    commits: Vec<String>,
    baseline: Vec<String>,
}

pub(super) fn is_revert_message(message: &str) -> bool {
    let lower = message.trim_start().to_ascii_lowercase();
    lower.starts_with("revert ")
        || lower.starts_with("revert:")
        || lower
            .lines()
            .any(|line| line.trim_start().starts_with("this reverts commit "))
}

fn tokens(line: &str) -> Vec<&str> {
    line.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|token| !token.is_empty())
        .collect()
}

// Extensions may retain a line's symbols in the same changed hunk. Never
// look elsewhere in the file, where a duplicate has different ownership.
fn retains_line(old: &str, new: &str) -> bool {
    if old == new {
        return true;
    }
    let old = tokens(old);
    let new = tokens(new);
    if old.is_empty() {
        return false;
    }
    let mut cursor = 0;
    old.into_iter().all(|token| {
        let Some(offset) = new[cursor..]
            .iter()
            .position(|candidate| *candidate == token)
        else {
            return false;
        };
        cursor += offset + 1;
        true
    })
}

fn advance(
    owner: &OwnedLines,
    changes: &[Hunk],
    ordinary: bool,
    resolution_parent_lines: Option<&std::collections::HashSet<String>>,
    reverted: bool,
    commit: &str,
) -> Option<OwnedLines> {
    let mut next = OwnedLines {
        positions: Vec::new(),
        commits: owner.commits.clone(),
        baseline: owner.baseline.clone(),
    };
    for line in &owner.positions {
        let mut offset = 0isize;
        let mut changed = false;
        for hunk in changes {
            if hunk.old_count > 0
                && (hunk.old_start..hunk.old_start + hunk.old_count).contains(line)
            {
                if reverted {
                    return None;
                }
                let added: Vec<_> = hunk
                    .added
                    .iter()
                    .filter(|line| meaningful(line))
                    .cloned()
                    .collect();
                if !owner.baseline.is_empty() && added == owner.baseline {
                    // Unlabeled inverse patches also restore pre-delivery content.
                    return None;
                }
                let removed_index = *line - hunk.old_start;
                let old = hunk.removed.get(removed_index)?;
                let occurrence = hunk.removed[..removed_index]
                    .iter()
                    .filter(|previous| *previous == old)
                    .count();
                let retained: Vec<_> = hunk
                    .added
                    .iter()
                    .enumerate()
                    .filter(|(_, new)| retains_line(old, new))
                    .collect();
                if let Some((index, _)) = retained.get(occurrence) {
                    next.positions.push(hunk.new_start + *index);
                } else {
                    // A merge may only transport content from a proven
                    // parent. Its resolution cannot invent ownership.
                    // Duplicate removal is loss, not a novel replacement.
                    let novel_resolution = resolution_parent_lines.is_some_and(|parents| {
                        hunk.added
                            .iter()
                            .any(|line| meaningful(line) && !parents.contains(line))
                    });
                    if (!ordinary && !novel_resolution)
                        || hunk.added.iter().any(|new| retains_line(old, new))
                    {
                        return None;
                    }
                    let replacement: Vec<_> = hunk
                        .added
                        .iter()
                        .enumerate()
                        .filter(|(_, new)| meaningful(new))
                        .map(|(index, _)| hunk.new_start + index)
                        .collect();
                    if replacement.is_empty() {
                        return None;
                    }
                    next.positions.extend(replacement);
                    if !next.commits.iter().any(|known| known == commit) {
                        next.commits.push(commit.to_string());
                    }
                }
                changed = true;
                break;
            }
            let precedes = if hunk.old_count == 0 {
                hunk.old_start < *line
            } else {
                hunk.old_start + hunk.old_count <= *line
            };
            if precedes {
                offset += hunk.new_count as isize - hunk.old_count as isize;
            }
        }
        if !changed {
            next.positions.push(line.checked_add_signed(offset)?);
        }
    }
    next.positions.sort_unstable();
    next.positions.dedup();
    Some(next)
}

/// Prove each delivered line's ownership through every intervening edit on
/// an actual descendant route to the target. Deleted/reverted ownership ends;
/// a later file touch or matching duplicate cannot resurrect it. At a merge,
/// only a surviving line from a parent that already proves delivery counts.
/// `None` leaves binary/deletion-only patches to the reverse-patch proof.
pub(super) fn line_content_presence(
    repo: &Path,
    parent: &str,
    delivery: &str,
    target: &str,
    path: &str,
) -> Result<Option<super::DeliveryContentPresence>, String> {
    line_content_presence_with_resolution(repo, parent, delivery, target, path, None)
}

/// Only a caller that independently measured a task-owned remerge resolution
/// may authorize that one merge's novel replacement hunks on this path.
pub(super) fn line_content_presence_with_resolution(
    repo: &Path,
    parent: &str,
    delivery: &str,
    target: &str,
    path: &str,
    resolution: Option<&str>,
) -> Result<Option<super::DeliveryContentPresence>, String> {
    let delivery_commit = super::resolve_branch_sha(repo, &format!("{delivery}^{{commit}}"))
        .ok_or("delivery line anchor does not resolve to a commit")?;
    let target_commit = super::resolve_branch_sha(repo, &format!("{target}^{{commit}}"))
        .ok_or("delivery line target does not resolve to a commit")?;
    let delivery = delivery_commit.as_str();
    let target = target_commit.as_str();
    let initial = match hunks(repo, parent, delivery, path) {
        Ok(hunks) => hunks,
        Err(reason) if reason.starts_with("binary delivery") => return Ok(None),
        Err(reason) => return Err(reason),
    };
    let owners: Vec<_> = initial
        .iter()
        .flat_map(|hunk| {
            hunk.added
                .iter()
                .enumerate()
                .filter(|(_, line)| meaningful(line))
                .map(move |(index, _)| {
                    Some(OwnedLines {
                        positions: vec![hunk.new_start + index],
                        commits: Vec::new(),
                        baseline: hunk
                            .removed
                            .iter()
                            .filter(|line| meaningful(line))
                            .cloned()
                            .collect(),
                    })
                })
        })
        .collect();
    if owners.is_empty() {
        return Ok(None);
    }
    if !super::git_commit_is_ancestor(repo, delivery, target) {
        return Err("delivery line ancestry is not proven on the target".into());
    }
    let history = text(
        repo,
        &[
            "rev-list",
            "--ancestry-path",
            "--topo-order",
            "--reverse",
            "--parents",
            &format!("{delivery}..{target}"),
        ],
    )?;
    let mut states = std::collections::HashMap::new();
    states.insert(delivery.to_string(), owners);
    for record in history.lines() {
        let fields: Vec<_> = record.split_whitespace().collect();
        let commit = fields[0];
        let ordinary = fields.len() == 2;
        let reverted = is_revert_message(&text(repo, &["show", "-s", "--format=%B", commit])?);
        let resolution_parent_lines = if resolution == Some(commit) {
            let mut lines = std::collections::HashSet::new();
            for prior in fields.iter().skip(1) {
                let output = Command::new("git")
                    .args(["show", &format!("{prior}:{path}")])
                    .current_dir(repo)
                    .output()
                    .map_err(|error| error.to_string())?;
                if output.status.success() {
                    let contents = String::from_utf8(output.stdout)
                        .map_err(|_| "resolution parent is not UTF-8")?;
                    lines.extend(contents.lines().map(|line| line.trim().to_string()));
                }
            }
            Some(lines)
        } else {
            None
        };
        let mut merged: Vec<Option<OwnedLines>> = vec![None; states[delivery].len()];
        for prior in fields.iter().skip(1) {
            let Some(previous) = states.get(*prior) else {
                continue;
            };
            let changes = hunks(repo, prior, commit, path)?;
            for (index, owner) in previous.iter().enumerate() {
                if merged[index].is_none() {
                    merged[index] = owner.as_ref().and_then(|owner| {
                        advance(
                            owner,
                            &changes,
                            ordinary,
                            resolution_parent_lines.as_ref(),
                            reverted,
                            commit,
                        )
                    });
                }
            }
        }
        states.insert(commit.to_string(), merged);
    }
    let final_owners = states
        .get(target)
        .ok_or("target delivery line state is unavailable")?;
    if final_owners.iter().any(Option::is_none) {
        return Ok(Some(super::DeliveryContentPresence::Dropped {
            paths: vec![path.to_string()],
        }));
    }
    let mut commits = Vec::new();
    for owner in final_owners.iter().flatten() {
        for commit in &owner.commits {
            if !commits.contains(commit) {
                commits.push(commit.clone());
            }
        }
    }
    Ok(Some(if commits.is_empty() {
        super::DeliveryContentPresence::Present {
            paths: vec![path.to_string()],
        }
    } else {
        super::DeliveryContentPresence::Superseded {
            paths: vec![path.to_string()],
            commits,
        }
    }))
}
