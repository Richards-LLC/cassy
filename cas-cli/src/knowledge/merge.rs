//! Page body composition, cost-tiered merging and provenance surgery
//! (EPIC cas-7d31 / cas-c9be).
//!
//! A distilled body is frontmatter plus a sequence of *fragments*, each
//! introduced by a machine-readable provenance marker:
//!
//! ```text
//! <!-- cas:sources ["README.md"] -->
//! ```
//!
//! That marker is what makes cascade delete cheap and exact: when a source dies,
//! its fragments are cut out of every page that cites it without asking an LLM
//! anything. Text before the first marker is treated as hand-written and is
//! never touched.
//!
//! Merging is cost-tiered, most passes costing nothing:
//!
//! - **(a) normalized containment** — the incoming text is already stated by the
//!   page: union the provenance, keep the body byte-identical, no LLM call.
//! - **(b) small page** — rewrite the whole page so it reads as one voice
//!   (one LLM call, and it falls back to (c) if the call fails).
//! - **(c) large page** — append the new material as its own fragment. The old
//!   body is preserved verbatim; nothing is ever silently lost.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

/// Marker prefix introducing a provenance fragment.
pub const MARKER_PREFIX: &str = "<!-- cas:sources ";
const MARKER_SUFFIX: &str = " -->";

/// Pages at or below this many characters are cheap enough to rewrite whole.
pub const DEFAULT_SMALL_PAGE_CHARS: usize = 2_000;

/// Which merge strategy a page/incoming pair earns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeTier {
    /// (a) Already covered — only provenance changes. Costs nothing.
    UnionSourcesOnly,
    /// (b) Small page — rewrite the whole thing with one LLM call.
    FullRewrite,
    /// (c) Large page — append a delta fragment, old body verbatim.
    AppendDelta,
}

/// One provenance-tagged span of a page body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    /// Sources this span was distilled from. Empty = hand-written preamble.
    pub sources: Vec<String>,
    /// The span's markdown, without its marker line.
    pub text: String,
}

impl Fragment {
    pub fn new(sources: Vec<String>, text: impl Into<String>) -> Self {
        Self {
            sources,
            text: text.into(),
        }
    }
}

/// Frontmatter fields CAS owns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frontmatter {
    pub title: String,
    pub page_type: String,
    pub sources: Vec<String>,
    pub locked: bool,
    pub updated: Option<String>,
}

/// Split a body into `(frontmatter_block_without_fences, rest)`.
pub fn split_frontmatter(body: &str) -> (Option<&str>, &str) {
    let trimmed = body.strip_prefix('\u{feff}').unwrap_or(body);
    if !trimmed.starts_with("---") {
        return (None, body);
    }
    let after_open = match trimmed.find('\n') {
        Some(index) => &trimmed[index + 1..],
        None => return (None, body),
    };
    let mut offset = 0usize;
    for line in after_open.split_inclusive('\n') {
        if line.trim_end() == "---" {
            let block = &after_open[..offset];
            let rest = &after_open[offset + line.len()..];
            return (Some(block), rest.trim_start_matches('\n'));
        }
        offset += line.len();
    }
    (None, body)
}

/// Parse the frontmatter fields CAS writes. Deliberately a tiny reader, not a
/// YAML engine: unknown keys are ignored, and a malformed block simply yields
/// defaults (which means `locked` stays false only if it is not asserted).
pub fn parse_frontmatter(body: &str) -> Frontmatter {
    let (Some(block), _) = split_frontmatter(body) else {
        return Frontmatter::default();
    };
    let mut frontmatter = Frontmatter::default();
    let mut in_sources = false;

    for line in block.lines() {
        let trimmed = line.trim();
        if in_sources {
            if let Some(item) = trimmed.strip_prefix("- ") {
                frontmatter.sources.push(unquote(item.trim()).to_string());
                continue;
            }
            in_sources = false;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "title" => frontmatter.title = unquote(value).to_string(),
            "type" => frontmatter.page_type = unquote(value).to_string(),
            "locked" => frontmatter.locked = value.eq_ignore_ascii_case("true"),
            "updated" => frontmatter.updated = Some(unquote(value).to_string()),
            "sources" => {
                if value.is_empty() {
                    in_sources = true;
                } else if let Ok(list) = serde_json::from_str::<Vec<String>>(value) {
                    frontmatter.sources = list;
                }
            }
            _ => {}
        }
    }
    frontmatter
}

/// Has a human (or agent) claimed this page? A locked page is never rewritten.
pub fn is_locked_body(body: &str) -> bool {
    parse_frontmatter(body).locked
}

