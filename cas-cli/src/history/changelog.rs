//! CHANGELOG release-section parser (EPIC cas-6212 / cas-9a38, spec §8 + §11 M6).
//!
//! Turns `CHANGELOG.md` into one [`HistoryDoc`] per release section, keyed
//! `changelog:v2.49.0`, and maps each section to the **git tag range it
//! describes** — which is the whole point of parsing it. "What shipped in
//! 2.49.0" is a CHANGELOG question; "which commits shipped in 2.49.0" is a git
//! question; the tag range is the join between them, and without it the
//! CHANGELOG is just more prose in the corpus.
//!
//! # Ranges are resolved against real tags, never invented
//!
//! A section's range is `<previous-release-tag>..<this-release-tag>`, and both
//! ends must exist in the repository's actual tag list. A version with no
//! corresponding tag (an entry written before the release was cut, or a
//! renamed tag scheme) gets **no range** rather than a plausible-looking one —
//! a fabricated range would resolve to a commit set that silently belongs to
//! some other release. The oldest section likewise has no lower bound, so it
//! gets a tag but no range.
//!
//! `## [Unreleased]` is indexed like any other section, with the open range
//! `<latest-tag>..HEAD`. Omitting it would drop exactly the material most
//! likely to be asked about.
//!
//! # Why the file is re-parsed in full every pass
//!
//! There is no watermark here. The file is ~150 KB, parsing is a single linear
//! scan, and an in-place edit to an old section changes no timestamp that a
//! watermark could observe. Re-parsing is cheaper than being wrong, and the
//! store's change detection ([`cas_store::HistoryStore::upsert_docs`]) already
//! ensures an unchanged section costs nothing downstream: only sections whose
//! text actually moved are re-queued for embedding.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};
use cas_store::{DOC_KIND_CHANGELOG, HistoryDoc, SOURCE_CHANGELOG};

use super::refs::{DocRefs, extract_from_text};

/// One parsed release section, before it becomes a [`HistoryDoc`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseSection {
    /// Version as written in the heading (`2.49.0`, or `Unreleased`).
    pub version: String,
    /// Release date from the heading, when present (`2026-08-07`).
    pub date: Option<String>,
    /// Section text, heading excluded, trailing blank lines trimmed.
    pub body: String,
    /// True for the `[Unreleased]` section.
    pub unreleased: bool,
}

/// A section joined to the repository's tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSection {
    pub section: ReleaseSection,
    /// The tag naming this release, if the repository actually has one.
    pub tag: Option<String>,
    /// `<previous>..<this>`, or `<latest>..HEAD` for `[Unreleased]`. `None`
    /// when either end is unresolvable.
    pub tag_range: Option<String>,
}

/// Split a CHANGELOG into release sections, newest first (file order).
///
/// Fenced code blocks are tracked so a ```` ```markdown ```` example containing
/// a `## [1.0.0]` line cannot invent a release.
pub fn parse_sections(text: &str) -> Vec<ReleaseSection> {
    let mut sections: Vec<ReleaseSection> = Vec::new();
    let mut current: Option<(String, Option<String>, bool, Vec<&str>)> = None;
    let mut fence: Option<String> = None;

    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(open) = trimmed
            .starts_with("```")
            .then(|| trimmed.chars().take_while(|c| *c == '`').collect::<String>())
        {
            match &fence {
                // A closing fence must be at least as long as the opener
                // (CommonMark), which is what makes nested examples survivable.
                Some(existing) if open.len() >= existing.len() => fence = None,
                Some(_) => {}
                None => fence = Some(open),
            }
        }

        if fence.is_none()
            && let Some(heading) = line.strip_prefix("## ")
            && let Some((version, date)) = parse_heading(heading)
        {
            if let Some((v, d, u, body)) = current.take() {
                sections.push(finish(v, d, u, &body));
            }
            let unreleased = version.eq_ignore_ascii_case("unreleased");
            current = Some((version, date, unreleased, Vec::new()));
            continue;
        }

        if let Some((_, _, _, body)) = current.as_mut() {
            body.push(line);
        }
    }
    if let Some((v, d, u, body)) = current.take() {
        sections.push(finish(v, d, u, &body));
    }
    sections
}

