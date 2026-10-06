//! Attribute missing delivery lines to explicit later edits, including side
//! branches merged into the target. A file touch alone cannot prove evolution.
use std::path::Path;
use std::process::Command;
use super::epic_measurement::CommandExt as _;

#[derive(Default, Clone)]
struct Hunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    removed: Vec<String>,
    added: Vec<String>,
}

fn text(repo: &Path, args: &[&str]) -> Result<String, String> {
    super::epic_measurement::check()?;
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .measurement_output()
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
    parse_hunks(&patch)
}

fn parse_hunks(patch: &str) -> Result<Vec<Hunk>, String> {
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
    normalize_ordinary(repo, left, path, hunks(repo, left, right, path)?)
}

fn normalize_ordinary(
    repo: &Path,
    left: &str,
    path: &str,
    raw: Vec<Hunk>,
) -> Result<Vec<Hunk>, String> {
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
    retire_draft: bool,
    internal_resolution: bool,
    reverted: bool,
    commit: &str,
) -> Option<OwnedLines> {
    if reverted && owner.positions.is_empty() && !changes.is_empty() {
        return None;
    }
    // Retirement cannot erase the baseline-restoration fence. In particular,
    // a later QA merge restoring that line beside a novel neighbour rejects
    // even if the original draft had already been retired.
    if (internal_resolution || retire_draft && owner.positions.is_empty())
        && changes
            .iter()
            .flat_map(|hunk| &hunk.added)
            .any(|line| meaningful(line) && owner.baseline.contains(line))
    {
        return None;
    }
    let mut next = OwnedLines {
        positions: Vec::new(),
        commits: owner.commits.clone(),
        baseline: owner.baseline.clone(),
    };
    // An owned QA merge may evolve into its own novel line. Imported parent
    // text still requires exact equality; task ownership cannot make a stale
    // expanded parent line carry a restricted delivery's ownership.
    let retains_owned_line = |old: &str, new: &str| {
        let novel_internal = internal_resolution
            && resolution_parent_lines.is_some_and(|parents| !parents.contains(new));
        retains_line(old, new, ordinary || novel_internal)
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
                if !owner.baseline.is_empty()
                    && (added == owner.baseline
                        || internal_resolution
                            && added.iter().any(|line| owner.baseline.contains(line)))
                {
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
                        retains_owned_line(old, new)
                            || merge_union.is_some_and(|union| union.retains_list(*line, old, new))
                            || parallel_edits
                                .get(line)
                                .is_some_and(|edits| edits.contains_key(new.as_str()))
                    })
                    .collect();
                if let Some((index, new)) = retained.get(occurrence) {
                    next.positions.push(hunk.new_start + *index);
                    if (ordinary || internal_resolution)
                        && old != *new
                        && !next.commits.iter().any(|known| known == commit)
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
                            retains_owned_line(old, new)
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
                    if replacement.is_empty() && !retire_draft {
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
    line_content_presence_impl(
        repo,
        parent,
        delivery,
        target,
        path,
        resolutions,
        false,
        &[],
        false,
    )
}

pub(super) fn resolution_content_presence(
    repo: &Path,
    parent: &str,
    delivery: &str,
    target: &str,
    path: &str,
) -> Result<Option<super::DeliveryContentPresence>, String> {
    line_content_presence_impl(repo, parent, delivery, target, path, &[], true, &[], false)
}

/// Caller proves the final handoff's own content on this path before allowing
/// draft retirement. The cycle is task-owned first-parent history only;
/// post-handoff edits and all other callers retain the ordinary drop rules.
pub(super) fn line_content_presence_with_task_cycle(
    repo: &Path,
    parent: &str,
    delivery: &str,
    target: &str,
    path: &str,
    resolutions: &[String],
    cycle: &[String],
) -> Result<Option<super::DeliveryContentPresence>, String> {
    line_content_presence_impl(
        repo,
        parent,
        delivery,
        target,
        path,
        resolutions,
        false,
        cycle,
        false,
    )
}

/// Only the exact-final-snapshot companion may ask whether some attributed
/// effect survives. Use the ordinary ownership history with no draft
/// retirement: a deleted/reverted owner cannot be resurrected by a copy.
pub(super) fn surviving_line_content(
    repo: &Path,
    parent: &str,
    delivery: &str,
    target: &str,
    path: &str,
) -> Result<Option<super::DeliveryContentPresence>, String> {
    line_content_presence_impl(repo, parent, delivery, target, path, &[], false, &[], true)
}

/// cas-24d8: one close asked for the same ownership walk about 78 times
/// (deliveries × paths × callers), each a full `delivery..target` history
/// walk. The answer depends only on immutable Git objects once every ref is
/// resolved to a commit, so it is memoized by those commits. Errors are not
/// cached: a measurement-budget expiry is not a property of the history.
type WalkKey = (
    std::path::PathBuf,
    String,
    String,
    String,
    String,
    Vec<String>,
    bool,
    Vec<String>,
    bool,
);

static WALKS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<WalkKey, Option<super::DeliveryContentPresence>>>,
> = std::sync::OnceLock::new();

/// Bound on memoized walks; the cache is cleared when it fills.
const WALK_CACHE_LIMIT: usize = 4096;

fn line_content_presence_impl(
    repo: &Path,
    parent: &str,
    delivery: &str,
    target: &str,
    path: &str,
    resolutions: &[String],
    novel_only: bool,
    cycle: &[String],
    allow_partial: bool,
) -> Result<Option<super::DeliveryContentPresence>, String> {
    let commit =
        |reference: &str| super::resolve_branch_sha(repo, &format!("{reference}^{{commit}}"));
    let key = match (commit(parent), commit(delivery), commit(target)) {
        (Some(parent), Some(delivery), Some(target)) => Some((
            repo.to_path_buf(),
            parent,
            delivery,
            target,
            path.to_string(),
            resolutions.to_vec(),
            novel_only,
            cycle.to_vec(),
            allow_partial,
        )),
        _ => None,
    };
    let walks = WALKS.get_or_init(Default::default);
    if let Some(key) = key.as_ref()
        && let Ok(cache) = walks.lock()
        && let Some(answer) = cache.get(key)
    {
        return Ok(answer.clone());
    }
    let answer = line_content_presence_uncached(
        repo,
        parent,
        delivery,
        target,
        path,
        resolutions,
        novel_only,
        cycle,
        allow_partial,
    )?;
    if let Some(key) = key
        && let Ok(mut cache) = walks.lock()
    {
        if cache.len() >= WALK_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key, answer.clone());
    }
    Ok(answer)
}

/// cas-bdd2: a merge's bases, and a base's edits on a path, are the same for
/// every walk that crosses that merge (one per delivery path and caller),
/// so they are memoized by commit ids. Only successful answers are kept: a
/// measurement-budget expiry is not a property of the history.
type BaseKey = (std::path::PathBuf, String, String);
type HunkKey = (std::path::PathBuf, String, String, String);
static MERGE_BASES: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<BaseKey, String>>,
> = std::sync::OnceLock::new();
static BASE_HUNKS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<HunkKey, Vec<Hunk>>>,
> = std::sync::OnceLock::new();

fn cached_merge_bases(repo: &Path, first: &str, second: &str) -> Result<String, String> {
    let key = (repo.to_path_buf(), first.to_string(), second.to_string());
    let cache = MERGE_BASES.get_or_init(Default::default);
    if let Some(bases) = cache.lock().ok().and_then(|cache| cache.get(&key).cloned()) {
        return Ok(bases);
    }
    let bases = text(repo, &["merge-base", "--all", first, second])?;
    if let Ok(mut cache) = cache.lock() {
        if cache.len() >= WALK_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key, bases.clone());
    }
    Ok(bases)
}

