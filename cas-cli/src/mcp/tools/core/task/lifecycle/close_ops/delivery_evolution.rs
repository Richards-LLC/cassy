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

// Git may split a local loop rewrite into a replacement and a deletion
// separated only by its closing brace. At ordinary commits, keep that
// complete changed block together; never join across executable context or
// apply this normalization at a merge.
fn ordinary_hunks(repo: &Path, left: &str, right: &str, path: &str) -> Result<Vec<Hunk>, String> {
    let raw = hunks(repo, left, right, path)?;
    if !raw.windows(2).any(|pair| {
        pair[0].old_count > 0
            && pair[0].new_count > 0
            && pair[1].old_count > 0
            && pair[1].new_count == 0
            && pair[1].old_start == pair[0].old_start + pair[0].old_count + 1
    }) {
        return Ok(raw);
    }
    let contents = text(repo, &["show", &format!("{left}:{path}")])?;
    let lines: Vec<_> = contents.lines().map(str::trim).collect();
    let mut merged: Vec<Hunk> = Vec::new();
    for hunk in raw {
        if let Some(previous) = merged.last_mut() {
            let old_end = previous.old_start + previous.old_count;
            let new_end = previous.new_start + previous.new_count;
            if previous.old_count > 0
                && previous.new_count > 0
                && hunk.old_count > 0
                && hunk.new_count == 0
                && hunk.old_start == old_end + 1
                && hunk.new_start == new_end
                && lines.get(old_end - 1) == Some(&"}")
            {
                previous.removed.push("}".into());
                previous.added.push("}".into());
                previous.old_count += 1 + hunk.old_count;
                previous.new_count += 1;
                previous.removed.extend(hunk.removed);
                continue;
            }
        }
        merged.push(hunk);
    }
    Ok(merged)
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

// A comma-separated identifier list is narrower than a token subsequence:
// calls, comments, operators, strings and duplicate members cannot qualify.
fn identifier_list(line: &str) -> Option<Vec<&str>> {
    if !line.contains(',') {
        return None;
    }
    let items: Vec<_> = line
        .strip_suffix(',')
        .unwrap_or(line)
        .split(',')
        .map(str::trim)
        .collect();
    let mut seen = std::collections::HashSet::new();
    for item in &items {
        let mut chars = item.chars();
        if !chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
            || !seen.insert(*item)
        {
            return None;
        }
    }
    Some(items)
}

fn ordered_subset(left: &[&str], right: &[&str]) -> bool {
    let mut cursor = 0;
    left.iter().all(|item| {
        let Some(offset) = right[cursor..]
            .iter()
            .position(|candidate| candidate == item)
        else {
            return false;
        };
        cursor += offset + 1;
        true
    })
}

struct MergeUnionContext<'a> {
    base_to_prior: &'a [Hunk],
    base_to_other: &'a [Hunk],
}

#[derive(PartialEq, Eq)]
struct LexicalEdit {
    start: usize,
    end: usize,
    added: String,
}

fn lexemes(line: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut previous = None;
    for (index, c) in line.char_indices() {
        let class = if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        };
        if index > start && (previous != Some(class) || class == 2) {
            result.push(&line[start..index]);
            start = index;
        }
        previous = Some(class);
    }
    if start < line.len() {
        result.push(&line[start..]);
    }
    result
}