fn finish(version: String, date: Option<String>, unreleased: bool, body: &[&str]) -> ReleaseSection {
    ReleaseSection {
        version,
        date,
        body: body.join("\n").trim().to_string(),
        unreleased,
    }
}

/// `[2.49.0] - 2026-08-07`, `[Unreleased]`, or the unbracketed `2.49.0 - date`.
fn parse_heading(heading: &str) -> Option<(String, Option<String>)> {
    let heading = heading.trim();
    let (version, rest) = match heading.strip_prefix('[') {
        Some(rest) => {
            let (v, tail) = rest.split_once(']')?;
            (v.trim().to_string(), tail)
        }
        None => {
            let mut parts = heading.splitn(2, [' ', '\t']);
            let v = parts.next()?.trim().to_string();
            (v, parts.next().unwrap_or(""))
        }
    };
    if version.is_empty() {
        return None;
    }
    if !version.eq_ignore_ascii_case("unreleased")
        && !version.starts_with(|c: char| c.is_ascii_digit() || c == 'v')
    {
        return None;
    }
    let date = rest
        .trim()
        .trim_start_matches(['-', '–', '—'])
        .trim()
        .split_whitespace()
        .next()
        .filter(|d| d.len() == 10 && d.as_bytes()[4] == b'-' && d.as_bytes()[7] == b'-')
        .map(str::to_string);
    Some((version, date))
}