fn unquote(value: &str) -> &str {
    let value = value.trim();
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

/// Render CAS-owned frontmatter. `locked` is always written as the value the
/// store holds; distillation never writes `locked: true` on its own.
pub fn render_frontmatter(meta: &Frontmatter) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("title: {}\n", yaml_scalar(&meta.title)));
    out.push_str(&format!("type: {}\n", yaml_scalar(&meta.page_type)));
    out.push_str("sources:\n");
    for source in &meta.sources {
        out.push_str(&format!("  - {}\n", yaml_scalar(source)));
    }
    if let Some(updated) = &meta.updated {
        out.push_str(&format!("updated: {updated}\n"));
    }
    out.push_str(&format!("locked: {}\n", meta.locked));
    out.push_str("---\n");
    out
}

fn yaml_scalar(value: &str) -> String {
    if value.is_empty()
        || value.contains(':')
        || value.contains('#')
        || value.starts_with(' ')
        || value.ends_with(' ')
    {
        serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""))
    } else {
        value.to_string()
    }
}

/// Marker line for a fragment.
pub fn fragment_marker(sources: &[String]) -> String {
    let json = serde_json::to_string(sources).unwrap_or_else(|_| "[]".to_string());
    format!("{MARKER_PREFIX}{json}{MARKER_SUFFIX}")
}

fn parse_marker(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    let inner = trimmed
        .strip_prefix(MARKER_PREFIX)?
        .strip_suffix(MARKER_SUFFIX)?;
    serde_json::from_str::<Vec<String>>(inner.trim()).ok()
}

/// Split a body (frontmatter already removed or not) into fragments.
pub fn split_fragments(body: &str) -> Vec<Fragment> {
    let (_, content) = split_frontmatter(body);
    let mut fragments: Vec<Fragment> = Vec::new();
    let mut current = Fragment::new(Vec::new(), String::new());
    let mut started = false;

    for line in content.lines() {
        if let Some(sources) = parse_marker(line) {
            if started || !current.text.trim().is_empty() {
                current.text = current.text.trim().to_string();
                if !current.text.is_empty() || !current.sources.is_empty() {
                    fragments.push(std::mem::replace(
                        &mut current,
                        Fragment::new(Vec::new(), String::new()),
                    ));
                }
            }
            current = Fragment::new(sources, String::new());
            started = true;
            continue;
        }
        current.text.push_str(line);
        current.text.push('\n');
    }

    current.text = current.text.trim().to_string();
    if !current.text.is_empty() || !current.sources.is_empty() {
        fragments.push(current);
    }
    fragments
}

/// Render fragments back to a body (without frontmatter).
pub fn render_fragments(fragments: &[Fragment]) -> String {
    let mut out = String::new();
    for fragment in fragments {
        if !fragment.sources.is_empty() {
            out.push_str(&fragment_marker(&fragment.sources));
            out.push('\n');
        }
        out.push_str(fragment.text.trim());
        out.push_str("\n\n");
    }
    out.trim_end().to_string()
}

/// Compose a full page body: frontmatter + fragments.
pub fn compose_body(meta: &Frontmatter, fragments: &[Fragment]) -> String {
    format!(
        "{}\n{}\n",
        render_frontmatter(meta),
        render_fragments(fragments)
    )
}

/// Whitespace/case-insensitive normalization used by the containment check.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = true;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
            last_space = false;
        } else {
            // Punctuation is dropped: "the store." and "the store" are the same
            // claim, and an LLM rewording punctuation must not re-bill a merge.
            last_space = false;
        }
    }
    out.trim().to_string()
}

/// Pick the merge tier for an existing body vs. incoming text.
pub fn choose_tier(existing_body: &str, incoming: &str, small_page_chars: usize) -> MergeTier {
    let existing_normalized = normalize(&fragments_text(existing_body));
    let incoming_normalized = normalize(incoming);

    if incoming_normalized.is_empty() || existing_normalized.contains(&incoming_normalized) {
        return MergeTier::UnionSourcesOnly;
    }
    if existing_body.chars().count() <= small_page_chars {
        MergeTier::FullRewrite
    } else {
        MergeTier::AppendDelta
    }
}