fn lexical_edits(base: &[&str], changed: &[&str]) -> Option<Vec<LexicalEdit>> {
    // Bound memory and reject ambiguous/oversized evidence conservatively.
    if (base.len() + 1).checked_mul(changed.len() + 1)? > 1_000_000 {
        return None;
    }
    let mut lengths = vec![vec![0usize; changed.len() + 1]; base.len() + 1];
    for i in (0..base.len()).rev() {
        for j in (0..changed.len()).rev() {
            lengths[i][j] = if base[i] == changed[j] {
                lengths[i + 1][j + 1] + 1
            } else {
                lengths[i + 1][j].max(lengths[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut edits = Vec::new();
    let mut pending: Option<LexicalEdit> = None;
    while i < base.len() || j < changed.len() {
        if i < base.len() && j < changed.len() && base[i] == changed[j] {
            if let Some(edit) = pending.take() {
                edits.push(edit);
            }
            i += 1;
            j += 1;
        } else {
            let edit = pending.get_or_insert_with(|| LexicalEdit {
                start: i,
                end: i,
                added: String::new(),
            });
            if i < base.len() && (j == changed.len() || lengths[i + 1][j] >= lengths[i][j + 1]) {
                i += 1;
                edit.end = i;
            } else {
                edit.added.push_str(changed[j]);
                j += 1;
            }
        }
    }
    if let Some(edit) = pending {
        edits.push(edit);
    }
    Some(edits)
}

fn composed_parallel_line(base: &str, prior: &str, other: &str) -> Option<String> {
    let base = lexemes(base);
    let mut edits = lexical_edits(&base, &lexemes(prior))?;
    let side = lexical_edits(&base, &lexemes(other))?;
    if side.is_empty() {
        return None;
    }
    for edit in side {
        if !edits.contains(&edit) {
            edits.push(edit);
        }
    }
    edits.sort_by_key(|edit| (edit.start, edit.end));
    let mut result = String::new();
    let mut cursor = 0;
    for (index, edit) in edits.iter().enumerate() {
        if index > 0 && edit.start <= cursor {
            return None;
        }
        result.push_str(&base[cursor..edit.start].concat());
        result.push_str(&edit.added);
        cursor = edit.end;
    }
    result.push_str(&base[cursor..].concat());
    Some(result)
}

fn additive_import_union(base: &str, prior: &str, other: &str, merged: &str) -> bool {
    fn parse(line: &str) -> Option<(Vec<&str>, &str)> {
        let (names, module) = line.strip_prefix("import {")?.split_once("} from ")?;
        let names: Vec<_> = names
            .trim()
            .trim_end_matches(',')
            .split(',')
            .map(str::trim)
            .collect();
        let mut unique = std::collections::HashSet::new();
        if names.iter().any(|name| {
            name.is_empty()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                || !unique.insert(*name)
        }) {
            return None;
        }
        Some((names, module))
    }
    let Some((base_names, module)) = parse(base) else {
        return false;
    };
    let Some((prior_names, prior_module)) = parse(prior) else {
        return false;
    };
    let Some((other_names, other_module)) = parse(other) else {
        return false;
    };
    let Some((merged_names, merged_module)) = parse(merged) else {
        return false;
    };
    if module != prior_module
        || module != other_module
        || module != merged_module
        || prior_names.len() <= base_names.len()
        || other_names.len() <= base_names.len()
        || !base_names
            .iter()
            .all(|name| prior_names.contains(name) && other_names.contains(name))
    {
        return false;
    }
    let union: std::collections::HashSet<_> = prior_names
        .iter()
        .chain(other_names.iter())
        .copied()
        .collect();
    merged_names.len() == union.len() && merged_names.iter().all(|name| union.contains(name))
}

type ParallelEdits = std::collections::HashMap<usize, std::collections::HashMap<String, String>>;

fn parallel_edit_proofs(
    repo: &Path,
    base: &str,
    other: &str,
    path: &str,
    context: &MergeUnionContext<'_>,
    changes: &[Hunk],
    previous: &[Option<OwnedLines>],
) -> Result<ParallelEdits, String> {
    let mut proofs = ParallelEdits::new();
    let positions: std::collections::HashSet<_> = previous
        .iter()
        .flatten()
        .flat_map(|owner| owner.positions.iter().copied())
        .collect();
    for position in positions {
        let Some(change) = changes.iter().find(|hunk| {
            hunk.old_count > 0
                && (hunk.old_start..hunk.old_start + hunk.old_count).contains(&position)
        }) else {
            continue;
        };
        let old = &change.removed[position - change.old_start];
        let mut offset = 0isize;
        let mut correspondence = None;
        let mut unmappable = false;
        for hunk in context.base_to_prior {
            if hunk.new_count > 0
                && (hunk.new_start..hunk.new_start + hunk.new_count).contains(&position)
            {
                if hunk.old_count == 1 && hunk.new_count == 1 {
                    correspondence = Some((hunk.old_start, hunk.removed[0].as_str()));
                } else {
                    unmappable = true;
                }
                break;
            }
            let precedes = if hunk.new_count == 0 {
                hunk.new_start < position
            } else {
                hunk.new_start + hunk.new_count <= position
            };
            if precedes {
                offset += hunk.old_count as isize - hunk.new_count as isize;
            }
        }
        if unmappable {
            continue;
        }
        let Some(base_position) = position.checked_add_signed(offset) else {
            continue;
        };
        let (base_position, baseline) = correspondence.unwrap_or((base_position, old.as_str()));
        let Some(side_hunk) = context.base_to_other.iter().find(|hunk| {
            hunk.old_count == 1
                && hunk.new_count == 1
                && hunk.old_start == base_position
                && hunk.removed[0] == baseline
        }) else {
            continue;
        };
        for (side_index, side_line) in side_hunk.added.iter().enumerate() {
            let composed = composed_parallel_line(baseline, old, side_line);
            let candidates: Vec<_> = change
                .added
                .iter()
                .filter(|new| {
                    *new != old
                        && (composed.as_ref() == Some(*new)
                            || additive_import_union(baseline, old, side_line, new))
                })
                .collect();
            if candidates.is_empty() {
                continue;
            }
            // Name the actual ordinary side edit, not the importing merge.
            // Its blamed line must be an added line in a surviving patch
            // replacing this exact base line, on a descendant side commit.
            let side_position = side_hunk.new_start + side_index;
            let blamed = text(
                repo,
                &[
                    "blame",
                    "--line-porcelain",
                    "-L",
                    &format!("{side_position},{side_position}"),
                    other,
                    "--",
                    path,
                ],
            )?;
            let header: Vec<_> = blamed
                .lines()
                .next()
                .unwrap_or("")
                .split_whitespace()
                .collect();
            if header.len() < 3 {
                return Err("side line blame lacks a commit/range".into());
            }
            let commit = header[0];
            let original_position: usize =
                header[1].parse().map_err(|_| "invalid side blame range")?;
            if commit == base
                || !super::git_commit_is_ancestor(repo, base, commit)
                || !super::git_commit_is_ancestor(repo, commit, other)
            {
                continue;
            }
            let parents = text(repo, &["rev-list", "--parents", "-n", "1", commit])?;
            let parents: Vec<_> = parents.split_whitespace().collect();
            if parents.len() != 2
                || is_revert_message(&text(repo, &["show", "-s", "--format=%B", commit])?)
            {
                continue;
            }
            let authored = hunks(repo, parents[1], commit, path)?;
            if authored.iter().any(|hunk| {
                hunk.old_count == 1
                    && hunk.new_count == 1
                    && hunk.removed[0] == baseline
                    && original_position >= hunk.new_start
                    && hunk.added.get(original_position - hunk.new_start) == Some(side_line)
            }) {
                for new in candidates {
                    proofs
                        .entry(position)
                        .or_default()
                        .insert(new.clone(), commit.to_string());
                }
            }
        }
    }
    Ok(proofs)
}

impl MergeUnionContext<'_> {
    fn retains_list(&self, position: usize, old: &str, new: &str) -> bool {
        let Some(prior) = identifier_list(old) else {
            return false;
        };
        let Some(merged) = identifier_list(new) else {
            return false;
        };
        // The owned parent line must come from a changed single base line.
        // This keeps correspondence positional even when an adjacent list
        // line was inserted; a matching duplicate elsewhere proves nothing.
        let Some(prior_hunk) = self.base_to_prior.iter().find(|hunk| {
            hunk.new_count > 0
                && (hunk.new_start..hunk.new_start + hunk.new_count).contains(&position)
        }) else {
            return false;
        };
        if prior_hunk.old_count != 1
            || prior_hunk
                .added
                .get(position - prior_hunk.new_start)
                .map(String::as_str)
                != Some(old)
        {
            return false;
        }
        let Some(base_line) = prior_hunk.removed.first() else {
            return false;
        };
        let Some(base) = identifier_list(base_line) else {
            return false;
        };
        if base.len() >= prior.len() || !ordered_subset(&base, &prior) {
            // In particular, removing a baseline member and later restoring
            // it at a stale merge is never an additive union.
            return false;
        }
        let Some(other_hunk) = self.base_to_other.iter().find(|hunk| {
            hunk.old_count > 0
                && (hunk.old_start..hunk.old_start + hunk.old_count).contains(&prior_hunk.old_start)
        }) else {
            return false;
        };
        if other_hunk.old_count != 1 || other_hunk.removed.first() != Some(base_line) {
            return false;
        }
        let candidates: Vec<_> = other_hunk
            .added
            .iter()
            .filter_map(|line| identifier_list(line))
            .filter(|other| base.len() < other.len() && ordered_subset(&base, other))
            .collect();
        let [other] = candidates.as_slice() else {
            return false;
        };
        let union: std::collections::HashSet<_> =
            prior.iter().chain(other.iter()).copied().collect();
        merged.len() == union.len()
            && merged.iter().all(|item| union.contains(item))
            && ordered_subset(&prior, &merged)
            && ordered_subset(other, &merged)
    }
}

// Only ordinary edits may retain a generic line's symbols in a changed hunk.
// Merge text requires exact equality; separately proven list unions use
// MergeUnionContext, never this token-subsequence heuristic.
fn retains_line(old: &str, new: &str, ordinary: bool) -> bool {
    if old == new {
        return true;
    }
    if !ordinary {
        return false;
    }
    // Commenting out executable content is replacement, not retention.
    if ["//", "/*", "#", "--", "<!--"]
        .iter()
        .any(|prefix| new.starts_with(*prefix) && !old.starts_with(*prefix))
    {
        return false;
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
    merge_union: Option<&MergeUnionContext<'_>>,
    parallel_edits: &ParallelEdits,
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
                    .filter(|(_, new)| {
                        retains_line(old, new, ordinary)
                            || merge_union.is_some_and(|union| union.retains_list(*line, old, new))
                            || parallel_edits
                                .get(line)
                                .is_some_and(|edits| edits.contains_key(new.as_str()))
                    })
                    .collect();
                if let Some((index, new)) = retained.get(occurrence) {
                    next.positions.push(hunk.new_start + *index);
                    if ordinary && old != *new && !next.commits.iter().any(|known| known == commit)
                    {
                        next.commits.push(commit.to_string());
                    }
                    if let Some(author) = parallel_edits
                        .get(line)
                        .and_then(|edits| edits.get(new.as_str()))
                        && !next.commits.contains(author)
                    {
                        next.commits.push(author.clone());
                    }
                } else {
                    // A merge may only transport content from a proven
                    // parent. Its resolution cannot invent ownership.
                    // Duplicate removal is loss, not a novel replacement.
                    let novel_resolution = resolution_parent_lines.is_some_and(|parents| {
                        hunk.added
                            .iter()
                            .any(|line| meaningful(line) && !parents.contains(line))
                            && !hunk
                                .added
                                .iter()
                                .any(|line| parents.contains(line) && retains_line(old, line, true))
                    });
                    if (!ordinary && !novel_resolution)
                        || hunk.added.iter().any(|new| {
                            retains_line(old, new, ordinary)
                                || merge_union
                                    .is_some_and(|union| union.retains_list(*line, old, new))
                                || parallel_edits
                                    .get(line)
                                    .is_some_and(|edits| edits.contains_key(new.as_str()))
                        })
                    {
                        return None;
                    }
                    let replacement: Vec<_> = hunk
                        .added
                        .iter()
                        .enumerate()
                        .filter(|(_, new)| {
                            meaningful(new)
                                && (ordinary
                                    || resolution_parent_lines
                                        .is_some_and(|parents| !parents.contains(*new)))
                        })
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
/// only surviving parent content counts, including an exact additive list
/// union proved against both parents' edits of the same merge-base line.
/// `None` leaves binary/deletion-only patches to the reverse-patch proof.
pub(super) fn line_content_presence(
    repo: &Path,
    parent: &str,
    delivery: &str,
    target: &str,
    path: &str,
) -> Result<Option<super::DeliveryContentPresence>, String> {
    line_content_presence_with_resolutions(repo, parent, delivery, target, path, &[])
}

/// Only a caller that independently measured a task-owned remerge resolution
/// may authorize those merges' novel replacement hunks on this path.
pub(super) fn line_content_presence_with_resolutions(
    repo: &Path,
    parent: &str,
    delivery: &str,
    target: &str,
    path: &str,
    resolutions: &[String],
) -> Result<Option<super::DeliveryContentPresence>, String> {
    line_content_presence_impl(repo, parent, delivery, target, path, resolutions, false)
}

pub(super) fn resolution_content_presence(
    repo: &Path,
    parent: &str,
    delivery: &str,
    target: &str,
    path: &str,
) -> Result<Option<super::DeliveryContentPresence>, String> {
    line_content_presence_impl(repo, parent, delivery, target, path, &[], true)
}

fn line_content_presence_impl(
    repo: &Path,
    parent: &str,
    delivery: &str,
    target: &str,
    path: &str,
    resolutions: &[String],
    novel_only: bool,
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
    // Imported side content is not authored by this merge resolution. Its
    // task owners are checked independently; only novel resolution lines
    // may add ownership here.
    let mut inherited = std::collections::HashSet::new();
    if novel_only {
        let parents = text(repo, &["rev-list", "--parents", "-n", "1", delivery])?;
        let parents: Vec<_> = parents.split_whitespace().collect();
        if parents.len() != 3 {
            return Err("resolution needs exactly two parents".into());
        }
        for prior in parents.iter().skip(1) {
            let output = Command::new("git")
                .args(["show", &format!("{prior}:{path}")])
                .current_dir(repo)
                .output()
                .map_err(|error| error.to_string())?;
            if output.status.success() {
                let contents = String::from_utf8(output.stdout)
                    .map_err(|_| "resolution parent is not UTF-8")?;
                inherited.extend(contents.lines().map(|line| line.trim().to_string()));
            }
        }
    }
    let owners: Vec<_> = initial
        .iter()
        .flat_map(|hunk| {
            hunk.added
                .iter()
                .enumerate()
                .filter(|(_, line)| meaningful(line) && !inherited.contains(*line))
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
        let mut union_changes = None;
        let mut union_base = None;
        let mut union_checked = false;
        let resolution_parent_lines = if resolutions.iter().any(|resolution| resolution == commit) {
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
            let changes = if ordinary {
                ordinary_hunks(repo, prior, commit, path)?
            } else {
                hunks(repo, prior, commit, path)?
            };
            // Most merge edges leave owned content unchanged. Only measure
            // both parents' base edits when a changed live hunk could need
            // list union or an independently authored parallel edit.
            if !union_checked
                && fields.len() == 3
                && previous.iter().any(Option::is_some)
                && changes.iter().any(|hunk| {
                    !hunk.removed.is_empty() && hunk.added.iter().any(|line| meaningful(line))
                })
            {
                union_checked = true;
                let bases = text(repo, &["merge-base", "--all", fields[1], fields[2]])?;
                let bases: Vec<_> = bases.lines().collect();
                if let [base] = bases.as_slice() {
                    union_base = Some(base.to_string());
                    union_changes = Some((
                        hunks(repo, base, fields[1], path)?,
                        hunks(repo, base, fields[2], path)?,
                    ));
                }
            }
            let merge_union = union_changes.as_ref().map(|(first, second)| {
                if *prior == fields[1] {
                    MergeUnionContext {
                        base_to_prior: first,
                        base_to_other: second,
                    }
                } else {
                    MergeUnionContext {
                        base_to_prior: second,
                        base_to_other: first,
                    }
                }
            });
            let parallel_edits = if let (Some(base), Some(context)) =
                (union_base.as_deref(), merge_union.as_ref())
            {
                let other = if *prior == fields[1] {
                    fields[2]
                } else {
                    fields[1]
                };
                parallel_edit_proofs(repo, base, other, path, context, &changes, previous)?
            } else {
                ParallelEdits::new()
            };
            let advanced: Vec<_> = previous
                .iter()
                .map(|owner| {
                    owner.as_ref().and_then(|owner| {
                        advance(
                            owner,
                            &changes,
                            ordinary,
                            merge_union.as_ref(),
                            &parallel_edits,
                            resolution_parent_lines.as_ref(),
                            reverted,
                            commit,
                        )
                    })
                })
                .collect();
            for index in 0..previous.len() {
                if merged[index].is_none() {
                    merged[index] = advanced[index].clone();
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
