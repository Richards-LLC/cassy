//! The task timeline is an untrusted text sink. Marker lines are reserved for
//! the explicit trusted append used by artifact publication and attested
//! proposal rendering.

use std::borrow::Cow;
use std::collections::HashSet;

const RESERVED: &[&str] = &[
    "ARTIFACT_PROOF:",
    "--- BEGIN SERVER-ATTESTED PROPOSAL PROVENANCE ---",
    "--- END SERVER-ATTESTED PROPOSAL PROVENANCE ---",
    "--- BEGIN CLIENT-ASSERTED PROPOSAL PROVENANCE ---",
    "--- END CLIENT-ASSERTED PROPOSAL PROVENANCE ---",
];

pub fn is_reserved_line(line: &str) -> bool {
    RESERVED.iter().any(|marker| line.starts_with(marker))
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

/// Indent new marker lines while preserving exact marker lines already stored.
/// Pull can reorder note blocks, and a concurrent append can change their
/// surrounding text, so prior notes cannot be matched as one contiguous span.
pub fn neutralize_new_markers<'a>(notes: &'a str, previous: Option<&str>) -> Cow<'a, str> {
    let prior_markers: HashSet<&str> = previous
        .into_iter()
        .flat_map(str::lines)
        .filter(|line| is_reserved_line(line))
        .collect();
    let mut out = String::new();
    let mut changed = false;
    for line in notes.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        if is_reserved_line(text) && !prior_markers.contains(text) {
            out.push(' ');
            changed = true;
        }
        out.push_str(line);
    }
    if changed { Cow::Owned(out) } else { Cow::Borrowed(notes) }
}