fn cached_hunks(repo: &Path, left: &str, right: &str, path: &str) -> Result<Vec<Hunk>, String> {
    let key = (
        repo.to_path_buf(),
        left.to_string(),
        right.to_string(),
        path.to_string(),
    );
    let cache = BASE_HUNKS.get_or_init(Default::default);
    if let Some(found) = cache.lock().ok().and_then(|cache| cache.get(&key).cloned()) {
        return Ok(found);
    }
    let found = hunks(repo, left, right, path)?;
    if let Ok(mut cache) = cache.lock() {
        if cache.len() >= WALK_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key, found.clone());
    }
    Ok(found)
}

/// cas-24d8: the blob each commit holds at `path`, from one batched
/// `cat-file`; `None` for a commit where the path is absent. An edge whose two
/// ends hold the same blob has no hunks, and `advance` maps every owner
/// through an empty change set unchanged, so the walk can skip such an edge
/// (and the commit message read it would have needed) without spawning Git.
/// Any failure returns `None` and the walk measures every edge as before.
fn path_blobs(
    repo: &Path,
    commits: &[&str],
    path: &str,
) -> Option<std::collections::HashMap<String, Option<String>>> {
    use std::io::{Seek, Write};
    super::epic_measurement::check().ok()?;
    if path.contains('\n') {
        return None;
    }
    let mut input = tempfile::tempfile().ok()?;
    for commit in commits {
        writeln!(input, "{commit}:{path}").ok()?;
    }
    input.rewind().ok()?;
    let output = Command::new("git")
        .args(["cat-file", "--batch-check=%(objectname) %(objecttype)"])
        .current_dir(repo)
        .measurement_output_with_stdin(std::process::Stdio::from(input))
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let listing = String::from_utf8(output.stdout).ok()?;
    let lines: Vec<_> = listing.lines().collect();
    if lines.len() != commits.len() {
        return None;
    }
    Some(
        commits
            .iter()
            .zip(lines)
            .map(|(commit, line)| {
                let blob = (!line.ends_with(" missing"))
                    .then(|| line.split_once(' '))
                    .flatten()
                    .filter(|(_, kind)| *kind == "blob")
                    .map(|(id, _)| id.to_string());
                (commit.to_string(), blob)
            })
            .collect(),
    )
}