/// The prose of a body with frontmatter and markers stripped.
pub fn fragments_text(body: &str) -> String {
    split_fragments(body)
        .into_iter()
        .map(|fragment| fragment.text)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Tier (c): append `incoming` as its own provenance-tagged fragment, leaving
/// every existing fragment byte-identical.
pub fn append_delta(
    existing_body: &str,
    incoming: &str,
    source: &str,
    at: DateTime<Utc>,
) -> Vec<Fragment> {
    let mut fragments = split_fragments(existing_body);
    let text = format!(
        "## Update from `{source}` ({})\n\n{}",
        at.format("%Y-%m-%d"),
        incoming.trim()
    );
    fragments.push(Fragment::new(vec![source.to_string()], text));
    fragments
}

/// Union of two source lists, sorted and deduplicated so page provenance is
/// order-independent (a page's sources are a set, not a history).
pub fn union_sources(existing: &[String], incoming: &[String]) -> Vec<String> {
    let set: BTreeSet<&String> = existing.iter().chain(incoming.iter()).collect();
    set.into_iter().cloned().collect()
}

/// What removing a source did to a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StripOutcome {
    /// The page never cited that source in any fragment.
    Unchanged,
    /// Fragments were cut and/or provenance narrowed.
    Rewritten {
        body: String,
        /// A surviving fragment was co-authored by the dead source, so its
        /// prose may still describe it. Provenance is exact; prose is not —
        /// the page is flagged for re-distillation on the next pass.
        needs_redistill: bool,
    },
}