/// Which tag names this version, given the repository's real tag set.
fn tag_for(version: &str, tags: &BTreeSet<String>) -> Option<String> {
    let bare = version.trim_start_matches('v');
    for candidate in [format!("v{bare}"), bare.to_string()] {
        if tags.contains(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Join parsed sections to the tag list, computing each section's range.
///
/// `sections` must be newest-first (the order [`parse_sections`] returns for a
/// conventional Keep-a-Changelog file).
pub fn resolve_ranges(sections: Vec<ReleaseSection>, tags: &BTreeSet<String>) -> Vec<ResolvedSection> {
    let resolved_tags: Vec<Option<String>> = sections
        .iter()
        .map(|s| (!s.unreleased).then(|| tag_for(&s.version, tags)).flatten())
        .collect();

    sections
        .into_iter()
        .enumerate()
        .map(|(idx, section)| {
            // The previous release is the next *tagged* section below this one;
            // skipping untagged sections keeps a gap from silently widening a
            // range across a release nobody tagged.
            let previous = resolved_tags[idx + 1..].iter().flatten().next();
            let tag = resolved_tags[idx].clone();
            let tag_range = if section.unreleased {
                previous.map(|prev| format!("{prev}..HEAD"))
            } else {
                match (&tag, previous) {
                    (Some(this), Some(prev)) => Some(format!("{prev}..{this}")),
                    _ => None,
                }
            };
            ResolvedSection {
                section,
                tag,
                tag_range,
            }
        })
        .collect()
}

/// Turn a resolved section into the row that gets stored.
pub fn to_doc(resolved: &ResolvedSection, repository: &str) -> HistoryDoc {
    let version = &resolved.section.version;
    let slug = if resolved.section.unreleased {
        "unreleased".to_string()
    } else {
        resolved
            .tag
            .clone()
            .unwrap_or_else(|| format!("v{}", version.trim_start_matches('v')))
    };

    let mut refs: DocRefs = extract_from_text(&resolved.section.body);
    refs.tag = resolved.tag.clone();
    refs.tag_range = resolved.tag_range.clone();

    // The release date is both `created_at` and `updated_at`: a CHANGELOG
    // section has exactly one timestamp, and inventing a distinct "updated"
    // moment would be fabricating a fact the file does not carry.
    let at = resolved
        .section
        .date
        .as_ref()
        .map(|d| format!("{d}T00:00:00Z"));

    HistoryDoc {
        id: format!("changelog:{slug}"),
        doc_kind: DOC_KIND_CHANGELOG.to_string(),
        number: None,
        title: Some(version.clone()),
        body: (!resolved.section.body.is_empty()).then(|| resolved.section.body.clone()),
        state: Some(if resolved.section.unreleased {
            "unreleased".to_string()
        } else {
            "released".to_string()
        }),
        author: None,
        created_at: at.clone(),
        updated_at: at,
        closed_at: None,
        url: None,
        refs_json: refs.to_json(),
        repository: repository.to_string(),
        source: SOURCE_CHANGELOG.to_string(),
    }
}

/// Read the repository's tag list. An empty set is a legitimate answer (a fresh
/// clone with no tags) and produces ranges of `None`, never invented ones.
pub fn repo_tags(repo_root: &Path) -> BTreeSet<String> {
    std::process::Command::new("git")
        .args(["tag", "--list"])
        .current_dir(repo_root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Where the CHANGELOG lives, if it exists. Absence is a declared boundary
/// (spec §10.2), not an error: plenty of repositories have no CHANGELOG.
pub fn changelog_path(repo_root: &Path) -> Option<std::path::PathBuf> {
    ["CHANGELOG.md", "CHANGELOG.markdown", "CHANGELOG"]
        .iter()
        .map(|name| repo_root.join(name))
        .find(|p| p.is_file())
}

/// Parse the repository's CHANGELOG into storable docs.
///
/// `Ok(None)` means there is no CHANGELOG — reported by the caller as a
/// boundary, not swallowed and not raised as a failure.
///
/// The file is read as bytes and decoded through
/// [`crate::daemon::source_text::decode_source`] rather than demanding UTF-8
/// up front (GH #698, extended by cas-c736). A CHANGELOG that has been through
/// a Windows editor is commonly UTF-16 or UTF-8-with-BOM; `read_to_string`
/// failed the whole changelog pass on the first with an encoding message that
/// named no remedy, and silently glued the BOM onto the second's first heading
/// so its opening release parsed as prose. A file we genuinely cannot decode is
/// reported by name and reason, which the caller records as the pass's
/// `changelog_error`.
pub fn collect(repo_root: &Path, repository: &str) -> Result<Option<Vec<HistoryDoc>>> {
    let Some(path) = changelog_path(repo_root) else {
        return Ok(None);
    };
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let text = crate::daemon::source_text::decode_source(&bytes).map_err(|reason| {
        anyhow::anyhow!(
            "skipped {}: {}",
            path.display(),
            reason.as_str()
        )
    })?;
    let tags = repo_tags(repo_root);
    let resolved = resolve_ranges(parse_sections(&text), &tags);
    Ok(Some(
        resolved.iter().map(|r| to_doc(r, repository)).collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# Changelog

Preamble prose.

## [Unreleased]

### Added
- something new

## [2.51.0] - 2026-08-07

### Fixed
- fixed a thing in 58084e5a, closes #155

## [2.50.0] - 2026-08-06

### Removed
- removed a thing

## [2.49.0] - 2026-08-05

Oldest section.
";

    fn tags() -> BTreeSet<String> {
        ["v2.49.0", "v2.50.0", "v2.51.0"]
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn sections_split_on_release_headings_only() {
        let sections = parse_sections(SAMPLE);
        let versions: Vec<&str> = sections.iter().map(|s| s.version.as_str()).collect();
        assert_eq!(versions, ["Unreleased", "2.51.0", "2.50.0", "2.49.0"]);
        assert_eq!(sections[1].date.as_deref(), Some("2026-08-07"));
        assert!(sections[0].unreleased);
        assert!(sections[1].body.contains("fixed a thing"));
        assert!(
            !sections[1].body.contains("2.50.0"),
            "a section must stop at the next heading"
        );
        assert!(
            !sections[0].body.starts_with('\n'),
            "leading blank lines must be trimmed"
        );
    }

    #[test]
    fn a_heading_inside_a_fenced_block_is_not_a_release() {
        let text = "## [1.0.0] - 2026-01-01\n\nExample:\n\n```markdown\n## [9.9.9] - 2099-01-01\n```\n\nstill 1.0.0\n";
        let sections = parse_sections(text);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].version, "1.0.0");
        assert!(sections[0].body.contains("still 1.0.0"));
    }

    #[test]
    fn ranges_span_adjacent_tags_and_unreleased_runs_to_head() {
        let resolved = resolve_ranges(parse_sections(SAMPLE), &tags());
        assert_eq!(resolved[0].tag_range.as_deref(), Some("v2.51.0..HEAD"));
        assert_eq!(
            resolved[1].tag_range.as_deref(),
            Some("v2.50.0..v2.51.0"),
            "2.51.0 covers everything since the previous tag"
        );
        assert_eq!(resolved[2].tag_range.as_deref(), Some("v2.49.0..v2.50.0"));
        assert_eq!(
            resolved[3].tag_range, None,
            "the oldest section has no lower bound and must not invent one"
        );
        assert_eq!(resolved[3].tag.as_deref(), Some("v2.49.0"));
    }

    /// The failure this guards: a version with no tag getting a range anyway,
    /// which would attribute another release's commits to it.
    #[test]
    fn an_untagged_version_gets_no_range() {
        let mut partial = tags();
        partial.remove("v2.51.0");
        let resolved = resolve_ranges(parse_sections(SAMPLE), &partial);

        assert_eq!(resolved[1].tag, None);
        assert_eq!(resolved[1].tag_range, None, "{:?}", resolved[1]);
        // Unreleased and 2.50.0 skip the untagged release rather than pretend
        // it is not there.
        assert_eq!(resolved[0].tag_range.as_deref(), Some("v2.50.0..HEAD"));
        assert_eq!(resolved[2].tag_range.as_deref(), Some("v2.49.0..v2.50.0"));
    }

    #[test]
    fn no_tags_at_all_yields_no_ranges() {
        let resolved = resolve_ranges(parse_sections(SAMPLE), &BTreeSet::new());
        assert!(resolved.iter().all(|r| r.tag_range.is_none() && r.tag.is_none()));
    }

    #[test]
    fn docs_carry_the_range_and_the_scraped_references() {
        let resolved = resolve_ranges(parse_sections(SAMPLE), &tags());
        let doc = to_doc(&resolved[1], "/repo");
        assert_eq!(doc.id, "changelog:v2.51.0");
        assert_eq!(doc.doc_kind, "changelog");
        assert_eq!(doc.updated_at.as_deref(), Some("2026-08-07T00:00:00Z"));
        assert_eq!(doc.state.as_deref(), Some("released"));

        let refs: DocRefs = serde_json::from_str(doc.refs_json.as_deref().unwrap()).unwrap();
        assert_eq!(refs.tag_range.as_deref(), Some("v2.50.0..v2.51.0"));
        assert_eq!(refs.tag.as_deref(), Some("v2.51.0"));
        assert_eq!(refs.issues, vec![155]);
        assert_eq!(refs.commits, vec!["58084e5a".to_string()]);

        assert_eq!(to_doc(&resolved[0], "/repo").id, "changelog:unreleased");
    }

    #[test]
    fn unbracketed_headings_and_non_release_headings() {
        let sections = parse_sections("## 1.2.3 - 2026-01-02\n\nbody\n\n## Contributing\n\nnope\n");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].version, "1.2.3");
        assert!(
            sections[0].body.contains("nope"),
            "a non-release heading stays inside the section it appears in"
        );
    }

    #[test]
    fn a_missing_changelog_is_a_boundary_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(collect(dir.path(), "/repo").unwrap().is_none());
    }

    /// cas-c736: encoding fixtures for the read site itself.
    ///
    /// `SAMPLE` is re-encoded rather than hand-written per case so the assertion
    /// is about the decode, not about a second copy of the changelog grammar.
    fn write_changelog(dir: &Path, bytes: &[u8]) {
        std::fs::write(dir.join("CHANGELOG.md"), bytes).unwrap();
    }

    fn utf16_bytes(text: &str, little_endian: bool) -> Vec<u8> {
        let mut bytes = if little_endian {
            vec![0xFF, 0xFE]
        } else {
            vec![0xFE, 0xFF]
        };
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&if little_endian {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        bytes
    }

    fn collected_versions(dir: &Path) -> Vec<String> {
        collect(dir, "/repo")
            .expect("a decodable changelog must not fail the pass")
            .expect("CHANGELOG.md is present")
            .iter()
            .map(|doc| doc.id.clone())
            .collect()
    }

    #[test]
    fn a_utf16_le_changelog_is_decoded_rather_than_failing_the_pass() {
        let dir = tempfile::tempdir().unwrap();
        write_changelog(dir.path(), &utf16_bytes(SAMPLE, true));
        let ids = collected_versions(dir.path());
        assert!(
            ids.contains(&"changelog:v2.51.0".to_string()),
            "UTF-16 LE changelog did not parse: {ids:?}"
        );
        assert_eq!(ids.len(), 4, "every release section must survive the decode");
    }

    #[test]
    fn a_utf16_be_changelog_is_decoded_rather_than_failing_the_pass() {
        let dir = tempfile::tempdir().unwrap();
        write_changelog(dir.path(), &utf16_bytes(SAMPLE, false));
        let ids = collected_versions(dir.path());
        assert!(
            ids.contains(&"changelog:v2.49.0".to_string()),
            "UTF-16 BE changelog did not parse: {ids:?}"
        );
    }

    #[test]
    fn a_utf8_bom_does_not_swallow_the_changelogs_first_heading() {
        // The quiet half: with the BOM left on, a file whose FIRST line is a
        // release heading has that heading read as "\u{feff}## [1.0.0] ..." and
        // the whole release is lost. Start the fixture at the heading so the
        // difference is visible rather than hidden behind a preamble.
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"## [1.0.0] - 2026-01-01\n\n### Added\n- first release\n");
        write_changelog(dir.path(), &bytes);
        let ids = collected_versions(dir.path());
        assert_eq!(ids, vec!["changelog:v1.0.0".to_string()]);
    }

    #[test]
    fn an_undecodable_changelog_is_named_with_its_reason() {
        let dir = tempfile::tempdir().unwrap();
        // A UTF-16 BOM followed by an odd byte count: decodable by no rule we
        // are willing to guess at.
        let mut bytes = utf16_bytes("## [1.0.0] - 2026-01-01\n", true);
        bytes.push(0x41);
        write_changelog(dir.path(), &bytes);
        let error = collect(dir.path(), "/repo")
            .expect_err("an undecodable changelog must be reported, not silently empty")
            .to_string();
        assert!(
            error.contains("CHANGELOG.md") && error.contains("odd byte count"),
            "the failure must name the file and the reason, got: {error}"
        );
    }

    /// The real file on this repository, parsed end to end.
    #[test]
    fn parses_this_repositorys_own_changelog() {
        let root = crate::test_paths::workspace_root();
        let Ok(bytes) = std::fs::read(root.join("CHANGELOG.md")) else {
            return; // not a source checkout; nothing to assert against
        };
        // Decoded the same way `collect` does, so this test cannot pass on a
        // path the production reader would refuse.
        let text = crate::daemon::source_text::decode_source(&bytes)
            .expect("this repository's own CHANGELOG must decode");
        let sections = parse_sections(&text);
        assert!(
            sections.len() > 20,
            "expected the real CHANGELOG's many releases, got {}",
            sections.len()
        );
        assert!(sections[0].unreleased || sections[0].date.is_some());
        assert!(
            sections.iter().filter(|s| !s.unreleased).all(|s| {
                s.version
                    .trim_start_matches('v')
                    .starts_with(|c: char| c.is_ascii_digit())
            }),
            "a non-version heading was parsed as a release"
        );
        // Every section must be non-trivial; an empty body means the splitter
        // ate the content.
        let empty = sections.iter().filter(|s| s.body.is_empty()).count();
        assert!(empty <= 1, "{empty} empty sections — splitter lost content");
    }
}