/// cas-24d8: every changing edge's patch on `path` from one `diff-tree
/// --stdin`. A line `<commit> <prior>` diffs `prior` to `commit`. Only edges
/// whose blobs differ are fed, so each yields exactly one chunk, headed by
/// the commit id; chunks are matched to edges in order and the headers are
/// checked. Under host load the per-edge `git diff` spawns were the close's
/// remaining cost (about 2k per close on the v35 epic). Any mismatch returns
/// `None`, and the walk diffs edge by edge as before.
fn edge_patches(
    repo: &Path,
    edges: &[(&str, &str)],
    path: &str,
) -> Option<std::collections::HashMap<(String, String), String>> {
    use std::io::{Seek, Write};
    if edges.is_empty() {
        return Some(std::collections::HashMap::new());
    }
    super::epic_measurement::check().ok()?;
    let mut input = tempfile::tempfile().ok()?;
    for (prior, commit) in edges {
        writeln!(input, "{commit} {prior}").ok()?;
    }
    input.rewind().ok()?;
    let output = Command::new("git")
        .args([
            "diff-tree",
            "--stdin",
            "-p",
            "--unified=0",
            "--no-renames",
            "--",
            path,
        ])
        .current_dir(repo)
        .measurement_output_with_stdin(std::process::Stdio::from(input))
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let listing = String::from_utf8(output.stdout).ok()?;
    let is_header =
        |line: &str| line.len() == 40 && line.bytes().all(|byte| byte.is_ascii_hexdigit());
    let mut chunks: Vec<(String, String)> = Vec::new();
    for line in listing.lines() {
        if is_header(line) {
            chunks.push((line.to_string(), String::new()));
        } else if let Some((_, patch)) = chunks.last_mut() {
            patch.push_str(line);
            patch.push('\n');
        } else {
            return None;
        }
    }
    if chunks.len() != edges.len() {
        return None;
    }
    let mut patches = std::collections::HashMap::new();
    for ((prior, commit), (header, patch)) in edges.iter().zip(chunks) {
        if !header.eq_ignore_ascii_case(commit) {
            return None;
        }
        patches.insert((prior.to_string(), commit.to_string()), patch);
    }
    Some(patches)
}

/// cas-24d8: the full message of each commit in one `show -s`, keyed by id.
fn commit_messages(
    repo: &Path,
    commits: &[&str],
) -> Option<std::collections::HashMap<String, String>> {
    if commits.is_empty() {
        return Some(std::collections::HashMap::new());
    }
    let mut args = vec!["show", "-s", "--format=%x1e%H%x1f%B"];
    args.extend(commits.iter().copied());
    let listing = text(repo, &args).ok()?;
    let messages: std::collections::HashMap<_, _> = listing
        .split('\u{1e}')
        .filter_map(|record| {
            let (id, message) = record.split_once('\u{1f}')?;
            Some((id.trim().to_string(), message.to_string()))
        })
        .collect();
    commits
        .iter()
        .all(|commit| messages.contains_key(*commit))
        .then_some(messages)
}