/// Cut every fragment belonging solely to `source`, and narrow the provenance
/// of fragments it merely co-authored. Frontmatter `sources` is updated to
/// match. No LLM call.
pub fn strip_source(body: &str, source: &str) -> StripOutcome {
    let fragments = split_fragments(body);
    let mut kept: Vec<Fragment> = Vec::new();
    let mut changed = false;
    let mut needs_redistill = false;

    for fragment in fragments {
        if !fragment.sources.iter().any(|s| s == source) {
            kept.push(fragment);
            continue;
        }
        changed = true;
        if fragment.sources.len() == 1 {
            continue; // sole author: the span goes with it
        }
        let mut narrowed = fragment.clone();
        narrowed.sources.retain(|s| s != source);
        needs_redistill = true;
        kept.push(narrowed);
    }

    if !changed {
        return StripOutcome::Unchanged;
    }

    let mut meta = parse_frontmatter(body);
    meta.sources.retain(|s| s != source);
    if meta.sources.is_empty() {
        meta.sources = kept
            .iter()
            .flat_map(|fragment| fragment.sources.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
    }

    StripOutcome::Rewritten {
        body: compose_body(&meta, &kept),
        needs_redistill,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> Frontmatter {
        Frontmatter {
            title: "Build System".to_string(),
            page_type: "architecture".to_string(),
            sources: vec!["README.md".to_string()],
            locked: false,
            updated: Some("2026-08-06T00:00:00Z".to_string()),
        }
    }

    #[test]
    fn frontmatter_round_trips() {
        let body = compose_body(&meta(), &[Fragment::new(vec!["README.md".into()], "Text.")]);
        let parsed = parse_frontmatter(&body);
        assert_eq!(parsed.title, "Build System");
        assert_eq!(parsed.page_type, "architecture");
        assert_eq!(parsed.sources, vec!["README.md".to_string()]);
        assert!(!parsed.locked);
    }

    #[test]
    fn a_hand_locked_page_is_detected() {
        let body = "---\ntitle: Mine\nlocked: true\n---\n\nHands off.\n";
        assert!(is_locked_body(body));
        assert!(!is_locked_body(
            "---\ntitle: Mine\nlocked: false\n---\n\nok\n"
        ));
        assert!(!is_locked_body("no frontmatter at all"));
    }

    #[test]
    fn fragments_round_trip_through_markers() {
        let fragments = vec![
            Fragment::new(vec!["a.md".into()], "First span."),
            Fragment::new(vec!["b.md".into(), "c.md".into()], "Second span."),
        ];
        let body = compose_body(&meta(), &fragments);
        assert_eq!(split_fragments(&body), fragments);
    }

    #[test]
    fn handwritten_preamble_has_no_provenance_and_survives() {
        let body = "Hand written intro.\n\n<!-- cas:sources [\"a.md\"] -->\nDistilled.\n";
        let fragments = split_fragments(body);
        assert_eq!(fragments.len(), 2);
        assert!(fragments[0].sources.is_empty());
        assert_eq!(fragments[0].text, "Hand written intro.");

        match strip_source(body, "a.md") {
            StripOutcome::Rewritten { body, .. } => {
                assert!(body.contains("Hand written intro."));
                assert!(!body.contains("Distilled."));
            }
            other => panic!("expected rewrite, got {other:?}"),
        }
    }

    #[test]
    fn tier_a_fires_when_the_page_already_says_it() {
        let body = compose_body(
            &meta(),
            &[Fragment::new(
                vec!["a.md".into()],
                "The store is transactional.",
            )],
        );
        assert_eq!(
            choose_tier(
                &body,
                "the store is transactional!",
                DEFAULT_SMALL_PAGE_CHARS
            ),
            MergeTier::UnionSourcesOnly
        );
        assert_eq!(
            choose_tier(&body, "", DEFAULT_SMALL_PAGE_CHARS),
            MergeTier::UnionSourcesOnly
        );
    }

    #[test]
    fn tier_b_and_c_split_on_page_size() {
        let small = compose_body(&meta(), &[Fragment::new(vec!["a.md".into()], "Short.")]);
        assert_eq!(
            choose_tier(&small, "Brand new claim.", DEFAULT_SMALL_PAGE_CHARS),
            MergeTier::FullRewrite
        );

        let large = compose_body(
            &meta(),
            &[Fragment::new(vec!["a.md".into()], "x ".repeat(2_000))],
        );
        assert_eq!(
            choose_tier(&large, "Brand new claim.", DEFAULT_SMALL_PAGE_CHARS),
            MergeTier::AppendDelta
        );
    }

    #[test]
    fn append_delta_preserves_the_old_body_verbatim() {
        let original = compose_body(
            &meta(),
            &[Fragment::new(vec!["a.md".into()], "Original prose.")],
        );
        let fragments = append_delta(&original, "New material.", "b.md", Utc::now());
        assert_eq!(fragments.len(), 2);
        assert_eq!(fragments[0].text, "Original prose.");
        assert!(fragments[1].text.contains("New material."));
        assert_eq!(fragments[1].sources, vec!["b.md".to_string()]);
    }

    #[test]
    fn stripping_a_sole_author_removes_its_span_only() {
        let body = compose_body(
            &Frontmatter {
                sources: vec!["a.md".into(), "b.md".into()],
                ..meta()
            },
            &[
                Fragment::new(vec!["a.md".into()], "From A."),
                Fragment::new(vec!["b.md".into()], "From B."),
            ],
        );
        match strip_source(&body, "a.md") {
            StripOutcome::Rewritten {
                body,
                needs_redistill,
            } => {
                assert!(!body.contains("From A."));
                assert!(body.contains("From B."));
                assert!(!needs_redistill);
                assert_eq!(parse_frontmatter(&body).sources, vec!["b.md".to_string()]);
            }
            other => panic!("expected rewrite, got {other:?}"),
        }
    }

    #[test]
    fn stripping_a_co_author_narrows_provenance_and_flags_redistill() {
        let body = compose_body(
            &Frontmatter {
                sources: vec!["a.md".into(), "b.md".into()],
                ..meta()
            },
            &[Fragment::new(
                vec!["a.md".into(), "b.md".into()],
                "Merged prose from both.",
            )],
        );
        match strip_source(&body, "a.md") {
            StripOutcome::Rewritten {
                body,
                needs_redistill,
            } => {
                assert!(body.contains("Merged prose from both."));
                assert!(needs_redistill, "prose may still describe the dead source");
                assert_eq!(split_fragments(&body)[0].sources, vec!["b.md".to_string()]);
            }
            other => panic!("expected rewrite, got {other:?}"),
        }
    }

    #[test]
    fn stripping_an_uncited_source_is_a_no_op() {
        let body = compose_body(&meta(), &[Fragment::new(vec!["a.md".into()], "Text.")]);
        assert_eq!(strip_source(&body, "zzz.md"), StripOutcome::Unchanged);
    }

    #[test]
    fn provenance_is_a_sorted_set() {
        let union = union_sources(
            &["b.md".to_string(), "a.md".to_string()],
            &["a.md".to_string(), "c.md".to_string()],
        );
        assert_eq!(union, vec!["a.md", "b.md", "c.md"]);
    }

    #[test]
    fn yaml_scalars_with_colons_are_quoted() {
        let meta = Frontmatter {
            title: "A: B".to_string(),
            page_type: "guide".to_string(),
            sources: vec!["docs/a: b.md".to_string()],
            locked: false,
            updated: None,
        };
        let rendered = render_frontmatter(&meta);
        let parsed = parse_frontmatter(&format!("{rendered}\nbody\n"));
        assert_eq!(parsed.title, "A: B");
        assert_eq!(parsed.sources, vec!["docs/a: b.md".to_string()]);
    }
}
