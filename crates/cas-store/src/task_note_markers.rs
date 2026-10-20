//! The task timeline is an untrusted text sink. Marker lines are reserved for
//! the explicit trusted append used by artifact publication and attested
//! proposal rendering.

use std::borrow::Cow;

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

fn copy_untrusted(span: &str, out: &mut String) -> bool {
    let mut changed = false;
    for line in span.split_inclusive('\n') {
        if is_reserved_line(line) {
            out.push(' ');
            changed = true;
        }
        out.push_str(line);
    }
    changed
}

/// Indent new marker lines, preserving a pre-existing trusted note span when
/// an ordinary task update carries it forward unchanged.
pub fn neutralize_new_markers<'a>(notes: &'a str, previous: Option<&str>) -> Cow<'a, str> {
    let mut out = String::new();
    let (before, retained, after) = previous
        .filter(|prior| !prior.is_empty())
        .and_then(|prior| notes.find(prior).map(|at| (&notes[..at], prior, &notes[at + prior.len()..])))
        .unwrap_or((notes, "", ""));
    let changed_before = copy_untrusted(before, &mut out);
    out.push_str(retained);
    let changed_after = copy_untrusted(after, &mut out);
    if changed_before || changed_after { Cow::Owned(out) } else { Cow::Borrowed(notes) }
}