fn line_content_presence_uncached(
    repo: &Path,
    parent: &str,
    delivery: &str,
    target: &str,
    path: &str,
    resolutions: &[String],
    novel_only: bool,
    cycle: &[String],
    allow_partial: bool,
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
                .measurement_output()
                .map_err(|error| error.to_string())?;
            if output.status.success() {
                let contents = String::from_utf8(output.stdout)
                    .map_err(|_| "resolution parent is not UTF-8")?;
                inherited.extend(contents.lines().map(|line| line.trim().to_string()));
            }
        }
    }
    let mut owners: Vec<_> = initial
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
    // A deletion-only draft revision has no added owners. Retire it only
    // when every meaningful removed line came from an earlier eligible
    // author in this task cycle. The caller independently proves final
    // handoff content; this empty state cannot prove that content or retire
    // a deletion of baseline/foreign work.
    if owners.is_empty() && cycle.iter().any(|owned| owned == delivery) {
        let mut task_draft = false;
        for hunk in &initial {
            if !hunk.removed.iter().any(|line| meaningful(line)) {
                continue;
            }
            let blame = text(
                repo,
                &[
                    "blame",
                    "--line-porcelain",
                    "-L",
                    &format!("{},+{}", hunk.old_start, hunk.old_count),
                    parent,
                    "--",
                    path,
                ],
            )?;
            let mut author = None;
            for record in blame.lines() {
                if let Some(line) = record.strip_prefix('\t') {
                    if meaningful(line) {
                        if !author.is_some_and(|sha| cycle.iter().any(|owned| owned == sha)) {
                            return Ok(None);
                        }
                        task_draft = true;
                    }
                } else {
                    let fields: Vec<_> = record.split_whitespace().collect();
                    if fields.len() >= 3
                        && fields[0].len() == 40
                        && fields[0].bytes().all(|byte| byte.is_ascii_hexdigit())
                        && fields[1].parse::<usize>().is_ok()
                        && fields[2].parse::<usize>().is_ok()
                    {
                        author = Some(fields[0]);
                    }
                }
            }
        }
        if task_draft {
            owners.push(Some(OwnedLines {
                positions: Vec::new(),
                commits: vec![delivery.to_string()],
                baseline: Vec::new(),
            }));
        }
    }
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
    let mut walked = vec![delivery];
    for record in history.lines() {
        for commit in record.split_whitespace() {
            if !walked.contains(&commit) {
                walked.push(commit);
            }
        }
    }
    let blobs = path_blobs(repo, &walked, path);
    let unchanged = |prior: &str, commit: &str| {
        blobs.as_ref().is_some_and(|blobs| {
            matches!((blobs.get(prior), blobs.get(commit)), (Some(left), Some(right)) if left == right)
        })
    };
    let mut changing_edges = Vec::new();
    let mut changing_commits = Vec::new();
    for record in history.lines() {
        let fields: Vec<_> = record.split_whitespace().collect();
        for prior in fields.iter().skip(1) {
            if !unchanged(prior, fields[0]) {
                changing_edges.push((*prior, fields[0]));
                if !changing_commits.contains(&fields[0]) {
                    changing_commits.push(fields[0]);
                }
            }
        }
    }
    let patches = edge_patches(repo, &changing_edges, path);
    let messages = commit_messages(repo, &changing_commits);
    for record in history.lines() {
        let fields: Vec<_> = record.split_whitespace().collect();
        let commit = fields[0];
        let ordinary = fields.len() == 2;
        // A revert label only matters for an edge that changes the path.
        let reverted = !fields.iter().skip(1).all(|prior| unchanged(prior, commit))
            && match messages.as_ref().and_then(|messages| messages.get(commit)) {
                Some(message) => is_revert_message(message),
                None => is_revert_message(&text(repo, &["show", "-s", "--format=%B", commit])?),
            };
        let mut union_changes = None;
        let mut union_base = None;
        let mut union_checked = false;
        let resolution_parent_lines = if resolutions.iter().any(|resolution| resolution == commit) {
            let mut lines = std::collections::HashSet::new();
            for prior in fields.iter().skip(1) {
                let output = Command::new("git")
                    .args(["show", &format!("{prior}:{path}")])
                    .current_dir(repo)
                    .measurement_output()
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
            if unchanged(prior, commit) {
                // No hunks on this edge: ownership passes through unchanged.
                for (index, owner) in previous.iter().enumerate() {
                    if merged[index].is_none() {
                        merged[index] = owner.clone();
                    }
                }
                continue;
            }
            let batched = patches
                .as_ref()
                .and_then(|patches| patches.get(&(prior.to_string(), commit.to_string())));
            let changes = match (batched, ordinary) {
                (Some(patch), true) => normalize_ordinary(repo, prior, path, parse_hunks(patch)?)?,
                (Some(patch), false) => parse_hunks(patch)?,
                (None, true) => ordinary_hunks(repo, prior, commit, path)?,
                (None, false) => hunks(repo, prior, commit, path)?,
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
                let bases = cached_merge_bases(repo, fields[1], fields[2])?;
                let bases: Vec<_> = bases.lines().collect();
                if let [base] = bases.as_slice() {
                    union_base = Some(base.to_string());
                    union_changes = Some((
                        cached_hunks(repo, base, fields[1], path)?,
                        cached_hunks(repo, base, fields[2], path)?,
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
                            ordinary && cycle.iter().any(|owned| owned == commit),
                            !ordinary
                                && resolution_parent_lines.is_some()
                                && cycle.iter().any(|owned| owned == commit),
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
    let lost = if allow_partial {
        !final_owners.iter().flatten().any(|owner| !owner.positions.is_empty())
    } else {
        final_owners.iter().any(Option::is_none)
    };
    if lost {
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
