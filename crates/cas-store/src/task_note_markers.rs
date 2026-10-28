//! The task timeline is an untrusted text sink. Marker lines are reserved for
//! the explicit trusted append used by artifact publication and attested
//! proposal rendering.

use std::borrow::Cow;
use std::collections::HashSet;
use std::ops::Range;

const PROVENANCE_BLOCKS: &[(&str, &str)] = &[
    (
        "--- BEGIN SERVER-ATTESTED PROPOSAL PROVENANCE ---",
        "--- END SERVER-ATTESTED PROPOSAL PROVENANCE ---",
    ),
    (
        "--- BEGIN CLIENT-ASSERTED PROPOSAL PROVENANCE ---",
        "--- END CLIENT-ASSERTED PROPOSAL PROVENANCE ---",
    ),
];

pub fn is_reserved_line(line: &str) -> bool {
    line.starts_with("ARTIFACT_PROOF:")
        || PROVENANCE_BLOCKS
            .iter()
            .any(|(begin, end)| line.starts_with(begin) || line.starts_with(end))
}

/// Exact text delimited by line boundaries, including multi-line receipts.
pub(crate) fn contains_complete_lines(notes: &str, text: &str) -> bool {
    std::iter::once(0)
        .chain(notes.match_indices('\n').map(|(offset, _)| offset + 1))
        .any(|start| {
            notes[start..]
                .strip_prefix(text)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('\n'))
        })
}

fn is_timestamped_note(text: &str) -> bool {
    text.starts_with('[')
        && text.get(17..18) == Some("]")
        && text.get(1..17).is_some_and(|timestamp| {
            chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M").is_ok()
        })
}

/// Keep a timestamped timeline entry's paragraphs and proof lines together.
/// Untimestamped preambles and rendered provenance retain paragraph boundaries.
pub fn note_blocks(notes: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut start = 0;
    for (offset, _) in notes.match_indices("\n\n") {
        let next = &notes[offset + 2..];
        if is_timestamped_note(next)
            || PROVENANCE_BLOCKS.iter().any(|(begin, _)| next.starts_with(begin))
            || !is_timestamped_note(notes[start..offset].trim())
        {
            blocks.push(notes[start..offset].trim());
            start = offset + 2;
        }
    }
    blocks.push(notes[start..].trim());
    blocks.into_iter().filter(|block| !block.is_empty()).collect()
}

/// A trusted artifact receipt upgrades only its exact, complete raw copy.
/// Different enriched receipts and unrelated proof lines remain untouched.
pub(crate) fn replace_raw_artifact_note(notes: &str, receipt: &str) -> Option<String> {
    if !is_timestamped_note(receipt)
        || !receipt.lines().any(|line| line.starts_with("ARTIFACT_PROOF:"))
    {
        return None;
    }
    let raw = receipt.lines()
        .filter(|line| !line.starts_with("ARTIFACT_PROOF:"))
        .collect::<Vec<_>>().join("\n");
    let copies = notes.match_indices(&raw)
        .filter_map(|(start, _)| {
            let end = start + raw.len();
            ((start == 0 || notes[..start].ends_with("\n\n"))
                && (end == notes.len() || notes[end..].starts_with("\n\n")))
                .then_some(start..end)
        }).collect::<Vec<_>>();
    if copies.is_empty() {
        return None;
    }
    let mut enriched_exists = contains_complete_lines(notes, receipt);
    let mut next = notes.to_string();
    for mut range in copies.into_iter().rev() {
        if enriched_exists {
            if next[range.end..].starts_with("\n\n") {
                range.end += 2;
            } else if range.start >= 2 {
                range.start -= 2;
            }
            next.replace_range(range, "");
        } else {
            next.replace_range(range, receipt);
            enriched_exists = true;
        }
    }
    Some(next)
}

fn provenance_blocks(notes: &str) -> Vec<Range<usize>> {
    let mut blocks = Vec::new();
    let mut open = None;
    let mut offset = 0;
    for line in notes.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        if let Some((_, end)) = PROVENANCE_BLOCKS.iter().find(|(begin, _)| *begin == text) {
            open = Some((offset, *end));
        } else if let Some((start, end)) = open {
            if text == end {
                blocks.push(start..offset + line.trim_end_matches('\n').len());
                open = None;
            } else if is_reserved_line(text) {
                open = None;
            }
        }
        offset += line.len();
    }
    blocks
}

pub fn replace_rendered_provenance(notes: &str, rendered: &str) -> String {
    let retained = notes
        .split("\n\n")
        .filter(|paragraph| {
            let paragraph = paragraph.trim();
            !((paragraph.starts_with("--- BEGIN SERVER-ATTESTED PROPOSAL PROVENANCE ---")
                && paragraph.ends_with("--- END SERVER-ATTESTED PROPOSAL PROVENANCE ---"))
                || (paragraph.starts_with("--- BEGIN CLIENT-ASSERTED PROPOSAL PROVENANCE ---")
                    && paragraph.ends_with("--- END CLIENT-ASSERTED PROPOSAL PROVENANCE ---")))
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if retained.trim().is_empty() {
        rendered.to_string()
    } else {
        format!("{}\n\n{rendered}", retained.trim_end())
    }
}

/// Preserve stored artifact lines and whole, unchanged provenance blocks.
/// Pull can reorder note blocks, and a concurrent append can change their
/// surrounding text, so prior notes cannot be matched as one contiguous span.
pub fn neutralize_new_markers<'a>(notes: &'a str, previous: Option<&str>) -> Cow<'a, str> {
    let prior_markers: HashSet<&str> = previous
        .into_iter()
        .flat_map(str::lines)
        .filter(|line| line.starts_with("ARTIFACT_PROOF:"))
        .collect();
    let preserved_blocks = provenance_blocks(notes)
        .into_iter()
        .filter(|range| {
            previous.is_some_and(|prior| contains_complete_lines(prior, &notes[range.clone()]))
        })
        .collect::<Vec<_>>();
    let mut out = String::new();
    let mut changed = false;
    let mut offset = 0;
    for line in notes.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        if is_reserved_line(text)
            && !prior_markers.contains(text)
            && !preserved_blocks.iter().any(|range| range.contains(&offset))
        {
            out.push(' ');
            changed = true;
        }
        out.push_str(line);
        offset += line.len();
    }
    if changed { Cow::Owned(out) } else { Cow::Borrowed(notes) }
}
